//! Tempo-syncable multi-shape LFO voice node with Sample/Hold PRNG.

use super::{
    write_copy_module_normalized, NodeInputs, ProcessCtx, TriggerCtx, TriggerState, VoiceLocks,
    VoiceModule,
};
use crate::modular_voice::ModuleParams;
use cadence_core::dsp::{ideal_waveform_f64, phase_wrap, LogSmoother, XorShift32};
use cadence_core::rack::{LfoModule, LfoShape, ModuleKind, ModuleKindTag, ModuleParam, Waveform};

#[derive(Clone, Copy, Debug)]
pub(in crate::modular_voice) struct LfoNode {
    current: LfoModule,
    pub target: LfoModule,
    phase: f64,
    rng: XorShift32,
    hold: f64,
    rate: LogSmoother,
}

impl LfoNode {
    #[inline]
    pub(super) fn new(params: LfoModule) -> Self {
        Self {
            current: params,
            target: params,
            phase: 0.0,
            rng: XorShift32::new(1),
            hold: 0.0,
            rate: LogSmoother::new(f64::from(params.rate_hz), 0.01),
        }
    }
}

impl VoiceModule for LfoNode {
    #[inline]
    fn tag(&self) -> ModuleKindTag {
        ModuleKindTag::Lfo
    }

    #[inline]
    fn target_params(&self) -> ModuleParams {
        ModuleParams::Lfo(self.target)
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
            ModuleKind::Lfo,
            |k| match k {
                ModuleKind::Lfo(m) => Some(m),
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
        let ModuleKind::Lfo(s) = src else {
            return;
        };
        if !locks.is_module_locked(module_id, ModuleParam::LfoRate) {
            self.target.rate_hz = s.rate_hz;
        }
        self.sync_target_logs();
    }

    #[inline]
    fn on_trigger(&mut self, ctx: &TriggerCtx, _state: &mut TriggerState<'_>) {
        let lfo = self.target;
        if ctx.eff_legato {
            self.current.shape = lfo.shape;
            self.current.sync = lfo.sync;
            self.current.division_steps = lfo.division_steps;
            self.current.retrigger = lfo.retrigger;
            self.current.bipolar = lfo.bipolar;
        } else {
            self.current = lfo;
        }

        if ctx.eff_retarget {
            self.rate.set_target(f64::from(lfo.rate_hz));
            if !ctx.eff_legato {
                self.rate.snap_to_target();
                if lfo.retrigger {
                    self.phase = 0.0;
                }
            }
            return;
        }

        let mut rng = XorShift32::new(ctx.base_seed.wrapping_add(0xa511_e9b3).wrapping_add(
            u32::from(ctx.mod_id).wrapping_mul(cadence_core::dsp::SECONDARY_WEYL_SALT_32),
        ));
        self.hold = rng.next_bipolar_f64();
        self.rng = rng;
        self.phase = if lfo.retrigger {
            0.0
        } else {
            phase_wrap(ctx.clock * lfo.effective_hz(ctx.bpm))
        };
        self.rate = LogSmoother::new(f64::from(lfo.rate_hz), 0.01);
    }

    #[inline]
    fn smooth_step(&mut self, s64: f64) {
        self.current.rate_hz = self.rate.step(s64) as f32;
    }

    #[inline]
    fn process(&mut self, _inputs: &NodeInputs, ctx: &mut ProcessCtx<'_>) -> [f64; 2] {
        let lfo = self.current;
        let bipolar = match lfo.shape {
            LfoShape::Sine => ideal_waveform_f64(Waveform::Sine, self.phase, 0.5),
            LfoShape::Triangle => ideal_waveform_f64(Waveform::Triangle, self.phase, 0.5),
            LfoShape::Saw => ideal_waveform_f64(Waveform::Saw, self.phase, 0.5),
            LfoShape::Square => ideal_waveform_f64(Waveform::Pulse, self.phase, 0.5),
            LfoShape::SampleHold => self.hold,
        };
        let val = if lfo.bipolar {
            bipolar
        } else {
            0.5 * (bipolar + 1.0)
        };
        let old_phase = self.phase;
        self.phase = phase_wrap(self.phase + lfo.effective_hz(ctx.bpm) / ctx.sample_rate);
        if self.phase < old_phase {
            self.hold = self.rng.next_bipolar_f64();
        }
        [val, val]
    }
}
