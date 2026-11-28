//! Stateless unit-circle algebraic & Hardy-space exciters, rational `SO(2)`
//! Cayley phasors, and raised-cosine mallet strike pulses.
//!
//! All waveform readouts evaluate statelessly on a unit-circle phasor
//! `u = c + i * s` (`c^2 + s^2 = 1`) with exact unit-peak or unit-`L^2`
//! normalization, zero DC bias on odd real-valued waveforms, and continuous
//! pitch-aware harmonic reach guarding. All operations are zero-allocation,
//! lock-free, and panic-free.

use core::f64::consts::PI;

use super::math::{cayley_rotation, sanitize_sample_rate, TAU};

/// Wave exciter family selectable on unit-circle phasor evaluation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CubicWaveform {
    /// Smooth `C^inf` algebraic Mobius jet (`rho`, `k`).
    MobiusJet,
    /// Half-scaled dual-phase zero-DC algebraic Mobius PWM.
    MobiusPwm,
    /// Zero-`sqrt` bare-cubic rational saw readout (`1 / n^3` upper tail).
    BareCubic,
    /// Exact unit-peak logarithmic Hardy saw readout.
    HardySaw,
    /// Pure unit-circle sine (or complex analytic spinning phasor).
    Sine,
    /// `L^2`-normalized Szego reproducing-kernel analytic driver.
    Szego,
    /// `L^2`-normalized two-pole dipole analytic driver.
    Dipole,
    /// `L^2`-normalized Poisson formant-carrier analytic driver.
    Poisson,
}

/// Continuous `C^inf` pitch-aware harmonic reach guard:
///
/// `rho_eff = min(rho, exp(-24 * freq_hz / sample_rate))`.
///
/// Pins the exterior singularity decay at Nyquist to `exp(-12)` (`~ -104 dB`)
/// without harmonic-count staircases during continuous pitch glides.
#[inline]
pub fn pitch_guard_rho(rho: f64, freq_hz: f64, sample_rate: f64) -> f64 {
    if !rho.is_finite() || rho <= 0.0 {
        return 0.0;
    }
    if !freq_hz.is_finite() || !sample_rate.is_finite() || freq_hz <= 0.0 || sample_rate <= 0.0 {
        return rho;
    }
    rho.min((-24.0 * freq_hz / sample_rate).exp())
}

/// Precomputed algebraic exciter coefficients for a `(rho, k)` pair.
#[derive(Clone, Copy, Debug)]
pub struct MobiusCoeffs {
    /// Spectral decay radius in `[0, 1)`.
    pub rho: f64,
    /// Curvature shape parameter `k >= 0`.
    pub k: f64,
    /// Rational denominator factor `D = (1 + k) + (1 - k) * rho^2`.
    pub d: f64,
    /// Rational numerator factor `N = (1 + k) - (1 - k) * rho^2`.
    pub n: f64,
    /// Cached `2 * rho`.
    pub two_rho: f64,
    /// Rational inverse pole radius `r = 2 * rho / D`.
    pub r: f64,
    /// Exact unit-peak factor `sqrt(1 - r^2) + k * r` for [`bare_cubic_exciter`].
    pub bare_gain: f64,
    /// Exact unit-peak factor `1 / asin(rho)` for [`hardy_saw_exciter`].
    pub hardy_gain: f64,
}

impl MobiusCoeffs {
    /// Compute all algebraic exciter coefficients for spectral reach `rho` and
    /// curvature `k >= 0`.
    #[inline]
    pub fn new(rho: f64, k: f64) -> Self {
        let rho = if rho.is_finite() {
            rho.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let k = if k.is_finite() && k > 0.0 { k } else { 0.0 };
        let q = rho * rho;
        let d = (1.0 + k) + (1.0 - k) * q;
        let n = (1.0 + k) - (1.0 - k) * q;
        let two_rho = 2.0 * rho;
        let r = if d > 0.0 { two_rho / d } else { 0.0 };
        let bare_gain = (1.0 - r * r).max(0.0).sqrt() + k * r;
        let hardy_gain = if rho > 1e-12 { 1.0 / rho.asin() } else { 1.0 };
        Self {
            rho,
            k,
            d,
            n,
            two_rho,
            r,
            bare_gain,
            hardy_gain,
        }
    }
}

/// Exact unit-peak, zero-DC algebraic Mobius jet exciter on unit-circle phasor
/// `(c, s)` for spectral decay radius `rho in [0, 1)` and curvature `k >= 0`.
///
/// Exact endpoints: `rho = 0` gives `s = sin(theta)`; `k = 0` gives the exact
/// rational Blaschke map `Im((u - rho) / (1 - rho * u))`.
#[inline]
pub fn mobius_exciter(c: f64, s: f64, rho: f64, k: f64) -> f64 {
    let q = rho * rho;
    let d = (1.0 + k) + (1.0 - k) * q;
    let n = (1.0 + k) - (1.0 - k) * q;
    let x = d * c - 2.0 * rho;
    let y = n * s;
    let den = (x * x + y * y).sqrt();
    if den > 0.0 {
        y / den
    } else {
        0.0
    }
}

/// Evaluate the unit-peak, zero-DC algebraic Mobius jet exciter using
/// precomputed [`MobiusCoeffs`].
#[inline]
pub fn mobius_exciter_coeffs(c: f64, s: f64, coeffs: &MobiusCoeffs) -> f64 {
    let mx = coeffs.d * c - coeffs.two_rho;
    let my = coeffs.n * s;
    let den = (mx * mx + my * my).sqrt();
    if den > 0.0 {
        my / den
    } else {
        0.0
    }
}

/// Half-scaled, zero-DC dual-phase Mobius PWM exciter:
/// `0.5 * (G(c, s) - G(c * width_c + s * width_s, s * width_c - c * width_s))`.
#[inline]
pub fn mobius_pwm(c: f64, s: f64, width_c: f64, width_s: f64, coeffs: &MobiusCoeffs) -> f64 {
    let a = mobius_exciter_coeffs(c, s, coeffs);
    let c2 = c * width_c + s * width_s;
    let s2 = s * width_c - c * width_s;
    let b = mobius_exciter_coeffs(c2, s2, coeffs);
    0.5 * (a - b)
}

/// Zero-`sqrt`, exact unit-peak, zero-DC bare-cubic saw readout on unit-circle
/// phasor `(c, s)` using precomputed normalization `gain = sqrt(1 - r^2) + k * r`.
#[inline]
pub fn bare_cubic_exciter(c: f64, s: f64, r: f64, k: f64, gain: f64) -> f64 {
    let den = 1.0 - r * c + k * r * s.abs();
    if den > 0.0 {
        (gain * s) / den
    } else {
        0.0
    }
}

/// Exact unit-peak, zero-DC logarithmic Hardy saw readout on unit-circle phasor
/// `(c, s)` using precomputed normalization `gain = 1 / asin(r)`.
#[inline]
pub fn hardy_saw_exciter(c: f64, s: f64, r: f64, gain: f64) -> f64 {
    if r <= 1e-12 {
        return s;
    }
    (r * s).atan2(1.0 - r * c) * gain
}

/// Precomputed `L^2`-normalized Hardy-space complex analytic exciter coefficients.
#[derive(Clone, Copy, Debug)]
pub struct HardyCoeffs {
    /// Pitch-guarded Hardy pole radius `r in [0, 1)`.
    pub r: f64,
    /// Exact `L^2` normalization `sqrt(1 - r^2)` for [`szego_analytic`].
    pub szego_norm: f64,
    /// `cos(psi)` for dipole angle `psi = pi * (1 - geometry)`.
    pub psi_c: f64,
    /// `sin(psi)` for dipole angle `psi = pi * (1 - geometry)`.
    pub psi_s: f64,
    /// Exact `L^2` normalization for [`dipole_analytic`].
    pub dipole_norm: f64,
    /// `sin(0.5 * psi)` numerator rotation cosine for [`dipole_analytic`].
    pub dipole_c: f64,
    /// `-cos(0.5 * psi)` numerator rotation sine for [`dipole_analytic`].
    pub dipole_s: f64,
    /// Exact `L^2` normalization `sqrt((1 - r^2) / (1 + r^2)) * (1 - r^2)` for [`poisson_analytic`].
    pub poisson_norm: f64,
}

impl HardyCoeffs {
    /// Compute pitch-guarded Hardy-space pole radius and exact `L^2`
    /// normalizations for the active waveform, spectral reach `rho`, exciter
    /// `geometry`, fundamental `freq_hz`, and `sample_rate`.
    #[inline]
    pub fn new(
        waveform: CubicWaveform,
        rho: f64,
        geometry: f64,
        freq_hz: f64,
        sample_rate: f64,
        pitch_guard: bool,
    ) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let freq = if freq_hz.is_finite() && freq_hz > 0.0 {
            freq_hz
        } else {
            1.0
        };
        let rho_clamped = if rho.is_finite() {
            rho.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let geom = if geometry.is_finite() { geometry } else { 0.5 };

        let harms = (sr / (2.0 * freq.max(1.0))).floor().max(2.0);
        let mut r = rho_clamped;
        if pitch_guard {
            match waveform {
                CubicWaveform::Dipole => {
                    r = r.min((1e-4_f64 / harms).powf(1.0 / (harms - 1.0)));
                }
                CubicWaveform::Szego => {
                    r = r.min(1e-4_f64.powf(1.0 / harms));
                }
                CubicWaveform::Poisson => {
                    let cnt = ((0.5 * sr - freq * (1.0 + 5.0 * geom)) / freq.max(1.0)).floor();
                    r = r.min(if cnt > 0.0 {
                        5e-5_f64.powf(1.0 / cnt)
                    } else {
                        0.0
                    });
                }
                _ => {
                    r = pitch_guard_rho(rho_clamped, freq, sr);
                }
            }
        }
        r = r.clamp(0.0, 1.0);

        let r2 = r * r;
        let szego_norm = (1.0 - r2).max(0.0).sqrt();
        let psi = PI * (1.0 - geom);
        let (psi_s, psi_c) = psi.sin_cos();
        let dipole_norm = (((1.0 - r2) * (1.0 - 2.0 * r2 * psi_c + r2 * r2)) / (1.0 + r2))
            .max(0.0)
            .sqrt();
        let (half_s, half_c) = (0.5 * psi).sin_cos();
        let dipole_c = half_s;
        let dipole_s = -half_c;
        let poisson_norm = ((1.0 - r2) / (1.0 + r2)).max(0.0).sqrt() * (1.0 - r2);

        Self {
            r,
            szego_norm,
            psi_c,
            psi_s,
            dipole_norm,
            dipole_c,
            dipole_s,
            poisson_norm,
        }
    }
}

/// Evaluate the `L^2`-normalized Szego analytic driver `sqrt(1 - r^2) * u / (1 - r * u)`
/// on unit-circle phasor `(c, s)`.
#[inline]
pub fn szego_analytic(c: f64, s: f64, r: f64, szego_norm: f64) -> (f64, f64) {
    let d = 1.0 - 2.0 * r * c + r * r;
    if d > 0.0 {
        ((szego_norm * (c - r)) / d, (szego_norm * s) / d)
    } else {
        (0.0, 0.0)
    }
}

/// Evaluate the `L^2`-normalized two-pole dipole analytic driver on unit-circle
/// phasor `(c, s)` using precomputed [`HardyCoeffs`].
#[inline]
pub fn dipole_analytic(c: f64, s: f64, coeffs: &HardyCoeffs) -> (f64, f64) {
    let r = coeffs.r;
    let a = 1.0 - r * c;
    let b = -r * s;
    let ec = coeffs.psi_c * c - coeffs.psi_s * s;
    let es = coeffs.psi_s * c + coeffs.psi_c * s;
    let d = 1.0 - r * ec;
    let e = -r * es;
    let dr = a * d - b * e;
    let di = a * e + b * d;
    let nx = (coeffs.dipole_c * c - coeffs.dipole_s * s) * coeffs.dipole_norm;
    let ny = (coeffs.dipole_s * c + coeffs.dipole_c * s) * coeffs.dipole_norm;
    let den = dr * dr + di * di;
    if den > 0.0 {
        ((nx * dr + ny * di) / den, (ny * dr - nx * di) / den)
    } else {
        (0.0, 0.0)
    }
}

/// Evaluate the `L^2`-normalized Poisson formant-carrier analytic driver on
/// fundamental cosine `c` and formant carrier phasor `(fc, fs)`.
#[inline]
pub fn poisson_analytic(c: f64, fc: f64, fs: f64, r: f64, poisson_norm: f64) -> (f64, f64) {
    let den = 1.0 - 2.0 * r * c + r * r;
    if den > 0.0 {
        let env = poisson_norm / den;
        (fc * env, fs * env)
    } else {
        (0.0, 0.0)
    }
}

/// RMS-preserving chirality projection `v_chi = (v + chi * conj(v)) / sqrt(1 + chi^2)`.
///
/// Interpolates continuously from pure spinning analytic drive (`chirality = 0`)
/// to pure real-axis drive (`chirality = 1`).
#[inline]
pub fn apply_chirality(re: f64, im: f64, chirality: f64) -> (f64, f64) {
    let ic = 1.0 / (1.0 + chirality * chirality).sqrt();
    (re * (1.0 + chirality) * ic, im * (1.0 - chirality) * ic)
}

/// Unit-circle phasor with exact stationary half-step rotation, zero-trig
/// rational `SO(2)` Cayley pitch glides, Strang midpoint evaluation, and
/// periodic unit-norm renormalization every `4_096` samples.
#[derive(Clone, Copy, Debug)]
pub struct CayleyPhasor {
    h_full: f64,
    h_half: f64,
    log_freq: f64,
    target_log_freq: f64,
    gliding: bool,
    slide_k: f64,
    freq_hz: f64,
    osc_freq: f64,
    c: f64,
    s: f64,
    rot_c: f64,
    rot_s: f64,
    step_count: u32,
}

impl CayleyPhasor {
    /// Create a unit-circle phasor initialized at `(1.0, 0.0)` with stationary
    /// frequency `freq_hz` at `sample_rate` Hz.
    #[inline]
    pub fn new(sample_rate: f64, freq_hz: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let h_full = 1.0 / sr;
        let h_half = 0.5 * h_full;
        let f = if freq_hz.is_finite() && freq_hz >= 0.0 {
            freq_hz
        } else {
            0.0
        };
        let ang_h = TAU * f * h_half;
        let (rot_s, rot_c) = ang_h.sin_cos();
        let log_f = if f > 0.0 { f.ln() } else { f64::NEG_INFINITY };
        Self {
            h_full,
            h_half,
            log_freq: log_f,
            target_log_freq: log_f,
            gliding: false,
            slide_k: 0.0,
            freq_hz: f,
            osc_freq: f,
            c: 1.0,
            s: 0.0,
            rot_c,
            rot_s,
            step_count: 0,
        }
    }

    /// Set stationary frequency immediately (canceling any active glide).
    #[inline]
    pub fn set_freq(&mut self, freq_hz: f64) {
        let f = if freq_hz.is_finite() && freq_hz >= 0.0 {
            freq_hz
        } else {
            0.0
        };
        let log_f = if f > 0.0 { f.ln() } else { f64::NEG_INFINITY };
        self.log_freq = log_f;
        self.target_log_freq = log_f;
        self.gliding = false;
        self.freq_hz = f;
        if f.to_bits() != self.osc_freq.to_bits() {
            self.osc_freq = f;
            let ang_h = TAU * f * self.h_half;
            let (rot_s, rot_c) = ang_h.sin_cos();
            self.rot_c = rot_c;
            self.rot_s = rot_s;
        }
    }

    /// Begin an exponential log-frequency glide toward `target_hz` with time
    /// constant `glide_sec` seconds (snapping immediately when `glide_sec <= 0`
    /// or gliding from/to `0 Hz`).
    #[inline]
    pub fn set_glide(&mut self, target_hz: f64, glide_sec: f64) {
        let target = if target_hz.is_finite() && target_hz >= 0.0 {
            target_hz
        } else {
            0.0
        };
        if !glide_sec.is_finite() || glide_sec <= 0.0 || target <= 0.0 || !self.log_freq.is_finite()
        {
            self.set_freq(target);
            return;
        }
        self.target_log_freq = target.ln();
        self.slide_k = (-self.h_full / glide_sec).exp();
        if (self.target_log_freq - self.log_freq).abs() < 1e-10 {
            self.set_freq(target);
        } else {
            self.gliding = true;
        }
    }

    /// Reset unit-circle phase state to `(1.0, 0.0)` and clear renormalization counter.
    #[inline]
    pub fn reset_phase(&mut self) {
        self.c = 1.0;
        self.s = 0.0;
        self.step_count = 0;
    }

    /// Current instantaneous frequency in Hz.
    #[inline]
    pub fn frequency(&self) -> f64 {
        self.freq_hz
    }

    /// Current unit-circle phasor state `(c, s)`.
    #[inline]
    pub fn state(&self) -> (f64, f64) {
        (self.c, self.s)
    }

    /// Whether an exponential pitch glide is currently active.
    #[inline]
    pub fn is_gliding(&self) -> bool {
        self.gliding
    }

    /// Advance pitch glide (if active) and rotate the phasor by `h / 2` to the
    /// Strang midpoint `t_n + h / 2`, returning `(c_mid, s_mid)`.
    ///
    /// Pair each call with [`CayleyPhasor::step_second_half`] to finish the
    /// sample interval, or call [`CayleyPhasor::step_sample`] directly.
    #[inline]
    pub fn step_midpoint(&mut self) -> (f64, f64) {
        if self.gliding {
            let d = self.target_log_freq - self.log_freq;
            let sliding = if d.abs() < 1e-10 {
                self.log_freq = self.target_log_freq;
                self.gliding = false;
                false
            } else {
                self.log_freq = self.target_log_freq - d * self.slide_k;
                true
            };
            let f = self.log_freq.exp();
            self.freq_hz = f;
            if f.to_bits() != self.osc_freq.to_bits() {
                self.osc_freq = f;
                if sliding {
                    let (rc, rs) = cayley_rotation(0.5 * TAU * f * self.h_half);
                    self.rot_c = rc;
                    self.rot_s = rs;
                } else {
                    let ang_h = TAU * f * self.h_half;
                    let (rs, rc) = ang_h.sin_cos();
                    self.rot_c = rc;
                    self.rot_s = rs;
                }
            }
        }
        let c_mid = self.rot_c * self.c - self.rot_s * self.s;
        let s_mid = self.rot_s * self.c + self.rot_c * self.s;
        self.c = c_mid;
        self.s = s_mid;
        (c_mid, s_mid)
    }

    /// Complete the second half-step `h / 2` to `t_{n+1}` with periodic
    /// unit-norm renormalization every `4_096` samples.
    #[inline]
    pub fn step_second_half(&mut self) {
        let c_next = self.rot_c * self.c - self.rot_s * self.s;
        let s_next = self.rot_s * self.c + self.rot_c * self.s;
        self.c = c_next;
        self.s = s_next;
        self.step_count = self.step_count.wrapping_add(1);
        if (self.step_count & 4095) == 0 {
            let norm_sq = self.c * self.c + self.s * self.s;
            if norm_sq > 0.0 {
                let inv = 1.0 / norm_sq.sqrt();
                self.c *= inv;
                self.s *= inv;
            } else {
                self.c = 1.0;
                self.s = 0.0;
            }
        }
    }

    /// Advance one full sample (`h / 2` + `h / 2`) and return the Strang
    /// midpoint phasor `(c_mid, s_mid)` for waveform evaluation.
    #[inline]
    pub fn step_sample(&mut self) -> (f64, f64) {
        let mid = self.step_midpoint();
        self.step_second_half();
        mid
    }
}

/// Map normalized mallet hardness `geometry in [0, 1]` to physical raised-cosine
/// contact duration in seconds (`0.14 ms` hard metal to `12.14 ms` soft felt).
#[inline]
pub fn strike_time_from_geometry(geometry: f64) -> f64 {
    let g = if geometry.is_finite() {
        geometry.clamp(0.0, 1.0)
    } else {
        0.55
    };
    0.00014 + 0.012 * (1.0 - g).powi(3)
}

/// Raised-cosine mallet impact pulse evaluated at Strang midpoints `age + 0.5 * h`.
#[derive(Clone, Copy, Debug)]
pub struct StrikePulse {
    h: f64,
    h_half: f64,
    age: f64,
    strike_time: f64,
    drive: f64,
    velocity: f64,
    active: bool,
}

impl StrikePulse {
    /// Map normalized mallet hardness `geometry in [0, 1]` to contact duration
    /// in seconds.
    #[inline]
    pub fn strike_time_from_geometry(geometry: f64) -> f64 {
        strike_time_from_geometry(geometry)
    }

    /// Construct an idle strike pulse generator at `sample_rate` Hz.
    #[inline]
    pub fn new(sample_rate: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let h = 1.0 / sr;
        Self {
            h,
            h_half: 0.5 * h,
            age: 0.0,
            strike_time: strike_time_from_geometry(0.55),
            drive: 0.0,
            velocity: 0.0,
            active: false,
        }
    }

    /// Start a new raised-cosine strike pulse with hardness `geometry`,
    /// excitation `drive`, and note `velocity`.
    #[inline]
    pub fn trigger(&mut self, geometry: f64, drive: f64, velocity: f64) {
        self.strike_time = strike_time_from_geometry(geometry);
        self.drive = if drive.is_finite() { drive } else { 0.0 };
        self.velocity = if velocity.is_finite() { velocity } else { 0.0 };
        self.age = 0.0;
        self.active = true;
    }

    /// Reset pulse state to idle silence.
    #[inline]
    pub fn reset(&mut self) {
        self.age = 0.0;
        self.active = false;
    }

    /// Active contact duration in seconds.
    #[inline]
    pub fn strike_time(&self) -> f64 {
        self.strike_time
    }

    /// Whether the contact pulse still has remaining samples to emit.
    #[inline]
    pub fn is_active(&self) -> bool {
        self.active && self.age < self.strike_time
    }

    /// Evaluate the raised-cosine pulse at Strang midpoint `age + 0.5 * h`,
    /// advance `age += h`, and return `(active, pulse_value)`.
    #[inline]
    pub fn step_midpoint(&mut self) -> (bool, f64) {
        if !self.active || self.age >= self.strike_time {
            self.active = false;
            return (false, 0.0);
        }
        let t_mid = self.age + self.h_half;
        let pulse = 0.5 * (1.0 - (TAU * t_mid / self.strike_time).cos());
        self.age += self.h;
        if self.age >= self.strike_time {
            self.active = false;
        }
        (true, pulse)
    }

    /// Scale a pulse value into scalar excitation drive `pulse * drive * velocity * 0.5`.
    #[inline]
    pub fn scalar_drive(&self, pulse: f64) -> f64 {
        pulse * self.drive * self.velocity * 0.5
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mobius_bare_hardy_unit_peak_and_zero_dc() {
        let coeffs = MobiusCoeffs::new(0.88, 0.65);
        let steps = 4096;
        let mut sum_mob = 0.0;
        let mut peak_mob = 0.0_f64;
        let mut sum_pwm = 0.0;
        let mut peak_pwm = 0.0_f64;
        let mut sum_bare = 0.0;
        let mut peak_bare = 0.0_f64;
        let mut sum_hardy = 0.0;
        let mut peak_hardy = 0.0_f64;
        let (ws, wc) = (TAU * 0.35).sin_cos();

        for i in 0..steps {
            let theta = TAU * (i as f64) / (steps as f64);
            let (s, c) = theta.sin_cos();
            let vm = mobius_exciter_coeffs(c, s, &coeffs);
            assert!((vm - mobius_exciter(c, s, 0.88, 0.65)).abs() < 1e-15);
            sum_mob += vm;
            peak_mob = peak_mob.max(vm.abs());

            let vp = mobius_pwm(c, s, wc, ws, &coeffs);
            sum_pwm += vp;
            peak_pwm = peak_pwm.max(vp.abs());

            let vb = bare_cubic_exciter(c, s, coeffs.r, coeffs.k, coeffs.bare_gain);
            sum_bare += vb;
            peak_bare = peak_bare.max(vb.abs());

            let vh = hardy_saw_exciter(c, s, coeffs.rho, coeffs.hardy_gain);
            sum_hardy += vh;
            peak_hardy = peak_hardy.max(vh.abs());
        }

        assert!(sum_mob.abs() / (steps as f64) < 1e-12);
        assert!((peak_mob - 1.0).abs() < 1e-3);
        assert!(sum_pwm.abs() / (steps as f64) < 1e-12);
        assert!(peak_pwm <= 1.0 + 1e-12);
        assert!(sum_bare.abs() / (steps as f64) < 1e-12);
        assert!((peak_bare - 1.0).abs() < 1e-3);
        assert!(sum_hardy.abs() / (steps as f64) < 1e-12);
        assert!((peak_hardy - 1.0).abs() < 1e-3);
    }

    #[test]
    fn hardy_drivers_unit_l2_and_chirality() {
        let steps = 4096;
        let sz_c = HardyCoeffs::new(CubicWaveform::Szego, 0.75, 0.5, 220.0, 48_000.0, true);
        let dp_c = HardyCoeffs::new(CubicWaveform::Dipole, 0.75, 0.4, 220.0, 48_000.0, true);
        let mut sz_e = 0.0;
        let mut dp_e = 0.0;
        let mut chi_e = 0.0;
        for i in 0..steps {
            let theta = TAU * (i as f64) / (steps as f64);
            let (s, c) = theta.sin_cos();
            let (xr, xi) = szego_analytic(c, s, sz_c.r, sz_c.szego_norm);
            sz_e += xr * xr + xi * xi;
            let (cr, ci) = apply_chirality(xr, xi, 0.6);
            chi_e += cr * cr + ci * ci;

            let (dr, di) = dipole_analytic(c, s, &dp_c);
            dp_e += dr * dr + di * di;
        }
        assert!((sz_e / (steps as f64) - 1.0).abs() < 1e-4);
        assert!((chi_e / (steps as f64) - 1.0).abs() < 1e-4);
        assert!((dp_e / (steps as f64) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn cayley_phasor_norm_glide_and_strike_lifecycle() {
        let mut ph = CayleyPhasor::new(48_000.0, 440.0);
        ph.set_glide(880.0, 0.01);
        assert!(ph.is_gliding());
        for _ in 0..8192 {
            let (cm, sm) = ph.step_sample();
            assert!((cm * cm + sm * sm - 1.0).abs() < 1e-12);
        }
        assert!((ph.frequency() - 880.0).abs() < 1e-3);

        let mut ph_a = CayleyPhasor::new(48_000.0, 330.0);
        let mut ph_b = CayleyPhasor::new(48_000.0, 330.0);
        for _ in 0..16 {
            let mid_a = ph_a.step_midpoint();
            ph_a.step_second_half();
            let mid_b = ph_b.step_sample();
            assert!((mid_a.0 - mid_b.0).abs() < 1e-15);
            assert!((mid_a.1 - mid_b.1).abs() < 1e-15);
        }

        let mut sp = StrikePulse::new(48_000.0);
        assert!(!sp.is_active());
        sp.trigger(0.5, 1.0, 0.8);
        assert!(sp.is_active());
        let mut emitted = 0usize;
        let mut max_pulse = 0.0_f64;
        while sp.is_active() {
            let (active, p) = sp.step_midpoint();
            assert!(active);
            assert!((0.0..=1.0).contains(&p));
            max_pulse = max_pulse.max(p);
            emitted += 1;
        }
        assert!(emitted > 10);
        assert!(max_pulse > 0.95);
        let (still_active, pulse_after) = sp.step_midpoint();
        assert!(!still_active);
        assert_eq!(pulse_after.to_bits(), 0);
        assert!((sp.scalar_drive(1.0) - 0.4).abs() < 1e-15);
    }
}
