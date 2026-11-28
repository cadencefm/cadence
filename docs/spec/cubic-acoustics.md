# Cubic Acoustics & Modular Physical Primitives

This specification defines Cadence's first-principles cubic-acoustic and Möbius mathematical primitive library (`crates/core/src/dsp/cubic/`), its modular decomposition onto Cadence's 4-column Eurorack Rack and Studio Mixer, and its deterministic `no_std`-compatible hardware DSP target profile.

> **Related specifications:** [DSP & Audio Engine](dsp-and-audio-engine.md) · [Material & Timing](material-and-timing.md) · [Decks & Interaction](decks-and-interaction.md) · [Verification](../verification.md)

---

## 1. Design Philosophy & Mathematical Guarantees

Conventional software and hardware synthesizers rely on heterogeneous approximations—mipmapped wavetables, PolyBLEP discontinuity patches, oversampled `tanh` clips, iterative Newton-Raphson ZDF loops, and delay-line reverb networks—each requiring separate anti-aliasing workarounds and stabilization clamps.

Cadence's cubic-acoustic primitive set derives oscillators, envelopes, resonant filters, nonlinear modal bodies, self-oscillating wind exciters, physical waveshapers, and delay-free reverberant rooms from one compact family of exact closed-form local flows and rational Lie-group transformations:

1. **Analytically Exact Local Flows:** Every elementary sub-step—linear + bare-cubic + State-Kerr free evolution, Riccati shock clamping above ambient headroom $P_{\text{atm}}$, opposing-biased pair diffusion, 0-Hz RC/cubic capacitor charge and discharge, rational $SO(2)$ Cayley phase rotation, rational $SO(3)$ Cayley-Rodrigues three-wave triads, one-shot quadratic ZDF diode ladder solve, and exact Bernoulli orifice volume transfer—is solved in closed form with zero Newton iterations and zero sub-step discretization error.
2. **Global Second-Order Convergence ($O(h^2)$) at $1\times$ Sample Rate:** Non-commuting local operators are composed via time-symmetric Strang or 2-sample palindromic schedules, achieving global $O(h^2)$ sample-rate convergence at native `48 kHz` with zero oversampling and zero wavetable ROM.
3. **Unconditional Passivity & Zero Hidden Clamps:** Internal signal paths never clip or clamp state variables. Ambient linear headroom $P_{\text{atm}}$ yields exact linear exponential behavior (zero intermodulation distortion) for $|w| \le P_{\text{atm}}$ and transitions smoothly into exact Riccati shock contraction above $P_{\text{atm}}$, bounding energy physically without artificial limiters.
4. **Zero-Comb Quadrature Stereo:** Complex modal and filter states expose analytic quadrature companions (`(x, y)` or `(y, quad_x)`) that project into stereo `[L, R]` whose mono sum $(L + R)/2$ is algebraically free of comb filtering at every spread angle.
5. **Single Pure Core for Desktop, Web, and Bare-Metal Hardware:** All primitives operate in fixed-capacity stack/boxable structs with zero dynamic allocation in hot paths, zero locks, zero panics, and zero third-party DSP crate dependencies—ready for desktop `f64`, WASM AudioWorklet, and `no_std` Cortex-M7 Eurorack firmware.

---

## 2. Core Primitive Library (`crates/core/src/dsp/cubic/`)

All pure math lives in `crates/core/src/dsp/cubic/`, partitioned into six orthogonal submodules:

| Submodule | Primary Primitives | Core Mathematical Invariants |
| --- | --- | --- |
| `math.rs` | `exprel_neg`, `log1p_div`, `cayley_rotation`, `omc_rotation_f32`, `atomic_pair_scalar`, `radial_pair_step`, `atomic_jet`, `ZeroHzCapacitor`, `cubic_phase_flow`, `cubic_periodic_phase_flow`, `cubic_directed_transfer`, `bernoulli_transfer_exact` | `< 1e-15` branch continuity; exact unit modulus `c^2 + s^2 == 1`; exact semigroup composition and time-reversibility; $f(0) = 0, f'(0) = 1$ for `atomic_jet`; cancellation-free `omc = 2 sin^2(theta/2)` for `f32` targets |
| `exciter.rs` | `pitch_guard_rho`, `MobiusCoeffs`, `mobius_exciter`, `mobius_pwm`, `bare_cubic_exciter`, `hardy_saw_exciter`, `szego_analytic`, `dipole_analytic`, `poisson_analytic`, `apply_chirality`, `StrikePulse`, `CayleyPhasor` | Exact unit peak `±1.0` and zero DC for real waveforms; $C^\infty$ continuous pitch-aware Nyquist reach guard pinning exterior singularities at `-104 dB`; $L^2$-normalized Hardy-space complex analytic drivers; zero-trig rational glide |
| `filter.rs` | `ImLpfStage`, `ImLpfCascade` (`2`/`3`/`4`-pole), `DiodeLadderFilter` (`C_1 = 2C`, post-sat `C17` HPF) | Butterworth continuous prototype at `res = 0, beta = 0, drag = 0`; exact trapezoidal stage handoff (`O(h^2)`); zero-comb `step_stereo`; 1-shot two-branch cancellation-free quadratic ZDF solve; `E[u_fb] == 0` post-sat DC rejection; exact pairwise & total charge conservation `2*v0 + v1 + v2 + v3` in diode bark |
| `modal.rs` | `BodyGeometry` (`Harmonic`, `Stopped`, `Membrane`, `Bar`, `Plate`, `Cluster`), `body_ratio`, `PairTopology` (`Star`, `Cascade`, `Bloom`, `Ring`, `Swarm`), `TriadAxis` (`Diff`, `Sum`, `Manley`), `ResonantTriads`, `build_resonant_triads`, `triad_rodrigues_pass`, `pair_exchange`, `ModalBank` (`MAX_ATOMS = 48`) | Scale-invariant deterministic Zakharov triad selection; orthogonal `SO(3)` energy conservation `< 1e-14`; exact joint Riccati + bare-cubic + Kerr free flow across the `|z| = P_atm` boundary; palindromic `O(h^2)` free decay; exact-adjoint reverse Gauss-Seidel driven sweeps |
| `aero.rs` | `FlueJet`, `BrassReed` | Single-voice self-oscillating physical wind primitives (no multi-rank organ chests/ranks): 1-complex-atom damped convective flue jet with SR-invariant continuous turbulence, and 2-way symplectic mechanical brass blade + centered $C^2$ rolling-shallot Bernoulli valve (`delta = 0.20 * x0`) conserving joint SI Joules to `< 1e-13` with exact `O(h^2)` convergence even during active beating |
| `room.rs` | `AmbientModalRoom` (`ROOM_MODES = 96`, `48` Even/Odd pairs) | Zero delay lines; hybrid log (`38..260 Hz`) + power-law (`260..9600 Hz`) modal lattice; opposing-biased ambient-air input port ($P_{\text{atm}} = 6.0$); physical air-absorption $RT_{60}(f) = 2.65 / (1 + (f/3600)^{1.45})\text{ s}$; symmetric Strang `D(h/2) -> F(h/2) -> I(h) -> F(h/2) -> D(h/2)` (`O(h^2)`); strict monotonic passivity; Fibonacci golden-angle stereo decorrelation |

### 2.1 Singularity-Free Flows & Ambient Headroom

Inside ambient headroom $|w| \le P_{\text{atm}}$, the opposing-biased scalar cubic pair flow

$$\frac{dw}{dt} = -\frac{\alpha}{2}\left((w + P)|w + P| + (w - P)|w - P|\right)$$

reduces identically to linear exponential relaxation $w^+ = w e^{-2 \eta P}$ ($\eta = \alpha h$), giving zero intermodulation distortion at nominal operating levels. Above headroom ($|w| > P$), it obeys the exact Riccati tangent solution with mid-step boundary crossing at $t_{\text{cross}} = \frac{1}{\alpha P}\operatorname{atan2}(|w| - P, |w| + P)$, clamping transient excursions back into $|w| \le P$ in finite time without hard clipping. At $P = 0$, it collapses smoothly to the bare-cubic reciprocal flow $w^+ = w / (1 + \eta |w|)$.

For complex modal atoms with simultaneous linear radiation $\lambda$, bare-cubic internal drag $g$, angular frequency $\omega$, and State-Kerr pitch bend $\beta$:

$$\frac{da}{dt} = -\lambda a - g a^2 \quad (a \le P_{\text{atm}}), \qquad \frac{da}{dt} = -(\alpha + g)a^2 - \alpha P_{\text{atm}}^2 \quad (a > P_{\text{atm}}), \qquad \frac{d\phi}{dt} = \omega(1 + \beta a)$$

`joint_free_kerr` integrates both amplitude and nonlinear phase in a single exact closed form using `exprel_neg` and `log1p_div`, handling mid-step crossing of $a = P_{\text{atm}}$ analytically with zero splitting error.

### 2.2 Algebraic & Hardy-Space Exciters (Zero Wavetables)

On a unit-circle phasor $u = c + i s = e^{i\theta}$ advanced by rational $SO(2)$ Cayley rotation during continuous pitch glides, the pitch-aware continuous $C^\infty$ harmonic reach guard

$$\rho_{\text{eff}} = \min\!\left(\rho, \exp\!\left(-\frac{24 f}{f_s}\right)\right)$$

pins the nearest exterior complex singularity so Nyquist energy remains below $-104\text{ dB}$ across all pitches with zero harmonic-count staircases:

- **Möbius Jet (`mobius_exciter`):** $G(c, s; \rho, k) = \frac{N s}{\sqrt{(D c - 2\rho)^2 + (N s)^2}}$, where $D = (1+k) + (1-k)\rho^2$ and $N = (1+k) - (1-k)\rho^2$. Exact unit peak $\pm 1$, zero DC, smooth morph from sine ($\rho = 0$) through Blaschke saw ($k = 0$) to bright curved pulses ($k > 0$).
- **Möbius PWM (`mobius_pwm`):** Phase-shifted difference $\frac{1}{2}\big(G(\theta) - G(\theta - 2\pi w)\big)$ giving exact zero-DC pulse-width modulation.
- **Bare-Cubic Saw (`bare_cubic_exciter`):** Zero-`sqrt` rational readout $B(c, s; r, k) = \frac{(\sqrt{1-r^2} + kr)\, s}{1 - rc + kr|s|}$ with $1/n^3$ high-frequency rolloff and exact $\pm 1$ peak.
- **Hardy Saw (`hardy_saw_exciter`):** $H(c, s; r) = \operatorname{atan2}(r s, 1 - r c) / \arcsin(r)$, exact logarithmic sawtooth with $\pm 1$ peak.
- **Complex Analytic Hardy Drivers (`szego_analytic`, `dipole_analytic`, `poisson_analytic`):** Exact $L^2$-normalized complex waveforms $(x, y)$ with chirality mixing for driving complex modal banks directly across stereo/quadrature cables.

### 2.3 Resonant Filters (`ImLpfCascade` & `DiodeLadderFilter`)

- **Path A — `ImLpfCascade` (`2`, `3`, or `4`-pole):** Cascaded driven complex `CubicAtom` stages with readout $y = \operatorname{Im}(z)$, exact $0\text{ dB}$ DC gain, Butterworth magnitude response in the linear limit ($Q_2 = 1/\sqrt{2}$, $Q_{4a} = 1/(2\cos\frac{3\pi}{8})$, $Q_{4b} = 1/(2\cos\frac{\pi}{8})$), trapezoidal inter-stage handoff $y_{\text{mid}} = \frac{1}{2}(y_{\text{prev}} + y_{\text{next}})$ for global $O(h^2)$, State-Kerr pitch-bending resonance, bare-cubic peak self-limiting, and zero-comb quadrature stereo readout $L/R = \cos(\psi/2)\, y \pm \sin(\psi/2)\, q_x$.
- **Path B — `DiodeLadderFilter`:** Four-stage $C_1 = 2C$ passive diode RC ladder solved via 1-shot Thomas elimination over prewarped $g = \tan(\omega_c h / 2)$, $g_1 = g/2$, and a two-branch cancellation-free quadratic solve for the nonlinear feedback voltage $m_4$. Placing the series `C17` HPF (`72.3 Hz`) strictly *after* feedback saturation guarantees $\mathbb{E}[u_{\text{fb}}] \equiv 0$ even on asymmetric waveforms. Symmetric Strang bracketing with three reciprocal opposing-biased diode bark edges ($P = 0.16$) conserves total capacitor charge $2v_0 + v_1 + v_2 + v_3$ identically.

### 2.4 Modal Interaction Operators & Scale-Invariant $SO(3)$ Triads

`ModalBank` supports up to `MAX_ATOMS = 48` complex modes with six geometry families (`Harmonic`, `Stopped`, `Membrane` Bessel $J_m$ zeros, `Bar`, 2D Kirchhoff `Plate`, and `Cluster`), five pair-exchange topologies (`Star`, `Cascade`, `Bloom`, `Ring`, `Swarm`), and lossless three-wave resonant coupling:

$$\frac{dx_t}{dt} = 0, \qquad \frac{dy_t}{dt} = \gamma\, (D_t x_t) \times y_t$$

integrated via the exact rational $SO(3)$ Cayley-Rodrigues map $R(w) = (I - [w]_\times)^{-1}(I + [w]_\times)$ (`Diff`, `Sum`, or frequency-weighted `Manley` axes). `build_resonant_triads` snaps near-exact detunings $|\omega_a + \omega_b - \omega_c|/\omega_c \le 10^{-12}$ to `0.0` and breaks ties deterministically by lowest mode index $a$, making the triad graph strictly scale-invariant across fundamental frequencies.

### 2.5 Single-Voice Aeroacoustic Wind Primitives (`Aero`)

Multi-rank pipe organ wind chests, stop actions, and rank tables are intentionally out of scope for Cadence's core modular architecture. Single-voice physical wind excitation is provided directly by two self-contained primitives:

- **`FlueJet`:** One complex convective jet atom coupled two-way to a modal bore (`Harmonic` open pipe or `Stopped` odd-harmonic pipe) via `atomic_jet` labium deflection, `atomic_pair_scalar` mouth port, and sample-rate-invariant continuous-time turbulence ($\propto \sqrt{3/h}$), stepped in a symmetric Strang $O(h^2)$ schedule.
- **`BrassReed`:** Symplectic mechanical brass tongue coupled two-way to a conical modal bore and supply plenum through an exact cancellation-free Bernoulli orifice and $SO(3)$ Manley-Rowe bore triads. Reed closure uses a centered $C^2$ rolling-shallot aperture profile over $\delta = 0.20\, x_0$ ($\pm 0.10\, x_0$ around $h_{\text{raw}} = 0$):

  $$h_{\text{open}}(h_{\text{raw}}) = \begin{cases} 0 & h_{\text{raw}} \le -\delta/2 \\ \delta\, u^3\!\left(1 - \tfrac{1}{2}u\right),\; u = \frac{h_{\text{raw}} + \delta/2}{\delta} & |h_{\text{raw}}| < \delta/2 \\ h_{\text{raw}} & h_{\text{raw}} \ge \delta/2 \end{cases}$$

  guaranteeing exact zero leakage when seated, exact geometric aperture when open, joint SI Joule conservation (plenum + bore + reed), and strict $O(h^2)$ convergence even during active reed beating.

---

## 3. Architectural Decomposition Rule: Split vs. Fused Modules

Cadence's Eurorack patchbay executes modules in Kahn topological order with stereo `[f64; 2]` cables and $z^{-1}$ 1-sample delay on feedback cycles (`crates/core/src/rack/compile.rs`). Mapping physical acoustics onto this patchbay obeys one clean boundary rule:

- **One-way signal chains split cleanly across Rack cables:** An exciter (`Vco`), modulator (`Env`, `Lfo`), filter (`Filter`), modal resonator (`Modal`), waveshaper (`Drive`), amplifier (`Vca`), and spatial room (`ModalRoom`) exchange energy unidirectionally (or through deliberate $z^{-1}$ musical feedback). Cadence's existing `[f64; 2]` cables natively carry either stereo `[L, R]` audio or complex analytic `[re, im]` signals (e.g., Hardy-space `Vco` driving `Modal.In` with full chirality).
- **Stiff two-way energy-conserving couplings stay fused inside one module:** A flute jet coupled to its pipe mouth (`FlueJet`), a beating brass reed exchanging exact air volume with its bore (`BrassReed`), and the internal pair/triad/Riccati dynamics of a resonant body (`ModalBank`) cannot be split across a $z^{-1}$ cable without losing unconditional passivity and $O(h^2)$ convergence at $1\times$ sample rate. Therefore:
  - `Aero` fuses the single-voice wind exciter + its own primary bore, outputting stereo quadrature audio to feed any downstream `Filter`, `Modal`, `Drive`, or `Vca`.
  - `Modal` fuses the 48-mode bank + its internal pair/triad network + an optional built-in `StrikePulse` exciter on `TrigIn`, while also accepting arbitrary external stereo/complex audio on `In` (from `Vco`, `Noise`, `Sampler`, or `Aero`).

This gives full mix-and-match modular routing—such as a Möbius `Vco` through `DiodeLadder`, a `FlueJet` through a `Plate` `Modal` body and `Drive::AtomicJet`, or a `Strike`-excited `Membrane` kick with State-Kerr pitch drop—without compromising physical stability.

---

## 4. Mapping onto Cadence's Rack & Studio Mixer

### 4.1 Rack Modules (4-Column Eurorack Patchbay)

| Column | Module | Role & Cubic Capabilities |
| --- | --- | --- |
| `0` (`SOURCES`) | **`Vco`** | Extends/replaces classic waveforms with algebraic & Hardy-space exciters (`MobiusJet`, `MobiusPwm`, `BareCubic`, `HardySaw`, `Szego`, `Dipole`, `Poisson`, `Sine`) using continuous `rho` (brightness/reach), `shape` ($k$), `width`, `chirality`, $C^\infty$ pitch-aware Nyquist guard, and zero-trig Cayley pitch glide. Zero wavetables; supports real stereo or complex `[re, im]` output for `Modal`. |
| `0` (`SOURCES`) | **`Aero`** *(new)* | Self-contained physical wind source (`FlueJet` and `BrassReed`): `PitchCv`, `TrigIn` (gate), `Cv` (pressure/drive), bore tuning (`Harmonic`/`Stopped`), geometry/embouchure, brightness, radiation, Kerr, triads, stereo spread. |
| `1` (`MODULATORS`) | **`Env`** | Adds `EnvMode::Capacitor` backed by `ZeroHzCapacitor`: finite-resistance RC attack pulse + exact linear/bare-cubic self-limiting discharge (`tau_decay`, `g_cubic`, `tau_charge`) supporting both crisp filter decays and additive self-limiting `C13` accent accumulation. |
| `2` (`PROCESSORS`) | **`Filter`** | Adds `ImLpf2`, `ImLpf3`, `ImLpf4` (`ImLpfCascade` with State-Kerr resonance, bare-cubic peak control, and zero-comb quadrature stereo `spread`) and `DiodeLadder` (1-shot ZDF `C_1 = 2C` passive diode ladder with post-sat `72.3 Hz` `C17` HPF and charge-conserving diode bark). |
| `2` (`PROCESSORS`) | **`Modal`** *(replaces `Resonator`)* | Up to 48 complex modes (`ModalBank`), 6 geometries (`Harmonic`/`Stopped`/`Membrane`/`Bar`/`Plate`/`Cluster`), 5 pair topologies (`Star`/`Cascade`/`Bloom`/`Ring`/`Swarm`), lossless `SO(3)` triads (`Diff`/`Sum`/`Manley`), State-Kerr pitch drop, ambient headroom $P_{\text{atm}}$, built-in `StrikePulse` on `TrigIn` (hardness/geometry + velocity), external complex/stereo drive on `In`, `PitchCv`, `Cv`, and quadrature stereo readout. |
| `2` (`PROCESSORS`) | **`Drive`** | Adds `AtomicJet` (slope-invariant $C^\infty$ asymmetric jet shaper) and `ShockPort` (opposing-biased ambient-air headroom limiter with linear interior and Riccati outer knee) operating cleanly at $1\times$ sample rate. |

### 4.2 Studio Mixer Effect

| Effect | Slot | Role & Cubic Capabilities |
| --- | --- | --- |
| **`ModalRoom`** | Mixer Insert / Return (`StereoEffect`) | Delay-free 96-mode (`48` Even/Odd pairs) $U(1)$-symmetric `AmbientModalRoom`: zero delay-line RAM, strictly passive monotonic tail decay, physical frequency-dependent $RT_{60}(f)$, ambient headroom $P_{\text{atm}} = 6.0$ (zero port IMD), Fibonacci stereo decorrelation, and exact `is_quiet()` tail sleeping. |

---

## 5. Hardware & Eurorack DSP Target Profile

Because `crates/core/src/dsp/cubic/` uses zero external crates, zero heap allocations in processing loops, zero wavetable ROM, and zero delay-line SDRAM, the exact same Rust core compiles for bare-metal embedded targets (e.g., STM32H7 / Daisy Seed Cortex-M7 @ `480 MHz`, `1 MiB` internal SRAM, `48 kHz` audio ISR):

| Primitive | Working State (`f64`) | Working State (`f32`) | Per-Sample Complexity (1× 48 kHz) |
| --- | --- | --- | --- |
| `CayleyPhasor` + `mobius_exciter` | ~128 B | ~64 B | 1 `sqrt`, 0 `sin`/`cos` during glide, 0 table lookups |
| `ZeroHzCapacitor` | ~64 B | ~32 B | 1 multiply-add or 1 reciprocal (`~10` cycles) |
| `ImLpfCascade` (4-pole stereo) | ~200 B | ~100 B | 2 complex stages, 0 Newton loops, exact quadrature stereo |
| `DiodeLadderFilter` (4-stage ZDF) | ~96 B | ~48 B | 1 Thomas sweep + 1 `sqrt` + 6 bark edges, 0 Newton loops |
| `ModalBank` (16 modes / 48 modes) | ~4.5 KB / ~12 KB | ~2.3 KB / ~6 KB | Linear in $N$ modes + $O(N)$ sparse triads, zero heap |
| `FlueJet` / `BrassReed` (12–16 modes) | ~4.5 KB | ~2.3 KB | Exact closed-form bore + jet/valve, zero oversampling |
| `AmbientModalRoom` (96 modes) | ~18 KB | ~9 KB | 48 Even/Odd pairs, **0 bytes** external delay SDRAM |

**Single-Precision (`f32`) Hardware Precision Guard:** On `f32`-only MCUs or `f32` SIMD lanes, low-frequency rotations (`< 200 Hz`) suffer catastrophic cancellation if stored as `(cos θ, sin θ)` because `cos θ ≈ 1.0`. `math::omc_rotation_f32` provides the cancellation-free `(omc, sin)` representation where $\operatorname{omc} = 2\sin^2(\theta/2) = 1 - \cos\theta$, updating $(x, y) \mapsto (x - \operatorname{omc}\cdot x - \sin\cdot y,\; y - \operatorname{omc}\cdot y + \sin\cdot x)$ with full 24-bit mantissa precision down to sub-Hz frequencies.

---

## 6. Staged Delivery & Verification Plan

### Step 1 — Pure Primitive Core (`crates/core/src/dsp/cubic/`)
- Implement all six pure, zero-allocation submodules (`math`, `exciter`, `filter`, `modal`, `aero`, `room`) inside `crates/core/src/dsp/cubic/` and export them cleanly from `crates/core/src/dsp/mod.rs`.
- Keep large fixed-array structs (`ModalBank`, `AmbientModalRoom`) boxable (`Box::new` constructors) to uphold debug stack safety across the workspace.
- Verify every mathematical invariant in `crates/core/src/dsp/cubic/tests.rs`:
  - Elementary branch continuity (`< 1e-15`), rational `SO(2)` unit modulus & $O(w^5)$ phase accuracy, and `f32` `omc` rotation accuracy.
  - Exciter unit peaks, zero DC, exact endpoints, $L^2$ Hardy normalization, and `-100 dB` Nyquist reach guard across pitches.
  - `joint_free_kerr` exact RK4 match and cross-boundary semigroup consistency across `|z| = P_atm`.
  - `ZeroHzCapacitor` exact ODE match and self-limiting accumulation.
  - `pair_exchange` mean preservation and passivity; rational phase flows semigroup & reversibility; `cubic_directed_transfer` exact energy conservation & telescoping.
  - `SO(3)` Cayley-Rodrigues triad orthogonality (`< 1e-14`), exact reversibility, scale-invariant `build_resonant_triads` across fundamental frequencies, and upward sum-frequency transfer.
  - `ImLpfCascade` 2/3/4-pole Butterworth DC & `-3.01 dB` cutoff response, zero-comb stereo mono sum, and global $O(h^2)$ refinement.
  - `DiodeLadderFilter` global $O(h^2)$, post-sat `C17` zero-DC feedback return, exact charge conservation `2*v0 + v1 + v2 + v3`, and extreme overdrive stability (`100×` drive).
  - `ModalBank` global $O(h^2)$ across all 5 pair topologies, `Membrane` State-Kerr kick pitch drop, and `Plate` triad cymbal wash.
  - `FlueJet` damped convective stability & $O(h^2)$; `BrassReed` $O(h^2)$ (including active beating via centered $C^2$ rolling-shallot closure) and joint SI Joule conservation.
  - `AmbientModalRoom` global $O(h^2)$, strict monotonic zero-input passivity, stereo decorrelation (low IACC), and silence drain.

### Step 2 — Rack, Mixer, Engine, App & GUI Integration
- Wire `Vco`, `Aero`, `Env::Capacitor`, `Filter::{ImLpf, DiodeLadder}`, `Modal` (cleanly replacing `Resonator` with zero legacy shims), `Drive::{AtomicJet, ShockPort}`, and Mixer `ModalRoom` across `cadence-core`, `cadence-engine`, `cadence-app`, and `cadence-gui`.
- Update parameter metadata, CLI/Ex commands, modulation targets, MIDI Learn, signal graph, visualizers, and factory presets symmetrically.

### Step 3 — Embedded `no_std` Target Extraction
- Gate desktop-only dependencies cleanly so `cadence-core::dsp::cubic` builds under `#![no_std]` with `libm` and verified `f32`/SIMD cycle counts for Cortex-M7 Eurorack firmware.
