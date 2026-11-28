//! Terminal zone output voice node with CV gate, Mid/Side spread, DC blocker,
//! constant-power (`-3.01 dB` center) voice/zone panner, and transparent `+12.04 dBFS`
//! anti-blowup safety guard (`voice_guard_f64`).
//!
//! Uses [`pan_mono_equal_power_f64`] intentionally as the voice/zone tier of
//! `cadence_core::dsp::gain`'s three-tier stereo pan contract, preserving constant
//! stereo power (`L² + R² == 1.0`) across `[-1.0, 1.0]` for both mono and stereo voices.

use super::{
    write_copy_module_normalized, NodeInputs, ProcessCtx, TailBanks, TriggerCtx, TriggerState,
    VoiceLocks, VoiceModule,
};
use crate::modular_voice::ModuleParams;
use cadence_core::dsp::{
    db_to_lin_f64, pan_mono_equal_power_f64, sanitize_clamp_f32, sanitize_clamp_f64,
    smooth_step_f32, stereo_ms_decode_f64, stereo_ms_encode_f64, voice_guard_f64, DcBlocker,
};
use cadence_core::rack::{
    ModuleKind, ModuleKindTag, ModuleParam, OutModule, OUT_MAX_GAIN_DB, OUT_MIN_GAIN_DB,
};

#[derive(Clone, Copy, Debug)]
pub(in crate::modular_voice) struct OutNode {
    current: OutModule,
    pub target: OutModule,
    dc_blocker: DcBlocker,
    has_cv: bool,
    cached_gain_db_bits: u32,
    cached_gain_lin: f64,
    cached_pan_bits: u64,
    cached_pan: [f64; 2],
}

impl Default for OutNode {
    fn default() -> Self {
        Self::new(OutModule::default(), 48_000.0)
    }
}

impl OutNode {
    #[inline]
    pub(super) fn new(params: OutModule, sample_rate: f64) -> Self {
        let pan = sanitize_clamp_f64(f64::from(params.pan), -1.0, 1.0, 0.0);
        let gain_db = sanitize_clamp_f32(params.gain_db, OUT_MIN_GAIN_DB, OUT_MAX_GAIN_DB, 0.0);
        Self {
            current: params,
            target: params,
            dc_blocker: DcBlocker::new(sample_rate),
            has_cv: false,
            cached_gain_db_bits: gain_db.to_bits(),
            cached_gain_lin: db_to_lin_f64(f64::from(gain_db)),
            cached_pan_bits: pan.to_bits(),
            cached_pan: pan_mono_equal_power_f64(pan),
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
            self.dc_blocker.set_sample_rate(ctx.sample_rate);
            self.has_cv = ctx.has_cv;
            return;
        }
        self.dc_blocker = DcBlocker::new(ctx.sample_rate);
        self.has_cv = ctx.has_cv;
    }

    #[inline]
    fn smooth_step(&mut self, s64: f64) {
        let s32 = s64 as f32;
        smooth_step_f32(&mut self.current.gain_db, self.target.gain_db, s32);
        smooth_step_f32(&mut self.current.pan, self.target.pan, s32);
        smooth_step_f32(&mut self.current.spread, self.target.spread, s32);
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
        let spread = sanitize_clamp_f64(f64::from(out_p.spread), 0.0, 1.0, 0.0);
        if spread > 0.0 {
            let [mid, side] = stereo_ms_encode_f64(sig);
            sig = stereo_ms_decode_f64(mid, side * (1.0 + spread));
        }
        let centered = self.dc_blocker.process_stereo(sig);
        let gain_db = sanitize_clamp_f32(out_p.gain_db, OUT_MIN_GAIN_DB, OUT_MAX_GAIN_DB, 0.0);
        let gain_db_bits = gain_db.to_bits();
        if gain_db_bits != self.cached_gain_db_bits {
            self.cached_gain_db_bits = gain_db_bits;
            self.cached_gain_lin = db_to_lin_f64(f64::from(gain_db));
        }
        let gain = self.cached_gain_lin * ctx.velocity;
        let eff_pan = sanitize_clamp_f64(
            f64::from(out_p.pan) + f64::from(ctx.zone_pan),
            -1.0,
            1.0,
            0.0,
        );
        let pan_bits = eff_pan.to_bits();
        if pan_bits != self.cached_pan_bits {
            self.cached_pan_bits = pan_bits;
            self.cached_pan = pan_mono_equal_power_f64(eff_pan);
        }
        let [pl, pr] = self.cached_pan;
        [
            voice_guard_f64(centered[0] * gain * pl),
            voice_guard_f64(centered[1] * gain * pr),
        ]
    }

    #[inline]
    fn is_tail_quiet(&self, _banks: TailBanks<'_>) -> bool {
        self.dc_blocker.quiet()
    }
}
