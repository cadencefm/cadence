//! Voltage-controlled amplifier and 4-quadrant audio-rate ring modulator voice node.

use super::{
    write_copy_module_normalized, NodeInputs, ProcessCtx, TriggerCtx, TriggerState, VoiceLocks,
    VoiceModule, SILENCE_FLOOR,
};
use crate::modular_voice::ModuleParams;
use cadence_core::dsp::smooth_step_f32;
use cadence_core::rack::{ModuleKind, ModuleKindTag, ModuleParam, VcaModule};

#[derive(Clone, Copy, Debug)]
pub(in crate::modular_voice) struct VcaNode {
    current: VcaModule,
    pub target: VcaModule,
    has_cv: bool,
}

impl VcaNode {
    #[inline]
    pub(super) fn new(params: VcaModule) -> Self {
        Self {
            current: params,
            target: params,
            has_cv: false,
        }
    }

    /// True when both the smoothed and target DC bias sit within [`SILENCE_FLOOR`],
    /// guaranteeing an envelope-gated VCA passes exact silence once its CV reaches zero.
    #[inline]
    pub(in crate::modular_voice) fn is_bias_closed(&self) -> bool {
        f64::from(self.current.bias).abs() <= SILENCE_FLOOR
            && f64::from(self.target.bias).abs() <= SILENCE_FLOOR
    }
}

impl VoiceModule for VcaNode {
    #[inline]
    fn tag(&self) -> ModuleKindTag {
        ModuleKindTag::Vca
    }

    #[inline]
    fn target_params(&self) -> ModuleParams {
        ModuleParams::Vca(self.target)
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
            ModuleKind::Vca,
            |k| match k {
                ModuleKind::Vca(m) => Some(m),
                _ => None,
            },
            param,
            norm,
            total_frames,
        )
    }

    #[inline]
    fn update_continuous(&mut self, src: &ModuleKind, module_id: u16, locks: VoiceLocks<'_>) {
        let ModuleKind::Vca(s) = src else {
            return;
        };
        if !locks.is_module_locked(module_id, ModuleParam::VcaGain) {
            self.target.gain = s.gain;
        }
        if !locks.is_module_locked(module_id, ModuleParam::VcaBias) {
            self.target.bias = s.bias;
        }
    }

    #[inline]
    fn on_trigger(&mut self, ctx: &TriggerCtx, _state: &mut TriggerState<'_>) {
        if !ctx.eff_legato {
            self.current = self.target;
        }
        self.has_cv = ctx.has_cv;
    }

    #[inline]
    fn smooth_step(&mut self, s64: f64) {
        let s32 = s64 as f32;
        smooth_step_f32(&mut self.current.gain, self.target.gain, s32);
        smooth_step_f32(&mut self.current.bias, self.target.bias, s32);
    }

    #[inline]
    fn process(&mut self, inputs: &NodeInputs, _ctx: &mut ProcessCtx<'_>) -> [f64; 2] {
        let gain = f64::from(self.current.gain);
        if self.has_cv {
            let bias = f64::from(self.current.bias);
            [
                inputs.in_audio[0] * gain * (bias + inputs.cv_stereo[0]),
                inputs.in_audio[1] * gain * (bias + inputs.cv_stereo[1]),
            ]
        } else {
            [inputs.in_audio[0] * gain, inputs.in_audio[1] * gain]
        }
    }
}
