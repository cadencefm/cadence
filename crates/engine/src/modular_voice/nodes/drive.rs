//! 2x oversampled multi-mode waveshaper voice node with asymmetry, DC blocker, and tone LPF.

use super::{
    write_copy_module_normalized, NodeInputs, ProcessCtx, TriggerCtx, TriggerState, VoiceLocks,
    VoiceModule,
};
use crate::modular_voice::ModuleParams;
use cadence_core::dsp::{
    db_to_lin_f64, mix_stereo_f64, sanitize_clamp_f32, sanitize_clamp_f64,
    shape_drive_oversampled_2x_coeffs, smooth_step_f32, DcBlocker, DriveShaperCoeffs, Halfband2x,
    Halfband2xDryAlign, LogSmoother, ModalResonator, TptOnePoleLpf, DRIVE_MAX_OUTPUT_DB,
    DRIVE_MAX_TONE_HZ, DRIVE_MIN_OUTPUT_DB, DRIVE_MIN_TONE_HZ,
};
use cadence_core::mixer::DriveMode;
use cadence_core::rack::{DriveModule, ModuleKind, ModuleKindTag, ModuleParam};

#[derive(Clone, Copy, Debug)]
pub(in crate::modular_voice) struct DriveNode {
    current: DriveModule,
    pub target: DriveModule,
    os: Halfband2x,
    dry_align: Halfband2xDryAlign,
    tone_lpf: TptOnePoleLpf,
    dc_blocker: DcBlocker,
    tone: LogSmoother,
    cached_shaper_key: (u64, u64, DriveMode),
    cached_shaper: DriveShaperCoeffs,
    cached_out_db_bits: u32,
    cached_out_gain: f64,
    cached_tone_key: (u64, u64),
}

impl DriveNode {
    #[inline]
    pub(super) fn new(params: DriveModule, sample_rate: f64) -> Self {
        let init_drive = sanitize_clamp_f64(f64::from(params.drive), 0.0, 1.0, 0.0);
        let init_asym = sanitize_clamp_f64(f64::from(params.asymmetry), -1.0, 1.0, 0.0);
        let clamped_db = sanitize_clamp_f32(
            params.output_db,
            DRIVE_MIN_OUTPUT_DB,
            DRIVE_MAX_OUTPUT_DB,
            0.0,
        );
        let tone = LogSmoother::new_clamped(
            f64::from(params.tone_hz),
            f64::from(DRIVE_MIN_TONE_HZ),
            f64::from(DRIVE_MAX_TONE_HZ),
        );
        let tone_hz = tone.current();
        Self {
            current: params,
            target: params,
            os: Halfband2x::new(),
            dry_align: Halfband2xDryAlign::new(),
            tone_lpf: TptOnePoleLpf::new(sample_rate, tone_hz),
            dc_blocker: DcBlocker::new(sample_rate),
            cached_tone_key: (tone_hz.to_bits(), sample_rate.to_bits()),
            tone,
            cached_shaper_key: (init_drive.to_bits(), init_asym.to_bits(), params.mode),
            cached_shaper: DriveShaperCoeffs::new(init_drive, init_asym, params.mode),
            cached_out_db_bits: clamped_db.to_bits(),
            cached_out_gain: db_to_lin_f64(f64::from(clamped_db)),
        }
    }

    #[cfg(test)]
    pub(in crate::modular_voice) fn cached_tone_sr_bits(&self) -> u64 {
        self.cached_tone_key.1
    }
}

impl VoiceModule for DriveNode {
    #[inline]
    fn tag(&self) -> ModuleKindTag {
        ModuleKindTag::Drive
    }

    #[inline]
    fn target_params(&self) -> ModuleParams {
        ModuleParams::Drive(self.target)
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
            ModuleKind::Drive,
            |k| match k {
                ModuleKind::Drive(m) => Some(m),
                _ => None,
            },
            param,
            norm,
            total_frames,
        )
    }

    #[inline]
    fn sync_target_logs(&mut self) {
        self.tone.set_target(f64::from(self.target.tone_hz));
    }

    #[inline]
    fn update_continuous(&mut self, src: &ModuleKind, module_id: u16, locks: VoiceLocks<'_>) {
        let ModuleKind::Drive(s) = src else {
            return;
        };
        if !locks.is_module_locked(module_id, ModuleParam::Drive) {
            self.target.drive = s.drive;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Asymmetry) {
            self.target.asymmetry = s.asymmetry;
        }
        if !locks.is_module_locked(module_id, ModuleParam::ToneHz) {
            self.target.tone_hz = s.tone_hz;
        }
        if !locks.is_module_locked(module_id, ModuleParam::OutputDb) {
            self.target.output_db = s.output_db;
        }
        if !locks.is_module_locked(module_id, ModuleParam::Mix) {
            self.target.mix = s.mix;
        }
        self.sync_target_logs();
    }

    #[inline]
    fn on_trigger(&mut self, ctx: &TriggerCtx, _state: &mut TriggerState<'_>) {
        let drv = self.target;
        if ctx.eff_legato {
            self.current.mode = drv.mode;
        } else {
            self.current = drv;
        }

        if ctx.eff_retarget {
            self.dc_blocker.set_sample_rate(ctx.sample_rate);
            self.tone.set_target(f64::from(drv.tone_hz));
            if !ctx.eff_legato {
                self.tone.snap_to_target();
                let hz = self.tone.current();
                self.tone_lpf.set_cutoff(ctx.sample_rate, hz);
                self.cached_tone_key = (hz.to_bits(), ctx.sample_rate.to_bits());
            }
            return;
        }

        self.os = Halfband2x::new();
        self.dry_align = Halfband2xDryAlign::new();
        self.tone = LogSmoother::new_clamped(
            f64::from(drv.tone_hz),
            f64::from(DRIVE_MIN_TONE_HZ),
            f64::from(DRIVE_MAX_TONE_HZ),
        );
        let hz = self.tone.current();
        self.tone_lpf = TptOnePoleLpf::new(ctx.sample_rate, hz);
        self.cached_tone_key = (hz.to_bits(), ctx.sample_rate.to_bits());
        self.dc_blocker = DcBlocker::new(ctx.sample_rate);
    }

    #[inline]
    fn smooth_step(&mut self, s64: f64) {
        let s32 = s64 as f32;
        smooth_step_f32(&mut self.current.drive, self.target.drive, s32);
        smooth_step_f32(&mut self.current.asymmetry, self.target.asymmetry, s32);
        smooth_step_f32(&mut self.current.output_db, self.target.output_db, s32);
        smooth_step_f32(&mut self.current.mix, self.target.mix, s32);
        self.current.tone_hz = self.tone.step(s64) as f32;
    }

    #[inline]
    fn control_tick(&mut self, sample_rate: f64) {
        let hz = self.tone.current();
        let key = (hz.to_bits(), sample_rate.to_bits());
        if key != self.cached_tone_key {
            self.cached_tone_key = key;
            self.tone_lpf.set_cutoff(sample_rate, hz);
        }
    }

    #[inline]
    fn process(&mut self, inputs: &NodeInputs, _ctx: &mut ProcessCtx<'_>) -> [f64; 2] {
        let drv = self.current;
        let cv_mono = inputs.cv_mono();
        let eff_drive = sanitize_clamp_f64(f64::from(drv.drive) + cv_mono, 0.0, 1.0, 0.0);
        let asym = sanitize_clamp_f64(f64::from(drv.asymmetry), -1.0, 1.0, 0.0);
        let shaper_key = (eff_drive.to_bits(), asym.to_bits(), drv.mode);
        if shaper_key != self.cached_shaper_key {
            self.cached_shaper_key = shaper_key;
            self.cached_shaper = DriveShaperCoeffs::new(eff_drive, asym, drv.mode);
        }
        let dry_aligned = self.dry_align.process_stereo(inputs.in_audio);
        let shaped =
            shape_drive_oversampled_2x_coeffs(&mut self.os, inputs.in_audio, self.cached_shaper);
        let dc_free = self.dc_blocker.process_stereo(shaped);
        let toned = self.tone_lpf.process_stereo(dc_free);
        let clamped_db =
            sanitize_clamp_f32(drv.output_db, DRIVE_MIN_OUTPUT_DB, DRIVE_MAX_OUTPUT_DB, 0.0);
        let out_db_bits = clamped_db.to_bits();
        if out_db_bits != self.cached_out_db_bits {
            self.cached_out_db_bits = out_db_bits;
            self.cached_out_gain = db_to_lin_f64(f64::from(clamped_db));
        }
        let out_g = self.cached_out_gain;
        let wet = [toned[0] * out_g, toned[1] * out_g];
        mix_stereo_f64(dry_aligned, wet, f64::from(drv.mix))
    }

    #[inline]
    fn is_tail_quiet(
        &self,
        _resonator_banks: &[ModalResonator],
        _mobius_banks: &[cadence_core::dsp::MobiusSynth],
    ) -> bool {
        self.os.quiet()
            && self.dry_align.quiet()
            && self.tone_lpf.quiet()
            && self.dc_blocker.quiet()
    }
}
