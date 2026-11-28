//! Single-voice physical aeroacoustic wind source node (`FlueJet` and `BrassReed`)
//! referencing the voice's pre-allocated heap-hosted aero bank pool.

use super::{
    resolve_tracked_pitch_hz, write_copy_module_normalized, AeroBankSlot, NodeInputs, ProcessCtx,
    TailBanks, TrackedPitch, TriggerCtx, TriggerState, VoiceLocks, VoiceModule, TRIG_THRESHOLD,
};
use crate::modular_voice::ModuleParams;
use cadence_core::dsp::cubic::{BrassReedConfig, FlueJetConfig};
use cadence_core::dsp::{
    decay_coeff_60db_ms, mix_seed, sanitize_clamp_f64, sanitize_sample_rate, smooth_step_f32,
    LogSmoother, FAST_RELEASE_MS, TAIL_SILENCE_FLOOR_F64,
};
use cadence_core::rack::{
    AeroMode, AeroModule, BodyGeometry, ModuleKind, ModuleKindTag, ModuleParam, MAX_AEROS_PER_ZONE,
};

const AERO_MIN_HZ: f64 = 16.0;
const AERO_FALLBACK_HZ: f64 = 220.0;
const AERO_MIN_BEAT_PA: f64 = 200.0;
const AERO_MAX_BEAT_PA: f64 = 8_000.0;
const AERO_SEED_SALT: u64 = 0x9E37_79B9_7F4A_7C15;

#[derive(Clone, Copy, Debug)]
pub(in crate::modular_voice) struct AeroNode {
    current: AeroModule,
    pub target: AeroModule,
    bank_idx: u8,
    fixed_hz: LogSmoother,
    p_beat_pa: LogSmoother,
    rng_seed: u64,
    has_trig_in: bool,
    has_cv: bool,
    voice_gate: bool,
    fade_active: bool,
    fade_gain: f64,
    fade_coeff: f64,
    applied_mode: AeroMode,
    applied_geometry: BodyGeometry,
    applied_atoms: u8,
    applied_f0_hz: f64,
    applied_drive: f64,
    applied_velocity: f64,
    applied_embouchure: f32,
    applied_p_beat_pa: f64,
    applied_brightness: f32,
    applied_absorption: f32,
    applied_radiation: f32,
    applied_kerr_semitones: f32,
    applied_triad_amount: f32,
    applied_spread: f32,
    initialized: bool,
}

impl AeroNode {
    #[inline]
    pub(super) fn new(params: AeroModule, sample_rate: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        Self {
            current: params,
            target: params,
            bank_idx: 0,
            fixed_hz: LogSmoother::new(f64::from(params.fixed_hz), 20.0),
            p_beat_pa: LogSmoother::new_clamped(
                f64::from(params.p_beat_pa),
                AERO_MIN_BEAT_PA,
                AERO_MAX_BEAT_PA,
            ),
            rng_seed: AERO_SEED_SALT,
            has_trig_in: false,
            has_cv: false,
            voice_gate: false,
            fade_active: false,
            fade_gain: 1.0,
            fade_coeff: decay_coeff_60db_ms(FAST_RELEASE_MS, sr),
            applied_mode: params.mode,
            applied_geometry: params.geometry,
            applied_atoms: 0,
            applied_f0_hz: 0.0,
            applied_drive: -1.0,
            applied_velocity: -1.0,
            applied_embouchure: -1.0,
            applied_p_beat_pa: -1.0,
            applied_brightness: -1.0,
            applied_absorption: -1.0,
            applied_radiation: -1.0,
            applied_kerr_semitones: f32::NAN,
            applied_triad_amount: -1.0,
            applied_spread: -1.0,
            initialized: false,
        }
    }

    /// True when the blowing gate is closed and the active physical bore tail has decayed
    /// below the audibility floor (or a choke/force-release fade has finished).
    #[inline]
    pub(in crate::modular_voice) fn is_quiet(&self, banks: &[AeroBankSlot]) -> bool {
        if self.fade_active {
            return self.fade_gain <= 0.0;
        }
        if !self.has_trig_in && self.voice_gate {
            return false;
        }
        banks
            .get(usize::from(self.bank_idx))
            .is_none_or(|bank| match self.current.mode {
                AeroMode::FlueJet => bank.flue.is_quiet(),
                AeroMode::BrassReed => bank.reed.is_quiet(),
            })
    }

    /// Approximate instantaneous audibility in `[0.0, 1.0]` for voice-stealing priority.
    #[inline]
    pub(in crate::modular_voice) fn current_level(&self, banks: &[AeroBankSlot]) -> f64 {
        if self.fade_active && self.fade_gain <= 0.0 {
            return 0.0;
        }
        let fade = if self.fade_active {
            self.fade_gain
        } else {
            1.0
        };
        let Some(bank) = banks.get(usize::from(self.bank_idx)) else {
            return 0.0;
        };
        let env = match self.current.mode {
            AeroMode::FlueJet => {
                if bank.flue.gate() {
                    1.0
                } else {
                    bank.flue.energy().max(0.0).sqrt().clamp(0.0, 1.0)
                }
            }
            AeroMode::BrassReed => {
                if bank.reed.gate() {
                    1.0
                } else {
                    bank.reed.bore_audibility().max(0.0).sqrt().clamp(0.0, 1.0)
                }
            }
        };
        env * fade * f64::from(self.current.level).clamp(0.0, 1.0)
    }

    #[inline]
    fn compute_seed(ctx: &TriggerCtx) -> u64 {
        let mixed = mix_seed(ctx.base_seed, u32::from(ctx.mod_id));
        let note_bits = (ctx.target_note_pitch.round().clamp(0.0, 127.0) as u64) << 32;
        let seed = (u64::from(mixed) | note_bits) ^ AERO_SEED_SALT;
        if seed == 0 {
            AERO_SEED_SALT
        } else {
            seed
        }
    }
}

impl VoiceModule for AeroNode {
    #[inline]
    fn tag(&self) -> ModuleKindTag {
        ModuleKindTag::Aero
    }

    #[inline]
    fn target_params(&self) -> ModuleParams {
        ModuleParams::Aero(self.target)
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
            ModuleKind::Aero,
            |k| match k {
                ModuleKind::Aero(m) => Some(m),
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
        self.p_beat_pa.set_target(f64::from(self.target.p_beat_pa));
    }

    #[inline]
    fn update_continuous(&mut self, src: &ModuleKind, module_id: u16, locks: VoiceLocks<'_>) {
        let ModuleKind::Aero(s) = src else {
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
        if !locks.is_module_locked(module_id, ModuleParam::Drive) {
            self.target.drive = s.drive;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Embouchure) {
            self.target.embouchure = s.embouchure;
        }
        if !locks.is_module_locked(module_id, ModuleParam::BeatPressure) {
            self.target.p_beat_pa = s.p_beat_pa;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Brightness) {
            self.target.brightness = s.brightness;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Absorption) {
            self.target.absorption = s.absorption;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Radiation) {
            self.target.radiation = s.radiation;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Kerr) {
            self.target.kerr_semitones = s.kerr_semitones;
        }
        if !locks.is_module_locked(module_id, ModuleParam::TriadAmount) {
            self.target.triad_amount = s.triad_amount;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Spread) {
            self.target.spread = s.spread;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Level) {
            self.target.level = s.level;
        }
        self.sync_target_logs();
    }

    #[inline]
    fn on_trigger(&mut self, ctx: &TriggerCtx, state: &mut TriggerState<'_>) {
        let prev_mode = self.current.mode;
        let aero = self.target;
        if ctx.eff_legato {
            self.current.mode = aero.mode;
            self.current.geometry = aero.geometry;
            self.current.atoms = aero.atoms;
            self.current.pitch_tracking = aero.pitch_tracking;
        } else {
            self.current = aero;
        }

        let sr = sanitize_sample_rate(ctx.sample_rate);
        self.has_trig_in = ctx.has_trig_in;
        self.has_cv = ctx.has_cv;
        self.voice_gate = true;
        self.fade_active = false;
        self.fade_gain = 1.0;
        self.fade_coeff = decay_coeff_60db_ms(FAST_RELEASE_MS, sr);

        if ctx.eff_retarget {
            let bidx = usize::from(self.bank_idx);
            state.next_aero_bank = state.next_aero_bank.max(bidx + 1);
            if let Some(bank) = state.aero_banks.get_mut(bidx) {
                if bank.sync_sample_rate(sr) {
                    self.initialized = false;
                } else if prev_mode != aero.mode {
                    match prev_mode {
                        AeroMode::FlueJet => bank.flue.set_gate(false),
                        AeroMode::BrassReed => bank.reed.set_gate(false),
                    }
                    match aero.mode {
                        AeroMode::FlueJet => bank.flue.reset(),
                        AeroMode::BrassReed => bank.reed.reset(),
                    }
                    self.initialized = false;
                }
            }
            self.fixed_hz.set_target(f64::from(aero.fixed_hz));
            self.p_beat_pa.set_target(f64::from(aero.p_beat_pa));
            if !ctx.eff_legato {
                self.fixed_hz.snap_to_target();
                self.p_beat_pa.snap_to_target();
                self.initialized = false;
            }
            return;
        }

        let bank_idx = state
            .next_aero_bank
            .min(MAX_AEROS_PER_ZONE.saturating_sub(1));
        state.next_aero_bank += 1;
        if let Some(bank) = state.aero_banks.get_mut(bank_idx) {
            bank.reset_for_rate(sr);
        }
        self.bank_idx = bank_idx as u8;
        self.rng_seed = Self::compute_seed(ctx);
        self.fixed_hz = LogSmoother::new(f64::from(aero.fixed_hz), 20.0);
        self.p_beat_pa = LogSmoother::new_clamped(
            f64::from(aero.p_beat_pa),
            AERO_MIN_BEAT_PA,
            AERO_MAX_BEAT_PA,
        );
        self.applied_mode = aero.mode;
        self.applied_geometry = aero.geometry;
        self.applied_atoms = 0;
        self.applied_f0_hz = 0.0;
        self.applied_drive = -1.0;
        self.applied_velocity = -1.0;
        self.applied_embouchure = -1.0;
        self.applied_p_beat_pa = -1.0;
        self.applied_brightness = -1.0;
        self.applied_absorption = -1.0;
        self.applied_radiation = -1.0;
        self.applied_kerr_semitones = f32::NAN;
        self.applied_triad_amount = -1.0;
        self.applied_spread = -1.0;
        self.initialized = false;
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
        smooth_step_f32(&mut self.current.drive, self.target.drive, s32);
        smooth_step_f32(&mut self.current.embouchure, self.target.embouchure, s32);
        self.current.p_beat_pa = self.p_beat_pa.step(s64) as f32;
        smooth_step_f32(&mut self.current.brightness, self.target.brightness, s32);
        smooth_step_f32(&mut self.current.absorption, self.target.absorption, s32);
        smooth_step_f32(&mut self.current.radiation, self.target.radiation, s32);
        smooth_step_f32(
            &mut self.current.kerr_semitones,
            self.target.kerr_semitones,
            s32,
        );
        smooth_step_f32(
            &mut self.current.triad_amount,
            self.target.triad_amount,
            s32,
        );
        smooth_step_f32(&mut self.current.spread, self.target.spread, s32);
        smooth_step_f32(&mut self.current.level, self.target.level, s32);
    }

    #[inline]
    fn process(&mut self, inputs: &NodeInputs, ctx: &mut ProcessCtx<'_>) -> [f64; 2] {
        let aero = self.current;
        let Some(bank) = ctx.aero_banks.get_mut(usize::from(self.bank_idx)) else {
            return [0.0, 0.0];
        };
        if self.fade_active && self.fade_gain <= 0.0 {
            return [0.0, 0.0];
        }

        if !self.initialized || ctx.control_tick {
            let sr = ctx.sample_rate;
            let max_hz = (sr * 0.44).max(AERO_MIN_HZ);
            let base_hz = resolve_tracked_pitch_hz(
                TrackedPitch {
                    mode: aero.pitch_tracking,
                    fixed_hz: self.fixed_hz.current(),
                    note_pitch: ctx.note_pitch,
                    rack_tune: ctx.rack_tune,
                    tune_semitones: aero.tune_semitones,
                    fine_cents: aero.fine_cents,
                    pitch_cv: inputs.pitch_cv,
                },
                AERO_MIN_HZ,
                max_hz,
                AERO_FALLBACK_HZ,
            );
            let eff_drive = if self.has_cv {
                sanitize_clamp_f64(f64::from(aero.drive) + inputs.cv_mono(), 0.0, 1.0, 0.6)
            } else {
                sanitize_clamp_f64(f64::from(aero.drive), 0.0, 1.0, 0.6)
            };
            let eff_vel = sanitize_clamp_f64(ctx.velocity, 0.0, 1.0, 0.8);
            let eff_p_beat = sanitize_clamp_f64(
                self.p_beat_pa.current(),
                AERO_MIN_BEAT_PA,
                AERO_MAX_BEAT_PA,
                1_500.0,
            );
            let needs_update = !self.initialized
                || self.applied_mode != aero.mode
                || self.applied_geometry != aero.geometry
                || self.applied_atoms != aero.atoms
                || (base_hz - self.applied_f0_hz).abs() > (self.applied_f0_hz * 1e-4).max(0.01)
                || (eff_drive - self.applied_drive).abs() > 1e-4
                || (eff_vel - self.applied_velocity).abs() > 1e-4
                || (aero.embouchure - self.applied_embouchure).abs() > 1e-4
                || (eff_p_beat - self.applied_p_beat_pa).abs()
                    > (self.applied_p_beat_pa * 1e-4).max(0.1)
                || (aero.brightness - self.applied_brightness).abs() > 1e-4
                || (aero.absorption - self.applied_absorption).abs() > 1e-4
                || (aero.radiation - self.applied_radiation).abs() > 1e-4
                || !self.applied_kerr_semitones.is_finite()
                || (aero.kerr_semitones - self.applied_kerr_semitones).abs() > 1e-4
                || (aero.triad_amount - self.applied_triad_amount).abs() > 1e-4
                || (aero.spread - self.applied_spread).abs() > 1e-4;
            if needs_update {
                let atoms = usize::from(aero.atoms.clamp(1, 48));
                let geometry = aero.geometry.to_cubic();
                let embouchure = sanitize_clamp_f64(f64::from(aero.embouchure), 0.0, 1.0, 0.55);
                let brightness = sanitize_clamp_f64(f64::from(aero.brightness), 0.0, 1.0, 0.5);
                let absorption = sanitize_clamp_f64(f64::from(aero.absorption), 0.0, 0.5, 0.045);
                let radiation = sanitize_clamp_f64(f64::from(aero.radiation), 0.0, 1.0, 0.2);
                let kerr_semitones =
                    sanitize_clamp_f64(f64::from(aero.kerr_semitones), -12.0, 12.0, 0.0);
                let triad_amount = sanitize_clamp_f64(f64::from(aero.triad_amount), 0.0, 1.0, 0.0);
                let spread = sanitize_clamp_f64(f64::from(aero.spread), 0.0, 1.0, 0.35);
                match aero.mode {
                    AeroMode::FlueJet => bank.flue.configure(FlueJetConfig {
                        fundamental_hz: base_hz,
                        atoms,
                        geometry,
                        drive: eff_drive,
                        velocity: eff_vel,
                        embouchure,
                        brightness,
                        absorption,
                        radiation,
                        kerr_semitones,
                        triad_amount,
                        spread,
                        seed: self.rng_seed,
                        ..FlueJetConfig::default()
                    }),
                    AeroMode::BrassReed => bank.reed.configure(BrassReedConfig {
                        fundamental_hz: base_hz,
                        atoms,
                        geometry,
                        drive: eff_drive,
                        velocity: eff_vel,
                        tongue_stiffness: embouchure,
                        p_beat_pa: eff_p_beat,
                        brightness,
                        radiation,
                        kerr_semitones,
                        triad_amount,
                        spread,
                        ..BrassReedConfig::default()
                    }),
                }
                self.applied_mode = aero.mode;
                self.applied_geometry = aero.geometry;
                self.applied_atoms = aero.atoms;
                self.applied_f0_hz = base_hz;
                self.applied_drive = eff_drive;
                self.applied_velocity = eff_vel;
                self.applied_embouchure = aero.embouchure;
                self.applied_p_beat_pa = eff_p_beat;
                self.applied_brightness = aero.brightness;
                self.applied_absorption = aero.absorption;
                self.applied_radiation = aero.radiation;
                self.applied_kerr_semitones = aero.kerr_semitones;
                self.applied_triad_amount = aero.triad_amount;
                self.applied_spread = aero.spread;
                self.initialized = true;
            }
        }

        let gate = if self.fade_active {
            false
        } else if self.has_trig_in {
            inputs.trig_in >= TRIG_THRESHOLD
        } else {
            self.voice_gate
        };

        let raw = match aero.mode {
            AeroMode::FlueJet => {
                bank.flue.set_gate(gate);
                bank.flue.step()
            }
            AeroMode::BrassReed => {
                bank.reed.set_gate(gate);
                bank.reed.step()
            }
        };

        let mut gain = sanitize_clamp_f64(f64::from(aero.level), 0.0, 1.0, 0.85);
        if self.fade_active {
            gain *= self.fade_gain;
            self.fade_gain *= self.fade_coeff;
            if self.fade_gain < TAIL_SILENCE_FLOOR_F64 {
                self.fade_gain = 0.0;
                match aero.mode {
                    AeroMode::FlueJet => bank.flue.reset(),
                    AeroMode::BrassReed => bank.reed.reset(),
                }
            }
        }
        [raw[0] * gain, raw[1] * gain]
    }

    #[inline]
    fn on_note_off(&mut self, _has_lifetime_envs: bool) {
        self.voice_gate = false;
    }

    #[inline]
    fn on_force_release(&mut self, _sample_rate: f64, rel_coeff: f64) {
        self.voice_gate = false;
        self.fade_active = true;
        self.fade_coeff = rel_coeff;
    }

    #[inline]
    fn on_choke(&mut self, _sample_rate: f64, choke_coeff: f64) {
        self.voice_gate = false;
        self.fade_active = true;
        self.fade_coeff = choke_coeff;
    }

    #[inline]
    fn is_tail_quiet(&self, banks: TailBanks<'_>) -> bool {
        self.is_quiet(banks.aero_banks)
    }
}
