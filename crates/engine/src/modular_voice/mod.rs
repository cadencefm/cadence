//! Real-time-safe, zero-allocation Eurorack modular voice engine executing
//! [`CompiledZoneGraph`] programs at full `f64` audio rate with stereo `[f64; 2]`
//! wires and `z⁻¹` 1-sample feedback (`prev_slots`).
//!
//! Strictly composes `cadence_core::dsp::*` — zero duplicated DSP math.

mod nodes;

use cadence_core::dsp::{
    control_stride_mask, decay_coeff_60db_ms, flush_denormal_f64, fm_depth_from_amount,
    glide_coeff_ms, one_pole_coeff_ms, sanitize_sample, sanitize_sample_rate, smooth_step_f32,
    smooth_step_f64, stereo_mid_f64, voice_guard_f64, AdsrStage, ModalResonator, CHOKE_MS,
    FAST_RELEASE_MS, TAIL_SILENCE_FLOOR_F64,
};
pub use cadence_core::rack::SamplerParams;
use cadence_core::rack::{
    cable_signal_level, denormalize_cable_amount, module_sample_key, CompiledCable, CompiledOp,
    CompiledZoneGraph, DriveModule, EnvModule, FilterMode, FilterModule, InPort, LfoModule,
    LoFiModule, ModuleKind, ModuleKindTag, ModuleParam, NoiseModule, OutModule, OutPort,
    PitchTracking, RackZone, ResonatorModule, SampleLoopMode, TriggerMode, UpdatePolicy, VcaModule,
    VcoModule, VoiceMode, VoiceSettings, MAX_CABLES_PER_ZONE, MAX_MODULES_PER_ZONE,
    MAX_RESONATORS_PER_ZONE,
};
#[cfg(test)]
use cadence_core::rack::{LfoShape, ResonatorMode, SamplerModule};
use cadence_core::runtime::{RackActivityTelemetry, SampleBank};
use cadence_core::sampler::SampleData;
use nodes::{ModuleNode, NodeInputs, ProcessCtx, TriggerCtx, TriggerState, VoiceLocks};
use std::sync::Arc;

const VOICE_CONTROL_RATE_HZ: f64 = 3_000.0;
const DEFAULT_TAIL_TIMEOUT_SEC: f64 = 2.0;
/// Upper fallback cap for stuck/self-oscillating resonator tails (`28 s` covers bright
/// modes up to `12 s` RT60 decaying below `-120 dB` with excitation headroom; normal
/// tails retire early as soon as `ModalResonator::quiet()` triggers).
const MAX_RESONATOR_TAIL_SEC: f64 = 28.0;
/// Multiplier from base resonator `decay_ms` to fallback tail timeout (`1.455x` max
/// brightness stretch × `2.0x` for `-120 dB` quiet threshold + headroom).
const RESONATOR_TAIL_SAFETY_MULT: f64 = 3.2;

/// Maximum number of inline per-voice P-locks tracked without heap allocation.
pub const MAX_VOICE_LOCKS: usize = 32;

/// Trigger onset metadata for starting or retargeting a [`ModularVoice`].
#[derive(Clone, Copy, Debug)]
pub struct VoiceStart {
    pub midi: u8,
    pub velocity: f32,
    /// `None` holds a live gate until `note_off()`; `Some` is a scheduled gate length in frames.
    pub gate_frames: Option<u32>,
    pub seed: u32,
    /// Used only to initialize free-running oscillator/LFO phase when phase reset/retrigger is off.
    pub clock_seconds: f64,
    /// Prior monophonic pitch (fractional MIDI note) to portamento-glide from on a fresh trigger
    /// when `glide_ms > 0` and no active mono voice is being retargeted.
    pub glide_from_pitch: Option<f64>,
}

/// Borrowed zone trigger specification for staging or retargeting a [`ModularVoice`].
#[derive(Clone, Copy)]
pub struct ZoneTriggerSpec<'a> {
    pub inst_id: u64,
    pub zone: &'a RackZone,
    pub graph: &'a CompiledZoneGraph,
    pub sample_rate: f64,
    pub bpm: f64,
    pub zone_pan: f32,
    pub rack_tune: f32,
    pub is_preview: bool,
}

/// Compact inline key identifying one P-locked module parameter or patch cable on a voice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceLockKey {
    Module { module_id: u16, param: ModuleParam },
    Cable { cable_id: u16 },
}

/// Zero-allocation `Copy` parameter snapshot for any of the 11 Eurorack module kinds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ModuleParams {
    Vco(VcoModule),
    Sampler(SamplerParams),
    Noise(NoiseModule),
    Env(EnvModule),
    Lfo(LfoModule),
    Filter(FilterModule),
    Vca(VcaModule),
    Drive(DriveModule),
    LoFi(LoFiModule),
    Resonator(ResonatorModule),
    Out(OutModule),
}

impl Default for ModuleParams {
    fn default() -> Self {
        Self::Out(OutModule::default())
    }
}

impl ModuleParams {
    #[inline]
    pub fn from_kind(kind: &ModuleKind) -> Self {
        match kind {
            ModuleKind::Vco(m) => Self::Vco(*m),
            ModuleKind::Sampler(m) => Self::Sampler(m.params()),
            ModuleKind::Noise(m) => Self::Noise(*m),
            ModuleKind::Env(m) => Self::Env(*m),
            ModuleKind::Lfo(m) => Self::Lfo(*m),
            ModuleKind::Filter(m) => Self::Filter(*m),
            ModuleKind::Vca(m) => Self::Vca(*m),
            ModuleKind::Drive(m) => Self::Drive(*m),
            ModuleKind::LoFi(m) => Self::LoFi(*m),
            ModuleKind::Resonator(m) => Self::Resonator(*m),
            ModuleKind::Out(m) => Self::Out(*m),
        }
    }

    #[inline]
    pub fn tag(self) -> ModuleKindTag {
        match self {
            Self::Vco(_) => ModuleKindTag::Vco,
            Self::Sampler(_) => ModuleKindTag::Sampler,
            Self::Noise(_) => ModuleKindTag::Noise,
            Self::Env(_) => ModuleKindTag::Env,
            Self::Lfo(_) => ModuleKindTag::Lfo,
            Self::Filter(_) => ModuleKindTag::Filter,
            Self::Vca(_) => ModuleKindTag::Vca,
            Self::Drive(_) => ModuleKindTag::Drive,
            Self::LoFi(_) => ModuleKindTag::LoFi,
            Self::Resonator(_) => ModuleKindTag::Resonator,
            Self::Out(_) => ModuleKindTag::Out,
        }
    }

    /// Delegate normalized parameter writes straight to `cadence-core`'s SPOT
    /// (`SamplerParams::write_normalized` / `ModuleKind::write_normalized`) with zero heap allocation.
    #[inline]
    pub fn write_normalized(&mut self, param: ModuleParam, norm: f32, total_frames: usize) -> bool {
        let mut kind = match *self {
            Self::Sampler(mut m) => {
                let ok = m.write_normalized(param, norm, total_frames);
                if ok {
                    *self = Self::Sampler(m);
                }
                return ok;
            }
            Self::Vco(m) => ModuleKind::Vco(m),
            Self::Noise(m) => ModuleKind::Noise(m),
            Self::Env(m) => ModuleKind::Env(m),
            Self::Lfo(m) => ModuleKind::Lfo(m),
            Self::Filter(m) => ModuleKind::Filter(m),
            Self::Vca(m) => ModuleKind::Vca(m),
            Self::Drive(m) => ModuleKind::Drive(m),
            Self::LoFi(m) => ModuleKind::LoFi(m),
            Self::Resonator(m) => ModuleKind::Resonator(m),
            Self::Out(m) => ModuleKind::Out(m),
        };
        if !kind.write_normalized(param, norm, total_frames) {
            return false;
        }
        *self = Self::from_kind(&kind);
        true
    }
}

const EMPTY_OP: CompiledOp = CompiledOp {
    module_id: 0,
    module_index: 0,
    kind: ModuleKindTag::Out,
    column: 3,
    in_cable_start: 0,
    in_cable_len: 0,
};

const EMPTY_CABLE: CompiledCable = CompiledCable {
    cable_id: 0,
    cable_index: 0,
    from_slot: 0,
    from_port: OutPort::Out,
    to_slot: 0,
    to_port: InPort::In,
    amount: 0.0,
    feedback: false,
};

/// Real-time-safe, zero-allocation Eurorack modular zone voice.
///
/// Intentionally `!Copy` so callers never accidentally duplicate the ~12 KB
/// module-node array on debug thread stacks; always heap-box in voice pools
/// (`Box<ModularVoice>`).
#[derive(Debug)]
pub struct ModularVoice {
    active: bool,
    released: bool,
    sample_rate: f64,
    bpm: f64,
    age: u64,
    gate_frames: Option<u32>,
    voice_settings: VoiceSettings,
    glide_coeff: f64,
    cached_glide_ms: f32,
    zone_root_midi: u8,
    note_pitch: f64,
    target_note_pitch: f64,
    velocity: f64,
    target_velocity: f64,
    zone_pan: f32,
    target_zone_pan: f32,
    rack_tune: f64,
    target_rack_tune: f64,
    smoothing: f64,
    control_counter: u32,
    control_mask: u32,
    tail_frames: u32,
    tail_elapsed_frames: u32,
    has_resonator: bool,

    // Fallback gate/choke envelope used when a zone has no lifetime Env modules.
    fallback_env: f64,
    fallback_releasing: bool,
    fallback_release_coeff: f64,
    sampler_only: bool,
    sampler_only_one_shot: bool,
    any_sampler_has_trig_in: bool,

    // Compiled flat DAG snapshot (zero heap pointers).
    ops: [CompiledOp; MAX_MODULES_PER_ZONE],
    op_count: usize,
    cables: [CompiledCable; MAX_CABLES_PER_ZONE],
    cable_count: usize,
    module_slot_by_id: [(u16, u8); MAX_MODULES_PER_ZONE],
    cable_index_by_id: [(u16, u8); MAX_CABLES_PER_ZONE],
    out_slot: u8,
    lifetime_envs: [u8; MAX_MODULES_PER_ZONE],
    lifetime_env_count: usize,
    env_gated_vcas: [u8; MAX_MODULES_PER_ZONE],
    env_gated_vca_count: usize,
    post_gate_tail_slots: [u8; MAX_MODULES_PER_ZONE],
    post_gate_tail_count: usize,
    sampler_slots: [u8; MAX_MODULES_PER_ZONE],
    sampler_slot_count: usize,

    // Smoothed cable attenuverters and self-contained per-slot module nodes.
    cable_current: [f32; MAX_CABLES_PER_ZONE],
    cable_target: [f32; MAX_CABLES_PER_ZONE],
    nodes: [ModuleNode; MAX_MODULES_PER_ZONE],
    // Heap-hosted 32-mode physical resonator banks (pre-allocated once per voice, zero RT allocs).
    resonator_banks: Box<[ModalResonator]>,

    // Stereo audio/CV slot buffers (`prev_slots` serves `z⁻¹` feedback edges).
    slots: [[f64; 2]; MAX_MODULES_PER_ZONE],
    prev_slots: [[f64; 2]; MAX_MODULES_PER_ZONE],

    // Inline per-voice P-lock overrides (zero heap allocs).
    locked_keys: [Option<VoiceLockKey>; MAX_VOICE_LOCKS],
    locked_count: usize,
}

impl ModularVoice {
    /// Allocate an inactive [`ModularVoice`] directly on the heap (stack-safe in debug builds).
    pub fn new_boxed_inactive(sample_rate: f64) -> Box<Self> {
        let sr = sanitize_sample_rate(sample_rate).clamp(8_000.0, 384_000.0);
        Box::new(Self {
            active: false,
            released: true,
            sample_rate: sr,
            bpm: 120.0,
            age: 0,
            gate_frames: None,
            voice_settings: VoiceSettings::default(),
            glide_coeff: 1.0,
            cached_glide_ms: 0.0,
            zone_root_midi: 60,
            note_pitch: 60.0,
            target_note_pitch: 60.0,
            velocity: 0.0,
            target_velocity: 0.0,
            zone_pan: 0.0,
            target_zone_pan: 0.0,
            rack_tune: 0.0,
            target_rack_tune: 0.0,
            smoothing: one_pole_coeff_ms(5.0, sr),
            control_counter: 0,
            control_mask: control_stride_mask(sr, VOICE_CONTROL_RATE_HZ),
            tail_frames: (sr * DEFAULT_TAIL_TIMEOUT_SEC) as u32,
            tail_elapsed_frames: 0,
            has_resonator: false,
            fallback_env: 0.0,
            fallback_releasing: false,
            fallback_release_coeff: decay_coeff_60db_ms(FAST_RELEASE_MS, sr),
            sampler_only: false,
            sampler_only_one_shot: false,
            any_sampler_has_trig_in: false,
            ops: [EMPTY_OP; MAX_MODULES_PER_ZONE],
            op_count: 0,
            cables: [EMPTY_CABLE; MAX_CABLES_PER_ZONE],
            cable_count: 0,
            module_slot_by_id: [(0, 0); MAX_MODULES_PER_ZONE],
            cable_index_by_id: [(0, 0); MAX_CABLES_PER_ZONE],
            out_slot: 0,
            lifetime_envs: [0; MAX_MODULES_PER_ZONE],
            lifetime_env_count: 0,
            env_gated_vcas: [0; MAX_MODULES_PER_ZONE],
            env_gated_vca_count: 0,
            post_gate_tail_slots: [0; MAX_MODULES_PER_ZONE],
            post_gate_tail_count: 0,
            sampler_slots: [0; MAX_MODULES_PER_ZONE],
            sampler_slot_count: 0,
            cable_current: [0.0; MAX_CABLES_PER_ZONE],
            cable_target: [0.0; MAX_CABLES_PER_ZONE],
            nodes: [ModuleNode::default(); MAX_MODULES_PER_ZONE],
            resonator_banks: vec![ModalResonator::new(sr); MAX_RESONATORS_PER_ZONE]
                .into_boxed_slice(),
            slots: [[0.0; 2]; MAX_MODULES_PER_ZONE],
            prev_slots: [[0.0; 2]; MAX_MODULES_PER_ZONE],
            locked_keys: [None; MAX_VOICE_LOCKS],
            locked_count: 0,
        })
    }

    /// Convenience constructor allocating a triggered [`ModularVoice`] on the heap.
    pub fn new_boxed(spec: &ZoneTriggerSpec<'_>, start: VoiceStart) -> Box<Self> {
        let mut voice = Self::new_boxed_inactive(spec.sample_rate);
        voice.begin_trigger_from_zone(spec);
        voice.finalize_trigger(start, false, false);
        voice
    }

    #[inline]
    pub fn deactivate(&mut self) {
        self.active = false;
        self.released = true;
    }

    #[inline]
    pub fn is_active(&self) -> bool {
        self.active
    }

    #[inline]
    pub fn is_releasing(&self) -> bool {
        if !self.active || self.released {
            return true;
        }
        if self.lifetime_env_count > 0 {
            return self.lifetime_envs[..self.lifetime_env_count]
                .iter()
                .all(|&s| match self.nodes.get(usize::from(s)) {
                    Some(ModuleNode::Env(node)) => node.env.is_releasing(),
                    _ => true,
                });
        }
        if self.sampler_only && self.sampler_slot_count > 0 {
            return self.sampler_slots[..self.sampler_slot_count]
                .iter()
                .all(|&s| match self.nodes.get(usize::from(s)) {
                    Some(ModuleNode::Sampler(node)) => !node.cursor.is_playing() || node.releasing,
                    _ => true,
                });
        }
        self.fallback_releasing
    }

    #[inline]
    pub fn current_level(&self) -> f32 {
        if !self.active {
            return 0.0;
        }
        if self.lifetime_env_count > 0 {
            let mut max_env = 0.0_f64;
            for &s in &self.lifetime_envs[..self.lifetime_env_count] {
                if let Some(ModuleNode::Env(node)) = self.nodes.get(usize::from(s)) {
                    max_env = max_env.max(node.env.level());
                }
            }
            return (max_env * self.velocity) as f32;
        }
        if self.sampler_only && self.sampler_slot_count > 0 {
            let mut max_s = 0.0_f64;
            for &s in &self.sampler_slots[..self.sampler_slot_count] {
                if let Some(ModuleNode::Sampler(node)) = self.nodes.get(usize::from(s)) {
                    if node.cursor.is_playing() {
                        max_s = max_s.max(node.release_gain);
                    }
                }
            }
            return (max_s * self.velocity) as f32;
        }
        (self.fallback_env * self.velocity) as f32
    }

    /// O(log N) lookup of topological execution slot from stable `module_id`.
    #[inline]
    pub fn slot_for_module(&self, module_id: u16) -> Option<usize> {
        self.module_slot_by_id[..self.op_count]
            .binary_search_by_key(&module_id, |&(id, _)| id)
            .ok()
            .map(|pos| usize::from(self.module_slot_by_id[pos].1))
    }

    /// O(log N) lookup of compiled cable index from stable `cable_id`.
    #[inline]
    pub fn cable_index_for_id(&self, cable_id: u16) -> Option<usize> {
        self.cable_index_by_id[..self.cable_count]
            .binary_search_by_key(&cable_id, |&(id, _)| id)
            .ok()
            .map(|pos| usize::from(self.cable_index_by_id[pos].1))
    }

    #[inline]
    pub fn is_module_locked(&self, module_id: u16, param: ModuleParam) -> bool {
        let key = VoiceLockKey::Module { module_id, param };
        self.locked_keys[..self.locked_count].contains(&Some(key))
    }

    #[inline]
    pub fn is_cable_locked(&self, cable_id: u16) -> bool {
        let key = VoiceLockKey::Cable { cable_id };
        self.locked_keys[..self.locked_count].contains(&Some(key))
    }

    #[inline]
    pub fn clear_locks(&mut self) {
        self.locked_count = 0;
    }

    #[inline]
    pub fn record_lock(&mut self, key: VoiceLockKey) {
        if self.locked_keys[..self.locked_count].contains(&Some(key)) {
            return;
        }
        if self.locked_count < MAX_VOICE_LOCKS {
            self.locked_keys[self.locked_count] = Some(key);
            self.locked_count += 1;
        }
    }

    /// Diagnostic query: effective modulated cutoff frequency of the first `Filter` module
    /// (or `20_000.0` if this zone has no `Filter`).
    pub fn effective_filter_cutoff_hz(&self) -> f32 {
        for node in &self.nodes[..self.op_count] {
            if let ModuleNode::Filter(flt) = node {
                return flt.effective_cutoff_hz;
            }
        }
        20_000.0
    }

    /// Diagnostic query: effective modulated cutoff of a specific `Filter` module by `module_id`.
    pub fn module_filter_cutoff_hz(&self, module_id: u16) -> Option<f32> {
        let slot = self.slot_for_module(module_id)?;
        match self.nodes.get(slot) {
            Some(ModuleNode::Filter(flt)) => Some(flt.effective_cutoff_hz),
            _ => None,
        }
    }

    /// Diagnostic query: target `ModuleParams` snapshot for `module_id`.
    pub fn module_target_params(&self, module_id: u16) -> Option<ModuleParams> {
        let slot = self.slot_for_module(module_id)?;
        self.nodes.get(slot).map(ModuleNode::target_params)
    }

    /// Diagnostic query: current `AdsrStage` and level of an `Env` module.
    pub fn env_stage_and_level(&self, module_id: u16) -> Option<(AdsrStage, f64)> {
        let slot = self.slot_for_module(module_id)?;
        match self.nodes.get(slot) {
            Some(ModuleNode::Env(node)) => Some((node.env.stage(), node.env.level())),
            _ => None,
        }
    }

    /// Current pitch (fractional MIDI note, including active portamento glide).
    #[inline]
    pub fn current_note_pitch(&self) -> f64 {
        self.note_pitch
    }

    /// Target pitch (fractional MIDI note) configured by the most recent trigger/retarget.
    #[inline]
    pub fn target_note_pitch(&self) -> f64 {
        self.target_note_pitch
    }

    /// Whether this voice's zone is configured for monophonic voice allocation.
    #[inline]
    pub fn is_mono(&self) -> bool {
        self.voice_settings.voice_mode == VoiceMode::Mono
    }

    /// Diagnostic query: last computed playback step ratio of a `Sampler` module.
    pub fn sampler_step_ratio(&self, module_id: u16) -> Option<f64> {
        let slot = self.slot_for_module(module_id)?;
        match self.nodes.get(slot) {
            Some(ModuleNode::Sampler(node)) => Some(node.last_step_ratio),
            _ => None,
        }
    }

    /// Diagnostic query: target attenuverter amount of a cable by `cable_id`.
    pub fn cable_target_amount(&self, cable_id: u16) -> Option<f32> {
        let ci = self.cable_index_for_id(cable_id)?;
        self.cable_target.get(ci).copied()
    }

    /// Check whether `graph` matches the currently loaded topological slot layout.
    pub fn matches_topology(&self, graph: &CompiledZoneGraph) -> bool {
        let n = graph.ops.len().min(MAX_MODULES_PER_ZONE);
        if n != self.op_count {
            return false;
        }
        for s in 0..n {
            if self.ops[s].module_id != graph.ops[s].module_id
                || self.ops[s].kind != graph.ops[s].kind
            {
                return false;
            }
        }
        true
    }

    /// Stage 1 of zero-allocation voice trigger: load graph metadata and base zone
    /// parameters into node targets / `cable_target` without touching existing
    /// DSP states yet (so mono retarget can inspect & preserve them in Stage 2).
    ///
    /// Callers may apply trigger-time `ModLane`s and `ParamLock`s via
    /// [`apply_trigger_module_param`], [`apply_trigger_cable_norm`], and
    /// [`set_trigger_zone_overrides`] before calling [`finalize_trigger`].
    pub fn begin_trigger_from_zone(&mut self, spec: &ZoneTriggerSpec<'_>) -> bool {
        let ZoneTriggerSpec {
            inst_id,
            zone,
            graph,
            sample_rate,
            bpm,
            zone_pan,
            rack_tune,
            is_preview,
        } = *spec;
        let sr = sanitize_sample_rate(sample_rate).clamp(8_000.0, 384_000.0);
        let same_topology = self.active && self.matches_topology(graph);
        self.sample_rate = sr;
        self.control_mask = control_stride_mask(sr, VOICE_CONTROL_RATE_HZ);
        self.bpm = if bpm.is_finite() {
            bpm.clamp(20.0, 300.0)
        } else {
            120.0
        };
        self.smoothing = one_pole_coeff_ms(5.0, sr);
        self.voice_settings = zone.voice;
        self.sync_glide_coeff();
        self.zone_root_midi = zone.root_midi.min(127);
        self.target_zone_pan = sanitize_sample(zone_pan).clamp(-1.0, 1.0);
        self.target_rack_tune = f64::from(sanitize_sample(rack_tune).clamp(-24.0, 24.0));
        self.clear_locks();

        // Copy compiled graph into fixed inline arrays and stage base parameters on each node.
        let op_count = graph.ops.len().min(MAX_MODULES_PER_ZONE);
        self.op_count = op_count;
        for s in 0..op_count {
            self.ops[s] = graph.ops[s];
            self.module_slot_by_id[s] = graph
                .module_slot_by_id
                .get(s)
                .copied()
                .unwrap_or((graph.ops[s].module_id, s as u8));
            let orig_idx = usize::from(graph.ops[s].module_index);
            if let Some(module) = zone.modules.get(orig_idx) {
                self.nodes[s].stage_from_kind(&module.kind, sr);
            } else {
                self.nodes[s] = ModuleNode::default();
            }
            if let ModuleNode::Sampler(smp) = &mut self.nodes[s] {
                let new_key = module_sample_key(inst_id, zone.id, self.ops[s].module_id);
                if smp.sample_key != new_key {
                    smp.sample_key = new_key;
                    smp.cached_bank_idx = None;
                }
                smp.is_preview = is_preview;
            }
        }

        let cable_count = graph.cables.len().min(MAX_CABLES_PER_ZONE);
        self.cable_count = cable_count;
        for ci in 0..cable_count {
            let c = graph.cables[ci];
            self.cables[ci] = c;
            self.cable_target[ci] = sanitize_sample(c.amount);
            self.cable_index_by_id[ci] = graph
                .cable_index_by_id
                .get(ci)
                .copied()
                .unwrap_or((c.cable_id, ci as u8));
        }

        self.out_slot = graph.out_slot.min(op_count.saturating_sub(1) as u8);

        self.lifetime_env_count = graph.lifetime_envs.len().min(MAX_MODULES_PER_ZONE);
        self.lifetime_envs[..self.lifetime_env_count]
            .copy_from_slice(&graph.lifetime_envs[..self.lifetime_env_count]);

        self.env_gated_vca_count = graph.env_gated_vcas.len().min(MAX_MODULES_PER_ZONE);
        self.env_gated_vcas[..self.env_gated_vca_count]
            .copy_from_slice(&graph.env_gated_vcas[..self.env_gated_vca_count]);

        self.post_gate_tail_count = graph.post_gate_tail_slots.len().min(MAX_MODULES_PER_ZONE);
        self.post_gate_tail_slots[..self.post_gate_tail_count]
            .copy_from_slice(&graph.post_gate_tail_slots[..self.post_gate_tail_count]);

        self.sampler_slot_count = graph.sampler_slots.len().min(MAX_MODULES_PER_ZONE);
        self.sampler_slots[..self.sampler_slot_count]
            .copy_from_slice(&graph.sampler_slots[..self.sampler_slot_count]);

        same_topology
    }

    /// Apply a trigger-time normalized value to a module parameter before [`finalize_trigger`].
    #[inline]
    pub fn apply_trigger_module_param(
        &mut self,
        module_id: u16,
        param: ModuleParam,
        norm: f32,
        total_frames: usize,
        lock: bool,
    ) {
        let Some(slot) = self.slot_for_module(module_id) else {
            return;
        };
        if slot < self.op_count {
            if self.nodes[slot].write_target_normalized(param, norm, total_frames)
                && param == ModuleParam::RootMidi
            {
                if let ModuleNode::Sampler(smp) = &self.nodes[slot] {
                    self.zone_root_midi = smp.target.root_midi;
                }
            }
            if lock {
                self.record_lock(VoiceLockKey::Module { module_id, param });
            }
        }
    }

    /// Apply a trigger-time normalized value to a patch cable attenuverter before [`finalize_trigger`].
    #[inline]
    pub fn apply_trigger_cable_norm(&mut self, cable_id: u16, norm: f32, lock: bool) {
        let Some(ci) = self.cable_index_for_id(cable_id) else {
            return;
        };
        if ci < self.cable_count {
            let c = self.cables[ci];
            let to_slot = usize::from(c.to_slot);
            if to_slot < self.op_count {
                let to_kind = self.ops[to_slot].kind;
                self.cable_target[ci] = denormalize_cable_amount(to_kind, c.to_port, norm);
                if lock {
                    self.record_lock(VoiceLockKey::Cable { cable_id });
                }
            }
        }
    }

    #[inline]
    fn sync_glide_coeff(&mut self) {
        let ms = self.voice_settings.glide_ms;
        self.cached_glide_ms = ms;
        self.glide_coeff = glide_coeff_ms(ms, self.sample_rate);
    }

    /// Apply trigger-time zone-level overrides (from ModLanes/P-locks) before [`finalize_trigger`].
    #[inline]
    pub fn set_trigger_zone_overrides(
        &mut self,
        voice_settings: VoiceSettings,
        zone_root_midi: u8,
        zone_pan: f32,
        rack_tune: f32,
    ) {
        self.voice_settings = voice_settings;
        self.sync_glide_coeff();
        self.zone_root_midi = zone_root_midi.min(127);
        self.target_zone_pan = sanitize_sample(zone_pan).clamp(-1.0, 1.0);
        self.target_rack_tune = f64::from(sanitize_sample(rack_tune).clamp(-24.0, 24.0));
    }

    fn all_lifetime_envs_one_shot(&self, use_target: bool) -> bool {
        if self.lifetime_env_count == 0 {
            return false;
        }
        self.lifetime_envs[..self.lifetime_env_count]
            .iter()
            .all(|&s| match self.nodes.get(usize::from(s)) {
                Some(ModuleNode::Env(node)) => {
                    let env = if use_target {
                        node.target
                    } else {
                        node.current
                    };
                    env.trigger_mode == TriggerMode::OneShot
                }
                _ => false,
            })
    }

    /// Stage 2 of zero-allocation trigger/retarget: initialize or smoothly retarget
    /// all module DSP states in place after trigger-time ModLanes and P-locks are applied.
    pub fn finalize_trigger(&mut self, start: VoiceStart, is_retarget: bool, request_legato: bool) {
        let sr = self.sample_rate;
        let vel = sanitize_sample(start.velocity).clamp(0.0, 1.0);
        self.target_velocity = f64::from(vel);
        let was_active = self.active;
        let was_releasing = self.is_releasing();
        let prev_all_one_shot = self.all_lifetime_envs_one_shot(false);
        let next_all_one_shot = self.all_lifetime_envs_one_shot(true);

        let eff_retarget = is_retarget && was_active;
        let eff_legato = eff_retarget
            && request_legato
            && !was_releasing
            && !prev_all_one_shot
            && !next_all_one_shot;

        let target_pitch = if self.voice_settings.pitch_tracking == PitchTracking::Fixed {
            f64::from(self.zone_root_midi)
        } else {
            f64::from(start.midi.min(127))
        };
        self.target_note_pitch = target_pitch;
        self.sync_glide_coeff();
        if eff_retarget && self.voice_settings.glide_ms > 0.0 {
            // Continue gliding smoothly from the currently sounding/releasing voice pitch.
        } else if let Some(prev_pitch) = start.glide_from_pitch.filter(|p| {
            p.is_finite()
                && self.voice_settings.glide_ms > 0.0
                && self.voice_settings.pitch_tracking != PitchTracking::Fixed
        }) {
            self.note_pitch = prev_pitch.clamp(0.0, 127.0);
        } else {
            self.note_pitch = target_pitch;
        }

        // Compute source classification for lifecycle & one-shot semantics.
        let op_count = self.op_count;
        let cable_count = self.cable_count;
        let mut has_non_sampler_source = false;
        let mut all_samplers_one_shot = self.sampler_slot_count > 0;
        let mut any_sampler_trig = false;
        for (s, node) in self.nodes.iter().take(op_count).enumerate() {
            match node {
                ModuleNode::Vco(_) | ModuleNode::Noise(_) => has_non_sampler_source = true,
                ModuleNode::Sampler(smp) => {
                    if slot_has_in_port(
                        &self.ops[..op_count],
                        &self.cables[..cable_count],
                        s,
                        InPort::TrigIn,
                    ) {
                        any_sampler_trig = true;
                    }
                    if !(smp.target.one_shot && smp.target.loop_mode == SampleLoopMode::Off) {
                        all_samplers_one_shot = false;
                    }
                }
                _ => {}
            }
        }
        self.sampler_only = self.sampler_slot_count > 0 && !has_non_sampler_source;
        self.sampler_only_one_shot =
            self.sampler_only && all_samplers_one_shot && !any_sampler_trig;
        self.any_sampler_has_trig_in = any_sampler_trig;

        self.active = true;
        self.released = false;
        self.age = 0;
        self.gate_frames = start.gate_frames;
        self.tail_frames = (sr * DEFAULT_TAIL_TIMEOUT_SEC) as u32;
        self.tail_elapsed_frames = 0;
        self.control_counter = 0;
        self.fallback_env = 1.0;
        self.fallback_releasing = false;
        self.fallback_release_coeff = decay_coeff_60db_ms(FAST_RELEASE_MS, sr);

        let clock = if start.clock_seconds.is_finite() {
            start.clock_seconds.max(0.0)
        } else {
            0.0
        };
        let base_seed = start.seed.max(1);

        if !eff_legato {
            self.velocity = self.target_velocity;
            self.zone_pan = self.target_zone_pan;
            self.rack_tune = self.target_rack_tune;
            self.cable_current[..cable_count].copy_from_slice(&self.cable_target[..cable_count]);
        }

        if !eff_retarget {
            for s in 0..op_count {
                self.slots[s] = [0.0; 2];
                self.prev_slots[s] = [0.0; 2];
            }
        }

        let ops = &self.ops[..op_count];
        let cables = &self.cables[..cable_count];
        let mut tstate = TriggerState {
            resonator_banks: &mut self.resonator_banks,
            next_res_bank: 0,
        };
        for s in 0..op_count {
            let acid_vel_amount = if ops[s].kind == ModuleKindTag::Env {
                acid_filter_vel_amount_for_env(cables, &self.nodes[..op_count], s as u8)
            } else {
                None
            };
            let tctx = TriggerCtx {
                sample_rate: sr,
                bpm: self.bpm,
                mod_id: ops[s].module_id,
                base_seed,
                clock,
                vel,
                target_note_pitch: self.target_note_pitch,
                target_rack_tune: self.target_rack_tune,
                phase_reset: self.voice_settings.phase_reset,
                eff_retarget,
                eff_legato,
                has_fm_in: slot_has_in_port(ops, cables, s, InPort::FmIn),
                has_trig_in: slot_has_in_port(ops, cables, s, InPort::TrigIn),
                has_cv: slot_has_in_port(ops, cables, s, InPort::Cv),
                acid_vel_amount,
            };
            self.nodes[s].on_trigger(&tctx, &mut tstate);
        }
        self.has_resonator = tstate.next_res_bank > 0;
        let max_tail_res_decay_ms = if self.has_resonator {
            let use_post_gate_only = self.lifetime_env_count > 0
                && !(self.sampler_only && !self.any_sampler_has_trig_in)
                && self.all_env_gates_closed();
            self.max_tail_resonator_decay_ms(use_post_gate_only)
        } else {
            0.0
        };
        self.tail_frames = self.compute_tail_frames(max_tail_res_decay_ms);
    }

    /// Retarget an active voice cleanly in-place (preserving glide, phases, filter
    /// integrators, and legato envelope progress when `legato` is true).
    pub fn retarget_from_zone(
        &mut self,
        spec: &ZoneTriggerSpec<'_>,
        start: VoiceStart,
        legato: bool,
    ) {
        let same_topology = self.begin_trigger_from_zone(spec);
        self.finalize_trigger(start, same_topology, legato);
    }

    /// Update all unlocked `UpdatePolicy::Continuous` module and cable targets from
    /// the live `RackZone` (plus zone pan, rack tune, and tempo). Zero heap allocations.
    pub fn update_continuous_from_zone(
        &mut self,
        zone: &RackZone,
        zone_pan: f32,
        rack_tune: f32,
        bpm: f64,
    ) {
        if bpm.is_finite() {
            self.bpm = bpm.clamp(20.0, 300.0);
        }
        self.target_zone_pan = sanitize_sample(zone_pan).clamp(-1.0, 1.0);
        self.target_rack_tune = f64::from(sanitize_sample(rack_tune).clamp(-24.0, 24.0));

        for m in &zone.modules {
            let Some(slot) = self.slot_for_module(m.id) else {
                continue;
            };
            if slot >= self.op_count || self.nodes[slot].tag() != m.kind.tag() {
                continue;
            }
            let locks = VoiceLocks::new(&self.locked_keys[..self.locked_count]);
            self.nodes[slot].update_continuous(&m.kind, m.id, locks);
        }

        for c in &zone.cables {
            if self.is_cable_locked(c.id) {
                continue;
            }
            let Some(ci) = self.cable_index_for_id(c.id) else {
                continue;
            };
            if ci < self.cable_count {
                self.cable_target[ci] = sanitize_sample(c.amount);
            }
        }
    }

    /// Fast-path update for continuous `Zone.Pan` and `Rack.Tune` modulation lanes
    /// without rescanning unchanged module/cable topology.
    #[inline]
    pub fn set_continuous_zone_pan_and_tune(&mut self, zone_pan: f32, rack_tune: f32) {
        self.target_zone_pan = sanitize_sample(zone_pan).clamp(-1.0, 1.0);
        self.target_rack_tune = f64::from(sanitize_sample(rack_tune).clamp(-24.0, 24.0));
    }

    /// Apply one unlocked continuous module modulation (`ModLane`) directly onto this voice.
    #[inline]
    pub fn apply_continuous_module_param(&mut self, module_id: u16, param: ModuleParam, norm: f32) {
        if param.policy() != UpdatePolicy::Continuous || self.is_module_locked(module_id, param) {
            return;
        }
        let Some(slot) = self.slot_for_module(module_id) else {
            return;
        };
        if slot < self.op_count && self.nodes[slot].write_target_normalized(param, norm, 0) {
            self.nodes[slot].sync_target_logs();
        }
    }

    /// Apply one unlocked continuous cable attenuverter modulation (`ModLane`) directly onto this voice.
    #[inline]
    pub fn apply_continuous_cable_norm(&mut self, cable_id: u16, norm: f32) {
        if self.is_cable_locked(cable_id) {
            return;
        }
        let Some(ci) = self.cable_index_for_id(cable_id) else {
            return;
        };
        if ci < self.cable_count {
            let c = self.cables[ci];
            let to_slot = usize::from(c.to_slot);
            if to_slot < self.op_count {
                let to_kind = self.ops[to_slot].kind;
                self.cable_target[ci] = denormalize_cable_amount(to_kind, c.to_port, norm);
            }
        }
    }

    /// Standard note-off release: honors `TriggerMode::OneShot` envelopes and
    /// `Sampler.one_shot` pads so one-shots finish their natural decay.
    pub fn note_off(&mut self) {
        if !self.active || self.sampler_only_one_shot {
            return;
        }
        let has_lifetime_envs = self.lifetime_env_count > 0;
        if has_lifetime_envs && self.all_lifetime_envs_one_shot(false) {
            return;
        }
        self.released = true;
        if !has_lifetime_envs {
            self.fallback_releasing = true;
        }
        for node in &mut self.nodes[..self.op_count] {
            node.on_note_off(has_lifetime_envs);
        }
    }

    /// Forced release (panic / `release_all` / mono voice switch): guarantees the
    /// voice terminates cleanly even for `OneShot` envelopes or samplers.
    pub fn force_release(&mut self) {
        if !self.active {
            return;
        }
        self.released = true;
        self.fallback_releasing = true;
        let sr = self.sample_rate;
        let rel_coeff = decay_coeff_60db_ms(FAST_RELEASE_MS, sr);
        self.fallback_release_coeff = rel_coeff;
        for node in &mut self.nodes[..self.op_count] {
            node.on_force_release(sr, rel_coeff);
        }
    }

    /// Fast ~3.5 ms choke fade for drum-kit choke groups (cancels pending bursts & pre-delay).
    pub fn choke(&mut self, sample_rate: f64) {
        if !self.active {
            return;
        }
        let sr = sanitize_sample_rate(sample_rate);
        let choke_coeff = decay_coeff_60db_ms(CHOKE_MS, sr);
        self.released = true;
        self.fallback_releasing = true;
        self.fallback_release_coeff = choke_coeff;
        for node in &mut self.nodes[..self.op_count] {
            node.on_choke(sr, choke_coeff);
        }
    }

    #[inline]
    fn smooth_params(&mut self) {
        let s64 = self.smoothing;
        let s32 = s64 as f32;
        smooth_step_f64(&mut self.velocity, self.target_velocity, s64);
        smooth_step_f32(&mut self.zone_pan, self.target_zone_pan, s32);
        smooth_step_f64(&mut self.rack_tune, self.target_rack_tune, s64);

        for ci in 0..self.cable_count {
            smooth_step_f32(&mut self.cable_current[ci], self.cable_target[ci], s32);
        }

        for node in &mut self.nodes[..self.op_count] {
            node.smooth_step(s64);
        }
    }

    #[inline]
    fn update_control_rate_states(&mut self) {
        if self.cached_glide_ms.to_bits() != self.voice_settings.glide_ms.to_bits() {
            self.sync_glide_coeff();
        }
        let sr = self.sample_rate;
        for node in &mut self.nodes[..self.op_count] {
            node.control_tick(sr);
        }
    }

    /// Execute one full stereo audio frame of the compiled Eurorack DAG (`f64` internal,
    /// `[f32; 2]` voice output). Zero allocations, zero locks, zero panics.
    #[inline]
    pub fn next_frame(&mut self, bank: &SampleBank, preview: Option<&Arc<SampleData>>) -> [f32; 2] {
        let mut empty_mods = [0.0_f32; 0];
        let mut empty_cables = [0.0_f32; 0];
        self.next_frame_inner::<false>(bank, preview, &mut empty_mods, &mut empty_cables)
    }

    #[inline(always)]
    fn next_frame_inner<const RECORD_ACTIVITY: bool>(
        &mut self,
        bank: &SampleBank,
        preview: Option<&Arc<SampleData>>,
        mod_peaks: &mut [f32],
        cable_peaks: &mut [f32],
    ) -> [f32; 2] {
        if !self.active || self.op_count == 0 {
            return [0.0; 2];
        }
        if !self.released
            && self
                .gate_frames
                .is_some_and(|gate| self.age >= u64::from(gate))
        {
            self.note_off();
        }

        self.smooth_params();

        if self.voice_settings.glide_ms > 0.0 {
            smooth_step_f64(
                &mut self.note_pitch,
                self.target_note_pitch,
                self.glide_coeff,
            );
        } else {
            self.note_pitch = self.target_note_pitch;
        }

        if self.fallback_releasing {
            self.fallback_env *= self.fallback_release_coeff;
            if self.fallback_env < TAIL_SILENCE_FLOOR_F64 {
                self.fallback_env = 0.0;
            }
        }

        let control_tick = self.control_counter == 0;
        if control_tick {
            self.update_control_rate_states();
        }
        self.control_counter = self.control_counter.wrapping_add(1) & self.control_mask;

        let op_count = self.op_count;
        let cable_count = self.cable_count;
        let mut pctx = ProcessCtx {
            sample_rate: self.sample_rate,
            bpm: self.bpm,
            note_pitch: self.note_pitch,
            rack_tune: self.rack_tune,
            velocity: self.velocity,
            zone_pan: self.zone_pan,
            control_tick,
            use_fallback_gate: self.lifetime_env_count == 0 && !self.sampler_only,
            fallback_env: self.fallback_env,
            bank,
            preview,
            resonator_banks: &mut self.resonator_banks,
        };

        for s in 0..op_count {
            let inputs = gather_node_inputs::<RECORD_ACTIVITY>(
                self.ops[s],
                &self.ops[..op_count],
                &self.cables[..cable_count],
                &self.cable_current[..cable_count],
                &self.slots[..op_count],
                &self.prev_slots[..op_count],
                cable_peaks,
            );
            let out = self.nodes[s].process(&inputs, &mut pctx);
            let flushed = [flush_denormal_f64(out[0]), flush_denormal_f64(out[1])];
            self.slots[s] = flushed;
            if RECORD_ACTIVITY {
                if let Some(peak_slot) = mod_peaks.get_mut(s) {
                    let mag = flushed[0].abs().max(flushed[1].abs()) as f32;
                    if mag.is_finite() && mag > *peak_slot {
                        *peak_slot = mag;
                    }
                }
            }
        }

        self.prev_slots[..op_count].copy_from_slice(&self.slots[..op_count]);

        let out_idx = usize::from(self.out_slot).min(op_count - 1);
        let raw_out = self.slots[out_idx];
        let out = [raw_out[0] as f32, raw_out[1] as f32];

        self.age = self.age.saturating_add(1);
        self.update_voice_lifetime(control_tick);

        if !out[0].is_finite() || !out[1].is_finite() {
            self.active = false;
            return [0.0; 2];
        }
        out
    }

    /// Execute up to `out.len()` consecutive frames for this voice while keeping its
    /// entire modular state hot in L1D cache. Returns the number of active frames
    /// written (`0..=out.len()`) before voice retirement.
    pub fn render_block(
        &mut self,
        bank: &SampleBank,
        preview: Option<&Arc<SampleData>>,
        out: &mut [[f32; 2]],
    ) -> usize {
        self.render_block_with_activity(bank, preview, out, None)
    }

    /// Execute up to `out.len()` consecutive frames, optionally merging per-module and
    /// per-cable signal peaks into `activity` once per block with zero heap allocations.
    pub fn render_block_with_activity(
        &mut self,
        bank: &SampleBank,
        preview: Option<&Arc<SampleData>>,
        out: &mut [[f32; 2]],
        activity: Option<&mut RackActivityTelemetry>,
    ) -> usize {
        if !self.active || self.op_count == 0 {
            return 0;
        }
        let mut written = 0usize;
        match activity {
            None => {
                let mut empty_mods = [0.0_f32; 0];
                let mut empty_cables = [0.0_f32; 0];
                for slot in out.iter_mut() {
                    if !self.active {
                        break;
                    }
                    *slot = self.next_frame_inner::<false>(
                        bank,
                        preview,
                        &mut empty_mods,
                        &mut empty_cables,
                    );
                    written += 1;
                }
            }
            Some(telemetry) => {
                let mut mod_peaks = [0.0_f32; MAX_MODULES_PER_ZONE];
                let mut cable_peaks = [0.0_f32; MAX_CABLES_PER_ZONE];
                for slot in out.iter_mut() {
                    if !self.active {
                        break;
                    }
                    *slot = self.next_frame_inner::<true>(
                        bank,
                        preview,
                        &mut mod_peaks,
                        &mut cable_peaks,
                    );
                    written += 1;
                }
                if written > 0 {
                    for (op, &peak) in self.ops[..self.op_count].iter().zip(&mod_peaks) {
                        telemetry.merge_module_peak(op.module_id, peak);
                    }
                    for (cable, &peak) in self.cables[..self.cable_count].iter().zip(&cable_peaks) {
                        telemetry.merge_cable_peak(cable.cable_id, peak);
                    }
                }
            }
        }
        written
    }

    #[inline]
    fn all_env_gates_closed(&self) -> bool {
        self.env_gated_vcas[..self.env_gated_vca_count]
            .iter()
            .all(|&s| match self.nodes.get(usize::from(s)) {
                Some(ModuleNode::Vca(node)) => node.is_bias_closed(),
                _ => true,
            })
    }

    #[inline]
    fn max_tail_resonator_decay_ms(&self, use_post_gate_only: bool) -> f64 {
        if self.lifetime_env_count == 0 && !self.sampler_only {
            // Fallback gate attenuates `Out` directly; no resonator sits post-gate.
            return 0.0;
        }
        let mut best = 0.0_f64;
        if use_post_gate_only {
            for &slot in &self.post_gate_tail_slots[..self.post_gate_tail_count] {
                if let Some(ModuleNode::Resonator(res)) = self.nodes.get(usize::from(slot)) {
                    best = best.max(res.max_tail_decay_ms());
                }
            }
        } else {
            for node in &self.nodes[..self.op_count] {
                if let ModuleNode::Resonator(res) = node {
                    best = best.max(res.max_tail_decay_ms());
                }
            }
        }
        best
    }

    #[inline]
    fn compute_tail_frames(&self, max_res_decay_ms: f64) -> u32 {
        let tail_sec = if max_res_decay_ms > 0.0 {
            (max_res_decay_ms * 0.001 * RESONATOR_TAIL_SAFETY_MULT)
                .clamp(DEFAULT_TAIL_TIMEOUT_SEC, MAX_RESONATOR_TAIL_SEC)
        } else {
            DEFAULT_TAIL_TIMEOUT_SEC
        };
        (self.sample_rate * tail_sec) as u32
    }

    #[inline]
    fn update_voice_lifetime(&mut self, control_tick: bool) {
        let envs_done = if self.lifetime_env_count > 0 {
            self.lifetime_envs[..self.lifetime_env_count]
                .iter()
                .all(|&s| match self.nodes.get(usize::from(s)) {
                    Some(ModuleNode::Env(node)) => node.env.is_done(),
                    _ => true,
                })
        } else {
            true
        };

        let samplers_done = if self.sampler_slot_count > 0 {
            self.sampler_slots[..self.sampler_slot_count]
                .iter()
                .all(|&s| match self.nodes.get(usize::from(s)) {
                    Some(ModuleNode::Sampler(node)) => !node.cursor.is_playing(),
                    _ => true,
                })
        } else {
            true
        };

        let sources_done = if self.lifetime_env_count > 0 {
            let auto_finish_on_samplers = self.sampler_only && !self.any_sampler_has_trig_in;
            envs_done || (auto_finish_on_samplers && samplers_done)
        } else if self.sampler_only {
            if self.any_sampler_has_trig_in {
                samplers_done && self.released && self.fallback_env <= TAIL_SILENCE_FLOOR_F64
            } else {
                samplers_done
            }
        } else {
            self.fallback_releasing && self.fallback_env <= TAIL_SILENCE_FLOOR_F64
        };

        if !sources_done {
            return;
        }

        if self.has_resonator
            && (self.tail_elapsed_frames == 0 || control_tick || self.tail_frames <= 1)
        {
            let use_post_gate_only = envs_done && self.all_env_gates_closed();
            let budget =
                self.compute_tail_frames(self.max_tail_resonator_decay_ms(use_post_gate_only));
            let needed = budget.saturating_sub(self.tail_elapsed_frames);
            if needed > self.tail_frames {
                self.tail_frames = needed;
            }
        }

        self.tail_elapsed_frames = self.tail_elapsed_frames.saturating_add(1);
        self.tail_frames = self.tail_frames.saturating_sub(1);
        if self.tail_frames == 0 || self.tails_quiet(envs_done) {
            self.active = false;
        }
    }

    #[inline]
    fn tails_quiet(&self, envs_done: bool) -> bool {
        if self.lifetime_env_count == 0 && !self.sampler_only {
            let out_idx = usize::from(self.out_slot).min(self.op_count.saturating_sub(1));
            return self
                .nodes
                .get(out_idx)
                .is_none_or(|node| node.is_tail_quiet(&self.resonator_banks));
        }
        if envs_done && self.all_env_gates_closed() {
            self.post_gate_tail_slots[..self.post_gate_tail_count]
                .iter()
                .all(|&s| {
                    self.nodes
                        .get(usize::from(s))
                        .is_none_or(|node| node.is_tail_quiet(&self.resonator_banks))
                })
        } else {
            self.nodes[..self.op_count]
                .iter()
                .all(|node| node.is_tail_quiet(&self.resonator_banks))
        }
    }
}

#[inline]
fn slot_has_in_port(
    ops: &[CompiledOp],
    cables: &[CompiledCable],
    slot: usize,
    port: InPort,
) -> bool {
    let Some(op) = ops.get(slot) else {
        return false;
    };
    let start = usize::from(op.in_cable_start).min(cables.len());
    let end = (start + usize::from(op.in_cable_len)).min(cables.len());
    cables[start..end].iter().any(|c| c.to_port == port)
}

/// Find the highest `vel_amount` of any `AcidLowPass` `Filter` driven on `Cv` by `env_slot`.
fn acid_filter_vel_amount_for_env(
    cables: &[CompiledCable],
    nodes: &[ModuleNode],
    env_slot: u8,
) -> Option<f32> {
    let mut best: Option<f32> = None;
    for c in cables {
        if c.from_slot == env_slot && c.to_port == InPort::Cv {
            if let Some(ModuleNode::Filter(flt)) = nodes.get(usize::from(c.to_slot)) {
                if flt.target.mode == FilterMode::AcidLowPass && flt.target.vel_amount > 0.0 {
                    best =
                        Some(best.map_or(flt.target.vel_amount, |b| b.max(flt.target.vel_amount)));
                }
            }
        }
    }
    best
}

/// Gather all incoming stereo audio, CV, pitch, FM, PWM, and trigger signals for one module slot.
#[inline(always)]
fn gather_node_inputs<const RECORD_ACTIVITY: bool>(
    op: CompiledOp,
    ops: &[CompiledOp],
    cables: &[CompiledCable],
    cable_current: &[f32],
    slots: &[[f64; 2]],
    prev_slots: &[[f64; 2]],
    cable_peaks: &mut [f32],
) -> NodeInputs {
    let mut inputs = NodeInputs::default();
    let c_start = usize::from(op.in_cable_start).min(cables.len());
    let c_end = (c_start + usize::from(op.in_cable_len)).min(cables.len());

    for ci in c_start..c_end {
        let c = cables[ci];
        let from = usize::from(c.from_slot);
        if from >= ops.len() {
            continue;
        }
        let raw_src = if c.feedback {
            [
                voice_guard_f64(prev_slots[from][0]),
                voice_guard_f64(prev_slots[from][1]),
            ]
        } else {
            slots[from]
        };
        let amt_f32 = cable_current[ci];
        if RECORD_ACTIVITY {
            if let Some(peak_slot) = cable_peaks.get_mut(ci) {
                let src_mag = raw_src[0].abs().max(raw_src[1].abs()) as f32;
                let level = cable_signal_level(op.kind, c.to_port, src_mag, amt_f32);
                if level > *peak_slot {
                    *peak_slot = level;
                }
            }
        }
        let amt = f64::from(amt_f32);
        match c.to_port {
            InPort::In => {
                inputs.in_audio[0] += raw_src[0] * amt;
                inputs.in_audio[1] += raw_src[1] * amt;
            }
            InPort::Cv => {
                inputs.cv_stereo[0] += raw_src[0] * amt;
                inputs.cv_stereo[1] += raw_src[1] * amt;
                if op.kind == ModuleKindTag::Filter && ops[from].kind == ModuleKindTag::Env {
                    let env_lvl = stereo_mid_f64(raw_src).clamp(0.0, 1.0);
                    if env_lvl > inputs.filter_env_level {
                        inputs.filter_env_level = env_lvl;
                    }
                }
            }
            InPort::PitchCv => {
                inputs.pitch_cv += stereo_mid_f64(raw_src) * amt;
            }
            InPort::FmIn => {
                let mono = stereo_mid_f64(raw_src);
                let depth = amt.signum() * fm_depth_from_amount(amt.abs() as f32);
                inputs.fm_cycles += mono * depth;
            }
            InPort::PwCv => {
                inputs.pw_cv += stereo_mid_f64(raw_src) * amt;
            }
            InPort::TrigIn => {
                inputs.trig_in += stereo_mid_f64(raw_src) * amt;
            }
        }
    }
    inputs
}

#[cfg(test)]
mod tests;
