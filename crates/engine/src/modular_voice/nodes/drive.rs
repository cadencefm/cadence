//! 2x oversampled multi-mode waveshaper voice node with asymmetry, DC blocker, and tone LPF.

use super::{
    write_copy_module_normalized, NodeInputs, ProcessCtx, TriggerCtx, TriggerState, VoiceLocks,
    VoiceModule,
};
use crate::modular_voice::ModuleParams;
use cadence_core::dsp::{
    db_to_lin_f64, shape_drive_oversampled_2x, DcBlocker, Halfband2x, LogSmoother, ModalResonator,
    TptOnePoleLpf,
};
use cadence_core::rack::{DriveModule, ModuleKind, ModuleKindTag, ModuleParam};

#[derive(Clone, Copy, Debug)]
pub(in crate::modular_voice) struct DriveNode {
    current: DriveModule,
    pub target: DriveModule,
    os: Halfband2x,
    tone_lpf: TptOnePoleLpf,
    dc_blocker: DcBlocker,
    tone: LogSmoother,
}

impl DriveNode {
    #[inline]
    pub(super) fn new(params: DriveModule, sample_rate: f64) -> Self {
        Self {
            current: params,
            target: params,
            os: Halfband2x::new(),
            tone_lpf: TptOnePoleLpf::new(sample_rate, f64::from(params.tone_hz)),
            dc_blocker: DcBlocker::new(sample_rate),
            tone: LogSmoother::new_clamped(f64::from(params.tone_hz), 600.0, 18_000.0),
        }
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
            self.tone.set_target(f64::from(drv.tone_hz));
            if !ctx.eff_legato {
                self.tone.snap_to_target();
                self.tone_lpf
                    .set_cutoff(ctx.sample_rate, self.tone.current());
            }
            return;
        }

        self.os = Halfband2x::new();
        self.tone_lpf = TptOnePoleLpf::new(ctx.sample_rate, f64::from(drv.tone_hz));
        self.dc_blocker = DcBlocker::new(ctx.sample_rate);
        self.tone = LogSmoother::new_clamped(f64::from(drv.tone_hz), 600.0, 18_000.0);
    }

    #[inline]
    fn smooth_step(&mut self, s64: f64) {
        let s32 = s64 as f32;
        self.current.drive += (self.target.drive - self.current.drive) * s32;
        self.current.asymmetry += (self.target.asymmetry - self.current.asymmetry) * s32;
        self.current.output_db += (self.target.output_db - self.current.output_db) * s32;
        self.current.mix += (self.target.mix - self.current.mix) * s32;
        self.current.tone_hz = self.tone.step(s64) as f32;
    }

    #[inline]
    fn control_tick(&mut self, sample_rate: f64) {
        self.tone_lpf.set_cutoff(sample_rate, self.tone.current());
    }

    #[inline]
    fn process(&mut self, inputs: &NodeInputs, _ctx: &mut ProcessCtx<'_>) -> [f64; 2] {
        let drv = self.current;
        let cv_mono = 0.5 * (inputs.cv_stereo[0] + inputs.cv_stereo[1]);
        let eff_drive = (f64::from(drv.drive) + cv_mono).clamp(0.0, 1.0);
        let asym = f64::from(drv.asymmetry).clamp(-1.0, 1.0);
        let shaped =
            shape_drive_oversampled_2x(&mut self.os, inputs.in_audio, eff_drive, asym, drv.mode);
        let dc_free = self.dc_blocker.process_stereo(shaped);
        let toned = self.tone_lpf.process_stereo(dc_free);
        let out_g = db_to_lin_f64(f64::from(drv.output_db.clamp(-12.0, 6.0)));
        let mix = f64::from(drv.mix.clamp(0.0, 1.0));
        if mix >= 1.0 - 1e-6 {
            [toned[0] * out_g, toned[1] * out_g]
        } else if mix <= 1e-6 {
            inputs.in_audio
        } else {
            let inv = 1.0 - mix;
            [
                inputs.in_audio[0] * inv + toned[0] * out_g * mix,
                inputs.in_audio[1] * inv + toned[1] * out_g * mix,
            ]
        }
    }

    #[inline]
    fn is_tail_quiet(&self, _resonator_banks: &[ModalResonator]) -> bool {
        self.os.quiet() && self.tone_lpf.quiet() && self.dc_blocker.quiet()
    }
}
