//! Portable mixer/device definitions. No UI, filesystem, or audio backend dependencies.

mod channel;
mod effect_params;
mod effects;
mod flags;
mod validate;

#[cfg(test)]
mod tests;

pub use crate::dispatch_effect_kind;
pub use crate::dsp::gain::{
    channel_gain_to_norm, norm_to_channel_gain, nudge_channel_gain_db, CHANNEL_GAIN_UNITY_NORM,
    MAX_CHANNEL_GAIN, MAX_CHANNEL_GAIN_DB,
};
pub use channel::{
    normalize_duck_recovery_ms, Channel, ChannelKind, ChannelParam, ChannelRef, ChannelSend,
    ChannelStrip, Duck, MasterChannel, MixerAutomation, MixerBreakpoint, MixerParamTarget,
    DUCK_RECOVERY_MAX_MS, DUCK_RECOVERY_MIN_MS,
};
pub use effect_params::{EffectParam, EffectParamDescriptor, ParameterUnit};
pub use effects::{
    ChorusPatch, CompressorPatch, DelayMode, DelayPatch, DriveMode, DrivePatch, EffectDevice,
    EffectKind, EffectKindTag, EqPatch, FilterPatch, LimiterPatch, ReverbPatch,
};
pub use flags::LiveFlags;
pub use validate::valid_output_targets;

use crate::filter_patterns::CompiledFilterPattern;
use crate::params::ValueMode;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const MAX_AUDIO_CHANNELS: usize = 16;
pub const MAX_GROUPS: usize = 4;
pub const MAX_RETURNS: usize = 4;
pub const MAX_MIXER_CHANNELS: usize = MAX_AUDIO_CHANNELS + MAX_GROUPS + MAX_RETURNS;
pub const MAX_DUCKS: usize = 16;
pub const MAX_CHANNELS: usize = crate::material::MAX_INSTRUMENTS + MAX_MIXER_CHANNELS + 1;
pub const MAX_INSERTS: usize = 4;
pub const MAX_EFFECT_DEVICES: usize = 24;
pub const MAX_SENDS: usize = 4;
pub const MAX_MIXER_AUTOMATIONS: usize = 256;
pub const MAX_DELAY_SECONDS: f64 = 12.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mixer {
    pub master: MasterChannel,
    pub channels: Vec<Channel>,
    pub automations: Vec<MixerAutomation>,
    pub ducks: Vec<Duck>,
    #[serde(default, skip_serializing_if = "LiveFlags::is_empty")]
    pub flags: LiveFlags,
    /// Compiled custom project-pattern trigger grids for `Filter` inserts with `env_pattern_id: Some(..)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filter_env_triggers: Vec<(u64, CompiledFilterPattern)>,
    /// Lock-free snapshot of external rhythmic filter grooves prepared off the audio thread.
    #[serde(skip)]
    pub filter_grooves: Arc<[CompiledFilterPattern]>,
}

impl Default for Mixer {
    fn default() -> Self {
        let mut delay = Channel::new(1, ChannelKind::Return, "Delay");
        delay.strip.gain = 0.55;
        delay.inserts.push(EffectDevice::delay(1));
        Self {
            master: MasterChannel {
                strip: ChannelStrip {
                    gain: 0.7,
                    ..ChannelStrip::default()
                },
                inserts: Vec::new(),
            },
            channels: vec![delay],
            automations: Vec::new(),
            ducks: Vec::new(),
            flags: LiveFlags::default(),
            filter_env_triggers: Vec::new(),
            filter_grooves: Arc::from([]),
        }
    }
}

impl Mixer {
    /// Return the pre-compiled custom rhythmic trigger pattern for `device_id`, if bound.
    pub fn filter_env_trigger(&self, device_id: u64) -> Option<CompiledFilterPattern> {
        self.filter_env_triggers
            .iter()
            .find_map(|&(id, pat)| (id == device_id).then_some(pat))
    }

    /// Wait-free, allocation-free O(1) lookup of a rhythmic filter groove by ID from the
    /// sparse-safe compiled bank snapshot. Missing IDs or absent banks fall back cleanly to
    /// [`CompiledFilterPattern::neutral`] without aliasing onto another groove.
    #[inline]
    pub fn resolve_filter_groove(&self, id: u8) -> CompiledFilterPattern {
        self.filter_grooves
            .get(usize::from(id))
            .copied()
            .unwrap_or_else(CompiledFilterPattern::neutral)
    }

    pub fn channel(&self, id: u64) -> Option<&Channel> {
        self.channels.iter().find(|c| c.id == id)
    }

    pub fn channel_mut(&mut self, id: u64) -> Option<&mut Channel> {
        self.channels.iter_mut().find(|c| c.id == id)
    }

    /// Iterate over every insert effect device across all channels and Master.
    pub fn all_inserts(&self) -> impl Iterator<Item = &EffectDevice> {
        self.channels
            .iter()
            .flat_map(|c| c.inserts.iter())
            .chain(self.master.inserts.iter())
    }

    /// Mutably iterate over every insert effect device across all channels and Master.
    pub fn all_inserts_mut(&mut self) -> impl Iterator<Item = &mut EffectDevice> {
        self.channels
            .iter_mut()
            .flat_map(|c| c.inserts.iter_mut())
            .chain(self.master.inserts.iter_mut())
    }
}

/// Cyclic song automation; the final Ramp point interpolates to the first.
pub fn resolve_automation(lane: &MixerAutomation, song_step: f64, song_steps: u32) -> f32 {
    let Some(first) = lane.points.first() else {
        return 0.0;
    };
    if lane.points.len() == 1 {
        return first.value;
    }
    let length = f64::from(song_steps.max(1));
    let position = if song_step.is_finite() {
        song_step.rem_euclid(length)
    } else {
        0.0
    };
    let next = lane
        .points
        .partition_point(|p| f64::from(p.step) <= position);
    let next = if next == lane.points.len() { 0 } else { next };
    let previous = if next == 0 {
        lane.points.len() - 1
    } else {
        next - 1
    };
    let a = lane.points[previous];
    if lane.mode == ValueMode::Hold {
        return a.value;
    }
    let b = lane.points[next];
    let span = (f64::from(b.step) - f64::from(a.step)).rem_euclid(length);
    if span == 0.0 {
        return a.value;
    }
    let elapsed = (position - f64::from(a.step)).rem_euclid(length);
    a.value + (b.value - a.value) * (elapsed / span) as f32
}
