//! Multi-mode ZDF filter voice node (SVF 12 dB, Acid ladder 18/24 dB, and TPT 6 dB LPF).

use super::{
    write_copy_module_normalized, NodeInputs, ProcessCtx, TriggerCtx, TriggerState, VoiceLocks,
    VoiceModule,
};
use crate::modular_voice::ModuleParams;
use cadence_core::dsp::{
    clamp_freq_hz, control_stride_mask, filter_velocity_mod, mix_stereo_f64, octaves_to_ratio_f64,
    sanitize_clamp_f32, sanitize_clamp_f64, smooth_step_f32, AcidCoeffs, AcidLadder, LogSmoother,
    ModalResonator, SoftClipCoeffs, Svf, SvfCoeffs, TptOnePoleLpf,
};
use cadence_core::rack::{FilterMode, FilterModule, ModuleKind, ModuleKindTag, ModuleParam};

const FILTER_CV_RATE_HZ: f64 = 12_000.0;
const BIG_CV_JUMP_OCTAVES: f64 = 0.25;

#[derive(Clone, Copy, Debug)]
pub(in crate::modular_voice) struct FilterNode {
    current: FilterModule,
    pub target: FilterModule,
    svf: Svf,
    acid: AcidLadder,
    one_pole: TptOnePoleLpf,
    svf_coeffs: SvfCoeffs,
    acid_coeffs: AcidCoeffs,
    cutoff: LogSmoother,
    pub effective_cutoff_hz: f32,
    last_cv_octaves: f64,
    cv_sub_counter: u32,
    cv_subrate_mask: u32,
    has_cv: bool,
    cached_drive_bits: u64,
    cached_drive_coeffs: SoftClipCoeffs,
    cached_coeff_key: (FilterMode, u64, u32, u64),
}

impl FilterNode {
    #[inline]
    pub(super) fn new(params: FilterModule, sample_rate: f64) -> Self {
        let fc = clamp_freq_hz(f64::from(params.cutoff_hz), 20.0, sample_rate, 1_000.0);
        let drv = f64::from(sanitize_clamp_f32(params.drive, 0.0, 1.0, 0.0));
        Self {
            current: params,
            target: params,
            svf: Svf::default(),
            acid: AcidLadder::default(),
            one_pole: TptOnePoleLpf::new(sample_rate, fc),
            svf_coeffs: SvfCoeffs::new(sample_rate, fc, params.resonance),
            acid_coeffs: AcidCoeffs::new(sample_rate, fc, params.resonance),
            cutoff: LogSmoother::new(f64::from(params.cutoff_hz), 20.0),
            effective_cutoff_hz: fc as f32,
            last_cv_octaves: f64::NAN,
            cv_sub_counter: 0,
            cv_subrate_mask: control_stride_mask(sample_rate, FILTER_CV_RATE_HZ),
            has_cv: false,
            cached_drive_bits: drv.to_bits(),
            cached_drive_coeffs: SoftClipCoeffs::new(drv),
            cached_coeff_key: (
                params.mode,
                fc.to_bits(),
                params.resonance.to_bits(),
                sample_rate.to_bits(),
            ),
        }
    }

    #[cfg(test)]
    pub(in crate::modular_voice) fn cv_subrate_mask(&self) -> u32 {
        self.cv_subrate_mask
    }

    #[inline]
    fn update_coeffs(
        &mut self,
        sample_rate: f64,
        mode: FilterMode,
        cutoff_hz: f64,
        resonance: f32,
    ) {
        let key = (
            mode,
            cutoff_hz.to_bits(),
            resonance.to_bits(),
            sample_rate.to_bits(),
        );
        if key == self.cached_coeff_key {
            return;
        }
        self.cached_coeff_key = key;
        match mode {
            FilterMode::AcidLowPass => {
                self.acid_coeffs = AcidCoeffs::new(sample_rate, cutoff_hz, resonance);
            }
            FilterMode::LowPass6 => {
                self.one_pole.set_cutoff(sample_rate, cutoff_hz);
            }
            _ => {
                self.svf_coeffs = SvfCoeffs::new(sample_rate, cutoff_hz, resonance);
            }
        }
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
    fn on_trigger(&mut self, ctx: &TriggerCtx, _state: &mut TriggerState<'_>) {
        let prev_mode = self.current.mode;
        let flt = self.target;
        if ctx.eff_legato {
            self.current.mode = flt.mode;
        } else {
            self.current = flt;
        }

        let sr = ctx.sample_rate;
        self.cv_subrate_mask = control_stride_mask(sr, FILTER_CV_RATE_HZ);
        self.cv_sub_counter = 0;
        if ctx.eff_retarget {
            if prev_mode != flt.mode {
                self.svf.reset();
                self.acid.reset();
                self.one_pole.reset();
                self.last_cv_octaves = f64::NAN;
            }
            self.has_cv = ctx.has_cv;
            self.cutoff.set_target(f64::from(flt.cutoff_hz));
            if !ctx.eff_legato {
                self.cutoff.snap_to_target();
            }
            let fc = clamp_freq_hz(self.cutoff.current(), 20.0, sr, 1_000.0);
            self.effective_cutoff_hz = fc as f32;
            self.update_coeffs(sr, self.current.mode, fc, self.current.resonance);
            return;
        }

        let fc = clamp_freq_hz(f64::from(flt.cutoff_hz), 20.0, sr, 1_000.0);
        self.svf = Svf::default();
        self.acid = AcidLadder::default();
        self.one_pole = TptOnePoleLpf::new(sr, fc);
        self.svf_coeffs = SvfCoeffs::new(sr, fc, flt.resonance);
        self.acid_coeffs = AcidCoeffs::new(sr, fc, flt.resonance);
        self.cached_coeff_key = (
            flt.mode,
            fc.to_bits(),
            flt.resonance.to_bits(),
            sr.to_bits(),
        );
        self.cutoff = LogSmoother::new(f64::from(flt.cutoff_hz), 20.0);
        self.effective_cutoff_hz = fc as f32;
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
        let cv_octaves = 0.5 * (inputs.cv_stereo[0] + inputs.cv_stereo[1]);
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
            self.effective_cutoff_hz = fc as f32;
            self.update_coeffs(sr, flt.mode, fc, vel_mod.resonance);
        }

        let drv = f64::from(sanitize_clamp_f32(flt.drive, 0.0, 1.0, 0.0));
        let driven = if drv > 0.0 {
            let drv_bits = drv.to_bits();
            if drv_bits != self.cached_drive_bits {
                self.cached_drive_bits = drv_bits;
                self.cached_drive_coeffs = SoftClipCoeffs::new(drv);
            }
            let c = self.cached_drive_coeffs;
            [c.shape(inputs.in_audio[0]), c.shape(inputs.in_audio[1])]
        } else {
            inputs.in_audio
        };
        let wet = match flt.mode {
            FilterMode::AcidLowPass => self.acid.process_stereo(driven, self.acid_coeffs),
            FilterMode::LowPass6 => self.one_pole.process_stereo(driven),
            mode => self.svf.process_stereo(driven, self.svf_coeffs, mode),
        };
        mix_stereo_f64(inputs.in_audio, wet, f64::from(flt.mix))
    }

    #[inline]
    fn is_tail_quiet(&self, _resonator_banks: &[ModalResonator]) -> bool {
        self.svf.quiet() && self.acid.quiet() && self.one_pole.quiet()
    }
}
