//! Pure MIDI controller routing, 1-key MIDI Learn, device profiles, Focused Smart Macros,
//! soft-pickup takeover, live MIDI note recording (step & quantized), and semantic
//! bi-directional hardware feedback (RGB pad LEDs, OLED display, transport LEDs, CC echo).
mod deck_ui;
mod dispatch;
mod feedback;
mod learn;
mod record;
#[cfg(test)]
mod tests;

use crate::*;
use cadence_core::midi_map::{
    FeedbackProtocol, MidiFeedback, MidiMapping, MidiPadMode, MidiProfile, MidiSource,
};
use cadence_core::music::note_name;
use cadence_core::runtime::MidiMessage;
use std::collections::VecDeque;
use std::time::Instant;

pub(super) const MIDI_LOG_CAPACITY: usize = 12;
pub(super) const PICKUP_TOLERANCE: f32 = 0.04;
pub(super) const RELATIVE_STEP: f32 = 0.015;

/// Typed rows of the `DEVICE & PROFILE` section on the MIDI Devices tab (`gd`),
/// mirroring [`crate::AudioDeckRow`] and [`crate::SettingsCoreRow`].
#[repr(usize)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiDeckRow {
    In = 0,
    Out = 1,
    Profile = 2,
    Pads = 3,
    Pickup = 4,
}

impl MidiDeckRow {
    pub const COUNT: usize = 5;

    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn from_index(idx: usize) -> Option<Self> {
        match idx {
            0 => Some(Self::In),
            1 => Some(Self::Out),
            2 => Some(Self::Profile),
            3 => Some(Self::Pads),
            4 => Some(Self::Pickup),
            _ => None,
        }
    }
}

/// One recent incoming MIDI message displayed in the MIDI Deck activity monitor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MidiLogEntry {
    pub source: String,
    pub value: String,
    pub action: String,
}

/// Request from `cadence-app` to the host bridge to change connected hardware MIDI ports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MidiConnectRequest {
    /// `None` = unchanged, `Some(None)` = disconnect, `Some(Some(name))` = connect matching port.
    pub input: Option<Option<String>>,
    /// `None` = unchanged, `Some(None)` = disconnect, `Some(Some(name))` = connect matching port.
    pub output: Option<Option<String>>,
}

/// Transient state for live MIDI CC automation recording and gesture window tracking.
#[derive(Clone, Debug, Default)]
pub(crate) struct MidiCcRecState {
    /// Most recently recorded CC automation target label (shown in Chrome & MIDI Deck).
    pub last_target_label: Option<String>,
    /// Timestamp of the last coalesced MIDI CC / Macro gesture tick (1.5s window).
    pub last_gesture_tick: Option<Instant>,
}

/// Diff cache preventing redundant SysEx / LED packets on unchanged frames,
/// plus transient live MIDI CC recording state (keeps `App` layout stable).
#[derive(Clone, Debug, Default)]
pub(crate) struct MidiFeedbackCache {
    pub(super) pads: [Option<(u8, u8, u8)>; 8],
    pub(super) transport: [Option<bool>; 5],
    pub(crate) cc_rec: MidiCcRecState,
}

impl MidiFeedbackCache {
    pub fn reset(&mut self) {
        self.pads = [None; 8];
        self.transport = [None; 5];
    }
}

impl App {
    /// Whether the Devices Deck (`gd` / `Ctrl+d`) currently has focus.
    pub fn midi_focused(&self) -> bool {
        self.layout
            .focused()
            .is_some_and(|d| d.kind == layout::DeckKind::Midi)
    }

    /// Active device profile (custom loaded JSON overrides built-in ID).
    pub fn active_midi_profile(&self) -> MidiProfile {
        if let Some(custom) = &self.midi.custom_profile {
            return custom.clone();
        }
        MidiProfile::by_id(&self.config.midi_profile)
    }

    /// Active feedback wire protocol from the current device profile.
    pub fn active_midi_feedback_protocol(&self) -> FeedbackProtocol {
        self.active_midi_profile().feedback
    }

    /// Merged effective MIDI mappings: user-learned mappings take precedence over profile defaults.
    /// Returns `(mapping, is_user_mapping)`.
    pub fn effective_midi_mappings(&self) -> Vec<(MidiMapping, bool)> {
        let mut out: Vec<(MidiMapping, bool)> = self
            .config
            .midi_mappings
            .iter()
            .cloned()
            .map(|m| (m, true))
            .collect();
        let profile = self.active_midi_profile();
        for pm in profile.mappings {
            let shadowed = self
                .config
                .midi_mappings
                .iter()
                .any(|um| um.source == pm.source || um.target == pm.target);
            if !shadowed {
                out.push((pm, false));
            }
        }
        out
    }

    /// Update the discovered OS MIDI input/output port lists from the host bridge.
    pub fn set_midi_available_ports(&mut self, inputs: Vec<String>, outputs: Vec<String>) {
        self.midi.in_ports = inputs;
        self.midi.out_ports = outputs;
    }

    /// Clear soft-pickup latch states (e.g., when switching focused instrument/tab or project).
    pub fn reset_midi_pickup(&mut self) {
        self.midi.pickup_latched.clear();
    }

    pub(super) fn push_midi_log(&mut self, source: String, value: String, action: String) {
        self.midi.log.push_front(MidiLogEntry {
            source,
            value,
            action,
        });
        while self.midi.log.len() > MIDI_LOG_CAPACITY {
            self.midi.log.pop_back();
        }
    }

    /// Dispatch an incoming MIDI message through Learn, live recording, profile bindings, or note playback.
    pub fn handle_midi_event(&mut self, message: MidiMessage) -> Result<Outcome> {
        // 1. Update live status string & CC meter array.
        let (src_text, val_text) = match message {
            MidiMessage::NoteOn {
                channel,
                note,
                velocity,
            } => (
                format!("{} ch{}", note_name(note), channel + 1),
                format!("vel {velocity}"),
            ),
            MidiMessage::NoteOff { channel, note } => (
                format!("{} ch{}", note_name(note), channel + 1),
                "off".into(),
            ),
            MidiMessage::ProgramChange { channel, program } => {
                (format!("PC ch{}", channel + 1), format!("#{}", program + 1))
            }
            MidiMessage::Control {
                channel,
                controller,
                value,
            } => {
                if usize::from(controller) < self.midi.cc_live.len() {
                    self.midi.cc_live[usize::from(controller)] = Some(value);
                }
                (
                    format!("CC {controller:3} ch{}", channel + 1),
                    format!("{value:3}"),
                )
            }
            MidiMessage::PitchBend { channel, value } => {
                (format!("Bend ch{}", channel + 1), format!("{value:+5}"))
            }
            MidiMessage::Mmc { command } => (
                "MMC".into(),
                match command {
                    0x01 => "Stop".into(),
                    0x02 | 0x03 => "Play".into(),
                    0x06 => "Rec".into(),
                    0x09 => "Pause".into(),
                    other => format!("0x{other:02X}"),
                },
            ),
        };
        self.midi.last = Some(format!("{src_text} = {val_text}"));

        // 2. If MIDI Learn is armed, capture the next CC / NoteOn / PitchBend immediately!
        if let Some(target) = self.midi.learn {
            let captured = match message {
                MidiMessage::Control {
                    channel,
                    controller,
                    ..
                } => Some(MidiSource::Cc {
                    channel: Some(channel),
                    cc: controller,
                }),
                MidiMessage::NoteOn { channel, note, .. } => Some(MidiSource::Note {
                    channel: Some(channel),
                    note,
                }),
                MidiMessage::PitchBend { channel, .. } => Some(MidiSource::PitchBend {
                    channel: Some(channel),
                }),
                _ => None,
            };
            if let Some(source) = captured {
                self.midi.learn = None;
                return Ok(self.complete_midi_learn(target, source));
            }
        }

        // 3. Universal MMC transport SysEx (works with every hardware controller out of the box!).
        if let MidiMessage::Mmc { command } = message {
            let mut out = match command {
                0x01 => self.apply(Action::Stop)?,
                0x02 | 0x03 => self.apply(Action::Play { playing: true })?,
                0x06 => self.toggle_midi_record(),
                0x09 => self.apply(Action::Play {
                    playing: !self.transport.playing,
                })?,
                _ => Outcome::default(),
            };
            self.push_midi_log(src_text, val_text, self.status.clone());
            self.sync_midi_feedback(&mut out);
            return Ok(out);
        }

        // 4. Check explicit & profile MidiMappings first!
        let mappings = self.effective_midi_mappings();
        let matched: Vec<MidiMapping> = mappings
            .into_iter()
            .filter_map(|(m, _)| {
                let hit = match message {
                    MidiMessage::Control {
                        channel,
                        controller,
                        ..
                    } => m.source.matches_cc(channel, controller),
                    MidiMessage::NoteOn { channel, note, .. }
                    | MidiMessage::NoteOff { channel, note } => {
                        m.source.matches_note(channel, note)
                    }
                    MidiMessage::PitchBend { channel, .. } => m.source.matches_pitch_bend(channel),
                    _ => false,
                };
                hit.then_some(m)
            })
            .collect();

        if !matched.is_empty() {
            let mut total_out = Outcome::default();
            let mut action_desc = String::new();
            for mapping in matched {
                let res = self.dispatch_midi_mapping(&mapping, message)?;
                if !res.1.is_empty() {
                    action_desc = res.1;
                }
                merge_outcomes(&mut total_out, res.0);
            }
            self.push_midi_log(src_text, val_text, action_desc);
            self.sync_midi_feedback(&mut total_out);
            return Ok(total_out);
        }

        // 5. Check performance pad modes (Sections / Mutes) for the profile's 8 pads.
        let profile = self.active_midi_profile();
        if let MidiMessage::NoteOn { channel, note, .. } = message {
            let channel_ok = profile.pad_channel.is_none_or(|pc| pc == channel);
            if channel_ok {
                if let Some(pad_idx) = profile.pad_notes.iter().position(|&n| n == note) {
                    match self.config.midi_pad_mode {
                        MidiPadMode::Sections => {
                            let mut out = self.trigger_section_jump(pad_idx)?;
                            let sec_name = self
                                .doc
                                .project()
                                .sections
                                .get(pad_idx)
                                .map_or("Section", |s| s.name.as_str())
                                .to_string();
                            self.push_midi_log(
                                src_text,
                                val_text,
                                format!("Pad {} → {sec_name}", pad_idx + 1),
                            );
                            out.midi_feedback.push(MidiFeedback::Display {
                                line1: format!("Section {}", pad_idx + 1),
                                line2: sec_name,
                                value_bar: None,
                            });
                            self.sync_midi_feedback(&mut out);
                            return Ok(out);
                        }
                        MidiPadMode::Mutes => {
                            if let Some(lane) = self.doc.project().lanes.get(pad_idx) {
                                let lane_id = lane.id;
                                let lane_name = lane.name.clone();
                                let muted = !self.effective_lane_muted(lane_id);
                                let mut out = self.apply(Action::SetLaneMuted {
                                    lane: lane_id,
                                    muted,
                                })?;
                                let state = if muted { "MUTED" } else { "ACTIVE" };
                                self.push_midi_log(
                                    src_text,
                                    val_text,
                                    format!("{lane_name} {state}"),
                                );
                                out.midi_feedback.push(MidiFeedback::Display {
                                    line1: lane_name,
                                    line2: state.into(),
                                    value_bar: None,
                                });
                                self.sync_midi_feedback(&mut out);
                                return Ok(out);
                            }
                        }
                        MidiPadMode::Notes => {}
                    }
                }
            }
        }

        // 6. Standard live note playing + Program Change + Live MIDI Note Recording!
        let mut out = match message {
            MidiMessage::NoteOn { note, velocity, .. } => {
                let instrument = self
                    .jam_instrument()
                    .context("MIDI note: select a lane or instrument to play")?;
                let mut out = Outcome::default();
                if let Some(at) = self.input.midi_held.iter().position(|(n, _)| *n == note) {
                    let (_, previous) = self.input.midi_held.remove(at);
                    out.live_notes_off.push(InstrumentNoteOff {
                        instrument_id: previous,
                        midi: note,
                    });
                }
                self.input.midi_held.push((note, instrument));
                out.live_notes_on.push(InstrumentNoteOn {
                    instrument_id: instrument,
                    midi: note,
                    velocity: f32::from(velocity) / 127.0,
                    sustain: true,
                    part_id: None,
                });
                let mut action = format!("Play {}", note_name(note));
                if self.midi.record {
                    if let Ok((rec_out, rec_label)) = self.record_midi_note(note, velocity) {
                        merge_outcomes(&mut out, rec_out);
                        action = rec_label;
                    }
                }
                self.push_midi_log(src_text, val_text, action);
                out
            }
            MidiMessage::NoteOff { note, .. } => {
                let Some(at) = self.input.midi_held.iter().position(|(n, _)| *n == note) else {
                    return Ok(Outcome::default());
                };
                let (_, instrument) = self.input.midi_held.remove(at);
                Outcome {
                    live_notes_off: vec![InstrumentNoteOff {
                        instrument_id: instrument,
                        midi: note,
                    }],
                    ..Outcome::default()
                }
            }
            MidiMessage::ProgramChange { program, .. } => {
                let out = self.trigger_section_jump(usize::from(program))?;
                self.push_midi_log(src_text, val_text, format!("Section {}", program + 1));
                out
            }
            MidiMessage::Control { .. } | MidiMessage::PitchBend { .. } => {
                self.push_midi_log(src_text, val_text, "unmapped".into());
                Outcome::default()
            }
            MidiMessage::Mmc { .. } => Outcome::default(),
        };
        self.sync_midi_feedback(&mut out);
        Ok(out)
    }
}

fn merge_outcomes(dst: &mut Outcome, src: Outcome) {
    dst.merge(src);
}

/// Helper to create an empty `VecDeque` cleanly.
pub(crate) fn new_midi_log() -> VecDeque<MidiLogEntry> {
    VecDeque::with_capacity(MIDI_LOG_CAPACITY)
}
