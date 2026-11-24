//! Integration tests for modal overlays and the Action Palette:
//! 1. Unified Rack Instrument Preset Browser (`P`): 6-category filtering (`h/l`), bare templates,
//!    `/` search, live non-destructive preview on `j/k` (zero undo / zero dirty on `Esc` & `Ctrl-c`),
//!    single undo commit on `Enter`, `a` live audition, `S` save preset flow, and `r` modal
//!    `RenameInstrument` (shared 1–80 SPOT).
//! 2. Song / Project Properties modal (`gP`): Title, Tempo (BPM), Key Root, all 13 named Scales
//!    from `cadence_core::music::NAMED_SCALES`, Length (Bars), End Rule, Live Timing + undo/redo.
//! 3. Dedicated Mini-Notation Editor (`Mode::MiniEditor`): live keystroke parse validation
//!    (`valid` + event count vs syntax error blocking commit), `Enter` commit, `Esc` cancel.
//! 4. Unified `Mode::ValueInput`: exact numeric/unit entry across Rack Module, Effect, Rack Zone,
//!    Mixer Fader/Pan/Send, Placement Offset, and Macro Dial.
//! 5. Fuzzy Action Palette (`:` & `Ctrl-p` -> `Mode::Palette`): fuzzy match execution (`"song key"`,
//!    `"preset"`, `"reverb"`, `"arm take"`, dynamic section/pattern/instrument entries), top-slot
//!    command passthrough for `--key-script` & power users, and Jam mode clean return.

mod common;

use cadence_app::view::OverlayView;
use cadence_app::{
    Action, App, ControlTarget, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MixerSlot, RackTab,
};
use cadence_core::macros::{MacroBinding, MacroControl};
use cadence_core::material::{
    Boundary, EditTiming, InstrumentInstance, Modulation, ModulationTarget, ParameterLink, Part,
    Pattern, PatternKind, Project, ValuePoint, ValueUnit,
};
use cadence_core::mixer::{ChannelRef, EffectKind};
use cadence_core::music::{EndRule, NAMED_SCALES};
use cadence_core::params::{InstrumentParam, ModuleParam, ParamAddress, ValueMode};
use cadence_core::presets::{InstrumentPreset, PresetCatalog};
use cadence_core::rack::{ModuleKind, RackPatch, RackZone};
use cadence_core::runtime::AudioSnapshot;
use cadence_core::sampler::SampleLibrary;
use std::path::Path;

fn write_test_wav(path: &Path, frames: usize) {
    common::write_sine_wav(path, 1, 48_000, frames, 440.0, 0.43);
}

fn send_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> cadence_app::Outcome {
    app.handle_key(KeyEvent::new(code, modifiers))
        .expect("handle_key")
}

fn synth_zone(id: u16, name: &str, midi: u8, cutoff_hz: f32) -> RackZone {
    let mut zone = RackPatch::default_synth().zones.remove(0);
    zone.id = id;
    zone.name = name.into();
    zone.low_midi = midi;
    zone.high_midi = midi;
    zone.root_midi = midi;
    if let Some(ModuleKind::Filter(f)) = zone.module_mut(2).map(|m| &mut m.kind) {
        f.cutoff_hz = cutoff_hz;
    }
    zone
}

#[test]
fn preset_browser_category_filtering_live_preview_zero_undo_esc_single_undo_enter_save_and_rename()
{
    let dir = tempfile::tempdir().unwrap();
    let presets_dir = dir.path().join("presets");
    let samples_dir = dir.path().join("samples");
    std::fs::create_dir_all(&presets_dir).unwrap();
    std::fs::create_dir_all(&samples_dir).unwrap();

    // Synthetic WAV for Sampler audition & readiness.
    let wav_path = samples_dir.join("tone.wav");
    write_test_wav(&wav_path, 4096);

    // Build synthetic Synth, Sampler, and Kit Rack presets on disk (100% self-contained).
    let mut bright_synth = RackPatch::default_synth();
    if let Some(ModuleKind::Filter(f)) = bright_synth.zones[0].module_mut(2).map(|m| &mut m.kind) {
        f.cutoff_hz = 6400.0;
        f.resonance = 0.42;
    }
    InstrumentPreset::new("Bright Saw", Some("Lead".into()), bright_synth.clone())
        .save(&presets_dir.join("bright-saw.preset.json"))
        .unwrap();

    let mut sub_synth = RackPatch::default_synth();
    if let Some(ModuleKind::Filter(f)) = sub_synth.zones[0].module_mut(2).map(|m| &mut m.kind) {
        f.cutoff_hz = 320.0;
    }
    InstrumentPreset::new("Deep Sub", Some("Bass".into()), sub_synth)
        .save(&presets_dir.join("deep-sub.preset.json"))
        .unwrap();

    let mut sampler_preset = RackPatch::default_sampler();
    if let Some(ModuleKind::Sampler(s)) = sampler_preset.zones[0].module_mut(1).map(|m| &mut m.kind)
    {
        s.path = Some("tone.wav".into());
    }
    if let Some(ModuleKind::Filter(f)) = sampler_preset.zones[0].module_mut(2).map(|m| &mut m.kind)
    {
        f.cutoff_hz = 4200.0;
    }
    InstrumentPreset::new("Clean Tone Sampler", Some("Sampler".into()), sampler_preset)
        .save(&presets_dir.join("clean-tone.preset.json"))
        .unwrap();

    let kit_preset = RackPatch::default_kit();
    InstrumentPreset::new("Synthetic Kit", Some("Kit".into()), kit_preset)
        .save(&presets_dir.join("synthetic-kit.preset.json"))
        .unwrap();

    let save_path = dir.path().join("project.json");
    let lib = SampleLibrary::from_roots_only(&[samples_dir], Some(&save_path));
    let mut app = App::new_with_library(Project::demo(), save_path, lib).unwrap();
    app.set_preset_catalog(PresetCatalog::from_roots(std::slice::from_ref(
        &presets_dir,
    )));
    assert!(!app.dirty());
    let rev0 = app.revision();

    // Focus the first Rack instrument ("Saw").
    let synth_id = app.project().instruments[0].id;
    let orig_patch = app.instrument(synth_id).unwrap().rack.clone();
    let orig_name = app.instrument(synth_id).unwrap().name.clone();
    app.execute_command(&format!("instrument {orig_name}"))
        .unwrap();

    // 1. Open Preset Browser via `P` on Instrument deck -> unified `RACK` catalog with category tabs!
    app.press_keys("P").unwrap();
    assert_eq!(app.mode_name(), "PRESETS");
    let pb_all = app.preset_browser_view().expect("PresetBrowser view");
    assert_eq!(pb_all.engine_label, "RACK");
    assert_eq!(pb_all.active_category, "All");
    assert_eq!(pb_all.categories.len(), 6);
    assert!(pb_all.items.iter().any(|i| i.name == "Bright Saw"));
    assert!(pb_all.items.iter().any(|i| i.name == "Deep Sub"));
    assert!(pb_all.items.iter().any(|i| i.name == "Clean Tone Sampler"));
    assert!(pb_all.items.iter().any(|i| i.name == "Synthetic Kit"));

    // Cycling category (`l`) to `Synth` filters cleanly to Synth presets only!
    app.press_keys("l").unwrap();
    let pb = app.preset_browser_view().unwrap();
    assert_eq!(pb.active_category, "Synth");
    assert!(pb.items.iter().any(|i| i.name == "Bright Saw"));
    assert!(pb.items.iter().any(|i| i.name == "Deep Sub"));
    assert!(
        !pb.items.iter().any(|i| i.name == "Clean Tone Sampler"),
        "Sampler presets must be hidden under Synth category"
    );
    assert!(
        !pb.items.iter().any(|i| i.name == "Synthetic Kit"),
        "Kit presets must be hidden under Synth category"
    );

    // 2. Live non-destructive preview on `j`/`k`: updates live audio score WITHOUT undo or dirty!
    let preview_outs = app.press_keys("j").unwrap();
    assert!(
        preview_outs.iter().any(|o| o.changed),
        "Scrolling presets must refresh audio engine live"
    );
    assert!(
        !app.dirty(),
        "Live preview while scrolling must NOT mark document dirty"
    );
    assert_eq!(
        app.revision(),
        rev0,
        "Live preview must NOT bump document revision or push undo"
    );
    // Live audio_compiled() must reflect the previewed RackPatch immediately!
    let live_patch = &app
        .audio_compiled()
        .instruments
        .iter()
        .find(|i| i.id == synth_id)
        .unwrap()
        .rack;
    assert_ne!(
        live_patch, &orig_patch,
        "Live compiled audio score must reflect the previewed preset immediately"
    );

    // 3. Audition `A` inside Preset Browser emits audition_instrument!
    let aud_outs = app.press_keys("A").unwrap();
    assert!(
        aud_outs.iter().any(|o| o.audition_instrument.is_some()),
        "`A` in Preset Browser must audition a note on the previewed instrument"
    );

    // 4. Universal `/` search: Enter commits filter -> 1st Esc clears filter -> 2nd Esc cancels!
    app.press_keys("/deep<Enter>").unwrap();
    let pb_filt = app.preset_browser_view().unwrap();
    assert!(!pb_filt.searching);
    assert_eq!(pb_filt.query, "deep");
    assert_eq!(pb_filt.items.len(), 1);
    assert_eq!(pb_filt.items[0].name, "Deep Sub");

    // First Esc clears filter and stays in Preset Browser.
    app.press_keys("<Esc>").unwrap();
    assert_eq!(app.mode_name(), "PRESETS");
    assert_eq!(app.preset_browser_view().unwrap().query, "");

    // Second Esc restores exact original patch with ZERO undo entries & unchanged dirty()!
    app.press_keys("<Esc>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert_eq!(app.instrument(synth_id).unwrap().rack, orig_patch);
    assert!(!app.dirty(), "Esc cancel must leave document clean");
    assert_eq!(
        app.revision(),
        rev0,
        "Esc cancel must leave revision untouched"
    );
    let undo_out = app.apply(Action::Undo).unwrap();
    assert!(
        !undo_out.changed,
        "Esc cancel must have left ZERO entries on the undo stack"
    );

    // Ctrl-c during preview also restores original patch with ZERO undo entries!
    app.press_keys("Pj").unwrap();
    send_key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(app.mode_name(), "NORMAL");
    assert_eq!(app.instrument(synth_id).unwrap().rack, orig_patch);
    assert!(!app.dirty());
    assert_eq!(app.revision(), rev0);

    // 5. Enter commits ONE single clean undoable transaction!
    app.press_keys("P/bright<Enter><Enter>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert!(app.dirty());
    assert_eq!(app.revision(), rev0 + 1);
    assert_eq!(app.instrument(synth_id).unwrap().rack, bright_synth);
    // ONE single `u` restores exact original patch!
    app.press_keys("u").unwrap();
    assert_eq!(app.instrument(synth_id).unwrap().rack, orig_patch);
    assert!(!app.dirty());

    // 6. Category filtering on Sampler & Kit tabs works cleanly across any Rack instrument!
    app.execute_command("instrument TestSamp sampler").unwrap();
    app.press_keys("Pll").unwrap(); // All -> Synth -> Sampler
    let pb_samp = app.preset_browser_view().unwrap();
    assert_eq!(pb_samp.engine_label, "RACK");
    assert_eq!(pb_samp.active_category, "Sampler");
    assert!(pb_samp.items.iter().any(|i| i.name == "Clean Tone Sampler"));
    assert!(!pb_samp.items.iter().any(|i| i.name == "Bright Saw"));
    app.press_keys("<Esc>").unwrap();

    app.execute_command("instrument TestKit kit").unwrap();
    app.press_keys("Pllll").unwrap(); // All -> Synth -> Sampler -> Drums -> Kit
    let pb_kit = app.preset_browser_view().unwrap();
    assert_eq!(pb_kit.engine_label, "RACK");
    assert_eq!(pb_kit.active_category, "Kit");
    assert!(pb_kit.items.iter().any(|i| i.name == "Synthetic Kit"));
    assert!(!pb_kit.items.iter().any(|i| i.name == "Bright Saw"));
    app.press_keys("<Esc>").unwrap();

    // 7. `Ctrl+Shift+s` opens FileHub Save tab; row 3 is Save Active Instrument Preset!
    app.execute_command(&format!("instrument {orig_name}"))
        .unwrap();
    app.press_keys("<c-s-s>jjj").unwrap();
    assert_eq!(app.mode_name(), "FILES");
    let snap = AudioSnapshot::default();
    let fv = app.frame_view(&snap, "silent");
    match &fv.overlay {
        Some(OverlayView::FileHub(hub)) => {
            assert_eq!(hub.active_tab, cadence_app::FileHubTab::Save);
            assert_eq!(hub.cursor, 3, "Row 3 must be Save Active Instrument Preset");
        }
        other => panic!("expected FileHub Save overlay, got {other:?}"),
    }
    app.press_keys("<Enter>").unwrap();
    // Saved preset file exists on disk under `<asset_base>/presets/`!
    assert!(
        dir.path()
            .join("presets")
            .read_dir()
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().ends_with(".preset.json")),
        "Pressing Enter on FileHub Save preset row must write <asset_base>/presets/<slug>.preset.json"
    );
    app.press_keys("<Esc>").unwrap();

    // 8. Contextual `r` on Instrument deck renames focused Quick Pin on `QUICK` rows,
    //    and opens modal WorkflowKind::RenameInstrument on `RACK` / `ROUTING` rows!
    app.press_keys("r").unwrap();
    assert_eq!(app.mode_name(), "VALUE ENTRY");
    app.press_keys("<Esc>").unwrap();
    app.press_keys("wr").unwrap();
    assert_eq!(app.mode_name(), "WORKFLOW");
    let draft = app.authoring_view().expect("RenameInstrument draft");
    assert_eq!(draft.title, "RENAME INSTRUMENT");
    // Empty name is rejected by shared 1–80 validation.
    send_key(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
    app.press_keys("<Enter>").unwrap();
    assert_eq!(
        app.mode_name(),
        "WORKFLOW",
        "Empty instrument name must be rejected"
    );
    app.press_keys("Velvet Lead<Enter>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert_eq!(app.instrument(synth_id).unwrap().name, "Velvet Lead");
    // Undo restores previous instrument name cleanly!
    app.press_keys("u").unwrap();
    assert_eq!(app.instrument(synth_id).unwrap().name, orig_name);
}

#[test]
fn preset_browser_multizone_rack_preview_preserves_zone_modulations_links_and_macros() {
    let dir = tempfile::tempdir().unwrap();
    let presets_dir = dir.path().join("presets");
    std::fs::create_dir_all(&presets_dir).unwrap();

    // Build a clean Project containing a 2-zone Modular Rack instrument ("PercKit") with:
    //   1. A zone-2 Modulation targeting `Module { zone: 2, module: 2, param: Cutoff }`,
    //   2. A Part playing PercKit with a zone-2 `ParameterLink`, and
    //   3. A Project Macro (`M1`) with a zone-2 `MacroBinding`.
    let mut project = Project::demo();
    let kit_id = project.max_id() + 1;
    let orig_kit = RackPatch {
        kit_name: "PercKit".into(),
        zones: vec![
            synth_zone(1, "Kick", 36, 7_000.0),
            synth_zone(2, "Snare", 37, 3_200.0),
        ],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 20_000.0,
        resonance: 0.0,
        panel_pins: Vec::new(),
    };
    project.instruments.push(InstrumentInstance {
        id: kit_id,
        name: "PercKit".into(),
        rack: orig_kit,
        channel: project.mixer.channels[0].id,
    });

    let val_pat_id = project.max_id() + 1;
    project.patterns.push(Pattern {
        id: val_pat_id,
        name: "ZoneRamp".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 16,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 0.25,
                },
                ValuePoint {
                    step: 8,
                    value: 0.85,
                },
            ],
        },
    });

    let zone2_param = InstrumentParam::Module {
        zone: 2,
        module: 2,
        param: ModuleParam::Cutoff,
    };
    let zone2_addr = ParamAddress::from_instrument(kit_id, zone2_param);

    let mod_id = project.max_id() + 1;
    project.modulations.push(Modulation {
        id: mod_id,
        name: "SnareCut".into(),
        target: ModulationTarget::Instrument {
            instrument: kit_id,
            parameter: zone2_param,
        },
        pattern: val_pat_id,
    });

    let part_id = project.max_id() + 1;
    let note_pat_id = project.patterns[0].id;
    let mut kit_part = Part::new(part_id, "KitPart", note_pat_id, kit_id);
    kit_part.parameter_links.push(ParameterLink {
        target: zone2_param,
        pattern: val_pat_id,
    });
    project.parts.push(kit_part);

    project.macros.push(MacroControl {
        index: 0,
        name: "SnareMacro".into(),
        value: 0.5,
        bindings: vec![MacroBinding::new(zone2_addr, 0.15, 0.9)],
    });
    project.validate().unwrap();

    // Save two Kit Rack presets: a 1-zone preset (triggers zone-2 pruning during preview)
    // and an alternate 2-zone preset (still compatible with zone-2 Filter Cutoff).
    let one_zone_kit = RackPatch {
        kit_name: "One Zone".into(),
        zones: vec![synth_zone(1, "SoloKick", 36, 900.0)],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 20_000.0,
        resonance: 0.0,
        panel_pins: Vec::new(),
    };
    InstrumentPreset::new("One Zone Minimal", Some("Kit".into()), one_zone_kit)
        .save(&presets_dir.join("one-zone-minimal.preset.json"))
        .unwrap();

    let two_zone_alt = RackPatch {
        kit_name: "Two Zone Alt".into(),
        zones: vec![
            synth_zone(1, "AltKick", 36, 7_000.0),
            synth_zone(2, "AltSnare", 37, 6_800.0),
        ],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 20_000.0,
        resonance: 0.0,
        panel_pins: Vec::new(),
    };
    InstrumentPreset::new("Two Zone Alt", Some("Kit".into()), two_zone_alt)
        .save(&presets_dir.join("two-zone-alt.preset.json"))
        .unwrap();

    let mut app = App::new(project, dir.path().join("kit-project.json")).unwrap();
    app.set_preset_catalog(PresetCatalog::from_roots(&[presets_dir]));
    app.execute_command("instrument PercKit").unwrap();
    assert!(!app.dirty());
    let rev0 = app.revision();
    let orig_project = app.project().clone();

    let has_zone2_bindings = |p: &Project| {
        let has_mod = p.modulations.iter().any(|m| m.id == mod_id);
        let has_link = p
            .parts
            .iter()
            .find(|pt| pt.id == part_id)
            .is_some_and(|pt| pt.parameter_links.iter().any(|l| l.target == zone2_param));
        let has_macro = p
            .macro_control(0)
            .is_some_and(|mc| mc.bindings.iter().any(|b| b.target == zone2_addr));
        (has_mod, has_link, has_macro)
    };
    assert_eq!(has_zone2_bindings(&orig_project), (true, true, true));

    // 1. Open Preset Browser (`P`) and preview 1-zone `Blank Kit`:
    //    temporarily drops to 1 zone in live preview without dirtying or bumping revision.
    app.press_keys("P/blank kit<Enter>").unwrap();
    assert_eq!(app.mode_name(), "PRESETS");
    assert!(!app.dirty());
    assert_eq!(app.revision(), rev0);
    assert_eq!(
        has_zone2_bindings(app.project()),
        (false, false, false),
        "1-zone preview must prune incompatible zone-2 bindings in the previewed score"
    );

    // 2. Scroll past 1-zone preset onto compatible 2-zone `Two Zone Alt`:
    //    preview MUST branch from untouched `original_project`, preserving ALL 3 zone-2 bindings!
    app.press_keys("<Esc>/two zone<Enter>").unwrap();
    assert_eq!(
        has_zone2_bindings(app.project()),
        (true, true, true),
        "Scrolling past Blank Kit onto a 2-zone kit must NOT cascade zone pruning"
    );
    assert!(!app.dirty());
    assert_eq!(app.revision(), rev0);

    // 3. Scroll back to `One Zone Minimal`, then cancel (`Esc` x2):
    //    restores exact `orig_project` bit-for-bit with ZERO undo entries and clean state!
    app.press_keys("<Esc>/one zone<Enter>").unwrap();
    assert_eq!(has_zone2_bindings(app.project()), (false, false, false));
    app.press_keys("<Esc><Esc>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert_eq!(
        app.project(),
        &orig_project,
        "Esc cancel must restore exact original Project including all zone modulations, links, and macros"
    );
    assert!(!app.dirty());
    assert_eq!(app.revision(), rev0);
    assert!(
        !app.apply(Action::Undo).unwrap().changed,
        "Esc cancel must leave ZERO undo entries"
    );

    // 3b. Toggling live mute (`Alt-m`) while previewing 1-zone preset and then pressing `Esc`
    //     keeps the live mute flag active while still restoring `orig_project` bit-for-bit!
    app.press_keys("P/one zone<Enter>").unwrap();
    assert_eq!(has_zone2_bindings(app.project()), (false, false, false));
    send_key(&mut app, KeyCode::Char('m'), KeyModifiers::ALT);
    app.press_keys("<Esc><Esc>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert_eq!(app.project(), &orig_project);
    assert!(
        !app.flags().muted_lanes.is_empty() || !app.flags().muted_channels.is_empty(),
        "Live mute toggled during preview must survive Preset Browser cancel"
    );
    // Unmute back cleanly -> document returns to clean (!dirty).
    send_key(&mut app, KeyCode::Char('m'), KeyModifiers::ALT);
    assert!(!app.dirty());

    // 4. `Ctrl+Shift+j` from Preset Browser while previewing 1-zone preset restores `orig_project` cleanly
    //    before entering Jam mode (no leaked preview!).
    app.press_keys("P/blank kit<Enter>").unwrap();
    assert_eq!(has_zone2_bindings(app.project()), (false, false, false));
    send_key(
        &mut app,
        KeyCode::Char('J'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert!(app.is_jam());
    assert_eq!(
        app.project(),
        &orig_project,
        "Entering Jam via Ctrl+Shift+j from Preset Browser must cancel preview and restore original Project"
    );
    app.press_keys("<Esc>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");

    // 5. Commit `Blank Kit` via `Enter`, then press `u` ONCE:
    //    ONE single Undo restores the full 2-zone kit AND all 3 zone-2 bindings bit-for-bit!
    app.press_keys("P/blank kit<Enter><Enter>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert!(app.dirty());
    assert_eq!(app.revision(), rev0 + 1);
    assert_eq!(has_zone2_bindings(app.project()), (false, false, false));

    app.press_keys("u").unwrap();
    assert_eq!(
        app.project(),
        &orig_project,
        "Single Undo after committing Blank Kit must restore all zone modulations, links, and macros"
    );
    assert!(!app.dirty());
}

#[test]
fn song_properties_modal_all_fields_thirteen_scales_and_undo_redo() {
    let mut app = App::new(Project::demo(), "unused.json".into()).unwrap();
    let orig_title = app.project().title.clone();
    let orig_bpm = app.project().bpm;
    let orig_root = app.project().root_midi;
    let orig_scale = app.project().scale.clone();
    let orig_bars = app.project().bars;
    let orig_end = app.project().end;

    // Open Song Properties via `gP`.
    app.press_keys("gP").unwrap();
    assert_eq!(app.mode_name(), "SONG PROPS");
    assert!(app.song_properties_view().is_some());

    // 1. Row 0: Title inline edit (`Enter` -> `Ctrl-u` -> type -> `Enter`).
    app.press_keys("<Enter>").unwrap();
    assert_eq!(app.mode_name(), "SONG PROPS");
    send_key(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
    app.press_keys("Neon Horizon<Enter>").unwrap();
    assert_eq!(app.project().title, "Neon Horizon");

    // 2. Row 1: Tempo (BPM) `j` -> `Alt+l` (+1), `Alt+L` (+10), exact `<Enter>142.5<Enter>`.
    app.press_keys("j<m-l>").unwrap();
    assert!((app.project().bpm - (orig_bpm + 1.0)).abs() < 1e-6);
    app.press_keys("<m-L>").unwrap();
    assert!((app.project().bpm - (orig_bpm + 11.0)).abs() < 1e-6);
    // Repeat nudge coalesces into a single undo entry!
    let rev_before_repeat = app.revision();
    app.handle_key(KeyEvent::new_with_kind(
        KeyCode::Char('l'),
        KeyModifiers::ALT,
        KeyEventKind::Repeat,
    ))
    .unwrap();
    app.handle_key(KeyEvent::new_with_kind(
        KeyCode::Char('l'),
        KeyModifiers::ALT,
        KeyEventKind::Repeat,
    ))
    .unwrap();
    assert!((app.project().bpm - (orig_bpm + 13.0)).abs() < 1e-6);
    assert!(app.revision() > rev_before_repeat);
    // Exact BPM via unified ValueInput returns cleanly to Song Properties!
    app.press_keys("<Enter>142.5<Enter>").unwrap();
    assert_eq!(app.mode_name(), "SONG PROPS");
    assert!((app.project().bpm - 142.5).abs() < 1e-6);

    // 3. Row 2: Key Root (`j` -> `Alt+l` +1 st, exact `<Enter>F#3<Enter>`).
    app.press_keys("j<m-l>").unwrap();
    assert_eq!(app.project().root_midi, orig_root + 1);
    app.press_keys("<Enter>F#3<Enter>").unwrap();
    assert_eq!(app.project().root_midi, 54); // F#3 = MIDI 54

    // 4. Row 3: Scale cycles cleanly through ALL 13 named scales in `NAMED_SCALES`!
    app.press_keys("j").unwrap();
    assert_eq!(NAMED_SCALES.len(), 13);
    let mut visited = std::collections::HashSet::new();
    for _ in 0..13 {
        visited.insert(app.project().scale.clone());
        app.press_keys("<m-l>").unwrap();
    }
    assert_eq!(
        visited.len(),
        13,
        "Cycling Scale row 13 times must visit all 13 authoritative NAMED_SCALES"
    );
    // Check that extended modal/symmetric scales are present in the cycle.
    for required in ["Lydian", "Melodic Minor", "Blues", "Whole Tone"] {
        let spec = NAMED_SCALES.iter().find(|s| s.label == required).unwrap();
        assert!(
            visited.contains(spec.intervals),
            "Missing scale {required} in Song Properties cycle"
        );
    }

    // 5. Row 4: Length (Bars) `j` -> `Alt+l` (+1 bar), `Alt+L` (+8 bars).
    app.press_keys("j<m-l>").unwrap();
    assert_eq!(app.project().bars, orig_bars + 1);
    app.press_keys("<m-L>").unwrap();
    assert_eq!(app.project().bars, orig_bars + 9);

    // 6. Row 5: End Rule cycles Loop -> Stop -> Continue -> Loop!
    app.press_keys("j<m-l>").unwrap();
    assert_eq!(app.project().end, orig_end.next());
    app.press_keys("<m-l>").unwrap();
    assert_eq!(app.project().end, orig_end.next().next());
    app.press_keys("<m-l>").unwrap();
    assert_eq!(app.project().end, orig_end);
    app.press_keys("<m-l>").unwrap();
    assert_eq!(app.project().end, EndRule::Stop);

    // 7. Rows 6–8: Live Timing (Launch Quantize, Edit Timing, Late Tolerance)!
    let orig_tol = app.project().timing.late_tolerance_steps;
    app.press_keys("j<m-l>").unwrap();
    assert_eq!(app.project().timing.launch, Boundary::Bars { count: 2 });
    app.press_keys("j<m-l>").unwrap();
    assert_eq!(app.project().timing.edits, EditTiming::Launch);
    app.press_keys("j<m-l>").unwrap();
    assert_eq!(app.project().timing.late_tolerance_steps, orig_tol + 1);

    // Undo/Redo work cleanly right inside Song Properties!
    app.press_keys("u").unwrap();
    assert_eq!(app.project().timing.late_tolerance_steps, orig_tol);
    app.press_keys("U").unwrap();
    assert_eq!(app.project().timing.late_tolerance_steps, orig_tol + 1);

    // Esc closes Song Properties directly back to NORMAL!
    app.press_keys("<Esc>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert_ne!(app.project().title, orig_title);
    assert_eq!(app.project().scale, orig_scale);
}

#[test]
fn mini_notation_editor_live_validation_commit_and_cancel() {
    let mut app = App::new(Project::demo(), "unused.json".into()).unwrap();
    app.execute_command("mini Groove 8 c4 e4 g4 b4").unwrap();
    app.press_keys("gp").unwrap();

    // Both `i` and `Enter` open dedicated `Mode::MiniEditor`.
    app.press_keys("<Enter>").unwrap();
    assert_eq!(app.mode_name(), "MINI EDITOR");
    let me = app.mini_editor_view().expect("MiniEditor view");
    assert_eq!(me.code, "c4 e4 g4 b4");
    assert!(me.valid);
    assert_eq!(me.event_count, 4);
    assert!(me.error.is_none());

    // Live syntax error validation BEFORE pressing Enter!
    send_key(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
    app.press_keys("[c4 e4").unwrap();
    let me_err = app.mini_editor_view().unwrap();
    assert!(!me_err.valid);
    assert!(me_err.error.is_some());

    // Pressing Enter while invalid refuses to commit and keeps modal open!
    app.press_keys("<Enter>").unwrap();
    assert_eq!(app.mode_name(), "MINI EDITOR");

    // Fixing syntax immediately flips live state back to valid!
    app.press_keys("] ~ [g4 b4]").unwrap();
    let me_ok = app.mini_editor_view().unwrap();
    assert!(me_ok.valid, "err={:?}", me_ok.error);
    assert_eq!(me_ok.event_count, 4);

    // Enter commits `Edit::PutPattern` cleanly and closes modal!
    app.press_keys("<Enter>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    let pat = app
        .project()
        .patterns
        .iter()
        .find(|p| p.name == "Groove")
        .unwrap();
    match &pat.kind {
        PatternKind::Mini { code, steps } => {
            assert_eq!(code, "[c4 e4] ~ [g4 b4]");
            assert_eq!(*steps, 8);
        }
        other => panic!("expected Mini pattern, got {other:?}"),
    }

    // Reopen with `Enter`, edit buffer, Esc cancels without mutating pattern!
    app.press_keys("<Enter>").unwrap();
    assert_eq!(app.mode_name(), "MINI EDITOR");
    send_key(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
    app.press_keys("c5 d5<Esc>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    let pat2 = app
        .project()
        .patterns
        .iter()
        .find(|p| p.name == "Groove")
        .unwrap();
    match &pat2.kind {
        PatternKind::Mini { code, .. } => assert_eq!(code, "[c4 e4] ~ [g4 b4]"),
        other => panic!("expected Mini pattern, got {other:?}"),
    }
}

#[test]
fn unified_value_input_across_rack_effect_zone_mixer_and_macros() {
    let mut app = App::new(Project::demo(), "unused.json".into()).unwrap();

    // 1. Rack Filter `Cutoff` via digit entry & exact units ("1.75khz") + live parse validation!
    let inst_id = app.project().instruments[0].id;
    let inst_name = app.project().instruments[0].name.clone();
    app.execute_command(&format!("instrument {inst_name}"))
        .unwrap();
    let cutoff_idx = app
        .control_descriptors()
        .iter()
        .position(|c| {
            matches!(
                c.target,
                ControlTarget::Param(ParamAddress::Module {
                    param: ModuleParam::Cutoff,
                    ..
                })
            )
        })
        .expect("Cutoff control");
    app.control_cursor = cutoff_idx;
    app.press_keys("<Enter>1").unwrap();
    assert_eq!(app.mode_name(), "VALUE ENTRY");
    assert_eq!(app.command_line(), None, "Must NOT open Mode::Command");
    app.press_keys(".75khz").unwrap();
    let vi = app.value_input_view().unwrap();
    assert!(vi.error.is_none());
    assert!(vi.live_preview.is_some());
    // Test live error on nonsense text, then clear & commit exact 1750 Hz.
    app.press_keys("zzz").unwrap();
    assert!(app.value_input_view().unwrap().error.is_some());
    send_key(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
    app.press_keys("1750hz<Enter>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    let Some(ModuleKind::Filter(flt)) = app.instrument(inst_id).unwrap().rack.zones[0]
        .module(4)
        .map(|m| &m.kind)
    else {
        panic!("expected Filter module 4 on demo Saw");
    };
    assert!((flt.cutoff_hz - 1750.0).abs() < 1.0);

    // 2. Effect parameter via unified ValueInput.
    let ch0 = ChannelRef::Channel(app.project().mixer.channels[0].id);
    app.apply(Action::AddEffect {
        channel: ch0,
        kind: "reverb".into(),
    })
    .unwrap();
    let fx_id = app.project().mixer.channels[0].inserts.last().unwrap().id;
    app.execute_command(&format!("fx open {fx_id}")).ok();
    app.press_keys("<Enter>50%<Enter>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");

    // 3. Rack structural Zone field (`LowMidi` -> "C2") via unified ValueInput on Zones tab!
    app.execute_command("instrument Kit kit").unwrap();
    let kit_id = app.selected_instrument.unwrap();
    app.press_keys("H").unwrap(); // Panel -> Zones
    assert_eq!(app.rack_ui.tab, RackTab::Zones);
    let low_idx = app
        .control_descriptors()
        .iter()
        .position(|c| {
            matches!(
                c.target,
                ControlTarget::Zone {
                    field: cadence_app::ZoneControl::LowMidi,
                    ..
                }
            )
        })
        .expect("LowMidi control");
    app.control_cursor = low_idx;
    app.press_keys("<Enter>C2<Enter>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert_eq!(app.instrument(kit_id).unwrap().rack.zones[0].low_midi, 36);

    // 4. Mixer Fader & Pan via unified ValueInput!
    app.press_keys("gm").unwrap();
    app.selected_channel = ch0;
    app.view_state.mixer_slot = MixerSlot::Fader;
    app.press_keys("<Enter>-6db<Enter>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    let gain_db = cadence_core::dsp::lin_to_db(
        app.channel(app.project().mixer.channels[0].id)
            .unwrap()
            .strip
            .gain,
    );
    assert!((gain_db - (-6.0)).abs() < 0.15, "gain_db={gain_db}");

    app.view_state.mixer_slot = MixerSlot::Pan;
    app.press_keys("<Enter>R50<Enter>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    let pan = app
        .channel(app.project().mixer.channels[0].id)
        .unwrap()
        .strip
        .pan;
    assert!((pan - 0.5).abs() < 1e-3, "pan={pan}");

    // 5. Macro Dial exact entry via unified ValueInput!
    app.press_keys("gk").unwrap();
    app.press_keys("<Enter>75%<Enter>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    let m0 = app.project().macro_control(0).expect("M1 created");
    assert!((m0.value - 0.75).abs() < 1e-3);
}

#[test]
fn fuzzy_action_palette_search_dynamic_items_and_command_passthrough() {
    let dir = tempfile::tempdir().unwrap();
    let presets_dir = dir.path().join("presets");
    std::fs::create_dir_all(&presets_dir).unwrap();
    InstrumentPreset::new(
        "Palette Saw",
        Some("Lead".into()),
        RackPatch::default_synth(),
    )
    .save(&presets_dir.join("palette-saw.preset.json"))
    .unwrap();

    let mut app = App::new(Project::demo(), dir.path().join("demo.json")).unwrap();
    app.set_preset_catalog(PresetCatalog::from_roots(&[presets_dir]));

    // 1. Open via `Ctrl+Shift+p`, fuzzy search `"song key"` -> opens Song Properties on Key Root!
    send_key(
        &mut app,
        KeyCode::Char('P'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert_eq!(app.mode_name(), "PALETTE");
    let empty_pal = app.palette_view().expect("Palette view");
    assert!(!empty_pal.items.is_empty());
    assert!(!empty_pal.command_passthrough);

    app.press_keys("song key").unwrap();
    let sk_pal = app.palette_view().unwrap();
    assert!(!sk_pal.command_passthrough);
    assert!(
        sk_pal.items[0].label.contains("Key"),
        "top match should be Set Song Key & Scale, got {:?}",
        sk_pal.items[0].label
    );
    app.press_keys("<Enter>").unwrap();
    assert_eq!(app.mode_name(), "SONG PROPS");
    app.press_keys("<Esc>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");

    // 2. Open via `:`, type `"preset"` -> fuzzy matches Browse Instrument Presets (NOT broken :preset)!
    app.press_keys(":preset").unwrap();
    let pr_pal = app.palette_view().unwrap();
    assert!(
        !pr_pal.command_passthrough,
        "0-arg `preset` must NOT hijack fuzzy matching with a broken Ex command"
    );
    assert!(pr_pal.items[0].label.contains("Preset"));
    app.press_keys("<Enter>").unwrap();
    assert_eq!(app.mode_name(), "PRESETS");
    app.press_keys("<Esc>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");

    // 3. Fuzzy search `"reverb"` -> executes Add Effect: Reverb!
    let ch = app.selected_channel;
    let fx_before = match ch {
        ChannelRef::Master => app.project().mixer.master.inserts.len(),
        ChannelRef::Channel(id) => app.channel(id).unwrap().inserts.len(),
    };
    app.press_keys(":reverb<Enter>").unwrap();
    let fx_after = match ch {
        ChannelRef::Master => app.project().mixer.master.inserts.len(),
        ChannelRef::Channel(id) => app.channel(id).unwrap().inserts.len(),
    };
    assert_eq!(fx_after, fx_before + 1);
    let added = match ch {
        ChannelRef::Master => app.project().mixer.master.inserts.last().unwrap(),
        ChannelRef::Channel(id) => app.channel(id).unwrap().inserts.last().unwrap(),
    };
    assert!(matches!(added.kind, EffectKind::Reverb(_)));

    // 4. Fuzzy search `"arm take"` -> arms take recording!
    assert!(!app.takes.armed);
    app.press_keys(":arm take<Enter>").unwrap();
    assert!(app.takes.armed);

    // 5. Dynamic project section jump from Action Palette!
    app.execute_command("section Bridge 5 4").unwrap();
    app.press_keys(":jump bridge<Enter>").unwrap();
    assert_eq!(
        app.arrange_ui.step,
        4 * 16,
        "Jump Section: Bridge must move arrange cursor to bar 5"
    );

    // 6. Seamless Command Passthrough when user hasn't moved cursor off index 0!
    app.press_keys(":bpm 136").unwrap();
    let cmd_pal = app.palette_view().unwrap();
    assert!(cmd_pal.command_passthrough);
    assert!(cmd_pal.items[0].is_command);
    assert!(cmd_pal.items[0].label.contains("bpm 136"));
    app.press_keys("<Enter>").unwrap();
    assert!((app.project().bpm - 136.0).abs() < 1e-6);

    app.press_keys(":key D4 lydian<Enter>").unwrap();
    assert_eq!(app.project().root_midi, 62);
    assert_eq!(
        cadence_core::music::scale_label(&app.project().scale),
        "Lydian"
    );

    // 7. Palette from Jam mode (`:` / `Ctrl+Shift+p`) returns cleanly to Jam on `Esc` and command run!
    app.press_keys("gj").unwrap();
    assert!(app.is_jam());
    send_key(
        &mut app,
        KeyCode::Char('P'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert_eq!(app.mode_name(), "PALETTE");
    app.press_keys("<Esc>").unwrap();
    assert!(
        app.is_jam(),
        "Esc from Palette opened in Jam must return to Jam"
    );
    // `<`/`>` shift Jam octave cleanly.
    let oct0 = app.note_octave();
    app.press_keys(">").unwrap();
    assert_eq!(app.note_octave(), oct0 + 1);
    app.press_keys("<Esc>").unwrap();
    assert!(!app.is_jam());
}

#[test]
fn palette_command_arity_static_and_dynamic_entries() {
    let mut app = App::new(Project::demo(), "palette_parity.json".into()).unwrap();

    // Ensure project has at least one modulation and one effect for dynamic Palette entries.
    let first_ch = ChannelRef::Channel(app.project().mixer.channels[0].id);
    app.apply(Action::AddEffect {
        channel: first_ch,
        kind: "drive".into(),
    })
    .unwrap();
    let fx_id = app.project().mixer.channels[0]
        .inserts
        .last()
        .expect("added drive effect")
        .id;
    app.apply(Action::RenameEffect {
        device: fx_id,
        name: "WarmthDrive".into(),
    })
    .unwrap();
    let val_pat_id = app.project().max_id() + 1;
    let mut p = app.project().clone();
    p.patterns.push(Pattern {
        id: val_pat_id,
        name: "SweepVal".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 16,
            mode: ValueMode::Ramp,
            points: vec![ValuePoint {
                step: 0,
                value: 0.5,
            }],
        },
    });
    let mod_id = val_pat_id + 1;
    p.modulations.push(Modulation {
        id: mod_id,
        name: "LeadGainMod".into(),
        target: ModulationTarget::ChannelGain { channel: first_ch },
        pattern: val_pat_id,
    });
    let mut app = App::new(p, "palette_parity.json".into()).unwrap();

    // 1. Command arity: `clips` requires arg (`clear [all]`), while `midi`, `quantize`, `wav` allow 0-arg!
    app.press_keys(":clips").unwrap();
    assert!(
        !app.palette_view().unwrap().command_passthrough,
        "0-arg `:clips` must NOT trigger command passthrough"
    );
    app.press_keys(" clear").unwrap();
    assert!(
        app.palette_view().unwrap().command_passthrough,
        "`:clips clear` MUST trigger command passthrough"
    );
    app.press_keys("<Esc>").unwrap();

    for zero_arg_cmd in ["midi", "quantize", "wav"] {
        app.press_keys(&format!(":{zero_arg_cmd}")).unwrap();
        assert!(
            app.palette_view().unwrap().command_passthrough,
            "0-arg `:{zero_arg_cmd}` MUST trigger command passthrough"
        );
        app.press_keys("<Esc>").unwrap();
    }

    // 2. Verify all required static & dynamic Palette labels exist and zero `:cmd` shortcuts leak!
    app.press_keys(":").unwrap();
    let all_items = app.palette_view().unwrap().items;
    let labels: Vec<&str> = all_items.iter().map(|i| i.label.as_str()).collect();
    for expected in [
        "New Blank Project",
        "Clear Clipped Meters",
        "Reload Project Samples",
        "Reload External Presets",
        "Inspect Pattern Dependencies",
        "Import… (.rbs / Preset / MIDI / Take)",
        "Toggle Jam Mode",
        "Stage Live Changes",
        "Keep Focused Live Override",
        "Keep All Live Overrides",
        "Release Focused Lane Override",
        "Release All Lane Overrides",
        "New Placed Part",
        "Toggle Contextual Key Hints",
        "Default New-Project Scale",
        "Delete Instrument",
        "Rename Pattern",
        "Rename Part",
        "Rename Lane",
        "Rename Channel",
        "Rename Effect",
        "Open Part:",
        "Open Channel:",
        "Open Effect: WarmthDrive",
        "Open Lane:",
        "Open Modulation: LeadGainMod",
    ] {
        assert!(
            labels.iter().any(|l| l.contains(expected)),
            "Palette missing expected entry containing `{expected}`"
        );
    }
    for item in &all_items {
        assert!(
            !(item.shortcut.starts_with(':') && item.shortcut.len() > 1),
            "Palette entry `{}` must not advertise raw `:cmd` shortcut `{}`",
            item.label,
            item.shortcut
        );
    }
    app.press_keys("<Esc>").unwrap();

    // 3. Execute dynamic entries (`Open Effect`, `Open Channel`, `Open Part`, `Open Lane`, `Open Modulation`).
    app.press_keys(":Open Effect: WarmthDrive<Enter>").unwrap();
    assert!(matches!(app.detail, cadence_app::Detail::Effect(_)));

    app.press_keys(":Open Channel: Master<Enter>").unwrap();
    assert_eq!(app.detail, cadence_app::Detail::Channel(ChannelRef::Master));

    app.press_keys(":Open Modulation: LeadGainMod<Enter>")
        .unwrap();
    assert_eq!(app.mode_name(), "WORKFLOW");
    app.press_keys("<Esc>").unwrap();

    let first_part_name = app.project().parts[0].name.clone();
    app.press_keys(&format!(":Open Part: {first_part_name}<Enter>"))
        .unwrap();
    assert!(matches!(app.detail, cadence_app::Detail::Part(_)));

    let first_lane_name = app.project().lanes[0].name.clone();
    app.press_keys(&format!(":Open Lane: {first_lane_name}<Enter>"))
        .unwrap();
    assert_eq!(app.selected_lane, Some(app.project().lanes[0].id));
}
