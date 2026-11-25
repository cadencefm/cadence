//! File Hub directory listing, bounded header peek/preview, row builders, and Quick/Browse/Import navigation.
use super::{FileEntryKind, FileHubEntry, FileHubState, FileHubTab};
use crate::*;
use std::collections::HashSet;

/// Maximum recent projects listed on the unfiltered Quick Open tab so Bundled Examples and
/// Live Takes stay immediately visible without scrolling; `/` search matches the full list.
const MAX_QUICK_RECENT: usize = 8;

const SECTION_FILES: &str = "FILES";
const SECTION_IMPORT_FILES: &str = "IMPORTABLE (.RBS / PRESETS / MIDI / TAKES)";

/// Cursor position for landing cleanly inside a directory browser: selects the first actual
/// child entry (bundle, folder, or file) rather than `..`, falling back to `..` if the folder
/// is empty or `0` if no directory section is present.
pub(super) fn first_dir_entry_cursor(entries: &[FileHubEntry]) -> usize {
    let is_dir_section = |sec: &str| sec == SECTION_FILES || sec == SECTION_IMPORT_FILES;
    entries
        .iter()
        .position(|e| is_dir_section(e.section) && e.kind != FileEntryKind::ParentDir)
        .or_else(|| {
            entries
                .iter()
                .position(|e| is_dir_section(e.section) && e.kind == FileEntryKind::ParentDir)
        })
        .unwrap_or(0)
}

struct ScannedDir {
    bundles: Vec<(String, PathBuf)>,
    dirs: Vec<(String, PathBuf)>,
    files: Vec<(String, FileEntryKind, PathBuf)>,
}

fn scan_dir_entries(
    io: &dyn HostIo,
    dir: &Path,
    detect_bundles: bool,
    matches: &impl Fn(&str, &str) -> bool,
) -> ScannedDir {
    let mut bundles = Vec::new();
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    if let Ok(read) = io.read_dir(dir) {
        for entry in read {
            let name = entry.file_name;
            let path = entry.path;
            if name.starts_with('.') {
                continue;
            }
            if entry.is_dir {
                if detect_bundles && is_portable_bundle_dir(io, &path) {
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
    }
    bundles.sort_by_key(|a| a.0.to_ascii_lowercase());
    dirs.sort_by_key(|a| a.0.to_ascii_lowercase());
    files.sort_by_key(|a| a.0.to_ascii_lowercase());
    ScannedDir {
        bundles,
        dirs,
        files,
    }
}

fn push_scanned_entries(
    entries: &mut Vec<FileHubEntry>,
    section: &'static str,
    scanned: ScannedDir,
    io: &dyn HostIo,
    skip_seen: Option<&mut HashSet<PathBuf>>,
) {
    let mut skip_seen = skip_seen;
    let mut allow = |path: &Path| {
        if let Some(seen) = skip_seen.as_deref_mut() {
            let canon = io.canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            seen.insert(canon)
        } else {
            true
        }
    };
    for (name, path) in scanned.bundles {
        if allow(&path) {
            entries.push(FileHubEntry {
                section,
                kind: FileEntryKind::Bundle,
                label: name,
                detail: "portable bundle · project.json + samples/".into(),
                path,
                preset_id: None,
            });
        }
    }
    for (name, path) in scanned.dirs {
        if allow(&path) {
            entries.push(FileHubEntry {
                section,
                kind: FileEntryKind::Dir,
                label: format!("{name}/"),
                detail: "folder".into(),
                path,
                preset_id: None,
            });
        }
    }
    for (name, kind, path) in scanned.files {
        if allow(&path) {
            entries.push(FileHubEntry {
                section,
                kind,
                label: name,
                detail: kind.badge().into(),
                path,
                preset_id: None,
            });
        }
    }
}

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

        if hub.tab == FileHubTab::Quick {
            let mut seen_quick = HashSet::new();
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

            let mut recent_added = 0usize;
            for recent in &self.config.recent_projects {
                if query.is_empty() && recent_added >= MAX_QUICK_RECENT {
                    break;
                }
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
                    let canon = io.canonicalize(recent).unwrap_or_else(|_| recent.clone());
                    seen_quick.insert(canon);
                    entries.push(FileHubEntry {
                        section: "RECENT",
                        kind,
                        label,
                        detail,
                        path: recent.clone(),
                        preset_id: None,
                    });
                    recent_added += 1;
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
                    let canon = io
                        .canonicalize(&ex.path)
                        .unwrap_or_else(|_| ex.path.clone());
                    seen_quick.insert(canon);
                    entries.push(ex);
                }
            }

            // Surface `.take.jsonl` files from `<project_dir>/takes/` and `<project_dir>/`.
            let project_dir = self.doc.asset_base();
            let take_dirs = [
                (project_dir.join("takes"), "takes/ · .take.jsonl"),
                (project_dir.clone(), ".take.jsonl"),
            ];
            for (dir_path, detail_label) in take_dirs {
                if let Ok(mut take_entries) = io.read_dir(&dir_path) {
                    take_entries.sort_by(|a, b| a.file_name.cmp(&b.file_name));
                    for entry in take_entries {
                        if entry.is_file && is_take_jsonl_path(&entry.path) {
                            let canon = io
                                .canonicalize(&entry.path)
                                .unwrap_or_else(|_| entry.path.clone());
                            if !seen_quick.contains(&canon)
                                && matches(&entry.file_name, detail_label)
                            {
                                seen_quick.insert(canon);
                                entries.push(FileHubEntry {
                                    section: "LIVE TAKES",
                                    kind: FileEntryKind::Take,
                                    label: entry.file_name,
                                    detail: detail_label.into(),
                                    path: entry.path,
                                    preset_id: None,
                                });
                            }
                        }
                    }
                }
            }

            // When actively filtering on Quick Open (`/`), also match items in the current folder
            // so `gf /foo <Enter>` finds local projects, bundles, presets, or folders immediately
            // without cluttering the default unfiltered Quick view.
            if !query.is_empty() {
                let scanned = scan_dir_entries(io, &hub.dir, true, &matches);
                push_scanned_entries(
                    &mut entries,
                    "CURRENT FOLDER",
                    scanned,
                    io,
                    Some(&mut seen_quick),
                );
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

        // Dedicated directory listing for Browse and Import tabs.
        if hub.tab.has_dir_browser() {
            let sec = if hub.tab == FileHubTab::Import {
                SECTION_IMPORT_FILES
            } else {
                SECTION_FILES
            };
            if let Some(parent) = hub.dir.parent() {
                if query.is_empty() {
                    entries.push(FileHubEntry {
                        section: sec,
                        kind: FileEntryKind::ParentDir,
                        label: ".. (Parent Directory)".into(),
                        detail: parent.display().to_string(),
                        path: parent.to_path_buf(),
                        preset_id: None,
                    });
                }
            }
            let detect_bundles = hub.tab == FileHubTab::Browse;
            let scanned = scan_dir_entries(io, &hub.dir, detect_bundles, &matches);
            push_scanned_entries(&mut entries, sec, scanned, io, None);
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

    /// Navigate into `target` directory, clearing any filter and landing the cursor on the
    /// first real entry inside the directory section (or `..` if the folder is empty).
    fn navigate_file_hub_dir(&self, hub: &mut FileHubState, target: PathBuf) {
        hub.dir = target;
        hub.query.clear();
        hub.searching = false;
        self.refresh_file_hub_entries(hub);
        hub.cursor = first_dir_entry_cursor(&hub.entries);
        self.warm_file_hub_preview(hub);
    }

    /// Navigate up to the parent directory and restore focus onto the subdirectory just exited.
    fn navigate_file_hub_parent(&self, hub: &mut FileHubState) {
        let Some(parent) = hub.dir.parent().map(Path::to_path_buf) else {
            return;
        };
        let exited_dir = hub.dir.clone();
        let exited_canon = self
            .io
            .canonicalize(&exited_dir)
            .unwrap_or_else(|_| exited_dir.clone());
        hub.dir = parent;
        hub.query.clear();
        hub.searching = false;
        self.refresh_file_hub_entries(hub);
        let restored = hub.entries.iter().position(|e| {
            matches!(e.kind, FileEntryKind::Dir | FileEntryKind::Bundle)
                && (e.path == exited_dir
                    || self
                        .io
                        .canonicalize(&e.path)
                        .is_ok_and(|c| c == exited_canon))
        });
        hub.cursor = restored.unwrap_or_else(|| first_dir_entry_cursor(&hub.entries));
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
                let prev = hub.tab.cycle(-1);
                self.switch_file_hub_tab(hub, prev);
            }
            KeyCode::Char('l') | KeyCode::Right => {
                let next = hub.tab.cycle(1);
                self.switch_file_hub_tab(hub, next);
            }
            KeyCode::Char('R') if hub.tab == FileHubTab::Import => {
                self.reload_presets_with_status();
                hub.preview_cache.clear();
                self.refresh_file_hub_entries(hub);
            }
            KeyCode::Char('/') => {
                hub.searching = true;
            }
            KeyCode::Char('~') if hub.tab.has_dir_browser() => {
                if let Some(home) = self.io.home_dir().filter(|p| self.io.is_dir(p)) {
                    self.status = format!("Browsing home · {}", home.display());
                    self.navigate_file_hub_dir(hub, home);
                } else {
                    self.status = "Home directory unavailable".into();
                }
            }
            KeyCode::Char('.') if hub.tab.has_dir_browser() => {
                let root = self.doc.asset_base().clone();
                self.status = format!("Browsing project folder · {}", root.display());
                self.navigate_file_hub_dir(hub, root);
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
            KeyCode::Backspace if hub.tab.has_dir_browser() => {
                self.navigate_file_hub_parent(hub);
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
                        FileEntryKind::ParentDir => {
                            self.navigate_file_hub_parent(hub);
                        }
                        FileEntryKind::Dir => {
                            if hub.tab == FileHubTab::Quick {
                                hub.tab = FileHubTab::Browse;
                            }
                            self.navigate_file_hub_dir(hub, entry.path);
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
