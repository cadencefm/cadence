//! Pure Blackman-windowed sinc stereo resampler (zero I/O, zero backend dependencies).
use crate::dsp::math::sanitize_sample;

/// Anti-aliased Blackman-windowed sinc stereo resampler (e.g. downsampling live
/// 48 kHz speaker audio cleanly to 24 kHz WAV).
pub fn resample_stereo(frames: &[[f32; 2]], src_rate: u32, dst_rate: u32) -> Vec<[f32; 2]> {
    if frames.is_empty() || src_rate == 0 || dst_rate == 0 || src_rate == dst_rate {
        return frames.to_vec();
    }
    let ratio = f64::from(src_rate) / f64::from(dst_rate);
    let out_len = ((frames.len() as f64 / ratio).round() as usize).max(1);
    let cutoff = (f64::from(dst_rate) / f64::from(src_rate)).min(1.0);
    // Scale kernel half-width when downsampling so the anti-aliasing lowpass stays sharp.
    let half_taps = ((8.0 / cutoff).ceil() as isize).clamp(8, 32);
    let half_taps_f = half_taps as f64;
    let max_idx = (frames.len() - 1) as isize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let center = i as f64 * ratio;
        let center_idx = center.floor() as isize;
        let mut acc_l = 0.0_f64;
        let mut acc_r = 0.0_f64;
        let mut norm = 0.0_f64;
        for tap in (center_idx - half_taps + 1)..=(center_idx + half_taps) {
            let x = center - tap as f64;
            if x.abs() >= half_taps_f {
                continue;
            }
            let arg = std::f64::consts::PI * x * cutoff;
            let sinc = if arg.abs() < 1e-12 {
                1.0
            } else {
                arg.sin() / arg
            };
            // Blackman window over [-half_taps, +half_taps].
            let u = (x + half_taps_f) / (2.0 * half_taps_f);
            let win = 0.42 - 0.5 * (2.0 * std::f64::consts::PI * u).cos()
                + 0.08 * (4.0 * std::f64::consts::PI * u).cos();
            let w = sinc * win;
            let clamped = tap.clamp(0, max_idx) as usize;
            let sample = frames[clamped];
            acc_l += f64::from(sanitize_sample(sample[0])) * w;
            acc_r += f64::from(sanitize_sample(sample[1])) * w;
            norm += w;
        }
        let inv = if norm.abs() > 1e-12 { 1.0 / norm } else { 0.0 };
        out.push([(acc_l * inv) as f32, (acc_r * inv) as f32]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_stereo_preserves_dc_identity_and_attenuates_aliasing() {
        // Identity & edge cases.
        assert!(resample_stereo(&[], 48_000, 24_000).is_empty());
        let dc = vec![[0.5_f32, -0.25_f32]; 256];
        assert_eq!(resample_stereo(&dc, 48_000, 48_000), dc);
        assert_eq!(resample_stereo(&dc, 0, 24_000), dc);

        // Downsample 48 kHz -> 24 kHz preserves DC gain accurately.
        let down_dc = resample_stereo(&dc, 48_000, 24_000);
        assert_eq!(down_dc.len(), 128);
        for frame in &down_dc[16..112] {
            assert!((frame[0] - 0.5).abs() < 1e-4, "L DC drift: {}", frame[0]);
            assert!((frame[1] + 0.25).abs() < 1e-4, "R DC drift: {}", frame[1]);
        }

        // Anti-aliasing: an 18 kHz sine at 48 kHz is above 12 kHz Nyquist of 24 kHz
        // and must be strongly attenuated in the interior (> 36 dB rejection).
        let n = 4096;
        let high_sine: Vec<[f32; 2]> = (0..n)
            .map(|i| {
                let s = (std::f64::consts::TAU * 18_000.0 * i as f64 / 48_000.0).sin() as f32;
                [s, -s]
            })
            .collect();
        let down_hi = resample_stereo(&high_sine, 48_000, 24_000);
        assert_eq!(down_hi.len(), n / 2);
        let interior_peak = down_hi[128..down_hi.len() - 128]
            .iter()
            .fold(0.0_f32, |m, f| m.max(f[0].abs()).max(f[1].abs()));
        assert!(
            interior_peak < 0.015,
            "18 kHz tone must be strongly attenuated when downsampling 48k->24k, got peak {interior_peak}"
        );

        // Passband: a 1 kHz sine at 48 kHz -> 24 kHz preserves amplitude cleanly.
        let low_sine: Vec<[f32; 2]> = (0..n)
            .map(|i| {
                let s = (std::f64::consts::TAU * 1_000.0 * i as f64 / 48_000.0).sin() as f32;
                [s, s]
            })
            .collect();
        let down_lo = resample_stereo(&low_sine, 48_000, 24_000);
        let lo_peak = down_lo[128..down_lo.len() - 128]
            .iter()
            .fold(0.0_f32, |m, f| m.max(f[0].abs()));
        assert!(
            (lo_peak - 1.0).abs() < 0.02,
            "1 kHz passband tone should keep unity peak, got {lo_peak}"
        );

        // Non-finite input samples sanitize cleanly without infecting output.
        let dirty = vec![[f32::NAN, f32::INFINITY]; 64];
        let clean = resample_stereo(&dirty, 48_000, 24_000);
        assert!(clean
            .iter()
            .all(|f| f[0].is_finite() && f[1].is_finite() && f[0] == 0.0 && f[1] == 0.0));
    }
}
