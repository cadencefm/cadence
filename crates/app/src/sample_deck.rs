//! Dedicated Sample / Waveform Deck (`gw`, `DeckKind::Sample`):
//! non-destructive crop/loop editing, zero-crossing snapping, transient & equal-grid slicing,
//! active-slice auditioning, one-step Slice-to-Rack-Kit conversion, lossless RIFF WAV metadata
//! write-back, and `:slice` / `:sample write-meta` Ex commands.
use super::*;
use crate::document::CoalesceKey;
use crate::layout::{Deck, DeckKind, Target};
use cadence_core::dsp::{
    default_zero_snap_radius, detect_grid_slices, detect_transient_slices, slice_regions,
    snap_to_zero_crossing,
};
use cadence_core::rack::{SampleLoopMode, SamplerModule, MAX_ZONES};
use cadence_core::sampler::{
    normalize_slices, update_wav_riff_metadata_bytes, SampleMetadata, SamplePreview, SampleSlice,
    MAX_SAMPLE_FILE_BYTES, MAX_SAMPLE_SLICES,
};

pub(crate) const SAMPLE_ZOOM_MIN: f32 = 1.0;
pub(crate) const SAMPLE_ZOOM_MAX: f32 = 128.0;
pub(crate) const SAMPLE_MIN_VISIBLE_FRAMES: usize = 32;

/// Single point of truth for the Sample deck's visible frame window `[start, end)`.
/// Applies 10% horizontal scrolloff so scrubbing glides smoothly and zooming keeps the
/// edit cursor visible inside the viewport.
pub(crate) fn sample_deck_window(
    cursor_frame: usize,
    total_frames: usize,
    zoom: f32,
    prev_start: usize,
) -> (usize, usize) {
    if total_frames == 0 {
        return (0, 0);
    }
    let clamped_zoom = zoom.clamp(SAMPLE_ZOOM_MIN, SAMPLE_ZOOM_MAX);
    let min_vis = SAMPLE_MIN_VISIBLE_FRAMES.min(total_frames);
    let vis = ((total_frames as f64) / f64::from(clamped_zoom))
        .round()
        .clamp(min_vis as f64, total_frames as f64) as usize;
    if vis >= total_frames {
        return (0, total_frames);
    }
    let max_start = total_frames - vis;
    let cursor = cursor_frame.min(total_frames.saturating_sub(1));
    let mut start = prev_start.min(max_start);
    let margin = (vis / 10).min(vis / 3);
    if cursor < start.saturating_add(margin) {
        start = cursor.saturating_sub(margin);
    } else if cursor >= start.saturating_add(vis).saturating_sub(margin) {
        start = cursor
            .saturating_add(margin)
            .saturating_add(1)
            .saturating_sub(vis);
    }
    start = start.min(max_start);
    (start, (start + vis).min(total_frames))
}

/// Format a frame offset as concise seconds or milliseconds.
pub(crate) fn format_sample_time(frame: usize, sample_rate: u32) -> String {
    if sample_rate == 0 {
        return format!("{frame}f");
    }
    let secs = frame as f64 / f64::from(sample_rate);
    if secs < 1.0 {
        format!("{:.1}ms", secs * 1000.0)
    } else {
        format!("{secs:.3}s")
    }
}

/// Return the effective inclusive-start / exclusive-end crop bounds for `smp` within `total_frames`.
pub(crate) fn effective_sampler_bounds(smp: &SamplerModule, total_frames: usize) -> (usize, usize) {
    if total_frames == 0 {
        return (0, 0);
    }
    let start = smp.sample_start.min(total_frames.saturating_sub(1));
    let end = if smp.sample_end == 0 {
        total_frames
    } else {
        smp.sample_end.clamp(start + 1, total_frames)
    };
    (start, end)
}

/// Find the active slice region index containing `cursor_frame` (or nearest slice before it).
pub(crate) fn active_slice_index_for_cursor(
    regions: &[(usize, usize, Option<&str>)],
    cursor_frame: usize,
) -> Option<usize> {
    if regions.is_empty() {
        return None;
    }
    if let Some(pos) = regions
        .iter()
        .position(|&(s, e, _)| cursor_frame >= s && cursor_frame < e)
    {
        return Some(pos);
    }
    if cursor_frame < regions[0].0 {
        Some(0)
    } else {
        Some(regions.len() - 1)
    }
}

/// Whether a path refers to a `.wav` file (case-insensitive).
pub(crate) fn is_wav_locator(path: Option<&str>) -> bool {
    path.and_then(|p| Path::new(p).extension())
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("wav"))
}

impl App {
    /// Locate a Sampler module `(instrument, zone, module)` inside `instrument_id`,
    /// preferring the focused Rack Panel/Patchbay module if it is a Sampler, else the
    /// active zone's first Sampler, else the first Sampler in any zone.
    pub(crate) fn sampler_target_for_instrument(
        &self,
        instrument_id: Id,
    ) -> Option<(Id, u16, u16)> {
        let inst = self.instrument(instrument_id)?;
        let z_idx = self.active_zone_index(&inst.rack);
        if let Some(zone) = inst.rack.zones.get(z_idx) {
            if let Some(focused_mod) = self.focused_rack_module_id(&inst.rack) {
                if zone
                    .module(focused_mod)
                    .is_some_and(|m| matches!(m.kind, ModuleKind::Sampler(_)))
                {
                    return Some((instrument_id, zone.id, focused_mod));
                }
            }
            if let Some(m) = zone
                .modules
                .iter()
                .find(|m| matches!(m.kind, ModuleKind::Sampler(_)))
            {
                return Some((instrument_id, zone.id, m.id));
            }
        }
        inst.rack
            .sampler_modules()
            .next()
            .map(|(z_id, _, m_id, _)| (instrument_id, z_id, m_id))
    }

    /// Currently focused (or visible) `DeckKind::Sample` target, if valid.
    pub(crate) fn focused_sample_target(&self) -> Option<(Id, u16, u16)> {
        let extract = |deck: &Deck| match deck.target {
            Target::Sample {
                instrument,
                zone,
                module,
            } => self
                .sampler_module_at(instrument, zone, module)
                .map(|_| (instrument, zone, module)),
            _ => None,
        };
        if let Some(deck) = self.layout.focused() {
            if let Some(t) = extract(deck) {
                return Some(t);
            }
        }
        if let Some(deck) = self.layout.working() {
            if let Some(t) = extract(deck) {
                return Some(t);
            }
        }
        self.layout.pinned().iter().find_map(extract)
    }

    /// Resolve the best Sampler module target in the project for opening or acting on the Sample deck.
    pub(crate) fn resolve_sample_deck_target(&self) -> Option<(Id, u16, u16)> {
        if let Some(t) = self.focused_sample_target() {
            return Some(t);
        }
        if let Some(inst_id) = self.active_instrument().or(self.selected_instrument) {
            if let Some(t) = self.sampler_target_for_instrument(inst_id) {
                return Some(t);
            }
        }
        for inst in &self.doc.project().instruments {
            if let Some(t) = self.sampler_target_for_instrument(inst.id) {
                return Some(t);
            }
        }
        None
    }

    /// Immutable reference to the `SamplerModule` at `(instrument, zone, module)`.
    pub(crate) fn sampler_module_at(
        &self,
        instrument: Id,
        zone: u16,
        module: u16,
    ) -> Option<&SamplerModule> {
        let m = self
            .instrument(instrument)?
            .rack
            .zone(zone)?
            .module(module)?;
        match &m.kind {
            ModuleKind::Sampler(smp) => Some(smp),
            _ => None,
        }
    }

    /// Loaded decoded audio buffer for `(instrument, zone, module)`, if any.
    pub(crate) fn loaded_sample_for(
        &self,
        instrument: Id,
        zone: u16,
        module: u16,
    ) -> Option<Arc<SampleData>> {
        self.assets
            .samples
            .get(&module_sample_key(instrument, zone, module))
            .cloned()
    }

    /// Open the dedicated Sample / Waveform deck (`gw`) targeting the active/first Sampler.
    pub(crate) fn open_sample_deck(&mut self) -> Result<Outcome> {
        let (instrument, zone, module) = self.resolve_sample_deck_target().context(
            "No Sampler instrument in project · press B to browse samples or n to create a Sampler",
        )?;
        self.open_sample_deck_for(instrument, zone, module)
    }

    /// Open the dedicated Sample / Waveform deck (`gw`) targeting a specific Sampler module.
    pub(crate) fn open_sample_deck_for(
        &mut self,
        instrument: Id,
        zone: u16,
        module: u16,
    ) -> Result<Outcome> {
        ensure!(
            self.sampler_module_at(instrument, zone, module).is_some(),
            "Target module is not a Sampler"
        );
        self.selected_instrument = Some(instrument);
        if let Some(inst) = self.instrument(instrument) {
            if let Some(z_idx) = inst.rack.zones.iter().position(|z| z.id == zone) {
                self.rack_ui.zone_cursor = z_idx;
            }
        }
        let target = (instrument, zone, module);
        let frames = self
            .loaded_sample_for(instrument, zone, module)
            .map_or(0, |s| s.frames);
        if self.sample_ui.last_target.get() != Some(target) {
            self.sample_ui.last_target.set(Some(target));
            let start = self
                .sampler_module_at(instrument, zone, module)
                .map_or(0, |s| s.sample_start);
            self.sample_ui.cursor_frame = if frames > 0 { start.min(frames - 1) } else { 0 };
            self.sample_ui.zoom = 1.0;
            self.sample_ui.view_start_frame.set(0);
        }
        self.go_to_deck(Deck::new(
            DeckKind::Sample,
            Target::Sample {
                instrument,
                zone,
                module,
            },
        ));
        self.update_sample_deck_status(instrument, zone, module);
        Ok(Outcome::default())
    }

    fn update_sample_deck_status(&mut self, instrument: Id, zone: u16, module: u16) {
        let Some(smp) = self.sampler_module_at(instrument, zone, module).cloned() else {
            return;
        };
        let Some(sample) = self.loaded_sample_for(instrument, zone, module) else {
            self.status = "Sample deck (no file loaded) · B browse samples".into();
            return;
        };
        let frames = sample.frames;
        let cursor = self.sample_ui.cursor_frame.min(frames.saturating_sub(1));
        let (start, end) = effective_sampler_bounds(&smp, frames);
        let regions = slice_regions(&smp.slices, start, end);
        let time_str = format_sample_time(cursor, sample.sample_rate);
        let slice_hint = if !smp.slices.is_empty() {
            if let Some(idx) = active_slice_index_for_cursor(&regions, cursor) {
                let lbl = regions[idx]
                    .2
                    .map(|l| format!(" '{l}'"))
                    .unwrap_or_default();
                format!(" · Slice {}/{}{}", idx + 1, regions.len(), lbl)
            } else {
                format!(" · {} slices", smp.slices.len())
            }
        } else {
            String::new()
        };
        let snap = if self.sample_ui.snap_zero {
            "ZC"
        } else {
            "FREE"
        };
        self.status = format!(
            "Sample · {time_str} (f{cursor}){slice_hint} · {snap} · [/] crop · s split · d/D slice · A play · K kit"
        );
    }

    /// Snap `frame` to nearest zero-crossing inside `[min_frame, max_frame]` if `snap_zero` is enabled.
    fn maybe_snap_frame(
        &self,
        sample: &SampleData,
        frame: usize,
        min_frame: usize,
        max_frame: usize,
    ) -> usize {
        let clamped = frame.clamp(min_frame, max_frame);
        if !self.sample_ui.snap_zero || sample.frames == 0 {
            return clamped;
        }
        let radius = default_zero_snap_radius(sample.sample_rate);
        snap_to_zero_crossing(
            &sample.left,
            sample.right.as_deref(),
            clamped,
            min_frame,
            max_frame,
            radius,
        )
    }

    /// Horizontal zoom on the Sample deck (`=`/`+`/`-`, `Ctrl+=`/`-`, `z=`/`z-`).
    pub(crate) fn zoom_sample_deck(&mut self, direction: i8) {
        let Some((inst, zone, module)) = self.focused_sample_target() else {
            return;
        };
        let frames = self
            .loaded_sample_for(inst, zone, module)
            .map_or(0, |s| s.frames);
        let prev = self.sample_ui.zoom.clamp(SAMPLE_ZOOM_MIN, SAMPLE_ZOOM_MAX);
        let next = if direction > 0 {
            (prev * 1.5).min(SAMPLE_ZOOM_MAX)
        } else {
            (prev / 1.5).max(SAMPLE_ZOOM_MIN)
        };
        self.sample_ui.zoom = if (next - 1.0).abs() < 0.05 { 1.0 } else { next };
        if frames > 0 {
            // Re-center viewport around cursor when zooming in/out.
            let vis = ((frames as f64) / f64::from(self.sample_ui.zoom))
                .round()
                .clamp(SAMPLE_MIN_VISIBLE_FRAMES.min(frames) as f64, frames as f64)
                as usize;
            let half = vis / 2;
            let ideal_start = self
                .sample_ui
                .cursor_frame
                .saturating_sub(half)
                .min(frames.saturating_sub(vis));
            let (vs, _) = sample_deck_window(
                self.sample_ui.cursor_frame,
                frames,
                self.sample_ui.zoom,
                ideal_start,
            );
            self.sample_ui.view_start_frame.set(vs);
        }
        self.status = format!("Sample zoom {:.1}×", self.sample_ui.zoom);
    }

    /// Reset Sample deck horizontal zoom to 1.0× (full file).
    pub(crate) fn zoom_sample_deck_reset(&mut self) {
        self.sample_ui.zoom = 1.0;
        self.sample_ui.view_start_frame.set(0);
        self.status = "Sample zoom 1.0× · full waveform".into();
    }

    /// Jump Sample deck cursor to start (`0`/`gg`) or end (`$`/`G`/`ge`) of the file/crop.
    pub(crate) fn navigate_sample_deck_edge(&mut self, end: bool) {
        let Some((inst, zone, module)) = self.focused_sample_target() else {
            return;
        };
        let Some(sample) = self.loaded_sample_for(inst, zone, module) else {
            return;
        };
        let frames = sample.frames;
        if frames == 0 {
            return;
        }
        self.sample_ui.cursor_frame = if end { frames - 1 } else { 0 };
        let (vs, _) = sample_deck_window(
            self.sample_ui.cursor_frame,
            frames,
            self.sample_ui.zoom,
            self.sample_ui.view_start_frame.get(),
        );
        self.sample_ui.view_start_frame.set(vs);
        self.update_sample_deck_status(inst, zone, module);
    }

    /// Page Sample deck cursor forward/backward by 50% of the visible window (`PgUp`/`PgDn`).
    pub(crate) fn navigate_sample_deck_page(&mut self, forward: bool, count: usize) {
        let Some((inst, zone, module)) = self.focused_sample_target() else {
            return;
        };
        let Some(sample) = self.loaded_sample_for(inst, zone, module) else {
            return;
        };
        let frames = sample.frames;
        if frames == 0 {
            return;
        }
        let (vs, ve) = sample_deck_window(
            self.sample_ui.cursor_frame,
            frames,
            self.sample_ui.zoom,
            self.sample_ui.view_start_frame.get(),
        );
        let vis = ve.saturating_sub(vs).max(1);
        let stride = (vis / 2).max(1).saturating_mul(count.max(1));
        self.sample_ui.cursor_frame = if forward {
            self.sample_ui
                .cursor_frame
                .saturating_add(stride)
                .min(frames - 1)
        } else {
            self.sample_ui.cursor_frame.saturating_sub(stride)
        };
        let (nvs, _) =
            sample_deck_window(self.sample_ui.cursor_frame, frames, self.sample_ui.zoom, vs);
        self.sample_ui.view_start_frame.set(nvs);
        self.update_sample_deck_status(inst, zone, module);
    }

    /// `NORMAL`-mode key dispatch when `DeckKind::Sample` is focused.
    /// Strict 4-tier keyboard grammar: `h`/`j`/`k`/`l`/`H`/`L`/`w`/`b` ONLY navigate & select.
    pub(crate) fn handle_sample_deck_key(
        &mut self,
        key: KeyEvent,
        count: usize,
    ) -> Option<Result<Outcome>> {
        let (inst, zone, module) = self.focused_sample_target()?;
        let loaded = self.loaded_sample_for(inst, zone, module);
        let frames = loaded.as_ref().map_or(0, |s| s.frames);

        let scrub = |app: &mut App, dir: isize, coarse: bool| -> Outcome {
            if frames == 0 {
                app.status = "No sample loaded · press B to browse samples".into();
                return Outcome::default();
            }
            let (vs, ve) = sample_deck_window(
                app.sample_ui.cursor_frame,
                frames,
                app.sample_ui.zoom,
                app.sample_ui.view_start_frame.get(),
            );
            let vis = ve.saturating_sub(vs).max(1);
            let base = if coarse {
                (vis / 10).max(1)
            } else {
                (vis / 100).max(1)
            };
            let delta = base.saturating_mul(count.max(1));
            app.sample_ui.cursor_frame = if dir < 0 {
                app.sample_ui.cursor_frame.saturating_sub(delta)
            } else {
                app.sample_ui
                    .cursor_frame
                    .saturating_add(delta)
                    .min(frames - 1)
            };
            let (nvs, _) =
                sample_deck_window(app.sample_ui.cursor_frame, frames, app.sample_ui.zoom, vs);
            app.sample_ui.view_start_frame.set(nvs);
            app.update_sample_deck_status(inst, zone, module);
            Outcome::default()
        };

        match key.code {
            // Tier 1 navigation — strictly non-mutating
            KeyCode::Char('h') | KeyCode::Left => Some(Ok(scrub(self, -1, false))),
            KeyCode::Char('l') | KeyCode::Right => Some(Ok(scrub(self, 1, false))),
            KeyCode::Char('H') => Some(Ok(scrub(self, -1, true))),
            KeyCode::Char('L') => Some(Ok(scrub(self, 1, true))),
            KeyCode::Char('j') | KeyCode::Down => {
                self.sample_ui.toolbar_field =
                    self.sample_ui.toolbar_field.step(count.max(1) as isize);
                self.status = format!(
                    "Sample control: {} · Alt+h/l ,/. adjust · Enter exact",
                    self.sample_ui.toolbar_field.label()
                );
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.sample_ui.toolbar_field =
                    self.sample_ui.toolbar_field.step(-(count.max(1) as isize));
                self.status = format!(
                    "Sample control: {} · Alt+h/l ,/. adjust · Enter exact",
                    self.sample_ui.toolbar_field.label()
                );
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('w') => {
                Some(Ok(self.jump_sample_marker(inst, zone, module, true, count)))
            }
            KeyCode::Char('b') => {
                Some(Ok(self.jump_sample_marker(inst, zone, module, false, count)))
            }
            KeyCode::Char('0') => {
                self.navigate_sample_deck_edge(false);
                Some(Ok(Outcome::default()))
            }
            KeyCode::Char('$' | 'G') => {
                self.navigate_sample_deck_edge(true);
                Some(Ok(Outcome::default()))
            }

            // Crop & Loop boundaries
            KeyCode::Char('[') => Some(self.set_sample_crop_from_cursor(inst, zone, module, false)),
            KeyCode::Char(']') => Some(self.set_sample_crop_from_cursor(inst, zone, module, true)),
            KeyCode::Char('{') => Some(self.set_sample_loop_from_cursor(inst, zone, module, false)),
            KeyCode::Char('}') => Some(self.set_sample_loop_from_cursor(inst, zone, module, true)),
            KeyCode::Char('m') => Some(self.cycle_sample_loop_mode(inst, zone, module)),
            KeyCode::Char('v') => {
                self.sample_ui.snap_zero = !self.sample_ui.snap_zero;
                self.status = if self.sample_ui.snap_zero {
                    "Zero-crossing snap: ON".into()
                } else {
                    "Zero-crossing snap: OFF (free sample-accurate placement)".into()
                };
                Some(Ok(Outcome::default()))
            }

            // Slicing verbs
            KeyCode::Char('s') => Some(self.split_sample_at_cursor(inst, zone, module)),
            KeyCode::Char('x') => Some(self.delete_sample_slice_at_cursor(inst, zone, module)),
            KeyCode::Char('X') => Some(self.clear_sample_slices(inst, zone, module)),
            KeyCode::Char('d') => Some(self.auto_slice_transients(inst, zone, module)),
            KeyCode::Char('D') => Some(self.auto_slice_grid(inst, zone, module)),
            KeyCode::Char('r') => Some(self.begin_rename_active_slice(inst, zone, module)),

            // Audition, Browser, Slice-to-Kit, RIFF write-back
            KeyCode::Char('A') => Some(self.audition_active_sample_slice(inst, zone, module)),
            KeyCode::Char('B') => Some(self.begin_sample_browser(BrowserTarget::AssignModule {
                instrument: inst,
                zone_id: zone,
                module_id: Some(module),
            })),
            KeyCode::Char('K') => Some(self.open_slice_to_kit_picker(inst, zone, module)),
            KeyCode::Char('W') => Some(self.apply(Action::WriteSampleRiffMetadata {
                instrument: inst,
                zone,
                module,
            })),
            KeyCode::Enter => Some(self.handle_sample_deck_enter(inst, zone, module)),
            _ => None,
        }
    }

    fn jump_sample_marker(
        &mut self,
        inst: Id,
        zone: u16,
        module: u16,
        forward: bool,
        count: usize,
    ) -> Outcome {
        let Some(smp) = self.sampler_module_at(inst, zone, module).cloned() else {
            return Outcome::default();
        };
        let Some(sample) = self.loaded_sample_for(inst, zone, module) else {
            return Outcome::default();
        };
        let frames = sample.frames;
        if frames == 0 {
            return Outcome::default();
        }
        let (start, end) = effective_sampler_bounds(&smp, frames);
        let mut markers: Vec<usize> = Vec::with_capacity(smp.slices.len() + 2);
        markers.push(start);
        for s in &smp.slices {
            if s.frame < frames {
                markers.push(s.frame);
            }
        }
        markers.push(end.saturating_sub(1));
        markers.sort_unstable();
        markers.dedup();
        if markers.is_empty() {
            return Outcome::default();
        }
        let mut cur = self.sample_ui.cursor_frame.min(frames - 1);
        for _ in 0..count.max(1) {
            if forward {
                if let Some(&next) = markers.iter().find(|&&m| m > cur) {
                    cur = next;
                } else {
                    break;
                }
            } else if let Some(&prev) = markers.iter().rev().find(|&&m| m < cur) {
                cur = prev;
            } else {
                break;
            }
        }
        self.sample_ui.cursor_frame = cur;
        let (vs, _) = sample_deck_window(
            cur,
            frames,
            self.sample_ui.zoom,
            self.sample_ui.view_start_frame.get(),
        );
        self.sample_ui.view_start_frame.set(vs);
        self.update_sample_deck_status(inst, zone, module);
        Outcome::default()
    }

    fn set_sample_crop_from_cursor(
        &mut self,
        inst: Id,
        zone: u16,
        module: u16,
        is_end: bool,
    ) -> Result<Outcome> {
        let smp = self
            .sampler_module_at(inst, zone, module)
            .context("Sampler module missing")?
            .clone();
        let sample = self
            .loaded_sample_for(inst, zone, module)
            .context("Load a sample first (press B)")?;
        ensure!(sample.frames > 16, "Sample is too short to crop");
        let (cur_start, cur_end) = effective_sampler_bounds(&smp, sample.frames);
        let cursor = self.sample_ui.cursor_frame.min(sample.frames - 1);
        if is_end {
            let min_end = (cur_start + 16).min(sample.frames);
            let snapped = self.maybe_snap_frame(&sample, cursor, min_end, sample.frames);
            let new_end = snapped.clamp(min_end, sample.frames);
            let stored = if new_end >= sample.frames { 0 } else { new_end };
            self.sample_ui.cursor_frame = new_end.min(sample.frames - 1);
            self.apply(Action::SetSamplerRegion {
                instrument: inst,
                zone,
                module,
                sample_start: None,
                sample_end: Some(stored),
                loop_mode: None,
                loop_start: None,
                loop_end: None,
                loop_xfade_ms: None,
            })
        } else {
            let max_start = cur_end.saturating_sub(16);
            let snapped = self.maybe_snap_frame(&sample, cursor, 0, max_start);
            let new_start = snapped.min(max_start);
            self.sample_ui.cursor_frame = new_start;
            self.apply(Action::SetSamplerRegion {
                instrument: inst,
                zone,
                module,
                sample_start: Some(new_start),
                sample_end: None,
                loop_mode: None,
                loop_start: None,
                loop_end: None,
                loop_xfade_ms: None,
            })
        }
    }

    fn set_sample_loop_from_cursor(
        &mut self,
        inst: Id,
        zone: u16,
        module: u16,
        is_end: bool,
    ) -> Result<Outcome> {
        let smp = self
            .sampler_module_at(inst, zone, module)
            .context("Sampler module missing")?
            .clone();
        let sample = self
            .loaded_sample_for(inst, zone, module)
            .context("Load a sample first (press B)")?;
        ensure!(sample.frames > 16, "Sample is too short to loop");
        let (start_b, end_b) = effective_sampler_bounds(&smp, sample.frames);
        let cur_ls = smp.loop_start.clamp(start_b, end_b.saturating_sub(16));
        let cur_le = if smp.loop_end == 0 {
            end_b
        } else {
            smp.loop_end.clamp(cur_ls + 16, end_b)
        };
        let cursor = self.sample_ui.cursor_frame.min(sample.frames - 1);
        let enable_mode = if smp.loop_mode == SampleLoopMode::Off {
            Some(SampleLoopMode::Forward)
        } else {
            None
        };
        if is_end {
            let min_le = (cur_ls + 16).min(end_b);
            let snapped = self.maybe_snap_frame(&sample, cursor, min_le, end_b);
            let new_le = snapped.clamp(min_le, end_b);
            self.sample_ui.cursor_frame = new_le.min(sample.frames - 1);
            self.apply(Action::SetSamplerRegion {
                instrument: inst,
                zone,
                module,
                sample_start: None,
                sample_end: None,
                loop_mode: enable_mode,
                loop_start: None,
                loop_end: Some(new_le),
                loop_xfade_ms: None,
            })
        } else {
            let max_ls = cur_le.saturating_sub(16).max(start_b);
            let snapped = self.maybe_snap_frame(&sample, cursor, start_b, max_ls);
            let new_ls = snapped.clamp(start_b, max_ls);
            self.sample_ui.cursor_frame = new_ls;
            self.apply(Action::SetSamplerRegion {
                instrument: inst,
                zone,
                module,
                sample_start: None,
                sample_end: None,
                loop_mode: enable_mode,
                loop_start: Some(new_ls),
                loop_end: Some(cur_le),
                loop_xfade_ms: None,
            })
        }
    }

    fn cycle_sample_loop_mode(&mut self, inst: Id, zone: u16, module: u16) -> Result<Outcome> {
        let smp = self
            .sampler_module_at(inst, zone, module)
            .context("Sampler module missing")?
            .clone();
        let next = match smp.loop_mode {
            SampleLoopMode::Off => SampleLoopMode::Forward,
            SampleLoopMode::Forward => SampleLoopMode::PingPong,
            SampleLoopMode::PingPong => SampleLoopMode::Off,
        };
        self.apply(Action::SetSamplerRegion {
            instrument: inst,
            zone,
            module,
            sample_start: None,
            sample_end: None,
            loop_mode: Some(next),
            loop_start: None,
            loop_end: None,
            loop_xfade_ms: None,
        })
    }

    fn split_sample_at_cursor(&mut self, inst: Id, zone: u16, module: u16) -> Result<Outcome> {
        let smp = self
            .sampler_module_at(inst, zone, module)
            .context("Sampler module missing")?
            .clone();
        let sample = self
            .loaded_sample_for(inst, zone, module)
            .context("Load a sample first (press B)")?;
        let frames = sample.frames;
        ensure!(frames > 16, "Sample is too short to slice");
        let (start, end) = effective_sampler_bounds(&smp, frames);
        let snapped = self.maybe_snap_frame(
            &sample,
            self.sample_ui.cursor_frame,
            start,
            end.saturating_sub(1),
        );
        let mut slices = smp.slices.clone();
        if slices.is_empty() && snapped > start.saturating_add(64) {
            slices.push(SampleSlice::new(start, None));
        }
        if slices.iter().any(|s| s.frame.abs_diff(snapped) <= 8) {
            self.status = "A slice marker already sits at the cursor".into();
            return Ok(Outcome::default());
        }
        ensure!(
            slices.len() < MAX_SAMPLE_SLICES,
            "Maximum of {MAX_SAMPLE_SLICES} slice markers reached"
        );
        slices.push(SampleSlice::new(snapped, None));
        self.sample_ui.cursor_frame = snapped;
        self.apply(Action::SetSamplerSlices {
            instrument: inst,
            zone,
            module,
            slices,
        })
    }

    fn delete_sample_slice_at_cursor(
        &mut self,
        inst: Id,
        zone: u16,
        module: u16,
    ) -> Result<Outcome> {
        let smp = self
            .sampler_module_at(inst, zone, module)
            .context("Sampler module missing")?
            .clone();
        if smp.slices.is_empty() {
            self.status = "No slice markers to delete".into();
            return Ok(Outcome::default());
        }
        let cursor = self.sample_ui.cursor_frame;
        // Prefer exact/nearest marker at or before cursor; else nearest marker overall.
        let idx = smp
            .slices
            .iter()
            .enumerate()
            .min_by_key(|(_, s)| s.frame.abs_diff(cursor))
            .map(|(i, _)| i)
            .unwrap_or(0);
        let mut slices = smp.slices.clone();
        let removed = slices.remove(idx);
        let out = self.apply(Action::SetSamplerSlices {
            instrument: inst,
            zone,
            module,
            slices,
        })?;
        self.status = format!(
            "Deleted slice {} (frame {}) · {} remaining",
            idx + 1,
            removed.frame,
            self.sampler_module_at(inst, zone, module)
                .map_or(0, |s| s.slices.len())
        );
        Ok(out)
    }

    fn clear_sample_slices(&mut self, inst: Id, zone: u16, module: u16) -> Result<Outcome> {
        let smp = self
            .sampler_module_at(inst, zone, module)
            .context("Sampler module missing")?;
        if smp.slices.is_empty() {
            self.status = "No slice markers to clear".into();
            return Ok(Outcome::default());
        }
        self.apply(Action::SetSamplerSlices {
            instrument: inst,
            zone,
            module,
            slices: Vec::new(),
        })
    }

    pub(crate) fn auto_slice_transients(
        &mut self,
        inst: Id,
        zone: u16,
        module: u16,
    ) -> Result<Outcome> {
        let smp = self
            .sampler_module_at(inst, zone, module)
            .context("Sampler module missing")?
            .clone();
        let sample = self
            .loaded_sample_for(inst, zone, module)
            .context("Load a sample first (press B)")?;
        let (start, end) = effective_sampler_bounds(&smp, sample.frames);
        let frames = detect_transient_slices(
            &sample.left,
            sample.right.as_deref(),
            sample.sample_rate,
            start,
            end,
            self.sample_ui.sensitivity,
            self.sample_ui.min_gap_ms,
            MAX_SAMPLE_SLICES,
        );
        let slices: Vec<SampleSlice> = frames
            .into_iter()
            .map(|f| SampleSlice::new(f, None))
            .collect();
        let count = slices.len();
        let out = self.apply(Action::SetSamplerSlices {
            instrument: inst,
            zone,
            module,
            slices,
        })?;
        self.status = format!(
            "Auto-sliced {count} transient{} (sens {:.0}%, gap {:.0}ms) · K converts to Rack Kit",
            if count == 1 { "" } else { "s" },
            self.sample_ui.sensitivity * 100.0,
            self.sample_ui.min_gap_ms
        );
        Ok(out)
    }

    pub(crate) fn auto_slice_grid(&mut self, inst: Id, zone: u16, module: u16) -> Result<Outcome> {
        let smp = self
            .sampler_module_at(inst, zone, module)
            .context("Sampler module missing")?
            .clone();
        let sample = self
            .loaded_sample_for(inst, zone, module)
            .context("Load a sample first (press B)")?;
        let (start, end) = effective_sampler_bounds(&smp, sample.frames);
        let divs = self.sample_ui.grid_divisions.clamp(2, 32);
        let frames = detect_grid_slices(
            start,
            end,
            divs,
            &sample.left,
            sample.right.as_deref(),
            sample.sample_rate,
            self.sample_ui.snap_zero,
        );
        let slices: Vec<SampleSlice> = frames
            .into_iter()
            .map(|f| SampleSlice::new(f, None))
            .collect();
        let count = slices.len();
        let out = self.apply(Action::SetSamplerSlices {
            instrument: inst,
            zone,
            module,
            slices,
        })?;
        self.status = format!(
            "Grid-sliced into {count} equal region{} · K converts to Rack Kit",
            if count == 1 { "" } else { "s" }
        );
        Ok(out)
    }

    fn begin_rename_active_slice(&mut self, inst: Id, zone: u16, module: u16) -> Result<Outcome> {
        let smp = self
            .sampler_module_at(inst, zone, module)
            .context("Sampler module missing")?
            .clone();
        ensure!(
            !smp.slices.is_empty(),
            "No slice markers to rename · press s, d, or D first"
        );
        let frames = self
            .loaded_sample_for(inst, zone, module)
            .map_or(usize::MAX, |s| s.frames);
        let (start, end) = effective_sampler_bounds(&smp, frames);
        let regions = slice_regions(&smp.slices, start, end);
        let active_region =
            active_slice_index_for_cursor(&regions, self.sample_ui.cursor_frame).unwrap_or(0);
        let target_frame = regions[active_region].0;
        let slice_index = smp
            .slices
            .iter()
            .position(|s| s.frame == target_frame)
            .unwrap_or(0);
        let current = smp.slices[slice_index].label.clone().unwrap_or_default();
        self.begin_sample_slice_label_input(inst, zone, module, slice_index, current)
    }

    fn audition_active_sample_slice(
        &mut self,
        inst: Id,
        zone: u16,
        module: u16,
    ) -> Result<Outcome> {
        let smp = self
            .sampler_module_at(inst, zone, module)
            .context("Sampler module missing")?
            .clone();
        let sample = self
            .loaded_sample_for(inst, zone, module)
            .context("Load a sample first (press B)")?;
        let (start, end) = effective_sampler_bounds(&smp, sample.frames);
        let regions = slice_regions(&smp.slices, start, end);
        let (a_start, a_end, label) = if let Some(idx) =
            active_slice_index_for_cursor(&regions, self.sample_ui.cursor_frame)
        {
            let (s, e, lbl) = regions[idx];
            let name = lbl
                .map(str::to_string)
                .unwrap_or_else(|| format!("Slice {}", idx + 1));
            (s, e, name)
        } else {
            (start, end, "Sample".into())
        };
        let dur_ms =
            (a_end.saturating_sub(a_start) as f64 / f64::from(sample.sample_rate.max(1))) * 1000.0;
        self.status = format!("Auditioning {label} ({dur_ms:.0}ms)");
        Ok(Outcome {
            preview_sample: Some(SamplePreview::region(sample, a_start, a_end)),
            ..Outcome::default()
        })
    }

    fn handle_sample_deck_enter(&mut self, inst: Id, zone: u16, module: u16) -> Result<Outcome> {
        if self.loaded_sample_for(inst, zone, module).is_none() {
            return self.begin_sample_browser(BrowserTarget::AssignModule {
                instrument: inst,
                zone_id: zone,
                module_id: Some(module),
            });
        }
        match self.sample_ui.toolbar_field {
            SampleToolbarField::SnapZero => {
                self.sample_ui.snap_zero = !self.sample_ui.snap_zero;
                self.status = format!(
                    "Zero-crossing snap: {}",
                    if self.sample_ui.snap_zero {
                        "ON"
                    } else {
                        "OFF"
                    }
                );
                Ok(Outcome::default())
            }
            SampleToolbarField::Cursor => {
                if self
                    .sampler_module_at(inst, zone, module)
                    .is_some_and(|s| !s.slices.is_empty())
                {
                    self.begin_rename_active_slice(inst, zone, module)
                } else {
                    self.audition_active_sample_slice(inst, zone, module)
                }
            }
            SampleToolbarField::LoopXfade => {
                let addr = ParamAddress::Module {
                    instrument: inst,
                    zone,
                    module,
                    param: ModuleParam::LoopXfade,
                };
                self.begin_param_value_input(addr, String::new())
            }
            field @ (SampleToolbarField::Sensitivity
            | SampleToolbarField::MinGap
            | SampleToolbarField::GridDivisions) => self.begin_sample_toolbar_value_input(field),
        }
    }

    /// Tier 3 (`Alt+h/l`) and Tier 4 (`,/.` & `</>`) direct adjustment on the Sample deck.
    pub(crate) fn nudge_sample_deck(
        &mut self,
        dir: i8,
        coarse: bool,
        repeat: bool,
    ) -> Result<Outcome> {
        let Some((inst, zone, module)) = self.focused_sample_target() else {
            return Ok(Outcome::default());
        };
        let sign = if dir < 0 { -1.0f32 } else { 1.0f32 };
        match self.sample_ui.toolbar_field {
            SampleToolbarField::Sensitivity => {
                let step = if coarse { 0.15 } else { 0.05 };
                self.sample_ui.sensitivity =
                    (self.sample_ui.sensitivity + sign * step).clamp(0.05, 1.0);
                self.status = format!(
                    "Transient sensitivity: {:.0}% · press d to re-slice",
                    self.sample_ui.sensitivity * 100.0
                );
                Ok(Outcome::default())
            }
            SampleToolbarField::MinGap => {
                let step = if coarse { 50.0 } else { 10.0 };
                self.sample_ui.min_gap_ms =
                    (self.sample_ui.min_gap_ms + sign * step).clamp(10.0, 500.0);
                self.status = format!(
                    "Transient min gap: {:.0}ms · press d to re-slice",
                    self.sample_ui.min_gap_ms
                );
                Ok(Outcome::default())
            }
            SampleToolbarField::GridDivisions => {
                let step: isize = if coarse { 4 } else { 1 };
                let delta = if dir < 0 { -step } else { step };
                self.sample_ui.grid_divisions =
                    (self.sample_ui.grid_divisions as isize + delta).clamp(2, 32) as usize;
                self.status = format!(
                    "Grid divisions: {} · press D to grid-slice",
                    self.sample_ui.grid_divisions
                );
                Ok(Outcome::default())
            }
            SampleToolbarField::SnapZero => {
                if !repeat {
                    self.sample_ui.snap_zero = !self.sample_ui.snap_zero;
                    self.status = format!(
                        "Zero-crossing snap: {}",
                        if self.sample_ui.snap_zero {
                            "ON"
                        } else {
                            "OFF"
                        }
                    );
                }
                Ok(Outcome::default())
            }
            SampleToolbarField::LoopXfade => {
                let smp = self
                    .sampler_module_at(inst, zone, module)
                    .context("Sampler module missing")?
                    .clone();
                let step = if coarse { 5.0 } else { 1.0 };
                let next = (smp.loop_xfade_ms + sign * step).clamp(0.0, 50.0);
                self.commit_sampler_edit(
                    inst,
                    zone,
                    module,
                    |s| s.loop_xfade_ms = next,
                    format!("Loop crossfade → {next:.1}ms"),
                    Some((
                        CoalesceKey::SampleLoopXfade {
                            instrument: inst,
                            zone,
                            module,
                        },
                        repeat,
                    )),
                )
            }
            SampleToolbarField::Cursor => {
                self.nudge_active_slice_marker(inst, zone, module, dir, coarse, repeat)
            }
        }
    }

    fn nudge_active_slice_marker(
        &mut self,
        inst: Id,
        zone: u16,
        module: u16,
        dir: i8,
        coarse: bool,
        repeat: bool,
    ) -> Result<Outcome> {
        let smp = self
            .sampler_module_at(inst, zone, module)
            .context("Sampler module missing")?
            .clone();
        let Some(sample) = self.loaded_sample_for(inst, zone, module) else {
            self.status = "No sample loaded · press B to browse samples".into();
            return Ok(Outcome::default());
        };
        if smp.slices.is_empty() {
            self.status = "No slice marker to nudge · press s, d, or D to add slices".into();
            return Ok(Outcome::default());
        }
        let frames = sample.frames;
        let (start, end) = effective_sampler_bounds(&smp, frames);
        let (vs, ve) = sample_deck_window(
            self.sample_ui.cursor_frame,
            frames,
            self.sample_ui.zoom,
            self.sample_ui.view_start_frame.get(),
        );
        let vis = ve.saturating_sub(vs).max(1);
        let step = if coarse {
            (vis / 25).max(16)
        } else {
            (vis / 200).max(1)
        };
        let cursor = self.sample_ui.cursor_frame;
        let idx = smp
            .slices
            .iter()
            .enumerate()
            .min_by_key(|(_, s)| s.frame.abs_diff(cursor))
            .map(|(i, _)| i)
            .unwrap_or(0);
        let cur_frame = smp.slices[idx].frame;
        let min_f = if idx == 0 {
            start
        } else {
            smp.slices[idx - 1].frame.saturating_add(16)
        };
        let max_f = if idx + 1 < smp.slices.len() {
            smp.slices[idx + 1].frame.saturating_sub(16)
        } else {
            end.saturating_sub(16)
        };
        if min_f > max_f {
            return Ok(Outcome::default());
        }
        let raw_next = if dir < 0 {
            cur_frame.saturating_sub(step)
        } else {
            cur_frame.saturating_add(step)
        }
        .clamp(min_f, max_f);
        let next_frame = if self.sample_ui.snap_zero && !repeat {
            self.maybe_snap_frame(&sample, raw_next, min_f, max_f)
        } else {
            raw_next
        };
        if next_frame == cur_frame {
            return Ok(Outcome::default());
        }
        self.sample_ui.cursor_frame = next_frame;
        let (nvs, _) = sample_deck_window(next_frame, frames, self.sample_ui.zoom, vs);
        self.sample_ui.view_start_frame.set(nvs);
        let time_str = format_sample_time(next_frame, sample.sample_rate);
        self.commit_sampler_edit(
            inst,
            zone,
            module,
            |s| {
                if let Some(sl) = s.slices.get_mut(idx) {
                    sl.frame = next_frame;
                }
                normalize_slices(&mut s.slices, Some(frames));
            },
            format!("Moved slice {} → {time_str} (f{next_frame})", idx + 1),
            Some((
                CoalesceKey::SampleSliceMove {
                    instrument: inst,
                    zone,
                    module,
                    index: idx,
                },
                repeat,
            )),
        )
    }

    /// Universal reset (`Alt+Backspace`) on the Sample deck.
    pub(crate) fn reset_sample_deck_focused(&mut self) -> Result<Outcome> {
        let Some((inst, zone, module)) = self.focused_sample_target() else {
            return Ok(Outcome::default());
        };
        match self.sample_ui.toolbar_field {
            SampleToolbarField::Sensitivity => {
                self.sample_ui.sensitivity = 0.55;
                self.status = "Transient sensitivity reset → 55%".into();
                Ok(Outcome::default())
            }
            SampleToolbarField::MinGap => {
                self.sample_ui.min_gap_ms = 50.0;
                self.status = "Transient min gap reset → 50ms".into();
                Ok(Outcome::default())
            }
            SampleToolbarField::GridDivisions => {
                self.sample_ui.grid_divisions = 8;
                self.status = "Grid divisions reset → 8".into();
                Ok(Outcome::default())
            }
            SampleToolbarField::SnapZero => {
                self.sample_ui.snap_zero = true;
                self.status = "Zero-crossing snap reset → ON".into();
                Ok(Outcome::default())
            }
            SampleToolbarField::LoopXfade => self.apply(Action::SetSamplerRegion {
                instrument: inst,
                zone,
                module,
                sample_start: None,
                sample_end: None,
                loop_mode: None,
                loop_start: None,
                loop_end: None,
                loop_xfade_ms: Some(4.0),
            }),
            SampleToolbarField::Cursor => self.apply(Action::SetSamplerRegion {
                instrument: inst,
                zone,
                module,
                sample_start: Some(0),
                sample_end: Some(0),
                loop_mode: Some(SampleLoopMode::Off),
                loop_start: Some(0),
                loop_end: Some(0),
                loop_xfade_ms: None,
            }),
        }
    }

    /// Helper mutating a single `SamplerModule` inside an instrument's `RackPatch`
    /// through the validated `Document::commit` pipeline.
    fn commit_sampler_edit(
        &mut self,
        instrument: Id,
        zone: u16,
        module: u16,
        mutate: impl FnOnce(&mut SamplerModule),
        label: String,
        coalesce: Option<(CoalesceKey, bool)>,
    ) -> Result<Outcome> {
        let mut rack = self
            .instrument(instrument)
            .context("Instrument missing")?
            .rack
            .clone();
        let z = rack.zone_mut(zone).context("Rack zone missing")?;
        let m = z.module_mut(module).context("Rack module missing")?;
        match &mut m.kind {
            ModuleKind::Sampler(smp) => mutate(smp),
            _ => bail!("Module {} is not a Sampler", m.name),
        }
        let edits = vec![Edit::UpdateRackPatch {
            id: instrument,
            rack,
        }];
        match coalesce {
            Some((key, merge)) => self.commit_coalesced(edits, label, key, merge),
            None => self.commit(edits, label),
        }
    }

    /// Execute `Action::SetSamplerRegion`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn apply_set_sampler_region(
        &mut self,
        instrument: Id,
        zone: u16,
        module: u16,
        sample_start: Option<usize>,
        sample_end: Option<usize>,
        loop_mode: Option<SampleLoopMode>,
        loop_start: Option<usize>,
        loop_end: Option<usize>,
        loop_xfade_ms: Option<f32>,
    ) -> Result<Outcome> {
        let frames = self
            .loaded_sample_for(instrument, zone, module)
            .map_or(0, |s| s.frames);
        let sr = self
            .loaded_sample_for(instrument, zone, module)
            .map_or(0, |s| s.sample_rate);
        let mut summary = Vec::new();
        if let Some(s) = sample_start {
            summary.push(format!("start {}", format_sample_time(s, sr)));
        }
        if let Some(e) = sample_end {
            summary.push(if e == 0 {
                "end full".into()
            } else {
                format!("end {}", format_sample_time(e, sr))
            });
        }
        if let Some(lm) = loop_mode {
            summary.push(format!("loop {}", lm.label()));
        }
        if let Some(ls) = loop_start {
            summary.push(format!("loop start {}", format_sample_time(ls, sr)));
        }
        if let Some(le) = loop_end {
            summary.push(format!("loop end {}", format_sample_time(le, sr)));
        }
        if let Some(xf) = loop_xfade_ms {
            summary.push(format!("xfade {xf:.1}ms"));
        }
        let label = if summary.is_empty() {
            "Updated Sampler region".into()
        } else {
            format!("Sampler region: {}", summary.join(" · "))
        };
        self.commit_sampler_edit(
            instrument,
            zone,
            module,
            |smp| {
                if let Some(s) = sample_start {
                    smp.sample_start = if frames > 0 {
                        s.min(frames.saturating_sub(1))
                    } else {
                        s
                    };
                }
                if let Some(e) = sample_end {
                    smp.sample_end = if frames > 0 && e > 0 {
                        e.clamp(smp.sample_start.saturating_add(1), frames)
                    } else {
                        e
                    };
                }
                if smp.sample_end > 0 && smp.sample_start >= smp.sample_end {
                    smp.sample_start = smp.sample_end.saturating_sub(1);
                }
                if let Some(lm) = loop_mode {
                    smp.loop_mode = lm;
                }
                if let Some(ls) = loop_start {
                    smp.loop_start = if frames > 0 {
                        ls.min(frames.saturating_sub(1))
                    } else {
                        ls
                    };
                }
                if let Some(le) = loop_end {
                    smp.loop_end = if frames > 0 && le > 0 {
                        le.clamp(smp.loop_start.saturating_add(1), frames)
                    } else {
                        le
                    };
                }
                if smp.loop_end > 0 && smp.loop_start >= smp.loop_end {
                    smp.loop_start = smp.loop_end.saturating_sub(1);
                }
                if let Some(xf) = loop_xfade_ms {
                    smp.loop_xfade_ms = xf.clamp(0.0, 50.0);
                }
            },
            label,
            None,
        )
    }

    /// Execute `Action::SetSamplerSlices`.
    pub(crate) fn apply_set_sampler_slices(
        &mut self,
        instrument: Id,
        zone: u16,
        module: u16,
        slices: Vec<SampleSlice>,
    ) -> Result<Outcome> {
        let frames = self
            .loaded_sample_for(instrument, zone, module)
            .map(|s| s.frames);
        let mut normalized = slices;
        normalize_slices(&mut normalized, frames);
        let count = normalized.len();
        let label = if count == 0 {
            "Cleared sample slices".into()
        } else {
            format!(
                "Updated sample slices ({count} marker{})",
                if count == 1 { "" } else { "s" }
            )
        };
        self.commit_sampler_edit(
            instrument,
            zone,
            module,
            |smp| smp.slices = normalized,
            label,
            None,
        )
    }

    /// Execute `Action::SliceToRackKit` in a single atomic undoable transaction.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn apply_slice_to_rack_kit(
        &mut self,
        instrument: Id,
        zone: u16,
        module: u16,
        replace_existing: bool,
        create_pattern: bool,
        choke_group: Option<u8>,
        kit_name: Option<String>,
    ) -> Result<Outcome> {
        let source_inst = self
            .instrument(instrument)
            .context("Source instrument missing")?
            .clone();
        let source_smp = self
            .sampler_module_at(instrument, zone, module)
            .context("Source Sampler module missing")?
            .clone();
        let sample = self
            .loaded_sample_for(instrument, zone, module)
            .context("Load a sample before slicing to a Rack kit")?;
        ensure!(
            !source_smp.slices.is_empty(),
            "No slice markers yet · press d (auto), D (grid), or s (split) first"
        );
        let frames = sample.frames;
        let (crop_start, crop_end) = effective_sampler_bounds(&source_smp, frames);
        let regions = slice_regions(&source_smp.slices, crop_start, crop_end);
        let pad_count = regions.len().min(MAX_ZONES);
        ensure!(pad_count > 0, "Sample has no playable slice regions");

        let base_label = kit_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                if !sample.name.trim().is_empty() {
                    sample.name.clone()
                } else {
                    source_inst.name.clone()
                }
            });
        let kit_rack = RackPatch::from_sample_slices(
            &base_label,
            &source_smp,
            frames,
            choke_group.or(Some(1)),
        )?;

        let mut edits = Vec::new();
        let project = self.doc.project();
        let mut id_count = 0usize;
        if !replace_existing {
            id_count += 2; // channel + instrument
        }
        if create_pattern {
            id_count += 4; // pattern + part + lane + placement
        }
        let ids = self.doc.fresh_ids(id_count)?;
        let mut id_cursor = 0usize;

        let (target_inst_id, inst_display_name) = if replace_existing {
            edits.push(Edit::UpdateRackPatch {
                id: instrument,
                rack: kit_rack,
            });
            (instrument, source_inst.name.clone())
        } else {
            let ch_id = ids[id_cursor];
            let inst_id = ids[id_cursor + 1];
            id_cursor += 2;
            let raw_name = if base_label.to_ascii_lowercase().ends_with("kit") {
                base_label.clone()
            } else {
                format!("{base_label} Kit")
            };
            let inst_name = crate::unique_name(
                &raw_name,
                project.instruments.iter().map(|i| i.name.as_str()),
                None,
            );
            let ch_name = crate::unique_name(
                &inst_name,
                project.mixer.channels.iter().map(|c| c.name.as_str()),
                None,
            );
            edits.push(Edit::CreateChannel {
                channel: Channel::new(ch_id, ChannelKind::Audio, ch_name),
            });
            edits.push(Edit::CreateInstrument {
                instrument: InstrumentInstance {
                    id: inst_id,
                    name: inst_name.clone(),
                    rack: kit_rack,
                    channel: ch_id,
                },
            });
            (inst_id, inst_name)
        };

        let mut created_pat: Option<(Id, Id, Id, Id, usize)> = None;
        if create_pattern {
            let pat_id = ids[id_cursor];
            let part_id = ids[id_cursor + 1];
            let lane_id = ids[id_cursor + 2];
            let placement_id = ids[id_cursor + 3];

            // Build a musically timed sequential chromatic slice pattern (C2 = MIDI 36 + i).
            let crop_span = crop_end.saturating_sub(crop_start).max(1);
            let step_secs = 60.0 / project.bpm.max(20.0) / 4.0;
            let crop_secs = crop_span as f64 / f64::from(sample.sample_rate.max(1));
            let raw_steps = (crop_secs / step_secs).round() as usize;
            let musical_len = if raw_steps <= 8 {
                8usize
            } else if raw_steps <= 16 {
                16
            } else if raw_steps <= 32 {
                32
            } else {
                raw_steps.div_ceil(16) * 16
            };
            let pat_len = musical_len
                .max(pad_count)
                .clamp(1, material::MAX_LITERAL_STEPS);
            let mut steps = vec![Step::default(); pat_len];

            // Place each slice at its proportional musical step (resolving collisions forward).
            let mut last_step: Option<usize> = None;
            let mut placed_steps: Vec<(usize, usize)> = Vec::with_capacity(pad_count);
            for (i, &(r_start, _, _)) in regions.iter().take(pad_count).enumerate() {
                let rel = (r_start.saturating_sub(crop_start)) as f64 / crop_span as f64;
                let ideal = (rel * pat_len as f64).round() as usize;
                let step_idx = match last_step {
                    None => ideal.min(pat_len - 1),
                    Some(prev) => ideal.max(prev + 1).min(pat_len - 1),
                };
                if steps[step_idx].active {
                    // Fallback: tightly pack remaining slices if crowded near the end.
                    if let Some(free) = steps.iter().position(|s| !s.active) {
                        placed_steps.push((free, i));
                        steps[free].active = true;
                        last_step = Some(free);
                    }
                } else {
                    placed_steps.push((step_idx, i));
                    steps[step_idx].active = true;
                    last_step = Some(step_idx);
                }
            }
            for idx_pos in 0..placed_steps.len() {
                let (step_idx, slice_i) = placed_steps[idx_pos];
                let next_step = placed_steps
                    .get(idx_pos + 1)
                    .map(|&(s, _)| s)
                    .unwrap_or(pat_len);
                let gate = (next_step.saturating_sub(step_idx).max(1) as f32).clamp(0.5, 16.0);
                let midi = (36usize + slice_i).min(127) as u8;
                let (degree, accidental) = project.midi_to_degree(midi);
                steps[step_idx] = Step {
                    active: true,
                    degree,
                    accidental,
                    velocity: 0.9,
                    gate,
                    ..Step::default()
                };
            }

            let pat_name = crate::unique_name(
                &format!("{inst_display_name} Slices"),
                project.patterns.iter().map(|p| p.name.as_str()),
                None,
            );
            let part_name = crate::unique_name(
                &inst_display_name,
                project.parts.iter().map(|p| p.name.as_str()),
                None,
            );
            let lane_name = crate::unique_name(
                &inst_display_name,
                project.lanes.iter().map(|l| l.name.as_str()),
                None,
            );
            let start_step = self.arrange_ui.step / 16 * 16;
            let length_steps = (pat_len as u32).max(16).div_ceil(16) * 16;
            let needed_bars = ((start_step + length_steps) / 16).max(1) as usize;
            if needed_bars > project.bars && needed_bars <= material::MAX_BARS {
                edits.push(Edit::SetBars { bars: needed_bars });
            }

            edits.push(Edit::PutPattern {
                pattern: Pattern {
                    id: pat_id,
                    name: pat_name,
                    kind: PatternKind::Notes { steps },
                },
            });
            edits.push(Edit::PutPart {
                part: Part {
                    id: part_id,
                    name: part_name,
                    pattern: pat_id,
                    instrument: target_inst_id,
                    transpose: 0,
                    velocity_scale: 1.0,
                    parameter_links: Vec::new(),
                },
            });
            edits.push(Edit::PutLane {
                lane: Lane {
                    id: lane_id,
                    name: lane_name,
                    parts: vec![LanePart {
                        part: part_id,
                        phase: PhaseMode::Restart,
                        offset_steps: 0,
                    }],
                    placements: vec![Placement {
                        id: placement_id,
                        part: part_id,
                        start_step,
                        length_steps,
                        phase: PhaseMode::Restart,
                        offset_steps: 0,
                        exceptions: Vec::new(),
                    }],
                },
            });
            created_pat = Some((pat_id, part_id, lane_id, placement_id, pat_len));
        }

        let truncation = if regions.len() > MAX_ZONES {
            format!(" (capped from {} to {MAX_ZONES} zones)", regions.len())
        } else {
            String::new()
        };
        let label = match created_pat {
            Some((_, _, _, _, pat_len)) => format!(
                "Sliced {pad_count} regions → '{inst_display_name}' ({pad_count} zones + {pat_len}-step pattern){truncation}"
            ),
            None => format!("Sliced {pad_count} regions → '{inst_display_name}' ({pad_count} zones){truncation}"),
        };

        let mut out = self.commit(edits, label)?;
        self.sync_loaded_samples();
        out.samples_changed = true;
        if let Some((pat_id, part_id, lane_id, placement_id, _)) = created_pat {
            self.selected_pattern = Some(pat_id);
            self.selected_part = Some(part_id);
            self.selected_lane = Some(lane_id);
            self.selected_placement = Some(placement_id);
        }
        self.selected_instrument = Some(target_inst_id);
        self.rack_ui.zone_cursor = 0;
        self.rack_ui.panel_slot = 0;
        self.rack_ui.tab = RackTab::Zones;
        self.open_instrument(target_inst_id)?;
        Ok(out)
    }

    /// Execute `Action::WriteSampleRiffMetadata`: losslessly rewrite RIFF `inst`/`smpl`/`cue `/`LIST/adtl/labl`
    /// chunks inside the source WAV file via `HostIo` without touching audio PCM/compressed frames.
    pub(crate) fn apply_write_sample_riff_metadata(
        &mut self,
        instrument: Id,
        zone: u16,
        module: u16,
    ) -> Result<Outcome> {
        let smp = self
            .sampler_module_at(instrument, zone, module)
            .context("Sampler module missing")?
            .clone();
        let locator = smp
            .path
            .as_deref()
            .context("Sampler has no audio file assigned")?;
        let sample = self
            .loaded_sample_for(instrument, zone, module)
            .context("Sample is not loaded")?;
        let resolved = sample.resolved_path.clone();
        ensure!(
            is_wav_locator(Some(locator))
                || resolved
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("wav")),
            "RIFF cue/loop metadata can only be written to WAV files (not '{}')",
            resolved
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(locator)
        );
        let existing = self
            .io
            .read_bounded(&resolved, MAX_SAMPLE_FILE_BYTES)
            .with_context(|| format!("Failed reading WAV '{}'", resolved.display()))?;
        let base_meta = Some(&sample.meta);
        let meta: SampleMetadata = smp.to_riff_metadata(sample.frames, base_meta);
        let updated =
            update_wav_riff_metadata_bytes(&existing, sample.sample_rate, sample.frames, &meta)?;
        self.io
            .write_atomic(&resolved, MAX_SAMPLE_FILE_BYTES, &updated)
            .with_context(|| format!("Failed writing WAV metadata to '{}'", resolved.display()))?;

        // Invalidate cached decode and refresh sample bank so `sample.metadata` reflects the written chunks.
        self.assets.library.invalidate_decoded_cache();
        self.sync_loaded_samples();
        let fname = resolved
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(locator);
        self.status = format!(
            "Wrote RIFF metadata to '{fname}' ({} slice{} · loop {})",
            meta.slices.len(),
            if meta.slices.len() == 1 { "" } else { "s" },
            meta.loop_mode.label()
        );
        Ok(Outcome {
            samples_changed: true,
            ..Outcome::default()
        })
    }

    /// Execute `:slice <subcommand>` Ex commands.
    pub(crate) fn execute_slice_command(&mut self, args: &[String]) -> Result<Outcome> {
        let usage = "Usage: :slice auto [sens] [min_gap_ms] | grid <N> | add [frame] [label] | rename <idx> <label> | del <idx> | clear | kit [replace] [nopattern]";
        ensure!(!args.is_empty(), "{usage}");
        let (inst, zone, module) = self
            .resolve_sample_deck_target()
            .context("No Sampler instrument in project")?;
        let sub = args[0].to_ascii_lowercase();
        match sub.as_str() {
            "auto" | "transients" | "detect" => {
                if let Some(s) = args.get(1) {
                    let v: f32 = s
                        .trim_end_matches('%')
                        .parse()
                        .context("Invalid sensitivity")?;
                    let norm = if s.ends_with('%') || v > 1.0 {
                        v / 100.0
                    } else {
                        v
                    };
                    self.sample_ui.sensitivity = norm.clamp(0.05, 1.0);
                }
                if let Some(g) = args.get(2) {
                    let ms: f32 = g
                        .trim_end_matches("ms")
                        .parse()
                        .context("Invalid min gap ms")?;
                    self.sample_ui.min_gap_ms = ms.clamp(10.0, 500.0);
                }
                self.auto_slice_transients(inst, zone, module)
            }
            "grid" | "equal" => {
                ensure!(args.len() == 2, "Usage: :slice grid <2..=32>");
                let divs: usize = args[1].parse().context("Grid divisions must be 2..=32")?;
                ensure!((2..=32).contains(&divs), "Grid divisions must be 2..=32");
                self.sample_ui.grid_divisions = divs;
                self.auto_slice_grid(inst, zone, module)
            }
            "add" | "split" => {
                let smp = self
                    .sampler_module_at(inst, zone, module)
                    .context("Sampler missing")?
                    .clone();
                let sample = self
                    .loaded_sample_for(inst, zone, module)
                    .context("Sample is not loaded")?;
                let frame = if let Some(tok) = args.get(1) {
                    if let Some(ms) = tok.strip_suffix("ms") {
                        let v: f64 = ms.parse().context("Invalid ms offset")?;
                        ((v / 1000.0) * f64::from(sample.sample_rate))
                            .round()
                            .max(0.0) as usize
                    } else if let Some(s) = tok.strip_suffix('s') {
                        let v: f64 = s.parse().context("Invalid seconds offset")?;
                        (v * f64::from(sample.sample_rate)).round().max(0.0) as usize
                    } else {
                        tok.parse::<usize>().context("Invalid frame index")?
                    }
                } else {
                    self.sample_ui.cursor_frame
                };
                let label = if args.len() >= 3 {
                    let joined = args[2..].join(" ");
                    let trimmed = joined.trim();
                    (!trimmed.is_empty()).then(|| trimmed.chars().take(48).collect())
                } else {
                    None
                };
                let mut slices = smp.slices.clone();
                ensure!(
                    slices.len() < MAX_SAMPLE_SLICES,
                    "Maximum of {MAX_SAMPLE_SLICES} slices reached"
                );
                slices.push(SampleSlice {
                    frame: frame.min(sample.frames.saturating_sub(1)),
                    label,
                });
                self.apply(Action::SetSamplerSlices {
                    instrument: inst,
                    zone,
                    module,
                    slices,
                })
            }
            "rename" | "label" => {
                ensure!(
                    args.len() >= 3,
                    "Usage: :slice rename <1-based-index> <label>"
                );
                let one_based: usize = args[1].parse().context("Slice index must be 1..=64")?;
                ensure!(one_based >= 1, "Slice index is 1-based");
                let smp = self
                    .sampler_module_at(inst, zone, module)
                    .context("Sampler missing")?
                    .clone();
                let idx = one_based - 1;
                ensure!(
                    idx < smp.slices.len(),
                    "Slice {one_based} out of range (1..={})",
                    smp.slices.len()
                );
                let mut slices = smp.slices.clone();
                let raw = args[2..].join(" ");
                let trimmed = raw.trim();
                slices[idx].label =
                    (!trimmed.is_empty()).then(|| trimmed.chars().take(48).collect());
                self.apply(Action::SetSamplerSlices {
                    instrument: inst,
                    zone,
                    module,
                    slices,
                })
            }
            "del" | "delete" | "rm" => {
                ensure!(args.len() == 2, "Usage: :slice del <1-based-index>");
                let one_based: usize = args[1].parse().context("Slice index must be 1..=64")?;
                ensure!(one_based >= 1, "Slice index is 1-based");
                let smp = self
                    .sampler_module_at(inst, zone, module)
                    .context("Sampler missing")?
                    .clone();
                let idx = one_based - 1;
                ensure!(
                    idx < smp.slices.len(),
                    "Slice {one_based} out of range (1..={})",
                    smp.slices.len()
                );
                let mut slices = smp.slices.clone();
                slices.remove(idx);
                self.apply(Action::SetSamplerSlices {
                    instrument: inst,
                    zone,
                    module,
                    slices,
                })
            }
            "clear" => self.apply(Action::SetSamplerSlices {
                instrument: inst,
                zone,
                module,
                slices: Vec::new(),
            }),
            "kit" => {
                let mut replace_existing = false;
                let mut create_pattern = true;
                for flag in &args[1..] {
                    match flag.to_ascii_lowercase().as_str() {
                        "replace" => replace_existing = true,
                        "nopattern" | "no-pattern" | "kit-only" => create_pattern = false,
                        "pattern" => create_pattern = true,
                        other => {
                            bail!("Unknown :slice kit option '{other}' (use replace, nopattern)")
                        }
                    }
                }
                self.apply(Action::SliceToRackKit {
                    instrument: inst,
                    zone,
                    module,
                    replace_existing,
                    create_pattern,
                    choke_group: Some(1),
                    kit_name: None,
                })
            }
            _ => bail!("{usage}"),
        }
    }

    /// Execute `:sample write-meta` Ex command.
    pub(crate) fn execute_sample_write_meta_command(&mut self) -> Result<Outcome> {
        let (inst, zone, module) = self
            .resolve_sample_deck_target()
            .context("No Sampler instrument in project")?;
        self.apply(Action::WriteSampleRiffMetadata {
            instrument: inst,
            zone,
            module,
        })
    }
}
