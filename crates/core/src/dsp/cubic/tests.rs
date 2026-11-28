//! Mathematical, numerical convergence (`O(h^2)`), energy conservation,
//! passivity, aliasing, and acoustic verification suite for `dsp::cubic`.

use super::*;

#[allow(clippy::too_many_arguments)]
fn rk4_joint_free_kerr(
    x0: f64,
    y0: f64,
    omega: f64,
    beta: f64,
    g: f64,
    lambda: f64,
    patm: f64,
    dt: f64,
    substeps: usize,
) -> (f64, f64) {
    let deriv = |x: f64, y: f64| -> (f64, f64) {
        let a = (x * x + y * y).sqrt();
        if a == 0.0 {
            return (0.0, 0.0);
        }
        let radial_rate = if patm > 0.0 && a > patm && lambda > 0.0 {
            let alpha = lambda / (2.0 * patm);
            -(alpha * (a * a + patm * patm) + g * a * a) / a
        } else {
            -(lambda + g * a)
        };
        let ang_rate = omega * (1.0 + beta * a);
        (
            radial_rate * x - ang_rate * y,
            radial_rate * y + ang_rate * x,
        )
    };

    let h = dt / substeps as f64;
    let mut x = x0;
    let mut y = y0;
    for _ in 0..substeps {
        let (k1x, k1y) = deriv(x, y);
        let (k2x, k2y) = deriv(x + 0.5 * h * k1x, y + 0.5 * h * k1y);
        let (k3x, k3y) = deriv(x + 0.5 * h * k2x, y + 0.5 * h * k2y);
        let (k4x, k4y) = deriv(x + h * k3x, y + h * k3y);
        x += (h / 6.0) * (k1x + 2.0 * k2x + 2.0 * k3x + k4x);
        y += (h / 6.0) * (k1y + 2.0 * k2y + 2.0 * k3y + k4y);
    }
    (x, y)
}

fn l2_err(a: &[f64], b: &[f64]) -> f64 {
    assert_eq!(a.len(), b.len());
    let mut sum = 0.0;
    for (&x, &y) in a.iter().zip(b.iter()) {
        let d = x - y;
        sum += d * d;
    }
    (sum / a.len().max(1) as f64).sqrt()
}

// =============================================================================
// 1. Elementary Helpers, Rotations & Exact Local Flows
// =============================================================================

#[test]
fn cayley_rotation_order_five_phase_and_omc_f32_sub_bass() {
    let err_w = |w: f64| {
        let (c, s) = cayley_rotation(w);
        assert!((c * c + s * s - 1.0).abs() < 1e-15);
        let ang = s.atan2(c);
        (ang - 2.0 * w).abs()
    };
    let e1 = err_w(0.08);
    let e2 = err_w(0.04);
    let ratio = e1 / e2;
    assert!(
        (ratio - 32.0).abs() < 0.2,
        "Cayley Pade (1,2) phase error should scale as O(w^5) (~32x), got {ratio:.3}"
    );

    // Sub-bass 18.35 Hz @ 48 kHz over 48_000 steps (1 full second):
    // omc_rotation_f32 avoids cos(theta) -> 1.0 cancellation.
    let sr = 48_000.0;
    let f_sub = 18.35;
    let theta = TAU * f_sub / sr;
    let (omc, sin) = omc_rotation_f32(theta);
    let (mut x, mut y) = (1.0_f32, 0.0_f32);
    for _ in 0..48_000 {
        (x, y) = apply_omc_rotation_f32(x, y, omc, sin);
    }
    let expected = TAU * f_sub;
    let phase_err = ((y as f64).atan2(x as f64) - expected.sin().atan2(expected.cos())).abs();
    assert!(
        phase_err < 1e-3,
        "omc_rotation_f32 sub-bass 1s drift too large: {phase_err:.6} rad"
    );
}

#[test]
fn joint_free_kerr_matches_rk4_interior_outer_and_crossing() {
    let omega = TAU * 180.0;
    let beta = 0.25;
    let g = 8.0;
    let lambda = 35.0;
    let patm = 1.2;

    // Case A: strictly inside headroom
    let (xa, ya) = joint_free_kerr(0.5, -0.4, omega, beta, g, lambda, patm, 0.004);
    let (rxa, rya) = rk4_joint_free_kerr(0.5, -0.4, omega, beta, g, lambda, patm, 0.004, 4096);
    assert!((xa - rxa).abs() < 1e-11 && (ya - rya).abs() < 1e-11);

    // Case B: strictly outside headroom (stays above patm)
    let (xb, yb) = joint_free_kerr(3.2, 1.4, omega, beta, g, lambda, patm, 0.002);
    let (rxb, ryb) = rk4_joint_free_kerr(3.2, 1.4, omega, beta, g, lambda, patm, 0.002, 4096);
    assert!((xb - rxb).abs() < 1e-11 && (yb - ryb).abs() < 1e-11);

    // Case C: crosses |z| = patm mid-step
    let (xc, yc) = joint_free_kerr(2.4, -0.7, omega, beta, g, lambda, patm, 0.025);
    let (rxc, ryc) = rk4_joint_free_kerr(2.4, -0.7, omega, beta, g, lambda, patm, 0.025, 8192);
    assert!(
        (xc - rxc).abs() < 1e-9 && (yc - ryc).abs() < 1e-9,
        "Mid-step crossing mismatch: ({xc}, {yc}) vs ({rxc}, {ryc})"
    );
}

#[test]
fn zero_hz_capacitor_exact_ode_and_self_limiting_accumulation() {
    let sr = 48_000.0;
    let tau_decay = 0.145;
    let g = 1.8;
    let mut cap = ZeroHzCapacitor::new(sr, tau_decay, g, 0.0022);
    cap.c = 1.25;

    let steps = 2400;
    for _ in 0..steps {
        cap.step();
    }
    let dt_tot = steps as f64 / sr;
    let lam = 1.0 / tau_decay;
    let t_eff = dt_tot * exprel_neg(lam * dt_tot);
    let exact = (1.25 * (-lam * dt_tot).exp()) / (1.0 + g * 1.25 * t_eff);
    assert!(
        (cap.value() - exact).abs() < 1e-13,
        "ZeroHzCapacitor drifted from exact ODE: {} vs {}",
        cap.value(),
        exact
    );

    // Repeated additive accent pulses remain self-limited without hard clipping.
    cap.reset();
    let mut peaks = [0.0_f64; 16];
    for slot in &mut peaks {
        cap.add(0.9, 64);
        for _ in 0..64 {
            cap.step();
        }
        *slot = cap.value();
        for _ in 0..4800 {
            cap.step();
        }
    }
    assert!(peaks[15] > peaks[0], "Accent pulses should accumulate");
    assert!(
        peaks[15].is_finite() && peaks[15] < 4.0,
        "Cubic discharge should self-limit repeated additive accents: {}",
        peaks[15]
    );
}

#[test]
fn radial_pair_step_u1_equivariance_and_phase_flow_fixed_point() {
    let theta = 0.73_f64;
    let (s, c) = theta.sin_cos();
    let rot = |x: f64, y: f64| (c * x - s * y, s * x + c * y);

    let (z1x, z1y) = radial_pair_step(2.4, -1.1, 0.3, 0.7, 0.8, 0.45);
    let (rz1x, rz1y) = rot(z1x, z1y);

    let (rx0, ry0) = rot(2.4, -1.1);
    let (rvx, rvy) = rot(0.3, 0.7);
    let (z2x, z2y) = radial_pair_step(rx0, ry0, rvx, rvy, 0.8, 0.45);
    assert!((rz1x - z2x).abs() < 1e-14 && (rz1y - z2y).abs() < 1e-14);

    // Fixed point at z = -R on cubic_phase_flow stays exact.
    let (fx, fy) = cubic_phase_flow(-2.0, 0.0, 5.0, 1.7);
    assert!((fx + 2.0).abs() < 1e-15 && fy.abs() < 1e-15);

    // Semigroup on cubic_periodic_phase_flow across chart crossings.
    let (p1x, p1y) = cubic_periodic_phase_flow(0.6, 0.8, 3.7, 0.41);
    let (p2x, p2y) = cubic_periodic_phase_flow(p1x, p1y, 3.7, 0.83);
    let (p_all_x, p_all_y) = cubic_periodic_phase_flow(0.6, 0.8, 3.7, 1.24);
    assert!((p2x - p_all_x).abs() < 1e-13 && (p2y - p_all_y).abs() < 1e-13);
}

// =============================================================================
// 2. Exciters & Pitch-Aware Nyquist Reach Guard
// =============================================================================

#[test]
fn exciter_endpoints_and_nyquist_reach_guard() {
    // Exact endpoints: rho = 0 -> sin(theta), k = 0 -> rational Blaschke map.
    for i in 0..256 {
        let theta = TAU * i as f64 / 256.0;
        let (s, c) = theta.sin_cos();
        assert!((mobius_exciter(c, s, 0.0, 0.8) - s).abs() < 1e-15);
        let rho = 0.72;
        let blaschke_im = ((1.0 - rho * rho) * s) / (1.0 - 2.0 * rho * c + rho * rho);
        assert!((mobius_exciter(c, s, rho, 0.0) - blaschke_im).abs() < 1e-14);
    }

    // Verify first aliased harmonic (k_alias = floor(sr / (2 * freq)) + 1) sits below -98 dB
    // via exact single-cycle DFT of the continuous Mobius exciter across pitches.
    let sr = 48_000.0;
    for &freq in &[440.0, 2000.0, 6000.0] {
        let rho = pitch_guard_rho(0.995, freq, sr);
        let coeffs = MobiusCoeffs::new(rho, 0.7);
        let k_alias = (0.5 * sr / freq).floor() as usize + 1;
        let mut fund_re = 0.0;
        let mut fund_im = 0.0;
        let mut alias_re = 0.0;
        let mut alias_im = 0.0;
        let n_cyc = 2048usize;
        for idx in 0..n_cyc {
            let t = TAU * idx as f64 / n_cyc as f64;
            let (s, c) = t.sin_cos();
            let v = mobius_exciter_coeffs(c, s, &coeffs);
            fund_re += v * t.cos();
            fund_im += v * t.sin();
            let tn = t * k_alias as f64;
            alias_re += v * tn.cos();
            alias_im += v * tn.sin();
        }
        let fund_mag = fund_re.hypot(fund_im);
        let alias_mag = alias_re.hypot(alias_im);
        let alias_db = 20.0 * (alias_mag / fund_mag.max(1e-30)).max(1e-12).log10();
        assert!(
            alias_db < -98.0,
            "Pitch guard failed at {freq} Hz: first aliased harmonic (k={k_alias}) = {alias_db:.1} dB"
        );
    }

    // Continuity of pitch_guard_rho across a glide (no staircase discontinuities).
    let mut prev = pitch_guard_rho(0.99, 100.0, sr);
    for step in 1..1000 {
        let f = 100.0 + step as f64 * 10.0;
        let cur = pitch_guard_rho(0.99, f, sr);
        assert!(cur <= prev + 1e-15);
        assert!((prev - cur) < 0.01);
        prev = cur;
    }
}

#[test]
fn poisson_driver_l2_and_strike_impulse_integral() {
    let steps = 4096;
    let coeffs = HardyCoeffs::new(CubicWaveform::Poisson, 0.7, 0.4, 220.0, 48_000.0, true);
    let mut energy = 0.0;
    for i in 0..steps {
        let theta = TAU * i as f64 / steps as f64;
        let (s, c) = theta.sin_cos();
        let (fs, fc) = (3.0 * theta).sin_cos();
        let (x, y) = poisson_analytic(c, fc, fs, coeffs.r, coeffs.poisson_norm);
        energy += x * x + y * y;
        let _ = s;
    }
    assert!((energy / steps as f64 - 1.0).abs() < 1e-3);

    // Strike pulse integral: sum (2 / strike_time) * pulse(t_mid) * h == 1.0.
    let sr = 48_000.0;
    let mut sp = StrikePulse::new(sr);
    sp.trigger(0.4, 1.0, 1.0);
    let mut integral = 0.0;
    while sp.is_active() {
        let (active, p) = sp.step_midpoint();
        assert!(active);
        integral += p * (2.0 / sp.strike_time()) * (1.0 / sr);
    }
    assert!(
        (integral - 1.0).abs() < 1e-3,
        "Strike impulse integral should be ~1.0, got {integral:.6}"
    );
}

// =============================================================================
// 3. Resonant Filters (`ImLpfCascade` & `DiodeLadderFilter`)
// =============================================================================

#[test]
fn im_lpf_butterworth_response_and_zero_comb_stereo() {
    let sr = 96_000.0;
    let fc = 800.0;

    for &(poles, expected_oct_slope_db) in &[
        (ImLpfPoles::Two, -12.0),
        (ImLpfPoles::Three, -18.0),
        (ImLpfPoles::Four, -24.0),
    ] {
        // DC gain == 0 dB
        let mut f_dc = ImLpfCascade::new(sr, poles);
        let mut y_dc = 0.0;
        for _ in 0..8000 {
            y_dc = f_dc.step(1.0, fc, 0.0, 0.0, 0.0);
        }
        assert!(
            (y_dc - 1.0).abs() < 1e-4,
            "{poles:?} DC gain expected 1.0, got {y_dc}"
        );

        // Cutoff magnitude at fc == -3.01 dB
        let measure_db = |test_hz: f64| -> f64 {
            let mut flt = ImLpfCascade::new(sr, poles);
            let settle = (sr * 0.08) as usize;
            let measure = (sr * 0.04) as usize;
            let mut in_sq = 0.0;
            let mut out_sq = 0.0;
            for n in 0..(settle + measure) {
                let u = (TAU * test_hz * (n as f64 + 0.5) / sr).sin();
                let y = flt.step(u, fc, 0.0, 0.0, 0.0);
                if n >= settle {
                    in_sq += u * u;
                    out_sq += y * y;
                }
            }
            10.0 * (out_sq / in_sq).log10()
        };
        let mag_fc_db = measure_db(fc);
        assert!(
            (mag_fc_db - (-3.0103)).abs() < 0.08,
            "{poles:?} Butterworth -3.01 dB at fc failed: {mag_fc_db:.3} dB"
        );
        let mag_4fc_db = measure_db(4.0 * fc);
        let mag_8fc_db = measure_db(8.0 * fc);
        let slope = mag_8fc_db - mag_4fc_db;
        assert!(
            (slope - expected_oct_slope_db).abs() < 0.8,
            "{poles:?} stopband slope expected ~{expected_oct_slope_db} dB/oct, got {slope:.2} dB/oct"
        );

        // Zero-comb algebraic mono compatibility: (L + R) / 2 == cos(spread / 2) * y
        let mut flt_s = ImLpfCascade::new(sr, poles);
        let spread = 1.15_f64;
        let half_cos = (0.5 * spread).cos();
        for n in 0..256 {
            let u = (TAU * 330.0 * n as f64 / sr).sin();
            let [l, r] = flt_s.step_stereo(u, fc, 0.65, 0.12, 0.08, spread);
            let mono = 0.5 * (l + r);
            assert!(
                (mono - half_cos * flt_s.out).abs() < 1e-14,
                "{poles:?} stereo mono sum comb error"
            );
        }
    }
}

#[test]
fn im_lpf_cascade_second_order_convergence() {
    let run = |sr: f64, stride: usize| -> Vec<f64> {
        let mut flt = ImLpfCascade::new(sr, ImLpfPoles::Four);
        let total = 256 * stride;
        let mut out = Vec::with_capacity(256);
        for n in 0..total {
            let t_mid = (n as f64 + 0.5) / sr;
            let u = (TAU * 220.0 * t_mid).sin() + 0.4 * (TAU * 550.0 * t_mid).cos();
            let y = flt.step(u, 900.0, 0.65, 0.15, 0.12);
            if (n + 1) % stride == 0 {
                out.push(y);
            }
        }
        out
    };
    let y1 = run(48_000.0, 1);
    let y2 = run(96_000.0, 2);
    let y4 = run(192_000.0, 4);
    let e12 = l2_err(&y1, &y2);
    let e24 = l2_err(&y2, &y4);
    let ratio = e12 / e24;
    assert!(
        ratio > 3.7 && ratio < 4.3,
        "ImLpfCascade expected O(h^2) ratio ~4.0, got {ratio:.3} (e12={e12:.3e}, e24={e24:.3e})"
    );
}

#[test]
fn diode_ladder_charge_conservation_post_sat_hpf_overdrive_and_order_two() {
    // 1. Exact total charge conservation 2*v[0] + v[1] + v[2] + v[3] in diode bark
    let mut flt = DiodeLadderFilter::new(48_000.0, DEFAULT_HPF_HZ);
    flt.v = [0.85, -0.42, 0.55, -0.91];
    let q0 = flt.total_charge();
    flt.bark_pass(0.35, false);
    assert!((flt.total_charge() - q0).abs() < 1e-14);
    flt.bark_pass(0.35, true);
    assert!((flt.total_charge() - q0).abs() < 1e-14);

    // Strictly linear (no-op) below threshold 0.16
    flt.v = [0.05, 0.10, -0.02, 0.08];
    let before = flt.v;
    flt.bark_pass(0.8, false);
    assert_eq!(flt.v, before);

    // 2. Post-saturation C17 HPF removes feedback DC even on an asymmetric sawtooth
    let sr = 48_000.0;
    let mut flt_c17 = DiodeLadderFilter::new(sr, DEFAULT_HPF_HZ);
    let mut sum_fb = 0.0;
    let settle = 12_000;
    let measure = 12_000;
    for n in 0..(settle + measure) {
        let phase = (110.0 * n as f64 / sr).fract();
        let saw = 2.0 * phase - 1.0 + 0.35; // intentionally DC-biased asymmetric drive
        let y = flt_c17.step(saw, 600.0, 0.75, 0.2);
        assert!(y.is_finite());
        if n >= settle {
            let f = y / (1.0 + y.abs() / 0.40);
            sum_fb += f - flt_c17.dc;
        }
    }
    let mean_fb = (sum_fb / measure as f64).abs();
    assert!(
        mean_fb < 5e-3,
        "Post-sat C17 feedback return should reject DC, got mean {mean_fb:.5}"
    );

    // 3. Extreme 100x overdrive stability
    let mut flt_od = DiodeLadderFilter::new(sr, DEFAULT_HPF_HZ);
    for n in 0..2048 {
        let u = 100.0 * (TAU * 130.0 * n as f64 / sr).sin();
        let y = flt_od.step(u, 1400.0, 0.95, 1.0);
        assert!(y.is_finite() && y.abs() < 200.0);
    }

    // 4. Global O(h^2) convergence with resonance + post-sat C17 + nonlinear bark
    let run = |rate: f64, stride: usize| -> Vec<f64> {
        let mut d = DiodeLadderFilter::new(rate, DEFAULT_HPF_HZ);
        let total = 256 * stride;
        let mut out = Vec::with_capacity(256);
        for n in 0..total {
            let t_mid = (n as f64 + 0.5) / rate;
            let u = 0.9 * (TAU * 180.0 * t_mid).sin() + 0.5 * (TAU * 430.0 * t_mid).cos();
            let y = d.step(u, 750.0, 0.72, 0.25);
            if (n + 1) % stride == 0 {
                out.push(y);
            }
        }
        out
    };
    let y1 = run(48_000.0, 1);
    let y2 = run(96_000.0, 2);
    let y4 = run(192_000.0, 4);
    let ratio = l2_err(&y1, &y2) / l2_err(&y2, &y4);
    assert!(
        ratio > 3.7 && ratio < 4.3,
        "DiodeLadderFilter expected O(h^2) ratio ~4.0, got {ratio:.3}"
    );
}

// =============================================================================
// 4. Modal Bank, Pair Coupling & Scale-Invariant SO(3) Triads
// =============================================================================

#[test]
fn triads_orthogonality_reversibility_scale_invariance_and_upward_transfer() {
    // Scale invariance across six fundamental frequencies (including sub-bass 32.7 Hz)
    let base_freqs = [32.703_195_66, 55.0, 110.0, 220.0, 440.0, 880.0];
    let n = 16usize;
    let build_for = |f0: f64| {
        let omegas: Vec<f64> = (0..n).map(|k| TAU * f0 * (k + 1) as f64).collect();
        build_resonant_triads(&omegas, 3, 1e-6)
    };
    let ref_triads = build_for(base_freqs[0]);
    assert!(ref_triads.len() > 10);
    for &f0 in &base_freqs[1..] {
        let t = build_for(f0);
        assert_eq!(t.len(), ref_triads.len());
        for idx in 0..t.len() {
            assert_eq!(
                t.triplets[idx], ref_triads.triplets[idx],
                "Triad selection changed at f0={f0}"
            );
            assert!((t.da[idx] - ref_triads.da[idx]).abs() < 1e-13);
            assert!((t.db[idx] - ref_triads.db[idx]).abs() < 1e-13);
        }
    }

    // Orthogonality & time-reversibility across all 3 TriadAxis variants
    let x: Vec<f64> = (0..n).map(|k| ((k + 1) as f64 * 0.7).sin()).collect();
    let y0: Vec<f64> = (0..n).map(|k| ((k + 1) as f64 * 1.1).cos()).collect();
    let e0: f64 = y0.iter().map(|v| v * v).sum();

    for axis in [TriadAxis::Diff, TriadAxis::Sum, TriadAxis::Manley] {
        let mut y = y0.clone();
        triad_rodrigues_pass(&x, &mut y, &ref_triads, 0.08, axis, false);
        let e1: f64 = y.iter().map(|v| v * v).sum();
        assert!(
            (e1 - e0).abs() < 1e-14,
            "{axis:?} SO(3) energy conservation violated: {e1} vs {e0}"
        );
        triad_rodrigues_pass(&x, &mut y, &ref_triads, -0.08, axis, true);
        for k in 0..n {
            assert!(
                (y[k] - y0[k]).abs() < 1e-14,
                "{axis:?} SO(3) time reversibility failed at mode {k}"
            );
        }
    }

    // Upward sum-frequency transfer (modes 0 + 1 -> mode 2) on Sum & Manley axes
    let omegas = [TAU * 100.0, TAU * 200.0, TAU * 300.0];
    let tri = build_resonant_triads(&omegas, 3, 1e-6);
    assert_eq!(tri.len(), 1);
    assert_eq!(tri.triplets[0], [0, 1, 2]);
    for axis in [TriadAxis::Sum, TriadAxis::Manley] {
        let x3 = [0.8, 0.6, 0.0];
        let mut y3 = [0.5, -0.4, 0.0];
        triad_rodrigues_pass(&x3, &mut y3, &tri, 0.1, axis, false);
        assert!(
            y3[2].abs() > 1e-3,
            "{axis:?} should transfer energy into upper sum mode 2"
        );
    }
}

#[test]
fn pair_exchange_mean_conservation_and_passivity() {
    for &headroom in &[0.0, 0.5, 2.0] {
        let mut x = [1.8, -0.6];
        let mut y = [-0.9, 1.4];
        let mx0 = 0.5 * (x[0] + x[1]);
        let my0 = 0.5 * (y[0] + y[1]);
        let e0 = x[0] * x[0] + y[0] * y[0] + x[1] * x[1] + y[1] * y[1];

        pair_exchange(&mut x, &mut y, 0, 1, 0.35, headroom);

        let mx1 = 0.5 * (x[0] + x[1]);
        let my1 = 0.5 * (y[0] + y[1]);
        let e1 = x[0] * x[0] + y[0] * y[0] + x[1] * x[1] + y[1] * y[1];
        assert!((mx1 - mx0).abs() < 1e-15 && (my1 - my0).abs() < 1e-15);
        assert!(e1 < e0, "pair_exchange must be strictly dissipative");
    }
}

#[test]
fn modal_bank_second_order_convergence_all_five_topologies() {
    let topologies = [
        PairTopology::Star,
        PairTopology::Cascade,
        PairTopology::Bloom,
        PairTopology::Ring,
        PairTopology::Swarm,
    ];

    for topo in topologies {
        let make_cfg = || ModalConfig {
            fundamental_hz: 180.0,
            atoms: 8,
            geometry: BodyGeometry::Harmonic,
            pair_topology: topo,
            coupling: 0.45,
            absorption: 0.05,
            radiation: 0.2,
            brightness: 0.5,
            headroom: 0.4,
            kerr_semitones: 1.5,
            triad_amount: 0.3,
            triad_axis: TriadAxis::Sum,
            spread: 0.4,
        };

        // Palindromic free decay O(h^2) sampled on even steps
        let run_free = |sr: f64, stride: usize| -> Vec<f64> {
            let mut bank = ModalBank::new_boxed(sr, make_cfg());
            for k in 0..bank.active_modes() {
                bank.set_mode_state(k, 0.5 / (k + 1) as f64, -0.3 / (k + 1) as f64);
            }
            let total = 128 * 2 * stride;
            let mut out = Vec::with_capacity(128);
            for n in 0..total {
                let [l, r] = bank.step_free_decay();
                if (n + 1) % (2 * stride) == 0 {
                    out.push(0.5 * (l + r));
                }
            }
            out
        };
        let f1 = run_free(48_000.0, 1);
        let f2 = run_free(96_000.0, 2);
        let f4 = run_free(192_000.0, 4);
        let ratio_free = l2_err(&f1, &f2) / l2_err(&f2, &f4);
        assert!(
            ratio_free > 3.5 && ratio_free < 4.5,
            "ModalBank free decay {topo:?} expected O(h^2) ~4.0, got {ratio_free:.3}"
        );

        // Continuous-wave driven O(h^2) with exact-adjoint reverse Gauss-Seidel
        let run_wave = |sr: f64, stride: usize| -> Vec<f64> {
            let mut bank = ModalBank::new_boxed(sr, make_cfg());
            let total = 192 * stride;
            let mut out = Vec::with_capacity(192);
            for n in 0..total {
                let t_mid = (n as f64 + 0.5) / sr;
                let (s, c) = (TAU * 180.0 * t_mid).sin_cos();
                let [l, r] = bank.step_wave(true, 0.6 * c, 0.6 * s);
                if (n + 1) % stride == 0 {
                    out.push(0.5 * (l + r));
                }
            }
            out
        };
        let w1 = run_wave(48_000.0, 1);
        let w2 = run_wave(96_000.0, 2);
        let w4 = run_wave(192_000.0, 4);
        let ratio_wave = l2_err(&w1, &w2) / l2_err(&w2, &w4);
        assert!(
            ratio_wave > 3.5 && ratio_wave < 4.5,
            "ModalBank wave drive {topo:?} expected O(h^2) ~4.0, got {ratio_wave:.3}"
        );
    }
}

#[test]
fn modal_bank_membrane_kerr_kick_and_plate_triad_wash() {
    let sr = 48_000.0;

    // Membrane kick with positive State-Kerr pitch drop: early frequency > late frequency
    let mut kick = ModalBank::new_boxed(
        sr,
        ModalConfig {
            fundamental_hz: 55.0,
            atoms: 8,
            geometry: BodyGeometry::Membrane,
            absorption: 0.04,
            radiation: 0.18,
            kerr_semitones: 12.0,
            spread: 0.0,
            ..ModalConfig::default()
        },
    );
    let mut pulse = StrikePulse::new(sr);
    pulse.trigger(0.75, 1.8, 1.0);
    let mut phases = Vec::with_capacity(4800);
    for _ in 0..4800 {
        let (act, p) = pulse.step_midpoint();
        kick.step_strike(act, p, 1.8, 1.0, pulse.strike_time());
        let (re, im) = kick.mode_state(0);
        phases.push(im.atan2(re));
    }
    let avg_hz = |slice: &[f64]| -> f64 {
        let mut dphi_sum = 0.0;
        for w in slice.windows(2) {
            let mut d = w[1] - w[0];
            while d > core::f64::consts::PI {
                d -= TAU;
            }
            while d < -core::f64::consts::PI {
                d += TAU;
            }
            dphi_sum += d;
        }
        (dphi_sum / (slice.len() - 1) as f64) * sr / TAU
    };
    let early_hz = avg_hz(&phases[120..600]);
    let late_hz = avg_hz(&phases[3600..4400]);
    assert!(
        early_hz > late_hz + 5.0,
        "Membrane Kerr kick should pitch-drop: early={early_hz:.1} Hz, late={late_hz:.1} Hz"
    );

    // Plate cymbal strike: SO(3) triads pump energy into upper partials
    let run_plate = |triad_amount: f64| -> f64 {
        let mut plate = ModalBank::new_boxed(
            sr,
            ModalConfig {
                fundamental_hz: 320.0,
                atoms: 24,
                geometry: BodyGeometry::Plate,
                absorption: 0.015,
                radiation: 0.08,
                triad_amount,
                triad_axis: TriadAxis::Sum,
                spread: 0.6,
                ..ModalConfig::default()
            },
        );
        // Excite strictly the lowest 4 modes and observe energy pumped into modes 12..24
        for k in 0..4 {
            plate.set_mode_state(k, 0.7, 0.3);
        }
        for _ in 0..1200 {
            plate.step_free_decay();
        }
        let mut upper_e = 0.0;
        for k in 12..plate.active_modes() {
            let (re, im) = plate.mode_state(k);
            upper_e += re * re + im * im;
        }
        upper_e
    };
    let upper_no_tri = run_plate(0.0);
    let upper_with_tri = run_plate(0.65);
    assert!(
        upper_with_tri > upper_no_tri + 1e-3,
        "Plate triads should pump energy into upper modes: with={upper_with_tri:.6} vs without={upper_no_tri:.6}"
    );
}

// =============================================================================
// 5. Single-Voice Aeroacoustic Wind Primitives (`FlueJet` & `BrassReed`)
// =============================================================================

#[test]
fn flue_jet_oscillation_damping_and_second_order_convergence() {
    let sr = 48_000.0;
    let cfg = FlueJetConfig {
        fundamental_hz: 440.0,
        atoms: 6,
        geometry: BodyGeometry::Harmonic,
        drive: 0.75,
        velocity: 0.9,
        embouchure: 0.6,
        brightness: 0.55,
        absorption: 0.05,
        radiation: 0.15,
        noise_enabled: true,
        ..FlueJetConfig::default()
    };
    let mut flue = FlueJet::new_boxed(sr, cfg);
    flue.set_gate(true);
    let mut peak = 0.0_f64;
    for _ in 0..12_000 {
        let [l, r] = flue.step();
        assert!(l.is_finite() && r.is_finite());
        peak = peak.max(l.abs().max(r.abs()));
    }
    assert!(
        peak > 0.05 && peak < 20.0,
        "FlueJet peak out of range: {peak}"
    );

    // Release decays smoothly
    flue.set_gate(false);
    let e_rel = flue.energy();
    for _ in 0..12_000 {
        flue.step();
    }
    assert!(flue.energy() < e_rel * 0.1);

    // Deterministic zero-noise O(h^2) convergence
    let run = |rate: f64, stride: usize| -> Vec<f64> {
        let mut f = FlueJet::new_boxed(
            rate,
            FlueJetConfig {
                noise_enabled: false,
                coupling: 0.25,
                triad_amount: 0.25,
                ..cfg
            },
        );
        f.set_gate(true);
        f.set_jet_state(0.05, -0.02);
        let total = 256 * stride;
        let mut out = Vec::with_capacity(256);
        for n in 0..total {
            let [l, r] = f.step_with_noise(0.0);
            if (n + 1) % stride == 0 {
                out.push(0.5 * (l + r));
            }
        }
        out
    };
    let y1 = run(48_000.0, 1);
    let y2 = run(96_000.0, 2);
    let y4 = run(192_000.0, 4);
    let ratio = l2_err(&y1, &y2) / l2_err(&y2, &y4);
    assert!(
        ratio > 3.6 && ratio < 4.4,
        "FlueJet expected O(h^2) ratio ~4.0, got {ratio:.3}"
    );
}

#[test]
fn brass_reed_si_joule_conservation_and_order_two_with_active_beating() {
    // 1. Exact SI Joule conservation for both active-supply and sealed-plenum runs,
    //    with active beating (p_beat_pa = 750 Pa < nominal 850 Pa), pair coupling, and triads.
    for &supply_area in &[0.0, 3.6e-4] {
        let mut reed = BrassReed::new_boxed(
            48_000.0,
            BrassReedConfig {
                fundamental_hz: 110.0,
                atoms: 12,
                drive: 0.85,
                velocity: 0.95,
                p_beat_pa: 750.0,
                coupling: 0.25,
                triad_amount: 0.35,
                triad_axis: TriadAxis::Manley,
                supply_area,
                ..BrassReedConfig::default()
            },
        );
        reed.set_gate(true);
        let e0 = reed.total_energy_joules();
        let mut ledger = ReedEnergyLedger::default();
        let mut peak = 0.0_f64;
        for _ in 0..4800 {
            let [l, r] = reed.step_with_ledger(&mut ledger);
            assert!(l.is_finite() && r.is_finite());
            peak = peak.max(l.abs().max(r.abs()));
        }
        let e1 = reed.total_energy_joules();
        let net_flow = ledger.work_supply
            - ledger.diss_supply
            - ledger.diss_bore
            - ledger.diss_valve
            - ledger.diss_pair
            - ledger.diss_bernoulli;
        let residual = ((e1 - e0) - net_flow).abs();
        assert!(
            residual < 1e-13,
            "BrassReed SI Joule conservation failed (supply_area={supply_area}): residual={residual:.3e}"
        );
        assert!(
            ledger.diss_bore >= 0.0 && ledger.diss_valve >= 0.0 && ledger.diss_bernoulli >= 0.0
        );
        if supply_area > 0.0 {
            assert!(
                peak > 0.02,
                "Active BrassReed should produce sound, peak={peak}"
            );
        }
    }

    // 2. Global O(h^2) convergence in BOTH non-beating (1500 Pa) AND active-beating (750 Pa)
    //    regimes — verifying the centered C^2 rolling-shallot closure through contact.
    for &p_beat_pa in &[1500.0, 750.0] {
        let run = |rate: f64, stride: usize| -> Vec<f64> {
            let mut reed = BrassReed::new_boxed(
                rate,
                BrassReedConfig {
                    fundamental_hz: 146.83,
                    atoms: 10,
                    drive: 0.9,
                    velocity: 1.0,
                    p_beat_pa,
                    coupling: 0.2,
                    triad_amount: 0.3,
                    triad_axis: TriadAxis::Manley,
                    ..BrassReedConfig::default()
                },
            );
            reed.set_gate(true);
            let total = 320 * stride;
            let mut out = Vec::with_capacity(320);
            for n in 0..total {
                let [l, r] = reed.step();
                if (n + 1) % stride == 0 {
                    out.push(0.5 * (l + r));
                }
            }
            out
        };
        let y1 = run(48_000.0, 1);
        let y2 = run(96_000.0, 2);
        let y4 = run(192_000.0, 4);
        let e12 = l2_err(&y1, &y2);
        let e24 = l2_err(&y2, &y4);
        let ratio = e12 / e24;
        assert!(
            ratio > 3.6 && ratio < 4.4,
            "BrassReed (p_beat={p_beat_pa} Pa) expected O(h^2) ratio ~4.0, got {ratio:.3} (e12={e12:.3e}, e24={e24:.3e})"
        );
    }
}

// =============================================================================
// 6. Zero-Delay 96-Mode Ambient Modal Room (`AmbientModalRoom`)
// =============================================================================

#[test]
fn ambient_modal_room_passivity_iacc_and_second_order_convergence() {
    let sr = 48_000.0;
    let mut room = AmbientModalRoom::new_boxed(sr);
    assert!(room.is_quiet());

    // Excite with a huge impulse exceeding P_atm = 6.0 to engage outer shock clamping too
    let _ = room.step([12.0, 12.0]);
    let mut prev_e = room.energy();
    assert!(prev_e > 0.0 && !room.is_quiet());

    let mut sum_lr = 0.0;
    let mut sum_l2 = 0.0;
    let mut sum_r2 = 0.0;
    for _ in 0..4096 {
        let [l, r] = room.step([0.0, 0.0]);
        assert!(l.is_finite() && r.is_finite());
        let cur_e = room.energy();
        assert!(
            cur_e < prev_e,
            "AmbientModalRoom must strictly dissipate zero-input energy: cur={cur_e} >= prev={prev_e}"
        );
        prev_e = cur_e;
        sum_lr += l * r;
        sum_l2 += l * l;
        sum_r2 += r * r;
    }

    // Zero-lag IACC on a centered mono impulse should indicate diffuse decorrelation
    let iacc = (sum_lr / (sum_l2 * sum_r2).sqrt().max(1e-30)).abs();
    assert!(
        iacc < 0.35,
        "AmbientModalRoom stereo decorrelation too correlated: |IACC| = {iacc:.3}"
    );

    room.reset();
    assert!(room.is_quiet());

    // Global O(h^2) convergence across 48 / 96 / 192 kHz
    let run = |rate: f64, stride: usize| -> Vec<f64> {
        let mut r = AmbientModalRoom::new_boxed(rate);
        let total = 256 * stride;
        let mut out = Vec::with_capacity(256);
        for n in 0..total {
            let t_mid = (n as f64 + 0.5) / rate;
            let env = (-t_mid / 0.002).exp();
            let s_l = env * (TAU * 310.0 * t_mid).sin();
            let s_r = env * (TAU * 470.0 * t_mid).cos();
            let [wl, wr] = r.step([s_l, s_r]);
            if (n + 1) % stride == 0 {
                out.push(0.5 * (wl + wr));
            }
        }
        out
    };
    let y1 = run(48_000.0, 1);
    let y2 = run(96_000.0, 2);
    let y4 = run(192_000.0, 4);
    let ratio = l2_err(&y1, &y2) / l2_err(&y2, &y4);
    assert!(
        ratio > 3.6 && ratio < 4.4,
        "AmbientModalRoom expected O(h^2) ratio ~4.0, got {ratio:.3}"
    );
}
