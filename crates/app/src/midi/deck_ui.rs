//! MIDI Controller Deck (`gd` / `Ctrl+d`) key handling and `:midi ...` Ex commands.
//!
//! Layout contract (`docs/spec/decks-and-interaction.md` §4B & §4D):
//! - `DEVICE & PROFILE` vertical group (rows `0..5`: `IN`, `OUT`, `Profile`, `Pads`, `Pickup`)
//!   directly above `MAPPINGS` (`5..5 + len`).
//! - `j`/`k` walks smoothly across both groups; `w`/`b` jumps between groups; bare `h`/`l` is no-op.
//! - `Alt+h`/`l` & `,`/`.` cycle values; `Alt+Backspace` resets; `Enter` opens port/profile/mode
//!   picker (or toggles Pickup / arms MIDI Learn on mapping rows).
//! - Global `t` (Transport prefix), `i`, and `Backspace` are never shadowed on `gd`.
use super::MidiDeckRow;
use crate::devices::cycle_optional;
use crate::*;
use cadence_core::midi_map::{MidiFeedback, MidiPadMode, MidiProfile, MidiValueMode};

impl App {
    pub(crate) fn set_midi_in_port_choice(&mut self, next: Option<String>) -> Outcome {
        self.config.midi_in_port = next.clone();
        self.settings_ui.config_dirty = true;
        self.maybe_persist_config();
        self.status = match &next {
            Some(p) => format!("Connecting MIDI input → {p}"),
            None => "Disconnecting MIDI input".into(),
        };
        Outcome {
            midi_connect: Some(MidiConnectRequest {
                input: Some(next),
                output: None,
            }),
            ..Outcome::default()
        }
    }

    pub(crate) fn set_midi_out_port_choice(&mut self, next: Option<String>) -> Outcome {
        self.config.midi_out_port = next.clone();
        self.settings_ui.config_dirty = true;
        self.maybe_persist_config();
        self.status = match &next {
            Some(p) => format!("Connecting MIDI output → {p}"),
            None => "Disconnecting MIDI output".into(),
        };
        Outcome {
            midi_connect: Some(MidiConnectRequest {
                input: None,
                output: Some(next),
            }),
            ..Outcome::default()
        }
    }

    pub(crate) fn set_midi_pad_mode_choice(&mut self, mode: MidiPadMode) -> Outcome {
        self.config.midi_pad_mode = mode;
        self.settings_ui.config_dirty = true;
        self.maybe_persist_config();
        self.status = format!("MIDI Pads → {}", mode.label());
        let mut out = Outcome::default();
        out.midi_feedback.push(MidiFeedback::Display {
            line1: "Pad Mode".into(),
            line2: mode.label().into(),
            value_bar: None,
        });
        self.sync_midi_feedback(&mut out);
        out
    }

    pub(crate) fn set_midi_pickup_choice(&mut self, enabled: bool) -> Outcome {
        self.config.midi_pickup = enabled;
        self.settings_ui.config_dirty = true;
        self.maybe_persist_config();
        self.reset_midi_pickup();
        self.status = format!("MIDI Soft Pickup → {}", if enabled { "On" } else { "Off" });
        Outcome::default()
    }

    /// Switch to built-in MIDI profile `id`, clearing any loaded custom profile (and optionally
    /// clearing custom user mappings when confirmed by the user).
    pub(crate) fn apply_builtin_midi_profile(
        &mut self,
        id: &str,
        clear_user_mappings: bool,
    ) -> Outcome {
        self.midi.custom_profile = None;
        self.config.midi_profile_path = None;
        if clear_user_mappings {
            self.config.midi_mappings.clear();
        }
        let prof = MidiProfile::by_id(id);
        self.config.midi_profile = prof.id.clone();
        self.settings_ui.config_dirty = true;
        self.maybe_persist_config();
        self.reset_midi_pickup();
        let total = MidiDeckRow::COUNT + self.effective_midi_mappings().len();
        self.midi.cursor = self.midi.cursor.min(total.saturating_sub(1));
        self.status = format!(
            "MIDI Profile → {} ({} bindings · feedback: {})",
            prof.name,
            prof.mappings.len(),
            prof.feedback.label()
        );
        Outcome {
            midi_feedback: self.force_midi_feedback_sync(),
            ..Outcome::default()
        }
    }

    /// Guard switching built-in profiles behind `Mode::Confirm` whenever a custom `.midi.json`
    /// profile or user-learned mappings would be discarded.
    pub(crate) fn request_switch_midi_profile(&mut self, target_id: &str) -> Outcome {
        let prof = MidiProfile::by_id(target_id);
        let would_discard =
            self.midi.custom_profile.is_some() || !self.config.midi_mappings.is_empty();
        if would_discard {
            self.input.mode = Mode::Confirm(ConfirmAction::SwitchMidiProfile(prof.id));
            self.status = format!(
                "Confirm switch to {} · y / Enter confirm · Esc cancel",
                prof.name
            );
            return Outcome::default();
        }
        self.apply_builtin_midi_profile(&prof.id, false)
    }

    /// Clear all user-learned MIDI mappings (called after `Mode::Confirm` or `:midi clear`).
    pub(crate) fn clear_user_midi_mappings(&mut self) -> Outcome {
        let count = self.config.midi_mappings.len();
        self.config.midi_mappings.clear();
        self.settings_ui.config_dirty = true;
        self.maybe_persist_config();
        let total = MidiDeckRow::COUNT + self.effective_midi_mappings().len();
        self.midi.cursor = self.midi.cursor.min(total.saturating_sub(1));
        self.status = format!(
            "Cleared {count} user MIDI mapping{}",
            if count == 1 { "" } else { "s" }
        );
        Outcome {
            midi_feedback: self.force_midi_feedback_sync(),
            ..Outcome::default()
        }
    }

    pub(crate) fn cycle_focused_midi_mapping_mode(&mut self, dir: isize) {
        let mappings = self.effective_midi_mappings();
        let Some(map_idx) = self.midi.cursor.checked_sub(MidiDeckRow::COUNT) else {
            return;
        };
        let Some((mut mapping, is_user)) = mappings.get(map_idx).cloned() else {
            return;
        };
        mapping.mode = mapping.mode.cycle(dir as i32);
        let mode_label = mapping.mode.label();
        if is_user {
            if let Some(um) = self
                .config
                .midi_mappings
                .iter_mut()
                .find(|m| m.id == mapping.id)
            {
                um.mode = mapping.mode;
            }
        } else {
            mapping.id = self.next_midi_mapping_id();
            self.config.midi_mappings.push(mapping);
        }
        self.settings_ui.config_dirty = true;
        self.maybe_persist_config();
        self.status = format!("Mapping mode → {mode_label}");
    }

    /// Tier 3 (`Alt+h`/`l`) & Tier 4 (`,`/`.`) adjustment for the focused MIDI Deck row.
    pub(crate) fn adjust_midi_deck_row(&mut self, dir: isize) -> Result<Outcome> {
        let total = MidiDeckRow::COUNT + self.effective_midi_mappings().len();
        self.midi.cursor = self.midi.cursor.min(total.saturating_sub(1));
        match MidiDeckRow::from_index(self.midi.cursor) {
            Some(MidiDeckRow::In) => {
                let next = cycle_optional(&self.midi.in_ports, self.midi.port.as_ref(), dir);
                Ok(self.set_midi_in_port_choice(next))
            }
            Some(MidiDeckRow::Out) => {
                let next = cycle_optional(&self.midi.out_ports, self.midi.out_port.as_ref(), dir);
                Ok(self.set_midi_out_port_choice(next))
            }
            Some(MidiDeckRow::Profile) => {
                let next_id = MidiProfile::cycle_builtin_id(&self.config.midi_profile, dir);
                Ok(self.request_switch_midi_profile(next_id))
            }
            Some(MidiDeckRow::Pads) => {
                let next = self.config.midi_pad_mode.cycle(dir as i32);
                Ok(self.set_midi_pad_mode_choice(next))
            }
            Some(MidiDeckRow::Pickup) => Ok(self.set_midi_pickup_choice(!self.config.midi_pickup)),
            None => {
                self.cycle_focused_midi_mapping_mode(dir);
                Ok(Outcome::default())
            }
        }
    }

    /// Universal reset (`Alt+Backspace`) for the focused MIDI Deck row.
    pub(crate) fn reset_midi_deck_row(&mut self) -> Result<Outcome> {
        let total = MidiDeckRow::COUNT + self.effective_midi_mappings().len();
        self.midi.cursor = self.midi.cursor.min(total.saturating_sub(1));
        match MidiDeckRow::from_index(self.midi.cursor) {
            Some(MidiDeckRow::In) => Ok(self.set_midi_in_port_choice(None)),
            Some(MidiDeckRow::Out) => Ok(self.set_midi_out_port_choice(None)),
            Some(MidiDeckRow::Profile) => Ok(self.request_switch_midi_profile("generic")),
            Some(MidiDeckRow::Pads) => Ok(self.set_midi_pad_mode_choice(MidiPadMode::Notes)),
            Some(MidiDeckRow::Pickup) => Ok(self.set_midi_pickup_choice(true)),
            None => {
                let mappings = self.effective_midi_mappings();
                if let Some((mapping, is_user)) =
                    mappings.get(self.midi.cursor - MidiDeckRow::COUNT)
                {
                    if *is_user {
                        let id = mapping.id;
                        if let Some(um) = self.config.midi_mappings.iter_mut().find(|m| m.id == id)
                        {
                            um.mode = MidiValueMode::Absolute;
                            um.invert = false;
                        }
                        self.settings_ui.config_dirty = true;
                        self.maybe_persist_config();
                        self.status = "Reset mapping mode → Abs (normal)".into();
                    }
                }
                Ok(Outcome::default())
            }
        }
    }

    /// Primary `Enter` action on the focused MIDI Deck row:
    /// opens port/profile/pad picker on `DEVICE & PROFILE`, toggles Pickup, or arms MIDI Learn on `MAPPINGS`.
    pub(crate) fn activate_midi_deck_row(&mut self) -> Result<Outcome> {
        let total = MidiDeckRow::COUNT + self.effective_midi_mappings().len();
        self.midi.cursor = self.midi.cursor.min(total.saturating_sub(1));
        match MidiDeckRow::from_index(self.midi.cursor) {
            Some(MidiDeckRow::In) => self.open_midi_port_picker(true),
            Some(MidiDeckRow::Out) => self.open_midi_port_picker(false),
            Some(MidiDeckRow::Profile) => self.open_midi_profile_picker(),
            Some(MidiDeckRow::Pads) => self.open_midi_pad_mode_picker(),
            Some(MidiDeckRow::Pickup) => Ok(self.set_midi_pickup_choice(!self.config.midi_pickup)),
            None => Ok(self.toggle_midi_learn()),
        }
    }

    /// Handle keyboard controls when the MIDI Controller Deck (`gd`) has focus.
    pub(crate) fn handle_midi_deck_key(
        &mut self,
        key: KeyEvent,
        count: usize,
    ) -> Option<Result<Outcome>> {
        let mappings = self.effective_midi_mappings();
        let total = MidiDeckRow::COUNT + mappings.len();
        self.midi.cursor = self.midi.cursor.min(total.saturating_sub(1));

        match key.code {
            // Tier 1 vertical navigation across DEVICE & PROFILE (0..5) and MAPPINGS (5..)
            KeyCode::Char('j') | KeyCode::Down => {
                self.midi.cursor = (self.midi.cursor + count).min(total.saturating_sub(1));
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.midi.cursor = self.midi.cursor.saturating_sub(count);
                Some(Ok(Outcome::default()))
            }
            // Bare h/l strictly no-op on 1-axis vertical inspector deck
            KeyCode::Char('h' | 'l') | KeyCode::Left | KeyCode::Right => {
                Some(Ok(Outcome::default()))
            }
            // Group jump between DEVICE & PROFILE (0) and MAPPINGS (5)
            KeyCode::Char('w' | 'b') => {
                if !mappings.is_empty() {
                    self.midi.cursor = if self.midi.cursor < MidiDeckRow::COUNT {
                        MidiDeckRow::COUNT
                    } else {
                        0
                    };
                } else {
                    self.midi.cursor = 0;
                }
                Some(Ok(Outcome::default()))
            }
            KeyCode::Enter => Some(self.activate_midi_deck_row()),
            KeyCode::Char('a') => Some(self.open_midi_add_mapping_picker()),
            KeyCode::Char('p' | 'P') => {
                let dir = if key.code == KeyCode::Char('P') {
                    -1
                } else {
                    1
                };
                let next_id = MidiProfile::cycle_builtin_id(&self.config.midi_profile, dir);
                Some(Ok(self.request_switch_midi_profile(next_id)))
            }
            KeyCode::Char('r') => {
                self.status = "Rescanning OS MIDI ports…".into();
                Some(Ok(Outcome {
                    midi_rescan: true,
                    ..Outcome::default()
                }))
            }
            KeyCode::Char('v') => {
                let Some(map_idx) = self.midi.cursor.checked_sub(MidiDeckRow::COUNT) else {
                    self.status = "Select a mapping row to toggle invert".into();
                    return Some(Ok(Outcome::default()));
                };
                if let Some((mut mapping, is_user)) = mappings.get(map_idx).cloned() {
                    mapping.invert = !mapping.invert;
                    let inv = mapping.invert;
                    if is_user {
                        if let Some(um) = self
                            .config
                            .midi_mappings
                            .iter_mut()
                            .find(|m| m.id == mapping.id)
                        {
                            um.invert = inv;
                        }
                    } else {
                        mapping.id = self.next_midi_mapping_id();
                        self.config.midi_mappings.push(mapping);
                    }
                    self.settings_ui.config_dirty = true;
                    self.maybe_persist_config();
                    self.status = format!("Mapping invert → {}", if inv { "On" } else { "Off" });
                }
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('x') => {
                let Some(map_idx) = self.midi.cursor.checked_sub(MidiDeckRow::COUNT) else {
                    self.status =
                        "Select a user mapping row to delete · X clears all user mappings".into();
                    return Some(Ok(Outcome::default()));
                };
                if let Some((mapping, is_user)) = mappings.get(map_idx) {
                    if *is_user {
                        let id = mapping.id;
                        let label = mapping.label.clone();
                        self.config.midi_mappings.retain(|m| m.id != id);
                        self.settings_ui.config_dirty = true;
                        self.maybe_persist_config();
                        let next_total = MidiDeckRow::COUNT + self.effective_midi_mappings().len();
                        self.midi.cursor = self.midi.cursor.min(next_total.saturating_sub(1));
                        self.status = format!("Removed user mapping: {label}");
                    } else {
                        self.status =
                            "Built-in profile mapping · press Enter to re-learn or switch Profile"
                                .into();
                    }
                }
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('X') => {
                if self.config.midi_mappings.is_empty() {
                    self.status = "No user MIDI mappings to clear".into();
                } else {
                    self.input.mode = Mode::Confirm(ConfirmAction::ClearMidiMappings);
                    self.status = format!(
                        "Clear {} user MIDI mapping(s)? y / Enter confirm · Esc cancel",
                        self.config.midi_mappings.len()
                    );
                }
                Some(Ok(Outcome::default()))
            }
            // Do NOT shadow global `t` (Transport), `i`, or `Backspace` (cancel pending / back).
            _ => None,
        }
    }

    /// `:midi ...` command handler.
    pub(crate) fn execute_midi_command(&mut self, args: &[String]) -> Result<Outcome> {
        match args.first().map(|s| s.to_ascii_lowercase()).as_deref() {
            None | Some("deck" | "open") => {
                self.open_devices_deck(DevicesTab::Midi);
                self.status =
                    "Devices · MIDI · H/L tab · j/k row · w/b group · Alt+h/l ,/. cycle · Enter pick/learn · a add · Ctrl+s save"
                        .into();
                Ok(Outcome::default())
            }
            Some("learn") => Ok(self.toggle_midi_learn()),
            Some("unlearn") => Ok(self.unlearn_midi_target(None)),
            Some("rec" | "record") => Ok(self.toggle_midi_record()),
            Some("rescan") => {
                self.status = "Rescanning OS MIDI ports…".into();
                Ok(Outcome {
                    midi_rescan: true,
                    ..Outcome::default()
                })
            }
            Some("profile") => {
                let id = args
                    .get(1)
                    .context("Usage: :midi profile generic|standard_cc|minilab3_daw")?;
                let normalized = id.trim().to_ascii_lowercase();
                ensure!(
                    matches!(
                        normalized.as_str(),
                        "generic"
                            | "standard_cc"
                            | "standard"
                            | "minilab3"
                            | "minilab3_daw"
                            | "minilab_3"
                            | "arturia_minilab3"
                    ),
                    "Unknown MIDI profile '{id}'; use generic, standard_cc, or minilab3_daw"
                );
                Ok(self.apply_builtin_midi_profile(&normalized, false))
            }
            Some("profile-save") => {
                let path = PathBuf::from(
                    args.get(1)
                        .context("Usage: :midi profile-save PATH.json")?,
                );
                self.save_active_midi_profile(&path)?;
                Ok(Outcome::default())
            }
            Some("profile-load") => {
                let path = PathBuf::from(
                    args.get(1)
                        .context("Usage: :midi profile-load PATH.json")?,
                );
                self.load_midi_profile_from_path(&path)
            }
            Some("pads") => {
                let mode = match args.get(1).map(|s| s.to_ascii_lowercase()).as_deref() {
                    Some("notes" | "kits" | "drums") => MidiPadMode::Notes,
                    Some("sections" | "sec") => MidiPadMode::Sections,
                    Some("mutes" | "mute") => MidiPadMode::Mutes,
                    _ => bail!("Usage: :midi pads notes|sections|mutes"),
                };
                Ok(self.set_midi_pad_mode_choice(mode))
            }
            Some("pickup") => {
                let on = match args.get(1).map(|s| s.to_ascii_lowercase()).as_deref() {
                    Some("on" | "true" | "1") => true,
                    Some("off" | "false" | "0") => false,
                    None => !self.config.midi_pickup,
                    _ => bail!("Usage: :midi pickup [on|off]"),
                };
                Ok(self.set_midi_pickup_choice(on))
            }
            Some("clear") => Ok(self.clear_user_midi_mappings()),
            Some(other) => bail!(
                "Unknown :midi subcommand '{other}' · try learn|unlearn|rec|profile|pads|pickup|rescan|clear"
            ),
        }
    }
}
