//! Pure `1×`-sample-rate cubic algebraic and Hardy-space VCO oscillators,
//! automatic `C^∞` pitch-aware Nyquist reach guarding, unified M/S quadrature
//! stereo encoding, dedicated LFO shape evaluation, constant-power unison
//! helpers, and `1×` through-zero linear FM.
//!
//! All oscillator waveforms compose stateless closed-form unit-circle exciters
//! from [`crate::dsp::cubic::exciter`] with zero oversampling, zero wavetables,
//! and zero heap allocations.

use crate::dsp::cubic::exciter::{
    bare_cubic_exciter, dipole_analytic, hardy_saw_exciter, mobius_exciter_coeffs, mobius_pwm,
    pitch_guard_rho, poisson_analytic, szego_analytic, CubicWaveform, HardyCoeffs, MobiusCoeffs,
};
use crate::dsp::gain::pan_mono_equal_power_f64;
use crate::dsp::math::{
    sanitize_sample_f64, sanitize_sample_rate, semitones_to_ratio_f64, NYQUIST_SAFE_RATIO,
};
use crate::rack::{LfoShape, Waveform};
use std::f64::consts::{FRAC_PI_4, SQRT_2, TAU};

/// Maximum unison voices supported per oscillator in `VcoModule`.
pub const MAX_UNISON: usize = 7;

/// Maximum spectral reach radius supported by `VcoModule`.
pub const VCO_MAX_REACH: f64 = 0.995;

/// Upper bound of the cubic curvature parameter `k = cubic_curvature(shape)`
/// for `MobiusJet`, `MobiusPwm`, and `BareCubic`.
pub const CUBIC_MAX_CURVATURE: f64 = 12.0;

/// Perceptually uniform quintic curvature taper `k(shape) = 12 * shape^5` in `[0, 12]`.
///
/// Maps `shape = 0.0` to the pure rational Blaschke limit (`k = 0.0`), centers the classic
/// analog sawtooth (`k = 0.375`, `rms ≈ 0.58`, `H2 ≈ -4.8 dB`) at `shape = 0.5`, and morphs
/// smoothly into a rounded odd-harmonic square (`k = 12.0`) at `shape = 1.0`.
#[inline]
pub fn cubic_curvature(shape: f64) -> f64 {
    let s = sanitize_sample_f64(shape).clamp(0.0, 1.0);
    let s2 = s * s;
    CUBIC_MAX_CURVATURE * s2 * s2 * s
}

/// Wrap phase into `[0.0, 1.0)`.
#[inline]
pub fn phase_wrap(phase: f64) -> f64 {
    let p = sanitize_sample_f64(phase);
    let wrapped = p.rem_euclid(1.0);
    if wrapped >= 1.0 {
        0.0
    } else {
        wrapped
    }
}

/// Evaluate a deterministic [`LfoShape`] at wrapped phase `phase ∈ [0, 1)` in `f64`.
///
/// `Sine` and `Triangle` share `sin(2πφ)` zero-crossing alignment: `Triangle`
/// starts click-free at `0.0` (`phase = 0.0`), peaks at `+1.0` (`0.25`), crosses
/// `0.0` (`0.5`), and troughs at `-1.0` (`0.75`). `SampleHold` returns `0.0`
/// because stateful random stepping is owned by the voice LFO node.
#[inline]
pub fn lfo_shape_sample_f64(shape: LfoShape, phase: f64) -> f64 {
    let p = phase_wrap(phase);
    match shape {
        LfoShape::Sine => (TAU * p).sin(),
        LfoShape::Triangle => {
            let t = phase_wrap(p + 0.25);
            1.0 - 4.0 * (t - 0.5).abs()
        }
        LfoShape::Saw => 2.0 * p - 1.0,
        LfoShape::Square => {
            if p < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        LfoShape::SampleHold => 0.0,
    }
}

/// Evaluate a deterministic [`LfoShape`] in `f32` for UI LFO waveform plots.
///
/// Re-exported by `crate::rack::lfo_shape_sample`.
#[inline]
pub fn lfo_shape_sample(shape: LfoShape, phase: f32) -> f32 {
    lfo_shape_sample_f64(shape, f64::from(phase)) as f32
}

/// Unified Mid/Side quadrature stereo encoder with constant stereo RMS power across
/// `chirality ∈ [-1.0, 1.0]` and algebraically zero-comb mono collapse
/// (`0.5 * (L + R) == re / sqrt(1 + chi²)`).
///
/// At `chirality == 0.0`, returns `[re, re]` identically without touching `im`.
#[inline]
pub fn ms_quadrature_encode(re: f64, im: f64, chirality: f64) -> [f64; 2] {
    let re = sanitize_sample_f64(re);
    let chi = sanitize_sample_f64(chirality).clamp(-1.0, 1.0);
    if chi == 0.0 {
        return [re, re];
    }
    let im = sanitize_sample_f64(im);
    let inv = 1.0 / (1.0 + chi * chi).sqrt();
    let re_s = re * inv;
    let im_s = (chi * im) * inv;
    [re_s + im_s, re_s - im_s]
}

/// Unified Mid/Side quadrature encoder for unit-`L²` complex analytic Hardy drivers
/// (`E[re²] == E[im²] == 0.5`), scaling by `√2` so every channel has exact unit RMS
/// across all `chirality ∈ [-1.0, 1.0]`:
/// - `chi == 0.0`: `L == R == √2 · re` (`E[L²] = 1.0`).
/// - `chi == +1.0`: `L == re + im`, `R == re - im` (`Mid == re`, `Side == +im`).
/// - `chi == -1.0`: `L == re - im`, `R == re + im` (`Mid == re`, `Side == -im`).
#[inline]
fn ms_quadrature_encode_analytic(re: f64, im: f64, chirality: f64) -> [f64; 2] {
    let re = sanitize_sample_f64(re);
    let chi = sanitize_sample_f64(chirality).clamp(-1.0, 1.0);
    if chi == 0.0 {
        let mono = re * SQRT_2;
        return [mono, mono];
    }
    let im = sanitize_sample_f64(im);
    let inv = SQRT_2 / (1.0 + chi * chi).sqrt();
    let re_s = re * inv;
    let im_s = (chi * im) * inv;
    [re_s + im_s, re_s - im_s]
}

/// Precomputed stack-only coefficients for a single waveform evaluation or unison voice.
#[derive(Clone, Copy, Debug)]
enum PreparedOsc {
    MobiusJet(MobiusCoeffs),
    MobiusPwm {
        coeffs: MobiusCoeffs,
        width_c: f64,
        width_s: f64,
    },
    BareCubic(MobiusCoeffs),
    HardySaw(MobiusCoeffs),
    Sine,
    Szego(HardyCoeffs),
    Dipole(HardyCoeffs),
    Poisson {
        coeffs: HardyCoeffs,
        m0: f64,
        w0: f64,
        w1: f64,
    },
}

/// Pulse-width specification for [`PreparedOsc::new`], allowing [`step_vco_bank`]
/// to hoist `(cos(2π·pw), sin(2π·pw))` once outside the unison voice loop.
#[derive(Clone, Copy)]
enum WidthSpec {
    Pw(f64),
    CosSin(f64, f64),
}

impl PreparedOsc {
    #[inline]
    fn new(
        waveform: Waveform,
        reach: f64,
        shape: f64,
        width: WidthSpec,
        freq_hz: f64,
        sample_rate: f64,
        pitch_guard: bool,
    ) -> Self {
        let reach = sanitize_sample_f64(reach).clamp(0.0, VCO_MAX_REACH);
        let shape = sanitize_sample_f64(shape).clamp(0.0, 1.0);
        let sr = sanitize_sample_rate(sample_rate);
        let freq = sanitize_sample_f64(freq_hz).abs();
        let guard = pitch_guard && freq > 0.0;
        let rho = if guard {
            pitch_guard_rho(reach, freq, sr)
        } else {
            reach
        };

        match waveform {
            Waveform::MobiusJet => Self::MobiusJet(MobiusCoeffs::new(rho, cubic_curvature(shape))),
            Waveform::MobiusPwm => {
                let (width_c, width_s) = match width {
                    WidthSpec::CosSin(wc, ws) => (wc, ws),
                    WidthSpec::Pw(pulse_width) => {
                        let pw = sanitize_sample_f64(pulse_width).clamp(0.02, 0.98);
                        let (ws, wc) = (TAU * pw).sin_cos();
                        (wc, ws)
                    }
                };
                Self::MobiusPwm {
                    coeffs: MobiusCoeffs::new(rho, cubic_curvature(shape)),
                    width_c,
                    width_s,
                }
            }
            Waveform::BareCubic => Self::BareCubic(MobiusCoeffs::new(rho, cubic_curvature(shape))),
            Waveform::HardySaw => Self::HardySaw(MobiusCoeffs::new(rho, 0.0)),
            Waveform::Sine => Self::Sine,
            Waveform::Szego => Self::Szego(HardyCoeffs::new(
                CubicWaveform::Szego,
                rho,
                shape,
                freq,
                sr,
                guard,
            )),
            Waveform::Dipole => Self::Dipole(HardyCoeffs::new(
                CubicWaveform::Dipole,
                rho,
                shape,
                freq,
                sr,
                guard,
            )),
            Waveform::Poisson => {
                // Continuous harmonic ratio `m = 1 + 5 * shape`, capped continuously below
                // Nyquist when guarded so blended integer harmonics `(m0, m0 + 1)` never fold.
                let m_raw = 1.0 + 5.0 * shape;
                let m_eff = if guard {
                    let m_max = (sr * NYQUIST_SAFE_RATIO / freq - 1.0).max(1.0);
                    m_raw.min(m_max)
                } else {
                    m_raw
                };
                let geom_eff = ((m_eff - 1.0) / 5.0).clamp(0.0, 1.0);
                let coeffs =
                    HardyCoeffs::new(CubicWaveform::Poisson, rho, geom_eff, freq, sr, guard);
                let m0 = m_eff.floor();
                let frac = (m_eff - m0).clamp(0.0, 1.0);
                let (w0, w1) = if frac == 0.0 {
                    (1.0, 0.0)
                } else {
                    // Exact L² normalization of the periodic adjacent-harmonic blend
                    // `(1 - frac) e^{i m0 θ} + frac e^{i (m0 + 1) θ}` under the Poisson kernel:
                    //   E[P_r² · |w0 + w1 e^{iθ}|²] / E[P_r²] = 1 - 2 f (1 - f) (1 - r)² / (1 + r²).
                    let r = coeffs.r;
                    let omr = 1.0 - r;
                    let blend_power = 1.0 - 2.0 * frac * (1.0 - frac) * (omr * omr) / (1.0 + r * r);
                    let inv = 1.0 / blend_power.max(1e-12).sqrt();
                    ((1.0 - frac) * inv, frac * inv)
                };
                Self::Poisson { coeffs, m0, w0, w1 }
            }
        }
    }

    #[inline]
    fn eval_poisson_analytic(
        coeffs: &HardyCoeffs,
        m0: f64,
        w0: f64,
        w1: f64,
        theta: f64,
        c: f64,
        s: f64,
    ) -> (f64, f64) {
        // Rotating the formant carrier by +π/4 aligns `cos²(mθ + π/4)` and `sin²(mθ + π/4)`
        // symmetrically across the even Poisson spike so `E[re²] == E[im²] == 0.5` identically.
        let (s0, c0) = (m0 * theta + FRAC_PI_4).sin_cos();
        let (fc, fs) = if w1 == 0.0 {
            (c0, s0)
        } else {
            let c1 = c0 * c - s0 * s;
            let s1 = s0 * c + c0 * s;
            (w0 * c0 + w1 * c1, w0 * s0 + w1 * s1)
        };
        poisson_analytic(c, fc, fs, coeffs.r, coeffs.poisson_norm)
    }

    /// Evaluate the real component `re` on wrapped `phase ∈ [0, 1)`.
    #[inline]
    fn eval_re(&self, phase: f64) -> f64 {
        let theta = TAU * phase_wrap(phase);
        let (s, c) = theta.sin_cos();
        match *self {
            Self::MobiusJet(ref coeffs) => mobius_exciter_coeffs(c, s, coeffs),
            Self::MobiusPwm {
                ref coeffs,
                width_c,
                width_s,
            } => mobius_pwm(c, s, width_c, width_s, coeffs),
            Self::BareCubic(ref coeffs) => {
                bare_cubic_exciter(c, s, coeffs.r, coeffs.k, coeffs.bare_gain)
            }
            Self::HardySaw(ref coeffs) => hardy_saw_exciter(c, s, coeffs.rho, coeffs.hardy_gain),
            Self::Sine => s,
            Self::Szego(ref coeffs) => szego_analytic(c, s, coeffs.r, coeffs.szego_norm).0,
            Self::Dipole(ref coeffs) => dipole_analytic(c, s, coeffs).0,
            Self::Poisson {
                ref coeffs,
                m0,
                w0,
                w1,
            } => Self::eval_poisson_analytic(coeffs, m0, w0, w1, theta, c, s).0,
        }
    }

    /// Evaluate M/S quadrature stereo `[L, R]` on wrapped `phase ∈ [0, 1)`.
    #[inline]
    fn eval_stereo(&self, phase: f64, chirality: f64) -> [f64; 2] {
        let theta = TAU * phase_wrap(phase);
        let (s, c) = theta.sin_cos();
        let chi = sanitize_sample_f64(chirality).clamp(-1.0, 1.0);
        match *self {
            Self::MobiusJet(ref coeffs) => {
                let re = mobius_exciter_coeffs(c, s, coeffs);
                if chi == 0.0 {
                    [re, re]
                } else {
                    let im = mobius_exciter_coeffs(-s, c, coeffs);
                    ms_quadrature_encode(re, im, chi)
                }
            }
            Self::MobiusPwm {
                ref coeffs,
                width_c,
                width_s,
            } => {
                let re = mobius_pwm(c, s, width_c, width_s, coeffs);
                if chi == 0.0 {
                    [re, re]
                } else {
                    let im = mobius_pwm(-s, c, width_c, width_s, coeffs);
                    ms_quadrature_encode(re, im, chi)
                }
            }
            Self::BareCubic(ref coeffs) => {
                let re = bare_cubic_exciter(c, s, coeffs.r, coeffs.k, coeffs.bare_gain);
                if chi == 0.0 {
                    [re, re]
                } else {
                    let im = bare_cubic_exciter(-s, c, coeffs.r, coeffs.k, coeffs.bare_gain);
                    ms_quadrature_encode(re, im, chi)
                }
            }
            Self::HardySaw(ref coeffs) => {
                let re = hardy_saw_exciter(c, s, coeffs.rho, coeffs.hardy_gain);
                if chi == 0.0 {
                    [re, re]
                } else {
                    let im = hardy_saw_exciter(-s, c, coeffs.rho, coeffs.hardy_gain);
                    ms_quadrature_encode(re, im, chi)
                }
            }
            Self::Sine => ms_quadrature_encode(s, c, chi),
            Self::Szego(ref coeffs) => {
                let (re, im) = szego_analytic(c, s, coeffs.r, coeffs.szego_norm);
                ms_quadrature_encode_analytic(re, im, chi)
            }
            Self::Dipole(ref coeffs) => {
                let (re, im) = dipole_analytic(c, s, coeffs);
                ms_quadrature_encode_analytic(re, im, chi)
            }
            Self::Poisson {
                ref coeffs,
                m0,
                w0,
                w1,
            } => {
                let (re, im) = Self::eval_poisson_analytic(coeffs, m0, w0, w1, theta, c, s);
                ms_quadrature_encode_analytic(re, im, chi)
            }
        }
    }
}

/// Pure `1×`-sample-rate stereo cubic/Hardy oscillator with automatic `C^∞`
/// pitch-aware Nyquist reach guarding and unified M/S quadrature `chirality ∈ [-1, 1]`.
///
/// When `freq_hz == 0.0`, the reach guard falls back cleanly to unguarded `reach`.
#[inline]
#[allow(clippy::too_many_arguments)]
pub fn cubic_oscillator_stereo(
    waveform: Waveform,
    phase: f64,
    freq_hz: f64,
    sample_rate: f64,
    reach: f64,
    shape: f64,
    pulse_width: f64,
    chirality: f64,
) -> [f64; 2] {
    let prep = PreparedOsc::new(
        waveform,
        reach,
        shape,
        WidthSpec::Pw(pulse_width),
        freq_hz,
        sample_rate,
        true,
    );
    prep.eval_stereo(phase, chirality)
}

/// Pitch-guarded mono Mid sample of a pure `1×` cubic/Hardy oscillator (`chirality = 0.0`).
#[inline]
pub fn oscillator_sample(
    waveform: Waveform,
    phase: f64,
    freq_hz: f64,
    sample_rate: f64,
    reach: f64,
    shape: f64,
    pulse_width: f64,
) -> f64 {
    cubic_oscillator_stereo(
        waveform,
        phase,
        freq_hz,
        sample_rate,
        reach,
        shape,
        pulse_width,
        0.0,
    )[0]
}

/// Exact un-guarded single-cycle real curve `re` in `f64` — single point of truth
/// shared by UI waveform plots ([`ideal_waveform`]).
#[inline]
pub fn ideal_waveform_f64(
    waveform: Waveform,
    phase: f64,
    reach: f64,
    shape: f64,
    pulse_width: f64,
) -> f64 {
    let prep = PreparedOsc::new(
        waveform,
        reach,
        shape,
        WidthSpec::Pw(pulse_width),
        0.0,
        48_000.0,
        false,
    );
    prep.eval_re(phase)
}

/// Exact un-guarded single-cycle real curve `re` in `f32` for UI waveform plots.
///
/// Re-exported by `crate::rack::ideal_waveform`.
#[inline]
pub fn ideal_waveform(
    waveform: Waveform,
    phase: f32,
    reach: f32,
    shape: f32,
    pulse_width: f32,
) -> f32 {
    ideal_waveform_f64(
        waveform,
        f64::from(phase),
        f64::from(reach),
        f64::from(shape),
        f64::from(pulse_width),
    ) as f32
}

/// Convert a detune offset in cents to a frequency multiplier (`2^(cents / 1200)`).
#[inline]
pub fn unison_detune_ratio(cents: f64) -> f64 {
    semitones_to_ratio_f64(sanitize_sample_f64(cents) / 100.0)
}

/// Loudness compensation for `count` uncorrelated detuned unison voices (`1 / sqrt(N)`),
/// preserving perceived RMS loudness across 1..=7 unison voices.
#[inline]
pub fn unison_gain_comp(count: usize) -> f64 {
    1.0 / (count.max(1) as f64).sqrt()
}

/// Per-voice detune ratio and pan data returned by [`unison_offsets`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnisonVoiceOffset {
    /// Frequency ratio (`2^(cents / 1200)`).
    pub detune_ratio: f64,
    /// Normalized stereo pan position in `[-1.0, 1.0]`.
    pub pan_pos: f32,
    /// Unity-centered equal-power stereo gains (`center == [1.0, 1.0]`, `L² + R² == 2.0`).
    pub pan: [f64; 2],
}

impl Default for UnisonVoiceOffset {
    fn default() -> Self {
        Self {
            detune_ratio: 1.0,
            pan_pos: 0.0,
            pan: [1.0, 1.0],
        }
    }
}

/// Compute symmetric detune ratios and unity-centered equal-power stereo pan weights
/// (`center == [1.0, 1.0]`, `L² + R² == 2.0`) for `count` unison voices (`1..=MAX_UNISON`).
/// Zero heap allocation.
pub fn unison_offsets(
    count: usize,
    detune_cents: f32,
    width: f32,
) -> [UnisonVoiceOffset; MAX_UNISON] {
    let n = count.clamp(1, MAX_UNISON);
    let detune = f64::from(if detune_cents.is_finite() {
        detune_cents
    } else {
        0.0
    });
    let w = if width.is_finite() {
        width.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let mut out = [UnisonVoiceOffset::default(); MAX_UNISON];
    for (i, slot) in out.iter_mut().take(n).enumerate() {
        let norm = if n == 1 {
            0.0
        } else {
            2.0 * i as f64 / (n - 1) as f64 - 1.0
        };
        let raw_pan = (norm as f32 * w).clamp(-1.0, 1.0);
        let pan_pos = if raw_pan == 0.0 { 0.0 } else { raw_pan };
        let pan = if pan_pos == 0.0 {
            [1.0, 1.0]
        } else {
            let [pl, pr] = pan_mono_equal_power_f64(f64::from(pan_pos));
            [pl * SQRT_2, pr * SQRT_2]
        };
        *slot = UnisonVoiceOffset {
            detune_ratio: unison_detune_ratio(norm * detune),
            pan_pos,
            pan,
        };
    }
    out
}

/// Map normalized `fm_amount ∈ [0.0, 1.0]` to linear FM modulation depth.
#[inline]
pub fn fm_depth_from_amount(fm_amount: f32) -> f64 {
    let f = f64::from(if fm_amount.is_finite() {
        fm_amount.clamp(0.0, 1.0)
    } else {
        0.0
    });
    f * f * 2.5
}

/// Parameters for one pure `1×`-sample-rate stereo frame of a cubic/Hardy VCO bank
/// (`1..=MAX_UNISON`) with automatic pitch-aware Nyquist reach guarding, M/S quadrature
/// chirality, and `1×` through-zero linear FM.
#[derive(Clone, Copy, Debug)]
pub struct VcoStepParams {
    pub waveform: Waveform,
    pub base_freq_hz: f64,
    pub sample_rate: f64,
    pub reach: f64,
    pub shape: f64,
    pub chirality: f64,
    pub pw: f64,
    /// `1×` through-zero linear FM instantaneous phase increment in cycles/sample.
    pub fm_cycles: f64,
    pub level: f64,
    pub voices: usize,
}

/// Evaluate one pure `1×`-rate stereo frame of a `1..=MAX_UNISON` cubic/Hardy VCO bank.
///
/// Supports signed instantaneous phase increments (`dt ∈ [-NYQUIST_SAFE_RATIO, NYQUIST_SAFE_RATIO]`)
/// for clean `1×` through-zero linear FM, automatically guards harmonic reach against
/// `|inst_hz|` per voice, and preserves exact M/S quadrature `[L, R]` when `pan == [1.0, 1.0]`.
#[inline]
pub fn step_vco_bank(
    params: VcoStepParams,
    phases: &mut [f64; MAX_UNISON],
    ratios: &[f64; MAX_UNISON],
    pan: &[[f64; 2]; MAX_UNISON],
) -> [f64; 2] {
    let voices = params.voices.clamp(1, MAX_UNISON);
    let sr = sanitize_sample_rate(params.sample_rate);
    let base_hz = sanitize_sample_f64(params.base_freq_hz).max(0.0);
    let fm_hz = sanitize_sample_f64(params.fm_cycles) * sr;
    let level = sanitize_sample_f64(params.level);
    let width = if matches!(params.waveform, Waveform::MobiusPwm) {
        let pw = sanitize_sample_f64(params.pw).clamp(0.02, 0.98);
        let (ws, wc) = (TAU * pw).sin_cos();
        WidthSpec::CosSin(wc, ws)
    } else {
        WidthSpec::Pw(params.pw)
    };

    if voices == 1 {
        let ratio = sanitize_sample_f64(ratios[0]).max(0.0);
        let inst_hz = base_hz * ratio + fm_hz;
        let dt = (inst_hz / sr).clamp(-NYQUIST_SAFE_RATIO, NYQUIST_SAFE_RATIO);
        let prep = PreparedOsc::new(
            params.waveform,
            params.reach,
            params.shape,
            width,
            inst_hz.abs(),
            sr,
            true,
        );
        let stereo = prep.eval_stereo(phases[0], params.chirality);
        phases[0] = phase_wrap(phases[0] + dt);
        return [stereo[0] * level * pan[0][0], stereo[1] * level * pan[0][1]];
    }

    let mut acc = [0.0_f64; 2];
    let mut cached_guard_bits = u64::MAX;
    let mut cached_prep = PreparedOsc::Sine;

    for i in 0..voices {
        let ratio = sanitize_sample_f64(ratios[i]).max(0.0);
        let inst_hz = base_hz * ratio + fm_hz;
        let dt = (inst_hz / sr).clamp(-NYQUIST_SAFE_RATIO, NYQUIST_SAFE_RATIO);
        let guard_hz = inst_hz.abs();
        let guard_bits = guard_hz.to_bits();
        if guard_bits != cached_guard_bits {
            cached_guard_bits = guard_bits;
            cached_prep = PreparedOsc::new(
                params.waveform,
                params.reach,
                params.shape,
                width,
                guard_hz,
                sr,
                true,
            );
        }
        let stereo = cached_prep.eval_stereo(phases[i], params.chirality);
        phases[i] = phase_wrap(phases[i] + dt);
        acc[0] += stereo[0] * level * pan[i][0];
        acc[1] += stereo[1] * level * pan[i][1];
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single_cycle_dft_mag(buf: &[f64], harmonic: usize) -> f64 {
        let n = buf.len() as f64;
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &x) in buf.iter().enumerate() {
            let ang = TAU * (harmonic as f64) * (i as f64) / n;
            re += x * ang.cos();
            im -= x * ang.sin();
        }
        2.0 * re.hypot(im) / n
    }

    fn windowed_dft_mag(buf: &[f64], target_hz: f64, sr: f64) -> f64 {
        let n = buf.len() as f64;
        let (mut re, mut im, mut wsum) = (0.0, 0.0, 0.0);
        for (i, &x) in buf.iter().enumerate() {
            let w = 0.5 - 0.5 * (TAU * i as f64 / n).cos();
            let ang = TAU * target_hz * i as f64 / sr;
            re += x * w * ang.cos();
            im -= x * w * ang.sin();
            wsum += w;
        }
        2.0 * re.hypot(im) / wsum
    }

    #[test]
    fn all_eight_waveforms_are_finite_zero_dc_and_properly_normalized() {
        let steps = 4096;
        let reach = 0.88;
        let shape = 0.5;
        let pw = 0.35;

        for &wf in Waveform::ALL {
            let mut sum_re = 0.0;
            let mut sum_l = 0.0;
            let mut sum_r = 0.0;
            let mut peak_re = 0.0_f64;
            let mut energy_l = 0.0;
            let mut energy_r = 0.0;

            for i in 0..steps {
                let phase = i as f64 / steps as f64;
                let ideal = ideal_waveform_f64(wf, phase, reach, shape, pw);
                let ideal_f32 =
                    ideal_waveform(wf, phase as f32, reach as f32, shape as f32, pw as f32);
                assert!(ideal.is_finite(), "{wf:?}: non-finite ideal sample");
                assert!(ideal_f32.is_finite(), "{wf:?}: non-finite f32 ideal sample");
                assert!((ideal - f64::from(ideal_f32)).abs() < 1e-5);

                let [l, r] =
                    cubic_oscillator_stereo(wf, phase, 0.0, 48_000.0, reach, shape, pw, 0.0);
                assert!(l.is_finite() && r.is_finite(), "{wf:?}: non-finite stereo");
                assert_eq!(l, r, "{wf:?}: chirality=0 must produce exact L == R");

                sum_re += ideal;
                peak_re = peak_re.max(ideal.abs());
                sum_l += l;
                sum_r += r;
                energy_l += l * l;
                energy_r += r * r;
            }

            let mean_re = (sum_re / steps as f64).abs();
            let mean_l = (sum_l / steps as f64).abs();
            let mean_r = (sum_r / steps as f64).abs();
            let rms_l = (energy_l / steps as f64).sqrt();
            let rms_r = (energy_r / steps as f64).sqrt();

            match wf {
                Waveform::MobiusJet | Waveform::BareCubic | Waveform::HardySaw | Waveform::Sine => {
                    assert!(mean_re < 1e-3, "{wf:?}: DC bias too large: {mean_re}");
                    assert!(
                        (peak_re - 1.0).abs() < 2e-3,
                        "{wf:?}: expected unit peak, got {peak_re}"
                    );
                }
                Waveform::MobiusPwm => {
                    assert!(mean_re < 1e-3, "MobiusPwm: DC bias too large: {mean_re}");
                    assert!(
                        peak_re > 0.5 && peak_re <= 1.0 + 1e-12,
                        "MobiusPwm: expected bounded peak <= 1.0, got {peak_re}"
                    );
                }
                Waveform::Szego | Waveform::Dipole => {
                    assert!(mean_l < 1e-3 && mean_r < 1e-3, "{wf:?}: DC bias too large");
                    assert!(
                        (rms_l - 1.0).abs() < 1e-3 && (rms_r - 1.0).abs() < 1e-3,
                        "{wf:?}: expected unit L² mono RMS, got L={rms_l} R={rms_r}"
                    );
                }
                Waveform::Poisson => {
                    // Poisson formant kernel has positive envelope mean; verify exact unit L² RMS.
                    assert!(
                        (rms_l - 1.0).abs() < 1e-3 && (rms_r - 1.0).abs() < 1e-3,
                        "Poisson: expected unit L² mono RMS, got L={rms_l} R={rms_r}"
                    );
                }
            }
        }

        // Verify MobiusPwm stays zero-DC across pulse widths and sample rates.
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            let freq = 200.0;
            let dt = freq / sr;
            let frames = (sr * 0.5) as usize;
            for pw in [0.15, 0.5, 0.85] {
                let mut phase = 0.0;
                let mut sum = 0.0;
                for _ in 0..frames {
                    sum += oscillator_sample(Waveform::MobiusPwm, phase, freq, sr, 0.92, 0.5, pw);
                    phase = phase_wrap(phase + dt);
                }
                let dc = (sum / frames as f64).abs();
                assert!(dc < 1e-3, "sr={sr} pw={pw}: MobiusPwm dc={dc}");
            }
        }
    }

    #[test]
    fn nyquist_reach_guard_pins_aliased_harmonics_below_minus_90_db_at_6khz() {
        let sr = 48_000.0_f64;
        let freq = 6_000.0_f64;
        let k_alias = (0.5 * sr / freq).floor() as usize + 1; // 5th harmonic (30 kHz)
        let n = 4096;

        // Every C^∞ waveform stays below -90 dB at the first aliased harmonic across shapes.
        let c_inf_waves = [
            Waveform::MobiusJet,
            Waveform::MobiusPwm,
            Waveform::HardySaw,
            Waveform::Sine,
            Waveform::Szego,
            Waveform::Dipole,
            Waveform::Poisson,
        ];

        for wf in c_inf_waves {
            for &shape in &[0.0_f64, 0.4, 0.5, 1.0] {
                let buf: Vec<f64> = (0..n)
                    .map(|i| {
                        let phase = i as f64 / n as f64;
                        oscillator_sample(wf, phase, freq, sr, VCO_MAX_REACH, shape, 0.35)
                    })
                    .collect();
                let inband_peak = (1..k_alias)
                    .map(|h| single_cycle_dft_mag(&buf, h))
                    .fold(0.0_f64, f64::max);
                let alias_mag = single_cycle_dft_mag(&buf, k_alias);
                let alias_db =
                    crate::dsp::lin_to_db_f64((alias_mag / inband_peak.max(1e-30)).max(1e-12));
                assert!(
                    alias_db < -90.0,
                    "{wf:?} shape={shape} @ 6 kHz/48 kHz: H{k_alias}/inband = {alias_db:+.1} dB (expected < -90 dB)"
                );
            }
        }

        // BareCubic at shape=0 (k=0) is C^∞ (<-90 dB); at k>0 it exhibits its bounded 1/n^3 tail.
        let bare_k0: Vec<f64> = (0..n)
            .map(|i| {
                oscillator_sample(
                    Waveform::BareCubic,
                    i as f64 / n as f64,
                    freq,
                    sr,
                    VCO_MAX_REACH,
                    0.0,
                    0.5,
                )
            })
            .collect();
        let bare_k0_db = crate::dsp::lin_to_db_f64(
            (single_cycle_dft_mag(&bare_k0, k_alias)
                / single_cycle_dft_mag(&bare_k0, 1).max(1e-30))
            .max(1e-12),
        );
        assert!(
            bare_k0_db < -90.0,
            "BareCubic k=0 @ 6 kHz/48 kHz: H{k_alias}/H1 = {bare_k0_db:+.1} dB"
        );
        let bare_k6: Vec<f64> = (0..n)
            .map(|i| {
                oscillator_sample(
                    Waveform::BareCubic,
                    i as f64 / n as f64,
                    freq,
                    sr,
                    VCO_MAX_REACH,
                    0.5,
                    0.5,
                )
            })
            .collect();
        let bare_k6_db = crate::dsp::lin_to_db_f64(
            (single_cycle_dft_mag(&bare_k6, k_alias)
                / single_cycle_dft_mag(&bare_k6, 1).max(1e-30))
            .max(1e-12),
        );
        assert!(
            bare_k6_db < -50.0,
            "BareCubic k=6 @ 6 kHz/48 kHz: H{k_alias}/H1 = {bare_k6_db:+.1} dB"
        );
    }

    #[test]
    fn chirality_preserves_constant_rms_and_exact_zero_comb_mono_sum() {
        let steps = 4096;
        let reach = 0.85;
        let shape = 0.5;
        let pw = 0.35;
        let sr = 48_000.0;
        let freq = 220.0;
        let chis = [-1.0_f64, -0.6, -0.25, 0.0, 0.25, 0.6, 1.0];

        for &wf in Waveform::ALL {
            let mut stereo_rms_list = Vec::with_capacity(chis.len());

            for &chi in &chis {
                let mut e_l = 0.0;
                let mut e_r = 0.0;
                let expected_scale = 1.0 / (1.0 + chi * chi).sqrt();

                for i in 0..steps {
                    let phase = i as f64 / steps as f64;
                    let [l, r] =
                        cubic_oscillator_stereo(wf, phase, freq, sr, reach, shape, pw, chi);
                    let [mono_l, _] =
                        cubic_oscillator_stereo(wf, phase, freq, sr, reach, shape, pw, 0.0);

                    // Exact zero-comb mono sum: Mid == 0.5 * (L + R) == mono / sqrt(1 + chi^2).
                    let mid = 0.5 * (l + r);
                    let expected_mid = mono_l * expected_scale;
                    assert!(
                        (mid - expected_mid).abs() < 1e-12,
                        "{wf:?} chi={chi} i={i}: mono collapse comb error mid={mid} vs {expected_mid}"
                    );

                    e_l += l * l;
                    e_r += r * r;
                }

                let rms_l = (e_l / steps as f64).sqrt();
                let rms_r = (e_r / steps as f64).sqrt();
                let stereo_rms = (0.5 * (e_l + e_r) / steps as f64).sqrt();
                stereo_rms_list.push(stereo_rms);

                // Analytic Hardy waves and Sine also keep individual L/R RMS exact across chirality.
                if matches!(wf, Waveform::Szego | Waveform::Dipole) {
                    assert!(
                        (rms_l - 1.0).abs() < 1e-3 && (rms_r - 1.0).abs() < 1e-3,
                        "{wf:?} chi={chi}: expected unit L/R RMS, got L={rms_l} R={rms_r}"
                    );
                }
            }

            let min_rms = stereo_rms_list
                .iter()
                .copied()
                .fold(f64::INFINITY, f64::min);
            let max_rms = stereo_rms_list.iter().copied().fold(0.0_f64, f64::max);
            let spread_db = crate::dsp::lin_to_db_f64(max_rms / min_rms);
            assert!(
                spread_db < 0.05,
                "{wf:?}: chirality stereo RMS spread = {spread_db:.5} dB (expected < 0.05 dB)"
            );
        }

        // Direct unit tests on ms_quadrature_encode cardinal points and NaN sanitization.
        assert_eq!(ms_quadrature_encode(0.75, -0.4, 0.0), [0.75, 0.75]);
        assert_eq!(
            ms_quadrature_encode(f64::NAN, f64::INFINITY, f64::NAN),
            [0.0, 0.0]
        );
        let [lp, rp] = ms_quadrature_encode_analytic(0.6, 0.3, 1.0);
        assert!((0.5 * (lp + rp) - 0.6).abs() < 1e-15);
        assert!((0.5 * (lp - rp) - 0.3).abs() < 1e-15);
        let [ln, rn] = ms_quadrature_encode_analytic(0.6, 0.3, -1.0);
        assert!((0.5 * (ln + rn) - 0.6).abs() < 1e-15);
        assert!((0.5 * (ln - rn) + 0.3).abs() < 1e-15);
    }

    #[test]
    fn through_zero_linear_fm_1x_produces_symmetric_bessel_sidebands_and_stays_clean() {
        let sr = 48_000.0;
        let fc = 3_000.0;
        let fm = 700.0;
        // Peak frequency deviation exceeds carrier (3_500 Hz > 3_000 Hz) so instantaneous
        // frequency reverses smoothly through zero (`beta = 3500 / 700 = 5.0`? Use `beta = 2.0`
        // for clean non-overlapping symmetric sidebands, plus a second TZ pass with `delta_f > fc`).
        let ratios = [1.0; MAX_UNISON];
        let pan = [[1.0, 1.0]; MAX_UNISON];

        // 1. Exact symmetric Bessel sidebands at fc +/- k*fm (k = 1, 2) with fc=3000, fm=700, delta_f=1400 (beta=2.0).
        let delta_f = 1_400.0;
        let measure = 16_384;
        let mut phases = [0.0; MAX_UNISON];
        let mut buf = Vec::with_capacity(measure);
        for n in 0..measure {
            let fm_hz = delta_f * (TAU * fm * n as f64 / sr).cos();
            let out = step_vco_bank(
                VcoStepParams {
                    waveform: Waveform::Sine,
                    base_freq_hz: fc,
                    sample_rate: sr,
                    reach: 0.9,
                    shape: 0.5,
                    chirality: 0.0,
                    pw: 0.5,
                    fm_cycles: fm_hz / sr,
                    level: 1.0,
                    voices: 1,
                },
                &mut phases,
                &ratios,
                &pan,
            );
            assert!(out[0].is_finite() && out[1].is_finite());
            buf.push(out[0]);
        }

        // At beta = 2.0: |J1(2.0)| ≈ 0.5767, |J2(2.0)| ≈ 0.3528.
        let s_p1 = windowed_dft_mag(&buf, fc + fm, sr);
        let s_m1 = windowed_dft_mag(&buf, fc - fm, sr);
        let s_p2 = windowed_dft_mag(&buf, fc + 2.0 * fm, sr);
        let s_m2 = windowed_dft_mag(&buf, fc - 2.0 * fm, sr);
        let sym1_db = crate::dsp::lin_to_db_f64(s_p1 / s_m1).abs();
        let sym2_db = crate::dsp::lin_to_db_f64(s_p2 / s_m2).abs();
        assert!(
            sym1_db < 0.05 && (s_p1 - 0.5767).abs() < 0.02,
            "1x FM k=±1 sidebands must be symmetric and match |J1(2)|≈0.577: +1={s_p1:.4}, -1={s_m1:.4}, diff={sym1_db:.3} dB"
        );
        assert!(
            sym2_db < 0.05 && (s_p2 - 0.3528).abs() < 0.02,
            "1x FM k=±2 sidebands must be symmetric and match |J2(2)|≈0.353: +2={s_p2:.4}, -2={s_m2:.4}, diff={sym2_db:.3} dB"
        );

        // 2. Deep through-zero phase reversal (`inst_hz` drops to -1_500 Hz) across all 8 waveforms
        //    and multi-voice unison across sample rates: zero NaNs, zero panics, bounded output.
        let offsets = unison_offsets(3, 14.0, 0.7);
        let mut u_ratios = [1.0; MAX_UNISON];
        let mut u_pan = [[1.0, 1.0]; MAX_UNISON];
        for i in 0..3 {
            u_ratios[i] = offsets[i].detune_ratio;
            u_pan[i] = offsets[i].pan;
        }
        for &wf in Waveform::ALL {
            for &sample_rate in &[44_100.0, 48_000.0, 96_000.0] {
                let mut u_phases = [0.0, 0.23, 0.61, 0.0, 0.0, 0.0, 0.0];
                let mut saw_reverse = false;
                for n in 0..1024 {
                    let mod_hz = 2_200.0 * (TAU * 910.0 * n as f64 / sample_rate).cos();
                    if 440.0 + mod_hz < 0.0 {
                        saw_reverse = true;
                    }
                    let out = step_vco_bank(
                        VcoStepParams {
                            waveform: wf,
                            base_freq_hz: 440.0,
                            sample_rate,
                            reach: 0.95,
                            shape: 0.5,
                            chirality: 0.6,
                            pw: 0.35,
                            fm_cycles: mod_hz / sample_rate,
                            level: unison_gain_comp(3),
                            voices: 3,
                        },
                        &mut u_phases,
                        &u_ratios,
                        &u_pan,
                    );
                    assert!(
                        out[0].is_finite() && out[1].is_finite(),
                        "{wf:?} sr={sample_rate} n={n}: non-finite TZ-FM output {out:?}"
                    );
                }
                assert!(
                    saw_reverse,
                    "test must exercise negative instantaneous frequency"
                );
            }
        }
    }

    #[test]
    fn lfo_shape_sample_matches_exact_cardinal_endpoints() {
        // Sine: 0 -> 0, 0.25 -> +1, 0.5 -> 0, 0.75 -> -1.
        assert!(lfo_shape_sample_f64(LfoShape::Sine, 0.0).abs() < 1e-12);
        assert!((lfo_shape_sample_f64(LfoShape::Sine, 0.25) - 1.0).abs() < 1e-12);
        assert!(lfo_shape_sample_f64(LfoShape::Sine, 0.5).abs() < 1e-12);
        assert!((lfo_shape_sample_f64(LfoShape::Sine, 0.75) + 1.0).abs() < 1e-12);

        // Triangle: click-free 0 -> 0, 0.25 -> +1, 0.5 -> 0, 0.75 -> -1.
        assert!(lfo_shape_sample_f64(LfoShape::Triangle, 0.0).abs() < 1e-12);
        assert!((lfo_shape_sample_f64(LfoShape::Triangle, 0.25) - 1.0).abs() < 1e-12);
        assert!(lfo_shape_sample_f64(LfoShape::Triangle, 0.5).abs() < 1e-12);
        assert!((lfo_shape_sample_f64(LfoShape::Triangle, 0.75) + 1.0).abs() < 1e-12);

        // Saw: 0 -> -1, 0.5 -> 0, 0.75 -> +0.5.
        assert!((lfo_shape_sample_f64(LfoShape::Saw, 0.0) + 1.0).abs() < 1e-12);
        assert!(lfo_shape_sample_f64(LfoShape::Saw, 0.5).abs() < 1e-12);
        assert!((lfo_shape_sample_f64(LfoShape::Saw, 0.75) - 0.5).abs() < 1e-12);

        // Square: +1 for p < 0.5, -1 for p >= 0.5.
        assert_eq!(lfo_shape_sample_f64(LfoShape::Square, 0.0), 1.0);
        assert_eq!(lfo_shape_sample_f64(LfoShape::Square, 0.499), 1.0);
        assert_eq!(lfo_shape_sample_f64(LfoShape::Square, 0.5), -1.0);
        assert_eq!(lfo_shape_sample_f64(LfoShape::Square, 0.999), -1.0);

        // SampleHold stateless placeholder is 0.0.
        assert_eq!(lfo_shape_sample_f64(LfoShape::SampleHold, 0.37), 0.0);

        // f32 wrapper matches f64 SPOT cleanly.
        for shape in [
            LfoShape::Sine,
            LfoShape::Triangle,
            LfoShape::Saw,
            LfoShape::Square,
            LfoShape::SampleHold,
        ] {
            for &p in &[0.0_f32, 0.25, 0.5, 0.75] {
                let got = lfo_shape_sample(shape, p);
                let expected = lfo_shape_sample_f64(shape, f64::from(p)) as f32;
                assert!((got - expected).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn unison_gain_comp_preserves_rms_loudness_for_five_detuned_voices() {
        let sr = 48_000.0;
        let base_hz = 220.0;
        let frames = 48_000;

        let measure_rms = |count: usize, detune: f32| -> f64 {
            let offsets = unison_offsets(count, detune, 0.0);
            let mut ratios = [1.0; MAX_UNISON];
            let mut pan = [[1.0, 1.0]; MAX_UNISON];
            for i in 0..count {
                ratios[i] = offsets[i].detune_ratio;
                pan[i] = offsets[i].pan;
            }
            let mut phases = [0.071, 0.223, 0.467, 0.823, 0.619, 0.0, 0.0];
            let params = VcoStepParams {
                waveform: Waveform::MobiusJet,
                base_freq_hz: base_hz,
                sample_rate: sr,
                reach: 0.92,
                shape: 0.5,
                chirality: 0.0,
                pw: 0.5,
                fm_cycles: 0.0,
                level: unison_gain_comp(count),
                voices: count,
            };
            let mut energy = 0.0;
            for _ in 0..frames {
                let out = step_vco_bank(params, &mut phases, &ratios, &pan);
                energy += out[0] * out[0];
            }
            (energy / frames as f64).sqrt()
        };

        let rms1 = measure_rms(1, 0.0);
        let rms5 = measure_rms(5, 18.0);
        let diff_db = crate::dsp::lin_to_db_f64(rms5 / rms1);
        assert!(
            diff_db.abs() < 1.0,
            "5 detuned unison voices should match 1-voice RMS within 1 dB: rms1={rms1:.4}, rms5={rms5:.4}, diff={diff_db:+.2} dB"
        );
    }

    #[test]
    fn unison_offsets_are_unity_centered_equal_power_and_match_single_voice_at_zero_width() {
        let def = UnisonVoiceOffset::default();
        assert_eq!(def.detune_ratio, 1.0);
        assert_eq!(def.pan_pos, 0.0);
        assert_eq!(def.pan, [1.0, 1.0]);

        for count in 1..=MAX_UNISON {
            for &width in &[0.0_f32, 0.25, 0.5, 0.75, 1.0] {
                let offsets = unison_offsets(count, 20.0, width);
                for (i, off) in offsets.iter().take(count).enumerate() {
                    assert!(
                        off.pan[0] >= 0.0 && off.pan[1] >= 0.0,
                        "count={count} width={width} i={i}: negative pan {:?}",
                        off.pan
                    );
                    let power = off.pan[0] * off.pan[0] + off.pan[1] * off.pan[1];
                    assert!(
                        (power - 2.0).abs() < 1e-12,
                        "count={count} width={width} i={i}: L^2 + R^2 = {power} != 2.0 (pan={:?})",
                        off.pan
                    );
                    if off.pan_pos == 0.0 {
                        assert_eq!(
                            off.pan,
                            [1.0, 1.0],
                            "count={count} width={width} i={i}: center pan must equal exact [1.0, 1.0]"
                        );
                    }
                    let mirror = offsets[count - 1 - i];
                    assert!((off.pan[0] - mirror.pan[1]).abs() < 1e-12);
                    assert!((off.pan[1] - mirror.pan[0]).abs() < 1e-12);
                }
                for slot in &offsets[count..] {
                    assert_eq!(*slot, def);
                }
                if width == 0.0 {
                    for off in &offsets[..count] {
                        assert_eq!(off.pan_pos, 0.0);
                        assert_eq!(off.pan, [1.0, 1.0]);
                    }
                } else if width == 1.0 && count >= 2 {
                    assert_eq!(offsets[0].pan_pos, -1.0);
                    assert_eq!(offsets[0].pan, [SQRT_2, 0.0]);
                    assert_eq!(offsets[count - 1].pan_pos, 1.0);
                    assert_eq!(offsets[count - 1].pan, [0.0, SQRT_2]);
                }
            }
        }

        // Non-finite inputs sanitize cleanly to unity center.
        let nan_offsets = unison_offsets(5, f32::NAN, f32::INFINITY);
        for off in &nan_offsets[..5] {
            assert_eq!(off.detune_ratio, 1.0);
            assert_eq!(off.pan_pos, 0.0);
            assert_eq!(off.pan, [1.0, 1.0]);
        }

        // Exact parity with voices == 1 when width == 0.0 and detune == 0.0 across chirality.
        let sr = 48_000.0;
        let base_hz = 330.0;
        let unit_ratios = [1.0; MAX_UNISON];
        let unit_pan = [[1.0, 1.0]; MAX_UNISON];

        for &chirality in &[0.0_f64, 0.75] {
            let single_params = VcoStepParams {
                waveform: Waveform::MobiusJet,
                base_freq_hz: base_hz,
                sample_rate: sr,
                reach: 0.92,
                shape: 0.5,
                chirality,
                pw: 0.5,
                fm_cycles: 0.0,
                level: 1.0,
                voices: 1,
            };

            for count in 2..=MAX_UNISON {
                let offsets = unison_offsets(count, 0.0, 0.0);
                let mut ratios = [1.0; MAX_UNISON];
                let mut pan = [[1.0, 1.0]; MAX_UNISON];
                for i in 0..count {
                    ratios[i] = offsets[i].detune_ratio;
                    pan[i] = offsets[i].pan;
                }
                let multi_params = VcoStepParams {
                    voices: count,
                    level: 1.0 / count as f64,
                    ..single_params
                };
                let mut p1 = [0.125; MAX_UNISON];
                let mut pm = [0.125; MAX_UNISON];
                for frame in 0..256 {
                    let y1 = step_vco_bank(single_params, &mut p1, &unit_ratios, &unit_pan);
                    let ym = step_vco_bank(multi_params, &mut pm, &ratios, &pan);
                    if chirality == 0.0 {
                        assert_eq!(
                            ym[0], ym[1],
                            "count={count} frame={frame}: width=0, chi=0 must keep L == R"
                        );
                    }
                    assert!(
                        (ym[0] - y1[0]).abs() < 1e-12 && (ym[1] - y1[1]).abs() < 1e-12,
                        "count={count} frame={frame} chi={chirality}: width=0 unison drifted from single voice: ym={ym:?} vs y1={y1:?}"
                    );
                }
            }
        }
    }
}
