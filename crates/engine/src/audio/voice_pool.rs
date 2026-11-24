//! Real-time voice pool managing priority voice allocation, micro-fade voice
//! stealing, active-voice index compaction, choke groups, monophonic/legato
//! retargeting, held live notes, and per-voice continuous modulation updates.
use super::shared::{AuditionKind, AuditionRequest, PREVIEW_TRACK, VOICES};
use super::voices::{HeldNotes, Voice};
use super::*;
use crate::modular_voice::{VoiceStart, ZoneTriggerSpec};
use cadence_core::dsp::{db_to_lin, mix_seed, sanitize_sample, stereo_mid_f32};
use cadence_core::params::{InstrumentParam, RackParam, ZoneParam};
use cadence_core::rack::{
    module_sample_key, ModuleKind, ModuleParam, PitchTracking, RackPatch, RackZone, VoiceMode,
    VoiceSettings,
};
use cadence_core::score::{ParamLock, ScoreEvent, ScoreWindow, MAX_INSTRUMENTS};
use std::sync::Arc;

const MAX_MATCHED_ZONES: usize = 8;
const STEAL_FADE_SLOTS: usize = 8;
const STEAL_FADE_SEC: f64 = 0.002;
const STEAL_FADE_MIN_FRAMES: u16 = 16;
const STEAL_FADE_MAX_FRAMES: u16 = 2048;
const MONO_PITCH_SLOTS: usize = MAX_INSTRUMENTS * 4;

#[inline]
pub(super) fn steal_fade_frames(sample_rate: f64) -> u16 {
    ((STEAL_FADE_SEC * sample_rate).round() as u16)
        .clamp(STEAL_FADE_MIN_FRAMES, STEAL_FADE_MAX_FRAMES)
}

#[derive(Clone, Copy, Default)]
struct MonoPitchSlot {
    occupied: bool,
    track_id: u64,
    zone_id: u16,
    pitch: f64,
}

#[derive(Clone, Copy, Default)]
struct StealFade {
    track: u8,
    track_id: u64,
    sample: [f32; 2],
    remaining: u16,
    total: u16,
}

/// Read-only engine snapshot passed into voice-pool trigger operations.
#[derive(Clone, Copy)]
pub(super) struct ZoneTriggerCtx<'a> {
    pub(super) project: &'a ScoreWindow,
    pub(super) samples: &'a SampleBank,
    pub(super) sample_rate: f64,
    pub(super) rendered_frames: u64,
}

impl ZoneTriggerCtx<'_> {
    #[inline]
    fn sample_frames(&self, key: u64) -> usize {
        self.samples
            .iter()
            .find(|(id, _)| *id == key)
            .map_or(0, |(_, s)| s.frames)
    }
}

/// Normalized voice trigger request targeting an instrument track.
#[derive(Clone, Copy)]
pub(super) struct TriggerRequest<'a> {
    pub(super) inst_index: usize,
    pub(super) live_key: Option<u8>,
    pub(super) midi: u8,
    pub(super) velocity: f32,
    pub(super) gate_seconds: f64,
    pub(super) seed: u32,
    pub(super) mod_pos: Option<f64>,
    pub(super) locks: &'a [ParamLock],
}

/// Live Jam Key note payload targeting a resolved instrument track.
#[derive(Clone, Copy)]
pub(super) struct LiveNote {
    pub(super) track_idx: usize,
    pub(super) track_id: u64,
    pub(super) midi: u8,
    pub(super) velocity: f32,
}

/// Stereo voice outputs accumulated across one audio frame.
pub(super) struct VoiceFrameOut {
    pub(super) sources: [[f32; 2]; MAX_INSTRUMENTS],
    pub(super) preview: [f32; 2],
    pub(super) track_scope: [f32; MAX_INSTRUMENTS],
}

/// Clamp automation lookup position to the final arrangement step when
/// `EndRule::Continue` sustains past the score end.
#[inline]
pub(super) fn automation_position(project: &ScoreWindow, abs_pos: f64) -> f64 {
    if project.live_window.is_none() && project.end_rule == EndRule::Continue {
        let max_step = project.arrangement_steps().saturating_sub(1) as f64;
        abs_pos.min(max_step)
    } else {
        abs_pos
    }
}

#[derive(Clone, Copy)]
struct RackBusState {
    gain_db: f32,
    pan: f32,
    tune: f32,
    cutoff: f32,
    resonance: f32,
}

impl RackBusState {
    #[inline]
    fn from_rack(rack: &RackPatch) -> Self {
        Self {
            gain_db: rack.gain_db,
            pan: rack.pan,
            tune: rack.tune_semitones,
            cutoff: rack.cutoff_hz,
            resonance: rack.resonance,
        }
    }

    #[inline]
    fn apply_norm(&mut self, param: RackParam, norm: f32) {
        let val = param.denormalize(norm);
        match param {
            RackParam::Gain => self.gain_db = val,
            RackParam::Pan => self.pan = val,
            RackParam::Tune => self.tune = val,
            RackParam::Cutoff => self.cutoff = val,
            RackParam::Resonance => self.resonance = val,
        }
    }
}

#[derive(Clone, Copy)]
struct ZoneTriggerState {
    gain_db: f32,
    pan: f32,
    root_midi: u8,
    voice: VoiceSettings,
}

impl ZoneTriggerState {
    #[inline]
    fn from_zone(zone: &RackZone) -> Self {
        Self {
            gain_db: zone.gain_db,
            pan: zone.pan,
            root_midi: zone.root_midi,
            voice: zone.voice,
        }
    }

    #[inline]
    fn apply_norm(&mut self, param: ZoneParam, norm: f32) {
        let val = param.denormalize(norm);
        match param {
            ZoneParam::Gain => self.gain_db = val,
            ZoneParam::Pan => self.pan = val,
            ZoneParam::VoiceMode => {
                self.voice.voice_mode = VoiceMode::from_index(val.round().max(0.0) as usize);
            }
            ZoneParam::Legato => self.voice.legato = val >= 0.5,
            ZoneParam::Glide => self.voice.glide_ms = val,
            ZoneParam::PitchTracking => {
                self.voice.pitch_tracking =
                    PitchTracking::from_index(val.round().max(0.0) as usize);
            }
            ZoneParam::PhaseReset => self.voice.phase_reset = val >= 0.5,
            ZoneParam::RootMidi => self.root_midi = val.round().clamp(0.0, 127.0) as u8,
        }
    }
}

/// Fixed-capacity real-time voice pool with O(active) rendering and zero-alloc
/// voice allocation, choking, mono/legato retargeting, and steal de-clicking.
pub(super) struct VoicePool {
    voices: Box<[Voice]>,
    active_voices: [u16; VOICES],
    active_voice_count: usize,
    voice_cursor: usize,
    steal_fades: [StealFade; STEAL_FADE_SLOTS],
    steal_fade_cursor: usize,
    held_notes: Box<[HeldNotes]>,
    mono_last_pitch: Box<[MonoPitchSlot]>,
    mono_pitch_cursor: usize,
}

impl VoicePool {
    pub(super) fn new(sample_rate: f64) -> Self {
        let held_notes = vec![HeldNotes::default(); MAX_INSTRUMENTS].into_boxed_slice();
        let mono_last_pitch = vec![MonoPitchSlot::default(); MONO_PITCH_SLOTS].into_boxed_slice();
        let voices = (0..VOICES)
            .map(|_| Voice::new_inactive(sample_rate))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            voices,
            active_voices: [0; VOICES],
            active_voice_count: 0,
            voice_cursor: 0,
            steal_fades: [StealFade::default(); STEAL_FADE_SLOTS],
            steal_fade_cursor: 0,
            held_notes,
            mono_last_pitch,
            mono_pitch_cursor: 0,
        }
    }

    #[cfg(test)]
    #[inline]
    pub(super) fn voices(&self) -> &[Voice] {
        &self.voices
    }

    pub(super) fn clear_all(&mut self) {
        for voice in self.voices.iter_mut() {
            voice.deactivate();
        }
        self.active_voice_count = 0;
        self.steal_fades = [StealFade::default(); STEAL_FADE_SLOTS];
        self.mono_last_pitch.fill(MonoPitchSlot::default());
        self.mono_pitch_cursor = 0;
    }

    pub(super) fn clear_held_notes(&mut self) {
        self.held_notes.fill(HeldNotes::default());
    }

    pub(super) fn clear_preview_voices(&mut self) {
        for &slot_u16 in &self.active_voices[..self.active_voice_count] {
            let voice = &mut self.voices[slot_u16 as usize];
            if voice.track == PREVIEW_TRACK {
                voice.deactivate();
            }
        }
        for fade in &mut self.steal_fades {
            if fade.track == PREVIEW_TRACK {
                *fade = StealFade::default();
            }
        }
    }

    pub(super) fn release_all_live(&mut self) {
        for &slot_u16 in &self.active_voices[..self.active_voice_count] {
            let voice = &mut self.voices[slot_u16 as usize];
            if voice.live_key.is_some() {
                voice.force_release();
            }
        }
        self.held_notes.fill(HeldNotes::default());
    }

    /// Re-index active voices and prune held-note stacks after a live score update.
    pub(super) fn remap_tracks(&mut self, project: &ScoreWindow) {
        for &slot_u16 in &self.active_voices[..self.active_voice_count] {
            let voice = &mut self.voices[slot_u16 as usize];
            if !voice.active || voice.track == PREVIEW_TRACK {
                continue;
            }
            if let Some(index) = project
                .instruments
                .iter()
                .position(|t| t.id == voice.track_id)
            {
                voice.track = index as u8;
            } else {
                voice.deactivate();
            }
        }
        for held in self.held_notes.iter_mut() {
            if !project
                .instruments
                .iter()
                .any(|t| Some(t.id) == held.track_id)
            {
                *held = HeldNotes::default();
            }
        }
        for slot in self.mono_last_pitch.iter_mut() {
            if slot.occupied && !project.instruments.iter().any(|t| t.id == slot.track_id) {
                *slot = MonoPitchSlot::default();
            }
        }
        for fade in &mut self.steal_fades {
            if fade.remaining == 0 || fade.track == PREVIEW_TRACK {
                continue;
            }
            if let Some(index) = project
                .instruments
                .iter()
                .position(|t| t.id == fade.track_id)
            {
                fade.track = index as u8;
            } else {
                *fade = StealFade::default();
            }
        }
    }

    #[inline]
    fn last_mono_pitch(&self, track_id: u64, zone_id: u16) -> Option<f64> {
        self.mono_last_pitch
            .iter()
            .find(|s| s.occupied && s.track_id == track_id && s.zone_id == zone_id)
            .map(|s| s.pitch)
    }

    #[inline]
    fn record_mono_pitch(&mut self, track_id: u64, zone_id: u16, pitch: f64) {
        if !pitch.is_finite() {
            return;
        }
        if let Some(slot) = self
            .mono_last_pitch
            .iter_mut()
            .find(|s| s.occupied && s.track_id == track_id && s.zone_id == zone_id)
        {
            slot.pitch = pitch;
            return;
        }
        if let Some(slot) = self.mono_last_pitch.iter_mut().find(|s| !s.occupied) {
            *slot = MonoPitchSlot {
                occupied: true,
                track_id,
                zone_id,
                pitch,
            };
            return;
        }
        let idx = self.mono_pitch_cursor % MONO_PITCH_SLOTS;
        self.mono_pitch_cursor = self.mono_pitch_cursor.wrapping_add(1);
        self.mono_last_pitch[idx] = MonoPitchSlot {
            occupied: true,
            track_id,
            zone_id,
            pitch,
        };
    }

    #[inline]
    fn track_active_slot(&mut self, slot: usize) {
        let tag = slot as u16;
        if !self.active_voices[..self.active_voice_count].contains(&tag)
            && self.active_voice_count < VOICES
        {
            self.active_voices[self.active_voice_count] = tag;
            self.active_voice_count += 1;
        }
    }

    fn record_steal_fade_at(&mut self, slot: usize, sample_rate: f64) {
        let (active, sample, track, track_id) = {
            let voice = &self.voices[slot];
            (voice.active, voice.last_output, voice.track, voice.track_id)
        };
        if !active {
            return;
        }
        if sample[0].abs() < 1e-6 && sample[1].abs() < 1e-6 {
            return;
        }
        let fade_frames = steal_fade_frames(sample_rate);
        let idx = self.steal_fade_cursor % STEAL_FADE_SLOTS;
        self.steal_fade_cursor = (idx + 1) % STEAL_FADE_SLOTS;
        self.steal_fades[idx] = StealFade {
            track,
            track_id,
            sample,
            remaining: fade_frames,
            total: fade_frames,
        };
    }

    /// Priority voice allocator across all `VOICES` slots:
    /// 1) first inactive slot,
    /// 2) quietest releasing non-preview voice,
    /// 3) oldest non-preview voice,
    /// 4) oldest voice overall — with micro-fade de-click on active steals.
    fn allocate_voice_slot(&mut self, sample_rate: f64) -> usize {
        if let Some(slot) = (0..VOICES)
            .map(|offset| (self.voice_cursor + offset) % VOICES)
            .find(|&index| !self.voices[index].active)
        {
            self.voice_cursor = (slot + 1) % VOICES;
            return slot;
        }

        let mut best_rel: Option<(usize, f32, u32)> = None;
        let mut best_old: Option<(usize, u32)> = None;
        let mut best_any: (usize, u32) = (0, 0);

        for (index, voice) in self.voices.iter().enumerate() {
            if voice.age >= best_any.1 {
                best_any = (index, voice.age);
            }
            if voice.track == PREVIEW_TRACK {
                continue;
            }
            if best_old.is_none_or(|(_, age)| voice.age >= age) {
                best_old = Some((index, voice.age));
            }
            if voice.is_releasing() {
                let lvl = voice.current_level();
                if best_rel.is_none_or(|(_, b_lvl, b_age)| {
                    lvl < b_lvl || (lvl == b_lvl && voice.age > b_age)
                }) {
                    best_rel = Some((index, lvl, voice.age));
                }
            }
        }

        let slot = best_rel
            .map(|(i, _, _)| i)
            .or_else(|| best_old.map(|(i, _)| i))
            .unwrap_or(best_any.0);
        self.record_steal_fade_at(slot, sample_rate);
        self.voice_cursor = (slot + 1) % VOICES;
        slot
    }

    fn held_slot(&mut self, track_id: u64) -> usize {
        let index = self
            .held_notes
            .iter()
            .position(|h| h.track_id == Some(track_id))
            .or_else(|| self.held_notes.iter().position(|h| h.track_id.is_none()))
            .unwrap_or(0);
        if self.held_notes[index].track_id != Some(track_id) {
            self.held_notes[index] = HeldNotes {
                track_id: Some(track_id),
                ..HeldNotes::default()
            };
        }
        index
    }

    /// Choke all active voices on `track_id` whose `choke_group` bit is set in `choke_mask`.
    fn choke_active(&mut self, track_id: u64, choke_mask: u16, sample_rate: f64) {
        for &slot_u16 in &self.active_voices[..self.active_voice_count] {
            let voice = &mut self.voices[slot_u16 as usize];
            if voice.active
                && voice.track_id == track_id
                && voice.choke_group > 0
                && voice.choke_group < 16
                && (choke_mask & (1u16 << voice.choke_group)) != 0
            {
                voice.choke(sample_rate);
            }
        }
    }

    /// Find an active (or releasing) monophonic voice for `(track_id, zone_id)`,
    /// releasing any surplus voices on the same zone, and report whether legato applies.
    fn find_mono_retarget(
        &mut self,
        track_id: u64,
        zone_id: u16,
        legato_enabled: bool,
    ) -> Option<(usize, bool)> {
        let mono_slot = self.active_voices[..self.active_voice_count]
            .iter()
            .copied()
            .map(usize::from)
            .find(|&idx| {
                let v = &self.voices[idx];
                v.active && v.track_id == track_id && v.zone_id == zone_id && !v.is_releasing()
            })
            .or_else(|| {
                self.active_voices[..self.active_voice_count]
                    .iter()
                    .copied()
                    .map(usize::from)
                    .find(|&idx| {
                        let v = &self.voices[idx];
                        v.active && v.track_id == track_id && v.zone_id == zone_id
                    })
            })?;

        for &idx_u16 in &self.active_voices[..self.active_voice_count] {
            let index = usize::from(idx_u16);
            let voice = &mut self.voices[index];
            if index != mono_slot
                && voice.active
                && voice.track_id == track_id
                && voice.zone_id == zone_id
            {
                voice.force_release();
            }
        }
        let v = &self.voices[mono_slot];
        let legato =
            legato_enabled && !v.is_releasing() && (v.live_key.is_some() || v.age < v.gate);
        Some((mono_slot, legato))
    }

    fn activate_preview_voice(&mut self, slot: usize, zone_id: u16, gate: u32) {
        let voice = &mut self.voices[slot];
        voice.active = true;
        voice.track = PREVIEW_TRACK;
        voice.track_id = 0;
        voice.zone_id = zone_id;
        voice.rack_locks = 0;
        voice.zone_locks = 0;
        voice.live_key = None;
        voice.choke_group = 0;
        voice.age = 0;
        voice.gate = gate;
        self.track_active_slot(slot);
    }

    /// Trigger a browser sample or preset audition request on preview voice slots.
    pub(super) fn trigger_audition(
        &mut self,
        ctx: &ZoneTriggerCtx<'_>,
        request: &AuditionRequest,
        seed_base: u32,
    ) {
        self.clear_preview_voices();

        let sr = ctx.sample_rate;
        let bpm = ctx.project.bpm;
        let clock_seconds = ctx.rendered_frames as f64 / sr;

        match &request.kind {
            AuditionKind::Stop => {}
            AuditionKind::Sample {
                sample,
                zone,
                graph,
            } => {
                let slot = self.allocate_voice_slot(sr);
                let root = sample.meta.root_midi.min(127);
                let start = VoiceStart {
                    midi: root,
                    velocity: 0.82,
                    gate_frames: None,
                    seed: seed_base,
                    clock_seconds,
                    glide_from_pitch: None,
                };
                let voice = &mut self.voices[slot];
                voice.modular.begin_trigger_from_zone(&ZoneTriggerSpec {
                    inst_id: 0,
                    zone,
                    graph,
                    sample_rate: sr,
                    bpm,
                    zone_pan: 0.0,
                    rack_tune: 0.0,
                    is_preview: true,
                });
                voice.modular.finalize_trigger(start, false, false);
                voice.snap_rack_bus(1.0, 0.0, 20_000.0, 0.0, sr);
                self.activate_preview_voice(slot, zone.id, u32::MAX);
            }
            AuditionKind::Rack {
                patch,
                compiled_zones,
                midi,
                velocity,
            } => {
                let mut matched = [0usize; MAX_MATCHED_ZONES];
                let mut count = 0usize;
                for (z_idx, zone) in patch.zones.iter().enumerate() {
                    if zone.matches(*midi, *velocity) && count < MAX_MATCHED_ZONES {
                        matched[count] = z_idx;
                        count += 1;
                    }
                }
                let fallback_root_midi = if count == 0 && !patch.zones.is_empty() {
                    matched[0] = 0;
                    count = 1;
                    true
                } else {
                    false
                };

                for (layer_idx, &z_idx) in matched[..count].iter().enumerate() {
                    let (Some(zone), Some(graph)) =
                        (patch.zones.get(z_idx), compiled_zones.get(z_idx))
                    else {
                        continue;
                    };
                    let max_attack = graph
                        .lifetime_envs
                        .iter()
                        .filter_map(|&s| {
                            let op = graph.ops.get(usize::from(s))?;
                            let m = zone.modules.get(usize::from(op.module_index))?;
                            match m.kind {
                                ModuleKind::Env(e) => Some(e.attack_ms),
                                _ => None,
                            }
                        })
                        .fold(4.0_f32, f32::max);
                    let gate_seconds = (f64::from(max_attack) * 0.001 + 0.3).clamp(0.5, 3.0);
                    let gate_frames =
                        (gate_seconds * sr).round().clamp(1.0, u32::MAX as f64) as u32;
                    let eff_midi = if fallback_root_midi {
                        zone.root_midi
                    } else {
                        *midi
                    };
                    let start = VoiceStart {
                        midi: eff_midi,
                        velocity: *velocity,
                        gate_frames: Some(gate_frames),
                        seed: mix_seed(seed_base, layer_idx as u32),
                        clock_seconds,
                        glide_from_pitch: None,
                    };
                    let slot = self.allocate_voice_slot(sr);
                    let voice = &mut self.voices[slot];
                    voice.modular.begin_trigger_from_zone(&ZoneTriggerSpec {
                        inst_id: 0,
                        zone,
                        graph,
                        sample_rate: sr,
                        bpm,
                        zone_pan: zone.pan,
                        rack_tune: patch.tune_semitones,
                        is_preview: false,
                    });
                    voice.modular.finalize_trigger(start, false, false);
                    voice.snap_rack_bus(
                        db_to_lin(patch.gain_db + zone.gain_db),
                        patch.pan,
                        patch.cutoff_hz,
                        patch.resonance,
                        sr,
                    );
                    self.activate_preview_voice(slot, zone.id, gate_frames);
                }
            }
        }
    }

    /// Handle live Jam Key NoteOn: push onto held stack, release duplicate poly
    /// voices for the same key, and trigger all matching zones.
    pub(super) fn handle_live_note_on(
        &mut self,
        ctx: &ZoneTriggerCtx<'_>,
        note: LiveNote,
        seed: u32,
    ) {
        let held = self.held_slot(note.track_id);
        self.held_notes[held].press(note.midi, note.velocity);
        for &slot_u16 in &self.active_voices[..self.active_voice_count] {
            let voice = &mut self.voices[slot_u16 as usize];
            if voice.active && voice.track_id == note.track_id && voice.live_key == Some(note.midi)
            {
                let is_mono = ctx.project.instruments[note.track_idx]
                    .rack
                    .zone(voice.zone_id)
                    .is_some_and(|z| z.voice.voice_mode == VoiceMode::Mono);
                if !is_mono {
                    voice.begin_release();
                    voice.live_key = None;
                }
            }
        }
        self.trigger_on_track(
            ctx,
            &TriggerRequest {
                inst_index: note.track_idx,
                live_key: Some(note.midi),
                midi: note.midi,
                velocity: note.velocity * 0.90,
                gate_seconds: 25.0,
                seed,
                mod_pos: None,
                locks: &[],
            },
        );
    }

    /// Handle live Jam Key NoteOff: pop key from held stack, retarget mono zones
    /// back to any remaining held key, and release all other matching voices.
    pub(super) fn handle_live_note_off(
        &mut self,
        ctx: &ZoneTriggerCtx<'_>,
        note: LiveNote,
        audition_serial: &mut u32,
    ) {
        let held = self.held_slot(note.track_id);
        let _ = self.held_notes[held].remove(note.midi);
        let mut mono_retargets: [(usize, u8, f32); MAX_MATCHED_ZONES] =
            [(0, 0, 0.0); MAX_MATCHED_ZONES];
        let mut retarget_count = 0usize;

        for &slot_u16 in &self.active_voices[..self.active_voice_count] {
            let voice = &mut self.voices[slot_u16 as usize];
            if !(voice.active
                && voice.track_id == note.track_id
                && voice.live_key == Some(note.midi))
            {
                continue;
            }
            let zone_info = ctx.project.instruments[note.track_idx]
                .rack
                .zones
                .iter()
                .enumerate()
                .find(|(_, z)| z.id == voice.zone_id);
            if let Some((z_idx, zone)) = zone_info {
                if zone.voice.voice_mode == VoiceMode::Mono {
                    let fallback = self.held_notes[held].last_matching(|k, v| zone.matches(k, v));
                    if let Some((hk, hv)) = fallback {
                        if retarget_count < MAX_MATCHED_ZONES {
                            mono_retargets[retarget_count] = (z_idx, hk, hv);
                            retarget_count += 1;
                        }
                        continue;
                    }
                }
            }
            voice.begin_release();
            voice.live_key = None;
        }

        for &(z_idx, hk, hv) in &mono_retargets[..retarget_count] {
            *audition_serial = audition_serial.wrapping_add(1);
            self.trigger_single_zone(
                ctx,
                z_idx,
                &TriggerRequest {
                    inst_index: note.track_idx,
                    live_key: Some(hk),
                    midi: hk,
                    velocity: hv * 0.90,
                    gate_seconds: 25.0,
                    seed: *audition_serial,
                    mod_pos: None,
                    locks: &[],
                },
            );
        }
    }

    /// Trigger a compiled score event and all of its chord tones.
    pub(super) fn trigger_score_event(
        &mut self,
        ctx: &ZoneTriggerCtx<'_>,
        event: &ScoreEvent,
        locks: &[ParamLock],
        seed_base: u32,
        mod_pos: f64,
    ) {
        let inst_index = usize::from(event.inst_idx);
        let step_sec = ctx.project.step_seconds();
        self.trigger_on_track(
            ctx,
            &TriggerRequest {
                inst_index,
                live_key: None,
                midi: event.midi,
                velocity: event.velocity,
                gate_seconds: step_sec * f64::from(event.gate),
                seed: seed_base,
                mod_pos: Some(mod_pos),
                locks,
            },
        );
        for (idx, tone) in event.chord_tones().iter().enumerate() {
            let chord_seed = seed_base.wrapping_add(
                ((idx as u32) + 1).wrapping_mul(cadence_core::dsp::SECONDARY_WEYL_SALT_32),
            );
            self.trigger_on_track(
                ctx,
                &TriggerRequest {
                    inst_index,
                    live_key: None,
                    midi: tone.midi,
                    velocity: tone.velocity,
                    gate_seconds: step_sec * f64::from(tone.gate),
                    seed: chord_seed,
                    mod_pos: Some(mod_pos),
                    locks,
                },
            );
        }
    }

    /// Unified zero-allocation Rack trigger dispatching across all matching zones,
    /// applying choke groups first so simultaneous layered zones never choke each
    /// other, then spawning or mono-retargeting each layer.
    pub(super) fn trigger_on_track(&mut self, ctx: &ZoneTriggerCtx<'_>, req: &TriggerRequest<'_>) {
        let Some(inst) = ctx.project.instruments.get(req.inst_index) else {
            return;
        };
        let track_id = inst.id;
        let raw_vel = sanitize_sample(req.velocity).clamp(0.0, 1.0);

        let mut matched = [0usize; MAX_MATCHED_ZONES];
        let mut match_count = 0usize;
        let mut choke_mask: u16 = 0;
        for (z_idx, zone) in inst.rack.zones.iter().enumerate() {
            if zone.matches(req.midi, raw_vel) && match_count < MAX_MATCHED_ZONES {
                matched[match_count] = z_idx;
                match_count += 1;
                if let Some(g) = zone.choke_group {
                    if g > 0 && g < 16 {
                        choke_mask |= 1u16 << g;
                    }
                }
            }
        }
        if match_count == 0 {
            return;
        }

        if choke_mask != 0 {
            self.choke_active(track_id, choke_mask, ctx.sample_rate);
        }

        for (layer_idx, &zone_idx) in matched[..match_count].iter().enumerate() {
            let layer_req = TriggerRequest {
                velocity: raw_vel,
                seed: mix_seed(req.seed, layer_idx as u32),
                ..*req
            };
            self.trigger_single_zone(ctx, zone_idx, &layer_req);
        }
    }

    /// Trigger or monophonically retarget one resolved Rack zone.
    pub(super) fn trigger_single_zone(
        &mut self,
        ctx: &ZoneTriggerCtx<'_>,
        zone_idx: usize,
        req: &TriggerRequest<'_>,
    ) {
        let Some(inst) = ctx.project.instruments.get(req.inst_index) else {
            return;
        };
        let (Some(zone), Some(graph)) = (
            inst.rack.zones.get(zone_idx),
            inst.compiled_zones.get(zone_idx),
        ) else {
            return;
        };
        let track_id = inst.id;
        let zone_id = zone.id;
        let zone_choke = zone.choke_group.unwrap_or(0);
        let raw_vel = sanitize_sample(req.velocity).clamp(0.0, 1.0);
        let auto_pos = req.mod_pos.map(|p| automation_position(ctx.project, p));

        let (rack_bus, rack_locks) =
            resolve_trigger_rack_bus(ctx.project, req.inst_index, auto_pos, req.locks);
        let base_zone_root = zone.root_midi;
        let (zone_state, zone_locks) =
            resolve_trigger_zone_state(ctx.project, req.inst_index, zone, auto_pos, req.locks);

        let sr = ctx.sample_rate;
        let bpm = ctx.project.bpm;
        let clock_seconds = ctx.rendered_frames as f64 / sr;
        let gate_frames = req.live_key.is_none().then(|| {
            (req.gate_seconds.max(0.001) * sr)
                .round()
                .clamp(1.0, u32::MAX as f64) as u32
        });
        let is_mono_zone = zone_state.voice.voice_mode == VoiceMode::Mono;
        let glide_from_pitch = (is_mono_zone && zone_state.voice.glide_ms > 0.0)
            .then(|| self.last_mono_pitch(track_id, zone_id))
            .flatten();
        let start = VoiceStart {
            midi: req.midi,
            velocity: raw_vel,
            gate_frames,
            seed: req.seed,
            clock_seconds,
            glide_from_pitch,
        };

        let mono = is_mono_zone
            .then(|| self.find_mono_retarget(track_id, zone_id, zone_state.voice.legato))
            .flatten();

        let (slot, is_retarget, legato) = match mono {
            Some((slot, leg)) => (slot, true, leg),
            None => (self.allocate_voice_slot(sr), false, false),
        };

        let idx_u8 = req.inst_index as u8;
        let voice = &mut self.voices[slot];
        let same_topology = voice.modular.begin_trigger_from_zone(&ZoneTriggerSpec {
            inst_id: track_id,
            zone,
            graph,
            sample_rate: sr,
            bpm,
            zone_pan: zone_state.pan,
            rack_tune: rack_bus.tune,
            is_preview: false,
        });
        voice.modular.set_trigger_zone_overrides(
            zone_state.voice,
            zone_state.root_midi,
            zone_state.pan,
            rack_bus.tune,
        );

        // Propagate modulated/locked ZoneParam::RootMidi onto zone Samplers unless overridden.
        if zone_state.root_midi != base_zone_root {
            let root_norm = ModuleParam::RootMidi.normalize(f32::from(zone_state.root_midi));
            for m in &zone.modules {
                if matches!(m.kind, ModuleKind::Sampler(_)) {
                    voice.modular.apply_trigger_module_param(
                        m.id,
                        ModuleParam::RootMidi,
                        root_norm,
                        0,
                        false,
                    );
                }
            }
        }

        let mut apply_target = |target: InstrumentParam, value: f32, is_lock: bool| match target {
            InstrumentParam::Module {
                zone: z,
                module,
                param,
            } if z == zone_id => {
                let frames = if param.is_frame_based() {
                    ctx.sample_frames(module_sample_key(track_id, zone_id, module))
                } else {
                    0
                };
                voice
                    .modular
                    .apply_trigger_module_param(module, param, value, frames, is_lock);
            }
            InstrumentParam::Cable { zone: z, cable } if z == zone_id => {
                voice
                    .modular
                    .apply_trigger_cable_norm(cable, value, is_lock);
            }
            _ => {}
        };

        if let Some(ap) = auto_pos {
            for lane in &ctx.project.mod_lanes {
                if lane.inst_idx == idx_u8 {
                    apply_target(lane.target, lane.evaluate(ap), false);
                }
            }
        }

        for lock in req.locks {
            apply_target(lock.target, lock.value, true);
        }

        voice
            .modular
            .finalize_trigger(start, is_retarget && same_topology, legato);
        let mono_target_pitch = is_mono_zone.then(|| voice.modular.target_note_pitch());
        voice.active = true;
        voice.track = idx_u8;
        voice.track_id = track_id;
        voice.zone_id = zone_id;
        voice.rack_locks = rack_locks;
        voice.zone_locks = zone_locks;
        voice.rack_gain_db = rack_bus.gain_db;
        voice.rack_pan = rack_bus.pan;
        voice.rack_tune = rack_bus.tune;
        voice.rack_cutoff_hz = rack_bus.cutoff;
        voice.rack_resonance = rack_bus.resonance;
        voice.zone_gain_db = zone_state.gain_db;
        voice.zone_pan = zone_state.pan;
        voice.live_key = req.live_key;
        voice.choke_group = zone_choke;
        voice.age = 0;
        voice.gate = gate_frames.unwrap_or(u32::MAX);

        let gain_lin = db_to_lin(rack_bus.gain_db + zone_state.gain_db);
        if is_retarget && legato {
            voice.set_rack_bus_targets(gain_lin, rack_bus.pan, rack_bus.cutoff, rack_bus.resonance);
        } else {
            voice.snap_rack_bus(
                gain_lin,
                rack_bus.pan,
                rack_bus.cutoff,
                rack_bus.resonance,
                sr,
            );
        }
        if let Some(pitch) = mono_target_pitch {
            self.record_mono_pitch(track_id, zone_id, pitch);
        }
        self.track_active_slot(slot);
    }

    /// Apply continuous (non-Step) Rack, Zone, Module, and Cable modulation lanes
    /// to every active voice while respecting trigger-time parameter lock bitmasks.
    pub(super) fn update_continuous_controls(&mut self, project: &ScoreWindow, auto_pos: f64) {
        let rack_gain_bit = 1u8 << RackParam::Gain.id();
        let rack_pan_bit = 1u8 << RackParam::Pan.id();
        let rack_tune_bit = 1u8 << RackParam::Tune.id();
        let rack_cut_bit = 1u8 << RackParam::Cutoff.id();
        let rack_res_bit = 1u8 << RackParam::Resonance.id();
        let zone_gain_bit = 1u8 << (ZoneParam::Gain as u8);
        let zone_pan_bit = 1u8 << (ZoneParam::Pan as u8);

        for (inst_idx, inst) in project.instruments.iter().take(MAX_INSTRUMENTS).enumerate() {
            let idx_u8 = inst_idx as u8;
            let rack = &inst.rack;
            let mut rack_bus = RackBusState::from_rack(rack);

            for lane in &project.mod_lanes {
                if lane.inst_idx != idx_u8 {
                    continue;
                }
                if let InstrumentParam::Rack { param } = lane.target {
                    rack_bus.apply_norm(param, lane.evaluate(auto_pos));
                }
            }

            for &slot_u16 in &self.active_voices[..self.active_voice_count] {
                let voice = &mut self.voices[slot_u16 as usize];
                if !voice.active || voice.track != idx_u8 {
                    continue;
                }

                let eff_rack_gain = if voice.rack_locks & rack_gain_bit != 0 {
                    voice.rack_gain_db
                } else {
                    rack_bus.gain_db
                };
                let eff_rack_pan = if voice.rack_locks & rack_pan_bit != 0 {
                    voice.rack_pan
                } else {
                    rack_bus.pan
                };
                let eff_rack_tune = if voice.rack_locks & rack_tune_bit != 0 {
                    voice.rack_tune
                } else {
                    rack_bus.tune
                };
                let eff_rack_cutoff = if voice.rack_locks & rack_cut_bit != 0 {
                    voice.rack_cutoff_hz
                } else {
                    rack_bus.cutoff
                };
                let eff_rack_res = if voice.rack_locks & rack_res_bit != 0 {
                    voice.rack_resonance
                } else {
                    rack_bus.resonance
                };

                let base_zone = rack.zone(voice.zone_id);
                let mut z_gain_db = if voice.zone_locks & zone_gain_bit != 0 {
                    voice.zone_gain_db
                } else {
                    base_zone.map_or(voice.zone_gain_db, |z| z.gain_db)
                };
                let mut z_pan = if voice.zone_locks & zone_pan_bit != 0 {
                    voice.zone_pan
                } else {
                    base_zone.map_or(voice.zone_pan, |z| z.pan)
                };

                for lane in &project.mod_lanes {
                    if lane.inst_idx != idx_u8 {
                        continue;
                    }
                    if let InstrumentParam::Zone { zone, param } = lane.target {
                        if zone != voice.zone_id {
                            continue;
                        }
                        match param {
                            ZoneParam::Gain if voice.zone_locks & zone_gain_bit == 0 => {
                                z_gain_db = param.denormalize(lane.evaluate(auto_pos));
                            }
                            ZoneParam::Pan if voice.zone_locks & zone_pan_bit == 0 => {
                                z_pan = param.denormalize(lane.evaluate(auto_pos));
                            }
                            _ => {}
                        }
                    }
                }

                voice.set_rack_bus_targets(
                    db_to_lin(eff_rack_gain + z_gain_db),
                    eff_rack_pan,
                    eff_rack_cutoff,
                    eff_rack_res,
                );

                if let Some(zone) = base_zone {
                    voice.modular.update_continuous_from_zone(
                        zone,
                        z_pan,
                        eff_rack_tune,
                        project.bpm,
                    );
                }

                for lane in &project.mod_lanes {
                    if lane.inst_idx != idx_u8 {
                        continue;
                    }
                    match lane.target {
                        InstrumentParam::Module {
                            zone,
                            module,
                            param,
                        } if zone == voice.zone_id => {
                            voice.modular.apply_continuous_module_param(
                                module,
                                param,
                                lane.evaluate(auto_pos),
                            );
                        }
                        InstrumentParam::Cable { zone, cable } if zone == voice.zone_id => {
                            voice
                                .modular
                                .apply_continuous_cable_norm(cable, lane.evaluate(auto_pos));
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// Render one stereo frame across all active voices (compacting finished
    /// slots in O(1)) and sum any active voice-steal micro-fade tails.
    pub(super) fn render_frame(
        &mut self,
        samples: &SampleBank,
        preview_ref: Option<&Arc<SampleData>>,
    ) -> VoiceFrameOut {
        let mut out = VoiceFrameOut {
            sources: [[0.0_f32; 2]; MAX_INSTRUMENTS],
            preview: [0.0_f32; 2],
            track_scope: [0.0_f32; MAX_INSTRUMENTS],
        };

        let mut active_idx = 0usize;
        while active_idx < self.active_voice_count {
            let slot = self.active_voices[active_idx] as usize;
            let voice = &mut self.voices[slot];
            if !voice.active {
                self.active_voice_count -= 1;
                self.active_voices[active_idx] = self.active_voices[self.active_voice_count];
                continue;
            }
            let [left, right] = voice.next_stereo(samples, preview_ref);
            let track = voice.track as usize;
            if track < MAX_INSTRUMENTS {
                out.track_scope[track] += stereo_mid_f32([left, right]);
                out.sources[track][0] += left;
                out.sources[track][1] += right;
            } else {
                out.preview[0] += left;
                out.preview[1] += right;
            }
            let retired_mono = if !voice.active {
                (voice.track != PREVIEW_TRACK && voice.modular.is_mono()).then(|| {
                    (
                        voice.track_id,
                        voice.zone_id,
                        voice.modular.current_note_pitch(),
                    )
                })
            } else {
                None
            };
            if !voice.active {
                if let Some((tid, zid, pitch)) = retired_mono {
                    self.record_mono_pitch(tid, zid, pitch);
                }
                self.active_voice_count -= 1;
                self.active_voices[active_idx] = self.active_voices[self.active_voice_count];
            } else {
                active_idx += 1;
            }
        }

        // Apply short micro-fade de-click tails for any recently stolen active voices.
        for fade in &mut self.steal_fades {
            if fade.remaining == 0 {
                continue;
            }
            let gain = f32::from(fade.remaining) / f32::from(fade.total.max(1));
            let l = fade.sample[0] * gain;
            let r = fade.sample[1] * gain;
            let track = fade.track as usize;
            if track < MAX_INSTRUMENTS {
                out.track_scope[track] += stereo_mid_f32([l, r]);
                out.sources[track][0] += l;
                out.sources[track][1] += r;
            } else {
                out.preview[0] += l;
                out.preview[1] += r;
            }
            fade.remaining -= 1;
        }

        out
    }
}

/// Resolve trigger-time Rack-bus values and P-lock bitmask (zero heap allocations).
fn resolve_trigger_rack_bus(
    project: &ScoreWindow,
    inst_index: usize,
    auto_pos: Option<f64>,
    locks: &[ParamLock],
) -> (RackBusState, u8) {
    let mut state = RackBusState::from_rack(&project.instruments[inst_index].rack);
    let mut rack_locks: u8 = 0;

    if let Some(ap) = auto_pos {
        let idx_u8 = inst_index as u8;
        for lane in &project.mod_lanes {
            if lane.inst_idx == idx_u8 {
                if let InstrumentParam::Rack { param } = lane.target {
                    state.apply_norm(param, lane.evaluate(ap));
                }
            }
        }
    }

    for lock in locks {
        if let InstrumentParam::Rack { param } = lock.target {
            rack_locks |= 1u8 << param.id();
            state.apply_norm(param, lock.value);
        }
    }

    (state, rack_locks)
}

/// Resolve trigger-time Zone parameters and P-lock bitmask (zero heap allocations).
fn resolve_trigger_zone_state(
    project: &ScoreWindow,
    inst_index: usize,
    zone: &RackZone,
    auto_pos: Option<f64>,
    locks: &[ParamLock],
) -> (ZoneTriggerState, u8) {
    let mut state = ZoneTriggerState::from_zone(zone);
    let mut zone_locks: u8 = 0;

    if let Some(ap) = auto_pos {
        let idx_u8 = inst_index as u8;
        for lane in &project.mod_lanes {
            if lane.inst_idx == idx_u8 {
                if let InstrumentParam::Zone { zone: z, param } = lane.target {
                    if z == zone.id {
                        state.apply_norm(param, lane.evaluate(ap));
                    }
                }
            }
        }
    }

    for lock in locks {
        if let InstrumentParam::Zone { zone: z, param } = lock.target {
            if z == zone.id {
                zone_locks |= 1u8 << (param as u8);
                state.apply_norm(param, lock.value);
            }
        }
    }

    (state, zone_locks)
}
