//! Projection, miniature dependency-graph preview, and shortcut-collision tests for pattern `Draft`.
use super::form::{DraftCandidate, DraftField, DraftFieldView, DraftInput, DraftView};
use super::pattern::{Draft, NewKind, Operation};
use anyhow::{ensure, Context, Result};
use cadence_core::material::{
    Id, Pattern, PatternKind, PatternType, PatternValue, Project, ValueUnit, MAX_PATTERNS,
};
use cadence_core::music::note_name;
use cadence_core::params::ValueMode;
use std::collections::{HashSet, VecDeque};

impl Draft {
    pub fn view(&self, project: &Project) -> DraftView {
        let fields = self
            .fields()
            .into_iter()
            .map(|field| DraftFieldView {
                label: field.label().into(),
                value: self.field_value(project, field),
                active: field == self.field,
            })
            .collect();
        let candidates = if self.field.is_reference() {
            self.references(project, self.field)
        } else {
            self.choices()
                .iter()
                .map(|&(key, label, detail)| DraftCandidate {
                    id: None,
                    key: Some(key),
                    label: label.into(),
                    detail: detail.into(),
                })
                .collect()
        };
        let selected = if self.field.is_reference() {
            let current = self.reference(self.field);
            candidates.iter().position(|c| c.id == current)
        } else if self.field.is_choice() {
            Some(self.choice_index())
        } else {
            None
        };
        let (mut summary, preview) = match self.candidate_kind(project).and_then(|kind| {
            let (temporary, root) = preview_project(project, &kind, self.target)?;
            let kind = temporary.pattern_type(root)?;
            let period = temporary.pattern_period(root)?;
            let preview = temporary
                .preview(root, 0, 8)?
                .into_iter()
                .map(|sample| format_preview(&temporary, &sample.value))
                .collect::<Vec<_>>();
            Ok((
                format!(
                    "{} · {period}-step repeat · retained definition",
                    type_label(kind)
                ),
                vec![format!("Steps 1–8: {}", preview.join("  "))],
            ))
        }) {
            Ok(value) => value,
            Err(_) => (
                if self.derived {
                    "Choose compatible inputs to preview the retained result"
                } else {
                    "Choose a kind, name and parameters"
                }
                .into(),
                Vec::new(),
            ),
        };
        if self.field.is_reference() && candidates.is_empty() {
            summary = if self.query.is_empty() {
                format!(
                    "No compatible {}. Esc returns to create material first.",
                    self.field.label().to_lowercase()
                )
            } else {
                "No matching sources. Esc clears the filter; choose only a visible result.".into()
            };
        }
        let esc_label = if self.searching || !self.query.is_empty() {
            "clear filter"
        } else {
            "cancel"
        };
        let hints = if self.field.is_reference() && self.searching {
            vec![
                ("type", "filter"),
                ("↑/↓", "select"),
                ("Enter/Tab", "apply filter"),
                ("Ctrl+u", "clear"),
                ("Shift+Tab", "back"),
                ("Esc", "clear filter"),
            ]
        } else if self.field == DraftField::ExtraParts {
            vec![
                ("j/k", "select"),
                ("/", "filter"),
                ("Enter", "add / done"),
                ("x", "pop extra"),
                ("Shift+Tab", "back"),
                ("Esc", esc_label),
            ]
        } else if self.field.is_reference() {
            vec![
                ("j/k", "select"),
                ("/", "filter"),
                ("Enter", "choose"),
                ("Shift+Tab", "back"),
                ("Esc", esc_label),
            ]
        } else if self.field.is_choice() {
            vec![
                (
                    match self.field {
                        DraftField::Kind => "n/r/d/v/s/g/m/e",
                        DraftField::Operation => "shortcut",
                        DraftField::ArpMode => "u/d/p/r",
                        DraftField::ValueUnit => "n/d",
                        _ => "h/r",
                    },
                    "choose",
                ),
                ("j/k", "select"),
                ("Enter", "accept"),
                ("Shift+Tab", "back"),
                ("Esc", "cancel"),
            ]
        } else {
            vec![
                (
                    "type",
                    if self.replace_on_type {
                        "replace selected text"
                    } else {
                        "edit text"
                    },
                ),
                ("Enter", "accept"),
                ("Ctrl+u", "clear"),
                ("Shift+Tab", "back"),
                ("Esc", "cancel"),
            ]
        };
        DraftView {
            title: if self.target.is_some() {
                if self.derived {
                    "EDIT DERIVATION"
                } else {
                    "EDIT PATTERN"
                }
            } else if self.derived {
                "DERIVE PATTERN"
            } else {
                "NEW PATTERN"
            }
            .into(),
            fields,
            field: self.field,
            field_label: self.field.label().into(),
            input: if self.field.is_reference() {
                DraftInput::Reference
            } else if self.field.is_choice() {
                DraftInput::Choice
            } else if matches!(
                self.field,
                DraftField::Name
                    | DraftField::MiniCode
                    | DraftField::LowMidi
                    | DraftField::HighMidi
            ) {
                DraftInput::Text
            } else {
                DraftInput::Number
            },
            text: self.text().unwrap_or("").into(),
            replace_on_type: self.replace_on_type,
            query: self.query.clone(),
            searching: self.searching,
            candidates,
            selected,
            summary,
            preview,
            error: self.error.clone(),
            hints,
        }
    }

    fn field_value(&self, project: &Project, field: DraftField) -> String {
        match field {
            DraftField::Kind => match self.kind {
                NewKind::Notes => "Notes",
                NewKind::Rhythm => "Rhythm",
                NewKind::Degrees => "Degrees / Hold",
                NewKind::Values => "Normalized values",
                NewKind::Seed => "Seeded normalized values",
                NewKind::SeedDegrees => "Seeded degree values",
                NewKind::Mini => "Mini notation",
                NewKind::Euclidean => "Euclidean rhythm",
            }
            .into(),
            DraftField::Operation => match self.operation {
                Operation::Add => "Add degrees",
                Operation::Mask => "Mask",
                Operation::Transpose => "Transpose",
                Operation::From => "Notes from values",
                Operation::TimeScale => "Time scale",
                Operation::Swing => "Swing",
                Operation::Ratchet => "Ratchet",
                Operation::Shift => "Shift",
                Operation::Reverse => "Reverse",
                Operation::Chance => "Chance",
                Operation::Every => "Every",
                Operation::Arp => "Arp",
                Operation::RangeFold => "Range fold",
                Operation::Concat => "Concat",
                Operation::Stack => "Stack",
                Operation::Shape => "Shape",
            }
            .into(),
            DraftField::Interpolation => if self.interpolation == ValueMode::Hold {
                "Hold"
            } else {
                "Ramp"
            }
            .into(),
            DraftField::ValueUnit => if self.kind == NewKind::SeedDegrees {
                "Degrees"
            } else {
                "Normalized"
            }
            .into(),
            DraftField::ArpMode => self.arp_mode.label().into(),
            DraftField::Name => self.name.clone(),
            DraftField::Period => self.period.clone(),
            DraftField::MiniCode => self.mini_code.clone(),
            DraftField::Pulses => self.pulses.clone(),
            DraftField::Rotation => self.rotation.clone(),
            DraftField::Seed => self.seed.clone(),
            DraftField::Minimum => self.minimum.clone(),
            DraftField::Maximum => self.maximum.clone(),
            DraftField::Semitones => self.semitones.clone(),
            DraftField::DegreeOffset => self.degree_offset.clone(),
            DraftField::Velocity => self.velocity.clone(),
            DraftField::Gate => self.gate.clone(),
            DraftField::TimeNum => self.time_num.clone(),
            DraftField::TimeDen => self.time_den.clone(),
            DraftField::SwingAmount => self.swing_amount.clone(),
            DraftField::RatchetCount => self.ratchet_count.clone(),
            DraftField::ShiftSteps => self.shift_steps.clone(),
            DraftField::Probability => self.probability.clone(),
            DraftField::EveryInterval => self.every_interval.clone(),
            DraftField::ArpRate => self.arp_rate.clone(),
            DraftField::LowMidi => self.low_midi.clone(),
            DraftField::HighMidi => self.high_midi.clone(),
            DraftField::GateSource => self.gate_source.map_or_else(
                || "None (keep source)".into(),
                |id| {
                    project
                        .pattern(id)
                        .map_or_else(|| format!("Missing #{id}"), |p| p.name.clone())
                },
            ),
            DraftField::ExtraParts => {
                if self.extra_parts.is_empty() {
                    "None (2 parts)".into()
                } else {
                    self.extra_parts
                        .iter()
                        .map(|&id| {
                            project
                                .pattern(id)
                                .map_or_else(|| format!("#{id}"), |p| p.name.clone())
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            }
            reference => self.reference(reference).map_or_else(
                || "Choose…".into(),
                |id| {
                    project
                        .pattern(id)
                        .map_or_else(|| format!("Missing #{id}"), |p| p.name.clone())
                },
            ),
        }
    }
}

pub(super) fn type_label(kind: PatternType) -> &'static str {
    match kind {
        PatternType::Notes => "Notes",
        PatternType::Rhythm => "Rhythm",
        PatternType::Values(ValueUnit::Degrees) => "Degree values",
        PatternType::Values(ValueUnit::Normalized) => "Normalized values",
    }
}

pub(super) fn depends_on(project: &Project, source: Id, target: Id) -> bool {
    let mut visited = HashSet::new();
    let mut pending = VecDeque::from([source]);
    while let Some(id) = pending.pop_front() {
        if id == target {
            return true;
        }
        if visited.insert(id) {
            if visited.len() > MAX_PATTERNS {
                return true;
            }
            if let Some(pattern) = project.pattern(id) {
                pending.extend(pattern.kind.sources());
            }
        }
    }
    false
}

/// Build only the draft's needed source graph, with no instrument, mixer,
/// timeline compilation or asset preparation. Canonical core evaluation supplies
/// the miniature result instead of a second implementation of musical semantics.
pub(super) fn preview_project(
    project: &Project,
    kind: &PatternKind,
    target: Option<Id>,
) -> Result<(Project, Id)> {
    let mut preview = Project::empty();
    preview.root_midi = project.root_midi;
    preview.scale = project.scale.clone();
    let mut needed = HashSet::new();
    let mut pending = VecDeque::from(kind.sources());
    while let Some(id) = pending.pop_front() {
        ensure!(Some(id) != target, "A derivation cannot depend on itself");
        if needed.insert(id) {
            ensure!(
                needed.len() <= MAX_PATTERNS,
                "Draft source graph exceeds bounds"
            );
            let source = project.pattern(id).context("A draft source was removed")?;
            pending.extend(source.kind.sources());
            preview.patterns.push(source.clone());
        }
    }
    let root = match target {
        Some(id) => id,
        None => (1..=MAX_PATTERNS as u64 + 1)
            .find(|id| !needed.contains(id))
            .context("Draft pattern ID space exhausted")?,
    };
    preview.patterns.push(Pattern {
        id: root,
        name: "Draft result".into(),
        kind: kind.clone(),
    });
    Ok((preview, root))
}

fn format_preview(project: &Project, value: &PatternValue) -> String {
    match value {
        PatternValue::Notes(step) if step.active => {
            let first = note_name(project.pitch_midi(step.degree, step.accidental, 0));
            if step.chord.is_empty() {
                first
            } else {
                format!("{first}+{}", step.chord.len())
            }
        }
        PatternValue::Rhythm(true) => "x".into(),
        PatternValue::Value(v) if v.fract() == 0.0 => format!("{v:+.0}"),
        PatternValue::Value(v) => format!("{v:.2}"),
        _ => "·".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_draft_choice_shortcuts_never_collide_with_navigation_or_each_other() {
        // Reserved choice navigation keys matched before shortcut lookup in `Draft::handle_key`.
        const RESERVED: [char; 3] = ['j', 'k', '/'];
        let mut total_shortcuts = 0usize;
        for field in DraftField::ALL {
            let mut seen = HashSet::new();
            for &(key, label, _) in Draft::choices_for_field(field) {
                let lower = key.to_ascii_lowercase();
                assert_eq!(
                    key, lower,
                    "{field:?} candidate '{label}' shortcut '{key}' must be lowercase"
                );
                assert!(
                    !RESERVED.contains(&lower),
                    "{field:?} candidate '{label}' shortcut '{key}' collides with {RESERVED:?}"
                );
                assert!(
                    seen.insert(lower),
                    "{field:?} has duplicate choice shortcut '{key}' (on '{label}')"
                );
                total_shortcuts += 1;
            }
        }
        assert!(
            total_shortcuts >= 30,
            "expected at least 30 pattern draft choice shortcuts across DraftField::ALL, got {total_shortcuts}"
        );
    }
}
