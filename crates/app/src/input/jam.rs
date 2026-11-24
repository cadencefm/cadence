//! Jam: the computer keyboard as an instrument. The number row moves the song to a section,
//! the q row names lanes and the bottom two rows play notes. Jam is the Alt
//! layer locked on: the same section and lane keys work from every mode with Alt.
use super::keyboard::{midi_note, note_key, octave_step, shift_octave, ACCENT_VELOCITY, VELOCITY};
use super::*;

impl App {
    pub fn is_jam(&self) -> bool {
        self.input.mode == Mode::Jam
    }

    /// The lane whose layer is open after a lane key.
    pub fn lane_layer(&self) -> Option<Id> {
        self.input.lane_layer
    }

    /// The lane performance keys act on: the last one named with a lane key,
    /// otherwise the selected lane.
    pub fn focused_lane(&self) -> Option<Id> {
        self.input.focused_lane.or(self.selected_lane)
    }

    /// The octave the bottom-row notes play in.
    pub fn note_octave(&self) -> i8 {
        self.input.octave
    }

    pub(crate) fn enter_jam(&mut self) -> Outcome {
        self.dismiss_transient_modals();
        self.input.mode = Mode::Jam;
        self.input.count = None;
        self.input.lane_layer = None;
        // Notes go to what you are on until a lane key names a lane.
        self.input.focused_lane = None;
        self.status = match self.jam_instrument().and_then(|id| self.instrument(id)) {
            Some(instrument) => format!("Jam · notes play {}", instrument.name),
            None => "Jam".into(),
        };
        Outcome::default()
    }

    pub(crate) fn leave_jam(&mut self) -> Outcome {
        self.input.mode = Mode::Normal;
        self.input.lane_layer = None;
        self.status = "Left Jam".into();
        self.release_held()
    }

    pub(crate) fn handle_jam(&mut self, key: KeyEvent, sustain_notes: bool) -> Result<Outcome> {
        if key.kind == KeyEventKind::Repeat {
            return Ok(Outcome::default());
        }
        match key.code {
            KeyCode::Esc => Ok(self.leave_jam()),
            KeyCode::Char(' ') => self.apply(Action::Play {
                playing: !self.transport.playing,
            }),
            KeyCode::Backspace => self.apply(Action::CancelPending { all: false }),
            KeyCode::Char('\\') => self.toggle_song_loop(),
            KeyCode::Enter => match self.staging.as_deref() {
                None => Ok(Outcome::default()),
                Some([]) => {
                    let out = self.apply(Action::FinishStaging {
                        choice: StageChoice::Discard,
                    })?;
                    self.status = "Exited staging".into();
                    Ok(out)
                }
                Some(_) => self.apply(Action::FinishStaging {
                    choice: StageChoice::Fire,
                }),
            },
            KeyCode::BackTab => {
                self.input.bank = self.input.bank.saturating_sub(1);
                self.status = format!("Bank {}", self.input.bank + 1);
                Ok(Outcome::default())
            }
            KeyCode::Tab => {
                let lanes = self.doc.project().lanes.len().div_ceil(LANE_KEYS.len());
                let sections = self
                    .doc
                    .project()
                    .sections
                    .len()
                    .div_ceil(SECTION_KEYS.len());
                if self.input.bank + 1 < lanes.max(sections).max(1) {
                    self.input.bank += 1;
                }
                self.status = format!("Bank {}", self.input.bank + 1);
                Ok(Outcome::default())
            }
            KeyCode::Char(':') => {
                self.open_palette(String::new(), Some(Mode::Jam));
                Ok(Outcome::default())
            }
            KeyCode::Char('?') => {
                self.input.mode = Mode::Help;
                self.input.resume_mode = Some(Mode::Jam);
                self.help_ui.overlay_scroll = 0;
                Ok(Outcome::default())
            }
            KeyCode::Char(c) if octave_step(c).is_some() => {
                let step = octave_step(c).expect("octave key");
                self.input.octave = shift_octave(self.input.octave, step);
                self.status = format!("Octave {}", self.input.octave);
                Ok(Outcome::default())
            }
            KeyCode::Char(c) if SECTION_KEYS.contains(&c) => self.number_key(c),
            // Uppercase lane keys still name lanes, so Caps Lock only accents.
            KeyCode::Char(c) if LANE_KEYS.contains(&c.to_ascii_lowercase()) => {
                self.open_lane_layer(c.to_ascii_lowercase())
            }
            KeyCode::Char(c) => Ok(self.jam_note(c, sustain_notes)?.unwrap_or_default()),
            _ => Ok(Outcome::default()),
        }
    }

    /// A number-row key chooses a section in the current bank.
    pub(crate) fn number_key(&mut self, key: char) -> Result<Outcome> {
        let index = SECTION_KEYS
            .iter()
            .position(|k| *k == key)
            .context("Not a number key")?;
        self.trigger_section_jump(index)
    }

    /// Name a lane: it becomes the focused lane and the next key chooses a
    /// part or toggles it.
    pub(crate) fn open_lane_layer(&mut self, key: char) -> Result<Outcome> {
        let index = LANE_KEYS
            .iter()
            .position(|k| *k == key)
            .context("Not a lane key")?;
        let lane = self
            .play_bank_lanes()
            .get(index)
            .copied()
            .with_context(|| format!("No lane on {key}"))?;
        self.input.lane_layer = Some(lane);
        self.input.focused_lane = Some(lane);
        let name = self.lane(lane).map_or("?".into(), |l| l.name.clone());
        self.status = format!(
            "{name} · 1–9 live override · {key} toggle override · Alt+x release · Alt+c keep"
        );
        Ok(Outcome::default())
    }

    /// The key that names a lane in the current bank.
    fn lane_key(&self, lane: Id) -> Option<char> {
        self.play_bank_lanes()
            .iter()
            .position(|l| *l == lane)
            .map(|i| LANE_KEYS[i])
    }

    /// The key after a lane key, in any mode: a digit launches that part
    /// (launching the part already playing restarts it) and the lane key again
    /// toggles the lane. Any other key closes the layer and acts normally, so
    /// a lane key and then a note focuses the lane and plays the note.
    pub(crate) fn lane_layer_key(&mut self, lane: Id, key: KeyEvent) -> Option<Result<Outcome>> {
        if key.kind == KeyEventKind::Repeat {
            self.input.lane_layer = Some(lane);
            return Some(Ok(Outcome::default()));
        }
        match key.code {
            KeyCode::Esc => {
                self.status = "Lane layer closed".into();
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char(c) if Some(c.to_ascii_lowercase()) == self.lane_key(lane) => {
                Some(self.apply(Action::ToggleLane { lane, at: None }))
            }
            KeyCode::Char(c @ '1'..='9') => {
                let index = c as usize - '1' as usize;
                Some(self.lane_part_at(lane, index).and_then(|part| {
                    self.apply(Action::Launch {
                        lane,
                        part,
                        at: None,
                    })
                }))
            }
            _ => None,
        }
    }

    /// The instrument Jam and MIDI notes play: the named lane's playing (or
    /// first) part, otherwise what you are on, otherwise the selected lane's.
    pub(crate) fn jam_instrument(&self) -> Option<Id> {
        let lane_instrument = |lane: Id| {
            self.live
                .expected()
                .iter()
                .find(|a| a.lane == lane)
                .and_then(|a| self.doc.project().part_instrument(a.part.part))
                .or_else(|| {
                    self.lane(lane)
                        .and_then(|l| l.parts.first())
                        .and_then(|p| self.doc.project().part_instrument(p.part))
                })
        };
        self.input
            .focused_lane
            .and_then(lane_instrument)
            .or_else(|| self.active_instrument())
            .or_else(|| self.selected_lane.and_then(lane_instrument))
    }

    /// Play a note key. Without any instrument, the preview patch sounds.
    pub(crate) fn jam_note(&mut self, c: char, sustain_notes: bool) -> Result<Option<Outcome>> {
        let Some((semitone, accent)) = note_key(c) else {
            return Ok(None);
        };
        let held = c.to_ascii_lowercase();
        if self.input.held.iter().any(|&(k, _, _)| k == held) {
            return Ok(Some(Outcome::default()));
        }
        let midi = midi_note(self.input.octave, semitone);
        let velocity = if accent { ACCENT_VELOCITY } else { VELOCITY };
        let Some(instrument) = self.jam_instrument() else {
            return Ok(Some(Outcome {
                rack_audition: Some((self.preview_patch.clone(), midi, velocity)),
                ..Outcome::default()
            }));
        };
        self.instrument_samples_ready(instrument)?;
        if sustain_notes {
            self.input.held.push((held, instrument, midi));
        }
        Ok(Some(Outcome {
            live_notes_on: vec![InstrumentNoteOn {
                instrument_id: instrument,
                midi,
                velocity,
                sustain: sustain_notes,
                part_id: self
                    .selected_part
                    .filter(|part| self.part(*part).is_some_and(|b| b.instrument == instrument)),
            }],
            ..Outcome::default()
        }))
    }
}
