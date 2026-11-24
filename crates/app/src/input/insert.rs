//! Step Insert mode (`i` on editable Pattern Notes): piano-keyboard and scale-degree step entry.
use super::keyboard::{midi_note, note_key, octave_step, shift_octave, ACCENT_VELOCITY};
use super::*;
use crate::editing::{step_voices_sorted, StepVoiceSlot};

impl App {
    pub(crate) fn handle_insert(&mut self, key: KeyEvent) -> Result<Outcome> {
        if key.kind == KeyEventKind::Repeat {
            return Ok(Outcome::default());
        }
        if let KeyCode::Char(c) = key.code {
            if let Some(step) = octave_step(c) {
                self.input.octave = shift_octave(self.input.octave, step);
                self.status = format!("Octave {}", self.input.octave);
                return Ok(Outcome::default());
            }
        }
        let mut value = self.current_note()?;
        let (mut audition, mut audition_vel) = (false, value.velocity);
        let midi = match key.code {
            KeyCode::Char('.') => {
                value = Step::default();
                0
            }
            KeyCode::Char(c) if ('1'..='8').contains(&c) => {
                let deg = c.to_digit(10).unwrap() as i16;
                if value.active && !value.chord.is_empty() {
                    let voices = step_voices_sorted(self.doc.project(), &value);
                    let slot = voices
                        .get(self.note_voice.min(voices.len().saturating_sub(1)))
                        .map_or(StepVoiceSlot::Root, |v| v.slot);
                    match slot {
                        StepVoiceSlot::Root => {
                            value.degree = deg;
                            value.accidental = 0;
                            audition_vel = value.velocity;
                        }
                        StepVoiceSlot::Chord(idx) if idx < value.chord.len() => {
                            value.chord[idx].degree = deg;
                            value.chord[idx].accidental = 0;
                            audition_vel = value.chord[idx].velocity;
                        }
                        _ => {}
                    }
                } else {
                    value.active = true;
                    value.degree = deg;
                    value.accidental = 0;
                    value.chord.clear();
                    audition_vel = value.velocity;
                }
                audition = true;
                self.doc.project().pitch_midi(deg, 0, 0)
            }
            KeyCode::Char(c) => {
                let Some((semitone, accent)) = note_key(c) else {
                    return Ok(Outcome::default());
                };
                let midi = midi_note(self.input.octave, semitone);
                let (degree, accidental) = self.doc.project().midi_to_degree(midi);
                if value.active && !value.chord.is_empty() {
                    let voices = step_voices_sorted(self.doc.project(), &value);
                    let slot = voices
                        .get(self.note_voice.min(voices.len().saturating_sub(1)))
                        .map_or(StepVoiceSlot::Root, |v| v.slot);
                    match slot {
                        StepVoiceSlot::Root => {
                            value.degree = degree;
                            value.accidental = accidental;
                            if accent {
                                value.velocity = ACCENT_VELOCITY;
                            }
                            audition_vel = value.velocity;
                        }
                        StepVoiceSlot::Chord(idx) if idx < value.chord.len() => {
                            value.chord[idx].degree = degree;
                            value.chord[idx].accidental = accidental;
                            if accent {
                                value.chord[idx].velocity = ACCENT_VELOCITY;
                            }
                            audition_vel = value.chord[idx].velocity;
                        }
                        _ => {}
                    }
                } else {
                    if accent {
                        value.velocity = ACCENT_VELOCITY;
                    }
                    value.active = true;
                    value.degree = degree;
                    value.accidental = accidental;
                    value.chord.clear();
                    audition_vel = value.velocity;
                }
                audition = true;
                midi
            }
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                self.clamp_note_voice();
                return Ok(Outcome::default());
            }
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(self.current_length().saturating_sub(1));
                self.clamp_note_voice();
                return Ok(Outcome::default());
            }
            _ => return Ok(Outcome::default()),
        };
        let edit = self.edits_for_current_note(value)?;
        let mut out = self.apply(Action::Edit { edit })?;
        if audition {
            self.attach_note_audition(&mut out, midi, audition_vel);
        }
        self.cursor = (self.cursor + 1) % self.current_length().max(1);
        self.clamp_note_voice();
        self.input.mode = Mode::Insert;
        Ok(out)
    }
}
