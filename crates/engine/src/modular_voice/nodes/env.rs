//! ADSR / Hold-Decay modular envelope voice node with pre-delay, multi-burst, and external TrigIn.

use super::{
    write_copy_module_normalized, NodeInputs, ProcessCtx, TriggerCtx, TriggerState, VoiceLocks,
    VoiceModule,
};
use crate::modular_voice::ModuleParams;
use cadence_core::dsp::{acid_filter_decay_ms, ModularEnv};
use cadence_core::rack::{
    EnvModule, FilterMode, ModuleKind, ModuleKindTag, ModuleParam, TriggerMode,
};

#[derive(Clone, Copy, Debug)]
pub(in crate::modular_voice) struct EnvNode {
    pub current: EnvModule,
    pub target: EnvModule,
    pub env: ModularEnv,
    prev_trig: f64,
    has_trig_in: bool,
}

impl EnvNode {
    #[inline]
    pub(super) fn new(params: EnvModule, sample_rate: f64) -> Self {
        Self {
            current: params,
            target: params,
            env: ModularEnv::new_idle(sample_rate, params.to_dsp_params()),
            prev_trig: 0.0,
            has_trig_in: false,
        }
    }
}

impl VoiceModule for EnvNode {
    #[inline]
    fn tag(&self) -> ModuleKindTag {
        ModuleKindTag::Env
    }

    #[inline]
    fn target_params(&self) -> ModuleParams {
        ModuleParams::Env(self.target)
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
            ModuleKind::Env,
            |k| match k {
                ModuleKind::Env(m) => Some(m),
                _ => None,
            },
            param,
            norm,
            total_frames,
        )
    }

    #[inline]
    fn update_continuous(&mut self, src: &ModuleKind, module_id: u16, locks: VoiceLocks<'_>) {
        let ModuleKind::Env(s) = src else {
            return;
        };
        if !locks.is_module_locked(module_id, ModuleParam::Sustain) {
            self.target.sustain = s.sustain;
        }
    }

    #[inline]
    fn on_trigger(&mut self, ctx: &TriggerCtx, _state: &mut TriggerState<'_>) {
        let target = self.target;
        if ctx.eff_legato {
            self.current.mode = target.mode;
            self.current.attack_ms = target.attack_ms;
            self.current.hold_ms = target.hold_ms;
            self.current.decay_ms = target.decay_ms;
            self.current.release_ms = target.release_ms;
            self.current.trigger_mode = target.trigger_mode;
            self.current.delay_ms = target.delay_ms;
            self.current.bursts = target.bursts;
            self.current.burst_spacing_ms = target.burst_spacing_ms;
        } else {
            self.current = target;
        }

        let mut dsp_params = target.to_dsp_params();
        if let Some(vel_amt) = ctx.acid_vel_amount {
            dsp_params.decay_ms = acid_filter_decay_ms(
                FilterMode::AcidLowPass,
                dsp_params.decay_ms,
                vel_amt,
                ctx.vel,
            );
        }

        if ctx.eff_retarget {
            if ctx.has_trig_in {
                self.has_trig_in = true;
            } else if ctx.eff_legato {
                let mut next_env = ModularEnv::new(ctx.sample_rate, dsp_params);
                next_env.adopt_progress_from(self.env);
                self.env = next_env;
                self.prev_trig = 0.0;
                self.has_trig_in = false;
            } else {
                self.env.retrigger(ctx.sample_rate, dsp_params);
                self.prev_trig = 0.0;
                self.has_trig_in = false;
            }
            return;
        }

        self.env = if ctx.has_trig_in {
            ModularEnv::new_idle(ctx.sample_rate, dsp_params)
        } else {
            ModularEnv::new(ctx.sample_rate, dsp_params)
        };
        self.prev_trig = 0.0;
        self.has_trig_in = ctx.has_trig_in;
    }

    #[inline]
    fn smooth_step(&mut self, s64: f64) {
        let s32 = s64 as f32;
        self.current.sustain += (self.target.sustain - self.current.sustain) * s32;
    }

    #[inline]
    fn process(&mut self, inputs: &NodeInputs, ctx: &mut ProcessCtx<'_>) -> [f64; 2] {
        let env_p = self.current;
        if self.has_trig_in {
            if self.prev_trig < 0.5 && inputs.trig_in >= 0.5 {
                self.env.retrigger(ctx.sample_rate, env_p.to_dsp_params());
            } else if self.prev_trig >= 0.5 && inputs.trig_in < 0.5 {
                self.env.release();
            }
            self.prev_trig = inputs.trig_in;
        }
        let v = self.env.next(env_p.sustain);
        [v, v]
    }

    #[inline]
    fn on_note_off(&mut self, has_lifetime_envs: bool) {
        if has_lifetime_envs && !self.has_trig_in {
            self.env.release();
        }
    }

    #[inline]
    fn on_force_release(&mut self, sample_rate: f64, _rel_coeff: f64) {
        if self.current.trigger_mode == TriggerMode::OneShot {
            self.env.choke(sample_rate);
        } else {
            self.env.release();
        }
    }

    #[inline]
    fn on_choke(&mut self, sample_rate: f64, _choke_coeff: f64) {
        self.env.choke(sample_rate);
    }
}
