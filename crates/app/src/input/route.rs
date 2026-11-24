//! Top-level key routing: from a key event to the active mode, prefix, overlay, or Normal handler.
use super::*;

impl App {
    pub(crate) fn clear_entry_state(&mut self) {
        self.clear_browser();
        self.modal = ActiveModal::None;
        self.input.count = None;
    }

    pub fn release_held(&mut self) -> Outcome {
        Outcome {
            live_notes_off: self
                .input
                .held
                .drain(..)
                .map(|(_, instrument_id, midi)| InstrumentNoteOff {
                    instrument_id,
                    midi,
                })
                .collect(),
            ..Outcome::default()
        }
    }

    /// Handle an interactive keyboard event where `KeyEventKind::Release` follows `Press`,
    /// sustaining held Jam notes until key release.
    pub fn handle_key(&mut self, raw: KeyEvent) -> Result<Outcome> {
        self.dispatch_key(raw, true)
    }

    /// Handle a scripted press-only keyboard event without `KeyEventKind::Release`,
    /// firing Jam notes as finite one-shots rather than holding them indefinitely.
    pub fn handle_script_key(&mut self, raw: KeyEvent) -> Result<Outcome> {
        self.dispatch_key(raw, false)
    }

    fn dispatch_key(&mut self, raw: KeyEvent, sustain_notes: bool) -> Result<Outcome> {
        let key = normalize_shifted_key(raw);
        if key.kind == KeyEventKind::Press {
            self.doc.break_coalesce();
            self.clear_midi_cc_rec_gesture();
        }
        if key.kind == KeyEventKind::Release {
            if let KeyCode::Char(c) = key.code {
                let mut off = Vec::new();
                self.input.held.retain(|&(key, id, midi)| {
                    if key == c.to_ascii_lowercase() {
                        off.push(InstrumentNoteOff {
                            instrument_id: id,
                            midi,
                        });
                        false
                    } else {
                        true
                    }
                });
                return Ok(Outcome {
                    live_notes_off: off,
                    ..Outcome::default()
                });
            }
            return Ok(Outcome::default());
        }
        if key.code == KeyCode::Home {
            self.input.count = None;
            return if key.kind == KeyEventKind::Repeat {
                Ok(Outcome::default())
            } else {
                self.apply(Action::Stop)
            };
        }
        // After a lane key, the next key chooses a part or toggles the lane, in
        // every mode; any other key closes the layer and carries on.
        if let Some(lane) = self.input.lane_layer.take() {
            if let Some(result) = self.lane_layer_key(lane, key) {
                return result;
            }
        }
        if key.modifiers.contains(KeyModifiers::ALT) {
            if let Some(result) = self.handle_alt(key) {
                return result;
            }
        }
        // Pending authoring/value fields own text, choice and cancellation keys.
        // Ctrl+c cancels any open non-Normal mode back to Normal.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if key.kind == KeyEventKind::Repeat {
                return Ok(Outcome::default());
            }
            if self.input.mode == Mode::Normal {
                self.input.count = None;
                self.input.mode = Mode::Confirm(ConfirmAction::Quit);
                return Ok(self.release_held());
            }
            if self.has_preview_browser() {
                let mut out = self.cancel_active_preview_browser()?;
                out.merge(self.release_held());
                return Ok(out);
            }
            self.dismiss_transient_modals();
            self.input.mode = Mode::Normal;
            self.status = "Cancelled".into();
            return Ok(self.release_held());
        }
        // Ctrl+Shift+j toggles Jam from Normal/Jam and cleanly cancels live preview browsers.
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && (key.modifiers.contains(KeyModifiers::SHIFT)
                || matches!(key.code, KeyCode::Char('J')))
            && matches!(key.code, KeyCode::Char('j' | 'J'))
            && (matches!(self.input.mode, Mode::Normal | Mode::Jam) || self.has_preview_browser())
        {
            if key.kind == KeyEventKind::Repeat {
                return Ok(Outcome::default());
            }
            self.input.count = None;
            let mut pre_out = Outcome::default();
            if self.has_preview_browser() {
                pre_out = self.cancel_active_preview_browser()?;
            }
            let mut out = if self.is_jam() {
                self.leave_jam()
            } else {
                self.enter_jam()
            };
            out.merge(pre_out);
            return Ok(out);
        }
        match self.input.mode {
            Mode::Create => return self.handle_create(key),
            Mode::Workflow => return self.handle_workflow(key),
            Mode::Browser => return self.handle_browser(key),
            Mode::PresetBrowser => return self.handle_preset_browser(key),
            Mode::GrooveBrowser => return self.handle_groove_browser(key),
            Mode::SongProperties => return self.handle_song_properties(key),
            Mode::MiniEditor => return self.handle_mini_editor(key),
            Mode::Palette => return self.handle_palette(key),
            Mode::FileHub => return self.handle_file_hub(key),
            Mode::Pick => return self.handle_picker(key),
            Mode::Authoring => return self.handle_authoring(key),
            Mode::ValueInput => return self.handle_value_input(key),
            Mode::Interpolation => return self.handle_value_mode(key),
            Mode::ArrangeViews => return self.handle_arrange_views(key),
            Mode::ArrangementInspect => return self.handle_arrangement_inspection(key),
            _ => {}
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            let repeat = key.kind == KeyEventKind::Repeat;
            let shifted = key.modifiers.contains(KeyModifiers::SHIFT)
                || matches!(key.code, KeyCode::Char(c) if c.is_ascii_uppercase() || matches!(c, '<' | '?'));
            let norm = match key.code {
                KeyCode::Char('<') => KeyCode::Char(','),
                KeyCode::Char('?') => KeyCode::Char('/'),
                KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
                other => other,
            };
            // Zoom chords work in Normal & Insert (including Shift+= -> '+')
            if matches!(self.input.mode, Mode::Normal | Mode::Insert) {
                match norm {
                    KeyCode::Char('=' | '+') => {
                        self.zoom_view(1);
                        return Ok(Outcome::default());
                    }
                    KeyCode::Char('-') if !shifted => {
                        self.zoom_view(-1);
                        return Ok(Outcome::default());
                    }
                    KeyCode::Char('0') if !shifted => {
                        self.zoom_view_reset();
                        return Ok(Outcome::default());
                    }
                    _ => {}
                }
            }
            // Clear text input passes through to active search/filter/path editors
            if !shifted
                && norm == KeyCode::Char('u')
                && (matches!(self.input.mode, Mode::Search(_))
                    || (self.input.mode == Mode::Help && self.help_ui.searching)
                    || (self.input.mode == Mode::Settings && self.settings_ui.editing.is_some()))
            {
                // Fall through to active mode handler below.
            } else if repeat {
                return Ok(Outcome::default());
            } else if shifted {
                // Ctrl+Shift+<key> = all modals, overlays & CUA Redo
                match norm {
                    KeyCode::Char('p') if matches!(self.input.mode, Mode::Normal | Mode::Jam) => {
                        let resume = self.is_jam().then_some(Mode::Jam);
                        self.open_palette(String::new(), resume);
                        return Ok(Outcome::default());
                    }
                    KeyCode::Char('/')
                        if matches!(self.input.mode, Mode::Normal | Mode::Jam | Mode::Help) =>
                    {
                        if self.input.mode == Mode::Help {
                            self.help_ui.searching = false;
                            self.help_ui.query.clear();
                            self.input.mode = self.take_resume_mode();
                        } else {
                            self.help_ui.searching = false;
                            self.help_ui.query.clear();
                            self.help_ui.overlay_scroll = 0;
                            self.input.resume_mode = self.is_jam().then_some(Mode::Jam);
                            self.input.mode = Mode::Help;
                        }
                        return Ok(Outcome::default());
                    }
                    KeyCode::Char('z') if self.input.mode == Mode::Normal => {
                        self.input.count = None;
                        return self.apply(Action::Redo);
                    }
                    KeyCode::Char('f' | 'o') if self.input.mode == Mode::Normal => {
                        self.open_file_hub(FileHubTab::Open);
                        return Ok(Outcome::default());
                    }
                    KeyCode::Char('s') if self.input.mode == Mode::Normal => {
                        if self.take_focused() {
                            self.open_file_hub_for_take_save();
                        } else if self.midi_focused() && self.devices.tab == DevicesTab::Midi {
                            self.open_file_hub_for_midi_profile_save();
                        } else {
                            self.open_file_hub(FileHubTab::Save);
                        }
                        return Ok(Outcome::default());
                    }
                    KeyCode::Char('e') if self.input.mode == Mode::Normal => {
                        self.open_file_hub(FileHubTab::Export);
                        return Ok(Outcome::default());
                    }
                    KeyCode::Char('i') if self.input.mode == Mode::Normal => {
                        self.open_song_properties(song_props::SongPropRow::Title, false);
                        return Ok(Outcome::default());
                    }
                    KeyCode::Char(',') if self.input.mode == Mode::Normal => {
                        self.open_settings_overlay();
                        return Ok(Outcome::default());
                    }
                    KeyCode::Char('b') if self.input.mode == Mode::Normal => {
                        return self.browse_samples();
                    }
                    KeyCode::Char('n') if self.input.mode == Mode::Normal => {
                        return self.begin_create();
                    }
                    _ => return Ok(Outcome::default()),
                }
            } else {
                // Ctrl+<key> unshifted = all 10 docked decks + CUA core
                match norm {
                    KeyCode::Char('s') if self.input.mode == Mode::Settings => {
                        self.save_config()?;
                        return Ok(Outcome::default());
                    }
                    KeyCode::Char('s') if self.input.mode == Mode::Normal => {
                        if self.midi_focused() {
                            self.save_config()?;
                            return Ok(Outcome::default());
                        }
                        if self.take_focused() {
                            return self.apply(Action::SaveTake { path: None });
                        }
                        if self.doc.is_untitled() {
                            self.open_file_hub(FileHubTab::Save);
                            return Ok(Outcome::default());
                        }
                        return self.apply(Action::Save {
                            path: self.doc.save_path().clone(),
                        });
                    }
                    KeyCode::Char('z') if self.input.mode == Mode::Normal => {
                        self.input.count = None;
                        return self.apply(Action::Undo);
                    }
                    KeyCode::Char('r') if self.input.mode == Mode::Normal => {
                        self.input.count = None;
                        return self.apply(Action::Redo);
                    }
                    KeyCode::Char('w') if self.input.mode == Mode::Normal => {
                        self.input.count = None;
                        self.input.mode = Mode::Window;
                        return Ok(Outcome::default());
                    }
                    KeyCode::Char(',') if self.input.mode == Mode::Normal => {
                        self.open_settings_overlay();
                        return Ok(Outcome::default());
                    }
                    KeyCode::Char('a') if self.input.mode == Mode::Normal => {
                        return self.focus_docked_deck(layout::DeckKind::Arrange);
                    }
                    KeyCode::Char('p') if self.input.mode == Mode::Normal => {
                        return self.focus_docked_deck(layout::DeckKind::Pattern);
                    }
                    KeyCode::Char('b') if self.input.mode == Mode::Normal => {
                        return self.focus_docked_deck(layout::DeckKind::Bank);
                    }
                    KeyCode::Char('m') if self.input.mode == Mode::Normal => {
                        return self.focus_docked_deck(layout::DeckKind::Mixer);
                    }
                    KeyCode::Char('i') if self.input.mode == Mode::Normal => {
                        return self.focus_docked_deck(layout::DeckKind::Instrument);
                    }
                    KeyCode::Char('k') if self.input.mode == Mode::Normal => {
                        return self.focus_docked_deck(layout::DeckKind::Macros);
                    }
                    KeyCode::Char('t') if self.input.mode == Mode::Normal => {
                        return self.focus_docked_deck(layout::DeckKind::Take);
                    }
                    KeyCode::Char('g') if self.input.mode == Mode::Normal => {
                        return self.focus_docked_deck(layout::DeckKind::Graph);
                    }
                    KeyCode::Char('d') if self.input.mode == Mode::Normal => {
                        return self.focus_docked_deck(layout::DeckKind::Midi);
                    }
                    KeyCode::Char('o') if self.input.mode == Mode::Normal => {
                        return self.focus_docked_deck(layout::DeckKind::Output);
                    }
                    _ => return Ok(Outcome::default()),
                }
            }
        }
        if self.input.mode == Mode::Jam {
            return self.handle_jam(key, sustain_notes);
        }
        if self.input.mode == Mode::Stage {
            return self.handle_stage_prefix(key);
        }
        if key.code == KeyCode::Esc {
            return self.handle_escape();
        }
        if key.kind == KeyEventKind::Repeat
            && matches!(self.input.mode, Mode::Go | Mode::Transport | Mode::Window)
        {
            return Ok(Outcome::default());
        }
        match self.input.mode.clone() {
            Mode::Search(query) => self.handle_search_mode(query, key),
            Mode::Transport => self.handle_transport_prefix(key),
            Mode::Window => self.handle_window_prefix(key),
            Mode::Go => self.handle_go_prefix(key),
            Mode::Help => self.handle_help_overlay(key),
            Mode::Settings => self.handle_settings_overlay(key),
            Mode::Inspect | Mode::Assets => self.handle_inspect_overlay(key),
            Mode::Tutorial => self.handle_tutorial_overlay(key),
            Mode::Confirm(action) => self.handle_confirm_overlay(action, key),
            Mode::Insert => self.handle_insert(key),
            Mode::Normal => self.handle_normal_key(key),
            Mode::Create
            | Mode::Workflow
            | Mode::Browser
            | Mode::PresetBrowser
            | Mode::GrooveBrowser
            | Mode::SongProperties
            | Mode::MiniEditor
            | Mode::Palette
            | Mode::FileHub
            | Mode::Pick
            | Mode::Authoring
            | Mode::ValueInput
            | Mode::Interpolation
            | Mode::ArrangeViews
            | Mode::ArrangementInspect
            | Mode::Jam
            | Mode::Stage => {
                unreachable!("non-normal modes handled above")
            }
        }
    }

    fn handle_escape(&mut self) -> Result<Outcome> {
        if self.input.mode == Mode::Normal && self.midi.learn.take().is_some() {
            self.status = "MIDI Learn cancelled".into();
            return Ok(Outcome::default());
        }
        if self.input.mode == Mode::Normal && self.input.count.take().is_some() {
            self.status = "Count cleared".into();
            return Ok(Outcome::default());
        }
        if self.input.mode == Mode::Normal && self.staging.is_some() {
            return self.apply(Action::FinishStaging {
                choice: StageChoice::Discard,
            });
        }
        if self.input.mode == Mode::Normal
            && self.take_focused()
            && (self.takes.visual_anchor.is_some()
                || self.takes.mark_start.is_some()
                || self.takes.mark_end.is_some())
        {
            self.takes.visual_anchor = None;
            self.takes.mark_start = None;
            self.takes.mark_end = None;
            self.status = "Cleared take range marks".into();
            return Ok(Outcome::default());
        }
        match self.input.mode {
            Mode::Normal => self.back(),
            Mode::Search(_) => {
                if let Some(origin) = self.modal.take_search_origin() {
                    self.restore_navigation(origin);
                } else {
                    self.query.clear();
                }
                self.input.mode = Mode::Normal;
                Ok(Outcome::default())
            }
            Mode::Help if self.help_ui.searching || !self.help_ui.query.is_empty() => {
                self.help_ui.searching = false;
                self.help_ui.query.clear();
                self.help_ui.overlay_scroll = 0;
                Ok(Outcome::default())
            }
            Mode::Settings if self.settings_ui.editing.is_some() => {
                self.settings_ui.editing = None;
                self.status = "Directory entry cancelled".into();
                Ok(Outcome::default())
            }
            Mode::Help | Mode::Inspect if self.input.resume_mode.is_some() => {
                self.input.mode = self.take_resume_mode();
                Ok(Outcome::default())
            }
            Mode::Tutorial => {
                self.input.resume_mode = None;
                self.input.mode = Mode::Normal;
                Ok(self.release_held())
            }
            _ => {
                self.input.mode = Mode::Normal;
                Ok(self.release_held())
            }
        }
    }

    pub fn press_keys(&mut self, script: &str) -> Result<Vec<Outcome>> {
        parse_key_sequence(script)
            .into_iter()
            .map(|key| self.handle_script_key(key))
            .collect()
    }
}
