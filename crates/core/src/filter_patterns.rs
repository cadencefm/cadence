//! External rhythmic filter envelope groove bank (`presets/filter_grooves.json`)
//! and fixed-size zero-allocation trigger grids for Cadence's unified `Filter` effect.
//!
//! Supports 8th-, 16th-, and 32nd-note step resolutions, variable pattern lengths
//! (`1..=64`, including odd-meter polyrhythms), per-step velocity accents (`0..=127`),
//! and snappy vs. smooth-attack sweep contours — usable both standalone in any Cadence
//! track (alongside custom project `Pattern` bindings) and with user-supplied external
//! groove banks of up to 128 slots (`0..=127`, matching 7-bit MIDI & stable modulation
//! coordinates).
//!
//! Zero groove patterns are compiled into the binary. If `presets/filter_grooves.json`
//! is missing or deleted — or if a project references a groove ID not present in the
//! loaded bank — lookups degrade gracefully to a neutral 1-step pulse
//! ([`CompiledFilterPattern::neutral`]) without aliasing onto another groove.

use crate::config::UserConfig;
use crate::material::{Id, Pattern, PatternKind, ValuePoint, ValueUnit};
use crate::params::ValueMode;
use crate::presets::PresetCatalog;
use crate::store;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// Maximum addressable groove bank slots (`0..=127`, aligned with 7-bit MIDI and
/// stable normalized `EffectParam::EnvPattern` modulation coordinates).
pub const FILTER_ENV_PATTERN_COUNT: usize = 128;
pub const MAX_FILTER_ENV_STEPS: usize = 64;
pub const MAX_FILTER_GROOVES_BYTES: u64 = 512 * 1024;

/// Whether a JSON path follows a recognizable Filter Groove Bank naming convention
/// (`filter_grooves.json`, `*.grooves.json`, or `*filter_groove*.json`).
pub fn is_filter_groove_bank_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    lower == "filter_grooves.json"
        || lower.ends_with(".grooves.json")
        || (lower.ends_with(".json") && lower.contains("filter_groove"))
}

mod serde_steps {
    use super::MAX_FILTER_ENV_STEPS;
    use serde::{de, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(
        steps: &[u8; MAX_FILTER_ENV_STEPS],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        steps.as_slice().serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<[u8; MAX_FILTER_ENV_STEPS], D::Error> {
        let vec = <Vec<u8> as serde::Deserialize>::deserialize(deserializer)?;
        if vec.len() > MAX_FILTER_ENV_STEPS {
            return Err(de::Error::invalid_length(
                vec.len(),
                &"at most 64 filter envelope steps",
            ));
        }
        let mut out = [0u8; MAX_FILTER_ENV_STEPS];
        out[..vec.len()].copy_from_slice(&vec);
        Ok(out)
    }
}

/// Strip an optional leading numeric slot prefix (`"08 Dotted Bounce"` -> `"Dotted Bounce"`).
pub fn strip_groove_id_prefix(name: &str) -> &str {
    let trimmed = name.trim();
    if let Some((head, tail)) = trimmed.split_once(char::is_whitespace) {
        let tail = tail.trim_start();
        if !head.is_empty() && head.bytes().all(|b| b.is_ascii_digit()) && !tail.is_empty() {
            return tail;
        }
    }
    trimmed
}

/// Format a groove name with its 2-digit slot ID (`"24 My Groove"`), avoiding duplicate prefixes.
pub fn format_groove_name_with_id(id: u8, raw_name: &str) -> String {
    let base = strip_groove_id_prefix(raw_name);
    format!("{id:02} {base}")
}

/// Return the lowest unused groove ID in `0..FILTER_ENV_PATTERN_COUNT`, if any slot is free.
pub fn next_available_id(grooves: &[FilterEnvPreset]) -> Option<u8> {
    let mut used = [false; FILTER_ENV_PATTERN_COUNT];
    for g in grooves {
        if let Some(slot) = used.get_mut(usize::from(g.id)) {
            *slot = true;
        }
    }
    used.iter().position(|&taken| !taken).map(|idx| idx as u8)
}

/// Validate and insert or replace a groove by `groove.id`, keeping `grooves` sorted by `id`.
pub fn upsert_groove(grooves: &mut Vec<FilterEnvPreset>, groove: FilterEnvPreset) -> Result<()> {
    groove.validate()?;
    if let Some(existing) = grooves.iter_mut().find(|g| g.id == groove.id) {
        *existing = groove;
    } else {
        ensure!(
            grooves.len() < FILTER_ENV_PATTERN_COUNT,
            "Filter groove bank is full ({FILTER_ENV_PATTERN_COUNT} grooves maximum)"
        );
        grooves.push(groove);
    }
    grooves.sort_by_key(|g| g.id);
    Ok(())
}

/// One rhythmic envelope groove definition loaded from external `filter_grooves.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilterEnvPreset {
    pub id: u8,
    pub name: String,
    pub category: String,
    /// Steps per 4/4 bar (`8`, `16`, or `32`).
    pub resolution: u8,
    /// Active step count (`1..=64`).
    pub length: u8,
    /// True for smooth wah/ramp contours that slew smoothly between consecutive steps;
    /// false for crisp percussive rhythmic gate/filter triggers.
    pub smooth_attack: bool,
    /// Per-step trigger velocities (`0` = hold/decay without retrigger, `1..=127` = trigger height).
    pub steps: Vec<u8>,
}

impl FilterEnvPreset {
    /// Neutral 1-step fallback groove used whenever `filter_grooves.json` is absent or an ID is missing.
    pub fn neutral(id: u8) -> Self {
        Self {
            id,
            name: format!("{id:02} Neutral Pulse"),
            category: "Neutral".into(),
            resolution: 16,
            length: 1,
            smooth_attack: false,
            steps: vec![127],
        }
    }

    /// Build a validated [`FilterEnvPreset`] directly from a [`CompiledFilterPattern`].
    pub fn from_compiled(
        id: u8,
        name: impl Into<String>,
        category: impl Into<String>,
        compiled: CompiledFilterPattern,
    ) -> Result<Self> {
        let len = usize::from(compiled.length).clamp(1, MAX_FILTER_ENV_STEPS);
        let steps = compiled.steps[..len]
            .iter()
            .map(|&v| v.min(127))
            .collect::<Vec<u8>>();
        let preset = Self {
            id,
            name: name.into().trim().to_string(),
            category: category.into().trim().to_string(),
            resolution: match compiled.resolution {
                8 | 16 | 32 => compiled.resolution,
                _ => 16,
            },
            length: len as u8,
            smooth_attack: compiled.smooth_attack,
            steps,
        };
        preset.validate()?;
        Ok(preset)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            usize::from(self.id) < FILTER_ENV_PATTERN_COUNT,
            "Filter groove id {} must be 0..={}",
            self.id,
            FILTER_ENV_PATTERN_COUNT - 1
        );
        ensure!(
            matches!(self.resolution, 8 | 16 | 32),
            "Filter groove {} resolution {} must be 8, 16, or 32",
            self.id,
            self.resolution
        );
        ensure!(
            (1..=MAX_FILTER_ENV_STEPS as u8).contains(&self.length),
            "Filter groove {} length {} must be 1..={MAX_FILTER_ENV_STEPS}",
            self.id,
            self.length
        );
        ensure!(
            self.steps.len() == usize::from(self.length),
            "Filter groove {} steps length {} must match declared length {}",
            self.id,
            self.steps.len(),
            self.length
        );
        ensure!(
            self.steps.iter().all(|&v| v <= 127),
            "Filter groove {} step velocities must be 0..=127",
            self.id
        );
        let name = self.name.trim();
        ensure!(
            !name.is_empty()
                && name.chars().count() <= 80
                && !self.name.chars().any(char::is_control),
            "Filter groove {} name must be 1..=80 printable characters",
            self.id
        );
        let category = self.category.trim();
        ensure!(
            !category.is_empty()
                && category.chars().count() <= 40
                && !self.category.chars().any(char::is_control),
            "Filter groove {} category must be 1..=40 printable characters",
            self.id
        );
        Ok(())
    }

    pub fn compile(&self) -> CompiledFilterPattern {
        let mut steps = [0u8; MAX_FILTER_ENV_STEPS];
        let len = usize::from(self.length)
            .min(self.steps.len())
            .clamp(1, MAX_FILTER_ENV_STEPS);
        steps[..len].copy_from_slice(&self.steps[..len]);
        CompiledFilterPattern {
            resolution: match self.resolution {
                8 | 16 | 32 => self.resolution,
                _ => 16,
            },
            length: len as u8,
            smooth_attack: self.smooth_attack,
            steps,
        }
    }

    /// Convert this groove's step contour into an editable literal Cadence `Values(Normalized)`
    /// pattern (`Ramp` when `smooth_attack` is true, `Hold` when snappy).
    pub fn to_pattern(&self, id: Id) -> Pattern {
        let len = usize::from(self.length)
            .min(self.steps.len())
            .clamp(1, MAX_FILTER_ENV_STEPS);
        let points = self.steps[..len]
            .iter()
            .enumerate()
            .map(|(step, &vel)| ValuePoint {
                step: step as u32,
                value: f32::from(vel.min(127)) / 127.0,
            })
            .collect();
        let name = strip_groove_id_prefix(&self.name).to_string();
        Pattern {
            id,
            name: if name.is_empty() {
                format!("Groove {:02}", self.id)
            } else {
                name
            },
            kind: PatternKind::Values {
                unit: ValueUnit::Normalized,
                period: len as u32,
                mode: if self.smooth_attack {
                    ValueMode::Ramp
                } else {
                    ValueMode::Hold
                },
                points,
            },
        }
    }

    /// Convert this groove into project pattern(s) that preserve exact timing & step values
    /// across `compile_filter_pattern` for ALL resolutions (`8`, `16`, and `32`):
    /// - `16th`: 1 editable literal `Values(Normalized)` pattern (`base_id`).
    /// - `8th` / `32nd`: editable literal `Values(Normalized)` (`base_id`) + `TimeScale` wrapper
    ///   (`1/2` for 8th, `2/1` for 32nd) at `wrapper_id`.
    ///
    /// Returns `(patterns_to_add, bound_pattern_id)`.
    pub fn to_project_patterns(
        &self,
        base_id: Id,
        wrapper_id: Id,
        base_name: String,
        wrapper_name: Option<String>,
    ) -> (Vec<Pattern>, Id) {
        let mut base = self.to_pattern(base_id);
        base.name = base_name;
        match self.resolution {
            8 | 32 => {
                let (num, den, suffix) = if self.resolution == 32 {
                    (2, 1, "1/32")
                } else {
                    (1, 2, "1/8")
                };
                let wrap_name = wrapper_name.unwrap_or_else(|| format!("{} {suffix}", base.name));
                let wrapper = Pattern {
                    id: wrapper_id,
                    name: wrap_name,
                    kind: PatternKind::TimeScale {
                        source: base_id,
                        num,
                        den,
                    },
                };
                (vec![base, wrapper], wrapper_id)
            }
            _ => (vec![base], base_id),
        }
    }
}

/// On-disk schema for `presets/filter_grooves.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilterGrooveFile {
    pub version: u32,
    pub grooves: Vec<FilterEnvPreset>,
}

/// Runtime catalog of external rhythmic filter grooves (`presets/filter_grooves.json`).
///
/// Supports up to [`FILTER_ENV_PATTERN_COUNT`] (`128`) grooves indexed by explicit `id` (`0..=127`).
/// Missing IDs degrade cleanly to [`CompiledFilterPattern::neutral`] / [`FilterEnvPreset::neutral`]
/// rather than aliasing onto neighboring slots.
#[derive(Clone, Debug)]
pub struct FilterGrooveCatalog {
    grooves: Vec<FilterEnvPreset>,
    slot_to_index: [Option<u8>; FILTER_ENV_PATTERN_COUNT],
    compiled: Arc<[CompiledFilterPattern]>,
    source_path: Option<PathBuf>,
}

impl Default for FilterGrooveCatalog {
    fn default() -> Self {
        Self::empty()
    }
}

impl FilterGrooveCatalog {
    /// Empty catalog: every lookup returns [`CompiledFilterPattern::neutral`].
    pub fn empty() -> Self {
        Self {
            grooves: Vec::new(),
            slot_to_index: [None; FILTER_ENV_PATTERN_COUNT],
            compiled: Arc::from([]),
            source_path: None,
        }
    }

    pub fn from_grooves(mut grooves: Vec<FilterEnvPreset>) -> Result<Self> {
        ensure!(
            grooves.len() <= FILTER_ENV_PATTERN_COUNT,
            "Filter groove bank contains {} grooves (maximum {FILTER_ENV_PATTERN_COUNT})",
            grooves.len()
        );
        let mut seen = [false; FILTER_ENV_PATTERN_COUNT];
        for g in &grooves {
            g.validate()?;
            let id = usize::from(g.id);
            ensure!(!seen[id], "Duplicate filter groove id {}", g.id);
            seen[id] = true;
        }
        grooves.sort_by_key(|g| g.id);

        let mut slot_to_index = [None; FILTER_ENV_PATTERN_COUNT];
        let bank_len = grooves
            .last()
            .map_or(0, |last| usize::from(last.id) + 1)
            .min(FILTER_ENV_PATTERN_COUNT);
        let mut compiled = vec![CompiledFilterPattern::neutral(); bank_len];

        for (idx, g) in grooves.iter().enumerate() {
            let slot = usize::from(g.id);
            slot_to_index[slot] = Some(idx as u8);
            compiled[slot] = g.compile();
        }
        Ok(Self {
            grooves,
            slot_to_index,
            compiled: Arc::from(compiled),
            source_path: None,
        })
    }

    /// Parse and validate a `filter_grooves.json` payload directly from memory.
    pub fn parse_bytes(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() as u64 <= MAX_FILTER_GROOVES_BYTES,
            "Filter groove bank exceeds {MAX_FILTER_GROOVES_BYTES} byte limit"
        );
        let doc: FilterGrooveFile =
            serde_json::from_slice(bytes).context("Parsing filter grooves JSON")?;
        ensure!(
            doc.version == 1,
            "Unsupported filter grooves version {}",
            doc.version
        );
        Self::from_grooves(doc.grooves)
    }

    /// Validate, sort, and serialize a multi-groove bank to pretty JSON bytes with trailing newline.
    pub fn serialize_grooves(grooves: &[FilterEnvPreset]) -> Result<Vec<u8>> {
        let validated = Self::from_grooves(grooves.to_vec())?;
        let doc = FilterGrooveFile {
            version: 1,
            grooves: validated.grooves,
        };
        let mut bytes = serde_json::to_vec_pretty(&doc).context("Serializing filter grooves")?;
        bytes.push(b'\n');
        ensure!(
            bytes.len() as u64 <= MAX_FILTER_GROOVES_BYTES,
            "Serialized filter groove bank exceeds {MAX_FILTER_GROOVES_BYTES} byte limit"
        );
        Ok(bytes)
    }

    pub fn load_file(path: &Path) -> Result<Self> {
        let bytes = store::read_bounded(path, MAX_FILTER_GROOVES_BYTES)?;
        let catalog = Self::parse_bytes(&bytes)
            .with_context(|| format!("Parsing filter grooves {}", path.display()))?;
        Ok(catalog.with_source_path(path.to_path_buf()))
    }

    /// Validate and persist `grooves` atomically to `path` via temp-file + fsync + rename.
    pub fn save_file(path: &Path, grooves: &[FilterEnvPreset]) -> Result<Self> {
        let bytes = Self::serialize_grooves(grooves)?;
        store::write_atomic_bytes(path, MAX_FILTER_GROOVES_BYTES, &bytes)?;
        let catalog = Self::from_grooves(grooves.to_vec())?;
        Ok(catalog.with_source_path(path.to_path_buf()))
    }

    /// Search `roots` in priority order for `filter_grooves.json`.
    /// Never fails if the file is missing or unreadable; returns [`Self::empty`].
    pub fn load_from_roots(roots: &[PathBuf]) -> Self {
        for root in roots {
            let candidate = root.join("filter_grooves.json");
            if candidate.is_file() {
                if let Ok(catalog) = Self::load_file(&candidate) {
                    return catalog;
                }
            }
        }
        Self::empty()
    }

    /// Discover `filter_grooves.json` across default preset roots cleanly.
    pub fn discover(project_path: Option<&Path>, config: &UserConfig) -> Self {
        let roots = PresetCatalog::default_roots(&[], project_path, config);
        Self::load_from_roots(&roots)
    }

    pub fn with_source_path(mut self, path: PathBuf) -> Self {
        self.source_path = Some(path);
        self
    }

    pub fn source_path(&self) -> Option<&Path> {
        self.source_path.as_deref()
    }

    pub fn len(&self) -> usize {
        self.grooves.len()
    }

    pub fn is_empty(&self) -> bool {
        self.grooves.is_empty()
    }

    pub fn grooves(&self) -> &[FilterEnvPreset] {
        &self.grooves
    }

    /// O(1) lock-free lookup table indexed by groove `id` (`0..=max_id`), with neutral fill
    /// for sparse gaps so RT `Mixer::resolve_filter_groove(id)` is zero-alloc and O(1).
    pub fn compiled_bank(&self) -> Arc<[CompiledFilterPattern]> {
        Arc::clone(&self.compiled)
    }

    /// O(1) lookup of an explicitly defined groove by `id`.
    pub fn get(&self, id: u8) -> Option<&FilterEnvPreset> {
        let idx = self.slot_to_index.get(usize::from(id)).copied().flatten()?;
        self.grooves.get(usize::from(idx))
    }

    pub fn contains(&self, id: u8) -> bool {
        self.get(id).is_some()
    }

    /// Look up groove `id`, returning [`FilterEnvPreset::neutral`] if absent from the bank.
    pub fn preset(&self, id: u8) -> FilterEnvPreset {
        self.get(id)
            .cloned()
            .unwrap_or_else(|| FilterEnvPreset::neutral(id))
    }

    /// Look up compiled groove `id`, returning [`CompiledFilterPattern::neutral`] if absent.
    pub fn compiled(&self, id: u8) -> CompiledFilterPattern {
        self.compiled
            .get(usize::from(id))
            .copied()
            .unwrap_or_else(CompiledFilterPattern::neutral)
    }

    /// Step from `current_id` by `delta` strictly across the catalog's loaded groove IDs
    /// (clamped to first/last loaded groove; falls back to `0..=127` when catalog is empty).
    pub fn step_id(&self, current_id: u8, delta: isize) -> u8 {
        if self.grooves.is_empty() {
            return (isize::from(current_id) + delta)
                .clamp(0, (FILTER_ENV_PATTERN_COUNT - 1) as isize) as u8;
        }
        if delta == 0 {
            return current_id;
        }
        if let Some(idx) = self
            .slot_to_index
            .get(usize::from(current_id))
            .copied()
            .flatten()
        {
            let next = (isize::from(idx) + delta).clamp(0, (self.grooves.len() - 1) as isize);
            return self.grooves[next as usize].id;
        }
        // `current_id` is absent from the loaded bank (e.g. gap or higher ID from another bank):
        // step cleanly onto the neighboring loaded groove in the requested direction.
        let ins = self.grooves.partition_point(|g| g.id < current_id);
        let next = if delta > 0 {
            let offset = delta.unsigned_abs().saturating_sub(1);
            ins.saturating_add(offset).min(self.grooves.len() - 1)
        } else {
            let back = delta.unsigned_abs();
            ins.saturating_sub(back).min(self.grooves.len() - 1)
        };
        self.grooves[next].id
    }

    pub fn next_available_id(&self) -> Option<u8> {
        next_available_id(&self.grooves)
    }

    pub fn find_by_name(&self, query: &str) -> Option<u8> {
        let needle = query.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return None;
        }
        self.grooves
            .iter()
            .find(|g| g.name.to_ascii_lowercase().contains(&needle))
            .map(|g| g.id)
    }
}

/// Fixed-size zero-allocation trigger grid consumed on the real-time audio thread
/// (either copied from an external groove bank snapshot or compiled from any project `Pattern`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledFilterPattern {
    /// Steps per 4/4 bar (`8`, `16`, or `32`).
    pub resolution: u8,
    /// Active step count (`1..=64`).
    pub length: u8,
    /// Smooth sweep slew vs. snappy trigger attack.
    pub smooth_attack: bool,
    /// Velocity slots (`0` = rest/decay, `1..=127` = trigger level).
    #[serde(with = "serde_steps")]
    pub steps: [u8; MAX_FILTER_ENV_STEPS],
}

impl Default for CompiledFilterPattern {
    fn default() -> Self {
        Self::neutral()
    }
}

impl CompiledFilterPattern {
    /// Neutral 1-step pulse fallback (zero external files, zero allocations).
    pub const fn neutral() -> Self {
        let mut steps = [0u8; MAX_FILTER_ENV_STEPS];
        steps[0] = 127;
        Self {
            resolution: 16,
            length: 1,
            smooth_attack: false,
            steps,
        }
    }

    /// Resolve a groove (`0..=127`) from the process-default [`FilterGrooveCatalog`]
    /// (falling back gracefully to [`Self::neutral`] when `filter_grooves.json` is absent
    /// or the ID is missing).
    ///
    /// Non-real-time convenience helper for stateless `cadence-core` callers; `cadence-app`
    /// owns an explicit `HostIo`-loaded [`FilterGrooveCatalog`] per session, and the audio
    /// thread resolves lock-free from `Mixer::filter_grooves`.
    pub fn from_preset(id: u8) -> Self {
        with_active_catalog(|cat| cat.compiled(id))
    }

    /// Number of pattern steps per 16th-note transport step (`0.5`, `1.0`, or `2.0`).
    #[inline]
    pub fn steps_per_sixteenth(self) -> f64 {
        match self.resolution {
            8 => 0.5,
            32 => 2.0,
            _ => 1.0,
        }
    }

    /// Active slice of step velocities (`len == self.length`).
    #[inline]
    pub fn active_steps(&self) -> &[u8] {
        let len = usize::from(self.length).clamp(1, MAX_FILTER_ENV_STEPS);
        &self.steps[..len]
    }

    /// Resolve absolute pattern step, wrapped slot index, and trigger velocity (`0..=127`)
    /// at fractional 16th-note song position `song_step_16th`.
    #[inline]
    pub fn step_at(&self, song_step_16th: f64) -> (u64, usize, u8) {
        let len = usize::from(self.length).clamp(1, MAX_FILTER_ENV_STEPS);
        let pat_pos = (song_step_16th.max(0.0) * self.steps_per_sixteenth()).floor() as u64;
        let slot = (pat_pos % len as u64) as usize;
        (pat_pos, slot, self.steps[slot])
    }

    /// Lightweight deterministic preview of `(active_slot, env_level_0_to_1)` at `song_step_16th`
    /// for UI visualizers so the GPU Filter Deck matches the real-time DSP sweep exactly.
    pub fn preview_envelope(&self, song_step_16th: f64, bpm: f64, decay_ms: f32) -> (usize, f32) {
        crate::dsp::env::RhythmicEnv::eval_at(self, song_step_16th, bpm, decay_ms)
    }
}

/// Process-default catalog fallback used strictly by stateless `cadence-core` helpers
/// (`EffectParam::format_native`, `Project::compile` outside `cadence-app`, and CLI `--render`).
///
/// Interactive sessions in `cadence-app` own an isolated `FilterGrooveCatalog` loaded through
/// `HostIo` so tests (including `MemoryHostIo`) never race on or pollute global state.
static ACTIVE_GROOVES: RwLock<Option<Arc<FilterGrooveCatalog>>> = RwLock::new(None);

fn with_active_catalog<R>(f: impl FnOnce(&FilterGrooveCatalog) -> R) -> R {
    if let Ok(guard) = ACTIVE_GROOVES.read() {
        if let Some(cat) = guard.as_ref() {
            return f(cat);
        }
    }
    let discovered = Arc::new(FilterGrooveCatalog::discover(
        None,
        &UserConfig::load_default(),
    ));
    let out = f(&discovered);
    if let Ok(mut guard) = ACTIVE_GROOVES.write() {
        if guard.is_none() {
            *guard = Some(discovered);
        }
    }
    out
}

/// Load external `filter_grooves.json` from `roots` and install it as the process-default catalog.
/// Returns the number of loaded grooves (`0` if absent — never fails).
pub fn load_filter_grooves(roots: &[PathBuf]) -> usize {
    let cat = FilterGrooveCatalog::load_from_roots(roots);
    let count = cat.len();
    set_filter_grooves(cat);
    count
}

/// Install an explicit [`FilterGrooveCatalog`] as the process-default non-RT catalog.
pub fn set_filter_grooves(catalog: FilterGrooveCatalog) {
    if let Ok(mut guard) = ACTIVE_GROOVES.write() {
        *guard = Some(Arc::new(catalog));
    }
}

/// Cheap `Arc` snapshot of compiled grooves from the process-default catalog
/// (attached onto `Mixer::filter_grooves` during standalone `Project::compile`).
pub fn current_compiled_filter_grooves() -> Arc<[CompiledFilterPattern]> {
    with_active_catalog(FilterGrooveCatalog::compiled_bank)
}

/// Look up an external rhythmic filter groove (`0..=127`), falling back cleanly to a
/// neutral 1-step pulse if `presets/filter_grooves.json` is absent or the ID is missing.
pub fn filter_env_preset(id: u8) -> FilterEnvPreset {
    with_active_catalog(|cat| cat.preset(id))
}

/// Display name for a rhythmic filter groove slot (`0..=127`).
pub fn filter_env_preset_name(id: u8) -> String {
    filter_env_preset(id).name
}

/// Search loaded external filter grooves by case-insensitive name substring.
pub fn find_filter_groove_index(query: &str) -> Option<u8> {
    with_active_catalog(|cat| cat.find_by_name(query))
}

/// Step `current_id` by `delta` across loaded IDs in the process-default catalog.
pub fn step_filter_groove_id(current_id: u8, delta: isize) -> u8 {
    with_active_catalog(|cat| cat.step_id(current_id, delta))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::Project;

    #[test]
    fn missing_filter_grooves_file_falls_back_gracefully_to_neutral_pulse() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = FilterGrooveCatalog::load_from_roots(&[dir.path().to_path_buf()]);
        assert!(catalog.is_empty());
        assert_eq!(catalog.compiled(0), CompiledFilterPattern::neutral());
        assert_eq!(catalog.compiled(42), CompiledFilterPattern::neutral());
        assert_eq!(catalog.compiled(127), CompiledFilterPattern::neutral());
        let fallback = catalog.preset(12);
        assert_eq!(fallback.length, 1);
        assert_eq!(fallback.resolution, 16);
        assert_eq!(fallback.steps, vec![127]);
        assert!(fallback.name.contains("12"));
        assert_eq!(catalog.find_by_name("swell"), None);
    }

    #[test]
    fn clean_room_factory_filter_grooves_json_validates_and_never_aliases_missing_ids() {
        let path = Path::new(env!("CADENCE_WORKSPACE_DIR")).join("presets/filter_grooves.json");
        let catalog =
            FilterGrooveCatalog::load_file(&path).expect("load presets/filter_grooves.json");
        assert_eq!(catalog.len(), 24);
        assert_eq!(catalog.source_path(), Some(path.as_path()));

        let mut seen_res = [false; 3]; // 8, 16, 32
        let mut has_smooth = false;
        let mut has_snappy = false;
        let mut has_odd_len = false;

        for (idx, preset) in catalog.grooves().iter().enumerate() {
            assert_eq!(usize::from(preset.id), idx);
            assert!(matches!(preset.resolution, 8 | 16 | 32));
            match preset.resolution {
                8 => seen_res[0] = true,
                16 => seen_res[1] = true,
                32 => seen_res[2] = true,
                _ => unreachable!(),
            }
            if preset.smooth_attack {
                has_smooth = true;
            } else {
                has_snappy = true;
            }
            if preset.length % 2 == 1 {
                has_odd_len = true;
            }
            assert!((1..=MAX_FILTER_ENV_STEPS as u8).contains(&preset.length));
            assert_eq!(preset.steps.len(), usize::from(preset.length));
            assert!(preset.steps.iter().any(|&v| v > 0));
            assert!(preset.steps.iter().all(|&v| v <= 127));
            let compiled = catalog.compiled(preset.id);
            assert_eq!(compiled.length, preset.length);
            assert_eq!(compiled.resolution, preset.resolution);
            assert_eq!(compiled.smooth_attack, preset.smooth_attack);
            assert_eq!(compiled.active_steps(), preset.steps.as_slice());
        }
        assert!(seen_res.iter().all(|&b| b), "must cover 8th, 16th, 32nd");
        assert!(has_smooth && has_snappy, "must cover smooth & snappy");
        assert!(has_odd_len, "must cover odd polymetric lengths");

        // Missing IDs (e.g. 24, 54, 127) must degrade cleanly to neutral (never alias onto slot 23!).
        assert!(!catalog.contains(24));
        assert!(!catalog.contains(54));
        assert_eq!(catalog.compiled(24), CompiledFilterPattern::neutral());
        assert_eq!(catalog.compiled(54), CompiledFilterPattern::neutral());
        assert_eq!(catalog.compiled(127), CompiledFilterPattern::neutral());
        assert_eq!(catalog.preset(54), FilterEnvPreset::neutral(54));
        assert_eq!(catalog.next_available_id(), Some(24));

        // Bank-aware stepping stays strictly inside loaded groove IDs (0..=23).
        assert_eq!(catalog.step_id(0, -1), 0);
        assert_eq!(catalog.step_id(0, 1), 1);
        assert_eq!(catalog.step_id(23, 1), 23);
        assert_eq!(catalog.step_id(20, 5), 23);
        assert_eq!(catalog.step_id(54, -1), 23);
        assert_eq!(catalog.step_id(54, 1), 23);
    }

    #[test]
    fn sparse_and_large_custom_banks_load_persist_and_step_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let user_root = dir.path().join("user");
        let factory_root = dir.path().join("factory");
        std::fs::create_dir_all(&user_root).unwrap();
        std::fs::create_dir_all(&factory_root).unwrap();

        // Build a 60-slot custom bank (0..=54 + sparse slot 120) proving >55 slots work cleanly.
        let mut grooves: Vec<FilterEnvPreset> = (0u8..55)
            .map(|id| FilterEnvPreset {
                id,
                name: format!("{id:02} Custom Slot"),
                category: "External".into(),
                resolution: if id % 3 == 0 { 32 } else { 16 },
                length: 8,
                smooth_attack: id % 2 == 1,
                steps: vec![127, 0, 90, 0, 110, 0, 75, 0],
            })
            .collect();
        grooves.push(FilterEnvPreset {
            id: 120,
            name: "120 High Slot".into(),
            category: "Sparse".into(),
            resolution: 8,
            length: 4,
            smooth_attack: true,
            steps: vec![30, 80, 127, 60],
        });

        let user_path = user_root.join("filter_grooves.json");
        let saved = FilterGrooveCatalog::save_file(&user_path, &grooves).unwrap();
        assert_eq!(saved.len(), 56);
        assert_eq!(saved.next_available_id(), Some(55));
        assert_eq!(saved.compiled(54).length, 8);
        assert_eq!(saved.compiled(55), CompiledFilterPattern::neutral());
        assert_eq!(saved.compiled(119), CompiledFilterPattern::neutral());
        assert_eq!(saved.compiled(120).resolution, 8);
        assert_eq!(saved.compiled(120).active_steps(), &[30, 80, 127, 60]);

        // Sparse stepping jumps cleanly over empty gap 55..=119!
        assert_eq!(saved.step_id(54, 1), 120);
        assert_eq!(saved.step_id(120, -1), 54);
        assert_eq!(saved.step_id(80, 1), 120);
        assert_eq!(saved.step_id(80, -1), 54);

        // User root overrides factory root cleanly via load_from_roots precedence.
        let factory_src =
            Path::new(env!("CADENCE_WORKSPACE_DIR")).join("presets/filter_grooves.json");
        std::fs::copy(&factory_src, factory_root.join("filter_grooves.json")).unwrap();
        let resolved = FilterGrooveCatalog::load_from_roots(&[user_root, factory_root]);
        assert_eq!(resolved.len(), 56);
        assert_eq!(resolved.preset(54).name, "54 Custom Slot");
        assert_eq!(resolved.preset(120).name, "120 High Slot");

        // Duplicate IDs and out-of-range IDs (>= 128) are rejected cleanly.
        let mut dup = vec![FilterEnvPreset::neutral(3), FilterEnvPreset::neutral(3)];
        assert!(FilterGrooveCatalog::from_grooves(dup.clone()).is_err());
        dup.pop();
        let mut bad_id = FilterEnvPreset::neutral(128);
        assert!(bad_id.validate().is_err());
        bad_id.id = 127;
        assert!(bad_id.validate().is_ok());

        // upsert_groove replaces existing IDs and inserts new IDs sorted.
        upsert_groove(&mut grooves, FilterEnvPreset::neutral(55)).unwrap();
        assert_eq!(grooves.len(), 57);
        assert_eq!(next_available_id(&grooves), Some(56));
    }

    #[test]
    fn all_24_factory_grooves_round_trip_through_project_patterns_bit_identically() {
        let path = Path::new(env!("CADENCE_WORKSPACE_DIR")).join("presets/filter_grooves.json");
        let catalog = FilterGrooveCatalog::load_file(&path).unwrap();
        for groove in catalog.grooves() {
            let mut project = Project::empty();
            let (pats, bound_id) = groove.to_project_patterns(
                10,
                11,
                strip_groove_id_prefix(&groove.name).into(),
                None,
            );
            for pat in pats {
                project.patterns.push(pat);
            }
            project
                .validate()
                .unwrap_or_else(|e| panic!("groove {} failed validate: {e:#}", groove.name));
            let round_tripped = project
                .compile_filter_pattern(bound_id)
                .unwrap_or_else(|| panic!("groove {} failed compile_filter_pattern", groove.name));
            assert_eq!(
                round_tripped,
                groove.compile(),
                "groove '{}' (res={}, len={}, smooth={}) must round-trip identically",
                groove.name,
                groove.resolution,
                groove.length,
                groove.smooth_attack
            );

            // And converting CompiledFilterPattern back via from_compiled preserves everything!
            let rebuilt = FilterEnvPreset::from_compiled(
                groove.id,
                &groove.name,
                &groove.category,
                round_tripped,
            )
            .unwrap();
            assert_eq!(rebuilt, *groove);
        }
    }
}
