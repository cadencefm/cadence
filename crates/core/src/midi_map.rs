//! Data-driven MIDI controller mapping, MIDI Learn targets, device profiles,
//! and semantic bi-directional hardware feedback (RGB pad LEDs, OLED display,
//! transport LEDs, and CC echo). Zero hardware/IO dependencies.
use crate::params::ParamAddress;
use crate::store::{read_bounded, write_atomic};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const MAX_MIDI_PROFILE_BYTES: u64 = 128 * 1024;

/// Incoming MIDI source selector for a `MidiMapping`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MidiSource {
    /// Control Change (`channel: None` matches any MIDI channel 0..15).
    Cc {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channel: Option<u8>,
        cc: u8,
    },
    /// Note On/Off trigger (e.g. controller pad).
    Note {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channel: Option<u8>,
        note: u8,
    },
    /// 14-bit Pitch Bend wheel/strip.
    PitchBend {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channel: Option<u8>,
    },
}

impl MidiSource {
    pub fn matches_cc(self, ch: u8, controller: u8) -> bool {
        matches!(self, Self::Cc { channel, cc } if cc == controller && channel.is_none_or(|c| c == ch))
    }

    pub fn matches_note(self, ch: u8, n: u8) -> bool {
        matches!(self, Self::Note { channel, note } if note == n && channel.is_none_or(|c| c == ch))
    }

    pub fn matches_pitch_bend(self, ch: u8) -> bool {
        matches!(self, Self::PitchBend { channel } if channel.is_none_or(|c| c == ch))
    }

    pub fn short_badge(self) -> String {
        match self {
            Self::Cc { cc, .. } => format!("CC {cc}"),
            Self::Note { note, .. } => format!("N{note}"),
            Self::PitchBend { .. } => "BEND".into(),
        }
    }

    pub fn display_label(self) -> String {
        let ch_suffix = |c: Option<u8>| match c {
            Some(ch) => format!(" ch{}", ch + 1),
            None => " omni".into(),
        };
        match self {
            Self::Cc { channel, cc } => format!("CC {cc:3}{}", ch_suffix(channel)),
            Self::Note { channel, note } => format!(
                "Note {} ({note}){}",
                crate::music::note_name(note),
                ch_suffix(channel)
            ),
            Self::PitchBend { channel } => format!("PitchBend{}", ch_suffix(channel)),
        }
    }
}

crate::define_choice_enum! {
    /// How raw incoming MIDI bytes (`0..=127`) translate into normalized changes or triggers.
    #[derive(Default)]
    pub enum MidiValueMode {
        /// Absolute `0..=127` mapped across `[min, max]` (supports soft pickup takeover).
        #[default]
        Absolute => ("absolute", "Abs", ["abs"]),
        /// Endless encoder twos-complement relative (`1..=63` = `+delta`, `65..=127` = `-(128 - v)`).
        RelativeSigned => ("relative_signed", "Rel±", ["rel", "relative"]),
        /// Endless encoder binary-offset relative (`64` = 0, `65..=127` = `+delta`, `1..=63` = `-delta`).
        RelativeOffset64 => ("relative_offset64", "Rel64", ["rel64"]),
        /// Fires once on press (`value > 0`).
        Trigger => ("trigger", "Trig", ["trig"]),
        /// Toggles state on press (`value > 0`).
        Toggle => ("toggle", "Tog", ["tog"]),
        /// Active while held (`value > 0`), inactive on release (`value == 0`).
        Momentary => ("momentary", "Mom", ["mom"]),
    }
}

impl MidiValueMode {
    /// Decode a relative encoder delta in ticks (`+1`, `-1`, etc.) for relative modes.
    pub fn decode_relative_delta(self, raw: u8) -> Option<i16> {
        match self {
            Self::RelativeSigned => {
                if raw == 0 {
                    None
                } else if raw < 64 {
                    Some(i16::from(raw))
                } else {
                    Some(i16::from(raw) - 128)
                }
            }
            Self::RelativeOffset64 => {
                let d = i16::from(raw) - 64;
                (d != 0).then_some(d)
            }
            _ => None,
        }
    }
}

/// Transport & navigation actions bindable to hardware buttons or encoders.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MidiTransportAction {
    PlayPause,
    Stop,
    Record,
    LoopToggle,
    TapTempo,
    PrevSection,
    NextSection,
    NavigateControl,
    ConfirmControl,
    KeepLive,
}

impl MidiTransportAction {
    pub const ALL: [Self; 10] = [
        Self::PlayPause,
        Self::Stop,
        Self::Record,
        Self::LoopToggle,
        Self::TapTempo,
        Self::PrevSection,
        Self::NextSection,
        Self::NavigateControl,
        Self::ConfirmControl,
        Self::KeepLive,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::PlayPause => "Play / Pause",
            Self::Stop => "Stop",
            Self::Record => "MIDI Record",
            Self::LoopToggle => "Loop Toggle",
            Self::TapTempo => "Tap Tempo",
            Self::PrevSection => "Prev Section",
            Self::NextSection => "Next Section",
            Self::NavigateControl => "Scroll Control",
            Self::ConfirmControl => "Confirm / Trigger",
            Self::KeepLive => "Keep Live Overrides",
        }
    }
}

/// Destination target inside Cadence controlled by a `MidiMapping`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MidiTarget {
    /// Fixed explicit parameter address (Rack, Zone, Module, Cable, Channel, Send, Effect).
    Param { address: ParamAddress },
    /// Smart auto-mapped macro slot (`0..=7`) tracking the currently focused instrument/effect/channel
    /// (or Project Macro 1–8 when configured and not focused in an object deck).
    FocusedMacro { slot: usize },
    /// Explicit assignable Project Macro slot (`0..=7`, M1..M8).
    ProjectMacro { slot: usize },
    /// Channel strip gain for lane index `0..=15` (following arrangement lane order).
    LaneGain { lane_index: usize },
    /// Transport / live-performance action.
    Transport { action: MidiTransportAction },
    /// Jump to arrangement section index (`0..=9`).
    LaunchSection { index: usize },
    /// Launch part slot `part_index` on lane `lane_index`.
    LaunchLanePart {
        lane_index: usize,
        part_index: usize,
    },
    /// Toggle mute on arrangement lane `lane_index`.
    MuteLane { lane_index: usize },
    /// Toggle solo on arrangement lane `lane_index`.
    SoloLane { lane_index: usize },
}

impl MidiTarget {
    pub fn default_label(self) -> String {
        match self {
            Self::Param { address } => match address {
                ParamAddress::Rack { param, .. } => format!("Rack · {}", param.label()),
                ParamAddress::Zone { zone, param, .. } => {
                    format!("Zone z{zone} · {}", param.label())
                }
                ParamAddress::Module {
                    zone,
                    module,
                    param,
                    ..
                } => {
                    format!("z{zone}.m{module} · {}", param.label())
                }
                ParamAddress::Cable { zone, cable, .. } => {
                    format!("z{zone}.c{cable} · Cable Amt")
                }
                ParamAddress::Channel { channel, param } => {
                    format!("{channel} · {}", param.label())
                }
                ParamAddress::Send { channel, target } => format!("{channel} → ch:{target}"),
                ParamAddress::Effect { param, .. } => format!("FX · {}", param.label()),
            },
            Self::FocusedMacro { slot } => format!("Focused Macro {}", slot + 1),
            Self::ProjectMacro { slot } => format!("Project Macro {}", slot + 1),
            Self::LaneGain { lane_index } => format!("Lane {} Gain", lane_index + 1),
            Self::Transport { action } => format!("Transport · {}", action.label()),
            Self::LaunchSection { index } => format!("Section {}", index + 1),
            Self::LaunchLanePart {
                lane_index,
                part_index,
            } => format!("Lane {} · Part {}", lane_index + 1, part_index + 1),
            Self::MuteLane { lane_index } => format!("Mute Lane {}", lane_index + 1),
            Self::SoloLane { lane_index } => format!("Solo Lane {}", lane_index + 1),
        }
    }
}

fn default_one() -> f32 {
    1.0
}

fn default_true() -> bool {
    true
}

/// One persisted MIDI controller binding.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MidiMapping {
    pub id: u64,
    #[serde(default)]
    pub label: String,
    pub source: MidiSource,
    #[serde(default)]
    pub mode: MidiValueMode,
    pub target: MidiTarget,
    #[serde(default)]
    pub min: f32,
    #[serde(default = "default_one")]
    pub max: f32,
    #[serde(default)]
    pub invert: bool,
    #[serde(default = "default_true")]
    pub pickup: bool,
}

impl MidiMapping {
    pub fn new(id: u64, source: MidiSource, mode: MidiValueMode, target: MidiTarget) -> Self {
        Self {
            id,
            label: target.default_label(),
            source,
            mode,
            target,
            min: 0.0,
            max: 1.0,
            invert: false,
            pickup: true,
        }
    }

    pub fn map_normalized(&self, raw_norm: f32) -> f32 {
        let t = if self.invert {
            1.0 - raw_norm.clamp(0.0, 1.0)
        } else {
            raw_norm.clamp(0.0, 1.0)
        };
        (self.min + t * (self.max - self.min)).clamp(0.0, 1.0)
    }
}

crate::define_choice_enum! {
    /// How the controller's performance pads behave when not overridden by an explicit `MidiMapping`.
    #[derive(Default)]
    pub enum MidiPadMode {
        /// Pads send standard notes (plays Rack kits / instruments directly with velocity).
        #[default]
        Notes => ("notes", "Notes / Kits", ["kits"]),
        /// Pads 1..8 jump directly to Song Sections 1..8 with live RGB LED feedback.
        Sections => ("sections", "Sections 1–8", ["sec"]),
        /// Pads 1..8 toggle Lane Mutes 1..8 with live RGB LED feedback.
        Mutes => ("mutes", "Lane Mutes 1–8", ["mute"]),
    }
}

/// Hardware feedback wire protocol selected by a `MidiProfile` (zero hardcoded UI logic).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackProtocol {
    /// Do not transmit MIDI feedback messages.
    #[default]
    None,
    /// Generic MIDI CC echo + NoteOn velocity (`0/64/127`) pad LED feedback.
    StandardCcNote,
    /// Arturia MiniLab 3 class-compliant DAW-mode SysEx (OLED 2-line display + RGB pad/transport LEDs).
    ArturiaDaw,
}

impl FeedbackProtocol {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "Off",
            Self::StandardCcNote => "Standard CC/Note",
            Self::ArturiaDaw => "Arturia DAW SysEx",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportLedKind {
    Loop,
    Stop,
    Play,
    Record,
    Tap,
}

/// Pure semantic hardware feedback event emitted by `cadence-app` and serialized
/// by `cadence-engine::midi` according to the active `FeedbackProtocol`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MidiFeedback {
    /// Enable/disable controller DAW integration mode (e.g., on connect/disconnect).
    InitDawMode(bool),
    /// Push two lines of ASCII text (+ optional `0..=127` bar value) to the controller OLED/LCD.
    Display {
        line1: String,
        line2: String,
        value_bar: Option<u8>,
    },
    /// Set RGB LED color (`0..=127` per channel) for performance pad `0..=7`.
    PadRgb { index: u8, r: u8, g: u8, b: u8 },
    /// Set illuminated state of a hardware transport button LED.
    TransportLed {
        kind: TransportLedKind,
        active: bool,
    },
    /// Echo a normalized CC value back to a motorized fader / LED ring.
    CcValue { channel: u8, cc: u8, value: u8 },
}

/// Reusable, serializable controller device profile.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MidiProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub feedback: FeedbackProtocol,
    #[serde(default)]
    pub pad_channel: Option<u8>,
    #[serde(default = "default_pad_notes")]
    pub pad_notes: [u8; 8],
    #[serde(default)]
    pub mappings: Vec<MidiMapping>,
}

fn default_pad_notes() -> [u8; 8] {
    [36, 37, 38, 39, 40, 41, 42, 43]
}

impl MidiProfile {
    pub const BUILTIN_IDS: [&'static str; 3] = ["generic", "standard_cc", "minilab3_daw"];

    pub fn by_id(id: &str) -> Self {
        match id.trim().to_ascii_lowercase().as_str() {
            "standard_cc" | "standard" => Self::standard_cc(),
            "minilab3" | "minilab3_daw" | "minilab_3" | "arturia_minilab3" => Self::minilab3_daw(),
            _ => Self::generic(),
        }
    }

    pub fn cycle_builtin_id(current: &str, dir: isize) -> &'static str {
        let idx = Self::BUILTIN_IDS
            .iter()
            .position(|&p| p == current)
            .unwrap_or(0) as isize;
        let len = Self::BUILTIN_IDS.len() as isize;
        Self::BUILTIN_IDS[(idx + dir).rem_euclid(len) as usize]
    }

    /// Minimal generic profile: clean canvas for MIDI Learn + universal MMC transport.
    pub fn generic() -> Self {
        Self {
            id: "generic".into(),
            name: "Generic MIDI".into(),
            feedback: FeedbackProtocol::StandardCcNote,
            pad_channel: Some(9), // Channel 10 (0-indexed 9) standard GM drum/pad channel
            pad_notes: default_pad_notes(),
            mappings: Vec::new(),
        }
    }

    /// Standard class-compliant CC profile (Knobs CC 74/71/72/93/73/75/79/78, Faders CC 22..25,
    /// Transport CC 105..109) matching default user programs on MiniLab 3 and common MIDI keyboards.
    pub fn standard_cc() -> Self {
        let mut id = 1u64;
        let mut next = || {
            let v = id;
            id += 1;
            v
        };
        let mut mappings = Vec::new();
        for (slot, cc) in [74, 71, 72, 93, 73, 75, 79, 78].into_iter().enumerate() {
            mappings.push(MidiMapping::new(
                next(),
                MidiSource::Cc { channel: None, cc },
                MidiValueMode::Absolute,
                MidiTarget::FocusedMacro { slot },
            ));
        }
        for (lane_index, cc) in [22, 23, 24].into_iter().enumerate() {
            mappings.push(MidiMapping::new(
                next(),
                MidiSource::Cc { channel: None, cc },
                MidiValueMode::Absolute,
                MidiTarget::LaneGain { lane_index },
            ));
        }
        mappings.push(MidiMapping::new(
            next(),
            MidiSource::Cc {
                channel: None,
                cc: 25,
            },
            MidiValueMode::Absolute,
            MidiTarget::Param {
                address: ParamAddress::Channel {
                    channel: crate::mixer::ChannelRef::Master,
                    param: crate::mixer::ChannelParam::Gain,
                },
            },
        ));
        for (cc, action) in [
            (105, MidiTransportAction::LoopToggle),
            (106, MidiTransportAction::Stop),
            (107, MidiTransportAction::PlayPause),
            (108, MidiTransportAction::Record),
            (109, MidiTransportAction::TapTempo),
        ] {
            mappings.push(MidiMapping::new(
                next(),
                MidiSource::Cc { channel: None, cc },
                MidiValueMode::Trigger,
                MidiTarget::Transport { action },
            ));
        }
        Self {
            id: "standard_cc".into(),
            name: "Standard CC".into(),
            feedback: FeedbackProtocol::StandardCcNote,
            pad_channel: Some(9),
            pad_notes: default_pad_notes(),
            mappings,
        }
    }

    /// Data-driven profile for Arturia MiniLab 3 in DAW mode (Shift+Pad 3 → DAW):
    /// 8 encoders → Focused Macros 1–8, 4 faders → Lanes 1–3 + Master,
    /// Main encoder → scroll/confirm, Shift+Pads 4–8 → Transport,
    /// plus OLED display & RGB pad/transport LED feedback via `FeedbackProtocol::ArturiaDaw`.
    pub fn minilab3_daw() -> Self {
        let mut id = 1u64;
        let mut next = || {
            let v = id;
            id += 1;
            v
        };
        let mut mappings = Vec::new();
        // 8 knobs in MiniLab 3 DAW mode (Channel 1 = 0).
        for (slot, cc) in [86, 87, 89, 90, 110, 111, 116, 117].into_iter().enumerate() {
            mappings.push(MidiMapping::new(
                next(),
                MidiSource::Cc {
                    channel: Some(0),
                    cc,
                },
                MidiValueMode::Absolute,
                MidiTarget::FocusedMacro { slot },
            ));
        }
        // 4 faders in MiniLab 3 DAW mode: CCs 14, 15, 30, 31 (0x0E, 0x0F, 0x1E, 0x1F).
        for (lane_index, cc) in [14, 15, 30].into_iter().enumerate() {
            mappings.push(MidiMapping::new(
                next(),
                MidiSource::Cc {
                    channel: Some(0),
                    cc,
                },
                MidiValueMode::Absolute,
                MidiTarget::LaneGain { lane_index },
            ));
        }
        mappings.push(MidiMapping::new(
            next(),
            MidiSource::Cc {
                channel: Some(0),
                cc: 31,
            },
            MidiValueMode::Absolute,
            MidiTarget::Param {
                address: ParamAddress::Channel {
                    channel: crate::mixer::ChannelRef::Master,
                    param: crate::mixer::ChannelParam::Gain,
                },
            },
        ));
        // Main encoder turn (CC 28, Offset64 relative) & click (CC 118, trigger).
        mappings.push(MidiMapping::new(
            next(),
            MidiSource::Cc {
                channel: Some(0),
                cc: 28,
            },
            MidiValueMode::RelativeOffset64,
            MidiTarget::Transport {
                action: MidiTransportAction::NavigateControl,
            },
        ));
        mappings.push(MidiMapping::new(
            next(),
            MidiSource::Cc {
                channel: Some(0),
                cc: 118,
            },
            MidiValueMode::Trigger,
            MidiTarget::Transport {
                action: MidiTransportAction::ConfirmControl,
            },
        ));
        // Shift + Pads 4..8 transport CCs (105..109).
        for (cc, action) in [
            (105, MidiTransportAction::LoopToggle),
            (106, MidiTransportAction::Stop),
            (107, MidiTransportAction::PlayPause),
            (108, MidiTransportAction::Record),
            (109, MidiTransportAction::TapTempo),
        ] {
            mappings.push(MidiMapping::new(
                next(),
                MidiSource::Cc { channel: None, cc },
                MidiValueMode::Trigger,
                MidiTarget::Transport { action },
            ));
        }
        Self {
            id: "minilab3_daw".into(),
            name: "Arturia MiniLab 3 (DAW)".into(),
            feedback: FeedbackProtocol::ArturiaDaw,
            pad_channel: Some(9),
            pad_notes: default_pad_notes(),
            mappings,
        }
    }

    pub fn to_json_bytes(&self) -> Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec_pretty(self).context("Serializing MidiProfile")?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes).context("Parsing MIDI profile JSON")
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        let bytes = self.to_json_bytes()?;
        write_atomic(path, MAX_MIDI_PROFILE_BYTES, |buf| {
            buf.extend_from_slice(&bytes);
            Ok(())
        })
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let bytes = read_bounded(path, MAX_MIDI_PROFILE_BYTES)
            .with_context(|| format!("Reading MIDI profile {}", path.display()))?;
        Self::from_json_bytes(&bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_profiles_roundtrip_json_and_decode_relative_modes() {
        for id in MidiProfile::BUILTIN_IDS {
            let prof = MidiProfile::by_id(id);
            let json = serde_json::to_string_pretty(&prof).unwrap();
            let parsed: MidiProfile = serde_json::from_str(&json).unwrap();
            assert_eq!(prof, parsed);
        }
        assert_eq!(
            MidiValueMode::RelativeSigned.decode_relative_delta(1),
            Some(1)
        );
        assert_eq!(
            MidiValueMode::RelativeSigned.decode_relative_delta(127),
            Some(-1)
        );
        assert_eq!(
            MidiValueMode::RelativeOffset64.decode_relative_delta(65),
            Some(1)
        );
        assert_eq!(
            MidiValueMode::RelativeOffset64.decode_relative_delta(63),
            Some(-1)
        );
        assert_eq!(
            MidiValueMode::RelativeOffset64.decode_relative_delta(64),
            None
        );

        let dir = tempfile::tempdir().unwrap();
        let huge = dir.path().join("oversized_profile.json");
        std::fs::write(&huge, vec![b' '; (MAX_MIDI_PROFILE_BYTES + 1) as usize]).unwrap();
        assert!(MidiProfile::load_from(&huge).is_err());
    }
}
