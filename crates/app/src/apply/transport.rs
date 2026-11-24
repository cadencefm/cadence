//! Transport playback, stop, loop-restart, and bar-cueing action handlers.
use crate::*;

impl App {
    pub(super) fn apply_play(&mut self, playing: bool) -> Result<Outcome> {
        let mut out = Outcome::default();
        if playing && !self.is_solo_auditioning() {
            let song_len = self.doc.project().arrangement_steps() as u64;
            let now = self.transport.next_step;
            if self
                .live
                .timeline()
                .stop_at(now, song_len)
                .is_some_and(|stop| now >= stop)
            {
                let seg = cadence_core::timeline::Segment::new(now, 0, None, self.end_rule());
                self.live.set_segment(seg);
                if let Some(take) = &mut self.takes.active {
                    take.record_segment(now, seg, "restart");
                }
                self.refresh_audio()?;
                out.changed = true;
                out.transport.push(TransportIntent::Seek { step: now });
            }
            self.ensure_take_open()?;
        }
        if !playing {
            self.handle_recording_on_transport_stop(&mut out);
        }
        self.transport.playing = playing;
        self.live.playing = playing;
        out.transport.push(TransportIntent::Play { playing });
        Ok(out)
    }

    pub(super) fn apply_stop(&mut self) -> Result<Outcome> {
        self.input.count = None;
        self.stop_pending_browser_audition();
        if matches!(self.input.mode, Mode::Go | Mode::Transport | Mode::Window) {
            self.input.mode = Mode::Normal;
        }
        let mut out = self.release_held();
        self.handle_recording_on_transport_stop(&mut out);
        out.stop_audition = true;
        if self.is_solo_auditioning() {
            let stopped = self.stop_audition()?;
            out.live_notes_off.extend(stopped.live_notes_off);
        } else {
            self.live.audition = None;
        }
        let loop_region = self.playing_loop();
        let cur_song = self.song_position_opt(self.transport.step);
        if let Some((start, end)) = loop_region.filter(|(start, _)| {
            self.transport.playing
                || cur_song != Some(u64::from(*start))
                || self.live.pending_timeline().is_some()
        }) {
            // First Home inside a loop pauses at loop start, preserving loop + overrides.
            self.live.cancel_pending_timeline();
            let now = self.transport.next_step;
            let seg = cadence_core::timeline::Segment::new(
                now,
                u64::from(start),
                Some((u64::from(start), u64::from(end - start))),
                self.end_rule(),
            );
            self.live.set_segment(seg);
            if let Some(take) = &mut self.takes.active {
                take.record_segment(now, seg, "loop start");
            }
            self.transport.step = now;
            self.transport.next_step = now;
            self.transport.playing = false;
            self.live.next_step = now;
            self.live.playing = false;
            self.refresh_audio()?;
            self.status = format!(
                "Paused at loop start (bar {}) · Home again for song start · Space resumes",
                start / 16 + 1
            );
            out.changed = true;
            out.transport.push(TransportIntent::Play { playing: false });
            out.transport.push(TransportIntent::Seek { step: now });
        } else {
            // Second Home (or first Home with no loop): seal take, clear overrides/loop,
            // return to song start 0 without cutting ringing voices/tails.
            self.follow.clear_anchors();
            self.takes.growing_keeps.clear();
            self.takes.growing_keep_revision = None;
            self.staging = None;
            if self.input.mode == Mode::Stage {
                self.input.mode = Mode::Normal;
            }
            self.finish_active_take()?;
            self.live.active_mut().clear();
            self.live.pending.clear();
            self.sync_flags_from_live()?;
            self.live.audition = None;
            self.live.set_segment(cadence_core::timeline::Segment::new(
                0,
                0,
                None,
                self.end_rule(),
            ));
            self.live.next_step = 0;
            self.live.playing = false;
            self.transport.playing = false;
            self.transport.step = 0;
            self.transport.next_step = 0;
            self.transport.parked = None;
            self.refresh_audio()?;
            self.status = "Stopped · song start · Space resumes".into();
            out.changed = true;
            out.transport.push(TransportIntent::Play { playing: false });
            out.transport.push(TransportIntent::Seek { step: 0 });
        }
        Ok(out)
    }

    pub(super) fn apply_restart_loop(&mut self) -> Result<Outcome> {
        let mut out = self.release_held();
        if self.is_solo_auditioning() {
            self.transport.step = 0;
            self.transport.next_step = 0;
            self.live.next_step = 0;
            self.refresh_audio()?;
            self.status = "Restarted audition · play/pause preserved".into();
            out.changed = true;
            out.transport.push(TransportIntent::Seek { step: 0 });
            return Ok(out);
        }
        self.live.cancel_pending_timeline();
        let loop_region = self.playing_loop();
        let target = loop_region.map_or(0, |(start, _)| u64::from(start));
        let region = loop_region.map(|(s, e)| (u64::from(s), u64::from(e - s)));
        let now = self.transport.next_step;
        let seg = cadence_core::timeline::Segment::new(now, target, region, self.end_rule());
        self.live.set_segment(seg);
        if let Some(take) = &mut self.takes.active {
            take.record_segment(now, seg, "restart");
        }
        self.transport.step = now;
        self.transport.next_step = now;
        self.live.next_step = now;
        self.refresh_audio()?;
        self.status = "Restarted active loop · play/pause preserved".into();
        out.changed = true;
        out.transport.push(TransportIntent::Seek { step: now });
        Ok(out)
    }

    pub(super) fn apply_cue_bar(&mut self, bar: usize) -> Result<Outcome> {
        ensure!(bar < self.doc.project().bars, "Bar out of range");
        if !self.transport.playing {
            return self.cue_song(bar * 16, false);
        }
        let now = self.transport.next_step;
        let at = now.div_ceil(16) * 16;
        let seg =
            cadence_core::timeline::Segment::new(at, (bar * 16) as u64, None, self.end_rule());
        let label = format!("Cue bar {}", bar + 1);
        self.live.enqueue(
            at,
            cadence_core::live::Change::Timeline { segment: seg },
            label.clone(),
        );
        let landed = self.live.land_due();
        self.record_landed(landed);
        self.refresh_audio()?;
        self.status = self.describe_landing(&label);
        Ok(Outcome {
            changed: true,
            ..Outcome::default()
        })
    }
}
