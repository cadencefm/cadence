//! What is playing right now: the live performance state, each lane's override,
//! live flags, timeline segments, and changes waiting for their boundary.
//!
//! Live state is not authored. It never enters undo history or changes the
//! song revision. Song edits made while performing are applied to the document
//! at once (so editors show them), and the performance records the document
//! snapshot to be *heard* from the change's boundary.
use crate::material::{
    Audition, Boundary, Edit, Id, LanePart, LiveFlags, LiveSegment, Override, Project,
    LIVE_RING_STEPS, STEPS_PER_BAR,
};
use crate::rack::RackPatch;
use crate::score::ScoreWindow;
use crate::timeline::{Segment, Timeline};
use anyhow::Result;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub enum Change {
    /// The document as it sounds from the boundary.
    Song {
        project: Arc<Project>,
        edits: Option<Vec<Edit>>,
    },
    /// Live lane/channel mute and solo flags heard from the boundary.
    Flags {
        flags: LiveFlags,
    },
    Launch {
        lane: Id,
        part: LanePart,
    },
    /// Arrangement timeline segment heard from the boundary.
    Timeline {
        segment: Segment,
    },
    Release {
        lane: Id,
    },
    ReleaseAll,
}

#[derive(Clone, Debug)]
pub struct Pending {
    pub id: u64,
    pub at: u64,
    pub change: Change,
    pub label: String,
}

/// A gesture collected while staging, fired with the others on one boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StagedChange {
    Launch { lane: Id, part: LanePart },
    Release { lane: Id },
    Mute { lane: Id, muted: bool },
    Solo { lane: Id, solo: bool },
    JumpSection { section: Id },
}

impl StagedChange {
    pub fn label(&self, project: &Project) -> String {
        let lane = |id| project.lane(id).map_or("?".to_string(), |l| l.name.clone());
        match self {
            Self::Launch { lane: id, part } => format!(
                "launch {} in {}",
                project.part(part.part).map_or("?", |p| p.name.as_str()),
                lane(*id)
            ),
            Self::Release { lane: id } => format!("release {}", lane(*id)),
            Self::Mute { lane: id, muted } => {
                format!("{} {}", if *muted { "mute" } else { "unmute" }, lane(*id))
            }
            Self::Solo { lane: id, solo } => {
                format!("{} {}", if *solo { "solo" } else { "unsolo" }, lane(*id))
            }
            Self::JumpSection { section } => format!(
                "jump to {}",
                project
                    .sections
                    .iter()
                    .find(|s| s.id == *section)
                    .map_or("?", |s| s.name.as_str())
            ),
        }
    }
}

/// A change that has been heard, with the transport step it landed on.
#[derive(Clone, Debug)]
pub struct Landed {
    pub at: u64,
    pub change: Change,
    pub label: String,
}

/// Complete performance state at one instant, folded identically by live
/// playback and take replay.
#[derive(Clone, Debug)]
pub struct PerformanceState {
    pub project: Arc<Project>,
    pub flags: LiveFlags,
    pub active: Vec<Override>,
    pub timeline: Option<Segment>,
}

impl PerformanceState {
    pub fn apply(&mut self, at: u64, change: &Change) {
        match change {
            Change::Timeline { segment } => self.timeline = Some(*segment),
            Change::Song { project, .. } => {
                self.project = Arc::clone(project);
                self.active.retain(|a| self.project.override_valid(a));
                if let Some(seg) = &mut self.timeline {
                    seg.end = self.project.end;
                }
            }
            Change::Flags { flags } => self.flags = flags.clone(),
            Change::Launch { lane, part } => {
                launch_into(&self.project, &mut self.active, *lane, *part, at);
            }
            Change::Release { lane } => self.active.retain(|a| a.lane != *lane),
            Change::ReleaseAll => self.active.clear(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Performance {
    /// Landed state after every change reached by the transport.
    state: PerformanceState,
    /// Landed plus pending timeline segments, for O(log n) future step queries.
    timeline: Timeline,
    /// Waiting changes, in the order they were made.
    pub pending: Vec<Pending>,
    /// Transient pattern audition layered on (or soloed on) the ring.
    /// Never recorded in takes and never kept by Keep.
    pub audition: Option<Audition>,
    /// The transport step triggered next.
    pub next_step: u64,
    pub playing: bool,
    /// First step of the compiled ring window.
    pub base: u64,
    /// Increments with every live change; separate from the song revision.
    pub sequence: u64,
    next_id: u64,
    /// The last part each lane played, for toggling it back on.
    pub last_part: Vec<(Id, LanePart)>,
}

impl Performance {
    pub fn new(project: &Project) -> Self {
        Self::new_with_flags(project, LiveFlags::default())
    }

    pub fn new_with_flags(project: &Project, flags: LiveFlags) -> Self {
        let initial = Segment::new(0, 0, None, project.end);
        Self {
            state: PerformanceState {
                project: Arc::new(project.clone()),
                flags,
                active: Vec::new(),
                timeline: Some(initial),
            },
            timeline: Timeline::new(initial),
            pending: Vec::new(),
            audition: None,
            next_step: 0,
            playing: false,
            base: 0,
            sequence: 0,
            next_id: 1,
            last_part: Vec::new(),
        }
    }

    pub fn overriding(&self) -> bool {
        !self.state.active.is_empty()
    }

    pub fn heard(&self) -> &Project {
        &self.state.project
    }

    pub fn heard_flags(&self) -> &LiveFlags {
        &self.state.flags
    }

    pub fn active(&self) -> &[Override] {
        &self.state.active
    }

    pub fn active_mut(&mut self) -> &mut Vec<Override> {
        &mut self.state.active
    }

    pub fn heard_segment(&self) -> Segment {
        self.state
            .timeline
            .expect("performance always has an active timeline segment")
    }

    pub fn timeline(&self) -> &Timeline {
        &self.timeline
    }

    pub fn pending_timeline(&self) -> Option<(u64, Segment)> {
        self.pending.iter().rev().find_map(|p| match p.change {
            Change::Timeline { segment } => Some((p.at, segment)),
            _ => None,
        })
    }

    pub fn set_segment(&mut self, segment: Segment) {
        self.state.timeline = Some(segment);
        self.rebuild_timeline();
        self.sequence += 1;
    }

    fn rebuild_timeline(&mut self) {
        let mut tl = Timeline::new(self.heard_segment());
        let mut remaps: Vec<Segment> = self
            .pending
            .iter()
            .filter_map(|p| match p.change {
                Change::Timeline { segment } => Some(segment),
                _ => None,
            })
            .collect();
        remaps.sort_by_key(|s| s.from);
        for segment in remaps {
            tl.remap(segment);
        }
        self.timeline = tl;
    }

    /// Forget pending changes and treat `project` and `flags` as heard now.
    pub fn reset_heard(&mut self, project: &Project, flags: &LiveFlags) {
        self.pending.clear();
        self.update_heard_project(project, flags);
    }

    /// Treat `project` and `flags` as heard now while preserving any pending live changes.
    pub fn update_heard_project(&mut self, project: &Project, flags: &LiveFlags) {
        self.state.project = Arc::new(project.clone());
        self.state.flags = flags.clone();
        self.state.active.retain(|a| project.override_valid(a));
        if let Some(seg) = &mut self.state.timeline {
            seg.end = project.end;
        }
        self.rebuild_timeline();
        self.sequence += 1;
    }

    /// The step a change made now lands on.
    pub fn boundary(&self, project: &Project, boundary: Boundary) -> Result<u64> {
        if !self.playing {
            return Ok(self.next_step);
        }
        let Some(quantum) = boundary.quantum(project)? else {
            return Ok(self.next_step);
        };
        let next = self.next_step;
        let upcoming = next.div_ceil(quantum) * quantum;
        if next > 0 && upcoming > next {
            // A press just after a boundary usually meant that boundary.
            let last = next - 1;
            let previous = last / quantum * quantum;
            let missed = last - previous + 1;
            if missed <= u64::from(project.timing.late_tolerance_steps) && previous != upcoming {
                return Ok(previous);
            }
        }
        Ok(upcoming)
    }

    fn lane_of(change: &Change) -> Option<Id> {
        match change {
            Change::Launch { lane, .. } | Change::Release { lane } => Some(*lane),
            _ => None,
        }
    }

    /// Queue a change. Song and flag changes never overtake one another. A
    /// launch or release replaces any pending launch or release for the same lane.
    pub fn enqueue(&mut self, at: u64, change: Change, label: String) -> u64 {
        let mut at = at;
        match &change {
            Change::Song { .. } => {
                if let Some(last) = self
                    .pending
                    .iter()
                    .filter(|p| matches!(p.change, Change::Song { .. }))
                    .map(|p| p.at)
                    .max()
                {
                    at = at.max(last);
                }
            }
            Change::Flags { .. } => {
                if let Some(last) = self
                    .pending
                    .iter()
                    .filter(|p| matches!(p.change, Change::Flags { .. }))
                    .map(|p| p.at)
                    .max()
                {
                    at = at.max(last);
                }
            }
            Change::ReleaseAll => self
                .pending
                .retain(|p| !matches!(p.change, Change::Launch { .. } | Change::Release { .. })),
            // One timeline remap waits at a time: the newest replaces it.
            Change::Timeline { .. } => self
                .pending
                .retain(|p| !matches!(p.change, Change::Timeline { .. })),
            _ => {
                if let Some(lane) = Self::lane_of(&change) {
                    self.pending
                        .retain(|p| Self::lane_of(&p.change) != Some(lane));
                }
            }
        }
        let id = self.next_id;
        self.next_id += 1;
        let is_timeline = matches!(change, Change::Timeline { .. });
        self.pending.push(Pending {
            id,
            at,
            change,
            label,
        });
        if is_timeline {
            self.rebuild_timeline();
        }
        self.sequence += 1;
        id
    }

    /// Cancel the most recent pending change, or all of them.
    pub fn cancel(&mut self, all: bool) -> Vec<Pending> {
        let cancelled = if all {
            std::mem::take(&mut self.pending)
        } else {
            self.pending.pop().into_iter().collect()
        };
        if !cancelled.is_empty() {
            self.rebuild_timeline();
            self.sequence += 1;
        }
        cancelled
    }

    /// Cancel specific pending changes by id and rebuild the timeline.
    pub fn remove_pending(&mut self, ids: &[u64]) -> bool {
        let before = self.pending.len();
        self.pending.retain(|p| !ids.contains(&p.id));
        if self.pending.len() != before {
            self.rebuild_timeline();
            self.sequence += 1;
            true
        } else {
            false
        }
    }

    /// Cancel any pending timeline remaps without affecting other pending changes.
    pub fn cancel_pending_timeline(&mut self) -> bool {
        let before = self.pending.len();
        self.pending
            .retain(|p| !matches!(p.change, Change::Timeline { .. }));
        if self.pending.len() != before {
            self.rebuild_timeline();
            self.sequence += 1;
            true
        } else {
            false
        }
    }

    fn ordered(&self) -> Vec<&Pending> {
        let mut ordered: Vec<_> = self.pending.iter().collect();
        ordered.sort_by_key(|p| p.at);
        ordered
    }

    /// Land every change the transport has reached. Paused, a change lands
    /// immediately.
    pub fn land_due(&mut self) -> Vec<Landed> {
        let reached = |at: u64| at < self.next_step || !self.playing && at <= self.next_step;
        let mut due: Vec<Pending> = Vec::new();
        let mut kept = Vec::new();
        for pending in std::mem::take(&mut self.pending) {
            if reached(pending.at) {
                due.push(pending);
            } else {
                kept.push(pending);
            }
        }
        self.pending = kept;
        due.sort_by_key(|p| p.at);
        let mut landed = Vec::new();
        for pending in due {
            self.state.apply(pending.at, &pending.change);
            if let Change::Launch { lane, part } = pending.change {
                self.last_part.retain(|(l, _)| *l != lane);
                self.last_part.push((lane, part));
            }
            landed.push(Landed {
                at: pending.at,
                change: pending.change,
                label: pending.label,
            });
        }
        if !landed.is_empty() {
            self.rebuild_timeline();
            self.sequence += 1;
        }
        landed
    }

    /// Land everything now, as when a change cannot wait for the timeline.
    pub fn land_all(&mut self) -> Vec<Landed> {
        let now = self.next_step;
        for pending in &mut self.pending {
            pending.at = pending.at.min(now);
        }
        let playing = self.playing;
        self.playing = false;
        let landed = self.land_due();
        self.playing = playing;
        landed
    }

    /// Activity a lane will have once every pending change lands.
    pub fn expected(&self) -> Vec<Override> {
        let mut state = self.state.clone();
        for pending in self.ordered() {
            state.apply(pending.at, &pending.change);
        }
        state.active
    }

    pub fn last_part(&self, lane: Id) -> Option<LanePart> {
        self.last_part
            .iter()
            .find(|(l, _)| *l == lane)
            .map(|(_, p)| *p)
    }

    pub fn lane_state(&self, lane: Id) -> Option<&Override> {
        self.state.active.iter().find(|a| a.lane == lane)
    }

    pub fn pending_for_lane(&self, lane: Id) -> impl Iterator<Item = &Pending> + '_ {
        self.pending
            .iter()
            .filter(move |p| Self::lane_of(&p.change) == Some(lane))
    }

    /// Whether the compiled ring window no longer covers `next_step` with sufficient
    /// lookahead (either `next_step` moved into the second half of the ring or rewound
    /// before `self.base`).
    pub fn needs_extension(&self) -> bool {
        self.next_step < self.base || self.next_step >= self.base + LIVE_RING_STEPS / 2
    }

    /// Compile the ring window starting at the bar of the step last played.
    /// `statics` and `current_flags` supply instruments and the mixer, which are heard at once.
    pub fn compile(
        &mut self,
        statics: &Project,
        current_flags: &LiveFlags,
        audition_patch: Option<RackPatch>,
    ) -> Result<ScoreWindow> {
        let last = self.next_step.saturating_sub(1);
        self.base = last / STEPS_PER_BAR as u64 * STEPS_PER_BAR as u64;
        let mut owned: Vec<(u64, PerformanceState)> = vec![(0, self.state.clone())];
        for pending in self.ordered() {
            let mut next = owned.last().expect("first segment").1.clone();
            next.apply(pending.at, &pending.change);
            if owned.last().is_some_and(|(from, _)| *from == pending.at) {
                *owned.last_mut().expect("segment") = (pending.at, next);
            } else {
                owned.push((pending.at, next));
            }
        }
        let segments: Vec<LiveSegment<'_>> = owned
            .iter()
            .map(|(from, state)| LiveSegment {
                from: *from,
                song: &state.project,
                flags: &state.flags,
                active: &state.active,
                timeline: state.timeline,
                audition: self.audition,
            })
            .collect();
        statics.compile_live_with_audition(current_flags, self.base, &segments, audition_patch)
    }
}

/// A lane starts a part by value; the newest launch takes the part's instrument
/// from any other lane playing it.
pub(crate) fn launch_into(
    project: &Project,
    active: &mut Vec<Override>,
    lane: Id,
    part: LanePart,
    at: u64,
) {
    if project.lane(lane).is_none() || project.part(part.part).is_none() {
        return;
    }
    if let Some(instrument) = project.part_instrument(part.part) {
        active.retain(|a| project.part_instrument(a.part.part) != Some(instrument));
    }
    active.retain(|a| a.lane != lane);
    active.push(Override {
        lane,
        part,
        anchor: at,
    });
    active.sort_by_key(|a| a.lane);
}

/// `bar.beat.step`, one-based, for a transport step.
pub fn position_label(step: u64) -> String {
    format!(
        "{:03}.{}.{}",
        step / 16 + 1,
        step % 16 / 4 + 1,
        step % 4 + 1
    )
}

/// Beats (rounded up) until a step, from the step about to play.
pub fn beats_until(next_step: u64, at: u64) -> u64 {
    at.saturating_sub(next_step).div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_never_snaps_backward_on_exact_boundary_and_update_heard_preserves_pending() {
        let mut project = Project::demo();
        project.timing.late_tolerance_steps = 4;
        let mut perf = Performance::new(&project);
        perf.playing = true;

        // Exact beat boundary (step 4 with quantum=4, late_tolerance=4) must land on 4, NOT 0!
        perf.next_step = 4;
        assert_eq!(perf.boundary(&project, Boundary::Beat).unwrap(), 4);

        // One step past a beat boundary (step 5) snaps cleanly back to 4 within late tolerance.
        perf.next_step = 5;
        assert_eq!(perf.boundary(&project, Boundary::Beat).unwrap(), 4);

        // Exact bar boundary (step 16 with quantum=16, late_tolerance=4) stays on 16.
        perf.next_step = 16;
        assert_eq!(perf.boundary(&project, Boundary::Bar).unwrap(), 16);

        // update_heard_project updates heard project/flags while preserving pending live changes!
        perf.enqueue(
            32,
            Change::Launch {
                lane: project.lanes[0].id,
                part: LanePart::new(project.parts[0].id),
            },
            "launch".into(),
        );
        assert_eq!(perf.pending.len(), 1);
        let mut updated = project.clone();
        updated.bpm = 144.0;
        let mut flags = LiveFlags::default();
        flags.set_lane_muted(project.lanes[1].id, true);
        perf.update_heard_project(&updated, &flags);
        assert_eq!(perf.heard().bpm, 144.0);
        assert!(perf.heard_flags().lane_muted(project.lanes[1].id));
        assert_eq!(
            perf.pending.len(),
            1,
            "update_heard_project must preserve pending live changes"
        );
    }

    #[test]
    fn needs_extension_detects_both_forward_horizon_and_rewind_below_base() {
        let project = Project::demo();
        let flags = LiveFlags::default();
        let mut perf = Performance::new(&project);
        perf.next_step = LIVE_RING_STEPS / 2 + 16;
        let win_mid = perf.compile(&project, &flags, None).unwrap();
        let mid_window = win_mid.live_window.expect("live window");
        assert!(mid_window.start >= LIVE_RING_STEPS / 2);
        assert!(mid_window.contains(perf.next_step));
        assert!(!perf.needs_extension());

        // Advancing past half the ring triggers forward extension.
        perf.next_step = mid_window.start + LIVE_RING_STEPS / 2;
        assert!(perf.needs_extension());

        // Rewinding behind `base` also triggers recompile so the ring covers `next_step`.
        perf.next_step = 0;
        assert!(perf.needs_extension());
        let win_rewound = perf.compile(&project, &flags, None).unwrap();
        let rewound_window = win_rewound.live_window.expect("live window");
        assert_eq!(rewound_window.start, 0);
        assert!(rewound_window.contains(0));
        assert!(!perf.needs_extension());
    }
}
