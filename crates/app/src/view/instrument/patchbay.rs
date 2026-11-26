//! Eurorack Patchbay column, node, and cable view projection.
use super::{
    module_card_detail, sanitize_wave, PatchbayCableKind, PatchbayCableView, PatchbayColumnView,
    PatchbayDeckView, PatchbayNodeView,
};
use crate::rack::patchbay_columns;
use crate::App;
use cadence_core::material::Id;
use cadence_core::params::InstrumentParam;
use cadence_core::rack::{
    compile_zone_graph, format_cable_amount, InPort, ModuleKindTag, RackZone, PATCHBAY_COLUMNS,
};
use cadence_core::runtime::AudioSnapshot;
use std::collections::{HashMap, HashSet};

fn port_y_for(target_tag: ModuleKindTag, port: InPort) -> f32 {
    let ports = target_tag.in_ports();
    if ports.len() <= 1 {
        return 0.5;
    }
    let idx = ports.iter().position(|&p| p == port).unwrap_or(0);
    let frac = idx as f32 / (ports.len() - 1) as f32;
    0.24 + frac * 0.52
}

pub(super) fn empty_patchbay_view(live: &[f32]) -> PatchbayDeckView {
    PatchbayDeckView {
        columns: PATCHBAY_COLUMNS
            .iter()
            .map(|&title| PatchbayColumnView {
                title,
                nodes: Vec::new(),
            })
            .collect(),
        nodes: Vec::new(),
        cables: Vec::new(),
        selected_col: 0,
        selected_row: 0,
        selected_module: None,
        selected_cable: None,
        selected_summary: "No zones".into(),
        has_feedback: false,
        feedback_badge: "z-1",
        live_waveform: sanitize_wave(live),
    }
}

impl App {
    pub(super) fn build_patchbay_view(
        &self,
        inst_id: Id,
        zone: &RackZone,
        live: &[f32],
        snapshot: &AudioSnapshot,
        query_time: f64,
    ) -> PatchbayDeckView {
        let cols_ids = patchbay_columns(zone);
        let mut sel_col = self
            .rack_ui
            .patchbay_col
            .min(cols_ids.len().saturating_sub(1));
        if cols_ids[sel_col].is_empty() {
            sel_col = cols_ids.iter().position(|c| !c.is_empty()).unwrap_or(0);
        }
        let sel_row = if cols_ids[sel_col].is_empty() {
            0
        } else {
            self.rack_ui.patchbay_row.min(cols_ids[sel_col].len() - 1)
        };
        let selected_module = cols_ids[sel_col].get(sel_row).copied();
        let selected_cable = self
            .rack_ui
            .patchbay_cable_id
            .filter(|&cid| zone.cable(cid).is_some());

        let compiled = compile_zone_graph(zone).ok();
        let has_feedback = compiled.as_ref().is_some_and(|g| g.has_feedback());

        let mut pos_map: HashMap<u16, (usize, usize)> = HashMap::new();
        for (c, ids) in cols_ids.iter().enumerate() {
            for (r, &mid) in ids.iter().enumerate() {
                pos_map.insert(mid, (c, r));
            }
        }

        // Highlight set: nodes touching the selected cable OR selected module's direct/upstream/downstream peers.
        let mut path_modules: HashSet<u16> = HashSet::new();
        if let Some(cid) = selected_cable {
            if let Some(cable) = zone.cable(cid) {
                path_modules.insert(cable.from_module);
                path_modules.insert(cable.to_module);
            }
        } else if let Some(mid) = selected_module {
            path_modules.insert(mid);
            for cable in &zone.cables {
                if cable.from_module == mid || cable.to_module == mid {
                    path_modules.insert(cable.from_module);
                    path_modules.insert(cable.to_module);
                }
            }
        }

        let mut columns = Vec::with_capacity(4);
        let mut flat_nodes = Vec::with_capacity(zone.modules.len());
        for (c, ids) in cols_ids.iter().enumerate() {
            let mut nodes = Vec::with_capacity(ids.len());
            for (r, &mid) in ids.iter().enumerate() {
                let Some(module) = zone.module(mid) else {
                    continue;
                };
                let tag = module.kind.tag();
                let activity = snapshot.rack_module_peak(inst_id, zone.id, mid);
                let is_audio_module = !matches!(tag, ModuleKindTag::Env | ModuleKindTag::Lfo);
                let node = PatchbayNodeView {
                    module_id: mid,
                    col: c,
                    row: r,
                    name: module.name.clone(),
                    badge: tag.short_badge(),
                    detail: module_card_detail(module),
                    kind: tag,
                    activity,
                    hot: is_audio_module && activity > 1.0,
                    selected: selected_cable.is_none() && selected_module == Some(mid),
                    in_path: path_modules.contains(&mid),
                    incoming_ports: tag.in_ports().to_vec(),
                    has_output: tag.has_output(),
                };
                flat_nodes.push(node.clone());
                nodes.push(node);
            }
            columns.push(PatchbayColumnView {
                title: PATCHBAY_COLUMNS[c],
                nodes,
            });
        }

        let mut cables = Vec::with_capacity(zone.cables.len());
        for cable in &zone.cables {
            let (Some(&(fc, fr)), Some(&(tc, tr))) = (
                pos_map.get(&cable.from_module),
                pos_map.get(&cable.to_module),
            ) else {
                continue;
            };
            let dst_tag = zone
                .module(cable.to_module)
                .map_or(ModuleKindTag::Out, |m| m.kind.tag());
            let feedback = compiled
                .as_ref()
                .and_then(|g| g.cables.iter().find(|cc| cc.cable_id == cable.id))
                .is_some_and(|cc| cc.feedback);
            let live_amount = self
                .resolve_live_rack_param(
                    inst_id,
                    InstrumentParam::Cable {
                        zone: zone.id,
                        cable: cable.id,
                    },
                    query_time,
                )
                .map(|norm| {
                    cadence_core::rack::denormalize_cable_amount(dst_tag, cable.to_port, norm)
                })
                .unwrap_or(cable.amount);
            let mut label = format_cable_amount(dst_tag, cable.to_port, live_amount);
            if feedback {
                label.push_str(" z-1");
            }
            let selected = selected_cable == Some(cable.id);
            let highlighted = selected
                || (selected_cable.is_none()
                    && selected_module
                        .is_some_and(|mid| cable.from_module == mid || cable.to_module == mid));
            let cable_activity = snapshot.rack_cable_peak(inst_id, zone.id, cable.id);
            cables.push(PatchbayCableView {
                cable_id: cable.id,
                from_module: cable.from_module,
                to_module: cable.to_module,
                to_port: cable.to_port,
                from_col: fc,
                from_row: fr,
                to_col: tc,
                to_row: tr,
                to_port_y: port_y_for(dst_tag, cable.to_port),
                kind: PatchbayCableKind::from_port(cable.to_port),
                amount: live_amount,
                activity: cable_activity,
                label,
                feedback,
                selected,
                highlighted,
            });
        }

        let selected_summary =
            format_patchbay_summary(zone, &cables, selected_cable, selected_module);

        PatchbayDeckView {
            columns,
            nodes: flat_nodes,
            cables,
            selected_col: sel_col,
            selected_row: sel_row,
            selected_module,
            selected_cable,
            selected_summary,
            has_feedback,
            feedback_badge: "z-1",
            live_waveform: sanitize_wave(live),
        }
    }
}

fn format_patchbay_summary(
    zone: &RackZone,
    cables: &[PatchbayCableView],
    selected_cable: Option<u16>,
    selected_module: Option<u16>,
) -> String {
    if let Some(cid) = selected_cable {
        if let Some(c) = cables.iter().find(|c| c.cable_id == cid) {
            let src = zone.module(c.from_module).map_or("?", |m| m.name.as_str());
            let dst = zone.module(c.to_module).map_or("?", |m| m.name.as_str());
            format!(
                "CABLE #{cid} · {src} → {dst}.{} · {}",
                c.to_port.short_label(),
                c.label
            )
        } else {
            format!("CABLE #{cid}")
        }
    } else if let Some(mid) = selected_module {
        if let Some(m) = zone.module(mid) {
            let out_n = zone.cables.iter().filter(|c| c.from_module == mid).count();
            let in_n = zone.cables.iter().filter(|c| c.to_module == mid).count();
            format!(
                "{} ({}) · {} · {in_n} in / {out_n} out · c patch · Enter panel",
                m.name,
                m.kind.tag().short_badge(),
                module_card_detail(m)
            )
        } else {
            "Empty Patchbay".into()
        }
    } else {
        "Empty Patchbay · press a to add a module".into()
    }
}
