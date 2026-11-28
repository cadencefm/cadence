//! Resonant low-pass filter primitives for Cubic Acoustic Dynamics.
//!
//! Provides two exact closed-form resonant low-pass filter families:
//! - [`ImLpfStage`] and [`ImLpfCascade`]: driven complex resonator stages reading
//!   `Im(z)` in 2-pole (-12 dB/oct), 3-pole (-18 dB/oct), and 4-pole (-24 dB/oct)
//!   Butterworth-limit cascades with joint radial bare-cubic drag, State-Kerr
//!   dispersion, and zero-comb quadrature stereo observation.
//! - [`DiodeLadderFilter`]: exact 1-shot algebraic zero-delay-feedback 4-stage
//!   passive diode RC ladder (`C_1 = 2C`) with optional post-saturation series
//!   feedback-return HPF and charge-conserving symmetric Strang diode bark.

use super::math::{log1p_div, sanitize_sample_rate, TAU};
use std::f64::consts::{FRAC_1_SQRT_2, PI};

/// 2-pole Butterworth quality factor `1 / sqrt(2)`.
const Q_BUTTER_2: f64 = FRAC_1_SQRT_2;

/// 4-pole Butterworth Stage 1 quality factor `1 / (2 * cos(3 * pi / 8))`.
pub(crate) const Q_BUTTER_4A: f64 = 1.306_562_964_876_376_6;

/// 4-pole Butterworth Stage 2 quality factor `1 / (2 * cos(pi / 8))`.
pub(crate) const Q_BUTTER_4B: f64 = 0.541_196_100_146_197;

/// `sin(pi / 8)`, equal to `w_0 / w_c` for 4-pole Butterworth Stage 2.
const SIN_PI_8: f64 = 0.382_683_432_365_089_8;

/// `cos(pi / 8)`, equal to `gamma / w_c = 1 / (2 * Q_4B)` for 4-pole Butterworth Stage 2.
const COS_PI_8: f64 = 0.923_879_532_511_286_7;

/// Default post-saturation feedback-return HPF cutoff frequency in Hz.
pub const DEFAULT_HPF_HZ: f64 = 72.3;

/// Differential capacitor voltage threshold below which diode bark is strictly linear.
pub const BARK_THRESHOLD: f64 = 0.16;

/// Filter pole order for [`ImLpfCascade`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImLpfPoles {
    /// 2-pole (-12 dB/oct) single complex stage.
    Two,
    /// 3-pole (-18 dB/oct) complex stage followed by a real first-order stage.
    Three,
    /// 4-pole (-24 dB/oct) cascade of two complex stages.
    Four,
}

/// Single-stage driven complex 2-pole (-12 dB/oct) resonant low-pass filter
/// reading the imaginary coordinate `y = Im(z)`.
///
/// Linear sub-flow:
/// `dz/dt = (i * w_0 - gamma) * (z - z_eq)`, `z_eq = (gamma / w_0 + i) * u`,
/// `w_c = 2 * pi * f_c`, `gamma = w_c / (2 * Q)`,
/// `w_0 = max(1e-6, sqrt(max(0, w_c^2 - gamma^2)))`,
/// yielding exact 0 dB DC gain `H_y(s) = w_c^2 / (s^2 + 2 * gamma * s + w_c^2)`.
///
/// Nonlinear sub-flow (symmetric Strang `NL(h/2) -> Linear(h) -> NL(h/2)`)
/// integrates bare-cubic drag and State-Kerr dispersion jointly in closed form
/// for global `O(h^2)` accuracy.
#[derive(Clone, Copy, Debug)]
pub struct ImLpfStage {
    pub x: f64,
    pub y: f64,
    pub w0_ratio: f64,
    pub damp_ratio: f64,
    sample_rate: f64,
    h: f64,
}

impl ImLpfStage {
    /// Create a zeroed single resonant low-pass stage for `sample_rate` Hz.
    #[inline]
    pub fn new(sample_rate: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        Self {
            x: 0.0,
            y: 0.0,
            w0_ratio: 1.0,
            damp_ratio: 0.0,
            sample_rate: sr,
            h: 1.0 / sr,
        }
    }

    /// Configured sample rate in Hz.
    #[inline]
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Reset complex state `z = x + i * y` to zero.
    #[inline]
    pub fn reset(&mut self) {
        self.x = 0.0;
        self.y = 0.0;
    }

    /// Exact closed-form radial bare-cubic drag and State-Kerr nonlinear substep over `dt`.
    #[inline]
    pub fn nl_step(&mut self, w0: f64, beta: f64, g: f64, dt: f64) {
        let a = (self.x * self.x + self.y * self.y).sqrt();
        if a == 0.0 || !a.is_finite() || (g == 0.0 && beta == 0.0) {
            return;
        }
        let u = g * a * dt;
        let denom = 1.0 + u;
        if denom <= 0.0 || !denom.is_finite() {
            return;
        }
        let rad = 1.0 / denom;
        if beta == 0.0 {
            self.x *= rad;
            self.y *= rad;
            return;
        }
        let phi = w0 * beta * a * dt * log1p_div(u);
        let (s, c) = phi.sin_cos();
        let nx = (c * self.x - s * self.y) * rad;
        self.y = (s * self.x + c * self.y) * rad;
        self.x = nx;
    }

    /// Advance the stage by one sample via symmetric Strang splitting:
    /// `NL(h/2) -> exact complex shift-and-rotate(h) -> NL(h/2)`.
    ///
    /// Returns the imaginary low-pass readout `y = Im(z)`.
    #[inline]
    pub fn step(&mut self, u: f64, fc: f64, q: f64, beta: f64, g_self: f64) -> f64 {
        let u_safe = if u.is_finite() { u } else { 0.0 };
        let fc_safe = if fc.is_finite() && fc > 0.0 { fc } else { 0.0 };
        let beta_safe = if beta.is_finite() { beta } else { 0.0 };
        let wc = TAU * fc_safe;
        let q_safe = if q > 1e-6 { q } else { 1e-6 };
        let gamma = wc / (2.0 * q_safe);
        let disc = wc * wc - gamma * gamma;
        let w0 = disc.max(0.0).sqrt().max(1e-6);
        self.w0_ratio = if wc > 0.0 { w0 / wc } else { 0.0 };
        self.damp_ratio = 1.0 / (2.0 * q_safe);

        let half_h = 0.5 * self.h;
        let g_drag = if g_self.is_finite() && g_self > 0.0 {
            g_self * wc
        } else {
            0.0
        };
        self.nl_step(w0, beta_safe, g_drag, half_h);

        let eqx = (gamma / w0) * u_safe;
        let dx = self.x - eqx;
        let dy = self.y - u_safe;
        let decay = (-gamma * self.h).exp();
        let (sin_w, cos_w) = (w0 * self.h).sin_cos();
        let c = cos_w * decay;
        let s = sin_w * decay;
        self.x = eqx + c * dx - s * dy;
        self.y = u_safe + s * dx + c * dy;

        self.nl_step(w0, beta_safe, g_drag, half_h);
        self.y
    }
}

/// Cascaded `Im(z)` resonant low-pass filter supporting 2-pole (-12 dB/oct),
/// 3-pole (-18 dB/oct), and 4-pole (-24 dB/oct) topologies.
///
/// In the continuous-time linear limit (`res = 0`, `beta = 0`, `drag = 0`), all
/// three pole orders realize Butterworth transfer functions (0 dB DC gain,
/// -3.01 dB at `fc`). Stage 1 hands off to Stage 2 via exact trapezoidal
/// midpoint `0.5 * (y_prev + y_next)` to preserve global `O(h^2)` accuracy.
///
/// Exposes Stage 1 coordinates `(x0, y0)`, Stage 2 / real-stage coordinates
/// `(x1, y1)`, and normalized derivative companion `quad_x` for zero-comb
/// quadrature stereo observation via [`Self::step_stereo`].
#[derive(Clone, Copy, Debug)]
pub struct ImLpfCascade {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
    pub quad_x: f64,
    pub out: f64,
    sample_rate: f64,
    h: f64,
    poles: ImLpfPoles,
    stage1: ImLpfStage,
    stage2: ImLpfStage,
    real_stage: f64,
}

impl ImLpfCascade {
    /// Create a zeroed cascade filter for `sample_rate` Hz with the given pole order.
    #[inline]
    pub fn new(sample_rate: f64, poles: ImLpfPoles) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        Self {
            x0: 0.0,
            y0: 0.0,
            x1: 0.0,
            y1: 0.0,
            quad_x: 0.0,
            out: 0.0,
            sample_rate: sr,
            h: 1.0 / sr,
            poles,
            stage1: ImLpfStage::new(sr),
            stage2: ImLpfStage::new(sr),
            real_stage: 0.0,
        }
    }

    /// Configured sample rate in Hz.
    #[inline]
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Select filter pole order without allocating, clearing inactive downstream stage state.
    #[inline]
    pub fn set_poles(&mut self, poles: ImLpfPoles) {
        if poles != self.poles {
            match poles {
                ImLpfPoles::Two => {
                    self.stage2.reset();
                    self.real_stage = 0.0;
                    self.x1 = 0.0;
                    self.y1 = 0.0;
                }
                ImLpfPoles::Three => {
                    self.stage2.reset();
                    self.x1 = 0.0;
                }
                ImLpfPoles::Four => {
                    self.real_stage = 0.0;
                }
            }
            self.poles = poles;
        }
    }

    /// Active filter pole order.
    #[inline]
    pub fn poles(&self) -> ImLpfPoles {
        self.poles
    }

    /// Reset all internal filter stages and observation coordinates to zero.
    #[inline]
    pub fn reset(&mut self) {
        self.stage1.reset();
        self.stage2.reset();
        self.real_stage = 0.0;
        self.x0 = 0.0;
        self.y0 = 0.0;
        self.x1 = 0.0;
        self.y1 = 0.0;
        self.quad_x = 0.0;
        self.out = 0.0;
    }

    /// Advance the cascade by one sample and return the mono `Im(z)` low-pass output.
    #[inline]
    pub fn step(&mut self, u: f64, fc: f64, res: f64, beta: f64, drag: f64) -> f64 {
        let fc_safe = if fc.is_finite() && fc > 0.0 { fc } else { 0.0 };
        let res_safe = if res.is_finite() { res } else { 0.0 };
        let drag_safe = if drag.is_finite() && drag > 0.0 {
            drag
        } else {
            0.0
        };

        match self.poles {
            ImLpfPoles::Two => {
                let q = Q_BUTTER_2 * 14.0_f64.powf(res_safe);
                let y = self.stage1.step(u, fc_safe, q, beta, drag_safe);
                self.x0 = self.stage1.x;
                self.y0 = self.stage1.y;
                self.x1 = 0.0;
                self.y1 = 0.0;
                self.quad_x = self.stage1.w0_ratio * self.x0 - self.stage1.damp_ratio * self.y0;
                self.out = y;
                y
            }
            ImLpfPoles::Three => {
                let q = 12.0_f64.powf(res_safe);
                let prev = self.stage1.y;
                let next = self.stage1.step(u, fc_safe, q, beta, drag_safe);
                let mid = 0.5 * (prev + next);
                self.x0 = self.stage1.x;
                self.y0 = self.stage1.y;
                self.real_stage = mid + (self.real_stage - mid) * (-TAU * fc_safe * self.h).exp();
                self.x1 = 0.0;
                self.y1 = self.real_stage;
                self.quad_x = next - self.real_stage;
                self.out = self.real_stage;
                self.real_stage
            }
            ImLpfPoles::Four => {
                let q = Q_BUTTER_4A * 10.5_f64.powf(res_safe);
                let prev = self.stage1.y;
                let next = self.stage1.step(u, fc_safe, q, beta, drag_safe);
                let mid = 0.5 * (prev + next);
                self.x0 = self.stage1.x;
                self.y0 = self.stage1.y;
                let y2 = self
                    .stage2
                    .step(mid, fc_safe, Q_BUTTER_4B, 0.0, drag_safe * 0.25);
                self.x1 = self.stage2.x;
                self.y1 = self.stage2.y;
                self.quad_x = SIN_PI_8 * self.x1 - COS_PI_8 * self.y1;
                self.out = y2;
                y2
            }
        }
    }

    /// Advance the cascade by one sample and project complex state into zero-comb
    /// quadrature stereo:
    /// `L = cos(spread_angle / 2) * y + sin(spread_angle / 2) * quad_x`,
    /// `R = cos(spread_angle / 2) * y - sin(spread_angle / 2) * quad_x`.
    ///
    /// At `spread_angle == 0.0`, `L == R == y` identically. The mono sum
    /// `(L + R) / 2` equals `cos(spread_angle / 2) * y` with zero comb filtering.
    #[inline]
    pub fn step_stereo(
        &mut self,
        u: f64,
        fc: f64,
        res: f64,
        beta: f64,
        drag: f64,
        spread_angle: f64,
    ) -> [f64; 2] {
        let y = self.step(u, fc, res, beta, drag);
        if !spread_angle.is_finite() || spread_angle == 0.0 {
            return [y, y];
        }
        let (s, c) = (0.5 * spread_angle).sin_cos();
        let qx = self.quad_x;
        [c * y + s * qx, c * y - s * qx]
    }

    /// Total squared state energy across complex and real stages.
    #[inline]
    pub fn energy(&self) -> f64 {
        self.x0 * self.x0
            + self.y0 * self.y0
            + self.x1 * self.x1
            + self.y1 * self.y1
            + self.real_stage * self.real_stage
    }

    /// Whether all filter stages have decayed below the `-140 dB` silence floor.
    #[inline]
    pub fn quiet(&self) -> bool {
        self.energy() < 1e-14
    }
}

/// Exact 1-shot algebraic zero-delay-feedback 4-stage passive diode RC ladder
/// (`C_1 = 2C`) with optional post-saturation series feedback-return HPF
/// (default 72.3 Hz) and symmetric Strang charge-conserving diode bark.
///
/// Key analytical properties:
/// - Implicit midpoint Thomas elimination over prewarped `g = tan(w_c * h / 2)`,
///   `g_1 = g / 2` yields global `O(h^2)` accuracy with zero Newton iterations
///   and zero 1-sample feedback delay.
/// - Two-branch cancellation-free quadratic root solves `|m_4|` for both
///   `q >= 0` (`2 * B / (q + root)`) and `q < 0` (`0.5 * sat * (root - q)`).
/// - Placing the feedback HPF strictly post-saturation keeps feedback mean zero
///   even on phase-dispersed asymmetric waveforms.
/// - Reciprocal diode bark edges conserve pairwise and total capacitor charge
///   `2 * v[0] + v[1] + v[2] + v[3]` identically while remaining strictly linear
///   for `|v[i] - v[j]| <= BARK_THRESHOLD`.
#[derive(Clone, Copy, Debug)]
pub struct DiodeLadderFilter {
    pub v: [f64; 4],
    pub dc: f64,
    sample_rate: f64,
    h: f64,
    hpf_hz: f64,
    gh: f64,
    ah: f64,
}

impl DiodeLadderFilter {
    /// Default post-saturation feedback-return HPF cutoff frequency in Hz.
    pub const DEFAULT_HPF_HZ: f64 = self::DEFAULT_HPF_HZ;

    /// Differential capacitor voltage threshold below which diode bark is strictly linear.
    pub const BARK_THRESHOLD: f64 = self::BARK_THRESHOLD;

    /// Create a zeroed diode ladder filter for `sample_rate` Hz and feedback HPF `hpf_hz` Hz.
    ///
    /// Pass `0.0` for direct feedback or [`Self::DEFAULT_HPF_HZ`] (`72.3`) for AC-coupled
    /// post-saturation feedback return.
    #[inline]
    pub fn new(sample_rate: f64, hpf_hz: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let mut filter = Self {
            v: [0.0; 4],
            dc: 0.0,
            sample_rate: sr,
            h: 1.0 / sr,
            hpf_hz: 0.0,
            gh: 0.0,
            ah: 1.0,
        };
        filter.set_hpf(hpf_hz);
        filter
    }

    /// Configured sample rate in Hz.
    #[inline]
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Configure the post-saturation feedback HPF cutoff in Hz without allocating.
    /// Non-positive or non-finite values select direct feedback (`0.0`).
    #[inline]
    pub fn set_hpf(&mut self, hpf_hz: f64) {
        self.hpf_hz = if hpf_hz.is_finite() && hpf_hz > 0.0 {
            hpf_hz
        } else {
            0.0
        };
        self.gh = if self.hpf_hz > 0.0 {
            ((PI * self.hpf_hz) / self.sample_rate).min(0.47 * PI).tan()
        } else {
            0.0
        };
        self.ah = 1.0 / (1.0 + self.gh);
    }

    /// Active feedback-return HPF cutoff frequency in Hz (`0.0` when disabled).
    #[inline]
    pub fn hpf_hz(&self) -> f64 {
        self.hpf_hz
    }

    /// Reset all four capacitor voltages and feedback HPF state to zero.
    #[inline]
    pub fn reset(&mut self) {
        self.v = [0.0; 4];
        self.dc = 0.0;
    }

    /// Single charge-conserving opposing-biased reciprocal diode bark edge between
    /// capacitors `i` and `j` with inverse-capacitance weights `(wi, wj)`.
    ///
    /// Strictly linear (no-op) for `|v[i] - v[j]| <= BARK_THRESHOLD`, and conserves
    /// `(1 / wi) * v[i] + (1 / wj) * v[j]` identically above threshold. Out-of-range
    /// indices are ignored safely.
    #[inline]
    pub fn diode_bark_edge(&mut self, i: usize, j: usize, wi: f64, wj: f64, eta: f64) {
        if i >= 4 || j >= 4 {
            return;
        }
        let d = self.v[i] - self.v[j];
        let a = d.abs();
        if a <= Self::BARK_THRESHOLD || !eta.is_finite() || eta <= 0.0 {
            return;
        }
        let w_sum = wi + wj;
        if !w_sum.is_finite() || w_sum <= 0.0 {
            return;
        }
        let excess = a - Self::BARK_THRESHOLD;
        let new_ex = excess / (1.0 + w_sum * eta * excess);
        let sign = if d < 0.0 { -1.0 } else { 1.0 };
        let drop = ((excess - new_ex) * sign) / w_sum;
        self.v[i] -= wi * drop;
        self.v[j] += wj * drop;
    }

    /// Execute one pass across all three reciprocal diode bark edges (forward or
    /// reverse for symmetric Strang closure), conserving total ladder charge
    /// `2 * v[0] + v[1] + v[2] + v[3]` identically.
    #[inline]
    pub fn bark_pass(&mut self, eta: f64, reverse: bool) {
        if !reverse {
            self.diode_bark_edge(0, 1, 0.5, 1.0, eta);
            self.diode_bark_edge(1, 2, 1.0, 1.0, eta);
            self.diode_bark_edge(2, 3, 1.0, 1.0, eta);
        } else {
            self.diode_bark_edge(2, 3, 1.0, 1.0, eta);
            self.diode_bark_edge(1, 2, 1.0, 1.0, eta);
            self.diode_bark_edge(0, 1, 0.5, 1.0, eta);
        }
    }

    /// Total capacitor charge `2 * v[0] + v[1] + v[2] + v[3]` (with `C_1 = 2C`).
    #[inline]
    pub fn total_charge(&self) -> f64 {
        2.0 * self.v[0] + self.v[1] + self.v[2] + self.v[3]
    }

    /// Advance the 4-stage `C_1 = 2C` diode ladder by one sample:
    /// `Bark(h/2) -> 1-shot ZDF implicit midpoint + post-sat HPF(h) -> Bark^dagger(h/2)`.
    ///
    /// Returns the fourth-stage capacitor voltage `v[3]`.
    #[inline]
    pub fn step(&mut self, u: f64, fc: f64, res: f64, drag: f64) -> f64 {
        let u_safe = if u.is_finite() { u } else { 0.0 };
        let fc_safe = if fc.is_finite() && fc > 0.0 { fc } else { 0.0 };
        let res_safe = if res.is_finite() && res > 0.0 {
            res
        } else {
            0.0
        };
        let drag_safe = if drag.is_finite() && drag > 0.0 {
            drag
        } else {
            0.0
        };

        let wc = TAU * fc_safe;
        let g = (wc * self.h * 0.5).min(0.47 * PI).tan();
        let g1 = 0.5 * g;
        let eta = (0.10 + drag_safe * (10.0 / 3.0)) * wc * self.h * 0.5;

        self.bark_pass(eta, false);

        let [v0, v1, v2, v3] = self.v;
        let b4 = 1.0 / (1.0 + g);
        let b3 = 1.0 / (1.0 + 2.0 * g - g * g * b4);
        let r3 = b3 * (v2 + g * b4 * v3);
        let b2 = 1.0 / (1.0 + 2.0 * g - g * g * b3);
        let r2 = b2 * (v1 + g * r3);
        let b1 = 1.0 / (1.0 + 2.0 * g1 - g1 * g * b2);
        let r1 = b1 * (v0 + g1 * r2);
        let g1b1 = g1 * b1;
        let m20 = r2 + g * b2 * r1;
        let m30 = r3 + g * b3 * m20;
        let s_zero = b4 * (v3 + g * m30);
        let g_gain = g1b1 * (g * b2) * (g * b3) * (g * b4);

        let k_res = 16.8 * res_safe;
        let ah = self.ah;
        let sat = 0.40;
        let has_hpf = self.hpf_hz > 0.0;
        let ak = if has_hpf { ah * k_res } else { k_res };
        let rhs = if has_hpf {
            g_gain * (u_safe + ak * self.dc) + s_zero
        } else {
            g_gain * u_safe + s_zero
        };
        let kappa = ak * g_gain;
        let b_abs = rhs.abs();

        let ym = if b_abs > 0.0 {
            let q = 1.0 + kappa - b_abs / sat;
            let root = (q * q + (4.0 * b_abs) / sat).sqrt();
            if q >= 0.0 {
                (2.0 * b_abs) / (q + root)
            } else {
                0.5 * sat * (root - q)
            }
        } else {
            0.0
        };

        let m4_sol = if rhs < 0.0 { -ym } else { ym };
        let f = m4_sol / (1.0 + ym / sat);
        let um = if has_hpf {
            u_safe - ak * (f - self.dc)
        } else {
            u_safe - k_res * f
        };

        let m1 = r1 + g1b1 * um;
        let m2 = r2 + g * b2 * m1;
        let m3 = r3 + g * b3 * m2;
        let m4 = b4 * (v3 + g * m3);
        self.v[0] = 2.0 * m1 - v0;
        self.v[1] = 2.0 * m2 - v1;
        self.v[2] = 2.0 * m3 - v2;
        self.v[3] = 2.0 * m4 - v3;

        if has_hpf {
            let dm = ah * (self.dc + self.gh * f);
            self.dc = 2.0 * dm - self.dc;
        }

        self.bark_pass(eta, true);
        self.v[3]
    }

    /// Total squared state energy across all four ladder stages and post-sat feedback HPF.
    #[inline]
    pub fn energy(&self) -> f64 {
        self.v[0] * self.v[0]
            + self.v[1] * self.v[1]
            + self.v[2] * self.v[2]
            + self.v[3] * self.v[3]
            + self.dc * self.dc
    }
}
