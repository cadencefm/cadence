//! Continuous zero-pop `1×`-rate cubic saturation waveshapers, voice safety guard,
//! and Cadence's multi-mode `Drive` insert processor (`AtomicJet` / `ShockPort`).
//!
//! All drive waveshapers compose closed-form cubic primitives from
//! [`crate::dsp::cubic::math`] and satisfy exact identity `f(x, 0.0) == x` with
//! smooth continuity across `drive ∈ [0.0, 1.0]` for click-free automation from zero.

use crate::dsp::biquad::{DcBlocker, TptOnePoleLpf};
use crate::dsp::cubic::math::{atomic_jet, atomic_pair_scalar};
use crate::dsp::math::{
    clamp_freq_hz, control_stride_last_tick, control_stride_mask, db_to_lin, db_to_lin_f64,
    mix_stereo_f64, one_pole_coeff_ms, sanitize_clamp_f64, sanitize_frame, sanitize_sample,
    sanitize_sample_f64, sanitize_sample_rate, BypassFader, BypassStep, LogSmoother, Smoother,
    LIN_FLOOR_F64, NYQUIST_SAFE_RATIO,
};
use crate::mixer::{DriveMode, DrivePatch};

/// Minimum post-shaper tone low-pass cutoff in Hz shared by `Drive` and `DriveModule`.
pub const DRIVE_MIN_TONE_HZ: f32 = 600.0;
/// Maximum post-shaper tone low-pass cutoff in Hz shared by `Drive` and `DriveModule`.
pub const DRIVE_MAX_TONE_HZ: f32 = 18_000.0;
/// Minimum post-shaper output trim in dB shared by `Drive` and `DriveModule`.
pub const DRIVE_MIN_OUTPUT_DB: f32 = -12.0;
/// Maximum post-shaper output trim in dB shared by `Drive` and `DriveModule`.
pub const DRIVE_MAX_OUTPUT_DB: f32 = 6.0;

/// Precomputed parameter-only coefficients for [`soft_clip_drive_f64`], avoiding
/// redundant per-sample `powf` and `tanh(k)` calls across stereo channels and steady-state frames.
#[derive(Clone, Copy, Debug)]
pub struct SoftClipCoeffs {
    k: f64,
    scale: f64,
    bypass: bool,
    linear_only: bool,
}

impl SoftClipCoeffs {
    pub const BYPASS: Self = Self {
        k: 0.0,
        scale: 1.0,
        bypass: true,
        linear_only: false,
    };

    #[inline]
    pub fn new(drive: f64) -> Self {
        let d = sanitize_sample_f64(drive).clamp(0.0, 1.0);
        if d <= 0.0 {
            return Self::BYPASS;
        }
        let k = d * 5.0;
        let comp = (1.0 + k).powf(-0.22);
        if k < 1e-4 {
            return Self {
                k,
                scale: comp,
                bypass: false,
                linear_only: true,
            };
        }
        Self {
            k,
            scale: comp / k.tanh(),
            bypass: false,
            linear_only: false,
        }
    }

    #[inline]
    pub fn shape(self, x: f64) -> f64 {
        let x = sanitize_sample_f64(x);
        if self.bypass {
            return x;
        }
        if self.linear_only {
            return x * self.scale;
        }
        (self.k * x).tanh() * self.scale
    }
}

/// Symmetric soft-clipping saturator in `f64` for synth voices and filter pre-drive.
///
/// Satisfies `soft_clip_drive_f64(x, 0.0) == x` exactly, is `C^∞` smooth across
/// `drive ∈ [0.0, 1.0]`, odd-symmetric (zero DC offset), and strictly bounded.
#[inline]
pub fn soft_clip_drive_f64(x: f64, drive: f64) -> f64 {
    SoftClipCoeffs::new(drive).shape(x)
}

/// Symmetric soft-clipping saturator (`f32` wrapper over [`soft_clip_drive_f64`]).
#[inline]
pub fn soft_clip_drive(x: f32, drive: f32) -> f32 {
    soft_clip_drive_f64(f64::from(x), f64::from(drive)) as f32
}

#[derive(Clone, Copy, Debug)]
enum ShaperParams {
    Bypass,
    Linear {
        gain: f64,
    },
    AtomicJet {
        scale: f64,
        asym_eff: f64,
        out_gain: f64,
    },
    ShockPort {
        eta: f64,
        p_pos: f64,
        p_neg: f64,
        gain_pos: f64,
        gain_neg: f64,
    },
}

/// Precomputed parameter-only `1×`-rate cubic waveshaper coefficients shared by
/// [`drive_shape_mode_asym_f64`], [`Drive`], and `cadence-engine`'s `DriveNode`.
///
/// Computing parameter-only exponentials, powers, and headroom thresholds once per
/// parameter update eliminates redundant per-sample transcendental work across stereo
/// channels and steady-state frames while keeping the transfer function strictly identical.
#[derive(Clone, Copy, Debug)]
pub struct DriveShaperCoeffs {
    params: ShaperParams,
}

impl DriveShaperCoeffs {
    pub const BYPASS: Self = Self {
        params: ShaperParams::Bypass,
    };

    #[inline]
    pub fn new(drive: f64, asymmetry: f64, mode: DriveMode) -> Self {
        let d = sanitize_sample_f64(drive).clamp(0.0, 1.0);
        if d <= 0.0 {
            return Self::BYPASS;
        }
        let a = sanitize_sample_f64(asymmetry).clamp(-1.0, 1.0);
        let params = match mode {
            DriveMode::AtomicJet => {
                let k = d * (1.5 + 10.5 * d);
                let out_gain = (1.0 + k * k).sqrt() * (1.0 + k).powf(-0.18);
                if k <= 1e-9 {
                    ShaperParams::Linear { gain: out_gain }
                } else {
                    let scale = 1.0 / k;
                    let asym_eff = if a == 0.0 {
                        0.0
                    } else {
                        (0.90 * a * (1.0 - (-6.0 * d).exp())).clamp(-0.95, 0.95)
                    };
                    ShaperParams::AtomicJet {
                        scale,
                        asym_eff,
                        out_gain,
                    }
                }
            }
            DriveMode::ShockPort => {
                let p_base = 1.0 / (1.0 + 5.5 * d * d);
                let eta = d * (0.8 + 4.2 * d);
                let makeup = (1.0 / p_base).powf(0.32);
                if a == 0.0 {
                    let comp = (2.0 * eta * p_base).exp();
                    let gain = comp * makeup;
                    ShaperParams::ShockPort {
                        eta,
                        p_pos: p_base,
                        p_neg: p_base,
                        gain_pos: gain,
                        gain_neg: gain,
                    }
                } else {
                    let asym_eff = 0.55 * a * (1.0 - (-6.0 * d).exp());
                    let p_pos = p_base * (1.0 - asym_eff);
                    let p_neg = p_base * (1.0 + asym_eff);
                    let comp_pos = (2.0 * eta * p_pos).exp();
                    let comp_neg = (2.0 * eta * p_neg).exp();
                    ShaperParams::ShockPort {
                        eta,
                        p_pos,
                        p_neg,
                        gain_pos: comp_pos * makeup,
                        gain_neg: comp_neg * makeup,
                    }
                }
            }
        };
        Self { params }
    }

    #[inline]
    pub const fn is_bypass(self) -> bool {
        matches!(self.params, ShaperParams::Bypass)
    }

    #[inline]
    pub fn shape(self, x: f64) -> f64 {
        let x = sanitize_sample_f64(x);
        match self.params {
            ShaperParams::Bypass => x,
            ShaperParams::Linear { gain } => x * gain,
            ShaperParams::AtomicJet {
                scale,
                asym_eff,
                out_gain,
            } => atomic_jet(x, scale, asym_eff) * out_gain,
            ShaperParams::ShockPort {
                eta,
                p_pos,
                p_neg,
                gain_pos,
                gain_neg,
            } => {
                if x >= 0.0 {
                    atomic_pair_scalar(x, 0.0, p_pos, eta) * gain_pos
                } else {
                    atomic_pair_scalar(x, 0.0, p_neg, eta) * gain_neg
                }
            }
        }
    }
}

/// Single-point-of-truth `1×`-rate cubic waveshaper in `f64` for all [`DriveMode`] circuits
/// with continuous bipolar `asymmetry ∈ [-1.0, 1.0]`, shared by both real-time
/// [`Drive::process`] and UI transfer plots ([`drive_transfer_sample`]).
///
/// - [`DriveMode::AtomicJet`]: Slope-invariant algebraic `C^∞` jet deflection curve
///   composing [`atomic_jet`]; odd-symmetric at `asymmetry == 0.0`, while non-zero
///   `asymmetry` splits positive and negative ceilings asymmetrically with constant
///   small-signal slope at `0.0`.
/// - [`DriveMode::ShockPort`]: Opposing-biased ambient-air headroom shaper composing
///   [`atomic_pair_scalar`] with exact linear interior (`0.000%` THD inside
///   `|x| <= min(p_pos, p_neg)`) and finite-time Riccati clamping above threshold;
///   non-zero `asymmetry` splits positive and negative headroom thresholds cleanly.
///
/// Every mode satisfies exact `drive_shape_mode_asym_f64(x, 0.0, asym, mode) == x`,
/// `drive_shape_mode_asym_f64(0.0, d, asym, mode) == 0.0`, odd symmetry when
/// `asymmetry == 0.0`, and finite smooth output across extreme inputs.
#[inline]
pub fn drive_shape_mode_asym_f64(x: f64, drive: f64, asymmetry: f64, mode: DriveMode) -> f64 {
    DriveShaperCoeffs::new(drive, asymmetry, mode).shape(x)
}

/// Multi-mode cubic saturation waveshaper with asymmetry (`f32` wrapper over [`drive_shape_mode_asym_f64`]).
#[inline]
pub fn drive_shape_mode_asym(x: f32, drive: f32, asymmetry: f32, mode: DriveMode) -> f32 {
    drive_shape_mode_asym_f64(f64::from(x), f64::from(drive), f64::from(asymmetry), mode) as f32
}

/// Single-point-of-truth symmetric cubic waveshaper in `f64` (`asymmetry = 0.0`), delegating
/// directly to [`drive_shape_mode_asym_f64`].
#[inline]
pub fn drive_shape_mode_f64(x: f64, drive: f64, mode: DriveMode) -> f64 {
    drive_shape_mode_asym_f64(x, drive, 0.0, mode)
}

/// Multi-mode symmetric cubic saturation waveshaper (`f32` wrapper over [`drive_shape_mode_f64`]).
#[inline]
pub fn drive_shape_mode(x: f32, drive: f32, mode: DriveMode) -> f32 {
    drive_shape_mode_f64(f64::from(x), f64::from(drive), mode) as f32
}

/// Asymmetric cubic `AtomicJet` waveshaper in `f64`, delegating directly to
/// [`DriveShaperCoeffs`] in [`DriveMode::AtomicJet`].
#[inline]
pub fn asymmetric_drive_shape_f64(x: f64, drive: f64, asymmetry: f64) -> f64 {
    DriveShaperCoeffs::new(drive, asymmetry, DriveMode::AtomicJet).shape(x)
}

/// Asymmetric cubic `AtomicJet` waveshaper (`f32` wrapper over [`asymmetric_drive_shape_f64`]).
#[inline]
pub fn asymmetric_drive_shape(x: f32, drive: f32, asymmetry: f32) -> f32 {
    asymmetric_drive_shape_f64(f64::from(x), f64::from(drive), f64::from(asymmetry)) as f32
}

/// Transparent `C¹`-continuous soft-knee safety guard (`f64`).
///
/// Passes inputs with `|x| <= knee` bit-identically untouched, then compresses
/// overshoot smoothly via `tanh` into `(-knee - headroom, knee + headroom)`
/// with unit slope at `|x| == knee`. Non-finite inputs return `0.0`.
///
/// Strictly for per-voice anti-blowup or feedback-loop bounding; NEVER use
/// behind a Master bus or Limiter!
#[inline]
pub fn soft_knee_guard_f64(x: f64, knee: f64, headroom: f64) -> f64 {
    let x = sanitize_sample_f64(x);
    let knee = if knee.is_finite() { knee.max(0.0) } else { 0.0 };
    let a = x.abs();
    if a <= knee {
        return x;
    }
    if !headroom.is_finite() || headroom <= 0.0 {
        return x.signum() * knee;
    }
    x.signum() * (knee + headroom * ((a - knee) / headroom).tanh())
}

/// Linear pass-through knee of [`voice_guard_f64`] (`4.0`, `+12.04 dBFS`), allowing
/// voice `Out.gain_db` up to `+12 dB`, stereo spread, hard pan, resonant filter peaks,
/// and feedback loops to flow unclipped into the 32-bit float mixer bus.
pub const VOICE_GUARD_KNEE: f64 = 4.0;

/// Soft-knee compression headroom above [`VOICE_GUARD_KNEE`] (`4.0`).
pub const VOICE_GUARD_HEADROOM: f64 = 4.0;

/// Strict asymptotic anti-blowup ceiling of [`voice_guard_f64`] (`8.0`).
pub const VOICE_GUARD_CEILING: f64 = VOICE_GUARD_KNEE + VOICE_GUARD_HEADROOM;

/// Transparent `C¹`-continuous per-voice and feedback-loop anti-blowup safety guard (`f64`).
///
/// Passes inputs inside `[-VOICE_GUARD_KNEE, VOICE_GUARD_KNEE]` (`[-4.0, 4.0]`, `+12.04 dBFS`)
/// bit-identically untouched, then smoothly compresses extreme overshoots into
/// `(-VOICE_GUARD_CEILING, VOICE_GUARD_CEILING)` (`(-8.0, 8.0)`). Non-finite inputs return `0.0`.
///
/// Strictly for per-voice / feedback-loop anti-blowup; NEVER use behind a Master bus or Limiter!
#[inline]
pub fn voice_guard_f64(x: f64) -> f64 {
    soft_knee_guard_f64(x, VOICE_GUARD_KNEE, VOICE_GUARD_HEADROOM)
}

/// Pure transfer sample for UI visualizers matching the `Drive` insert DSP exactly
/// across all [`DriveMode`] variants (ignoring frequency-dependent tone filtering
/// and DC blocking).
#[inline]
pub fn drive_transfer_sample(x: f32, patch: &DrivePatch) -> f32 {
    let x = sanitize_sample(x);
    let shaped = drive_shape_mode_asym(x, patch.drive, patch.asymmetry, patch.mode);
    let out_gain =
        db_to_lin(sanitize_sample(patch.output_db).clamp(DRIVE_MIN_OUTPUT_DB, DRIVE_MAX_OUTPUT_DB));
    let mix = sanitize_sample(patch.mix).clamp(0.0, 1.0);
    x * (1.0 - mix) + shaped * out_gain * mix
}

/// Pure `1×`-rate cubic multi-mode saturation insert composing [`DriveShaperCoeffs`]
/// across `AtomicJet` and `ShockPort`, click-free `DriveMode` crossfading, [`DcBlocker`],
/// [`TptOnePoleLpf`], smoothed parameters, and click-free bypass crossfade. Zero heap allocations.
pub struct Drive {
    sample_rate: f64,
    mode: DriveMode,
    prev_mode: DriveMode,
    mode_xfade: Smoother,
    drive: Smoother,
    asymmetry: Smoother,
    output_gain: Smoother,
    mix: Smoother,
    tone_hz: LogSmoother,
    tone_coeff: f64,
    bypass: BypassFader,
    tone_lpf: TptOnePoleLpf,
    dc_blocker: DcBlocker,
    last_patch: DrivePatch,
    has_processed: bool,
    ctrl_count: u32,
    ctrl_mask: u32,
    cached_shaper_key: (u64, u64, DriveMode),
    cached_shaper: DriveShaperCoeffs,
    cached_tone_bits: u64,
}

impl Drive {
    pub fn new(sample_rate: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let default = DrivePatch::default();
        let init_drive = f64::from(sanitize_sample(default.drive).clamp(0.0, 1.0));
        let init_asym = f64::from(sanitize_sample(default.asymmetry).clamp(-1.0, 1.0));
        let init_tone = clamp_freq_hz(
            f64::from(default.tone_hz),
            f64::from(DRIVE_MIN_TONE_HZ),
            sr,
            f64::from(DRIVE_MAX_TONE_HZ),
        );
        let mut d = Self {
            sample_rate: sr,
            mode: default.mode,
            prev_mode: default.mode,
            mode_xfade: Smoother::new(1.0, 3.0, sr),
            drive: Smoother::new(init_drive, 4.0, sr),
            asymmetry: Smoother::new(init_asym, 4.0, sr),
            output_gain: Smoother::new(db_to_lin_f64(f64::from(default.output_db)), 4.0, sr),
            mix: Smoother::new(f64::from(default.mix), 4.0, sr),
            tone_hz: LogSmoother::new_clamped(
                init_tone,
                f64::from(DRIVE_MIN_TONE_HZ),
                sr * NYQUIST_SAFE_RATIO,
            ),
            tone_coeff: one_pole_coeff_ms(4.0, sr),
            bypass: BypassFader::new(sr),
            tone_lpf: TptOnePoleLpf::new(sr, init_tone),
            dc_blocker: DcBlocker::new(sr),
            last_patch: default,
            has_processed: false,
            ctrl_count: 0,
            ctrl_mask: control_stride_mask(sr, 6_000.0),
            cached_shaper_key: (init_drive.to_bits(), init_asym.to_bits(), default.mode),
            cached_shaper: DriveShaperCoeffs::new(init_drive, init_asym, default.mode),
            cached_tone_bits: init_tone.to_bits(),
        };
        d.set_parameters(default);
        d
    }

    #[cfg(test)]
    pub(crate) fn is_idle(&self) -> bool {
        self.bypass.is_idle()
    }

    #[cfg(test)]
    pub(crate) fn tone_hz_current(&self) -> f64 {
        self.tone_hz.current()
    }

    pub fn reset(&mut self) {
        self.tone_lpf.reset();
        self.dc_blocker.reset();
        self.prev_mode = self.mode;
        self.mode_xfade.snap(1.0);
        self.drive.snap(self.drive.target());
        self.asymmetry.snap(self.asymmetry.target());
        self.output_gain.snap(self.output_gain.target());
        self.mix.snap(self.mix.target());
        self.tone_hz.snap_to_target();
        let tone = self.tone_hz.current();
        self.tone_lpf.set_cutoff(self.sample_rate, tone);
        self.cached_tone_bits = tone.to_bits();
        let d = self.drive.current();
        let a = self.asymmetry.current();
        self.cached_shaper_key = (d.to_bits(), a.to_bits(), self.mode);
        self.cached_shaper = DriveShaperCoeffs::new(d, a, self.mode);
        self.bypass.reset();
        self.has_processed = false;
        self.ctrl_count = 0;
    }

    pub fn set_parameters(&mut self, patch: DrivePatch) {
        let def = DrivePatch::default();
        let drive = sanitize_clamp_f64(f64::from(patch.drive), 0.0, 1.0, f64::from(def.drive));
        let asym = sanitize_clamp_f64(
            f64::from(patch.asymmetry),
            -1.0,
            1.0,
            f64::from(def.asymmetry),
        );
        let out_db = sanitize_clamp_f64(
            f64::from(patch.output_db),
            f64::from(DRIVE_MIN_OUTPUT_DB),
            f64::from(DRIVE_MAX_OUTPUT_DB),
            f64::from(def.output_db),
        );
        let mix = sanitize_clamp_f64(f64::from(patch.mix), 0.0, 1.0, f64::from(def.mix));
        let tone = clamp_freq_hz(
            f64::from(patch.tone_hz),
            f64::from(DRIVE_MIN_TONE_HZ),
            self.sample_rate,
            f64::from(DRIVE_MAX_TONE_HZ),
        );
        let sanitized = DrivePatch {
            mode: patch.mode,
            drive: drive as f32,
            asymmetry: asym as f32,
            tone_hz: tone as f32,
            output_db: out_db as f32,
            mix: mix as f32,
        };
        if self.has_processed && sanitized == self.last_patch {
            return;
        }
        self.last_patch = sanitized;
        let out_gain = db_to_lin_f64(out_db);
        if !self.has_processed {
            self.mode = patch.mode;
            self.prev_mode = patch.mode;
            self.mode_xfade.snap(1.0);
            self.drive.snap(drive);
            self.asymmetry.snap(asym);
            self.output_gain.snap(out_gain);
            self.mix.snap(mix);
            self.tone_hz.snap(tone);
            self.tone_lpf.set_cutoff(self.sample_rate, tone);
            self.cached_tone_bits = tone.to_bits();
            self.cached_shaper_key = (drive.to_bits(), asym.to_bits(), patch.mode);
            self.cached_shaper = DriveShaperCoeffs::new(drive, asym, patch.mode);
        } else {
            if patch.mode != self.mode {
                self.prev_mode = self.mode;
                self.mode = patch.mode;
                self.mode_xfade.snap(0.0);
                self.mode_xfade.set_target(1.0);
            }
            self.drive.set_target(drive);
            self.asymmetry.set_target(asym);
            self.output_gain.set_target(out_gain);
            self.mix.set_target(mix);
            self.tone_hz.set_target(tone);
        }
    }

    #[inline]
    pub fn advance_silent_frames(&mut self, frames: usize, bypassed: bool) {
        if frames == 0 {
            return;
        }
        self.has_processed = true;
        self.bypass.advance_silent(bypassed);
        self.drive.advance_silent(frames);
        self.asymmetry.advance_silent(frames);
        self.output_gain.advance_silent(frames);
        self.mix.advance_silent(frames);
        self.mode_xfade.advance_silent(frames);
        if let Some(last_tick) = control_stride_last_tick(self.ctrl_count, self.ctrl_mask, frames) {
            self.tone_hz.advance_silent(self.tone_coeff, last_tick + 1);
            let tone = self.tone_hz.current();
            let tone_bits = tone.to_bits();
            if tone_bits != self.cached_tone_bits {
                self.cached_tone_bits = tone_bits;
                self.tone_lpf.set_cutoff(self.sample_rate, tone);
            }
            let rem = frames - 1 - last_tick;
            if rem > 0 {
                self.tone_hz.advance_silent(self.tone_coeff, rem);
            }
        } else {
            self.tone_hz.advance_silent(self.tone_coeff, frames);
        }
        self.ctrl_count = self.ctrl_count.wrapping_add(frames as u32) & self.ctrl_mask;
        let drv = self.drive.current();
        let asym = self.asymmetry.current();
        let key = (drv.to_bits(), asym.to_bits(), self.mode);
        if key != self.cached_shaper_key {
            self.cached_shaper_key = key;
            self.cached_shaper = DriveShaperCoeffs::new(drv, asym, self.mode);
        }
    }

    pub fn process(&mut self, input: [f32; 2], bypassed: bool) -> [f32; 2] {
        self.has_processed = true;
        let dry = sanitize_frame(input);
        let bypass_w = match self.bypass.step(bypassed) {
            BypassStep::Idle => {
                self.advance_silent_frames(1, true);
                return dry;
            }
            BypassStep::SettledBypass => {
                self.tone_lpf.reset();
                self.dc_blocker.reset();
                self.bypass.mark_idle();
                self.advance_silent_frames(1, true);
                return dry;
            }
            BypassStep::Active(w) => w,
        };

        let drv = self.drive.next();
        let asym = self.asymmetry.next();
        let out_g = self.output_gain.next();
        let mix = self.mix.next();
        let tone = self.tone_hz.step(self.tone_coeff);
        let m_xf = self.mode_xfade.next();
        if self.ctrl_count == 0 {
            let tone_bits = tone.to_bits();
            if tone_bits != self.cached_tone_bits {
                self.cached_tone_bits = tone_bits;
                self.tone_lpf.set_cutoff(self.sample_rate, tone);
            }
        }
        self.ctrl_count = self.ctrl_count.wrapping_add(1) & self.ctrl_mask;

        let dry64 = [f64::from(dry[0]), f64::from(dry[1])];
        let shaped = if drv <= 0.0 || m_xf >= 1.0 - LIN_FLOOR_F64 {
            let key = (drv.to_bits(), asym.to_bits(), self.mode);
            if key != self.cached_shaper_key {
                self.cached_shaper_key = key;
                self.cached_shaper = DriveShaperCoeffs::new(drv, asym, self.mode);
            }
            [
                self.cached_shaper.shape(dry64[0]),
                self.cached_shaper.shape(dry64[1]),
            ]
        } else {
            let inv = 1.0 - m_xf;
            let ca = DriveShaperCoeffs::new(drv, asym, self.prev_mode);
            let cb = DriveShaperCoeffs::new(drv, asym, self.mode);
            [
                ca.shape(dry64[0]) * inv + cb.shape(dry64[0]) * m_xf,
                ca.shape(dry64[1]) * inv + cb.shape(dry64[1]) * m_xf,
            ]
        };

        let dc_free = self.dc_blocker.process_stereo(shaped);
        let toned = self.tone_lpf.process_stereo(dc_free);
        let wet = [toned[0] * out_g, toned[1] * out_g];
        let active_out = mix_stereo_f64(dry64, wet, mix);
        let out = mix_stereo_f64(dry64, active_out, bypass_w);
        [out[0] as f32, out[1] as f32]
    }
}

impl crate::dsp::StereoEffect for Drive {
    #[inline]
    fn process(&mut self, input: [f32; 2], bypassed: bool) -> [f32; 2] {
        Drive::process(self, input, bypassed)
    }

    #[inline]
    fn process_block(&mut self, buf: &mut [[f32; 2]], bypassed: bool) {
        if buf.is_empty() {
            return;
        }
        self.has_processed = true;
        if self.bypass.check_idle_bypass(bypassed) {
            self.advance_silent_frames(buf.len(), true);
            for frame in buf.iter_mut() {
                *frame = sanitize_frame(*frame);
            }
            return;
        }
        for frame in buf.iter_mut() {
            *frame = Drive::process(self, *frame, bypassed);
        }
    }

    #[inline]
    fn reset(&mut self) {
        Drive::reset(self);
    }

    #[inline]
    fn is_quiet(&self, bypassed: bool) -> bool {
        self.bypass.is_settled_for(bypassed)
            && (bypassed || (self.tone_lpf.quiet() && self.dc_blocker.quiet()))
    }

    #[inline]
    fn advance_silent_frames(&mut self, frames: usize, bypassed: bool) {
        Drive::advance_silent_frames(self, frames, bypassed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::StereoEffect;
    use std::f64::consts::TAU;

    fn dft_mag(samples: &[f32], freq_hz: f64, sr: f64) -> f64 {
        let n = samples.len() as f64;
        let mut re = 0.0_f64;
        let mut im = 0.0_f64;
        for (i, &s) in samples.iter().enumerate() {
            let ph = TAU * freq_hz * i as f64 / sr;
            re += f64::from(s) * ph.cos();
            im -= f64::from(s) * ph.sin();
        }
        2.0 * re.hypot(im) / n
    }

    fn dft_mag_f64(samples: &[f64], freq_hz: f64, sr: f64) -> f64 {
        let n = samples.len() as f64;
        let mut re = 0.0_f64;
        let mut im = 0.0_f64;
        for (i, &s) in samples.iter().enumerate() {
            let ph = TAU * freq_hz * i as f64 / sr;
            re += s * ph.cos();
            im -= s * ph.sin();
        }
        2.0 * re.hypot(im) / n
    }

    #[test]
    fn waveshapers_are_exact_identity_at_zero_and_continuous_near_zero_for_all_modes() {
        for &x in &[-1.0_f32, -0.75, -0.5, -0.1, 0.0, 0.1, 0.5, 0.75, 1.0] {
            assert_eq!(soft_clip_drive(x, 0.0), x);
            for &asym in &[-1.0_f32, 0.0, 1.0] {
                assert_eq!(asymmetric_drive_shape(x, 0.0, asym), x);
            }

            let sc_tiny = soft_clip_drive(x, 0.002);
            assert!(
                (sc_tiny - x).abs() < 0.01,
                "soft_clip_drive pop at drive=0.002 for x={x}: {sc_tiny} vs {x}"
            );

            for mode in DriveMode::ALL {
                assert_eq!(
                    drive_shape_mode(x, 0.0, mode),
                    x,
                    "{mode:?} must satisfy exact f(x, 0) == x"
                );
                let tiny = drive_shape_mode(x, 0.002, mode);
                assert!(
                    (tiny - x).abs() < 0.01,
                    "{mode:?} pop at drive=0.002 for x={x}: {tiny} vs {x}"
                );
            }
        }

        // Exact zero crossing at x == 0.0 and finite smooth output at extreme inputs across all modes.
        for i in 0..=20 {
            let d = f64::from(i) / 20.0;
            assert_eq!(soft_clip_drive_f64(0.0, d), 0.0);
            for mode in DriveMode::ALL {
                for &asym in &[-1.0_f64, -0.5, 0.0, 0.5, 1.0] {
                    assert_eq!(
                        drive_shape_mode_asym_f64(0.0, d, asym, mode),
                        0.0,
                        "{mode:?} must cross zero exactly at d={d} asym={asym}"
                    );
                    let hi = drive_shape_mode_asym_f64(100.0, d, asym, mode);
                    let lo = drive_shape_mode_asym_f64(-100.0, d, asym, mode);
                    assert!(
                        hi.is_finite() && lo.is_finite() && hi > 0.0 && lo < 0.0,
                        "{mode:?} extreme input must stay finite and sign-preserving: d={d} asym={asym} lo={lo} hi={hi}"
                    );
                }
            }
        }

        // Both modes are strictly odd-symmetric at asymmetry == 0.0 and asymmetric for asymmetry != 0.0.
        for mode in DriveMode::ALL {
            for &d in &[0.2, 0.5, 0.85, 1.0] {
                for &x in &[0.05, 0.25, 0.7, 1.0, 100.0] {
                    let p = drive_shape_mode_f64(x, d, mode);
                    let n = drive_shape_mode_f64(-x, d, mode);
                    assert!(
                        (p + n).abs() < 1e-12,
                        "{mode:?} must be strictly odd-symmetric at asym=0: d={d} x={x} p={p} n={n}"
                    );
                }
            }
            let pos = drive_shape_mode_asym_f64(0.7, 0.85, 0.65, mode);
            let neg = drive_shape_mode_asym_f64(-0.7, 0.85, 0.65, mode);
            assert!(
                (pos + neg).abs() > 0.02,
                "{mode:?} with asymmetry must produce asymmetric half-cycles: pos={pos}, neg={neg}"
            );
        }

        // asymmetric_drive_shape delegates to DriveMode::AtomicJet.
        for &x in &[-0.8_f64, -0.2, 0.0, 0.35, 0.9] {
            assert_eq!(
                asymmetric_drive_shape_f64(x, 0.7, 0.5),
                drive_shape_mode_asym_f64(x, 0.7, 0.5, DriveMode::AtomicJet)
            );
        }
    }

    #[test]
    fn atomic_jet_preserves_zero_slope_across_asymmetry_while_splitting_ceilings() {
        let d = 0.8_f64;
        let k = d * (1.5 + 10.5 * d);
        let expected_slope = (1.0 + k * k).sqrt() * (1.0 + k).powf(-0.18);
        let eps = 1e-7;
        for &asym in &[-0.9, -0.4, 0.0, 0.4, 0.9] {
            let coeffs = DriveShaperCoeffs::new(d, asym, DriveMode::AtomicJet);
            let slope = (coeffs.shape(eps) - coeffs.shape(-eps)) / (2.0 * eps);
            assert!(
                (slope - expected_slope).abs() < 1e-7,
                "asym={asym}: expected slope {expected_slope}, got {slope}"
            );
        }
        let asym_pos = DriveShaperCoeffs::new(d, 0.8, DriveMode::AtomicJet);
        let ceil_pos = asym_pos.shape(100.0);
        let ceil_neg = asym_pos.shape(-100.0);
        assert!(ceil_pos.is_finite() && ceil_neg.is_finite());
        assert!(
            ceil_pos > 0.0 && ceil_neg < 0.0 && (ceil_pos + ceil_neg).abs() > 0.5,
            "AtomicJet ceilings should split cleanly under asymmetry: pos={ceil_pos}, neg={ceil_neg}"
        );
    }

    #[test]
    fn shock_port_linear_interior_has_zero_thd_and_outer_riccati_clamps_smoothly() {
        for &d in &[0.25_f64, 0.6, 1.0] {
            let p_base = 1.0 / (1.0 + 5.5 * d * d);
            let makeup = (1.0 / p_base).powf(0.32);
            for &asym in &[-0.7_f64, 0.0, 0.7] {
                let asym_eff = 0.55 * asym * (1.0 - (-6.0 * d).exp());
                let p_pos = p_base * (1.0 - asym_eff);
                let p_neg = p_base * (1.0 + asym_eff);
                let headroom = p_pos.min(p_neg);
                let coeffs = DriveShaperCoeffs::new(d, asym, DriveMode::ShockPort);

                // Exact linear interior across [-headroom, +headroom] with identical +/- slope.
                for step in -10..=10 {
                    let x = headroom * (f64::from(step) / 10.0);
                    let got = coeffs.shape(x);
                    let expected = x * makeup;
                    assert!(
                        (got - expected).abs() < 1e-13,
                        "ShockPort interior nonlinearity d={d} asym={asym} x={x}: got={got} expected={expected}"
                    );
                }

                // Pure sine strictly inside headroom has 0.000% THD (H2, H3, H5 < 1e-12 * H1).
                let sr = 48_000.0;
                let f0 = 300.0;
                let n = 4_800;
                let amp = headroom * 0.95;
                let buf: Vec<f64> = (0..n)
                    .map(|i| coeffs.shape((TAU * f0 * i as f64 / sr).sin() * amp))
                    .collect();
                let h1 = dft_mag_f64(&buf, f0, sr);
                let h2 = dft_mag_f64(&buf, f0 * 2.0, sr);
                let h3 = dft_mag_f64(&buf, f0 * 3.0, sr);
                let h5 = dft_mag_f64(&buf, f0 * 5.0, sr);
                assert!(
                    (h1 - amp * makeup).abs() < 1e-12,
                    "d={d} asym={asym}: interior fundamental gain drifted"
                );
                assert!(
                    h2 < h1 * 1e-12 && h3 < h1 * 1e-12 && h5 < h1 * 1e-12,
                    "d={d} asym={asym}: interior sine must have 0.000% THD: h1={h1} h2={h2} h3={h3} h5={h5}"
                );

                // Above headroom, Riccati clamping stays smooth, monotone, sub-linear, and finite.
                let mut prev = coeffs.shape(p_pos);
                for &x in &[
                    p_pos * 1.2,
                    p_pos * 2.0,
                    p_pos * 4.0,
                    p_pos * 10.0,
                    p_pos * 40.0,
                    100.0,
                ] {
                    if x <= p_pos {
                        continue;
                    }
                    let y = coeffs.shape(x);
                    assert!(
                        y.is_finite() && y >= prev && y < x * makeup,
                        "d={d} asym={asym} x={x}: expected smooth sub-linear Riccati clamp, prev={prev} y={y} lin={}",
                        x * makeup
                    );
                    prev = y;
                }
            }
        }
    }

    #[test]
    fn voice_guard_is_transparent_to_knee_c1_continuous_and_bounded_inside_ceiling() {
        assert_eq!(VOICE_GUARD_KNEE, 4.0);
        assert_eq!(VOICE_GUARD_HEADROOM, 4.0);
        assert_eq!(VOICE_GUARD_CEILING, 8.0);

        // Exact bit-identical pass-through across [-4.0, 4.0].
        for &x in &[
            -VOICE_GUARD_KNEE,
            -3.99,
            -2.5,
            -1.0,
            -0.5,
            0.0,
            0.5,
            1.0,
            2.5,
            3.99,
            VOICE_GUARD_KNEE,
        ] {
            assert_eq!(voice_guard_f64(x), x);
        }

        // C^1 unit slope across both +VOICE_GUARD_KNEE (+4.0) and -VOICE_GUARD_KNEE (-4.0).
        let eps = 1e-6;
        for &knee_pt in &[-VOICE_GUARD_KNEE, VOICE_GUARD_KNEE] {
            let slope_left = (voice_guard_f64(knee_pt) - voice_guard_f64(knee_pt - eps)) / eps;
            let slope_right = (voice_guard_f64(knee_pt + eps) - voice_guard_f64(knee_pt)) / eps;
            assert!(
                (slope_left - 1.0).abs() < 1e-6,
                "knee={knee_pt}: left slope={slope_left}"
            );
            assert!(
                (slope_right - 1.0).abs() < 1e-6,
                "knee={knee_pt}: right slope={slope_right}"
            );
        }

        // Smooth odd-symmetric compression strictly inside (-8.0, 8.0).
        for &x in &[-50.0_f64, -16.0, -8.0, -4.5, 4.5, 8.0, 16.0, 50.0] {
            let y = voice_guard_f64(x);
            assert!(
                y.is_finite() && y.abs() > VOICE_GUARD_KNEE && y.abs() < VOICE_GUARD_CEILING,
                "x={x} -> y={y}"
            );
            assert_eq!(y.signum(), x.signum());
            assert_eq!(voice_guard_f64(-x), -y);
        }
        for &extreme in &[-1e6_f64, 1e6] {
            let y = voice_guard_f64(extreme);
            assert!(y.is_finite() && y.abs() <= VOICE_GUARD_CEILING);
            assert_eq!(y.signum(), extreme.signum());
        }

        // Non-finite inputs sanitize to 0.0.
        assert_eq!(voice_guard_f64(f64::NAN), 0.0);
        assert_eq!(voice_guard_f64(f64::INFINITY), 0.0);
        assert_eq!(voice_guard_f64(f64::NEG_INFINITY), 0.0);
    }

    #[test]
    fn drive_processor_saturates_cleanly_with_minimal_dc_across_all_modes_and_sample_rates() {
        for mode in DriveMode::ALL {
            for sr in [44_100.0, 48_000.0, 96_000.0] {
                let mut drv = Drive::new(sr);
                drv.set_parameters(DrivePatch {
                    mode,
                    drive: 0.85,
                    asymmetry: 0.0,
                    tone_hz: 6000.0,
                    output_db: 0.0,
                    mix: 1.0,
                });
                drv.reset();
                let warmup = (sr * 0.25) as usize;
                let measure = (sr * 0.25) as usize;
                let mut dc_sum = 0.0_f64;
                let mut max_peak = 0.0_f32;
                for i in 0..(warmup + measure) {
                    let x = (TAU * 120.0 * i as f64 / sr).sin() as f32 * 0.8;
                    let y = drv.process([x, x], false);
                    assert!(y[0].is_finite() && y[1].is_finite());
                    max_peak = max_peak.max(y[0].abs());
                    if i >= warmup {
                        dc_sum += f64::from(y[0]);
                    }
                }
                let dc = (dc_sum / measure as f64).abs();
                assert!(max_peak < 1.2, "mode={mode:?} sr={sr}: max_peak={max_peak}");
                assert!(dc < 2e-3, "mode={mode:?} sr={sr}: dc={dc}");
            }
        }
    }

    #[test]
    fn drive_mode_transitions_are_click_free_and_bounded() {
        let sr = 48_000.0;
        let modes = [
            DriveMode::AtomicJet,
            DriveMode::ShockPort,
            DriveMode::AtomicJet,
        ];
        for pair in modes.windows(2) {
            let from = pair[0];
            let to = pair[1];
            let mut switched = Drive::new(sr);
            let mut reference = Drive::new(sr);
            let p_from = DrivePatch {
                mode: from,
                drive: 0.8,
                asymmetry: 0.25,
                tone_hz: 10_000.0,
                output_db: 0.0,
                mix: 1.0,
            };
            let p_to = DrivePatch { mode: to, ..p_from };
            switched.set_parameters(p_from);
            reference.set_parameters(p_from);

            let switch_at = 2_400usize;
            for i in 0..4_800 {
                if i == switch_at {
                    switched.set_parameters(p_to);
                }
                let x = (TAU * 180.0 * i as f64 / sr).sin() as f32 * 0.6;
                let y = switched.process([x, x], false);
                let y_ref = reference.process([x, x], false);
                assert!(y[0].is_finite() && y[1].is_finite());
                assert!(y[0].abs() < 1.5 && y[1].abs() < 1.5);
                if i == switch_at {
                    let delta = (y[0] - y_ref[0]).abs();
                    assert!(
                        delta < 0.02,
                        "transition {from:?}->{to:?} clicked on switch sample: delta={delta}"
                    );
                }
            }
        }
    }

    #[test]
    fn drive_asymmetry_preserves_zero_drive_and_zero_dc_and_introduces_even_harmonics() {
        for mode in DriveMode::ALL {
            for &asym in &[-1.0_f64, -0.5, 0.0, 0.5, 1.0] {
                for &x in &[-0.9, -0.3, 0.0, 0.3, 0.9] {
                    assert_eq!(drive_shape_mode_asym_f64(x, 0.0, asym, mode), x);
                }
                for &d in &[0.0, 0.25, 0.65, 1.0] {
                    assert_eq!(drive_shape_mode_asym_f64(0.0, d, asym, mode), 0.0);
                }
            }
            for &x in &[-0.8, -0.2, 0.2, 0.8] {
                for &d in &[0.15, 0.5, 0.9] {
                    assert_eq!(
                        drive_shape_mode_asym_f64(x, d, 0.0, mode),
                        drive_shape_mode_f64(x, d, mode)
                    );
                }
            }
        }

        let sr = 48_000.0;
        let f0 = 200.0;
        for mode in DriveMode::ALL {
            // Symmetric setting generates negligible 2nd harmonic.
            let mut sym = Drive::new(sr);
            sym.set_parameters(DrivePatch {
                mode,
                drive: 0.7,
                asymmetry: 0.0,
                tone_hz: 16_000.0,
                output_db: 0.0,
                mix: 1.0,
            });
            sym.reset();
            let warmup = 4_800;
            let measure = 4_800;
            let mut sym_buf = Vec::with_capacity(measure);
            for i in 0..(warmup + measure) {
                let x = (TAU * f0 * i as f64 / sr).sin() as f32 * 0.65;
                let y = sym.process([x, x], false);
                if i >= warmup {
                    sym_buf.push(y[0]);
                }
            }
            let sym_h1 = dft_mag(&sym_buf, f0, sr);
            let sym_h2 = dft_mag(&sym_buf, f0 * 2.0, sr);
            assert!(
                sym_h2 < sym_h1 * 1e-3,
                "mode={mode:?} asymmetry=0.0 must suppress 2nd harmonic: h1={sym_h1:.4} h2={sym_h2:.6}"
            );

            // Non-zero asymmetry generates clear 2nd harmonic while staying DC-blocked and bounded.
            let mut drv = Drive::new(sr);
            drv.set_parameters(DrivePatch {
                mode,
                drive: 0.7,
                asymmetry: 0.65,
                tone_hz: 16_000.0,
                output_db: 0.0,
                mix: 1.0,
            });
            drv.reset();
            let mut buf = Vec::with_capacity(measure);
            let mut dc_sum = 0.0_f64;
            for i in 0..(warmup + measure) {
                let x = (TAU * f0 * i as f64 / sr).sin() as f32 * 0.65;
                let y = drv.process([x, x], false);
                assert!(y[0].is_finite() && y[0].abs() < 1.5);
                if i >= warmup {
                    buf.push(y[0]);
                    dc_sum += f64::from(y[0]);
                }
            }
            let h1 = dft_mag(&buf, f0, sr);
            let h2 = dft_mag(&buf, f0 * 2.0, sr);
            let dc = (dc_sum / measure as f64).abs();
            assert!(
                h2 > h1 * 0.04,
                "mode={mode:?} asymmetry=+0.65 must produce clear 2nd harmonic: h1={h1:.4} h2={h2:.4}"
            );
            assert!(
                dc < 2e-3,
                "mode={mode:?} asymmetry=+0.65 must stay DC-blocked: dc={dc}"
            );
        }
    }

    #[test]
    fn soft_knee_guard_is_c1_continuous_transparent_below_knee_and_bounded() {
        for &x in &[-4.0_f64, -2.5, 0.0, 2.5, 4.0] {
            assert_eq!(soft_knee_guard_f64(x, 4.0, 4.0), x);
        }
        for &x in &[-50.0_f64, -8.0, -4.5, 4.5, 8.0, 50.0] {
            let y = soft_knee_guard_f64(x, 4.0, 4.0);
            assert!(
                y.is_finite() && y.abs() > 4.0 && y.abs() <= 8.0,
                "x={x} -> y={y}"
            );
            assert_eq!(y.signum(), x.signum());
            assert!(
                (soft_knee_guard_f64(-x, 4.0, 4.0) + y).abs() < 1e-12,
                "must be odd-symmetric"
            );
        }
        let eps = 1e-6;
        let slope_left =
            (soft_knee_guard_f64(4.0, 4.0, 4.0) - soft_knee_guard_f64(4.0 - eps, 4.0, 4.0)) / eps;
        let slope_right =
            (soft_knee_guard_f64(4.0 + eps, 4.0, 4.0) - soft_knee_guard_f64(4.0, 4.0, 4.0)) / eps;
        assert!((slope_left - 1.0).abs() < 1e-6);
        assert!((slope_right - 1.0).abs() < 1e-6);

        for &x in &[-50.0_f64, -8.0, -4.0, -1.0, 0.0, 1.0, 4.0, 8.0, 50.0] {
            assert_eq!(
                voice_guard_f64(x),
                soft_knee_guard_f64(x, VOICE_GUARD_KNEE, VOICE_GUARD_HEADROOM)
            );
        }

        assert_eq!(soft_knee_guard_f64(f64::NAN, 4.0, 4.0), 0.0);
        assert_eq!(soft_knee_guard_f64(f64::INFINITY, 4.0, 4.0), 0.0);
        assert_eq!(soft_knee_guard_f64(10.0, 2.0, 0.0), 2.0);
        assert_eq!(soft_knee_guard_f64(-10.0, 2.0, -1.0), -2.0);
    }

    #[test]
    fn precomputed_shaper_coeffs_match_closed_form_reference_across_all_modes() {
        let ref_soft_clip = |x: f64, drive: f64| -> f64 {
            let x = sanitize_sample_f64(x);
            let d = sanitize_sample_f64(drive).clamp(0.0, 1.0);
            if d <= 0.0 {
                return x;
            }
            let k = d * 5.0;
            let comp = (1.0 + k).powf(-0.22);
            if k < 1e-4 {
                x * comp
            } else {
                (k * x).tanh() / k.tanh() * comp
            }
        };
        let ref_drive = |x: f64, drive: f64, asym: f64, mode: DriveMode| -> f64 {
            let x = sanitize_sample_f64(x);
            let d = sanitize_sample_f64(drive).clamp(0.0, 1.0);
            if d <= 0.0 {
                return x;
            }
            let a = sanitize_sample_f64(asym).clamp(-1.0, 1.0);
            match mode {
                DriveMode::AtomicJet => {
                    let k = d * (1.5 + 10.5 * d);
                    let out_gain = (1.0 + k * k).sqrt() * (1.0 + k).powf(-0.18);
                    if k <= 1e-9 {
                        x * out_gain
                    } else {
                        let scale = 1.0 / k;
                        let asym_eff = (0.90 * a * (1.0 - (-6.0 * d).exp())).clamp(-0.95, 0.95);
                        atomic_jet(x, scale, asym_eff) * out_gain
                    }
                }
                DriveMode::ShockPort => {
                    let p_base = 1.0 / (1.0 + 5.5 * d * d);
                    let eta = d * (0.8 + 4.2 * d);
                    let asym_eff = 0.55 * a * (1.0 - (-6.0 * d).exp());
                    let p_pos = p_base * (1.0 - asym_eff);
                    let p_neg = p_base * (1.0 + asym_eff);
                    let comp_pos = (2.0 * eta * p_pos).exp();
                    let comp_neg = (2.0 * eta * p_neg).exp();
                    let makeup = (1.0 / p_base).powf(0.32);
                    if x >= 0.0 {
                        atomic_pair_scalar(x, 0.0, p_pos, eta) * comp_pos * makeup
                    } else {
                        atomic_pair_scalar(x, 0.0, p_neg, eta) * comp_neg * makeup
                    }
                }
            }
        };

        let xs = [
            -100.0_f64, -2.5, -1.0, -0.45, -0.02, 0.0, 0.02, 0.45, 1.0, 2.5, 100.0,
        ];
        let drives = [0.0_f64, 1e-10, 1e-5, 0.15, 0.5, 0.85, 1.0];
        let asyms = [-1.0_f64, -0.4, 0.0, 0.4, 1.0];
        for &d in &drives {
            let sc = SoftClipCoeffs::new(d);
            for &x in &xs {
                let got = sc.shape(x);
                let expected = ref_soft_clip(x, d);
                assert!(
                    (got - expected).abs() <= 1e-13,
                    "SoftClipCoeffs mismatch d={d} x={x}: got={got} expected={expected}"
                );
            }
            for mode in DriveMode::ALL {
                for &a in &asyms {
                    let dc = DriveShaperCoeffs::new(d, a, mode);
                    for &x in &xs {
                        let got = dc.shape(x);
                        let expected = ref_drive(x, d, a, mode);
                        assert!(
                            (got - expected).abs() <= 1e-12,
                            "DriveShaperCoeffs mismatch mode={mode:?} d={d} a={a} x={x}: got={got} expected={expected}"
                        );
                    }
                }
            }
        }
        assert_eq!(SoftClipCoeffs::new(f64::NAN).shape(0.5), 0.5);
        assert_eq!(
            DriveShaperCoeffs::new(f64::NAN, 0.5, DriveMode::AtomicJet).shape(0.5),
            0.5
        );
    }

    #[test]
    fn parallel_drive_is_phase_aligned_with_flat_response_across_sample_rates() {
        for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            for mode in DriveMode::ALL {
                for &freq in &[200.0_f64, 1_000.0, 4_000.0, 8_000.0, 12_000.0, 16_000.0] {
                    if freq > sr * 0.40 {
                        continue;
                    }
                    let mut drv = Drive::new(sr);
                    drv.set_parameters(DrivePatch {
                        mode,
                        drive: 0.0,
                        asymmetry: 0.0,
                        tone_hz: DRIVE_MAX_TONE_HZ,
                        output_db: 0.0,
                        mix: 0.5,
                    });
                    let warmup = (sr * 0.05) as usize;
                    let measure = (sr * 0.05) as usize;
                    let mut in_sq = 0.0_f64;
                    let mut out_sq = 0.0_f64;
                    for i in 0..(warmup + measure) {
                        let x = (TAU * freq * i as f64 / sr).sin() as f32 * 0.5;
                        let y = drv.process([x, x], false);
                        if i >= warmup {
                            in_sq += f64::from(x) * f64::from(x);
                            out_sq += f64::from(y[0]) * f64::from(y[0]);
                        }
                    }
                    let gain_db = crate::dsp::lin_to_db_f64((out_sq / in_sq).sqrt());
                    assert!(
                        gain_db > -2.0 && gain_db < 0.2,
                        "mode={mode:?} sr={sr} freq={freq:.0} Hz: parallel Drive(drive=0, mix=0.5) drifted to {gain_db:+.2} dB"
                    );
                }
            }
        }
    }

    #[test]
    fn drive_tone_smooths_logarithmically_and_bypass_idles_across_sample_rates() {
        for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let mut drv = Drive::new(sr);
            assert!(drv.is_idle());
            let start_hz = 800.0_f32;
            let target_hz = 12_800.0_f32;
            let base = DrivePatch {
                mode: DriveMode::AtomicJet,
                drive: 0.6,
                asymmetry: 0.2,
                tone_hz: start_hz,
                output_db: 0.0,
                mix: 0.75,
            };
            drv.set_parameters(base);
            let _ = drv.process([0.3, -0.3], false);
            assert!(!drv.is_idle());
            assert!((drv.tone_hz_current() - f64::from(start_hz)).abs() < 1e-6);

            let swept = DrivePatch {
                tone_hz: target_hz,
                ..base
            };
            drv.set_parameters(swept);

            // At t = tau * ln(2) (tau = 4 ms), LogSmoother sits at geometric mean sqrt(800 * 12800) = 3200 Hz.
            let half_frames = (0.004 * 2.0_f64.ln() * sr).round() as usize;
            let mut prev = drv.tone_hz_current();
            for _ in 0..half_frames {
                let y = drv.process([0.2, -0.2], false);
                assert!(y[0].is_finite() && y[1].is_finite());
                let cur = drv.tone_hz_current();
                assert!(cur >= prev - 1e-9 && cur <= f64::from(target_hz) + 1e-6);
                prev = cur;
            }
            let geo_mean = (f64::from(start_hz) * f64::from(target_hz)).sqrt();
            assert!(
                (drv.tone_hz_current() - geo_mean).abs() / geo_mean < 0.05,
                "sr={sr}: expected log midpoint ~{geo_mean:.1} Hz, got {:.1} Hz",
                drv.tone_hz_current()
            );

            for _ in 0..((sr * 0.15) as usize) {
                let _ = drv.process([0.2, -0.2], false);
            }
            assert!(
                (drv.tone_hz_current() - f64::from(target_hz)).abs() < 1e-2,
                "sr={sr}: tone_hz did not settle to {target_hz}, got {}",
                drv.tone_hz_current()
            );

            // Bypass fades cleanly to idle and clears filter state.
            for _ in 0..((sr * 0.03) as usize) {
                let y = drv.process([0.4, -0.4], true);
                assert!(y[0].is_finite() && y[1].is_finite());
            }
            assert!(drv.is_idle(), "sr={sr}: bypassed Drive must idle");
            assert_eq!(drv.process([0.4, -0.4], true), [0.4, -0.4]);

            // Repeated NaN patches sanitize deterministically without poisoning state.
            let nan_patch = DrivePatch {
                mode: DriveMode::AtomicJet,
                drive: f32::NAN,
                asymmetry: f32::NAN,
                tone_hz: f32::NAN,
                output_db: f32::NAN,
                mix: f32::NAN,
            };
            drv.set_parameters(nan_patch);
            drv.set_parameters(nan_patch);
            let y = drv.process([0.5, -0.5], false);
            assert!(y[0].is_finite() && y[1].is_finite());
        }
    }

    #[test]
    fn drive_is_quiet_advance_silent_frames_and_process_block_parity() {
        let sr = 48_000.0;
        let p0 = DrivePatch {
            mode: DriveMode::AtomicJet,
            drive: 0.35,
            asymmetry: -0.2,
            tone_hz: 2_400.0,
            output_db: -1.5,
            mix: 0.8,
        };
        let p1 = DrivePatch {
            mode: DriveMode::ShockPort,
            drive: 0.85,
            asymmetry: 0.55,
            tone_hz: 11_000.0,
            output_db: 2.0,
            mix: 1.0,
        };

        // 1. Fresh active Drive settles quiet on silence and matches advance_silent_frames exactly.
        let mut frame_drv = Drive::new(sr);
        let mut fast_drv = Drive::new(sr);
        frame_drv.set_parameters(p0);
        fast_drv.set_parameters(p0);
        // Settle initial bypass crossfade with silence so both are active and quiet.
        for _ in 0..1_024 {
            assert_eq!(frame_drv.process([0.0, 0.0], false), [0.0, 0.0]);
            assert_eq!(fast_drv.process([0.0, 0.0], false), [0.0, 0.0]);
        }
        assert!(StereoEffect::is_quiet(&frame_drv, false));
        assert!(StereoEffect::is_quiet(&fast_drv, false));

        // Trigger a live parameter + mode transition while silent, then advance 197 frames.
        frame_drv.set_parameters(p1);
        fast_drv.set_parameters(p1);
        let silent_frames = 197;
        for _ in 0..silent_frames {
            assert_eq!(frame_drv.process([0.0, 0.0], false), [0.0, 0.0]);
        }
        StereoEffect::advance_silent_frames(&mut fast_drv, silent_frames, false);

        for i in 0..256 {
            let x0 = (TAU * 260.0 * i as f64 / sr).sin() as f32 * 0.7;
            let x1 = (TAU * 390.0 * i as f64 / sr).cos() as f32 * 0.6;
            let a = frame_drv.process([x0, x1], false);
            let b = fast_drv.process([x0, x1], false);
            assert!(
                (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6,
                "frame {i}: advance_silent_frames drifted: frame={a:?} fast={b:?}"
            );
        }
        assert!(!StereoEffect::is_quiet(&frame_drv, false));

        // 2. process_block matches frame-by-frame process bit-identically (active & bypassed).
        let mut single = Drive::new(sr);
        let mut block = Drive::new(sr);
        single.set_parameters(p1);
        block.set_parameters(p1);
        let mut blk_buf: Vec<[f32; 2]> = (0..128)
            .map(|i| {
                [
                    (TAU * 220.0 * i as f64 / sr).sin() as f32 * 0.75,
                    (TAU * 330.0 * i as f64 / sr).cos() as f32 * 0.65,
                ]
            })
            .collect();
        let ref_buf: Vec<[f32; 2]> = blk_buf.iter().map(|&f| single.process(f, false)).collect();
        StereoEffect::process_block(&mut block, &mut blk_buf, false);
        assert_eq!(blk_buf, ref_buf);

        // Reset clears filter state and restores quietness for settled bypass.
        StereoEffect::reset(&mut block);
        assert!(StereoEffect::is_quiet(&block, true));
    }
}
