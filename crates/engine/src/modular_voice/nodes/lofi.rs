//! Lo-Fi bit-depth quantizer and sample-rate decimator voice node.

use super::{
    write_copy_module_normalized, NodeInputs, ProcessCtx, TriggerCtx, TriggerState, VoiceLocks,
    VoiceModule,
};
use crate::modular_voice::ModuleParams;
use cadence_core::dsp::{LoFiProcessor, LogSmoother, LOFI_MAX_RATE_HZ, LOFI_MIN_RATE_HZ};
use cadence_core::rack::{LoFiModule, ModuleKind, ModuleKindTag, ModuleParam};

#[derive(Clone, Copy, Debug)]
pub(in crate::modular_voice) struct LoFiNode {
    current: LoFiModule,
    pub target: LoFiModule,
    proc: LoFiProcessor,
    rate: LogSmoother,
}

impl LoFiNode {
    #[inline]
    pub(super) fn new(params: LoFiModule, sample_rate: f64) -> Self {
        Self {
            current: params,
            target: params,
            proc: LoFiProcessor::new(sample_rate),
            rate: LogSmoother::new_clamped(
                f64::from(params.rate_hz),
                f64::from(LOFI_MIN_RATE_HZ),
                f64::from(LOFI_MAX_RATE_HZ),
            ),
        }
    }
}

impl VoiceModule for LoFiNode {
    #[inline]
    fn tag(&self) -> ModuleKindTag {
        ModuleKindTag::LoFi
    }

    #[inline]
    fn target_params(&self) -> ModuleParams {
        ModuleParams::LoFi(self.target)
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
            ModuleKind::LoFi,
            |k| match k {
                ModuleKind::LoFi(m) => Some(m),
                _ => None,
            },
            param,
            norm,
            total_frames,
        )
    }

    #[inline]
    fn sync_target_logs(&mut self) {
        self.rate.set_target(f64::from(self.target.rate_hz));
    }

    #[inline]
    fn update_continuous(&mut self, src: &ModuleKind, module_id: u16, locks: VoiceLocks<'_>) {
        let ModuleKind::LoFi(s) = src else {
            return;
        };
        if !locks.is_module_locked(module_id, ModuleParam::Bits) {
            self.target.bits = s.bits;
        }
        if !locks.is_module_locked(module_id, ModuleParam::SampleRateHz) {
            self.target.rate_hz = s.rate_hz;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Mix) {
            self.target.mix = s.mix;
        }
        self.sync_target_logs();
    }

    #[inline]
    fn on_trigger(&mut self, ctx: &TriggerCtx, _state: &mut TriggerState<'_>) {
        let lofi = self.target;
        if !ctx.eff_legato {
            self.current = lofi;
        }

        if ctx.eff_retarget {
            self.rate.set_target(f64::from(lofi.rate_hz));
            if !ctx.eff_legato {
                self.rate.snap_to_target();
            }
            return;
        }

        self.proc = LoFiProcessor::new(ctx.sample_rate);
        self.rate = LogSmoother::new_clamped(
            f64::from(lofi.rate_hz),
            f64::from(LOFI_MIN_RATE_HZ),
            f64::from(LOFI_MAX_RATE_HZ),
        );
    }

    #[inline]
    fn smooth_step(&mut self, s64: f64) {
        let s32 = s64 as f32;
        self.current.bits += (self.target.bits - self.current.bits) * s32;
        self.current.mix += (self.target.mix - self.current.mix) * s32;
        self.current.rate_hz = self.rate.step(s64) as f32;
    }

    #[inline]
    fn process(&mut self, inputs: &NodeInputs, _ctx: &mut ProcessCtx<'_>) -> [f64; 2] {
        let lofi = self.current;
        self.proc.process_stereo(
            inputs.in_audio,
            lofi.bits,
            self.rate.current() as f32,
            lofi.mix,
        )
    }
}
