//! Song-level deck key handlers (`Take`, `Graph`, `Bank` launchpad).
use super::*;

impl App {
    pub(crate) fn sync_take_visual_marks(&mut self) {
        if let Some(anchor) = self.takes.visual_anchor {
            self.takes.mark_start = Some(anchor.min(self.takes.cursor_bar));
            self.takes.mark_end = Some(anchor.max(self.takes.cursor_bar));
        }
    }

    pub(crate) fn keep_take_selection(&mut self) -> Result<Outcome> {
        let (a, b) = match (self.takes.mark_start, self.takes.mark_end) {
            (Some(s), Some(e)) => (s.min(e) + 1, s.max(e) + 1),
            (Some(s), None) => (
                s.min(self.takes.cursor_bar) + 1,
                s.max(self.takes.cursor_bar) + 1,
            ),
            (None, Some(e)) => (
                e.min(self.takes.cursor_bar) + 1,
                e.max(self.takes.cursor_bar) + 1,
            ),
            (None, None) => (self.takes.cursor_bar + 1, self.takes.cursor_bar + 1),
        };
        let out = self.keep_take_range(a, b)?;
        self.takes.visual_anchor = None;
        Ok(out)
    }

    pub(super) fn handle_take_key(
        &mut self,
        key: KeyEvent,
        count: usize,
    ) -> Option<Result<Outcome>> {
        let total_bars = self
            .current_take()
            .map_or(0, |t| t.steps().div_ceil(16).max(1) as usize);
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => {
                self.takes.cursor_bar = self.takes.cursor_bar.saturating_sub(count);
                self.sync_take_visual_marks();
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('l') | KeyCode::Right => {
                if total_bars > 0 {
                    self.takes.cursor_bar =
                        (self.takes.cursor_bar + count).min(total_bars.saturating_sub(1));
                }
                self.sync_take_visual_marks();
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('v') => {
                if total_bars == 0 {
                    self.status = "No take to mark · tr arms take recording".into();
                    return Some(Ok(Outcome::default()));
                }
                if self.takes.visual_anchor.take().is_some() {
                    if let (Some(a), Some(b)) = (self.takes.mark_start, self.takes.mark_end) {
                        self.status = format!(
                            "Locked take range B{:02}–{:02} · Enter / Alt+c keep · x clear",
                            a.min(b) + 1,
                            a.max(b) + 1
                        );
                    }
                } else {
                    let bar = self.takes.cursor_bar.min(total_bars.saturating_sub(1));
                    self.takes.cursor_bar = bar;
                    self.takes.visual_anchor = Some(bar);
                    self.takes.mark_start = Some(bar);
                    self.takes.mark_end = Some(bar);
                    self.status = format!(
                        "VISUAL mark from bar {} · h/l extend · v lock · Enter / Alt+c keep · Esc clear",
                        bar + 1
                    );
                }
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('x') => {
                self.takes.visual_anchor = None;
                self.takes.mark_start = None;
                self.takes.mark_end = None;
                self.status = "Cleared take range marks".into();
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('X') => Some(self.request_discard_take()),
            KeyCode::Char('o') => {
                self.open_file_hub_for_take_load();
                Some(Ok(Outcome::default()))
            }
            KeyCode::Enter => Some(self.keep_take_selection()),
            _ => None,
        }
    }

    pub(super) fn handle_graph_key(
        &mut self,
        key: KeyEvent,
        count: usize,
    ) -> Option<Result<Outcome>> {
        let cols = self.graph_target_columns();
        let col_count = cols.len();
        let max_col = col_count.saturating_sub(1);
        let max_col_i = max_col as isize;
        let move_col = |cur: usize, dir: isize| -> usize {
            let mut c = (cur.min(max_col)) as isize;
            for _ in 0..col_count {
                c = (c + dir).clamp(0, max_col_i);
                if !cols[c as usize].is_empty() {
                    return c as usize;
                }
                if (dir < 0 && c == 0) || (dir > 0 && c == max_col_i) {
                    break;
                }
            }
            cur.min(max_col)
        };
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => {
                for _ in 0..count {
                    self.view_state.graph_col = move_col(self.view_state.graph_col, -1);
                }
                let max_r = cols[self.view_state.graph_col].len().saturating_sub(1);
                self.view_state.graph_row = self.view_state.graph_row.min(max_r);
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('l') | KeyCode::Right => {
                for _ in 0..count {
                    self.view_state.graph_col = move_col(self.view_state.graph_col, 1);
                }
                let max_r = cols[self.view_state.graph_col].len().saturating_sub(1);
                self.view_state.graph_row = self.view_state.graph_row.min(max_r);
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.view_state.graph_row = self.view_state.graph_row.saturating_sub(count);
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('j') | KeyCode::Down => {
                let len = cols[self.view_state.graph_col.min(max_col)].len();
                if len > 0 {
                    self.view_state.graph_row = (self.view_state.graph_row + count).min(len - 1);
                }
                Some(Ok(Outcome::default()))
            }
            KeyCode::Enter => {
                let target = self.selected_graph_target()?;
                let res = match target {
                    crate::view::GraphTarget::Pattern(id) => {
                        self.select_pattern(id).map(|()| Outcome::default())
                    }
                    crate::view::GraphTarget::Part(id) => {
                        self.push_back();
                        self.selected_part = Some(id);
                        self.show_detail(Detail::Part(id));
                        Ok(Outcome::default())
                    }
                    crate::view::GraphTarget::Macro(index) => {
                        self.view_state.macro_cursor = usize::from(index);
                        self.go_to_deck(layout::Deck::song(layout::DeckKind::Macros));
                        Ok(Outcome::default())
                    }
                    crate::view::GraphTarget::Instrument(id) => {
                        self.open_instrument(id).map(|()| Outcome::default())
                    }
                    crate::view::GraphTarget::Channel(ch) => {
                        self.open_channel(ch).map(|()| Outcome::default())
                    }
                    crate::view::GraphTarget::Effect(id) => {
                        self.open_effect(id).map(|()| Outcome::default())
                    }
                };
                Some(res)
            }
            KeyCode::Char('o' | 'r') => {
                let target = self.selected_graph_target()?;
                let res = match target {
                    crate::view::GraphTarget::Channel(ChannelRef::Channel(cid)) => {
                        self.open_graph_route_channel_picker(cid)
                    }
                    crate::view::GraphTarget::Effect(dev_id) => match self.effect(dev_id) {
                        Some((ChannelRef::Channel(cid), _)) => {
                            self.open_graph_route_channel_picker(cid)
                        }
                        _ => {
                            self.status = "Master inserts route directly to Master output".into();
                            Ok(Outcome::default())
                        }
                    },
                    crate::view::GraphTarget::Instrument(inst_id) => {
                        self.open_graph_route_instrument_picker(inst_id)
                    }
                    crate::view::GraphTarget::Channel(ChannelRef::Master) => {
                        self.status = "Master is the final output bus".into();
                        Ok(Outcome::default())
                    }
                    _ => {
                        self.status =
                            "Select an Instrument, Channel, or Effect node to route output (o/r)"
                                .into();
                        Ok(Outcome::default())
                    }
                };
                Some(res)
            }
            KeyCode::Char('a') => Some(self.open_graph_add_picker()),
            KeyCode::Char('s') => Some(self.begin_workflow(WorkflowKind::Send)),
            KeyCode::Char('m') => Some(self.begin_workflow(WorkflowKind::Modulation)),
            KeyCode::Char('d') => Some(self.begin_workflow(WorkflowKind::Duck)),
            KeyCode::Char('e') => {
                let target = self.selected_graph_target()?;
                let editable = self.graph_editable_cables(target);
                let res = if editable.is_empty() {
                    self.status =
                        "No editable cables on node · s send · m mod · d duck · o route".into();
                    Ok(Outcome::default())
                } else if editable.len() == 1 {
                    self.edit_graph_cable(editable[0].0)
                } else {
                    self.open_graph_cable_picker("EDIT GRAPH CABLE".into(), editable, false)
                };
                Some(res)
            }
            KeyCode::Char('x') => {
                let target = self.selected_graph_target()?;
                let removable = self.graph_removable_cables(target);
                let res = if removable.is_empty() {
                    self.status = "No removable cables attached to selected node".into();
                    Ok(Outcome::default())
                } else if removable.len() == 1 {
                    self.unlink_graph_cable(removable[0].0)
                } else {
                    self.open_graph_cable_picker("UNLINK GRAPH CABLE".into(), removable, true)
                };
                Some(res)
            }
            KeyCode::Char('X') => {
                let target = self.selected_graph_target()?;
                Some(self.unlink_all_graph_cables(target))
            }
            _ => None,
        }
    }

    pub(crate) fn sync_bank_part_selection(&mut self) {
        let (lane_id, part_entry) = {
            let lanes = &self.doc.project().lanes;
            if lanes.is_empty() {
                return;
            }
            self.view_state.bank_lane_idx = self.view_state.bank_lane_idx.min(lanes.len() - 1);
            let lane = &lanes[self.view_state.bank_lane_idx];
            let entry = if lane.parts.is_empty() {
                self.view_state.bank_part_idx = 0;
                None
            } else {
                self.view_state.bank_part_idx =
                    self.view_state.bank_part_idx.min(lane.parts.len() - 1);
                Some(lane.parts[self.view_state.bank_part_idx])
            };
            (lane.id, entry)
        };
        self.selected_lane = Some(lane_id);
        if let Some(lp) = part_entry {
            self.selected_part = Some(lp.part);
            if let Some((pat, inst)) = self.part(lp.part).map(|p| (p.pattern, p.instrument)) {
                self.selected_pattern = Some(pat);
                self.selected_instrument = Some(inst);
            }
            if self.scope != EditScope::Definition {
                self.scope = EditScope::Definition;
            }
        }
        self.sync_view();
    }

    pub(super) fn handle_bank_key(
        &mut self,
        key: KeyEvent,
        _count: usize,
    ) -> Option<Result<Outcome>> {
        if key.code == KeyCode::Char('v') {
            self.view_state.bank_mode = match self.view_state.bank_mode {
                BankMode::Parts => BankMode::Material,
                BankMode::Material => BankMode::Parts,
            };
            self.status = match self.view_state.bank_mode {
                BankMode::Parts => {
                    "Bank · Parts launchpad (hjkl move · Enter launch · v material)".into()
                }
                BankMode::Material => {
                    "Bank · Material library (j/k select · Enter open · v parts)".into()
                }
            };
            return Some(Ok(Outcome::default()));
        }
        if self.view_state.bank_mode != BankMode::Parts {
            return None;
        }
        let lane_count = self.doc.project().lanes.len();
        if lane_count == 0 {
            return match key.code {
                KeyCode::Char('a') => Some(self.begin_workflow(WorkflowKind::Lane)),
                KeyCode::Char(
                    'j' | 'k' | 'h' | 'l' | '0' | '$' | 'G' | 'x' | 'X' | 'r' | 'm' | 's' | 'A',
                )
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::End
                | KeyCode::PageUp
                | KeyCode::PageDown
                | KeyCode::Enter => {
                    self.status = "No lanes yet · press a or n to create a lane".into();
                    Some(Ok(Outcome::default()))
                }
                _ => None,
            };
        }
        self.view_state.bank_lane_idx = self.view_state.bank_lane_idx.min(lane_count - 1);
        let lane_id = self.doc.project().lanes[self.view_state.bank_lane_idx].id;
        let parts = self.doc.project().lanes[self.view_state.bank_lane_idx]
            .parts
            .clone();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if self.view_state.bank_lane_idx + 1 < lane_count {
                    self.view_state.bank_lane_idx += 1;
                }
                self.sync_bank_part_selection();
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.view_state.bank_lane_idx = self.view_state.bank_lane_idx.saturating_sub(1);
                self.sync_bank_part_selection();
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('l') | KeyCode::Right => {
                if !parts.is_empty() && self.view_state.bank_part_idx + 1 < parts.len() {
                    self.view_state.bank_part_idx += 1;
                }
                self.sync_bank_part_selection();
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.view_state.bank_part_idx = self.view_state.bank_part_idx.saturating_sub(1);
                self.sync_bank_part_selection();
                Some(Ok(Outcome::default()))
            }
            KeyCode::Enter => {
                if let Some(&lp) = parts.get(
                    self.view_state
                        .bank_part_idx
                        .min(parts.len().saturating_sub(1)),
                ) {
                    let at = key
                        .modifiers
                        .contains(KeyModifiers::SHIFT)
                        .then_some(cadence_core::material::Boundary::Now);
                    Some(self.apply(Action::Launch {
                        lane: lane_id,
                        part: lp,
                        at,
                    }))
                } else {
                    self.status = "No parts on this lane · press a or n to add".into();
                    Some(Ok(Outcome::default()))
                }
            }
            KeyCode::Char('a') => Some(self.open_bank_add_part_picker(lane_id)),
            KeyCode::Char('r') => {
                if let Some(&lp) = parts.get(
                    self.view_state
                        .bank_part_idx
                        .min(parts.len().saturating_sub(1)),
                ) {
                    Some(self.begin_workflow(WorkflowKind::EditLanePart {
                        lane: lane_id,
                        part: lp.part,
                    }))
                } else {
                    self.status = "No part on this lane · press a to add".into();
                    Some(Ok(Outcome::default()))
                }
            }
            KeyCode::Char('X') => {
                if let Some(&lp) = parts.get(
                    self.view_state
                        .bank_part_idx
                        .min(parts.len().saturating_sub(1)),
                ) {
                    let res = self.apply(Action::DropLanePart {
                        lane: lane_id,
                        part: lp.part,
                    });
                    if res.is_ok() {
                        self.sync_bank_part_selection();
                    }
                    Some(res)
                } else {
                    self.status = "No part selected on this lane to drop".into();
                    Some(Ok(Outcome::default()))
                }
            }
            KeyCode::Char('A') => {
                if let Some(&lp) = parts.get(
                    self.view_state
                        .bank_part_idx
                        .min(parts.len().saturating_sub(1)),
                ) {
                    if let Some(part) = self.part(lp.part) {
                        let pat = part.pattern;
                        if self.audition_pattern() == Some(pat) {
                            return Some(self.stop_audition());
                        }
                        return Some(self.start_audition(pat, None));
                    }
                }
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('x') => Some(self.apply(Action::Release {
                lane: lane_id,
                at: None,
            })),
            KeyCode::Char('m') => Some(self.apply(Action::SetLaneMuted {
                lane: lane_id,
                muted: !self.effective_lane_muted(lane_id),
            })),
            KeyCode::Char('s') => Some(self.apply(Action::SetLaneSolo {
                lane: lane_id,
                solo: !self.effective_lane_solo(lane_id),
            })),
            _ => None,
        }
    }
}
