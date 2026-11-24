//! Deterministic zero-allocation PRNG (`XorShift32`), golden-ratio seed mixer
//! (`mix_seed`), sample-rate-invariant `-3 dB/octave` pinking filter (`PinkFilter`),
//! and constant-PSD White/Pink noise generator (`NoiseGen`).

use crate::dsp::filter::{Svf, SvfCoeffs};
use crate::dsp::math::{flush_denormal_f64, sanitize_sample_f64, sanitize_sample_rate};
use crate::rack::{FilterMode, NoiseColor};
use std::f64::consts::TAU;

/// 32-bit golden-ratio Weyl sequence constant (`floor(2^32 / phi)`).
pub const GOLDEN_RATIO_32: u32 = 0x9e37_79b9;

/// Secondary 32-bit Weyl sequence multiplier (upper 32 bits of SplitMix64's `0x517cc1b727220a95`)
/// used to decorrelate sibling voice, chord, sub-event, and module PRNG seeds.
pub const SECONDARY_WEYL_SALT_32: u32 = 0x517c_c1b7;

/// Reference sample rate (Hz) at which constant-PSD noise equals unscaled `[-1, 1]` PRNG output.
pub const NOISE_REF_SAMPLE_RATE: f64 = 44_100.0;

/// Audible ultrasonic ceiling (Hz) applied via a 4th-order Butterworth SVF when
/// `sample_rate > NOISE_REF_SAMPLE_RATE`, matching the `44.1 kHz` Nyquist ceiling so
/// wideband RMS and downstream nonlinear drive stay consistent from `44.1 kHz` through `192 kHz+`.
const NOISE_ULTRASONIC_LPF_HZ: f64 = NOISE_REF_SAMPLE_RATE * 0.5;

/// 4th-order Butterworth cascaded SVF damping factors: `2 cos(3π/8)` and `2 cos(π/8)`.
const BUTTER4_K1: f64 = 0.765_366_864_730_179_5;
const BUTTER4_K2: f64 = 1.847_759_065_022_573_5;

/// Number of cascaded 1-pole/1-zero shelving stages in [`PinkFilter`].
const PINK_STAGES: usize = 7;

/// Alternating continuous-time `(pole_hz, zero_hz)` pairs spaced by 2 octaves per stage
/// (`zero_hz = 2 * pole_hz`), approximating `-3.0103 dB/octave` (`1/sqrt(f)`) across
/// `20 Hz .. 20 kHz` at any sample rate.
const PINK_STAGE_HZ: [(f64, f64); PINK_STAGES] = [
    (6.0, 12.0),
    (24.0, 48.0),
    (96.0, 192.0),
    (384.0, 768.0),
    (1_536.0, 3_072.0),
    (6_144.0, 12_288.0),
    (24_576.0, 49_152.0),
];

/// Output gain calibrating the unit-DC cascade to `-14.3 dB` wideband RMS on unit-amplitude
/// constant-PSD white noise (matching classic Kellet pink noise at `44.1 kHz`).
const PINK_OUTPUT_GAIN: f64 = 7.958_537_75;

/// Deterministic golden-ratio seed mixer (`seed.wrapping_add(salt.wrapping_mul(0x9e37_79b9))`).
///
/// Decorrelates sibling voices, layers, steps, and modules without collisions on
/// small sequential indices (`salt = 0, 1, 2, ...`).
#[inline]
pub const fn mix_seed(seed: u32, salt: u32) -> u32 {
    seed.wrapping_add(salt.wrapping_mul(GOLDEN_RATIO_32))
}

/// Fast, deterministic, zero-allocation 32-bit xorshift PRNG for audio noise
/// and modulation (sample-and-hold LFOs, free-running phase randomization).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XorShift32 {
    state: u32,
}

impl Default for XorShift32 {
    fn default() -> Self {
        Self { state: 1 }
    }
}

impl XorShift32 {
    /// Construct a new PRNG from `seed`, clamping `0` to `1` so the xorshift state never stalls.
    #[inline]
    pub const fn new(seed: u32) -> Self {
        Self {
            state: if seed == 0 { 1 } else { seed },
        }
    }

    /// Advance the PRNG by one step and return the next non-zero `u32`.
    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        if x == 0 {
            x = 1;
        }
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }

    /// Advance the PRNG and return a uniform bipolar sample in `(-1.0, 1.0]` (`f64`).
    #[inline]
    pub fn next_bipolar_f64(&mut self) -> f64 {
        f64::from(self.next_u32()) / f64::from(u32::MAX) * 2.0 - 1.0
    }

    /// Current internal non-zero state.
    #[inline]
    pub const fn state(self) -> u32 {
        self.state
    }
}

/// Sample-rate-invariant 7-stage `-3.01 dB/octave` pinking filter.
///
/// Discretizes an alternating continuous-time pole-zero shelving ladder (`6 Hz .. 49 kHz`)
/// via DC- and Nyquist-magnitude-matched 1-pole/1-zero TDF-II sections so spectral tilt
/// and passband gain remain invariant across `24 kHz .. 192 kHz+`.
///
/// Zero-allocation, denormal-safe, and finite-guarded.
#[derive(Clone, Copy, Debug)]
pub struct PinkFilter {
    s: [f64; PINK_STAGES],
    p: [f64; PINK_STAGES],
    b0: [f64; PINK_STAGES],
    b1: [f64; PINK_STAGES],
}

impl Default for PinkFilter {
    fn default() -> Self {
        Self::new(NOISE_REF_SAMPLE_RATE)
    }
}

impl PinkFilter {
    /// Construct a pinking filter calibrated for `sample_rate` (Hz).
    #[inline]
    pub fn new(sample_rate: f64) -> Self {
        let mut filt = Self {
            s: [0.0; PINK_STAGES],
            p: [0.0; PINK_STAGES],
            b0: [0.0; PINK_STAGES],
            b1: [0.0; PINK_STAGES],
        };
        filt.set_sample_rate(sample_rate);
        filt
    }

    /// Recompute per-stage pole/zero coefficients for `sample_rate` (Hz) without clearing state.
    #[inline]
    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        let sr = sanitize_sample_rate(sample_rate);
        let nyq = sr * 0.5;
        for (k, &(fp, fz)) in PINK_STAGE_HZ.iter().enumerate() {
            let p = (-TAU * fp / sr).exp();
            let g_nyq = ((1.0 + (nyq / fz).powi(2)) / (1.0 + (nyq / fp).powi(2))).sqrt();
            let r = g_nyq * (1.0 + p) / (1.0 - p).max(1e-12);
            let z = (r - 1.0) / (r + 1.0);
            let g = (1.0 - p) / (1.0 - z).max(1e-12);
            self.p[k] = p;
            self.b0[k] = g;
            self.b1[k] = -g * z;
        }
    }

    #[inline]
    pub fn reset(&mut self) {
        self.s = [0.0; PINK_STAGES];
    }

    /// Filter one constant-PSD white noise sample into pink noise (`-3 dB/octave`).
    #[inline]
    pub fn process(&mut self, white: f64) -> f64 {
        let mut x = sanitize_sample_f64(white);
        for k in 0..PINK_STAGES {
            let y = self.b0[k] * x + self.s[k];
            self.s[k] = flush_denormal_f64(self.b1[k] * x + self.p[k] * y);
            x = y;
        }
        x * PINK_OUTPUT_GAIN
    }
}

/// Zero-allocation, sample-rate-invariant White/Pink noise generator composing
/// [`XorShift32`], a 4th-order Butterworth ultrasonic ceiling (above `44.1 kHz`),
/// and [`PinkFilter`].
///
/// Scales raw PRNG output by `sqrt(sample_rate / 44_100)` so in-band power spectral
/// density is constant across all sample rates, and band-limits above `21.3 kHz`
/// whenever `sample_rate > 44.1 kHz` so wideband RMS and downstream nonlinear stages
/// stay consistent from `44.1 kHz` through `192 kHz+`.
#[derive(Clone, Copy, Debug)]
pub struct NoiseGen {
    rng: XorShift32,
    pink: PinkFilter,
    psd_scale: f64,
    ultrasonic_active: bool,
    ult_stage1: Svf,
    ult_stage2: Svf,
    ult_c1: SvfCoeffs,
    ult_c2: SvfCoeffs,
}

impl Default for NoiseGen {
    fn default() -> Self {
        Self::new(1, NOISE_REF_SAMPLE_RATE)
    }
}

impl NoiseGen {
    /// Construct a constant-PSD noise generator seeded with `seed` for `sample_rate` (Hz).
    #[inline]
    pub fn new(seed: u32, sample_rate: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let mut gen = Self {
            rng: XorShift32::new(seed),
            pink: PinkFilter::new(sr),
            psd_scale: 1.0,
            ultrasonic_active: false,
            ult_stage1: Svf::default(),
            ult_stage2: Svf::default(),
            ult_c1: SvfCoeffs::with_damping(sr, NOISE_ULTRASONIC_LPF_HZ, BUTTER4_K1),
            ult_c2: SvfCoeffs::with_damping(sr, NOISE_ULTRASONIC_LPF_HZ, BUTTER4_K2),
        };
        gen.set_sample_rate(sr);
        gen
    }

    /// Update sample-rate scaling, ultrasonic ceiling, and pinking filter coefficients.
    #[inline]
    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        let sr = sanitize_sample_rate(sample_rate);
        self.psd_scale = (sr / NOISE_REF_SAMPLE_RATE).sqrt();
        self.ultrasonic_active = sr > NOISE_REF_SAMPLE_RATE + 1.0;
        if self.ultrasonic_active {
            self.ult_c1 = SvfCoeffs::with_damping(sr, NOISE_ULTRASONIC_LPF_HZ, BUTTER4_K1);
            self.ult_c2 = SvfCoeffs::with_damping(sr, NOISE_ULTRASONIC_LPF_HZ, BUTTER4_K2);
        } else {
            self.ult_stage1.reset();
            self.ult_stage2.reset();
        }
        self.pink.set_sample_rate(sr);
    }

    #[inline]
    pub fn reset(&mut self, seed: u32) {
        self.rng = XorShift32::new(seed);
        self.pink.reset();
        self.ult_stage1.reset();
        self.ult_stage2.reset();
    }

    /// Generate one constant-PSD white noise sample (`(-1.0, 1.0]` at `44.1 kHz`,
    /// ultrasonic-band-limited above `21.3 kHz` at higher sample rates).
    #[inline]
    pub fn next_white(&mut self) -> f64 {
        let raw = self.rng.next_bipolar_f64() * self.psd_scale;
        if self.ultrasonic_active {
            let s1 = self
                .ult_stage1
                .process_mono(raw, self.ult_c1, FilterMode::LowPass);
            self.ult_stage2
                .process_mono(s1, self.ult_c2, FilterMode::LowPass)
        } else {
            raw
        }
    }

    /// Generate one sample-rate-invariant pink noise sample (`-3 dB/octave` spectral tilt).
    #[inline]
    pub fn next_pink(&mut self) -> f64 {
        let white = self.next_white();
        self.pink.process(white)
    }

    /// Generate one sample for the selected [`NoiseColor`].
    #[inline]
    pub fn next_sample(&mut self, color: NoiseColor) -> f64 {
        match color {
            NoiseColor::White => self.next_white(),
            NoiseColor::Pink => self.next_pink(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_seed_is_deterministic_and_decorrelates_sequential_salts() {
        assert_eq!(mix_seed(42, 0), 42);
        assert_eq!(mix_seed(0, 1), GOLDEN_RATIO_32);
        assert_eq!(
            mix_seed(12345, 7),
            12345_u32.wrapping_add(7_u32.wrapping_mul(0x9e37_79b9))
        );
        let seeds: Vec<u32> = (0..16).map(|i| mix_seed(99, i)).collect();
        for i in 0..seeds.len() {
            for j in (i + 1)..seeds.len() {
                assert_ne!(seeds[i], seeds[j]);
            }
        }
    }

    #[test]
    fn xorshift32_clamps_zero_seed_is_deterministic_and_unbiased() {
        let mut from_zero = XorShift32::new(0);
        let mut from_one = XorShift32::new(1);
        assert_eq!(from_zero.state(), 1);
        assert_eq!(from_zero.next_u32(), from_one.next_u32());

        let mut rng = XorShift32::new(0x1234_5678);
        let mut sum = 0.0_f64;
        let mut min_v = f64::INFINITY;
        let mut max_v = f64::NEG_INFINITY;
        let n = 50_000;
        for _ in 0..n {
            let v = rng.next_bipolar_f64();
            assert!(v.is_finite() && (-1.0..=1.0).contains(&v));
            sum += v;
            min_v = min_v.min(v);
            max_v = max_v.max(v);
        }
        let mean = sum / n as f64;
        assert!(mean.abs() < 0.02, "expected near-zero mean, got {mean}");
        assert!(min_v < -0.99 && max_v > 0.99);
    }

    #[test]
    fn pink_filter_tilts_spectrum_warmer_than_white_and_handles_nan() {
        let mut white_gen = NoiseGen::new(0xdead_beef, 48_000.0);
        let mut pink_gen = NoiseGen::new(0xdead_beef, 48_000.0);
        let n = 48_000;

        let measure_hf_ratio = |gen: &mut NoiseGen, color: NoiseColor| -> f64 {
            let mut prev = 0.0_f64;
            let mut diff_sq = 0.0_f64;
            let mut energy = 0.0_f64;
            for i in 0..n {
                let s = gen.next_sample(color);
                assert!(s.is_finite());
                if i >= 1024 {
                    let d = s - prev;
                    diff_sq += d * d;
                    energy += s * s;
                }
                prev = s;
            }
            (diff_sq / energy.max(1e-12)).sqrt()
        };

        let white_hf = measure_hf_ratio(&mut white_gen, NoiseColor::White);
        let pink_hf = measure_hf_ratio(&mut pink_gen, NoiseColor::Pink);
        assert!(
            pink_hf < white_hf * 0.45,
            "pink noise must have much lower HF/LF ratio than white: pink={pink_hf:.3} vs white={white_hf:.3}"
        );

        // NaN input must sanitize cleanly and never poison filter state.
        let mut filt = PinkFilter::new(48_000.0);
        assert_eq!(filt.process(f64::NAN), 0.0);
        assert_eq!(filt.process(f64::INFINITY), 0.0);
        let y = filt.process(0.5);
        assert!(y.is_finite() && y.abs() > 0.0);
        filt.reset();
        assert_eq!(filt.process(0.0), 0.0);
    }

    #[test]
    fn noise_gen_reset_replays_exact_sequence() {
        let mut a = NoiseGen::new(777, 96_000.0);
        let seq_a: Vec<f64> = (0..64).map(|_| a.next_pink()).collect();
        a.reset(777);
        let seq_b: Vec<f64> = (0..64).map(|_| a.next_pink()).collect();
        assert_eq!(seq_a, seq_b);

        let mut w1 = NoiseGen::new(42, 48_000.0);
        let mut w2 = NoiseGen::new(42, 48_000.0);
        for _ in 0..32 {
            assert_eq!(w1.next_white(), w2.next_sample(NoiseColor::White));
        }
    }

    #[test]
    fn noise_gen_white_and_pink_are_sample_rate_invariant() {
        use crate::dsp::biquad::TptOnePoleLpf;
        use crate::dsp::math::lin_to_db_f64;

        // 1. At 44.1 kHz, white noise equals unscaled XorShift32 bit-for-bit.
        let mut gen44 = NoiseGen::new(0x55aa_1234, NOISE_REF_SAMPLE_RATE);
        let mut rng44 = XorShift32::new(0x55aa_1234);
        for _ in 0..64 {
            assert_eq!(gen44.next_white(), rng44.next_bipolar_f64());
        }

        // 2. Wideband White and Pink RMS remain invariant across 44.1 / 48 / 96 / 192 kHz.
        let measure_rms_db = |sr: f64, color: NoiseColor, mode: u8| -> f64 {
            let mut gen = NoiseGen::new(0x1357_9bdf, sr);
            let mut bp = Svf::default();
            let bp_coeffs = SvfCoeffs::new(sr, 1_200.0, 0.65);
            let mut lp = TptOnePoleLpf::new(sr, 1_000.0);
            let warmup = (sr * 0.05) as usize;
            let frames = (sr * 0.6) as usize;
            let mut sum_sq = 0.0_f64;
            for i in 0..(warmup + frames) {
                let s = gen.next_sample(color);
                let y = match mode {
                    0 => s,
                    1 => bp.process_mono(s, bp_coeffs, FilterMode::BandPass),
                    _ => lp.process_mono(s),
                };
                if i >= warmup {
                    sum_sq += y * y;
                }
            }
            lin_to_db_f64((sum_sq / frames as f64).sqrt())
        };

        let ref_white_wb = measure_rms_db(44_100.0, NoiseColor::White, 0);
        let ref_pink_wb = measure_rms_db(44_100.0, NoiseColor::Pink, 0);
        for sr in [48_000.0, 96_000.0, 192_000.0] {
            let w = measure_rms_db(sr, NoiseColor::White, 0);
            let p = measure_rms_db(sr, NoiseColor::Pink, 0);
            assert!(
                (w - ref_white_wb).abs() < 0.45,
                "wideband white drifted at sr={sr}: {w:.2} vs {ref_white_wb:.2} dB"
            );
            assert!(
                (p - ref_pink_wb).abs() < 0.45,
                "wideband pink drifted at sr={sr}: {p:.2} vs {ref_pink_wb:.2} dB"
            );
        }

        // 3. Band-limited White (1.2 kHz BPF) and LPF Pink (1 kHz LPF) stay invariant down to 24 kHz too.
        let ref_white_bp = measure_rms_db(44_100.0, NoiseColor::White, 1);
        let ref_pink_lp = measure_rms_db(44_100.0, NoiseColor::Pink, 2);
        for sr in [24_000.0, 48_000.0, 96_000.0, 192_000.0] {
            let w = measure_rms_db(sr, NoiseColor::White, 1);
            let p = measure_rms_db(sr, NoiseColor::Pink, 2);
            assert!(
                (w - ref_white_bp).abs() < 0.55,
                "bandpass white drifted at sr={sr}: {w:.2} vs {ref_white_bp:.2} dB"
            );
            assert!(
                (p - ref_pink_lp).abs() < 0.55,
                "lowpass pink drifted at sr={sr}: {p:.2} vs {ref_pink_lp:.2} dB"
            );
        }

        // 4. Deterministic sine-sweep verification of PinkFilter -3.01 dB/octave slope across 24..192 kHz.
        let measure_pink_gain_db = |sr: f64, freq_hz: f64| -> f64 {
            let mut filt = PinkFilter::new(sr);
            let warmup = (sr * 0.25) as usize;
            let frames = (sr * 0.10) as usize;
            let mut in_sq = 0.0_f64;
            let mut out_sq = 0.0_f64;
            for i in 0..(warmup + frames) {
                let x = (TAU * freq_hz * (i as f64) / sr).sin();
                let y = filt.process(x);
                if i >= warmup {
                    in_sq += x * x;
                    out_sq += y * y;
                }
            }
            lin_to_db_f64((out_sq / in_sq.max(1e-18)).sqrt())
        };

        let ref_1k = measure_pink_gain_db(44_100.0, 1_000.0);
        for sr in [24_000.0, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let g250 = measure_pink_gain_db(sr, 250.0);
            let g1k = measure_pink_gain_db(sr, 1_000.0);
            let g4k = measure_pink_gain_db(sr, 4_000.0);
            assert!(
                (g1k - ref_1k).abs() < 0.15,
                "PinkFilter 1 kHz gain drifted at sr={sr}: {g1k:.2} vs {ref_1k:.2} dB"
            );
            // Two octaves = -6.02 dB ideal pink slope.
            assert!(
                ((g250 - g1k) - 6.02).abs() < 0.25,
                "PinkFilter 250->1000 Hz slope off at sr={sr}: {:.2} dB",
                g250 - g1k
            );
            assert!(
                ((g1k - g4k) - 6.02).abs() < 0.45,
                "PinkFilter 1000->4000 Hz slope off at sr={sr}: {:.2} dB",
                g1k - g4k
            );
        }
    }
}
