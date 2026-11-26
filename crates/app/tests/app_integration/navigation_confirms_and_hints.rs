//! Integration tests covering Arrange placement duplication (`d`), unified edge/page
//! navigation across decks and overlays, destructive confirmation guards, overlay `Space`
//! transport, Placement & Song Properties exact `ValueInput`, GrooveBrowser `s` save,
//! contextual hints, Picker bounds, inline Draft `/` search, and MIDI repeat guards.
use cadence_app::drafts::{
    Draft, DraftField, DraftOutcome, WorkflowContext, WorkflowDraft, WorkflowKind, WorkflowOutcome,
};
use cadence_app::view::{OverlayView, HELP_ENTRIES};
use cadence_app::{
    Action, App, ControlTarget, Detail, FileHubTab, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    MemoryHostIo, RackTab,
};
use cadence_core::material::{Edit, Exception, PatternKind, Project};
use cadence_core::mixer::{ChannelRef, EffectKind, EffectParam};
use cadence_core::music::Step;
use cadence_core::params::ParamAddress;
use cadence_core::presets::PresetCatalog;
use cadence_core::runtime::AudioSnapshot;
use std::path::PathBuf;
use std::sync::Arc;

fn demo_app() -> App {
    App::new_in_memory(Project::demo(), PathBuf::from("ux-demo.json")).unwrap()
}

fn memory_app(project: Project) -> (App, Arc<MemoryHostIo>) {
    let io = Arc::new(MemoryHostIo::new("/virtual"));
    let app = App::new_with_host_io(
        project,
        PathBuf::from("/virtual/project.json"),
        Arc::clone(&io) as Arc<dyn cadence_app::HostIo>,
    )
    .unwrap();
    (app, io)
}

#[test]
fn arrange_d_duplicates_placement_preserving_exceptions_extending_bars_and_empty_bar_status() {
    let mut app = demo_app();
    app.press_keys("ga").unwrap();

    // Add a local exception onto lane 0's last placement (free space right after its end bar)
    // so we prove exceptions are preserved and bars auto-extend cleanly!
    let lane0_id = app.project().lanes[0].id;
    let orig = app.project().lanes[0].placements.last().unwrap().clone();
    let mut with_exc = orig.clone();
    with_exc.exceptions.push(Exception {
        step: 0,
        value: Step::default(),
    });
    app.apply(Action::Edit {
        edit: Edit::PutPlacement {
            lane: lane0_id,
            placement: with_exc.clone(),
        },
    })
    .unwrap();

    // Focus the trailing placement in Arrange and press `d`.
    app.selected_lane = Some(lane0_id);
    app.arrange_ui.step = with_exc.start_step;
    app.selected_placement = Some(with_exc.id);

    let count_before = app.lane(lane0_id).unwrap().placements.len();
    let bars_before = app.project().bars;
    app.press_keys("d").unwrap();

    let placements = &app.lane(lane0_id).unwrap().placements;
    assert_eq!(placements.len(), count_before + 1);
    let dup = placements.last().expect("duplicated placement");
    assert_ne!(dup.id, with_exc.id);
    assert_eq!(dup.part, with_exc.part);
    assert_eq!(dup.start_step, with_exc.start_step + with_exc.length_steps);
    assert_eq!(dup.length_steps, with_exc.length_steps);
    assert_eq!(dup.phase, with_exc.phase);
    assert_eq!(dup.offset_steps, with_exc.offset_steps);
    assert_eq!(dup.exceptions, with_exc.exceptions);
    assert_eq!(app.selected_placement, Some(dup.id));
    assert_eq!(app.arrange_ui.step, dup.start_step);
    assert!(app.status.contains("Duplicated"));
    assert!(
        app.project().bars > bars_before,
        "duplicating trailing placement past end of song must auto-extend bars"
    );

    // Single atomic undo removes the duplicate AND restores the previous bar length!
    app.apply(Action::Undo).unwrap();
    assert_eq!(app.lane(lane0_id).unwrap().placements.len(), count_before);
    assert_eq!(app.project().bars, bars_before);

    // Duplicating a placement whose next bar is already occupied rejects cleanly without mutating state.
    if app.lane(lane0_id).unwrap().placements.len() >= 2 {
        let first = app.lane(lane0_id).unwrap().placements[0].clone();
        app.arrange_ui.step = first.start_step;
        app.selected_placement = Some(first.id);
        let rev_before = app.revision();
        assert!(app.press_keys("d").is_err());
        assert_eq!(app.revision(), rev_before);
    }

    // On an empty bar with no placement under cursor, `d` reports a helpful status and does not fail.
    app.execute_command("bars 64").unwrap();
    app.press_keys("$").unwrap();
    assert_eq!(app.selected_placement, None);
    let rev_before = app.revision();
    app.press_keys("d").unwrap();
    assert_eq!(app.revision(), rev_before);
    assert!(app.status.contains("No placement under cursor"));
}

#[test]
fn unified_edge_and_page_navigation_across_graph_take_macros_song_props_and_overlays() {
    let (mut app, _) = memory_app(Project::demo());

    // 1. Signal Graph: 0 / $ / G / End / gg / ge / PgUp / PgDn.
    app.press_keys("gG").unwrap();
    assert!(app.graph_focused());
    app.press_keys("$").unwrap();
    let graph_end = app.view_state.graph_row;
    assert!(
        graph_end > 0,
        "expected multiple rows in Signal Graph column 0"
    );
    app.press_keys("0").unwrap();
    assert_eq!(app.view_state.graph_row, 0);
    app.press_keys("ge").unwrap();
    assert_eq!(app.view_state.graph_row, graph_end);
    app.press_keys("gg").unwrap();
    assert_eq!(app.view_state.graph_row, 0);
    app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE))
        .unwrap();
    assert!(app.view_state.graph_row > 0);
    app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.view_state.graph_row, 0);

    // 2. Take deck: gg / ge / 0 / $ / PgDn / PgUp.
    app.press_keys("tr ").unwrap();
    app.apply(Action::Advance { steps: 128 }).unwrap(); // 8 bars
    app.press_keys("tr<Home>gt").unwrap();
    assert!(app.take_focused());
    app.press_keys("ge").unwrap();
    assert!(app.takes.cursor_bar >= 7);
    app.press_keys("gg").unwrap();
    assert_eq!(app.takes.cursor_bar, 0);
    app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.takes.cursor_bar, 4);
    app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.takes.cursor_bar, 0);

    // 3. Macros deck: gg / ge + PgDn / PgUp across bindings + Alt+J / Alt+K reorder!
    let lead_ch = app.project().mixer.channels[0].id;
    let saw_id = app.project().instruments[0].id;
    let addr0 = ParamAddress::Channel {
        channel: ChannelRef::Channel(lead_ch),
        param: cadence_core::mixer::ChannelParam::Gain,
    };
    let addr1 = ParamAddress::Channel {
        channel: ChannelRef::Channel(lead_ch),
        param: cadence_core::mixer::ChannelParam::Pan,
    };
    let addr2 = ParamAddress::Rack {
        instrument: saw_id,
        param: cadence_core::rack::RackParam::Gain,
    };
    for addr in [addr0, addr1, addr2] {
        app.apply(Action::BindMacro {
            index: 0,
            target: addr,
            min: 0.0,
            max: 1.0,
        })
        .unwrap();
    }
    app.press_keys("gk").unwrap();
    app.press_keys("ge").unwrap();
    assert_eq!(app.view_state.macro_cursor, 7);
    app.press_keys("gg").unwrap();
    assert_eq!(app.view_state.macro_cursor, 0);
    assert_eq!(app.view_state.macro_binding_cursor, None);
    app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.view_state.macro_binding_cursor, Some(2));
    // Reorder binding 2 up with Alt+K (shifted!) -> moves earlier to index 1!
    app.press_keys("<m-K>").unwrap();
    assert_eq!(
        app.project().macro_control(0).unwrap().bindings[1].target,
        addr2
    );
    // Reorder binding 1 down with Alt+J (shifted!) -> moves back to index 2!
    app.press_keys("<m-J>").unwrap();
    assert_eq!(
        app.project().macro_control(0).unwrap().bindings[2].target,
        addr2
    );
    app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.view_state.macro_binding_cursor, None);

    // 4. Song Properties: 0 / $ / G / End / PgUp / PgDn + Enter ValueInput on Late Tolerance + printable title guard.
    app.press_keys("gP").unwrap();
    assert_eq!(app.mode_name(), "SONG PROPS");
    app.press_keys("$").unwrap();
    let sp_end = app
        .song_properties_view()
        .unwrap()
        .rows
        .iter()
        .position(|r| r.selected)
        .unwrap();
    assert_eq!(
        sp_end, 8,
        "last Song Properties row is Late Tolerance (index 8)"
    );
    // Enter on Late Tolerance opens ValueInput, commits 3, and returns cleanly to Song Properties!
    app.press_keys("<Enter>3<Enter>").unwrap();
    assert_eq!(app.mode_name(), "SONG PROPS");
    assert_eq!(app.project().timing.late_tolerance_steps, 3);
    app.press_keys("0").unwrap();
    assert!(app.song_properties_view().unwrap().rows[0].selected);
    app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE))
        .unwrap();
    assert!(app.song_properties_view().unwrap().rows[6].selected);
    app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE))
        .unwrap();
    assert!(app.song_properties_view().unwrap().rows[0].selected);
    app.press_keys("<Esc>").unwrap();

    // 5. Overlays: Help, Inspect (scroll clamped!), FileHub, Picker edge & page nav.
    app.press_keys("?").unwrap();
    assert_eq!(app.mode_name(), "HELP");
    app.press_keys("$").unwrap();
    let help_end = app.help_ui.overlay_scroll;
    assert!(help_end > 0);
    app.press_keys("0").unwrap();
    assert_eq!(app.help_ui.overlay_scroll, 0);
    app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE))
        .unwrap();
    assert!(app.help_ui.overlay_scroll > 0);
    app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.help_ui.overlay_scroll, 0);
    app.press_keys("<Esc>").unwrap();

    // Inspect overlay clamps scroll against inspect_overlay_lines()!
    app.press_keys("gpI").unwrap();
    assert_eq!(app.mode_name(), "INSPECT");
    let snap = AudioSnapshot::default();
    let total_lines = match app.frame_view(&snap, "test").overlay {
        Some(OverlayView::Inspect { lines, .. }) => lines.len(),
        other => panic!("expected Inspect overlay, got {other:?}"),
    };
    app.press_keys("G").unwrap();
    assert_eq!(app.help_ui.overlay_scroll, total_lines.saturating_sub(1));
    // Pressing `j` and `PageDown` at bottom MUST stay clamped and never overflow!
    app.press_keys("jjj").unwrap();
    app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.help_ui.overlay_scroll, total_lines.saturating_sub(1));
    app.press_keys("0").unwrap();
    assert_eq!(app.help_ui.overlay_scroll, 0);
    app.press_keys("<Esc>").unwrap();

    // FileHub edge & page nav on Save tab (6 rows: 0..=5).
    app.open_file_hub(FileHubTab::Save);
    app.press_keys("$").unwrap();
    let save_cursor = match app.frame_view(&snap, "test").overlay {
        Some(OverlayView::FileHub(fh)) => fh.cursor,
        other => panic!("expected FileHub, got {other:?}"),
    };
    assert_eq!(save_cursor, 5);
    app.press_keys("0").unwrap();
    let save_cursor_top = match app.frame_view(&snap, "test").overlay {
        Some(OverlayView::FileHub(fh)) => fh.cursor,
        other => panic!("expected FileHub, got {other:?}"),
    };
    assert_eq!(save_cursor_top, 0);
    app.press_keys("<Esc>").unwrap();
}

#[test]
fn palette_discard_take_and_bounce_and_x_confirm_guards_on_section_and_effect() {
    let (mut app, io) = memory_app(Project::demo());
    app.set_session_takes_dir(PathBuf::from("/virtual/session_takes"));

    // 1. Record a non-empty unsaved session take, seal it, then invoke Palette -> Discard Take!
    app.press_keys("tr ").unwrap();
    app.execute_command("bpm 133").unwrap();
    app.apply(Action::Advance { steps: 32 }).unwrap();
    app.press_keys("tr<Home>").unwrap();
    let session_files_before: Vec<PathBuf> = io
        .file_paths()
        .into_iter()
        .filter(|p| p.to_string_lossy().ends_with(".take.jsonl"))
        .collect();
    assert_eq!(session_files_before.len(), 1);

    app.press_keys(":Discard Take<Enter>").unwrap();
    assert_eq!(
        app.mode_name(),
        "CONFIRM",
        "Palette DiscardTake must guard unsaved take behind Mode::Confirm"
    );
    app.press_keys("<Esc>").unwrap();
    assert!(app.current_take().is_some(), "Esc preserves take");
    assert!(io.read_file(&session_files_before[0]).is_some());

    // Confirming discard clears take AND calls HostIo::remove_file on the unkept session file!
    app.press_keys(":Discard Take<Enter>y").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert!(app.current_take().is_none());
    assert!(
        io.read_file(&session_files_before[0]).is_none(),
        "DiscardTake must remove unkept session take journal via HostIo::remove_file"
    );

    // 2. Palette BounceSelection opens the Bounce modal dialog (not silent immediate bounce).
    app.press_keys(":Bounce<Enter>").unwrap();
    assert_eq!(
        app.mode_name(),
        "WORKFLOW",
        "Palette BounceSelection must open the Bounce dialog"
    );
    app.press_keys("<Esc>").unwrap();

    // 3. `X` on Arrange Section enters Mode::Confirm with section name & bar range.
    app.press_keys("ga0").unwrap();
    let sec = app.project().sections[0].clone();
    app.arrange_ui.step = sec.start_step;
    app.press_keys("X").unwrap();
    assert_eq!(app.mode_name(), "CONFIRM");
    let prompt = app.confirm_detail().unwrap();
    assert!(
        prompt.contains(&sec.name),
        "Section confirm must name the section, got: {prompt}"
    );
    app.press_keys("<Esc>").unwrap();
    assert!(app.project().sections.iter().any(|s| s.id == sec.id));
    app.press_keys("Xy").unwrap();
    assert!(!app.project().sections.iter().any(|s| s.id == sec.id));
    app.apply(Action::Undo).unwrap();
    assert!(app.project().sections.iter().any(|s| s.id == sec.id));

    // 4. `X` on Detail::Effect enters Mode::Confirm reporting dependent mod/macro counts!
    let delay_id = app
        .project()
        .mixer
        .channels
        .iter()
        .flat_map(|c| c.inserts.iter())
        .find(|d| matches!(d.kind, EffectKind::Delay(_)))
        .expect("Delay insert")
        .id;
    app.apply(Action::BindMacro {
        index: 0,
        target: ParamAddress::Effect {
            device: delay_id,
            param: EffectParam::Mix,
        },
        min: 0.0,
        max: 1.0,
    })
    .unwrap();
    app.execute_command(&format!("fx {delay_id}")).unwrap();
    app.press_keys("X").unwrap();
    assert_eq!(app.mode_name(), "CONFIRM");
    let fx_prompt = app.confirm_detail().unwrap();
    assert!(
        fx_prompt.contains("1 macro binding"),
        "Effect confirm must report dependent macro bindings, got: {fx_prompt}"
    );
    app.press_keys("<Esc>").unwrap();
    assert!(app.effect(delay_id).is_some());
    app.press_keys("Xy").unwrap();
    assert!(app.effect(delay_id).is_none());
    assert!(app.project().macro_control(0).unwrap().bindings.is_empty());
}

#[test]
fn space_play_pause_placement_value_input_and_groove_browser_save() {
    let mut app = demo_app();

    // 1. Space toggles play/pause inside SampleBrowser, FileHub, and Help.
    assert!(!app.playing());
    app.press_keys("B").unwrap();
    assert_eq!(app.mode_name(), "SAMPLES");
    app.press_keys(" ").unwrap();
    assert!(app.playing(), "Space must start playback in SampleBrowser");
    // Repeat Space must NOT double-toggle!
    app.handle_key(KeyEvent::new_with_kind(
        KeyCode::Char(' '),
        KeyModifiers::NONE,
        KeyEventKind::Repeat,
    ))
    .unwrap();
    assert!(app.playing());
    app.press_keys(" <Esc>").unwrap();
    assert!(!app.playing());

    app.press_keys("gf").unwrap();
    assert_eq!(app.mode_name(), "FILES");
    app.press_keys(" ").unwrap();
    assert!(app.playing(), "Space must start playback in FileHub");
    app.press_keys(" <Esc>").unwrap();
    assert!(!app.playing());

    app.press_keys("?").unwrap();
    assert_eq!(app.mode_name(), "HELP");
    app.press_keys(" ").unwrap();
    assert!(app.playing(), "Space must start playback in Help");
    app.press_keys(" <Esc>").unwrap();
    assert!(!app.playing());

    // 2. Detail::Placement `Starts` and `Length` exact ValueInput via Enter!
    let pl_id = app.project().lanes[0].placements.last().unwrap().id;
    app.apply(Action::SelectPlacement { placement: pl_id })
        .unwrap();
    assert_eq!(app.detail, Detail::Placement(pl_id));

    let start_idx = app
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::PlacementStart(id) if id == pl_id))
        .expect("PlacementStart control");
    app.control_cursor = start_idx;
    app.press_keys("<Enter>20b<Enter>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert_eq!(app.placement(pl_id).unwrap().1.start_step, 19 * 16);

    let len_idx = app
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::PlacementLength(id) if id == pl_id))
        .expect("PlacementLength control");
    app.control_cursor = len_idx;
    app.press_keys("<Enter>24s<Enter>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert_eq!(app.placement(pl_id).unwrap().1.length_steps, 24);

    // Rejects length < 4 steps and keeps ValueInput open with error.
    app.control_cursor = len_idx;
    app.press_keys("<Enter>2s<Enter>").unwrap();
    assert_eq!(app.mode_name(), "VALUE ENTRY");
    app.press_keys("<Esc>").unwrap();
    assert_eq!(app.placement(pl_id).unwrap().1.length_steps, 24);

    // 3. GrooveBrowser `s` restores preview and opens Save Filter Groove ValueInput!
    let mut app = App::new_with_library(
        Project::demo(),
        PathBuf::from("ux-demo.json"),
        cadence_core::sampler::SampleLibrary::from_roots(&[], None),
    )
    .unwrap();
    app.execute_command("fx add filter master").unwrap();
    let filter_id = app.project().mixer.master.inserts.last().unwrap().id;
    app.execute_command(&format!("fx {filter_id}")).unwrap();
    let orig_groove = match app.effect(filter_id).unwrap().1.kind {
        EffectKind::Filter(fp) => fp.env_pattern,
        _ => panic!("expected Filter"),
    };
    app.press_keys("P").unwrap();
    assert_eq!(app.mode_name(), "FILTER GROOVES");
    assert!(
        app.hints()
            .iter()
            .any(|(k, d)| k == "s" && d.contains("save")),
        "GrooveBrowser hints must advertise s save groove"
    );
    app.press_keys("jjs").unwrap();
    assert_eq!(app.mode_name(), "VALUE ENTRY");
    assert!(app.status.contains("Save Filter Groove"));
    assert!(
        matches!(app.effect(filter_id).unwrap().1.kind, EffectKind::Filter(fp) if fp.env_pattern == orig_groove),
        "GrooveBrowser `s` must restore untouched original groove before saving"
    );
    app.press_keys("<Esc>").unwrap();
}

#[test]
fn rhythm_staging_links_nonliteral_filehub_hints_and_help_rename_coverage() {
    let mut app = demo_app();

    // 1. PatternKind::Rhythm dedicated hints branch (`Enter/x` toggle hit, NOT Notes `voice`/`pitch`).
    let rhythm_id = app
        .project()
        .patterns
        .iter()
        .find(|p| matches!(p.kind, PatternKind::Rhythm { .. }))
        .map(|p| p.id)
        .unwrap_or_else(|| {
            app.press_keys("gp").unwrap();
            app.press_keys("nrTestRhy<Enter>16<Enter>").unwrap();
            app.selected_pattern.unwrap()
        });
    app.apply(Action::SelectPattern { pattern: rhythm_id })
        .unwrap();
    let rhy_hints = app.hints();
    assert!(
        rhy_hints
            .iter()
            .any(|(k, d)| k == "Enter/x" && d.contains("toggle hit")),
        "Rhythm hints must show Enter/x toggle hit, got: {rhy_hints:?}"
    );
    assert!(
        !rhy_hints.iter().any(|(_, d)| d == "pitch" || d == "voice"),
        "Rhythm hints must NOT fall through to Notes hints: {rhy_hints:?}"
    );

    // 2. Normal-mode Staging hints (`S/Alt+Enter` fire, `u` pop, `Esc` discard).
    app.press_keys("S").unwrap();
    let stage_hints = app.hints();
    assert!(stage_hints
        .iter()
        .any(|(k, d)| k == "S/Alt+Enter" && d == "fire"));
    assert!(stage_hints.iter().any(|(k, d)| k == "u" && d == "pop"));
    assert!(stage_hints
        .iter()
        .any(|(k, d)| k == "Esc" && d == "discard"));
    app.press_keys("<Esc>").unwrap();

    // 3. `e links` cue on Detail::Effect, Detail::Part, and Rack Panel/Patchbay/Zones.
    let delay_id = app
        .project()
        .mixer
        .channels
        .iter()
        .flat_map(|c| c.inserts.iter())
        .find(|d| matches!(d.kind, EffectKind::Delay(_)))
        .unwrap()
        .id;
    app.execute_command(&format!("fx {delay_id}")).unwrap();
    assert!(app
        .hints()
        .iter()
        .any(|(k, d)| k.contains('e') && d.contains("links")));

    let part0 = app.project().parts[0].id;
    app.execute_command(&format!("part {part0}")).unwrap();
    assert!(app
        .hints()
        .iter()
        .any(|(k, d)| k.contains('e') && d.contains("links")));

    let saw_id = app.project().instruments[0].id;
    app.execute_command(&format!("instrument {saw_id}"))
        .unwrap();
    for tab in [RackTab::Panel, RackTab::Patchbay, RackTab::Zones] {
        app.rack_ui.tab = tab;
        let h = app.hints();
        assert!(
            h.iter()
                .any(|(k, d)| k.contains('e') && d.contains("links")),
            "Rack {tab:?} hints must advertise e links, got: {h:?}"
        );
    }

    // 4. F/f/I/X cues on Derived, Mini, and Euclidean patterns.
    app.press_keys("gp").unwrap();
    let derived_id = app
        .project()
        .patterns
        .iter()
        .find(|p| !p.kind.sources().is_empty())
        .expect("derived pattern in demo")
        .id;
    app.apply(Action::SelectPattern {
        pattern: derived_id,
    })
    .unwrap();
    assert!(app
        .hints()
        .iter()
        .any(|(k, _)| k.contains('F') && k.contains('f') && k.contains('I') && k.contains('X')));

    // 5. Bank Parts uses `Enter/Shift+Enter` (not `S-Enter`).
    app.press_keys("gb").unwrap();
    let bank_hints = app.hints();
    assert!(bank_hints.iter().any(|(k, _)| k == "Enter/Shift+Enter"));
    assert!(!bank_hints.iter().any(|(k, _)| k.contains("S-Enter")));

    // 6. FileHub `R` reload hint appears ONLY on Import tab; `Bksp` appears on Browse/Import only.
    app.open_file_hub(FileHubTab::Quick);
    assert!(!app.hints().iter().any(|(k, _)| k == "R"));
    assert!(!app.hints().iter().any(|(k, _)| k == "Bksp"));
    app.open_file_hub(FileHubTab::Browse);
    assert!(!app.hints().iter().any(|(k, _)| k == "R"));
    assert!(app.hints().iter().any(|(k, _)| k == "Bksp"));
    app.open_file_hub(FileHubTab::Import);
    assert!(app.hints().iter().any(|(k, _)| k == "R"));
    assert!(app.hints().iter().any(|(k, _)| k == "Bksp"));
    app.press_keys("<Esc>").unwrap();

    // 7. `?` Help documents rename across entities cleanly.
    assert!(HELP_ENTRIES.iter().any(|e| e.category == "RENAME"
        && e.summary.contains("instrument")
        && e.summary.contains("lane")));
}

#[test]
fn picker_out_of_range_digit_draft_slash_search_mod_command_and_midi_repeat_guard() {
    let mut app = demo_app();

    // 1. Picker out-of-range digit stays open cleanly (never aborts to Normal!).
    app.press_keys("gm").unwrap();
    app.press_keys("a").unwrap(); // Mixer Add picker (3-4 items)
    assert_eq!(app.mode_name(), "PICK");
    let item_count = app.picker_view().unwrap().items.len();
    assert!(item_count < 9);
    app.press_keys("9").unwrap();
    assert_eq!(
        app.mode_name(),
        "PICK",
        "out-of-range digit in Picker must keep Picker open"
    );
    assert!(app.status.contains("out of range"));
    // Picker edge/page nav works while open.
    app.press_keys("G").unwrap();
    assert_eq!(app.picker_view().unwrap().cursor, item_count - 1);
    app.press_keys("g").unwrap();
    assert_eq!(app.picker_view().unwrap().cursor, 0);
    app.press_keys("<Esc>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");

    // 2. Inline Pattern Draft `/` search: Enter locks filter, Esc clears locked filter before closing.
    let proj = app.project().clone();
    let mut pd = Draft::derive(None);
    for ch in "TestDer".chars() {
        pd.handle_key(&proj, KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    pd.handle_key(&proj, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    pd.handle_key(&proj, KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE)); // Transpose -> Notes field
    assert_eq!(pd.view(&proj).field, DraftField::Notes);
    pd.handle_key(&proj, KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    pd.handle_key(&proj, KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    assert!(pd.view(&proj).searching);
    // Enter locks filter without leaving Notes field.
    assert_eq!(
        pd.handle_key(&proj, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        DraftOutcome::Continue
    );
    assert!(!pd.view(&proj).searching);
    assert_eq!(pd.view(&proj).query, "a");
    assert_eq!(pd.view(&proj).field, DraftField::Notes);
    // First Esc clears the locked filter without cancelling the draft.
    assert_eq!(
        pd.handle_key(&proj, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        DraftOutcome::Continue
    );
    assert!(pd.view(&proj).query.is_empty());
    // Second Esc cancels the draft cleanly.
    assert_eq!(
        pd.handle_key(&proj, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        DraftOutcome::Cancel
    );

    // 3. Inline Workflow Draft `/` search: Enter locks filter, Esc clears locked filter before closing.
    let presets = PresetCatalog::empty();
    let mut wd = WorkflowDraft::new(
        WorkflowKind::Send,
        &proj,
        WorkflowContext::default(),
        &presets,
    );
    wd.handle_key(&proj, KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    wd.handle_key(&proj, KeyEvent::new(KeyCode::Char('L'), KeyModifiers::NONE));
    assert!(wd.view(&proj).searching);
    assert_eq!(
        wd.handle_key(&proj, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        WorkflowOutcome::Continue
    );
    assert!(!wd.view(&proj).searching);
    assert_eq!(wd.view(&proj).field, DraftField::SendSource);
    assert_eq!(
        wd.handle_key(&proj, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        WorkflowOutcome::Continue
    );
    assert!(wd.view(&proj).query.is_empty());
    assert_eq!(
        wd.handle_key(&proj, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        WorkflowOutcome::Cancel
    );

    // 4. `:mod` Ex command routes through `parse_param_address` cleanly (incl. channel send:<RETURN>).
    let val_pat = app
        .project()
        .patterns
        .iter()
        .find(|p| {
            app.project().pattern_type(p.id).is_ok_and(|t| {
                t == cadence_core::material::PatternType::Values(
                    cadence_core::material::ValueUnit::Normalized,
                )
            })
        })
        .unwrap()
        .id;
    let lead_name = app.project().mixer.channels[0].name.clone();
    app.execute_command(&format!(
        "mod LeadEchoMod channel {lead_name} send:Echo {val_pat}"
    ))
    .unwrap();
    assert!(app
        .project()
        .modulations
        .iter()
        .any(|m| m.name == "LeadEchoMod"));

    // 5. MIDI deck key-repeat guard on Alt+l / `.` prevents cycling device/profile rows on repeat.
    app.press_keys("gd").unwrap();
    assert!(app.midi_focused());
    app.midi.cursor = 4; // Pickup row (boolean toggle)
    let pickup_before = app.config.midi_pickup;
    app.handle_key(KeyEvent::new_with_kind(
        KeyCode::Char('.'),
        KeyModifiers::NONE,
        KeyEventKind::Repeat,
    ))
    .unwrap();
    assert_eq!(
        app.config.midi_pickup, pickup_before,
        "repeated `.` on MIDI deck must not toggle row"
    );
    app.handle_key(KeyEvent::new_with_kind(
        KeyCode::Char('l'),
        KeyModifiers::ALT,
        KeyEventKind::Repeat,
    ))
    .unwrap();
    assert_eq!(
        app.config.midi_pickup, pickup_before,
        "repeated Alt+l on MIDI deck must not toggle row"
    );
}
