//! Atomic transaction application, `Edit` mutation, parameter writes, and Macro pruning.

use super::{
    Edit, Exception, Id, LanePart, LiveFlags, MacroControl, ModulationTarget, Pattern, PatternKind,
    PatternType, Project, ValuePoint, MAX_BARS, MAX_LITERAL_STEPS, MAX_PROJECT_MACROS,
    MAX_TRANSACTION_EDITS,
};
use crate::mixer::{Channel, ChannelParam, ChannelRef, EffectDevice};
use crate::music::{Step, StepCondition};
use crate::params::ParamAddress;
use crate::rack::{
    ModuleKind, MAX_CABLES_PER_ZONE, MAX_MODULES_PER_ZONE, MAX_PANEL_PINS, MAX_ZONES,
};
use crate::score::ScoreWindow;
use anyhow::{bail, ensure, Context, Result};

impl Project {
    fn prune_stale_instrument_refs(&mut self, inst_id: Id) {
        if let Some(inst) = self.instrument_mut(inst_id) {
            inst.rack.prune_invalid_pins();
        }
        let Some(rack) = self.instrument(inst_id).map(|inst| inst.rack.clone()) else {
            return;
        };
        self.modulations.retain(|m| {
            !matches!(
                m.target,
                ModulationTarget::Instrument {
                    instrument,
                    parameter,
                } if instrument == inst_id && !rack.supports_automatable_param(parameter)
            )
        });
        for part in &mut self.parts {
            if part.instrument == inst_id {
                part.parameter_links
                    .retain(|link| rack.supports_automatable_param(link.target));
            }
        }
        for mac in &mut self.macros {
            mac.bindings.retain(|binding| {
                !matches!(
                    binding.target.as_instrument(),
                    Some((instrument, param))
                        if instrument == inst_id && !rack.supports_automatable_param(param)
                )
            });
        }
    }

    fn prune_macro_bindings(&mut self) {
        if self.macros.is_empty() {
            return;
        }
        let valid_targets: Vec<Vec<ParamAddress>> = self
            .macros
            .iter()
            .map(|m| {
                m.bindings
                    .iter()
                    .filter(|b| self.validate_macro_target(b.target).is_ok())
                    .map(|b| b.target)
                    .collect()
            })
            .collect();
        for (mac, keep) in self.macros.iter_mut().zip(valid_targets) {
            mac.bindings.retain(|b| keep.contains(&b.target));
        }
    }
    pub fn apply(&mut self, edit: Edit) -> Result<ScoreWindow> {
        self.apply_transaction(&[edit])
    }
    pub fn apply_transaction(&mut self, edits: &[Edit]) -> Result<ScoreWindow> {
        self.apply_transaction_with_flags(edits, &LiveFlags::default())
    }
    fn check_channel_kind_invariance(&self, channel: &Channel) -> Result<()> {
        if let Some(existing) = self.channel(channel.id) {
            ensure!(
                existing.kind == channel.kind,
                "Cannot change channel '{}' (#{}) kind from {} to {}",
                existing.name,
                channel.id,
                existing.kind.label(),
                channel.kind.label()
            );
        }
        Ok(())
    }

    fn check_effect_kind_invariance(&self, inserts: &[EffectDevice]) -> Result<()> {
        for incoming in inserts {
            if let Some((_, existing)) = self.device(incoming.id) {
                ensure!(
                    std::mem::discriminant(&existing.kind)
                        == std::mem::discriminant(&incoming.kind),
                    "Cannot change effect '{}' (#{}) kind from {} to {}",
                    existing.name,
                    incoming.id,
                    existing.kind.label(),
                    incoming.kind.label()
                );
            }
        }
        Ok(())
    }

    fn channel_inserts_mut(&mut self, channel: ChannelRef) -> Result<&mut Vec<EffectDevice>> {
        match channel {
            ChannelRef::Master => Ok(&mut self.mixer.master.inserts),
            ChannelRef::Channel(id) => self
                .mixer
                .channels
                .iter_mut()
                .find(|c| c.id == id)
                .map(|c| &mut c.inserts)
                .with_context(|| format!("Channel #{id} does not exist")),
        }
    }

    fn remove_device_owned(&mut self, device: Id) -> Option<EffectDevice> {
        for ch in &mut self.mixer.channels {
            if let Some(pos) = ch.inserts.iter().position(|d| d.id == device) {
                return Some(ch.inserts.remove(pos));
            }
        }
        if let Some(pos) = self
            .mixer
            .master
            .inserts
            .iter()
            .position(|d| d.id == device)
        {
            return Some(self.mixer.master.inserts.remove(pos));
        }
        None
    }

    /// Apply an atomic transaction and validate the resulting project without compiling a full score.
    pub fn apply_transaction_validated(&mut self, edits: &[Edit]) -> Result<()> {
        ensure!(
            edits.len() <= MAX_TRANSACTION_EDITS,
            "Transaction exceeds {MAX_TRANSACTION_EDITS} operations"
        );
        let mut candidate = self.clone();
        let mut needs_macro_prune = false;
        for edit in edits {
            if matches!(
                edit,
                Edit::UpdateRackPatch { .. }
                    | Edit::RemoveRackZone { .. }
                    | Edit::RemoveRackModule { .. }
                    | Edit::DisconnectCable { .. }
                    | Edit::RemoveInstrument { .. }
                    | Edit::CreateChannel { .. }
                    | Edit::RemoveChannel { .. }
                    | Edit::AddEffect { .. }
                    | Edit::RemoveEffect { .. }
                    | Edit::UpdateEffect { .. }
            ) {
                needs_macro_prune = true;
            }
            candidate.mutate(edit)?;
        }
        if needs_macro_prune {
            for ch in &candidate.mixer.channels {
                self.check_channel_kind_invariance(ch)?;
                self.check_effect_kind_invariance(&ch.inserts)?;
            }
            self.check_effect_kind_invariance(&candidate.mixer.master.inserts)?;
            candidate.prune_macro_bindings();
        }
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }
    pub fn apply_transaction_with_flags(
        &mut self,
        edits: &[Edit],
        flags: &LiveFlags,
    ) -> Result<ScoreWindow> {
        let mut candidate = self.clone();
        candidate.apply_transaction_validated(edits)?;
        let compiled = candidate.compile_validated(flags)?;
        *self = candidate;
        Ok(compiled)
    }
    pub fn apply_edit_only(&mut self, edit: &Edit) -> Result<()> {
        self.mutate(edit)
    }
    fn mutate(&mut self, edit: &Edit) -> Result<()> {
        match edit {
            Edit::PutPattern { pattern } => upsert(&mut self.patterns, pattern.clone(), |p| p.id),
            Edit::RemovePattern { id } => remove(&mut self.patterns, *id, |p| p.id, "Pattern")?,
            Edit::SetNote {
                pattern,
                step,
                value,
            } => {
                let PatternKind::Notes { steps } = &mut self
                    .pattern_mut(*pattern)
                    .context("Pattern is missing")?
                    .kind
                else {
                    bail!("Edit a literal Notes definition, add a placement exception, or freeze a range")
                };
                *steps.get_mut(*step).context("Note step is out of range")? = value.clone();
            }
            Edit::SetRhythm {
                pattern,
                step,
                active,
            } => {
                let PatternKind::Rhythm { steps } = &mut self
                    .pattern_mut(*pattern)
                    .context("Pattern is missing")?
                    .kind
                else {
                    bail!("Expected a literal Rhythm")
                };
                *steps
                    .get_mut(*step)
                    .context("Rhythm step is out of range")? = *active;
            }
            Edit::SetValue {
                pattern,
                step,
                value,
            } => {
                let PatternKind::Values { period, points, .. } = &mut self
                    .pattern_mut(*pattern)
                    .context("Pattern is missing")?
                    .kind
                else {
                    bail!("Expected literal Values; freeze a seeded source before editing individual points")
                };
                ensure!(*step < *period, "Value point is out of range");
                points.retain(|p| p.step != *step);
                if let Some(value) = value {
                    points.push(ValuePoint {
                        step: *step,
                        value: *value,
                    });
                    points.sort_by_key(|p| p.step);
                }
            }
            Edit::ResizePattern { pattern, length } => {
                let p = self.pattern_mut(*pattern).context("Pattern is missing")?;
                let max_len = match p.kind {
                    PatternKind::Values { .. } => MAX_BARS * 16,
                    _ => MAX_LITERAL_STEPS,
                };
                ensure!(
                    (1..=max_len).contains(length),
                    "Pattern length must be 1..{max_len}"
                );
                match &mut p.kind {
                    PatternKind::Notes { steps } => steps.resize(*length, Step::default()),
                    PatternKind::Rhythm { steps } => steps.resize(*length, false),
                    PatternKind::Values { period, points, .. } => {
                        *period = *length as u32;
                        points.retain(|p| p.step < *period);
                    }
                    PatternKind::Mini { steps, .. } => {
                        *steps = *length as u16;
                    }
                    PatternKind::Euclidean {
                        pulses,
                        steps,
                        rotation,
                    } => {
                        let next_steps = *length as u16;
                        let max_rot = next_steps.saturating_sub(1) as i16;
                        *steps = next_steps;
                        *pulses = (*pulses).min(next_steps);
                        *rotation = (*rotation).clamp(-max_rot, max_rot);
                    }
                    _ => bail!(
                        "Derived periods follow their sources; edit the source or freeze a range"
                    ),
                }
            }
            Edit::ForkPattern { source, id, name } => {
                ensure!(
                    self.pattern(*id).is_none(),
                    "New pattern identity is already used"
                );
                let mut copy = self
                    .pattern(*source)
                    .context("Source pattern is missing")?
                    .clone();
                copy.id = *id;
                copy.name = name.trim().into();
                self.patterns.push(copy);
            }
            Edit::FreezePattern {
                source,
                id,
                name,
                start,
                length,
            } => {
                ensure!(
                    self.pattern(*id).is_none(),
                    "New pattern identity is already used"
                );
                ensure!(
                    (1..=MAX_LITERAL_STEPS).contains(length),
                    "Freeze range must have 1..{MAX_LITERAL_STEPS} steps"
                );
                let kind = match self.pattern_type(*source)? {
                    PatternType::Notes => PatternKind::Notes {
                        steps: self
                            .preview_note_steps(*source, *start, *length)?
                            .into_iter()
                            .map(|mut step| {
                                step.condition = StepCondition::ALWAYS;
                                step.source_part = None;
                                step
                            })
                            .collect(),
                    },
                    PatternType::Rhythm => PatternKind::Rhythm {
                        steps: self
                            .preview(*source, *start, *length)?
                            .into_iter()
                            .map(|p| p.value.active())
                            .collect(),
                    },
                    PatternType::Values(unit) => {
                        let mode = self.value_mode(*source)?;
                        PatternKind::Values {
                            unit,
                            period: *length as u32,
                            mode,
                            points: (0..*length)
                                .map(|i| {
                                    Ok(ValuePoint {
                                        step: i as u32,
                                        value: self.value_at(*source, *start as f64 + i as f64)?,
                                    })
                                })
                                .collect::<Result<_>>()?,
                        }
                    }
                };
                self.patterns.push(Pattern {
                    id: *id,
                    name: name.trim().into(),
                    kind,
                });
            }
            Edit::CreateInstrument { instrument } => {
                ensure!(
                    self.instrument(instrument.id).is_none(),
                    "Instrument #{} already exists",
                    instrument.id
                );
                self.instruments.push(instrument.clone());
            }
            Edit::UpdateInstrumentMeta { id, name, channel } => {
                let inst = self
                    .instrument_mut(*id)
                    .with_context(|| format!("Instrument #{id} does not exist"))?;
                if let Some(next_name) = name {
                    inst.name = next_name.trim().into();
                }
                if let Some(next_channel) = channel {
                    inst.channel = *next_channel;
                }
            }
            Edit::UpdateRackPatch { id, rack } => {
                let inst = self
                    .instrument_mut(*id)
                    .with_context(|| format!("Instrument #{id} does not exist"))?;
                inst.rack = rack.clone();
                self.prune_stale_instrument_refs(*id);
            }
            Edit::AddRackZone {
                instrument,
                zone,
                index,
            } => {
                let inst = self
                    .instrument_mut(*instrument)
                    .with_context(|| format!("Instrument #{instrument} does not exist"))?;
                ensure!(
                    inst.rack.zone(zone.id).is_none(),
                    "Zone z{} already exists on '{}'",
                    zone.id,
                    inst.name
                );
                ensure!(
                    inst.rack.zones.len() < MAX_ZONES,
                    "Rack '{}' already has the maximum {MAX_ZONES} zones",
                    inst.name
                );
                match index {
                    Some(at) => {
                        ensure!(
                            *at <= inst.rack.zones.len(),
                            "Zone index {at} is out of range (0..={})",
                            inst.rack.zones.len()
                        );
                        inst.rack.zones.insert(*at, zone.clone());
                    }
                    None => inst.rack.zones.push(zone.clone()),
                }
            }
            Edit::RemoveRackZone { instrument, zone } => {
                let inst = self
                    .instrument_mut(*instrument)
                    .with_context(|| format!("Instrument #{instrument} does not exist"))?;
                ensure!(
                    inst.rack.zones.len() > 1,
                    "Cannot remove the last zone from '{}'",
                    inst.name
                );
                let pos = inst
                    .rack
                    .zone_index(*zone)
                    .with_context(|| format!("Zone z{zone} does not exist on '{}'", inst.name))?;
                inst.rack.zones.remove(pos);
                self.prune_stale_instrument_refs(*instrument);
            }
            Edit::ReorderRackZone {
                instrument,
                zone,
                target_index,
            } => {
                let inst = self
                    .instrument_mut(*instrument)
                    .with_context(|| format!("Instrument #{instrument} does not exist"))?;
                let pos = inst
                    .rack
                    .zone_index(*zone)
                    .with_context(|| format!("Zone z{zone} does not exist on '{}'", inst.name))?;
                ensure!(
                    *target_index < inst.rack.zones.len(),
                    "Target zone index {target_index} is out of range (0..{})",
                    inst.rack.zones.len()
                );
                let item = inst.rack.zones.remove(pos);
                inst.rack.zones.insert(*target_index, item);
            }
            Edit::AddRackModule {
                instrument,
                zone,
                module,
            } => {
                let inst = self
                    .instrument_mut(*instrument)
                    .with_context(|| format!("Instrument #{instrument} does not exist"))?;
                let z = inst
                    .rack
                    .zone_mut(*zone)
                    .with_context(|| format!("Zone z{zone} does not exist"))?;
                ensure!(
                    z.module(module.id).is_none(),
                    "Module m{} already exists in zone '{}'",
                    module.id,
                    z.name
                );
                ensure!(
                    z.modules.len() < MAX_MODULES_PER_ZONE,
                    "Zone '{}' already has the maximum {MAX_MODULES_PER_ZONE} modules",
                    z.name
                );
                ensure!(
                    !matches!(module.kind, ModuleKind::Out(_)),
                    "Zone '{}' already has a mandatory Out module",
                    z.name
                );
                z.modules.push(module.clone());
            }
            Edit::RemoveRackModule {
                instrument,
                zone,
                module,
            } => {
                let inst = self
                    .instrument_mut(*instrument)
                    .with_context(|| format!("Instrument #{instrument} does not exist"))?;
                let z = inst
                    .rack
                    .zone_mut(*zone)
                    .with_context(|| format!("Zone z{zone} does not exist"))?;
                let pos = z
                    .modules
                    .iter()
                    .position(|m| m.id == *module)
                    .with_context(|| {
                        format!("Module m{module} does not exist in zone '{}'", z.name)
                    })?;
                ensure!(
                    !matches!(z.modules[pos].kind, ModuleKind::Out(_)),
                    "Cannot remove the mandatory Out module from zone '{}'",
                    z.name
                );
                z.modules.remove(pos);
                z.cables
                    .retain(|c| c.from_module != *module && c.to_module != *module);
                self.prune_stale_instrument_refs(*instrument);
            }
            Edit::RenameRackModule {
                instrument,
                zone,
                module,
                name,
            } => {
                let inst = self
                    .instrument_mut(*instrument)
                    .with_context(|| format!("Instrument #{instrument} does not exist"))?;
                let z = inst
                    .rack
                    .zone_mut(*zone)
                    .with_context(|| format!("Zone z{zone} does not exist"))?;
                let m = z
                    .module_mut(*module)
                    .with_context(|| format!("Module m{module} does not exist"))?;
                m.name = name.trim().into();
            }
            Edit::ConnectCable {
                instrument,
                zone,
                cable,
            } => {
                let inst = self
                    .instrument_mut(*instrument)
                    .with_context(|| format!("Instrument #{instrument} does not exist"))?;
                let z = inst
                    .rack
                    .zone_mut(*zone)
                    .with_context(|| format!("Zone z{zone} does not exist"))?;
                ensure!(
                    z.cable(cable.id).is_none(),
                    "Cable c{} already exists in zone '{}'",
                    cable.id,
                    z.name
                );
                ensure!(
                    z.cables.len() < MAX_CABLES_PER_ZONE,
                    "Zone '{}' already has the maximum {MAX_CABLES_PER_ZONE} patch cables",
                    z.name
                );
                z.cables.push(*cable);
            }
            Edit::DisconnectCable {
                instrument,
                zone,
                cable,
            } => {
                let inst = self
                    .instrument_mut(*instrument)
                    .with_context(|| format!("Instrument #{instrument} does not exist"))?;
                let z = inst
                    .rack
                    .zone_mut(*zone)
                    .with_context(|| format!("Zone z{zone} does not exist"))?;
                let pos = z
                    .cables
                    .iter()
                    .position(|c| c.id == *cable)
                    .with_context(|| {
                        format!("Cable c{cable} does not exist in zone '{}'", z.name)
                    })?;
                z.cables.remove(pos);
                self.prune_stale_instrument_refs(*instrument);
            }
            Edit::SetCableAmount {
                instrument,
                zone,
                cable,
                amount,
            } => {
                self.set_param(
                    ParamAddress::Cable {
                        instrument: *instrument,
                        zone: *zone,
                        cable: *cable,
                    },
                    *amount,
                )?;
            }
            Edit::PinPanelControl {
                instrument,
                pin,
                index,
            } => {
                let inst = self
                    .instrument_mut(*instrument)
                    .with_context(|| format!("Instrument #{instrument} does not exist"))?;
                ensure!(
                    inst.rack.contains_param(pin.target),
                    "Panel pin target does not exist on '{}'",
                    inst.name
                );
                let mut clean = pin.clone();
                clean.label = clean.label.trim().into();
                if let Some(existing) = inst
                    .rack
                    .panel_pins
                    .iter()
                    .position(|p| p.target == clean.target)
                {
                    inst.rack.panel_pins.remove(existing);
                }
                ensure!(
                    inst.rack.panel_pins.len() < MAX_PANEL_PINS,
                    "Rack '{}' already has the maximum {MAX_PANEL_PINS} panel pins",
                    inst.name
                );
                match index {
                    Some(at) => {
                        ensure!(
                            *at <= inst.rack.panel_pins.len(),
                            "Panel pin index {at} is out of range (0..={})",
                            inst.rack.panel_pins.len()
                        );
                        inst.rack.panel_pins.insert(*at, clean);
                    }
                    None => inst.rack.panel_pins.push(clean),
                }
            }
            Edit::UnpinPanelControl { instrument, index } => {
                let inst = self
                    .instrument_mut(*instrument)
                    .with_context(|| format!("Instrument #{instrument} does not exist"))?;
                ensure!(
                    *index < inst.rack.panel_pins.len(),
                    "Panel pin index {index} is out of range"
                );
                inst.rack.panel_pins.remove(*index);
            }
            Edit::RemoveInstrument { id } => {
                remove(&mut self.instruments, *id, |p| p.id, "Instrument")?;
            }
            Edit::PutPart { part } => upsert(&mut self.parts, part.clone(), |p| p.id),
            Edit::RemovePart { id } => {
                remove(&mut self.parts, *id, |p| p.id, "Part")?;
                for lane in &mut self.lanes {
                    lane.parts.retain(|entry| entry.part != *id);
                }
            }
            Edit::PutLane { lane } => upsert(&mut self.lanes, lane.clone(), |p| p.id),
            Edit::RemoveLane { id } => remove(&mut self.lanes, *id, |p| p.id, "Lane")?,
            Edit::ReorderLane { lane, target_index } => {
                let pos = self
                    .lanes
                    .iter()
                    .position(|l| l.id == *lane)
                    .with_context(|| format!("Lane #{lane} does not exist"))?;
                ensure!(
                    *target_index < self.lanes.len(),
                    "Target lane index {target_index} is out of range (0..{})",
                    self.lanes.len()
                );
                let item = self.lanes.remove(pos);
                self.lanes.insert(*target_index, item);
            }
            Edit::AddLanePart { lane, entry } => {
                let target = self.lane_mut(*lane).context("Lane is missing")?;
                if let Some(existing) = target.parts.iter_mut().find(|e| e.part == entry.part) {
                    *existing = *entry;
                } else {
                    target.parts.push(*entry);
                }
            }
            Edit::DropLanePart { lane, part } => {
                let target = self.lane_mut(*lane).context("Lane is missing")?;
                let before = target.parts.len();
                target.parts.retain(|e| e.part != *part);
                ensure!(
                    target.parts.len() != before,
                    "Part is not listed on this lane"
                );
            }
            Edit::PutPlacement { lane, placement } => {
                ensure!(self.lane(*lane).is_some(), "Lane is missing");
                for l in &mut self.lanes {
                    l.placements.retain(|p| p.id != placement.id);
                }
                let target = self.lane_mut(*lane).context("Lane is missing")?;
                target.placements.push(placement.clone());
                target.placements.sort_by_key(|p| (p.start_step, p.id));
                if !target.parts.iter().any(|e| e.part == placement.part) {
                    target.parts.push(LanePart {
                        part: placement.part,
                        phase: placement.phase,
                        offset_steps: placement.offset_steps,
                    });
                }
            }
            Edit::RemovePlacement { id } => {
                ensure!(self.placement(*id).is_some(), "Placement is missing");
                for lane in &mut self.lanes {
                    lane.placements.retain(|p| p.id != *id);
                }
            }
            Edit::SetException {
                placement,
                step,
                value,
            } => {
                let place = self
                    .placement_mut(*placement)
                    .context("Placement is missing")?;
                ensure!(*step < place.length_steps, "Exception is outside placement");
                place.exceptions.retain(|o| o.step != *step);
                if let Some(value) = value {
                    place.exceptions.push(Exception {
                        step: *step,
                        value: value.clone(),
                    });
                    place.exceptions.sort_by_key(|o| o.step);
                }
            }
            Edit::PutSection { section } => upsert(&mut self.sections, section.clone(), |p| p.id),
            Edit::RemoveSection { id } => remove(&mut self.sections, *id, |p| p.id, "Section")?,
            Edit::CreateChannel { channel } => {
                ensure!(
                    self.channel(channel.id).is_none(),
                    "Channel #{} already exists",
                    channel.id
                );
                let mut seen_devices =
                    std::collections::HashSet::with_capacity(channel.inserts.len());
                for dev in &channel.inserts {
                    ensure!(
                        self.device(dev.id).is_none() && seen_devices.insert(dev.id),
                        "Effect device #{} already exists",
                        dev.id
                    );
                }
                self.mixer.channels.push(channel.clone());
            }
            Edit::UpdateChannelMeta {
                id,
                name,
                output,
                sends,
            } => {
                let ch = self
                    .mixer
                    .channels
                    .iter_mut()
                    .find(|c| c.id == *id)
                    .with_context(|| format!("Channel #{id} does not exist"))?;
                if let Some(next_name) = name {
                    ch.name = next_name.trim().into();
                }
                if let Some(next_output) = output {
                    ch.output = *next_output;
                }
                if let Some(next_sends) = sends {
                    ch.sends = next_sends.clone();
                }
            }
            Edit::RemoveChannel { id } => {
                remove(&mut self.mixer.channels, *id, |p| p.id, "Channel")?;
            }
            Edit::AddEffect {
                channel,
                device,
                index,
            } => {
                ensure!(
                    self.device(device.id).is_none(),
                    "Effect device #{} already exists",
                    device.id
                );
                let inserts = self.channel_inserts_mut(*channel)?;
                match index {
                    Some(at) => {
                        ensure!(
                            *at <= inserts.len(),
                            "Effect slot index {at} is out of range (0..={})",
                            inserts.len()
                        );
                        inserts.insert(*at, device.clone());
                    }
                    None => inserts.push(device.clone()),
                }
            }
            Edit::RemoveEffect { device } => {
                ensure!(
                    self.remove_device_owned(*device).is_some(),
                    "Effect device #{device} does not exist"
                );
            }
            Edit::MoveEffect {
                device,
                target_channel,
                target_slot,
            } => {
                let dev = self
                    .remove_device_owned(*device)
                    .with_context(|| format!("Effect device #{device} does not exist"))?;
                let inserts = self.channel_inserts_mut(*target_channel)?;
                match target_slot {
                    Some(slot) => {
                        ensure!(
                            *slot <= inserts.len(),
                            "Effect slot index {slot} is out of range (0..={})",
                            inserts.len()
                        );
                        inserts.insert(*slot, dev);
                    }
                    None => inserts.push(dev),
                }
            }
            Edit::UpdateEffect {
                device,
                name,
                bypassed,
                kind,
            } => {
                let existing = self
                    .device_mut(*device)
                    .with_context(|| format!("Effect device #{device} does not exist"))?;
                if let Some(next_name) = name {
                    existing.name = next_name.trim().into();
                }
                if let Some(next_bypassed) = bypassed {
                    existing.bypassed = *next_bypassed;
                }
                if let Some(incoming_kind) = kind {
                    ensure!(
                        std::mem::discriminant(&existing.kind)
                            == std::mem::discriminant(incoming_kind),
                        "Cannot change effect '{}' (#{}) kind from {} to {}",
                        existing.name,
                        device,
                        existing.kind.label(),
                        incoming_kind.label()
                    );
                    existing.kind = *incoming_kind;
                }
            }
            Edit::SetParam { address, value } => self.set_param(*address, *value)?,
            Edit::PutModulation { modulation } => {
                upsert(&mut self.modulations, modulation.clone(), |p| p.id)
            }
            Edit::RemoveModulation { id } => {
                remove(&mut self.modulations, *id, |p| p.id, "Modulation")?
            }
            Edit::PutDuck { duck } => upsert(&mut self.ducks, duck.clone(), |p| p.id),
            Edit::RemoveDuck { id } => remove(&mut self.ducks, *id, |p| p.id, "Duck")?,
            Edit::SetTempo { bpm } => self.bpm = *bpm,
            Edit::SetKey { root_midi, scale } => {
                self.root_midi = *root_midi;
                self.scale = scale.clone();
            }
            Edit::SetBars { bars } => self.bars = *bars,
            Edit::SetEndRule { end } => self.end = *end,
            Edit::RenameProject { title } => self.title = title.trim().into(),
            Edit::SetLiveTiming { timing } => self.timing = *timing,
            Edit::SetMacroValue { index, value } => {
                ensure!(
                    usize::from(*index) < MAX_PROJECT_MACROS,
                    "Macro index {} exceeds 0..{MAX_PROJECT_MACROS}",
                    index
                );
                ensure!(
                    value.is_finite() && (0.0..=1.0).contains(value),
                    "Macro value must be finite and within 0..1"
                );
                if let Some(mac) = self.macro_control_mut(*index) {
                    mac.value = *value;
                } else {
                    let mut mac = MacroControl::new(*index, MacroControl::default_name(*index));
                    mac.value = *value;
                    self.macros.push(mac);
                    self.macros.sort_by_key(|m| m.index);
                }
            }
            Edit::PutMacro { macro_control } => {
                let mut mc = macro_control.clone();
                mc.name = mc.name.trim().into();
                if let Some(at) = self.macros.iter().position(|m| m.index == mc.index) {
                    self.macros[at] = mc;
                } else {
                    self.macros.push(mc);
                    self.macros.sort_by_key(|m| m.index);
                }
            }
            Edit::RemoveMacro { index } => {
                ensure!(
                    !self
                        .modulations
                        .iter()
                        .any(|m| m.target == ModulationTarget::Macro { index: *index }),
                    "Macro {} is driven by a modulation; remove the modulation first",
                    usize::from(*index) + 1
                );
                let before = self.macros.len();
                self.macros.retain(|m| m.index != *index);
                ensure!(
                    self.macros.len() != before,
                    "Macro {} does not exist",
                    usize::from(*index) + 1
                );
            }
        }
        Ok(())
    }

    fn set_param(&mut self, address: ParamAddress, value: f32) -> Result<()> {
        if let Some((instrument, param)) = address.as_instrument() {
            let inst = self
                .instrument_mut(instrument)
                .context("Instrument is missing")?;
            return inst.rack.write_param_native(param, value);
        }
        match address {
            ParamAddress::Channel { channel, param } => {
                let strip = match channel {
                    ChannelRef::Master => &mut self.mixer.master.strip,
                    ChannelRef::Channel(id) => {
                        &mut self
                            .mixer
                            .channels
                            .iter_mut()
                            .find(|c| c.id == id)
                            .context("Channel is missing")?
                            .strip
                    }
                };
                match param {
                    ChannelParam::Gain => {
                        ensure!(
                            value.is_finite()
                                && (0.0..=crate::dsp::MAX_CHANNEL_GAIN).contains(&value),
                            "Channel gain must be within 0..{}",
                            crate::dsp::MAX_CHANNEL_GAIN
                        );
                        strip.gain = value;
                    }
                    ChannelParam::Pan => {
                        ensure!(
                            value.is_finite() && (-1.0..=1.0).contains(&value),
                            "Channel pan must be within -1..1"
                        );
                        strip.pan = value;
                    }
                }
            }
            ParamAddress::Send { channel, target } => {
                ensure!(
                    value.is_finite() && (0.0..=1.0).contains(&value),
                    "Send amount must be within 0..1"
                );
                let ChannelRef::Channel(id) = channel else {
                    bail!("Only channels have sends");
                };
                let bus = self
                    .mixer
                    .channels
                    .iter_mut()
                    .find(|c| c.id == id)
                    .context("Channel is missing")?;
                if let Some(send) = bus.sends.iter_mut().find(|s| s.target == target) {
                    send.amount = value;
                } else {
                    bus.sends.push(crate::mixer::ChannelSend {
                        target,
                        amount: value,
                    });
                }
            }
            ParamAddress::Effect { device, param } => {
                let target = self
                    .device_mut(device)
                    .context("Effect device is missing")?;
                target.write_native(param, value)?;
            }
            ParamAddress::Rack { .. }
            | ParamAddress::Zone { .. }
            | ParamAddress::Module { .. }
            | ParamAddress::Cable { .. } => unreachable!(),
        }
        Ok(())
    }
}

fn upsert<T>(items: &mut Vec<T>, value: T, id: impl Fn(&T) -> Id) {
    if let Some(at) = items.iter().position(|item| id(item) == id(&value)) {
        items[at] = value;
    } else {
        items.push(value);
    }
}
fn remove<T>(items: &mut Vec<T>, value: Id, id: impl Fn(&T) -> Id, kind: &str) -> Result<()> {
    let before = items.len();
    items.retain(|item| id(item) != value);
    ensure!(items.len() != before, "{kind} does not exist");
    Ok(())
}
