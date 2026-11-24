//! One transport owner (`Transport`) plus song loop & end-rule controls.
//!
//! `\` switches between playing the arrangement and looping a region of it;
//! `[` and `]` in Arrange set the region at the cursor; `:loop` takes the exact
//! same path. The song's end rule decides what song playback does at the end.
use super::*;
use cadence_core::live::Change;
use cadence_core::music::EndRule;
use cadence_core::timeline::Segment;

/// Single source of truth for transport playback state, observed ring cursor,
/// loop brackets, and parked position during solo audition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Transport {
    pub playing: bool,
    /// Last observed transport step on the unbounded ring clock.
    pub step: u64,
    /// Next step the engine will trigger on the unbounded ring clock.
    pub next_step: u64,
    /// Remembered loop brackets `(start_step, end_step)` in song steps.
    pub loop_region: Option<(u32, u32)>,
    /// Saved song state while solo-auditioning from stopped transport:
    /// `(step, next_step, segment)`.
    pub parked: Option<(u64, u64, Segment)>,
}

impl App {
    pub fn playing(&self) -> bool {
        self.transport.playing
    }

    pub fn transport_step(&self) -> u64 {
        self.transport.step
    }

    pub fn transport_next(&self) -> u64 {
        self.transport.next_step
    }

    /// The remembered loop brackets, as a half-open range of song steps.
    pub fn loop_region(&self) -> Option<(u32, u32)> {
        self.transport.loop_region
    }

    pub fn end_rule(&self) -> EndRule {
        self.doc.project().end
    }

    /// The region the timeline is looping right now, if any.
    pub fn playing_loop(&self) -> Option<(u32, u32)> {
        let (start, len) = self.live.heard_segment().region?;
        Some((start as u32, (start + len) as u32))
    }

    /// `[` and `]`: the loop region starts at, or ends after, the cursor's bar.
    pub(super) fn set_loop_edge(&mut self, end: bool) -> Result<Outcome> {
        let bar = self.arrange_ui.step / 16 * 16;
        let (start, stop) = self.transport.loop_region.unwrap_or((bar, bar + 16 * 4));
        let region = if end {
            (start.min(bar), bar + 16)
        } else {
            (bar, stop.max(bar + 16))
        };
        let total = self.doc.project().arrangement_steps() as u32;
        let region = (
            region.0.min(total.saturating_sub(1)),
            region.1.min(total).max(region.0 + 1),
        );
        self.transport.loop_region = Some(region);
        self.status = format!(
            "Loop region bars {}–{} · \\ loops it",
            region.0 / 16 + 1,
            region.1.div_ceil(16)
        );
        if self.playing_loop().is_some() {
            return self.activate_loop(region);
        }
        Ok(Outcome::default())
    }

    /// Exit any active loop and play the song linearly from the current position.
    pub(super) fn disable_loop(&mut self) -> Result<Outcome> {
        self.live.cancel_pending_timeline();
        let now = self.transport.next_step;
        let song = self.song_position(now);
        let segment = Segment::new(now, song, None, self.end_rule());
        self.live.set_segment(segment);
        if let Some(take) = &mut self.takes.active {
            take.record_segment(now, segment, "song");
        }
        self.refresh_audio()?;
        self.status = "Playing the song".into();
        Ok(Outcome {
            changed: true,
            ..Outcome::default()
        })
    }

    /// Clear remembered `[` / `]` loop brackets and return to linear song playback.
    pub(super) fn clear_loop_region(&mut self) -> Result<Outcome> {
        self.transport.loop_region = None;
        let out = if self.playing_loop().is_some() {
            self.disable_loop()?
        } else {
            Outcome::default()
        };
        self.status = "Cleared loop points".into();
        Ok(out)
    }

    /// Loop `region` `(start_step, end_step)` on the timeline.
    ///
    /// - Paused: remaps immediately at `now` (keeping position if inside `region`,
    ///   otherwise snapping to `region.0`).
    /// - Playing inside `region`: remaps immediately at `now` with zero discontinuity.
    /// - Playing outside `region`: schedules `Change::Timeline` on the launch boundary
    ///   entering at `region.0`.
    pub(super) fn activate_loop(&mut self, region: (u32, u32)) -> Result<Outcome> {
        let total = self.doc.project().arrangement_steps() as u32;
        ensure!(
            region.0 < total,
            "The loop region starts past the song's end"
        );
        let region = (region.0, region.1.min(total).max(region.0 + 1));
        self.transport.loop_region = Some(region);
        let start = u64::from(region.0);
        let end = u64::from(region.1);
        let len = end - start;
        let now = self.transport.next_step;
        let cur_song = self.song_position_opt(now);
        let label = format!(
            "Looping bars {}–{}",
            region.0 / 16 + 1,
            region.1.div_ceil(16)
        );

        if !self.transport.playing {
            self.live.cancel_pending_timeline();
            let song = cur_song
                .filter(|&s| (start..end).contains(&s))
                .unwrap_or(start);
            let segment = Segment::new(now, song, Some((start, len)), self.end_rule());
            self.live.set_segment(segment);
            if let Some(take) = &mut self.takes.active {
                take.record_segment(now, segment, "loop");
            }
            self.refresh_audio()?;
            self.status = label;
            return Ok(Outcome {
                changed: true,
                ..Outcome::default()
            });
        }

        if let Some(song) = cur_song.filter(|&s| (start..end).contains(&s)) {
            self.live.cancel_pending_timeline();
            let segment = Segment::new(now, song, Some((start, len)), self.end_rule());
            self.live.set_segment(segment);
            if let Some(take) = &mut self.takes.active {
                take.record_segment(now, segment, "loop");
            }
            self.refresh_audio()?;
            self.status = label;
            return Ok(Outcome {
                changed: true,
                ..Outcome::default()
            });
        }

        let at = self
            .live
            .boundary(self.doc.project(), self.doc.project().timing.launch)?;
        let segment = Segment::new(at, start, Some((start, len)), self.end_rule());
        self.live
            .enqueue(at, Change::Timeline { segment }, label.clone());
        let landed = self.live.land_due();
        self.record_landed(landed);
        self.refresh_audio()?;
        self.status = self.describe_landing(&label);
        Ok(Outcome {
            changed: true,
            ..Outcome::default()
        })
    }

    /// `\`: loop the region, or else the section under the Arrange cursor, or
    /// else its bar; from a loop, return to the song.
    pub(super) fn toggle_song_loop(&mut self) -> Result<Outcome> {
        if self.playing_loop().is_some()
            || self
                .live
                .pending_timeline()
                .is_some_and(|(_, seg)| seg.region.is_some())
        {
            return self.disable_loop();
        }
        let cursor = self.arrange_ui.step;
        let region = self.transport.loop_region.unwrap_or_else(|| {
            self.doc
                .project()
                .sections
                .iter()
                .find(|s| (s.start_step..s.start_step + s.length_steps).contains(&cursor))
                .map_or((cursor / 16 * 16, cursor / 16 * 16 + 16), |s| {
                    (s.start_step, s.start_step + s.length_steps)
                })
        });
        self.activate_loop(region)
    }

    pub(super) fn set_end_rule(&mut self, end: EndRule) -> Result<Outcome> {
        self.apply(Action::Edit {
            edit: Edit::SetEndRule { end },
        })?;
        self.status = format!(
            "At the end the song will {}",
            match end {
                EndRule::Loop => "loop to the start",
                EndRule::Stop => "stop",
                EndRule::Continue => "keep playing",
            }
        );
        Ok(Outcome {
            changed: true,
            ..Outcome::default()
        })
    }
}
