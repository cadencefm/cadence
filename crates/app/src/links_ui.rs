//! Universal `e` Links/Modulation/Macro/Part P-Lock controller, fuzzy Macro target picker,
//! exact parameter deck jump, and downstream `Values` consumer projection.
use super::*;
use crate::drafts::WorkflowContext;
use crate::view::PatternConsumerChip;
use cadence_core::macros::{MacroControl, MAX_MACRO_BINDINGS, MAX_PROJECT_MACROS};
use cadence_core::material::{ParameterLink, MAX_LINKS};
use cadence_core::mixer::EffectKind;

/// Action executed when the user chooses an item inside the unified `e` Links menu picker.
#[derive(Clone, Debug)]
pub(crate) enum LinkMenuAction {
    JumpToMacro {
        index: u8,
        binding: Option<usize>,
    },
    JumpToPattern(Id),
    JumpToParam(ParamAddress),
    JumpToPart {
        part: Id,
        link_index: Option<usize>,
    },
    JumpToEffect(Id),
    BindParamToMacroSlot {
        target: ParamAddress,
        slot: u8,
    },
    PickMacroSlotForParam(ParamAddress),
    UnbindMacro {
        index: u8,
        binding: usize,
    },
    AttachModulation {
        target: ModulationTarget,
        pattern: Option<Id>,
    },
    EditModulation(Id),
    UnlinkModulation(Id),
    PickPartForParamLink {
        instrument: Id,
        target: InstrumentParam,
    },
    PickPatternForPartLink {
        part: Id,
        target: InstrumentParam,
    },
    PickParamForPartLink {
        part: Id,
        replace_index: Option<usize>,
        preset_pattern: Option<Id>,
    },
    ApplyPartLink {
        part: Id,
        target: InstrumentParam,
        /// `None` creates a new 16-step normalized Ramp `Values` pattern atomically.
        pattern: Option<Id>,
    },
    ReplacePartLinkTarget {
        part: Id,
        index: usize,
        new_target: InstrumentParam,
    },
    UnlinkPartLink {
        part: Id,
        index: usize,
    },
    UnlinkFilterEnv {
        device: Id,
    },
    AttachPatternAsModulation(Id),
    PickPartForPatternLink(Id),
}

/// Single source of truth for human-readable short instrument parameter labels
/// using actual zone/module/cable names instead of raw IDs.
pub(crate) fn format_instrument_param_short(
    project: &Project,
    instrument: Id,
    param: InstrumentParam,
) -> String {
    let inst = project.instrument(instrument);
    let multi_zone = inst.is_some_and(|i| i.rack.zones.len() > 1);
    match param {
        InstrumentParam::Rack { param } => format!("Rack {}", param.label()),
        InstrumentParam::Zone { zone, param } => {
            if multi_zone {
                let zname = inst
                    .and_then(|i| i.rack.zone(zone))
                    .map_or_else(|| format!("z{zone}"), |z| z.name.clone());
                format!("{zname} {}", param.label())
            } else {
                format!("Zone {}", param.label())
            }
        }
        InstrumentParam::Module {
            zone,
            module,
            param,
        } => {
            let z = inst.and_then(|i| i.rack.zone(zone));
            let mname = z
                .and_then(|z| z.module(module))
                .map_or_else(|| format!("m{module}"), |m| m.name.clone());
            if multi_zone {
                let zname = z.map_or_else(|| format!("z{zone}"), |z| z.name.clone());
                format!("{zname}.{mname} {}", param.label())
            } else {
                format!("{mname} {}", param.label())
            }
        }
        InstrumentParam::Cable { zone, cable } => {
            if let Some(z) = inst.and_then(|i| i.rack.zone(zone)) {
                if let Some(c) = z.cable(cable) {
                    let from = z
                        .module(c.from_module)
                        .map_or_else(|| format!("m{}", c.from_module), |m| m.name.clone());
                    let to = z
                        .module(c.to_module)
                        .map_or_else(|| format!("m{}", c.to_module), |m| m.name.clone());
                    return format!("{from}→{to}.{}", c.to_port.slug());
                }
            }
            format!("Cable c{cable}")
        }
    }
}

fn channel_label(project: &Project, ch: ChannelRef) -> String {
    match ch {
        ChannelRef::Master => "Master".into(),
        ChannelRef::Channel(id) => project
            .mixer
            .channels
            .iter()
            .find(|c| c.id == id)
            .map_or_else(|| format!("Ch #{id}"), |c| c.name.clone()),
    }
}

/// Single source of truth for human-readable `ModulationTarget` labels across
/// links menus, pattern consumer chips, action palette, and modulation workflows.
pub(crate) fn format_modulation_target(project: &Project, target: ModulationTarget) -> String {
    match target {
        ModulationTarget::Macro { index } => {
            let mname = project
                .macro_control(index)
                .map_or_else(|| MacroControl::default_name(index), |m| m.name.clone());
            format!("M{} {mname}", usize::from(index) + 1)
        }
        ModulationTarget::Instrument {
            instrument,
            parameter,
        } => {
            let iname = project
                .instrument(instrument)
                .map_or_else(|| format!("Inst #{instrument}"), |i| i.name.clone());
            format!(
                "{iname} · {}",
                format_instrument_param_short(project, instrument, parameter)
            )
        }
        ModulationTarget::Effect { device, parameter } => {
            let dev_lbl = project.device(device).map_or_else(
                || format!("FX #{device}"),
                |(ch, d)| format!("{} · {}", channel_label(project, ch), d.name),
            );
            format!("{dev_lbl} · {}", parameter.label())
        }
        ModulationTarget::ChannelGain { channel } => {
            format!("{} · Gain", channel_label(project, channel))
        }
        ModulationTarget::ChannelPan { channel } => {
            format!("{} · Pan", channel_label(project, channel))
        }
        ModulationTarget::Send { channel, target } => format!(
            "{} · Send → {}",
            channel_label(project, channel),
            channel_label(project, ChannelRef::Channel(target))
        ),
    }
}

fn modulation_target_to_param_address(target: ModulationTarget) -> Option<ParamAddress> {
    match target {
        ModulationTarget::Instrument {
            instrument,
            parameter,
        } => Some(ParamAddress::from_instrument(instrument, parameter)),
        ModulationTarget::Effect { device, parameter } => Some(ParamAddress::Effect {
            device,
            param: parameter,
        }),
        ModulationTarget::ChannelGain { channel } => Some(ParamAddress::Channel {
            channel,
            param: ChannelParam::Gain,
        }),
        ModulationTarget::ChannelPan { channel } => Some(ParamAddress::Channel {
            channel,
            param: ChannelParam::Pan,
        }),
        ModulationTarget::Send { channel, target } => Some(ParamAddress::Send { channel, target }),
        ModulationTarget::Macro { .. } => None,
    }
}

impl App {
    /// Human-readable short parameter label for an instrument's `InstrumentParam`
    /// using actual zone/module/cable names instead of raw IDs.
    pub(crate) fn format_instrument_param_short(
        &self,
        instrument: Id,
        param: InstrumentParam,
    ) -> String {
        format_instrument_param_short(self.doc.project(), instrument, param)
    }

    /// Enumerate every automatable `ParamAddress` across all 7 `MacroTargetKind`s
    /// (`Rack`, `Zone`, `Module`, `Cable`, `Channel`, `Send`, `Effect`) for fuzzy binding pickers.
    pub(crate) fn all_bindable_macro_targets(&self) -> Vec<(ParamAddress, String, String)> {
        let project = self.project();
        let mut out = Vec::new();
        let mut push_addr = |addr: ParamAddress| {
            let (kind, label) = self.format_macro_target(addr);
            let cur = self
                .read_param_normalized(addr)
                .map(|n| self.format_param_normalized_value(addr, n))
                .unwrap_or_else(|| "0%".into());
            out.push((addr, format!("[{}] {label}", kind.badge()), cur));
        };

        // 1–4. Rack, Zone, Module, Cable parameters across every instrument.
        for inst in &project.instruments {
            for p in RackParam::ALL {
                if p.is_automatable() {
                    push_addr(ParamAddress::Rack {
                        instrument: inst.id,
                        param: p,
                    });
                }
            }
            for zone in &inst.rack.zones {
                for zp in ZoneParam::ALL
                    .iter()
                    .copied()
                    .filter(|p| p.is_automatable())
                {
                    push_addr(ParamAddress::Zone {
                        instrument: inst.id,
                        zone: zone.id,
                        param: zp,
                    });
                }
                for m in &zone.modules {
                    for &mp in m.kind.tag().params() {
                        if mp.is_automatable() {
                            push_addr(ParamAddress::Module {
                                instrument: inst.id,
                                zone: zone.id,
                                module: m.id,
                                param: mp,
                            });
                        }
                    }
                }
                for c in &zone.cables {
                    push_addr(ParamAddress::Cable {
                        instrument: inst.id,
                        zone: zone.id,
                        cable: c.id,
                    });
                }
            }
        }

        // 5. Channel Gain & Pan across all channels + Master.
        for ch_ref in self.channels() {
            push_addr(ParamAddress::Channel {
                channel: ch_ref,
                param: ChannelParam::Gain,
            });
            push_addr(ParamAddress::Channel {
                channel: ch_ref,
                param: ChannelParam::Pan,
            });
        }

        // 6. Channel → Return Sends (all valid non-Return source × Return target pairs).
        let returns: Vec<Id> = project
            .mixer
            .channels
            .iter()
            .filter(|c| c.kind == ChannelKind::Return)
            .map(|c| c.id)
            .collect();
        for ch in &project.mixer.channels {
            if ch.kind != ChannelKind::Return {
                for &ret_id in &returns {
                    push_addr(ParamAddress::Send {
                        channel: ChannelRef::Channel(ch.id),
                        target: ret_id,
                    });
                }
            }
        }

        // 7. Effect parameters across all channel inserts + Master inserts.
        for ch in &project.mixer.channels {
            for dev in &ch.inserts {
                for &ep in dev.params() {
                    if dev.read_normalized(ep).is_some() {
                        push_addr(ParamAddress::Effect {
                            device: dev.id,
                            param: ep,
                        });
                    }
                }
            }
        }
        for dev in &project.mixer.master.inserts {
            for &ep in dev.params() {
                if dev.read_normalized(ep).is_some() {
                    push_addr(ParamAddress::Effect {
                        device: dev.id,
                        param: ep,
                    });
                }
            }
        }

        out
    }

    /// Enumerate all automatable `InstrumentParam`s on `instrument` (for Part P-lock picker).
    pub(crate) fn instrument_automatable_params(
        &self,
        instrument: Id,
    ) -> Vec<(InstrumentParam, String, String)> {
        let Some(inst) = self.instrument(instrument) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut push_param = |ip: InstrumentParam, badge: &'static str| {
            let addr = ParamAddress::from_instrument(instrument, ip);
            let label = self.format_instrument_param_short(instrument, ip);
            let cur = self
                .read_param_normalized(addr)
                .map(|n| self.format_param_normalized_value(addr, n))
                .unwrap_or_else(|| "—".into());
            out.push((ip, format!("[{badge}] {label}"), cur));
        };
        for p in RackParam::ALL {
            if p.is_automatable() {
                push_param(InstrumentParam::Rack { param: p }, "RACK");
            }
        }
        for zone in &inst.rack.zones {
            for zp in ZoneParam::ALL
                .iter()
                .copied()
                .filter(|p| p.is_automatable())
            {
                push_param(
                    InstrumentParam::Zone {
                        zone: zone.id,
                        param: zp,
                    },
                    "ZONE",
                );
            }
            for m in &zone.modules {
                for &mp in m.kind.tag().params() {
                    if mp.is_automatable() {
                        push_param(
                            InstrumentParam::Module {
                                zone: zone.id,
                                module: m.id,
                                param: mp,
                            },
                            m.kind.tag().short_badge(),
                        );
                    }
                }
            }
            for c in &zone.cables {
                push_param(
                    InstrumentParam::Cable {
                        zone: zone.id,
                        cable: c.id,
                    },
                    "CABLE",
                );
            }
        }
        out
    }

    /// Jump directly to the deck, tab, zone, module/cable, and exact `control_cursor` of `target`.
    pub(crate) fn jump_to_param_address(&mut self, target: ParamAddress) -> Result<()> {
        match target {
            ParamAddress::Rack { instrument, .. } => {
                self.open_instrument(instrument)?;
                self.rack_ui.tab = RackTab::Panel;
                self.rack_ui.panel_slot = 0;
                self.rack_ui.patchbay_cable_id = None;
                self.focus_control_for_param(target);
            }
            ParamAddress::Zone {
                instrument, zone, ..
            } => {
                self.open_instrument(instrument)?;
                if let Some(z_idx) = self
                    .instrument(instrument)
                    .and_then(|i| i.rack.zones.iter().position(|z| z.id == zone))
                {
                    self.rack_ui.zone_cursor = z_idx;
                }
                self.rack_ui.tab = RackTab::Zones;
                self.rack_ui.patchbay_cable_id = None;
                if !self.focus_control_for_param(target) {
                    // Fallback to Out module on Panel if ZoneParam is not listed on Zones tab.
                    let out_mod = self
                        .instrument(instrument)
                        .and_then(|i| i.rack.zone(zone))
                        .and_then(|z| {
                            z.modules
                                .iter()
                                .find(|m| m.kind.tag() == ModuleKindTag::Out)
                                .map(|m| m.id)
                        });
                    if let Some(mid) = out_mod {
                        self.focus_rack_module_panel(instrument, mid);
                        self.focus_control_for_param(target);
                    }
                }
            }
            ParamAddress::Module {
                instrument,
                zone,
                module,
                ..
            } => {
                self.open_instrument(instrument)?;
                if let Some(z_idx) = self
                    .instrument(instrument)
                    .and_then(|i| i.rack.zones.iter().position(|z| z.id == zone))
                {
                    self.rack_ui.zone_cursor = z_idx;
                }
                self.focus_rack_module_panel(instrument, module);
                self.focus_control_for_param(target);
            }
            ParamAddress::Cable {
                instrument,
                zone,
                cable,
            } => {
                self.open_instrument(instrument)?;
                let focus_mod = self.instrument(instrument).and_then(|i| {
                    let z_idx = i.rack.zones.iter().position(|z| z.id == zone)?;
                    let from_mod = i.rack.zones[z_idx].cable(cable).map(|c| c.from_module);
                    Some((z_idx, i.rack.zones[z_idx].clone(), from_mod))
                });
                if let Some((z_idx, z_clone, from_mod)) = focus_mod {
                    self.rack_ui.zone_cursor = z_idx;
                    self.rack_ui.tab = RackTab::Patchbay;
                    if let Some(mid) = from_mod {
                        self.focus_patchbay_module(&z_clone, mid);
                    }
                    self.rack_ui.patchbay_cable_id = Some(cable);
                    self.status = self.describe_patchbay_cable(&z_clone, cable);
                }
            }
            ParamAddress::Effect { device, .. } => {
                self.open_effect(device)?;
                self.focus_control_for_param(target);
            }
            ParamAddress::Channel { channel, .. } | ParamAddress::Send { channel, .. } => {
                self.open_channel(channel)?;
                self.focus_control_for_param(target);
            }
        }
        Ok(())
    }

    fn focus_control_for_param(&mut self, target: ParamAddress) -> bool {
        if let Some(pos) = self
            .control_descriptors()
            .iter()
            .position(|c| matches!(c.target, ControlTarget::Param(a) if a == target))
        {
            self.control_cursor = pos;
            return true;
        }
        false
    }

    /// Downstream consumer chips rendered on `Values` patterns in `gp`.
    pub(crate) fn values_pattern_consumers(&self, pattern_id: Id) -> Vec<PatternConsumerChip> {
        let project = self.project();
        let mut chips = Vec::new();
        for m in project
            .modulations
            .iter()
            .filter(|m| m.pattern == pattern_id)
        {
            let label = format!("Drives {}", format_modulation_target(project, m.target));
            chips.push(PatternConsumerChip {
                label,
                detail: m.name.clone(),
            });
        }
        for part in &project.parts {
            for link in &part.parameter_links {
                if link.pattern == pattern_id {
                    let plbl = self.format_instrument_param_short(part.instrument, link.target);
                    chips.push(PatternConsumerChip {
                        label: format!("P-Lock {} · {plbl}", part.name),
                        detail: part.name.clone(),
                    });
                }
            }
        }
        for ch in &project.mixer.channels {
            for dev in &ch.inserts {
                if matches!(dev.kind, EffectKind::Filter(fp) if fp.env_pattern_id == Some(pattern_id))
                {
                    chips.push(PatternConsumerChip {
                        label: format!("Filter Env · {}", dev.name),
                        detail: ch.name.clone(),
                    });
                }
            }
        }
        for dev in &project.mixer.master.inserts {
            if matches!(dev.kind, EffectKind::Filter(fp) if fp.env_pattern_id == Some(pattern_id)) {
                chips.push(PatternConsumerChip {
                    label: format!("Filter Env · {}", dev.name),
                    detail: "Master".into(),
                });
            }
        }
        for p in &project.patterns {
            if p.kind.sources().contains(&pattern_id) {
                chips.push(PatternConsumerChip {
                    label: format!("Feeds {}", p.name),
                    detail: kind_label(&p.kind).into(),
                });
            }
        }
        chips
    }

    /// Universal `e` entry point across Rack (`Panel`/`Patchbay`/`Zones`), `Detail::Channel`,
    /// `Detail::Effect`, `Detail::Part`, `Mixer` slots, `Macros`, and `Values` patterns.
    pub(crate) fn open_focused_links_menu(&mut self) -> Result<Outcome> {
        if self.macros_focused() {
            self.clamp_macro_binding_cursor();
            let slot = self.view_state.macro_cursor as u8;
            if let Some(b_idx) = self.view_state.macro_binding_cursor {
                if let Some(target) = self
                    .project()
                    .macro_control(slot)
                    .and_then(|m| m.bindings.get(b_idx))
                    .map(|b| b.target)
                {
                    return self.open_param_links_menu(target);
                }
            }
            return self.open_macro_slot_links_menu(slot);
        }

        if self.deck_focused() {
            match self.detail {
                Detail::Instrument(inst_id) => {
                    if self.rack_ui.tab == RackTab::Patchbay {
                        if let Some(cid) = self.rack_ui.patchbay_cable_id {
                            if let Some(z) = self.active_rack_zone(inst_id) {
                                return self.open_param_links_menu(ParamAddress::Cable {
                                    instrument: inst_id,
                                    zone: z.id,
                                    cable: cid,
                                });
                            }
                        }
                        self.status =
                            "Select a cable with w/b or open a module Panel (Enter) to manage links"
                                .into();
                        return Ok(Outcome::default());
                    }
                    if let Some(ctrl) = self.selected_control() {
                        return self.open_control_target_links_menu(ctrl.target);
                    }
                }
                Detail::Channel(_) | Detail::Effect(_) => {
                    if let Some(ctrl) = self.selected_control() {
                        return self.open_control_target_links_menu(ctrl.target);
                    }
                }
                Detail::Part(part_id) => {
                    if let Some(ctrl) = self.selected_control() {
                        if let ControlTarget::PartParamLink { part, index, .. } = ctrl.target {
                            return self.open_part_link_item_menu(part, index);
                        }
                    }
                    return self.open_part_links_overview_menu(part_id);
                }
                Detail::Placement(_) | Detail::None => {}
            }
        }

        if self.mixer_canvas_focused() {
            self.clamp_mixer_slot();
            return match self.view_state.mixer_slot {
                MixerSlot::Fader => self.open_param_links_menu(ParamAddress::Channel {
                    channel: self.selected_channel,
                    param: ChannelParam::Gain,
                }),
                MixerSlot::Pan => self.open_param_links_menu(ParamAddress::Channel {
                    channel: self.selected_channel,
                    param: ChannelParam::Pan,
                }),
                MixerSlot::Send(idx) => {
                    if let Some((ch, target)) = self.mixer_focused_send(idx) {
                        self.open_param_links_menu(ParamAddress::Send {
                            channel: ChannelRef::Channel(ch),
                            target,
                        })
                    } else {
                        Ok(Outcome::default())
                    }
                }
                MixerSlot::Insert(idx) => {
                    if let Some(dev_id) = self.mixer_focused_insert(idx) {
                        self.open_effect_links_overview_menu(dev_id)
                    } else {
                        Ok(Outcome::default())
                    }
                }
                MixerSlot::Duck(idx) => {
                    if let Some(id) = self.mixer_focused_duck(idx) {
                        self.begin_workflow(WorkflowKind::EditDuck(id))
                    } else {
                        Ok(Outcome::default())
                    }
                }
                MixerSlot::Output => {
                    self.status =
                        "Output bus route · Enter opens bus picker · focus Fader/Pan/Send/FX for links (e)"
                            .into();
                    Ok(Outcome::default())
                }
            };
        }

        if self.pattern_canvas_focused()
            || (self.bank_focused() && self.view_state.bank_mode == BankMode::Material)
        {
            if let Some(pat_id) = self.selected_pattern {
                if matches!(
                    self.project().pattern_type(pat_id),
                    Ok(PatternType::Values(_))
                ) {
                    return self.open_pattern_consumers_menu(pat_id);
                }
            }
        }

        self.status =
            "Focus a continuous parameter, Macro slot, Part, or Values pattern · press e for links"
                .into();
        Ok(Outcome::default())
    }

    fn open_control_target_links_menu(&mut self, target: ControlTarget) -> Result<Outcome> {
        match target {
            ControlTarget::Param(addr) => self.open_param_links_menu(addr),
            ControlTarget::ModulationLink { id, .. } => self.open_modulation_item_menu(id),
            ControlTarget::PartParamLink { part, index, .. } => {
                self.open_part_link_item_menu(part, index)
            }
            ControlTarget::MacroBinding { index, binding } => {
                if let Some(addr) = self
                    .project()
                    .macro_control(index)
                    .and_then(|m| m.bindings.get(binding))
                    .map(|b| b.target)
                {
                    self.open_param_links_menu(addr)
                } else {
                    Ok(Outcome::default())
                }
            }
            ControlTarget::FilterEnvSource(dev_id) => self.open_filter_env_links_menu(dev_id),
            ControlTarget::Duck(id)
            | ControlTarget::DuckDepth(id)
            | ControlTarget::DuckRecovery(id) => self.begin_workflow(WorkflowKind::EditDuck(id)),
            ControlTarget::Effect(dev_id) => self.open_effect_links_overview_menu(dev_id),
            _ => {
                self.status =
                    "Focus a continuous parameter or link row · press e to manage Macro/Mod/P-lock links"
                        .into();
                Ok(Outcome::default())
            }
        }
    }

    /// `e` on Macros Dial header (`M1..M8`): attach / edit / unlink / jump to driving Modulation.
    pub(crate) fn open_macro_slot_links_menu(&mut self, slot: u8) -> Result<Outcome> {
        let project = self.project();
        let slot1 = usize::from(slot) + 1;
        let mname = project
            .macro_control(slot)
            .map_or_else(|| MacroControl::default_name(slot), |m| m.name.clone());
        let mut entries: Vec<(LinkMenuAction, String, String)> = Vec::new();
        let drivers: Vec<(Id, String, Id, String)> = project
            .modulations
            .iter()
            .filter(|m| m.target == ModulationTarget::Macro { index: slot })
            .map(|m| {
                let pname = project
                    .pattern(m.pattern)
                    .map_or_else(|| format!("Pattern #{}", m.pattern), |p| p.name.clone());
                (m.id, m.name.clone(), m.pattern, pname)
            })
            .collect();
        if drivers.is_empty() {
            entries.push((
                LinkMenuAction::AttachModulation {
                    target: ModulationTarget::Macro { index: slot },
                    pattern: None,
                },
                format!("+ Attach global Modulation to M{slot1}…"),
                "Values pattern → automate macro dial".into(),
            ));
        } else {
            for (mod_id, mod_name, pat_id, pat_name) in drivers {
                entries.push((
                    LinkMenuAction::JumpToPattern(pat_id),
                    format!("Jump to pattern: {pat_name}"),
                    format!("MOD {mod_name} · open Values"),
                ));
                entries.push((
                    LinkMenuAction::EditModulation(mod_id),
                    format!("Edit modulation: {mod_name}"),
                    "change source pattern or name".into(),
                ));
                entries.push((
                    LinkMenuAction::UnlinkModulation(mod_id),
                    format!("Unlink modulation: {mod_name}"),
                    "remove driving modulation".into(),
                ));
            }
        }
        self.open_link_menu_picker(format!("LINKS · M{slot1} {mname}"), entries)
    }

    /// `e` on any continuous `ParamAddress`: inspect/jump/edit/unlink active Macros, Modulations,
    /// and Part P-locks, or bind to `M1–M8`, attach Modulation, or add Part P-lock.
    pub(crate) fn open_param_links_menu(&mut self, address: ParamAddress) -> Result<Outcome> {
        let (_, label) = self.format_macro_target(address);
        if !address.is_automatable() {
            self.status = format!(
                "{label} is a discrete/structural parameter and cannot be modulated or macro-bound"
            );
            return Ok(Outcome::default());
        }

        let project = self.project();
        let mut entries: Vec<(LinkMenuAction, String, String)> = Vec::new();

        // 1. Active Macro bindings targeting `address`.
        let mut macros: Vec<&MacroControl> = project.macros.iter().collect();
        macros.sort_by_key(|m| m.index);
        for mac in macros {
            for (b_idx, b) in mac.bindings.iter().enumerate() {
                if b.target == address {
                    let arrow = if b.is_inverted() { "↓" } else { "→" };
                    let min_s = self.format_param_normalized_value(address, b.min);
                    let max_s = self.format_param_normalized_value(address, b.max);
                    entries.push((
                        LinkMenuAction::JumpToMacro {
                            index: mac.index,
                            binding: Some(b_idx),
                        },
                        format!("Jump to M{} {}", usize::from(mac.index) + 1, mac.name),
                        format!("{min_s} {arrow} {max_s} · open Macros"),
                    ));
                    entries.push((
                        LinkMenuAction::UnbindMacro {
                            index: mac.index,
                            binding: b_idx,
                        },
                        format!("Unbind from M{} {}", usize::from(mac.index) + 1, mac.name),
                        "remove macro binding".into(),
                    ));
                }
            }
        }

        // 2. Active global Modulation targeting `address`.
        let mod_target = self.param_address_to_mod_target(address);
        let mut has_direct_mod = false;
        if let Some(mt) = mod_target {
            for m in project.modulations.iter().filter(|m| m.target == mt) {
                has_direct_mod = true;
                let pname = project
                    .pattern(m.pattern)
                    .map_or_else(|| format!("Pattern #{}", m.pattern), |p| p.name.clone());
                entries.push((
                    LinkMenuAction::JumpToPattern(m.pattern),
                    format!("Jump to Mod pattern: {pname}"),
                    format!("MOD {} · open Values", m.name),
                ));
                entries.push((
                    LinkMenuAction::EditModulation(m.id),
                    format!("Edit modulation: {}", m.name),
                    "change source pattern or target".into(),
                ));
                entries.push((
                    LinkMenuAction::UnlinkModulation(m.id),
                    format!("Unlink modulation: {}", m.name),
                    "remove global modulation".into(),
                ));
            }
        }

        // 3. Active Part P-locks (`Part.parameter_links`) targeting `(inst_id, inst_param)`.
        if let Some((inst_id, inst_param)) = address.as_instrument() {
            for part in project.parts.iter().filter(|p| p.instrument == inst_id) {
                for (l_idx, link) in part.parameter_links.iter().enumerate() {
                    if link.target == inst_param {
                        let pname = project.pattern(link.pattern).map_or_else(
                            || format!("Pattern #{}", link.pattern),
                            |p| p.name.clone(),
                        );
                        entries.push((
                            LinkMenuAction::JumpToPattern(link.pattern),
                            format!("Jump to P-lock pattern: {pname}"),
                            format!("Part {} · open Values", part.name),
                        ));
                        entries.push((
                            LinkMenuAction::PickPatternForPartLink {
                                part: part.id,
                                target: inst_param,
                            },
                            format!("Edit P-lock on {}", part.name),
                            "change per-note Values pattern".into(),
                        ));
                        entries.push((
                            LinkMenuAction::UnlinkPartLink {
                                part: part.id,
                                index: l_idx,
                            },
                            format!("Unlink P-lock on {}", part.name),
                            "remove per-note parameter link".into(),
                        ));
                    }
                }
            }
        }

        // 4. Add / Bind actions.
        entries.push((
            LinkMenuAction::PickMacroSlotForParam(address),
            "+ Bind to Macro M1–M8…".into(),
            "map onto a Project Macro slot (0% → 100%)".into(),
        ));
        if let Some(mt) = mod_target {
            if !has_direct_mod {
                entries.push((
                    LinkMenuAction::AttachModulation {
                        target: mt,
                        pattern: None,
                    },
                    "+ Attach global Modulation…".into(),
                    "Values pattern → continuous modulation".into(),
                ));
            }
        }
        if let Some((inst_id, inst_param)) = address.as_instrument() {
            let matching_parts: Vec<(Id, String)> = project
                .parts
                .iter()
                .filter(|p| p.instrument == inst_id)
                .map(|p| (p.id, p.name.clone()))
                .collect();
            if matching_parts.len() == 1 {
                let (pid, pname) = &matching_parts[0];
                entries.push((
                    LinkMenuAction::PickPatternForPartLink {
                        part: *pid,
                        target: inst_param,
                    },
                    format!("+ Set Part P-lock on {pname}…"),
                    "per-note Values pattern".into(),
                ));
            } else if matching_parts.len() > 1 {
                entries.push((
                    LinkMenuAction::PickPartForParamLink {
                        instrument: inst_id,
                        target: inst_param,
                    },
                    "+ Set Part P-lock…".into(),
                    format!("choose from {} parts", matching_parts.len()),
                ));
            }
        }

        self.open_link_menu_picker(format!("LINKS · {label}"), entries)
    }

    fn open_modulation_item_menu(&mut self, mod_id: Id) -> Result<Outcome> {
        let m = self
            .project()
            .modulations
            .iter()
            .find(|m| m.id == mod_id)
            .context("Modulation missing")?
            .clone();
        let pname = self
            .pattern(m.pattern)
            .map_or_else(|| format!("Pattern #{}", m.pattern), |p| p.name.clone());
        let entries = vec![
            (
                LinkMenuAction::JumpToPattern(m.pattern),
                format!("Jump to pattern: {pname}"),
                "open Values source".into(),
            ),
            (
                LinkMenuAction::EditModulation(m.id),
                format!("Edit modulation: {}", m.name),
                "change source pattern or target".into(),
            ),
            (
                LinkMenuAction::UnlinkModulation(m.id),
                format!("Unlink modulation: {}", m.name),
                "remove global modulation".into(),
            ),
        ];
        self.open_link_menu_picker(format!("MODULATION · {}", m.name), entries)
    }

    fn open_filter_env_links_menu(&mut self, device: Id) -> Result<Outcome> {
        let (_, dev) = self.effect(device).context("Effect missing")?;
        let dev_name = dev.name.clone();
        let env_pat = match dev.kind {
            EffectKind::Filter(fp) => fp.env_pattern_id,
            _ => None,
        };
        let mut entries = Vec::new();
        if let Some(pid) = env_pat {
            let pname = self
                .pattern(pid)
                .map_or_else(|| format!("Pattern #{pid}"), |p| p.name.clone());
            entries.push((
                LinkMenuAction::JumpToPattern(pid),
                format!("Jump to Filter Env pattern: {pname}"),
                "open linked pattern".into(),
            ));
            entries.push((
                LinkMenuAction::UnlinkFilterEnv { device },
                format!("Unlink Filter Env pattern ({pname})"),
                "revert to Groove Bank".into(),
            ));
        } else {
            self.status =
                "Filter Env uses Groove Bank · press P for Groove Browser or Alt+h/l to select pattern"
                    .into();
            return Ok(Outcome::default());
        }
        self.open_link_menu_picker(format!("FILTER ENV · {dev_name}"), entries)
    }

    fn open_effect_links_overview_menu(&mut self, device: Id) -> Result<Outcome> {
        let project = self.project();
        let Some((_, dev)) = project.device(device) else {
            return Ok(Outcome::default());
        };
        let dev_name = dev.name.clone();
        let mut entries = Vec::new();
        for mac in &project.macros {
            for (b_idx, b) in mac.bindings.iter().enumerate() {
                if matches!(b.target, ParamAddress::Effect { device: d, .. } if d == device) {
                    let (_, tname) = self.format_macro_target(b.target);
                    entries.push((
                        LinkMenuAction::JumpToMacro {
                            index: mac.index,
                            binding: Some(b_idx),
                        },
                        format!(
                            "Jump to M{} {} ({tname})",
                            usize::from(mac.index) + 1,
                            mac.name
                        ),
                        "open Macros deck".into(),
                    ));
                    entries.push((
                        LinkMenuAction::UnbindMacro {
                            index: mac.index,
                            binding: b_idx,
                        },
                        format!("Unbind {tname} from M{}", usize::from(mac.index) + 1),
                        "remove macro binding".into(),
                    ));
                }
            }
        }
        for m in project.modulations.iter().filter(
            |m| matches!(m.target, ModulationTarget::Effect { device: d, .. } if d == device),
        ) {
            let pname = project
                .pattern(m.pattern)
                .map_or_else(|| format!("Pattern #{}", m.pattern), |p| p.name.clone());
            entries.push((
                LinkMenuAction::JumpToPattern(m.pattern),
                format!("Jump to Mod pattern: {pname}"),
                format!("MOD {}", m.name),
            ));
            entries.push((
                LinkMenuAction::EditModulation(m.id),
                format!("Edit modulation: {}", m.name),
                "change source pattern or parameter".into(),
            ));
            entries.push((
                LinkMenuAction::UnlinkModulation(m.id),
                format!("Unlink modulation: {}", m.name),
                "remove global modulation".into(),
            ));
        }
        if let EffectKind::Filter(fp) = dev.kind {
            if let Some(pid) = fp.env_pattern_id {
                let pname = project
                    .pattern(pid)
                    .map_or_else(|| format!("Pattern #{pid}"), |p| p.name.clone());
                entries.push((
                    LinkMenuAction::JumpToPattern(pid),
                    format!("Jump to Filter Env: {pname}"),
                    "open linked pattern".into(),
                ));
                entries.push((
                    LinkMenuAction::UnlinkFilterEnv { device },
                    format!("Unlink Filter Env ({pname})"),
                    "revert to Groove Bank".into(),
                ));
            }
        }
        if let Some(&first_param) = dev.params().first() {
            entries.push((
                LinkMenuAction::AttachModulation {
                    target: ModulationTarget::Effect {
                        device,
                        parameter: first_param,
                    },
                    pattern: None,
                },
                format!("+ Attach global Modulation to {dev_name}…"),
                "Values pattern → effect parameter".into(),
            ));
        }
        entries.push((
            LinkMenuAction::JumpToEffect(device),
            format!("Open {dev_name} Effect deck"),
            "focus individual parameters to bind/mod".into(),
        ));
        self.open_link_menu_picker(format!("LINKS · FX {dev_name}"), entries)
    }

    /// `a` on `Detail::Part(part_id)`: add a new per-note P-lock (`Part.parameter_links`).
    pub(crate) fn open_add_part_param_link_picker(&mut self, part_id: Id) -> Result<Outcome> {
        let part = self.part(part_id).context("Part missing")?;
        ensure!(
            part.parameter_links.len() < MAX_LINKS,
            "Part '{}' already has the maximum {MAX_LINKS} P-lock links",
            part.name
        );
        self.open_part_link_param_picker(part_id, None, None)
    }

    fn open_part_link_param_picker(
        &mut self,
        part_id: Id,
        replace_index: Option<usize>,
        preset_pattern: Option<Id>,
    ) -> Result<Outcome> {
        let part = self.part(part_id).context("Part missing")?.clone();
        let params = self.instrument_automatable_params(part.instrument);
        ensure!(
            !params.is_empty(),
            "Instrument has no automatable parameters to P-lock"
        );
        let mut entries = Vec::with_capacity(params.len());
        for (ip, name, cur) in params {
            let existing = part.parameter_links.iter().any(|l| l.target == ip);
            let detail = if existing {
                format!("{cur} · [linked]")
            } else {
                cur
            };
            let action = if let Some(idx) = replace_index {
                LinkMenuAction::ReplacePartLinkTarget {
                    part: part_id,
                    index: idx,
                    new_target: ip,
                }
            } else if let Some(pat_id) = preset_pattern {
                LinkMenuAction::ApplyPartLink {
                    part: part_id,
                    target: ip,
                    pattern: Some(pat_id),
                }
            } else {
                LinkMenuAction::PickPatternForPartLink {
                    part: part_id,
                    target: ip,
                }
            };
            entries.push((action, name, detail));
        }
        let title = if replace_index.is_some() {
            format!("CHANGE P-LOCK TARGET · {}", part.name)
        } else {
            format!("ADD P-LOCK PARAM · {}", part.name)
        };
        self.open_link_menu_picker(title, entries)
    }

    fn open_part_link_item_menu(&mut self, part_id: Id, index: usize) -> Result<Outcome> {
        let part = self.part(part_id).context("Part missing")?.clone();
        let link = part
            .parameter_links
            .get(index)
            .cloned()
            .context("Parameter link missing")?;
        let param_lbl = self.format_instrument_param_short(part.instrument, link.target);
        let pat_name = self
            .pattern(link.pattern)
            .map_or_else(|| format!("Pattern #{}", link.pattern), |p| p.name.clone());
        let inst_name = self
            .instrument(part.instrument)
            .map_or_else(|| format!("Inst #{}", part.instrument), |i| i.name.clone());
        let entries = vec![
            (
                LinkMenuAction::JumpToPattern(link.pattern),
                format!("Jump to Values pattern: {pat_name}"),
                "open per-note curve".into(),
            ),
            (
                LinkMenuAction::PickPatternForPartLink {
                    part: part_id,
                    target: link.target,
                },
                "Change Values pattern…".into(),
                format!("currently {pat_name}"),
            ),
            (
                LinkMenuAction::PickParamForPartLink {
                    part: part_id,
                    replace_index: Some(index),
                    preset_pattern: None,
                },
                "Change target parameter…".into(),
                format!("currently {param_lbl}"),
            ),
            (
                LinkMenuAction::JumpToParam(ParamAddress::from_instrument(
                    part.instrument,
                    link.target,
                )),
                format!("Jump to {param_lbl} on {inst_name}"),
                "open Rack parameter".into(),
            ),
            (
                LinkMenuAction::UnlinkPartLink {
                    part: part_id,
                    index,
                },
                format!("Unlink P-lock ({param_lbl})"),
                "remove from Part".into(),
            ),
        ];
        self.open_link_menu_picker(format!("P-LOCK · {} · {param_lbl}", part.name), entries)
    }

    fn open_part_links_overview_menu(&mut self, part_id: Id) -> Result<Outcome> {
        let part = self.part(part_id).context("Part missing")?.clone();
        let mut entries = Vec::new();
        for (idx, link) in part.parameter_links.iter().enumerate() {
            let param_lbl = self.format_instrument_param_short(part.instrument, link.target);
            let pat_name = self
                .pattern(link.pattern)
                .map_or_else(|| format!("Pattern #{}", link.pattern), |p| p.name.clone());
            entries.push((
                LinkMenuAction::JumpToPattern(link.pattern),
                format!("Jump to {pat_name} ({param_lbl})"),
                "open Values curve".into(),
            ));
            entries.push((
                LinkMenuAction::PickPatternForPartLink {
                    part: part_id,
                    target: link.target,
                },
                format!("Edit P-lock: {param_lbl}"),
                format!("rebind pattern (currently {pat_name})"),
            ));
            entries.push((
                LinkMenuAction::UnlinkPartLink {
                    part: part_id,
                    index: idx,
                },
                format!("Unlink P-lock: {param_lbl}"),
                "remove per-note link".into(),
            ));
        }
        if part.parameter_links.len() < MAX_LINKS {
            entries.push((
                LinkMenuAction::PickParamForPartLink {
                    part: part_id,
                    replace_index: None,
                    preset_pattern: None,
                },
                format!("+ Add Part P-lock on {}…", part.name),
                "link a normalized Values pattern to a Rack parameter".into(),
            ));
        }
        self.open_link_menu_picker(format!("PART P-LOCKS · {}", part.name), entries)
    }

    /// `e` on a `Values` pattern in `gp` or `gb` Material: inspect/jump/edit/unlink consumers or attach new ones.
    pub(crate) fn open_pattern_consumers_menu(&mut self, pattern_id: Id) -> Result<Outcome> {
        let project = self.project();
        let pat = self.pattern(pattern_id).context("Pattern missing")?;
        let pat_name = pat.name.clone();
        let is_norm_values = project.pattern_type(pattern_id).ok()
            == Some(PatternType::Values(ValueUnit::Normalized));

        let mut entries: Vec<(LinkMenuAction, String, String)> = Vec::new();

        // 1. Global Modulations driven by this pattern.
        for m in project
            .modulations
            .iter()
            .filter(|m| m.pattern == pattern_id)
        {
            let target_label = format_modulation_target(project, m.target);
            if let ModulationTarget::Macro { index } = m.target {
                entries.push((
                    LinkMenuAction::JumpToMacro {
                        index,
                        binding: None,
                    },
                    format!("Jump to {target_label}"),
                    format!("MOD {} · open Macros", m.name),
                ));
            } else if let Some(addr) = modulation_target_to_param_address(m.target) {
                entries.push((
                    LinkMenuAction::JumpToParam(addr),
                    format!("Jump to {target_label}"),
                    format!("MOD {}", m.name),
                ));
            }
            entries.push((
                LinkMenuAction::EditModulation(m.id),
                format!("Edit modulation: {}", m.name),
                "change target or name".into(),
            ));
            entries.push((
                LinkMenuAction::UnlinkModulation(m.id),
                format!("Unlink modulation: {}", m.name),
                "remove global modulation".into(),
            ));
        }

        // 2. Part P-locks driven by this pattern.
        for part in &project.parts {
            for (idx, link) in part.parameter_links.iter().enumerate() {
                if link.pattern == pattern_id {
                    let plbl = self.format_instrument_param_short(part.instrument, link.target);
                    entries.push((
                        LinkMenuAction::JumpToPart {
                            part: part.id,
                            link_index: Some(idx),
                        },
                        format!("Jump to Part {} · {plbl}", part.name),
                        "open Part P-lock".into(),
                    ));
                    entries.push((
                        LinkMenuAction::UnlinkPartLink {
                            part: part.id,
                            index: idx,
                        },
                        format!("Unlink P-lock from {} ({plbl})", part.name),
                        "remove per-note link".into(),
                    ));
                }
            }
        }

        // 3. Filter FX envelope triggers using this pattern.
        for ch in &project.mixer.channels {
            for dev in &ch.inserts {
                if matches!(dev.kind, EffectKind::Filter(fp) if fp.env_pattern_id == Some(pattern_id))
                {
                    entries.push((
                        LinkMenuAction::JumpToEffect(dev.id),
                        format!("Jump to Filter FX: {} ({})", dev.name, ch.name),
                        "Filter Env consumer".into(),
                    ));
                    entries.push((
                        LinkMenuAction::UnlinkFilterEnv { device: dev.id },
                        format!("Unlink Filter Env from {}", dev.name),
                        "revert to Groove Bank".into(),
                    ));
                }
            }
        }
        for dev in &project.mixer.master.inserts {
            if matches!(dev.kind, EffectKind::Filter(fp) if fp.env_pattern_id == Some(pattern_id)) {
                entries.push((
                    LinkMenuAction::JumpToEffect(dev.id),
                    format!("Jump to Filter FX: {} (Master)", dev.name),
                    "Filter Env consumer".into(),
                ));
                entries.push((
                    LinkMenuAction::UnlinkFilterEnv { device: dev.id },
                    format!("Unlink Filter Env from {}", dev.name),
                    "revert to Groove Bank".into(),
                ));
            }
        }

        // 4. Derived patterns consuming this pattern.
        for p in &project.patterns {
            if p.kind.sources().contains(&pattern_id) {
                entries.push((
                    LinkMenuAction::JumpToPattern(p.id),
                    format!("Jump to derived: {}", p.name),
                    kind_label(&p.kind).into(),
                ));
            }
        }

        // 5. Attach as new Modulation or Part P-lock (when normalized Values).
        if is_norm_values {
            entries.push((
                LinkMenuAction::AttachPatternAsModulation(pattern_id),
                format!("+ Attach '{pat_name}' as global Modulation…"),
                "modulate Rack, FX, Channel, or Macro".into(),
            ));
            if !project.parts.is_empty() {
                entries.push((
                    LinkMenuAction::PickPartForPatternLink(pattern_id),
                    format!("+ Attach '{pat_name}' as Part P-lock…"),
                    "link to a Part's instrument parameter".into(),
                ));
            }
        }

        if entries.is_empty() {
            self.status = format!("'{pat_name}' has no downstream consumers");
            return Ok(Outcome::default());
        }
        self.open_link_menu_picker(format!("CONSUMERS · {pat_name}"), entries)
    }

    /// Unlink a Part `parameter_link` by index in one undoable edit (`x` on `Detail::Part` or menu).
    pub(crate) fn unlink_part_param_link(&mut self, part_id: Id, index: usize) -> Result<Outcome> {
        let mut part = self.part(part_id).context("Part missing")?.clone();
        ensure!(index < part.parameter_links.len(), "Parameter link missing");
        let removed = part.parameter_links.remove(index);
        let plbl = self.format_instrument_param_short(part.instrument, removed.target);
        let pname = part.name.clone();
        self.commit(
            vec![Edit::PutPart { part }],
            format!("Unlinked P-lock {plbl} from '{pname}' · u restores"),
        )
    }

    /// Execute the selected `LinkMenuAction` from `Purpose::LinkMenu`.
    pub(crate) fn execute_link_menu_action(&mut self, action: LinkMenuAction) -> Result<Outcome> {
        match action {
            LinkMenuAction::JumpToMacro { index, binding } => {
                self.push_back();
                self.view_state.macro_cursor = usize::from(index);
                self.view_state.macro_binding_cursor = binding;
                self.go_to_deck(layout::Deck::song(layout::DeckKind::Macros));
                Ok(Outcome::default())
            }
            LinkMenuAction::JumpToPattern(id) => {
                self.select_pattern(id)?;
                Ok(Outcome::default())
            }
            LinkMenuAction::JumpToParam(addr) => {
                self.jump_to_param_address(addr)?;
                Ok(Outcome::default())
            }
            LinkMenuAction::JumpToPart { part, link_index } => {
                self.push_back();
                self.selected_part = Some(part);
                self.show_detail(Detail::Part(part));
                if let Some(idx) = link_index {
                    if let Some(pos) = self.control_descriptors().iter().position(|c| {
                        matches!(c.target, ControlTarget::PartParamLink { part: p, index: i, .. } if p == part && i == idx)
                    }) {
                        self.control_cursor = pos;
                    }
                }
                Ok(Outcome::default())
            }
            LinkMenuAction::JumpToEffect(device) => {
                self.open_effect(device)?;
                Ok(Outcome::default())
            }
            LinkMenuAction::PickMacroSlotForParam(target) => {
                let (_, tname) = self.format_macro_target(target);
                let mut entries = Vec::with_capacity(MAX_PROJECT_MACROS);
                for i in 0..MAX_PROJECT_MACROS {
                    let slot = i as u8;
                    let (name, detail) = if let Some(mac) = self.project().macro_control(slot) {
                        let already = mac.bindings.iter().any(|b| b.target == target);
                        let det = if already {
                            format!("{} bindings · [already bound]", mac.bindings.len())
                        } else {
                            format!("{} / {MAX_MACRO_BINDINGS} bindings", mac.bindings.len())
                        };
                        (format!("M{} · {}", i + 1, mac.name), det)
                    } else {
                        (
                            format!("M{} · {}", i + 1, MacroControl::default_name(slot)),
                            "unassigned".into(),
                        )
                    };
                    entries.push((
                        LinkMenuAction::BindParamToMacroSlot { target, slot },
                        name,
                        detail,
                    ));
                }
                self.open_link_menu_picker(format!("BIND TO MACRO · {tname}"), entries)
            }
            LinkMenuAction::BindParamToMacroSlot { target, slot } => {
                self.apply(Action::BindMacro {
                    index: slot,
                    target,
                    min: 0.0,
                    max: 1.0,
                })
            }
            LinkMenuAction::UnbindMacro { index, binding } => self.apply(Action::UnbindMacro {
                index,
                binding: Some(binding),
            }),
            LinkMenuAction::AttachModulation { target, pattern } => {
                let mut ctx = self.workflow_context();
                ctx.mod_target = Some(target);
                if let Some(pid) = pattern {
                    ctx.pattern = Some(pid);
                }
                if let ModulationTarget::Macro { index } = target {
                    ctx.macro_slot = Some(index);
                }
                self.begin_workflow_with_context(WorkflowKind::Modulation, ctx)
            }
            LinkMenuAction::EditModulation(id) => {
                self.begin_workflow(WorkflowKind::EditModulation(id))
            }
            LinkMenuAction::UnlinkModulation(id) => {
                let name = self
                    .project()
                    .modulations
                    .iter()
                    .find(|m| m.id == id)
                    .map_or_else(|| format!("#{id}"), |m| m.name.clone());
                self.commit(
                    vec![Edit::RemoveModulation { id }],
                    format!("Unlinked modulation '{name}' · u restores"),
                )
            }
            LinkMenuAction::PickPartForParamLink { instrument, target } => {
                let plbl = self.format_instrument_param_short(instrument, target);
                let entries: Vec<_> = self
                    .project()
                    .parts
                    .iter()
                    .filter(|p| p.instrument == instrument)
                    .map(|p| {
                        let linked = p
                            .parameter_links
                            .iter()
                            .find(|l| l.target == target)
                            .and_then(|l| self.pattern(l.pattern))
                            .map(|pat| format!("linked → {}", pat.name))
                            .unwrap_or_else(|| format!("{} P-locks", p.parameter_links.len()));
                        (
                            LinkMenuAction::PickPatternForPartLink { part: p.id, target },
                            p.name.clone(),
                            linked,
                        )
                    })
                    .collect();
                self.open_link_menu_picker(format!("P-LOCK PART · {plbl}"), entries)
            }
            LinkMenuAction::PickPatternForPartLink { part, target } => {
                let part_obj = self.part(part).context("Part missing")?;
                let part_name = part_obj.name.clone();
                let plbl = self.format_instrument_param_short(part_obj.instrument, target);
                let current_pat = part_obj
                    .parameter_links
                    .iter()
                    .find(|l| l.target == target)
                    .map(|l| l.pattern);
                let mut entries: Vec<(LinkMenuAction, String, String)> = self
                    .project()
                    .patterns
                    .iter()
                    .filter(|p| {
                        self.project().pattern_type(p.id).ok()
                            == Some(PatternType::Values(ValueUnit::Normalized))
                    })
                    .map(|p| {
                        let suffix = if current_pat == Some(p.id) {
                            " · [current]"
                        } else {
                            ""
                        };
                        (
                            LinkMenuAction::ApplyPartLink {
                                part,
                                target,
                                pattern: Some(p.id),
                            },
                            p.name.clone(),
                            format!("Values · {} steps{suffix}", self.pattern_period(p.id)),
                        )
                    })
                    .collect();
                entries.push((
                    LinkMenuAction::ApplyPartLink {
                        part,
                        target,
                        pattern: None,
                    },
                    "+ New Values pattern".into(),
                    "16-step normalized Ramp curve".into(),
                ));
                self.open_link_menu_picker(
                    format!("P-LOCK PATTERN · {part_name} · {plbl}"),
                    entries,
                )
            }
            LinkMenuAction::PickParamForPartLink {
                part,
                replace_index,
                preset_pattern,
            } => self.open_part_link_param_picker(part, replace_index, preset_pattern),
            LinkMenuAction::ApplyPartLink {
                part: part_id,
                target,
                pattern,
            } => {
                let mut part = self.part(part_id).context("Part missing")?.clone();
                let plbl = self.format_instrument_param_short(part.instrument, target);
                let mut edits = Vec::new();
                let pat_id = match pattern {
                    Some(pid) => pid,
                    None => {
                        let pid = self.doc.fresh_ids(1)?[0];
                        let pat_name = crate::unique_name(
                            &format!("{} {plbl}", part.name),
                            self.project().patterns.iter().map(|p| p.name.as_str()),
                            Some(120),
                        );
                        edits.push(Edit::PutPattern {
                            pattern: Pattern {
                                id: pid,
                                name: pat_name,
                                kind: PatternKind::Values {
                                    unit: ValueUnit::Normalized,
                                    period: 16,
                                    mode: ValueMode::Ramp,
                                    points: vec![ValuePoint {
                                        step: 0,
                                        value: 0.5,
                                    }],
                                },
                            },
                        });
                        pid
                    }
                };
                if let Some(existing) = part.parameter_links.iter_mut().find(|l| l.target == target)
                {
                    existing.pattern = pat_id;
                } else {
                    ensure!(
                        part.parameter_links.len() < MAX_LINKS,
                        "Part '{}' already has the maximum {MAX_LINKS} P-lock links",
                        part.name
                    );
                    part.parameter_links.push(ParameterLink {
                        target,
                        pattern: pat_id,
                    });
                }
                let pname = part.name.clone();
                edits.push(Edit::PutPart { part });
                let out = self.commit(edits, format!("Linked P-lock {pname} · {plbl}"))?;
                if pattern.is_none() {
                    self.select_pattern(pat_id)?;
                }
                Ok(out)
            }
            LinkMenuAction::ReplacePartLinkTarget {
                part: part_id,
                index,
                new_target,
            } => {
                let mut part = self.part(part_id).context("Part missing")?.clone();
                ensure!(index < part.parameter_links.len(), "Parameter link missing");
                let pat_id = part.parameter_links[index].pattern;
                ensure!(
                    !part
                        .parameter_links
                        .iter()
                        .enumerate()
                        .any(|(i, l)| i != index && l.target == new_target),
                    "Part '{}' already has a P-lock on that parameter",
                    part.name
                );
                part.parameter_links[index] = ParameterLink {
                    target: new_target,
                    pattern: pat_id,
                };
                let plbl = self.format_instrument_param_short(part.instrument, new_target);
                let pname = part.name.clone();
                self.commit(
                    vec![Edit::PutPart { part }],
                    format!("Retargeted P-lock on {pname} → {plbl}"),
                )
            }
            LinkMenuAction::UnlinkPartLink { part, index } => {
                self.unlink_part_param_link(part, index)
            }
            LinkMenuAction::UnlinkFilterEnv { device } => {
                let edit = self.effect_edit(device, |d| {
                    if let EffectKind::Filter(fp) = &mut d.kind {
                        fp.env_pattern_id = None;
                    }
                    Ok(())
                })?;
                self.commit(
                    vec![edit],
                    "Unlinked filter envelope pattern · u restores".into(),
                )
            }
            LinkMenuAction::AttachPatternAsModulation(pat_id) => {
                let mut ctx = WorkflowContext {
                    pattern: Some(pat_id),
                    ..self.workflow_context()
                };
                // Avoid pre-filling an unrelated channel when invoked from Pattern/Bank.
                if !self.mixer_canvas_focused() {
                    ctx.mixer_channel = None;
                }
                self.begin_workflow_with_context(WorkflowKind::Modulation, ctx)
            }
            LinkMenuAction::PickPartForPatternLink(pat_id) => {
                let pat_name = self
                    .pattern(pat_id)
                    .map_or_else(|| format!("Pattern #{pat_id}"), |p| p.name.clone());
                let entries: Vec<_> = self
                    .project()
                    .parts
                    .iter()
                    .map(|p| {
                        let iname = self
                            .instrument(p.instrument)
                            .map_or("?", |i| i.name.as_str());
                        (
                            LinkMenuAction::PickParamForPartLink {
                                part: p.id,
                                replace_index: None,
                                preset_pattern: Some(pat_id),
                            },
                            p.name.clone(),
                            format!("{iname} · {} P-locks", p.parameter_links.len()),
                        )
                    })
                    .collect();
                self.open_link_menu_picker(format!("P-LOCK PART · {pat_name}"), entries)
            }
        }
    }

    /// Unlink the focused link control row (`ModulationLink`, `PartParamLink`, or `MacroBinding`).
    pub(crate) fn unlink_focused_control_link(&mut self) -> Option<Result<Outcome>> {
        let target = self.selected_control().map(|c| c.target)?;
        match target {
            ControlTarget::PartParamLink { part, index, .. } => {
                Some(self.unlink_part_param_link(part, index))
            }
            ControlTarget::ModulationLink { id, .. } => {
                let name = self
                    .project()
                    .modulations
                    .iter()
                    .find(|m| m.id == id)
                    .map_or_else(|| format!("#{id}"), |m| m.name.clone());
                Some(self.commit(
                    vec![Edit::RemoveModulation { id }],
                    format!("Unlinked modulation '{name}' · u restores"),
                ))
            }
            ControlTarget::MacroBinding { index, binding } => {
                Some(self.apply(Action::UnbindMacro {
                    index,
                    binding: Some(binding),
                }))
            }
            ControlTarget::FilterEnvSource(device) => {
                let has_custom = self.effect(device).is_some_and(|(_, d)| {
                    matches!(d.kind, EffectKind::Filter(fp) if fp.env_pattern_id.is_some())
                });
                if has_custom {
                    Some(self.set_filter_env_source(device, None))
                } else {
                    self.status = "Filter already uses Groove Bank · Enter picks a pattern".into();
                    Some(Ok(Outcome::default()))
                }
            }
            _ => None,
        }
    }
}
