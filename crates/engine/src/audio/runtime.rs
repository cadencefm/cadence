//! Live CPAL audio device runtime, RT-safe callback watchdog/circuit breaker,
//! explicit device/rate/buffer selection, hot-plug detection, and silent fallback clock.
use super::engine::Engine;
#[cfg(feature = "audio")]
use super::input::{build_input_stream_for_format, resolve_input_stream};
use super::input::{process_input_slice, InputMeterState};
use super::shared::*;
use super::*;
use cadence_core::dsp::{hann_fade_out, sanitize_sample};
use cadence_core::mixer::MAX_EFFECT_DEVICES;
use cadence_core::rack::{compile_zone_graph, ModuleKind, RackPatch, RackZone, SampleLoopMode};
use cadence_core::runtime::{
    default_auto_buffer_frames, AudioErrorCode, AudioInputRequest, AudioInputStatus,
    AudioInventory, AudioStreamRequest, CaptureArmSpec, CapturePhase, EffectMeter,
    MAX_AUDIO_BUFFER_FRAMES, MAX_AUDIO_SAMPLE_RATE, MIN_AUDIO_BUFFER_FRAMES, MIN_AUDIO_SAMPLE_RATE,
};
#[cfg(feature = "audio")]
use cadence_core::runtime::{AudioDeviceInfo, STANDARD_SAMPLE_RATES};
use std::sync::atomic::{fence, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Pre-allocated stereo frame capacity for the overload fade-out tail ring (`1024` frames).
pub(super) const OVERLOAD_FADE_BUF_CAPACITY: usize = 1024;
/// Click-free overload tail fade-out duration (`~2.67 ms`: `128` fr @ 48 kHz, `~512` fr @ 192 kHz).
const OVERLOAD_FADE_SEC: f64 = 0.00267;
/// Minimum consecutive overruns required alongside [`OVERLOAD_SUSTAINED_SEC`].
const OVERLOAD_MIN_CONSECUTIVE: u32 = 3;
/// Cumulative overrun audio-time threshold (`50 ms`) before tripping the circuit breaker.
const OVERLOAD_SUSTAINED_SEC: f64 = 0.050;
const OVERLOAD_SEVERE_MULTIPLIER: u32 = 4;
const OVERLOAD_ACTIVE_PEAK_THRESHOLD: f32 = 1e-4;
/// Smoothing time constant (`120 ms`) for callback CPU load EMA.
const WATCHDOG_EMA_TAU_SEC: f64 = 0.12;

/// Compute the click-free overload fade-out length in frames scaled to `sample_rate` Hz,
/// bounded by [`OVERLOAD_FADE_BUF_CAPACITY`].
#[cfg_attr(not(feature = "audio"), allow(dead_code))]
#[inline]
pub(super) fn overload_fade_frames(sample_rate: u32) -> usize {
    let sr = f64::from(sample_rate.clamp(MIN_AUDIO_SAMPLE_RATE, MAX_AUDIO_SAMPLE_RATE));
    ((sr * OVERLOAD_FADE_SEC).round() as usize).clamp(16, OVERLOAD_FADE_BUF_CAPACITY)
}

/// Detected OS audio device, stream format, or hardware availability change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioDeviceChange {
    DefaultOutputChanged {
        from: String,
        to: String,
    },
    SampleRateChanged {
        device: String,
        from: u32,
        to: u32,
    },
    FormatChanged {
        device: String,
        from_channels: u16,
        to_channels: u16,
    },
    DeviceLost {
        device: String,
    },
    StreamFailed {
        message: String,
    },
}

impl AudioDeviceChange {
    /// Human-readable one-line description for status bars and logs.
    pub fn summary(&self) -> String {
        match self {
            Self::DefaultOutputChanged { from, to } => {
                format!("Default audio output changed: {from} -> {to}")
            }
            Self::SampleRateChanged { device, from, to } => {
                format!("{device} sample rate changed: {from} Hz -> {to} Hz")
            }
            Self::FormatChanged {
                device,
                from_channels,
                to_channels,
            } => {
                format!("{device} channels changed: {from_channels} ch -> {to_channels} ch")
            }
            Self::DeviceLost { device } => {
                format!("Audio output device disconnected: {device}")
            }
            Self::StreamFailed { message } => {
                format!("Audio stream failed: {message}")
            }
        }
    }
}

/// Output sample conversion abstraction shared by live CPAL streams and
/// feature-independent unit tests.
#[cfg_attr(not(feature = "audio"), allow(dead_code))]
pub(super) trait OutputSample: Copy {
    const EQUILIBRIUM: Self;
    fn from_unit_f32(sample: f32) -> Self;
}

#[cfg(feature = "audio")]
impl<T: cpal::Sample + cpal::FromSample<f32>> OutputSample for T {
    const EQUILIBRIUM: Self = T::EQUILIBRIUM;

    #[inline]
    fn from_unit_f32(sample: f32) -> Self {
        T::from_sample(sample)
    }
}

#[cfg(not(feature = "audio"))]
impl OutputSample for f32 {
    const EQUILIBRIUM: Self = 0.0;

    #[inline]
    fn from_unit_f32(sample: f32) -> Self {
        sample
    }
}

#[cfg(not(feature = "audio"))]
impl OutputSample for i16 {
    const EQUILIBRIUM: Self = 0;

    #[inline]
    fn from_unit_f32(sample: f32) -> Self {
        (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16
    }
}

/// Lock-free, zero-allocation audio callback budget monitor and overload circuit breaker.
///
/// Tracks smoothed EMA CPU load, decaying recent peak load, cumulative xruns, and
/// consecutive callback overruns. When sustained overruns or a severe stall occur,
/// [`render_output_slice_with_elapsed`] ramps the block tail smoothly to exact silence,
/// pauses transport, silences voices/FX in place, and latches a zero-DSP silence fast path
/// until the user presses Play or Reset.
#[cfg_attr(not(feature = "audio"), allow(dead_code))]
pub(super) struct CallbackWatchdog {
    ema_load: f64,
    peak_load: f64,
    peak_hold_secs: f64,
    initialized: bool,
    consecutive_overruns: u32,
    overrun_secs: f64,
    was_tripped: bool,
    tail_stereo: Box<[[f32; 2]]>,
}

#[cfg_attr(not(feature = "audio"), allow(dead_code))]
impl CallbackWatchdog {
    pub(super) fn new() -> Self {
        Self {
            ema_load: 0.0,
            peak_load: 0.0,
            peak_hold_secs: 0.0,
            initialized: false,
            consecutive_overruns: 0,
            overrun_secs: 0.0,
            was_tripped: false,
            tail_stereo: vec![[0.0; 2]; OVERLOAD_FADE_BUF_CAPACITY].into_boxed_slice(),
        }
    }

    #[inline]
    fn record_load(&mut self, elapsed: Duration, budget: Duration, shared: &Shared) {
        let budget_secs = budget.as_secs_f64();
        let ratio = if budget_secs > 0.0 {
            (elapsed.as_secs_f64() / budget_secs).clamp(0.0, 10.0)
        } else {
            0.0
        };
        if !self.initialized {
            self.ema_load = ratio;
            self.peak_load = ratio;
            self.peak_hold_secs = 1.0;
            self.initialized = true;
        } else {
            let alpha = (1.0 - (-budget_secs / WATCHDOG_EMA_TAU_SEC).exp()).clamp(1e-4, 1.0);
            self.ema_load += (ratio - self.ema_load) * alpha;
            if ratio >= self.peak_load {
                self.peak_load = ratio;
                self.peak_hold_secs = 1.0;
            } else if self.peak_hold_secs > 0.0 {
                self.peak_hold_secs = (self.peak_hold_secs - budget_secs).max(0.0);
            } else {
                let decay = (-budget_secs / 0.6).exp();
                self.peak_load = (self.peak_load * decay).max(self.ema_load);
            }
        }
        let ema_permille = (self.ema_load * 1000.0).round().clamp(0.0, 10_000.0) as u32;
        let peak_permille = (self.peak_load * 1000.0).round().clamp(0.0, 10_000.0) as u32;
        shared
            .cpu_load_permille
            .store(ema_permille, Ordering::Relaxed);
        shared
            .peak_cpu_permille
            .store(peak_permille, Ordering::Relaxed);
    }

    #[inline]
    fn observe_block(
        &mut self,
        elapsed: Duration,
        budget: Duration,
        active_audio: bool,
        shared: &Shared,
    ) -> bool {
        self.record_load(elapsed, budget, shared);
        if elapsed > budget {
            shared.xrun_count.fetch_add(1, Ordering::Relaxed);
            self.consecutive_overruns = self.consecutive_overruns.saturating_add(1);
            self.overrun_secs += budget.as_secs_f64();
        } else {
            self.consecutive_overruns = 0;
            self.overrun_secs = 0.0;
        }
        let severe_stall =
            active_audio && elapsed > budget.saturating_mul(OVERLOAD_SEVERE_MULTIPLIER);
        let sustained_overload = self.consecutive_overruns >= OVERLOAD_MIN_CONSECUTIVE
            && self.overrun_secs >= OVERLOAD_SUSTAINED_SEC;
        sustained_overload || severe_stall
    }
}

#[cfg_attr(not(feature = "audio"), allow(dead_code))]
#[inline]
fn write_stereo_frame<T: OutputSample>(frame: &mut [T], left: f32, right: f32) {
    if frame.len() == 1 {
        let mono = sanitize_sample((left + right) * 0.5).clamp(-1.0, 1.0);
        frame[0] = T::from_unit_f32(mono);
    } else {
        for (channel, sample) in frame.iter_mut().enumerate() {
            *sample = match channel {
                0 => T::from_unit_f32(sanitize_sample(left).clamp(-1.0, 1.0)),
                1 => T::from_unit_f32(sanitize_sample(right).clamp(-1.0, 1.0)),
                _ => T::EQUILIBRIUM,
            };
        }
    }
}

/// RT-safe output callback body measured with wall-clock [`Instant::now`].
#[cfg_attr(not(feature = "audio"), allow(dead_code))]
pub(super) fn render_output_slice<T: OutputSample>(
    engine: &mut Engine,
    watchdog: &mut CallbackWatchdog,
    shared: &Shared,
    data: &mut [T],
    channels: usize,
    sample_rate: u32,
) {
    let start = Instant::now();
    render_output_slice_with_elapsed(
        engine,
        watchdog,
        shared,
        data,
        channels,
        sample_rate,
        || start.elapsed(),
    );
}

/// RT-safe output callback body with injectable elapsed-duration measurement for deterministic tests.
#[cfg_attr(not(feature = "audio"), allow(dead_code))]
pub(super) fn render_output_slice_with_elapsed<T: OutputSample>(
    engine: &mut Engine,
    watchdog: &mut CallbackWatchdog,
    shared: &Shared,
    data: &mut [T],
    channels: usize,
    sample_rate: u32,
    measure_elapsed: impl FnOnce() -> Duration,
) {
    if channels == 0 || data.is_empty() {
        return;
    }
    let frames = data.len() / channels;
    if frames == 0 {
        data.fill(T::EQUILIBRIUM);
        return;
    }

    shared.buffer_frames.store(frames as u32, Ordering::Relaxed);
    let budget =
        Duration::from_secs_f64(frames as f64 / f64::from(sample_rate.max(MIN_AUDIO_SAMPLE_RATE)));

    engine.poll_controls();
    if shared.overload_tripped.load(Ordering::Acquire) {
        watchdog.was_tripped = true;
        data.fill(T::EQUILIBRIUM);
        watchdog.record_load(measure_elapsed(), budget, shared);
        return;
    }
    if watchdog.was_tripped {
        watchdog.was_tripped = false;
        watchdog.consecutive_overruns = 0;
        watchdog.overrun_secs = 0.0;
    }

    let was_playing = shared.playing.load(Ordering::Relaxed);
    let fade_frames = overload_fade_frames(sample_rate).min(watchdog.tail_stereo.len());
    let tail_start = frames.saturating_sub(fade_frames);
    let mut block_peak = 0.0_f32;

    for frame_idx in 0..frames {
        let [left, right] = engine.next_frame();
        let cl = sanitize_sample(left).clamp(-1.0, 1.0);
        let cr = sanitize_sample(right).clamp(-1.0, 1.0);
        block_peak = block_peak.max(cl.abs()).max(cr.abs());
        if frame_idx >= tail_start {
            if let Some(slot) = watchdog.tail_stereo.get_mut(frame_idx - tail_start) {
                *slot = [cl, cr];
            }
        }
        let offset = frame_idx * channels;
        write_stereo_frame(&mut data[offset..offset + channels], cl, cr);
    }
    let used = frames * channels;
    if used < data.len() {
        data[used..].fill(T::EQUILIBRIUM);
    }

    let elapsed = measure_elapsed();
    let active_audio = was_playing
        || shared.playing.load(Ordering::Relaxed)
        || block_peak > OVERLOAD_ACTIVE_PEAK_THRESHOLD;
    if watchdog.observe_block(elapsed, budget, active_audio, shared) {
        let ramp_len = frames.min(fade_frames);
        for (i, &[l, r]) in watchdog.tail_stereo.iter().take(ramp_len).enumerate() {
            let frame_idx = tail_start + i;
            let offset = frame_idx * channels;
            if i + 1 == ramp_len {
                data[offset..offset + channels].fill(T::EQUILIBRIUM);
            } else {
                let progress = (i + 1) as f32 / ramp_len as f32;
                let gain = hann_fade_out(progress);
                write_stereo_frame(&mut data[offset..offset + channels], l * gain, r * gain);
            }
        }
        shared.playing.store(false, Ordering::Release);
        engine.silence_in_place();
        shared.error_code.store(
            AudioErrorCode::OverloadCircuitBreaker as u8,
            Ordering::Release,
        );
        shared.overload_tripped.store(true, Ordering::Release);
        watchdog.was_tripped = true;
        watchdog.consecutive_overruns = 0;
        watchdog.overrun_secs = 0.0;
    }
}

/// Resolve desired callback buffer frame count clamped to a hardware `(min, max)` range.
#[cfg_attr(not(feature = "audio"), allow(dead_code))]
pub(super) fn resolve_target_buffer_frames(
    requested: Option<u32>,
    range: Option<(u32, u32)>,
    sample_rate: u32,
) -> Option<u32> {
    let target = requested
        .unwrap_or_else(|| default_auto_buffer_frames(sample_rate))
        .clamp(MIN_AUDIO_BUFFER_FRAMES, MAX_AUDIO_BUFFER_FRAMES);
    range.map(|(min, max)| {
        let lo = min.min(max);
        let hi = min.max(max);
        target.clamp(lo, hi)
    })
}

/// Find a device index by exact name first, then case-insensitive substring match.
#[cfg_attr(not(feature = "audio"), allow(dead_code))]
pub(super) fn match_device_name_index(names: &[&str], requested: &str) -> Option<usize> {
    let needle = requested.trim();
    if needle.is_empty() {
        return None;
    }
    if let Some(idx) = names.iter().position(|&n| n == needle) {
        return Some(idx);
    }
    let needle_lower = needle.to_lowercase();
    names
        .iter()
        .position(|&n| n.to_lowercase().contains(&needle_lower))
}

/// Cloneable off-RT consumer handle for [`AudioRuntime`]'s lock-free stereo tap ring.
/// Allows background capture threads (`listen --live`) to drain audio without blocking
/// the GUI frame loop. Callers must enforce single-consumer access while active.
#[derive(Clone)]
pub struct AudioTapHandle {
    shared: Arc<Shared>,
}

impl AudioTapHandle {
    #[cfg(test)]
    pub(super) fn from_shared(shared: Arc<Shared>) -> Self {
        Self { shared }
    }

    pub fn sample_rate(&self) -> u32 {
        self.shared
            .sample_rate
            .load(Ordering::Acquire)
            .max(MIN_AUDIO_SAMPLE_RATE)
    }

    pub fn is_alive(&self) -> bool {
        self.shared.alive.load(Ordering::Acquire) && !self.shared.failed.load(Ordering::Acquire)
    }

    /// Arm the lock-free tap ring, discarding any previously unread frames without racing `tap_write`.
    pub fn start(&self) {
        self.shared.tap_enabled.store(false, Ordering::Release);
        let write = self.shared.tap_write.load(Ordering::Acquire);
        self.shared.tap_read.store(write, Ordering::Release);
        self.shared.tap_dropped.store(0, Ordering::Relaxed);
        self.shared.tap_enabled.store(true, Ordering::Release);
    }

    /// Drain all currently available stereo frames into `out`, returning dropped frames since last drain.
    pub fn drain(&self, out: &mut Vec<[f32; 2]>) -> u64 {
        let write = self.shared.tap_write.load(Ordering::Acquire);
        let read = self.shared.tap_read.load(Ordering::Relaxed);
        let available = write.wrapping_sub(read).min(AUDIO_TAP_CAPACITY as u64) as usize;
        if available > 0 {
            out.reserve(available);
            for offset in 0..available {
                let slot = (read.wrapping_add(offset as u64) as usize) & AUDIO_TAP_MASK;
                let left = f32::from_bits(self.shared.audio_tap[slot][0].load(Ordering::Relaxed));
                let right = f32::from_bits(self.shared.audio_tap[slot][1].load(Ordering::Relaxed));
                out.push([left, right]);
            }
            self.shared
                .tap_read
                .store(read.wrapping_add(available as u64), Ordering::Release);
        }
        self.shared.tap_dropped.swap(0, Ordering::Relaxed)
    }

    /// Disarm the lock-free tap ring so the audio callback does zero tap work.
    pub fn stop(&self) {
        self.shared.tap_enabled.store(false, Ordering::Release);
    }
}

/// Owns the device stream or the paced silent renderer.
pub struct AudioRuntime {
    shared: Arc<Shared>,
    description: String,
    active_request: AudioStreamRequest,
    #[cfg_attr(not(feature = "audio"), allow(dead_code))]
    active_output_name: Option<String>,
    #[cfg_attr(not(feature = "audio"), allow(dead_code))]
    active_output_is_default: bool,
    active_input_request: AudioInputRequest,
    active_input_name: Option<String>,
    synthetic_input_state: InputMeterState,
    silent_mode: bool,
    worker: Option<JoinHandle<()>>,
    #[cfg(feature = "audio")]
    stream: Option<cpal::Stream>,
    #[cfg(feature = "audio")]
    input_stream: Option<cpal::Stream>,
}

impl AudioRuntime {
    pub fn start(project: ScoreWindow, silent: bool) -> Result<Self> {
        let mut lib = SampleLibrary::discover(&[], None);
        // Live sessions may open incomplete projects for repair. Missing sampler
        // voices stay silent; callers needing diagnostics use load_project_samples.
        let report = load_project_samples(&project, &mut lib);
        Self::start_with_samples(project, report.bank, silent)
    }

    pub fn start_with_samples(
        project: ScoreWindow,
        samples: SampleBank,
        silent: bool,
    ) -> Result<Self> {
        Self::start_with_config(project, samples, silent, AudioStreamRequest::default())
    }

    /// Start the audio runtime with explicit preferred output device, sample rate,
    /// and callback buffer size (or a silent paced clock when `silent` is true).
    pub fn start_with_config(
        project: ScoreWindow,
        samples: SampleBank,
        silent: bool,
        request: AudioStreamRequest,
    ) -> Result<Self> {
        project.validate()?;
        if silent {
            return Self::start_silent(project, samples, request);
        }
        #[cfg(feature = "audio")]
        {
            Self::start_device(project, samples, request)
        }
        #[cfg(not(feature = "audio"))]
        {
            let _ = (project, samples, request);
            bail!(
                "native audio is disabled in this build; use --silent or build with the audio feature"
            )
        }
    }

    /// Re-open the active native audio output device cleanly after a stream error
    /// or hardware disconnect, preserving the current [`AudioStreamRequest`] and
    /// keeping existing [`AudioTapHandle`]s alive.
    pub fn restart_device(&mut self, project: ScoreWindow, samples: SampleBank) -> Result<()> {
        let request = self.active_request.clone();
        self.reconfigure_device(project, samples, request)
    }

    /// Validate and switch to a new output device, sample rate, or buffer size while
    /// preserving `Arc<Shared>` (so [`AudioTapHandle`] consumers stay connected).
    ///
    /// If the requested device or configuration is invalid, returns `Err` without
    /// tearing down the currently running audio stream.
    pub fn reconfigure_device(
        &mut self,
        project: ScoreWindow,
        samples: SampleBank,
        request: AudioStreamRequest,
    ) -> Result<()> {
        project.validate()?;
        #[cfg(feature = "audio")]
        {
            use cpal::traits::StreamTrait;

            // Validate and resolve hardware device + stream config BEFORE stopping the active stream.
            let resolved = resolve_output_stream(&request, true)?;

            // Stop active stream or silent clock cleanly so zero callbacks touch Shared concurrently.
            self.shared.playing.store(false, Ordering::Release);
            if let Some(stream) = self.stream.take() {
                let _ = stream.pause();
                drop(stream);
            }
            if let Some(worker) = self.worker.take() {
                self.shared.alive.store(false, Ordering::Release);
                let _ = worker.join();
            }

            self.shared.reset_for_reconfigure();
            self.shared.is_silent.store(false, Ordering::Release);
            apply_stream_metadata(&self.shared, &resolved);

            let engine = Engine::new(
                project,
                samples,
                resolved.sample_rate,
                Arc::clone(&self.shared),
            );
            let started = build_stream_for_format(
                &resolved.device,
                &resolved.config,
                resolved.sample_format,
                engine,
                Arc::clone(&self.shared),
            )
            .and_then(|stream| {
                stream.play().context("starting audio device")?;
                Ok(stream)
            });

            match started {
                Ok(stream) => {
                    self.stream = Some(stream);
                    self.silent_mode = false;
                    self.active_request = resolved.effective_request;
                    self.active_output_name = Some(resolved.device_name.clone());
                    self.active_output_is_default = resolved.is_default;
                    self.description = format_device_description(
                        &resolved.device_name,
                        resolved.sample_rate,
                        resolved.channels,
                        resolved.buffer_frames,
                    );
                    Ok(())
                }
                Err(err) => {
                    let msg = format!("{err:#}");
                    self.shared
                        .error_code
                        .store(AudioErrorCode::BackendError as u8, Ordering::Release);
                    self.shared.set_last_error(Some(msg));
                    self.shared.playing.store(false, Ordering::Release);
                    self.shared.failed.store(true, Ordering::Release);
                    self.stream = None;
                    self.silent_mode = false;
                    self.description = format!("Audio device failed · {}", resolved.device_name);
                    Err(err)
                }
            }
        }
        #[cfg(not(feature = "audio"))]
        {
            let _ = (project, samples, request);
            bail!(
                "native audio is disabled in this build; use --silent or build with the audio feature"
            )
        }
    }

    /// Whether the underlying CPAL audio output stream reported a fatal device error.
    pub fn is_failed(&self) -> bool {
        self.shared.failed.load(Ordering::Acquire)
    }

    /// Active stream configuration request (device pin, preferred rate, preferred buffer).
    pub fn active_request(&self) -> &AudioStreamRequest {
        &self.active_request
    }

    /// Last off-RT stream error or diagnostic warning, falling back to the latched
    /// atomic [`AudioErrorCode`] message when overloaded or failed.
    pub fn last_error(&self) -> Option<String> {
        if let Some(err) = self.shared.last_error() {
            return Some(err);
        }
        let code = AudioErrorCode::from_u8(self.shared.error_code.load(Ordering::Acquire));
        code.message().map(str::to_owned)
    }

    /// Immediately silence all ringing voices and FX tails at the current step and
    /// clear any latched overload circuit breaker.
    pub fn panic_silence(&self) {
        let (step, _) = unpack_step_position(self.shared.step_position.load(Ordering::Acquire));
        for track in self.shared.note_owned.iter() {
            for owned in track {
                owned.store(false, Ordering::Release);
            }
        }
        if !self.transport(TransportIntent::Reset { step }) {
            self.shared.playing.store(false, Ordering::Release);
            self.shared.request_release_all();
            self.shared.clear_overload_latch();
        }
    }

    /// Enumerate available OS audio input/output devices and live active stream properties.
    pub fn enumerate_inventory(&self) -> AudioInventory {
        if self.silent_mode {
            let input_tel = self.shared.read_input_telemetry();
            return AudioInventory {
                active_sample_rate: self.sample_rate(),
                active_format_label: "Silent".into(),
                active_input: self.active_input_name.clone(),
                active_input_sample_rate: input_tel.sample_rate,
                active_input_channels: input_tel.channels,
                active_input_buffer_frames: input_tel.buffer_frames,
                silent_mode: true,
                last_error: self.last_error(),
                input_last_error: self.shared.input_last_error(),
                ..AudioInventory::default()
            };
        }
        #[cfg(feature = "audio")]
        {
            use cpal::traits::{DeviceTrait, HostTrait};
            let host = cpal::default_host();
            let default_out_name = host.default_output_device().and_then(|d| d.name().ok());
            let default_in_name = host.default_input_device().and_then(|d| d.name().ok());

            let outputs = host
                .output_devices()
                .map(|devices| {
                    devices
                        .map(|dev| {
                            let name = dev.name().unwrap_or_else(|_| "Unknown Output".into());
                            let is_default = default_out_name.as_deref() == Some(name.as_str());
                            build_device_info(
                                name,
                                is_default,
                                dev.default_output_config().ok(),
                                dev.supported_output_configs().ok(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();

            let inputs = host
                .input_devices()
                .map(|devices| {
                    devices
                        .map(|dev| {
                            let name = dev.name().unwrap_or_else(|_| "Unknown Input".into());
                            let is_default = default_in_name.as_deref() == Some(name.as_str());
                            build_device_info(
                                name,
                                is_default,
                                dev.default_input_config().ok(),
                                dev.supported_input_configs().ok(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();

            let health = self.shared.read_health();
            let input_tel = self.shared.read_input_telemetry();
            AudioInventory {
                outputs,
                inputs,
                active_output: self.active_output_name.clone(),
                active_output_is_default: self.active_output_is_default,
                active_sample_rate: self.sample_rate(),
                active_buffer_frames: health.buffer_frames,
                active_channels: health.channels,
                active_format_label: health.format_label().into(),
                active_input: self.active_input_name.clone(),
                active_input_sample_rate: input_tel.sample_rate,
                active_input_channels: input_tel.channels,
                active_input_buffer_frames: input_tel.buffer_frames,
                silent_mode: false,
                last_error: self.last_error(),
                input_last_error: self.shared.input_last_error(),
            }
        }
        #[cfg(not(feature = "audio"))]
        {
            let input_tel = self.shared.read_input_telemetry();
            AudioInventory {
                active_sample_rate: self.sample_rate(),
                active_format_label: "Silent".into(),
                active_input: self.active_input_name.clone(),
                active_input_sample_rate: input_tel.sample_rate,
                active_input_channels: input_tel.channels,
                active_input_buffer_frames: input_tel.buffer_frames,
                silent_mode: true,
                last_error: self.last_error(),
                input_last_error: self.shared.input_last_error(),
                ..AudioInventory::default()
            }
        }
    }

    /// Off-RT hot-plug & format change detector polled periodically by the host loop.
    pub fn detect_device_change(&self) -> Option<AudioDeviceChange> {
        if self.is_failed() {
            let message = self
                .last_error()
                .unwrap_or_else(|| "Audio stream failed".into());
            return Some(AudioDeviceChange::StreamFailed { message });
        }
        if self.silent_mode {
            return None;
        }
        #[cfg(feature = "audio")]
        {
            use cpal::traits::{DeviceTrait, HostTrait};
            let host = cpal::default_host();
            let active_sr = self.shared.sample_rate.load(Ordering::Acquire);
            let active_ch = self.shared.channels.load(Ordering::Acquire);

            if let Some(req_name) = self
                .active_request
                .output_device
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let devices: Vec<(String, cpal::Device)> = host
                    .output_devices()
                    .ok()?
                    .map(|dev| {
                        let name = dev.name().unwrap_or_default();
                        (name, dev)
                    })
                    .collect();
                let names: Vec<&str> = devices.iter().map(|(n, _)| n.as_str()).collect();
                let lookup_name = self.active_output_name.as_deref().unwrap_or(req_name);
                let found = match_device_name_index(&names, lookup_name)
                    .or_else(|| match_device_name_index(&names, req_name));
                let Some(idx) = found else {
                    return Some(AudioDeviceChange::DeviceLost {
                        device: lookup_name.to_owned(),
                    });
                };
                if self.active_request.sample_rate.is_none() {
                    if let Ok(cfg) = devices[idx].1.default_output_config() {
                        let nominal_sr = cfg.sample_rate().0;
                        if nominal_sr != active_sr {
                            return Some(AudioDeviceChange::SampleRateChanged {
                                device: devices[idx].0.clone(),
                                from: active_sr,
                                to: nominal_sr,
                            });
                        }
                    }
                }
                return None;
            }

            // System Default mode: follow OS default switches and nominal sample-rate/channel changes.
            let Some(default_dev) = host.default_output_device() else {
                return Some(AudioDeviceChange::DeviceLost {
                    device: self
                        .active_output_name
                        .clone()
                        .unwrap_or_else(|| "Default device".into()),
                });
            };
            let cur_name = default_dev
                .name()
                .unwrap_or_else(|_| "Default device".into());
            if let Some(active_name) = &self.active_output_name {
                if cur_name != *active_name {
                    return Some(AudioDeviceChange::DefaultOutputChanged {
                        from: active_name.clone(),
                        to: cur_name,
                    });
                }
            }
            if self.active_request.sample_rate.is_none() {
                if let Ok(cfg) = default_dev.default_output_config() {
                    let nominal_sr = cfg.sample_rate().0;
                    if nominal_sr != active_sr {
                        return Some(AudioDeviceChange::SampleRateChanged {
                            device: cur_name,
                            from: active_sr,
                            to: nominal_sr,
                        });
                    }
                    if active_ch > 0 && cfg.channels() != active_ch {
                        return Some(AudioDeviceChange::FormatChanged {
                            device: cur_name,
                            from_channels: active_ch,
                            to_channels: cfg.channels(),
                        });
                    }
                }
            }
            None
        }
        #[cfg(not(feature = "audio"))]
        {
            None
        }
    }

    fn start_silent(
        project: ScoreWindow,
        samples: SampleBank,
        request: AudioStreamRequest,
    ) -> Result<Self> {
        let shared = Shared::new_arc();
        shared.is_silent.store(true, Ordering::Release);
        let mut engine = Engine::new(project, samples, SILENT_RATE, Arc::clone(&shared));
        let thread_shared = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("cadence-silent-audio".into())
            .spawn(move || {
                let quantum = Duration::from_secs_f64(SILENT_BLOCK as f64 / SILENT_RATE as f64);
                let mut deadline = Instant::now();
                while thread_shared.alive.load(Ordering::Acquire) {
                    engine.poll_controls();
                    for _ in 0..SILENT_BLOCK {
                        let _ = engine.next_frame();
                    }
                    deadline += quantum;
                    let now = Instant::now();
                    if deadline > now {
                        thread::sleep(deadline - now);
                    } else if now.duration_since(deadline) > Duration::from_millis(100) {
                        deadline = now;
                    }
                }
            })
            .context("starting silent audio clock")?;
        Ok(Self {
            shared,
            description: "Silent synth clock · 48 kHz · no audio device".into(),
            active_request: request,
            active_output_name: None,
            active_output_is_default: false,
            active_input_request: AudioInputRequest::default(),
            active_input_name: None,
            synthetic_input_state: InputMeterState::new(SILENT_RATE),
            silent_mode: true,
            worker: Some(worker),
            #[cfg(feature = "audio")]
            stream: None,
            #[cfg(feature = "audio")]
            input_stream: None,
        })
    }

    #[cfg(feature = "audio")]
    fn start_device(
        project: ScoreWindow,
        samples: SampleBank,
        request: AudioStreamRequest,
    ) -> Result<Self> {
        use cpal::traits::StreamTrait;
        let resolved = resolve_output_stream(&request, false)?;
        let shared = Shared::new_arc();
        shared.is_silent.store(false, Ordering::Release);
        if let Some(warning) = resolved.warning.clone() {
            shared.set_last_error(Some(warning));
        }
        apply_stream_metadata(&shared, &resolved);
        let engine = Engine::new(project, samples, resolved.sample_rate, Arc::clone(&shared));
        let stream = build_stream_for_format(
            &resolved.device,
            &resolved.config,
            resolved.sample_format,
            engine,
            Arc::clone(&shared),
        )?;
        stream.play().context("starting audio device")?;
        let description = format_device_description(
            &resolved.device_name,
            resolved.sample_rate,
            resolved.channels,
            resolved.buffer_frames,
        );
        Ok(Self {
            shared,
            description,
            active_request: resolved.effective_request,
            active_output_name: Some(resolved.device_name),
            active_output_is_default: resolved.is_default,
            active_input_request: AudioInputRequest::default(),
            active_input_name: None,
            synthetic_input_state: InputMeterState::new(resolved.sample_rate),
            silent_mode: false,
            worker: None,
            stream: Some(stream),
            input_stream: None,
        })
    }

    pub fn replace_project(&self, project: ScoreWindow) -> Result<()> {
        self.replace_project_with_assets(project, None, false)
    }

    /// Score and assets share one identity boundary, including queued/coalesced
    /// loads. A boundary without assets intentionally clears the old sample bank.
    pub fn replace_project_with_assets(
        &self,
        project: ScoreWindow,
        samples: Option<SampleBank>,
        project_boundary: bool,
    ) -> Result<()> {
        project.validate()?;
        if !self.shared.alive.load(Ordering::Acquire) {
            bail!("audio runtime is shut down");
        }
        let generation = if project_boundary {
            let previous = self
                .shared
                .input_generation
                .try_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| anyhow::anyhow!("Audio project identity space exhausted"))?;
            for track in self.shared.note_owned.iter() {
                for owned in track {
                    owned.store(false, Ordering::Release);
                }
            }
            previous + 1
        } else {
            self.shared.input_generation.load(Ordering::Acquire)
        };
        let extra_delays = self.shared.prepare_extra_delays(&project);
        self.shared.publish_input_tracks(&project);
        self.shared.project_slot.publish_merged(
            ProjectPublication {
                project: Some(Box::new(project)),
                samples: if project_boundary {
                    Some(Box::new(samples.unwrap_or_default()))
                } else {
                    samples.map(Box::new)
                },
                extra_delays,
                generation,
            },
            |next, previous| {
                // A routine edit following an unadopted load must carry its bank.
                if next.samples.is_none() && next.generation == previous.generation {
                    next.samples = previous.samples.take();
                }
                // Preserve any off-RT pre-allocated Delay buffers across coalesced publishes.
                if !previous.extra_delays.is_empty() {
                    let mut merged = std::mem::take(&mut previous.extra_delays);
                    merged.append(&mut next.extra_delays);
                    next.extra_delays = merged;
                }
            },
        );
        Ok(())
    }

    fn publish_audition(&self, kind: AuditionKind) {
        if self.shared.alive.load(Ordering::Acquire) {
            let sequence = self
                .shared
                .event_sequence
                .fetch_add(1, Ordering::AcqRel)
                .wrapping_add(1);
            self.shared.audition_slot.publish(Some(AuditionRequest {
                kind,
                sequence,
                project_generation: self.shared.input_generation.load(Ordering::Acquire),
            }));
        }
    }
    /// An absolute audition request: the latest pending selection wins. The
    /// prepared resource and its trigger travel together, avoiding stale buffers.
    pub fn preview_sample(&self, sample: Arc<SampleData>) {
        let mut zone = RackZone::blank_sample(1, "Preview");
        let root = sample.meta.root_midi.min(127);
        zone.root_midi = root;
        if let Some(ModuleKind::Sampler(smp)) = zone.modules.first_mut().map(|m| &mut m.kind) {
            smp.root_midi = root;
            smp.fine_cents = sample.meta.fine_cents.clamp(-100.0, 100.0);
            smp.sample_start = 0;
            smp.sample_end = sample.frames;
            smp.one_shot = true;
            smp.loop_mode = SampleLoopMode::Off;
        }
        if let Ok(graph) = compile_zone_graph(&zone) {
            self.publish_audition(AuditionKind::Sample {
                sample,
                zone,
                graph,
            });
        }
    }

    pub fn preview_rack(&self, patch: RackPatch, midi: u8, velocity: f32) -> Result<()> {
        patch.validate()?;
        if !velocity.is_finite() {
            bail!("Rack preview velocity must be finite");
        }
        let mut compiled_zones = Vec::with_capacity(patch.zones.len());
        for zone in &patch.zones {
            compiled_zones.push(compile_zone_graph(zone)?);
        }
        self.publish_audition(AuditionKind::Rack {
            patch,
            compiled_zones,
            midi: midi.min(127),
            velocity: velocity.clamp(0.0, 1.0),
        });
        Ok(())
    }
    /// Stop temporary browser/preset voices without pausing or resetting music.
    pub fn stop_audition(&self) {
        self.publish_audition(AuditionKind::Stop);
    }

    fn instrument_index(&self, id: u64) -> Option<usize> {
        (0..self
            .shared
            .input_track_count
            .load(Ordering::Acquire)
            .min(MAX_INSTRUMENTS))
            .find(|&i| self.shared.input_track_id(i) == Some(id))
    }
    pub fn audition_instrument(&self, id: u64, midi: u8, velocity: f32) -> bool {
        let Some(index) = self.instrument_index(id) else {
            return false;
        };
        if !velocity.is_finite() || midi > 127 {
            return false;
        }
        let vel = (velocity.clamp(0.0, 1.0) * 65_535.0).round() as u64;
        let packed = 1 | ((index as u64) << 3) | ((midi as u64) << 11) | (vel << 18);
        self.shared.push_event_for_id(packed, id)
    }
    pub fn live_note_on_instrument(
        &self,
        id: u64,
        midi: u8,
        velocity: f32,
        sustain: bool,
        source_part: Option<u64>,
    ) -> bool {
        let Some(index) = self.instrument_index(id) else {
            return false;
        };
        if !velocity.is_finite() || midi > 127 {
            return false;
        }
        let velocity = (velocity.clamp(0.0, 1.0) * 65535.0).round() as u64;
        let tag = if sustain { 2 } else { 1 };
        let packed = tag | ((index as u64) << 3) | ((midi as u64) << 11) | (velocity << 18);
        if sustain && self.shared.note_owned[index][usize::from(midi)].load(Ordering::Acquire) {
            let old = self.shared.note_owners[index][usize::from(midi)].load(Ordering::Relaxed);
            if old != id
                && !self
                    .shared
                    .push_event_for_id(3 | ((index as u64) << 3) | ((midi as u64) << 11), old)
            {
                self.shared.request_release_all();
                return false;
            }
        }
        let accepted = self.shared.push_bound_event(packed, id, source_part);
        if accepted && sustain {
            self.shared.note_owners[index][usize::from(midi)].store(id, Ordering::Relaxed);
            self.shared.note_owned[index][usize::from(midi)].store(true, Ordering::Release);
        }
        accepted
    }
    pub fn live_note_off_instrument(&self, id: u64, midi: u8) -> bool {
        if midi > 127 {
            return false;
        }
        let key = usize::from(midi);
        if let Some(index) = (0..MAX_INSTRUMENTS).find(|&i| {
            self.shared.note_owned[i][key].load(Ordering::Acquire)
                && self.shared.note_owners[i][key].load(Ordering::Relaxed) == id
        }) {
            let accepted = self
                .shared
                .push_event_for_id(3 | ((index as u64) << 3) | ((midi as u64) << 11), id);
            self.shared.note_owned[index][key].store(false, Ordering::Release);
            // NoteOff must never leave a voice held merely because auditions filled
            // the bounded queue. A rare overload releases all owned live voices.
            if !accepted {
                self.shared.request_release_all();
            }
        }
        true
    }

    pub fn snapshot(&self) -> AudioSnapshot {
        self.shared.reclaim_all();
        let end = self.shared.wave_write.load(Ordering::Acquire);
        let waveform = (0..WAVE_SAMPLES)
            .map(|index| {
                f32::from_bits(
                    self.shared.waveform[(end + index) % WAVE_SAMPLES].load(Ordering::Relaxed),
                )
            })
            .collect();

        // Seqlock reader with explicit Acquire fence and up to 3 retries for weak-order CPUs.
        let mut track_count = 0usize;
        let mut track_ids = [0u64; MAX_INSTRUMENTS];
        let mut track_waveforms: Vec<Vec<f32>> = Vec::new();
        for _ in 0..3 {
            let gen1 = self.shared.scope_generation.load(Ordering::Acquire);
            if !gen1.is_multiple_of(2) {
                std::hint::spin_loop();
                continue;
            }
            let seq1 = self.shared.scope_sequence.load(Ordering::Acquire);
            let page = self.shared.scope_page.load(Ordering::Acquire).min(1);
            let tc = self
                .shared
                .track_count
                .load(Ordering::Acquire)
                .min(MAX_INSTRUMENTS);
            let ids = std::array::from_fn(|i| self.shared.track_ids[i].load(Ordering::Relaxed));
            let waves: Vec<Vec<f32>> = (0..tc)
                .map(|track| {
                    (0..WAVE_SAMPLES)
                        .map(|i| {
                            f32::from_bits(
                                self.shared.track_waveform[track][page * WAVE_SAMPLES + i]
                                    .load(Ordering::Relaxed),
                            )
                        })
                        .collect()
                })
                .collect();
            fence(Ordering::Acquire);
            let gen2 = self.shared.scope_generation.load(Ordering::Relaxed);
            let seq2 = self.shared.scope_sequence.load(Ordering::Relaxed);
            if gen1 == gen2 && seq1 == seq2 {
                track_count = tc;
                track_ids = ids;
                track_waveforms = waves;
                break;
            }
        }

        let mut channel_meters = Vec::new();
        let mut effect_meters = [EffectMeter::default(); MAX_EFFECT_DEVICES];
        let mut effect_meter_count = 0usize;
        for _ in 0..3 {
            let gen1 = self.shared.mixer_meter_generation.load(Ordering::Acquire);
            if !gen1.is_multiple_of(2) {
                std::hint::spin_loop();
                continue;
            }
            let cm: Vec<_> = self
                .shared
                .mixer_meters
                .iter()
                .filter_map(SharedChannelMeter::read)
                .collect();
            let mut em = [EffectMeter::default(); MAX_EFFECT_DEVICES];
            let mut em_count = 0usize;
            for slot in self.shared.effect_meters.iter() {
                if let Some(meter) = slot.read() {
                    if em_count < MAX_EFFECT_DEVICES {
                        em[em_count] = meter;
                        em_count += 1;
                    }
                }
            }
            fence(Ordering::Acquire);
            let gen2 = self.shared.mixer_meter_generation.load(Ordering::Relaxed);
            if gen1 == gen2 {
                channel_meters = cm;
                effect_meters = em;
                effect_meter_count = em_count;
                break;
            }
        }

        let (absolute_step, step_fraction) =
            unpack_step_position(self.shared.step_position.load(Ordering::Acquire));
        AudioSnapshot {
            channel_meters,
            effect_meters,
            effect_meter_count,
            playing: self.shared.playing.load(Ordering::Acquire),
            absolute_step,
            step_fraction,
            peak: f32::from_bits(self.shared.peak.load(Ordering::Relaxed)),
            peak_left: f32::from_bits(self.shared.peak_left.load(Ordering::Relaxed)),
            peak_right: f32::from_bits(self.shared.peak_right.load(Ordering::Relaxed)),
            waveform,
            sample_rate: self.shared.sample_rate.load(Ordering::Relaxed),
            track_ids,
            track_count,
            track_waveforms,
            event_overflows: self.shared.event_overflows.load(Ordering::Relaxed),
            pending: self.shared.project_slot.has_pending()
                || self.shared.transport_pending.load(Ordering::Acquire) != 0,
            next_step: self.shared.next_step.load(Ordering::Acquire),
            health: self.shared.read_health(),
            input: self.shared.read_input_telemetry(),
        }
    }

    /// Reset peak holds/overload latches without editing the musical project.
    pub fn clear_mixer_meters(&self, channel: Option<ChannelRef>) {
        let Some(channel) = channel else {
            self.shared.clear_all_meters.store(true, Ordering::Release);
            self.shared
                .input_clear_meters
                .store(true, Ordering::Release);
            self.shared.input_clipped.store(false, Ordering::Relaxed);
            return;
        };
        let (kind, id) = match channel {
            ChannelRef::Master => (0, 0),
            ChannelRef::Channel(id) => (2, id),
        };
        if !self.shared.push_event_for_id(4 | (kind << 3), id) {
            // Meter resets are harmless session state; queue pressure must not
            // silently lose a requested clear. Clearing all is the bounded fallback.
            self.shared.clear_all_meters.store(true, Ordering::Release);
        }
    }

    /// Queue one ordered gesture in a separate bounded control queue, so live
    /// note pressure cannot consume transport capacity. False means queue full
    /// or engine unavailable; callers must surface that failure.
    pub fn transport(&self, intent: TransportIntent) -> bool {
        self.shared.push_transport(intent)
    }

    /// Active audio engine sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.shared
            .sample_rate
            .load(Ordering::Acquire)
            .max(MIN_AUDIO_SAMPLE_RATE)
    }

    /// Configure the optional CPAL audio input device, channel mode, and software
    /// monitoring policy without interrupting active output playback.
    pub fn configure_input(&mut self, request: AudioInputRequest) -> Result<()> {
        self.shared
            .input_mode
            .store(request.mode.to_u8(), Ordering::Release);
        self.shared
            .input_monitor_mode
            .store(request.monitor.to_u8(), Ordering::Release);
        self.shared.refresh_monitoring_active();

        let normalized_req_name = request
            .input_device
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let prev_req_name = self
            .active_input_request
            .input_device
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let current_status =
            AudioInputStatus::from_u8(self.shared.input_status.load(Ordering::Acquire));

        self.active_input_request = AudioInputRequest {
            input_device: normalized_req_name.clone(),
            mode: request.mode,
            monitor: request.monitor,
        };

        let Some(requested_name) = normalized_req_name else {
            self.stop_input_stream();
            self.shared.set_input_last_error(None);
            self.active_input_name = None;
            return Ok(());
        };

        // Mode or monitor-only update while the same input stream is already running cleanly.
        if prev_req_name.as_deref() == Some(requested_name.as_str())
            && current_status == AudioInputStatus::Running
        {
            return Ok(());
        }

        if self.silent_mode {
            self.active_input_name = Some(requested_name);
            self.shared.set_input_last_error(None);
            return Ok(());
        }

        #[cfg(feature = "audio")]
        {
            use cpal::traits::StreamTrait;
            let preferred_sr = Some(self.sample_rate());
            let preferred_buf = self.active_request.buffer_frames;
            match resolve_input_stream(&requested_name, preferred_sr, preferred_buf) {
                Ok(resolved) => {
                    self.stop_input_stream();
                    self.shared.set_input_last_error(None);
                    self.shared
                        .input_sample_rate
                        .store(resolved.sample_rate, Ordering::Relaxed);
                    self.shared
                        .input_channels
                        .store(resolved.channels, Ordering::Relaxed);
                    self.shared
                        .input_buffer_frames
                        .store(resolved.buffer_frames, Ordering::Relaxed);
                    match build_input_stream_for_format(
                        &resolved.device,
                        &resolved.config,
                        resolved.sample_format,
                        Arc::clone(&self.shared),
                    )
                    .and_then(|stream| {
                        stream.play().context("starting audio input device")?;
                        Ok(stream)
                    }) {
                        Ok(stream) => {
                            self.input_stream = Some(stream);
                            self.active_input_name = Some(resolved.device_name);
                            self.shared
                                .input_status
                                .store(AudioInputStatus::Running as u8, Ordering::Release);
                            self.shared.refresh_monitoring_active();
                            Ok(())
                        }
                        Err(err) => {
                            let msg = format!("{err:#}");
                            self.shared.reset_input_stream_state();
                            self.shared
                                .input_status
                                .store(AudioInputStatus::Failed as u8, Ordering::Release);
                            self.shared.set_input_last_error(Some(msg));
                            self.active_input_name = None;
                            Err(err)
                        }
                    }
                }
                Err(err) => {
                    let msg = format!("{err:#}");
                    self.stop_input_stream();
                    self.shared
                        .input_status
                        .store(AudioInputStatus::Failed as u8, Ordering::Release);
                    self.shared.set_input_last_error(Some(msg));
                    self.active_input_name = None;
                    Err(err)
                }
            }
        }
        #[cfg(not(feature = "audio"))]
        {
            self.active_input_name = Some(requested_name);
            self.shared.set_input_last_error(None);
            Ok(())
        }
    }

    fn stop_input_stream(&mut self) {
        #[cfg(feature = "audio")]
        if let Some(stream) = self.input_stream.take() {
            use cpal::traits::StreamTrait;
            let _ = stream.pause();
            drop(stream);
        }
        self.shared.reset_input_stream_state();
    }

    /// Arm transport-quantized or immediate free-time audio input capture into the
    /// lock-free SPSC capture ring.
    pub fn arm_capture(&self, spec: CaptureArmSpec) -> Result<()> {
        if !self.shared.alive.load(Ordering::Acquire) {
            bail!("audio runtime is shut down");
        }
        // Reset consumer read cursor to current write head before arming.
        self.shared
            .capture_phase
            .store(CapturePhase::Idle as u8, Ordering::Release);
        let head = self.shared.capture_write.load(Ordering::Acquire);
        self.shared.capture_read.store(head, Ordering::Release);
        self.shared.capture_dropped.store(0, Ordering::Relaxed);
        self.shared
            .capture_frames_written
            .store(0, Ordering::Release);
        self.shared.capture_end_step.store(0, Ordering::Release);
        self.shared
            .capture_quantized
            .store(spec.quantized, Ordering::Relaxed);
        self.shared
            .capture_punch_in_step
            .store(spec.punch_in_step, Ordering::Release);
        self.shared
            .capture_has_punch_out
            .store(spec.punch_out_step.is_some(), Ordering::Release);
        self.shared
            .capture_punch_out_step
            .store(spec.punch_out_step.unwrap_or(0), Ordering::Release);
        self.shared
            .capture_tail_frames
            .store(spec.tail_frames, Ordering::Release);

        if spec.quantized {
            self.shared
                .capture_start_step
                .store(spec.punch_in_step, Ordering::Release);
            self.shared
                .capture_phase
                .store(CapturePhase::Armed as u8, Ordering::Release);
        } else {
            let cur_step = self.shared.next_step.load(Ordering::Acquire);
            self.shared
                .capture_start_step
                .store(cur_step, Ordering::Release);
            self.shared
                .capture_phase
                .store(CapturePhase::Recording as u8, Ordering::Release);
        }
        self.shared.refresh_monitoring_active();
        Ok(())
    }

    /// Stop an active audio capture cleanly (collecting any configured round-trip
    /// latency tail frames when a hardware input stream is running) or disarm if
    /// still waiting for punch-in.
    pub fn stop_capture(&self) -> CapturePhase {
        let phase = CapturePhase::from_u8(self.shared.capture_phase.load(Ordering::Acquire));
        match phase {
            CapturePhase::Armed => {
                self.shared
                    .capture_phase
                    .store(CapturePhase::Idle as u8, Ordering::Release);
                self.shared.refresh_monitoring_active();
                CapturePhase::Idle
            }
            CapturePhase::Recording => {
                let end_step = self.shared.next_step.load(Ordering::Acquire);
                self.shared
                    .capture_end_step
                    .store(end_step, Ordering::Release);
                let input_running =
                    AudioInputStatus::from_u8(self.shared.input_status.load(Ordering::Acquire))
                        == AudioInputStatus::Running;
                let has_tail = self.shared.capture_tail_frames.load(Ordering::Acquire) > 0;
                let next = if input_running && has_tail {
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
                CapturePhase::from_u8(self.shared.capture_phase.load(Ordering::Acquire))
            }
            other => other,
        }
    }

    /// Immediately seal a `Recording` or `Finishing` capture without discarding
    /// frames already queued in the capture ring.
    pub fn force_seal_capture(&self) {
        self.shared.capture_tail_frames.store(0, Ordering::Release);
        self.shared
            .capture_phase
            .store(CapturePhase::Idle as u8, Ordering::Release);
        self.shared.refresh_monitoring_active();
    }

    /// Cancel an armed or active capture immediately and discard any buffered frames.
    pub fn cancel_capture(&self) {
        self.shared
            .capture_phase
            .store(CapturePhase::Idle as u8, Ordering::Release);
        self.shared.capture_tail_frames.store(0, Ordering::Release);
        let head = self.shared.capture_write.load(Ordering::Acquire);
        self.shared.capture_read.store(head, Ordering::Release);
        self.shared.capture_dropped.store(0, Ordering::Relaxed);
        self.shared
            .capture_frames_written
            .store(0, Ordering::Release);
        self.shared.refresh_monitoring_active();
    }

    /// Off-RT lock-free consumer draining up to `max_frames` captured stereo frames
    /// from the capture ring. Returns `(drained_frames, dropped_frames_since_last_drain)`.
    pub fn drain_capture_frames(&self, out: &mut Vec<[f32; 2]>, max_frames: usize) -> (usize, u64) {
        let read = self.shared.capture_read.load(Ordering::Relaxed);
        let write = self.shared.capture_write.load(Ordering::Acquire);
        let available = (write.wrapping_sub(read) as usize).min(max_frames);
        if available > 0 {
            out.reserve(available);
            for offset in 0..available {
                let slot = (read.wrapping_add(offset as u64) as usize) & AUDIO_CAPTURE_MASK;
                let left =
                    f32::from_bits(self.shared.capture_ring[slot][0].load(Ordering::Relaxed));
                let right =
                    f32::from_bits(self.shared.capture_ring[slot][1].load(Ordering::Relaxed));
                out.push([left, right]);
            }
            self.shared
                .capture_read
                .store(read.wrapping_add(available as u64), Ordering::Release);
        }
        let dropped = self.shared.capture_dropped.swap(0, Ordering::Relaxed);
        (available, dropped)
    }

    /// Deterministic headless/test injection feeding stereo frames directly through
    /// the production RT input callback path (`process_input_slice`).
    pub fn inject_input_frames(&mut self, frames: &[[f32; 2]], sample_rate: u32) {
        let mut interleaved = Vec::with_capacity(frames.len() * 2);
        for &[l, r] in frames {
            interleaved.push(l);
            interleaved.push(r);
        }
        process_input_slice(
            &mut self.synthetic_input_state,
            &self.shared,
            &interleaved,
            2,
            sample_rate,
        );
    }

    /// Snapshot of live hardware audio input metering, monitoring, and capture phase.
    pub fn input_telemetry(&self) -> cadence_core::runtime::AudioInputTelemetry {
        self.shared.read_input_telemetry()
    }

    /// Obtain a cloneable consumer handle to the lock-free stereo tap ring.
    pub fn tap_handle(&self) -> AudioTapHandle {
        AudioTapHandle {
            shared: Arc::clone(&self.shared),
        }
    }

    pub fn description(&self) -> &str {
        if self.shared.failed.load(Ordering::Acquire) {
            if self.description.starts_with("Audio device failed") {
                &self.description
            } else {
                "Audio device failed · restart or use --silent"
            }
        } else if self.shared.overload_tripped.load(Ordering::Acquire) {
            "Audio callback overload · paused safely"
        } else {
            &self.description
        }
    }

    pub fn shutdown(&mut self) {
        self.shared.playing.store(false, Ordering::Release);
        self.shared.alive.store(false, Ordering::Release);
        self.cancel_capture();
        self.stop_input_stream();
        #[cfg(feature = "audio")]
        if let Some(stream) = self.stream.take() {
            use cpal::traits::StreamTrait;
            let _ = stream.pause();
            drop(stream);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.shared.reclaim_all();
    }
}

impl Drop for AudioRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(feature = "audio")]
struct ResolvedOutput {
    device: cpal::Device,
    device_name: String,
    is_default: bool,
    effective_request: AudioStreamRequest,
    config: cpal::StreamConfig,
    sample_rate: u32,
    channels: u16,
    sample_format: cpal::SampleFormat,
    buffer_frames: u32,
    warning: Option<String>,
}

#[cfg(feature = "audio")]
fn format_device_description(
    name: &str,
    sample_rate: u32,
    channels: u16,
    buffer_frames: u32,
) -> String {
    if buffer_frames > 0 {
        format!("{name} · {sample_rate} Hz · {channels} ch · {buffer_frames} smp")
    } else {
        format!("{name} · {sample_rate} Hz · {channels} ch")
    }
}

#[cfg(feature = "audio")]
fn format_metadata(sample_format: cpal::SampleFormat) -> (u16, bool) {
    match sample_format {
        cpal::SampleFormat::F32 => (32, true),
        cpal::SampleFormat::F64 => (64, true),
        cpal::SampleFormat::I32 => (32, false),
        cpal::SampleFormat::I16 | cpal::SampleFormat::U16 => (16, false),
        _ => (32, true),
    }
}

#[cfg(feature = "audio")]
fn sample_format_rank(sample_format: cpal::SampleFormat) -> Option<u8> {
    match sample_format {
        cpal::SampleFormat::F32 => Some(5),
        cpal::SampleFormat::I32 => Some(4),
        cpal::SampleFormat::I16 => Some(3),
        cpal::SampleFormat::F64 => Some(2),
        cpal::SampleFormat::U16 => Some(1),
        _ => None,
    }
}

#[cfg(feature = "audio")]
fn channel_preference_rank(channels: u16) -> u16 {
    if channels == 2 {
        u16::MAX
    } else if channels > 2 {
        u16::MAX - channels
    } else if channels == 1 {
        1
    } else {
        0
    }
}

#[cfg(feature = "audio")]
fn select_buffer_size(
    supported: cpal::SupportedBufferSize,
    requested: Option<u32>,
    sample_rate: u32,
) -> (cpal::BufferSize, u32) {
    let range = match supported {
        cpal::SupportedBufferSize::Range { min, max } => Some((min, max)),
        cpal::SupportedBufferSize::Unknown => None,
    };
    match resolve_target_buffer_frames(requested, range, sample_rate) {
        Some(frames) => (cpal::BufferSize::Fixed(frames), frames),
        None => (cpal::BufferSize::Default, 0),
    }
}

#[cfg(feature = "audio")]
fn apply_stream_metadata(shared: &Shared, resolved: &ResolvedOutput) {
    let (bit_depth, is_float) = format_metadata(resolved.sample_format);
    shared
        .sample_rate
        .store(resolved.sample_rate, Ordering::Release);
    shared
        .buffer_frames
        .store(resolved.buffer_frames, Ordering::Release);
    shared.channels.store(resolved.channels, Ordering::Release);
    shared.bit_depth.store(bit_depth, Ordering::Release);
    shared
        .sample_format_is_float
        .store(is_float, Ordering::Release);
}

#[cfg(feature = "audio")]
fn find_best_config_for_rate(
    device: &cpal::Device,
    target_sr: u32,
) -> Option<cpal::SupportedStreamConfig> {
    use cpal::traits::DeviceTrait;
    device
        .supported_output_configs()
        .ok()?
        .filter(|range| {
            range.channels() > 0
                && range.min_sample_rate().0 <= target_sr
                && target_sr <= range.max_sample_rate().0
                && sample_format_rank(range.sample_format()).is_some()
        })
        .max_by_key(|range| {
            (
                channel_preference_rank(range.channels()),
                sample_format_rank(range.sample_format()).unwrap_or(0),
            )
        })
        .map(|range| range.with_sample_rate(cpal::SampleRate(target_sr)))
}

#[cfg(feature = "audio")]
fn select_supported_output_config(
    device: &cpal::Device,
    requested_sample_rate: Option<u32>,
) -> Result<cpal::SupportedStreamConfig> {
    use cpal::traits::DeviceTrait;

    if let Some(sr) = requested_sample_rate {
        if !(MIN_AUDIO_SAMPLE_RATE..=MAX_AUDIO_SAMPLE_RATE).contains(&sr) {
            bail!(
                "requested audio sample rate {sr} Hz outside supported range {MIN_AUDIO_SAMPLE_RATE}..={MAX_AUDIO_SAMPLE_RATE} Hz"
            );
        }
        if let Some(cfg) = find_best_config_for_rate(device, sr) {
            return Ok(cfg);
        }
    }

    let default_cfg = device
        .default_output_config()
        .context("reading audio device configuration")?;
    if sample_format_rank(default_cfg.sample_format()).is_some() {
        return Ok(default_cfg);
    }

    // If the device's default format isn't supported directly, pick the best supported config at its nominal rate.
    let nominal_sr = default_cfg.sample_rate().0;
    Ok(find_best_config_for_rate(device, nominal_sr).unwrap_or(default_cfg))
}

#[cfg(feature = "audio")]
fn resolve_output_stream(
    request: &AudioStreamRequest,
    strict_missing: bool,
) -> Result<ResolvedOutput> {
    use cpal::traits::{DeviceTrait, HostTrait};

    let host = cpal::default_host();
    let default_device = host.default_output_device();
    let default_name = default_device.as_ref().and_then(|d| d.name().ok());

    let requested_name = request
        .output_device
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let (device, device_name, is_default, effective_output_device, warning) =
        if let Some(req_name) = requested_name {
            let mut candidates: Vec<(String, cpal::Device)> = host
                .output_devices()
                .map(|devs| {
                    devs.map(|dev| {
                        let name = dev.name().unwrap_or_default();
                        (name, dev)
                    })
                    .collect()
                })
                .unwrap_or_default();
            let names: Vec<&str> = candidates.iter().map(|(n, _)| n.as_str()).collect();
            if let Some(idx) = match_device_name_index(&names, req_name) {
                let (matched_name, matched_dev) = candidates.swap_remove(idx);
                let is_def = default_name.as_deref() == Some(matched_name.as_str());
                (
                    matched_dev,
                    matched_name.clone(),
                    is_def,
                    Some(matched_name),
                    None,
                )
            } else if strict_missing {
                bail!("audio output device '{req_name}' not found");
            } else {
                let fallback = default_device
                    .context("no default audio output device; try --silent or export a WAV")?;
                let fb_name = default_name
                    .or_else(|| fallback.name().ok())
                    .unwrap_or_else(|| "Default device".into());
                let warn = format!("Saved output device '{req_name}' unavailable; using {fb_name}");
                (fallback, fb_name, true, None, Some(warn))
            }
        } else {
            let dev = default_device
                .context("no default audio output device; try --silent or export a WAV")?;
            let name = default_name
                .or_else(|| dev.name().ok())
                .unwrap_or_else(|| "Default device".into());
            (dev, name, true, None, None)
        };

    let supported = select_supported_output_config(&device, request.sample_rate)?;
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels();
    if channels == 0 || !(MIN_AUDIO_SAMPLE_RATE..=MAX_AUDIO_SAMPLE_RATE).contains(&sample_rate) {
        bail!("unsupported audio device configuration: {sample_rate} Hz, {channels} channels");
    }
    let sample_format = supported.sample_format();
    if sample_format_rank(sample_format).is_none() {
        bail!(
            "unsupported audio sample format {sample_format:?}; try another output device or --silent"
        );
    }
    let (buffer_size, buffer_frames) =
        select_buffer_size(*supported.buffer_size(), request.buffer_frames, sample_rate);
    let config = cpal::StreamConfig {
        channels,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size,
    };
    let effective_request = AudioStreamRequest {
        output_device: effective_output_device,
        sample_rate: request.sample_rate,
        buffer_frames: request.buffer_frames,
    };

    Ok(ResolvedOutput {
        device,
        device_name,
        is_default,
        effective_request,
        config,
        sample_rate,
        channels,
        sample_format,
        buffer_frames,
        warning,
    })
}

#[cfg(feature = "audio")]
fn build_device_info<I>(
    name: String,
    is_default: bool,
    default_cfg: Option<cpal::SupportedStreamConfig>,
    supported_cfgs: Option<I>,
) -> AudioDeviceInfo
where
    I: Iterator<Item = cpal::SupportedStreamConfigRange>,
{
    let default_sample_rate = default_cfg.as_ref().map_or(0, |cfg| cfg.sample_rate().0);
    let default_channels = default_cfg.as_ref().map_or(0, |cfg| cfg.channels());

    let mut supported_sample_rates: Vec<u32> = Vec::new();
    let mut min_buffer_frames: Option<u32> = None;
    let mut max_buffer_frames: Option<u32> = None;
    let mut fallback_channels: u16 = 0;

    if let Some(configs) = supported_cfgs {
        for range in configs {
            let min_sr = range.min_sample_rate().0;
            let max_sr = range.max_sample_rate().0;
            for &std_sr in STANDARD_SAMPLE_RATES {
                if std_sr >= min_sr && std_sr <= max_sr && !supported_sample_rates.contains(&std_sr)
                {
                    supported_sample_rates.push(std_sr);
                }
            }
            if let cpal::SupportedBufferSize::Range { min, max } = *range.buffer_size() {
                let lo = min.min(max);
                let hi = min.max(max);
                min_buffer_frames = Some(min_buffer_frames.map_or(lo, |cur| cur.min(lo)));
                max_buffer_frames = Some(max_buffer_frames.map_or(hi, |cur| cur.max(hi)));
            }
            if fallback_channels == 0 || (fallback_channels != 2 && range.channels() == 2) {
                fallback_channels = range.channels();
            }
        }
    }
    supported_sample_rates.sort_unstable();
    if supported_sample_rates.is_empty() && default_sample_rate > 0 {
        supported_sample_rates.push(default_sample_rate);
    }

    AudioDeviceInfo {
        name,
        is_default,
        default_sample_rate,
        supported_sample_rates,
        min_buffer_frames,
        max_buffer_frames,
        channels: if default_channels > 0 {
            default_channels
        } else {
            fallback_channels
        },
    }
}

#[cfg(feature = "audio")]
fn build_stream_for_format(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    format: cpal::SampleFormat,
    engine: Engine,
    shared: Arc<Shared>,
) -> Result<cpal::Stream> {
    match format {
        cpal::SampleFormat::F32 => build_stream::<f32>(device, config, engine, shared),
        cpal::SampleFormat::F64 => build_stream::<f64>(device, config, engine, shared),
        cpal::SampleFormat::I32 => build_stream::<i32>(device, config, engine, shared),
        cpal::SampleFormat::I16 => build_stream::<i16>(device, config, engine, shared),
        cpal::SampleFormat::U16 => build_stream::<u16>(device, config, engine, shared),
        other => bail!(
            "unsupported audio sample format {other:?}; try another output device or --silent"
        ),
    }
}

#[cfg(feature = "audio")]
fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut engine: Engine,
    shared: Arc<Shared>,
) -> Result<cpal::Stream>
where
    T: cpal::Sample + cpal::SizedSample + cpal::FromSample<f32>,
{
    use cpal::traits::DeviceTrait;
    let channels = config.channels as usize;
    let sample_rate = config.sample_rate.0;
    let mut watchdog = CallbackWatchdog::new();
    let cb_shared = Arc::clone(&shared);
    let err_shared = shared;
    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            render_output_slice(
                &mut engine,
                &mut watchdog,
                &cb_shared,
                data,
                channels,
                sample_rate,
            );
        },
        move |error| {
            let code = match error {
                cpal::StreamError::DeviceNotAvailable => AudioErrorCode::DeviceUnavailable,
                _ => AudioErrorCode::BackendError,
            };
            err_shared.set_last_error(Some(error.to_string()));
            err_shared.error_code.store(code as u8, Ordering::Release);
            err_shared.playing.store(false, Ordering::Release);
            err_shared.failed.store(true, Ordering::Release);
        },
        None,
    )?;
    Ok(stream)
}

#[cfg(test)]
mod effect_meter_tests {
    use super::*;
    use cadence_core::material::{
        InstrumentInstance, Lane, Part, Pattern, PatternKind, Placement, Project,
    };
    use cadence_core::mixer::{Channel, ChannelKind, CompressorPatch, EffectDevice, EffectKind};
    use cadence_core::music::{EndRule, Step};
    use cadence_core::rack::{ModuleKind, RackPatch};

    #[test]
    fn silent_runtime_publishes_live_effect_gr_and_hold_db() {
        let mut p = Project::empty();
        p.bpm = 120.0;
        p.bars = 2;
        p.end = EndRule::Loop;
        p.mixer.master.strip.gain = 1.0;
        let comp_id = 42_u64;
        p.mixer.master.inserts.push(EffectDevice {
            id: comp_id,
            name: "Master Comp".into(),
            bypassed: false,
            kind: EffectKind::Compressor(CompressorPatch {
                threshold_db: -24.0,
                ratio: 8.0,
                attack_ms: 1.0,
                release_ms: 80.0,
                makeup_db: 0.0,
                mix: 1.0,
            }),
        });
        p.mixer
            .channels
            .push(Channel::new(1, ChannelKind::Audio, "Lead"));
        let hit = Step {
            active: true,
            degree: 1,
            velocity: 1.0,
            gate: 0.9,
            ..Step::default()
        };
        p.patterns.push(Pattern {
            id: 1,
            name: "Sustained".into(),
            kind: PatternKind::Notes {
                steps: vec![hit; 16],
            },
        });
        let mut rack = RackPatch::default_synth();
        if let Some(m) = rack.zones[0].module_mut(4) {
            if let ModuleKind::Env(ref mut env) = m.kind {
                env.attack_ms = 1.0;
                env.sustain = 1.0;
            }
        }
        p.instruments.push(InstrumentInstance {
            id: 1,
            name: "Lead".into(),
            rack,
            channel: 1,
        });
        p.parts.push(Part::new(1, "LeadPart", 1, 1));
        let mut lane = Lane::new(1, "Lead");
        lane.placements.push(Placement::new(1, 1, 0, 32));
        p.lanes.push(lane);

        let score = p.compile().unwrap();
        let rt = AudioRuntime::start(score, true).unwrap();
        assert!(rt.transport(TransportIntent::Play { playing: true }));

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut observed_gr = 0.0_f32;
        let mut observed_gr_hold = 0.0_f32;
        while Instant::now() < deadline {
            let snap = rt.snapshot();
            let gr = snap.effect_gain_reduction_db(comp_id);
            let hold = snap.effect_gr_hold_db(comp_id);
            if gr > 1.0 && hold >= gr {
                observed_gr = gr;
                observed_gr_hold = hold;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            observed_gr > 1.0 && observed_gr.is_finite(),
            "expected positive live compressor gain reduction dB in AudioSnapshot, got {observed_gr}"
        );
        assert!(
            observed_gr_hold >= observed_gr && observed_gr_hold.is_finite(),
            "expected live compressor gr_hold_db ({observed_gr_hold}) >= gain_reduction_db ({observed_gr})"
        );

        // Bypassing the compressor resets its live GR and GR hold readouts to 0.0 dB.
        p.mixer.master.inserts[0].bypassed = true;
        rt.replace_project(p.compile().unwrap()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut bypassed_gr = observed_gr;
        let mut bypassed_hold = observed_gr_hold;
        while Instant::now() < deadline {
            let snap = rt.snapshot();
            bypassed_gr = snap.effect_gain_reduction_db(comp_id);
            bypassed_hold = snap.effect_gr_hold_db(comp_id);
            if bypassed_gr == 0.0 && bypassed_hold == 0.0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            bypassed_gr, 0.0,
            "bypassed compressor must report 0.0 dB gain reduction"
        );
        assert_eq!(
            bypassed_hold, 0.0,
            "bypassed compressor must report 0.0 dB held gain reduction"
        );
    }
}
