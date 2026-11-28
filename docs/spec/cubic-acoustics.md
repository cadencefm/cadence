# Cubic Acoustics & Modular Physical Primitives

This specification defines Cadence's first-principles cubic-acoustic and Möbius mathematical primitive library (`crates/core/src/dsp/cubic/`), its full native `1×`-sample-rate integration onto Cadence's 4-column Eurorack Rack and Studio Mixer, and its deterministic `no_std`-compatible hardware DSP target profile.

> **Related specifications:** [DSP & Audio Engine](dsp-and-audio-engine.md) · [Material & Timing](material-and-timing.md) · [Decks & Interaction](decks-and-interaction.md) · [Verification](../verification.md)

---

## 1. Design Philosophy & Mathematical Guarantees

Conventional software and hardware synthesizers rely on heterogeneous approximations—mipmapped wavetables, PolyBLEP discontinuity patches, oversampled `tanh` clips, iterative Newton-Raphson ZDF loops, and delay-line reverb networks—each requiring separate anti-aliasing workarounds and stabilization clamps.

Cadence's cubic-acoustic engine derives oscillators, envelopes, resonant filters, nonlinear modal bodies, self-oscillating wind exciters, physical waveshapers, and delay-free reverberant rooms from one compact family of exact closed-form local flows and rational Lie-group transformations:

1. **Analytically Exact Local Flows:** Every elementary sub-step—linear + bare-cubic + State-Kerr free evolution, Riccati shock clamping above ambient headroom $P_{\text{atm}}$, opposing-biased pair diffusion, 0-Hz RC/cubic capacitor charge and discharge, rational $SO(2)$ Cayley phase rotation, rational $SO(3)$ Cayley-Rodrigues three-wave triads, one-shot quadratic ZDF diode ladder solve, and exact Bernoulli orifice volume transfer—is solved in closed form with zero Newton iterations and zero sub-step discretization error.
2. **Pure `1×` Sample-Rate Execution (Zero Oversampling):** Non-commuting local operators compose via time-symmetric Strang or 2-sample palindromic schedules, achieving global $O(h^2)$ convergence at native `48 kHz` with **zero oversampling**, **zero halfband filters**, and **zero wavetable ROM** anywhere in the real-time signal path.
3. **Unconditional Passivity & Zero Hidden Clamps:** Internal signal paths never clip or clamp state variables. Ambient linear headroom $P_{\text{atm}}$ yields exact linear exponential behavior (zero intermodulation distortion) for $|w| \le P_{\text{atm}}$ and transitions smoothly into exact Riccati shock contraction above $P_{\text{atm}}$, bounding energy physically without artificial limiters.
4. **Unified M/S-Encoded Quadrature Cables:** Complex quadrature signals encode onto Cadence's existing `[f64; 2]` stereo cables as $L = \text{re} + \text{im}$, $R = \text{re} - \text{im}$ so $\text{Mid} = \frac{1}{2}(L + R) \equiv \text{re}$ and $\text{Side} = \frac{1}{2}(L - R) \equiv \text{im}$. This guarantees constant RMS power across all `chirality`, algebraically zero-comb mono collapse, transparent passage through `Vca`/`Drive`/`Filter`/`Out`/meters with zero extra jacks, and exact complex `(re, im)` excitation into `Modal.In`.
5. **Single Pure Core for Desktop, Web, and Bare-Metal Hardware:** All primitives operate in fixed-capacity stack/boxable structs with zero dynamic allocation in hot paths, zero locks, zero panics, and zero third-party DSP crate dependencies—ready for desktop `f64`, WASM AudioWorklet, and `no_std` Cortex-M7 Eurorack firmware.

---

## 2. Core Primitive Library (`crates/core/src/dsp/cubic/`)

All pure math lives in `crates/core/src/dsp/cubic/`, partitioned into six orthogonal submodules:

| Submodule | Primary Primitives | Core Mathematical Invariants |
| --- | --- | --- |
| `math.rs` | `exprel_neg`, `log1p_div`, `cayley_rotation`, `omc_rotation_f32`, `atomic_pair_scalar`, `radial_pair_step`, `atomic_jet`, `ZeroHzCapacitor`, `cubic_phase_flow`, `cubic_periodic_phase_flow`, `cubic_directed_transfer`, `bernoulli_valve_exact`, `bernoulli_supply_exact`, `rolling_shallot_aperture` | `< 1e-15` branch continuity; exact unit modulus `c^2 + s^2 == 1`; exact semigroup composition and time-reversibility; $f(0) = 0, f'(0) = 1$ for `atomic_jet`; cancellation-free `omc = 2 sin^2(theta/2)` for `f32` targets |
| `exciter.rs` | `pitch_guard_rho`, `MobiusCoeffs`, `mobius_exciter`, `mobius_pwm`, `bare_cubic_exciter`, `hardy_saw_exciter`, `szego_analytic`, `dipole_analytic`, `poisson_analytic`, `apply_chirality`, `StrikePulse`, `CayleyPhasor` | Exact unit peak `±1.0` and zero DC for real waveforms; $C^\infty$ continuous pitch-aware Nyquist reach guard pinning first aliased harmonic below `-98 dB`; $L^2$-normalized Hardy-space complex analytic drivers; zero-trig rational glide |
| `filter.rs` | `ImLpfStage`, `ImLpfCascade` (`2`/`3`/`4`-pole), `DiodeLadderFilter` (`C_1 = 2C`, post-sat `C17` HPF) | Butterworth continuous prototype at `res = 0, beta = 0, drag = 0`; exact trapezoidal stage handoff (`O(h^2)`); zero-comb `step_stereo`; 1-shot two-branch cancellation-free quadratic ZDF solve; `E[u_fb] == 0` post-sat DC rejection; exact pairwise & total charge conservation `2*v0 + v1 + v2 + v3` in diode bark |
| `modal.rs` | `BodyGeometry` (`Harmonic`, `Stopped`, `Membrane`, `Bar`, `Plate`, `Cluster`), `body_ratio`, `PairTopology` (`Star`, `Cascade`, `Bloom`, `Ring`, `Swarm`), `TriadAxis` (`Diff`, `Sum`, `Manley`), `ResonantTriads`, `build_resonant_triads`, `triad_rodrigues_pass`, `pair_exchange`, `ModalBank` (`MAX_ATOMS = 48`) | Scale-invariant deterministic Zakharov triad selection; orthogonal `SO(3)` energy conservation `< 1e-14`; exact joint Riccati + bare-cubic + Kerr free flow across the `|z| = P_atm` boundary; palindromic `O(h^2)` free decay; exact-adjoint reverse Gauss-Seidel driven sweeps |
| `aero.rs` | `FlueJet`, `BrassReed` | Single-voice self-oscillating physical wind primitives (no multi-rank organ chests/ranks): 1-complex-atom damped convective flue jet with SR-invariant continuous turbulence, and 2-way symplectic mechanical brass blade + centered $C^2$ rolling-shallot Bernoulli valve (`delta = 0.20 * x0`) conserving joint SI Joules to `< 1e-13` with exact `O(h^2)` convergence even during active beating |
| `room.rs` | `AmbientModalRoom` (`ROOM_MODES = 96`, `48` Even/Odd pairs) | Zero delay lines; hybrid log (`38..260 Hz`) + power-law (`260..9600 Hz`) modal lattice parameterized by `size`, `decay_seconds`, `damping_hz`, and `diffusion`; opposing-biased ambient-air input port ($P_{\text{atm}} = 6.0$); physical air-absorption $RT_{60}(f) = T_0 / (1 + (f/f_d)^{1.45})$; symmetric Strang `D(h/2) -> F(h/2) -> I(h) -> F(h/2) -> D(h/2)` (`O(h^2)`); strict monotonic passivity; Fibonacci golden-angle stereo decorrelation |

---

## 3. Architectural Decomposition Rule: Split vs. Fused Modules

Cadence's Eurorack patchbay executes modules in Kahn topological order with stereo `[f64; 2]` cables and $z^{-1}$ 1-sample delay on feedback cycles (`crates/core/src/rack/compile.rs`). Mapping physical acoustics onto this patchbay obeys two clean rules:

1. **M/S-Encoded Quadrature on `[f64; 2]` Cables:** Every cable carries `[L, R]` where complex quadrature sources encode $L = \text{re} + \text{im}$, $R = \text{re} - \text{im}$, so any consumer can read stereo directly or recover exact $(\text{re}, \text{im}) = (\frac{L+R}{2}, \frac{L-R}{2})$ with constant RMS across `chirality` and zero-comb mono collapse.
2. **Stiff Two-Way Couplings Stay Fused Inside One Module:** One-way chains (`Vco → Filter → Modal → Drive → Vca → Out`) split freely across patchbay cables, while stiff two-way energy-conserving loops (convective flue jet ↔ bore; symplectic brass reed ↔ Bernoulli valve ↔ bore; internal modal pair/triad/Riccati dynamics) stay fused inside `Aero` and `Modal` to guarantee unconditional passivity, SI Joule conservation, and $O(h^2)$ convergence at `1×` sample rate.

---

## 4. Rack & Studio Mixer Integration Contract

Every superseded legacy engine (`PolyBLEP`, `AcidLadder`, SVF LP, `ModalResonator`, FDN `Reverb`, `Drive::{Tape, HardClip, Fold}`, and all `Halfband2x` oversampling) is replaced cleanly across `cadence-core`, `cadence-engine`, `cadence-app`, `cadence-gui`, `cadence-formats`, `examples/`, and `presets/`.

### 4.1 Rack Modules (4-Column Eurorack Patchbay)

| Column | Module | Ports | Contract & Parameters |
| --- | --- | --- | --- |
| `0` (`SOURCES`) | **`Vco`** | `PitchCv`, `FmIn`, `PwCv` | Pure `1×` cubic oscillator (zero PolyBLEP, zero 2× FM oversampling). `Waveform::{MobiusJet, MobiusPwm, BareCubic, HardySaw, Sine, Szego, Dipole, Poisson}` with automatic $C^\infty$ pitch-aware Nyquist guard; adds `reach` (`0..0.995`), `shape` (`0..1`: curvature $k$ / Dipole-Poisson geometry), and `chirality` (`-1..+1` M/S quadrature); keeps `pulse_width` + `PwCv` (PWM width on `MobiusPwm`, reach mod otherwise), `tune`, `fine`, `phase`, `level`, `pitch_tracking`, `fixed_hz`, unison (`1..=7`), and `1×` TZ-FM (`FmIn`). |
| `0` (`SOURCES`) | **`Aero`** *(new)* | `PitchCv`, `TrigIn`, `Cv` | Unified single-voice physical wind source (`AeroMode::{FlueJet, BrassReed}`): `Cv` modulates blowing drive/pressure; unpatched `TrigIn` tracks voice gate; exposes full physical controls (`geometry`, `atoms`, `pitch_tracking`/`tune`/`fine`/`fixed_hz`, `drive`, `embouchure`, `p_beat_pa`, `brightness`, `absorption`, `radiation`, `kerr`, `triad_amount`, `spread`, `level`); heap-boxed state with automatic bore-tail voice lifetime. |
| `1` (`MODULATORS`) | **`Env`** | `TrigIn` | Adds `EnvMode::Capacitor` (`ZeroHzCapacitor`: `attack_ms` → `tau_charge`, `hold_ms` → pulse width, `decay_ms` → `tau_decay`), explicit `cubic_limit` ($g_{\text{cubic}} \ge 0$), and `TriggerMode::Additive` (self-limiting `C13` accent stacking alongside `Gate` and `OneShot`). |
| `2` (`PROCESSORS`) | **`Filter`** | `In`, `Cv` | Shared `FilterMode::{ImLpf2, ImLpf3, ImLpf4, DiodeLadder, BandPass, HighPass, LowPass6}` (`ImLpf2` default). `drive` maps to diode bark / `ImLpf` drag; adds `kerr` (`-12..+12 st`) and quadrature M/S `spread` (`0..1`) symmetrically across Rack `FilterModule` and Mixer `FilterPatch`. Exact analytical `filter_transfer` shared with UI curves. |
| `2` (`PROCESSORS`) | **`Drive`** | `In`, `Cv` | Pure `1×`-rate shared `DriveMode::{AtomicJet, ShockPort}` (zero `Halfband2x` oversampling). Retains `drive`, `asymmetry`, `tone_hz`, `output_db`, `mix`, and `DcBlocker` with exact `f(x, 0, a) == x` identity and shared UI transfer curves. |
| `2` (`PROCESSORS`) | **`Modal`** *(replaces `Resonator`)* | `In`, `PitchCv`, `TrigIn`, `Cv` | Full 48-mode physical `ModalBank`: `geometry`, `atoms`, `pitch_tracking`/`tune`/`fine`/`fixed_hz`, `absorption`, `radiation`, `brightness`, `headroom`, `kerr`, `pair_topology`, `coupling`, `triad_amount`, `triad_axis`, `spread`, `strike_drive`, `strike_hardness`, `mix`. Unpatched `In` fires built-in `StrikePulse` on trigger/`TrigIn`; patched `In` drives M/S complex wave excitation `(Mid, Side)` with optional layered `Strike` transient; automatic energy-based voice tail lifetime. |

### 4.2 Studio Mixer Effects

| Effect | Contract & Parameters |
| --- | --- |
| **`Reverb`** | Replaces FDN completely with a parameterized zero-delay 96-mode `AmbientModalRoom` (`0` bytes of delay buffers): `size` (inverse modal frequency lattice scale), `decay_seconds` ($RT_{60}(0)$), `damping_hz` (air-absorption crossover knee), `diffusion` (`0..1` Even/Odd coupling rate, replacing `predelay_ms`), `width` (Fibonacci stereo decorrelation), and `mix`. |
| **`Drive`** | Pure `1×`-rate `DriveMode::{AtomicJet, ShockPort}` matching Rack `Drive` identically. |
| **`Filter`** | Shared `FilterMode::{ImLpf2, ImLpf3, ImLpf4, DiodeLadder, BandPass, HighPass, LowPass6}` + `kerr` and `spread`, sharing `MultiModeFilter` with Rack `Filter`. |

---

## 5. Hardware & Eurorack DSP Target Profile

Because `crates/core/src/dsp/cubic/` uses zero external crates, zero heap allocations in processing loops, zero wavetable ROM, zero oversampling, and zero delay-line SDRAM, the same Rust core compiles cleanly for bare-metal Cortex-M7 targets (e.g., STM32H7 / Daisy Seed @ `480 MHz`, `1 MiB` SRAM, `48 kHz` audio ISR):

| Primitive | Working State (`f64`) | Working State (`f32`) | Per-Sample Complexity (`1× 48 kHz`) |
| --- | --- | --- | --- |
| `CayleyPhasor` + `mobius_exciter` | ~128 B | ~64 B | 1 `sqrt`, 0 `sin`/`cos` during glide, 0 table lookups |
| `ZeroHzCapacitor` | ~64 B | ~32 B | 1 multiply-add or 1 reciprocal |
| `ImLpfCascade` (4-pole stereo) | ~200 B | ~100 B | 2 complex stages, 0 Newton loops, exact quadrature stereo |
| `DiodeLadderFilter` (4-stage ZDF) | ~96 B | ~48 B | 1 Thomas sweep + 1 `sqrt` + 6 bark edges, 0 Newton loops |
| `ModalBank` (16 / 48 modes) | ~4.5 KB / ~12 KB | ~2.3 KB / ~6 KB | Linear in $N$ modes + $O(N)$ sparse triads, zero heap |
| `FlueJet` / `BrassReed` (12–16 modes) | ~4.5 KB | ~2.3 KB | Exact closed-form bore + jet/valve, zero oversampling |
| `AmbientModalRoom` (96 modes) | ~18 KB | ~9 KB | 48 Even/Odd pairs, **0 bytes** external delay SDRAM |

Single-precision (`f32`) hardware targets use `math::omc_rotation_f32` ($\operatorname{omc} = 2\sin^2(\theta/2)$) to preserve full 24-bit mantissa precision at sub-bass frequencies.

---

## 6. Staged Delivery & Verification Plan

- **Step 1 (Complete):** Pure primitive core `crates/core/src/dsp/cubic/` (`math`, `exciter`, `filter`, `modal`, `aero`, `room`) + 27-test mathematical & $O(h^2)$ verification suite.
- **Step 2A:** Parameterized Mixer `Reverb` (`AmbientModalRoom`, replacing FDN & `predelay_ms` → `diffusion`), `1×` `Drive` (`AtomicJet`/`ShockPort`, deleting `Halfband2x` from Drive), shared `Filter` (`ImLpf2/3/4` + `DiodeLadder` + `kerr`/`spread`, deleting `AcidLadder` & SVF LP) + migrated callers, visualizers, `rbs`, examples, presets, and GPU screenshot verification.
- **Step 2B:** `1×` cubic `Vco` (8 waveforms + `reach`/`shape`/`chirality` + M/S quadrature + `1×` TZ-FM, deleting PolyBLEP & remaining `Halfband2x`), `Env::Capacitor` (`cubic_limit` + `TriggerMode::Additive`), new `Aero` module (`FlueJet`/`BrassReed`) + migrated callers, visualizers, `rbs`, examples, presets, and GPU screenshot verification.
- **Step 2C:** `Modal` replacing `Resonator` (48-mode `ModalBank` + `StrikePulse` + M/S complex wave excitation, deleting `ModalResonator`) + migrated drum/kit presets & examples + new cubic showcase presets (`Cubic 303`, `Kerr Kick`, `Plate Cymbal`, `Flue`, `Brass Reed`, `Szego Pad`) + visualizers and GPU screenshot verification.
- **Step 3:** Embedded `#![no_std]` target extraction (`libm` + `const` Kirchhoff plate table + `f32` Cortex-M7 benchmark harness).
