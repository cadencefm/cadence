//! Rack zone structural mutations (`apply_zone_*`).
use super::{compact_text, format_midi_note};
use crate::*;
use cadence_core::music::note_name;
use cadence_core::presets::InstrumentPreset;
use cadence_core::rack::{
    ModuleKind, PitchTracking, RackPatch, RackTemplate, RackZone, TriggerMode, MAX_ZONES,
};

pub(crate) fn next_free_midi(rack: &RackPatch) -> u8 {
    for candidate in 36u8..=127 {
        if !rack
            .zones
            .iter()
            .any(|z| z.low_midi <= candidate && candidate <= z.high_midi)
        {
            return candidate;
        }
    }
    rack.zones
        .last()
        .map_or(36, |z| z.high_midi.saturating_add(1).min(127))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AddZoneTemplate {
    BlankSampler,
    SynthPad,
    BlankModular,
}

fn default_synth_pad_zone(zone_id: u16, name: String, midi: u8) -> RackZone {
    let mut zone = RackPatch::default_synth()
        .zones
        .into_iter()
        .next()
        .unwrap_or_else(|| RackZone::blank_sample(zone_id, name.clone()));
    let midi = midi.min(127);
    zone.id = zone_id;
    zone.name = name;
    zone.low_midi = midi;
    zone.high_midi = midi;
    zone.root_midi = midi;
    zone.voice.pitch_tracking = PitchTracking::Fixed;
    zone.voice.phase_reset = true;
    for m in &mut zone.modules {
        match &mut m.kind {
            ModuleKind::Vco(v) => {
                v.pitch_tracking = PitchTracking::Fixed;
                v.fixed_hz = cadence_core::dsp::midi_to_hz(f64::from(midi)) as f32;
            }
            ModuleKind::Env(e) => {
                e.trigger_mode = TriggerMode::OneShot;
                e.attack_ms = 0.5;
                e.decay_ms = 220.0;
                e.sustain = 0.0;
                e.release_ms = 40.0;
            }
            _ => {}
        }
    }
    zone
}

impl App {
    pub(super) fn clone_rack_patch(&self, id: Id) -> Result<RackPatch> {
        Ok(self
            .instrument(id)
            .context("Instrument missing")?
            .rack
            .clone())
    }

    pub(crate) fn apply_zone_select(
        &mut self,
        instrument: Option<Id>,
        query: &str,
    ) -> Result<Outcome> {
        let id = self.resolve_rack_instrument_id(instrument)?;
        let (idx, summary, rack_clone) = {
            let rack = &self.instrument(id).context("Instrument missing")?.rack;
            let idx = rack
                .find_zone(query)
                .with_context(|| format!("Zone '{query}' not found"))?;
            let zone = &rack.zones[idx];
            let summary = format!(
                "Zone {}/{}: {} ({}m/{}c)",
                idx + 1,
                rack.zones.len(),
                zone.name,
                zone.modules.len(),
                zone.cables.len()
            );
            (idx, summary, rack.clone())
        };
        self.rack_ui.zone_cursor = idx;
        self.clamp_rack_state(&rack_clone);
        let count = self.control_descriptors().len();
        self.control_cursor = self.control_cursor.min(count.saturating_sub(1));
        self.status = summary;
        Ok(Outcome::default())
    }

    pub(crate) fn apply_zone_step(
        &mut self,
        instrument: Option<Id>,
        delta: isize,
    ) -> Result<Outcome> {
        let id = self.resolve_rack_instrument_id(instrument)?;
        self.step_zone_cursor_for(id, delta);
        Ok(Outcome::default())
    }

    pub(crate) fn apply_zone_add(
        &mut self,
        instrument: Option<Id>,
        name: Option<String>,
        midi: Option<u8>,
        sample_path: Option<String>,
    ) -> Result<Outcome> {
        let id = self.resolve_rack_instrument_id(instrument)?;
        let rack = self.clone_rack_patch(id)?;
        ensure!(
            rack.zones.len() < MAX_ZONES,
            "Rack already has the maximum {MAX_ZONES} zones"
        );
        let zone_id = rack.next_zone_id();
        let note = midi.unwrap_or_else(|| next_free_midi(&rack)).min(127);
        let zone_name = name
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| {
                if let Some(path) = &sample_path {
                    Path::new(path)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .filter(|s| !s.is_empty())
                        .map(|s| compact_text(s, 24))
                        .unwrap_or_else(|| format!("Pad {}", rack.zones.len() + 1))
                } else {
                    format!("Pad {}", rack.zones.len() + 1)
                }
            });
        let mut zone = if let Some(path) = &sample_path {
            let mut z = RackZone::sample_pad(zone_id, zone_name.clone(), note, path.trim());
            self.load_and_apply_zone_sample(&mut z, Some(1), path, note, true, false)?;
            z
        } else {
            default_synth_pad_zone(zone_id, zone_name.clone(), note)
        };
        if let Some(explicit_midi) = midi {
            if zone.low_midi == zone.high_midi && zone.root_midi != explicit_midi.min(127) {
                zone.set_midi_bounds(explicit_midi, explicit_midi, Some(explicit_midi));
            }
        }
        let zone_root = zone.root_midi;
        let new_idx = rack.zones.len();
        let out = self.commit(
            vec![Edit::AddRackZone {
                instrument: id,
                zone,
                index: None,
            }],
            format!(
                "Added zone {}: {zone_name} ({})",
                new_idx + 1,
                format_midi_note(zone_root),
            ),
        )?;
        self.rack_ui.zone_cursor = new_idx;
        if let Some(r) = self.instrument(id).map(|i| i.rack.clone()) {
            self.clamp_rack_state(&r);
        }
        Ok(out)
    }

    pub(crate) fn add_rack_zone_template(
        &mut self,
        instrument_id: Id,
        template: AddZoneTemplate,
    ) -> Result<Outcome> {
        let rack = self.clone_rack_patch(instrument_id)?;
        ensure!(
            rack.zones.len() < MAX_ZONES,
            "Rack already has the maximum {MAX_ZONES} zones"
        );
        let zone_id = rack.next_zone_id();
        let next_num = rack.zones.len() + 1;
        let is_pad_kit = rack.zones.iter().any(|z| z.low_midi == z.high_midi);
        let (zone, kind_label) = match template {
            AddZoneTemplate::BlankSampler => {
                let name = crate::unique_name(
                    &format!("Zone {next_num}"),
                    rack.zones.iter().map(|z| z.name.as_str()),
                    Some(32),
                );
                let mut z = RackZone::blank_sample(zone_id, name);
                if is_pad_kit {
                    let note = next_free_midi(&rack);
                    z.set_midi_bounds(note, note, Some(note));
                }
                (z, "Blank Sampler")
            }
            AddZoneTemplate::SynthPad => {
                let note = next_free_midi(&rack);
                let name = crate::unique_name(
                    &format!("Pad {next_num}"),
                    rack.zones.iter().map(|z| z.name.as_str()),
                    Some(32),
                );
                (default_synth_pad_zone(zone_id, name, note), "Synth Pad")
            }
            AddZoneTemplate::BlankModular => {
                let name = crate::unique_name(
                    &format!("Modular {next_num}"),
                    rack.zones.iter().map(|z| z.name.as_str()),
                    Some(32),
                );
                let mut z = RackPatch::from_template(RackTemplate::Modular)
                    .zones
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| RackZone::blank_sample(zone_id, name.clone()));
                z.id = zone_id;
                z.name = name;
                if is_pad_kit {
                    let note = next_free_midi(&rack);
                    z.set_midi_bounds(note, note, Some(note));
                }
                (z, "Blank Modular")
            }
        };
        let zname = zone.name.clone();
        let new_idx = rack.zones.len();
        let out = self.commit(
            vec![Edit::AddRackZone {
                instrument: instrument_id,
                zone,
                index: None,
            }],
            format!("Added {kind_label} zone '{zname}' (z{zone_id})"),
        )?;
        self.rack_ui.zone_cursor = new_idx;
        self.rack_ui.panel_slot = 0;
        self.rack_ui.patchbay_cable_id = None;
        if let Some(r) = self.instrument(instrument_id).map(|i| i.rack.clone()) {
            self.clamp_rack_state(&r);
        }
        let count = self.control_descriptors().len();
        self.control_cursor = self.control_cursor.min(count.saturating_sub(1));
        Ok(out)
    }

    pub(crate) fn add_rack_zone_from_preset(
        &mut self,
        instrument_id: Id,
        preset_spec: &str,
    ) -> Result<Outcome> {
        let preset = self.resolve_instrument_preset(preset_spec)?;
        let preset_name = preset.name.clone();
        ensure!(
            preset.patch.zones.len() == 1,
            "Cannot append multi-zone preset '{preset_name}' as a single zone (use P Preset Browser on the Rack)"
        );
        let rack = self.clone_rack_patch(instrument_id)?;
        ensure!(
            rack.zones.len() < MAX_ZONES,
            "Rack already has the maximum {MAX_ZONES} zones"
        );
        let mut zone = preset
            .patch
            .zones
            .into_iter()
            .next()
            .context("Preset has no zones")?;
        let zone_id = rack.next_zone_id();
        zone.id = zone_id;
        let zname = crate::unique_name(
            &compact_text(&preset_name, 28),
            rack.zones.iter().map(|z| z.name.as_str()),
            Some(32),
        );
        zone.set_name(&zname)?;
        if rack.zones.iter().any(|z| z.low_midi == z.high_midi) {
            let note = next_free_midi(&rack);
            zone.set_midi_bounds(note, note, Some(note));
            for m in &mut zone.modules {
                if let ModuleKind::Sampler(s) = &mut m.kind {
                    s.root_midi = note;
                }
            }
        }
        let has_samplers = zone
            .modules
            .iter()
            .any(|m| matches!(m.kind, ModuleKind::Sampler(_)));
        let new_idx = rack.zones.len();
        let mut out = self.commit(
            vec![Edit::AddRackZone {
                instrument: instrument_id,
                zone,
                index: None,
            }],
            format!("Added zone '{zname}' from preset '{preset_name}'"),
        )?;
        self.rack_ui.zone_cursor = new_idx;
        self.rack_ui.panel_slot = 0;
        self.rack_ui.patchbay_cable_id = None;
        if let Some(r) = self.instrument(instrument_id).map(|i| i.rack.clone()) {
            self.clamp_rack_state(&r);
        }
        let count = self.control_descriptors().len();
        self.control_cursor = self.control_cursor.min(count.saturating_sub(1));
        if has_samplers {
            self.sync_loaded_samples();
            out.samples_changed = true;
        }
        Ok(out)
    }

    pub(crate) fn apply_zone_duplicate(
        &mut self,
        instrument: Option<Id>,
        name: Option<String>,
    ) -> Result<Outcome> {
        let id = self.resolve_rack_instrument_id(instrument)?;
        let rack = self.clone_rack_patch(id)?;
        ensure!(!rack.zones.is_empty(), "Rack has no zones");
        ensure!(
            rack.zones.len() < MAX_ZONES,
            "Rack already has the maximum {MAX_ZONES} zones"
        );
        let cur = rack.clamp_zone_index(self.rack_ui.zone_cursor);
        let mut dup = rack.zones[cur].clone();
        dup.id = rack.next_zone_id();
        let new_name = name
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| {
                crate::unique_name(
                    &format!("{} Copy", dup.name),
                    rack.zones.iter().map(|z| z.name.as_str()),
                    Some(32),
                )
            });
        dup.set_name(&new_name)?;
        if dup.low_midi == dup.high_midi {
            let free = next_free_midi(&rack);
            dup.set_midi_bounds(free, free, Some(free));
        }
        let new_idx = cur + 1;
        let out = self.commit(
            vec![Edit::AddRackZone {
                instrument: id,
                zone: dup,
                index: Some(new_idx),
            }],
            format!("Duplicated zone → {}: {new_name}", new_idx + 1),
        )?;
        self.rack_ui.zone_cursor = new_idx;
        self.rack_ui.panel_slot = 0;
        self.rack_ui.patchbay_cable_id = None;
        if let Some(r) = self.instrument(id).map(|i| i.rack.clone()) {
            self.clamp_rack_state(&r);
        }
        let count = self.control_descriptors().len();
        self.control_cursor = self.control_cursor.min(count.saturating_sub(1));
        Ok(out)
    }

    pub(crate) fn apply_zone_remove(&mut self, instrument: Option<Id>) -> Result<Outcome> {
        let id = self.resolve_rack_instrument_id(instrument)?;
        let rack = self.clone_rack_patch(id)?;
        ensure!(
            rack.zones.len() > 1,
            "Cannot remove the last remaining zone in a Rack"
        );
        let cur = rack.clamp_zone_index(self.rack_ui.zone_cursor);
        let removed_id = rack.zones[cur].id;
        let removed_name = rack.zones[cur].name.clone();
        let next_cursor = cur.min(rack.zones.len().saturating_sub(2));
        let out = self.commit(
            vec![Edit::RemoveRackZone {
                instrument: id,
                zone: removed_id,
            }],
            format!("Removed zone {removed_name}"),
        )?;
        self.rack_ui.zone_cursor = next_cursor;
        if let Some(r) = self.instrument(id).map(|i| i.rack.clone()) {
            self.clamp_rack_state(&r);
        }
        Ok(out)
    }

    pub(crate) fn apply_zone_set_sample(
        &mut self,
        instrument: Option<Id>,
        path: String,
    ) -> Result<Outcome> {
        let id = self.resolve_rack_instrument_id(instrument)?;
        let mut rack = self.clone_rack_patch(id)?;
        ensure!(!rack.zones.is_empty(), "Rack has no zones");
        let cur = rack.clamp_zone_index(self.rack_ui.zone_cursor);
        let trimmed = path.trim().to_string();
        let fallback_midi = rack.zones[cur].root_midi;
        let mod_id = Self::ensure_zone_sampler_module(&mut rack.zones[cur], None)?;
        self.load_and_apply_zone_sample(
            &mut rack.zones[cur],
            Some(mod_id),
            &trimmed,
            fallback_midi,
            true,
            false,
        )?;
        let zone_name = rack.zones[cur].name.clone();
        self.commit(
            vec![Edit::UpdateRackPatch { id, rack }],
            format!("Zone {zone_name} sample → {trimmed}"),
        )
    }

    pub(crate) fn apply_zone_load_preset(
        &mut self,
        instrument: Option<Id>,
        preset_spec: &str,
    ) -> Result<Outcome> {
        let id = self.resolve_rack_instrument_id(instrument)?;
        let preset = match preset_spec.trim().to_ascii_lowercase().as_str() {
            "blank" | "empty" => InstrumentPreset::bare_default(RackTemplate::Sampler),
            _ => self.resolve_instrument_preset(preset_spec)?,
        };
        let preset_name = preset.name.clone();
        ensure!(
            preset.patch.zones.len() == 1,
            "Cannot load multi-zone preset '{preset_name}' into a single zone (press P on the Rack instrument to browse multi-zone presets)"
        );
        let src_zone = preset
            .patch
            .zones
            .into_iter()
            .next()
            .context("Preset has no zones")?;
        let mut rack = self.clone_rack_patch(id)?;
        ensure!(!rack.zones.is_empty(), "Rack has no zones");
        let cur = rack.clamp_zone_index(self.rack_ui.zone_cursor);
        let zone = &mut rack.zones[cur];
        let has_samplers = src_zone
            .modules
            .iter()
            .any(|m| matches!(m.kind, ModuleKind::Sampler(_)));
        zone.voice = src_zone.voice;
        zone.modules = src_zone.modules;
        zone.cables = src_zone.cables;
        if zone.low_midi == zone.high_midi {
            for m in &mut zone.modules {
                if let ModuleKind::Sampler(s) = &mut m.kind {
                    s.root_midi = zone.root_midi;
                }
            }
        }
        let zone_name = zone.name.clone();
        let out = self.commit(
            vec![Edit::UpdateRackPatch { id, rack }],
            format!("Loaded preset '{preset_name}' into zone {zone_name}"),
        )?;
        if has_samplers {
            let missing_count = self
                .assets
                .issues
                .iter()
                .filter(|issue| issue.instrument_id == id)
                .count();
            if missing_count > 0 {
                self.status = format!(
                    "Loaded preset '{preset_name}' into zone {zone_name} ({missing_count} missing sample{} — check Assets)",
                    if missing_count == 1 { "" } else { "s" }
                );
            }
        }
        Ok(out)
    }

    pub(crate) fn apply_zone_import_folder(
        &mut self,
        instrument: Option<Id>,
        folder: String,
    ) -> Result<Outcome> {
        let id = self.resolve_rack_instrument_id(instrument)?;
        let imported = self.import_multisample_folder(&folder)?;
        let zone_count = imported.zones.len();
        let kit_name = imported.kit_name.clone();
        let mut rack = self.clone_rack_patch(id)?;
        rack.kit_name = kit_name.clone();
        rack.zones = imported.zones;
        rack.panel_pins.clear();
        rack.validate()?;
        let out = self.commit(
            vec![Edit::UpdateRackPatch { id, rack }],
            format!("Imported {zone_count} zones from {kit_name}"),
        )?;
        self.rack_ui.zone_cursor = 0;
        if let Some(r) = self.instrument(id).map(|i| i.rack.clone()) {
            self.clamp_rack_state(&r);
        }
        Ok(out)
    }

    pub(crate) fn apply_zone_rename(
        &mut self,
        instrument: Option<Id>,
        name: String,
    ) -> Result<Outcome> {
        let id = self.resolve_rack_instrument_id(instrument)?;
        let mut rack = self.clone_rack_patch(id)?;
        ensure!(!rack.zones.is_empty(), "Rack has no zones");
        let cur = rack.clamp_zone_index(self.rack_ui.zone_cursor);
        rack.zones[cur].set_name(&name)?;
        let next_name = rack.zones[cur].name.clone();
        self.commit(
            vec![Edit::UpdateRackPatch { id, rack }],
            format!("Renamed zone {} → {next_name}", cur + 1),
        )
    }

    pub(crate) fn apply_zone_set_range(
        &mut self,
        instrument: Option<Id>,
        low_midi: u8,
        high_midi: u8,
        root_midi: Option<u8>,
    ) -> Result<Outcome> {
        ensure!(
            low_midi <= high_midi && high_midi <= 127 && root_midi.is_none_or(|r| r <= 127),
            "Zone MIDI range must satisfy 0 <= low <= high <= 127"
        );
        let id = self.resolve_rack_instrument_id(instrument)?;
        let mut rack = self.clone_rack_patch(id)?;
        ensure!(!rack.zones.is_empty(), "Rack has no zones");
        let cur = rack.clamp_zone_index(self.rack_ui.zone_cursor);
        rack.zones[cur].set_midi_bounds(low_midi, high_midi, root_midi);
        let zone = &rack.zones[cur];
        let label = format!(
            "Zone {} range → {}..{} (root {})",
            zone.name,
            note_name(zone.low_midi),
            note_name(zone.high_midi),
            note_name(zone.root_midi)
        );
        self.commit(vec![Edit::UpdateRackPatch { id, rack }], label)
    }

    pub(crate) fn apply_zone_set_velocity(
        &mut self,
        instrument: Option<Id>,
        low_vel: f32,
        high_vel: f32,
    ) -> Result<Outcome> {
        ensure!(
            low_vel.is_finite()
                && high_vel.is_finite()
                && (0.0..=1.0).contains(&low_vel)
                && (0.0..=1.0).contains(&high_vel)
                && low_vel <= high_vel,
            "Zone velocity range must satisfy 0 <= low <= high <= 1"
        );
        let id = self.resolve_rack_instrument_id(instrument)?;
        let mut rack = self.clone_rack_patch(id)?;
        ensure!(!rack.zones.is_empty(), "Rack has no zones");
        let cur = rack.clamp_zone_index(self.rack_ui.zone_cursor);
        rack.zones[cur].set_vel_bounds(low_vel, high_vel);
        let zone = &rack.zones[cur];
        let label = format!(
            "Zone {} vel → {:.0}%..{:.0}%",
            zone.name,
            zone.low_vel * 100.0,
            zone.high_vel * 100.0
        );
        self.commit(vec![Edit::UpdateRackPatch { id, rack }], label)
    }

    pub(crate) fn apply_zone_set_choke(
        &mut self,
        instrument: Option<Id>,
        choke_group: Option<u8>,
    ) -> Result<Outcome> {
        let id = self.resolve_rack_instrument_id(instrument)?;
        let mut rack = self.clone_rack_patch(id)?;
        ensure!(!rack.zones.is_empty(), "Rack has no zones");
        let cur = rack.clamp_zone_index(self.rack_ui.zone_cursor);
        rack.zones[cur].set_choke_group(choke_group)?;
        let zone_name = rack.zones[cur].name.clone();
        let label = format!(
            "Zone {zone_name} choke → {}",
            match choke_group {
                None => "None".into(),
                Some(g) => format!("Group {g}"),
            }
        );
        self.commit(vec![Edit::UpdateRackPatch { id, rack }], label)
    }
}
