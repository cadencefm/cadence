//! Unified external instrument presets (`InstrumentPreset`) and runtime disk
//! scanner (`PresetCatalog`) for Cadence's unified `RackPatch` instrument model.
//!
//! Zero factory preset patches are compiled into the binary; only the bare
//! neutral engine templates live in code (`RackPatch::from_template`).
//! Missing or empty preset directories degrade gracefully with zero errors.
use crate::config::UserConfig;
use crate::rack::{RackPatch, RackTemplate};
use crate::store;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Maximum external preset JSON file size (512 KiB — safely bounded for 32-zone Racks).
pub const MAX_PRESET_BYTES: u64 = 512 * 1024;
const MAX_PRESET_FILES: usize = 2_048;
const MAX_PRESET_SCAN_ITEMS: usize = 16_384;

/// Standalone editable instrument preset document for a [`RackPatch`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstrumentPreset {
    pub version: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub patch: RackPatch,
}

/// Current schema version for [`InstrumentPreset`] JSON documents.
pub const INSTRUMENT_PRESET_VERSION: u32 = 1;

impl InstrumentPreset {
    pub fn new(name: impl Into<String>, category: Option<String>, patch: RackPatch) -> Self {
        Self {
            version: INSTRUMENT_PRESET_VERSION,
            name: name.into(),
            category,
            patch,
        }
    }

    /// Construct the bare neutral engine default preset for `template` (zero disk I/O).
    pub fn bare_default(template: RackTemplate) -> Self {
        Self::new(
            template.default_preset_name(),
            None,
            RackPatch::from_template(template),
        )
    }

    /// True when this preset represents a multi-zone or explicitly `Kit`-categorized Rack kit.
    pub fn is_kit(&self) -> bool {
        is_kit_preset(self.category.as_deref(), &self.patch)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == INSTRUMENT_PRESET_VERSION,
            "Unsupported instrument preset version {}; expected {INSTRUMENT_PRESET_VERSION}",
            self.version
        );
        ensure!(
            !self.name.trim().is_empty()
                && self.name.chars().count() <= 80
                && !self.name.chars().any(char::is_control),
            "Preset name must contain 1–80 printable characters"
        );
        if let Some(cat) = &self.category {
            ensure!(
                !cat.trim().is_empty()
                    && cat.chars().count() <= 48
                    && !cat.chars().any(char::is_control),
                "Preset category must contain 1–48 printable characters"
            );
        }
        self.patch.validate()
    }

    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self> {
        let preset: Self =
            serde_json::from_slice(bytes).context("Invalid Instrument preset document")?;
        preset.validate()?;
        Ok(preset)
    }

    pub fn to_json_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut buf = serde_json::to_vec_pretty(self)?;
        buf.push(b'\n');
        Ok(buf)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let bytes = store::read_bounded(path, MAX_PRESET_BYTES)?;
        Self::from_json_bytes(&bytes)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let bytes = self.to_json_bytes()?;
        store::write_atomic_bytes(path, MAX_PRESET_BYTES, &bytes)
            .with_context(|| format!("Saving preset {}", path.display()))
    }
}

/// One validated external preset discovered on disk by [`PresetCatalog`].
#[derive(Clone, Debug, PartialEq)]
pub struct PresetEntry {
    pub id: String,
    pub name: String,
    pub category: Option<String>,
    pub path: PathBuf,
    pub patch: RackPatch,
}

fn is_kit_preset(category: Option<&str>, patch: &RackPatch) -> bool {
    category.is_some_and(|c| c.to_ascii_lowercase().contains("kit")) || patch.zones.len() > 1
}

impl PresetEntry {
    /// Materialize this catalog entry as an [`InstrumentPreset`].
    pub fn to_preset(&self) -> InstrumentPreset {
        InstrumentPreset::new(self.name.clone(), self.category.clone(), self.patch.clone())
    }

    /// True when this preset represents a multi-zone or explicitly `Kit`-categorized Rack kit.
    pub fn is_kit(&self) -> bool {
        is_kit_preset(self.category.as_deref(), &self.patch)
    }
}

/// Runtime catalog of external `.json` [`InstrumentPreset`]s scanned from configured `preset_dirs`.
#[derive(Clone, Debug, Default)]
pub struct PresetCatalog {
    roots: Vec<PathBuf>,
    entries: Vec<PresetEntry>,
    warnings: Vec<String>,
}

/// Pure lookup key normalization (lowercase, spaces/underscores -> hyphen, collapsed).
/// Never hardcodes any patch data; maps only common short aliases onto external preset slugs.
pub fn normalize_preset_key(input: &str) -> String {
    let mut slug = String::with_capacity(input.len());
    let mut last_dash = true;
    for ch in input.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if (ch.is_whitespace() || matches!(ch, '-' | '_')) && !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let trimmed = slug.trim_end_matches('-');
    match trimmed {
        "808" | "tr808" => "tr-808".into(),
        "909" | "tr909" => "tr-909".into(),
        "edm" | "edmkit" | "kit" => "edm-kit".into(),
        "303" | "acid" | "tb303" | "tb-303" => "acid-303".into(),
        "lead" => "pulse-lead".into(),
        "pad" => "warm-pad".into(),
        "bass" | "sub" => "sub-bass".into(),
        "hat" => "closed-hat".into(),
        other => other.to_string(),
    }
}

fn compact_alnum(input: &str) -> String {
    input
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn preset_id_from_path(path: &Path) -> String {
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("preset");
    let stem = file_name
        .strip_suffix(".preset.json")
        .or_else(|| file_name.strip_suffix(".json"))
        .unwrap_or(file_name);
    let mut out = String::with_capacity(stem.len());
    let mut last_dash = true;
    for ch in stem.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if (ch.is_whitespace() || matches!(ch, '-' | '_')) && !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let clean = out.trim_end_matches('-');
    if clean.is_empty() {
        "preset".into()
    } else {
        clean.to_string()
    }
}

impl PresetCatalog {
    /// Empty catalog with zero roots, zero entries, and zero errors.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Resolve prioritized existing preset root directories:
    /// 1. `<project_dir>/presets`
    /// 2. Explicit `extra_dirs`
    /// 3. `UserConfig.preset_dirs`
    /// 4. `<user_config_dir>/presets`
    /// 5. `./presets`
    /// 6. `<CADENCE_WORKSPACE_DIR>/presets` (workspace dev/test root)
    /// 7. `<exe_dir>/presets`
    pub fn default_roots(
        extra_dirs: &[PathBuf],
        project_path: Option<&Path>,
        config: &UserConfig,
    ) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = Vec::new();
        let mut push_unique = |candidate: PathBuf| {
            let resolved = candidate.canonicalize().unwrap_or(candidate);
            if resolved.is_dir() && !roots.iter().any(|r| r == &resolved) {
                roots.push(resolved);
            }
        };

        if let Some(proj) = project_path {
            if let Some(parent) = proj.parent().filter(|p| !p.as_os_str().is_empty()) {
                push_unique(parent.join("presets"));
            }
        }
        for dir in extra_dirs {
            push_unique(dir.clone());
        }
        for dir in &config.preset_dirs {
            if !dir.trim().is_empty() {
                push_unique(store::expand_tilde(dir));
            }
        }
        if let Some(cfg_dir) = UserConfig::default_config_dir() {
            push_unique(cfg_dir.join("presets"));
        }
        push_unique(PathBuf::from("presets"));
        if let Some(ws) = option_env!("CADENCE_WORKSPACE_DIR").filter(|s| !s.is_empty()) {
            push_unique(Path::new(ws).join("presets"));
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                push_unique(dir.join("presets"));
            }
        }
        roots
    }

    /// Discover and scan all default preset roots cleanly. Never fails if directories are missing.
    pub fn discover(
        extra_dirs: &[PathBuf],
        project_path: Option<&Path>,
        config: &UserConfig,
    ) -> Self {
        let roots = Self::default_roots(extra_dirs, project_path, config);
        Self::from_roots(&roots)
    }

    /// Scan ONLY the explicitly supplied root directories. Missing roots are ignored cleanly.
    pub fn from_roots(roots: &[PathBuf]) -> Self {
        let mut unique_roots = Vec::new();
        for root in roots {
            let resolved = root.canonicalize().unwrap_or_else(|_| root.clone());
            if resolved.is_dir() && !unique_roots.iter().any(|r| r == &resolved) {
                unique_roots.push(resolved);
            }
        }
        let mut catalog = Self {
            roots: unique_roots,
            entries: Vec::new(),
            warnings: Vec::new(),
        };
        catalog.rescan();
        catalog
    }

    /// Re-scan configured roots with bounded reads and non-fatal skipped-file warnings.
    pub fn rescan(&mut self) {
        self.rescan_with_limits(MAX_PRESET_FILES, MAX_PRESET_SCAN_ITEMS);
    }

    fn rescan_with_limits(&mut self, max_files: usize, max_items: usize) {
        self.entries.clear();
        self.warnings.clear();
        let mut visited_dirs: HashSet<PathBuf> = HashSet::new();
        let mut seen_ids: HashSet<String> = HashSet::new();
        let mut scanned_items = 0usize;

        'roots: for root in &self.roots {
            let mut stack = vec![root.clone()];
            while let Some(dir) = stack.pop() {
                let Ok(real_dir) = dir.canonicalize() else {
                    continue;
                };
                if !visited_dirs.insert(real_dir) {
                    continue;
                }
                let Ok(read_dir) = fs::read_dir(&dir) else {
                    continue;
                };
                let remaining = max_items.saturating_sub(scanned_items);
                let mut children: Vec<PathBuf> = read_dir
                    .flatten()
                    .take(remaining.saturating_add(1))
                    .map(|e| e.path())
                    .collect();
                let hit_item_limit = children.len() > remaining;
                if hit_item_limit {
                    children.truncate(remaining);
                }
                scanned_items = scanned_items.saturating_add(children.len());
                children.sort();
                for child in children {
                    let Ok(meta) = fs::symlink_metadata(&child) else {
                        continue;
                    };
                    if meta.file_type().is_dir() {
                        stack.push(child);
                        continue;
                    }
                    if !child.is_file() {
                        continue;
                    }
                    let is_json = child
                        .extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case("json"));
                    if !is_json {
                        continue;
                    }
                    // Skip the external rhythmic filter groove bank JSON (loaded by `filter_patterns`).
                    if child
                        .file_name()
                        .and_then(|s| s.to_str())
                        .is_some_and(|n| n.eq_ignore_ascii_case("filter_grooves.json"))
                    {
                        continue;
                    }
                    if self.entries.len() >= max_files {
                        self.warnings.push(format!(
                            "Preset catalog reached limit ({max_files} presets)"
                        ));
                        break 'roots;
                    }
                    match InstrumentPreset::load(&child) {
                        Ok(preset) => {
                            let id = preset_id_from_path(&child);
                            let key = normalize_preset_key(&id);
                            if seen_ids.insert(key) {
                                self.entries.push(PresetEntry {
                                    id,
                                    name: preset.name,
                                    category: preset.category,
                                    path: child,
                                    patch: preset.patch,
                                });
                            }
                        }
                        Err(err) => {
                            self.warnings.push(format!(
                                "Skipped invalid preset {}: {err:#}",
                                child.display()
                            ));
                        }
                    }
                }
                if hit_item_limit {
                    self.warnings
                        .push(format!("Preset scan reached item limit ({max_items})"));
                    break 'roots;
                }
            }
        }

        self.entries.sort_by(|a, b| {
            a.category
                .as_deref()
                .unwrap_or("")
                .to_ascii_lowercase()
                .cmp(&b.category.as_deref().unwrap_or("").to_ascii_lowercase())
                .then_with(|| {
                    a.name
                        .to_ascii_lowercase()
                        .cmp(&b.name.to_ascii_lowercase())
                })
                .then_with(|| a.id.cmp(&b.id))
        });
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    pub fn entries(&self) -> &[PresetEntry] {
        &self.entries
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// True if `token` names the bare built-in neutral engine default for `template`
    /// (`init`/`default`/`blank`/`synth`, `blank`/`empty`/`sampler`, `kit`, `modular`, etc.).
    pub fn is_bare_default_token(template: RackTemplate, token: &str) -> bool {
        let key = token.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        RackTemplate::parse(&key) == Some(template)
            || matches!(key.as_str(), "blank" | "empty" | "default")
    }

    /// Look up a preset across all categories by slug `id` or display `name`
    /// (case/hyphen/underscore insensitive with standard alias normalization).
    pub fn find(&self, query: &str) -> Option<&PresetEntry> {
        self.find_matching(None, query)
    }

    /// Look up a preset restricted to a single optional category (case-insensitive).
    pub fn find_by_category(&self, category: &str, query: &str) -> Option<&PresetEntry> {
        self.find_matching(Some(category), query)
    }

    fn find_matching(&self, category: Option<&str>, query: &str) -> Option<&PresetEntry> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return None;
        }
        let norm = normalize_preset_key(trimmed);
        let compact = compact_alnum(&norm);
        let raw_compact = compact_alnum(trimmed);
        let matches_cat = |e: &&PresetEntry| {
            category.is_none_or(|cat| {
                e.category
                    .as_deref()
                    .is_some_and(|c| c.eq_ignore_ascii_case(cat))
            })
        };

        // 1. Exact normalized ID match (including alias normalization).
        if let Some(hit) = self
            .entries
            .iter()
            .filter(matches_cat)
            .find(|e| normalize_preset_key(&e.id) == norm || e.id.eq_ignore_ascii_case(trimmed))
        {
            return Some(hit);
        }
        // 2. Exact normalized display name match.
        if let Some(hit) =
            self.entries.iter().filter(matches_cat).find(|e| {
                normalize_preset_key(&e.name) == norm || e.name.eq_ignore_ascii_case(trimmed)
            })
        {
            return Some(hit);
        }
        // 3. Punctuation-stripped match (`pulselead` -> `pulse-lead`, `tr808` -> `tr-808`).
        self.entries.iter().filter(matches_cat).find(|e| {
            let cid = compact_alnum(&e.id);
            let cname = compact_alnum(&e.name);
            cid == compact || cid == raw_compact || cname == compact || cname == raw_compact
        })
    }

    /// Comma-separated preview of available preset IDs for user-facing error hints.
    pub fn names_hint(&self, category: Option<&str>) -> String {
        let ids: Vec<&str> = self
            .entries
            .iter()
            .filter(|e| {
                category.is_none_or(|cat| {
                    e.category
                        .as_deref()
                        .is_some_and(|c| c.eq_ignore_ascii_case(cat))
                })
            })
            .take(12)
            .map(|e| e.id.as_str())
            .collect();
        if ids.is_empty() {
            "none found in preset_dirs (bare defaults remain available)".into()
        } else {
            ids.join(", ")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rack::{ModuleParam, RackZone};

    #[test]
    fn instrument_preset_roundtrips_rack_templates_and_preserves_file_on_invalid_save() {
        let folder = tempfile::tempdir().unwrap();

        // 1. Synth Rack preset roundtrip + atomic invalid save guard
        let synth_path = folder.path().join("lead.preset.json");
        let mut synth_preset =
            InstrumentPreset::new("My Synth", Some("Lead".into()), RackPatch::default_synth());
        synth_preset.save(&synth_path).unwrap();
        assert_eq!(InstrumentPreset::load(&synth_path).unwrap(), synth_preset);
        let previous = fs::read(&synth_path).unwrap();
        synth_preset.patch.cutoff_hz = f32::NAN;
        assert!(synth_preset.save(&synth_path).is_err());
        assert_eq!(fs::read(&synth_path).unwrap(), previous);

        // 2. Sampler Rack preset roundtrip
        let sampler_path = folder.path().join("kick.preset.json");
        let mut sampler_rack = RackPatch::default_sampler();
        if let Some((_, _, smp)) = sampler_rack.sampler_modules_mut().next() {
            smp.path = Some("factory-samples/tr-808/01-bd.wav".into());
            smp.root_midi = 36;
        }
        let sampler_preset =
            InstrumentPreset::new("Punch Kick", Some("Drums".into()), sampler_rack);
        sampler_preset.save(&sampler_path).unwrap();
        assert_eq!(
            InstrumentPreset::load(&sampler_path).unwrap(),
            sampler_preset
        );

        // 3. Multi-zone Kit Rack preset roundtrip
        let kit_path = folder.path().join("kit.preset.json");
        let mut kit_rack = RackPatch::default_kit();
        kit_rack.kit_name = "Mini Kit".into();
        kit_rack.zones = vec![RackZone::sample_pad(
            1,
            "BD",
            36,
            "factory-samples/tr-808/01-bd.wav",
        )];
        let kit_preset = InstrumentPreset::new("Mini Kit", Some("Drum Kits".into()), kit_rack);
        kit_preset.save(&kit_path).unwrap();
        assert_eq!(InstrumentPreset::load(&kit_path).unwrap(), kit_preset);

        // 4. Bare defaults exist for every RackTemplate without disk I/O.
        for tmpl in RackTemplate::ALL {
            let preset = InstrumentPreset::bare_default(*tmpl);
            preset.validate().unwrap();
        }
    }

    #[test]
    fn preset_catalog_scans_aliases_handles_priority_invalid_files_and_empty_dirs() {
        // Missing/nonexistent root yields an empty catalog with zero errors or warnings.
        let empty = PresetCatalog::from_roots(&[PathBuf::from("/nonexistent/cadence/presets")]);
        assert!(empty.is_empty());
        assert!(empty.warnings().is_empty());
        assert!(empty.find("tr-808").is_none());

        let base = tempfile::tempdir().unwrap();
        let proj_presets = base.path().join("proj_presets");
        let global_presets = base.path().join("global_presets");
        fs::create_dir_all(proj_presets.join("synth")).unwrap();
        fs::create_dir_all(global_presets.join("synth")).unwrap();
        fs::create_dir_all(global_presets.join("kits")).unwrap();

        // Project override of acid-303 has filter cutoff 999 Hz; global has 440 Hz.
        let mut p_override = RackPatch::acid_303();
        p_override
            .write_param_native(
                crate::params::InstrumentParam::Module {
                    zone: 1,
                    module: 2,
                    param: ModuleParam::Cutoff,
                },
                999.0,
            )
            .unwrap();
        InstrumentPreset::new("Acid 303", Some("Bass".into()), p_override)
            .save(&proj_presets.join("synth/acid-303.json"))
            .unwrap();

        let mut p_global = RackPatch::acid_303();
        p_global
            .write_param_native(
                crate::params::InstrumentParam::Module {
                    zone: 1,
                    module: 2,
                    param: ModuleParam::Cutoff,
                },
                440.0,
            )
            .unwrap();
        InstrumentPreset::new("Acid 303", Some("Bass".into()), p_global)
            .save(&global_presets.join("synth/acid-303.json"))
            .unwrap();

        InstrumentPreset::new("TR-808", Some("Drum Kits".into()), RackPatch::default_kit())
            .save(&global_presets.join("kits/tr-808.json"))
            .unwrap();

        // Malformed JSON and filter_grooves.json live alongside valid presets:
        // filter_grooves.json must be silently skipped; bad JSON must produce a non-fatal warning.
        fs::write(
            global_presets.join("filter_grooves.json"),
            b"{\"version\":1,\"grooves\":[]}",
        )
        .unwrap();
        fs::write(global_presets.join("broken.json"), b"{ not valid json").unwrap();

        let catalog = PresetCatalog::from_roots(&[proj_presets, global_presets]);
        assert_eq!(catalog.entries().len(), 2);
        assert_eq!(catalog.warnings().len(), 1);
        assert!(catalog.warnings()[0].contains("broken.json"));

        // Alias lookups (`303`, `acid`, `808`, `tr808`, `TR-808`) resolve cleanly.
        let acid = catalog.find("303").expect("alias 303 must resolve");
        assert_eq!(acid.id, "acid-303");
        let cutoff = acid
            .patch
            .read_param_native(crate::params::InstrumentParam::Module {
                zone: 1,
                module: 2,
                param: ModuleParam::Cutoff,
            })
            .unwrap();
        assert!(
            (cutoff - 999.0).abs() < 1e-4,
            "project preset must take precedence over global preset"
        );
        assert_eq!(
            catalog.find_by_category("Drum Kits", "808").unwrap().id,
            "tr-808"
        );
        assert_eq!(
            catalog.find_by_category("drum kits", "tr808").unwrap().id,
            "tr-808"
        );
        assert!(catalog.find_by_category("Bass", "808").is_none());
    }

    #[test]
    fn preset_catalog_caps_directory_scan_items_boundedly() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..12 {
            let p = InstrumentPreset::new(
                format!("Patch {i:02}"),
                Some("Synth".into()),
                RackPatch::default_synth(),
            );
            p.save(&dir.path().join(format!("patch-{i:02}.json")))
                .unwrap();
        }
        let mut catalog = PresetCatalog {
            roots: vec![dir.path().to_path_buf()],
            entries: Vec::new(),
            warnings: Vec::new(),
        };
        catalog.rescan_with_limits(100, 5);
        assert_eq!(catalog.entries().len(), 5);
        assert!(
            catalog
                .warnings()
                .iter()
                .any(|w| w.contains("Preset scan reached item limit (5)")),
            "expected scan item limit warning, got {:?}",
            catalog.warnings()
        );
    }
}
