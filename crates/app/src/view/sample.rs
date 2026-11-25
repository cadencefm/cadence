//! Semantic view projection for the dedicated Sample / Waveform deck (`DeckKind::Sample`).
use super::*;
use crate::sample_deck::{
    active_slice_index_for_cursor, effective_sampler_bounds, is_wav_locator, sample_deck_window,
};
use crate::SampleToolbarField;
use cadence_core::dsp::{compute_waveform_peaks_window, slice_regions};
use cadence_core::material::Id;
use cadence_core::rack::{ModuleKind, SampleLoopMode, MAX_ZONES};
use std::path::Path;

const WAVEFORM_DETAIL_BINS: usize = 512;
const WAVEFORM_OVERVIEW_BINS: usize = 160;

#[derive(Clone, Debug)]
pub struct SampleSliceView {
    pub index: usize,
    pub frame: usize,
    pub end_frame: usize,
    pub label: Option<String>,
    pub midi_note: Option<u8>,
    pub active: bool,
}

#[derive(Clone, Debug)]
pub struct SampleDeckView {
    pub instrument_id: Id,
    pub zone_id: u16,
    pub module_id: u16,
    pub instrument_name: String,
    pub zone_name: String,
    pub module_name: String,
    pub sample_path: Option<String>,
    pub sample_name: String,
    pub loaded: bool,
    pub load_error: Option<String>,
    pub is_wav_file: bool,
    pub has_riff_cues: bool,
    pub riff_sync_dirty: bool,
    pub sample_rate: u32,
    pub channels: u8,
    pub total_frames: usize,
    pub bpm: f64,
    pub sample_start: usize,
    pub sample_end: usize,
    pub loop_mode: SampleLoopMode,
    pub loop_start: usize,
    pub loop_end: usize,
    pub loop_xfade_ms: f32,
    pub root_midi: u8,
    pub view_start_frame: usize,
    pub view_end_frame: usize,
    pub zoom: f32,
    pub cursor_frame: usize,
    pub peaks: Vec<(f32, f32)>,
    pub overview_peaks: Vec<(f32, f32)>,
    pub slices: Vec<SampleSliceView>,
    pub active_slice_idx: Option<usize>,
    pub toolbar_field: SampleToolbarField,
    pub sensitivity: f32,
    pub min_gap_ms: f32,
    pub grid_divisions: usize,
    pub snap_zero: bool,
}

impl App {
    pub(super) fn build_sample_deck_view(
        &self,
        instrument_id: Id,
        zone_id: u16,
        module_id: u16,
    ) -> SampleDeckView {
        let inst = self.instrument(instrument_id);
        let zone = inst.and_then(|i| i.rack.zone(zone_id));
        let module = zone.and_then(|z| z.module(module_id));
        let smp = module.and_then(|m| match &m.kind {
            ModuleKind::Sampler(s) => Some(s),
            _ => None,
        });

        let instrument_name = inst.map_or_else(|| "Sampler".into(), |i| i.name.clone());
        let zone_name = zone.map_or_else(|| "Zone".into(), |z| z.name.clone());
        let module_name = module.map_or_else(|| "Sampler".into(), |m| m.name.clone());
        let sample_path = smp.and_then(|s| s.path.clone());
        let bpm = self.project().bpm;
        let sample = self.loaded_sample_for(instrument_id, zone_id, module_id);

        let load_error = sample_path.as_deref().and_then(|locator| {
            self.assets
                .issues
                .iter()
                .find(|iss| {
                    iss.instrument_id == instrument_id && iss.path.as_deref() == Some(locator)
                })
                .map(|iss| iss.message.clone())
        });

        let Some(smp) = smp else {
            return SampleDeckView {
                instrument_id,
                zone_id,
                module_id,
                instrument_name,
                zone_name,
                module_name,
                sample_path,
                sample_name: "(missing sampler)".into(),
                loaded: false,
                load_error,
                is_wav_file: false,
                has_riff_cues: false,
                riff_sync_dirty: false,
                sample_rate: 0,
                channels: 0,
                total_frames: 0,
                bpm,
                sample_start: 0,
                sample_end: 0,
                loop_mode: SampleLoopMode::Off,
                loop_start: 0,
                loop_end: 0,
                loop_xfade_ms: 4.0,
                root_midi: 60,
                view_start_frame: 0,
                view_end_frame: 0,
                zoom: 1.0,
                cursor_frame: 0,
                peaks: Vec::new(),
                overview_peaks: Vec::new(),
                slices: Vec::new(),
                active_slice_idx: None,
                toolbar_field: self.sample_ui.toolbar_field,
                sensitivity: self.sample_ui.sensitivity,
                min_gap_ms: self.sample_ui.min_gap_ms,
                grid_divisions: self.sample_ui.grid_divisions,
                snap_zero: self.sample_ui.snap_zero,
            };
        };

        let Some(sample) = sample else {
            let sample_name = sample_path
                .as_deref()
                .and_then(|p| Path::new(p).file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("No sample loaded")
                .to_string();
            return SampleDeckView {
                instrument_id,
                zone_id,
                module_id,
                instrument_name,
                zone_name,
                module_name,
                is_wav_file: is_wav_locator(sample_path.as_deref()),
                sample_path,
                sample_name,
                loaded: false,
                load_error,
                has_riff_cues: false,
                riff_sync_dirty: false,
                sample_rate: 0,
                channels: 0,
                total_frames: 0,
                bpm,
                sample_start: smp.sample_start,
                sample_end: smp.sample_end,
                loop_mode: smp.loop_mode,
                loop_start: smp.loop_start,
                loop_end: smp.loop_end,
                loop_xfade_ms: smp.loop_xfade_ms,
                root_midi: smp.root_midi,
                view_start_frame: 0,
                view_end_frame: 0,
                zoom: 1.0,
                cursor_frame: 0,
                peaks: Vec::new(),
                overview_peaks: Vec::new(),
                slices: Vec::new(),
                active_slice_idx: None,
                toolbar_field: self.sample_ui.toolbar_field,
                sensitivity: self.sample_ui.sensitivity,
                min_gap_ms: self.sample_ui.min_gap_ms,
                grid_divisions: self.sample_ui.grid_divisions,
                snap_zero: self.sample_ui.snap_zero,
            };
        };

        let target = (instrument_id, zone_id, module_id);
        let total_frames = sample.frames;
        let (sample_start, sample_end) = effective_sampler_bounds(smp, total_frames);
        let loop_start = smp.loop_start.min(total_frames.saturating_sub(1));
        let loop_end = if smp.loop_end == 0 {
            sample_end
        } else {
            smp.loop_end
                .clamp(loop_start.saturating_add(1), total_frames)
        };
        let cursor_frame = self
            .sample_ui
            .cursor_frame
            .min(total_frames.saturating_sub(1));
        let zoom = self.sample_ui.zoom;
        let prev_start = if self.sample_ui.last_target.get() == Some(target) {
            self.sample_ui.view_start_frame.get()
        } else {
            0
        };
        let (view_start_frame, view_end_frame) =
            sample_deck_window(cursor_frame, total_frames, zoom, prev_start);
        if self.sample_ui.last_target.get() == Some(target) {
            self.sample_ui.view_start_frame.set(view_start_frame);
        }

        let peaks = compute_waveform_peaks_window(
            &sample.left,
            sample.right.as_deref(),
            view_start_frame,
            view_end_frame,
            WAVEFORM_DETAIL_BINS,
        );
        let overview_peaks = if sample.peaks.len() <= WAVEFORM_OVERVIEW_BINS {
            sample.peaks.to_vec()
        } else {
            compute_waveform_peaks_window(
                &sample.left,
                sample.right.as_deref(),
                0,
                total_frames,
                WAVEFORM_OVERVIEW_BINS,
            )
        };

        let regions = slice_regions(&smp.slices, sample_start, sample_end);
        let active_slice_idx = if smp.slices.is_empty() {
            None
        } else {
            active_slice_index_for_cursor(&regions, cursor_frame)
        };
        let slices: Vec<SampleSliceView> = if smp.slices.is_empty() {
            Vec::new()
        } else {
            regions
                .iter()
                .enumerate()
                .map(|(i, &(rs, re, lbl))| SampleSliceView {
                    index: i,
                    frame: rs,
                    end_frame: re,
                    label: lbl.map(str::to_string),
                    midi_note: (i < MAX_ZONES).then_some((36 + i).min(127) as u8),
                    active: active_slice_idx == Some(i),
                })
                .collect()
        };

        let is_wav_file = is_wav_locator(sample_path.as_deref())
            || sample
                .resolved_path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("wav"));
        let has_riff_cues = !sample.meta.slices.is_empty();
        let disk_meta = &sample.meta;
        let current_meta = smp.to_riff_metadata(total_frames, Some(disk_meta));
        let riff_sync_dirty = is_wav_file
            && (current_meta.slices != disk_meta.slices
                || current_meta.loop_mode != disk_meta.loop_mode
                || (current_meta.loop_mode != SampleLoopMode::Off
                    && (current_meta.loop_start != disk_meta.loop_start
                        || current_meta.loop_end != disk_meta.loop_end)));

        SampleDeckView {
            instrument_id,
            zone_id,
            module_id,
            instrument_name,
            zone_name,
            module_name,
            sample_path,
            sample_name: sample.name.clone(),
            loaded: true,
            load_error,
            is_wav_file,
            has_riff_cues,
            riff_sync_dirty,
            sample_rate: sample.sample_rate,
            channels: sample.channels,
            total_frames,
            bpm,
            sample_start,
            sample_end,
            loop_mode: smp.loop_mode,
            loop_start,
            loop_end,
            loop_xfade_ms: smp.loop_xfade_ms,
            root_midi: smp.root_midi,
            view_start_frame,
            view_end_frame,
            zoom,
            cursor_frame,
            peaks,
            overview_peaks,
            slices,
            active_slice_idx,
            toolbar_field: self.sample_ui.toolbar_field,
            sensitivity: self.sample_ui.sensitivity,
            min_gap_ms: self.sample_ui.min_gap_ms,
            grid_divisions: self.sample_ui.grid_divisions,
            snap_zero: self.sample_ui.snap_zero,
        }
    }
}
