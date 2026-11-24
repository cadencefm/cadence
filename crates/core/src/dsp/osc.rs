//! Bandlimited PolyBLEP/PolyBLAMP oscillators (with signed `dt` support for
//! through-zero FM), ideal UI waveform evaluation, constant-power unison helpers,
//! and 2× polyphase-halfband oversampled through-zero linear FM.

use crate::dsp::gain::pan_mono_equal_power;
use crate::dsp::math::{sanitize_sample_f64, semitones_to_ratio_f64};
use crate::dsp::saturate::Halfband2x;
use crate::rack::Waveform;
use std::f64::consts::TAU;

/// Maximum unison voices supported per oscillator in `VcoModule`.
pub const MAX_UNISON: usize = 7;

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

/// Polynomial Band-Limited Step (PolyBLEP) residual at wrapped phase `t ∈ [0, 1)`
/// for signed phase increment `dt` (supports reverse phase crossings during through-zero FM).
///
/// Because the ideal bandlimiting sinc kernel is even-symmetric in time, the step
/// residual depends on `|dt|` so both forward (`dt > 0`) and reverse (`dt < 0`)
/// traversals of the discontinuity receive exact width and polarity suppression.
#[inline]
pub fn poly_blep(t: f64, dt: f64) -> f64 {
    let adt = sanitize_sample_f64(dt).abs().min(0.49);
    if adt < 1e-12 {
        return 0.0;
    }
    if t < adt {
        let x = t / adt;
        2.0 * x - x * x - 1.0
    } else if t > 1.0 - adt {
        let x = (t - 1.0) / adt;
        x * x + 2.0 * x + 1.0
    } else {
        0.0
    }
}

/// Polynomial Band-Limited Ramp (PolyBLAMP) corner residual at wrapped phase `t ∈ [0, 1)`
/// for signed phase increment `dt` (triangle slope discontinuities, including reverse crossings).
#[inline]
pub fn poly_blamp(t: f64, dt: f64) -> f64 {
    let adt = sanitize_sample_f64(dt).abs().min(0.49);
    if adt < 1e-12 {
        return 0.0;
    }
    if t < adt {
        let x = 1.0 - t / adt;
        adt * x * x * x / 3.0
    } else if t > 1.0 - adt {
        let x = 1.0 + (t - 1.0) / adt;
        adt * x * x * x / 3.0
    } else {
        0.0
    }
}

/// Ideal (non-bandlimited) single-cycle oscillator sample in `f64` — single point
/// of truth shared by UI waveform plots ([`ideal_waveform`]) and LFO evaluation.
///
/// `Sine`, `Triangle`, and `Pulse` (`pw = 0.5`) share `sin(2πφ)` fundamental phase:
/// `Triangle` starts click-free at `0.0` (`phase = 0.0`), peaks at `+1.0` (`0.25`),
/// crosses `0.0` (`0.5`), and troughs at `-1.0` (`0.75`).
#[inline]
pub fn ideal_waveform_f64(waveform: Waveform, phase: f64, pulse_width: f64) -> f64 {
    let phase = phase_wrap(phase);
    match waveform {
        Waveform::Sine => (TAU * phase).sin(),
        Waveform::Triangle => {
            let t = phase_wrap(phase + 0.25);
            1.0 - 4.0 * (t - 0.5).abs()
        }
        Waveform::Saw => 2.0 * phase - 1.0,
        Waveform::Pulse => {
            let width = sanitize_sample_f64(pulse_width).clamp(0.02, 0.98);
            let bipolar = if phase < width { 1.0 } else { -1.0 };
            bipolar - (2.0 * width - 1.0)
        }
    }
}

/// Ideal (non-bandlimited) single-cycle oscillator sample for UI waveform plots.
///
/// Re-exported by `crate::rack::ideal_waveform`.
#[inline]
pub fn ideal_waveform(waveform: Waveform, phase: f32, pulse_width: f32) -> f32 {
    ideal_waveform_f64(waveform, f64::from(phase), f64::from(pulse_width)) as f32
}

/// Bandlimited oscillator sample with PolyBLEP/PolyBLAMP alias suppression and
/// exact DC compensation on variable-width Pulse waves. Supports signed `dt`.
#[inline]
pub fn oscillator_sample(waveform: Waveform, phase: f64, dt: f64, pulse_width: f64) -> f64 {
    let phase = phase_wrap(phase);
    match waveform {
        Waveform::Sine => (TAU * phase).sin(),
        Waveform::Saw => 2.0 * phase - 1.0 - poly_blep(phase, dt),
        Waveform::Pulse => {
            let pw = sanitize_sample_f64(pulse_width).clamp(0.02, 0.98);
            (if phase < pw { 1.0 } else { -1.0 }) - (2.0 * pw - 1.0) + poly_blep(phase, dt)
                - poly_blep(phase_wrap(phase - pw), dt)
        }
        Waveform::Triangle => {
            let t = phase_wrap(phase + 0.25);
            1.0 - 4.0 * (t - 0.5).abs() + 4.0 * poly_blamp(t, dt)
                - 4.0 * poly_blamp(phase_wrap(t + 0.5), dt)
        }
    }
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
    /// Constant-power equal-power stereo gains (`L² + R² == 1.0`).
    pub pan: [f32; 2],
}

impl Default for UnisonVoiceOffset {
    fn default() -> Self {
        Self {
            detune_ratio: 1.0,
            pan_pos: 0.0,
            pan: pan_mono_equal_power(0.0),
        }
    }
}

/// Compute symmetric detune ratios and constant-power stereo pan weights for
/// `count` unison voices (`1..=MAX_UNISON`). Zero heap allocation.
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
        let pan_pos = (norm as f32 * w).clamp(-1.0, 1.0);
        *slot = UnisonVoiceOffset {
            detune_ratio: unison_detune_ratio(norm * detune),
            pan_pos,
            pan: pan_mono_equal_power(pan_pos),
        };
    }
    out
}

/// Map normalized `osc_b_fm ∈ [0.0, 1.0]` to linear phase-modulation depth in cycles.
#[inline]
pub fn fm_depth_from_amount(fm_amount: f32) -> f64 {
    let f = f64::from(if fm_amount.is_finite() {
        fm_amount.clamp(0.0, 1.0)
    } else {
        0.0
    });
    f * f * 2.5
}

/// Parameters for one base-rate stereo frame of a PolyBLEP/PolyBLAMP VCO bank (`1..=MAX_UNISON`).
#[derive(Clone, Copy, Debug)]
pub struct VcoStepParams {
    pub waveform: Waveform,
    pub unison_count: usize,
    pub base_hz: f64,
    pub sample_rate: f64,
    pub pw: f64,
    pub level: f64,
}

/// Evaluate one stereo frame of a `1..=MAX_UNISON` VCO bank directly at base rate
/// (zero delay, zero oversampling overhead when `FmIn` is unpatched).
#[inline]
pub fn step_vco_bank(
    params: VcoStepParams,
    phases: &mut [f64; MAX_UNISON],
    unison_ratios: &[f64; MAX_UNISON],
    unison_pan: &[[f64; 2]; MAX_UNISON],
) -> [f64; 2] {
    let count = params.unison_count.clamp(1, MAX_UNISON);
    let nyq_cap = params.sample_rate * 0.45;
    if count == 1 {
        let dt = params.base_hz.min(nyq_cap) / params.sample_rate;
        let val = oscillator_sample(params.waveform, phases[0], dt, params.pw) * params.level;
        phases[0] = phase_wrap(phases[0] + dt);
        return [val, val];
    }
    let mut acc = [0.0_f64; 2];
    for copy in 0..count {
        let dt = (params.base_hz * unison_ratios[copy]).min(nyq_cap) / params.sample_rate;
        let val = oscillator_sample(params.waveform, phases[copy], dt, params.pw) * params.level;
        phases[copy] = phase_wrap(phases[copy] + dt);
        acc[0] += val * unison_pan[copy][0];
        acc[1] += val * unison_pan[copy][1];
    }
    acc
}

/// Evaluate one stereo frame of a `1..=MAX_UNISON` VCO bank with 2× polyphase-halfband
/// oversampled through-zero linear phase modulation (`FmIn` patched).
///
/// Upsamples `fm_cycles` cleanly via [`Halfband2x::upsample_mono`], evaluates every active
/// unison copy with exact signed instantaneous phase increment `half_dt + Δfm` at both
/// chronological half-steps (`even` at `t`, `odd` at `t + 0.5`), and decimates the stereo
/// sum via [`Halfband2x::downsample_stereo`] (`< 0.01 dB` passband ripple to `19 kHz`,
/// `> 40 dB` foldback sideband suppression).
#[inline]
pub fn step_vco_bank_fm_2x(
    params: VcoStepParams,
    os: &mut Halfband2x,
    phases: &mut [f64; MAX_UNISON],
    prev_fm_2x: &mut f64,
    unison_ratios: &[f64; MAX_UNISON],
    unison_pan: &[[f64; 2]; MAX_UNISON],
    fm_cycles: f64,
) -> [f64; 2] {
    let count = params.unison_count.clamp(1, MAX_UNISON);
    let nyq_cap = params.sample_rate * 0.45;
    let (fm_even, fm_odd) = os.upsample_mono(fm_cycles);
    let dfm0 = fm_even - *prev_fm_2x;
    let dfm1 = fm_odd - fm_even;
    *prev_fm_2x = fm_odd;

    let (even_out, odd_out) = if count == 1 {
        let half_dt = (params.base_hz.min(nyq_cap) / params.sample_rate) * 0.5;
        let s0 = oscillator_sample(
            params.waveform,
            phase_wrap(phases[0] + fm_even),
            half_dt + dfm0,
            params.pw,
        ) * params.level;
        phases[0] = phase_wrap(phases[0] + half_dt);
        let s1 = oscillator_sample(
            params.waveform,
            phase_wrap(phases[0] + fm_odd),
            half_dt + dfm1,
            params.pw,
        ) * params.level;
        phases[0] = phase_wrap(phases[0] + half_dt);
        ([s0, s0], [s1, s1])
    } else {
        let mut acc0 = [0.0_f64; 2];
        let mut acc1 = [0.0_f64; 2];
        for copy in 0..count {
            let half_dt =
                ((params.base_hz * unison_ratios[copy]).min(nyq_cap) / params.sample_rate) * 0.5;
            let s0 = oscillator_sample(
                params.waveform,
                phase_wrap(phases[copy] + fm_even),
                half_dt + dfm0,
                params.pw,
            ) * params.level;
            phases[copy] = phase_wrap(phases[copy] + half_dt);
            let s1 = oscillator_sample(
                params.waveform,
                phase_wrap(phases[copy] + fm_odd),
                half_dt + dfm1,
                params.pw,
            ) * params.level;
            phases[copy] = phase_wrap(phases[copy] + half_dt);
            let pan = unison_pan[copy];
            acc0[0] += s0 * pan[0];
            acc0[1] += s0 * pan[1];
            acc1[0] += s1 * pan[0];
            acc1[1] += s1 * pan[1];
        }
        (acc0, acc1)
    };
    os.downsample_stereo(even_out, odd_out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poly_blep_and_blamp_handle_signed_dt_symmetrically() {
        for &t in &[0.001, 0.01, 0.5, 0.99, 0.999] {
            let dt = 0.02;
            assert_eq!(poly_blep(t, dt), poly_blep(t, -dt));
            assert_eq!(poly_blamp(t, dt), poly_blamp(t, -dt));
        }
        assert_eq!(poly_blep(0.5, 0.01), 0.0);
        assert_eq!(poly_blep(0.005, 0.0), 0.0);
        assert!(poly_blep(0.005, -0.02) < 0.0);
        assert!(poly_blep(0.995, -0.02) > 0.0);
    }

    #[test]
    fn pulse_oscillator_is_dc_compensated_across_pulse_widths() {
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            let freq = 200.0;
            let dt = freq / sr;
            let frames = (sr * 0.5) as usize; // exact integer cycles at 200 Hz
            for pw in [0.15, 0.5, 0.85] {
                let mut phase = 0.0;
                let mut sum = 0.0;
                for _ in 0..frames {
                    sum += oscillator_sample(Waveform::Pulse, phase, dt, pw);
                    phase = phase_wrap(phase + dt);
                }
                let dc = (sum / frames as f64).abs();
                assert!(dc < 0.01, "sr={sr} pw={pw}: dc={dc}");
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
            let gain = unison_gain_comp(count);
            let mut phases = [0.071, 0.223, 0.467, 0.823, 0.619];
            let mut energy = 0.0;
            for _ in 0..frames {
                let mut s = 0.0;
                for (i, off) in offsets.iter().take(count).enumerate() {
                    let dt = (base_hz * off.detune_ratio) / sr;
                    s += oscillator_sample(Waveform::Saw, phases[i], dt, 0.5) * gain;
                    phases[i] = phase_wrap(phases[i] + dt);
                }
                energy += s * s;
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
    fn triangle_aligns_with_sine_zero_crossing_phase_and_rounds_corners() {
        let dt = 0.01;
        // Ideal Triangle matches sin(2πφ) phase: 0°→0, 90°→+1, 180°→0, 270°→-1.
        assert!(ideal_waveform_f64(Waveform::Triangle, 0.0, 0.5).abs() < 1e-12);
        assert!((ideal_waveform_f64(Waveform::Triangle, 0.25, 0.5) - 1.0).abs() < 1e-12);
        assert!(ideal_waveform_f64(Waveform::Triangle, 0.5, 0.5).abs() < 1e-12);
        assert!((ideal_waveform_f64(Waveform::Triangle, 0.75, 0.5) + 1.0).abs() < 1e-12);

        // Bandlimited Triangle is click-free at zero crossings and PolyBLAMP-rounded at peaks/troughs.
        assert!(oscillator_sample(Waveform::Triangle, 0.0, dt, 0.5).abs() < 1e-12);
        assert!(oscillator_sample(Waveform::Triangle, 0.5, dt, 0.5).abs() < 1e-12);
        let peak = oscillator_sample(Waveform::Triangle, 0.25, dt, 0.5);
        let trough = oscillator_sample(Waveform::Triangle, 0.75, dt, 0.5);
        assert!(
            peak < 1.0 && peak > 0.98,
            "expected rounded peak, got {peak}"
        );
        assert!(
            trough > -1.0 && trough < -0.98,
            "expected rounded trough, got {trough}"
        );
        assert!((peak + trough).abs() < 1e-12);
    }

    #[test]
    fn through_zero_fm_2x_halfband_preserves_high_sidebands_and_crushes_foldback() {
        // 1. Bounded & finite across sample rates, discontinuous Saw carrier, and multi-voice unison.
        let offsets = unison_offsets(3, 12.0, 0.5);
        let mut ratios = [1.0; MAX_UNISON];
        let mut pan = [[1.0, 1.0]; MAX_UNISON];
        for i in 0..3 {
            ratios[i] = offsets[i].detune_ratio;
            pan[i] = [f64::from(offsets[i].pan[0]), f64::from(offsets[i].pan[1])];
        }
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            let params = VcoStepParams {
                waveform: Waveform::Saw,
                unison_count: 3,
                base_hz: 440.0,
                sample_rate: sr,
                pw: 0.5,
                level: unison_gain_comp(3),
            };
            let mut os = Halfband2x::new();
            let mut phases = [0.0; MAX_UNISON];
            let mut prev_fm = 0.0;
            let depth = fm_depth_from_amount(0.85);
            for i in 0..2048 {
                let mod_sig = oscillator_sample(
                    Waveform::Pulse,
                    phase_wrap(i as f64 * 880.0 / sr),
                    880.0 / sr,
                    0.35,
                );
                let out = step_vco_bank_fm_2x(
                    params,
                    &mut os,
                    &mut phases,
                    &mut prev_fm,
                    &ratios,
                    &pan,
                    mod_sig * depth,
                );
                assert!(
                    out[0].is_finite() && out[0].abs() <= 2.5,
                    "sr={sr} i={i}: out[0]={}",
                    out[0]
                );
                assert!(
                    out[1].is_finite() && out[1].abs() <= 2.5,
                    "sr={sr} i={i}: out[1]={}",
                    out[1]
                );
            }
        }

        // 2. Analytical passband flatness + >35 dB foldback rejection vs 1x FM:
        //    6000 Hz Sine carrier modulated by 3700 Hz Sine at beta = 0.45 cycles (2.8274 rad):
        //    - True k=+3 passband sideband at 17100 Hz must match 1x within < 0.05 dB (boxcar droops -1.7 dB!).
        //    - Folded k=+7 alias (6000 + 7*3700 = 31900 -> 16100 Hz) must be crushed > 35 dB vs 1x!
        let sr = 48_000.0;
        let fc = 6_000.0;
        let fm = 3_700.0;
        let beta_cycles = 0.45;
        let warmup = 2_048;
        let measure = 8_192;
        let params = VcoStepParams {
            waveform: Waveform::Sine,
            unison_count: 1,
            base_hz: fc,
            sample_rate: sr,
            pw: 0.5,
            level: 1.0,
        };
        let mut os = Halfband2x::new();
        let mut phases_hb = [0.0; MAX_UNISON];
        let mut prev_fm = 0.0;
        let mut phase_1x = 0.0;
        let mut buf_hb = Vec::with_capacity(measure);
        let mut buf_1x = Vec::with_capacity(measure);
        for n in 0..(warmup + measure) {
            let mod_cycles = beta_cycles * (TAU * fm * n as f64 / sr).sin();
            let y_hb = step_vco_bank_fm_2x(
                params,
                &mut os,
                &mut phases_hb,
                &mut prev_fm,
                &ratios,
                &pan,
                mod_cycles,
            )[0];
            let y_1x = (TAU * (phase_1x + mod_cycles)).sin();
            phase_1x = phase_wrap(phase_1x + fc / sr);
            if n >= warmup {
                buf_hb.push(y_hb);
                buf_1x.push(y_1x);
            }
        }
        let dft_mag = |buf: &[f64], target_hz: f64| -> f64 {
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
        };
        let true_hb = dft_mag(&buf_hb, 17_100.0);
        let true_1x = dft_mag(&buf_1x, 17_100.0);
        let passband_err_db = crate::dsp::lin_to_db_f64(true_hb / true_1x);
        assert!(
            passband_err_db.abs() < 0.05,
            "2x Halfband FM must preserve 17.1 kHz sideband flatly: err={passband_err_db:+.4} dB"
        );
        let alias_hb = dft_mag(&buf_hb, 16_100.0);
        let alias_1x = dft_mag(&buf_1x, 16_100.0);
        let reject_db = crate::dsp::lin_to_db_f64((alias_hb + 1e-12) / alias_1x);
        assert!(
            reject_db < -35.0,
            "2x Halfband FM must crush 16.1 kHz foldback alias by >35 dB vs 1x: {reject_db:+.1} dB (hb={alias_hb:.6}, 1x={alias_1x:.6})"
        );
    }
}
