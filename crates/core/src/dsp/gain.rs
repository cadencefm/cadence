//! Single-source-of-truth pan laws, channel fader headroom/taper, dB nudging,
//! dB formatting, and channel strip gain staging.
//!
//! Two distinct, mathematically well-defined pan laws cover all stereo routing:
//! - [`pan_mono_equal_power`][] — constant-power (`-3.01 dB` center, `L² + R² == 1.0`)
//!   for mono sources (oscillators, unison voices, mono voices, and rack zones).
//! - [`pan_stereo_balance`][] — unity-normalized equal-power stereo balance
//!   (`tan((1 ∓ p) * π/4)`) preserving centered stereo fields at exact `[1.0, 1.0]`
//!   unity while matching the exact `L/R` ratio of an analog `-3 dB` constant-power
//!   console pan pot everywhere across `-1.0..=1.0`, used for stereo mixer strips
//!   and buses.

use crate::dsp::math::{db_to_lin, lin_to_db, sanitize_sample, sanitize_sample_f64, LIN_FLOOR_F32};
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
}
