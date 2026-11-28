//! Shared cubic and zero-delay-feedback (TPT/ZDF) multi-mode filter (`MultiModeFilter`),
//! State Variable Filter (`Svf`), exact analytical complex transfer functions (including
//! parallel dry/wet mix), Diode Ladder accent modulation, and Cadence's rhythmic stereo
//! `RhythmicFilter` insert processor.

use crate::dsp::biquad::{DcBlocker, TptOnePoleLpf};
use crate::dsp::cubic::filter::{
    DiodeLadderFilter, ImLpfCascade, ImLpfPoles, DEFAULT_HPF_HZ, Q_BUTTER_4A, Q_BUTTER_4B,
};
use crate::dsp::cubic::math::atomic_jet;
use crate::dsp::env::RhythmicEnv;
use crate::dsp::math::{
    clamp_freq_hz, control_stride_last_tick, control_stride_mask, flush_denormal_f64,
    lin_to_db_f64, mix_stereo_f64, one_pole_coeff_ms, sanitize_clamp_f64, sanitize_frame,
    sanitize_sample, sanitize_sample_f64, sanitize_sample_rate, BypassFader, BypassStep,
    LogSmoother, Smoother, LIN_FLOOR_F64, NYQUIST_SAFE_RATIO, TAIL_SILENCE_FLOOR_F64,
};
use crate::dsp::saturate::SoftClipCoeffs;
use crate::filter_patterns::CompiledFilterPattern;
use crate::mixer::FilterPatch;
use crate::rack::FilterMode;
use std::f64::consts::{FRAC_1_SQRT_2, LN_2, PI};

/// Velocity threshold above which Diode Ladder accent engages.
/// Sits cleanly above Cadence's default `Step.velocity = 0.80` so normal notes keep
/// their authored `filter_decay_ms`, while accented (`>= 0.90` / `1.0`) hits kick in crisply.
pub const ACID_ACCENT_THRESHOLD: f64 = 0.85;
/// Maximum native `ModuleParam::VelAmount` (`4.0` octaves), used to normalize
/// Diode Ladder accent resonance boost across the full parameter range.
pub const ACID_FILTER_VEL_MAX: f64 = 4.0;

/// Normalized Diode Ladder accent strength in `[0.0, 1.0]` for `velocity`.
/// Returns `0.0` at or below `ACID_ACCENT_THRESHOLD` (`0.85`) and ramps linearly to `1.0` at `1.0`.
#[inline]
pub fn acid_accent_amount(velocity: f64) -> f64 {
    if !velocity.is_finite() {
        return 0.0;
    }
    ((velocity - ACID_ACCENT_THRESHOLD) / (1.0 - ACID_ACCENT_THRESHOLD)).clamp(0.0, 1.0)
}

/// Effective filter envelope decay time (ms) for Diode Ladder voices.
/// Accented hits (`velocity > ACID_ACCENT_THRESHOLD` with `filter_vel_amount > 0`) snap
/// decay shorter without ever lengthening an already-short decay; default (`0.80`) and
/// non-accented notes leave `filter_decay_ms` untouched.
#[inline]
pub fn acid_filter_decay_ms(
    mode: FilterMode,
    filter_decay_ms: f32,
    filter_vel_amount: f32,
    velocity: f32,
) -> f32 {
    if mode == FilterMode::DiodeLadder
        && filter_vel_amount > 0.0
        && acid_accent_amount(f64::from(velocity)) > 1e-4
    {
        (filter_decay_ms * 0.48)
            .clamp(42.0, 210.0)
            .min(filter_decay_ms)
    } else {
        filter_decay_ms
    }
}

/// Filter cutoff octave offset and effective resonance contributed by velocity / accent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AcidAccentMod {
    pub cutoff_octaves: f64,
    pub resonance: f32,
}

/// Compute velocity-driven filter cutoff octave boost and effective resonance for any `FilterMode`.
///
/// - `FilterMode::DiodeLadder` with `filter_vel_amount > 0`: applies the smooth above-threshold
///   [`acid_accent_amount`] "wow" curve and boosts resonance normalized by [`ACID_FILTER_VEL_MAX`].
/// - Other modes: responds smoothly across the full `0.0..=1.0` velocity range without altering
///   resonance.
#[inline]
pub fn filter_velocity_mod(
    mode: FilterMode,
    velocity: f64,
    filter_envelope: f64,
    filter_vel_amount: f32,
    base_resonance: f32,
) -> AcidAccentMod {
    let vel = if velocity.is_finite() {
        velocity.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let env = if filter_envelope.is_finite() {
        filter_envelope.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let vel_amt = if filter_vel_amount.is_finite() {
        f64::from(filter_vel_amount)
    } else {
        0.0
    };
    let base_res = if base_resonance.is_finite() {
        base_resonance.clamp(0.0, 0.95)
    } else {
        0.0
    };
    if mode == FilterMode::DiodeLadder && vel_amt > 0.0 {
        let accent = acid_accent_amount(vel);
        let wow = env * (1.35 - 0.35 * env) + 0.18;
        let cutoff_octaves = accent * vel_amt * wow;
        let knob = (vel_amt / ACID_FILTER_VEL_MAX).clamp(0.0, 1.0);
        let boost = (accent * knob * (0.28 + 0.72 * env) * 0.14) as f32;
        AcidAccentMod {
            cutoff_octaves,
            resonance: (base_res + boost).clamp(0.0, 0.95),
        }
    } else {
        let cutoff_octaves = if vel_amt != 0.0 {
            vel * vel_amt * (env + 0.25)
        } else {
            0.0
        };
        AcidAccentMod {
            cutoff_octaves,
            resonance: base_res,
        }
    }
}

/// Precomputed 2-pole TPT/ZDF State Variable Filter coefficients.
#[derive(Clone, Copy, Debug)]
pub struct SvfCoeffs {
    pub a1: f64,
    pub a2: f64,
    pub a3: f64,
    pub k: f64,
}

impl SvfCoeffs {
    #[inline]
    pub fn new(sample_rate: f64, cutoff_hz: f64, resonance: f32) -> Self {
        let res = f64::from(if resonance.is_finite() {
            resonance.clamp(0.0, 0.95)
        } else {
            0.0
        });
        Self::with_damping(sample_rate, cutoff_hz, 2.0 - 1.98 * res)
    }

    /// Construct 2-pole TPT/ZDF State Variable Filter coefficients directly from a
    /// continuous-time damping factor `k = 1 / Q` (e.g. Butterworth `k = sqrt(2)`).
    #[inline]
    pub fn with_damping(sample_rate: f64, cutoff_hz: f64, k: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let fc = clamp_freq_hz(cutoff_hz, 20.0, sr, 20.0);
        let k = if k.is_finite() {
            k.clamp(0.05, 4.0)
        } else {
            std::f64::consts::SQRT_2
        };
        let g = (PI * fc / sr).tan();
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        Self {
            a1,
            a2,
            a3: g * a2,
            k,
        }
    }
}

/// Stereo/mono 2-pole TPT/ZDF State Variable Filter (`f64`, denormal flush, NaN guard).
#[derive(Clone, Copy, Debug, Default)]
pub struct Svf {
    ic1: [f64; 2],
    ic2: [f64; 2],
}

impl Svf {
    #[inline]
    pub fn reset(&mut self) {
        self.ic1 = [0.0; 2];
        self.ic2 = [0.0; 2];
    }

    #[inline]
    fn step_channel_all(&mut self, ch: usize, input: f64, coeff: SvfCoeffs) -> (f64, f64, f64) {
        let x = sanitize_sample_f64(input);
        let v3 = x - self.ic2[ch];
        let band = coeff.a1 * self.ic1[ch] + coeff.a2 * v3;
        let low = self.ic2[ch] + coeff.a2 * self.ic1[ch] + coeff.a3 * v3;
        self.ic1[ch] = flush_denormal_f64(2.0 * band - self.ic1[ch]);
        self.ic2[ch] = flush_denormal_f64(2.0 * low - self.ic2[ch]);
        let high = x - coeff.k * band - low;
        (low, band, high)
    }

    #[inline]
    fn pick_output(mode: FilterMode, outputs: (f64, f64, f64)) -> f64 {
        let (low, band, high) = outputs;
        match mode {
            FilterMode::BandPass => band,
            FilterMode::HighPass => high,
            FilterMode::ImLpf2
            | FilterMode::ImLpf3
            | FilterMode::ImLpf4
            | FilterMode::DiodeLadder
            | FilterMode::LowPass6 => low,
        }
    }

    #[inline]
    fn step_channel(&mut self, ch: usize, input: f64, coeff: SvfCoeffs, mode: FilterMode) -> f64 {
        Self::pick_output(mode, self.step_channel_all(ch, input, coeff))
    }

    #[inline]
    pub fn process_mono(&mut self, input: f64, coeff: SvfCoeffs, mode: FilterMode) -> f64 {
        self.step_channel(0, input, coeff, mode)
    }

    #[inline]
    pub fn process_stereo(
        &mut self,
        input: [f64; 2],
        coeff: SvfCoeffs,
        mode: FilterMode,
    ) -> [f64; 2] {
        [
            self.step_channel(0, input[0], coeff, mode),
            self.step_channel(1, input[1], coeff, mode),
        ]
    }

    /// Whether both integrator states on both channels have decayed below silence (`1e-7`).
    #[inline]
    pub fn quiet(&self) -> bool {
        self.ic1.iter().all(|v| v.abs() < TAIL_SILENCE_FLOOR_F64)
            && self.ic2.iter().all(|v| v.abs() < TAIL_SILENCE_FLOOR_F64)
    }
}

#[inline]
fn biquad_lp(u: f64, q: f64) -> (f64, f64) {
    let d_re = 1.0 - u * u;
    let d_im = u / q.max(1e-6);
    let denom_sq = (d_re * d_re + d_im * d_im).max(1e-24);
    (d_re / denom_sq, -d_im / denom_sq)
}

#[inline]
fn complex_mul(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
}

/// Exact small-signal complex transfer function `H(jω) = (re, im)` of the continuous-time
/// Butterworth/resonant `Im(z)` low-pass cascade (`ImLpf2`, `ImLpf3`, `ImLpf4`).
///
/// Guarantees exact `0 dB` DC gain, `-3.01 dB` at `cutoff_hz` when `resonance == 0`,
/// and exact `-12 / -18 / -24 dB/oct` stopband slopes.
#[inline]
pub fn im_lpf_transfer(
    poles: ImLpfPoles,
    cutoff_hz: f32,
    resonance: f32,
    freq_hz: f64,
    sample_rate: f64,
) -> (f64, f64) {
    let sr = sanitize_sample_rate(sample_rate);
    let fc = clamp_freq_hz(f64::from(cutoff_hz), 20.0, sr, 1_000.0);
    let f = sanitize_clamp_f64(freq_hz, 0.0, sr * 0.499, 0.0);
    let res = sanitize_clamp_f64(f64::from(resonance), 0.0, 0.95, 0.0);
    let u = f / fc;
    match poles {
        ImLpfPoles::Two => biquad_lp(u, FRAC_1_SQRT_2 * 14.0_f64.powf(res)),
        ImLpfPoles::Three => {
            let s1 = biquad_lp(u, 12.0_f64.powf(res));
            let denom = (1.0 + u * u).max(1e-24);
            complex_mul(s1, (1.0 / denom, -u / denom))
        }
        ImLpfPoles::Four => {
            let s1 = biquad_lp(u, Q_BUTTER_4A * 10.5_f64.powf(res));
            let s2 = biquad_lp(u, Q_BUTTER_4B);
            complex_mul(s1, s2)
        }
    }
}

/// Resonance-coupled output recovery gain for the passive 4-stage `C_1 = 2C` diode ladder.
///
/// The passive ladder's midband transmission falls as `1 / (1 + k)` (`k = 16.8 * res`), so a
/// post-ladder recovery stage (`+13.0 dB` at `res = 0.74`, unity at `res = 0`) restores line-level
/// parity with `ImLpf2/3/4` without perturbing the ladder's physical saturation or bark thresholds.
#[inline]
pub fn diode_ladder_output_gain(resonance: f64) -> f64 {
    let res = sanitize_clamp_f64(resonance, 0.0, 0.95, 0.0);
    1.0 + 0.26 * (16.8 * res)
}

/// Exact small-signal complex transfer function `H(jω) = (re, im)` of the 4-stage `C_1 = 2C`
/// passive diode RC ladder (`k = 16.8 * res`) with post-saturation `72.3 Hz` (`C17`) feedback HPF
/// and [`diode_ladder_output_gain`] recovery stage.
#[inline]
pub fn diode_ladder_transfer(
    cutoff_hz: f32,
    resonance: f32,
    freq_hz: f64,
    sample_rate: f64,
) -> (f64, f64) {
    let sr = sanitize_sample_rate(sample_rate);
    let fc = clamp_freq_hz(f64::from(cutoff_hz), 20.0, sr, 1_000.0);
    let f = sanitize_clamp_f64(freq_hz, 0.0, sr * 0.499, 0.0);
    let res = sanitize_clamp_f64(f64::from(resonance), 0.0, 0.95, 0.0);
    let tan_f = (PI * f / sr).tan();
    let u = tan_f / (PI * fc / sr).tan().max(1e-12);
    let u2 = u * u;
    let u4 = u2 * u2;

    let u_h = tan_f / (PI * DEFAULT_HPF_HZ / sr).tan().max(1e-12);
    let h_denom = 1.0 + u_h * u_h;
    let h_re = (u_h * u_h) / h_denom;
    let h_im = u_h / h_denom;

    let k = 16.8 * res;
    let d_re = (1.0 - 21.0 * u2 + 2.0 * u4) + k * h_re;
    let d_im = (11.0 * u - 12.0 * u * u2) + k * h_im;
    let denom_sq = (d_re * d_re + d_im * d_im).max(1e-24);
    let gain = diode_ladder_output_gain(res);
    (gain * d_re / denom_sq, -gain * d_im / denom_sq)
}

/// Exact complex transfer function `H(jω) = (re, im)` of the TPT/ZDF State Variable Filter.
#[inline]
pub fn svf_transfer(
    mode: FilterMode,
    cutoff_hz: f32,
    resonance: f32,
    freq_hz: f64,
    sample_rate: f64,
) -> (f64, f64) {
    let sr = sanitize_sample_rate(sample_rate);
    let fc = clamp_freq_hz(f64::from(cutoff_hz), 20.0, sr, 1_000.0);
    let f = sanitize_clamp_f64(freq_hz, 0.0, sr * 0.499, 0.0);
    let u = (PI * f / sr).tan() / (PI * fc / sr).tan().max(1e-12);
    let res = sanitize_clamp_f64(f64::from(resonance), 0.0, 0.95, 0.0);
    let k = 2.0 - 1.98 * res;
    let d_re = 1.0 - u * u;
    let d_im = k * u;
    let denom_sq = (d_re * d_re + d_im * d_im).max(1e-24);
    match mode {
        FilterMode::BandPass => (u * d_im / denom_sq, u * d_re / denom_sq),
        FilterMode::HighPass => (-u * u * d_re / denom_sq, u * u * d_im / denom_sq),
        FilterMode::ImLpf2
        | FilterMode::ImLpf3
        | FilterMode::ImLpf4
        | FilterMode::DiodeLadder
        | FilterMode::LowPass6 => (d_re / denom_sq, -d_im / denom_sq),
    }
}

/// Exact complex transfer function `H(jω) = (re, im)` of the bilinear/TPT 1-pole
/// `6 dB/oct` low-pass filter (`H(s) = 1 / (1 + s/ωc)`).
#[inline]
pub fn one_pole_lpf_transfer(cutoff_hz: f32, freq_hz: f64, sample_rate: f64) -> (f64, f64) {
    let sr = sanitize_sample_rate(sample_rate);
    let fc = clamp_freq_hz(f64::from(cutoff_hz), 20.0, sr, 1_000.0);
    let f = sanitize_clamp_f64(freq_hz, 0.0, sr * 0.499, 0.0);
    let u = (PI * f / sr).tan() / (PI * fc / sr).tan().max(1e-12);
    let denom = (1.0 + u * u).max(1e-24);
    (1.0 / denom, -u / denom)
}

/// Exact complex transfer function `H(jω) = (re, im)` for any `FilterMode`.
#[inline]
pub fn filter_transfer(
    mode: FilterMode,
    cutoff_hz: f32,
    resonance: f32,
    freq_hz: f64,
    sample_rate: f64,
) -> (f64, f64) {
    match mode {
        FilterMode::ImLpf2 => {
            im_lpf_transfer(ImLpfPoles::Two, cutoff_hz, resonance, freq_hz, sample_rate)
        }
        FilterMode::ImLpf3 => im_lpf_transfer(
            ImLpfPoles::Three,
            cutoff_hz,
            resonance,
            freq_hz,
            sample_rate,
        ),
        FilterMode::ImLpf4 => {
            im_lpf_transfer(ImLpfPoles::Four, cutoff_hz, resonance, freq_hz, sample_rate)
        }
        FilterMode::DiodeLadder => {
            diode_ladder_transfer(cutoff_hz, resonance, freq_hz, sample_rate)
        }
        FilterMode::BandPass | FilterMode::HighPass => {
            svf_transfer(mode, cutoff_hz, resonance, freq_hz, sample_rate)
        }
        FilterMode::LowPass6 => one_pole_lpf_transfer(cutoff_hz, freq_hz, sample_rate),
    }
}

/// Exact linear magnitude `|H(jω)|` for any `FilterMode`.
#[inline]
pub fn filter_magnitude(
    mode: FilterMode,
    cutoff_hz: f32,
    resonance: f32,
    freq_hz: f64,
    sample_rate: f64,
) -> f64 {
    let (re, im) = filter_transfer(mode, cutoff_hz, resonance, freq_hz, sample_rate);
    re.hypot(im)
}

/// Exact magnitude response in decibels for any `FilterMode` (`-120 dB` floor).
#[inline]
pub fn filter_magnitude_db(
    mode: FilterMode,
    cutoff_hz: f32,
    resonance: f32,
    freq_hz: f64,
    sample_rate: f64,
) -> f64 {
    lin_to_db_f64(filter_magnitude(
        mode,
        cutoff_hz,
        resonance,
        freq_hz,
        sample_rate,
    ))
}

/// Exact parallel dry/wet magnitude response in decibels: `|(1 - mix) + mix * H(jω)|`,
/// accounting for complex phase interaction between dry and filtered paths.
#[inline]
pub fn filter_mix_magnitude_db(
    mode: FilterMode,
    cutoff_hz: f32,
    resonance: f32,
    mix: f32,
    freq_hz: f64,
    sample_rate: f64,
) -> f64 {
    let m = sanitize_clamp_f64(f64::from(mix), 0.0, 1.0, 1.0);
    let (h_re, h_im) = filter_transfer(mode, cutoff_hz, resonance, freq_hz, sample_rate);
    let re = (1.0 - m) + m * h_re;
    let im = m * h_im;
    lin_to_db_f64(re.hypot(im))
}

/// Unified zero-allocation stereo multi-mode filter composing Cadence's four
/// filter topologies (`ImLpfCascade`, `DiodeLadderFilter`, `Svf`, and `TptOnePoleLpf`).
///
/// Serves as the single real-time processing core shared by both [`RhythmicFilter`]
/// and modular Rack voice filter nodes.
#[derive(Clone, Copy, Debug)]
pub struct MultiModeFilter {
    sample_rate: f64,
    svf: Svf,
    one_pole: TptOnePoleLpf,
    im_lpf: [ImLpfCascade; 2],
    diode: [DiodeLadderFilter; 2],
    svf_coeffs: SvfCoeffs,
    cached_svf_key: (u64, u32),
    cached_one_pole_bits: u64,
    cached_drive_bits: u64,
    cached_drive_coeffs: SoftClipCoeffs,
}

impl MultiModeFilter {
    /// Create a zeroed stereo multi-mode filter for `sample_rate` Hz.
    pub fn new(sample_rate: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let init_fc = clamp_freq_hz(1_000.0, 20.0, sr, 1_000.0);
        Self {
            sample_rate: sr,
            svf: Svf::default(),
            one_pole: TptOnePoleLpf::new(sr, init_fc),
            im_lpf: [
                ImLpfCascade::new(sr, ImLpfPoles::Two),
                ImLpfCascade::new(sr, ImLpfPoles::Two),
            ],
            diode: [
                DiodeLadderFilter::new(sr, DEFAULT_HPF_HZ),
                DiodeLadderFilter::new(sr, DEFAULT_HPF_HZ),
            ],
            svf_coeffs: SvfCoeffs::new(sr, init_fc, 0.0),
            cached_svf_key: (init_fc.to_bits(), 0.0_f32.to_bits()),
            cached_one_pole_bits: init_fc.to_bits(),
            cached_drive_bits: 0.0_f64.to_bits(),
            cached_drive_coeffs: SoftClipCoeffs::new(0.0),
        }
    }

    /// Map a [`FilterMode`] to its owning internal topology index:
    /// - `0`: SVF (`BandPass`, `HighPass`)
    /// - `1`: Driven complex `Im(z)` cascade (`ImLpf2`, `ImLpf3`, `ImLpf4`)
    /// - `2`: Passive diode RC ladder (`DiodeLadder`)
    /// - `3`: 1-pole TPT low-pass (`LowPass6`)
    #[inline]
    pub const fn topology_id(mode: FilterMode) -> u8 {
        match mode {
            FilterMode::BandPass | FilterMode::HighPass => 0,
            FilterMode::ImLpf2 | FilterMode::ImLpf3 | FilterMode::ImLpf4 => 1,
            FilterMode::DiodeLadder => 2,
            FilterMode::LowPass6 => 3,
        }
    }

    /// Reset all four internal filter topologies to zero.
    #[inline]
    pub fn reset(&mut self) {
        self.svf.reset();
        self.one_pole.reset();
        self.im_lpf[0].reset();
        self.im_lpf[1].reset();
        self.diode[0].reset();
        self.diode[1].reset();
    }

    /// Reset only the internal filter topology owning `mode`.
    #[inline]
    pub fn reset_topology_for(&mut self, mode: FilterMode) {
        match Self::topology_id(mode) {
            0 => self.svf.reset(),
            1 => {
                self.im_lpf[0].reset();
                self.im_lpf[1].reset();
            }
            2 => {
                self.diode[0].reset();
                self.diode[1].reset();
            }
            _ => self.one_pole.reset(),
        }
    }

    /// Whether all four internal topologies have decayed below silence (`1e-7` amplitude / `1e-14` energy).
    #[inline]
    pub fn quiet(&self) -> bool {
        self.svf.quiet()
            && self.one_pole.quiet()
            && self.im_lpf.iter().all(ImLpfCascade::quiet)
            && self
                .diode
                .iter()
                .all(|f| f.energy() < 1e-14 && f.dc.abs() < TAIL_SILENCE_FLOOR_F64)
    }

    #[inline]
    fn apply_soft_clip(&mut self, x: [f64; 2], drv: f64) -> [f64; 2] {
        if drv <= 0.0 {
            return x;
        }
        let bits = drv.to_bits();
        if bits != self.cached_drive_bits {
            self.cached_drive_bits = bits;
            self.cached_drive_coeffs = SoftClipCoeffs::new(drv);
        }
        [
            self.cached_drive_coeffs.shape(x[0]),
            self.cached_drive_coeffs.shape(x[1]),
        ]
    }

    #[inline]
    fn ensure_svf_coeffs(&mut self, fc: f64, res: f64) {
        let res_f32 = res as f32;
        let key = (fc.to_bits(), res_f32.to_bits());
        if key != self.cached_svf_key {
            self.cached_svf_key = key;
            self.svf_coeffs = SvfCoeffs::new(self.sample_rate, fc, res_f32);
        }
    }

    #[inline]
    fn ensure_one_pole_cutoff(&mut self, fc: f64) {
        let bits = fc.to_bits();
        if bits != self.cached_one_pole_bits {
            self.cached_one_pole_bits = bits;
            self.one_pole.set_cutoff(self.sample_rate, fc);
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[inline]
    fn step_im_lpf(
        &mut self,
        x: [f64; 2],
        poles: ImLpfPoles,
        fc: f64,
        res: f64,
        drv: f64,
        kerr_semitones: f64,
        spread: f64,
    ) -> [f64; 2] {
        if self.im_lpf[0].poles() != poles {
            if self.im_lpf[0].quiet() && self.im_lpf[1].quiet() {
                self.im_lpf[0].reset();
                self.im_lpf[1].reset();
            }
            self.im_lpf[0].set_poles(poles);
            self.im_lpf[1].set_poles(poles);
        }
        let kerr = sanitize_clamp_f64(kerr_semitones, -12.0, 12.0, 0.0);
        let beta = 2.0_f64.powf(kerr / 12.0) - 1.0;
        let drag = 0.65 * drv;
        let y_l = self.im_lpf[0].step(x[0], fc, res, beta, drag);
        let y_r = self.im_lpf[1].step(x[1], fc, res, beta, drag);
        let sp = sanitize_clamp_f64(spread, 0.0, 1.0, 0.0);
        if sp <= 0.0 {
            [y_l, y_r]
        } else {
            let angle = 0.5 * PI * sp;
            let (s, c) = (0.5 * angle).sin_cos();
            let q_mid = 0.5 * (self.im_lpf[0].quad_x + self.im_lpf[1].quad_x);
            [c * y_l + s * q_mid, c * y_r - s * q_mid]
        }
    }

    /// Process one stereo sample frame through the selected [`FilterMode`].
    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub fn process_stereo(
        &mut self,
        input: [f64; 2],
        mode: FilterMode,
        cutoff_hz: f64,
        resonance: f32,
        drive: f64,
        kerr_semitones: f64,
        spread: f64,
    ) -> [f64; 2] {
        let x = [sanitize_sample_f64(input[0]), sanitize_sample_f64(input[1])];
        let fc = clamp_freq_hz(cutoff_hz, 20.0, self.sample_rate, 1_000.0);
        let res = sanitize_clamp_f64(f64::from(resonance), 0.0, 0.95, 0.0);
        let drv = sanitize_clamp_f64(drive, 0.0, 1.0, 0.0);

        match mode {
            FilterMode::ImLpf2 => {
                self.step_im_lpf(x, ImLpfPoles::Two, fc, res, drv, kerr_semitones, spread)
            }
            FilterMode::ImLpf3 => {
                self.step_im_lpf(x, ImLpfPoles::Three, fc, res, drv, kerr_semitones, spread)
            }
            FilterMode::ImLpf4 => {
                self.step_im_lpf(x, ImLpfPoles::Four, fc, res, drv, kerr_semitones, spread)
            }
            FilterMode::DiodeLadder => {
                let u_gain = 1.0 + 1.0 * drv;
                let out_gain = diode_ladder_output_gain(res);
                [
                    atomic_jet(
                        self.diode[0].step(x[0] * u_gain, fc, res, drv) * out_gain,
                        0.98,
                        0.0,
                    ),
                    atomic_jet(
                        self.diode[1].step(x[1] * u_gain, fc, res, drv) * out_gain,
                        0.98,
                        0.0,
                    ),
                ]
            }
            FilterMode::BandPass | FilterMode::HighPass => {
                let driven = self.apply_soft_clip(x, drv);
                self.ensure_svf_coeffs(fc, res);
                self.svf.process_stereo(driven, self.svf_coeffs, mode)
            }
            FilterMode::LowPass6 => {
                let driven = self.apply_soft_clip(x, drv);
                self.ensure_one_pole_cutoff(fc);
                self.one_pole.process_stereo(driven)
            }
        }
    }

    /// Process one stereo sample frame while click-free crossfading from `prev_mode` to `mode`
    /// using normalized weight `xfade` in `[0.0, 1.0]`.
    ///
    /// - Same-SVF transitions (`BandPass <-> HighPass`) perform a single shared topology step.
    /// - Same-`ImLpf` pole transitions preserve continuous Stage 1 state cleanly.
    /// - Cross-topology transitions run both independent topologies in parallel and blend.
    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub fn process_stereo_crossfade(
        &mut self,
        input: [f64; 2],
        prev_mode: FilterMode,
        mode: FilterMode,
        xfade: f64,
        cutoff_hz: f64,
        resonance: f32,
        drive: f64,
        kerr_semitones: f64,
        spread: f64,
    ) -> [f64; 2] {
        let xf = sanitize_clamp_f64(xfade, 0.0, 1.0, 1.0);
        if xf >= 1.0 - LIN_FLOOR_F64 || prev_mode == mode {
            return self.process_stereo(
                input,
                mode,
                cutoff_hz,
                resonance,
                drive,
                kerr_semitones,
                spread,
            );
        }

        let prev_topo = Self::topology_id(prev_mode);
        let next_topo = Self::topology_id(mode);
        let inv = 1.0 - xf;

        if prev_topo == 0 && next_topo == 0 {
            let x = [sanitize_sample_f64(input[0]), sanitize_sample_f64(input[1])];
            let fc = clamp_freq_hz(cutoff_hz, 20.0, self.sample_rate, 1_000.0);
            let res = sanitize_clamp_f64(f64::from(resonance), 0.0, 0.95, 0.0);
            let drv = sanitize_clamp_f64(drive, 0.0, 1.0, 0.0);
            let driven = self.apply_soft_clip(x, drv);
            self.ensure_svf_coeffs(fc, res);
            let mut out = [0.0; 2];
            for ch in 0..2 {
                let taps = self.svf.step_channel_all(ch, driven[ch], self.svf_coeffs);
                let prev_y = Svf::pick_output(prev_mode, taps);
                let next_y = Svf::pick_output(mode, taps);
                out[ch] = prev_y * inv + next_y * xf;
            }
            out
        } else if prev_topo == 1 && next_topo == 1 {
            let next_out = self.process_stereo(
                input,
                mode,
                cutoff_hz,
                resonance,
                drive,
                kerr_semitones,
                spread,
            );
            if prev_mode == FilterMode::ImLpf2 {
                let stage1_out = [self.im_lpf[0].y0, self.im_lpf[1].y0];
                [
                    stage1_out[0] * inv + next_out[0] * xf,
                    stage1_out[1] * inv + next_out[1] * xf,
                ]
            } else {
                next_out
            }
        } else {
            let prev_out = self.process_stereo(
                input,
                prev_mode,
                cutoff_hz,
                resonance,
                drive,
                kerr_semitones,
                spread,
            );
            let next_out = self.process_stereo(
                input,
                mode,
                cutoff_hz,
                resonance,
                drive,
                kerr_semitones,
                spread,
            );
            [
                prev_out[0] * inv + next_out[0] * xf,
                prev_out[1] * inv + next_out[1] * xf,
            ]
        }
    }
}

/// Stereo resonant multi-mode rhythmic filter insert processor composing
/// [`MultiModeFilter`], [`RhythmicEnv`], [`DcBlocker`], smooth parameter slewing
/// (cutoff, resonance, drive, State-Kerr bend, quadrature spread, mix), and
/// click-free mode & bypass crossfades. Zero heap allocations.
pub struct RhythmicFilter {
    sample_rate: f64,
    mode: FilterMode,
    prev_mode: FilterMode,
    mode_xfade: Smoother,
    cutoff: LogSmoother,
    cutoff_coeff: f64,
    resonance: Smoother,
    drive: Smoother,
    kerr: Smoother,
    spread: Smoother,
    mix: Smoother,
    bypass: BypassFader,
    env: RhythmicEnv,
    env_amount_ln2: f64,
    filter: MultiModeFilter,
    dc_blocker: DcBlocker,
    effective_fc: f64,
    cached_fc_key: (u64, u64),
    last_patch: FilterPatch,
    ctrl_count: u32,
    ctrl_mask: u32,
    has_processed: bool,
    #[cfg(test)]
    coeff_recomputes: u64,
}

impl RhythmicFilter {
    pub fn new(sample_rate: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let default = FilterPatch::default();
        let init_fc = clamp_freq_hz(f64::from(default.cutoff_hz), 20.0, sr, 1_000.0);
        let init_drive = sanitize_clamp_f64(f64::from(default.drive), 0.0, 1.0, 0.0);
        let init_kerr = sanitize_clamp_f64(f64::from(default.kerr_semitones), -12.0, 12.0, 0.0);
        let init_spread = sanitize_clamp_f64(f64::from(default.spread), 0.0, 1.0, 0.0);
        let mut f = Self {
            sample_rate: sr,
            mode: default.mode,
            prev_mode: default.mode,
            mode_xfade: Smoother::new(1.0, 3.0, sr),
            cutoff: LogSmoother::new_clamped(init_fc, 20.0, sr * NYQUIST_SAFE_RATIO),
            cutoff_coeff: one_pole_coeff_ms(4.0, sr),
            resonance: Smoother::new(f64::from(default.resonance), 4.0, sr),
            drive: Smoother::new(init_drive, 4.0, sr),
            kerr: Smoother::new(init_kerr, 4.0, sr),
            spread: Smoother::new(init_spread, 4.0, sr),
            mix: Smoother::new(f64::from(default.mix), 4.0, sr),
            bypass: BypassFader::new(sr),
            env: RhythmicEnv::new(sr),
            env_amount_ln2: 0.0,
            filter: MultiModeFilter::new(sr),
            dc_blocker: DcBlocker::new(sr),
            effective_fc: init_fc,
            cached_fc_key: (u64::MAX, u64::MAX),
            last_patch: default,
            ctrl_count: 0,
            ctrl_mask: control_stride_mask(sr, 6_000.0),
            has_processed: false,
            #[cfg(test)]
            coeff_recomputes: 0,
        };
        f.set_parameters(default);
        f
    }

    #[cfg(test)]
    pub(crate) fn is_idle(&self) -> bool {
        self.bypass.is_idle()
    }

    #[cfg(test)]
    pub(crate) fn cutoff_hz_current(&self) -> f64 {
        self.cutoff.current()
    }

    #[cfg(test)]
    pub(crate) fn coeff_recomputes(&self) -> u64 {
        self.coeff_recomputes
    }

    pub fn reset(&mut self) {
        self.filter.reset();
        self.dc_blocker.reset();
        self.env.reset();
        self.cutoff.snap_to_target();
        self.resonance.snap(self.resonance.target());
        self.drive.snap(self.drive.target());
        self.kerr.snap(self.kerr.target());
        self.spread.snap(self.spread.target());
        self.mix.snap(self.mix.target());
        self.prev_mode = self.mode;
        self.mode_xfade.snap(1.0);
        self.bypass.reset();
        self.has_processed = false;
        self.ctrl_count = 0;
        self.update_effective_cutoff(0.0, true);
    }

    fn set_parameters_common(&mut self, patch: FilterPatch) {
        let def = FilterPatch::default();
        let fc = clamp_freq_hz(
            f64::from(patch.cutoff_hz),
            20.0,
            self.sample_rate,
            f64::from(def.cutoff_hz),
        );
        let res = sanitize_clamp_f64(
            f64::from(patch.resonance),
            0.0,
            0.95,
            f64::from(def.resonance),
        );
        let drv = sanitize_clamp_f64(f64::from(patch.drive), 0.0, 1.0, f64::from(def.drive));
        let kerr = sanitize_clamp_f64(
            f64::from(patch.kerr_semitones),
            -12.0,
            12.0,
            f64::from(def.kerr_semitones),
        );
        let spread = sanitize_clamp_f64(f64::from(patch.spread), 0.0, 1.0, f64::from(def.spread));
        let mix = sanitize_clamp_f64(f64::from(patch.mix), 0.0, 1.0, f64::from(def.mix));
        let amt = sanitize_clamp_f64(
            f64::from(patch.env_amount),
            -4.0,
            4.0,
            f64::from(def.env_amount),
        );
        let env_decay_ms = sanitize_clamp_f64(
            f64::from(patch.env_decay_ms),
            15.0,
            1500.0,
            f64::from(def.env_decay_ms),
        );
        let sanitized = FilterPatch {
            mode: patch.mode,
            cutoff_hz: fc as f32,
            resonance: res as f32,
            drive: drv as f32,
            kerr_semitones: kerr as f32,
            spread: spread as f32,
            mix: mix as f32,
            env_pattern: patch.env_pattern,
            env_pattern_id: patch.env_pattern_id,
            env_amount: amt as f32,
            env_decay_ms: env_decay_ms as f32,
        };
        if self.has_processed && sanitized == self.last_patch {
            return;
        }
        self.last_patch = sanitized;
        self.env_amount_ln2 = if amt.abs() < 1e-4 { 0.0 } else { amt * LN_2 };

        if !self.has_processed {
            self.mode = patch.mode;
            self.prev_mode = patch.mode;
            self.mode_xfade.snap(1.0);
            self.cutoff.snap(fc);
            self.resonance.snap(res);
            self.drive.snap(drv);
            self.kerr.snap(kerr);
            self.spread.snap(spread);
            self.mix.snap(mix);
            self.update_effective_cutoff(self.env.current(), true);
        } else {
            if patch.mode != self.mode {
                let prev_topo = MultiModeFilter::topology_id(self.mode);
                let next_topo = MultiModeFilter::topology_id(patch.mode);
                if self.prev_mode != self.mode {
                    let abandoned_topo = MultiModeFilter::topology_id(self.prev_mode);
                    if abandoned_topo != prev_topo && abandoned_topo != next_topo {
                        self.filter.reset_topology_for(self.prev_mode);
                    }
                }
                if prev_topo != next_topo || (next_topo == 1 && self.filter.quiet()) {
                    self.filter.reset_topology_for(patch.mode);
                }
                self.prev_mode = self.mode;
                self.mode = patch.mode;
                self.mode_xfade.snap(0.0);
                self.mode_xfade.set_target(1.0);
                self.ctrl_count = 0;
            }
            self.cutoff.set_target(fc);
            self.resonance.set_target(res);
            self.drive.set_target(drv);
            self.kerr.set_target(kerr);
            self.spread.set_target(spread);
            self.mix.set_target(mix);
        }
    }

    pub fn set_parameters(&mut self, patch: FilterPatch) {
        self.set_parameters_common(patch);
        self.env
            .set_pattern(CompiledFilterPattern::neutral(), patch.env_decay_ms);
    }

    /// Update parameters together with a compiled rhythmic pattern and transport clock.
    pub fn set_parameters_with_clock(
        &mut self,
        patch: FilterPatch,
        compiled: CompiledFilterPattern,
        song_pos: f64,
        bpm: f64,
    ) {
        self.set_parameters_common(patch);
        self.env
            .sync_song_pos(compiled, patch.env_decay_ms, song_pos, bpm);
    }

    #[inline]
    fn update_effective_cutoff(&mut self, env_level: f64, force: bool) {
        let raw_env_mod = if self.env_amount_ln2 == 0.0 {
            0.0
        } else {
            env_level * self.env_amount_ln2
        };
        let env_mod = if raw_env_mod == 0.0 { 0.0 } else { raw_env_mod };
        let base_fc = self.cutoff.current();
        let key = (base_fc.to_bits(), env_mod.to_bits());
        if !force && key == self.cached_fc_key {
            return;
        }
        self.cached_fc_key = key;
        #[cfg(test)]
        {
            self.coeff_recomputes += 1;
        }
        self.effective_fc = if env_mod == 0.0 {
            base_fc
        } else {
            clamp_freq_hz(base_fc * env_mod.exp(), 20.0, self.sample_rate, 1_000.0)
        };
    }

    #[inline]
    fn finalize_settled_mode_xfade(&mut self) {
        if self.prev_mode != self.mode {
            if MultiModeFilter::topology_id(self.prev_mode)
                != MultiModeFilter::topology_id(self.mode)
            {
                self.filter.reset_topology_for(self.prev_mode);
            }
            self.prev_mode = self.mode;
        }
    }

    #[inline]
    fn advance_smoothers_and_env(&mut self, frames: usize) -> f64 {
        let env_level = if self.env_amount_ln2 != 0.0 {
            self.env.advance_silent(frames)
        } else {
            0.0
        };
        self.cutoff.advance_silent(self.cutoff_coeff, frames);
        self.resonance.advance_silent(frames);
        self.drive.advance_silent(frames);
        self.kerr.advance_silent(frames);
        self.spread.advance_silent(frames);
        self.mix.advance_silent(frames);
        self.mode_xfade.advance_silent(frames);
        env_level
    }

    #[inline]
    pub fn advance_silent_frames(&mut self, frames: usize, bypassed: bool) {
        if frames == 0 {
            return;
        }
        self.has_processed = true;
        self.bypass.advance_silent(bypassed);
        if let Some(last_tick) = control_stride_last_tick(self.ctrl_count, self.ctrl_mask, frames) {
            let env_at_tick = self.advance_smoothers_and_env(last_tick + 1);
            self.update_effective_cutoff(env_at_tick, false);
            let rem = frames - 1 - last_tick;
            if rem > 0 {
                self.advance_smoothers_and_env(rem);
            }
        } else {
            self.advance_smoothers_and_env(frames);
        }
        self.ctrl_count = self.ctrl_count.wrapping_add(frames as u32) & self.ctrl_mask;
        if self.mode_xfade.current() >= 1.0 - LIN_FLOOR_F64 {
            self.finalize_settled_mode_xfade();
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
                self.filter.reset();
                self.dc_blocker.reset();
                self.bypass.mark_idle();
                self.advance_silent_frames(1, true);
                return dry;
            }
            BypassStep::Active(w) => w,
        };

        let env_level = if self.env_amount_ln2 != 0.0 {
            self.env.next_sample()
        } else {
            0.0
        };

        self.cutoff.step(self.cutoff_coeff);
        let res = self.resonance.next();
        let drv = self.drive.next();
        let kerr = self.kerr.next();
        let spread = self.spread.next();
        let mix = self.mix.next();
        let m_xf = self.mode_xfade.next();

        if self.ctrl_count == 0 {
            self.update_effective_cutoff(env_level, false);
        }
        self.ctrl_count = self.ctrl_count.wrapping_add(1) & self.ctrl_mask;

        let dry64 = [f64::from(dry[0]), f64::from(dry[1])];
        let filtered = if m_xf >= 1.0 - LIN_FLOOR_F64 {
            self.finalize_settled_mode_xfade();
            self.filter.process_stereo(
                dry64,
                self.mode,
                self.effective_fc,
                res as f32,
                drv,
                kerr,
                spread,
            )
        } else {
            self.filter.process_stereo_crossfade(
                dry64,
                self.prev_mode,
                self.mode,
                m_xf,
                self.effective_fc,
                res as f32,
                drv,
                kerr,
                spread,
            )
        };

        let dc_free = self.dc_blocker.process_stereo(filtered);
        let wet = [
            sanitize_sample_f64(dc_free[0]),
            sanitize_sample_f64(dc_free[1]),
        ];
        let out = mix_stereo_f64(dry64, wet, mix * bypass_w);
        [
            sanitize_sample(out[0] as f32),
            sanitize_sample(out[1] as f32),
        ]
    }
}

impl crate::dsp::StereoEffect for RhythmicFilter {
    #[inline]
    fn process(&mut self, input: [f32; 2], bypassed: bool) -> [f32; 2] {
        RhythmicFilter::process(self, input, bypassed)
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
            *frame = RhythmicFilter::process(self, *frame, bypassed);
        }
    }

    #[inline]
    fn reset(&mut self) {
        RhythmicFilter::reset(self);
    }

    #[inline]
    fn is_quiet(&self, bypassed: bool) -> bool {
        self.bypass.is_settled_for(bypassed)
            && (bypassed || (self.filter.quiet() && self.dc_blocker.quiet()))
    }

    #[inline]
    fn advance_silent_frames(&mut self, frames: usize, bypassed: bool) {
        RhythmicFilter::advance_silent_frames(self, frames, bypassed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::StereoEffect;
    use std::f64::consts::TAU;

    fn measure_multi_mode_db(mode: FilterMode, cutoff: f32, res: f32, freq: f64, sr: f64) -> f64 {
        let warmup = (sr * 0.15) as usize;
        let measure = (sr * 0.10) as usize;
        let mut flt = MultiModeFilter::new(sr);
        let mut in_sq = 0.0;
        let mut out_sq = 0.0;
        // Small amplitude (0.002) stays strictly inside the small-signal linear regime
        // of DiodeLadder (below BARK_THRESHOLD and feedback saturation) for exact transfer checks.
        let amp = 0.002;
        for i in 0..(warmup + measure) {
            let x = (TAU * freq * i as f64 / sr).sin() * amp;
            let y = flt.process_stereo([x, x], mode, f64::from(cutoff), res, 0.0, 0.0, 0.0)[0];
            if i >= warmup {
                in_sq += x * x;
                out_sq += y * y;
            }
        }
        lin_to_db_f64((out_sq / in_sq).sqrt())
    }

    #[test]
    fn all_filter_modes_analytical_response_matches_dsp_across_sample_rates() {
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            for &mode in FilterMode::ALL {
                let cutoff = 1_000.0_f32;
                let res = 0.65_f32;
                for freq in [250.0, 1_000.0, 3_500.0] {
                    let predicted = filter_magnitude_db(mode, cutoff, res, freq, sr);
                    let measured = measure_multi_mode_db(mode, cutoff, res, freq, sr);
                    assert!(
                        (predicted - measured).abs() < 0.5,
                        "sr={sr} mode={mode:?} freq={freq}: predicted={predicted:.2} dB vs measured={measured:.2} dB"
                    );
                }
            }
        }

        // Exact Butterworth DC / -3 dB / stopband slopes for ImLpf2/3/4 and 0 dB DC for DiodeLadder.
        let sr = 48_000.0;
        let fc = 1_000.0_f32;
        for (poles, expected_slope_db_per_oct) in [
            (ImLpfPoles::Two, -12.0),
            (ImLpfPoles::Three, -18.0),
            (ImLpfPoles::Four, -24.0),
        ] {
            let (dc_re, dc_im) = im_lpf_transfer(poles, fc, 0.0, 0.0, sr);
            assert!(
                (dc_re.hypot(dc_im) - 1.0).abs() < 1e-12,
                "{poles:?}: expected 0 dB DC gain"
            );
            let (fc_re, fc_im) = im_lpf_transfer(poles, fc, 0.0, f64::from(fc), sr);
            let fc_db = lin_to_db_f64(fc_re.hypot(fc_im));
            assert!(
                (fc_db + 3.0103).abs() < 0.02,
                "{poles:?}: expected -3.01 dB at fc, got {fc_db:.4}"
            );
            let (h4_re, h4_im) = im_lpf_transfer(poles, fc, 0.0, 4_000.0, sr);
            let (h8_re, h8_im) = im_lpf_transfer(poles, fc, 0.0, 8_000.0, sr);
            let slope = lin_to_db_f64(h8_re.hypot(h8_im)) - lin_to_db_f64(h4_re.hypot(h4_im));
            assert!(
                (slope - expected_slope_db_per_oct).abs() < 0.35,
                "{poles:?}: expected {expected_slope_db_per_oct} dB/oct stopband, got {slope:.2}"
            );
        }
        for res in [0.0_f32, 0.5, 0.95] {
            let (re, im) = diode_ladder_transfer(fc, res, 0.0, sr);
            let expected = diode_ladder_output_gain(f64::from(res));
            assert!(
                (re.hypot(im) - expected).abs() < 1e-12,
                "DiodeLadder DC gain must match output recovery gain ({expected}) at res={res}"
            );
        }
    }

    #[test]
    fn im_lpf_kerr_dispersion_and_quadrature_spread_preserve_mono_compatibility() {
        let sr = 48_000.0;
        for mode in [FilterMode::ImLpf2, FilterMode::ImLpf3, FilterMode::ImLpf4] {
            // 1. Quadrature spread: spread=0 yields exact L == R; spread > 0 creates stereo width
            //    whose mono sum (L + R) / 2 equals cos(angle / 2) * y_mono identically (zero comb).
            let spread = 0.75;
            let angle = 0.5 * PI * spread;
            let expected_c = (0.5 * angle).cos();
            let mut mono_flt = MultiModeFilter::new(sr);
            let mut wide_flt = MultiModeFilter::new(sr);
            let mut max_side = 0.0_f64;
            for i in 0..2_048 {
                let x = (TAU * 700.0 * i as f64 / sr).sin() * 0.4;
                let m = mono_flt.process_stereo([x, x], mode, 900.0, 0.65, 0.2, 2.5, 0.0);
                let w = wide_flt.process_stereo([x, x], mode, 900.0, 0.65, 0.2, 2.5, spread);
                assert_eq!(m[0], m[1], "{mode:?}: spread=0 must produce exact L == R");
                let mono_sum = 0.5 * (w[0] + w[1]);
                assert!(
                    (mono_sum - expected_c * m[0]).abs() < 1e-12,
                    "{mode:?} frame {i}: mono sum {mono_sum} != cos(angle/2)*y {}",
                    expected_c * m[0]
                );
                max_side = max_side.max((w[0] - w[1]).abs());
            }
            assert!(
                max_side > 0.05,
                "{mode:?}: quadrature spread must generate clear stereo width, got {max_side}"
            );

            // 2. State-Kerr dispersion shifts nonlinear phase/response cleanly without instability.
            let mut f_zero = MultiModeFilter::new(sr);
            let mut f_pos = MultiModeFilter::new(sr);
            let mut f_neg = MultiModeFilter::new(sr);
            let mut diff_pos = 0.0_f64;
            let mut diff_neg = 0.0_f64;
            for i in 0..2_048 {
                let x = (TAU * 900.0 * i as f64 / sr).sin() * 0.6;
                let y0 = f_zero.process_stereo([x, x], mode, 900.0, 0.7, 0.35, 0.0, 0.0)[0];
                let yp = f_pos.process_stereo([x, x], mode, 900.0, 0.7, 0.35, 8.0, 0.0)[0];
                let yn = f_neg.process_stereo([x, x], mode, 900.0, 0.7, 0.35, -8.0, 0.0)[0];
                assert!(yp.is_finite() && yn.is_finite() && yp.abs() < 10.0 && yn.abs() < 10.0);
                if i > 512 {
                    diff_pos = diff_pos.max((yp - y0).abs());
                    diff_neg = diff_neg.max((yn - y0).abs());
                }
            }
            assert!(
                diff_pos > 0.05 && diff_neg > 0.05,
                "{mode:?}: Kerr dispersion must measurably shape resonant output (pos={diff_pos}, neg={diff_neg})"
            );
        }
    }

    #[test]
    fn parallel_mix_magnitude_captures_complex_phase_summation() {
        let sr = 48_000.0;
        // At mix = 0.0, response must be flat 0 dB everywhere.
        for freq in [100.0, 1_000.0, 8_000.0] {
            let db = filter_mix_magnitude_db(FilterMode::ImLpf2, 500.0, 0.5, 0.0, freq, sr);
            assert!(db.abs() < 1e-6, "mix=0 should be 0 dB, got {db}");
        }
        // At mix = 1.0, must match filter_magnitude_db exactly.
        let full = filter_mix_magnitude_db(FilterMode::DiodeLadder, 800.0, 0.7, 1.0, 800.0, sr);
        let base = filter_magnitude_db(FilterMode::DiodeLadder, 800.0, 0.7, 800.0, sr);
        assert!((full - base).abs() < 1e-6);

        // At mix = 0.5, verify against actual RhythmicFilter DSP!
        let mut rf = RhythmicFilter::new(sr);
        rf.set_parameters(FilterPatch {
            mode: FilterMode::ImLpf2,
            cutoff_hz: 800.0,
            resonance: 0.3,
            drive: 0.0,
            mix: 0.5,
            ..FilterPatch::default()
        });
        rf.reset();
        let warmup = 4800;
        let measure = 4800;
        let freq = 800.0;
        let mut in_sq = 0.0;
        let mut out_sq = 0.0;
        for i in 0..(warmup + measure) {
            let x = (TAU * freq * i as f64 / sr).sin() as f32 * 0.2;
            let y = rf.process([x, x], false);
            if i >= warmup {
                in_sq += f64::from(x) * f64::from(x);
                out_sq += f64::from(y[0]) * f64::from(y[0]);
            }
        }
        let measured_db = lin_to_db_f64((out_sq / in_sq).sqrt());
        let predicted_db = filter_mix_magnitude_db(FilterMode::ImLpf2, 800.0, 0.3, 0.5, freq, sr);
        assert!(
            (measured_db - predicted_db).abs() < 0.5,
            "parallel mix: measured={measured_db:.2} dB vs predicted={predicted_db:.2} dB"
        );
    }

    #[test]
    fn rhythmic_filter_envelope_and_mode_switches_are_bounded_and_clean() {
        let sr = 48_000.0;
        let bpm = 120.0;
        let mut steps = [0u8; crate::filter_patterns::MAX_FILTER_ENV_STEPS];
        steps[0] = 127;
        let pat0 = CompiledFilterPattern {
            resolution: 16,
            length: 16,
            smooth_attack: false,
            steps,
        };
        let mut f = RhythmicFilter::new(sr);
        let patch = FilterPatch {
            mode: FilterMode::ImLpf2,
            cutoff_hz: 250.0,
            resonance: 0.25,
            drive: 0.0,
            kerr_semitones: 0.0,
            spread: 0.0,
            mix: 1.0,
            env_pattern: 0,
            env_pattern_id: None,
            env_amount: 3.5,
            env_decay_ms: 80.0,
        };
        let mut peak_early = 0.0_f32;
        let mut peak_late = 0.0_f32;
        for i in 0..24_000 {
            if i % 64 == 0 {
                let pos = (i as f64) * bpm / (15.0 * sr);
                f.set_parameters_with_clock(patch, pat0, pos, bpm);
            }
            let s = (TAU * 1500.0 * i as f64 / sr).sin() as f32 * 0.5;
            let y = f.process([s, s], false);
            assert!(y[0].is_finite() && y[0].abs() < 4.0);
            if (256..2000).contains(&i) {
                peak_early = peak_early.max(y[0].abs());
            } else if i >= 20_000 {
                peak_late = peak_late.max(y[0].abs());
            }
        }
        assert!(peak_early > peak_late * 3.0);

        // Mode transitions across all 7 modes remain finite, bounded, and drain cleanly to quiet.
        for &mode in FilterMode::ALL {
            f.set_parameters(FilterPatch { mode, ..patch });
            for i in 0..512 {
                let s = (TAU * 440.0 * i as f64 / sr).sin() as f32 * 0.4;
                let y = f.process([s, -s], false);
                assert!(y[0].is_finite() && y[1].is_finite() && y[0].abs() < 4.0);
            }
        }
        // Active switch from ImLpf3 -> ImLpf2 -> DiodeLadder still drains cleanly to is_quiet(false).
        for mode in [
            FilterMode::ImLpf3,
            FilterMode::ImLpf2,
            FilterMode::DiodeLadder,
        ] {
            f.set_parameters(FilterPatch {
                mode,
                env_amount: 0.0,
                ..patch
            });
            for i in 0..1_024 {
                let s = (TAU * 440.0 * i as f64 / sr).sin() as f32 * 0.4;
                let _ = f.process([s, -s], false);
            }
        }
        for _ in 0..(sr as usize) {
            let _ = f.process([0.0, 0.0], false);
            if f.is_quiet(false) {
                break;
            }
        }
        assert!(
            f.is_quiet(false),
            "filter must drain cleanly to quiet after active multi-topology mode transitions"
        );
    }

    #[test]
    fn acid_accent_math_spares_default_velocity_and_normalizes_by_full_range() {
        // Default Cadence Step.velocity (0.80) and ReBirth non-accent (0.64) must NOT trigger accent.
        for vel in [0.0, 0.5, 0.64, 0.80, ACID_ACCENT_THRESHOLD] {
            assert_eq!(acid_accent_amount(vel), 0.0);
            assert_eq!(
                acid_filter_decay_ms(FilterMode::DiodeLadder, 260.0, 3.0, vel as f32),
                260.0
            );
            let m = filter_velocity_mod(FilterMode::DiodeLadder, vel, 1.0, 3.0, 0.6);
            assert_eq!(m.cutoff_octaves, 0.0);
            assert!((m.resonance - 0.6).abs() < 1e-6);
        }
        // Accented velocity (> 0.85, e.g. 0.925 and 1.0) snaps decay shorter and boosts cutoff + res.
        assert!((acid_accent_amount(1.0) - 1.0).abs() < 1e-6);
        let mid = acid_accent_amount(0.925);
        assert!((mid - 0.5).abs() < 1e-6);
        let snapped = acid_filter_decay_ms(FilterMode::DiodeLadder, 260.0, 3.0, 1.0);
        assert!((snapped - 260.0 * 0.48).abs() < 1e-4);
        // Short decay must never be lengthened by the 42 ms floor.
        assert_eq!(
            acid_filter_decay_ms(FilterMode::DiodeLadder, 25.0, 3.0, 1.0),
            25.0
        );
        // Resonance boost scales continuously across the full FilterVelAmount range (0..=4.0).
        let at_32 = filter_velocity_mod(FilterMode::DiodeLadder, 1.0, 1.0, 3.2, 0.5);
        let at_40 = filter_velocity_mod(FilterMode::DiodeLadder, 1.0, 1.0, 4.0, 0.5);
        assert!(at_32.resonance > 0.5);
        assert!(
            at_40.resonance > at_32.resonance + 0.02,
            "4.0 must boost resonance more than 3.2 (at_32={}, at_40={})",
            at_32.resonance,
            at_40.resonance
        );
        assert!((at_40.resonance - 0.64).abs() < 1e-5);
        // Standard ImLpf2 responds smoothly across full 0..=1 velocity without altering resonance.
        let lp = filter_velocity_mod(FilterMode::ImLpf2, 0.5, 1.0, 2.0, 0.4);
        assert!((lp.cutoff_octaves - 1.25).abs() < 1e-6);
        assert!((lp.resonance - 0.4).abs() < 1e-6);
        // Non-finite inputs sanitize safely.
        let nan_mod = filter_velocity_mod(
            FilterMode::DiodeLadder,
            f64::NAN,
            f64::NAN,
            f32::NAN,
            f32::NAN,
        );
        assert_eq!(nan_mod.cutoff_octaves, 0.0);
        assert_eq!(nan_mod.resonance, 0.0);
    }

    #[test]
    fn low_pass_6_matches_one_pole_transfer_and_crossfades_cleanly() {
        let sr = 48_000.0;
        let fc = 1_000.0_f32;
        // Exact SPOT parity between `filter_transfer(LowPass6)` and `TptOnePoleLpf::magnitude`.
        for &f in &[50.0, 250.0, 1_000.0, 2_000.0, 4_000.0, 8_000.0] {
            let m_filt = filter_magnitude(FilterMode::LowPass6, fc, 0.8, f, sr);
            let m_1p = TptOnePoleLpf::magnitude(f64::from(fc), f, sr);
            assert!(
                (m_filt - m_1p).abs() < 1e-12,
                "f={f}: filter_magnitude={m_filt} != TptOnePoleLpf={m_1p}"
            );
        }
        // -3.01 dB at cutoff, ~-6 dB/oct gentler than 2-pole ImLpf2 (-12 dB/oct) at 4*fc.
        let at_fc_db = filter_magnitude_db(FilterMode::LowPass6, fc, 0.0, 1_000.0, sr);
        assert!((at_fc_db + 3.0103).abs() < 0.05, "at_fc_db={at_fc_db}");
        let lp6_4k = filter_magnitude_db(FilterMode::LowPass6, fc, 0.0, 4_000.0, sr);
        let lp12_4k = filter_magnitude_db(FilterMode::ImLpf2, fc, 0.0, 4_000.0, sr);
        assert!(
            lp6_4k > lp12_4k + 9.0,
            "1-pole LP 6dB must be much gentler at 4*fc than 2-pole LP: lp6={lp6_4k:.2} lp12={lp12_4k:.2}"
        );

        // RhythmicFilter processes LowPass6 accurately and crossfades click-free with all sibling modes.
        let mut rf = RhythmicFilter::new(sr);
        let p_lp6 = FilterPatch {
            mode: FilterMode::LowPass6,
            cutoff_hz: 1_000.0,
            resonance: 0.5,
            drive: 0.0,
            mix: 1.0,
            ..FilterPatch::default()
        };
        rf.set_parameters(p_lp6);
        for &other in FilterMode::ALL {
            rf.set_parameters(FilterPatch {
                mode: other,
                ..p_lp6
            });
            for i in 0..1_200 {
                let x = (TAU * 440.0 * i as f64 / sr).sin() as f32 * 0.5;
                let y = rf.process([x, x], false);
                assert!(y[0].is_finite() && y[1].is_finite());
                assert!(y[0].abs() < 4.0 && y[1].abs() < 4.0);
            }
        }
    }

    #[test]
    fn filter_transfers_and_rhythmic_filter_sanitize_nan_parameters() {
        for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            for &mode in FilterMode::ALL {
                let (re, im) = filter_transfer(mode, f32::NAN, f32::NAN, f64::NAN, sr);
                assert!(re.is_finite() && im.is_finite(), "sr={sr} mode={mode:?}");
                let mag_db =
                    filter_mix_magnitude_db(mode, f32::NAN, f32::NAN, f32::NAN, 1_000.0, sr);
                assert!(mag_db.is_finite(), "sr={sr} mode={mode:?}");
            }

            let mut rf = RhythmicFilter::new(sr);
            rf.set_parameters(FilterPatch {
                mode: FilterMode::ImLpf2,
                cutoff_hz: f32::NAN,
                resonance: f32::NAN,
                drive: f32::NAN,
                kerr_semitones: f32::NAN,
                spread: f32::NAN,
                mix: f32::NAN,
                env_amount: f32::NAN,
                env_decay_ms: f32::NAN,
                ..FilterPatch::default()
            });
            for _ in 0..128 {
                let y = rf.process([0.5, -0.5], false);
                assert!(y[0].is_finite() && y[1].is_finite());
            }
        }
    }

    #[test]
    fn rhythmic_filter_preserves_unclipped_resonant_headroom_and_caches_static_coeffs() {
        for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            // 1. High-resonance ImLpf2 sine at cutoff exceeds +12 dBFS (> 4.5) without clipping
            //    and matches analytical filter_magnitude_db across all sample rates.
            let mut rf = RhythmicFilter::new(sr);
            let patch = FilterPatch {
                mode: FilterMode::ImLpf2,
                cutoff_hz: 1_000.0,
                resonance: 0.95,
                drive: 0.0,
                kerr_semitones: 0.0,
                spread: 0.0,
                mix: 1.0,
                env_amount: 0.0,
                ..FilterPatch::default()
            };
            rf.set_parameters(patch);

            let warmup = (sr * 0.12) as usize;
            let measure = (sr * 0.05) as usize;
            let mut in_sq = 0.0_f64;
            let mut out_sq = 0.0_f64;
            let mut peak = 0.0_f32;
            for i in 0..(warmup + measure) {
                let x = (TAU * 1_000.0 * i as f64 / sr).sin() as f32 * 0.7;
                let y = rf.process([x, -x], false);
                assert!(y[0].is_finite() && y[1].is_finite());
                if i >= warmup {
                    peak = peak.max(y[0].abs()).max(y[1].abs());
                    in_sq += f64::from(x) * f64::from(x);
                    out_sq += f64::from(y[0]) * f64::from(y[0]);
                }
            }
            assert!(
                peak > 4.5,
                "sr={sr}: expected unclipped resonant peak > 4.5, got {peak}"
            );
            let measured_db = lin_to_db_f64((out_sq / in_sq).sqrt());
            let expected_db = filter_magnitude_db(FilterMode::ImLpf2, 1_000.0, 0.95, 1_000.0, sr);
            assert!(
                (measured_db - expected_db).abs() < 0.5,
                "sr={sr}: resonant gain measured={measured_db:.2} dB vs predicted={expected_db:.2} dB"
            );

            // 2. Static settled parameters incur zero redundant coefficient recomputations,
            //    even when set_parameters is re-called every control block.
            let recomputes_after_steady = rf.coeff_recomputes();
            for i in 0..2048 {
                if i % 64 == 0 {
                    rf.set_parameters(patch);
                }
                let _ = rf.process([0.2, -0.2], false);
            }
            assert_eq!(
                rf.coeff_recomputes(),
                recomputes_after_steady,
                "sr={sr}: static settled filter must not recompute coefficients"
            );

            // 3. Bypass settles cleanly to idle via BypassFader.
            assert!(!rf.is_idle());
            for _ in 0..((sr * 0.03) as usize) {
                let _ = rf.process([0.3, -0.3], true);
            }
            assert!(rf.is_idle(), "sr={sr}: bypassed filter must idle");
            assert_eq!(rf.process([0.3, -0.3], true), [0.3, -0.3]);
        }
    }

    #[test]
    fn rhythmic_filter_cutoff_smooths_logarithmically_across_sample_rates() {
        for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let mut rf = RhythmicFilter::new(sr);
            let start_hz = 200.0_f32;
            let target_hz = 8_000.0_f32;
            rf.set_parameters(FilterPatch {
                mode: FilterMode::ImLpf2,
                cutoff_hz: start_hz,
                resonance: 0.25,
                drive: 0.4,
                mix: 1.0,
                env_amount: 0.0,
                ..FilterPatch::default()
            });
            let _ = rf.process([0.1, -0.1], false);
            assert!((rf.cutoff_hz_current() - f64::from(start_hz)).abs() < 1e-6);

            let recomputes_before = rf.coeff_recomputes();
            rf.set_parameters(FilterPatch {
                mode: FilterMode::ImLpf2,
                cutoff_hz: target_hz,
                resonance: 0.25,
                drive: 0.4,
                mix: 1.0,
                env_amount: 0.0,
                ..FilterPatch::default()
            });

            // After t = tau * ln(2) (tau = 4 ms), a log-domain smoother sits at geometric mean
            // sqrt(200 * 8000) = 1264.9 Hz (not arithmetic midpoint 4100 Hz).
            let half_frames = (0.004 * 2.0_f64.ln() * sr).round() as usize;
            let mut prev = rf.cutoff_hz_current();
            for _ in 0..half_frames {
                let y = rf.process([0.1, -0.1], false);
                assert!(y[0].is_finite() && y[1].is_finite());
                let cur = rf.cutoff_hz_current();
                assert!(cur >= prev - 1e-9 && cur <= f64::from(target_hz) + 1e-6);
                prev = cur;
            }
            let geo_mean = (f64::from(start_hz) * f64::from(target_hz)).sqrt();
            assert!(
                (rf.cutoff_hz_current() - geo_mean).abs() / geo_mean < 0.05,
                "sr={sr}: expected log midpoint ~{geo_mean:.1} Hz, got {:.1} Hz",
                rf.cutoff_hz_current()
            );

            // Settles cleanly to exact target and then stops recomputing coefficients.
            for _ in 0..((sr * 0.15) as usize) {
                let _ = rf.process([0.1, -0.1], false);
            }
            assert!(
                (rf.cutoff_hz_current() - f64::from(target_hz)).abs() < 1e-3,
                "sr={sr}: cutoff did not settle to {target_hz}, got {}",
                rf.cutoff_hz_current()
            );
            assert!(rf.coeff_recomputes() > recomputes_before);
            let settled_count = rf.coeff_recomputes();
            for _ in 0..1024 {
                let _ = rf.process([0.1, -0.1], false);
            }
            assert_eq!(rf.coeff_recomputes(), settled_count);
        }
    }
}
