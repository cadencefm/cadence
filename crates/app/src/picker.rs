//! Pickers: a list over the working deck that filters as you type after `/`,
//! chooses with a digit or Enter, and auditions with `A`.
use super::links_ui::LinkMenuAction;
use super::verbs::Replace;
use super::*;
use crate::view::GraphCable;
use cadence_core::macros::{MacroControl, MAX_MACRO_BINDINGS};
use cadence_core::rack::{
    cable_default_amount, compile_zone_graph, format_cable_amount, InPort, ModuleKindTag,
    PatchCable,
};
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub(super) enum Purpose {
    Replace {
        placement: Id,
        what: Replace,
    },
    ReplacePart {
        part: Id,
        what: Replace,
    },
    BankAddLanePart {
        lane: Id,
    },
    LaunchCycle,
    FilterEnvSource {
        device: Id,
    },
    GraphRouteChannel {
        channel: Id,
    },
    GraphRouteInstrument {
        instrument: Id,
    },
    GraphEditCable {
        cables: Vec<GraphCable>,
    },
    GraphDeleteCable {
        cables: Vec<GraphCable>,
    },
    GraphAdd,
    RackAddModule {
        instrument: Id,
        zone_id: u16,
        kinds: Vec<ModuleKindTag>,
    },
    RackAddZone {
        instrument: Id,
    },
    RackZonePreset {
        instrument: Id,
        replace_active: bool,
        preset_ids: Vec<String>,
    },
    RackConnectCable {
        instrument: Id,
        zone_id: u16,
        from_module: u16,
        targets: Vec<(u16, InPort)>,
    },
    RackRepatchCable {
        instrument: Id,
        zone_id: u16,
        cable_id: u16,
        targets: Vec<(u16, InPort)>,
    },
    RackSelectCable {
        instrument: Id,
        zone_id: u16,
        cables: Vec<u16>,
    },
    MacroBindTarget {
        slot: u8,
        targets: Vec<ParamAddress>,
    },
    LinkMenu {
        actions: Vec<LinkMenuAction>,
    },
    MixerAdd {
        channel: ChannelRef,
    },
    MidiPort {
        is_input: bool,
        ports: Vec<Option<String>>,
    },
    MidiProfile,
    MidiPadMode,
    MidiAddMapping {
        targets: Vec<(cadence_core::midi_map::MidiTarget, String)>,
    },
    AudioOutput {
        cue: bool,
        devices: Vec<Option<String>>,
    },
    AudioInput {
        devices: Vec<Option<String>>,
    },
    AudioSampleRate {
        rates: Vec<Option<u32>>,
    },
    AudioBufferSize {
        sizes: Vec<Option<u32>>,
    },
    SliceToKit {
        instrument: Id,
        zone: u16,
        module: u16,
    },
    SampleEdit {
        instrument: Id,
        zone: u16,
        module: u16,
    },
}

impl Purpose {
    fn cancel_message(&self) -> &'static str {
        match self {
            Self::LaunchCycle => "Launch quantize unchanged",
            Self::FilterEnvSource { .. } => "Env source unchanged",
            Self::GraphRouteChannel { .. } | Self::GraphRouteInstrument { .. } => {
                "Routing unchanged"
            }
            Self::GraphEditCable { .. }
            | Self::GraphDeleteCable { .. }
            | Self::RackConnectCable { .. }
            | Self::RackRepatchCable { .. }
            | Self::RackSelectCable { .. } => "Cables unchanged",
            Self::RackAddModule { .. } => "No module added",
            Self::BankAddLanePart { .. } => "No part added to lane",
            Self::MacroBindTarget { .. } => "No binding added",
            Self::LinkMenu { .. } => "Links unchanged",
            Self::MixerAdd { .. } | Self::GraphAdd => "Nothing added",
            Self::MidiPort { .. } | Self::MidiProfile | Self::MidiPadMode => {
                "MIDI settings unchanged"
            }
            Self::MidiAddMapping { .. } => "No MIDI mapping added",
            Self::AudioOutput { .. }
            | Self::AudioInput { .. }
            | Self::AudioSampleRate { .. }
            | Self::AudioBufferSize { .. } => "Audio device settings unchanged",
            Self::RackAddZone { .. } | Self::RackZonePreset { .. } => "Zones unchanged",
            Self::SliceToKit { .. } => "Slice to Rack Kit cancelled",
            Self::SampleEdit { .. } => "Sample edit cancelled",
            Self::Replace { .. } | Self::ReplacePart { .. } => "Nothing replaced",
        }
    }

    fn supports_audition(&self) -> bool {
        matches!(
            self,
            Self::Replace { .. }
                | Self::ReplacePart { .. }
                | Self::BankAddLanePart { .. }
                | Self::LaunchCycle
                | Self::FilterEnvSource { .. }
        )
    }
}

#[derive(Clone, Debug)]
pub(super) struct Picker {
    purpose: Purpose,
    title: String,
    /// Each choice: its ID, name and a short description.
    items: Vec<(Id, String, String)>,
    current: Option<Id>,
    query: String,
    searching: bool,
    cursor: usize,
}

/// What the UI shows for an open picker.
#[derive(Clone, Debug)]
pub struct PickerView {
    pub title: String,
    pub items: Vec<(String, String, bool)>,
    pub cursor: usize,
    pub filter: Option<String>,
    pub searching: bool,
    pub audition: bool,
}

impl Picker {
    fn visible(&self) -> Vec<&(Id, String, String)> {
        let trimmed = self.query.trim().to_lowercase();
        if trimmed.is_empty() {
            return self.items.iter().collect();
        }
        let tokens: Vec<&str> = trimmed.split_whitespace().collect();
        self.items
            .iter()
            .filter(|(_, name, _)| {
                let lower = name.to_lowercase();
                tokens.iter().all(|tok| lower.contains(tok))
            })
            .collect()
    }
}

impl App {
    fn replace_picker_items(&self, what: Replace) -> Vec<(Id, String, String)> {
        match what {
            Replace::Pattern => self
                .doc
                .project()
                .patterns
                .iter()
                .filter(|p| {
                    self.doc
                        .project()
                        .pattern_type(p.id)
                        .is_ok_and(|t| t == PatternType::Notes)
                })
                .map(|p| (p.id, p.name.clone(), kind_label(&p.kind).to_string()))
                .collect(),
            Replace::Instrument => self
                .doc
                .project()
                .instruments
                .iter()
                .map(|i| {
                    let zones = i.rack.zones.len();
                    let mods: usize = i.rack.zones.iter().map(|z| z.modules.len()).sum();
                    (i.id, i.name.clone(), format!("Rack · {zones}z/{mods}m"))
                })
                .collect(),
        }
    }

    pub(super) fn open_replace(&mut self, placement: Id, what: Replace) -> Result<Outcome> {
        let (lane, p) = self.placement(placement).context("Placement missing")?;
        let part = self.part(p.part).context("Part missing")?;
        let bar = p.start_step / 16 + 1;
        let (title, current) = match what {
            Replace::Pattern => (
                format!("REPLACE PATTERN · {} · bar {bar}", lane.name),
                part.pattern,
            ),
            Replace::Instrument => (
                format!("REPLACE SOUND · {} · bar {bar}", lane.name),
                part.instrument,
            ),
        };
        let items = self.replace_picker_items(what);
        let cursor = items
            .iter()
            .position(|(id, _, _)| *id == current)
            .unwrap_or(0);
        self.open_picker(Picker {
            purpose: Purpose::Replace { placement, what },
            title,
            items,
            current: Some(current),
            query: String::new(),
            searching: false,
            cursor,
        });
        Ok(Outcome::default())
    }

    pub(super) fn open_replace_part(&mut self, part_id: Id, what: Replace) -> Result<Outcome> {
        let part = self.part(part_id).context("Part missing")?;
        let part_name = part.name.clone();
        let (title, current) = match what {
            Replace::Pattern => (format!("REBIND PATTERN · {part_name}"), part.pattern),
            Replace::Instrument => (format!("REBIND SOUND · {part_name}"), part.instrument),
        };
        let items = self.replace_picker_items(what);
        let cursor = items
            .iter()
            .position(|(id, _, _)| *id == current)
            .unwrap_or(0);
        self.open_picker(Picker {
            purpose: Purpose::ReplacePart {
                part: part_id,
                what,
            },
            title,
            items,
            current: Some(current),
            query: String::new(),
            searching: false,
            cursor,
        });
        Ok(Outcome::default())
    }

    pub(super) fn open_bank_add_part_picker(&mut self, lane: Id) -> Result<Outcome> {
        let lane_obj = self.lane(lane).context("Lane missing")?;
        ensure!(
            lane_obj.parts.len() < cadence_core::material::MAX_LANE_PARTS,
            "Lane '{}' already has {} launchpad parts",
            lane_obj.name,
            cadence_core::material::MAX_LANE_PARTS
        );
        let lane_name = lane_obj.name.clone();
        let existing: HashSet<Id> = lane_obj.parts.iter().map(|e| e.part).collect();
        let mut items: Vec<(Id, String, String)> = self
            .doc
            .project()
            .parts
            .iter()
            .filter(|part| !existing.contains(&part.id))
            .map(|part| {
                let pat = self.pattern(part.pattern).map_or("?", |p| p.name.as_str());
                let inst = self
                    .instrument(part.instrument)
                    .map_or("?", |i| i.name.as_str());
                (part.id, part.name.clone(), format!("{pat} → {inst}"))
            })
            .collect();
        items.push((
            0,
            "[+] Create new Part…".into(),
            format!("Create a new Part and add it to {lane_name}"),
        ));
        self.open_picker(Picker {
            purpose: Purpose::BankAddLanePart { lane },
            title: format!("ADD PART TO LANE · {lane_name}"),
            items,
            current: None,
            query: String::new(),
            searching: false,
            cursor: 0,
        });
        Ok(Outcome::default())
    }

    pub(super) fn open_cycle_boundary_picker(&mut self) -> Result<Outcome> {
        let items: Vec<(Id, String, String)> = self
            .doc
            .project()
            .patterns
            .iter()
            .map(|p| {
                (
                    p.id,
                    p.name.clone(),
                    format!(
                        "{} · {} steps",
                        kind_label(&p.kind),
                        self.pattern_period(p.id)
                    ),
                )
            })
            .collect();
        ensure!(
            !items.is_empty(),
            "Create a pattern before selecting a Cycle launch boundary"
        );
        let current = match self.doc.project().timing.launch {
            cadence_core::material::Boundary::Cycle { pattern } => Some(pattern),
            _ => None,
        };
        let cursor = current
            .and_then(|cid| items.iter().position(|(id, _, _)| *id == cid))
            .unwrap_or(0);
        self.open_picker(Picker {
            purpose: Purpose::LaunchCycle,
            title: "LAUNCH QUANTIZE · CYCLE PATTERN".into(),
            items,
            current,
            query: String::new(),
            searching: false,
            cursor,
        });
        Ok(Outcome::default())
    }

    pub(super) fn open_filter_env_source_picker(&mut self, device: Id) -> Result<Outcome> {
        let (_, dev) = self.effect(device).context("Effect missing")?;
        let current_pat = match dev.kind {
            cadence_core::mixer::EffectKind::Filter(fp) => fp.env_pattern_id,
            _ => bail!("Only Filter effects support a rhythmic envelope source"),
        };
        let dev_name = dev.name.clone();
        let bank_count = self.assets.filter_grooves.len();
        let bank_desc = if bank_count == 0 {
            "Built-in bank (empty · neutral pulse)".into()
        } else {
            format!(
                "Built-in bank ({bank_count} groove{}) · P browses",
                if bank_count == 1 { "" } else { "s" }
            )
        };
        let mut items: Vec<(Id, String, String)> = vec![(0, "Groove Bank".into(), bank_desc)];
        for pid in self.filter_env_pattern_candidates() {
            if let Some(pat) = self.pattern(pid) {
                let period = self.pattern_period(pid);
                let detail = if period > 64 {
                    format!(
                        "{} · {period} steps (64-step window)",
                        kind_label(&pat.kind)
                    )
                } else {
                    format!("{} · {period} steps", kind_label(&pat.kind))
                };
                items.push((pid, pat.name.clone(), detail));
            }
        }
        let current = Some(current_pat.unwrap_or(0));
        let cursor = current
            .and_then(|cid| items.iter().position(|(id, _, _)| *id == cid))
            .unwrap_or(0);
        self.open_picker(Picker {
            purpose: Purpose::FilterEnvSource { device },
            title: format!("FILTER ENV SOURCE · {dev_name}"),
            items,
            current,
            query: String::new(),
            searching: false,
            cursor,
        });
        Ok(Outcome::default())
    }

    pub(super) fn open_graph_route_channel_picker(&mut self, channel: Id) -> Result<Outcome> {
        let ch = self.channel(channel).context("Channel missing")?;
        ensure!(
            ch.kind != ChannelKind::Return,
            "Return channels always route directly to Master"
        );
        let ch_name = ch.name.clone();
        let current_out = ch.output;
        let targets =
            cadence_core::mixer::valid_output_targets(&self.doc.project().mixer.channels, channel);
        let items: Vec<(Id, String, String)> = targets
            .into_iter()
            .map(|dest| match dest {
                ChannelRef::Master => (0, "Master".into(), "Master output bus".into()),
                ChannelRef::Channel(gid) => {
                    let gname = self.channel_name(ChannelRef::Channel(gid)).to_string();
                    let next = self
                        .channel(gid)
                        .map_or("Master".into(), |g| self.channel_name(g.output).to_string());
                    (gid, gname, format!("Group bus → {next}"))
                }
            })
            .collect();
        let current_id = match current_out {
            ChannelRef::Master => 0,
            ChannelRef::Channel(id) => id,
        };
        let cursor = items
            .iter()
            .position(|(id, _, _)| *id == current_id)
            .unwrap_or(0);
        self.open_picker(Picker {
            purpose: Purpose::GraphRouteChannel { channel },
            title: format!("ROUTE OUTPUT · {ch_name}"),
            items,
            current: Some(current_id),
            query: String::new(),
            searching: false,
            cursor,
        });
        Ok(Outcome::default())
    }

    pub(super) fn open_graph_route_instrument_picker(&mut self, instrument: Id) -> Result<Outcome> {
        let inst = self.instrument(instrument).context("Instrument missing")?;
        let inst_name = inst.name.clone();
        let current_ch = inst.channel;
        let items: Vec<(Id, String, String)> = self
            .doc
            .project()
            .mixer
            .channels
            .iter()
            .filter(|c| c.kind == ChannelKind::Audio)
            .map(|c| {
                (
                    c.id,
                    c.name.clone(),
                    format!("Audio channel → {}", self.channel_name(c.output)),
                )
            })
            .collect();
        ensure!(
            !items.is_empty(),
            "No Audio channels available for instrument routing"
        );
        let cursor = items
            .iter()
            .position(|(id, _, _)| *id == current_ch)
            .unwrap_or(0);
        self.open_picker(Picker {
            purpose: Purpose::GraphRouteInstrument { instrument },
            title: format!("ROUTE INSTRUMENT · {inst_name}"),
            items,
            current: Some(current_ch),
            query: String::new(),
            searching: false,
            cursor,
        });
        Ok(Outcome::default())
    }

    pub(super) fn open_graph_cable_picker(
        &mut self,
        title: String,
        entries: Vec<(GraphCable, String, String)>,
        delete: bool,
    ) -> Result<Outcome> {
        let mut cables = Vec::with_capacity(entries.len());
        let mut items = Vec::with_capacity(entries.len());
        for (idx, (cable, name, detail)) in entries.into_iter().enumerate() {
            cables.push(cable);
            items.push((idx as Id, name, detail));
        }
        self.open_picker(Picker {
            purpose: if delete {
                Purpose::GraphDeleteCable { cables }
            } else {
                Purpose::GraphEditCable { cables }
            },
            title,
            items,
            current: None,
            query: String::new(),
            searching: false,
            cursor: 0,
        });
        Ok(Outcome::default())
    }

    pub(crate) fn open_rack_add_module_picker(&mut self, instrument: Id) -> Result<Outcome> {
        let inst = self.instrument(instrument).context("Instrument missing")?;
        let z_idx = inst.rack.clamp_zone_index(self.rack_ui.zone_cursor);
        let zone = inst.rack.zones.get(z_idx).context("Rack zone missing")?;
        ensure!(
            zone.modules.len() < cadence_core::rack::MAX_MODULES_PER_ZONE,
            "Zone '{}' already has the maximum {} modules",
            zone.name,
            cadence_core::rack::MAX_MODULES_PER_ZONE
        );
        let zone_id = zone.id;
        let zone_name = zone.name.clone();
        let kinds = ModuleKindTag::ADDABLE.to_vec();
        let items: Vec<(Id, String, String)> = kinds
            .iter()
            .enumerate()
            .map(|(i, &tag)| {
                let desc = match tag {
                    ModuleKindTag::Vco => "Oscillator · Sine/Tri/Saw/Pulse · FM & PW",
                    ModuleKindTag::Sampler => "Sample player · WAV loop/slice/pitch",
                    ModuleKindTag::Noise => "Noise source · White/Pink",
                    ModuleKindTag::Env => "ADSR / Hold-Decay envelope (CV modulator)",
                    ModuleKindTag::Lfo => "Low-frequency oscillator (free/sync CV)",
                    ModuleKindTag::Filter => "SVF/Acid/6dB filter · Cutoff/Res/CV",
                    ModuleKindTag::Vca => "Voltage-controlled amplifier / RingMod",
                    ModuleKindTag::Drive => "Waveshaper saturation & overdrive",
                    ModuleKindTag::LoFi => "Bitcrusher & sample-rate reducer",
                    ModuleKindTag::Resonator => {
                        "Modal physical resonator · String/Bar/Membrane/Plate"
                    }
                    ModuleKindTag::Out => "Voice output stage",
                };
                (i as Id, tag.label().to_string(), desc.to_string())
            })
            .collect();
        self.open_picker(Picker {
            purpose: Purpose::RackAddModule {
                instrument,
                zone_id,
                kinds,
            },
            title: format!("ADD MODULE · {zone_name}"),
            items,
            current: None,
            query: String::new(),
            searching: false,
            cursor: 0,
        });
        self.status = "Add module · j/k select · 1–9, 0 or Enter add · Esc cancel".into();
        Ok(Outcome::default())
    }

    pub(crate) fn open_rack_add_zone_picker(&mut self, instrument: Id) -> Result<Outcome> {
        let inst = self.instrument(instrument).context("Instrument missing")?;
        let inst_name = inst.name.clone();
        let active_zone = inst
            .rack
            .zones
            .get(self.active_zone_index(&inst.rack))
            .map(|z| z.name.clone())
            .unwrap_or_else(|| "Zone".into());
        let items = vec![
            (
                0,
                "Blank Sampler".into(),
                "New [Sampler → Out] zone ready for sample load".into(),
            ),
            (
                1,
                "Synth Pad".into(),
                "New fixed-pitch Synth drum/percussion voice on next free key".into(),
            ),
            (
                2,
                "Blank Modular".into(),
                "Clean Eurorack canvas (Out module ready to patch)".into(),
            ),
            (
                3,
                "Sample Browser…".into(),
                "Browse audio files and add a new sample-loaded zone".into(),
            ),
            (
                4,
                "From Preset (New Zone)…".into(),
                "Append any single-zone Rack preset as a new zone".into(),
            ),
            (
                5,
                format!("Load Preset into '{active_zone}'…"),
                "Replace the active zone's voice from a single-zone preset".into(),
            ),
            (
                6,
                "Import Sample Folder…".into(),
                "Scan a folder of audio files into key-mapped kit zones".into(),
            ),
        ];
        self.open_picker(Picker {
            purpose: Purpose::RackAddZone { instrument },
            title: format!("ADD ZONE · {inst_name}"),
            items,
            current: None,
            query: String::new(),
            searching: false,
            cursor: 0,
        });
        self.status = "Add zone · 1–7 or Enter choose · / filter · Esc cancel".into();
        Ok(Outcome::default())
    }

    pub(crate) fn open_rack_zone_preset_picker(
        &mut self,
        instrument: Id,
        replace_active: bool,
    ) -> Result<Outcome> {
        let inst = self.instrument(instrument).context("Instrument missing")?;
        let title = if replace_active {
            let zname = inst
                .rack
                .zones
                .get(self.active_zone_index(&inst.rack))
                .map_or("Zone", |z| z.name.as_str());
            format!("LOAD PRESET INTO ZONE · {zname}")
        } else {
            format!("ADD ZONE FROM PRESET · {}", inst.name)
        };
        let mut entries: Vec<(String, String, String)> = self
            .assets
            .presets
            .entries()
            .iter()
            .filter(|e| e.patch.zones.len() == 1)
            .map(|e| {
                let cat = e.category.as_deref().unwrap_or("Rack");
                let z0 = &e.patch.zones[0];
                (
                    e.id.clone(),
                    e.name.clone(),
                    format!("{cat} · {}m/{}c", z0.modules.len(), z0.cables.len()),
                )
            })
            .collect();
        entries.sort_by_key(|a| a.1.to_ascii_lowercase());
        ensure!(
            !entries.is_empty(),
            "No single-zone presets available in preset catalog"
        );
        let mut preset_ids = Vec::with_capacity(entries.len());
        let mut items = Vec::with_capacity(entries.len());
        for (idx, (id, name, detail)) in entries.into_iter().enumerate() {
            preset_ids.push(id);
            items.push((idx as Id, name, detail));
        }
        self.open_picker(Picker {
            purpose: Purpose::RackZonePreset {
                instrument,
                replace_active,
                preset_ids,
            },
            title,
            items,
            current: None,
            query: String::new(),
            searching: false,
            cursor: 0,
        });
        self.status = "Choose single-zone preset · / filter · Enter load · Esc cancel".into();
        Ok(Outcome::default())
    }

    #[allow(clippy::type_complexity)]
    fn rack_cable_target_candidates(
        zone: &cadence_core::rack::RackZone,
        from_module: u16,
        src_tag: ModuleKindTag,
        existing_cable: Option<PatchCable>,
    ) -> (Vec<(u16, InPort)>, Vec<(Id, String, String)>, Option<Id>) {
        let mut candidates: Vec<(u8, u16, InPort, String, String, bool)> = Vec::new();
        for dst in &zone.modules {
            let dst_tag = dst.kind.tag();
            for &port in dst_tag.in_ports() {
                let is_current =
                    existing_cable.is_some_and(|c| dst.id == c.to_module && port == c.to_port);
                let dup = zone.cables.iter().any(|c| {
                    existing_cable.is_none_or(|ec| c.id != ec.id)
                        && c.from_module == from_module
                        && c.to_module == dst.id
                        && c.to_port == port
                });
                if dup {
                    continue;
                }
                let default_amt = cable_default_amount(dst_tag, port);
                let mut probe = zone.clone();
                let probe_id = if let Some(ec) = existing_cable {
                    if let Some(c) = probe.cable_mut(ec.id) {
                        c.to_module = dst.id;
                        c.to_port = port;
                    }
                    ec.id
                } else {
                    let cid = probe.next_cable_id();
                    probe
                        .cables
                        .push(PatchCable::new(cid, from_module, dst.id, port, default_amt));
                    cid
                };
                let is_feedback = compile_zone_graph(&probe)
                    .ok()
                    .and_then(|g| {
                        g.cables
                            .iter()
                            .find(|c| c.cable_id == probe_id)
                            .map(|c| c.feedback)
                    })
                    .unwrap_or(false);
                let is_default = dst_tag.default_in_port(src_tag) == Some(port);
                let rank = if is_current {
                    0
                } else if dst.id == from_module {
                    4
                } else if is_feedback {
                    3
                } else if is_default {
                    1
                } else {
                    2
                };
                let mut detail = format!(
                    "{} · default {}",
                    port.label(),
                    format_cable_amount(dst_tag, port, default_amt)
                );
                if is_current {
                    detail.push_str(" (current)");
                } else if is_feedback {
                    detail.push_str(" [z-1]");
                } else if existing_cable.is_none() && is_default {
                    detail.push_str(" *");
                }
                candidates.push((
                    rank,
                    dst.id,
                    port,
                    format!("{}.{}", dst.name, port.short_label()),
                    detail,
                    is_current,
                ));
            }
        }
        candidates.sort_by_key(|(rank, _, _, _, _, _)| *rank);
        let mut targets = Vec::with_capacity(candidates.len());
        let mut items = Vec::with_capacity(candidates.len());
        let mut current = None;
        for (idx, (_, dst_id, port, name, detail, is_cur)) in candidates.into_iter().enumerate() {
            if is_cur {
                current = Some(idx as Id);
            }
            targets.push((dst_id, port));
            items.push((idx as Id, name, detail));
        }
        (targets, items, current)
    }

    pub(crate) fn open_rack_repatch_cable_picker(
        &mut self,
        instrument: Id,
        zone_id: u16,
        cable_id: u16,
    ) -> Result<Outcome> {
        let inst = self.instrument(instrument).context("Instrument missing")?;
        let zone = inst.rack.zone(zone_id).context("Rack zone missing")?;
        let cable = *zone.cable(cable_id).context("Patchbay cable missing")?;
        let src = zone
            .module(cable.from_module)
            .context("Source module missing")?;
        let src_name = src.name.clone();
        let (targets, items, current) = Self::rack_cable_target_candidates(
            zone,
            cable.from_module,
            src.kind.tag(),
            Some(cable),
        );
        ensure!(
            !items.is_empty(),
            "No valid destination ports available to re-patch c{cable_id}"
        );
        self.open_picker(Picker {
            purpose: Purpose::RackRepatchCable {
                instrument,
                zone_id,
                cable_id,
                targets,
            },
            title: format!("RE-PATCH c{cable_id} FROM {src_name} → TARGET.PORT"),
            items,
            current,
            query: String::new(),
            searching: false,
            cursor: 0,
        });
        self.status = format!(
            "Re-patch c{cable_id} from {src_name} (preserves pins & modulations) · Enter choose"
        );
        Ok(Outcome::default())
    }

    pub(crate) fn open_rack_connect_cable_picker(
        &mut self,
        instrument: Id,
        zone_id: u16,
        from_module: u16,
    ) -> Result<Outcome> {
        let inst = self.instrument(instrument).context("Instrument missing")?;
        let zone = inst.rack.zone(zone_id).context("Rack zone missing")?;
        let src = zone.module(from_module).context("Source module missing")?;
        ensure!(
            src.kind.has_output(),
            "Module '{}' ({}) has no output jack",
            src.name,
            src.kind.tag().label()
        );
        ensure!(
            zone.cables.len() < cadence_core::rack::MAX_CABLES_PER_ZONE,
            "Zone '{}' already has the maximum {} cables",
            zone.name,
            cadence_core::rack::MAX_CABLES_PER_ZONE
        );
        let src_name = src.name.clone();
        let zone_name = zone.name.clone();
        let (targets, items, _) =
            Self::rack_cable_target_candidates(zone, from_module, src.kind.tag(), None);
        ensure!(
            !items.is_empty(),
            "No valid input ports available in zone '{zone_name}'"
        );
        self.open_picker(Picker {
            purpose: Purpose::RackConnectCable {
                instrument,
                zone_id,
                from_module,
                targets,
            },
            title: format!("PATCH FROM {src_name} → TARGET.PORT"),
            items,
            current: None,
            query: String::new(),
            searching: false,
            cursor: 0,
        });
        self.status = format!("Patch from {src_name} · select target.port · Enter sets amount");
        Ok(Outcome::default())
    }

    pub(crate) fn open_rack_select_cable_picker(
        &mut self,
        instrument: Id,
        zone_id: u16,
        attached: &[u16],
    ) -> Result<Outcome> {
        let inst = self.instrument(instrument).context("Instrument missing")?;
        let zone = inst.rack.zone(zone_id).context("Rack zone missing")?;
        let compiled = compile_zone_graph(zone).ok();
        let mut cables = Vec::new();
        let mut items = Vec::new();
        for &cid in attached {
            let Some(cable) = zone.cable(cid) else {
                continue;
            };
            let src_name = zone
                .module(cable.from_module)
                .map_or("?", |m| m.name.as_str());
            let (dst_name, dst_tag) = zone
                .module(cable.to_module)
                .map_or(("?", ModuleKindTag::Out), |m| {
                    (m.name.as_str(), m.kind.tag())
                });
            let is_fb = compiled
                .as_ref()
                .and_then(|g| g.cables.iter().find(|c| c.cable_id == cid))
                .is_some_and(|c| c.feedback);
            let mut detail = format_cable_amount(dst_tag, cable.to_port, cable.amount);
            if is_fb {
                detail.push_str(" [z-1]");
            }
            let idx = cables.len() as Id;
            cables.push(cid);
            items.push((
                idx,
                format!(
                    "#{cid} {src_name} → {dst_name}.{}",
                    cable.to_port.short_label()
                ),
                detail,
            ));
        }
        ensure!(!cables.is_empty(), "No attached cables found");
        self.open_picker(Picker {
            purpose: Purpose::RackSelectCable {
                instrument,
                zone_id,
                cables,
            },
            title: "EDIT PATCH CABLE AMOUNT".into(),
            items,
            current: None,
            query: String::new(),
            searching: false,
            cursor: 0,
        });
        self.status = "Select cable to edit amount · Enter edits · Esc cancel".into();
        Ok(Outcome::default())
    }

    pub(crate) fn open_macro_bind_picker(&mut self, slot: u8) -> Result<Outcome> {
        let existing_bindings: Vec<ParamAddress> = self
            .project()
            .macro_control(slot)
            .map(|m| m.bindings.iter().map(|b| b.target).collect())
            .unwrap_or_default();
        ensure!(
            existing_bindings.len() < MAX_MACRO_BINDINGS,
            "Macro M{} already has the maximum {MAX_MACRO_BINDINGS} bindings",
            usize::from(slot) + 1
        );
        let all = self.all_bindable_macro_targets();
        ensure!(
            !all.is_empty(),
            "No bindable parameters exist in this project yet"
        );
        let mname = self
            .project()
            .macro_control(slot)
            .map_or_else(|| MacroControl::default_name(slot), |m| m.name.clone());
        let mut targets = Vec::with_capacity(all.len());
        let mut items = Vec::with_capacity(all.len());
        for (addr, name, cur) in all {
            let detail = if existing_bindings.contains(&addr) {
                format!("{cur} · [bound]")
            } else {
                cur
            };
            let idx = targets.len() as Id;
            targets.push(addr);
            items.push((idx, name, detail));
        }
        self.open_picker(Picker {
            purpose: Purpose::MacroBindTarget { slot, targets },
            title: format!("BIND TARGET → M{} {mname}", usize::from(slot) + 1),
            items,
            current: None,
            query: String::new(),
            searching: false,
            cursor: 0,
        });
        self.status = format!(
            "Bind parameter to M{} · / fuzzy search · j/k select · Enter binds · Esc cancel",
            usize::from(slot) + 1
        );
        Ok(Outcome::default())
    }

    pub(crate) fn open_link_menu_picker(
        &mut self,
        title: String,
        entries: Vec<(LinkMenuAction, String, String)>,
    ) -> Result<Outcome> {
        let mut actions = Vec::with_capacity(entries.len());
        let mut items = Vec::with_capacity(entries.len());
        for (idx, (action, name, detail)) in entries.into_iter().enumerate() {
            actions.push(action);
            items.push((idx as Id, name, detail));
        }
        self.open_picker(Picker {
            purpose: Purpose::LinkMenu { actions },
            title,
            items,
            current: None,
            query: String::new(),
            searching: false,
            cursor: 0,
        });
        self.status = "Links · 1–9 / Enter choose · / filter · Esc cancel".into();
        Ok(Outcome::default())
    }

    pub(crate) fn open_graph_add_picker(&mut self) -> Result<Outcome> {
        self.open_picker(Picker {
            purpose: Purpose::GraphAdd,
            title: "ADD GRAPH CONNECTION".into(),
            items: vec![
                (
                    0,
                    "Add Channel Send…".into(),
                    "Route an Audio or Group channel to a Return bus (s)".into(),
                ),
                (
                    1,
                    "Add Modulation…".into(),
                    "Connect a Values pattern to any Rack, Effect, Channel, or Macro target (m)"
                        .into(),
                ),
                (
                    2,
                    "Add Sidechain Duck…".into(),
                    "Trigger gain ducking on a channel from a Part (d)".into(),
                ),
            ],
            query: String::new(),
            searching: false,
            cursor: 0,
            current: None,
        });
        self.status = "Add Graph connection · j/k select · 1–3/Enter choose · Esc cancel".into();
        Ok(Outcome::default())
    }

    pub(crate) fn open_mixer_add_picker(&mut self, channel: ChannelRef) -> Result<Outcome> {
        let ch_name = self.channel_name(channel).to_string();
        let can_send = match channel {
            ChannelRef::Master => false,
            ChannelRef::Channel(cid) => self
                .channel(cid)
                .is_some_and(|c| c.kind != ChannelKind::Return),
        };
        let mut items: Vec<(Id, String, String)> = vec![(
            0,
            "Add Insert Effect…".into(),
            format!(
                "Insert EQ, Filter, Comp, Drive, Delay, Chorus, Reverb, or Limiter on {ch_name}"
            ),
        )];
        if can_send {
            items.push((
                1,
                "Add Channel Send…".into(),
                format!("Route {ch_name} to a Return bus"),
            ));
        }
        items.push((
            2,
            "Add Sidechain Duck…".into(),
            format!("Configure part-triggered ducking on {ch_name}"),
        ));
        items.push((
            3,
            "Add Mixer Channel…".into(),
            "Create a new Audio, Group, or Return channel".into(),
        ));
        self.open_picker(Picker {
            purpose: Purpose::MixerAdd { channel },
            title: format!("ADD TO MIXER · {ch_name}"),
            items,
            current: None,
            query: String::new(),
            searching: false,
            cursor: 0,
        });
        self.status = "Add to mixer · 1–4 / Enter choose · / filter · Esc cancel".into();
        Ok(Outcome::default())
    }

    pub(crate) fn open_midi_port_picker(&mut self, is_input: bool) -> Result<Outcome> {
        let (available, connected, title) = if is_input {
            (
                self.midi.in_ports.clone(),
                self.midi.port.clone(),
                "MIDI INPUT PORT",
            )
        } else {
            (
                self.midi.out_ports.clone(),
                self.midi.out_port.clone(),
                "MIDI OUTPUT PORT",
            )
        };
        let mut ports: Vec<Option<String>> = vec![None];
        let mut items: Vec<(Id, String, String)> = vec![(
            0,
            "Off (Disconnected)".into(),
            if is_input {
                "Disable MIDI input device".into()
            } else {
                "Disable MIDI output LED/OLED feedback".into()
            },
        )];
        for (idx, name) in available.into_iter().enumerate() {
            let id = (idx + 1) as Id;
            let detail = if connected.as_deref() == Some(name.as_str()) {
                "Connected".into()
            } else {
                "Available OS MIDI port".into()
            };
            ports.push(Some(name.clone()));
            items.push((id, name, detail));
        }
        let current = ports
            .iter()
            .position(|p| p == &connected)
            .map(|i| i as Id)
            .unwrap_or(0);
        self.open_picker(Picker {
            purpose: Purpose::MidiPort { is_input, ports },
            title: title.into(),
            items,
            current: Some(current),
            query: String::new(),
            searching: false,
            cursor: current as usize,
        });
        self.status = format!("{title} · j/k select · Enter connect · / filter · Esc cancel");
        Ok(Outcome::default())
    }

    pub(crate) fn open_midi_profile_picker(&mut self) -> Result<Outcome> {
        use cadence_core::midi_map::MidiProfile;
        let mut items: Vec<(Id, String, String)> = MidiProfile::BUILTIN_IDS
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let prof = MidiProfile::by_id(id);
                (
                    i as Id,
                    prof.name,
                    format!(
                        "{} bindings · feedback: {}",
                        prof.mappings.len(),
                        prof.feedback.label()
                    ),
                )
            })
            .collect();
        items.push((
            100,
            "Load Custom Profile (.midi.json)…".into(),
            "Browse or import a custom controller profile JSON via File Hub".into(),
        ));
        items.push((
            101,
            "Save Active Profile (.midi.json)…".into(),
            "Export active profile and user MIDI Learn mappings via File Hub".into(),
        ));
        let current = if self.midi.custom_profile.is_some() {
            None
        } else {
            MidiProfile::BUILTIN_IDS
                .iter()
                .position(|&id| id == self.config.midi_profile)
                .map(|i| i as Id)
        };
        let cursor = current.unwrap_or(0) as usize;
        self.open_picker(Picker {
            purpose: Purpose::MidiProfile,
            title: "MIDI CONTROLLER PROFILE".into(),
            items,
            current,
            query: String::new(),
            searching: false,
            cursor,
        });
        self.status = "MIDI Profile · 1–5 / Enter choose · / filter · Esc cancel".into();
        Ok(Outcome::default())
    }

    pub(crate) fn open_slice_to_kit_picker(
        &mut self,
        instrument: Id,
        zone: u16,
        module: u16,
    ) -> Result<Outcome> {
        let smp = self
            .sampler_module_at(instrument, zone, module)
            .context("Sampler module missing")?;
        ensure!(
            !smp.slices.is_empty(),
            "No slice markers yet · press d (auto), D (grid), or s (split) first"
        );
        let sample = self
            .loaded_sample_for(instrument, zone, module)
            .context("Load a sample before slicing to a Rack kit")?;
        let (crop_start, crop_end) =
            crate::sample_deck::effective_sampler_bounds(smp, sample.frames);
        let region_count =
            cadence_core::dsp::slice_regions(&smp.slices, crop_start, crop_end).len();
        let pad_count = region_count.min(cadence_core::rack::MAX_ZONES);
        let items = vec![
            (
                0,
                format!("New Rack Kit + Slice Pattern ({pad_count} pads)"),
                "Creates a new Audio channel, chromatic Rack Kit (C2+), sequential Pattern & Arrange Placement".into(),
            ),
            (
                1,
                format!("New Rack Kit Only ({pad_count} pads)"),
                "Creates a new Audio channel and chromatic Rack Kit (C2+) with choke group 1".into(),
            ),
            (
                2,
                format!("Replace Current Rack with Sliced Kit ({pad_count} pads)"),
                "Replaces this instrument's Rack zones with the sliced kit (single undoable step)".into(),
            ),
        ];
        self.open_picker(Picker {
            purpose: Purpose::SliceToKit {
                instrument,
                zone,
                module,
            },
            title: format!("SLICE TO RACK KIT · {} ({pad_count} pads)", sample.name),
            items,
            current: Some(0),
            query: String::new(),
            searching: false,
            cursor: 0,
        });
        self.status = "Slice to Rack Kit · 1–3 / Enter choose · Esc cancel".into();
        Ok(Outcome::default())
    }

    pub(crate) fn open_sample_edit_picker(
        &mut self,
        instrument: Id,
        zone: u16,
        module: u16,
    ) -> Result<Outcome> {
        let smp = self
            .sampler_module_at(instrument, zone, module)
            .context("Sampler module missing")?;
        let sample = self
            .loaded_sample_for(instrument, zone, module)
            .context("Load a sample first (press B)")?;
        let total = sample.frames;
        let sr = sample.sample_rate.max(8_000);
        let (crop_start, crop_end) = crate::sample_deck::effective_sampler_bounds(smp, total);
        let has_crop = crop_start > 0 || crop_end < total;
        let trim_desc = if has_crop {
            let dur = crate::sample_deck::format_sample_time(crop_end - crop_start, sr);
            format!("Crop ({dur}) → new WAV (u undoes)")
        } else {
            "Set crop bounds ([ / ]) first to trim".into()
        };
        let fi_frames = crate::sample_deck::compute_fade_frames_for_cursor(
            crop_start,
            crop_end,
            self.sample_ui.cursor_frame,
            sr,
            None,
            true,
        );
        let fo_frames = crate::sample_deck::compute_fade_frames_for_cursor(
            crop_start,
            crop_end,
            self.sample_ui.cursor_frame,
            sr,
            None,
            false,
        );
        let fi_dur = crate::sample_deck::format_sample_time(fi_frames, sr);
        let fo_dur = crate::sample_deck::format_sample_time(fo_frames, sr);
        let items = vec![
            (0, "Trim Crop → New WAV  [T]".into(), trim_desc),
            (
                1,
                format!(
                    "Normalize Peak ({:.1} dBFS)  [N]",
                    cadence_core::dsp::DEFAULT_NORMALIZE_TARGET_DBFS
                ),
                "Stereo-linked peak norm → new WAV".into(),
            ),
            (
                2,
                "Reverse Audio & Slices".into(),
                "Reverse audio, bounds & slices → WAV".into(),
            ),
            (
                3,
                format!("Fade In ({fi_dur})"),
                "Hann fade-in (cursor or default)".into(),
            ),
            (
                4,
                format!("Fade Out ({fo_dur})"),
                "Hann fade-out (cursor or default)".into(),
            ),
        ];
        self.open_picker(Picker {
            purpose: Purpose::SampleEdit {
                instrument,
                zone,
                module,
            },
            title: format!("OFFLINE SAMPLE EDIT · {} (new WAV · u undoes)", sample.name),
            items,
            current: Some(if has_crop { 0 } else { 1 }),
            query: String::new(),
            searching: false,
            cursor: if has_crop { 0 } else { 1 },
        });
        self.status = "Offline Sample Edit · 1–5 / Enter apply · Esc cancel".into();
        Ok(Outcome::default())
    }

    pub(crate) fn open_midi_pad_mode_picker(&mut self) -> Result<Outcome> {
        use cadence_core::midi_map::MidiPadMode;
        let items: Vec<(Id, String, String)> = MidiPadMode::ALL
            .iter()
            .enumerate()
            .map(|(i, &mode)| {
                let desc = match mode {
                    MidiPadMode::Notes => "Play Rack kits & instruments directly with pad velocity",
                    MidiPadMode::Sections => "Pads 1–8 jump directly to Song Sections 1–8",
                    MidiPadMode::Mutes => "Pads 1–8 toggle Lane Mutes 1–8",
                };
                (i as Id, mode.label().into(), desc.into())
            })
            .collect();
        let current = MidiPadMode::ALL
            .iter()
            .position(|&m| m == self.config.midi_pad_mode)
            .unwrap_or(0);
        self.open_picker(Picker {
            purpose: Purpose::MidiPadMode,
            title: "PERFORMANCE PAD MODE".into(),
            items,
            current: Some(current as Id),
            query: String::new(),
            searching: false,
            cursor: current,
        });
        self.status = "Pad Mode · 1–3 / Enter choose · Esc cancel".into();
        Ok(Outcome::default())
    }

    pub(crate) fn open_midi_add_mapping_picker(&mut self) -> Result<Outcome> {
        use cadence_core::midi_map::{MidiTarget, MidiTransportAction};
        let mut targets: Vec<(MidiTarget, String)> = Vec::new();
        let mut items: Vec<(Id, String, String)> = Vec::new();
        let mut push_target = |target: MidiTarget, name: String, detail: String| {
            let idx = targets.len() as Id;
            targets.push((target, name.clone()));
            items.push((idx, name, detail));
        };

        // 1. Transport actions
        for action in MidiTransportAction::ALL {
            push_target(
                MidiTarget::Transport { action },
                format!("Transport · {}", action.label()),
                "Hardware button / encoder transport control".into(),
            );
        }

        // 2. Smart Macros (Project Macros M1–M8 + Focused Smart Macros 1–8)
        for slot in 0..cadence_core::macros::MAX_PROJECT_MACROS {
            let mname = self.doc.project().macro_control(slot as u8).map_or_else(
                || cadence_core::macros::MacroControl::default_name(slot as u8),
                |m| m.name.clone(),
            );
            push_target(
                MidiTarget::ProjectMacro { slot },
                format!("Project Macro M{} · {mname}", slot + 1),
                "Fixed Project Macro dial M1–M8".into(),
            );
        }
        for slot in 0..cadence_core::macros::MAX_PROJECT_MACROS {
            push_target(
                MidiTarget::FocusedMacro { slot },
                format!("Smart Macro {} (Focused)", slot + 1),
                "Tracks focused Instrument/Effect/Channel or Project Macros".into(),
            );
        }

        // 3. Sections (all project sections, or slots 1–8)
        let sec_count = self.doc.project().sections.len().max(8);
        for index in 0..sec_count {
            let label = self.doc.project().sections.get(index).map_or_else(
                || format!("Section {}", index + 1),
                |s| format!("Section {} · {}", index + 1, s.name),
            );
            push_target(
                MidiTarget::LaunchSection { index },
                label,
                "Trigger section jump on launch boundary".into(),
            );
        }

        // 4. Master & Lanes (Gain / Mute / Solo / Bank Part launch)
        push_target(
            MidiTarget::Param {
                address: ParamAddress::Channel {
                    channel: ChannelRef::Master,
                    param: ChannelParam::Gain,
                },
            },
            "Master · Gain".into(),
            "Master output channel fader".into(),
        );
        let lane_count = self.doc.project().lanes.len().max(4);
        for lane_index in 0..lane_count {
            let lane_name = self
                .doc
                .project()
                .lanes
                .get(lane_index)
                .map_or_else(|| format!("Lane {}", lane_index + 1), |l| l.name.clone());
            push_target(
                MidiTarget::LaneGain { lane_index },
                format!("Lane {} Gain · {lane_name}", lane_index + 1),
                "Channel fader following arrangement lane order".into(),
            );
            push_target(
                MidiTarget::MuteLane { lane_index },
                format!("Mute Lane {} · {lane_name}", lane_index + 1),
                "Toggle lane mute".into(),
            );
            push_target(
                MidiTarget::SoloLane { lane_index },
                format!("Solo Lane {} · {lane_name}", lane_index + 1),
                "Toggle lane solo".into(),
            );
            if let Some(lane) = self.doc.project().lanes.get(lane_index) {
                for (part_index, lp) in lane.parts.iter().enumerate() {
                    let pname = self.part(lp.part).map_or("Part", |p| p.name.as_str());
                    push_target(
                        MidiTarget::LaunchLanePart {
                            lane_index,
                            part_index,
                        },
                        format!("Launch {lane_name} · {pname}"),
                        format!("Lane {} · Part slot {}", lane_index + 1, part_index + 1),
                    );
                }
            }
        }

        self.open_picker(Picker {
            purpose: Purpose::MidiAddMapping { targets },
            title: "ADD MIDI MAPPING".into(),
            items,
            current: None,
            query: String::new(),
            searching: false,
            cursor: 0,
        });
        self.status = "Add MIDI Mapping · / filter · Enter arms MIDI Learn · Esc cancel".into();
        Ok(Outcome::default())
    }

    pub(crate) fn open_audio_output_picker(&mut self, cue: bool) -> Result<Outcome> {
        let mut devices: Vec<Option<String>> = vec![None];
        let mut items: Vec<(Id, String, String)> = Vec::new();
        if cue {
            items.push((
                0,
                "Off".into(),
                "Disable dedicated cue output (reserved · future cue bus)".into(),
            ));
        } else {
            let detail = match &self.devices.inventory.active_output {
                Some(active) => format!("Follow OS default output (active: {active})"),
                None => "Follow OS default audio output device".into(),
            };
            items.push((0, "System Default".into(), detail));
        }
        for dev in &self.devices.inventory.outputs {
            let idx = devices.len() as Id;
            let mut tags = vec![format!("{} ch", dev.channels)];
            if dev.default_sample_rate > 0 {
                tags.push(crate::devices::format_sample_rate(dev.default_sample_rate));
            }
            if dev.is_default {
                tags.push("OS Default".into());
            }
            if !cue && self.devices.inventory.active_output.as_deref() == Some(dev.name.as_str()) {
                tags.push("Active".into());
            }
            devices.push(Some(dev.name.clone()));
            items.push((idx, dev.name.clone(), tags.join(" · ")));
        }
        let selected_name = if cue {
            &self.config.audio_cue_device
        } else {
            &self.config.audio_output_device
        };
        let current = devices
            .iter()
            .position(|d| d == selected_name)
            .map(|i| i as Id)
            .unwrap_or(0);
        let title = if cue {
            "AUDIO CUE OUTPUT"
        } else {
            "AUDIO OUTPUT DEVICE"
        };
        self.open_picker(Picker {
            purpose: Purpose::AudioOutput { cue, devices },
            title: title.into(),
            items,
            current: Some(current),
            query: String::new(),
            searching: false,
            cursor: current as usize,
        });
        self.status = format!("{title} · j/k select · Enter apply · / filter · Esc cancel");
        Ok(Outcome::default())
    }

    pub(crate) fn open_audio_input_picker(&mut self) -> Result<Outcome> {
        let mut devices: Vec<Option<String>> = vec![None];
        let mut items: Vec<(Id, String, String)> = vec![(
            0,
            "None".into(),
            "Disable audio input (no OS microphone stream opened)".into(),
        )];
        for dev in &self.devices.inventory.inputs {
            let idx = devices.len() as Id;
            let mut tags = vec![format!("{} ch", dev.channels)];
            if dev.default_sample_rate > 0 {
                tags.push(crate::devices::format_sample_rate(dev.default_sample_rate));
            }
            if dev.is_default {
                tags.push("OS Default".into());
            }
            devices.push(Some(dev.name.clone()));
            items.push((idx, dev.name.clone(), tags.join(" · ")));
        }
        let current = devices
            .iter()
            .position(|d| d == &self.config.audio_input_device)
            .map(|i| i as Id)
            .unwrap_or(0);
        self.open_picker(Picker {
            purpose: Purpose::AudioInput { devices },
            title: "AUDIO INPUT DEVICE".into(),
            items,
            current: Some(current),
            query: String::new(),
            searching: false,
            cursor: current as usize,
        });
        self.status = "AUDIO INPUT DEVICE · j/k select · Enter save · / filter · Esc cancel".into();
        Ok(Outcome::default())
    }

    pub(crate) fn open_audio_sample_rate_picker(&mut self) -> Result<Outcome> {
        let mut rates: Vec<Option<u32>> = vec![None];
        let active_sr = self.devices.inventory.active_sample_rate;
        let auto_detail = if active_sr > 0 {
            format!(
                "Use hardware device default (active: {})",
                crate::devices::format_sample_rate(active_sr)
            )
        } else {
            "Use hardware device default sample rate".into()
        };
        let mut items: Vec<(Id, String, String)> = vec![(0, "Auto (Device)".into(), auto_detail)];
        for sr in self.available_sample_rates() {
            let idx = rates.len() as Id;
            let detail = if active_sr == sr {
                format!("{sr} Hz · Active")
            } else {
                format!("{sr} Hz")
            };
            rates.push(Some(sr));
            items.push((idx, crate::devices::format_sample_rate(sr), detail));
        }
        let current = rates
            .iter()
            .position(|r| *r == self.config.audio_sample_rate)
            .map(|i| i as Id)
            .unwrap_or(0);
        self.open_picker(Picker {
            purpose: Purpose::AudioSampleRate { rates },
            title: "AUDIO SAMPLE RATE".into(),
            items,
            current: Some(current),
            query: String::new(),
            searching: false,
            cursor: current as usize,
        });
        self.status = "AUDIO SAMPLE RATE · j/k select · Enter apply · / filter · Esc cancel".into();
        Ok(Outcome::default())
    }

    pub(crate) fn open_audio_buffer_size_picker(&mut self) -> Result<Outcome> {
        let sr = self.effective_audio_sample_rate(0);
        let auto_fr = cadence_core::runtime::default_auto_buffer_frames(sr);
        let mut sizes: Vec<Option<u32>> = vec![None];
        let mut items: Vec<(Id, String, String)> = vec![(
            0,
            crate::devices::auto_buffer_label().into(),
            format!(
                "Default: {} at {}",
                crate::devices::format_buffer_latency(auto_fr, sr),
                crate::devices::format_sample_rate(sr)
            ),
        )];
        for frames in self.available_buffer_sizes() {
            let idx = sizes.len() as Id;
            let note = match frames {
                0..=128 => "Ultra-low latency · higher CPU sensitivity",
                129..=256 => "Low latency · responsive live performance",
                257..=512 => "Balanced studio latency & headroom",
                _ => "High safety headroom · prevents xruns",
            };
            sizes.push(Some(frames));
            items.push((
                idx,
                crate::devices::format_buffer_latency(frames, sr),
                note.into(),
            ));
        }
        let current = sizes
            .iter()
            .position(|s| *s == self.config.audio_buffer_size)
            .map(|i| i as Id)
            .unwrap_or(0);
        self.open_picker(Picker {
            purpose: Purpose::AudioBufferSize { sizes },
            title: "AUDIO BUFFER SIZE".into(),
            items,
            current: Some(current),
            query: String::new(),
            searching: false,
            cursor: current as usize,
        });
        self.status = "AUDIO BUFFER SIZE · j/k select · Enter apply · / filter · Esc cancel".into();
        Ok(Outcome::default())
    }

    pub(super) fn edit_graph_cable(&mut self, cable: GraphCable) -> Result<Outcome> {
        match cable {
            GraphCable::Send { source, target, .. } => {
                let mut ctx = self.workflow_context();
                ctx.mixer_channel = Some(ChannelRef::Channel(source));
                ctx.send_target = Some(target);
                self.begin_workflow_with_context(WorkflowKind::Send, ctx)
            }
            GraphCable::Modulation(id) => self.begin_workflow(WorkflowKind::EditModulation(id)),
            GraphCable::MacroBinding { index, binding } => {
                self.push_back();
                self.view_state.macro_cursor = usize::from(index);
                self.view_state.macro_binding_cursor = Some(binding);
                self.go_to_deck(layout::Deck::song(layout::DeckKind::Macros));
                Ok(Outcome::default())
            }
            GraphCable::Duck(id) => self.begin_workflow(WorkflowKind::EditDuck(id)),
            GraphCable::BusRoute { channel, .. } => self.open_graph_route_channel_picker(channel),
            GraphCable::FilterEnv { device, .. } => {
                self.open_effect(device)?;
                if let Some(pos) = self.control_descriptors().iter().position(
                    |c| matches!(c.target, ControlTarget::FilterEnvSource(d) if d == device),
                ) {
                    self.control_cursor = pos;
                }
                Ok(Outcome::default())
            }
            GraphCable::ParamLink { part, index, .. } => {
                self.push_back();
                self.selected_part = Some(part);
                self.show_detail(Detail::Part(part));
                if let Some(pos) = self.control_descriptors().iter().position(|c| {
                    matches!(c.target, ControlTarget::PartParamLink { part: p, index: i, .. } if p == part && i == index)
                }) {
                    self.control_cursor = pos;
                }
                Ok(Outcome::default())
            }
        }
    }

    fn open_picker(&mut self, picker: Picker) {
        self.modal = ActiveModal::Picker(Box::new(picker));
        self.input.mode = Mode::Pick;
    }

    pub fn picker_searching(&self) -> bool {
        self.modal.picker().is_some_and(|p| p.searching)
    }

    pub fn picker_query(&self) -> &str {
        self.modal.picker().map_or("", |p| p.query.as_str())
    }

    pub fn picker_view(&self) -> Option<PickerView> {
        let picker = self.modal.picker()?;
        let visible = picker.visible();
        Some(PickerView {
            title: picker.title.clone(),
            items: visible
                .iter()
                .map(|(id, name, detail)| {
                    (name.clone(), detail.clone(), Some(*id) == picker.current)
                })
                .collect(),
            cursor: picker.cursor.min(visible.len().saturating_sub(1)),
            filter: (picker.searching || !picker.query.is_empty()).then(|| picker.query.clone()),
            searching: picker.searching,
            audition: picker.purpose.supports_audition(),
        })
    }

    pub(super) fn handle_picker(&mut self, key: KeyEvent) -> Result<Outcome> {
        let Some(picker) = self.modal.picker_mut() else {
            self.input.mode = Mode::Normal;
            return Ok(Outcome::default());
        };
        let count = picker.visible().len();
        if picker.searching {
            match key.code {
                KeyCode::Esc => {
                    picker.searching = false;
                    picker.query.clear();
                    picker.cursor = 0;
                }
                KeyCode::Enter | KeyCode::Tab => {
                    picker.searching = false;
                    picker.cursor = picker.cursor.min(count.saturating_sub(1));
                }
                KeyCode::Backspace => {
                    picker.query.pop();
                    picker.cursor = 0;
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    picker.query.clear();
                    picker.cursor = 0;
                }
                KeyCode::Down => picker.cursor = (picker.cursor + 1).min(count.saturating_sub(1)),
                KeyCode::Up => picker.cursor = picker.cursor.saturating_sub(1),
                KeyCode::Char(c)
                    if !c.is_control()
                        && !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                        && picker.query.len() < 64 =>
                {
                    picker.query.push(c);
                    picker.cursor = 0;
                }
                _ => {}
            }
            return Ok(Outcome::default());
        }
        match key.code {
            KeyCode::Esc => {
                if !picker.query.is_empty() {
                    picker.query.clear();
                    picker.cursor = 0;
                    return Ok(Outcome::default());
                }
                let cancel_msg = self
                    .modal
                    .picker()
                    .map_or("Cancelled", |p| p.purpose.cancel_message());
                self.modal = ActiveModal::None;
                self.input.mode = Mode::Normal;
                let out = if self.is_auditioning() {
                    self.stop_audition()?
                } else {
                    Outcome::default()
                };
                self.status = cancel_msg.into();
                return Ok(out);
            }
            KeyCode::Char('/') => picker.searching = true,
            KeyCode::Char('j') | KeyCode::Down => {
                picker.cursor = (picker.cursor + 1).min(count.saturating_sub(1))
            }
            KeyCode::Char('k') | KeyCode::Up => picker.cursor = picker.cursor.saturating_sub(1),
            KeyCode::Char('g') => picker.cursor = 0,
            KeyCode::Char('$' | 'G') | KeyCode::End => picker.cursor = count.saturating_sub(1),
            KeyCode::PageUp => picker.cursor = picker.cursor.saturating_sub(6),
            KeyCode::PageDown => picker.cursor = (picker.cursor + 6).min(count.saturating_sub(1)),
            KeyCode::Char(c @ '1'..='9') => {
                return self.choose_picked(Some(c as usize - '1' as usize))
            }
            KeyCode::Char('0') => return self.choose_picked(Some(9)),
            KeyCode::Enter => return self.choose_picked(None),
            KeyCode::Char('A') => {
                let Some(picker) = self.modal.picker() else {
                    return Ok(Outcome::default());
                };
                if !picker.purpose.supports_audition() {
                    return Ok(Outcome::default());
                }
                let Some(chosen_id) = picker.visible().get(picker.cursor).map(|e| e.0) else {
                    return Ok(Outcome::default());
                };
                match picker.purpose {
                    Purpose::Replace {
                        what: Replace::Pattern,
                        ..
                    }
                    | Purpose::ReplacePart {
                        what: Replace::Pattern,
                        ..
                    } => return self.start_audition(chosen_id, None),
                    Purpose::Replace {
                        placement,
                        what: Replace::Instrument,
                    } => {
                        let pattern = self
                            .placement(placement)
                            .and_then(|(_, p)| self.part(p.part))
                            .map(|part| part.pattern)
                            .context("Placement pattern missing")?;
                        return self.start_audition_on(pattern, chosen_id, None);
                    }
                    Purpose::ReplacePart {
                        part,
                        what: Replace::Instrument,
                    } => {
                        let pattern = self
                            .part(part)
                            .map(|p| p.pattern)
                            .context("Part pattern missing")?;
                        return self.start_audition_on(pattern, chosen_id, None);
                    }
                    Purpose::BankAddLanePart { .. } => {
                        if chosen_id != 0 {
                            if let Some(part) = self.part(chosen_id) {
                                let (pat, inst) = (part.pattern, part.instrument);
                                return self.start_audition_on(pat, inst, None);
                            }
                        }
                    }
                    Purpose::LaunchCycle | Purpose::FilterEnvSource { .. } => {
                        if chosen_id != 0
                            && self
                                .doc
                                .project()
                                .pattern_type(chosen_id)
                                .is_ok_and(|t| t == PatternType::Notes)
                        {
                            return self.start_audition(chosen_id, None);
                        }
                    }
                    Purpose::GraphRouteChannel { .. }
                    | Purpose::GraphRouteInstrument { .. }
                    | Purpose::GraphEditCable { .. }
                    | Purpose::GraphDeleteCable { .. }
                    | Purpose::RackAddModule { .. }
                    | Purpose::RackAddZone { .. }
                    | Purpose::RackZonePreset { .. }
                    | Purpose::RackConnectCable { .. }
                    | Purpose::RackRepatchCable { .. }
                    | Purpose::RackSelectCable { .. }
                    | Purpose::MacroBindTarget { .. }
                    | Purpose::LinkMenu { .. }
                    | Purpose::MixerAdd { .. }
                    | Purpose::GraphAdd
                    | Purpose::MidiPort { .. }
                    | Purpose::MidiProfile
                    | Purpose::MidiPadMode
                    | Purpose::MidiAddMapping { .. }
                    | Purpose::AudioOutput { .. }
                    | Purpose::AudioInput { .. }
                    | Purpose::AudioSampleRate { .. }
                    | Purpose::AudioBufferSize { .. }
                    | Purpose::SliceToKit { .. }
                    | Purpose::SampleEdit { .. } => {}
                }
            }
            _ => {}
        }
        Ok(Outcome::default())
    }

    fn choose_picked(&mut self, index: Option<usize>) -> Result<Outcome> {
        let chosen = {
            let Some(picker) = self.modal.picker() else {
                self.input.mode = Mode::Normal;
                return Ok(Outcome::default());
            };
            let visible = picker.visible();
            let target_idx = index.unwrap_or(picker.cursor);
            match visible.get(target_idx) {
                Some((id, _, _)) => *id,
                None => {
                    if let Some(digit_idx) = index {
                        let n = digit_idx + 1;
                        self.status = if visible.is_empty() {
                            "No matching choices · Esc clears filter".into()
                        } else {
                            format!("Choice {n} out of range (1..={})", visible.len())
                        };
                    } else {
                        self.status = "No matching choices · Esc clears filter".into();
                    }
                    return Ok(Outcome::default());
                }
            }
        };
        let picker = self.modal.take_picker().context("No picker")?;
        self.input.mode = Mode::Normal;
        let mut out = if self.is_auditioning() {
            self.stop_audition()?
        } else {
            Outcome::default()
        };
        let edit = match picker.purpose {
            Purpose::Replace { placement, what } => {
                self.replace_on_placement(placement, what, chosen)?
            }
            Purpose::ReplacePart { part, what } => self.replace_on_part(part, what, chosen)?,
            Purpose::BankAddLanePart { lane } => {
                if chosen == 0 {
                    let mut ctx = self.workflow_context();
                    ctx.lane = Some(lane);
                    ctx.add_part_to_lane = Some(lane);
                    self.begin_workflow_with_context(WorkflowKind::Part, ctx)?
                } else {
                    let res = self.apply(Action::AddLanePart {
                        lane,
                        entry: cadence_core::material::LanePart::new(chosen),
                    })?;
                    if let Some(lane_idx) =
                        self.doc.project().lanes.iter().position(|l| l.id == lane)
                    {
                        self.view_state.bank_lane_idx = lane_idx;
                        if let Some(slot_idx) = self.doc.project().lanes[lane_idx]
                            .parts
                            .iter()
                            .position(|e| e.part == chosen)
                        {
                            self.view_state.bank_part_idx = slot_idx;
                        }
                    }
                    self.sync_bank_part_selection();
                    res
                }
            }
            Purpose::LaunchCycle => self
                .set_launch_boundary(cadence_core::material::Boundary::Cycle { pattern: chosen })?,
            Purpose::FilterEnvSource { device } => {
                let next = (chosen != 0).then_some(chosen);
                self.set_filter_env_source(device, next)?
            }
            Purpose::GraphRouteChannel { channel } => {
                let output = if chosen == 0 {
                    ChannelRef::Master
                } else {
                    ChannelRef::Channel(chosen)
                };
                self.apply(Action::SetChannelOutput { channel, output })?
            }
            Purpose::GraphRouteInstrument { instrument } => {
                self.apply(Action::RouteInstrument {
                    instrument,
                    channel: chosen,
                })?
            }
            Purpose::GraphEditCable { cables } => {
                let cable = *cables
                    .get(chosen as usize)
                    .context("Cable choice missing")?;
                self.edit_graph_cable(cable)?
            }
            Purpose::GraphDeleteCable { cables } => {
                let cable = *cables
                    .get(chosen as usize)
                    .context("Cable choice missing")?;
                self.unlink_graph_cable(cable)?
            }
            Purpose::RackAddModule {
                instrument,
                zone_id,
                kinds,
            } => {
                let kind = *kinds
                    .get(chosen as usize)
                    .context("Module choice missing")?;
                self.apply(Action::RackAddModule {
                    instrument,
                    zone: zone_id,
                    kind,
                    name: None,
                })?
            }
            Purpose::RackAddZone { instrument } => match chosen {
                0 => self.add_rack_zone_template(
                    instrument,
                    crate::zones::AddZoneTemplate::BlankSampler,
                )?,
                1 => self
                    .add_rack_zone_template(instrument, crate::zones::AddZoneTemplate::SynthPad)?,
                2 => self.add_rack_zone_template(
                    instrument,
                    crate::zones::AddZoneTemplate::BlankModular,
                )?,
                3 => self.begin_sample_browser(browser::BrowserTarget::AddZone { instrument })?,
                4 => self.open_rack_zone_preset_picker(instrument, false)?,
                5 => self.open_rack_zone_preset_picker(instrument, true)?,
                6 => self.begin_workflow(WorkflowKind::ZoneImportFolder(instrument))?,
                _ => Outcome::default(),
            },
            Purpose::RackZonePreset {
                instrument,
                replace_active,
                preset_ids,
            } => {
                let preset_id = preset_ids
                    .get(chosen as usize)
                    .cloned()
                    .context("Preset choice missing")?;
                if replace_active {
                    self.apply_zone_load_preset(Some(instrument), &preset_id)?
                } else {
                    self.add_rack_zone_from_preset(instrument, &preset_id)?
                }
            }
            Purpose::RackConnectCable {
                instrument,
                zone_id,
                from_module,
                targets,
            } => {
                let &(to_module, to_port) = targets
                    .get(chosen as usize)
                    .context("Cable target choice missing")?;
                self.begin_rack_cable_amount_input(
                    instrument,
                    zone_id,
                    from_module,
                    to_module,
                    to_port,
                )?
            }
            Purpose::RackRepatchCable {
                instrument,
                zone_id,
                cable_id,
                targets,
            } => {
                let &(to_module, to_port) = targets
                    .get(chosen as usize)
                    .context("Cable target choice missing")?;
                self.apply_rack_repatch_cable(instrument, zone_id, cable_id, to_module, to_port)?
            }
            Purpose::RackSelectCable {
                instrument,
                zone_id,
                cables,
            } => {
                let cid = *cables
                    .get(chosen as usize)
                    .context("Cable choice missing")?;
                self.rack_ui.patchbay_cable_id = Some(cid);
                self.begin_param_value_input(
                    ParamAddress::Cable {
                        instrument,
                        zone: zone_id,
                        cable: cid,
                    },
                    String::new(),
                )?
            }
            Purpose::MacroBindTarget { slot, targets } => {
                let target = *targets
                    .get(chosen as usize)
                    .context("Binding target missing")?;
                let res = self.apply(Action::BindMacro {
                    index: slot,
                    target,
                    min: 0.0,
                    max: 1.0,
                })?;
                self.view_state.macro_cursor = usize::from(slot);
                self.view_state.macro_binding_cursor = self
                    .project()
                    .macro_control(slot)
                    .and_then(|m| m.bindings.iter().position(|b| b.target == target));
                res
            }
            Purpose::LinkMenu { actions } => {
                let action = actions
                    .get(chosen as usize)
                    .cloned()
                    .context("Link menu choice missing")?;
                self.execute_link_menu_action(action)?
            }
            Purpose::GraphAdd => match chosen {
                0 => self.begin_workflow(WorkflowKind::Send)?,
                1 => self.begin_workflow(WorkflowKind::Modulation)?,
                2 => self.begin_workflow(WorkflowKind::Duck)?,
                _ => Outcome::default(),
            },
            Purpose::MixerAdd { channel } => {
                let mut ctx = self.workflow_context();
                ctx.mixer_channel = Some(channel);
                match chosen {
                    0 => self.begin_workflow_with_context(WorkflowKind::Effect, ctx)?,
                    1 => self.begin_workflow_with_context(WorkflowKind::Send, ctx)?,
                    2 => self.begin_workflow_with_context(WorkflowKind::Duck, ctx)?,
                    3 => self.begin_workflow_with_context(WorkflowKind::Channel, ctx)?,
                    _ => Outcome::default(),
                }
            }
            Purpose::MidiPort { is_input, ports } => {
                let port = ports
                    .get(chosen as usize)
                    .cloned()
                    .context("MIDI port choice missing")?;
                if is_input {
                    self.set_midi_in_port_choice(port)
                } else {
                    self.set_midi_out_port_choice(port)
                }
            }
            Purpose::MidiProfile => {
                use cadence_core::midi_map::MidiProfile;
                match chosen {
                    100 => {
                        self.open_file_hub_for_midi_profile_load();
                        Outcome::default()
                    }
                    101 => {
                        self.open_file_hub_for_midi_profile_save();
                        Outcome::default()
                    }
                    idx => {
                        let id = *MidiProfile::BUILTIN_IDS
                            .get(idx as usize)
                            .context("MIDI profile choice missing")?;
                        self.request_switch_midi_profile(id)
                    }
                }
            }
            Purpose::MidiPadMode => {
                use cadence_core::midi_map::MidiPadMode;
                let mode = *MidiPadMode::ALL
                    .get(chosen as usize)
                    .context("MIDI pad mode choice missing")?;
                self.set_midi_pad_mode_choice(mode)
            }
            Purpose::MidiAddMapping { targets } => {
                let (target, label) = targets
                    .get(chosen as usize)
                    .cloned()
                    .context("MIDI mapping target missing")?;
                self.arm_midi_learn_target(target, label)
            }
            Purpose::AudioOutput { cue, devices } => {
                let dev = devices
                    .get(chosen as usize)
                    .cloned()
                    .context("Audio output choice missing")?;
                if cue {
                    self.set_audio_cue_choice(dev)
                } else {
                    self.set_audio_output_choice(dev)
                }
            }
            Purpose::AudioInput { devices } => {
                let dev = devices
                    .get(chosen as usize)
                    .cloned()
                    .context("Audio input choice missing")?;
                self.set_audio_input_choice(dev)
            }
            Purpose::AudioSampleRate { rates } => {
                let rate = *rates
                    .get(chosen as usize)
                    .context("Audio sample rate choice missing")?;
                self.set_audio_sample_rate_choice(rate)
            }
            Purpose::AudioBufferSize { sizes } => {
                let size = *sizes
                    .get(chosen as usize)
                    .context("Audio buffer size choice missing")?;
                self.set_audio_buffer_size_choice(size)
            }
            Purpose::SliceToKit {
                instrument,
                zone,
                module,
            } => match chosen {
                0 => self.apply(Action::SliceToRackKit {
                    instrument,
                    zone,
                    module,
                    replace_existing: false,
                    create_pattern: true,
                    choke_group: Some(1),
                    kit_name: None,
                })?,
                1 => self.apply(Action::SliceToRackKit {
                    instrument,
                    zone,
                    module,
                    replace_existing: false,
                    create_pattern: false,
                    choke_group: Some(1),
                    kit_name: None,
                })?,
                2 => self.apply(Action::SliceToRackKit {
                    instrument,
                    zone,
                    module,
                    replace_existing: true,
                    create_pattern: false,
                    choke_group: Some(1),
                    kit_name: None,
                })?,
                _ => Outcome::default(),
            },
            Purpose::SampleEdit {
                instrument,
                zone,
                module,
            } => {
                let edit = match chosen {
                    0 => Some(SampleEditOp::Trim),
                    1 => Some(SampleEditOp::Normalize {
                        target_dbfs: cadence_core::dsp::DEFAULT_NORMALIZE_TARGET_DBFS,
                    }),
                    2 => Some(SampleEditOp::Reverse),
                    3 => Some(SampleEditOp::FadeIn { duration_ms: None }),
                    4 => Some(SampleEditOp::FadeOut { duration_ms: None }),
                    _ => None,
                };
                match edit {
                    Some(edit) => self.apply(Action::ProcessSample {
                        instrument,
                        zone,
                        module,
                        edit,
                    })?,
                    None => Outcome::default(),
                }
            }
        };
        out.merge(edit);
        Ok(out)
    }
}
