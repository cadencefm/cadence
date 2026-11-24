//! Rack instrument creation, renaming, preset loading/saving, and sample/zone materialization.
use super::{load_preset_via_io, resolve_preset_file_path_with};
use crate::*;

impl App {
    pub(super) fn apply_rename_instrument(
        &mut self,
        instrument: Id,
        name: String,
    ) -> Result<Outcome> {
        let trimmed = drafts::form::validate_printable_name("Instrument", &name, 80)?;
        let old_name = self
            .instrument(instrument)
            .context("Instrument missing")?
            .name
            .clone();
        ensure!(
            !self
                .doc
                .project()
                .instruments
                .iter()
                .any(|i| i.id != instrument && i.name.eq_ignore_ascii_case(&trimmed)),
            "An instrument named '{trimmed}' already exists"
        );
        self.commit(
            vec![Edit::UpdateInstrumentMeta {
                id: instrument,
                name: Some(trimmed.clone()),
                channel: None,
            }],
            format!("Renamed instrument '{old_name}' → '{trimmed}'"),
        )
    }

    pub(crate) fn instrument_delete_summary(&self, id: Id) -> Result<(String, String)> {
        let project = self.doc.project();
        let inst = project.instrument(id).context("Instrument missing")?;
        let part_ids: Vec<Id> = project
            .parts
            .iter()
            .filter(|p| p.instrument == id)
            .map(|p| p.id)
            .collect();
        let placement_count: usize = project
            .lanes
            .iter()
            .flat_map(|l| &l.placements)
            .filter(|pl| part_ids.contains(&pl.part))
            .count();
        let duck_count = project
            .ducks
            .iter()
            .filter(|d| part_ids.contains(&d.source_part))
            .count();
        let mod_count = project
            .modulations
            .iter()
            .filter(|m| {
                matches!(
                    m.target,
                    ModulationTarget::Instrument { instrument, .. } if instrument == id
                )
            })
            .count();
        let macro_count: usize = project
            .macros
            .iter()
            .flat_map(|m| &m.bindings)
            .filter(|b| b.target.as_instrument().is_some_and(|(iid, _)| iid == id))
            .count();

        let mut deps = Vec::new();
        if !part_ids.is_empty() {
            deps.push(format!(
                "{} part{}",
                part_ids.len(),
                if part_ids.len() == 1 { "" } else { "s" }
            ));
        }
        if placement_count > 0 {
            deps.push(format!(
                "{placement_count} placement{}",
                if placement_count == 1 { "" } else { "s" }
            ));
        }
        if duck_count > 0 {
            deps.push(format!(
                "{duck_count} duck{}",
                if duck_count == 1 { "" } else { "s" }
            ));
        }
        if mod_count > 0 {
            deps.push(format!(
                "{mod_count} modulation{}",
                if mod_count == 1 { "" } else { "s" }
            ));
        }
        if macro_count > 0 {
            deps.push(format!(
                "{macro_count} macro binding{}",
                if macro_count == 1 { "" } else { "s" }
            ));
        }
        let detail = if deps.is_empty() {
            format!(
                "Delete instrument '{}'?\nNo dependent parts or modulations · undoable with u.",
                inst.name
            )
        } else {
            format!(
                "Delete instrument '{}'?\nAlso removes {} · undoable with u.",
                inst.name,
                deps.join(", ")
            )
        };
        Ok((inst.name.clone(), detail))
    }

    pub(crate) fn request_delete_instrument(&mut self, id: Id) -> Result<Outcome> {
        let (name, detail) = self.instrument_delete_summary(id)?;
        let single_line = detail.replace('\n', " · ");
        self.input.mode = Mode::Confirm(ConfirmAction::DeleteInstrument(id));
        self.status = format!("{single_line} · y/Enter confirm · Esc cancel ({name})");
        Ok(Outcome::default())
    }

    pub(super) fn apply_delete_instrument(&mut self, id: Id) -> Result<Outcome> {
        let project = self.doc.project();
        let inst_name = project
            .instrument(id)
            .context("Instrument missing")?
            .name
            .clone();
        let part_ids: Vec<Id> = project
            .parts
            .iter()
            .filter(|p| p.instrument == id)
            .map(|p| p.id)
            .collect();
        let mut edits = Vec::new();
        for lane in &project.lanes {
            for pl in &lane.placements {
                if part_ids.contains(&pl.part) {
                    edits.push(Edit::RemovePlacement { id: pl.id });
                }
            }
        }
        for duck in &project.ducks {
            if part_ids.contains(&duck.source_part) {
                edits.push(Edit::RemoveDuck { id: duck.id });
            }
        }
        for &part_id in &part_ids {
            edits.push(Edit::RemovePart { id: part_id });
        }
        for m in &project.modulations {
            if matches!(
                m.target,
                ModulationTarget::Instrument { instrument, .. } if instrument == id
            ) {
                edits.push(Edit::RemoveModulation { id: m.id });
            }
        }
        edits.push(Edit::RemoveInstrument { id });

        let label = if part_ids.is_empty() {
            format!("Deleted instrument '{inst_name}' · u restores")
        } else {
            format!(
                "Deleted instrument '{inst_name}' ({} part{} removed) · u restores",
                part_ids.len(),
                if part_ids.len() == 1 { "" } else { "s" }
            )
        };
        let out = self.commit(edits, label)?;
        if self.detail == Detail::Instrument(id)
            || matches!(self.detail, Detail::Part(pid) if part_ids.contains(&pid))
        {
            if let Some(next_id) = self.doc.project().instruments.first().map(|i| i.id) {
                self.selected_instrument = Some(next_id);
                self.detail = Detail::Instrument(next_id);
                self.rack_ui.zone_cursor = 0;
                self.rack_ui.panel_slot = 0;
                self.rack_ui.patchbay_cable_id = None;
                self.control_cursor = 0;
            } else {
                self.detail = Detail::None;
                self.view_state.deck_visible = false;
                self.control_cursor = 0;
            }
        }
        self.normalize_focus();
        Ok(out)
    }

    /// Ensure `zone` has a `Sampler` module (returning `preferred` if it is a `Sampler`, else
    /// the first existing `Sampler`, else adding a new `Sampler` wired `Sampler -> Out.In`).
    pub(crate) fn ensure_zone_sampler_module(
        zone: &mut RackZone,
        preferred: Option<u16>,
    ) -> Result<u16> {
        if let Some(mid) = preferred {
            if zone
                .module(mid)
                .is_some_and(|m| matches!(m.kind, ModuleKind::Sampler(_)))
            {
                return Ok(mid);
            }
        }
        if let Some(existing) = zone
            .modules
            .iter()
            .find(|m| matches!(m.kind, ModuleKind::Sampler(_)))
            .map(|m| m.id)
        {
            return Ok(existing);
        }
        ensure!(
            zone.modules.len() < cadence_core::rack::MAX_MODULES_PER_ZONE,
            "Zone '{}' already has the maximum {} modules",
            zone.name,
            cadence_core::rack::MAX_MODULES_PER_ZONE
        );
        let mod_id = zone.next_module_id();
        let name = crate::rack::next_module_name(zone, ModuleKindTag::Sampler);
        zone.modules.push(RackModule {
            id: mod_id,
            name,
            kind: ModuleKindTag::Sampler.default_kind(),
        });
        if let Some(out_id) = zone.out_module().map(|m| m.id) {
            if zone.cables.len() < cadence_core::rack::MAX_CABLES_PER_ZONE {
                let cid = zone.next_cable_id();
                zone.cables
                    .push(PatchCable::new(cid, mod_id, out_id, InPort::In, 1.0));
            }
        }
        Ok(mod_id)
    }

    /// Single source of truth for loading a sample file and applying full RIFF `smpl` +
    /// 7-byte `inst` metadata onto a `RackZone`'s target (or first / newly-added) `Sampler` module.
    pub(crate) fn load_and_apply_zone_sample(
        &mut self,
        zone: &mut RackZone,
        module_id: Option<u16>,
        path: &str,
        fallback_midi: u8,
        force_reload: bool,
        require_loaded: bool,
    ) -> Result<(u16, Option<Arc<SampleData>>)> {
        let trimmed = path.trim();
        ensure!(
            !trimmed.is_empty() && trimmed.len() <= 1024 && !trimmed.chars().any(char::is_control),
            "Sample path must be 1–1024 printable characters"
        );
        ensure!(
            !Path::new(trimmed).is_absolute(),
            "Sample path must be project-relative or a catalog label, not absolute ({trimmed})"
        );
        let sampler_id = Self::ensure_zone_sampler_module(zone, module_id)?;
        let load_result = if force_reload {
            self.assets.library.reload(trimmed)
        } else {
            self.assets.library.load(trimmed)
        };
        match load_result {
            Ok(sample) => {
                let resolved_fallback = cadence_core::sampler::parse_filename_midi(&sample.name)
                    .or_else(|| cadence_core::sampler::parse_filename_midi(trimmed))
                    .unwrap_or(fallback_midi);
                zone.apply_sample_metadata(
                    Some(sampler_id),
                    trimmed.to_string(),
                    &sample.meta,
                    sample.frames,
                    resolved_fallback,
                );
                zone.validate()?;
                Ok((sampler_id, Some(sample)))
            }
            Err(err) if require_loaded => Err(err),
            Err(_) => {
                if let Some(ModuleKind::Sampler(smp)) =
                    zone.module_mut(sampler_id).map(|m| &mut m.kind)
                {
                    if smp.path.as_deref() != Some(trimmed) {
                        smp.path = Some(trimmed.to_string());
                        smp.sample_start = 0;
                        smp.sample_end = 0;
                        smp.loop_start = 0;
                        smp.loop_end = 0;
                    }
                }
                zone.validate()?;
                Ok((sampler_id, None))
            }
        }
    }

    /// Compute a portable relative (`./...` or `../...`) or catalog sample locator for a file on disk.
    pub(crate) fn portable_sample_locator(&self, file_path: &Path) -> Result<String> {
        let canon = self
            .io
            .canonicalize(file_path)
            .unwrap_or_else(|_| file_path.to_path_buf());
        let asset_base = self.doc.asset_base();
        let canon_base = self
            .io
            .canonicalize(asset_base)
            .unwrap_or_else(|_| asset_base.clone());
        if let Ok(rel) = canon
            .strip_prefix(asset_base)
            .or_else(|_| canon.strip_prefix(&canon_base))
        {
            return Ok(format!("./{}", rel.to_string_lossy().replace('\\', "/")));
        }
        if let Some(label) = self.assets.library.catalog_label_for(&canon) {
            return Ok(label);
        }
        if let Some(rel) = document::relative_path(&canon_base, &canon) {
            return Ok(rel);
        }
        bail!(
            "Sample {} is outside the project directory and sample roots",
            file_path.display()
        )
    }

    /// Scan an audio folder, read RIFF `inst`+`smpl` metadata (and filename pitches),
    /// and construct a validated key-split multi-zone `RackPatch`.
    pub(crate) fn import_multisample_folder(&mut self, folder: &str) -> Result<RackPatch> {
        let files = self.assets.library.list_folder_samples(folder)?;
        let mut entries = Vec::with_capacity(files.len());
        for (stem, path) in files {
            let locator = self.portable_sample_locator(&path)?;
            let sample = self.assets.library.load(&locator).or_else(|_| {
                let loaded = self.io.load_sample_file(&path, Some(locator.clone()))?;
                self.assets
                    .library
                    .cache_prepared(&path, Arc::clone(&loaded));
                Ok::<_, anyhow::Error>(loaded)
            })?;
            entries.push((stem, locator, sample.meta, sample.frames));
        }
        let mut rack = RackPatch::from_multisample_entries(&entries)?;
        let label = Path::new(folder.trim())
            .file_name()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("Kit");
        rack.kit_name = label.chars().take(64).collect();
        rack.validate()?;
        Ok(rack)
    }

    /// Resolve a preset file path either directly or relative to the active project directory.
    pub(crate) fn resolve_preset_file_path(&self, raw: &str) -> Option<PathBuf> {
        resolve_preset_file_path_with(self.io.as_ref(), self.doc.asset_base(), raw)
    }

    pub(crate) fn load_preset_from_io(&self, path: &Path) -> Result<InstrumentPreset> {
        load_preset_via_io(self.io.as_ref(), path)
    }

    /// Resolve a preset specification (bare template token, file path, or `PresetCatalog` entry)
    /// into a validated unified [`InstrumentPreset`].
    pub(crate) fn resolve_instrument_preset(&self, spec: &str) -> Result<InstrumentPreset> {
        let trimmed = spec.trim();
        ensure!(!trimmed.is_empty(), "Preset name or path cannot be empty");

        for &tmpl in RackTemplate::ALL {
            if PresetCatalog::is_bare_default_token(tmpl, trimmed) {
                return Ok(InstrumentPreset::bare_default(tmpl));
            }
        }
        if let Some(tmpl) = RackTemplate::parse(trimmed) {
            return Ok(InstrumentPreset::bare_default(tmpl));
        }

        if let Some(path) = self.resolve_preset_file_path(trimmed) {
            return self.load_preset_from_io(&path);
        }

        if let Some(entry) = self.assets.presets.find(trimmed) {
            return Ok(entry.to_preset());
        }
        bail!(
            "Unknown Rack preset '{}' (available: {}; templates: synth, sampler, kit, modular, or a .json path)",
            trimmed,
            self.assets.presets.names_hint(None)
        )
    }

    /// Apply any unified [`InstrumentPreset`] onto `instrument_id`, syncing loaded samples
    /// and surfacing any missing sample references non-fatally.
    pub(crate) fn apply_loaded_instrument_preset(
        &mut self,
        instrument_id: Id,
        preset: InstrumentPreset,
    ) -> Result<Outcome> {
        let inst_name = self
            .instrument(instrument_id)
            .context("Instrument missing")?
            .name
            .clone();
        preset.patch.validate()?;
        let preset_name = preset.name;
        let has_samplers = preset.patch.has_samplers();
        let zone_count = preset.patch.zones.len();
        let mut out = self.commit(
            vec![Edit::UpdateRackPatch {
                id: instrument_id,
                rack: preset.patch,
            }],
            format!("Loaded Rack preset '{preset_name}' into {inst_name}"),
        )?;
        self.rack_ui.zone_cursor = self.rack_ui.zone_cursor.min(zone_count.saturating_sub(1));
        self.rack_ui.panel_slot = 0;
        self.rack_ui.patchbay_cable_id = None;
        if has_samplers {
            self.sync_loaded_samples();
            out.samples_changed = true;
            let missing_count = self
                .assets
                .issues
                .iter()
                .filter(|issue| issue.instrument_id == instrument_id)
                .count();
            if missing_count > 0 {
                self.status = format!(
                    "Loaded Rack preset '{preset_name}' into {inst_name} ({missing_count} missing sample{} — check Assets)",
                    if missing_count == 1 { "" } else { "s" }
                );
            }
        }
        Ok(out)
    }

    /// Load an external `InstrumentPreset` (by catalog name, bare template, or file path) onto `instrument_id`.
    pub(crate) fn load_instrument_preset(
        &mut self,
        instrument_id: Id,
        spec: &str,
    ) -> Result<Outcome> {
        ensure!(
            self.instrument(instrument_id).is_some(),
            "Instrument missing"
        );
        let preset = self.resolve_instrument_preset(spec)?;
        self.apply_loaded_instrument_preset(instrument_id, preset)
    }

    pub(crate) fn default_preset_category(rack: &RackPatch) -> &'static str {
        if rack.zones.len() > 1 {
            "Kit"
        } else if rack.has_samplers() {
            "Sampler"
        } else if rack
            .zones
            .first()
            .is_some_and(|z| z.cables.len() >= 5 || z.modules.len() >= 6)
        {
            "Modular"
        } else {
            "Synth"
        }
    }

    pub(crate) fn save_instrument_preset_to_path(
        &mut self,
        instrument_id: Id,
        preset_name: &str,
        category: Option<String>,
        path: &Path,
    ) -> Result<()> {
        let clean_name = drafts::form::validate_printable_name("Preset", preset_name, 80)?;
        let inst = self
            .instrument(instrument_id)
            .context("Selected instrument is missing")?;
        let resolved_category = category
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .or_else(|| Some(Self::default_preset_category(&inst.rack).into()));
        let preset = InstrumentPreset {
            version: INSTRUMENT_PRESET_VERSION,
            name: clean_name.clone(),
            category: resolved_category,
            patch: inst.rack.clone(),
        };
        let bytes = preset.to_json_bytes()?;
        self.io
            .write_atomic(path, cadence_core::presets::MAX_PRESET_BYTES, &bytes)
            .with_context(|| format!("Saving preset {}", path.display()))?;
        self.reload_presets();
        self.status = format!("Saved Rack preset '{clean_name}' → {}", path.display());
        Ok(())
    }

    /// Save the active/focused Rack instrument as a standalone unified `InstrumentPreset` JSON file.
    pub(crate) fn save_active_instrument_preset(&mut self, path: &Path) -> Result<()> {
        let inst_id = self
            .active_instrument()
            .or(self.selected_instrument)
            .context("Select an instrument to save an InstrumentPreset")?;
        let name = self
            .instrument(inst_id)
            .context("Selected instrument is missing")?
            .name
            .clone();
        self.save_instrument_preset_to_path(inst_id, &name, None, path)
    }

    /// Single source of truth materializing any `SoundChoice` into a validated `RackPatch`.
    pub(crate) fn materialize_sound_choice(
        &mut self,
        sound: &workflow::SoundChoice,
        require_loaded: bool,
    ) -> Result<RackPatch> {
        let rack = match sound {
            workflow::SoundChoice::Template(tmpl) => RackPatch::from_template(*tmpl),
            workflow::SoundChoice::Preset(spec) => self.resolve_instrument_preset(spec)?.patch,
            workflow::SoundChoice::Sampler { path, .. } => {
                let mut rack = RackPatch::default_sampler();
                if let Some(path) = path.as_deref() {
                    if let Some(z0) = rack.zones.first_mut() {
                        self.load_and_apply_zone_sample(z0, None, path, 60, false, require_loaded)?;
                    }
                }
                rack
            }
            workflow::SoundChoice::Folder { path, label } => {
                let mut imported = self.import_multisample_folder(path)?;
                if !label.trim().is_empty() {
                    imported.kit_name = label.trim().chars().take(64).collect();
                }
                imported
            }
        };
        rack.validate()?;
        Ok(rack)
    }

    pub(crate) fn create_instrument(
        &mut self,
        name: String,
        rack: RackPatch,
        channel: Option<Id>,
    ) -> Result<Outcome> {
        let ids = self.doc.fresh_ids(if channel.is_some() { 1 } else { 2 })?;
        let id = ids[0];
        let channel = channel.unwrap_or(ids.get(1).copied().unwrap_or(0));
        let mut edits = Vec::new();
        if ids.len() == 2 {
            edits.push(Edit::CreateChannel {
                channel: Channel::new(channel, ChannelKind::Audio, name.clone()),
            });
        }
        edits.push(Edit::CreateInstrument {
            instrument: InstrumentInstance {
                id,
                name,
                rack,
                channel,
            },
        });
        let out = self.commit(edits, format!("Created Rack instrument {id}"))?;
        self.open_instrument(id)?;
        Ok(out)
    }
}
