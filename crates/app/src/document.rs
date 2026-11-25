//! Loading, saving, committing edits and undo history.
use super::*;

pub fn load_document_with_io(io: &dyn HostIo, path: &Path) -> Result<(Project, LiveFlags)> {
    let bytes = io.read_bounded(path, MAX_PROJECT_BYTES)?;
    let (project, flags) = cadence_core::material::parse_document_json(&bytes)
        .context("Invalid retained-material project")?;
    project.validate()?;
    project.compile_with_flags(&flags)?;
    Ok((project, flags))
}

pub fn load_project_with_io(io: &dyn HostIo, path: &Path) -> Result<Project> {
    load_document_with_io(io, path).map(|(project, _)| project)
}

pub fn load_document(path: &Path) -> Result<(Project, LiveFlags)> {
    load_document_with_io(&CoreHostIo, path)
}

pub fn load_project(path: &Path) -> Result<Project> {
    load_document(path).map(|(project, _)| project)
}

pub(super) fn atomic_save_with_io(
    io: &dyn HostIo,
    project: &Project,
    flags: &LiveFlags,
    path: &Path,
) -> Result<()> {
    project.validate()?;
    let mut payload = Vec::new();
    cadence_core::material::write_document_json(&mut payload, project, flags)?;
    io.write_atomic(path, MAX_PROJECT_BYTES, &payload)
}

pub(super) fn absolute_parent_with_io(io: &dyn HostIo, path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let path = if parent.is_absolute() {
        parent.to_path_buf()
    } else {
        io.current_dir()?.join(parent)
    };
    Ok(io.canonicalize(&path).unwrap_or(path))
}

pub(super) struct History {
    project: Project,
    /// Saved only to restore the previous document's flags across a New/Open boundary;
    /// same-document undo and redo never touch live flags.
    boundary_flags: LiveFlags,
    asset_base: PathBuf,
    save_path: PathBuf,
    untitled: bool,
    identity: u64,
}

impl History {
    pub(super) fn identity(&self) -> u64 {
        self.identity
    }
    pub(super) fn project(&self) -> &Project {
        &self.project
    }
}

/// Which way to move through undo history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HistoryStep {
    Undo,
    Redo,
}

/// Key identifying consecutive held nudges or continuous gestures that coalesce into a single undo entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CoalesceKey {
    Param(ParamAddress),
    Zone {
        instrument: Id,
        zone_idx: usize,
        field: ZoneControl,
    },
    PartTranspose(Id),
    PartVelocity(Id),
    PlacementOffset(Id),
    DuckDepth(Id),
    DuckRecovery(Id),
    Macro(u8),
    MacroBinding {
        index: u8,
        binding: usize,
    },
    MidiCcRecord {
        target: ModulationTarget,
        pattern: Id,
    },
    SongTempo,
    SongKey,
    SongBars,
    SongTiming,
    PatternStep {
        pattern: Id,
        scope: EditScope,
        step: usize,
    },
    PatternValue {
        pattern: Id,
        step: usize,
    },
    PatternValueSlide {
        pattern: Id,
    },
    PlacementShape(Id),
    SampleSliceMove {
        instrument: Id,
        zone: u16,
        module: u16,
        index: usize,
    },
    SampleLoopXfade {
        instrument: Id,
        zone: u16,
        module: u16,
    },
}

/// The authored document: the project and its compiled score, its live flags
/// (outside undo), its identity across New and Open, its save state and undo
/// history. Every change to the project goes through `install`, `restore` or
/// `adopt`, so the revision, the object-ID high-water mark and history always agree.
pub struct Document {
    project: Project,
    flags: LiveFlags,
    compiled: ScoreWindow,
    filter_grooves_bank: Arc<[cadence_core::filter_patterns::CompiledFilterPattern]>,
    revision: u64,
    save_path: PathBuf,
    untitled: bool,
    asset_base: PathBuf,
    /// Distinguishes documents across New and Open; undo can cross them.
    identity: u64,
    identity_high_water: u64,
    /// The highest object ID ever used, so new objects never reuse one.
    high_water: Id,
    undo: Vec<History>,
    redo: Vec<History>,
    coalesce_key: Option<CoalesceKey>,
    saved: Project,
    saved_flags: LiveFlags,
    saved_asset_base: PathBuf,
}

impl Document {
    pub(super) fn new(
        project: Project,
        mut flags: LiveFlags,
        save_path: PathBuf,
        asset_base: PathBuf,
        filter_grooves_bank: Arc<[cadence_core::filter_patterns::CompiledFilterPattern]>,
    ) -> Result<Self> {
        project.prune_flags(&mut flags);
        let mut compiled = project.compile_with_flags(&flags)?;
        compiled.mixer.filter_grooves = Arc::clone(&filter_grooves_bank);
        Ok(Self {
            high_water: project.max_id(),
            saved: project.clone(),
            saved_flags: flags.clone(),
            saved_asset_base: asset_base.clone(),
            project,
            flags,
            compiled,
            filter_grooves_bank,
            revision: 0,
            save_path,
            untitled: false,
            asset_base,
            identity: 0,
            identity_high_water: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            coalesce_key: None,
        })
    }
    pub(super) fn set_filter_grooves_bank(
        &mut self,
        bank: Arc<[cadence_core::filter_patterns::CompiledFilterPattern]>,
    ) {
        self.compiled.mixer.filter_grooves = Arc::clone(&bank);
        self.filter_grooves_bank = bank;
    }
    pub fn project(&self) -> &Project {
        &self.project
    }
    pub fn flags(&self) -> &LiveFlags {
        &self.flags
    }
    pub fn compiled(&self) -> &ScoreWindow {
        &self.compiled
    }
    /// Increases with every recorded change, undo and redo.
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn save_path(&self) -> &PathBuf {
        &self.save_path
    }
    pub fn is_untitled(&self) -> bool {
        self.untitled
    }
    /// The directory relative sample paths resolve against.
    pub fn asset_base(&self) -> &PathBuf {
        &self.asset_base
    }
    pub fn dirty(&self) -> bool {
        self.project != self.saved
            || self.flags != self.saved_flags
            || self.asset_base != self.saved_asset_base
    }
    pub(super) fn identity(&self) -> u64 {
        self.identity
    }
    pub(super) fn fresh_ids(&self, count: usize) -> Result<Vec<Id>> {
        (1..=count)
            .map(|n| {
                self.high_water
                    .checked_add(n as u64)
                    .context("Object identity space exhausted")
            })
            .collect()
    }
    /// Validate a transaction against a copy of the project, taking the sound-only
    /// fast path when edits only change tempo, macro values, or instrument/mixer parameters.
    pub(super) fn prepare(&self, edits: &[Edit]) -> Result<(Project, ScoreWindow)> {
        let mut next = self.project.clone();
        let sound_only = !edits.is_empty()
            && edits.iter().all(|e| {
                matches!(
                    e,
                    Edit::SetParam { .. } | Edit::SetTempo { .. } | Edit::SetMacroValue { .. }
                )
            });
        if sound_only {
            for edit in edits {
                next.apply_edit_only(edit)?;
            }
            next.validate()?;
            let mut compiled = next.patch_score_sound(&self.compiled, &self.flags)?;
            compiled.mixer.filter_grooves = Arc::clone(&self.filter_grooves_bank);
            return Ok((next, compiled));
        }
        let mut compiled = next.apply_transaction_with_flags(edits, &self.flags)?;
        compiled.mixer.filter_grooves = Arc::clone(&self.filter_grooves_bank);
        Ok((next, compiled))
    }
    /// Update live mute/solo flags without creating an undo entry or advancing revision.
    pub(super) fn set_flags(&mut self, flags: LiveFlags) -> Result<bool> {
        self.project.validate_flags(&flags)?;
        if flags == self.flags {
            return Ok(false);
        }
        let mut compiled = if flags.muted_lanes == self.flags.muted_lanes
            && flags.solo_lanes == self.flags.solo_lanes
        {
            self.project.patch_score_sound(&self.compiled, &flags)?
        } else {
            self.project.compile_with_flags(&flags)?
        };
        compiled.mixer.filter_grooves = Arc::clone(&self.filter_grooves_bank);
        self.flags = flags;
        self.compiled = compiled;
        Ok(true)
    }
    /// Install a recorded project revision with an undo entry.
    pub(super) fn install(&mut self, project: Project, compiled: ScoreWindow) {
        self.coalesce_key = None;
        self.push_undo();
        self.revision += 1;
        self.high_water = self.high_water.max(project.max_id());
        project.prune_flags(&mut self.flags);
        self.project = project;
        self.compiled = compiled;
    }
    /// Update the current revision in place without adding a new undo entry.
    ///
    /// Required by Growing Keep (`overrides.rs::grow_active_keeps`): unlike interactive
    /// knob/MIDI coalescing (whose `CoalesceKey` is deliberately cleared on every non-edit
    /// keystroke via `break_coalesce`), a live Song-mode kept passage must keep extending
    /// bar-by-bar under its initial single undo entry while the performer navigates or jams,
    /// terminating only when `growing_keep_revision` diverges from a real document edit or
    /// the transport switches to Loop.
    pub(super) fn install_in_place(&mut self, project: Project, compiled: ScoreWindow) {
        self.coalesce_key = None;
        self.revision += 1;
        self.high_water = self.high_water.max(project.max_id());
        project.prune_flags(&mut self.flags);
        self.project = project;
        self.compiled = compiled;
    }
    /// Install an edit, coalescing consecutive held nudges sharing the same `CoalesceKey`
    /// into a single undo entry while still advancing revision.
    pub(super) fn install_coalesced(
        &mut self,
        project: Project,
        compiled: ScoreWindow,
        key: CoalesceKey,
        coalesce: bool,
    ) {
        let reuse = coalesce
            && self.coalesce_key == Some(key)
            && !self.undo.is_empty()
            && self.redo.is_empty();
        if !reuse {
            self.push_undo();
        }
        self.coalesce_key = Some(key);
        self.revision += 1;
        self.high_water = self.high_water.max(project.max_id());
        project.prune_flags(&mut self.flags);
        self.project = project;
        self.compiled = compiled;
    }
    pub(super) fn break_coalesce(&mut self) {
        self.coalesce_key = None;
    }
    /// Temporarily swap an instrument's patch starting from an untouched `base` project snapshot
    /// for non-destructive live auditioning (Preset Browser scroll preview).
    ///
    /// Always clones `base` (never a previously previewed project) so scrolling past a preset with
    /// fewer zones (such as `Blank Kit`) never cascades zone-pruning into subsequent
    /// previews or leaks into the real document. Never touches `undo`, `redo`, `saved`, `revision`,
    /// `high_water`, or `coalesce_key`.
    pub(super) fn preview_instrument_patch_from(
        &mut self,
        base: &Project,
        base_flags: &LiveFlags,
        base_compiled: &ScoreWindow,
        instrument_id: Id,
        rack: RackPatch,
    ) -> Result<()> {
        let mut next = base.clone();
        next.apply_edit_only(&Edit::UpdateRackPatch {
            id: instrument_id,
            rack,
        })?;
        next.validate()?;
        let mut compiled = if &self.flags == base_flags
            && next.modulations == base.modulations
            && next.macros == base.macros
            && next.parts == base.parts
        {
            next.patch_score_sound(base_compiled, &self.flags)?
        } else {
            next.compile_with_flags(&self.flags)?
        };
        compiled.mixer.filter_grooves = Arc::clone(&self.filter_grooves_bank);
        self.project = next;
        self.compiled = compiled;
        Ok(())
    }

    /// Temporarily switch a `Filter` insert's active groove starting from an untouched `base`
    /// project snapshot for non-destructive live Groove Browser auditioning.
    /// Never touches `undo`, `redo`, `saved`, `revision`, `high_water`, or `coalesce_key`.
    pub(super) fn preview_filter_groove_from(
        &mut self,
        base: &Project,
        base_flags: &LiveFlags,
        base_compiled: &ScoreWindow,
        device_id: Id,
        groove_id: u8,
    ) -> Result<()> {
        let mut next = base.clone();
        let mut found = false;
        for dev in next.mixer.master.inserts.iter_mut().chain(
            next.mixer
                .channels
                .iter_mut()
                .flat_map(|c| c.inserts.iter_mut()),
        ) {
            if dev.id == device_id {
                if let cadence_core::mixer::EffectKind::Filter(fp) = &mut dev.kind {
                    fp.env_pattern_id = None;
                    fp.env_pattern = groove_id;
                    found = true;
                }
                break;
            }
        }
        ensure!(found, "Filter effect #{device_id} not found");
        next.validate()?;
        let mut compiled = if &self.flags == base_flags {
            next.patch_score_sound(base_compiled, &self.flags)?
        } else {
            next.compile_with_flags(&self.flags)?
        };
        compiled.mixer.filter_grooves = Arc::clone(&self.filter_grooves_bank);
        self.project = next;
        self.compiled = compiled;
        Ok(())
    }

    /// Restore the exact pre-preview `Project` verbatim (reusing `base_compiled` when live
    /// mute/solo flags are unchanged, or recompiling under current flags otherwise) without
    /// touching `undo`, `redo`, `saved`, `revision`, `high_water`, or `coalesce_key`.
    pub(super) fn restore_preview_snapshot(
        &mut self,
        project: Project,
        base_flags: &LiveFlags,
        base_compiled: ScoreWindow,
    ) -> Result<()> {
        let mut compiled = if &self.flags == base_flags {
            base_compiled
        } else {
            project.compile_with_flags(&self.flags)?
        };
        compiled.mixer.filter_grooves = Arc::clone(&self.filter_grooves_bank);
        self.project = project;
        self.compiled = compiled;
        Ok(())
    }
    /// Snapshot the current project as an undo entry and clear redo.
    pub(super) fn push_undo(&mut self) {
        if self.undo.len() >= HISTORY_LIMIT {
            self.undo.remove(0);
        }
        self.undo.push(self.snapshot());
        self.redo.clear();
    }
    pub(super) fn can_step(&self, step: HistoryStep) -> bool {
        match step {
            HistoryStep::Undo => !self.undo.is_empty(),
            HistoryStep::Redo => !self.redo.is_empty(),
        }
    }
    /// Take the entry to restore, moving the current project onto the
    /// opposite stack.
    pub(super) fn step(&mut self, step: HistoryStep) -> Option<History> {
        self.coalesce_key = None;
        let entry = match step {
            HistoryStep::Undo => self.undo.pop(),
            HistoryStep::Redo => self.redo.pop(),
        }?;
        let current = self.snapshot();
        match step {
            HistoryStep::Undo => self.redo.push(current),
            HistoryStep::Redo => self.undo.push(current),
        }
        Some(entry)
    }
    /// Restore an undo or redo entry as a new revision. Same-document undo/redo
    /// preserves current live flags untouched.
    pub(super) fn restore(&mut self, entry: History) -> Result<()> {
        let boundary = entry.identity != self.identity;
        let mut flags = if boundary {
            entry.boundary_flags
        } else {
            self.flags.clone()
        };
        entry.project.prune_flags(&mut flags);
        let mut compiled = entry.project.compile_with_flags(&flags)?;
        compiled.mixer.filter_grooves = Arc::clone(&self.filter_grooves_bank);
        self.high_water = self.high_water.max(entry.project.max_id());
        self.project = entry.project;
        self.flags = flags;
        self.compiled = compiled;
        self.asset_base = entry.asset_base;
        self.save_path = entry.save_path;
        self.untitled = entry.untitled;
        self.identity = entry.identity;
        self.revision += 1;
        Ok(())
    }
    /// Replace the document for New or Open. The previous one stays reachable
    /// through undo.
    pub(super) fn adopt(
        &mut self,
        project: Project,
        mut flags: LiveFlags,
        compiled: ScoreWindow,
        base: PathBuf,
        path: PathBuf,
        is_new: bool,
    ) -> Result<()> {
        self.coalesce_key = None;
        let identity = self
            .identity_high_water
            .checked_add(1)
            .context("Project identity exhausted")?;
        if self.undo.len() >= HISTORY_LIMIT {
            self.undo.remove(0);
        }
        self.undo.push(self.snapshot());
        self.redo.clear();
        project.prune_flags(&mut flags);
        self.high_water = self.high_water.max(project.max_id());
        self.project = project;
        self.flags = flags;
        let mut compiled = compiled;
        compiled.mixer.filter_grooves = Arc::clone(&self.filter_grooves_bank);
        self.compiled = compiled;
        self.asset_base = base;
        self.save_path = path;
        self.untitled = is_new;
        self.identity_high_water = identity;
        self.identity = identity;
        self.revision += 1;
        if !is_new {
            self.mark_saved();
        }
        Ok(())
    }
    /// Record that the project on disk now matches, at `path` if given.
    pub(super) fn mark_saved(&mut self) {
        self.saved = self.project.clone();
        self.saved_flags = self.flags.clone();
        self.saved_asset_base = self.asset_base.clone();
        self.untitled = false;
    }
    pub(super) fn set_save_path(&mut self, path: PathBuf) {
        self.save_path = path;
        self.untitled = false;
    }
    fn snapshot(&self) -> History {
        History {
            project: self.project.clone(),
            boundary_flags: self.flags.clone(),
            asset_base: self.asset_base.clone(),
            save_path: self.save_path.clone(),
            untitled: self.untitled,
            identity: self.identity,
        }
    }
}

/// Computes a portable relative path (`./...` or `../...`) from `from_dir` to `to_file`.
pub(super) fn relative_path(from_dir: &Path, to_file: &Path) -> Option<String> {
    use std::path::Component;
    let from_comps: Vec<_> = from_dir.components().collect();
    let to_comps: Vec<_> = to_file.components().collect();
    let common = from_comps
        .iter()
        .zip(&to_comps)
        .take_while(|(a, b)| a == b)
        .count();
    if common == 0 {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    for comp in &from_comps[common..] {
        if matches!(comp, Component::Normal(_)) {
            parts.push("..".into());
        }
    }
    for comp in &to_comps[common..] {
        if let Component::Normal(s) = comp {
            parts.push(s.to_string_lossy().into_owned());
        }
    }
    if parts.is_empty() {
        return None;
    }
    let joined = parts.join("/");
    if joined.starts_with("../") {
        Some(joined)
    } else {
        Some(format!("./{joined}"))
    }
}

struct FromHereRewrite {
    edits: Vec<Edit>,
    fork_id: Option<Id>,
    status: String,
}

/// Fingerprint of every sample-backed instrument, zone, & Sampler module locator so we only
/// re-run `sync_loaded_samples()` / publish a new `SampleBank` when sample
/// paths or Sampler modules actually change.
pub(super) fn sample_bindings_changed(before: &Project, after: &Project) -> bool {
    type SampleBindingSig<'a> = (Id, &'a str, u16, &'a str, u16, Option<&'a str>);
    fn sig(project: &Project) -> Vec<SampleBindingSig<'_>> {
        let mut out = Vec::new();
        for inst in &project.instruments {
            for (zone_id, zone, mod_id, smp) in inst.rack.sampler_modules() {
                out.push((
                    inst.id,
                    inst.name.as_str(),
                    zone_id,
                    zone.name.as_str(),
                    mod_id,
                    smp.path.as_deref(),
                ));
            }
        }
        out
    }
    sig(before) != sig(after)
}

impl App {
    fn relativize_sample_locator(
        &self,
        sample_key: Id,
        locator: &str,
        parent: &Path,
    ) -> Option<String> {
        let candidate = Path::new(locator);
        let resolved = self
            .assets
            .samples
            .get(&sample_key)
            .map(|s| s.resolved_path.clone())
            .or_else(|| self.assets.library.resolve_path(locator))
            .or_else(|| {
                if locator.starts_with("./") || locator.starts_with("../") {
                    Some(self.doc.asset_base().join(candidate))
                } else {
                    None
                }
            })?;
        let resolved = self.io.canonicalize(&resolved).unwrap_or(resolved);
        let explicit_relative = locator.starts_with("./") || locator.starts_with("../");
        if let Ok(relative) = resolved.strip_prefix(parent) {
            Some(format!(
                "./{}",
                relative.to_string_lossy().replace('\\', "/")
            ))
        } else if !explicit_relative {
            if let Some(label) = self.assets.library.catalog_label_for(&resolved) {
                Some(label)
            } else {
                relative_path(parent, &resolved)
            }
        } else {
            relative_path(parent, &resolved)
        }
    }

    pub fn save_project(&self, path: &Path) -> Result<()> {
        let parent = absolute_parent_with_io(self.io.as_ref(), path)?;
        let mut saved = self.doc.project().clone();
        for instrument in &mut saved.instruments {
            let inst_id = instrument.id;
            for (zone_id, mod_id, smp) in instrument.rack.sampler_modules_mut() {
                if let Some(locator) = smp.path.as_deref() {
                    let key = module_sample_key(inst_id, zone_id, mod_id);
                    if let Some(next) = self.relativize_sample_locator(key, locator, &parent) {
                        smp.path = Some(next);
                    }
                }
            }
        }
        atomic_save_with_io(self.io.as_ref(), &saved, self.doc.flags(), path)
    }
    pub(super) fn commit(&mut self, edits: Vec<Edit>, label: String) -> Result<Outcome> {
        if self.from_here {
            self.from_here = false;
            if let Some(rewrite) = self.rewrite_edits_from_here(&edits)? {
                let out = self.commit_inner(
                    rewrite.edits,
                    format!("{label}{}", rewrite.status),
                    None,
                    None,
                )?;
                if let Some(id) = rewrite.fork_id {
                    self.selected_pattern = Some(id);
                }
                return Ok(out);
            }
        }
        self.commit_inner(edits, label, None, None)
    }

    /// Rewrite a single-pattern edit when `'` ("from here") is armed: forks the edited
    /// pattern (plus any derived patterns & parts that depend on it), redirects the edit
    /// onto the fork, and splits/retargets placements from the boundary bar onward so
    /// everything before `from_step` stays untouched — all in one undoable transaction.
    fn rewrite_edits_from_here(&self, edits: &[Edit]) -> Result<Option<FromHereRewrite>> {
        if edits.len() != 1 {
            return Ok(None);
        }
        let target_pat = match &edits[0] {
            Edit::SetNote { pattern, .. }
            | Edit::SetRhythm { pattern, .. }
            | Edit::SetValue { pattern, .. }
            | Edit::ResizePattern { pattern, .. } => *pattern,
            Edit::PutPattern { pattern } if self.pattern(pattern.id).is_some() => pattern.id,
            _ => return Ok(None),
        };
        let project = self.doc.project();
        let Some(orig_pat) = project.pattern(target_pat) else {
            return Ok(None);
        };
        let from_step = if self.transport.playing {
            let at = self
                .live
                .boundary(project, project.timing.launch)
                .unwrap_or(self.live.next_step);
            (self.song_position(at) / 16 * 16) as u32
        } else {
            self.arrange_ui.step / 16 * 16
        };
        let bar_num = from_step / 16 + 1;
        let mut next_id = self.doc.fresh_ids(1)?[0];
        let mut pat_map: HashMap<Id, Id> = HashMap::new();
        let mut new_edits: Vec<Edit> = Vec::new();

        // 1. Fork the target pattern itself, then apply the incoming edit to the fork.
        let forked_root = next_id;
        next_id += 1;
        let mut root_copy = orig_pat.clone();
        root_copy.id = forked_root;
        root_copy.name = crate::unique_name(
            &format!("{} B{bar_num}", orig_pat.name),
            project.patterns.iter().map(|p| p.name.as_str()),
            None,
        );
        pat_map.insert(target_pat, forked_root);
        let redirected = match &edits[0] {
            Edit::SetNote { step, value, .. } => Edit::SetNote {
                pattern: forked_root,
                step: *step,
                value: value.clone(),
            },
            Edit::SetRhythm { step, active, .. } => Edit::SetRhythm {
                pattern: forked_root,
                step: *step,
                active: *active,
            },
            Edit::SetValue { step, value, .. } => Edit::SetValue {
                pattern: forked_root,
                step: *step,
                value: *value,
            },
            Edit::ResizePattern { length, .. } => Edit::ResizePattern {
                pattern: forked_root,
                length: *length,
            },
            Edit::PutPattern { pattern } => {
                let mut updated = pattern.clone();
                updated.id = forked_root;
                updated.name = root_copy.name.clone();
                Edit::PutPattern { pattern: updated }
            }
            _ => unreachable!(),
        };
        new_edits.push(Edit::PutPattern { pattern: root_copy });
        new_edits.push(redirected);

        // 2. Transitively fork any derived patterns whose source(s) were forked.
        let mut existing_pat_names: Vec<String> =
            project.patterns.iter().map(|p| p.name.clone()).collect();
        existing_pat_names.push(format!("{} B{bar_num}", orig_pat.name));
        let mut changed = true;
        while changed {
            changed = false;
            for pat in &project.patterns {
                if pat_map.contains_key(&pat.id) {
                    continue;
                }
                if pat.kind.sources().iter().any(|s| pat_map.contains_key(s)) {
                    let fid = next_id;
                    next_id += 1;
                    let mut forked = pat.clone();
                    forked.id = fid;
                    forked.name = crate::unique_name(
                        &format!("{} B{bar_num}", pat.name),
                        existing_pat_names.iter().map(String::as_str),
                        None,
                    );
                    existing_pat_names.push(forked.name.clone());
                    forked.kind = remap_pattern_sources(&pat.kind, &pat_map);
                    pat_map.insert(pat.id, fid);
                    new_edits.push(Edit::PutPattern { pattern: forked });
                    changed = true;
                }
            }
        }

        // 3. Fork any Parts pointing to a forked pattern (or parameter link).
        let mut part_map: HashMap<Id, Id> = HashMap::new();
        let mut existing_part_names: Vec<String> =
            project.parts.iter().map(|p| p.name.clone()).collect();
        for part in &project.parts {
            let pat_changed = pat_map.contains_key(&part.pattern);
            let links_changed = part
                .parameter_links
                .iter()
                .any(|l| pat_map.contains_key(&l.pattern));
            if pat_changed || links_changed {
                let fid = next_id;
                next_id += 1;
                let mut forked = part.clone();
                forked.id = fid;
                forked.name = crate::unique_name(
                    &format!("{} B{bar_num}", part.name),
                    existing_part_names.iter().map(String::as_str),
                    None,
                );
                existing_part_names.push(forked.name.clone());
                if let Some(&np) = pat_map.get(&part.pattern) {
                    forked.pattern = np;
                }
                for link in &mut forked.parameter_links {
                    if let Some(&np) = pat_map.get(&link.pattern) {
                        link.pattern = np;
                    }
                }
                part_map.insert(part.id, fid);
                new_edits.push(Edit::PutPart { part: forked });
            }
        }

        // 4. Split/retarget placements on every lane from `from_step` onward.
        for lane in &project.lanes {
            let mut updated_placements = Vec::with_capacity(lane.placements.len() + 2);
            let mut lane_touched = false;
            for pl in &lane.placements {
                let Some(&forked_part) = part_map.get(&pl.part) else {
                    updated_placements.push(pl.clone());
                    continue;
                };
                let start = pl.start_step;
                let end = pl.start_step + pl.length_steps;
                if end <= from_step {
                    updated_placements.push(pl.clone());
                } else if start >= from_step {
                    let mut moved = pl.clone();
                    moved.part = forked_part;
                    updated_placements.push(moved);
                    lane_touched = true;
                } else {
                    // Straddles `from_step`: head keeps original, tail plays forked part.
                    let head_len = from_step - start;
                    let tail_len = end - from_step;
                    let mut head = pl.clone();
                    head.length_steps = head_len;
                    head.exceptions.retain(|e| e.step < head_len);
                    let mut tail = pl.clone();
                    tail.id = next_id;
                    next_id += 1;
                    tail.part = forked_part;
                    tail.start_step = from_step;
                    tail.length_steps = tail_len;
                    if tail.phase == PhaseMode::Restart {
                        tail.offset_steps += head_len as i32;
                    }
                    tail.exceptions = pl
                        .exceptions
                        .iter()
                        .filter(|e| e.step >= head_len)
                        .map(|e| cadence_core::material::Exception {
                            step: e.step - head_len,
                            value: e.value.clone(),
                        })
                        .collect();
                    updated_placements.push(head);
                    updated_placements.push(tail);
                    lane_touched = true;
                }
            }
            if lane_touched {
                let mut next_lane = lane.clone();
                next_lane.placements = updated_placements;
                new_edits.push(Edit::PutLane { lane: next_lane });
            }
        }

        Ok(Some(FromHereRewrite {
            edits: new_edits,
            fork_id: Some(forked_root),
            status: format!(" · forked from bar {bar_num}"),
        }))
    }
    pub(super) fn commit_param(
        &mut self,
        address: ParamAddress,
        value: f32,
        label: String,
        coalesce: bool,
    ) -> Result<Outcome> {
        let focused_mixer_send = if let ParamAddress::Send {
            channel: ChannelRef::Channel(ch_id),
            target,
        } = address
        {
            if self.selected_channel == ChannelRef::Channel(ch_id) {
                if let MixerSlot::Send(idx) = self.view_state.mixer_slot {
                    self.mixer_focused_send(idx)
                        .filter(|&(c, t)| c == ch_id && t == target)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        let focused_detail_send = matches!(address, ParamAddress::Send { .. })
            && matches!(self.detail, Detail::Channel(_))
            && self
                .control_descriptors()
                .get(self.control_cursor)
                .is_some_and(|c| matches!(c.target, ControlTarget::Param(a) if a == address));

        let out = self.commit_inner(
            vec![Edit::SetParam { address, value }],
            label,
            Some((CoalesceKey::Param(address), coalesce)),
            None,
        )?;

        if let Some((ch_id, target)) = focused_mixer_send {
            if let Some(new_idx) = self
                .channel_send_targets(ch_id)
                .iter()
                .position(|&(t, _)| t == target)
            {
                self.view_state.mixer_slot = MixerSlot::Send(new_idx);
            }
        }
        if focused_detail_send {
            if let Some(new_idx) = self
                .control_descriptors()
                .iter()
                .position(|c| matches!(c.target, ControlTarget::Param(a) if a == address))
            {
                self.control_cursor = new_idx;
            }
        }
        Ok(out)
    }
    pub(super) fn commit_coalesced(
        &mut self,
        edits: Vec<Edit>,
        label: String,
        key: CoalesceKey,
        coalesce: bool,
    ) -> Result<Outcome> {
        if self.from_here {
            return self.commit(edits, label);
        }
        self.commit_inner(edits, label, Some((key, coalesce)), None)
    }
    /// Coalesced commit that lands immediately on `Boundary::Now` (for live MIDI CC recording).
    pub(super) fn commit_coalesced_immediate(
        &mut self,
        edits: Vec<Edit>,
        label: String,
        key: CoalesceKey,
        coalesce: bool,
    ) -> Result<Outcome> {
        self.commit_inner(
            edits,
            label,
            Some((key, coalesce)),
            Some(cadence_core::material::Boundary::Now),
        )
    }
    fn commit_inner(
        &mut self,
        edits: Vec<Edit>,
        label: String,
        coalesce_key: Option<(CoalesceKey, bool)>,
        live_boundary: Option<cadence_core::material::Boundary>,
    ) -> Result<Outcome> {
        let previous = self.doc.project().clone();
        let (next, compiled) = self.doc.prepare(&edits)?;
        if follow::material_changed(self.doc.project(), &next) || follow::points_edited(&edits) {
            self.suspend_follow();
        }
        if &next == self.doc.project() {
            self.status = label;
            return Ok(Outcome::default());
        }
        let samples_changed = sample_bindings_changed(&previous, &next);
        if let Some((key, coalesce)) = coalesce_key {
            self.doc.install_coalesced(next, compiled, key, coalesce);
        } else {
            self.doc.install(next, compiled);
        }
        self.last_edit = Some(edits.clone());
        if samples_changed {
            self.sync_loaded_samples();
        }
        self.normalize_focus();
        if let Some(boundary) = live_boundary {
            self.live_song_changed(&previous, Some(edits), boundary, &label)?;
        } else {
            self.live_after_commit(&previous, Some(edits), &label)?;
        }
        self.status = label;
        let mut out = Outcome {
            changed: true,
            samples_changed,
            ..Outcome::default()
        };
        self.validate_active_audition(&mut out)?;
        Ok(out)
    }
    /// Seal any active take journal before switching documents, stopping cleanly, or exiting.
    pub(super) fn finish_active_take(&mut self) -> Result<()> {
        if let Some(mut journal) = self.takes.active.take() {
            let path = journal.path.clone();
            let landed = self.live.land_due();
            journal.record(&landed);
            if let Some(sealed) = journal.seal(self.live.next_step)? {
                self.takes.last = Some(sealed);
                self.takes.last_sealed_path = path;
                if let Some(dir) = &self.takes.session_dir {
                    let _ = self
                        .io
                        .prune_takes_dir(dir, cadence_core::store::DEFAULT_UNKEPT_TAKES);
                }
            }
        }
        Ok(())
    }

    /// One shutdown path run on every clean exit: seals an active take,
    /// releases held keyboard and MIDI notes, and stops transport/audition.
    pub fn shutdown(&mut self) -> Outcome {
        if let Err(error) = self.finish_active_take() {
            self.status = format!("Take save error on exit: {error:#}");
        }
        let mut out = self.release_held();
        out.live_notes_off
            .extend(self.release_midi().live_notes_off);
        self.transport.playing = false;
        self.live.playing = false;
        out.stop_audition = true;
        out.transport.push(TransportIntent::Reset { step: 0 });
        out
    }

    /// Reset all document-scoped session, performance and transport state
    /// after New, Open or cross-document undo/redo.
    fn reset_session_for_document(&mut self, invalidate_cache: bool) -> Result<Outcome> {
        self.follow.clear_anchors();
        self.arrange_ui.viewport_start_step.set(0);
        self.view_state.mixer_scroll.set(0);
        self.view_state.mixer_detail_scroll.set(0);
        self.view_state.mixer_detail_channel.set(None);
        self.sample_ui = SampleDeckUiState::default();
        self.arrange_ui.event = None;
        self.arrange_ui.pitch_anchor = None;
        self.arrange_ui.pitch_lane = None;
        self.arrange_ui.source_cursor = 0;
        self.assets
            .library
            .set_project_path(&self.doc.asset_base().join("project.json"));
        if invalidate_cache {
            self.assets.library.invalidate_decoded_cache();
        }
        self.sync_loaded_samples();
        self.live = cadence_core::live::Performance::new_with_flags(
            self.doc.project(),
            self.doc.flags().clone(),
        );
        self.staging = None;
        self.from_here = false;
        self.takes.growing_keeps.clear();
        self.takes.growing_keep_revision = None;
        self.transport = Transport::default();
        self.scope = EditScope::Definition;
        self.reset_layout();
        self.back.clear();
        self.dismiss_transient_modals();
        self.input.mode = Mode::Normal;
        self.input.lane_layer = None;
        self.input.focused_lane = None;
        self.query.clear();
        self.normalize_focus();
        self.refresh_audio()?;
        let mut out = self.release_held();
        out.live_notes_off
            .extend(self.release_midi().live_notes_off);
        out.changed = true;
        out.samples_changed = true;
        out.project_boundary = true;
        out.stop_audition = true;
        out.transport.push(TransportIntent::Reset { step: 0 });
        Ok(out)
    }

    /// Undo or redo one transaction.
    pub(super) fn step_history(&mut self, step: HistoryStep) -> Result<Outcome> {
        self.takes.growing_keeps.clear();
        self.takes.growing_keep_revision = None;
        if !self.doc.can_step(step) {
            self.status = match step {
                HistoryStep::Undo => "Nothing to undo",
                HistoryStep::Redo => "Nothing to redo",
            }
            .into();
            return Ok(Outcome::default());
        }
        let entry = self.doc.step(step).context("History entry missing")?;
        let boundary = entry.identity() != self.doc.identity();
        let cancel_rec = if boundary && self.recording.is_active() {
            self.cancel_audio_recording()?
        } else {
            Outcome::default()
        };
        if boundary {
            self.finish_active_take()?;
        }
        let (boundary_out, samples_changed) = self.restore_history(entry)?;
        self.status = match step {
            HistoryStep::Undo => "Undid one musical transaction",
            HistoryStep::Redo => "Redid one musical transaction",
        }
        .into();
        if let Some(mut replaced) = boundary_out {
            if cancel_rec.recording_command.is_some() {
                replaced.recording_command = cancel_rec.recording_command;
            }
            return Ok(replaced);
        }
        let mut out = self.release_held();
        out.changed = true;
        out.samples_changed = samples_changed;
        self.validate_active_audition(&mut out)?;
        Ok(out)
    }
    pub(super) fn restore_history(&mut self, entry: History) -> Result<(Option<Outcome>, bool)> {
        let boundary = entry.identity() != self.doc.identity();
        let previous = (!boundary).then(|| self.doc.project().clone());
        if !boundary && follow::material_changed(self.doc.project(), entry.project()) {
            self.suspend_follow();
        }
        self.doc.restore(entry)?;
        if boundary {
            return Ok((Some(self.reset_session_for_document(false)?), true));
        }
        self.assets
            .library
            .set_project_path(&self.doc.asset_base().join("project.json"));
        let samples_changed = previous
            .as_ref()
            .is_none_or(|prev| sample_bindings_changed(prev, self.doc.project()));
        if samples_changed {
            self.sync_loaded_samples();
        }
        if let Some(previous) = previous {
            let edits = cadence_core::take::diff_project_edits(&previous, self.doc.project());
            self.live_after_commit(&previous, Some(edits), "Undo/redo")?;
        }
        self.normalize_focus();
        Ok((None, samples_changed))
    }
    /// Install a crash-recovery snapshot on top of the on-disk document without
    /// modifying the saved file. The document remains dirty and `u` reverts to disk.
    pub fn restore_recovered_snapshot(&mut self, project: Project, flags: LiveFlags) -> Result<()> {
        let compiled = project.compile_with_flags(&flags)?;
        self.doc.install(project, compiled);
        self.doc.set_flags(flags)?;
        self.sync_loaded_samples();
        self.live.reset_heard(self.doc.project(), self.doc.flags());
        self.normalize_focus();
        self.refresh_audio()?;
        self.status = "Recovered unsaved changes · Ctrl+s saves to disk · u reverts".into();
        Ok(())
    }

    pub(super) fn replace_document(
        &mut self,
        project: Project,
        flags: LiveFlags,
        base: PathBuf,
        path: PathBuf,
        is_new: bool,
    ) -> Result<Outcome> {
        let cancel_rec = if self.recording.is_active() {
            self.cancel_audio_recording()?
        } else {
            Outcome::default()
        };
        self.finish_active_take()?;
        let compiled = project.compile_with_flags(&flags)?;
        self.doc
            .adopt(project, flags, compiled, base, path, is_new)?;
        if !self.assets.custom_presets {
            self.assets.presets =
                self.io
                    .discover_presets(&[], Some(self.doc.save_path()), &self.config);
            let groove_roots = self.filter_groove_search_roots();
            let grooves = self.io.load_filter_grooves(&groove_roots);
            self.assets.filter_grooves = grooves;
            self.doc
                .set_filter_grooves_bank(self.assets.filter_grooves.compiled_bank());
        }
        let mut out = self.reset_session_for_document(true)?;
        if cancel_rec.recording_command.is_some() {
            out.recording_command = cancel_rec.recording_command;
        }
        self.status = if is_new {
            "New material workspace"
        } else {
            "Opened retained definitions and relationships"
        }
        .into();
        Ok(out)
    }
}

fn remap_pattern_sources(kind: &PatternKind, map: &HashMap<Id, Id>) -> PatternKind {
    kind.remap_sources(|id| map.get(&id).copied().unwrap_or(id))
}
