//! Integration tests for external `InstrumentPreset` & `PresetCatalog` discovery,
//! unified Rack preset loading, `:zone preset`, Filter Groove bank browsing/forking/saving,
//! missing-asset resilience, and factory kit audio integrity.
use crate::common;

use cadence_app::{
    authoring::DraftField,
    workflow::{SoundChoice, WorkflowContext, WorkflowDraft, WorkflowKind, WorkflowPlan},
    Action, App, Detail, FileDialogRequest, KeyCode, KeyEvent, KeyModifiers,
};
use cadence_core::{
    assets::load_project_samples,
    filter_patterns::{CompiledFilterPattern, FilterGrooveCatalog},
    material::{
        Edit, InstrumentInstance, Lane, Part, Pattern, PatternKind, PhaseMode, Placement, Project,
    },
    mixer::{Channel, ChannelKind},
    music::Step,
    params::{ParamAddress, ZoneParam},
    presets::{InstrumentPreset, PresetCatalog},
    rack::{drum_midi, ModuleKind, RackPatch, RackTemplate},
    sampler::{load_sample_file, SampleLibrary},
};
use cadence_engine::audio::{render_measure_report, render_schedule_pcm};
#[cfg(feature = "rbs")]
use cadence_formats::rbs::import_rbs_bytes_with_presets;
use common::repo_root;
use std::path::PathBuf;

fn draft_keys(d: &mut WorkflowDraft, p: &Project, seq: &str) {
    for ch in seq.chars() {
        let code = if ch == '\n' {
            KeyCode::Enter
        } else {
            KeyCode::Char(ch)
        };
        let _ = d.handle_key(p, KeyEvent::new(code, KeyModifiers::NONE));
    }
}

#[cfg(feature = "rbs")]
fn minimal_synthetic_rbs_v42() -> Vec<u8> {
    common::minimal_rbs_bytes("EmptyCat", 128_000)
}

#[test]
fn empty_preset_dirs_and_missing_samples_never_crash_and_surface_clear_notices() {
    let dir = tempfile::tempdir().unwrap();
    let save_path = dir.path().join("project.json");
    let lib = SampleLibrary::from_roots_only(&[], Some(&save_path));
    let mut app = App::new_with_library(Project::empty(), save_path, lib).unwrap();
    app.set_preset_catalog(PresetCatalog::empty());

    // 1. WorkflowDraft with an empty PresetCatalog ALWAYS offers bare defaults first!
    let p = Project::empty();
    let mut ds = WorkflowDraft::new(
        WorkflowKind::Instrument,
        &p,
        WorkflowContext::default(),
        app.presets(),
    );
    draft_keys(&mut ds, &p, "InitOne\ns");
    assert_eq!(ds.view(&p).field, DraftField::Preset);
    assert_eq!(ds.view(&p).candidates[0].label, "Init Synth");
    draft_keys(&mut ds, &p, "\n\n");
    let WorkflowPlan::Instrument(spec) = ds.plan(&p).unwrap() else {
        panic!("expected instrument plan");
    };
    assert_eq!(spec.sound, SoundChoice::Template(RackTemplate::Synth));

    let mut dm = WorkflowDraft::new(
        WorkflowKind::Instrument,
        &p,
        WorkflowContext::default(),
        app.presets(),
    );
    draft_keys(&mut dm, &p, "BlankSamp\nm");
    assert_eq!(dm.view(&p).field, DraftField::Sample);
    assert_eq!(dm.view(&p).candidates[0].label, "Blank Sampler");
    draft_keys(&mut dm, &p, "s\n");
    let WorkflowPlan::Instrument(spec) = dm.plan(&p).unwrap() else {
        panic!("expected sampler plan");
    };
    assert_eq!(spec.sound, SoundChoice::Template(RackTemplate::Sampler));

    let mut dk = WorkflowDraft::new(
        WorkflowKind::Instrument,
        &p,
        WorkflowContext::default(),
        app.presets(),
    );
    draft_keys(&mut dk, &p, "BlankKit\nd");
    assert_eq!(dk.view(&p).field, DraftField::KitSource);
    assert_eq!(dk.view(&p).candidates[0].label, "Blank Kit");
    draft_keys(&mut dk, &p, "b\n");
    let WorkflowPlan::Instrument(spec) = dk.plan(&p).unwrap() else {
        panic!("expected kit plan");
    };
    assert_eq!(spec.sound, SoundChoice::Template(RackTemplate::Kit));

    let mut du = WorkflowDraft::new(
        WorkflowKind::Instrument,
        &p,
        WorkflowContext::default(),
        app.presets(),
    );
    draft_keys(&mut du, &p, "BlankMod\nu");
    assert_eq!(du.view(&p).field, DraftField::Preset);
    assert_eq!(du.view(&p).candidates[0].label, "Blank Modular");
    draft_keys(&mut du, &p, "\n\n");
    let WorkflowPlan::Instrument(spec) = du.plan(&p).unwrap() else {
        panic!("expected modular plan");
    };
    assert_eq!(spec.sound, SoundChoice::Template(RackTemplate::Modular));

    // 2. Ex commands create bare Synth / Sampler / Kit / Modular even with empty PresetCatalog!
    app.execute_command("instrument BareSynth synth").unwrap();
    app.execute_command("sampler BareSampler").unwrap();
    app.execute_command("kit BareKit").unwrap(); // falls back cleanly to Blank Kit!
    app.execute_command("instrument BareMod modular").unwrap();
    assert_eq!(app.project().instruments.len(), 4);

    // 3. Loading a Sampler Rack preset whose WAV file is missing NEVER crashes; records SampleIssue + status notice!
    let missing_preset = dir.path().join("missing-wav.preset.json");
    let mut ghost_rack = RackPatch::default_sampler();
    if let Some((_, _, smp)) = ghost_rack.sampler_modules_mut().next() {
        smp.path = Some("factory-samples/deleted/ghost.wav".into());
    }
    InstrumentPreset::new("GhostPad", Some("Pad".into()), ghost_rack)
        .save(&missing_preset)
        .unwrap();
    app.execute_command("instrument BareSampler").unwrap();
    app.execute_command(&format!("preset-load {}", missing_preset.display()))
        .expect("missing sample inside preset must load non-fatally");
    assert!(!app.sample_issues().is_empty());
    assert!(
        app.status.contains("missing sample"),
        "status should surface missing sample notice: {}",
        app.status
    );

    // 4. Missing filter_grooves.json falls back cleanly to a valid neutral 1-step pulse!
    let missing_grooves = FilterGrooveCatalog::load_from_roots(&[dir.path().to_path_buf()]);
    assert!(missing_grooves.is_empty());
    assert_eq!(
        missing_grooves.compiled(17),
        CompiledFilterPattern::neutral()
    );

    // 5. Importing an `.rbs` file with an empty PresetCatalog NEVER crashes and surfaces clear notices!
    #[cfg(feature = "rbs")]
    {
        let rbs_bytes = minimal_synthetic_rbs_v42();
        let (rbs_proj, summary) =
            import_rbs_bytes_with_presets(&rbs_bytes, "EmptyCat", &PresetCatalog::empty())
                .expect("RBS import must survive missing external presets");
        rbs_proj.validate().unwrap();
        rbs_proj.compile().unwrap();
        assert!(
            !summary.notices.is_empty(),
            "missing external presets during RBS import must produce clear notices"
        );
        assert!(
            summary.status_line().contains("missing"),
            "status_line must surface missing preset notice: {}",
            summary.status_line()
        );

        // Same path via App::apply(Action::ImportRbs)
        let rbs_path = dir.path().join("empty_cat.rbs");
        std::fs::write(&rbs_path, &rbs_bytes).unwrap();
        app.apply(Action::ImportRbs { path: rbs_path })
            .expect("Action::ImportRbs must survive empty PresetCatalog");
        assert!(
            app.status.contains("missing"),
            "app.status after RBS import with empty presets: {}",
            app.status
        );
    }
}

#[test]
fn unified_instrument_preset_save_load_and_zone_preset() {
    let dir = tempfile::tempdir().unwrap();
    let save_path = dir.path().join("project.json");
    let lib = common::factory_sample_library(Some(&save_path), false);
    let mut app = App::new_with_library(Project::empty(), save_path, lib).unwrap();

    // Create three Rack instruments: Synth preset, Sampler instrument, and multi-zone Kit preset.
    app.execute_command("instrument AcidLead acid-303").unwrap();
    let synth_id = app.selected_instrument.unwrap();
    app.execute_command("sampler ClavSamp factory-samples/tr-808/15-cl.wav")
        .unwrap();
    let sampler_id = app.selected_instrument.unwrap();
    app.execute_command("kit Kit808 tr-808").unwrap();
    let kit_id = app.selected_instrument.unwrap();

    // 1. Save all three as unified InstrumentPreset JSON files.
    let synth_path = dir.path().join("acid.preset.json");
    let sampler_path = dir.path().join("clav.preset.json");
    let kit_path = dir.path().join("kit.preset.json");

    app.execute_command("instrument AcidLead").unwrap();
    app.execute_command(&format!("preset-save {}", synth_path.display()))
        .unwrap();
    app.execute_command("instrument ClavSamp").unwrap();
    app.execute_command(&format!("preset-save {}", sampler_path.display()))
        .unwrap();
    app.execute_command("instrument Kit808").unwrap();
    app.execute_command(&format!("preset-save {}", kit_path.display()))
        .unwrap();

    let loaded_synth = InstrumentPreset::load(&synth_path).unwrap();
    assert_eq!(loaded_synth.patch.zones.len(), 1);
    assert!(!loaded_synth.patch.has_samplers());
    let loaded_sampler = InstrumentPreset::load(&sampler_path).unwrap();
    assert_eq!(loaded_sampler.patch.zones.len(), 1);
    assert!(loaded_sampler.patch.has_samplers());
    let loaded_kit = InstrumentPreset::load(&kit_path).unwrap();
    assert_eq!(loaded_kit.patch.zones.len(), 16);
    assert!(loaded_kit.patch.has_samplers());

    // 2. Loading via `:preset`, `Action::LoadPreset`, and `FileHub` succeeds uniformly across all Racks!
    app.execute_command("instrument AcidLead").unwrap();
    app.execute_command("preset pulse-lead").unwrap();
    app.apply_file_dialog_result(FileDialogRequest::ImportAsset, synth_path.clone())
        .unwrap();

    app.execute_command("instrument ClavSamp").unwrap();
    app.execute_command("preset tr-808-ma").unwrap();
    app.apply_file_dialog_result(FileDialogRequest::ImportAsset, sampler_path.clone())
        .unwrap();

    app.execute_command("instrument Kit808").unwrap();
    app.execute_command("preset tr-909").unwrap();
    app.apply_file_dialog_result(FileDialogRequest::ImportAsset, kit_path.clone())
        .unwrap();

    // 3. Structural invariants: unknown preset names, invalid empty-zone Racks, and duplicate
    //    `Edit::CreateInstrument` IDs are all strictly rejected!
    assert!(app
        .apply(Action::LoadPreset {
            instrument: synth_id,
            preset: "nonexistent-preset-404".into(),
        })
        .is_err());
    let _ = sampler_id;
    let synth_ch = app.instrument(synth_id).unwrap().channel;
    assert!(app
        .apply(Action::Edit {
            edit: Edit::UpdateRackPatch {
                id: synth_id,
                rack: RackPatch {
                    zones: Vec::new(),
                    ..RackPatch::default_synth()
                },
            },
        })
        .is_err());
    assert!(app
        .apply(Action::Edit {
            edit: Edit::CreateInstrument {
                instrument: InstrumentInstance {
                    id: synth_id,
                    name: "IllegalDuplicateId".into(),
                    channel: synth_ch,
                    rack: RackPatch::default_sampler(),
                },
            },
        })
        .is_err());

    // 4. `:zone preset <NAME|PATH>` loads single-zone Synth & Sampler presets into a Kit zone,
    //    and strictly rejects multi-zone Kit presets!
    app.execute_command("instrument Kit808").unwrap();
    app.execute_command("zone select 1").unwrap();
    app.execute_command("zone preset analog-kick").unwrap();
    {
        let kit = &app.instrument(kit_id).unwrap().rack;
        assert!(
            kit.zones[0]
                .modules
                .iter()
                .any(|m| matches!(m.kind, ModuleKind::Vco(_))),
            "loading analog-kick into zone 0 must install its VCO synth graph"
        );
        assert!(
            !kit.zones[0]
                .modules
                .iter()
                .any(|m| matches!(m.kind, ModuleKind::Sampler(_))),
            "analog-kick zone should have no Sampler module"
        );
    }
    app.execute_command(&format!("zone preset {}", sampler_path.display()))
        .unwrap();
    {
        let kit = &app.instrument(kit_id).unwrap().rack;
        let sp = kit.zones[0]
            .modules
            .iter()
            .find_map(|m| match &m.kind {
                ModuleKind::Sampler(sp) => Some(sp),
                _ => None,
            })
            .expect("expected Sampler module after loading Sampler preset");
        assert_eq!(
            sp.root_midi, kit.zones[0].root_midi,
            "single-note drum pad root_midi must align on :zone preset"
        );
    }
    assert!(
        app.execute_command("zone preset tr-909").is_err(),
        ":zone preset must reject loading a multi-zone Kit preset into a single zone"
    );
}

#[test]
fn factory_sources_presets_samples_metadata_and_24khz_kit_audition_are_valid() {
    let root = repo_root();

    // 1. All 3 provenance source projects load and pass Project::validate()!
    for kit_dir in ["tr-808", "tr-909", "edm-kit"] {
        let path = root
            .join("factory-sources")
            .join(kit_dir)
            .join("project.json");
        let (proj, _flags) = cadence_app::load_document(&path)
            .unwrap_or_else(|e| panic!("failed to load {}: {e:#}", path.display()));
        proj.validate()
            .unwrap_or_else(|e| panic!("invalid source project {}: {e:#}", path.display()));
        proj.compile()
            .unwrap_or_else(|e| panic!("failed to compile {}: {e:#}", path.display()));
    }

    // 2. External PresetCatalog scans ./presets cleanly with ZERO warnings and finds all expected entries!
    let catalog = PresetCatalog::from_roots(&[root.join("presets")]);
    assert!(
        catalog.warnings().is_empty(),
        "factory presets/ must have zero invalid files: {:?}",
        catalog.warnings()
    );
    for id in [
        "acid-303",
        "sub-bass",
        "reese",
        "pulse-lead",
        "pluck",
        "brass-pad",
        "warm-pad",
        "kick",
        "snare",
        "closed-hat",
        "open-hat",
    ] {
        let entry = catalog
            .find(id)
            .unwrap_or_else(|| panic!("missing factory Synth preset '{id}'"));
        assert_eq!(entry.patch.zones.len(), 1, "preset '{id}' must be 1-zone");
        assert!(
            !entry.patch.has_samplers(),
            "synth preset '{id}' should be pure synthesis"
        );
    }
    for id in ["tr-808", "tr-909", "edm-kit"] {
        let entry = catalog
            .find_by_category("Kit", id)
            .unwrap_or_else(|| panic!("missing factory Kit preset '{id}'"));
        assert!(
            entry.patch.zones.len() >= 11 && entry.patch.has_samplers(),
            "kit preset '{id}' must be a multi-zone sample kit"
        );
    }
    for (id, expected_zones) in [
        ("tr-808-synth", 16usize),
        ("tr-909-synth", 11),
        ("edm-kit-synth", 11),
    ] {
        let entry = catalog
            .find_by_category("Kit", id)
            .unwrap_or_else(|| panic!("missing factory live synth Kit preset '{id}'"));
        assert_eq!(
            entry.patch.zones.len(),
            expected_zones,
            "live synth kit '{id}' zone count mismatch"
        );
        assert!(
            !entry.patch.has_samplers(),
            "live synth kit '{id}' must be pure modular synthesis"
        );
        assert_eq!(
            entry.patch.panel_pins.len(),
            16,
            "live synth kit '{id}' should carry 16 curated panel_pins"
        );
        let ch = entry
            .patch
            .zones
            .iter()
            .find(|z| z.root_midi == drum_midi::CH)
            .expect("CH zone");
        let oh = entry
            .patch
            .zones
            .iter()
            .find(|z| z.root_midi == drum_midi::OH)
            .expect("OH zone");
        assert_eq!(ch.choke_group, Some(1));
        assert_eq!(oh.choke_group, Some(1));
    }
    let tr808_voices = [
        "bd", "sd", "lt", "mt", "ht", "rs", "ch", "cp", "cb", "cy", "oh", "lc", "mc", "hc", "cl",
        "ma",
    ];
    let eleven_voices = [
        "bd", "sd", "lt", "mt", "ht", "rs", "ch", "cp", "cr", "rd", "oh",
    ];
    for (prefix, voices) in [
        ("tr-808", tr808_voices.as_slice()),
        ("tr-909", eleven_voices.as_slice()),
        ("edm-kit", eleven_voices.as_slice()),
    ] {
        for voice in voices {
            let id = format!("{prefix}-{voice}");
            let entry = catalog
                .find_by_category("Drum", &id)
                .unwrap_or_else(|| panic!("missing generated modular Drum preset '{id}'"));
            assert_eq!(entry.patch.zones.len(), 1, "'{id}' must be 1-zone");
            assert_eq!(entry.patch.zones[0].low_midi, 0);
            assert_eq!(entry.patch.zones[0].high_midi, 127);
            assert!(
                !entry.patch.has_samplers(),
                "generated drum preset '{id}' must be pure modular synthesis"
            );
            assert!(
                !entry.patch.panel_pins.is_empty(),
                "generated drum preset '{id}' must preserve curated panel_pins"
            );
        }
    }

    // 3. External clean-room presets/filter_grooves.json loads all 24 original grooves cleanly!
    let grooves = FilterGrooveCatalog::load_from_roots(&[root.join("presets")]);
    assert_eq!(grooves.len(), 24);
    assert_eq!(grooves.compiled(54), CompiledFilterPattern::neutral());

    // 4. Every rendered factory sample decodes cleanly with embedded RIFF inst + smpl metadata!
    for kit_id in ["tr-808", "tr-909", "edm-kit"] {
        let entry = catalog.find_by_category("Kit", kit_id).unwrap();
        let kit = &entry.patch;
        for zone in &kit.zones {
            let sp = zone
                .modules
                .iter()
                .find_map(|m| match &m.kind {
                    ModuleKind::Sampler(sp) => Some(sp),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{kit_id} zone '{}' should have a Sampler", zone.name));
            let rel = sp.path.as_deref().expect("zone sample path");
            assert!(
                rel.starts_with("factory-samples/"),
                "factory kit locator should start with factory-samples/: {rel}"
            );
            let abs = root.join(rel);
            let sample = load_sample_file(&abs, Some(rel.to_string())).unwrap();
            assert!(sample.frames > 500, "{rel} too short ({})", sample.frames);
            assert!(sample.meta.has_root, "{rel} missing RIFF smpl/inst root");
            assert_eq!(sample.meta.root_midi, zone.root_midi, "{rel} root mismatch");
            assert_eq!(
                sample.meta.low_midi,
                Some(zone.low_midi),
                "{rel} low_midi mismatch"
            );
            assert_eq!(
                sample.meta.high_midi,
                Some(zone.high_midi),
                "{rel} high_midi mismatch"
            );
            let peak = sample
                .left
                .iter()
                .chain(sample.right.as_deref().unwrap_or(&[]).iter())
                .fold(0.0_f32, |m, &s| m.max(s.abs()));
            assert!(
                peak > 0.20 && peak <= 1.0,
                "{rel} peak {peak} out of clean range"
            );
        }
    }

    // 5. 24 kHz audition sanity check: render baked + live synth TR-808, TR-909, and EDM Kit beats
    //    in parallel, verifying hat choke, zero clips, sample integrity, and stereo zone panning!
    let mut lib = common::factory_sample_library(None, false);
    let kit_ids = [
        "tr-808",
        "tr-909",
        "edm-kit",
        "tr-808-synth",
        "tr-909-synth",
        "edm-kit-synth",
    ];
    let mut kit_inputs = Vec::with_capacity(kit_ids.len());
    let mut kit_banks: std::collections::HashMap<
        &str,
        (
            cadence_core::score::ScoreWindow,
            cadence_core::runtime::SampleBank,
        ),
    > = std::collections::HashMap::new();
    for kit_id in kit_ids {
        let rack = catalog
            .find_by_category("Kit", kit_id)
            .unwrap()
            .patch
            .clone();
        let is_sample_kit = rack.has_samplers();
        let mut p = Project::empty();
        p.bpm = 126.0;
        p.bars = 1;
        p.root_midi = drum_midi::BD; // 36
        p.scale = (0..12).collect(); // chromatic degrees -> direct pad mapping
        p.mixer
            .channels
            .push(Channel::new(1, ChannelKind::Audio, "Drums"));
        p.instruments.push(InstrumentInstance {
            id: 1,
            name: kit_id.into(),
            channel: 1,
            rack,
        });
        let hit = |deg: i16, vel: f32| Step {
            active: true,
            degree: deg,
            velocity: vel,
            gate: 1.0,
            ..Step::default()
        };
        let mut steps = vec![Step::default(); 16];
        // Four-on-the-floor BD (36->1), SD (37->2), CH (42->7), CP (43->8), CB/CR (44->9), OH (46->11)
        steps[0] = hit(1, 0.95); // BD
        steps[2] = hit(11, 0.85); // OH
        steps[3] = hit(7, 0.85); // CH chokes OH!
        steps[4] = hit(2, 0.92); // SD
        steps[6] = hit(7, 0.80); // CH
        steps[8] = hit(1, 0.95); // BD
        steps[10] = hit(9, 0.80); // CB / CR
        steps[12] = hit(8, 0.92); // CP
        steps[14] = hit(11, 0.82); // OH
        steps[15] = hit(7, 0.85); // CH
        p.patterns.push(Pattern {
            id: 1,
            name: "Beat".into(),
            kind: PatternKind::Notes { steps },
        });
        p.parts.push(Part::new(1, "Groove", 1, 1));
        let mut lane = Lane::new(1, "Drums");
        lane.placements.push(Placement {
            id: 1,
            part: 1,
            start_step: 0,
            length_steps: 16,
            phase: PhaseMode::Song,
            offset_steps: 0,
            exceptions: Vec::new(),
        });
        p.lanes.push(lane);

        let score = p.compile().unwrap();
        let loaded = load_project_samples(&score, &mut lib);
        assert!(
            loaded.issues.is_empty(),
            "{kit_id} had sample issues: {:?}",
            loaded.issues
        );
        let cp_zone_id = p.instruments[0]
            .rack
            .zones
            .iter()
            .find(|z| z.low_midi <= drum_midi::CP && drum_midi::CP <= z.high_midi)
            .expect("CP zone")
            .id;
        let mut panned = p;
        panned
            .apply(Edit::SetParam {
                address: ParamAddress::Zone {
                    instrument: 1,
                    zone: cp_zone_id,
                    param: ZoneParam::Pan,
                },
                value: 0.65,
            })
            .unwrap();
        let panned_score = panned.compile().unwrap();
        kit_banks.insert(kit_id, (score.clone(), loaded.bank.clone()));
        kit_inputs.push((kit_id, is_sample_kit, score, panned_score, loaded.bank));
    }

    std::thread::scope(|s| {
        let handles: Vec<_> = kit_inputs
            .into_iter()
            .map(|(kit_id, is_sample_kit, score, panned_score, bank)| {
                s.spawn(move || {
                    let (report, pcm) =
                        render_schedule_pcm(&[(0, &score)], 0, 16, bank.clone(), 24_000).unwrap();
                    let peak = report.output.peak[0].max(report.output.peak[1]);
                    let rms = report.output.rms[0].max(report.output.rms[1]);
                    assert_eq!(
                        report.output.full_scale_samples, 0,
                        "{kit_id} audition groove clipped!"
                    );
                    assert!(
                        peak > 0.25,
                        "{kit_id} audition groove peak too quiet: {:?}",
                        report.output.peak
                    );
                    assert!(
                        rms > 0.03,
                        "{kit_id} audition groove RMS too quiet: {:?}",
                        report.output.rms
                    );
                    assert!(
                        report.output.dc[0].abs() < 0.005 && report.output.dc[1].abs() < 0.005,
                        "{kit_id} DC offset too high: {:?}",
                        report.output.dc
                    );

                    // Truthful mono/stereo verification:
                    // - If the kit's on-disk samples include true stereo L/R differences, playback must preserve them.
                    // - If a dry hardware kit uses pure dry mono samples with centered zones, playback must stay clean balanced mono.
                    let bank_has_stereo_content = bank.iter().any(|(_, s)| {
                        s.right.as_deref().is_some_and(|r| {
                            s.left
                                .iter()
                                .zip(r.iter())
                                .any(|(&l, &rr)| (l - rr).abs() > 1e-4)
                        })
                    });
                    let max_lr_diff = pcm
                        .iter()
                        .map(|&[l, r]| (l as i32 - r as i32).abs())
                        .max()
                        .unwrap_or(0);
                    if is_sample_kit {
                        if bank_has_stereo_content {
                            assert!(
                                max_lr_diff > 0,
                                "{kit_id} contains stereo sample data that must survive stereo playback"
                            );
                        } else {
                            assert!(
                                max_lr_diff <= 1,
                                "{kit_id} dry centered mono kit should remain balanced mono (max_lr_diff={max_lr_diff})"
                            );
                        }
                    } else {
                        assert!(
                            bank.is_empty(),
                            "live synth kit '{kit_id}' must require zero external WAV samples"
                        );
                    }

                    // Verify stereo kit image via per-zone panning (works identically for dry mono AND stereo kits!).
                    let (panned_report, panned_pcm) =
                        render_schedule_pcm(&[(0, &panned_score)], 0, 16, bank, 24_000).unwrap();
                    assert_eq!(
                        panned_report.output.full_scale_samples, 0,
                        "{kit_id} panned stereo groove clipped!"
                    );
                    assert!(
                        panned_pcm
                            .iter()
                            .any(|&[l, r]| (l as i32 - r as i32).abs() > 500),
                        "{kit_id} panned CP zone must produce genuine stereo L/R width"
                    );
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
    });

    // Live synth kits must match their baked sample kits 1:1 across 44.1 kHz and 48 kHz,
    // and noise/resonator synth grooves must remain sample-rate invariant up to 96 kHz.
    let parity_pairs: &[(&str, &str)] = if cfg!(debug_assertions) {
        &[("tr-808", "tr-808-synth")]
    } else {
        &[
            ("tr-808", "tr-808-synth"),
            ("tr-909", "tr-909-synth"),
            ("edm-kit", "edm-kit-synth"),
        ]
    };
    let kit_banks = &kit_banks;
    std::thread::scope(|s| {
        let handles: Vec<_> = parity_pairs
            .iter()
            .map(|&(baked_id, synth_id)| {
                s.spawn(move || {
                    let render_at = |id: &str, sr: u32| -> (f64, f64) {
                        let (score, bank) = &kit_banks[id];
                        let rep = render_measure_report(score, bank.clone(), 1, sr).unwrap();
                        (
                            rep.output.peak[0].max(rep.output.peak[1]),
                            rep.output.rms[0].max(rep.output.rms[1]),
                        )
                    };
                    let mut s44_rms = 0.0_f64;
                    for sr in [44_100u32, 48_000] {
                        let (b_peak, b_rms) = render_at(baked_id, sr);
                        let (s_peak, s_rms) = render_at(synth_id, sr);
                        if sr == 44_100 {
                            s44_rms = s_rms;
                        }
                        let rms_diff_db = cadence_core::dsp::lin_to_db((s_rms / b_rms) as f32).abs();
                        let peak_diff_db = cadence_core::dsp::lin_to_db((s_peak / b_peak) as f32).abs();
                        assert!(
                            rms_diff_db < 0.6 && peak_diff_db < 1.5,
                            "{synth_id} must match {baked_id} playback level @ {sr} Hz (baked peak={b_peak:.4}/rms={b_rms:.4}, synth peak={s_peak:.4}/rms={s_rms:.4}, dRMS={rms_diff_db:.2} dB, dPeak={peak_diff_db:.2} dB)"
                        );
                    }
                    let (_, s96_rms) = render_at(synth_id, 96_000);
                    let sr_drift_db = cadence_core::dsp::lin_to_db((s96_rms / s44_rms) as f32).abs();
                    assert!(
                        sr_drift_db < 0.6,
                        "{synth_id} groove RMS drifted between 44.1 kHz and 96 kHz: {sr_drift_db:.2} dB"
                    );
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
    });

    // Keep bare RackPatch default verified cleanly.
    RackPatch::default_synth().validate().unwrap();
}

#[test]
fn filter_groove_bank_stepping_browser_fork_and_save_end_to_end() {
    use cadence_app::MemoryHostIo;
    use cadence_core::filter_patterns::FilterEnvPreset;
    use cadence_core::mixer::{EffectDevice, EffectKind, EffectParam, FilterPatch};
    use std::sync::Arc;

    let root = repo_root();
    let factory_bytes = std::fs::read(root.join("presets/filter_grooves.json")).unwrap();
    let factory_cat = FilterGrooveCatalog::parse_bytes(&factory_bytes).unwrap();
    assert_eq!(factory_cat.len(), 24);

    // 1. Disk-free MemoryHostIo workflow: Browser, search, live preview, Esc cancel, Enter commit,
    //    bank-aware stepping, Fork -> Pattern round-trip, and Save -> filter_grooves.json + reload!
    let mem_io = Arc::new(MemoryHostIo::new("/virtual"));
    mem_io.set_filter_grooves(factory_cat.clone());

    let mut project = Project::empty();
    let filter_id = project.max_id() + 1;
    project.mixer.master.inserts.push(EffectDevice {
        id: filter_id,
        name: "Rhythmic Filter".into(),
        bypassed: false,
        kind: EffectKind::Filter(FilterPatch {
            env_pattern: 8,
            env_pattern_id: None,
            env_amount: 2.5,
            env_decay_ms: 200.0,
            ..FilterPatch::default()
        }),
    });
    let mut app = App::new_with_host_io(
        project,
        PathBuf::from("/virtual/project.json"),
        Arc::clone(&mem_io) as Arc<dyn cadence_app::HostIo>,
    )
    .unwrap();
    assert_eq!(app.filter_grooves().len(), 24);
    assert_eq!(
        app.audio_project().mixer.resolve_filter_groove(8),
        factory_cat.compiled(8)
    );
    assert_eq!(
        app.audio_project().mixer.resolve_filter_groove(54),
        CompiledFilterPattern::neutral()
    );

    // Open Filter Effect deck and test bank-aware stepping (never lands on empty slot 24!).
    app.execute_command(&format!("fx {filter_id}")).unwrap();
    // Move to EnvPattern control and step via ParamAddress::Effect nudging (or :param env_pattern 23 + EDIT l).
    app.execute_command("param env_pattern 23").unwrap();
    let rev_before_browser = app.revision();
    let fp_at_23 = match app.effect(filter_id).unwrap().1.kind {
        EffectKind::Filter(fp) => fp,
        _ => unreachable!(),
    };
    assert_eq!(fp_at_23.env_pattern, 23);

    // In EDIT mode on Pattern control, pressing `l` at 23 stays at 23 (does not step into empty 24!).
    let pat_ctrl_idx = app
        .control_descriptors()
        .iter()
        .position(|c| {
            matches!(
                c.target,
                cadence_app::ControlTarget::Param(cadence_core::params::ParamAddress::Effect {
                    param: EffectParam::EnvPattern,
                    ..
                })
            )
        })
        .expect("EnvPattern control");
    app.control_cursor = pat_ctrl_idx;
    app.press_keys("<m-l>").unwrap();
    let fp_after_step = match app.effect(filter_id).unwrap().1.kind {
        EffectKind::Filter(fp) => fp,
        _ => unreachable!(),
    };
    assert_eq!(
        fp_after_step.env_pattern, 23,
        "24-groove factory bank must stop at slot 23 and never land on empty slot 24"
    );

    // 2. Open Filter Groove Browser via `Enter` on `env_pattern`: live preview + `/` search + `Esc` zero-undo restore!
    let rev_before = app.revision();
    let dirty_before = app.dirty();
    app.press_keys("<Enter>").unwrap();
    assert!(
        app.groove_browser_view().is_some(),
        "Enter on env_pattern must open Filter Groove Browser"
    );
    // Search for "velvet" -> matches "12 Velvet Arch", previews live!
    app.press_keys("/velvet<Enter>").unwrap();
    let gb_view = app.groove_browser_view().unwrap();
    assert_eq!(gb_view.items.len(), 1);
    assert_eq!(gb_view.items[0].id, 12);
    assert_eq!(gb_view.live_preview_name.as_deref(), Some("12 Velvet Arch"));
    // Live audio score already reflects previewed groove 12 without bumping revision!
    let preview_fp = match app.audio_project().mixer.master.inserts[0].kind {
        EffectKind::Filter(fp) => fp,
        _ => unreachable!(),
    };
    assert_eq!(preview_fp.env_pattern, 12);
    assert_eq!(app.revision(), rev_before);

    // Esc #1 clears search filter; Esc #2 cancels browser and restores slot 23 with ZERO undo trace!
    app.press_keys("<Esc><Esc>").unwrap();
    assert!(app.groove_browser_view().is_none());
    assert_eq!(app.revision(), rev_before);
    assert_eq!(app.dirty(), dirty_before);
    let restored_fp = match app.effect(filter_id).unwrap().1.kind {
        EffectKind::Filter(fp) => fp,
        _ => unreachable!(),
    };
    assert_eq!(restored_fp.env_pattern, 23);

    // 3. Reopen via `P`, search for 32nd-note groove "shutter" (slot 16), press Enter -> single undoable commit!
    app.press_keys("P/shutter<Enter><Enter>").unwrap();
    assert!(app.groove_browser_view().is_none());
    assert_eq!(app.revision(), rev_before + 1);
    let committed_fp = match app.effect(filter_id).unwrap().1.kind {
        EffectKind::Filter(fp) => fp,
        _ => unreachable!(),
    };
    assert_eq!(committed_fp.env_pattern, 16);
    assert!(committed_fp.env_pattern_id.is_none());

    // Single Undo restores slot 23; Redo returns to slot 16!
    app.apply(Action::Undo).unwrap();
    assert_eq!(
        match app.effect(filter_id).unwrap().1.kind {
            EffectKind::Filter(fp) => fp.env_pattern,
            _ => unreachable!(),
        },
        23
    );
    app.apply(Action::Redo).unwrap();
    assert_eq!(
        match app.effect(filter_id).unwrap().1.kind {
            EffectKind::Filter(fp) => fp.env_pattern,
            _ => unreachable!(),
        },
        16
    );
    let _ = rev_before_browser;

    // 4. Fork bank groove 16 (`16 Trance Shutter`, res=32, len=16) -> editable project Pattern!
    //    Verify single atomic undo step AND bit-identical compile_filter_pattern round-trip!
    let rev_before_fork = app.revision();
    app.press_keys("f").unwrap();
    assert_eq!(app.revision(), rev_before_fork + 1);
    let forked_fp = match app.effect(filter_id).unwrap().1.kind {
        EffectKind::Filter(fp) => fp,
        _ => unreachable!(),
    };
    let bound_pat_id = forked_fp
        .env_pattern_id
        .expect("Fork must bind Filter.env_pattern_id");
    let round_tripped = app
        .project()
        .compile_filter_pattern(bound_pat_id)
        .expect("Forked pattern must compile cleanly");
    assert_eq!(
        round_tripped,
        factory_cat.compiled(16),
        "Fork -> compile_filter_pattern must preserve exact 32nd-note timing & steps"
    );
    // Single Undo cleanly removes forked pattern(s) and unbinds env_pattern_id!
    app.apply(Action::Undo).unwrap();
    assert!(app.project().patterns.is_empty());
    assert!(match app.effect(filter_id).unwrap().1.kind {
        EffectKind::Filter(fp) => fp.env_pattern_id.is_none(),
        _ => unreachable!(),
    });
    app.apply(Action::Redo).unwrap();

    // 5. Save Filter Groove to Bank via Palette (`Save Filter Groove to Bank`) -> unified ValueInput -> atomic MemoryHostIo write ->
    //    preserves all 24 factory grooves + adds slot 24 -> reloads catalog & refreshes RT audio snapshot!
    app.press_keys(":save filter groove<Enter><C-u>Neon Shutter<Enter>")
        .unwrap();
    let saved_bytes = mem_io
        .read_file("/virtual/presets/filter_grooves.json")
        .expect("filter_grooves.json written atomically via HostIo");
    let parsed_bank = FilterGrooveCatalog::parse_bytes(&saved_bytes).unwrap();
    assert_eq!(parsed_bank.len(), 25, "must preserve 24 factory + 1 custom");
    assert_eq!(parsed_bank.preset(24).name, "24 Neon Shutter");
    assert_eq!(parsed_bank.preset(24).category, "Custom");
    assert_eq!(parsed_bank.compiled(24), factory_cat.compiled(16));
    assert_eq!(app.filter_grooves().len(), 25);
    assert_eq!(
        app.audio_project().mixer.resolve_filter_groove(24),
        factory_cat.compiled(16),
        "RT Mixer::filter_grooves snapshot must refresh immediately after saving"
    );
    let selected_fp = match app.effect(filter_id).unwrap().1.kind {
        EffectKind::Filter(fp) => fp,
        _ => unreachable!(),
    };
    assert_eq!(selected_fp.env_pattern, 24);
    assert!(selected_fp.env_pattern_id.is_none());

    // Undo after Save restores the bound custom pattern cleanly while keeping bank file intact!
    app.apply(Action::Undo).unwrap();
    let undone_fp = match app.effect(filter_id).unwrap().1.kind {
        EffectKind::Filter(fp) => fp,
        _ => unreachable!(),
    };
    assert_eq!(undone_fp.env_pattern_id, Some(bound_pat_id));
    assert!(mem_io
        .read_file("/virtual/presets/filter_grooves.json")
        .is_some());

    // 6. Real-disk override precedence test with a 55-slot external user bank!
    let dir = tempfile::tempdir().unwrap();
    let project_presets = dir.path().join("presets");
    let custom_55: Vec<FilterEnvPreset> = (0u8..55)
        .map(|id| FilterEnvPreset {
            id,
            name: format!("{id:02} External Bank"),
            category: "External".into(),
            resolution: 16,
            length: 4,
            smooth_attack: false,
            steps: vec![127, 0, (id * 2).min(127), 0],
        })
        .collect();
    FilterGrooveCatalog::save_file(&project_presets.join("filter_grooves.json"), &custom_55)
        .unwrap();

    let mut disk_proj = Project::empty();
    let disk_flt = disk_proj.max_id() + 1;
    disk_proj.mixer.master.inserts.push(EffectDevice {
        id: disk_flt,
        name: "External Filter".into(),
        bypassed: false,
        kind: EffectKind::Filter(FilterPatch {
            env_pattern: 53,
            env_pattern_id: None,
            env_amount: 2.0,
            ..FilterPatch::default()
        }),
    });
    let song_path = dir.path().join("song.json");
    let disk_lib = cadence_core::sampler::SampleLibrary::from_roots(&[], Some(&song_path));
    let mut disk_app = App::new_with_library(disk_proj, song_path, disk_lib).unwrap();
    assert_eq!(
        disk_app.filter_grooves().len(),
        55,
        "project-local 55-slot bank must override 24-slot factory bank"
    );
    disk_app.execute_command(&format!("fx {disk_flt}")).unwrap();
    let pat_idx = disk_app
        .control_descriptors()
        .iter()
        .position(|c| {
            matches!(
                c.target,
                cadence_app::ControlTarget::Param(cadence_core::params::ParamAddress::Effect {
                    param: EffectParam::EnvPattern,
                    ..
                })
            )
        })
        .unwrap();
    disk_app.control_cursor = pat_idx;
    disk_app.press_keys("<m-l><m-l>").unwrap();
    let stepped_55 = match disk_app.effect(disk_flt).unwrap().1.kind {
        EffectKind::Filter(fp) => fp.env_pattern,
        _ => unreachable!(),
    };
    assert_eq!(
        stepped_55, 54,
        "55-slot external bank must step 53 -> 54 and clamp cleanly at 54"
    );
    assert_eq!(
        disk_app
            .audio_project()
            .mixer
            .resolve_filter_groove(54)
            .active_steps(),
        &[127, 0, 108, 0]
    );

    // 7. Action Palette discovers and launches Browse / Fork / Save Filter Groove from any deck!
    disk_app
        .press_keys("<Esc>:Browse Filter Grooves<Enter>")
        .unwrap();
    assert!(
        disk_app.groove_browser_view().is_some(),
        "Action Palette 'Browse Filter Grooves' must open Groove Browser"
    );
    disk_app.press_keys("<Esc>").unwrap();
    assert!(disk_app.groove_browser_view().is_none());

    disk_app
        .press_keys(":Fork Filter Groove to Pattern<Enter>")
        .unwrap();
    assert_eq!(disk_app.project().patterns.len(), 1);
    disk_app
        .press_keys(":Save Filter Groove to Bank<Enter>")
        .unwrap();
    assert!(
        disk_app.value_input_view().is_some(),
        "Action Palette 'Save Filter Groove to Bank' must open unified ValueInput prompt"
    );
    disk_app.press_keys("<Esc>").unwrap();
}

#[test]
fn filter_groove_browser_preserves_clean_dirty_flag_and_cancels_on_alt_jam_and_keep() {
    use cadence_app::MemoryHostIo;
    use cadence_core::mixer::{EffectDevice, EffectKind, FilterPatch};
    use std::sync::Arc;

    let root = repo_root();
    let factory_bytes = std::fs::read(root.join("presets/filter_grooves.json")).unwrap();
    let factory_cat = FilterGrooveCatalog::parse_bytes(&factory_bytes).unwrap();
    let mem_io = Arc::new(MemoryHostIo::new("/virtual"));
    mem_io.set_filter_grooves(factory_cat);

    let mut project = Project::empty();
    let filter_id = project.max_id() + 1;
    project.mixer.master.inserts.push(EffectDevice {
        id: filter_id,
        name: "Rhythmic Filter".into(),
        bypassed: false,
        kind: EffectKind::Filter(FilterPatch {
            env_pattern: 8,
            env_pattern_id: None,
            env_amount: 2.0,
            ..FilterPatch::default()
        }),
    });
    let mut app = App::new_with_host_io(
        project,
        PathBuf::from("/virtual/project.json"),
        Arc::clone(&mem_io) as Arc<dyn cadence_app::HostIo>,
    )
    .unwrap();
    app.execute_command(&format!("fx {filter_id}")).unwrap();
    assert!(!app.dirty(), "fresh project must start clean");

    // 1. Live preview on a clean document must NOT mark App::dirty() (no false `[+]` badge).
    app.press_keys("Pj").unwrap();
    assert!(app.groove_browser_view().is_some());
    let preview_slot = match app.audio_project().mixer.master.inserts[0].kind {
        EffectKind::Filter(fp) => fp.env_pattern,
        _ => unreachable!(),
    };
    assert_eq!(preview_slot, 9, "moving down should live-preview slot 9");
    assert!(
        !app.dirty(),
        "non-destructive Groove Browser live preview must preserve clean dirty() == false"
    );

    // 2. Ctrl+Shift+j (enter Jam) during Groove Browser preview must cancel preview cleanly, restore
    //    slot 8, clear groove_browser state, and enter Jam without leaking slot 9.
    app.handle_key(KeyEvent::new(
        KeyCode::Char('J'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    ))
    .unwrap();
    assert!(
        app.groove_browser_view().is_none(),
        "Ctrl+Shift+j must close Groove Browser"
    );
    assert!(app.is_jam(), "Ctrl+Shift+j must enter Jam mode");
    let after_alt_j = match app.effect(filter_id).unwrap().1.kind {
        EffectKind::Filter(fp) => fp.env_pattern,
        _ => unreachable!(),
    };
    assert_eq!(
        after_alt_j, 8,
        "Alt-j must restore pre-preview groove slot 8 instead of leaking slot 9"
    );
    assert!(!app.dirty(), "document remains clean after Alt-j cancel");

    // 3. Exit Jam, make a real post-Jam edit, and trigger request_quit(): because groove_browser
    //    was cleanly cleared on Alt-j, request_quit() must NEVER overwrite the new edit with
    //    a stale pre-Jam snapshot!
    app.press_keys("<Esc>").unwrap();
    assert!(!app.is_jam());
    app.execute_command("param env_pattern 15").unwrap();
    assert!(app.dirty());
    let quit_out = app.request_quit();
    assert!(!quit_out.quit, "dirty post-Jam edit must prompt confirm");
    let after_quit_req = match app.effect(filter_id).unwrap().1.kind {
        EffectKind::Filter(fp) => fp.env_pattern,
        _ => unreachable!(),
    };
    assert_eq!(
        after_quit_req, 15,
        "post-Jam edit must survive request_quit() without stale groove_browser rollback"
    );
    app.press_keys("<Esc>").unwrap();

    // 4. Alt-k (Keep) during Groove Browser preview must also cancel preview cleanly so an
    //    uncommitted preview slot cannot leak into the document.
    app.press_keys("Pj").unwrap();
    assert!(app.groove_browser_view().is_some());
    assert_eq!(
        match app.audio_project().mixer.master.inserts[0].kind {
            EffectKind::Filter(fp) => fp.env_pattern,
            _ => unreachable!(),
        },
        16
    );
    // Alt-c may return an error when no lane is focused to keep, OR succeed if a lane is focused;
    // either way Groove Browser must already have cancelled & restored slot 15 before keep runs!
    let _ = app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::ALT));
    assert!(
        app.groove_browser_view().is_none(),
        "Alt-c must close Groove Browser"
    );
    let after_alt_c = match app.effect(filter_id).unwrap().1.kind {
        EffectKind::Filter(fp) => fp.env_pattern,
        _ => unreachable!(),
    };
    assert_eq!(
        after_alt_c, 15,
        "Alt-c must restore pre-preview slot 15 without leaking slot 16"
    );
}

#[test]
fn rack_preset_loading_structural_guards_and_jam_readiness() {
    use cadence_core::params::InstrumentParam;
    use cadence_core::rack::ModuleParam;

    let dir = tempfile::tempdir().unwrap();
    let save_path = dir.path().join("project.json");
    let lib = SampleLibrary::from_roots_only(&[], Some(&save_path));
    let mut app = App::new_with_library(Project::demo(), save_path, lib).unwrap();

    let synth_id = app
        .project()
        .instruments
        .iter()
        .find(|i| !i.rack.has_samplers())
        .unwrap()
        .id;
    app.execute_command("instrument TestSamp sampler").unwrap();
    let samp_id = app.selected_instrument.unwrap();
    app.execute_command("instrument TestKit tr808").unwrap();
    let kit_id = app.selected_instrument.unwrap();

    // 1. `Action::SetRack` validates structural Rack invariants (rejects empty zones).
    let invalid_rack = RackPatch {
        zones: Vec::new(),
        ..RackPatch::default_synth()
    };
    assert!(app
        .apply(Action::SetRack {
            instrument: synth_id,
            rack: invalid_rack,
        })
        .is_err());
    app.apply(Action::SetRack {
        instrument: samp_id,
        rack: RackPatch::default_sampler(),
    })
    .unwrap();

    // 2. Corrupt preset files are rejected cleanly and valid `:preset-load` is undoable.
    let preset_file = dir.path().join("lead.preset.json");
    let mut acid_patch = RackPatch::default_synth();
    acid_patch
        .write_param_native(
            InstrumentParam::Module {
                zone: 1,
                module: 2,
                param: ModuleParam::Cutoff,
            },
            420.0,
        )
        .unwrap();
    InstrumentPreset::new("TestLead", Some("Synth".into()), acid_patch)
        .save(&preset_file)
        .unwrap();

    let corrupt_file = dir.path().join("corrupt.preset.json");
    std::fs::write(&corrupt_file, b"{ broken").unwrap();
    app.execute_command("instrument TestKit").unwrap();
    assert_eq!(app.selected_instrument, Some(kit_id));
    assert!(app
        .execute_command(&format!("preset-load {}", corrupt_file.display()))
        .is_err());

    let before_load = app.instrument(samp_id).unwrap().rack.clone();
    app.execute_command("instrument TestSamp").unwrap();
    app.execute_command(&format!("preset-load {}", preset_file.display()))
        .unwrap();
    let loaded_cut = app
        .instrument(samp_id)
        .unwrap()
        .rack
        .read_param_native(InstrumentParam::Module {
            zone: 1,
            module: 2,
            param: ModuleParam::Cutoff,
        })
        .unwrap();
    assert!((loaded_cut - 420.0).abs() < 1.0);
    app.apply(Action::Undo).unwrap();
    assert_eq!(app.instrument(samp_id).unwrap().rack, before_load);

    // 3. Save via native FileDialogRequest::SaveInstrumentPreset.
    let synth_name = app.instrument(synth_id).unwrap().name.clone();
    app.execute_command(&format!("instrument {synth_name}"))
        .unwrap();
    let saved_out = dir.path().join("saved-synth.preset.json");
    app.apply_file_dialog_result(
        FileDialogRequest::SaveInstrumentPreset {
            default_name: "saved-synth.preset.json".into(),
        },
        saved_out.clone(),
    )
    .unwrap();
    assert!(saved_out.is_file());

    // 4. Rack sample readiness in Jam rejects unassigned & missing sample paths, and plays once loaded.
    app.execute_command("instrument UnloadedKit kit").unwrap();
    let unloaded_kit = app.selected_instrument.unwrap();
    app.detail = Detail::Instrument(unloaded_kit);
    app.press_keys("gj").unwrap();
    assert!(app.is_jam(), "gj must enter Jam mode");
    let unassigned_err = app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
    assert!(
        unassigned_err.is_err(),
        "Jam must reject Rack with unassigned Sampler module instead of playing silence"
    );
    app.press_keys("<Esc>").unwrap();

    app.execute_command("zone sample ./missing_hit.wav")
        .unwrap();
    app.press_keys("gj").unwrap();
    assert!(app.is_jam(), "gj must enter Jam mode");
    let jam_err = app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
    assert!(
        jam_err.is_err(),
        "Jam must reject Rack with unavailable sample path"
    );
    app.press_keys("<Esc>").unwrap();

    // Load a synthetic WAV into Zone 1 -> Jam now succeeds.
    let hit_wav = dir.path().join("hit.wav");
    common::write_sine_wav(&hit_wav, 1, 44_100, 400, 440.0, 0.43);
    app.execute_command("zone sample ./hit.wav").unwrap();
    app.press_keys("gj").unwrap();
    let jam_ok = app
        .handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(jam_ok.live_notes_on.len(), 1);
    assert_eq!(jam_ok.live_notes_on[0].instrument_id, unloaded_kit);
}
