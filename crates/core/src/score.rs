//! Flat executable score (`ScoreWindow`) shared by real-time playback,
//! offline WAV rendering, asset readiness, and arrangement projection.

use crate::material::{EventOrigin, Id};
use crate::mixer::{Channel, ChannelParam, ChannelRef, EffectDevice, Mixer, MixerParamTarget};
use crate::music::{EndRule, MAX_CHORD_TONES, STEPS_PER_BAR};
use crate::params::{InstrumentParam, ValueMode};
use crate::rack::{compile_zone_graph, CompiledZoneGraph, RackPatch};
use anyhow::{ensure, Result};

pub use crate::material::MAX_INSTRUMENTS;
pub const MAX_BARS: usize = crate::material::MAX_BARS;

/// Transport step window covered by a live ring score.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveWindow {
    /// First transport step represented in the ring (inclusive).
    pub start: u64,
    /// Exclusive upper bound (`start + ring_steps`).
    pub end: u64,
    /// Transport step where playback stops, when the song's end rule is Stop.
    pub stop_at: Option<u64>,
}

impl LiveWindow {
    pub fn contains(self, step: u64) -> bool {
        step >= self.start && step < self.end
    }

    pub fn length(self) -> u64 {
        self.end.saturating_sub(self.start).max(1)
    }

    pub fn slot(self, step: u64) -> usize {
        (step % self.length()) as usize
    }
}

/// One resolved chord tone attached to a `ScoreEvent`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ResolvedChordTone {
    pub midi: u8,
    pub velocity: f32,
    pub gate: f32,
}

/// One per-hit normalized parameter lock attached to a `ScoreEvent`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamLock {
    pub target: InstrumentParam,
    pub value: f32,
}

/// One dense instrument entry inside a `ScoreWindow`.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreInstrument {
    pub id: Id,
    pub name: String,
    pub rack: RackPatch,
    /// Pre-compiled flat topological DAGs aligned 1:1 with `rack.zones`.
    pub compiled_zones: Vec<CompiledZoneGraph>,
    pub channel: Id,
}

impl ScoreInstrument {
    /// Re-compile all `rack.zones` into flat [`CompiledZoneGraph`] programs.
    /// Must be called after any static macro mutation so `CompiledCable.amount` matches `rack`.
    pub fn compile_zones(&mut self) -> Result<()> {
        self.compiled_zones.clear();
        self.compiled_zones.reserve(self.rack.zones.len());
        for zone in &self.rack.zones {
            self.compiled_zones.push(compile_zone_graph(zone)?);
        }
        Ok(())
    }

    /// Look up the compiled DAG for stable `zone_id`.
    #[inline]
    pub fn compiled_zone(&self, zone_id: u16) -> Option<&CompiledZoneGraph> {
        self.compiled_zones.iter().find(|g| g.zone_id == zone_id)
    }

    /// True if any zone inside this Rack contains a `Sampler` module.
    pub fn has_samplers(&self) -> bool {
        self.rack.has_samplers()
    }
}

/// One scheduled note hit (possibly polyphonic chord + per-hit parameter locks)
/// at fractional offset `offset ∈ [0.0, 1.0)` inside a 16th-note step.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreEvent {
    pub inst_idx: u8,
    pub part_id: Option<Id>,
    /// Fractional position inside the 16th-note step (`0.0 <= offset < 1.0`).
    pub offset: f32,
    pub degree: i16,
    pub accidental: i8,
    pub midi: u8,
    pub velocity: f32,
    pub gate: f32,
    pub chord: [ResolvedChordTone; MAX_CHORD_TONES],
    pub chord_len: u8,
    pub lock_start: u32,
    pub lock_len: u16,
}

impl ScoreEvent {
    pub fn chord_tones(&self) -> &[ResolvedChordTone] {
        &self.chord[..usize::from(self.chord_len)]
    }
}

/// Active continuous modulation lane targeting one instrument parameter.
/// Enforces the Value Rule structurally: only modulated targets exist here.
#[derive(Clone, Debug, PartialEq)]
pub struct ModLane {
    pub inst_idx: u8,
    pub target: InstrumentParam,
    pub mode: ValueMode,
    /// Normalized `0.0..=1.0` value sampled at each step `0..timeline_steps`.
    pub values: Vec<f32>,
}

impl ModLane {
    /// Evaluate this modulation lane at fractional step position `pos` (in `[0, len)`).
    #[inline]
    pub fn evaluate(&self, pos: f64) -> f32 {
        let len = self.values.len();
        if len == 0 {
            return 0.0;
        }
        let wrapped = pos.rem_euclid(len as f64);
        let cur = (wrapped.floor() as usize) % len;
        match self.mode {
            ValueMode::Hold => self.values[cur].clamp(0.0, 1.0),
            ValueMode::Ramp => {
                if len == 1 {
                    return self.values[0].clamp(0.0, 1.0);
                }
                let next = (cur + 1) % len;
                let frac = (wrapped - wrapped.floor()) as f32;
                let a = self.values[cur];
                let b = self.values[next];
                (a + (b - a) * frac).clamp(0.0, 1.0)
            }
        }
    }
}

/// Flat executable score for a finite song or live sliding ring.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreWindow {
    pub title: String,
    pub bpm: f64,
    pub root_midi: u8,
    pub scale: Vec<i8>,
    pub bars: usize,
    pub end_rule: EndRule,
    pub live_window: Option<LiveWindow>,
    pub instruments: Vec<ScoreInstrument>,
    pub mixer: Mixer,
    /// Flat per-step events sorted by `(step_slot, offset, inst_idx)`.
    pub events: Vec<ScoreEvent>,
    /// `(start, end)` bounds into `events` for each step slot `0..timeline_steps`.
    pub step_ranges: Vec<(u32, u32)>,
    /// Flat per-event parameter locks referenced by `ScoreEvent::{lock_start, lock_len}`.
    pub locks: Vec<ParamLock>,
    /// Active continuous/step instrument modulations (Value Rule: unmodulated params absent).
    pub mod_lanes: Vec<ModLane>,
    /// Sorted unique instrument IDs that emit at least one event in this window.
    pub required_instrument_ids: Vec<Id>,
    /// Provenance records linking emitted events back to source lanes/placements/parts/patterns.
    pub provenance: Vec<EventOrigin>,
}

impl ScoreWindow {
    pub fn bars(&self) -> usize {
        self.bars.max(1)
    }

    pub fn arrangement_steps(&self) -> usize {
        self.bars() * STEPS_PER_BAR
    }

    pub fn step_seconds(&self) -> f64 {
        60.0 / (self.bpm.max(1.0) * 4.0)
    }

    pub fn instrument_index(&self, id: Id) -> Option<usize> {
        self.instruments.iter().position(|inst| inst.id == id)
    }

    /// Map an absolute transport step to its slot in `step_ranges` (`0..timeline_steps`).
    #[inline]
    pub fn step_slot(&self, abs_step: u64) -> Option<usize> {
        let len = self.step_ranges.len();
        if len == 0 {
            return None;
        }
        if let Some(window) = self.live_window {
            if !window.contains(abs_step) {
                return None;
            }
            Some(window.slot(abs_step).min(len - 1))
        } else {
            if self.end_rule == EndRule::Continue && abs_step >= len as u64 {
                return None;
            }
            Some((abs_step % len as u64) as usize)
        }
    }

    /// Zero-allocation slice of all events (including sub-step events) firing during `abs_step`.
    #[inline]
    pub fn step_events(&self, abs_step: u64) -> &[ScoreEvent] {
        let Some(slot) = self.step_slot(abs_step) else {
            return &[];
        };
        let (start, end) = self.step_ranges[slot];
        &self.events[start as usize..end as usize]
    }

    /// First event emitted by instrument `inst_idx` at `abs_step`, if any.
    #[inline]
    pub fn event_for(&self, inst_idx: usize, abs_step: u64) -> Option<&ScoreEvent> {
        self.step_events(abs_step)
            .iter()
            .find(|ev| usize::from(ev.inst_idx) == inst_idx)
    }

    /// Zero-allocation slice of per-hit parameter locks for `event`.
    #[inline]
    pub fn event_locks(&self, event: &ScoreEvent) -> &[ParamLock] {
        let start = event.lock_start as usize;
        let end = start + usize::from(event.lock_len);
        &self.locks[start..end]
    }

    /// Find an effect device on any mixer channel or Master by its stable ID.
    pub fn find_device(&self, id: u64) -> Option<(ChannelRef, &EffectDevice)> {
        for ch in &self.mixer.channels {
            if let Some(dev) = ch.inserts.iter().find(|d| d.id == id) {
                return Some((ChannelRef::Channel(ch.id), dev));
            }
        }
        if let Some(dev) = self.mixer.master.inserts.iter().find(|d| d.id == id) {
            return Some((ChannelRef::Master, dev));
        }
        None
    }

    pub fn channel(&self, id: u64) -> Option<&Channel> {
        self.mixer.channels.iter().find(|c| c.id == id)
    }

    pub fn channel_name(&self, channel: ChannelRef) -> Option<&str> {
        match channel {
            ChannelRef::Master => Some("Master"),
            ChannelRef::Channel(id) => self.channel(id).map(|c| c.name.as_str()),
        }
    }

    pub fn read_mixer_param(&self, target: MixerParamTarget) -> Option<f32> {
        match target {
            MixerParamTarget::Channel { channel, param } => {
                let strip = match channel {
                    ChannelRef::Channel(id) => &self.channel(id)?.strip,
                    ChannelRef::Master => &self.mixer.master.strip,
                };
                Some(match param {
                    ChannelParam::Gain => param.normalize(strip.gain),
                    ChannelParam::Pan => param.normalize(strip.pan),
                })
            }
            MixerParamTarget::ChannelGainScale { channel } => {
                let exists = match channel {
                    ChannelRef::Channel(id) => self.channel(id).is_some(),
                    ChannelRef::Master => true,
                };
                exists.then_some(1.0)
            }
            MixerParamTarget::Send { channel, target } => {
                let sends = match channel {
                    ChannelRef::Channel(id) => &self.channel(id)?.sends,
                    ChannelRef::Master => return None,
                };
                Some(
                    sends
                        .iter()
                        .find(|s| s.target == target)
                        .map_or(0.0, |s| s.amount.clamp(0.0, 1.0)),
                )
            }
            MixerParamTarget::Device { device_id, param } => {
                self.find_device(device_id)?.1.read_normalized(param)
            }
        }
    }

    /// Resolve a mixer parameter at `song_step`, applying mixer automation if present.
    pub fn resolve_mixer_param(&self, target: MixerParamTarget, song_step: f64) -> f32 {
        let base = self.read_mixer_param(target).unwrap_or(0.0);
        self.mixer
            .automations
            .iter()
            .find(|lane| lane.target == target)
            .map_or(base, |lane| {
                crate::mixer::resolve_automation(lane, song_step, self.arrangement_steps() as u32)
            })
    }

    /// Whether a specific mixer parameter has an active automation lane (Value Rule guard).
    pub fn has_mixer_automation(&self, target: MixerParamTarget) -> bool {
        self.mixer
            .automations
            .iter()
            .any(|lane| lane.target == target)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.bpm.is_finite() && (20.0..=300.0).contains(&self.bpm),
            "Tempo must be finite and between 20 and 300 BPM"
        );
        ensure!(
            (1..=MAX_BARS).contains(&self.bars),
            "Score bars must be 1..={MAX_BARS}"
        );
        ensure!(
            self.instruments.len() <= MAX_INSTRUMENTS,
            "At most {MAX_INSTRUMENTS} instruments are supported"
        );
        for inst in &self.instruments {
            ensure!(
                inst.compiled_zones.len() == inst.rack.zones.len(),
                "Score instrument '{}' compiled zone graph count mismatch",
                inst.name
            );
        }
        ensure!(
            self.step_ranges.len() == self.arrangement_steps(),
            "Score step_ranges length mismatch"
        );
        self.mixer.validate(self.arrangement_steps() as u32)?;
        Ok(())
    }

    /// Fast sound-only patch update: updates BPM, instrument patches, and mixer state
    /// while reusing all evaluated events, locks, modulations, and provenance!
    pub fn with_sound_updates(
        &self,
        bpm: f64,
        instruments: Vec<ScoreInstrument>,
        mixer: Mixer,
    ) -> Self {
        let mut next = self.clone();
        next.bpm = bpm;
        next.instruments = instruments;
        next.mixer = mixer;
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_window_validate_checks_mixer_topology() {
        let mut score = ScoreWindow {
            title: "Test".into(),
            bpm: 120.0,
            root_midi: 60,
            scale: vec![0, 2, 4, 5, 7, 9, 11],
            bars: 1,
            end_rule: EndRule::Loop,
            live_window: None,
            instruments: Vec::new(),
            mixer: Mixer::default(),
            events: Vec::new(),
            step_ranges: vec![(0, 0); STEPS_PER_BAR],
            locks: Vec::new(),
            mod_lanes: Vec::new(),
            required_instrument_ids: Vec::new(),
            provenance: Vec::new(),
        };
        score.validate().unwrap();
        score.mixer.master.strip.gain = f32::NAN;
        assert!(score.validate().is_err());
    }
}
