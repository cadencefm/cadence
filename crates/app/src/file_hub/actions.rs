//! File Hub Open/Save/Import/Export execution and native OS Finder dialog routing.
use super::{
    browse::{classify_json, is_take_jsonl_path},
    slugify, ExportMode, ExportScopeKind, FileDialogRequest, FileEntryKind, FileHubState,
    FileHubTab,
};
use crate::*;
use cadence_core::bundle::BundleReport;
use cadence_core::midi_map::{MidiProfile, MAX_MIDI_PROFILE_BYTES};
use std::collections::HashMap;

impl App {
    /// Save a self-contained portable project bundle (`project.json` + copied `samples/`).
    pub fn save_portable_bundle_to(&mut self, target_dir: &Path) -> Result<BundleReport> {
        let resolved: HashMap<u64, PathBuf> = self
            .assets
            .samples
            .iter()
            .map(|(&id, data)| (id, data.resolved_path.clone()))
            .collect();
        let report = self.io.save_portable_bundle(
            self.doc.project(),
            self.doc.flags(),
            self.doc.asset_base(),
            &self.assets.library,
            &resolved,
            target_dir,
        )?;
        self.record_recent_project(&report.project_path);
        self.maybe_persist_config();
        let missing_note = if report.missing_samples.is_empty() {
            String::new()
        } else {
            format!(" ({} missing)", report.missing_samples.len())
        };
        self.status = format!(
            "Bundled → {} ({} sample{}{})",
            report.project_path.display(),
            report.samples_copied,
            if report.samples_copied == 1 { "" } else { "s" },
            missing_note
        );
        Ok(report)
    }

    /// Build a rich `ExportRequest` from custom parameters.
    pub fn build_export_request(
        &self,
        path: PathBuf,
        mode: ExportMode,
        scope: ExportScopeKind,
        custom_bars: u32,
        sample_rate: u32,
    ) -> Result<ExportRequest> {
        self.ensure_samples_ready()?;
        let project_bars = self.doc.project().bars as u32;
        let (start_step, steps, bars, use_take) = match scope {
            ExportScopeKind::FullSong => (0, u64::from(project_bars) * 16, project_bars, false),
            ExportScopeKind::LoopRegion => {
                if let Some((start, end)) = self.playing_loop() {
                    let len = u64::from(end.saturating_sub(start)).max(16);
                    (u64::from(start), len, len.div_ceil(16) as u32, false)
                } else {
                    (0, u64::from(project_bars) * 16, project_bars, false)
                }
            }
            ExportScopeKind::CustomBars => {
                let b = custom_bars.clamp(1, 1024);
                (0, u64::from(b) * 16, b, false)
            }
            ExportScopeKind::ActiveTake => {
                let take = self
                    .current_take()
                    .context("No recorded take yet · play a performance first")?;
                let len = take.steps().max(16);
                (0, len, len.div_ceil(16) as u32, true)
            }
        };
        Ok(ExportRequest {
            path,
            bars,
            start_step,
            steps,
            sample_rate,
            mode,
            use_take,
        })
    }

    /// Guard destructive project load / `.rbs` import behind unsaved-changes confirmation.
    /// Leaves FileHub open and intact if loading fails.
    pub(super) fn load_or_confirm_project(
        &mut self,
        path: PathBuf,
        is_rbs: bool,
    ) -> Result<Outcome> {
        if self.dirty() {
            self.modal = ActiveModal::None;
            self.input.mode = Mode::Confirm(if is_rbs {
                ConfirmAction::ImportRbs(path)
            } else {
                ConfirmAction::Open(path)
            });
            return Ok(Outcome::default());
        }
        let out = if is_rbs {
            self.apply(Action::ImportRbs { path })?
        } else {
            self.apply(Action::Load { path })?
        };
        self.input.mode = Mode::Normal;
        self.modal = ActiveModal::None;
        Ok(out)
    }

    /// Save the active MIDI profile (including user-learned bindings) to `path` and persist its path in `UserConfig`.
    pub(crate) fn save_active_midi_profile(&mut self, path: &Path) -> Result<()> {
        let mut prof = self.active_midi_profile();
        prof.mappings = self
            .effective_midi_mappings()
            .into_iter()
            .map(|(m, _)| m)
            .collect();
        let bytes = prof.to_json_bytes()?;
        self.io.write_atomic(path, MAX_MIDI_PROFILE_BYTES, &bytes)?;
        self.midi.custom_profile = Some(prof);
        self.config.midi_profile_path = Some(path.to_path_buf());
        self.settings_ui.config_dirty = true;
        self.maybe_persist_config();
        self.status = format!("Saved MIDI profile → {}", path.display());
        Ok(())
    }

    /// Load a custom MIDI controller profile JSON file via `self.io` (shared by FileHub and `:midi profile-load`),
    /// persisting `config.midi_profile_path` cleanly for future sessions.
    pub(crate) fn load_midi_profile_from_path(&mut self, path: &Path) -> Result<Outcome> {
        let bytes = self
            .io
            .read_bounded(path, MAX_MIDI_PROFILE_BYTES)
            .with_context(|| format!("Reading MIDI profile {}", path.display()))?;
        let prof = MidiProfile::from_json_bytes(&bytes)?;
        self.status = format!(
            "Loaded MIDI profile '{}' ({} bindings)",
            prof.name,
            prof.mappings.len()
        );
        self.midi.custom_profile = Some(prof);
        self.config.midi_profile_path = Some(path.to_path_buf());
        self.settings_ui.config_dirty = true;
        self.maybe_persist_config();
        self.reset_midi_pickup();
        Ok(Outcome {
            midi_feedback: self.force_midi_feedback_sync(),
            ..Outcome::default()
        })
    }

    /// Load a custom MIDI controller profile through FileHub (Open tab, Import tab, or
    /// native Finder dialog), closing FileHub on success while preserving FileHub state on error.
    pub(super) fn load_file_hub_midi_profile(&mut self, path: &Path) -> Result<Outcome> {
        let out = self.load_midi_profile_from_path(path)?;
        self.modal = ActiveModal::None;
        self.input.mode = Mode::Normal;
        Ok(out)
    }

    /// Load a Filter Groove Bank JSON file through FileHub (Browse tab, Import tab, or
    /// native Finder dialog), closing FileHub on success while preserving FileHub state on error.
    pub(super) fn load_file_hub_groove_bank(&mut self, path: &Path) -> Result<Outcome> {
        let out = self.load_filter_groove_bank_from_path(path)?;
        self.modal = ActiveModal::None;
        self.input.mode = Mode::Normal;
        Ok(out)
    }

    /// Route an arbitrary `.json` file opened from the Open tab or Finder dialog:
    /// detects `InstrumentPreset` and `MidiProfile` JSON cleanly instead of sending them
    /// to the Cadence project loader, and surfaces parse errors directly for explicit
    /// `.preset.json` / `.midi.json` files without triggering `ConfirmAction::Open`.
    pub(super) fn open_file_hub_json(&mut self, path: PathBuf) -> Result<Outcome> {
        self.import_file_hub_asset(&path, true)
    }

    fn apply_export_dialog_result(&mut self, mode: ExportMode, path: PathBuf) -> Result<Outcome> {
        let (scope, custom_bars, sample_rate) = self.modal.file_hub().map_or(
            (
                ExportScopeKind::FullSong,
                self.doc.project().bars as u32,
                48_000,
            ),
            |h| (h.export_scope, h.export_custom_bars, h.export_sample_rate),
        );
        if let Some(hub) = self.modal.file_hub_mut() {
            hub.export_mode = mode;
            hub.export_dest = path.display().to_string();
        }
        let export = self.build_export_request(path, mode, scope, custom_bars, sample_rate)?;
        Ok(Outcome {
            export: Some(export),
            ..Outcome::default()
        })
    }

    /// Handle a path selected by the user in a native OS Finder / file dialog (`rfd`).
    pub fn apply_file_dialog_result(
        &mut self,
        req: FileDialogRequest,
        path: PathBuf,
    ) -> Result<Outcome> {
        match req {
            FileDialogRequest::OpenProject => {
                if path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|n| n.to_ascii_lowercase().ends_with(".take.jsonl"))
                {
                    let out = self.apply(Action::LoadTake { path })?;
                    self.input.mode = Mode::Normal;
                    self.modal = ActiveModal::None;
                    return Ok(out);
                }
                if cadence_formats::find_import_format_for_path(&path).is_some() {
                    self.load_or_confirm_project(path, true)
                } else {
                    self.open_file_hub_json(path)
                }
            }
            FileDialogRequest::SaveProjectAs { .. } => {
                if let Some(hub) = self.modal.file_hub_mut() {
                    hub.save_as_path = path.display().to_string();
                }
                let out = self.apply(Action::Save { path })?;
                self.input.mode = Mode::Normal;
                self.modal = ActiveModal::None;
                Ok(out)
            }
            FileDialogRequest::SaveBundleDir => {
                if let Some(hub) = self.modal.file_hub_mut() {
                    hub.bundle_dir = path.display().to_string();
                }
                self.save_portable_bundle_to(&path)?;
                Ok(Outcome::default())
            }
            FileDialogRequest::SaveInstrumentPreset { .. } => {
                if let Some(hub) = self.modal.file_hub_mut() {
                    hub.preset_path = path.display().to_string();
                }
                self.save_active_instrument_preset(&path)?;
                Ok(Outcome::default())
            }
            FileDialogRequest::SaveMidiProfile { .. } => {
                if let Some(hub) = self.modal.file_hub_mut() {
                    hub.midi_profile_path = path.display().to_string();
                }
                self.save_active_midi_profile(&path)?;
                Ok(Outcome::default())
            }
            FileDialogRequest::SaveTake { .. } => {
                if let Some(hub) = self.modal.file_hub_mut() {
                    hub.take_path = path.display().to_string();
                }
                self.apply(Action::SaveTake { path: Some(path) })
            }
            FileDialogRequest::ImportAsset => self.import_file_hub_asset(&path, false),
            FileDialogRequest::ExportWav { .. } => {
                self.apply_export_dialog_result(ExportMode::MasterWav, path)
            }
            FileDialogRequest::ExportStemsDir => {
                self.apply_export_dialog_result(ExportMode::MasterAndStems, path)
            }
            FileDialogRequest::PickSampleDir => {
                self.add_config_dir(SettingsDirKind::SampleDirs, path.display().to_string())?;
                Ok(Outcome {
                    samples_changed: true,
                    ..Outcome::default()
                })
            }
            FileDialogRequest::PickPresetDir => {
                self.add_config_dir(SettingsDirKind::PresetDirs, path.display().to_string())?;
                Ok(Outcome::default())
            }
            FileDialogRequest::PickWorkflowSampleFolder => self.apply_workflow_sample_folder(path),
        }
    }

    /// Load a validated [`InstrumentPreset`] through FileHub (Open tab, Import tab, or
    /// native Finder dialog), closing FileHub and leaving focus cleanly on the instrument.
    pub(super) fn load_file_hub_preset(&mut self, preset: InstrumentPreset) -> Result<Outcome> {
        let inst_id = self
            .active_instrument()
            .or(self.selected_instrument)
            .context("Select an instrument to load an InstrumentPreset")?;
        let preset_name = preset.name.clone();
        let inst_name = self
            .instrument(inst_id)
            .context("Instrument missing")?
            .name
            .clone();
        let out = self.apply_loaded_instrument_preset(inst_id, preset)?;
        self.modal = ActiveModal::None;
        self.input.mode = Mode::Normal;
        if self.detail != Detail::Instrument(inst_id) || !self.deck_focused() {
            self.open_instrument(inst_id)?;
        }
        let missing_count = self
            .assets
            .issues
            .iter()
            .filter(|issue| issue.instrument_id == inst_id)
            .count();
        self.status = if missing_count > 0 {
            format!(
                "Loaded preset {preset_name} into {inst_name} ({missing_count} missing sample{} — check Assets)",
                if missing_count == 1 { "" } else { "s" }
            )
        } else {
            format!("Loaded preset {preset_name} into {inst_name}")
        };
        Ok(out)
    }

    pub(super) fn import_file_hub_asset(
        &mut self,
        path: &Path,
        allow_project_fallback: bool,
    ) -> Result<Outcome> {
        if is_take_jsonl_path(path) {
            let out = self.apply(Action::LoadTake {
                path: path.to_path_buf(),
            })?;
            self.input.mode = Mode::Normal;
            self.modal = ActiveModal::None;
            return Ok(out);
        }
        if cadence_formats::find_import_format_for_path(path).is_some() {
            return self.load_or_confirm_project(path.to_path_buf(), true);
        }
        // Explicit `.midi.json`, groove bank JSONs (`filter_grooves.json` / `*.grooves.json`),
        // and `.preset.json` (or files inside `presets/`) must never swallow parse errors and
        // fall through to `load_or_confirm_project` (which would close FileHub and trigger
        // `ConfirmAction::Open` when `dirty()`).
        match classify_json(path) {
            FileEntryKind::MidiProfile => {
                return self.load_file_hub_midi_profile(path);
            }
            FileEntryKind::FilterGrooveBank => {
                return self.load_file_hub_groove_bank(path);
            }
            FileEntryKind::InstrumentPreset => {
                let preset = self.load_preset_from_io(path)?;
                return self.load_file_hub_preset(preset);
            }
            _ => {}
        }
        // Ambiguously named `.json`: try MIDI Profile, then Filter Groove Bank, then InstrumentPreset.
        if let Ok(out) = self.load_file_hub_midi_profile(path) {
            return Ok(out);
        }
        if let Ok(out) = self.load_file_hub_groove_bank(path) {
            return Ok(out);
        }
        match self.load_preset_from_io(path) {
            Ok(preset) => self.load_file_hub_preset(preset),
            Err(preset_err) if !allow_project_fallback => Err(preset_err),
            Err(_) => self.load_or_confirm_project(path.to_path_buf(), false),
        }
    }

    pub(super) fn request_file_hub_dialog(&mut self, hub: &FileHubState) -> Outcome {
        let slug = slugify(&self.doc.project().title);
        let req = match hub.tab {
            FileHubTab::Quick | FileHubTab::Browse => FileDialogRequest::OpenProject,
            FileHubTab::Save => match hub.cursor {
                2 => FileDialogRequest::SaveBundleDir,
                3 => FileDialogRequest::SaveInstrumentPreset {
                    default_name: Path::new(&hub.preset_path)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("instrument.preset.json")
                        .to_string(),
                },
                4 => FileDialogRequest::SaveMidiProfile {
                    default_name: Path::new(&hub.midi_profile_path)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("midi-profile.midi.json")
                        .to_string(),
                },
                5 => FileDialogRequest::SaveTake {
                    default_name: Path::new(&hub.take_path)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("live.take.jsonl")
                        .to_string(),
                },
                _ => FileDialogRequest::SaveProjectAs {
                    default_name: format!("{slug}.json"),
                },
            },
            FileHubTab::Import => FileDialogRequest::ImportAsset,
            FileHubTab::Export => match hub.export_mode {
                ExportMode::MasterWav => FileDialogRequest::ExportWav {
                    default_name: format!("{slug}.wav"),
                },
                ExportMode::MasterAndStems => FileDialogRequest::ExportStemsDir,
            },
        };
        self.status = "Opening native OS file dialog…".into();
        Outcome {
            file_dialog: Some(req),
            ..Outcome::default()
        }
    }

    pub(super) fn handle_file_hub_save_key(
        &mut self,
        hub: &mut FileHubState,
        key: KeyEvent,
    ) -> Result<Outcome> {
        const ROWS: usize = 6;
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => {
                hub.tab = hub.tab.cycle(-1);
                hub.cursor = 0;
                self.refresh_file_hub_entries(hub);
            }
            KeyCode::Char('l') | KeyCode::Right => {
                hub.tab = hub.tab.cycle(1);
                hub.cursor = 0;
                self.refresh_file_hub_entries(hub);
            }
            KeyCode::Char('j') | KeyCode::Down => {
                hub.cursor = (hub.cursor + 1).min(ROWS - 1);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                hub.cursor = hub.cursor.saturating_sub(1);
            }
            KeyCode::Char('e') => {
                hub.editing_field = match hub.cursor {
                    1 => Some(hub.save_as_path.clone()),
                    2 => Some(hub.bundle_dir.clone()),
                    3 => Some(hub.preset_path.clone()),
                    4 => Some(hub.midi_profile_path.clone()),
                    5 => Some(hub.take_path.clone()),
                    _ => None,
                };
            }
            KeyCode::Enter => match hub.cursor {
                0 => {
                    let path = if self.doc.is_untitled() {
                        PathBuf::from(&hub.save_as_path)
                    } else {
                        self.doc.save_path().clone()
                    };
                    let out = self.apply(Action::Save { path })?;
                    self.input.mode = Mode::Normal;
                    return Ok(out);
                }
                1 => {
                    let path = PathBuf::from(&hub.save_as_path);
                    let out = self.apply(Action::Save { path })?;
                    self.input.mode = Mode::Normal;
                    return Ok(out);
                }
                2 => {
                    let dir = PathBuf::from(&hub.bundle_dir);
                    self.save_portable_bundle_to(&dir)?;
                    return Ok(Outcome::default());
                }
                3 => {
                    let path = PathBuf::from(&hub.preset_path);
                    self.save_active_instrument_preset(&path)?;
                }
                4 => {
                    let path = PathBuf::from(&hub.midi_profile_path);
                    self.save_active_midi_profile(&path)?;
                }
                5 => {
                    let path = PathBuf::from(&hub.take_path);
                    return self.apply(Action::SaveTake { path: Some(path) });
                }
                _ => {}
            },
            _ => {}
        }
        Ok(Outcome::default())
    }

    pub(super) fn handle_file_hub_export_key(
        &mut self,
        hub: &mut FileHubState,
        key: KeyEvent,
    ) -> Result<Outcome> {
        const ROWS: usize = 5;
        let adjust_row = |hub: &mut FileHubState, dir: isize, title: &str| match hub.cursor {
            0 => hub.toggle_export_mode(title),
            1 => hub.export_scope = hub.export_scope.cycle(dir),
            2 => hub.cycle_export_sample_rate(dir),
            _ => {}
        };
        if key.modifiers.contains(KeyModifiers::ALT) {
            let dir = match key.code {
                KeyCode::Char('h' | 'H') | KeyCode::Left => Some(-1isize),
                KeyCode::Char('l' | 'L') | KeyCode::Right => Some(1isize),
                _ => None,
            };
            if let Some(dir) = dir {
                let title = self.doc.project().title.clone();
                adjust_row(hub, dir, &title);
            }
            return Ok(Outcome::default());
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                hub.cursor = (hub.cursor + 1).min(ROWS - 1);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                hub.cursor = hub.cursor.saturating_sub(1);
            }
            KeyCode::Char('h') | KeyCode::Left => {
                hub.tab = hub.tab.cycle(-1);
                hub.cursor = 0;
                self.refresh_file_hub_entries(hub);
            }
            KeyCode::Char('l') | KeyCode::Right => {
                hub.tab = hub.tab.cycle(1);
                hub.cursor = 0;
                self.refresh_file_hub_entries(hub);
            }
            KeyCode::Char(',' | '<') => {
                let title = self.doc.project().title.clone();
                adjust_row(hub, -1, &title);
            }
            KeyCode::Char('.' | '>') => {
                let title = self.doc.project().title.clone();
                adjust_row(hub, 1, &title);
            }
            KeyCode::Char('+' | '=') => {
                hub.export_scope = ExportScopeKind::CustomBars;
                hub.export_custom_bars = (hub.export_custom_bars + 4).min(1024);
            }
            KeyCode::Char('-') => {
                hub.export_scope = ExportScopeKind::CustomBars;
                hub.export_custom_bars = hub.export_custom_bars.saturating_sub(4).max(1);
            }
            KeyCode::Char('e') => {
                if hub.cursor == 3 {
                    hub.editing_field = Some(hub.export_dest.clone());
                }
            }
            KeyCode::Enter => match hub.cursor {
                0 => hub.toggle_export_mode(&self.doc.project().title),
                1 => hub.export_scope = hub.export_scope.cycle(1),
                2 => hub.cycle_export_sample_rate(1),
                3 => {
                    hub.editing_field = Some(hub.export_dest.clone());
                }
                4 => {
                    let export = self.build_export_request(
                        PathBuf::from(&hub.export_dest),
                        hub.export_mode,
                        hub.export_scope,
                        hub.export_custom_bars,
                        hub.export_sample_rate,
                    )?;
                    self.status = format!("Rendering → {}…", export.path.display());
                    return Ok(Outcome {
                        export: Some(export),
                        ..Outcome::default()
                    });
                }
                _ => {}
            },
            _ => {}
        }
        Ok(Outcome::default())
    }
}
