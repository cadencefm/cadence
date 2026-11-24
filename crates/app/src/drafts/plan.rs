//! Semantic workflow specs, plans, and validation rules for staged object creation & editing.
use super::form::{
    bounded_integer, is_name, is_picker, parse_duration, parse_normalized, parse_position,
    validate_new_name, validate_printable_name, DraftField,
};
use super::workflow::{
    BounceScopeKind, BounceTargetKind, KitSourceChoice, RefChoice, SamplerSourceChoice,
    WorkflowDraft,
};
use crate::bounce::{parse_bar_range, parse_zone_midi_token, BounceScope, BounceTarget};
use crate::recording::RecordingMode;
use anyhow::{ensure, Context, Result};
use cadence_core::macros::{MAX_MACRO_NAME_LEN, MAX_PROJECT_MACROS};
use cadence_core::material::{
    Id, LanePart, ModulationTarget, PatternType, PhaseMode, Project, ValueUnit,
    MAX_GLOBAL_MODULATIONS, MAX_INSTRUMENTS, MAX_LANES, MAX_LITERAL_STEPS, MAX_PARTS, MAX_PATTERNS,
    MAX_PLACEMENTS, MAX_SECTIONS,
};
use cadence_core::mixer::{
    ChannelKind, ChannelRef, MAX_AUDIO_CHANNELS, MAX_DUCKS, MAX_EFFECT_DEVICES, MAX_GROUPS,
    MAX_INSERTS, MAX_RETURNS, MAX_SENDS,
};
use cadence_core::rack::RackTemplate;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkflowKind {
    Instrument,
    RenameInstrument(Id),
    Part,
    RenamePart(Id),
    Lane,
    RenameLane(Id),
    EditLanePart { lane: Id, part: Id },
    FreezePattern(Id),
    Place,
    PlacedPart,
    Section,
    EditSection(Id),
    Channel,
    RenameChannel(Id),
    RenameMacro(u8),
    Effect,
    RenameEffect(Id),
    MoveEffect(Id),
    Send,
    Duck,
    EditDuck(Id),
    Modulation,
    EditModulation(Id),
    ZoneImportFolder(Id),
    SavePreset(Id),
    Bounce,
    Record,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkflowContext {
    pub pattern: Option<Id>,
    pub instrument: Option<Id>,
    pub part: Option<Id>,
    pub lane: Option<Id>,
    pub add_part_to_lane: Option<Id>,
    pub channel: Option<Id>,
    pub mixer_channel: Option<ChannelRef>,
    pub effect: Option<Id>,
    pub macro_slot: Option<u8>,
    pub mod_target: Option<ModulationTarget>,
    pub send_target: Option<Id>,
    pub arrange_step: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ModPatternChoice {
    Existing(Id),
    NewValues { name: String },
}

#[derive(Clone, Debug, PartialEq)]
pub enum PatternChoice {
    Existing(Id),
    NewNotes { name: String, length: usize },
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChannelChoice {
    Existing(Id),
    New { name: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum SoundChoice {
    Template(RackTemplate),
    Preset(String),
    Sampler {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    Folder {
        path: String,
        label: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct InstrumentSpec {
    pub name: String,
    pub sound: SoundChoice,
    pub channel: ChannelChoice,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InstrumentChoice {
    Existing(Id),
    New(InstrumentSpec),
}

#[derive(Clone, Debug, PartialEq)]
pub struct PartSpec {
    pub name: String,
    pub pattern: PatternChoice,
    pub instrument: InstrumentChoice,
    pub transpose: i16,
    pub velocity_scale: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PartChoice {
    Existing(Id),
    New(PartSpec),
}

#[derive(Clone, Debug, PartialEq)]
pub enum LaneChoice {
    Existing(Id),
    New { name: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlacementSpec {
    pub part: PartChoice,
    pub lane: LaneChoice,
    pub start_step: u32,
    pub length_steps: u32,
    pub phase: PhaseMode,
    pub offset_steps: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WorkflowPlan {
    Instrument(InstrumentSpec),
    Part(PartSpec),
    BankPart {
        lane: Id,
        spec: PartSpec,
    },
    RenamePart {
        part: Id,
        name: String,
    },
    Lane {
        name: String,
    },
    RenameLane {
        lane: Id,
        name: String,
    },
    EditLanePart {
        lane: Id,
        entry: LanePart,
    },
    FreezePattern {
        pattern: Id,
        name: String,
        start: i64,
        length: usize,
    },
    Place(PlacementSpec),
    PlacedPart(PlacementSpec),
    Section {
        id: Option<Id>,
        name: String,
        start_step: u32,
        length_steps: u32,
    },
    Channel {
        name: String,
        kind: ChannelKind,
        output: ChannelRef,
    },
    RenameInstrument {
        instrument: Id,
        name: String,
    },
    RenameChannel {
        channel: Id,
        name: String,
    },
    RenameMacro {
        index: u8,
        name: String,
    },
    Effect {
        channel: ChannelRef,
        kind: String,
    },
    RenameEffect {
        device: Id,
        name: String,
    },
    MoveEffect {
        device: Id,
        channel: ChannelRef,
        position: usize,
    },
    Send {
        source: Id,
        target: ChannelChoice,
        amount: f32,
    },
    Duck {
        id: Option<Id>,
        name: String,
        source_part: Id,
        target: ChannelRef,
        depth: f32,
        recovery_ms: f32,
    },
    Modulation {
        id: Option<Id>,
        name: String,
        pattern: ModPatternChoice,
        target: ModulationTarget,
    },
    ZoneImportFolder {
        instrument: Id,
        folder: String,
    },
    SavePreset {
        instrument: Id,
        name: String,
        category: String,
    },
    Bounce {
        scope: BounceScope,
        name: Option<String>,
        create_part: bool,
        target: BounceTarget,
    },
    Record {
        mode: RecordingMode,
        bars: Option<usize>,
        name: Option<String>,
        place_in_arrange: bool,
        target: BounceTarget,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum WorkflowOutcome {
    Continue,
    Cancel,
    ChooseSample,
    BrowseFolder,
    AuditionPreset(Option<String>),
    Submit(Box<WorkflowPlan>),
}

fn channel_insert_count(project: &Project, ch: ChannelRef) -> Option<usize> {
    match ch {
        ChannelRef::Master => Some(project.mixer.master.inserts.len()),
        ChannelRef::Channel(id) => project.channel(id).map(|c| c.inserts.len()),
    }
}

fn total_effect_count(project: &Project) -> usize {
    project.mixer.master.inserts.len()
        + project
            .mixer
            .channels
            .iter()
            .map(|c| c.inserts.len())
            .sum::<usize>()
}

impl WorkflowDraft {
    pub(super) fn validate_field(&self, project: &Project, field: DraftField) -> Result<()> {
        if field == DraftField::Name
            && matches!(self.kind, WorkflowKind::Bounce | WorkflowKind::Record)
        {
            let trimmed = self.name.trim();
            if !trimmed.is_empty() {
                let label = if self.kind == WorkflowKind::Record {
                    "Recording"
                } else {
                    "Bounce"
                };
                validate_printable_name(label, trimmed, 80)?;
            }
            return Ok(());
        }
        if field == DraftField::Name && matches!(self.kind, WorkflowKind::SavePreset(_)) {
            validate_printable_name("Preset", &self.name, 80)?;
            return Ok(());
        }
        if is_name(field) {
            let name = self.named_text(project, field);
            if field == DraftField::Name && matches!(self.kind, WorkflowKind::RenameEffect(_)) {
                validate_printable_name("Effect", &name, 80)?;
                return Ok(());
            }
            if field == DraftField::Name && matches!(self.kind, WorkflowKind::RenameMacro(_)) {
                validate_printable_name("Macro", &name, MAX_MACRO_NAME_LEN)?;
                return Ok(());
            }
            let names: Vec<&str> = match field {
                DraftField::Name => match self.kind {
                    WorkflowKind::Instrument => project
                        .instruments
                        .iter()
                        .map(|i| i.name.as_str())
                        .collect(),
                    WorkflowKind::RenameInstrument(edit_id) => project
                        .instruments
                        .iter()
                        .filter(|i| i.id != edit_id)
                        .map(|i| i.name.as_str())
                        .collect(),
                    WorkflowKind::Lane => project.lanes.iter().map(|l| l.name.as_str()).collect(),
                    WorkflowKind::RenameLane(edit_id) => project
                        .lanes
                        .iter()
                        .filter(|l| l.id != edit_id)
                        .map(|l| l.name.as_str())
                        .collect(),
                    WorkflowKind::RenamePart(edit_id) => project
                        .parts
                        .iter()
                        .filter(|p| p.id != edit_id)
                        .map(|p| p.name.as_str())
                        .collect(),
                    WorkflowKind::FreezePattern(_) => {
                        project.patterns.iter().map(|p| p.name.as_str()).collect()
                    }
                    WorkflowKind::Section => {
                        project.sections.iter().map(|s| s.name.as_str()).collect()
                    }
                    WorkflowKind::EditSection(edit_id) => project
                        .sections
                        .iter()
                        .filter(|s| s.id != edit_id)
                        .map(|s| s.name.as_str())
                        .collect(),
                    WorkflowKind::Channel => project
                        .mixer
                        .channels
                        .iter()
                        .map(|c| c.name.as_str())
                        .collect(),
                    WorkflowKind::RenameChannel(edit_id) => project
                        .mixer
                        .channels
                        .iter()
                        .filter(|c| c.id != edit_id)
                        .map(|c| c.name.as_str())
                        .collect(),
                    WorkflowKind::Duck => project.ducks.iter().map(|d| d.name.as_str()).collect(),
                    WorkflowKind::EditDuck(edit_id) => project
                        .ducks
                        .iter()
                        .filter(|d| d.id != edit_id)
                        .map(|d| d.name.as_str())
                        .collect(),
                    WorkflowKind::Modulation => project
                        .modulations
                        .iter()
                        .map(|m| m.name.as_str())
                        .collect(),
                    WorkflowKind::EditModulation(edit_id) => project
                        .modulations
                        .iter()
                        .filter(|m| m.id != edit_id)
                        .map(|m| m.name.as_str())
                        .collect(),
                    _ => project.parts.iter().map(|b| b.name.as_str()).collect(),
                },
                DraftField::PatternName => {
                    project.patterns.iter().map(|p| p.name.as_str()).collect()
                }
                DraftField::InstrumentName => project
                    .instruments
                    .iter()
                    .map(|i| i.name.as_str())
                    .collect(),
                DraftField::ChannelName => project
                    .mixer
                    .channels
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect(),
                DraftField::PartName => project.parts.iter().map(|b| b.name.as_str()).collect(),
                DraftField::LaneName => project.lanes.iter().map(|l| l.name.as_str()).collect(),
                _ => Vec::new(),
            };
            let limit = if field == DraftField::ChannelName
                || matches!(
                    self.kind,
                    WorkflowKind::Channel
                        | WorkflowKind::RenameChannel(_)
                        | WorkflowKind::RenameInstrument(_)
                        | WorkflowKind::Duck
                        | WorkflowKind::EditDuck(_)
                ) {
                80
            } else {
                120
            };
            validate_new_name(&name, names.into_iter(), limit)?;
        } else {
            match field {
                DraftField::PatternLength => {
                    let n: usize = self
                        .pattern_length
                        .trim()
                        .parse()
                        .context("Enter a whole number of note steps")?;
                    ensure!(
                        (1..=MAX_LITERAL_STEPS).contains(&n),
                        "Note length must be 1..{MAX_LITERAL_STEPS} steps"
                    );
                }
                DraftField::PartTranspose => {
                    bounded_integer(&self.part_transpose, -96, 96, "Part transpose")?;
                }
                DraftField::PartVelocity => {
                    parse_normalized(&self.part_velocity)?;
                }
                DraftField::Start => {
                    if matches!(self.kind, WorkflowKind::FreezePattern(_)) {
                        let pos = parse_position(&self.start)?;
                        ensure!(pos <= 1_048_576, "Freeze start step is out of range");
                    } else {
                        ensure!(
                            parse_position(&self.start)? < project.arrangement_steps() as u32,
                            "Start is outside the arrangement"
                        );
                    }
                }
                DraftField::Duration => {
                    let start = parse_position(&self.start)?;
                    let length = parse_duration(&self.duration)?;
                    let label = if matches!(
                        self.kind,
                        WorkflowKind::Section | WorkflowKind::EditSection(_)
                    ) {
                        "Section"
                    } else {
                        "Placement"
                    };
                    ensure!(
                        start
                            .checked_add(length)
                            .is_some_and(|end| end <= project.arrangement_steps() as u32),
                        "{label} must fit the arrangement; shorten its duration or extend the song first"
                    );
                }
                DraftField::Offset => {
                    let offset: i32 = self
                        .offset
                        .trim()
                        .parse()
                        .context("Enter a signed step offset")?;
                    ensure!(
                        offset.unsigned_abs() <= 1_048_576,
                        "Offset must be within ±1048576 steps"
                    );
                }
                DraftField::EffectSlot => {
                    let slot: usize = self
                        .effect_slot
                        .trim()
                        .parse()
                        .context("Enter a 1-based insert slot number")?;
                    ensure!(slot >= 1, "Insert slots start at 1");
                    let WorkflowKind::MoveEffect(device_id) = self.kind else {
                        return Ok(());
                    };
                    let owner = project.device(device_id).map(|(c, _)| c);
                    let dest = self.effect_channel;
                    let count = channel_insert_count(project, dest).unwrap_or(0);
                    let max_slot = if owner == Some(dest) {
                        count.max(1)
                    } else {
                        (count + 1).min(MAX_INSERTS)
                    };
                    ensure!(
                        slot <= max_slot,
                        "Slot must be 1..={max_slot} on this channel"
                    );
                }
                DraftField::SendAmount => {
                    if !self.send_amount.trim().eq_ignore_ascii_case("off") {
                        parse_normalized(&self.send_amount)?;
                    }
                }
                DraftField::DuckDepth => {
                    parse_normalized(&self.duck_depth)?;
                }
                DraftField::DuckRecovery => {
                    let ms = crate::commands::parse_ms(&self.duck_recovery)?;
                    ensure!(
                        (10.0..=2000.0).contains(&ms),
                        "Duck recovery must be 10..2000 ms"
                    );
                }
                DraftField::Sample => {
                    if let Some((p, _)) = &self.sample {
                        ensure!(!p.trim().is_empty(), "Sample path cannot be empty");
                    }
                    let current = self.selected_choice(DraftField::Sample);
                    ensure!(
                        self.choices(project, DraftField::Sample)
                            .iter()
                            .any(|c| Some(&c.choice) == current.as_ref()),
                        "Choose a visible Sampler source"
                    );
                }
                DraftField::SampleFolder => {
                    ensure!(
                        !self.sample_folder.trim().is_empty(),
                        "Enter a sample folder path (relative to project or sample root)"
                    );
                }
                DraftField::BounceRange => {
                    let tok = self.bounce_range.trim().to_string();
                    let (start_bar, end_bar, _) = parse_bar_range(&[tok])?;
                    let max_bars = project.bars.max(1);
                    ensure!(
                        start_bar >= 1 && start_bar <= end_bar && end_bar <= max_bars,
                        "Bar range must satisfy 1 <= start <= end <= {max_bars}"
                    );
                }
                DraftField::BounceZoneMidi => {
                    parse_zone_midi_token(&self.bounce_zone_midi)
                        .context("Enter MIDI note 0..127 or note name (e.g. C5, F#2)")?;
                }
                DraftField::RecordBars => {
                    let trimmed = self.record_bars.trim();
                    if !trimmed.is_empty()
                        && !trimmed.eq_ignore_ascii_case("open")
                        && !trimmed.eq_ignore_ascii_case("auto")
                    {
                        let n: usize = trimmed
                            .trim_end_matches("bars")
                            .trim_end_matches("bar")
                            .trim()
                            .parse()
                            .context("Enter 1..=64 bars or leave blank for open/loop")?;
                        ensure!(
                            (1..=64).contains(&n),
                            "Recording length must be 1..=64 bars"
                        );
                    }
                }
                f if is_picker(f) => {
                    let current = self.selected_choice(f);
                    ensure!(
                        self.choices(project, f)
                            .iter()
                            .any(|c| Some(&c.choice) == current.as_ref()),
                        "Choose a visible compatible {}",
                        f.label().to_lowercase()
                    );
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn plan(&self, project: &Project) -> Result<WorkflowPlan> {
        for field in self.fields() {
            self.validate_field(project, field)?;
        }
        let plan = match self.kind {
            WorkflowKind::Instrument => WorkflowPlan::Instrument(self.instrument_spec(project)?),
            WorkflowKind::RenameInstrument(instrument) => WorkflowPlan::RenameInstrument {
                instrument,
                name: self.name.trim().into(),
            },
            WorkflowKind::Part => {
                let spec = self.part_spec(project)?;
                if let Some(lane) = self.context.add_part_to_lane {
                    WorkflowPlan::BankPart { lane, spec }
                } else {
                    WorkflowPlan::Part(spec)
                }
            }
            WorkflowKind::RenamePart(part) => WorkflowPlan::RenamePart {
                part,
                name: self.name.trim().into(),
            },
            WorkflowKind::Lane => WorkflowPlan::Lane {
                name: self.name.trim().into(),
            },
            WorkflowKind::RenameLane(lane) => WorkflowPlan::RenameLane {
                lane,
                name: self.name.trim().into(),
            },
            WorkflowKind::EditLanePart { lane, part } => WorkflowPlan::EditLanePart {
                lane,
                entry: LanePart {
                    part,
                    phase: self.phase,
                    offset_steps: self
                        .offset
                        .trim()
                        .parse()
                        .context("Enter a signed step offset")?,
                },
            },
            WorkflowKind::FreezePattern(pattern) => WorkflowPlan::FreezePattern {
                pattern,
                name: self.name.trim().into(),
                start: i64::from(parse_position(&self.start)?),
                length: self
                    .pattern_length
                    .trim()
                    .parse()
                    .context("Enter a whole number of steps to freeze")?,
            },
            WorkflowKind::Place => WorkflowPlan::Place(self.placement_spec(project)?),
            WorkflowKind::PlacedPart => WorkflowPlan::PlacedPart(self.placement_spec(project)?),
            WorkflowKind::Section => WorkflowPlan::Section {
                id: None,
                name: self.name.trim().into(),
                start_step: parse_position(&self.start)?,
                length_steps: parse_duration(&self.duration)?,
            },
            WorkflowKind::EditSection(id) => WorkflowPlan::Section {
                id: Some(id),
                name: self.name.trim().into(),
                start_step: parse_position(&self.start)?,
                length_steps: parse_duration(&self.duration)?,
            },
            WorkflowKind::Channel => WorkflowPlan::Channel {
                name: self.name.trim().into(),
                kind: self.channel_kind,
                output: if self.channel_kind == ChannelKind::Return {
                    ChannelRef::Master
                } else {
                    self.channel_output
                },
            },
            WorkflowKind::RenameChannel(channel) => WorkflowPlan::RenameChannel {
                channel,
                name: self.name.trim().into(),
            },
            WorkflowKind::RenameMacro(index) => WorkflowPlan::RenameMacro {
                index,
                name: self.name.trim().into(),
            },
            WorkflowKind::Effect => WorkflowPlan::Effect {
                channel: self.effect_channel,
                kind: self.effect_kind.into(),
            },
            WorkflowKind::RenameEffect(device) => WorkflowPlan::RenameEffect {
                device,
                name: self.name.trim().into(),
            },
            WorkflowKind::MoveEffect(device) => {
                let slot: usize = self.effect_slot.trim().parse()?;
                WorkflowPlan::MoveEffect {
                    device,
                    channel: self.effect_channel,
                    position: slot.saturating_sub(1),
                }
            }
            WorkflowKind::Send => {
                let source = self.send_source.context("Choose a source channel")?;
                let target = match self.send_target.context("Choose a Return channel")? {
                    RefChoice::Existing(id) => ChannelChoice::Existing(id),
                    RefChoice::New => ChannelChoice::New {
                        name: self
                            .named_text(project, DraftField::ChannelName)
                            .trim()
                            .into(),
                    },
                };
                let amount = if self.send_amount.trim().eq_ignore_ascii_case("off") {
                    0.0
                } else {
                    parse_normalized(&self.send_amount)?
                };
                WorkflowPlan::Send {
                    source,
                    target,
                    amount,
                }
            }
            WorkflowKind::Duck => WorkflowPlan::Duck {
                id: None,
                name: self.name.trim().into(),
                source_part: self
                    .duck_source
                    .context("Choose a sidechain trigger part")?,
                target: self.duck_target,
                depth: parse_normalized(&self.duck_depth)?,
                recovery_ms: crate::commands::parse_ms(&self.duck_recovery)?,
            },
            WorkflowKind::EditDuck(id) => WorkflowPlan::Duck {
                id: Some(id),
                name: self.name.trim().into(),
                source_part: self
                    .duck_source
                    .context("Choose a sidechain trigger part")?,
                target: self.duck_target,
                depth: parse_normalized(&self.duck_depth)?,
                recovery_ms: crate::commands::parse_ms(&self.duck_recovery)?,
            },
            WorkflowKind::Modulation | WorkflowKind::EditModulation(_) => {
                let edit_id = match self.kind {
                    WorkflowKind::EditModulation(id) => Some(id),
                    _ => None,
                };
                let pattern = match self
                    .mod_pattern
                    .context("Choose a normalized Values source")?
                {
                    RefChoice::Existing(id) => ModPatternChoice::Existing(id),
                    RefChoice::New => ModPatternChoice::NewValues {
                        name: self
                            .named_text(project, DraftField::PatternName)
                            .trim()
                            .into(),
                    },
                };
                let target = self
                    .mod_target
                    .context("Choose a modulation target and parameter")?;
                WorkflowPlan::Modulation {
                    id: edit_id,
                    name: self.named_text(project, DraftField::Name).trim().into(),
                    pattern,
                    target,
                }
            }
            WorkflowKind::ZoneImportFolder(instrument) => WorkflowPlan::ZoneImportFolder {
                instrument,
                folder: self.sample_folder.trim().into(),
            },
            WorkflowKind::SavePreset(instrument) => WorkflowPlan::SavePreset {
                instrument,
                name: self.name.trim().into(),
                category: self.preset_category.to_string(),
            },
            WorkflowKind::Bounce => {
                let scope = match self.bounce_scope_kind {
                    BounceScopeKind::Part => {
                        let part_id = self
                            .bounce_part
                            .context("Choose a Part to bounce (or switch Scope to Range)")?;
                        BounceScope::Part(part_id)
                    }
                    BounceScopeKind::Pattern => {
                        let pat_id = self
                            .bounce_pattern
                            .context("Choose a Notes Pattern to bounce")?;
                        BounceScope::Pattern(pat_id)
                    }
                    BounceScopeKind::Range => {
                        let (start_bar, end_bar, _) =
                            parse_bar_range(&[self.bounce_range.trim().to_string()])?;
                        BounceScope::Range { start_bar, end_bar }
                    }
                };
                let name = {
                    let trimmed = self.name.trim();
                    (!trimmed.is_empty()).then(|| trimmed.to_string())
                };
                let target = match self.bounce_target_kind {
                    BounceTargetKind::NewSampler => BounceTarget::NewSampler,
                    BounceTargetKind::ReplaceZone => {
                        let inst_id = self
                            .bounce_zone_instrument
                            .context("Choose a target Rack instrument")?;
                        let inst = project
                            .instrument(inst_id)
                            .context("Target Rack instrument missing")?;
                        let z_idx = inst.rack.clamp_zone_index(self.bounce_zone_index);
                        BounceTarget::RackZone {
                            instrument: inst_id,
                            zone_index: Some(z_idx),
                            midi: None,
                            zone_name: name.clone(),
                        }
                    }
                    BounceTargetKind::NewZoneMidi => {
                        let inst_id = self
                            .bounce_zone_instrument
                            .context("Choose a target Rack instrument")?;
                        let midi = parse_zone_midi_token(&self.bounce_zone_midi)
                            .context("Enter MIDI note 0..127 or note name (e.g. C5)")?;
                        BounceTarget::RackZone {
                            instrument: inst_id,
                            zone_index: None,
                            midi: Some(midi),
                            zone_name: name.clone(),
                        }
                    }
                };
                WorkflowPlan::Bounce {
                    scope,
                    name,
                    create_part: self.bounce_create_part,
                    target,
                }
            }
            WorkflowKind::Record => {
                let name = {
                    let trimmed = self.name.trim();
                    (!trimmed.is_empty()).then(|| trimmed.to_string())
                };
                let bars = {
                    let trimmed = self.record_bars.trim();
                    if trimmed.is_empty()
                        || trimmed.eq_ignore_ascii_case("open")
                        || trimmed.eq_ignore_ascii_case("auto")
                    {
                        None
                    } else {
                        let n: usize = trimmed
                            .trim_end_matches("bars")
                            .trim_end_matches("bar")
                            .trim()
                            .parse()
                            .context("Enter 1..=64 bars or leave blank for open/loop")?;
                        ensure!(
                            (1..=64).contains(&n),
                            "Recording length must be 1..=64 bars"
                        );
                        Some(n)
                    }
                };
                let target = match self.bounce_target_kind {
                    BounceTargetKind::NewSampler => BounceTarget::NewSampler,
                    BounceTargetKind::ReplaceZone => {
                        let inst_id = self
                            .bounce_zone_instrument
                            .context("Choose a target Rack instrument")?;
                        let inst = project
                            .instrument(inst_id)
                            .context("Target Rack instrument missing")?;
                        let z_idx = inst.rack.clamp_zone_index(self.bounce_zone_index);
                        BounceTarget::RackZone {
                            instrument: inst_id,
                            zone_index: Some(z_idx),
                            midi: None,
                            zone_name: name.clone(),
                        }
                    }
                    BounceTargetKind::NewZoneMidi => {
                        let inst_id = self
                            .bounce_zone_instrument
                            .context("Choose a target Rack instrument")?;
                        let midi = parse_zone_midi_token(&self.bounce_zone_midi)
                            .context("Enter MIDI note 0..127 or note name (e.g. C5)")?;
                        BounceTarget::RackZone {
                            instrument: inst_id,
                            zone_index: None,
                            midi: Some(midi),
                            zone_name: name.clone(),
                        }
                    }
                };
                WorkflowPlan::Record {
                    mode: self.record_mode,
                    bars,
                    name,
                    place_in_arrange: self.bounce_create_part,
                    target,
                }
            }
        };
        plan.validate(project)?;
        Ok(plan)
    }

    fn instrument_spec(&self, project: &Project) -> Result<InstrumentSpec> {
        let sound = match self.rack_template {
            RackTemplate::Synth => match &self.preset {
                Some(id) => SoundChoice::Preset(id.clone()),
                None => SoundChoice::Template(RackTemplate::Synth),
            },
            RackTemplate::Modular => match &self.preset {
                Some(id) => SoundChoice::Preset(id.clone()),
                None => SoundChoice::Template(RackTemplate::Modular),
            },
            RackTemplate::Sampler => match &self.sampler_source {
                SamplerSourceChoice::SelectedSample => match &self.sample {
                    Some((path, label)) => SoundChoice::Sampler {
                        path: Some(path.clone()),
                        label: Some(label.clone()),
                    },
                    None => SoundChoice::Template(RackTemplate::Sampler),
                },
                SamplerSourceChoice::Blank | SamplerSourceChoice::Browse => {
                    SoundChoice::Template(RackTemplate::Sampler)
                }
                SamplerSourceChoice::Preset(id) => SoundChoice::Preset(id.clone()),
            },
            RackTemplate::Kit => match &self.kit_source {
                KitSourceChoice::Blank => SoundChoice::Template(RackTemplate::Kit),
                KitSourceChoice::Preset(kit) => SoundChoice::Preset(kit.clone()),
                KitSourceChoice::Folder => {
                    let path = self.sample_folder.trim().to_string();
                    ensure!(!path.is_empty(), "Enter a sample folder path");
                    let label = std::path::Path::new(&path)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .filter(|s| !s.is_empty())
                        .unwrap_or("Kit")
                        .to_string();
                    SoundChoice::Folder { path, label }
                }
            },
        };
        let channel = match self.channel.context("Choose an Audio destination")? {
            RefChoice::Existing(id) => ChannelChoice::Existing(id),
            RefChoice::New => ChannelChoice::New {
                name: self
                    .named_text(project, DraftField::ChannelName)
                    .trim()
                    .into(),
            },
        };
        Ok(InstrumentSpec {
            name: self.instrument_name(project).trim().into(),
            sound,
            channel,
        })
    }

    fn part_spec(&self, project: &Project) -> Result<PartSpec> {
        let pattern = match self.pattern.context("Choose note material")? {
            RefChoice::Existing(id) => PatternChoice::Existing(id),
            RefChoice::New => PatternChoice::NewNotes {
                name: self
                    .named_text(project, DraftField::PatternName)
                    .trim()
                    .into(),
                length: self.pattern_length.trim().parse()?,
            },
        };
        let instrument = match self.instrument.context("Choose a sound")? {
            RefChoice::Existing(id) => InstrumentChoice::Existing(id),
            RefChoice::New => InstrumentChoice::New(self.instrument_spec(project)?),
        };
        Ok(PartSpec {
            name: self.part_name(project).trim().into(),
            pattern,
            instrument,
            transpose: bounded_integer(&self.part_transpose, -96, 96, "Part transpose")?,
            velocity_scale: parse_normalized(&self.part_velocity)?,
        })
    }

    fn placement_spec(&self, project: &Project) -> Result<PlacementSpec> {
        let part = if self.kind == WorkflowKind::PlacedPart {
            PartChoice::New(self.part_spec(project)?)
        } else {
            match self.part.context("Choose a part")? {
                RefChoice::Existing(id) => PartChoice::Existing(id),
                RefChoice::New => PartChoice::New(self.part_spec(project)?),
            }
        };
        let lane = match self.lane.context("Choose a lane")? {
            RefChoice::Existing(id) => LaneChoice::Existing(id),
            RefChoice::New => LaneChoice::New {
                name: self.named_text(project, DraftField::LaneName).trim().into(),
            },
        };
        Ok(PlacementSpec {
            part,
            lane,
            start_step: parse_position(&self.start)?,
            length_steps: parse_duration(&self.duration)?,
            phase: self.phase,
            offset_steps: self.offset.trim().parse()?,
        })
    }
}

impl WorkflowPlan {
    pub fn validate(&self, project: &Project) -> Result<()> {
        let mut counts = [0usize; 5]; // patterns, instruments, Audio channels, parts, lanes
        match self {
            Self::Instrument(i) => validate_instrument(project, i, &mut counts)?,
            Self::Part(b) => validate_part(project, b, &mut counts)?,
            Self::BankPart { lane, spec } => {
                ensure!(project.lane(*lane).is_some(), "Lane no longer exists");
                validate_part(project, spec, &mut counts)?;
            }
            Self::RenamePart { part, name } => {
                ensure!(project.part(*part).is_some(), "Part no longer exists");
                validate_new_name(
                    name,
                    project
                        .parts
                        .iter()
                        .filter(|p| p.id != *part)
                        .map(|p| p.name.as_str()),
                    120,
                )?;
            }
            Self::Lane { name } => {
                validate_new_name(name, project.lanes.iter().map(|l| l.name.as_str()), 120)?;
                counts[4] += 1;
            }
            Self::RenameLane { lane, name } => {
                ensure!(project.lane(*lane).is_some(), "Lane no longer exists");
                validate_new_name(
                    name,
                    project
                        .lanes
                        .iter()
                        .filter(|l| l.id != *lane)
                        .map(|l| l.name.as_str()),
                    120,
                )?;
            }
            Self::EditLanePart { lane, entry } => {
                ensure!(project.lane(*lane).is_some(), "Lane no longer exists");
                ensure!(project.part(entry.part).is_some(), "Part no longer exists");
                ensure!(
                    entry.offset_steps.unsigned_abs() <= 1_048_576,
                    "Offset must be within ±1048576 steps"
                );
            }
            Self::FreezePattern {
                pattern,
                name,
                length,
                ..
            } => {
                ensure!(
                    project.pattern(*pattern).is_some(),
                    "Pattern no longer exists"
                );
                validate_new_name(name, project.patterns.iter().map(|p| p.name.as_str()), 120)?;
                ensure!(
                    (1..=MAX_LITERAL_STEPS).contains(length),
                    "Freeze length must be 1..{MAX_LITERAL_STEPS} steps"
                );
                counts[0] += 1;
            }
            Self::Place(p) | Self::PlacedPart(p) => {
                let instrument = match &p.part {
                    PartChoice::Existing(id) => Some(
                        project
                            .part(*id)
                            .context("Part no longer exists")?
                            .instrument,
                    ),
                    PartChoice::New(b) => {
                        validate_part(project, b, &mut counts)?;
                        match b.instrument {
                            InstrumentChoice::Existing(id) => Some(id),
                            InstrumentChoice::New(_) => None,
                        }
                    }
                };
                let lane = match &p.lane {
                    LaneChoice::Existing(id) => {
                        let lane = project.lane(*id).context("Lane no longer exists")?;
                        Some(lane)
                    }
                    LaneChoice::New { name } => {
                        validate_new_name(
                            name,
                            project.lanes.iter().map(|l| l.name.as_str()),
                            120,
                        )?;
                        counts[4] += 1;
                        None
                    }
                };
                let end = p
                    .start_step
                    .checked_add(p.length_steps)
                    .context("Placement range overflow")?;
                ensure!(
                    p.length_steps > 0 && end <= project.arrangement_steps() as u32,
                    "Placement must fit the arrangement"
                );
                ensure!(
                    p.offset_steps.unsigned_abs() <= 1_048_576,
                    "Offset must be within ±1048576 steps"
                );
                let overlaps = |start: u32, length: u32| {
                    start < end && p.start_step < start.saturating_add(length)
                };
                if let Some(lane) = lane {
                    ensure!(
                        !lane
                            .placements
                            .iter()
                            .any(|q| overlaps(q.start_step, q.length_steps)),
                        "Placement overlaps existing material in this lane; choose another time or lane"
                    );
                }
                if let Some(instrument) = instrument {
                    ensure!(
                        !project
                            .lanes
                            .iter()
                            .flat_map(|l| &l.placements)
                            .any(|q| project
                                .part(q.part)
                                .is_some_and(|b| b.instrument == instrument)
                                && overlaps(q.start_step, q.length_steps)),
                        "This instrument already plays in that range; choose a new instrument instance or a different time"
                    );
                }
                ensure!(
                    project
                        .lanes
                        .iter()
                        .map(|l| l.placements.len())
                        .sum::<usize>()
                        < MAX_PLACEMENTS,
                    "Placement limit reached"
                );
            }
            Self::Section {
                id,
                name,
                start_step,
                length_steps,
            } => {
                if let Some(edit_id) = *id {
                    ensure!(
                        project.sections.iter().any(|s| s.id == edit_id),
                        "Section no longer exists"
                    );
                } else {
                    ensure!(
                        project.sections.len() < MAX_SECTIONS,
                        "Section limit reached ({MAX_SECTIONS})"
                    );
                }
                validate_new_name(
                    name,
                    project
                        .sections
                        .iter()
                        .filter(|s| Some(s.id) != *id)
                        .map(|s| s.name.as_str()),
                    120,
                )?;
                let end = start_step
                    .checked_add(*length_steps)
                    .context("Section range overflow")?;
                ensure!(
                    *length_steps > 0 && end <= project.arrangement_steps() as u32,
                    "Section must fit the arrangement"
                );
            }
            Self::Channel { name, kind, output } => {
                validate_new_name(
                    name,
                    project.mixer.channels.iter().map(|c| c.name.as_str()),
                    80,
                )?;
                let (existing, limit, label) = match kind {
                    ChannelKind::Audio => (
                        project
                            .mixer
                            .channels
                            .iter()
                            .filter(|c| c.kind == ChannelKind::Audio)
                            .count(),
                        MAX_AUDIO_CHANNELS,
                        "Audio",
                    ),
                    ChannelKind::Group => (
                        project
                            .mixer
                            .channels
                            .iter()
                            .filter(|c| c.kind == ChannelKind::Group)
                            .count(),
                        MAX_GROUPS,
                        "Group",
                    ),
                    ChannelKind::Return => (
                        project
                            .mixer
                            .channels
                            .iter()
                            .filter(|c| c.kind == ChannelKind::Return)
                            .count(),
                        MAX_RETURNS,
                        "Return",
                    ),
                };
                ensure!(existing < limit, "{label}-channel limit reached ({limit})");
                match output {
                    ChannelRef::Master => {}
                    ChannelRef::Channel(dest_id) => {
                        ensure!(
                            *kind != ChannelKind::Return,
                            "Return channels always route to Master"
                        );
                        ensure!(
                            project
                                .channel(*dest_id)
                                .is_some_and(|c| c.kind == ChannelKind::Group),
                            "Output destination must be Master or an existing Group channel"
                        );
                    }
                }
            }
            Self::RenameInstrument { instrument, name } => {
                ensure!(
                    project.instrument(*instrument).is_some(),
                    "Instrument no longer exists"
                );
                validate_new_name(
                    name,
                    project
                        .instruments
                        .iter()
                        .filter(|i| i.id != *instrument)
                        .map(|i| i.name.as_str()),
                    80,
                )?;
            }
            Self::RenameChannel { channel, name } => {
                ensure!(
                    project.channel(*channel).is_some(),
                    "Channel no longer exists"
                );
                validate_new_name(
                    name,
                    project
                        .mixer
                        .channels
                        .iter()
                        .filter(|c| c.id != *channel)
                        .map(|c| c.name.as_str()),
                    80,
                )?;
            }
            Self::RenameEffect { device, name } => {
                ensure!(project.device(*device).is_some(), "Effect no longer exists");
                validate_printable_name("Effect", name, 80)?;
            }
            Self::RenameMacro { index, name } => {
                ensure!(
                    usize::from(*index) < MAX_PROJECT_MACROS,
                    "Macro slot must be 1..={MAX_PROJECT_MACROS}"
                );
                validate_printable_name("Macro", name, MAX_MACRO_NAME_LEN)?;
            }
            Self::Effect { channel, kind } => {
                ensure!(
                    cadence_core::mixer::EffectDevice::is_kind_name(kind),
                    "Unknown effect kind '{kind}'"
                );
                let inserts = channel_insert_count(project, *channel)
                    .context("Target channel no longer exists")?;
                ensure!(
                    inserts < MAX_INSERTS,
                    "Channel already has {MAX_INSERTS} inserts"
                );
                ensure!(
                    total_effect_count(project) < MAX_EFFECT_DEVICES,
                    "Effect device limit reached ({MAX_EFFECT_DEVICES})"
                );
            }
            Self::MoveEffect {
                device,
                channel,
                position,
            } => {
                let (owner, _) = project
                    .device(*device)
                    .context("Effect device no longer exists")?;
                let inserts = channel_insert_count(project, *channel)
                    .context("Target channel no longer exists")?;
                if owner == *channel {
                    ensure!(*position < inserts, "Target slot is out of range");
                } else {
                    ensure!(
                        inserts < MAX_INSERTS,
                        "Destination channel already has {MAX_INSERTS} inserts"
                    );
                    ensure!(*position <= inserts, "Target slot is out of range");
                }
            }
            Self::Send {
                source,
                target,
                amount,
            } => {
                let src = project
                    .channel(*source)
                    .context("Source channel no longer exists")?;
                ensure!(
                    src.kind != ChannelKind::Return,
                    "Return channels cannot send to another Return"
                );
                ensure!(
                    amount.is_finite() && (0.0..=1.0).contains(amount),
                    "Send amount must be 0..1"
                );
                match target {
                    ChannelChoice::Existing(ret_id) => {
                        ensure!(
                            project
                                .channel(*ret_id)
                                .is_some_and(|c| c.kind == ChannelKind::Return),
                            "Send target must be an existing Return channel"
                        );
                        let already = src.sends.iter().any(|s| s.target == *ret_id);
                        ensure!(
                            already || src.sends.len() < MAX_SENDS,
                            "Channel already has {MAX_SENDS} sends"
                        );
                    }
                    ChannelChoice::New { name } => {
                        validate_new_name(
                            name,
                            project.mixer.channels.iter().map(|c| c.name.as_str()),
                            80,
                        )?;
                        let returns = project
                            .mixer
                            .channels
                            .iter()
                            .filter(|c| c.kind == ChannelKind::Return)
                            .count();
                        ensure!(
                            returns < MAX_RETURNS,
                            "Return-channel limit reached ({MAX_RETURNS})"
                        );
                        ensure!(
                            src.sends.len() < MAX_SENDS,
                            "Channel already has {MAX_SENDS} sends"
                        );
                    }
                }
            }
            Self::Duck {
                id,
                name,
                source_part,
                target,
                depth,
                recovery_ms,
            } => {
                if let Some(edit_id) = *id {
                    ensure!(
                        project.ducks.iter().any(|d| d.id == edit_id),
                        "Duck no longer exists"
                    );
                } else {
                    ensure!(
                        project.ducks.len() < MAX_DUCKS,
                        "Duck limit reached ({MAX_DUCKS})"
                    );
                }
                validate_new_name(
                    name,
                    project
                        .ducks
                        .iter()
                        .filter(|d| Some(d.id) != *id)
                        .map(|d| d.name.as_str()),
                    80,
                )?;
                ensure!(
                    project.part(*source_part).is_some(),
                    "Sidechain source part no longer exists"
                );
                ensure!(
                    channel_insert_count(project, *target).is_some(),
                    "Duck target channel no longer exists"
                );
                ensure!(
                    depth.is_finite() && (0.0..=1.0).contains(depth),
                    "Duck depth must be 0..1"
                );
                ensure!(
                    recovery_ms.is_finite()
                        && (cadence_core::mixer::DUCK_RECOVERY_MIN_MS
                            ..=cadence_core::mixer::DUCK_RECOVERY_MAX_MS)
                            .contains(recovery_ms),
                    "Duck recovery must be {}..{} ms",
                    cadence_core::mixer::DUCK_RECOVERY_MIN_MS,
                    cadence_core::mixer::DUCK_RECOVERY_MAX_MS
                );
            }
            Self::Modulation {
                id,
                name,
                pattern,
                target,
            } => {
                if let Some(edit_id) = *id {
                    ensure!(
                        project.modulations.iter().any(|m| m.id == edit_id),
                        "Modulation no longer exists"
                    );
                } else {
                    ensure!(
                        project.modulations.len() < MAX_GLOBAL_MODULATIONS,
                        "Modulation limit reached ({MAX_GLOBAL_MODULATIONS})"
                    );
                }
                validate_new_name(
                    name,
                    project
                        .modulations
                        .iter()
                        .filter(|m| Some(m.id) != *id)
                        .map(|m| m.name.as_str()),
                    120,
                )?;
                match pattern {
                    ModPatternChoice::Existing(pat_id) => ensure!(
                        project.pattern_type(*pat_id).ok()
                            == Some(PatternType::Values(ValueUnit::Normalized)),
                        "Modulation source must be a normalized Values pattern"
                    ),
                    ModPatternChoice::NewValues { name: pat_name } => {
                        validate_new_name(
                            pat_name,
                            project.patterns.iter().map(|p| p.name.as_str()),
                            120,
                        )?;
                        counts[0] += 1;
                    }
                }
                match *target {
                    ModulationTarget::Instrument {
                        instrument,
                        parameter,
                    } => {
                        let inst = project
                            .instrument(instrument)
                            .context("Target instrument no longer exists")?;
                        inst.validate_parameter(parameter)?;
                    }
                    ModulationTarget::Effect { device, parameter } => {
                        let (_, dev) = project
                            .device(device)
                            .context("Target effect no longer exists")?;
                        ensure!(
                            dev.supports_param(parameter),
                            "Effect does not support that parameter"
                        );
                    }
                    ModulationTarget::ChannelGain { channel }
                    | ModulationTarget::ChannelPan { channel } => {
                        ensure!(
                            channel_insert_count(project, channel).is_some(),
                            "Target channel no longer exists"
                        );
                    }
                    ModulationTarget::Send { channel, target } => {
                        let ChannelRef::Channel(src_id) = channel else {
                            anyhow::bail!("Master cannot send to a Return channel");
                        };
                        ensure!(
                            project
                                .channel(src_id)
                                .is_some_and(|c| c.kind != ChannelKind::Return),
                            "Send modulation source must be an Audio or Group channel"
                        );
                        ensure!(
                            project
                                .channel(target)
                                .is_some_and(|c| c.kind == ChannelKind::Return),
                            "Send modulation destination must be a Return channel"
                        );
                    }
                    ModulationTarget::Macro { index } => {
                        ensure!(
                            usize::from(index) < MAX_PROJECT_MACROS,
                            "Macro slot must be 1..={MAX_PROJECT_MACROS}"
                        );
                    }
                }
            }
            Self::ZoneImportFolder { instrument, folder } => {
                ensure!(
                    project.instrument(*instrument).is_some(),
                    "Target Rack instrument no longer exists"
                );
                ensure!(
                    !folder.trim().is_empty(),
                    "Sample folder path cannot be empty"
                );
            }
            Self::SavePreset {
                instrument, name, ..
            } => {
                ensure!(
                    project.instrument(*instrument).is_some(),
                    "Instrument no longer exists"
                );
                validate_printable_name("Preset", name, 80)?;
            }
            Self::Bounce {
                scope,
                name,
                target,
                ..
            } => {
                if let Some(n) = name {
                    validate_printable_name("Bounce", n, 80)?;
                }
                match *scope {
                    BounceScope::Part(id) => {
                        ensure!(project.part(id).is_some(), "Bounce source part missing");
                    }
                    BounceScope::Pattern(id) => {
                        ensure!(
                            project.pattern_type(id).ok() == Some(PatternType::Notes),
                            "Bounce source pattern must produce notes"
                        );
                    }
                    BounceScope::Range { start_bar, end_bar } => {
                        let max_bars = project.bars.max(1);
                        ensure!(
                            start_bar >= 1 && start_bar <= end_bar && end_bar <= max_bars,
                            "Bar range must satisfy 1 <= start <= end <= {max_bars}"
                        );
                    }
                }
                if let BounceTarget::RackZone { instrument, .. } = target {
                    ensure!(
                        project.instrument(*instrument).is_some(),
                        "Bounce target Rack instrument missing"
                    );
                }
            }
            Self::Record {
                bars, name, target, ..
            } => {
                if let Some(n) = name {
                    validate_printable_name("Recording", n, 80)?;
                }
                if let Some(b) = *bars {
                    ensure!(
                        (1..=64).contains(&b),
                        "Recording length must be 1..=64 bars"
                    );
                }
                if let BounceTarget::RackZone { instrument, .. } = target {
                    ensure!(
                        project.instrument(*instrument).is_some(),
                        "Recording target Rack instrument missing"
                    );
                }
            }
        }
        ensure!(
            project.patterns.len() + counts[0] <= MAX_PATTERNS,
            "Pattern limit reached"
        );
        ensure!(
            project.instruments.len() + counts[1] <= MAX_INSTRUMENTS,
            "Instrument limit reached"
        );
        ensure!(
            project
                .mixer
                .channels
                .iter()
                .filter(|c| c.kind == ChannelKind::Audio)
                .count()
                + counts[2]
                <= MAX_AUDIO_CHANNELS,
            "Audio-channel limit reached"
        );
        ensure!(
            project.parts.len() + counts[3] <= MAX_PARTS,
            "Part limit reached"
        );
        ensure!(
            project.lanes.len() + counts[4] <= MAX_LANES,
            "Lane limit reached"
        );
        Ok(())
    }

    pub fn new_object_count(&self) -> usize {
        fn inst(i: &InstrumentSpec) -> usize {
            1 + usize::from(matches!(i.channel, ChannelChoice::New { .. }))
        }
        fn part(b: &PartSpec) -> usize {
            1 + usize::from(matches!(b.pattern, PatternChoice::NewNotes { .. }))
                + match &b.instrument {
                    InstrumentChoice::Existing(_) => 0,
                    InstrumentChoice::New(i) => inst(i),
                }
        }
        match self {
            Self::Instrument(i) => inst(i),
            Self::Part(b) | Self::BankPart { spec: b, .. } => part(b),
            Self::Lane { .. } | Self::Channel { .. } => 1,
            Self::RenameInstrument { .. }
            | Self::RenamePart { .. }
            | Self::RenameLane { .. }
            | Self::EditLanePart { .. }
            | Self::FreezePattern { .. }
            | Self::RenameChannel { .. }
            | Self::RenameEffect { .. }
            | Self::RenameMacro { .. }
            | Self::Effect { .. }
            | Self::MoveEffect { .. }
            | Self::ZoneImportFolder { .. }
            | Self::SavePreset { .. }
            | Self::Bounce { .. }
            | Self::Record { .. } => 0,
            Self::Section { id, .. } | Self::Duck { id, .. } => usize::from(id.is_none()),
            Self::Modulation { id, pattern, .. } => {
                usize::from(id.is_none())
                    + usize::from(matches!(pattern, ModPatternChoice::NewValues { .. }))
            }
            Self::Send { target, .. } => usize::from(matches!(target, ChannelChoice::New { .. })),
            Self::Place(p) | Self::PlacedPart(p) => {
                1 + usize::from(matches!(p.lane, LaneChoice::New { .. }))
                    + match &p.part {
                        PartChoice::Existing(_) => 0,
                        PartChoice::New(b) => part(b),
                    }
            }
        }
    }
}

fn validate_instrument(
    project: &Project,
    instrument: &InstrumentSpec,
    counts: &mut [usize; 5],
) -> Result<()> {
    validate_new_name(
        &instrument.name,
        project.instruments.iter().map(|i| i.name.as_str()),
        120,
    )?;
    counts[1] += 1;
    match &instrument.sound {
        SoundChoice::Sampler { path: Some(p), .. } => {
            ensure!(!p.trim().is_empty(), "Sample path cannot be empty");
        }
        SoundChoice::Folder { path, .. } => {
            ensure!(
                !path.trim().is_empty(),
                "Sample folder path cannot be empty"
            );
        }
        _ => {}
    }
    match &instrument.channel {
        ChannelChoice::Existing(id) => ensure!(
            project
                .channel(*id)
                .is_some_and(|c| c.kind == ChannelKind::Audio),
            "Destination must be an existing Audio channel"
        ),
        ChannelChoice::New { name } => {
            validate_new_name(
                name,
                project.mixer.channels.iter().map(|c| c.name.as_str()),
                80,
            )?;
            counts[2] += 1;
        }
    }
    Ok(())
}

fn validate_part(project: &Project, part: &PartSpec, counts: &mut [usize; 5]) -> Result<()> {
    validate_new_name(
        &part.name,
        project.parts.iter().map(|b| b.name.as_str()),
        120,
    )?;
    counts[3] += 1;
    match &part.pattern {
        PatternChoice::Existing(id) => ensure!(
            project.pattern_type(*id).ok() == Some(PatternType::Notes),
            "Part needs existing note-producing material"
        ),
        PatternChoice::NewNotes { name, length } => {
            validate_new_name(name, project.patterns.iter().map(|p| p.name.as_str()), 120)?;
            ensure!(
                (1..=MAX_LITERAL_STEPS).contains(length),
                "Notes length must be 1..{MAX_LITERAL_STEPS} steps"
            );
            counts[0] += 1;
        }
    }
    match &part.instrument {
        InstrumentChoice::Existing(id) => {
            project
                .instrument(*id)
                .context("Instrument no longer exists")?;
        }
        InstrumentChoice::New(instrument) => validate_instrument(project, instrument, counts)?,
    }
    ensure!(
        (-96..=96).contains(&part.transpose)
            && part.velocity_scale.is_finite()
            && (0.0..=1.0).contains(&part.velocity_scale),
        "Part transpose/velocity out of range"
    );
    Ok(())
}
