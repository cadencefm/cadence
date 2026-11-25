//! Complex State-Kerr Cubic-Atom Synthesizer voice node referencing the voice's
//! heap-hosted [`MobiusSynth`] bank pool (zero RT allocations).

use super::{
    resolve_tracked_pitch_hz, write_copy_module_normalized, NodeInputs, ProcessCtx, TrackedPitch,
    TriggerCtx, TriggerState, VoiceLocks, VoiceModule, TRIG_THRESHOLD,
};
use crate::modular_voice::ModuleParams;
use cadence_core::dsp::{
    sanitize_clamp_f32, sanitize_clamp_f64, smooth_step_f32, LogSmoother, MobiusSynth,
    MobiusSynthParams, ModalResonator, MOBIUS_MAX_HZ, MOBIUS_MAX_STIFFNESS, MOBIUS_MIN_HZ,
    MOBIUS_MIN_STIFFNESS, NYQUIST_SAFE_RATIO,
};
use cadence_core::rack::{
    MobiusModule, ModuleKind, ModuleKindTag, ModuleParam, MAX_MOBIUS_PER_ZONE,
};

#[derive(Clone, Copy, Debug)]
pub(in crate::modular_voice) struct MobiusNode {
    current: MobiusModule,
    pub target: MobiusModule,
    bank_idx: u8,
    fixed_hz: LogSmoother,
    stiffness: LogSmoother,
    has_cv: bool,
    has_trig_in: bool,
    prev_trig: bool,
    pending_note_off: bool,
    pending_host_fade_coeff: f64,
    applied_params: Option<MobiusSynthParams>,
    initialized: bool,
}

impl MobiusNode {
    #[inline]
    pub(super) fn new(params: MobiusModule) -> Self {
        Self {
            current: params,
            target: params,
            bank_idx: 0,
            fixed_hz: LogSmoother::new(f64::from(params.fixed_hz), 20.0),
            stiffness: LogSmoother::new_clamped(
                f64::from(params.stiffness),
                f64::from(MOBIUS_MIN_STIFFNESS),
                f64::from(MOBIUS_MAX_STIFFNESS),
            ),
            has_cv: false,
            has_trig_in: false,
            prev_trig: false,
            pending_note_off: false,
            pending_host_fade_coeff: 0.0,
            applied_params: None,
            initialized: false,
        }
    }

    #[inline]
    pub(in crate::modular_voice) fn bank_idx(&self) -> usize {
        usize::from(self.bank_idx)
    }

    /// Lower bound of smoothed/target stiffness (used to size pure cubic ring-out tail budget).
    #[inline]
    pub(in crate::modular_voice) fn min_effective_stiffness(&self) -> f64 {
        let xi0 = self.stiffness.current().min(self.stiffness.target());
        let tilt = f64::from(self.current.stiffness_tilt.min(self.target.stiffness_tilt));
        let factor = if tilt < 0.0 { 2.0_f64.powf(tilt) } else { 1.0 };
        (xi0 * factor).clamp(
            f64::from(MOBIUS_MIN_STIFFNESS),
            f64::from(MOBIUS_MAX_STIFFNESS),
        )
    }

    #[inline]
    fn build_dsp_params(
        &self,
        m: MobiusModule,
        base_hz: f64,
        brightness: f64,
    ) -> MobiusSynthParams {
        let mut p = m.to_dsp_params(base_hz, brightness);
        p.stiffness = self.stiffness.current();
        p
    }
}

impl VoiceModule for MobiusNode {
    #[inline]
    fn tag(&self) -> ModuleKindTag {
        ModuleKindTag::Mobius
    }

    #[inline]
    fn target_params(&self) -> ModuleParams {
        ModuleParams::Mobius(self.target)
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
            ModuleKind::Mobius,
            |k| match k {
                ModuleKind::Mobius(m) => Some(m),
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
        self.stiffness.set_target(f64::from(self.target.stiffness));
    }

    #[inline]
    fn update_continuous(&mut self, src: &ModuleKind, module_id: u16, locks: VoiceLocks<'_>) {
        let ModuleKind::Mobius(s) = src else {
            return;
        };
        if !locks.is_module_locked(module_id, ModuleParam::Brightness) {
            self.target.brightness = s.brightness;
        }
        if !locks.is_module_locked(module_id, ModuleParam::MobiusShape) {
            self.target.shape = s.shape;
        }
        if !locks.is_module_locked(module_id, ModuleParam::MobiusChirality) {
            self.target.chirality = s.chirality;
        }
        if !locks.is_module_locked(module_id, ModuleParam::MobiusTexture) {
            self.target.texture = s.texture;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Sustain) {
            self.target.sustain = s.sustain;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Structure) {
            self.target.structure = s.structure;
        }
        if !locks.is_module_locked(module_id, ModuleParam::MobiusStiffness) {
            self.target.stiffness = s.stiffness;
        }
        if !locks.is_module_locked(module_id, ModuleParam::MobiusStiffnessTilt) {
            self.target.stiffness_tilt = s.stiffness_tilt;
        }
        if !locks.is_module_locked(module_id, ModuleParam::MobiusKerr) {
            self.target.kerr_semitones = s.kerr_semitones;
        }
        if !locks.is_module_locked(module_id, ModuleParam::MobiusKerrSpread) {
            self.target.kerr_spread = s.kerr_spread;
        }
        if !locks.is_module_locked(module_id, ModuleParam::MobiusCoupling) {
            self.target.coupling = s.coupling;
        }
        if !locks.is_module_locked(module_id, ModuleParam::MobiusDistance) {
            self.target.distance_ms = s.distance_ms;
        }
        if !locks.is_module_locked(module_id, ModuleParam::MobiusSympathy) {
            self.target.sympathy = s.sympathy;
        }
        if !locks.is_module_locked(module_id, ModuleParam::MobiusFeedback) {
            self.target.feedback = s.feedback;
        }
        if !locks.is_module_locked(module_id, ModuleParam::MobiusFbCycles) {
            self.target.fb_cycles = s.fb_cycles;
        }
        if !locks.is_module_locked(module_id, ModuleParam::MobiusFbMs) {
            self.target.fb_ms = s.fb_ms;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Tune) {
            self.target.tune_semitones = s.tune_semitones;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Fine) {
            self.target.fine_cents = s.fine_cents;
        }
        if !locks.is_module_locked(module_id, ModuleParam::FixedHz) {
            self.target.fixed_hz = s.fixed_hz;
        }
        if !locks.is_module_locked(module_id, ModuleParam::KeyTrack) {
            self.target.key_tracking = s.key_tracking;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Spread) {
            self.target.stereo_width = s.stereo_width;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Mix) {
            self.target.mix = s.mix;
        }
        self.sync_target_logs();
    }

    #[inline]
    fn on_trigger(&mut self, ctx: &TriggerCtx, state: &mut TriggerState<'_>) {
        let m = self.target;
        if ctx.eff_legato {
            self.current.exciter = m.exciter;
            self.current.attack_ms = m.attack_ms;
            self.current.decay_ms = m.decay_ms;
            self.current.atoms = m.atoms;
            self.current.dispersion = m.dispersion;
            self.current.topology = m.topology;
            self.current.fb_lock = m.fb_lock;
            self.current.pitch_tracking = m.pitch_tracking;
        } else {
            self.current = m;
        }

        let max_hz = (ctx.sample_rate * NYQUIST_SAFE_RATIO).min(f64::from(MOBIUS_MAX_HZ));
        let base_hz = resolve_tracked_pitch_hz(
            TrackedPitch {
                mode: self.current.pitch_tracking,
                fixed_hz: f64::from(self.current.fixed_hz),
                note_pitch: ctx.target_note_pitch,
                rack_tune: ctx.target_rack_tune,
                tune_semitones: self.current.tune_semitones,
                fine_cents: self.current.fine_cents,
                pitch_cv: 0.0,
            },
            f64::from(MOBIUS_MIN_HZ),
            max_hz,
            220.0,
        );

        if ctx.eff_retarget {
            let bidx = usize::from(self.bank_idx);
            state.next_mobius_bank = state.next_mobius_bank.max(bidx + 1);
            self.has_cv = ctx.has_cv;
            self.has_trig_in = ctx.has_trig_in;
            self.pending_note_off = false;
            self.pending_host_fade_coeff = 0.0;
            self.fixed_hz.set_target(f64::from(m.fixed_hz));
            self.stiffness.set_target(f64::from(m.stiffness));
            if !ctx.eff_legato {
                self.fixed_hz.snap_to_target();
                self.stiffness.snap_to_target();
            }
            let dsp_params = self.build_dsp_params(
                self.current,
                base_hz,
                sanitize_clamp_f64(f64::from(self.current.brightness), 0.0, 1.0, 0.72),
            );
            if let Some(bank) = state.mobius_banks.get_mut(bidx) {
                let _ = bank.set_sample_rate(ctx.sample_rate);
                bank.trigger(
                    f64::from(ctx.vel),
                    &dsp_params,
                    ctx.eff_legato,
                    ctx.phase_reset,
                );
            }
            self.applied_params = Some(dsp_params);
            self.initialized = true;
            return;
        }

        let bank_idx = state
            .next_mobius_bank
            .min(MAX_MOBIUS_PER_ZONE.saturating_sub(1));
        state.next_mobius_bank += 1;
        self.bank_idx = bank_idx as u8;
        self.fixed_hz = LogSmoother::new(f64::from(m.fixed_hz), 20.0);
        self.stiffness = LogSmoother::new_clamped(
            f64::from(m.stiffness),
            f64::from(MOBIUS_MIN_STIFFNESS),
            f64::from(MOBIUS_MAX_STIFFNESS),
        );
        self.has_cv = ctx.has_cv;
        self.has_trig_in = ctx.has_trig_in;
        self.prev_trig = false;
        self.pending_note_off = false;
        self.pending_host_fade_coeff = 0.0;

        let dsp_params = self.build_dsp_params(
            self.current,
            base_hz,
            sanitize_clamp_f64(f64::from(self.current.brightness), 0.0, 1.0, 0.72),
        );
        if let Some(bank) = state.mobius_banks.get_mut(bank_idx) {
            bank.reset(ctx.sample_rate);
            bank.trigger(f64::from(ctx.vel), &dsp_params, false, ctx.phase_reset);
        }
        self.applied_params = Some(dsp_params);
        self.initialized = true;
    }

    #[inline]
    fn smooth_step(&mut self, s64: f64) {
        let s32 = s64 as f32;
        smooth_step_f32(&mut self.current.brightness, self.target.brightness, s32);
        smooth_step_f32(&mut self.current.shape, self.target.shape, s32);
        smooth_step_f32(&mut self.current.chirality, self.target.chirality, s32);
        smooth_step_f32(&mut self.current.texture, self.target.texture, s32);
        smooth_step_f32(&mut self.current.sustain, self.target.sustain, s32);
        smooth_step_f32(&mut self.current.structure, self.target.structure, s32);
        self.current.stiffness = self.stiffness.step(s64) as f32;
        smooth_step_f32(
            &mut self.current.stiffness_tilt,
            self.target.stiffness_tilt,
            s32,
        );
        smooth_step_f32(
            &mut self.current.kerr_semitones,
            self.target.kerr_semitones,
            s32,
        );
        smooth_step_f32(&mut self.current.kerr_spread, self.target.kerr_spread, s32);
        smooth_step_f32(&mut self.current.coupling, self.target.coupling, s32);
        smooth_step_f32(&mut self.current.distance_ms, self.target.distance_ms, s32);
        smooth_step_f32(&mut self.current.sympathy, self.target.sympathy, s32);
        smooth_step_f32(&mut self.current.feedback, self.target.feedback, s32);
        smooth_step_f32(&mut self.current.fb_cycles, self.target.fb_cycles, s32);
        smooth_step_f32(&mut self.current.fb_ms, self.target.fb_ms, s32);
        smooth_step_f32(
            &mut self.current.tune_semitones,
            self.target.tune_semitones,
            s32,
        );
        smooth_step_f32(&mut self.current.fine_cents, self.target.fine_cents, s32);
        self.current.fixed_hz = self.fixed_hz.step(s64) as f32;
        smooth_step_f32(
            &mut self.current.key_tracking,
            self.target.key_tracking,
            s32,
        );
        smooth_step_f32(
            &mut self.current.stereo_width,
            self.target.stereo_width,
            s32,
        );
        smooth_step_f32(&mut self.current.mix, self.target.mix, s32);
    }

    #[inline]
    fn process(&mut self, inputs: &NodeInputs, ctx: &mut ProcessCtx<'_>) -> [f64; 2] {
        let m = self.current;
        let Some(bank) = ctx.mobius_banks.get_mut(usize::from(self.bank_idx)) else {
            return inputs.in_audio;
        };

        if self.pending_host_fade_coeff > 0.0 {
            bank.begin_host_fade(self.pending_host_fade_coeff);
            self.pending_host_fade_coeff = 0.0;
            self.pending_note_off = false;
        } else if self.pending_note_off {
            bank.note_off();
            self.pending_note_off = false;
        }

        let max_hz = (ctx.sample_rate * NYQUIST_SAFE_RATIO).min(f64::from(MOBIUS_MAX_HZ));
        let eff_bright = if self.has_cv {
            f64::from(sanitize_clamp_f32(
                m.brightness + inputs.cv_mono() as f32,
                0.0,
                1.0,
                0.72,
            ))
        } else {
            f64::from(sanitize_clamp_f32(m.brightness, 0.0, 1.0, 0.72))
        };

        let mut recomputed: Option<MobiusSynthParams> = None;
        if !self.initialized || ctx.control_tick || inputs.pitch_cv != 0.0 || self.has_cv {
            let base_hz = resolve_tracked_pitch_hz(
                TrackedPitch {
                    mode: m.pitch_tracking,
                    fixed_hz: self.fixed_hz.current(),
                    note_pitch: ctx.note_pitch,
                    rack_tune: ctx.rack_tune,
                    tune_semitones: m.tune_semitones,
                    fine_cents: m.fine_cents,
                    pitch_cv: inputs.pitch_cv,
                },
                f64::from(MOBIUS_MIN_HZ),
                max_hz,
                220.0,
            );
            let next_params = self.build_dsp_params(m, base_hz, eff_bright);
            if self.applied_params != Some(next_params) {
                bank.set_parameters(&next_params);
                self.applied_params = Some(next_params);
            }
            recomputed = Some(next_params);
            self.initialized = true;
        }

        if self.has_trig_in {
            let high = inputs.trig_in >= TRIG_THRESHOLD;
            if high && !self.prev_trig {
                let p = recomputed
                    .or(self.applied_params)
                    .unwrap_or_else(|| self.build_dsp_params(m, 220.0, eff_bright));
                bank.retrigger_exciter(ctx.velocity, &p);
            }
            self.prev_trig = high;
        }

        let (out, rad) =
            bank.process_stereo_bridged(inputs.in_audio, f64::from(m.mix), ctx.bridge_return);
        ctx.bridge_rad = ctx.bridge_rad + rad;
        out
    }

    #[inline]
    fn on_note_off(&mut self, _has_lifetime_envs: bool) {
        self.pending_note_off = true;
    }

    #[inline]
    fn on_force_release(&mut self, _sample_rate: f64, rel_coeff: f64) {
        self.pending_host_fade_coeff = rel_coeff;
    }

    #[inline]
    fn on_choke(&mut self, _sample_rate: f64, choke_coeff: f64) {
        self.pending_host_fade_coeff = choke_coeff;
    }

    #[inline]
    fn is_tail_quiet(
        &self,
        _resonator_banks: &[ModalResonator],
        mobius_banks: &[MobiusSynth],
    ) -> bool {
        mobius_banks
            .get(usize::from(self.bank_idx))
            .is_none_or(MobiusSynth::is_quiet)
    }
}
