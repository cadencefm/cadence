//! Modal acoustic resonator bank with 1-body exact joint free flows, 2-body
//! mean-preserving reciprocal pair coupling, and 3-body lossless rational SO(3)
//! Cayley-Rodrigues resonant triads.
//!
//! All real-time stepping paths are zero-allocation, lock-free, and panic-free.

use core::f64::consts::PI;
use std::sync::OnceLock;

use super::math::{
    atomic_pair_scalar, cayley_rotation, exprel_neg, joint_free_kerr, sanitize_sample_rate,
    MAX_ATOMS, TAU,
};

/// Maximum number of precomputed 3-wave resonant triads stored in a modal bank.
pub const MAX_TRIADS: usize = MAX_ATOMS * 3;

/// Minimum base fundamental frequency (Hz) for [`ModalBank`].
pub const MODAL_MIN_HZ: f32 = 20.0;
/// Maximum base fundamental frequency (Hz) for [`ModalBank`].
pub const MODAL_MAX_HZ: f32 = 12_000.0;
/// Squared amplitude energy floor below which an unexcited [`ModalBank`] tail is quiet
/// (`sqrt(1e-14) = 1e-7`, aligned with `TAIL_SILENCE_FLOOR_F64`).
pub const MODAL_SILENCE_ENERGY: f64 = 1e-14;

/// Analytical mode frequency and normalized radiation weight (shared by DSP & UI visualizers).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ModalModeDescriptor {
    /// Resonant frequency of this mode (Hz).
    pub freq_hz: f64,
    /// Peak-normalized visual amplitude weight in `[0.0, 1.0]`.
    pub gain: f64,
    /// True when this mode sits below the active Nyquist safety ceiling (`0.44 * sr`).
    pub active: bool,
}

/// Pure SPOT computation of active physical modal frequencies and peak-normalized visual
/// weights for [`ModalBank`], matching [`ModalBank::configure`]'s exact eigenfrequency ratios,
/// `0.44 * sr` Nyquist ceiling, and brightness tilt exponent `-1.45 + 2.55 * brightness`.
pub fn modal_mode_descriptors(
    geometry: BodyGeometry,
    atoms: usize,
    base_hz: f64,
    brightness: f64,
    absorption: f64,
    radiation: f64,
    sample_rate: f64,
) -> ([ModalModeDescriptor; MAX_ATOMS], usize) {
    let sr = sanitize_sample_rate(sample_rate);
    let f_base =
        sanitize_fundamental(base_hz, sr).clamp(f64::from(MODAL_MIN_HZ), f64::from(MODAL_MAX_HZ));
    let req_n = atoms.clamp(1, MAX_ATOMS);
    let mut n = req_n;
    while n > 1 && f_base * body_ratio(geometry, n - 1, req_n) > 0.44 * sr {
        n -= 1;
    }

    let bright = finite_or(brightness, 0.5).clamp(0.0, 1.0);
    let absorb = finite_or(absorption, 0.035).clamp(0.0, 0.5);
    let rad = finite_or(radiation, 0.12).clamp(0.0, 1.0);
    let tilt_exp = -1.45 + 2.55 * bright;

    let mut out = [ModalModeDescriptor::default(); MAX_ATOMS];
    let mut max_amp = 1e-9_f64;
    for (k, slot) in out.iter_mut().take(n).enumerate() {
        let ratio = body_ratio(geometry, k, req_n);
        let freq_hz = (f_base * ratio).clamp(f64::from(MODAL_MIN_HZ), 20_000.0);
        let rad_w = ratio.powf(tilt_exp);
        let damp = 1.0 + (0.35 + 2.2 * absorb) * 0.12 * (k as f64) + 0.12 * rad * ratio.powf(0.32);
        let amp = (rad_w / damp).max(0.0);
        max_amp = max_amp.max(amp);
        *slot = ModalModeDescriptor {
            freq_hz,
            gain: amp,
            active: true,
        };
    }
    for slot in out.iter_mut().take(n) {
        slot.gain = (slot.gain / max_amp).clamp(0.0, 1.0);
    }
    (out, n)
}

/// Circular membrane eigenfrequency ratios (Bessel `J_m` zero ratios, first 16 modes).
pub const BESSEL_RATIOS: [f64; 16] = [
    1.0, 1.593, 2.136, 2.296, 2.653, 2.917, 3.156, 3.501, 3.600, 3.890, 4.150, 4.490, 4.810, 5.120,
    5.440, 5.790,
];

/// Inharmonic metal bar eigenfrequency ratios (first 16 modes).
pub const BAR_RATIOS: [f64; 16] = [
    1.0, 1.414, 1.980, 2.756, 3.420, 4.190, 5.140, 5.930, 6.840, 7.690, 8.930, 9.810, 10.94, 12.11,
    13.42, 14.85,
];

/// Close-doublet harmonic cluster eigenfrequency ratios (first 16 modes).
pub const CLUSTER_RATIOS: [f64; 16] = [
    1.0, 1.005, 0.996, 1.498, 2.006, 1.994, 2.997, 3.008, 4.012, 3.989, 5.010, 6.020, 7.010, 8.030,
    9.010, 10.02,
];

/// Modal eigenfrequency family for an acoustic resonator body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyGeometry {
    /// Open cylindrical / conical harmonic series (`1, 2, 3, ...`).
    Harmonic,
    /// Stopped pipe odd harmonic series (`1, 3, 5, ...`).
    Stopped,
    /// Circular membrane Bessel roots with linear continuation.
    Membrane,
    /// Free-free inharmonic metallic bar with power-law continuation.
    Bar,
    /// 2D circular Kirchhoff plate Chladni spectrum strided across the bank.
    Plate,
    /// Near-harmonic doublet cluster spectrum.
    Cluster,
}

/// Graph topology for 2-body reciprocal pair exchange and driven port sweeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairTopology {
    /// Fundamental hub connected to every upper mode.
    Star,
    /// Linear nearest-neighbor chain `(k - 1, k)`.
    Cascade,
    /// Amplitude-modulated star hub with midpoint predictor symmetry.
    Bloom,
    /// Closed nearest-neighbor ring `(k - 1, k)` plus `(n - 1, 0)`.
    Ring,
    /// Circulant small-world graph combining ring, stride `n / 3`, and antipodal chords.
    Swarm,
}

/// Diagonal axis weighting `D_t = diag(d_a, d_b, d_c)` for 3-body gyroscopic triads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriadAxis {
    /// Unweighted cyclic difference channel: `D_t = diag(1, 1, 1)`.
    Diff,
    /// Direct sum-frequency channel: `D_t = diag(1, -1, 0)`.
    Sum,
    /// Frequency-weighted Manley-Rowe channel: `D_t = diag(omega_b / w_mean, -omega_a / w_mean, 0)`.
    Manley,
}

/// Sorted 2D circular Kirchhoff plate Chladni modal ratio table, normalized to unit fundamental.
pub fn plate_candidates() -> &'static [f64] {
    static PLATE_CANDIDATES: OnceLock<Vec<f64>> = OnceLock::new();
    PLATE_CANDIDATES.get_or_init(|| {
        let mut cand = Vec::with_capacity(1400);
        for n in 0..20usize {
            let nf = n as f64;
            for m in 0..36usize {
                if (m == 0 && n == 0) || (m == 1 && n == 0) {
                    continue;
                }
                let mf = m as f64;
                let idx = mf + 2.0 * nf;
                let dome = 1.0 + 0.30 / (1.0 + 0.25 * mf * mf + 0.5 * nf * nf);
                let r = (idx / 2.0).powf(1.85) * dome;
                if r > 54.0 {
                    continue;
                }
                if m == 0 {
                    cand.push(r);
                } else {
                    let sp = (0.0042 * (13.7 * mf + 5.3 * nf).sin()).abs();
                    cand.push(r * (1.0 - sp));
                    cand.push(r * (1.0 + sp));
                }
            }
        }
        cand.sort_by(f64::total_cmp);
        let r0 = cand.first().copied().filter(|&v| v > 0.0).unwrap_or(1.0);
        for v in &mut cand {
            *v /= r0;
        }
        cand
    })
}

/// Pure modal frequency ratio lookup for mode index `k` in a bank of `n` modes.
#[inline]
pub fn body_ratio(geometry: BodyGeometry, k: usize, n: usize) -> f64 {
    match geometry {
        BodyGeometry::Harmonic => (k + 1) as f64,
        BodyGeometry::Stopped => (2 * k + 1) as f64,
        BodyGeometry::Plate => {
            let table = plate_candidates();
            let len = table.len();
            if len == 0 {
                return 1.0;
            }
            let denom = if n > 1 { n - 1 } else { 1 };
            let idx = k.saturating_mul(len - 1) / denom;
            table[idx.min(len - 1)]
        }
        BodyGeometry::Membrane => {
            if k < BESSEL_RATIOS.len() {
                BESSEL_RATIOS[k]
            } else {
                5.790 + 0.35 * (k - 15) as f64
            }
        }
        BodyGeometry::Bar => {
            if k < BAR_RATIOS.len() {
                BAR_RATIOS[k]
            } else {
                14.85 * ((k as f64 + 1.5) / 16.5).powf(1.85)
            }
        }
        BodyGeometry::Cluster => {
            if k < CLUSTER_RATIOS.len() {
                CLUSTER_RATIOS[k]
            } else {
                (k - 5) as f64 + 0.012 * (k as f64 * 2.1).sin()
            }
        }
    }
}

/// Fixed-capacity ordered Zakharov resonant triads `(a, b, c)` (`omega_a < omega_b < omega_c`)
/// with precomputed Manley-Rowe axis weights `da = omega_b / omega_mean`,
/// `db = -omega_a / omega_mean`.
#[derive(Clone, Debug)]
pub struct ResonantTriads {
    pub triplets: [[usize; 3]; MAX_TRIADS],
    pub da: [f64; MAX_TRIADS],
    pub db: [f64; MAX_TRIADS],
    pub len: usize,
}

impl ResonantTriads {
    /// Construct an empty triad table with zero active entries.
    pub const fn empty() -> Self {
        Self {
            triplets: [[0; 3]; MAX_TRIADS],
            da: [0.0; MAX_TRIADS],
            db: [0.0; MAX_TRIADS],
            len: 0,
        }
    }

    /// Number of active resonant triads.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` when no resonant triads are stored.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Default for ResonantTriads {
    fn default() -> Self {
        Self::empty()
    }
}

/// Populate `out` in place with up to `n_per` strictly ordered unique resonant triads per upper
/// mode, minimizing relative detuning `|omega_a + omega_b - omega_c| / omega_c`.
///
/// Zero heap allocations. Detunings `<= 1e-12` snap to exact `0.0` before sorting, and ties break
/// deterministically by lowest lower-mode index `i` so triad selection is strictly invariant under
/// uniform frequency scaling.
pub(crate) fn build_resonant_triads_into(
    omegas: &[f64],
    n_per: usize,
    max_detuning: f64,
    out: &mut ResonantTriads,
) {
    out.len = 0;
    let n = omegas.len().min(MAX_ATOMS);
    if n < 3 || n_per == 0 {
        return;
    }

    let mut order = [0usize; MAX_ATOMS];
    let mut sum = 0.0;
    for i in 0..n {
        order[i] = i;
        sum += omegas[i];
    }
    let f_mean = (sum / n as f64).max(1e-30);
    order[..n].sort_unstable_by(|&a, &b| omegas[a].total_cmp(&omegas[b]).then_with(|| a.cmp(&b)));

    let mut f = [0.0f64; MAX_ATOMS];
    for i in 0..n {
        f[i] = omegas[order[i]];
    }

    let mut cands = [(0.0f64, 0usize, 0usize); 2 * MAX_ATOMS];
    let mut seen = [(0usize, 0usize, 0usize); MAX_TRIADS];
    let mut seen_len = 0usize;

    for j in 2..n {
        let fj = f[j];
        if !fj.is_finite() || fj <= 0.0 {
            continue;
        }
        let mut cand_len = 0usize;
        for i in 0..(j - 1) {
            let target = fj - f[i];
            if target < f[0] * 0.45 {
                continue;
            }
            let mut lo = i + 1;
            let mut hi = j - 1;
            while lo < hi {
                let mid = (lo + hi) >> 1;
                if f[mid] < target {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            if lo > i + 1 && lo - 1 < j {
                let raw_det = (f[i] + f[lo - 1] - fj).abs() / fj;
                let det = if raw_det <= 1e-12 { 0.0 } else { raw_det };
                if det <= max_detuning {
                    cands[cand_len] = (det, i, lo - 1);
                    cand_len += 1;
                }
            }
            if lo > i && lo < j {
                let raw_det = (f[i] + f[lo] - fj).abs() / fj;
                let det = if raw_det <= 1e-12 { 0.0 } else { raw_det };
                if det <= max_detuning {
                    cands[cand_len] = (det, i, lo);
                    cand_len += 1;
                }
            }
        }

        cands[..cand_len].sort_unstable_by(|a, b| {
            a.0.total_cmp(&b.0)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.cmp(&b.2))
        });

        let mut added = 0usize;
        for &(_, ci, cb) in &cands[..cand_len] {
            if added >= n_per || out.len >= MAX_TRIADS {
                break;
            }
            let a = order[ci];
            let b = order[cb];
            let c = order[j];
            let mut key = [a, b, c];
            key.sort_unstable();
            let k = (key[0], key[1], key[2]);
            if !seen[..seen_len].contains(&k) {
                seen[seen_len] = k;
                seen_len += 1;
                let idx = out.len;
                out.triplets[idx] = [a, b, c];
                out.da[idx] = omegas[b] / f_mean;
                out.db[idx] = -omegas[a] / f_mean;
                out.len += 1;
                added += 1;
            }
        }
    }
}

/// Build ordered Zakharov three-wave resonant triads for modal angular frequencies `omegas`.
///
/// Selects up to `n_per` unique strictly ordered triads `(a, b, c)` per upper mode `c` with
/// `omega_a < omega_b < omega_c` whose relative resonance detuning
/// `|omega_a + omega_b - omega_c| / omega_c` does not exceed `max_detuning`.
///
/// Zero heap allocations and strictly invariant under uniform scaling of `omegas`.
pub fn build_resonant_triads(omegas: &[f64], n_per: usize, max_detuning: f64) -> ResonantTriads {
    let mut out = ResonantTriads::empty();
    build_resonant_triads_into(omegas, n_per, max_detuning, &mut out);
    out
}

/// Exact single-pass rational SO(3) Cayley-Rodrigues weighted-axis triad pass integrating
/// `dx_t/dt = 0`, `dy_t/dt = gamma * ((D_t * x_t) x y_t)` via
/// `R(w) = (I - [w]_x)^(-1) (I + [w]_x)`, `w = half_gamma_h * (D_t * x_t)`.
///
/// Strictly orthogonal (`||y^+|| == ||y||`), energy-conserving, time-reversible, and
/// free of trigonometric or square-root evaluations.
#[inline]
pub fn triad_rodrigues_pass(
    x: &[f64],
    y: &mut [f64],
    triads: &ResonantTriads,
    half_gamma_h: f64,
    axis: TriadAxis,
    reverse: bool,
) {
    let count = triads.len.min(MAX_TRIADS);
    let m = x.len().min(y.len());
    if count == 0 || half_gamma_h == 0.0 || m < 3 {
        return;
    }
    match axis {
        TriadAxis::Sum => {
            for idx in 0..count {
                let p = if reverse { count - 1 - idx } else { idx };
                let [i, j, k] = triads.triplets[p];
                if i >= m || j >= m || k >= m {
                    continue;
                }
                let wx = half_gamma_h * x[i];
                let wy = -half_gamma_h * x[j];
                let yi = y[i];
                let yj = y[j];
                let yk = y[k];
                let cx = wy * yk;
                let cy = -wx * yk;
                let cz = wx * yj - wy * yi;
                let w2 = wx * wx + wy * wy;
                let k2 = 2.0 / (1.0 + w2);
                y[i] = yi + k2 * (cx + wy * cz);
                y[j] = yj + k2 * (cy - wx * cz);
                y[k] = yk + k2 * (cz - w2 * yk);
            }
        }
        TriadAxis::Manley => {
            for idx in 0..count {
                let p = if reverse { count - 1 - idx } else { idx };
                let [i, j, k] = triads.triplets[p];
                if i >= m || j >= m || k >= m {
                    continue;
                }
                let wx = half_gamma_h * triads.da[p] * x[i];
                let wy = half_gamma_h * triads.db[p] * x[j];
                let yi = y[i];
                let yj = y[j];
                let yk = y[k];
                let cx = wy * yk;
                let cy = -wx * yk;
                let cz = wx * yj - wy * yi;
                let w2 = wx * wx + wy * wy;
                let k2 = 2.0 / (1.0 + w2);
                y[i] = yi + k2 * (cx + wy * cz);
                y[j] = yj + k2 * (cy - wx * cz);
                y[k] = yk + k2 * (cz - w2 * yk);
            }
        }
        TriadAxis::Diff => {
            for idx in 0..count {
                let p = if reverse { count - 1 - idx } else { idx };
                let [i, j, k] = triads.triplets[p];
                if i >= m || j >= m || k >= m {
                    continue;
                }
                let wx = half_gamma_h * x[i];
                let wy = half_gamma_h * x[j];
                let wz = half_gamma_h * x[k];
                let yi = y[i];
                let yj = y[j];
                let yk = y[k];
                let cx = wy * yk - wz * yj;
                let cy = wz * yi - wx * yk;
                let cz = wx * yj - wy * yi;
                let k2 = 2.0 / (1.0 + wx * wx + wy * wy + wz * wz);
                y[i] = yi + k2 * (cx + wy * cz - wz * cy);
                y[j] = yj + k2 * (cy + wz * cx - wx * cz);
                y[k] = yk + k2 * (cz + wx * cy - wy * cx);
            }
        }
    }
}

/// 2-body symmetric, mean-preserving, strictly dissipative reciprocal pair contraction
/// between complex atoms `i` and `j`.
///
/// Strictly conserves the complex mean `(z_i + z_j) / 2` while contracting the difference
/// `z_i - z_j` through the bare-cubic (`headroom <= 0`) or opposing-biased ambient-air
/// (`headroom > 0`) flow.
#[inline]
pub fn pair_exchange(x: &mut [f64], y: &mut [f64], i: usize, j: usize, eta: f64, headroom: f64) {
    let m = x.len().min(y.len());
    if i >= m || j >= m || i == j || eta <= 0.0 {
        return;
    }
    let dx = x[i] - x[j];
    let dy = y[i] - y[j];
    let a = (dx * dx + dy * dy).sqrt();
    if a == 0.0 {
        return;
    }
    let f = if headroom <= 0.0 {
        let u = 2.0 * eta * a;
        0.5 * (u / (1.0 + u))
    } else {
        let eta_eff = eta / (1.0 + 2.0 * headroom);
        if a <= headroom {
            -0.5 * (-4.0 * eta_eff * headroom).exp_m1()
        } else {
            let a_new = atomic_pair_scalar(a, 0.0, headroom, 2.0 * eta_eff);
            0.5 * (1.0 - a_new / a)
        }
    };
    x[i] -= f * dx;
    y[i] -= f * dy;
    x[j] += f * dx;
    y[j] += f * dy;
}

/// Precomputed per-mode coefficients for a single joint free flow evaluation.
#[derive(Clone, Copy)]
pub(crate) struct FreeStepCoeffs {
    pub omega: f64,
    pub beta: f64,
    pub g: f64,
    pub lam: f64,
    pub patm: f64,
    pub dt: f64,
    pub exp_l: f64,
    pub t_eff: f64,
    pub rot_c: f64,
    pub rot_s: f64,
}

/// Advance one complex modal state through the exact joint free flow.
///
/// Uses the precomputed rotation and exponential-integral factors on the interior linear-Kerr-off
/// path (`beta == 0` and `|z| <= patm`), falling back to the full exact `joint_free_kerr` flow
/// whenever Kerr dispersion is active or amplitude exceeds ambient headroom.
#[inline]
pub(crate) fn joint_free_mode(xr: f64, xi: f64, c: FreeStepCoeffs) -> (f64, f64) {
    let r2 = xr * xr + xi * xi;
    if r2 == 0.0 {
        return (0.0, 0.0);
    }
    if c.beta == 0.0 && (c.patm <= 0.0 || r2 <= c.patm * c.patm) {
        let a0 = r2.sqrt();
        let rad_scale = c.exp_l / (1.0 + c.g * a0 * c.t_eff);
        (
            (xr * c.rot_c - xi * c.rot_s) * rad_scale,
            (xr * c.rot_s + xi * c.rot_c) * rad_scale,
        )
    } else {
        joint_free_kerr(xr, xi, c.omega, c.beta, c.g, c.lam, c.patm, c.dt)
    }
}

/// Graph parameters for a 2-body reciprocal pair coupling pass.
#[derive(Clone, Copy)]
pub(crate) struct CoupleParams {
    pub n: usize,
    pub topology: PairTopology,
    pub passive: f64,
    pub headroom: f64,
}

/// 2-body reciprocal pair coupling pass over all five graph topologies.
///
/// For [`PairTopology::Bloom`], evaluates the state-dependent hub rate using a half-step midpoint
/// hub predictor `|z_0(h/2)|` so forward/reverse composition remains `O(h^2)` time-symmetric
/// alongside exact pair-mean conservation and strict passivity.
#[inline]
pub(crate) fn couple_pairs(
    x: &mut [f64],
    y: &mut [f64],
    params: CoupleParams,
    reverse: bool,
    scale: f64,
) {
    let n = params.n.min(x.len()).min(y.len());
    let e = params.passive * scale;
    if n < 2 || e <= 0.0 {
        return;
    }
    let headroom = params.headroom;

    match params.topology {
        PairTopology::Star | PairTopology::Bloom => {
            let w = if params.topology == PairTopology::Bloom {
                let r0 = (x[0] * x[0] + y[0] * y[0]).sqrt();
                let w0 = (e * (0.25 + 2.0 * r0)) / (n - 1) as f64;
                let mut mx0 = x[0];
                let mut my0 = y[0];
                for i in 1..n {
                    let dx = x[0] - x[i];
                    let dy = y[0] - y[i];
                    let a = (dx * dx + dy * dy).sqrt();
                    if a > 0.0 {
                        let f_half = if headroom <= 0.0 {
                            let u = w0 * a;
                            0.5 * (u / (1.0 + u))
                        } else {
                            let eta_eff = (0.5 * w0) / (1.0 + 2.0 * headroom);
                            if a <= headroom {
                                -0.5 * (-4.0 * eta_eff * headroom).exp_m1()
                            } else {
                                0.5 * (1.0
                                    - atomic_pair_scalar(a, 0.0, headroom, 2.0 * eta_eff) / a)
                            }
                        };
                        mx0 -= f_half * dx;
                        my0 -= f_half * dy;
                    }
                }
                let r_mid = (mx0 * mx0 + my0 * my0).sqrt();
                (e * (0.25 + 2.0 * r_mid)) / (n - 1) as f64
            } else {
                e / (n - 1) as f64
            };
            if !reverse {
                for i in 1..n {
                    pair_exchange(x, y, 0, i, w, headroom);
                }
            } else {
                for i in (1..n).rev() {
                    pair_exchange(x, y, 0, i, w, headroom);
                }
            }
        }
        PairTopology::Swarm => {
            if n == 2 {
                pair_exchange(x, y, 0, 1, e, headroom);
                return;
            }
            let s2 = n / 3;
            let s3 = n / 2;
            let has_s2 = s2 >= 2 && s2 != s3;
            let has_s3 = s3 >= 2;
            let deg = 1.0 + if has_s2 { 1.0 } else { 0.0 } + if has_s3 { 0.5 } else { 0.0 };
            let w = e / deg;
            if !reverse {
                for i in 0..n {
                    pair_exchange(x, y, i, (i + 1) % n, w, headroom);
                }
                if has_s2 {
                    for i in 0..n {
                        pair_exchange(x, y, i, (i + s2) % n, w, headroom);
                    }
                }
                if has_s3 {
                    let lim = if (n & 1) == 0 { s3 } else { n };
                    for i in 0..lim {
                        pair_exchange(x, y, i, (i + s3) % n, w, headroom);
                    }
                }
            } else {
                if has_s3 {
                    let lim = if (n & 1) == 0 { s3 } else { n };
                    for i in (0..lim).rev() {
                        pair_exchange(x, y, i, (i + s3) % n, w, headroom);
                    }
                }
                if has_s2 {
                    for i in (0..n).rev() {
                        pair_exchange(x, y, i, (i + s2) % n, w, headroom);
                    }
                }
                for i in (0..n).rev() {
                    pair_exchange(x, y, i, (i + 1) % n, w, headroom);
                }
            }
        }
        PairTopology::Cascade | PairTopology::Ring => {
            let is_ring = params.topology == PairTopology::Ring;
            if !reverse {
                for i in 1..n {
                    pair_exchange(x, y, i - 1, i, e, headroom);
                }
                if is_ring {
                    pair_exchange(x, y, n - 1, 0, e, headroom);
                }
            } else {
                if is_ring {
                    pair_exchange(x, y, n - 1, 0, e, headroom);
                }
                for i in (1..n).rev() {
                    pair_exchange(x, y, i - 1, i, e, headroom);
                }
            }
        }
    }
}

#[inline]
pub(crate) fn sanitize_fundamental(hz: f64, sample_rate: f64) -> f64 {
    let base = if hz.is_finite() && hz > 0.0 {
        hz
    } else {
        220.0
    };
    base.min(0.36 * sample_rate)
}

#[inline]
pub(crate) fn finite_or(v: f64, fallback: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        fallback
    }
}

#[inline]
fn triad_max_detuning(geometry: BodyGeometry) -> f64 {
    if geometry == BodyGeometry::Harmonic {
        1e-6
    } else {
        f64::INFINITY
    }
}

#[inline]
pub(crate) fn triad_half_gh(triad_amount: f64, w_body: f64, n: usize, h: f64) -> f64 {
    let tri_amt = finite_or(triad_amount, 0.0).max(0.0);
    if tri_amt > 0.0 && n >= 3 {
        0.5 * tri_amt * tri_amt * 14.0 * w_body * (n as f64).sqrt() * h
    } else {
        0.0
    }
}

/// Cached 3-body Zakharov resonant triads and coupling step parameters shared by
/// modal resonators and aeroacoustic bores.
#[derive(Clone, Debug)]
pub(crate) struct TriadCache {
    triads: ResonantTriads,
    valid: bool,
    geometry: BodyGeometry,
    req_n: usize,
    n: usize,
    half_gh: f64,
    axis: TriadAxis,
}

impl TriadCache {
    pub(crate) const fn empty() -> Self {
        Self {
            triads: ResonantTriads::empty(),
            valid: false,
            geometry: BodyGeometry::Harmonic,
            req_n: 0,
            n: 0,
            half_gh: 0.0,
            axis: TriadAxis::Sum,
        }
    }

    /// Rebuild cached resonant triads in place whenever geometry or mode count changes,
    /// and update the active coupling step `half_gh` and axis weighting.
    pub(crate) fn ensure(
        &mut self,
        geometry: BodyGeometry,
        req_n: usize,
        n: usize,
        omega: &[f64; MAX_ATOMS],
        half_gh: f64,
        axis: TriadAxis,
    ) {
        let n = n.min(MAX_ATOMS);
        if !self.valid || self.geometry != geometry || self.req_n != req_n || self.n != n {
            if n >= 3 {
                build_resonant_triads_into(
                    &omega[..n],
                    3,
                    triad_max_detuning(geometry),
                    &mut self.triads,
                );
            } else {
                self.triads.len = 0;
            }
            self.valid = true;
            self.geometry = geometry;
            self.req_n = req_n;
            self.n = n;
        }
        self.half_gh = half_gh;
        self.axis = axis;
    }

    /// Apply a forward or reverse lossless SO(3) Cayley-Rodrigues triad pass when active.
    #[inline]
    pub(crate) fn apply_pass(
        &self,
        x: &[f64; MAX_ATOMS],
        y: &mut [f64; MAX_ATOMS],
        n: usize,
        half_step: bool,
        reverse: bool,
    ) {
        let step_gh = if half_step {
            0.5 * self.half_gh
        } else {
            self.half_gh
        };
        if step_gh > 0.0 {
            let m = n.min(MAX_ATOMS);
            triad_rodrigues_pass(
                &x[..m],
                &mut y[..m],
                &self.triads,
                step_gh,
                self.axis,
                reverse,
            );
        }
    }
}

/// Configuration parameters for a [`ModalBank`].
#[derive(Clone, Copy, Debug)]
pub struct ModalConfig {
    pub fundamental_hz: f64,
    pub atoms: usize,
    pub geometry: BodyGeometry,
    pub pair_topology: PairTopology,
    pub coupling: f64,
    pub absorption: f64,
    pub radiation: f64,
    pub brightness: f64,
    pub headroom: f64,
    pub kerr_semitones: f64,
    pub triad_amount: f64,
    pub triad_axis: TriadAxis,
    pub spread: f64,
}

impl Default for ModalConfig {
    fn default() -> Self {
        Self {
            fundamental_hz: 220.0,
            atoms: 16,
            geometry: BodyGeometry::Harmonic,
            pair_topology: PairTopology::Star,
            coupling: 0.0,
            absorption: 0.045,
            radiation: 0.15,
            brightness: 0.5,
            headroom: 0.0,
            kerr_semitones: 0.0,
            triad_amount: 0.0,
            triad_axis: TriadAxis::Sum,
            spread: 0.35,
        }
    }
}

/// Multi-mode nonlinear acoustic resonator bank (1 to [`MAX_ATOMS`] complex modes).
///
/// Combines 1-body exact joint continuous free flows (`J_h`), 2-body reciprocal pair exchange
/// across five graph topologies, 3-body lossless SO(3) Cayley-Rodrigues triads, 2-sample
/// palindromic `O(h^2)` free decay, symmetric Strang `O(h^2)` strike and continuous-wave drive
/// schedules, and zero-comb quadrature stereo readout.
#[derive(Clone, Debug)]
pub struct ModalBank {
    sample_rate: f64,
    h_full: f64,
    h_half: f64,
    n: usize,
    req_n: usize,
    geometry: BodyGeometry,
    pair_topology: PairTopology,
    coupling: f64,
    xi: f64,
    headroom: f64,
    passive: f64,
    output_gain: f64,
    step_idx: u64,

    x: [f64; MAX_ATOMS],
    y: [f64; MAX_ATOMS],
    omega: [f64; MAX_ATOMS],
    rot_c: [f64; MAX_ATOMS],
    rot_s: [f64; MAX_ATOMS],
    rot_cf: [f64; MAX_ATOMS],
    rot_sf: [f64; MAX_ATOMS],
    exp_f: [f64; MAX_ATOMS],
    t_eff_f: [f64; MAX_ATOMS],
    exp_h: [f64; MAX_ATOMS],
    t_eff_h: [f64; MAX_ATOMS],
    beta: [f64; MAX_ATOMS],
    g: [f64; MAX_ATOMS],
    lam: [f64; MAX_ATOMS],
    bore_patm: [f64; MAX_ATOMS],
    port: [f64; MAX_ATOMS],
    hit_w: [f64; MAX_ATOMS],
    read: [f64; MAX_ATOMS],
    pan_c: [f64; MAX_ATOMS],
    pan_s: [f64; MAX_ATOMS],
    old_x: [f64; MAX_ATOMS],
    old_y: [f64; MAX_ATOMS],

    triads: TriadCache,
}

impl ModalBank {
    fn zeroed(sample_rate: f64) -> Self {
        let sr = sanitize_sample_rate(sample_rate);
        let h_full = 1.0 / sr;
        Self {
            sample_rate: sr,
            h_full,
            h_half: 0.5 * h_full,
            n: 0,
            req_n: 1,
            geometry: BodyGeometry::Harmonic,
            pair_topology: PairTopology::Star,
            coupling: 0.0,
            xi: 0.045,
            headroom: 0.0,
            passive: 0.0,
            output_gain: 0.42,
            step_idx: 0,
            x: [0.0; MAX_ATOMS],
            y: [0.0; MAX_ATOMS],
            omega: [0.0; MAX_ATOMS],
            rot_c: [1.0; MAX_ATOMS],
            rot_s: [0.0; MAX_ATOMS],
            rot_cf: [1.0; MAX_ATOMS],
            rot_sf: [0.0; MAX_ATOMS],
            exp_f: [1.0; MAX_ATOMS],
            t_eff_f: [0.0; MAX_ATOMS],
            exp_h: [1.0; MAX_ATOMS],
            t_eff_h: [0.0; MAX_ATOMS],
            beta: [0.0; MAX_ATOMS],
            g: [0.0; MAX_ATOMS],
            lam: [0.0; MAX_ATOMS],
            bore_patm: [6.0; MAX_ATOMS],
            port: [0.0; MAX_ATOMS],
            hit_w: [0.0; MAX_ATOMS],
            read: [0.0; MAX_ATOMS],
            pan_c: [1.0; MAX_ATOMS],
            pan_s: [0.0; MAX_ATOMS],
            old_x: [0.0; MAX_ATOMS],
            old_y: [0.0; MAX_ATOMS],
            triads: TriadCache::empty(),
        }
    }

    /// Construct a new [`ModalBank`] configured for `sample_rate`.
    pub fn new(sample_rate: f64, config: ModalConfig) -> Self {
        let _ = plate_candidates();
        let mut bank = Self::zeroed(sample_rate);
        bank.configure(config);
        bank
    }

    /// Construct a heap-allocated [`ModalBank`] to keep debug stack frames compact.
    pub fn new_boxed(sample_rate: f64, config: ModalConfig) -> Box<Self> {
        let _ = plate_candidates();
        let mut bank = Box::new(Self::zeroed(sample_rate));
        bank.configure(config);
        bank
    }

    /// Reconfigure all modal frequencies, loss factors, pan angles, and resonant triads
    /// without allocating.
    pub fn configure(&mut self, config: ModalConfig) {
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
        self.req_n = req_n;
        self.geometry = config.geometry;
        self.pair_topology = config.pair_topology;
        self.coupling = finite_or(config.coupling, 0.0).max(0.0);
        self.xi = finite_or(config.absorption, 0.045).max(0.0);
        self.headroom = finite_or(config.headroom, 0.0).max(0.0);

        let kerr_st = finite_or(config.kerr_semitones, 0.0);
        let beta_base = 2.0_f64.powf(kerr_st / 12.0) - 1.0;
        let h = self.h_full;
        let h_half = self.h_half;
        let brightness = finite_or(config.brightness, 0.5);
        let tilt_exp = -1.45 + 2.55 * brightness;
        let xi = self.xi;
        let radiation = finite_or(config.radiation, 0.15).max(0.0);
        let rad_base = 0.25 + 62.0 * radiation * radiation;
        let spread = finite_or(config.spread, 0.0);

        let mut r_sq = 0.0;
        let mut hit_sq = 0.0;
        for k in 0..n {
            let ratio = body_ratio(config.geometry, k, req_n);
            let om = w_body * ratio;
            self.omega[k] = om;

            let (sh, ch) = (om * h_half).sin_cos();
            self.rot_c[k] = ch;
            self.rot_s[k] = sh;
            let (sf, cf) = (om * h).sin_cos();
            self.rot_cf[k] = cf;
            self.rot_sf[k] = sf;
            self.beta[k] = beta_base;

            let rad_w = ratio.powf(tilt_exp);
            self.read[k] = rad_w;
            r_sq += rad_w * rad_w;

            let hw = ((k + 1) as f64 * PI * 0.317).sin();
            self.hit_w[k] = hw;
            hit_sq += hw * hw;

            let g_k = xi * om;
            let lam = rad_base * ratio.powf(0.32);
            let patm = 6.0;
            self.g[k] = g_k;
            self.lam[k] = lam;
            self.bore_patm[k] = patm;
            self.exp_f[k] = (-lam * h).exp();
            self.t_eff_f[k] = h * exprel_neg(lam * h);
            self.exp_h[k] = (-lam * h_half).exp();
            self.t_eff_h[k] = h_half * exprel_neg(lam * h_half);
            self.port[k] = g_k * h;

            let sign = if k & 1 == 0 { 1.0 } else { -1.0 };
            let ang = spread * sign * (0.20 + 0.85 * ((k + 1) as f64 / n as f64));
            let (ps, pc) = (0.5 * ang).sin_cos();
            self.pan_c[k] = pc;
            self.pan_s[k] = ps;
        }

        let r_norm = r_sq.sqrt();
        let r_inv = if r_norm > 0.0 { 1.0 / r_norm } else { 1.0 };
        let hit_inv = if hit_sq > 0.0 {
            (n as f64 / hit_sq).sqrt()
        } else {
            1.0
        };
        for k in 0..n {
            self.read[k] *= r_inv;
            self.hit_w[k] *= hit_inv;
        }

        self.passive = 0.035 * w_body * self.coupling * h;
        self.output_gain = 0.42;

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

    /// Update modal angular frequencies and rotation factors smoothly during pitch glides
    /// using zero-trig rational SO(2) Cayley steps.
    pub fn update_gliding_freq(&mut self, fundamental_hz: f64) {
        let n = self.n;
        if n == 0 || !fundamental_hz.is_finite() || fundamental_hz <= 0.0 {
            return;
        }
        let f_base = fundamental_hz.min(0.36 * self.sample_rate);
        let w_body = TAU * f_base;
        let om_cap = 0.48 * TAU * self.sample_rate;
        let req_n = self.req_n.max(1);
        for k in 0..n {
            let om = (w_body * body_ratio(self.geometry, k, req_n)).min(om_cap);
            self.omega[k] = om;
            let (ch, sh) = cayley_rotation(0.5 * om * self.h_half);
            self.rot_c[k] = ch;
            self.rot_s[k] = sh;
            let (cf, sf) = cayley_rotation(0.5 * om * self.h_full);
            self.rot_cf[k] = cf;
            self.rot_sf[k] = sf;
        }
    }

    /// Configured sample rate in Hz.
    #[inline]
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Update sample rate without clearing ringing modal states; returns `true` if the
    /// sanitized sample rate changed (signaling the caller to call [`Self::configure`]).
    #[inline]
    pub fn set_sample_rate(&mut self, sample_rate: f64) -> bool {
        let sr = sanitize_sample_rate(sample_rate);
        if sr.to_bits() == self.sample_rate.to_bits() {
            return false;
        }
        self.sample_rate = sr;
        self.h_full = 1.0 / sr;
        self.h_half = 0.5 * self.h_full;
        true
    }

    /// Clear all modal oscillation state and palindromic parity to clean silence while
    /// preserving the active configuration.
    pub fn reset(&mut self) {
        self.x.fill(0.0);
        self.y.fill(0.0);
        self.old_x.fill(0.0);
        self.old_y.fill(0.0);
        self.step_idx = 0;
    }

    /// Number of active modes below the Nyquist safety ceiling.
    #[inline]
    pub fn active_modes(&self) -> usize {
        self.n
    }

    /// True when total squared modal energy has decayed below [`MODAL_SILENCE_ENERGY`].
    #[inline]
    pub fn is_quiet(&self) -> bool {
        self.energy() < MODAL_SILENCE_ENERGY
    }

    #[inline]
    fn joint_free_step(&mut self, k: usize, full_step: bool, g_scale: f64) {
        let (dt, exp_l, t_eff, rot_c, rot_s) = if full_step {
            (
                self.h_full,
                self.exp_f[k],
                self.t_eff_f[k],
                self.rot_cf[k],
                self.rot_sf[k],
            )
        } else {
            (
                self.h_half,
                self.exp_h[k],
                self.t_eff_h[k],
                self.rot_c[k],
                self.rot_s[k],
            )
        };
        let (nx, ny) = joint_free_mode(
            self.x[k],
            self.y[k],
            FreeStepCoeffs {
                omega: self.omega[k],
                beta: self.beta[k],
                g: self.g[k] * g_scale,
                lam: self.lam[k],
                patm: self.bore_patm[k],
                dt,
                exp_l,
                t_eff,
                rot_c,
                rot_s,
            },
        );
        self.x[k] = nx;
        self.y[k] = ny;
    }

    #[inline]
    fn couple(&mut self, reverse: bool, scale: f64) {
        couple_pairs(
            &mut self.x,
            &mut self.y,
            CoupleParams {
                n: self.n,
                topology: self.pair_topology,
                passive: self.passive,
                headroom: self.headroom,
            },
            reverse,
            scale,
        );
    }

    #[inline]
    fn step_port_atom(&mut self, k: usize, tx: f64, ty: f64, eta: f64, headroom: f64) {
        let wx = self.x[k] - tx;
        let wy = self.y[k] - ty;
        let r = (wx * wx + wy * wy).sqrt();
        if eta <= 0.0 || r == 0.0 {
            return;
        }
        let kfac = if headroom <= 0.0 {
            1.0 / (1.0 + eta * r)
        } else {
            let eta_eff = eta / (1.0 + 2.0 * headroom);
            if r <= headroom {
                (-2.0 * eta_eff * headroom).exp()
            } else {
                atomic_pair_scalar(r, 0.0, headroom, eta_eff) / r
            }
        };
        self.x[k] = tx + wx * kfac;
        self.y[k] = ty + wy * kfac;
    }

    /// Forward or exact-adjoint reverse Gauss-Seidel half-step driven-port sweep across all
    /// five graph topologies.
    fn drive_port_pass(&mut self, in_x: f64, in_y: f64, scale: f64, reverse: bool) {
        let n = self.n;
        if n == 0 {
            return;
        }
        let headroom = self.headroom;
        if n == 1 {
            self.step_port_atom(0, in_x, in_y, scale * self.port[0], headroom);
            return;
        }

        let c = self.coupling;
        match self.pair_topology {
            PairTopology::Cascade | PairTopology::Bloom => {
                let is_cascade = self.pair_topology == PairTopology::Cascade;
                let xi = self.xi;
                if !reverse {
                    let mut tx = in_x;
                    let mut ty = in_y;
                    for k in 0..n {
                        self.step_port_atom(k, tx, ty, scale * self.port[k], headroom);
                        let wx = tx - self.x[k];
                        let wy = ty - self.y[k];
                        let sc = if is_cascade {
                            0.5 + 0.5 * c
                        } else {
                            let r = (wx * wx + wy * wy).sqrt();
                            let frac = (xi * r) / (1.0 + xi * r);
                            (0.5 + 1.5 * c) * frac
                        };
                        tx = sc * wx;
                        ty = sc * wy;
                    }
                } else {
                    let mut tx = in_x;
                    let mut ty = in_y;
                    for k in 0..n {
                        self.old_x[k] = tx;
                        self.old_y[k] = ty;
                        let wx = tx - self.x[k];
                        let wy = ty - self.y[k];
                        let sc = if is_cascade {
                            0.5 + 0.5 * c
                        } else {
                            let r = (wx * wx + wy * wy).sqrt();
                            let frac = (xi * r) / (1.0 + xi * r);
                            (0.5 + 1.5 * c) * frac
                        };
                        tx = sc * wx;
                        ty = sc * wy;
                    }
                    for k in (0..n).rev() {
                        self.step_port_atom(
                            k,
                            self.old_x[k],
                            self.old_y[k],
                            scale * self.port[k],
                            headroom,
                        );
                    }
                }
            }
            PairTopology::Star | PairTopology::Ring | PairTopology::Swarm => {
                let mut sx = 0.0;
                let mut sy = 0.0;
                if matches!(self.pair_topology, PairTopology::Star | PairTopology::Swarm) {
                    for k in 0..n {
                        sx += self.x[k];
                        sy += self.y[k];
                    }
                }
                let inv_nm1 = 1.0 / (n - 1) as f64;
                for idx in 0..n {
                    let k = if reverse { n - 1 - idx } else { idx };
                    let (tx, ty) = match self.pair_topology {
                        PairTopology::Ring => {
                            let prev = if k == 0 { n - 1 } else { k - 1 };
                            let inj_x = if k == 0 { (1.0 - c) * in_x } else { 0.0 };
                            let inj_y = if k == 0 { (1.0 - c) * in_y } else { 0.0 };
                            (c * self.x[prev] + inj_x, c * self.y[prev] + inj_y)
                        }
                        PairTopology::Swarm => (
                            (1.0 - c) * in_x + c * (sx - self.x[k]) * inv_nm1,
                            (1.0 - c) * in_y + c * (sy - self.y[k]) * inv_nm1,
                        ),
                        PairTopology::Star => {
                            if k == 0 {
                                (
                                    (1.0 - c * c) * in_x + c * (sx - self.x[0]) * inv_nm1,
                                    (1.0 - c * c) * in_y + c * (sy - self.y[0]) * inv_nm1,
                                )
                            } else {
                                (c * self.x[0], c * self.y[0])
                            }
                        }
                        PairTopology::Cascade | PairTopology::Bloom => (in_x, in_y),
                    };
                    let ox = self.x[k];
                    let oy = self.y[k];
                    self.step_port_atom(k, tx, ty, scale * self.port[k], headroom);
                    sx += self.x[k] - ox;
                    sy += self.y[k] - oy;
                }
            }
        }
    }

    /// 2-sample palindromic single-pass `O(h^2)` free decay:
    /// even samples run `J_h -> Pair(h, fwd) -> Triad(h, fwd)`, and odd samples run
    /// `Triad(h, rev) -> Pair(h, rev) -> J_h`.
    pub fn step_free_decay(&mut self) -> [f64; 2] {
        let n = self.n;
        if (self.step_idx & 1) == 0 {
            for k in 0..n {
                self.joint_free_step(k, true, 1.0);
            }
            self.couple(false, 1.0);
            self.triads
                .apply_pass(&self.x, &mut self.y, n, false, false);
        } else {
            self.triads.apply_pass(&self.x, &mut self.y, n, false, true);
            self.couple(true, 1.0);
            for k in (0..n).rev() {
                self.joint_free_step(k, true, 1.0);
            }
        }
        self.step_idx = self.step_idx.wrapping_add(1);
        self.read_stereo()
    }

    /// Advance the bank through a symmetric Strang `O(h^2)` mallet strike step, falling back
    /// to [`Self::step_free_decay`] once `strike_active` is `false`.
    pub fn step_strike(
        &mut self,
        strike_active: bool,
        strike_pulse: f64,
        drive: f64,
        velocity: f64,
        strike_time: f64,
    ) -> [f64; 2] {
        if !strike_active || !strike_time.is_finite() || strike_time <= 0.0 {
            return self.step_free_decay();
        }
        let n = self.n;
        let half_imp = 0.5 * strike_pulse * drive * velocity * (2.0 / strike_time) * self.h_full;

        for k in 0..n {
            self.joint_free_step(k, false, 1.0);
        }
        self.couple(false, 0.5);
        for k in 0..n {
            self.y[k] += half_imp * self.hit_w[k];
        }
        self.triads.apply_pass(&self.x, &mut self.y, n, true, false);
        self.triads.apply_pass(&self.x, &mut self.y, n, true, true);
        for k in (0..n).rev() {
            self.y[k] += half_imp * self.hit_w[k];
        }
        self.couple(true, 0.5);
        for k in (0..n).rev() {
            self.joint_free_step(k, false, 1.0);
        }
        self.step_idx = self.step_idx.wrapping_add(1);
        self.read_stereo()
    }

    /// Advance the bank through a symmetric Strang `O(h^2)` continuous-wave driven-port step,
    /// falling back to [`Self::step_free_decay`] when `gate` is `false`.
    pub fn step_wave(&mut self, gate: bool, in_re: f64, in_im: f64) -> [f64; 2] {
        if !gate {
            return self.step_free_decay();
        }
        let n = self.n;

        for k in 0..n {
            self.joint_free_step(k, false, 0.0);
        }
        self.couple(false, 0.5);
        self.drive_port_pass(in_re, in_im, 0.5, false);
        self.triads.apply_pass(&self.x, &mut self.y, n, true, false);
        self.triads.apply_pass(&self.x, &mut self.y, n, true, true);
        self.drive_port_pass(in_re, in_im, 0.5, true);
        self.couple(true, 0.5);
        for k in (0..n).rev() {
            self.joint_free_step(k, false, 0.0);
        }
        self.step_idx = self.step_idx.wrapping_add(1);
        self.read_stereo()
    }

    /// Unified symmetric Strang `O(h^2)` step supporting simultaneous M/S complex wave
    /// excitation (`(in_re, in_im)`) and layered mallet strike impulse (`strike_pulse`).
    ///
    /// Dispatches to [`Self::step_free_decay`], [`Self::step_strike`], or [`Self::step_wave`]
    /// when only one or neither excitation channel is active.
    #[allow(clippy::too_many_arguments)]
    pub fn step_wave_and_strike(
        &mut self,
        wave_active: bool,
        in_re: f64,
        in_im: f64,
        strike_active: bool,
        strike_pulse: f64,
        strike_drive: f64,
        velocity: f64,
        strike_time: f64,
    ) -> [f64; 2] {
        let strike_ok = strike_active
            && strike_time.is_finite()
            && strike_time > 0.0
            && strike_drive.is_finite()
            && strike_drive > 0.0;
        if !wave_active && !strike_ok {
            return self.step_free_decay();
        }
        if !wave_active {
            return self.step_strike(true, strike_pulse, strike_drive, velocity, strike_time);
        }
        if !strike_ok {
            return self.step_wave(true, in_re, in_im);
        }

        let n = self.n;
        let half_imp =
            0.5 * strike_pulse * strike_drive * velocity * (2.0 / strike_time) * self.h_full;
        for k in 0..n {
            self.joint_free_step(k, false, 0.0);
        }
        self.couple(false, 0.5);
        self.drive_port_pass(in_re, in_im, 0.5, false);
        for k in 0..n {
            self.y[k] += half_imp * self.hit_w[k];
        }
        self.triads.apply_pass(&self.x, &mut self.y, n, true, false);
        self.triads.apply_pass(&self.x, &mut self.y, n, true, true);
        for k in (0..n).rev() {
            self.y[k] += half_imp * self.hit_w[k];
        }
        self.drive_port_pass(in_re, in_im, 0.5, true);
        self.couple(true, 0.5);
        for k in (0..n).rev() {
            self.joint_free_step(k, false, 0.0);
        }
        self.step_idx = self.step_idx.wrapping_add(1);
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

    /// Total squared modal amplitude `sum_{k < n} (x[k]^2 + y[k]^2)`.
    #[inline]
    pub fn energy(&self) -> f64 {
        let mut en = 0.0;
        for k in 0..self.n {
            en += self.x[k] * self.x[k] + self.y[k] * self.y[k];
        }
        en
    }

    /// Complex modal state `(re, im)` for mode `k`, or `(0.0, 0.0)` when out of range.
    #[inline]
    pub fn mode_state(&self, k: usize) -> (f64, f64) {
        if k < self.n {
            (self.x[k], self.y[k])
        } else {
            (0.0, 0.0)
        }
    }

    /// Set complex modal state `(re, im)` for active mode `k` (bounds-safe no-op if `k >= n`).
    #[inline]
    pub fn set_mode_state(&mut self, k: usize, re: f64, im: f64) {
        if k < self.n {
            self.x[k] = re;
            self.y[k] = im;
        }
    }

    /// Angular frequency `omega` (rad/s) for active mode `k`, or `0.0` when out of range.
    #[inline]
    pub fn mode_omega(&self, k: usize) -> f64 {
        if k < self.n {
            self.omega[k]
        } else {
            0.0
        }
    }
}
