//! Physical 32-mode modal resonator voice node referencing the voice's heap-hosted bank pool.

use super::{
    resolve_tracked_pitch_hz, write_copy_module_normalized, NodeInputs, ProcessCtx, TrackedPitch,
    TriggerCtx, TriggerState, VoiceLocks, VoiceModule,
};
use crate::modular_voice::ModuleParams;
use cadence_core::dsp::{
    sanitize_clamp_f32, smooth_step_f32, LogSmoother, ModalResonator, NYQUIST_SAFE_RATIO,
    RESONATOR_MAX_DECAY_MS, RESONATOR_MAX_HZ, RESONATOR_MIN_DECAY_MS, RESONATOR_MIN_HZ,
};
use cadence_core::rack::{
    ModuleKind, ModuleKindTag, ModuleParam, ResonatorMode, ResonatorModule, MAX_RESONATORS_PER_ZONE,
};

#[derive(Clone, Copy, Debug)]
pub(in crate::modular_voice) struct ResonatorNode {
    current: ResonatorModule,
    pub target: ResonatorModule,
    bank_idx: u8,
    fixed_hz: LogSmoother,
    decay: LogSmoother,
    has_cv: bool,
    applied_mode: ResonatorMode,
    applied_f0_hz: f64,
    applied_structure: f32,
    applied_decay_ms: f32,
    applied_brightness: f32,
    applied_position: f32,
    initialized: bool,
    #[cfg(test)]
    param_update_count: u32,
}

impl ResonatorNode {
    #[inline]
    pub(super) fn new(params: ResonatorModule) -> Self {
        Self {
            current: params,
            target: params,
            bank_idx: 0,
            fixed_hz: LogSmoother::new(f64::from(params.fixed_hz), 20.0),
            decay: LogSmoother::new_clamped(
                f64::from(params.decay_ms),
                f64::from(RESONATOR_MIN_DECAY_MS),
                f64::from(RESONATOR_MAX_DECAY_MS),
            ),
            has_cv: false,
            applied_mode: params.mode,
            applied_f0_hz: 0.0,
            applied_structure: -1.0,
            applied_decay_ms: -1.0,
            applied_brightness: -1.0,
            applied_position: -1.0,
            initialized: false,
            #[cfg(test)]
            param_update_count: 0,
        }
    }

    /// Maximum of the current smoothed decay and sanitized target decay in milliseconds
    /// for post-gate resonator tail sizing (handles legato and continuous decay changes).
    #[inline]
    pub(in crate::modular_voice) fn max_tail_decay_ms(&self) -> f64 {
        self.decay.current().max(self.decay.target())
    }

    #[cfg(test)]
    pub(in crate::modular_voice) fn applied_f0_hz(&self) -> f64 {
        self.applied_f0_hz
    }

    #[cfg(test)]
    pub(in crate::modular_voice) fn param_update_count(&self) -> u32 {
        self.param_update_count
    }
}

impl VoiceModule for ResonatorNode {
    #[inline]
    fn tag(&self) -> ModuleKindTag {
        ModuleKindTag::Resonator
    }

    #[inline]
    fn target_params(&self) -> ModuleParams {
        ModuleParams::Resonator(self.target)
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
            ModuleKind::Resonator,
            |k| match k {
                ModuleKind::Resonator(m) => Some(m),
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
        self.decay.set_target(f64::from(self.target.decay_ms));
    }

    #[inline]
    fn update_continuous(&mut self, src: &ModuleKind, module_id: u16, locks: VoiceLocks<'_>) {
        let ModuleKind::Resonator(s) = src else {
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
        if !locks.is_module_locked(module_id, ModuleParam::Structure) {
            self.target.structure = s.structure;
        }
        if !locks.is_module_locked(module_id, ModuleParam::ResonatorDecay) {
            self.target.decay_ms = s.decay_ms;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Brightness) {
            self.target.brightness = s.brightness;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Position) {
            self.target.position = s.position;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Mix) {
            self.target.mix = s.mix;
        }
        self.sync_target_logs();
    }

    #[inline]
    fn on_trigger(&mut self, ctx: &TriggerCtx, state: &mut TriggerState<'_>) {
        let res = self.target;
        if ctx.eff_legato {
            self.current.mode = res.mode;
            self.current.pitch_tracking = res.pitch_tracking;
        } else {
            self.current = res;
        }

        if ctx.eff_retarget {
            let bidx = usize::from(self.bank_idx);
            state.next_res_bank = state.next_res_bank.max(bidx + 1);
            if let Some(bank) = state.resonator_banks.get_mut(bidx) {
                if bank.set_sample_rate(ctx.sample_rate) {
                    self.initialized = false;
                }
            }
            self.has_cv = ctx.has_cv;
            self.fixed_hz.set_target(f64::from(res.fixed_hz));
            self.decay.set_target(f64::from(res.decay_ms));
            if !ctx.eff_legato {
                self.fixed_hz.snap_to_target();
                self.decay.snap_to_target();
                self.initialized = false;
            }
            return;
        }

        let bank_idx = state
            .next_res_bank
            .min(MAX_RESONATORS_PER_ZONE.saturating_sub(1));
        state.next_res_bank += 1;
        if let Some(bank) = state.resonator_banks.get_mut(bank_idx) {
            bank.reset(ctx.sample_rate);
        }
        self.bank_idx = bank_idx as u8;
        self.fixed_hz = LogSmoother::new(f64::from(res.fixed_hz), 20.0);
        self.decay = LogSmoother::new_clamped(
            f64::from(res.decay_ms),
            f64::from(RESONATOR_MIN_DECAY_MS),
            f64::from(RESONATOR_MAX_DECAY_MS),
        );
        self.has_cv = ctx.has_cv;
        self.applied_mode = res.mode;
        self.applied_f0_hz = 0.0;
        self.applied_structure = -1.0;
        self.applied_decay_ms = -1.0;
        self.applied_brightness = -1.0;
        self.applied_position = -1.0;
        self.initialized = false;
        #[cfg(test)]
        {
            self.param_update_count = 0;
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
        smooth_step_f32(&mut self.current.structure, self.target.structure, s32);
        self.current.decay_ms = self.decay.step(s64) as f32;
        smooth_step_f32(&mut self.current.brightness, self.target.brightness, s32);
        smooth_step_f32(&mut self.current.position, self.target.position, s32);
        smooth_step_f32(&mut self.current.mix, self.target.mix, s32);
    }

    #[inline]
    fn process(&mut self, inputs: &NodeInputs, ctx: &mut ProcessCtx<'_>) -> [f64; 2] {
        let res = self.current;
        if let Some(bank) = ctx.resonator_banks.get_mut(usize::from(self.bank_idx)) {
            if !self.initialized || ctx.control_tick {
                let sr = ctx.sample_rate;
                let max_hz = (sr * NYQUIST_SAFE_RATIO).min(f64::from(RESONATOR_MAX_HZ));
                let base_hz = resolve_tracked_pitch_hz(
                    TrackedPitch {
                        mode: res.pitch_tracking,
                        fixed_hz: self.fixed_hz.current(),
                        note_pitch: ctx.note_pitch,
                        rack_tune: ctx.rack_tune,
                        tune_semitones: res.tune_semitones,
                        fine_cents: res.fine_cents,
                        pitch_cv: inputs.pitch_cv,
                    },
                    f64::from(RESONATOR_MIN_HZ),
                    max_hz,
                    440.0,
                );
                let eff_bright = if self.has_cv {
                    sanitize_clamp_f32(res.brightness + inputs.cv_mono() as f32, 0.0, 1.0, 0.65)
                } else {
                    sanitize_clamp_f32(res.brightness, 0.0, 1.0, 0.65)
                };
                let decay_ms = self.decay.current() as f32;
                let needs_update = !self.initialized
                    || self.applied_mode != res.mode
                    || (base_hz - self.applied_f0_hz).abs() > (self.applied_f0_hz * 1e-4).max(0.01)
                    || (res.structure - self.applied_structure).abs() > 1e-4
                    || (decay_ms - self.applied_decay_ms).abs()
                        > (self.applied_decay_ms * 1e-4).max(0.01)
                    || (eff_bright - self.applied_brightness).abs() > 1e-4
                    || (res.position - self.applied_position).abs() > 1e-4;
                if needs_update {
                    bank.set_parameters(
                        res.mode,
                        base_hz,
                        res.structure,
                        decay_ms,
                        eff_bright,
                        res.position,
                    );
                    self.applied_mode = res.mode;
                    self.applied_f0_hz = base_hz;
                    self.applied_structure = res.structure;
                    self.applied_decay_ms = decay_ms;
                    self.applied_brightness = eff_bright;
                    self.applied_position = res.position;
                    self.initialized = true;
                    #[cfg(test)]
                    {
                        self.param_update_count = self.param_update_count.saturating_add(1);
                    }
                }
            }
            bank.process_stereo(inputs.in_audio, f64::from(res.mix))
        } else {
            inputs.in_audio
        }
    }

    #[inline]
    fn is_tail_quiet(&self, resonator_banks: &[ModalResonator]) -> bool {
        resonator_banks
            .get(usize::from(self.bank_idx))
            .is_none_or(ModalResonator::quiet)
    }
}
