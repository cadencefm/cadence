//! Continuous zero-pop saturation waveshapers, voice safety guard, and Cadence's
//! multi-mode `Drive` insert processor (`Tape` / `HardClip` / `Fold`, with zero-alloc
//! 2× oversampling across all three modes).
//!
//! All waveshapers satisfy exact identity `f(x, 0.0) == x` with smooth continuity
//! across `drive ∈ [0.0, 1.0]` for click-free drive automation from zero.

use crate::dsp::biquad::{DcBlocker, TptOnePoleLpf};
use crate::dsp::math::{
    db_to_lin, db_to_lin_f64, flush_denormal_f64, sanitize_frame, sanitize_sample,
    sanitize_sample_f64, sanitize_sample_rate, Smoother,
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

/// Precomputed parameter-only waveshaper coefficients shared by [`drive_shape_mode_asym_f64`],
/// [`shape_drive_oversampled_2x_coeffs`], [`Drive`], and `cadence-engine`'s `DriveNode`.
///
/// Computing `exp`, `powf`, `tanh(k)`, `tanh(bias)`, and `sin(bias)` once per parameter update
/// eliminates 4× redundant transcendental work per oversampled stereo frame while keeping the
/// waveshaper transfer function strictly identical.
#[derive(Clone, Copy, Debug)]
pub struct DriveShaperCoeffs {
    mode: DriveMode,
    bypass: bool,
    linear_only: bool,
    has_asym: bool,
    k: f64,
    bias: f64,
    bias_offset: f64,
    scale: f64,
    w: f64,
    quad: f64,
    ceil_pos: f64,
    ceil_neg: f64,
}

impl DriveShaperCoeffs {
    pub const BYPASS: Self = Self {
        mode: DriveMode::Tape,
        bypass: true,
        linear_only: false,
        has_asym: false,
        k: 0.0,
        bias: 0.0,
        bias_offset: 0.0,
        scale: 1.0,
        w: 0.0,
        quad: 0.0,
        ceil_pos: 1.0,
        ceil_neg: 1.0,
    };

    #[inline]
    pub fn new(drive: f64, asymmetry: f64, mode: DriveMode) -> Self {
        let d = sanitize_sample_f64(drive).clamp(0.0, 1.0);
        if d <= 0.0 {
            return Self::BYPASS;
        }
        let a = sanitize_sample_f64(asymmetry).clamp(-1.0, 1.0);
        let d2 = d * d;
        let has_asym = a != 0.0;
        let env_a = if has_asym {
            1.0 - (-5.0 * d).exp()
        } else {
            0.0
        };
        match mode {
            DriveMode::Tape => {
                let k = d * 1.5 + d2 * 12.5;
                let bias = 0.08 * d2 + 0.70 * a * env_a;
                let comp = (1.0 + k).powf(-0.18);
                if k < 1e-4 {
                    Self {
                        mode,
                        bypass: false,
                        linear_only: true,
                        has_asym,
                        k,
                        bias,
                        bias_offset: 0.0,
                        scale: comp,
                        w: 0.0,
                        quad: 0.0,
                        ceil_pos: 1.0,
                        ceil_neg: 1.0,
                    }
                } else {
                    Self {
                        mode,
                        bypass: false,
                        linear_only: false,
                        has_asym,
                        k,
                        bias,
                        bias_offset: bias.tanh(),
                        scale: comp / k.tanh(),
                        w: 0.0,
                        quad: 0.0,
                        ceil_pos: 1.0,
                        ceil_neg: 1.0,
                    }
                }
            }
            DriveMode::HardClip => {
                let k = 1.0 + d * 2.5 + d2 * 16.0 + d2 * d * 45.0;
                let w = 1.0 - (-30.0 * d2).exp();
                let comp = k.powf(-0.15);
                Self {
                    mode,
                    bypass: false,
                    linear_only: false,
                    has_asym,
                    k,
                    bias: 0.0,
                    bias_offset: 0.0,
                    scale: comp * w,
                    w,
                    quad: 0.24 * a * env_a,
                    ceil_pos: 1.0 - 0.30 * a * env_a,
                    ceil_neg: 1.0 + 0.30 * a * env_a,
                }
            }
            DriveMode::Fold => {
                let k = 1.0 + d * 2.5 + d2 * 14.0;
                let w = 1.0 - (-30.0 * d2).exp();
                let env_5d = if has_asym {
                    env_a
                } else {
                    1.0 - (-5.0 * d).exp()
                };
                let fold_scale = 1.0 / (1.0 + 0.5 * env_5d);
                let bias = 0.55 * a * env_a;
                Self {
                    mode,
                    bypass: false,
                    linear_only: false,
                    has_asym,
                    k,
                    bias,
                    bias_offset: bias.sin(),
                    scale: fold_scale * w,
                    w,
                    quad: 0.0,
                    ceil_pos: 1.0,
                    ceil_neg: 1.0,
                }
            }
        }
    }

    #[inline]
    pub const fn is_bypass(self) -> bool {
        self.bypass
    }

    #[inline]
    pub fn shape(self, x: f64) -> f64 {
        let x = sanitize_sample_f64(x);
        if self.bypass {
            return x;
        }
        match self.mode {
            DriveMode::Tape => {
                if self.linear_only {
                    return x * self.scale;
                }
                ((self.k * x + self.bias).tanh() - self.bias_offset) * self.scale
            }
            DriveMode::HardClip => {
                let u = self.k * x;
                let u_asym = if !self.has_asym {
                    u
                } else {
                    u + self.quad * (u * u) / (1.0 + 0.5 * u.abs())
                };
                let ceil = if !self.has_asym {
                    1.0
                } else if u_asym >= 0.0 {
                    self.ceil_pos
                } else {
                    self.ceil_neg
                };
                let knee = 0.96 * ceil;
                let span = (ceil - knee).max(1e-6);
                let clipped = soft_knee_guard_f64(u_asym, knee, span);
                x * (1.0 - self.w) + clipped * self.scale
            }
            DriveMode::Fold => {
                let folded = (self.k * x + self.bias).sin() - self.bias_offset;
                x * (1.0 - self.w) + folded * self.scale
            }
        }
    }
}

/// Single-point-of-truth waveshaper in `f64` for all [`DriveMode`] circuits with
/// continuous bipolar `asymmetry ∈ [-1.0, 1.0]`, shared by both real-time
/// [`Drive::process`] and UI transfer plots ([`drive_transfer_sample`]).
///
/// - [`DriveMode::Tape`]: Warm asymmetric tube/tape saturation (`0 -> 14×` overdrive);
///   `asymmetry` shifts cathode/tape bias for controllable 2nd + 4th even harmonics.
/// - [`DriveMode::HardClip`]: Aggressive op-amp/diode hard clipper (`1× -> 64.5×` = `+36 dB`
///   overdrive) with a tight `C²` `tanh` knee at `0.96`, smooth zero-pop onset from linear `x`,
///   and `k^(-0.15)` output compensation; `asymmetry` introduces smooth pre-clip diode curvature
///   and asymmetric `+/-` clip rails (like 1 vs 2 anti-parallel diodes).
/// - [`DriveMode::Fold`]: West-Coast sine wavefolder (`k = 1 + 2.5 d + 14 d^2`);
///   `asymmetry` adds wavefolder symmetry offset for rich even-harmonic folding.
///
/// Every mode satisfies exact `drive_shape_mode_asym_f64(x, 0.0, asym, mode) == x`,
/// `drive_shape_mode_asym_f64(0.0, d, asym, mode) == 0.0`, and bit-identical output to
/// symmetric operation when `asymmetry == 0.0`.
#[inline]
pub fn drive_shape_mode_asym_f64(x: f64, drive: f64, asymmetry: f64, mode: DriveMode) -> f64 {
    DriveShaperCoeffs::new(drive, asymmetry, mode).shape(x)
}

/// Multi-mode saturation waveshaper with asymmetry (`f32` wrapper over [`drive_shape_mode_asym_f64`]).
#[inline]
pub fn drive_shape_mode_asym(x: f32, drive: f32, asymmetry: f32, mode: DriveMode) -> f32 {
    drive_shape_mode_asym_f64(f64::from(x), f64::from(drive), f64::from(asymmetry), mode) as f32
}

/// Single-point-of-truth symmetric waveshaper in `f64` (`asymmetry = 0.0`), delegating
/// directly to [`drive_shape_mode_asym_f64`].
#[inline]
pub fn drive_shape_mode_f64(x: f64, drive: f64, mode: DriveMode) -> f64 {
    drive_shape_mode_asym_f64(x, drive, 0.0, mode)
}

/// Multi-mode saturation waveshaper (`f32` wrapper over [`drive_shape_mode_f64`]).
#[inline]
pub fn drive_shape_mode(x: f32, drive: f32, mode: DriveMode) -> f32 {
    drive_shape_mode_f64(f64::from(x), f64::from(drive), mode) as f32
}

/// Asymmetric analog-style tube/tape waveshaper in `f64` (2nd + odd harmonics),
/// delegating directly to [`drive_shape_mode_f64`] in [`DriveMode::Tape`].
#[inline]
pub fn asymmetric_drive_shape_f64(x: f64, drive: f64) -> f64 {
    drive_shape_mode_f64(x, drive, DriveMode::Tape)
}

/// Asymmetric analog-style waveshaper (`f32` wrapper over [`asymmetric_drive_shape_f64`]).
#[inline]
pub fn asymmetric_drive_shape(x: f32, drive: f32) -> f32 {
    asymmetric_drive_shape_f64(f64::from(x), f64::from(drive)) as f32
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

/// Transparent voice anti-blowup safety guard (`C¹` continuous at `±0.95`, strictly
/// bounded inside `(-1.0, 1.0)`).
///
/// Strictly for per-voice synth anti-blowup; NEVER use behind a Master bus or Limiter!
#[inline]
pub fn soft_guard_f64(x: f64) -> f64 {
    soft_knee_guard_f64(x, 0.95, 0.05)
}

/// Transparent voice anti-blowup safety guard (`f32`).
#[inline]
pub fn soft_guard(x: f32) -> f32 {
    soft_guard_f64(f64::from(x)) as f32
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

/// 9th-order elliptic polyphase allpass halfband 2× oversampler coefficients (`65 dB` stopband).
///
/// Transfer function `H(z) = 0.5 · (A0(z²) + z⁻¹ · A1(z²))`, where each branch cascades two
/// first-order allpass sections `(a + z⁻²) / (1 + a·z⁻²)`.
/// Passband ripple is `< 0.005 dB` up to `0.43·fs` (`19 kHz` at `44.1 kHz`) with `>= 65 dB`
/// image/alias suppression above `0.59·fs`.
const HALFBAND_A0: [f64; 2] = [0.089_432_841_682, 0.576_520_547_749];
const HALFBAND_A1: [f64; 2] = [0.310_201_463_863, 0.850_257_852_675];

/// Zero-allocation stereo polyphase allpass halfband 2× upsampler + downsampler.
///
/// Uses Direct-Form II Transposed first-order allpass stages (1 `f64` state per stage per
/// channel, 144 bytes total for stereo up+down) so passband frequencies up to `19 kHz`
/// pass with `< 0.01 dB` ripple while nonlinear foldback aliases are suppressed by `> 40 dB`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Halfband2x {
    up_a0: [[f64; 2]; 2],
    up_a1: [[f64; 2]; 2],
    dn_a0: [[f64; 2]; 2],
    dn_a1: [[f64; 2]; 2],
    dn_odd_prev: [f64; 2],
}

impl Halfband2x {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    #[inline]
    fn cascade(state: &mut [f64; 2], coeffs: &[f64; 2], mut x: f64) -> f64 {
        for i in 0..2 {
            let a = coeffs[i];
            let y = a * x + state[i];
            state[i] = flush_denormal_f64(x - a * y);
            x = y;
        }
        x
    }

    /// Upsample one mono base-rate sample (channel 0) into `(even, odd)` double-rate samples.
    #[inline]
    pub fn upsample_mono(&mut self, input: f64) -> (f64, f64) {
        let x0 = sanitize_sample_f64(input);
        let even = Self::cascade(&mut self.up_a0[0], &HALFBAND_A0, x0);
        let odd = Self::cascade(&mut self.up_a1[0], &HALFBAND_A1, x0);
        (even, odd)
    }

    /// Upsample one stereo base-rate frame into `(even, odd)` double-rate frames.
    #[inline]
    pub fn upsample_stereo(&mut self, input: [f64; 2]) -> ([f64; 2], [f64; 2]) {
        let x0 = sanitize_sample_f64(input[0]);
        let x1 = sanitize_sample_f64(input[1]);
        let even = [
            Self::cascade(&mut self.up_a0[0], &HALFBAND_A0, x0),
            Self::cascade(&mut self.up_a0[1], &HALFBAND_A0, x1),
        ];
        let odd = [
            Self::cascade(&mut self.up_a1[0], &HALFBAND_A1, x0),
            Self::cascade(&mut self.up_a1[1], &HALFBAND_A1, x1),
        ];
        (even, odd)
    }

    /// Downsample one `(even, odd)` double-rate frame pair back into one stereo base-rate frame.
    #[inline]
    pub fn downsample_stereo(&mut self, even: [f64; 2], odd: [f64; 2]) -> [f64; 2] {
        let e0 = sanitize_sample_f64(even[0]);
        let e1 = sanitize_sample_f64(even[1]);
        let o0 = sanitize_sample_f64(odd[0]);
        let o1 = sanitize_sample_f64(odd[1]);
        let out = [
            0.5 * (Self::cascade(&mut self.dn_a0[0], &HALFBAND_A0, e0)
                + Self::cascade(&mut self.dn_a1[0], &HALFBAND_A1, self.dn_odd_prev[0])),
            0.5 * (Self::cascade(&mut self.dn_a0[1], &HALFBAND_A0, e1)
                + Self::cascade(&mut self.dn_a1[1], &HALFBAND_A1, self.dn_odd_prev[1])),
        ];
        self.dn_odd_prev = [o0, o1];
        out
    }

    /// Process one stereo frame at 2× oversampling through a double-rate stereo closure `f`.
    #[inline]
    pub fn process_stereo_with(
        &mut self,
        input: [f64; 2],
        mut f: impl FnMut([f64; 2]) -> [f64; 2],
    ) -> [f64; 2] {
        let (u_even, u_odd) = self.upsample_stereo(input);
        let y_even = f(u_even);
        let y_odd = f(u_odd);
        self.downsample_stereo(y_even, y_odd)
    }

    /// True once all internal filter states have decayed below the silence floor.
    #[inline]
    pub fn quiet(&self) -> bool {
        const EPS: f64 = 1e-10;
        self.up_a0
            .iter()
            .chain(self.up_a1.iter())
            .chain(self.dn_a0.iter())
            .chain(self.dn_a1.iter())
            .flatten()
            .chain(self.dn_odd_prev.iter())
            .all(|v| v.abs() < EPS)
    }
}

/// Zero-alloc 2× oversampled stereo drive shaper using precomputed [`DriveShaperCoeffs`].
#[inline]
pub fn shape_drive_oversampled_2x_coeffs(
    os: &mut Halfband2x,
    input: [f64; 2],
    coeffs: DriveShaperCoeffs,
) -> [f64; 2] {
    if coeffs.is_bypass() {
        return os.process_stereo_with(input, |x| x);
    }
    os.process_stereo_with(input, |x| [coeffs.shape(x[0]), coeffs.shape(x[1])])
}

/// Single-point-of-truth zero-alloc 2× oversampled stereo drive shaper
/// (polyphase elliptic halfband upsample → waveshaper → polyphase halfband downsample)
/// shared by [`Drive`] channel inserts and `cadence-engine` modular rack `Drive` modules.
///
/// Applies identically across all three [`DriveMode`] circuits (`Tape`, `HardClip`, `Fold`).
/// Keeps `os` primed in-phase at `drive <= 0.0` so sweeping drive across zero is click-free
/// while preserving `< 0.01 dB` passband flatness up to `19 kHz`.
#[inline]
pub fn shape_drive_oversampled_2x(
    os: &mut Halfband2x,
    input: [f64; 2],
    drive: f64,
    asymmetry: f64,
    mode: DriveMode,
) -> [f64; 2] {
    let coeffs = DriveShaperCoeffs::new(drive, asymmetry, mode);
    shape_drive_oversampled_2x_coeffs(os, input, coeffs)
}

/// Multi-mode saturator/distortion/wavefolder insert composing [`drive_shape_mode_asym_f64`],
/// zero-alloc polyphase halfband 2× oversampling ([`Halfband2x`] / [`shape_drive_oversampled_2x`])
/// across `Tape`, `HardClip`, and `Fold`, click-free `DriveMode` crossfading, [`DcBlocker`],
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
    tone_hz: Smoother,
    bypass_wet: Smoother,
    os: Halfband2x,
    tone_lpf: TptOnePoleLpf,
    dc_blocker: DcBlocker,
    has_processed: bool,
    was_active: bool,
    ctrl_count: u8,
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
        let mut d = Self {
            sample_rate: sr,
            mode: default.mode,
            prev_mode: default.mode,
            mode_xfade: Smoother::new(1.0, 3.0, sr),
            drive: Smoother::new(init_drive, 4.0, sr),
            asymmetry: Smoother::new(init_asym, 4.0, sr),
            output_gain: Smoother::new(db_to_lin_f64(f64::from(default.output_db)), 4.0, sr),
            mix: Smoother::new(f64::from(default.mix), 4.0, sr),
            tone_hz: Smoother::new(f64::from(default.tone_hz), 4.0, sr),
            bypass_wet: Smoother::new(1.0, 1.5, sr),
            os: Halfband2x::new(),
            tone_lpf: TptOnePoleLpf::new(sr, f64::from(default.tone_hz)),
            dc_blocker: DcBlocker::new(sr),
            has_processed: false,
            was_active: false,
            ctrl_count: 0,
            cached_shaper_key: (init_drive.to_bits(), init_asym.to_bits(), default.mode),
            cached_shaper: DriveShaperCoeffs::new(init_drive, init_asym, default.mode),
            cached_tone_bits: f64::from(default.tone_hz).to_bits(),
        };
        d.set_parameters(default);
        d
    }

    pub fn reset(&mut self) {
        self.os.reset();
        self.tone_lpf.reset();
        self.dc_blocker.reset();
        self.prev_mode = self.mode;
        self.mode_xfade.snap(1.0);
        self.drive.snap(self.drive.target());
        self.asymmetry.snap(self.asymmetry.target());
        self.output_gain.snap(self.output_gain.target());
        self.mix.snap(self.mix.target());
        self.tone_hz.snap(self.tone_hz.target());
        self.tone_lpf
            .set_cutoff(self.sample_rate, self.tone_hz.current());
        self.cached_tone_bits = self.tone_hz.current().to_bits();
        let d = self.drive.current();
        let a = self.asymmetry.current();
        self.cached_shaper_key = (d.to_bits(), a.to_bits(), self.mode);
        self.cached_shaper = DriveShaperCoeffs::new(d, a, self.mode);
        self.bypass_wet.snap(1.0);
        self.has_processed = false;
        self.was_active = false;
        self.ctrl_count = 0;
    }

    pub fn set_parameters(&mut self, patch: DrivePatch) {
        let drive = f64::from(sanitize_sample(patch.drive).clamp(0.0, 1.0));
        let asym = f64::from(sanitize_sample(patch.asymmetry).clamp(-1.0, 1.0));
        let out_gain = db_to_lin_f64(f64::from(
            sanitize_sample(patch.output_db).clamp(DRIVE_MIN_OUTPUT_DB, DRIVE_MAX_OUTPUT_DB),
        ));
        let mix = f64::from(sanitize_sample(patch.mix).clamp(0.0, 1.0));
        let tone_raw = if patch.tone_hz.is_finite() {
            f64::from(patch.tone_hz)
        } else {
            f64::from(DRIVE_MAX_TONE_HZ)
        };
        let tone = tone_raw.clamp(f64::from(DRIVE_MIN_TONE_HZ), self.sample_rate * 0.45);
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

    pub fn process(&mut self, input: [f32; 2], bypassed: bool) -> [f32; 2] {
        self.has_processed = true;
        let dry = sanitize_frame(input);
        if bypassed && !self.was_active {
            self.bypass_wet.snap(0.0);
            return dry;
        }
        self.bypass_wet.set_target(if bypassed { 0.0 } else { 1.0 });
        let bypass_w = self.bypass_wet.next();
        if bypassed && bypass_w <= 1e-4 {
            self.bypass_wet.snap(0.0);
            self.os.reset();
            self.tone_lpf.reset();
            self.dc_blocker.reset();
            self.was_active = false;
            return dry;
        }
        self.was_active = true;

        let drv = self.drive.next();
        let asym = self.asymmetry.next();
        let out_g = self.output_gain.next();
        let mix = self.mix.next();
        let tone = self.tone_hz.next();
        let m_xf = self.mode_xfade.next();
        if self.ctrl_count == 0 {
            let tone_bits = tone.to_bits();
            if tone_bits != self.cached_tone_bits {
                self.cached_tone_bits = tone_bits;
                self.tone_lpf.set_cutoff(self.sample_rate, tone);
            }
        }
        self.ctrl_count = (self.ctrl_count + 1) & 7;

        let dry64 = [f64::from(dry[0]), f64::from(dry[1])];
        let shaped = if drv <= 0.0 || m_xf >= 1.0 - 1e-6 {
            let key = (drv.to_bits(), asym.to_bits(), self.mode);
            if key != self.cached_shaper_key {
                self.cached_shaper_key = key;
                self.cached_shaper = DriveShaperCoeffs::new(drv, asym, self.mode);
            }
            shape_drive_oversampled_2x_coeffs(&mut self.os, dry64, self.cached_shaper)
        } else {
            // All three modes share the same 2× halfband oversampler: blend linearly at 2× rate.
            let inv = 1.0 - m_xf;
            let ca = DriveShaperCoeffs::new(drv, asym, self.prev_mode);
            let cb = DriveShaperCoeffs::new(drv, asym, self.mode);
            self.os.process_stereo_with(dry64, |x| {
                [
                    ca.shape(x[0]) * inv + cb.shape(x[0]) * m_xf,
                    ca.shape(x[1]) * inv + cb.shape(x[1]) * m_xf,
                ]
            })
        };

        let dc_free = self.dc_blocker.process_stereo(shaped);
        let toned = self.tone_lpf.process_stereo(dc_free);
        let eff_wet = mix * bypass_w;
        let eff_dry = 1.0 - eff_wet;
        [
            (dry64[0] * eff_dry + toned[0] * out_g * eff_wet) as f32,
            (dry64[1] * eff_dry + toned[1] * out_g * eff_wet) as f32,
        ]
    }
}

impl crate::dsp::StereoEffect for Drive {
    #[inline]
    fn process(&mut self, input: [f32; 2], bypassed: bool) -> [f32; 2] {
        Drive::process(self, input, bypassed)
    }

    #[inline]
    fn reset(&mut self) {
        Drive::reset(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn waveshapers_are_exact_identity_at_zero_and_continuous_near_zero_for_all_modes() {
        for &x in &[-1.0_f32, -0.75, -0.5, -0.1, 0.0, 0.1, 0.5, 0.75, 1.0] {
            assert_eq!(soft_clip_drive(x, 0.0), x);
            assert_eq!(asymmetric_drive_shape(x, 0.0), x);

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
        // Zero static DC offset at silence for any drive level across all modes.
        for i in 0..=20 {
            let d = i as f32 / 20.0;
            assert_eq!(soft_clip_drive(0.0, d), 0.0);
            for mode in DriveMode::ALL {
                assert!(
                    drive_shape_mode(0.0, d, mode).abs() < 1e-7,
                    "{mode:?} static DC at d={d}"
                );
            }
        }
        // Tape is asymmetric (even harmonics), while HardClip & Fold are strictly odd-symmetric (zero DC).
        let pos = asymmetric_drive_shape(0.7, 0.9);
        let neg = asymmetric_drive_shape(-0.7, 0.9);
        assert!(
            (pos + neg).abs() > 0.01,
            "expected Tape asymmetric 2nd harmonic: pos={pos}, neg={neg}"
        );
        for mode in [DriveMode::HardClip, DriveMode::Fold] {
            for &d in &[0.2, 0.5, 0.85, 1.0] {
                for &x in &[0.05, 0.25, 0.7, 1.0] {
                    let p = drive_shape_mode_f64(x, d, mode);
                    let n = drive_shape_mode_f64(-x, d, mode);
                    assert!(
                        (p + n).abs() < 1e-12,
                        "{mode:?} must be strictly odd-symmetric: d={d} x={x} p={p} n={n}"
                    );
                }
                let at_one = drive_shape_mode_f64(1.0, d, mode).abs();
                if mode == DriveMode::HardClip && d >= 0.5 {
                    assert!(
                        at_one <= 0.9,
                        "HardClip at x=1.0 d={d} must stay <= 0.9, got {at_one}"
                    );
                }
                assert!(
                    at_one <= 1.0,
                    "{mode:?} at x=1.0 d={d} must stay <= 1.0, got {at_one}"
                );
            }
        }
    }

    #[test]
    fn soft_guard_is_transparent_below_095_and_bounded_below_1() {
        for &x in &[-0.95_f32, -0.5, 0.0, 0.5, 0.95] {
            assert_eq!(soft_guard(x), x);
        }
        for &x in &[-10.0_f32, -2.0, -1.0, 1.0, 2.0, 10.0] {
            let y = soft_guard(x);
            assert!(y.is_finite() && y.abs() <= 1.0 && y.abs() > 0.95);
            assert_eq!(y.signum(), x.signum());
        }
        assert_eq!(soft_guard(f32::NAN), 0.0);
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
    fn hard_clip_pins_minus_20_dbfs_sine_into_strong_odd_harmonics() {
        let sr = 48_000.0;
        let f0 = 300.0; // integer cycles inside 4800-sample measurement window (30 exact cycles)
        for &drive in &[0.85_f32, 1.0_f32] {
            let mut drv = Drive::new(sr);
            drv.set_parameters(DrivePatch {
                mode: DriveMode::HardClip,
                drive,
                asymmetry: 0.0,
                tone_hz: 18_000.0,
                output_db: 0.0,
                mix: 1.0,
            });
            drv.reset();
            let warmup = 4_800;
            let measure = 4_800;
            let mut buf = Vec::with_capacity(measure);
            for i in 0..(warmup + measure) {
                // -20 dBFS amplitude = 0.10
                let x = (TAU * f0 * i as f64 / sr).sin() as f32 * 0.10;
                let y = drv.process([x, x], false);
                if i >= warmup {
                    buf.push(y[0]);
                }
            }
            let h1 = dft_mag(&buf, f0, sr);
            let h2 = dft_mag(&buf, f0 * 2.0, sr);
            let h3 = dft_mag(&buf, f0 * 3.0, sr);
            let h5 = dft_mag(&buf, f0 * 5.0, sr);
            let h7 = dft_mag(&buf, f0 * 7.0, sr);
            assert!(
                h1 > 0.35,
                "drive={drive}: -20 dBFS input must pin strongly, h1={h1}"
            );
            assert!(
                h3 > h1 * 0.22 && h5 > h1 * 0.10 && h7 > h1 * 0.05,
                "drive={drive}: expected crisp odd harmonics 3f/5f/7f: h1={h1:.4} h3={h3:.4} h5={h5:.4} h7={h7:.4}"
            );
            assert!(
                h2 < h1 * 1e-3,
                "drive={drive}: odd-symmetric HardClip must have negligible 2nd harmonic: h2={h2:.6} h1={h1:.4}"
            );
        }
    }

    #[test]
    fn fold_mode_exhibits_non_monotonic_west_coast_foldback_and_2x_antialiasing() {
        // 1. Transfer curve must rise, peak, and fold back downward past zero as x sweeps 0..1.
        let samples: Vec<f64> = (0..=100)
            .map(|i| drive_shape_mode_f64(i as f64 / 100.0, 0.75, DriveMode::Fold))
            .collect();
        let max_val = samples.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let min_val = samples.iter().copied().fold(f64::INFINITY, f64::min);
        let mut sign_reversals = 0usize;
        for w in samples.windows(3) {
            let d1 = w[1] - w[0];
            let d2 = w[2] - w[1];
            if d1 * d2 < 0.0 {
                sign_reversals += 1;
            }
        }
        assert!(
            max_val > 0.5 && min_val < -0.5 && sign_reversals >= 3,
            "Fold at d=0.75 must fold back non-monotonically across zero: max={max_val}, min={min_val}, reversals={sign_reversals}"
        );

        // 2. Verify Drive::process with 2x oversampling reduces foldback aliasing on high-freq tone
        // compared to naive 1x waveshaping at 48 kHz.
        let sr = 48_000.0;
        let freq = 4_000.0;
        let mut drv = Drive::new(sr);
        drv.set_parameters(DrivePatch {
            mode: DriveMode::Fold,
            drive: 0.7,
            asymmetry: 0.0,
            tone_hz: 18_000.0,
            output_db: 0.0,
            mix: 1.0,
        });
        drv.reset();
        let warmup = 2_400;
        let measure = 4_800;
        let mut os_buf = Vec::with_capacity(measure);
        let mut naive_buf = Vec::with_capacity(measure);
        for i in 0..(warmup + measure) {
            let x = (TAU * freq * i as f64 / sr).sin() as f32 * 0.9;
            let y = drv.process([x, x], false);
            if i >= warmup {
                os_buf.push(y[0]);
                naive_buf.push(drive_shape_mode(x, 0.7, DriveMode::Fold));
            }
        }
        // Check a non-harmonic aliased frequency (e.g. 5*4k = 20k ok, 7*4k = 28k aliases to 20k,
        // 9*4k = 36k aliases to 12k, 11*4k = 44k aliases to 4k, 13*4k = 52k aliases to 4k).
        // Better: use an inharmonic prime-ish fundamental 3700 Hz so 9*3700=33300 aliases to 14700 Hz
        // and 11*3700=40700 aliases to 7300 Hz, distinct from harmonics!
        let f_in = 3_700.0;
        drv.reset();
        os_buf.clear();
        naive_buf.clear();
        for i in 0..(warmup + measure) {
            let x = (TAU * f_in * i as f64 / sr).sin() as f32 * 0.9;
            let y = drv.process([x, x], false);
            if i >= warmup {
                os_buf.push(y[0]);
                naive_buf.push(drive_shape_mode(x, 0.7, DriveMode::Fold));
            }
        }
        // 11 * 3700 = 40700 Hz aliases to 48000 - 40700 = 7300 Hz (below tone LPF 18 kHz!).
        let alias_f = sr - 11.0 * f_in; // 7300 Hz
        let alias_os = dft_mag(&os_buf, alias_f, sr);
        let alias_1x = dft_mag(&naive_buf, alias_f, sr);
        assert!(
            alias_os < alias_1x * 0.45,
            "2x oversampling must suppress foldback aliasing at {alias_f} Hz: os={alias_os:.5} vs 1x={alias_1x:.5}"
        );

        // 3. Verify DriveMode::Tape is also 2x oversampled and suppresses foldback aliasing.
        let mut tape = Drive::new(sr);
        tape.set_parameters(DrivePatch {
            mode: DriveMode::Tape,
            drive: 0.85,
            asymmetry: 0.1,
            tone_hz: 18_000.0,
            output_db: 0.0,
            mix: 1.0,
        });
        tape.reset();
        os_buf.clear();
        naive_buf.clear();
        for i in 0..(warmup + measure) {
            let x = (TAU * f_in * i as f64 / sr).sin() as f32 * 0.9;
            let y = tape.process([x, x], false);
            if i >= warmup {
                os_buf.push(y[0]);
                naive_buf.push(drive_shape_mode_asym(x, 0.85, 0.1, DriveMode::Tape));
            }
        }
        let tape_alias_f = sr - 9.0 * f_in; // 14700 Hz (odd harmonic foldback)
        let tape_os = dft_mag(&os_buf, tape_alias_f, sr);
        let tape_1x = dft_mag(&naive_buf, tape_alias_f, sr);
        assert!(
            tape_os < tape_1x * 0.15,
            "2x Halfband2x must suppress Tape foldback aliasing at {tape_alias_f} Hz: os={tape_os:.6} vs 1x={tape_1x:.6}"
        );

        // 4. Verify Halfband2x preserves flat passband (< 0.02 dB ripple up to 19 kHz @ 44.1 kHz).
        let sr44 = 44_100.0;
        for &freq_hz in &[200.0_f64, 5_000.0, 14_000.0, 18_000.0, 19_000.0] {
            let mut hb = Halfband2x::new();
            let mut in_sq = 0.0_f64;
            let mut out_sq = 0.0_f64;
            for i in 0..4_096 {
                let x = (TAU * freq_hz * i as f64 / sr44).sin();
                let y = hb.process_stereo_with([x, x], |s| s);
                if i >= 1_024 {
                    in_sq += x * x;
                    out_sq += y[0] * y[0];
                }
            }
            let diff_db = crate::dsp::lin_to_db_f64((out_sq / in_sq).sqrt());
            assert!(
                diff_db.abs() < 0.02,
                "Halfband2x must be flat at {freq_hz} Hz @ 44.1 kHz, got {diff_db:+.4} dB"
            );
        }
    }

    #[test]
    fn drive_mode_transitions_are_click_free_and_bounded() {
        let sr = 48_000.0;
        let modes = [
            DriveMode::Tape,
            DriveMode::HardClip,
            DriveMode::Fold,
            DriveMode::Tape,
            DriveMode::Fold,
            DriveMode::HardClip,
        ];
        // Verify every directed mode transition stays bounded and introduces negligible
        // single-sample discontinuity relative to an unswitched reference on the switch sample.
        for pair in modes.windows(2) {
            let from = pair[0];
            let to = pair[1];
            let mut switched = Drive::new(sr);
            let mut reference = Drive::new(sr);
            let p_from = DrivePatch {
                mode: from,
                drive: 0.8,
                asymmetry: 0.0,
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
                    // On the exact switch sample, the 3ms crossfade starts near 0 -> < 0.02 delta!
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
                // Exact identity at drive == 0.0 regardless of asymmetry.
                for &x in &[-0.9, -0.3, 0.0, 0.3, 0.9] {
                    assert_eq!(drive_shape_mode_asym_f64(x, 0.0, asym, mode), x);
                }
                // Exact zero at x == 0.0 for all drive/asymmetry settings (zero DC).
                for &d in &[0.0, 0.25, 0.65, 1.0] {
                    assert_eq!(drive_shape_mode_asym_f64(0.0, d, asym, mode), 0.0);
                }
            }
            // Bit-identical to symmetric drive_shape_mode_f64 when asymmetry == 0.0.
            for &x in &[-0.8, -0.2, 0.2, 0.8] {
                for &d in &[0.15, 0.5, 0.9] {
                    assert_eq!(
                        drive_shape_mode_asym_f64(x, d, 0.0, mode),
                        drive_shape_mode_f64(x, d, mode)
                    );
                }
            }
        }

        // Verify HardClip & Fold with non-zero asymmetry generate strong 2nd harmonic on a pure sine
        // (whereas symmetric asymmetry == 0.0 generates negligible H2), while staying DC-free & bounded.
        let sr = 48_000.0;
        let f0 = 200.0;
        for mode in [DriveMode::HardClip, DriveMode::Fold, DriveMode::Tape] {
            let mut drv = Drive::new(sr);
            drv.set_parameters(DrivePatch {
                mode,
                drive: 0.45,
                asymmetry: 0.65,
                tone_hz: 16_000.0,
                output_db: 0.0,
                mix: 1.0,
            });
            drv.reset();
            let warmup = 4_800;
            let measure = 4_800;
            let mut buf = Vec::with_capacity(measure);
            let mut dc_sum = 0.0_f64;
            for i in 0..(warmup + measure) {
                let x = (TAU * f0 * i as f64 / sr).sin() as f32 * 0.45;
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
        // Transparent below knee and odd-symmetric above knee for feedback guard (knee=4, headroom=4).
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
        // Check C^1 unit slope across the knee (4.0).
        let eps = 1e-6;
        let slope_left =
            (soft_knee_guard_f64(4.0, 4.0, 4.0) - soft_knee_guard_f64(4.0 - eps, 4.0, 4.0)) / eps;
        let slope_right =
            (soft_knee_guard_f64(4.0 + eps, 4.0, 4.0) - soft_knee_guard_f64(4.0, 4.0, 4.0)) / eps;
        assert!((slope_left - 1.0).abs() < 1e-6);
        assert!((slope_right - 1.0).abs() < 1e-6);

        // Matches soft_guard_f64 exactly at (knee=0.95, headroom=0.05).
        for &x in &[-5.0_f64, -1.0, -0.95, 0.0, 0.95, 1.0, 5.0] {
            assert_eq!(soft_guard_f64(x), soft_knee_guard_f64(x, 0.95, 0.05));
        }

        // Non-finite and zero-headroom safety.
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
            let d2 = d * d;
            let env_a = 1.0 - (-5.0 * d).exp();
            match mode {
                DriveMode::Tape => {
                    let k = d * 1.5 + d2 * 12.5;
                    let bias = 0.08 * d2 + 0.70 * a * env_a;
                    let comp = (1.0 + k).powf(-0.18);
                    if k < 1e-4 {
                        x * comp
                    } else {
                        ((k * x + bias).tanh() - bias.tanh()) / k.tanh() * comp
                    }
                }
                DriveMode::HardClip => {
                    let k = 1.0 + d * 2.5 + d2 * 16.0 + d2 * d * 45.0;
                    let w = 1.0 - (-30.0 * d2).exp();
                    let comp = k.powf(-0.15);
                    let u = k * x;
                    let u_asym = u + (0.24 * a * env_a) * (u * u) / (1.0 + 0.5 * u.abs());
                    let ceil = if u_asym >= 0.0 {
                        1.0 - 0.30 * a * env_a
                    } else {
                        1.0 + 0.30 * a * env_a
                    };
                    let knee = 0.96 * ceil;
                    let clipped = soft_knee_guard_f64(u_asym, knee, (ceil - knee).max(1e-6));
                    x * (1.0 - w) + clipped * comp * w
                }
                DriveMode::Fold => {
                    let k = 1.0 + d * 2.5 + d2 * 14.0;
                    let w = 1.0 - (-30.0 * d2).exp();
                    let fold_scale = 1.0 / (1.0 + 0.5 * env_a);
                    let bias = 0.55 * a * env_a;
                    let folded = (k * x + bias).sin() - bias.sin();
                    x * (1.0 - w) + folded * fold_scale * w
                }
            }
        };

        let xs = [-2.5_f64, -1.0, -0.45, -0.02, 0.0, 0.02, 0.45, 1.0, 2.5];
        let drives = [0.0_f64, 1e-5, 0.15, 0.5, 0.85, 1.0];
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
            for mode in [DriveMode::Tape, DriveMode::HardClip, DriveMode::Fold] {
                for &a in &asyms {
                    let dc = DriveShaperCoeffs::new(d, a, mode);
                    for &x in &xs {
                        let got = dc.shape(x);
                        let expected = ref_drive(x, d, a, mode);
                        assert!(
                            (got - expected).abs() <= 1e-13,
                            "DriveShaperCoeffs mismatch mode={mode:?} d={d} a={a} x={x}: got={got} expected={expected}"
                        );
                    }
                }
            }
        }
        assert_eq!(SoftClipCoeffs::new(f64::NAN).shape(0.5), 0.5);
        assert_eq!(
            DriveShaperCoeffs::new(f64::NAN, 0.5, DriveMode::Tape).shape(0.5),
            0.5
        );
    }

    #[test]
    fn drive_insert_parameter_sweeps_mode_crossfade_and_reset_stay_coherent() {
        let sr = 48_000.0;
        let mut drv = Drive::new(sr);
        let p_tape = DrivePatch {
            mode: DriveMode::Tape,
            drive: 0.65,
            asymmetry: 0.3,
            tone_hz: 6_000.0,
            output_db: -2.0,
            mix: 1.0,
        };
        let p_fold = DrivePatch {
            mode: DriveMode::Fold,
            drive: 0.8,
            asymmetry: -0.4,
            tone_hz: 12_000.0,
            output_db: 1.0,
            mix: 1.0,
        };
        drv.set_parameters(p_tape);
        for i in 0..512 {
            let s = (std::f64::consts::TAU * 330.0 * i as f64 / sr).sin() as f32 * 0.6;
            let out = drv.process([s, -s], false);
            assert!(out[0].is_finite() && out[1].is_finite());
        }
        // Switch mode + parameters mid-stream (exercises mode crossfade and cache refresh).
        drv.set_parameters(p_fold);
        for i in 512..3072 {
            let s = (std::f64::consts::TAU * 330.0 * i as f64 / sr).sin() as f32 * 0.6;
            let out = drv.process([s, -s], false);
            assert!(out[0].is_finite() && out[1].is_finite());
        }
        // After reset + fresh init at p_fold, steady-state outputs must match bit-for-bit.
        drv.reset();
        let mut fresh = Drive::new(sr);
        fresh.set_parameters(p_fold);
        for i in 0..256 {
            let s = (std::f64::consts::TAU * 440.0 * i as f64 / sr).sin() as f32 * 0.5;
            assert_eq!(drv.process([s, s], false), fresh.process([s, s], false));
        }
    }
}
