//! Mixer topology, routing acyclicity, send/insert, duck, and automation validation.

use super::{
    Channel, ChannelKind, ChannelRef, ChannelSend, EffectDevice, Mixer, MixerParamTarget,
    MAX_AUDIO_CHANNELS, MAX_DUCKS, MAX_EFFECT_DEVICES, MAX_GROUPS, MAX_INSERTS,
    MAX_MIXER_AUTOMATIONS, MAX_RETURNS, MAX_SENDS,
};
use anyhow::{bail, ensure, Context, Result};
use std::collections::HashSet;

pub(super) fn validate_name(name: &str, kind: &str) -> Result<()> {
    ensure!(
        !name.trim().is_empty()
            && name.chars().count() <= 80
            && !name.chars().any(char::is_control),
        "{kind} name must contain 1..80 printable characters"
    );
    Ok(())
}

/// Returns true if routing Group `start_id` to `first_hop` forms a Group routing cycle.
fn has_group_cycle(channels: &[Channel], start_id: u64, first_hop: ChannelRef) -> bool {
    let mut visited = HashSet::new();
    visited.insert(start_id);
    let mut cur = first_hop;
    while let ChannelRef::Channel(next_id) = cur {
        if !visited.insert(next_id) {
            return true;
        }
        let Some(next_ch) = channels.iter().find(|c| c.id == next_id) else {
            break;
        };
        if next_ch.kind != ChannelKind::Group {
            break;
        }
        cur = next_ch.output;
    }
    false
}

impl Mixer {
    pub fn validate(&self, song_steps: u32) -> Result<()> {
        self.master.strip.validate()?;
        ensure!(
            self.channels
                .iter()
                .filter(|c| c.kind == ChannelKind::Audio)
                .count()
                <= MAX_AUDIO_CHANNELS,
            "At most {MAX_AUDIO_CHANNELS} Audio channels are supported"
        );
        ensure!(
            self.channels
                .iter()
                .filter(|c| c.kind == ChannelKind::Group)
                .count()
                <= MAX_GROUPS,
            "At most {MAX_GROUPS} Groups are supported"
        );
        ensure!(
            self.channels
                .iter()
                .filter(|c| c.kind == ChannelKind::Return)
                .count()
                <= MAX_RETURNS,
            "At most {MAX_RETURNS} Returns are supported"
        );
        let mut channel_ids = HashSet::new();
        for ch in &self.channels {
            ensure!(channel_ids.insert(ch.id), "Duplicate channel ID {}", ch.id);
            validate_name(&ch.name, "Channel")?;
            ch.strip.validate()?;
            ensure!(
                ch.kind != ChannelKind::Return || ch.sends.is_empty(),
                "Returns cannot send to other channels"
            );
            match (ch.kind, ch.output) {
                (_, ChannelRef::Master) => {}
                (ChannelKind::Audio, ChannelRef::Channel(id)) => ensure!(
                    self.channel(id)
                        .is_some_and(|c| c.kind == ChannelKind::Group),
                    "Audio channels may output only to Master or a Group"
                ),
                (ChannelKind::Group, ChannelRef::Channel(id)) => {
                    ensure!(id != ch.id, "Group '{}' cannot output to itself", ch.name);
                    ensure!(
                        self.channel(id)
                            .is_some_and(|c| c.kind == ChannelKind::Group),
                        "Groups may output only to Master or another Group"
                    );
                }
                (ChannelKind::Return, ChannelRef::Channel(_)) => {
                    bail!("Returns output to Master; routing a Return to another channel is unsupported");
                }
            }
        }
        // Reject any Group -> ... -> Group routing cycle via explicit walk.
        for ch in &self.channels {
            if ch.kind == ChannelKind::Group {
                ensure!(
                    !has_group_cycle(&self.channels, ch.id, ch.output),
                    "Group routing cycle detected involving group '{}' (#{})",
                    ch.name,
                    ch.id
                );
            }
        }
        let mut device_ids = HashSet::new();
        self.validate_inserts(&self.master.inserts, &mut device_ids)?;
        for ch in &self.channels {
            self.validate_inserts(&ch.inserts, &mut device_ids)?;
            self.validate_sends(&ch.sends)?;
        }
        ensure!(
            device_ids.len() <= MAX_EFFECT_DEVICES,
            "At most {MAX_EFFECT_DEVICES} effect devices are supported"
        );
        ensure!(
            self.automations.len() <= MAX_MIXER_AUTOMATIONS,
            "At most {MAX_MIXER_AUTOMATIONS} mixer automation lanes are supported"
        );
        ensure!(
            self.ducks.len() <= MAX_DUCKS,
            "At most {MAX_DUCKS} ducks are supported"
        );
        let mut duck_ids = HashSet::new();
        for duck in &self.ducks {
            duck.validate()?;
            ensure!(duck_ids.insert(duck.id), "Duplicate duck ID {}", duck.id);
            match duck.target {
                ChannelRef::Master => {}
                ChannelRef::Channel(id) => {
                    ensure!(self.channel(id).is_some(), "Duck target channel is missing")
                }
            }
        }
        let mut targets = HashSet::new();
        for lane in &self.automations {
            ensure!(
                targets.insert(lane.target),
                "Duplicate mixer automation target"
            );
            ensure!(
                !lane.points.is_empty(),
                "Mixer automation needs at least one point"
            );
            let mut previous = None;
            for point in &lane.points {
                ensure!(
                    point.step < song_steps,
                    "Mixer automation point outside song"
                );
                ensure!(
                    previous.is_none_or(|step| point.step > step),
                    "Mixer automation points must be unique and sorted"
                );
                ensure!(
                    point.value.is_finite() && (0.0..=1.0).contains(&point.value),
                    "Mixer automation value must be within 0..1"
                );
                previous = Some(point.step);
            }
            let has_channel = |channel| match channel {
                ChannelRef::Master => true,
                ChannelRef::Channel(id) => self.channel(id).is_some(),
            };
            match lane.target {
                MixerParamTarget::Channel { channel, .. }
                | MixerParamTarget::ChannelGainScale { channel } => {
                    ensure!(has_channel(channel), "Mixer automation channel is missing");
                }
                MixerParamTarget::Device { device_id, param } => {
                    let device = self
                        .all_inserts()
                        .find(|d| d.id == device_id)
                        .context("Mixer automation device is missing")?;
                    ensure!(
                        device.supports_param(param),
                        "Mixer automation parameter is unsupported by its device"
                    );
                }
                MixerParamTarget::Send { channel, target } => {
                    let sends = match channel {
                        ChannelRef::Master => None,
                        ChannelRef::Channel(id) => self.channel(id).map(|c| c.sends.as_slice()),
                    };
                    ensure!(
                        sends.is_some_and(|s| s.iter().any(|send| send.target == target)),
                        "Mixer automation send is missing"
                    );
                }
            }
        }
        Ok(())
    }

    fn validate_inserts(&self, inserts: &[EffectDevice], ids: &mut HashSet<u64>) -> Result<()> {
        ensure!(
            inserts.len() <= MAX_INSERTS,
            "At most {MAX_INSERTS} inserts per channel are supported"
        );
        for device in inserts {
            ensure!(
                ids.insert(device.id),
                "Duplicate effect device ID {}",
                device.id
            );
            device.validate()?;
        }
        Ok(())
    }

    fn validate_send(&self, target: u64, amount: f32) -> Result<()> {
        ensure!(
            self.channel(target)
                .is_some_and(|c| c.kind == ChannelKind::Return),
            "Send target must be an existing Return"
        );
        ensure!(
            amount.is_finite() && (0.0..=1.0).contains(&amount),
            "Send amount must be within 0..1"
        );
        Ok(())
    }

    fn validate_sends(&self, sends: &[ChannelSend]) -> Result<()> {
        ensure!(
            sends.len() <= MAX_SENDS,
            "At most {MAX_SENDS} sends per channel are supported"
        );
        let mut targets = HashSet::new();
        for send in sends {
            ensure!(targets.insert(send.target), "Duplicate send destination");
            self.validate_send(send.target, send.amount)?;
        }
        Ok(())
    }
}

/// Return all valid, acyclic [`ChannelRef`] output destinations for `source_id`
/// (`Master` first, followed by every valid non-cyclic `Group` in channel order).
///
/// Single source of truth shared by UI output cyclers and validation tests.
pub fn valid_output_targets(channels: &[Channel], source_id: u64) -> Vec<ChannelRef> {
    let Some(source) = channels.iter().find(|c| c.id == source_id) else {
        return vec![ChannelRef::Master];
    };
    let mut out = vec![ChannelRef::Master];
    match source.kind {
        ChannelKind::Return => out,
        ChannelKind::Audio => {
            for ch in channels {
                if ch.kind == ChannelKind::Group {
                    out.push(ChannelRef::Channel(ch.id));
                }
            }
            out
        }
        ChannelKind::Group => {
            for ch in channels {
                if ch.kind != ChannelKind::Group || ch.id == source_id {
                    continue;
                }
                if !has_group_cycle(channels, source_id, ChannelRef::Channel(ch.id)) {
                    out.push(ChannelRef::Channel(ch.id));
                }
            }
            out
        }
    }
}
