//! RBJ Audio EQ Cookbook biquads, bilinear/TPT 1-pole LPF & HPF filters,
//! normalized stereo DC blocker, and Cadence's 4-band studio `Eq` insert.
//!
//! Analytical frequency responses (`BiquadCoeffs::magnitude_db`,
//! `TptOnePoleLpf::magnitude_db`, `TptOnePoleHpf::magnitude_db`, `eq_response_db`)
//! share the exact same formulas as the real-time DSP processors so UI plots
//! never drift from heard audio.

use crate::dsp::math::{
    flush_denormal_f64, lin_to_db_f64, one_pole_coeff_ms, sanitize_frame, sanitize_sample_f64,
    sanitize_sample_rate, Smoother,
};
use crate::mixer::EqPatch;
use std::f64::consts::{FRAC_1_SQRT_2, PI, TAU};

/// Normalized second-order biquad coefficients (`a0 == 1.0`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BiquadCoeffs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
    pub active: bool,
}

impl Default for BiquadCoeffs {
    fn default() -> Self {
        Self::bypass()
    }
}

impl BiquadCoeffs {
    /// Unity pass-through coefficients.
    #[inline]
    pub const fn bypass() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            active: false,
        }
    }

    /// 12 dB/oct RBJ Cookbook high-pass filter.
    pub fn highpass(sample_rate: f64, freq_hz: f64, q: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let f = sanitize_sample_f64(freq_hz).clamp(10.0, sr * 0.45);
        let q = sanitize_sample_f64(q).max(0.1);
        let w0 = TAU * f / sr;
        let cos_w0 = w0.cos();
        let alpha = w0.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: ((1.0 + cos_w0) * 0.5) / a0,
            b1: -(1.0 + cos_w0) / a0,
            b2: ((1.0 + cos_w0) * 0.5) / a0,
            a1: (-2.0 * cos_w0) / a0,
            a2: (1.0 - alpha) / a0,
            active: true,
        }
    }

    /// RBJ Cookbook low-shelf filter (`S = 1.0` Butterworth shelf slope).
    pub fn low_shelf(sample_rate: f64, freq_hz: f64, gain_db: f64) -> Self {
        let gain_db = sanitize_sample_f64(gain_db);
        if gain_db.abs() < 0.01 {
            return Self::bypass();
        }
        let sr = sanitize_sample_rate(sample_rate);
        let a = 10.0_f64.powf(gain_db / 40.0);
        let f = sanitize_sample_f64(freq_hz).clamp(20.0, sr * 0.45);
        let w0 = TAU * f / sr;
        let cos_w0 = w0.cos();
        let alpha = w0.sin() * 0.5 * 2.0_f64.sqrt();
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
        let a0 = (a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
        Self {
            b0: (a * ((a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha)) / a0,
            b1: (2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0)) / a0,
            b2: (a * ((a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha)) / a0,
            a1: (-2.0 * ((a - 1.0) + (a + 1.0) * cos_w0)) / a0,
            a2: ((a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha) / a0,
            active: true,
        }
    }

    /// RBJ Cookbook peaking EQ filter.
    pub fn peaking(sample_rate: f64, freq_hz: f64, q: f64, gain_db: f64) -> Self {
        let gain_db = sanitize_sample_f64(gain_db);
        if gain_db.abs() < 0.01 {
            return Self::bypass();
        }
        let sr = sanitize_sample_rate(sample_rate);
        let a = 10.0_f64.powf(gain_db / 40.0);
        let f = sanitize_sample_f64(freq_hz).clamp(20.0, sr * 0.45);
        let q = sanitize_sample_f64(q).max(0.1);
        let w0 = TAU * f / sr;
        let cos_w0 = w0.cos();
        let alpha = w0.sin() / (2.0 * q);
        let a0 = 1.0 + alpha / a;
        Self {
            b0: (1.0 + alpha * a) / a0,
            b1: (-2.0 * cos_w0) / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: (-2.0 * cos_w0) / a0,
            a2: (1.0 - alpha / a) / a0,
            active: true,
        }
    }

    /// RBJ Cookbook high-shelf filter (`S = 1.0` Butterworth shelf slope).
    pub fn high_shelf(sample_rate: f64, freq_hz: f64, gain_db: f64) -> Self {
        let gain_db = sanitize_sample_f64(gain_db);
        if gain_db.abs() < 0.01 {
            return Self::bypass();
        }
        let sr = sanitize_sample_rate(sample_rate);
        let a = 10.0_f64.powf(gain_db / 40.0);
        let f = sanitize_sample_f64(freq_hz).clamp(20.0, sr * 0.45);
        let w0 = TAU * f / sr;
        let cos_w0 = w0.cos();
        let alpha = w0.sin() * 0.5 * 2.0_f64.sqrt();
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
        let a0 = (a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
        Self {
            b0: (a * ((a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha)) / a0,
            b1: (-2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0)) / a0,
            b2: (a * ((a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha)) / a0,
            a1: (2.0 * ((a - 1.0) - (a + 1.0) * cos_w0)) / a0,
            a2: ((a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha) / a0,
            active: true,
        }
    }

    /// Exact linear magnitude `|H(e^{jω})|` at `freq_hz` for `sample_rate`.
    #[inline]
    pub fn magnitude(&self, freq_hz: f64, sample_rate: f64) -> f64 {
        if !self.active {
            return 1.0;
        }
        let sr = sanitize_sample_rate(sample_rate);
        let w = TAU * sanitize_sample_f64(freq_hz).clamp(0.0, sr * 0.5) / sr;
        let cw = w.cos();
        let sw = w.sin();
        let c2w = (2.0 * w).cos();
        let s2w = (2.0 * w).sin();
        let nr = self.b0 + self.b1 * cw + self.b2 * c2w;
        let ni = -self.b1 * sw - self.b2 * s2w;
        let dr = 1.0 + self.a1 * cw + self.a2 * c2w;
        let di = -self.a1 * sw - self.a2 * s2w;
        nr.hypot(ni) / dr.hypot(di).max(1e-12)
    }

    /// Exact magnitude response in decibels (`-120 dB` floor).
    #[inline]
    pub fn magnitude_db(&self, freq_hz: f64, sample_rate: f64) -> f64 {
        lin_to_db_f64(self.magnitude(freq_hz, sample_rate))
    }

    #[inline]
    fn max_diff(self, other: Self) -> f64 {
        (self.b0 - other.b0)
            .abs()
            .max((self.b1 - other.b1).abs())
            .max((self.b2 - other.b2).abs())
            .max((self.a1 - other.a1).abs())
            .max((self.a2 - other.a2).abs())
    }
}

/// Stereo Transposed Direct-Form II biquad with smooth coefficient interpolation,
/// NaN protection, denormal flushing, and clean bypass state reset.
#[derive(Clone, Copy, Debug)]
pub struct StereoBiquad {
    current: BiquadCoeffs,
    target: BiquadCoeffs,
    smooth_coeff: f64,
    z1: [f64; 2],
    z2: [f64; 2],
}

impl Default for StereoBiquad {
    fn default() -> Self {
        Self::new(48_000.0)
    }
}

impl StereoBiquad {
    /// Construct a bypassed stereo biquad with 4 ms coefficient interpolation.
    pub fn new(sample_rate: f64) -> Self {
        Self {
            current: BiquadCoeffs::bypass(),
            target: BiquadCoeffs::bypass(),
            smooth_coeff: one_pole_coeff_ms(4.0, sample_rate),
            z1: [0.0; 2],
            z2: [0.0; 2],
        }
    }

    /// Reset delay registers and snap coefficients to target immediately.
    pub fn reset(&mut self) {
        self.z1 = [0.0; 2];
        self.z2 = [0.0; 2];
        self.current = self.target;
    }

    /// Snap coefficients immediately (for initialization or reset).
    pub fn snap_coeffs(&mut self, coeffs: BiquadCoeffs) {
        self.current = coeffs;
        self.target = coeffs;
        if !coeffs.active {
            self.z1 = [0.0; 2];
            self.z2 = [0.0; 2];
        }
    }

    /// Set new target coefficients to interpolate smoothly (convex combination of
    /// stable 2nd-order denominators preserves strict BIBO stability).
    pub fn set_coeffs(&mut self, target: BiquadCoeffs) {
        self.target = target;
    }

    /// Process one stereo `f64` frame.
    #[inline]
    pub fn process(&mut self, input: [f64; 2]) -> [f64; 2] {
        let x = [sanitize_sample_f64(input[0]), sanitize_sample_f64(input[1])];
        let diff = self.current.max_diff(self.target);
        if diff <= 1e-12 {
            self.current = self.target;
            if !self.current.active {
                self.z1 = [0.0; 2];
                self.z2 = [0.0; 2];
                return x;
            }
        } else {
            let a = self.smooth_coeff;
            self.current.b0 += (self.target.b0 - self.current.b0) * a;
            self.current.b1 += (self.target.b1 - self.current.b1) * a;
            self.current.b2 += (self.target.b2 - self.current.b2) * a;
            self.current.a1 += (self.target.a1 - self.current.a1) * a;
            self.current.a2 += (self.target.a2 - self.current.a2) * a;
            self.current.active = true;
        }

        let c = self.current;
        let mut out = [0.0; 2];
        for ch in 0..2 {
            let y = c.b0 * x[ch] + self.z1[ch];
            let z1 = c.b1 * x[ch] - c.a1 * y + self.z2[ch];
            let z2 = c.b2 * x[ch] - c.a2 * y;
            self.z1[ch] = flush_denormal_f64(z1);
            self.z2[ch] = flush_denormal_f64(z2);
            out[ch] = sanitize_sample_f64(y);
        }
        out
    }
}

/// Bilinear / Topology-Preserving Transform (TPT) 1-pole low-pass filter (`-3.01 dB` at `fc`,
/// unity DC gain, zero Nyquist passband distortion across all sample rates).
#[derive(Clone, Copy, Debug)]
pub struct TptOnePoleLpf {
    g_coeff: f64,
    s: [f64; 2],
}

impl Default for TptOnePoleLpf {
    fn default() -> Self {
        Self::new(48_000.0, 8_000.0)
    }
}

impl TptOnePoleLpf {
    pub fn new(sample_rate: f64, cutoff_hz: f64) -> Self {
        let mut f = Self {
            g_coeff: 0.5,
            s: [0.0; 2],
        };
        f.set_cutoff(sample_rate, cutoff_hz);
        f
    }

    #[inline]
    pub fn set_cutoff(&mut self, sample_rate: f64, cutoff_hz: f64) {
        let sr = sanitize_sample_rate(sample_rate);
        let fc = sanitize_sample_f64(cutoff_hz).clamp(5.0, sr * 0.45);
        let g = (PI * fc / sr).tan();
        self.g_coeff = g / (1.0 + g);
    }

    #[inline]
    pub fn reset(&mut self) {
        self.s = [0.0; 2];
    }

    /// Whether both channel integrator states have decayed below silence (`1e-7`).
    #[inline]
    pub fn quiet(&self) -> bool {
        const SILENCE: f64 = 1e-7;
        self.s.iter().all(|v| v.abs() < SILENCE)
    }

    #[inline]
    pub fn process_mono(&mut self, input: f64) -> f64 {
        let x = sanitize_sample_f64(input);
        let v = self.g_coeff * (x - self.s[0]);
        let y = v + self.s[0];
        self.s[0] = flush_denormal_f64(y + v);
        y
    }

    #[inline]
    pub fn process_stereo(&mut self, input: [f64; 2]) -> [f64; 2] {
        let mut out = [0.0; 2];
        for ch in 0..2 {
            let x = sanitize_sample_f64(input[ch]);
            let v = self.g_coeff * (x - self.s[ch]);
            let y = v + self.s[ch];
            self.s[ch] = flush_denormal_f64(y + v);
            out[ch] = y;
        }
        out
    }

    /// Exact linear magnitude `|H_LP(jw)|` at `freq_hz` for `cutoff_hz` and `sample_rate`.
    #[inline]
    pub fn magnitude(cutoff_hz: f64, freq_hz: f64, sample_rate: f64) -> f64 {
        let sr = sanitize_sample_rate(sample_rate);
        let fc = sanitize_sample_f64(cutoff_hz).clamp(5.0, sr * 0.45);
        let f = sanitize_sample_f64(freq_hz).clamp(0.0, sr * 0.499_999);
        let u = (PI * f / sr).tan() / (PI * fc / sr).tan().max(1e-12);
        1.0 / (1.0 + u * u).sqrt()
    }

    /// Exact magnitude response in decibels (`-3.01 dB` at `fc`, `0.0 dB` at DC).
    #[inline]
    pub fn magnitude_db(cutoff_hz: f64, freq_hz: f64, sample_rate: f64) -> f64 {
        lin_to_db_f64(Self::magnitude(cutoff_hz, freq_hz, sample_rate))
    }
}

/// Bilinear / Topology-Preserving Transform (TPT) 1-pole high-pass filter (`-3.01 dB` at `fc`,
/// flat `0.0 dB` unity passband up to Nyquist across 44.1 / 48 / 96 kHz).
#[derive(Clone, Copy, Debug)]
pub struct TptOnePoleHpf {
    lpf: TptOnePoleLpf,
}

impl Default for TptOnePoleHpf {
    fn default() -> Self {
        Self::new(48_000.0, 100.0)
    }
}

impl TptOnePoleHpf {
    pub fn new(sample_rate: f64, cutoff_hz: f64) -> Self {
        Self {
            lpf: TptOnePoleLpf::new(sample_rate, cutoff_hz),
        }
    }

    #[inline]
    pub fn set_cutoff(&mut self, sample_rate: f64, cutoff_hz: f64) {
        self.lpf.set_cutoff(sample_rate, cutoff_hz);
    }

    #[inline]
    pub fn reset(&mut self) {
        self.lpf.reset();
    }

    #[inline]
    pub fn process_mono(&mut self, input: f64) -> f64 {
        let x = sanitize_sample_f64(input);
        x - self.lpf.process_mono(x)
    }

    #[inline]
    pub fn process_stereo(&mut self, input: [f64; 2]) -> [f64; 2] {
        let x = [sanitize_sample_f64(input[0]), sanitize_sample_f64(input[1])];
        let lp = self.lpf.process_stereo(x);
        [x[0] - lp[0], x[1] - lp[1]]
    }

    /// Exact linear magnitude `|H_HP(jw)|` at `freq_hz` for `cutoff_hz` and `sample_rate`.
    #[inline]
    pub fn magnitude(cutoff_hz: f64, freq_hz: f64, sample_rate: f64) -> f64 {
        let sr = sanitize_sample_rate(sample_rate);
        let fc = sanitize_sample_f64(cutoff_hz).clamp(5.0, sr * 0.45);
        let f = sanitize_sample_f64(freq_hz).clamp(0.0, sr * 0.499_999);
        let u = (PI * f / sr).tan() / (PI * fc / sr).tan().max(1e-12);
        u / (1.0 + u * u).sqrt()
    }

    /// Exact magnitude response in decibels (`-3.01 dB` at `fc`, `0.0 dB` at Nyquist).
    #[inline]
    pub fn magnitude_db(cutoff_hz: f64, freq_hz: f64, sample_rate: f64) -> f64 {
        lin_to_db_f64(Self::magnitude(cutoff_hz, freq_hz, sample_rate))
    }
}

/// Single clean 12 Hz normalized stereo DC blocker (unity passband gain `(1 + r) / 2`).
#[derive(Clone, Copy, Debug)]
pub struct DcBlocker {
    r: f64,
    norm: f64,
    x1: [f64; 2],
    y1: [f64; 2],
}

impl Default for DcBlocker {
    fn default() -> Self {
        Self::new(48_000.0)
    }
}

impl DcBlocker {
    pub const CUTOFF_HZ: f64 = 12.0;

    pub fn new(sample_rate: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let r = (-TAU * Self::CUTOFF_HZ / sr).exp();
        Self {
            r,
            norm: (1.0 + r) * 0.5,
            x1: [0.0; 2],
            y1: [0.0; 2],
        }
    }

    #[inline]
    pub fn reset(&mut self) {
        self.x1 = [0.0; 2];
        self.y1 = [0.0; 2];
    }

    #[inline]
    pub fn process_mono(&mut self, input: f64) -> f64 {
        let x = sanitize_sample_f64(input);
        let y = x - self.x1[0] + self.r * self.y1[0];
        self.x1[0] = x;
        self.y1[0] = flush_denormal_f64(y);
        self.y1[0] * self.norm
    }

    #[inline]
    pub fn process_stereo(&mut self, input: [f64; 2]) -> [f64; 2] {
        let mut out = [0.0; 2];
        for ch in 0..2 {
            let x = sanitize_sample_f64(input[ch]);
            let y = x - self.x1[ch] + self.r * self.y1[ch];
            self.x1[ch] = x;
            self.y1[ch] = flush_denormal_f64(y);
            out[ch] = self.y1[ch] * self.norm;
        }
        out
    }

    /// Whether both channels have settled below silence (`1e-7`).
    #[inline]
    pub fn quiet(&self) -> bool {
        const SILENCE: f64 = 1e-7;
        self.x1.iter().all(|v| v.abs() < SILENCE) && self.y1.iter().all(|v| v.abs() < SILENCE)
    }
}

/// Compute the 4 RBJ biquad stages for an `EqPatch` at `sample_rate`.
#[inline]
pub fn eq_stage_coeffs(patch: &EqPatch, sample_rate: f64) -> [BiquadCoeffs; 4] {
    let sr = sanitize_sample_rate(sample_rate);
    let hpf = if patch.hpf_hz <= 21.0 {
        BiquadCoeffs::bypass()
    } else {
        BiquadCoeffs::highpass(sr, f64::from(patch.hpf_hz), FRAC_1_SQRT_2)
    };
    let low = BiquadCoeffs::low_shelf(sr, 120.0, f64::from(patch.low_db));
    let mid = BiquadCoeffs::peaking(
        sr,
        f64::from(patch.mid_hz).clamp(160.0, 6_000.0),
        f64::from(patch.mid_q).clamp(0.3, 8.0),
        f64::from(patch.mid_db),
    );
    let high = BiquadCoeffs::high_shelf(sr, 6_000.0, f64::from(patch.high_db));
    [hpf, low, mid, high]
}

/// Exact analytical magnitude response in dB of Cadence's 4-band studio `Eq` at `freq_hz`.
///
/// Single source of truth consumed by BOTH `Eq` verification and `cadence-app::view::effect`.
pub fn eq_response_db(patch: &EqPatch, freq_hz: f64, sample_rate: f64) -> f64 {
    let sr = sanitize_sample_rate(sample_rate);
    let stages = eq_stage_coeffs(patch, sr);
    let mag = stages
        .iter()
        .fold(1.0_f64, |acc, c| acc * c.magnitude(freq_hz, sr));
    lin_to_db_f64(mag)
}

/// 4-band studio parametric EQ (`HPF` + `Low Shelf` + sweepable `Mid Peak` + `High Shelf`)
/// with smooth click-free coefficient interpolation and clean bypass crossfade/reset.
pub struct Eq {
    sample_rate: f64,
    last: EqPatch,
    has_processed: bool,
    hpf: StereoBiquad,
    low: StereoBiquad,
    mid: StereoBiquad,
    high: StereoBiquad,
    bypass_wet: Smoother,
    was_active: bool,
}

impl Eq {
    pub fn new(sample_rate: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let mut eq = Self {
            sample_rate: sr,
            last: EqPatch::default(),
            has_processed: false,
            hpf: StereoBiquad::new(sr),
            low: StereoBiquad::new(sr),
            mid: StereoBiquad::new(sr),
            high: StereoBiquad::new(sr),
            bypass_wet: Smoother::new(1.0, 1.5, sr),
            was_active: false,
        };
        eq.set_parameters(EqPatch::default());
        eq
    }

    pub fn reset(&mut self) {
        self.hpf.reset();
        self.low.reset();
        self.mid.reset();
        self.high.reset();
        self.bypass_wet.snap(1.0);
        self.has_processed = false;
        self.was_active = false;
    }

    pub fn set_parameters(&mut self, patch: EqPatch) {
        if self.has_processed && patch == self.last {
            return;
        }
        self.last = patch;
        let [hpf, low, mid, high] = eq_stage_coeffs(&patch, self.sample_rate);
        if !self.has_processed {
            self.hpf.snap_coeffs(hpf);
            self.low.snap_coeffs(low);
            self.mid.snap_coeffs(mid);
            self.high.snap_coeffs(high);
        } else {
            self.hpf.set_coeffs(hpf);
            self.low.set_coeffs(low);
            self.mid.set_coeffs(mid);
            self.high.set_coeffs(high);
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
        let wet_mix = self.bypass_wet.next();
        if bypassed && wet_mix <= 1e-4 {
            self.bypass_wet.snap(0.0);
            self.hpf.reset();
            self.low.reset();
            self.mid.reset();
            self.high.reset();
            self.was_active = false;
            return dry;
        }
        self.was_active = true;

        let mut s = [f64::from(dry[0]), f64::from(dry[1])];
        s = self.hpf.process(s);
        s = self.low.process(s);
        s = self.mid.process(s);
        s = self.high.process(s);

        if wet_mix >= 1.0 - 1e-4 {
            [s[0] as f32, s[1] as f32]
        } else {
            let dry_mix = 1.0 - wet_mix;
            [
                (f64::from(dry[0]) * dry_mix + s[0] * wet_mix) as f32,
                (f64::from(dry[1]) * dry_mix + s[1] * wet_mix) as f32,
            ]
        }
    }
}

impl crate::dsp::StereoEffect for Eq {
    #[inline]
    fn process(&mut self, input: [f32; 2], bypassed: bool) -> [f32; 2] {
        Eq::process(self, input, bypassed)
    }

    #[inline]
    fn reset(&mut self) {
        Eq::reset(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measure_eq_db(patch: EqPatch, freq: f64, sr: f64) -> f64 {
        let mut eq = Eq::new(sr);
        eq.set_parameters(patch);
        eq.reset();
        let warmup = (sr * 0.15) as usize;
        let measure = (sr * 0.10) as usize;
        let mut out_sq = 0.0_f64;
        let mut in_sq = 0.0_f64;
        for i in 0..(warmup + measure) {
            let x = (TAU * freq * i as f64 / sr).sin() as f32 * 0.5;
            let y = eq.process([x, x], false);
            if i >= warmup {
                in_sq += f64::from(x) * f64::from(x);
                out_sq += f64::from(y[0]) * f64::from(y[0]);
            }
        }
        lin_to_db_f64((out_sq / in_sq.max(1e-18)).sqrt())
    }

    #[test]
    fn eq_analytical_response_matches_dsp_across_sample_rates() {
        let patch = EqPatch {
            hpf_hz: 120.0,
            low_db: 4.5,
            mid_db: -6.0,
            mid_hz: 1450.0,
            mid_q: 2.2,
            high_db: 5.0,
        };
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            for freq in [60.0, 120.0, 350.0, 1450.0, 4000.0, 10_000.0] {
                let predicted = eq_response_db(&patch, freq, sr);
                let measured = measure_eq_db(patch, freq, sr);
                assert!(
                    (predicted - measured).abs() < 0.35,
                    "sr={sr} freq={freq}: predicted={predicted:.2} dB vs measured={measured:.2} dB"
                );
            }
        }
    }

    #[test]
    fn tpt_one_pole_hpf_has_flat_unity_nyquist_passband_across_sample_rates() {
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            let fc = 6_000.0;
            // Analytical check at fc (-3.01 dB) and near Nyquist (0.0 dB).
            let at_fc = TptOnePoleHpf::magnitude_db(fc, fc, sr);
            assert!(
                (at_fc + 3.0103).abs() < 0.02,
                "sr={sr}: HPF at fc = {at_fc} dB"
            );
            let near_nyquist = TptOnePoleHpf::magnitude_db(fc, sr * 0.48, sr);
            assert!(
                near_nyquist.abs() < 0.05,
                "sr={sr}: HPF near Nyquist = {near_nyquist} dB (expected ~0 dB)"
            );

            // Sine-sweep DSP verification at fc and 0.44 * sr.
            for (freq, tol) in [(fc, 0.25), (sr * 0.44, 0.25)] {
                let mut hpf = TptOnePoleHpf::new(sr, fc);
                let warmup = (sr * 0.05) as usize;
                let measure = (sr * 0.05) as usize;
                let mut in_sq = 0.0;
                let mut out_sq = 0.0;
                for i in 0..(warmup + measure) {
                    let x = (TAU * freq * i as f64 / sr).sin();
                    let y = hpf.process_mono(x);
                    if i >= warmup {
                        in_sq += x * x;
                        out_sq += y * y;
                    }
                }
                let measured_db = lin_to_db_f64((out_sq / in_sq).sqrt());
                let predicted_db = TptOnePoleHpf::magnitude_db(fc, freq, sr);
                assert!(
                    (measured_db - predicted_db).abs() < tol,
                    "sr={sr} freq={freq}: measured={measured_db:.2} vs predicted={predicted_db:.2}"
                );
            }
        }
    }

    #[test]
    fn eq_sweeps_and_bypass_remain_finite_and_reset_cleanly() {
        let sr = 48_000.0;
        let mut eq = Eq::new(sr);
        // Flat EQ is bit-transparent.
        assert_eq!(eq.process([0.5, -0.25], false), [0.5, -0.25]);

        // Rapid automated mid_hz / mid_q / hpf_hz sweep stays strictly bounded.
        for i in 0..4800 {
            if i % 32 == 0 {
                let t = i as f32 / 4800.0;
                eq.set_parameters(EqPatch {
                    hpf_hz: 20.0 + t * 500.0,
                    low_db: -6.0 + t * 12.0,
                    mid_db: 9.0,
                    mid_hz: 200.0 + t * 4000.0,
                    mid_q: 0.5 + t * 6.0,
                    high_db: 6.0 - t * 12.0,
                });
            }
            let x = (TAU * 440.0 * i as f64 / sr).sin() as f32 * 0.3;
            let y = eq.process([x, -x], false);
            assert!(y[0].is_finite() && y[0].abs() < 4.0);
        }

        // Bypass settles cleanly and resets stale IIR state.
        for _ in 0..1000 {
            let y = eq.process([0.0, 0.0], true);
            assert!(y[0].is_finite() && y[1].is_finite());
        }
        assert_eq!(eq.process([0.4, -0.4], true), [0.4, -0.4]);
    }
}
