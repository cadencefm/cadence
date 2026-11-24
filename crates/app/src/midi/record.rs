//! Live MIDI Note + CC + Project Macro automation recording (`R`).
use crate::*;
use cadence_core::midi_map::MidiFeedback;
use cadence_core::music::{midi_to_degree, note_name, ChordTone, MAX_CHORD_TONES};
use std::time::Instant;

fn trim_name(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        trimmed.to_string()
    } else {
        trimmed.chars().take(max_chars).collect()
    }
}

fn unique_label<'a>(base: &str, existing: impl Iterator<Item = &'a str>) -> String {
    crate::unique_name(base, existing, None)
}

impl App {
    /// Break the active MIDI CC recording undo-coalescing gesture (called on key press or record toggle).
    pub(crate) fn clear_midi_cc_rec_gesture(&mut self) {
        self.midi.feedback_cache.cc_rec.last_gesture_tick = None;
        self.doc.break_coalesce();
    }

    fn take_midi_coalesce_window(&mut self) -> bool {
        let now = Instant::now();
        let within_window = self
            .midi
            .feedback_cache
            .cc_rec
            .last_gesture_tick
            .is_some_and(|prev| now.duration_since(prev).as_millis() <= 1500);
        self.midi.feedback_cache.cc_rec.last_gesture_tick = Some(now);
        within_window
    }

    /// Most recently recorded live MIDI CC automation target label (while recording is armed).
    pub fn midi_cc_last_recorded(&self) -> Option<&str> {
        self.midi
            .record
            .then_some(self.midi.feedback_cache.cc_rec.last_target_label.as_deref())
            .flatten()
    }

    /// Toggle live MIDI note + CC automation recording (`R`): step-entry when stopped,
    /// quantized note overdub & CC automation recording when playing.
    pub fn toggle_midi_record(&mut self) -> Outcome {
        self.midi.record = !self.midi.record;
        self.midi.last_rec_step = None;
        self.clear_midi_cc_rec_gesture();
        if !self.midi.record {
            self.midi.feedback_cache.cc_rec.last_target_label = None;
        }
        self.status = if self.midi.record {
            if self.transport.playing {
                "● REC · NOTE+CC ON · play notes or move MIDI CCs to record automation".into()
            } else {
                "● MIDI STEP REC ON · play notes to enter · press Space to record CCs live".into()
            }
        } else {
            "MIDI Record off".into()
        };
        let mut out = Outcome::default();
        out.midi_feedback.push(MidiFeedback::Display {
            line1: if self.midi.record {
                "REC NOTE+CC ON"
            } else {
                "MIDI REC OFF"
            }
            .into(),
            line2: if self.transport.playing {
                "Live Notes + CC"
            } else {
                "Step Entry"
            }
            .into(),
            value_bar: None,
        });
        self.sync_midi_feedback(&mut out);
        out
    }

    /// Shared lazy CC automation lane builder used by both parameter CC recording and
    /// Project Macro CC recording (single point of truth).
    fn build_lazy_cc_record_edits(
        &mut self,
        mod_target: ModulationTarget,
        base_label: &str,
        norm: f32,
        direct_edit: impl FnOnce(bool) -> Option<Edit>,
    ) -> Result<(Id, u32, Vec<Edit>)> {
        let max_steps =
            (cadence_core::material::MAX_BARS * cadence_core::material::STEPS_PER_BAR) as u32;
        let default_len = (self.doc.project().arrangement_steps() as u32).clamp(16, max_steps);
        let song_step = self.song_step_heard();

        let existing_mod = self
            .doc
            .project()
            .modulations
            .iter()
            .find(|m| m.target == mod_target)
            .cloned();

        let existing_values_pat = existing_mod.as_ref().and_then(|m| {
            self.pattern(m.pattern).and_then(|p| match &p.kind {
                PatternKind::Values {
                    unit: ValueUnit::Normalized,
                    period,
                    ..
                } => Some((p.id, (*period).max(1))),
                _ => None,
            })
        });

        if let Some((pat_id, pat_len)) = existing_values_pat {
            let step = (song_step % u64::from(pat_len)) as u32;
            let mut edits = Vec::with_capacity(2);
            if let Some(edit) = direct_edit(true) {
                edits.push(edit);
            }
            edits.push(Edit::SetValue {
                pattern: pat_id,
                step,
                value: Some(norm),
            });
            Ok((pat_id, step, edits))
        } else {
            let step = (song_step % u64::from(default_len)) as u32;
            let fresh = self
                .doc
                .fresh_ids(if existing_mod.is_some() { 1 } else { 2 })?;
            let pat_id = fresh[0];
            let base = trim_name(base_label, 108);
            let pat_name = unique_label(
                &base,
                self.doc.project().patterns.iter().map(|p| p.name.as_str()),
            );
            let pattern = Pattern {
                id: pat_id,
                name: pat_name,
                kind: PatternKind::Values {
                    unit: ValueUnit::Normalized,
                    period: default_len,
                    mode: ValueMode::Ramp,
                    points: vec![ValuePoint { step, value: norm }],
                },
            };
            let modulation = if let Some(mut existing) = existing_mod {
                existing.pattern = pat_id;
                existing
            } else {
                let mod_id = fresh[1];
                let mod_name = unique_label(
                    &base,
                    self.doc
                        .project()
                        .modulations
                        .iter()
                        .map(|m| m.name.as_str()),
                );
                Modulation {
                    id: mod_id,
                    name: mod_name,
                    target: mod_target,
                    pattern: pat_id,
                }
            };
            let mut edits = Vec::with_capacity(3);
            if let Some(edit) = direct_edit(false) {
                edits.push(edit);
            }
            edits.push(Edit::PutPattern { pattern });
            edits.push(Edit::PutModulation { modulation });
            Ok((pat_id, step, edits))
        }
    }

    /// Record a live incoming MIDI CC (or PitchBend) parameter move into a `Values` modulation lane
    /// during playback. Lazily creates a `Values` pattern + `Modulation` if none exists, overdubs
    /// sorted points at `song_step_heard() % pat_len`, coalesces rapid consecutive ticks for
    /// atomic undo, and immediately refreshes live audio.
    pub(super) fn record_midi_cc_automation(
        &mut self,
        address: ParamAddress,
        norm: f32,
    ) -> Result<Option<(Outcome, String, String)>> {
        let Some(mod_target) = self.param_address_to_mod_target(address) else {
            return Ok(None);
        };
        let norm = norm.clamp(0.0, 1.0);
        let (native, short_name, mut val_str) =
            self.resolve_param_normalized_display(address, norm)?;
        let is_channel_gain = matches!(mod_target, ModulationTarget::ChannelGain { .. });
        if is_channel_gain {
            val_str = format!("{:.0}%", norm * 100.0);
        }
        let needs_gain_reset = if is_channel_gain {
            if let ParamAddress::Channel { channel, .. } = address {
                self.strip(channel)
                    .is_some_and(|s| (s.gain - 1.0).abs() > 1e-6)
            } else {
                false
            }
        } else {
            false
        };

        let (pat_id, step, edits) = self.build_lazy_cc_record_edits(
            mod_target,
            &format!("{short_name} CC"),
            norm,
            |existed| {
                if is_channel_gain {
                    (!existed || needs_gain_reset).then_some(Edit::SetParam {
                        address,
                        value: 1.0,
                    })
                } else {
                    Some(Edit::SetParam {
                        address,
                        value: native,
                    })
                }
            },
        )?;

        let label = format!("Rec CC {short_name} = {val_str} @ step {}", step + 1);
        self.midi.feedback_cache.cc_rec.last_target_label = Some(short_name.clone());
        let coalesce = self.take_midi_coalesce_window();
        let out = self.commit_coalesced_immediate(
            edits,
            label,
            document::CoalesceKey::MidiCcRecord {
                target: mod_target,
                pattern: pat_id,
            },
            coalesce,
        )?;
        Ok(Some((out, short_name, val_str)))
    }

    /// Apply a normalized `0.0..=1.0` value to Project Macro `index` with smooth undo coalescing.
    /// Returns `(Outcome, macro_name, formatted_pct)`.
    pub(crate) fn apply_macro_value_midi(
        &mut self,
        index: u8,
        norm: f32,
    ) -> Result<(Outcome, String, String)> {
        let value = norm.clamp(0.0, 1.0);
        let name = self.doc.project().macro_control(index).map_or_else(
            || cadence_core::macros::MacroControl::default_name(index),
            |m| m.name.clone(),
        );
        let val_str = format!("{:.0}%", value * 100.0);
        let label = format!("M{} {name} → {val_str}", usize::from(index) + 1);
        let coalesce = self.take_midi_coalesce_window();
        let out = self.commit_coalesced(
            vec![Edit::SetMacroValue { index, value }],
            label,
            document::CoalesceKey::Macro(index),
            coalesce,
        )?;
        Ok((out, name, val_str))
    }

    /// Record a live incoming MIDI CC (or PitchBend) move on Project Macro `index` (`0..8`)
    /// directly into a `Values` lane targeting `ModulationTarget::Macro { index }` with coalesced undo.
    pub(super) fn record_midi_macro_automation(
        &mut self,
        index: u8,
        norm: f32,
    ) -> Result<(Outcome, String, String)> {
        let norm = norm.clamp(0.0, 1.0);
        let mod_target = ModulationTarget::Macro { index };
        let short_name = self.doc.project().macro_control(index).map_or_else(
            || cadence_core::macros::MacroControl::default_name(index),
            |m| m.name.clone(),
        );
        let val_str = format!("{:.0}%", norm * 100.0);
        let (pat_id, step, edits) = self.build_lazy_cc_record_edits(
            mod_target,
            &format!("M{} {short_name} CC", usize::from(index) + 1),
            norm,
            |_| Some(Edit::SetMacroValue { index, value: norm }),
        )?;

        let label = format!(
            "Rec CC M{} {short_name} = {val_str} @ step {}",
            usize::from(index) + 1,
            step + 1
        );
        self.midi.feedback_cache.cc_rec.last_target_label =
            Some(format!("M{} {short_name}", usize::from(index) + 1));
        let coalesce = self.take_midi_coalesce_window();
        let out = self.commit_coalesced_immediate(
            edits,
            label,
            document::CoalesceKey::MidiCcRecord {
                target: mod_target,
                pattern: pat_id,
            },
            coalesce,
        )?;
        Ok((out, short_name, val_str))
    }

    /// Record a live MIDI NoteOn into the active literal note pattern (step-entry when stopped,
    /// quantized live recording with polyphonic chord stacking when playing).
    pub(super) fn record_midi_note(&mut self, midi: u8, velocity: u8) -> Result<(Outcome, String)> {
        let pat_id = self
            .selected_pattern
            .filter(|&id| {
                self.pattern(id)
                    .is_some_and(|p| matches!(p.kind, PatternKind::Notes { .. }))
            })
            .or_else(|| {
                // Fall back to the selected lane's first note pattern.
                let lane_id = self.selected_lane?;
                let lane = self.lane(lane_id)?;
                for lp in &lane.parts {
                    if let Some(part) = self.part(lp.part) {
                        if self
                            .pattern(part.pattern)
                            .is_some_and(|p| matches!(p.kind, PatternKind::Notes { .. }))
                        {
                            return Some(part.pattern);
                        }
                    }
                }
                None
            })
            .context("Select a literal Notes pattern for MIDI recording")?;

        let (pat_len, existing_steps) = match &self.pattern(pat_id).context("Pattern missing")?.kind
        {
            PatternKind::Notes { steps } => (steps.len().max(1), steps.clone()),
            _ => bail!("Pattern is not editable notes"),
        };

        let project = self.doc.project();
        let (degree, accidental) = midi_to_degree(project.root_midi, &project.scale, midi, 0);
        let vel = (f32::from(velocity) / 127.0).clamp(0.05, 1.0);

        let (step_idx, step_value, label) = if !self.transport.playing {
            let idx = self.cursor % pat_len;
            let step = Step {
                active: true,
                degree,
                accidental,
                velocity: vel,
                gate: 0.75,
                condition: cadence_core::music::StepCondition::ALWAYS,
                chord: Vec::new(),
                source_part: None,
            };
            self.cursor = (idx + 1) % pat_len;
            (
                idx,
                step,
                format!("Step {}/{pat_len} ← {}", idx + 1, note_name(midi)),
            )
        } else {
            let song_step = self.song_step_heard();
            let idx = (song_step as usize) % pat_len;
            let mut step = existing_steps.get(idx).cloned().unwrap_or_default();
            let same_live_step = self.midi.last_rec_step == Some(song_step) && step.active;
            if same_live_step
                && (step.degree != degree || step.accidental != accidental)
                && !step
                    .chord
                    .iter()
                    .any(|c| c.degree == degree && c.accidental == accidental)
                && step.chord.len() < MAX_CHORD_TONES
            {
                step.chord.push(ChordTone {
                    degree,
                    accidental,
                    velocity: vel,
                    gate: 0.75,
                });
            } else {
                step = Step {
                    active: true,
                    degree,
                    accidental,
                    velocity: vel,
                    gate: 0.75,
                    condition: cadence_core::music::StepCondition::ALWAYS,
                    chord: Vec::new(),
                    source_part: None,
                };
            }
            self.midi.last_rec_step = Some(song_step);
            (
                idx,
                step,
                format!("Rec step {} ← {}", idx + 1, note_name(midi)),
            )
        };

        let mut out = self.apply(Action::Edit {
            edit: Edit::SetNote {
                pattern: pat_id,
                step: step_idx,
                value: step_value,
            },
        })?;
        self.status = label.clone();
        out.midi_feedback.push(MidiFeedback::Display {
            line1: "MIDI Record".into(),
            line2: label.clone(),
            value_bar: Some(velocity),
        });
        Ok((out, label))
    }
}
