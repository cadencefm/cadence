//! Integration tests for File Hub, Settings asset directory management,
//! portable bundles, preset/MIDI JSON routing, bounded previews, and dirty guards.
mod common;

use cadence_app::host_io::{HostIo, MemoryHostIo};
use cadence_app::view::{FileHubView, FrameView, OverlayView};
use cadence_app::{
    parse_key_sequence, Action, App, Detail, FileDialogRequest, FileHubTab, KeyCode, KeyEvent,
    KeyModifiers,
};
use cadence_core::material::Project;
use cadence_core::midi_map::MidiProfile;
use cadence_core::params::InstrumentParam;
use cadence_core::presets::{InstrumentPreset, PresetCatalog};
use cadence_core::rack::{ModuleKind, ModuleParam, RackPatch};
use cadence_core::runtime::AudioSnapshot;
use cadence_core::sampler::SampleLibrary;
use cadence_core::take::TakeJournal;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn file_hub_view(app: &App) -> FileHubView {
    let fv = app.frame_view(&AudioSnapshot::default(), "mem");
    match fv.overlay {
        Some(OverlayView::FileHub(fh)) => fh,
        other => panic!("expected FileHub overlay, got {other:?}"),
    }
}

fn snap_view(app: &App) -> FrameView {
    app.frame_view(&AudioSnapshot::default(), "Test · 48 kHz")
}

fn move_file_hub_cursor(app: &mut App, target_idx: usize) {
    let cur = file_hub_view(app).cursor;
    if target_idx > cur {
        for _ in cur..target_idx {
            app.press_keys("j").unwrap();
        }
    } else if target_idx < cur {
        for _ in target_idx..cur {
            app.press_keys("k").unwrap();
        }
    }
    assert_eq!(file_hub_view(app).cursor, target_idx);
}

fn write_test_wav(path: &Path, frames: usize, sample_rate: u32) {
    common::write_sine_wav(path, 2, sample_rate, frames, 220.0, 0.43);
}

fn hermetic_app(project: Project, save_path: PathBuf, roots: &[PathBuf]) -> App {
    let lib = SampleLibrary::from_roots(roots, Some(&save_path));
    App::new_with_library(project, save_path, lib).unwrap()
}

#[test]
fn filehub_take_bundle_preset_library_open_json_routing_and_reload_presets() {
    let tmp_presets = tempfile::tempdir().unwrap();
    let io = Arc::new(MemoryHostIo::new("/virtual/studio"));

    // Seed a portable Bundle directory (`/virtual/studio/MyBundle` with `project.json` + `samples/`).
    let mut bundled_proj = Project::empty();
    bundled_proj.title = "Portable Bundle Song".into();
    bundled_proj.bpm = 134.0;
    let bundle_bytes = serde_json::to_vec_pretty(&bundled_proj).unwrap();
    io.insert_file("/virtual/studio/MyBundle/project.json", bundle_bytes);
    io.insert_file("/virtual/studio/MyBundle/samples/kick.wav", b"RIFFfake");

    // Seed a standalone InstrumentPreset `.json` (not named `.preset.json`) to verify Open tab routing.
    let mut synth_patch = RackPatch::default_synth();
    synth_patch
        .write_param_native(
            InstrumentParam::Module {
                zone: 1,
                module: 2,
                param: ModuleParam::Cutoff,
            },
            1234.0,
        )
        .unwrap();
    let custom_preset = InstrumentPreset::new("Neon Acid", Some("Lead".into()), synth_patch);
    io.insert_file(
        "/virtual/studio/neon-acid.json",
        custom_preset.to_json_bytes().unwrap(),
    );

    // Seed a MIDI profile `.json` to verify Open tab routes it cleanly too.
    let midi_prof = MidiProfile::minilab3_daw();
    io.insert_file(
        "/virtual/studio/studio-pad.json",
        midi_prof.to_json_bytes().unwrap(),
    );

    // Build an external PresetCatalog from a synthetic temp dir (Synth + Kit Rack) and wire into MemoryHostIo.
    let mut kit = RackPatch::default_kit();
    kit.zones[0].name = "ExtKick".into();
    kit.zones[0].set_midi_bounds(36, 36, Some(36));
    let kit_preset = InstrumentPreset::new("Ext Kit", Some("Kit".into()), kit);
    custom_preset
        .save(&tmp_presets.path().join("neon-acid.preset.json"))
        .unwrap();
    kit_preset
        .save(&tmp_presets.path().join("ext-kit.preset.json"))
        .unwrap();
    io.set_presets(PresetCatalog::from_roots(&[tmp_presets
        .path()
        .to_path_buf()]));

    let mut app = App::new_with_host_io(
        Project::demo(),
        "/virtual/studio/session.json".into(),
        Arc::clone(&io) as Arc<dyn HostIo>,
    )
    .unwrap();
    app.reload_presets();

    // 1. Export tab preview copy describes the 32-bit float mix path accurately.
    app.open_file_hub(FileHubTab::Export);
    let fh_exp = file_hub_view(&app);
    assert!(fh_exp.preview.contains("32-bit float mix"));
    app.press_keys("<Esc>").unwrap();

    // 2. Load a Take via Action::LoadTake (purely over MemoryHostIo) and Save a Take via FileHub Save tab.
    let mut journal =
        TakeJournal::open_with_sink(None, None, app.project(), app.flags(), &[], 0, None).unwrap();
    let mut muted_flags = app.flags().clone();
    muted_flags.muted_lanes.insert(app.project().lanes[0].id);
    journal.record_flags(4, muted_flags, "Mute Lane 1");
    let sealed_take = journal.seal(32).unwrap().expect("sealed take");
    io.insert_file(
        "/virtual/studio/seed.take.jsonl",
        sealed_take.to_jsonl_bytes().unwrap(),
    );
    app.apply(Action::LoadTake {
        path: "/virtual/studio/seed.take.jsonl".into(),
    })
    .unwrap();
    assert!(app.current_take().is_some());

    app.open_file_hub(FileHubTab::Save);
    let save_take_row = file_hub_view(&app)
        .rows
        .iter()
        .position(|r| r.badge == "TAKE")
        .expect("FileHub Save tab must include Save Take row");
    move_file_hub_cursor(&mut app, save_take_row);
    app.press_keys("e").unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
        .unwrap();
    app.press_keys("/virtual/studio/jam-session.take.jsonl<Enter><Enter>")
        .unwrap();
    assert!(
        io.read_file("/virtual/studio/jam-session.take.jsonl")
            .is_some(),
        "FileHub Save Take must persist .take.jsonl through HostIo"
    );
    app.press_keys("<Esc>").unwrap();

    // 3. Quick Open lists `.take.jsonl` cleanly without `FILES` clutter; Browse tab lists
    //    both `.take.jsonl`, `MyBundle` Portable Bundle directory, and local `.json` files.
    app.open_file_hub(FileHubTab::Quick);
    let fh_quick = file_hub_view(&app);
    assert!(
        fh_quick.rows.iter().all(|r| r.section != "FILES"),
        "Quick Open must keep directory browsing separated into Browse"
    );
    assert!(
        fh_quick.rows.iter().any(|r| r.badge == "TAKE"),
        "Quick Open must surface local .take.jsonl live takes"
    );

    app.open_file_hub(FileHubTab::Browse);
    let fh_browse = file_hub_view(&app);
    let take_idx = fh_browse
        .rows
        .iter()
        .position(|r| r.badge == "TAKE")
        .expect("Browse tab must detect .take.jsonl files");
    let bundle_idx = fh_browse
        .rows
        .iter()
        .position(|r| r.badge == "BUNDLE" && r.label.contains("MyBundle"))
        .expect("Browse tab must detect portable Bundle directories");
    let preset_json_idx = fh_browse
        .rows
        .iter()
        .position(|r| r.label.contains("neon-acid.json"))
        .expect("Browse tab lists .json files");

    move_file_hub_cursor(&mut app, bundle_idx);
    assert!(file_hub_view(&app).preview.contains("Portable Bundle"));

    // Selecting a preset `.json` in Browse tab loads it onto the active instrument.
    let orig_title = app.project().title.clone();
    move_file_hub_cursor(&mut app, preset_json_idx);
    app.press_keys("<Enter>").unwrap();
    assert_eq!(
        app.project().title,
        orig_title,
        "Opening a preset .json must not replace the active project"
    );
    let active_inst = app.instrument(app.selected_instrument.unwrap()).unwrap();
    let loaded_cutoff = active_inst
        .rack
        .read_param_native(InstrumentParam::Module {
            zone: 1,
            module: 2,
            param: ModuleParam::Cutoff,
        })
        .expect("Filter cutoff on loaded Rack preset");
    assert!((loaded_cutoff - 1234.0).abs() < 1.0);

    // Selecting `.take.jsonl` in Browse tab loads it into the Take deck (`gt`) via HostIo.
    app.open_file_hub(FileHubTab::Browse);
    move_file_hub_cursor(&mut app, take_idx);
    app.press_keys("<Enter>").unwrap();
    assert!(
        app.take_focused(),
        "Loading a take from FileHub opens Take deck"
    );
    assert_eq!(app.current_take().unwrap().events.len(), 1);

    // Selecting the Portable Bundle in Browse tab loads `MyBundle/project.json`.
    app.apply(Action::Undo).unwrap();
    assert!(!app.dirty());
    app.open_file_hub(FileHubTab::Browse);
    let bundle_idx = file_hub_view(&app)
        .rows
        .iter()
        .position(|r| r.badge == "BUNDLE" && r.label.contains("MyBundle"))
        .unwrap();
    move_file_hub_cursor(&mut app, bundle_idx);
    app.press_keys("<Enter>").unwrap();
    assert_eq!(app.project().title, "Portable Bundle Song");
    assert_eq!(app.project().bpm, 134.0);

    // Create a Kit Rack instrument so PRESET LIBRARY direct load targets it cleanly.
    app.execute_command("instrument TargetKit kit").unwrap();
    let kit_inst_id = app.selected_instrument.unwrap();

    // 4. Import tab surfaces `PRESET LIBRARY` entries, loads them directly, and `R` reloads.
    app.open_file_hub(FileHubTab::Import);
    let fh_imp = file_hub_view(&app);
    assert!(
        fh_imp
            .rows
            .iter()
            .any(|r| r.section == "PRESET LIBRARY" && r.label == "Ext Kit"),
        "Import tab must list external PRESET LIBRARY entries"
    );

    InstrumentPreset::new(
        "Fresh Bass",
        Some("Bass".into()),
        RackPatch::default_synth(),
    )
    .save(&tmp_presets.path().join("fresh-bass.preset.json"))
    .unwrap();
    io.set_presets(PresetCatalog::from_roots(&[tmp_presets
        .path()
        .to_path_buf()]));
    app.press_keys("R").unwrap();
    assert!(
        file_hub_view(&app)
            .rows
            .iter()
            .any(|r| r.section == "PRESET LIBRARY" && r.label == "Fresh Bass"),
        "Pressing R in FileHub Import tab must reload external PresetCatalog"
    );

    let kit_idx = file_hub_view(&app)
        .rows
        .iter()
        .position(|r| r.section == "PRESET LIBRARY" && r.label == "Ext Kit")
        .unwrap();
    move_file_hub_cursor(&mut app, kit_idx);
    app.press_keys("<Enter>").unwrap();
    let loaded_kit = &app.instrument(kit_inst_id).unwrap().rack;
    assert!(loaded_kit.zones.iter().any(|z| z.name == "ExtKick"));

    // 5. `:reload-presets` Ex command reports preset count in status.
    app.execute_command("reload-presets").unwrap();
    assert!(
        app.status.contains("Reloaded 3 Rack presets"),
        "expected preset count in status, got: {}",
        app.status
    );
}

#[test]
fn settings_modal_exposes_and_edits_sample_dirs_and_preset_dirs_with_auto_refresh() {
    let tmp = tempfile::tempdir().unwrap();
    let sample_dir = tmp.path().join("my-samples");
    let preset_dir = tmp.path().join("my-presets");
    std::fs::create_dir_all(&sample_dir).unwrap();
    std::fs::create_dir_all(&preset_dir).unwrap();
    write_test_wav(&sample_dir.join("snare_custom.wav"), 2400, 48_000);
    let ext_preset =
        InstrumentPreset::new("DirPad", Some("Pad".into()), RackPatch::default_synth());
    ext_preset
        .save(&preset_dir.join("dir-pad.preset.json"))
        .unwrap();

    let io = Arc::new(MemoryHostIo::new("/virtual"));
    let mut app = App::new_with_host_io(
        Project::demo(),
        "/virtual/project.json".into(),
        Arc::clone(&io) as Arc<dyn HostIo>,
    )
    .unwrap();
    app.config.sample_dirs.clear();
    app.config.preset_dirs.clear();
    app.settings_ui.config_dirty = false;

    app.press_keys("g,").unwrap();
    let sample_add_row = app
        .frame_view(&AudioSnapshot::default(), "mem")
        .overlay
        .and_then(|o| match o {
            OverlayView::Settings(sv) => sv
                .rows
                .iter()
                .position(|r| r.label.contains("Sample Directories")),
            _ => None,
        })
        .expect("Sample Directories row");
    app.settings_ui.cursor = sample_add_row;
    app.press_keys("agarbage").unwrap();
    let fv_edit = app.frame_view(&AudioSnapshot::default(), "mem");
    let Some(OverlayView::Settings(sv_edit)) = fv_edit.overlay else {
        panic!("expected Settings overlay");
    };
    assert_eq!(
        sv_edit.editing_prompt.as_ref().map(|(_, b)| b.as_str()),
        Some("garbage")
    );
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
        .unwrap();
    let fv_cleared = app.frame_view(&AudioSnapshot::default(), "mem");
    let Some(OverlayView::Settings(sv_cleared)) = fv_cleared.overlay else {
        panic!("expected Settings overlay");
    };
    assert_eq!(
        sv_cleared.editing_prompt.as_ref().map(|(_, b)| b.as_str()),
        Some("")
    );

    let sample_dir_str = sample_dir.display().to_string();
    app.press_keys(&format!("{sample_dir_str}<Enter>")).unwrap();
    assert_eq!(app.config.sample_dirs, vec![sample_dir_str.clone()]);
    assert!(app.settings_ui.config_dirty);

    let preset_add_row = app
        .frame_view(&AudioSnapshot::default(), "mem")
        .overlay
        .and_then(|o| match o {
            OverlayView::Settings(sv) => sv
                .rows
                .iter()
                .position(|r| r.label.contains("Preset Directories")),
            _ => None,
        })
        .expect("Preset Directories row");
    app.settings_ui.cursor = preset_add_row;
    let preset_dir_str = preset_dir.display().to_string();
    app.press_keys(&format!("a{preset_dir_str}<Enter>"))
        .unwrap();
    assert_eq!(app.config.preset_dirs, vec![preset_dir_str.clone()]);

    // Press `s` to save config via `HostIo`.
    app.press_keys("s").unwrap();
    let saved = io
        .saved_config()
        .expect("Settings save must write UserConfig via HostIo");
    assert_eq!(saved.sample_dirs, vec![sample_dir_str.clone()]);
    assert_eq!(saved.preset_dirs, vec![preset_dir_str.clone()]);
    assert!(!app.settings_ui.config_dirty);

    // Removing a directory row via `x` updates config cleanly.
    let sample_entry_row = app
        .frame_view(&AudioSnapshot::default(), "mem")
        .overlay
        .and_then(|o| match o {
            OverlayView::Settings(sv) => sv.rows.iter().position(|r| r.value == sample_dir_str),
            _ => None,
        })
        .expect("configured sample_dir entry row");
    app.settings_ui.cursor = sample_entry_row;
    app.press_keys("x").unwrap();
    assert!(app.config.sample_dirs.is_empty());
    assert!(app.settings_ui.config_dirty);

    // Verify `SampleLibrary::discover_with_config` & `PresetCatalog::discover` scan configured dirs.
    let cfg = cadence_core::config::UserConfig {
        sample_dirs: vec![sample_dir.display().to_string()],
        preset_dirs: vec![preset_dir.display().to_string()],
        ..cadence_core::config::UserConfig::default()
    };
    let lib = SampleLibrary::discover_with_config(&[], None, &cfg);
    assert!(
        lib.visible_entries().any(|(_, e)| e.name == "snare_custom"),
        "SampleLibrary::discover_with_config must scan configured sample_dirs"
    );
    let pres = PresetCatalog::discover(&[], None, &cfg);
    assert!(
        pres.entries().iter().any(|e| e.name == "DirPad"),
        "PresetCatalog::discover must scan configured preset_dirs"
    );
}

#[test]
fn file_hub_auto_closes_on_preset_load_across_open_import_and_finder_and_keeps_open_on_error() {
    let dir = tempfile::tempdir().unwrap();
    let presets_dir = dir.path().join("my-presets");
    std::fs::create_dir_all(&presets_dir).unwrap();

    let mut bright_patch = RackPatch::default_synth();
    if let Some(ModuleKind::Filter(f)) = bright_patch.zones[0].module_mut(2).map(|m| &mut m.kind) {
        f.cutoff_hz = 4_200.0;
    }
    let bright_preset = InstrumentPreset::new("Bright Lead", Some("Leads".into()), bright_patch);
    let bright_path = dir.path().join("bright-lead.preset.json");
    bright_preset.save(&bright_path).unwrap();
    let lib_preset_path = presets_dir.join("bright-lead-lib.preset.json");
    bright_preset.save(&lib_preset_path).unwrap();

    let sampler_preset = InstrumentPreset::new(
        "Clean Sampler",
        Some("Sampler".into()),
        RackPatch::default_sampler(),
    );
    let sampler_path = dir.path().join("clean-sampler.preset.json");
    sampler_preset.save(&sampler_path).unwrap();

    let corrupt_path = dir.path().join("corrupt.preset.json");
    std::fs::write(&corrupt_path, "{ not valid json").unwrap();

    let proj_path = dir.path().join("demo.json");
    let mut app = hermetic_app(Project::demo(), proj_path.clone(), &[]);
    app.config.preset_dirs = vec![presets_dir.display().to_string()];
    app.reload_presets();

    let synth_inst = &app.project().instruments[0];
    let synth_id = synth_inst.id;
    let synth_name = synth_inst.name.clone();
    app.apply(Action::SelectInstrument {
        instrument: synth_id,
    })
    .unwrap();

    // 1. FileHub Quick Open `/` search matches local `.preset.json` and auto-closes cleanly.
    app.open_file_hub(FileHubTab::Quick);
    assert_eq!(app.mode_name(), "FILES");
    app.press_keys("/bright-lead.preset<Enter><Enter>").unwrap();
    assert_eq!(
        app.mode_name(),
        "NORMAL",
        "Quick Open search preset load must auto-close FileHub"
    );
    assert!(snap_view(&app).overlay.is_none());
    assert_eq!(app.detail, Detail::Instrument(synth_id));
    assert!(
        app.deck_focused(),
        "focus must land cleanly on the instrument deck"
    );
    assert_eq!(
        app.status,
        format!("Loaded preset Bright Lead into {synth_name}")
    );
    let Some(ModuleKind::Filter(loaded_flt)) = app.instrument(synth_id).unwrap().rack.zones[0]
        .module(2)
        .map(|m| &m.kind)
    else {
        panic!("expected Filter module 2 in loaded Rack preset");
    };
    assert!((loaded_flt.cutoff_hz - 4_200.0).abs() < 1.0);

    // 2. FileHub Import tab (PRESET LIBRARY entry)
    app.open_file_hub(FileHubTab::Import);
    assert_eq!(app.mode_name(), "FILES");
    app.press_keys("/Bright Lead<Enter><Enter>").unwrap();
    assert_eq!(
        app.mode_name(),
        "NORMAL",
        "Import tab preset load must auto-close FileHub"
    );
    assert!(snap_view(&app).overlay.is_none());
    assert_eq!(app.detail, Detail::Instrument(synth_id));
    assert_eq!(
        app.status,
        format!("Loaded preset Bright Lead into {synth_name}")
    );

    // 3. Native Finder Dialog (`ImportAsset` and `OpenProject`)
    app.open_file_hub(FileHubTab::Import);
    app.apply_file_dialog_result(FileDialogRequest::ImportAsset, bright_path.clone())
        .unwrap();
    assert_eq!(
        app.mode_name(),
        "NORMAL",
        "Finder ImportAsset must auto-close FileHub"
    );
    assert!(snap_view(&app).overlay.is_none());
    assert_eq!(
        app.status,
        format!("Loaded preset Bright Lead into {synth_name}")
    );

    app.open_file_hub(FileHubTab::Quick);
    app.apply_file_dialog_result(FileDialogRequest::OpenProject, bright_path)
        .unwrap();
    assert_eq!(
        app.mode_name(),
        "NORMAL",
        "Finder OpenProject with preset JSON must auto-close FileHub"
    );
    assert!(snap_view(&app).overlay.is_none());
    assert_eq!(
        app.status,
        format!("Loaded preset Bright Lead into {synth_name}")
    );

    // Cross-topology Sampler Rack preset loads cleanly onto any Rack instrument.
    app.open_file_hub(FileHubTab::Import);
    app.apply_file_dialog_result(FileDialogRequest::ImportAsset, sampler_path)
        .unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert!(app.instrument(synth_id).unwrap().rack.has_samplers());

    // 4. Corrupt preset error keeps FileHub open.
    app.apply(Action::Save { path: proj_path }).unwrap();
    assert!(!app.dirty());
    app.open_file_hub(FileHubTab::Quick);
    let err = app.apply_file_dialog_result(FileDialogRequest::ImportAsset, corrupt_path);
    assert!(err.is_err(), "corrupt preset JSON must fail cleanly");
    assert_eq!(
        app.mode_name(),
        "FILES",
        "FileHub must stay open when preset loading errors"
    );
}

#[test]
fn file_hub_preserves_state_on_error_bounds_preview_guards_dirty_and_ctrl_c_closes() {
    let dir = tempfile::tempdir().unwrap();
    let valid_proj = dir.path().join("other.json");
    let big_proj = dir.path().join("huge.json");
    let dir_as_file = dir.path().join("cannot_overwrite_dir");
    std::fs::create_dir_all(&dir_as_file).unwrap();

    let seed_lib = cadence_core::sampler::SampleLibrary::from_roots(&[], Some(&valid_proj));
    let mut seed_app =
        App::new_with_library(Project::demo(), valid_proj.clone(), seed_lib).unwrap();
    seed_app
        .apply(Action::Save {
            path: valid_proj.clone(),
        })
        .unwrap();

    // Create a >8 MiB file to verify bounded preview skips unbounded read cleanly.
    {
        let f = std::fs::File::create(&big_proj).unwrap();
        f.set_len(9 * 1024 * 1024).unwrap();
    }

    let lib = cadence_core::sampler::SampleLibrary::from_roots(&[], Some(&valid_proj));
    let mut app = App::new_with_library(Project::demo(), valid_proj.clone(), lib).unwrap();
    // 1. Bounded preview handles oversized files safely.
    app.open_file_hub(FileHubTab::Browse);
    app.press_keys("/huge.json<Enter>").unwrap();
    let fv = app.frame_view(&AudioSnapshot::default(), "silent");
    match fv.overlay {
        Some(OverlayView::FileHub(hub)) => {
            assert!(
                !hub.preview.is_empty(),
                "Bounded preview should safely summarize oversized file"
            );
        }
        other => panic!("expected FileHub overlay, got {other:?}"),
    }
    // `Ctrl-c` in FileHub closes and clears the overlay cleanly.
    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
        .unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert!(
        app.frame_view(&AudioSnapshot::default(), "silent")
            .overlay
            .is_none(),
        "Ctrl-c must close & clear FileHub overlay"
    );

    // 2. Preserve FileHub tab & cursor on error (trying to Save As onto a directory path fails).
    app.open_file_hub(FileHubTab::Save);
    app.press_keys("je").unwrap();
    let edit_seq = format!("<c-u>{}<Enter><Enter>", dir_as_file.display());
    let mut saw_err = false;
    for key in parse_key_sequence(&edit_seq) {
        if app.handle_key(key).is_err() {
            saw_err = true;
        }
    }
    assert!(saw_err, "Saving over a directory path must error");
    let fv_after_err = app.frame_view(&AudioSnapshot::default(), "silent");
    match fv_after_err.overlay {
        Some(OverlayView::FileHub(hub)) => {
            assert_eq!(hub.active_tab, FileHubTab::Save);
            assert_eq!(
                hub.cursor, 1,
                "FileHub must retain tab & cursor after an error"
            );
        }
        other => panic!("FileHub must stay open on error, got {other:?}"),
    }
    app.press_keys("<Esc>").unwrap();

    // 3. Dirty guard on destructive native FileDialogRequest::OpenProject.
    app.execute_command("bpm 144").unwrap();
    assert!(app.dirty());
    app.apply_file_dialog_result(FileDialogRequest::OpenProject, valid_proj.clone())
        .unwrap();
    let fv_confirm = app.frame_view(&AudioSnapshot::default(), "silent");
    assert!(
        matches!(fv_confirm.overlay, Some(OverlayView::Confirm { .. })),
        "Opening a project while dirty must prompt Confirm instead of silently clobbering"
    );
    assert_eq!(
        app.project().bpm,
        144.0,
        "Dirty project must remain intact until confirmed"
    );

    app.press_keys("<Esc>").unwrap();
    assert_eq!(app.project().bpm, 144.0);
    app.apply_file_dialog_result(FileDialogRequest::OpenProject, valid_proj)
        .unwrap();
    app.press_keys("y").unwrap();
    assert_eq!(app.project().bpm, 136.0);
    assert!(!app.dirty());
}

#[test]
fn file_hub_finder_routing_for_presets_and_midi_profiles() {
    let dir = tempfile::tempdir().unwrap();
    let save_path = dir.path().join("project.json");
    let lib = cadence_core::sampler::SampleLibrary::from_roots(&[], Some(&save_path));
    let mut app = App::new_with_library(Project::demo(), save_path, lib).unwrap();

    app.open_file_hub(FileHubTab::Save);
    app.press_keys("jjj").unwrap(); // row 3: Save Active Instrument Preset
    let dlg3 = app
        .handle_key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT))
        .unwrap();
    assert!(matches!(
        dlg3.file_dialog,
        Some(FileDialogRequest::SaveInstrumentPreset { .. })
    ));
    app.press_keys("j").unwrap(); // row 4: Save Active MIDI Profile
    let dlg4 = app
        .handle_key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT))
        .unwrap();
    assert!(matches!(
        dlg4.file_dialog,
        Some(FileDialogRequest::SaveMidiProfile { .. })
    ));
    let midi_out = dir.path().join("custom.midi.json");
    app.apply_file_dialog_result(
        FileDialogRequest::SaveMidiProfile {
            default_name: "custom.midi.json".into(),
        },
        midi_out.clone(),
    )
    .unwrap();
    assert!(midi_out.is_file());

    // On Import tab regardless of selected row, `F` emits unified `FileDialogRequest::ImportAsset`.
    let dummy_preset = dir.path().join("aa_lead.preset.json");
    InstrumentPreset::new("AA", None, RackPatch::default_synth())
        .save(&dummy_preset)
        .unwrap();
    app.open_file_hub(FileHubTab::Import);
    let dlg_imp_top = app
        .handle_key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT))
        .unwrap();
    assert_eq!(
        dlg_imp_top.file_dialog,
        Some(FileDialogRequest::ImportAsset)
    );
    app.press_keys("j").unwrap();
    let dlg_imp = app
        .handle_key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT))
        .unwrap();
    assert_eq!(dlg_imp.file_dialog, Some(FileDialogRequest::ImportAsset));
    app.press_keys("<Esc>").unwrap();
}

#[test]
fn import_rbs_command_respects_dirty_confirm_guard_and_force_bang() {
    let mut app = App::new_in_memory(Project::demo(), PathBuf::from("demo.json")).unwrap();
    app.execute_command("bpm 138").unwrap();
    assert!(app.dirty());

    let snap = AudioSnapshot::default();
    app.execute_command("import-rbs nonexistent_song.rbs")
        .unwrap();
    let fv = app.frame_view(&snap, "ok");
    assert!(
        matches!(fv.overlay, Some(OverlayView::Confirm { .. })),
        "dirty `:import-rbs` must prompt with Confirm overlay, got {:?}",
        fv.overlay
    );
    app.press_keys("<Esc>").unwrap();
    assert!(app.frame_view(&snap, "ok").overlay.is_none());

    // `:import-rbs!` bypasses Confirm and attempts immediate import (failing on missing file or disabled feature).
    let err = app
        .execute_command("import-rbs! nonexistent_song.rbs")
        .unwrap_err();
    assert!(
        err.to_string().contains("nonexistent_song.rbs")
            || err.to_string().to_ascii_lowercase().contains("import")
            || err
                .to_string()
                .to_ascii_lowercase()
                .contains("no such file")
            || err.to_string().to_ascii_lowercase().contains("rbs"),
        "forced `:import-rbs!` should bypass confirm and run import immediately, got: {err:#}"
    );
}

#[test]
fn settings_clamps_toggles_persistence_and_workflow_folder_dialog() {
    let dir = tempfile::tempdir().unwrap();
    let io = Arc::new(MemoryHostIo::new(dir.path()));
    let mut app = App::new_with_host_io(
        Project::demo(),
        dir.path().join("settings.json"),
        Arc::clone(&io) as Arc<dyn HostIo>,
    )
    .unwrap();
    app.apply_config(cadence_core::config::UserConfig::default());

    // 1. Theme selection from Action Palette persists UserConfig via HostIo.
    app.press_keys(":Theme: Studio Light<Enter>").unwrap();
    assert_eq!(app.config.theme, cadence_core::config::ThemeId::StudioLight);
    let saved_after_palette = io
        .saved_config()
        .expect("Palette SetTheme must persist UserConfig");
    assert_eq!(
        saved_after_palette.theme,
        cadence_core::config::ThemeId::StudioLight
    );

    // 2. Contextual Key Hints toggle hides/shows ChromeView.hints and persists via HostIo.
    assert!(
        !snap_view(&app).chrome.hints.is_empty(),
        "Contextual Key Hints start visible by default"
    );
    app.press_keys(":Toggle Contextual Key Hints<Enter>")
        .unwrap();
    assert!(!app.config.show_key_hints);
    assert!(
        snap_view(&app).chrome.hints.is_empty(),
        "Disabling Contextual Key Hints must hide ChromeView.hints"
    );
    assert!(!io.saved_config().unwrap().show_key_hints);

    // 3. Open Settings (`g,`), verify BPM clamp (20..300), root clamp (24..84),
    //    exact ValueInput on Enter, Default Scale row, and accurate `Ctrl+s` reset hint!
    app.press_keys("g,").unwrap();
    assert_eq!(app.mode_name(), "SETTINGS");

    // Helper to locate a Settings row by label cleanly without hardcoded row indices.
    let find_settings_row = |app: &App, label: &str| -> usize {
        let fv = snap_view(app);
        let Some(cadence_app::view::OverlayView::Settings(sv)) = fv.overlay else {
            panic!("expected Settings overlay");
        };
        sv.rows
            .iter()
            .position(|r| r.label == label)
            .unwrap_or_else(|| panic!("missing Settings row {label:?}"))
    };

    // Contextual Key Hints toggle back on via Enter.
    app.settings_ui.cursor = find_settings_row(&app, "Contextual Key Hints");
    app.press_keys("<Enter>").unwrap();
    assert!(app.config.show_key_hints);
    assert!(!snap_view(&app).chrome.hints.is_empty());

    // Default Tempo (BPM) clamps strictly to 20..300 (matching UserConfig::validate).
    app.settings_ui.cursor = find_settings_row(&app, "Default Tempo");
    app.press_keys("<Enter>20<Enter>").unwrap();
    assert_eq!(app.mode_name(), "SETTINGS");
    assert!((app.config.default_bpm - 20.0).abs() < 1e-6);
    app.press_keys("<").unwrap(); // coarse down at lower bound must stay at 20.0
    assert!((app.config.default_bpm - 20.0).abs() < 1e-6);
    app.press_keys("<Enter>300<Enter>").unwrap();
    assert!((app.config.default_bpm - 300.0).abs() < 1e-6);
    app.press_keys(">").unwrap(); // coarse up at upper bound must stay at 300.0
    assert!((app.config.default_bpm - 300.0).abs() < 1e-6);
    assert!(app.config.validate().is_ok());

    // Default Key Root clamps strictly to 24..84 (C1..C6).
    app.settings_ui.cursor = find_settings_row(&app, "Default Key");
    app.press_keys("<Enter>24<Enter>").unwrap();
    assert_eq!(app.config.default_root_midi, 24);
    app.press_keys("<").unwrap();
    assert_eq!(app.config.default_root_midi, 24);
    app.press_keys("<Enter>84<Enter>").unwrap();
    assert_eq!(app.config.default_root_midi, 84);
    app.press_keys(">").unwrap();
    assert_eq!(app.config.default_root_midi, 84);
    assert!(app.config.validate().is_ok());

    // Default Scale cycles cleanly and applies to `Project::empty_with_config`.
    app.settings_ui.cursor = find_settings_row(&app, "Default Scale");
    let orig_scale = app.config.default_scale.clone();
    app.press_keys(".").unwrap();
    assert_ne!(app.config.default_scale, orig_scale);
    let new_proj = Project::empty_with_config(&app.config);
    let expected_scale = cadence_core::music::named_scale(&app.config.default_scale).unwrap();
    assert_eq!(new_proj.scale, expected_scale);

    // `r` resets Settings to defaults with accurate `Ctrl+s` status hint (no `press Enter or s` lie).
    app.press_keys("r").unwrap();
    assert!(
        app.status.contains("Ctrl+s"),
        "reset_config status must advertise Ctrl+s: {}",
        app.status
    );
    assert!(
        !app.status.contains("press Enter or s"),
        "reset_config status must not advertise obsolete Enter/s save hint: {}",
        app.status
    );
    // `Ctrl+s` saves config via HostIo cleanly.
    app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .unwrap();
    assert!(!app.settings_ui.config_dirty);
    app.press_keys("<Esc>").unwrap();

    // 4. `WorkflowOutcome::BrowseFolder` emits `FileDialogRequest::PickWorkflowSampleFolder`
    //    and round-trips through `App::apply_file_dialog_result`!
    let sample_folder = dir.path().join("kit-hits");
    std::fs::create_dir_all(&sample_folder).unwrap();
    // Open New Instrument workflow (`ni`), accept name (`<Enter>`), choose Kit (`d`), choose Sample folder (`f`), press `F`!
    app.press_keys("ni<Enter>df").unwrap();
    assert_eq!(app.mode_name(), "WORKFLOW");
    let browse_out = app
        .handle_key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT))
        .unwrap();
    assert_eq!(
        browse_out.file_dialog,
        Some(FileDialogRequest::PickWorkflowSampleFolder),
        "Pressing F on workflow folder step must request PickWorkflowSampleFolder host dialog"
    );
    app.apply_file_dialog_result(
        FileDialogRequest::PickWorkflowSampleFolder,
        sample_folder.clone(),
    )
    .unwrap();
    assert_eq!(app.mode_name(), "WORKFLOW");
    let wf_text = match snap_view(&app).overlay {
        Some(OverlayView::Authoring(w)) => w
            .fields
            .iter()
            .find(|f| f.active)
            .map(|f| f.value.clone())
            .unwrap_or_default(),
        other => panic!("expected Authoring/Workflow overlay after applying folder, got {other:?}"),
    };
    assert_eq!(wf_text, sample_folder.display().to_string());
    app.press_keys("<Esc>").unwrap();
}
