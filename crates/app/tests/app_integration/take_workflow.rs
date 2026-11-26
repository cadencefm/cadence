//! End-to-end tests for explicit take arming, take journaling, visual range
//! marking in the Take deck, discard confirmation, and in-memory take recording.
use cadence_app::view::{DeckView, FrameView, OverlayView};
use cadence_app::{Action, App, KeyCode, KeyEvent, KeyModifiers, MemoryHostIo};
use cadence_core::material::Project;
use cadence_core::runtime::AudioSnapshot;
use cadence_core::sampler::SampleLibrary;
use cadence_core::take::Take;
use std::path::PathBuf;
use std::sync::Arc;

fn hermetic_app(project: Project, save_path: PathBuf, roots: &[PathBuf]) -> App {
    let lib = SampleLibrary::from_roots(roots, Some(&save_path));
    App::new_with_library(project, save_path, lib).unwrap()
}

fn snap_view(app: &App, playing: bool) -> FrameView {
    let snap = AudioSnapshot {
        playing,
        absolute_step: app.transport_step(),
        sample_rate: 48_000,
        ..AudioSnapshot::default()
    };
    app.frame_view(&snap, "Test · 48 kHz")
}

#[test]
fn unarmed_play_loop_mix_and_undo_redo_never_record_a_take() {
    let dir = tempfile::tempdir().unwrap();
    let takes_dir = dir.path().join("session-takes");
    let mut app = hermetic_app(Project::demo(), dir.path().join("demo.json"), &[]);
    app.set_session_takes_dir(takes_dir.clone());

    assert!(
        !app.config.auto_record_takes,
        "auto_record_takes must default to false"
    );
    assert!(!app.takes.armed, "take_armed must default to false");

    // Start playback, loop, mute/unmute, edit BPM, Undo, Redo — all while unarmed.
    app.press_keys(" ").unwrap();
    assert!(app.playing());
    app.apply(Action::Advance { steps: 64 }).unwrap();

    let lead_lane = app.project().lanes[0].id;
    app.apply(Action::SetLaneMuted {
        lane: lead_lane,
        muted: true,
    })
    .unwrap();
    app.execute_command("bpm 142").unwrap();
    app.apply(Action::Advance { steps: 32 }).unwrap();
    app.press_keys("u").unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL))
        .unwrap();
    app.apply(Action::Advance { steps: 32 }).unwrap();

    assert!(
        app.take_recording().is_none(),
        "unarmed play must not open a take"
    );
    assert!(
        app.current_take().is_none(),
        "unarmed play must not create a take"
    );
    assert!(app.active_take_path().is_none());
    assert!(app.last_sealed_take_path().is_none());

    let fv = snap_view(&app, true);
    assert!(!fv.chrome.take_armed);
    assert_eq!(fv.chrome.take_recording, None);

    // Take deck reports clean unarmed empty state.
    app.press_keys("gt").unwrap();
    let fv_take = snap_view(&app, true);
    let take_view = fv_take
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Take(v) => Some(v),
            _ => None,
        })
        .expect("Take deck view");
    assert!(take_view.empty);
    assert!(!take_view.take_armed);

    // Stop playback: still zero takes recorded and zero session take files written.
    app.press_keys("<Home>").unwrap();
    assert!(app.current_take().is_none());
    assert!(
        !takes_dir.exists() || std::fs::read_dir(&takes_dir).unwrap().count() == 0,
        "unarmed session must not write session take files"
    );
}

#[test]
fn explicit_take_arming_mid_play_seal_discard_confirm_and_settings_toggle() {
    let dir = tempfile::tempdir().unwrap();
    let takes_dir = dir.path().join("session-takes");
    let mut app = hermetic_app(Project::demo(), dir.path().join("demo.json"), &[]);
    app.set_session_takes_dir(takes_dir);

    // Arm while stopped via `tr`: TAKE ARM badge (take_armed=true, take_recording=None).
    app.press_keys("tr").unwrap();
    assert!(app.takes.armed);
    assert!(app.take_recording().is_none());
    let fv_arm = snap_view(&app, false);
    assert!(fv_arm.chrome.take_armed);
    assert_eq!(fv_arm.chrome.take_recording, None);

    // Disarm via `tr`: badge disappears.
    app.press_keys("tr").unwrap();
    assert!(!app.takes.armed);

    // Start playing UNARMED, advance 1 bar, then arm mid-playback via `tr`.
    app.press_keys(" ").unwrap();
    app.apply(Action::Advance { steps: 16 }).unwrap();
    assert!(app.take_recording().is_none());
    app.press_keys("tr").unwrap();
    assert!(app.takes.armed);
    assert!(
        app.take_recording().is_some(),
        "arming while playing must immediately open a TakeJournal"
    );

    // Record live events so the take is non-empty and unsaved.
    app.execute_command("bpm 144").unwrap();
    app.apply(Action::Advance { steps: 32 }).unwrap();
    let fv_rec = snap_view(&app, true);
    assert!(fv_rec.chrome.take_armed);
    assert!(
        fv_rec.chrome.take_recording.is_some_and(|n| n >= 1),
        "chrome must report live take event count: {:?}",
        fv_rec.chrome.take_recording
    );

    // Disarm while playing seals the active take cleanly.
    app.press_keys("tr").unwrap();
    assert!(!app.takes.armed);
    assert!(app.take_recording().is_none());
    assert!(
        app.current_take().is_some(),
        "sealed take stays inspectable in Take deck"
    );
    assert!(app.last_sealed_take_path().is_some());
    app.press_keys("<Home>").unwrap();

    // Open Take deck (`gt`) and press `X` (Discard Take): must confirm for unsaved take with events!
    app.press_keys("gt").unwrap();
    app.press_keys("X").unwrap();
    assert_eq!(app.mode_name(), "CONFIRM");
    // Cancel with Esc preserves the take.
    app.press_keys("<Esc>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert!(app.current_take().is_some());

    // Press `X` again then `y` to confirm discard: clears take, sealed path, marks, visual anchor.
    app.press_keys("Xy").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert!(app.current_take().is_none());
    assert!(app.last_sealed_take_path().is_none());
    assert_eq!(app.takes.mark_start, None);
    assert_eq!(app.takes.mark_end, None);
    assert_eq!(app.takes.visual_anchor, None);

    let fv_empty = snap_view(&app, false);
    let tv = fv_empty
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Take(v) => Some(v),
            _ => None,
        })
        .unwrap();
    assert!(tv.empty);

    // Ex commands `:take arm`, `:take disarm`, `:take discard` work too.
    app.execute_command("take arm").unwrap();
    assert!(app.takes.armed);
    app.press_keys(" ").unwrap();
    app.apply(Action::Advance { steps: 16 }).unwrap();
    assert!(app.take_recording().is_some());
    app.execute_command("take discard").unwrap();
    assert!(app.current_take().is_none());
    assert!(
        !app.takes.armed,
        "discarding an active playing take disarms cleanly"
    );
    app.press_keys("<Home>").unwrap();

    // Settings (`g,`) live toggle for `Auto-Record Takes` updates config AND `take_armed`.
    app.press_keys("g,").unwrap();
    let fv_settings = snap_view(&app, false);
    let auto_row = match &fv_settings.overlay {
        Some(OverlayView::Settings(sv)) => sv
            .rows
            .iter()
            .position(|r| r.label == "Auto-Record Takes")
            .expect("Auto-Record Takes row in Settings"),
        other => panic!("expected Settings overlay, got {other:?}"),
    };
    for _ in 0..auto_row {
        app.press_keys("j").unwrap();
    }
    app.press_keys("<m-l>").unwrap();
    assert!(app.config.auto_record_takes);
    assert!(app.takes.armed);
    app.press_keys("<m-h><Esc>").unwrap();
    assert!(!app.config.auto_record_takes);
    assert!(!app.takes.armed);
}

#[test]
fn take_deck_visual_range_marking_sweeps_locks_clears_keeps_and_has_clean_hints() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = hermetic_app(Project::demo(), dir.path().join("demo.json"), &[]);

    app.press_keys("tr ").unwrap();
    app.apply(Action::Advance { steps: 16 }).unwrap();
    app.execute_command("bpm 140").unwrap();
    app.apply(Action::Advance { steps: 80 }).unwrap(); // 6 bars total
    app.press_keys("tr<Home>gt").unwrap();

    // Move to bar 2 (0-based index 2 = B03) and press `v` to enter visual marking.
    app.press_keys("llv").unwrap();
    assert_eq!(app.takes.visual_anchor, Some(2));
    assert_eq!(app.takes.mark_start, Some(2));
    assert_eq!(app.takes.mark_end, Some(2));

    // Sweep forward 2 bars (`ll`) → B03–B05 (indices 2..=4).
    app.press_keys("ll").unwrap();
    assert_eq!(app.takes.cursor_bar, 4);
    assert_eq!(app.takes.mark_start, Some(2));
    assert_eq!(app.takes.mark_end, Some(4));

    let fv_vis = snap_view(&app, false);
    let tv_vis = fv_vis
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Take(v) => Some(v),
            _ => None,
        })
        .unwrap();
    assert!(tv_vis.visual_selecting);
    assert_eq!(tv_vis.mark_range, Some((2, 4)));

    // Sweep backward past anchor (`hhhh`) → cursor at 0, range 0..=2!
    app.press_keys("hhhh").unwrap();
    assert_eq!(app.takes.cursor_bar, 0);
    assert_eq!(app.takes.mark_start, Some(0));
    assert_eq!(app.takes.mark_end, Some(2));

    // Press `v` again to lock the range cleanly.
    app.press_keys("v").unwrap();
    assert_eq!(app.takes.visual_anchor, None);
    assert_eq!(app.takes.mark_start, Some(0));
    assert_eq!(app.takes.mark_end, Some(2));

    // Esc clears range marks first without leaving Take deck!
    app.press_keys("<Esc>").unwrap();
    assert_eq!(app.takes.mark_start, None);
    assert_eq!(app.takes.mark_end, None);

    // Visual mark bars 1..=2 (`vl`) and keep (`Enter`) into arrangement.
    let rev_before = app.revision();
    app.press_keys("vl<Enter>").unwrap();
    assert!(
        app.revision() > rev_before,
        "Enter must keep visually marked take range"
    );
    assert_eq!(app.takes.visual_anchor, None);

    // Verify Take deck hints advertise `v`, `X`, and `tr`.
    let fv_hints = snap_view(&app, false);
    let hint_keys: Vec<&str> = fv_hints
        .chrome
        .hints
        .iter()
        .map(|(k, _)| k.as_str())
        .collect();
    assert!(
        hint_keys.contains(&"v"),
        "Take hints must advertise v: {hint_keys:?}"
    );
    assert!(
        hint_keys.contains(&"X"),
        "Take hints must advertise X: {hint_keys:?}"
    );
    assert!(
        hint_keys.contains(&"tr"),
        "Take hints must advertise tr: {hint_keys:?}"
    );
}

#[test]
fn take_journal_records_and_seals_purely_in_memory_host_io() {
    let io = Arc::new(MemoryHostIo::new("/virtual"));
    let mut app = App::new_with_host_io(
        Project::demo(),
        PathBuf::from("/virtual/project.json"),
        Arc::clone(&io) as Arc<dyn cadence_app::HostIo>,
    )
    .unwrap();
    app.set_session_takes_dir(PathBuf::from("/virtual/session_takes"));
    app.apply(Action::SetTakeArmed { armed: true }).unwrap();
    app.apply(Action::Play { playing: true }).unwrap();
    app.observe_transport(16, 16, true, false);
    // Stop seals the take journal into virtual MemoryHostIo.
    app.apply(Action::Stop).unwrap();
    app.apply(Action::Stop).unwrap();
    let take_files: Vec<PathBuf> = io
        .file_paths()
        .into_iter()
        .filter(|p| p.to_string_lossy().ends_with(".take.jsonl"))
        .collect();
    assert_eq!(
        take_files.len(),
        1,
        "MemoryHostIo must receive sealed take JSONL in virtual memory: {take_files:?}"
    );
    let raw = io.read_file(&take_files[0]).unwrap();
    let parsed_take = Take::from_jsonl_bytes(&raw, false).unwrap();
    assert!(parsed_take.end_step >= 16);
}
