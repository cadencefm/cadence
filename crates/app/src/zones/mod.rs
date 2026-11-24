//! Interactive Rack zone editor controls, commands, and mutations.
mod edit;
mod ops;

pub(crate) use ops::{next_free_midi, AddZoneTemplate};

use crate::*;
use cadence_core::music::note_name;
use cadence_core::params::{ParamAddress, ZoneParam};
use cadence_core::rack::{ModuleKind, RackPatch};

/// Interactive structural zone-level control targets for `Detail::Instrument(id)` on the Zones tab.
///
/// Continuous/automatable zone parameters (`Zone Gain`, `Zone Pan`, `Root Note`, `VOICE` settings,
/// and all inner module / cable parameters) use first-class [`ControlTarget::Param`] so MIDI Learn,
/// Live CC Recording, Macros, Modulations, and Undo coalescing share one path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZoneControl {
    Name,
    LowMidi,
    HighMidi,
    LowVel,
    HighVel,
    ChokeGroup,
    SamplePath,
}

impl ZoneControl {
    pub fn is_adjustable(self) -> bool {
        !matches!(self, Self::Name | Self::SamplePath)
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Name => "Zone Name",
            Self::LowMidi => "MIDI Low",
            Self::HighMidi => "MIDI High",
            Self::LowVel => "Velocity Low",
            Self::HighVel => "Velocity High",
            Self::ChokeGroup => "Choke Group",
            Self::SamplePath => "Sample",
        }
    }

    pub(crate) fn units_hint(self) -> Option<&'static str> {
        match self {
            Self::SamplePath => None,
            Self::Name => Some("1–32 printable characters"),
            Self::LowMidi | Self::HighMidi => Some("MIDI 0–127 or note name (e.g. C2, F#3)"),
            Self::LowVel | Self::HighVel => Some("0..1 or 0%..100%"),
            Self::ChokeGroup => Some("none / off / 0, or group 1..8"),
        }
    }
}

pub(super) fn compact_text(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars || max_chars <= 1 {
        text.to_string()
    } else {
        let head: String = text.chars().take(max_chars - 1).collect();
        format!("{head}…")
    }
}

fn compact_path(path: &str, max_chars: usize) -> String {
    let count = path.chars().count();
    if count <= max_chars || max_chars <= 2 {
        return path.to_string();
    }
    if let Some(file) = Path::new(path).file_name().and_then(|s| s.to_str()) {
        let fcount = file.chars().count();
        if fcount <= max_chars {
            return file.to_string();
        }
        let tail: String = file
            .chars()
            .skip(fcount.saturating_sub(max_chars - 1))
            .collect();
        return format!("…{tail}");
    }
    let tail: String = path
        .chars()
        .skip(count.saturating_sub(max_chars - 1))
        .collect();
    format!("…{tail}")
}

pub(super) fn format_midi_note(midi: u8) -> String {
    format!("{} ({midi})", note_name(midi))
}

impl App {
    /// Resolve the active Rack instrument ID (every Cadence instrument is a unified `RackPatch`).
    pub(crate) fn resolve_rack_instrument_id(&self, explicit: Option<Id>) -> Result<Id> {
        let id = explicit
            .or_else(|| self.active_instrument())
            .or(self.selected_instrument)
            .context("Select an instrument first")?;
        ensure!(self.instrument(id).is_some(), "Instrument missing");
        Ok(id)
    }

    /// Build interactive controls for the focused zone on the `Zones` tab of a Rack instrument.
    pub(crate) fn zone_control_descriptors(&self, id: Id, rack: &RackPatch) -> Vec<Control> {
        if rack.zones.is_empty() {
            return Vec::new();
        }
        let idx = rack.clamp_zone_index(self.rack_ui.zone_cursor);
        let zone = &rack.zones[idx];
        let zone_id = zone.id;
        let mut out = Vec::with_capacity(15);
        let make_struct = |group: &'static str,
                           label: &str,
                           display: String,
                           normalized: Option<f32>,
                           field: ZoneControl| Control {
            group,
            label: label.into(),
            display,
            normalized,
            target: ControlTarget::Zone {
                instrument: id,
                field,
            },
            controlled: false,
        };
        let make_zone_param = |group: &'static str, label: &str, param: ZoneParam| {
            let addr = ParamAddress::Zone {
                instrument: id,
                zone: zone_id,
                param,
            };
            let norm = param.read_normalized(zone);
            let display = param.format_native(param.read_native(zone));
            Control {
                group,
                label: label.into(),
                display,
                normalized: Some(norm),
                target: ControlTarget::Param(addr),
                controlled: self.param_is_controlled(addr),
            }
        };

        out.push(make_struct(
            "ZONE",
            "Name",
            compact_text(&zone.name, 16),
            None,
            ZoneControl::Name,
        ));
        out.push(make_struct(
            "ZONE",
            "MIDI Low",
            format_midi_note(zone.low_midi),
            Some(f32::from(zone.low_midi) / 127.0),
            ZoneControl::LowMidi,
        ));
        out.push(make_struct(
            "ZONE",
            "MIDI High",
            format_midi_note(zone.high_midi),
            Some(f32::from(zone.high_midi) / 127.0),
            ZoneControl::HighMidi,
        ));
        out.push(make_zone_param("ZONE", "Root Note", ZoneParam::RootMidi));
        out.push(make_struct(
            "ZONE",
            "Vel Low",
            format!("{:.0}%", zone.low_vel * 100.0),
            Some(zone.low_vel.clamp(0.0, 1.0)),
            ZoneControl::LowVel,
        ));
        out.push(make_struct(
            "ZONE",
            "Vel High",
            format!("{:.0}%", zone.high_vel * 100.0),
            Some(zone.high_vel.clamp(0.0, 1.0)),
            ZoneControl::HighVel,
        ));
        out.push(make_zone_param("ZONE", "Zone Gain", ZoneParam::Gain));
        out.push(make_zone_param("ZONE", "Zone Pan", ZoneParam::Pan));
        out.push(make_struct(
            "ZONE",
            "Choke Group",
            match zone.choke_group {
                None => "None".into(),
                Some(g) => format!("Group {g}"),
            },
            None,
            ZoneControl::ChokeGroup,
        ));

        let sampler_path = zone.modules.iter().find_map(|m| match &m.kind {
            ModuleKind::Sampler(s) => Some(s.path.as_deref()),
            _ => None,
        });
        let sample_display = match sampler_path {
            Some(Some(p)) if !p.trim().is_empty() => compact_path(p, 17),
            Some(_) => "<unassigned> (Enter)".into(),
            None => "<add Sampler> (Enter)".into(),
        };
        out.push(make_struct(
            "ZONE",
            "Sample",
            sample_display,
            None,
            ZoneControl::SamplePath,
        ));

        for param in [
            ZoneParam::VoiceMode,
            ZoneParam::Legato,
            ZoneParam::Glide,
            ZoneParam::PitchTracking,
            ZoneParam::PhaseReset,
        ] {
            out.push(make_zone_param("VOICE", param.label(), param));
        }

        out
    }

    /// Step `zone_cursor` across the active Rack instrument's zones (wrapping cleanly).
    pub fn step_zone_cursor(&mut self, delta: isize) {
        let Some(id) = self.detail_instrument().or(self.selected_instrument) else {
            return;
        };
        self.step_zone_cursor_for(id, delta);
    }

    pub(crate) fn step_zone_cursor_for(&mut self, id: Id, delta: isize) {
        let (next, summary) = {
            let Some(inst) = self.instrument(id) else {
                return;
            };
            let rack = &inst.rack;
            if rack.zones.is_empty() {
                self.rack_ui.zone_cursor = 0;
                return;
            }
            let len = rack.zones.len() as isize;
            let cur = rack.clamp_zone_index(self.rack_ui.zone_cursor) as isize;
            let next = (cur + delta).rem_euclid(len) as usize;
            let zone = &rack.zones[next];
            let summary = format!(
                "Zone {}/{}: {} ({}m/{}c)",
                next + 1,
                rack.zones.len(),
                zone.name,
                zone.modules.len(),
                zone.cables.len()
            );
            (next, summary)
        };
        self.rack_ui.zone_cursor = next;
        if let Some(rack) = self.instrument(id).map(|i| i.rack.clone()) {
            self.clamp_rack_state(&rack);
        }
        let count = self.control_descriptors().len();
        self.control_cursor = self.control_cursor.min(count.saturating_sub(1));
        self.status = summary;
    }
}
