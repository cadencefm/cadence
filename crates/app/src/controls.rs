//! Instrument, channel and effect control descriptors and edits.
use super::*;

#[derive(Clone, Debug)]
pub enum ControlTarget {
    Param(ParamAddress),
    Mute(ChannelRef),
    Solo(ChannelRef),
    Effect(Id),
    EffectChannel(Id),
    EffectSlot(Id),
    Bypass(Id),
    FilterEnvSource(Id),
    Instrument(Id),
    Pattern(Id),
    Macro(u8),
    ModulationLink {
        id: Id,
        pattern: Id,
    },
    PartParamLink {
        part: Id,
        index: usize,
        pattern: Id,
    },
    MacroBinding {
        index: u8,
        binding: usize,
    },
    Placement(Id),
    PlacementStart(Id),
    PlacementLength(Id),
    PlacementPhase(Id),
    PlacementOffset(Id),
    Part(Id),
    PartTranspose(Id),
    PartVelocity(Id),
    Output(Id),
    ChannelOutput(Id),
    Duck(Id),
    DuckDepth(Id),
    DuckRecovery(Id),
    SamplerSource {
        instrument: Id,
        zone: u16,
        module: u16,
    },
    Zone {
        instrument: Id,
        field: ZoneControl,
    },
    Text,
}

#[derive(Clone, Debug)]
pub struct Control {
    pub group: &'static str,
    pub label: String,
    pub display: String,
    pub normalized: Option<f32>,
    pub target: ControlTarget,
    pub controlled: bool,
}

impl App {
    pub fn channel(&self, id: Id) -> Option<&Channel> {
        self.doc
            .project()
            .mixer
            .channels
            .iter()
            .find(|c| c.id == id)
    }
    pub fn channel_name(&self, channel: ChannelRef) -> &str {
        match channel {
            ChannelRef::Master => "Master",
            ChannelRef::Channel(id) => self.channel(id).map_or("Missing", |c| c.name.as_str()),
        }
    }
    /// Channels in canonical console order: `Audio → Group → Return → Master`.
    pub fn channels(&self) -> Vec<ChannelRef> {
        let channels = &self.doc.project().mixer.channels;
        let mut out = Vec::with_capacity(channels.len() + 1);
        for kind in [ChannelKind::Audio, ChannelKind::Group, ChannelKind::Return] {
            out.extend(
                channels
                    .iter()
                    .filter(|c| c.kind == kind)
                    .map(|c| ChannelRef::Channel(c.id)),
            );
        }
        out.push(ChannelRef::Master);
        out
    }

    /// Single point of truth for all active/linked send targets on `channel_id` in canonical order:
    /// explicit `Channel.sends` first (`Some(amount)`), followed by synthetic Macro-bound and
    /// Modulation-only sends targeting existing Return buses (`None` → implicit `0%` base).
    pub(crate) fn channel_send_targets(&self, channel_id: Id) -> Vec<(Id, Option<f32>)> {
        let Some(ch) = self.channel(channel_id) else {
            return Vec::new();
        };
        if ch.kind == ChannelKind::Return {
            return Vec::new();
        }
        let ch_ref = ChannelRef::Channel(channel_id);
        let project = self.doc.project();
        let mut out: Vec<(Id, Option<f32>)> = ch
            .sends
            .iter()
            .map(|s| (s.target, Some(s.amount)))
            .collect();
        for mac in &project.macros {
            for binding in &mac.bindings {
                if let ParamAddress::Send { channel, target } = binding.target {
                    if channel == ch_ref
                        && !out.iter().any(|&(t, _)| t == target)
                        && self
                            .channel(target)
                            .is_some_and(|dst| dst.kind == ChannelKind::Return)
                    {
                        out.push((target, None));
                    }
                }
            }
        }
        for m in &project.modulations {
            if let ModulationTarget::Send { channel, target } = m.target {
                if channel == ch_ref
                    && !out.iter().any(|&(t, _)| t == target)
                    && self
                        .channel(target)
                        .is_some_and(|dst| dst.kind == ChannelKind::Return)
                {
                    out.push((target, None));
                }
            }
        }
        out
    }

    /// Available interactive vertical strip rows for `channel` in canonical order:
    /// `Fader → Pan → Inserts → Sends → Output → Ducks`.
    pub fn mixer_slots_for(&self, channel: ChannelRef) -> Vec<MixerSlot> {
        let mut slots = vec![MixerSlot::Fader, MixerSlot::Pan];
        let insert_count = match channel {
            ChannelRef::Master => self.doc.project().mixer.master.inserts.len(),
            ChannelRef::Channel(id) => self.channel(id).map_or(0, |c| c.inserts.len()),
        };
        for i in 0..insert_count {
            slots.push(MixerSlot::Insert(i));
        }
        if let ChannelRef::Channel(id) = channel {
            if let Some(ch) = self.channel(id) {
                for i in 0..self.channel_send_targets(id).len() {
                    slots.push(MixerSlot::Send(i));
                }
                if ch.kind != ChannelKind::Return {
                    slots.push(MixerSlot::Output);
                }
            }
        }
        let duck_count = self
            .doc
            .project()
            .ducks
            .iter()
            .filter(|d| d.target == channel)
            .count();
        for i in 0..duck_count {
            slots.push(MixerSlot::Duck(i));
        }
        slots
    }

    /// Clamp `self.view_state.mixer_slot` cleanly to `self.selected_channel`'s available rows.
    pub(crate) fn clamp_mixer_slot(&mut self) {
        let slots = self.mixer_slots_for(self.selected_channel);
        if slots.contains(&self.view_state.mixer_slot) {
            return;
        }
        self.view_state.mixer_slot = match self.view_state.mixer_slot {
            MixerSlot::Insert(_) => slots
                .iter()
                .rev()
                .copied()
                .find(|s| matches!(s, MixerSlot::Insert(_)))
                .unwrap_or(MixerSlot::Fader),
            MixerSlot::Send(_) => slots
                .iter()
                .rev()
                .copied()
                .find(|s| matches!(s, MixerSlot::Send(_)))
                .unwrap_or(MixerSlot::Fader),
            MixerSlot::Duck(_) => slots
                .iter()
                .rev()
                .copied()
                .find(|s| matches!(s, MixerSlot::Duck(_)))
                .unwrap_or(MixerSlot::Fader),
            _ => MixerSlot::Fader,
        };
    }

    /// Step vertical focus inside the selected Mixer strip (`j`/`k` in `NORMAL`).
    pub(crate) fn step_mixer_slot(&mut self, delta: isize, wrap: bool) {
        let slots = self.mixer_slots_for(self.selected_channel);
        if slots.is_empty() {
            self.view_state.mixer_slot = MixerSlot::Fader;
            return;
        }
        let cur = slots
            .iter()
            .position(|&s| s == self.view_state.mixer_slot)
            .unwrap_or(0) as isize;
        let next = if wrap {
            (cur + delta).rem_euclid(slots.len() as isize) as usize
        } else {
            (cur + delta).clamp(0, slots.len() as isize - 1) as usize
        };
        self.view_state.mixer_slot = slots[next];
        self.status = self.mixer_slot_status();
    }

    /// Human-readable status readout for the currently focused Mixer strip row.
    pub(crate) fn mixer_slot_status(&self) -> String {
        let ch = self.selected_channel;
        let name = self.channel_name(ch);
        match self.view_state.mixer_slot {
            MixerSlot::Fader => {
                let db = self.strip(ch).map_or_else(
                    || "0.0".into(),
                    |s| cadence_core::dsp::format_gain_db(s.gain),
                );
                format!("{name} · Fader {db} dB")
            }
            MixerSlot::Pan => {
                let pan = self
                    .strip(ch)
                    .map_or_else(|| "C".into(), |s| cadence_core::params::format_pan(s.pan));
                format!("{name} · Pan {pan}")
            }
            MixerSlot::Insert(idx) => {
                let inserts = match ch {
                    ChannelRef::Master => &self.doc.project().mixer.master.inserts,
                    ChannelRef::Channel(id) => {
                        let Some(c) = self.channel(id) else {
                            return format!("{name} · Insert {}", idx + 1);
                        };
                        &c.inserts
                    }
                };
                if let Some(dev) = inserts.get(idx) {
                    format!(
                        "{name} · FX {}: {}{}",
                        idx + 1,
                        dev.name,
                        if dev.bypassed { " (bypassed)" } else { "" }
                    )
                } else {
                    format!("{name} · FX {}", idx + 1)
                }
            }
            MixerSlot::Send(idx) => {
                if let ChannelRef::Channel(id) = ch {
                    if let Some((target, explicit)) =
                        self.channel_send_targets(id).get(idx).copied()
                    {
                        let dst = self.channel_name(ChannelRef::Channel(target));
                        let amt = explicit.unwrap_or(0.0);
                        let suffix = if explicit.is_none() { " (auto)" } else { "" };
                        return format!("{name} · Send → {dst} {:.0}%{suffix}", amt * 100.0);
                    }
                }
                format!("{name} · Send {}", idx + 1)
            }
            MixerSlot::Output => {
                let dst: String = match ch {
                    ChannelRef::Master => "DAC / WAV".into(),
                    ChannelRef::Channel(id) => self
                        .channel(id)
                        .map_or_else(|| "Master".into(), |c| self.channel_name(c.output).into()),
                };
                format!("{name} · Output → {dst}")
            }
            MixerSlot::Duck(idx) => {
                if let Some(duck) = self
                    .doc
                    .project()
                    .ducks
                    .iter()
                    .filter(|d| d.target == ch)
                    .nth(idx)
                {
                    format!(
                        "{name} · Duck {} {:.0}% / {:.0}ms",
                        duck.name,
                        duck.depth * 100.0,
                        duck.recovery_ms
                    )
                } else {
                    format!("{name} · Duck {}", idx + 1)
                }
            }
        }
    }

    pub(crate) fn mixer_focused_insert(&self, idx: usize) -> Option<Id> {
        match self.selected_channel {
            ChannelRef::Master => self
                .doc
                .project()
                .mixer
                .master
                .inserts
                .get(idx)
                .map(|d| d.id),
            ChannelRef::Channel(id) => self.channel(id)?.inserts.get(idx).map(|d| d.id),
        }
    }

    pub(crate) fn mixer_focused_send(&self, idx: usize) -> Option<(Id, Id)> {
        let ChannelRef::Channel(ch_id) = self.selected_channel else {
            return None;
        };
        let (target, _) = self.channel_send_targets(ch_id).get(idx).copied()?;
        Some((ch_id, target))
    }

    pub(crate) fn mixer_focused_duck(&self, idx: usize) -> Option<Id> {
        self.doc
            .project()
            .ducks
            .iter()
            .filter(|d| d.target == self.selected_channel)
            .nth(idx)
            .map(|d| d.id)
    }

    /// All project patterns eligible as a Filter rhythmic envelope trigger source,
    /// matching `Project::validate()` and `:param source` (all non-Degree patterns,
    /// including `period > 64` whose compiled envelope clamps cleanly to 64 steps).
    pub(crate) fn filter_env_pattern_candidates(&self) -> Vec<Id> {
        self.doc
            .project()
            .patterns
            .iter()
            .filter(|pat| {
                self.doc
                    .project()
                    .pattern_type(pat.id)
                    .is_ok_and(|pt| !matches!(pt, PatternType::Values(ValueUnit::Degrees)))
            })
            .map(|pat| pat.id)
            .collect()
    }

    pub(crate) fn set_filter_env_source(
        &mut self,
        device: Id,
        next: Option<Id>,
    ) -> Result<Outcome> {
        let edit = self.effect_edit(device, |d| match &mut d.kind {
            cadence_core::mixer::EffectKind::Filter(fp) => {
                fp.env_pattern_id = next;
                Ok(())
            }
            _ => bail!("Only Filter effects support a rhythmic envelope source"),
        })?;
        let label = match next {
            None => {
                let count = self.assets.filter_grooves.len();
                if count == 0 {
                    "Env Source → Groove Bank (empty · neutral pulse)".into()
                } else {
                    format!(
                        "Env Source → Groove Bank ({count} groove{})",
                        if count == 1 { "" } else { "s" }
                    )
                }
            }
            Some(pid) => format!(
                "Env Source → Pattern: {}",
                self.pattern(pid).map_or("?", |p| p.name.as_str())
            ),
        };
        let out = self.apply(Action::Edit { edit })?;
        self.status = label;
        Ok(out)
    }
    pub fn effect(&self, id: Id) -> Option<(ChannelRef, &EffectDevice)> {
        self.doc
            .project()
            .mixer
            .channels
            .iter()
            .find_map(|c| {
                c.inserts
                    .iter()
                    .find(|d| d.id == id)
                    .map(|d| (ChannelRef::Channel(c.id), d))
            })
            .or_else(|| {
                self.doc
                    .project()
                    .mixer
                    .master
                    .inserts
                    .iter()
                    .find(|d| d.id == id)
                    .map(|d| (ChannelRef::Master, d))
            })
    }
    pub(super) fn strip(&self, channel: ChannelRef) -> Option<cadence_core::mixer::ChannelStrip> {
        match channel {
            ChannelRef::Master => Some(self.doc.project().mixer.master.strip),
            ChannelRef::Channel(id) => self.channel(id).map(|c| c.strip),
        }
    }
    pub(super) fn effect_edit(
        &self,
        id: Id,
        edit: impl FnOnce(&mut EffectDevice) -> Result<()>,
    ) -> Result<Edit> {
        let (_, prev) = self.effect(id).context("Effect missing")?;
        let mut next = prev.clone();
        edit(&mut next)?;
        Ok(Edit::UpdateEffect {
            device: id,
            name: (next.name != prev.name).then_some(next.name),
            bypassed: (next.bypassed != prev.bypassed).then_some(next.bypassed),
            kind: (next.kind != prev.kind).then_some(next.kind),
        })
    }
    pub fn control_descriptors(&self) -> Vec<Control> {
        self.control_descriptors_for(self.detail)
    }
    /// The controls an object deck shows for its target.
    pub fn control_descriptors_for(&self, detail: Detail) -> Vec<Control> {
        let text = |group: &'static str, label: &str, display: String, target| Control {
            group,
            label: label.into(),
            display,
            normalized: None,
            target,
            controlled: false,
        };
        let mut result = Vec::new();
        match detail {
            Detail::Instrument(id) => {
                if let Some(instrument) = self.instrument(id) {
                    let rack = &instrument.rack;
                    let zone_idx = self.active_zone_index(rack);
                    let push_param =
                        |result: &mut Vec<Control>,
                         group: &'static str,
                         label: String,
                         target: InstrumentParam| {
                            let addr = ParamAddress::from_instrument(id, target);
                            let frames = match target {
                                InstrumentParam::Module { zone, module, .. } => {
                                    self.module_sample_frames(id, zone, module)
                                }
                                _ => 0,
                            };
                            let display = rack
                                .format_param(target, frames)
                                .unwrap_or_else(|| "—".into());
                            let normalized = rack.read_param_normalized(target, frames);
                            result.push(Control {
                                group,
                                label,
                                display,
                                normalized,
                                target: ControlTarget::Param(addr),
                                controlled: self.param_is_controlled(addr),
                            });
                        };
                    let push_module_controls =
                        |result: &mut Vec<Control>, zone: &RackZone, module_id: u16| {
                            let Some(m) = zone.module(module_id) else {
                                return;
                            };
                            let group = m.kind.tag().short_badge();
                            if let ModuleKind::Sampler(smp) = &m.kind {
                                let sample_display = smp
                                    .path
                                    .as_deref()
                                    .filter(|p| !p.trim().is_empty())
                                    .map(|p| {
                                        Path::new(p)
                                            .file_name()
                                            .and_then(|s| s.to_str())
                                            .unwrap_or(p)
                                            .to_string()
                                    })
                                    .unwrap_or_else(|| "(none · Enter to browse)".into());
                                result.push(text(
                                    "SOURCE",
                                    "Sample",
                                    sample_display,
                                    ControlTarget::SamplerSource {
                                        instrument: id,
                                        zone: zone.id,
                                        module: m.id,
                                    },
                                ));
                            }
                            for &param in m.kind.tag().params() {
                                push_param(
                                    result,
                                    group,
                                    param.label().into(),
                                    InstrumentParam::Module {
                                        zone: zone.id,
                                        module: m.id,
                                        param,
                                    },
                                );
                            }
                            for cid in crate::rack::module_attached_cables(zone, m.id) {
                                if let Some(c) = zone.cable(cid) {
                                    let from_name = zone.module(c.from_module).map_or_else(
                                        || format!("m{}", c.from_module),
                                        |x| x.name.clone(),
                                    );
                                    let to_name = zone.module(c.to_module).map_or_else(
                                        || format!("m{}", c.to_module),
                                        |x| x.name.clone(),
                                    );
                                    push_param(
                                        result,
                                        "PATCH",
                                        format!("{from_name} → {to_name}.{}", c.to_port.slug()),
                                        InstrumentParam::Cable {
                                            zone: zone.id,
                                            cable: cid,
                                        },
                                    );
                                }
                            }
                            if m.kind.tag() == ModuleKindTag::Out {
                                for param in ZoneParam::ALL {
                                    push_param(
                                        result,
                                        "VOICE",
                                        param.label().into(),
                                        InstrumentParam::Zone {
                                            zone: zone.id,
                                            param,
                                        },
                                    );
                                }
                            }
                        };

                    match self.rack_ui.tab {
                        RackTab::Panel => {
                            if let Some(zone) = rack.zones.get(zone_idx) {
                                match self.active_panel_pill(id) {
                                    Some((_, None)) => {
                                        for pin in &rack.panel_pins {
                                            if rack.contains_param(pin.target) {
                                                push_param(
                                                    &mut result,
                                                    "QUICK",
                                                    pin.display_label().into(),
                                                    pin.target,
                                                );
                                            }
                                        }
                                        for param in RackParam::ALL {
                                            let target = InstrumentParam::Rack { param };
                                            if !rack.panel_pins.iter().any(|p| p.target == target) {
                                                push_param(
                                                    &mut result,
                                                    "RACK",
                                                    param.label().into(),
                                                    target,
                                                );
                                            }
                                        }
                                    }
                                    Some((_, Some(mod_id))) => {
                                        push_module_controls(&mut result, zone, mod_id);
                                    }
                                    None => {}
                                }
                            }
                        }
                        RackTab::Patchbay => {
                            if let Some(zone) = rack.zones.get(zone_idx) {
                                if let Some(cid) = self.rack_ui.patchbay_cable_id {
                                    if let Some(c) = zone.cable(cid) {
                                        let from_name = zone.module(c.from_module).map_or_else(
                                            || format!("m{}", c.from_module),
                                            |x| x.name.clone(),
                                        );
                                        let to_name = zone.module(c.to_module).map_or_else(
                                            || format!("m{}", c.to_module),
                                            |x| x.name.clone(),
                                        );
                                        push_param(
                                            &mut result,
                                            "CABLE",
                                            format!("{from_name} → {to_name}.{}", c.to_port.slug()),
                                            InstrumentParam::Cable {
                                                zone: zone.id,
                                                cable: cid,
                                            },
                                        );
                                    }
                                }
                                if let Some(mod_id) = self.focused_patchbay_module_in(zone) {
                                    push_module_controls(&mut result, zone, mod_id);
                                }
                            }
                        }
                        RackTab::Zones => {
                            result.extend(self.zone_control_descriptors(id, rack));
                            for param in RackParam::ALL {
                                push_param(
                                    &mut result,
                                    "RACK",
                                    param.label().into(),
                                    InstrumentParam::Rack { param },
                                );
                            }
                        }
                    }
                    let mut inst_macros: Vec<&cadence_core::macros::MacroControl> =
                        self.doc.project().macros.iter().collect();
                    inst_macros.sort_by_key(|m| m.index);
                    for mac in inst_macros {
                        for (b_idx, binding) in mac.bindings.iter().enumerate() {
                            if let Some((inst_id, ip)) = binding.target.as_instrument() {
                                if inst_id == id {
                                    let param_lbl = self.format_instrument_param_short(id, ip);
                                    result.push(self.macro_binding_link_control(
                                        mac, b_idx, binding, &param_lbl,
                                    ));
                                }
                            }
                        }
                    }
                    for modulation in self.doc.project().modulations.iter().filter(|m| {
                        matches!(m.target, ModulationTarget::Instrument { instrument, .. } if instrument == id)
                    }) {
                        let param_lbl = match modulation.target {
                            ModulationTarget::Instrument { parameter, .. } => {
                                self.format_instrument_param_short(id, parameter)
                            }
                            _ => String::new(),
                        };
                        result.push(text(
                            "LINKS",
                            &format!("Mod · {} · {param_lbl}", modulation.name),
                            self.pattern(modulation.pattern)
                                .map_or("Missing", |p| p.name.as_str())
                                .into(),
                            ControlTarget::ModulationLink {
                                id: modulation.id,
                                pattern: modulation.pattern,
                            },
                        ));
                    }
                    for part in self
                        .doc
                        .project()
                        .parts
                        .iter()
                        .filter(|b| b.instrument == id)
                    {
                        for (idx, link) in part.parameter_links.iter().enumerate() {
                            result.push(text(
                                "LINKS",
                                &format!(
                                    "{} · {}",
                                    part.name,
                                    self.format_instrument_param_short(id, link.target)
                                ),
                                self.pattern(link.pattern)
                                    .map_or("Missing", |p| p.name.as_str())
                                    .into(),
                                ControlTarget::PartParamLink {
                                    part: part.id,
                                    index: idx,
                                    pattern: link.pattern,
                                },
                            ));
                        }
                    }
                    result.push(text(
                        "ROUTING",
                        "Output",
                        self.channel_name(ChannelRef::Channel(instrument.channel))
                            .into(),
                        ControlTarget::Output(instrument.channel),
                    ));
                }
            }
            Detail::Effect(id) => {
                if let Some((owner, device)) = self.effect(id) {
                    let is_filter =
                        matches!(device.kind, cadence_core::mixer::EffectKind::Filter(_));
                    let custom_pat_id = match device.kind {
                        cadence_core::mixer::EffectKind::Filter(p) => p.env_pattern_id,
                        _ => None,
                    };
                    for p in device.parameter_descriptors() {
                        let is_env_param = matches!(
                            p.param,
                            EffectParam::EnvPattern
                                | EffectParam::EnvAmount
                                | EffectParam::EnvDecay
                        );
                        if is_filter && p.param == EffectParam::EnvPattern {
                            let src_display = match custom_pat_id {
                                Some(pid) => self.pattern(pid).map_or_else(
                                    || format!("Pattern #{pid}"),
                                    |pat| format!("Pat: {}", pat.name),
                                ),
                                None => "Groove Bank".into(),
                            };
                            result.push(text(
                                "ENVELOPE",
                                "Source",
                                src_display,
                                ControlTarget::FilterEnvSource(id),
                            ));
                        }
                        let group = if is_env_param {
                            "ENVELOPE"
                        } else if is_filter {
                            "FILTER"
                        } else {
                            "EFFECT"
                        };
                        let label = p.param.label();
                        let formatted = self.format_effect_param_native(p.param, p.native_value);
                        let display =
                            if p.param == EffectParam::EnvPattern && custom_pat_id.is_some() {
                                format!("{formatted} (bank)")
                            } else {
                                formatted
                            };
                        let addr = ParamAddress::Effect {
                            device: id,
                            param: p.param,
                        };
                        result.push(Control {
                            group,
                            label: label.into(),
                            display,
                            normalized: Some(p.normalized),
                            target: ControlTarget::Param(addr),
                            controlled: self.param_is_controlled(addr),
                        });
                    }
                    result.push(text(
                        "STATE",
                        "Bypass",
                        if device.bypassed {
                            "Bypassed"
                        } else {
                            "Active"
                        }
                        .into(),
                        ControlTarget::Bypass(id),
                    ));
                    for modulation in self.doc.project().modulations.iter().filter(|m| {
                        matches!(m.target, ModulationTarget::Effect { device, .. } if device == id)
                    }) {
                        let param_label = match modulation.target {
                            ModulationTarget::Effect { parameter, .. } => parameter.label(),
                            _ => modulation.name.as_str(),
                        };
                        result.push(text(
                            "LINKS",
                            &format!("Mod · {} · {param_label}", modulation.name),
                            self.pattern(modulation.pattern)
                                .map_or("Missing", |p| p.name.as_str())
                                .into(),
                            ControlTarget::ModulationLink {
                                id: modulation.id,
                                pattern: modulation.pattern,
                            },
                        ));
                    }
                    let mut fx_macros: Vec<&cadence_core::macros::MacroControl> =
                        self.doc.project().macros.iter().collect();
                    fx_macros.sort_by_key(|m| m.index);
                    for mac in fx_macros {
                        for (b_idx, binding) in mac.bindings.iter().enumerate() {
                            if let ParamAddress::Effect { device, param } = binding.target {
                                if device == id {
                                    result.push(self.macro_binding_link_control(
                                        mac,
                                        b_idx,
                                        binding,
                                        param.label(),
                                    ));
                                }
                            }
                        }
                    }
                    let owner_inserts = match owner {
                        ChannelRef::Master => self.doc.project().mixer.master.inserts.as_slice(),
                        ChannelRef::Channel(cid) => {
                            self.channel(cid).map_or(&[][..], |c| c.inserts.as_slice())
                        }
                    };
                    let slot = owner_inserts.iter().position(|d| d.id == id).unwrap_or(0);
                    result.push(text(
                        "ROUTING",
                        "Channel",
                        self.channel_name(owner).into(),
                        ControlTarget::EffectChannel(id),
                    ));
                    result.push(text(
                        "ROUTING",
                        "Slot",
                        format!("{}/{}", slot + 1, owner_inserts.len().max(1)),
                        ControlTarget::EffectSlot(id),
                    ));
                }
            }
            Detail::Channel(channel) => {
                if let Some(strip) = self.strip(channel) {
                    for (param, label, display, norm) in [
                        (
                            ChannelParam::Gain,
                            "Trim",
                            cadence_core::dsp::format_gain_db(strip.gain),
                            ChannelParam::Gain.normalize(strip.gain),
                        ),
                        (
                            ChannelParam::Pan,
                            "Pan",
                            cadence_core::params::format_pan(strip.pan),
                            ChannelParam::Pan.normalize(strip.pan),
                        ),
                    ] {
                        let addr = ParamAddress::Channel { channel, param };
                        result.push(Control {
                            group: "STRIP",
                            label: label.into(),
                            display,
                            normalized: Some(norm),
                            target: ControlTarget::Param(addr),
                            controlled: self.param_is_controlled(addr),
                        });
                    }
                    result.push(text(
                        "STRIP",
                        "Mute",
                        if self.doc.flags().channel_muted(channel) {
                            "Muted"
                        } else {
                            "Off"
                        }
                        .into(),
                        ControlTarget::Mute(channel),
                    ));
                    if channel != ChannelRef::Master {
                        result.push(text(
                            "STRIP",
                            "Solo",
                            if self.doc.flags().channel_solo(channel) {
                                "Solo"
                            } else {
                                "Off"
                            }
                            .into(),
                            ControlTarget::Solo(channel),
                        ));
                    }
                    if let ChannelRef::Channel(id) = channel {
                        if let Some(c) = self.channel(id) {
                            let output_target = if c.kind == ChannelKind::Return {
                                ControlTarget::Text
                            } else {
                                ControlTarget::ChannelOutput(id)
                            };
                            result.push(text(
                                "ROUTING",
                                "Output",
                                self.channel_name(c.output).into(),
                                output_target,
                            ));
                            for (target, explicit) in self.channel_send_targets(id) {
                                let addr = ParamAddress::Send { channel, target };
                                let amt = explicit.unwrap_or(0.0);
                                result.push(Control {
                                    group: "SENDS",
                                    label: format!(
                                        "Send → {}",
                                        self.channel_name(ChannelRef::Channel(target))
                                    ),
                                    display: format!("{:.0}%", amt * 100.0),
                                    normalized: Some(amt),
                                    target: ControlTarget::Param(addr),
                                    controlled: self.param_is_controlled(addr),
                                });
                            }
                        }
                    }
                    let effects = match channel {
                        ChannelRef::Master => self.doc.project().mixer.master.inserts.as_slice(),
                        ChannelRef::Channel(id) => {
                            self.channel(id).map_or(&[][..], |c| c.inserts.as_slice())
                        }
                    };
                    for (slot, d) in effects.iter().enumerate() {
                        result.push(text(
                            "INSERTS",
                            &format!("Slot {} · #{}", slot + 1, d.id),
                            format!("{}{}", d.name, if d.bypassed { " · bypassed" } else { "" }),
                            ControlTarget::Effect(d.id),
                        ));
                    }
                    for m in self.doc.project().modulations.iter().filter(|m| {
                        matches!(
                            m.target,
                            ModulationTarget::ChannelGain { channel: c }
                                | ModulationTarget::ChannelPan { channel: c }
                                | ModulationTarget::Send { channel: c, .. }
                                if c == channel
                        )
                    }) {
                        let label = match m.target {
                            ModulationTarget::ChannelPan { .. } => "Pan shape".to_string(),
                            ModulationTarget::Send { target, .. } => {
                                format!("Send → {}", self.channel_name(ChannelRef::Channel(target)))
                            }
                            _ => "Level shape".to_string(),
                        };
                        result.push(text(
                            "MODULATION",
                            &label,
                            self.pattern(m.pattern)
                                .map_or("Missing", |p| p.name.as_str())
                                .into(),
                            ControlTarget::ModulationLink {
                                id: m.id,
                                pattern: m.pattern,
                            },
                        ));
                    }
                    let mut ch_macros: Vec<&cadence_core::macros::MacroControl> =
                        self.doc.project().macros.iter().collect();
                    ch_macros.sort_by_key(|m| m.index);
                    for mac in ch_macros {
                        for (b_idx, binding) in mac.bindings.iter().enumerate() {
                            let param_label = match binding.target {
                                ParamAddress::Channel { channel: c, param } if c == channel => {
                                    Some(param.label().to_string())
                                }
                                ParamAddress::Send { channel: c, target } if c == channel => {
                                    Some(format!(
                                        "Send → {}",
                                        self.channel_name(ChannelRef::Channel(target))
                                    ))
                                }
                                _ => None,
                            };
                            if let Some(param_label) = param_label {
                                result.push(self.macro_binding_link_control(
                                    mac,
                                    b_idx,
                                    binding,
                                    &param_label,
                                ));
                            }
                        }
                    }
                    for d in self
                        .doc
                        .project()
                        .ducks
                        .iter()
                        .filter(|d| d.target == channel)
                    {
                        let src_name = self
                            .part(d.source_part)
                            .map_or("Missing", |p| p.name.as_str());
                        result.push(text(
                            "DUCK",
                            &format!("{} ← {src_name}", d.name),
                            format!("{:.0}% / {:.0}ms", d.depth * 100.0, d.recovery_ms),
                            ControlTarget::Duck(d.id),
                        ));
                        result.push(Control {
                            group: "DUCK",
                            label: format!("{} Depth", d.name),
                            display: format!("{:.0}%", d.depth * 100.0),
                            normalized: Some(d.depth.clamp(0.0, 1.0)),
                            target: ControlTarget::DuckDepth(d.id),
                            controlled: false,
                        });
                        result.push(Control {
                            group: "DUCK",
                            label: format!("{} Recovery", d.name),
                            display: format!("{:.0}ms", d.recovery_ms),
                            normalized: Some(cadence_core::mixer::normalize_duck_recovery_ms(
                                d.recovery_ms,
                            )),
                            target: ControlTarget::DuckRecovery(d.id),
                            controlled: false,
                        });
                    }
                }
            }
            Detail::Part(id) => {
                if let Some(b) = self.part(id) {
                    result.push(text(
                        "PART",
                        "Pattern",
                        self.pattern(b.pattern)
                            .map_or("Missing", |p| p.name.as_str())
                            .into(),
                        ControlTarget::Pattern(b.pattern),
                    ));
                    result.push(text(
                        "PART",
                        "Instrument",
                        self.instrument(b.instrument)
                            .map_or("Missing", |i| i.name.as_str())
                            .into(),
                        ControlTarget::Instrument(b.instrument),
                    ));
                    result.push(text(
                        "PERFORMANCE",
                        "Transpose",
                        format!("{:+}st", b.transpose),
                        ControlTarget::PartTranspose(id),
                    ));
                    result.push(text(
                        "PERFORMANCE",
                        "Velocity",
                        format!("{:.0}%", b.velocity_scale * 100.0),
                        ControlTarget::PartVelocity(id),
                    ));
                    for (idx, link) in b.parameter_links.iter().enumerate() {
                        result.push(text(
                            "LINKS",
                            &format!(
                                "Per-note {}",
                                self.format_instrument_param_short(b.instrument, link.target)
                            ),
                            self.pattern(link.pattern)
                                .map_or("Missing", |p| p.name.as_str())
                                .into(),
                            ControlTarget::PartParamLink {
                                part: b.id,
                                index: idx,
                                pattern: link.pattern,
                            },
                        ));
                    }
                }
            }
            Detail::Placement(id) => {
                if let Some((_, p)) = self.placement(id) {
                    if let Some(b) = self.part(p.part) {
                        result.push(text(
                            "LINKS",
                            "Part",
                            b.name.clone(),
                            ControlTarget::Part(b.id),
                        ));
                        result.push(text(
                            "LINKS",
                            "Pattern",
                            self.pattern(b.pattern)
                                .map_or("Missing", |p| p.name.as_str())
                                .into(),
                            ControlTarget::Pattern(b.pattern),
                        ));
                        result.push(text(
                            "LINKS",
                            "Instrument",
                            self.instrument(b.instrument)
                                .map_or("Missing", |i| i.name.as_str())
                                .into(),
                            ControlTarget::Instrument(b.instrument),
                        ));
                    }
                    result.push(text(
                        "TIMING",
                        "Starts",
                        format!(
                            "bar {} · step {}",
                            p.start_step / 16 + 1,
                            p.start_step % 16 + 1
                        ),
                        ControlTarget::PlacementStart(id),
                    ));
                    result.push(text(
                        "TIMING",
                        "Length",
                        format!("{} steps", p.length_steps),
                        ControlTarget::PlacementLength(id),
                    ));
                    result.push(text(
                        "TIMING",
                        "Phase",
                        match p.phase {
                            PhaseMode::Song => "Song",
                            PhaseMode::Restart => "Restart",
                        }
                        .into(),
                        ControlTarget::PlacementPhase(id),
                    ));
                    result.push(text(
                        "TIMING",
                        "Offset",
                        format!("{:+} steps", p.offset_steps),
                        ControlTarget::PlacementOffset(id),
                    ));
                    result.push(text(
                        "TIMING",
                        "Exceptions",
                        p.exceptions.len().to_string(),
                        ControlTarget::Placement(id),
                    ));
                }
            }
            Detail::None => {}
        }
        result
    }
    pub(super) fn set_parameter_text(&mut self, name: &str, value: &str) -> Result<Outcome> {
        if let Some(address) = self.parse_dotted_zone_param(name, false) {
            let native = self.parse_param_native(address, value)?;
            return self.apply(Action::SetParam {
                address,
                value: native,
            });
        }
        let target_detail = if self.deck_active() {
            self.detail
        } else if self.mixer_canvas_focused() {
            Detail::Channel(self.selected_channel)
        } else {
            bail!("Open the instrument, effect or channel deck before entering its parameter");
        };
        match target_detail {
            Detail::Instrument(id) => {
                let instrument = self.instrument(id).context("Instrument missing")?;
                let parsed = InstrumentParam::parse_for_rack(name, &instrument.rack, false)
                    .context(
                        "Unknown Rack parameter (use pin label, MODULE.PARAM, ZONE.MODULE.PARAM, cN, or rack gain|pan|tune|cutoff|resonance)",
                    )?;
                let address = ParamAddress::from_instrument(id, parsed);
                let native = self.parse_param_native(address, value)?;
                self.apply(Action::SetParam {
                    address,
                    value: native,
                })
            }
            Detail::Effect(id) => {
                if matches!(name, "source" | "env_source" | "trigger") {
                    let trimmed = value.trim();
                    let lower = trimmed.to_ascii_lowercase();
                    let next_pat = if matches!(lower.as_str(), "factory" | "bank" | "none" | "off")
                    {
                        None
                    } else if let Ok(pid) = trimmed.parse::<u64>() {
                        ensure!(self.pattern(pid).is_some(), "Pattern #{pid} not found");
                        Some(pid)
                    } else if let Some(pat) = self
                        .doc
                        .project()
                        .patterns
                        .iter()
                        .find(|p| p.name.eq_ignore_ascii_case(trimmed))
                        .or_else(|| {
                            self.doc
                                .project()
                                .patterns
                                .iter()
                                .find(|p| p.name.to_ascii_lowercase().contains(&lower))
                        })
                    {
                        Some(pat.id)
                    } else {
                        bail!("Source expects 'factory' or a project pattern name/id");
                    };
                    return self.set_filter_env_source(id, next_pat);
                }
                let (_, dev) = self.effect(id).context("Effect missing")?;
                let param = dev.parse_param(name).context("Unknown effect parameter")?;
                let address = ParamAddress::Effect { device: id, param };
                let native = self.parse_param_native(address, value)?;
                self.apply(Action::SetParam {
                    address,
                    value: native,
                })
            }
            Detail::Channel(channel) => {
                let lower = name.trim().to_ascii_lowercase();
                if let Some(target_raw) = lower.strip_prefix("send:") {
                    let target = self.channel_id(target_raw)?;
                    let address = ParamAddress::Send { channel, target };
                    let native = self.parse_param_native(address, value)?;
                    return self.apply(Action::SetParam {
                        address,
                        value: native,
                    });
                }
                if lower == "send" {
                    let mut parts = value.split_whitespace();
                    let target_raw = parts
                        .next()
                        .context("Usage: :param send <RETURN> <AMOUNT>")?;
                    let amount_raw = parts
                        .next()
                        .context("Usage: :param send <RETURN> <AMOUNT>")?;
                    ensure!(
                        parts.next().is_none(),
                        "Usage: :param send <RETURN> <AMOUNT>"
                    );
                    let target = self.channel_id(target_raw)?;
                    let address = ParamAddress::Send { channel, target };
                    let native = self.parse_param_native(address, amount_raw)?;
                    return self.apply(Action::SetParam {
                        address,
                        value: native,
                    });
                }
                let param = param_ops::parse_channel_param(name)
                    .context("Channel parameters: gain|level|trim, pan, send:<RETURN>")?;
                let address = ParamAddress::Channel { channel, param };
                let native = self.parse_param_native(address, value)?;
                self.apply(Action::SetParam {
                    address,
                    value: native,
                })
            }
            _ => bail!("Open an instrument, effect, or channel before entering its parameter"),
        }
    }
    fn macro_binding_link_control(
        &self,
        mac: &cadence_core::macros::MacroControl,
        binding_idx: usize,
        binding: &cadence_core::macros::MacroBinding,
        param_label: &str,
    ) -> Control {
        let arrow = if binding.is_inverted() { "↓" } else { "→" };
        let min_s = self.format_param_normalized_value(binding.target, binding.min);
        let max_s = self.format_param_normalized_value(binding.target, binding.max);
        Control {
            group: "LINKS",
            label: format!(
                "M{} {} · {param_label}",
                usize::from(mac.index) + 1,
                mac.name
            ),
            display: format!("{min_s} {arrow} {max_s}"),
            normalized: None,
            target: ControlTarget::MacroBinding {
                index: mac.index,
                binding: binding_idx,
            },
            controlled: false,
        }
    }

    pub(super) fn jump_control_group(&mut self, direction: i8) {
        let controls = self.control_descriptors();
        if controls.is_empty() {
            return;
        }
        let groups: Vec<&str> = controls.iter().map(|c| c.group).collect();
        let cur = self.control_cursor.min(controls.len() - 1);
        let has_multiple_groups = groups.windows(2).any(|w| w[0] != w[1]);
        if has_multiple_groups {
            if let Some(next) = crate::jump_group_index(&groups, cur, direction >= 0) {
                self.control_cursor = next;
            }
        } else if direction >= 0 {
            self.control_cursor = (cur + 4).min(controls.len() - 1);
        } else {
            self.control_cursor = cur.saturating_sub(4);
        }
        if let Some(ctrl) = controls.get(self.control_cursor) {
            self.status = format!("{} · {}", ctrl.group, ctrl.label);
        }
    }
    pub(crate) fn selected_control(&self) -> Option<Control> {
        self.control_descriptors().get(self.control_cursor).cloned()
    }
    pub(super) fn nudge_control(
        &mut self,
        direction: i8,
        coarse: bool,
        coalesce: bool,
    ) -> Result<Outcome> {
        let Some(control) = self.control_descriptors().get(self.control_cursor).cloned() else {
            return Ok(Outcome::default());
        };
        match control.target {
            ControlTarget::PartTranspose(id) | ControlTarget::PartVelocity(id) => {
                let mut part = self.part(id).context("Part missing")?.clone();
                let (key, label) = if matches!(control.target, ControlTarget::PartTranspose(_)) {
                    part.transpose = (part.transpose
                        + i16::from(direction) * if coarse { 12 } else { 1 })
                    .clamp(-96, 96);
                    (
                        document::CoalesceKey::PartTranspose(id),
                        format!("{} transpose → {:+}st", part.name, part.transpose),
                    )
                } else {
                    let step_pct = if coarse { 5.0 } else { 1.0 };
                    part.velocity_scale = (((part.velocity_scale * 100.0).round()
                        + f32::from(direction) * step_pct)
                        / 100.0)
                        .clamp(0.0, 1.0);
                    (
                        document::CoalesceKey::PartVelocity(id),
                        format!(
                            "{} velocity → {:.0}%",
                            part.name,
                            part.velocity_scale * 100.0
                        ),
                    )
                };
                self.commit_coalesced(vec![Edit::PutPart { part }], label, key, coalesce)
            }
            ControlTarget::Param(address) => self.nudge_param(address, direction, coarse, coalesce),
            ControlTarget::Mute(channel) => self.apply(Action::SetChannelMuted {
                channel,
                muted: !self.doc.flags().channel_muted(channel),
            }),
            ControlTarget::Solo(channel) => self.apply(Action::SetChannelSolo {
                channel,
                solo: !self.doc.flags().channel_solo(channel),
            }),
            ControlTarget::Bypass(id) | ControlTarget::Effect(id) => {
                self.apply(Action::SetEffectBypass {
                    device: id,
                    bypassed: !self.effect(id).context("Effect missing")?.1.bypassed,
                })
            }
            ControlTarget::FilterEnvSource(id) => {
                let current = match self.effect(id).context("Effect missing")?.1.kind {
                    cadence_core::mixer::EffectKind::Filter(p) => p.env_pattern_id,
                    _ => return Ok(Outcome::default()),
                };
                let mut candidates: Vec<Option<Id>> = vec![None];
                candidates.extend(self.filter_env_pattern_candidates().into_iter().map(Some));
                let pos = candidates.iter().position(|&c| c == current).unwrap_or(0) as isize;
                let step = if direction >= 0 { 1 } else { -1 };
                let next_idx = (pos + step).rem_euclid(candidates.len() as isize) as usize;
                self.set_filter_env_source(id, candidates[next_idx])
            }
            ControlTarget::ChannelOutput(id) => self.cycle_channel_output(id, direction),
            ControlTarget::Zone { instrument, field } => {
                self.nudge_zone_control(instrument, field, direction, coarse, coalesce)
            }
            ControlTarget::PlacementStart(id) => {
                let shift = i64::from(direction) * if coarse { 16 } else { 4 };
                self.reshape_placement(id, shift, 0, coalesce)
            }
            ControlTarget::PlacementLength(id) => {
                let resize = i64::from(direction) * if coarse { 16 } else { 4 };
                self.reshape_placement(id, 0, resize, coalesce)
            }
            ControlTarget::PlacementPhase(id) => {
                let (lane, pl) = self.placement(id).context("Placement missing")?;
                let lane_id = lane.id;
                let mut updated = pl.clone();
                updated.phase = match updated.phase {
                    PhaseMode::Song => PhaseMode::Restart,
                    PhaseMode::Restart => PhaseMode::Song,
                };
                let label = format!(
                    "Placement phase → {}",
                    match updated.phase {
                        PhaseMode::Song => "Song",
                        PhaseMode::Restart => "Restart",
                    }
                );
                self.commit(
                    vec![Edit::PutPlacement {
                        lane: lane_id,
                        placement: updated,
                    }],
                    label,
                )
            }
            ControlTarget::PlacementOffset(id) => {
                let (lane, pl) = self.placement(id).context("Placement missing")?;
                let lane_id = lane.id;
                let mut updated = pl.clone();
                let step = i32::from(direction) * if coarse { 16 } else { 1 };
                updated.offset_steps = (updated.offset_steps + step).clamp(-1_048_576, 1_048_576);
                let label = format!("Placement offset → {:+} steps", updated.offset_steps);
                self.commit_coalesced(
                    vec![Edit::PutPlacement {
                        lane: lane_id,
                        placement: updated,
                    }],
                    label,
                    document::CoalesceKey::PlacementOffset(id),
                    coalesce,
                )
            }
            ControlTarget::EffectChannel(id) => {
                let (owner, _) = self.effect(id).context("Effect missing")?;
                let candidates: Vec<ChannelRef> = self
                    .channels()
                    .into_iter()
                    .filter(|&c| {
                        c == owner || {
                            let len = match c {
                                ChannelRef::Master => self.doc.project().mixer.master.inserts.len(),
                                ChannelRef::Channel(cid) => {
                                    self.channel(cid).map_or(0, |ch| ch.inserts.len())
                                }
                            };
                            len < cadence_core::mixer::MAX_INSERTS
                        }
                    })
                    .collect();
                if candidates.len() <= 1 {
                    return Ok(Outcome::default());
                }
                let pos = candidates.iter().position(|&c| c == owner).unwrap_or(0) as isize;
                let step = if direction >= 0 { 1 } else { -1 };
                let next_channel =
                    candidates[(pos + step).rem_euclid(candidates.len() as isize) as usize];
                let position = match next_channel {
                    ChannelRef::Master => self.doc.project().mixer.master.inserts.len(),
                    ChannelRef::Channel(cid) => self.channel(cid).map_or(0, |ch| ch.inserts.len()),
                };
                let out = self.apply(Action::MoveEffect {
                    device: id,
                    channel: next_channel,
                    position,
                })?;
                self.selected_channel = next_channel;
                Ok(out)
            }
            ControlTarget::EffectSlot(id) => {
                self.reorder_effect_in_channel(id, if direction >= 0 { 1 } else { -1 })
            }
            ControlTarget::DuckDepth(id) => self.nudge_duck_depth(id, direction, coarse, coalesce),
            ControlTarget::DuckRecovery(id) => {
                let mut duck = self
                    .doc
                    .project()
                    .ducks
                    .iter()
                    .find(|d| d.id == id)
                    .context("Duck missing")?
                    .clone();
                let step_ms = if coarse { 50.0 } else { 5.0 };
                duck.recovery_ms = (duck.recovery_ms.round() + f32::from(direction) * step_ms)
                    .clamp(
                        cadence_core::mixer::DUCK_RECOVERY_MIN_MS,
                        cadence_core::mixer::DUCK_RECOVERY_MAX_MS,
                    );
                let label = format!("{} recovery → {:.0}ms", duck.name, duck.recovery_ms);
                self.commit_coalesced(
                    vec![Edit::PutDuck { duck }],
                    label,
                    document::CoalesceKey::DuckRecovery(id),
                    coalesce,
                )
            }
            _ => Ok(Outcome::default()),
        }
    }

    pub(super) fn reorder_effect_in_channel(
        &mut self,
        device: Id,
        delta: isize,
    ) -> Result<Outcome> {
        let (owner, _) = self.effect(device).context("Effect missing")?;
        let inserts = match owner {
            ChannelRef::Master => &self.doc.project().mixer.master.inserts,
            ChannelRef::Channel(cid) => &self.channel(cid).context("Channel missing")?.inserts,
        };
        let Some(pos) = inserts.iter().position(|d| d.id == device) else {
            return Ok(Outcome::default());
        };
        let max_idx = inserts.len().saturating_sub(1) as isize;
        let next = (pos as isize + delta).clamp(0, max_idx) as usize;
        if next == pos {
            self.status = if delta < 0 {
                "Effect is already first in chain".into()
            } else {
                "Effect is already last in chain".into()
            };
            return Ok(Outcome::default());
        }
        let out = self.apply(Action::MoveEffect {
            device,
            channel: owner,
            position: next,
        })?;
        if matches!(self.detail, Detail::Channel(_)) {
            if let Some(new_idx) = self
                .control_descriptors()
                .iter()
                .position(|c| matches!(c.target, ControlTarget::Effect(id) if id == device))
            {
                self.control_cursor = new_idx;
            }
        }
        if self.mixer_canvas_focused() && self.selected_channel == owner {
            self.view_state.mixer_slot = MixerSlot::Insert(next);
        }
        Ok(out)
    }

    pub(super) fn cycle_channel_output(&mut self, id: Id, direction: i8) -> Result<Outcome> {
        let Some(ch) = self.channel(id) else {
            return Ok(Outcome::default());
        };
        let current = ch.output;
        let targets =
            cadence_core::mixer::valid_output_targets(&self.doc.project().mixer.channels, id);
        if targets.len() <= 1 {
            return Ok(Outcome::default());
        }
        let pos = targets.iter().position(|&t| t == current).unwrap_or(0) as isize;
        let step = if direction >= 0 { 1 } else { -1 };
        let next_idx = (pos + step).rem_euclid(targets.len() as isize) as usize;
        let output = targets[next_idx];
        self.apply(Action::SetChannelOutput {
            channel: id,
            output,
        })
    }

    pub(super) fn nudge_duck_depth(
        &mut self,
        id: Id,
        direction: i8,
        coarse: bool,
        coalesce: bool,
    ) -> Result<Outcome> {
        let mut duck = self
            .doc
            .project()
            .ducks
            .iter()
            .find(|d| d.id == id)
            .context("Duck missing")?
            .clone();
        let step_pct = if coarse { 10.0 } else { 1.0 };
        duck.depth = (((duck.depth * 100.0).round() + f32::from(direction) * step_pct) / 100.0)
            .clamp(0.0, 1.0);
        let label = format!("{} depth → {:.0}%", duck.name, duck.depth * 100.0);
        self.commit_coalesced(
            vec![Edit::PutDuck { duck }],
            label,
            document::CoalesceKey::DuckDepth(id),
            coalesce,
        )
    }

    /// Reset the currently focused object-deck control to its default value (`Alt+Backspace`).
    pub(super) fn reset_focused_control(&mut self) -> Result<Outcome> {
        let Some(control) = self.control_descriptors().get(self.control_cursor).cloned() else {
            return Ok(Outcome::default());
        };
        match control.target {
            ControlTarget::Param(address) => self.reset_param_default(address),
            ControlTarget::PartTranspose(id) => {
                let mut part = self.part(id).context("Part missing")?.clone();
                part.transpose = 0;
                let label = format!("Reset {} transpose → 0st", part.name);
                self.commit(vec![Edit::PutPart { part }], label)
            }
            ControlTarget::PartVelocity(id) => {
                let mut part = self.part(id).context("Part missing")?.clone();
                part.velocity_scale = 1.0;
                let label = format!("Reset {} velocity → 100%", part.name);
                self.commit(vec![Edit::PutPart { part }], label)
            }
            ControlTarget::PlacementOffset(id) => {
                let (lane, pl) = self.placement(id).context("Placement missing")?;
                let lane_id = lane.id;
                let mut updated = pl.clone();
                updated.offset_steps = 0;
                self.commit(
                    vec![Edit::PutPlacement {
                        lane: lane_id,
                        placement: updated,
                    }],
                    "Reset placement offset → 0 steps".into(),
                )
            }
            ControlTarget::PlacementPhase(id) => {
                let (lane, pl) = self.placement(id).context("Placement missing")?;
                let lane_id = lane.id;
                let mut updated = pl.clone();
                updated.phase = PhaseMode::Song;
                self.commit(
                    vec![Edit::PutPlacement {
                        lane: lane_id,
                        placement: updated,
                    }],
                    "Reset placement phase → Song".into(),
                )
            }
            ControlTarget::Mute(channel) => self.apply(Action::SetChannelMuted {
                channel,
                muted: false,
            }),
            ControlTarget::Solo(channel) => self.apply(Action::SetChannelSolo {
                channel,
                solo: false,
            }),
            ControlTarget::Bypass(id) | ControlTarget::Effect(id) => {
                self.apply(Action::SetEffectBypass {
                    device: id,
                    bypassed: false,
                })
            }
            ControlTarget::DuckDepth(id) => {
                let mut duck = self
                    .doc
                    .project()
                    .ducks
                    .iter()
                    .find(|d| d.id == id)
                    .context("Duck missing")?
                    .clone();
                duck.depth = 0.0;
                let label = format!("Reset {} depth → 0%", duck.name);
                self.commit(vec![Edit::PutDuck { duck }], label)
            }
            ControlTarget::DuckRecovery(id) => {
                let mut duck = self
                    .doc
                    .project()
                    .ducks
                    .iter()
                    .find(|d| d.id == id)
                    .context("Duck missing")?
                    .clone();
                duck.recovery_ms = 120.0;
                let label = format!("Reset {} recovery → 120ms", duck.name);
                self.commit(vec![Edit::PutDuck { duck }], label)
            }
            ControlTarget::Zone { instrument, field } => {
                let mut rack = self
                    .instrument(instrument)
                    .context("Instrument missing")?
                    .rack
                    .clone();
                if rack.zones.is_empty() {
                    return Ok(Outcome::default());
                }
                let zone_idx = rack.clamp_zone_index(self.rack_ui.zone_cursor);
                let zone = &mut rack.zones[zone_idx];
                let label = match field {
                    ZoneControl::LowMidi => {
                        zone.set_low_midi(0);
                        format!("Reset {} low key → C-1", zone.name)
                    }
                    ZoneControl::HighMidi => {
                        zone.set_high_midi(127);
                        format!("Reset {} high key → G9", zone.name)
                    }
                    ZoneControl::LowVel => {
                        zone.set_low_vel(0.0);
                        format!("Reset {} low vel → 0%", zone.name)
                    }
                    ZoneControl::HighVel => {
                        zone.set_high_vel(1.0);
                        format!("Reset {} high vel → 100%", zone.name)
                    }
                    ZoneControl::ChokeGroup => {
                        zone.set_choke_group(None)?;
                        format!("Reset {} choke group → None", zone.name)
                    }
                    ZoneControl::Name | ZoneControl::SamplePath => {
                        return Ok(Outcome::default());
                    }
                };
                self.commit(
                    vec![Edit::UpdateRackPatch {
                        id: instrument,
                        rack,
                    }],
                    label,
                )
            }
            _ => Ok(Outcome::default()),
        }
    }
}
