//! Physical 48-mode nonlinear acoustic body voice node (`ModalBank` + `StrikePulse`)
//! referencing the voice's pre-allocated heap-hosted modal bank pool.

use super::{
    resolve_tracked_pitch_hz, write_copy_module_normalized, ModalBankSlot, NodeInputs, ProcessCtx,
    TailBanks, TrackedPitch, TriggerCtx, TriggerState, VoiceLocks, VoiceModule, TRIG_THRESHOLD,
};
use crate::modular_voice::ModuleParams;
use cadence_core::dsp::cubic::{ModalConfig, MAX_ATOMS, MODAL_MAX_HZ, MODAL_MIN_HZ};
use cadence_core::dsp::{
    decay_coeff_60db_ms, mix_stereo_f64, sanitize_clamp_f64, sanitize_sample_rate, smooth_step_f32,
    stereo_ms_encode_f64, LogSmoother, FAST_RELEASE_MS, TAIL_SILENCE_FLOOR_F64,
};
use cadence_core::rack::{
    BodyGeometry, ModalModule, ModuleKind, ModuleKindTag, ModuleParam, PairTopology, TriadAxis,
    MAX_MODALS_PER_ZONE,
};

const MODAL_FALLBACK_HZ: f64 = 220.0;
const MODAL_NYQUIST_FUNDAMENTAL_RATIO: f64 = 0.36;
const MODAL_WAVE_ACTIVE_EPS: f64 = 1e-12;

#[derive(Clone, Copy, Debug)]
pub(in crate::modular_voice) struct ModalNode {
    current: ModalModule,
    pub target: ModalModule,
    bank_idx: u8,
    fixed_hz: LogSmoother,
    has_in: bool,
    has_trig_in: bool,
    has_cv: bool,
    prev_trig: f64,
    velocity: f64,
    fade_active: bool,
    fade_gain: f64,
    fade_coeff: f64,
    applied_geometry: BodyGeometry,
    applied_atoms: u8,
    applied_pair_topology: PairTopology,
    applied_triad_axis: TriadAxis,
    applied_f0_hz: f64,
    applied_absorption: f64,
    applied_radiation: f64,
    applied_brightness: f64,
    applied_headroom: f64,
    applied_kerr_semitones: f64,
    applied_coupling: f64,
    applied_triad_amount: f64,
    applied_spread: f64,
    initialized: bool,
    #[cfg(test)]
    configure_count: u32,
    #[cfg(test)]
    glide_update_count: u32,
}

impl ModalNode {
    #[inline]
    pub(super) fn new(params: ModalModule, sample_rate: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        Self {
            current: params,
            target: params,
            bank_idx: 0,
            fixed_hz: LogSmoother::new(f64::from(params.fixed_hz), f64::from(MODAL_MIN_HZ)),
            has_in: false,
            has_trig_in: false,
            has_cv: false,
            prev_trig: 0.0,
            velocity: 0.8,
            fade_active: false,
            fade_gain: 1.0,
            fade_coeff: decay_coeff_60db_ms(FAST_RELEASE_MS, sr),
            applied_geometry: params.geometry,
            applied_atoms: 0,
            applied_pair_topology: params.pair_topology,
            applied_triad_axis: params.triad_axis,
            applied_f0_hz: 0.0,
            applied_absorption: -1.0,
            applied_radiation: -1.0,
            applied_brightness: -1.0,
            applied_headroom: -1.0,
            applied_kerr_semitones: f64::NAN,
            applied_coupling: -1.0,
            applied_triad_amount: -1.0,
            applied_spread: -1.0,
            initialized: false,
            #[cfg(test)]
            configure_count: 0,
            #[cfg(test)]
            glide_update_count: 0,
        }
    }

    /// True when any active mallet strike has finished and the physical modal bank energy
    /// has decayed below `MODAL_SILENCE_ENERGY` (or a choke/force-release fade has finished).
    #[inline]
    pub(in crate::modular_voice) fn is_quiet(&self, banks: &[ModalBankSlot]) -> bool {
        if self.fade_active {
            return self.fade_gain <= 0.0;
        }
        banks
            .get(usize::from(self.bank_idx))
            .is_none_or(|slot| !slot.strike.is_active() && slot.bank.is_quiet())
    }

    /// Approximate instantaneous audibility in `[0.0, 1.0]` for voice-stealing priority.
    #[inline]
    pub(in crate::modular_voice) fn current_level(&self, banks: &[ModalBankSlot]) -> f64 {
        if self.fade_active && self.fade_gain <= 0.0 {
            return 0.0;
        }
        let fade = if self.fade_active {
            self.fade_gain
        } else {
            1.0
        };
        let Some(slot) = banks.get(usize::from(self.bank_idx)) else {
            return 0.0;
        };
        let env = if slot.strike.is_active() {
            1.0
        } else {
            slot.bank.energy().max(0.0).sqrt().clamp(0.0, 1.0)
        };
        env * fade
    }

    #[inline]
    fn trigger_strike(modal: ModalModule, velocity: f64, slot: &mut ModalBankSlot) {
        let strike_drive = sanitize_clamp_f64(f64::from(modal.strike_drive), 0.0, 2.0, 0.75);
        if strike_drive > 0.0 {
            let hardness = sanitize_clamp_f64(f64::from(modal.strike_hardness), 0.0, 1.0, 0.5);
            slot.strike.trigger(hardness, strike_drive, velocity);
        }
    }

    #[cfg(test)]
    pub(in crate::modular_voice) fn applied_f0_hz(&self) -> f64 {
        self.applied_f0_hz
    }

    #[cfg(test)]
    pub(in crate::modular_voice) fn configure_count(&self) -> u32 {
        self.configure_count
    }

    #[cfg(test)]
    pub(in crate::modular_voice) fn glide_update_count(&self) -> u32 {
        self.glide_update_count
    }

    #[cfg(test)]
    pub(in crate::modular_voice) fn bank_idx(&self) -> usize {
        usize::from(self.bank_idx)
    }
}

impl VoiceModule for ModalNode {
    #[inline]
    fn tag(&self) -> ModuleKindTag {
        ModuleKindTag::Modal
    }

    #[inline]
    fn target_params(&self) -> ModuleParams {
        ModuleParams::Modal(self.target)
    }

    #[inline]
    fn write_target_normalized(
        &mut self,
        param: ModuleParam,
        norm: f32,
        total_frames: usize,
    ) -> bool {
        write_copy_module_normalized(
            &mut self.target,
            ModuleKind::Modal,
            |k| match k {
                ModuleKind::Modal(m) => Some(m),
                _ => None,
            },
            param,
            norm,
            total_frames,
        )
    }

    #[inline]
    fn sync_target_logs(&mut self) {
        self.fixed_hz.set_target(f64::from(self.target.fixed_hz));
    }

    #[inline]
    fn update_continuous(&mut self, src: &ModuleKind, module_id: u16, locks: VoiceLocks<'_>) {
        let ModuleKind::Modal(s) = src else {
            return;
        };
        if !locks.is_module_locked(module_id, ModuleParam::Tune) {
            self.target.tune_semitones = s.tune_semitones;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Fine) {
            self.target.fine_cents = s.fine_cents;
        }
        if !locks.is_module_locked(module_id, ModuleParam::FixedHz) {
            self.target.fixed_hz = s.fixed_hz;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Absorption) {
            self.target.absorption = s.absorption;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Radiation) {
            self.target.radiation = s.radiation;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Brightness) {
            self.target.brightness = s.brightness;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Headroom) {
            self.target.headroom = s.headroom;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Kerr) {
            self.target.kerr_semitones = s.kerr_semitones;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Coupling) {
            self.target.coupling = s.coupling;
        }
        if !locks.is_module_locked(module_id, ModuleParam::TriadAmount) {
            self.target.triad_amount = s.triad_amount;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Spread) {
            self.target.spread = s.spread;
        }
        if !locks.is_module_locked(module_id, ModuleParam::StrikeDrive) {
            self.target.strike_drive = s.strike_drive;
        }
        if !locks.is_module_locked(module_id, ModuleParam::StrikeHardness) {
            self.target.strike_hardness = s.strike_hardness;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Mix) {
            self.target.mix = s.mix;
        }
        self.sync_target_logs();
    }

    #[inline]
    fn on_trigger(&mut self, ctx: &TriggerCtx, state: &mut TriggerState<'_>) {
        let modal = self.target;
        if ctx.eff_legato {
            self.current.geometry = modal.geometry;
            self.current.atoms = modal.atoms;
            self.current.pitch_tracking = modal.pitch_tracking;
            self.current.pair_topology = modal.pair_topology;
            self.current.triad_axis = modal.triad_axis;
        } else {
            self.current = modal;
        }

        let sr = sanitize_sample_rate(ctx.sample_rate);
        self.has_in = ctx.has_in;
        self.has_trig_in = ctx.has_trig_in;
        self.has_cv = ctx.has_cv;
        self.velocity = sanitize_clamp_f64(f64::from(ctx.vel), 0.0, 1.0, 0.8);
        self.fade_active = false;
        self.fade_gain = 1.0;
        self.fade_coeff = decay_coeff_60db_ms(FAST_RELEASE_MS, sr);

        if ctx.eff_retarget {
            let bidx = usize::from(self.bank_idx);
            state.next_modal_bank = state.next_modal_bank.max(bidx + 1);
            self.fixed_hz.set_target(f64::from(modal.fixed_hz));
            if let Some(slot) = state.modal_banks.get_mut(bidx) {
                if slot.sync_sample_rate(sr) {
                    self.initialized = false;
                }
                if !ctx.eff_legato {
                    self.fixed_hz.snap_to_target();
                    self.prev_trig = 0.0;
                    self.initialized = false;
                    if !ctx.has_trig_in {
                        Self::trigger_strike(modal, self.velocity, slot);
                    }
                }
            } else if !ctx.eff_legato {
                self.fixed_hz.snap_to_target();
                self.prev_trig = 0.0;
                self.initialized = false;
            }
            return;
        }

        let bank_idx = state
            .next_modal_bank
            .min(MAX_MODALS_PER_ZONE.saturating_sub(1));
        state.next_modal_bank += 1;
        self.bank_idx = bank_idx as u8;
        self.prev_trig = 0.0;
        self.fixed_hz = LogSmoother::new(f64::from(modal.fixed_hz), f64::from(MODAL_MIN_HZ));
        if let Some(slot) = state.modal_banks.get_mut(bank_idx) {
            slot.reset_for_rate(sr);
            if !ctx.has_trig_in {
                Self::trigger_strike(modal, self.velocity, slot);
            }
        }
        self.applied_geometry = modal.geometry;
        self.applied_atoms = 0;
        self.applied_pair_topology = modal.pair_topology;
        self.applied_triad_axis = modal.triad_axis;
        self.applied_f0_hz = 0.0;
        self.applied_absorption = -1.0;
        self.applied_radiation = -1.0;
        self.applied_brightness = -1.0;
        self.applied_headroom = -1.0;
        self.applied_kerr_semitones = f64::NAN;
        self.applied_coupling = -1.0;
        self.applied_triad_amount = -1.0;
        self.applied_spread = -1.0;
        self.initialized = false;
        #[cfg(test)]
        {
            self.configure_count = 0;
            self.glide_update_count = 0;
        }
    }

    #[inline]
    fn smooth_step(&mut self, s64: f64) {
        let s32 = s64 as f32;
        smooth_step_f32(
            &mut self.current.tune_semitones,
            self.target.tune_semitones,
            s32,
        );
        smooth_step_f32(&mut self.current.fine_cents, self.target.fine_cents, s32);
        self.current.fixed_hz = self.fixed_hz.step(s64) as f32;
        smooth_step_f32(&mut self.current.absorption, self.target.absorption, s32);
        smooth_step_f32(&mut self.current.radiation, self.target.radiation, s32);
        smooth_step_f32(&mut self.current.brightness, self.target.brightness, s32);
        smooth_step_f32(&mut self.current.headroom, self.target.headroom, s32);
        smooth_step_f32(
            &mut self.current.kerr_semitones,
            self.target.kerr_semitones,
            s32,
        );
        smooth_step_f32(&mut self.current.coupling, self.target.coupling, s32);
        smooth_step_f32(
            &mut self.current.triad_amount,
            self.target.triad_amount,
            s32,
        );
        smooth_step_f32(&mut self.current.spread, self.target.spread, s32);
        smooth_step_f32(
            &mut self.current.strike_drive,
            self.target.strike_drive,
            s32,
        );
        smooth_step_f32(
            &mut self.current.strike_hardness,
            self.target.strike_hardness,
            s32,
        );
        smooth_step_f32(&mut self.current.mix, self.target.mix, s32);
    }

    #[inline]
    fn process(&mut self, inputs: &NodeInputs, ctx: &mut ProcessCtx<'_>) -> [f64; 2] {
        let modal = self.current;
        let Some(slot) = ctx.modal_banks.get_mut(usize::from(self.bank_idx)) else {
            return inputs.in_audio;
        };
        if self.fade_active && self.fade_gain <= 0.0 {
            return [0.0, 0.0];
        }

        if !self.initialized || ctx.control_tick {
            let sr = ctx.sample_rate;
            let max_hz = (sr * MODAL_NYQUIST_FUNDAMENTAL_RATIO)
                .min(f64::from(MODAL_MAX_HZ))
                .max(f64::from(MODAL_MIN_HZ));
            let f0_hz = resolve_tracked_pitch_hz(
                TrackedPitch {
                    mode: modal.pitch_tracking,
                    fixed_hz: self.fixed_hz.current(),
                    note_pitch: ctx.note_pitch,
                    rack_tune: ctx.rack_tune,
                    tune_semitones: modal.tune_semitones,
                    fine_cents: modal.fine_cents,
                    pitch_cv: inputs.pitch_cv,
                },
                f64::from(MODAL_MIN_HZ),
                max_hz,
                MODAL_FALLBACK_HZ,
            );
            let atoms = modal.atoms.clamp(1, MAX_ATOMS as u8);
            let eff_bright = if self.has_cv {
                sanitize_clamp_f64(
                    f64::from(modal.brightness) + inputs.cv_mono(),
                    0.0,
                    1.0,
                    0.5,
                )
            } else {
                sanitize_clamp_f64(f64::from(modal.brightness), 0.0, 1.0, 0.5)
            };
            let absorption = sanitize_clamp_f64(f64::from(modal.absorption), 0.0, 0.5, 0.035);
            let radiation = sanitize_clamp_f64(f64::from(modal.radiation), 0.0, 1.0, 0.12);
            let headroom = sanitize_clamp_f64(f64::from(modal.headroom), 0.0, 4.0, 0.5);
            let kerr_semitones =
                sanitize_clamp_f64(f64::from(modal.kerr_semitones), -12.0, 12.0, 0.0);
            let coupling = sanitize_clamp_f64(f64::from(modal.coupling), 0.0, 1.0, 0.0);
            let triad_amount = sanitize_clamp_f64(f64::from(modal.triad_amount), 0.0, 1.0, 0.0);
            let spread = sanitize_clamp_f64(f64::from(modal.spread), 0.0, 1.0, 0.35);

            let f0_changed =
                (f0_hz - self.applied_f0_hz).abs() > (self.applied_f0_hz * 1e-4).max(0.01);
            let other_changed = !self.initialized
                || self.applied_geometry != modal.geometry
                || self.applied_atoms != atoms
                || self.applied_pair_topology != modal.pair_topology
                || self.applied_triad_axis != modal.triad_axis
                || (absorption - self.applied_absorption).abs() > 1e-4
                || (radiation - self.applied_radiation).abs() > 1e-4
                || (eff_bright - self.applied_brightness).abs() > 1e-4
                || (headroom - self.applied_headroom).abs() > 1e-4
                || !self.applied_kerr_semitones.is_finite()
                || (kerr_semitones - self.applied_kerr_semitones).abs() > 1e-4
                || (coupling - self.applied_coupling).abs() > 1e-4
                || (triad_amount - self.applied_triad_amount).abs() > 1e-4
                || (spread - self.applied_spread).abs() > 1e-4;

            if other_changed {
                slot.bank.configure(ModalConfig {
                    fundamental_hz: f0_hz,
                    atoms: usize::from(atoms),
                    geometry: modal.geometry.to_cubic(),
                    pair_topology: modal.pair_topology.to_cubic(),
                    coupling,
                    absorption,
                    radiation,
                    brightness: eff_bright,
                    headroom,
                    kerr_semitones,
                    triad_amount,
                    triad_axis: modal.triad_axis.to_cubic(),
                    spread,
                });
                self.applied_geometry = modal.geometry;
                self.applied_atoms = atoms;
                self.applied_pair_topology = modal.pair_topology;
                self.applied_triad_axis = modal.triad_axis;
                self.applied_f0_hz = f0_hz;
                self.applied_absorption = absorption;
                self.applied_radiation = radiation;
                self.applied_brightness = eff_bright;
                self.applied_headroom = headroom;
                self.applied_kerr_semitones = kerr_semitones;
                self.applied_coupling = coupling;
                self.applied_triad_amount = triad_amount;
                self.applied_spread = spread;
                self.initialized = true;
                #[cfg(test)]
                {
                    self.configure_count = self.configure_count.saturating_add(1);
                }
            } else if f0_changed {
                slot.bank.update_gliding_freq(f0_hz);
                self.applied_f0_hz = f0_hz;
                #[cfg(test)]
                {
                    self.glide_update_count = self.glide_update_count.saturating_add(1);
                }
            }
        }

        let trig = inputs.trig_in;
        if self.has_trig_in
            && !self.fade_active
            && self.prev_trig < TRIG_THRESHOLD
            && trig >= TRIG_THRESHOLD
        {
            Self::trigger_strike(modal, self.velocity, slot);
        }
        self.prev_trig = trig;

        let [in_re, in_im] = stereo_ms_encode_f64(inputs.in_audio);
        let wave_active = self.has_in && (in_re.abs() + in_im.abs() > MODAL_WAVE_ACTIVE_EPS);
        let (strike_active, strike_pulse) = slot.strike.step_midpoint();
        let wet = slot.bank.step_wave_and_strike(
            wave_active,
            in_re,
            in_im,
            strike_active,
            strike_pulse,
            slot.strike.drive(),
            slot.strike.velocity(),
            slot.strike.strike_time(),
        );

        let mut out = mix_stereo_f64(inputs.in_audio, wet, f64::from(modal.mix));
        if self.fade_active {
            out = [out[0] * self.fade_gain, out[1] * self.fade_gain];
            self.fade_gain *= self.fade_coeff;
            if self.fade_gain <= TAIL_SILENCE_FLOOR_F64 {
                self.fade_gain = 0.0;
                slot.bank.reset();
                slot.strike.reset();
            }
        }
        out
    }

    #[inline]
    fn on_force_release(&mut self, _sample_rate: f64, rel_coeff: f64) {
        self.fade_active = true;
        self.fade_coeff = rel_coeff;
    }

    #[inline]
    fn on_choke(&mut self, _sample_rate: f64, choke_coeff: f64) {
        self.fade_active = true;
        self.fade_coeff = choke_coeff;
    }

    #[inline]
    fn is_tail_quiet(&self, banks: TailBanks<'_>) -> bool {
        self.is_quiet(banks.modal_banks)
    }
}
