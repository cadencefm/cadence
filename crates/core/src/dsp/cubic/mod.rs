//! First-principles cubic-acoustic and Mobius mathematical DSP primitives.
//!
//! Pure, zero-dependency, zero-allocation real-time primitives providing exact
//! closed-form local flows, rational `SO(2)` / `SO(3)` Lie-group rotations,
//! algebraic and Hardy-space unit-circle exciters, zero-delay-feedback resonant
//! filters, nonlinear modal bodies, single-voice aeroacoustic wind sources, and
//! a delay-free 96-mode ambient-air reverberant room.

pub mod aero;
pub mod exciter;
pub mod filter;
pub mod math;
pub mod modal;
pub mod room;

#[cfg(test)]
mod tests;

pub use aero::{BrassReed, BrassReedConfig, FlueJet, FlueJetConfig, ReedEnergyLedger};
pub use exciter::{
    apply_chirality, bare_cubic_exciter, dipole_analytic, hardy_saw_exciter, mobius_exciter,
    mobius_exciter_coeffs, mobius_pwm, pitch_guard_rho, poisson_analytic,
    strike_time_from_geometry, szego_analytic, CayleyPhasor, CubicWaveform, HardyCoeffs,
    MobiusCoeffs, StrikePulse,
};
pub use filter::{
    DiodeLadderFilter, ImLpfCascade, ImLpfPoles, ImLpfStage, BARK_THRESHOLD, DEFAULT_HPF_HZ,
};
pub use math::{
    apply_omc_rotation_f32, atomic_jet, atomic_pair_scalar, bernoulli_supply_exact,
    bernoulli_valve_exact, cayley_rotation, cubic_directed_transfer, cubic_periodic_phase_flow,
    cubic_phase_flow, exprel_neg, joint_free_kerr, joint_free_radial, log1p_div, omc_rotation_f32,
    radial_pair_step, rolling_shallot_aperture, xorshift64_bipolar, DirectedTransfer,
    ZeroHzCapacitor, AIR_RHO, DEFAULT_RNG_SEED, MAX_ATOMS, NOMINAL_CHEST_PA, SQRT3, TAU,
};
pub use modal::{
    body_ratio, build_resonant_triads, pair_exchange, plate_candidates, triad_rodrigues_pass,
    BodyGeometry, ModalBank, ModalConfig, PairTopology, ResonantTriads, TriadAxis, BAR_RATIOS,
    BESSEL_RATIOS, CLUSTER_RATIOS, MAX_TRIADS,
};
pub use room::{
    AmbientModalRoom, ROOM_F_MAX, ROOM_F_MIN, ROOM_F_SPLIT, ROOM_HALF, ROOM_MODES, ROOM_PATM,
};
