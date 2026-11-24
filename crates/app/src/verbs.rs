//! Verbs on object decks. A placement's deck is a verb sheet: its properties
//! open with Enter, and single keys act on the placement. Every verb is one
//! undoable edit that changes only this placement; other uses of its pattern,
//! part and sound stay as they are.
use super::*;

/// What the replace picker swaps on a placement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Replace {
    Pattern,
    Instrument,
}

impl App {
    /// The focused Placement deck's non-mutating/explicit verb keys in `NORMAL`, or `None`.
    pub(super) fn placement_verb(
        &mut self,
        id: Id,
        key: KeyEvent,
        _count: usize,
    ) -> Option<Result<Outcome>> {
        Some(match key.code {
            KeyCode::Char('r') => {
                let what = if matches!(
                    self.selected_control().map(|c| c.target),
                    Some(ControlTarget::Instrument(_))
                ) {
                    Replace::Instrument
                } else {
                    Replace::Pattern
                };
                self.open_replace(id, what)
            }
            KeyCode::Char('f') => self.fork_for_placement(id),
            KeyCode::Char('o') => self.execute_command(&format!("local {id}")),
            KeyCode::Char('x') => self.delete_placement(id),
            KeyCode::Char('X') => self.clear_all_placement_exceptions(id),
            KeyCode::Char('A') => {
                let (_, placement) = self.placement(id)?;
                let pattern = self.part(placement.part)?.pattern;
                if self.audition_pattern() == Some(pattern) {
                    self.stop_audition()
                } else {
                    self.start_audition(pattern, None)
                }
            }
            _ => return None,
        })
    }

    /// The focused Part deck's verb keys (`r` rebind/rename, `X` delete with confirm), or `None`.
    pub(super) fn part_verb(
        &mut self,
        id: Id,
        key: KeyEvent,
        _count: usize,
    ) -> Option<Result<Outcome>> {
        Some(match key.code {
            KeyCode::Char('r') => match self.selected_control().map(|c| c.target) {
                Some(ControlTarget::Pattern(_)) => self.open_replace_part(id, Replace::Pattern),
                Some(ControlTarget::Instrument(_)) => {
                    self.open_replace_part(id, Replace::Instrument)
                }
                _ => self.begin_workflow(WorkflowKind::RenamePart(id)),
            },
            KeyCode::Char('X') => self.request_delete_part(id),
            _ => return None,
        })
    }

    fn clear_all_placement_exceptions(&mut self, id: Id) -> Result<Outcome> {
        let (lane, placement) = self.placement(id).context("Placement missing")?;
        if placement.exceptions.is_empty() {
            self.status = "Placement has no local exceptions to clear".into();
            return Ok(Outcome::default());
        }
        let lane_id = lane.id;
        let count = placement.exceptions.len();
        let mut updated = placement.clone();
        updated.exceptions.clear();
        self.commit(
            vec![Edit::PutPlacement {
                lane: lane_id,
                placement: updated,
            }],
            format!(
                "Cleared {count} local exception{} · u restores",
                if count == 1 { "" } else { "s" }
            ),
        )
    }

    /// Rebind an existing `Part`'s pattern or instrument in-place.
    pub(super) fn replace_on_part(
        &mut self,
        part_id: Id,
        what: Replace,
        with: Id,
    ) -> Result<Outcome> {
        let mut updated = self.part(part_id).context("Part missing")?.clone();
        let unchanged = match what {
            Replace::Pattern => updated.pattern == with,
            Replace::Instrument => updated.instrument == with,
        };
        if unchanged {
            self.status = "Part is already bound to that".into();
            return Ok(Outcome::default());
        }
        let label = match what {
            Replace::Pattern => {
                let pat_name = self.pattern(with).context("Pattern missing")?.name.clone();
                updated.pattern = with;
                format!("Rebound '{}' pattern → '{pat_name}'", updated.name)
            }
            Replace::Instrument => {
                let inst_name = self
                    .instrument(with)
                    .context("Instrument missing")?
                    .name
                    .clone();
                updated.instrument = with;
                format!("Rebound '{}' sound → '{inst_name}'", updated.name)
            }
        };
        let pat = updated.pattern;
        let inst = updated.instrument;
        let out = self.commit(vec![Edit::PutPart { part: updated }], label)?;
        self.selected_pattern = Some(pat);
        self.selected_instrument = Some(inst);
        self.sync_view();
        Ok(out)
    }

    /// Reorder an arrangement lane vertically by `delta` (`-1` up, `+1` down).
    pub(super) fn reorder_lane_by(&mut self, lane: Id, delta: isize) -> Result<Outcome> {
        let lanes = &self.doc.project().lanes;
        let Some(cur_idx) = lanes.iter().position(|l| l.id == lane) else {
            return Ok(Outcome::default());
        };
        let target_index =
            (cur_idx as isize + delta).clamp(0, lanes.len().saturating_sub(1) as isize) as usize;
        if target_index == cur_idx {
            return Ok(Outcome::default());
        }
        self.apply(Action::ReorderLane { lane, target_index })
    }

    /// Move a placement or change its length, in steps (coalesced on repeat).
    pub(super) fn reshape_placement(
        &mut self,
        id: Id,
        shift: i64,
        resize: i64,
        coalesce: bool,
    ) -> Result<Outcome> {
        let (lane, placement) = self.placement(id).context("Placement missing")?;
        let lane = lane.id;
        let mut placement = placement.clone();
        let start = placement.start_step as i64 + shift;
        let length = placement.length_steps as i64 + resize;
        ensure!(start >= 0, "The placement already starts at bar 1");
        ensure!(
            length >= 4,
            "A placement is at least one beat (4 steps) long"
        );
        ensure!(
            placement.exceptions.iter().all(|e| e.step < length as u32),
            "Local exception lies past the shortened end"
        );
        placement.start_step = start as u32;
        placement.length_steps = length as u32;
        let end_step = placement
            .start_step
            .checked_add(placement.length_steps)
            .context("Placement extends past arrangement bounds")?;
        let end_bar = (end_step as usize).div_ceil(16).max(1);
        ensure!(
            end_bar <= cadence_core::material::MAX_BARS,
            "Placement cannot extend past the {}-bar arrangement limit",
            cadence_core::material::MAX_BARS
        );
        let mut edits = Vec::new();
        if end_bar > self.doc.project().bars {
            edits.push(Edit::SetBars { bars: end_bar });
        }
        edits.push(Edit::PutPlacement { lane, placement });
        let label = if shift != 0 {
            format!(
                "Moved placement → bar {} · step {}",
                start / 16 + 1,
                start % 16 + 1
            )
        } else {
            format!("Resized placement → {length} steps")
        };
        let out = self.commit_coalesced(
            edits,
            label,
            document::CoalesceKey::PlacementShape(id),
            coalesce,
        )?;
        if shift != 0 {
            self.arrange_ui.step = start as u32;
        } else {
            self.arrange_ui.step = (start as u32).min(self.arrange_ui.step.max(start as u32));
        }
        self.selected_placement = Some(id);
        Ok(out)
    }

    /// Move a placement vertically across arrangement lanes (`Alt+j/k`).
    pub(super) fn move_placement_lane(
        &mut self,
        id: Id,
        delta: isize,
        coalesce: bool,
    ) -> Result<Outcome> {
        let (cur_lane, placement) = self.placement(id).context("Placement missing")?;
        let cur_lane_id = cur_lane.id;
        let placement = placement.clone();
        let lanes = &self.doc.project().lanes;
        let Some(cur_idx) = lanes.iter().position(|l| l.id == cur_lane_id) else {
            return Ok(Outcome::default());
        };
        let next_idx =
            (cur_idx as isize + delta).clamp(0, lanes.len().saturating_sub(1) as isize) as usize;
        if next_idx == cur_idx {
            return Ok(Outcome::default());
        }
        let target_lane_id = lanes[next_idx].id;
        let target_lane_name = lanes[next_idx].name.clone();
        let out = self.commit_coalesced(
            vec![Edit::PutPlacement {
                lane: target_lane_id,
                placement,
            }],
            format!("Moved placement → {target_lane_name}"),
            document::CoalesceKey::PlacementShape(id),
            coalesce,
        )?;
        self.selected_lane = Some(target_lane_id);
        self.selected_placement = Some(id);
        Ok(out)
    }

    fn delete_placement(&mut self, id: Id) -> Result<Outcome> {
        ensure!(self.placement(id).is_some(), "Placement missing");
        let out = self.apply(Action::Edit {
            edit: Edit::RemovePlacement { id },
        })?;
        self.status = "Deleted the placement · u restores it".into();
        Ok(out)
    }

    /// Duplicate the focused Arrange placement immediately after its end bar on the same lane.
    pub(super) fn duplicate_selected_placement(&mut self) -> Result<Outcome> {
        let Some(id) = self.selected_placement else {
            self.status = "No placement under cursor · Enter places a part".into();
            return Ok(Outcome::default());
        };
        let (lane, placement) = self.placement(id).context("Placement missing")?;
        let lane_id = lane.id;
        let mut duplicated = placement.clone();
        let start_step = duplicated
            .start_step
            .saturating_add(duplicated.length_steps);
        let end_step = start_step.saturating_add(duplicated.length_steps);
        let end_bar = (end_step as usize).div_ceil(16).max(1);
        ensure!(
            end_bar <= cadence_core::material::MAX_BARS,
            "Cannot duplicate placement past the {}-bar arrangement limit",
            cadence_core::material::MAX_BARS
        );
        duplicated.id = self.doc.fresh_ids(1)?[0];
        duplicated.start_step = start_step;
        let part_name = self
            .part(duplicated.part)
            .map_or("part", |p| p.name.as_str())
            .to_string();
        let mut edits = Vec::new();
        if end_bar > self.doc.project().bars {
            edits.push(Edit::SetBars { bars: end_bar });
        }
        edits.push(Edit::PutPlacement {
            lane: lane_id,
            placement: duplicated,
        });
        let label = format!("Duplicated '{part_name}' at bar {}", start_step / 16 + 1);
        let out = self.commit(edits, label)?;
        self.arrange_ui.step = start_step;
        self.selected_lane = Some(lane_id);
        self.select_at_cursor();
        Ok(out)
    }

    /// Point a placement at a part with a different pattern or sound,
    /// reusing a matching part or creating one.
    pub(super) fn replace_on_placement(
        &mut self,
        id: Id,
        what: Replace,
        with: Id,
    ) -> Result<Outcome> {
        let (lane, placement) = self.placement(id).context("Placement missing")?;
        let lane = lane.id;
        let mut placement = placement.clone();
        let current = self.part(placement.part).context("Part missing")?.clone();
        let mut wanted = current.clone();
        match what {
            Replace::Pattern => wanted.pattern = with,
            Replace::Instrument => wanted.instrument = with,
        }
        let existing = self.doc.project().parts.iter().find(|b| {
            b.pattern == wanted.pattern
                && b.instrument == wanted.instrument
                && b.transpose == wanted.transpose
                && b.velocity_scale == wanted.velocity_scale
                && b.parameter_links == wanted.parameter_links
        });
        let mut edits = Vec::new();
        placement.part = match existing {
            Some(part) => part.id,
            None => {
                wanted.id = self.doc.fresh_ids(1)?[0];
                let pattern = self
                    .pattern(wanted.pattern)
                    .map_or("Part", |p| p.name.as_str());
                let instrument = self
                    .instrument(wanted.instrument)
                    .map_or("sound", |i| i.name.as_str());
                wanted.name = crate::unique_name(
                    &format!("{pattern} · {instrument}"),
                    self.doc.project().parts.iter().map(|b| b.name.as_str()),
                    None,
                );
                let id = wanted.id;
                edits.push(Edit::PutPart { part: wanted });
                id
            }
        };
        if placement.part == current.id {
            self.status = "Already playing that".into();
            return Ok(Outcome::default());
        }
        edits.push(Edit::PutPlacement { lane, placement });
        self.apply(Action::Transaction { edits })
    }

    /// Give one placement its own fork of its pattern. The fork keeps its
    /// inputs, so edits to them still reach it.
    fn fork_for_placement(&mut self, id: Id) -> Result<Outcome> {
        let (lane, placement) = self.placement(id).context("Placement missing")?;
        let lane = lane.id;
        let mut placement = placement.clone();
        let part = self.part(placement.part).context("Part missing")?.clone();
        let source = self.pattern(part.pattern).context("Pattern missing")?;
        let name = crate::unique_name(
            &format!("{} fork", source.name),
            self.doc.project().patterns.iter().map(|p| p.name.as_str()),
            None,
        );
        let ids = self.doc.fresh_ids(2)?;
        let (fork, bound) = (ids[0], ids[1]);
        let mut forked = part.clone();
        forked.id = bound;
        forked.pattern = fork;
        forked.name = crate::unique_name(
            &format!("{} fork", part.name),
            self.doc.project().parts.iter().map(|b| b.name.as_str()),
            None,
        );
        placement.part = bound;
        let out = self.apply(Action::Transaction {
            edits: vec![
                Edit::ForkPattern {
                    source: part.pattern,
                    id: fork,
                    name: name.clone(),
                },
                Edit::PutPart { part: forked },
                Edit::PutPlacement { lane, placement },
            ],
        })?;
        self.status = format!("This placement now plays {name}, a fork that keeps its inputs");
        Ok(out)
    }
}
