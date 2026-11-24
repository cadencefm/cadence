//! Single-source-of-truth pan laws, Mid/Side & mono summation helpers, channel fader
//! headroom/taper, dB nudging, dB formatting, and channel strip gain staging.
//!
//! Stereo routing across Cadence follows one coherent three-tier pan contract:
//! - **Intra-patch unison panning** ([`crate::dsp::osc::unison_offsets`]): unity-centered
//!   equal-power (`pan_mono_equal_power_f64(p) * SQRT_2`, center `[1.0, 1.0]`,
//!   `L² + R² == 2.0`) matching single-voice mono wires `[x, x]` with zero gain jump when
//!   changing `unison_voices` between `1` and `2..=7`.
//! - **Voice / zone output panning** ([`pan_mono_equal_power`]): constant-power
//!   (`-3.01 dB` center, `L² + R² == 1.0`, peak bounded in `[0.0, 1.0]`) at `OutNode`,
//!   preserving constant acoustic stereo power across `-1.0..=1.0` for both mono and
//!   uncorrelated stereo voices.
//! - **Stereo bus & console strip balance** ([`pan_stereo_balance`]): unity-normalized
//!   equal-power stereo balance (`tan((1 ∓ p) * π/4)`) preserving centered stereo fields
//!   at exact `[1.0, 1.0]` unity while matching the exact `L/R` ratio of an analog `-3 dB`
//!   constant-power console pan pot everywhere across `-1.0..=1.0`, used for `Rack` bus
//!   panning and all mixer strips so nested Audio -> Group -> Master chains never compound
//!   `-3 dB` center attenuation.
//!
//! Voltage-average Mid/Side and mono downmix helpers ([`stereo_mid_f64`],
//! [`stereo_mid_f32`], [`stereo_ms_encode_f64`], [`stereo_ms_decode_f64`]) preserve exact
//! unity (`[x, x] -> x`) for mono CV/FM modulation, oscilloscope display, and mono DAC
//! downmixes paired with unity-centered stereo buses.

use crate::dsp::math::{
    db_to_lin, lin_to_db, sanitize_clamp_f64, sanitize_sample, sanitize_sample_f64, LIN_FLOOR_F32,
    SMOOTH_SETTLE_EPS_F64,
};
use std::f64::consts::FRAC_PI_4;

/// Upper fader stepping headroom in decibels (`+6.0 dB` studio console boost).
pub const MAX_CHANNEL_GAIN_DB: f32 = 6.0;

/// Maximum linear channel strip gain (`2.0` = `+6.02 dB`), cleanly covering
/// `db_to_lin(6.0)` (`1.99526`) as well as exact `2.0` / `200%` / `+6 dB`.
pub const MAX_CHANNEL_GAIN: f32 = 2.0;

/// Normalized `0.0..=1.0` fader position corresponding to exact unity (`1.0` / `0.0 dB`),
/// matching a studio console 3/4-throw unity detent.
pub const CHANNEL_GAIN_UNITY_NORM: f32 = 0.75;

/// Power-taper exponent `p = ln(2.0) / ln(4.0 / 3.0)` so `MAX_CHANNEL_GAIN * 0.75^p == 1.0`.
const CHANNEL_GAIN_TAPER_EXP: f64 = 2.409_420_839_653_209;

/// Map a normalized `0.0..=1.0` control position to linear channel strip gain
/// `0.0..=MAX_CHANNEL_GAIN` (`2.0`).
///
/// Exact smooth console power taper (`C∞` on `(0, 1]`):
/// - `0.0` -> `0.0` (`-inf dB`)
/// - `0.75` -> `1.0` (`0.0 dB` exact unity detent)
/// - `1.0` -> `2.0` (`+6.02 dB` exact maximum headroom)
#[inline]
pub fn norm_to_channel_gain(norm: f32) -> f32 {
    if !norm.is_finite() || norm <= 0.0 {
        return 0.0;
    }
    if norm >= 1.0 {
        return MAX_CHANNEL_GAIN;
    }
    let gain = f64::from(MAX_CHANNEL_GAIN) * f64::from(norm).powf(CHANNEL_GAIN_TAPER_EXP);
    (gain as f32).clamp(0.0, MAX_CHANNEL_GAIN)
}

/// Map a linear channel strip gain `0.0..=MAX_CHANNEL_GAIN` (`2.0`) back to normalized `0.0..=1.0`.
///
/// Exact inverse of [`norm_to_channel_gain`][]: `0.0 -> 0.0`, `1.0 -> 0.75`, `2.0 -> 1.0`.
#[inline]
pub fn channel_gain_to_norm(gain: f32) -> f32 {
    if !gain.is_finite() || gain <= 0.0 {
        return 0.0;
    }
    if gain >= MAX_CHANNEL_GAIN {
        return 1.0;
    }
    let norm = (f64::from(gain) / f64::from(MAX_CHANNEL_GAIN)).powf(1.0 / CHANNEL_GAIN_TAPER_EXP);
    (norm as f32).clamp(0.0, 1.0)
}

/// Step a linear channel strip gain by `delta_db` decibels within `-inf` / `-60..=+6.0 dB`.
///
/// Single point of truth composed by both Mixer deck `j`/`k` and `Detail::Channel` nudging:
/// - Preserves `-60 dB` floor stepping (`<= 0.001000001` or `< -60 dB` snaps down to `0.0`).
/// - Stepping up from `0.0` by `delta_db > 0` enters at `-60.0 + (delta_db - 1.0).max(0.0)` dB.
/// - Snaps `|db| < 1e-4` to exact `1.0` (`0.0 dB` unity).
/// - Clamps upper bound to [`MAX_CHANNEL_GAIN_DB`] (`+6.0 dB`) / [`MAX_CHANNEL_GAIN`].
#[inline]
pub fn nudge_channel_gain_db(gain: f32, delta_db: f32) -> f32 {
    let gain = sanitize_sample(gain).clamp(0.0, MAX_CHANNEL_GAIN);
    if !delta_db.is_finite() || delta_db == 0.0 {
        return gain;
    }
    if delta_db > 0.0 {
        let db = if gain <= 0.0 {
            -60.0 + (delta_db - 1.0).max(0.0)
        } else {
            lin_to_db(gain).max(-60.0) + delta_db
        };
        let clamped_db = db.min(MAX_CHANNEL_GAIN_DB);
        if clamped_db.abs() < 1e-4 {
            1.0
        } else {
            db_to_lin(clamped_db).clamp(0.0, MAX_CHANNEL_GAIN)
        }
    } else if gain <= 0.001_000_001 {
        0.0
    } else {
        let db = lin_to_db(gain) + delta_db;
        if db < -60.0 {
            0.0
        } else if db.abs() < 1e-4 {
            1.0
        } else {
            db_to_lin(db.min(MAX_CHANNEL_GAIN_DB)).clamp(0.0, MAX_CHANNEL_GAIN)
        }
    }
}

/// Format a linear amplitude/gain in decibels without unit suffix (`"-inf"`, `"+0.0"`, `"-6.0"`, `"+4.0"`).
///
/// Returns `"-inf"` when non-finite or `<= LIN_FLOOR_F32`, and normalizes `"-0.0"` to `"+0.0"`.
pub fn format_db_value(gain: f32) -> String {
    if !gain.is_finite() || gain <= LIN_FLOOR_F32 {
        "-inf".into()
    } else {
        let formatted = format!("{:+.1}", lin_to_db(gain));
        if formatted == "-0.0" {
            "+0.0".into()
        } else {
            formatted
        }
    }
}

/// Format a linear channel gain with `" dB"` suffix (`"-inf dB"`, `"+0.0 dB"`, `"+6.0 dB"`).
pub fn format_gain_db(gain: f32) -> String {
    format!("{} dB", format_db_value(gain))
}

/// Constant-power mono panner (`-3.01 dB` center, `L² + R² == 1.0` across `-1.0..=1.0`).
///
/// `pan = -1.0` -> `[1.0, 0.0]`, `pan = 0.0` -> `[0.7071, 0.7071]`, `pan = +1.0` -> `[0.0, 1.0]`.
#[inline]
pub fn pan_mono_equal_power(pan: f32) -> [f32; 2] {
    let [l, r] = pan_mono_equal_power_f64(f64::from(pan));
    [l as f32, r as f32]
}

/// Constant-power mono panner in `f64` (`L² + R² == 1.0` across `-1.0..=1.0`).
#[inline]
pub fn pan_mono_equal_power_f64(pan: f64) -> [f64; 2] {
    let p = sanitize_sample_f64(pan).clamp(-1.0, 1.0);
    if p <= -1.0 {
        return [1.0, 0.0];
    }
    if p >= 1.0 {
        return [0.0, 1.0];
    }
    let theta = (p + 1.0) * FRAC_PI_4;
    [theta.cos(), theta.sin()]
}

/// Unity-normalized equal-power stereo balance panner preserving centered stereo buses at `[1.0, 1.0]`.
///
/// - `pan = 0.0`: `[1.0, 1.0]` (exact unity pass-through for centered stereo buses).
/// - `pan > 0.0`: `[tan((1 - pan) * pi/4), 1.0]` (matches `cos(theta) / sin(theta)` of [`pan_mono_equal_power`]).
/// - `pan < 0.0`: `[1.0, tan((1 + pan) * pi/4)]` (matches `sin(theta) / cos(theta)` of [`pan_mono_equal_power`]).
/// - `pan <= -1.0`: `[1.0, 0.0]`, `pan >= +1.0`: `[0.0, 1.0]`.
///
/// Strictly bounded in `[0.0, 1.0]` (never boosts an already-full-scale stereo bus) while
/// delivering crisp, analog-console stereo positioning (`-3.5 dB` at `p = 0.25`, `-6.0 dB`
/// at `p = 0.41`, `-7.66 dB` at `p = 0.50`).
#[inline]
pub fn pan_stereo_balance(pan: f32) -> [f32; 2] {
    let [l, r] = pan_stereo_balance_f64(f64::from(pan));
    [l as f32, r as f32]
}

/// Unity-normalized equal-power stereo balance panner in `f64`.
#[inline]
pub fn pan_stereo_balance_f64(pan: f64) -> [f64; 2] {
    let p = sanitize_sample_f64(pan).clamp(-1.0, 1.0);
    if p <= -1.0 {
        return [1.0, 0.0];
    }
    if p >= 1.0 {
        return [0.0, 1.0];
    }
    if p > 0.0 {
        [((1.0 - p) * FRAC_PI_4).tan(), 1.0]
    } else if p < 0.0 {
        [1.0, ((1.0 + p) * FRAC_PI_4).tan()]
    } else {
        [1.0, 1.0]
    }
}

/// Apply post-fader linear channel gain (`gain * scale * duck`) without strip panning.
/// Used to tap standard console post-fader, pre-pan stereo aux sends.
#[inline]
pub fn apply_fader_gain(input: [f32; 2], gain: f32, scale: f32, duck: f32) -> [f32; 2] {
    let g = sanitize_sample(gain) * sanitize_sample(scale) * sanitize_sample(duck);
    [sanitize_sample(input[0]) * g, sanitize_sample(input[1]) * g]
}

/// Apply stereo strip balance panning ([`pan_stereo_balance`]) to a frame.
#[inline]
pub fn apply_pan_balance(input: [f32; 2], pan: f32) -> [f32; 2] {
    let [pl, pr] = pan_stereo_balance(pan);
    [
        sanitize_sample(input[0]) * pl,
        sanitize_sample(input[1]) * pr,
    ]
}

/// Apply combined channel strip gain (`gain * scale * duck`) and stereo balance pan.
#[inline]
pub fn apply_strip_gain(input: [f32; 2], gain: f32, scale: f32, duck: f32, pan: f32) -> [f32; 2] {
    apply_pan_balance(apply_fader_gain(input, gain, scale, duck), pan)
}

/// Voltage-average stereo-to-mono Mid sum (`0.5 * (L + R)`) in `f64`.
///
/// Preserves exact unity for correlated mono/CV signals (`[v, v] -> v`) and sanitizes
/// non-finite inputs to `0.0`. Used across modular CV/FM mono inputs and M/S encoding.
#[inline]
pub fn stereo_mid_f64(stereo: [f64; 2]) -> f64 {
    0.5 * (sanitize_sample_f64(stereo[0]) + sanitize_sample_f64(stereo[1]))
}

/// Voltage-average stereo-to-mono Mid sum (`0.5 * (L + R)`) in `f32`.
///
/// Preserves exact unity for centered stereo buses (`[1.0, 1.0] -> 1.0`) without clipping
/// mono DACs, oscilloscopes, or mono waveform taps.
#[inline]
pub fn stereo_mid_f32(stereo: [f32; 2]) -> f32 {
    0.5 * (sanitize_sample(stereo[0]) + sanitize_sample(stereo[1]))
}

/// Encode stereo `[L, R]` into voltage-average Mid/Side `[mid, side]` (`f64`).
///
/// Exact inverse of [`stereo_ms_decode_f64`]: `mid = 0.5 * (L + R)`, `side = 0.5 * (L - R)`.
#[inline]
pub fn stereo_ms_encode_f64(stereo: [f64; 2]) -> [f64; 2] {
    let l = sanitize_sample_f64(stereo[0]);
    let r = sanitize_sample_f64(stereo[1]);
    [0.5 * (l + r), 0.5 * (l - r)]
}

/// Decode Mid/Side `(mid, side)` back into stereo `[mid + side, mid - side]` (`f64`).
///
/// Exact inverse of [`stereo_ms_encode_f64`].
#[inline]
pub fn stereo_ms_decode_f64(mid: f64, side: f64) -> [f64; 2] {
    let m = sanitize_sample_f64(mid);
    let s = sanitize_sample_f64(side);
    [m + s, m - s]
}

/// Equal-power Mid/Side stereo width control for uncorrelated stereo fields (`f64`).
///
/// Scales the Side channel by `width ∈ [0.0, 2.0]` and applies the exact uncorrelated
/// RMS normalization `sqrt(2 / (1 + width²))`:
/// - `width = 1.0` -> exact unity pass-through `[L, R]` (`0.00 dB`).
/// - `width = 0.0` -> equal-power mono sum `[(L + R) / sqrt(2), (L + R) / sqrt(2)]` (`0.00 dB` RMS).
/// - All `width ∈ [0.0, 2.0]` preserve `Var(out_L) == Var(out_R) == σ²` for uncorrelated
///   stereo sources such as FDN reverb outputs.
#[inline]
pub fn stereo_ms_width_uncorrelated_f64(stereo: [f64; 2], width: f64) -> [f64; 2] {
    let l = sanitize_sample_f64(stereo[0]);
    let r = sanitize_sample_f64(stereo[1]);
    let w = sanitize_clamp_f64(width, 0.0, 2.0, 1.0);
    if (w - 1.0).abs() <= SMOOTH_SETTLE_EPS_F64 {
        return [l, r];
    }
    let [mid, raw_side] = stereo_ms_encode_f64([l, r]);
    let norm = (2.0 / (1.0 + w * w)).sqrt();
    let [out_l, out_r] = stereo_ms_decode_f64(mid, raw_side * w);
    [out_l * norm, out_r * norm]
}

/// Equal-power Mid/Side stereo width control for uncorrelated stereo fields (`f32`).
#[inline]
pub fn stereo_ms_width_uncorrelated(stereo: [f32; 2], width: f32) -> [f32; 2] {
    let [l, r] = stereo_ms_width_uncorrelated_f64(
        [f64::from(stereo[0]), f64::from(stereo[1])],
        f64::from(width),
    );
    [l as f32, r as f32]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_equal_power_preserves_unit_power_everywhere() {
        for i in -100..=100 {
            let pan = i as f32 / 100.0;
            let [l, r] = pan_mono_equal_power(pan);
            let power = l * l + r * r;
            assert!(
                (power - 1.0).abs() < 1e-6,
                "pan={pan}: l={l}, r={r}, L^2+R^2={power}"
            );
            assert!((0.0..=1.0).contains(&l) && (0.0..=1.0).contains(&r));
        }
        assert_eq!(pan_mono_equal_power(-1.0), [1.0, 0.0]);
        assert_eq!(pan_mono_equal_power(1.0), [0.0, 1.0]);
        let [cl, cr] = pan_mono_equal_power(0.0);
        assert!((cl - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert!((cr - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert_eq!(pan_mono_equal_power(f32::NAN), pan_mono_equal_power(0.0));
    }

    #[test]
    fn stereo_balance_matches_equal_power_lr_ratios_with_unity_center() {
        assert_eq!(pan_stereo_balance(0.0), [1.0, 1.0]);
        assert_eq!(pan_stereo_balance(-1.0), [1.0, 0.0]);
        assert_eq!(pan_stereo_balance(1.0), [0.0, 1.0]);
        assert_eq!(pan_stereo_balance(-2.5), [1.0, 0.0]);
        assert_eq!(pan_stereo_balance(2.5), [0.0, 1.0]);
        assert_eq!(pan_stereo_balance(f32::NAN), [1.0, 1.0]);
        assert_eq!(pan_stereo_balance(f32::INFINITY), [1.0, 1.0]);
        assert_eq!(pan_stereo_balance(f32::NEG_INFINITY), [1.0, 1.0]);

        // Exact L/R ratio parity with pan_mono_equal_power across (-1.0, 1.0).
        for i in -99..=99 {
            let p = i as f64 / 100.0;
            let [sl, sr] = pan_stereo_balance_f64(p);
            let [ml, mr] = pan_mono_equal_power_f64(p);
            assert!((0.0..=1.0).contains(&sl) && (0.0..=1.0).contains(&sr));
            assert!(
                (sl / sr - ml / mr).abs() < 1e-12,
                "p={p}: stereo L/R={} != mono L/R={}",
                sl / sr,
                ml / mr
            );
        }

        // Responsive analog console attenuation checkpoints: -3.5 dB @ 0.25, -6.0 dB @ 0.41, -7.66 dB @ 0.50.
        let db_25 = lin_to_db(pan_stereo_balance(0.25)[0]);
        let db_41 = lin_to_db(pan_stereo_balance(0.41)[0]);
        let db_50 = lin_to_db(pan_stereo_balance(0.50)[0]);
        assert!((db_25 - (-3.50)).abs() < 0.05, "p=0.25 db={db_25}");
        assert!((db_41 - (-6.02)).abs() < 0.05, "p=0.41 db={db_41}");
        assert!((db_50 - (-7.66)).abs() < 0.05, "p=0.50 db={db_50}");

        // Strict monotonic attenuation on opposite channel, unity on active channel.
        let mut prev_l = 1.0_f32;
        for i in 1..=100 {
            let p = i as f32 / 100.0;
            let [l, r] = pan_stereo_balance(p);
            assert_eq!(r, 1.0);
            assert!(l < prev_l && l >= 0.0, "p={p}: l={l}, prev={prev_l}");
            prev_l = l;
        }
        // Symmetry across 0.
        for i in 1..=100 {
            let p = i as f32 / 100.0;
            let [lp, rp] = pan_stereo_balance(p);
            let [ln, rn] = pan_stereo_balance(-p);
            assert!((lp - rn).abs() < 1e-6 && (rp - ln).abs() < 1e-6);
        }
    }

    #[test]
    fn channel_gain_taper_roundtrips_monotonically_with_exact_unity_detent() {
        assert_eq!(norm_to_channel_gain(0.0), 0.0);
        assert_eq!(norm_to_channel_gain(-0.5), 0.0);
        assert_eq!(norm_to_channel_gain(f32::NAN), 0.0);
        assert_eq!(norm_to_channel_gain(f32::INFINITY), 0.0);
        assert_eq!(norm_to_channel_gain(CHANNEL_GAIN_UNITY_NORM), 1.0);
        assert_eq!(norm_to_channel_gain(1.0), MAX_CHANNEL_GAIN);
        assert_eq!(norm_to_channel_gain(1.5), MAX_CHANNEL_GAIN);

        assert_eq!(channel_gain_to_norm(0.0), 0.0);
        assert_eq!(channel_gain_to_norm(-0.5), 0.0);
        assert_eq!(channel_gain_to_norm(f32::NAN), 0.0);
        assert_eq!(channel_gain_to_norm(1.0), CHANNEL_GAIN_UNITY_NORM);
        assert_eq!(channel_gain_to_norm(MAX_CHANNEL_GAIN), 1.0);
        assert_eq!(channel_gain_to_norm(3.0), 1.0);

        let mut prev = -1.0_f32;
        for i in 0..=200 {
            let n = i as f32 / 200.0;
            let g = norm_to_channel_gain(n);
            assert!(g > prev, "non-monotonic at n={n}: g={g}, prev={prev}");
            prev = g;
            let rt = channel_gain_to_norm(g);
            assert!(
                (rt - n).abs() < 1e-6,
                "roundtrip failed: n={n}, g={g}, rt={rt}"
            );
        }
    }

    #[test]
    fn nudge_channel_gain_db_handles_floor_unity_snap_and_headroom() {
        // Floor behavior: -60 dB -> step down snaps to 0.0; step up from 0.0 enters at -60 dB.
        assert_eq!(nudge_channel_gain_db(0.001, -1.0), 0.0);
        assert!((nudge_channel_gain_db(0.0, 1.0) - 0.001).abs() < 1e-8);
        assert!((lin_to_db(nudge_channel_gain_db(0.0, 3.0)) - (-58.0)).abs() < 1e-4);

        // Exact unity snap from below and above.
        assert_eq!(nudge_channel_gain_db(db_to_lin(-1.0), 1.0), 1.0);
        assert_eq!(nudge_channel_gain_db(db_to_lin(3.0), -3.0), 1.0);

        // Headroom boost up to +6.0 dB max.
        let plus_3 = nudge_channel_gain_db(1.0, 3.0);
        assert!((lin_to_db(plus_3) - 3.0).abs() < 1e-4);
        let plus_6 = nudge_channel_gain_db(1.0, 6.0);
        assert!((lin_to_db(plus_6) - 6.0).abs() < 1e-4);
        let clamped = nudge_channel_gain_db(plus_6, 3.0);
        assert!((lin_to_db(clamped) - MAX_CHANNEL_GAIN_DB).abs() < 1e-4);
        assert!(clamped <= MAX_CHANNEL_GAIN);

        // Zero / non-finite deltas and NaN gain are safe.
        assert_eq!(nudge_channel_gain_db(1.25, 0.0), 1.25);
        assert_eq!(nudge_channel_gain_db(1.25, f32::NAN), 1.25);
        assert_eq!(nudge_channel_gain_db(f32::NAN, 0.0), 0.0);
    }

    #[test]
    fn format_db_value_and_gain_db_normalize_zero_and_silence() {
        assert_eq!(format_db_value(0.0), "-inf");
        assert_eq!(format_db_value(LIN_FLOOR_F32), "-inf");
        assert_eq!(format_db_value(-1.0), "-inf");
        assert_eq!(format_db_value(f32::NAN), "-inf");
        assert_eq!(format_db_value(1.0), "+0.0");
        assert_eq!(format_db_value(db_to_lin(-0.02)), "+0.0");
        assert_eq!(format_db_value(db_to_lin(-6.0)), "-6.0");
        assert_eq!(format_db_value(db_to_lin(6.0)), "+6.0");
        assert_eq!(format_db_value(MAX_CHANNEL_GAIN), "+6.0");

        assert_eq!(format_gain_db(0.0), "-inf dB");
        assert_eq!(format_gain_db(1.0), "+0.0 dB");
        assert_eq!(format_gain_db(db_to_lin(3.5)), "+3.5 dB");
    }

    #[test]
    fn apply_strip_gain_composes_factors_and_sanitizes() {
        let out = apply_strip_gain([0.5, -0.5], 0.8, 0.5, 0.5, 0.0);
        assert!((out[0] - 0.1).abs() < 1e-6);
        assert!((out[1] + 0.1).abs() < 1e-6);
        let boosted = apply_strip_gain([0.5, -0.5], 2.0, 1.0, 1.0, 0.0);
        assert!((boosted[0] - 1.0).abs() < 1e-6);
        assert!((boosted[1] + 1.0).abs() < 1e-6);
        let hard_right = apply_strip_gain([1.0, 1.0], 1.0, 1.0, 1.0, 1.0);
        assert_eq!(hard_right, [0.0, 1.0]);
        let nan_safe = apply_strip_gain([f32::NAN, 0.5], 1.0, 1.0, 1.0, 0.0);
        assert_eq!(nan_safe, [0.0, 0.5]);
    }

    #[test]
    fn stereo_ms_width_uncorrelated_preserves_rms_across_width_sweep() {
        // Exact identity at width = 1.0, exact constant-power mono at width = 0.0.
        assert_eq!(
            stereo_ms_width_uncorrelated_f64([0.6, -0.2], 1.0),
            [0.6, -0.2]
        );
        let mono = stereo_ms_width_uncorrelated_f64([1.0, 0.0], 0.0);
        assert!((mono[0] - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
        assert!((mono[1] - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
        assert_eq!(
            stereo_ms_width_uncorrelated_f64([0.4, -0.3], f64::NAN),
            [0.4, -0.3]
        );

        // Two orthogonal unit-RMS sinusoids model uncorrelated stereo FDN taps.
        let n = 4096usize;
        for &w in &[0.0_f64, 0.25, 0.5, 0.75, 1.0, 1.5, 2.0] {
            let mut in_e = [0.0_f64; 2];
            let mut out_e = [0.0_f64; 2];
            for i in 0..n {
                let l = (std::f64::consts::TAU * 13.0 * i as f64 / n as f64).sin();
                let r = (std::f64::consts::TAU * 29.0 * i as f64 / n as f64).cos();
                let y = stereo_ms_width_uncorrelated_f64([l, r], w);
                in_e[0] += l * l;
                in_e[1] += r * r;
                out_e[0] += y[0] * y[0];
                out_e[1] += y[1] * y[1];
            }
            for ch in 0..2 {
                let diff_db = crate::dsp::lin_to_db_f64((out_e[ch] / in_e[ch]).sqrt());
                assert!(
                    diff_db.abs() < 0.01,
                    "width={w} ch={ch}: uncorrelated RMS drifted by {diff_db:+.4} dB"
                );
            }
        }
    }

    #[test]
    fn stereo_mid_and_ms_helpers_preserve_unity_roundtrip_and_sanitize() {
        // Exact unity on mono frames (including > 1.0 headroom).
        for &v in &[-2.5_f64, -1.0, -0.25, 0.0, 0.5, 1.0, 3.5] {
            assert_eq!(stereo_mid_f64([v, v]), v);
            assert_eq!(stereo_mid_f32([v as f32, v as f32]), v as f32);
            assert_eq!(stereo_ms_encode_f64([v, v]), [v, 0.0]);
            assert_eq!(stereo_ms_decode_f64(v, 0.0), [v, v]);
            assert_eq!(stereo_ms_encode_f64([v, -v]), [0.0, v]);
            assert_eq!(stereo_ms_decode_f64(0.0, v), [v, -v]);
        }

        // Asymmetric stereo voltage-average mid sum.
        assert!((stereo_mid_f64([1.0, 0.0]) - 0.5).abs() < 1e-12);
        assert!((stereo_mid_f64([1.2, -0.4]) - 0.4).abs() < 1e-12);
        assert_eq!(stereo_mid_f64([-1.0, 1.0]), 0.0);
        assert!((stereo_mid_f32([1.0, 0.0]) - 0.5).abs() < 1e-6);
        assert!((stereo_mid_f32([1.2, -0.4]) - 0.4).abs() < 1e-6);
        assert_eq!(stereo_mid_f32([-1.0, 1.0]), 0.0);

        // Exact M/S encode <-> decode roundtrip across arbitrary stereo amplitudes.
        let vals = [-3.25_f64, -1.0, -0.3, 0.0, 0.4, 1.0, 2.75];
        for &l in &vals {
            for &r in &vals {
                let [m, s] = stereo_ms_encode_f64([l, r]);
                assert!((m - stereo_mid_f64([l, r])).abs() < 1e-12);
                let [rl, rr] = stereo_ms_decode_f64(m, s);
                assert!(
                    (rl - l).abs() < 1e-12 && (rr - r).abs() < 1e-12,
                    "M/S roundtrip failed for [{l}, {r}] -> [{m}, {s}] -> [{rl}, {rr}]"
                );
            }
        }

        // Non-finite sanitization on all four helpers.
        assert_eq!(stereo_mid_f64([f64::NAN, 0.8]), 0.4);
        assert_eq!(stereo_mid_f64([0.8, f64::INFINITY]), 0.4);
        assert_eq!(stereo_mid_f64([f64::NEG_INFINITY, f64::NAN]), 0.0);
        assert!((stereo_mid_f32([f32::NAN, 0.8]) - 0.4).abs() < 1e-6);
        assert!((stereo_mid_f32([0.8, f32::INFINITY]) - 0.4).abs() < 1e-6);
        assert_eq!(stereo_mid_f32([f32::NEG_INFINITY, f32::NAN]), 0.0);
        assert_eq!(stereo_ms_encode_f64([f64::NAN, 0.6]), [0.3, -0.3]);
        assert_eq!(stereo_ms_encode_f64([0.6, f64::INFINITY]), [0.3, 0.3]);
        assert_eq!(
            stereo_ms_encode_f64([f64::NAN, f64::NEG_INFINITY]),
            [0.0, 0.0]
        );
        assert_eq!(stereo_ms_decode_f64(f64::NAN, 0.5), [0.5, -0.5]);
        assert_eq!(stereo_ms_decode_f64(0.5, f64::INFINITY), [0.5, 0.5]);
        assert_eq!(
            stereo_ms_decode_f64(f64::NEG_INFINITY, f64::NAN),
            [0.0, 0.0]
        );
    }
}
