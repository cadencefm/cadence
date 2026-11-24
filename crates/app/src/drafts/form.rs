//! Shared form field descriptors, input views, unit parsers, and validation helpers.
use anyhow::{ensure, Context, Result};
use cadence_core::material::{Id, PatternKind, STEPS_PER_BAR};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftField {
    Kind,
    Name,
    Period,
    Interpolation,
    ValueUnit,
    MiniCode,
    Pulses,
    Rotation,
    Seed,
    Minimum,
    Maximum,
    Operation,
    Notes,
    Degrees,
    Rhythm,
    Source,
    AltSource,
    ExtraParts,
    VelocitySource,
    GateSource,
    Semitones,
    DegreeOffset,
    Velocity,
    Gate,
    TimeNum,
    TimeDen,
    SwingAmount,
    RatchetCount,
    ShiftSteps,
    Probability,
    EveryInterval,
    ArpMode,
    ArpRate,
    LowMidi,
    HighMidi,
    InstrumentKind,
    Preset,
    KitSource,
    Sample,
    SampleFolder,
    Channel,
    Pattern,
    PatternLength,
    Instrument,
    Part,
    PartTranspose,
    PartVelocity,
    Lane,
    Start,
    Duration,
    Phase,
    Offset,
    PatternName,
    InstrumentName,
    ChannelName,
    PartName,
    LaneName,
    ChannelKind,
    ChannelOutput,
    EffectKind,
    EffectChannel,
    EffectSlot,
    SendSource,
    SendTarget,
    SendAmount,
    DuckSource,
    DuckTarget,
    DuckDepth,
    DuckRecovery,
    ModPattern,
    ModTarget,
    ModParam,
    PresetCategory,
    BounceAction,
    BounceScope,
    BouncePart,
    BouncePattern,
    BounceRange,
    BounceTarget,
    BounceZoneInstrument,
    BounceZone,
    BounceZoneMidi,
    BounceCreatePart,
    RecordAction,
    RecordMode,
    RecordBars,
}

impl DraftField {
    pub const ALL: [Self; 86] = [
        Self::Kind,
        Self::Name,
        Self::Period,
        Self::Interpolation,
        Self::ValueUnit,
        Self::MiniCode,
        Self::Pulses,
        Self::Rotation,
        Self::Seed,
        Self::Minimum,
        Self::Maximum,
        Self::Operation,
        Self::Notes,
        Self::Degrees,
        Self::Rhythm,
        Self::Source,
        Self::AltSource,
        Self::ExtraParts,
        Self::VelocitySource,
        Self::GateSource,
        Self::Semitones,
        Self::DegreeOffset,
        Self::Velocity,
        Self::Gate,
        Self::TimeNum,
        Self::TimeDen,
        Self::SwingAmount,
        Self::RatchetCount,
        Self::ShiftSteps,
        Self::Probability,
        Self::EveryInterval,
        Self::ArpMode,
        Self::ArpRate,
        Self::LowMidi,
        Self::HighMidi,
        Self::InstrumentKind,
        Self::Preset,
        Self::KitSource,
        Self::Sample,
        Self::SampleFolder,
        Self::Channel,
        Self::Pattern,
        Self::PatternLength,
        Self::Instrument,
        Self::Part,
        Self::PartTranspose,
        Self::PartVelocity,
        Self::Lane,
        Self::Start,
        Self::Duration,
        Self::Phase,
        Self::Offset,
        Self::PatternName,
        Self::InstrumentName,
        Self::ChannelName,
        Self::PartName,
        Self::LaneName,
        Self::ChannelKind,
        Self::ChannelOutput,
        Self::EffectKind,
        Self::EffectChannel,
        Self::EffectSlot,
        Self::SendSource,
        Self::SendTarget,
        Self::SendAmount,
        Self::DuckSource,
        Self::DuckTarget,
        Self::DuckDepth,
        Self::DuckRecovery,
        Self::ModPattern,
        Self::ModTarget,
        Self::ModParam,
        Self::PresetCategory,
        Self::BounceAction,
        Self::BounceScope,
        Self::BouncePart,
        Self::BouncePattern,
        Self::BounceRange,
        Self::BounceTarget,
        Self::BounceZoneInstrument,
        Self::BounceZone,
        Self::BounceZoneMidi,
        Self::BounceCreatePart,
        Self::RecordAction,
        Self::RecordMode,
        Self::RecordBars,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Kind => "Kind",
            Self::Name => "Name",
            Self::Period => "Period / steps",
            Self::Interpolation => "Interpolation",
            Self::ValueUnit => "Value unit",
            Self::MiniCode => "Mini-notation",
            Self::Pulses => "Pulses",
            Self::Rotation => "Rotation",
            Self::Seed => "Seed",
            Self::Minimum => "Minimum",
            Self::Maximum => "Maximum",
            Self::Operation => "Operation",
            Self::Notes => "Notes source",
            Self::Degrees => "Degree source",
            Self::Rhythm => "Trigger source",
            Self::Source => "Source pattern",
            Self::AltSource => "Second / alt source",
            Self::ExtraParts => "Additional parts",
            Self::VelocitySource => "Velocity curve",
            Self::GateSource => "Gate curve",
            Self::Semitones => "Semitones",
            Self::DegreeOffset => "Degree offset",
            Self::Velocity => "Velocity",
            Self::Gate => "Gate / steps",
            Self::TimeNum => "Speed numerator",
            Self::TimeDen => "Speed denominator",
            Self::SwingAmount => "Swing amount",
            Self::RatchetCount => "Ratchet hits",
            Self::ShiftSteps => "Shift / steps",
            Self::Probability => "Probability",
            Self::EveryInterval => "Every N cycles",
            Self::ArpMode => "Arp order",
            Self::ArpRate => "Arp rate / step",
            Self::LowMidi => "Low note / MIDI",
            Self::HighMidi => "High note / MIDI",
            Self::InstrumentKind => "Rack template",
            Self::Preset => "Preset",
            Self::KitSource => "Kit / Folder source",
            Self::Sample => "Sample",
            Self::SampleFolder => "Sample folder",
            Self::Channel => "Audio destination",
            Self::Pattern => "Note material",
            Self::PatternLength => "Note length / steps",
            Self::Instrument => "Instrument",
            Self::Part => "Part",
            Self::PartTranspose => "Transpose / semitones",
            Self::PartVelocity => "Velocity scale",
            Self::Lane => "Lane",
            Self::Start => "Start / bar or step",
            Self::Duration => "Duration / bars or steps",
            Self::Phase => "Clock phase",
            Self::Offset => "Offset / steps",
            Self::PatternName => "New pattern name",
            Self::InstrumentName => "New instrument name",
            Self::ChannelName => "New Audio channel",
            Self::PartName => "New part name",
            Self::LaneName => "New lane name",
            Self::ChannelKind => "Channel type",
            Self::ChannelOutput => "Output bus",
            Self::EffectKind => "Effect type",
            Self::EffectChannel => "Target channel",
            Self::EffectSlot => "Insert slot",
            Self::SendSource => "Source channel",
            Self::SendTarget => "Return channel",
            Self::SendAmount => "Send level",
            Self::DuckSource => "Sidechain part",
            Self::DuckTarget => "Target channel",
            Self::DuckDepth => "Duck depth",
            Self::DuckRecovery => "Recovery / ms",
            Self::ModPattern => "Value source",
            Self::ModTarget => "Target node",
            Self::ModParam => "Target parameter",
            Self::PresetCategory => "Preset category",
            Self::BounceAction => "Action",
            Self::BounceScope => "Bounce scope",
            Self::BouncePart => "Source part",
            Self::BouncePattern => "Source pattern",
            Self::BounceRange => "Bar range (A..B)",
            Self::BounceTarget => "Bounce target",
            Self::BounceZoneInstrument => "Target Rack",
            Self::BounceZone => "Target zone",
            Self::BounceZoneMidi => "Zone MIDI note",
            Self::BounceCreatePart => "Trigger part (--part)",
            Self::RecordAction => "Action",
            Self::RecordMode => "Timing mode",
            Self::RecordBars => "Length (bars, blank=open)",
        }
    }

    pub(super) fn is_reference(self) -> bool {
        matches!(
            self,
            Self::Notes
                | Self::Degrees
                | Self::Rhythm
                | Self::Source
                | Self::AltSource
                | Self::ExtraParts
                | Self::VelocitySource
                | Self::GateSource
        )
    }

    pub(super) fn is_choice(self) -> bool {
        matches!(
            self,
            Self::Kind | Self::Operation | Self::Interpolation | Self::ValueUnit | Self::ArpMode
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftInput {
    Text,
    Number,
    Choice,
    Reference,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DraftFieldView {
    pub label: String,
    pub value: String,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DraftCandidate {
    pub id: Option<Id>,
    pub key: Option<char>,
    pub label: String,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DraftView {
    pub title: String,
    pub fields: Vec<DraftFieldView>,
    pub field: DraftField,
    pub field_label: String,
    pub input: DraftInput,
    pub text: String,
    /// A prefilled field is selected on entry; typing replaces that selection.
    pub replace_on_type: bool,
    pub query: String,
    pub searching: bool,
    pub candidates: Vec<DraftCandidate>,
    pub selected: Option<usize>,
    pub summary: String,
    pub preview: Vec<String>,
    pub error: Option<String>,
    pub hints: Vec<(&'static str, &'static str)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DraftSubmission {
    pub target: Option<Id>,
    pub name: String,
    pub kind: PatternKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DraftOutcome {
    Continue,
    Cancel,
    Submit(DraftSubmission),
}

pub(super) fn is_picker(field: DraftField) -> bool {
    matches!(
        field,
        DraftField::Pattern
            | DraftField::Instrument
            | DraftField::Channel
            | DraftField::Part
            | DraftField::Lane
            | DraftField::InstrumentKind
            | DraftField::Preset
            | DraftField::Sample
            | DraftField::KitSource
            | DraftField::Phase
            | DraftField::ChannelKind
            | DraftField::ChannelOutput
            | DraftField::EffectKind
            | DraftField::EffectChannel
            | DraftField::SendSource
            | DraftField::SendTarget
            | DraftField::DuckSource
            | DraftField::DuckTarget
            | DraftField::ModPattern
            | DraftField::ModTarget
            | DraftField::ModParam
            | DraftField::PresetCategory
            | DraftField::BounceAction
            | DraftField::BounceScope
            | DraftField::BouncePart
            | DraftField::BouncePattern
            | DraftField::BounceTarget
            | DraftField::BounceZoneInstrument
            | DraftField::BounceZone
            | DraftField::BounceCreatePart
            | DraftField::RecordAction
            | DraftField::RecordMode
    )
}

pub(super) fn is_name(field: DraftField) -> bool {
    matches!(
        field,
        DraftField::Name
            | DraftField::PatternName
            | DraftField::InstrumentName
            | DraftField::ChannelName
            | DraftField::PartName
            | DraftField::LaneName
    )
}

/// Shared single-point-of-truth validation for 1..=limit printable names across
/// workflows, `Action::RenameInstrument`, `Action::RenameChannel`, and `Action::RenameEffect`.
pub(crate) fn validate_printable_name(entity: &str, name: &str, limit: usize) -> Result<String> {
    let trimmed = name.trim();
    ensure!(
        !trimmed.is_empty()
            && trimmed.chars().count() <= limit
            && !trimmed.chars().any(char::is_control),
        "{entity} name must be 1–{limit} printable characters"
    );
    Ok(trimmed.to_string())
}

pub(super) fn validate_new_name<'a>(
    name: &str,
    names: impl Iterator<Item = &'a str>,
    limit: usize,
) -> Result<()> {
    let trimmed = validate_printable_name("Entity", name, limit)?;
    ensure!(
        !names
            .into_iter()
            .any(|other| other.eq_ignore_ascii_case(&trimmed)),
        "Name '{trimmed}' already exists; choose another name"
    );
    Ok(())
}

pub(super) fn unique_name<'a>(
    base: &str,
    names: impl Iterator<Item = &'a str>,
    limit: usize,
) -> String {
    crate::unique_name(base, names, Some(limit))
}

pub(super) fn bounded_integer(text: &str, low: i16, high: i16, label: &str) -> Result<i16> {
    let value = text
        .trim()
        .parse::<i16>()
        .with_context(|| format!("{label} must be an integer"))?;
    ensure!(
        (low..=high).contains(&value),
        "{label} must be {low}..{high}"
    );
    Ok(value)
}

pub(super) fn parse_normalized(text: &str) -> Result<f32> {
    crate::param_ops::parse_normalized(text)
}

pub(super) fn parse_midi_note(text: &str) -> Result<u8> {
    let trimmed = text.trim();
    if let Ok(n) = trimmed.parse::<u8>() {
        ensure!(n <= 127, "MIDI note must be 0..=127");
        return Ok(n);
    }
    cadence_core::music::parse_root_midi(trimmed)
        .context("Enter a MIDI note 0..=127 or note name (e.g. C3, F#4)")
}

fn parse_units(text: &str) -> Result<(u32, bool)> {
    let text = text.trim();
    let (digits, steps) = if let Some(t) = text.strip_suffix(['s', 'S']) {
        (t, true)
    } else if let Some(t) = text.strip_suffix(['b', 'B']) {
        (t, false)
    } else {
        (text, false)
    };
    let value: u32 = digits
        .trim()
        .parse()
        .context("Use whole bars (4 or 4b), or sixteenth steps (5s)")?;
    ensure!(value > 0, "Positions and durations start at 1");
    Ok((value, steps))
}

pub fn parse_position(text: &str) -> Result<u32> {
    let (value, steps) = parse_units(text)?;
    if steps {
        Ok(value - 1)
    } else {
        (value - 1)
            .checked_mul(STEPS_PER_BAR as u32)
            .context("Start position overflow")
    }
}

pub fn parse_duration(text: &str) -> Result<u32> {
    let (value, steps) = parse_units(text)?;
    if steps {
        Ok(value)
    } else {
        value
            .checked_mul(STEPS_PER_BAR as u32)
            .context("Duration overflow")
    }
}

pub(super) fn format_start(step: u32) -> String {
    if step.is_multiple_of(16) {
        (step / 16 + 1).to_string()
    } else {
        format!("{}s", step + 1)
    }
}

pub(super) fn format_duration(length: u32) -> String {
    if length.is_multiple_of(16) {
        (length / 16).to_string()
    } else {
        format!("{length}s")
    }
}
