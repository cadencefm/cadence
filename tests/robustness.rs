//! Robustness tests: input panics, wrong-deck verbs, stray modifiers, key repeat,
//! document replacement, quit rules, and strict file/take validation.
use cadence_app::keys::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use cadence_app::{load_project, Action, App};
use cadence_core::{
    material::{PatternKind, Project},
    sampler::SampleLibrary,
    take::Take,
};
use cadence_engine::midi::MidiMessage;
use std::fs;

fn hermetic_app(project: Project, path: std::path::PathBuf) -> App {
    let lib = SampleLibrary::from_roots(&[], Some(&path));
    App::new_with_library(project, path, lib).unwrap()
}

#[test]
fn midi_messages_never_panic_including_out_of_range_program_changes() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = hermetic_app(Project::demo(), dir.path().join("demo.json"));
    // Program changes 0..=127 must never panic, even past SECTION_KEYS.len() (10)
    for program in 0..=127u8 {
        let _ = app.handle_midi(MidiMessage::ProgramChange {
            channel: 0,
            program,
        });
    }
    for note in 0..=127u8 {
        let _ = app.handle_midi(MidiMessage::NoteOn {
            channel: 0,
            note,
            velocity: 100,
        });
        let _ = app.handle_midi(MidiMessage::NoteOff { channel: 0, note });
    }
    for cc in 0..=127u8 {
        let _ = app.handle_midi(MidiMessage::Control {
            channel: 0,
            controller: cc,
            value: 64,
        });
    }
}

#[test]
fn pinned_placement_with_focused_derived_pattern_ignores_x() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = hermetic_app(Project::demo(), dir.path().join("demo.json"));
    let placements_before: usize = app.project().lanes.iter().map(|l| l.placements.len()).sum();
    // Open a placement and pin its Placement deck
    app.press_keys("ga<Enter>zp").unwrap();
    // Open a derived pattern (Moving = id 4 in demo) and select its second source with `j`
    let derived = app
        .project()
        .patterns
        .iter()
        .find(|p| !p.kind.sources().is_empty())
        .unwrap()
        .id;
    app.apply(Action::SelectPattern { pattern: derived })
        .unwrap();
    app.press_keys("j").unwrap();
    // Pressing `x` on the derived pattern must NOT delete the pinned Placement
    app.press_keys("x").unwrap();
    let placements_after: usize = app.project().lanes.iter().map(|l| l.placements.len()).sum();
    assert_eq!(placements_before, placements_after);
}

#[test]
fn unhandled_alt_and_ctrl_chords_do_nothing_and_repeat_is_guarded() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = hermetic_app(Project::demo(), dir.path().join("demo.json"));
    let before = app.project().clone();
    // Stray Alt-z or Ctrl-x must not fall through to `z` (Window mode) or `x` (delete)
    app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::ALT))
        .unwrap();
    assert!(!app.is_window_prefix());
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL))
        .unwrap();
    assert_eq!(app.project(), &before);

    // Repeat on `\`, `?`, `a`, `P` does nothing
    for ch in ['\\', '?', 'a', 'P'] {
        let out = app
            .handle_key(KeyEvent::new_with_kind(
                KeyCode::Char(ch),
                KeyModifiers::NONE,
                KeyEventKind::Repeat,
            ))
            .unwrap();
        assert!(!out.changed && !out.quit);
        assert!(!app.show_help());
    }
}

#[test]
fn quit_rules_bare_q_ignored_ctrl_c_cancels_or_prompts_and_help_returns_to_jam() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = hermetic_app(Project::demo(), dir.path().join("demo.json"));

    // Bare `q` does not quit
    let out = app
        .handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
        .unwrap();
    assert!(!out.quit);

    // Help from Jam returns to Jam, and `qq` never quits
    app.press_keys("gj?<Esc>").unwrap();
    assert!(app.is_jam());
    for _ in 0..2 {
        if let Ok(out) = app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)) {
            assert!(!out.quit);
        }
    }
    assert!(app.is_jam());

    // Ctrl-c cancels Jam back to Normal without quitting
    let out = app
        .handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
        .unwrap();
    assert!(!out.quit);
    assert!(!app.is_jam());
    assert!(!app.confirm_quit());

    // Ctrl-c in Normal asks before quitting; only y, Enter and Esc work
    let out = app
        .handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
        .unwrap();
    assert!(!out.quit);
    assert!(app.confirm_quit());
    // `q` and `s` are ignored in Confirm
    let out = app
        .handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
        .unwrap();
    assert!(!out.quit && app.confirm_quit());
    // Esc cancels
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .unwrap();
    assert!(!app.confirm_quit());

    // `:q` on clean doc quits directly; on dirty doc prompts, `y` confirms
    let out = app.execute_command("q").unwrap();
    assert!(out.quit);

    app.execute_command("bpm 140").unwrap();
    assert!(app.dirty());
    let out = app.execute_command("q").unwrap();
    assert!(!out.quit && app.confirm_quit());
    let out = app
        .handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))
        .unwrap();
    assert!(out.quit);
}

#[test]
fn document_replacement_resets_performance_tracks_save_path_and_seals_take_on_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first.json");
    let second = dir.path().join("second.json");
    let session_takes = dir.path().join("session-takes");

    let mut p1 = Project::demo();
    p1.title = "First Song".into();
    let mut p2 = Project::demo();
    p2.title = "Second Song".into();

    let app1 = hermetic_app(p1.clone(), first.clone());
    app1.save_project(&first).unwrap();
    let app2 = hermetic_app(p2.clone(), second.clone());
    app2.save_project(&second).unwrap();

    // Open while performing silences the old song and seals the active take
    let mut app = hermetic_app(p1, first.clone());
    app.set_session_takes_dir(session_takes);
    app.apply(Action::SetTakeArmed { armed: true }).unwrap();
    app.apply(Action::Play { playing: true }).unwrap();
    let open_take_path = app.active_take_path().unwrap().to_path_buf();
    app.apply(Action::Advance { steps: 16 }).unwrap();

    let out = app
        .apply(Action::Load {
            path: second.clone(),
        })
        .unwrap();
    assert!(!app.playing());
    assert!(!app.overriding());
    assert!(out.project_boundary);
    assert_eq!(app.document().save_path(), &second);
    // Take was sealed cleanly on Open
    assert!(Take::load(&open_take_path).is_ok());

    // Undo across Open restores first file's save path so `:w` writes to `first`
    app.apply(Action::Undo).unwrap();
    assert_eq!(app.document().save_path(), &first);
    app.execute_command("bpm 144").unwrap();
    app.execute_command("w").unwrap();
    assert_eq!(load_project(&first).unwrap().bpm, 144.0);
    assert_eq!(load_project(&second).unwrap().bpm, 136.0);

    // `:new!` starts untitled so `:w` without path refuses to overwrite `first`
    app.execute_command("new!").unwrap();
    assert!(app.document().is_untitled());
    assert!(app.execute_command("w").is_err());

    // Quitting mid-take via shutdown() leaves a loadable take on disk
    app.apply(Action::Play { playing: true }).unwrap();
    let shutdown_take = app.active_take_path().unwrap().to_path_buf();
    app.apply(Action::Advance { steps: 8 }).unwrap();
    let out = app.shutdown();
    assert!(!app.playing());
    assert!(out.stop_audition);
    assert!(Take::load(&shutdown_take).is_ok());
}

#[test]
fn armed_take_writes_minimal_undo_deltas_seals_at_stopping_end_and_recovers() {
    use cadence_core::music::EndRule;
    use cadence_core::take::TakeChange;

    let dir = tempfile::tempdir().unwrap();
    let session_takes = dir.path().join("takes");
    let mut project = Project::demo();
    project.bars = 2;
    project.end = EndRule::Stop;
    for lane in &mut project.lanes {
        lane.placements.retain(|p| p.start_step < 32);
        for p in &mut lane.placements {
            p.length_steps = p.length_steps.min(32 - p.start_step);
        }
    }
    project.sections.clear();

    let mut app = hermetic_app(project, dir.path().join("stop.json"));
    app.set_session_takes_dir(session_takes);
    app.apply(Action::SetTakeArmed { armed: true }).unwrap();
    app.apply(Action::Play { playing: true }).unwrap();
    let take_path = app.active_take_path().unwrap().to_path_buf();

    // Edit tempo and undo mid-take: journal must store compact Edits, never Replace!
    app.execute_command("bpm 140").unwrap();
    app.apply(Action::Advance { steps: 16 }).unwrap();
    app.apply(Action::Undo).unwrap();
    // Advance past the 2-bar stopping end (32 steps): transport stops and seals the take!
    app.apply(Action::Advance { steps: 32 }).unwrap();
    assert!(!app.playing());
    assert!(
        app.take_recording().is_none(),
        "stopping end must auto-seal the take"
    );
    let take = Take::load(&take_path).unwrap();
    assert_eq!(take.end_step, 32);
    assert!(
        take.events
            .iter()
            .all(|e| !matches!(e.change, TakeChange::Replace { .. })),
        "undo must emit minimal Edits, never full-project Replace: {:?}",
        take.events
    );
    assert!(
        take.events
            .iter()
            .any(|e| matches!(e.change, TakeChange::Edits { .. })),
        "undo delta must be recorded as TakeChange::Edits"
    );

    // Crash-recovery restore marks dirty without touching disk and is undoable back to disk.
    let mut recovered = app.project().clone();
    recovered.bpm = 152.0;
    app.restore_recovered_snapshot(recovered, app.flags().clone())
        .unwrap();
    assert!(app.dirty());
    assert_eq!(app.project().bpm, 152.0);
    app.apply(Action::Undo).unwrap();
    assert_eq!(app.project().bpm, 136.0);
}

#[test]
fn bad_projects_and_takes_are_rejected_without_panicking() {
    let dir = tempfile::tempdir().unwrap();
    let bad_proj = dir.path().join("bad.json");
    for payload in ["", "{", "{\"version\":5}", "null", "\u{0}\u{1}\u{2}"] {
        fs::write(&bad_proj, payload).unwrap();
        assert!(load_project(&bad_proj).is_err());
    }

    // Valid take with trailing garbage after end record must be rejected
    let valid_take = dir.path().join("take.jsonl");
    let mut app = hermetic_app(Project::demo(), dir.path().join("demo.json"));
    app.apply(Action::SetTakeArmed { armed: true }).unwrap();
    app.apply(Action::Play { playing: true }).unwrap();
    app.apply(Action::Advance { steps: 16 }).unwrap();
    app.apply(Action::SaveTake {
        path: Some(valid_take.clone()),
    })
    .unwrap();
    assert!(Take::load(&valid_take).is_ok());

    let mut content = fs::read_to_string(&valid_take).unwrap();
    content.push_str("{\"at\":32,\"label\":\"trailing\",\"change\":{\"op\":\"release_all\"}}\n");
    fs::write(&valid_take, content).unwrap();
    assert!(
        Take::load(&valid_take).is_err(),
        "trailing line after end record must be rejected"
    );

    // Note nudges reject non-note patterns cleanly and value nudges clamp
    let mut app = hermetic_app(Project::demo(), dir.path().join("demo.json"));
    let val_pat = app
        .project()
        .patterns
        .iter()
        .find(|p| matches!(p.kind, PatternKind::Values { .. }))
        .unwrap()
        .id;
    app.apply(Action::SelectPattern { pattern: val_pat })
        .unwrap();
    for _ in 0..150 {
        let _ = app.press_keys("=");
    }
    assert!(app.project().validate().is_ok());
}

#[test]
fn sampler_envelope_range_is_unified_and_held_nudges_coalesce_to_one_undo() {
    use cadence_core::{
        music::Step,
        params::{apply_rack_modulation, InstrumentParam},
        rack::{ModuleKind, ModuleParam, RackPatch},
    };

    // Unified Rack Env range: Attack 0.1..=10000 ms, Decay/Release 1..=20000 ms
    let mut rack = RackPatch::default_sampler();
    let attack_p = InstrumentParam::Module {
        zone: 1,
        module: 4,
        param: ModuleParam::Attack,
    };
    let decay_p = InstrumentParam::Module {
        zone: 1,
        module: 4,
        param: ModuleParam::Decay,
    };
    let release_p = InstrumentParam::Module {
        zone: 1,
        module: 4,
        param: ModuleParam::Release,
    };
    apply_rack_modulation(&mut rack, attack_p, 1.0, 1000);
    apply_rack_modulation(&mut rack, decay_p, 1.0, 1000);
    apply_rack_modulation(&mut rack, release_p, 1.0, 1000);
    let ModuleKind::Env(env) = rack.zones[0].module(4).unwrap().kind else {
        panic!("expected Amp Env")
    };
    assert!((env.attack_ms - 10_000.0).abs() < 1e-2);
    assert!((env.decay_ms - 20_000.0).abs() < 1e-2);
    assert!((env.release_ms - 20_000.0).abs() < 1e-2);
    assert!(rack.validate().is_ok());
    assert!((rack.read_param_normalized(attack_p, 1000).unwrap() - 1.0).abs() < 1e-5);
    assert!((rack.read_param_normalized(decay_p, 1000).unwrap() - 1.0).abs() < 1e-5);
    assert!((rack.read_param_normalized(release_p, 1000).unwrap() - 1.0).abs() < 1e-5);

    // Step never persists compiler provenance and rejects it on deserialize
    let mut step = Step {
        active: true,
        source_part: Some(42),
        ..Step::default()
    };
    let json = serde_json::to_string(&step).unwrap();
    assert!(!json.contains("source_part"));
    step.source_part = None;
    let parsed: Step = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, step);
    let bad_json = r#"{"active":true,"degree":1,"velocity":0.8,"gate":0.75,"source_part":42}"#;
    assert!(serde_json::from_str::<Step>(bad_json).is_err());

    // Held parameter nudges (Press + Repeat...) coalesce into ONE undo step
    let dir = tempfile::tempdir().unwrap();
    let mut app = hermetic_app(Project::demo(), dir.path().join("demo.json"));
    let inst_id = app.project().instruments[0].id;
    app.apply(Action::SelectInstrument {
        instrument: inst_id,
    })
    .unwrap();
    app.press_keys("gi").unwrap();
    let initial_rack = app.project().instrument(inst_id).unwrap().rack.clone();
    // Move to Cutoff on Quick Panel (index 1: Wave, Cutoff, Res, ...)
    app.execute_command("tab panel").unwrap();
    app.control_cursor = 1;
    let press = KeyEvent::new_with_kind(KeyCode::Char('l'), KeyModifiers::ALT, KeyEventKind::Press);
    let repeat =
        KeyEvent::new_with_kind(KeyCode::Char('l'), KeyModifiers::ALT, KeyEventKind::Repeat);
    app.handle_key(press).unwrap();
    for _ in 0..5 {
        app.handle_key(repeat).unwrap();
    }
    let cutoff_param = InstrumentParam::parse_for_rack("cutoff", &initial_rack, false).unwrap();
    let before_cutoff = initial_rack.read_param_native(cutoff_param).unwrap();
    let after_cutoff = app
        .project()
        .instrument(inst_id)
        .unwrap()
        .rack
        .read_param_native(cutoff_param)
        .unwrap();
    assert!(after_cutoff > before_cutoff * 1.4);
    // One single undo restores exact pre-nudge cutoff
    app.apply(Action::Undo).unwrap();
    assert_eq!(
        app.project().instrument(inst_id).unwrap().rack,
        initial_rack
    );
}

#[test]
fn keys_in_graph_take_midi_and_macros_never_leak_into_arrange_pattern_or_mixer() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = hermetic_app(Project::demo(), dir.path().join("demo.json"));
    let total_placements =
        |app: &App| -> usize { app.project().lanes.iter().map(|l| l.placements.len()).sum() };

    app.press_keys("ga").unwrap();
    assert!(app.selected_placement.is_some());
    let initial_placements = total_placements(&app);
    let initial_rev = app.revision();
    let initial_flags = app.flags().clone();

    // 1. Signal Graph (`gG`): unhandled keys must not delete an Arrange placement.
    app.press_keys("gG").unwrap();
    assert!(app.graph_focused());
    app.press_keys("x=HL[]'m<Esc>s<Esc>").unwrap();
    assert_eq!(
        total_placements(&app),
        initial_placements,
        "`x` in Signal Graph must not delete an Arrange placement"
    );
    assert_eq!(app.revision(), initial_rev);
    assert_eq!(app.flags(), &initial_flags);

    // 2. Take Deck (`gt`): `x` clears marks only, never Arrange placements.
    app.press_keys("gt").unwrap();
    assert!(app.take_focused());
    app.press_keys("xm=HL").unwrap();
    assert_eq!(total_placements(&app), initial_placements);
    assert_eq!(app.revision(), initial_rev);
    assert_eq!(app.flags(), &initial_flags);

    // 3. Devices · MIDI Deck (`gd`): unhandled keys never mutate Arrange/Pattern/Mixer.
    app.press_keys("gd").unwrap();
    assert!(app.midi_focused());
    app.press_keys("xms=HL").unwrap();
    assert_eq!(total_placements(&app), initial_placements);
    assert_eq!(app.revision(), initial_rev);
    assert_eq!(app.flags(), &initial_flags);

    // 4. Macros Deck (`gk`): unhandled keys on unconfigured slot never mutate Arrange.
    app.press_keys("gk").unwrap();
    assert!(app.macros_focused());
    app.press_keys("xms[]").unwrap();
    assert_eq!(total_placements(&app), initial_placements);
    assert_eq!(app.revision(), initial_rev);
    assert_eq!(app.flags(), &initial_flags);
}

#[test]
fn ctrl_r_redo_is_blocked_in_modal_overlays_and_repeat_nudges_coalesce() {
    use cadence_app::ControlTarget;
    use cadence_core::{params::ParamAddress, rack::ModuleParam, sampler::SampleLoopMode};

    let dir = tempfile::tempdir().unwrap();
    let mut app = hermetic_app(Project::demo(), dir.path().join("demo.json"));

    // 1. Ctrl-r does not fire Redo while inside Command/Palette or Help overlays.
    app.execute_command("bpm 140").unwrap();
    app.apply(Action::Undo).unwrap();
    assert_eq!(app.project().bpm, 136.0);
    app.press_keys(":").unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL))
        .unwrap();
    assert_eq!(
        app.project().bpm,
        136.0,
        "Ctrl-r must not fire Redo while in Palette/Command mode"
    );
    app.press_keys("<Esc>").unwrap();

    app.press_keys("?").unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL))
        .unwrap();
    assert_eq!(
        app.project().bpm,
        136.0,
        "Ctrl-r must not fire Redo while in Help mode"
    );
    app.press_keys("<Esc>").unwrap();

    // 2. Coalesced undo for PartTranspose, PartVelocity, and ModuleParam::LoopMode.
    let part_id = app.project().parts[0].id;
    let part_name = app.part(part_id).unwrap().name.clone();
    let init_tr = app.part(part_id).unwrap().transpose;
    app.execute_command(&format!("part {part_name}")).unwrap();
    let tr_row = app
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::PartTranspose(_)))
        .unwrap();
    app.control_cursor = tr_row;
    app.handle_key(KeyEvent::new_with_kind(
        KeyCode::Char('l'),
        KeyModifiers::ALT,
        KeyEventKind::Press,
    ))
    .unwrap();
    for _ in 0..3 {
        app.handle_key(KeyEvent::new_with_kind(
            KeyCode::Char('l'),
            KeyModifiers::ALT,
            KeyEventKind::Repeat,
        ))
        .unwrap();
    }
    assert_eq!(app.part(part_id).unwrap().transpose, init_tr + 4);
    app.apply(Action::Undo).unwrap();
    assert_eq!(
        app.part(part_id).unwrap().transpose,
        init_tr,
        "4 repeat PartTranspose nudges must coalesce into 1 undo step"
    );

    let init_vel = app.part(part_id).unwrap().velocity_scale;
    let vel_row = app
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::PartVelocity(_)))
        .unwrap();
    app.control_cursor = vel_row;
    app.handle_key(KeyEvent::new_with_kind(
        KeyCode::Char('h'),
        KeyModifiers::ALT,
        KeyEventKind::Press,
    ))
    .unwrap();
    for _ in 0..3 {
        app.handle_key(KeyEvent::new_with_kind(
            KeyCode::Char('h'),
            KeyModifiers::ALT,
            KeyEventKind::Repeat,
        ))
        .unwrap();
    }
    assert!(app.part(part_id).unwrap().velocity_scale < init_vel - 0.02);
    app.apply(Action::Undo).unwrap();
    assert!((app.part(part_id).unwrap().velocity_scale - init_vel).abs() < 1e-4);

    app.execute_command("instrument LoopSamp sampler").unwrap();
    let samp_id = app.selected_instrument.unwrap();
    app.press_keys("l").unwrap();
    let loop_row = app
        .control_descriptors()
        .iter()
        .position(|c| {
            matches!(
                c.target,
                ControlTarget::Param(ParamAddress::Module {
                    param: ModuleParam::LoopMode,
                    ..
                })
            )
        })
        .unwrap();
    app.control_cursor = loop_row;
    app.handle_key(KeyEvent::new_with_kind(
        KeyCode::Char('l'),
        KeyModifiers::ALT,
        KeyEventKind::Press,
    ))
    .unwrap();
    app.handle_key(KeyEvent::new_with_kind(
        KeyCode::Char('l'),
        KeyModifiers::ALT,
        KeyEventKind::Repeat,
    ))
    .unwrap();
    let loop_mode_of = |app: &App| {
        app.instrument(samp_id)
            .unwrap()
            .rack
            .sampler_modules()
            .next()
            .unwrap()
            .3
            .loop_mode
    };
    assert_eq!(loop_mode_of(&app), SampleLoopMode::PingPong);
    app.apply(Action::Undo).unwrap();
    assert_eq!(
        loop_mode_of(&app),
        SampleLoopMode::Off,
        "2 repeat LoopMode nudges must coalesce into 1 undo step"
    );
}
