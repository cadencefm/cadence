//! Off-RT host audio capture drainer, round-trip latency compensation, and
//! atomic 24-bit WAV finalization bridging `AudioRuntime` and `App`.
use anyhow::Result;
use cadence_app::{
    App, CompletedRecording, Outcome, RecordingHostCommand, RecordingMode, RecordingRequest,
};
use cadence_core::dsp::LivePeakRecorder;
use cadence_core::runtime::{AudioInputStatus, CapturePhase};
use cadence_engine::audio::{finalize_recording_wav, AudioRuntime};
use std::time::{Duration, Instant};

/// Upper bound on in-memory recording capture length (15 minutes at any supported sample rate).
const MAX_RECORDING_SECONDS: usize = 15 * 60;
/// Maximum time to wait in `CapturePhase::Finishing` for post-punch-out tail frames
/// before force-sealing (guards against disconnected or silent input streams).
const FINISHING_TIMEOUT: Duration = Duration::from_millis(450);
const LIVE_PEAK_BINS: usize = 512;
const INITIAL_PEAK_FRAMES_PER_BIN: usize = 128;

struct ActiveRecording {
    request: RecordingRequest,
    frames: Vec<[f32; 2]>,
    peak_tracker: LivePeakRecorder,
    dropped_frames: u64,
    sample_rate: u32,
    seen_recording: bool,
    finishing_since: Option<Instant>,
    actual_start_abs_step: u64,
    actual_end_abs_step: u64,
}

/// Host-side session recorder draining the lock-free SPSC capture ring off-RT
/// and finalizing recorded audio into portable 24-bit WAV clips.
#[derive(Default)]
pub struct HostRecorder {
    active: Option<ActiveRecording>,
}

impl HostRecorder {
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// Execute one [`RecordingHostCommand`] emitted by `cadence-app` against `AudioRuntime`.
    pub fn handle_command(
        &mut self,
        cmd: &RecordingHostCommand,
        audio: &mut AudioRuntime,
    ) -> Result<()> {
        match cmd {
            RecordingHostCommand::Arm { request } => {
                let req = (**request).clone();
                let tel = audio.input_telemetry();
                let sr = if tel.sample_rate > 0 {
                    tel.sample_rate
                } else {
                    audio.sample_rate()
                };
                audio.arm_capture(req.arm_spec)?;
                let already_recording = !req.arm_spec.quantized;
                self.active = Some(ActiveRecording {
                    actual_start_abs_step: req.punch_in_abs_step,
                    actual_end_abs_step: req.punch_out_abs_step.unwrap_or(0),
                    request: req,
                    frames: Vec::with_capacity(sr as usize * 8),
                    peak_tracker: LivePeakRecorder::new(
                        LIVE_PEAK_BINS,
                        INITIAL_PEAK_FRAMES_PER_BIN,
                    ),
                    dropped_frames: 0,
                    sample_rate: sr,
                    seen_recording: already_recording,
                    finishing_since: None,
                });
                Ok(())
            }
            RecordingHostCommand::Stop => {
                let next_phase = audio.stop_capture();
                if let Some(rec) = self.active.as_mut() {
                    if next_phase == CapturePhase::Idle && !rec.seen_recording {
                        // Disarmed before punch-in ever triggered.
                        self.active = None;
                    } else if rec.finishing_since.is_none() {
                        rec.finishing_since = Some(Instant::now());
                    }
                }
                Ok(())
            }
            RecordingHostCommand::Cancel => {
                audio.cancel_capture();
                self.active = None;
                Ok(())
            }
        }
    }

    /// Drain newly captured audio frames and, once capture seals, finalize the 24-bit WAV
    /// and complete the transaction inside `app`.
    ///
    /// Returns `(mutated, follow_up_outcome)` so the caller can dispatch `follow_up_outcome`
    /// through `apply_outcome_with_hooks` (publishing the new `ScoreWindow` & `SampleBank`).
    pub fn poll(&mut self, app: &mut App, audio: &mut AudioRuntime) -> (bool, Option<Outcome>) {
        let Some(rec) = self.active.as_mut() else {
            return (false, None);
        };

        let snap = audio.snapshot();
        let inp = snap.input;
        if inp.sample_rate > 0 {
            rec.sample_rate = inp.sample_rate;
        }
        let sr = rec.sample_rate.max(8_000);
        let max_frames = (sr as usize).saturating_mul(MAX_RECORDING_SECONDS);
        let prev_len = rec.frames.len();
        let remaining_cap = max_frames.saturating_sub(prev_len);
        let (drained, dropped) = if remaining_cap > 0 {
            audio.drain_capture_frames(&mut rec.frames, remaining_cap)
        } else {
            (0, 0)
        };
        rec.dropped_frames = rec.dropped_frames.saturating_add(dropped);
        if drained > 0 {
            rec.peak_tracker.push_stereo(&rec.frames[prev_len..]);
            app.set_recording_preview_peaks(rec.peak_tracker.snapshot_peaks());
        }
        let mut mutated = drained > 0;

        if matches!(
            inp.capture_phase,
            CapturePhase::Recording | CapturePhase::Finishing
        ) || drained > 0
            || !rec.frames.is_empty()
        {
            rec.seen_recording = true;
            if inp.capture_start_step > 0 {
                rec.actual_start_abs_step = inp.capture_start_step;
            }
            if inp.capture_end_step > 0 {
                rec.actual_end_abs_step = inp.capture_end_step;
            }
        }

        // Enforce bounded memory ceiling cleanly.
        if rec.frames.len() >= max_frames
            && matches!(
                inp.capture_phase,
                CapturePhase::Recording | CapturePhase::Finishing
            )
        {
            audio.force_seal_capture();
        }

        // Guard Finishing phase against disconnected or non-hardware silent input streams.
        let phase_now = if inp.capture_phase == CapturePhase::Finishing {
            let started = *rec.finishing_since.get_or_insert_with(Instant::now);
            if inp.status != AudioInputStatus::Running || started.elapsed() >= FINISHING_TIMEOUT {
                audio.force_seal_capture();
                CapturePhase::Idle
            } else {
                CapturePhase::Finishing
            }
        } else {
            inp.capture_phase
        };

        let should_finalize = phase_now == CapturePhase::Idle
            && (rec.seen_recording || rec.finishing_since.is_some());
        if !should_finalize {
            return (mutated, None);
        }

        // Final ring drain before sealing so zero tail frames are lost.
        let rem = max_frames.saturating_sub(rec.frames.len());
        if rem > 0 {
            let (_, more_dropped) = audio.drain_capture_frames(&mut rec.frames, rem);
            rec.dropped_frames = rec.dropped_frames.saturating_add(more_dropped);
        }
        let Some(finished) = self.active.take() else {
            return (mutated, None);
        };
        let final_snap = audio.snapshot();
        let final_inp = final_snap.input;
        let final_sr = finished.sample_rate.max(8_000);
        let start_step = if final_inp.capture_start_step > 0 {
            final_inp.capture_start_step
        } else {
            finished.actual_start_abs_step
        };
        let end_step = if final_inp.capture_end_step > 0 {
            final_inp.capture_end_step
        } else {
            finished.actual_end_abs_step
        };

        if finished.frames.is_empty() {
            app.fail_recording("captured zero audio frames (check Audio Input in Ctrl+d)");
            return (true, None);
        }

        let trim_frames = if finished.request.mode == RecordingMode::Free {
            0
        } else {
            final_inp.round_trip_latency_frames(
                final_snap.health,
                final_snap.sample_rate,
                finished.request.latency_trim_ms,
            )
        };

        match finalize_recording_wav(
            &finished.frames,
            final_sr,
            trim_frames,
            &finished.request.output_path,
        ) {
            Ok(stats) => {
                let completed = CompletedRecording {
                    request: finished.request,
                    frames: stats.frames,
                    sample_rate: stats.sample_rate,
                    duration_sec: stats.duration_sec,
                    peak_left_db: stats.peak_left_db,
                    peak_right_db: stats.peak_right_db,
                    full_scale_samples: stats.full_scale_samples,
                    dropped_frames: finished.dropped_frames,
                    actual_start_abs_step: start_step,
                    actual_end_abs_step: end_step,
                };
                match app.complete_recording(completed) {
                    Ok(follow_up) => {
                        mutated = true;
                        (mutated, Some(follow_up))
                    }
                    Err(err) => {
                        app.fail_recording(format!("{err:#}"));
                        (true, None)
                    }
                }
            }
            Err(err) => {
                app.fail_recording(format!("{err:#}"));
                (true, None)
            }
        }
    }
}
