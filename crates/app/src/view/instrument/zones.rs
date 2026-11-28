//! Rack zone table, module pill strip, and selected-zone summary projection.
use super::{RackModulePillView, RackZoneView};
use crate::rack::rack_panel_pills;
use cadence_core::music::note_name;
use cadence_core::params::format_pan;
use cadence_core::rack::{ModuleKind, RackPatch, RackZone};

pub(super) fn build_rack_zone_views(rack: &RackPatch, selected_zone: usize) -> Vec<RackZoneView> {
    rack.zones
        .iter()
        .enumerate()
        .map(|(i, z)| {
            let note_range = if z.low_midi == z.high_midi {
                note_name(z.low_midi)
            } else {
                format!("{}–{}", note_name(z.low_midi), note_name(z.high_midi))
            };
            let vel_range = if z.low_vel <= 0.01 && z.high_vel >= 0.99 {
                "FULL".into()
            } else {
                format!("{:.0}–{:.0}%", z.low_vel * 100.0, z.high_vel * 100.0)
            };
            let has_sampler = z
                .modules
                .iter()
                .any(|m| matches!(m.kind, ModuleKind::Sampler(_)));
            let source_tag = if has_sampler {
                "SAMPLER"
            } else if z
                .modules
                .iter()
                .any(|m| matches!(m.kind, ModuleKind::Vco(_)))
            {
                "SYNTH"
            } else if z
                .modules
                .iter()
                .any(|m| matches!(m.kind, ModuleKind::Aero(_)))
            {
                "AERO"
            } else if z
                .modules
                .iter()
                .any(|m| matches!(m.kind, ModuleKind::Noise(_)))
            {
                "NOISE"
            } else {
                "RACK"
            };
            let sample_label = z.modules.iter().find_map(|m| match &m.kind {
                ModuleKind::Sampler(s) => s.path.as_deref().and_then(|raw| {
                    let trimmed = raw.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(
                            std::path::Path::new(trimmed)
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or(trimmed)
                                .to_string(),
                        )
                    }
                }),
                _ => None,
            });
            let voice_summary = {
                let mut parts = vec![z.voice.voice_mode.label().to_string()];
                if z.voice.legato {
                    parts.push("Legato".into());
                }
                if z.voice.glide_ms >= 0.5 {
                    parts.push(format!("{:.0}ms", z.voice.glide_ms));
                }
                parts.push(z.voice.pitch_tracking.label().to_string());
                parts.join(" · ")
            };
            RackZoneView {
                id: z.id,
                name: z.name.clone(),
                low_midi: z.low_midi,
                high_midi: z.high_midi,
                root_midi: z.root_midi,
                low_vel: z.low_vel,
                high_vel: z.high_vel,
                note_range,
                vel_range,
                voice_summary,
                sample_label,
                module_count: z.modules.len(),
                cable_count: z.cables.len(),
                has_sampler,
                source_tag,
                choke_group: z.choke_group,
                selected: i == selected_zone,
            }
        })
        .collect()
}

pub(super) fn build_rack_module_pills(
    rack: &RackPatch,
    zone: &RackZone,
    panel_slot: usize,
) -> (Vec<RackModulePillView>, Option<u16>) {
    let pills = rack_panel_pills(rack, zone);
    let active_slot = panel_slot.min(pills.len().saturating_sub(1));
    let active_module_id = pills.get(active_slot).and_then(|p| p.module_id);
    let modules = pills
        .iter()
        .enumerate()
        .map(|(idx, p)| match p.module_id {
            None => RackModulePillView {
                module_id: None,
                kind: None,
                label: if rack.panel_pins.is_empty() {
                    "Rack".into()
                } else {
                    "Quick".into()
                },
                tag: if rack.panel_pins.is_empty() {
                    "RACK"
                } else {
                    "PINS"
                },
                column: 0,
                active: idx == active_slot,
            },
            Some(mid) => {
                let (kind, label, tag, column) = zone.module(mid).map_or_else(
                    || (None, format!("Mod {mid}"), "MOD", 0),
                    |m| {
                        let t = m.kind.tag();
                        (Some(t), m.name.clone(), t.short_badge(), m.kind.column())
                    },
                );
                RackModulePillView {
                    module_id: Some(mid),
                    kind,
                    label,
                    tag,
                    column,
                    active: idx == active_slot,
                }
            }
        })
        .collect();
    (modules, active_module_id)
}

pub(super) fn format_selected_zone_summary(
    zone: &RackZone,
    selected_zone: usize,
    zone_count: usize,
) -> String {
    let lo = note_name(zone.low_midi);
    let hi = note_name(zone.high_midi);
    let gain = if zone.gain_db.abs() < 0.05 {
        "0.0 dB".to_string()
    } else {
        format!("{:+.1} dB", zone.gain_db)
    };
    let pan = format_pan(zone.pan);
    let choke = match zone.choke_group {
        Some(g) => format!("Choke {g}"),
        None => "Choke —".into(),
    };
    format!(
        "ZONE {}/{zone_count} · {} · {lo}..{hi} · Vel {:.0}-{:.0}% · {}m/{}c · {gain} · {pan} · {choke}",
        selected_zone + 1,
        zone.name,
        zone.low_vel * 100.0,
        zone.high_vel * 100.0,
        zone.modules.len(),
        zone.cables.len(),
    )
}
