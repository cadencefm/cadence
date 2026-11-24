//! Unified JSONL host protocol SPOT shared by `--script`, `--stdio`, and live GUI `--ctl`.
//!
//! When connected to a live [`AudioRuntime`] (`--stdio`), a reader thread feeds
//! incoming lines over a channel so the host ticks `observe_transport` every
//! ~16 ms while idle, landing pending changes and extending the ring window
//! automatically without waiting for another stdin line.
use super::{apply_outcome_with_hooks, HeadlessOutcomeHooks, OutcomeHooks};
use anyhow::{bail, ensure, Context, Result};
use cadence_app::{Action, App};
use cadence_core::dsp::{lin_to_db_f64, resample_stereo};
use cadence_core::mixer::{ChannelRef, LiveFlags};
use cadence_core::runtime::AudioSnapshot;
use cadence_engine::audio::{
    render_schedule_f32, write_stereo_wav_16bit, AudioRuntime, AudioTapHandle,
};
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, Instant};

const IDLE_TICK: Duration = Duration::from_millis(16);
pub const DEFAULT_LISTEN_SAMPLE_RATE: u32 = 24_000;
const MIN_LIVE_LISTEN_SECS: f64 = 0.25;
const MAX_LIVE_LISTEN_SECS: f64 = 30.0;
const MAX_LISTEN_BARS: u32 = 256;

fn default_listen_sample_rate() -> u32 {
    DEFAULT_LISTEN_SAMPLE_RATE
}

/// Listen capture request: either deterministic offline bar-range rendering from
/// live in-memory state (`Bars`), or real-time lock-free speaker tap capture (`Live`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ListenRequest {
    /// Render exact 1-based half-open `[start_bar..end_bar)` window from the live
    /// in-memory `Project` + `Flags` (applying temporary `solo` without mutating GUI state).
    Bars {
        start_bar: u32,
        end_bar: u32,
        #[serde(default)]
        solo: Vec<String>,
        #[serde(default = "default_listen_sample_rate")]
        sample_rate: u32,
        #[serde(default)]
        out_path: Option<PathBuf>,
    },
    /// Capture live speaker output via `AudioRuntime`'s lock-free SPSC stereo tap ring.
    Live {
        seconds: f64,
        #[serde(default = "default_listen_sample_rate")]
        sample_rate: u32,
        #[serde(default)]
        out_path: Option<PathBuf>,
    },
}

/// One unified request envelope shared by `--script`, `--stdio`, and `--ctl`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
    #[serde(default)]
    pub compact: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<Action>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmd: Option<String>,
    #[serde(default)]
    pub meters: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen: Option<ListenRequest>,
}

/// Execution context passed into [`execute_host_request`].
pub struct HostRequestContext<'a> {
    pub audio: Option<&'a mut AudioRuntime>,
    pub default_ear_path: PathBuf,
    pub live_tap_busy: Option<&'a Arc<AtomicBool>>,
}

/// Result of dispatching a single [`HostRequest`].
pub enum HostRequestOutcome {
    Immediate {
        response: serde_json::Value,
        mutated: bool,
        quit: bool,
        failed: bool,
    },
    /// Non-blocking live speaker tap job ready to run on a background thread (GUI `--ctl`)
    /// or synchronously (`--stdio`).
    DeferredLiveListen(LiveListenJob),
}

struct TapBusyGuard {
    tap: AudioTapHandle,
    busy: Option<Arc<AtomicBool>>,
}

impl Drop for TapBusyGuard {
    fn drop(&mut self) {
        self.tap.stop();
        if let Some(busy) = &self.busy {
            busy.store(false, Ordering::Release);
        }
    }
}

/// Self-contained off-GUI live audio tap capture job.
///
/// Owns its [`TapBusyGuard`] from construction so dropping the job without
/// running [`execute`](Self::execute) (for example, if `thread::Builder::spawn`
/// fails) unconditionally stops the tap and releases `live_tap_busy`.
pub struct LiveListenJob {
    id: Option<serde_json::Value>,
    revision: u64,
    seconds: f64,
    sample_rate: u32,
    out_path: PathBuf,
    guard: TapBusyGuard,
}

impl LiveListenJob {
    pub fn request_id(&self) -> Option<&serde_json::Value> {
        self.id.as_ref()
    }

    /// Collect live speaker output from the lock-free tap ring, resample cleanly to
    /// `sample_rate`, write stereo 16-bit WAV atomically, and return structured telemetry.
    pub fn execute(self) -> (serde_json::Value, bool) {
        let id = self.id.clone();
        let revision = self.revision;
        let _guard = self.guard;
        let tap = &_guard.tap;
        let result: Result<serde_json::Value> = (|| {
            let src_sr = tap.sample_rate();
            let target_src_frames = ((self.seconds * f64::from(src_sr)).round() as usize).max(1);
            let mut captured: Vec<[f32; 2]> = Vec::with_capacity(target_src_frames + 1024);
            let mut dropped_frames = 0u64;
            tap.start();
            let deadline = Instant::now() + Duration::from_secs_f64(self.seconds + 3.0);
            while captured.len() < target_src_frames && Instant::now() < deadline {
                dropped_frames = dropped_frames.saturating_add(tap.drain(&mut captured));
                if !tap.is_alive() {
                    break;
                }
                if captured.len() < target_src_frames {
                    std::thread::sleep(Duration::from_millis(12));
                }
            }
            tap.stop();
            dropped_frames = dropped_frames.saturating_add(tap.drain(&mut captured));
            if captured.len() > target_src_frames {
                captured.truncate(target_src_frames);
            }
            ensure!(
                !captured.is_empty(),
                "Live audio tap captured zero frames (audio engine may be stopped or unavailable)"
            );
            let mut raw_peak = 0.0_f64;
            let mut raw_sq = 0.0_f64;
            let mut raw_clipped = false;
            for &[l, r] in &captured {
                if !l.is_finite() || !r.is_finite() || l.abs() > 1.0 || r.abs() > 1.0 {
                    raw_clipped = true;
                }
                let lf = f64::from(cadence_core::dsp::sanitize_sample(l));
                let rf = f64::from(cadence_core::dsp::sanitize_sample(r));
                raw_peak = raw_peak.max(lf.abs()).max(rf.abs());
                raw_sq += 0.5 * (lf * lf + rf * rf);
            }
            let resampled = resample_stereo(&captured, src_sr, self.sample_rate);
            let wav_report =
                write_stereo_wav_16bit(&self.out_path, self.sample_rate, &resampled, true)?;
            let rms = (raw_sq / captured.len().max(1) as f64).sqrt();
            let duration_sec = wav_report.frames as f64 / f64::from(self.sample_rate);
            let clipped = raw_clipped || wav_report.full_scale_samples > 0;
            Ok(serde_json::json!({
                "id": id,
                "ok": true,
                "revision": revision,
                "listen": {
                    "mode": "live",
                    "path": self.out_path.display().to_string(),
                    "sample_rate": self.sample_rate,
                    "source_sample_rate": src_sr,
                    "frames": wav_report.frames,
                    "duration_sec": duration_sec,
                    "peak_db": lin_to_db_f64(raw_peak),
                    "rms_db": lin_to_db_f64(rms),
                    "clipped": clipped,
                    "full_scale_samples": wav_report.full_scale_samples,
                    "dropped_frames": dropped_frames,
                }
            }))
        })();
        match result {
            Ok(resp) => (resp, false),
            Err(err) => (
                serde_json::json!({
                    "id": self.id,
                    "ok": false,
                    "revision": self.revision,
                    "error": format!("{err:#}"),
                }),
                true,
            ),
        }
    }
}

fn channel_meters_json(app: &App, snapshot: &AudioSnapshot) -> Vec<serde_json::Value> {
    snapshot
        .channel_meters
        .iter()
        .map(|m| {
            let name = match m.channel {
                ChannelRef::Master => "Master",
                ChannelRef::Channel(id) => app
                    .project()
                    .mixer
                    .channels
                    .iter()
                    .find(|c| c.id == id)
                    .map_or("Channel", |c| c.name.as_str()),
            };
            serde_json::json!({
                "channel": m.channel,
                "name": name,
                "input_peak": m.input_peak,
                "input_peak_hold": m.input_peak_hold,
                "output_peak": m.output_peak,
                "output_peak_hold": m.output_peak_hold,
                "hot": m.hot,
                "clipped": m.clipped,
            })
        })
        .collect()
}

fn effect_meters_json(snapshot: &AudioSnapshot) -> Vec<serde_json::Value> {
    let effect_count = snapshot
        .effect_meter_count
        .min(snapshot.effect_meters.len());
    snapshot.effect_meters[..effect_count]
        .iter()
        .map(|e| {
            serde_json::json!({
                "device_id": e.device_id,
                "gain_reduction_db": e.gain_reduction_db,
                "gr_hold_db": e.gr_hold_db,
            })
        })
        .collect()
}

fn audio_health_json(snapshot: &AudioSnapshot, audio: Option<&AudioRuntime>) -> serde_json::Value {
    let h = snapshot.health;
    serde_json::json!({
        "status": h.status,
        "cpu_load": h.cpu_load,
        "peak_cpu_load": h.peak_cpu_load,
        "xrun_count": h.xrun_count,
        "sample_rate": snapshot.sample_rate,
        "buffer_frames": h.buffer_frames,
        "buffer_latency_ms": h.buffer_latency_ms(snapshot.sample_rate),
        "channels": h.channels,
        "bit_depth": h.bit_depth,
        "format": h.format_label(),
        "overloaded": h.overloaded,
        "failed": h.failed,
        "error_code": h.error_code,
        "description": audio.map(AudioRuntime::description),
        "last_error": audio.and_then(AudioRuntime::last_error),
    })
}

fn audio_input_json(snapshot: &AudioSnapshot) -> serde_json::Value {
    let inp = snapshot.input;
    serde_json::json!({
        "status": inp.status,
        "sample_rate": inp.sample_rate,
        "channels": inp.channels,
        "buffer_frames": inp.buffer_frames,
        "peak": inp.peak,
        "peak_hold": inp.peak_hold,
        "clipped": inp.clipped,
        "digital_silence": inp.digital_silence,
        "mode": inp.mode.slug(),
        "monitor_mode": inp.monitor_mode.slug(),
        "monitoring": inp.monitoring,
        "capture_phase": inp.capture_phase,
        "captured_frames": inp.captured_frames,
        "dropped_frames": inp.dropped_frames,
    })
}

fn transport_json(
    app: &App,
    snapshot: &AudioSnapshot,
    audio: Option<&AudioRuntime>,
) -> serde_json::Value {
    serde_json::json!({
        "channel_meters": channel_meters_json(app, snapshot),
        "effect_meters": effect_meters_json(snapshot),
        "health": audio_health_json(snapshot, audio),
        "audio_input": audio_input_json(snapshot),
        "playing": snapshot.playing,
        "absolute_step": snapshot.absolute_step,
        "pending": snapshot.pending,
        "next_step": snapshot.next_step,
    })
}

fn apply_temporary_solo_flags(app: &App, solo: &[String]) -> Result<LiveFlags> {
    let mut flags = app.flags().clone();
    if solo.is_empty() {
        return Ok(flags);
    }
    let project = app.project();
    flags.solo_channels.clear();
    flags.solo_lanes.clear();
    for raw in solo {
        let token = raw.trim();
        ensure!(!token.is_empty(), "Empty --solo channel/lane specifier");
        let stripped = token.strip_prefix("channel:").unwrap_or(token);
        let parsed_id = stripped.parse::<u64>().ok();

        // 1. Match Mixer channel (ID or case-insensitive name).
        if let Some(ch) = project
            .mixer
            .channels
            .iter()
            .find(|c| parsed_id == Some(c.id) || c.name.eq_ignore_ascii_case(token))
        {
            flags.solo_channels.insert(ch.id);
            flags.muted_channels.remove(&ch.id);
            continue;
        }
        // 2. Match Arrangement lane (ID or case-insensitive name).
        if let Some(lane) = project
            .lanes
            .iter()
            .find(|l| parsed_id == Some(l.id) || l.name.eq_ignore_ascii_case(token))
        {
            flags.solo_lanes.insert(lane.id);
            flags.muted_lanes.remove(&lane.id);
            continue;
        }
        // 3. Match Instrument name -> solo its routed channel.
        if let Some(inst) = project
            .instruments
            .iter()
            .find(|i| i.name.eq_ignore_ascii_case(token))
        {
            flags.solo_channels.insert(inst.channel);
            flags.muted_channels.remove(&inst.channel);
            continue;
        }
        let channels: Vec<&str> = project
            .mixer
            .channels
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        let lanes: Vec<&str> = project.lanes.iter().map(|l| l.name.as_str()).collect();
        bail!(
            "Unknown solo target '{token}' (channels: [{}], lanes: [{}])",
            channels.join(", "),
            lanes.join(", ")
        );
    }
    Ok(flags)
}

fn execute_listen_bars(
    app: &App,
    id: Option<serde_json::Value>,
    start_bar: u32,
    end_bar: u32,
    solo: &[String],
    sample_rate: u32,
    out_path: PathBuf,
) -> Result<serde_json::Value> {
    ensure!(
        start_bar >= 1 && end_bar > start_bar,
        "Invalid bar range {start_bar}..{end_bar}: use 1-based half-open [start_bar..end_bar) (e.g. 1..5)"
    );
    let bar_count = end_bar - start_bar;
    ensure!(
        bar_count <= MAX_LISTEN_BARS,
        "Listen range {start_bar}..{end_bar} exceeds maximum {MAX_LISTEN_BARS} bars"
    );
    ensure!(
        (8_000..=192_000).contains(&sample_rate),
        "Listen sample_rate must be between 8000 and 192000 Hz"
    );

    let flags = apply_temporary_solo_flags(app, solo)?;
    let score = app.project().compile_with_flags(&flags)?;
    let start_step = u64::from(start_bar - 1) * 16;
    let steps = u64::from(bar_count) * 16;
    let (report, f32_frames) = render_schedule_f32(
        &[(0, &score)],
        start_step,
        steps,
        app.sample_bank(),
        sample_rate,
    )?;
    let wav_stats = write_stereo_wav_16bit(&out_path, sample_rate, &f32_frames, true)?;

    let total_frames = f32_frames.len();
    let mut per_bar = Vec::with_capacity(bar_count as usize);
    let mut any_bar_clipped = false;
    for idx in 0..bar_count as usize {
        let from = (idx * total_frames) / bar_count as usize;
        let to = ((idx + 1) * total_frames) / bar_count as usize;
        let slice = &f32_frames[from..to];
        let mut bar_peak = 0.0_f64;
        let mut bar_sq = 0.0_f64;
        let mut bar_clipped = false;
        for &[l, r] in slice {
            if !l.is_finite() || !r.is_finite() || l.abs() > 1.0 || r.abs() > 1.0 {
                bar_clipped = true;
            }
            let lf = f64::from(cadence_core::dsp::sanitize_sample(l));
            let rf = f64::from(cadence_core::dsp::sanitize_sample(r));
            bar_peak = bar_peak.max(lf.abs()).max(rf.abs());
            bar_sq += 0.5 * (lf * lf + rf * rf);
        }
        any_bar_clipped |= bar_clipped;
        let bar_rms = (bar_sq / slice.len().max(1) as f64).sqrt();
        per_bar.push(serde_json::json!({
            "bar": start_bar + idx as u32,
            "peak_db": lin_to_db_f64(bar_peak),
            "rms_db": lin_to_db_f64(bar_rms),
            "clipped": bar_clipped,
        }));
    }

    let overall_peak = report.output.peak[0].max(report.output.peak[1]);
    let overall_rms = ((report.output.rms[0].powi(2) + report.output.rms[1].powi(2)) * 0.5).sqrt();
    let clipped = any_bar_clipped || report.channels.iter().any(|c| c.clipped);
    let duration_sec = wav_stats.frames as f64 / f64::from(sample_rate);

    Ok(serde_json::json!({
        "id": id,
        "ok": true,
        "revision": app.revision(),
        "listen": {
            "mode": "bars",
            "path": out_path.display().to_string(),
            "start_bar": start_bar,
            "end_bar": end_bar,
            "solo": solo,
            "sample_rate": sample_rate,
            "frames": wav_stats.frames,
            "duration_sec": duration_sec,
            "clipped": clipped,
            "full_scale_samples": wav_stats.full_scale_samples,
            "peak": report.output.peak,
            "peak_db": lin_to_db_f64(overall_peak),
            "rms": report.output.rms,
            "rms_db": lin_to_db_f64(overall_rms),
            "bars": per_bar,
            "channels": report.channels,
            "effects": report.effects,
        }
    }))
}

fn error_response(
    id: Option<serde_json::Value>,
    revision: u64,
    msg: impl Into<String>,
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "ok": false,
        "revision": revision,
        "error": msg.into(),
    })
}

/// Dispatch a parsed [`HostRequest`] against `app` using ONE unified SPOT shared by
/// `--script`, `--stdio`, and live GUI `--ctl`.
pub fn execute_host_request(
    app: &mut App,
    request: HostRequest,
    ctx: &mut HostRequestContext<'_>,
    hooks: &mut dyn OutcomeHooks,
) -> HostRequestOutcome {
    let id = request.id.clone();

    let op_count = usize::from(request.action.is_some())
        + usize::from(request.cmd.is_some())
        + usize::from(request.meters)
        + usize::from(request.listen.is_some());
    if op_count == 0 {
        return HostRequestOutcome::Immediate {
            response: error_response(
                id,
                app.revision(),
                "Request must specify one of: action, cmd, meters, or listen",
            ),
            mutated: false,
            quit: false,
            failed: true,
        };
    }
    if op_count > 1 {
        return HostRequestOutcome::Immediate {
            response: error_response(
                id,
                app.revision(),
                "Request may specify only one of: action, cmd, meters, or listen",
            ),
            mutated: false,
            quit: false,
            failed: true,
        };
    }

    if let Some(expected) = request.expected_revision {
        if expected != app.revision() {
            return HostRequestOutcome::Immediate {
                response: error_response(
                    id,
                    app.revision(),
                    format!(
                        "Revision conflict: expected {expected}, current {}",
                        app.revision()
                    ),
                ),
                mutated: false,
                quit: false,
                failed: true,
            };
        }
    }

    if request.meters {
        let (playing, step, channel_meters, effect_meters, health, audio_input) =
            if let Some(audio) = ctx.audio.as_deref() {
                let snap = audio.snapshot();
                (
                    snap.playing,
                    snap.absolute_step,
                    channel_meters_json(app, &snap),
                    effect_meters_json(&snap),
                    audio_health_json(&snap, Some(audio)),
                    audio_input_json(&snap),
                )
            } else {
                let empty = AudioSnapshot::default();
                (
                    app.transport.playing,
                    app.transport.step,
                    Vec::new(),
                    Vec::new(),
                    audio_health_json(&empty, None),
                    audio_input_json(&empty),
                )
            };
        return HostRequestOutcome::Immediate {
            response: serde_json::json!({
                "id": id,
                "ok": true,
                "revision": app.revision(),
                "playing": playing,
                "step": step,
                "bpm": app.project().bpm,
                "status": app.status,
                "dirty": app.dirty(),
                "channel_meters": channel_meters,
                "effect_meters": effect_meters,
                "health": health,
                "audio_input": audio_input,
            }),
            mutated: false,
            quit: false,
            failed: false,
        };
    }

    if let Some(listen) = request.listen {
        return match listen {
            ListenRequest::Bars {
                start_bar,
                end_bar,
                solo,
                sample_rate,
                out_path,
            } => {
                let dest = out_path.unwrap_or_else(|| ctx.default_ear_path.clone());
                match execute_listen_bars(
                    app,
                    id.clone(),
                    start_bar,
                    end_bar,
                    &solo,
                    sample_rate,
                    dest,
                ) {
                    Ok(response) => HostRequestOutcome::Immediate {
                        response,
                        mutated: false,
                        quit: false,
                        failed: false,
                    },
                    Err(err) => HostRequestOutcome::Immediate {
                        response: error_response(id, app.revision(), format!("{err:#}")),
                        mutated: false,
                        quit: false,
                        failed: true,
                    },
                }
            }
            ListenRequest::Live {
                seconds,
                sample_rate,
                out_path,
            } => {
                let Some(audio) = ctx.audio.as_deref() else {
                    return HostRequestOutcome::Immediate {
                        response: error_response(
                            id,
                            app.revision(),
                            "listen --live requires a running audio engine (use live GUI --ctl or --stdio; use listen --bars for offline rendering)",
                        ),
                        mutated: false,
                        quit: false,
                        failed: true,
                    };
                };
                if !seconds.is_finite() || seconds <= 0.0 {
                    return HostRequestOutcome::Immediate {
                        response: error_response(
                            id,
                            app.revision(),
                            "listen --live seconds must be a positive finite number",
                        ),
                        mutated: false,
                        quit: false,
                        failed: true,
                    };
                }
                if !(8_000..=192_000).contains(&sample_rate) {
                    return HostRequestOutcome::Immediate {
                        response: error_response(
                            id,
                            app.revision(),
                            "listen sample_rate must be between 8000 and 192000 Hz",
                        ),
                        mutated: false,
                        quit: false,
                        failed: true,
                    };
                }
                if let Some(busy) = ctx.live_tap_busy {
                    if busy
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_err()
                    {
                        return HostRequestOutcome::Immediate {
                            response: error_response(
                                id,
                                app.revision(),
                                "Another live audio capture is already in progress; retry once it finishes",
                            ),
                            mutated: false,
                            quit: false,
                            failed: true,
                        };
                    }
                }
                let guard = TapBusyGuard {
                    tap: audio.tap_handle(),
                    busy: ctx.live_tap_busy.map(Arc::clone),
                };
                let clamped = seconds.clamp(MIN_LIVE_LISTEN_SECS, MAX_LIVE_LISTEN_SECS);
                let dest = out_path.unwrap_or_else(|| ctx.default_ear_path.clone());
                HostRequestOutcome::DeferredLiveListen(LiveListenJob {
                    id,
                    revision: app.revision(),
                    seconds: clamped,
                    sample_rate,
                    out_path: dest,
                    guard,
                })
            }
        };
    }

    let action = match (request.action, request.cmd) {
        (Some(action), None) => action,
        (None, Some(line)) => Action::Command { line },
        _ => unreachable!("op_count == 1"),
    };
    let is_get_state = matches!(action, Action::GetState);

    match app.apply(action) {
        Ok(outcome) => {
            let mut failed = false;
            let mut host_error: Option<String> = None;
            let had_fatal_side_effect = outcome.export.is_some()
                || outcome.bounce.is_some()
                || outcome.file_dialog.is_some()
                || outcome.audio_reconfigure.is_some()
                || outcome.audio_restart;

            if let Err(err) =
                apply_outcome_with_hooks(app, ctx.audio.as_deref_mut(), &outcome, hooks)
            {
                host_error = Some(format!("{err:#}"));
                failed = true;
            }

            let snapshot = if let Some(audio) = ctx.audio.as_deref_mut() {
                let snap = audio.snapshot();
                let observed = app.observe_snapshot(&snap);
                if let Err(err) = apply_outcome_with_hooks(app, Some(audio), &observed, hooks) {
                    if host_error.is_none() {
                        host_error = Some(format!("{err:#}"));
                    }
                    failed = true;
                }
                Some(audio.snapshot())
            } else {
                None
            };

            let mut response = if request.compact {
                app.compact_state_response_json(id.clone(), outcome.inspection.clone())
            } else {
                app.full_state_response_json(id.clone(), outcome.inspection.clone())
            };

            if let Some(snap) = snapshot.as_ref() {
                if request.compact {
                    response["playing"] = serde_json::json!(snap.playing);
                    response["step"] = serde_json::json!(snap.absolute_step);
                    response["channel_meters"] = serde_json::json!(channel_meters_json(app, snap));
                    response["effect_meters"] = serde_json::json!(effect_meters_json(snap));
                }
                response["transport"] = transport_json(app, snap, ctx.audio.as_deref());
            }

            if outcome.export.is_some() {
                if let Some(summary) = &app.last_export_summary {
                    response["export"] = serde_json::json!({
                        "target": summary.target,
                        "files_written": summary.files_written,
                        "duration_sec": summary.duration_sec,
                        "peak_left_db": summary.peak_left_db,
                        "peak_right_db": summary.peak_right_db,
                        "rms_db": summary.rms_db,
                        "clips": summary.clips,
                        "sample_rate": summary.sample_rate,
                    });
                }
            }

            if let Some(err_text) = host_error {
                response["audio_error"] = serde_json::json!(err_text);
                if had_fatal_side_effect {
                    response["ok"] = serde_json::json!(false);
                    response["error"] = serde_json::json!(err_text);
                }
            }

            HostRequestOutcome::Immediate {
                response,
                mutated: !is_get_state || outcome.changed || outcome.samples_changed,
                quit: outcome.quit,
                failed,
            }
        }
        Err(error) => HostRequestOutcome::Immediate {
            response: error_response(id, app.revision(), format!("{error:#}")),
            mutated: false,
            quit: false,
            failed: true,
        },
    }
}

/// Parse one newline-delimited JSON [`HostRequest`] and dispatch it via [`execute_host_request`].
pub fn execute_host_request_bytes(
    app: &mut App,
    bytes: &[u8],
    ctx: &mut HostRequestContext<'_>,
    hooks: &mut dyn OutcomeHooks,
) -> HostRequestOutcome {
    match serde_json::from_slice::<HostRequest>(bytes) {
        Ok(request) => execute_host_request(app, request, ctx, hooks),
        Err(error) => HostRequestOutcome::Immediate {
            response: error_response(None, app.revision(), error.to_string()),
            mutated: false,
            quit: false,
            failed: true,
        },
    }
}

enum OwnedRequestLine {
    Eof,
    Complete(Vec<u8>),
    Oversized,
}

fn default_headless_ear_path() -> PathBuf {
    std::env::temp_dir().join(format!("cadence-ear-{}.wav", std::process::id()))
}

fn tick_live(app: &mut App, audio: &mut AudioRuntime) -> Result<()> {
    let mut hooks = HeadlessOutcomeHooks;
    match app.poll_browser_updates() {
        Ok(outcome) => apply_outcome_with_hooks(app, Some(audio), &outcome, &mut hooks)?,
        Err(error) => app.status = format!("{error:#}"),
    }
    let snapshot = audio.snapshot();
    let observed = app.observe_snapshot(&snapshot);
    apply_outcome_with_hooks(app, Some(audio), &observed, &mut hooks)?;
    Ok(())
}

/// Read requests from `input` until it ends, writing one JSON response per
/// line to `output`. Successful requests stay applied when a later one fails.
pub fn process_requests<R: BufRead + Send + 'static, W: Write>(
    app: &mut App,
    mut input: R,
    mut stdout: W,
    engine: Option<&mut AudioRuntime>,
) -> Result<()> {
    let default_ear = default_headless_ear_path();
    let tap_busy = Arc::new(AtomicBool::new(false));
    let mut hooks = HeadlessOutcomeHooks;

    if let Some(audio) = engine {
        let (tx, rx) = mpsc::sync_channel::<io::Result<OwnedRequestLine>>(STDIO_QUEUE_CAPACITY);
        std::thread::Builder::new()
            .name("cadence-stdio-reader".into())
            .spawn(move || {
                let mut line = Vec::with_capacity(MAX_REQUEST_BYTES);
                loop {
                    match read_request_line(&mut input, &mut line) {
                        Ok(RequestLine::Eof) => {
                            let _ = tx.send(Ok(OwnedRequestLine::Eof));
                            break;
                        }
                        Ok(RequestLine::Oversized) => {
                            if tx.send(Ok(OwnedRequestLine::Oversized)).is_err() {
                                break;
                            }
                        }
                        Ok(RequestLine::Complete) => {
                            if tx
                                .send(Ok(OwnedRequestLine::Complete(line.clone())))
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(err) => {
                            let _ = tx.send(Err(err));
                            break;
                        }
                    }
                }
            })
            .context("Spawning stdio reader thread")?;
        let mut failed = false;
        loop {
            tick_live(app, audio)?;
            match rx.recv_timeout(IDLE_TICK) {
                Ok(Ok(OwnedRequestLine::Eof)) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Ok(Err(err)) => return Err(err.into()),
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Ok(Ok(OwnedRequestLine::Oversized)) => {
                    failed = true;
                    let response =
                        error_response(None, app.revision(), "JSONL request exceeds 64 KiB");
                    serde_json::to_writer(&mut stdout, &response)?;
                    writeln!(stdout)?;
                    stdout.flush()?;
                }
                Ok(Ok(OwnedRequestLine::Complete(bytes))) => {
                    if bytes.iter().all(u8::is_ascii_whitespace) {
                        continue;
                    }
                    tick_live(app, audio)?;
                    let mut ctx = HostRequestContext {
                        audio: Some(&mut *audio),
                        default_ear_path: default_ear.clone(),
                        live_tap_busy: Some(&tap_busy),
                    };
                    let (response, req_failed) =
                        match execute_host_request_bytes(app, &bytes, &mut ctx, &mut hooks) {
                            HostRequestOutcome::Immediate {
                                response, failed, ..
                            } => (response, failed),
                            HostRequestOutcome::DeferredLiveListen(job) => job.execute(),
                        };
                    failed |= req_failed;
                    serde_json::to_writer(&mut stdout, &response)?;
                    writeln!(stdout)?;
                    stdout.flush()?;
                }
            }
        }
        let shutdown = app.shutdown();
        let _ = apply_outcome_with_hooks(app, Some(audio), &shutdown, &mut hooks);
        if failed {
            bail!("one or more JSONL requests failed; successful requests remain applied");
        }
        return Ok(());
    }

    let mut line = Vec::with_capacity(MAX_REQUEST_BYTES);
    let mut failed = false;
    loop {
        if let Err(error) = app.poll_browser_updates() {
            app.status = format!("{error:#}");
        }
        let response = match read_request_line(&mut input, &mut line)? {
            RequestLine::Eof => break,
            RequestLine::Oversized => {
                failed = true;
                error_response(None, app.revision(), "JSONL request exceeds 64 KiB")
            }
            RequestLine::Complete if line.iter().all(u8::is_ascii_whitespace) => continue,
            RequestLine::Complete => {
                let mut ctx = HostRequestContext {
                    audio: None,
                    default_ear_path: default_ear.clone(),
                    live_tap_busy: Some(&tap_busy),
                };
                let (response, req_failed) =
                    match execute_host_request_bytes(app, &line, &mut ctx, &mut hooks) {
                        HostRequestOutcome::Immediate {
                            response, failed, ..
                        } => (response, failed),
                        HostRequestOutcome::DeferredLiveListen(job) => job.execute(),
                    };
                failed |= req_failed;
                response
            }
        };
        serde_json::to_writer(&mut stdout, &response)?;
        writeln!(stdout)?;
        stdout.flush()?;
    }
    if failed {
        bail!("one or more JSONL requests failed; successful requests remain applied");
    }
    Ok(())
}

pub const MAX_REQUEST_BYTES: usize = 65_536;
const STDIO_QUEUE_CAPACITY: usize = 64;

pub enum RequestLine {
    Eof,
    Complete,
    Oversized,
}

/// Read one newline-delimited frame bounded to `max_bytes`.
///
/// When `drain_oversized` is true, bytes past `max_bytes` are discarded until the
/// terminating `\n` (or EOF) so streaming JSONL sessions recover on the next line.
/// When false, returns [`RequestLine::Oversized`] immediately without waiting for `\n`.
pub fn read_bounded_line<R: BufRead>(
    input: &mut R,
    line: &mut Vec<u8>,
    max_bytes: usize,
    drain_oversized: bool,
) -> io::Result<RequestLine> {
    line.clear();
    let mut saw_bytes = false;
    let mut oversized = false;
    loop {
        let available = input.fill_buf()?;
        if available.is_empty() {
            return Ok(if !saw_bytes {
                RequestLine::Eof
            } else if oversized {
                RequestLine::Oversized
            } else {
                RequestLine::Complete
            });
        }
        saw_bytes = true;
        let newline = available.iter().position(|&byte| byte == b'\n');
        let length = newline.unwrap_or(available.len());
        if !oversized {
            if length <= max_bytes.saturating_sub(line.len()) {
                line.extend_from_slice(&available[..length]);
            } else {
                oversized = true;
                line.clear();
                if !drain_oversized {
                    input.consume(length + usize::from(newline.is_some()));
                    return Ok(RequestLine::Oversized);
                }
            }
        }
        input.consume(length + usize::from(newline.is_some()));
        if newline.is_some() {
            return Ok(if oversized {
                RequestLine::Oversized
            } else {
                RequestLine::Complete
            });
        }
    }
}

pub fn read_request_line<R: BufRead>(input: &mut R, line: &mut Vec<u8>) -> io::Result<RequestLine> {
    read_bounded_line(input, line, MAX_REQUEST_BYTES, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadence_app::MemoryHostIo;
    use cadence_core::material::Project;

    #[test]
    fn dropping_live_listen_job_without_execute_releases_tap_busy_guard() {
        let mut app = App::new_with_host_io(
            Project::demo(),
            PathBuf::from("/virtual/project.json"),
            Arc::new(MemoryHostIo::default()),
        )
        .unwrap();
        let mut audio = AudioRuntime::start(app.audio_project().clone(), true).unwrap();
        let busy = Arc::new(AtomicBool::new(false));
        let mut ctx = HostRequestContext {
            audio: Some(&mut audio),
            default_ear_path: PathBuf::from("/virtual/ear.wav"),
            live_tap_busy: Some(&busy),
        };
        let mut hooks = HeadlessOutcomeHooks;

        let make_live_req = |id: u64| HostRequest {
            id: Some(serde_json::json!(id)),
            listen: Some(ListenRequest::Live {
                seconds: 0.5,
                sample_rate: DEFAULT_LISTEN_SAMPLE_RATE,
                out_path: None,
            }),
            ..HostRequest::default()
        };

        let outcome = execute_host_request(&mut app, make_live_req(42), &mut ctx, &mut hooks);
        let HostRequestOutcome::DeferredLiveListen(job) = outcome else {
            panic!("expected DeferredLiveListen");
        };
        assert_eq!(job.request_id(), Some(&serde_json::json!(42)));
        assert!(
            busy.load(Ordering::Acquire),
            "live_tap_busy must be true while job is alive"
        );

        // Second concurrent request is cleanly rejected while job holds the guard.
        let busy_outcome = execute_host_request(&mut app, make_live_req(43), &mut ctx, &mut hooks);
        assert!(matches!(
            busy_outcome,
            HostRequestOutcome::Immediate { failed: true, .. }
        ));

        // Simulating a thread::Builder::spawn failure: drop `job` without calling `execute()`.
        drop(job);
        assert!(
            !busy.load(Ordering::Acquire),
            "dropping LiveListenJob without execute() must release live_tap_busy"
        );

        // Subsequent request immediately acquires the guard again.
        let outcome3 = execute_host_request(&mut app, make_live_req(44), &mut ctx, &mut hooks);
        assert!(matches!(
            outcome3,
            HostRequestOutcome::DeferredLiveListen(_)
        ));
        assert!(busy.load(Ordering::Acquire));
        drop(outcome3);
        assert!(
            !busy.load(Ordering::Acquire),
            "dropping outcome3 immediately clears live_tap_busy again"
        );
    }

    #[test]
    fn meters_and_state_responses_expose_audio_health_and_dispatch_audio_actions() {
        let mut app = App::new_with_host_io(
            Project::demo(),
            PathBuf::from("/virtual/project.json"),
            Arc::new(MemoryHostIo::default()),
        )
        .unwrap();
        let mut audio = AudioRuntime::start(app.audio_project().clone(), true).unwrap();
        let mut ctx = HostRequestContext {
            audio: Some(&mut audio),
            default_ear_path: PathBuf::from("/virtual/ear.wav"),
            live_tap_busy: None,
        };
        let mut hooks = HeadlessOutcomeHooks;

        let meters = execute_host_request(
            &mut app,
            HostRequest {
                id: Some(serde_json::json!(1)),
                meters: true,
                ..HostRequest::default()
            },
            &mut ctx,
            &mut hooks,
        );
        let HostRequestOutcome::Immediate {
            response, failed, ..
        } = meters
        else {
            panic!("expected Immediate meters response");
        };
        assert!(!failed);
        assert_eq!(response["ok"], true);
        assert_eq!(response["health"]["status"], "silent");
        assert_eq!(response["health"]["overloaded"], false);
        assert_eq!(response["health"]["failed"], false);
        assert_eq!(response["health"]["xrun_count"], 0);
        assert!(response["health"]["description"]
            .as_str()
            .is_some_and(|d| d.contains("Silent")));

        let rescan = execute_host_request(
            &mut app,
            HostRequest {
                id: Some(serde_json::json!(2)),
                compact: true,
                action: Some(Action::RescanAudio),
                ..HostRequest::default()
            },
            &mut ctx,
            &mut hooks,
        );
        let HostRequestOutcome::Immediate {
            response: rescan_resp,
            failed: rescan_failed,
            ..
        } = rescan
        else {
            panic!("expected Immediate rescan response");
        };
        assert!(!rescan_failed);
        assert_eq!(rescan_resp["ok"], true);
        assert_eq!(rescan_resp["transport"]["health"]["status"], "silent");
        assert!(app.devices.inventory.silent_mode);
        assert_eq!(app.devices.inventory.active_sample_rate, 48_000);

        let panic_res = execute_host_request(
            &mut app,
            HostRequest {
                id: Some(serde_json::json!(3)),
                compact: true,
                action: Some(Action::AudioPanic),
                ..HostRequest::default()
            },
            &mut ctx,
            &mut hooks,
        );
        let HostRequestOutcome::Immediate {
            response: panic_resp,
            failed: panic_failed,
            ..
        } = panic_res
        else {
            panic!("expected Immediate panic response");
        };
        assert!(!panic_failed);
        assert_eq!(panic_resp["ok"], true);
        assert_eq!(panic_resp["playing"], false);
    }
}
