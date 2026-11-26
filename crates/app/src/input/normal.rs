//! Normal-mode deck-gated key handling.
//!
//! Strict invariant: in `Mode::Normal`, `h`/`j`/`k`/`l` (and Arrow keys) ONLY
//! navigate/select and NEVER mutate audio parameters, faders, knobs, notes, or placements.
use super::*;

impl App {
    pub(super) fn handle_normal_key(&mut self, key: KeyEvent) -> Result<Outcome> {
        if self.macros_focused()
            && self.input.count.is_none()
            && matches!(key.code, KeyCode::Char('1'..='8'))
        {
            if let Some(result) = self.handle_macros_deck_key(key, 1) {
                return result;
            }
        }
        if self.count_digit(key) {
            return Ok(Outcome::default());
        }
        let had_count = self.input.count.is_some();
        let count = self.input.count.take().unwrap_or(1);
        let repeat = key.kind == KeyEventKind::Repeat;
        if repeat
            && matches!(
                key.code,
                KeyCode::Char(
                    'm' | 's'
                        | 'x'
                        | 'X'
                        | 'Z'
                        | 'z'
                        | ' '
                        | 'p'
                        | 'P'
                        | 't'
                        | 'g'
                        | 'n'
                        | 'd'
                        | 'r'
                        | 'c'
                        | 'e'
                        | 'f'
                        | 'v'
                        | '\\'
                        | 'a'
                        | 'A'
                        | 'C'
                        | '?'
                        | 'M'
                        | 'R'
                        | 'S'
                        | 'B'
                        | 'D'
                        | 'F'
                        | 'K'
                        | 'N'
                        | 'T'
                        | 'W'
                        | 'i'
                        | 'I'
                        | 'o'
                        | ';'
                        | '\''
                ) | KeyCode::Enter
            )
        {
            return Ok(Outcome::default());
        }
        if self.midi.learn.is_some() && matches!(key.code, KeyCode::Char('x') | KeyCode::Backspace)
        {
            return Ok(self.unlearn_midi_target(None));
        }
        if key.modifiers.contains(KeyModifiers::ALT) {
            self.suspend_follow_for_key(key);
            return self.handle_direct_manipulate(key, count);
        }
        if self.take_focused() {
            if let Some(result) = self.handle_take_key(key, count) {
                return result;
            }
        }
        if self.graph_focused() {
            if let Some(result) = self.handle_graph_key(key, count) {
                return result;
            }
        }
        if self.midi_focused() {
            if let Some(result) = self.handle_devices_deck_key(key, count) {
                return result;
            }
        }
        if self.macros_focused() {
            if let Some(result) = self.handle_macros_deck_key(key, count) {
                return result;
            }
        }
        if self.bank_focused() {
            if let Some(result) = self.handle_bank_key(key, count) {
                return result;
            }
        }
        if self.sample_deck_focused() {
            if let Some(result) = self.handle_sample_deck_key(key, count) {
                return result;
            }
        }
        if self.deck_focused() {
            match self.detail {
                Detail::Placement(id) => {
                    if let Some(result) = self.placement_verb(id, key, count) {
                        return result;
                    }
                }
                Detail::Part(id) => {
                    if let Some(result) = self.part_verb(id, key, count) {
                        return result;
                    }
                }
                Detail::Instrument(id) => {
                    if key.code == KeyCode::Char('x') && self.rack_ui.tab != RackTab::Patchbay {
                        if let Some(result) = self.unlink_focused_control_link() {
                            return result;
                        }
                    }
                    if let Some(result) = self.handle_rack_deck_key(id, key, count) {
                        return result;
                    }
                }
                _ => {}
            }
        }
        self.suspend_follow_for_key(key);
        match key.code {
            KeyCode::Char('\'') => {
                self.from_here = !self.from_here;
                let bar = if self.transport.playing {
                    self.song_step_heard() / 16 + 1
                } else {
                    u64::from(self.arrange_ui.step / 16 + 1)
                };
                self.status = if self.from_here {
                    format!("' From here armed · next pattern edit forks from bar {bar}")
                } else {
                    "From here cancelled".into()
                };
                return Ok(Outcome::default());
            }
            KeyCode::Char('v') if self.arrange_canvas_focused() => {
                self.input.mode = Mode::ArrangeViews;
            }
            KeyCode::Char('g') => self.input.mode = Mode::Go,
            KeyCode::Char('t') => self.input.mode = Mode::Transport,
            KeyCode::Char('z') => self.input.mode = Mode::Window,
            KeyCode::Char(':') => self.open_palette(String::new(), None),
            KeyCode::Char('=' | '+')
                if self.arrange_canvas_focused()
                    || self.pattern_canvas_focused()
                    || self.sample_deck_focused() =>
            {
                self.zoom_view(1);
            }
            KeyCode::Char('-')
                if self.arrange_canvas_focused()
                    || self.pattern_canvas_focused()
                    || self.sample_deck_focused() =>
            {
                self.zoom_view(-1);
            }
            KeyCode::Char('?') => {
                self.input.mode = Mode::Help;
                self.help_ui.overlay_scroll = 0;
            }
            KeyCode::Char('/') => {
                let origin = Some(self.navigation());
                self.modal = ActiveModal::Search { origin };
                self.view_state.bank_mode = BankMode::Material;
                self.show_deck(layout::Deck::song(layout::DeckKind::Bank));
                self.input.mode = Mode::Search(String::new());
            }
            KeyCode::Char(' ') => {
                return self.apply(Action::Play {
                    playing: !self.transport.playing,
                })
            }
            KeyCode::Char('u') => {
                if self.staging.as_ref().is_some_and(|s| !s.is_empty()) {
                    return self.pop_staged();
                }
                return self.apply(Action::Undo);
            }
            KeyCode::Char('U') => return self.apply(Action::Redo),
            KeyCode::Char('P')
                if self.deck_focused() && matches!(self.detail, Detail::Instrument(_)) =>
            {
                return self.open_preset_browser();
            }
            KeyCode::Char('P') if self.is_filter_effect_detail() => {
                return self.open_groove_browser();
            }
            KeyCode::Char('f') if self.is_filter_effect_detail() => {
                return self.fork_active_filter_groove();
            }
            KeyCode::Char('S') => return self.toggle_stage_key(),
            KeyCode::Char('M') => return Ok(self.toggle_midi_learn()),
            KeyCode::Char('R') => return Ok(self.toggle_midi_record()),
            // Tier 4: one-hand scalar adjustment (,/. fine, </> coarse) + `;` repeat last edit
            KeyCode::Char(',') => return self.handle_scalar_adjust(-1, false, repeat, count),
            KeyCode::Char('.') => return self.handle_scalar_adjust(1, false, repeat, count),
            KeyCode::Char('<') => return self.handle_scalar_adjust(-1, true, repeat, count),
            KeyCode::Char('>') => return self.handle_scalar_adjust(1, true, repeat, count),
            KeyCode::Char(';') => return self.repeat_last(),
            KeyCode::Tab => self.cycle_focus(),
            KeyCode::BackTab => self.cycle_decks(false),
            KeyCode::Enter => return self.open_current(),
            KeyCode::Char('i') if self.pattern_canvas_focused() => {
                ensure!(
                    self.can_edit_current_pattern_notes(),
                    "Open a literal note source (Enter/gs) or local occurrence (o from Placement)"
                );
                self.clamp_note_voice();
                self.input.mode = Mode::Insert;
                self.status = "STEP INSERT · 1–8 degree · z–, notes · . rest · Esc normal".into();
                return Ok(Outcome::default());
            }
            KeyCode::Char('o') if self.pattern_canvas_focused() => {
                return self.stack_chord_voice();
            }
            KeyCode::Char('[')
                if self.pattern_canvas_focused() && self.can_edit_current_pattern_notes() =>
            {
                return self.edit_selected_voice_accidental(-1);
            }
            KeyCode::Char(']' | '#')
                if self.pattern_canvas_focused() && self.can_edit_current_pattern_notes() =>
            {
                return self.edit_selected_voice_accidental(1);
            }
            KeyCode::Char('c')
                if self.pattern_canvas_focused() && self.can_edit_current_pattern_notes() =>
            {
                return self.cycle_step_condition(1);
            }
            KeyCode::Char('C')
                if self.pattern_canvas_focused() && self.can_edit_current_pattern_notes() =>
            {
                return self.cycle_step_condition(-1);
            }
            KeyCode::Char('m')
                if self.pattern_canvas_focused() && self.can_edit_current_pattern_notes() =>
            {
                return self.toggle_material();
            }
            KeyCode::Char('C')
                if self.mixer_canvas_focused()
                    || (self.deck_focused() && matches!(self.detail, Detail::Channel(_))) =>
            {
                if repeat {
                    return Ok(Outcome::default());
                }
                let channel = match self.detail {
                    Detail::Channel(ch) if self.deck_focused() => Some(ch),
                    _ => Some(self.selected_channel),
                };
                return self.apply(Action::ClearMeters { channel });
            }
            KeyCode::Char('b') if self.mixer_canvas_focused() => {
                if repeat {
                    return Ok(Outcome::default());
                }
                self.clamp_mixer_slot();
                if let MixerSlot::Insert(idx) = self.view_state.mixer_slot {
                    if let Some(device) = self.mixer_focused_insert(idx) {
                        return self.apply(Action::SetEffectBypass {
                            device,
                            bypassed: !self.effect(device).context("Effect missing")?.1.bypassed,
                        });
                    }
                }
            }
            KeyCode::Char('b')
                if self.deck_focused() && matches!(self.detail, Detail::Channel(_)) =>
            {
                if let Some(&ControlTarget::Effect(device)) = self
                    .control_descriptors()
                    .get(self.control_cursor)
                    .map(|c| &c.target)
                {
                    if repeat {
                        return Ok(Outcome::default());
                    }
                    return self.apply(Action::SetEffectBypass {
                        device,
                        bypassed: !self.effect(device).context("Effect missing")?.1.bypassed,
                    });
                }
                self.jump_control_group(-1);
                return Ok(Outcome::default());
            }
            KeyCode::Char('v') if self.mixer_canvas_focused() => {
                self.clamp_mixer_slot();
                if let MixerSlot::Insert(idx) = self.view_state.mixer_slot {
                    if let Some(device) = self.mixer_focused_insert(idx) {
                        return self.begin_workflow(WorkflowKind::MoveEffect(device));
                    }
                }
            }
            KeyCode::Char('o') if self.mixer_canvas_focused() => {
                self.open_channel(self.selected_channel)?;
                return Ok(Outcome::default());
            }
            KeyCode::Char('n') => return self.begin_create(),
            KeyCode::Char('B') => return self.browse_samples(),
            KeyCode::Char('d')
                if self.pattern_deck_focused()
                    || (self.bank_focused() && self.view_state.bank_mode == BankMode::Material) =>
            {
                return self.begin_authoring(Draft::derive(self.selected_pattern));
            }
            KeyCode::Char('r')
                if self.pattern_deck_focused()
                    || (self.bank_focused() && self.view_state.bank_mode == BankMode::Material) =>
            {
                let id = self.selected_pattern.context("Select a pattern to edit")?;
                return self.begin_authoring(Draft::edit(self.doc.project(), id)?);
            }
            KeyCode::Char('F')
                if self.pattern_deck_focused()
                    || (self.bank_focused() && self.view_state.bank_mode == BankMode::Material) =>
            {
                let id = self
                    .selected_pattern
                    .context("Select a pattern to freeze")?;
                let pat = self.pattern(id).context("Pattern missing")?;
                if matches!(
                    pat.kind,
                    PatternKind::Notes { .. }
                        | PatternKind::Rhythm { .. }
                        | PatternKind::Values { .. }
                ) {
                    self.status = format!("Pattern '{}' is already literal", pat.name);
                    return Ok(Outcome::default());
                }
                return self.begin_workflow(WorkflowKind::FreezePattern(id));
            }
            KeyCode::Char('f')
                if self.pattern_deck_focused()
                    || (self.bank_focused() && self.view_state.bank_mode == BankMode::Material) =>
            {
                let id = self.selected_pattern.context("Select a pattern to fork")?;
                let source = self.pattern(id).context("Pattern missing")?;
                let name = crate::unique_name(
                    &format!("{} fork", source.name),
                    self.doc.project().patterns.iter().map(|p| p.name.as_str()),
                    None,
                );
                return self.apply(Action::Fork { source: id, name });
            }
            KeyCode::Char('I')
                if self.pattern_deck_focused()
                    || (self.bank_focused() && self.view_state.bank_mode == BankMode::Material) =>
            {
                self.help_ui.overlay_scroll = 0;
                self.input.mode = Mode::Inspect;
                self.status = "Inspect · j/k scroll · Esc close".into();
                return Ok(Outcome::default());
            }
            KeyCode::Char('c')
                if self.mixer_canvas_focused()
                    || (self.deck_focused() && matches!(self.detail, Detail::Channel(_))) =>
            {
                return self.begin_workflow(WorkflowKind::Channel);
            }
            KeyCode::Char('f')
                if self.mixer_canvas_focused()
                    || (self.deck_focused() && matches!(self.detail, Detail::Channel(_))) =>
            {
                return self.begin_workflow(WorkflowKind::Effect);
            }
            KeyCode::Char('a')
                if self.mixer_canvas_focused()
                    || (self.deck_focused() && matches!(self.detail, Detail::Channel(_))) =>
            {
                let channel = match self.detail {
                    Detail::Channel(ch) if self.deck_focused() => ch,
                    _ => self.selected_channel,
                };
                return self.open_mixer_add_picker(channel);
            }
            KeyCode::Char('w') if self.mixer_canvas_focused() => {
                return self.begin_workflow(WorkflowKind::Send);
            }
            KeyCode::Char('e') => return self.open_focused_links_menu(),
            KeyCode::Char('a') if self.deck_focused() && matches!(self.detail, Detail::Part(_)) => {
                if let Detail::Part(part_id) = self.detail {
                    return self.open_add_part_param_link_picker(part_id);
                }
            }
            KeyCode::Char('d') if self.arrange_canvas_focused() => {
                return self.duplicate_selected_placement();
            }
            KeyCode::Char('d')
                if self.mixer_canvas_focused()
                    || (self.deck_focused() && matches!(self.detail, Detail::Channel(_))) =>
            {
                return self.begin_workflow(WorkflowKind::Duck);
            }
            KeyCode::Char('r') if self.mixer_canvas_focused() => {
                self.clamp_mixer_slot();
                match self.view_state.mixer_slot {
                    MixerSlot::Insert(idx) => {
                        if let Some(device) = self.mixer_focused_insert(idx) {
                            return self.begin_workflow(WorkflowKind::RenameEffect(device));
                        }
                    }
                    MixerSlot::Duck(idx) => {
                        if let Some(id) = self.mixer_focused_duck(idx) {
                            return self.begin_workflow(WorkflowKind::EditDuck(id));
                        }
                    }
                    _ => {}
                }
                let ChannelRef::Channel(id) = self.selected_channel else {
                    bail!("Master channel cannot be renamed");
                };
                return self.begin_workflow(WorkflowKind::RenameChannel(id));
            }
            KeyCode::Char('r')
                if self.deck_focused() && matches!(self.detail, Detail::Channel(_)) =>
            {
                if let Some(ctrl) = self.control_descriptors().get(self.control_cursor) {
                    match ctrl.target {
                        ControlTarget::Effect(device) => {
                            return self.begin_workflow(WorkflowKind::RenameEffect(device));
                        }
                        ControlTarget::Duck(id)
                        | ControlTarget::DuckDepth(id)
                        | ControlTarget::DuckRecovery(id) => {
                            return self.begin_workflow(WorkflowKind::EditDuck(id));
                        }
                        _ => {}
                    }
                }
                let Detail::Channel(ChannelRef::Channel(id)) = self.detail else {
                    bail!("Master channel cannot be renamed");
                };
                return self.begin_workflow(WorkflowKind::RenameChannel(id));
            }
            KeyCode::Char('r')
                if self.deck_focused() && matches!(self.detail, Detail::Effect(_)) =>
            {
                if let Detail::Effect(id) = self.detail {
                    return self.begin_workflow(WorkflowKind::RenameEffect(id));
                }
            }
            KeyCode::Char('v')
                if self.deck_focused()
                    && matches!(self.detail, Detail::Effect(_) | Detail::Channel(_)) =>
            {
                if let Some(id) = self.focused_effect_for_reorder() {
                    return self.begin_workflow(WorkflowKind::MoveEffect(id));
                }
            }
            KeyCode::Char('r') if self.arrange_canvas_focused() => {
                if let Some(sec) = self.section_at_cursor() {
                    return self.begin_workflow(WorkflowKind::EditSection(sec.id));
                }
                if let Some(lane_id) = self.selected_lane {
                    return self.begin_workflow(WorkflowKind::RenameLane(lane_id));
                }
                bail!("No section or lane selected to edit");
            }
            KeyCode::Char('X') if self.arrange_canvas_focused() => {
                if let Some(sec) = self.section_at_cursor() {
                    return self.request_delete_section(sec.id);
                }
                if let Some(lane_id) = self.selected_lane {
                    return self.request_delete_lane(lane_id);
                }
                bail!("No section or lane selected to delete");
            }
            KeyCode::Char('X') if self.pattern_canvas_focused() => {
                if self.can_edit_current_pattern_notes() {
                    return self.clear_current_step();
                }
                if let Some(id) = self.selected_pattern {
                    return self.request_delete_pattern(id);
                }
            }
            KeyCode::Char('X')
                if self.bank_focused() && self.view_state.bank_mode == BankMode::Material =>
            {
                let id = self
                    .selected_pattern
                    .context("Select a pattern to delete")?;
                return self.request_delete_pattern(id);
            }
            KeyCode::Char('X') if self.mixer_canvas_focused() => {
                let ChannelRef::Channel(id) = self.selected_channel else {
                    bail!("Master channel cannot be deleted");
                };
                return self.request_delete_channel(id);
            }
            KeyCode::Char('X') if self.deck_focused() => match self.detail {
                Detail::Channel(ChannelRef::Channel(id)) => {
                    return self.request_delete_channel(id);
                }
                Detail::Channel(ChannelRef::Master) => {
                    bail!("Master channel cannot be deleted");
                }
                Detail::Effect(id) => {
                    return self.request_delete_effect(id);
                }
                _ => {}
            },
            KeyCode::Char('A') if self.deck_focused() && matches!(self.detail, Detail::Part(_)) => {
                if let Detail::Part(part_id) = self.detail {
                    if let Some(part) = self.part(part_id) {
                        let pat = part.pattern;
                        if self.audition_pattern() == Some(pat) {
                            return self.stop_audition();
                        }
                        return self.start_audition(pat, None);
                    }
                }
            }
            KeyCode::Char('A')
                if self.pattern_deck_focused()
                    || (self.bank_focused() && self.view_state.bank_mode == BankMode::Material) =>
            {
                if self.is_auditioning() {
                    return self.stop_audition();
                }
                return self.start_audition(
                    self.selected_pattern.context("Select a note pattern")?,
                    None,
                );
            }
            KeyCode::Char('h') | KeyCode::Left => {
                if self.bank_focused() {
                    self.move_deck_focus(layout::Direction::Left);
                } else if self.mixer_canvas_focused() {
                    self.move_selection(-(count as isize));
                } else if self.arrange_canvas_focused() {
                    let count = count.min(u32::MAX as usize / 16) as u32;
                    let stride = if self.arrange_ui.view == ArrangeView::Placements {
                        16 * count
                    } else {
                        count
                    };
                    self.arrange_ui.step = self.arrange_ui.step.saturating_sub(stride);
                    self.select_at_cursor();
                } else if self.pattern_canvas_focused() {
                    self.cursor = self.cursor.saturating_sub(count);
                    self.clamp_note_voice();
                }
            }
            KeyCode::Char('l') | KeyCode::Right => {
                if self.bank_focused() {
                    return self.open_current();
                } else if self.mixer_canvas_focused() {
                    self.move_selection(count as isize);
                } else if self.arrange_canvas_focused() {
                    let count = count.min(u32::MAX as usize / 16) as u32;
                    let stride = if self.arrange_ui.view == ArrangeView::Placements {
                        16 * count
                    } else {
                        count
                    };
                    self.arrange_ui.step = self.arrange_ui.step.saturating_add(stride).min(
                        (self.doc.project().bars as u32)
                            .saturating_mul(16)
                            .saturating_sub(1),
                    );
                    self.select_at_cursor();
                } else if self.pattern_canvas_focused() {
                    self.cursor = self
                        .cursor
                        .saturating_add(count)
                        .min(self.current_length().saturating_sub(1));
                    self.clamp_note_voice();
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if self.value_canvas() {
                    return self.move_point(true, count);
                }
                if self.mixer_canvas_focused() {
                    self.step_mixer_slot(count as isize, false);
                } else if self.pattern_canvas_focused() {
                    if self.can_edit_current_pattern_notes() {
                        self.step_note_voice(-(count as isize), false);
                    } else if self.scope == EditScope::Definition {
                        self.step_pattern_source(count as isize);
                    }
                } else {
                    self.move_selection(count as isize);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.value_canvas() {
                    return self.move_point(false, count);
                }
                if self.mixer_canvas_focused() {
                    self.step_mixer_slot(-(count as isize), false);
                } else if self.pattern_canvas_focused() {
                    if self.can_edit_current_pattern_notes() {
                        self.step_note_voice(count as isize, false);
                    } else if self.scope == EditScope::Definition {
                        self.step_pattern_source(-(count as isize));
                    }
                } else {
                    self.move_selection(-(count as isize));
                }
            }
            KeyCode::Char('H') => {
                if self.pattern_canvas_focused() {
                    self.cursor = self.cursor.saturating_sub(16 * count);
                    self.clamp_note_voice();
                } else if self.arrange_canvas_focused() {
                    self.jump_section(false, count);
                } else if self.mixer_canvas_focused() {
                    self.move_selection(
                        -(self
                            .view_state
                            .mixer_page_size
                            .max(1)
                            .saturating_mul(count)
                            .min(isize::MAX as usize) as isize),
                    );
                }
            }
            KeyCode::Char('L') => {
                if self.pattern_canvas_focused() {
                    self.cursor = self
                        .cursor
                        .saturating_add(16 * count)
                        .min(self.current_length().saturating_sub(1));
                    self.clamp_note_voice();
                } else if self.arrange_canvas_focused() {
                    self.jump_section(true, count);
                } else if self.mixer_canvas_focused() {
                    self.move_selection(
                        self.view_state
                            .mixer_page_size
                            .max(1)
                            .saturating_mul(count)
                            .min(isize::MAX as usize) as isize,
                    );
                }
            }
            KeyCode::Char('w') => {
                if self.deck_focused() {
                    self.jump_control_group(1);
                } else if self.evaluated_arrange_canvas() {
                    return self.move_arrangement_event(true, count);
                } else if self.arrange_canvas_focused() {
                    self.jump_section(true, count);
                } else if self.pattern_canvas_focused() {
                    return self.move_point(true, count);
                }
            }
            KeyCode::Char('b') => {
                if self.deck_focused() {
                    self.jump_control_group(-1);
                } else if self.evaluated_arrange_canvas() {
                    return self.move_arrangement_event(false, count);
                } else if self.arrange_canvas_focused() {
                    self.jump_section(false, count);
                } else if self.pattern_canvas_focused() {
                    return self.move_point(false, count);
                }
            }
            KeyCode::PageDown => return self.navigate_page(true, count),
            KeyCode::PageUp => return self.navigate_page(false, count),
            KeyCode::Char('0') => self.navigate_edge(false),
            KeyCode::Char('$' | 'G') | KeyCode::End => self.navigate_edge(true),
            KeyCode::Char('x') => {
                if self.value_canvas() {
                    return self.delete_value_point();
                }
                if self.deck_focused() {
                    if let Some(result) = self.unlink_focused_control_link() {
                        return result;
                    }
                }
                if let Detail::Effect(_) = if self.deck_focused() {
                    self.detail
                } else {
                    Detail::None
                } {
                    self.status = "Press m to toggle bypass · X to delete effect".into();
                    return Ok(Outcome::default());
                }
                if self.deck_focused() && matches!(self.detail, Detail::Channel(_)) {
                    if let Some(ctrl) = self.control_descriptors().get(self.control_cursor).cloned()
                    {
                        match ctrl.target {
                            ControlTarget::Effect(device) => {
                                return self.apply(Action::DeleteEffect { device });
                            }
                            ControlTarget::Param(ParamAddress::Send {
                                channel: ChannelRef::Channel(ch),
                                target,
                            }) => {
                                return self.apply(Action::SetChannelSend {
                                    channel: ch,
                                    target,
                                    amount: None,
                                });
                            }
                            ControlTarget::Duck(id)
                            | ControlTarget::DuckDepth(id)
                            | ControlTarget::DuckRecovery(id) => {
                                let out = self.apply(Action::Edit {
                                    edit: Edit::RemoveDuck { id },
                                })?;
                                self.status = "Deleted duck · u restores it".into();
                                return Ok(out);
                            }
                            _ => {}
                        }
                    }
                }
                if self.mixer_canvas_focused() {
                    self.clamp_mixer_slot();
                    match self.view_state.mixer_slot {
                        MixerSlot::Insert(idx) => {
                            if let Some(device) = self.mixer_focused_insert(idx) {
                                let out = self.apply(Action::DeleteEffect { device })?;
                                self.clamp_mixer_slot();
                                return Ok(out);
                            }
                        }
                        MixerSlot::Send(idx) => {
                            if let Some((ch, target)) = self.mixer_focused_send(idx) {
                                let out = self.apply(Action::SetChannelSend {
                                    channel: ch,
                                    target,
                                    amount: None,
                                })?;
                                self.clamp_mixer_slot();
                                return Ok(out);
                            }
                        }
                        MixerSlot::Duck(idx) => {
                            if let Some(id) = self.mixer_focused_duck(idx) {
                                let out = self.apply(Action::Edit {
                                    edit: Edit::RemoveDuck { id },
                                })?;
                                self.clamp_mixer_slot();
                                self.status = "Deleted duck · u restores it".into();
                                return Ok(out);
                            }
                        }
                        MixerSlot::Fader | MixerSlot::Pan | MixerSlot::Output => {
                            self.status =
                                "Press X to delete channel · j/k selects Insert/Send/Duck to remove"
                                    .into();
                        }
                    }
                    return Ok(Outcome::default());
                }
                if self.arrange_canvas_focused() {
                    if let Some(lane) = self.selected_lane {
                        if self.live.lane_state(lane).is_some()
                            || self.live.pending_for_lane(lane).next().is_some()
                            || self.staged_launch_on_lane(lane).is_some()
                        {
                            return self.apply(Action::Release { lane, at: None });
                        }
                    }
                    if let Some(id) = self.selected_placement {
                        let out = self.apply(Action::Edit {
                            edit: Edit::RemovePlacement { id },
                        })?;
                        self.select_at_cursor();
                        self.status = "Deleted placement · u restores it".into();
                        return Ok(out);
                    }
                }
                if self.pattern_canvas_focused() {
                    if self.can_edit_current_pattern_notes() {
                        return self.delete_selected_voice();
                    }
                    if self
                        .selected_pattern_object()
                        .is_some_and(|p| !p.kind.sources().is_empty())
                    {
                        self.status =
                            "Derived pattern · j/k selects source · Enter/gs opens".into();
                        return Ok(Outcome::default());
                    }
                    return self.toggle_material();
                }
            }
            KeyCode::Char('m') if self.value_canvas() => return self.begin_value_mode(),
            KeyCode::Char('m') if self.deck_focused() => match self.detail {
                Detail::Effect(id) => {
                    return self.apply(Action::SetEffectBypass {
                        device: id,
                        bypassed: !self.effect(id).context("Effect missing")?.1.bypassed,
                    });
                }
                Detail::Channel(channel) => {
                    return self.apply(Action::SetChannelMuted {
                        channel,
                        muted: !self.doc.flags().channel_muted(channel),
                    });
                }
                _ => {}
            },
            KeyCode::Char('s') if self.deck_focused() => {
                if let Detail::Channel(channel) = self.detail {
                    if channel != ChannelRef::Master {
                        return self.apply(Action::SetChannelSolo {
                            channel,
                            solo: !self.doc.flags().channel_solo(channel),
                        });
                    }
                }
            }
            KeyCode::Char('m') if self.mixer_canvas_focused() => {
                let channel = self.selected_channel;
                return self.apply(Action::SetChannelMuted {
                    channel,
                    muted: !self.doc.flags().channel_muted(channel),
                });
            }
            KeyCode::Char('s') if self.mixer_canvas_focused() => {
                let channel = self.selected_channel;
                if channel != ChannelRef::Master {
                    return self.apply(Action::SetChannelSolo {
                        channel,
                        solo: !self.doc.flags().channel_solo(channel),
                    });
                }
            }
            KeyCode::Char('m') if self.arrange_canvas_focused() => {
                if let Some(lane) = self.selected_lane {
                    return self.apply(Action::SetLaneMuted {
                        lane,
                        muted: !self.effective_lane_muted(lane),
                    });
                }
            }
            KeyCode::Char('s') if self.arrange_canvas_focused() => {
                if let Some(lane) = self.selected_lane {
                    return self.apply(Action::SetLaneSolo {
                        lane,
                        solo: !self.effective_lane_solo(lane),
                    });
                }
            }
            KeyCode::Char('\\') => return self.toggle_song_loop(),
            KeyCode::Char('[') if self.arrange_canvas_focused() => {
                return self.set_loop_edge(false)
            }
            KeyCode::Char(']') if self.arrange_canvas_focused() => return self.set_loop_edge(true),
            KeyCode::Char('y')
                if (self.arrange_canvas_focused()
                    && self.arrange_ui.view == ArrangeView::Placements)
                    || self.pattern_canvas_focused() =>
            {
                return self.yank(count);
            }
            KeyCode::Char('p')
                if (self.arrange_canvas_focused()
                    && self.arrange_ui.view == ArrangeView::Placements)
                    || self.pattern_canvas_focused() =>
            {
                return self.paste(true);
            }
            KeyCode::Char('P')
                if (self.arrange_canvas_focused()
                    && self.arrange_ui.view == ArrangeView::Placements)
                    || self.pattern_canvas_focused() =>
            {
                return self.paste(false);
            }
            KeyCode::Backspace => {
                if self.pattern_canvas_focused() && matches!(self.scope, EditScope::Placement(_)) {
                    return self.clear_placement_step_exception();
                }
                if self
                    .live
                    .pending
                    .iter()
                    .any(|p| !matches!(p.change, cadence_core::live::Change::Song { .. }))
                {
                    return self.apply(Action::CancelPending { all: false });
                }
                return self.back();
            }
            _ => {}
        }
        self.normalize_focus();
        if had_count && self.status.starts_with("Count ") {
            self.status = match self.focused_canvas() {
                Some(Workspace::Pattern) => {
                    format!("Step {} / offset {}", self.cursor + 1, self.cursor)
                }
                Some(Workspace::Mixer) => {
                    format!("Selected {}", self.channel_name(self.selected_channel))
                }
                _ => "Count cleared".into(),
            };
        }
        Ok(Outcome::default())
    }
}
