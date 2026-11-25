//! Filter Groove Browser (`P` on Filter Effect deck / `Enter` on `env_pattern`),
//! Fork-to-Pattern (`f`), and Save-to-Bank (`s`) workflows.
//!
//! Mirrors Cadence's Instrument Preset Browser UX: `/` live search, category &
//! resolution/length/step-bar + coupled-ODE envelope preview, non-destructive live audio
//! preview while scrolling, single-transaction undoable commit on `Enter`, and zero-trace
//! restore on `Esc`.
use super::*;
use cadence_core::dsp::env::RhythmicEnv;
use cadence_core::filter_patterns::{
    format_groove_name_with_id, next_available_id, strip_groove_id_prefix, upsert_groove,
    CompiledFilterPattern, FilterEnvPreset, FilterGrooveCatalog, FILTER_ENV_PATTERN_COUNT,
    MAX_FILTER_GROOVES_BYTES,
};
use cadence_core::mixer::EffectKind;

const GROOVE_BROWSER_PREVIEW_PPS: usize = 6;

#[derive(Clone, Debug)]
pub struct GrooveBrowserItemView {
    pub id: u8,
    pub name: String,
    pub category: String,
    pub resolution: u8,
    pub length: u8,
    pub smooth_attack: bool,
    pub summary: String,
    pub is_original: bool,
    pub selected: bool,
}

#[derive(Clone, Debug)]
pub struct GrooveBrowserPreview {
    pub id: u8,
    pub name: String,
    pub category: String,
    pub resolution: u8,
    pub length: u8,
    pub smooth_attack: bool,
    pub steps: Vec<f32>,
    pub env_curve: Vec<f32>,
    pub active_hits: usize,
    pub peak_velocity: u8,
    pub cycle_label: String,
    pub env_decay_ms: f32,
    pub env_amount: f32,
}

#[derive(Clone, Debug)]
pub struct GrooveBrowserView {
    pub device_name: String,
    pub channel_name: String,
    pub bank_source: String,
    pub query: String,
    pub searching: bool,
    pub live_preview_name: Option<String>,
    pub preview_error: Option<String>,
    pub total_count: usize,
    pub items: Vec<GrooveBrowserItemView>,
    pub cursor: usize,
    pub selected_preview: Option<GrooveBrowserPreview>,
}

#[derive(Clone, Debug)]
pub(crate) struct GrooveBrowserState {
    pub device_id: Id,
    pub device_name: String,
    pub channel_name: String,
    pub original_project: Project,
    pub original_flags: LiveFlags,
    pub original_compiled: ScoreWindow,
    pub original_env_pattern: u8,
    pub original_env_pattern_id: Option<Id>,
    pub original_dirty: bool,
    pub all_entries: Vec<FilterEnvPreset>,
    pub cursor: usize,
    pub query: String,
    pub searching: bool,
    pub pre_search_query: String,
    pub previewed_name: Option<String>,
    pub preview_error: Option<String>,
}

impl GrooveBrowserState {
    pub(crate) fn visible_indices(&self) -> Vec<usize> {
        let q = self.query.trim();
        self.all_entries
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                if q.is_empty() {
                    return true;
                }
                let contour = if item.smooth_attack {
                    "smooth wah ramp"
                } else {
                    "punch snappy gate"
                };
                matches_query(
                    q,
                    &format!(
                        "{:02} {} {} 1/{} {}st {}",
                        item.id, item.name, item.category, item.resolution, item.length, contour
                    ),
                )
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub(crate) fn selected_item(&self) -> Option<&FilterEnvPreset> {
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

fn summarize_groove_brief(g: &FilterEnvPreset) -> String {
    let atk = if g.smooth_attack { "Smooth" } else { "Punch" };
    format!("1/{} · {}st · {atk}", g.resolution, g.length)
}

fn format_cycle_duration(resolution: u8, length: u8) -> String {
    let len = u32::from(length.max(1));
    let res = u32::from(match resolution {
        8 | 16 | 32 => resolution,
        _ => 16,
    });
    if len.is_multiple_of(res) {
        let bars = len / res;
        format!("{bars} bar{}", if bars == 1 { "" } else { "s" })
    } else if (len * 4).is_multiple_of(res) {
        let beats = (len * 4) / res;
        format!("{beats} beat{}", if beats == 1 { "" } else { "s" })
    } else {
        format!("{len}/{res} bar")
    }
}

fn unique_pattern_name(project: &Project, preferred: &str, reserved: &[String]) -> String {
    let base = preferred.trim();
    let base = if base.is_empty() {
        "Filter Groove"
    } else {
        base
    };
    let taken = |cand: &str| {
        project
            .patterns
            .iter()
            .any(|p| p.name.eq_ignore_ascii_case(cand))
            || reserved.iter().any(|r| r.eq_ignore_ascii_case(cand))
    };
    if !taken(base) {
        return base.to_string();
    }
    for n in 2..=9999 {
        let cand = format!("{base} {n}");
        if !taken(&cand) {
            return cand;
        }
    }
    format!("{base} Copy")
}

fn parse_explicit_slot_prefix(raw: &str) -> Option<(u8, &str)> {
    let trimmed = raw.trim();
    let (head, tail) = trimmed.split_once(char::is_whitespace)?;
    let tail = tail.trim_start();
    if head.is_empty() || tail.is_empty() || !head.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let id = head.parse::<u8>().ok()?;
    (usize::from(id) < FILTER_ENV_PATTERN_COUNT).then_some((id, tail))
}

impl App {
    /// Whether the focused detail deck is a rhythmic `Filter` insert effect.
    pub(crate) fn is_filter_effect_detail(&self) -> bool {
        if !self.deck_focused() {
            return false;
        }
        let Detail::Effect(id) = self.detail else {
            return false;
        };
        self.effect(id)
            .is_some_and(|(_, d)| matches!(d.kind, EffectKind::Filter(_)))
    }

    /// Resolve the most relevant `Filter` effect device: focused Effect deck →
    /// first Filter on `selected_channel` → first Filter anywhere in the mixer.
    pub(crate) fn active_filter_device(&self) -> Option<Id> {
        if let Detail::Effect(id) = self.detail {
            if self
                .effect(id)
                .is_some_and(|(_, d)| matches!(d.kind, EffectKind::Filter(_)))
            {
                return Some(id);
            }
        }
        let p = self.doc.project();
        let on_selected = match self.selected_channel {
            ChannelRef::Master => p.mixer.master.inserts.as_slice(),
            ChannelRef::Channel(cid) => self
                .channel(cid)
                .map_or(&[][..], |ch| ch.inserts.as_slice()),
        };
        if let Some(dev) = on_selected
            .iter()
            .find(|d| matches!(d.kind, EffectKind::Filter(_)))
        {
            return Some(dev.id);
        }
        p.mixer
            .channels
            .iter()
            .flat_map(|c| c.inserts.iter())
            .chain(p.mixer.master.inserts.iter())
            .find(|d| matches!(d.kind, EffectKind::Filter(_)))
            .map(|d| d.id)
    }

    /// Open the Filter Groove Browser for the active/focused Filter effect.
    pub(crate) fn open_groove_browser(&mut self) -> Result<Outcome> {
        let Some(device_id) = self.active_filter_device() else {
            self.status = "No Filter effect in project · add one with nf → Filter".into();
            return Ok(Outcome::default());
        };
        self.open_groove_browser_for(device_id)
    }

    /// Open the Filter Groove Browser targeting a specific `Filter` device ID.
    pub(crate) fn open_groove_browser_for(&mut self, device_id: Id) -> Result<Outcome> {
        let Some((owner, dev)) = self.effect(device_id) else {
            self.status = "Filter effect is missing".into();
            return Ok(Outcome::default());
        };
        let EffectKind::Filter(patch) = dev.kind else {
            self.status = format!("{} is not a Filter effect", dev.name);
            return Ok(Outcome::default());
        };
        let device_name = dev.name.clone();
        let channel_name = self.channel_name(owner).to_string();
        let original_env_pattern = patch.env_pattern;
        let original_env_pattern_id = patch.env_pattern_id;
        let original_project = self.doc.project().clone();
        let original_flags = self.doc.flags().clone();
        let original_compiled = self.doc.compiled().clone();
        let original_dirty = self.doc.dirty();

        let all_entries: Vec<FilterEnvPreset> = self.assets.filter_grooves.grooves().to_vec();
        let initial_cursor = all_entries
            .iter()
            .position(|g| g.id == original_env_pattern)
            .unwrap_or(0);

        self.modal = ActiveModal::GrooveBrowser(Box::new(GrooveBrowserState {
            device_id,
            device_name: device_name.clone(),
            channel_name,
            original_project,
            original_flags,
            original_compiled,
            original_env_pattern,
            original_env_pattern_id,
            original_dirty,
            all_entries,
            cursor: initial_cursor,
            query: String::new(),
            searching: false,
            pre_search_query: String::new(),
            previewed_name: None,
            preview_error: None,
        }));
        self.input.count = None;
        self.input.mode = Mode::GrooveBrowser;
        let out = self.apply_groove_browser_preview()?;
        self.status = format!(
            "Filter Grooves · {device_name} · j/k live preview · / filter · f fork · s save · o bank · Enter load · Esc restore"
        );
        Ok(out)
    }

    fn apply_groove_browser_preview(&mut self) -> Result<Outcome> {
        let Some((
            device_id,
            original_project,
            original_flags,
            original_compiled,
            original_env_pattern,
            original_env_pattern_id,
            chosen,
        )) = self.modal.groove_browser().map(|gb| {
            (
                gb.device_id,
                gb.original_project.clone(),
                gb.original_flags.clone(),
                gb.original_compiled.clone(),
                gb.original_env_pattern,
                gb.original_env_pattern_id,
                gb.selected_item().map(|g| (g.id, g.name.clone())),
            )
        })
        else {
            return Ok(Outcome::default());
        };

        let is_original = chosen
            .as_ref()
            .is_none_or(|(id, _)| *id == original_env_pattern && original_env_pattern_id.is_none());

        let preview_res = if let Some((target_id, _)) = chosen.as_ref().filter(|_| !is_original) {
            self.doc.preview_filter_groove_from(
                &original_project,
                &original_flags,
                &original_compiled,
                device_id,
                *target_id,
            )
        } else {
            self.doc
                .restore_preview_snapshot(original_project, &original_flags, original_compiled)
        };

        match preview_res {
            Ok(()) => {
                self.refresh_audio()?;
                if let Some(gb) = self.modal.groove_browser_mut() {
                    gb.preview_error = None;
                    gb.previewed_name = if is_original {
                        None
                    } else {
                        chosen.as_ref().map(|(_, name)| name.clone())
                    };
                }
                if let Some((_, name)) = chosen {
                    self.status = format!(
                        "Previewing groove '{name}' · Space play/pause · f fork · s save · o bank · Enter load · Esc restore"
                    );
                }
                Ok(Outcome {
                    changed: true,
                    ..Outcome::default()
                })
            }
            Err(err) => {
                let msg = format!("{err:#}");
                if let Some(gb) = self.modal.groove_browser_mut() {
                    gb.preview_error = Some(msg.clone());
                }
                self.status = format!("Groove preview unavailable: {msg}");
                Ok(Outcome::default())
            }
        }
    }

    /// Cancel the Filter Groove Browser, restoring the exact original `Project` and audio state
    /// with ZERO undo stack entries and untouched `dirty()` / `revision()`.
    pub(crate) fn cancel_groove_browser(&mut self) -> Result<Outcome> {
        let Some(gb) = self.modal.take_groove_browser() else {
            return Ok(Outcome::default());
        };
        self.doc.restore_preview_snapshot(
            gb.original_project,
            &gb.original_flags,
            gb.original_compiled,
        )?;
        self.refresh_audio()?;
        self.input.mode = Mode::Normal;
        self.status = format!("Groove preview cancelled · restored {}", gb.device_name);
        Ok(Outcome {
            changed: true,
            ..Outcome::default()
        })
    }

    /// Commit the highlighted bank groove as ONE single clean undoable transaction.
    fn commit_groove_browser(&mut self) -> Result<Outcome> {
        let Some(gb) = self.modal.take_groove_browser() else {
            return Ok(Outcome::default());
        };
        let Some(selected) = gb.selected_item().cloned() else {
            self.modal = ActiveModal::GrooveBrowser(gb);
            self.status = "No matching groove · Esc clears filter".into();
            return Ok(Outcome::default());
        };
        self.doc.restore_preview_snapshot(
            gb.original_project,
            &gb.original_flags,
            gb.original_compiled,
        )?;
        self.input.mode = Mode::Normal;

        if selected.id == gb.original_env_pattern && gb.original_env_pattern_id.is_none() {
            self.refresh_audio()?;
            self.status = format!("Kept groove '{}' on {}", selected.name, gb.device_name);
            return Ok(Outcome {
                changed: true,
                ..Outcome::default()
            });
        }

        let edit = self.effect_edit(gb.device_id, |d| {
            if let EffectKind::Filter(fp) = &mut d.kind {
                fp.env_pattern = selected.id;
                fp.env_pattern_id = None;
            }
            Ok(())
        })?;
        self.commit(
            vec![edit],
            format!("Filter '{}' groove → {}", gb.device_name, selected.name),
        )
    }

    /// Fork the currently highlighted groove inside the browser directly to an editable project Pattern.
    fn fork_from_groove_browser(&mut self) -> Result<Outcome> {
        let Some(gb) = self.modal.take_groove_browser() else {
            return Ok(Outcome::default());
        };
        let Some(selected) = gb.selected_item().cloned() else {
            self.modal = ActiveModal::GrooveBrowser(gb);
            self.status = "No matching groove to fork · Esc clears filter".into();
            return Ok(Outcome::default());
        };
        self.doc.restore_preview_snapshot(
            gb.original_project,
            &gb.original_flags,
            gb.original_compiled,
        )?;
        self.input.mode = Mode::Normal;
        self.fork_preset_into_filter(gb.device_id, &selected)
    }

    pub(super) fn handle_groove_browser(&mut self, key: KeyEvent) -> Result<Outcome> {
        let repeat = key.kind == KeyEventKind::Repeat;
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if key.code == KeyCode::Char('u') {
                if let Some(gb) = self.modal.groove_browser_mut() {
                    if gb.searching {
                        gb.query.clear();
                        gb.cursor = 0;
                        gb.clamp_cursor();
                        return self.apply_groove_browser_preview();
                    }
                }
            }
            return Ok(Outcome::default());
        }

        let searching = self.modal.groove_browser().is_some_and(|gb| gb.searching);
        if searching {
            match key.code {
                KeyCode::Enter | KeyCode::Tab => {
                    if let Some(gb) = self.modal.groove_browser_mut() {
                        gb.searching = false;
                        gb.clamp_cursor();
                    }
                    return self.apply_groove_browser_preview();
                }
                KeyCode::Esc => {
                    if let Some(gb) = self.modal.groove_browser_mut() {
                        gb.query = gb.pre_search_query.clone();
                        gb.searching = false;
                        gb.cursor = 0;
                        gb.clamp_cursor();
                    }
                    return self.apply_groove_browser_preview();
                }
                KeyCode::Up => {
                    if let Some(gb) = self.modal.groove_browser_mut() {
                        gb.cursor = gb.cursor.saturating_sub(1);
                    }
                    return self.apply_groove_browser_preview();
                }
                KeyCode::Down => {
                    if let Some(gb) = self.modal.groove_browser_mut() {
                        let max = gb.visible_indices().len().saturating_sub(1);
                        gb.cursor = (gb.cursor + 1).min(max);
                    }
                    return self.apply_groove_browser_preview();
                }
                KeyCode::Backspace => {
                    if let Some(gb) = self.modal.groove_browser_mut() {
                        gb.query.pop();
                        gb.cursor = 0;
                        gb.clamp_cursor();
                    }
                    return self.apply_groove_browser_preview();
                }
                KeyCode::Char(c) if !c.is_control() => {
                    if let Some(gb) = self.modal.groove_browser_mut() {
                        if gb.query.len() < 64 {
                            gb.query.push(c);
                            gb.cursor = 0;
                            gb.clamp_cursor();
                        }
                    }
                    return self.apply_groove_browser_preview();
                }
                _ => return Ok(Outcome::default()),
            }
        }

        match key.code {
            KeyCode::Esc => {
                let has_query = self
                    .modal
                    .groove_browser()
                    .is_some_and(|gb| !gb.query.is_empty());
                if has_query {
                    if let Some(gb) = self.modal.groove_browser_mut() {
                        gb.query.clear();
                        gb.cursor = 0;
                        gb.clamp_cursor();
                    }
                    return self.apply_groove_browser_preview();
                }
                self.cancel_groove_browser()
            }
            KeyCode::Char('/') if !repeat => {
                if let Some(gb) = self.modal.groove_browser_mut() {
                    gb.pre_search_query = gb.query.clone();
                    gb.searching = true;
                }
                self.status = "Filter grooves · Enter applies · Esc restores".into();
                Ok(Outcome::default())
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(gb) = self.modal.groove_browser_mut() {
                    let max = gb.visible_indices().len().saturating_sub(1);
                    gb.cursor = (gb.cursor + 1).min(max);
                }
                self.apply_groove_browser_preview()
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(gb) = self.modal.groove_browser_mut() {
                    gb.cursor = gb.cursor.saturating_sub(1);
                }
                self.apply_groove_browser_preview()
            }
            KeyCode::Char('0' | 'g') => {
                if let Some(gb) = self.modal.groove_browser_mut() {
                    gb.cursor = 0;
                }
                self.apply_groove_browser_preview()
            }
            KeyCode::Char('$' | 'G') | KeyCode::End => {
                if let Some(gb) = self.modal.groove_browser_mut() {
                    gb.cursor = gb.visible_indices().len().saturating_sub(1);
                }
                self.apply_groove_browser_preview()
            }
            KeyCode::PageUp => {
                if let Some(gb) = self.modal.groove_browser_mut() {
                    gb.cursor = gb.cursor.saturating_sub(6);
                }
                self.apply_groove_browser_preview()
            }
            KeyCode::PageDown => {
                if let Some(gb) = self.modal.groove_browser_mut() {
                    let max = gb.visible_indices().len().saturating_sub(1);
                    gb.cursor = (gb.cursor + 6).min(max);
                }
                self.apply_groove_browser_preview()
            }
            KeyCode::Char('f') if !repeat => self.fork_from_groove_browser(),
            KeyCode::Char('s') if !repeat => {
                let Some(device_id) = self.modal.groove_browser().map(|gb| gb.device_id) else {
                    return Ok(Outcome::default());
                };
                // Restore the original untouched Filter state before opening Save Groove
                // so the user saves their active Filter's groove rather than a transient preview.
                let mut out = self.cancel_groove_browser()?;
                let save_out = self.begin_save_filter_groove(device_id)?;
                out.merge(save_out);
                Ok(out)
            }
            KeyCode::Char('o') if !repeat => {
                let out = self.cancel_groove_browser()?;
                self.open_file_hub_for_groove_bank_load();
                Ok(out)
            }
            KeyCode::Char(' ') if !repeat => self.apply(Action::Play {
                playing: !self.transport.playing,
            }),
            KeyCode::Enter if !repeat => self.commit_groove_browser(),
            _ => Ok(Outcome::default()),
        }
    }

    pub fn groove_browser_view(&self) -> Option<GrooveBrowserView> {
        let gb = self
            .modal
            .groove_browser()
            .filter(|_| self.input.mode == Mode::GrooveBrowser)?;
        let vis = gb.visible_indices();
        let items: Vec<GrooveBrowserItemView> = vis
            .iter()
            .enumerate()
            .map(|(row_idx, &entry_idx)| {
                let g = &gb.all_entries[entry_idx];
                GrooveBrowserItemView {
                    id: g.id,
                    name: g.name.clone(),
                    category: g.category.clone(),
                    resolution: g.resolution,
                    length: g.length,
                    smooth_attack: g.smooth_attack,
                    summary: summarize_groove_brief(g),
                    is_original: g.id == gb.original_env_pattern
                        && gb.original_env_pattern_id.is_none(),
                    selected: row_idx == gb.cursor,
                }
            })
            .collect();

        let (decay_ms, amount) = self
            .effect(gb.device_id)
            .and_then(|(_, d)| match d.kind {
                EffectKind::Filter(fp) => Some((fp.env_decay_ms, fp.env_amount)),
                _ => None,
            })
            .unwrap_or((240.0, 2.0));
        let bpm = self.doc.project().bpm;

        let selected_preview = gb.selected_item().map(|g| {
            let compiled = g.compile();
            let env_curve =
                RhythmicEnv::preview(&compiled, decay_ms, bpm, GROOVE_BROWSER_PREVIEW_PPS);
            let steps = g.steps.iter().map(|&v| f32::from(v) / 127.0).collect();
            let active_hits = g.steps.iter().filter(|&&v| v > 0).count();
            let peak_velocity = g.steps.iter().copied().max().unwrap_or(0);
            GrooveBrowserPreview {
                id: g.id,
                name: g.name.clone(),
                category: g.category.clone(),
                resolution: g.resolution,
                length: g.length,
                smooth_attack: g.smooth_attack,
                steps,
                env_curve,
                active_hits,
                peak_velocity,
                cycle_label: format_cycle_duration(g.resolution, g.length),
                env_decay_ms: decay_ms,
                env_amount: amount,
            }
        });

        let bank_source = self
            .assets
            .filter_grooves
            .source_path()
            .and_then(|p| p.file_name().and_then(|n| n.to_str()))
            .map(str::to_string)
            .unwrap_or_else(|| {
                if self.assets.filter_grooves.is_empty() {
                    "No bank loaded".into()
                } else {
                    "Loaded bank".into()
                }
            });

        Some(GrooveBrowserView {
            device_name: gb.device_name.clone(),
            channel_name: gb.channel_name.clone(),
            bank_source,
            query: gb.query.clone(),
            searching: gb.searching,
            live_preview_name: gb.previewed_name.clone(),
            preview_error: gb.preview_error.clone(),
            total_count: gb.all_entries.len(),
            items,
            cursor: gb.cursor,
            selected_preview,
        })
    }

    // ─── Fork Bank Groove → Editable Project Pattern ─────────────────────────

    /// Fork the active Filter's groove into an editable project `Pattern` and bind
    /// `Filter.env_pattern_id = Some(new_id)` in ONE atomic `Document::commit` step.
    pub(crate) fn fork_active_filter_groove(&mut self) -> Result<Outcome> {
        let Some(device_id) = self.active_filter_device() else {
            self.status = "No Filter effect in project · add one with nf → Filter".into();
            return Ok(Outcome::default());
        };
        self.fork_filter_groove_to_pattern(device_id)
    }

    pub(crate) fn fork_filter_groove_to_pattern(&mut self, device_id: Id) -> Result<Outcome> {
        let (_, dev) = self.effect(device_id).context("Filter effect missing")?;
        let EffectKind::Filter(fp) = dev.kind else {
            bail!("{} is not a Filter effect", dev.name);
        };
        let preset = match fp.env_pattern_id {
            None => self.assets.filter_grooves.preset(fp.env_pattern),
            Some(pid) => {
                let compiled = self
                    .doc
                    .project()
                    .compile_filter_pattern(pid)
                    .context("Active Filter pattern could not be compiled")?;
                let pat_name = self
                    .pattern(pid)
                    .map_or_else(|| format!("Pattern {pid}"), |p| p.name.clone());
                FilterEnvPreset::from_compiled(
                    fp.env_pattern,
                    format!("{pat_name} Groove"),
                    "Custom",
                    compiled,
                )?
            }
        };
        self.fork_preset_into_filter(device_id, &preset)
    }

    fn fork_preset_into_filter(
        &mut self,
        device_id: Id,
        preset: &FilterEnvPreset,
    ) -> Result<Outcome> {
        let needs_wrapper = matches!(preset.resolution, 8 | 32);
        let ids = self.doc.fresh_ids(if needs_wrapper { 2 } else { 1 })?;
        let base_id = ids[0];
        let wrapper_id = if needs_wrapper { ids[1] } else { base_id };

        let stem = strip_groove_id_prefix(&preset.name);
        let base_name = unique_pattern_name(self.doc.project(), stem, &[]);
        let wrapper_name = needs_wrapper.then(|| {
            let suffix = if preset.resolution == 32 {
                "1/32"
            } else {
                "1/8"
            };
            unique_pattern_name(
                self.doc.project(),
                &format!("{base_name} {suffix}"),
                std::slice::from_ref(&base_name),
            )
        });
        let (patterns, bound_id) =
            preset.to_project_patterns(base_id, wrapper_id, base_name.clone(), wrapper_name);

        let mut edits: Vec<Edit> = patterns
            .into_iter()
            .map(|pattern| Edit::PutPattern { pattern })
            .collect();
        edits.push(self.effect_edit(device_id, |d| {
            if let EffectKind::Filter(fp) = &mut d.kind {
                fp.env_pattern_id = Some(bound_id);
            }
            Ok(())
        })?);

        let label = format!("Fork groove '{}' → pattern '{}'", preset.name, base_name);
        let out = self.commit(edits, label)?;
        self.selected_pattern = Some(base_id);
        self.status = format!(
            "Forked '{}' → editable pattern '{base_name}' (gp edits · u undoes)",
            preset.name
        );
        Ok(out)
    }

    // ─── Save / Export Groove to Multi-Groove Bank (`filter_grooves.json`) ──

    /// Open unified `Mode::ValueInput` prompt to save the active Filter's groove into
    /// `<asset_base>/presets/filter_grooves.json`.
    pub(crate) fn begin_save_active_filter_groove(&mut self) -> Result<Outcome> {
        let Some(device_id) = self.active_filter_device() else {
            self.status = "No Filter effect in project · add one with nf → Filter".into();
            return Ok(Outcome::default());
        };
        self.begin_save_filter_groove(device_id)
    }

    pub(crate) fn begin_save_filter_groove(&mut self, device_id: Id) -> Result<Outcome> {
        let (_, dev) = self.effect(device_id).context("Filter effect missing")?;
        let EffectKind::Filter(fp) = dev.kind else {
            bail!("{} is not a Filter effect", dev.name);
        };
        let (compiled, default_name, source_summary) = match fp.env_pattern_id {
            Some(pid) => {
                let compiled = self
                    .doc
                    .project()
                    .compile_filter_pattern(pid)
                    .context("Active Filter pattern cannot be exported as a groove")?;
                let pat_name = self
                    .pattern(pid)
                    .map_or_else(|| format!("Pattern {pid}"), |p| p.name.clone());
                (
                    compiled,
                    pat_name.clone(),
                    format!(
                        "Pat: {pat_name} · 1/{} · {}st · {}",
                        compiled.resolution,
                        compiled.length,
                        if compiled.smooth_attack {
                            "Smooth"
                        } else {
                            "Punch"
                        }
                    ),
                )
            }
            None => {
                let preset = self.assets.filter_grooves.preset(fp.env_pattern);
                let compiled = preset.compile();
                let stem = strip_groove_id_prefix(&preset.name);
                let suggested = format!("{stem} Custom");
                (
                    compiled,
                    suggested,
                    format!(
                        "{} · 1/{} · {}st · {}",
                        preset.name,
                        compiled.resolution,
                        compiled.length,
                        if compiled.smooth_attack {
                            "Smooth"
                        } else {
                            "Punch"
                        }
                    ),
                )
            }
        };

        let existing = self.load_grooves_for_save_base()?;
        let suggested_id = next_available_id(&existing).unwrap_or(fp.env_pattern);
        self.begin_save_filter_groove_input(
            device_id,
            compiled,
            suggested_id,
            source_summary,
            default_name,
        )
    }

    fn filter_groove_bank_save_path(&self) -> PathBuf {
        self.doc
            .asset_base()
            .join("presets")
            .join("filter_grooves.json")
    }

    /// Read existing grooves from `<asset_base>/presets/filter_grooves.json` when present,
    /// otherwise clone the currently loaded catalog so factory grooves are always preserved!
    fn load_grooves_for_save_base(&self) -> Result<Vec<FilterEnvPreset>> {
        let bank_path = self.filter_groove_bank_save_path();
        if self.io.is_file(&bank_path) {
            let bytes = self
                .io
                .read_bounded(&bank_path, MAX_FILTER_GROOVES_BYTES)
                .with_context(|| format!("Reading {}", bank_path.display()))?;
            let cat = FilterGrooveCatalog::parse_bytes(&bytes)
                .with_context(|| format!("Parsing {}", bank_path.display()))?;
            Ok(cat.grooves().to_vec())
        } else {
            Ok(self.assets.filter_grooves.grooves().to_vec())
        }
    }

    /// Shared SPOT used by BOTH live `ValueInput` preview and final atomic bank persistence.
    pub(crate) fn plan_save_filter_groove(
        &self,
        compiled: CompiledFilterPattern,
        raw_name: &str,
    ) -> Result<(u8, FilterEnvPreset, Vec<FilterEnvPreset>, bool)> {
        let trimmed = raw_name.trim();
        ensure!(!trimmed.is_empty(), "Enter a groove name (1..=80 chars)");
        ensure!(
            !trimmed.chars().any(char::is_control),
            "Groove name cannot contain control characters"
        );
        let mut grooves = self.load_grooves_for_save_base()?;
        let (target_id, clean_label, replacing) = if let Some((explicit_id, tail)) =
            parse_explicit_slot_prefix(trimmed)
        {
            let exists = grooves.iter().any(|g| g.id == explicit_id);
            (explicit_id, tail, exists)
        } else if let Some(existing) = grooves.iter().find(|g| {
            g.category.eq_ignore_ascii_case("Custom")
                && strip_groove_id_prefix(&g.name).eq_ignore_ascii_case(trimmed)
        }) {
            (existing.id, trimmed, true)
        } else {
            let id = next_available_id(&grooves).context(
                    "Filter groove bank is full (128 slots max) · prefix an ID like '24 Name' to overwrite",
                )?;
            (id, trimmed, false)
        };
        let formatted_name = format_groove_name_with_id(target_id, clean_label);
        let preset = FilterEnvPreset::from_compiled(target_id, formatted_name, "Custom", compiled)?;
        upsert_groove(&mut grooves, preset.clone())?;
        Ok((target_id, preset, grooves, replacing))
    }

    pub(crate) fn commit_save_filter_groove(
        &mut self,
        device_id: Id,
        compiled: CompiledFilterPattern,
        raw_name: &str,
    ) -> Result<Outcome> {
        let (target_id, preset, grooves, replacing) =
            self.plan_save_filter_groove(compiled, raw_name)?;
        let bank_path = self.filter_groove_bank_save_path();
        let bytes = FilterGrooveCatalog::serialize_grooves(&grooves)?;
        self.io
            .write_atomic(&bank_path, MAX_FILTER_GROOVES_BYTES, &bytes)?;

        // Reload active catalog via HostIo (prioritizing `<asset_base>/presets`) and refresh RT bank!
        let roots = self.filter_groove_search_roots();
        let reloaded = self.io.load_filter_grooves(&roots);
        let bank_count = reloaded.len();
        self.set_filter_grooves(reloaded);

        let needs_project_edit = self.effect(device_id).is_some_and(|(_, d)| match d.kind {
            EffectKind::Filter(fp) => fp.env_pattern != target_id || fp.env_pattern_id.is_some(),
            _ => false,
        });
        let mut out = if needs_project_edit {
            let dev_name = self
                .effect(device_id)
                .map_or_else(|| format!("Filter #{device_id}"), |(_, d)| d.name.clone());
            let edit = self.effect_edit(device_id, |d| {
                if let EffectKind::Filter(fp) = &mut d.kind {
                    fp.env_pattern = target_id;
                    fp.env_pattern_id = None;
                }
                Ok(())
            })?;
            self.commit(
                vec![edit],
                format!("Filter '{dev_name}' groove → {}", preset.name),
            )?
        } else {
            self.refresh_audio()?;
            Outcome {
                changed: true,
                ..Outcome::default()
            }
        };
        out.changed = true;
        let verb = if replacing { "Updated" } else { "Saved" };
        self.status = format!(
            "{verb} groove '{}' → presets/filter_grooves.json ({bank_count} grooves loaded)",
            preset.name
        );
        Ok(out)
    }

    // ─── Load / Reload External Filter Groove Bank ──────────────────────────

    fn resolve_groove_bank_path(&self, raw: &Path) -> PathBuf {
        if self.io.is_file(raw) {
            return raw.to_path_buf();
        }
        if raw.is_relative() {
            let rel_project = self.doc.asset_base().join(raw);
            if self.io.is_file(&rel_project) {
                return rel_project;
            }
            for root in self.filter_groove_search_roots() {
                let candidate = root.join(raw);
                if self.io.is_file(&candidate) {
                    return candidate;
                }
            }
        }
        raw.to_path_buf()
    }

    /// Load an arbitrary Filter Groove Bank JSON file via `self.io` (shared by File Hub,
    /// Finder Import, and `:groove-bank <path>`) and immediately refresh the live RT bank.
    pub(crate) fn load_filter_groove_bank_from_path(&mut self, path: &Path) -> Result<Outcome> {
        let resolved = self.resolve_groove_bank_path(path);
        let bytes = self
            .io
            .read_bounded(&resolved, MAX_FILTER_GROOVES_BYTES)
            .with_context(|| format!("Reading Filter Groove Bank {}", resolved.display()))?;
        let catalog = FilterGrooveCatalog::parse_bytes(&bytes)
            .with_context(|| format!("Parsing Filter Groove Bank {}", resolved.display()))?
            .with_source_path(resolved.clone());
        let count = catalog.len();
        let label = resolved
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("filter_grooves.json")
            .to_string();
        self.set_filter_grooves(catalog);
        self.refresh_audio()?;
        self.status = format!(
            "Loaded Filter Groove Bank '{label}' ({count} groove{})",
            if count == 1 { "" } else { "s" }
        );
        Ok(Outcome {
            changed: true,
            ..Outcome::default()
        })
    }

    /// Reload the active Filter Groove Bank from standard preset search roots (`:groove-bank reload`).
    pub(crate) fn reload_filter_groove_bank(&mut self) -> Result<Outcome> {
        let roots = self.filter_groove_search_roots();
        let reloaded = self.io.load_filter_grooves(&roots);
        let count = reloaded.len();
        let label = reloaded
            .source_path()
            .and_then(|p| p.file_name().and_then(|n| n.to_str()))
            .unwrap_or("presets/filter_grooves.json")
            .to_string();
        self.set_filter_grooves(reloaded);
        self.refresh_audio()?;
        self.status = if count == 0 {
            "Reloaded Filter Groove Bank (no bank found in preset roots)".into()
        } else {
            format!(
                "Reloaded Filter Groove Bank '{label}' ({count} groove{})",
                if count == 1 { "" } else { "s" }
            )
        };
        Ok(Outcome {
            changed: true,
            ..Outcome::default()
        })
    }
}
