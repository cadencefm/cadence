//! Channel strips, bus/return routing references, sidechain ducks, and mixer automation targets.

use super::validate::validate_name;
use super::{
    channel_gain_to_norm, norm_to_channel_gain, EffectDevice, EffectParam, MAX_CHANNEL_GAIN,
};
use crate::params::ValueMode;
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "id",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ChannelRef {
    Channel(u64),
    Master,
}

impl fmt::Display for ChannelRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Channel(id) => write!(f, "channel:{id}"),
            Self::Master => write!(f, "master"),
        }
    }
}

impl ChannelRef {
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.trim().to_ascii_lowercase();
        if input == "master" {
            return Some(Self::Master);
        }
        let (kind, id) = input.split_once(':')?;
        let id = id.parse().ok()?;
        match kind {
            "channel" => Some(Self::Channel(id)),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelStrip {
    pub gain: f32,
    pub pan: f32,
}

impl Default for ChannelStrip {
    fn default() -> Self {
        Self {
            gain: 1.0,
            pan: 0.0,
        }
    }
}

impl ChannelStrip {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.gain.is_finite() && (0.0..=MAX_CHANNEL_GAIN).contains(&self.gain),
            "Channel gain must be finite and within 0..{MAX_CHANNEL_GAIN}"
        );
        ensure!(
            self.pan.is_finite() && (-1.0..=1.0).contains(&self.pan),
            "Channel pan must be finite and within -1..1"
        );
        Ok(())
    }
}

crate::define_choice_enum! {
    pub enum ChannelKind {
        Audio => ("audio", "Audio"),
        Group => ("group", "Group"),
        Return => ("return", "Return"),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelSend {
    pub target: u64,
    pub amount: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MasterChannel {
    pub strip: ChannelStrip,
    pub inserts: Vec<EffectDevice>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Channel {
    pub id: u64,
    pub name: String,
    pub kind: ChannelKind,
    pub output: ChannelRef,
    pub strip: ChannelStrip,
    pub inserts: Vec<EffectDevice>,
    pub sends: Vec<ChannelSend>,
}

impl Channel {
    pub fn new(id: u64, kind: ChannelKind, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            kind,
            output: ChannelRef::Master,
            strip: ChannelStrip::default(),
            inserts: Vec::new(),
            sends: Vec::new(),
        }
    }
}

/// Minimum and maximum sidechain [`Duck`] recovery time in milliseconds.
pub const DUCK_RECOVERY_MIN_MS: f32 = 10.0;
pub const DUCK_RECOVERY_MAX_MS: f32 = 2000.0;

/// Normalize a sidechain [`Duck`] recovery time (`DUCK_RECOVERY_MIN_MS..=DUCK_RECOVERY_MAX_MS`)
/// onto `[0.0, 1.0]`.
#[inline]
pub fn normalize_duck_recovery_ms(ms: f32) -> f32 {
    if !ms.is_finite() {
        return 0.0;
    }
    ((ms - DUCK_RECOVERY_MIN_MS) / (DUCK_RECOVERY_MAX_MS - DUCK_RECOVERY_MIN_MS)).clamp(0.0, 1.0)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Duck {
    pub id: u64,
    pub name: String,
    pub source_part: u64,
    pub target: ChannelRef,
    pub depth: f32,
    pub recovery_ms: f32,
}

impl Duck {
    pub fn validate(&self) -> Result<()> {
        validate_name(&self.name, "Duck")?;
        ensure!(
            self.depth.is_finite() && (0.0..=1.0).contains(&self.depth),
            "Duck depth must be 0..1"
        );
        ensure!(
            self.recovery_ms.is_finite()
                && (DUCK_RECOVERY_MIN_MS..=DUCK_RECOVERY_MAX_MS).contains(&self.recovery_ms),
            "Duck recovery must be {DUCK_RECOVERY_MIN_MS}..{DUCK_RECOVERY_MAX_MS} ms"
        );
        Ok(())
    }
}

crate::define_choice_enum! {
    pub enum ChannelParam {
        Gain => ("gain", "Gain", ["level", "trim"]),
        Pan => ("pan", "Pan"),
    }
}

impl ChannelParam {
    /// Normalize a native channel parameter value into `0.0..=1.0`.
    #[inline]
    pub fn normalize(self, native: f32) -> f32 {
        match self {
            Self::Gain => channel_gain_to_norm(native),
            Self::Pan => {
                if !native.is_finite() {
                    0.5
                } else {
                    ((native + 1.0) * 0.5).clamp(0.0, 1.0)
                }
            }
        }
    }

    /// Denormalize a `0.0..=1.0` value into native channel units
    /// (`0.0..=MAX_CHANNEL_GAIN` for `Gain`, `-1.0..=1.0` for `Pan`).
    #[inline]
    pub fn denormalize(self, norm: f32) -> f32 {
        match self {
            Self::Gain => norm_to_channel_gain(norm),
            Self::Pan => {
                if !norm.is_finite() {
                    0.0
                } else {
                    (norm.clamp(0.0, 1.0) * 2.0 - 1.0).clamp(-1.0, 1.0)
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case", deny_unknown_fields)]
pub enum MixerParamTarget {
    Channel {
        channel: ChannelRef,
        param: ChannelParam,
    },
    /// Internal multiplicative level-shape envelope driven by `ModulationTarget::ChannelGain`.
    ChannelGainScale {
        channel: ChannelRef,
    },
    Send {
        channel: ChannelRef,
        target: u64,
    },
    Device {
        device_id: u64,
        param: EffectParam,
    },
}

impl MixerParamTarget {
    pub fn label(self) -> String {
        match self {
            Self::Channel { channel, param } => format!("{channel} {}", param.label()),
            Self::ChannelGainScale { channel } => format!("{channel} Level Shape"),
            Self::Send { channel, target } => format!("{channel} send → channel:{target}"),
            Self::Device { device_id, param } => format!("device:{device_id} {}", param.label()),
        }
    }

    pub fn format_normalized(self, value: f32) -> String {
        match self {
            Self::Channel {
                param: ChannelParam::Gain,
                ..
            } => crate::dsp::format_gain_db(ChannelParam::Gain.denormalize(value)),
            Self::Channel {
                param: ChannelParam::Pan,
                ..
            } => crate::params::format_pan(ChannelParam::Pan.denormalize(value)),
            Self::Device { param, .. } => param.format_normalized(value),
            _ => format!("{:.0}%", value.clamp(0.0, 1.0) * 100.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MixerBreakpoint {
    pub step: u32,
    pub value: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MixerAutomation {
    pub target: MixerParamTarget,
    pub mode: ValueMode,
    pub points: Vec<MixerBreakpoint>,
}
