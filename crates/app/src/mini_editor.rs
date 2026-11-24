//! Dedicated Mini-Notation Editor modal (`Mode::MiniEditor`) with live keystroke parse
//! validation via `cadence_core::mini::parse_mini` and sub-step event count preview.
use super::*;
use cadence_core::evaluate::EvaluatedEvent;
use cadence_core::mini::{evaluate_mini_into, parse_mini};
use cadence_core::music::{note_name, pitch_midi};

#[derive(Clone, Debug)]
pub struct MiniEditorView {
    pub pattern_name: String,
    pub steps: u16,
    pub code: String,
    pub valid: bool,
    pub event_count: usize,
    pub preview_summary: String,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct MiniEditorState {
    pub pattern_id: Id,
    pub pattern_name: String,
    pub steps: u16,
    pub code: String,
    pub valid: bool,
    pub event_count: usize,
    pub preview_summary: String,
    pub error: Option<String>,
}

impl App {
    pub(crate) fn open_mini_editor(&mut self, pattern_id: Id) -> Result<Outcome> {
        let pat = self
            .pattern(pattern_id)
            .context("Selected pattern is missing")?;
        let PatternKind::Mini { code, steps } = &pat.kind else {
            bail!("Pattern '{}' is not a Mini-Notation pattern", pat.name);
        };
        let mut state = MiniEditorState {
            pattern_id,
            pattern_name: pat.name.clone(),
            steps: *steps,
            code: code.clone(),
            valid: true,
            event_count: 0,
            preview_summary: String::new(),
            error: None,
        };
        self.recompute_mini_preview(&mut state);
        self.modal = ActiveModal::MiniEditor(Box::new(state));
        self.input.count = None;
        self.input.mode = Mode::MiniEditor;
        self.status =
            "Mini-Notation Editor · type code · Enter commits when valid · Esc cancels".into();
        Ok(self.release_held())
    }

    /// Open Mini-Notation Editor for the currently selected Mini pattern, or the first Mini pattern
    /// in the project if another pattern is currently selected.
    pub(crate) fn open_active_mini_editor(&mut self) -> Result<Outcome> {
        if let Some(pat) = self.selected_pattern_object() {
            if matches!(pat.kind, PatternKind::Mini { .. }) {
                let id = pat.id;
                return self.open_mini_editor(id);
            }
        }
        if let Some(id) = self
            .doc
            .project()
            .patterns
            .iter()
            .find(|p| matches!(p.kind, PatternKind::Mini { .. }))
            .map(|p| p.id)
        {
            self.select_pattern(id)?;
            return self.open_mini_editor(id);
        }
        self.status = "No Mini-Notation pattern in project · n → m creates one".into();
        Ok(Outcome::default())
    }

    fn recompute_mini_preview(&self, state: &mut MiniEditorState) {
        match parse_mini(&state.code) {
            Ok(nodes) => {
                let project = self.doc.project();
                let mut total_events = 0usize;
                let mut preview_notes: Vec<String> = Vec::new();
                let mut buf: Vec<EvaluatedEvent> = Vec::new();
                for step in 0..i64::from(state.steps) {
                    buf.clear();
                    evaluate_mini_into(
                        &nodes,
                        state.steps,
                        state.pattern_id,
                        step,
                        |midi| project.midi_to_degree(midi),
                        &mut buf,
                    );
                    total_events += buf.len();
                    for ev in &buf {
                        if preview_notes.len() < 12 {
                            let midi = pitch_midi(
                                project.root_midi,
                                &project.scale,
                                ev.step.degree,
                                ev.step.accidental,
                                0,
                            );
                            preview_notes.push(note_name(midi));
                        }
                    }
                }
                state.valid = true;
                state.event_count = total_events;
                state.error = None;
                state.preview_summary = if preview_notes.is_empty() {
                    "All rests in cycle 1".into()
                } else {
                    let suffix = if total_events > preview_notes.len() {
                        " …"
                    } else {
                        ""
                    };
                    format!("{}{suffix}", preview_notes.join(" "))
                };
            }
            Err(err) => {
                state.valid = false;
                state.event_count = 0;
                state.preview_summary.clear();
                state.error = Some(format!("{err:#}"));
            }
        }
    }

    pub fn mini_editor_view(&self) -> Option<MiniEditorView> {
        let state = self
            .modal
            .mini_editor()
            .filter(|_| self.input.mode == Mode::MiniEditor)?;
        Some(MiniEditorView {
            pattern_name: state.pattern_name.clone(),
            steps: state.steps,
            code: state.code.clone(),
            valid: state.valid,
            event_count: state.event_count,
            preview_summary: state.preview_summary.clone(),
            error: state.error.clone(),
        })
    }

    pub(super) fn handle_mini_editor(&mut self, key: KeyEvent) -> Result<Outcome> {
        let repeat = key.kind == KeyEventKind::Repeat;
        if key.code == KeyCode::Esc {
            self.modal = ActiveModal::None;
            self.input.mode = Mode::Normal;
            self.status = "Mini-Notation edit cancelled".into();
            return Ok(Outcome::default());
        }
        if key.code == KeyCode::Enter {
            if repeat {
                return Ok(Outcome::default());
            }
            let Some(state) = self.modal.mini_editor().cloned() else {
                self.input.mode = Mode::Normal;
                return Ok(Outcome::default());
            };
            if !state.valid {
                if let Some(err) = &state.error {
                    self.status = format!("Fix syntax error before committing: {err}");
                }
                return Ok(Outcome::default());
            }
            let mut pattern = self
                .pattern(state.pattern_id)
                .context("Mini pattern no longer exists")?
                .clone();
            pattern.kind = PatternKind::Mini {
                code: state.code.trim().to_string(),
                steps: state.steps,
            };
            let name = pattern.name.clone();
            let out = self.commit(
                vec![Edit::PutPattern { pattern }],
                format!("Updated mini-notation on '{name}'"),
            )?;
            self.modal = ActiveModal::None;
            self.input.mode = Mode::Normal;
            return Ok(out);
        }

        let Some(mut state) = self.modal.take_mini_editor() else {
            return Ok(Outcome::default());
        };
        let mut mutated = false;
        match key.code {
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.code.clear();
                mutated = true;
            }
            KeyCode::Backspace => {
                state.code.pop();
                mutated = true;
            }
            KeyCode::Char(c)
                if !c.is_control()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && state.code.len() < 1024 =>
            {
                state.code.push(c);
                mutated = true;
            }
            _ => {}
        }
        if mutated {
            self.recompute_mini_preview(&mut state);
        }
        self.modal = ActiveModal::MiniEditor(state);
        Ok(Outcome::default())
    }
}
