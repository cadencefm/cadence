//! Step & 2D polyphonic voice edits at the cursor and repeat.
//!
//! Single point of truth for visual chord-voice sorting (`0` = lowest pitch .. `N-1` = highest
//! pitch), per-voice pitch/accidental/gate/velocity editing, chord stacking (`o`), single-voice
//! deletion (`x`), and whole-step clear (`X`).
use super::*;
use cadence_core::music::{note_name, MAX_CHORD_TONES, MAX_DEGREE, MIN_DEGREE};

/// Authoritative location of one voice inside a [`Step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StepVoiceSlot {
    Root,
    Chord(usize),
}

impl StepVoiceSlot {
    fn sort_key(self) -> usize {
        match self {
            Self::Root => 0,
            Self::Chord(i) => i + 1,
        }
    }
}

/// Visual pitch-ordered snapshot of one note voice inside a [`Step`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StepVoiceInfo {
    pub slot: StepVoiceSlot,
    pub degree: i16,
    pub accidental: i8,
    pub midi: u8,
    pub velocity: f32,
    pub gate: f32,
}

/// Return all sounding voices of `step` sorted visually from lowest pitch (`0`) to highest (`N-1`),
/// using a deterministic tie-breaker (`Root`, then `Chord(0..)`).
pub(crate) fn step_voices_sorted(project: &Project, step: &Step) -> Vec<StepVoiceInfo> {
    if !step.active {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(1 + step.chord.len());
    out.push(StepVoiceInfo {
        slot: StepVoiceSlot::Root,
        degree: step.degree,
        accidental: step.accidental,
        midi: project.pitch_midi(step.degree, step.accidental, 0),
        velocity: step.velocity,
        gate: step.gate,
    });
    for (idx, tone) in step.chord.iter().enumerate() {
        out.push(StepVoiceInfo {
            slot: StepVoiceSlot::Chord(idx),
            degree: tone.degree,
            accidental: tone.accidental,
            midi: project.pitch_midi(tone.degree, tone.accidental, 0),
            velocity: tone.velocity,
            gate: tone.gate,
        });
    }
    out.sort_by_key(|v| (v.midi, v.degree, v.accidental, v.slot.sort_key()));
    out
}

/// Canonical header/status readout for the focused voice on a step
/// (e.g., `Voice 2/4 · A♭4 · len 1.50 · vel 88%`).
pub(crate) fn format_voice_readout(voices: &[StepVoiceInfo], selected: usize) -> Option<String> {
    if voices.is_empty() {
        return None;
    }
    let idx = selected.min(voices.len() - 1);
    let v = voices[idx];
    Some(format!(
        "Voice {}/{} · {} · len {:.2} · vel {:.0}%",
        idx + 1,
        voices.len(),
        note_name(v.midi),
        v.gate,
        v.velocity * 100.0
    ))
}

impl App {
    pub(super) fn edits_for_current_note(&self, value: Step) -> Result<Edit> {
        match self.scope {
            EditScope::Definition => {
                let id = self.selected_pattern.context("Select a pattern")?;
                ensure!(
                    matches!(
                        self.pattern(id).map(|p| &p.kind),
                        Some(PatternKind::Notes { .. })
                    ),
                    "Derived material is a preview: open a source (Enter/gs), open local occurrence (o from Placement), or freeze (F)"
                );
                Ok(Edit::SetNote {
                    pattern: id,
                    step: self.cursor,
                    value,
                })
            }
            EditScope::Placement(placement) => {
                // This is one occurrence; source conditions were evaluated at its clock.
                let mut value = value;
                value.condition = cadence_core::music::StepCondition::ALWAYS;
                Ok(Edit::SetException {
                    placement,
                    step: u32::try_from(self.cursor).context("Placement step is out of range")?,
                    value: Some(value),
                })
            }
        }
    }

    pub(super) fn current_note(&self) -> Result<Step> {
        match self.current_value(self.cursor)? {
            PatternValue::Notes(step) => Ok(step),
            _ => bail!("Select note material"),
        }
    }

    pub(crate) fn can_edit_current_pattern_notes(&self) -> bool {
        let Some(id) = self.selected_pattern else {
            return false;
        };
        match self.scope {
            EditScope::Definition => self
                .pattern(id)
                .is_some_and(|p| matches!(p.kind, PatternKind::Notes { .. })),
            EditScope::Placement(_) => self
                .doc
                .project()
                .pattern_type(id)
                .is_ok_and(|pt| pt == PatternType::Notes),
        }
    }

    /// Sorted voices at the current Pattern step cursor (`0` = lowest .. `N-1` = highest).
    pub(crate) fn current_step_voices(&self) -> Vec<StepVoiceInfo> {
        let Ok(step) = self.current_note() else {
            return Vec::new();
        };
        step_voices_sorted(self.doc.project(), &step)
    }

    /// Clamp `self.note_voice` cleanly to the voices present at `self.cursor`.
    pub(crate) fn clamp_note_voice(&mut self) {
        let count = self.current_step_voices().len();
        self.note_voice = self.note_voice.min(count.saturating_sub(1));
    }

    /// Move `self.note_voice` visually up (`delta > 0`) or down (`delta < 0`) through the chord stack.
    pub(crate) fn step_note_voice(&mut self, delta: isize, wrap: bool) {
        let voices = self.current_step_voices();
        if voices.is_empty() {
            self.note_voice = 0;
            self.status = format!("Step {} · empty", self.cursor + 1);
            return;
        }
        let cur = self.note_voice.min(voices.len() - 1) as isize;
        let next = if wrap {
            (cur + delta).rem_euclid(voices.len() as isize) as usize
        } else {
            (cur + delta).clamp(0, voices.len() as isize - 1) as usize
        };
        self.note_voice = next;
        if let Some(readout) = format_voice_readout(&voices, next) {
            self.status = format!("Step {} · {readout}", self.cursor + 1);
        }
    }

    /// Audition a single MIDI note through the selected instrument (or preview rack).
    pub(crate) fn attach_note_audition(&self, out: &mut Outcome, midi: u8, velocity: f32) {
        if let Some(id) = self.selected_instrument {
            out.audition_instrument = Some((id, midi, velocity));
        } else {
            out.rack_audition = Some((self.preview_patch.clone(), midi, velocity));
        }
    }

    fn selected_slot_in(&self, step: &Step) -> Option<StepVoiceSlot> {
        let voices = step_voices_sorted(self.doc.project(), step);
        if voices.is_empty() {
            return None;
        }
        let idx = self.note_voice.min(voices.len() - 1);
        Some(voices[idx].slot)
    }

    fn retrack_slot(&mut self, step: &Step, slot: StepVoiceSlot) {
        let voices = step_voices_sorted(self.doc.project(), step);
        if let Some(pos) = voices.iter().position(|v| v.slot == slot) {
            self.note_voice = pos;
        } else {
            self.note_voice = self.note_voice.min(voices.len().saturating_sub(1));
        }
        if let Some(readout) = format_voice_readout(&voices, self.note_voice) {
            self.status = format!("Step {} · {readout}", self.cursor + 1);
        }
    }

    /// Delete ONLY the selected voice at `self.cursor` (`x`).
    /// If root is deleted and chord tones remain, promotes the next chord tone cleanly to root.
    pub(crate) fn delete_selected_voice(&mut self) -> Result<Outcome> {
        let mut step = self.current_note()?;
        if !step.active {
            self.note_voice = 0;
            self.status = format!("Step {} is already empty", self.cursor + 1);
            return Ok(Outcome::default());
        }
        let Some(slot) = self.selected_slot_in(&step) else {
            return Ok(Outcome::default());
        };
        match slot {
            StepVoiceSlot::Chord(idx) if idx < step.chord.len() => {
                step.chord.remove(idx);
            }
            StepVoiceSlot::Root => {
                if !step.chord.is_empty() {
                    let promoted = step.chord.remove(0);
                    step.degree = promoted.degree;
                    step.accidental = promoted.accidental;
                    step.velocity = promoted.velocity;
                    step.gate = promoted.gate;
                } else {
                    step.active = false;
                    step.chord.clear();
                }
            }
            _ => {}
        }
        let edit = self.edits_for_current_note(step.clone())?;
        let out = self.apply(Action::Edit { edit })?;
        let voices = step_voices_sorted(self.doc.project(), &step);
        self.note_voice = self.note_voice.min(voices.len().saturating_sub(1));
        self.status = format_voice_readout(&voices, self.note_voice).map_or_else(
            || format!("Step {} cleared", self.cursor + 1),
            |r| format!("Deleted voice · {r}"),
        );
        Ok(out)
    }

    /// Clear the entire current step (`X`): deactivates root and clears all chord tones.
    pub(crate) fn clear_current_step(&mut self) -> Result<Outcome> {
        let mut step = self.current_note()?;
        step.active = false;
        step.chord.clear();
        let edit = self.edits_for_current_note(step)?;
        let out = self.apply(Action::Edit { edit })?;
        self.note_voice = 0;
        self.status = format!("Cleared step {}", self.cursor + 1);
        Ok(out)
    }

    /// Add/stack a new chord voice on the current step (`o`) while staying in `Mode::Normal`.
    /// Defaults to root degree `1` on an empty step, or `+2` scale degrees above the highest voice.
    pub(crate) fn stack_chord_voice(&mut self) -> Result<Outcome> {
        ensure!(
            self.can_edit_current_pattern_notes(),
            "Open a literal note source (Enter/gs) or local occurrence (o from Placement)"
        );
        let mut step = self.current_note()?;
        let (target_slot, new_midi, new_vel) = if !step.active {
            step.active = true;
            step.degree = 1;
            step.accidental = 0;
            step.velocity = 0.8;
            step.gate = 0.75;
            step.chord.clear();
            let midi = self.doc.project().pitch_midi(step.degree, 0, 0);
            (StepVoiceSlot::Root, midi, step.velocity)
        } else {
            ensure!(
                step.chord.len() < MAX_CHORD_TONES,
                "Step already has maximum {} chord voices",
                MAX_CHORD_TONES + 1
            );
            let voices = step_voices_sorted(self.doc.project(), &step);
            let highest = voices
                .last()
                .copied()
                .context("Active step has no voices")?;
            let degree = highest
                .degree
                .saturating_add(2)
                .clamp(MIN_DEGREE, MAX_DEGREE);
            let tone = ChordTone {
                degree,
                accidental: 0,
                velocity: highest.velocity,
                gate: highest.gate,
            };
            step.chord.push(tone);
            let slot = StepVoiceSlot::Chord(step.chord.len() - 1);
            let midi = self
                .doc
                .project()
                .pitch_midi(tone.degree, tone.accidental, 0);
            (slot, midi, tone.velocity)
        };
        let edit = self.edits_for_current_note(step.clone())?;
        let mut out = self.apply(Action::Edit { edit })?;
        self.attach_note_audition(&mut out, new_midi, new_vel);
        self.retrack_slot(&step, target_slot);
        Ok(out)
    }

    fn commit_step_coalesced(&mut self, step: Step, coalesce: bool) -> Result<Outcome> {
        let edit = self.edits_for_current_note(step)?;
        let pattern = self.selected_pattern.unwrap_or(0);
        let label = format!("Edited step {}", self.cursor + 1);
        self.commit_coalesced(
            vec![edit],
            label,
            document::CoalesceKey::PatternStep {
                pattern,
                scope: self.scope,
                step: self.cursor,
            },
            coalesce,
        )
    }

    /// Shift ONLY the selected voice's scale degree by `degree_delta` (`Alt+j/k` ±1, `Alt+J/K` ±1 oct).
    pub(crate) fn edit_selected_voice_pitch(
        &mut self,
        degree_delta: i16,
        coalesce: bool,
    ) -> Result<Outcome> {
        let mut step = self.current_note()?;
        let slot = if !step.active {
            step.active = true;
            step.degree = 1i16
                .checked_add(degree_delta)
                .unwrap_or(1)
                .clamp(MIN_DEGREE, MAX_DEGREE);
            step.accidental = 0;
            step.chord.clear();
            StepVoiceSlot::Root
        } else {
            let Some(slot) = self.selected_slot_in(&step) else {
                return Ok(Outcome::default());
            };
            match slot {
                StepVoiceSlot::Root => {
                    step.degree = step
                        .degree
                        .checked_add(degree_delta)
                        .context("Degree overflow")?
                        .clamp(MIN_DEGREE, MAX_DEGREE);
                }
                StepVoiceSlot::Chord(idx) => {
                    let tone = step.chord.get_mut(idx).context("Chord voice missing")?;
                    tone.degree = tone
                        .degree
                        .checked_add(degree_delta)
                        .context("Degree overflow")?
                        .clamp(MIN_DEGREE, MAX_DEGREE);
                }
            }
            slot
        };
        let (midi, vel) = match slot {
            StepVoiceSlot::Root => (
                self.doc
                    .project()
                    .pitch_midi(step.degree, step.accidental, 0),
                step.velocity,
            ),
            StepVoiceSlot::Chord(idx) => {
                let t = step.chord[idx];
                (
                    self.doc.project().pitch_midi(t.degree, t.accidental, 0),
                    t.velocity,
                )
            }
        };
        let mut out = self.commit_step_coalesced(step.clone(), coalesce)?;
        self.attach_note_audition(&mut out, midi, vel);
        self.retrack_slot(&step, slot);
        Ok(out)
    }

    /// Adjust ONLY the selected voice's accidental (`[` / `]` / `#`, clamped to `-1..=+1`).
    pub(crate) fn edit_selected_voice_accidental(&mut self, delta: i8) -> Result<Outcome> {
        let mut step = self.current_note()?;
        if !step.active {
            step.active = true;
            step.degree = 1;
            step.accidental = delta.clamp(-1, 1);
            step.chord.clear();
            let midi = self
                .doc
                .project()
                .pitch_midi(step.degree, step.accidental, 0);
            let vel = step.velocity;
            let edit = self.edits_for_current_note(step.clone())?;
            let mut out = self.apply(Action::Edit { edit })?;
            self.attach_note_audition(&mut out, midi, vel);
            self.retrack_slot(&step, StepVoiceSlot::Root);
            return Ok(out);
        }
        let Some(slot) = self.selected_slot_in(&step) else {
            return Ok(Outcome::default());
        };
        let (midi, vel) = match slot {
            StepVoiceSlot::Root => {
                step.accidental = (step.accidental + delta).clamp(-1, 1);
                (
                    self.doc
                        .project()
                        .pitch_midi(step.degree, step.accidental, 0),
                    step.velocity,
                )
            }
            StepVoiceSlot::Chord(idx) => {
                let tone = step.chord.get_mut(idx).context("Chord voice missing")?;
                tone.accidental = (tone.accidental + delta).clamp(-1, 1);
                (
                    self.doc
                        .project()
                        .pitch_midi(tone.degree, tone.accidental, 0),
                    tone.velocity,
                )
            }
        };
        let edit = self.edits_for_current_note(step.clone())?;
        let mut out = self.apply(Action::Edit { edit })?;
        self.attach_note_audition(&mut out, midi, vel);
        self.retrack_slot(&step, slot);
        Ok(out)
    }

    /// Cycle step trigger condition (`c` / `C`) on a literal Pattern Notes definition step.
    pub(crate) fn cycle_step_condition(&mut self, delta: i8) -> Result<Outcome> {
        ensure!(
            self.scope == EditScope::Definition,
            "Step conditions apply to shared Pattern definitions"
        );
        let mut step = self.current_note()?;
        if !step.active {
            self.status = "Empty step · press o or i to add a note first".into();
            return Ok(Outcome::default());
        }
        step.condition = step.condition.cycle(delta);
        let cond_label = if step.condition.is_always() {
            "Always (1:1)".to_string()
        } else {
            step.condition.label()
        };
        let edit = self.edits_for_current_note(step)?;
        let out = self.apply(Action::Edit { edit })?;
        self.status = format!("Step {} condition → {cond_label}", self.cursor + 1);
        Ok(out)
    }

    /// Re-inherit the focused step from the underlying pattern definition (`Backspace` in `EditScope::Placement`).
    pub(crate) fn clear_placement_step_exception(&mut self) -> Result<Outcome> {
        let EditScope::Placement(placement_id) = self.scope else {
            return Ok(Outcome::default());
        };
        let (_, place) = self.placement(placement_id).context("Placement missing")?;
        let step = self.cursor as u32;
        if !place.exceptions.iter().any(|e| e.step == step) {
            self.status = format!(
                "Step {} already inherits from pattern definition · Esc returns",
                self.cursor + 1
            );
            return Ok(Outcome::default());
        }
        let out = self.apply(Action::Edit {
            edit: Edit::SetException {
                placement: placement_id,
                step,
                value: None,
            },
        })?;
        self.status = format!(
            "Re-inherited step {} from pattern definition · u restores",
            self.cursor + 1
        );
        Ok(out)
    }

    /// Adjust ONLY the selected voice's gate length (`Alt+h/l` ±0.25, `Alt+H/L` ±1.0).
    pub(crate) fn edit_selected_voice_gate(
        &mut self,
        delta_steps: f32,
        coalesce: bool,
    ) -> Result<Outcome> {
        let mut step = self.current_note()?;
        if !step.active {
            self.status = "Empty step · press o or i to add a note first".into();
            return Ok(Outcome::default());
        }
        let Some(slot) = self.selected_slot_in(&step) else {
            return Ok(Outcome::default());
        };
        match slot {
            StepVoiceSlot::Root => {
                step.gate = (((step.gate + delta_steps) * 100.0).round() / 100.0)
                    .clamp(param_ops::MIN_STEP_GATE, param_ops::MAX_STEP_GATE);
            }
            StepVoiceSlot::Chord(idx) => {
                let tone = step.chord.get_mut(idx).context("Chord voice missing")?;
                tone.gate = (((tone.gate + delta_steps) * 100.0).round() / 100.0)
                    .clamp(param_ops::MIN_STEP_GATE, param_ops::MAX_STEP_GATE);
            }
        }
        let out = self.commit_step_coalesced(step.clone(), coalesce)?;
        self.retrack_slot(&step, slot);
        Ok(out)
    }

    /// Adjust ONLY the selected voice's velocity (`,/.` ±5%, `</>` ±15%).
    pub(crate) fn edit_selected_voice_velocity(
        &mut self,
        delta: f32,
        coalesce: bool,
    ) -> Result<Outcome> {
        let mut step = self.current_note()?;
        if !step.active {
            self.status = "Empty step · press o or i to add a note first".into();
            return Ok(Outcome::default());
        }
        let Some(slot) = self.selected_slot_in(&step) else {
            return Ok(Outcome::default());
        };
        match slot {
            StepVoiceSlot::Root => {
                step.velocity =
                    (((step.velocity + delta) * 100.0).round() / 100.0).clamp(0.05, 1.0);
            }
            StepVoiceSlot::Chord(idx) => {
                let tone = step.chord.get_mut(idx).context("Chord voice missing")?;
                tone.velocity =
                    (((tone.velocity + delta) * 100.0).round() / 100.0).clamp(0.05, 1.0);
            }
        }
        let out = self.commit_step_coalesced(step.clone(), coalesce)?;
        self.retrack_slot(&step, slot);
        Ok(out)
    }

    /// Toggle rest/active state on the current step (`m` on Notes, or rhythm toggle).
    pub(crate) fn toggle_material(&mut self) -> Result<Outcome> {
        let id = self.selected_pattern.context("Select material")?;
        match self.pattern(id).context("Pattern missing")?.kind.clone() {
            PatternKind::Rhythm { steps } => {
                ensure!(!steps.is_empty(), "Rhythm pattern is empty");
                let idx = self.cursor % steps.len();
                self.apply(Action::Edit {
                    edit: Edit::SetRhythm {
                        pattern: id,
                        step: idx,
                        active: !steps[idx],
                    },
                })
            }
            PatternKind::Values { .. } => self.apply(Action::Edit {
                edit: Edit::SetValue {
                    pattern: id,
                    step: self.cursor as u32,
                    value: None,
                },
            }),
            _ => {
                let mut value = self.current_note()?;
                value.active = !value.active;
                let edit = self.edits_for_current_note(value.clone())?;
                let out = self.apply(Action::Edit { edit })?;
                self.clamp_note_voice();
                Ok(out)
            }
        }
    }

    pub(super) fn repeat_last(&mut self) -> Result<Outcome> {
        let edits = self.last_edit.clone().context("No edit to repeat")?;
        let Ok([single]) = <[Edit; 1]>::try_from(edits) else {
            bail!("Creation and compound transactions are not blindly repeated");
        };
        let edit = match single {
            Edit::SetNote { value, .. } => self.edits_for_current_note(value)?,
            Edit::SetException { value, .. } => {
                let EditScope::Placement(placement) = self.scope else {
                    bail!("Repeat this exception in placement scope")
                };
                Edit::SetException {
                    placement,
                    step: self.cursor as u32,
                    value,
                }
            }
            Edit::SetValue { value, .. } => Edit::SetValue {
                pattern: self.selected_pattern.context("Select values")?,
                step: self.cursor as u32,
                value,
            },
            Edit::SetRhythm { active, .. } => Edit::SetRhythm {
                pattern: self.selected_pattern.context("Select rhythm")?,
                step: self.cursor,
                active,
            },
            _ => bail!("This object edit has no safe retargeting rule"),
        };
        self.apply(Action::Edit { edit })
    }
}
