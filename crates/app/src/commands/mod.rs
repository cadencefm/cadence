//! Ex commands, parsed into actions and edits.
mod arrange;
mod instruments;
mod material;
mod mixer;
mod registry;

pub(crate) use registry::{
    ArrangeVerb, ExVerb, InstrumentVerb, LiveVerb, MaterialVerb, MixerVerb, SessionVerb,
};

use crate::*;

pub(crate) fn split_words(input: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escape = false;
    for c in input.chars() {
        if escape {
            current.push(c);
            escape = false;
            continue;
        }
        if c == '\\' {
            escape = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                current.push(c);
            }
            continue;
        }
        if c == '\'' || c == '"' {
            quote = Some(c);
        } else if c.is_whitespace() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
        } else {
            current.push(c);
        }
    }
    ensure!(quote.is_none() && !escape, "Unfinished quote or escape");
    if !current.is_empty() {
        words.push(current);
    }
    Ok(words)
}

pub(crate) fn parse_phase(value: &str) -> Result<PhaseMode> {
    match value {
        "song" => Ok(PhaseMode::Song),
        "restart" => Ok(PhaseMode::Restart),
        _ => bail!("Phase is song or restart"),
    }
}

pub(crate) fn parse_ms(value: &str) -> Result<f32> {
    let value = value.trim();
    let n = if let Some(ms) = value.strip_suffix("ms") {
        ms.trim().parse()?
    } else if let Some(seconds) = value.strip_suffix('s') {
        seconds.trim().parse::<f32>()? * 1000.0
    } else {
        value.parse()?
    };
    ensure!(f32::is_finite(n), "Time must be finite");
    Ok(n)
}

pub(super) fn require_args(args: &[String], n: usize, usage: &str) -> Result<()> {
    ensure!(args.len() == n, "Usage: {usage}");
    Ok(())
}

impl App {
    pub(crate) fn pattern_id(&self, value: &str) -> Result<Id> {
        lookup(
            value,
            self.doc
                .project()
                .patterns
                .iter()
                .map(|p| (p.id, p.name.as_str())),
            "pattern",
        )
    }

    pub(crate) fn instrument_id(&self, value: &str) -> Result<Id> {
        lookup(
            value,
            self.doc
                .project()
                .instruments
                .iter()
                .map(|p| (p.id, p.name.as_str())),
            "instrument",
        )
    }

    pub(crate) fn part_id(&self, value: &str) -> Result<Id> {
        lookup(
            value,
            self.doc
                .project()
                .parts
                .iter()
                .map(|p| (p.id, p.name.as_str())),
            "part",
        )
    }

    pub(crate) fn lane_id(&self, value: &str) -> Result<Id> {
        lookup(
            value,
            self.doc
                .project()
                .lanes
                .iter()
                .map(|p| (p.id, p.name.as_str())),
            "lane",
        )
    }

    pub(crate) fn channel_ref(&self, value: &str) -> Result<ChannelRef> {
        if value.eq_ignore_ascii_case("master") {
            return Ok(ChannelRef::Master);
        }
        let value = value.strip_prefix("channel:").unwrap_or(value);
        Ok(ChannelRef::Channel(lookup(
            value,
            self.doc
                .project()
                .mixer
                .channels
                .iter()
                .map(|p| (p.id, p.name.as_str())),
            "channel",
        )?))
    }

    pub(crate) fn channel_id(&self, value: &str) -> Result<Id> {
        let ChannelRef::Channel(id) = self.channel_ref(value)? else {
            bail!("Choose an Audio, Group, or Return channel")
        };
        Ok(id)
    }

    pub(crate) fn step_from_text(&self, text: &str) -> Result<Step> {
        if matches!(text, "." | "~" | "-") {
            return Ok(Step::default());
        }
        let mut pitches = Vec::new();
        for note in text.split(',') {
            let pitch = if let Ok(degree) = note.parse::<i16>() {
                (degree.checked_add(1).context("Degree overflow")?, 0)
            } else {
                let midi = cadence_core::music::parse_root_midi(note)
                    .context("Use a scale degree, MIDI note name, or . rest")?;
                self.doc.project().midi_to_degree(midi)
            };
            pitches.push(pitch);
        }
        ensure!(
            !pitches.is_empty() && pitches.len() <= 6,
            "A step supports up to six chord notes"
        );
        pitches.sort_unstable();
        pitches.dedup();
        let mut step = Step {
            active: true,
            degree: pitches[0].0,
            accidental: pitches[0].1,
            velocity: 0.8,
            gate: 0.75,
            ..Step::default()
        };
        step.chord = pitches
            .into_iter()
            .skip(1)
            .map(|(degree, accidental)| ChordTone {
                degree,
                accidental,
                velocity: 0.8,
                gate: 0.75,
            })
            .collect();
        Ok(step)
    }

    pub fn execute_command(&mut self, raw: &str) -> Result<Outcome> {
        // Commands leave the command line, but never Jam.
        if !self.is_jam() {
            self.input.mode = Mode::Normal;
        }
        let words = split_words(raw)?;
        if words.is_empty() {
            return Ok(Outcome::default());
        }
        let command = words[0].as_str();
        let args = &words[1..];
        let Some(verb) = ExVerb::parse(command) else {
            bail!("Unknown command '{command}' · ? lists command families");
        };
        match verb {
            ExVerb::Live(v) => self.execute_live_verb(v, args),
            ExVerb::Arrange(v) => self.execute_arrange_verb(v, args),
            ExVerb::Material(v) => self.execute_material_verb(v, args),
            ExVerb::Instrument(v) => self.execute_instrument_verb(v, args),
            ExVerb::Mixer(v) => self.execute_mixer_verb(v, args),
            ExVerb::Session(v) => self.execute_session_verb(v, args),
        }
    }

    fn execute_session_verb(&mut self, verb: SessionVerb, args: &[String]) -> Result<Outcome> {
        match verb {
            SessionVerb::New { force } => {
                if !force {
                    if let Some(first) = args.first() {
                        if let Some(template) = cadence_core::rack::RackTemplate::parse(first) {
                            let name = if args.len() > 1 {
                                args[1..].join(" ")
                            } else {
                                unique_name(
                                    template.label(),
                                    self.doc
                                        .project()
                                        .instruments
                                        .iter()
                                        .map(|i| i.name.as_str()),
                                    None,
                                )
                            };
                            return self.apply(Action::CreateInstrument {
                                name,
                                sound: workflow::SoundChoice::Template(template),
                                channel: None,
                            });
                        }
                    }
                }
                let title = if args.is_empty() {
                    None
                } else {
                    Some(args.join(" "))
                };
                if !force && self.dirty() {
                    self.input.mode = Mode::Confirm(ConfirmAction::New(title));
                    Ok(Outcome::default())
                } else {
                    self.apply(Action::New { title })
                }
            }
            SessionVerb::Write => {
                ensure!(args.len() <= 1, "Usage: :w [path]");
                let path = match args.first() {
                    Some(p) => PathBuf::from(p),
                    None => {
                        ensure!(
                            !self.doc.is_untitled(),
                            "Untitled project · save with :w PATH"
                        );
                        self.doc.save_path().clone()
                    }
                };
                self.apply(Action::Save { path })
            }
            SessionVerb::Open { force } => {
                require_args(args, 1, ":e PATH")?;
                let path = PathBuf::from(&args[0]);
                if !force && self.dirty() {
                    self.input.mode = Mode::Confirm(ConfirmAction::Open(path));
                    Ok(Outcome::default())
                } else {
                    self.apply(Action::Load { path })
                }
            }
            SessionVerb::ImportRbs { force } => {
                ensure!(!args.is_empty(), "Usage: :import-rbs PATH");
                let path = PathBuf::from(args.join(" "));
                if !force && self.dirty() {
                    self.input.mode = Mode::Confirm(ConfirmAction::ImportRbs(path));
                    Ok(Outcome::default())
                } else {
                    self.apply(Action::ImportRbs { path })
                }
            }
            SessionVerb::Quit { force: false } => {
                require_args(args, 0, ":q")?;
                Ok(self.request_quit())
            }
            SessionVerb::Quit { force: true } => Ok(Outcome {
                quit: true,
                ..Outcome::default()
            }),
            SessionVerb::WriteQuit => {
                ensure!(
                    !self.doc.is_untitled(),
                    "Untitled project · save with :w PATH first"
                );
                let mut out = self.apply(Action::Save {
                    path: self.doc.save_path().clone(),
                })?;
                out.quit = true;
                Ok(out)
            }
            SessionVerb::Inspect => {
                ensure!(args.len() <= 1, "Usage: :inspect [PATTERN]");
                let out = self.apply(Action::Inspect {
                    pattern: args.first().map(|s| self.pattern_id(s)).transpose()?,
                })?;
                self.help_ui.overlay_scroll = 0;
                self.input.mode = Mode::Inspect;
                Ok(out)
            }
            SessionVerb::Assets => {
                self.help_ui.overlay_scroll = 0;
                self.input.mode = Mode::Assets;
                Ok(Outcome::default())
            }
            SessionVerb::Deck => {
                ensure!(args.len() <= 1, "Usage: :deck [on|off|toggle|pin|unpin]");
                match args.first().map(String::as_str).unwrap_or("toggle") {
                    "on" => self.apply(Action::SetDeck { visible: true }),
                    "off" => self.apply(Action::SetDeck { visible: false }),
                    "toggle" => self.apply(Action::SetDeck {
                        visible: !self.deck_active(),
                    }),
                    "pin" => self.apply(Action::PinDeck { pinned: true }),
                    "unpin" => self.apply(Action::PinDeck { pinned: false }),
                    _ => bail!("Usage: :deck [on|off|toggle|pin|unpin]"),
                }
            }
            SessionVerb::Tips => {
                let next = match args.first().map(String::as_str) {
                    None | Some("toggle") => !self.view_state.tips,
                    Some("on") => true,
                    Some("off") => false,
                    _ => bail!("Usage: :tips [on|off|toggle]"),
                };
                self.set_key_hints_visible(next);
                Ok(Outcome::default())
            }
            SessionVerb::Tutorial => {
                self.input.mode = Mode::Tutorial;
                self.help_ui.tutorial_step = 0;
                Ok(Outcome::default())
            }
            SessionVerb::Help => {
                self.help_ui.searching = false;
                self.help_ui.query.clear();
                self.help_ui.overlay_scroll = 0;
                self.input.mode = Mode::Help;
                Ok(Outcome::default())
            }
            SessionVerb::Settings => {
                self.settings_ui.cursor = 0;
                self.input.mode = Mode::Settings;
                Ok(Outcome::default())
            }
            SessionVerb::Files => {
                self.open_file_hub(FileHubTab::Open);
                Ok(Outcome::default())
            }
            SessionVerb::SaveAs => {
                self.open_file_hub(FileHubTab::Save);
                Ok(Outcome::default())
            }
            SessionVerb::Zone => self.zone_command(args),
            SessionVerb::Patch => self.patch_command(args),
            SessionVerb::Unpatch => self.unpatch_command(args),
            SessionVerb::Pin => self.pin_command(args),
            SessionVerb::Unpin => self.unpin_command(args),
            SessionVerb::Bounce => self.bounce_command(args),
            SessionVerb::Record => self.record_command(args),
            SessionVerb::Audio => self.execute_audio_command(args),
            SessionVerb::Panic => {
                require_args(args, 0, ":panic")?;
                self.apply(Action::AudioPanic)
            }
            SessionVerb::Midi => self.execute_midi_command(args),
        }
    }
}
