//! Cancellable drafts for sound, part, arrangement, section, and mixer workflows.
//!
//! Existing objects are referenced by ID. New objects remain semantic specs until
//! the application accepts the whole plan as one validated transaction. This layer
//! allocates no authored IDs and never mutates the Project.
use super::form::{
    format_duration, format_start, is_name, is_picker, unique_name, DraftCandidate, DraftField,
};
pub use super::form::{parse_duration, parse_position};
pub use super::plan::{
    ChannelChoice, InstrumentChoice, InstrumentSpec, LaneChoice, PartChoice, PartSpec,
    PatternChoice, PlacementSpec, SoundChoice, WorkflowContext, WorkflowKind, WorkflowOutcome,
    WorkflowPlan,
};
use crate::keys::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use cadence_core::macros::{MacroControl, MAX_PROJECT_MACROS};
use cadence_core::material::{Id, ModulationTarget, PatternType, PhaseMode, Project, ValueUnit};
use cadence_core::mixer::{ChannelKind, ChannelRef};
use cadence_core::presets::PresetCatalog;
use cadence_core::rack::RackTemplate;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModTargetNode {
    Instrument(Id),
    Effect(Id),
    Channel(ChannelRef),
    Macro(u8),
}

impl ModTargetNode {
    pub(super) fn from_target(target: ModulationTarget) -> Self {
        match target {
            ModulationTarget::Instrument { instrument, .. } => Self::Instrument(instrument),
            ModulationTarget::Effect { device, .. } => Self::Effect(device),
            ModulationTarget::ChannelGain { channel }
            | ModulationTarget::ChannelPan { channel }
            | ModulationTarget::Send { channel, .. } => Self::Channel(channel),
            ModulationTarget::Macro { index } => Self::Macro(index),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RefChoice {
    Existing(Id),
    New,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SamplerSourceChoice {
    SelectedSample,
    Blank,
    Browse,
    Preset(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum KitSourceChoice {
    Blank,
    Preset(String),
    Folder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BounceActionChoice {
    BounceNow,
    EditScope,
    EditRange,
    EditTarget,
    EditName,
    EditCreatePart,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RecordActionChoice {
    RecordNow,
    EditMode,
    EditBars,
    EditTarget,
    EditName,
    EditArrange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BounceScopeKind {
    Part,
    Pattern,
    Range,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BounceTargetKind {
    NewSampler,
    ReplaceZone,
    NewZoneMidi,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Choice {
    Reference(RefChoice),
    Template(RackTemplate),
    Preset(Option<String>),
    SamplerSource(SamplerSourceChoice),
    KitSource(KitSourceChoice),
    Phase(PhaseMode),
    ChannelKind(ChannelKind),
    ChannelRef(ChannelRef),
    EffectKind(&'static str),
    ExistingId(Id),
    ModTargetNode(ModTargetNode),
    ModTarget(ModulationTarget),
    PresetCategory(&'static str),
    BounceAction(BounceActionChoice),
    BounceScopeKind(BounceScopeKind),
    BounceTargetKind(BounceTargetKind),
    BounceZoneIndex(usize),
    RecordAction(RecordActionChoice),
    RecordMode(crate::recording::RecordingMode),
    Bool(bool),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PresetChoiceItem {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) category: Option<String>,
    pub(super) is_kit: bool,
}

pub(super) struct Candidate {
    pub(super) view: DraftCandidate,
    pub(super) choice: Choice,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowDraft {
    pub(super) kind: WorkflowKind,
    pub(super) field: DraftField,
    pub(super) context: WorkflowContext,
    pub(super) name: String,
    pub(super) pattern_name: String,
    pub(super) instrument_name: String,
    pub(super) channel_name: String,
    pub(super) part_name: String,
    pub(super) lane_name: String,
    pub(super) pattern_length: String,
    pub(super) part_transpose: String,
    pub(super) part_velocity: String,
    pub(super) start: String,
    pub(super) duration: String,
    pub(super) offset: String,
    pub(super) pattern: Option<RefChoice>,
    pub(super) instrument: Option<RefChoice>,
    pub(super) channel: Option<RefChoice>,
    pub(super) part: Option<RefChoice>,
    pub(super) lane: Option<RefChoice>,
    pub(super) rack_template: RackTemplate,
    pub(super) preset: Option<String>,
    pub(super) sampler_source: SamplerSourceChoice,
    pub(super) kit_source: KitSourceChoice,
    pub(super) rack_presets: Vec<PresetChoiceItem>,
    pub(super) sample_folder: String,
    pub(super) phase: PhaseMode,
    pub(super) sample: Option<(String, String)>,
    pub(super) channel_kind: ChannelKind,
    pub(super) channel_output: ChannelRef,
    pub(super) effect_kind: &'static str,
    pub(super) effect_channel: ChannelRef,
    pub(super) effect_slot: String,
    pub(super) send_source: Option<Id>,
    pub(super) send_target: Option<RefChoice>,
    pub(super) send_amount: String,
    pub(super) duck_source: Option<Id>,
    pub(super) duck_target: ChannelRef,
    pub(super) duck_depth: String,
    pub(super) duck_recovery: String,
    pub(super) mod_pattern: Option<RefChoice>,
    pub(super) mod_target_node: Option<ModTargetNode>,
    pub(super) mod_target: Option<ModulationTarget>,
    pub(super) preset_category: &'static str,
    pub(super) bounce_action: BounceActionChoice,
    pub(super) bounce_scope_kind: BounceScopeKind,
    pub(super) bounce_part: Option<Id>,
    pub(super) bounce_pattern: Option<Id>,
    pub(super) bounce_range: String,
    pub(super) bounce_target_kind: BounceTargetKind,
    pub(super) bounce_zone_instrument: Option<Id>,
    pub(super) bounce_zone_index: usize,
    pub(super) bounce_zone_midi: String,
    pub(super) bounce_create_part: bool,
    pub(super) record_action: RecordActionChoice,
    pub(super) record_mode: crate::recording::RecordingMode,
    pub(super) record_bars: String,
    pub(super) query: String,
    pub(super) searching: bool,
    pub(super) replace_on_type: bool,
    pub(super) name_edited: bool,
    pub(super) duration_edited: bool,
    pub(super) slot_edited: bool,
    pub(super) error: Option<String>,
}

impl WorkflowDraft {
    pub fn new(
        kind: WorkflowKind,
        project: &Project,
        context: WorkflowContext,
        presets: &PresetCatalog,
    ) -> Self {
        let horizon = project.arrangement_steps().max(1) as u32;
        let start = context.arrange_step.min(horizon - 1);
        let default_mixer_ch = context
            .mixer_channel
            .filter(|ch| match ch {
                ChannelRef::Master => true,
                ChannelRef::Channel(id) => project.channel(*id).is_some(),
            })
            .or_else(|| context.channel.map(ChannelRef::Channel))
            .unwrap_or(ChannelRef::Master);

        let name = match kind {
            WorkflowKind::Instrument => unique_name(
                "Instrument",
                project.instruments.iter().map(|i| i.name.as_str()),
                120,
            ),
            WorkflowKind::Part => {
                unique_name("Part", project.parts.iter().map(|b| b.name.as_str()), 120)
            }
            WorkflowKind::RenamePart(id) => project
                .part(id)
                .map_or_else(|| "Part".into(), |p| p.name.clone()),
            WorkflowKind::Lane => {
                unique_name("Lane", project.lanes.iter().map(|l| l.name.as_str()), 120)
            }
            WorkflowKind::RenameLane(id) => project
                .lane(id)
                .map_or_else(|| "Lane".into(), |l| l.name.clone()),
            WorkflowKind::EditLanePart { .. } => String::new(),
            WorkflowKind::FreezePattern(id) => {
                let base = project.pattern(id).map_or("Pattern", |p| p.name.as_str());
                unique_name(
                    &format!("{base} frozen"),
                    project.patterns.iter().map(|p| p.name.as_str()),
                    120,
                )
            }
            WorkflowKind::Place | WorkflowKind::PlacedPart => {
                unique_name("Part", project.parts.iter().map(|b| b.name.as_str()), 120)
            }
            WorkflowKind::Section => unique_name(
                "Section",
                project.sections.iter().map(|s| s.name.as_str()),
                120,
            ),
            WorkflowKind::EditSection(id) => project
                .sections
                .iter()
                .find(|s| s.id == id)
                .map_or_else(|| "Section".into(), |s| s.name.clone()),
            WorkflowKind::Channel => unique_name(
                "Audio",
                project.mixer.channels.iter().map(|c| c.name.as_str()),
                80,
            ),
            WorkflowKind::RenameInstrument(id) => project
                .instrument(id)
                .map_or_else(|| "Instrument".into(), |i| i.name.clone()),
            WorkflowKind::RenameChannel(id) => project
                .channel(id)
                .map_or_else(|| "Channel".into(), |c| c.name.clone()),
            WorkflowKind::RenameEffect(id) => project
                .device(id)
                .map_or_else(|| "Effect".into(), |(_, d)| d.name.clone()),
            WorkflowKind::RenameMacro(index) => project
                .macro_control(index)
                .map_or_else(|| MacroControl::default_name(index), |m| m.name.clone()),
            WorkflowKind::Effect | WorkflowKind::MoveEffect(_) | WorkflowKind::Send => {
                String::new()
            }
            WorkflowKind::Duck => {
                unique_name("Duck", project.ducks.iter().map(|d| d.name.as_str()), 80)
            }
            WorkflowKind::EditDuck(id) => project
                .ducks
                .iter()
                .find(|d| d.id == id)
                .map_or_else(|| "Duck".into(), |d| d.name.clone()),
            WorkflowKind::Modulation => unique_name(
                "Mod",
                project.modulations.iter().map(|m| m.name.as_str()),
                120,
            ),
            WorkflowKind::EditModulation(id) => project
                .modulations
                .iter()
                .find(|m| m.id == id)
                .map_or_else(|| "Mod".into(), |m| m.name.clone()),
            WorkflowKind::ZoneImportFolder(_) | WorkflowKind::Bounce | WorkflowKind::Record => {
                String::new()
            }
            WorkflowKind::SavePreset(id) => project
                .instrument(id)
                .map_or_else(|| "Preset".into(), |i| i.name.clone()),
        };

        let (init_start, init_duration) = match kind {
            WorkflowKind::EditSection(id) => project
                .sections
                .iter()
                .find(|s| s.id == id)
                .map_or((start, 16.min(horizon - start)), |s| {
                    (s.start_step, s.length_steps)
                }),
            WorkflowKind::Section => {
                let remaining = horizon - start;
                (start, 64.min(remaining).max(16.min(remaining)).max(1))
            }
            WorkflowKind::PlacedPart => (start, horizon - start),
            _ => (start, 16.min(horizon - start)),
        };

        let (init_fx_ch, init_fx_slot) = match kind {
            WorkflowKind::MoveEffect(dev_id) => {
                if let Some((owner, _)) = project.device(dev_id) {
                    let inserts = match owner {
                        ChannelRef::Master => &project.mixer.master.inserts,
                        ChannelRef::Channel(id) => project
                            .channel(id)
                            .map_or(&[][..], |c| c.inserts.as_slice()),
                    };
                    let slot = inserts
                        .iter()
                        .position(|d| d.id == dev_id)
                        .map_or(1, |i| i + 1);
                    (owner, slot.to_string())
                } else {
                    (default_mixer_ch, "1".into())
                }
            }
            _ => (default_mixer_ch, "1".into()),
        };

        let init_send_source = match default_mixer_ch {
            ChannelRef::Channel(id)
                if project
                    .channel(id)
                    .is_some_and(|c| c.kind != ChannelKind::Return) =>
            {
                Some(id)
            }
            _ => project
                .mixer
                .channels
                .iter()
                .find(|c| c.kind != ChannelKind::Return)
                .map(|c| c.id),
        };
        let init_send_target = context
            .send_target
            .filter(|&id| {
                project
                    .channel(id)
                    .is_some_and(|c| c.kind == ChannelKind::Return)
            })
            .map(RefChoice::Existing)
            .or_else(|| {
                project
                    .mixer
                    .channels
                    .iter()
                    .find(|c| c.kind == ChannelKind::Return)
                    .map(|c| RefChoice::Existing(c.id))
            })
            .or(Some(RefChoice::New));
        let init_send_amount = match (init_send_source, init_send_target) {
            (Some(src), Some(RefChoice::Existing(ret))) => project
                .channel(src)
                .and_then(|c| c.sends.iter().find(|s| s.target == ret))
                .map_or_else(|| "30%".into(), |s| format!("{:.0}%", s.amount * 100.0)),
            _ => "30%".into(),
        };

        let (init_mod_pat, init_mod_node, init_mod_target) = match kind {
            WorkflowKind::EditModulation(id) => project
                .modulations
                .iter()
                .find(|m| m.id == id)
                .map_or((None, None, None), |m| {
                    (
                        Some(RefChoice::Existing(m.pattern)),
                        Some(ModTargetNode::from_target(m.target)),
                        Some(m.target),
                    )
                }),
            _ => {
                let pat = context
                    .pattern
                    .filter(|&id| {
                        project.pattern_type(id).ok()
                            == Some(PatternType::Values(ValueUnit::Normalized))
                    })
                    .or_else(|| {
                        project
                            .patterns
                            .iter()
                            .find(|p| {
                                project.pattern_type(p.id).ok()
                                    == Some(PatternType::Values(ValueUnit::Normalized))
                            })
                            .map(|p| p.id)
                    })
                    .map(RefChoice::Existing)
                    .or(Some(RefChoice::New));
                let (node, target) = if let Some(t) = context.mod_target {
                    (Some(ModTargetNode::from_target(t)), Some(t))
                } else {
                    let node = context
                        .effect
                        .filter(|&id| project.device(id).is_some())
                        .map(ModTargetNode::Effect)
                        .or_else(|| {
                            context
                                .macro_slot
                                .filter(|&idx| usize::from(idx) < MAX_PROJECT_MACROS)
                                .map(ModTargetNode::Macro)
                        })
                        .or_else(|| {
                            context
                                .mixer_channel
                                .filter(|ch| match ch {
                                    ChannelRef::Master => true,
                                    ChannelRef::Channel(id) => project.channel(*id).is_some(),
                                })
                                .map(ModTargetNode::Channel)
                        })
                        .or_else(|| {
                            context
                                .instrument
                                .filter(|&id| project.instrument(id).is_some())
                                .map(ModTargetNode::Instrument)
                        })
                        .or_else(|| {
                            project
                                .instruments
                                .first()
                                .map(|i| ModTargetNode::Instrument(i.id))
                        })
                        .or(Some(ModTargetNode::Channel(ChannelRef::Master)));
                    let target = match node {
                        Some(ModTargetNode::Macro(index)) => {
                            Some(ModulationTarget::Macro { index })
                        }
                        _ => None,
                    };
                    (node, target)
                };
                (pat, node, target)
            }
        };

        let (init_duck_src, init_duck_target, init_duck_depth, init_duck_rec) = match kind {
            WorkflowKind::EditDuck(id) => project.ducks.iter().find(|d| d.id == id).map_or_else(
                || {
                    (
                        context.part.or_else(|| project.parts.first().map(|p| p.id)),
                        default_mixer_ch,
                        "50%".into(),
                        "160ms".into(),
                    )
                },
                |d| {
                    (
                        Some(d.source_part),
                        d.target,
                        format!("{:.0}%", d.depth * 100.0),
                        format!("{:.0}ms", d.recovery_ms),
                    )
                },
            ),
            _ => (
                context
                    .part
                    .filter(|&id| project.part(id).is_some())
                    .or_else(|| project.parts.first().map(|p| p.id)),
                default_mixer_ch,
                "50%".into(),
                "160ms".into(),
            ),
        };

        let initial_field = match kind {
            WorkflowKind::Place => DraftField::Part,
            WorkflowKind::EditLanePart { .. } => DraftField::Phase,
            WorkflowKind::Channel => DraftField::ChannelKind,
            WorkflowKind::Effect => DraftField::EffectKind,
            WorkflowKind::MoveEffect(_) => DraftField::EffectChannel,
            WorkflowKind::Send => DraftField::SendSource,
            WorkflowKind::Modulation | WorkflowKind::EditModulation(_) => DraftField::ModPattern,
            WorkflowKind::ZoneImportFolder(_) => DraftField::SampleFolder,
            WorkflowKind::Bounce => DraftField::BounceAction,
            WorkflowKind::Record => DraftField::RecordAction,
            _ => DraftField::Name,
        };
        let init_preset_category = match kind {
            WorkflowKind::SavePreset(id) => project
                .instrument(id)
                .map(|i| crate::App::default_preset_category(&i.rack))
                .unwrap_or("Synth"),
            _ => "Synth",
        };
        let default_bounce_part = context
            .part
            .filter(|&id| project.part(id).is_some())
            .or_else(|| project.parts.first().map(|p| p.id));
        let default_bounce_pattern = context
            .pattern
            .filter(|&id| project.pattern_type(id).ok() == Some(PatternType::Notes))
            .or_else(|| {
                project
                    .patterns
                    .iter()
                    .find(|p| project.pattern_type(p.id).ok() == Some(PatternType::Notes))
                    .map(|p| p.id)
            });
        let default_bounce_inst = context
            .instrument
            .filter(|&id| project.instrument(id).is_some())
            .or_else(|| project.instruments.first().map(|i| i.id));

        let (init_phase, init_offset) = match kind {
            WorkflowKind::EditLanePart { lane, part } => project
                .lane(lane)
                .and_then(|l| l.parts.iter().find(|e| e.part == part))
                .map_or((PhaseMode::Song, "0".into()), |e| {
                    (e.phase, e.offset_steps.to_string())
                }),
            _ => (PhaseMode::Song, "0".into()),
        };

        let init_pattern_length = match kind {
            WorkflowKind::FreezePattern(id) => project
                .pattern_period(id)
                .unwrap_or(16)
                .clamp(1, cadence_core::material::MAX_LITERAL_STEPS as u64)
                .to_string(),
            _ => "16".into(),
        };

        let init_start_str = match kind {
            WorkflowKind::FreezePattern(_) => format_start(0),
            _ => format_start(init_start),
        };

        let mut draft = Self {
            kind,
            field: initial_field,
            context,
            name,
            pattern_name: String::new(),
            instrument_name: String::new(),
            channel_name: String::new(),
            part_name: String::new(),
            lane_name: String::new(),
            pattern_length: init_pattern_length,
            part_transpose: "0".into(),
            part_velocity: "100%".into(),
            start: init_start_str,
            duration: format_duration(init_duration),
            offset: init_offset,
            pattern: context
                .pattern
                .filter(|&id| project.pattern_type(id).ok() == Some(PatternType::Notes))
                .map(RefChoice::Existing),
            instrument: if kind == WorkflowKind::PlacedPart {
                Some(RefChoice::New)
            } else {
                context
                    .instrument
                    .filter(|&id| project.instrument(id).is_some())
                    .map(RefChoice::Existing)
            },
            channel: context
                .channel
                .filter(|&id| {
                    project
                        .channel(id)
                        .is_some_and(|c| c.kind == ChannelKind::Audio)
                })
                .map(RefChoice::Existing)
                .or(Some(RefChoice::New)),
            part: context
                .part
                .filter(|&id| project.part(id).is_some())
                .map(RefChoice::Existing),
            lane: if kind == WorkflowKind::PlacedPart {
                Some(RefChoice::New)
            } else {
                context
                    .lane
                    .filter(|&id| project.lane(id).is_some())
                    .map(RefChoice::Existing)
            },
            rack_template: RackTemplate::Synth,
            preset: None,
            sampler_source: SamplerSourceChoice::Blank,
            kit_source: KitSourceChoice::Blank,
            rack_presets: presets
                .entries()
                .iter()
                .map(|e| PresetChoiceItem {
                    id: e.id.clone(),
                    name: e.name.clone(),
                    category: e.category.clone(),
                    is_kit: e.is_kit(),
                })
                .collect(),
            sample_folder: String::new(),
            phase: init_phase,
            sample: None,
            channel_kind: ChannelKind::Audio,
            channel_output: ChannelRef::Master,
            effect_kind: "eq",
            effect_channel: init_fx_ch,
            effect_slot: init_fx_slot,
            send_source: init_send_source,
            send_target: init_send_target,
            send_amount: init_send_amount,
            duck_source: init_duck_src,
            duck_target: init_duck_target,
            duck_depth: init_duck_depth,
            duck_recovery: init_duck_rec,
            mod_pattern: init_mod_pat,
            mod_target_node: init_mod_node,
            mod_target: init_mod_target,
            preset_category: init_preset_category,
            bounce_action: BounceActionChoice::BounceNow,
            bounce_scope_kind: if default_bounce_part.is_some() {
                BounceScopeKind::Part
            } else if default_bounce_pattern.is_some() {
                BounceScopeKind::Pattern
            } else {
                BounceScopeKind::Range
            },
            bounce_part: default_bounce_part,
            bounce_pattern: default_bounce_pattern,
            bounce_range: format!("1..{}", project.bars.max(1)),
            bounce_target_kind: BounceTargetKind::NewSampler,
            bounce_zone_instrument: default_bounce_inst,
            bounce_zone_index: 0,
            bounce_zone_midi: "C4".into(),
            bounce_create_part: false,
            record_action: RecordActionChoice::RecordNow,
            record_mode: crate::recording::RecordingMode::Quantized,
            record_bars: String::new(),
            query: String::new(),
            searching: false,
            replace_on_type: true,
            name_edited: matches!(
                kind,
                WorkflowKind::RenameInstrument(_)
                    | WorkflowKind::RenamePart(_)
                    | WorkflowKind::RenameLane(_)
                    | WorkflowKind::RenameChannel(_)
                    | WorkflowKind::RenameEffect(_)
                    | WorkflowKind::RenameMacro(_)
                    | WorkflowKind::EditSection(_)
                    | WorkflowKind::EditDuck(_)
                    | WorkflowKind::EditModulation(_)
                    | WorkflowKind::FreezePattern(_)
            ),
            duration_edited: matches!(kind, WorkflowKind::EditSection(_)),
            slot_edited: false,
            error: None,
        };
        // A complete empty-workspace journey must not require leaving a picker.
        draft.ensure_reference(project, DraftField::Pattern);
        draft.ensure_reference(project, DraftField::Instrument);
        draft.ensure_reference(project, DraftField::Part);
        draft.ensure_reference(project, DraftField::Lane);
        draft.ensure_reference(project, DraftField::ChannelOutput);
        draft.ensure_reference(project, DraftField::EffectChannel);
        draft.ensure_reference(project, DraftField::SendSource);
        draft.ensure_reference(project, DraftField::SendTarget);
        draft.ensure_reference(project, DraftField::DuckSource);
        draft.ensure_reference(project, DraftField::DuckTarget);
        draft.ensure_reference(project, DraftField::ModPattern);
        draft.ensure_reference(project, DraftField::ModTarget);
        draft.ensure_reference(project, DraftField::ModParam);
        if kind == WorkflowKind::Modulation && !draft.name_edited {
            draft.name = draft.default_modulation_name(project);
        }
        draft.enter(project, draft.field);
        draft
    }

    pub fn instrument_with_sample(
        project: &Project,
        context: WorkflowContext,
        path: String,
        label: String,
        presets: &PresetCatalog,
    ) -> Self {
        let mut draft = Self::new(WorkflowKind::Instrument, project, context, presets);
        draft.rack_template = RackTemplate::Sampler;
        draft.sampler_source = SamplerSourceChoice::SelectedSample;
        draft.name = unique_name(
            &label,
            project.instruments.iter().map(|i| i.name.as_str()),
            120,
        );
        draft.sample = Some((path, label));
        draft.replace_on_type = true;
        draft
    }

    pub fn bounce(
        project: &Project,
        context: WorkflowContext,
        scope: crate::bounce::BounceScope,
        target: crate::bounce::BounceTarget,
        presets: &PresetCatalog,
    ) -> Self {
        let mut draft = Self::new(WorkflowKind::Bounce, project, context, presets);
        match scope {
            crate::bounce::BounceScope::Part(id) => {
                draft.bounce_scope_kind = BounceScopeKind::Part;
                draft.bounce_part = Some(id);
            }
            crate::bounce::BounceScope::Pattern(id) => {
                draft.bounce_scope_kind = BounceScopeKind::Pattern;
                draft.bounce_pattern = Some(id);
            }
            crate::bounce::BounceScope::Range { start_bar, end_bar } => {
                draft.bounce_scope_kind = BounceScopeKind::Range;
                draft.bounce_range = format!("{start_bar}..{end_bar}");
            }
        }
        match target {
            crate::bounce::BounceTarget::NewSampler => {
                draft.bounce_target_kind = BounceTargetKind::NewSampler;
            }
            crate::bounce::BounceTarget::RackZone {
                instrument,
                zone_index,
                midi,
                zone_name,
            } => {
                draft.bounce_zone_instrument = Some(instrument);
                if let Some(zi) = zone_index {
                    draft.bounce_target_kind = BounceTargetKind::ReplaceZone;
                    draft.bounce_zone_index = zi;
                } else {
                    draft.bounce_target_kind = BounceTargetKind::NewZoneMidi;
                    if let Some(m) = midi {
                        draft.bounce_zone_midi = cadence_core::music::note_name(m);
                    }
                }
                if let Some(zn) = zone_name {
                    draft.name = zn;
                }
            }
        }
        draft.bounce_action = BounceActionChoice::BounceNow;
        draft.enter(project, DraftField::BounceAction);
        draft
    }

    pub fn record(
        project: &Project,
        context: WorkflowContext,
        mode: crate::recording::RecordingMode,
        bars: Option<usize>,
        target: crate::bounce::BounceTarget,
        presets: &PresetCatalog,
    ) -> Self {
        let mut draft = Self::new(WorkflowKind::Record, project, context, presets);
        draft.record_mode = mode;
        draft.record_bars = bars.map_or_else(String::new, |b| b.to_string());
        draft.bounce_create_part = matches!(target, crate::bounce::BounceTarget::NewSampler)
            && mode != crate::recording::RecordingMode::Free;
        match target {
            crate::bounce::BounceTarget::NewSampler => {
                draft.bounce_target_kind = BounceTargetKind::NewSampler;
            }
            crate::bounce::BounceTarget::RackZone {
                instrument,
                zone_index,
                midi,
                zone_name,
            } => {
                draft.bounce_zone_instrument = Some(instrument);
                if let Some(zi) = zone_index {
                    draft.bounce_target_kind = BounceTargetKind::ReplaceZone;
                    draft.bounce_zone_index = zi;
                } else {
                    draft.bounce_target_kind = BounceTargetKind::NewZoneMidi;
                    if let Some(m) = midi {
                        draft.bounce_zone_midi = cadence_core::music::note_name(m);
                    }
                }
                if let Some(zn) = zone_name {
                    draft.name = zn;
                }
            }
        }
        draft.record_action = RecordActionChoice::RecordNow;
        draft.enter(project, DraftField::RecordAction);
        draft
    }

    pub fn kind(&self) -> WorkflowKind {
        self.kind
    }

    pub fn active_field(&self) -> DraftField {
        self.field
    }

    pub fn set_sample_folder(&mut self, folder: String) {
        self.sample_folder = folder;
        self.replace_on_type = false;
        self.error = None;
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn selected_sample(&self) -> Option<(&str, &str)> {
        self.sample
            .as_ref()
            .map(|(path, label)| (path.as_str(), label.as_str()))
    }

    pub fn sample_target_label(&self, project: &Project) -> String {
        format!("{} · staged Sampler", self.instrument_name(project))
    }

    pub fn set_sample(
        &mut self,
        project: &Project,
        path: String,
        label: String,
    ) -> WorkflowOutcome {
        if path.trim().is_empty() {
            self.error = Some("Choose a sample file".into());
            return WorkflowOutcome::Continue;
        }
        self.sample = Some((path, label.clone()));
        self.rack_template = RackTemplate::Sampler;
        self.sampler_source = SamplerSourceChoice::SelectedSample;
        self.error = None;
        if self.kind == WorkflowKind::Instrument && !self.name_edited {
            self.name = unique_name(
                &label,
                project.instruments.iter().map(|i| i.name.as_str()),
                120,
            );
        }
        if self.field == DraftField::Sample {
            self.advance(project)
        } else {
            WorkflowOutcome::Continue
        }
    }

    pub fn handle_key(&mut self, project: &Project, key: KeyEvent) -> WorkflowOutcome {
        if key.kind == KeyEventKind::Release {
            return WorkflowOutcome::Continue;
        }
        if key.code == KeyCode::Esc {
            if self.searching || (is_picker(self.field) && !self.query.is_empty()) {
                self.searching = false;
                self.query.clear();
                self.error = None;
                self.ensure_reference(project, self.field);
                return WorkflowOutcome::Continue;
            }
            return WorkflowOutcome::Cancel;
        }
        if key.kind == KeyEventKind::Repeat && matches!(key.code, KeyCode::Enter | KeyCode::BackTab)
        {
            return WorkflowOutcome::Continue;
        }
        if key.code == KeyCode::BackTab {
            self.previous(project);
            return WorkflowOutcome::Continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => return WorkflowOutcome::Cancel,
                KeyCode::Char('f') if self.field == DraftField::SampleFolder => {
                    return WorkflowOutcome::BrowseFolder;
                }
                KeyCode::Char('u') => {
                    if is_picker(self.field) {
                        self.query.clear();
                        self.searching = true;
                        self.ensure_reference(project, self.field);
                    } else if let Some(text) = self.text_mut() {
                        text.clear();
                        self.replace_on_type = false;
                    }
                    self.error = None;
                }
                _ => {}
            }
            return WorkflowOutcome::Continue;
        }
        if key.modifiers.contains(KeyModifiers::ALT) {
            return WorkflowOutcome::Continue;
        }
        if is_picker(self.field) {
            if self.searching {
                match key.code {
                    KeyCode::Enter | KeyCode::Tab => {
                        self.searching = false;
                        return WorkflowOutcome::Continue;
                    }
                    KeyCode::Char(c) if !c.is_control() && self.query.chars().count() < 120 => {
                        self.query.push(c)
                    }
                    KeyCode::Backspace => {
                        self.query.pop();
                    }
                    KeyCode::Up => {
                        self.move_choice(project, -1);
                        return WorkflowOutcome::Continue;
                    }
                    KeyCode::Down => {
                        self.move_choice(project, 1);
                        return WorkflowOutcome::Continue;
                    }
                    _ => return WorkflowOutcome::Continue,
                }
                self.ensure_reference(project, self.field);
                self.error = None;
                return WorkflowOutcome::Continue;
            }
            match key.code {
                KeyCode::Char('/') => {
                    self.query.clear();
                    self.searching = true;
                    self.error = None;
                }
                KeyCode::Char('j') | KeyCode::Down => self.move_choice(project, 1),
                KeyCode::Char('k') | KeyCode::Up => self.move_choice(project, -1),
                KeyCode::Char('A') if self.field == DraftField::Preset => {
                    return WorkflowOutcome::AuditionPreset(self.preset.clone())
                }
                KeyCode::Enter => return self.advance(project),
                KeyCode::Backspace => self.previous(project),
                KeyCode::Char(c) if key.kind != KeyEventKind::Repeat => {
                    let choices = self.choices(project, self.field);
                    if let Some(choice) = choices
                        .iter()
                        .find(|p| p.view.key == Some(c.to_ascii_lowercase()))
                    {
                        self.set_choice(project, self.field, choice.choice.clone());
                        self.error = None;
                        if matches!(
                            choice.choice,
                            Choice::SamplerSource(SamplerSourceChoice::Browse)
                        ) {
                            return WorkflowOutcome::ChooseSample;
                        }
                        return self.advance(project);
                    }
                }
                _ => {}
            }
            return WorkflowOutcome::Continue;
        }
        match key.code {
            KeyCode::Char('F')
                if self.field == DraftField::SampleFolder
                    && self.sample_folder.is_empty()
                    && key.kind != KeyEventKind::Repeat =>
            {
                WorkflowOutcome::BrowseFolder
            }
            KeyCode::Enter => self.advance(project),
            KeyCode::Backspace => {
                let replace = self.replace_on_type;
                if let Some(text) = self.text_mut() {
                    if replace {
                        text.clear();
                    } else {
                        text.pop();
                    }
                }
                self.replace_on_type = false;
                self.error = None;
                WorkflowOutcome::Continue
            }
            KeyCode::Char(c) if !c.is_control() => {
                let replace = self.replace_on_type;
                let max = if self.field == DraftField::SampleFolder {
                    512
                } else if is_name(self.field) {
                    120
                } else {
                    32
                };
                if let Some(text) = self.text_mut() {
                    if replace {
                        text.clear();
                    }
                    if text.chars().count() < max {
                        text.push(c);
                        self.error = None;
                    } else {
                        self.error = Some(format!("Input is limited to {max} characters"));
                    }
                }
                if self.field == DraftField::Name {
                    self.name_edited = true;
                }
                if self.field == DraftField::Duration {
                    self.duration_edited = true;
                }
                if self.field == DraftField::EffectSlot {
                    self.slot_edited = true;
                }
                self.replace_on_type = false;
                WorkflowOutcome::Continue
            }
            _ => WorkflowOutcome::Continue,
        }
    }

    pub(super) fn fields(&self) -> Vec<DraftField> {
        use DraftField::*;
        let mut fields = Vec::new();
        match self.kind {
            WorkflowKind::Instrument => {
                fields.push(Name);
                self.instrument_fields(&mut fields, false);
            }
            WorkflowKind::Part => {
                fields.push(Name);
                self.part_fields(&mut fields);
            }
            WorkflowKind::Lane => fields.push(Name),
            WorkflowKind::EditLanePart { .. } => fields.extend([Phase, Offset]),
            WorkflowKind::FreezePattern(_) => fields.extend([Name, Start, PatternLength]),
            WorkflowKind::Place => {
                fields.push(Part);
                if self.part == Some(RefChoice::New) {
                    fields.push(PartName);
                    self.part_fields(&mut fields);
                }
                self.placement_fields(&mut fields);
            }
            WorkflowKind::PlacedPart => {
                fields.push(Name);
                self.part_fields(&mut fields);
                self.placement_fields(&mut fields);
            }
            WorkflowKind::Section | WorkflowKind::EditSection(_) => {
                fields.extend([Name, Start, Duration]);
            }
            WorkflowKind::Channel => {
                fields.extend([ChannelKind, Name]);
                if self.channel_kind != cadence_core::mixer::ChannelKind::Return {
                    fields.push(ChannelOutput);
                }
            }
            WorkflowKind::RenameInstrument(_)
            | WorkflowKind::RenameChannel(_)
            | WorkflowKind::RenameEffect(_)
            | WorkflowKind::RenamePart(_)
            | WorkflowKind::RenameLane(_)
            | WorkflowKind::RenameMacro(_) => {
                fields.push(Name);
            }
            WorkflowKind::Effect => {
                fields.extend([EffectKind, EffectChannel]);
            }
            WorkflowKind::MoveEffect(_) => {
                fields.extend([EffectChannel, EffectSlot]);
            }
            WorkflowKind::Send => {
                fields.extend([SendSource, SendTarget]);
                if self.send_target == Some(RefChoice::New) {
                    fields.push(ChannelName);
                }
                fields.push(SendAmount);
            }
            WorkflowKind::Duck | WorkflowKind::EditDuck(_) => {
                fields.extend([Name, DuckSource, DuckTarget, DuckDepth, DuckRecovery]);
            }
            WorkflowKind::Modulation | WorkflowKind::EditModulation(_) => {
                fields.push(ModPattern);
                if self.mod_pattern == Some(RefChoice::New) {
                    fields.push(PatternName);
                }
                fields.push(ModTarget);
                if !matches!(self.mod_target_node, Some(ModTargetNode::Macro(_))) {
                    fields.push(ModParam);
                }
                fields.push(Name);
            }
            WorkflowKind::ZoneImportFolder(_) => {
                fields.push(SampleFolder);
            }
            WorkflowKind::SavePreset(_) => {
                fields.extend([Name, PresetCategory]);
            }
            WorkflowKind::Bounce => {
                fields.push(BounceAction);
                fields.push(BounceScope);
                match self.bounce_scope_kind {
                    BounceScopeKind::Part => fields.push(BouncePart),
                    BounceScopeKind::Pattern => fields.push(BouncePattern),
                    BounceScopeKind::Range => fields.push(BounceRange),
                }
                fields.push(BounceTarget);
                match self.bounce_target_kind {
                    BounceTargetKind::NewSampler => {}
                    BounceTargetKind::ReplaceZone => {
                        fields.extend([BounceZoneInstrument, BounceZone]);
                    }
                    BounceTargetKind::NewZoneMidi => {
                        fields.extend([BounceZoneInstrument, BounceZoneMidi]);
                    }
                }
                fields.extend([Name, BounceCreatePart]);
            }
            WorkflowKind::Record => {
                fields.push(RecordAction);
                fields.push(RecordMode);
                if self.record_mode != crate::recording::RecordingMode::Free {
                    fields.push(RecordBars);
                }
                fields.push(BounceTarget);
                match self.bounce_target_kind {
                    BounceTargetKind::NewSampler => {}
                    BounceTargetKind::ReplaceZone => {
                        fields.extend([BounceZoneInstrument, BounceZone]);
                    }
                    BounceTargetKind::NewZoneMidi => {
                        fields.extend([BounceZoneInstrument, BounceZoneMidi]);
                    }
                }
                fields.extend([Name, BounceCreatePart]);
            }
        }
        fields
    }

    fn part_fields(&self, fields: &mut Vec<DraftField>) {
        use DraftField::*;
        fields.push(Pattern);
        if self.pattern == Some(RefChoice::New) {
            fields.extend([PatternName, PatternLength]);
        }
        fields.push(Instrument);
        if self.instrument == Some(RefChoice::New) {
            fields.push(InstrumentName);
            self.instrument_fields(fields, true);
        }
        fields.extend([PartTranspose, PartVelocity]);
    }

    fn instrument_fields(&self, fields: &mut Vec<DraftField>, _nested: bool) {
        use DraftField::*;
        fields.push(InstrumentKind);
        match self.rack_template {
            RackTemplate::Synth | RackTemplate::Modular => fields.push(Preset),
            RackTemplate::Sampler => fields.push(Sample),
            RackTemplate::Kit => {
                fields.push(KitSource);
                if self.kit_source == KitSourceChoice::Folder {
                    fields.push(SampleFolder);
                }
            }
        }
        fields.push(Channel);
        if self.channel == Some(RefChoice::New) {
            fields.push(ChannelName);
        }
    }

    fn placement_fields(&self, fields: &mut Vec<DraftField>) {
        use DraftField::*;
        fields.push(Lane);
        if self.lane == Some(RefChoice::New) {
            fields.push(LaneName);
        }
        fields.extend([Start, Duration, Phase, Offset]);
    }

    fn enter(&mut self, project: &Project, field: DraftField) {
        self.field = field;
        self.query.clear();
        self.searching = false;
        self.error = None;
        self.ensure_reference(project, field);
        if is_name(field)
            && !matches!(self.kind, WorkflowKind::Bounce | WorkflowKind::Record)
            && self.text().is_some_and(str::is_empty)
        {
            let name = self.name_default(project, field);
            if let Some(text) = self.text_mut() {
                *text = name;
            }
        }
        self.replace_on_type = self.text().is_some_and(|s| !s.is_empty());
    }

    fn previous(&mut self, project: &Project) {
        let fields = self.fields();
        if let Some(index) = fields
            .iter()
            .position(|f| *f == self.field)
            .filter(|i| *i > 0)
        {
            self.enter(project, fields[index - 1]);
        }
    }

    fn advance(&mut self, project: &Project) -> WorkflowOutcome {
        if self.field == DraftField::BounceAction {
            match self.bounce_action {
                BounceActionChoice::BounceNow => {
                    return match self.plan(project) {
                        Ok(plan) => WorkflowOutcome::Submit(Box::new(plan)),
                        Err(e) => {
                            self.error = Some(e.to_string());
                            WorkflowOutcome::Continue
                        }
                    };
                }
                BounceActionChoice::EditScope => {
                    self.enter(project, DraftField::BounceScope);
                    return WorkflowOutcome::Continue;
                }
                BounceActionChoice::EditRange => {
                    self.bounce_scope_kind = BounceScopeKind::Range;
                    self.enter(project, DraftField::BounceRange);
                    return WorkflowOutcome::Continue;
                }
                BounceActionChoice::EditTarget => {
                    self.enter(project, DraftField::BounceTarget);
                    return WorkflowOutcome::Continue;
                }
                BounceActionChoice::EditName => {
                    self.enter(project, DraftField::Name);
                    return WorkflowOutcome::Continue;
                }
                BounceActionChoice::EditCreatePart => {
                    self.enter(project, DraftField::BounceCreatePart);
                    return WorkflowOutcome::Continue;
                }
            }
        }
        if self.field == DraftField::RecordAction {
            match self.record_action {
                RecordActionChoice::RecordNow => {
                    return match self.plan(project) {
                        Ok(plan) => WorkflowOutcome::Submit(Box::new(plan)),
                        Err(e) => {
                            self.error = Some(e.to_string());
                            WorkflowOutcome::Continue
                        }
                    };
                }
                RecordActionChoice::EditMode => {
                    self.enter(project, DraftField::RecordMode);
                    return WorkflowOutcome::Continue;
                }
                RecordActionChoice::EditBars => {
                    if self.record_mode == crate::recording::RecordingMode::Free {
                        self.record_mode = crate::recording::RecordingMode::Quantized;
                    }
                    self.enter(project, DraftField::RecordBars);
                    return WorkflowOutcome::Continue;
                }
                RecordActionChoice::EditTarget => {
                    self.enter(project, DraftField::BounceTarget);
                    return WorkflowOutcome::Continue;
                }
                RecordActionChoice::EditName => {
                    self.enter(project, DraftField::Name);
                    return WorkflowOutcome::Continue;
                }
                RecordActionChoice::EditArrange => {
                    self.enter(project, DraftField::BounceCreatePart);
                    return WorkflowOutcome::Continue;
                }
            }
        }
        if self.field == DraftField::Sample && self.sampler_source == SamplerSourceChoice::Browse {
            return WorkflowOutcome::ChooseSample;
        }
        if let Err(e) = self.validate_field(project, self.field) {
            self.error = Some(e.to_string());
            return WorkflowOutcome::Continue;
        }
        if is_name(self.field) {
            if let Some(text) = self.text_mut() {
                *text = text.trim().into();
            }
            if self.field == DraftField::Name {
                self.name_edited = true;
            }
        }
        if self.field == DraftField::Start && !self.duration_edited {
            if let Ok(start) = parse_position(&self.start) {
                let remaining = (project.arrangement_steps() as u32).saturating_sub(start);
                self.duration = format_duration(if self.kind == WorkflowKind::PlacedPart {
                    remaining
                } else if self.kind == WorkflowKind::Section {
                    64.min(remaining).max(16.min(remaining)).max(1)
                } else {
                    16.min(remaining)
                });
            }
        }
        if self.field == DraftField::EffectChannel && !self.slot_edited {
            if let WorkflowKind::MoveEffect(dev_id) = self.kind {
                let owner = project.device(dev_id).map(|(c, _)| c);
                let count = match self.effect_channel {
                    ChannelRef::Master => project.mixer.master.inserts.len(),
                    ChannelRef::Channel(id) => project.channel(id).map_or(0, |c| c.inserts.len()),
                };
                if owner != Some(self.effect_channel) {
                    self.effect_slot = (count + 1).to_string();
                }
            }
        }
        if matches!(self.field, DraftField::SendSource | DraftField::SendTarget)
            && self.kind == WorkflowKind::Send
        {
            if let (Some(src), Some(RefChoice::Existing(ret))) =
                (self.send_source, self.send_target)
            {
                if let Some(existing) = project
                    .channel(src)
                    .and_then(|c| c.sends.iter().find(|s| s.target == ret))
                {
                    self.send_amount = format!("{:.0}%", existing.amount * 100.0);
                }
            }
        }
        if matches!(self.field, DraftField::ModTarget | DraftField::ModParam)
            && self.kind == WorkflowKind::Modulation
            && !self.name_edited
        {
            self.name = self.default_modulation_name(project);
        }
        let fields = self.fields();
        let index = fields.iter().position(|f| *f == self.field).unwrap_or(0);
        if let Some(&next) = fields.get(index + 1) {
            self.enter(project, next);
            WorkflowOutcome::Continue
        } else {
            match self.plan(project) {
                Ok(plan) => WorkflowOutcome::Submit(Box::new(plan)),
                Err(e) => {
                    self.error = Some(e.to_string());
                    WorkflowOutcome::Continue
                }
            }
        }
    }

    pub(super) fn text(&self) -> Option<&str> {
        Some(match self.field {
            DraftField::Name => &self.name,
            DraftField::PatternName => &self.pattern_name,
            DraftField::InstrumentName => &self.instrument_name,
            DraftField::ChannelName => &self.channel_name,
            DraftField::PartName => &self.part_name,
            DraftField::LaneName => &self.lane_name,
            DraftField::PatternLength => &self.pattern_length,
            DraftField::PartTranspose => &self.part_transpose,
            DraftField::PartVelocity => &self.part_velocity,
            DraftField::Start => &self.start,
            DraftField::Duration => &self.duration,
            DraftField::Offset => &self.offset,
            DraftField::SampleFolder => &self.sample_folder,
            DraftField::EffectSlot => &self.effect_slot,
            DraftField::SendAmount => &self.send_amount,
            DraftField::DuckDepth => &self.duck_depth,
            DraftField::DuckRecovery => &self.duck_recovery,
            DraftField::BounceRange => &self.bounce_range,
            DraftField::BounceZoneMidi => &self.bounce_zone_midi,
            DraftField::RecordBars => &self.record_bars,
            _ => return None,
        })
    }

    fn text_mut(&mut self) -> Option<&mut String> {
        Some(match self.field {
            DraftField::Name => &mut self.name,
            DraftField::PatternName => &mut self.pattern_name,
            DraftField::InstrumentName => &mut self.instrument_name,
            DraftField::ChannelName => &mut self.channel_name,
            DraftField::PartName => &mut self.part_name,
            DraftField::LaneName => &mut self.lane_name,
            DraftField::PatternLength => &mut self.pattern_length,
            DraftField::PartTranspose => &mut self.part_transpose,
            DraftField::PartVelocity => &mut self.part_velocity,
            DraftField::Start => &mut self.start,
            DraftField::Duration => &mut self.duration,
            DraftField::Offset => &mut self.offset,
            DraftField::SampleFolder => &mut self.sample_folder,
            DraftField::EffectSlot => &mut self.effect_slot,
            DraftField::SendAmount => &mut self.send_amount,
            DraftField::DuckDepth => &mut self.duck_depth,
            DraftField::DuckRecovery => &mut self.duck_recovery,
            DraftField::BounceRange => &mut self.bounce_range,
            DraftField::BounceZoneMidi => &mut self.bounce_zone_midi,
            DraftField::RecordBars => &mut self.record_bars,
            _ => return None,
        })
    }

    pub(super) fn reference(&self, field: DraftField) -> Option<RefChoice> {
        match field {
            DraftField::Pattern => self.pattern,
            DraftField::Instrument => self.instrument,
            DraftField::Channel => self.channel,
            DraftField::Part => self.part,
            DraftField::Lane => self.lane,
            DraftField::SendTarget => self.send_target,
            DraftField::ModPattern => self.mod_pattern,
            _ => None,
        }
    }

    pub(super) fn selected_choice(&self, field: DraftField) -> Option<Choice> {
        match field {
            DraftField::InstrumentKind => Some(Choice::Template(self.rack_template)),
            DraftField::Preset => Some(Choice::Preset(self.preset.clone())),
            DraftField::Sample => Some(Choice::SamplerSource(self.sampler_source.clone())),
            DraftField::KitSource => Some(Choice::KitSource(self.kit_source.clone())),
            DraftField::Phase => Some(Choice::Phase(self.phase)),
            DraftField::ChannelKind => Some(Choice::ChannelKind(self.channel_kind)),
            DraftField::ChannelOutput => Some(Choice::ChannelRef(self.channel_output)),
            DraftField::EffectKind => Some(Choice::EffectKind(self.effect_kind)),
            DraftField::EffectChannel => Some(Choice::ChannelRef(self.effect_channel)),
            DraftField::SendSource => self.send_source.map(Choice::ExistingId),
            DraftField::DuckSource => self.duck_source.map(Choice::ExistingId),
            DraftField::DuckTarget => Some(Choice::ChannelRef(self.duck_target)),
            DraftField::ModTarget => self.mod_target_node.map(Choice::ModTargetNode),
            DraftField::ModParam => self.mod_target.map(Choice::ModTarget),
            DraftField::PresetCategory => Some(Choice::PresetCategory(self.preset_category)),
            DraftField::BounceAction => Some(Choice::BounceAction(self.bounce_action)),
            DraftField::BounceScope => Some(Choice::BounceScopeKind(self.bounce_scope_kind)),
            DraftField::BouncePart => self.bounce_part.map(Choice::ExistingId),
            DraftField::BouncePattern => self.bounce_pattern.map(Choice::ExistingId),
            DraftField::BounceTarget => Some(Choice::BounceTargetKind(self.bounce_target_kind)),
            DraftField::BounceZoneInstrument => self.bounce_zone_instrument.map(Choice::ExistingId),
            DraftField::BounceZone => Some(Choice::BounceZoneIndex(self.bounce_zone_index)),
            DraftField::BounceCreatePart => Some(Choice::Bool(self.bounce_create_part)),
            DraftField::RecordAction => Some(Choice::RecordAction(self.record_action)),
            DraftField::RecordMode => Some(Choice::RecordMode(self.record_mode)),
            _ => self.reference(field).map(Choice::Reference),
        }
    }

    fn set_choice(&mut self, project: &Project, field: DraftField, choice: Choice) {
        match (field, choice) {
            (DraftField::Pattern, Choice::Reference(value)) => self.pattern = Some(value),
            (DraftField::Instrument, Choice::Reference(value)) => self.instrument = Some(value),
            (DraftField::Channel, Choice::Reference(value)) => self.channel = Some(value),
            (DraftField::Part, Choice::Reference(value)) => self.part = Some(value),
            (DraftField::Lane, Choice::Reference(value)) => self.lane = Some(value),
            (DraftField::SendTarget, Choice::Reference(value)) => self.send_target = Some(value),
            (DraftField::ModPattern, Choice::Reference(value)) => self.mod_pattern = Some(value),
            (DraftField::ModTarget, Choice::ModTargetNode(node)) => {
                self.mod_target_node = Some(node);
                if let ModTargetNode::Macro(index) = node {
                    self.mod_target = Some(ModulationTarget::Macro { index });
                } else if self
                    .mod_target
                    .is_some_and(|t| ModTargetNode::from_target(t) != node)
                {
                    self.mod_target = None;
                }
                self.ensure_reference(project, DraftField::ModParam);
            }
            (DraftField::ModParam, Choice::ModTarget(target)) => self.mod_target = Some(target),
            (DraftField::InstrumentKind, Choice::Template(value)) => self.rack_template = value,
            (DraftField::Preset, Choice::Preset(value)) => self.preset = value,
            (DraftField::Sample, Choice::SamplerSource(value)) => {
                if matches!(
                    value,
                    SamplerSourceChoice::Blank | SamplerSourceChoice::Preset(_)
                ) {
                    self.sample = None;
                }
                self.sampler_source = value;
            }
            (DraftField::KitSource, Choice::KitSource(value)) => self.kit_source = value,
            (DraftField::Phase, Choice::Phase(value)) => self.phase = value,
            (DraftField::ChannelKind, Choice::ChannelKind(value)) => {
                self.channel_kind = value;
                if !self.name_edited && self.kind == WorkflowKind::Channel {
                    self.name = unique_name(
                        value.label(),
                        project.mixer.channels.iter().map(|c| c.name.as_str()),
                        80,
                    );
                }
            }
            (DraftField::ChannelOutput, Choice::ChannelRef(value)) => self.channel_output = value,
            (DraftField::EffectKind, Choice::EffectKind(value)) => self.effect_kind = value,
            (DraftField::EffectChannel, Choice::ChannelRef(value)) => self.effect_channel = value,
            (DraftField::SendSource, Choice::ExistingId(value)) => self.send_source = Some(value),
            (DraftField::DuckSource, Choice::ExistingId(value)) => self.duck_source = Some(value),
            (DraftField::DuckTarget, Choice::ChannelRef(value)) => self.duck_target = value,
            (DraftField::PresetCategory, Choice::PresetCategory(cat)) => self.preset_category = cat,
            (DraftField::BounceAction, Choice::BounceAction(act)) => self.bounce_action = act,
            (DraftField::BounceScope, Choice::BounceScopeKind(kind)) => {
                self.bounce_scope_kind = kind
            }
            (DraftField::BouncePart, Choice::ExistingId(id)) => self.bounce_part = Some(id),
            (DraftField::BouncePattern, Choice::ExistingId(id)) => self.bounce_pattern = Some(id),
            (DraftField::BounceTarget, Choice::BounceTargetKind(kind)) => {
                self.bounce_target_kind = kind
            }
            (DraftField::BounceZoneInstrument, Choice::ExistingId(id)) => {
                self.bounce_zone_instrument = Some(id);
                if let Some(inst) = project.instrument(id) {
                    self.bounce_zone_index = inst.rack.clamp_zone_index(self.bounce_zone_index);
                }
            }
            (DraftField::BounceZone, Choice::BounceZoneIndex(idx)) => self.bounce_zone_index = idx,
            (DraftField::BounceCreatePart, Choice::Bool(b)) => self.bounce_create_part = b,
            (DraftField::RecordAction, Choice::RecordAction(act)) => self.record_action = act,
            (DraftField::RecordMode, Choice::RecordMode(mode)) => {
                self.record_mode = mode;
                if mode == crate::recording::RecordingMode::Free {
                    self.bounce_create_part = false;
                } else if self.bounce_target_kind == BounceTargetKind::NewSampler {
                    self.bounce_create_part = true;
                }
            }
            _ => {}
        }
    }

    fn move_choice(&mut self, project: &Project, delta: isize) {
        let choices = self.choices(project, self.field);
        if choices.is_empty() {
            return;
        }
        let current = self.selected_choice(self.field);
        let at = choices
            .iter()
            .position(|c| Some(&c.choice) == current.as_ref())
            .unwrap_or(0);
        let next = (at as isize + delta).clamp(0, choices.len() as isize - 1) as usize;
        self.set_choice(project, self.field, choices[next].choice.clone());
        self.error = None;
    }

    fn ensure_reference(&mut self, project: &Project, field: DraftField) {
        if !is_picker(field) {
            return;
        }
        let choices = self.choices(project, field);
        let selected = self.selected_choice(field);
        if !choices.iter().any(|c| Some(&c.choice) == selected.as_ref()) {
            if let Some(first) = choices.first() {
                self.set_choice(project, field, first.choice.clone());
            } else {
                match field {
                    DraftField::Pattern => self.pattern = None,
                    DraftField::Instrument => self.instrument = None,
                    DraftField::Channel => self.channel = None,
                    DraftField::Part => self.part = None,
                    DraftField::Lane => self.lane = None,
                    DraftField::SendSource => self.send_source = None,
                    DraftField::SendTarget => self.send_target = None,
                    DraftField::DuckSource => self.duck_source = None,
                    DraftField::ModPattern => self.mod_pattern = None,
                    DraftField::ModTarget => self.mod_target_node = None,
                    DraftField::ModParam => self.mod_target = None,
                    DraftField::BouncePart => self.bounce_part = None,
                    DraftField::BouncePattern => self.bounce_pattern = None,
                    DraftField::BounceZoneInstrument => self.bounce_zone_instrument = None,
                    _ => {}
                }
            }
        }
    }
}
