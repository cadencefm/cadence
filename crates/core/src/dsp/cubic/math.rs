//! Pure mathematical primitives for cubic acoustic dynamics.
//!
//! Provides singularity-free elementary helpers, rational `SO(2)` Cayley and
//! cancellation-free `f32` rotations, exact closed-form 1-body radial/Kerr and
//! opposing-biased cubic pair flows, lossless stereographic phase flows,
//! conservative one-way energy routing, exact Bernoulli pneumatic transfer,
//! smooth `C^2` rolling-shallot valve closure, deterministic bipolar noise,
//! and zero-frequency `CubicAtom` RC/cubic charge capacitors.
//!
//! All routines are zero-allocation, lock-free, panic-free, and strictly
//! deterministic for real-time audio execution.

/// Full turn in radians (`2 * pi`).
pub const TAU: f64 = core::f64::consts::TAU;

/// Standard dry air density in `kg / m^3`.
pub const AIR_RHO: f64 = 1.2;

/// Nominal organ wind-chest gauge pressure in Pascals (`Pa`).
pub const NOMINAL_CHEST_PA: f64 = 850.0;

/// Square root of `3` (continuous-time white-noise scaling for uniform `[-1, 1]`).
pub const SQRT3: f64 = 1.732_050_807_568_877_2;

/// Maximum number of complex modal atoms per voice.
pub const MAX_ATOMS: usize = 48;

/// Default fallback sample rate in Hz when a non-positive or non-finite rate is supplied.
pub(crate) const DEFAULT_SAMPLE_RATE: f64 = 48_000.0;

/// Default non-zero seed for [`xorshift64_bipolar`].
pub const DEFAULT_RNG_SEED: u64 = 0x9E37_79B9_7F4A_7C15;

/// Sanitize a sample rate in Hz, falling back to `48_000.0` when non-positive or non-finite.
#[inline]
pub(crate) fn sanitize_sample_rate(sample_rate: f64) -> f64 {
    if sample_rate.is_finite() && sample_rate > 0.0 {
        sample_rate
    } else {
        DEFAULT_SAMPLE_RATE
    }
}

/// Singularity-free evaluation of `exprel(-x) = (1 - exp(-x)) / x`.
///
/// Uses a 4-term Taylor polynomial for `|x| < 1e-6` and `-(-x).exp_m1() / x`
/// otherwise, maintaining `< 1e-15` continuity across `x = 0` and `|x| = 1e-6`.
#[inline]
pub fn exprel_neg(x: f64) -> f64 {
    if x.abs() < 1e-6 {
        1.0 - 0.5 * x + (x * x) / 6.0 - (x * x * x) / 24.0
    } else {
        -(-x).exp_m1() / x
    }
}

/// Singularity-free evaluation of `ln(1 + u) / u` for `u > -1` (typically `u >= 0`).
///
/// Uses a 4-term Taylor polynomial for `|u| < 1e-6` and `u.ln_1p() / u`
/// otherwise, maintaining `< 1e-15` continuity across `u = 0` and `|u| = 1e-6`.
#[inline]
pub fn log1p_div(u: f64) -> f64 {
    if u.abs() < 1e-6 {
        1.0 - 0.5 * u + (u * u) / 3.0 - (u * u * u) / 4.0
    } else {
        u.ln_1p() / u
    }
}

/// Zero-trig rational `SO(2)` Cayley rotation using the Pade `(1, 2)` half-angle
/// tangent approximation:
///
/// `tau = w / (1 - w^2 / 3)`, `c = (1 - tau^2) / (1 + tau^2)`, `s = (2 * tau) / (1 + tau^2)`.
///
/// Preserves unit modulus `c^2 + s^2 == 1` to IEEE-754 precision with `O(w^5)`
/// phase error for sub-Nyquist step angles `|w| < pi / 2`.
#[inline]
pub fn cayley_rotation(half_angle: f64) -> (f64, f64) {
    let denom = 1.0 - (half_angle * half_angle) / 3.0;
    if denom == 0.0 {
        return (-1.0, 0.0);
    }
    let tau = half_angle / denom;
    let t2 = tau * tau;
    let inv = 1.0 / (1.0 + t2);
    ((1.0 - t2) * inv, (2.0 * tau) * inv)
}

/// Cancellation-free `(1 - cos(theta), sin(theta))` rotation coefficients
/// computed in `f64` and rounded to `f32` for single-precision sub-bass accuracy.
///
/// Computing `omc = 2 * sin^2(theta / 2)` avoids catastrophic cancellation in
/// `1 - cos(theta)` at low frequencies.
#[inline]
pub fn omc_rotation_f32(theta: f64) -> (f32, f32) {
    let sh = (0.5 * theta).sin();
    let omc = (2.0 * sh * sh) as f32;
    let sin = theta.sin() as f32;
    (omc, sin)
}

/// Apply a cancellation-free `f32` rotation parameterized by `(omc, sin)`
/// where `omc = 1 - cos(theta)` and `sin = sin(theta)`.
#[inline(always)]
pub fn apply_omc_rotation_f32(x: f32, y: f32, omc: f32, sin: f32) -> (f32, f32) {
    (x - omc * x - sin * y, y - omc * y + sin * x)
}

/// Exact closed-form opposing-biased scalar cubic pair flow solving:
///
/// `dx/dt = -(alpha / 2) * ((w + P) * |w + P| + (w - P) * |w - P|)`, `w = x - target`,
///
/// with dimensionless exposure `eta = alpha * dt >= 0` and ambient headroom
/// `bias = P >= 0`.
///
/// - Inner acoustic branch (`|w| <= P`): exact linear exponential contraction
///   `w^+ = w * exp(-2 * eta * P)` with zero intermodulation distortion.
/// - Outer shock branch (`|w| > P`): exact finite-time tangent clamping into
///   `|w| <= P` with seamless boundary transition.
/// - Bare-cubic limit (`P <= 0`): `w^+ = w / (1 + eta * |w|)`.
#[inline]
pub fn atomic_pair_scalar(x: f64, target: f64, bias: f64, eta: f64) -> f64 {
    let d = x - target;
    let a = d.abs();
    if eta <= 0.0 || a == 0.0 {
        return x;
    }
    if bias <= 0.0 {
        return target + d / (1.0 + eta * a);
    }
    let clock = eta * bias;
    if a <= bias {
        return target + d * (-2.0 * clock).exp();
    }
    let cross = (a - bias).atan2(a + bias);
    let after = if clock < cross {
        let t = if clock < 1e-6 {
            eta * (1.0 + (clock * clock) / 3.0)
        } else {
            clock.tan() / bias
        };
        (a - bias * bias * t) / (1.0 + a * t)
    } else {
        bias * (-2.0 * (clock - cross)).exp()
    };
    if d < 0.0 {
        target - after
    } else {
        target + after
    }
}

/// Exact `U(1)`-invariant radial opposing-biased cubic pair flow in `C`.
///
/// Contracts the complex residual `w = (xr - vr) + i * (xi - vi)` radially
/// toward target `v = vr + i * vi` using [`atomic_pair_scalar`], preserving
/// strict convex passivity `|z^+ - v| <= |z - v|` and `|z^+| <= max(|z|, |v|)`.
#[inline]
pub fn radial_pair_step(xr: f64, xi: f64, vr: f64, vi: f64, bias: f64, eta: f64) -> (f64, f64) {
    let wx = xr - vr;
    let wy = xi - vi;
    let r = (wx * wx + wy * wy).sqrt();
    if eta <= 0.0 || r == 0.0 {
        return (xr, xi);
    }
    let scale = if bias <= 0.0 {
        1.0 / (1.0 + eta * r)
    } else if r <= bias {
        (-2.0 * eta * bias).exp()
    } else {
        atomic_pair_scalar(r, 0.0, bias, eta) / r
    };
    (vr + wx * scale, vi + wy * scale)
}

/// Exact algebraic `C^inf` slope-invariant asymmetric jet deflection curve:
///
/// `a = 0.85 * asym`, `u = x / scale + a`,
/// `f(x; scale, asym) = scale * (1 + a^2)^(3/2) * (u / sqrt(1 + u^2) - a / sqrt(1 + a^2))`.
///
/// Returns `0.0` when `scale <= 0.0`. Guarantees bitwise `f(0.0) == 0.0` and
/// exact unit slope `f'(0) == 1.0` for all `asym` in `(-1, 1)`.
#[inline]
pub fn atomic_jet(x: f64, scale: f64, asym: f64) -> f64 {
    if scale <= 0.0 {
        return 0.0;
    }
    let a = 0.85 * asym;
    let a2 = a * a;
    let one_plus_a2 = 1.0 + a2;
    let sqrt_one_plus_a2 = one_plus_a2.sqrt();
    let inv_na = 1.0 / sqrt_one_plus_a2;
    let u = x / scale + a;
    let inv_nu = 1.0 / (1.0 + u * u).sqrt();
    scale * (u * inv_nu - a * inv_na) * one_plus_a2 * sqrt_one_plus_a2
}

/// Shared exact closed-form radial contraction and optional Kerr amplitude integral
/// `int_0^dt a(s) ds` for [`joint_free_kerr`] and [`joint_free_radial`].
#[inline(always)]
fn joint_free_radial_and_integral(
    a0: f64,
    g: f64,
    lambda: f64,
    patm: f64,
    dt: f64,
    need_integral: bool,
) -> (f64, f64) {
    let g = g.max(0.0);
    let lambda = lambda.max(0.0);

    if patm > 0.0 && a0 > patm && lambda > 0.0 {
        let alpha = lambda / (2.0 * patm);
        let a_coeff = g + alpha;
        let b_coeff = alpha * patm * patm;
        let sq_ab = (a_coeff * b_coeff).sqrt();
        let sq_a_over_b = (a_coeff / b_coeff).sqrt();
        let sq_b_over_a = (b_coeff / a_coeff).sqrt();
        let cross_clock = ((a0 - patm) * sq_ab).atan2(b_coeff + a_coeff * a0 * patm);
        let clock = sq_ab * dt;

        let a_end;
        let int_r;
        if clock < cross_clock {
            let tc = clock.tan();
            let denom = 1.0 + a0 * sq_a_over_b * tc;
            a_end = (a0 - sq_b_over_a * tc) / denom;
            if need_integral {
                let da = ((a_coeff * a0 * a0 + b_coeff) * tc) / (sq_ab * denom);
                int_r = ((a_coeff * da * (a0 + a_end)) / (a_coeff * a_end * a_end + b_coeff))
                    .ln_1p()
                    / (2.0 * a_coeff);
            } else {
                int_r = 0.0;
            }
        } else {
            let t_cross = cross_clock / sq_ab;
            let rem = if dt > t_cross { dt - t_cross } else { 0.0 };
            let rem_t_eff = rem * exprel_neg(lambda * rem);
            let rem_exp = (-lambda * rem).exp();
            let u_rem = g * patm * rem_t_eff;
            a_end = (patm * rem_exp) / (1.0 + u_rem);
            if need_integral {
                let int_outer = ((a_coeff * (a0 - patm) * (a0 + patm))
                    / (a_coeff * patm * patm + b_coeff))
                    .ln_1p()
                    / (2.0 * a_coeff);
                let int_inner = patm * rem_t_eff * log1p_div(u_rem);
                int_r = int_outer + int_inner;
            } else {
                int_r = 0.0;
            }
        }

        return (a_end / a0, int_r);
    }

    let t_eff = dt * exprel_neg(lambda * dt);
    let exp_l = (-lambda * dt).exp();
    let u = g * a0 * t_eff;
    let rad_scale = exp_l / (1.0 + u);
    let int_r = if need_integral {
        a0 * t_eff * log1p_div(u)
    } else {
        0.0
    };
    (rad_scale, int_r)
}

/// Exact closed-form 1-body joint continuous free flow integrating linear
/// radiation `lambda`, bare-cubic drag `g`, outer Riccati shock clamping above
/// ambient headroom `patm` (with exact mid-step boundary crossing at `|z| = patm`),
/// natural angular rotation `omega`, and State-Kerr pitch coupling `beta` in one
/// splitting-free step.
#[allow(clippy::too_many_arguments)]
#[inline]
pub fn joint_free_kerr(
    xr: f64,
    xi: f64,
    omega: f64,
    beta: f64,
    g: f64,
    lambda: f64,
    patm: f64,
    dt: f64,
) -> (f64, f64) {
    let a0 = (xr * xr + xi * xi).sqrt();
    if a0 == 0.0 || dt <= 0.0 {
        return (xr, xi);
    }
    let need_kerr = beta != 0.0;
    let (rad_scale, int_r) = joint_free_radial_and_integral(a0, g, lambda, patm, dt, need_kerr);
    let dphi = if need_kerr {
        omega * (dt + beta * int_r)
    } else {
        omega * dt
    };
    let (s, c) = dphi.sin_cos();
    ((xr * c - xi * s) * rad_scale, (xr * s + xi * c) * rad_scale)
}

/// Exact closed-form 1-body radial contraction without phase rotation.
///
/// Evaluates the same interior linear + bare-cubic flow and outer Riccati shock
/// branch (including mid-step crossing at `|z| = patm`) as [`joint_free_kerr`]
/// without computing trigonometric phase rotation.
#[inline]
pub fn joint_free_radial(xr: f64, xi: f64, g: f64, lambda: f64, patm: f64, dt: f64) -> (f64, f64) {
    let a0 = (xr * xr + xi * xi).sqrt();
    if a0 == 0.0 || dt <= 0.0 {
        return (xr, xi);
    }
    let (rad_scale, _) = joint_free_radial_and_integral(a0, g, lambda, patm, dt, false);
    (xr * rad_scale, xi * rad_scale)
}

/// Exact lossless rational continuous phase flow on the circle `|z| = R`
/// solving `dz/dt = i * gamma * (|z| + Re(z)) * z` with zero trigonometric calls.
///
/// Uses the cancellation-free identity `R + x = y^2 / (R - x)` for `x < 0` so
/// exact radius preservation, semigroup composition, time-reversibility, and
/// fixed-point stability at `z = -R` hold to machine precision.
#[inline]
pub fn cubic_phase_flow(x: f64, y: f64, gamma: f64, dt: f64) -> (f64, f64) {
    let r = (x * x + y * y).sqrt();
    if r <= 0.0 {
        return (0.0, 0.0);
    }
    let exp = 0.5 * gamma * dt;
    let r_plus_x = if x >= 0.0 { r + x } else { (y * y) / (r - x) };
    let a = 1.0 + exp * y;
    let b = exp * r_plus_x;
    let inv = 1.0 / (a * a + b * b);
    let c = (a * a - b * b) * inv;
    let s = (2.0 * a * b) * inv;
    (c * x - s * y, s * x + c * y)
}

/// Exact perpetual periodic two-chart rational phase flow on the circle `|z| = R`
/// solving `dz/dt = i * gamma * (|z| + |Re(z)|) * z` with exact period
/// `T = 4 / (gamma * R)` and zero trigonometric calls.
///
/// Encodes `(x, y)` into the period-4 stereographic coordinate `p in [-1, 3)`
/// obeying `dp/dt = gamma * R`, wraps `p` cleanly via Euclidean remainder, and
/// decodes rationally back onto `|z| = R` with exact radius preservation,
/// semigroup composition, and time-reversibility.
#[inline]
pub fn cubic_periodic_phase_flow(x: f64, y: f64, gamma: f64, dt: f64) -> (f64, f64) {
    let r = (x * x + y * y).sqrt();
    if r <= 0.0 {
        return (0.0, 0.0);
    }
    let p = if x >= 0.0 {
        y / (r + x)
    } else {
        2.0 - y / (r - x)
    };
    let p_next = (p + gamma * r * dt + 1.0).rem_euclid(4.0) - 1.0;
    let (u, sigma) = if p_next <= 1.0 {
        (p_next, 1.0)
    } else {
        (p_next - 2.0, -1.0)
    };
    let u2 = u * u;
    let inv = (sigma * r) / (1.0 + u2);
    (inv * (1.0 - u2), inv * (2.0 * u))
}

/// Output of [`cubic_directed_transfer`], recording the updated donor amplitude
/// `x`, updated receiver energy `e`, cancellation-free residual `r = x_0 - x`,
/// and exact transferred quadratic energy `d_e = 0.5 * (x_0^2 - x^2)`.
#[derive(Clone, Copy, Debug, Default)]
pub struct DirectedTransfer {
    /// Updated donor amplitude `q = x / (1 + gamma * dt * |x|)`.
    pub x: f64,
    /// Updated receiver energy `e + d_e`.
    pub e: f64,
    /// Cancellation-free removed amplitude residual `r = q * gamma * dt * |x|`.
    pub r: f64,
    /// Exact quadratic energy transferred `d_e = r * (q + 0.5 * r)`.
    pub d_e: f64,
}

/// Exact directed one-way energy routing from bare-cubic donor state `x` into
/// non-negative receiver energy register `e`, integrating
/// `dx/dt = -gamma * |x| * x`, `de/dt = gamma * |x|^3` identically over `dt >= 0`.
///
/// Strictly conserves `0.5 * x^2 + e` and satisfies exact semigroup telescoping.
#[inline]
pub fn cubic_directed_transfer(x: f64, e: f64, gamma: f64, dt: f64) -> DirectedTransfer {
    let ax = x.abs();
    let u = gamma * dt * ax;
    let mut q = x;
    let mut r = 0.0;
    let mut d_e = 0.0;
    if ax > 0.0 && u > 0.0 {
        q = x / (1.0 + u);
        r = q * u;
        d_e = r * (q + 0.5 * r);
    }
    DirectedTransfer {
        x: q,
        e: e + d_e,
        r,
        d_e,
    }
}

/// Exact cancellation-free Bernoulli orifice volume transfer `dv` (`m^3`) across
/// aperture `area` (`m^2`) and equivalent pneumatic compliance `c_eq` (`m^3 / Pa`)
/// driven by initial gauge pressure drop `dp0` (`Pa`) over step `dt` (`s`).
#[inline]
pub fn bernoulli_valve_exact(dp0: f64, area: f64, c_eq: f64, dt: f64) -> f64 {
    if dp0 == 0.0 || area <= 0.0 || c_eq <= 0.0 || dt <= 0.0 {
        return 0.0;
    }
    let s0 = dp0.abs().sqrt();
    let c_rate = (area * (2.0 / AIR_RHO).sqrt()) / (2.0 * c_eq);
    let ds = s0.min(c_rate * dt);
    let d_dp_mag = ds * (2.0 * s0 - ds);
    let d_dp = if dp0 >= 0.0 { d_dp_mag } else { -d_dp_mag };
    c_eq * d_dp
}

/// Exact cancellation-free Bernoulli wind-chest supply recharge from supply
/// pressure `p_sup` into reservoir pressure `p` with compliance `c_air` and
/// supply aperture `a_sup` over step `dt`.
///
/// Returns `(p_new, w_sup, d_sup)`, where `w_sup` is supply work (`J`) and
/// `d_sup >= 0` is orifice dissipation (`J`), conserving
/// `0.5 * c_air * (p_new^2 - p^2) == w_sup - d_sup` to machine precision.
#[inline]
pub fn bernoulli_supply_exact(
    p: f64,
    p_sup: f64,
    c_air: f64,
    a_sup: f64,
    dt: f64,
) -> (f64, f64, f64) {
    let dp0 = p_sup - p;
    if dp0 == 0.0 || c_air <= 0.0 || a_sup <= 0.0 || dt <= 0.0 {
        return (p, 0.0, 0.0);
    }
    let s0 = dp0.abs().sqrt();
    let c_rate = (a_sup * (2.0 / AIR_RHO).sqrt()) / (2.0 * c_air);
    let p_new = if c_rate * dt >= s0 {
        p_sup
    } else {
        let ds = c_rate * dt;
        let d_dp_mag = ds * (2.0 * s0 - ds);
        p + if dp0 >= 0.0 { d_dp_mag } else { -d_dp_mag }
    };
    let dv = c_air * (p_new - p);
    let w_sup = p_sup * dv;
    let d_sup = 0.5 * (dp0 + (p_sup - p_new)) * dv;
    (p_new, w_sup, d_sup)
}

/// Centered `C^2`-continuous rolling-shallot reed aperture closure over the
/// transition window `delta = 0.20 * x0`.
///
/// Maps `h_raw <= -0.5 * delta` to `0.0`, `h_raw >= 0.5 * delta` to `h_raw`,
/// and `(-0.5 * delta, 0.5 * delta)` via `delta * u^3 * (1 - 0.5 * u)` with
/// `u = (h_raw + 0.5 * delta) / delta`. Falls back to `h_raw.max(0.0)` when
/// `x0 <= 0.0`.
#[inline]
pub fn rolling_shallot_aperture(h_raw: f64, x0: f64) -> f64 {
    if x0 <= 0.0 {
        return h_raw.max(0.0);
    }
    let delta = 0.20 * x0;
    let half_delta = 0.5 * delta;
    if h_raw <= -half_delta {
        0.0
    } else if h_raw >= half_delta {
        h_raw
    } else {
        let u = (h_raw + half_delta) / delta;
        delta * u * u * u * (1.0 - 0.5 * u)
    }
}

/// Deterministic `xorshift64` PRNG returning uniform bipolar samples in `[-1.0, 1.0)`.
///
/// Automatically seeds a zero state with the 64-bit golden-ratio constant
/// `0x9E37_79B9_7F4A_7C15` so the generator never stalls at zero.
#[inline]
pub fn xorshift64_bipolar(state: &mut u64) -> f64 {
    let mut x = if *state == 0 {
        DEFAULT_RNG_SEED
    } else {
        *state
    };
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    ((x >> 11) as f64) * (1.0 / 9_007_199_254_740_992.0) * 2.0 - 1.0
}

/// Zero-frequency `CubicAtom` RC charge and cubic self-limiting discharge
/// capacitor solving `dc/dt = -lambda * c - g * |c| * c` during free discharge
/// and exact exponential RC charging during finite-duration trigger pulses.
#[derive(Clone, Copy, Debug)]
pub struct ZeroHzCapacitor {
    sample_rate: f64,
    h: f64,
    pub c: f64,
    pub remaining: u32,
    pub target: f64,
    tau_decay: f64,
    g: f64,
    tau_charge: f64,
    decay: f64,
    t_eff: f64,
    charge: f64,
}

impl ZeroHzCapacitor {
    /// Construct a zero-initialized capacitor at `sample_rate` Hz (falling back
    /// to `48_000.0` Hz if non-positive or non-finite).
    #[inline]
    pub fn new(sample_rate: f64, tau_decay: f64, g_cubic: f64, tau_charge: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let h = 1.0 / sr;
        let mut cap = Self {
            sample_rate: sr,
            h,
            c: 0.0,
            remaining: 0,
            target: 0.0,
            tau_decay: 0.0,
            g: 0.0,
            tau_charge: 0.0,
            decay: 1.0,
            t_eff: h,
            charge: 0.0,
        };
        cap.set_times(tau_decay, g_cubic, tau_charge);
        cap
    }

    /// Update linear decay time constant, bare-cubic self-limiting rate `g >= 0`,
    /// and finite-resistance charging time constant without allocating.
    #[inline]
    pub fn set_times(&mut self, tau_decay: f64, g_cubic: f64, tau_charge: f64) {
        self.tau_decay = tau_decay;
        self.g = if g_cubic.is_finite() && g_cubic > 0.0 {
            g_cubic
        } else {
            0.0
        };
        self.tau_charge = tau_charge;
        let lambda = if tau_decay > 0.0 {
            1.0 / tau_decay
        } else {
            0.0
        };
        self.decay = (-lambda * self.h).exp();
        self.t_eff = self.h * exprel_neg(lambda * self.h);
        self.charge = if tau_charge > 0.0 {
            (-self.h / tau_charge).exp()
        } else {
            0.0
        };
    }

    /// Begin a finite-resistance charge pulse toward absolute target `level`
    /// lasting `samples` audio frames.
    #[inline]
    pub fn trigger(&mut self, level: f64, samples: u32) {
        self.target = level;
        self.remaining = samples;
    }

    /// Begin an additive finite-resistance charge pulse toward `self.c + level`
    /// lasting `samples` audio frames.
    #[inline]
    pub fn add(&mut self, level: f64, samples: u32) {
        self.target = self.c + level;
        self.remaining = samples;
    }

    /// Reset capacitor charge and active pulse state to zero.
    #[inline]
    pub fn reset(&mut self) {
        self.c = 0.0;
        self.remaining = 0;
        self.target = 0.0;
    }

    /// Advance the capacitor by one sample and return the updated charge `c`.
    #[inline]
    pub fn step(&mut self) -> f64 {
        if self.remaining > 0 {
            self.c = self.target + (self.c - self.target) * self.charge;
            self.remaining -= 1;
        } else {
            let c = self.c;
            self.c = (c * self.decay) / (1.0 + self.g * c.abs() * self.t_eff);
        }
        self.c
    }

    /// Current capacitor charge value.
    #[inline]
    pub fn value(&self) -> f64 {
        self.c
    }

    /// Configured sample rate in Hz.
    #[inline]
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Configured linear discharge time constant in seconds.
    #[inline]
    pub fn tau_decay(&self) -> f64 {
        self.tau_decay
    }

    /// Configured finite-resistance charging time constant in seconds.
    #[inline]
    pub fn tau_charge(&self) -> f64 {
        self.tau_charge
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exprel_and_log1p_div_continuity() {
        assert!((exprel_neg(0.0) - 1.0).abs() < 1e-15);
        assert!((exprel_neg(1e-6 - 1e-12) - exprel_neg(1e-6 + 1e-12)).abs() < 1e-11);
        assert!((exprel_neg(1.0) - (1.0 - (-1.0_f64).exp())).abs() < 1e-15);

        assert!((log1p_div(0.0) - 1.0).abs() < 1e-15);
        assert!((log1p_div(1e-6 - 1e-12) - log1p_div(1e-6 + 1e-12)).abs() < 1e-11);
        assert!((log1p_div(1.0) - 2.0_f64.ln()).abs() < 1e-15);
    }

    #[test]
    fn cayley_and_omc_rotations_preserve_norm() {
        for &w in &[0.0, 1e-4, 0.05, 0.5, 1.2] {
            let (c, s) = cayley_rotation(w);
            assert!((c * c + s * s - 1.0).abs() < 1e-15);
        }
        let theta = TAU * 18.35 / 48_000.0;
        let (omc, sin) = omc_rotation_f32(theta);
        let (x, y) = apply_omc_rotation_f32(1.0, 0.0, omc, sin);
        assert!(((x as f64) - theta.cos()).abs() < 1e-7);
        assert!(((y as f64) - theta.sin()).abs() < 1e-7);
    }

    #[test]
    fn atomic_pair_and_radial_pair_semigroup() {
        let x0 = 3.5;
        let target = 0.25;
        let bias = 1.0;
        let e1 = 0.35;
        let e2 = 0.65;
        let one = atomic_pair_scalar(x0, target, bias, e1 + e2);
        let two = atomic_pair_scalar(atomic_pair_scalar(x0, target, bias, e1), target, bias, e2);
        assert!((one - two).abs() < 1e-14);

        let (rx, ry) = radial_pair_step(2.0, -1.5, 0.5, 0.25, 0.8, 0.4);
        let r0 = ((2.0 - 0.5_f64).powi(2) + (-1.5 - 0.25_f64).powi(2)).sqrt();
        let r1 = ((rx - 0.5).powi(2) + (ry - 0.25).powi(2)).sqrt();
        assert!((r1 - atomic_pair_scalar(r0, 0.0, 0.8, 0.4)).abs() < 1e-14);
    }

    #[test]
    fn atomic_jet_zero_and_unit_slope() {
        for &asym in &[-0.9, -0.35, 0.0, 0.42, 0.9] {
            assert_eq!(atomic_jet(0.0, 0.25, asym).to_bits(), 0);
            let eps = 1e-7;
            let slope = (atomic_jet(eps, 0.25, asym) - atomic_jet(-eps, 0.25, asym)) / (2.0 * eps);
            assert!((slope - 1.0).abs() < 1e-9);
        }
        assert_eq!(atomic_jet(0.5, 0.0, 0.3).to_bits(), 0);
    }

    #[test]
    fn joint_free_kerr_exact_semigroup_across_shock_boundary() {
        let omega = TAU * 220.0;
        let beta = 0.35;
        let g = 12.0;
        let lambda = 40.0;
        let patm = 1.0;
        let dt1 = 0.008;
        let dt2 = 0.017;

        let (x_one, y_one) = joint_free_kerr(2.4, -0.9, omega, beta, g, lambda, patm, dt1 + dt2);
        let (x_mid, y_mid) = joint_free_kerr(2.4, -0.9, omega, beta, g, lambda, patm, dt1);
        let (x_two, y_two) = joint_free_kerr(x_mid, y_mid, omega, beta, g, lambda, patm, dt2);
        assert!((x_one - x_two).abs() < 1e-13);
        assert!((y_one - y_two).abs() < 1e-13);

        let (rx, ry) = joint_free_radial(2.4, -0.9, g, lambda, patm, dt1 + dt2);
        let r_kerr = (x_one * x_one + y_one * y_one).sqrt();
        let r_rad = (rx * rx + ry * ry).sqrt();
        assert!((r_kerr - r_rad).abs() < 1e-14);
    }

    #[test]
    fn phase_flows_and_directed_transfer_invariants() {
        let (x1, y1) = cubic_phase_flow(0.6, 0.8, 3.0, 0.2);
        assert!(((x1 * x1 + y1 * y1).sqrt() - 1.0).abs() < 1e-14);
        let (xb, yb) = cubic_phase_flow(x1, y1, 3.0, -0.2);
        assert!((xb - 0.6).abs() < 1e-14 && (yb - 0.8).abs() < 1e-14);

        let gamma = 4.0 * 100.0;
        let (px, py) = cubic_periodic_phase_flow(0.6, 0.8, gamma, 0.01);
        assert!((px - 0.6).abs() < 1e-13 && (py - 0.8).abs() < 1e-13);

        let t1 = cubic_directed_transfer(1.5, 0.2, 2.0, 0.1);
        let t2 = cubic_directed_transfer(t1.x, t1.e, 2.0, 0.25);
        let t_all = cubic_directed_transfer(1.5, 0.2, 2.0, 0.35);
        assert!((0.5 * t_all.x * t_all.x + t_all.e - (0.5 * 1.5 * 1.5 + 0.2)).abs() < 1e-15);
        assert!((t2.e - t_all.e).abs() < 1e-15);
    }

    #[test]
    fn bernoulli_energy_balance_and_shallot_smoothness() {
        let c_air = 6.0e-6;
        let (p_new, w_sup, d_sup) =
            bernoulli_supply_exact(600.0, 850.0, c_air, 3.6e-4, 1.0 / 48_000.0);
        let d_e = 0.5 * c_air * (p_new * p_new - 600.0 * 600.0);
        assert!(d_sup >= 0.0);
        assert!((d_e - (w_sup - d_sup)).abs() < 1e-14);

        let dv = bernoulli_valve_exact(850.0, 1.0e-5, c_air, 1.0 / 48_000.0);
        assert!(dv > 0.0);

        let x0 = 0.95e-3;
        let half = 0.10 * x0;
        assert_eq!(rolling_shallot_aperture(-half, x0).to_bits(), 0);
        assert!((rolling_shallot_aperture(half, x0) - half).abs() < 1e-16);
        assert!(rolling_shallot_aperture(0.0, x0) > 0.0);
        assert_eq!(rolling_shallot_aperture(-1.0, 0.0).to_bits(), 0);
        assert!((rolling_shallot_aperture(0.5, 0.0) - 0.5).abs() < 1e-15);
    }

    #[test]
    fn xorshift_and_zero_hz_capacitor_basics() {
        let mut rng = 0u64;
        for _ in 0..64 {
            let v = xorshift64_bipolar(&mut rng);
            assert!((-1.0..1.0).contains(&v));
        }

        let mut cap = ZeroHzCapacitor::new(48_000.0, 0.145, 0.45, 0.0022);
        cap.trigger(1.0, 100);
        for _ in 0..100 {
            cap.step();
        }
        let peak = cap.value();
        assert!(peak > 0.5 && peak <= 1.0);
        for _ in 0..1000 {
            cap.step();
        }
        assert!(cap.value() < peak && cap.value() > 0.0);
    }
}
