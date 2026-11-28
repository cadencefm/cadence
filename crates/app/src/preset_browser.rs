//! Unified Instrument Preset Browser (`P` on Instrument deck) with live non-destructive preview,
//! category filtering (`All` / `Synth` / `Sampler` / `Drums` / `Kit` / `Modular`), universal `/` search,
//! live audition (`a`), and single-transaction undoable commit on `Enter` / zero-trace restore on `Esc`.
use super::*;
use cadence_core::music::note_name;
use cadence_core::params::format_hz;
use cadence_core::presets::InstrumentPreset;
use cadence_core::rack::{module_sample_key, ModuleKind, RackPatch, RackTemplate};

pub const PRESET_CATEGORIES: &[&str] = &["All", "Synth", "Sampler", "Drums", "Kit", "Modular"];

#[derive(Clone, Debug)]
pub struct PresetBrowserItemView {
    pub name: String,
    pub category: String,
    pub summary: String,
    pub is_original: bool,
    pub selected: bool,
}

#[derive(Clone, Debug)]
pub struct PresetBrowserView {
    pub instrument_name: String,
    pub engine_label: &'static str,
    pub categories: Vec<(&'static str, bool)>,
    pub active_category: &'static str,
    pub query: String,
    pub searching: bool,
    pub live_preview_name: Option<String>,
    pub preview_error: Option<String>,
    pub total_count: usize,
    pub items: Vec<PresetBrowserItemView>,
    pub cursor: usize,
    pub selected_details: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct PresetBrowserItem {
    pub id: String,
    pub name: String,
    pub category: String,
    pub summary: String,
    pub preset: InstrumentPreset,
}

#[derive(Clone, Debug)]
pub(crate) struct PresetBrowserState {
    pub instrument_id: Id,
    pub instrument_name: String,
    pub original_project: Project,
    pub original_flags: LiveFlags,
    pub original_compiled: ScoreWindow,
    pub original_patch: RackPatch,
    pub original_rack_ui: RackUiState,
    pub original_dirty: bool,
    pub all_entries: Vec<PresetBrowserItem>,
    pub category_idx: usize,
    pub cursor: usize,
    pub query: String,
    pub searching: bool,
    pub pre_search_query: String,
    pub previewed_name: Option<String>,
    pub preview_error: Option<String>,
}

fn matches_preset_category(item: &PresetBrowserItem, category: &str) -> bool {
    if category == "All" {
        return true;
    }
    let cat = item.category.trim().to_ascii_lowercase();
    let patch = &item.preset.patch;
    match category {
        "Synth" => {
            matches!(
                cat.as_str(),
                "synth" | "bass" | "lead" | "pad" | "acid" | "keys" | "pluck" | "arp"
            ) || (cat != "drum"
                && cat != "drums"
                && cat != "kit"
                && cat != "sampler"
                && patch.zones.len() == 1
                && patch.zones.first().is_some_and(|z| {
                    z.modules.iter().any(|m| {
                        matches!(
                            m.kind,
                            ModuleKind::Vco(_) | ModuleKind::Aero(_)
                        ) || matches!(m.kind, ModuleKind::Modal(ref mdl) if mdl.strike_drive > 0.0)
                    })
                }))
        }
        "Sampler" => {
            cat.contains("sampler")
                || cat.contains("sample")
                || (patch.zones.len() == 1
                    && patch.zones.first().is_some_and(|z| {
                        z.modules
                            .iter()
                            .any(|m| matches!(m.kind, ModuleKind::Sampler(_)))
                    }))
        }
        "Drums" => cat.contains("drum") || cat.contains("perc"),
        "Kit" => item.preset.is_kit(),
        "Modular" => {
            cat.contains("modular")
                || cat.contains("rack")
                || patch
                    .zones
                    .iter()
                    .any(|z| z.cables.len() >= 5 || z.modules.len() >= 6)
        }
        _ => cat == category.to_ascii_lowercase(),
    }
}

impl PresetBrowserState {
    pub(crate) fn active_category(&self) -> &'static str {
        PRESET_CATEGORIES
            .get(self.category_idx)
            .copied()
            .unwrap_or("All")
    }

    pub(crate) fn visible_indices(&self) -> Vec<usize> {
        let q = self.query.trim();
        let cat = self.active_category();
        self.all_entries
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                matches_preset_category(item, cat)
                    && (q.is_empty()
                        || matches_query(
                            q,
                            &format!(
                                "{} {} {} {}",
                                item.name, item.category, item.id, item.summary
                            ),
                        ))
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub(crate) fn selected_item(&self) -> Option<&PresetBrowserItem> {
        let vis = self.visible_indices();
        vis.get(self.cursor).map(|&idx| &self.all_entries[idx])
    }

    fn clamp_cursor(&mut self) {
        let count = self.visible_indices().len();
        self.cursor = if count == 0 {
            0
        } else {
            self.cursor.min(count - 1)
        };
    }
}

fn summarize_rack_brief(patch: &RackPatch) -> String {
    if patch.zones.len() > 1 {
        let total_mods: usize = patch.zones.iter().map(|z| z.modules.len()).sum();
        return format!(
            "{} zones · {} mods · {}",
            patch.zones.len(),
            total_mods,
            format_hz(patch.cutoff_hz)
        );
    }
    let Some(zone) = patch.zones.first() else {
        return "Empty Rack".into();
    };
    let mut lead: Option<String> = None;
    for module in &zone.modules {
        match &module.kind {
            ModuleKind::Sampler(s) => {
                let src = s
                    .path
                    .as_deref()
                    .and_then(|raw| Path::new(raw).file_name().and_then(|n| n.to_str()))
                    .unwrap_or("(no sample)");
                lead = Some(format!("Sampler {src}"));
                break;
            }
            ModuleKind::Vco(v) => {
                lead = Some(v.waveform.label().to_string());
                break;
            }
            ModuleKind::Aero(a) => {
                lead = Some(format!("{} {}", a.mode.label(), a.geometry.label()));
                break;
            }
            ModuleKind::Modal(m) if lead.is_none() => {
                lead = Some(format!("Modal {}", m.geometry.label()));
            }
            _ => {}
        }
    }
    let head = lead.unwrap_or_else(|| "Modular".into());
    let filt = zone
        .modules
        .iter()
        .find_map(|m| match &m.kind {
            ModuleKind::Filter(f) => Some(format!("{} {}", f.mode.label(), format_hz(f.cutoff_hz))),
            _ => None,
        })
        .unwrap_or_else(|| format_hz(patch.cutoff_hz));
    format!(
        "{head} · {filt} · {}m/{}c",
        zone.modules.len(),
        zone.cables.len()
    )
}

fn describe_rack_details(patch: &RackPatch) -> Vec<String> {
    let mut lines = vec![format!(
        "Rack: {} zone{} · bus {:+.1} dB · pan {:+.0}% · filter {}",
        patch.zones.len(),
        if patch.zones.len() == 1 { "" } else { "s" },
        patch.gain_db,
        patch.pan * 100.0,
        format_hz(patch.cutoff_hz),
    )];
    if !patch.panel_pins.is_empty() {
        let pin_labels: Vec<&str> = patch
            .panel_pins
            .iter()
            .take(5)
            .map(|p| p.label.as_str())
            .collect();
        lines.push(format!(
            "Pins ({}): {}",
            patch.panel_pins.len(),
            pin_labels.join(" · ")
        ));
    }
    if patch.zones.len() == 1 {
        if let Some(zone) = patch.zones.first() {
            let mod_list: Vec<String> = zone
                .modules
                .iter()
                .take(8)
                .map(|m| format!("{} ({})", m.name, m.kind.tag().label()))
                .collect();
            lines.push(format!(
                "Modules ({}): {}",
                zone.modules.len(),
                mod_list.join(" · ")
            ));
            lines.push(format!(
                "Patchbay: {} cable{} · voice {} · glide {:.0}ms",
                zone.cables.len(),
                if zone.cables.len() == 1 { "" } else { "s" },
                zone.voice.voice_mode.label(),
                zone.voice.glide_ms
            ));
            for m in zone.modules.iter().take(4) {
                match &m.kind {
                    ModuleKind::Vco(v) => lines.push(format!(
                        "{}: {} · {:+}st / {:+.0}c · lvl {:.0}%",
                        m.name,
                        v.waveform.label(),
                        v.tune_semitones,
                        v.fine_cents,
                        v.level * 100.0
                    )),
                    ModuleKind::Aero(a) => lines.push(format!(
                        "{}: {} · {} · {}m · drv {:.0}%",
                        m.name,
                        a.mode.label(),
                        a.geometry.label(),
                        a.atoms,
                        a.drive * 100.0
                    )),
                    ModuleKind::Filter(f) => lines.push(format!(
                        "{}: {} · {} · res {:.0}%",
                        m.name,
                        f.mode.label(),
                        format_hz(f.cutoff_hz),
                        f.resonance * 100.0
                    )),
                    ModuleKind::Modal(mdl) => lines.push(format!(
                        "{}: {} · {}m · damp {:.3}",
                        m.name,
                        mdl.geometry.label(),
                        mdl.atoms,
                        mdl.absorption
                    )),
                    ModuleKind::Sampler(s) => lines.push(format!(
                        "{}: {} · root {}",
                        m.name,
                        s.path.as_deref().unwrap_or("(no sample)"),
                        note_name(zone.root_midi)
                    )),
                    _ => {}
                }
            }
        }
    } else {
        for zone in patch.zones.iter().take(6) {
            let tag = zone
                .modules
                .iter()
                .find(|m| {
                    matches!(
                        m.kind,
                        ModuleKind::Sampler(_)
                            | ModuleKind::Vco(_)
                            | ModuleKind::Aero(_)
                            | ModuleKind::Modal(_)
                            | ModuleKind::Noise(_)
                    )
                })
                .map_or("Modular", |m| m.kind.tag().label());
            lines.push(format!(
                "z{} {} · {}..{} · {} ({}m/{}c)",
                zone.id,
                zone.name,
                note_name(zone.low_midi),
                note_name(zone.high_midi),
                tag,
                zone.modules.len(),
                zone.cables.len()
            ));
        }
        if patch.zones.len() > 6 {
            lines.push(format!("… +{} more zones", patch.zones.len() - 6));
        }
    }
    lines
}

fn build_preset_browser_entries(
    presets: &cadence_core::presets::PresetCatalog,
) -> Vec<PresetBrowserItem> {
    let mut all_entries: Vec<PresetBrowserItem> = presets
        .entries()
        .iter()
        .map(|entry| PresetBrowserItem {
            id: entry.id.clone(),
            name: entry.name.clone(),
            category: entry.category.clone().unwrap_or_else(|| "Rack".to_string()),
            summary: summarize_rack_brief(&entry.patch),
            preset: entry.to_preset(),
        })
        .collect();
    all_entries.sort_by(|a, b| {
        a.category
            .to_ascii_lowercase()
            .cmp(&b.category.to_ascii_lowercase())
            .then_with(|| {
                a.name
                    .to_ascii_lowercase()
                    .cmp(&b.name.to_ascii_lowercase())
            })
    });

    for &template in RackTemplate::ALL {
        let bare = InstrumentPreset::bare_default(template);
        if !all_entries
            .iter()
            .any(|e| e.name.eq_ignore_ascii_case(&bare.name))
        {
            all_entries.push(PresetBrowserItem {
                id: template.label().to_ascii_lowercase(),
                name: bare.name.clone(),
                category: template.label().to_string(),
                summary: summarize_rack_brief(&bare.patch),
                preset: bare,
            });
        }
    }
    all_entries
}

impl App {
    /// Open the unified Rack Instrument Preset Browser for the focused/selected instrument.
    pub(crate) fn open_preset_browser(&mut self) -> Result<Outcome> {
        let Some(inst_id) = self.active_instrument().or(self.selected_instrument) else {
            self.status = "Select an instrument first · gi opens Instrument deck".into();
            return Ok(Outcome::default());
        };
        let inst = self
            .instrument(inst_id)
            .context("Selected instrument is missing")?;
        let instrument_name = inst.name.clone();
        let original_patch = inst.rack.clone();
        let original_project = self.doc.project().clone();
        let original_flags = self.doc.flags().clone();
        let original_compiled = self.doc.compiled().clone();
        let original_rack_ui = self.rack_ui;
        let original_dirty = self.doc.dirty();

        let all_entries = build_preset_browser_entries(&self.assets.presets);

        let initial_cursor = all_entries
            .iter()
            .position(|e| e.preset.patch == original_patch)
            .unwrap_or(0);

        self.modal = ActiveModal::PresetBrowser(Box::new(PresetBrowserState {
            instrument_id: inst_id,
            instrument_name: instrument_name.clone(),
            original_project,
            original_flags,
            original_compiled,
            original_patch,
            original_rack_ui,
            original_dirty,
            all_entries,
            category_idx: 0,
            cursor: initial_cursor,
            query: String::new(),
            searching: false,
            pre_search_query: String::new(),
            previewed_name: None,
            preview_error: None,
        }));
        self.input.count = None;
        self.input.mode = Mode::PresetBrowser;
        let out = self.apply_preset_browser_preview()?;
        self.status = format!(
            "Presets · {} (RACK) · h/l category · j/k preview · s save · R reload · A audition · Enter load · Esc restore",
            instrument_name
        );
        Ok(out)
    }

    /// Open the File Hub Save tab pre-focused on row 3 (`Save Active Instrument Preset`).
    pub(crate) fn open_save_preset_flow(&mut self) {
        self.open_file_hub(FileHubTab::Save);
        if let Some(hub) = self.modal.file_hub_mut() {
            hub.cursor = 3;
        }
        self.status =
            "Save Instrument Preset · Enter saves · e edits path · F Finder · Esc back".into();
    }

    /// Audition a musical note or focused zone on `instrument_id`.
    /// Shared SPOT used by Instrument deck (`A`/`v`/Edit `a`) and Preset Browser (`a`).
    pub(crate) fn audition_instrument_preview(&mut self, instrument_id: Id) -> Result<Outcome> {
        let Some(inst) = self.instrument(instrument_id) else {
            return Ok(Outcome::default());
        };
        if inst.rack.zones.len() > 1 {
            return self.audition_focused_zone(instrument_id);
        }
        let Some(zone) = inst.rack.zones.first() else {
            self.status = format!("{} has no zones to audition", inst.name);
            return Ok(Outcome::default());
        };
        let has_synth_src = zone.modules.iter().any(|m| {
            matches!(
                m.kind,
                ModuleKind::Vco(_) | ModuleKind::Aero(_) | ModuleKind::Noise(_)
            ) || matches!(m.kind, ModuleKind::Modal(ref mdl) if mdl.strike_drive > 0.0)
        });
        let samplers: Vec<u16> = zone
            .modules
            .iter()
            .filter_map(|m| matches!(m.kind, ModuleKind::Sampler(_)).then_some(m.id))
            .collect();
        if !samplers.is_empty() && !has_synth_src {
            let any_loaded = samplers.iter().any(|&mid| {
                self.assets
                    .samples
                    .contains_key(&module_sample_key(instrument_id, zone.id, mid))
            });
            if !any_loaded {
                self.status = format!(
                    "{} has no loaded sample · press B to open Sample Browser",
                    inst.name
                );
                return Ok(Outcome::default());
            }
        }
        let midi = if !samplers.is_empty() && !has_synth_src {
            zone.root_midi
        } else {
            self.doc.project().root_midi.clamp(36, 72)
        };
        self.status = format!("Audition {} ({})", inst.name, note_name(midi));
        Ok(Outcome {
            audition_instrument: Some((instrument_id, midi, 0.88)),
            ..Outcome::default()
        })
    }

    /// Apply the currently highlighted preset non-destructively to the live audio engine,
    /// always branching from the untouched `original_project` snapshot so zone/module
    /// pruning during preview never leaks across previews or mutates the real document.
    fn apply_preset_browser_preview(&mut self) -> Result<Outcome> {
        let Some((
            inst_id,
            original_project,
            original_flags,
            original_compiled,
            original_patch,
            chosen,
        )) = self.modal.preset_browser().map(|pb| {
            (
                pb.instrument_id,
                pb.original_project.clone(),
                pb.original_flags.clone(),
                pb.original_compiled.clone(),
                pb.original_patch.clone(),
                pb.selected_item()
                    .map(|i| (i.name.clone(), i.preset.patch.clone())),
            )
        })
        else {
            return Ok(Outcome::default());
        };
        let (target_name, target_patch) = match chosen {
            Some((name, patch)) => (Some(name), patch),
            None => (None, original_patch.clone()),
        };

        let previous = self.doc.project().clone();
        let preview_res = if target_patch == original_patch {
            self.doc
                .restore_preview_snapshot(original_project, &original_flags, original_compiled)
        } else {
            self.doc.preview_instrument_patch_from(
                &original_project,
                &original_flags,
                &original_compiled,
                inst_id,
                target_patch.clone(),
            )
        };
        match preview_res {
            Ok(()) => {
                let samples_changed =
                    document::sample_bindings_changed(&previous, self.doc.project());
                if samples_changed {
                    self.sync_loaded_samples();
                }
                self.rack_ui.zone_cursor = target_patch.clamp_zone_index(self.rack_ui.zone_cursor);
                self.clamp_rack_state(&target_patch);
                self.refresh_audio()?;
                if let Some(pb) = self.modal.preset_browser_mut() {
                    pb.preview_error = None;
                    pb.previewed_name = if target_patch == original_patch {
                        None
                    } else {
                        target_name.clone()
                    };
                }
                if let Some(name) = target_name {
                    self.status =
                        format!("Previewing '{name}' · A audition · Enter load · Esc restore");
                }
                Ok(Outcome {
                    changed: true,
                    samples_changed,
                    ..Outcome::default()
                })
            }
            Err(err) => {
                let msg = format!("{err:#}");
                if let Some(pb) = self.modal.preset_browser_mut() {
                    pb.preview_error = Some(msg.clone());
                }
                self.status = format!("Preview unavailable: {msg}");
                Ok(Outcome::default())
            }
        }
    }

    /// Cancel the Preset Browser, restoring the exact original `Project` (including all
    /// zone/module/cable modulations, parameter links, and macro bindings) + audio/sample state
    /// with ZERO undo stack entries and unchanged `dirty()` / `revision()`.
    pub(crate) fn cancel_preset_browser(&mut self) -> Result<Outcome> {
        let Some(pb) = self.modal.take_preset_browser() else {
            return Ok(Outcome::default());
        };
        let previous = self.doc.project().clone();
        self.doc.restore_preview_snapshot(
            pb.original_project,
            &pb.original_flags,
            pb.original_compiled,
        )?;
        self.rack_ui = pb.original_rack_ui;
        let samples_changed = document::sample_bindings_changed(&previous, self.doc.project());
        if samples_changed {
            self.sync_loaded_samples();
        }
        self.refresh_audio()?;
        self.input.mode = Mode::Normal;
        self.status = format!("Preset preview cancelled · restored {}", pb.instrument_name);
        Ok(Outcome {
            changed: true,
            samples_changed,
            ..Outcome::default()
        })
    }

    /// Commit the highlighted preset as ONE single clean undoable transaction.
    fn commit_preset_browser(&mut self) -> Result<Outcome> {
        let Some(pb) = self.modal.take_preset_browser() else {
            return Ok(Outcome::default());
        };
        let Some(selected) = pb.selected_item().cloned() else {
            self.modal = ActiveModal::PresetBrowser(pb);
            self.status = "No matching preset · Esc clears filter".into();
            return Ok(Outcome::default());
        };
        // Restore the exact original project snapshot silently first so the single committed
        // transaction records the full unpruned `original_project` on the undo stack cleanly.
        let prev_preview = self.doc.project().clone();
        self.doc.restore_preview_snapshot(
            pb.original_project,
            &pb.original_flags,
            pb.original_compiled,
        )?;
        self.rack_ui = pb.original_rack_ui;

        self.input.mode = Mode::Normal;
        if selected.preset.patch == pb.original_patch {
            let samples_changed =
                document::sample_bindings_changed(&prev_preview, self.doc.project());
            if samples_changed {
                self.sync_loaded_samples();
            }
            self.refresh_audio()?;
            self.status = format!("Kept current patch on {}", pb.instrument_name);
            return Ok(Outcome {
                changed: true,
                samples_changed,
                ..Outcome::default()
            });
        }

        let mut out = self.apply_loaded_instrument_preset(pb.instrument_id, selected.preset)?;
        if document::sample_bindings_changed(&prev_preview, self.doc.project()) {
            out.samples_changed = true;
        }
        Ok(out)
    }

    pub(super) fn handle_preset_browser(&mut self, key: KeyEvent) -> Result<Outcome> {
        let repeat = key.kind == KeyEventKind::Repeat;
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if key.code == KeyCode::Char('u') {
                if let Some(pb) = self.modal.preset_browser_mut() {
                    if pb.searching {
                        pb.query.clear();
                        pb.cursor = 0;
                        pb.clamp_cursor();
                        return self.apply_preset_browser_preview();
                    }
                }
            }
            return Ok(Outcome::default());
        }

        let searching = self.modal.preset_browser().is_some_and(|pb| pb.searching);
        if searching {
            match key.code {
                KeyCode::Enter | KeyCode::Tab => {
                    if let Some(pb) = self.modal.preset_browser_mut() {
                        pb.searching = false;
                        pb.clamp_cursor();
                    }
                    return self.apply_preset_browser_preview();
                }
                KeyCode::Esc => {
                    if let Some(pb) = self.modal.preset_browser_mut() {
                        pb.query = pb.pre_search_query.clone();
                        pb.searching = false;
                        pb.cursor = 0;
                        pb.clamp_cursor();
                    }
                    return self.apply_preset_browser_preview();
                }
                KeyCode::Up => {
                    if let Some(pb) = self.modal.preset_browser_mut() {
                        pb.cursor = pb.cursor.saturating_sub(1);
                    }
                    return self.apply_preset_browser_preview();
                }
                KeyCode::Down => {
                    if let Some(pb) = self.modal.preset_browser_mut() {
                        let max = pb.visible_indices().len().saturating_sub(1);
                        pb.cursor = (pb.cursor + 1).min(max);
                    }
                    return self.apply_preset_browser_preview();
                }
                KeyCode::Backspace => {
                    if let Some(pb) = self.modal.preset_browser_mut() {
                        pb.query.pop();
                        pb.cursor = 0;
                        pb.clamp_cursor();
                    }
                    return self.apply_preset_browser_preview();
                }
                KeyCode::Char(c) if !c.is_control() => {
                    if let Some(pb) = self.modal.preset_browser_mut() {
                        if pb.query.len() < 64 {
                            pb.query.push(c);
                            pb.cursor = 0;
                            pb.clamp_cursor();
                        }
                    }
                    return self.apply_preset_browser_preview();
                }
                _ => return Ok(Outcome::default()),
            }
        }

        match key.code {
            KeyCode::Esc => {
                let has_query = self
                    .modal
                    .preset_browser()
                    .is_some_and(|pb| !pb.query.is_empty());
                if has_query {
                    if let Some(pb) = self.modal.preset_browser_mut() {
                        pb.query.clear();
                        pb.cursor = 0;
                        pb.clamp_cursor();
                    }
                    return self.apply_preset_browser_preview();
                }
                self.cancel_preset_browser()
            }
            KeyCode::Char('/') if !repeat => {
                if let Some(pb) = self.modal.preset_browser_mut() {
                    pb.pre_search_query = pb.query.clone();
                    pb.searching = true;
                }
                self.status = "Filter presets · Enter applies · Esc restores".into();
                Ok(Outcome::default())
            }
            KeyCode::Char('h' | 'H') | KeyCode::Left | KeyCode::BackTab => {
                if let Some(pb) = self.modal.preset_browser_mut() {
                    let len = PRESET_CATEGORIES.len();
                    pb.category_idx = (pb.category_idx + len - 1) % len;
                    pb.cursor = 0;
                    pb.clamp_cursor();
                }
                self.apply_preset_browser_preview()
            }
            KeyCode::Char('l' | 'L') | KeyCode::Right | KeyCode::Tab => {
                if let Some(pb) = self.modal.preset_browser_mut() {
                    pb.category_idx = (pb.category_idx + 1) % PRESET_CATEGORIES.len();
                    pb.cursor = 0;
                    pb.clamp_cursor();
                }
                self.apply_preset_browser_preview()
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(pb) = self.modal.preset_browser_mut() {
                    let max = pb.visible_indices().len().saturating_sub(1);
                    pb.cursor = (pb.cursor + 1).min(max);
                }
                self.apply_preset_browser_preview()
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(pb) = self.modal.preset_browser_mut() {
                    pb.cursor = pb.cursor.saturating_sub(1);
                }
                self.apply_preset_browser_preview()
            }
            KeyCode::Char('0' | 'g') => {
                if let Some(pb) = self.modal.preset_browser_mut() {
                    pb.cursor = 0;
                }
                self.apply_preset_browser_preview()
            }
            KeyCode::Char('$' | 'G') | KeyCode::End => {
                if let Some(pb) = self.modal.preset_browser_mut() {
                    pb.cursor = pb.visible_indices().len().saturating_sub(1);
                }
                self.apply_preset_browser_preview()
            }
            KeyCode::PageUp => {
                if let Some(pb) = self.modal.preset_browser_mut() {
                    pb.cursor = pb.cursor.saturating_sub(6);
                }
                self.apply_preset_browser_preview()
            }
            KeyCode::PageDown => {
                if let Some(pb) = self.modal.preset_browser_mut() {
                    let max = pb.visible_indices().len().saturating_sub(1);
                    pb.cursor = (pb.cursor + 6).min(max);
                }
                self.apply_preset_browser_preview()
            }
            KeyCode::Char('R') if !repeat => {
                let prev_selected_id = self
                    .modal
                    .preset_browser()
                    .and_then(|pb| pb.selected_item().map(|i| i.id.clone()));
                let count = self.reload_presets_with_status();
                let entries = build_preset_browser_entries(&self.assets.presets);
                if let Some(pb) = self.modal.preset_browser_mut() {
                    pb.all_entries = entries;
                    if let Some(prev_id) = prev_selected_id {
                        let vis = pb.visible_indices();
                        if let Some(pos) = vis
                            .iter()
                            .position(|&idx| pb.all_entries[idx].id == prev_id)
                        {
                            pb.cursor = pos;
                        }
                    }
                    pb.clamp_cursor();
                }
                let out = self.apply_preset_browser_preview()?;
                self.status = format!(
                    "Reloaded {count} presets · j/k preview · s save · Enter load · Esc restore"
                );
                Ok(out)
            }
            KeyCode::Char('s') if !repeat => {
                let Some(inst_id) = self.modal.preset_browser().map(|pb| pb.instrument_id) else {
                    return Ok(Outcome::default());
                };
                // Restore the original untouched instrument patch before opening Save Preset
                // so the user saves their own Rack patch rather than a transient browse preview.
                let mut out = self.cancel_preset_browser()?;
                let wf_out = self.begin_workflow(WorkflowKind::SavePreset(inst_id))?;
                out.merge(wf_out);
                Ok(out)
            }
            KeyCode::Char('A') if !repeat => {
                let Some(inst_id) = self.modal.preset_browser().map(|pb| pb.instrument_id) else {
                    return Ok(Outcome::default());
                };
                self.audition_instrument_preview(inst_id)
            }
            KeyCode::Char(' ') if !repeat => self.apply(Action::Play {
                playing: !self.transport.playing,
            }),
            KeyCode::Enter if !repeat => self.commit_preset_browser(),
            _ => Ok(Outcome::default()),
        }
    }

    pub fn preset_browser_view(&self) -> Option<PresetBrowserView> {
        let pb = self
            .modal
            .preset_browser()
            .filter(|_| self.input.mode == Mode::PresetBrowser)?;
        let vis = pb.visible_indices();
        let items: Vec<PresetBrowserItemView> = vis
            .iter()
            .enumerate()
            .map(|(row_idx, &entry_idx)| {
                let item = &pb.all_entries[entry_idx];
                PresetBrowserItemView {
                    name: item.name.clone(),
                    category: item.category.clone(),
                    summary: item.summary.clone(),
                    is_original: item.preset.patch == pb.original_patch,
                    selected: row_idx == pb.cursor,
                }
            })
            .collect();
        let selected_details = pb
            .selected_item()
            .map(|item| describe_rack_details(&item.preset.patch))
            .unwrap_or_default();
        let categories = PRESET_CATEGORIES
            .iter()
            .enumerate()
            .map(|(idx, &label)| (label, idx == pb.category_idx))
            .collect();
        Some(PresetBrowserView {
            instrument_name: pb.instrument_name.clone(),
            engine_label: "RACK",
            categories,
            active_category: pb.active_category(),
            query: pb.query.clone(),
            searching: pb.searching,
            live_preview_name: pb.previewed_name.clone(),
            preview_error: pb.preview_error.clone(),
            total_count: pb.all_entries.len(),
            items,
            cursor: pb.cursor,
            selected_details,
        })
    }
}
