//! White/Pink noise generator voice node with independent TPT 1-pole HPF and LPF band shaping.

use super::{
    write_copy_module_normalized, NodeInputs, ProcessCtx, TriggerCtx, TriggerState, VoiceLocks,
    VoiceModule,
};
use crate::modular_voice::ModuleParams;
use cadence_core::dsp::{
    mix_seed, smooth_step_f32, LogSmoother, NoiseGen, TptOnePoleHpf, TptOnePoleLpf,
};
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
    cached_hp_key: (u64, u64),
    cached_lp_key: (u64, u64),
}

impl NoiseNode {
    #[inline]
    pub(super) fn new(params: NoiseModule, sample_rate: f64) -> Self {
        let hp = LogSmoother::new(f64::from(params.highpass_hz), 20.0);
        let lp = LogSmoother::new(f64::from(params.lowpass_hz), 20.0);
        let sr_bits = sample_rate.to_bits();
        Self {
            current: params,
            target: params,
            gen: NoiseGen::new(1, sample_rate),
            hpf: TptOnePoleHpf::new(sample_rate, hp.current()),
            lpf: TptOnePoleLpf::new(sample_rate, lp.current()),
            cached_hp_key: (hp.current().to_bits(), sr_bits),
            cached_lp_key: (lp.current().to_bits(), sr_bits),
            hp,
            lp,
        }
    }

    #[cfg(test)]
    pub(in crate::modular_voice) fn cached_filter_sr_bits(&self) -> (u64, u64) {
        (self.cached_hp_key.1, self.cached_lp_key.1)
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
            self.gen.set_sample_rate(ctx.sample_rate);
            self.hp.set_target(f64::from(noise.highpass_hz));
            self.lp.set_target(f64::from(noise.lowpass_hz));
            if !ctx.eff_legato {
                self.hp.snap_to_target();
                self.lp.snap_to_target();
                let hp_hz = self.hp.current();
                let lp_hz = self.lp.current();
                let sr_bits = ctx.sample_rate.to_bits();
                self.hpf.set_cutoff(ctx.sample_rate, hp_hz);
                self.lpf.set_cutoff(ctx.sample_rate, lp_hz);
                self.cached_hp_key = (hp_hz.to_bits(), sr_bits);
                self.cached_lp_key = (lp_hz.to_bits(), sr_bits);
            }
            return;
        }

        let seed = mix_seed(ctx.base_seed, u32::from(ctx.mod_id));
        self.gen = NoiseGen::new(seed, ctx.sample_rate);
        self.hp = LogSmoother::new(f64::from(noise.highpass_hz), 20.0);
        self.lp = LogSmoother::new(f64::from(noise.lowpass_hz), 20.0);
        let hp_hz = self.hp.current();
        let lp_hz = self.lp.current();
        let sr_bits = ctx.sample_rate.to_bits();
        self.hpf = TptOnePoleHpf::new(ctx.sample_rate, hp_hz);
        self.lpf = TptOnePoleLpf::new(ctx.sample_rate, lp_hz);
        self.cached_hp_key = (hp_hz.to_bits(), sr_bits);
        self.cached_lp_key = (lp_hz.to_bits(), sr_bits);
    }

    #[inline]
    fn smooth_step(&mut self, s64: f64) {
        let s32 = s64 as f32;
        smooth_step_f32(&mut self.current.level, self.target.level, s32);
        self.current.highpass_hz = self.hp.step(s64) as f32;
        self.current.lowpass_hz = self.lp.step(s64) as f32;
    }

    #[inline]
    fn control_tick(&mut self, sample_rate: f64) {
        let sr_bits = sample_rate.to_bits();
        let hp_hz = self.hp.current();
        let hp_key = (hp_hz.to_bits(), sr_bits);
        if hp_key != self.cached_hp_key {
            self.cached_hp_key = hp_key;
            self.hpf.set_cutoff(sample_rate, hp_hz);
        }
        let lp_hz = self.lp.current();
        let lp_key = (lp_hz.to_bits(), sr_bits);
        if lp_key != self.cached_lp_key {
            self.cached_lp_key = lp_key;
            self.lpf.set_cutoff(sample_rate, lp_hz);
        }
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
