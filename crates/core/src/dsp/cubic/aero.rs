//! Self-contained single-voice aeroacoustic physical primitives:
//! - [`FlueJet`]: 1-complex-atom damped convective air jet coupled two-way with a modal bore.
//! - [`BrassReed`]: 2-way symplectic mechanical brass reed tongue coupled to a pneumatic wind
//!   plenum and conical bore with `C^2` rolling-shallot closure, exact Bernoulli valve flows,
//!   and exact SI Joule energy accounting.
//!
//! All real-time stepping paths are zero-allocation, lock-free, and panic-free.

use core::f64::consts::PI;

use super::math::{
    atomic_jet, atomic_pair_scalar, bernoulli_supply_exact, bernoulli_valve_exact, exprel_neg,
    rolling_shallot_aperture, sanitize_sample_rate, xorshift64_bipolar, DEFAULT_RNG_SEED,
    MAX_ATOMS, NOMINAL_CHEST_PA, SQRT3, TAU,
};
use super::modal::{
    body_ratio, couple_pairs, finite_or, joint_free_mode, sanitize_fundamental, triad_half_gh,
    BodyGeometry, CoupleParams, FreeStepCoeffs, PairTopology, TriadAxis, TriadCache,
};

/// Dimensionless energy floor below which an ungated aeroacoustic bore tail is considered quiet.
const AERO_QUIET_FLOOR: f64 = 1e-14;

#[inline]
fn sanitize_seed(seed: u64) -> u64 {
    if seed == 0 {
        DEFAULT_RNG_SEED
    } else {
        seed
    }
}

/// Configuration parameters for a single-voice [`FlueJet`].
#[derive(Clone, Copy, Debug)]
pub struct FlueJetConfig {
    pub fundamental_hz: f64,
    pub atoms: usize,
    pub geometry: BodyGeometry,
    pub drive: f64,
    pub velocity: f64,
    pub embouchure: f64,
    pub brightness: f64,
    pub absorption: f64,
    pub radiation: f64,
    pub coupling: f64,
    pub pair_topology: PairTopology,
    pub kerr_semitones: f64,
    pub triad_amount: f64,
    pub triad_axis: TriadAxis,
    pub spread: f64,
    pub noise_enabled: bool,
    pub seed: u64,
}

impl Default for FlueJetConfig {
    fn default() -> Self {
        Self {
            fundamental_hz: 220.0,
            atoms: 12,
            geometry: BodyGeometry::Harmonic,
            drive: 0.6,
            velocity: 0.8,
            embouchure: 0.55,
            brightness: 0.5,
            absorption: 0.045,
            radiation: 0.15,
            coupling: 0.0,
            pair_topology: PairTopology::Star,
            kerr_semitones: 0.0,
            triad_amount: 0.0,
            triad_axis: TriadAxis::Sum,
            spread: 0.35,
            noise_enabled: true,
            seed: DEFAULT_RNG_SEED,
        }
    }
}

/// Single-voice 1-complex-atom damped convective air jet coupled two-way with a modal bore
/// through a time-symmetric Strang `O(h^2)` integrator.
#[derive(Clone, Debug)]
pub struct FlueJet {
    sample_rate: f64,
    h_full: f64,
    h_half: f64,
    inv_sqrt_h: f64,
    n: usize,
    gate: bool,
    pair_topology: PairTopology,
    passive: f64,
    output_gain: f64,

    qx: f64,
    qy: f64,
    jet_gain: f64,
    jet_scale: f64,
    jet_asym: f64,
    jet_p0: f64,
    jet_eta: f64,
    mouth_eta: f64,
    jet_c: f64,
    jet_s: f64,
    noise_amp: f64,
    noise_enabled: bool,
    configured_seed: u64,
    seed_initialized: bool,
    rng_state: u64,

    x: [f64; MAX_ATOMS],
    y: [f64; MAX_ATOMS],
    omega: [f64; MAX_ATOMS],
    rot_c: [f64; MAX_ATOMS],
    rot_s: [f64; MAX_ATOMS],
    exp_h: [f64; MAX_ATOMS],
    t_eff_h: [f64; MAX_ATOMS],
    beta: [f64; MAX_ATOMS],
    g: [f64; MAX_ATOMS],
    lam: [f64; MAX_ATOMS],
    bore_patm: [f64; MAX_ATOMS],
    weight: [f64; MAX_ATOMS],
    read: [f64; MAX_ATOMS],
    pan_c: [f64; MAX_ATOMS],
    pan_s: [f64; MAX_ATOMS],

    triads: TriadCache,
}

impl FlueJet {
    fn zeroed(sample_rate: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let h_full = 1.0 / sr;
        Self {
            sample_rate: sr,
            h_full,
            h_half: 0.5 * h_full,
            inv_sqrt_h: sr.sqrt(),
            n: 0,
            gate: false,
            pair_topology: PairTopology::Star,
            passive: 0.0,
            output_gain: 1.15,
            qx: 0.0,
            qy: 0.0,
            jet_gain: 2.5,
            jet_scale: 0.08,
            jet_asym: 0.0,
            jet_p0: 0.65,
            jet_eta: 0.0,
            mouth_eta: 0.0,
            jet_c: 1.0,
            jet_s: 0.0,
            noise_amp: 0.0,
            noise_enabled: true,
            configured_seed: DEFAULT_RNG_SEED,
            seed_initialized: false,
            rng_state: DEFAULT_RNG_SEED,
            x: [0.0; MAX_ATOMS],
            y: [0.0; MAX_ATOMS],
            omega: [0.0; MAX_ATOMS],
            rot_c: [1.0; MAX_ATOMS],
            rot_s: [0.0; MAX_ATOMS],
            exp_h: [1.0; MAX_ATOMS],
            t_eff_h: [0.0; MAX_ATOMS],
            beta: [0.0; MAX_ATOMS],
            g: [0.0; MAX_ATOMS],
            lam: [0.0; MAX_ATOMS],
            bore_patm: [2.0; MAX_ATOMS],
            weight: [0.0; MAX_ATOMS],
            read: [0.0; MAX_ATOMS],
            pan_c: [1.0; MAX_ATOMS],
            pan_s: [0.0; MAX_ATOMS],
            triads: TriadCache::empty(),
        }
    }

    /// Construct a new [`FlueJet`] configured for `sample_rate`.
    pub fn new(sample_rate: f64, config: FlueJetConfig) -> Self {
        let mut flue = Self::zeroed(sample_rate);
        flue.configure(config);
        flue
    }

    /// Construct a heap-allocated [`FlueJet`] to keep debug stack frames compact.
    pub fn new_boxed(sample_rate: f64, config: FlueJetConfig) -> Box<Self> {
        let mut flue = Box::new(Self::zeroed(sample_rate));
        flue.configure(config);
        flue
    }

    /// Reconfigure convective jet, bore modes, coupling, and resonant triads without allocating.
    pub fn configure(&mut self, config: FlueJetConfig) {
        let f_base = sanitize_fundamental(config.fundamental_hz, self.sample_rate);
        let w_body = TAU * f_base;

        let req_n = config.atoms.clamp(1, MAX_ATOMS);
        let mut n = req_n;
        while n > 1 && f_base * body_ratio(config.geometry, n - 1, req_n) > 0.44 * self.sample_rate
        {
            n -= 1;
        }
        if n > self.n {
            self.x[self.n..n].fill(0.0);
            self.y[self.n..n].fill(0.0);
        } else if n < self.n {
            self.x[n..self.n].fill(0.0);
            self.y[n..self.n].fill(0.0);
        }
        self.n = n;
        self.pair_topology = config.pair_topology;

        let h = self.h_full;
        let h_half = self.h_half;
        let drive = finite_or(config.drive, 0.6);
        let velocity = finite_or(config.velocity, 0.8);
        let brightness = finite_or(config.brightness, 0.5);
        let embouchure = finite_or(config.embouchure, 0.55);
        let xi = finite_or(config.absorption, 0.045).max(0.0);
        let radiation = finite_or(config.radiation, 0.15).max(0.0);
        let coupling = finite_or(config.coupling, 0.0).max(0.0);
        let kerr_st = finite_or(config.kerr_semitones, 0.0);
        let spread = finite_or(config.spread, 0.0);

        let p_scale =
            (0.60 + 0.40 * drive).max(0.0).sqrt() * (0.60 + 0.40 * velocity).max(0.0).sqrt();
        self.jet_gain = 2.05 + 0.45 * drive;
        self.jet_scale = ((0.060 + 0.048 * velocity) * (0.88 + 0.24 * brightness)).max(1e-9);
        self.jet_asym = 0.68 * embouchure;
        self.jet_p0 = 0.65 * p_scale;
        let alpha_jet = (0.50 * w_body) / (2.0 * 0.65);
        self.jet_eta = 0.5 * alpha_jet * h;
        let w_scale = TAU * 220.0 * (f_base.max(16.0) / 220.0).powf(0.65);
        self.mouth_eta = 0.38 * w_scale * h;
        let jet_tune = 0.96 + 0.04 * p_scale;
        let (js, jc) = (h_half * w_body * jet_tune).sin_cos();
        self.jet_c = jc;
        self.jet_s = js;
        self.noise_amp = SQRT3 * 1.4e-5 * self.inv_sqrt_h * p_scale;
        self.noise_enabled = config.noise_enabled;
        if !self.seed_initialized || config.seed != self.configured_seed {
            self.configured_seed = config.seed;
            self.rng_state = sanitize_seed(config.seed);
            self.seed_initialized = true;
        }

        let beta_base = 2.0_f64.powf(kerr_st / 12.0) - 1.0;
        let tilt_exp = -1.45 + 2.55 * brightness;
        let mut w_sq = 0.0;
        let mut r_sq = 0.0;
        for k in 0..n {
            let ratio = body_ratio(config.geometry, k, req_n);
            let om = w_body * ratio;
            self.omega[k] = om;
            let (sh, ch) = (om * h_half).sin_cos();
            self.rot_c[k] = ch;
            self.rot_s[k] = sh;
            self.beta[k] = beta_base;

            let drv_w = ((k + 1) as f64).powf(-0.78);
            self.weight[k] = drv_w;
            w_sq += drv_w * drv_w;

            let rad_w = ratio.powf(tilt_exp);
            self.read[k] = rad_w;
            r_sq += rad_w * rad_w;

            self.g[k] = 0.65 * (0.045 + 0.025 * k as f64) * w_scale * (0.4 + 2.0 * xi);
            let lam = 0.011 * w_scale * ratio.powf(0.45) * (0.15 + 1.5 * radiation);
            self.lam[k] = lam;
            self.bore_patm[k] = 2.0;
            self.exp_h[k] = (-lam * h_half).exp();
            self.t_eff_h[k] = h_half * exprel_neg(lam * h_half);

            let sign = if k & 1 == 0 { 1.0 } else { -1.0 };
            let ang = spread * sign * (0.20 + 0.85 * ((k + 1) as f64 / n as f64));
            let (ps, pc) = (0.5 * ang).sin_cos();
            self.pan_c[k] = pc;
            self.pan_s[k] = ps;
        }

        let w_norm = w_sq.sqrt();
        let w_inv = if w_norm > 0.0 { 1.0 / w_norm } else { 1.0 };
        let r_norm = r_sq.sqrt();
        let r_inv = if r_norm > 0.0 { 1.0 / r_norm } else { 1.0 };
        for k in 0..n {
            self.weight[k] *= w_inv;
            self.read[k] *= r_inv;
        }

        self.passive = 0.035 * w_body * coupling * h;
        self.output_gain = 1.15;

        let half_gh = triad_half_gh(config.triad_amount, w_body, n, h);
        self.triads.ensure(
            config.geometry,
            req_n,
            n,
            &self.omega,
            half_gh,
            config.triad_axis,
        );
    }

    /// Reset jet, bore state, gate, and PRNG to clean silence while preserving configuration.
    pub fn reset(&mut self) {
        self.x.fill(0.0);
        self.y.fill(0.0);
        self.qx = 0.0;
        self.qy = 0.0;
        self.gate = false;
        self.rng_state = sanitize_seed(self.configured_seed);
    }

    /// Open or close the blowing gate.
    #[inline]
    pub fn set_gate(&mut self, gate: bool) {
        self.gate = gate;
    }

    /// Current blowing gate state.
    #[inline]
    pub fn gate(&self) -> bool {
        self.gate
    }

    /// Number of active bore modes below the Nyquist safety ceiling.
    #[inline]
    pub fn active_modes(&self) -> usize {
        self.n
    }

    /// Complex convective jet state `(qx, qy)`.
    #[inline]
    pub fn jet_state(&self) -> (f64, f64) {
        (self.qx, self.qy)
    }

    /// Set complex convective jet state `(qx, qy)`.
    #[inline]
    pub fn set_jet_state(&mut self, qx: f64, qy: f64) {
        self.qx = qx;
        self.qy = qy;
    }

    #[inline]
    fn joint_free_half(&mut self, k: usize) {
        let (nx, ny) = joint_free_mode(
            self.x[k],
            self.y[k],
            FreeStepCoeffs {
                omega: self.omega[k],
                beta: self.beta[k],
                g: self.g[k],
                lam: self.lam[k],
                patm: self.bore_patm[k],
                dt: self.h_half,
                exp_l: self.exp_h[k],
                t_eff: self.t_eff_h[k],
                rot_c: self.rot_c[k],
                rot_s: self.rot_s[k],
            },
        );
        self.x[k] = nx;
        self.y[k] = ny;
    }

    /// Advance one audio sample using the internal deterministic PRNG when noise is enabled.
    pub fn step(&mut self) -> [f64; 2] {
        let noise = if self.gate && self.noise_enabled {
            xorshift64_bipolar(&mut self.rng_state)
        } else {
            0.0
        };
        self.step_with_noise(noise)
    }

    /// Advance one audio sample using caller-supplied jet turbulence sample `noise_sample`
    /// instead of the internal PRNG.
    pub fn step_with_noise(&mut self, noise_sample: f64) -> [f64; 2] {
        let n = self.n;
        let couple = CoupleParams {
            n,
            topology: self.pair_topology,
            passive: self.passive,
            headroom: 0.0,
        };

        for k in 0..n {
            self.joint_free_half(k);
        }
        couple_pairs(&mut self.x, &mut self.y, couple, false, 0.5);
        self.triads.apply_pass(&self.x, &mut self.y, n, true, false);

        let mut qx = self.jet_c * self.qx - self.jet_s * self.qy;
        let mut qy = self.jet_s * self.qx + self.jet_c * self.qy;
        let mut ux = 0.0;
        let mut uy = 0.0;
        for k in 0..n {
            ux += self.weight[k] * self.x[k];
            uy += self.weight[k] * self.y[k];
        }

        let g = if self.gate { 1.0 } else { 0.0 };
        qx = atomic_pair_scalar(qx, g * ux, self.jet_p0, self.jet_eta);
        qy = atomic_pair_scalar(qy, g * uy, self.jet_p0, self.jet_eta);
        if self.gate {
            let ctr = self.jet_gain * atomic_jet(qx, self.jet_scale, self.jet_asym)
                + noise_sample * self.noise_amp;
            let u1 = atomic_pair_scalar(ux, ctr, self.jet_p0, self.mouth_eta);
            let du = u1 - ux;
            for k in 0..n {
                self.x[k] += self.weight[k] * du;
            }
            ux = u1;
        }
        qx = atomic_pair_scalar(qx, g * ux, self.jet_p0, self.jet_eta);
        qy = atomic_pair_scalar(qy, g * uy, self.jet_p0, self.jet_eta);
        self.qx = self.jet_c * qx - self.jet_s * qy;
        self.qy = self.jet_s * qx + self.jet_c * qy;

        self.triads.apply_pass(&self.x, &mut self.y, n, true, true);
        couple_pairs(&mut self.x, &mut self.y, couple, true, 0.5);
        for k in (0..n).rev() {
            self.joint_free_half(k);
        }

        self.read_stereo()
    }

    /// Zero-comb quadrature stereo readout `[left, right]`.
    #[inline]
    pub fn read_stereo(&self) -> [f64; 2] {
        let mut l = 0.0;
        let mut r = 0.0;
        let g_out = self.output_gain;
        for k in 0..self.n {
            let xk = self.x[k];
            let yk = self.y[k];
            let w = g_out * self.read[k];
            l += w * (self.pan_c[k] * xk - self.pan_s[k] * yk);
            r += w * (self.pan_c[k] * xk + self.pan_s[k] * yk);
        }
        [l, r]
    }

    /// Total squared state energy `qx^2 + qy^2 + sum_{k < n} (x[k]^2 + y[k]^2)`.
    #[inline]
    pub fn energy(&self) -> f64 {
        let mut en = self.qx * self.qx + self.qy * self.qy;
        for k in 0..self.n {
            en += self.x[k] * self.x[k] + self.y[k] * self.y[k];
        }
        en
    }

    /// True when the blowing gate is closed and combined jet and bore energy has decayed
    /// below `1e-14`.
    #[inline]
    pub fn is_quiet(&self) -> bool {
        !self.gate && self.energy() < AERO_QUIET_FLOOR
    }
}

/// Configuration parameters for a single-voice [`BrassReed`].
#[derive(Clone, Copy, Debug)]
pub struct BrassReedConfig {
    pub fundamental_hz: f64,
    pub atoms: usize,
    pub geometry: BodyGeometry,
    pub drive: f64,
    pub velocity: f64,
    pub tongue_stiffness: f64,
    pub p_beat_pa: f64,
    pub brightness: f64,
    pub radiation: f64,
    pub coupling: f64,
    pub pair_topology: PairTopology,
    pub kerr_semitones: f64,
    pub triad_amount: f64,
    pub triad_axis: TriadAxis,
    pub spread: f64,
    pub c_air: f64,
    pub supply_area: f64,
}

impl Default for BrassReedConfig {
    fn default() -> Self {
        Self {
            fundamental_hz: 220.0,
            atoms: 12,
            geometry: BodyGeometry::Harmonic,
            drive: 0.6,
            velocity: 0.8,
            tongue_stiffness: 0.55,
            p_beat_pa: 1500.0,
            brightness: 0.5,
            radiation: 0.5,
            coupling: 0.0,
            pair_topology: PairTopology::Star,
            kerr_semitones: 0.0,
            triad_amount: 0.0,
            triad_axis: TriadAxis::Manley,
            spread: 0.35,
            c_air: 6.0e-6,
            supply_area: 3.6e-4,
        }
    }
}

/// Exact SI Joule work and dissipation ledger accumulated by [`BrassReed::step_with_ledger`].
///
/// Over any run from `t = 0` to `t`, exact joint conservation holds to floating-point precision:
/// `(E_total(t) - E_total(0)) - (work_supply - diss_supply - diss_bore - diss_valve - diss_pair - diss_bernoulli) == 0`.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReedEnergyLedger {
    pub work_supply: f64,
    pub diss_supply: f64,
    pub diss_bore: f64,
    pub diss_valve: f64,
    pub diss_pair: f64,
    pub diss_bernoulli: f64,
}

/// Single-voice 2-way symplectic mechanical brass reed coupled to a pneumatic wind plenum
/// and conical modal bore with `C^2` rolling-shallot closure, exact Bernoulli valve flows,
/// and lossless SO(3) Cayley-Rodrigues triads in an expanded time-symmetric Strang `O(h^2)`
/// schedule.
#[derive(Clone, Debug)]
pub struct BrassReed {
    sample_rate: f64,
    h_full: f64,
    h_half: f64,
    n: usize,
    gate: bool,
    pair_topology: PairTopology,
    passive: f64,
    output_gain: f64,

    c_air: f64,
    supply_area: f64,
    reed_pc: f64,
    reed_psup: f64,
    z_ref0: f64,
    alpha_sq: f64,

    zv_re: f64,
    zv_im: f64,
    v_rot_c: f64,
    v_rot_s: f64,
    gamma_v: f64,
    v_sqrt_k: f64,
    v_kick: f64,
    v_x0: f64,
    v_w: f64,

    x: [f64; MAX_ATOMS],
    y: [f64; MAX_ATOMS],
    omega: [f64; MAX_ATOMS],
    rot_c: [f64; MAX_ATOMS],
    rot_s: [f64; MAX_ATOMS],
    exp_h: [f64; MAX_ATOMS],
    t_eff_h: [f64; MAX_ATOMS],
    beta: [f64; MAX_ATOMS],
    g: [f64; MAX_ATOMS],
    lam: [f64; MAX_ATOMS],
    bore_patm: [f64; MAX_ATOMS],
    alpha: [f64; MAX_ATOMS],
    read: [f64; MAX_ATOMS],
    pan_c: [f64; MAX_ATOMS],
    pan_s: [f64; MAX_ATOMS],

    triads: TriadCache,
}

impl BrassReed {
    fn zeroed(sample_rate: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let h_full = 1.0 / sr;
        Self {
            sample_rate: sr,
            h_full,
            h_half: 0.5 * h_full,
            n: 0,
            gate: false,
            pair_topology: PairTopology::Star,
            passive: 0.0,
            output_gain: 0.32,
            c_air: 6.0e-6,
            supply_area: 3.6e-4,
            reed_pc: NOMINAL_CHEST_PA,
            reed_psup: NOMINAL_CHEST_PA,
            z_ref0: 1.0,
            alpha_sq: 0.0,
            zv_re: 0.0,
            zv_im: 0.0,
            v_rot_c: 1.0,
            v_rot_s: 0.0,
            gamma_v: 0.0,
            v_sqrt_k: 1.0,
            v_kick: 0.0,
            v_x0: 0.95e-3,
            v_w: 1.35e-2,
            x: [0.0; MAX_ATOMS],
            y: [0.0; MAX_ATOMS],
            omega: [0.0; MAX_ATOMS],
            rot_c: [1.0; MAX_ATOMS],
            rot_s: [0.0; MAX_ATOMS],
            exp_h: [1.0; MAX_ATOMS],
            t_eff_h: [0.0; MAX_ATOMS],
            beta: [0.0; MAX_ATOMS],
            g: [0.0; MAX_ATOMS],
            lam: [0.0; MAX_ATOMS],
            bore_patm: [1.0; MAX_ATOMS],
            alpha: [0.0; MAX_ATOMS],
            read: [0.0; MAX_ATOMS],
            pan_c: [1.0; MAX_ATOMS],
            pan_s: [0.0; MAX_ATOMS],
            triads: TriadCache::empty(),
        }
    }

    /// Construct a new [`BrassReed`] configured for `sample_rate`.
    pub fn new(sample_rate: f64, config: BrassReedConfig) -> Self {
        let mut reed = Self::zeroed(sample_rate);
        reed.configure(config);
        reed
    }

    /// Construct a heap-allocated [`BrassReed`] to keep debug stack frames compact.
    pub fn new_boxed(sample_rate: f64, config: BrassReedConfig) -> Box<Self> {
        let mut reed = Box::new(Self::zeroed(sample_rate));
        reed.configure(config);
        reed
    }

    /// Reconfigure bore acoustics, mechanical reed blade, pneumatic supply, and resonant triads
    /// without allocating.
    pub fn configure(&mut self, config: BrassReedConfig) {
        let f_base = sanitize_fundamental(config.fundamental_hz, self.sample_rate);
        let w_body = TAU * f_base;

        let req_n = config.atoms.clamp(1, MAX_ATOMS);
        let mut n = req_n;
        while n > 1 && f_base * body_ratio(config.geometry, n - 1, req_n) > 0.44 * self.sample_rate
        {
            n -= 1;
        }
        if n > self.n {
            self.x[self.n..n].fill(0.0);
            self.y[self.n..n].fill(0.0);
        } else if n < self.n {
            self.x[n..self.n].fill(0.0);
            self.y[n..self.n].fill(0.0);
        }
        self.n = n;
        self.pair_topology = config.pair_topology;

        let h = self.h_full;
        let h_half = self.h_half;
        let drive = finite_or(config.drive, 0.6);
        let velocity = finite_or(config.velocity, 0.8);
        let stiffness = finite_or(config.tongue_stiffness, 0.55);
        let p_beat = if config.p_beat_pa.is_finite() && config.p_beat_pa > 0.0 {
            config.p_beat_pa
        } else {
            1500.0
        };
        let brightness = finite_or(config.brightness, 0.5);
        let radiation = finite_or(config.radiation, 0.5).max(0.0);
        let coupling = finite_or(config.coupling, 0.0).max(0.0);
        let kerr_st = finite_or(config.kerr_semitones, 0.0);
        let spread = finite_or(config.spread, 0.0);

        let z0 = 9.2e5 * (220.0 / f_base.max(25.0)).powf(0.35);
        let p_sc_b = 460.0;
        let al0 = ((2.0 / PI) * w_body * z0).sqrt();
        let z_ref0 = p_sc_b / al0;
        self.z_ref0 = z_ref0.max(1e-9);
        let patm_b = 10.0 * z_ref0;
        let beta_base = 2.0_f64.powf(kerr_st / 12.0) - 1.0;
        let rad_fac = 0.35 + 1.3 * radiation;
        let denom_n = n.saturating_sub(1).max(1) as f64;

        let mut asq = 0.0;
        for k in 0..n {
            let ratio = body_ratio(config.geometry, k, req_n);
            let om = w_body * ratio;
            self.omega[k] = om;
            let (sh, ch) = (om * h_half).sin_cos();
            self.rot_c[k] = ch;
            self.rot_s[k] = sh;

            let mode_sc = 0.78_f64.powi(k as i32);
            let q_mode = (26.0 - 2.0 * k as f64).max(12.0);
            let al = mode_sc * al0;
            self.alpha[k] = al;
            asq += al * al;

            let z_ref = p_sc_b / al;
            self.beta[k] = beta_base / z_ref;
            let lam_tot = om / (2.0 * q_mode);
            let lam = 0.50 * lam_tot * rad_fac;
            self.lam[k] = lam;
            self.exp_h[k] = (-lam * h_half).exp();
            self.t_eff_h[k] = h_half * exprel_neg(lam * h_half);
            self.g[k] = (0.50 * lam_tot) / z_ref;
            self.bore_patm[k] = patm_b;

            let rw = (1.0 + 0.9 * brightness * (k as f64 / denom_n)) * al;
            self.read[k] = 0.00038 * rw;

            let sign = if k & 1 == 0 { 1.0 } else { -1.0 };
            let ang = spread * sign * (0.2 + 0.7 * ((k + 1) as f64 / n as f64));
            let (ps, pc) = (0.5 * ang).sin_cos();
            self.pan_c[k] = pc;
            self.pan_s[k] = ps;
        }
        self.alpha_sq = asq;

        let fv = f_base * (3.6 + 2.2 * stiffness).max(0.5);
        let om_v = TAU * fv;
        let s_v = 2.5e-5 * (220.0 / f_base.max(25.0)).powf(0.80);
        let x0 = 0.95e-3;
        let k_v = (s_v * p_beat) / x0;
        let m_v = k_v / (om_v * om_v);
        self.v_sqrt_k = k_v.sqrt();
        self.v_kick = s_v / m_v.sqrt();
        self.v_x0 = x0;
        self.v_w = 1.35e-2;
        self.gamma_v = (om_v / (2.0 * 3.1)) / (self.v_sqrt_k * x0);
        let (vs, vc) = (om_v * h_half).sin_cos();
        self.v_rot_c = vc;
        self.v_rot_s = vs;

        self.reed_psup =
            NOMINAL_CHEST_PA * (0.62 + 0.48 * drive).max(0.0) * (0.65 + 0.35 * velocity).max(0.0);
        self.c_air = if config.c_air.is_finite() && config.c_air > 0.0 {
            config.c_air
        } else {
            6.0e-6
        };
        self.supply_area = if config.supply_area.is_finite() && config.supply_area >= 0.0 {
            config.supply_area
        } else {
            3.6e-4
        };
        self.passive = 0.035 * w_body * coupling * h;
        self.output_gain = 0.32;

        let half_gh = triad_half_gh(config.triad_amount, w_body, n, h) / self.z_ref0;
        self.triads.ensure(
            config.geometry,
            req_n,
            n,
            &self.omega,
            half_gh,
            config.triad_axis,
        );
    }

    /// Reset bore, reed tongue, gate, and wind plenum to nominal idle state while preserving
    /// configuration.
    pub fn reset(&mut self) {
        self.x.fill(0.0);
        self.y.fill(0.0);
        self.zv_re = 0.0;
        self.zv_im = 0.0;
        self.reed_pc = NOMINAL_CHEST_PA;
        self.gate = false;
    }

    /// Open or close the reed valve gate.
    #[inline]
    pub fn set_gate(&mut self, gate: bool) {
        self.gate = gate;
    }

    /// Current valve gate state.
    #[inline]
    pub fn gate(&self) -> bool {
        self.gate
    }

    /// Set the instantaneous pneumatic plenum pressure in Pascals (non-finite inputs ignored).
    #[inline]
    pub fn set_plenum_pressure(&mut self, p_pa: f64) {
        if p_pa.is_finite() {
            self.reed_pc = p_pa;
        }
    }

    /// Instantaneous pneumatic plenum pressure in Pascals.
    #[inline]
    pub fn plenum_pressure(&self) -> f64 {
        self.reed_pc
    }

    /// Target bellows supply pressure in Pascals.
    #[inline]
    pub fn supply_pressure(&self) -> f64 {
        self.reed_psup
    }

    /// Number of active bore modes below the Nyquist safety ceiling.
    #[inline]
    pub fn active_modes(&self) -> usize {
        self.n
    }

    #[inline]
    fn joint_free_half_with_loss(&mut self, k: usize) -> f64 {
        let xr = self.x[k];
        let xi = self.y[k];
        let e0 = 0.5 * (xr * xr + xi * xi);
        let (nx, ny) = joint_free_mode(
            xr,
            xi,
            FreeStepCoeffs {
                omega: self.omega[k],
                beta: self.beta[k],
                g: self.g[k],
                lam: self.lam[k],
                patm: self.bore_patm[k],
                dt: self.h_half,
                exp_l: self.exp_h[k],
                t_eff: self.t_eff_h[k],
                rot_c: self.rot_c[k],
                rot_s: self.rot_s[k],
            },
        );
        self.x[k] = nx;
        self.y[k] = ny;
        e0 - 0.5 * (nx * nx + ny * ny)
    }

    /// Advance one audio sample without returning an external energy ledger.
    pub fn step(&mut self) -> [f64; 2] {
        let mut ledger = ReedEnergyLedger::default();
        self.step_with_ledger(&mut ledger)
    }

    /// Advance one audio sample and accumulate exact SI Joule work and dissipation into `ledger`.
    pub fn step_with_ledger(&mut self, ledger: &mut ReedEnergyLedger) -> [f64; 2] {
        let n = self.n;
        let dt = self.h_full;
        let h_half = self.h_half;
        let c_air = self.c_air;
        let couple = CoupleParams {
            n,
            topology: self.pair_topology,
            passive: self.passive,
            headroom: 0.0,
        };

        // 1. Supply(h/2)
        let (p_sup_1, w_sup_1, d_sup_1) = bernoulli_supply_exact(
            self.reed_pc,
            self.reed_psup,
            c_air,
            self.supply_area,
            h_half,
        );
        self.reed_pc = p_sup_1;
        ledger.work_supply += w_sup_1;
        ledger.diss_supply += d_sup_1;

        // 2. J(h/2)
        for k in 0..n {
            ledger.diss_bore += self.joint_free_half_with_loss(k);
        }

        // 3. Pair(h/2, fwd)
        if self.passive > 0.0 && n >= 2 {
            let e0 = self.bore_energy_joules();
            couple_pairs(&mut self.x, &mut self.y, couple, false, 0.5);
            ledger.diss_pair += e0 - self.bore_energy_joules();
        }

        // 4. Triad(h/2, fwd) — lossless SO(3) isometry
        self.triads.apply_pass(&self.x, &mut self.y, n, true, false);

        // 5. ValveFree(h/2): exact bare-cubic radial contraction + rotation
        let vr0 = self.zv_re;
        let vi0 = self.zv_im;
        let ev0 = 0.5 * (vr0 * vr0 + vi0 * vi0);
        let vinv = 1.0 / (1.0 + h_half * self.gamma_v * (vr0 * vr0 + vi0 * vi0).sqrt());
        let vr = vr0 * vinv;
        let vi = vi0 * vinv;
        let nvr = vr * self.v_rot_c - vi * self.v_rot_s;
        let mut nvi = vr * self.v_rot_s + vi * self.v_rot_c;
        ledger.diss_valve += ev0 - 0.5 * (nvr * nvr + nvi * nvi);

        // 6. Midpoint [Blade(h/2) -> BernoulliValve(h) -> Blade(h/2)] when gated
        if self.gate {
            let mut p_c = self.reed_pc;
            let mut pb = 0.0;
            for k in 0..n {
                pb += self.alpha[k] * self.x[k];
            }

            let asq = self.alpha_sq;
            let c_eq = 1.0 / (1.0 / c_air + asq);
            let sq_ceq = c_eq.sqrt();
            let th = (self.v_kick / sq_ceq) * h_half;
            let (sb, cb) = th.sin_cos();
            let sh = (0.5 * th).sin();
            let omc = 2.0 * sh * sh;

            // Blade(h/2) — exact orthogonal rotation conserving plenum + bore + valve energy
            let up = sq_ceq * (p_c - pb);
            let nvi_next = nvi * cb + up * sb;
            let dv_b = sq_ceq * (up * omc + nvi * sb);
            nvi = nvi_next;
            p_c -= dv_b / c_air;
            for k in 0..n {
                self.x[k] += self.alpha[k] * dv_b;
            }
            pb += asq * dv_b;

            // BernoulliValve(h) through C^2 rolling-shallot aperture
            let h_raw = self.v_x0 + nvr / self.v_sqrt_k;
            let h_open = rolling_shallot_aperture(h_raw, self.v_x0);
            if h_open > 0.0 {
                let area = self.v_w * h_open;
                let dp0 = p_c - pb;
                let dv = bernoulli_valve_exact(dp0, area, c_eq, dt);
                if dv != 0.0 {
                    p_c -= dv / c_air;
                    for k in 0..n {
                        self.x[k] += self.alpha[k] * dv;
                    }
                    pb += asq * dv;
                    let dp1 = p_c - pb;
                    ledger.diss_bernoulli += 0.5 * (dp0 + dp1) * dv;
                }
            }

            // Blade(h/2) — exact orthogonal rotation
            let up2 = sq_ceq * (p_c - pb);
            let nvi_next2 = nvi * cb + up2 * sb;
            let dv_b2 = sq_ceq * (up2 * omc + nvi * sb);
            nvi = nvi_next2;
            p_c -= dv_b2 / c_air;
            for k in 0..n {
                self.x[k] += self.alpha[k] * dv_b2;
            }
            self.reed_pc = p_c;
        }

        // 7. ValveFree(h/2): rotation + exact bare-cubic radial contraction
        let ev1 = 0.5 * (nvr * nvr + nvi * nvi);
        let rvr = nvr * self.v_rot_c - nvi * self.v_rot_s;
        let rvi = nvr * self.v_rot_s + nvi * self.v_rot_c;
        let vinv2 = 1.0 / (1.0 + h_half * self.gamma_v * (rvr * rvr + rvi * rvi).sqrt());
        self.zv_re = rvr * vinv2;
        self.zv_im = rvi * vinv2;
        ledger.diss_valve += ev1 - 0.5 * (self.zv_re * self.zv_re + self.zv_im * self.zv_im);

        // 8. Triad(h/2, rev) — lossless SO(3) isometry
        self.triads.apply_pass(&self.x, &mut self.y, n, true, true);

        // 9. Pair(h/2, rev)
        if self.passive > 0.0 && n >= 2 {
            let e0 = self.bore_energy_joules();
            couple_pairs(&mut self.x, &mut self.y, couple, true, 0.5);
            ledger.diss_pair += e0 - self.bore_energy_joules();
        }

        // 10. J(h/2)
        for k in (0..n).rev() {
            ledger.diss_bore += self.joint_free_half_with_loss(k);
        }

        // 11. Supply(h/2)
        let (p_sup_2, w_sup_2, d_sup_2) = bernoulli_supply_exact(
            self.reed_pc,
            self.reed_psup,
            c_air,
            self.supply_area,
            h_half,
        );
        self.reed_pc = p_sup_2;
        ledger.work_supply += w_sup_2;
        ledger.diss_supply += d_sup_2;

        self.read_stereo()
    }

    /// Zero-comb quadrature stereo readout `[left, right]`.
    #[inline]
    pub fn read_stereo(&self) -> [f64; 2] {
        let mut l = 0.0;
        let mut r = 0.0;
        let g_out = self.output_gain;
        for k in 0..self.n {
            let xk = self.x[k];
            let yk = self.y[k];
            let w = g_out * self.read[k];
            l += w * (self.pan_c[k] * xk - self.pan_s[k] * yk);
            r += w * (self.pan_c[k] * xk + self.pan_s[k] * yk);
        }
        [l, r]
    }

    /// Stored bore acoustic energy in Joules: `0.5 * sum_{k < n} (x[k]^2 + y[k]^2)`.
    #[inline]
    pub fn bore_energy_joules(&self) -> f64 {
        let mut e = 0.0;
        for k in 0..self.n {
            e += 0.5 * (self.x[k] * self.x[k] + self.y[k] * self.y[k]);
        }
        e
    }

    /// Stored mechanical reed tongue energy in Joules: `0.5 * (zv_re^2 + zv_im^2)`.
    #[inline]
    pub fn valve_energy_joules(&self) -> f64 {
        0.5 * (self.zv_re * self.zv_re + self.zv_im * self.zv_im)
    }

    /// Stored pneumatic plenum energy in Joules: `0.5 * c_air * reed_pc^2`.
    #[inline]
    pub fn plenum_energy_joules(&self) -> f64 {
        0.5 * self.c_air * self.reed_pc * self.reed_pc
    }

    /// Total stored SI energy in Joules across plenum, bore, and reed tongue.
    #[inline]
    pub fn total_energy_joules(&self) -> f64 {
        self.plenum_energy_joules() + self.bore_energy_joules() + self.valve_energy_joules()
    }

    /// Dimensionless bore energy normalized to the nominal reference bore energy `0.5 * z_ref0^2`.
    #[inline]
    pub fn bore_audibility(&self) -> f64 {
        let e_ref = 0.5 * self.z_ref0 * self.z_ref0;
        self.bore_energy_joules() / e_ref.max(1e-30)
    }

    /// True when the reed valve gate is closed and pitch-normalized bore energy
    /// ([`Self::bore_audibility`]) has decayed below `1e-14`.
    ///
    /// While `!self.gate`, the pneumatic plenum continues recharging toward supply pressure
    /// and the mechanical reed blade is decoupled from both the bore modes and stereo readout,
    /// so voice-tail silence is governed strictly by bore acoustic energy.
    #[inline]
    pub fn is_quiet(&self) -> bool {
        !self.gate && self.bore_audibility() < AERO_QUIET_FLOOR
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flue_jet_and_brass_reed_bore_tail_quiet_lifecycle() {
        let sr = 48_000.0;

        // FlueJet: quiet when idle, active while gated, rings after release, then retires cleanly.
        let mut flue = FlueJet::new_boxed(
            sr,
            FlueJetConfig {
                fundamental_hz: 440.0,
                atoms: 8,
                drive: 0.75,
                velocity: 0.9,
                radiation: 0.25,
                triad_amount: 0.3,
                ..FlueJetConfig::default()
            },
        );
        assert!(!flue.gate());
        assert!(flue.is_quiet());

        flue.set_gate(true);
        assert!(flue.gate());
        assert!(!flue.is_quiet());
        for _ in 0..8_000 {
            flue.step();
        }
        assert!(flue.energy() > 1e-4);

        flue.set_gate(false);
        assert!(!flue.gate());
        assert!(
            !flue.is_quiet(),
            "FlueJet must preserve audible bore tail right after gate-off"
        );

        let mut flue_quiet_step = None;
        for step in 0..(sr as usize * 5) {
            flue.step();
            if flue.is_quiet() {
                flue_quiet_step = Some(step);
                break;
            }
        }
        assert!(
            flue_quiet_step.is_some(),
            "FlueJet bore tail failed to reach is_quiet() within 5s (energy={:.3e})",
            flue.energy()
        );
        let [fl, fr] = flue.read_stereo();
        assert!(fl.abs().max(fr.abs()) < 1e-6);

        flue.set_gate(true);
        flue.step();
        flue.reset();
        assert!(!flue.gate());
        assert!(flue.is_quiet());

        // BrassReed: quiet when idle/reset despite pressurized plenum, rings after release, retires cleanly.
        let mut reed = BrassReed::new_boxed(
            sr,
            BrassReedConfig {
                fundamental_hz: 220.0,
                atoms: 10,
                drive: 0.8,
                velocity: 0.95,
                radiation: 0.5,
                triad_amount: 0.3,
                ..BrassReedConfig::default()
            },
        );
        assert!(!reed.gate());
        assert!(reed.plenum_energy_joules() > 1.0);
        assert!(reed.is_quiet());

        reed.set_gate(true);
        assert!(reed.gate());
        assert!(!reed.is_quiet());
        for _ in 0..6_000 {
            reed.step();
        }
        assert!(reed.bore_audibility() > 1e-3);

        reed.set_gate(false);
        assert!(!reed.gate());
        assert!(
            !reed.is_quiet(),
            "BrassReed must preserve audible bore tail right after gate-off"
        );

        let mut reed_quiet_step = None;
        for step in 0..(sr as usize * 4) {
            reed.step();
            if reed.is_quiet() {
                reed_quiet_step = Some(step);
                break;
            }
        }
        assert!(
            reed_quiet_step.is_some(),
            "BrassReed bore tail failed to reach is_quiet() within 4s (audibility={:.3e})",
            reed.bore_audibility()
        );
        let [rl, rr] = reed.read_stereo();
        assert!(rl.abs().max(rr.abs()) < 1e-6);

        reed.set_gate(true);
        reed.step();
        reed.reset();
        assert!(!reed.gate());
        assert!(reed.is_quiet());
    }
}
