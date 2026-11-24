//! Core retained-material domain types, constants, and entity lookups.

use super::{LanePart, LiveTiming, MacroControl, STEPS_PER_BAR};
use crate::mixer::{
    Channel, ChannelRef, ChannelSend, Duck, EffectDevice, EffectKind, EffectParam, MasterChannel,
};
use crate::music::{EndRule, Step};
use crate::params::{InstrumentParam, ParamAddress, ValueMode};
use crate::rack::{PanelPin, PatchCable, RackModule, RackPatch, RackZone};
use serde::{Deserialize, Serialize};

pub(super) const MAX_DERIVED_OFFSET: u32 = 1_048_576;

pub const VERSION: u32 = 1;
pub const MAX_BARS: usize = 2048;
pub const MAX_PATTERNS: usize = 2048;
pub const MAX_LITERAL_STEPS: usize = 1024;
pub const MAX_INSTRUMENTS: usize = 16;
pub const MAX_LANES: usize = 16;
pub const MAX_PLACEMENTS: usize = 4096;
pub const MAX_DEPTH: usize = 32;
pub const MAX_PARTS: usize = 1024;
pub const MAX_LINKS: usize = 16;
pub const MAX_GLOBAL_MODULATIONS: usize = 256;
pub const MAX_SECTIONS: usize = 256;
pub const MAX_TRANSACTION_EDITS: usize = 16_384;

pub type Id = u64;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub version: u32,
    pub title: String,
    pub bpm: f64,
    pub root_midi: u8,
    pub scale: Vec<i8>,
    pub bars: usize,
    pub patterns: Vec<Pattern>,
    pub instruments: Vec<InstrumentInstance>,
    pub parts: Vec<Part>,
    pub lanes: Vec<Lane>,
    pub sections: Vec<Section>,
    pub mixer: MixerGraph,
    pub ducks: Vec<Duck>,
    pub modulations: Vec<Modulation>,
    /// Assignable Project Macros 1–8 (`index` in `0..8`), omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub macros: Vec<MacroControl>,
    /// Launch and live-edit timing defaults.
    #[serde(default, skip_serializing_if = "LiveTiming::is_default")]
    pub timing: LiveTiming,
    /// What song playback does at the end: loop, stop or continue.
    pub end: EndRule,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MixerGraph {
    pub master: MasterChannel,
    pub channels: Vec<Channel>,
}

impl MixerGraph {
    /// Iterate over every insert effect device across all channels and Master.
    pub fn all_inserts(&self) -> impl Iterator<Item = &EffectDevice> {
        self.channels
            .iter()
            .flat_map(|c| c.inserts.iter())
            .chain(self.master.inserts.iter())
    }

    /// Mutably iterate over every insert effect device across all channels and Master.
    pub fn all_inserts_mut(&mut self) -> impl Iterator<Item = &mut EffectDevice> {
        self.channels
            .iter_mut()
            .flat_map(|c| c.inserts.iter_mut())
            .chain(self.master.inserts.iter_mut())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pattern {
    pub id: Id,
    pub name: String,
    pub kind: PatternKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueUnit {
    Degrees,
    Normalized,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValuePoint {
    pub step: u32,
    pub value: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArpMode {
    Up,
    Down,
    UpDown,
    Random,
}

impl ArpMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "up" | "u" => Some(Self::Up),
            "down" | "dn" | "d" => Some(Self::Down),
            "updown" | "up_down" | "ud" | "pingpong" => Some(Self::UpDown),
            "random" | "rand" | "rnd" | "r" => Some(Self::Random),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::UpDown => "up-down",
            Self::Random => "random",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PatternKind {
    Notes {
        steps: Vec<Step>,
    },
    Rhythm {
        steps: Vec<bool>,
    },
    Values {
        unit: ValueUnit,
        period: u32,
        mode: ValueMode,
        points: Vec<ValuePoint>,
    },
    SeededValues {
        unit: ValueUnit,
        period: u32,
        seed: u64,
        min: f32,
        max: f32,
    },
    /// Strudel/Tidal-inspired inline mini-notation over `steps` sixteenth-steps per cycle.
    Mini {
        code: String,
        steps: u16,
    },
    /// Euclidean rhythm `E(pulses, steps, rotation)`.
    Euclidean {
        pulses: u16,
        steps: u16,
        rotation: i16,
    },
    AddDegrees {
        notes: Id,
        values: Id,
    },
    Mask {
        notes: Id,
        rhythm: Id,
    },
    Transpose {
        source: Id,
        semitones: i16,
    },
    NotesFromValues {
        values: Id,
        rhythm: Id,
        degree_offset: i16,
        velocity: f32,
        gate: f32,
    },
    /// Time-scaling (`num > den` = fast / triplets / subdivisions, `num < den` = slow).
    TimeScale {
        source: Id,
        num: u16,
        den: u16,
    },
    /// Sub-step 16th-note swing (`amount` in `0.0..=0.45`; delays odd 16th steps cleanly).
    Swing {
        source: Id,
        amount: f32,
    },
    /// Subdivides every active hit into `count` (1..=8) evenly-spaced sub-step retriggers.
    Ratchet {
        source: Id,
        count: u8,
    },
    /// Rotates/shifts any pattern in time by `steps` sixteenth-steps.
    Shift {
        source: Id,
        steps: i32,
    },
    /// Plays any pattern backwards over its natural period.
    Reverse {
        source: Id,
    },
    /// Deterministic per-hit probability gate using pure `det_hash(seed, id, time, salt)`.
    Chance {
        source: Id,
        probability: f32,
        seed: u32,
    },
    /// Plays `base` normally and switches to `alt` every `interval`-th cycle (bar/period turnaround).
    Every {
        interval: u16,
        base: Id,
        alt: Id,
    },
    /// Arpeggiates held chord notes of `source` at `rate` (1..=4) sub-step notes per step.
    Arp {
        source: Id,
        mode: ArpMode,
        rate: u8,
    },
    /// Folds out-of-range notes by octaves into `[low_midi, high_midi]`.
    RangeFold {
        source: Id,
        low_midi: u8,
        high_midi: u8,
    },
    /// Sequences multiple patterns end-to-end (`parts[0]` then `parts[1]` then ...).
    Concat {
        parts: Vec<Id>,
    },
    /// Layers multiple patterns simultaneously (merging sub-step events sorted by offset).
    Stack {
        parts: Vec<Id>,
    },
    /// Shapes velocity (and optionally gate) of `source` using normalized `Values` patterns.
    Shape {
        source: Id,
        velocity: Id,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gate: Option<Id>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", content = "unit", rename_all = "snake_case")]
pub enum PatternType {
    Notes,
    Rhythm,
    Values(ValueUnit),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum PatternValue {
    Notes(Step),
    Rhythm(bool),
    Value(f32),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstrumentInstance {
    pub id: Id,
    pub name: String,
    pub rack: RackPatch,
    pub channel: Id,
}

/// Per-note values sampled in the placement's clock and captured at onset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterLink {
    pub target: InstrumentParam,
    pub pattern: Id,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Part {
    pub id: Id,
    pub name: String,
    pub pattern: Id,
    pub instrument: Id,
    pub transpose: i16,
    pub velocity_scale: f32,
    pub parameter_links: Vec<ParameterLink>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseMode {
    #[default]
    Song,
    Restart,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exception {
    pub step: u32,
    pub value: Step,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    pub id: Id,
    pub part: Id,
    pub start_step: u32,
    pub length_steps: u32,
    pub phase: PhaseMode,
    pub offset_steps: i32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exceptions: Vec<Exception>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lane {
    pub id: Id,
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<LanePart>,
    pub placements: Vec<Placement>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
    pub id: Id,
    pub name: String,
    pub start_step: u32,
    pub length_steps: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModulationTarget {
    Instrument {
        instrument: Id,
        parameter: InstrumentParam,
    },
    Effect {
        device: Id,
        parameter: EffectParam,
    },
    ChannelGain {
        channel: ChannelRef,
    },
    ChannelPan {
        channel: ChannelRef,
    },
    Send {
        channel: ChannelRef,
        target: Id,
    },
    Macro {
        index: u8,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Modulation {
    pub id: Id,
    pub name: String,
    pub target: ModulationTarget,
    pub pattern: Id,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EventOrigin {
    pub song_step: u32,
    pub lane: Id,
    pub placement: Id,
    pub part: Id,
    pub pattern: Id,
    pub instrument: Id,
    pub pattern_time: i64,
    pub local_exception: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PreviewStep {
    pub time: i64,
    pub value: PatternValue,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Consumer {
    pub kind: String,
    pub id: Id,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PlacementSample {
    pub lane: Id,
    pub placement: Id,
    pub part: Id,
    pub instrument: Id,
    pub song_step: u32,
    pub pattern_time: i64,
    pub value: Step,
    pub eligible: bool,
    pub local_exception: bool,
}

/// Persistent edits only. Platform IO and transport remain application actions.
/// IDs are supplied by the session allocator and never recycle through undo.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Edit {
    PutPattern {
        pattern: Pattern,
    },
    RemovePattern {
        id: Id,
    },
    SetNote {
        pattern: Id,
        step: usize,
        value: Step,
    },
    SetRhythm {
        pattern: Id,
        step: usize,
        active: bool,
    },
    SetValue {
        pattern: Id,
        step: u32,
        value: Option<f32>,
    },
    ResizePattern {
        pattern: Id,
        length: usize,
    },
    ForkPattern {
        source: Id,
        id: Id,
        name: String,
    },
    FreezePattern {
        source: Id,
        id: Id,
        name: String,
        start: i64,
        length: usize,
    },
    CreateInstrument {
        instrument: InstrumentInstance,
    },
    UpdateInstrumentMeta {
        id: Id,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channel: Option<Id>,
    },
    UpdateRackPatch {
        id: Id,
        rack: RackPatch,
    },
    AddRackZone {
        instrument: Id,
        zone: RackZone,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
    },
    RemoveRackZone {
        instrument: Id,
        zone: u16,
    },
    ReorderRackZone {
        instrument: Id,
        zone: u16,
        target_index: usize,
    },
    AddRackModule {
        instrument: Id,
        zone: u16,
        module: RackModule,
    },
    RemoveRackModule {
        instrument: Id,
        zone: u16,
        module: u16,
    },
    RenameRackModule {
        instrument: Id,
        zone: u16,
        module: u16,
        name: String,
    },
    ConnectCable {
        instrument: Id,
        zone: u16,
        cable: PatchCable,
    },
    DisconnectCable {
        instrument: Id,
        zone: u16,
        cable: u16,
    },
    SetCableAmount {
        instrument: Id,
        zone: u16,
        cable: u16,
        amount: f32,
    },
    PinPanelControl {
        instrument: Id,
        pin: PanelPin,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
    },
    UnpinPanelControl {
        instrument: Id,
        index: usize,
    },
    RemoveInstrument {
        id: Id,
    },
    PutPart {
        part: Part,
    },
    RemovePart {
        id: Id,
    },
    PutLane {
        lane: Lane,
    },
    RemoveLane {
        id: Id,
    },
    ReorderLane {
        lane: Id,
        target_index: usize,
    },
    AddLanePart {
        lane: Id,
        entry: LanePart,
    },
    DropLanePart {
        lane: Id,
        part: Id,
    },
    PutPlacement {
        lane: Id,
        placement: Placement,
    },
    RemovePlacement {
        id: Id,
    },
    SetException {
        placement: Id,
        step: u32,
        value: Option<Step>,
    },
    PutSection {
        section: Section,
    },
    RemoveSection {
        id: Id,
    },
    CreateChannel {
        channel: Channel,
    },
    UpdateChannelMeta {
        id: Id,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<ChannelRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sends: Option<Vec<ChannelSend>>,
    },
    RemoveChannel {
        id: Id,
    },
    AddEffect {
        channel: ChannelRef,
        device: EffectDevice,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
    },
    RemoveEffect {
        device: Id,
    },
    MoveEffect {
        device: Id,
        target_channel: ChannelRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_slot: Option<usize>,
    },
    UpdateEffect {
        device: Id,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bypassed: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<EffectKind>,
    },
    SetParam {
        address: ParamAddress,
        value: f32,
    },
    PutModulation {
        modulation: Modulation,
    },
    RemoveModulation {
        id: Id,
    },
    PutDuck {
        duck: Duck,
    },
    RemoveDuck {
        id: Id,
    },
    SetTempo {
        bpm: f64,
    },
    SetKey {
        root_midi: u8,
        scale: Vec<i8>,
    },
    SetEndRule {
        end: EndRule,
    },
    SetBars {
        bars: usize,
    },
    RenameProject {
        title: String,
    },
    SetLiveTiming {
        timing: LiveTiming,
    },
    SetMacroValue {
        index: u8,
        value: f32,
    },
    PutMacro {
        macro_control: MacroControl,
    },
    RemoveMacro {
        index: u8,
    },
}

impl PatternKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Notes { .. } => "Notes",
            Self::Rhythm { .. } => "Rhythm",
            Self::Values { .. } => "Values",
            Self::SeededValues { .. } => "Seeded values",
            Self::Mini { .. } => "Mini",
            Self::Euclidean { .. } => "Euclidean",
            Self::AddDegrees { .. } => "Degree add",
            Self::Mask { .. } => "Mask",
            Self::Transpose { .. } => "Transpose",
            Self::NotesFromValues { .. } => "Notes from values",
            Self::TimeScale { .. } => "Time scale",
            Self::Swing { .. } => "Swing",
            Self::Ratchet { .. } => "Ratchet",
            Self::Shift { .. } => "Shift",
            Self::Reverse { .. } => "Reverse",
            Self::Chance { .. } => "Chance",
            Self::Every { .. } => "Every",
            Self::Arp { .. } => "Arp",
            Self::RangeFold { .. } => "Range fold",
            Self::Concat { .. } => "Concat",
            Self::Stack { .. } => "Stack",
            Self::Shape { .. } => "Shape",
        }
    }
    pub fn sources(&self) -> Vec<Id> {
        match self {
            Self::AddDegrees { notes, values } => vec![*notes, *values],
            Self::Mask { notes, rhythm } => vec![*notes, *rhythm],
            Self::Transpose { source, .. }
            | Self::TimeScale { source, .. }
            | Self::Swing { source, .. }
            | Self::Ratchet { source, .. }
            | Self::Shift { source, .. }
            | Self::Reverse { source }
            | Self::Chance { source, .. }
            | Self::Arp { source, .. }
            | Self::RangeFold { source, .. } => vec![*source],
            Self::NotesFromValues { values, rhythm, .. } => vec![*values, *rhythm],
            Self::Every { base, alt, .. } => vec![*base, *alt],
            Self::Concat { parts } | Self::Stack { parts } => parts.clone(),
            Self::Shape {
                source,
                velocity,
                gate,
            } => {
                let mut v = vec![*source, *velocity];
                if let Some(g) = gate {
                    v.push(*g);
                }
                v
            }
            _ => Vec::new(),
        }
    }
    pub fn source_roles(&self) -> Vec<(Id, &'static str)> {
        match self {
            Self::AddDegrees { notes, values } => vec![(*notes, "Notes"), (*values, "Degrees")],
            Self::Mask { notes, rhythm } => vec![(*notes, "Notes"), (*rhythm, "Mask")],
            Self::Transpose { source, .. } => vec![(*source, "Transposed source")],
            Self::NotesFromValues { values, rhythm, .. } => {
                vec![(*values, "Degrees"), (*rhythm, "Rhythm")]
            }
            Self::TimeScale { source, .. } => vec![(*source, "Time-scaled")],
            Self::Swing { source, .. } => vec![(*source, "Swung")],
            Self::Ratchet { source, .. } => vec![(*source, "Ratcheted")],
            Self::Shift { source, .. } => vec![(*source, "Shifted")],
            Self::Reverse { source } => vec![(*source, "Reversed")],
            Self::Chance { source, .. } => vec![(*source, "Chance")],
            Self::Every { base, alt, .. } => vec![(*base, "Base"), (*alt, "Every alt")],
            Self::Arp { source, .. } => vec![(*source, "Arp chord")],
            Self::RangeFold { source, .. } => vec![(*source, "Folded")],
            Self::Concat { parts } => parts.iter().map(|&id| (id, "Concat")).collect(),
            Self::Stack { parts } => parts.iter().map(|&id| (id, "Stack")).collect(),
            Self::Shape {
                source,
                velocity,
                gate,
            } => {
                let mut v = vec![(*source, "Notes"), (*velocity, "Velocity")];
                if let Some(g) = gate {
                    v.push((*g, "Gate"));
                }
                v
            }
            _ => Vec::new(),
        }
    }
    pub fn remap_sources(&self, mut map: impl FnMut(Id) -> Id) -> Self {
        match self {
            Self::AddDegrees { notes, values } => Self::AddDegrees {
                notes: map(*notes),
                values: map(*values),
            },
            Self::Mask { notes, rhythm } => Self::Mask {
                notes: map(*notes),
                rhythm: map(*rhythm),
            },
            Self::Transpose { source, semitones } => Self::Transpose {
                source: map(*source),
                semitones: *semitones,
            },
            Self::NotesFromValues {
                values,
                rhythm,
                degree_offset,
                velocity,
                gate,
            } => Self::NotesFromValues {
                values: map(*values),
                rhythm: map(*rhythm),
                degree_offset: *degree_offset,
                velocity: *velocity,
                gate: *gate,
            },
            Self::TimeScale { source, num, den } => Self::TimeScale {
                source: map(*source),
                num: *num,
                den: *den,
            },
            Self::Swing { source, amount } => Self::Swing {
                source: map(*source),
                amount: *amount,
            },
            Self::Ratchet { source, count } => Self::Ratchet {
                source: map(*source),
                count: *count,
            },
            Self::Shift { source, steps } => Self::Shift {
                source: map(*source),
                steps: *steps,
            },
            Self::Reverse { source } => Self::Reverse {
                source: map(*source),
            },
            Self::Chance {
                source,
                probability,
                seed,
            } => Self::Chance {
                source: map(*source),
                probability: *probability,
                seed: *seed,
            },
            Self::Every {
                interval,
                base,
                alt,
            } => Self::Every {
                interval: *interval,
                base: map(*base),
                alt: map(*alt),
            },
            Self::Arp { source, mode, rate } => Self::Arp {
                source: map(*source),
                mode: *mode,
                rate: *rate,
            },
            Self::RangeFold {
                source,
                low_midi,
                high_midi,
            } => Self::RangeFold {
                source: map(*source),
                low_midi: *low_midi,
                high_midi: *high_midi,
            },
            Self::Concat { parts } => Self::Concat {
                parts: parts.iter().map(|&id| map(id)).collect(),
            },
            Self::Stack { parts } => Self::Stack {
                parts: parts.iter().map(|&id| map(id)).collect(),
            },
            Self::Shape {
                source,
                velocity,
                gate,
            } => Self::Shape {
                source: map(*source),
                velocity: map(*velocity),
                gate: gate.map(&mut map),
            },
            other => other.clone(),
        }
    }
}
impl PatternValue {
    pub fn notes(&self) -> Option<&Step> {
        if let Self::Notes(step) = self {
            Some(step)
        } else {
            None
        }
    }
    pub fn active(&self) -> bool {
        match self {
            Self::Notes(step) => step.active,
            Self::Rhythm(active) => *active,
            Self::Value(_) => false,
        }
    }
}
impl Part {
    pub fn new(id: Id, name: impl Into<String>, pattern: Id, instrument: Id) -> Self {
        Self {
            id,
            name: name.into(),
            pattern,
            instrument,
            transpose: 0,
            velocity_scale: 1.0,
            parameter_links: Vec::new(),
        }
    }
}
impl Lane {
    pub fn new(id: Id, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            parts: Vec::new(),
            placements: Vec::new(),
        }
    }
}
impl Placement {
    pub fn new(id: Id, part: Id, start_step: u32, length_steps: u32) -> Self {
        Self {
            id,
            part,
            start_step,
            length_steps,
            phase: PhaseMode::Song,
            offset_steps: 0,
            exceptions: Vec::new(),
        }
    }
    pub fn pattern_time(&self, song_step: u32) -> i64 {
        i64::from(match self.phase {
            PhaseMode::Song => song_step,
            PhaseMode::Restart => song_step.saturating_sub(self.start_step),
        }) + i64::from(self.offset_steps)
    }
    pub fn end_step(&self) -> Option<u32> {
        self.start_step.checked_add(self.length_steps)
    }
}
impl Default for Project {
    fn default() -> Self {
        Self::empty()
    }
}

impl Project {
    pub fn arrangement_steps(&self) -> usize {
        self.bars * STEPS_PER_BAR
    }
    pub fn step_seconds(&self) -> f64 {
        15.0 / self.bpm
    }
    pub fn pattern(&self, id: Id) -> Option<&Pattern> {
        self.patterns.iter().find(|x| x.id == id)
    }
    pub fn pattern_mut(&mut self, id: Id) -> Option<&mut Pattern> {
        self.patterns.iter_mut().find(|x| x.id == id)
    }
    pub fn instrument(&self, id: Id) -> Option<&InstrumentInstance> {
        self.instruments.iter().find(|x| x.id == id)
    }
    pub fn instrument_mut(&mut self, id: Id) -> Option<&mut InstrumentInstance> {
        self.instruments.iter_mut().find(|x| x.id == id)
    }
    pub fn part(&self, id: Id) -> Option<&Part> {
        self.parts.iter().find(|x| x.id == id)
    }
    pub fn part_mut(&mut self, id: Id) -> Option<&mut Part> {
        self.parts.iter_mut().find(|x| x.id == id)
    }
    pub fn lane(&self, id: Id) -> Option<&Lane> {
        self.lanes.iter().find(|x| x.id == id)
    }
    pub fn lane_mut(&mut self, id: Id) -> Option<&mut Lane> {
        self.lanes.iter_mut().find(|x| x.id == id)
    }
    pub fn placement(&self, id: Id) -> Option<(&Lane, &Placement)> {
        self.lanes.iter().find_map(|lane| {
            lane.placements
                .iter()
                .find(|p| p.id == id)
                .map(|p| (lane, p))
        })
    }
    pub fn placement_mut(&mut self, id: Id) -> Option<&mut Placement> {
        self.lanes
            .iter_mut()
            .find_map(|lane| lane.placements.iter_mut().find(|p| p.id == id))
    }
    pub fn channel(&self, id: Id) -> Option<&Channel> {
        self.mixer.channels.iter().find(|x| x.id == id)
    }
    pub fn channel_mut(&mut self, id: Id) -> Option<&mut Channel> {
        self.mixer.channels.iter_mut().find(|x| x.id == id)
    }
    /// Iterate over every mixer insert effect device across all channels and Master.
    pub fn all_inserts(&self) -> impl Iterator<Item = &EffectDevice> {
        self.mixer.all_inserts()
    }

    /// Mutably iterate over every mixer insert effect device across all channels and Master.
    pub fn all_inserts_mut(&mut self) -> impl Iterator<Item = &mut EffectDevice> {
        self.mixer.all_inserts_mut()
    }

    pub fn device(&self, id: Id) -> Option<(ChannelRef, &EffectDevice)> {
        self.mixer
            .channels
            .iter()
            .find_map(|channel| {
                channel
                    .inserts
                    .iter()
                    .find(|d| d.id == id)
                    .map(|d| (ChannelRef::Channel(channel.id), d))
            })
            .or_else(|| {
                self.mixer
                    .master
                    .inserts
                    .iter()
                    .find(|d| d.id == id)
                    .map(|d| (ChannelRef::Master, d))
            })
    }
    pub fn device_mut(&mut self, id: Id) -> Option<&mut EffectDevice> {
        self.all_inserts_mut().find(|d| d.id == id)
    }
    pub fn channel_name(&self, channel: ChannelRef) -> Option<&str> {
        match channel {
            ChannelRef::Master => Some("Master"),
            ChannelRef::Channel(id) => self.channel(id).map(|c| c.name.as_str()),
        }
    }
    pub fn macro_control(&self, index: u8) -> Option<&MacroControl> {
        self.macros.iter().find(|m| m.index == index)
    }
    pub fn macro_control_mut(&mut self, index: u8) -> Option<&mut MacroControl> {
        self.macros.iter_mut().find(|m| m.index == index)
    }
    pub fn max_id(&self) -> Id {
        self.patterns
            .iter()
            .map(|x| x.id)
            .chain(self.instruments.iter().map(|x| x.id))
            .chain(self.parts.iter().map(|x| x.id))
            .chain(self.lanes.iter().map(|x| x.id))
            .chain(
                self.lanes
                    .iter()
                    .flat_map(|l| l.placements.iter().map(|x| x.id)),
            )
            .chain(self.sections.iter().map(|x| x.id))
            .chain(self.mixer.channels.iter().map(|x| x.id))
            .chain(self.all_inserts().map(|x| x.id))
            .chain(self.ducks.iter().map(|x| x.id))
            .chain(self.modulations.iter().map(|x| x.id))
            .max()
            .unwrap_or(0)
    }
}
