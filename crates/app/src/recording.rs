//! Pure live audio input recording controller (`t a` / `t A` / `t i` / `:rec`).
//!
//! `cadence-app` stays strictly pure (no `cadence-engine` or OS audio I/O):
//! `start_audio_recording` emits `Outcome::recording_command` (`RecordingHostCommand::Arm`)
//! with transport-quantized or free-time capture bounds, the host drains the lock-free
//! capture ring and writes an atomic 24-bit stereo WAV with latency compensation, and
//! `App::complete_recording` lands the WAV cleanly into a `Sampler` Rack + arranged
//! `Placement` or target `RackZone` via the shared `land_recorded_sample` pipeline.
use crate::bounce::{BounceTarget, SampleLandingSpec};
use crate::*;
use cadence_core::live::position_label;
use cadence_core::music::note_name;
use cadence_core::runtime::{
    AudioInputMode, AudioInputStatus, AudioSnapshot, CaptureArmSpec, CapturePhase,
    InputMonitorMode, MAX_AUDIO_LATENCY_TRIM_MS, MIN_AUDIO_LATENCY_TRIM_MS,
};

cadence_core::define_choice_enum! {
    /// Timing strategy for live audio input capture.
    #[derive(Default)]
    pub enum RecordingMode [array] {
        #[default]
        Quantized => ("quantized", "Quantized (Transport)", ["sync", "transport"]),
        CountIn => ("count_in", "1-Bar Count-in", ["countin", "1bar", "count-in"]),
        Free => ("free", "Free (No Transport)", ["manual", "unquantized"]),
    }
}

/// Pure recording specification emitted to the host on `Outcome::recording_command`.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordingRequest {
    pub mode: RecordingMode,
    pub target: BounceTarget,
    /// When `true` and `target == NewSampler`, also create a trigger `Part` and arranged
    /// `Placement` at `punch_in_song_step`.
    pub place_in_arrange: bool,
    /// Optional fixed recording length in bars (when `None`, loop bounds or manual stop applies).
    pub bars: Option<usize>,
    /// Display name for the created Sampler instrument / channel / lane (or zone label).
    pub instrument_name: String,
    /// Portable project-relative WAV path stored on the Sampler (`"samples/recordings/<slug>.wav"`).
    pub relative_path: String,
    /// Absolute disk path where the host writes the atomic 24-bit stereo WAV.
    pub output_path: PathBuf,
    /// Absolute transport step where quantized capture begins.
    pub punch_in_abs_step: u64,
    /// Song-timeline step (`0..arrangement_steps`) where an arranged `Placement` lands.
    pub punch_in_song_step: u32,
    /// Optional absolute transport step for automatic punch-out (loop end or fixed bar count).
    pub punch_out_abs_step: Option<u64>,
    pub source_lane: Option<Id>,
    /// Manual latency compensation trim in milliseconds copied from `UserConfig`.
    pub latency_trim_ms: f32,
    /// Lock-free capture gate specification handed to `AudioRuntime::arm_capture`.
    pub arm_spec: CaptureArmSpec,
}

/// Command emitted on `Outcome::recording_command` for the host recorder + engine.
#[derive(Clone, Debug, PartialEq)]
pub enum RecordingHostCommand {
    Arm { request: Box<RecordingRequest> },
    Stop,
    Cancel,
}

/// Completed 24-bit WAV recording handed back to `App::complete_recording` by the host.
#[derive(Clone, Debug, PartialEq)]
pub struct CompletedRecording {
    pub request: RecordingRequest,
    pub frames: u64,
    pub sample_rate: u32,
    pub duration_sec: f64,
    pub peak_left_db: f64,
    pub peak_right_db: f64,
    pub full_scale_samples: u64,
    pub dropped_frames: u64,
    pub actual_start_abs_step: u64,
    pub actual_end_abs_step: u64,
}

/// Options accepted by [`App::start_audio_recording`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StartRecordingOpts {
    pub mode: Option<RecordingMode>,
    pub target: Option<BounceTarget>,
    pub place_in_arrange: Option<bool>,
    pub bars: Option<usize>,
    pub name: Option<String>,
}

/// Live session state tracking hardware audio recording arming, capture progress, and errors.
#[derive(Clone, Debug, Default)]
pub struct RecordingState {
    pub phase: CapturePhase,
    pub active_request: Option<RecordingRequest>,
    pub captured_frames: u64,
    pub dropped_frames: u64,
    pub input_sample_rate: u32,
    pub last_input_failed: bool,
    /// Bounded live waveform peaks accumulated during an active recording for the Sample deck monitor.
    pub live_peaks: Vec<(f32, f32)>,
}

impl RecordingState {
    pub fn is_active(&self) -> bool {
        self.phase != CapturePhase::Idle || self.active_request.is_some()
    }
}

impl App {
    /// Publish bounded growing recording peaks from the off-RT host drainer (`HostRecorder`)
    /// so `DeckKind::Sample` renders the captured waveform in real time while recording.
    pub fn set_recording_preview_peaks(&mut self, peaks: Vec<(f32, f32)>) {
        self.recording.live_peaks = peaks;
    }

    /// Default recording destination matching `default_bounce_target()` 1:1:
    /// active Sample deck target when focused on `DeckKind::Sample`, active Rack zone when
    /// focused on a multi-zone Rack or the `Zones` tab, otherwise a brand-new Sampler Rack.
    pub(crate) fn default_record_target(&self) -> BounceTarget {
        self.default_bounce_target().unwrap_or_default()
    }

    /// Open the pre-filled `WorkflowKind::Record` dialog (`t A`). Pressing `Enter` immediately
    /// arms/starts with smart defaults (`t A → Enter`).
    pub(crate) fn open_record_dialog(&mut self) -> Result<Outcome> {
        let target = self.default_record_target();
        let default_bars = self
            .playing_loop()
            .filter(|(s, e)| e > s)
            .map(|(s, e)| (e - s).div_ceil(16) as usize);
        self.clear_entry_state();
        let origin = Some(self.navigation());
        let ctx = self.workflow_context();
        let draft = drafts::WorkflowDraft::record(
            self.doc.project(),
            ctx,
            RecordingMode::Quantized,
            default_bars,
            target,
            &self.assets.presets,
        );
        self.modal = ActiveModal::Workflow {
            draft: Box::new(draft),
            origin,
        };
        self.input.mode = Mode::Workflow;
        self.status =
            "Record Audio · Enter starts now · m mode · b bars · t target · n name · p arrange · Esc cancel"
                .into();
        Ok(self.release_held())
    }

    /// Direct toggle (`t a` / `:rec`):
    /// - `Idle` → arm/start immediately with smart defaults
    /// - `Armed` → disarm cleanly
    /// - `Recording` → stop & finalize into a 24-bit WAV
    pub(crate) fn toggle_audio_recording(&mut self) -> Result<Outcome> {
        match self.recording.phase {
            CapturePhase::Idle => {
                if self.config.audio_input_device.is_none() {
                    self.status =
                        "No audio input selected · open Devices · Audio (Ctrl+d) to choose an input"
                            .into();
                    return Ok(Outcome::default());
                }
                self.start_audio_recording(StartRecordingOpts::default())
            }
            CapturePhase::Armed => self.cancel_audio_recording(),
            CapturePhase::Recording => self.stop_audio_recording(),
            CapturePhase::Finishing => {
                self.status = "Finalizing audio recording…".into();
                Ok(Outcome::default())
            }
        }
    }

    /// Arm or start live audio input recording with explicit or smart-default options.
    pub(crate) fn start_audio_recording(&mut self, opts: StartRecordingOpts) -> Result<Outcome> {
        ensure!(
            self.recording.phase == CapturePhase::Idle && self.recording.active_request.is_none(),
            "Audio recording is already {} · stop or cancel it first",
            self.recording.phase.label()
        );
        ensure!(
            self.config.audio_input_device.is_some(),
            "No audio input selected · open Devices · Audio (Ctrl+d) or run :rec input <DEVICE>"
        );
        if let Some(bars) = opts.bars {
            ensure!(
                (1..=64).contains(&bars),
                "Recording length must be between 1 and 64 bars"
            );
        }

        let target = opts.target.unwrap_or_else(|| self.default_record_target());
        self.validate_bounce_target(&target)?;

        let mode = opts.mode.unwrap_or(RecordingMode::Quantized);
        let place_in_arrange = opts.place_in_arrange.unwrap_or_else(|| {
            matches!(target, BounceTarget::NewSampler) && mode != RecordingMode::Free
        });

        let mut out = Outcome::default();
        let (punch_in_abs_step, punch_in_song_step, punch_out_abs_step, quantized) = match mode {
            RecordingMode::Free => {
                let song_step = self.arrange_ui.step;
                (self.transport.next_step, song_step, None, false)
            }
            RecordingMode::Quantized | RecordingMode::CountIn => {
                let count_in_steps: u64 = if mode == RecordingMode::CountIn {
                    16
                } else {
                    0
                };
                let loop_region = self.playing_loop().filter(|(s, e)| e > s);
                if self.transport.playing {
                    let punch_in =
                        if let Some((ls, _)) = loop_region.filter(|_| opts.bars.is_none()) {
                            self.next_abs_step_at_song_step(u64::from(ls)) + count_in_steps
                        } else {
                            self.live
                                .boundary(self.doc.project(), self.doc.project().timing.launch)?
                                + count_in_steps
                        };
                    let punch_in_song = self
                        .song_position_opt(punch_in)
                        .unwrap_or(u64::from(self.arrange_ui.step))
                        as u32;
                    let punch_out = if let Some(bars) = opts.bars {
                        Some(punch_in + bars as u64 * 16)
                    } else {
                        loop_region.map(|(ls, le)| punch_in + u64::from(le - ls))
                    };
                    (punch_in, punch_in_song, punch_out, true)
                } else {
                    // Stopped: cue to loop start or snapped cursor bar while preserving any active
                    // loop region, start transport, and punch in cleanly (with optional 1-bar count-in).
                    let preserve_region =
                        loop_region.map(|(ls, le)| (u64::from(ls), u64::from(le - ls)));
                    let target_song_step = if let Some((ls, _)) = loop_region {
                        ls
                    } else {
                        (self.arrange_ui.step / 16) * 16
                    };
                    let cue_step = if count_in_steps > 0 {
                        if target_song_step >= 16 {
                            // Lead in from the bar preceding loop start / cursor bar; `Segment`
                            // plays linearly until `ls` and then enters the preserved loop cleanly.
                            target_song_step - 16
                        } else if let Some((ls, le)) = loop_region {
                            // Loop starts on bar 1: cue 1 bar before loop end so the count-in wraps
                            // right onto `ls` at `start_abs + 16`.
                            if le - ls >= 16 {
                                le - 16
                            } else {
                                ls
                            }
                        } else {
                            target_song_step
                        }
                    } else {
                        target_song_step
                    };
                    let cue_out =
                        self.cue_song_with_region(cue_step as usize, true, preserve_region)?;
                    out.merge(cue_out);
                    let start_abs = self.transport.next_step;
                    let punch_in = start_abs + count_in_steps;
                    let punch_in_song =
                        self.song_position_opt(punch_in)
                            .unwrap_or(u64::from(target_song_step)) as u32;
                    let punch_out = if let Some(bars) = opts.bars {
                        Some(punch_in + bars as u64 * 16)
                    } else {
                        loop_region.map(|(ls, le)| punch_in + u64::from(le - ls))
                    };
                    (punch_in, punch_in_song, punch_out, true)
                }
            }
        };

        let requested_name = opts
            .name
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Audio Rec".into());
        let (instrument_name, file_slug_base) = match &target {
            BounceTarget::NewSampler => {
                let unique = self.unique_named_entity(&requested_name, "Audio Rec");
                (unique.clone(), unique)
            }
            BounceTarget::RackZone {
                instrument,
                zone_index,
                midi,
                zone_name,
            } => {
                let inst = self.instrument(*instrument).context("Instrument missing")?;
                let zlabel = match *zone_index {
                    Some(idx) => inst
                        .rack
                        .zones
                        .get(idx)
                        .map_or_else(|| format!("z{}", idx + 1), |z| z.name.clone()),
                    None => zone_name
                        .clone()
                        .unwrap_or_else(|| note_name(midi.unwrap_or(60))),
                };
                (
                    format!("{} [{}]", inst.name, zlabel),
                    format!("{}-{}-{}", inst.name, zlabel, requested_name),
                )
            }
        };
        let (relative_path, output_path) =
            self.unique_sample_paths("samples/recordings", &file_slug_base, "rec");

        let inv = &self.devices.inventory;
        let in_sr = inv
            .active_input_sample_rate
            .max(self.recording.input_sample_rate)
            .max(self.effective_audio_sample_rate(0));
        let in_ms = if inv.active_input_buffer_frames > 0 && in_sr > 0 {
            inv.active_input_buffer_frames as f32 * 1000.0 / in_sr as f32
        } else {
            0.0
        };
        let out_sr = self.effective_audio_sample_rate(0);
        let out_ms = if inv.active_buffer_frames > 0 && out_sr > 0 {
            inv.active_buffer_frames as f32 * 1000.0 / out_sr as f32
        } else {
            0.0
        };
        let total_lat_ms = (in_ms + out_ms + self.config.audio_input_latency_ms).max(0.0);
        let tail_frames =
            ((f64::from(total_lat_ms) * f64::from(in_sr) / 1000.0).round() as u32).min(in_sr);

        let arm_spec = CaptureArmSpec {
            quantized,
            punch_in_step: punch_in_abs_step,
            punch_out_step: punch_out_abs_step,
            tail_frames,
        };
        let request = RecordingRequest {
            mode,
            target,
            place_in_arrange,
            bars: opts.bars,
            instrument_name: instrument_name.clone(),
            relative_path: relative_path.clone(),
            output_path,
            punch_in_abs_step,
            punch_in_song_step,
            punch_out_abs_step,
            source_lane: self.selected_lane,
            latency_trim_ms: self.config.audio_input_latency_ms,
            arm_spec,
        };

        self.recording.phase = if quantized {
            CapturePhase::Armed
        } else {
            CapturePhase::Recording
        };
        self.recording.captured_frames = 0;
        self.recording.dropped_frames = 0;
        self.recording.live_peaks.clear();
        self.recording.active_request = Some(request.clone());

        self.status = match mode {
            RecordingMode::Free => {
                format!("● Recording {instrument_name} (free) · t a or :rec stop finishes")
            }
            RecordingMode::CountIn => format!(
                "○ Count-in armed for {instrument_name} · punches in at {} · t a cancels",
                position_label(punch_in_abs_step)
            ),
            RecordingMode::Quantized => {
                if let Some(end) = punch_out_abs_step {
                    let bars = (end - punch_in_abs_step).div_ceil(16).max(1);
                    format!(
                        "○ Armed {instrument_name} ({bars} bar{}) · punches in at {} · t a cancels",
                        if bars == 1 { "" } else { "s" },
                        position_label(punch_in_abs_step)
                    )
                } else {
                    format!(
                        "○ Armed {instrument_name} · punches in at {} · t a cancels",
                        position_label(punch_in_abs_step)
                    )
                }
            }
        };

        out.recording_command = Some(RecordingHostCommand::Arm {
            request: Box::new(request),
        });
        Ok(out)
    }

    /// Stop an active recording cleanly (finalizing into WAV) or disarm if armed.
    pub(crate) fn stop_audio_recording(&mut self) -> Result<Outcome> {
        match self.recording.phase {
            CapturePhase::Idle => {
                self.status = "No audio recording is active".into();
                Ok(Outcome::default())
            }
            CapturePhase::Armed => self.cancel_audio_recording(),
            CapturePhase::Recording | CapturePhase::Finishing => {
                self.recording.phase = CapturePhase::Finishing;
                self.status = "Finalizing audio recording…".into();
                Ok(Outcome {
                    recording_command: Some(RecordingHostCommand::Stop),
                    ..Outcome::default()
                })
            }
        }
    }

    /// Cancel an armed or active recording immediately without writing a WAV.
    pub(crate) fn cancel_audio_recording(&mut self) -> Result<Outcome> {
        let was_active = self.recording.is_active();
        self.recording.phase = CapturePhase::Idle;
        self.recording.active_request = None;
        self.recording.captured_frames = 0;
        self.recording.dropped_frames = 0;
        self.recording.live_peaks.clear();
        self.status = if was_active {
            "Audio recording cancelled".into()
        } else {
            "No audio recording to cancel".into()
        };
        Ok(Outcome {
            recording_command: was_active.then_some(RecordingHostCommand::Cancel),
            ..Outcome::default()
        })
    }

    /// Hook called by `apply_stop` and `apply_play(false)` so stopping transport cleanly
    /// finalizes an active recording (or disarms an armed quantized recording).
    pub(crate) fn handle_recording_on_transport_stop(&mut self, out: &mut Outcome) {
        let Some(req) = &self.recording.active_request else {
            return;
        };
        if req.mode == RecordingMode::Free {
            return;
        }
        match self.recording.phase {
            CapturePhase::Armed => {
                self.recording.phase = CapturePhase::Idle;
                self.recording.active_request = None;
                self.recording.live_peaks.clear();
                out.recording_command = Some(RecordingHostCommand::Cancel);
            }
            CapturePhase::Recording => {
                self.recording.phase = CapturePhase::Finishing;
                out.recording_command = Some(RecordingHostCommand::Stop);
            }
            CapturePhase::Idle | CapturePhase::Finishing => {}
        }
    }

    /// Complete a finalized 24-bit WAV recording: incrementally register the file in
    /// `SampleLibrary`, load into `SampleBank`, land into a `Sampler` Rack (+ optional
    /// arranged `Placement`) or target `RackZone` in ONE atomic undoable commit.
    pub fn complete_recording(&mut self, completed: CompletedRecording) -> Result<Outcome> {
        self.recording.phase = CapturePhase::Idle;
        self.recording.active_request = None;
        self.recording.captured_frames = 0;
        self.recording.dropped_frames = 0;
        self.recording.live_peaks.clear();

        let req = &completed.request;
        let step_secs = self.doc.compiled().step_seconds().max(0.01);
        let recorded_steps = if let Some(end) = req.punch_out_abs_step {
            end.saturating_sub(req.punch_in_abs_step).max(1)
        } else if completed.actual_end_abs_step > completed.actual_start_abs_step {
            completed
                .actual_end_abs_step
                .saturating_sub(completed.actual_start_abs_step)
                .max(1)
        } else {
            ((completed.duration_sec / step_secs).ceil() as u64).max(1)
        }
        .clamp(1, material::MAX_LITERAL_STEPS as u64);

        let peak_db = completed.peak_left_db.max(completed.peak_right_db);
        let all_zero = !(peak_db.is_finite() && peak_db > -120.0);
        let peak_str = if !all_zero {
            format!("{peak_db:+.1} dBFS")
        } else {
            "-∞ dBFS".into()
        };
        let mut warnings = Vec::new();
        if all_zero {
            if cfg!(target_os = "macos") {
                warnings.push("all zeros — check Privacy & Security → Microphone".into());
            } else {
                warnings.push("all zeros — check OS Microphone permission".into());
            }
        }
        if completed.full_scale_samples > 0 {
            warnings.push(format!("{} clipped", completed.full_scale_samples));
        }
        if completed.dropped_frames > 0 {
            warnings.push(format!("{} dropped", completed.dropped_frames));
        }
        let warn_suffix = if warnings.is_empty() {
            String::new()
        } else {
            format!(" · !{}", warnings.join(", "))
        };
        let status_summary = match &req.target {
            BounceTarget::RackZone { .. } => format!(
                "Recorded → {} ({}, {:.2}s · {peak_str}{warn_suffix})",
                req.instrument_name, req.relative_path, completed.duration_sec
            ),
            BounceTarget::NewSampler => format!(
                "Recorded {} → {} ({:.2}s · {peak_str}{warn_suffix})",
                req.instrument_name, req.relative_path, completed.duration_sec
            ),
        };

        let place_at = (req.place_in_arrange && matches!(req.target, BounceTarget::NewSampler))
            .then_some(req.punch_in_song_step);

        self.land_recorded_sample(SampleLandingSpec {
            target: req.target.clone(),
            relative_path: req.relative_path.clone(),
            output_path: req.output_path.clone(),
            instrument_name: req.instrument_name.clone(),
            steps: recorded_steps,
            create_part: req.place_in_arrange,
            place_in_arrange_at: place_at,
            source_lane: req.source_lane,
            status_summary,
        })
    }

    /// Clear active recording state and report an actionable status if the host fails
    /// to finalize or write the recorded WAV.
    pub fn fail_recording(&mut self, message: impl Into<String>) {
        self.recording.phase = CapturePhase::Idle;
        self.recording.active_request = None;
        self.recording.captured_frames = 0;
        self.recording.dropped_frames = 0;
        self.recording.live_peaks.clear();
        self.status = format!("Audio recording failed: {}", message.into());
    }

    /// Sync live `RecordingState` with [`AudioSnapshot::input`] each frame.
    pub fn observe_audio_recording(&mut self, snapshot: &AudioSnapshot) {
        let inp = snapshot.input;
        if inp.sample_rate > 0 {
            self.recording.input_sample_rate = inp.sample_rate;
        }
        let failed = inp.status == AudioInputStatus::Failed;
        if failed && !self.recording.last_input_failed {
            let err = self
                .devices
                .inventory
                .input_last_error
                .as_deref()
                .unwrap_or("check microphone permissions or reconnect device");
            self.status = format!("Audio input error: {err} · Ctrl+d (Audio) to reconfigure");
        }
        self.recording.last_input_failed = failed;

        if self.recording.active_request.is_some() {
            let prev = self.recording.phase;
            self.recording.captured_frames = inp.captured_frames;
            self.recording.dropped_frames = inp.dropped_frames;
            if prev == CapturePhase::Armed && inp.capture_phase == CapturePhase::Recording {
                self.recording.phase = CapturePhase::Recording;
                if let Some(req) = &self.recording.active_request {
                    self.status =
                        format!("● Recording {}… · t a or Space stops", req.instrument_name);
                }
            } else if prev == CapturePhase::Recording
                && inp.capture_phase == CapturePhase::Finishing
            {
                self.recording.phase = CapturePhase::Finishing;
                self.status = "Finalizing audio recording…".into();
            }
        } else {
            self.recording.phase = CapturePhase::Idle;
        }
    }

    /// Cycle software input monitoring (`Off` → `Auto (Armed)` → `On` → `Off`) via `t i`.
    pub(crate) fn cycle_input_monitor(&mut self) -> Result<Outcome> {
        let next = self.config.audio_input_monitor.cycle(1);
        Ok(self.set_audio_input_monitor_choice(next))
    }

    /// Compute the next monotonic absolute step at or after `live.next_step` whose song
    /// position aligns with `target_song_step` (used for Loop Region punch-in).
    fn next_abs_step_at_song_step(&self, target_song_step: u64) -> u64 {
        let now = self.live.next_step;
        let seg = self.live.heard_segment();
        if let Some((_, loop_len)) = seg.region {
            if loop_len > 0 {
                let cur_song = self.song_position_opt(now).unwrap_or(0);
                if cur_song == target_song_step {
                    return now;
                }
                let delta = if target_song_step > cur_song {
                    target_song_step - cur_song
                } else {
                    loop_len.saturating_sub(cur_song.saturating_sub(target_song_step))
                };
                return now.saturating_add(delta);
            }
        }
        let song_len = (self.doc.project().arrangement_steps().max(1)) as u64;
        let cur_song = self.song_position_opt(now).unwrap_or(0);
        if cur_song == target_song_step {
            now
        } else if target_song_step > cur_song {
            now.saturating_add(target_song_step - cur_song)
        } else {
            now.saturating_add(song_len.saturating_sub(cur_song) + target_song_step)
        }
    }

    /// Execute `:rec` / `:record` Ex command.
    ///
    /// Forms:
    /// - `:rec` — smart toggle (arm/start or stop)
    /// - `:rec stop` — stop and finalize recording
    /// - `:rec cancel` — cancel without writing WAV
    /// - `:rec free [NAME]` — free-time recording without starting transport
    /// - `:rec countin [BARS] [NAME]` — 1-bar count-in recording
    /// - `:rec bars N [NAME]` — record fixed `N` bars
    /// - `:rec zone [INST] [ZONE|NOTE] [NAME]` — record into a Rack zone
    /// - `:rec monitor [off|auto|on]` — set/cycle software input monitoring
    /// - `:rec mode [stereo|left|right]` — set input channel routing
    /// - `:rec input [DEVICE|none]` — select or clear audio input device
    /// - `:rec latency <MS>` — set manual latency compensation trim (`-250..=250`)
    pub(super) fn record_command(&mut self, args: &[String]) -> Result<Outcome> {
        if args.is_empty() {
            return self.toggle_audio_recording();
        }
        let sub = args[0].to_ascii_lowercase();
        match sub.as_str() {
            "stop" | "finish" => self.stop_audio_recording(),
            "cancel" | "abort" | "disarm" => self.cancel_audio_recording(),
            "dialog" | "open" => self.open_record_dialog(),
            "start" | "arm" => {
                let name = (args.len() > 1).then(|| args[1..].join(" "));
                self.start_audio_recording(StartRecordingOpts {
                    name,
                    ..StartRecordingOpts::default()
                })
            }
            "free" => {
                let name = (args.len() > 1).then(|| args[1..].join(" "));
                self.start_audio_recording(StartRecordingOpts {
                    mode: Some(RecordingMode::Free),
                    place_in_arrange: Some(false),
                    name,
                    ..StartRecordingOpts::default()
                })
            }
            "countin" | "count-in" => {
                let (bars, name_slice) = match args.get(1).and_then(|s| s.parse::<usize>().ok()) {
                    Some(n) => (Some(n), &args[2..]),
                    None => (None, &args[1..]),
                };
                let name = (!name_slice.is_empty()).then(|| name_slice.join(" "));
                self.start_audio_recording(StartRecordingOpts {
                    mode: Some(RecordingMode::CountIn),
                    bars,
                    name,
                    ..StartRecordingOpts::default()
                })
            }
            "bars" => {
                let n = args
                    .get(1)
                    .context("Usage: :rec bars <1..=64> [NAME]")?
                    .parse::<usize>()
                    .context("Bar count must be a positive integer")?;
                let name = (args.len() > 2).then(|| args[2..].join(" "));
                self.start_audio_recording(StartRecordingOpts {
                    bars: Some(n),
                    name,
                    ..StartRecordingOpts::default()
                })
            }
            "zone" => {
                let target = self.resolve_sample_zone_target(&args[1..], "rec", "Rec")?;
                self.start_audio_recording(StartRecordingOpts {
                    target: Some(target),
                    place_in_arrange: Some(false),
                    ..StartRecordingOpts::default()
                })
            }
            "monitor" | "mon" => {
                if let Some(raw) = args.get(1) {
                    let mode = InputMonitorMode::parse(raw)
                        .context("Usage: :rec monitor [off|auto|on]")?;
                    Ok(self.set_audio_input_monitor_choice(mode))
                } else {
                    self.cycle_input_monitor()
                }
            }
            "mode" | "ch" | "channels" => {
                let raw = args
                    .get(1)
                    .context("Usage: :rec mode <stereo|left|right>")?;
                let mode = AudioInputMode::parse(raw)
                    .context("Input mode must be stereo, left (ch1), or right (ch2)")?;
                Ok(self.set_audio_input_mode_choice(mode))
            }
            "input" | "dev" | "device" => {
                if args.len() < 2 {
                    self.open_devices_deck(DevicesTab::Audio);
                    return Ok(Outcome::default());
                }
                let joined = args[1..].join(" ");
                let next =
                    if joined.eq_ignore_ascii_case("none") || joined.eq_ignore_ascii_case("off") {
                        None
                    } else {
                        Some(joined)
                    };
                Ok(self.set_audio_input_choice(next))
            }
            "latency" | "trim" => {
                let raw = args
                    .get(1)
                    .context("Usage: :rec latency <MS> (-250..=250)")?;
                let ms = crate::commands::parse_ms(raw)?;
                ensure!(
                    (MIN_AUDIO_LATENCY_TRIM_MS..=MAX_AUDIO_LATENCY_TRIM_MS).contains(&ms),
                    "Latency trim must be {MIN_AUDIO_LATENCY_TRIM_MS}..={MAX_AUDIO_LATENCY_TRIM_MS} ms"
                );
                Ok(self.set_audio_latency_trim_ms(ms))
            }
            _ => {
                let name = Some(args.join(" "));
                self.start_audio_recording(StartRecordingOpts {
                    name,
                    ..StartRecordingOpts::default()
                })
            }
        }
    }
}
