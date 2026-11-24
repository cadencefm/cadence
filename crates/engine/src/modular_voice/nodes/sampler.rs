//! Modular stereo sample-player voice node backed by [`SamplerParams`] and [`SamplerCursor`].

use super::{
    NodeInputs, ProcessCtx, TriggerCtx, TriggerState, VoiceLocks, VoiceModule, SILENCE_FLOOR,
};
use crate::modular_voice::ModuleParams;
use cadence_core::dsp::{
    decay_coeff_60db_ms, semitones_to_ratio_f64, smooth_step_f32, smooth_step_f64, SamplerCursor,
};
use cadence_core::rack::{ModuleKind, ModuleKindTag, ModuleParam, SampleLoopMode, SamplerParams};

#[derive(Clone, Copy, Debug)]
pub(in crate::modular_voice) struct SamplerNode {
    current: SamplerParams,
    pub target: SamplerParams,
    pub cursor: SamplerCursor,
    pub releasing: bool,
    pub release_gain: f64,
    release_coeff: f64,
    coarse_smooth: f64,
    prev_trig: f64,
    has_trig_in: bool,
    pub sample_key: u64,
    pub cached_bank_idx: Option<usize>,
    pub is_preview: bool,
    pub last_step_ratio: f64,
    cached_ratio_key: (u64, u32, u64),
}

impl SamplerNode {
    #[inline]
    pub(super) fn new(params: SamplerParams, sample_rate: f64) -> Self {
        Self {
            current: params,
            target: params,
            cursor: SamplerCursor::new(params.sample_start, false),
            releasing: false,
            release_gain: 1.0,
            release_coeff: decay_coeff_60db_ms(10.0, sample_rate),
            coarse_smooth: f64::from(params.coarse_semitones),
            prev_trig: 0.0,
            has_trig_in: false,
            sample_key: 0,
            cached_bank_idx: None,
            is_preview: false,
            last_step_ratio: 1.0,
            cached_ratio_key: (u64::MAX, u32::MAX, u64::MAX),
        }
    }
}

impl VoiceModule for SamplerNode {
    #[inline]
    fn tag(&self) -> ModuleKindTag {
        ModuleKindTag::Sampler
    }

    #[inline]
    fn target_params(&self) -> ModuleParams {
        ModuleParams::Sampler(self.target)
    }

    #[inline]
    fn write_target_normalized(
        &mut self,
        param: ModuleParam,
        norm: f32,
        total_frames: usize,
    ) -> bool {
        self.target.write_normalized(param, norm, total_frames)
    }

    #[inline]
    fn update_continuous(&mut self, src: &ModuleKind, module_id: u16, locks: VoiceLocks<'_>) {
        let ModuleKind::Sampler(s) = src else {
            return;
        };
        if !locks.is_module_locked(module_id, ModuleParam::CoarseTune) {
            self.target.coarse_semitones = s.coarse_semitones;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Fine) {
            self.target.fine_cents = s.fine_cents;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Level) {
            self.target.level = s.level;
        }
    }

    #[inline]
    fn on_trigger(&mut self, ctx: &TriggerCtx, _state: &mut TriggerState<'_>) {
        let smp = self.target;
        if ctx.eff_legato {
            self.current.root_midi = smp.root_midi;
            self.current.sample_start = smp.sample_start;
            self.current.sample_end = smp.sample_end;
            self.current.loop_mode = smp.loop_mode;
            self.current.loop_start = smp.loop_start;
            self.current.loop_end = smp.loop_end;
            self.current.loop_xfade_ms = smp.loop_xfade_ms;
            self.current.one_shot = smp.one_shot;
            self.has_trig_in = ctx.has_trig_in;
            self.releasing = false;
            self.release_gain = 1.0;
        } else {
            self.current = smp;
            self.cursor = SamplerCursor::new(smp.sample_start, !ctx.has_trig_in);
            self.releasing = false;
            self.release_gain = 1.0;
            self.release_coeff = decay_coeff_60db_ms(10.0, ctx.sample_rate);
            self.coarse_smooth = f64::from(smp.coarse_semitones);
            self.prev_trig = 0.0;
            self.has_trig_in = ctx.has_trig_in;
            self.last_step_ratio = 1.0;
            self.cached_ratio_key = (u64::MAX, u32::MAX, u64::MAX);
        }
    }

    #[inline]
    fn smooth_step(&mut self, s64: f64) {
        let s32 = s64 as f32;
        smooth_step_f64(
            &mut self.coarse_smooth,
            f64::from(self.target.coarse_semitones),
            s64,
        );
        self.current.coarse_semitones = self.target.coarse_semitones;
        smooth_step_f32(&mut self.current.fine_cents, self.target.fine_cents, s32);
        smooth_step_f32(&mut self.current.level, self.target.level, s32);
    }

    #[inline]
    fn process(&mut self, inputs: &NodeInputs, ctx: &mut ProcessCtx<'_>) -> [f64; 2] {
        let smp = self.current;
        if self.has_trig_in {
            if self.prev_trig < 0.5 && inputs.trig_in >= 0.5 {
                self.cursor.retrigger(smp.sample_start);
                self.releasing = false;
                self.release_gain = 1.0;
            }
            self.prev_trig = inputs.trig_in;
        }
        let maybe_sample = if self.is_preview {
            ctx.preview
        } else if let Some((_, data)) = self
            .cached_bank_idx
            .and_then(|idx| ctx.bank.get(idx))
            .filter(|(k, _)| *k == self.sample_key)
        {
            Some(data)
        } else if let Some((idx, (_, data))) = ctx
            .bank
            .iter()
            .enumerate()
            .find(|(_, (id, _))| *id == self.sample_key)
        {
            self.cached_bank_idx = Some(idx);
            Some(data)
        } else {
            self.cached_bank_idx = None;
            None
        };
        if !self.cursor.is_playing() {
            return [0.0, 0.0];
        }
        let Some(sample) = maybe_sample.filter(|data| data.frames > 0) else {
            self.cursor.stop();
            return [0.0, 0.0];
        };

        let sr = ctx.sample_rate;
        let semis = ((ctx.note_pitch - f64::from(smp.root_midi))
            + ctx.rack_tune
            + self.coarse_smooth
            + f64::from(smp.fine_cents) * 0.01
            + inputs.pitch_cv)
            .clamp(-96.0, 96.0);
        let ratio_key = (semis.to_bits(), sample.sample_rate, sr.to_bits());
        if ratio_key != self.cached_ratio_key {
            self.cached_ratio_key = ratio_key;
            self.last_step_ratio =
                (f64::from(sample.sample_rate) / sr) * semitones_to_ratio_f64(semis);
        }
        let step_ratio = self.last_step_ratio;
        let raw = self.cursor.step(sample, smp.region(), step_ratio, sr);
        let mut gain = f64::from(smp.level);
        if self.releasing {
            gain *= self.release_gain;
            self.release_gain *= self.release_coeff;
            if self.release_gain < SILENCE_FLOOR {
                self.release_gain = 0.0;
                self.cursor.stop();
            }
        }
        [raw[0] * gain, raw[1] * gain]
    }

    #[inline]
    fn on_note_off(&mut self, has_lifetime_envs: bool) {
        if !has_lifetime_envs
            && !(self.current.one_shot && self.current.loop_mode == SampleLoopMode::Off)
        {
            self.releasing = true;
        }
    }

    #[inline]
    fn on_force_release(&mut self, _sample_rate: f64, rel_coeff: f64) {
        self.releasing = true;
        self.release_coeff = rel_coeff;
    }

    #[inline]
    fn on_choke(&mut self, _sample_rate: f64, choke_coeff: f64) {
        self.releasing = true;
        self.release_coeff = choke_coeff;
    }
}
