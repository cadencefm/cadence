//! Multi-mode cubic and ZDF filter voice node composing `cadence_core::dsp::MultiModeFilter`
//! via the voice's heap-hosted filter bank pool.

use super::{
    write_copy_module_normalized, NodeInputs, ProcessCtx, TailBanks, TriggerCtx, TriggerState,
    VoiceLocks, VoiceModule,
};
use crate::modular_voice::ModuleParams;
use cadence_core::dsp::math::LIN_FLOOR_F64;
use cadence_core::dsp::{
    clamp_freq_hz, control_stride_mask, filter_velocity_mod, mix_stereo_f64, octaves_to_ratio_f64,
    sanitize_clamp_f32, sanitize_clamp_f64, sanitize_sample_rate, smooth_step_f32, LogSmoother,
    MultiModeFilter, Smoother,
};
use cadence_core::rack::{FilterMode, FilterModule, ModuleKind, ModuleKindTag, ModuleParam};

const FILTER_CV_RATE_HZ: f64 = 12_000.0;
const BIG_CV_JUMP_OCTAVES: f64 = 0.25;
const MODE_XFADE_MS: f32 = 3.0;

#[derive(Clone, Copy, Debug)]
pub(in crate::modular_voice) struct FilterNode {
    current: FilterModule,
    pub target: FilterModule,
    bank_idx: u8,
    prev_mode: FilterMode,
    mode_xfade: Smoother,
    cutoff: LogSmoother,
    effective_fc: f64,
    pub effective_cutoff_hz: f32,
    effective_resonance: f32,
    last_cv_octaves: f64,
    cv_sub_counter: u32,
    cv_subrate_mask: u32,
    has_cv: bool,
    cached_sr_bits: u64,
}

impl FilterNode {
    #[inline]
    pub(super) fn new(params: FilterModule, sample_rate: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let cutoff = LogSmoother::new(f64::from(params.cutoff_hz), 20.0);
        let fc = clamp_freq_hz(cutoff.current(), 20.0, sr, 1_000.0);
        Self {
            current: params,
            target: params,
            bank_idx: 0,
            prev_mode: params.mode,
            mode_xfade: Smoother::new(1.0, MODE_XFADE_MS, sr),
            cutoff,
            effective_fc: fc,
            effective_cutoff_hz: fc as f32,
            effective_resonance: sanitize_clamp_f32(params.resonance, 0.0, 0.95, 0.0),
            last_cv_octaves: f64::NAN,
            cv_sub_counter: 0,
            cv_subrate_mask: control_stride_mask(sr, FILTER_CV_RATE_HZ),
            has_cv: false,
            cached_sr_bits: sr.to_bits(),
        }
    }

    #[cfg(test)]
    pub(in crate::modular_voice) fn cv_subrate_mask(&self) -> u32 {
        self.cv_subrate_mask
    }
}

impl VoiceModule for FilterNode {
    #[inline]
    fn tag(&self) -> ModuleKindTag {
        ModuleKindTag::Filter
    }

    #[inline]
    fn target_params(&self) -> ModuleParams {
        ModuleParams::Filter(self.target)
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
            ModuleKind::Filter,
            |k| match k {
                ModuleKind::Filter(m) => Some(m),
                _ => None,
            },
            param,
            norm,
            total_frames,
        )
    }

    #[inline]
    fn sync_target_logs(&mut self) {
        self.cutoff.set_target(f64::from(self.target.cutoff_hz));
    }

    #[inline]
    fn update_continuous(&mut self, src: &ModuleKind, module_id: u16, locks: VoiceLocks<'_>) {
        let ModuleKind::Filter(s) = src else {
            return;
        };
        if !locks.is_module_locked(module_id, ModuleParam::Cutoff) {
            self.target.cutoff_hz = s.cutoff_hz;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Resonance) {
            self.target.resonance = s.resonance;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Drive) {
            self.target.drive = s.drive;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Kerr) {
            self.target.kerr_semitones = s.kerr_semitones;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Spread) {
            self.target.spread = s.spread;
        }
        if !locks.is_module_locked(module_id, ModuleParam::KeyTrack) {
            self.target.key_tracking = s.key_tracking;
        }
        if !locks.is_module_locked(module_id, ModuleParam::VelAmount) {
            self.target.vel_amount = s.vel_amount;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Mix) {
            self.target.mix = s.mix;
        }
        self.sync_target_logs();
    }

    #[inline]
    fn on_trigger(&mut self, ctx: &TriggerCtx, state: &mut TriggerState<'_>) {
        let prev_mode = self.current.mode;
        let flt = self.target;
        if ctx.eff_legato {
            self.current.mode = flt.mode;
        } else {
            self.current = flt;
        }

        let sr = sanitize_sample_rate(ctx.sample_rate);
        let sr_bits = sr.to_bits();
        self.bank_idx = ctx.slot;
        self.cv_subrate_mask = control_stride_mask(sr, FILTER_CV_RATE_HZ);
        self.cv_sub_counter = 0;

        if ctx.eff_retarget {
            let bank_rebuilt = state
                .filter_banks
                .get_mut(usize::from(self.bank_idx))
                .is_some_and(|bank| bank.sync_sample_rate(sr));
            if bank_rebuilt || sr_bits != self.cached_sr_bits {
                self.cached_sr_bits = sr_bits;
                self.mode_xfade = Smoother::new(1.0, MODE_XFADE_MS, sr);
                self.prev_mode = flt.mode;
                self.last_cv_octaves = f64::NAN;
            } else if prev_mode != flt.mode {
                if let Some(bank) = state.filter_banks.get_mut(usize::from(self.bank_idx)) {
                    let prev_topo = MultiModeFilter::topology_id(prev_mode);
                    let next_topo = MultiModeFilter::topology_id(flt.mode);
                    if self.prev_mode != prev_mode {
                        let abandoned_topo = MultiModeFilter::topology_id(self.prev_mode);
                        if abandoned_topo != prev_topo && abandoned_topo != next_topo {
                            bank.filter.reset_topology_for(self.prev_mode);
                        }
                    }
                    if prev_topo != next_topo || (next_topo == 1 && bank.filter.quiet()) {
                        bank.filter.reset_topology_for(flt.mode);
                    }
                }
                self.prev_mode = prev_mode;
                self.mode_xfade.snap(0.0);
                self.mode_xfade.set_target(1.0);
                self.last_cv_octaves = f64::NAN;
            }
            self.has_cv = ctx.has_cv;
            self.cutoff.set_target(f64::from(flt.cutoff_hz));
            if !ctx.eff_legato {
                self.cutoff.snap_to_target();
            }
            let fc = clamp_freq_hz(self.cutoff.current(), 20.0, sr, 1_000.0);
            self.effective_fc = fc;
            self.effective_cutoff_hz = fc as f32;
            self.effective_resonance = sanitize_clamp_f32(self.current.resonance, 0.0, 0.95, 0.0);
            return;
        }

        if let Some(bank) = state.filter_banks.get_mut(usize::from(self.bank_idx)) {
            bank.reset_for_rate(sr);
        }
        if sr_bits != self.cached_sr_bits {
            self.cached_sr_bits = sr_bits;
            self.mode_xfade = Smoother::new(1.0, MODE_XFADE_MS, sr);
        } else {
            self.mode_xfade.snap(1.0);
        }
        self.prev_mode = flt.mode;
        self.cutoff = LogSmoother::new(f64::from(flt.cutoff_hz), 20.0);
        let fc = clamp_freq_hz(self.cutoff.current(), 20.0, sr, 1_000.0);
        self.effective_fc = fc;
        self.effective_cutoff_hz = fc as f32;
        self.effective_resonance = sanitize_clamp_f32(flt.resonance, 0.0, 0.95, 0.0);
        self.last_cv_octaves = f64::NAN;
        self.has_cv = ctx.has_cv;
    }

    #[inline]
    fn smooth_step(&mut self, s64: f64) {
        let s32 = s64 as f32;
        self.current.cutoff_hz = self.cutoff.step(s64) as f32;
        smooth_step_f32(&mut self.current.resonance, self.target.resonance, s32);
        smooth_step_f32(&mut self.current.drive, self.target.drive, s32);
        smooth_step_f32(
            &mut self.current.kerr_semitones,
            self.target.kerr_semitones,
            s32,
        );
        smooth_step_f32(&mut self.current.spread, self.target.spread, s32);
        smooth_step_f32(
            &mut self.current.key_tracking,
            self.target.key_tracking,
            s32,
        );
        smooth_step_f32(&mut self.current.vel_amount, self.target.vel_amount, s32);
        smooth_step_f32(&mut self.current.mix, self.target.mix, s32);
    }

    #[inline]
    fn process(&mut self, inputs: &NodeInputs, ctx: &mut ProcessCtx<'_>) -> [f64; 2] {
        let flt = self.current;
        let sr = ctx.sample_rate;
        let cv_octaves = inputs.cv_mono();
        let cv_uninit = !self.last_cv_octaves.is_finite();
        let cv_delta = if cv_uninit {
            0.0
        } else {
            (cv_octaves - self.last_cv_octaves).abs()
        };
        let sub_tick = self.cv_sub_counter == 0;
        let big_jump = self.has_cv && cv_delta >= BIG_CV_JUMP_OCTAVES;
        let need_update = cv_uninit
            || ctx.control_tick
            || big_jump
            || (self.has_cv && sub_tick && cv_delta > 1e-5);
        self.cv_sub_counter = if need_update {
            1 & self.cv_subrate_mask
        } else {
            self.cv_sub_counter.wrapping_add(1) & self.cv_subrate_mask
        };
        if need_update {
            self.last_cv_octaves = cv_octaves;
            let key_oct = (ctx.note_pitch - 60.0) / 12.0 * f64::from(flt.key_tracking);
            let vel_mod = filter_velocity_mod(
                flt.mode,
                ctx.velocity,
                inputs.filter_env_level,
                flt.vel_amount,
                flt.resonance,
            );
            let total_oct = sanitize_clamp_f64(
                key_oct + cv_octaves + vel_mod.cutoff_octaves,
                -12.0,
                12.0,
                0.0,
            );
            let fc = clamp_freq_hz(
                self.cutoff.current() * octaves_to_ratio_f64(total_oct),
                20.0,
                sr,
                1_000.0,
            );
            self.effective_fc = fc;
            self.effective_cutoff_hz = fc as f32;
            self.effective_resonance = vel_mod.resonance;
        }

        let Some(bank) = ctx.filter_banks.get_mut(usize::from(self.bank_idx)) else {
            return inputs.in_audio;
        };

        let drv = f64::from(sanitize_clamp_f32(flt.drive, 0.0, 1.0, 0.0));
        let kerr = f64::from(sanitize_clamp_f32(flt.kerr_semitones, -12.0, 12.0, 0.0));
        let spread = f64::from(sanitize_clamp_f32(flt.spread, 0.0, 1.0, 0.0));
        let m_xf = self.mode_xfade.next();
        let wet = if m_xf >= 1.0 - LIN_FLOOR_F64 {
            if self.prev_mode != flt.mode {
                if MultiModeFilter::topology_id(self.prev_mode)
                    != MultiModeFilter::topology_id(flt.mode)
                {
                    bank.filter.reset_topology_for(self.prev_mode);
                }
                self.prev_mode = flt.mode;
            }
            bank.filter.process_stereo(
                inputs.in_audio,
                flt.mode,
                self.effective_fc,
                self.effective_resonance,
                drv,
                kerr,
                spread,
            )
        } else {
            bank.filter.process_stereo_crossfade(
                inputs.in_audio,
                self.prev_mode,
                flt.mode,
                m_xf,
                self.effective_fc,
                self.effective_resonance,
                drv,
                kerr,
                spread,
            )
        };
        mix_stereo_f64(inputs.in_audio, wet, f64::from(flt.mix))
    }

    #[inline]
    fn is_tail_quiet(&self, banks: TailBanks<'_>) -> bool {
        banks
            .filter_banks
            .get(usize::from(self.bank_idx))
            .is_none_or(|bank| bank.filter.quiet())
    }
}
