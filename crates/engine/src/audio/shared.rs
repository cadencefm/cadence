//! Lock-free shared audio state, command mailboxes, and atomic score publication.
use super::*;
use cadence_core::dsp::Delay;
use cadence_core::mixer::MAX_EFFECT_DEVICES;
use cadence_core::rack::{CompiledZoneGraph, RackPatch, RackZone};
use cadence_core::runtime::{
    AudioErrorCode, AudioHealth, AudioInputMode, AudioInputStatus, AudioInputTelemetry,
    AudioStreamStatus, CapturePhase, EffectMeter, InputMonitorMode, MAX_AUDIO_SAMPLE_RATE,
    MIN_AUDIO_SAMPLE_RATE,
};
use std::ptr;
use std::sync::atomic::{
    AtomicBool, AtomicPtr, AtomicU16, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering,
};
use std::sync::{Arc, Mutex};

pub(super) const VOICES: usize = 256;
pub(super) const WAVE_SAMPLES: usize = 256;
pub(super) const MAILBOXES: usize = 64;
pub(super) const TRANSPORT_MAILBOXES: usize = 16;
pub(super) const SILENT_RATE: u32 = 48_000;
pub(super) const SILENT_BLOCK: usize = 256;
pub(super) const CONTROL_BLOCK: usize = 64;
pub(super) const PREVIEW_TRACK: u8 = 254;
pub(super) const EVENT_RESERVED: u64 = 1 << 63;
/// Transport positions are unbounded; 40 bits is millennia of music at 300 BPM.
pub(super) const TRANSPORT_STEP_MASK: u64 = (1 << 40) - 1;
/// Packed `(step: 48 bits, frac_u16: 16 bits)` for tear-free snapshot reads.
pub(super) const STEP_POS_MASK: u64 = (1 << 48) - 1;
pub(super) const EVENT_HAS_BINDING: u64 = 1 << 62;
pub(super) const SCOPE_BUFFER_SAMPLES: usize = WAVE_SAMPLES * 2;
/// Power-of-two stereo audio tap ring capacity (`262_144` frames ≈ 5.46 s @ 48 kHz, ~2 MiB heap-boxed).
pub const AUDIO_TAP_CAPACITY: usize = 1 << 18;
pub(super) const AUDIO_TAP_MASK: usize = AUDIO_TAP_CAPACITY - 1;
/// Power-of-two stereo live audio input capture ring capacity (`262_144` frames ≈ 5.46 s @ 48 kHz).
pub const AUDIO_CAPTURE_CAPACITY: usize = 1 << 18;
pub(super) const AUDIO_CAPTURE_MASK: usize = AUDIO_CAPTURE_CAPACITY - 1;
/// Power-of-two stereo software input monitor ring capacity (`32_768` frames ≈ 170 ms @ 192 kHz,
/// `85 ms @ 384 kHz`, 256 KiB heap-boxed).
pub(super) const AUDIO_MONITOR_CAPACITY: usize = 1 << 15;
pub(super) const AUDIO_MONITOR_MASK: usize = AUDIO_MONITOR_CAPACITY - 1;

pub(super) fn pack_step_position(step: u64, frac: f64) -> u64 {
    let frac_u16 = (frac.clamp(0.0, 0.99998) * 65_536.0) as u64;
    (step & STEP_POS_MASK) | (frac_u16 << 48)
}

pub(super) fn unpack_step_position(bits: u64) -> (u64, f32) {
    (bits & STEP_POS_MASK, (bits >> 48) as f32 / 65_536.0)
}

pub(super) fn pack_transport_intent(intent: TransportIntent) -> u64 {
    match intent {
        TransportIntent::Play { playing } => 1 | (u64::from(playing) << 2),
        TransportIntent::Seek { step } => 2 | ((step & TRANSPORT_STEP_MASK) << 2),
        TransportIntent::Reset { step } => 3 | ((step & TRANSPORT_STEP_MASK) << 2),
    }
}

pub(super) fn unpack_transport_intent(bits: u64) -> Option<TransportIntent> {
    match bits & 0b11 {
        1 => Some(TransportIntent::Play {
            playing: bits & 4 != 0,
        }),
        2 => Some(TransportIntent::Seek {
            step: (bits >> 2) & TRANSPORT_STEP_MASK,
        }),
        3 => Some(TransportIntent::Reset {
            step: (bits >> 2) & TRANSPORT_STEP_MASK,
        }),
        _ => None,
    }
}

/// Lock-free single-producer / single-consumer latest-value pointer cell with
/// two retirement slots so the audio callback never drops heap allocations.
pub(super) struct AtomicBoxSlot<T> {
    pending: AtomicPtr<T>,
    retired: [AtomicPtr<T>; 2],
}

impl<T> AtomicBoxSlot<T> {
    pub(super) fn new() -> Self {
        Self {
            pending: AtomicPtr::new(ptr::null_mut()),
            retired: std::array::from_fn(|_| AtomicPtr::new(ptr::null_mut())),
        }
    }

    pub(super) fn reclaim(&self) {
        for slot in &self.retired {
            let old = slot.swap(ptr::null_mut(), Ordering::AcqRel);
            if !old.is_null() {
                // SAFETY: exclusive ownership transferred back from audio callback.
                unsafe { drop(Box::from_raw(old)) };
            }
        }
    }

    pub(super) fn publish(&self, value: T) {
        self.reclaim();
        let new = Box::into_raw(Box::new(value));
        let superseded = self.pending.swap(new, Ordering::AcqRel);
        if !superseded.is_null() {
            // SAFETY: never observed by callback; unique ownership obtained by swap.
            unsafe { drop(Box::from_raw(superseded)) };
        }
    }

    /// SPSC publication with control-thread inheritance from a superseded value.
    /// The callback sees either complete value, never the intermediate merge.
    pub(super) fn publish_merged(&self, mut value: T, merge: impl FnOnce(&mut T, &mut T)) {
        self.reclaim();
        let superseded = self.pending.swap(ptr::null_mut(), Ordering::AcqRel);
        if !superseded.is_null() {
            // SAFETY: swap removed the pointer before the callback could own it.
            let mut previous = unsafe { Box::from_raw(superseded) };
            merge(&mut value, &mut previous);
        }
        self.pending
            .store(Box::into_raw(Box::new(value)), Ordering::Release);
    }

    pub(super) fn commit_into(&self, target: &mut Box<T>) -> bool {
        let Some(retire_idx) = self
            .retired
            .iter()
            .position(|slot| slot.load(Ordering::Acquire).is_null())
        else {
            return false;
        };
        let pending = self.pending.swap(ptr::null_mut(), Ordering::AcqRel);
        if pending.is_null() {
            return false;
        }
        // SAFETY: produced by Box::into_raw in publish(); exclusively owned here.
        let next = unsafe { Box::from_raw(pending) };
        let old = std::mem::replace(target, next);
        self.retired[retire_idx].store(Box::into_raw(old), Ordering::Release);
        true
    }

    pub(super) fn has_pending(&self) -> bool {
        !self.pending.load(Ordering::Acquire).is_null()
    }

    /// Off-RT only: drop pending and retired values while no audio callback is running.
    #[cfg(feature = "audio")]
    pub(super) fn clear(&self) {
        for slot in std::iter::once(&self.pending).chain(self.retired.iter()) {
            let ptr = slot.swap(ptr::null_mut(), Ordering::AcqRel);
            if !ptr.is_null() {
                // SAFETY: called strictly off-RT after the audio stream/worker has stopped.
                unsafe { drop(Box::from_raw(ptr)) };
            }
        }
    }
}

impl<T> Drop for AtomicBoxSlot<T> {
    fn drop(&mut self) {
        for slot in std::iter::once(&self.pending).chain(self.retired.iter()) {
            let ptr = slot.swap(ptr::null_mut(), Ordering::AcqRel);
            if !ptr.is_null() {
                unsafe { drop(Box::from_raw(ptr)) };
            }
        }
    }
}

pub(super) enum AuditionKind {
    Sample {
        sample: Arc<SampleData>,
        zone: RackZone,
        graph: CompiledZoneGraph,
    },
    Rack {
        patch: RackPatch,
        compiled_zones: Vec<CompiledZoneGraph>,
        midi: u8,
        velocity: f32,
    },
    Stop,
}
pub(super) struct AuditionRequest {
    pub(super) kind: AuditionKind,
    pub(super) sequence: u64,
    pub(super) project_generation: u64,
}

#[derive(Clone, Copy, Default)]
pub(super) struct LiveEvent {
    pub(super) packed: u64,
    pub(super) track_id: u64,
    pub(super) sequence: u64,
    pub(super) project_generation: u64,
    pub(super) source_part: Option<u64>,
}

/// The identity boundary travels with the score; coalescing publications cannot
/// lose it and the callback retires the replaced score on the control thread.
pub(super) struct ProjectPublication {
    pub(super) project: Option<Box<ScoreWindow>>,
    pub(super) samples: Option<Box<SampleBank>>,
    /// Control-thread pre-allocated `Delay` instances handed into `MixerEngine` via
    /// zero-alloc/zero-drop swap.
    pub(super) extra_delays: Vec<Delay>,
    pub(super) generation: u64,
}

pub(super) struct SharedChannelMeter {
    pub(super) kind: AtomicU32,
    pub(super) id: AtomicU64,
    pub(super) input: [AtomicU32; 2],
    pub(super) input_hold: [AtomicU32; 2],
    pub(super) output: [AtomicU32; 2],
    pub(super) output_hold: [AtomicU32; 2],
    pub(super) hot: AtomicBool,
    pub(super) clipped: AtomicBool,
}
impl SharedChannelMeter {
    pub(super) fn new() -> Self {
        Self {
            kind: AtomicU32::new(0),
            id: AtomicU64::new(0),
            input: std::array::from_fn(|_| AtomicU32::new(0)),
            input_hold: std::array::from_fn(|_| AtomicU32::new(0)),
            output: std::array::from_fn(|_| AtomicU32::new(0)),
            output_hold: std::array::from_fn(|_| AtomicU32::new(0)),
            hot: AtomicBool::new(false),
            clipped: AtomicBool::new(false),
        }
    }
    pub(super) fn write(&self, meter: ChannelMeter) {
        let (kind, id) = match meter.channel {
            ChannelRef::Channel(id) => (2, id),
            ChannelRef::Master => (3, 0),
        };
        self.id.store(id, Ordering::Relaxed);
        for channel in 0..2 {
            self.input[channel].store(meter.input_peak[channel].to_bits(), Ordering::Relaxed);
            self.input_hold[channel]
                .store(meter.input_peak_hold[channel].to_bits(), Ordering::Relaxed);
            self.output[channel].store(meter.output_peak[channel].to_bits(), Ordering::Relaxed);
            self.output_hold[channel]
                .store(meter.output_peak_hold[channel].to_bits(), Ordering::Relaxed);
        }
        self.hot.store(meter.hot, Ordering::Relaxed);
        self.clipped.store(meter.clipped, Ordering::Relaxed);
        self.kind.store(kind, Ordering::Relaxed);
    }
    pub(super) fn read(&self) -> Option<ChannelMeter> {
        let id = self.id.load(Ordering::Relaxed);
        let channel = match self.kind.load(Ordering::Relaxed) {
            2 => ChannelRef::Channel(id),
            3 => ChannelRef::Master,
            _ => return None,
        };
        Some(ChannelMeter {
            channel,
            input_peak: std::array::from_fn(|i| {
                f32::from_bits(self.input[i].load(Ordering::Relaxed))
            }),
            input_peak_hold: std::array::from_fn(|i| {
                f32::from_bits(self.input_hold[i].load(Ordering::Relaxed))
            }),
            output_peak: std::array::from_fn(|i| {
                f32::from_bits(self.output[i].load(Ordering::Relaxed))
            }),
            output_peak_hold: std::array::from_fn(|i| {
                f32::from_bits(self.output_hold[i].load(Ordering::Relaxed))
            }),
            hot: self.hot.load(Ordering::Relaxed),
            clipped: self.clipped.load(Ordering::Relaxed),
        })
    }
}

pub(super) struct SharedEffectMeter {
    pub(super) valid: AtomicBool,
    pub(super) device_id: AtomicU64,
    pub(super) gain_reduction_db: AtomicU32,
    pub(super) gr_hold_db: AtomicU32,
}
impl SharedEffectMeter {
    pub(super) fn new() -> Self {
        Self {
            valid: AtomicBool::new(false),
            device_id: AtomicU64::new(0),
            gain_reduction_db: AtomicU32::new(0.0_f32.to_bits()),
            gr_hold_db: AtomicU32::new(0.0_f32.to_bits()),
        }
    }
    pub(super) fn write(&self, meter: EffectMeter) {
        self.device_id.store(meter.device_id, Ordering::Relaxed);
        self.gain_reduction_db
            .store(meter.gain_reduction_db.to_bits(), Ordering::Relaxed);
        self.gr_hold_db
            .store(meter.gr_hold_db.to_bits(), Ordering::Relaxed);
        self.valid.store(true, Ordering::Relaxed);
    }
    pub(super) fn clear(&self) {
        self.valid.store(false, Ordering::Relaxed);
    }
    pub(super) fn read(&self) -> Option<EffectMeter> {
        if !self.valid.load(Ordering::Relaxed) {
            return None;
        }
        Some(EffectMeter {
            device_id: self.device_id.load(Ordering::Relaxed),
            gain_reduction_db: f32::from_bits(self.gain_reduction_db.load(Ordering::Relaxed)),
            gr_hold_db: f32::from_bits(self.gr_hold_db.load(Ordering::Relaxed)),
        })
    }
}

pub(super) struct Shared {
    pub(super) playing: AtomicBool,
    pub(super) alive: AtomicBool,
    pub(super) failed: AtomicBool,
    pub(super) step_position: AtomicU64,
    pub(super) next_step: AtomicU64,
    pub(super) transport_events: [AtomicU64; TRANSPORT_MAILBOXES],
    pub(super) transport_pending: AtomicUsize,
    pub(super) transport_sequences: [AtomicU64; TRANSPORT_MAILBOXES],
    pub(super) transport_generations: [AtomicU64; TRANSPORT_MAILBOXES],
    pub(super) mixer_meter_generation: AtomicU64,
    pub(super) mixer_meters: Box<[SharedChannelMeter]>,
    pub(super) effect_meters: Box<[SharedEffectMeter]>,
    pub(super) clear_all_meters: AtomicBool,
    pub(super) peak: AtomicU32,
    pub(super) peak_left: AtomicU32,
    pub(super) peak_right: AtomicU32,
    pub(super) sample_rate: AtomicU32,
    pub(super) prepared_delays: AtomicUsize,
    pub(super) track_ids: [AtomicU64; MAX_INSTRUMENTS],
    pub(super) track_count: AtomicUsize,
    pub(super) scope_generation: AtomicU64,
    pub(super) track_waveform: Box<[[AtomicU32; SCOPE_BUFFER_SAMPLES]]>,
    pub(super) scope_page: AtomicUsize,
    pub(super) scope_sequence: AtomicU64,
    pub(super) input_track_ids: [AtomicU64; MAX_INSTRUMENTS],
    pub(super) input_track_count: AtomicUsize,
    pub(super) note_owners: Box<[[AtomicU64; 128]]>,
    pub(super) note_owned: Box<[[AtomicBool; 128]]>,
    pub(super) waveform: Box<[AtomicU32]>,
    pub(super) wave_write: AtomicUsize,
    /// Packed live event mailboxes:
    /// tag 1 = finite one-shot audition
    /// tag 2 = live Jam Key NoteOn (sustains until NoteOff)
    /// tag 3 = live Jam Key NoteOff
    pub(super) events: [AtomicU64; MAILBOXES],
    pub(super) event_track_ids: [AtomicU64; MAILBOXES],
    pub(super) event_bindings: [AtomicU64; MAILBOXES],
    pub(super) event_sequences: [AtomicU64; MAILBOXES],
    pub(super) event_generations: [AtomicU64; MAILBOXES],
    pub(super) event_sequence: AtomicU64,
    pub(super) event_overflows: AtomicU64,
    pub(super) release_all_requested: AtomicBool,
    pub(super) input_generation: AtomicU64,
    pub(super) project_slot: AtomicBoxSlot<ProjectPublication>,
    pub(super) audition_slot: AtomicBoxSlot<Option<AuditionRequest>>,
    /// Lock-free SPSC stereo audio tap ring for live speaker capture (`listen --live`).
    pub(super) tap_enabled: AtomicBool,
    pub(super) tap_write: AtomicU64,
    pub(super) tap_read: AtomicU64,
    pub(super) tap_dropped: AtomicU64,
    pub(super) audio_tap: Box<[[AtomicU32; 2]]>,
    /// Lock-free callback budget & stream health telemetry (RT-written, off-RT-read).
    pub(super) cpu_load_permille: AtomicU32,
    pub(super) peak_cpu_permille: AtomicU32,
    pub(super) xrun_count: AtomicU64,
    pub(super) overload_tripped: AtomicBool,
    pub(super) error_code: AtomicU8,
    pub(super) buffer_frames: AtomicU32,
    pub(super) channels: AtomicU16,
    pub(super) bit_depth: AtomicU16,
    pub(super) sample_format_is_float: AtomicBool,
    pub(super) is_silent: AtomicBool,
    /// Off-RT-only diagnostic message (CPAL error callback or open/restart error).
    /// NEVER touched from the audio render callback thread.
    pub(super) last_error: Mutex<Option<String>>,

    /// Lock-free hardware audio input status, metering, capture gate, and SPSC rings.
    pub(super) input_status: AtomicU8,
    pub(super) input_sample_rate: AtomicU32,
    pub(super) input_channels: AtomicU16,
    pub(super) input_buffer_frames: AtomicU32,
    pub(super) input_mode: AtomicU8,
    pub(super) input_monitor_mode: AtomicU8,
    pub(super) input_monitoring_active: AtomicBool,
    pub(super) input_peak: [AtomicU32; 2],
    pub(super) input_peak_hold: [AtomicU32; 2],
    pub(super) input_clipped: AtomicBool,
    pub(super) input_clear_meters: AtomicBool,
    pub(super) input_callbacks: AtomicU64,
    pub(super) input_nonzero_seen: AtomicBool,
    /// Off-RT-only diagnostic message for the audio input stream.
    pub(super) input_last_error: Mutex<Option<String>>,
    /// Transport-synchronized capture gate state (`CapturePhase`).
    pub(super) capture_phase: AtomicU8,
    pub(super) capture_quantized: AtomicBool,
    pub(super) capture_punch_in_step: AtomicU64,
    pub(super) capture_has_punch_out: AtomicBool,
    pub(super) capture_punch_out_step: AtomicU64,
    pub(super) capture_start_step: AtomicU64,
    pub(super) capture_end_step: AtomicU64,
    pub(super) capture_tail_frames: AtomicU32,
    pub(super) capture_write: AtomicU64,
    pub(super) capture_read: AtomicU64,
    pub(super) capture_dropped: AtomicU64,
    pub(super) capture_frames_written: AtomicU64,
    pub(super) capture_ring: Box<[[AtomicU32; 2]]>,
    /// Lock-free SPSC stereo ring feeding input monitoring into the output `Engine`.
    pub(super) monitor_write: AtomicU64,
    pub(super) monitor_read: AtomicU64,
    pub(super) monitor_ring: Box<[[AtomicU32; 2]]>,
}

impl Shared {
    /// Allocate `Shared` directly on the heap with boxed inner arrays so debug
    /// stack frames stay small.
    pub(super) fn new_arc() -> Arc<Self> {
        let mixer_meters = (0..MAX_CHANNELS)
            .map(|_| SharedChannelMeter::new())
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let effect_meters = (0..MAX_EFFECT_DEVICES)
            .map(|_| SharedEffectMeter::new())
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let track_waveform = (0..MAX_INSTRUMENTS)
            .map(|_| std::array::from_fn(|_| AtomicU32::new(0.0_f32.to_bits())))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let note_owners = (0..MAX_INSTRUMENTS)
            .map(|_| std::array::from_fn(|_| AtomicU64::new(0)))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let note_owned = (0..MAX_INSTRUMENTS)
            .map(|_| std::array::from_fn(|_| AtomicBool::new(false)))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let waveform = (0..WAVE_SAMPLES)
            .map(|_| AtomicU32::new(0.0_f32.to_bits()))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let audio_tap = (0..AUDIO_TAP_CAPACITY)
            .map(|_| [AtomicU32::new(0), AtomicU32::new(0)])
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let capture_ring = (0..AUDIO_CAPTURE_CAPACITY)
            .map(|_| [AtomicU32::new(0), AtomicU32::new(0)])
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let monitor_ring = (0..AUDIO_MONITOR_CAPACITY)
            .map(|_| [AtomicU32::new(0), AtomicU32::new(0)])
            .collect::<Vec<_>>()
            .into_boxed_slice();

        Arc::new(Self {
            playing: AtomicBool::new(false),
            alive: AtomicBool::new(true),
            failed: AtomicBool::new(false),
            step_position: AtomicU64::new(0),
            next_step: AtomicU64::new(0),
            transport_events: std::array::from_fn(|_| AtomicU64::new(0)),
            transport_pending: AtomicUsize::new(0),
            transport_sequences: std::array::from_fn(|_| AtomicU64::new(0)),
            transport_generations: std::array::from_fn(|_| AtomicU64::new(0)),
            mixer_meter_generation: AtomicU64::new(0),
            mixer_meters,
            effect_meters,
            clear_all_meters: AtomicBool::new(false),
            peak: AtomicU32::new(0.0_f32.to_bits()),
            peak_left: AtomicU32::new(0.0_f32.to_bits()),
            peak_right: AtomicU32::new(0.0_f32.to_bits()),
            sample_rate: AtomicU32::new(SILENT_RATE),
            prepared_delays: AtomicUsize::new(0),
            track_ids: std::array::from_fn(|_| AtomicU64::new(0)),
            track_count: AtomicUsize::new(0),
            scope_generation: AtomicU64::new(0),
            track_waveform,
            scope_page: AtomicUsize::new(0),
            scope_sequence: AtomicU64::new(0),
            input_track_ids: std::array::from_fn(|_| AtomicU64::new(0)),
            input_track_count: AtomicUsize::new(0),
            note_owners,
            note_owned,
            waveform,
            wave_write: AtomicUsize::new(0),
            events: std::array::from_fn(|_| AtomicU64::new(0)),
            event_track_ids: std::array::from_fn(|_| AtomicU64::new(0)),
            event_bindings: std::array::from_fn(|_| AtomicU64::new(0)),
            event_sequences: std::array::from_fn(|_| AtomicU64::new(0)),
            event_generations: std::array::from_fn(|_| AtomicU64::new(0)),
            event_sequence: AtomicU64::new(0),
            event_overflows: AtomicU64::new(0),
            release_all_requested: AtomicBool::new(false),
            input_generation: AtomicU64::new(0),
            project_slot: AtomicBoxSlot::new(),
            audition_slot: AtomicBoxSlot::new(),
            tap_enabled: AtomicBool::new(false),
            tap_write: AtomicU64::new(0),
            tap_read: AtomicU64::new(0),
            tap_dropped: AtomicU64::new(0),
            audio_tap,
            cpu_load_permille: AtomicU32::new(0),
            peak_cpu_permille: AtomicU32::new(0),
            xrun_count: AtomicU64::new(0),
            overload_tripped: AtomicBool::new(false),
            error_code: AtomicU8::new(AudioErrorCode::None as u8),
            buffer_frames: AtomicU32::new(0),
            channels: AtomicU16::new(0),
            bit_depth: AtomicU16::new(0),
            sample_format_is_float: AtomicBool::new(false),
            is_silent: AtomicBool::new(false),
            last_error: Mutex::new(None),
            input_status: AtomicU8::new(AudioInputStatus::Inactive as u8),
            input_sample_rate: AtomicU32::new(0),
            input_channels: AtomicU16::new(0),
            input_buffer_frames: AtomicU32::new(0),
            input_mode: AtomicU8::new(AudioInputMode::Stereo.to_u8()),
            input_monitor_mode: AtomicU8::new(InputMonitorMode::Off.to_u8()),
            input_monitoring_active: AtomicBool::new(false),
            input_peak: std::array::from_fn(|_| AtomicU32::new(0.0_f32.to_bits())),
            input_peak_hold: std::array::from_fn(|_| AtomicU32::new(0.0_f32.to_bits())),
            input_clipped: AtomicBool::new(false),
            input_clear_meters: AtomicBool::new(false),
            input_callbacks: AtomicU64::new(0),
            input_nonzero_seen: AtomicBool::new(false),
            input_last_error: Mutex::new(None),
            capture_phase: AtomicU8::new(CapturePhase::Idle as u8),
            capture_quantized: AtomicBool::new(true),
            capture_punch_in_step: AtomicU64::new(0),
            capture_has_punch_out: AtomicBool::new(false),
            capture_punch_out_step: AtomicU64::new(0),
            capture_start_step: AtomicU64::new(0),
            capture_end_step: AtomicU64::new(0),
            capture_tail_frames: AtomicU32::new(0),
            capture_write: AtomicU64::new(0),
            capture_read: AtomicU64::new(0),
            capture_dropped: AtomicU64::new(0),
            capture_frames_written: AtomicU64::new(0),
            capture_ring,
            monitor_write: AtomicU64::new(0),
            monitor_read: AtomicU64::new(0),
            monitor_ring,
        })
    }

    /// Off-RT only: record or clear the human-readable stream/device diagnostic.
    #[cfg_attr(not(feature = "audio"), allow(dead_code))]
    pub(super) fn set_last_error(&self, message: Option<String>) {
        let mut guard = self
            .last_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = message;
    }

    /// Off-RT only: clone the last recorded stream/device diagnostic, if any.
    pub(super) fn last_error(&self) -> Option<String> {
        self.last_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Off-RT only: record or clear the human-readable audio input stream diagnostic.
    pub(super) fn set_input_last_error(&self, message: Option<String>) {
        let mut guard = self
            .input_last_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = message;
    }

    /// Off-RT only: clone the last recorded audio input stream diagnostic, if any.
    pub(super) fn input_last_error(&self) -> Option<String> {
        self.input_last_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Lock-free atomic snapshot of live hardware audio input metering, monitoring,
    /// and capture gate state.
    pub(super) fn read_input_telemetry(&self) -> AudioInputTelemetry {
        let status = AudioInputStatus::from_u8(self.input_status.load(Ordering::Acquire));
        AudioInputTelemetry {
            status,
            sample_rate: self.input_sample_rate.load(Ordering::Relaxed),
            channels: self.input_channels.load(Ordering::Relaxed),
            buffer_frames: self.input_buffer_frames.load(Ordering::Relaxed),
            peak: [
                f32::from_bits(self.input_peak[0].load(Ordering::Relaxed)),
                f32::from_bits(self.input_peak[1].load(Ordering::Relaxed)),
            ],
            peak_hold: [
                f32::from_bits(self.input_peak_hold[0].load(Ordering::Relaxed)),
                f32::from_bits(self.input_peak_hold[1].load(Ordering::Relaxed)),
            ],
            clipped: self.input_clipped.load(Ordering::Relaxed),
            digital_silence: status == AudioInputStatus::Running
                && self.input_callbacks.load(Ordering::Relaxed) >= 24
                && !self.input_nonzero_seen.load(Ordering::Relaxed),
            mode: AudioInputMode::from_u8(self.input_mode.load(Ordering::Relaxed)),
            monitor_mode: InputMonitorMode::from_u8(
                self.input_monitor_mode.load(Ordering::Relaxed),
            ),
            monitoring: self.input_monitoring_active.load(Ordering::Acquire),
            capture_phase: CapturePhase::from_u8(self.capture_phase.load(Ordering::Acquire)),
            capture_start_step: self.capture_start_step.load(Ordering::Acquire),
            capture_end_step: self.capture_end_step.load(Ordering::Acquire),
            captured_frames: self.capture_frames_written.load(Ordering::Acquire),
            dropped_frames: self.capture_dropped.load(Ordering::Relaxed),
        }
    }

    /// Off-RT only: clear live audio input metering & status when closing the input stream.
    pub(super) fn reset_input_stream_state(&self) {
        self.input_status
            .store(AudioInputStatus::Inactive as u8, Ordering::Release);
        self.input_sample_rate.store(0, Ordering::Relaxed);
        self.input_channels.store(0, Ordering::Relaxed);
        self.input_buffer_frames.store(0, Ordering::Relaxed);
        self.input_monitoring_active.store(false, Ordering::Release);
        for slot in &self.input_peak {
            slot.store(0.0_f32.to_bits(), Ordering::Relaxed);
        }
        for slot in &self.input_peak_hold {
            slot.store(0.0_f32.to_bits(), Ordering::Relaxed);
        }
        self.input_clipped.store(false, Ordering::Relaxed);
        self.input_callbacks.store(0, Ordering::Relaxed);
        self.input_nonzero_seen.store(false, Ordering::Relaxed);
    }

    /// Recompute `input_monitoring_active` from current monitor policy, stream status, and capture phase.
    #[inline]
    pub(super) fn refresh_monitoring_active(&self) -> bool {
        let status = AudioInputStatus::from_u8(self.input_status.load(Ordering::Acquire));
        let active = if status != AudioInputStatus::Running {
            false
        } else {
            let mode = InputMonitorMode::from_u8(self.input_monitor_mode.load(Ordering::Acquire));
            match mode {
                InputMonitorMode::Off => false,
                InputMonitorMode::On => true,
                InputMonitorMode::Auto => {
                    let phase = CapturePhase::from_u8(self.capture_phase.load(Ordering::Acquire));
                    matches!(
                        phase,
                        CapturePhase::Armed | CapturePhase::Recording | CapturePhase::Finishing
                    )
                }
            }
        };
        self.input_monitoring_active
            .store(active, Ordering::Release);
        active
    }

    /// Lock-free atomic snapshot of live stream health & callback budget telemetry.
    pub(super) fn read_health(&self) -> AudioHealth {
        let failed = self.failed.load(Ordering::Acquire);
        let overloaded = self.overload_tripped.load(Ordering::Acquire);
        let is_silent = self.is_silent.load(Ordering::Acquire);
        let status = if failed {
            AudioStreamStatus::Failed
        } else if overloaded {
            AudioStreamStatus::Overloaded
        } else if is_silent {
            AudioStreamStatus::Silent
        } else {
            AudioStreamStatus::Running
        };
        let error_code = AudioErrorCode::from_u8(self.error_code.load(Ordering::Acquire));
        AudioHealth {
            status,
            cpu_load: self.cpu_load_permille.load(Ordering::Relaxed) as f32 / 1000.0,
            peak_cpu_load: self.peak_cpu_permille.load(Ordering::Relaxed) as f32 / 1000.0,
            xrun_count: self.xrun_count.load(Ordering::Relaxed),
            buffer_frames: self.buffer_frames.load(Ordering::Relaxed),
            channels: self.channels.load(Ordering::Relaxed),
            bit_depth: self.bit_depth.load(Ordering::Relaxed),
            sample_format_is_float: self.sample_format_is_float.load(Ordering::Relaxed),
            overloaded,
            failed,
            error_code,
        }
    }

    /// Strictly RT-safe: clear a latched overload circuit breaker and its matching error code.
    #[inline]
    pub(super) fn clear_overload_latch(&self) {
        if self.overload_tripped.swap(false, Ordering::AcqRel) {
            let _ = self.error_code.compare_exchange(
                AudioErrorCode::OverloadCircuitBreaker as u8,
                AudioErrorCode::None as u8,
                Ordering::AcqRel,
                Ordering::Relaxed,
            );
        }
    }

    /// Off-RT only: drain pending mailboxes and publication slots while no callback is active.
    #[cfg(feature = "audio")]
    pub(super) fn reset_for_reconfigure(&self) {
        for slot in &self.events {
            slot.store(0, Ordering::Release);
        }
        for slot in &self.transport_events {
            slot.store(0, Ordering::Release);
        }
        self.transport_pending.store(0, Ordering::Release);
        self.release_all_requested.store(false, Ordering::Release);
        self.clear_all_meters.store(false, Ordering::Release);
        for track in self.note_owned.iter() {
            for owned in track {
                owned.store(false, Ordering::Release);
            }
        }
        self.project_slot.clear();
        self.audition_slot.clear();
        self.cpu_load_permille.store(0, Ordering::Relaxed);
        self.peak_cpu_permille.store(0, Ordering::Relaxed);
        self.xrun_count.store(0, Ordering::Relaxed);
        self.peak.store(0.0_f32.to_bits(), Ordering::Relaxed);
        self.peak_left.store(0.0_f32.to_bits(), Ordering::Relaxed);
        self.peak_right.store(0.0_f32.to_bits(), Ordering::Relaxed);
        for sample in self.waveform.iter() {
            sample.store(0.0_f32.to_bits(), Ordering::Relaxed);
        }
        self.next_step.store(0, Ordering::Release);
        self.overload_tripped.store(false, Ordering::Release);
        self.failed.store(false, Ordering::Release);
        self.error_code
            .store(AudioErrorCode::None as u8, Ordering::Release);
        self.playing.store(false, Ordering::Release);
        self.alive.store(true, Ordering::Release);
        self.set_last_error(None);
    }

    /// Off-RT helper: pre-allocate any additional `Delay` buffers needed by `project`
    /// beyond what has already been prepared for this engine.
    pub(super) fn prepare_extra_delays(&self, project: &ScoreWindow) -> Vec<Delay> {
        let needed = crate::mix_engine::count_delay_devices(project);
        let sr = self
            .sample_rate
            .load(Ordering::Acquire)
            .clamp(MIN_AUDIO_SAMPLE_RATE, MAX_AUDIO_SAMPLE_RATE);
        let mut prev = self.prepared_delays.load(Ordering::Acquire);
        loop {
            if needed <= prev {
                return Vec::new();
            }
            match self.prepared_delays.compare_exchange_weak(
                prev,
                needed,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    let count = needed - prev;
                    return (0..count).map(|_| Delay::new(sr)).collect();
                }
                Err(actual) => prev = actual,
            }
        }
    }

    pub(super) fn reclaim_all(&self) {
        self.project_slot.reclaim();
        self.audition_slot.reclaim();
    }

    pub(super) fn publish_input_tracks(&self, project: &ScoreWindow) {
        // The control thread publishes desired identities before its score. Live
        // events can then target an existing part across a queued reordering.
        for (index, slot) in self.input_track_ids.iter().enumerate() {
            slot.store(
                project.instruments.get(index).map_or(0, |t| t.id),
                Ordering::Relaxed,
            );
        }
        self.input_track_count
            .store(project.instruments.len(), Ordering::Release);
    }

    pub(super) fn input_track_id(&self, track: usize) -> Option<u64> {
        (track
            < self
                .input_track_count
                .load(Ordering::Acquire)
                .min(MAX_INSTRUMENTS))
        .then(|| self.input_track_ids[track].load(Ordering::Relaxed))
    }

    pub(super) fn push_event_for_id(&self, packed: u64, track_id: u64) -> bool {
        self.push_bound_event(packed, track_id, None)
    }

    pub(super) fn push_bound_event(
        &self,
        packed: u64,
        track_id: u64,
        binding: Option<u64>,
    ) -> bool {
        let packed = packed
            | if binding.is_some() {
                EVENT_HAS_BINDING
            } else {
                0
            };
        if packed == 0 || !self.alive.load(Ordering::Acquire) {
            return false;
        }
        for (index, slot) in self.events.iter().enumerate() {
            if slot
                .compare_exchange(0, EVENT_RESERVED, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                self.event_track_ids[index].store(track_id, Ordering::Relaxed);
                self.event_bindings[index].store(binding.unwrap_or(0), Ordering::Relaxed);
                self.event_generations[index].store(
                    self.input_generation.load(Ordering::Acquire),
                    Ordering::Relaxed,
                );
                let sequence = self
                    .event_sequence
                    .fetch_add(1, Ordering::AcqRel)
                    .wrapping_add(1);
                self.event_sequences[index].store(sequence, Ordering::Relaxed);
                slot.store(packed, Ordering::Release);
                return true;
            }
        }
        self.event_overflows.fetch_add(1, Ordering::Relaxed);
        false
    }

    pub(super) fn request_release_all(&self) {
        self.release_all_requested.store(true, Ordering::Release);
        for track in self.note_owned.iter() {
            for owned in track {
                owned.store(false, Ordering::Release);
            }
        }
    }

    pub(super) fn push_transport(&self, intent: TransportIntent) -> bool {
        if !self.alive.load(Ordering::Acquire) || self.failed.load(Ordering::Acquire) {
            return false;
        }
        for (index, slot) in self.transport_events.iter().enumerate() {
            if slot
                .compare_exchange(0, EVENT_RESERVED, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                let sequence = self
                    .event_sequence
                    .fetch_add(1, Ordering::AcqRel)
                    .wrapping_add(1);
                self.transport_sequences[index].store(sequence, Ordering::Relaxed);
                self.transport_generations[index].store(
                    self.input_generation.load(Ordering::Acquire),
                    Ordering::Relaxed,
                );
                self.transport_pending.fetch_add(1, Ordering::AcqRel);
                slot.store(5 | (pack_transport_intent(intent) << 3), Ordering::Release);
                return true;
            }
        }
        false
    }
}

/// Load every required sampler asset, returning track/path diagnostics on failure.
pub fn build_sample_bank(project: &ScoreWindow, library: &mut SampleLibrary) -> Result<SampleBank> {
    let report = load_project_samples(project, library);
    report.ensure_ready()?;
    Ok(report.bank)
}
