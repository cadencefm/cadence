//! End-to-end integration tests for the dedicated Sample / Waveform deck (`gw`),
//! RIFF `cue `/`labl` roundtrip read & lossless write-back, transient/grid slicing,
//! active-slice region audition, and atomic Slice-to-Rack-Kit conversion.

use cadence_app::layout::DeckKind;
use cadence_app::view::DeckView;
use cadence_app::{Action, App, RackTab, SampleToolbarField};
use cadence_core::material::Project;
use cadence_core::rack::{ModuleKind, SampleLoopMode};
use cadence_core::runtime::AudioSnapshot;
use cadence_core::sampler::{
    load_sample_file, write_wav_with_metadata, SampleLibrary, SampleMetadata, SampleSlice,
};
use std::path::Path;

/// Synthesize a crisp 4-hit stereo percussive loop (`Kick`, `Hat`, `Snare`, `Perc`) at 44.1 kHz
/// with sharp transients and silent gaps between hits, optionally embedding RIFF cue/labl chunks.
fn write_four_hit_break_wav(
    path: &Path,
    sample_rate: u32,
    with_cues: bool,
) -> (Vec<f32>, Vec<f32>) {
    let frames = sample_rate as usize * 2; // 2.0 seconds = 1 bar at 120 BPM
    let mut left = vec![0.0f32; frames];
    let mut right = vec![0.0f32; frames];
    let hit_starts = [0usize, frames / 4, frames / 2, (frames * 3) / 4];
    let freqs = [62.0f32, 3_400.0, 210.0, 880.0];

    for (hit_i, &start) in hit_starts.iter().enumerate() {
        let dur = (sample_rate as usize / 6).min(frames - start);
        let base_f = freqs[hit_i];
        for i in 0..dur {
            let t = i as f32 / sample_rate as f32;
            let env = (-t * 28.0).exp();
            let pitch = if hit_i == 0 {
                base_f * (1.0 + 2.5 * (-t * 45.0).exp())
            } else {
                base_f
            };
            let phase = std::f32::consts::TAU * pitch * t;
            let sig = if hit_i == 1 || hit_i == 2 {
                // Deterministic pseudo-noise burst + tone for crisp HF transient flux
                let n =
                    ((i.wrapping_mul(1103515245).wrapping_add(12345) >> 16) as f32 / 32768.0) - 1.0;
                (phase.sin() * 0.45 + n * 0.55) * env * 0.85
            } else {
                phase.sin() * env * 0.85
            };
            left[start + i] = sig;
            right[start + i] = sig * if hit_i % 2 == 0 { 0.92 } else { 1.0 };
        }
    }

    let meta = if with_cues {
        SampleMetadata {
            root_midi: 60,
            has_root: true,
            loop_mode: SampleLoopMode::Off,
            slices: vec![
                SampleSlice::new(hit_starts[0], Some("Kick".into())),
                SampleSlice::new(hit_starts[1], Some("Hat".into())),
                SampleSlice::new(hit_starts[2], Some("Snare".into())),
                SampleSlice::new(hit_starts[3], Some("Perc".into())),
            ],
            ..SampleMetadata::default()
        }
    } else {
        SampleMetadata::default()
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    write_wav_with_metadata(path, &left, Some(&right), sample_rate, &meta).unwrap();
    (left, right)
}

fn app_with_temp_sampler(wav_name: &str, with_cues: bool) -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    let save_path = dir.path().join("project.json");
    let wav_path = dir.path().join("samples").join(wav_name);
    write_four_hit_break_wav(&wav_path, 44_100, with_cues);

    let lib = SampleLibrary::from_roots_only(&[dir.path().join("samples")], Some(&save_path));
    let mut app = App::new_with_library(Project::demo(), save_path, lib).unwrap();
    app.execute_command(&format!("sampler Break samples/{wav_name}"))
        .unwrap();
    assert!(app.ready(), "synthetic sampler must load cleanly");
    (dir, app)
}

fn find_sample_view(app: &App) -> cadence_app::view::SampleDeckView {
    let fv = app.frame_view(&AudioSnapshot::default(), "Test");
    fv.decks
        .into_iter()
        .find_map(|d| match d.view {
            DeckView::Sample(sv) => Some(sv),
            _ => None,
        })
        .expect("expected SampleDeckView in FrameView")
}

#[test]
fn riff_cue_slices_import_into_sampler_and_tier1_nav_never_mutates() {
    let (_dir, mut app) = app_with_temp_sampler("break_cues.wav", true);

    // Open Sample Deck via `gw`
    app.press_keys("gw").unwrap();
    assert_eq!(app.focused_deck_kind(), Some(DeckKind::Sample));

    let sv = find_sample_view(&app);
    assert!(sv.loaded);
    assert!(sv.is_wav_file);
    assert!(sv.has_riff_cues);
    assert!(!sv.riff_sync_dirty, "unmodified RIFF cues must match disk");
    assert_eq!(sv.slices.len(), 4);
    assert_eq!(sv.slices[0].label.as_deref(), Some("Kick"));
    assert_eq!(sv.slices[1].label.as_deref(), Some("Hat"));
    assert_eq!(sv.slices[2].label.as_deref(), Some("Snare"));
    assert_eq!(sv.slices[3].label.as_deref(), Some("Perc"));
    assert_eq!(sv.slices[0].midi_note, Some(36));
    assert_eq!(sv.slices[3].midi_note, Some(39));

    // Tier 1 navigation (`h/j/k/l`, `H/L`, `w/b`, `0/$`, `=/-`, `z0`) MUST NEVER mutate project!
    let before = app.project().clone();
    let rev = app.revision();
    app.press_keys("llhhHLwwbb0$jjkk==--z0").unwrap();
    assert_eq!(app.project(), &before);
    assert_eq!(app.revision(), rev);

    // Jump to slice 3 (`Snare`) using `w` and audition active slice (`A`)!
    app.press_keys("0ww").unwrap();
    let sv2 = find_sample_view(&app);
    assert_eq!(sv2.active_slice_idx, Some(2));
    let audition = app.press_keys("A").unwrap().pop().unwrap();
    let preview = audition
        .preview_sample
        .expect("A on Sample deck must emit SamplePreview");
    assert_eq!(preview.sample_start, sv2.slices[2].frame);
    assert_eq!(preview.sample_end, sv2.slices[2].end_frame);

    // Also verify `W` shortcut from Instrument Rack deck jumps straight into Sample Deck!
    app.press_keys("gi").unwrap();
    assert_eq!(app.focused_deck_kind(), Some(DeckKind::Instrument));
    app.press_keys("W").unwrap();
    assert_eq!(app.focused_deck_kind(), Some(DeckKind::Sample));
}

#[test]
fn crop_loop_transient_grid_split_nudge_and_lossless_riff_writeback() {
    let (dir, mut app) = app_with_temp_sampler("break_raw.wav", false);
    let wav_path = dir.path().join("samples/break_raw.wav");
    let before_loaded = load_sample_file(&wav_path, None).unwrap();
    assert!(before_loaded.meta.slices.is_empty());

    app.press_keys("gw").unwrap();
    let sv0 = find_sample_view(&app);
    assert!(sv0.slices.is_empty());

    // 1. Transient auto-slice (`d`) detects the 4 hits on the synthetic percussive break!
    app.press_keys("d").unwrap();
    let sv_auto = find_sample_view(&app);
    assert!(
        sv_auto.slices.len() >= 3 && sv_auto.slices.len() <= 8,
        "expected ~4 transient slices on 4-hit break, got {}",
        sv_auto.slices.len()
    );
    assert!(
        sv_auto.riff_sync_dirty,
        "modifying slices must flag RIFF metadata as unsaved"
    );

    // 2. Grid slice (`D`, default 8 divisions) and clear (`X`) + manual split (`s`)
    app.press_keys("D").unwrap();
    assert_eq!(find_sample_view(&app).slices.len(), 8);

    app.press_keys("X").unwrap();
    assert!(find_sample_view(&app).slices.is_empty());

    // Scrub into middle and press `s` — creates 2 regions ([start..cursor] and [cursor..end])
    app.press_keys("0LLLLs").unwrap();
    let sv_split = find_sample_view(&app);
    assert_eq!(sv_split.slices.len(), 2);

    // Rename active slice via `r` -> ValueInput
    app.press_keys("r<Ctrl+u>Drop<Enter>").unwrap();
    assert_eq!(
        find_sample_view(&app).slices[1].label.as_deref(),
        Some("Drop")
    );

    // Nudge slice marker with `,`/`.` and verify Undo coalescing/reversibility
    let before_frame = find_sample_view(&app).slices[1].frame;
    app.press_keys(".").unwrap();
    let after_frame = find_sample_view(&app).slices[1].frame;
    assert!(after_frame > before_frame, "`.` must nudge slice right");

    // Set crop `[` / `]` and loop `{` / `}` (auto-enables Forward loop!) + cycle loop mode `m`
    app.press_keys("0L[").unwrap();
    assert!(find_sample_view(&app).sample_start > 0);
    app.press_keys("L{").unwrap();
    assert_eq!(find_sample_view(&app).loop_mode, SampleLoopMode::Forward);
    app.press_keys("m").unwrap();
    assert_eq!(find_sample_view(&app).loop_mode, SampleLoopMode::PingPong);

    // Toolbar parameter navigation + snap toggle (`v`) + reset (`Alt+Backspace`)
    assert!(app.sample_ui.snap_zero);
    app.press_keys("v").unwrap();
    assert!(!app.sample_ui.snap_zero);
    app.press_keys("j").unwrap();
    assert_eq!(app.sample_ui.toolbar_field, SampleToolbarField::Sensitivity);
    app.press_keys(".").unwrap();
    assert!((app.sample_ui.sensitivity - 0.60).abs() < 1e-4);

    // 3. Explicit lossless RIFF WAV write-back (`W`)!
    let pre_write_view = find_sample_view(&app);
    assert!(pre_write_view.riff_sync_dirty);
    app.press_keys("W").unwrap();

    let post_write_view = find_sample_view(&app);
    assert!(
        !post_write_view.riff_sync_dirty,
        "after W write-back, disk RIFF metadata must match module"
    );
    assert!(post_write_view.has_riff_cues);

    // Independently reload the WAV from disk and verify exact PCM preservation + RIFF chunks!
    let reloaded = load_sample_file(&wav_path, None).unwrap();
    assert_eq!(reloaded.frames, before_loaded.frames);
    assert_eq!(reloaded.left, before_loaded.left);
    assert_eq!(reloaded.right, before_loaded.right);
    assert_eq!(reloaded.meta.loop_mode, SampleLoopMode::PingPong);
    assert_eq!(reloaded.meta.slices.len(), 2);
    assert_eq!(reloaded.meta.slices[1].label.as_deref(), Some("Drop"));
}

#[test]
fn slice_to_rack_kit_creates_chromatic_kit_pattern_and_undoes_atomically() {
    let (_dir, mut app) = app_with_temp_sampler("kit_break.wav", true);
    let inst_count_before = app.project().instruments.len();
    let pat_count_before = app.project().patterns.len();
    let lane_count_before = app.project().lanes.len();
    let snapshot_before = app.project().clone();

    // Open Sample Deck -> `K` opens Slice-to-Kit picker -> `1` creates New Kit + Slice Pattern!
    app.press_keys("gwK1").unwrap();

    assert_eq!(app.focused_deck_kind(), Some(DeckKind::Instrument));
    assert_eq!(app.rack_ui.tab, RackTab::Zones);
    assert_eq!(app.project().instruments.len(), inst_count_before + 1);
    assert_eq!(app.project().patterns.len(), pat_count_before + 1);
    assert_eq!(app.project().lanes.len(), lane_count_before + 1);

    let kit = app.project().instruments.last().unwrap();
    assert_eq!(kit.rack.zones.len(), 4);
    assert_eq!(kit.rack.zones[0].name, "Kick");
    assert_eq!(kit.rack.zones[0].low_midi, 36);
    assert_eq!(kit.rack.zones[0].high_midi, 36);
    assert_eq!(kit.rack.zones[0].choke_group, Some(1));
    assert_eq!(kit.rack.zones[2].name, "Snare");
    assert_eq!(kit.rack.zones[2].low_midi, 38);
    if let ModuleKind::Sampler(smp) = &kit.rack.zones[2].modules[0].kind {
        assert_eq!(smp.sample_start, 44_100);
        assert_eq!(smp.sample_end, 66_150);
    } else {
        panic!("expected Sampler module in sliced kit zone");
    }

    // Verify all 4 kit zones have their samples loaded and ready!
    assert!(app.ready());

    // Verify the generated sequential slice pattern has 4 active notes!
    let pat = app.project().patterns.last().unwrap();
    match &pat.kind {
        cadence_core::material::PatternKind::Notes { steps } => {
            let active_count = steps.iter().filter(|s| s.active).count();
            assert_eq!(active_count, 4);
        }
        other => panic!("expected Notes pattern, got {other:?}"),
    }

    // Single `u` (Undo) cleanly reverts the ENTIRE Slice-to-Kit + Pattern + Lane + Placement transaction!
    app.apply(Action::Undo).unwrap();
    assert_eq!(app.project(), &snapshot_before);

    // Test `:slice kit replace nopattern` Ex command too!
    app.press_keys("gw").unwrap();
    app.execute_command("slice grid 4").unwrap();
    app.execute_command("slice rename 1 Boom").unwrap();
    app.execute_command("slice kit replace nopattern").unwrap();
    assert_eq!(
        app.project().instruments.len(),
        inst_count_before,
        "replace must not add a new instrument"
    );
    let replaced = app.project().instruments.last().unwrap();
    assert_eq!(replaced.rack.zones.len(), 4);
    assert_eq!(replaced.rack.zones[0].name, "Boom");
}

#[test]
fn offline_sample_ops_trim_normalize_reverse_fade_write_portable_wavs_and_undo_cleanly() {
    use cadence_app::{HostIo, MemoryHostIo};
    use cadence_core::dsp::{db_to_lin, DEFAULT_NORMALIZE_TARGET_DBFS};
    use cadence_core::sampler::encode_wav_with_metadata_bytes;
    use std::path::PathBuf;
    use std::sync::Arc;

    let io = Arc::new(MemoryHostIo::new("/virtual/sample-ops"));
    let sr = 44_100u32;
    let frames = sr as usize; // 1.0s
    let mut left = vec![0.0_f32; frames];
    let mut right = vec![0.0_f32; frames];
    for i in 0..frames {
        let t = i as f32 / sr as f32;
        // Distinct asymmetric amplitude ramp so Reverse, Trim, Normalize, and Fade are unmistakable.
        let env = 0.08 + 0.17 * (i as f32 / frames as f32);
        let wave = (std::f32::consts::TAU * 220.0 * t).sin();
        left[i] = wave * env;
        right[i] = wave * env * 0.85;
    }
    let meta = SampleMetadata {
        root_midi: 60,
        has_root: true,
        loop_mode: SampleLoopMode::Forward,
        loop_start: frames / 4,
        loop_end: (frames * 3) / 4,
        slices: vec![
            SampleSlice::new(0, Some("Kick".into())),
            SampleSlice::new(frames / 3, Some("Hat".into())),
            SampleSlice::new((frames * 2) / 3, Some("Snare".into())),
        ],
        ..SampleMetadata::default()
    };
    let wav_bytes = encode_wav_with_metadata_bytes(&left, Some(&right), sr, &meta).unwrap();
    let src_abs = Path::new("/virtual/sample-ops/samples/source.wav");
    io.insert_file(src_abs, wav_bytes);

    let mut app = App::new_with_host_io(
        Project::demo(),
        PathBuf::from("/virtual/sample-ops/project.json"),
        Arc::clone(&io) as Arc<dyn HostIo>,
    )
    .unwrap();
    app.execute_command("sampler EditTrack samples/source.wav")
        .unwrap();
    app.press_keys("gw").unwrap();
    assert_eq!(app.focused_deck_kind(), Some(DeckKind::Sample));
    let before_proj = app.project().clone();

    let sampler_of = |app: &App| -> cadence_core::rack::SamplerModule {
        let inst = app
            .project()
            .instruments
            .iter()
            .find(|i| i.name == "EditTrack")
            .unwrap();
        match &inst.rack.zones[0].modules[0].kind {
            ModuleKind::Sampler(s) => s.clone(),
            _ => panic!("expected Sampler"),
        }
    };

    // 1. Normalize via `N` shortcut writes new portable WAV at -0.5 dBFS and undoes cleanly!
    app.press_keys("N").unwrap();
    let norm_smp = sampler_of(&app);
    let norm_rel = norm_smp.path.as_deref().unwrap();
    assert!(norm_rel.starts_with("samples/source-norm"));
    assert!(io
        .read_file(Path::new("/virtual/sample-ops").join(norm_rel))
        .is_some());
    let norm_data = app.sample_bank().into_iter().next().unwrap().1;
    let norm_peak = norm_data
        .left
        .iter()
        .chain(norm_data.right.as_ref().unwrap().iter())
        .fold(0.0_f32, |a, &s| a.max(s.abs()));
    assert!(
        (norm_peak - db_to_lin(DEFAULT_NORMALIZE_TARGET_DBFS)).abs() < 0.01,
        "normalized peak {norm_peak} should match -0.5 dBFS"
    );
    assert_eq!(norm_smp.slices.len(), 3);
    app.apply(Action::Undo).unwrap();
    assert_eq!(app.project(), &before_proj);

    // 2. Reverse via `e` Offline Edit picker -> `3` reverses audio AND labeled slice regions!
    app.press_keys("e3").unwrap();
    let rev_smp = sampler_of(&app);
    let rev_rel = rev_smp.path.as_deref().unwrap();
    assert!(rev_rel.starts_with("samples/source-rev"));
    assert_eq!(rev_smp.slices.len(), 3);
    assert_eq!(rev_smp.slices[0].label.as_deref(), Some("Snare"));
    assert_eq!(rev_smp.slices[1].label.as_deref(), Some("Hat"));
    assert_eq!(rev_smp.slices[2].label.as_deref(), Some("Kick"));
    app.apply(Action::Undo).unwrap();
    assert_eq!(app.project(), &before_proj);

    // 3. Crop middle `[frames/4 .. frames*3/4]` and Trim via `T` shortcut:
    //    writes trimmed WAV, resets crop to full (0..0), remaps interior slice ("Hat") to frame ~frames/12!
    let inst_id = app
        .project()
        .instruments
        .iter()
        .find(|i| i.name == "EditTrack")
        .unwrap()
        .id;
    app.apply(Action::SetSamplerRegion {
        instrument: inst_id,
        zone: 1,
        module: 1,
        sample_start: Some(frames / 4),
        sample_end: Some((frames * 3) / 4),
        loop_mode: None,
        loop_start: None,
        loop_end: None,
        loop_xfade_ms: None,
    })
    .unwrap();
    app.press_keys("T").unwrap();
    let trim_smp = sampler_of(&app);
    assert!(trim_smp
        .path
        .as_deref()
        .unwrap()
        .starts_with("samples/source-trim"));
    assert_eq!(trim_smp.sample_start, 0);
    assert_eq!(trim_smp.sample_end, 0);
    assert_eq!(trim_smp.slices.len(), 2);
    assert_eq!(trim_smp.slices[0].label.as_deref(), Some("Hat"));
    assert_eq!(trim_smp.slices[0].frame, frames / 3 - frames / 4);
    assert_eq!(trim_smp.slices[1].label.as_deref(), Some("Snare"));
    assert_eq!(trim_smp.slices[1].frame, (frames * 2) / 3 - frames / 4);
    let trim_data = app
        .sample_bank()
        .into_iter()
        .find(|(_, d)| d.resolved_path.to_string_lossy().contains("source-trim"))
        .expect("trimmed sample loaded")
        .1;
    assert_eq!(trim_data.frames, frames / 2);
    assert_eq!(trim_data.meta.slices.len(), 2);
    // Two undos revert Trim then Crop back to exact original project.
    app.apply(Action::Undo).unwrap();
    app.apply(Action::Undo).unwrap();
    assert_eq!(app.project(), &before_proj);

    // 4. Fade-In and Fade-Out via `:sample fade-in 50ms` and `:sample fade-out 50ms`.
    app.execute_command("sample fade-in 50ms").unwrap();
    let fi_smp = sampler_of(&app);
    assert!(fi_smp
        .path
        .as_deref()
        .unwrap()
        .starts_with("samples/source-fadein"));
    let fi_data = app
        .sample_bank()
        .into_iter()
        .find(|(_, d)| d.resolved_path.to_string_lossy().contains("source-fadein"))
        .unwrap()
        .1;
    assert!(fi_data.left[0].abs() < 1e-4, "fade-in must start at 0.0");
    let mid_peak = fi_data.left[frames / 2 - 100..frames / 2 + 100]
        .iter()
        .fold(0.0_f32, |a, &s| a.max(s.abs()));
    assert!(mid_peak > 0.10, "fade-in must leave interior audio intact");

    app.execute_command("sample fade-out 50ms").unwrap();
    let fo_data = app
        .sample_bank()
        .into_iter()
        .find(|(_, d)| d.resolved_path.to_string_lossy().contains("fadeout"))
        .unwrap()
        .1;
    assert!(
        fo_data.left[frames - 1].abs() < 1e-4,
        "fade-out must end at 0.0"
    );
    assert!(!src_abs.exists(), "MemoryHostIo must never touch real disk");
}

#[test]
fn sample_deck_live_recording_monitor_targets_active_zone_and_lands_back_in_sample_deck() {
    use cadence_app::{
        BounceTarget, CompletedRecording, HostIo, MemoryHostIo, RecordingHostCommand,
    };
    use cadence_core::dsp::LivePeakRecorder;
    use cadence_core::runtime::{AudioInputStatus, AudioInputTelemetry, CapturePhase};
    use cadence_core::sampler::encode_wav_with_metadata_bytes;
    use std::path::PathBuf;
    use std::sync::Arc;

    let io = Arc::new(MemoryHostIo::new("/virtual/rec-deck"));
    let mut app = App::new_with_host_io(
        Project::demo(),
        PathBuf::from("/virtual/rec-deck/project.json"),
        Arc::clone(&io) as Arc<dyn HostIo>,
    )
    .unwrap();
    app.config.audio_input_device = Some("Studio Interface".into());
    app.execute_command("sampler VocalSlot blank").unwrap();
    app.press_keys("gw").unwrap();
    assert_eq!(app.focused_deck_kind(), Some(DeckKind::Sample));
    let vocal_id = app
        .project()
        .instruments
        .iter()
        .find(|i| i.name == "VocalSlot")
        .unwrap()
        .id;

    // 1. Empty Sample Deck with running hardware input shows live input oscilloscope monitor.
    let mut snap = AudioSnapshot {
        sample_rate: 48_000,
        input: AudioInputTelemetry {
            status: AudioInputStatus::Running,
            sample_rate: 48_000,
            channels: 2,
            peak: [0.42, 0.38],
            peak_hold: [0.55, 0.50],
            ..AudioInputTelemetry::default()
        },
        input_waveform: (0..256).map(|i| (i as f32 * 0.12).sin() * 0.45).collect(),
        ..AudioSnapshot::default()
    };
    let fv_idle = app.frame_view(&snap, "test");
    let sv_idle = fv_idle
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Sample(s) => Some(s),
            _ => None,
        })
        .expect("SampleDeckView present");
    let mon_idle = sv_idle
        .record_monitor
        .as_ref()
        .expect("empty Sample deck with running input shows live monitor");
    assert_eq!(mon_idle.phase, CapturePhase::Idle);
    assert_eq!(mon_idle.live_scope.len(), 256);

    // 2. Pressing `ta` on Sample Deck targets THIS Sampler's zone and switches to Armed/Recording monitor!
    let rec_cmd = app
        .press_keys("ta")
        .unwrap()
        .into_iter()
        .find_map(|o| o.recording_command);
    let req = match rec_cmd {
        Some(RecordingHostCommand::Arm { request }) => *request,
        other => panic!("expected RecordingHostCommand::Arm, got {other:?}"),
    };
    assert_eq!(
        req.target,
        BounceTarget::RackZone {
            instrument: vocal_id,
            zone_index: Some(0),
            midi: None,
            zone_name: None,
        }
    );

    // Simulate active recording with streaming LivePeakRecorder peaks!
    snap.input.capture_phase = CapturePhase::Recording;
    snap.input.captured_frames = 48_000;
    app.observe_audio_recording(&snap);
    let mut tracker = LivePeakRecorder::new(512, 128);
    let captured_stereo: Vec<[f32; 2]> = (0..48_000)
        .map(|i| {
            let v = (std::f32::consts::TAU * 330.0 * i as f32 / 48_000.0).sin() * 0.65;
            [v, v * 0.9]
        })
        .collect();
    tracker.push_stereo(&captured_stereo);
    app.set_recording_preview_peaks(tracker.snapshot_peaks());

    let fv_rec = app.frame_view(&snap, "test");
    let sv_rec = fv_rec
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Sample(s) => Some(s),
            _ => None,
        })
        .unwrap();
    let mon_rec = sv_rec.record_monitor.as_ref().unwrap();
    assert_eq!(mon_rec.phase, CapturePhase::Recording);
    assert_eq!(mon_rec.captured_frames, 48_000);
    assert!(!mon_rec.recording_peaks.is_empty());

    // 3. Finalizing recording lands the new WAV directly back onto DeckKind::Sample ready to slice!
    let left: Vec<f32> = captured_stereo.iter().map(|f| f[0]).collect();
    let right: Vec<f32> = captured_stereo.iter().map(|f| f[1]).collect();
    let rec_wav =
        encode_wav_with_metadata_bytes(&left, Some(&right), 48_000, &SampleMetadata::default())
            .unwrap();
    io.insert_file(&req.output_path, rec_wav);
    let completed = CompletedRecording {
        request: req,
        frames: 48_000,
        sample_rate: 48_000,
        duration_sec: 1.0,
        peak_left_db: -3.7,
        peak_right_db: -4.6,
        full_scale_samples: 0,
        dropped_frames: 0,
        actual_start_abs_step: 0,
        actual_end_abs_step: 8,
    };
    app.complete_recording(completed).unwrap();
    assert_eq!(
        app.focused_deck_kind(),
        Some(DeckKind::Sample),
        "recording from Sample Deck must land directly back in Sample Deck"
    );

    snap.input.capture_phase = CapturePhase::Idle;
    snap.input.captured_frames = 0;
    let fv_done = app.frame_view(&snap, "test");
    let sv_done = fv_done
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Sample(s) => Some(s),
            _ => None,
        })
        .unwrap();
    assert!(sv_done.loaded);
    assert_eq!(sv_done.total_frames, 48_000);
    assert!(
        sv_done.record_monitor.is_none(),
        "loaded Sample Deck returns cleanly to waveform editor once recording finishes"
    );
}

#[test]
fn sample_deck_param_command_targets_focused_sampler_and_rack_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let wav_a = dir.path().join("samples/pad_a.wav");
    let wav_b = dir.path().join("samples/pad_b.wav");
    write_four_hit_break_wav(&wav_a, 48_000, false);
    write_four_hit_break_wav(&wav_b, 48_000, false);

    let save_path = dir.path().join("project.json");
    let lib = SampleLibrary::from_roots_only(&[dir.path().join("samples")], Some(&save_path));
    let mut app = App::new_with_library(Project::demo(), save_path, lib).unwrap();

    // 1. Single-zone Sampler focused on Sample Deck (`gw`): bare `:param` sets both Sampler
    //    module parameters AND Rack-level fallback parameters cleanly.
    app.execute_command("sampler LeadSmp samples/pad_a.wav")
        .unwrap();
    app.press_keys("gw").unwrap();
    assert!(app.sample_deck_focused());
    let single_id = app
        .project()
        .instruments
        .iter()
        .find(|i| i.name == "LeadSmp")
        .unwrap()
        .id;
    assert_eq!(app.scope_instrument(), Some(single_id));

    app.execute_command("param sample_start 1200").unwrap();
    app.execute_command("param sample_end 40000").unwrap();
    app.execute_command("param loop_mode forward").unwrap();
    app.execute_command("param loop_start 4800").unwrap();
    app.execute_command("param loop_end 24000").unwrap();
    app.execute_command("param loop_xfade 14ms").unwrap();
    app.execute_command("param gain -3dB").unwrap();
    app.execute_command("param rack.gain -4.5dB").unwrap();

    let single = app.instrument(single_id).unwrap();
    assert!((single.rack.zones[0].gain_db - (-3.0)).abs() < 0.05);
    assert!((single.rack.gain_db - (-4.5)).abs() < 0.05);
    let smp = match &single.rack.zones[0].modules[0].kind {
        ModuleKind::Sampler(s) => s,
        other => panic!("expected Sampler module, got {other:?}"),
    };
    assert_eq!(smp.sample_start, 1_200);
    assert_eq!(smp.sample_end, 40_000);
    assert_eq!(smp.loop_mode, SampleLoopMode::Forward);
    assert_eq!(smp.loop_start, 4_800);
    assert_eq!(smp.loop_end, 24_000);
    assert!((smp.loop_xfade_ms - 14.0).abs() < 0.1);

    // 2. Multi-zone Kit focused on Zone 2 in Sample Deck (`gw`): bare `:param sample_start`
    //    unambiguously updates Zone 2's Sampler without modifying Zone 1.
    app.execute_command("kit DualKit").unwrap();
    app.execute_command("zone sample samples/pad_a.wav")
        .unwrap();
    app.execute_command("zone add Rim 38 sample samples/pad_b.wav")
        .unwrap();
    app.execute_command("zone select Rim").unwrap();
    app.press_keys("gw").unwrap();
    assert!(app.sample_deck_focused());

    app.execute_command("param sample_start 2048").unwrap();
    app.execute_command("param loop_mode ping_pong").unwrap();

    let kit = app
        .project()
        .instruments
        .iter()
        .find(|i| i.name == "DualKit")
        .unwrap();
    let z1_smp = kit.rack.zones[0]
        .modules
        .iter()
        .find_map(|m| match &m.kind {
            ModuleKind::Sampler(s) => Some(s),
            _ => None,
        })
        .expect("Zone 1 Sampler");
    let z2_smp = kit
        .rack
        .zones
        .iter()
        .find(|z| z.name == "Rim")
        .expect("Rim zone")
        .modules
        .iter()
        .find_map(|m| match &m.kind {
            ModuleKind::Sampler(s) => Some(s),
            _ => None,
        })
        .expect("Rim zone Sampler");
    assert_eq!(z1_smp.sample_start, 0, "Zone 1 must remain untouched");
    assert_eq!(z1_smp.loop_mode, SampleLoopMode::Off);
    assert_eq!(
        z2_smp.sample_start, 2_048,
        "Zone 2 Sampler must receive bare :param while focused in Sample Deck"
    );
    assert_eq!(z2_smp.loop_mode, SampleLoopMode::PingPong);
}
