//! Semantic Relationship / Patchbay Graph deck view (`gG`).
//!
//! Organizes the live project signal web into 5 left-to-right columns:
//! `SOURCES` → `DERIVED` → `PARTS` → `INSTRUMENTS` → `MIXER / FX`,
//! computing upstream + downstream signal path highlighting for the selected node
//! and mapping `Enter` directly to the corresponding Pattern, Part, Instrument, or Mixer deck.
use crate::{kind_label, Action, App, Outcome};
use anyhow::{Context, Result};
use cadence_core::material::{Edit, Id, ModulationTarget};
use cadence_core::mixer::{ChannelKind, ChannelRef, EffectKind};
use cadence_core::params::ParamAddress;
use cadence_core::runtime::AudioSnapshot;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GraphTarget {
    Pattern(Id),
    Part(Id),
    Macro(u8),
    Instrument(Id),
    Channel(ChannelRef),
    Effect(Id),
}

/// Interactive mutable cable touching a Signal Graph node (for edit / unlink operations).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum GraphCable {
    Send { source: Id, target: Id, amount: f32 },
    Modulation(Id),
    Duck(Id),
    FilterEnv { device: Id, pattern: Id },
    ParamLink { part: Id, index: usize, pattern: Id },
    BusRoute { channel: Id, output: ChannelRef },
    MacroBinding { index: u8, binding: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphEdgeKind {
    /// Pattern algebra / source dependency (`Pattern -> Derived Pattern` or `Pattern -> Part`).
    Pattern,
    /// Audio/musical flow (`Part -> Instrument`, `Instrument -> Channel`).
    Signal,
    /// Bus routing / sends (`Channel -> Group/Master`).
    Routing,
    /// Continuous or per-note modulation (`Value Pattern -> Part/Macro/Instrument/Channel`).
    Modulation,
    /// Project Macro fan-out routing (`Macro -> Instrument/Effect-Channel/Channel`).
    Macro,
    /// Sidechain ducking (`Source Part -> Target Channel`).
    Sidechain,
}

#[derive(Clone, Debug)]
pub struct GraphNode {
    pub col: usize,
    pub row: usize,
    pub target: GraphTarget,
    pub name: String,
    pub badge: String,
    pub detail: String,
    pub color_index: usize,
    /// Linear unclamped 32-bit float peak for audio nodes (`Instrument`/`Channel`/`Master`),
    /// or normalized `0.0..=1.0` activity ratio for non-audio nodes (`Part`/`Macro`/`Effect`).
    pub activity: f32,
    /// Informational 32-bit float headroom flag (`> 0 dBFS` on an internal bus or pre-limiter Master).
    pub hot: bool,
    /// True digital clipping flag (Master post-fader `> 1.0` or non-finite sample anywhere).
    pub clipped: bool,
    pub selected: bool,
    pub in_path: bool,
    pub is_master: bool,
    pub is_effect: bool,
    pub is_bypassed: bool,
}

#[derive(Clone, Debug)]
pub struct GraphColumn {
    pub title: &'static str,
    pub nodes: Vec<GraphNode>,
}

#[derive(Clone, Copy, Debug)]
pub struct GraphEdge {
    pub from_col: usize,
    pub from_row: usize,
    pub to_col: usize,
    pub to_row: usize,
    pub kind: GraphEdgeKind,
    pub highlighted: bool,
    pub color_index: usize,
}

#[derive(Clone, Debug)]
pub struct GraphDeckView {
    pub columns: Vec<GraphColumn>,
    pub edges: Vec<GraphEdge>,
    pub selected_col: usize,
    pub selected_row: usize,
    pub selected_summary: String,
    pub selected_cables: String,
}

fn effect_tag(kind: &EffectKind) -> &'static str {
    kind.short_tag()
}

impl App {
    /// Return the ordered columns of `GraphTarget`s (5 columns normally, 6 with `MACROS` when configured).
    pub(crate) fn graph_target_columns(&self) -> Vec<Vec<GraphTarget>> {
        let project = self.project();
        let mut sources = Vec::new();
        let mut derived = Vec::new();
        for p in &project.patterns {
            if p.kind.sources().is_empty() {
                sources.push(GraphTarget::Pattern(p.id));
            } else {
                derived.push(GraphTarget::Pattern(p.id));
            }
        }
        let parts = project
            .parts
            .iter()
            .map(|p| GraphTarget::Part(p.id))
            .collect();
        let instruments = project
            .instruments
            .iter()
            .map(|i| GraphTarget::Instrument(i.id))
            .collect();
        let mut mixer: Vec<GraphTarget> = Vec::new();
        for ch in &project.mixer.channels {
            mixer.push(GraphTarget::Channel(ChannelRef::Channel(ch.id)));
            for dev in &ch.inserts {
                mixer.push(GraphTarget::Effect(dev.id));
            }
        }
        mixer.push(GraphTarget::Channel(ChannelRef::Master));
        for dev in &project.mixer.master.inserts {
            mixer.push(GraphTarget::Effect(dev.id));
        }
        if project.macros.is_empty() {
            vec![sources, derived, parts, instruments, mixer]
        } else {
            let macros = project
                .macros
                .iter()
                .map(|m| GraphTarget::Macro(m.index))
                .collect();
            vec![sources, derived, parts, macros, instruments, mixer]
        }
    }

    pub(crate) fn selected_graph_target(&self) -> Option<GraphTarget> {
        let cols = self.graph_target_columns();
        let col = self.view_state.graph_col.min(cols.len().saturating_sub(1));
        let list = &cols[col];
        if list.is_empty() {
            return cols.iter().find_map(|c| c.first().copied());
        }
        Some(list[self.view_state.graph_row.min(list.len() - 1)])
    }

    pub(super) fn build_graph_view(&self, snapshot: &AudioSnapshot) -> GraphDeckView {
        let project = self.project();
        let raw_cols = self.graph_target_columns();
        let titles: Vec<&'static str> = if project.macros.is_empty() {
            vec!["SOURCES", "DERIVED", "PARTS", "INSTRUMENTS", "MIXER / FX"]
        } else {
            vec![
                "SOURCES",
                "DERIVED",
                "PARTS",
                "MACROS",
                "INSTRUMENTS",
                "MIXER / FX",
            ]
        };

        // Resolve valid clamped selection.
        let mut sel_col = self
            .view_state
            .graph_col
            .min(raw_cols.len().saturating_sub(1));
        if raw_cols[sel_col].is_empty() {
            sel_col = raw_cols.iter().position(|c| !c.is_empty()).unwrap_or(0);
        }
        let sel_row = if raw_cols[sel_col].is_empty() {
            0
        } else {
            self.view_state.graph_row.min(raw_cols[sel_col].len() - 1)
        };
        let selected_target = raw_cols[sel_col].get(sel_row).copied();

        // Map every GraphTarget -> (col, row, color_index).
        let mut pos_map: HashMap<GraphTarget, (usize, usize, usize)> = HashMap::new();
        for (c, targets) in raw_cols.iter().enumerate() {
            for (r, &t) in targets.iter().enumerate() {
                let color_index = match t {
                    GraphTarget::Pattern(id)
                    | GraphTarget::Part(id)
                    | GraphTarget::Instrument(id) => id as usize,
                    GraphTarget::Macro(idx) => usize::from(idx),
                    GraphTarget::Channel(ChannelRef::Channel(id)) => id as usize,
                    GraphTarget::Channel(ChannelRef::Master) => 0,
                    GraphTarget::Effect(dev_id) => match project.device(dev_id) {
                        Some((ChannelRef::Channel(ch_id), _)) => ch_id as usize,
                        _ => 0,
                    },
                };
                pos_map.insert(t, (c, r, color_index));
            }
        }

        // Collect all directed edges (from -> to, kind).
        let mut raw_edges: Vec<(GraphTarget, GraphTarget, GraphEdgeKind)> = Vec::new();
        for p in &project.patterns {
            let dst = GraphTarget::Pattern(p.id);
            for src_id in p.kind.sources() {
                raw_edges.push((GraphTarget::Pattern(src_id), dst, GraphEdgeKind::Pattern));
            }
        }
        for part in &project.parts {
            let dst = GraphTarget::Part(part.id);
            raw_edges.push((
                GraphTarget::Pattern(part.pattern),
                dst,
                GraphEdgeKind::Pattern,
            ));
            for link in &part.parameter_links {
                raw_edges.push((
                    GraphTarget::Pattern(link.pattern),
                    dst,
                    GraphEdgeKind::Modulation,
                ));
            }
            raw_edges.push((
                dst,
                GraphTarget::Instrument(part.instrument),
                GraphEdgeKind::Signal,
            ));
        }
        for inst in &project.instruments {
            raw_edges.push((
                GraphTarget::Instrument(inst.id),
                GraphTarget::Channel(ChannelRef::Channel(inst.channel)),
                GraphEdgeKind::Signal,
            ));
        }
        let mut routed_sends: HashSet<(ChannelRef, Id)> = HashSet::new();
        for ch in &project.mixer.channels {
            let src_ref = ChannelRef::Channel(ch.id);
            let mut chain_tail = GraphTarget::Channel(src_ref);
            for dev in &ch.inserts {
                let dev_node = GraphTarget::Effect(dev.id);
                raw_edges.push((chain_tail, dev_node, GraphEdgeKind::Signal));
                if let EffectKind::Filter(fp) = dev.kind {
                    if let Some(pat_id) = fp.env_pattern_id {
                        raw_edges.push((
                            GraphTarget::Pattern(pat_id),
                            dev_node,
                            GraphEdgeKind::Modulation,
                        ));
                    }
                }
                chain_tail = dev_node;
            }
            let dst = GraphTarget::Channel(ch.output);
            raw_edges.push((chain_tail, dst, GraphEdgeKind::Routing));
            for send in &ch.sends {
                if routed_sends.insert((src_ref, send.target)) {
                    raw_edges.push((
                        chain_tail,
                        GraphTarget::Channel(ChannelRef::Channel(send.target)),
                        GraphEdgeKind::Routing,
                    ));
                }
            }
        }
        let mut master_tail = GraphTarget::Channel(ChannelRef::Master);
        for dev in &project.mixer.master.inserts {
            let dev_node = GraphTarget::Effect(dev.id);
            raw_edges.push((master_tail, dev_node, GraphEdgeKind::Signal));
            if let EffectKind::Filter(fp) = dev.kind {
                if let Some(pat_id) = fp.env_pattern_id {
                    raw_edges.push((
                        GraphTarget::Pattern(pat_id),
                        dev_node,
                        GraphEdgeKind::Modulation,
                    ));
                }
            }
            master_tail = dev_node;
        }
        for m in &project.modulations {
            let src = GraphTarget::Pattern(m.pattern);
            let dst = match m.target {
                ModulationTarget::Instrument { instrument, .. } => {
                    GraphTarget::Instrument(instrument)
                }
                ModulationTarget::ChannelGain { channel }
                | ModulationTarget::ChannelPan { channel } => GraphTarget::Channel(channel),
                ModulationTarget::Send { channel, target } => {
                    if routed_sends.insert((channel, target)) {
                        let send_origin = match channel {
                            ChannelRef::Channel(cid) => project
                                .channel(cid)
                                .and_then(|c| c.inserts.last())
                                .map_or(GraphTarget::Channel(channel), |d| {
                                    GraphTarget::Effect(d.id)
                                }),
                            ChannelRef::Master => GraphTarget::Channel(channel),
                        };
                        raw_edges.push((
                            send_origin,
                            GraphTarget::Channel(ChannelRef::Channel(target)),
                            GraphEdgeKind::Routing,
                        ));
                    }
                    GraphTarget::Channel(channel)
                }
                ModulationTarget::Effect { device, .. } => {
                    if project.device(device).is_some() {
                        GraphTarget::Effect(device)
                    } else {
                        continue;
                    }
                }
                ModulationTarget::Macro { index } => {
                    if project.macro_control(index).is_some() {
                        GraphTarget::Macro(index)
                    } else {
                        continue;
                    }
                }
            };
            raw_edges.push((src, dst, GraphEdgeKind::Modulation));
        }
        for mac in &project.macros {
            let src = GraphTarget::Macro(mac.index);
            let mut seen_dsts = HashSet::new();
            for binding in &mac.bindings {
                let dst = match binding.target {
                    ParamAddress::Rack { instrument, .. }
                    | ParamAddress::Zone { instrument, .. }
                    | ParamAddress::Module { instrument, .. }
                    | ParamAddress::Cable { instrument, .. } => GraphTarget::Instrument(instrument),
                    ParamAddress::Channel { channel, .. } | ParamAddress::Send { channel, .. } => {
                        GraphTarget::Channel(channel)
                    }
                    ParamAddress::Effect { device, .. } => {
                        if project.device(device).is_some() {
                            GraphTarget::Effect(device)
                        } else {
                            continue;
                        }
                    }
                };
                if seen_dsts.insert(dst) {
                    raw_edges.push((src, dst, GraphEdgeKind::Macro));
                }
            }
        }
        for d in &project.ducks {
            raw_edges.push((
                GraphTarget::Part(d.source_part),
                GraphTarget::Channel(d.target),
                GraphEdgeKind::Sidechain,
            ));
        }

        // Compute transitive upstream + downstream signal path for the selected node!
        let mut path_nodes: HashSet<GraphTarget> = HashSet::new();
        if let Some(sel) = selected_target {
            path_nodes.insert(sel);
            // Upstream BFS
            let mut q = VecDeque::from([sel]);
            while let Some(cur) = q.pop_front() {
                for &(a, b, _) in &raw_edges {
                    if b == cur && path_nodes.insert(a) {
                        q.push_back(a);
                    }
                }
            }
            // Downstream BFS
            let mut q = VecDeque::from([sel]);
            let mut seen_down = HashSet::from([sel]);
            while let Some(cur) = q.pop_front() {
                for &(a, b, _) in &raw_edges {
                    if a == cur && seen_down.insert(b) {
                        path_nodes.insert(b);
                        q.push_back(b);
                    }
                }
            }
        }

        // Build final edge structs.
        let mut edges = Vec::with_capacity(raw_edges.len());
        for (a, b, kind) in raw_edges {
            let (Some(&(fc, fr, fcol)), Some(&(tc, tr, _))) = (pos_map.get(&a), pos_map.get(&b))
            else {
                continue;
            };
            let highlighted = path_nodes.contains(&a) && path_nodes.contains(&b);
            edges.push(GraphEdge {
                from_col: fc,
                from_row: fr,
                to_col: tc,
                to_row: tr,
                kind,
                highlighted,
                color_index: fcol,
            });
        }

        // Build columns with rich live metadata & meters.
        let mut columns = Vec::with_capacity(raw_cols.len());
        for (c, targets) in raw_cols.iter().enumerate() {
            let mut nodes = Vec::with_capacity(targets.len());
            for (r, &target) in targets.iter().enumerate() {
                let selected = c == sel_col && r == sel_row;
                let in_path = path_nodes.contains(&target);
                let (
                    name,
                    badge,
                    detail,
                    color_index,
                    activity,
                    hot,
                    clipped,
                    is_master,
                    is_effect,
                    is_bypassed,
                ) = match target {
                    GraphTarget::Pattern(id) => {
                        let p = project.pattern(id);
                        let name = p.map_or_else(|| format!("Pat {id}"), |x| x.name.clone());
                        let badge = p.map_or("", |x| kind_label(&x.kind)).to_string();
                        let period = self.pattern_period(id);
                        let detail = format!("{period} steps");
                        (
                            name,
                            badge,
                            detail,
                            id as usize,
                            0.0,
                            false,
                            false,
                            false,
                            false,
                            false,
                        )
                    }
                    GraphTarget::Part(id) => {
                        let part = project.part(id);
                        let name = part.map_or_else(|| format!("Part {id}"), |x| x.name.clone());
                        let active = project.lanes.iter().any(|l| {
                            self.live
                                .lane_state(l.id)
                                .is_some_and(|s| s.part.part == id)
                        });
                        let badge = if active { "LIVE" } else { "PART" }.to_string();
                        let detail = part
                            .and_then(|pt| project.pattern(pt.pattern))
                            .map_or_else(String::new, |pat| pat.name.clone());
                        (
                            name,
                            badge,
                            detail,
                            id as usize,
                            if active { 0.85 } else { 0.0 },
                            false,
                            false,
                            false,
                            false,
                            false,
                        )
                    }
                    GraphTarget::Macro(index) => {
                        let mac = project.macro_control(index);
                        let name = mac.map_or_else(
                            || format!("Macro {}", usize::from(index) + 1),
                            |m| m.name.clone(),
                        );
                        let live = self.live_macro_modulation(index, snapshot);
                        let eff = live
                            .as_ref()
                            .map_or_else(|| mac.map_or(0.0, |m| m.value), |(v, _)| *v);
                        let badge = if live.is_some() {
                            format!("M{}·MOD", usize::from(index) + 1)
                        } else {
                            format!("M{}", usize::from(index) + 1)
                        };
                        let binds = mac.map_or(0, |m| m.bindings.len());
                        let detail = format!("{:.0}% · {binds}t", eff * 100.0);
                        (
                            name,
                            badge,
                            detail,
                            usize::from(index),
                            eff,
                            false,
                            false,
                            false,
                            false,
                            false,
                        )
                    }
                    GraphTarget::Instrument(id) => {
                        let inst = project.instrument(id);
                        let name = inst.map_or_else(|| format!("Inst {id}"), |x| x.name.clone());
                        let (badge, detail) = match inst.map(|i| &i.rack) {
                            Some(rack) => {
                                let zones = rack.zones.len();
                                let mods: usize = rack.zones.iter().map(|z| z.modules.len()).sum();
                                let detail = if zones > 1 && !rack.kit_name.is_empty() {
                                    format!("{} · {zones}z/{mods}m", rack.kit_name)
                                } else {
                                    format!("{zones}z · {mods}m")
                                };
                                ("RACK".into(), detail)
                            }
                            None => (String::new(), String::new()),
                        };
                        let mut peak = 0.0_f32;
                        let mut non_finite = false;
                        for &s in snapshot.track_waveform(id) {
                            if s.is_finite() {
                                peak = peak.max(s.abs());
                            } else {
                                non_finite = true;
                            }
                        }
                        let hot = peak > 1.0;
                        let clipped = non_finite;
                        (
                            name,
                            badge,
                            detail,
                            id as usize,
                            peak,
                            hot,
                            clipped,
                            false,
                            false,
                            false,
                        )
                    }
                    GraphTarget::Channel(ChannelRef::Channel(id)) => {
                        let ch = project.channel(id);
                        let name = ch.map_or_else(|| format!("Ch {id}"), |x| x.name.clone());
                        let badge = match ch.map(|x| x.kind) {
                            Some(ChannelKind::Audio) => "AUDIO",
                            Some(ChannelKind::Group) => "GROUP",
                            Some(ChannelKind::Return) => "RETURN",
                            None => "CH",
                        }
                        .to_string();
                        let send_cnt = ch.map_or(0, |x| x.sends.len());
                        let gain_str =
                            cadence_core::dsp::format_gain_db(ch.map_or(1.0, |x| x.strip.gain));
                        let detail = if send_cnt > 0 {
                            format!("{gain_str} · {send_cnt}s")
                        } else {
                            gain_str
                        };
                        let meter = snapshot.channel_meter(ChannelRef::Channel(id));
                        let peak = meter.map_or(0.0, |m| {
                            let l = if m.output_peak[0].is_finite() {
                                m.output_peak[0].max(0.0)
                            } else {
                                0.0
                            };
                            let r = if m.output_peak[1].is_finite() {
                                m.output_peak[1].max(0.0)
                            } else {
                                0.0
                            };
                            l.max(r)
                        });
                        let hot = meter.is_some_and(|m| m.hot) || peak > 1.0;
                        let clipped = meter.is_some_and(|m| m.clipped);
                        (
                            name,
                            badge,
                            detail,
                            id as usize,
                            peak,
                            hot,
                            clipped,
                            false,
                            false,
                            false,
                        )
                    }
                    GraphTarget::Channel(ChannelRef::Master) => {
                        let detail =
                            cadence_core::dsp::format_gain_db(project.mixer.master.strip.gain);
                        let meter = snapshot.channel_meter(ChannelRef::Master);
                        let meter_peak = meter.map_or(0.0, |m| {
                            let l = if m.output_peak[0].is_finite() {
                                m.output_peak[0].max(0.0)
                            } else {
                                0.0
                            };
                            let r = if m.output_peak[1].is_finite() {
                                m.output_peak[1].max(0.0)
                            } else {
                                0.0
                            };
                            l.max(r)
                        });
                        let snap_peak = if snapshot.peak.is_finite() {
                            snapshot.peak.max(0.0)
                        } else {
                            0.0
                        };
                        let peak = meter_peak.max(snap_peak);
                        let hot = meter.is_some_and(|m| m.hot) || peak > 1.0;
                        let clipped = meter.is_some_and(|m| m.clipped)
                            || !snapshot.peak.is_finite()
                            || peak > 1.0;
                        (
                            "MASTER".into(),
                            "OUT".into(),
                            detail,
                            0,
                            peak,
                            hot,
                            clipped,
                            true,
                            false,
                            false,
                        )
                    }
                    GraphTarget::Effect(dev_id) => {
                        if let Some((owner, dev)) = project.device(dev_id) {
                            let tag = effect_tag(&dev.kind);
                            let badge = if dev.bypassed {
                                format!("FX·{tag} BYP")
                            } else {
                                format!("FX·{tag}")
                            };
                            let gr = snapshot.effect_gain_reduction_db(dev_id);
                            let owner_name = self.channel_name(owner);
                            let detail = if gr > 0.25 {
                                format!("{owner_name} · -{gr:.1} dB")
                            } else if dev.bypassed {
                                format!("{owner_name} · bypassed")
                            } else {
                                owner_name.to_string()
                            };
                            let cidx = match owner {
                                ChannelRef::Channel(cid) => cid as usize,
                                ChannelRef::Master => 0,
                            };
                            let act = (gr / 18.0).clamp(0.0, 1.0);
                            (
                                dev.name.clone(),
                                badge,
                                detail,
                                cidx,
                                act,
                                false,
                                false,
                                owner == ChannelRef::Master,
                                true,
                                dev.bypassed,
                            )
                        } else {
                            (
                                format!("FX {dev_id}"),
                                "FX".into(),
                                String::new(),
                                0,
                                0.0,
                                false,
                                false,
                                false,
                                true,
                                false,
                            )
                        }
                    }
                };
                nodes.push(GraphNode {
                    col: c,
                    row: r,
                    target,
                    name,
                    badge,
                    detail,
                    color_index,
                    activity,
                    hot,
                    clipped,
                    selected,
                    in_path,
                    is_master,
                    is_effect,
                    is_bypassed,
                });
            }
            columns.push(GraphColumn {
                title: titles[c],
                nodes,
            });
        }

        let selected_summary = columns
            .get(sel_col)
            .and_then(|c| c.nodes.get(sel_row))
            .map_or_else(
                || "Empty project graph".into(),
                |n| format!("{} [{}] · {}", n.name, n.badge, n.detail),
            );
        let selected_cables = selected_target.map_or_else(String::new, |sel| {
            let in_cnt = edges
                .iter()
                .filter(|e| e.to_col == sel_col && e.to_row == sel_row)
                .count();
            let out_cnt = edges
                .iter()
                .filter(|e| e.from_col == sel_col && e.from_row == sel_row)
                .count();
            let editable = self.graph_editable_cables(sel).len();
            let removable = self.graph_removable_cables(sel).len();
            if removable > 0 {
                format!("{in_cnt} IN · {out_cnt} OUT · {editable} EDIT · {removable} UNLINK")
            } else {
                format!("{in_cnt} IN · {out_cnt} OUT")
            }
        });

        GraphDeckView {
            columns,
            edges,
            selected_col: sel_col,
            selected_row: sel_row,
            selected_summary,
            selected_cables,
        }
    }

    pub(crate) fn graph_editable_cables(
        &self,
        target: GraphTarget,
    ) -> Vec<(GraphCable, String, String)> {
        let project = self.project();
        let mut out = Vec::new();
        // 1. Return sends directly touching this channel (outgoing sends, or incoming on a Return).
        let ch_focus = match target {
            GraphTarget::Channel(ChannelRef::Channel(cid)) => Some(cid),
            _ => None,
        };
        if let Some(cid) = ch_focus {
            if let Some(ch) = project.channel(cid) {
                for send in &ch.sends {
                    out.push((
                        GraphCable::Send {
                            source: ch.id,
                            target: send.target,
                            amount: send.amount,
                        },
                        format!(
                            "SEND {} → {}",
                            ch.name,
                            self.channel_name(ChannelRef::Channel(send.target))
                        ),
                        format!("{:.0}%", send.amount * 100.0),
                    ));
                }
                if ch.kind == ChannelKind::Return {
                    for src in &project.mixer.channels {
                        for send in &src.sends {
                            if send.target == ch.id {
                                out.push((
                                    GraphCable::Send {
                                        source: src.id,
                                        target: ch.id,
                                        amount: send.amount,
                                    },
                                    format!("SEND {} → {}", src.name, ch.name),
                                    format!("{:.0}%", send.amount * 100.0),
                                ));
                            }
                        }
                    }
                }
            }
        }
        // 2. Modulations touching this node (source pattern OR target node).
        for m in &project.modulations {
            let pat_hit = matches!(target, GraphTarget::Pattern(pid) if m.pattern == pid);
            let dst_hit = match (target, m.target) {
                (GraphTarget::Instrument(id), ModulationTarget::Instrument { instrument, .. }) => {
                    id == instrument
                }
                (GraphTarget::Effect(id), ModulationTarget::Effect { device, .. }) => id == device,
                (
                    GraphTarget::Channel(ch),
                    ModulationTarget::ChannelGain { channel }
                    | ModulationTarget::ChannelPan { channel },
                ) => ch == channel,
                (
                    GraphTarget::Channel(ch),
                    ModulationTarget::Send {
                        channel,
                        target: ret_id,
                    },
                ) => ch == channel || ch == ChannelRef::Channel(ret_id),
                (GraphTarget::Macro(idx), ModulationTarget::Macro { index }) => idx == index,
                _ => false,
            };
            if pat_hit || dst_hit {
                let pat_name = project.pattern(m.pattern).map_or("?", |p| p.name.as_str());
                out.push((
                    GraphCable::Modulation(m.id),
                    format!("MOD {}", m.name),
                    format!("from {pat_name}"),
                ));
            }
        }
        // 3. Sidechain Ducks touching this Part or Channel.
        for d in &project.ducks {
            let hit = match target {
                GraphTarget::Part(pid) => d.source_part == pid,
                GraphTarget::Channel(ch) => d.target == ch,
                _ => false,
            };
            if hit {
                let src_name = project.part(d.source_part).map_or("?", |p| p.name.as_str());
                let dst_name = self.channel_name(d.target);
                out.push((
                    GraphCable::Duck(d.id),
                    format!("DUCK {}", d.name),
                    format!("{src_name} → {dst_name} ({:.0}%)", d.depth * 100.0),
                ));
            }
        }
        // 4. Filter envelope pattern link (`FilterPatch.env_pattern_id`).
        let push_filter_env = |out: &mut Vec<(GraphCable, String, String)>,
                               dev_id: Id,
                               dev_name: &str,
                               pat_id: Id| {
            let pat_name = project.pattern(pat_id).map_or("?", |p| p.name.as_str());
            out.push((
                GraphCable::FilterEnv {
                    device: dev_id,
                    pattern: pat_id,
                },
                format!("FILTER ENV {dev_name}"),
                format!("from {pat_name}"),
            ));
        };
        for ch in &project.mixer.channels {
            for dev in &ch.inserts {
                if let EffectKind::Filter(fp) = dev.kind {
                    if let Some(pat_id) = fp.env_pattern_id {
                        if matches!(target, GraphTarget::Effect(id) if id == dev.id)
                            || matches!(target, GraphTarget::Pattern(id) if id == pat_id)
                        {
                            push_filter_env(&mut out, dev.id, &dev.name, pat_id);
                        }
                    }
                }
            }
        }
        for dev in &project.mixer.master.inserts {
            if let EffectKind::Filter(fp) = dev.kind {
                if let Some(pat_id) = fp.env_pattern_id {
                    if matches!(target, GraphTarget::Effect(id) if id == dev.id)
                        || matches!(target, GraphTarget::Pattern(id) if id == pat_id)
                    {
                        push_filter_env(&mut out, dev.id, &dev.name, pat_id);
                    }
                }
            }
        }
        // 5. Part parameter_links (`Pattern -> Part`).
        for part in &project.parts {
            for (idx, link) in part.parameter_links.iter().enumerate() {
                if matches!(target, GraphTarget::Part(id) if id == part.id)
                    || matches!(target, GraphTarget::Pattern(id) if id == link.pattern)
                {
                    let pat_name = project
                        .pattern(link.pattern)
                        .map_or("?", |p| p.name.as_str());
                    let p_label = self.format_instrument_param_short(part.instrument, link.target);
                    out.push((
                        GraphCable::ParamLink {
                            part: part.id,
                            index: idx,
                            pattern: link.pattern,
                        },
                        format!("LINK {} ← {pat_name}", part.name),
                        p_label,
                    ));
                }
            }
        }
        // 6. Non-Master Group bus output route (editable via bus picker / resettable to Master).
        if let GraphTarget::Channel(ChannelRef::Channel(cid)) = target {
            if let Some(ch) = project.channel(cid) {
                if ch.kind != ChannelKind::Return && ch.output != ChannelRef::Master {
                    out.push((
                        GraphCable::BusRoute {
                            channel: ch.id,
                            output: ch.output,
                        },
                        format!("ROUTE {} → {}", ch.name, self.channel_name(ch.output)),
                        "output bus route".into(),
                    ));
                }
            }
        }
        // 7. Project Macro bindings touching this Macro node or destination node.
        let mut macros: Vec<_> = project.macros.iter().collect();
        macros.sort_by_key(|m| m.index);
        for mac in macros {
            for (b_idx, binding) in mac.bindings.iter().enumerate() {
                let macro_hit = matches!(target, GraphTarget::Macro(idx) if idx == mac.index);
                let dst_hit = match (target, binding.target) {
                    (
                        GraphTarget::Instrument(id),
                        ParamAddress::Rack { instrument, .. }
                        | ParamAddress::Zone { instrument, .. }
                        | ParamAddress::Module { instrument, .. }
                        | ParamAddress::Cable { instrument, .. },
                    ) => id == instrument,
                    (GraphTarget::Effect(id), ParamAddress::Effect { device, .. }) => id == device,
                    (GraphTarget::Channel(ch), ParamAddress::Channel { channel, .. }) => {
                        ch == channel
                    }
                    (
                        GraphTarget::Channel(ch),
                        ParamAddress::Send {
                            channel,
                            target: ret_id,
                        },
                    ) => ch == channel || ch == ChannelRef::Channel(ret_id),
                    _ => false,
                };
                if macro_hit || dst_hit {
                    let (_, target_name) = self.format_macro_target(binding.target);
                    let arrow = if binding.is_inverted() { "↓" } else { "→" };
                    out.push((
                        GraphCable::MacroBinding {
                            index: mac.index,
                            binding: b_idx,
                        },
                        format!(
                            "MACRO M{} {} → {target_name}",
                            usize::from(mac.index) + 1,
                            mac.name
                        ),
                        format!(
                            "{:.0}% {arrow} {:.0}%",
                            binding.min * 100.0,
                            binding.max * 100.0
                        ),
                    ));
                }
            }
        }
        out
    }

    pub(crate) fn graph_removable_cables(
        &self,
        target: GraphTarget,
    ) -> Vec<(GraphCable, String, String)> {
        self.graph_editable_cables(target)
    }

    pub(crate) fn unlink_graph_cable(&mut self, cable: GraphCable) -> Result<Outcome> {
        match cable {
            GraphCable::Send { source, target, .. } => self.apply(Action::SetChannelSend {
                channel: source,
                target,
                amount: None,
            }),
            GraphCable::Modulation(id) => {
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
            GraphCable::Duck(id) => {
                let name = self
                    .project()
                    .ducks
                    .iter()
                    .find(|d| d.id == id)
                    .map_or_else(|| format!("#{id}"), |d| d.name.clone());
                self.commit(
                    vec![Edit::RemoveDuck { id }],
                    format!("Unlinked duck '{name}' · u restores"),
                )
            }
            GraphCable::FilterEnv { device, .. } => {
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
            GraphCable::ParamLink { part, index, .. } => {
                let mut item = self.part(part).context("Part missing")?.clone();
                if index < item.parameter_links.len() {
                    item.parameter_links.remove(index);
                }
                let name = item.name.clone();
                self.commit(
                    vec![Edit::PutPart { part: item }],
                    format!("Unlinked parameter link on '{name}' · u restores"),
                )
            }
            GraphCable::BusRoute { channel, .. } => self.apply(Action::SetChannelOutput {
                channel,
                output: ChannelRef::Master,
            }),
            GraphCable::MacroBinding { index, binding } => self.apply(Action::UnbindMacro {
                index,
                binding: Some(binding),
            }),
        }
    }

    pub(crate) fn unlink_all_graph_cables(&mut self, target: GraphTarget) -> Result<Outcome> {
        let cables = self.graph_removable_cables(target);
        if cables.is_empty() {
            self.status = "No removable cables attached to selected node".into();
            return Ok(Outcome::default());
        }
        if cables.len() == 1 {
            return self.unlink_graph_cable(cables[0].0);
        }
        let mut edits: Vec<Edit> = Vec::new();
        let mut mod_ids: HashSet<Id> = HashSet::new();
        let mut duck_ids: HashSet<Id> = HashSet::new();
        let mut send_removals: HashMap<Id, HashSet<Id>> = HashMap::new();
        let mut bus_resets: HashSet<Id> = HashSet::new();
        let mut filter_env_devs: HashSet<Id> = HashSet::new();
        let mut part_link_removals: HashMap<Id, Vec<usize>> = HashMap::new();
        let mut macro_binding_removals: HashMap<u8, Vec<usize>> = HashMap::new();

        for (cable, _, _) in &cables {
            match *cable {
                GraphCable::Modulation(id) => {
                    mod_ids.insert(id);
                }
                GraphCable::Duck(id) => {
                    duck_ids.insert(id);
                }
                GraphCable::Send { source, target, .. } => {
                    send_removals.entry(source).or_default().insert(target);
                    for m in &self.project().modulations {
                        if matches!(
                            m.target,
                            ModulationTarget::Send {
                                channel: ChannelRef::Channel(ch),
                                target: ret,
                            } if ch == source && ret == target
                        ) {
                            mod_ids.insert(m.id);
                        }
                    }
                }
                GraphCable::BusRoute { channel, .. } => {
                    bus_resets.insert(channel);
                }
                GraphCable::FilterEnv { device, .. } => {
                    filter_env_devs.insert(device);
                }
                GraphCable::ParamLink { part, index, .. } => {
                    part_link_removals.entry(part).or_default().push(index);
                }
                GraphCable::MacroBinding { index, binding } => {
                    macro_binding_removals
                        .entry(index)
                        .or_default()
                        .push(binding);
                }
            }
        }
        for id in mod_ids {
            edits.push(Edit::RemoveModulation { id });
        }
        for id in duck_ids {
            edits.push(Edit::RemoveDuck { id });
        }
        let mut touched_channels: HashSet<Id> = send_removals.keys().copied().collect();
        touched_channels.extend(bus_resets.iter().copied());
        for ch_id in touched_channels {
            if let Some(ch) = self.channel(ch_id) {
                let sends = send_removals.get(&ch_id).map(|targets| {
                    ch.sends
                        .iter()
                        .filter(|s| !targets.contains(&s.target))
                        .cloned()
                        .collect()
                });
                let output = bus_resets.contains(&ch_id).then_some(ChannelRef::Master);
                edits.push(Edit::UpdateChannelMeta {
                    id: ch_id,
                    name: None,
                    output,
                    sends,
                });
            }
        }
        for dev_id in filter_env_devs {
            edits.push(self.effect_edit(dev_id, |d| {
                if let EffectKind::Filter(fp) = &mut d.kind {
                    fp.env_pattern_id = None;
                }
                Ok(())
            })?);
        }
        for (part_id, mut indices) in part_link_removals {
            if let Some(mut part) = self.part(part_id).cloned() {
                indices.sort_unstable();
                indices.dedup();
                for idx in indices.into_iter().rev() {
                    if idx < part.parameter_links.len() {
                        part.parameter_links.remove(idx);
                    }
                }
                edits.push(Edit::PutPart { part });
            }
        }
        for (index, mut bindings) in macro_binding_removals {
            if let Some(mut mac) = self.project().macro_control(index).cloned() {
                bindings.sort_unstable();
                bindings.dedup();
                for b_idx in bindings.into_iter().rev() {
                    if b_idx < mac.bindings.len() {
                        mac.bindings.remove(b_idx);
                    }
                }
                edits.push(Edit::PutMacro { macro_control: mac });
            }
        }
        let n = cables.len();
        self.commit(edits, format!("Unlinked {n} cables · u restores"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadence_core::material::Project;
    use cadence_core::runtime::ChannelMeter;

    #[test]
    fn graph_nodes_format_gain_db_and_preserve_unclamped_float_headroom_and_flags() {
        let mut proj = Project::demo();
        let ch0_id = proj.mixer.channels[0].id;
        proj.mixer.channels[0].strip.gain = 0.0;
        proj.mixer.master.strip.gain = 2.0; // +6.0 dB
        let inst0_id = proj.instruments[0].id;
        let inst0_zones = proj.instruments[0].rack.zones.len();
        let inst0_mods: usize = proj.instruments[0]
            .rack
            .zones
            .iter()
            .map(|z| z.modules.len())
            .sum();
        let app = App::new_in_memory(proj, "graph-unit.json".into()).unwrap();

        let mut snap = AudioSnapshot {
            peak: 1.5,
            track_count: 1,
            track_waveforms: vec![vec![0.2, 1.35, -0.5]],
            channel_meters: vec![
                ChannelMeter {
                    channel: ChannelRef::Channel(ch0_id),
                    output_peak: [1.4, 0.9],
                    hot: true,
                    clipped: false,
                    ..ChannelMeter::default()
                },
                ChannelMeter {
                    channel: ChannelRef::Master,
                    output_peak: [1.5, 1.2],
                    hot: true,
                    clipped: true,
                    ..ChannelMeter::default()
                },
            ],
            ..AudioSnapshot::default()
        };
        snap.track_ids[0] = inst0_id;

        let view = app.build_graph_view(&snap);
        let find_node = |target: GraphTarget| {
            view.columns
                .iter()
                .flat_map(|c| c.nodes.iter())
                .find(|n| n.target == target)
                .unwrap()
        };
        let inst_node = find_node(GraphTarget::Instrument(inst0_id));
        assert_eq!(inst_node.badge, "RACK");
        assert!(
            inst_node.detail.contains(&format!("{inst0_zones}z"))
                && inst_node.detail.contains(&format!("{inst0_mods}m")),
            "expected Rack zone/module detail, got {:?}",
            inst_node.detail
        );
        assert!((inst_node.activity - 1.35).abs() < 1e-5);
        assert!(inst_node.hot);
        assert!(!inst_node.clipped);

        let ch_node = find_node(GraphTarget::Channel(ChannelRef::Channel(ch0_id)));
        assert!(
            ch_node.detail.starts_with("-inf dB"),
            "expected format_gain_db `-inf dB`, got {:?}",
            ch_node.detail
        );
        assert!((ch_node.activity - 1.4).abs() < 1e-5);
        assert!(ch_node.hot);
        assert!(!ch_node.clipped);

        let master_node = find_node(GraphTarget::Channel(ChannelRef::Master));
        assert_eq!(master_node.detail, "+6.0 dB");
        assert!((master_node.activity - 1.5).abs() < 1e-5);
        assert!(master_node.hot);
        assert!(master_node.clipped);
    }
}
