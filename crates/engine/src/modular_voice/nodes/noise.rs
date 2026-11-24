//! White/Pink noise generator voice node with independent TPT 1-pole HPF and LPF band shaping.

use super::{
    write_copy_module_normalized, NodeInputs, ProcessCtx, TriggerCtx, TriggerState, VoiceLocks,
    VoiceModule,
};
use crate::modular_voice::ModuleParams;
use cadence_core::dsp::{mix_seed, LogSmoother, NoiseGen, TptOnePoleHpf, TptOnePoleLpf};
use cadence_core::rack::{ModuleKind, ModuleKindTag, ModuleParam, NoiseModule};

#[derive(Clone, Copy, Debug)]
pub(in crate::modular_voice) struct NoiseNode {
    current: NoiseModule,
    pub target: NoiseModule,
    gen: NoiseGen,
    hpf: TptOnePoleHpf,
    lpf: TptOnePoleLpf,
    hp: LogSmoother,
    lp: LogSmoother,
}

impl NoiseNode {
    #[inline]
    pub(super) fn new(params: NoiseModule, sample_rate: f64) -> Self {
        Self {
            current: params,
            target: params,
            gen: NoiseGen::new(1, sample_rate),
            hpf: TptOnePoleHpf::new(sample_rate, f64::from(params.highpass_hz)),
            lpf: TptOnePoleLpf::new(sample_rate, f64::from(params.lowpass_hz)),
            hp: LogSmoother::new(f64::from(params.highpass_hz), 20.0),
            lp: LogSmoother::new(f64::from(params.lowpass_hz), 20.0),
        }
    }
}

impl VoiceModule for NoiseNode {
    #[inline]
    fn tag(&self) -> ModuleKindTag {
        ModuleKindTag::Noise
    }

    #[inline]
    fn target_params(&self) -> ModuleParams {
        ModuleParams::Noise(self.target)
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
            ModuleKind::Noise,
            |k| match k {
                ModuleKind::Noise(m) => Some(m),
                _ => None,
            },
            param,
            norm,
            total_frames,
        )
    }

    #[inline]
    fn sync_target_logs(&mut self) {
        self.hp.set_target(f64::from(self.target.highpass_hz));
        self.lp.set_target(f64::from(self.target.lowpass_hz));
    }

    #[inline]
    fn update_continuous(&mut self, src: &ModuleKind, module_id: u16, locks: VoiceLocks<'_>) {
        let ModuleKind::Noise(s) = src else {
            return;
        };
        if !locks.is_module_locked(module_id, ModuleParam::HighpassHz) {
            self.target.highpass_hz = s.highpass_hz;
        }
        if !locks.is_module_locked(module_id, ModuleParam::LowpassHz) {
            self.target.lowpass_hz = s.lowpass_hz;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Level) {
            self.target.level = s.level;
        }
        self.sync_target_logs();
    }

    #[inline]
    fn on_trigger(&mut self, ctx: &TriggerCtx, _state: &mut TriggerState<'_>) {
        let noise = self.target;
        if ctx.eff_legato {
            self.current.color = noise.color;
        } else {
            self.current = noise;
        }

        if ctx.eff_retarget {
            self.hp.set_target(f64::from(noise.highpass_hz));
            self.lp.set_target(f64::from(noise.lowpass_hz));
            if !ctx.eff_legato {
                self.hp.snap_to_target();
                self.lp.snap_to_target();
                self.hpf.set_cutoff(ctx.sample_rate, self.hp.current());
                self.lpf.set_cutoff(ctx.sample_rate, self.lp.current());
            }
            return;
        }

        let seed = mix_seed(ctx.base_seed, u32::from(ctx.mod_id));
        self.gen = NoiseGen::new(seed, ctx.sample_rate);
        self.hpf = TptOnePoleHpf::new(ctx.sample_rate, f64::from(noise.highpass_hz));
        self.lpf = TptOnePoleLpf::new(ctx.sample_rate, f64::from(noise.lowpass_hz));
        self.hp = LogSmoother::new(f64::from(noise.highpass_hz), 20.0);
        self.lp = LogSmoother::new(f64::from(noise.lowpass_hz), 20.0);
    }

    #[inline]
    fn smooth_step(&mut self, s64: f64) {
        let s32 = s64 as f32;
        self.current.level += (self.target.level - self.current.level) * s32;
        self.current.highpass_hz = self.hp.step(s64) as f32;
        self.current.lowpass_hz = self.lp.step(s64) as f32;
    }

    #[inline]
    fn control_tick(&mut self, sample_rate: f64) {
        self.hpf.set_cutoff(sample_rate, self.hp.current());
        self.lpf.set_cutoff(sample_rate, self.lp.current());
    }

    #[inline]
    fn process(&mut self, _inputs: &NodeInputs, _ctx: &mut ProcessCtx<'_>) -> [f64; 2] {
        let noise = self.current;
        let colored = self.gen.next_sample(noise.color);
        let hp = if noise.highpass_hz > 20.5 {
            self.hpf.process_mono(colored)
        } else {
            colored
        };
        let lp = if noise.lowpass_hz < 19_900.0 {
            self.lpf.process_mono(hp)
        } else {
            hp
        };
        let v = lp * f64::from(noise.level);
        [v, v]
    }
}
