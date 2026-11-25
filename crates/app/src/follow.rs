//! Session viewport following never changes the musical editing coordinates.
use super::{App, EditScope, KeyCode, KeyEvent, Workspace};
use cadence_core::follow::{arrange_page, FollowMode, FollowPage};
use cadence_core::material::{Edit, Id, Project, MAX_BARS};
use cadence_core::playback::{LiveProjection, PlaybackClock, PlaybackRequest, PlaybackScope};

impl App {
    pub fn follow_mode(&self) -> FollowMode {
        self.follow.mode()
    }

    pub fn live_projection(&self, observed_step: u64) -> LiveProjection<'_> {
        LiveProjection {
            transport_step: observed_step,
            segment: Some(self.live.timeline().segment_at(observed_step)),
            active: self.live.active(),
            audition: self.live.audition,
        }
    }

    fn playback_clock(&self, observed_step: u64) -> PlaybackClock {
        if let Some(aud) = self.live.audition.filter(|a| a.solo) {
            return PlaybackClock::Audition {
                pattern: aud.pattern,
                step: observed_step.saturating_sub(aud.anchor) as f64,
                length: aud.length,
            };
        }
        match self.song_position_opt(observed_step) {
            Some(song_step) => PlaybackClock::Song {
                step: song_step as f64,
            },
            None => PlaybackClock::PastEnd,
        }
    }

    /// The frontend supplies the engine's observed clock, including while paused.
    pub fn playback_request(&self, observed_step: u64) -> Option<PlaybackRequest> {
        self.playback_request_of(self.selected_pattern, self.scope, observed_step)
    }

    /// Where playback is in one Pattern deck's pattern and scope.
    pub fn playback_request_of(
        &self,
        pattern: Option<Id>,
        scope: EditScope,
        observed_step: u64,
    ) -> Option<PlaybackRequest> {
        Some(PlaybackRequest {
            pattern: pattern?,
            scope: match scope {
                EditScope::Definition => PlaybackScope::Definition,
                EditScope::Placement(id) => PlaybackScope::Placement(id),
            },
            preferred_placement: self.selected_placement,
            clock: self.playback_clock(observed_step),
        })
    }

    pub fn pattern_view_page(&self, observed_step: u64) -> Option<FollowPage> {
        self.pattern_page_of(
            self.selected_pattern,
            self.scope,
            self.cursor,
            observed_step,
        )
    }

    /// The page one Pattern deck shows, following playback when it may.
    pub fn pattern_page_of(
        &self,
        pattern: Option<Id>,
        scope: EditScope,
        cursor: usize,
        observed_step: u64,
    ) -> Option<FollowPage> {
        let length = self.length_of(pattern, scope).max(1);
        let page_steps = self.pattern_visible_steps(length);
        self.follow
            .pattern_page_live_sized(
                self.doc.project(),
                self.doc.flags(),
                self.live_projection(observed_step),
                self.playback_request_of(pattern, scope, observed_step)?,
                cursor,
                page_steps,
            )
            .ok()
    }

    pub fn arrange_view_page(&self, observed_step: u64, visible_bars: usize) -> FollowPage {
        let prev = self.arrange_ui.viewport_start_step.get();
        let page = arrange_page(
            self.follow.mode(),
            self.playback_clock(observed_step),
            self.arrange_ui.step as usize,
            Some(prev),
            visible_bars.clamp(1, MAX_BARS),
            self.doc.project().bars,
        )
        .expect("validated project and observed integer transport");
        self.arrange_ui.viewport_start_step.set(page.start);
        page
    }

    pub(super) fn describe_follow(&mut self) {
        self.status = match self.follow.mode() {
            FollowMode::Off => "Follow off · viewport uses editing position",
            FollowMode::Active => {
                "Follow on · viewport tracks playhead · move hands off here · tf toggles"
            }
            FollowMode::Suspended => "Follow suspended at playhead · tf resumes",
        }
        .into();
    }

    /// Seed the active workspace's editing cursor from the current playhead position
    /// before suspending Follow via manual navigation, so the viewport hands off
    /// seamlessly at the playhead instead of snapping back to an old parked cursor.
    pub(super) fn seed_cursor_from_playhead(&mut self) {
        if !self.follow.active() || self.is_jam() {
            return;
        }
        match self.focused_canvas() {
            Some(Workspace::Arrange) => {
                if let PlaybackClock::Song { step } = self.playback_clock(self.transport.step) {
                    let horizon = self.doc.project().arrangement_steps().max(1);
                    let heard = step.rem_euclid(horizon as f64).floor() as u32;
                    let snapped = if self.arrange_ui.view == crate::ArrangeView::Placements {
                        (heard / 16) * 16
                    } else {
                        heard
                    };
                    self.arrange_ui.step = snapped.min(horizon.saturating_sub(1) as u32);
                    self.select_at_cursor();
                }
            }
            Some(Workspace::Pattern) => {
                if let Some(step) = self
                    .pattern_view_page(self.transport.step)
                    .and_then(|page| page.anchor)
                {
                    let max_step = self.current_length().saturating_sub(1);
                    self.cursor = step.min(max_step);
                }
            }
            Some(Workspace::Mixer) | None => {}
        }
    }

    /// Editing and moving suspend Follow, except in Jam: there the keyboard is
    /// an instrument, so the view keeps following the performance, even through
    /// an edit typed on the command line.
    pub(super) fn suspend_follow(&mut self) {
        if self.follow.active() && !self.is_jam() {
            self.follow.suspend();
            self.describe_follow();
        }
    }

    pub(super) fn suspend_follow_for_key(&mut self, key: KeyEvent) {
        let Some(ws) = self.focused_canvas() else {
            return;
        };
        let alt = key.modifiers.contains(crate::KeyModifiers::ALT);
        let relative_nav = !alt
            && (matches!(
                key.code,
                KeyCode::Char('h' | 'j' | 'k' | 'l' | 'H' | 'L')
                    | KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::PageUp
                    | KeyCode::PageDown
            ) || (matches!(key.code, KeyCode::Char('w' | 'b'))
                && (ws == Workspace::Pattern || self.evaluated_arrange_canvas())));
        let absolute_nav =
            !alt && matches!(key.code, KeyCode::Char('0' | '$' | 'G') | KeyCode::End);
        let alt_edit = alt
            && matches!(
                key.code,
                KeyCode::Char('h' | 'j' | 'k' | 'l' | 'H' | 'J' | 'K' | 'L')
                    | KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::Backspace
            );
        let editing = match ws {
            Workspace::Pattern => {
                alt_edit
                    || matches!(
                        key.code,
                        KeyCode::Char(
                            ',' | '.'
                                | '<'
                                | '>'
                                | 'x'
                                | 'X'
                                | 'i'
                                | 'o'
                                | 'm'
                                | 'c'
                                | 'C'
                                | '['
                                | ']'
                                | '#'
                                | ';'
                        )
                    )
                    || (key.code == KeyCode::Backspace
                        && matches!(self.scope, EditScope::Placement(_)))
                    || (self.value_canvas() && key.code == KeyCode::Enter)
            }
            Workspace::Arrange => {
                alt_edit || matches!(key.code, KeyCode::Char(',' | '.' | '<' | '>'))
            }
            Workspace::Mixer => false,
        };
        if ws != Workspace::Mixer && (relative_nav || absolute_nav || editing) {
            if relative_nav {
                self.seed_cursor_from_playhead();
            }
            self.suspend_follow();
        }
    }
}

/// Sound and mixer parameter edits remain playable while following. Material,
/// timing, and source association edits return the viewport to its editing page.
pub(super) fn material_changed(before: &Project, after: &Project) -> bool {
    before.patterns != after.patterns
        || before.bars != after.bars
        || before.root_midi != after.root_midi
        || before.scale != after.scale
        || before.lanes.len() != after.lanes.len()
        || before
            .lanes
            .iter()
            .zip(&after.lanes)
            .any(|(a, b)| a.id != b.id || a.placements != b.placements)
        || before.parts.len() != after.parts.len()
        || before
            .parts
            .iter()
            .zip(&after.parts)
            .any(|(a, b)| a.id != b.id || a.pattern != b.pattern)
}

pub(super) fn points_edited(edits: &[Edit]) -> bool {
    edits.iter().any(|edit| {
        matches!(
            edit,
            Edit::SetNote { .. }
                | Edit::SetRhythm { .. }
                | Edit::SetValue { .. }
                | Edit::SetException { .. }
                | Edit::ResizePattern { .. }
        )
    })
}
