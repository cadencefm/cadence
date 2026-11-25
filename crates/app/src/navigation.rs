//! Selection, focus, the context deck and back navigation.
use super::*;

impl App {
    pub(super) fn navigation(&self) -> Navigation {
        Navigation {
            layout: self.layout.clone(),
            arrange: ArrangeNav {
                view: self.arrange_ui.view,
                step: self.arrange_ui.step,
                event: self.arrange_ui.event,
                pitch_anchor: self.arrange_ui.pitch_anchor,
                source_cursor: self.arrange_ui.source_cursor,
                inspecting: self.input.mode == Mode::ArrangementInspect,
            },
            selection: SelectionNav {
                scope: self.scope,
                pattern: self.selected_pattern,
                lane: self.selected_lane,
                placement: self.selected_placement,
                part: self.selected_part,
                instrument: self.selected_instrument,
                channel: self.selected_channel,
            },
            cursors: CursorNav {
                step: self.cursor,
                note_voice: self.note_voice,
                control: self.control_cursor,
                source: self.source_cursor,
                mixer_slot: self.view_state.mixer_slot,
                macro_binding: self.view_state.macro_binding_cursor,
            },
            rack_ui: self.rack_ui,
            query: self.query.clone(),
        }
    }
    pub(super) fn restore_navigation(&mut self, n: Navigation) {
        if self.selected_pattern != n.selection.pattern
            || self.scope != n.selection.scope
            || self.cursor != n.cursors.step
            || self.arrange_ui.step != n.arrange.step
            || self.selected_lane != n.selection.lane
            || self.selected_placement != n.selection.placement
        {
            self.suspend_follow();
        }
        self.arrange_ui.view = n.arrange.view;
        self.arrange_ui.step = n.arrange.step;
        self.arrange_ui.event = n.arrange.event;
        self.arrange_ui.pitch_anchor = n.arrange.pitch_anchor;
        self.arrange_ui.pitch_lane = n.selection.lane;
        self.arrange_ui.source_cursor = n.arrange.source_cursor;
        if n.arrange.inspecting {
            self.input.mode = Mode::ArrangementInspect;
        }
        self.scope = n.selection.scope;
        self.selected_pattern = n.selection.pattern;
        self.selected_lane = n.selection.lane;
        self.selected_placement = n.selection.placement;
        self.selected_part = n.selection.part;
        self.selected_instrument = n.selection.instrument;
        self.selected_channel = n.selection.channel;
        self.cursor = n.cursors.step;
        self.note_voice = n.cursors.note_voice;
        self.control_cursor = n.cursors.control;
        self.source_cursor = n.cursors.source;
        self.view_state.mixer_slot = n.cursors.mixer_slot;
        self.view_state.macro_binding_cursor = n.cursors.macro_binding;
        self.rack_ui = n.rack_ui;
        self.query = n.query;
        self.layout = n.layout;
        // Derive the view first, so selection repair sees the restored decks.
        self.sync_view();
        self.normalize_focus();
    }
    pub(super) fn push_back(&mut self) {
        self.back.push(self.navigation());
        if self.back.len() > 32 {
            self.back.remove(0);
        }
    }
    /// Open an object's deck in the working spot, or focus it where it is.
    pub(super) fn show_detail(&mut self, detail: Detail) {
        if let Some(deck) = Self::object_deck(detail) {
            self.show_deck(deck);
        }
    }
    pub(super) fn select_library_pattern(&mut self, pattern: Option<Id>) {
        if self.selected_pattern == pattern {
            return;
        }
        self.suspend_follow();
        if matches!(self.scope, EditScope::Placement(_)) && !self.modal.has_search_origin() {
            self.push_back();
        }
        self.selected_pattern = pattern;
        self.scope = EditScope::Definition;
        self.cursor = 0;
        self.source_cursor = 0;
    }
    pub(super) fn follow_deck(&mut self) {
        self.sync_view();
    }
    /// Open the selection's sound, part or channel in a deck, or close the
    /// object deck that has focus.
    pub(super) fn set_deck_visible(&mut self, visible: bool) -> Result<()> {
        if visible {
            // An object deck already on screen takes focus, pinned ones first.
            let layout = &self.layout;
            let shown = (0..layout.pinned().len())
                .map(layout::Spot::Pinned)
                .chain([layout::Spot::Working])
                .find(|spot| layout.deck(*spot).is_some_and(|d| d.kind.is_object()));
            if let Some(spot) = shown {
                if let Some(deck) = layout.deck(spot).copied() {
                    self.show_deck(deck);
                }
                return Ok(());
            }
            let target = if self.workspace() == Workspace::Mixer {
                Detail::Channel(self.selected_channel)
            } else if let Some(id) = self.selected_instrument {
                Detail::Instrument(id)
            } else if let Some(id) = self.selected_part {
                Detail::Part(id)
            } else {
                bail!("Select a sound, part or channel before opening a deck");
            };
            self.push_back();
            self.show_detail(target);
            self.status = "Deck open".into();
        } else if self.layout.focused().is_some_and(|d| d.kind.is_object()) {
            self.close_deck()?;
        }
        Ok(())
    }
    pub(super) fn pin_deck(&mut self, pinned: bool) -> Result<()> {
        let focused = self.layout.focus();
        if self.layout.is_pinned(focused) != pinned {
            self.toggle_deck_pin()?;
        }
        Ok(())
    }
    pub(super) fn cycle_focus(&mut self) {
        self.cycle_decks(true);
    }
    pub(super) fn zoom_active(&mut self) {
        self.toggle_deck_zoom();
    }

    pub(crate) fn step_pattern_source(&mut self, delta: isize) {
        let sources = self
            .selected_pattern_object()
            .map(|p| p.kind.sources())
            .unwrap_or_default();
        if sources.is_empty() {
            return;
        }
        let next =
            (self.source_cursor as isize + delta).clamp(0, sources.len() as isize - 1) as usize;
        self.source_cursor = next;
        if let Some(src) = self.pattern(sources[next]) {
            self.status = format!(
                "Source {}/{}: {} ({}) · Enter/gs opens",
                next + 1,
                sources.len(),
                src.name,
                kind_label(&src.kind),
            );
        }
    }
    pub(super) fn navigate_edge(&mut self, end: bool) {
        if matches!(
            self.focused_canvas(),
            Some(Workspace::Arrange | Workspace::Pattern)
        ) {
            self.suspend_follow();
        }
        match self.focused_deck_kind() {
            Some(layout::DeckKind::Bank) => {
                if self.view_state.bank_mode == BankMode::Parts {
                    let count = self.doc.project().lanes.len();
                    if count > 0 {
                        self.view_state.bank_lane_idx = if end { count - 1 } else { 0 };
                        self.sync_bank_part_selection();
                    }
                } else {
                    let patterns = self.visible_patterns();
                    let pattern = if end {
                        patterns.last()
                    } else {
                        patterns.first()
                    }
                    .map(|p| p.id);
                    self.select_library_pattern(pattern);
                }
            }
            Some(kind) if kind.is_object() => {
                if matches!(self.detail, Detail::Instrument(_))
                    && self.rack_ui.tab == RackTab::Patchbay
                {
                    if let Some(inst_id) = self.detail_instrument() {
                        if let Some(inst) = self.instrument(inst_id) {
                            let zone_idx = self.active_zone_index(&inst.rack);
                            if let Some(zone) = inst.rack.zones.get(zone_idx).cloned() {
                                self.rack_patchbay_navigate_edge(&zone, end);
                            }
                        }
                    }
                    return;
                }
                self.control_cursor = if end {
                    self.control_descriptors().len().saturating_sub(1)
                } else {
                    0
                };
            }
            Some(layout::DeckKind::Pattern) => {
                self.cursor = if end {
                    self.current_length().saturating_sub(1)
                } else {
                    0
                };
            }
            Some(layout::DeckKind::Arrange) => {
                self.arrange_ui.step = if end {
                    if self.arrange_ui.view == ArrangeView::Placements {
                        (self.doc.project().bars as u32 - 1) * 16
                    } else {
                        self.doc.project().arrangement_steps().saturating_sub(1) as u32
                    }
                } else {
                    0
                };
                self.select_at_cursor();
            }
            Some(layout::DeckKind::Mixer) => {
                let channels = self.channels();
                if let Some(&channel) = if end {
                    channels.last()
                } else {
                    channels.first()
                } {
                    self.selected_channel = channel;
                    self.clamp_mixer_slot();
                }
            }
            Some(layout::DeckKind::Midi) => {
                if self.devices.tab == DevicesTab::Audio {
                    let total = AudioDeckRow::ALL.len();
                    self.devices.cursor = if end { total.saturating_sub(1) } else { 0 };
                } else {
                    let total =
                        crate::midi::MidiDeckRow::COUNT + self.effective_midi_mappings().len();
                    self.midi.cursor = if end { total.saturating_sub(1) } else { 0 };
                }
            }
            Some(layout::DeckKind::Graph) => {
                let cols = self.graph_target_columns();
                let max_col = cols.len().saturating_sub(1);
                let len = cols
                    .get(self.view_state.graph_col.min(max_col))
                    .map_or(0, Vec::len);
                self.view_state.graph_row = if end { len.saturating_sub(1) } else { 0 };
            }
            Some(layout::DeckKind::Take) => {
                let total_bars = self
                    .current_take()
                    .map_or(0, |t| (t.steps() as usize).div_ceil(16));
                self.takes.cursor_bar = if end { total_bars.saturating_sub(1) } else { 0 };
                self.sync_take_visual_marks();
            }
            Some(layout::DeckKind::Macros) => {
                self.view_state.macro_cursor = if end {
                    cadence_core::macros::MAX_PROJECT_MACROS - 1
                } else {
                    0
                };
                self.view_state.macro_binding_cursor = None;
                self.status = self.macro_slot_status(self.view_state.macro_cursor as u8);
            }
            _ => return,
        }
        self.follow_deck();
    }

    /// Unified `PgUp` / `PgDn` paging across every focused deck.
    pub(super) fn navigate_page(&mut self, forward: bool, count: usize) -> Result<Outcome> {
        let step_count = count.max(1);
        let signed = |base: usize| -> isize {
            let mag = base.saturating_mul(step_count).min(isize::MAX as usize) as isize;
            if forward {
                mag
            } else {
                -mag
            }
        };
        if matches!(
            self.focused_canvas(),
            Some(Workspace::Arrange | Workspace::Pattern)
        ) {
            self.suspend_follow();
        }
        match self.focused_deck_kind() {
            Some(layout::DeckKind::Arrange) if self.evaluated_arrange_canvas() => {
                return self.move_arrangement_page(forward, step_count);
            }
            Some(layout::DeckKind::Arrange) => {
                self.move_selection(signed(6));
            }
            Some(layout::DeckKind::Pattern) => {
                let stride = 16usize.saturating_mul(step_count);
                self.cursor = if forward {
                    self.cursor
                        .saturating_add(stride)
                        .min(self.current_length().saturating_sub(1))
                } else {
                    self.cursor.saturating_sub(stride)
                };
                self.clamp_note_voice();
            }
            Some(layout::DeckKind::Mixer) => {
                let page = self.view_state.mixer_page_size.max(1);
                self.move_selection(signed(page));
            }
            Some(layout::DeckKind::Bank) => {
                if self.view_state.bank_mode == BankMode::Parts {
                    let lane_count = self.doc.project().lanes.len();
                    let stride = 6usize.saturating_mul(step_count);
                    if lane_count > 0 {
                        self.view_state.bank_lane_idx = if forward {
                            self.view_state
                                .bank_lane_idx
                                .saturating_add(stride)
                                .min(lane_count - 1)
                        } else {
                            self.view_state.bank_lane_idx.saturating_sub(stride)
                        };
                        self.sync_bank_part_selection();
                    }
                } else {
                    self.move_selection(signed(6));
                }
            }
            Some(kind) if kind.is_object() => {
                if matches!(self.detail, Detail::Instrument(_))
                    && self.rack_ui.tab == RackTab::Patchbay
                {
                    if let Some(inst_id) = self.detail_instrument() {
                        if let Some(inst) = self.instrument(inst_id) {
                            let zone_idx = self.active_zone_index(&inst.rack);
                            if let Some(zone) = inst.rack.zones.get(zone_idx).cloned() {
                                self.rack_patchbay_navigate_page(&zone, forward, step_count);
                            }
                        }
                    }
                    return Ok(Outcome::default());
                }
                self.move_selection(signed(6));
            }
            Some(layout::DeckKind::Graph) => {
                let cols = self.graph_target_columns();
                let max_col = cols.len().saturating_sub(1);
                let len = cols
                    .get(self.view_state.graph_col.min(max_col))
                    .map_or(0, Vec::len);
                let stride = 6usize.saturating_mul(step_count);
                self.view_state.graph_row = if forward {
                    self.view_state
                        .graph_row
                        .saturating_add(stride)
                        .min(len.saturating_sub(1))
                } else {
                    self.view_state.graph_row.saturating_sub(stride)
                };
            }
            Some(layout::DeckKind::Take) => {
                let total_bars = self
                    .current_take()
                    .map_or(0, |t| (t.steps() as usize).div_ceil(16));
                let stride = 4usize.saturating_mul(step_count);
                self.takes.cursor_bar = if forward {
                    self.takes
                        .cursor_bar
                        .saturating_add(stride)
                        .min(total_bars.saturating_sub(1))
                } else {
                    self.takes.cursor_bar.saturating_sub(stride)
                };
                self.sync_take_visual_marks();
            }
            Some(layout::DeckKind::Macros) => {
                self.step_macro_vertical(signed(6), false);
            }
            Some(layout::DeckKind::Midi) => {
                let stride = 6usize.saturating_mul(step_count);
                if self.devices.tab == DevicesTab::Audio {
                    let total = AudioDeckRow::ALL.len();
                    self.devices.cursor = if forward {
                        self.devices
                            .cursor
                            .saturating_add(stride)
                            .min(total.saturating_sub(1))
                    } else {
                        self.devices.cursor.saturating_sub(stride)
                    };
                } else {
                    let total =
                        crate::midi::MidiDeckRow::COUNT + self.effective_midi_mappings().len();
                    self.midi.cursor = if forward {
                        self.midi
                            .cursor
                            .saturating_add(stride)
                            .min(total.saturating_sub(1))
                    } else {
                        self.midi.cursor.saturating_sub(stride)
                    };
                }
            }
            _ => return Ok(Outcome::default()),
        }
        self.follow_deck();
        Ok(Outcome::default())
    }
    pub(super) fn normalize_focus(&mut self) {
        if self
            .selected_pattern
            .is_none_or(|id| self.pattern(id).is_none())
        {
            self.selected_pattern = self.doc.project().patterns.first().map(|p| p.id);
        }
        if self.selected_lane.is_none_or(|id| self.lane(id).is_none()) {
            self.selected_lane = self.doc.project().lanes.first().map(|p| p.id);
        }
        if self.selected_part.is_some_and(|id| self.part(id).is_none()) {
            self.selected_part = None;
        }
        if self
            .selected_placement
            .is_some_and(|id| self.placement(id).is_none())
        {
            self.selected_placement = None;
            self.scope = EditScope::Definition;
        }
        if self
            .selected_instrument
            .is_some_and(|id| self.instrument(id).is_none())
        {
            self.selected_instrument = self.doc.project().instruments.first().map(|i| i.id);
        }
        self.retain_decks();
        if matches!(self.selected_channel, ChannelRef::Channel(id) if self.channel(id).is_none()) {
            self.selected_channel = ChannelRef::Master;
        }
        let part_context = match self.scope {
            EditScope::Placement(id) => self.placement(id).map(|(_, p)| p.part),
            EditScope::Definition if self.arrange_canvas_focused() => self
                .selected_placement
                .and_then(|id| self.placement(id).map(|(_, p)| p.part)),
            _ => None,
        };
        if let Some((id, pattern, instrument)) =
            part_context.and_then(|id| self.part(id).map(|b| (id, b.pattern, b.instrument)))
        {
            self.selected_part = Some(id);
            self.selected_pattern = Some(pattern);
            self.selected_instrument = Some(instrument);
        }
        self.sync_view();
        if matches!(self.scope,EditScope::Placement(id) if self.placement(id).is_none()) {
            self.scope = EditScope::Definition;
        }
        self.cursor = self.cursor.min(self.current_length().saturating_sub(1));
        self.clamp_note_voice();
        self.clamp_mixer_slot();
        self.clamp_macro_binding_cursor();
        self.arrange_ui.step = self
            .arrange_ui
            .step
            .min(self.doc.project().bars as u32 * 16 - 1);
        self.normalize_arrangement_event();
        if let Some(rack) = self
            .detail_instrument()
            .or(self.selected_instrument)
            .and_then(|id| self.instrument(id))
            .map(|i| i.rack.clone())
        {
            self.clamp_rack_state(&rack);
        }
        let count = self.control_descriptors().len();
        self.control_cursor = self.control_cursor.min(count.saturating_sub(1));
    }
    pub(super) fn select_pattern(&mut self, id: Id) -> Result<()> {
        ensure!(self.pattern(id).is_some(), "Pattern missing");
        self.suspend_follow();
        self.push_back();
        self.selected_pattern = Some(id);
        self.show_workspace(Workspace::Pattern);
        self.scope = EditScope::Definition;
        self.cursor = 0;
        self.query.clear();
        Ok(())
    }
    pub(super) fn select_placement(&mut self, id: Id) -> Result<()> {
        ensure!(self.placement(id).is_some(), "Placement missing");
        self.push_back();
        self.set_placement_context(id)?;
        self.show_detail(Detail::Placement(id));
        Ok(())
    }
    pub(super) fn set_placement_context(&mut self, id: Id) -> Result<()> {
        let (l, p) = self.placement(id).context("Placement missing")?;
        let (lane, part, start) = (l.id, p.part, p.start_step);
        let b = self.part(part).context("Part missing")?;
        let (pattern, instrument) = (b.pattern, b.instrument);
        self.suspend_follow();
        self.selected_lane = Some(lane);
        self.selected_placement = Some(id);
        self.selected_part = Some(part);
        self.selected_pattern = Some(pattern);
        self.selected_instrument = Some(instrument);
        self.arrange_ui.step = start;
        Ok(())
    }
    pub(super) fn open_instrument(&mut self, id: Id) -> Result<()> {
        ensure!(self.instrument(id).is_some(), "Instrument missing");
        let prev = match self.detail {
            Detail::Instrument(cur) => Some(cur),
            _ => self.selected_instrument,
        };
        let switched = prev != Some(id);
        self.push_back();
        self.selected_instrument = Some(id);
        self.show_detail(Detail::Instrument(id));
        if switched {
            self.rack_ui.panel_slot = 0;
            self.control_cursor = 0;
        }
        Ok(())
    }
    pub(super) fn open_channel(&mut self, c: ChannelRef) -> Result<()> {
        ensure!(self.strip(c).is_some(), "Channel missing");
        self.push_back();
        self.selected_channel = c;
        self.show_detail(Detail::Channel(c));
        Ok(())
    }
    pub(super) fn open_effect(&mut self, id: Id) -> Result<()> {
        let (owner, _) = self.effect(id).context("Effect missing")?;
        self.push_back();
        self.selected_channel = owner;
        self.show_detail(Detail::Effect(id));
        self.control_cursor = 0;
        Ok(())
    }
    pub(super) fn move_selection(&mut self, delta: isize) {
        match self.focused_deck_kind() {
            Some(layout::DeckKind::Bank) => {
                let ids: Vec<_> = self.visible_patterns().iter().map(|p| p.id).collect();
                if let Some(id) = move_id(self.selected_pattern, &ids, delta) {
                    self.select_library_pattern(Some(id));
                }
                return;
            }
            Some(kind) if kind.is_object() => {
                let count = self.control_descriptors().len();
                if count > 0 {
                    self.control_cursor = (self.control_cursor as isize + delta)
                        .clamp(0, count as isize - 1)
                        as usize;
                }
                return;
            }
            Some(layout::DeckKind::Arrange) => {
                self.suspend_follow();
                let ids: Vec<_> = self.doc.project().lanes.iter().map(|l| l.id).collect();
                self.selected_lane = move_id(self.selected_lane, &ids, delta);
                self.select_at_cursor();
            }
            Some(layout::DeckKind::Pattern) => {
                self.cursor = (self.cursor as isize + delta)
                    .clamp(0, self.current_length().saturating_sub(1) as isize)
                    as usize;
            }
            Some(layout::DeckKind::Mixer) => {
                let ids = self.channels();
                if !ids.is_empty() {
                    let at = ids
                        .iter()
                        .position(|&c| c == self.selected_channel)
                        .unwrap_or(0);
                    self.selected_channel =
                        ids[(at as isize + delta).clamp(0, ids.len() as isize - 1) as usize];
                    self.clamp_mixer_slot();
                }
            }
            _ => return,
        }
        self.follow_deck();
    }
    pub(super) fn select_at_cursor(&mut self) {
        self.arrange_ui.event = None;
        let selected = self
            .selected_lane
            .and_then(|id| self.lane(id))
            .and_then(|l| {
                l.placements.iter().find(|p| {
                    p.start_step <= self.arrange_ui.step
                        && self.arrange_ui.step < p.start_step + p.length_steps
                })
            })
            .map(|p| p.id);
        self.selected_placement = selected;
        if selected.is_none() {
            self.selected_part = None;
        }
        if let Some(id) = selected {
            if let Some((_, p)) = self.placement(id) {
                let b = p.part;
                let (pattern, instrument) = self
                    .part(b)
                    .map(|b| (Some(b.pattern), Some(b.instrument)))
                    .unwrap_or((None, None));
                self.selected_part = Some(b);
                self.selected_pattern = pattern;
                self.selected_instrument = instrument;
            }
        }
        self.normalize_arrangement_event();
        if !self.follow.active() {
            let visible_bars = self.arrange_visible_bars();
            let _ = self.arrange_view_page(self.transport.step, visible_bars);
        }
        self.follow_deck();
    }
    pub(super) fn back(&mut self) -> Result<Outcome> {
        if self.is_auditioning() {
            return self.stop_audition();
        }
        if self.deck_focused()
            && matches!(self.detail, Detail::Instrument(_))
            && self.rack_ui.tab == RackTab::Patchbay
            && self.rack_ui.patchbay_cable_id.is_some()
        {
            self.rack_ui.patchbay_cable_id = None;
            self.status = "Patchbay · cable deselected".into();
            return Ok(Outcome::default());
        }
        if self.layout.zoomed() {
            self.layout.set_zoom(false);
            self.sync_view();
            return Ok(Outcome::default());
        }
        if let Some(navigation) = self.back.pop() {
            self.restore_navigation(navigation);
        } else {
            self.status = "At the start of the trail".into();
        }
        Ok(Outcome::default())
    }

    pub(super) fn open_current(&mut self) -> Result<Outcome> {
        if self.bank_focused() {
            self.select_pattern(self.selected_pattern.context("Create a pattern first")?)?;
            return Ok(Outcome::default());
        }
        if self.deck_focused() {
            let Some(control) = self.control_descriptors().get(self.control_cursor).cloned() else {
                return Ok(Outcome::default());
            };
            match control.target {
                ControlTarget::Pattern(id)
                | ControlTarget::ModulationLink { pattern: id, .. }
                | ControlTarget::PartParamLink { pattern: id, .. } => self.select_pattern(id)?,
                ControlTarget::Macro(index) => {
                    self.push_back();
                    self.view_state.macro_cursor =
                        usize::from(index).min(cadence_core::macros::MAX_PROJECT_MACROS - 1);
                    self.view_state.macro_binding_cursor = None;
                    self.go_to_deck(layout::Deck::song(layout::DeckKind::Macros));
                }
                ControlTarget::MacroBinding { index, binding } => {
                    self.push_back();
                    self.view_state.macro_cursor =
                        usize::from(index).min(cadence_core::macros::MAX_PROJECT_MACROS - 1);
                    self.view_state.macro_binding_cursor = Some(binding);
                    self.go_to_deck(layout::Deck::song(layout::DeckKind::Macros));
                }
                ControlTarget::Instrument(id) => self.open_instrument(id)?,
                ControlTarget::Output(id) => self.open_channel(ChannelRef::Channel(id))?,
                ControlTarget::Effect(id) => {
                    self.push_back();
                    self.show_detail(Detail::Effect(id));
                }
                ControlTarget::EffectChannel(id) => {
                    return self.begin_workflow(WorkflowKind::MoveEffect(id));
                }
                ControlTarget::Part(id) => {
                    self.push_back();
                    self.show_detail(Detail::Part(id));
                }
                ControlTarget::Placement(id) => {
                    self.push_back();
                    self.set_placement_context(id)?;
                    self.show_workspace(Workspace::Pattern);
                    self.scope = EditScope::Placement(id);
                    self.cursor = 0;
                    self.clamp_note_voice();
                }
                ControlTarget::Duck(id) => {
                    return self.begin_workflow(WorkflowKind::EditDuck(id));
                }
                ControlTarget::DuckDepth(id) => {
                    return self.begin_duck_depth_value_input(id, String::new());
                }
                ControlTarget::DuckRecovery(id) => {
                    return self.begin_duck_recovery_value_input(id, String::new());
                }
                ControlTarget::SamplerSource {
                    instrument,
                    zone,
                    module,
                } => {
                    return self.begin_sample_browser(browser::BrowserTarget::AssignModule {
                        instrument,
                        zone_id: zone,
                        module_id: Some(module),
                    });
                }
                ControlTarget::Param(ParamAddress::Effect {
                    device,
                    param: EffectParam::EnvPattern,
                }) => {
                    return self.open_groove_browser_for(device);
                }
                ControlTarget::Param(address) => {
                    return self.begin_param_value_input(address, String::new());
                }
                ControlTarget::Zone { instrument, field } => {
                    return match field {
                        ZoneControl::SamplePath | ZoneControl::Name => {
                            self.open_zone_control(instrument, field)
                        }
                        _ => self.begin_zone_field_value_input(instrument, field, String::new()),
                    };
                }
                ControlTarget::PlacementStart(id) => {
                    return self.begin_placement_start_value_input(id, String::new());
                }
                ControlTarget::PlacementLength(id) => {
                    return self.begin_placement_length_value_input(id, String::new());
                }
                ControlTarget::PlacementOffset(id) => {
                    return self.begin_placement_offset_value_input(id, String::new());
                }
                ControlTarget::PartTranspose(id) => {
                    return self.begin_part_transpose_value_input(id, String::new());
                }
                ControlTarget::PartVelocity(id) => {
                    return self.begin_part_velocity_value_input(id, String::new());
                }
                ControlTarget::ChannelOutput(id) => {
                    return self.open_graph_route_channel_picker(id);
                }
                ControlTarget::FilterEnvSource(id) => {
                    return self.open_filter_env_source_picker(id);
                }
                ControlTarget::Mute(_)
                | ControlTarget::Solo(_)
                | ControlTarget::Bypass(_)
                | ControlTarget::PlacementPhase(_) => {
                    return self.nudge_control(1, false, false);
                }
                _ => {}
            }
            return Ok(Outcome::default());
        }
        match self.focused_deck_kind() {
            Some(layout::DeckKind::Arrange) if self.evaluated_arrange_canvas() => {
                self.open_arrangement_inspection()
            }
            Some(layout::DeckKind::Arrange) => {
                if let Some(id) = self.selected_placement {
                    self.select_placement(id)?;
                } else {
                    return self.begin_workflow(WorkflowKind::Place);
                }
                Ok(Outcome::default())
            }
            Some(layout::DeckKind::Pattern) => {
                if self.scope == EditScope::Definition {
                    let sources = self
                        .selected_pattern_object()
                        .map(|p| p.kind.sources())
                        .unwrap_or_default();
                    if !sources.is_empty() {
                        let idx = self.source_cursor.min(sources.len() - 1);
                        self.source_cursor = idx;
                        self.select_pattern(sources[idx])?;
                        return Ok(Outcome::default());
                    }
                }
                if self.value_canvas() {
                    return self.begin_value_input();
                }
                if let Some(pat) = self.selected_pattern_object() {
                    if matches!(pat.kind, PatternKind::Mini { .. }) {
                        let id = pat.id;
                        return self.open_mini_editor(id);
                    }
                    if matches!(pat.kind, PatternKind::Rhythm { .. }) {
                        return self.toggle_material();
                    }
                }
                self.audition_current()
            }
            Some(layout::DeckKind::Mixer) => {
                self.clamp_mixer_slot();
                match self.view_state.mixer_slot {
                    MixerSlot::Insert(idx) => {
                        if let Some(dev_id) = self.mixer_focused_insert(idx) {
                            self.open_effect(dev_id)?;
                        }
                        Ok(Outcome::default())
                    }
                    MixerSlot::Output => {
                        if let ChannelRef::Channel(cid) = self.selected_channel {
                            self.open_graph_route_channel_picker(cid)
                        } else {
                            Ok(Outcome::default())
                        }
                    }
                    MixerSlot::Fader => self.begin_param_value_input(
                        ParamAddress::Channel {
                            channel: self.selected_channel,
                            param: ChannelParam::Gain,
                        },
                        String::new(),
                    ),
                    MixerSlot::Pan => self.begin_param_value_input(
                        ParamAddress::Channel {
                            channel: self.selected_channel,
                            param: ChannelParam::Pan,
                        },
                        String::new(),
                    ),
                    MixerSlot::Send(idx) => {
                        if let Some((ch, target)) = self.mixer_focused_send(idx) {
                            self.begin_param_value_input(
                                ParamAddress::Send {
                                    channel: ChannelRef::Channel(ch),
                                    target,
                                },
                                String::new(),
                            )
                        } else {
                            Ok(Outcome::default())
                        }
                    }
                    MixerSlot::Duck(idx) => {
                        if let Some(duck_id) = self.mixer_focused_duck(idx) {
                            self.begin_duck_depth_value_input(duck_id, String::new())
                        } else {
                            Ok(Outcome::default())
                        }
                    }
                }
            }
            _ => Ok(Outcome::default()),
        }
    }
}
