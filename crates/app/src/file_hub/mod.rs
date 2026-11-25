//! Tabbed Project & File Hub (`gf`, `Ctrl+Shift+o`, `Ctrl+Shift+s`, `Ctrl+Shift+e`, `:files`):
//! Quick Open (New/Recent/Examples/Takes), dedicated Browse directory manager,
//! Save & Portable Bundle exporter, `.rbs` & Preset importer, and Master / Per-Channel
//! Stems WAV exporter — 100% pure data (native OS dialogs stay behind `HostBridge` via
//! `FileDialogRequest`).
mod actions;
mod browse;

use crate::*;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FileHubTab {
    #[default]
    Quick,
    Browse,
    Save,
    Import,
    Export,
}

impl FileHubTab {
    pub const ALL: [Self; 5] = [
        Self::Quick,
        Self::Browse,
        Self::Save,
        Self::Import,
        Self::Export,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Quick => "Quick Open",
            Self::Browse => "Browse",
            Self::Save => "Save & Bundle",
            Self::Import => "Import",
            Self::Export => "Export Audio",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Quick => 0,
            Self::Browse => 1,
            Self::Save => 2,
            Self::Import => 3,
            Self::Export => 4,
        }
    }

    pub fn cycle(self, dir: isize) -> Self {
        let idx = self.index() as isize;
        let len = Self::ALL.len() as isize;
        Self::ALL[(idx + dir).rem_euclid(len) as usize]
    }

    /// True for list/filterable tabs (`Quick`, `Browse`, `Import`) as opposed to form tabs (`Save`, `Export`).
    pub fn is_list_tab(self) -> bool {
        matches!(self, Self::Quick | Self::Browse | Self::Import)
    }

    /// True for tabs that browse `hub.dir` directly (`Browse`, `Import`).
    pub fn has_dir_browser(self) -> bool {
        matches!(self, Self::Browse | Self::Import)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExportMode {
    #[default]
    MasterWav,
    MasterAndStems,
}

impl ExportMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::MasterWav => "Master Stereo WAV",
            Self::MasterAndStems => "Master + Per-Channel Stems",
        }
    }

    pub fn toggle(self) -> Self {
        match self {
            Self::MasterWav => Self::MasterAndStems,
            Self::MasterAndStems => Self::MasterWav,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExportScopeKind {
    #[default]
    FullSong,
    LoopRegion,
    CustomBars,
    ActiveTake,
}

impl ExportScopeKind {
    pub const ALL: [Self; 4] = [
        Self::FullSong,
        Self::LoopRegion,
        Self::CustomBars,
        Self::ActiveTake,
    ];

    pub fn cycle(self, dir: isize) -> Self {
        let idx = Self::ALL.iter().position(|&k| k == self).unwrap_or(0) as isize;
        let len = Self::ALL.len() as isize;
        Self::ALL[(idx + dir).rem_euclid(len) as usize]
    }
}

/// Request from `cadence-app` for the GUI host to open a native OS Finder / file dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileDialogRequest {
    OpenProject,
    SaveProjectAs { default_name: String },
    SaveBundleDir,
    SaveInstrumentPreset { default_name: String },
    SaveMidiProfile { default_name: String },
    SaveTake { default_name: String },
    ImportAsset,
    ExportWav { default_name: String },
    ExportStemsDir,
    PickSampleDir,
    PickPresetDir,
    PickWorkflowSampleFolder,
}

/// Post-export telemetry surfaced inside the File Hub's Export Audio tab.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportSummary {
    pub target: String,
    pub files_written: usize,
    pub duration_sec: f64,
    pub peak_left_db: f64,
    pub peak_right_db: f64,
    pub rms_db: f64,
    pub clips: u64,
    pub sample_rate: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileEntryKind {
    NewProject,
    ParentDir,
    Dir,
    Bundle,
    Project,
    Example,
    Take,
    Rbs,
    InstrumentPreset,
    MidiProfile,
}

impl FileEntryKind {
    pub fn badge(self) -> &'static str {
        match self {
            Self::NewProject => "NEW",
            Self::ParentDir | Self::Dir => "DIR",
            Self::Bundle => "BUNDLE",
            Self::Project => "PROJ",
            Self::Example => "DEMO",
            Self::Take => "TAKE",
            Self::Rbs => "RBS",
            Self::InstrumentPreset => "PRESET",
            Self::MidiProfile => "MIDI",
        }
    }
}

#[derive(Clone, Debug)]
pub struct FileHubEntry {
    pub section: &'static str,
    pub kind: FileEntryKind,
    pub label: String,
    pub detail: String,
    pub path: PathBuf,
    pub preset_id: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct FileHubState {
    pub tab: FileHubTab,
    pub dir: PathBuf,
    pub entries: Vec<FileHubEntry>,
    pub cursor: usize,
    pub query: String,
    pub searching: bool,
    pub editing_field: Option<String>,
    pub save_as_path: String,
    pub bundle_dir: String,
    pub preset_path: String,
    pub midi_profile_path: String,
    pub take_path: String,
    pub export_mode: ExportMode,
    pub export_scope: ExportScopeKind,
    pub export_custom_bars: u32,
    pub export_sample_rate: u32,
    pub export_dest: String,
    pub preview_cache: HashMap<PathBuf, String>,
}

impl FileHubState {
    pub(super) fn toggle_export_mode(&mut self, title: &str) {
        self.export_mode = self.export_mode.toggle();
        let slug = slugify(title);
        self.export_dest = match self.export_mode {
            ExportMode::MasterWav => self.dir.join(format!("{slug}.wav")).display().to_string(),
            ExportMode::MasterAndStems => {
                self.dir.join(format!("{slug}-stems")).display().to_string()
            }
        };
    }

    pub(super) fn cycle_export_sample_rate(&mut self, dir: isize) {
        let idx = SAMPLE_RATES
            .iter()
            .position(|&sr| sr == self.export_sample_rate)
            .unwrap_or(1) as isize;
        self.export_sample_rate =
            SAMPLE_RATES[(idx + dir).rem_euclid(SAMPLE_RATES.len() as isize) as usize];
    }
}

pub(super) const SAMPLE_RATES: [u32; 3] = [44_100, 48_000, 96_000];

pub(super) fn slugify(title: &str) -> String {
    crate::slugify(title, "cadence-song", None)
}

impl App {
    /// Open the tabbed GPU File Hub modal directly onto `tab`.
    pub fn open_file_hub(&mut self, tab: FileHubTab) {
        let slug = slugify(&self.doc.project().title);
        let dir = self.doc.asset_base().clone();
        let save_as_path = if self.doc.is_untitled() {
            dir.join(format!("{slug}.json")).display().to_string()
        } else {
            self.doc.save_path().display().to_string()
        };
        let bundle_dir = dir.join(format!("{slug}-bundle")).display().to_string();
        let preset_name = self
            .active_instrument()
            .or(self.selected_instrument)
            .and_then(|id| self.instrument(id))
            .map(|i| slugify(&i.name))
            .unwrap_or_else(|| "instrument".into());
        let preset_path = dir
            .join("presets")
            .join(format!("{preset_name}.preset.json"))
            .display()
            .to_string();
        let midi_profile_path = self
            .config
            .midi_profile_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| {
                dir.join(format!("{}.midi.json", self.active_midi_profile().id))
                    .display()
                    .to_string()
            });
        let take_path = dir
            .join("takes")
            .join(format!("{slug}.take.jsonl"))
            .display()
            .to_string();
        let export_dest = dir.join(format!("{slug}.wav")).display().to_string();
        let export_custom_bars = self.doc.project().bars as u32;

        let mut state = FileHubState {
            tab,
            dir,
            entries: Vec::new(),
            cursor: 0,
            query: String::new(),
            searching: false,
            editing_field: None,
            save_as_path,
            bundle_dir,
            preset_path,
            midi_profile_path,
            take_path,
            export_mode: ExportMode::MasterWav,
            export_scope: ExportScopeKind::FullSong,
            export_custom_bars,
            export_sample_rate: 48_000,
            export_dest,
            preview_cache: HashMap::new(),
        };
        self.refresh_file_hub_entries(&mut state);
        if tab == FileHubTab::Browse {
            state.cursor = browse::first_dir_entry_cursor(&state.entries);
            self.warm_file_hub_preview(&mut state);
        }
        self.modal = ActiveModal::FileHub(Box::new(state));
        self.input.mode = Mode::FileHub;
        self.status = format!(
            "File Hub · {} · H/L or Tab switch · F native Finder · Esc close",
            tab.label()
        );
    }

    /// Switch active File Hub tab cleanly, clearing any search filter and setting
    /// the initial cursor (`Browse` lands on the first child entry rather than `..`).
    pub(super) fn switch_file_hub_tab(&self, hub: &mut FileHubState, tab: FileHubTab) {
        hub.tab = tab;
        hub.query.clear();
        hub.searching = false;
        self.refresh_file_hub_entries(hub);
        hub.cursor = if tab == FileHubTab::Browse {
            browse::first_dir_entry_cursor(&hub.entries)
        } else {
            0
        };
        if tab.is_list_tab() {
            self.warm_file_hub_preview(hub);
        }
    }

    /// Open File Hub on the Quick Open tab focused on `.take.jsonl` live takes when present.
    pub fn open_file_hub_for_take_load(&mut self) {
        self.open_file_hub(FileHubTab::Quick);
        if let Some(mut hub) = self.modal.take_file_hub() {
            if let Some(idx) = hub
                .entries
                .iter()
                .position(|e| e.kind == FileEntryKind::Take)
            {
                hub.cursor = idx;
                self.warm_file_hub_preview(&mut hub);
            }
            self.modal = ActiveModal::FileHub(hub);
        }
        self.status = "File Hub · Load Take · Enter opens · F native Finder · Esc close".into();
    }

    /// Open File Hub on the Save tab pre-positioned on `Save Active Live Take`.
    pub fn open_file_hub_for_take_save(&mut self) {
        self.open_file_hub(FileHubTab::Save);
        if let Some(hub) = self.modal.file_hub_mut() {
            hub.cursor = 5;
        }
        self.status = "File Hub · Save Take As · e edit path · F Finder · Enter write".into();
    }

    /// Open File Hub on the Import tab focused on `.midi.json` custom profiles when present.
    pub fn open_file_hub_for_midi_profile_load(&mut self) {
        self.open_file_hub(FileHubTab::Import);
        if let Some(mut hub) = self.modal.take_file_hub() {
            if let Some(idx) = hub
                .entries
                .iter()
                .position(|e| e.kind == FileEntryKind::MidiProfile)
            {
                hub.cursor = idx;
                self.warm_file_hub_preview(&mut hub);
            }
            self.modal = ActiveModal::FileHub(hub);
        }
        self.status =
            "File Hub · Load MIDI Profile (.midi.json) · Enter loads · F native Finder · Esc close"
                .into();
    }

    /// Open File Hub on the Save tab pre-positioned on `Save Active MIDI Profile`.
    pub fn open_file_hub_for_midi_profile_save(&mut self) {
        self.open_file_hub(FileHubTab::Save);
        if let Some(hub) = self.modal.file_hub_mut() {
            hub.cursor = 4;
        }
        self.status =
            "File Hub · Save Active MIDI Profile · e edit path · F Finder · Enter write".into();
    }

    pub fn record_export_summary(&mut self, summary: ExportSummary) {
        self.status = format!(
            "Exported {} ({} file{}, {:.1}s · peak {:+.1}/{:+.1} dBFS · {} clips)",
            summary.target,
            summary.files_written,
            if summary.files_written == 1 { "" } else { "s" },
            summary.duration_sec,
            summary.peak_left_db,
            summary.peak_right_db,
            summary.clips
        );
        self.last_export_summary = Some(summary);
    }

    pub(crate) fn handle_file_hub(&mut self, key: KeyEvent) -> Result<Outcome> {
        let Some(mut hub) = self.modal.take_file_hub() else {
            self.input.mode = Mode::Normal;
            return Ok(Outcome::default());
        };
        let result = self.handle_file_hub_inner(&mut hub, key);
        // Preserve FileHub state on errors and whenever FileHub remains active.
        if self.input.mode == Mode::FileHub && self.modal.file_hub().is_none() {
            self.modal = ActiveModal::FileHub(hub);
        }
        result
    }

    fn handle_file_hub_inner(&mut self, hub: &mut FileHubState, key: KeyEvent) -> Result<Outcome> {
        // 1. Inline path editor active (Save As / Bundle / Preset / Export Dest).
        if let Some(mut buf) = hub.editing_field.take() {
            match key.code {
                KeyCode::Esc => {
                    return Ok(Outcome::default());
                }
                KeyCode::Enter => {
                    match (hub.tab, hub.cursor) {
                        (FileHubTab::Save, 1) => hub.save_as_path = buf,
                        (FileHubTab::Save, 2) => hub.bundle_dir = buf,
                        (FileHubTab::Save, 3) => hub.preset_path = buf,
                        (FileHubTab::Save, 4) => hub.midi_profile_path = buf,
                        (FileHubTab::Save, 5) => hub.take_path = buf,
                        (FileHubTab::Export, 3) => hub.export_dest = buf,
                        _ => {}
                    }
                    self.status = "Path updated · press Enter again to run".into();
                    return Ok(Outcome::default());
                }
                KeyCode::Backspace => {
                    buf.pop();
                    hub.editing_field = Some(buf);
                    return Ok(Outcome::default());
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    buf.clear();
                    hub.editing_field = Some(buf);
                    return Ok(Outcome::default());
                }
                KeyCode::Char(c) if !c.is_control() && buf.len() < 512 => {
                    buf.push(c);
                    hub.editing_field = Some(buf);
                    return Ok(Outcome::default());
                }
                _ => {
                    hub.editing_field = Some(buf);
                    return Ok(Outcome::default());
                }
            }
        }

        // 2. Live search filter on Quick / Browse / Import tabs.
        if hub.searching {
            match key.code {
                KeyCode::Esc => {
                    hub.searching = false;
                    hub.query.clear();
                    self.refresh_file_hub_entries(hub);
                    hub.cursor = if hub.tab == FileHubTab::Browse {
                        browse::first_dir_entry_cursor(&hub.entries)
                    } else {
                        0
                    };
                    self.warm_file_hub_preview(hub);
                    return Ok(Outcome::default());
                }
                KeyCode::Enter | KeyCode::Tab => {
                    hub.searching = false;
                    return Ok(Outcome::default());
                }
                KeyCode::Down => {
                    if !hub.entries.is_empty() {
                        hub.cursor = (hub.cursor + 1).min(hub.entries.len() - 1);
                        self.warm_file_hub_preview(hub);
                    }
                    return Ok(Outcome::default());
                }
                KeyCode::Up => {
                    hub.cursor = hub.cursor.saturating_sub(1);
                    self.warm_file_hub_preview(hub);
                    return Ok(Outcome::default());
                }
                KeyCode::Backspace => {
                    hub.query.pop();
                    hub.cursor = 0;
                    self.refresh_file_hub_entries(hub);
                    if hub.query.is_empty() && hub.tab == FileHubTab::Browse {
                        hub.cursor = browse::first_dir_entry_cursor(&hub.entries);
                        self.warm_file_hub_preview(hub);
                    }
                    return Ok(Outcome::default());
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    hub.query.clear();
                    hub.cursor = 0;
                    self.refresh_file_hub_entries(hub);
                    if hub.tab == FileHubTab::Browse {
                        hub.cursor = browse::first_dir_entry_cursor(&hub.entries);
                        self.warm_file_hub_preview(hub);
                    }
                    return Ok(Outcome::default());
                }
                KeyCode::Char(c)
                    if !c.is_control()
                        && !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                        && hub.query.len() < 64 =>
                {
                    hub.query.push(c);
                    hub.cursor = 0;
                    self.refresh_file_hub_entries(hub);
                    return Ok(Outcome::default());
                }
                _ => {
                    return Ok(Outcome::default());
                }
            }
        }

        // 3. Global File Hub keys: Esc clears active filter first then closes, H/L / Tab / 1-5 switch tabs,
        //    Space toggles transport, 0/$/G/End/PgUp/PgDn navigate rows, F native OS Finder.
        let row_count = match hub.tab {
            FileHubTab::Quick | FileHubTab::Browse | FileHubTab::Import => hub.entries.len(),
            FileHubTab::Save => 6,
            FileHubTab::Export => 5,
        };
        let max_row = row_count.saturating_sub(1);
        match key.code {
            KeyCode::Esc => {
                if !hub.query.is_empty() {
                    hub.query.clear();
                    self.refresh_file_hub_entries(hub);
                    hub.cursor = if hub.tab == FileHubTab::Browse {
                        browse::first_dir_entry_cursor(&hub.entries)
                    } else {
                        0
                    };
                    self.warm_file_hub_preview(hub);
                    return Ok(Outcome::default());
                }
                self.input.mode = Mode::Normal;
                return Ok(Outcome::default());
            }
            KeyCode::Char(' ') if key.kind != KeyEventKind::Repeat => {
                return self.apply(Action::Play {
                    playing: !self.transport.playing,
                });
            }
            KeyCode::Char('0') => {
                hub.cursor = 0;
                if hub.tab.is_list_tab() {
                    self.warm_file_hub_preview(hub);
                }
                return Ok(Outcome::default());
            }
            KeyCode::Char('$' | 'G') | KeyCode::End => {
                hub.cursor = max_row;
                if hub.tab.is_list_tab() {
                    self.warm_file_hub_preview(hub);
                }
                return Ok(Outcome::default());
            }
            KeyCode::PageUp => {
                hub.cursor = hub.cursor.saturating_sub(8);
                if hub.tab.is_list_tab() {
                    self.warm_file_hub_preview(hub);
                }
                return Ok(Outcome::default());
            }
            KeyCode::PageDown => {
                hub.cursor = (hub.cursor + 8).min(max_row);
                if hub.tab.is_list_tab() {
                    self.warm_file_hub_preview(hub);
                }
                return Ok(Outcome::default());
            }
            KeyCode::Tab | KeyCode::Char('L') => {
                let next = hub.tab.cycle(1);
                self.switch_file_hub_tab(hub, next);
                return Ok(Outcome::default());
            }
            KeyCode::BackTab | KeyCode::Char('H') => {
                let prev = hub.tab.cycle(-1);
                self.switch_file_hub_tab(hub, prev);
                return Ok(Outcome::default());
            }
            KeyCode::Char(c @ '1'..='5') => {
                let next = FileHubTab::ALL[(c as u8 - b'1') as usize];
                self.switch_file_hub_tab(hub, next);
                return Ok(Outcome::default());
            }
            KeyCode::Char('F') => {
                return Ok(self.request_file_hub_dialog(hub));
            }
            _ => {}
        }

        // 4. Per-tab interaction.
        match hub.tab {
            FileHubTab::Quick | FileHubTab::Browse | FileHubTab::Import => {
                self.handle_file_hub_browse_key(hub, key)
            }
            FileHubTab::Save => self.handle_file_hub_save_key(hub, key),
            FileHubTab::Export => self.handle_file_hub_export_key(hub, key),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadence_core::material::Project;

    #[test]
    fn file_hub_tabs_stems_export_and_native_dialog_handoff_work() {
        let dir = tempfile::tempdir().unwrap();
        let proj_path = dir.path().join("demo.json");
        let mut app = App::new(Project::demo(), proj_path.clone()).unwrap();

        // 1. `gf` opens File Hub on Quick Open tab; `F` emits native OpenProject dialog request!
        app.press_keys("gf").unwrap();
        assert_eq!(app.input.mode, Mode::FileHub);
        assert_eq!(app.modal.file_hub().unwrap().tab, FileHubTab::Quick);
        let dlg_out = app
            .handle_key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT))
            .unwrap();
        assert_eq!(dlg_out.file_dialog, Some(FileDialogRequest::OpenProject));

        // 2. Switch to Export Audio tab with `5`, toggle Master+Stems, cycle rate to 96k, render!
        app.press_keys("5").unwrap();
        assert_eq!(app.modal.file_hub().unwrap().tab, FileHubTab::Export);
        app.press_keys("<m-l>").unwrap(); // Toggle Master+Stems on row 0 via Tier 3
        assert_eq!(
            app.modal.file_hub().unwrap().export_mode,
            ExportMode::MasterAndStems
        );
        app.press_keys("jj").unwrap(); // Move to Sample Rate (row 2)
        app.press_keys(".").unwrap(); // 48_000 -> 96_000 via Tier 4
        assert_eq!(app.modal.file_hub().unwrap().export_sample_rate, 96_000);
        app.press_keys("jj").unwrap(); // Move to Render Now (row 4)
        let exp_out = app
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        let req = exp_out.export.expect("expected ExportRequest");
        assert_eq!(req.mode, ExportMode::MasterAndStems);
        assert_eq!(req.sample_rate, 96_000);

        // 3. `:bundle` creates portable bundle directory + records recent project!
        let bundle_dir = dir.path().join("export-bundle");
        app.execute_command(&format!("bundle {}", bundle_dir.display()))
            .unwrap();
        assert!(bundle_dir.join("project.json").is_file());
        assert!(!app.config.recent_projects.is_empty());

        // 4. Native Finder SaveProjectAs callback saves project and closes modal!
        let save_target = dir.path().join("saved-via-finder.json");
        app.apply_file_dialog_result(
            FileDialogRequest::SaveProjectAs {
                default_name: "demo.json".into(),
            },
            save_target.clone(),
        )
        .unwrap();
        assert!(save_target.is_file());
        assert_eq!(app.input.mode, Mode::Normal);
    }

    #[test]
    fn file_hub_save_preset_defaults_to_presets_subdir_creates_dir_and_appears_in_preset_browser() {
        let dir = tempfile::tempdir().unwrap();
        let proj_path = dir.path().join("demo.json");
        let presets_dir = dir.path().join("presets");
        assert!(
            !presets_dir.exists(),
            "presets/ subdir should not pre-exist before saving"
        );

        let mut app = App::new(Project::demo(), proj_path).unwrap();
        let inst = app
            .project()
            .instruments
            .first()
            .expect("demo instrument")
            .clone();
        app.execute_command(&format!("instrument \"{}\"", inst.name))
            .unwrap();

        // `Ctrl+Shift+s` opens File Hub Save tab; `jjj` focuses row 3 (Save Active Instrument Preset).
        app.press_keys("<c-s-s>jjj").unwrap();
        let expected_path = app
            .doc
            .asset_base()
            .join("presets")
            .join(format!("{}.preset.json", slugify(&inst.name)));
        {
            let hub = app.modal.file_hub().expect("FileHub state active");
            assert_eq!(hub.tab, FileHubTab::Save);
            assert_eq!(hub.cursor, 3);
            assert_eq!(PathBuf::from(&hub.preset_path), expected_path);
        }

        // Pressing Enter creates `<asset_base>/presets/` via write_atomic and triggers reload_presets().
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert!(
            presets_dir.is_dir(),
            "save_active_instrument_preset must create missing presets/ parent directory"
        );
        assert!(
            expected_path.is_file(),
            "preset must be written to {}",
            expected_path.display()
        );

        // Close File Hub and open `P` Preset Browser: the newly saved preset is immediately discoverable.
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        app.press_keys("P").unwrap();
        let pb = app
            .preset_browser_view()
            .expect("PresetBrowserView must open on P");
        assert!(
            pb.items.iter().any(|item| item.name == inst.name),
            "newly saved preset '{}' must immediately appear in Preset Browser; got {:?}",
            inst.name,
            pb.items.iter().map(|i| &i.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn corrupt_preset_and_midi_while_dirty_keep_file_hub_open_without_confirm() {
        let dir = tempfile::tempdir().unwrap();
        let proj_path = dir.path().join("demo.json");
        let corrupt_preset = dir.path().join("broken.preset.json");
        let corrupt_midi = dir.path().join("broken.midi.json");
        let corrupt_generic = dir.path().join("broken.json");
        std::fs::write(&corrupt_preset, b"{ not valid preset json").unwrap();
        std::fs::write(&corrupt_midi, b"{ not valid midi json").unwrap();
        std::fs::write(&corrupt_generic, b"{ not valid json").unwrap();

        let mut app = App::new(Project::demo(), proj_path).unwrap();
        // Make document dirty so any accidental fallthrough to `load_or_confirm_project`
        // would close FileHub and open `Mode::Confirm(ConfirmAction::Open(_))`.
        app.execute_command("bpm 133").unwrap();
        assert!(app.dirty(), "project must be dirty before opening FileHub");

        // 1. Selecting corrupt `.preset.json` inside FileHub Browse tab returns real error & stays in FileHub.
        app.open_file_hub(FileHubTab::Browse);
        let preset_idx = app
            .modal
            .file_hub()
            .unwrap()
            .entries
            .iter()
            .position(|e| e.label == "broken.preset.json")
            .expect("broken.preset.json listed in Browse tab");
        app.modal.file_hub_mut().unwrap().cursor = preset_idx;
        let err = app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            err.is_err(),
            "corrupt .preset.json while dirty must return parse error, not ConfirmAction::Open"
        );
        assert_eq!(app.input.mode, Mode::FileHub);
        assert!(
            app.modal.file_hub().is_some(),
            "FileHub must stay open on corrupt .preset.json"
        );
        assert!(app.dirty());

        // 2. Selecting corrupt `.midi.json` inside FileHub Import tab returns real error & stays in FileHub.
        app.open_file_hub(FileHubTab::Import);
        let midi_idx = app
            .modal
            .file_hub()
            .unwrap()
            .entries
            .iter()
            .position(|e| e.label == "broken.midi.json")
            .expect("broken.midi.json listed in Import tab");
        app.modal.file_hub_mut().unwrap().cursor = midi_idx;
        let err = app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            err.is_err(),
            "corrupt .midi.json while dirty must return parse error, not ConfirmAction::Open"
        );
        assert_eq!(app.input.mode, Mode::FileHub);
        assert!(app.modal.file_hub().is_some());

        // 3. Native Finder `ImportAsset` with corrupt `.preset.json` or `.json` while dirty stays in FileHub.
        let err = app.apply_file_dialog_result(FileDialogRequest::ImportAsset, corrupt_preset);
        assert!(err.is_err());
        assert_eq!(app.input.mode, Mode::FileHub);
        assert!(app.modal.file_hub().is_some());

        let err = app.apply_file_dialog_result(FileDialogRequest::ImportAsset, corrupt_generic);
        assert!(err.is_err());
        assert_eq!(app.input.mode, Mode::FileHub);
        assert!(app.modal.file_hub().is_some());
    }

    #[test]
    fn file_hub_midi_profile_auto_closes_on_success_and_stays_open_on_error() {
        let dir = tempfile::tempdir().unwrap();
        let proj_path = dir.path().join("demo.json");
        let valid_midi = dir.path().join("studio.midi.json");
        let ambig_midi = dir.path().join("controller.json");
        let corrupt_midi = dir.path().join("corrupt.midi.json");

        let mut profile = cadence_core::midi_map::MidiProfile::by_id("minilab3_daw");
        profile.name = "Studio Rig".into();
        let profile_bytes = profile.to_json_bytes().unwrap();
        std::fs::write(&valid_midi, &profile_bytes).unwrap();
        std::fs::write(&ambig_midi, &profile_bytes).unwrap();
        std::fs::write(&corrupt_midi, b"{ corrupt midi profile").unwrap();

        let mut app = App::new(Project::demo(), proj_path).unwrap();

        // 1. Valid `.midi.json` via FileHub Browse tab closes FileHub and installs custom profile.
        app.open_file_hub(FileHubTab::Browse);
        let idx = app
            .modal
            .file_hub()
            .unwrap()
            .entries
            .iter()
            .position(|e| e.label == "studio.midi.json")
            .expect("studio.midi.json listed in Browse tab");
        app.modal.file_hub_mut().unwrap().cursor = idx;
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .expect("valid .midi.json must load cleanly");
        assert_eq!(app.input.mode, Mode::Normal);
        assert!(
            app.modal.file_hub().is_none(),
            "FileHub must auto-close after loading a valid .midi.json"
        );
        assert!(app.status.contains("Studio Rig"));
        assert_eq!(
            app.midi.custom_profile.as_ref().map(|p| p.name.as_str()),
            Some("Studio Rig")
        );

        // 2. Valid `.midi.json` via FileHub Import tab also auto-closes FileHub.
        app.open_file_hub(FileHubTab::Import);
        let imp_idx = app
            .modal
            .file_hub()
            .unwrap()
            .entries
            .iter()
            .position(|e| e.label == "studio.midi.json")
            .expect("studio.midi.json listed in Import tab");
        app.modal.file_hub_mut().unwrap().cursor = imp_idx;
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .expect("valid .midi.json via Import tab must load cleanly");
        assert_eq!(app.input.mode, Mode::Normal);
        assert!(app.modal.file_hub().is_none());

        // 3. Ambiguously named `.json` containing a valid MidiProfile via Finder also auto-closes FileHub.
        app.open_file_hub(FileHubTab::Quick);
        app.apply_file_dialog_result(FileDialogRequest::OpenProject, ambig_midi)
            .expect("valid MidiProfile .json via Finder must load cleanly");
        assert_eq!(app.input.mode, Mode::Normal);
        assert!(app.modal.file_hub().is_none());

        // 4. Corrupt `.midi.json` via FileHub Browse stays in FileHub and surfaces error without clobbering state.
        app.open_file_hub(FileHubTab::Browse);
        let bad_idx = app
            .modal
            .file_hub()
            .unwrap()
            .entries
            .iter()
            .position(|e| e.label == "corrupt.midi.json")
            .expect("corrupt.midi.json listed in Browse tab");
        app.modal.file_hub_mut().unwrap().cursor = bad_idx;
        let err = app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(err.is_err(), "corrupt .midi.json must surface error");
        assert_eq!(
            app.input.mode,
            Mode::FileHub,
            "FileHub mode must remain active on corrupt .midi.json"
        );
        assert!(
            app.modal.file_hub().is_some(),
            "FileHub overlay must stay open on corrupt .midi.json"
        );
        assert_eq!(
            app.midi.custom_profile.as_ref().map(|p| p.name.as_str()),
            Some("Studio Rig"),
            "failed load must preserve previously loaded MIDI profile"
        );
    }

    #[test]
    fn file_hub_five_tabs_directory_cursor_memory_recent_cap_and_quick_jumps() {
        let io = Arc::new(MemoryHostIo::new("/virtual/studio"));
        io.set_home_dir("/virtual/home");
        io.insert_file("/virtual/home/home-sketch.json", b"{}");
        io.create_dir("/virtual/studio/alpha");
        io.create_dir("/virtual/studio/beta/deep");
        io.insert_file("/virtual/studio/beta/deep/nested.json", b"{}");
        io.create_dir("/virtual/studio/empty");
        io.insert_file("/virtual/studio/gamma.json", b"{}");

        // Populate 11 existing recent projects to verify Quick caps unfiltered RECENT at 8
        // while `/` search matches across all recents + current folder.
        let mut recent_paths = Vec::new();
        for idx in 0..11 {
            let p = PathBuf::from(format!("/virtual/recents/song-{idx:02}.json"));
            io.insert_file(&p, b"{}");
            recent_paths.push(p);
        }

        let mut app = App::new_with_host_io(
            Project::demo(),
            PathBuf::from("/virtual/studio/session.json"),
            Arc::clone(&io) as Arc<dyn HostIo>,
        )
        .unwrap();
        app.config.recent_projects = recent_paths;

        // 1. `gf` opens Quick Open: zero directory clutter, capped at 8 RECENT entries.
        app.press_keys("gf").unwrap();
        {
            let hub = app.modal.file_hub().unwrap();
            assert_eq!(hub.tab, FileHubTab::Quick);
            assert_eq!(hub.entries[0].kind, FileEntryKind::NewProject);
            assert!(
                hub.entries.iter().all(|e| e.section != "FILES"),
                "Quick Open must not stack FILES below Recent/Examples"
            );
            let recent_count = hub.entries.iter().filter(|e| e.section == "RECENT").count();
            assert_eq!(
                recent_count, 8,
                "Unfiltered Quick Open caps RECENT at 8 entries"
            );
        }

        // 2. `/` search on Quick matches older recents (>8) AND current-folder files/dirs!
        app.press_keys("/song-10<Enter>").unwrap();
        assert!(app
            .modal
            .file_hub()
            .unwrap()
            .entries
            .iter()
            .any(|e| e.section == "RECENT" && e.label == "song-10.json"));
        app.press_keys("<Esc>").unwrap(); // clears filter, stays in FileHub
        assert_eq!(app.input.mode, Mode::FileHub);
        app.press_keys("/gamma<Enter>").unwrap();
        assert!(app
            .modal
            .file_hub()
            .unwrap()
            .entries
            .iter()
            .any(|e| e.section == "CURRENT FOLDER" && e.label == "gamma.json"));
        app.press_keys("<Esc>").unwrap();

        // 3. `2` switches to dedicated Browse tab; cursor lands on first child (`alpha/`), NOT `..`.
        app.press_keys("2").unwrap();
        {
            let hub = app.modal.file_hub().unwrap();
            assert_eq!(hub.tab, FileHubTab::Browse);
            assert_eq!(hub.entries[0].kind, FileEntryKind::ParentDir);
            assert_eq!(hub.cursor, 1, "Browse must land on first real child entry");
            assert_eq!(hub.entries[hub.cursor].label, "alpha/");
        }

        // 4. Move to `beta/`, Enter -> lands on `deep/` (index 1); Enter -> lands on `nested.json` (index 1).
        app.press_keys("j<Enter>").unwrap();
        {
            let hub = app.modal.file_hub().unwrap();
            assert_eq!(hub.dir, PathBuf::from("/virtual/studio/beta"));
            assert_eq!(hub.cursor, 1);
            assert_eq!(hub.entries[hub.cursor].label, "deep/");
        }
        app.press_keys("<Enter>").unwrap();
        {
            let hub = app.modal.file_hub().unwrap();
            assert_eq!(hub.dir, PathBuf::from("/virtual/studio/beta/deep"));
            assert_eq!(hub.cursor, 1);
            assert_eq!(hub.entries[hub.cursor].label, "nested.json");
        }

        // 5. Backspace restores focus onto `deep/`; Enter on `..` restores focus onto `beta/`!
        app.press_keys("<Backspace>").unwrap();
        {
            let hub = app.modal.file_hub().unwrap();
            assert_eq!(hub.dir, PathBuf::from("/virtual/studio/beta"));
            assert_eq!(
                hub.entries[hub.cursor].label, "deep/",
                "Backspace must restore cursor onto exited folder"
            );
        }
        app.press_keys("0<Enter>").unwrap(); // index 0 = `.. (Parent Directory)`
        {
            let hub = app.modal.file_hub().unwrap();
            assert_eq!(hub.dir, PathBuf::from("/virtual/studio"));
            assert_eq!(
                hub.entries[hub.cursor].label, "beta/",
                "Enter on .. must restore cursor onto exited folder"
            );
        }

        // 6. Entering an empty folder lands cleanly on `..` (index 0); Backspace restores onto `empty/`.
        app.press_keys("j<Enter>").unwrap(); // `empty/` is right after `beta/`
        {
            let hub = app.modal.file_hub().unwrap();
            assert_eq!(hub.dir, PathBuf::from("/virtual/studio/empty"));
            assert_eq!(hub.cursor, 0);
            assert_eq!(hub.entries[0].kind, FileEntryKind::ParentDir);
        }
        app.press_keys("<Backspace>").unwrap();
        assert_eq!(
            app.modal.file_hub().unwrap().entries[app.modal.file_hub().unwrap().cursor].label,
            "empty/"
        );

        // 7. `~` jumps to Home directory and `.` jumps back to project directory.
        app.press_keys("~").unwrap();
        {
            let hub = app.modal.file_hub().unwrap();
            assert_eq!(hub.dir, PathBuf::from("/virtual/home"));
            assert_eq!(hub.entries[hub.cursor].label, "home-sketch.json");
        }
        app.press_keys(".").unwrap();
        {
            let hub = app.modal.file_hub().unwrap();
            assert_eq!(hub.dir, PathBuf::from("/virtual/studio"));
            assert_eq!(hub.entries[hub.cursor].label, "alpha/");
        }

        // 8. Numeric tab shortcuts `1`..=`5` switch cleanly across all 5 tabs.
        app.press_keys("3").unwrap();
        assert_eq!(app.modal.file_hub().unwrap().tab, FileHubTab::Save);
        app.press_keys("4").unwrap();
        assert_eq!(app.modal.file_hub().unwrap().tab, FileHubTab::Import);
        app.press_keys("5").unwrap();
        assert_eq!(app.modal.file_hub().unwrap().tab, FileHubTab::Export);
        app.press_keys("1").unwrap();
        assert_eq!(app.modal.file_hub().unwrap().tab, FileHubTab::Quick);
    }
}
