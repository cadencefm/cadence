//! Sample-clock real-time audio engine coordinating lock-free control/score
//! publication, step scheduling, voice pool rendering, mixing, and telemetry.
use super::input::InputMonitorDrain;
use super::scheduler::{StepAction, StepScheduler};
use super::shared::*;
use super::voice_pool::{automation_position, LiveNote, TriggerRequest, VoicePool, ZoneTriggerCtx};
#[cfg(test)]
use super::voices::Voice;
use super::*;
use crate::mix_engine::MixerEngine;
use cadence_core::dsp::{control_stride_mask, one_pole_decay_ms, sanitize_sample};
use cadence_core::mixer::ChannelRef;
use cadence_core::runtime::{AudioInputStatus, CapturePhase};
use cadence_core::score::{ScoreWindow, MAX_INSTRUMENTS};
use std::sync::atomic::{fence, Ordering};
use std::sync::Arc;

/// Target continuous control / automation evaluation cadence (`750 Hz`:
/// `64` frames @ 44.1/48 kHz, `128` @ 96 kHz, `256` @ 192 kHz).
const CONTROL_TARGET_HZ: f64 = 750.0;
/// Target live mixer meter seqlock publication cadence (`187.5 Hz`:
/// `256` frames @ 44.1/48 kHz, `512` @ 96 kHz, `1024` @ 192 kHz).
const METER_PUBLISH_TARGET_HZ: f64 = 187.5;
/// Target master oscilloscope downsampling cadence (`1_500 Hz`:
/// `32` frames @ 44.1/48 kHz, `64` @ 96 kHz, `128` @ 192 kHz).
const WAVE_DOWNSAMPLE_TARGET_HZ: f64 = 1_500.0;

/// Construct a [`ZoneTriggerCtx`] borrowing only read-only fields of `Engine`
/// so callers can simultaneously borrow `voice_pool`, `scheduler`, or `mixer` mutably.
macro_rules! trigger_ctx {
    ($this:expr) => {
        ZoneTriggerCtx {
            project: &$this.project,
            samples: &$this.samples,
            sample_rate: $this.sample_rate,
            rendered_frames: $this.rendered_frames,
        }
    };
}

pub(super) struct Engine {
    project: Box<ScoreWindow>,
    publication: Box<ProjectPublication>,
    project_generation: u64,
    samples: Box<SampleBank>,
    audition: Box<Option<AuditionRequest>>,
    audition_pending: bool,
    scope_offset: usize,
    scope_write_page: usize,
    shared: Arc<Shared>,
    sample_rate: f64,
    voice_pool: VoicePool,
    scheduler: StepScheduler,
    control_counter: u32,
    control_mask: u32,
    mixer: Box<MixerEngine>,
    rendered_frames: u64,
    play_step: u64,
    wave_counter: u32,
    wave_mask: u32,
    wave_peak: f32,
    meter_counter: u32,
    meter_mask: u32,
    output_meter_decay: f32,
    level: f32,
    level_left: f32,
    level_right: f32,
    audition_serial: u32,
    monitor_drain: InputMonitorDrain,
}

impl Engine {
    pub(super) fn new(
        project: ScoreWindow,
        samples: SampleBank,
        sample_rate: u32,
        shared: Arc<Shared>,
    ) -> Self {
        let mixer = MixerEngine::new(&project, sample_rate);
        shared.publish_input_tracks(&project);
        shared.sample_rate.store(sample_rate, Ordering::Release);
        shared
            .prepared_delays
            .store(mixer.allocated_delay_count(), Ordering::Release);
        shared.pdc_latency_frames.store(
            mixer.total_output_latency_frames().min(u32::MAX as usize) as u32,
            Ordering::Release,
        );
        let sr = f64::from(sample_rate);
        let project_generation = shared.input_generation.load(Ordering::Acquire);
        let mut engine = Self {
            project: Box::new(project),
            publication: Box::new(ProjectPublication {
                project: None,
                samples: None,
                extra_delays: Vec::new(),
                generation: project_generation,
            }),
            project_generation,
            samples: Box::new(samples),
            audition: Box::new(None),
            audition_pending: false,
            scope_offset: 0,
            scope_write_page: 1,
            shared,
            sample_rate: sr,
            voice_pool: VoicePool::new(sr),
            scheduler: StepScheduler::new(),
            control_counter: 0,
            control_mask: control_stride_mask(sr, CONTROL_TARGET_HZ),
            mixer,
            rendered_frames: 0,
            play_step: 0,
            wave_counter: 0,
            wave_mask: control_stride_mask(sr, WAVE_DOWNSAMPLE_TARGET_HZ),
            wave_peak: 0.0,
            meter_counter: 0,
            meter_mask: control_stride_mask(sr, METER_PUBLISH_TARGET_HZ),
            output_meter_decay: one_pole_decay_ms(300.0, sr) as f32,
            level: 0.0,
            level_left: 0.0,
            level_right: 0.0,
            audition_serial: 0,
            monitor_drain: InputMonitorDrain::new(),
        };
        engine.publish_scope_tracks(true);
        engine.publish_mixer_meters();
        engine.update_control_block(false);
        engine.mixer.snap_smoothers();
        engine.control_counter = 0;
        engine
    }

    #[cfg(test)]
    #[inline]
    pub(super) fn voices(&self) -> &[Voice] {
        self.voice_pool.voices()
    }

    #[inline]
    pub(super) fn play_step(&self) -> u64 {
        self.play_step
    }

    /// Stage a score update into the lock-free publication slot for the current generation.
    pub(super) fn publish_score_update(&self, score: ScoreWindow) {
        let extra_delays = self.shared.prepare_extra_delays(&score);
        self.shared.project_slot.publish(ProjectPublication {
            project: Some(Box::new(score)),
            samples: None,
            extra_delays,
            generation: self.project_generation,
        });
    }

    /// Collect whole-render channel and effect telemetry against the active score.
    pub(super) fn collect_render_telemetry(
        &self,
    ) -> (Vec<RenderChannelReport>, Vec<RenderEffectReport>) {
        let channels = self
            .mixer
            .render_channel_meters()
            .map(|meter| RenderChannelReport {
                channel: meter.channel,
                name: self
                    .project
                    .channel_name(meter.channel)
                    .unwrap_or("Unknown")
                    .into(),
                input_peak_hold: meter.input_peak_hold,
                output_peak_hold: meter.output_peak_hold,
                hot: meter.hot,
                clipped: meter.clipped,
            })
            .collect();
        let effects = self
            .mixer
            .render_effect_stats()
            .filter_map(|(device_id, max_gr_db, avg_gr_db)| {
                let (channel, device) = self.project.find_device(device_id)?;
                Some(RenderEffectReport {
                    device_id,
                    channel,
                    kind: device.kind,
                    max_gr_db,
                    avg_gr_db,
                })
            })
            .collect();
        (channels, effects)
    }

    fn commit_pending(&mut self) {
        if self.shared.project_slot.commit_into(&mut self.publication) {
            let Some(project) = self.publication.project.take() else {
                return;
            };
            self.publication.project = Some(std::mem::replace(&mut self.project, project));
            if let Some(samples) = self.publication.samples.take() {
                self.publication.samples = Some(std::mem::replace(&mut self.samples, samples));
            }
            if !self.publication.extra_delays.is_empty() {
                self.mixer
                    .install_delay_buffers(&mut self.publication.extra_delays);
            }
            let boundary = self.project_generation != self.publication.generation;
            self.mixer.adopt(&self.project, boundary);
            self.shared.pdc_latency_frames.store(
                self.mixer
                    .total_output_latency_frames()
                    .min(u32::MAX as usize) as u32,
                Ordering::Release,
            );
            if boundary {
                self.project_generation = self.publication.generation;
                self.voice_pool.clear_all();
                self.voice_pool.clear_held_notes();
                self.scheduler.clear_pending();
                self.level = 0.0;
                self.level_left = 0.0;
                self.level_right = 0.0;
            }
            self.voice_pool.remap_tracks(&self.project);
            self.publish_scope_tracks(boundary);
            self.publish_mixer_meters();
            self.control_counter = 0;
        }
    }

    fn publish_mixer_meters(&self) {
        // Explicit Release fence after opening odd increment ensures weak-memory readers
        // never observe torn payload writes ahead of the generation bump.
        self.shared
            .mixer_meter_generation
            .fetch_add(1, Ordering::Relaxed);
        fence(Ordering::Release);
        let mut count = 0;
        for (slot, meter) in self.shared.mixer_meters.iter().zip(self.mixer.meters()) {
            slot.write(meter);
            count += 1;
        }
        for slot in &self.shared.mixer_meters[count..] {
            slot.kind.store(0, Ordering::Relaxed);
        }
        let mut fx_count = 0;
        for (slot, meter) in self
            .shared
            .effect_meters
            .iter()
            .zip(self.mixer.effect_meters())
        {
            slot.write(meter);
            fx_count += 1;
        }
        for slot in &self.shared.effect_meters[fx_count..] {
            slot.clear();
        }
        self.shared
            .mixer_meter_generation
            .fetch_add(1, Ordering::Release);
    }

    fn publish_scope_tracks(&mut self, boundary: bool) {
        let previous_count = self.shared.track_count.load(Ordering::Relaxed);
        self.shared.scope_generation.fetch_add(1, Ordering::Relaxed);
        fence(Ordering::Release);
        for index in 0..MAX_INSTRUMENTS {
            let id = self
                .project
                .instruments
                .get(index)
                .map_or(0, |inst| inst.id);
            if boundary
                || index >= previous_count
                || index >= self.project.instruments.len()
                || self.shared.track_ids[index].load(Ordering::Relaxed) != id
            {
                for sample in &self.shared.track_waveform[index] {
                    sample.store(0.0_f32.to_bits(), Ordering::Relaxed);
                }
            }
            self.shared.track_ids[index].store(id, Ordering::Relaxed);
        }
        self.shared
            .track_count
            .store(self.project.instruments.len(), Ordering::Release);
        self.shared.scope_generation.fetch_add(1, Ordering::Release);
    }

    /// Strictly RT-safe, zero-allocation emergency silence: clears active voices,
    /// steal fades, held notes, pending sub-step events, and mixer FX tails/meters
    /// so ringing voices and reverbs/delays stop burning CPU immediately.
    pub(super) fn silence_in_place(&mut self) {
        self.voice_pool.clear_all();
        self.voice_pool.clear_held_notes();
        self.scheduler.clear_pending();
        self.mixer.reset();
        self.monitor_drain.reset();
        self.level = 0.0;
        self.level_left = 0.0;
        self.level_right = 0.0;
        self.wave_peak = 0.0;
        self.shared.peak.store(0.0_f32.to_bits(), Ordering::Relaxed);
        self.shared
            .peak_left
            .store(0.0_f32.to_bits(), Ordering::Relaxed);
        self.shared
            .peak_right
            .store(0.0_f32.to_bits(), Ordering::Relaxed);
        self.publish_mixer_meters();
    }

    fn hard_reset(&mut self, start_step: u64) {
        self.shared.playing.store(false, Ordering::Release);
        self.shared.clear_overload_latch();
        self.play_step = start_step;
        self.scheduler.reset(start_step);
        self.control_counter = 0;
        self.silence_in_place();
        self.shared
            .step_position
            .store(pack_step_position(start_step, 0.0), Ordering::Release);
        self.shared.next_step.store(start_step, Ordering::Release);
    }

    /// Seek to `step` without killing ringing voices or effect tails.
    pub(super) fn seek_to_step(&mut self, step: u64) {
        self.play_step = step;
        self.scheduler.seek(step);
        self.control_counter = 0;
        self.shared
            .step_position
            .store(pack_step_position(step, 0.0), Ordering::Release);
        self.shared.next_step.store(step, Ordering::Release);
    }

    /// Evaluate and snap mixer & effect smoothers to the active step immediately
    /// to eliminate startup slew when rendering from `start_step > 0`.
    pub(super) fn snap_controls_to_current_step(&mut self) {
        self.update_control_block(true);
        self.mixer.snap_smoothers();
        self.control_counter = 0;
    }

    fn apply_transport(&mut self, intent: TransportIntent) {
        match intent {
            TransportIntent::Play { playing } => {
                if playing {
                    self.shared.clear_overload_latch();
                }
                self.shared.playing.store(playing, Ordering::Release);
            }
            TransportIntent::Seek { step } => self.seek_to_step(step),
            TransportIntent::Reset { step } => self.hard_reset(step),
        }
    }

    pub(super) fn poll_controls(&mut self) {
        let cutoff = self.shared.event_sequence.load(Ordering::Acquire);
        if self.shared.clear_all_meters.swap(false, Ordering::AcqRel) {
            self.mixer.clear_meters(None);
            self.publish_mixer_meters();
        }
        self.commit_pending();

        if self.shared.audition_slot.commit_into(&mut self.audition) {
            // A sample resource has changed. No previous audition may read the
            // replacement buffer, even when this request awaits the next cutoff.
            self.voice_pool.clear_preview_voices();
            self.audition_pending = true;
        }

        // Snapshot a producer sequence boundary before reading mailboxes. Events
        // posted during this poll wait until the next poll instead of overtaking
        // older events in a previously visited slot.
        let mut events = [LiveEvent::default(); MAILBOXES + TRANSPORT_MAILBOXES + 1];
        let mut count = 0;
        for index in 0..MAILBOXES {
            let packed = self.shared.events[index].load(Ordering::Acquire);
            if packed == 0 || packed == EVENT_RESERVED {
                continue;
            }
            let sequence = self.shared.event_sequences[index].load(Ordering::Relaxed);
            if sequence > cutoff {
                continue;
            }
            let track_id = self.shared.event_track_ids[index].load(Ordering::Relaxed);
            let project_generation = self.shared.event_generations[index].load(Ordering::Relaxed);
            let source_part = (packed & EVENT_HAS_BINDING != 0)
                .then(|| self.shared.event_bindings[index].load(Ordering::Relaxed));
            // Read metadata before freeing the slot for the producer.
            self.shared.events[index].store(0, Ordering::Release);
            events[count] = LiveEvent {
                packed,
                track_id,
                sequence,
                project_generation,
                source_part,
            };
            count += 1;
        }
        let mut has_transport = false;
        for index in 0..TRANSPORT_MAILBOXES {
            let packed = self.shared.transport_events[index].load(Ordering::Acquire);
            if packed == 0 || packed == EVENT_RESERVED {
                continue;
            }
            let sequence = self.shared.transport_sequences[index].load(Ordering::Relaxed);
            if sequence > cutoff {
                continue;
            }
            let project_generation =
                self.shared.transport_generations[index].load(Ordering::Relaxed);
            self.shared.transport_events[index].store(0, Ordering::Release);
            events[count] = LiveEvent {
                packed,
                sequence,
                project_generation,
                ..LiveEvent::default()
            };
            count += 1;
            has_transport = true;
        }
        if has_transport {
            // Explicit transport resolves against the latest prepared score.
            self.commit_pending();
        }
        if self.audition_pending {
            if let Some(request) = self
                .audition
                .as_ref()
                .as_ref()
                .filter(|request| request.sequence <= cutoff)
            {
                events[count] = LiveEvent {
                    packed: 6,
                    sequence: request.sequence,
                    project_generation: request.project_generation,
                    ..LiveEvent::default()
                };
                count += 1;
                self.audition_pending = false;
            }
        }
        events[..count].sort_unstable_by_key(|event| event.sequence);
        for event in &events[..count] {
            self.handle_live_event(*event);
            if event.packed & 7 == 5 {
                self.shared.transport_pending.fetch_sub(1, Ordering::AcqRel);
            }
        }
        if self
            .shared
            .release_all_requested
            .swap(false, Ordering::AcqRel)
        {
            self.voice_pool.release_all_live();
        }
    }

    fn trigger_audition(&mut self) {
        let Some(request) = self.audition.as_ref().as_ref() else {
            self.voice_pool.clear_preview_voices();
            return;
        };
        self.audition_serial = self.audition_serial.wrapping_add(1);
        let ctx = trigger_ctx!(self);
        self.voice_pool
            .trigger_audition(&ctx, request, self.audition_serial);
    }

    fn handle_live_event(&mut self, event: LiveEvent) {
        if event.project_generation != self.project_generation {
            return;
        }
        let tag = event.packed & 0b111;
        if tag == 6 {
            self.trigger_audition();
            return;
        }
        if tag == 5 {
            if let Some(intent) = unpack_transport_intent(event.packed >> 3) {
                self.apply_transport(intent);
            }
            return;
        }
        if tag == 4 {
            let channel = match (event.packed >> 3) & 0b11 {
                0 => ChannelRef::Master,
                2 => ChannelRef::Channel(event.track_id),
                _ => return,
            };
            self.mixer.clear_meters(Some(channel));
            self.publish_mixer_meters();
            return;
        }
        let midi = ((event.packed >> 11) & 0x7f) as u8;
        let velocity = ((event.packed >> 18) & 0xffff) as f32 / 65_535.0;
        let Some(track_idx) = self
            .project
            .instruments
            .iter()
            .position(|t| t.id == event.track_id)
        else {
            return;
        };
        if matches!(tag, 1 | 2) {
            if let Some(binding) = event.source_part {
                self.mixer.trigger_part(binding);
            }
        }
        let note = LiveNote {
            track_idx,
            track_id: event.track_id,
            midi,
            velocity,
        };
        match tag {
            1 => {
                self.audition_serial = self.audition_serial.wrapping_add(1);
                let seed = self.audition_serial;
                let ctx = trigger_ctx!(self);
                self.voice_pool.trigger_on_track(
                    &ctx,
                    &TriggerRequest {
                        inst_index: track_idx,
                        live_key: None,
                        midi,
                        velocity: velocity * 0.85,
                        gate_seconds: 0.22,
                        seed,
                        mod_pos: None,
                        locks: &[],
                    },
                );
            }
            2 => {
                self.audition_serial = self.audition_serial.wrapping_add(1);
                let seed = self.audition_serial;
                let ctx = trigger_ctx!(self);
                self.voice_pool.handle_live_note_on(&ctx, note, seed);
            }
            3 => {
                let ctx = trigger_ctx!(self);
                self.voice_pool
                    .handle_live_note_off(&ctx, note, &mut self.audition_serial);
            }
            _ => {}
        }
    }

    fn update_control_block(&mut self, playing: bool) {
        let (last_abs_step, frac) = self.scheduler.step_position(playing);
        self.shared
            .step_position
            .store(pack_step_position(last_abs_step, frac), Ordering::Release);
        let abs_pos = last_abs_step as f64 + frac;
        let auto_pos = automation_position(&self.project, abs_pos);
        self.voice_pool
            .update_continuous_controls(&self.project, auto_pos);
        self.mixer.update_controls(&self.project, auto_pos, abs_pos);
    }

    fn sync_capture_gate_for_step(&self, step: u64) {
        let phase = CapturePhase::from_u8(self.shared.capture_phase.load(Ordering::Acquire));
        match phase {
            CapturePhase::Armed => {
                let quantized = self.shared.capture_quantized.load(Ordering::Relaxed);
                let punch_in = self.shared.capture_punch_in_step.load(Ordering::Acquire);
                if !quantized || step >= punch_in {
                    self.shared
                        .capture_start_step
                        .store(step, Ordering::Release);
                    let _ = self.shared.capture_phase.compare_exchange(
                        CapturePhase::Armed as u8,
                        CapturePhase::Recording as u8,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    );
                    self.shared.refresh_monitoring_active();
                }
            }
            CapturePhase::Recording => {
                if self.shared.capture_has_punch_out.load(Ordering::Acquire) {
                    let punch_out = self.shared.capture_punch_out_step.load(Ordering::Acquire);
                    if step >= punch_out {
                        self.finish_capture_if_recording(step);
                    }
                }
            }
            CapturePhase::Idle | CapturePhase::Finishing => {}
        }
    }

    fn finish_capture_if_recording(&self, end_step: u64) {
        let phase = CapturePhase::from_u8(self.shared.capture_phase.load(Ordering::Acquire));
        if phase != CapturePhase::Recording {
            return;
        }
        self.shared
            .capture_end_step
            .store(end_step, Ordering::Release);
        let input_running =
            AudioInputStatus::from_u8(self.shared.input_status.load(Ordering::Acquire))
                == AudioInputStatus::Running;
        let next = if input_running && self.shared.capture_tail_frames.load(Ordering::Acquire) > 0 {
            CapturePhase::Finishing
        } else {
            CapturePhase::Idle
        };
        let _ = self.shared.capture_phase.compare_exchange(
            CapturePhase::Recording as u8,
            next as u8,
            Ordering::AcqRel,
            Ordering::Relaxed,
        );
        self.shared.refresh_monitoring_active();
    }

    fn step(&mut self) {
        self.commit_pending();

        match StepScheduler::resolve_step_action(&self.project, self.play_step) {
            StepAction::StopAt(stop) => {
                self.finish_capture_if_recording(stop);
                self.play_step = stop;
                self.scheduler.seek(stop);
                self.control_counter = 0;
                self.shared.playing.store(false, Ordering::Release);
                self.shared
                    .step_position
                    .store(pack_step_position(stop, 0.0), Ordering::Release);
                self.shared.next_step.store(stop, Ordering::Release);
                return;
            }
            StepAction::Trigger(step) => {
                self.sync_capture_gate_for_step(step);
                self.trigger_song_step(step);
                self.play_step = step + 1;
            }
        }

        self.shared
            .next_step
            .store(self.play_step, Ordering::Release);
        self.control_counter = 0;
    }

    fn trigger_song_step(&mut self, abs_step: u64) {
        self.shared
            .step_position
            .store(pack_step_position(abs_step, 0.0), Ordering::Release);
        let ctx = trigger_ctx!(self);
        self.scheduler.schedule_step(
            &self.project,
            abs_step,
            self.sample_rate,
            &self.shared.event_overflows,
            |event, locks, seed, mod_pos| {
                if let Some(binding) = event.part_id {
                    self.mixer.trigger_part(binding);
                }
                self.voice_pool
                    .trigger_score_event(&ctx, event, locks, seed, mod_pos);
            },
        );
    }

    fn fire_due_sub_events(&mut self) {
        let ctx = trigger_ctx!(self);
        self.scheduler.fire_due(|event, locks, seed, mod_pos| {
            if let Some(binding) = event.part_id {
                self.mixer.trigger_part(binding);
            }
            self.voice_pool
                .trigger_score_event(&ctx, event, locks, seed, mod_pos);
        });
    }

    pub(super) fn next_frame(&mut self) -> [f32; 2] {
        self.rendered_frames = self.rendered_frames.wrapping_add(1);
        let mut playing = self.shared.playing.load(Ordering::Relaxed);
        if playing {
            // Fire due sub-step events before downbeat `step()` so offset=0.9999 never inverts.
            self.fire_due_sub_events();
            if self.scheduler.is_step_due() {
                self.scheduler.consume_step_phase();
                self.step();
                playing = self.shared.playing.load(Ordering::Relaxed);
            }
        }
        // Run control block before advancing frame / step_phase to keep modulation aligned to frame 0.
        if self.control_counter == 0 {
            self.update_control_block(playing);
        }
        self.control_counter = self.control_counter.wrapping_add(1) & self.control_mask;
        if playing {
            self.scheduler
                .advance_frame(self.project.step_seconds(), self.sample_rate);
        }

        let preview_ref = self
            .audition
            .as_ref()
            .as_ref()
            .and_then(|request| match &request.kind {
                AuditionKind::Sample { sample, .. } => Some(sample),
                _ => None,
            });
        let frame_out = self.voice_pool.render_frame(&self.samples, preview_ref);
        let mixed = self.mixer.process(&frame_out.sources, frame_out.preview);

        let scope_index = self.scope_write_page * WAVE_SAMPLES + self.scope_offset;
        let inst_count = self.project.instruments.len().min(MAX_INSTRUMENTS);
        for (slot, value) in self.shared.track_waveform[..inst_count]
            .iter()
            .zip(&frame_out.track_scope[..inst_count])
        {
            slot[scope_index].store(value.to_bits(), Ordering::Relaxed);
        }
        self.scope_offset += 1;
        if self.scope_offset == WAVE_SAMPLES {
            self.shared
                .scope_page
                .store(self.scope_write_page, Ordering::Release);
            self.shared.scope_sequence.fetch_add(1, Ordering::Release);
            self.scope_write_page ^= 1;
            self.scope_offset = 0;
        }

        // Unclipped 32-bit float Master output (clamping happens only at final DAC / WAV boundaries).
        let mon = self
            .monitor_drain
            .next_frame(&self.shared, self.sample_rate);
        let out = [
            sanitize_sample(mixed[0] + mon[0]),
            sanitize_sample(mixed[1] + mon[1]),
        ];
        // Lock-free SPSC stereo tap ring (zero overhead when disabled).
        if self.shared.tap_enabled.load(Ordering::Relaxed) {
            let write = self.shared.tap_write.load(Ordering::Relaxed);
            let read = self.shared.tap_read.load(Ordering::Acquire);
            if write.wrapping_sub(read) < AUDIO_TAP_CAPACITY as u64 {
                let slot = (write as usize) & AUDIO_TAP_MASK;
                self.shared.audio_tap[slot][0].store(out[0].to_bits(), Ordering::Relaxed);
                self.shared.audio_tap[slot][1].store(out[1].to_bits(), Ordering::Relaxed);
                self.shared
                    .tap_write
                    .store(write.wrapping_add(1), Ordering::Release);
            } else {
                self.shared.tap_dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
        let decay = self.output_meter_decay;
        self.level_left = (self.level_left * decay).max(out[0].abs());
        self.level_right = (self.level_right * decay).max(out[1].abs());
        self.level = self.level_left.max(self.level_right);

        // Peak-preserving signed-max waveform downsample at ~1_500 Hz across sample rates.
        let mono_sample = (out[0] + out[1]) * 0.5;
        if mono_sample.abs() >= self.wave_peak.abs() {
            self.wave_peak = mono_sample;
        }
        self.wave_counter = self.wave_counter.wrapping_add(1) & self.wave_mask;
        if self.wave_counter == 0 {
            let index = self.shared.wave_write.load(Ordering::Relaxed);
            self.shared.waveform[index].store(self.wave_peak.to_bits(), Ordering::Relaxed);
            self.shared
                .wave_write
                .store((index + 1) % WAVE_SAMPLES, Ordering::Release);
            self.wave_peak = 0.0;
        }

        // Throttle mixer meter seqlock publication to ~187.5 Hz across sample rates.
        self.meter_counter = self.meter_counter.wrapping_add(1) & self.meter_mask;
        if self.meter_counter == 0 {
            self.shared
                .peak
                .store(self.level.to_bits(), Ordering::Relaxed);
            self.shared
                .peak_left
                .store(self.level_left.to_bits(), Ordering::Relaxed);
            self.shared
                .peak_right
                .store(self.level_right.to_bits(), Ordering::Relaxed);
            self.publish_mixer_meters();
        }
        out
    }
}
