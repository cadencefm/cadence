mod common;

use cadence_app::{load_project, Action, App};
use cadence_core::{material::Project, rack::module_sample_key, sampler::SampleLibrary};
use std::{fs, path::Path, process::Command};

fn sampled_project(path: &str) -> Project {
    let mut app = App::new_with_library(
        Project::empty(),
        "unused.json".into(),
        SampleLibrary::from_roots_only(&[], None),
    )
    .unwrap();
    app.execute_command("part new Tone sampler 0 . . .")
        .unwrap();
    let id = app.project().instruments[0].id;
    let mut rack = app.project().instruments[0].rack.clone();
    for (_, _, smp) in rack.sampler_modules_mut() {
        smp.path = Some(path.into());
    }
    app.apply(Action::SetRack {
        instrument: id,
        rack,
    })
    .unwrap();
    app.project().clone()
}
fn save(project: &Project, path: &Path) {
    fs::write(path, serde_json::to_vec_pretty(project).unwrap()).unwrap();
}
fn tone(path: &Path) {
    common::write_sine_wav(path, 1, 8_000, 800, 220.0, 0.61);
}
fn check(path: &Path, cwd: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_cadence"))
        .current_dir(cwd)
        .arg("--project")
        .arg(path)
        .arg("--check")
        .output()
        .unwrap()
}

#[test]
fn missing_required_asset_reports_readiness_and_preserves_export_destination() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("project.json");
    save(&sampled_project("./samples/cyber-hat.wav"), &path);
    let checked = check(&path, folder.path());
    assert!(!checked.status.success());
    let summary: serde_json::Value = serde_json::from_slice(&checked.stdout).unwrap();
    assert_eq!(summary["valid"], true);
    assert_eq!(summary["ready"], false);
    assert_eq!(
        summary["sample_issues"][0]["path"],
        "./samples/cyber-hat.wav"
    );
    let output = folder.path().join("take.wav");
    fs::write(&output, b"keep this existing take").unwrap();
    let rendered = Command::new(env!("CARGO_BIN_EXE_cadence"))
        .arg("--project")
        .arg(&path)
        .arg("--render")
        .arg(&output)
        .args(["--bars", "1"])
        .output()
        .unwrap();
    assert!(!rendered.status.success());
    assert!(String::from_utf8_lossy(&rendered.stderr).contains("cyber-hat.wav"));
    assert_eq!(fs::read(output).unwrap(), b"keep this existing take");
}

#[test]
fn portable_relative_assets_do_not_fall_back_to_a_different_library_file() {
    let folder = tempfile::tempdir().unwrap();
    let dir = folder.path().join("moved");
    fs::create_dir(&dir).unwrap();
    let path = dir.join("project.json");
    save(&sampled_project("./tone.wav"), &path);
    tone(&dir.join("tone.wav"));
    assert!(check(&path, folder.path()).status.success());
    fs::remove_file(dir.join("tone.wav")).unwrap();
    assert!(!check(&path, folder.path()).status.success());
}

#[test]
fn load_undo_redo_and_save_as_keep_the_correct_asset_origin() {
    let folder = tempfile::tempdir().unwrap();
    let base = folder.path().canonicalize().unwrap();
    let first = base.join("first");
    let second = base.join("second");
    let saved = base.join("saved");
    for dir in [&first, &second, &saved] {
        fs::create_dir(dir).unwrap();
    }
    for dir in [&first, &second] {
        tone(&dir.join("tone.wav"));
        save(&sampled_project("./tone.wav"), &dir.join("project.json"));
    }
    let library = SampleLibrary::from_roots(&[], Some(&first.join("project.json")));
    let mut app = App::new_with_library(
        sampled_project("./tone.wav"),
        first.join("project.json"),
        library,
    )
    .unwrap();
    let id = app.project().instruments[0].id;
    let sample_key = module_sample_key(id, 1, 1);
    app.ensure_samples_ready().unwrap();
    app.apply(Action::Load {
        path: second.join("project.json"),
    })
    .unwrap();
    assert_eq!(
        app.instrument_sample(sample_key)
            .unwrap()
            .resolved_path
            .canonicalize()
            .unwrap(),
        second.join("tone.wav")
    );
    assert_eq!(app.revision(), 1);
    app.apply(Action::Undo).unwrap();
    assert_eq!(
        app.instrument_sample(sample_key)
            .unwrap()
            .resolved_path
            .canonicalize()
            .unwrap(),
        first.join("tone.wav")
    );
    assert!(app.dirty());
    app.apply(Action::Redo).unwrap();
    assert_eq!(
        app.instrument_sample(sample_key)
            .unwrap()
            .resolved_path
            .canonicalize()
            .unwrap(),
        second.join("tone.wav")
    );
    assert!(!app.dirty());
    let before = app.project().clone();
    app.save_project(&saved.join("project.json")).unwrap();
    assert_eq!(app.project(), &before);
    assert!(check(&saved.join("project.json"), &base).status.success());
    assert!(!saved.join("tone.wav").exists());
    assert!(load_project(&saved.join("project.json"))
        .unwrap()
        .compile()
        .is_ok());
}

#[test]
fn unused_missing_instruments_do_not_block_render_but_scheduling_them_does() {
    let folder = tempfile::tempdir().unwrap();
    let mut project = sampled_project("./absent.wav");
    let placement = project.lanes[0].placements.remove(0);
    let mut app = App::new(project, folder.path().join("project.json")).unwrap();
    assert_eq!(app.sample_issues().len(), 1);
    assert!(app.ready());
    let lane = app.project().lanes[0].id;
    app.apply(Action::Edit {
        edit: cadence_core::material::Edit::PutPlacement { lane, placement },
    })
    .unwrap();
    assert!(!app.ready());
    app.apply(Action::Undo).unwrap();
    assert!(app.ready());
}

#[test]
fn explicit_reopen_reloads_repaired_or_corrupt_sample_data() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("project.json");
    let sample = folder.path().join("tone.wav");
    let project = sampled_project("./tone.wav");
    save(&project, &path);
    tone(&sample);
    let mut app = App::new(project, path.clone()).unwrap();
    app.ensure_samples_ready().unwrap();
    fs::write(&sample, b"corrupt replacement").unwrap();
    app.apply(Action::Load { path: path.clone() }).unwrap();
    assert!(!app.ready());
    tone(&sample);
    app.apply(Action::Load { path }).unwrap();
    assert!(app.ready());
    let id = app.project().instruments[0].id;
    let mut rack = app.project().instruments[0].rack.clone();
    for (_, _, smp) in rack.sampler_modules_mut() {
        smp.path = Some("./missing.wav".into());
    }
    app.apply(Action::SetRack {
        instrument: id,
        rack,
    })
    .unwrap();
    assert!(!app.ready());
    app.apply(Action::Undo).unwrap();
    assert!(app.ready());
}

#[test]
fn hot_path_parameter_edits_skip_sample_resync_and_save_as_relativizes_rack_zone_paths() {
    use cadence_app::{KeyCode, KeyEvent, KeyModifiers};

    // 1. Hot-path knob & fader edits do not trigger SampleBank resync.
    let mut app = App::new(Project::demo(), "demo.json".into()).unwrap();
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
    app.press_keys("i").unwrap();
    let out_synth = app
        .handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE))
        .unwrap();
    assert!(!out_synth.samples_changed);
    app.press_keys("<Esc>gmi").unwrap();
    let out_fader = app
        .handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE))
        .unwrap();
    assert!(!out_fader.samples_changed);
    app.press_keys("<Esc>").unwrap();

    // 2. Save As across directories re-relativizes Rack zone Sampler paths cleanly.
    let dir = tempfile::tempdir().unwrap();
    let samples_dir = dir.path().join("samples");
    let sub_a = dir.path().join("sub_a");
    let sub_b = dir.path().join("sub_b").join("nested");
    fs::create_dir_all(&samples_dir).unwrap();
    fs::create_dir_all(&sub_a).unwrap();
    fs::create_dir_all(&sub_b).unwrap();
    tone(&samples_dir.join("snare.wav"));

    let mut save_app = App::new(Project::demo(), sub_a.join("orig.json")).unwrap();
    save_app.execute_command("kit KitRel tr808").unwrap();
    save_app.execute_command("instrument KitRel").unwrap();
    save_app
        .execute_command("zone sample ../samples/snare.wav")
        .unwrap();
    let save_path = sub_b.join("saved_kit.json");
    save_app
        .execute_command(&format!("w \"{}\"", save_path.display()))
        .unwrap();
    let raw_json = fs::read_to_string(&save_path).unwrap();
    assert!(
        raw_json.contains("../../samples/snare.wav"),
        "Rack zone Sampler path must re-relativize on Save As across dirs, got json snippet: {}",
        raw_json
            .lines()
            .filter(|l| l.contains("snare.wav"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
