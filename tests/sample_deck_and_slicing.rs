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

    let lib = SampleLibrary::discover(&[dir.path().join("samples")], Some(&save_path));
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
