//! Mixer channel, effect insert, routing, and safe-delete action handlers.
use crate::*;

impl App {
    pub(super) fn apply_create_channel(
        &mut self,
        name: String,
        kind: ChannelKind,
    ) -> Result<Outcome> {
        let id = self.doc.fresh_ids(1)?[0];
        let out = self.commit(
            vec![Edit::CreateChannel {
                channel: Channel::new(id, kind, name),
            }],
            format!("Created channel {id}"),
        )?;
        self.open_channel(ChannelRef::Channel(id))?;
        Ok(out)
    }

    pub(super) fn apply_rename_channel(&mut self, channel: Id, name: String) -> Result<Outcome> {
        let trimmed = drafts::form::validate_printable_name("Channel", &name, 80)?;
        ensure!(
            !trimmed.eq_ignore_ascii_case("Master"),
            "Channel name 'Master' is reserved"
        );
        ensure!(
            !self
                .doc
                .project()
                .mixer
                .channels
                .iter()
                .any(|c| c.id != channel && c.name.eq_ignore_ascii_case(&trimmed)),
            "A channel named '{trimmed}' already exists"
        );
        let old_name = self
            .channel(channel)
            .context("Channel missing")?
            .name
            .clone();
        self.commit(
            vec![Edit::UpdateChannelMeta {
                id: channel,
                name: Some(trimmed.clone()),
                output: None,
                sends: None,
            }],
            format!("Renamed channel '{old_name}' → '{trimmed}'"),
        )
    }

    pub(super) fn apply_add_effect(
        &mut self,
        channel: ChannelRef,
        kind: String,
    ) -> Result<Outcome> {
        let id = self.doc.fresh_ids(1)?[0];
        let mut device = EffectDevice::from_kind_name(id, &kind)
            .with_context(|| format!("Effects: {}", EffectDevice::kind_names_hint()))?;
        if device.supports_param(EffectParam::Mix) {
            if matches!(channel, ChannelRef::Channel(id) if self.channel(id).is_some_and(|c| c.kind == ChannelKind::Return))
            {
                device.write_normalized(EffectParam::Mix, 1.0)?;
            } else if device.prefers_subtle_insert_mix() {
                device.write_normalized(EffectParam::Mix, 0.3)?;
            }
        }
        let out = self.commit(
            vec![Edit::AddEffect {
                channel,
                device,
                index: None,
            }],
            format!("Added {kind} device {id}"),
        )?;
        self.push_back();
        self.selected_channel = channel;
        self.show_detail(Detail::Effect(id));
        Ok(out)
    }

    pub(crate) fn effect_delete_summary(&self, device: Id) -> Result<String> {
        let (owner, dev) = self.effect(device).context("Effect missing")?;
        let owner_name = self.channel_name(owner);
        let mods = self
            .doc
            .project()
            .modulations
            .iter()
            .filter(|m| {
                matches!(
                    m.target,
                    ModulationTarget::Effect { device: d, .. } if d == device
                )
            })
            .count();
        let macro_bindings: usize = self
            .doc
            .project()
            .macros
            .iter()
            .map(|mac| {
                mac.bindings
                    .iter()
                    .filter(|b| {
                        matches!(b.target, ParamAddress::Effect { device: d, .. } if d == device)
                    })
                    .count()
            })
            .sum();
        let mut notes = Vec::new();
        if mods > 0 {
            notes.push(format!("{mods} mod{}", if mods == 1 { "" } else { "s" }));
        }
        if macro_bindings > 0 {
            notes.push(format!(
                "{macro_bindings} macro binding{}",
                if macro_bindings == 1 { "" } else { "s" }
            ));
        }
        if notes.is_empty() {
            Ok(format!("Delete effect '{}' on {owner_name}?", dev.name))
        } else {
            Ok(format!(
                "Delete effect '{}' on {owner_name} (removes {})?",
                dev.name,
                notes.join(", ")
            ))
        }
    }

    pub(crate) fn request_delete_effect(&mut self, device: Id) -> Result<Outcome> {
        let msg = self.effect_delete_summary(device)?;
        self.input.mode = Mode::Confirm(ConfirmAction::DeleteEffect(device));
        self.status = format!("{msg} y confirm · Esc cancel");
        Ok(Outcome::default())
    }

    pub(super) fn apply_delete_effect(&mut self, device: Id) -> Result<Outcome> {
        let (owner, dev) = self.effect(device).context("Effect missing")?;
        let name = dev.name.clone();
        let mut edits: Vec<Edit> = self
            .doc
            .project()
            .modulations
            .iter()
            .filter(|m| {
                matches!(
                    m.target,
                    ModulationTarget::Effect { device: target_dev, .. }
                        if target_dev == device
                )
            })
            .map(|m| Edit::RemoveModulation { id: m.id })
            .collect();
        edits.push(Edit::RemoveEffect { device });
        let out = self.commit(edits, format!("Deleted effect {name}"))?;
        if self.detail == Detail::Effect(device) {
            self.selected_channel = owner;
            self.show_detail(Detail::Channel(owner));
        }
        Ok(out)
    }

    pub(super) fn apply_move_effect(
        &mut self,
        device: Id,
        channel: ChannelRef,
        position: usize,
    ) -> Result<Outcome> {
        let dest_name = self.channel_name(channel).to_string();
        self.commit(
            vec![Edit::MoveEffect {
                device,
                target_channel: channel,
                target_slot: Some(position),
            }],
            format!("Moved effect {device} → {dest_name} slot {}", position + 1),
        )
    }

    pub(super) fn apply_set_effect_bypass(
        &mut self,
        device: Id,
        bypassed: bool,
    ) -> Result<Outcome> {
        let name = self
            .effect(device)
            .context("Effect missing")?
            .1
            .name
            .clone();
        self.commit(
            vec![Edit::UpdateEffect {
                device,
                name: None,
                bypassed: Some(bypassed),
                kind: None,
            }],
            format!("{} {}", name, if bypassed { "bypassed" } else { "active" }),
        )
    }

    pub(super) fn apply_rename_effect(&mut self, device: Id, name: String) -> Result<Outcome> {
        let trimmed = drafts::form::validate_printable_name("Effect", &name, 80)?;
        let old_name = self
            .effect(device)
            .context("Effect missing")?
            .1
            .name
            .clone();
        self.commit(
            vec![Edit::UpdateEffect {
                device,
                name: Some(trimmed.clone()),
                bypassed: None,
                kind: None,
            }],
            format!("Renamed effect '{old_name}' → '{trimmed}'"),
        )
    }

    pub(super) fn apply_route_instrument(
        &mut self,
        instrument: Id,
        channel: Id,
    ) -> Result<Outcome> {
        let inst_name = self
            .instrument(instrument)
            .context("Instrument missing")?
            .name
            .clone();
        let dest_name = self.channel_name(ChannelRef::Channel(channel)).to_string();
        self.commit(
            vec![Edit::UpdateInstrumentMeta {
                id: instrument,
                name: None,
                channel: Some(channel),
            }],
            format!("Routed {inst_name} → {dest_name}"),
        )
    }

    pub(super) fn apply_set_channel_output(
        &mut self,
        channel: Id,
        output: ChannelRef,
    ) -> Result<Outcome> {
        let ch_name = self
            .channel(channel)
            .context("Channel missing")?
            .name
            .clone();
        let out_name = self.channel_name(output).to_string();
        self.commit(
            vec![Edit::UpdateChannelMeta {
                id: channel,
                name: None,
                output: Some(output),
                sends: None,
            }],
            format!("{ch_name} output → {out_name}"),
        )
    }

    pub(super) fn apply_set_channel_send(
        &mut self,
        channel: Id,
        target: Id,
        amount: Option<f32>,
    ) -> Result<Outcome> {
        let mut sends = self
            .channel(channel)
            .context("Channel missing")?
            .sends
            .clone();
        let mut edits = Vec::new();
        if let Some(amount) = amount {
            if let Some(s) = sends.iter_mut().find(|s| s.target == target) {
                s.amount = amount;
            } else {
                sends.push(ChannelSend { target, amount });
            }
        } else {
            sends.retain(|s| s.target != target);
            for m in &self.doc.project().modulations {
                if matches!(
                    m.target,
                    ModulationTarget::Send {
                        channel: ChannelRef::Channel(ch),
                        target: send_target,
                    } if ch == channel && send_target == target
                ) {
                    edits.push(Edit::RemoveModulation { id: m.id });
                }
            }
        }
        edits.push(Edit::UpdateChannelMeta {
            id: channel,
            name: None,
            output: None,
            sends: Some(sends),
        });
        self.commit(edits, "Updated channel send".into())
    }

    fn channel_delete_preflight(&self, id: Id) -> Result<(String, Option<Id>, usize)> {
        let project = self.doc.project();
        let ch = project.channel(id).context("Channel missing")?;
        let routed_instruments = project
            .instruments
            .iter()
            .filter(|i| i.channel == id)
            .count();
        let fallback_audio = if routed_instruments > 0 {
            let fallback = project
                .mixer
                .channels
                .iter()
                .find(|c| c.id != id && c.kind == ChannelKind::Audio)
                .map(|c| c.id)
                .context(
                    "Cannot delete the only Audio channel while instruments are routed to it",
                )?;
            Some(fallback)
        } else {
            None
        };
        let insert_ids: Vec<Id> = ch.inserts.iter().map(|d| d.id).collect();
        let child_outputs = project
            .mixer
            .channels
            .iter()
            .filter(|c| c.id != id && c.output == ChannelRef::Channel(id))
            .count();
        let incoming_sends = project
            .mixer
            .channels
            .iter()
            .filter(|c| c.id != id)
            .flat_map(|c| c.sends.iter())
            .filter(|s| s.target == id)
            .count();
        let mod_deps = project
            .modulations
            .iter()
            .filter(|m| Self::modulation_targets_channel(&m.target, id, &insert_ids))
            .count();
        let duck_deps = project
            .ducks
            .iter()
            .filter(|d| d.target == ChannelRef::Channel(id))
            .count();
        let macro_deps = project
            .macros
            .iter()
            .flat_map(|m| m.bindings.iter())
            .filter(|b| Self::param_targets_channel(b.target, id, &insert_ids))
            .count();
        let deps = routed_instruments
            + child_outputs
            + incoming_sends
            + ch.inserts.len()
            + ch.sends.len()
            + mod_deps
            + duck_deps
            + macro_deps;
        Ok((ch.name.clone(), fallback_audio, deps))
    }

    fn modulation_targets_channel(target: &ModulationTarget, id: Id, insert_ids: &[Id]) -> bool {
        match *target {
            ModulationTarget::ChannelGain { channel }
            | ModulationTarget::ChannelPan { channel } => channel == ChannelRef::Channel(id),
            ModulationTarget::Send { channel, target } => {
                channel == ChannelRef::Channel(id) || target == id
            }
            ModulationTarget::Effect { device, .. } => insert_ids.contains(&device),
            ModulationTarget::Instrument { .. } | ModulationTarget::Macro { .. } => false,
        }
    }

    fn param_targets_channel(addr: ParamAddress, id: Id, insert_ids: &[Id]) -> bool {
        match addr {
            ParamAddress::Channel { channel, .. } => channel == ChannelRef::Channel(id),
            ParamAddress::Send { channel, target } => {
                channel == ChannelRef::Channel(id) || target == id
            }
            ParamAddress::Effect { device, .. } => insert_ids.contains(&device),
            ParamAddress::Rack { .. }
            | ParamAddress::Zone { .. }
            | ParamAddress::Module { .. }
            | ParamAddress::Cable { .. } => false,
        }
    }

    pub(crate) fn request_delete_channel(&mut self, id: Id) -> Result<Outcome> {
        let (name, _, deps) = self.channel_delete_preflight(id)?;
        if deps > 0 {
            self.input.mode = Mode::Confirm(ConfirmAction::DeleteChannel(id));
            self.status = format!(
                "Delete channel '{name}' ({deps} dependent{} will be cleaned up/rerouted)? y confirm · Esc cancel",
                if deps == 1 { "" } else { "s" }
            );
            Ok(Outcome::default())
        } else {
            self.delete_channel_safe(id)
        }
    }

    pub(crate) fn delete_channel_safe(&mut self, id: Id) -> Result<Outcome> {
        let (ch_name, fallback_audio, _) = self.channel_delete_preflight(id)?;
        let project = self.doc.project();
        let ch = project.channel(id).context("Channel missing")?;
        let insert_ids: Vec<Id> = ch.inserts.iter().map(|d| d.id).collect();
        let mut edits = Vec::new();
        let mut rerouted = 0usize;

        if let Some(fallback) = fallback_audio {
            for inst in &project.instruments {
                if inst.channel == id {
                    edits.push(Edit::UpdateInstrumentMeta {
                        id: inst.id,
                        name: None,
                        channel: Some(fallback),
                    });
                    rerouted += 1;
                }
            }
        }
        for other in &project.mixer.channels {
            if other.id == id {
                continue;
            }
            let routes_here = other.output == ChannelRef::Channel(id);
            let sends_here = other.sends.iter().any(|s| s.target == id);
            if routes_here || sends_here {
                if routes_here {
                    rerouted += 1;
                }
                edits.push(Edit::UpdateChannelMeta {
                    id: other.id,
                    name: None,
                    output: routes_here.then_some(ChannelRef::Master),
                    sends: sends_here.then(|| {
                        other
                            .sends
                            .iter()
                            .filter(|s| s.target != id)
                            .cloned()
                            .collect()
                    }),
                });
            }
        }
        for m in &project.modulations {
            if Self::modulation_targets_channel(&m.target, id, &insert_ids) {
                edits.push(Edit::RemoveModulation { id: m.id });
            }
        }
        for d in &project.ducks {
            if d.target == ChannelRef::Channel(id) {
                edits.push(Edit::RemoveDuck { id: d.id });
            }
        }
        edits.push(Edit::RemoveChannel { id });

        let label = if rerouted > 0 {
            format!("Deleted channel '{ch_name}' ({rerouted} rerouted)")
        } else {
            format!("Deleted channel '{ch_name}'")
        };
        let out = self.commit(edits, label)?;
        if self.selected_channel == ChannelRef::Channel(id) {
            self.selected_channel = ChannelRef::Master;
        }
        if self.detail == Detail::Channel(ChannelRef::Channel(id))
            || matches!(self.detail, Detail::Effect(dev) if insert_ids.contains(&dev))
        {
            self.detail = Detail::Channel(ChannelRef::Master);
            self.control_cursor = 0;
        }
        Ok(out)
    }
}
