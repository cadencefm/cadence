//! Lock-free, zero-allocation CPAL audio input capture, live stereo metering,
//! and drift-tolerant software input monitoring.
use super::shared::*;
use cadence_core::dsp::{one_pole_decay_ms, sanitize_sample, sanitize_sample_rate, PeakHold};
use cadence_core::runtime::{
    AudioInputMode, AudioInputStatus, CapturePhase, InputMonitorMode, MAX_AUDIO_SAMPLE_RATE,
    MIN_AUDIO_SAMPLE_RATE,
};
use std::sync::atomic::Ordering;

/// Input sample conversion abstraction shared by live CPAL capture callbacks and
/// feature-independent unit tests.
pub(super) trait InputSample: Copy {
    fn to_unit_f32(self) -> f32;
}

#[cfg(feature = "audio")]
impl<T: cpal::Sample> InputSample for T
where
    f32: cpal::FromSample<T>,
{
    #[inline]
    fn to_unit_f32(self) -> f32 {
        cpal::FromSample::from_sample_(self)
    }
}

#[cfg(not(feature = "audio"))]
impl InputSample for f32 {
    #[inline]
    fn to_unit_f32(self) -> f32 {
        self
    }
}

#[cfg(not(feature = "audio"))]
impl InputSample for i16 {
    #[inline]
    fn to_unit_f32(self) -> f32 {
        f32::from(self) / 32_768.0
    }
}

#[cfg(not(feature = "audio"))]
impl InputSample for i32 {
    #[inline]
    fn to_unit_f32(self) -> f32 {
        (self as f64 / 2_147_483_648.0) as f32
    }
}

const INPUT_HOLD_SECONDS: f64 = 1.5;
const INPUT_CLIP_THRESHOLD: f32 = 0.9999;
/// Click-free software monitor fade time (`1.5 ms`), scaled by output sample rate.
const MONITOR_RAMP_SEC: f64 = 0.0015;
/// Drift-guard high-water latency threshold (`45 ms`) in input frames.
const MONITOR_HIGH_WATER_SEC: f64 = 0.045;
/// Post-trim target ring depth (`6 ms`) in input frames.
const MONITOR_TARGET_DEPTH_SEC: f64 = 0.006;
/// Minimum input-buffer multiple allowed before triggering the clock-drift guard.
const MONITOR_HIGH_WATER_BUFFER_MULT: u64 = 3;
/// Minimum high-water and target depths in frames for ultra-low sample rates / tiny callbacks.
const MONITOR_MIN_HIGH_WATER_FRAMES: u64 = 512;
const MONITOR_MIN_TARGET_DEPTH_FRAMES: u64 = 64;

/// Compute `(high_water, target_depth)` in input frames scaled cleanly to `(in_sr, in_buf)`
/// and bounded inside `AUDIO_MONITOR_CAPACITY`.
#[inline]
pub(super) fn monitor_watermarks(in_sr: f64, in_buf: u64) -> (u64, u64) {
    let sr = sanitize_sample_rate(in_sr);
    let max_high_water = (AUDIO_MONITOR_CAPACITY * 3 / 4) as u64;
    let high_water = ((sr * MONITOR_HIGH_WATER_SEC).round() as u64)
        .max(in_buf.saturating_mul(MONITOR_HIGH_WATER_BUFFER_MULT))
        .clamp(MONITOR_MIN_HIGH_WATER_FRAMES, max_high_water);
    let target_depth = ((sr * MONITOR_TARGET_DEPTH_SEC).round() as u64)
        .max(in_buf)
        .clamp(MONITOR_MIN_TARGET_DEPTH_FRAMES, high_water / 2);
    (high_water, target_depth)
}

/// Strictly RT-safe, zero-allocation state owned by the audio input callback thread.
pub(super) struct InputMeterState {
    peak: [f32; 2],
    peak_hold: [PeakHold; 2],
    clipped: bool,
    decay_coeff: f32,
    hold_decay_coeff: f32,
    hold_duration_frames: u32,
    sample_rate: u32,
}

impl InputMeterState {
    pub(super) fn new(sample_rate: u32) -> Self {
        let mut s = Self {
            peak: [0.0; 2],
            peak_hold: [PeakHold::new(); 2],
            clipped: false,
            decay_coeff: 0.999,
            hold_decay_coeff: 0.999,
            hold_duration_frames: 72_000,
            sample_rate: 0,
        };
        s.configure(sample_rate);
        s
    }

    #[inline]
    pub(super) fn configure(&mut self, sample_rate: u32) {
        let sr = sample_rate.clamp(MIN_AUDIO_SAMPLE_RATE, MAX_AUDIO_SAMPLE_RATE);
        if self.sample_rate != sr {
            self.sample_rate = sr;
            let sr_f = f64::from(sr);
            self.decay_coeff = one_pole_decay_ms(220.0, sr_f) as f32;
            self.hold_decay_coeff = one_pole_decay_ms(450.0, sr_f) as f32;
            self.hold_duration_frames = (INPUT_HOLD_SECONDS * sr_f).round() as u32;
        }
    }

    #[inline]
    pub(super) fn clear(&mut self) {
        self.peak = [0.0; 2];
        self.peak_hold[0].reset();
        self.peak_hold[1].reset();
        self.clipped = false;
    }

    #[inline]
    fn observe(&mut self, stereo: [f32; 2]) {
        for (ch, sample) in stereo.into_iter().enumerate() {
            let a = sample.abs();
            if a >= INPUT_CLIP_THRESHOLD {
                self.clipped = true;
            }
            self.peak[ch] = (self.peak[ch] * self.decay_coeff).max(a);
            self.peak_hold[ch].observe(
                a,
                self.peak[ch],
                self.hold_duration_frames,
                self.hold_decay_coeff,
                PeakHold::DEFAULT_ZERO_FLOOR,
            );
        }
    }
}

/// Extract routed stereo `[left, right]` from one interleaved hardware input frame.
#[inline]
fn route_input_frame<T: InputSample>(frame: &[T], mode: AudioInputMode) -> [f32; 2] {
    let ch0 = sanitize_sample(frame.first().map_or(0.0, |s| s.to_unit_f32()));
    let ch1 = if frame.len() >= 2 {
        sanitize_sample(frame[1].to_unit_f32())
    } else {
        ch0
    };
    match mode {
        AudioInputMode::Stereo => [ch0, ch1],
        AudioInputMode::MonoLeft => [ch0, ch0],
        AudioInputMode::MonoRight => [ch1, ch1],
    }
}

/// RT-safe audio input callback body shared by live CPAL input streams and deterministic tests.
///
/// Never allocates, never acquires locks, and never panics.
pub(super) fn process_input_slice<T: InputSample>(
    state: &mut InputMeterState,
    shared: &Shared,
    data: &[T],
    channels: usize,
    sample_rate: u32,
) {
    if channels == 0 || data.is_empty() {
        return;
    }
    let frames = data.len() / channels;
    if frames == 0 {
        return;
    }

    state.configure(sample_rate);
    shared
        .input_buffer_frames
        .store(frames as u32, Ordering::Relaxed);
    shared
        .input_sample_rate
        .store(sample_rate, Ordering::Relaxed);
    shared
        .input_channels
        .store(channels.min(u16::MAX as usize) as u16, Ordering::Relaxed);
    shared
        .input_status
        .store(AudioInputStatus::Running as u8, Ordering::Release);

    if shared.input_clear_meters.swap(false, Ordering::AcqRel) {
        state.clear();
        shared.input_clipped.store(false, Ordering::Relaxed);
    }

    let mode = AudioInputMode::from_u8(shared.input_mode.load(Ordering::Relaxed));
    let monitor_mode = InputMonitorMode::from_u8(shared.input_monitor_mode.load(Ordering::Relaxed));
    let mut phase = CapturePhase::from_u8(shared.capture_phase.load(Ordering::Acquire));

    if phase == CapturePhase::Finishing && shared.capture_tail_frames.load(Ordering::Acquire) == 0 {
        let _ = shared.capture_phase.compare_exchange(
            CapturePhase::Finishing as u8,
            CapturePhase::Idle as u8,
            Ordering::AcqRel,
            Ordering::Relaxed,
        );
        phase = CapturePhase::Idle;
    }

    let monitoring = match monitor_mode {
        InputMonitorMode::Off => false,
        InputMonitorMode::On => true,
        InputMonitorMode::Auto => matches!(
            phase,
            CapturePhase::Armed | CapturePhase::Recording | CapturePhase::Finishing
        ),
    };
    shared
        .input_monitoring_active
        .store(monitoring, Ordering::Release);

    let mut cap_write = shared.capture_write.load(Ordering::Relaxed);
    let cap_read = shared.capture_read.load(Ordering::Acquire);
    let mut cap_written_delta = 0_u64;
    let mut cap_dropped_delta = 0_u64;
    let mut tail_remaining = if phase == CapturePhase::Finishing {
        shared.capture_tail_frames.load(Ordering::Acquire)
    } else {
        0
    };

    let mut mon_write = shared.monitor_write.load(Ordering::Relaxed);
    let mon_read = shared.monitor_read.load(Ordering::Acquire);
    let mut any_nonzero = false;

    for frame_idx in 0..frames {
        let offset = frame_idx * channels;
        let stereo = route_input_frame(&data[offset..offset + channels], mode);
        if stereo[0] != 0.0 || stereo[1] != 0.0 {
            any_nonzero = true;
        }
        state.observe(stereo);

        let should_capture = match phase {
            CapturePhase::Recording => true,
            CapturePhase::Finishing => {
                if tail_remaining > 0 {
                    tail_remaining -= 1;
                    true
                } else {
                    false
                }
            }
            CapturePhase::Idle | CapturePhase::Armed => false,
        };
        if should_capture {
            if cap_write.wrapping_sub(cap_read) < AUDIO_CAPTURE_CAPACITY as u64 {
                let slot = (cap_write as usize) & AUDIO_CAPTURE_MASK;
                shared.capture_ring[slot][0].store(stereo[0].to_bits(), Ordering::Relaxed);
                shared.capture_ring[slot][1].store(stereo[1].to_bits(), Ordering::Relaxed);
                cap_write = cap_write.wrapping_add(1);
                cap_written_delta += 1;
            } else {
                cap_dropped_delta += 1;
            }
        }

        if monitoring && mon_write.wrapping_sub(mon_read) < AUDIO_MONITOR_CAPACITY as u64 {
            let slot = (mon_write as usize) & AUDIO_MONITOR_MASK;
            shared.monitor_ring[slot][0].store(stereo[0].to_bits(), Ordering::Relaxed);
            shared.monitor_ring[slot][1].store(stereo[1].to_bits(), Ordering::Relaxed);
            mon_write = mon_write.wrapping_add(1);
        }
    }

    if cap_written_delta > 0 {
        shared.capture_write.store(cap_write, Ordering::Release);
        shared
            .capture_frames_written
            .fetch_add(cap_written_delta, Ordering::Release);
    }
    if cap_dropped_delta > 0 {
        shared
            .capture_dropped
            .fetch_add(cap_dropped_delta, Ordering::Relaxed);
    }
    if phase == CapturePhase::Finishing {
        shared
            .capture_tail_frames
            .store(tail_remaining, Ordering::Release);
        if tail_remaining == 0 {
            let _ = shared.capture_phase.compare_exchange(
                CapturePhase::Finishing as u8,
                CapturePhase::Idle as u8,
                Ordering::AcqRel,
                Ordering::Relaxed,
            );
        }
    }
    if monitoring {
        shared.monitor_write.store(mon_write, Ordering::Release);
    }

    shared.input_peak[0].store(state.peak[0].to_bits(), Ordering::Relaxed);
    shared.input_peak[1].store(state.peak[1].to_bits(), Ordering::Relaxed);
    shared.input_peak_hold[0].store(state.peak_hold[0].value().to_bits(), Ordering::Relaxed);
    shared.input_peak_hold[1].store(state.peak_hold[1].value().to_bits(), Ordering::Relaxed);
    if state.clipped {
        shared.input_clipped.store(true, Ordering::Relaxed);
    }
    shared
        .input_frames_seen
        .fetch_add(frames as u64, Ordering::Relaxed);
    if any_nonzero {
        shared.input_nonzero_seen.store(true, Ordering::Relaxed);
    }
}

/// Strictly RT-safe, SPSC consumer draining `Shared::monitor_ring` into the output
/// audio callback with fractional linear resampling and clock-drift protection.
pub(super) struct InputMonitorDrain {
    active_gain: f32,
    prev_frame: [f32; 2],
    next_frame: [f32; 2],
    primed: bool,
    frac: f64,
}

impl InputMonitorDrain {
    pub(super) fn new() -> Self {
        Self {
            active_gain: 0.0,
            prev_frame: [0.0; 2],
            next_frame: [0.0; 2],
            primed: false,
            frac: 0.0,
        }
    }

    #[inline]
    pub(super) fn reset(&mut self) {
        self.active_gain = 0.0;
        self.prev_frame = [0.0; 2];
        self.next_frame = [0.0; 2];
        self.primed = false;
        self.frac = 0.0;
    }

    #[inline]
    fn pop_frame(shared: &Shared, read: &mut u64, write: u64) -> Option<[f32; 2]> {
        if write.wrapping_sub(*read) == 0 {
            return None;
        }
        let slot = (*read as usize) & AUDIO_MONITOR_MASK;
        let l = f32::from_bits(shared.monitor_ring[slot][0].load(Ordering::Relaxed));
        let r = f32::from_bits(shared.monitor_ring[slot][1].load(Ordering::Relaxed));
        *read = read.wrapping_add(1);
        Some([l, r])
    }

    #[inline]
    pub(super) fn next_frame(&mut self, shared: &Shared, output_sample_rate: f64) -> [f32; 2] {
        let want_monitor = shared.input_monitoring_active.load(Ordering::Acquire);
        if !want_monitor && self.active_gain <= 0.0 {
            let write = shared.monitor_write.load(Ordering::Acquire);
            if shared.monitor_read.load(Ordering::Relaxed) != write {
                shared.monitor_read.store(write, Ordering::Release);
            }
            self.primed = false;
            return [0.0, 0.0];
        }

        let out_sr = sanitize_sample_rate(output_sample_rate);
        let ramp_step = (1.0 / (out_sr * MONITOR_RAMP_SEC).round().max(1.0)) as f32;
        let in_sr_raw = shared.input_sample_rate.load(Ordering::Relaxed);
        let in_sr_f = if in_sr_raw >= MIN_AUDIO_SAMPLE_RATE {
            f64::from(in_sr_raw.min(MAX_AUDIO_SAMPLE_RATE))
        } else {
            out_sr
        };
        let step = if in_sr_raw >= MIN_AUDIO_SAMPLE_RATE && output_sample_rate > 0.0 {
            (in_sr_f / out_sr).clamp(0.25, 4.0)
        } else {
            1.0
        };
        let in_buf = u64::from(shared.input_buffer_frames.load(Ordering::Relaxed));
        let out_buf_in =
            (f64::from(shared.buffer_frames.load(Ordering::Relaxed)) * step).ceil() as u64;
        let (high_water, target_depth) = monitor_watermarks(in_sr_f, in_buf.max(out_buf_in));

        let mut read = shared.monitor_read.load(Ordering::Relaxed);
        let write = shared.monitor_write.load(Ordering::Acquire);
        let avail = write.wrapping_sub(read);

        // Guard against clock drift between separate input & output audio hardware.
        if avail > high_water {
            read = write.wrapping_sub(target_depth);
            self.primed = false;
            self.frac = 0.0;
        }

        if !self.primed {
            if let (Some(a), Some(b)) = (
                Self::pop_frame(shared, &mut read, write),
                Self::pop_frame(shared, &mut read, write),
            ) {
                self.prev_frame = a;
                self.next_frame = b;
                self.primed = true;
                self.frac = 0.0;
            } else {
                shared.monitor_read.store(read, Ordering::Release);
                self.active_gain = (self.active_gain - ramp_step).max(0.0);
                return [
                    self.next_frame[0] * self.active_gain,
                    self.next_frame[1] * self.active_gain,
                ];
            }
        }

        let mut starved = false;
        while self.frac >= 1.0 {
            if let Some(next) = Self::pop_frame(shared, &mut read, write) {
                self.prev_frame = self.next_frame;
                self.next_frame = next;
                self.frac -= 1.0;
            } else {
                self.frac = 1.0;
                starved = true;
                break;
            }
        }
        shared.monitor_read.store(read, Ordering::Release);

        let t = self.frac.clamp(0.0, 1.0) as f32;
        let sample = [
            self.prev_frame[0] + (self.next_frame[0] - self.prev_frame[0]) * t,
            self.prev_frame[1] + (self.next_frame[1] - self.prev_frame[1]) * t,
        ];
        if !starved {
            self.frac += step;
        }

        if want_monitor && !starved {
            self.active_gain = (self.active_gain + ramp_step).min(1.0);
        } else {
            self.active_gain = (self.active_gain - ramp_step).max(0.0);
            if !want_monitor && self.active_gain <= 0.0 {
                let latest = shared.monitor_write.load(Ordering::Acquire);
                shared.monitor_read.store(latest, Ordering::Release);
                self.primed = false;
            }
        }

        [
            sanitize_sample(sample[0] * self.active_gain),
            sanitize_sample(sample[1] * self.active_gain),
        ]
    }
}

#[cfg(feature = "audio")]
pub(super) struct ResolvedInput {
    pub(super) device: cpal::Device,
    pub(super) device_name: String,
    pub(super) config: cpal::StreamConfig,
    pub(super) sample_rate: u32,
    pub(super) channels: u16,
    pub(super) sample_format: cpal::SampleFormat,
    pub(super) buffer_frames: u32,
}

#[cfg(feature = "audio")]
pub(super) fn resolve_input_stream(
    requested_name: &str,
    preferred_sample_rate: Option<u32>,
    preferred_buffer_frames: Option<u32>,
) -> anyhow::Result<ResolvedInput> {
    use super::runtime::{match_device_name_index, resolve_target_buffer_frames};
    use anyhow::{bail, Context};
    use cpal::traits::{DeviceTrait, HostTrait};

    let host = cpal::default_host();
    let devices: Vec<(String, cpal::Device)> = host
        .input_devices()
        .context("enumerating audio input devices")?
        .map(|dev| {
            let name = dev.name().unwrap_or_else(|_| "Unknown Input".into());
            (name, dev)
        })
        .collect();
    if devices.is_empty() {
        bail!("no audio input devices available");
    }

    let names: Vec<&str> = devices.iter().map(|(n, _)| n.as_str()).collect();
    let idx = match_device_name_index(&names, requested_name)
        .with_context(|| format!("audio input device '{requested_name}' not found"))?;
    let (device_name, device) = devices
        .into_iter()
        .nth(idx)
        .context("input device missing")?;

    let default_cfg = device
        .default_input_config()
        .with_context(|| format!("reading default input format for '{device_name}'"))?;
    let supported: Vec<cpal::SupportedStreamConfigRange> = device
        .supported_input_configs()
        .map(Iterator::collect)
        .unwrap_or_default();

    let desired_sr = preferred_sample_rate
        .filter(|&r| (MIN_AUDIO_SAMPLE_RATE..=MAX_AUDIO_SAMPLE_RATE).contains(&r))
        .unwrap_or_else(|| default_cfg.sample_rate().0);

    // Prefer a supported range matching desired_sr (with >= 2 channels if available, else default),
    // falling back cleanly to the hardware default format.
    let chosen_range = supported
        .iter()
        .filter(|r| r.min_sample_rate().0 <= desired_sr && desired_sr <= r.max_sample_rate().0)
        .max_by_key(|r| {
            let ch_score = if r.channels() == 2 {
                3
            } else if r.channels() == default_cfg.channels() {
                2
            } else if r.channels() >= 1 {
                1
            } else {
                0
            };
            let fmt_score = match r.sample_format() {
                cpal::SampleFormat::F32 => 4,
                cpal::SampleFormat::I32 => 3,
                cpal::SampleFormat::I16 => 2,
                _ => 1,
            };
            (ch_score, fmt_score)
        });

    let (sample_rate, channels, sample_format, buffer_range) = if let Some(range) = chosen_range {
        let br = match range.buffer_size() {
            cpal::SupportedBufferSize::Range { min, max } => Some((*min, *max)),
            cpal::SupportedBufferSize::Unknown => None,
        };
        (desired_sr, range.channels(), range.sample_format(), br)
    } else {
        let br = match default_cfg.buffer_size() {
            cpal::SupportedBufferSize::Range { min, max } => Some((*min, *max)),
            cpal::SupportedBufferSize::Unknown => None,
        };
        (
            default_cfg.sample_rate().0,
            default_cfg.channels(),
            default_cfg.sample_format(),
            br,
        )
    };

    // Use the OS hardware's native input buffer size when Buffer Size is `Auto` (`None`) so
    // CoreAudio/Bluetooth HFP/USB interfaces never fight an explicit fixed input buffer size.
    let target_buf = preferred_buffer_frames.and_then(|_| {
        resolve_target_buffer_frames(preferred_buffer_frames, buffer_range, sample_rate)
    });
    let buffer_size = match target_buf {
        Some(frames) => cpal::BufferSize::Fixed(frames),
        None => cpal::BufferSize::Default,
    };
    let config = cpal::StreamConfig {
        channels,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size,
    };

    Ok(ResolvedInput {
        device,
        device_name,
        config,
        sample_rate,
        channels,
        sample_format,
        buffer_frames: target_buf.unwrap_or(0),
    })
}

#[cfg(feature = "audio")]
pub(super) fn build_input_stream_for_format(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    shared: std::sync::Arc<Shared>,
) -> anyhow::Result<cpal::Stream> {
    use anyhow::{bail, Context};
    use cpal::traits::DeviceTrait;

    let channels = usize::from(config.channels.max(1));
    let sample_rate = config.sample_rate.0.max(MIN_AUDIO_SAMPLE_RATE);

    fn build_typed<T>(
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        channels: usize,
        sample_rate: u32,
        shared: std::sync::Arc<Shared>,
    ) -> anyhow::Result<cpal::Stream>
    where
        T: cpal::SizedSample,
        f32: cpal::FromSample<T>,
    {
        let mut state = InputMeterState::new(sample_rate);
        let data_shared = std::sync::Arc::clone(&shared);
        let err_shared = std::sync::Arc::clone(&shared);
        device
            .build_input_stream(
                config,
                move |data: &[T], _| {
                    process_input_slice(&mut state, &data_shared, data, channels, sample_rate);
                },
                move |err| {
                    err_shared
                        .input_status
                        .store(AudioInputStatus::Failed as u8, Ordering::Release);
                    err_shared
                        .input_monitoring_active
                        .store(false, Ordering::Release);
                    err_shared.set_input_last_error(Some(format!("Audio input error: {err}")));
                },
                None,
            )
            .context("building CPAL audio input stream")
    }

    match sample_format {
        cpal::SampleFormat::F32 => {
            build_typed::<f32>(device, config, channels, sample_rate, shared)
        }
        cpal::SampleFormat::F64 => {
            build_typed::<f64>(device, config, channels, sample_rate, shared)
        }
        cpal::SampleFormat::I32 => {
            build_typed::<i32>(device, config, channels, sample_rate, shared)
        }
        cpal::SampleFormat::I16 => {
            build_typed::<i16>(device, config, channels, sample_rate, shared)
        }
        cpal::SampleFormat::U16 => {
            build_typed::<u16>(device, config, channels, sample_rate, shared)
        }
        other => bail!("unsupported audio input sample format: {other:?}"),
    }
}
