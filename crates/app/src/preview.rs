//! Pattern audition (solo when stopped, layered on the launch boundary when
//! playing), cueing, single-note audition, and unified transport observation.
use super::*;
use cadence_core::live::position_label;
use cadence_core::timeline::Segment;

impl App {
    /// Observe a full live [`cadence_core::runtime::AudioSnapshot`]: transport position
    /// advancement, pending landing, take recording, Follow, and edge-triggered stream health alerts.
    pub fn observe_snapshot(&mut self, snapshot: &cadence_core::runtime::AudioSnapshot) -> Outcome {
        let out = self.observe_transport(
            snapshot.absolute_step,
            snapshot.next_step,
            snapshot.playing,
            snapshot.pending,
        );
        self.observe_audio_health(&snapshot.health);
        self.observe_audio_recording(snapshot);
        out
    }

    /// The engine's transport each frame. Lands pending changes, records takes,
    /// keeps the ring ahead of the playhead, and updates Follow observation.
    pub fn observe_transport(
        &mut self,
        step: u64,
        next_step: u64,
        playing: bool,
        pending: bool,
    ) -> Outcome {
        if pending {
            return Outcome::default();
        }
        let was_playing = self.transport.playing;
        self.transport.step = step;
        self.transport.next_step = next_step;
        self.transport.playing = playing;
        self.live.playing = playing;
        self.live.next_step = next_step;
        let landed = self.live.land_due();
        self.record_landed(landed);
        if let Some(take) = &mut self.takes.active {
            take.advance_to(next_step);
        }
        let mut out = Outcome::default();
        if playing && !self.takes.growing_keeps.is_empty() && self.grow_active_keeps() {
            out.changed = true;
        }
        if was_playing && !playing && !self.is_solo_auditioning() {
            let song_len = self.doc.project().arrangement_steps() as u64;
            if self
                .live
                .timeline()
                .stop_at(next_step, song_len)
                .is_some_and(|stop| next_step >= stop)
            {
                self.takes.growing_keeps.clear();
                self.takes.growing_keep_revision = None;
                let _ = self.finish_active_take();
            }
        }
        if self.live.needs_extension() {
            match self.refresh_audio() {
                Ok(()) => out.changed = true,
                Err(error) => self.status = format!("Live: {error:#}"),
            }
        }
        self.observe_follow(step);
        out
    }

    pub(super) fn observe_follow(&mut self, step: u64) {
        if self.follow.active()
            && self.workspace() == Workspace::Pattern
            && !self.view_state.deck_zoomed
        {
            if let Some(request) = self.playback_request(step) {
                let live = cadence_core::playback::LiveProjection {
                    transport_step: step,
                    segment: Some(self.live.timeline().segment_at(step)),
                    active: self.live.active(),
                    audition: self.live.audition,
                };
                let _ =
                    self.follow
                        .observe_live(self.doc.project(), self.doc.flags(), live, request);
            }
        }
    }

    pub fn inspect_assets(&self) -> serde_json::Value {
        serde_json::json!({"ready":self.ready(),"issues":self.sample_issues(),"required":self.required_sample_issues(),"repair":"Select an instrument, press B to assign a sample, then press r in Sample Assets (or Reload Project Samples in Palette)"})
    }

    pub fn inspect(&self, pattern: Option<Id>) -> serde_json::Value {
        let id = pattern.or(self.selected_pattern);
        let material = id.and_then(|id| self.pattern(id));
        let preview = id.map(|id| {
            self.doc
                .project()
                .preview(id, self.cursor as i64, 16)
                .map_err(|e| e.to_string())
        });
        serde_json::json!({"pattern":material,"period":id.and_then(|id|self.doc.project().pattern_period(id).ok()),"dependencies":id.and_then(|id|self.doc.project().dependencies(id).ok()),"consumers":id.map(|id|self.consumers(id)),"preview":preview,"placement":self.selected_placement.and_then(|id|self.placement(id).map(|(_,p)|p)),"part":self.selected_part_object(),"instrument":self.selected_instrument_object(),"scope":self.scope,"modulations":self.doc.project().modulations,"ducks":self.doc.project().ducks,"required_instrument_ids":self.doc.compiled().required_instrument_ids})
    }

    /// Cue song step `step` (preserving active overrides and ringing voices/tails).
    pub(super) fn cue_song(&mut self, step: usize, play: bool) -> Result<Outcome> {
        self.cue_song_with_region(step, play, None)
    }

    /// Cue song step `step` while optionally preserving an active loop `region` (`(start, len)`).
    pub(super) fn cue_song_with_region(
        &mut self,
        step: usize,
        play: bool,
        region: Option<(u64, u64)>,
    ) -> Result<Outcome> {
        ensure!(
            step < self.doc.project().arrangement_steps(),
            "Song step out of range"
        );
        let mut out = Outcome::default();
        if self.is_solo_auditioning() {
            let stopped = self.stop_audition()?;
            out.live_notes_off.extend(stopped.live_notes_off);
            out.changed |= stopped.changed;
        }
        out.live_notes_off
            .extend(self.release_held().live_notes_off);
        let playing = play || self.transport.playing;
        self.live.cancel_pending_timeline();
        let now = self.transport.next_step;
        let segment = Segment::new(now, step as u64, region, self.end_rule());
        self.live.set_segment(segment);
        if let Some(take) = &mut self.takes.active {
            take.record_segment(now, segment, "cue");
        }
        if playing {
            self.ensure_take_open()?;
        }
        self.transport.step = now;
        self.transport.next_step = now;
        self.transport.playing = playing;
        self.live.next_step = now;
        self.live.playing = playing;
        self.refresh_audio()?;
        out.changed = true;
        out.transport.push(TransportIntent::Seek { step: now });
        out.transport.push(TransportIntent::Play { playing });
        self.status = format!(
            "{} song at {:03}.{}.{} · Arrange cursor unchanged",
            if playing { "Playing" } else { "Cued" },
            step / 16 + 1,
            step % 16 / 4 + 1,
            step % 4 + 1
        );
        Ok(out)
    }

    /// Verify that all sample buffers referenced by `instrument_id` (across every
    /// `Sampler` module in every `RackZone`) are assigned, loaded, and ready.
    pub(crate) fn instrument_samples_ready(&self, instrument_id: Id) -> Result<()> {
        let Some(inst) = self.instrument(instrument_id) else {
            return Ok(());
        };
        let zone_count = inst.rack.zones.len();
        for (zone_id, zone, mod_id, smp) in inst.rack.sampler_modules() {
            if smp.path.is_none() {
                if zone_count <= 1 {
                    bail!(
                        "Rack '{}' has no sample assigned · press B to assign a WAV",
                        inst.name
                    );
                }
                bail!(
                    "Rack '{}' zone '{}' has no sample assigned · press B to assign a WAV",
                    inst.name,
                    zone.name
                );
            }
            let key = module_sample_key(instrument_id, zone_id, mod_id);
            ensure!(
                self.assets.samples.contains_key(&key),
                "Rack '{}' zone '{}' sample is unavailable",
                inst.name,
                zone.name
            );
        }
        Ok(())
    }

    /// Audition a note pattern in context using the active instrument.
    pub(super) fn start_audition(&mut self, pattern: Id, length: Option<usize>) -> Result<Outcome> {
        let instrument = self
            .active_instrument()
            .filter(|id| self.instrument(*id).is_some())
            .unwrap_or(AUDITION_INSTRUMENT_ID);
        self.start_audition_on(pattern, instrument, length)
    }

    /// Audition a note pattern through an explicit `instrument` ID:
    /// - Stopped: plays alone from step 0 with transport parked, restoring on stop.
    /// - Playing: joins over the live mix on the launch boundary without
    ///   disturbing any other instrument, take, or override.
    pub(super) fn start_audition_on(
        &mut self,
        pattern: Id,
        instrument: Id,
        length: Option<usize>,
    ) -> Result<Outcome> {
        ensure!(
            self.doc.project().pattern_type(pattern)? == PatternType::Notes,
            "Rhythm/value sources need a note-producing derivation to audition"
        );
        let period = self.pattern_period(pattern);
        let length = length.unwrap_or(period.min(material::MAX_LITERAL_STEPS));
        ensure!(
            (1..=material::MAX_LITERAL_STEPS).contains(&length),
            "Audition range must be 1..1024 steps"
        );
        self.instrument_samples_ready(instrument)?;
        if !self.transport.playing || self.is_solo_auditioning() {
            if self.transport.parked.is_none() {
                self.transport.parked = Some((
                    self.transport.step,
                    self.transport.next_step,
                    self.live.heard_segment(),
                ));
            }
            self.live.audition = Some(Audition {
                pattern,
                instrument,
                anchor: 0,
                length,
                solo: true,
            });
            self.transport.step = 0;
            self.transport.next_step = 0;
            self.transport.playing = true;
            self.live.next_step = 0;
            self.live.playing = true;
            self.refresh_audio()?;
            self.status = format!("AUDITION (SOLO) · {length} steps · a / Esc stops");
            let mut out = self.release_held();
            out.changed = true;
            out.transport.push(TransportIntent::Seek { step: 0 });
            out.transport.push(TransportIntent::Play { playing: true });
            Ok(out)
        } else {
            let anchor = self
                .live
                .boundary(self.doc.project(), self.doc.project().timing.launch)?;
            self.live.audition = Some(Audition {
                pattern,
                instrument,
                anchor,
                length,
                solo: false,
            });
            self.refresh_audio()?;
            self.status = format!(
                "AUDITION (LIVE) · {length} steps · joins at {} · a / Esc stops",
                position_label(anchor)
            );
            Ok(Outcome {
                changed: true,
                ..Outcome::default()
            })
        }
    }

    pub(super) fn stop_audition(&mut self) -> Result<Outcome> {
        let Some(aud) = self.live.audition.take() else {
            return Ok(Outcome::default());
        };
        let mut out = self.release_held();
        if aud.solo {
            let (step, next_step, segment) =
                self.transport
                    .parked
                    .take()
                    .unwrap_or((0, 0, self.live.heard_segment()));
            self.transport.step = step;
            self.transport.next_step = next_step;
            self.transport.playing = false;
            self.live.next_step = next_step;
            self.live.playing = false;
            self.live.set_segment(segment);
            self.refresh_audio()?;
            out.changed = true;
            out.transport.push(TransportIntent::Play { playing: false });
            out.transport.push(TransportIntent::Seek { step });
            self.status = "Stopped audition · returned to song".into();
        } else {
            self.refresh_audio()?;
            out.changed = true;
            self.status = "Stopped audition · live playback continues".into();
        }
        Ok(out)
    }

    /// Re-validate an active audition after a document edit or undo/redo; stop
    /// cleanly if its pattern or sample disappeared.
    pub(super) fn validate_active_audition(&mut self, out: &mut Outcome) -> Result<()> {
        let Some(aud) = self.live.audition else {
            return Ok(());
        };
        let pattern_ok = self
            .doc
            .project()
            .pattern_type(aud.pattern)
            .is_ok_and(|t| t == PatternType::Notes);
        let instrument_ok = if self.instrument(aud.instrument).is_some() {
            self.instrument_samples_ready(aud.instrument).is_ok()
        } else {
            aud.instrument == AUDITION_INSTRUMENT_ID
        };
        if !pattern_ok || !instrument_ok {
            let stopped = self.stop_audition()?;
            out.changed |= stopped.changed;
            out.transport.extend(stopped.transport);
            out.live_notes_off.extend(stopped.live_notes_off);
            self.status = "Edit applied · audition ended".into();
        }
        Ok(())
    }

    pub(super) fn audition_current(&mut self) -> Result<Outcome> {
        let step = self.current_note()?;
        if !step.active {
            return Ok(Outcome::default());
        }
        let midi = self
            .doc
            .project()
            .pitch_midi(step.degree, step.accidental, 0);
        if let Some(id) = self.selected_instrument {
            self.instrument_samples_ready(id)?;
            Ok(Outcome {
                audition_instrument: Some((id, midi, step.velocity)),
                ..Outcome::default()
            })
        } else {
            Ok(Outcome {
                rack_audition: Some((self.preview_patch.clone(), midi, step.velocity)),
                ..Outcome::default()
            })
        }
    }
}
