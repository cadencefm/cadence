//! Self-contained per-module voice nodes, uniform [`VoiceModule`] lifecycle trait,
//! and exhaustive zero-cost static dispatch (`dispatch_module_node!`).

mod drive;
mod env;
mod filter;
mod lfo;
mod lofi;
mod noise;
mod out;
mod resonator;
mod sampler;
mod vca;
mod vco;

pub(super) use drive::DriveNode;
pub(super) use env::EnvNode;
pub(super) use filter::FilterNode;
pub(super) use lfo::LfoNode;
pub(super) use lofi::LoFiNode;
pub(super) use noise::NoiseNode;
pub(super) use out::OutNode;
pub(super) use resonator::ResonatorNode;
pub(super) use sampler::SamplerNode;
pub(super) use vca::VcaNode;
pub(super) use vco::VcoNode;

use super::{ModuleParams, VoiceLockKey};
use cadence_core::dsp::{midi_to_hz, sanitize_clamp_f64, semitones_to_ratio_f64, ModalResonator};
use cadence_core::rack::{ModuleKind, ModuleKindTag, ModuleParam, PitchTracking};
use cadence_core::runtime::SampleBank;
use cadence_core::sampler::SampleData;
use std::sync::Arc;

/// Rising/falling gate trigger threshold for `InPort::TrigIn` edge detection.
pub(super) const TRIG_THRESHOLD: f64 = 0.5;

/// Inputs for resolving a pitch-tracked (`Fixed` or `Chromatic`) module's base frequency.
#[derive(Clone, Copy)]
pub(super) struct TrackedPitch {
    pub mode: PitchTracking,
    pub fixed_hz: f64,
    pub note_pitch: f64,
    pub rack_tune: f64,
    pub tune_semitones: f32,
    pub fine_cents: f32,
    pub pitch_cv: f64,
}

/// Compute the cache-keyable `(pitch_arg, fixed_hz_for_mode)` coordinate for a pitch-tracked node:
/// - `Fixed`: `(semitone_offset ∈ [-96, 96], sanitized fixed_hz)`
/// - `Chromatic`: `(midi_note ∈ [-64, 180], 0.0)`
#[inline]
pub(super) fn tracked_pitch_arg(p: TrackedPitch) -> (f64, f64) {
    let offset =
        p.rack_tune + f64::from(p.tune_semitones) + f64::from(p.fine_cents) * 0.01 + p.pitch_cv;
    match p.mode {
        PitchTracking::Fixed => (
            sanitize_clamp_f64(offset, -96.0, 96.0, 0.0),
            sanitize_clamp_f64(p.fixed_hz, 0.0, 1e9, 440.0),
        ),
        PitchTracking::Chromatic => (
            sanitize_clamp_f64(p.note_pitch + offset, -64.0, 180.0, 69.0),
            0.0,
        ),
    }
}

/// Convert a [`tracked_pitch_arg`] coordinate into a sanitized frequency clamped to `[min_hz, max_hz]`.
#[inline]
pub(super) fn tracked_pitch_arg_to_hz(
    mode: PitchTracking,
    pitch_arg: f64,
    fixed_hz: f64,
    min_hz: f64,
    max_hz: f64,
    fallback_hz: f64,
) -> f64 {
    let raw_hz = match mode {
        PitchTracking::Fixed => fixed_hz * semitones_to_ratio_f64(pitch_arg),
        PitchTracking::Chromatic => midi_to_hz(pitch_arg),
    };
    sanitize_clamp_f64(raw_hz, min_hz, max_hz, fallback_hz)
}

/// Resolve a pitch-tracked (`Fixed` or `Chromatic`) module's base frequency in Hz,
/// clamped safely into `[min_hz, max_hz]`.
#[inline]
pub(super) fn resolve_tracked_pitch_hz(
    pitch: TrackedPitch,
    min_hz: f64,
    max_hz: f64,
    fallback_hz: f64,
) -> f64 {
    let (pitch_arg, fixed_hz) = tracked_pitch_arg(pitch);
    tracked_pitch_arg_to_hz(pitch.mode, pitch_arg, fixed_hz, min_hz, max_hz, fallback_hz)
}

/// Read-only view of the voice's inline per-voice P-lock table.
#[derive(Clone, Copy)]
pub(super) struct VoiceLocks<'a> {
    keys: &'a [Option<VoiceLockKey>],
}

impl<'a> VoiceLocks<'a> {
    #[inline]
    pub(super) fn new(keys: &'a [Option<VoiceLockKey>]) -> Self {
        Self { keys }
    }

    #[inline]
    pub(super) fn is_module_locked(self, module_id: u16, param: ModuleParam) -> bool {
        self.keys
            .contains(&Some(VoiceLockKey::Module { module_id, param }))
    }
}

/// Immutable per-slot context passed into [`VoiceModule::on_trigger`].
#[derive(Clone, Copy)]
pub(super) struct TriggerCtx {
    pub sample_rate: f64,
    pub bpm: f64,
    pub mod_id: u16,
    pub base_seed: u32,
    pub clock: f64,
    pub vel: f32,
    pub target_note_pitch: f64,
    pub target_rack_tune: f64,
    pub phase_reset: bool,
    pub eff_retarget: bool,
    pub eff_legato: bool,
    pub has_fm_in: bool,
    pub has_trig_in: bool,
    pub has_cv: bool,
    pub acid_vel_amount: Option<f32>,
}

/// Shared mutable trigger-allocation state carried across module slots during `finalize_trigger`.
pub(super) struct TriggerState<'a> {
    pub resonator_banks: &'a mut [ModalResonator],
    pub next_res_bank: usize,
}

/// Summed stereo audio and CV inputs gathered from a module's incoming patch cables.
#[derive(Clone, Copy, Default)]
pub(super) struct NodeInputs {
    pub in_audio: [f64; 2],
    pub cv_stereo: [f64; 2],
    pub pitch_cv: f64,
    pub fm_cycles: f64,
    pub pw_cv: f64,
    pub trig_in: f64,
    pub filter_env_level: f64,
}

/// Shared per-frame voice context passed into [`VoiceModule::process`].
pub(super) struct ProcessCtx<'a> {
    pub sample_rate: f64,
    pub bpm: f64,
    pub note_pitch: f64,
    pub rack_tune: f64,
    pub velocity: f64,
    pub zone_pan: f32,
    pub control_tick: bool,
    pub use_fallback_gate: bool,
    pub fallback_env: f64,
    pub bank: &'a SampleBank,
    pub preview: Option<&'a Arc<SampleData>>,
    pub resonator_banks: &'a mut [ModalResonator],
}

/// Delegate normalized parameter writes on a `Copy` module struct straight to
/// `cadence-core`'s `ModuleKind::write_normalized` SPOT with zero heap allocations.
#[inline]
pub(super) fn write_copy_module_normalized<T: Copy>(
    target: &mut T,
    wrap: fn(T) -> ModuleKind,
    unwrap: fn(ModuleKind) -> Option<T>,
    param: ModuleParam,
    norm: f32,
    total_frames: usize,
) -> bool {
    let mut kind = wrap(*target);
    if !kind.write_normalized(param, norm, total_frames) {
        return false;
    }
    if let Some(updated) = unwrap(kind) {
        *target = updated;
        true
    } else {
        false
    }
}

/// Uniform zero-cost lifecycle contract implemented by all 11 modular voice nodes.
pub(super) trait VoiceModule {
    fn tag(&self) -> ModuleKindTag;
    fn target_params(&self) -> ModuleParams;
    fn write_target_normalized(
        &mut self,
        param: ModuleParam,
        norm: f32,
        total_frames: usize,
    ) -> bool;

    #[inline]
    fn sync_target_logs(&mut self) {}

    fn update_continuous(&mut self, src: &ModuleKind, module_id: u16, locks: VoiceLocks<'_>);
    fn on_trigger(&mut self, ctx: &TriggerCtx, state: &mut TriggerState<'_>);
    fn smooth_step(&mut self, s64: f64);

    #[inline]
    fn control_tick(&mut self, _sample_rate: f64) {}

    fn process(&mut self, inputs: &NodeInputs, ctx: &mut ProcessCtx<'_>) -> [f64; 2];

    #[inline]
    fn on_note_off(&mut self, _has_lifetime_envs: bool) {}

    #[inline]
    fn on_force_release(&mut self, _sample_rate: f64, _rel_coeff: f64) {}

    #[inline]
    fn on_choke(&mut self, _sample_rate: f64, _choke_coeff: f64) {}

    #[inline]
    fn is_tail_quiet(&self, _resonator_banks: &[ModalResonator]) -> bool {
        true
    }
}

/// Single unified node slot owning its `current` params, `target` params, and DSP state,
/// making cross-variant param/state mismatches structurally unrepresentable.
#[derive(Clone, Copy, Debug)]
pub(super) enum ModuleNode {
    Vco(VcoNode),
    Sampler(SamplerNode),
    Noise(NoiseNode),
    Env(EnvNode),
    Lfo(LfoNode),
    Filter(FilterNode),
    Vca(VcaNode),
    Drive(DriveNode),
    LoFi(LoFiNode),
    Resonator(ResonatorNode),
    Out(OutNode),
}

impl Default for ModuleNode {
    fn default() -> Self {
        Self::Out(OutNode::default())
    }
}

/// Exhaustive zero-cost static dispatch across all 11 [`ModuleNode`] variants.
macro_rules! dispatch_module_node {
    ($node:expr, |$inner:ident| $body:expr) => {
        match $node {
            ModuleNode::Vco($inner) => $body,
            ModuleNode::Sampler($inner) => $body,
            ModuleNode::Noise($inner) => $body,
            ModuleNode::Env($inner) => $body,
            ModuleNode::Lfo($inner) => $body,
            ModuleNode::Filter($inner) => $body,
            ModuleNode::Vca($inner) => $body,
            ModuleNode::Drive($inner) => $body,
            ModuleNode::LoFi($inner) => $body,
            ModuleNode::Resonator($inner) => $body,
            ModuleNode::Out($inner) => $body,
        }
    };
}

impl ModuleNode {
    #[inline]
    pub(super) fn from_kind(kind: &ModuleKind, sample_rate: f64) -> Self {
        match kind {
            ModuleKind::Vco(m) => Self::Vco(VcoNode::new(*m)),
            ModuleKind::Sampler(m) => Self::Sampler(SamplerNode::new(m.params(), sample_rate)),
            ModuleKind::Noise(m) => Self::Noise(NoiseNode::new(*m, sample_rate)),
            ModuleKind::Env(m) => Self::Env(EnvNode::new(*m, sample_rate)),
            ModuleKind::Lfo(m) => Self::Lfo(LfoNode::new(*m)),
            ModuleKind::Filter(m) => Self::Filter(FilterNode::new(*m, sample_rate)),
            ModuleKind::Vca(m) => Self::Vca(VcaNode::new(*m)),
            ModuleKind::Drive(m) => Self::Drive(DriveNode::new(*m, sample_rate)),
            ModuleKind::LoFi(m) => Self::LoFi(LoFiNode::new(*m, sample_rate)),
            ModuleKind::Resonator(m) => Self::Resonator(ResonatorNode::new(*m)),
            ModuleKind::Out(m) => Self::Out(OutNode::new(*m, sample_rate)),
        }
    }

    /// Stage base zone parameters into `target` without disturbing existing `current`
    /// or DSP state when the slot already holds the same module variant.
    #[inline]
    pub(super) fn stage_from_kind(&mut self, kind: &ModuleKind, sample_rate: f64) {
        match (self, kind) {
            (Self::Vco(n), ModuleKind::Vco(m)) => n.target = *m,
            (Self::Sampler(n), ModuleKind::Sampler(m)) => n.target = m.params(),
            (Self::Noise(n), ModuleKind::Noise(m)) => n.target = *m,
            (Self::Env(n), ModuleKind::Env(m)) => n.target = *m,
            (Self::Lfo(n), ModuleKind::Lfo(m)) => n.target = *m,
            (Self::Filter(n), ModuleKind::Filter(m)) => n.target = *m,
            (Self::Vca(n), ModuleKind::Vca(m)) => n.target = *m,
            (Self::Drive(n), ModuleKind::Drive(m)) => n.target = *m,
            (Self::LoFi(n), ModuleKind::LoFi(m)) => n.target = *m,
            (Self::Resonator(n), ModuleKind::Resonator(m)) => n.target = *m,
            (Self::Out(n), ModuleKind::Out(m)) => n.target = *m,
            (slot, other) => *slot = Self::from_kind(other, sample_rate),
        }
    }

    #[inline]
    pub(super) fn tag(&self) -> ModuleKindTag {
        dispatch_module_node!(self, |n| n.tag())
    }

    #[inline]
    pub(super) fn target_params(&self) -> ModuleParams {
        dispatch_module_node!(self, |n| n.target_params())
    }

    #[inline]
    pub(super) fn write_target_normalized(
        &mut self,
        param: ModuleParam,
        norm: f32,
        total_frames: usize,
    ) -> bool {
        dispatch_module_node!(self, |n| n.write_target_normalized(
            param,
            norm,
            total_frames
        ))
    }

    #[inline]
    pub(super) fn sync_target_logs(&mut self) {
        dispatch_module_node!(self, |n| n.sync_target_logs())
    }

    #[inline]
    pub(super) fn update_continuous(
        &mut self,
        src: &ModuleKind,
        module_id: u16,
        locks: VoiceLocks<'_>,
    ) {
        dispatch_module_node!(self, |n| n.update_continuous(src, module_id, locks))
    }

    #[inline]
    pub(super) fn on_trigger(&mut self, ctx: &TriggerCtx, state: &mut TriggerState<'_>) {
        dispatch_module_node!(self, |n| n.on_trigger(ctx, state))
    }

    #[inline]
    pub(super) fn smooth_step(&mut self, s64: f64) {
        dispatch_module_node!(self, |n| n.smooth_step(s64))
    }

    #[inline]
    pub(super) fn control_tick(&mut self, sample_rate: f64) {
        dispatch_module_node!(self, |n| n.control_tick(sample_rate))
    }

    #[inline]
    pub(super) fn process(&mut self, inputs: &NodeInputs, ctx: &mut ProcessCtx<'_>) -> [f64; 2] {
        dispatch_module_node!(self, |n| n.process(inputs, ctx))
    }

    #[inline]
    pub(super) fn on_note_off(&mut self, has_lifetime_envs: bool) {
        dispatch_module_node!(self, |n| n.on_note_off(has_lifetime_envs))
    }

    #[inline]
    pub(super) fn on_force_release(&mut self, sample_rate: f64, rel_coeff: f64) {
        dispatch_module_node!(self, |n| n.on_force_release(sample_rate, rel_coeff))
    }

    #[inline]
    pub(super) fn on_choke(&mut self, sample_rate: f64, choke_coeff: f64) {
        dispatch_module_node!(self, |n| n.on_choke(sample_rate, choke_coeff))
    }

    #[inline]
    pub(super) fn is_tail_quiet(&self, resonator_banks: &[ModalResonator]) -> bool {
        dispatch_module_node!(self, |n| n.is_tail_quiet(resonator_banks))
    }
}
