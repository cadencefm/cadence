//! Zero-delay 96-mode `U(1)`-symmetric ambient-air modal room.
//!
//! Models a reverberant acoustic hall with zero delay lines using 96 complex
//! eigenmodes partitioned into 48 Even + 48 Odd pairs:
//! - Hybrid acoustic modal density: 18 log-spaced bass modes (38..260 Hz) joined
//!   to 78 power-law treble modes (260..9600 Hz) with deterministic jitter,
//!   scalable in real time via `size` without reallocating or clearing state.
//! - Opposing-biased ambient-air input port (`P_atm = 6.0`): strictly linear
//!   exponential contraction inside `P_atm` (zero port IMD) with smooth outer
//!   shock guard and physical air-absorption
//!   `RT60(f) = decay_seconds / (1 + (f / damping_hz)^1.45)` seconds.
//! - Gentle per-mode bare-cubic stone loss and passive mean-preserving Even/Odd
//!   reciprocal diffusion composed in a time-symmetric `O(h^2)` Strang schedule:
//!   `D(h/2) -> F(h/2) -> I(h) -> F(h/2) -> D(h/2)`, where `F = R o G` combines
//!   commuting eigenmode rotation and radial stone drag.
//! - Fibonacci golden-angle stereo injection and quadrature stereo readout for
//!   diffuse decorrelation.

use super::math::{atomic_pair_scalar, sanitize_sample_rate, DEFAULT_SAMPLE_RATE, TAU};
use crate::dsp::math::sanitize_clamp_f64;

/// Total number of complex acoustic eigenmodes in [`AmbientModalRoom`].
pub const ROOM_MODES: usize = 96;

/// Number of Even/Odd eigenmode pairs (`ROOM_MODES / 2`).
pub const ROOM_HALF: usize = ROOM_MODES / 2;

/// Lowest base room bass mode frequency in Hz at canonical `size = 0.5`.
pub const ROOM_F_MIN: f64 = 38.0;

/// Crossover frequency in Hz between log-spaced bass and power-law treble modes.
pub const ROOM_F_SPLIT: f64 = 260.0;

/// Upper nominal treble mode frequency ceiling in Hz (clamped below Nyquist).
pub const ROOM_F_MAX: f64 = 9600.0;

/// Linear ambient-air headroom threshold for the opposing-biased input port.
pub const ROOM_PATM: f64 = 6.0;

/// Canonical neutral room size parameter (`freq_scale == 1.0`).
pub const ROOM_DEFAULT_SIZE: f64 = 0.5;

/// Canonical reference hall low-frequency `RT60` decay time in seconds.
pub const ROOM_DEFAULT_DECAY_SECONDS: f64 = 2.65;

/// Minimum supported `RT60` decay time in seconds.
pub const ROOM_MIN_DECAY_SECONDS: f64 = 0.2;

/// Maximum supported `RT60` decay time in seconds.
pub const ROOM_MAX_DECAY_SECONDS: f64 = 12.0;

/// Canonical air-absorption crossover frequency in Hz.
pub const ROOM_DEFAULT_DAMPING_HZ: f64 = 3600.0;

/// Minimum supported air-absorption damping frequency in Hz.
pub const ROOM_MIN_DAMPING_HZ: f64 = 200.0;

/// Canonical neutral Even/Odd pair diffusion parameter (`diff_scale == 1.0`).
pub const ROOM_DEFAULT_DIFFUSION: f64 = 0.5;

/// Physical air-absorption power-law exponent in `RT60(f) = decay / (1 + (f / damp)^p)`.
pub const ROOM_AIR_ABSORPTION_EXP: f64 = 1.45;

/// Physical air-absorption `RT60` in seconds at frequency `freq_hz` Hz for low-frequency
/// `decay_seconds` and air-absorption crossover `damping_hz`.
#[inline]
pub fn room_mode_rt60(decay_seconds: f64, damping_hz: f64, freq_hz: f64) -> f64 {
    decay_seconds / (1.0 + (freq_hz / damping_hz).powf(ROOM_AIR_ABSORPTION_EXP))
}

/// Fibonacci golden angle `pi * (3 - sqrt(5))` in radians for diffuse stereo projection.
const GOLDEN_ANGLE: f64 = 2.399_963_229_728_653;

/// Number of log-spaced bass eigenmodes below [`ROOM_F_SPLIT`].
const BASS_MODES: usize = 18;

/// Threshold below which stored quadratic modal energy is treated as silent.
const QUIET_ENERGY_FLOOR: f64 = 1e-18;

/// Shared zero-delay 96-mode `U(1)`-symmetric ambient-air modal room.
#[derive(Clone, Debug)]
pub struct AmbientModalRoom {
    sample_rate: f64,
    h: f64,
    base_freqs: [f64; ROOM_MODES],
    ev_x: [f64; ROOM_HALF],
    ev_y: [f64; ROOM_HALF],
    od_x: [f64; ROOM_HALF],
    od_y: [f64; ROOM_HALF],
    ev_c: [f64; ROOM_HALF],
    ev_s: [f64; ROOM_HALF],
    od_c: [f64; ROOM_HALF],
    od_s: [f64; ROOM_HALF],
    ev_k: [f64; ROOM_HALF],
    od_k: [f64; ROOM_HALF],
    ev_sh: [f64; ROOM_HALF],
    od_sh: [f64; ROOM_HALF],
    ev_g: [f64; ROOM_HALF],
    od_g: [f64; ROOM_HALF],
    ev_il: [f64; ROOM_HALF],
    ev_ir: [f64; ROOM_HALF],
    od_il: [f64; ROOM_HALF],
    od_ir: [f64; ROOM_HALF],
    ev_orl: [f64; ROOM_HALF],
    ev_oil: [f64; ROOM_HALF],
    ev_orr: [f64; ROOM_HALF],
    ev_oir: [f64; ROOM_HALF],
    od_orl: [f64; ROOM_HALF],
    od_oil: [f64; ROOM_HALF],
    od_orr: [f64; ROOM_HALF],
    od_oir: [f64; ROOM_HALF],
    kd: [f64; ROOM_HALF],
}

impl AmbientModalRoom {
    #[inline]
    fn zeroed() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            h: 1.0 / DEFAULT_SAMPLE_RATE,
            base_freqs: [0.0; ROOM_MODES],
            ev_x: [0.0; ROOM_HALF],
            ev_y: [0.0; ROOM_HALF],
            od_x: [0.0; ROOM_HALF],
            od_y: [0.0; ROOM_HALF],
            ev_c: [0.0; ROOM_HALF],
            ev_s: [0.0; ROOM_HALF],
            od_c: [0.0; ROOM_HALF],
            od_s: [0.0; ROOM_HALF],
            ev_k: [0.0; ROOM_HALF],
            od_k: [0.0; ROOM_HALF],
            ev_sh: [0.0; ROOM_HALF],
            od_sh: [0.0; ROOM_HALF],
            ev_g: [0.0; ROOM_HALF],
            od_g: [0.0; ROOM_HALF],
            ev_il: [0.0; ROOM_HALF],
            ev_ir: [0.0; ROOM_HALF],
            od_il: [0.0; ROOM_HALF],
            od_ir: [0.0; ROOM_HALF],
            ev_orl: [0.0; ROOM_HALF],
            ev_oil: [0.0; ROOM_HALF],
            ev_orr: [0.0; ROOM_HALF],
            ev_oir: [0.0; ROOM_HALF],
            od_orl: [0.0; ROOM_HALF],
            od_oil: [0.0; ROOM_HALF],
            od_orr: [0.0; ROOM_HALF],
            od_oir: [0.0; ROOM_HALF],
            kd: [0.0; ROOM_HALF],
        }
    }

    fn init(&mut self, sample_rate: f64) {
        let sr = sanitize_sample_rate(sample_rate);
        self.sample_rate = sr;
        self.h = 1.0 / sr;

        let f_max = ROOM_F_MAX.min(0.45 * sr);
        for (k, slot) in self.base_freqs[..BASS_MODES].iter_mut().enumerate() {
            let kf = k as f64;
            *slot = ROOM_F_MIN
                * (ROOM_F_SPLIT / ROOM_F_MIN).powf(kf / BASS_MODES as f64)
                * (1.0 + 0.009 * (kf * 31.1 + 0.3).sin());
        }
        let n_hi = ROOM_MODES - BASS_MODES;
        let split_pow = ROOM_F_SPLIT.powf(1.35);
        let span_pow = f_max.powf(1.35) - split_pow;
        let inv_exp = 1.0 / 1.35;
        for (k, slot) in self.base_freqs[BASS_MODES..].iter_mut().enumerate() {
            let t = (k as f64 + 0.5) / n_hi as f64;
            *slot = (split_pow + t * span_pow).powf(inv_exp)
                * (1.0 + 0.006 * ((k + BASS_MODES) as f64 * 19.3 + 0.7).sin());
        }

        let out_gain = 0.65 / (ROOM_MODES as f64).sqrt();
        for j in 0..ROOM_HALF {
            let a_e = (2 * j) as f64 * GOLDEN_ANGLE;
            let a_o = (2 * j + 1) as f64 * GOLDEN_ANGLE;
            let (sin_ae, cos_ae) = a_e.sin_cos();
            let (sin_ao, cos_ao) = a_o.sin_cos();
            self.ev_il[j] = cos_ae;
            self.ev_ir[j] = sin_ae;
            self.od_il[j] = cos_ao;
            self.od_ir[j] = sin_ao;

            let th_el = a_e * 1.37 + 0.4;
            let th_er = a_e * 1.37 + 1.97;
            let th_ol = a_o * 1.43 + 0.9;
            let th_or = a_o * 1.43 + 2.47;
            let (sin_el, cos_el) = th_el.sin_cos();
            let (sin_er, cos_er) = th_er.sin_cos();
            let (sin_ol, cos_ol) = th_ol.sin_cos();
            let (sin_or, cos_or) = th_or.sin_cos();
            self.ev_orl[j] = out_gain * cos_el;
            self.ev_oil[j] = out_gain * sin_el;
            self.ev_orr[j] = out_gain * cos_er;
            self.ev_oir[j] = out_gain * sin_er;
            self.od_orl[j] = out_gain * cos_ol;
            self.od_oil[j] = out_gain * sin_ol;
            self.od_orr[j] = out_gain * cos_or;
            self.od_oir[j] = out_gain * sin_or;
        }

        self.configure(
            ROOM_DEFAULT_SIZE,
            ROOM_DEFAULT_DECAY_SECONDS,
            ROOM_DEFAULT_DAMPING_HZ,
            ROOM_DEFAULT_DIFFUSION,
        );
    }

    /// Reconfigure room size (`0..=1`), low-frequency `RT60` decay (`0.2..=12.0 s`),
    /// air-absorption damping (`200..=0.45*sr Hz`), and Even/Odd pair diffusion
    /// (`0..=1`) in place with zero allocations and without clearing modal state.
    pub fn configure(&mut self, size: f64, decay_seconds: f64, damping_hz: f64, diffusion: f64) {
        let sr = self.sample_rate;
        let nyq_cap = 0.45 * sr;
        let size = sanitize_clamp_f64(size, 0.0, 1.0, ROOM_DEFAULT_SIZE);
        let decay_seconds = sanitize_clamp_f64(
            decay_seconds,
            ROOM_MIN_DECAY_SECONDS,
            ROOM_MAX_DECAY_SECONDS,
            ROOM_DEFAULT_DECAY_SECONDS,
        );
        let damping_hz = sanitize_clamp_f64(
            damping_hz,
            ROOM_MIN_DAMPING_HZ,
            nyq_cap.max(ROOM_MIN_DAMPING_HZ),
            ROOM_DEFAULT_DAMPING_HZ,
        );
        let diffusion = sanitize_clamp_f64(diffusion, 0.0, 1.0, ROOM_DEFAULT_DIFFUSION);

        let freq_scale = 2.0_f64.powf(0.65 * (0.5 - size));
        let stone_scale = (ROOM_DEFAULT_DECAY_SECONDS / decay_seconds).clamp(0.25, 2.5);
        let diff_scale = 2.0 * diffusion;
        let ln_1000 = 1000.0_f64.ln();
        let h_half = 0.5 * self.h;

        for j in 0..ROOM_HALF {
            let f_e = (self.base_freqs[2 * j] * freq_scale).min(nyq_cap);
            let f_o = (self.base_freqs[2 * j + 1] * freq_scale).min(nyq_cap);
            let w_e = TAU * f_e;
            let w_o = TAU * f_o;

            // Half-step rotation, half-step stone drag, and half-step diffusion for symmetric Strang O(h^2).
            let (sin_e, cos_e) = (w_e * h_half).sin_cos();
            let (sin_o, cos_o) = (w_o * h_half).sin_cos();
            self.ev_c[j] = cos_e;
            self.ev_s[j] = sin_e;
            self.od_c[j] = cos_o;
            self.od_s[j] = sin_o;

            let rt_e = room_mode_rt60(decay_seconds, damping_hz, f_e);
            let rt_o = room_mode_rt60(decay_seconds, damping_hz, f_o);
            let lam_e = ln_1000 / rt_e;
            let lam_o = ln_1000 / rt_o;
            self.ev_k[j] = (-lam_e * self.h).exp();
            self.od_k[j] = (-lam_o * self.h).exp();
            self.ev_sh[j] = (lam_e * self.h) / (2.0 * ROOM_PATM);
            self.od_sh[j] = (lam_o * self.h) / (2.0 * ROOM_PATM);
            self.ev_g[j] = (0.35 + 0.00018 * f_e) * stone_scale * h_half;
            self.od_g[j] = (0.35 + 0.00018 * f_o) * stone_scale * h_half;

            let d_rate = diff_scale * (4.2 + 12.0 * (j as f64 / ROOM_HALF as f64));
            self.kd[j] = (-2.0 * d_rate * h_half).exp();
        }
    }

    /// Construct a 96-mode ambient-air room on the stack for `sample_rate` Hz.
    pub fn new(sample_rate: f64) -> Self {
        let mut room = Self::zeroed();
        room.init(sample_rate);
        room
    }

    /// Construct a heap-boxed 96-mode ambient-air room for `sample_rate` Hz,
    /// initializing precomputed tables directly in place to minimize debug stack usage.
    pub fn new_boxed(sample_rate: f64) -> Box<Self> {
        let mut room = Box::new(Self::zeroed());
        room.init(sample_rate);
        room
    }

    /// Configured sample rate in Hz.
    #[inline]
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Clear all 96 complex modal states to zero without allocating.
    #[inline]
    pub fn reset(&mut self) {
        self.ev_x.fill(0.0);
        self.ev_y.fill(0.0);
        self.od_x.fill(0.0);
        self.od_y.fill(0.0);
    }

    /// Total stored quadratic modal energy `0.5 * sum(|z_k|^2)` across all 96 modes.
    #[inline]
    pub fn energy(&self) -> f64 {
        let mut sum = 0.0;
        for j in 0..ROOM_HALF {
            sum += self.ev_x[j] * self.ev_x[j]
                + self.ev_y[j] * self.ev_y[j]
                + self.od_x[j] * self.od_x[j]
                + self.od_y[j] * self.od_y[j];
        }
        0.5 * sum
    }

    /// True when stored quadratic modal energy has decayed below `1e-18`.
    #[inline]
    pub fn is_quiet(&self) -> bool {
        self.energy() < QUIET_ENERGY_FLOOR
    }

    /// Advance all 96 ambient-air room modes by one stereo sample `[dl, dr]`
    /// and return the quadrature wet stereo output `[wl, wr]`.
    #[inline]
    pub fn step(&mut self, input: [f64; 2]) -> [f64; 2] {
        let dl = if input[0].is_finite() { input[0] } else { 0.0 };
        let dr = if input[1].is_finite() { input[1] } else { 0.0 };
        let mut wl = 0.0;
        let mut wr = 0.0;

        for j in 0..ROOM_HALF {
            let kd = self.kd[j];
            let ev_c = self.ev_c[j];
            let ev_s = self.ev_s[j];
            let od_c = self.od_c[j];
            let od_s = self.od_s[j];
            let ev_g = self.ev_g[j];
            let od_g = self.od_g[j];

            let mut ex = self.ev_x[j];
            let mut ey = self.ev_y[j];
            let mut ox = self.od_x[j];
            let mut oy = self.od_y[j];

            // 1. Half-step passive mean-preserving Even/Odd diffusion D(h/2).
            let mut mx = 0.5 * (ex + ox);
            let mut my = 0.5 * (ey + oy);
            let mut hdx = 0.5 * (ex - ox) * kd;
            let mut hdy = 0.5 * (ey - oy) * kd;
            ex = mx + hdx;
            ox = mx - hdx;
            ey = my + hdy;
            oy = my - hdy;

            // 2. Half-step exact commuting stone drag + rotation F(h/2) = R(h/2) o G(h/2).
            let mut f = 1.0 / (1.0 + ev_g * (ex * ex + ey * ey).sqrt());
            let mut nx = (ev_c * ex - ev_s * ey) * f;
            ey = (ev_s * ex + ev_c * ey) * f;
            ex = nx;

            f = 1.0 / (1.0 + od_g * (ox * ox + oy * oy).sqrt());
            nx = (od_c * ox - od_s * oy) * f;
            oy = (od_s * ox + od_c * oy) * f;
            ox = nx;

            // 3. Full-step opposing-biased ambient-air input port I(h).
            let ev_re = self.ev_il[j] * dl + self.ev_ir[j] * dr;
            let ev_im = -self.ev_ir[j] * dl + self.ev_il[j] * dr;
            let e1 = ex - ev_re;
            let e2 = ey - ev_im;
            let ed = (e1 * e1 + e2 * e2).sqrt();
            if ed <= ROOM_PATM {
                let rk = self.ev_k[j];
                ex = ev_re + e1 * rk;
                ey = ev_im + e2 * rk;
            } else {
                let sc = atomic_pair_scalar(ed, 0.0, ROOM_PATM, self.ev_sh[j]) / ed;
                ex = ev_re + e1 * sc;
                ey = ev_im + e2 * sc;
            }

            let od_re = self.od_il[j] * dl + self.od_ir[j] * dr;
            let od_im = -self.od_ir[j] * dl + self.od_il[j] * dr;
            let o1 = ox - od_re;
            let o2 = oy - od_im;
            let od = (o1 * o1 + o2 * o2).sqrt();
            if od <= ROOM_PATM {
                let rk = self.od_k[j];
                ox = od_re + o1 * rk;
                oy = od_im + o2 * rk;
            } else {
                let sc = atomic_pair_scalar(od, 0.0, ROOM_PATM, self.od_sh[j]) / od;
                ox = od_re + o1 * sc;
                oy = od_im + o2 * sc;
            }

            // 4. Closing half-step commuting stone drag + rotation F(h/2).
            f = 1.0 / (1.0 + ev_g * (ex * ex + ey * ey).sqrt());
            nx = (ev_c * ex - ev_s * ey) * f;
            ey = (ev_s * ex + ev_c * ey) * f;
            ex = nx;

            f = 1.0 / (1.0 + od_g * (ox * ox + oy * oy).sqrt());
            nx = (od_c * ox - od_s * oy) * f;
            oy = (od_s * ox + od_c * oy) * f;
            ox = nx;

            // 5. Closing half-step passive mean-preserving Even/Odd diffusion D(h/2).
            mx = 0.5 * (ex + ox);
            my = 0.5 * (ey + oy);
            hdx = 0.5 * (ex - ox) * kd;
            hdy = 0.5 * (ey - oy) * kd;
            ex = mx + hdx;
            ox = mx - hdx;
            ey = my + hdy;
            oy = my - hdy;
            self.ev_x[j] = ex;
            self.ev_y[j] = ey;
            self.od_x[j] = ox;
            self.od_y[j] = oy;

            wl += self.ev_orl[j] * ex
                + self.ev_oil[j] * ey
                + self.od_orl[j] * ox
                + self.od_oil[j] * oy;
            wr += self.ev_orr[j] * ex
                + self.ev_oir[j] * ey
                + self.od_orr[j] * ox
                + self.od_oir[j] * oy;
        }

        [wl, wr]
    }
}
