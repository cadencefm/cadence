use cadence_app::{authoring::DraftField, Action, App};
use cadence_core::{
    material::{PatternKind, PhaseMode, Project},
    sampler::SampleLibrary,
};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
fn app() -> App {
    App::new_with_library(
        Project::empty(),
        "unused.json".into(),
        cadence_core::sampler::SampleLibrary::from_roots_only(&[], None),
    )
    .unwrap()
}
fn finish(app: &mut App) {
    for _ in 0..40 {
        if !app.is_authoring() {
            return;
        }
        let view = app.authoring_view().unwrap();
        assert!(
            view.error.is_none(),
            "{}: {:?}",
            view.field_label,
            view.error
        );
        app.press_keys("<Enter>").unwrap();
    }
    panic!("workflow did not finish: {:?}", app.authoring_view());
}
fn reach(app: &mut App, field: DraftField) {
    for _ in 0..40 {
        let view = app.authoring_view().expect("workflow remains open");
        assert!(view.error.is_none(), "{view:?}");
        if view.field == field {
            return;
        }
        app.press_keys("<Enter>").unwrap();
    }
    panic!("field not reached");
}
fn wait_browser(app: &mut App) -> Vec<cadence_app::Outcome> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut outcomes = Vec::new();
    loop {
        outcomes.push(app.poll_browser_updates().unwrap());
        if !app.browser_busy() {
            return outcomes;
        }
        assert!(Instant::now() < deadline, "browser decode deadline");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn wav(path: &Path, frames: usize) {
    let mut out = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate: 8000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for i in 0..frames {
        out.write_sample(
            ((i as f64 * 220.0 * std::f64::consts::TAU / 8000.0).sin() * 16000.0) as i16,
        )
        .unwrap();
    }
    out.finalize().unwrap();
}
fn library_app(root: &Path, roots: &[PathBuf]) -> App {
    let path = root.join("project.json");
    let library = SampleLibrary::from_roots(roots, Some(&path));
    App::new_with_library(Project::empty(), path, library).unwrap()
}

#[test]
fn separate_modal_workflows_reuse_material_and_build_an_explicit_placement() {
    let mut a = app();
    a.execute_command("bars 4").unwrap();
    a.press_keys("gpnnMotif<Enter>5<Enter>i13587<Esc>ndHarmony<Enter>64<Enter>24l<Enter>-2<Enter>8l<Enter>2<Enter>24l<Enter>-1<Enter>dMoving<Enter>a<Enter><Enter>").unwrap();
    let moving = a.selected_pattern.unwrap();
    let sources = a.project().patterns.clone();
    let revision = a.revision();
    a.press_keys("niLead<Enter>s/pulse<Enter>").unwrap();
    assert_eq!(a.project().instruments.len(), 0);
    assert_eq!(a.revision(), revision);
    finish(&mut a);
    assert_eq!(a.project().instruments.len(), 1);
    assert_eq!(a.project().mixer.channels.len(), 1);
    assert_eq!(a.revision(), revision + 1);
    a.press_keys("nbLeadPart<Enter><Enter><Enter><Enter><Enter>")
        .unwrap();
    let part = a.project().parts[0].id;
    assert_eq!(a.project().parts[0].pattern, moving);
    a.press_keys("nlMain<Enter>").unwrap();
    a.press_keys("no<Enter><Enter>2<Enter>2<Enter>r-3<Enter>")
        .unwrap();
    let placement = &a.project().lanes[0].placements[0];
    assert_eq!(placement.part, part);
    assert_eq!(placement.start_step, 16);
    assert_eq!(placement.length_steps, 32);
    assert_eq!(placement.phase, PhaseMode::Restart);
    assert_eq!(placement.offset_steps, -3);
    assert_eq!(a.project().patterns, sources);
    assert!(a.project().compile().is_ok());
}

#[test]
fn playable_stages_the_complete_bundle_then_commits_one_undo() {
    let mut a = app();
    let original = a.project().clone();
    a.press_keys("npNeon<Enter>").unwrap();
    for _ in 0..3 {
        a.press_keys("<Enter>").unwrap();
        assert_eq!(a.project(), &original);
        assert_eq!(a.revision(), 0);
    }
    finish(&mut a);
    assert_eq!(
        (
            a.project().patterns.len(),
            a.project().instruments.len(),
            a.project().mixer.channels.len(),
            a.project().parts.len(),
            a.project().lanes.len()
        ),
        (1, 1, 1, 1, 1)
    );
    assert_eq!(a.project().lanes[0].placements.len(), 1);
    assert_eq!(a.revision(), 1);
    assert_eq!(a.mode_name(), "STEP INSERT");
    assert!(
        matches!(&a.project().patterns[0].kind,PatternKind::Notes{steps} if steps.len()==16&&steps.iter().all(|s|!s.active))
    );
    a.press_keys("<Esc>u").unwrap();
    assert_eq!(a.project(), &original);
    a.press_keys("U").unwrap();
    assert_eq!(a.project().instruments.len(), 1);
}

#[test]
fn nested_cancel_and_overlap_rejection_leave_music_and_navigation_intact() {
    let mut a = app();
    a.execute_command("part new Existing init 0 2 4 6 7 . . .")
        .unwrap();
    a.execute_command("open 'Existing motif'").unwrap();
    a.cursor = 7;
    let before = a.project().clone();
    let revision = a.revision();
    a.press_keys("npOther<Enter><Enter>n").unwrap();
    assert!(a.is_authoring());
    a.press_keys("<Esc>").unwrap();
    assert_eq!(a.project(), &before);
    assert_eq!(a.revision(), revision);
    assert_eq!(a.cursor, 7);
    a.execute_command("place").unwrap();
    reach(&mut a, DraftField::Offset);
    a.press_keys("<Enter>").unwrap();
    assert!(a.authoring_view().unwrap().error.is_some());
    assert_eq!(a.project(), &before);
    assert_eq!(a.revision(), revision);
    a.press_keys("<Esc>").unwrap();
    assert_eq!(a.cursor, 7);
}

#[test]
fn nil_argument_commands_open_flows_and_new_menu_works_in_every_workspace() {
    for command in ["instrument", "part", "lane", "place", "part new"] {
        let mut a = app();
        a.execute_command(command).unwrap();
        assert!(a.is_authoring());
        a.press_keys("<Esc>").unwrap();
        assert_eq!(a.revision(), 0);
    }
    let mut a = app();
    for workspace in ["ga", "gp", "gm"] {
        a.press_keys(&format!("{workspace}n")).unwrap();
        let view = a.authoring_view().unwrap();
        assert!(view.candidates.iter().any(|c| c.key == Some('i')));
        assert!(view.candidates.iter().any(|c| c.key == Some('d')));
        a.press_keys("<Esc>").unwrap();
    }
    a.press_keys("ga<Enter>").unwrap();
    assert!(a.is_authoring());
}

#[test]
fn accepted_workflow_stops_audition_but_cancel_does_not() {
    let mut a = app();
    a.press_keys("gpnnCell<Enter>5<Enter>i13587<Esc>:audition 5<Enter>")
        .unwrap();
    a.press_keys("niDraft<Enter><Esc>").unwrap();
    assert!(a.is_auditioning());
    a.press_keys("niLead<Enter>").unwrap();
    let mut last = None;
    for _ in 0..24 {
        if !a.is_authoring() {
            break;
        }
        last = a.press_keys("<Enter>").unwrap().pop();
    }
    let out = last.unwrap();
    assert!(out.changed);
    assert!(!a.is_auditioning());
    assert!(out.stop_audition);
    assert_eq!(a.project().instruments.len(), 1);
}

#[test]
fn sample_browser_assigns_exact_duplicate_alias_only_on_accept_and_undo_restores_readiness() {
    let folder = tempfile::tempdir().unwrap();
    let first = folder.path().join("a");
    let second = folder.path().join("b");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    wav(&first.join("tone.wav"), 800);
    wav(&second.join("tone.wav"), 1600);
    let mut a = library_app(folder.path(), &[first, second.clone()]);
    a.execute_command("sampler Voice ./missing.wav").unwrap();
    let id = a.project().instruments[0].id;
    a.execute_command("pattern notes Trigger 0").unwrap();
    a.execute_command("part VoicePart Trigger Voice").unwrap();
    a.execute_command("lane Main").unwrap();
    a.execute_command("place VoicePart Main 1 1 song").unwrap();
    a.execute_command("instrument Voice").unwrap();
    let before = a.project().clone();
    let revision = a.revision();
    assert!(!a.ready());
    a.press_keys("Bj").unwrap();
    assert!(a.browser_view().unwrap().sample.is_none());
    wait_browser(&mut a);
    let view = a.browser_view().unwrap();
    assert_eq!(view.sample.unwrap().frames, 1600);
    let audition = a.press_keys("A").unwrap().pop().unwrap();
    assert_eq!(audition.preview_sample.unwrap().sample.frames, 1600);
    assert_eq!(a.project(), &before);
    let cancel = a.press_keys("<Esc>").unwrap().pop().unwrap();
    assert!(cancel.stop_audition);
    assert_eq!(a.project(), &before);
    assert_eq!(a.revision(), revision);
    a.press_keys("B<Enter>").unwrap();
    wait_browser(&mut a);
    assert!(!a.is_sample_browser());
    assert_eq!(a.project().instruments[0].id, id);
    assert!(a.ready());
    let (_, _, _, patch) = a.project().instruments[0]
        .rack
        .sampler_modules()
        .next()
        .expect("expected Sampler module");
    assert_eq!(patch.path.as_deref(), Some("./b/tone.wav"));
    assert_eq!(patch.sample_end, 1600);
    a.apply(Action::Undo).unwrap();
    assert_eq!(a.project(), &before);
    assert!(!a.ready());
}

#[test]
fn browser_decode_and_home_never_publish_late_auditions_or_stale_selections() {
    let folder = tempfile::tempdir().unwrap();
    wav(&folder.path().join("a.wav"), 800);
    wav(&folder.path().join("b.wav"), 1600);
    let mut a = library_app(folder.path(), &[folder.path().to_path_buf()]);
    let before = a.project().clone();
    a.press_keys("Bp<Home>j").unwrap();
    for out in wait_browser(&mut a) {
        assert!(out.preview_sample.is_none());
    }
    assert_eq!(a.browser_view().unwrap().sample.unwrap().frames, 1600);
    assert_eq!(a.project(), &before);
    let out = a.press_keys("A").unwrap().pop().unwrap();
    assert_eq!(out.preview_sample.unwrap().sample.frames, 1600);
    a.press_keys("<Esc>").unwrap();
    for _ in 0..3 {
        assert!(a.poll_browser_updates().unwrap().preview_sample.is_none());
    }
    assert_eq!(a.project(), &before);
    assert_eq!(a.revision(), 0);
}

#[test]
fn browser_new_sampler_is_staged_after_choice_and_failure_is_recoverable() {
    let folder = tempfile::tempdir().unwrap();
    wav(&folder.path().join("tone.wav"), 800);
    let mut a = library_app(folder.path(), &[folder.path().to_path_buf()]);
    a.press_keys("B<Enter>").unwrap();
    assert_eq!(a.project().instruments.len(), 0);
    wait_browser(&mut a);
    assert!(a.is_authoring());
    assert!(a.project().instruments.is_empty());
    a.press_keys("<Esc>").unwrap();
    assert_eq!(a.revision(), 0);
    a.press_keys("B<Enter>").unwrap();
    wait_browser(&mut a);
    finish(&mut a);
    assert_eq!(a.project().instruments.len(), 1);
    assert_eq!(a.revision(), 1);
    assert!(a.project().instruments[0].rack.has_samplers());
    let before = a.project().clone();
    std::fs::write(folder.path().join("tone.wav"), b"broken sample").unwrap();
    a.press_keys("Br").unwrap();
    wait_browser(&mut a);
    assert!(a.browser_view().unwrap().error.is_some());
    a.press_keys("<Enter>").unwrap();
    assert!(a.is_sample_browser());
    assert_eq!(a.project(), &before);
    wav(&folder.path().join("tone.wav"), 1200);
    a.press_keys("r").unwrap();
    wait_browser(&mut a);
    a.press_keys("<Enter>").unwrap();
    let (_, _, _, p) = a.project().instruments[0]
        .rack
        .sampler_modules()
        .next()
        .expect("expected Sampler module");
    assert_eq!(p.sample_end, 1200);
}

#[cfg(unix)]
#[test]
fn browser_handles_symlink_identity_and_rejects_a_retargeted_selection() {
    use std::os::unix::fs::symlink;
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().join("library");
    std::fs::create_dir(&root).unwrap();
    let source = folder.path().join("source.wav");
    let other = folder.path().join("other.wav");
    wav(&source, 800);
    wav(&other, 1600);
    let link = root.join("linked.wav");
    symlink(&source, &link).unwrap();
    let mut a = library_app(folder.path(), &[root]);
    a.press_keys("B").unwrap();
    wait_browser(&mut a);
    assert_eq!(a.browser_view().unwrap().sample.unwrap().frames, 800);
    std::fs::remove_file(&link).unwrap();
    symlink(&other, &link).unwrap();
    a.press_keys("<Enter>").unwrap();
    assert!(a.is_sample_browser());
    assert!(a.browser_view().unwrap().error.is_some());
    assert!(a.project().instruments.is_empty());
    a.press_keys("r").unwrap();
    wait_browser(&mut a);
    a.press_keys("<Enter>").unwrap();
    finish(&mut a);
    let (_, _, _, p) = a.project().instruments[0]
        .rack
        .sampler_modules()
        .next()
        .expect("expected Sampler module");
    assert_eq!(p.path.as_deref(), Some("./other.wav"));
    assert_eq!(p.sample_end, 1600);
}

#[test]
fn accepting_reloaded_equal_metadata_publishes_new_bytes_without_an_undo_entry() {
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("tone.wav");
    wav(&file, 800);
    let mut a = library_app(folder.path(), &[folder.path().to_path_buf()]);
    a.press_keys("B<Enter>").unwrap();
    wait_browser(&mut a);
    finish(&mut a);
    let id = a.project().instruments[0].id;
    let sample_key = cadence_core::rack::module_sample_key(id, 1, 1);
    let old = a.instrument_sample(sample_key).unwrap().clone();
    let before = a.project().clone();
    let revision = a.revision();
    let mut writer = hound::WavWriter::create(
        &file,
        hound::WavSpec {
            channels: 1,
            sample_rate: 8000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for _ in 0..800 {
        writer.write_sample(1234_i16).unwrap();
    }
    writer.finalize().unwrap();
    a.press_keys("Br").unwrap();
    wait_browser(&mut a);
    let out = a.press_keys("<Enter>").unwrap().pop().unwrap();
    assert!(!out.changed && out.samples_changed);
    assert_eq!(a.project(), &before);
    assert_eq!(a.revision(), revision);
    let new = a.instrument_sample(sample_key).unwrap();
    assert!(!std::sync::Arc::ptr_eq(&old, new));
    assert_ne!(old.frame_stereo(0), new.frame_stereo(0));
}

#[test]
fn rescan_discovers_new_assets_without_mutating_the_project() {
    let folder = tempfile::tempdir().unwrap();
    wav(&folder.path().join("a.wav"), 800);
    let mut a = library_app(folder.path(), &[folder.path().to_path_buf()]);
    a.press_keys("B").unwrap();
    wait_browser(&mut a);
    assert_eq!(a.browser_view().unwrap().total, 1);
    let before = a.project().clone();
    wav(&folder.path().join("b.wav"), 1600);
    a.press_keys("R").unwrap();
    wait_browser(&mut a);
    assert_eq!(a.browser_view().unwrap().total, 2);
    assert_eq!(a.project(), &before);
    assert_eq!(a.revision(), 0);
}

#[test]
fn action_new_honors_user_config_defaults_clears_overlays_and_supports_kit_placed_part() {
    use cadence_app::view::OverlayView;
    use cadence_app::workflow::SoundChoice;
    use cadence_app::FileHubTab;
    use cadence_core::{
        config::UserConfig, music::Step, rack::RackTemplate, runtime::AudioSnapshot,
    };

    // 1. `Action::New` clears open FileHub overlay and applies `config.default_bpm` / `default_root_midi`.
    let mut a = App::new_with_library(
        Project::demo(),
        "unused.json".into(),
        cadence_core::sampler::SampleLibrary::from_roots_only(&[], None),
    )
    .unwrap();
    a.config = UserConfig {
        default_bpm: 127.0,
        default_root_midi: 62, // D4
        ..UserConfig::default()
    };
    a.open_file_hub(FileHubTab::Quick);
    assert!(matches!(
        a.frame_view(&AudioSnapshot::default(), "silent").overlay,
        Some(OverlayView::FileHub(_))
    ));
    a.apply(Action::New { title: None }).unwrap();
    assert!(
        a.frame_view(&AudioSnapshot::default(), "silent")
            .overlay
            .is_none(),
        "Action::New must clear file_hub overlay"
    );
    assert_eq!(a.project().bpm, 127.0);
    assert_eq!(a.project().root_midi, 62);

    // 2. `Action::PlacedPart` supports Rack kit presets ("tr-808").
    a.apply(Action::PlacedPart {
        name: "EightOhEight".into(),
        sound: SoundChoice::Preset("tr-808".into()),
        notes: vec![Step {
            active: true,
            degree: 0,
            velocity: 0.9,
            gate: 1.0,
            ..Step::default()
        }],
        start_step: 0,
        length_steps: Some(16),
        transpose: None,
        velocity_scale: None,
    })
    .unwrap();
    assert!(a.project().instruments.iter().any(|i| {
        i.name == "EightOhEight sound" && i.rack.zones.len() == 16 && i.rack.has_samplers()
    }));

    // 3. `Action::CreatePart` and `Action::PlacedPart` support optional `transpose` & `velocity_scale`.
    let pat_id = a.project().patterns[0].id;
    let inst_id = a.project().instruments[0].id;
    a.apply(Action::CreatePart {
        name: "TransposedPart".into(),
        pattern: pat_id,
        instrument: inst_id,
        transpose: Some(-12),
        velocity_scale: Some(0.65),
    })
    .unwrap();
    let tp = a
        .project()
        .parts
        .iter()
        .find(|p| p.name == "TransposedPart")
        .unwrap();
    assert_eq!(tp.transpose, -12);
    assert!((tp.velocity_scale - 0.65).abs() < 1e-4);

    // Serde roundtrip with omitted optional fields defaults to 0 / 1.0.
    let json_omitted = serde_json::json!({
        "op": "placed_part",
        "name": "DefaultPlaced",
        "sound": { "template": "synth" },
        "notes": [{
            "active": true,
            "degree": 1,
            "accidental": 0,
            "velocity": 0.8,
            "gate": 1.0,
            "condition": { "turn": 1, "every": 1 }
        }],
        "start_step": 0,
        "length_steps": 16
    });
    let action_omitted: Action = serde_json::from_value(json_omitted).unwrap();
    a.apply(action_omitted).unwrap();
    let dp = a
        .project()
        .parts
        .iter()
        .find(|p| p.name == "DefaultPlaced")
        .unwrap();
    assert_eq!(dp.transpose, 0);
    assert!((dp.velocity_scale - 1.0).abs() < 1e-4);

    // Out-of-range velocity_scale is strictly rejected.
    assert!(a
        .apply(Action::PlacedPart {
            name: "BadScale".into(),
            sound: SoundChoice::Template(RackTemplate::Synth),
            notes: vec![Step {
                active: true,
                degree: 1,
                velocity: 0.9,
                gate: 1.0,
                ..Step::default()
            }],
            start_step: 0,
            length_steps: Some(16),
            transpose: Some(7),
            velocity_scale: Some(1.4),
        })
        .is_err());

    a.apply(Action::PlacedPart {
        name: "ScaledPlaced".into(),
        sound: SoundChoice::Template(RackTemplate::Synth),
        notes: vec![Step {
            active: true,
            degree: 1,
            velocity: 0.9,
            gate: 1.0,
            ..Step::default()
        }],
        start_step: 0,
        length_steps: Some(16),
        transpose: Some(7),
        velocity_scale: Some(0.85),
    })
    .unwrap();
    let sp = a
        .project()
        .parts
        .iter()
        .find(|p| p.name == "ScaledPlaced")
        .unwrap();
    assert_eq!(sp.transpose, 7);
    assert!((sp.velocity_scale - 0.85).abs() < 1e-4);

    // 4. Picker `A` auditions candidate sounds on `Replace::Instrument` (via `r` on Instrument row).
    let mut demo = App::new_in_memory(Project::demo(), "unused.json".into()).unwrap();
    demo.press_keys("ga<Enter>").unwrap();
    let inst_row = demo
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, cadence_app::ControlTarget::Instrument(_)))
        .expect("Instrument control row in Detail::Placement");
    demo.control_cursor = inst_row;
    demo.press_keys("r").unwrap();
    assert!(
        demo.picker_view().is_some_and(|pv| pv.audition),
        "Replace::Instrument picker must enable audition"
    );
    demo.press_keys("A").unwrap();
    assert!(
        demo.is_auditioning(),
        "Pressing 'A' in Replace::Instrument picker must start auditioning candidate sound"
    );
    demo.press_keys("<Esc>").unwrap();
    assert!(!demo.is_auditioning());
}
