//! Live mute and solo performance flags for lanes and audio channels.

use super::ChannelRef;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

fn is_false(value: &bool) -> bool {
    !*value
}

/// Live mute and solo flags for lanes and audio channels.
///
/// Saved with the project as you left them, never undone, never a song revision.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveFlags {
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub muted_lanes: BTreeSet<u64>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub solo_lanes: BTreeSet<u64>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub muted_channels: BTreeSet<u64>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub solo_channels: BTreeSet<u64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub master_muted: bool,
}

impl LiveFlags {
    pub fn is_empty(&self) -> bool {
        self.muted_lanes.is_empty()
            && self.solo_lanes.is_empty()
            && self.muted_channels.is_empty()
            && self.solo_channels.is_empty()
            && !self.master_muted
    }

    pub fn lane_muted(&self, lane: u64) -> bool {
        self.muted_lanes.contains(&lane)
    }

    pub fn lane_solo(&self, lane: u64) -> bool {
        self.solo_lanes.contains(&lane)
    }

    pub fn any_lane_solo(&self) -> bool {
        !self.solo_lanes.is_empty()
    }

    pub fn lane_enabled(&self, lane: u64) -> bool {
        !self.lane_muted(lane) && (!self.any_lane_solo() || self.lane_solo(lane))
    }

    pub fn set_lane_muted(&mut self, lane: u64, muted: bool) {
        if muted {
            self.muted_lanes.insert(lane);
        } else {
            self.muted_lanes.remove(&lane);
        }
    }

    pub fn set_lane_solo(&mut self, lane: u64, solo: bool) {
        if solo {
            self.solo_lanes.insert(lane);
        } else {
            self.solo_lanes.remove(&lane);
        }
    }

    pub fn channel_muted(&self, channel: ChannelRef) -> bool {
        match channel {
            ChannelRef::Master => self.master_muted,
            ChannelRef::Channel(id) => self.muted_channels.contains(&id),
        }
    }

    pub fn channel_solo(&self, channel: ChannelRef) -> bool {
        match channel {
            ChannelRef::Channel(id) => self.solo_channels.contains(&id),
            ChannelRef::Master => false,
        }
    }

    pub fn set_channel_muted(&mut self, channel: ChannelRef, muted: bool) -> Result<()> {
        match channel {
            ChannelRef::Master => self.master_muted = muted,
            ChannelRef::Channel(id) => {
                if muted {
                    self.muted_channels.insert(id);
                } else {
                    self.muted_channels.remove(&id);
                }
            }
        }
        Ok(())
    }

    pub fn set_channel_solo(&mut self, channel: ChannelRef, solo: bool) -> Result<()> {
        match channel {
            ChannelRef::Master => bail!("Master cannot be soloed"),
            ChannelRef::Channel(id) => {
                if solo {
                    self.solo_channels.insert(id);
                } else {
                    self.solo_channels.remove(&id);
                }
            }
        }
        Ok(())
    }

    pub fn mixer_compatible(&self, other: &Self) -> bool {
        self.muted_channels == other.muted_channels
            && self.solo_channels == other.solo_channels
            && self.master_muted == other.master_muted
    }
}
