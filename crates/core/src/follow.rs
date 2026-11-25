//! Transport-follow viewport projection, separate from editing coordinates.
//!
//! A frontend may observe clocks to retain the last active Song use through a gap.
//! The immutable query also works for a one-shot/headless snapshot with no history.
use crate::material::{Id, Project, MAX_BARS, MAX_PATTERNS, MAX_PLACEMENTS, STEPS_PER_BAR};
use crate::mixer::LiveFlags;
use crate::playback::{
    pattern_playback, pattern_playback_live, LiveProjection, PatternPlayback, PlaybackClock,
    PlaybackRequest, PlaybackScope,
};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

pub const PATTERN_PAGE_STEPS: usize = 16;
const MAX_ANCHORS: usize = MAX_PATTERNS + MAX_PLACEMENTS;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FollowMode {
    #[default]
    Off,
    Active,
    Suspended,
}
impl FollowMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Follow off",
            Self::Active => "Follow on",
            Self::Suspended => "Follow suspended",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PageOrigin {
    Editing,
    Transport,
    /// Last observed Song-use clock; no currently active use supplies a marker.
    Held,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct FollowPage {
    /// First visible step, in the queried song/source/local coordinate domain.
    pub start: usize,
    pub count: usize,
    /// Follow anchor, separate from the caller's unchanged editing cursor.
    pub anchor: Option<usize>,
    pub origin: PageOrigin,
}
impl FollowPage {
    pub fn following(self) -> bool {
        self.origin != PageOrigin::Editing
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct AnchorKey {
    pattern: Id,
    placement: Option<Id>,
}
impl From<PlaybackRequest> for AnchorKey {
    fn from(request: PlaybackRequest) -> Self {
        Self {
            pattern: request.pattern,
            placement: match request.scope {
                PlaybackScope::Definition => None,
                PlaybackScope::Placement(id) => Some(id),
            },
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct Anchor {
    source_time: f64,
    step: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FollowState {
    mode: FollowMode,
    anchors: HashMap<AnchorKey, Anchor>,
    order: VecDeque<AnchorKey>,
}
impl FollowState {
    pub fn mode(&self) -> FollowMode {
        self.mode
    }
    pub fn active(&self) -> bool {
        self.mode == FollowMode::Active
    }
    pub fn set_enabled(&mut self, enabled: bool) -> FollowMode {
        self.mode = if enabled {
            FollowMode::Active
        } else {
            FollowMode::Off
        };
        self.mode
    }
    /// Off and Suspended both enable/resume; Active disables.
    pub fn toggle(&mut self) -> FollowMode {
        self.set_enabled(self.mode != FollowMode::Active)
    }
    /// Manual navigation/editing returns to the editor's existing page.
    pub fn suspend(&mut self) -> FollowMode {
        if self.mode == FollowMode::Active {
            self.mode = FollowMode::Suspended;
        }
        self.mode
    }
    /// Call at document boundaries; session preference may be retained separately.
    pub fn clear_anchors(&mut self) {
        self.anchors.clear();
        self.order.clear();
    }
    pub fn reset(&mut self) {
        self.mode = FollowMode::Off;
        self.clear_anchors();
    }

    /// Observe a current projection and keep its preferred active Song clock.
    /// Solo audition clocks never overwrite Song anchors. Calling this while Off
    /// is harmless: observation is independent of whether the viewport follows.
    pub fn observe(
        &mut self,
        project: &Project,
        flags: &LiveFlags,
        request: PlaybackRequest,
    ) -> Result<PatternPlayback> {
        let projection = pattern_playback(project, flags, request)?;
        self.observe_projection(project, request, &projection);
        Ok(projection)
    }
    pub fn observe_live(
        &mut self,
        project: &Project,
        flags: &LiveFlags,
        live: LiveProjection<'_>,
        request: PlaybackRequest,
    ) -> Result<PatternPlayback> {
        let projection = pattern_playback_live(project, flags, live, request)?;
        self.observe_projection(project, request, &projection);
        Ok(projection)
    }
    /// Reuse a projection the frontend already needed for its playhead drawing.
    pub fn observe_projection(
        &mut self,
        project: &Project,
        request: PlaybackRequest,
        projection: &PatternPlayback,
    ) {
        self.prune(project);
        if matches!(request.clock, PlaybackClock::Audition { .. }) {
            return;
        }
        let Some(point) = projection.markers.first() else {
            return;
        };
        if !point.source_time.is_finite() || point.step >= projection.period {
            return;
        }
        let key = AnchorKey::from(request);
        self.anchors.insert(
            key,
            Anchor {
                source_time: point.source_time,
                step: point.step,
            },
        );
        self.order.retain(|old| *old != key);
        self.order.push_back(key);
        while self.order.len() > MAX_ANCHORS {
            if let Some(old) = self.order.pop_front() {
                self.anchors.remove(&old);
            }
        }
    }
    fn prune(&mut self, project: &Project) {
        self.anchors.retain(|key, _| {
            project.pattern(key.pattern).is_some()
                && key.placement.is_none_or(|id| {
                    project.placement(id).is_some_and(|(_, p)| {
                        project
                            .part(p.part)
                            .is_some_and(|b| b.pattern == key.pattern)
                    })
                })
        });
        self.order.retain(|key| self.anchors.contains_key(key));
    }
    /// Current transport takes precedence over cached anchors, so a headless
    /// snapshot has the same page without an earlier observe() call.
    pub fn pattern_page(
        &self,
        project: &Project,
        flags: &LiveFlags,
        request: PlaybackRequest,
        edit_cursor: usize,
    ) -> Result<FollowPage> {
        self.pattern_page_sized(project, flags, request, edit_cursor, PATTERN_PAGE_STEPS)
    }
    pub fn pattern_page_live(
        &self,
        project: &Project,
        flags: &LiveFlags,
        live: LiveProjection<'_>,
        request: PlaybackRequest,
        edit_cursor: usize,
    ) -> Result<FollowPage> {
        self.pattern_page_live_sized(
            project,
            flags,
            live,
            request,
            edit_cursor,
            PATTERN_PAGE_STEPS,
        )
    }
    pub fn pattern_page_live_sized(
        &self,
        project: &Project,
        flags: &LiveFlags,
        live: LiveProjection<'_>,
        request: PlaybackRequest,
        edit_cursor: usize,
        page_steps: usize,
    ) -> Result<FollowPage> {
        let projection = pattern_playback_live(project, flags, live, request)?;
        self.pattern_page_from_projection(request, &projection, edit_cursor, page_steps)
    }
    pub fn pattern_page_sized(
        &self,
        project: &Project,
        flags: &LiveFlags,
        request: PlaybackRequest,
        edit_cursor: usize,
        page_steps: usize,
    ) -> Result<FollowPage> {
        let projection = pattern_playback(project, flags, request)?;
        self.pattern_page_from_projection(request, &projection, edit_cursor, page_steps)
    }
    pub fn pattern_page_from_projection(
        &self,
        request: PlaybackRequest,
        projection: &PatternPlayback,
        edit_cursor: usize,
        page_steps: usize,
    ) -> Result<FollowPage> {
        validate_clock(request.clock)?;
        ensure!(
            projection.period > 0,
            "Pattern viewport needs a positive period"
        );
        ensure!(
            page_steps > 0,
            "Pattern viewport page size must be positive"
        );
        if !self.active() {
            return Ok(page(
                edit_cursor,
                projection.period,
                page_steps,
                PageOrigin::Editing,
            ));
        }
        if let Some(point) = projection.markers.first() {
            ensure!(
                point.step < projection.period,
                "Playback marker is outside its pattern"
            );
            return Ok(page(
                point.step,
                projection.period,
                page_steps,
                PageOrigin::Transport,
            ));
        }
        // Solo audition has no temporal placement gaps. An absent audition marker
        // means this view is outside its clock domain; do not borrow Song state.
        if !matches!(request.clock, PlaybackClock::Audition { .. }) {
            if let Some(anchor) = self.anchors.get(&AnchorKey::from(request)) {
                let step = match request.scope {
                    PlaybackScope::Definition => (anchor.source_time.floor() as i128)
                        .rem_euclid(projection.period as i128)
                        as usize,
                    PlaybackScope::Placement(_) => anchor.step.min(projection.period - 1),
                };
                return Ok(page(step, projection.period, page_steps, PageOrigin::Held));
            }
        }
        Ok(page(
            edit_cursor,
            projection.period,
            page_steps,
            PageOrigin::Editing,
        ))
    }
}

/// Bar-aligned horizontal scrolloff margin for an Arrange viewport of `visible_bars`.
///
/// Reserves ~20–25% lookahead on both leading and trailing edges while keeping an interior
/// dead-zone so small back-and-forth cursor adjustments do not jitter the viewport.
pub fn arrange_scrolloff_bars(visible_bars: usize) -> usize {
    if visible_bars < 3 {
        0
    } else {
        (visible_bars / 4).clamp(1, (visible_bars - 1) / 2)
    }
}

/// Arrange pages use song-step units (bar-aligned multiples of `STEPS_PER_BAR = 16`).
///
/// When `prev_start_step` is supplied, slides smoothly bar-by-bar once the active position
/// enters the leading/trailing scrolloff margin (`arrange_scrolloff_bars`), and jumps cleanly
/// with leading lookahead context on discontinuous leaps (section jumps, loop wraps, or
/// Follow activation far from the current viewport). Partial final pages are clamped back so
/// zoomed-in viewports remain filled to `song_bars`. Isolated preview always leaves the
/// Arrange viewport on its editing position.
pub fn arrange_page(
    mode: FollowMode,
    clock: PlaybackClock,
    edit_step: usize,
    prev_start_step: Option<usize>,
    visible_bars: usize,
    song_bars: usize,
) -> Result<FollowPage> {
    ensure!((1..=MAX_BARS).contains(&song_bars), "Invalid song horizon");
    ensure!(
        (1..=MAX_BARS).contains(&visible_bars),
        "Invalid Arrange page size"
    );
    validate_clock(clock)?;
    let horizon = song_bars * STEPS_PER_BAR;
    let (position, origin) = if mode == FollowMode::Active {
        if let PlaybackClock::Song { step } = clock {
            (
                step.rem_euclid(horizon as f64).floor() as usize,
                PageOrigin::Transport,
            )
        } else {
            (edit_step, PageOrigin::Editing)
        }
    } else {
        (edit_step, PageOrigin::Editing)
    };
    Ok(scrolled_arrange_page(
        position,
        prev_start_step,
        visible_bars,
        song_bars,
        origin,
    ))
}
fn validate_clock(clock: PlaybackClock) -> Result<()> {
    let step = match clock {
        PlaybackClock::Song { step } => step,
        PlaybackClock::Audition { step, length, .. } => {
            ensure!(
                length > 0 && length <= MAX_BARS * STEPS_PER_BAR,
                "Invalid audition length"
            );
            step
        }
        PlaybackClock::PastEnd => return Ok(()),
    };
    ensure!(
        step.is_finite() && step >= 0.0,
        "Invalid observed transport position"
    );
    Ok(())
}
fn page(position: usize, period: usize, page_steps: usize, origin: PageOrigin) -> FollowPage {
    let position = position.min(period - 1);
    let start = position / page_steps * page_steps;
    FollowPage {
        start,
        count: period.saturating_sub(start).min(page_steps),
        anchor: if origin == PageOrigin::Editing {
            None
        } else {
            Some(position)
        },
        origin,
    }
}
fn scrolled_arrange_page(
    position: usize,
    prev_start_step: Option<usize>,
    visible_bars: usize,
    song_bars: usize,
    origin: PageOrigin,
) -> FollowPage {
    let horizon = song_bars * STEPS_PER_BAR;
    let position = position.min(horizon - 1);
    let vis = visible_bars.min(song_bars).max(1);
    let page_steps = vis * STEPS_PER_BAR;
    let max_start_bar = song_bars.saturating_sub(vis);
    let scrolloff = arrange_scrolloff_bars(vis);
    let bar = position / STEPS_PER_BAR;

    let jump_anchor = |target_bar: usize| -> usize {
        if target_bar < vis.saturating_sub(scrolloff) {
            0
        } else {
            target_bar.saturating_sub(scrolloff).min(max_start_bar)
        }
    };

    let start_bar = if max_start_bar == 0 {
        0
    } else if let Some(prev_step) = prev_start_step {
        let prev_bar = (prev_step / STEPS_PER_BAR).min(max_start_bar);
        let cont_lo = if scrolloff == 0 {
            prev_bar.saturating_sub(1)
        } else {
            prev_bar
        };
        let cont_hi = if scrolloff == 0 {
            prev_bar.saturating_add(vis)
        } else {
            prev_bar + vis - 1
        };
        if bar >= cont_lo && bar <= cont_hi {
            let right_limit = prev_bar + vis - 1 - scrolloff;
            let left_limit = prev_bar + scrolloff;
            if bar > right_limit {
                bar.saturating_sub(vis - 1 - scrolloff).min(max_start_bar)
            } else if bar < left_limit {
                bar.saturating_sub(scrolloff)
            } else {
                prev_bar
            }
        } else {
            jump_anchor(bar)
        }
    } else {
        jump_anchor(bar)
    };

    let start = start_bar * STEPS_PER_BAR;
    FollowPage {
        start,
        count: horizon.saturating_sub(start).min(page_steps),
        anchor: if origin == PageOrigin::Editing {
            None
        } else {
            Some(position)
        },
        origin,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrange_page_clamps_partial_tail_pages_to_fill_viewport() {
        // 12-bar song at 10-bar zoom: bars 1..=8 stay at start 0; bars 11..=12 clamp back to bar 3 (step 32).
        let early = arrange_page(
            FollowMode::Off,
            PlaybackClock::Song { step: 0.0 },
            64,
            None,
            10,
            12,
        )
        .unwrap();
        assert_eq!((early.start, early.count), (0, 160));

        let tail_edit = arrange_page(
            FollowMode::Off,
            PlaybackClock::Song { step: 0.0 },
            160, // bar 11
            None,
            10,
            12,
        )
        .unwrap();
        assert_eq!(
            (tail_edit.start, tail_edit.count, tail_edit.origin),
            (32, 160, PageOrigin::Editing)
        );

        let tail_follow = arrange_page(
            FollowMode::Active,
            PlaybackClock::Song { step: 172.0 }, // bar 11 beat 4
            0,
            None,
            10,
            12,
        )
        .unwrap();
        assert_eq!(
            (
                tail_follow.start,
                tail_follow.count,
                tail_follow.anchor,
                tail_follow.origin
            ),
            (32, 160, Some(172), PageOrigin::Transport)
        );

        // 16-bar song at 6-bar zoom: bar 15 clamps to bars 11..=16 (step 160, count 96).
        let odd_divisor = arrange_page(
            FollowMode::Off,
            PlaybackClock::Song { step: 0.0 },
            224, // bar 15
            None,
            6,
            16,
        )
        .unwrap();
        assert_eq!((odd_divisor.start, odd_divisor.count), (160, 96));

        // Viewport wider than song horizon anchors cleanly at 0 with full song count.
        let wide = arrange_page(
            FollowMode::Off,
            PlaybackClock::Song { step: 0.0 },
            48,
            None,
            16,
            4,
        )
        .unwrap();
        assert_eq!((wide.start, wide.count), (0, 64));
    }

    #[test]
    fn arrange_page_smooth_scrolloff_slides_with_lookahead_deadzone_and_jumps() {
        // Scrolloff table: ~20-25% margin when visible_bars >= 3, 0 for 1-2 bars.
        assert_eq!(arrange_scrolloff_bars(1), 0);
        assert_eq!(arrange_scrolloff_bars(2), 0);
        assert_eq!(arrange_scrolloff_bars(4), 1);
        assert_eq!(arrange_scrolloff_bars(8), 2);
        assert_eq!(arrange_scrolloff_bars(16), 4);

        // 64-bar song, 16-bar viewport (scrolloff = 4 bars).
        // Walking forward bar-by-bar stays at 0 through bar 12 (idx 11), then slides +1 bar per bar
        // keeping 4 bars of trailing lookahead until clamping at bar 49 (start_bar = 48).
        let mut start = 0usize;
        for bar in 0..64usize {
            let p = arrange_page(
                FollowMode::Off,
                PlaybackClock::Song { step: 0.0 },
                bar * 16,
                Some(start),
                16,
                64,
            )
            .unwrap();
            let expected_start_bar = bar.saturating_sub(11).min(48);
            assert_eq!(
                p.start,
                expected_start_bar * 16,
                "forward walk at bar {}",
                bar + 1
            );
            start = p.start;
        }

        // Walking backward from bar 64 stays steady in dead-zone until bar 52 (idx 51 < 48 + 4),
        // then glides smoothly -1 bar per bar keeping 4 bars of leading lookahead.
        for bar in (0..64usize).rev() {
            let p = arrange_page(
                FollowMode::Off,
                PlaybackClock::Song { step: 0.0 },
                bar * 16,
                Some(start),
                16,
                64,
            )
            .unwrap();
            let expected_start_bar = if bar >= 52 { 48 } else { bar.saturating_sub(4) };
            assert_eq!(
                p.start,
                expected_start_bar * 16,
                "backward walk at bar {}",
                bar + 1
            );
            start = p.start;
        }

        // Discontinuous section jump from bar 1 (start=0) to bar 25 (idx 24) lands with 4-bar lead-in
        // (start_bar = 20 -> bars 21..=36), showing 4 lead-in bars + full 8-bar section + 4 ahead.
        let jumped = arrange_page(
            FollowMode::Off,
            PlaybackClock::Song { step: 0.0 },
            24 * 16,
            Some(0),
            16,
            64,
        )
        .unwrap();
        assert_eq!(jumped.start, 20 * 16);

        // FollowMode::Active glides identically bar-by-bar and snaps back to 0 on loop wrap.
        let mut follow_start = 0usize;
        for bar in 0..20usize {
            let p = arrange_page(
                FollowMode::Active,
                PlaybackClock::Song {
                    step: (bar * 16 + 7) as f64,
                },
                0,
                Some(follow_start),
                8, // scrolloff = 2 -> right_limit = start + 5
                64,
            )
            .unwrap();
            assert_eq!(p.start, bar.saturating_sub(5) * 16);
            assert_eq!(p.origin, PageOrigin::Transport);
            follow_start = p.start;
        }
        let wrapped = arrange_page(
            FollowMode::Active,
            PlaybackClock::Song { step: 2.0 },
            0,
            Some(follow_start),
            8,
            64,
        )
        .unwrap();
        assert_eq!(wrapped.start, 0);

        // Narrow 2-bar viewport (scrolloff = 0) still glides +1 / -1 bar cleanly at boundaries.
        let p2_fwd = arrange_page(
            FollowMode::Off,
            PlaybackClock::Song { step: 0.0 },
            2 * 16, // bar 3, 1 bar past [0..2)
            Some(0),
            2,
            16,
        )
        .unwrap();
        assert_eq!(p2_fwd.start, 16);
        let p2_back = arrange_page(
            FollowMode::Off,
            PlaybackClock::Song { step: 0.0 },
            0, // bar 1, 1 bar before [1..3)
            Some(16),
            2,
            16,
        )
        .unwrap();
        assert_eq!(p2_back.start, 0);
    }

    #[test]
    fn pattern_page_live_sized_tracks_transport_and_held_anchors_at_custom_zoom_spans() {
        let project = Project::demo();
        let flags = LiveFlags::default();
        let mut follow = FollowState::default();
        follow.set_enabled(true);

        let req_active = PlaybackRequest {
            pattern: 4, // Moving lead (period 320), active in bars 5..=16 (steps 64..256)
            scope: PlaybackScope::Definition,
            preferred_placement: Some(2),
            clock: PlaybackClock::Song { step: 83.0 },
        };
        follow
            .observe_live(&project, &flags, LiveProjection::default(), req_active)
            .unwrap();

        // Zoomed in to 8 steps: step 83 lies on page [80..88).
        let zoomed_in = follow
            .pattern_page_live_sized(
                &project,
                &flags,
                LiveProjection::default(),
                req_active,
                2,
                8,
            )
            .unwrap();
        assert_eq!(
            (
                zoomed_in.start,
                zoomed_in.count,
                zoomed_in.anchor,
                zoomed_in.origin
            ),
            (80, 8, Some(83), PageOrigin::Transport)
        );

        // Zoomed out to 32 steps: step 83 lies on page [64..96).
        let zoomed_out = follow
            .pattern_page_live_sized(
                &project,
                &flags,
                LiveProjection::default(),
                req_active,
                2,
                32,
            )
            .unwrap();
        assert_eq!(
            (
                zoomed_out.start,
                zoomed_out.count,
                zoomed_out.anchor,
                zoomed_out.origin
            ),
            (64, 32, Some(83), PageOrigin::Transport)
        );

        // During gap (step 20): held anchor (83) maps cleanly into the custom 8-step page [80..88).
        let req_gap = PlaybackRequest {
            clock: PlaybackClock::Song { step: 20.0 },
            ..req_active
        };
        let held = follow
            .pattern_page_live_sized(&project, &flags, LiveProjection::default(), req_gap, 2, 8)
            .unwrap();
        assert_eq!(
            (held.start, held.count, held.anchor, held.origin),
            (80, 8, Some(83), PageOrigin::Held)
        );
    }
}
