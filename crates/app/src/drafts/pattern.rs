//! Cancellable pattern authoring drafts and typed source choices covering all 22 `PatternKind` variants.
//!
//! A draft owns presentation input only. It never allocates authored identities
//! or edits a Project; its final submission uses the ordinary application action.
use super::form::{bounded_integer, parse_midi_note, parse_normalized};
pub use super::form::{
    DraftCandidate, DraftField, DraftFieldView, DraftInput, DraftOutcome, DraftSubmission,
    DraftView,
};
use super::pattern_view::{depends_on, preview_project, type_label};
use crate::keys::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use anyhow::{ensure, Context, Result};
use cadence_core::material::{
    ArpMode, Id, PatternKind, PatternType, Project, ValuePoint, ValueUnit, MAX_BARS,
    MAX_LITERAL_STEPS, MAX_PATTERNS, STEPS_PER_BAR,
};
use cadence_core::music::Step;
use cadence_core::params::ValueMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NewKind {
    Notes,
    Rhythm,
    Degrees,
    Values,
    Seed,
    SeedDegrees,
    Mini,
    Euclidean,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Operation {
    Add,
    Mask,
    Transpose,
    From,
    TimeScale,
    Swing,
    Ratchet,
    Shift,
    Reverse,
    Chance,
    Every,
    Arp,
    RangeFold,
    Concat,
    Stack,
    Shape,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    pub(super) derived: bool,
    pub(super) target: Option<Id>,
    pub(super) preferred: Option<Id>,
    pub(super) field: DraftField,
    pub(super) kind: NewKind,
    pub(super) operation: Operation,
    pub(super) interpolation: ValueMode,
    pub(super) arp_mode: ArpMode,
    pub(super) name: String,
    pub(super) period: String,
    pub(super) mini_code: String,
    pub(super) pulses: String,
    pub(super) rotation: String,
    pub(super) seed: String,
    pub(super) minimum: String,
    pub(super) maximum: String,
    pub(super) semitones: String,
    pub(super) degree_offset: String,
    pub(super) velocity: String,
    pub(super) gate: String,
    pub(super) time_num: String,
    pub(super) time_den: String,
    pub(super) swing_amount: String,
    pub(super) ratchet_count: String,
    pub(super) shift_steps: String,
    pub(super) probability: String,
    pub(super) every_interval: String,
    pub(super) arp_rate: String,
    pub(super) low_midi: String,
    pub(super) high_midi: String,
    pub(super) notes: Option<Id>,
    pub(super) degrees: Option<Id>,
    pub(super) rhythm: Option<Id>,
    pub(super) source: Option<Id>,
    pub(super) alt_source: Option<Id>,
    pub(super) extra_parts: Vec<Id>,
    pub(super) extra_cursor: Option<Id>,
    pub(super) velocity_source: Option<Id>,
    pub(super) gate_source: Option<Id>,
    pub(super) replace_on_type: bool,
    pub(super) query: String,
    pub(super) searching: bool,
    pub(super) error: Option<String>,
}

impl Default for Draft {
    fn default() -> Self {
        Self::new_pattern()
    }
}

impl Draft {
    pub fn new_pattern() -> Self {
        Self {
            derived: false,
            target: None,
            preferred: None,
            field: DraftField::Kind,
            kind: NewKind::Notes,
            operation: Operation::Add,
            interpolation: ValueMode::Hold,
            arp_mode: ArpMode::Up,
            name: String::new(),
            period: "16".into(),
            mini_code: "0 2 4 7".into(),
            pulses: "5".into(),
            rotation: "0".into(),
            seed: "771".into(),
            minimum: "0".into(),
            maximum: "1".into(),
            semitones: "12".into(),
            degree_offset: "0".into(),
            velocity: "80%".into(),
            gate: "0.75".into(),
            time_num: "2".into(),
            time_den: "1".into(),
            swing_amount: "15%".into(),
            ratchet_count: "2".into(),
            shift_steps: "1".into(),
            probability: "75%".into(),
            every_interval: "4".into(),
            arp_rate: "1".into(),
            low_midi: "C3".into(),
            high_midi: "C5".into(),
            notes: None,
            degrees: None,
            rhythm: None,
            source: None,
            alt_source: None,
            extra_parts: Vec::new(),
            extra_cursor: None,
            velocity_source: None,
            gate_source: None,
            replace_on_type: false,
            query: String::new(),
            searching: false,
            error: None,
        }
    }

    /// Construct a pattern creation draft pre-selected to a `Mode::Create` shortcut key
    /// (`n`, `r`, `d`, `v`, `s`, `g`, `m`, `e`) and immediately positioned on `DraftField::Name`.
    pub(crate) fn new_pattern_for_key(choice: char, project: &Project) -> Option<Self> {
        let kind = match choice.to_ascii_lowercase() {
            'n' => NewKind::Notes,
            'r' => NewKind::Rhythm,
            'd' => NewKind::Degrees,
            'v' => NewKind::Values,
            's' => NewKind::Seed,
            'g' => NewKind::SeedDegrees,
            'm' => NewKind::Mini,
            'e' => NewKind::Euclidean,
            _ => return None,
        };
        let mut draft = Self::new_pattern();
        draft.apply_new_kind(kind);
        draft.enter(DraftField::Name, project);
        Some(draft)
    }

    pub fn derive(preferred: Option<Id>) -> Self {
        Self {
            derived: true,
            preferred,
            field: DraftField::Name,
            ..Self::new_pattern()
        }
    }

    pub fn edit(project: &Project, target: Id) -> Result<Self> {
        let pattern = project.pattern(target).context("Pattern missing")?;
        match &pattern.kind {
            PatternKind::Mini { code, steps } => {
                let mut draft = Self::new_pattern();
                draft.target = Some(target);
                draft.name = pattern.name.clone();
                draft.kind = NewKind::Mini;
                draft.mini_code = code.clone();
                draft.period = steps.to_string();
                draft.enter(DraftField::MiniCode, project);
                return Ok(draft);
            }
            PatternKind::Euclidean {
                pulses,
                steps,
                rotation,
            } => {
                let mut draft = Self::new_pattern();
                draft.target = Some(target);
                draft.name = pattern.name.clone();
                draft.kind = NewKind::Euclidean;
                draft.period = steps.to_string();
                draft.pulses = pulses.to_string();
                draft.rotation = rotation.to_string();
                draft.enter(DraftField::Period, project);
                return Ok(draft);
            }
            PatternKind::SeededValues {
                unit,
                period,
                seed,
                min,
                max,
            } => {
                let mut draft = Self::new_pattern();
                draft.target = Some(target);
                draft.name = pattern.name.clone();
                draft.kind = match unit {
                    ValueUnit::Normalized => NewKind::Seed,
                    ValueUnit::Degrees => NewKind::SeedDegrees,
                };
                draft.period = period.to_string();
                draft.seed = seed.to_string();
                draft.minimum = if *unit == ValueUnit::Degrees {
                    format!("{:.0}", min.round())
                } else {
                    min.to_string()
                };
                draft.maximum = if *unit == ValueUnit::Degrees {
                    format!("{:.0}", max.round())
                } else {
                    max.to_string()
                };
                draft.enter(DraftField::ValueUnit, project);
                return Ok(draft);
            }
            PatternKind::Notes { steps } => {
                let mut draft = Self::new_pattern();
                draft.target = Some(target);
                draft.kind = NewKind::Notes;
                draft.name = pattern.name.clone();
                draft.period = steps.len().to_string();
                draft.enter(DraftField::Name, project);
                return Ok(draft);
            }
            PatternKind::Rhythm { steps } => {
                let mut draft = Self::new_pattern();
                draft.target = Some(target);
                draft.kind = NewKind::Rhythm;
                draft.name = pattern.name.clone();
                draft.period = steps.len().to_string();
                draft.enter(DraftField::Name, project);
                return Ok(draft);
            }
            PatternKind::Values {
                unit, period, mode, ..
            } => {
                let mut draft = Self::new_pattern();
                draft.target = Some(target);
                draft.kind = if *unit == ValueUnit::Degrees {
                    NewKind::Degrees
                } else {
                    NewKind::Values
                };
                draft.name = pattern.name.clone();
                draft.period = period.to_string();
                draft.interpolation = *mode;
                draft.enter(DraftField::Name, project);
                return Ok(draft);
            }
            _ => {}
        }

        let mut draft = Self::derive(None);
        draft.target = Some(target);
        draft.name = pattern.name.clone();
        match &pattern.kind {
            PatternKind::AddDegrees { notes, values } => {
                draft.operation = Operation::Add;
                draft.notes = Some(*notes);
                draft.degrees = Some(*values);
            }
            PatternKind::Mask { notes, rhythm } => {
                draft.operation = Operation::Mask;
                draft.notes = Some(*notes);
                draft.rhythm = Some(*rhythm);
            }
            PatternKind::Transpose { source, semitones } => {
                draft.operation = Operation::Transpose;
                draft.notes = Some(*source);
                draft.semitones = semitones.to_string();
            }
            PatternKind::NotesFromValues {
                values,
                rhythm,
                degree_offset,
                velocity,
                gate,
            } => {
                draft.operation = Operation::From;
                draft.degrees = Some(*values);
                draft.rhythm = Some(*rhythm);
                draft.degree_offset = degree_offset.to_string();
                draft.velocity = velocity.to_string();
                draft.gate = gate.to_string();
            }
            PatternKind::TimeScale { source, num, den } => {
                draft.operation = Operation::TimeScale;
                draft.source = Some(*source);
                draft.time_num = num.to_string();
                draft.time_den = den.to_string();
            }
            PatternKind::Swing { source, amount } => {
                draft.operation = Operation::Swing;
                draft.notes = Some(*source);
                draft.swing_amount = format!("{:.0}%", amount * 100.0);
            }
            PatternKind::Ratchet { source, count } => {
                draft.operation = Operation::Ratchet;
                draft.notes = Some(*source);
                draft.ratchet_count = count.to_string();
            }
            PatternKind::Shift { source, steps } => {
                draft.operation = Operation::Shift;
                draft.source = Some(*source);
                draft.shift_steps = steps.to_string();
            }
            PatternKind::Reverse { source } => {
                draft.operation = Operation::Reverse;
                draft.source = Some(*source);
            }
            PatternKind::Chance {
                source,
                probability,
                seed,
            } => {
                draft.operation = Operation::Chance;
                draft.source = Some(*source);
                draft.probability = format!("{:.0}%", probability * 100.0);
                draft.seed = seed.to_string();
            }
            PatternKind::Every {
                interval,
                base,
                alt,
            } => {
                draft.operation = Operation::Every;
                draft.every_interval = interval.to_string();
                draft.source = Some(*base);
                draft.alt_source = Some(*alt);
            }
            PatternKind::Arp { source, mode, rate } => {
                draft.operation = Operation::Arp;
                draft.notes = Some(*source);
                draft.arp_mode = *mode;
                draft.arp_rate = rate.to_string();
            }
            PatternKind::RangeFold {
                source,
                low_midi,
                high_midi,
            } => {
                draft.operation = Operation::RangeFold;
                draft.notes = Some(*source);
                draft.low_midi = cadence_core::music::note_name(*low_midi);
                draft.high_midi = cadence_core::music::note_name(*high_midi);
            }
            PatternKind::Concat { parts } => {
                draft.operation = Operation::Concat;
                draft.source = parts.first().copied();
                draft.alt_source = parts.get(1).copied();
                draft.extra_parts = parts.get(2..).map(<[Id]>::to_vec).unwrap_or_default();
                draft.extra_cursor = None;
            }
            PatternKind::Stack { parts } => {
                draft.operation = Operation::Stack;
                draft.source = parts.first().copied();
                draft.alt_source = parts.get(1).copied();
                draft.extra_parts = parts.get(2..).map(<[Id]>::to_vec).unwrap_or_default();
                draft.extra_cursor = None;
            }
            PatternKind::Shape {
                source,
                velocity,
                gate,
            } => {
                draft.operation = Operation::Shape;
                draft.notes = Some(*source);
                draft.velocity_source = Some(*velocity);
                draft.gate_source = *gate;
            }
            PatternKind::Notes { .. }
            | PatternKind::Rhythm { .. }
            | PatternKind::Values { .. }
            | PatternKind::SeededValues { .. }
            | PatternKind::Mini { .. }
            | PatternKind::Euclidean { .. } => unreachable!("handled above"),
        }
        draft.enter(DraftField::Operation, project);
        Ok(draft)
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    fn apply_new_kind(&mut self, kind: NewKind) {
        let prev = self.kind;
        self.kind = kind;
        if kind == NewKind::SeedDegrees && prev != NewKind::SeedDegrees {
            self.minimum = "-7".into();
            self.maximum = "7".into();
        } else if kind == NewKind::Seed && prev == NewKind::SeedDegrees {
            self.minimum = "0".into();
            self.maximum = "1".into();
        }
    }

    pub fn handle_key(&mut self, project: &Project, key: KeyEvent) -> DraftOutcome {
        if key.kind == KeyEventKind::Release {
            return DraftOutcome::Continue;
        }
        if key.code == KeyCode::Esc {
            if self.field.is_reference() && (self.searching || !self.query.is_empty()) {
                self.searching = false;
                self.query.clear();
                self.error = None;
                self.ensure_reference(project, self.field);
                return DraftOutcome::Continue;
            }
            return DraftOutcome::Cancel;
        }
        if key.kind == KeyEventKind::Repeat
            && (matches!(key.code, KeyCode::Enter | KeyCode::BackTab)
                || self.field.is_choice()
                    && matches!(key.code, KeyCode::Char(c) if c != 'j' && c != 'k'))
        {
            return DraftOutcome::Continue;
        }
        if key.code == KeyCode::BackTab {
            self.previous(project);
            return DraftOutcome::Continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if key.code == KeyCode::Char('u') {
                if self.field.is_reference() {
                    self.query.clear();
                    self.searching = true;
                    self.ensure_reference(project, self.field);
                } else if let Some(text) = self.text_mut() {
                    text.clear();
                    self.replace_on_type = false;
                }
                self.error = None;
            }
            return DraftOutcome::Continue;
        }
        if key.modifiers.contains(KeyModifiers::ALT) {
            return DraftOutcome::Continue;
        }
        if self.field.is_choice() {
            let options = self.choices();
            let current = self.choice_index();
            let select = match key.code {
                KeyCode::Char('j') | KeyCode::Down => Some((current + 1).min(options.len() - 1)),
                KeyCode::Char('k') | KeyCode::Up => Some(current.saturating_sub(1)),
                KeyCode::Char(c) => options
                    .iter()
                    .position(|choice| choice.0 == c.to_ascii_lowercase()),
                _ => None,
            };
            if let Some(index) = select {
                self.choose(index, project);
                self.error = None;
                if matches!(key.code, KeyCode::Char(c) if c != 'j' && c != 'k') {
                    return self.advance(project);
                }
            }
            return match key.code {
                KeyCode::Enter => self.advance(project),
                KeyCode::Backspace => {
                    self.previous(project);
                    DraftOutcome::Continue
                }
                _ => DraftOutcome::Continue,
            };
        }
        if self.field.is_reference() {
            match key.code {
                KeyCode::Char('/') if !self.searching => {
                    self.searching = true;
                    self.query.clear();
                    self.error = None;
                }
                KeyCode::Char('x') if !self.searching && self.field == DraftField::ExtraParts => {
                    self.extra_parts.pop();
                    self.error = None;
                }
                KeyCode::Down => self.move_reference(project, 1),
                KeyCode::Up => self.move_reference(project, -1),
                KeyCode::Char('j') if !self.searching => self.move_reference(project, 1),
                KeyCode::Char('k') if !self.searching => self.move_reference(project, -1),
                KeyCode::Char(c) if self.searching && !c.is_control() => {
                    if self.query.chars().count() < 120 {
                        self.query.push(c);
                    }
                    self.ensure_reference(project, self.field);
                    self.error = None;
                }
                KeyCode::Backspace if self.searching => {
                    self.query.pop();
                    self.ensure_reference(project, self.field);
                    self.error = None;
                }
                KeyCode::Enter | KeyCode::Tab if self.searching => {
                    self.searching = false;
                }
                KeyCode::Backspace => self.previous(project),
                KeyCode::Enter => return self.advance(project),
                _ => {}
            }
            return DraftOutcome::Continue;
        }
        match key.code {
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
                DraftOutcome::Continue
            }
            KeyCode::Char(c) if !c.is_control() => {
                let replace = self.replace_on_type;
                let maximum = match self.field {
                    DraftField::MiniCode => 512,
                    DraftField::Name => 120,
                    _ => 32,
                };
                if let Some(text) = self.text_mut() {
                    if replace {
                        text.clear();
                    }
                    if text.chars().count() < maximum {
                        text.push(c);
                    } else {
                        self.error = Some(format!("Input is limited to {maximum} characters"));
                    }
                }
                self.replace_on_type = false;
                // A corrected field gets validated when Enter is pressed.
                if self.text().is_some_and(|s| s.chars().count() < maximum) {
                    self.error = None;
                }
                DraftOutcome::Continue
            }
            _ => DraftOutcome::Continue,
        }
    }

    pub(super) fn fields(&self) -> Vec<DraftField> {
        use DraftField::*;
        if self.derived {
            let mut fields = vec![Name, Operation];
            fields.extend(match self.operation {
                self::Operation::Add => vec![Notes, Degrees],
                self::Operation::Mask => vec![Notes, Rhythm],
                self::Operation::Transpose => vec![Notes, Semitones],
                self::Operation::From => {
                    vec![Degrees, Rhythm, DegreeOffset, Velocity, Gate]
                }
                self::Operation::TimeScale => vec![Source, TimeNum, TimeDen],
                self::Operation::Swing => vec![Notes, SwingAmount],
                self::Operation::Ratchet => vec![Notes, RatchetCount],
                self::Operation::Shift => vec![Source, ShiftSteps],
                self::Operation::Reverse => vec![Source],
                self::Operation::Chance => vec![Source, Probability, Seed],
                self::Operation::Every => vec![EveryInterval, Source, AltSource],
                self::Operation::Arp => vec![Notes, ArpMode, ArpRate],
                self::Operation::RangeFold => vec![Notes, LowMidi, HighMidi],
                self::Operation::Concat | self::Operation::Stack => {
                    vec![Source, AltSource, ExtraParts]
                }
                self::Operation::Shape => vec![Notes, VelocitySource, GateSource],
            });
            fields
        } else if self.target.is_some() {
            match self.kind {
                NewKind::Mini => vec![Name, MiniCode, Period],
                NewKind::Euclidean => vec![Name, Period, Pulses, Rotation],
                NewKind::Seed | NewKind::SeedDegrees => {
                    vec![Name, ValueUnit, Period, Seed, Minimum, Maximum]
                }
                NewKind::Values => vec![Name, Period, Interpolation],
                _ => vec![Name, Period],
            }
        } else {
            let mut fields = vec![Kind, Name, Period];
            match self.kind {
                NewKind::Values => fields.push(Interpolation),
                NewKind::Seed | NewKind::SeedDegrees => fields.extend([Seed, Minimum, Maximum]),
                NewKind::Mini => fields.push(MiniCode),
                NewKind::Euclidean => fields.extend([Pulses, Rotation]),
                _ => {}
            }
            fields
        }
    }

    pub(super) fn choices(&self) -> &'static [(char, &'static str, &'static str)] {
        Self::choices_for_field(self.field)
    }

    pub(super) fn choices_for_field(
        field: DraftField,
    ) -> &'static [(char, &'static str, &'static str)] {
        match field {
            DraftField::Kind => &[
                ('n', "Notes", "Literal notes/chords; starts empty"),
                ('r', "Rhythm", "Literal trigger steps; starts empty"),
                ('d', "Degrees", "Held scale-degree offsets; starts at zero"),
                ('v', "Values", "Normalized 0–1 control points"),
                ('s', "Seeded values", "Deterministic normalized values"),
                ('g', "Seeded degrees", "Deterministic scale-degree offsets"),
                ('m', "Mini", "Strudel/Tidal inline mini-notation"),
                ('e', "Euclidean", "Bjorklund rhythm E(pulses, steps, rot)"),
            ],
            DraftField::Operation => &[
                (
                    'a',
                    "Add degrees",
                    "Notes + degree values; independent clocks",
                ),
                ('m', "Mask", "Notes gated by Rhythm or Notes onsets"),
                ('t', "Transpose", "Notes shifted in semitones"),
                (
                    'f',
                    "Notes from values",
                    "Degree values played on a trigger source",
                ),
                ('x', "Time scale", "Fast/slow ratio num/den on Notes/Rhythm"),
                ('w', "Swing", "Sub-step 16th-note swing delay (0–45%)"),
                (
                    'r',
                    "Ratchet",
                    "Subdivide each active hit into 1–8 retriggers",
                ),
                ('h', "Shift", "Rotate any pattern in time by signed steps"),
                ('v', "Reverse", "Play any pattern backwards over its period"),
                ('c', "Chance", "Deterministic per-hit probability gate"),
                ('e', "Every", "Switch to alt pattern every Nth cycle"),
                ('p', "Arp", "Arpeggiate held chord tones (up/down/ud/rand)"),
                ('o', "Range fold", "Octave-fold notes into [low, high] MIDI"),
                ('n', "Concat", "Sequence 2–16 same-type patterns end-to-end"),
                ('s', "Stack", "Layer 2–16 Notes or Rhythm patterns together"),
                ('g', "Shape", "Envelope Notes velocity (and optional gate)"),
            ],
            DraftField::Interpolation => &[
                ('h', "Hold", "Keep each point until the next point"),
                ('r', "Ramp", "Interpolate continuously between points"),
            ],
            DraftField::ValueUnit => &[
                ('n', "Normalized", "Scalar values in 0.0–1.0"),
                ('d', "Degrees", "Integer scale-degree offsets (-63..63)"),
            ],
            DraftField::ArpMode => &[
                ('u', "Up", "Ascending chord order"),
                ('d', "Down", "Descending chord order"),
                ('p', "Up-Down", "Ping-pong ascending then descending"),
                ('r', "Random", "Deterministic pseudo-random chord tone"),
            ],
            _ => &[],
        }
    }

    pub(super) fn choice_index(&self) -> usize {
        match self.field {
            DraftField::Kind => match self.kind {
                NewKind::Notes => 0,
                NewKind::Rhythm => 1,
                NewKind::Degrees => 2,
                NewKind::Values => 3,
                NewKind::Seed => 4,
                NewKind::SeedDegrees => 5,
                NewKind::Mini => 6,
                NewKind::Euclidean => 7,
            },
            DraftField::Operation => match self.operation {
                Operation::Add => 0,
                Operation::Mask => 1,
                Operation::Transpose => 2,
                Operation::From => 3,
                Operation::TimeScale => 4,
                Operation::Swing => 5,
                Operation::Ratchet => 6,
                Operation::Shift => 7,
                Operation::Reverse => 8,
                Operation::Chance => 9,
                Operation::Every => 10,
                Operation::Arp => 11,
                Operation::RangeFold => 12,
                Operation::Concat => 13,
                Operation::Stack => 14,
                Operation::Shape => 15,
            },
            DraftField::Interpolation => usize::from(self.interpolation == ValueMode::Ramp),
            DraftField::ValueUnit => usize::from(self.kind == NewKind::SeedDegrees),
            DraftField::ArpMode => match self.arp_mode {
                ArpMode::Up => 0,
                ArpMode::Down => 1,
                ArpMode::UpDown => 2,
                ArpMode::Random => 3,
            },
            _ => 0,
        }
    }

    fn choose(&mut self, index: usize, project: &Project) {
        match self.field {
            DraftField::Kind => {
                let kind = [
                    NewKind::Notes,
                    NewKind::Rhythm,
                    NewKind::Degrees,
                    NewKind::Values,
                    NewKind::Seed,
                    NewKind::SeedDegrees,
                    NewKind::Mini,
                    NewKind::Euclidean,
                ][index];
                self.apply_new_kind(kind);
            }
            DraftField::Operation => {
                self.operation = [
                    Operation::Add,
                    Operation::Mask,
                    Operation::Transpose,
                    Operation::From,
                    Operation::TimeScale,
                    Operation::Swing,
                    Operation::Ratchet,
                    Operation::Shift,
                    Operation::Reverse,
                    Operation::Chance,
                    Operation::Every,
                    Operation::Arp,
                    Operation::RangeFold,
                    Operation::Concat,
                    Operation::Stack,
                    Operation::Shape,
                ][index];
                if self.operation == Operation::Chance && self.seed == "771" {
                    self.seed = "1".into();
                }
                for field in self.fields().into_iter().filter(|f| f.is_reference()) {
                    self.ensure_reference(project, field);
                }
            }
            DraftField::Interpolation => {
                self.interpolation = if index == 0 {
                    ValueMode::Hold
                } else {
                    ValueMode::Ramp
                }
            }
            DraftField::ValueUnit => {
                let target_kind = if index == 0 {
                    NewKind::Seed
                } else {
                    NewKind::SeedDegrees
                };
                self.apply_new_kind(target_kind);
            }
            DraftField::ArpMode => {
                self.arp_mode =
                    [ArpMode::Up, ArpMode::Down, ArpMode::UpDown, ArpMode::Random][index];
            }
            _ => {}
        }
    }

    fn enter(&mut self, field: DraftField, project: &Project) {
        self.field = field;
        self.error = None;
        self.searching = false;
        self.query.clear();
        self.replace_on_type = self.text().is_some_and(|s| !s.is_empty());
        if field == DraftField::ExtraParts {
            self.extra_cursor = None;
            self.prune_incompatible_extra_parts(project);
        }
        if field == DraftField::Operation {
            for reference in self.fields().into_iter().filter(|f| f.is_reference()) {
                self.ensure_reference(project, reference);
            }
        } else if field.is_reference() {
            self.ensure_reference(project, field);
        }
    }

    fn previous(&mut self, project: &Project) {
        let fields = self.fields();
        if let Some(index) = fields
            .iter()
            .position(|&f| f == self.field)
            .filter(|&i| i > 0)
        {
            self.enter(fields[index - 1], project);
        }
    }

    fn advance(&mut self, project: &Project) -> DraftOutcome {
        if self.field == DraftField::ExtraParts {
            if let Some(id) = self.extra_cursor {
                if let Err(error) = self.validate_extra_candidate(project, id) {
                    self.error = Some(error.to_string());
                    return DraftOutcome::Continue;
                }
                if 2 + self.extra_parts.len() >= 16 {
                    self.error = Some("At most 16 parts are supported".into());
                    return DraftOutcome::Continue;
                }
                self.extra_parts.push(id);
                self.extra_cursor = None;
                self.searching = false;
                self.query.clear();
                self.error = None;
                return DraftOutcome::Continue;
            }
        }
        if let Err(error) = self.validate_field(project, self.field) {
            self.error = Some(error.to_string());
            return DraftOutcome::Continue;
        }
        if self.field == DraftField::Name {
            self.name = self.name.trim().into();
        }
        if self.field == DraftField::Source {
            self.prune_incompatible_extra_parts(project);
            self.ensure_reference(project, DraftField::AltSource);
        }
        let fields = self.fields();
        let index = fields.iter().position(|&f| f == self.field).unwrap_or(0);
        if let Some(&next) = fields.get(index + 1) {
            self.enter(next, project);
            DraftOutcome::Continue
        } else {
            match self.submission(project) {
                Ok(value) => DraftOutcome::Submit(value),
                Err(error) => {
                    self.error = Some(error.to_string());
                    DraftOutcome::Continue
                }
            }
        }
    }

    pub(super) fn text(&self) -> Option<&str> {
        Some(match self.field {
            DraftField::Name => &self.name,
            DraftField::Period => &self.period,
            DraftField::MiniCode => &self.mini_code,
            DraftField::Pulses => &self.pulses,
            DraftField::Rotation => &self.rotation,
            DraftField::Seed => &self.seed,
            DraftField::Minimum => &self.minimum,
            DraftField::Maximum => &self.maximum,
            DraftField::Semitones => &self.semitones,
            DraftField::DegreeOffset => &self.degree_offset,
            DraftField::Velocity => &self.velocity,
            DraftField::Gate => &self.gate,
            DraftField::TimeNum => &self.time_num,
            DraftField::TimeDen => &self.time_den,
            DraftField::SwingAmount => &self.swing_amount,
            DraftField::RatchetCount => &self.ratchet_count,
            DraftField::ShiftSteps => &self.shift_steps,
            DraftField::Probability => &self.probability,
            DraftField::EveryInterval => &self.every_interval,
            DraftField::ArpRate => &self.arp_rate,
            DraftField::LowMidi => &self.low_midi,
            DraftField::HighMidi => &self.high_midi,
            _ => return None,
        })
    }

    fn text_mut(&mut self) -> Option<&mut String> {
        Some(match self.field {
            DraftField::Name => &mut self.name,
            DraftField::Period => &mut self.period,
            DraftField::MiniCode => &mut self.mini_code,
            DraftField::Pulses => &mut self.pulses,
            DraftField::Rotation => &mut self.rotation,
            DraftField::Seed => &mut self.seed,
            DraftField::Minimum => &mut self.minimum,
            DraftField::Maximum => &mut self.maximum,
            DraftField::Semitones => &mut self.semitones,
            DraftField::DegreeOffset => &mut self.degree_offset,
            DraftField::Velocity => &mut self.velocity,
            DraftField::Gate => &mut self.gate,
            DraftField::TimeNum => &mut self.time_num,
            DraftField::TimeDen => &mut self.time_den,
            DraftField::SwingAmount => &mut self.swing_amount,
            DraftField::RatchetCount => &mut self.ratchet_count,
            DraftField::ShiftSteps => &mut self.shift_steps,
            DraftField::Probability => &mut self.probability,
            DraftField::EveryInterval => &mut self.every_interval,
            DraftField::ArpRate => &mut self.arp_rate,
            DraftField::LowMidi => &mut self.low_midi,
            DraftField::HighMidi => &mut self.high_midi,
            _ => return None,
        })
    }

    pub(super) fn reference(&self, field: DraftField) -> Option<Id> {
        match field {
            DraftField::Notes => self.notes,
            DraftField::Degrees => self.degrees,
            DraftField::Rhythm => self.rhythm,
            DraftField::Source => self.source,
            DraftField::AltSource => self.alt_source,
            DraftField::ExtraParts => self.extra_cursor,
            DraftField::VelocitySource => self.velocity_source,
            DraftField::GateSource => self.gate_source,
            _ => None,
        }
    }

    fn set_reference(&mut self, field: DraftField, value: Option<Id>) {
        match field {
            DraftField::Notes => self.notes = value,
            DraftField::Degrees => self.degrees = value,
            DraftField::Rhythm => self.rhythm = value,
            DraftField::Source => self.source = value,
            DraftField::AltSource => self.alt_source = value,
            DraftField::ExtraParts => self.extra_cursor = value,
            DraftField::VelocitySource => self.velocity_source = value,
            DraftField::GateSource => self.gate_source = value,
            _ => {}
        }
    }

    fn is_optional_reference(field: DraftField) -> bool {
        matches!(field, DraftField::GateSource | DraftField::ExtraParts)
    }

    fn prune_incompatible_extra_parts(&mut self, project: &Project) {
        let Some(base_type) = self.source.and_then(|id| project.pattern_type(id).ok()) else {
            self.extra_parts.clear();
            return;
        };
        let target = self.target;
        self.extra_parts.retain(|&id| {
            project.pattern_type(id).ok() == Some(base_type)
                && target.is_none_or(|t| !depends_on(project, id, t))
        });
    }

    fn eligible(&self, project: &Project, field: DraftField, id: Id) -> bool {
        let Ok(kind) = project.pattern_type(id) else {
            return false;
        };
        let type_ok = match field {
            DraftField::Notes => kind == PatternType::Notes,
            DraftField::Degrees => kind == PatternType::Values(ValueUnit::Degrees),
            DraftField::Rhythm => matches!(kind, PatternType::Rhythm | PatternType::Notes),
            DraftField::VelocitySource | DraftField::GateSource => {
                kind == PatternType::Values(ValueUnit::Normalized)
            }
            DraftField::Source => match self.operation {
                Operation::Chance | Operation::Stack => {
                    matches!(kind, PatternType::Notes | PatternType::Rhythm)
                }
                Operation::TimeScale
                | Operation::Shift
                | Operation::Reverse
                | Operation::Every
                | Operation::Concat => true,
                _ => kind == PatternType::Notes,
            },
            DraftField::AltSource | DraftField::ExtraParts => {
                match self.source.and_then(|src| project.pattern_type(src).ok()) {
                    Some(base_type) => kind == base_type,
                    None => match self.operation {
                        Operation::Stack => {
                            matches!(kind, PatternType::Notes | PatternType::Rhythm)
                        }
                        _ => true,
                    },
                }
            }
            _ => false,
        };
        type_ok
            && self
                .target
                .is_none_or(|target| !depends_on(project, id, target))
    }

    pub(super) fn references(&self, project: &Project, field: DraftField) -> Vec<DraftCandidate> {
        let query = if self.field == field {
            self.query.to_lowercase()
        } else {
            String::new()
        };
        let matches_q = |label: &str, detail: &str| {
            let haystack = format!("{label} {detail}").to_lowercase();
            query.split_whitespace().all(|word| haystack.contains(word))
        };
        let mut out = Vec::new();
        if field == DraftField::GateSource {
            let label = "None (keep source gate)";
            let detail = "Leave source note gate lengths unchanged";
            if matches_q(label, detail) {
                out.push(DraftCandidate {
                    id: None,
                    key: None,
                    label: label.into(),
                    detail: detail.into(),
                });
            }
        } else if field == DraftField::ExtraParts {
            let total = 2 + self.extra_parts.len();
            let label = format!("Done ({total} parts)");
            let detail = if self.extra_parts.is_empty() {
                "Accept 2 parts · pick a pattern below to append up to 16"
            } else {
                "Accept parts · pick below to append · x removes last extra"
            };
            if matches_q(&label, detail) {
                out.push(DraftCandidate {
                    id: None,
                    key: None,
                    label,
                    detail: detail.into(),
                });
            }
        }
        for p in project
            .patterns
            .iter()
            .take(MAX_PATTERNS)
            .filter(|p| self.eligible(project, field, p.id))
        {
            let Ok(kind) = project.pattern_type(p.id) else {
                continue;
            };
            let Ok(period) = project.pattern_period(p.id) else {
                continue;
            };
            let detail = format!("{} · {} steps · #{}", type_label(kind), period, p.id);
            if matches_q(&p.name, &detail) {
                out.push(DraftCandidate {
                    id: Some(p.id),
                    key: None,
                    label: p.name.clone(),
                    detail,
                });
            }
        }
        out
    }

    fn ensure_reference(&mut self, project: &Project, field: DraftField) {
        let candidates = self.references(project, field);
        let current = self.reference(field);
        if Self::is_optional_reference(field) {
            if candidates.iter().any(|c| c.id == current) {
                return;
            }
            let selected = candidates.first().and_then(|c| c.id);
            self.set_reference(field, selected);
            return;
        }
        if current.is_some_and(|id| candidates.iter().any(|c| c.id == Some(id))) {
            return;
        }
        let selected = if field == DraftField::AltSource {
            candidates
                .iter()
                .find(|c| c.id != self.source)
                .or_else(|| candidates.first())
                .and_then(|c| c.id)
        } else {
            self.preferred
                .filter(|id| candidates.iter().any(|c| c.id == Some(*id)))
                .or_else(|| candidates.first().and_then(|c| c.id))
        };
        self.set_reference(field, selected);
    }

    fn move_reference(&mut self, project: &Project, direction: i8) {
        let candidates = self.references(project, self.field);
        if candidates.is_empty() {
            return;
        }
        let current = self.reference(self.field);
        let index = candidates.iter().position(|c| c.id == current);
        let next = match (index, direction > 0) {
            (Some(i), true) => (i + 1).min(candidates.len() - 1),
            (Some(i), false) => i.saturating_sub(1),
            (None, true) => 0,
            (None, false) => candidates.len() - 1,
        };
        self.set_reference(self.field, candidates[next].id);
        self.error = None;
    }

    fn validate_extra_candidate(&self, project: &Project, id: Id) -> Result<()> {
        ensure!(
            self.eligible(project, DraftField::ExtraParts, id),
            "Part is missing, incompatible or would create a cycle"
        );
        if self.field == DraftField::ExtraParts {
            ensure!(
                self.references(project, DraftField::ExtraParts)
                    .iter()
                    .any(|c| c.id == Some(id)),
                "Choose a visible source from the filtered list"
            );
        }
        Ok(())
    }

    fn is_degree_seed(&self) -> bool {
        !self.derived && self.kind == NewKind::SeedDegrees
    }

    fn parse_seed_bound(&self, text: &str, label: &str) -> Result<f32> {
        if self.is_degree_seed() {
            Ok(f32::from(bounded_integer(text, -63, 63, label)?))
        } else {
            parse_normalized(text)
        }
    }

    fn validate_field(&self, project: &Project, field: DraftField) -> Result<()> {
        match field {
            DraftField::Name => {
                let name = self.name.trim();
                ensure!(
                    !name.is_empty()
                        && name.chars().count() <= 120
                        && !name.chars().any(char::is_control),
                    "Use a name with 1–120 printable characters"
                );
                ensure!(
                    !project
                        .patterns
                        .iter()
                        .any(|p| Some(p.id) != self.target && p.name.eq_ignore_ascii_case(name)),
                    "A pattern named '{name}' already exists"
                );
            }
            DraftField::Period => {
                let maximum = if matches!(
                    self.kind,
                    NewKind::Notes | NewKind::Rhythm | NewKind::Mini | NewKind::Euclidean
                ) {
                    MAX_LITERAL_STEPS
                } else {
                    MAX_BARS * STEPS_PER_BAR
                };
                let period = self
                    .period
                    .trim()
                    .parse::<usize>()
                    .context("Period must be an integer number of steps")?;
                ensure!(
                    (1..=maximum).contains(&period),
                    "Period must be 1–{maximum} steps"
                );
            }
            DraftField::MiniCode => {
                let code = self.mini_code.trim();
                ensure!(!code.is_empty(), "Enter mini-notation code (e.g. 0 2 4 7)");
                cadence_core::mini::parse_mini(code)?;
            }
            DraftField::Pulses => {
                let steps = self
                    .period
                    .trim()
                    .parse::<u16>()
                    .context("Steps must be 1–1024")?;
                let pulses = self
                    .pulses
                    .trim()
                    .parse::<u16>()
                    .context("Pulses must be an unsigned integer")?;
                ensure!(
                    pulses <= steps,
                    "Pulses must be 0–{steps} (cannot exceed steps)"
                );
            }
            DraftField::Rotation => {
                let steps = self
                    .period
                    .trim()
                    .parse::<i16>()
                    .unwrap_or(MAX_LITERAL_STEPS as i16);
                let bound = steps.saturating_sub(1);
                bounded_integer(&self.rotation, -bound, bound, "Rotation")?;
            }
            DraftField::Seed => {
                if self.derived && self.operation == Operation::Chance {
                    self.seed
                        .trim()
                        .parse::<u32>()
                        .context("Seed must be an unsigned 32-bit integer")?;
                } else {
                    self.seed
                        .trim()
                        .parse::<u64>()
                        .context("Seed must be an unsigned integer")?;
                }
            }
            DraftField::Minimum | DraftField::Maximum => {
                let value = self.parse_seed_bound(
                    if field == DraftField::Minimum {
                        &self.minimum
                    } else {
                        &self.maximum
                    },
                    field.label(),
                )?;
                if field == DraftField::Maximum {
                    ensure!(
                        self.parse_seed_bound(&self.minimum, "Minimum")? <= value,
                        "Maximum must be at least the minimum"
                    );
                }
            }
            DraftField::Semitones => {
                bounded_integer(&self.semitones, -96, 96, "Transpose")?;
            }
            DraftField::DegreeOffset => {
                bounded_integer(&self.degree_offset, -63, 63, "Degree offset")?;
            }
            DraftField::Velocity => {
                parse_normalized(&self.velocity)?;
            }
            DraftField::Gate => {
                let gate = self
                    .gate
                    .trim()
                    .parse::<f32>()
                    .context("Gate must be a number of steps")?;
                ensure!(
                    gate.is_finite()
                        && (crate::param_ops::MIN_STEP_GATE..=crate::param_ops::MAX_STEP_GATE)
                            .contains(&gate),
                    "Gate must be 0.05–64 steps"
                );
            }
            DraftField::TimeNum => {
                bounded_integer(&self.time_num, 1, 16, "Speed numerator")?;
            }
            DraftField::TimeDen => {
                bounded_integer(&self.time_den, 1, 16, "Speed denominator")?;
            }
            DraftField::SwingAmount => {
                let amt = parse_normalized(&self.swing_amount)?;
                ensure!(
                    (0.0..=0.45).contains(&amt),
                    "Swing amount must be 0–45% (0.0–0.45)"
                );
            }
            DraftField::RatchetCount => {
                bounded_integer(&self.ratchet_count, 1, 8, "Ratchet hits")?;
            }
            DraftField::ShiftSteps => {
                let steps = self
                    .shift_steps
                    .trim()
                    .parse::<i32>()
                    .context("Shift must be a signed integer number of steps")?;
                ensure!(
                    steps.unsigned_abs() <= 1_048_576,
                    "Shift must be within ±1048576 steps"
                );
            }
            DraftField::Probability => {
                parse_normalized(&self.probability)?;
            }
            DraftField::EveryInterval => {
                bounded_integer(&self.every_interval, 2, 64, "Every interval")?;
            }
            DraftField::ArpRate => {
                bounded_integer(&self.arp_rate, 1, 4, "Arp rate")?;
            }
            DraftField::LowMidi => {
                let low = parse_midi_note(&self.low_midi)?;
                ensure!(low <= 116, "Low note must be ≤ 116 (need ≥ 1 octave span)");
            }
            DraftField::HighMidi => {
                let low = parse_midi_note(&self.low_midi)?;
                let high = parse_midi_note(&self.high_midi)?;
                ensure!(
                    low <= high && high <= 127 && (high - low) >= 11,
                    "High note must be at least 11 semitones above low note (0..=127)"
                );
            }
            DraftField::GateSource => {
                if let Some(id) = self.gate_source {
                    ensure!(
                        self.eligible(project, DraftField::GateSource, id),
                        "Gate source is missing, incompatible or would create a cycle"
                    );
                }
                if self.field == DraftField::GateSource {
                    ensure!(
                        self.references(project, DraftField::GateSource)
                            .iter()
                            .any(|c| c.id == self.gate_source),
                        "Choose a visible option from the filtered list"
                    );
                }
            }
            DraftField::ExtraParts => {
                for &id in &self.extra_parts {
                    ensure!(
                        self.eligible(project, DraftField::ExtraParts, id),
                        "Part is missing, incompatible or would create a cycle"
                    );
                }
                if self.field == DraftField::ExtraParts && self.extra_cursor.is_none() {
                    ensure!(
                        self.references(project, DraftField::ExtraParts)
                            .iter()
                            .any(|c| c.id.is_none()),
                        "Clear filter to accept or choose a visible part"
                    );
                }
            }
            f if f.is_reference() => {
                let id = self
                    .reference(f)
                    .context("Choose a compatible source from the list")?;
                ensure!(
                    self.eligible(project, f, id),
                    "Source is missing, incompatible or would create a cycle"
                );
                if self.field == f {
                    ensure!(
                        self.references(project, f).iter().any(|c| c.id == Some(id)),
                        "Choose a visible source from the filtered list"
                    );
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn candidate_kind(&self, project: &Project) -> Result<PatternKind> {
        for field in self.fields().into_iter().filter(|f| *f != DraftField::Name) {
            self.validate_field(project, field)?;
        }
        Ok(if self.derived {
            match self.operation {
                Operation::Add => PatternKind::AddDegrees {
                    notes: self.notes.context("Choose notes")?,
                    values: self.degrees.context("Choose degree values")?,
                },
                Operation::Mask => PatternKind::Mask {
                    notes: self.notes.context("Choose notes")?,
                    rhythm: self.rhythm.context("Choose a trigger source")?,
                },
                Operation::Transpose => PatternKind::Transpose {
                    source: self.notes.context("Choose notes")?,
                    semitones: bounded_integer(&self.semitones, -96, 96, "Transpose")?,
                },
                Operation::From => PatternKind::NotesFromValues {
                    values: self.degrees.context("Choose degree values")?,
                    rhythm: self.rhythm.context("Choose a trigger source")?,
                    degree_offset: bounded_integer(&self.degree_offset, -63, 63, "Degree offset")?,
                    velocity: parse_normalized(&self.velocity)?,
                    gate: self.gate.trim().parse()?,
                },
                Operation::TimeScale => PatternKind::TimeScale {
                    source: self.source.context("Choose source pattern")?,
                    num: bounded_integer(&self.time_num, 1, 16, "Speed numerator")? as u16,
                    den: bounded_integer(&self.time_den, 1, 16, "Speed denominator")? as u16,
                },
                Operation::Swing => PatternKind::Swing {
                    source: self.notes.context("Choose notes")?,
                    amount: parse_normalized(&self.swing_amount)?,
                },
                Operation::Ratchet => PatternKind::Ratchet {
                    source: self.notes.context("Choose notes")?,
                    count: bounded_integer(&self.ratchet_count, 1, 8, "Ratchet hits")? as u8,
                },
                Operation::Shift => PatternKind::Shift {
                    source: self.source.context("Choose source pattern")?,
                    steps: self.shift_steps.trim().parse()?,
                },
                Operation::Reverse => PatternKind::Reverse {
                    source: self.source.context("Choose source pattern")?,
                },
                Operation::Chance => PatternKind::Chance {
                    source: self.source.context("Choose source pattern")?,
                    probability: parse_normalized(&self.probability)?,
                    seed: self.seed.trim().parse()?,
                },
                Operation::Every => PatternKind::Every {
                    interval: bounded_integer(&self.every_interval, 2, 64, "Every interval")?
                        as u16,
                    base: self.source.context("Choose base pattern")?,
                    alt: self.alt_source.context("Choose alternate pattern")?,
                },
                Operation::Arp => PatternKind::Arp {
                    source: self.notes.context("Choose notes")?,
                    mode: self.arp_mode,
                    rate: bounded_integer(&self.arp_rate, 1, 4, "Arp rate")? as u8,
                },
                Operation::RangeFold => PatternKind::RangeFold {
                    source: self.notes.context("Choose notes")?,
                    low_midi: parse_midi_note(&self.low_midi)?,
                    high_midi: parse_midi_note(&self.high_midi)?,
                },
                Operation::Concat => {
                    let mut parts = vec![
                        self.source.context("Choose first pattern")?,
                        self.alt_source.context("Choose second pattern")?,
                    ];
                    parts.extend(&self.extra_parts);
                    PatternKind::Concat { parts }
                }
                Operation::Stack => {
                    let mut parts = vec![
                        self.source.context("Choose first pattern")?,
                        self.alt_source.context("Choose second pattern")?,
                    ];
                    parts.extend(&self.extra_parts);
                    PatternKind::Stack { parts }
                }
                Operation::Shape => PatternKind::Shape {
                    source: self.notes.context("Choose notes")?,
                    velocity: self
                        .velocity_source
                        .context("Choose normalized velocity curve")?,
                    gate: self.gate_source,
                },
            }
        } else {
            let period = self.period.trim().parse::<u32>()?;
            let existing = self.target.and_then(|id| project.pattern(id));
            match self.kind {
                NewKind::Notes => {
                    let mut steps = match existing.map(|p| &p.kind) {
                        Some(PatternKind::Notes { steps }) => steps.clone(),
                        _ => Vec::new(),
                    };
                    steps.resize_with(period as usize, Step::default);
                    PatternKind::Notes { steps }
                }
                NewKind::Rhythm => {
                    let mut steps = match existing.map(|p| &p.kind) {
                        Some(PatternKind::Rhythm { steps }) => steps.clone(),
                        _ => Vec::new(),
                    };
                    steps.resize(period as usize, false);
                    PatternKind::Rhythm { steps }
                }
                NewKind::Degrees | NewKind::Values => {
                    let mut points = match existing.map(|p| &p.kind) {
                        Some(PatternKind::Values { points, .. }) => points
                            .iter()
                            .copied()
                            .filter(|pt| pt.step < period)
                            .collect(),
                        _ => Vec::new(),
                    };
                    if points.is_empty() {
                        points.push(ValuePoint {
                            step: 0,
                            value: 0.0,
                        });
                    }
                    PatternKind::Values {
                        unit: if self.kind == NewKind::Degrees {
                            ValueUnit::Degrees
                        } else {
                            ValueUnit::Normalized
                        },
                        period,
                        mode: if self.kind == NewKind::Degrees {
                            ValueMode::Hold
                        } else {
                            self.interpolation
                        },
                        points,
                    }
                }
                NewKind::Seed | NewKind::SeedDegrees => PatternKind::SeededValues {
                    unit: if self.kind == NewKind::SeedDegrees {
                        ValueUnit::Degrees
                    } else {
                        ValueUnit::Normalized
                    },
                    period,
                    seed: self.seed.trim().parse()?,
                    min: self.parse_seed_bound(&self.minimum, "Minimum")?,
                    max: self.parse_seed_bound(&self.maximum, "Maximum")?,
                },
                NewKind::Mini => PatternKind::Mini {
                    code: self.mini_code.trim().into(),
                    steps: period as u16,
                },
                NewKind::Euclidean => PatternKind::Euclidean {
                    pulses: self.pulses.trim().parse()?,
                    steps: period as u16,
                    rotation: self.rotation.trim().parse()?,
                },
            }
        })
    }

    fn submission(&self, project: &Project) -> Result<DraftSubmission> {
        self.validate_field(project, DraftField::Name)?;
        ensure!(
            self.target.is_some() || project.patterns.len() < MAX_PATTERNS,
            "The pattern library is full ({MAX_PATTERNS} definitions)"
        );
        if let Some(id) = self.target {
            ensure!(
                project.pattern(id).is_some(),
                "The edited pattern was removed"
            );
        }
        let kind = self.candidate_kind(project)?;
        // Signature validation catches depth and period overflow without compiling
        // the arrangement or touching its instruments/assets.
        let (preview, root) = preview_project(project, &kind, self.target)?;
        preview.pattern_type(root)?;
        Ok(DraftSubmission {
            target: self.target,
            name: self.name.trim().into(),
            kind,
        })
    }
}
