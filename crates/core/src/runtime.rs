//! Frontend- and engine-neutral runtime boundary types.
//!
//! Pure data structures shared across `core`, `engine`, `app`, `gui`,
//! and the host binary without pulling audio/MIDI/windowing crates into the
//! domain or controller layers.
use crate::mixer::{ChannelRef, MAX_EFFECT_DEVICES};
use crate::sampler::SampleData;
use crate::score::MAX_INSTRUMENTS;
use serde::Serialize;
use std::sync::Arc;

pub type SampleBank = Vec<(u64, Arc<SampleData>)>;

/// Supported audio device sample-rate range (`Hz`).
pub const MIN_AUDIO_SAMPLE_RATE: u32 = 8_000;
pub const MAX_AUDIO_SAMPLE_RATE: u32 = 384_000;
/// Supported explicit audio callback buffer size range (`frames`).
pub const MIN_AUDIO_BUFFER_FRAMES: u32 = 32;
pub const MAX_AUDIO_BUFFER_FRAMES: u32 = 8_192;
/// Standard discrete sample rates presented in the Devices · Audio UI.
pub const STANDARD_SAMPLE_RATES: &[u32] = &[44_100, 48_000, 88_200, 96_000, 176_400, 192_000];
/// Standard discrete buffer frame sizes presented in the Devices · Audio UI.
pub const STANDARD_BUFFER_SIZES: &[u32] = &[64, 128, 256, 512, 1024, 2048];
/// Manual audio recording latency compensation trim bounds in milliseconds.
pub const MIN_AUDIO_LATENCY_TRIM_MS: f32 = -250.0;
pub const MAX_AUDIO_LATENCY_TRIM_MS: f32 = 250.0;

crate::define_choice_enum! {
    /// Channel routing applied to hardware audio input frames before metering and capture.
    #[derive(Default)]
    pub enum AudioInputMode [array] {
        #[default]
        Stereo => ("stereo", "Stereo (1+2)", ["1+2", "1-2", "st"]),
        MonoLeft => ("mono_left", "Mono (Ch 1)", ["mono", "left", "ch1", "1", "l"]),
        MonoRight => ("mono_right", "Mono (Ch 2)", ["right", "ch2", "2", "r"]),
    }
}

impl AudioInputMode {
    pub const fn to_u8(self) -> u8 {
        match self {
            Self::Stereo => 0,
            Self::MonoLeft => 1,
            Self::MonoRight => 2,
        }
    }

    pub const fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::MonoLeft,
            2 => Self::MonoRight,
            _ => Self::Stereo,
        }
    }
}

crate::define_choice_enum! {
    /// Software live input monitoring policy (defaults to `Off` to prevent feedback).
    #[derive(Default)]
    pub enum InputMonitorMode [array] {
        #[default]
        Off => ("off", "Off", ["none", "0", "false"]),
        Auto => ("auto", "Auto (Armed)", ["armed", "rec", "when_armed"]),
        On => ("on", "On", ["always", "1", "true"]),
    }
}

impl InputMonitorMode {
    pub const fn to_u8(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Auto => 1,
            Self::On => 2,
        }
    }

    pub const fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Auto,
            2 => Self::On,
            _ => Self::Off,
        }
    }
}

/// Requested hardware audio input stream and monitoring configuration.
/// Applied independently from [`AudioStreamRequest`] so changing input routing
/// or monitoring never interrupts active output playback.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioInputRequest {
    /// Preferred OS input device name (`None` = inactive; no OS mic stream opened).
    pub input_device: Option<String>,
    /// Channel mapping applied to incoming hardware samples.
    pub mode: AudioInputMode,
    /// Software monitoring policy (`Off` / `Auto` / `On`).
    pub monitor: InputMonitorMode,
}

/// Coarse status of the optional hardware audio input stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum AudioInputStatus {
    /// No audio input device selected or stream closed.
    #[default]
    Inactive = 0,
    /// Hardware audio input stream is open and feeding live meters/capture.
    Running = 1,
    /// Opening or running the audio input stream failed (e.g. disconnected or permission denied).
    Failed = 2,
}

impl AudioInputStatus {
    pub const fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Running,
            2 => Self::Failed,
            _ => Self::Inactive,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Inactive => "INACTIVE",
            Self::Running => "ACTIVE",
            Self::Failed => "INPUT ERROR",
        }
    }
}

/// Lock-free capture gate phase published atomically between output transport,
/// input callback, and host recorder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum CapturePhase {
    #[default]
    Idle = 0,
    /// Waiting for quantized `punch_in_step` (or immediate start).
    Armed = 1,
    /// Actively pushing hardware input frames into the lock-free capture ring.
    Recording = 2,
    /// Collecting post-punch-out latency compensation tail frames before sealing.
    Finishing = 3,
}

impl CapturePhase {
    pub const fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Armed,
            2 => Self::Recording,
            3 => Self::Finishing,
            _ => Self::Idle,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Idle => "IDLE",
            Self::Armed => "ARM",
            Self::Recording => "REC",
            Self::Finishing => "FINISHING",
        }
    }
}

/// Transport-synchronized capture gate specification sent when arming audio recording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureArmSpec {
    /// When `true`, wait for transport to reach `punch_in_step` before opening capture;
    /// when `false`, begin capturing immediately regardless of transport playback state.
    pub quantized: bool,
    /// Target step at which quantized capture begins.
    pub punch_in_step: u64,
    /// Optional automatic punch-out step (e.g. loop end or fixed bar count).
    pub punch_out_step: Option<u64>,
    /// Extra input frames captured after punch-out to compensate for round-trip buffer latency.
    pub tail_frames: u32,
}

/// Lock-free live audio input metering, monitoring, and capture gate telemetry
/// published on [`AudioSnapshot`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct AudioInputTelemetry {
    pub status: AudioInputStatus,
    pub sample_rate: u32,
    pub channels: u16,
    pub buffer_frames: u32,
    pub peak: [f32; 2],
    pub peak_hold: [f32; 2],
    pub clipped: bool,
    /// True once an active input stream has delivered sustained bit-for-bit `0.0` silence
    /// (strong indicator of OS microphone privacy muting, e.g. macOS TCC, or muted input hardware).
    pub digital_silence: bool,
    pub mode: AudioInputMode,
    pub monitor_mode: InputMonitorMode,
    pub monitoring: bool,
    pub capture_phase: CapturePhase,
    pub capture_start_step: u64,
    pub capture_end_step: u64,
    pub captured_frames: u64,
    pub dropped_frames: u64,
}

impl AudioInputTelemetry {
    /// Estimated total round-trip recording latency in milliseconds (input buffer + output buffer
    /// + manual user trim), or `None` when input sample rate is unknown.
    pub fn round_trip_latency_ms(
        self,
        output_health: AudioHealth,
        output_sample_rate: u32,
        trim_ms: f32,
    ) -> Option<f32> {
        if self.sample_rate == 0 {
            return None;
        }
        let in_ms = if self.buffer_frames > 0 {
            self.buffer_frames as f32 * 1000.0 / self.sample_rate as f32
        } else {
            0.0
        };
        let out_ms = output_health
            .buffer_latency_ms(output_sample_rate)
            .unwrap_or(0.0);
        let trim = if trim_ms.is_finite() { trim_ms } else { 0.0 };
        Some(in_ms + out_ms + trim)
    }

    /// Estimated total round-trip latency converted to capture frames (signed; positive trims
    /// leading latency samples, negative pads silence).
    pub fn round_trip_latency_frames(
        self,
        output_health: AudioHealth,
        output_sample_rate: u32,
        trim_ms: f32,
    ) -> isize {
        let Some(ms) = self.round_trip_latency_ms(output_health, output_sample_rate, trim_ms)
        else {
            return 0;
        };
        (f64::from(ms) * f64::from(self.sample_rate) / 1000.0).round() as isize
    }
}

/// Default automatic buffer frame size when user config leaves `audio_buffer_size` unset.
#[inline]
pub const fn default_auto_buffer_frames() -> u32 {
    if cfg!(debug_assertions) {
        1024
    } else {
        512
    }
}

/// Requested hardware audio stream settings passed from host/UI to `cadence-engine`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioStreamRequest {
    /// Preferred output device name (`None` = System Default, follows OS default switches).
    pub output_device: Option<String>,
    /// Preferred sample rate in Hz (`None` = Device Default).
    pub sample_rate: Option<u32>,
    /// Preferred callback buffer size in frames (`None` = Auto default clamped to device range).
    pub buffer_frames: Option<u32>,
}

/// Coarse health state of the live audio engine stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum AudioStreamStatus {
    /// No active hardware device (e.g. `--silent` clock or test snapshot).
    #[default]
    Silent = 0,
    /// Hardware audio stream is running normally within callback budget.
    Running = 1,
    /// Callback overload circuit breaker tripped; transport auto-paused and silenced safely.
    Overloaded = 2,
    /// Underlying OS audio stream reported a fatal device/backend error.
    Failed = 3,
}

impl AudioStreamStatus {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Running,
            2 => Self::Overloaded,
            3 => Self::Failed,
            _ => Self::Silent,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Silent => "SILENT",
            Self::Running => "RUNNING",
            Self::Overloaded => "OVERLOAD PAUSED",
            Self::Failed => "STREAM ERROR",
        }
    }
}

/// Fixed lock-free error category stored in audio-thread atomics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum AudioErrorCode {
    #[default]
    None = 0,
    DeviceUnavailable = 1,
    BackendError = 2,
    OverloadCircuitBreaker = 3,
}

impl AudioErrorCode {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::DeviceUnavailable,
            2 => Self::BackendError,
            3 => Self::OverloadCircuitBreaker,
            _ => Self::None,
        }
    }

    pub fn message(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::DeviceUnavailable => Some("Audio device disconnected or unavailable"),
            Self::BackendError => Some("OS audio backend stream error"),
            Self::OverloadCircuitBreaker => {
                Some("Audio callback overload — paused safely to prevent glitching")
            }
        }
    }
}

/// Lock-free live audio stream health & callback budget telemetry published on [`AudioSnapshot`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct AudioHealth {
    pub status: AudioStreamStatus,
    /// Smoothed EMA ratio of callback execution time to buffer period (`0.0..`, `1.0` = 100%).
    pub cpu_load: f32,
    /// Decaying recent peak callback load ratio (`0.0..`).
    pub peak_cpu_load: f32,
    /// Total callback overruns / scheduling slips since stream start or reset.
    pub xrun_count: u64,
    /// Active hardware callback buffer size in frames (`0` when unknown or silent).
    pub buffer_frames: u32,
    /// Active hardware output channel count (`0` when silent).
    pub channels: u16,
    /// Active hardware sample bit depth (`16`, `32`, `64`, or `0` when silent).
    pub bit_depth: u16,
    /// True when the active hardware sample format is floating-point (`f32`/`f64`).
    pub sample_format_is_float: bool,
    /// True while the overload circuit breaker is latched (cleared on next Play or Reset).
    pub overloaded: bool,
    /// True when the underlying CPAL stream reported a fatal error.
    pub failed: bool,
    pub error_code: AudioErrorCode,
}

impl AudioHealth {
    /// Approximate one-way hardware buffer latency in milliseconds for a given `sample_rate`.
    pub fn buffer_latency_ms(self, sample_rate: u32) -> Option<f32> {
        if self.buffer_frames == 0 || sample_rate == 0 {
            return None;
        }
        Some(self.buffer_frames as f32 * 1000.0 / sample_rate as f32)
    }

    /// Compact human-readable sample format label (e.g. `"32-bit float"`, `"16-bit int"`).
    pub fn format_label(self) -> &'static str {
        match (self.bit_depth, self.sample_format_is_float) {
            (32, true) => "32-bit float",
            (64, true) => "64-bit float",
            (32, false) => "32-bit int",
            (24, false) => "24-bit int",
            (16, false) => "16-bit int",
            _ => "32-bit float",
        }
    }
}

/// Pure description of one OS audio input or output device (zero audio-backend dependency).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub is_default: bool,
    pub default_sample_rate: u32,
    pub supported_sample_rates: Vec<u32>,
    pub min_buffer_frames: Option<u32>,
    pub max_buffer_frames: Option<u32>,
    pub channels: u16,
}

/// Snapshot of available OS audio hardware and the currently running audio stream.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioInventory {
    pub outputs: Vec<AudioDeviceInfo>,
    pub inputs: Vec<AudioDeviceInfo>,
    /// Actual OS name of the currently active output device (updates on hot-plug/default switch).
    pub active_output: Option<String>,
    pub active_output_is_default: bool,
    pub active_sample_rate: u32,
    pub active_buffer_frames: u32,
    pub active_channels: u16,
    pub active_format_label: String,
    /// Actual OS name of the currently active audio input stream, if open.
    pub active_input: Option<String>,
    pub active_input_sample_rate: u32,
    pub active_input_channels: u16,
    pub active_input_buffer_frames: u32,
    pub silent_mode: bool,
    pub last_error: Option<String>,
    pub input_last_error: Option<String>,
}

/// One transport gesture, published atomically and ordered with live key events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportIntent {
    Play {
        playing: bool,
    },
    /// Seek transport position without cutting ringing voices or effect tails.
    Seek {
        step: u64,
    },
    /// Stop and silence voices/mixer, seeking to `step` (document boundary / shutdown).
    Reset {
        step: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChannelMeter {
    pub channel: ChannelRef,
    pub input_peak: [f32; 2],
    pub input_peak_hold: [f32; 2],
    pub output_peak: [f32; 2],
    pub output_peak_hold: [f32; 2],
    /// Informational 32-bit float headroom flag: true when an internal float bus
    /// exceeds 0 dBFS (`> 1.0`) on input or output (does NOT mean audible clipping!).
    pub hot: bool,
    /// True digital clipping flag: strictly true when non-finite occurs anywhere OR
    /// Master post-insert post-fader output exceeds `1.0` (0 dBFS DAC/WAV ceiling).
    pub clipped: bool,
}

impl Default for ChannelMeter {
    fn default() -> Self {
        Self {
            channel: ChannelRef::Master,
            input_peak: [0.0; 2],
            input_peak_hold: [0.0; 2],
            output_peak: [0.0; 2],
            output_peak_hold: [0.0; 2],
            hot: false,
            clipped: false,
        }
    }
}

/// Live zero-alloc telemetry for one active insert effect device.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EffectMeter {
    pub device_id: u64,
    /// Current dynamics gain reduction in dB (`>= 0.0`, `0.0` when inactive/bypassed).
    pub gain_reduction_db: f32,
    /// Peak-held dynamics gain reduction in dB (`>= 0.0`, 1.5s hold + decay).
    pub gr_hold_db: f32,
}

#[derive(Clone, Debug)]
pub struct AudioSnapshot {
    pub playing: bool,
    pub absolute_step: u64,
    /// Sub-step progress in `[0.0, 1.0)`, published tear-free with `absolute_step`
    /// every control block (~1.3 ms at 48 kHz) for smooth 60/120fps playheads.
    pub step_fraction: f32,
    pub peak: f32,
    pub peak_left: f32,
    pub peak_right: f32,
    pub waveform: Vec<f32>,
    pub sample_rate: u32,
    pub track_ids: [u64; MAX_INSTRUMENTS],
    pub track_count: usize,
    /// Full-rate dry instrument output; no channel gain/pan, mute, or shared FX.
    pub track_waveforms: Vec<Vec<f32>>,
    pub event_overflows: u64,
    pub channel_meters: Vec<ChannelMeter>,
    /// Fixed-size zero-alloc live effect metering (e.g. dynamics gain reduction dB).
    pub effect_meters: [EffectMeter; MAX_EFFECT_DEVICES],
    pub effect_meter_count: usize,
    pub pending: bool,
    /// The step the transport triggers next; live boundaries are computed from it.
    pub next_step: u64,
    /// Live stream health, callback CPU budget utilization, and xrun/overload state.
    pub health: AudioHealth,
    /// Live hardware audio input metering, software monitoring, and capture gate telemetry.
    pub input: AudioInputTelemetry,
}

impl Default for AudioSnapshot {
    fn default() -> Self {
        Self {
            playing: false,
            absolute_step: 0,
            step_fraction: 0.0,
            peak: 0.0,
            peak_left: 0.0,
            peak_right: 0.0,
            waveform: Vec::new(),
            sample_rate: 0,
            track_ids: [0; MAX_INSTRUMENTS],
            track_count: 0,
            track_waveforms: Vec::new(),
            event_overflows: 0,
            channel_meters: Vec::new(),
            effect_meters: [EffectMeter::default(); MAX_EFFECT_DEVICES],
            effect_meter_count: 0,
            pending: false,
            next_step: 0,
            health: AudioHealth::default(),
            input: AudioInputTelemetry::default(),
        }
    }
}

impl AudioSnapshot {
    pub fn channel_meter(&self, channel: ChannelRef) -> Option<&ChannelMeter> {
        self.channel_meters
            .iter()
            .find(|meter| meter.channel == channel)
    }

    pub fn effect_meter(&self, device_id: u64) -> Option<&EffectMeter> {
        self.effect_meters
            .iter()
            .take(self.effect_meter_count.min(MAX_EFFECT_DEVICES))
            .find(|meter| meter.device_id == device_id)
    }

    pub fn effect_gain_reduction_db(&self, device_id: u64) -> f32 {
        self.effect_meter(device_id).map_or(0.0, |m| {
            if m.gain_reduction_db.is_finite() {
                m.gain_reduction_db.max(0.0)
            } else {
                0.0
            }
        })
    }

    pub fn effect_gr_hold_db(&self, device_id: u64) -> f32 {
        self.effect_meter(device_id).map_or(0.0, |m| {
            if m.gr_hold_db.is_finite() {
                m.gr_hold_db.max(0.0)
            } else {
                0.0
            }
        })
    }

    pub fn set_effect_meter(&mut self, device_id: u64, gain_reduction_db: f32, gr_hold_db: f32) {
        let gr = if gain_reduction_db.is_finite() {
            gain_reduction_db.max(0.0)
        } else {
            0.0
        };
        let hold = if gr_hold_db.is_finite() {
            gr_hold_db.max(gr)
        } else {
            gr
        };
        let count = self.effect_meter_count.min(MAX_EFFECT_DEVICES);
        if let Some(existing) = self.effect_meters[..count]
            .iter_mut()
            .find(|m| m.device_id == device_id)
        {
            existing.gain_reduction_db = gr;
            existing.gr_hold_db = hold;
            return;
        }
        if count < MAX_EFFECT_DEVICES {
            self.effect_meters[count] = EffectMeter {
                device_id,
                gain_reduction_db: gr,
                gr_hold_db: hold,
            };
            self.effect_meter_count = count + 1;
        }
    }

    pub fn track_waveform(&self, track_id: u64) -> &[f32] {
        self.track_ids
            .iter()
            .take(self.track_count.min(MAX_INSTRUMENTS))
            .position(|&id| id == track_id)
            .and_then(|index| self.track_waveforms.get(index))
            .map_or(&[], Vec::as_slice)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MidiMessage {
    NoteOn {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    NoteOff {
        channel: u8,
        note: u8,
    },
    ProgramChange {
        channel: u8,
        program: u8,
    },
    Control {
        channel: u8,
        controller: u8,
        value: u8,
    },
    PitchBend {
        channel: u8,
        /// Signed 14-bit pitch bend (`-8192..=8191`, `0` = center).
        value: i16,
    },
    /// Universal MIDI Machine Control SysEx (`F0 7F <dev> 06 <cmd> F7`):
    /// `0x01` Stop, `0x02` Play, `0x06` Record, `0x09` Pause.
    Mmc {
        command: u8,
    },
}

impl MidiMessage {
    /// Parse a channel-voice message or universal MMC transport SysEx.
    /// A Note On with zero velocity is normalized to a Note Off.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let (&status, data) = bytes.split_first()?;
        if status == 0xf0 {
            // Standard MMC SysEx: F0 7F <device_id> 06 <command> F7
            if bytes.len() == 6
                && bytes[1] == 0x7f
                && bytes[3] == 0x06
                && bytes[bytes.len() - 1] == 0xf7
            {
                return Some(Self::Mmc { command: bytes[4] });
            }
            return None;
        }
        let channel = status & 0x0f;
        let byte = |i: usize| data.get(i).copied().filter(|b| *b < 0x80);
        match status & 0xf0 {
            0x80 => Some(Self::NoteOff {
                channel,
                note: byte(0)?,
            }),
            0x90 => {
                let note = byte(0)?;
                match byte(1)? {
                    0 => Some(Self::NoteOff { channel, note }),
                    velocity => Some(Self::NoteOn {
                        channel,
                        note,
                        velocity,
                    }),
                }
            }
            0xb0 => Some(Self::Control {
                channel,
                controller: byte(0)?,
                value: byte(1)?,
            }),
            0xc0 => Some(Self::ProgramChange {
                channel,
                program: byte(0)?,
            }),
            0xe0 => {
                let lsb = i16::from(byte(0)?);
                let msb = i16::from(byte(1)?);
                Some(Self::PitchBend {
                    channel,
                    value: ((msb << 7) | lsb) - 8192,
                })
            }
            _ => None,
        }
    }
}
