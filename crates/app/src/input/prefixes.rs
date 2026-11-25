//! Prefix modes (`Mode::Go`, `Mode::Transport`, `Mode::Window`) and modal overlay/line editors
//! (`Search`, `Help`, `Settings`, `Inspect`, `Assets`, `Tutorial`, `Confirm`).
use super::*;

impl App {
    pub(super) fn handle_search_mode(
        &mut self,
        mut query: String,
        key: KeyEvent,
    ) -> Result<Outcome> {
        match key.code {
            KeyCode::Enter => {
                self.query = query;
                self.input.mode = Mode::Normal;
                if let Some(origin) = self.modal.take_search_origin() {
                    self.back.push(origin);
                    if self.back.len() > 32 {
                        self.back.remove(0);
                    }
                }
                return Ok(Outcome::default());
            }
            KeyCode::Backspace => {
                query.pop();
            }
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => query.clear(),
            KeyCode::Char(c) if !c.is_control() && query.len() < 80 => query.push(c),
            _ => {}
        }
        self.input.mode = Mode::Search(query);
        let first = self.visible_patterns().first().map(|p| p.id);
        if !self
            .visible_patterns()
            .iter()
            .any(|p| Some(p.id) == self.selected_pattern)
        {
            self.select_library_pattern(first);
        }
        Ok(Outcome::default())
    }

    pub(super) fn handle_transport_prefix(&mut self, key: KeyEvent) -> Result<Outcome> {
        if key.kind == KeyEventKind::Repeat {
            return Ok(Outcome::default());
        }
        self.input.mode = Mode::Normal;
        match key.code {
            KeyCode::Char('f') => self.apply(Action::ToggleFollow),
            KeyCode::Char('g') => self.begin_cue_bar_value_input(),
            KeyCode::Char('l') => self.loop_section_at_cursor(),
            KeyCode::Char('x') => self.clear_loop_region(),
            KeyCode::Char('X') => self.apply(Action::CancelPending { all: true }),
            KeyCode::Char('s') => self.apply(Action::Stop),
            KeyCode::Char('e') => self.set_end_rule(self.end_rule().next()),
            KeyCode::Char('r') => self.apply(Action::ToggleTakeArmed),
            KeyCode::Char('R') => self.apply(Action::RestartLoop),
            KeyCode::Char('a') => self.apply(Action::ToggleAudioRecord),
            KeyCode::Char('A') => self.open_record_dialog(),
            KeyCode::Char('i') => self.apply(Action::CycleInputMonitor),
            KeyCode::Char('c') => self.apply(Action::CueSelection { play: false }),
            KeyCode::Char('C') => self.apply(Action::ClearMeters { channel: None }),
            KeyCode::Char('p') => self.apply(Action::CueSelection { play: true }),
            KeyCode::Char('b') => self.open_bounce_dialog(),
            KeyCode::Char('q') => self.cycle_launch_boundary(1),
            KeyCode::Char('Q') => self.open_cycle_boundary_picker(),
            KeyCode::Char('E') => self.toggle_edit_timing(),
            _ => {
                self.status = "Transport cancelled · t shows actions".into();
                Ok(Outcome::default())
            }
        }
    }

    pub(super) fn handle_window_prefix(&mut self, key: KeyEvent) -> Result<Outcome> {
        self.input.mode = Mode::Normal;
        match key.code {
            KeyCode::Char('w') | KeyCode::Tab => self.cycle_focus(),
            KeyCode::Char('z') => self.zoom_active(),
            KeyCode::Char('p') => self.toggle_deck_pin()?,
            KeyCode::Char('x') => self.close_deck()?,
            KeyCode::Char('o') => self.only_deck(),
            KeyCode::Char('h') => self.move_deck_focus(layout::Direction::Left),
            KeyCode::Char('j') => self.move_deck_focus(layout::Direction::Down),
            KeyCode::Char('k') => self.move_deck_focus(layout::Direction::Up),
            KeyCode::Char('l') => self.move_deck_focus(layout::Direction::Right),
            KeyCode::Char('=' | '+') => self.zoom_view(1),
            KeyCode::Char('-') => self.zoom_view(-1),
            KeyCode::Char('0') => self.zoom_view_reset(),
            KeyCode::Char(c @ '1'..='4') => {
                self.apply_layout_preset((c as u8) - b'0');
            }
            _ => self.status = "Deck action cancelled · z shows choices".into(),
        }
        Ok(Outcome::default())
    }

    /// Single source of truth for opening any of the 10 docked workspace decks
    /// (shared by `g<deck>` prefix and unshifted `Ctrl+<deck>` direct chords).
    pub(crate) fn focus_docked_deck(&mut self, deck: layout::DeckKind) -> Result<Outcome> {
        match deck {
            layout::DeckKind::Output => {
                return self.apply(Action::SetMonitor {
                    visible: !self.view_state.monitor_visible,
                });
            }
            layout::DeckKind::Arrange => {
                self.go_to_deck(self.workspace_deck(Workspace::Arrange));
                self.select_at_cursor();
            }
            layout::DeckKind::Pattern => {
                self.go_to_deck(self.workspace_deck(Workspace::Pattern));
            }
            layout::DeckKind::Sample => {
                return self.open_sample_deck();
            }
            layout::DeckKind::Bank => {
                self.view_state.bank_mode = BankMode::Parts;
                self.go_to_deck(layout::Deck::song(layout::DeckKind::Bank));
            }
            layout::DeckKind::Mixer => {
                self.go_to_deck(self.workspace_deck(Workspace::Mixer));
            }
            layout::DeckKind::Take => {
                self.go_to_deck(layout::Deck::song(layout::DeckKind::Take));
                if let Some(take) = self.current_take() {
                    let bars = take.steps().div_ceil(16).max(1) as usize;
                    self.takes.cursor_bar = self.takes.cursor_bar.min(bars - 1);
                }
            }
            layout::DeckKind::Graph => {
                self.go_to_deck(layout::Deck::song(layout::DeckKind::Graph));
            }
            layout::DeckKind::Midi => {
                self.go_to_deck(layout::Deck::song(layout::DeckKind::Midi));
            }
            layout::DeckKind::Macros => {
                self.go_to_deck(layout::Deck::song(layout::DeckKind::Macros));
            }
            layout::DeckKind::Instrument => {
                let id = self
                    .selected_instrument
                    .context("Select a part or instrument")?;
                self.open_instrument(id)?;
            }
            _ => {}
        }
        self.normalize_focus();
        Ok(Outcome::default())
    }

    pub(super) fn open_settings_overlay(&mut self) {
        self.settings_ui.cursor = 0;
        self.input.mode = Mode::Settings;
    }

    pub(super) fn handle_go_prefix(&mut self, key: KeyEvent) -> Result<Outcome> {
        self.input.mode = Mode::Normal;
        match key.code {
            KeyCode::Char('o') => return self.focus_docked_deck(layout::DeckKind::Output),
            KeyCode::Char('a') => return self.focus_docked_deck(layout::DeckKind::Arrange),
            KeyCode::Char('p') => return self.focus_docked_deck(layout::DeckKind::Pattern),
            KeyCode::Char('w') => return self.focus_docked_deck(layout::DeckKind::Sample),
            KeyCode::Char('b') => return self.focus_docked_deck(layout::DeckKind::Bank),
            KeyCode::Char('m') => return self.focus_docked_deck(layout::DeckKind::Mixer),
            KeyCode::Char('t') => return self.focus_docked_deck(layout::DeckKind::Take),
            KeyCode::Char('G') => return self.focus_docked_deck(layout::DeckKind::Graph),
            KeyCode::Char('d') => return self.focus_docked_deck(layout::DeckKind::Midi),
            KeyCode::Char('k') => return self.focus_docked_deck(layout::DeckKind::Macros),
            KeyCode::Char('i') => return self.focus_docked_deck(layout::DeckKind::Instrument),
            KeyCode::Char('j') => return Ok(self.enter_jam()),
            KeyCode::Char('f') => {
                self.open_file_hub(FileHubTab::Quick);
                return Ok(Outcome::default());
            }
            KeyCode::Char('P') => {
                self.open_song_properties(song_props::SongPropRow::Title, false);
                return Ok(Outcome::default());
            }
            KeyCode::Char(',') => {
                self.open_settings_overlay();
                return Ok(Outcome::default());
            }
            KeyCode::Char('S') => return self.browse_samples(),
            KeyCode::Char('s') => {
                // From a Filter Effect deck (or a Pattern link row inside Detail::Effect), jump directly
                // to the linked pattern; from a derived Pattern deck, open the selected source chip;
                // elsewhere, open the selected material's definition.
                if self.deck_focused() {
                    if let Detail::Effect(dev_id) = self.detail {
                        if let Some(
                            &ControlTarget::Pattern(pid)
                            | &ControlTarget::ModulationLink { pattern: pid, .. },
                        ) = self
                            .control_descriptors()
                            .get(self.control_cursor)
                            .map(|c| &c.target)
                        {
                            self.select_pattern(pid)?;
                            self.normalize_focus();
                            return Ok(Outcome::default());
                        }
                        if let Some((_, dev)) = self.effect(dev_id) {
                            if let cadence_core::mixer::EffectKind::Filter(fp) = dev.kind {
                                if let Some(pid) = fp.env_pattern_id {
                                    self.select_pattern(pid)?;
                                    self.normalize_focus();
                                    return Ok(Outcome::default());
                                }
                                if matches!(
                                    self.control_descriptors()
                                        .get(self.control_cursor)
                                        .map(|c| &c.target),
                                    Some(ControlTarget::FilterEnvSource(_))
                                ) {
                                    self.status = "Filter uses Groove Bank · Enter picks a pattern · f forks groove"
                                        .into();
                                    return Ok(Outcome::default());
                                }
                            }
                        }
                    }
                }
                let on_pattern = self
                    .layout
                    .focused()
                    .is_some_and(|d| d.kind == layout::DeckKind::Pattern);
                let source = on_pattern
                    .then(|| self.selected_pattern_object())
                    .flatten()
                    .and_then(|p| {
                        let sources = p.kind.sources();
                        if sources.is_empty() {
                            None
                        } else {
                            let idx = self.source_cursor.min(sources.len() - 1);
                            sources.get(idx).copied()
                        }
                    });
                let id = source
                    .or(self.selected_pattern)
                    .context("Select material")?;
                self.select_pattern(id)?;
            }
            KeyCode::Char('g') => self.navigate_edge(false),
            KeyCode::Char('e') => self.navigate_edge(true),
            _ => {}
        }
        self.normalize_focus();
        Ok(Outcome::default())
    }

    pub(super) fn handle_help_overlay(&mut self, key: KeyEvent) -> Result<Outcome> {
        let max_scroll = self.help_entry_count().saturating_sub(1);
        if self.help_ui.searching {
            match key.code {
                KeyCode::Enter | KeyCode::Tab => self.help_ui.searching = false,
                KeyCode::Backspace => {
                    self.help_ui.query.pop();
                    self.help_ui.overlay_scroll = 0;
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.help_ui.query.clear();
                    self.help_ui.overlay_scroll = 0;
                }
                KeyCode::Down => {
                    self.help_ui.overlay_scroll = (self.help_ui.overlay_scroll + 1).min(max_scroll)
                }
                KeyCode::Up => {
                    self.help_ui.overlay_scroll = self.help_ui.overlay_scroll.saturating_sub(1)
                }
                KeyCode::Char(c)
                    if !c.is_control()
                        && !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                        && self.help_ui.query.len() < 64 =>
                {
                    self.help_ui.query.push(c);
                    self.help_ui.overlay_scroll = 0;
                }
                _ => {}
            }
            return Ok(Outcome::default());
        }
        let tabs = crate::view::HelpTab::ALL.len();
        match key.code {
            KeyCode::Char('?') if key.kind != KeyEventKind::Repeat => {
                self.help_ui.searching = false;
                self.help_ui.query.clear();
                self.input.mode = self.take_resume_mode();
            }
            KeyCode::Char('/') => {
                self.help_ui.searching = true;
                self.help_ui.overlay_scroll = 0;
            }
            KeyCode::Char('h' | 'H') | KeyCode::Left | KeyCode::BackTab => {
                self.help_ui.tab = (self.help_ui.tab + tabs - 1) % tabs;
                self.help_ui.overlay_scroll = 0;
            }
            KeyCode::Char('l' | 'L') | KeyCode::Right | KeyCode::Tab => {
                self.help_ui.tab = (self.help_ui.tab + 1) % tabs;
                self.help_ui.overlay_scroll = 0;
            }
            KeyCode::Char(' ') if key.kind != KeyEventKind::Repeat => {
                return self.apply(Action::Play {
                    playing: !self.transport.playing,
                });
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.help_ui.overlay_scroll = (self.help_ui.overlay_scroll + 1).min(max_scroll);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.help_ui.overlay_scroll = self.help_ui.overlay_scroll.saturating_sub(1);
            }
            KeyCode::Char('0') => self.help_ui.overlay_scroll = 0,
            KeyCode::Char('$' | 'G') | KeyCode::End => self.help_ui.overlay_scroll = max_scroll,
            KeyCode::PageUp => {
                self.help_ui.overlay_scroll = self.help_ui.overlay_scroll.saturating_sub(8);
            }
            KeyCode::PageDown => {
                self.help_ui.overlay_scroll = (self.help_ui.overlay_scroll + 8).min(max_scroll);
            }
            _ => {}
        }
        Ok(Outcome::default())
    }

    pub(super) fn handle_settings_overlay(&mut self, key: KeyEvent) -> Result<Outcome> {
        if let Some((kind, mut buf)) = self.settings_ui.editing.take() {
            match key.code {
                KeyCode::Enter => {
                    if let Err(err) = self.add_config_dir(kind, buf.clone()) {
                        self.status = format!("{err:#}");
                        self.settings_ui.editing = Some((kind, buf));
                        return Ok(Outcome::default());
                    }
                    return Ok(Outcome {
                        samples_changed: kind == SettingsDirKind::SampleDirs,
                        ..Outcome::default()
                    });
                }
                KeyCode::Backspace => {
                    buf.pop();
                    self.settings_ui.editing = Some((kind, buf));
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    buf.clear();
                    self.settings_ui.editing = Some((kind, buf));
                }
                KeyCode::Char(c) if !c.is_control() && buf.len() < 512 => {
                    buf.push(c);
                    self.settings_ui.editing = Some((kind, buf));
                }
                _ => {
                    self.settings_ui.editing = Some((kind, buf));
                }
            }
            return Ok(Outcome::default());
        }

        let targets = self.settings_row_targets();
        let max_idx = targets.len().saturating_sub(1);
        self.settings_ui.cursor = self.settings_ui.cursor.min(max_idx);
        let active = targets[self.settings_ui.cursor];
        let groups: Vec<&str> = targets.iter().map(|t| t.group()).collect();

        // Tier 3 (Alt+h/l/H/L / Alt+Arrows) & universal reset (Alt+Backspace)
        if key.modifiers.contains(KeyModifiers::ALT) {
            let shifted = key.modifiers.contains(KeyModifiers::SHIFT);
            let repeat = key.kind == KeyEventKind::Repeat;
            if matches!(active, SettingsRowTarget::Core(_)) {
                match key.code {
                    KeyCode::Backspace if !repeat => self.reset_focused_setting(),
                    KeyCode::Char('h') | KeyCode::Left if !shifted => {
                        self.adjust_setting(-1, false)
                    }
                    KeyCode::Char('l') | KeyCode::Right if !shifted => {
                        self.adjust_setting(1, false)
                    }
                    KeyCode::Char('H') | KeyCode::Char('h') | KeyCode::Left => {
                        self.adjust_setting(-1, true)
                    }
                    KeyCode::Char('L') | KeyCode::Char('l') | KeyCode::Right => {
                        self.adjust_setting(1, true)
                    }
                    _ => {}
                }
            }
            return Ok(Outcome::default());
        }

        match key.code {
            // Tier 4 (,/. fine, </> coarse) direct 1D scalar adjustment
            KeyCode::Char(',') if matches!(active, SettingsRowTarget::Core(_)) => {
                self.adjust_setting(-1, false);
            }
            KeyCode::Char('.') if matches!(active, SettingsRowTarget::Core(_)) => {
                self.adjust_setting(1, false);
            }
            KeyCode::Char('<') if matches!(active, SettingsRowTarget::Core(_)) => {
                self.adjust_setting(-1, true);
            }
            KeyCode::Char('>') if matches!(active, SettingsRowTarget::Core(_)) => {
                self.adjust_setting(1, true);
            }
            // Tier 1 vertical navigation (bare h/l strictly no-op)
            KeyCode::Char('j') | KeyCode::Down => {
                self.settings_ui.cursor = (self.settings_ui.cursor + 1).min(max_idx);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.settings_ui.cursor = self.settings_ui.cursor.saturating_sub(1);
            }
            KeyCode::Char('0') => {
                self.settings_ui.cursor = 0;
            }
            KeyCode::Char('$' | 'G') | KeyCode::End => {
                self.settings_ui.cursor = max_idx;
            }
            KeyCode::PageUp => {
                self.settings_ui.cursor = self.settings_ui.cursor.saturating_sub(6);
            }
            KeyCode::PageDown => {
                self.settings_ui.cursor = (self.settings_ui.cursor + 6).min(max_idx);
            }
            KeyCode::Char('w') => {
                if let Some(next) = crate::jump_group_index(&groups, self.settings_ui.cursor, true)
                {
                    self.settings_ui.cursor = next;
                }
            }
            KeyCode::Char('b') => {
                if let Some(next) = crate::jump_group_index(&groups, self.settings_ui.cursor, false)
                {
                    self.settings_ui.cursor = next;
                }
            }
            KeyCode::Char('c')
                if active == SettingsRowTarget::Core(SettingsCoreRow::LaunchQuantize) =>
            {
                return self.open_cycle_boundary_picker();
            }
            KeyCode::Char('a') => {
                let kind = match active {
                    SettingsRowTarget::AddDir(k) | SettingsRowTarget::DirEntry(k, _) => k,
                    SettingsRowTarget::Core(_) => SettingsDirKind::SampleDirs,
                };
                self.settings_ui.editing = Some((kind, String::new()));
                self.status = match kind {
                    SettingsDirKind::SampleDirs => {
                        "Type a sample directory path · Enter adds · Esc cancels".into()
                    }
                    SettingsDirKind::PresetDirs => {
                        "Type a preset directory path · Enter adds · Esc cancels".into()
                    }
                };
            }
            KeyCode::Char('x') | KeyCode::Backspace => {
                if let SettingsRowTarget::DirEntry(kind, idx) = active {
                    let removed = self.remove_config_dir(kind, idx);
                    return Ok(Outcome {
                        samples_changed: removed && kind == SettingsDirKind::SampleDirs,
                        ..Outcome::default()
                    });
                }
            }
            KeyCode::Char('F') => {
                let req = match active {
                    SettingsRowTarget::AddDir(SettingsDirKind::SampleDirs)
                    | SettingsRowTarget::DirEntry(SettingsDirKind::SampleDirs, _) => {
                        Some(FileDialogRequest::PickSampleDir)
                    }
                    SettingsRowTarget::AddDir(SettingsDirKind::PresetDirs)
                    | SettingsRowTarget::DirEntry(SettingsDirKind::PresetDirs, _) => {
                        Some(FileDialogRequest::PickPresetDir)
                    }
                    SettingsRowTarget::Core(_) => None,
                };
                if let Some(req) = req {
                    self.status = "Opening native folder picker…".into();
                    return Ok(Outcome {
                        file_dialog: Some(req),
                        ..Outcome::default()
                    });
                }
            }
            KeyCode::Char('r') => self.reset_config(),
            KeyCode::Char('s') => {
                self.save_config()?;
            }
            KeyCode::Enter => match active {
                SettingsRowTarget::Core(SettingsCoreRow::DefaultBpm) => {
                    return self.begin_settings_default_bpm_value_input(String::new());
                }
                SettingsRowTarget::Core(SettingsCoreRow::DefaultRoot) => {
                    return self.begin_settings_default_root_value_input(String::new());
                }
                SettingsRowTarget::Core(SettingsCoreRow::LateTolerance) => {
                    return self.begin_late_tolerance_value_input(String::new(), Mode::Settings);
                }
                SettingsRowTarget::Core(_) => {
                    self.adjust_setting(1, false);
                }
                SettingsRowTarget::AddDir(kind) => {
                    self.settings_ui.editing = Some((kind, String::new()));
                    self.status = "Type directory path · Enter adds · Esc cancels".into();
                }
                SettingsRowTarget::DirEntry(_, _) => {
                    self.save_config()?;
                }
            },
            _ => {}
        }
        Ok(Outcome::default())
    }

    pub(super) fn handle_inspect_overlay(&mut self, key: KeyEvent) -> Result<Outcome> {
        if self.input.mode == Mode::Assets
            && key.code == KeyCode::Char('r')
            && key.kind != KeyEventKind::Repeat
        {
            return self.apply(Action::ReloadSamples);
        }
        let max_scroll = self.inspect_overlay_lines().len().saturating_sub(1);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.help_ui.overlay_scroll = (self.help_ui.overlay_scroll + 1).min(max_scroll);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.help_ui.overlay_scroll = self.help_ui.overlay_scroll.saturating_sub(1);
            }
            KeyCode::Char('0') => self.help_ui.overlay_scroll = 0,
            KeyCode::Char('$' | 'G') | KeyCode::End => self.help_ui.overlay_scroll = max_scroll,
            KeyCode::PageUp => {
                self.help_ui.overlay_scroll = self.help_ui.overlay_scroll.saturating_sub(12);
            }
            KeyCode::PageDown => {
                self.help_ui.overlay_scroll = (self.help_ui.overlay_scroll + 12).min(max_scroll);
            }
            _ => {}
        }
        Ok(Outcome::default())
    }

    pub(super) fn handle_tutorial_overlay(&mut self, key: KeyEvent) -> Result<Outcome> {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.help_ui.tutorial_step =
                    (self.help_ui.tutorial_step + 1).min(TUTORIAL.len().saturating_sub(1))
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.help_ui.tutorial_step = self.help_ui.tutorial_step.saturating_sub(1)
            }
            KeyCode::Enter => {
                self.open_palette(
                    TUTORIAL[self.help_ui.tutorial_step].2.into(),
                    Some(Mode::Tutorial),
                );
            }
            _ => {}
        }
        Ok(Outcome::default())
    }

    pub(super) fn handle_confirm_overlay(
        &mut self,
        action: ConfirmAction,
        key: KeyEvent,
    ) -> Result<Outcome> {
        if key.kind == KeyEventKind::Repeat {
            return Ok(Outcome::default());
        }
        match key.code {
            KeyCode::Char('s') if matches!(action, ConfirmAction::Quit) && self.dirty() => {
                self.input.mode = Mode::Normal;
                if self.doc.is_untitled() {
                    self.open_file_hub(FileHubTab::Save);
                    return Ok(Outcome::default());
                }
                let mut out = self.apply(Action::Save {
                    path: self.doc.save_path().clone(),
                })?;
                out.quit = true;
                Ok(out)
            }
            KeyCode::Char('y') | KeyCode::Enter => {
                self.input.mode = Mode::Normal;
                match action {
                    ConfirmAction::Quit => Ok(Outcome {
                        quit: true,
                        ..Outcome::default()
                    }),
                    ConfirmAction::New(title) => self.apply(Action::New { title }),
                    ConfirmAction::Open(path) => self.apply(Action::Load { path }),
                    ConfirmAction::ImportRbs(path) => self.apply(Action::ImportRbs { path }),
                    ConfirmAction::DeleteChannel(channel) => {
                        self.apply(Action::DeleteChannel { channel })
                    }
                    ConfirmAction::DeleteEffect(device) => {
                        self.apply(Action::DeleteEffect { device })
                    }
                    ConfirmAction::DeletePattern(pattern) => self.delete_pattern_safe(pattern),
                    ConfirmAction::DeletePart(part) => self.delete_part_safe(part),
                    ConfirmAction::DeleteLane(lane) => self.delete_lane_safe(lane),
                    ConfirmAction::DeleteSection(section) => self.delete_section_confirmed(section),
                    ConfirmAction::DeleteInstrument(instrument) => {
                        self.apply(Action::DeleteInstrument { instrument })
                    }
                    ConfirmAction::DiscardTake => self.apply(Action::DiscardTake),
                    ConfirmAction::ClearMidiMappings => Ok(self.clear_user_midi_mappings()),
                    ConfirmAction::SwitchMidiProfile(target_id) => {
                        Ok(self.apply_builtin_midi_profile(&target_id, true))
                    }
                }
            }
            _ => Ok(Outcome::default()),
        }
    }
}
