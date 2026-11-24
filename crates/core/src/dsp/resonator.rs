//! Pure, zero-allocation stereo 32-mode physical modal resonator ([`ModalResonator`])
//! and analytical mode descriptor SPOT ([`resonator_mode_descriptors`]) shared by
//! `cadence-engine` audio synthesis and `cadence-app` UI visualizers.
//!
//! Models four classic acoustic bodies via unconditionally stable 2-pole complex damped
//! phasor resonators (exact resonant frequencies up to Nyquist with zero bilinear warping
//! and exact `-60 dB` per-mode RT60 decay):
//! - `String`: stiff harmonic string dispersion (`f_k = f_0 * k * sqrt(1 + B k^2)`) with
//!   spatial pluck-position eigenfunction weighting (`sin(k pi p)`).
//! - `Bar`: Euler-Bernoulli free-free bar (marimba, vibraphone, glockenspiel, chime).
//! - `Membrane`: circular drumhead Bessel zeros `j_{m,n} / j_{0,1}` (timpani, conga, tabla).
//! - `Plate`: 2D circular bronze plate / bell / gong / cymbal following Weyl modal density
//!   with natural radial-asymmetry doublet splittings and bell-to-edge strike weighting.

use crate::dsp::math::{
    flush_denormal_f64, sanitize_clamp_f64, sanitize_sample_f64, sanitize_sample_rate,
    DECAY_LN_60DB, LIN_FLOOR_F64, SMOOTH_SETTLE_EPS_F64,
};
use crate::rack::ResonatorMode;
use std::f64::consts::{PI, TAU};

/// Upper pole-radius stability ceiling in `f64` (`1.0 - 1e-7`, supporting exact `-60 dB`
/// RT60 tails up to `~180 s` at `384 kHz` without truncating long decays at `192 kHz`).
const MAX_POLE_RADIUS: f64 = 0.999_999_9;
/// Minimum `1 - r²` floor guarding excitation normalization against rounding underflow.
const MIN_POLE_ENERGY_FLOOR: f64 = 1e-12;
/// Stereo-vs-mono channel equality threshold (`-240 dB`).
const STEREO_MATCH_EPS: f64 = SMOOTH_SETTLE_EPS_F64;

/// Number of parallel complex damped physical modes in [`ModalResonator`].
pub const RESONATOR_MODES: usize = 32;

/// Minimum base resonant frequency (Hz).
pub const RESONATOR_MIN_HZ: f32 = 20.0;
/// Maximum base resonant frequency (Hz).
pub const RESONATOR_MAX_HZ: f32 = 12_000.0;
/// Minimum base `-60 dB` modal decay time (ms).
pub const RESONATOR_MIN_DECAY_MS: f32 = 15.0;
/// Maximum base `-60 dB` modal decay time (ms).
pub const RESONATOR_MAX_DECAY_MS: f32 = 8_000.0;
/// Reference sample rate (Hz) anchoring continuous-time excitation coupling.
pub const RESONATOR_REF_SAMPLE_RATE: f64 = 44_100.0;
/// Audible upper mode frequency ceiling (Hz), matching `0.455 * 44_100 Hz` so high sample
/// rates (`48/96/192 kHz`) activate the exact same audible mode set without ultrasonic bloat.
pub const RESONATOR_AUDIBLE_MAX_HZ: f64 = 20_065.5;

/// Circular membrane Bessel roots `j_{m,n} / j_{0,1}` (first 32 modes, ascending).
const MEMBRANE_RATIOS: [f64; RESONATOR_MODES] = [
    1.000, 1.593, 2.136, 2.295, 2.653, 2.917, 3.155, 3.500, 3.600, 3.652, 4.059, 4.154, 4.479,
    4.601, 4.832, 4.903, 5.131, 5.318, 5.519, 5.565, 5.807, 5.959, 6.082, 6.235, 6.440, 6.615,
    6.790, 6.960, 7.140, 7.310, 7.490, 7.680,
];

/// Circular metallic plate / cymbal eigenmode ratios following 2D Weyl plate density
/// with natural `1.2..2.6%` radial-asymmetry doublet splittings across `1.0x..21.65x`.
const PLATE_RATIOS: [f64; RESONATOR_MODES] = [
    1.000, 1.540, 2.210, 2.940, 3.720, 4.480, 5.260, 6.080, 6.230, 6.950, 7.140, 7.820, 7.980,
    8.150, 8.860, 9.070, 9.780, 10.020, 10.740, 11.420, 11.680, 12.480, 13.180, 13.520, 14.350,
    15.120, 15.480, 16.400, 17.350, 18.420, 19.850, 21.650,
];

/// Analytical description of one physical mode (shared by DSP & UI visualizers).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ResonatorModeDescriptor {
    /// Resonant frequency of this mode (Hz).
    pub freq_hz: f64,
    /// Per-mode `-60 dB` RT60 decay time (seconds).
    pub decay_s: f64,
    /// RMS-normalized output weight of this mode (`0.0` when above Nyquist limit).
    pub gain: f64,
    /// True when this mode sits below the active audible/Nyquist ceiling
    /// (`(0.455 * sr).min(RESONATOR_AUDIBLE_MAX_HZ)`).
    pub active: bool,
}

/// Pure SPOT computation of all 32 physical mode frequencies, RT60 decay times, and
/// RMS-normalized excitation weights for any [`ResonatorMode`].
#[inline]
pub fn resonator_mode_descriptors(
    mode: ResonatorMode,
    base_hz: f64,
    structure: f32,
    decay_ms: f32,
    brightness: f32,
    position: f32,
    sample_rate: f64,
) -> [ResonatorModeDescriptor; RESONATOR_MODES] {
    let sr = sanitize_sample_rate(sample_rate);
    let max_hz = (sr * 0.455).min(RESONATOR_AUDIBLE_MAX_HZ);
    let f0 = sanitize_clamp_f64(
        base_hz,
        f64::from(RESONATOR_MIN_HZ),
        max_hz.min(f64::from(RESONATOR_MAX_HZ)),
        440.0,
    );
    let struc = sanitize_clamp_f64(f64::from(structure), 0.0, 1.0, 0.5);
    let dec_ms = sanitize_clamp_f64(
        f64::from(decay_ms),
        f64::from(RESONATOR_MIN_DECAY_MS),
        f64::from(RESONATOR_MAX_DECAY_MS),
        900.0,
    );
    let bright = sanitize_clamp_f64(f64::from(brightness), 0.0, 1.0, 0.65);
    let pos = sanitize_clamp_f64(f64::from(position), 0.0, 1.0, 0.35);

    let mut out = [ResonatorModeDescriptor::default(); RESONATOR_MODES];
    let damp_slope = -2.2 + 2.45 * bright;
    let amp_exp = -1.2 + 1.65 * bright;
    let base_decay_s = dec_ms * 0.001;
    let golden = 0.5 * (1.0 + 5.0_f64.sqrt());

    let mut sum_sq = 0.0_f64;
    for (i, slot) in out.iter_mut().enumerate() {
        let k = i as f64;
        let norm_k = k / (RESONATOR_MODES as f64 - 1.0);
        let (ratio, pos_w) = match mode {
            ResonatorMode::String => {
                let stiff = 0.00004 * 500.0_f64.powf(struc);
                let n = k + 1.0;
                let r = n * (1.0 + stiff * n * n).sqrt();
                let p = 0.06 + 0.44 * pos;
                let w = (n * PI * p).sin().abs();
                (r, w)
            }
            ResonatorMode::Bar => {
                let exp = 1.45 + 0.75 * struc;
                let r = ((2.0 * k + 3.0112) / 3.0112).powf(exp);
                let p = 0.08 + 0.42 * pos;
                let w = 0.25 + 0.75 * ((k + 1.0) * PI * p).sin().abs();
                (r, w)
            }
            ResonatorMode::Membrane => {
                let alpha = 0.72 + 0.56 * struc;
                let r = MEMBRANE_RATIOS[i].powf(alpha);
                let w =
                    (1.0 - pos) * (-2.2 * norm_k).exp() + pos * (0.25 + 0.75 * norm_k.powf(0.7));
                (r, w)
            }
            ResonatorMode::Plate => {
                let alpha = 0.72 + 0.56 * struc;
                let r = PLATE_RATIOS[i].powf(alpha);
                let nodal =
                    0.55 + 0.45 * ((k * golden + 0.3) * PI * (0.35 + 0.9 * pos)).cos().abs();
                let tilt_p = ((pos - 0.35) * 2.6 * (norm_k - 0.42))
                    .exp()
                    .clamp(0.10, 4.0);
                (r, nodal * tilt_p)
            }
        };

        let raw_freq = f0 * ratio;
        let active = raw_freq < max_hz && raw_freq >= 20.0;
        let freq = raw_freq.clamp(20.0, max_hz);
        let freq_ratio = (freq / f0.max(20.0)).max(1.0);
        let norm_f = (freq_ratio.log2() / 4.5).clamp(0.0, 1.5);
        let decay_s = (base_decay_s * (damp_slope * norm_f).exp()).clamp(0.008, 12.0);
        let gain = if active {
            pos_w * freq_ratio.powf(amp_exp)
        } else {
            0.0
        };
        sum_sq += gain * gain;
        *slot = ResonatorModeDescriptor {
            freq_hz: freq,
            decay_s,
            gain,
            active,
        };
    }

    let inv_norm = 1.0 / (sum_sq + 1e-12).sqrt();
    for slot in &mut out {
        slot.gain *= inv_norm;
    }
    out
}

/// Zero-allocation stereo 32-mode complex damped physical resonator bank.
///
/// Always heap-hosted inside `ModularVoice::resonator_banks` (`Box<[ModalResonator; _]>`)
/// so debug voice stacks remain compact while audio-thread evaluation performs zero
/// allocations, zero locks, and zero divisions.
#[derive(Clone, Copy, Debug)]
pub struct ModalResonator {
    sample_rate: f64,
    re: [[f64; RESONATOR_MODES]; 2],
    im: [[f64; RESONATOR_MODES]; 2],
    cos_r: [f64; RESONATOR_MODES],
    sin_r: [f64; RESONATOR_MODES],
    input_b: [f64; RESONATOR_MODES],
    gain: [f64; RESONATOR_MODES],
    active_count: usize,
    stereo_diverged: bool,
}

impl Default for ModalResonator {
    fn default() -> Self {
        Self::new(48_000.0)
    }
}

impl ModalResonator {
    /// Create a fresh silenced resonator bank initialized to default `String` modes.
    pub fn new(sample_rate: f64) -> Self {
        let mut bank = Self {
            sample_rate: sanitize_sample_rate(sample_rate),
            re: [[0.0; RESONATOR_MODES]; 2],
            im: [[0.0; RESONATOR_MODES]; 2],
            cos_r: [0.0; RESONATOR_MODES],
            sin_r: [0.0; RESONATOR_MODES],
            input_b: [0.0; RESONATOR_MODES],
            gain: [0.0; RESONATOR_MODES],
            active_count: 0,
            stereo_diverged: false,
        };
        bank.set_parameters(ResonatorMode::String, 440.0, 0.5, 900.0, 0.65, 0.35);
        bank
    }

    /// Clear all complex mode states and update sample rate.
    #[inline]
    pub fn reset(&mut self, sample_rate: f64) {
        self.sample_rate = sanitize_sample_rate(sample_rate);
        self.re = [[0.0; RESONATOR_MODES]; 2];
        self.im = [[0.0; RESONATOR_MODES]; 2];
        self.stereo_diverged = false;
    }

    /// Recompute per-mode phasor rotation (`cos_r`, `sin_r`), energy-preserving input
    /// coupling (`input_b`), and spatial/brightness weights (`gain`) from the SPOT descriptors.
    #[inline]
    pub fn set_parameters(
        &mut self,
        mode: ResonatorMode,
        base_hz: f64,
        structure: f32,
        decay_ms: f32,
        brightness: f32,
        position: f32,
    ) {
        let sr = self.sample_rate;
        let desc = resonator_mode_descriptors(
            mode, base_hz, structure, decay_ms, brightness, position, sr,
        );
        let mut active_count = 0usize;
        for (k, d) in desc.iter().enumerate() {
            if !d.active || d.gain <= 0.0 {
                self.cos_r[k] = 0.0;
                self.sin_r[k] = 0.0;
                self.input_b[k] = 0.0;
                self.gain[k] = 0.0;
                self.re[0][k] = 0.0;
                self.im[0][k] = 0.0;
                self.re[1][k] = 0.0;
                self.im[1][k] = 0.0;
                continue;
            }
            // Exact -60 dB pole radius: 10^(-3 / (decay_s * sr)) = exp(-ln(1000) / (decay_s * sr)).
            let r = (-DECAY_LN_60DB / (d.decay_s * sr))
                .exp()
                .clamp(0.0, MAX_POLE_RADIUS);
            let w = TAU * d.freq_hz / sr;
            let (sin_w, cos_w) = w.sin_cos();
            self.cos_r[k] = r * cos_w;
            self.sin_r[k] = r * sin_w;
            // Energy-preserving, sample-rate-invariant coupling across decay times and rates:
            // `sqrt((1 - r^2) * (44_100 / sr))` keeps both constant-PSD noise excitation
            // (`Var ∝ b^2 * sr / (1 - r^2)`) and coherent/feedback resonant peak gain
            // (`|H(f_k)| = b / (1 - r)`) invariant across `24..192 kHz` and `15..8000 ms`.
            self.input_b[k] = ((1.0 - r * r).max(MIN_POLE_ENERGY_FLOOR)
                * (RESONATOR_REF_SAMPLE_RATE / sr))
                .sqrt()
                * 2.4;
            self.gain[k] = d.gain;
            active_count = k + 1;
        }
        self.active_count = active_count;
    }

    /// Process one stereo frame `[L, R]` with linear dry/wet `mix ∈ [0.0, 1.0]`.
    #[inline]
    pub fn process_stereo(&mut self, input: [f64; 2], mix: f64) -> [f64; 2] {
        let dry_l = sanitize_sample_f64(input[0]);
        let dry_r = sanitize_sample_f64(input[1]);
        let m = sanitize_clamp_f64(mix, 0.0, 1.0, 1.0);
        let n = self.active_count;
        if n == 0 {
            return [dry_l * (1.0 - m), dry_r * (1.0 - m)];
        }
        // Guard extreme feedback excitations without altering normal dynamic range.
        let x_l = dry_l.clamp(-8.0, 8.0);
        let x_r = dry_r.clamp(-8.0, 8.0);
        let mono_in = (x_l - x_r).abs() < STEREO_MATCH_EPS;
        if !mono_in {
            self.stereo_diverged = true;
        }

        let mut wet_l = 0.0_f64;
        let mut wet_r = 0.0_f64;

        if mono_in && !self.stereo_diverged {
            // Fast mono path: step channel 0 and mirror cleanly to channel 1 (2x speedup on mono drums/synths).
            for k in 0..n {
                let c = self.cos_r[k];
                let s = self.sin_r[k];
                let re = self.re[0][k];
                let im = self.im[0][k];
                let nre = flush_denormal_f64(re * c - im * s + x_l * self.input_b[k]);
                let nim = flush_denormal_f64(re * s + im * c);
                self.re[0][k] = nre;
                self.im[0][k] = nim;
                self.re[1][k] = nre;
                self.im[1][k] = nim;
                wet_l += nim * self.gain[k];
            }
            wet_r = wet_l;
        } else {
            let mut any_diff = !mono_in;
            for k in 0..n {
                let c = self.cos_r[k];
                let s = self.sin_r[k];
                let b = self.input_b[k];
                let g = self.gain[k];

                let re0 = self.re[0][k];
                let im0 = self.im[0][k];
                let nre0 = flush_denormal_f64(re0 * c - im0 * s + x_l * b);
                let nim0 = flush_denormal_f64(re0 * s + im0 * c);
                self.re[0][k] = nre0;
                self.im[0][k] = nim0;
                wet_l += nim0 * g;

                let re1 = self.re[1][k];
                let im1 = self.im[1][k];
                let nre1 = flush_denormal_f64(re1 * c - im1 * s + x_r * b);
                let nim1 = flush_denormal_f64(re1 * s + im1 * c);
                self.re[1][k] = nre1;
                self.im[1][k] = nim1;
                wet_r += nim1 * g;

                if !any_diff
                    && ((nre0 - nre1).abs() > STEREO_MATCH_EPS
                        || (nim0 - nim1).abs() > STEREO_MATCH_EPS)
                {
                    any_diff = true;
                }
            }
            if !any_diff {
                self.stereo_diverged = false;
            }
        }

        [dry_l * (1.0 - m) + wet_l * m, dry_r * (1.0 - m) + wet_r * m]
    }

    /// Whether every active mode on both channels has decayed below `-120 dB` (`1e-6`).
    #[inline]
    pub fn quiet(&self) -> bool {
        let n = self.active_count;
        for k in 0..n {
            if self.re[0][k].abs() + self.im[0][k].abs() >= LIN_FLOOR_F64
                || self.re[1][k].abs() + self.im[1][k].abs() >= LIN_FLOOR_F64
            {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resonator_rt60_is_sample_rate_invariant() {
        // Measure the exact fundamental mode (`f0 = 220 Hz`) decay via DFT over a 20 ms
        // window starting at t = 0 ms vs t = 400 ms across 44.1 / 48 / 96 kHz: must hit
        // `-60 dB` within ±1.0 dB at `decay_ms = 400.0`.
        for sr in [44_100.0, 48_000.0, 96_000.0] {
            let mut res = ModalResonator::new(sr);
            let f0 = 220.0;
            res.set_parameters(ResonatorMode::String, f0, 0.0, 400.0, 0.5, 0.5);
            let win = (sr * 0.020) as usize;
            let t_400 = (sr * 0.400) as usize;
            let mut early_re = 0.0_f64;
            let mut early_im = 0.0_f64;
            let mut late_re = 0.0_f64;
            let mut late_im = 0.0_f64;
            for i in 0..(t_400 + win) {
                let x = if i == 0 { 1.0 } else { 0.0 };
                let y = res.process_stereo([x, x], 1.0)[0];
                let ang = TAU * f0 * (i as f64) / sr;
                if i < win {
                    early_re += y * ang.cos();
                    early_im += y * ang.sin();
                } else if i >= t_400 {
                    late_re += y * ang.cos();
                    late_im += y * ang.sin();
                }
            }
            let mag_early = early_re.hypot(early_im);
            let mag_late = late_re.hypot(late_im);
            assert!(
                mag_early > 1e-3,
                "sr={sr}: early f0 magnitude too small {mag_early}"
            );
            let drop_db = 20.0 * (mag_late / mag_early).log10();
            assert!(
                (-61.0..=-59.0).contains(&drop_db),
                "sr={sr}: expected -60.0 dB drop at 400 ms, got {drop_db:.2} dB"
            );
        }
    }

    #[test]
    fn resonator_mix_zero_is_transparent_and_all_modes_stay_bounded() {
        let sr = 48_000.0;
        let mut res = ModalResonator::new(sr);
        for mode in ResonatorMode::ALL {
            res.reset(sr);
            res.set_parameters(*mode, 520.0, 0.5, 600.0, 0.9, 0.7);
            // Dry bypass (`mix = 0.0`) must equal input verbatim.
            let y0 = res.process_stereo([0.42, -0.17], 0.0);
            assert!((y0[0] - 0.42).abs() < 1e-12 && (y0[1] + 0.17).abs() < 1e-12);

            // Impulse + NaN/Inf resilience + boundedness.
            res.reset(sr);
            let mut max_abs = 0.0_f64;
            for i in 0..4800 {
                let x = match i {
                    0 => 1.0,
                    10 => f64::NAN,
                    20 => f64::INFINITY,
                    _ => 0.0,
                };
                let y = res.process_stereo([x, -x * 0.5], 1.0);
                assert!(
                    y[0].is_finite() && y[1].is_finite(),
                    "mode={mode:?} produced non-finite"
                );
                max_abs = max_abs.max(y[0].abs()).max(y[1].abs());
            }
            assert!(
                max_abs > 0.01 && max_abs < 8.0,
                "mode={mode:?}: unexpected peak {max_abs}"
            );
        }
    }

    #[test]
    fn resonator_decays_to_quiet_after_impulse() {
        let sr = 44_100.0;
        let mut res = ModalResonator::new(sr);
        res.set_parameters(ResonatorMode::Plate, 600.0, 0.5, 40.0, 0.5, 0.5);
        let _ = res.process_stereo([1.0, 1.0], 1.0);
        assert!(!res.quiet());
        for _ in 0..(sr as usize) {
            let _ = res.process_stereo([0.0, 0.0], 1.0);
        }
        assert!(
            res.quiet(),
            "short 40 ms plate must settle to quiet within 1 s"
        );
    }

    #[test]
    fn resonator_descriptors_match_harmonic_string_and_inharmonic_plate() {
        let str_modes = resonator_mode_descriptors(
            ResonatorMode::String,
            200.0,
            0.0,
            800.0,
            0.5,
            0.2,
            48_000.0,
        );
        assert!((str_modes[0].freq_hz - 200.0).abs() < 0.1);
        assert!((str_modes[1].freq_hz - 400.0).abs() < 0.5);
        assert!((str_modes[2].freq_hz - 600.0).abs() < 1.5);

        let plate_modes = resonator_mode_descriptors(
            ResonatorMode::Plate,
            500.0,
            0.5,
            1200.0,
            0.9,
            0.6,
            48_000.0,
        );
        // Plate mode 1 ratio ~ 1.54 is strongly inharmonic (not 2.0x octave).
        let r1 = plate_modes[1].freq_hz / plate_modes[0].freq_hz;
        assert!(
            (1.4..=1.7).contains(&r1),
            "expected inharmonic plate ratio, got {r1}"
        );
        // Bright plate keeps high modes ringing nearly as long as low modes.
        assert!(
            plate_modes[20].decay_s > 0.5,
            "bright plate high mode decay too short"
        );
    }

    #[test]
    fn resonator_modes_and_excitation_gain_are_sample_rate_invariant() {
        use crate::dsp::math::lin_to_db_f64;
        use crate::dsp::noise::{mix_seed, NoiseGen};

        // 1. High-f0 Plate activates the exact same audible mode set and weights across 44.1..192 kHz.
        let ref_modes = resonator_mode_descriptors(
            ResonatorMode::Plate,
            1_340.0,
            0.48,
            1_600.0,
            0.82,
            0.68,
            44_100.0,
        );
        for sr in [48_000.0, 96_000.0, 192_000.0] {
            let modes = resonator_mode_descriptors(
                ResonatorMode::Plate,
                1_340.0,
                0.48,
                1_600.0,
                0.82,
                0.68,
                sr,
            );
            for (k, (a, b)) in ref_modes.iter().zip(modes.iter()).enumerate() {
                assert_eq!(
                    a.active, b.active,
                    "sr={sr} mode {k}: active mismatch ({} vs {})",
                    a.active, b.active
                );
                assert!(
                    (a.gain - b.gain).abs() < 1e-12,
                    "sr={sr} mode {k}: gain drift ({} vs {})",
                    a.gain,
                    b.gain
                );
            }
        }

        // 2. Exact Parseval noise-power gain (`sum h[n]^2 * (sr / 44_100)`) and live `NoiseGen`
        //    excitation into a 32-mode Plate remain invariant across 44.1 / 48 / 96 / 192 kHz.
        let parseval_db = |sr: f64| -> f64 {
            let mut res = ModalResonator::new(sr);
            res.set_parameters(ResonatorMode::Plate, 620.0, 0.5, 180.0, 0.72, 0.55);
            let frames = (sr * 0.300) as usize;
            let mut sum_h2 = 0.0_f64;
            for i in 0..frames {
                let x = if i == 0 { 1.0 } else { 0.0 };
                let y = res.process_stereo([x, x], 1.0)[0];
                sum_h2 += y * y;
            }
            lin_to_db_f64((sum_h2 * (sr / RESONATOR_REF_SAMPLE_RATE)).sqrt())
        };
        let ref_parseval_db = parseval_db(44_100.0);
        for sr in [48_000.0, 96_000.0, 192_000.0] {
            let db = parseval_db(sr);
            assert!(
                (db - ref_parseval_db).abs() < 0.05,
                "Parseval Plate noise gain drifted at sr={sr}: {db:.3} vs {ref_parseval_db:.3} dB"
            );
        }

        let measure_noise_plate_db = |sr: f64| -> f64 {
            let mut res = ModalResonator::new(sr);
            res.set_parameters(ResonatorMode::Plate, 620.0, 0.5, 60.0, 0.72, 0.55);
            let mut noise = NoiseGen::new(mix_seed(0xabc1_2345, 7), sr);
            let warmup = (sr * 0.080) as usize;
            let frames = (sr * 0.400) as usize;
            let mut sum_sq = 0.0_f64;
            for i in 0..(warmup + frames) {
                let x = noise.next_white();
                let y = res.process_stereo([x, x], 1.0)[0];
                if i >= warmup {
                    sum_sq += y * y;
                }
            }
            lin_to_db_f64((sum_sq / frames as f64).sqrt())
        };
        let ref_noise_db = measure_noise_plate_db(44_100.0);
        for sr in [48_000.0, 96_000.0, 192_000.0] {
            let db = measure_noise_plate_db(sr);
            assert!(
                (db - ref_noise_db).abs() < 0.45,
                "noise-excited Plate drifted at sr={sr}: {db:.2} vs {ref_noise_db:.2} dB"
            );
        }

        // 3. Coherent tonal sine excitation at resonance stays invariant across 24..192 kHz
        //    (f0 = 220 Hz keeps all 32 String modes below 24 kHz Nyquist).
        let measure_tonal_db = |sr: f64| -> f64 {
            let mut res = ModalResonator::new(sr);
            let f0 = 220.0;
            res.set_parameters(ResonatorMode::String, f0, 0.0, 200.0, 0.5, 0.5);
            let warmup = (sr * 0.150) as usize;
            let frames = (sr * 0.050) as usize;
            let mut sum_sq = 0.0_f64;
            for i in 0..(warmup + frames) {
                let x = (TAU * f0 * (i as f64) / sr).sin() * 0.05;
                let y = res.process_stereo([x, x], 1.0)[0];
                if i >= warmup {
                    sum_sq += y * y;
                }
            }
            lin_to_db_f64((sum_sq / frames as f64).sqrt())
        };
        let ref_tonal_db = measure_tonal_db(44_100.0);
        for sr in [24_000.0, 48_000.0, 96_000.0, 192_000.0] {
            let db = measure_tonal_db(sr);
            assert!(
                (db - ref_tonal_db).abs() < 0.25,
                "tonal-excited String drifted at sr={sr}: {db:.2} vs {ref_tonal_db:.2} dB"
            );
        }
    }

    #[test]
    fn resonator_preserves_stereo_tail_and_192k_long_rt60() {
        // 1. Stereo excitation tail must match two independent mono resonators on every sample
        //    across mode-0 zero-crossings and all sample rates (fast-mono must never clobber R).
        for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let mut stereo = ModalResonator::new(sr);
            let mut ref_l = ModalResonator::new(sr);
            let mut ref_r = ModalResonator::new(sr);
            for r in [&mut stereo, &mut ref_l, &mut ref_r] {
                r.set_parameters(ResonatorMode::Bar, 330.0, 0.4, 300.0, 0.7, 0.35);
            }
            let frames = (sr * 0.08) as usize;
            let mut stereo_diff = 0.0_f64;
            for i in 0..frames {
                let (xl, xr) = match i {
                    0 => (1.0, 0.0),
                    5 => (0.0, -0.8),
                    _ => (0.0, 0.0),
                };
                let y = stereo.process_stereo([xl, xr], 1.0);
                let yl = ref_l.process_stereo([xl, xl], 1.0)[0];
                let yr = ref_r.process_stereo([xr, xr], 1.0)[0];
                assert!(
                    (y[0] - yl).abs() < 1e-12 && (y[1] - yr).abs() < 1e-12,
                    "sr={sr} frame={i}: stereo tail diverged from dual-mono ref ({y:?} vs [{yl}, {yr}])"
                );
                stereo_diff += (y[0] - y[1]).powi(2);
            }
            assert!(stereo_diff > 1.0, "sr={sr}: expected sustained stereo tail");
        }

        // 2. Maximum 8000 ms decay at 192 kHz must hit true ~8.0 s RT60 (-15.0 dB after 2.0 s),
        //    not truncated by a low pole-radius ceiling (`0.999_995` capped RT60 to ~7.2 s -> -16.6 dB).
        let sr = 192_000.0;
        let f0 = 200.0; // Exact integer cycles inside a 25 ms DFT window (5 cycles).
        let mut res = ModalResonator::new(sr);
        res.set_parameters(
            ResonatorMode::String,
            f0,
            0.0,
            RESONATOR_MAX_DECAY_MS,
            0.5,
            0.5,
        );
        let win = (sr * 0.025) as usize;
        let t_2s = (sr * 2.0) as usize;
        let mut early_re = 0.0_f64;
        let mut early_im = 0.0_f64;
        let mut late_re = 0.0_f64;
        let mut late_im = 0.0_f64;
        for i in 0..(t_2s + win) {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let y = res.process_stereo([x, x], 1.0)[0];
            let ang = TAU * f0 * (i as f64) / sr;
            if i < win {
                early_re += y * ang.cos();
                early_im += y * ang.sin();
            } else if i >= t_2s {
                late_re += y * ang.cos();
                late_im += y * ang.sin();
            }
        }
        let drop_db = crate::dsp::lin_to_db_f64(late_re.hypot(late_im) / early_re.hypot(early_im));
        assert!(
            (-15.15..=-14.85).contains(&drop_db),
            "192 kHz 8000 ms RT60 expected -15.0 dB at 2.0 s, got {drop_db:.3} dB"
        );
    }
}
