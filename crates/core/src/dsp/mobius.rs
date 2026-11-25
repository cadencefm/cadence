//! Pure, zero-allocation Complex State-Kerr Cubic-Atom Synthesizer ([`MobiusSynth`]),
//! closed-form space-time atom primitives, and analytical descriptor SPOTs shared by
//! `cadence-engine` real-time synthesis and `cadence-app` GPU visualizers.
//!
//! All internal atom integration runs in `f64` from four exact closed-form primitives:
//! 1. Pure unlifted State-Kerr [`CubicAtom`] (`dz/dt = i*w0*(1 + beta*|z|)*z - g*|z - v|*(z - v)`)
//!    integrated via exact 3-stage symmetric Strang splitting (`O(dt^2)`, zero linear damping,
//!    zero internal clamps, zero RK2).
//! 2. Complex unit-disk target sources (`Szego`, singularity-free `Dipole`, `Poisson`,
//!    `Strike`, `External`) with exact `-80 dB` closed-form Nyquist caps and continuous
//!    chirality axis.
//! 3. Strictly convex linear-interpolated complex propagation delays ([`MobiusDelayBank`])
//!    preserving unconditional non-expansiveness for any delay `tau >= 0`.
//! 4. Exact zero-comb [`stereo_quad`] quadrature stereo projection (`0.00 dB` mono comb error).

use crate::dsp::math::{
    decay_coeff_60db_ms, flush_denormal_f64, mix_stereo_f64, sanitize_clamp_f64,
    sanitize_sample_f64, sanitize_sample_rate, NYQUIST_SAFE_RATIO, TAIL_SILENCE_FLOOR_F64,
};
use crate::rack::{MobiusDispersion, MobiusExciter, MobiusTopology};
use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::ops::{Add, Div, Mul, Neg, Sub};

/// Maximum active complex cubic atoms per [`MobiusSynth`] voice.
pub const MOBIUS_MAX_ATOMS: usize = 8;

/// Minimum base frequency (Hz).
pub const MOBIUS_MIN_HZ: f32 = 20.0;
/// Maximum base frequency (Hz).
pub const MOBIUS_MAX_HZ: f32 = 12_000.0;
/// Reference fundamental frequency (`C4`, MIDI 60) anchoring pitch-tracked stiffness scaling.
pub const MOBIUS_REF_HZ: f64 = 261.625_565_300_598_6;
/// Audible upper atom frequency ceiling (Hz), consistent across 44.1..192 kHz sample rates.
pub const MOBIUS_AUDIBLE_MAX_HZ: f64 = 20_065.5;

/// Minimum dimensionless stiffness ratio `xi = g / omega0`.
pub const MOBIUS_MIN_STIFFNESS: f32 = 0.005;
/// Maximum dimensionless stiffness ratio `xi = g / omega0`.
pub const MOBIUS_MAX_STIFFNESS: f32 = 10.0;
/// Minimum stiffness tilt across atoms (octaves; negative = Stiff->Loose halo).
pub const MOBIUS_MIN_STIFFNESS_TILT: f32 = -5.0;
/// Maximum stiffness tilt across atoms (octaves; positive = Loose->Stiff transient).
pub const MOBIUS_MAX_STIFFNESS_TILT: f32 = 5.0;
/// Minimum State-Kerr pitch bend at unit amplitude `|z| = 1` (semitones).
pub const MOBIUS_MIN_KERR_ST: f32 = -12.0;
/// Maximum State-Kerr pitch bend at unit amplitude `|z| = 1` (semitones).
pub const MOBIUS_MAX_KERR_ST: f32 = 12.0;
/// Maximum inter-atom wave propagation delay (ms).
pub const MOBIUS_MAX_DISTANCE_MS: f32 = 250.0;
/// Maximum pitch-locked bore reflection delay (fundamental cycles).
pub const MOBIUS_MAX_FB_CYCLES: f32 = 4.0;
/// Maximum fixed acoustic bore reflection delay (ms).
pub const MOBIUS_MAX_FB_MS: f32 = 25.0;

/// Nyquist alias guard epsilon (`-80 dB = 1e-4`).
const ALIAS_EPS: f64 = 1e-4;
/// Upper pole radius bound for `Dipole` double-pole conditioning.
const MAX_DIPOLE_R: f64 = 0.93;
/// Upper pole radius bound for `Szego` and `Poisson` disk kernels.
const MAX_SINGLE_POLE_R: f64 = 0.96;
/// Exciter gate-off de-click ramp duration (seconds); lets pure `mu = 0` atoms ring freely.
const EXCITER_RELEASE_SEC: f64 = 0.004;
/// Silence floor (`-72 dB`) for host voice-retirement detection once exciter is idle.
const VOICE_QUIET_FLOOR: f64 = 2.5e-4;
/// Delay pool duration budget (seconds of total `C32` atom-delay storage per slab).
const ATOM_DELAY_POOL_SEC: f64 = 0.67;
/// Bore delay duration budget (seconds of `C32` bore-reflection storage per slab).
const BORE_DELAY_POOL_SEC: f64 = 0.060;

/// Coprime inter-atom delay ratios preventing metallic comb flutter when `N > 2`.
const COPRIME_RATIOS: [f64; MOBIUS_MAX_ATOMS] = [1.0, 1.58, 2.57, 3.11, 3.89, 4.63, 5.21, 5.93];

/// Canonical 5-vowel vocal tract formants (Hz) for `MobiusDispersion::Formant` (`F1..F7`).
const VOWEL_FORMANTS_HZ: [[f64; MOBIUS_MAX_ATOMS - 1]; 5] = [
    // /oo/
    [300.0, 870.0, 2240.0, 3200.0, 4100.0, 5200.0, 6500.0],
    // /oh/
    [500.0, 900.0, 2400.0, 3300.0, 4200.0, 5400.0, 6800.0],
    // /ah/
    [730.0, 1090.0, 2440.0, 3400.0, 4400.0, 5600.0, 7000.0],
    // /eh/
    [530.0, 1840.0, 2480.0, 3500.0, 4500.0, 5800.0, 7200.0],
    // /ee/
    [270.0, 2290.0, 3010.0, 3800.0, 4800.0, 6000.0, 7500.0],
];

/// Double-precision complex scalar used for exact phase and conservation precision.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct C64 {
    pub re: f64,
    pub im: f64,
}

impl C64 {
    pub const ZERO: Self = Self { re: 0.0, im: 0.0 };
    pub const ONE: Self = Self { re: 1.0, im: 0.0 };

    #[inline]
    pub const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    #[inline]
    pub fn from_polar(mag: f64, angle: f64) -> Self {
        let (s, c) = angle.sin_cos();
        Self {
            re: mag * c,
            im: mag * s,
        }
    }

    #[inline]
    pub fn scale(self, s: f64) -> Self {
        Self {
            re: self.re * s,
            im: self.im * s,
        }
    }

    #[inline]
    pub fn conj(self) -> Self {
        Self {
            re: self.re,
            im: -self.im,
        }
    }

    #[inline]
    pub fn abs_sq(self) -> f64 {
        self.re * self.re + self.im * self.im
    }

    #[inline]
    pub fn abs(self) -> f64 {
        self.abs_sq().sqrt()
    }

    #[inline]
    pub fn rotate(self, cos_a: f64, sin_a: f64) -> Self {
        Self {
            re: self.re * cos_a - self.im * sin_a,
            im: self.re * sin_a + self.im * cos_a,
        }
    }

    #[inline]
    pub fn flush_denormal(self) -> Self {
        Self {
            re: flush_denormal_f64(self.re),
            im: flush_denormal_f64(self.im),
        }
    }

    #[inline]
    pub fn is_finite(self) -> bool {
        self.re.is_finite() && self.im.is_finite()
    }
}

impl Add for C64 {
    type Output = Self;
    #[inline]
    fn add(self, o: Self) -> Self {
        Self {
            re: self.re + o.re,
            im: self.im + o.im,
        }
    }
}

impl Sub for C64 {
    type Output = Self;
    #[inline]
    fn sub(self, o: Self) -> Self {
        Self {
            re: self.re - o.re,
            im: self.im - o.im,
        }
    }
}

impl Neg for C64 {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self {
            re: -self.re,
            im: -self.im,
        }
    }
}

impl Mul for C64 {
    type Output = Self;
    #[inline]
    fn mul(self, o: Self) -> Self {
        Self {
            re: self.re * o.re - self.im * o.im,
            im: self.re * o.im + self.im * o.re,
        }
    }
}

impl Div for C64 {
    type Output = Self;
    #[inline]
    fn div(self, o: Self) -> Self {
        let d = (o.re * o.re + o.im * o.im).max(1e-30);
        let inv = 1.0 / d;
        Self {
            re: (self.re * o.re + self.im * o.im) * inv,
            im: (self.im * o.re - self.re * o.im) * inv,
        }
    }
}

/// Compact single-precision complex storage for convex propagation delay lines.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct C32 {
    pub re: f32,
    pub im: f32,
}

impl C32 {
    pub const ZERO: Self = Self { re: 0.0, im: 0.0 };

    #[inline]
    pub fn from_c64(z: C64) -> Self {
        Self {
            re: z.re as f32,
            im: z.im as f32,
        }
    }

    #[inline]
    pub fn to_c64(self) -> C64 {
        C64 {
            re: f64::from(self.re),
            im: f64::from(self.im),
        }
    }
}

// =============================================================================
// PRIMITIVE 1: PURE UNLIFTED STATE-KERR CUBIC ATOM
// =============================================================================

/// Map dimensionless stiffness `xi = g / omega0`, natural frequency `freq_hz`,
/// and keyboard tracking exponent `key_track in [0, 1]` to physical cubic rate `g`.
///
/// At `key_track = 1.0`, `g = xi * 2*pi*freq_hz` is strictly pitch-invariant across octaves.
#[inline]
pub fn stiffness_to_g(xi: f64, freq_hz: f64, key_track: f64) -> f64 {
    let f = sanitize_clamp_f64(freq_hz, f64::from(MOBIUS_MIN_HZ), 24_000.0, MOBIUS_REF_HZ);
    let x = sanitize_clamp_f64(
        xi,
        f64::from(MOBIUS_MIN_STIFFNESS),
        f64::from(MOBIUS_MAX_STIFFNESS),
        0.2,
    );
    let p = sanitize_clamp_f64(key_track, 0.0, 1.0, 1.0);
    let omega_ref = TAU * MOBIUS_REF_HZ;
    x * omega_ref * (f / MOBIUS_REF_HZ).powf(p)
}

/// Map State-Kerr pitch bend `semitones` at unit amplitude `|z| = 1` to dimensionless `beta`.
#[inline]
pub fn semitones_to_beta(semitones: f64) -> f64 {
    let st = sanitize_clamp_f64(
        semitones,
        f64::from(MOBIUS_MIN_KERR_ST),
        f64::from(MOBIUS_MAX_KERR_ST),
        0.0,
    );
    2.0_f64.powf(st / 12.0) - 1.0
}

/// Physical parameters of one [`CubicAtom`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AtomParams {
    /// Natural angular frequency `omega0 = 2*pi*f0` (rad/s).
    pub omega: f64,
    /// Cubic potential coupling rate `g > 0` (`1 / (s * amplitude)`).
    pub g: f64,
    /// Dimensionless State-Kerr frequency coefficient `beta = 2^(st/12) - 1`.
    pub beta: f64,
}

impl AtomParams {
    #[inline]
    pub fn from_invariants(freq_hz: f64, xi: f64, kerr_st: f64, key_track: f64) -> Self {
        let f = sanitize_clamp_f64(freq_hz, f64::from(MOBIUS_MIN_HZ), 24_000.0, MOBIUS_REF_HZ);
        Self {
            omega: TAU * f,
            g: stiffness_to_g(xi, f, key_track),
            beta: semitones_to_beta(kerr_st),
        }
    }
}

/// Single pure unlifted State-Kerr complex cubic oscillator (`z in C`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CubicAtom {
    pub z: C64,
}

impl CubicAtom {
    /// Exact State-Kerr unitary rotation sub-flow (`|z|` strictly invariant):
    /// `R_h(z) = exp(i * omega * h * (1 + beta * |z|)) * z`.
    #[inline]
    pub fn rot_kerr(&mut self, p: AtomParams, h: f64) {
        let dth = p.omega * h * (1.0 + p.beta * self.z.abs());
        let (s, c) = dth.sin_cos();
        self.z = self.z.rotate(c, s);
    }

    /// Exact closed-form parabolic cubic potential sub-flow (`P_{h/2} o P_{h/2} == P_h`):
    /// returns `(w_abs, flux)` where `w_abs = (z - v) / (1 + u)` and
    /// `flux = w_res / (g * h) = |w| * w / (1 + u)` is the exact step-averaged cubic flux
    /// with zero `0/0` singularity as `g -> 0`.
    #[inline]
    pub fn cubic_pot(&mut self, v: C64, g: f64, h: f64) -> (C64, C64) {
        let w = self.z.sub(v);
        let w_mag = w.abs();
        let u = g * h * w_mag;
        let inv = 1.0 / (1.0 + u);
        let w_abs = w.scale(inv);
        let flux = w.scale(w_mag * inv);
        self.z = v.add(w_abs);
        (w_abs, flux)
    }

    /// Full symmetric 3-stage Strang step (`O(dt^2)`) returning `(w_abs, flux)`.
    #[inline]
    pub fn step_single(&mut self, p: AtomParams, v_mid: C64, dt: f64) -> (C64, C64) {
        self.rot_kerr(p, 0.5 * dt);
        let out = self.cubic_pot(v_mid, p.g, dt);
        self.rot_kerr(p, 0.5 * dt);
        out
    }
}

// =============================================================================
// PRIMITIVE 2: COMPLEX UNIT-DISK TARGET SOURCES & NYQUIST CAPS
// =============================================================================

/// Exact `-80 dB` Nyquist pole-radius cap for the `Szego` monopole kernel.
#[inline]
pub fn r_max_szego(f_v: f64, sample_rate: f64) -> f64 {
    let sr = sanitize_sample_rate(sample_rate);
    let fv = f_v.max(1.0);
    let n_nyq = (0.5 * sr / fv).floor().max(2.0);
    ALIAS_EPS.powf(1.0 / n_nyq).min(MAX_SINGLE_POLE_R)
}

/// Exact `-80 dB` Nyquist pole-radius cap for the `Dipole` double-pole kernel.
#[inline]
pub fn r_max_dipole(f_v: f64, sample_rate: f64) -> f64 {
    let sr = sanitize_sample_rate(sample_rate);
    let fv = f_v.max(1.0);
    let n_nyq = (0.5 * sr / fv).floor().max(2.0);
    (ALIAS_EPS / n_nyq)
        .powf(1.0 / (n_nyq - 1.0))
        .min(MAX_DIPOLE_R)
}

/// Exact `-80 dB` Nyquist pole-radius cap for the `Poisson` formant/sync kernel
/// (includes the `2 * r^m` two-sided cosine sideband factor).
#[inline]
pub fn r_max_poisson(f_v: f64, f_f: f64, sample_rate: f64) -> f64 {
    let sr = sanitize_sample_rate(sample_rate);
    let fv = f_v.max(1.0);
    let m_nyq = ((0.5 * sr - f_f).max(0.0) / fv).floor();
    if m_nyq < 1.0 {
        0.0
    } else {
        (0.5 * ALIAS_EPS).powf(1.0 / m_nyq).min(MAX_SINGLE_POLE_R)
    }
}

/// Unit-RMS, zero-DC Szego monopole kernel `sqrt(1 - r^2) * u / (1 - r * u)`.
#[inline]
pub fn eval_szego(theta: f64, r: f64) -> C64 {
    let rc = r.clamp(0.0, MAX_SINGLE_POLE_R);
    let u = C64::from_polar(1.0, theta);
    let norm = (1.0 - rc * rc).max(0.0).sqrt();
    u.scale(norm).div(C64::ONE.sub(u.scale(rc)))
}

/// Singularity-free, unit-RMS, zero-DC Möbius dipole kernel across `psi in [0, pi]`:
/// `psi = pi` yields exact odd-only clarinet (`H2, H4 = -inf`), `psi = 2*pi*d` yields
/// alias-free PWM, and `psi = 0` yields a finite double-pole brass formant.
#[inline]
pub fn eval_dipole(theta: f64, r: f64, psi: f64) -> C64 {
    let rc = r.clamp(0.0, MAX_DIPOLE_R);
    let u = C64::from_polar(1.0, theta);
    let e_psi = C64::from_polar(1.0, psi);
    let r2 = rc * rc;
    let norm =
        ((1.0 - r2).max(0.0) * (1.0 - 2.0 * r2 * e_psi.re + r2 * r2).max(0.0) / (1.0 + r2)).sqrt();
    // `-i * e^{i * psi / 2} = sin(psi/2) - i*cos(psi/2)`
    let (sh, ch) = (0.5 * psi).sin_cos();
    let num = u.mul(C64::new(sh, -ch)).scale(norm);
    let den = C64::ONE
        .sub(u.scale(rc))
        .mul(C64::ONE.sub(u.mul(e_psi).scale(rc)));
    num.div(den)
}

/// Unit-RMS smooth Poisson formant/sync kernel at fundamental phase `theta_v` and
/// carrier formant phase `theta_f`.
#[inline]
pub fn eval_poisson(theta_v: f64, theta_f: f64, r: f64) -> C64 {
    let rc = r.clamp(0.0, MAX_SINGLE_POLE_R);
    let r2 = rc * rc;
    let norm = ((1.0 - r2).max(0.0) / (1.0 + r2)).sqrt();
    let env = norm * (1.0 - r2) / (1.0 - 2.0 * rc * theta_v.cos() + r2).max(1e-12);
    C64::from_polar(env, theta_f)
}

/// Universal unit-RMS chirality tilt (`0.0` = analytic complex lock, `1.0` = real-axis bite).
#[inline]
pub fn apply_chirality(v: C64, chirality: f64) -> C64 {
    let chi = chirality.clamp(0.0, 1.0);
    if chi <= 0.0 {
        return v;
    }
    let inv = 1.0 / (1.0 + chi * chi).sqrt();
    v.add(v.conj().scale(chi)).scale(inv)
}

/// Map `brightness in [0, 1]` to physical mallet contact duration (`16.0 ms -> 0.4 ms`).
#[inline]
pub fn strike_duration_s(brightness: f64) -> f64 {
    let b = sanitize_clamp_f64(brightness, 0.0, 1.0, 0.72);
    0.016 * (0.025_f64).powf(b)
}

// =============================================================================
// PRIMITIVE 4: ZERO-COMB QUADRATURE STEREO PROJECTOR
// =============================================================================

/// Project complex atom state `z` onto stereo `[L, R]` via symmetric quadrature angle
/// `theta_rad in [-pi/2, +pi/2]`, guaranteeing exact `0.00 dB` mono-sum comb flatness.
#[inline]
pub fn stereo_quad(z: C64, theta_rad: f64) -> [f64; 2] {
    let (s, c) = (0.5 * theta_rad).sin_cos();
    [c * z.re - s * z.im, c * z.re + s * z.im]
}

// =============================================================================
// PRIMITIVE 3: CONVEX COMPLEX PROPAGATION DELAY BANK (ZERO RT ALLOC)
// =============================================================================

/// Pre-allocated contiguous complex delay slab shared by one [`MobiusSynth`]'s
/// `N <= 8` inter-atom propagation lines and single bore-reflection delay line.
///
/// Uses strictly convex linear fractional interpolation (`|z_del| <= max |z|`),
/// preserving unconditional non-expansiveness across arbitrary delays and topologies.
#[derive(Clone, Debug)]
pub struct MobiusDelayBank {
    buf: Box<[C32]>,
    atom_pool_len: usize,
    bore_offset: usize,
    bore_len: usize,
    atom_offsets: [usize; MOBIUS_MAX_ATOMS],
    atom_caps: [usize; MOBIUS_MAX_ATOMS],
    atom_wr: [usize; MOBIUS_MAX_ATOMS],
    bore_wr: usize,
    partitioned_n: usize,
    sample_rate: f64,
    ring_countdown: usize,
}

impl MobiusDelayBank {
    /// Allocate one complete complex delay slab for `sample_rate` (off-RT / pool init).
    pub fn new(sample_rate: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate).clamp(8_000.0, 384_000.0);
        let atom_pool_len = ((ATOM_DELAY_POOL_SEC * sr).ceil() as usize).max(64);
        let bore_len = ((BORE_DELAY_POOL_SEC * sr).ceil() as usize).max(64);
        let total = atom_pool_len + bore_len;
        let mut bank = Self {
            buf: vec![C32::ZERO; total].into_boxed_slice(),
            atom_pool_len,
            bore_offset: atom_pool_len,
            bore_len,
            atom_offsets: [0; MOBIUS_MAX_ATOMS],
            atom_caps: [0; MOBIUS_MAX_ATOMS],
            atom_wr: [0; MOBIUS_MAX_ATOMS],
            bore_wr: 0,
            partitioned_n: 0,
            sample_rate: sr,
            ring_countdown: 0,
        };
        bank.partition(2);
        bank
    }

    #[inline]
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Zero all delay lines in place without heap allocation.
    #[inline]
    pub fn clear(&mut self) {
        self.buf.fill(C32::ZERO);
        self.atom_wr = [0; MOBIUS_MAX_ATOMS];
        self.bore_wr = 0;
        self.ring_countdown = 0;
    }

    /// Partition `buf[..atom_pool_len]` proportionally across `n in 1..=MOBIUS_MAX_ATOMS`
    /// atoms according to [`COPRIME_RATIOS`] with zero heap allocation.
    #[inline]
    pub fn partition(&mut self, n: usize) {
        let count = n.clamp(1, MOBIUS_MAX_ATOMS);
        if self.partitioned_n == count {
            return;
        }
        let mut ratio_sum = 0.0_f64;
        for &r in &COPRIME_RATIOS[..count] {
            ratio_sum += r;
        }
        let mut cursor = 0usize;
        for (k, &ratio) in COPRIME_RATIOS.iter().enumerate() {
            if k < count {
                let remaining = self.atom_pool_len.saturating_sub(cursor);
                let cap = if k + 1 == count {
                    remaining.max(4)
                } else {
                    let min_tail = 4 * (count - 1 - k);
                    let share =
                        ((self.atom_pool_len as f64) * (ratio / ratio_sum)).floor() as usize;
                    share.clamp(4, remaining.saturating_sub(min_tail).max(4))
                };
                self.atom_offsets[k] = cursor;
                self.atom_caps[k] = cap;
                self.atom_wr[k] = 0;
                cursor = (cursor + cap).min(self.atom_pool_len);
            } else {
                self.atom_offsets[k] = 0;
                self.atom_caps[k] = 0;
                self.atom_wr[k] = 0;
            }
        }
        self.buf[..self.atom_pool_len].fill(C32::ZERO);
        self.partitioned_n = count;
        self.ring_countdown = 0;
    }

    /// Push one complex sample into atom `k`'s ring buffer.
    #[inline]
    pub fn push_atom(&mut self, k: usize, z: C64) {
        if k >= self.partitioned_n {
            return;
        }
        let cap = self.atom_caps[k];
        if cap == 0 {
            return;
        }
        let off = self.atom_offsets[k];
        let wr = self.atom_wr[k];
        self.buf[off + wr] = C32::from_c64(z);
        let next = wr + 1;
        self.atom_wr[k] = if next >= cap { 0 } else { next };
    }

    /// Convex linear-interpolated fractional read `d_samp >= 1.0` from atom `k`.
    #[inline]
    pub fn read_atom(&self, k: usize, d_samp: f64) -> C64 {
        if k >= self.partitioned_n {
            return C64::ZERO;
        }
        let cap = self.atom_caps[k];
        if cap < 3 {
            return C64::ZERO;
        }
        let off = self.atom_offsets[k];
        let wr = self.atom_wr[k];
        let d = d_samp.clamp(1.0, (cap - 2) as f64);
        let i0 = d.floor() as usize;
        let frac = d - i0 as f64;
        let idx0 = (wr + cap - i0) % cap;
        let idx1 = (wr + cap - i0 - 1) % cap;
        let s0 = self.buf[off + idx0].to_c64();
        let s1 = self.buf[off + idx1].to_c64();
        s0.scale(1.0 - frac).add(s1.scale(frac))
    }

    /// Push one complex pressure sample into the bore reflection delay line.
    #[inline]
    pub fn push_bore(&mut self, p: C64) {
        let cap = self.bore_len;
        if cap == 0 {
            return;
        }
        let wr = self.bore_wr;
        self.buf[self.bore_offset + wr] = C32::from_c64(p);
        let next = wr + 1;
        self.bore_wr = if next >= cap { 0 } else { next };
    }

    /// Convex linear-interpolated fractional read `d_samp >= 1.0` from the bore delay line.
    #[inline]
    pub fn read_bore(&self, d_samp: f64) -> C64 {
        let cap = self.bore_len;
        if cap < 3 {
            return C64::ZERO;
        }
        let wr = self.bore_wr;
        let d = d_samp.clamp(1.0, (cap - 2) as f64);
        let i0 = d.floor() as usize;
        let frac = d - i0 as f64;
        let idx0 = (wr + cap - i0) % cap;
        let idx1 = (wr + cap - i0 - 1) % cap;
        let s0 = self.buf[self.bore_offset + idx0].to_c64();
        let s1 = self.buf[self.bore_offset + idx1].to_c64();
        s0.scale(1.0 - frac).add(s1.scale(frac))
    }

    /// Update exact O(1) delay-tail drain countdown without scanning buffer memory.
    #[inline]
    fn tick_activity(&mut self, active_energy: bool, max_delay_samples: usize) {
        if active_energy && max_delay_samples > 0 {
            let capped = max_delay_samples.min(self.atom_pool_len);
            if capped > self.ring_countdown {
                self.ring_countdown = capped;
            }
        } else {
            self.ring_countdown = self.ring_countdown.saturating_sub(1);
        }
    }

    #[inline]
    pub fn is_drained(&self) -> bool {
        self.ring_countdown == 0
    }
}

// =============================================================================
// REPEATABLE N-ATOM SPACE-TIME STEPPERS (EXACT O(dt^2) & UNCONDITIONAL PASSIVITY)
// =============================================================================

/// Exact palindromic `-w_abs` conservation cascade (`tau = 0`).
///
/// At `coupling = 1.0`, `sum(z_mid[..n]) + v_rem == v_in` holds to machine precision (`~1e-15`)
/// and integration converges at exact `O(dt^2)` (`4.00x`).
#[inline]
pub fn step_cascade(
    atoms: &mut [CubicAtom; MOBIUS_MAX_ATOMS],
    params: &[AtomParams; MOBIUS_MAX_ATOMS],
    n: usize,
    v_in: C64,
    coupling: f64,
    dt: f64,
) -> ([C64; MOBIUS_MAX_ATOMS], C64) {
    let count = n.clamp(1, MOBIUS_MAX_ATOMS);
    let c = coupling.clamp(0.0, 1.0);
    let mut z_mid = [C64::ZERO; MOBIUS_MAX_ATOMS];

    if count == 1 {
        atoms[0].rot_kerr(params[0], 0.5 * dt);
        let (w_abs, _) = atoms[0].cubic_pot(v_in, params[0].g, dt);
        z_mid[0] = atoms[0].z;
        atoms[0].rot_kerr(params[0], 0.5 * dt);
        return (z_mid, w_abs.neg());
    }

    for k in 0..count {
        atoms[k].rot_kerr(params[k], 0.5 * dt);
    }

    let mut v_saved = [C64::ZERO; MOBIUS_MAX_ATOMS];
    let mut v_cur = v_in;
    for k in 0..(count - 1) {
        v_saved[k] = v_cur;
        let (w_abs_mid, _) = atoms[k].cubic_pot(v_cur, params[k].g, 0.5 * dt);
        z_mid[k] = atoms[k].z;
        v_cur = w_abs_mid.neg().scale(c);
    }

    let last = count - 1;
    let (w_abs_last, _) = atoms[last].cubic_pot(v_cur, params[last].g, dt);
    z_mid[last] = atoms[last].z;
    let v_rem = w_abs_last.neg();

    for k in (0..last).rev() {
        let _ = atoms[k].cubic_pot(v_saved[k], params[k].g, 0.5 * dt);
    }

    for k in 0..count {
        atoms[k].rot_kerr(params[k], 0.5 * dt);
    }
    (z_mid, v_rem)
}

/// Exact `O(dt^2)` step-averaged cubic flux bloom chain (`tau = 0`):
/// `v_0 = v_in`, `v_k = eta * flux_{k-1}`.
#[inline]
pub fn step_bloom(
    atoms: &mut [CubicAtom; MOBIUS_MAX_ATOMS],
    params: &[AtomParams; MOBIUS_MAX_ATOMS],
    n: usize,
    v_in: C64,
    eta: f64,
    dt: f64,
) {
    let count = n.clamp(1, MOBIUS_MAX_ATOMS);
    let gain = eta.max(0.0);
    for k in 0..count {
        atoms[k].rot_kerr(params[k], 0.5 * dt);
    }
    let mut v_cur = v_in;
    for k in 0..count {
        let (_, flux) = atoms[k].cubic_pot(v_cur, params[k].g, dt);
        if k + 1 < count {
            v_cur = flux.scale(gain);
        }
    }
    for k in 0..count {
        atoms[k].rot_kerr(params[k], 0.5 * dt);
    }
}

/// Exact `O(dt^2)` weighted-ball Star-Chamber sandwich (`tau = 0`):
/// `v_0 = (1 - c^2) * v_in + (c / M) * sum_{j=1..N-1} z_j`, `v_j = c * z_0`.
/// Preserves the exact invariant ball `{|z_0| <= |v_in|, |z_j| <= c * |v_in|}` with zero attack droop.
#[inline]
pub fn step_star(
    atoms: &mut [CubicAtom; MOBIUS_MAX_ATOMS],
    params: &[AtomParams; MOBIUS_MAX_ATOMS],
    n: usize,
    v_in: C64,
    c: f64,
    dt: f64,
) {
    let count = n.clamp(1, MOBIUS_MAX_ATOMS);
    if count == 1 {
        atoms[0].step_single(params[0], v_in, dt);
        return;
    }
    let cc = c.clamp(0.0, 1.0);
    let m_inv = 1.0 / (count - 1) as f64;
    let in_scale = 1.0 - cc * cc;

    for k in 0..count {
        atoms[k].rot_kerr(params[k], 0.5 * dt);
    }

    let mut body_sum = C64::ZERO;
    for atom in &atoms[1..count] {
        body_sum = body_sum.add(atom.z);
    }
    let v0_a = v_in.scale(in_scale).add(body_sum.scale(cc * m_inv));
    let _ = atoms[0].cubic_pot(v0_a, params[0].g, 0.5 * dt);

    let v_body = atoms[0].z.scale(cc);
    let mut body_sum_b = C64::ZERO;
    for k in 1..count {
        let _ = atoms[k].cubic_pot(v_body, params[k].g, dt);
        body_sum_b = body_sum_b.add(atoms[k].z);
    }

    let v0_b = v_in.scale(in_scale).add(body_sum_b.scale(cc * m_inv));
    let _ = atoms[0].cubic_pot(v0_b, params[0].g, 0.5 * dt);

    for k in 0..count {
        atoms[k].rot_kerr(params[k], 0.5 * dt);
    }
}

/// Strictly convex non-expansive closed ring (`tau = 0` symmetric Strang):
/// `v_0 = (1 - c) * v_in + c * z_{N-1}`, `v_k = c * z_{k-1}`.
#[inline]
pub fn step_ring(
    atoms: &mut [CubicAtom; MOBIUS_MAX_ATOMS],
    params: &[AtomParams; MOBIUS_MAX_ATOMS],
    n: usize,
    v_in: C64,
    c: f64,
    dt: f64,
) {
    let count = n.clamp(1, MOBIUS_MAX_ATOMS);
    if count == 1 {
        atoms[0].step_single(params[0], v_in, dt);
        return;
    }
    let cc = c.clamp(0.0, 1.0);
    for k in 0..count {
        atoms[k].rot_kerr(params[k], 0.5 * dt);
    }

    for k in 0..(count - 1) {
        let vk = if k == 0 {
            v_in.scale(1.0 - cc).add(atoms[count - 1].z.scale(cc))
        } else {
            atoms[k - 1].z.scale(cc)
        };
        let _ = atoms[k].cubic_pot(vk, params[k].g, 0.5 * dt);
    }
    let last = count - 1;
    let v_last = atoms[last - 1].z.scale(cc);
    let _ = atoms[last].cubic_pot(v_last, params[last].g, dt);
    for k in (0..last).rev() {
        let vk = if k == 0 {
            v_in.scale(1.0 - cc).add(atoms[last].z.scale(cc))
        } else {
            atoms[k - 1].z.scale(cc)
        };
        let _ = atoms[k].cubic_pot(vk, params[k].g, 0.5 * dt);
    }

    for k in 0..count {
        atoms[k].rot_kerr(params[k], 0.5 * dt);
    }
}

/// Exact `O(dt^2)` palindromic all-to-all mean-field Kuramoto swarm (`tau = 0`):
/// `v_k = (1 - c) * v_in + (c / (N - 1)) * sum_{j != k} z_j`.
#[inline]
pub fn step_swarm(
    atoms: &mut [CubicAtom; MOBIUS_MAX_ATOMS],
    params: &[AtomParams; MOBIUS_MAX_ATOMS],
    n: usize,
    v_in: C64,
    c: f64,
    dt: f64,
) {
    let count = n.clamp(1, MOBIUS_MAX_ATOMS);
    if count == 1 {
        atoms[0].step_single(params[0], v_in, dt);
        return;
    }
    let cc = c.clamp(0.0, 1.0);
    let inv_nm1 = 1.0 / (count - 1) as f64;
    let in_part = v_in.scale(1.0 - cc);

    for k in 0..count {
        atoms[k].rot_kerr(params[k], 0.5 * dt);
    }

    let mut sum = C64::ZERO;
    for atom in &atoms[..count] {
        sum = sum.add(atom.z);
    }

    for k in 0..(count - 1) {
        let old = atoms[k].z;
        let others = sum.sub(old).scale(inv_nm1);
        let vk = in_part.add(others.scale(cc));
        let _ = atoms[k].cubic_pot(vk, params[k].g, 0.5 * dt);
        sum = sum.sub(old).add(atoms[k].z);
    }

    let last = count - 1;
    {
        let old = atoms[last].z;
        let others = sum.sub(old).scale(inv_nm1);
        let vk = in_part.add(others.scale(cc));
        let _ = atoms[last].cubic_pot(vk, params[last].g, dt);
        sum = sum.sub(old).add(atoms[last].z);
    }

    for k in (0..last).rev() {
        let old = atoms[k].z;
        let others = sum.sub(old).scale(inv_nm1);
        let vk = in_part.add(others.scale(cc));
        let _ = atoms[k].cubic_pot(vk, params[k].g, 0.5 * dt);
        sum = sum.sub(old).add(atoms[k].z);
    }

    for k in 0..count {
        atoms[k].rot_kerr(params[k], 0.5 * dt);
    }
}

/// Exact `O(dt^2)` two-stage hybrid acoustic bell/bar (`Core` doublet `A0 <-> A1` +
/// `Halo` Moreau bloom chain `A0 -> A2 -> ... -> A_{N-1}`).
#[inline]
pub fn step_hybrid(
    atoms: &mut [CubicAtom; MOBIUS_MAX_ATOMS],
    params: &[AtomParams; MOBIUS_MAX_ATOMS],
    n: usize,
    v_in: C64,
    coupling: f64,
    dt: f64,
) {
    let count = n.clamp(1, MOBIUS_MAX_ATOMS);
    if count == 1 {
        atoms[0].step_single(params[0], v_in, dt);
        return;
    }
    let coup = coupling.clamp(0.0, 1.0);
    let c_pair = (0.35 + 0.60 * coup).clamp(0.0, 0.96);
    if count == 2 {
        step_star(atoms, params, 2, v_in, c_pair, dt);
        return;
    }
    let eta = 2.0 * coup;
    let in_scale = 1.0 - c_pair * c_pair;

    for k in 0..count {
        atoms[k].rot_kerr(params[k], 0.5 * dt);
    }

    let v0a = v_in.scale(in_scale).add(atoms[1].z.scale(c_pair));
    let (_, flux_0a) = atoms[0].cubic_pot(v0a, params[0].g, 0.5 * dt);

    let v1 = atoms[0].z.scale(c_pair);
    let _ = atoms[1].cubic_pot(v1, params[1].g, dt);

    let v0b = v_in.scale(in_scale).add(atoms[1].z.scale(c_pair));
    let (_, flux_0b) = atoms[0].cubic_pot(v0b, params[0].g, 0.5 * dt);

    // Total step-averaged flux from A0 across both half-steps: 0.5 * (flux_0a + flux_0b).
    let mut v_halo = flux_0a.add(flux_0b).scale(0.5 * eta);
    for k in 2..count {
        let (_, flux_k) = atoms[k].cubic_pot(v_halo, params[k].g, dt);
        v_halo = flux_k.scale(eta);
    }

    for k in 0..count {
        atoms[k].rot_kerr(params[k], 0.5 * dt);
    }
}

/// Space-time delayed step (`tau > 0`) for any [`MobiusTopology`], strictly composing
/// exact 3-stage `CubicAtom` steps with convex linear-interpolated `MobiusDelayBank` reads.
#[inline]
#[allow(clippy::too_many_arguments)]
fn step_delayed(
    topology: MobiusTopology,
    atoms: &mut [CubicAtom; MOBIUS_MAX_ATOMS],
    params: &[AtomParams; MOBIUS_MAX_ATOMS],
    delays: &mut MobiusDelayBank,
    delay_samples: &[f64; MOBIUS_MAX_ATOMS],
    n: usize,
    v_in: C64,
    coupling: f64,
    dt: f64,
) {
    let count = n.clamp(1, MOBIUS_MAX_ATOMS);
    if count == 1 {
        atoms[0].step_single(params[0], v_in, dt);
        return;
    }
    let coup = coupling.clamp(0.0, 1.0);
    let c_net = (0.96 * coup).clamp(0.0, 0.96);
    let eta = 2.0 * coup;

    // Read all delayed signals prior to advancing states so feedback causality is exact.
    let mut delayed = [C64::ZERO; MOBIUS_MAX_ATOMS];
    for k in 0..count {
        let d = delay_samples[k];
        delayed[k] = if d >= 1.0 {
            delays.read_atom(k, d)
        } else {
            atoms[k].z
        };
    }

    match topology {
        MobiusTopology::Cascade => {
            let mut push_vals = [C64::ZERO; MOBIUS_MAX_ATOMS];
            for k in 0..count {
                let vk = if k == 0 {
                    v_in
                } else {
                    delayed[k - 1].scale(coup)
                };
                let (w_abs, _) = atoms[k].step_single(params[k], vk, dt);
                push_vals[k] = w_abs.neg();
            }
            for (k, &val) in push_vals[..count].iter().enumerate() {
                delays.push_atom(k, val);
            }
        }
        MobiusTopology::Bloom => {
            let mut push_vals = [C64::ZERO; MOBIUS_MAX_ATOMS];
            for k in 0..count {
                let vk = if k == 0 {
                    v_in
                } else {
                    delayed[k - 1].scale(eta)
                };
                let (_, flux) = atoms[k].step_single(params[k], vk, dt);
                push_vals[k] = flux;
            }
            for (k, &val) in push_vals[..count].iter().enumerate() {
                delays.push_atom(k, val);
            }
        }
        MobiusTopology::Star => {
            let m_inv = 1.0 / (count - 1) as f64;
            let mut spoke_sum = C64::ZERO;
            for &z_del in &delayed[1..count] {
                spoke_sum = spoke_sum.add(z_del);
            }
            let v0 = v_in
                .scale(1.0 - c_net * c_net)
                .add(spoke_sum.scale(c_net * m_inv));
            let _ = atoms[0].step_single(params[0], v0, dt);

            for k in 1..count {
                let d_k = delay_samples[k];
                let z0_del = if d_k >= 1.0 {
                    delays.read_atom(0, d_k)
                } else {
                    atoms[0].z
                };
                let _ = atoms[k].step_single(params[k], z0_del.scale(c_net), dt);
            }
            for (k, atom) in atoms[..count].iter().enumerate() {
                delays.push_atom(k, atom.z);
            }
        }
        MobiusTopology::Ring => {
            for k in 0..count {
                let prev = if k == 0 { count - 1 } else { k - 1 };
                let vk = if k == 0 {
                    v_in.scale(1.0 - c_net).add(delayed[prev].scale(c_net))
                } else {
                    delayed[prev].scale(c_net)
                };
                let _ = atoms[k].step_single(params[k], vk, dt);
            }
            for (k, atom) in atoms[..count].iter().enumerate() {
                delays.push_atom(k, atom.z);
            }
        }
        MobiusTopology::Swarm => {
            let inv_nm1 = 1.0 / (count - 1) as f64;
            let mut sum = C64::ZERO;
            for &z_del in &delayed[..count] {
                sum = sum.add(z_del);
            }
            let in_part = v_in.scale(1.0 - c_net);
            for k in 0..count {
                let others = sum.sub(delayed[k]).scale(inv_nm1);
                let vk = in_part.add(others.scale(c_net));
                let _ = atoms[k].step_single(params[k], vk, dt);
            }
            for (k, atom) in atoms[..count].iter().enumerate() {
                delays.push_atom(k, atom.z);
            }
        }
        MobiusTopology::Hybrid => {
            let c_pair = (0.35 + 0.60 * coup).clamp(0.0, 0.96);
            if count == 2 {
                let v0 = v_in
                    .scale(1.0 - c_pair * c_pair)
                    .add(delayed[1].scale(c_pair));
                let _ = atoms[0].step_single(params[0], v0, dt);
                let _ = atoms[1].step_single(params[1], delayed[0].scale(c_pair), dt);
                delays.push_atom(0, atoms[0].z);
                delays.push_atom(1, atoms[1].z);
                return;
            }
            // Core doublet A0 <-> A1 at tau = 0 for tight body lock, with delayed Bloom halo A2..N.
            atoms[0].rot_kerr(params[0], 0.5 * dt);
            atoms[1].rot_kerr(params[1], 0.5 * dt);
            let in_scale = 1.0 - c_pair * c_pair;
            let v0a = v_in.scale(in_scale).add(atoms[1].z.scale(c_pair));
            let (_, flux_0a) = atoms[0].cubic_pot(v0a, params[0].g, 0.5 * dt);
            let _ = atoms[1].cubic_pot(atoms[0].z.scale(c_pair), params[1].g, dt);
            let v0b = v_in.scale(in_scale).add(atoms[1].z.scale(c_pair));
            let (_, flux_0b) = atoms[0].cubic_pot(v0b, params[0].g, 0.5 * dt);
            atoms[0].rot_kerr(params[0], 0.5 * dt);
            atoms[1].rot_kerr(params[1], 0.5 * dt);

            let mut push_vals = [C64::ZERO; MOBIUS_MAX_ATOMS];
            push_vals[0] = flux_0a.add(flux_0b).scale(0.5);
            push_vals[1] = atoms[1].z;

            for k in 2..count {
                let src_idx = if k == 2 { 0 } else { k - 1 };
                let vk = delayed[src_idx].scale(eta);
                let (_, flux_k) = atoms[k].step_single(params[k], vk, dt);
                push_vals[k] = flux_k;
            }
            for (k, &val) in push_vals[..count].iter().enumerate() {
                delays.push_atom(k, val);
            }
        }
    }
}

// =============================================================================
// SPOT DESCRIPTORS (SHARED BY REAL-TIME SYNTH & GPU VISUALIZERS)
// =============================================================================

/// Analytical description of one active cubic atom in a [`MobiusSynth`] bank
/// (shared SPOT between real-time DSP and UI visualizers).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MobiusAtomDescriptor {
    /// Natural frequency `f_k` (Hz).
    pub freq_hz: f64,
    /// Frequency ratio `f_k / f_0`.
    pub ratio: f64,
    /// Dimensionless stiffness `xi_k = g_k / omega_k` (at reference tracking).
    pub xi: f64,
    /// Physical cubic rate `g_k`.
    pub g: f64,
    /// State-Kerr pitch bend at unit amplitude `|z| = 1` (semitones).
    pub kerr_st: f64,
    /// Dimensionless State-Kerr coefficient `beta_k = 2^(kerr_st/12) - 1`.
    pub beta: f64,
    /// Quadrature stereo projection angle `theta_k in [-pi/2, +pi/2]` (radians).
    pub stereo_angle_rad: f64,
    /// Propagation delay assigned to this atom's outgoing edge (ms).
    pub delay_ms: f64,
    /// True when this atom is part of the active `0..N` bank.
    pub active: bool,
}

/// Complete real-time parameter snapshot for [`MobiusSynth`] and [`mobius_atom_descriptors`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MobiusSynthParams {
    pub exciter: MobiusExciter,
    pub base_hz: f64,
    pub brightness: f64,
    pub shape: f64,
    pub chirality: f64,
    pub attack_ms: f64,
    pub decay_ms: f64,
    pub sustain: f64,
    pub atoms: usize,
    pub dispersion: MobiusDispersion,
    pub structure: f64,
    pub stiffness: f64,
    pub stiffness_tilt: f64,
    pub kerr_semitones: f64,
    pub kerr_spread: f64,
    pub key_track: f64,
    pub topology: MobiusTopology,
    pub coupling: f64,
    pub distance_ms: f64,
    pub feedback: f64,
    pub fb_lock: bool,
    pub fb_cycles: f64,
    pub fb_ms: f64,
    pub stereo_width: f64,
}

impl Default for MobiusSynthParams {
    fn default() -> Self {
        Self {
            exciter: MobiusExciter::Dipole,
            base_hz: 220.0,
            brightness: 0.72,
            shape: 0.50,
            chirality: 0.20,
            attack_ms: 4.0,
            decay_ms: 220.0,
            sustain: 0.80,
            atoms: 2,
            dispersion: MobiusDispersion::Harmonic,
            structure: 0.35,
            stiffness: 0.20,
            stiffness_tilt: 0.0,
            kerr_semitones: 1.5,
            kerr_spread: 0.0,
            key_track: 1.0,
            topology: MobiusTopology::Cascade,
            coupling: 0.85,
            distance_ms: 0.0,
            feedback: 0.0,
            fb_lock: true,
            fb_cycles: 1.5,
            fb_ms: 8.4,
            stereo_width: 0.75,
        }
    }
}

/// Interpolate formant frequency for atom index `k >= 1` across the 5 canonical vowels.
#[inline]
fn formant_freq_hz(k: usize, structure: f64) -> f64 {
    let idx = (k - 1).min(MOBIUS_MAX_ATOMS - 2);
    let pos = structure.clamp(0.0, 1.0) * 4.0;
    let lo = (pos.floor() as usize).min(3);
    let hi = lo + 1;
    let frac = pos - lo as f64;
    let f_lo = VOWEL_FORMANTS_HZ[lo][idx];
    let f_hi = VOWEL_FORMANTS_HZ[hi][idx];
    // Log-frequency interpolation for natural vocal formant glides.
    f_lo * (f_hi / f_lo).powf(frac)
}

/// Compute symmetric opposite-angle quadrature stereo angle `theta_k` in `[-theta_max, +theta_max]`.
#[inline]
fn atom_stereo_angle(k: usize, n: usize, theta_max: f64) -> f64 {
    if n <= 1 {
        return theta_max;
    }
    if n % 2 == 1 {
        if k == 0 {
            return 0.0;
        }
        let pair_idx = (k - 1) / 2;
        let pair_count = (n - 1) / 2;
        let mag = if pair_count <= 1 {
            1.0
        } else {
            0.65 + 0.35 * (pair_idx as f64 / (pair_count - 1) as f64)
        };
        let sign = if k % 2 == 1 { -1.0 } else { 1.0 };
        sign * mag * theta_max
    } else {
        let pair_idx = k / 2;
        let pair_count = n / 2;
        let mag = if pair_count <= 1 {
            1.0
        } else {
            0.65 + 0.35 * (pair_idx as f64 / (pair_count - 1) as f64)
        };
        let sign = if k.is_multiple_of(2) { -1.0 } else { 1.0 };
        sign * mag * theta_max
    }
}

/// Pure SPOT computation of all active atom frequencies, stiffnesses, Kerr bends,
/// quadrature stereo angles, and coprime inter-atom propagation delays.
pub fn mobius_atom_descriptors(
    params: &MobiusSynthParams,
    sample_rate: f64,
) -> [MobiusAtomDescriptor; MOBIUS_MAX_ATOMS] {
    let sr = sanitize_sample_rate(sample_rate);
    let max_hz = (sr * NYQUIST_SAFE_RATIO).min(MOBIUS_AUDIBLE_MAX_HZ);
    let f0 = sanitize_clamp_f64(
        params.base_hz,
        f64::from(MOBIUS_MIN_HZ),
        max_hz.min(f64::from(MOBIUS_MAX_HZ)),
        220.0,
    );
    let n = params.atoms.clamp(1, MOBIUS_MAX_ATOMS);
    let struc = sanitize_clamp_f64(params.structure, 0.0, 1.0, 0.35);
    let xi0 = sanitize_clamp_f64(
        params.stiffness,
        f64::from(MOBIUS_MIN_STIFFNESS),
        f64::from(MOBIUS_MAX_STIFFNESS),
        0.20,
    );
    let tilt = sanitize_clamp_f64(
        params.stiffness_tilt,
        f64::from(MOBIUS_MIN_STIFFNESS_TILT),
        f64::from(MOBIUS_MAX_STIFFNESS_TILT),
        0.0,
    );
    let kerr0 = sanitize_clamp_f64(
        params.kerr_semitones,
        f64::from(MOBIUS_MIN_KERR_ST),
        f64::from(MOBIUS_MAX_KERR_ST),
        0.0,
    );
    let kspread = sanitize_clamp_f64(params.kerr_spread, -1.0, 1.0, 0.0);
    let key_track = sanitize_clamp_f64(params.key_track, 0.0, 1.0, 1.0);
    let dist_ms = sanitize_clamp_f64(
        params.distance_ms,
        0.0,
        f64::from(MOBIUS_MAX_DISTANCE_MS),
        0.0,
    );
    let theta_max = sanitize_clamp_f64(params.stereo_width, 0.0, 1.0, 0.75) * FRAC_PI_2;

    let max_coprime = if params.topology == MobiusTopology::Star && n >= 2 {
        COPRIME_RATIOS[n - 2]
    } else {
        COPRIME_RATIOS[n - 1]
    };

    let mut out = [MobiusAtomDescriptor::default(); MOBIUS_MAX_ATOMS];
    for (k, slot) in out.iter_mut().enumerate().take(n) {
        let norm_k = if n > 1 {
            k as f64 / (n - 1) as f64
        } else {
            0.0
        };

        let raw_hz = match params.dispersion {
            MobiusDispersion::Harmonic => {
                let m = (k + 1) as f64;
                let stiff_b = 0.02 * struc * struc;
                f0 * m * (1.0 + stiff_b * m * m).sqrt()
            }
            MobiusDispersion::Bar => {
                let exp = 1.65 + 0.55 * struc;
                if params.topology == MobiusTopology::Hybrid && n >= 3 {
                    if k == 0 {
                        f0
                    } else if k == 1 {
                        f0 * (1.008 + 0.015 * struc)
                    } else {
                        let m = (k - 1) as f64;
                        f0 * ((2.0 * m + 3.0112) / 3.0112).powf(exp)
                    }
                } else {
                    let m = k as f64;
                    f0 * ((2.0 * m + 3.0112) / 3.0112).powf(exp)
                }
            }
            MobiusDispersion::Cluster => {
                let width_cents = 4.0 + 116.0 * struc;
                let offset_cents = if n <= 1 {
                    0.0
                } else if matches!(
                    params.topology,
                    MobiusTopology::Star | MobiusTopology::Cascade
                ) {
                    if k == 0 {
                        0.0
                    } else {
                        let sign = if k % 2 == 1 { 1.0 } else { -1.0 };
                        let mag = k.div_ceil(2) as f64 / ((n / 2).max(1) as f64);
                        sign * mag * width_cents
                    }
                } else {
                    (2.0 * norm_k - 1.0) * width_cents
                };
                f0 * 2.0_f64.powf(offset_cents / 1200.0)
            }
            MobiusDispersion::Formant => {
                if k == 0 {
                    f0
                } else {
                    formant_freq_hz(k, struc)
                }
            }
            MobiusDispersion::Ratio => {
                let base_ratio = 1.25 + 1.75 * struc;
                f0 * base_ratio.powi(k as i32)
            }
        };

        let freq_hz = raw_hz.clamp(f64::from(MOBIUS_MIN_HZ), max_hz);
        let xi_k = (xi0 * 2.0_f64.powf(tilt * norm_k)).clamp(
            f64::from(MOBIUS_MIN_STIFFNESS),
            f64::from(MOBIUS_MAX_STIFFNESS),
        );
        let g = stiffness_to_g(xi_k, freq_hz, key_track);

        let kerr_k = if kspread < 0.0 {
            // Interpolate continuously from uniform (`0`) to alternating opposite-sign (`-1`).
            let alt = if k.is_multiple_of(2) { 1.0 } else { -1.0 };
            let factor = 1.0 + kspread.abs() * (alt - 1.0);
            kerr0 * factor
        } else {
            // Ascending Kerr gradient across upper atoms (`0..+1`).
            kerr0 * (1.0 + kspread * (2.0 * norm_k - 1.0))
        }
        .clamp(f64::from(MOBIUS_MIN_KERR_ST), f64::from(MOBIUS_MAX_KERR_ST));
        let beta = semitones_to_beta(kerr_k);

        let delay_ms = if dist_ms <= 0.0 || n <= 1 {
            0.0
        } else if params.topology == MobiusTopology::Star {
            if k == 0 {
                0.0
            } else {
                dist_ms * (COPRIME_RATIOS[k - 1] / max_coprime)
            }
        } else if params.topology == MobiusTopology::Ring && n == 2 {
            if k == 0 {
                0.74 * dist_ms
            } else {
                dist_ms
            }
        } else {
            dist_ms * (COPRIME_RATIOS[k] / max_coprime)
        };

        *slot = MobiusAtomDescriptor {
            freq_hz,
            ratio: freq_hz / f0,
            xi: xi_k,
            g,
            kerr_st: kerr_k,
            beta,
            stereo_angle_rad: atom_stereo_angle(k, n, theta_max),
            delay_ms,
            active: true,
        };
    }
    out
}

/// Pre-sample the exciter target waveform for the GPU visualizer (SPOT shared with DSP).
///
/// Returns `points` samples normalized cleanly into `[-1.0, 1.0]`.
pub fn mobius_target_preview(
    exciter: MobiusExciter,
    brightness: f32,
    shape: f32,
    chirality: f32,
    points: usize,
) -> Vec<f32> {
    let count = points.max(8);
    let b = sanitize_clamp_f64(f64::from(brightness), 0.0, 1.0, 0.72);
    let sh = sanitize_clamp_f64(f64::from(shape), 0.0, 1.0, 0.50);
    let chi = sanitize_clamp_f64(f64::from(chirality), 0.0, 1.0, 0.20);
    let ref_sr = 48_000.0;
    let ref_f0 = 220.0;
    let mut raw = Vec::with_capacity(count);
    let mut peak = 1e-6_f64;

    for i in 0..count {
        let u = i as f64 / (count - 1) as f64;
        let sample = match exciter {
            MobiusExciter::Szego => {
                let r = b * r_max_szego(ref_f0, ref_sr);
                let v = apply_chirality(eval_szego(2.0 * TAU * u, r), chi);
                v.re
            }
            MobiusExciter::Dipole => {
                let r = b * r_max_dipole(ref_f0, ref_sr);
                let psi = PI * (1.0 - sh);
                let v = apply_chirality(eval_dipole(2.0 * TAU * u, r, psi), chi);
                v.re
            }
            MobiusExciter::Poisson => {
                let k = 1.0 + 7.0 * sh;
                let r = b * r_max_poisson(ref_f0, ref_f0 * k, ref_sr);
                let th_v = 2.0 * TAU * u;
                let v = apply_chirality(eval_poisson(th_v, th_v * k, r), chi);
                v.re
            }
            MobiusExciter::Strike => {
                // Visual window spans 16 ms so mallet contact pulse narrows as brightness rises.
                let t = u * 0.016;
                let dur = strike_duration_s(b);
                if t <= dur {
                    0.5 * (1.0 - (TAU * t / dur).cos())
                } else {
                    0.0
                }
            }
            MobiusExciter::External => (2.0 * TAU * u).sin() * 0.75,
        };
        peak = peak.max(sample.abs());
        raw.push(sample);
    }

    let scale = if peak > 1.0 { 1.0 / peak } else { 1.0 };
    raw.into_iter().map(|s| (s * scale) as f32).collect()
}

// =============================================================================
// REAL-TIME SYNTHESIZER ENGINE (`MobiusSynth`)
// =============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DriveEnvStage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// Complete real-time, zero-allocation Complex State-Kerr Cubic-Atom Synthesizer bank.
///
/// Core atom state (~1 KB) contains zero heap pointers when constructed with
/// [`MobiusSynth::new_core`]; a pre-allocated [`MobiusDelayBank`] can be attached or
/// returned in O(1) via pointer swap (`attach_delay_bank` / `take_delay_bank`) for
/// zero-allocation voice pooling across 256 voices.
#[derive(Clone, Debug)]
pub struct MobiusSynth {
    sample_rate: f64,
    dt: f64,
    params: MobiusSynthParams,
    atoms: [CubicAtom; MOBIUS_MAX_ATOMS],
    atom_params: [AtomParams; MOBIUS_MAX_ATOMS],
    stereo_angles: [f64; MOBIUS_MAX_ATOMS],
    delay_samples: [f64; MOBIUS_MAX_ATOMS],
    active_atoms: usize,
    output_norm: f64,

    // Target source & drive envelope state.
    theta_v: f64,
    theta_f: f64,
    velocity: f64,
    drive_stage: DriveEnvStage,
    drive_level: f64,
    drive_release_start: f64,
    drive_release_pos: f64,
    strike_elapsed_s: f64,
    strike_duration: f64,
    strike_active: bool,
    strike_amp: f64,
    note_released: bool,

    // Host-only output de-click fade (used strictly for choke / force_release, NEVER in ODE).
    host_fade: f64,
    host_fade_active: bool,
    host_fade_coeff: f64,

    // Optional pre-allocated convex delay slab (swapped in/out by VoicePool with zero RT allocs).
    delays: Option<Box<MobiusDelayBank>>,
}

impl Default for MobiusSynth {
    fn default() -> Self {
        Self::new(48_000.0)
    }
}

impl MobiusSynth {
    /// Construct a lightweight voice-pool core with zero delay allocation (attach a pooled
    /// [`MobiusDelayBank`] via [`Self::attach_delay_bank`] on trigger).
    pub fn new_core(sample_rate: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate).clamp(8_000.0, 384_000.0);
        let mut synth = Self {
            sample_rate: sr,
            dt: 1.0 / sr,
            params: MobiusSynthParams::default(),
            atoms: [CubicAtom::default(); MOBIUS_MAX_ATOMS],
            atom_params: [AtomParams::default(); MOBIUS_MAX_ATOMS],
            stereo_angles: [0.0; MOBIUS_MAX_ATOMS],
            delay_samples: [0.0; MOBIUS_MAX_ATOMS],
            active_atoms: 2,
            output_norm: 0.72,
            theta_v: 0.0,
            theta_f: 0.0,
            velocity: 1.0,
            drive_stage: DriveEnvStage::Idle,
            drive_level: 0.0,
            drive_release_start: 0.0,
            drive_release_pos: 0.0,
            strike_elapsed_s: 0.0,
            strike_duration: 0.002,
            strike_active: false,
            strike_amp: 0.0,
            note_released: true,
            host_fade: 1.0,
            host_fade_active: false,
            host_fade_coeff: 1.0,
            delays: None,
        };
        synth.set_parameters(&MobiusSynthParams::default());
        synth
    }

    /// Construct a standalone [`MobiusSynth`] owning its own [`MobiusDelayBank`].
    pub fn new(sample_rate: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate).clamp(8_000.0, 384_000.0);
        let mut synth = Self::new_core(sr);
        synth.delays = Some(Box::new(MobiusDelayBank::new(sr)));
        synth.set_parameters(&MobiusSynthParams::default());
        synth
    }

    /// Attach a pre-allocated [`MobiusDelayBank`] in O(1) with zero heap operations.
    #[inline]
    pub fn attach_delay_bank(&mut self, mut bank: Box<MobiusDelayBank>) {
        bank.partition(self.active_atoms);
        bank.clear();
        self.delays = Some(bank);
    }

    /// Detach and return the owned [`MobiusDelayBank`] in O(1) for return to a pool.
    #[inline]
    pub fn take_delay_bank(&mut self) -> Option<Box<MobiusDelayBank>> {
        self.delays.take()
    }

    #[inline]
    pub fn has_delay_bank(&self) -> bool {
        self.delays.is_some()
    }

    /// Update sample rate without clearing ringing atoms; returns `true` if rate changed.
    #[inline]
    pub fn set_sample_rate(&mut self, sample_rate: f64) -> bool {
        let sr = sanitize_sample_rate(sample_rate).clamp(8_000.0, 384_000.0);
        if sr.to_bits() == self.sample_rate.to_bits() {
            return false;
        }
        self.sample_rate = sr;
        self.dt = 1.0 / sr;
        true
    }

    /// Silence all atoms, exciter envelopes, and any attached delay bank without reallocating.
    #[inline]
    pub fn reset(&mut self, sample_rate: f64) {
        let sr = sanitize_sample_rate(sample_rate).clamp(8_000.0, 384_000.0);
        self.sample_rate = sr;
        self.dt = 1.0 / sr;
        self.atoms = [CubicAtom::default(); MOBIUS_MAX_ATOMS];
        self.theta_v = 0.0;
        self.theta_f = 0.0;
        self.velocity = 1.0;
        self.drive_stage = DriveEnvStage::Idle;
        self.drive_level = 0.0;
        self.drive_release_start = 0.0;
        self.drive_release_pos = 0.0;
        self.strike_elapsed_s = 0.0;
        self.strike_active = false;
        self.strike_amp = 0.0;
        self.note_released = true;
        self.host_fade = 1.0;
        self.host_fade_active = false;
        self.host_fade_coeff = 1.0;
        if let Some(delays) = self.delays.as_mut() {
            delays.clear();
        }
    }

    /// Trigger a fresh note or legato retarget.
    #[inline]
    pub fn trigger(
        &mut self,
        velocity: f64,
        params: &MobiusSynthParams,
        legato: bool,
        phase_reset: bool,
    ) {
        let vel = sanitize_clamp_f64(velocity, 0.0, 2.0, 0.8);
        if !legato {
            self.atoms = [CubicAtom::default(); MOBIUS_MAX_ATOMS];
            if let Some(delays) = self.delays.as_mut() {
                delays.clear();
            }
            if phase_reset {
                self.theta_v = 0.0;
                self.theta_f = 0.0;
            }
            self.drive_level = 0.0;
        }
        self.set_parameters(params);
        self.velocity = vel;
        self.note_released = false;
        self.host_fade = 1.0;
        self.host_fade_active = false;
        self.host_fade_coeff = 1.0;

        if !legato
            || self.drive_stage == DriveEnvStage::Idle
            || self.drive_stage == DriveEnvStage::Release
        {
            self.start_exciter(vel, params);
        }
    }

    /// Re-trigger only the exciter (e.g., on rising `TrigIn` edge) while preserving ringing atoms.
    #[inline]
    pub fn retrigger_exciter(&mut self, velocity: f64, params: &MobiusSynthParams) {
        let vel = sanitize_clamp_f64(velocity, 0.0, 2.0, 0.8);
        self.velocity = vel;
        self.note_released = false;
        self.start_exciter(vel, params);
    }

    #[inline]
    fn start_exciter(&mut self, vel: f64, params: &MobiusSynthParams) {
        match params.exciter {
            MobiusExciter::Strike => {
                let dur = strike_duration_s(params.brightness);
                self.strike_duration = dur;
                self.strike_elapsed_s = 0.0;
                self.strike_active = true;
                // Keep cubic momentum transfer `A^2 * tau_hit` balanced across mallet hardness.
                let hardness_comp = (0.004 / dur).sqrt().clamp(0.75, 3.6);
                self.strike_amp = vel * 1.8 * hardness_comp;
                self.drive_stage = DriveEnvStage::Idle;
                self.drive_level = 0.0;
            }
            MobiusExciter::External => {
                self.strike_active = false;
                self.drive_stage = DriveEnvStage::Sustain;
                self.drive_level = 1.0;
            }
            MobiusExciter::Szego | MobiusExciter::Dipole | MobiusExciter::Poisson => {
                self.strike_active = false;
                self.drive_stage = DriveEnvStage::Attack;
            }
        }
    }

    /// Release the exciter target drive smoothly (`A(t) -> 0` over 4 ms) while letting
    /// all pure `mu = 0` cubic atoms ring out freely.
    #[inline]
    pub fn note_off(&mut self) {
        self.note_released = true;
        if self.drive_stage != DriveEnvStage::Idle && self.drive_stage != DriveEnvStage::Release {
            self.drive_release_start = self.drive_level;
            self.drive_release_pos = 0.0;
            self.drive_stage = DriveEnvStage::Release;
        }
    }

    /// Engage a host-side output de-click fade strictly for choke groups or panic release
    /// without altering the internal pure `mu = 0` atom ODE.
    #[inline]
    pub fn begin_host_fade(&mut self, fade_coeff: f64) {
        self.note_off();
        self.host_fade_active = true;
        self.host_fade_coeff = fade_coeff.clamp(0.0, 0.999_99);
    }

    /// Apply updated physical invariants and recompute per-atom parameters from SPOT.
    #[inline]
    pub fn set_parameters(&mut self, params: &MobiusSynthParams) {
        let desc = mobius_atom_descriptors(params, self.sample_rate);
        let n = params.atoms.clamp(1, MOBIUS_MAX_ATOMS);
        let sr = self.sample_rate;

        for (k, d) in desc.iter().enumerate() {
            if k < n && d.active {
                self.atom_params[k] = AtomParams {
                    omega: TAU * d.freq_hz,
                    g: d.g,
                    beta: d.beta,
                };
                self.stereo_angles[k] = d.stereo_angle_rad;
                self.delay_samples[k] = d.delay_ms * 0.001 * sr;
            } else {
                self.atom_params[k] = AtomParams::default();
                self.stereo_angles[k] = 0.0;
                self.delay_samples[k] = 0.0;
                self.atoms[k] = CubicAtom::default();
            }
        }

        if let Some(delays) = self.delays.as_mut() {
            delays.partition(n);
        }

        self.params = *params;
        self.params.atoms = n;
        self.active_atoms = n;
        // Consistent musical loudness across N = 1..8 atoms without compressing dynamics.
        self.output_norm = 0.72 / (1.0 + 0.30 * (n - 1) as f64).sqrt();
    }

    /// Advance the internal exciter amplitude envelope `A(t)`.
    #[inline]
    fn step_drive_env(&mut self) -> f64 {
        match self.drive_stage {
            DriveEnvStage::Idle => 0.0,
            DriveEnvStage::Attack => {
                let atk_s = (self.params.attack_ms * 0.001).max(0.0001);
                let step = self.dt / atk_s;
                self.drive_level += step;
                if self.drive_level >= 1.0 {
                    self.drive_level = 1.0;
                    self.drive_stage = DriveEnvStage::Decay;
                }
                self.drive_level
            }
            DriveEnvStage::Decay => {
                let sus = self.params.sustain.clamp(0.0, 1.0);
                let dec_ms = self.params.decay_ms.max(1.0);
                let coeff = decay_coeff_60db_ms(dec_ms, self.sample_rate);
                self.drive_level = sus + (self.drive_level - sus) * coeff;
                if (self.drive_level - sus).abs() < 1e-5 {
                    self.drive_level = sus;
                    self.drive_stage = if sus <= 1e-6 {
                        DriveEnvStage::Idle
                    } else {
                        DriveEnvStage::Sustain
                    };
                }
                self.drive_level
            }
            DriveEnvStage::Sustain => {
                let sus = self.params.sustain.clamp(0.0, 1.0);
                self.drive_level = sus;
                sus
            }
            DriveEnvStage::Release => {
                self.drive_release_pos += self.dt / EXCITER_RELEASE_SEC;
                if self.drive_release_pos >= 1.0 {
                    self.drive_level = 0.0;
                    self.drive_stage = DriveEnvStage::Idle;
                } else {
                    // Smooth half-cosine ramp to zero with zero derivative click.
                    let w = 0.5 * (1.0 + (PI * self.drive_release_pos).cos());
                    self.drive_level = self.drive_release_start * w;
                }
                self.drive_level
            }
        }
    }

    /// Evaluate one stereo frame `[L, R]` at full `f64` internal precision.
    #[inline]
    pub fn process_stereo(&mut self, ext_in: [f64; 2], mix: f64) -> [f64; 2] {
        let dry_l = sanitize_sample_f64(ext_in[0]);
        let dry_r = sanitize_sample_f64(ext_in[1]);
        let m = sanitize_clamp_f64(mix, 0.0, 1.0, 1.0);
        let n = self.active_atoms;
        let dt = self.dt;
        let sr = self.sample_rate;

        // 1. Read delayed projective bore pressure `p_tilde in D` (|p_tilde| < 1 unconditionally).
        let fb = self.params.feedback.clamp(0.0, 1.0);
        let p_tilde = if fb > 1e-6 {
            let bore_d = if self.params.fb_lock {
                let f0 = self.params.base_hz.max(f64::from(MOBIUS_MIN_HZ));
                (self
                    .params
                    .fb_cycles
                    .clamp(0.0, f64::from(MOBIUS_MAX_FB_CYCLES))
                    / f0)
                    * sr
            } else {
                self.params.fb_ms.clamp(0.0, f64::from(MOBIUS_MAX_FB_MS)) * 0.001 * sr
            };
            let p_del = if let Some(delays) = self.delays.as_ref() {
                if bore_d >= 1.0 {
                    delays.read_bore(bore_d)
                } else {
                    self.bore_pressure()
                }
            } else {
                self.bore_pressure()
            };
            p_del.scale(1.0 / (1.0 + p_del.abs()))
        } else {
            C64::ZERO
        };

        // 2. Synthesize unit-disk complex target `v_in(t)` with exact Nyquist caps.
        let v_in = match self.params.exciter {
            MobiusExciter::External => {
                let mid = 0.5 * (dry_l + dry_r);
                let side = 0.5 * (dry_l - dry_r);
                apply_chirality(C64::new(mid, side), self.params.chirality)
            }
            MobiusExciter::Strike => {
                if self.strike_active {
                    let t = self.strike_elapsed_s + 0.5 * dt;
                    self.strike_elapsed_s += dt;
                    if t >= self.strike_duration {
                        self.strike_active = false;
                        C64::ZERO
                    } else {
                        let pulse = 0.5 * (1.0 - (TAU * t / self.strike_duration).cos());
                        C64::new(self.strike_amp * pulse, 0.0)
                    }
                } else {
                    C64::ZERO
                }
            }
            MobiusExciter::Szego | MobiusExciter::Dipole | MobiusExciter::Poisson => {
                let env_a = self.step_drive_env();
                let f0 = self.params.base_hz.clamp(
                    f64::from(MOBIUS_MIN_HZ),
                    (sr * NYQUIST_SAFE_RATIO).min(f64::from(MOBIUS_MAX_HZ)),
                );
                let u_cur = C64::from_polar(1.0, self.theta_v);
                // Bounded Adler/Kuramoto pitch entrainment from returning bore wave.
                let f_eff = (f0 * (1.0 + 0.28 * fb * p_tilde.mul(u_cur.conj()).im))
                    .clamp(f64::from(MOBIUS_MIN_HZ), sr * NYQUIST_SAFE_RATIO);
                let dth_v = TAU * f_eff * dt;
                let th_v_mid = self.theta_v + 0.5 * dth_v;
                self.theta_v = (self.theta_v + dth_v).rem_euclid(TAU);

                let k_formant = 1.0 + 7.0 * self.params.shape.clamp(0.0, 1.0);
                let f_formant = (f_eff * k_formant).min(sr * 0.49);
                let dth_f = TAU * f_formant * dt;
                let th_f_mid = self.theta_f + 0.5 * dth_f;
                self.theta_f = (self.theta_f + dth_f).rem_euclid(TAU);

                if env_a <= 0.0 {
                    C64::ZERO
                } else {
                    let bright = self.params.brightness.clamp(0.0, 1.0);
                    let th_pm = th_v_mid + 5.8 * fb * p_tilde.re;
                    let valve_mod = 1.0 / (1.0 + 3.5 * fb * p_tilde.re.max(0.0));
                    let v_c = match self.params.exciter {
                        MobiusExciter::Szego => {
                            let r_cap = r_max_szego(f_eff, sr);
                            let r_eff = (bright * r_cap + 0.38 * fb * p_tilde.abs()).min(r_cap);
                            eval_szego(th_pm, r_eff)
                        }
                        MobiusExciter::Dipole => {
                            let r_cap = r_max_dipole(f_eff, sr);
                            let r_eff = (bright * r_cap + 0.38 * fb * p_tilde.abs()).min(r_cap);
                            let psi = PI * (1.0 - self.params.shape.clamp(0.0, 1.0));
                            eval_dipole(th_pm, r_eff, psi)
                        }
                        MobiusExciter::Poisson => {
                            let r_cap = r_max_poisson(f_eff, f_formant, sr);
                            let r_eff = (bright * r_cap + 0.38 * fb * p_tilde.abs()).min(r_cap);
                            eval_poisson(th_pm, th_f_mid + 5.8 * fb * p_tilde.re, r_eff)
                        }
                        MobiusExciter::Strike | MobiusExciter::External => C64::ZERO,
                    };
                    // Slight bore drive headroom when feedback active matches reed valve pressure.
                    let fb_boost = 1.0 + 0.45 * fb;
                    apply_chirality(v_c, self.params.chirality)
                        .scale(env_a * self.velocity * fb_boost * valve_mod)
                }
            }
        };

        // 3. Advance the N-atom space-time network via exact closed-form primitives.
        let use_delay = self.params.distance_ms > 0.0 && n > 1 && self.delays.is_some();
        if use_delay {
            if let Some(delays) = self.delays.as_mut() {
                step_delayed(
                    self.params.topology,
                    &mut self.atoms,
                    &self.atom_params,
                    delays,
                    &self.delay_samples,
                    n,
                    v_in,
                    self.params.coupling,
                    dt,
                );
            }
        } else {
            let coup = self.params.coupling.clamp(0.0, 1.0);
            let c_net = (0.96 * coup).clamp(0.0, 0.96);
            match self.params.topology {
                MobiusTopology::Cascade => {
                    let _ = step_cascade(&mut self.atoms, &self.atom_params, n, v_in, coup, dt);
                }
                MobiusTopology::Bloom => {
                    step_bloom(&mut self.atoms, &self.atom_params, n, v_in, 2.0 * coup, dt);
                }
                MobiusTopology::Star => {
                    step_star(&mut self.atoms, &self.atom_params, n, v_in, c_net, dt);
                }
                MobiusTopology::Ring => {
                    step_ring(&mut self.atoms, &self.atom_params, n, v_in, c_net, dt);
                }
                MobiusTopology::Swarm => {
                    step_swarm(&mut self.atoms, &self.atom_params, n, v_in, c_net, dt);
                }
                MobiusTopology::Hybrid => {
                    step_hybrid(&mut self.atoms, &self.atom_params, n, v_in, coup, dt);
                }
            }
        }

        // Flush denormals and guard against unexpected external NaN/Inf.
        let mut energy = 0.0_f64;
        for atom in &mut self.atoms[..n] {
            if !atom.z.is_finite() {
                atom.z = C64::ZERO;
            } else {
                atom.z = atom.z.flush_denormal();
                energy += atom.z.abs();
            }
        }

        // Push body pressure into bore delay line and update O(1) delay drain tracker.
        let p_now = self.bore_pressure();
        let max_delay_samp = if use_delay {
            (self.params.distance_ms * 0.001 * sr).ceil() as usize + 4
        } else {
            0
        };
        if let Some(delays) = self.delays.as_mut() {
            delays.push_bore(p_now);
            delays.tick_activity(energy > VOICE_QUIET_FLOOR, max_delay_samp);
        }

        // 4. Zero-comb quadrature stereo projection.
        let mut wet_l = 0.0_f64;
        let mut wet_r = 0.0_f64;
        for k in 0..n {
            let [l, r] = stereo_quad(self.atoms[k].z, self.stereo_angles[k]);
            wet_l += l;
            wet_r += r;
        }
        wet_l *= self.output_norm;
        wet_r *= self.output_norm;

        // Host-only choke / force-release de-click fade (never alters atom physics).
        if self.host_fade_active {
            self.host_fade *= self.host_fade_coeff;
            if self.host_fade < TAIL_SILENCE_FLOOR_F64 {
                self.host_fade = 0.0;
            }
            wet_l *= self.host_fade;
            wet_r *= self.host_fade;
        }

        let wet = [flush_denormal_f64(wet_l), flush_denormal_f64(wet_r)];
        if self.params.exciter == MobiusExciter::External || m < 1.0 {
            mix_stereo_f64([dry_l, dry_r], wet, m)
        } else {
            wet
        }
    }

    #[inline]
    fn bore_pressure(&self) -> C64 {
        let n = self.active_atoms;
        let mut p = self.atoms[0].z;
        for k in 1..n {
            let w = 0.65_f64.powi(k as i32);
            p = p.add(self.atoms[k].z.scale(w));
        }
        p
    }

    /// Instantaneous summed complex amplitude across all active atoms (for voice priority stealing).
    #[inline]
    pub fn current_energy(&self) -> f64 {
        if self.host_fade_active && self.host_fade <= 0.0 {
            return 0.0;
        }
        let mut sum = 0.0_f64;
        for atom in &self.atoms[..self.active_atoms] {
            sum += atom.z.abs();
        }
        let e = sum * self.output_norm;
        if self.host_fade_active {
            e * self.host_fade
        } else {
            e
        }
    }

    /// True when the exciter is silent and all atoms + propagation delays have fallen below `-72 dB`
    /// (or a host choke/force-release fade has finished).
    #[inline]
    pub fn is_quiet(&self) -> bool {
        if self.host_fade_active {
            return self.host_fade <= 0.0;
        }
        if self.strike_active || self.drive_stage != DriveEnvStage::Idle {
            return false;
        }
        if self.params.exciter == MobiusExciter::External && !self.note_released {
            return false;
        }
        let delays_done = self
            .delays
            .as_deref()
            .is_none_or(MobiusDelayBank::is_drained);
        delays_done && self.current_energy() < VOICE_QUIET_FLOOR
    }

    /// True once the exciter is no longer actively driving the atoms.
    #[inline]
    pub fn is_exciter_released(&self) -> bool {
        self.note_released
            || self.host_fade_active
            || (self.params.exciter == MobiusExciter::Strike && !self.strike_active)
            || (self.drive_stage == DriveEnvStage::Idle
                && self.params.exciter != MobiusExciter::External)
    }

    /// Read-only inspection of current atom states (for unit & physics tests).
    #[inline]
    pub fn atoms(&self) -> &[CubicAtom] {
        &self.atoms[..self.active_atoms]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rk4_step_system<const N: usize>(
        z: &mut [C64; N],
        t0: f64,
        dt: f64,
        rhs: impl Fn(f64, &[C64; N]) -> [C64; N],
    ) {
        let k1 = rhs(t0, z);
        let mut z2 = [C64::ZERO; N];
        for i in 0..N {
            z2[i] = z[i].add(k1[i].scale(0.5 * dt));
        }
        let k2 = rhs(t0 + 0.5 * dt, &z2);
        let mut z3 = [C64::ZERO; N];
        for i in 0..N {
            z3[i] = z[i].add(k2[i].scale(0.5 * dt));
        }
        let k3 = rhs(t0 + 0.5 * dt, &z3);
        let mut z4 = [C64::ZERO; N];
        for i in 0..N {
            z4[i] = z[i].add(k3[i].scale(dt));
        }
        let k4 = rhs(t0 + dt, &z4);
        for i in 0..N {
            let sum = k1[i].add(k2[i].scale(2.0)).add(k3[i].scale(2.0)).add(k4[i]);
            z[i] = z[i].add(sum.scale(dt / 6.0));
        }
    }

    fn atom_rhs(z: C64, v: C64, p: AtomParams) -> C64 {
        let rot = C64::new(-z.im, z.re).scale(p.omega * (1.0 + p.beta * z.abs()));
        let w = z.sub(v);
        rot.sub(w.scale(p.g * w.abs()))
    }

    fn goertzel_mag(sig: &[f64], sr: f64, freq: f64) -> f64 {
        let n = sig.len();
        let mut wsum = 0.0;
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &x) in sig.iter().enumerate() {
            let win = 0.5 * (1.0 - (TAU * i as f64 / n as f64).cos());
            wsum += win;
            let (s, c) = (TAU * freq * i as f64 / sr).sin_cos();
            re += x * win * c;
            im -= x * win * s;
        }
        2.0 * re.hypot(im) / wsum.max(1e-18)
    }

    #[test]
    fn single_and_four_atom_topologies_converge_at_exact_second_order() {
        let dur = 0.05_f64;
        let sr_ref = 384_000.0_f64;
        let dt_ref = 1.0 / sr_ref;
        let n_ref = (dur * sr_ref).round() as usize;

        let p4 = [
            AtomParams::from_invariants(220.0, 0.25, 2.0, 1.0),
            AtomParams::from_invariants(330.0, 0.18, -1.5, 1.0),
            AtomParams::from_invariants(550.0, 0.12, 1.2, 1.0),
            AtomParams::from_invariants(770.0, 0.09, -0.8, 1.0),
        ];
        let v_at = |t: f64| eval_dipole(TAU * 220.0 * t, 0.65, PI * 0.6).scale(0.85);

        let run_err = |topo: MobiusTopology, sr: f64, z_ref: &[C64; 4]| -> f64 {
            let dt = 1.0 / sr;
            let steps = (dur * sr).round() as usize;
            let mut atoms = [CubicAtom::default(); MOBIUS_MAX_ATOMS];
            let mut params = [AtomParams::default(); MOBIUS_MAX_ATOMS];
            params[..4].copy_from_slice(&p4);
            for i in 0..steps {
                let v = v_at((i as f64 + 0.5) * dt);
                match topo {
                    MobiusTopology::Cascade => {
                        let _ = step_cascade(&mut atoms, &params, 4, v, 1.0, dt);
                    }
                    MobiusTopology::Bloom => step_bloom(&mut atoms, &params, 4, v, 0.9, dt),
                    MobiusTopology::Star => step_star(&mut atoms, &params, 4, v, 0.72, dt),
                    MobiusTopology::Swarm => step_swarm(&mut atoms, &params, 4, v, 0.72, dt),
                    _ => unreachable!(),
                }
            }
            (0..4)
                .map(|k| atoms[k].z.sub(z_ref[k]).abs())
                .fold(0.0_f64, f64::max)
        };

        for topo in [
            MobiusTopology::Cascade,
            MobiusTopology::Bloom,
            MobiusTopology::Star,
            MobiusTopology::Swarm,
        ] {
            let mut z_ref = [C64::ZERO; 4];
            for i in 0..n_ref {
                let t0 = i as f64 * dt_ref;
                rk4_step_system(&mut z_ref, t0, dt_ref, |t, z| {
                    let v = v_at(t);
                    match topo {
                        MobiusTopology::Cascade => {
                            let v0 = v;
                            let v1 = v0.sub(z[0]);
                            let v2 = v1.sub(z[1]);
                            let v3 = v2.sub(z[2]);
                            [
                                atom_rhs(z[0], v0, p4[0]),
                                atom_rhs(z[1], v1, p4[1]),
                                atom_rhs(z[2], v2, p4[2]),
                                atom_rhs(z[3], v3, p4[3]),
                            ]
                        }
                        MobiusTopology::Bloom => {
                            let mut out = [C64::ZERO; 4];
                            let mut vc = v;
                            for k in 0..4 {
                                out[k] = atom_rhs(z[k], vc, p4[k]);
                                let w = z[k].sub(vc);
                                vc = w.scale(0.9 * w.abs());
                            }
                            out
                        }
                        MobiusTopology::Star => {
                            let c = 0.72;
                            let mean = z[1].add(z[2]).add(z[3]).scale(1.0 / 3.0);
                            let v0 = v.scale(1.0 - c * c).add(mean.scale(c));
                            let vb = z[0].scale(c);
                            [
                                atom_rhs(z[0], v0, p4[0]),
                                atom_rhs(z[1], vb, p4[1]),
                                atom_rhs(z[2], vb, p4[2]),
                                atom_rhs(z[3], vb, p4[3]),
                            ]
                        }
                        MobiusTopology::Swarm => {
                            let c = 0.72;
                            let sum = z[0].add(z[1]).add(z[2]).add(z[3]);
                            let mut out = [C64::ZERO; 4];
                            for k in 0..4 {
                                let others = sum.sub(z[k]).scale(1.0 / 3.0);
                                let vk = v.scale(1.0 - c).add(others.scale(c));
                                out[k] = atom_rhs(z[k], vk, p4[k]);
                            }
                            out
                        }
                        _ => unreachable!(),
                    }
                });
            }

            let e24 = run_err(topo, 24_000.0, &z_ref);
            let e48 = run_err(topo, 48_000.0, &z_ref);
            let e96 = run_err(topo, 96_000.0, &z_ref);
            let ratio_a = e24 / e48;
            let ratio_b = e48 / e96;
            assert!(
                (3.85..=4.20).contains(&ratio_a) && (3.92..=4.08).contains(&ratio_b),
                "{topo:?} must converge at O(dt^2): e24={e24:.3e}, e48={e48:.3e}, e96={e96:.3e}, ratios=({ratio_a:.3}, {ratio_b:.3})"
            );
            assert!(
                e48 < 1e-3,
                "{topo:?} 48kHz error {e48:.3e} unexpectedly large"
            );
        }
    }

    #[test]
    fn four_atom_cascade_conservation_holds_to_machine_precision() {
        let mut atoms = [CubicAtom::default(); MOBIUS_MAX_ATOMS];
        let mut params = [AtomParams::default(); MOBIUS_MAX_ATOMS];
        params[0] = AtomParams::from_invariants(220.0, 0.25, 1.5, 1.0);
        params[1] = AtomParams::from_invariants(660.0, 0.16, -1.0, 1.0);
        params[2] = AtomParams::from_invariants(1100.0, 0.11, 2.0, 1.0);
        params[3] = AtomParams::from_invariants(2400.0, 0.08, 0.5, 1.0);
        let dt = 1.0 / 48_000.0;
        let mut max_err = 0.0_f64;

        for i in 0..4000 {
            let v_in = eval_dipole(TAU * 220.0 * i as f64 * dt, 0.78, PI * 0.45);
            let (z_mid, v_rem) = step_cascade(&mut atoms, &params, 4, v_in, 1.0, dt);
            let total = z_mid[0]
                .add(z_mid[1])
                .add(z_mid[2])
                .add(z_mid[3])
                .add(v_rem);
            max_err = max_err.max(total.sub(v_in).abs());
        }
        assert!(
            max_err <= 1e-14,
            "4-atom cascade conservation error {max_err:.3e} exceeded 1e-14"
        );
    }

    #[test]
    fn star_invariant_ray_matches_analytical_slow_manifold() {
        // Two-atom Star (A0 <-> A1) with asymmetric stiffness gamma = g_B / g_A < 1:
        // Solve sqrt(gamma) * (lambda - c) = sqrt(lambda) * (1 - c * lambda) analytically.
        let c = 0.85_f64;
        let g_a = 1200.0_f64;
        let g_b = 120.0_f64;
        let gamma = g_b / g_a;
        let mut lo = c;
        let mut hi = 1.0 / c;
        for _ in 0..80 {
            let mid = 0.5 * (lo + hi);
            let f = gamma.sqrt() * (mid - c) - mid.sqrt() * (1.0 - c * mid);
            if f < 0.0 {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let lambda_star = 0.5 * (lo + hi);

        let mut atoms = [CubicAtom::default(); MOBIUS_MAX_ATOMS];
        atoms[0].z = C64::new(0.9, 0.0);
        atoms[1].z = C64::new(0.4, 0.0);
        let mut params = [AtomParams::default(); MOBIUS_MAX_ATOMS];
        let omega = TAU * 220.0;
        params[0] = AtomParams {
            omega,
            g: g_a,
            beta: 0.0,
        };
        params[1] = AtomParams {
            omega,
            g: g_b,
            beta: 0.0,
        };
        let dt = 1.0 / 48_000.0;
        for _ in 0..96_000 {
            step_star(&mut atoms, &params, 2, C64::ZERO, c, dt);
        }
        let ratio = atoms[1].z.abs() / atoms[0].z.abs();
        let rel_err = (ratio - lambda_star).abs() / lambda_star;
        assert!(
            rel_err < 1e-3,
            "Star invariant-ray ratio {ratio:.6} deviated from analytical {lambda_star:.6} (rel={rel_err:.4})"
        );
    }

    #[test]
    fn delay_coupled_networks_are_unconditionally_non_expansive() {
        let sr = 48_000.0;
        let dt = 1.0 / sr;
        let mut params = [AtomParams::default(); MOBIUS_MAX_ATOMS];
        params[0] = AtomParams {
            omega: TAU * 220.0,
            g: 2500.0,
            beta: semitones_to_beta(6.0),
        };
        params[1] = AtomParams {
            omega: TAU * 311.13,
            g: 2500.0,
            beta: semitones_to_beta(-6.0),
        };
        let d_samp = [0.0137 * sr, 0.0211 * sr, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

        for topo in [
            MobiusTopology::Star,
            MobiusTopology::Ring,
            MobiusTopology::Swarm,
        ] {
            let mut atoms = [CubicAtom::default(); MOBIUS_MAX_ATOMS];
            atoms[0].z = C64::from_polar(1.0, 0.3);
            atoms[1].z = C64::from_polar(1.0, 1.7);
            let mut delays = MobiusDelayBank::new(sr);
            delays.partition(2);
            // Pre-fill delay lines at unit amplitude so worst-case delayed state hits immediately.
            for _ in 0..1200 {
                delays.push_atom(0, atoms[0].z);
                delays.push_atom(1, atoms[1].z);
            }
            let mut max_norm = 1.0_f64;
            for _ in 0..4800 {
                step_delayed(
                    topo,
                    &mut atoms,
                    &params,
                    &mut delays,
                    &d_samp,
                    2,
                    C64::ZERO,
                    1.0,
                    dt,
                );
                max_norm = max_norm.max(atoms[0].z.abs()).max(atoms[1].z.abs());
            }
            assert!(
                max_norm <= 1.0 + 1e-12,
                "{topo:?} delay network expanded above unit ball: max_norm={max_norm:.15}"
            );
        }
    }

    #[test]
    fn high_note_a6_disk_sources_meet_nyquist_alias_budget() {
        let sr = 48_000.0;
        let f0 = 1760.0; // A6
        let n = 8192;

        // Verify raw disk sources stay <= -80 dB at Nyquist and post-atom stays <= -88 dB.
        for exciter in [
            MobiusExciter::Szego,
            MobiusExciter::Dipole,
            MobiusExciter::Poisson,
        ] {
            let mut raw_sig = Vec::with_capacity(n);
            let mut synth = MobiusSynth::new(sr);
            let p = MobiusSynthParams {
                exciter,
                base_hz: f0,
                brightness: 1.0,
                // For Poisson, select exact 3x formant (`k = 1 + 7 * 2/7 = 3.0`) so all true
                // sidebands sit on integer multiples of `f0` and folded Nyquist aliases land cleanly
                // at `sr - n*f0` (non-integer multiples of `f0`).
                shape: if exciter == MobiusExciter::Poisson {
                    2.0 / 7.0
                } else {
                    0.5
                },
                chirality: 0.0,
                attack_ms: 1.0,
                decay_ms: 200.0,
                sustain: 1.0,
                atoms: 1,
                stiffness: 0.25,
                kerr_semitones: 0.0,
                ..MobiusSynthParams::default()
            };
            synth.trigger(1.0, &p, false, true);

            let k_form = 1.0 + 7.0 * p.shape;
            let r_raw = match exciter {
                MobiusExciter::Szego => r_max_szego(f0, sr),
                MobiusExciter::Dipole => r_max_dipole(f0, sr),
                MobiusExciter::Poisson => r_max_poisson(f0, f0 * k_form, sr),
                _ => 0.0,
            };
            let mut post_sig = Vec::with_capacity(n);
            // Settle transient first.
            for _ in 0..4096 {
                let _ = synth.process_stereo([0.0, 0.0], 1.0);
            }
            for i in 0..n {
                let th = (TAU * f0 * i as f64 / sr).rem_euclid(TAU);
                let v = match exciter {
                    MobiusExciter::Szego => eval_szego(th, r_raw),
                    MobiusExciter::Dipole => eval_dipole(th, r_raw, PI * (1.0 - p.shape)),
                    MobiusExciter::Poisson => {
                        eval_poisson(th, (th * k_form).rem_euclid(TAU), r_raw)
                    }
                    _ => C64::ZERO,
                };
                raw_sig.push(v.re);
                let out = synth.process_stereo([0.0, 0.0], 1.0);
                post_sig.push(0.5 * (out[0] + out[1]));
            }

            // Probe inharmonic inter-harmonic alias bins near 18..23.5 kHz.
            let raw_ref =
                goertzel_mag(&raw_sig, sr, f0).max(goertzel_mag(&raw_sig, sr, f0 * k_form));
            let post_ref =
                goertzel_mag(&post_sig, sr, f0).max(goertzel_mag(&post_sig, sr, f0 * k_form));
            let mut raw_alias = 1e-18_f64;
            let mut post_alias = 1e-18_f64;
            for h in 10..=13 {
                // Midpoint between integer harmonics catches folded Nyquist spurs cleanly.
                let f_probe = (h as f64 + 0.37) * f0;
                if f_probe < 0.49 * sr {
                    raw_alias = raw_alias.max(goertzel_mag(&raw_sig, sr, f_probe));
                    post_alias = post_alias.max(goertzel_mag(&post_sig, sr, f_probe));
                }
            }
            let raw_db = 20.0 * (raw_alias / raw_ref.max(1e-18)).log10();
            let post_db = 20.0 * (post_alias / post_ref.max(1e-18)).log10();
            assert!(
                raw_db <= -78.0,
                "{exciter:?} A6 raw alias spur {raw_db:.1} dB exceeded -78 dB"
            );
            assert!(
                post_db <= -88.0,
                "{exciter:?} A6 post-atom alias spur {post_db:.1} dB exceeded -88 dB"
            );
        }
    }

    #[test]
    fn quadrature_stereo_has_exact_zero_db_mono_comb_error() {
        for deg in [0.0_f64, 30.0, 60.0, 80.0, 90.0] {
            let theta = deg.to_radians();
            let cos_half = (0.5 * theta).cos();
            let mut max_err = 0.0_f64;
            for h in 1..=20 {
                let z = C64::from_polar(1.0, 0.13 * h as f64);
                let [l, r] = stereo_quad(z, theta);
                let mono = 0.5 * (l + r);
                let expected = cos_half * z.re;
                max_err = max_err.max((mono - expected).abs());
            }
            assert!(
                max_err <= 1e-15,
                "Quadrature mono-sum comb error at {deg} deg was {max_err:.3e}"
            );
        }
    }

    #[test]
    fn pitch_tracking_keeps_rms_invariant_across_c2_to_c6() {
        let sr = 48_000.0;
        let freqs = [65.406, 130.813, 261.626, 523.251, 1046.502]; // C2..C6
        let mut rms_db = Vec::new();
        for &f0 in &freqs {
            let mut synth = MobiusSynth::new(sr);
            let p = MobiusSynthParams {
                exciter: MobiusExciter::Szego,
                base_hz: f0,
                brightness: 0.70,
                chirality: 0.0,
                attack_ms: 2.0,
                decay_ms: 100.0,
                sustain: 1.0,
                atoms: 2,
                stiffness: 0.25,
                kerr_semitones: 1.5,
                key_track: 1.0,
                topology: MobiusTopology::Cascade,
                ..MobiusSynthParams::default()
            };
            synth.trigger(0.85, &p, false, true);
            for _ in 0..9600 {
                let _ = synth.process_stereo([0.0, 0.0], 1.0);
            }
            let mut sum_sq = 0.0_f64;
            let win = 9600;
            for _ in 0..win {
                let [l, r] = synth.process_stereo([0.0, 0.0], 1.0);
                sum_sq += 0.5 * (l * l + r * r);
            }
            rms_db.push(20.0 * (sum_sq / win as f64).sqrt().max(1e-12).log10());
        }
        let lo = rms_db.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = rms_db.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        assert!(
            (hi - lo) <= 0.8,
            "C2..C6 RMS spread {:.2} dB exceeded 0.8 dB: {rms_db:?}",
            hi - lo
        );
    }

    #[test]
    fn delayed_bore_feedback_stays_bounded_and_clean_above_18khz() {
        let sr = 48_000.0;
        let mut synth = MobiusSynth::new(sr);
        let p = MobiusSynthParams {
            exciter: MobiusExciter::Dipole,
            base_hz: 220.0,
            brightness: 0.60,
            shape: 0.28,
            chirality: 0.1,
            attack_ms: 10.0,
            decay_ms: 100.0,
            sustain: 1.0,
            atoms: 2,
            dispersion: MobiusDispersion::Ratio,
            structure: 1.0, // 1x, 3x
            stiffness: 0.35,
            stiffness_tilt: -1.3,
            kerr_semitones: 1.5,
            topology: MobiusTopology::Cascade,
            coupling: 1.0,
            feedback: 0.90,
            fb_lock: true,
            fb_cycles: 1.5,
            ..MobiusSynthParams::default()
        };
        synth.trigger(1.0, &p, false, true);
        let total = (1.5 * sr) as usize;
        let mut mid = Vec::with_capacity(total);
        let mut max_abs = 0.0_f64;
        for _ in 0..total {
            let [l, r] = synth.process_stereo([0.0, 0.0], 1.0);
            assert!(l.is_finite() && r.is_finite());
            max_abs = max_abs.max(l.abs()).max(r.abs());
            mid.push(0.5 * (l + r));
        }
        assert!(
            max_abs < 8.0 && max_abs > 0.05,
            "Multiphonic output amplitude out of physical range: {max_abs}"
        );
        let slice = &mid[(0.8 * sr) as usize..(0.8 * sr) as usize + 8192];
        let peak_tone = [110.0, 220.0, 330.0, 440.0]
            .iter()
            .map(|&f| goertzel_mag(slice, sr, f))
            .fold(1e-18_f64, f64::max);
        let mut max_hi = 1e-18_f64;
        for k in 0..32 {
            let f = 18_000.0 + 5_200.0 * (k as f64 / 31.0);
            max_hi = max_hi.max(goertzel_mag(slice, sr, f));
        }
        let hi_db = 20.0 * (max_hi / peak_tone).log10();
        assert!(
            hi_db < -65.0,
            "Delayed bore multiphonic near-Nyquist energy {hi_db:.1} dB exceeded -65 dB"
        );
    }

    #[test]
    fn all_exciters_dispersions_and_topologies_produce_finite_stereo_and_clean_quiet_shutdown() {
        let sr = 48_000.0;
        for &exciter in MobiusExciter::ALL {
            let mut synth = MobiusSynth::new(sr);
            let p = MobiusSynthParams {
                exciter,
                base_hz: 220.0,
                atoms: 4,
                distance_ms: 15.0,
                ..MobiusSynthParams::default()
            };
            synth.trigger(0.85, &p, false, true);
            let mut peak = 0.0_f64;
            for i in 0..1200 {
                let ext = if exciter == MobiusExciter::External {
                    let s = (TAU * 220.0 * i as f64 / sr).sin() * 0.7;
                    [s, s * 0.8]
                } else {
                    [0.0, 0.0]
                };
                let [l, r] = synth.process_stereo(ext, 1.0);
                assert!(l.is_finite() && r.is_finite());
                peak = peak.max(l.abs()).max(r.abs());
            }
            assert!(
                peak > 1e-3,
                "Exciter {exciter:?} produced silent output (peak={peak})"
            );
            let preview = mobius_target_preview(exciter, 0.72, 0.5, 0.2, 64);
            assert_eq!(preview.len(), 64);
            assert!(preview.iter().all(|s| s.is_finite() && s.abs() <= 1.0001));
        }

        for &dispersion in MobiusDispersion::ALL {
            for &topology in MobiusTopology::ALL {
                let mut synth = MobiusSynth::new(sr);
                let p = MobiusSynthParams {
                    exciter: MobiusExciter::Dipole,
                    base_hz: 220.0,
                    atoms: 4,
                    dispersion,
                    topology,
                    distance_ms: 12.0,
                    ..MobiusSynthParams::default()
                };
                synth.trigger(0.8, &p, false, true);
                let mut peak = 0.0_f64;
                for _ in 0..800 {
                    let [l, r] = synth.process_stereo([0.0, 0.0], 1.0);
                    assert!(l.is_finite() && r.is_finite());
                    peak = peak.max(l.abs()).max(r.abs());
                }
                assert!(
                    peak > 1e-3,
                    "({dispersion:?}, {topology:?}) produced silent output"
                );
                // Choke / host fade must bring voice cleanly to quiet without NaN.
                synth.begin_host_fade(decay_coeff_60db_ms(3.5, sr));
                for _ in 0..2000 {
                    let [l, r] = synth.process_stereo([0.0, 0.0], 1.0);
                    assert!(l.is_finite() && r.is_finite());
                }
                assert!(
                    synth.is_quiet(),
                    "({dispersion:?}, {topology:?}) failed to settle quiet after host fade"
                );
            }
        }
    }
}
