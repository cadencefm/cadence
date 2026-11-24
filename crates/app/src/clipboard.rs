//! Yank and paste: placements in Arrange; note steps, rhythm hits, and value
//! breakpoints in Pattern (`[count]y` copies a span). `p` pastes after (after
//! the placement under the cursor, or into the next step), `P` at the cursor.
//! Pasting is an ordinary atomic undoable edit.
use super::*;
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub(super) enum Clip {
    /// A placement, pasted with a fresh identity and its local exceptions.
    Placement(Placement),
    /// One or more note steps (single step via `y`, span via `[count]y`).
    Notes(Vec<Step>),
    /// One or more rhythm hit states (single step via `y`, span via `[count]y`).
    Rhythm(Vec<bool>),
    /// One or more value breakpoints across `span` steps (`points` use relative offsets `0..span`).
    Values {
        unit: ValueUnit,
        span: u32,
        points: Vec<ValuePoint>,
    },
}

impl App {
    fn on_placements(&self) -> bool {
        self.arrange_canvas_focused() && self.arrange_ui.view == ArrangeView::Placements
    }

    fn on_steps(&self) -> bool {
        self.pattern_canvas_focused()
    }

    pub(super) fn yank(&mut self, count: usize) -> Result<Outcome> {
        let span = count.max(1);
        if self.on_placements() {
            let (_, placement) = self
                .selected_placement
                .and_then(|id| self.placement(id))
                .context("Move onto a placement to yank it")?;
            let name = self
                .part(placement.part)
                .map_or("placement".into(), |b| b.name.clone());
            self.clipboard = Some(Clip::Placement(placement.clone()));
            self.status = format!("Yanked {name} · p pastes after it, P at the cursor");
            return Ok(Outcome::default());
        }
        ensure!(
            self.on_steps(),
            "y yanks a placement in Arrange or steps/hits/values in Pattern"
        );
        let len = self.current_length().max(1);
        let start = self.cursor.min(len - 1);
        let take = span.min(len.saturating_sub(start)).max(1);

        if matches!(self.scope, EditScope::Placement(_)) {
            let mut steps = Vec::with_capacity(take);
            for offset in 0..take {
                match self.value_of(self.selected_pattern, self.scope, start + offset)? {
                    PatternValue::Notes(step) => steps.push(step),
                    _ => bail!("Expected note step in placement scope"),
                }
            }
            self.clipboard = Some(Clip::Notes(steps));
            self.status = if take == 1 {
                format!(
                    "Yanked step {} · p pastes into the next step, P here",
                    start + 1
                )
            } else {
                format!(
                    "Yanked {take} note steps ({}–{}) · p pastes after, P here",
                    start + 1,
                    start + take
                )
            };
            return Ok(Outcome::default());
        }

        let pat = self
            .selected_pattern_object()
            .context("Select a pattern")?
            .clone();
        let ty = self.doc.project().pattern_type(pat.id)?;
        match ty {
            PatternType::Notes => {
                let mut steps = Vec::with_capacity(take);
                for offset in 0..take {
                    match self.value_of(Some(pat.id), EditScope::Definition, start + offset)? {
                        PatternValue::Notes(step) => steps.push(step),
                        _ => bail!("Expected note step"),
                    }
                }
                self.clipboard = Some(Clip::Notes(steps));
                self.status = if take == 1 {
                    format!(
                        "Yanked step {} · p pastes into the next step, P here",
                        start + 1
                    )
                } else {
                    format!(
                        "Yanked {take} note steps ({}–{}) · p pastes after, P here",
                        start + 1,
                        start + take
                    )
                };
            }
            PatternType::Rhythm => {
                let mut hits = Vec::with_capacity(take);
                for offset in 0..take {
                    match self.value_of(Some(pat.id), EditScope::Definition, start + offset)? {
                        PatternValue::Rhythm(active) => hits.push(active),
                        _ => bail!("Expected rhythm hit"),
                    }
                }
                let active_count = hits.iter().filter(|&&on| on).count();
                self.clipboard = Some(Clip::Rhythm(hits));
                self.status = if take == 1 {
                    format!(
                        "Yanked rhythm step {} ({}) · p pastes next, P here",
                        start + 1,
                        if active_count > 0 { "hit" } else { "rest" }
                    )
                } else {
                    format!(
                        "Yanked {take} rhythm steps ({active_count} hits) · p pastes after, P here"
                    )
                };
            }
            PatternType::Values(unit) => {
                let span_u32 = take as u32;
                let points = if take == 1 {
                    let PatternValue::Value(value) = self.resolve_pattern(pat.id, start as i64)?
                    else {
                        bail!("Expected scalar value");
                    };
                    vec![ValuePoint { step: 0, value }]
                } else if let PatternKind::Values { points, .. } = &pat.kind {
                    let start_u32 = start as u32;
                    let end_u32 = start_u32 + span_u32;
                    let mut span_pts: Vec<ValuePoint> = points
                        .iter()
                        .filter(|p| p.step >= start_u32 && p.step < end_u32)
                        .map(|p| ValuePoint {
                            step: p.step - start_u32,
                            value: p.value,
                        })
                        .collect();
                    if span_pts.is_empty() {
                        let PatternValue::Value(value) =
                            self.resolve_pattern(pat.id, start as i64)?
                        else {
                            bail!("Expected scalar value");
                        };
                        span_pts.push(ValuePoint { step: 0, value });
                    }
                    span_pts
                } else {
                    let mut sampled = Vec::with_capacity(take);
                    for offset in 0..take {
                        let PatternValue::Value(value) =
                            self.resolve_pattern(pat.id, (start + offset) as i64)?
                        else {
                            bail!("Expected scalar value");
                        };
                        sampled.push(ValuePoint {
                            step: offset as u32,
                            value,
                        });
                    }
                    sampled
                };
                let count_pts = points.len();
                self.clipboard = Some(Clip::Values {
                    unit,
                    span: span_u32,
                    points,
                });
                self.status = if take == 1 {
                    format!(
                        "Yanked value at step {} · p pastes into the next step, P here",
                        start + 1
                    )
                } else {
                    format!(
                        "Yanked {count_pts} value point{} ({take} steps) · p pastes after, P here",
                        if count_pts == 1 { "" } else { "s" }
                    )
                };
            }
        }
        Ok(Outcome::default())
    }

    pub(super) fn paste(&mut self, after: bool) -> Result<Outcome> {
        let clip = self
            .clipboard
            .clone()
            .context("Nothing yanked · y yanks a placement or steps/hits/values")?;
        match clip {
            Clip::Placement(mut placement) => {
                ensure!(self.on_placements(), "Paste placements in Arrange");
                let lane = self.selected_lane.context("Select a lane")?;
                let under = self
                    .selected_placement
                    .and_then(|id| self.placement(id))
                    .filter(|(l, _)| l.id == lane)
                    .map(|(_, p)| p.start_step + p.length_steps);
                placement.start_step = match (after, under) {
                    (true, Some(end)) => end,
                    _ => self.arrange_ui.step / 16 * 16,
                };
                placement.id = self.doc.fresh_ids(1)?[0];
                let start = placement.start_step;
                // Pasting past the end lengthens the song in the same edit.
                let end_bar = (start + placement.length_steps).div_ceil(16) as usize;
                let mut edits = Vec::new();
                if end_bar > self.doc.project().bars {
                    edits.push(Edit::SetBars { bars: end_bar });
                }
                edits.push(Edit::PutPlacement { lane, placement });
                let out = self.apply(Action::Transaction { edits })?;
                self.arrange_ui.step = start;
                self.select_at_cursor();
                Ok(out)
            }
            Clip::Notes(steps) => {
                ensure!(self.on_steps(), "Paste notes in Pattern");
                let len = self.current_length().max(1);
                if after {
                    self.cursor = (self.cursor + 1) % len;
                }
                if steps.len() == 1 {
                    let edit = self.edits_for_current_note(steps[0].clone())?;
                    return self.apply(Action::Edit { edit });
                }
                let start = self.cursor;
                let mut edits = Vec::new();
                let build_result: Result<()> = (|| {
                    for (offset, step) in steps.into_iter().enumerate() {
                        if start + offset >= len {
                            break;
                        }
                        self.cursor = start + offset;
                        edits.push(self.edits_for_current_note(step)?);
                    }
                    Ok(())
                })();
                self.cursor = start;
                build_result?;
                ensure!(!edits.is_empty(), "Nothing fits at the current step");
                let pasted = edits.len();
                let out = self.apply(Action::Transaction { edits })?;
                self.status = format!("Pasted {pasted} note steps at step {}", start + 1);
                Ok(out)
            }
            Clip::Rhythm(hits) => {
                ensure!(self.on_steps(), "Paste rhythm in Pattern");
                ensure!(
                    self.scope == EditScope::Definition,
                    "Switch to Definition scope to paste rhythm hits"
                );
                let pat = self
                    .selected_pattern_object()
                    .context("Select a literal rhythm pattern")?;
                let PatternKind::Rhythm { steps } = &pat.kind else {
                    bail!("Select a literal Rhythm pattern to paste rhythm hits");
                };
                let pat_id = pat.id;
                let len = steps.len().max(1);
                if after {
                    self.cursor = (self.cursor + 1) % len;
                }
                let start = self.cursor;
                let edits: Vec<Edit> = hits
                    .into_iter()
                    .enumerate()
                    .take_while(|(offset, _)| start + offset < len)
                    .map(|(offset, active)| Edit::SetRhythm {
                        pattern: pat_id,
                        step: start + offset,
                        active,
                    })
                    .collect();
                ensure!(!edits.is_empty(), "Nothing fits at the current step");
                let pasted = edits.len();
                let out = if pasted == 1 {
                    self.apply(Action::Edit {
                        edit: edits.into_iter().next().unwrap(),
                    })?
                } else {
                    self.apply(Action::Transaction { edits })?
                };
                self.status = if pasted == 1 {
                    format!("Pasted rhythm hit at step {}", start + 1)
                } else {
                    format!("Pasted {pasted} rhythm steps at step {}", start + 1)
                };
                Ok(out)
            }
            Clip::Values { unit, span, points } => {
                ensure!(self.on_steps(), "Paste values in Pattern");
                ensure!(
                    self.scope == EditScope::Definition,
                    "Switch to Definition scope to paste values"
                );
                let pat = self
                    .selected_pattern_object()
                    .context("Select a literal value pattern")?;
                let PatternKind::Values {
                    unit: dest_unit,
                    period,
                    points: existing,
                    ..
                } = &pat.kind
                else {
                    bail!("Select a literal Values or Degrees pattern to paste values");
                };
                ensure!(
                    *dest_unit == unit,
                    "Clipboard holds {} values, target pattern is {}",
                    unit_label(unit),
                    unit_label(*dest_unit)
                );
                let pat_id = pat.id;
                let period = *period;
                let existing = existing.clone();
                if after {
                    self.cursor = (self.cursor + 1) % (period as usize).max(1);
                }
                let dest_start = self.cursor as u32;
                let dest_end = dest_start.saturating_add(span).min(period);
                let mut edits = Vec::new();
                let mut written_steps = HashSet::new();
                for pt in &points {
                    let target_step = dest_start.saturating_add(pt.step);
                    if target_step < period {
                        written_steps.insert(target_step);
                        edits.push(Edit::SetValue {
                            pattern: pat_id,
                            step: target_step,
                            value: Some(pt.value),
                        });
                    }
                }
                ensure!(
                    !written_steps.is_empty(),
                    "No clipboard value points fit within this pattern's period"
                );
                if span > 1 {
                    for old in &existing {
                        if old.step >= dest_start
                            && old.step < dest_end
                            && !written_steps.contains(&old.step)
                        {
                            edits.push(Edit::SetValue {
                                pattern: pat_id,
                                step: old.step,
                                value: None,
                            });
                        }
                    }
                }
                let written = written_steps.len();
                let out = if edits.len() == 1 {
                    self.apply(Action::Edit {
                        edit: edits.into_iter().next().unwrap(),
                    })?
                } else {
                    self.apply(Action::Transaction { edits })?
                };
                self.status = if written == 1 {
                    format!("Pasted value point at step {}", dest_start + 1)
                } else {
                    format!("Pasted {written} value points at step {}", dest_start + 1)
                };
                Ok(out)
            }
        }
    }
}

fn unit_label(unit: ValueUnit) -> &'static str {
    match unit {
        ValueUnit::Normalized => "normalized",
        ValueUnit::Degrees => "degree",
    }
}
