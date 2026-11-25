//! Overrides: parts launched over the arrangement on the single performance clock.
//! Releasing a lane brings its arrangement back, and Keep writes an override into
//! the arrangement as one undoable edit.
use super::*;
use cadence_core::material::{Exception, LanePart, MAX_BARS};
use std::collections::HashMap;

/// An active live override in Song mode that was kept via `Alt+c`/`Alt+C` and
/// grows bar-by-bar with the playhead as a single coalesced undo entry until
/// released or stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GrowingKeep {
    pub lane: Id,
    pub placement_id: Id,
    pub part: LanePart,
    pub anchor: u64,
    pub start_step: u32,
    pub end_step: u32,
}

impl App {
    /// Whether any lane currently has an active part override over the arrangement.
    pub fn overriding(&self) -> bool {
        self.live.overriding()
    }

    /// The song step a transport step maps to, or `None` past a continuing/stopping end.
    pub fn song_position_opt(&self, step: u64) -> Option<u64> {
        let length = self.doc.project().arrangement_steps().max(1) as u64;
        self.live.timeline().song_step(step, length)
    }

    /// Authoritative observed transport step for views: live audio step while playing,
    /// or `transport.next_step` while paused so cueing/rewinding reflects immediately.
    pub(crate) fn observed_transport_step(
        &self,
        snapshot: &cadence_core::runtime::AudioSnapshot,
    ) -> u64 {
        if snapshot.playing {
            snapshot.absolute_step
        } else {
            self.transport.next_step
        }
    }

    /// Continuous sub-step song position for smooth 60/120fps playheads.
    /// Returns `None` during solo audition or past a continuing/stopping end.
    pub fn song_position_smooth(
        &self,
        snapshot: &cadence_core::runtime::AudioSnapshot,
    ) -> Option<f64> {
        if self.is_solo_auditioning() {
            return None;
        }
        let step = self.observed_transport_step(snapshot);
        let song_step = self.song_position_opt(step)?;
        let frac = if snapshot.playing {
            f64::from(snapshot.step_fraction.clamp(0.0, 0.9999))
        } else {
            0.0
        };
        Some(song_step as f64 + frac)
    }

    /// Authoritative visible bar span for the Arrange deck (Placements, Overview, and Lane Roll).
    /// Base viewport (`100%`) is `song_bars.min(16)` bars at `arrange_zoom = 1.0`, scaling cleanly
    /// from `1` bar up to the song horizon (bounded by `MAX_VISIBLE_STEPS / 16`).
    pub fn arrange_visible_bars(&self) -> usize {
        let song_bars = self.doc.project().bars.max(1);
        let max_visible_bars = (cadence_core::arrangement_view::MAX_VISIBLE_STEPS / 16)
            .min(song_bars)
            .max(1);
        let default_bars = song_bars.min(16);
        let zoom = if self.arrange_ui.zoom.is_finite() && self.arrange_ui.zoom > 0.0 {
            self.arrange_ui.zoom
        } else {
            1.0
        };
        ((default_bars as f32 / zoom).round() as usize).clamp(1, max_visible_bars)
    }

    /// Effective Arrange zoom factor relative to the default `song_bars.min(16)`-bar viewport.
    pub fn arrange_effective_zoom(&self) -> f32 {
        let default_bars = self.doc.project().bars.clamp(1, 16) as f32;
        default_bars / self.arrange_visible_bars().max(1) as f32
    }

    /// Authoritative visible step span for the Pattern deck (`4..=64` steps, bounded by pattern length).
    /// Base viewport (`100%`) is `length.clamp(1, 16)` steps at `pattern_zoom = 1.0`.
    pub fn pattern_visible_steps(&self, length: usize) -> usize {
        let max_steps = length.clamp(1, 64);
        let min_steps = 4usize.min(max_steps);
        let default_steps = max_steps.min(16);
        let zoom = if self.view_state.pattern_zoom.is_finite() && self.view_state.pattern_zoom > 0.0
        {
            self.view_state.pattern_zoom
        } else {
            1.0
        };
        ((default_steps as f32 / zoom).round() as usize).clamp(min_steps, max_steps)
    }

    /// Effective Pattern zoom factor relative to the default `length.clamp(1, 16)`-step viewport.
    pub fn pattern_effective_zoom(&self, length: usize) -> f32 {
        let default_steps = length.clamp(1, 16) as f32;
        default_steps / self.pattern_visible_steps(length).max(1) as f32
    }

    /// Zoom the focused Arrange or Pattern deck horizontally (`direction > 0` zooms in).
    /// Guarantees every press changes the visible bar/step span monotonically until hitting bounds.
    /// Does not mutate zoom when neither Arrange nor Pattern is focused.
    pub fn zoom_view(&mut self, direction: i8) {
        if self.pattern_canvas_focused() {
            let length = self.current_length().max(1);
            let max_steps = length.clamp(1, 64);
            let min_steps = 4usize.min(max_steps);
            let default_steps = max_steps.min(16);
            let cur_steps = self.pattern_visible_steps(length);
            let target_steps = if direction > 0 {
                ((cur_steps as f32 / 1.25).round() as usize)
                    .min(cur_steps.saturating_sub(1))
                    .max(min_steps)
            } else {
                ((cur_steps as f32 * 1.25).round() as usize)
                    .max(cur_steps.saturating_add(1))
                    .min(max_steps)
            };
            let min_zoom = default_steps as f32 / max_steps as f32;
            let max_zoom = default_steps as f32 / min_steps as f32;
            self.view_state.pattern_zoom =
                (default_steps as f32 / target_steps as f32).clamp(min_zoom, max_zoom);
            let pct = self.view_state.pattern_zoom * 100.0;
            let unit = if target_steps == 1 { "step" } else { "steps" };
            if target_steps == length {
                self.status =
                    format!("Pattern zoom {pct:.0}% · full pattern ({target_steps} {unit})");
            } else {
                self.status = format!("Pattern zoom {pct:.0}% · {target_steps} {unit}");
            }
        } else if self.arrange_canvas_focused() {
            let song_bars = self.doc.project().bars.max(1);
            let max_visible_bars = (cadence_core::arrangement_view::MAX_VISIBLE_STEPS / 16)
                .min(song_bars)
                .max(1);
            let default_bars = song_bars.min(16);
            let cur_bars = self.arrange_visible_bars();
            let target_bars = if direction > 0 {
                ((cur_bars as f32 / 1.25).round() as usize)
                    .min(cur_bars.saturating_sub(1))
                    .max(1)
            } else {
                ((cur_bars as f32 * 1.25).round() as usize)
                    .max(cur_bars.saturating_add(1))
                    .min(max_visible_bars)
            };
            let min_zoom = default_bars as f32 / max_visible_bars as f32;
            let max_zoom = default_bars as f32;
            self.arrange_ui.zoom =
                (default_bars as f32 / target_bars as f32).clamp(min_zoom, max_zoom);
            self.arrange_ui.page_steps = None;
            let pct = self.arrange_ui.zoom * 100.0;
            let unit = if target_bars == 1 { "bar" } else { "bars" };
            if target_bars == song_bars {
                self.status = format!("Arrange zoom {pct:.0}% · full song ({target_bars} {unit})");
            } else {
                self.status = format!("Arrange zoom {pct:.0}% · {target_bars} {unit}");
            }
        } else {
            self.status = "Zoom applies to Arrange or Pattern · ga or gp to focus".into();
        }
    }

    /// Reset horizontal zoom on the focused Arrange or Pattern deck back to 100%.
    /// Does not mutate zoom when neither Arrange nor Pattern is focused.
    pub fn zoom_view_reset(&mut self) {
        if self.pattern_canvas_focused() {
            self.view_state.pattern_zoom = 1.0;
            let length = self.current_length().max(1);
            let steps = self.pattern_visible_steps(length);
            let unit = if steps == 1 { "step" } else { "steps" };
            self.status = if steps == length {
                format!("Pattern zoom 100% · full pattern ({steps} {unit})")
            } else {
                format!("Pattern zoom 100% · {steps} {unit}")
            };
        } else if self.arrange_canvas_focused() {
            self.arrange_ui.zoom = 1.0;
            self.arrange_ui.page_steps = None;
            let song_bars = self.doc.project().bars.max(1);
            let bars = self.arrange_visible_bars();
            let unit = if bars == 1 { "bar" } else { "bars" };
            self.status = if bars == song_bars {
                format!("Arrange zoom 100% · full song ({bars} {unit})")
            } else {
                format!("Arrange zoom 100% · {bars} {unit}")
            };
        } else {
            self.status = "Zoom applies to Arrange or Pattern · ga or gp to focus".into();
        }
    }

    /// The song position a transport step stands for (clamped to the last step
    /// past a continuing/stopping end when a numeric coordinate is required).
    pub fn song_position(&self, step: u64) -> u64 {
        let length = self.doc.project().arrangement_steps().max(1) as u64;
        self.live
            .timeline()
            .song_step(step, length)
            .unwrap_or_else(|| length.saturating_sub(1))
    }

    /// The parts a lane can play, in lane order.
    pub fn lane_parts(&self, lane: Id) -> Vec<LanePart> {
        self.lane(lane).map(|l| l.parts.clone()).unwrap_or_default()
    }

    /// Look up one entry in a lane's part list by zero-based index.
    pub(super) fn lane_part_at(&self, lane: Id, index: usize) -> Result<LanePart> {
        self.lane(lane)
            .context("Lane missing")?
            .parts
            .get(index)
            .copied()
            .with_context(|| format!("No part {} in this lane", index + 1))
    }

    /// The song step a lane's override is drawn from: where it landed in the
    /// current pass, or the start of the pass if it landed in an earlier one.
    pub fn override_from(&self, lane: Id) -> Option<u64> {
        let state = self.live.lane_state(lane)?;
        let seg = self.live.heard_segment();
        let now = self.live.next_step;
        let length = self.doc.project().arrangement_steps().max(1) as u64;
        let song = seg.song_step(now, length)?;
        let floor = seg.region.map_or(0, |(start, _)| start);
        let elapsed = now.saturating_sub(state.anchor.max(seg.from));
        Some(song - elapsed.min(song.saturating_sub(floor)))
    }

    /// Launches and releases waiting to land on a lane: the song step each
    /// lands on, and the part a launch brings (`None` for a release).
    pub fn pending_on_lane(&self, lane: Id) -> Vec<(u64, Option<Id>)> {
        self.live
            .pending
            .iter()
            .filter_map(|p| match p.change {
                cadence_core::live::Change::Launch { lane: l, part } if l == lane => {
                    Some((self.song_position(p.at), Some(part.part)))
                }
                cadence_core::live::Change::Release { lane: l } if l == lane => {
                    Some((self.song_position(p.at), None))
                }
                _ => None,
            })
            .collect()
    }

    /// Gestures collected while staging that act on a lane, with the song
    /// step they would land on if fired now: the part a launch brings, or
    /// `None` for a release.
    pub fn staged_on_lane(&self, lane: Id) -> Vec<(u64, Option<Id>)> {
        let project = self.doc.project();
        let Some(staging) = self.staging.as_ref() else {
            return Vec::new();
        };
        let Ok(at) = self.live.boundary(project, project.timing.launch) else {
            return Vec::new();
        };
        let song = self.song_position(at);
        staging
            .iter()
            .filter_map(|change| match change {
                cadence_core::live::StagedChange::Launch { lane: l, part } if *l == lane => {
                    Some((song, Some(part.part)))
                }
                cadence_core::live::StagedChange::Release { lane: l } if *l == lane => {
                    Some((song, None))
                }
                _ => None,
            })
            .collect()
    }

    /// Song step where staged changes would land if fired now, when staging is active.
    pub fn staged_boundary_step(&self) -> Option<u64> {
        self.staging.as_ref()?;
        let project = self.doc.project();
        let at = self.live.boundary(project, project.timing.launch).ok()?;
        Some(self.song_position(at))
    }

    /// Staged mute target for `lane`, if a mute/unmute gesture is collected for it.
    pub fn staged_lane_mute(&self, lane: Id) -> Option<bool> {
        self.staging.as_ref()?.iter().rev().find_map(|c| match c {
            cadence_core::live::StagedChange::Mute { lane: l, muted } if *l == lane => Some(*muted),
            _ => None,
        })
    }

    /// Staged solo target for `lane`, if a solo/unsolo gesture is collected for it.
    pub fn staged_lane_solo(&self, lane: Id) -> Option<bool> {
        self.staging.as_ref()?.iter().rev().find_map(|c| match c {
            cadence_core::live::StagedChange::Solo { lane: l, solo } if *l == lane => Some(*solo),
            _ => None,
        })
    }

    /// Staged launch `LanePart` for `lane`, if one is collected.
    pub fn staged_launch_on_lane(&self, lane: Id) -> Option<LanePart> {
        self.staging.as_ref()?.iter().rev().find_map(|c| match c {
            cadence_core::live::StagedChange::Launch { lane: l, part } if *l == lane => Some(*part),
            _ => None,
        })
    }

    /// Whether a release gesture is currently staged for `lane`.
    pub fn staged_release_on_lane(&self, lane: Id) -> bool {
        self.staging.as_ref().is_some_and(|s| {
            s.iter()
                .any(|c| matches!(c, cadence_core::live::StagedChange::Release { lane: l } if *l == lane))
        })
    }

    /// Staged section jump target ID, if one is collected.
    pub fn staged_section(&self) -> Option<Id> {
        self.staging.as_ref()?.iter().rev().find_map(|c| match c {
            cadence_core::live::StagedChange::JumpSection { section } => Some(*section),
            _ => None,
        })
    }

    /// Effective lane muted state accounting for any staged mute toggle on `lane`.
    pub(crate) fn effective_lane_muted(&self, lane: Id) -> bool {
        self.staged_lane_mute(lane)
            .unwrap_or_else(|| self.doc.flags().lane_muted(lane))
    }

    /// Effective lane solo state accounting for any staged solo toggle on `lane`.
    pub(crate) fn effective_lane_solo(&self, lane: Id) -> bool {
        self.staged_lane_solo(lane)
            .unwrap_or_else(|| self.doc.flags().lane_solo(lane))
    }

    /// Bars where a song edit waits to be heard.
    pub fn pending_edit_bars(&self) -> Vec<u64> {
        self.live
            .pending
            .iter()
            .filter(|p| matches!(p.change, cadence_core::live::Change::Song { .. }))
            .map(|p| self.song_position(p.at) / 16)
            .collect()
    }

    /// The song step currently being heard.
    pub fn song_step_heard(&self) -> u64 {
        self.song_position(self.transport.step)
    }

    /// Keep a single lane's override: in a loop it fills the region; in song mode
    /// it starts at the override's first bar and grows with the playhead until
    /// released or stopped — all inside one undoable edit.
    pub(super) fn keep_override(&mut self, lane: Id) -> Result<Outcome> {
        ensure!(
            self.live.lane_state(lane).is_some(),
            "This lane plays the arrangement; launch a part to keep"
        );
        self.keep_override_lanes(&[lane])
    }

    /// Keep every active lane's override in ONE single undoable transaction.
    pub(super) fn keep_all_overrides(&mut self) -> Result<Outcome> {
        let lanes: Vec<Id> = self.live.active().iter().map(|a| a.lane).collect();
        ensure!(!lanes.is_empty(), "Nothing is playing over the arrangement");
        self.keep_override_lanes(&lanes)
    }

    fn keep_override_lanes(&mut self, lanes: &[Id]) -> Result<Outcome> {
        let seg = self.live.heard_segment();
        let in_song = seg.region.is_none();
        let current_bars = self.doc.project().bars;
        let max_steps = (MAX_BARS * 16) as u32;

        // Allocate an ID pool generous enough for splits + new placements.
        let mut next_id = self.doc.fresh_ids(1)?[0];
        let mut edits = Vec::new();
        let mut max_end: u32 = (current_bars * 16) as u32;
        let mut new_growing = Vec::new();
        let mut summary = Vec::new();

        for &lane in lanes {
            let state = *self
                .live
                .lane_state(lane)
                .context("This lane plays the arrangement; launch a part to keep")?;
            let part = state.part;
            ensure!(self.part(part.part).is_some(), "Part missing");
            let (start, end) = match seg.region {
                Some((start, len)) => (start as u32, (start + len) as u32),
                None => {
                    let from = (seg
                        .song_step_unbounded(state.anchor.max(seg.from))
                        .unwrap_or(0)
                        / 16
                        * 16) as u32;
                    let now = (seg
                        .song_step_unbounded(self.live.next_step)
                        .unwrap_or(u64::from(from))
                        .div_ceil(16)
                        * 16) as u32;
                    let end = now.max(from + 16).min(max_steps);
                    ensure!(
                        from < max_steps && end > from,
                        "Override is past the {MAX_BARS}-bar arrangement limit"
                    );
                    (from, end)
                }
            };
            let lane_object = self.lane(lane).context("Lane missing")?.clone();
            let mut placements = lane_object.placements.clone();
            let placement_id = apply_placement_span(
                &mut placements,
                Placement {
                    id: 0,
                    part: part.part,
                    start_step: start,
                    length_steps: end - start,
                    phase: part.phase,
                    offset_steps: part.offset_steps,
                    exceptions: Vec::new(),
                },
                &mut next_id,
            );
            edits.push(Edit::PutLane {
                lane: Lane {
                    placements,
                    ..lane_object.clone()
                },
            });
            max_end = max_end.max(end);
            if in_song {
                new_growing.push(GrowingKeep {
                    lane,
                    placement_id,
                    part,
                    anchor: state.anchor,
                    start_step: start,
                    end_step: end,
                });
            }
            let part_name = self
                .part(part.part)
                .map_or("the part".into(), |b| b.name.clone());
            summary.push((part_name, lane_object.name, start / 16 + 1, end / 16));
        }

        if max_end > (current_bars * 16) as u32 {
            edits.insert(
                0,
                Edit::SetBars {
                    bars: max_end.div_ceil(16) as usize,
                },
            );
        }

        let out = self.apply(Action::Transaction { edits })?;
        if in_song && self.transport.playing {
            self.takes.growing_keeps = new_growing;
            self.takes.growing_keep_revision = Some(self.doc.revision());
        } else {
            self.takes.growing_keeps.clear();
            self.takes.growing_keep_revision = None;
        }

        if summary.len() == 1 {
            let (part_name, lane_name, b0, b1) = &summary[0];
            self.status = if in_song && self.transport.playing {
                format!("Keeping {part_name} on {lane_name} from bar {b0} (growing…)")
            } else {
                format!("Kept {part_name} on {lane_name} in bars {b0}–{b1}")
            };
        } else {
            self.status = format!(
                "Kept {} lane overrides in one edit{}",
                summary.len(),
                if in_song && self.transport.playing {
                    " (growing…)"
                } else {
                    ""
                }
            );
        }
        Ok(out)
    }

    /// Extend any active Song-mode kept overrides bar-by-bar in place without
    /// creating extra undo entries. Returns `true` if the arrangement grew.
    pub(super) fn grow_active_keeps(&mut self) -> bool {
        if self.takes.growing_keeps.is_empty() {
            return false;
        }
        // Stop coalescing if the user performed any other document edit or switched to loop.
        let seg = self.live.heard_segment();
        if seg.region.is_some() || self.takes.growing_keep_revision != Some(self.doc.revision()) {
            self.takes.growing_keeps.clear();
            self.takes.growing_keep_revision = None;
            return false;
        }
        let max_steps = (MAX_BARS * 16) as u32;
        let now_end = (seg
            .song_step_unbounded(self.live.next_step)
            .unwrap_or(0)
            .div_ceil(16)
            * 16)
            .min(u64::from(max_steps)) as u32;

        // Retain only keeps whose live override is still active and unchanged.
        self.takes.growing_keeps.retain(|k| {
            self.live
                .lane_state(k.lane)
                .is_some_and(|st| st.anchor == k.anchor && st.part == k.part)
        });
        if self.takes.growing_keeps.is_empty() {
            self.takes.growing_keep_revision = None;
            return false;
        }

        let needs_grow = self
            .takes
            .growing_keeps
            .iter()
            .any(|k| now_end > k.end_step);
        if !needs_grow {
            return false;
        }

        let Ok(fresh) = self.doc.fresh_ids(1) else {
            return false;
        };
        let mut next_id = fresh[0];
        let mut edits = Vec::new();
        let mut max_end = (self.doc.project().bars * 16) as u32;
        let mut updated = Vec::new();

        for keep in &self.takes.growing_keeps {
            if now_end <= keep.end_step {
                updated.push(keep.clone());
                continue;
            }
            let Some(lane_obj) = self.lane(keep.lane).cloned() else {
                continue;
            };
            let mut placements = lane_obj.placements.clone();
            // Remove the earlier version of our growing placement, then lay down the extended span
            // preserving the same placement ID cleanly.
            placements.retain(|p| p.id != keep.placement_id);
            apply_placement_span_with_id(
                &mut placements,
                Placement {
                    id: keep.placement_id,
                    part: keep.part.part,
                    start_step: keep.start_step,
                    length_steps: now_end - keep.start_step,
                    phase: keep.part.phase,
                    offset_steps: keep.part.offset_steps,
                    exceptions: Vec::new(),
                },
                &mut next_id,
            );
            edits.push(Edit::PutLane {
                lane: Lane {
                    placements,
                    ..lane_obj
                },
            });
            max_end = max_end.max(now_end);
            let mut grown = keep.clone();
            grown.end_step = now_end;
            updated.push(grown);
        }

        if edits.is_empty() {
            return false;
        }
        if max_end > (self.doc.project().bars * 16) as u32 {
            edits.insert(
                0,
                Edit::SetBars {
                    bars: max_end.div_ceil(16) as usize,
                },
            );
        }
        let Ok((next_project, compiled)) = self.doc.prepare(&edits) else {
            return false;
        };
        // Growing Keep intentionally installs in place (not via CoalesceKey) so live
        // navigation/jam keys calling `break_coalesce()` never fragment one kept passage
        // into per-bar undo snapshots; `growing_keep_revision` already stops growth on any
        // real user document edit.
        self.doc.install_in_place(next_project, compiled);
        self.takes.growing_keeps = updated;
        self.takes.growing_keep_revision = Some(self.doc.revision());
        let heard_flags = self.live.heard_flags().clone();
        self.live
            .update_heard_project(self.doc.project(), &heard_flags);
        let _ = self.refresh_audio();
        true
    }

    /// Keep an inclusive 1-based bar range `[start_bar..=end_bar]` from the active
    /// (or most recently sealed) take into the arrangement as one undoable edit:
    /// - Song-mode spans land at the song bars where they were heard.
    /// - Loop-mode spans land sequentially starting at the Arrange cursor bar.
    /// - Muted spans leave gaps.
    /// - Definitions edited mid-take are forked automatically so every kept span
    ///   retains live editable relationships reproducing what was heard.
    pub(super) fn keep_take_range(&mut self, start_bar: usize, end_bar: usize) -> Result<Outcome> {
        ensure!(
            start_bar >= 1 && end_bar >= start_bar,
            "Bar range must be 1-based with start <= end"
        );
        let take = self
            .current_take()
            .context("No take has been recorded yet")?;
        let total_bars = take.steps().div_ceil(16).max(1) as usize;
        ensure!(
            start_bar <= total_bars,
            "Take only has {total_bars} bar(s); bar {start_bar} is out of range"
        );
        let clamped_end = end_bar.min(total_bars);
        let range_start = take.header.start_step + (start_bar as u64 - 1) * 16;
        let range_end = (take.header.start_step + clamped_end as u64 * 16).min(take.end_step);
        ensure!(range_end > range_start, "Selected take range is empty");

        let states = take.states()?;
        let base_project = self.doc.project().clone();
        let mut resolver = KeepResolver::new(&base_project, self.doc.fresh_ids(1)?[0]);
        let mut lane_placements: HashMap<Id, Vec<Placement>> = base_project
            .lanes
            .iter()
            .map(|l| (l.id, l.placements.clone()))
            .collect();

        // Collect all bar-aligned boundaries & state change points within [range_start, range_end).
        let mut cuts = vec![range_start, range_end];
        let mut b = (range_start / 16 + 1) * 16;
        while b < range_end {
            cuts.push(b);
            b += 16;
        }
        for (at, _) in &states {
            if *at > range_start && *at < range_end {
                cuts.push(*at);
            }
        }
        cuts.sort_unstable();
        cuts.dedup();

        let mut loop_write_cursor = self.arrange_ui.step / 16 * 16;
        let max_steps = (MAX_BARS * 16) as u32;
        let mut max_end = (base_project.bars * 16) as u32;
        let mut spans_written = 0usize;

        for pair in cuts.windows(2) {
            let (t0, t1) = (pair[0], pair[1]);
            let len = (t1 - t0) as u32;
            if len == 0 {
                continue;
            }
            let st_idx = states
                .partition_point(|(from, _)| *from <= t0)
                .saturating_sub(1);
            let state = &states[st_idx].1;
            let seg = state.timeline.unwrap_or_else(|| {
                cadence_core::timeline::Segment::new(0, 0, None, state.project.end)
            });
            let song_len = state.project.arrangement_steps().max(1) as u64;
            let Some(heard_song_step) = seg.song_step(t0, song_len) else {
                continue;
            };
            let dest_start = if seg.region.is_some() {
                let d = loop_write_cursor;
                loop_write_cursor = loop_write_cursor.saturating_add(len);
                d
            } else {
                seg.song_step_unbounded(t0).unwrap_or(heard_song_step) as u32
            };
            ensure!(
                dest_start.saturating_add(len) <= max_steps,
                "Kept range exceeds the {MAX_BARS}-bar arrangement limit"
            );
            let any_solo = state
                .project
                .lanes
                .iter()
                .any(|l| state.flags.lane_solo(l.id));

            for lane in &state.project.lanes {
                let Some(list) = lane_placements.get_mut(&lane.id) else {
                    continue;
                };
                let muted = state.flags.lane_muted(lane.id)
                    || (any_solo && !state.flags.lane_solo(lane.id));
                if muted {
                    // Muted span creates a clean gap in the destination window.
                    clear_placement_range(
                        list,
                        dest_start,
                        dest_start + len,
                        &mut resolver.next_id,
                    );
                    continue;
                }

                // Determine what played on this lane at step `t0`.
                let active_span = if let Some(ov) = state.active.iter().find(|a| a.lane == lane.id)
                {
                    let offset = match ov.part.phase {
                        PhaseMode::Song => {
                            let elapsed_from_song = heard_song_step as i64 - dest_start as i64;
                            ov.part.offset_steps + elapsed_from_song as i32
                        }
                        PhaseMode::Restart => {
                            let elapsed = t0.saturating_sub(ov.anchor) as i32;
                            ov.part.offset_steps + elapsed
                        }
                    };
                    Some((ov.part.part, ov.part.phase, offset, Vec::new()))
                } else {
                    lane.placements
                        .iter()
                        .find(|p| {
                            let s = u64::from(p.start_step);
                            let e = u64::from(p.start_step + p.length_steps);
                            (s..e).contains(&heard_song_step)
                        })
                        .map(|p| {
                            let skip = (heard_song_step - u64::from(p.start_step)) as u32;
                            let offset = match p.phase {
                                PhaseMode::Song => {
                                    p.offset_steps
                                        + (heard_song_step as i64 - dest_start as i64) as i32
                                }
                                PhaseMode::Restart => p.offset_steps + skip as i32,
                            };
                            let exceptions = p
                                .exceptions
                                .iter()
                                .filter(|ex| ex.step >= skip && ex.step < skip + len)
                                .map(|ex| Exception {
                                    step: ex.step - skip,
                                    value: ex.value.clone(),
                                })
                                .collect();
                            (p.part, p.phase, offset, exceptions)
                        })
                };

                let Some((orig_part_id, phase, offset_steps, exceptions)) = active_span else {
                    clear_placement_range(
                        list,
                        dest_start,
                        dest_start + len,
                        &mut resolver.next_id,
                    );
                    continue;
                };

                let Some(state_part) = state.project.part(orig_part_id) else {
                    continue;
                };
                if base_project.instrument(state_part.instrument).is_none() {
                    continue;
                }
                let resolved_part_id = resolver.resolve_part(state_part, &state.project)?;
                apply_placement_span(
                    list,
                    Placement {
                        id: 0,
                        part: resolved_part_id,
                        start_step: dest_start,
                        length_steps: len,
                        phase,
                        offset_steps,
                        exceptions,
                    },
                    &mut resolver.next_id,
                );
                coalesce_adjacent_placements(list);
                max_end = max_end.max(dest_start + len);
                spans_written += 1;
            }
        }

        let created_patterns = resolver.created_patterns;
        let created_parts = resolver.created_parts;
        ensure!(
            spans_written > 0 || !created_patterns.is_empty(),
            "No audible lane passages in take bars {start_bar}–{clamped_end}"
        );

        let mut edits = Vec::new();
        if max_end > (base_project.bars * 16) as u32 {
            edits.push(Edit::SetBars {
                bars: max_end.div_ceil(16) as usize,
            });
        }
        for pattern in created_patterns {
            edits.push(Edit::PutPattern { pattern });
        }
        for part in created_parts {
            edits.push(Edit::PutPart { part });
        }
        for lane in &base_project.lanes {
            if let Some(updated) = lane_placements.remove(&lane.id) {
                if updated != lane.placements {
                    edits.push(Edit::PutLane {
                        lane: Lane {
                            placements: updated,
                            ..lane.clone()
                        },
                    });
                }
            }
        }
        ensure!(
            !edits.is_empty(),
            "Arrangement already matches take bars {start_bar}–{clamped_end}"
        );
        let forks = edits
            .iter()
            .filter(|e| matches!(e, Edit::PutPattern { .. }))
            .count();
        let out = self.apply(Action::Transaction { edits })?;
        self.status = if forks > 0 {
            format!(
                "Kept take bars {start_bar}–{clamped_end} (forked {forks} mid-take definition{})",
                if forks == 1 { "" } else { "s" }
            )
        } else {
            format!("Kept take bars {start_bar}–{clamped_end} into arrangement")
        };
        Ok(out)
    }
}

/// Remove or split any existing placements overlapping `[start, end)`.
fn clear_placement_range(placements: &mut Vec<Placement>, start: u32, end: u32, next_id: &mut Id) {
    let existing = std::mem::take(placements);
    for pl in existing {
        let (from, to) = (pl.start_step, pl.start_step + pl.length_steps);
        if to <= start || from >= end {
            placements.push(pl);
            continue;
        }
        if from < start {
            placements.push(trimmed(&pl, 0, start - from));
        }
        if to > end {
            let mut tail = trimmed(&pl, end - from, to - end);
            if from < start {
                tail.id = *next_id;
                *next_id += 1;
            }
            placements.push(tail);
        }
    }
}

/// Insert `incoming` over `[start, end)`, trimming/splitting any covered placements,
/// allocating a fresh placement ID and returning it.
fn apply_placement_span(
    placements: &mut Vec<Placement>,
    mut incoming: Placement,
    next_id: &mut Id,
) -> Id {
    let id = *next_id;
    *next_id += 1;
    incoming.id = id;
    apply_placement_span_with_id(placements, incoming, next_id);
    id
}

fn apply_placement_span_with_id(
    placements: &mut Vec<Placement>,
    incoming: Placement,
    next_id: &mut Id,
) {
    let start = incoming.start_step;
    let end = start + incoming.length_steps;
    clear_placement_range(placements, start, end, next_id);
    placements.push(incoming);
    placements.sort_by_key(|p| p.start_step);
}

/// Merge adjacent placements on the same lane that share identical part, phase,
/// continuous offset, and have no exceptions.
fn coalesce_adjacent_placements(placements: &mut Vec<Placement>) {
    placements.sort_by_key(|p| p.start_step);
    let mut merged: Vec<Placement> = Vec::with_capacity(placements.len());
    for pl in placements.drain(..) {
        if let Some(prev) = merged.last_mut().filter(|prev| {
            prev.part == pl.part
                && prev.phase == pl.phase
                && prev.start_step + prev.length_steps == pl.start_step
                && prev.exceptions.is_empty()
                && pl.exceptions.is_empty()
                && match prev.phase {
                    PhaseMode::Song => prev.offset_steps == pl.offset_steps,
                    PhaseMode::Restart => {
                        prev.offset_steps + prev.length_steps as i32 == pl.offset_steps
                    }
                }
        }) {
            prev.length_steps += pl.length_steps;
        } else {
            merged.push(pl);
        }
    }
    *placements = merged;
}

struct KeepResolver<'a> {
    base_project: &'a Project,
    next_id: Id,
    created_patterns: Vec<Pattern>,
    created_parts: Vec<Part>,
    pat_fork_cache: HashMap<(Id, String), Id>,
    part_fork_cache: HashMap<(Id, String), Id>,
}

impl<'a> KeepResolver<'a> {
    fn new(base_project: &'a Project, next_id: Id) -> Self {
        Self {
            base_project,
            next_id,
            created_patterns: Vec::new(),
            created_parts: Vec::new(),
            pat_fork_cache: HashMap::new(),
            part_fork_cache: HashMap::new(),
        }
    }

    fn resolve_pattern(&mut self, orig_id: Id, state_project: &Project) -> Result<Id> {
        let state_pat = state_project
            .pattern(orig_id)
            .with_context(|| format!("Take references missing pattern {orig_id}"))?;
        let mut remapped: HashMap<Id, Id> = HashMap::new();
        for src in state_pat.kind.sources() {
            let resolved = self.resolve_pattern(src, state_project)?;
            remapped.insert(src, resolved);
        }
        let rewritten_kind = state_pat
            .kind
            .remap_sources(|id| remapped.get(&id).copied().unwrap_or(id));

        if self
            .base_project
            .pattern(orig_id)
            .is_some_and(|p| p.kind == rewritten_kind)
        {
            return Ok(orig_id);
        }
        let key = (orig_id, serde_json::to_string(&rewritten_kind)?);
        if let Some(&cached) = self.pat_fork_cache.get(&key) {
            return Ok(cached);
        }
        let id = self.next_id;
        self.next_id += 1;
        let base_name = format!("{} take", state_pat.name);
        let name = crate::unique_name(
            &base_name,
            self.base_project
                .patterns
                .iter()
                .chain(self.created_patterns.iter())
                .map(|p| p.name.as_str()),
            None,
        );
        self.created_patterns.push(Pattern {
            id,
            name,
            kind: rewritten_kind,
        });
        self.pat_fork_cache.insert(key, id);
        Ok(id)
    }

    fn resolve_part(&mut self, state_part: &Part, state_project: &Project) -> Result<Id> {
        let pattern = self.resolve_pattern(state_part.pattern, state_project)?;
        let mut parameter_links = Vec::with_capacity(state_part.parameter_links.len());
        for link in &state_part.parameter_links {
            let lp = self.resolve_pattern(link.pattern, state_project)?;
            parameter_links.push(cadence_core::material::ParameterLink {
                pattern: lp,
                target: link.target,
            });
        }
        if self.base_project.part(state_part.id).is_some_and(|b| {
            b.pattern == pattern
                && b.instrument == state_part.instrument
                && b.transpose == state_part.transpose
                && (b.velocity_scale - state_part.velocity_scale).abs() < 1e-6
                && b.parameter_links == parameter_links
        }) {
            return Ok(state_part.id);
        }
        let sig = format!(
            "{pattern}:{}:{}:{:.4}:{:?}",
            state_part.instrument, state_part.transpose, state_part.velocity_scale, parameter_links
        );
        let key = (state_part.id, sig);
        if let Some(&cached) = self.part_fork_cache.get(&key) {
            return Ok(cached);
        }
        let id = self.next_id;
        self.next_id += 1;
        let base_name = format!("{} take", state_part.name);
        let name = crate::unique_name(
            &base_name,
            self.base_project
                .parts
                .iter()
                .chain(self.created_parts.iter())
                .map(|p| p.name.as_str()),
            None,
        );
        self.created_parts.push(Part {
            id,
            name,
            pattern,
            instrument: state_part.instrument,
            transpose: state_part.transpose,
            velocity_scale: state_part.velocity_scale,
            parameter_links,
        });
        self.part_fork_cache.insert(key, id);
        Ok(id)
    }
}

/// The part of a placement from `skip` steps in, `length` steps long.
fn trimmed(placement: &Placement, skip: u32, length: u32) -> Placement {
    let mut part = placement.clone();
    part.start_step += skip;
    part.length_steps = length;
    if part.phase == PhaseMode::Restart {
        part.offset_steps += skip as i32;
    }
    part.exceptions = placement
        .exceptions
        .iter()
        .filter(|o| (skip..skip + length).contains(&o.step))
        .map(|o| Exception {
            step: o.step - skip,
            value: o.value.clone(),
        })
        .collect();
    part
}
