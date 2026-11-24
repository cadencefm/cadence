//! Terminal zone output voice node with CV gate, M/S spread, DC blocker, equal-power pan, and guard.

use super::{
    write_copy_module_normalized, NodeInputs, ProcessCtx, TriggerCtx, TriggerState, VoiceLocks,
    VoiceModule,
};
use crate::modular_voice::ModuleParams;
use cadence_core::dsp::{
    db_to_lin_f64, pan_mono_equal_power_f64, soft_guard_f64, DcBlocker, ModalResonator,
};
use cadence_core::rack::{ModuleKind, ModuleKindTag, ModuleParam, OutModule};

#[derive(Clone, Copy, Debug)]
pub(in crate::modular_voice) struct OutNode {
    current: OutModule,
    pub target: OutModule,
    dc_blocker: DcBlocker,
    has_cv: bool,
}

impl Default for OutNode {
    fn default() -> Self {
        Self::new(OutModule::default(), 48_000.0)
    }
}

impl OutNode {
    #[inline]
    pub(super) fn new(params: OutModule, sample_rate: f64) -> Self {
        Self {
            current: params,
            target: params,
            dc_blocker: DcBlocker::new(sample_rate),
            has_cv: false,
        }
    }
}

impl VoiceModule for OutNode {
    #[inline]
    fn tag(&self) -> ModuleKindTag {
        ModuleKindTag::Out
    }

    #[inline]
    fn target_params(&self) -> ModuleParams {
        ModuleParams::Out(self.target)
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
            ModuleKind::Out,
            |k| match k {
                ModuleKind::Out(m) => Some(m),
                _ => None,
            },
            param,
            norm,
            total_frames,
        )
    }

    #[inline]
    fn update_continuous(&mut self, src: &ModuleKind, module_id: u16, locks: VoiceLocks<'_>) {
        let ModuleKind::Out(s) = src else {
            return;
        };
        if !locks.is_module_locked(module_id, ModuleParam::GainDb) {
            self.target.gain_db = s.gain_db;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Pan) {
            self.target.pan = s.pan;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Spread) {
            self.target.spread = s.spread;
        }
    }

    #[inline]
    fn on_trigger(&mut self, ctx: &TriggerCtx, _state: &mut TriggerState<'_>) {
        if !ctx.eff_legato {
            self.current = self.target;
        }
        if ctx.eff_retarget {
            self.has_cv = ctx.has_cv;
            return;
        }
        self.dc_blocker = DcBlocker::new(ctx.sample_rate);
        self.has_cv = ctx.has_cv;
    }

    #[inline]
    fn smooth_step(&mut self, s64: f64) {
        let s32 = s64 as f32;
        self.current.gain_db += (self.target.gain_db - self.current.gain_db) * s32;
        self.current.pan += (self.target.pan - self.current.pan) * s32;
        self.current.spread += (self.target.spread - self.current.spread) * s32;
    }

    #[inline]
    fn process(&mut self, inputs: &NodeInputs, ctx: &mut ProcessCtx<'_>) -> [f64; 2] {
        let out_p = self.current;
        let mut sig = inputs.in_audio;
        if self.has_cv {
            sig = [sig[0] * inputs.cv_stereo[0], sig[1] * inputs.cv_stereo[1]];
        }
        if ctx.use_fallback_gate {
            sig = [sig[0] * ctx.fallback_env, sig[1] * ctx.fallback_env];
        }
        let spread = f64::from(out_p.spread.clamp(0.0, 1.0));
        if spread > 0.0 {
            let mid = 0.5 * (sig[0] + sig[1]);
            let side = 0.5 * (sig[0] - sig[1]) * (1.0 + spread);
            sig = [mid + side, mid - side];
        }
        let centered = self.dc_blocker.process_stereo(sig);
        let gain = db_to_lin_f64(f64::from(out_p.gain_db)) * ctx.velocity;
        let [pl, pr] = pan_mono_equal_power_f64(
            (f64::from(out_p.pan) + f64::from(ctx.zone_pan)).clamp(-1.0, 1.0),
        );
        [
            soft_guard_f64(centered[0] * gain * pl),
            soft_guard_f64(centered[1] * gain * pr),
        ]
    }

    #[inline]
    fn is_tail_quiet(&self, _resonator_banks: &[ModalResonator]) -> bool {
        self.dc_blocker.quiet()
    }
}
