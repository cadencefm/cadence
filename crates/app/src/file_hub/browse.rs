//! File Hub directory listing, bounded header peek/preview, row builders, and Open/Import navigation.
use super::{FileEntryKind, FileHubEntry, FileHubState, FileHubTab};
use crate::*;

pub(super) fn is_take_jsonl_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|n| n.to_ascii_lowercase().ends_with(".take.jsonl"))
}

fn is_portable_bundle_dir(io: &dyn HostIo, dir: &Path) -> bool {
    io.is_file(&dir.join("project.json")) && io.is_dir(&dir.join("samples"))
}

pub(super) fn classify_json(path: &Path) -> FileEntryKind {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if name.ends_with(".midi.json") || name.contains("midi_profile") {
        return FileEntryKind::MidiProfile;
    }
    if name.ends_with(".preset.json")
        || (name != "filter_grooves.json"
            && path.ancestors().any(|a| {
                a.file_name()
                    .is_some_and(|n| n.eq_ignore_ascii_case("presets"))
            }))
    {
        return FileEntryKind::InstrumentPreset;
    }
    FileEntryKind::Project
}

pub(super) fn peek_file_preview(io: &dyn HostIo, path: &Path, kind: FileEntryKind) -> String {
    match kind {
        FileEntryKind::NewProject => {
            "Start a blank Cadence project using your Settings defaults (prompts if unsaved)".into()
        }
        FileEntryKind::ParentDir => "Go to parent directory".into(),
        FileEntryKind::Dir => format!("Folder · {}", path.display()),
        FileEntryKind::Bundle => {
            let proj_preview =
                peek_file_preview(io, &path.join("project.json"), FileEntryKind::Project);
            let sample_count = io
                .read_dir(&path.join("samples"))
                .map(|entries| entries.into_iter().filter(|e| e.is_file).count())
                .unwrap_or(0);
            format!(
                "Portable Bundle ({sample_count} sample{}) · {proj_preview}",
                if sample_count == 1 { "" } else { "s" }
            )
        }
        FileEntryKind::Take => {
            match io
                .read_bounded(path, cadence_core::take::MAX_TAKE_BYTES)
                .and_then(|bytes| cadence_core::take::Take::from_jsonl_bytes(&bytes, true))
            {
                Ok(take) => format!(
                    "Live Take · \"{}\" · {} bars · {} change{}",
                    take.header.project.title,
                    take.steps().div_ceil(16).max(1),
                    take.events.len(),
                    if take.events.len() == 1 { "" } else { "s" }
                ),
                Err(err) => format!("Live Take ({err})"),
            }
        }
        FileEntryKind::Rbs => {
            let fallback = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("ReBirth Song");
            let Some(max_bytes) = cadence_formats::max_import_bytes_for_path(path) else {
                return "ReBirth .rbs (disabled in this build)".into();
            };
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("rbs");
            let parsed = io
                .read_bounded(path, max_bytes)
                .and_then(|bytes| cadence_formats::peek_import_header_bytes(ext, &bytes, fallback));
            match parsed {
                Ok(line) => line,
                Err(err) => format!("ReBirth .rbs ({err})"),
            }
        }
        FileEntryKind::Project
        | FileEntryKind::Example
        | FileEntryKind::InstrumentPreset
        | FileEntryKind::MidiProfile => {
            const MAX_PREVIEW_BYTES: u64 = 8 * 1024 * 1024;
            if let Ok(len) = io.file_len(path) {
                if len > MAX_PREVIEW_BYTES {
                    return format!("Large JSON file ({:.1} MB)", len as f64 / 1_048_576.0);
                }
            }
            let Ok(bytes) = io.read_bounded(path, MAX_PREVIEW_BYTES) else {
                return format!("{}", path.display());
            };
            let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                return "Unparseable JSON file".into();
            };
            if let Some(proj) = val
                .get("project")
                .or_else(|| val.get("title").map(|_| &val))
            {
                if let (Some(title), Some(bpm), Some(bars)) = (
                    proj.get("title").and_then(|v| v.as_str()),
                    proj.get("bpm").and_then(|v| v.as_f64()),
                    proj.get("bars").and_then(|v| v.as_u64()),
                ) {
                    let insts = proj
                        .get("instruments")
                        .and_then(|v| v.as_array())
                        .map_or(0, Vec::len);
                    let pats = proj
                        .get("patterns")
                        .and_then(|v| v.as_array())
                        .map_or(0, Vec::len);
                    return format!(
                        "Cadence Project · \"{title}\" · {bars} bars @ {bpm:.1} BPM · {insts} inst · {pats} pat"
                    );
                }
            }
            if val.get("mappings").is_some() && val.get("feedback").is_some() {
                let name = val
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Profile");
                let n = val
                    .get("mappings")
                    .and_then(|v| v.as_array())
                    .map_or(0, Vec::len);
                return format!("MIDI Profile · \"{name}\" · {n} mappings");
            }
            if let Some(patch) = val.get("patch") {
                if let Some(name) = val.get("name").and_then(|v| v.as_str()) {
                    let cat = val
                        .get("category")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Rack");
                    let zones = patch
                        .get("zones")
                        .and_then(|v| v.as_array())
                        .map_or(0, Vec::len);
                    return if zones > 0 {
                        format!("{cat} Preset · \"{name}\" · {zones}z")
                    } else {
                        format!("{cat} Preset · \"{name}\"")
                    };
                }
            }
            format!("JSON file · {}", path.display())
        }
    }
}

fn discover_bundled_examples(io: &dyn HostIo, roots: &[PathBuf]) -> Vec<FileHubEntry> {
    let mut found = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for root in roots {
        let ex_dir = root.join("examples");
        let Ok(mut entries) = io.read_dir(&ex_dir) else {
            continue;
        };
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        for entry in entries {
            let candidate = if entry.is_dir {
                entry.path.join("project.json")
            } else if entry
                .path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("json"))
            {
                entry.path.clone()
            } else {
                continue;
            };
            if !io.is_file(&candidate) {
                continue;
            }
            let canon = io
                .canonicalize(&candidate)
                .unwrap_or_else(|_| candidate.clone());
            if !seen.insert(canon) {
                continue;
            }
            let folder_name = candidate
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("example");
            let label = match folder_name {
                "demo" => "Afterglow (Flagship EDM Demo)".into(),
                "glass-cathedral" => "Glass Cathedral (Ambient Poly)".into(),
                other => other.replace('-', " "),
            };
            found.push(FileHubEntry {
                section: "BUNDLED EXAMPLES",
                kind: FileEntryKind::Example,
                label,
                detail: candidate
                    .strip_prefix(root)
                    .unwrap_or(&candidate)
                    .display()
                    .to_string(),
                path: candidate,
                preset_id: None,
            });
        }
    }
    found
}

impl App {
    pub(super) fn refresh_file_hub_entries(&self, hub: &mut FileHubState) {
        let io = self.io.as_ref();
        let mut entries = Vec::new();
        let query = hub.query.trim().to_ascii_lowercase();
        let matches = |label: &str, detail: &str| {
            query.is_empty()
                || label.to_ascii_lowercase().contains(&query)
                || detail.to_ascii_lowercase().contains(&query)
        };

        if hub.tab == FileHubTab::Open {
            if matches(
                "New Blank Project",
                "Start a blank project with Settings defaults",
            ) {
                entries.push(FileHubEntry {
                    section: "PROJECT",
                    kind: FileEntryKind::NewProject,
                    label: "New Blank Project".into(),
                    detail: "Start a blank project with Settings defaults".into(),
                    path: PathBuf::from("<new-blank-project>"),
                    preset_id: None,
                });
            }
            for recent in &self.config.recent_projects {
                if !io.exists(recent) {
                    continue;
                }
                let kind = if cadence_formats::find_import_format_for_path(recent).is_some() {
                    FileEntryKind::Rbs
                } else {
                    FileEntryKind::Project
                };
                let label = recent
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("project")
                    .to_string();
                let detail = recent
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                if matches(&label, &detail) {
                    entries.push(FileHubEntry {
                        section: "RECENT",
                        kind,
                        label,
                        detail,
                        path: recent.clone(),
                        preset_id: None,
                    });
                }
            }

            let mut roots = Vec::new();
            for base in [Some(self.doc.asset_base().clone()), io.current_dir().ok()]
                .into_iter()
                .flatten()
            {
                for anc in base.ancestors().take(4) {
                    let p = anc.to_path_buf();
                    if !roots.contains(&p) {
                        roots.push(p);
                    }
                }
            }
            for ex in discover_bundled_examples(io, &roots) {
                if matches(&ex.label, &ex.detail) {
                    entries.push(ex);
                }
            }

            // Surface `.take.jsonl` files from `<dir>/takes/` when present.
            let takes_dir = hub.dir.join("takes");
            if let Ok(mut take_entries) = io.read_dir(&takes_dir) {
                take_entries.sort_by(|a, b| a.file_name.cmp(&b.file_name));
                for entry in take_entries {
                    if entry.is_file && is_take_jsonl_path(&entry.path) {
                        let detail = "takes/ · .take.jsonl".to_string();
                        if matches(&entry.file_name, &detail) {
                            entries.push(FileHubEntry {
                                section: "LIVE TAKES",
                                kind: FileEntryKind::Take,
                                label: entry.file_name,
                                detail,
                                path: entry.path,
                                preset_id: None,
                            });
                        }
                    }
                }
            }
        }

        // Discovered external PresetCatalog library on Import tab.
        if hub.tab == FileHubTab::Import {
            for preset in self.assets.presets.entries() {
                let cat = preset.category.as_deref().unwrap_or(&preset.id);
                let detail = format!("Rack ({}z) · {cat}", preset.patch.zones.len());
                if matches(&preset.name, &detail) || matches(&preset.id, &detail) {
                    entries.push(FileHubEntry {
                        section: "PRESET LIBRARY",
                        kind: FileEntryKind::InstrumentPreset,
                        label: preset.name.clone(),
                        detail,
                        path: preset.path.clone(),
                        preset_id: Some(preset.id.clone()),
                    });
                }
            }
        }

        // Directory listing for both Open and Import tabs.
        if matches!(hub.tab, FileHubTab::Open | FileHubTab::Import) {
            if let Some(parent) = hub.dir.parent() {
                if query.is_empty() {
                    entries.push(FileHubEntry {
                        section: "FILES",
                        kind: FileEntryKind::ParentDir,
                        label: ".. (Parent Directory)".into(),
                        detail: parent.display().to_string(),
                        path: parent.to_path_buf(),
                        preset_id: None,
                    });
                }
            }
            if let Ok(read) = io.read_dir(&hub.dir) {
                let mut bundles = Vec::new();
                let mut dirs = Vec::new();
                let mut files = Vec::new();
                for entry in read {
                    let name = entry.file_name;
                    let path = entry.path;
                    if name.starts_with('.') {
                        continue;
                    }
                    if entry.is_dir {
                        if hub.tab == FileHubTab::Open && is_portable_bundle_dir(io, &path) {
                            if matches(&name, "bundle") {
                                bundles.push((name, path));
                            }
                        } else if matches(&name, "") {
                            dirs.push((name, path));
                        }
                    } else if entry.is_file {
                        let ext = path
                            .extension()
                            .and_then(|e| e.to_str())
                            .unwrap_or("")
                            .to_ascii_lowercase();
                        let kind = if is_take_jsonl_path(&path) {
                            Some(FileEntryKind::Take)
                        } else if cadence_formats::find_import_format_by_ext(&ext).is_some() {
                            Some(FileEntryKind::Rbs)
                        } else if ext == "json" {
                            Some(classify_json(&path))
                        } else {
                            None
                        };
                        if let Some(kind) = kind {
                            if matches(&name, kind.badge()) {
                                files.push((name, kind, path));
                            }
                        }
                    }
                }
                bundles.sort_by_key(|a| a.0.to_ascii_lowercase());
                dirs.sort_by_key(|a| a.0.to_ascii_lowercase());
                files.sort_by_key(|a| a.0.to_ascii_lowercase());
                let sec = if hub.tab == FileHubTab::Import {
                    "IMPORTABLE (.RBS / PRESETS / MIDI / TAKES)"
                } else {
                    "FILES"
                };
                for (name, path) in bundles {
                    entries.push(FileHubEntry {
                        section: sec,
                        kind: FileEntryKind::Bundle,
                        label: name,
                        detail: "portable bundle · project.json + samples/".into(),
                        path,
                        preset_id: None,
                    });
                }
                for (name, path) in dirs {
                    entries.push(FileHubEntry {
                        section: sec,
                        kind: FileEntryKind::Dir,
                        label: format!("{name}/"),
                        detail: "folder".into(),
                        path,
                        preset_id: None,
                    });
                }
                for (name, kind, path) in files {
                    entries.push(FileHubEntry {
                        section: sec,
                        kind,
                        label: name,
                        detail: kind.badge().into(),
                        path,
                        preset_id: None,
                    });
                }
            }
        }

        hub.entries = entries;
        if hub.entries.is_empty() {
            hub.cursor = 0;
        } else {
            hub.cursor = hub.cursor.min(hub.entries.len() - 1);
        }
        // Warm preview cache for currently highlighted entry.
        self.warm_file_hub_preview(hub);
    }

    pub(super) fn warm_file_hub_preview(&self, hub: &mut FileHubState) {
        if let Some(entry) = hub.entries.get(hub.cursor) {
            let preview = self.preview_file_hub_entry(entry);
            hub.preview_cache
                .entry(entry.path.clone())
                .or_insert(preview);
        }
    }

    fn preview_file_hub_entry(&self, entry: &FileHubEntry) -> String {
        if let Some(id) = entry.preset_id.as_deref() {
            if let Some(p) = self.assets.presets.find(id) {
                let cat = p.category.as_deref().unwrap_or(&p.id);
                return format!(
                    "Rack Preset ({}z) · \"{}\" [{}] · {}",
                    p.patch.zones.len(),
                    p.name,
                    cat,
                    p.path.display()
                );
            }
        }
        peek_file_preview(self.io.as_ref(), &entry.path, entry.kind)
    }

    pub(super) fn handle_file_hub_browse_key(
        &mut self,
        hub: &mut FileHubState,
        key: KeyEvent,
    ) -> Result<Outcome> {
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => {
                hub.tab = hub.tab.cycle(-1);
                hub.cursor = 0;
                hub.query.clear();
                self.refresh_file_hub_entries(hub);
            }
            KeyCode::Char('l') | KeyCode::Right => {
                hub.tab = hub.tab.cycle(1);
                hub.cursor = 0;
                hub.query.clear();
                self.refresh_file_hub_entries(hub);
            }
            KeyCode::Char('R') if hub.tab == FileHubTab::Import => {
                self.reload_presets_with_status();
                hub.preview_cache.clear();
                self.refresh_file_hub_entries(hub);
            }
            KeyCode::Char('/') => {
                hub.searching = true;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if !hub.entries.is_empty() {
                    hub.cursor = (hub.cursor + 1).min(hub.entries.len() - 1);
                    self.warm_file_hub_preview(hub);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                hub.cursor = hub.cursor.saturating_sub(1);
                self.warm_file_hub_preview(hub);
            }
            KeyCode::Backspace => {
                if let Some(parent) = hub.dir.parent().map(Path::to_path_buf) {
                    hub.dir = parent;
                    hub.cursor = 0;
                    hub.query.clear();
                    self.refresh_file_hub_entries(hub);
                }
            }
            KeyCode::Enter => {
                if let Some(entry) = hub.entries.get(hub.cursor).cloned() {
                    match entry.kind {
                        FileEntryKind::NewProject => {
                            if self.dirty() {
                                self.modal = ActiveModal::None;
                                self.input.mode = Mode::Confirm(ConfirmAction::New(None));
                                self.status =
                                    "Unsaved changes · y / Enter discard and start new · Esc cancel"
                                        .into();
                                return Ok(Outcome::default());
                            }
                            return self.apply(Action::New { title: None });
                        }
                        FileEntryKind::ParentDir | FileEntryKind::Dir => {
                            hub.dir = entry.path;
                            hub.cursor = 0;
                            hub.query.clear();
                            self.refresh_file_hub_entries(hub);
                        }
                        FileEntryKind::Bundle => {
                            return self
                                .load_or_confirm_project(entry.path.join("project.json"), false);
                        }
                        FileEntryKind::Take => {
                            let out = self.apply(Action::LoadTake { path: entry.path })?;
                            self.input.mode = Mode::Normal;
                            self.modal = ActiveModal::None;
                            return Ok(out);
                        }
                        FileEntryKind::InstrumentPreset => {
                            if let Some(preset_id) = entry.preset_id.as_deref() {
                                if let Some(catalog_entry) = self.assets.presets.find(preset_id) {
                                    let preset = catalog_entry.to_preset();
                                    return self.load_file_hub_preset(preset);
                                }
                            }
                            return self.import_file_hub_asset(&entry.path, false);
                        }
                        FileEntryKind::MidiProfile => {
                            return self.import_file_hub_asset(&entry.path, false);
                        }
                        _ if hub.tab == FileHubTab::Import => {
                            return self.import_file_hub_asset(&entry.path, true);
                        }
                        FileEntryKind::Rbs => {
                            return self.load_or_confirm_project(entry.path, true);
                        }
                        FileEntryKind::Project | FileEntryKind::Example => {
                            return self.open_file_hub_json(entry.path);
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(Outcome::default())
    }
}
