//! Deterministic offline WAV rendering and signal telemetry.
use super::engine::Engine;
use super::shared::*;
use super::*;
use cadence_core::assets::ensure_sample_bank_ready;
use cadence_core::dsp::{sanitize_sample, AUDIO_SUB_BLOCK_FRAMES};
use cadence_core::mixer::{ChannelRef, EffectKind, MAX_MIXER_CHANNELS};
use cadence_core::music::STEPS_PER_BAR;
use cadence_core::runtime::{RackActivityTelemetry, SampleBank};
use cadence_core::score::ScoreWindow;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub fn render_wav_with_samples(
    project: &ScoreWindow,
    samples: SampleBank,
    path: impl AsRef<Path>,
    bars: u32,
    sample_rate: u32,
) -> Result<()> {
    render_wav_with_samples_report(project, samples, path, bars, sample_rate).map(|_| ())
}

/// Whole-render observations from actual channel processing, before WAV fading.
/// `hot` latches when a 32-bit float bus exceeds 0 dBFS (`> 1.0`) on input or output
/// (informational headroom, not audible clipping). `clipped` latches strictly on
/// non-finite samples anywhere OR Master post-insert post-fader output `> 1.0`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RenderChannelReport {
    pub channel: ChannelRef,
    pub name: String,
    pub input_peak_hold: [f32; 2],
    pub output_peak_hold: [f32; 2],
    pub hot: bool,
    pub clipped: bool,
}

/// Whole-render dynamics telemetry for an active Compressor or Limiter insert device.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RenderEffectReport {
    pub device_id: u64,
    pub channel: ChannelRef,
    pub kind: EffectKind,
    pub max_gr_db: f32,
    pub avg_gr_db: f32,
}

/// Measurements of written 16-bit PCM, normalized by i16::MAX. RMS and DC
/// include silence; these are sample statistics, not perceptual loudness.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RenderSignalReport {
    pub frames: u64,
    pub peak: [f64; 2],
    pub rms: [f64; 2],
    pub dc: [f64; 2],
    pub full_scale_samples: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RenderReport {
    pub bars: u32,
    pub sample_rate: u32,
    pub frames: u64,
    pub channels: Vec<RenderChannelReport>,
    pub effects: Vec<RenderEffectReport>,
    pub output: RenderSignalReport,
    /// Last 500 ms, excluding the final 10 ms so the exporter fade cannot hide
    /// an unfinished effect/release tail. Frame bounds are half-open.
    pub tail: RenderSignalReport,
    pub tail_start_frame: u64,
    pub tail_end_frame: u64,
}

#[derive(Default)]
struct RenderSignalAccumulator {
    frames: u64,
    peak: [f64; 2],
    sum: [f64; 2],
    squares: [f64; 2],
    full_scale_samples: u64,
}

impl RenderSignalAccumulator {
    fn observe(&mut self, pcm: [i16; 2]) {
        self.frames += 1;
        for (channel, sample) in pcm.into_iter().enumerate() {
            let value = f64::from(sample) / f64::from(i16::MAX);
            self.peak[channel] = self.peak[channel].max(value.abs());
            self.sum[channel] += value;
            self.squares[channel] += value * value;
            self.full_scale_samples += u64::from(sample.unsigned_abs() >= i16::MAX as u16);
        }
    }

    fn finish(self) -> RenderSignalReport {
        let denominator = self.frames.max(1) as f64;
        RenderSignalReport {
            frames: self.frames,
            peak: self.peak,
            rms: self.squares.map(|energy| (energy / denominator).sqrt()),
            dc: self.sum.map(|sum| sum / denominator),
            full_scale_samples: self.full_scale_samples,
        }
    }
}

/// Render through the production engine and report pre-protection channel
/// overloads plus exact PCM statistics. No normalization or corrective audio
/// processing is added. Preparation and report allocation stay off the callback.
pub fn render_wav_with_samples_report(
    project: &ScoreWindow,
    samples: SampleBank,
    path: impl AsRef<Path>,
    bars: u32,
    sample_rate: u32,
) -> Result<RenderReport> {
    if !(1..=1_024).contains(&bars) {
        bail!("WAV length must be between 1 and 1024 bars");
    }
    render_schedule_report(
        &[(0, project)],
        0,
        u64::from(bars) * STEPS_PER_BAR as u64,
        samples,
        path,
        sample_rate,
    )
}

/// Render through the production engine and report pre-protection channel
/// overloads plus exact PCM statistics without creating a WAV file.
pub fn render_measure_report(
    project: &ScoreWindow,
    samples: SampleBank,
    bars: u32,
    sample_rate: u32,
) -> Result<RenderReport> {
    if !(1..=1_024).contains(&bars) {
        bail!("WAV length must be between 1 and 1024 bars");
    }
    render_schedule_measure(
        &[(0, project)],
        0,
        u64::from(bars) * STEPS_PER_BAR as u64,
        samples,
        sample_rate,
    )
}

/// Renders `steps` steps from `start_step` in memory and returns its report.
pub fn render_schedule_measure(
    schedule: &[(u64, &ScoreWindow)],
    start_step: u64,
    steps: u64,
    samples: SampleBank,
    sample_rate: u32,
) -> Result<RenderReport> {
    render_schedule_inner(schedule, start_step, steps, samples, sample_rate, |_, _| {
        Ok(())
    })
}

/// Renders `steps` steps from `start_step` into memory, returning both the
/// report and raw stereo 16-bit PCM frames for exact diffing.
pub fn render_schedule_pcm(
    schedule: &[(u64, &ScoreWindow)],
    start_step: u64,
    steps: u64,
    samples: SampleBank,
    sample_rate: u32,
) -> Result<(RenderReport, Vec<[i16; 2]>)> {
    let mut frames = Vec::new();
    let report = render_schedule_inner(
        schedule,
        start_step,
        steps,
        samples,
        sample_rate,
        |_, pcm| {
            frames.push(pcm);
            Ok(())
        },
    )?;
    Ok((report, frames))
}

/// Renders `steps` steps from `start_step` into memory, returning both the
/// report and raw sanitized 32-bit float stereo frames directly from the
/// production engine (before final WAV fade/quantization).
pub fn render_schedule_f32(
    schedule: &[(u64, &ScoreWindow)],
    start_step: u64,
    steps: u64,
    samples: SampleBank,
    sample_rate: u32,
) -> Result<(RenderReport, Vec<[f32; 2]>)> {
    let mut frames = Vec::new();
    let report = render_schedule_inner(
        schedule,
        start_step,
        steps,
        samples,
        sample_rate,
        |stereo, _| {
            frames.push(stereo);
            Ok(())
        },
    )?;
    Ok((report, frames))
}

/// Render `steps` steps from `start_step` through the production engine while watching
/// one Modular Rack `(instrument_id, zone_id)`, returning the live [`RackActivityTelemetry`]
/// captured at the end of the rendered window.
pub fn render_rack_activity_snapshot(
    project: &ScoreWindow,
    samples: SampleBank,
    sample_rate: u32,
    start_step: u64,
    steps: u64,
    instrument_id: u64,
    zone_id: u16,
) -> Result<RackActivityTelemetry> {
    let schedule = [(0u64, project)];
    let frames = validate_render_schedule(&schedule, start_step, steps, &samples, sample_rate)?;
    let shared = Shared::new_arc();
    shared.playing.store(true, Ordering::Release);
    shared.rack_watch_zone_id.store(zone_id, Ordering::Release);
    shared
        .rack_watch_inst_id
        .store(instrument_id, Ordering::Release);
    let mut engine = Engine::new(project.clone(), samples, sample_rate, Arc::clone(&shared));
    if start_step > 0 {
        engine.seek_to_step(start_step);
        engine.snap_controls_to_current_step();
    }
    let mut block = [[0.0_f32; 2]; AUDIO_SUB_BLOCK_FRAMES];
    let mut index = 0u64;
    while index < frames {
        let chunk = (frames - index).min(AUDIO_SUB_BLOCK_FRAMES as u64) as usize;
        engine.render_block(&mut block[..chunk]);
        index += chunk as u64;
    }
    Ok(engine.rack_activity_snapshot())
}

/// Renders `steps` steps from `start_step`, adopting each scheduled score
/// exactly when the transport reaches its step. The first score plays from
/// the start; later ones carry tempo, patch or mixer changes.
pub fn render_schedule_report(
    schedule: &[(u64, &ScoreWindow)],
    start_step: u64,
    steps: u64,
    samples: SampleBank,
    path: impl AsRef<Path>,
    sample_rate: u32,
) -> Result<RenderReport> {
    let frames = validate_render_schedule(schedule, start_step, steps, &samples, sample_rate)?;
    write_stereo_wav_atomic(path.as_ref(), sample_rate, 16, frames, |writer| {
        render_schedule_validated(
            schedule,
            start_step,
            steps,
            frames,
            samples,
            sample_rate,
            |_, pcm| {
                for sample in pcm {
                    writer.write_sample(sample)?;
                }
                Ok(())
            },
        )
    })
}

static RENDER_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn ensure_riff_wav_stereo_size(frames: u64, bits_per_sample: u16) -> Result<()> {
    let bytes_per_frame = u64::from(bits_per_sample / 8).saturating_mul(2);
    if frames.saturating_mul(bytes_per_frame) > u64::from(u32::MAX) - 64 {
        bail!("WAV would exceed the 4 GiB RIFF limit; reduce duration or sample rate");
    }
    Ok(())
}

#[cfg(unix)]
fn sync_parent_dir(dir: &Path) {
    if let Ok(handle) = std::fs::File::open(dir) {
        let _ = handle.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_parent_dir(_dir: &Path) {}

/// Single atomic stereo PCM WAV writer shared by offline schedule renders,
/// live tap exports, and 24-bit audio input recordings: validates sample rate +
/// 4 GiB RIFF limit, creates parent directories, streams into a unique sibling
/// temp file, finalizes + fsyncs before atomic rename, and removes the temp file
/// on any failure.
fn write_stereo_wav_atomic<T>(
    dest: &Path,
    sample_rate: u32,
    bits_per_sample: u16,
    expected_frames: u64,
    write_samples: impl FnOnce(&mut hound::WavWriter<std::io::BufWriter<std::fs::File>>) -> Result<T>,
) -> Result<T> {
    ensure!(
        (8_000..=192_000).contains(&sample_rate),
        "WAV sample rate must be between 8000 and 192000 Hz"
    );
    ensure!(
        bits_per_sample == 16 || bits_per_sample == 24,
        "WAV bit depth must be 16 or 24 bits"
    );
    ensure_riff_wav_stereo_size(expected_frames, bits_per_sample)?;
    if let Some(parent) = dest.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Creating WAV directory {}", parent.display()))?;
    }
    let parent = dest
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = dest
        .file_name()
        .context("WAV destination needs a filename")?
        .to_string_lossy();
    let temp = parent.join(format!(
        ".{name}.{}.{}.wav.tmp",
        std::process::id(),
        RENDER_TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample,
        sample_format: hound::SampleFormat::Int,
    };
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .with_context(|| format!("creating {}", dest.display()))?;
    let sync_handle = match file.try_clone() {
        Ok(handle) => handle,
        Err(err) => {
            drop(file);
            let _ = std::fs::remove_file(&temp);
            return Err(err)
                .with_context(|| format!("preparing sync handle for {}", dest.display()));
        }
    };
    let mut writer = match hound::WavWriter::new(std::io::BufWriter::new(file), spec) {
        Ok(writer) => writer,
        Err(err) => {
            drop(sync_handle);
            let _ = std::fs::remove_file(&temp);
            return Err(err).with_context(|| format!("creating {}", dest.display()));
        }
    };
    match write_samples(&mut writer) {
        Ok(value) => {
            let commit = (|| -> Result<()> {
                writer.finalize().context("finishing WAV file")?;
                sync_handle
                    .sync_all()
                    .with_context(|| format!("syncing {}", dest.display()))?;
                drop(sync_handle);
                std::fs::rename(&temp, dest)
                    .with_context(|| format!("saving {}", dest.display()))?;
                sync_parent_dir(parent);
                Ok(())
            })();
            if let Err(err) = commit {
                let _ = std::fs::remove_file(&temp);
                return Err(err);
            }
            Ok(value)
        }
        Err(err) => {
            drop(writer);
            drop(sync_handle);
            let _ = std::fs::remove_file(&temp);
            Err(err)
        }
    }
}

fn validate_render_schedule(
    schedule: &[(u64, &ScoreWindow)],
    start_step: u64,
    steps: u64,
    samples: &SampleBank,
    sample_rate: u32,
) -> Result<u64> {
    let _ = schedule.first().context("Render needs a score")?;
    ensure!(
        schedule.windows(2).all(|w| w[0].0 <= w[1].0),
        "Render schedule must be ordered"
    );
    for (_, score) in schedule {
        score.validate()?;
        ensure_sample_bank_ready(score, samples)?;
    }
    if !(8_000..=192_000).contains(&sample_rate) {
        bail!("WAV sample rate must be between 8000 and 192000 Hz");
    }
    ensure!(steps > 0, "Render needs at least one step");
    let end = start_step
        .checked_add(steps)
        .context("Render step range overflowed u64")?;
    let mut seconds_frames = 0.0;
    for (index, (from, score)) in schedule.iter().enumerate() {
        // The first scheduled score always plays starting at `start_step` even when
        // `schedule[0].0 > start_step`.
        let from = if index == 0 {
            start_step
        } else {
            (*from).max(start_step)
        };
        let until = schedule
            .get(index + 1)
            .map_or(end, |(next, _)| (*next).clamp(from, end));
        if until > from {
            seconds_frames += (until - from) as f64 * score.step_seconds() * sample_rate as f64;
        }
    }
    let frames = seconds_frames.round() as u64;
    ensure_riff_wav_stereo_size(frames, 16)?;
    Ok(frames)
}

fn render_schedule_inner(
    schedule: &[(u64, &ScoreWindow)],
    start_step: u64,
    steps: u64,
    samples: SampleBank,
    sample_rate: u32,
    sink: impl FnMut([f32; 2], [i16; 2]) -> Result<()>,
) -> Result<RenderReport> {
    let frames = validate_render_schedule(schedule, start_step, steps, &samples, sample_rate)?;
    render_schedule_validated(
        schedule,
        start_step,
        steps,
        frames,
        samples,
        sample_rate,
        sink,
    )
}

fn render_schedule_validated(
    schedule: &[(u64, &ScoreWindow)],
    start_step: u64,
    steps: u64,
    frames: u64,
    samples: SampleBank,
    sample_rate: u32,
    mut sink: impl FnMut([f32; 2], [i16; 2]) -> Result<()>,
) -> Result<RenderReport> {
    let (_, project) = *schedule.first().context("Render needs a score")?;
    let shared = Shared::new_arc();
    shared.playing.store(true, Ordering::Release);
    let mut engine = Engine::new(project.clone(), samples, sample_rate, Arc::clone(&shared));
    if start_step > 0 {
        engine.seek_to_step(start_step);
        engine.snap_controls_to_current_step();
    }
    let mut upcoming = schedule.iter().skip(1).peekable();
    let fade_frames = (sample_rate as u64 / 200).max(1);
    let tail_start_frame = frames.saturating_sub(u64::from(sample_rate) / 2);
    let tail_end_frame = frames.saturating_sub(u64::from(sample_rate) / 100);
    let mut output = RenderSignalAccumulator::default();
    let mut tail = RenderSignalAccumulator::default();
    let mut block = [[0.0_f32; 2]; AUDIO_SUB_BLOCK_FRAMES];
    let mut index = 0u64;
    while index < frames {
        // Publish after the previous step played and before this one does;
        // the engine adopts pending scores at the start of each step.
        while let Some((at, score)) = upcoming.next_if(|(at, _)| engine.play_step() >= *at) {
            let _ = at;
            engine.publish_score_update((*score).clone());
        }
        let chunk = (frames - index).min(AUDIO_SUB_BLOCK_FRAMES as u64) as usize;
        let rendered = if upcoming.peek().is_some() {
            engine.render_sub_block(&mut block[..chunk])
        } else {
            engine.render_block(&mut block[..chunk]);
            chunk
        };
        for &raw in &block[..rendered] {
            let stereo = raw.map(sanitize_sample);
            let fade = if frames - index <= fade_frames {
                (frames - index - 1) as f32 / fade_frames as f32
            } else {
                1.0
            };
            // NaN-safe clamp strictly at the final i16 WAV conversion boundary.
            let pcm = stereo
                .map(|sample| (sample.clamp(-1.0, 1.0) * fade * i16::MAX as f32).round() as i16);
            sink(stereo, pcm)?;
            output.observe(pcm);
            if (tail_start_frame..tail_end_frame).contains(&index) {
                tail.observe(pcm);
            }
            index += 1;
        }
    }
    let (channels, effects) = engine.collect_render_telemetry();
    Ok(RenderReport {
        bars: steps.div_ceil(STEPS_PER_BAR as u64) as u32,
        sample_rate,
        frames,
        channels,
        effects,
        output: output.finish(),
        tail: tail.finish(),
        tail_start_frame,
        tail_end_frame,
    })
}

/// Render an exact step span `[start_step, start_step + steps)` from `score` to a stereo
/// 16-bit WAV file for resampling / bounce-to-sampler, creating parent directories if needed.
pub fn render_bounce_wav(
    score: &ScoreWindow,
    samples: SampleBank,
    start_step: u64,
    steps: u64,
    path: impl AsRef<Path>,
    sample_rate: u32,
) -> Result<RenderReport> {
    render_schedule_report(&[(0, score)], start_step, steps, samples, path, sample_rate)
}

/// Output paths and master telemetry from a multi-track stem export.
#[derive(Clone, Debug)]
pub struct StemExportSummary {
    pub master_path: std::path::PathBuf,
    pub stem_paths: Vec<std::path::PathBuf>,
    pub master_report: RenderReport,
}

fn sanitize_stem_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if (ch.is_whitespace() || ch == '-' || ch == '_')
            && !out.ends_with('-')
            && !out.is_empty()
        {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "Channel".into()
    } else {
        trimmed.to_string()
    }
}

/// Export `00-Master.wav` plus one isolated stereo WAV per active `Audio` channel
/// (`01-<Name>.wav`, `02-<Name>.wav`, ...) through the exact production mixer & FX graph.
///
/// Compiles all stem jobs deterministically upfront and renders independent WAVs in
/// parallel across scoped worker threads capped to [`std::thread::available_parallelism`],
/// preserving deterministic filenames, `stem_paths` order, and error propagation.
pub fn render_stems_with_samples_report(
    project: &cadence_core::material::Project,
    flags: &cadence_core::mixer::LiveFlags,
    samples: SampleBank,
    out_dir: impl AsRef<Path>,
    start_step: u64,
    steps: u64,
    sample_rate: u32,
) -> Result<StemExportSummary> {
    use std::sync::atomic::AtomicUsize;

    let dir = out_dir.as_ref();
    std::fs::create_dir_all(dir)
        .with_context(|| format!("Creating stems directory {}", dir.display()))?;

    let master_score = project.compile_with_flags(flags)?;
    let master_path = dir.join("00-Master.wav");

    let is_ancestor_group_muted = |mut out: ChannelRef| -> bool {
        for _ in 0..MAX_MIXER_CHANNELS {
            let ChannelRef::Channel(gid) = out else {
                return false;
            };
            if flags.channel_muted(ChannelRef::Channel(gid)) {
                return true;
            }
            let Some(group) = project.mixer.channels.iter().find(|c| c.id == gid) else {
                return false;
            };
            if group.kind != cadence_core::mixer::ChannelKind::Group {
                return false;
            }
            out = group.output;
        }
        false
    };

    let mut jobs: Vec<(ScoreWindow, std::path::PathBuf)> = Vec::new();
    jobs.push((master_score, master_path.clone()));

    let mut stem_paths = Vec::new();
    let mut stem_idx = 1usize;
    for ch in &project.mixer.channels {
        if ch.kind != cadence_core::mixer::ChannelKind::Audio {
            continue;
        }
        if flags.channel_muted(ChannelRef::Channel(ch.id)) || is_ancestor_group_muted(ch.output) {
            continue;
        }
        let mut solo_flags = flags.clone();
        solo_flags.solo_channels.clear();
        solo_flags.solo_channels.insert(ch.id);
        let stem_score = project.compile_with_flags(&solo_flags)?;
        let filename = format!("{stem_idx:02}-{}.wav", sanitize_stem_name(&ch.name));
        let stem_path = dir.join(filename);
        stem_paths.push(stem_path.clone());
        jobs.push((stem_score, stem_path));
        stem_idx += 1;
    }

    let worker_count = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .clamp(1, jobs.len());

    let mut outcomes: Vec<Option<Result<RenderReport>>> = (0..jobs.len()).map(|_| None).collect();
    if worker_count <= 1 {
        for (idx, (score, path)) in jobs.iter().enumerate() {
            outcomes[idx] = Some(render_schedule_report(
                &[(0, score)],
                start_step,
                steps,
                samples.clone(),
                path,
                sample_rate,
            ));
        }
    } else {
        type StemJobOutcome = (usize, Result<RenderReport>);
        let next_job = AtomicUsize::new(0);
        let jobs_ref = &jobs;
        let samples_ref = &samples;
        let batches: Vec<Result<Vec<StemJobOutcome>>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..worker_count)
                .map(|_| {
                    let next_job = &next_job;
                    scope.spawn(move || {
                        let mut local = Vec::new();
                        loop {
                            let idx = next_job.fetch_add(1, Ordering::Relaxed);
                            let Some((score, path)) = jobs_ref.get(idx) else {
                                break;
                            };
                            let res = render_schedule_report(
                                &[(0, score)],
                                start_step,
                                steps,
                                samples_ref.clone(),
                                path,
                                sample_rate,
                            );
                            local.push((idx, res));
                        }
                        local
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join()
                        .map_err(|_| anyhow::anyhow!("Stem export worker panicked"))
                })
                .collect()
        });
        for batch in batches {
            for (idx, res) in batch? {
                if let Some(slot) = outcomes.get_mut(idx) {
                    *slot = Some(res);
                }
            }
        }
    }

    let mut master_report = None;
    for (idx, outcome) in outcomes.into_iter().enumerate() {
        let report =
            outcome.with_context(|| format!("Missing render result for stem job {idx}"))??;
        if idx == 0 {
            master_report = Some(report);
        }
    }
    let master_report = master_report.context("Missing master render report")?;

    Ok(StemExportSummary {
        master_path,
        stem_paths,
        master_report,
    })
}

/// Atomically write stereo 32-bit float frames to a 16-bit PCM WAV file, applying
/// NaN-safe clamping (`[-1.0, 1.0]`) strictly at the final `i16` quantization boundary.
pub fn write_stereo_wav_16bit(
    path: impl AsRef<Path>,
    sample_rate: u32,
    frames: &[[f32; 2]],
    apply_end_fade: bool,
) -> Result<RenderSignalReport> {
    let total = frames.len() as u64;
    let fade_frames = if apply_end_fade {
        (sample_rate as u64 / 200).max(1)
    } else {
        0
    };
    write_stereo_wav_atomic(path.as_ref(), sample_rate, 16, total, |writer| {
        let mut stats = RenderSignalAccumulator::default();
        for (idx, &stereo) in frames.iter().enumerate() {
            let fade = if fade_frames > 0 && total - (idx as u64) <= fade_frames {
                (total - (idx as u64) - 1) as f32 / fade_frames as f32
            } else {
                1.0
            };
            let pcm = stereo.map(|s| {
                (sanitize_sample(s).clamp(-1.0, 1.0) * fade * i16::MAX as f32).round() as i16
            });
            writer.write_sample(pcm[0])?;
            writer.write_sample(pcm[1])?;
            stats.observe(pcm);
        }
        Ok(stats.finish())
    })
}

/// Statistics returned after atomically finalizing a 24-bit recorded stereo WAV clip.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordedWavStats {
    pub frames: u64,
    pub sample_rate: u32,
    pub duration_sec: f64,
    pub peak_left_db: f64,
    pub peak_right_db: f64,
    pub full_scale_samples: u64,
}

const PCM_24_MAX: f64 = 8_388_607.0;

/// Atomically write stereo 32-bit float frames to a 24-bit signed PCM WAV file,
/// clamping strictly to `[-1.0, 1.0]` at the final `i24` quantization boundary.
pub fn write_stereo_wav_24bit(
    path: impl AsRef<Path>,
    sample_rate: u32,
    frames: &[[f32; 2]],
) -> Result<RecordedWavStats> {
    ensure!(
        !frames.is_empty(),
        "Cannot write an empty 24-bit WAV recording"
    );
    let total = frames.len() as u64;
    write_stereo_wav_atomic(path.as_ref(), sample_rate, 24, total, |writer| {
        let mut peak = [0.0_f64; 2];
        let mut full_scale = 0_u64;
        for &stereo in frames {
            for (ch, s) in stereo.into_iter().enumerate() {
                let clamped = f64::from(sanitize_sample(s).clamp(-1.0, 1.0));
                let quant = (clamped * PCM_24_MAX).round() as i32;
                let norm = f64::from(quant).abs() / PCM_24_MAX;
                peak[ch] = peak[ch].max(norm);
                if quant.unsigned_abs() >= PCM_24_MAX as u32 {
                    full_scale += 1;
                }
                writer.write_sample(quant)?;
            }
        }
        Ok(RecordedWavStats {
            frames: total,
            sample_rate,
            duration_sec: total as f64 / f64::from(sample_rate.max(1)),
            peak_left_db: cadence_core::dsp::lin_to_db_f64(peak[0]),
            peak_right_db: cadence_core::dsp::lin_to_db_f64(peak[1]),
            full_scale_samples: full_scale,
        })
    })
}

/// Apply round-trip latency compensation (`trim_leading_frames > 0` skips leading frames,
/// `< 0` prepends bounded silence), smooth 1.5 ms edge micro-ramps to prevent clicks, and
/// atomically write a portable 24-bit stereo WAV recording.
pub fn finalize_recording_wav(
    frames: &[[f32; 2]],
    sample_rate: u32,
    trim_leading_frames: isize,
    dest: impl AsRef<Path>,
) -> Result<RecordedWavStats> {
    ensure!(
        !frames.is_empty(),
        "No audio frames were captured for recording"
    );
    let mut processed: Vec<[f32; 2]> = if trim_leading_frames > 0 {
        let skip = (trim_leading_frames as usize).min(frames.len().saturating_sub(1));
        frames[skip..].to_vec()
    } else if trim_leading_frames < 0 {
        let pad = ((-trim_leading_frames) as usize).min(sample_rate as usize);
        let mut out = Vec::with_capacity(pad + frames.len());
        out.resize(pad, [0.0, 0.0]);
        out.extend_from_slice(frames);
        out
    } else {
        frames.to_vec()
    };

    // Short 1.5 ms edge ramps eliminate start/stop clicks while keeping musical transients intact.
    let ramp = ((f64::from(sample_rate) * 0.0015).round() as usize).min(processed.len() / 4);
    if ramp > 0 {
        let len = processed.len();
        for i in 0..ramp {
            let g = i as f32 / ramp as f32;
            processed[i][0] *= g;
            processed[i][1] *= g;
            let tail = len - 1 - i;
            processed[tail][0] *= g;
            processed[tail][1] *= g;
        }
    }
    write_stereo_wav_24bit(dest, sample_rate, &processed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_schedule_creates_parent_dirs_and_guards_step_overflow() {
        let dir = tempfile::tempdir().unwrap();
        let fx = crate::test_fixtures::kitchen_sink_fixture(8_000);

        // Missing nested parent directories are created automatically by the atomic WAV pipeline.
        let nested_wav = dir.path().join("nested/exports/out.wav");
        let report = render_schedule_report(
            &[(0, &fx.score)],
            0,
            16,
            fx.samples.clone(),
            &nested_wav,
            8_000,
        )
        .unwrap();
        assert!(nested_wav.is_file());
        assert_eq!(report.sample_rate, 8_000);
        assert!(report.frames > 0);

        // Overflowing `start_step + steps` returns a clean error instead of wrapping or panicking.
        let err = validate_render_schedule(&[(0, &fx.score)], u64::MAX - 4, 16, &fx.samples, 8_000);
        assert!(err.is_err());
    }
}
