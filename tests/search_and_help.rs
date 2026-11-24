//! End-to-end tests for universal `/` search semantics across overlays,
//! Help overlay formatting/scroll/search behavior, and live hints accuracy.
mod common;

use cadence_app::view::{FrameView, OverlayView, HELP_ENTRIES};
use cadence_app::{App, KeyCode, KeyEvent, KeyModifiers};
use cadence_core::material::Project;
use cadence_core::runtime::AudioSnapshot;
use cadence_core::sampler::SampleLibrary;
use std::path::{Path, PathBuf};

fn write_test_wav(path: &Path, frames: usize, sample_rate: u32) {
    common::write_decay_wav(path, 1, sample_rate, frames, 220.0, 0.6);
}

fn hermetic_app(project: Project, save_path: PathBuf, roots: &[PathBuf]) -> App {
    let lib = SampleLibrary::from_roots(roots, Some(&save_path));
    App::new_with_library(project, save_path, lib).unwrap()
}

fn snap_view(app: &App) -> FrameView {
    let snap = AudioSnapshot {
        playing: app.playing(),
        absolute_step: app.transport_step(),
        sample_rate: 48_000,
        ..AudioSnapshot::default()
    };
    app.frame_view(&snap, "Test · 48 kHz")
}

fn wait_browser_close(app: &mut App) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while app.mode_name() == "SAMPLES" && std::time::Instant::now() < deadline {
        app.poll_browser_updates().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn universal_slash_search_semantics_across_browser_filehub_picker_and_help() {
    let dir = tempfile::tempdir().unwrap();
    let samples_dir = dir.path().join("samples");
    std::fs::create_dir_all(&samples_dir).unwrap();
    write_test_wav(&samples_dir.join("alpha-kick.wav"), 2400, 48_000);
    write_test_wav(&samples_dir.join("beta-snare.wav"), 2400, 48_000);

    // Seed two project JSON files in `dir` for FileHub filtering.
    let mut p_alpha = Project::demo();
    p_alpha.title = "Alpha Project".into();
    let app_a = hermetic_app(p_alpha, dir.path().join("alpha-song.json"), &[]);
    app_a
        .save_project(&dir.path().join("alpha-song.json"))
        .unwrap();
    let mut p_beta = Project::demo();
    p_beta.title = "Beta Project".into();
    let app_b = hermetic_app(p_beta, dir.path().join("beta-song.json"), &[]);
    app_b
        .save_project(&dir.path().join("beta-song.json"))
        .unwrap();

    let mut app = hermetic_app(
        Project::demo(),
        dir.path().join("demo.json"),
        &[samples_dir],
    );

    // --- (a) Sample Browser ---
    app.execute_command("sampler DrumBox").unwrap();
    let samp_id = app.selected_instrument.unwrap();
    app.press_keys("B").unwrap();
    assert_eq!(app.mode_name(), "SAMPLES");

    // `/` enters search; typing filters; `Enter` exits search keeping filter active.
    app.press_keys("/beta<Enter>").unwrap();
    assert_eq!(app.mode_name(), "SAMPLES");
    let fv = snap_view(&app);
    match &fv.overlay {
        Some(OverlayView::Browser(bv)) => {
            assert!(
                !bv.searching,
                "Enter in Browser search must exit search mode"
            );
            assert_eq!(bv.query, "beta", "Enter must preserve the filter query");
            assert_eq!(bv.entries.len(), 1);
            assert!(bv.entries[0].label.contains("beta-snare"));
        }
        other => panic!("expected Browser overlay, got {other:?}"),
    }
    // `j`/`k` navigate the filtered list without altering query.
    app.press_keys("jk").unwrap();
    let fv2 = snap_view(&app);
    match &fv2.overlay {
        Some(OverlayView::Browser(bv)) => assert_eq!(bv.query, "beta"),
        other => panic!("expected Browser overlay, got {other:?}"),
    }
    // First `Esc` while not searching clears non-empty query without closing Browser.
    app.press_keys("<Esc>").unwrap();
    assert_eq!(app.mode_name(), "SAMPLES");
    let fv3 = snap_view(&app);
    match &fv3.overlay {
        Some(OverlayView::Browser(bv)) => assert!(bv.query.is_empty()),
        other => panic!("expected Browser overlay, got {other:?}"),
    }
    // `/` + type + `Esc` while searching clears query & exits search cleanly (still open).
    app.press_keys("/alpha<Esc>").unwrap();
    assert_eq!(app.mode_name(), "SAMPLES");
    let fv4 = snap_view(&app);
    match &fv4.overlay {
        Some(OverlayView::Browser(bv)) => {
            assert!(!bv.searching);
            assert!(bv.query.is_empty());
        }
        other => panic!("expected Browser overlay, got {other:?}"),
    }
    // Second `Esc` with empty query closes Browser.
    app.press_keys("<Esc>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");

    // Reopen, search `/beta<Enter>` (commits filter) then `<Enter>` (accepts & assigns).
    app.press_keys("B/beta<Enter><Enter>").unwrap();
    wait_browser_close(&mut app);
    assert_eq!(app.mode_name(), "NORMAL");
    let (_, _, _, smp) = app
        .instrument(samp_id)
        .unwrap()
        .rack
        .sampler_modules()
        .next()
        .expect("expected Sampler module");
    assert!(
        smp.path.as_deref().unwrap_or("").contains("beta-snare.wav"),
        "second Enter after filter commit must assign filtered sample: {:?}",
        smp.path
    );

    // --- (b) File Hub ---
    app.press_keys("gf").unwrap();
    assert_eq!(app.mode_name(), "FILES");
    app.press_keys("/beta-song<Enter>").unwrap();
    assert_eq!(app.mode_name(), "FILES");
    match &snap_view(&app).overlay {
        Some(OverlayView::FileHub(hv)) => {
            assert!(
                !hv.searching,
                "Enter in FileHub search must exit search mode"
            );
            assert_eq!(hv.query, "beta-song");
            assert!(hv.rows.iter().any(|r| r.label.contains("beta-song")));
        }
        other => panic!("expected FileHub overlay, got {other:?}"),
    }
    app.press_keys("jk").unwrap();
    match &snap_view(&app).overlay {
        Some(OverlayView::FileHub(hv)) => assert_eq!(hv.query, "beta-song"),
        other => panic!("expected FileHub overlay, got {other:?}"),
    }
    app.press_keys("<Esc>").unwrap();
    assert_eq!(app.mode_name(), "FILES");
    match &snap_view(&app).overlay {
        Some(OverlayView::FileHub(hv)) => assert!(hv.query.is_empty()),
        other => panic!("expected FileHub overlay, got {other:?}"),
    }
    app.press_keys("/alpha<Esc>").unwrap();
    assert_eq!(app.mode_name(), "FILES");
    match &snap_view(&app).overlay {
        Some(OverlayView::FileHub(hv)) => {
            assert!(!hv.searching);
            assert!(hv.query.is_empty());
        }
        other => panic!("expected FileHub overlay, got {other:?}"),
    }
    app.press_keys("<Esc>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");

    // --- (c) Picker (`tQ` Launch Cycle) ---
    app.press_keys("tQ").unwrap();
    assert_eq!(app.mode_name(), "PICK");
    assert!(!app.picker_searching());
    let first_pat_name = app.project().patterns[0].name.clone();
    let needle: String = first_pat_name.chars().take(3).collect();
    app.press_keys(&format!("/{needle}")).unwrap();
    assert!(app.picker_searching());
    assert_eq!(app.picker_query(), needle.as_str());

    // `Enter` while searching commits filter to list nav (stays in PICK).
    app.press_keys("<Enter>").unwrap();
    assert_eq!(app.mode_name(), "PICK");
    assert!(!app.picker_searching());
    assert_eq!(app.picker_query(), needle.as_str());
    // `Esc` while not searching clears non-empty query first (stays in PICK).
    app.press_keys("<Esc>").unwrap();
    assert_eq!(app.mode_name(), "PICK");
    assert_eq!(app.picker_query(), "");
    // `/` + type + `Esc` while searching clears query & exits search (stays in PICK).
    app.press_keys("/zzz<Esc>").unwrap();
    assert_eq!(app.mode_name(), "PICK");
    assert!(!app.picker_searching());
    assert_eq!(app.picker_query(), "");
    // Second `Esc` closes Picker cleanly.
    app.press_keys("<Esc>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");

    // Reopen Picker, `/needle<Enter>` (commits filter) then `<Enter>` (chooses item).
    app.press_keys(&format!("tQ/{needle}<Enter><Enter>"))
        .unwrap();
    assert_eq!(app.mode_name(), "NORMAL");

    // --- (d) Help (`?`) ---
    app.press_keys("?").unwrap();
    assert_eq!(app.mode_name(), "HELP");
    app.press_keys("/take<Enter>").unwrap();
    assert_eq!(app.mode_name(), "HELP");
    match &snap_view(&app).overlay {
        Some(OverlayView::Help(hv)) => {
            assert!(!hv.searching, "Enter in Help search must exit search mode");
            assert_eq!(hv.query, "take");
            assert!(hv.entries.iter().any(|e| e.category == "TAKE DECK"));
        }
        other => panic!("expected Help overlay, got {other:?}"),
    }
    app.press_keys("jk").unwrap();
    match &snap_view(&app).overlay {
        Some(OverlayView::Help(hv)) => assert_eq!(hv.query, "take"),
        other => panic!("expected Help overlay, got {other:?}"),
    }
    app.press_keys("<Esc>").unwrap();
    assert_eq!(app.mode_name(), "HELP");
    match &snap_view(&app).overlay {
        Some(OverlayView::Help(hv)) => assert!(hv.query.is_empty()),
        other => panic!("expected Help overlay, got {other:?}"),
    }
    app.press_keys("/midi<Esc>").unwrap();
    assert_eq!(app.mode_name(), "HELP");
    match &snap_view(&app).overlay {
        Some(OverlayView::Help(hv)) => {
            assert!(!hv.searching);
            assert!(hv.query.is_empty());
        }
        other => panic!("expected Help overlay, got {other:?}"),
    }
    app.press_keys("<Esc>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
}

#[test]
fn help_entries_use_consistent_separators_and_cover_core_ux() {
    for entry in HELP_ENTRIES {
        for part in entry.keys.split(" · ") {
            assert!(
                !part.contains(" / "),
                "HELP_ENTRIES[{}] keys contain spaced slash ` / ` inside token {:?}: {:?}",
                entry.category,
                part,
                entry.keys
            );
        }
        assert!(
            !entry.keys.contains("  "),
            "HELP_ENTRIES[{}] keys contain double space: {:?}",
            entry.category,
            entry.keys
        );
        assert!(
            !entry.summary.contains("  "),
            "HELP_ENTRIES[{}] summary contains double space: {:?}",
            entry.category,
            entry.summary
        );
    }

    let find = |cat: &str| {
        HELP_ENTRIES
            .iter()
            .find(|e| e.category == cat)
            .unwrap_or_else(|| panic!("missing HELP_ENTRIES category {cat}"))
    };
    let search = find("SEARCH");
    assert!(
        search.keys.contains('/') && search.keys.contains("Enter") && search.keys.contains("Esc")
    );
    let take = find("TAKE DECK");
    assert!(take.keys.contains("tr") && take.keys.contains('v') && take.keys.contains('X'));
    let transport = find("TRANSPORT");
    assert!(transport.keys.contains("tr/tR"));
    let samples = find("SAMPLES");
    assert!(samples.keys.contains("Sample row"));
    let panel = find("RACK PANEL");
    assert!(panel.keys.contains("H/L tab") && panel.keys.contains("h/l"));
    let pb = find("PATCHBAY");
    assert!(pb.keys.contains("c patch") && pb.keys.contains("x cut"));
    let zones = find("RACK ZONES");
    assert!(zones.keys.contains("h/l zone") && zones.keys.contains("B sample"));
}

#[test]
fn help_overlay_scroll_clamps_in_browse_and_filtered_search_modes() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = hermetic_app(Project::demo(), dir.path().join("demo.json"), &[]);

    app.press_keys("?").unwrap();
    for _ in 0..200 {
        app.press_keys("j").unwrap();
    }
    let (scroll_browse, count_browse) = match snap_view(&app).overlay {
        Some(OverlayView::Help(h)) => (h.scroll, h.entries.len()),
        _ => panic!("expected Help"),
    };
    assert_eq!(
        scroll_browse,
        count_browse.saturating_sub(1),
        "Help browse scroll must clamp to entries.len() - 1"
    );

    // Filter down to a small subset via `/` and scroll past the bottom.
    app.press_keys("/macro").unwrap();
    for _ in 0..50 {
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
            .unwrap();
    }
    let (scroll_search, count_search) = match snap_view(&app).overlay {
        Some(OverlayView::Help(h)) => (h.scroll, h.entries.len()),
        _ => panic!("expected Help"),
    };
    assert!(count_search > 0);
    assert_eq!(
        scroll_search,
        count_search.saturating_sub(1),
        "Help search scroll must clamp to filtered entries.len() - 1"
    );
    app.press_keys("<Esc><Esc>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
}

#[test]
fn help_overlay_hints_bar_and_fx_usage_cover_all_decks_and_effects() {
    let all_help: String = HELP_ENTRIES
        .iter()
        .map(|e| format!("{} | {} | {}\n", e.category, e.keys, e.summary))
        .collect();

    for required in [
        "zx",
        "gk",
        "gf",
        "S ",
        "tb",
        "tc",
        "tp",
        "te",
        "tr",
        "ts",
        "tC",
        "Limiter",
        "DEVICE & PROFILE",
        "M1–M8",
        "Alt+hjkl",
        "H/L",
        "w/b",
        "EDIT",
        "NOTE+CC REC",
        "5/6-column",
        "8 zero-alloc",
    ] {
        assert!(
            all_help.contains(required),
            "HELP_ENTRIES missing documentation for `{required}`"
        );
    }

    // Zero raw `:cmd` leaks across the entire `?` Help overlay.
    for entry in HELP_ENTRIES {
        for field in [entry.keys, entry.summary] {
            for token in field.split_whitespace() {
                assert!(
                    !(token.starts_with(':')
                        && token.len() > 1
                        && token.as_bytes()[1].is_ascii_alphabetic()),
                    "HELP_ENTRIES[{}] must not leak raw Ex command `{token}`: {field:?}",
                    entry.category
                );
            }
        }
    }

    let gg_entry = HELP_ENTRIES
        .iter()
        .find(|e| e.keys.contains("gg/ge"))
        .expect("gg/ge help entry");
    assert!(gg_entry.summary.contains("song start/end"));

    let mut app = App::new(Project::demo(), PathBuf::from("demo.json")).unwrap();
    app.press_keys("gk").unwrap();
    let mac_hints = app.hints();
    assert!(
        mac_hints.iter().any(|(k, _)| k.contains("1–8")),
        "hints() must have a dedicated macros_focused() branch, got {mac_hints:?}"
    );

    app.press_keys("g").unwrap();
    let go_hints = app.hints();
    assert!(go_hints.iter().any(|(k, v)| k == "k" && v == "Macros"));
    app.press_keys("<Esc>").unwrap();

    app.press_keys("t").unwrap();
    let t_hints = app.hints();
    assert!(t_hints
        .iter()
        .any(|(k, v)| k == "b" && v.contains("bounce")));
    app.press_keys("<Esc>").unwrap();

    let err = app.execute_command("fx add").unwrap_err().to_string();
    for fx in [
        "filter", "comp", "limiter", "delay", "reverb", "eq", "chorus", "drive",
    ] {
        assert!(
            err.contains(fx),
            "`:fx add` usage hint must include `{fx}`, got: {err}"
        );
    }
}

#[test]
fn help_and_inspect_scrolling_preserve_control_cursor_and_ctrl_u_clears_help_query() {
    let mut app = App::new(Project::demo(), PathBuf::from("demo.json")).unwrap();

    let synth_name = app
        .project()
        .instruments
        .iter()
        .find(|i| !i.rack.has_samplers())
        .unwrap()
        .name
        .clone();
    app.execute_command(&format!("instrument \"{synth_name}\""))
        .unwrap();
    app.control_cursor = 5;
    app.press_keys("?jjj<Esc>").unwrap();
    assert_eq!(
        app.control_cursor, 5,
        "Scrolling Help overlay must not clobber control_cursor"
    );
    app.execute_command("inspect").unwrap();
    app.press_keys("jjjj<Esc>").unwrap();
    assert_eq!(
        app.control_cursor, 5,
        "Scrolling Inspect overlay must not clobber control_cursor"
    );

    // `Ctrl-u` in Help search clears `help_ui.query` without closing Help.
    app.press_keys("?/graph").unwrap();
    assert_eq!(app.mode_name(), "HELP");
    assert_eq!(app.help_ui.query, "graph");
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
        .unwrap();
    assert_eq!(
        app.mode_name(),
        "HELP",
        "Ctrl-u in Help search must not close Help"
    );
    assert_eq!(
        app.help_ui.query, "",
        "Ctrl-u in Help search must clear query"
    );
    app.press_keys("<Esc><Esc>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
}
