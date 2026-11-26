//! End-to-end integration tests for the Interactive Rack Zone Editor
//! and Resampling / Bounce-to-Sampler (`:zone`, `:bounce`, `execute_bounce_request`).
mod common;

use cadence::host::execute_bounce_request;
use cadence_app::{Action, App, BounceScope, ControlTarget, Detail, RackTab, ZoneControl};
use cadence_core::material::{Lane, Part, Pattern, PatternKind, PhaseMode, Placement, Project};
use cadence_core::music::Step;
use cadence_core::params::{InstrumentParam, ZoneParam};
use cadence_core::rack::{drum_midi, module_sample_key, ModuleKind, ModuleParam};
use cadence_core::sampler::SampleLoopMode;
use cadence_engine::audio::{render_measure_report, render_schedule_pcm};
use std::path::Path;

fn write_test_wav(path: &Path, freq_hz: f32, duration_sec: f32, sample_rate: u32) {
    let frames = (duration_sec * sample_rate as f32) as usize;
    common::write_decay_wav(path, 2, sample_rate, frames, freq_hz, 0.75);
}

fn find_control_index(app: &App, target_field: ZoneControl) -> usize {
    app.control_descriptors()
        .iter()
        .position(|c| {
            matches!(
                c.target,
                ControlTarget::Zone { field, .. } if field == target_field
            )
        })
        .unwrap_or_else(|| panic!("ZoneControl::{target_field:?} not found in controls"))
}

fn find_zone_param_index(app: &App, target_param: cadence_core::params::ZoneParam) -> usize {
    app.control_descriptors()
        .iter()
        .position(|c| {
            matches!(
                c.target,
                ControlTarget::Param(cadence_core::params::ParamAddress::Zone { param, .. })
                    if param == target_param
            )
        })
        .unwrap_or_else(|| panic!("ZoneParam::{target_param:?} not found in controls"))
}

fn make_temp_demo_app() -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    let save_path = dir.path().join("project.json");
    let project = Project::demo();
    let lib = cadence_core::sampler::SampleLibrary::from_roots(&[], Some(&save_path));
    let app = App::new_with_library(project, save_path, lib).unwrap();
    (dir, app)
}

#[test]
fn rack_zone_navigation_controls_direct_edit_and_undo_redo() {
    let (_dir, mut app) = make_temp_demo_app();

    // Create a TR-808 Rack Kit and open it in the Instrument deck (Zones tab).
    app.execute_command("kit Kit808 tr808").unwrap();
    let kit_id = app.selected_instrument.expect("created kit selected");
    assert_eq!(app.detail, Detail::Instrument(kit_id));
    assert_eq!(app.rack_ui.zone_cursor, 0);
    app.execute_command("tab zones").unwrap();
    assert_eq!(app.rack_ui.tab, RackTab::Zones);

    // Verify RACK and ZONE control groups are exposed for sampled TR-808 zone 0 (BD).
    let sample_controls = app.control_descriptors();
    assert!(sample_controls.iter().any(|c| c.group == "RACK"));
    assert!(sample_controls
        .iter()
        .any(|c| c.group == "ZONE" && c.label == "Name"));
    assert!(!sample_controls
        .iter()
        .any(|c| c.group == "ZONE" && c.label == "Zone"));
    assert!(sample_controls
        .iter()
        .any(|c| c.group == "ZONE" && c.label == "MIDI Low"));
    assert!(sample_controls
        .iter()
        .any(|c| c.group == "ZONE" && c.label == "Vel High"));
    assert!(sample_controls
        .iter()
        .any(|c| c.group == "ZONE" && c.label == "Sample"));

    // Load an external synthesized drum preset into Zone 0 via `:zone preset` and verify its modular voice!
    app.execute_command("zone preset analog-kick").unwrap();
    assert!(
        app.instrument(kit_id).unwrap().rack.zones[0]
            .modules
            .iter()
            .any(|m| matches!(m.kind, ModuleKind::Vco(_))),
        "analog-kick zone should contain a VCO module"
    );

    // Step zones via helper API and directly via horizontal pill navigation (`h`/`l`) on Zones.
    app.step_zone_cursor(1);
    assert_eq!(app.rack_ui.zone_cursor, 1);
    app.step_zone_cursor(-1);
    assert_eq!(app.rack_ui.zone_cursor, 0);

    app.press_keys("l").unwrap();
    assert_eq!(app.rack_ui.zone_cursor, 1);
    app.press_keys("h").unwrap();
    assert_eq!(app.rack_ui.zone_cursor, 0);

    // Nudge MIDI Low upward on a single-note pad (36..36 -> pushes high up to keep low <= high!).
    app.control_cursor = find_control_index(&app, ZoneControl::LowMidi);
    app.press_keys("<m-l>").unwrap();
    {
        let kit = &app.instrument(kit_id).unwrap().rack;
        assert_eq!((kit.zones[0].low_midi, kit.zones[0].high_midi), (37, 37));
    }
    // Coarse nudge MIDI High up by +12 semitones.
    app.control_cursor = find_control_index(&app, ZoneControl::HighMidi);
    app.press_keys("<m-L>").unwrap();
    {
        let kit = &app.instrument(kit_id).unwrap().rack;
        assert_eq!((kit.zones[0].low_midi, kit.zones[0].high_midi), (37, 49));
    }
    // Nudge Vel Low and Vel High while keeping 0 <= low_vel <= high_vel <= 1.
    app.control_cursor = find_control_index(&app, ZoneControl::LowVel);
    app.press_keys("<m-L>").unwrap(); // +5%
    app.control_cursor = find_control_index(&app, ZoneControl::HighVel);
    app.press_keys("<m-H>").unwrap(); // -5%
                                      // Nudge Zone Gain (+2.0 dB), Zone Pan (+0.10), Choke Group (None -> Group 1).
    app.control_cursor = find_zone_param_index(&app, ZoneParam::Gain);
    app.press_keys("<m-L>").unwrap();
    app.control_cursor = find_zone_param_index(&app, ZoneParam::Pan);
    app.press_keys("<m-L>").unwrap();
    app.control_cursor = find_control_index(&app, ZoneControl::ChokeGroup);
    app.press_keys("<m-l>").unwrap();

    // Switch to Panel tab, step from Rack overview pill (0) to Zone 0 VCO pill (1), and nudge Tune (+1 st)!
    app.execute_command("tab panel").unwrap();
    app.press_keys("l").unwrap();
    let tune_idx = app
        .control_descriptors()
        .iter()
        .position(|c| {
            matches!(
                c.target,
                ControlTarget::Param(cadence_core::params::ParamAddress::Module {
                    param: ModuleParam::Tune,
                    ..
                })
            )
        })
        .expect("VCO Tune control exposed on Panel tab");
    app.control_cursor = tune_idx;
    app.press_keys("<m-l>").unwrap();
    app.execute_command("tab zones").unwrap();

    {
        let kit = &app.instrument(kit_id).unwrap().rack;
        let z = &kit.zones[0];
        assert!((z.low_vel - 0.05).abs() < 1e-4);
        assert!((z.high_vel - 0.95).abs() < 1e-4);
        assert!((z.gain_db - 2.0).abs() < 1e-4);
        assert!((z.pan - 0.10).abs() < 1e-4);
        assert_eq!(z.choke_group, Some(1));
        let vco = z
            .modules
            .iter()
            .find_map(|m| match m.kind {
                ModuleKind::Vco(v) => Some(v),
                _ => None,
            })
            .expect("expected VCO in analog-kick zone");
        assert!((vco.tune_semitones - 1.0).abs() < 1e-4);
    }

    // Direct edit via Enter on Name row!
    app.control_cursor = find_control_index(&app, ZoneControl::Name);
    app.press_keys("<Enter>SubKick<Enter>").unwrap();
    assert_eq!(
        app.instrument(kit_id).unwrap().rack.zones[0].name,
        "SubKick"
    );

    // Undo and Redo restore and reapply zone rename cleanly.
    app.apply(Action::Undo).unwrap();
    assert_eq!(app.instrument(kit_id).unwrap().rack.zones[0].name, "BD");
    app.apply(Action::Redo).unwrap();
    assert_eq!(
        app.instrument(kit_id).unwrap().rack.zones[0].name,
        "SubKick"
    );
}

#[test]
fn rack_zone_ex_commands_sample_loading_and_audio_effect() {
    let (dir, mut app) = make_temp_demo_app();
    let wav_abs = dir.path().join("samples/punch.wav");
    write_test_wav(&wav_abs, 220.0, 0.25, 48_000);

    // Create kit + simple 1-bar BD pattern + part + placement so we can render audio!
    app.execute_command("kit TestKit tr808").unwrap();
    let kit_id = app.selected_instrument.unwrap();
    let baseline_zones = app.instrument(kit_id).unwrap().rack.zones.len();

    // :zone next / prev / select
    app.execute_command("zone next").unwrap();
    assert_eq!(app.rack_ui.zone_cursor, 1);
    app.execute_command("zone prev").unwrap();
    assert_eq!(app.rack_ui.zone_cursor, 0);
    app.execute_command("zone select OH").unwrap();
    assert_eq!(app.rack_ui.zone_cursor, 10);
    app.execute_command("zone select 1").unwrap();
    assert_eq!(app.rack_ui.zone_cursor, 0);

    // :zone add / dup / del / name / range / vel / choke
    app.execute_command("zone add Rim2 55 synth").unwrap();
    assert_eq!(app.rack_ui.zone_cursor, baseline_zones);
    app.execute_command("zone dup RimLayer").unwrap();
    assert_eq!(app.rack_ui.zone_cursor, baseline_zones + 1);
    app.execute_command("zone name RimTop").unwrap();
    app.execute_command("zone range C3 C4 60").unwrap();
    app.execute_command("zone vel 25% 90%").unwrap();
    app.execute_command("zone choke 4").unwrap();
    {
        let kit = &app.instrument(kit_id).unwrap().rack;
        let z = &kit.zones[app.rack_ui.zone_cursor];
        assert_eq!(z.name, "RimTop");
        assert_eq!((z.low_midi, z.high_midi, z.root_midi), (48, 60, 60));
        assert!((z.low_vel - 0.25).abs() < 1e-4 && (z.high_vel - 0.90).abs() < 1e-4);
        assert_eq!(z.choke_group, Some(4));
    }
    app.execute_command("zone choke none").unwrap();
    app.execute_command("zone del").unwrap(); // removes RimTop
    app.execute_command("zone del").unwrap(); // removes Rim2
    assert_eq!(
        app.instrument(kit_id).unwrap().rack.zones.len(),
        baseline_zones
    );

    // Build an isolated 1-bar drum pattern triggering MIDI 36 (BD) on beats 1 and 3.
    let catalog = cadence_core::presets::PresetCatalog::discover(
        &[],
        None,
        &cadence_core::config::UserConfig::default(),
    );
    let tr808_patch = catalog
        .find_by_category("Kit", "tr-808")
        .expect("factory TR-808 kit preset must be present in presets/kits/")
        .patch
        .clone();
    let mut project = Project::empty();
    let ch = cadence_core::mixer::Channel::new(1, cadence_core::mixer::ChannelKind::Audio, "Drums");
    project.mixer.channels.push(ch);
    project
        .instruments
        .push(cadence_core::material::InstrumentInstance {
            id: 2,
            name: "Kit808".into(),
            rack: tr808_patch,
            channel: 1,
        });
    let (deg, acc) = project.midi_to_degree(drum_midi::BD);
    let mut steps = vec![Step::default(); 16];
    for idx in [0usize, 8] {
        steps[idx] = Step {
            active: true,
            degree: deg,
            accidental: acc,
            velocity: 0.85,
            gate: 1.0,
            ..Step::default()
        };
    }
    project.patterns.push(Pattern {
        id: 3,
        name: "Kick".into(),
        kind: PatternKind::Notes { steps },
    });
    project.parts.push(Part {
        id: 4,
        name: "KickPart".into(),
        pattern: 3,
        instrument: 2,
        transpose: 0,
        velocity_scale: 1.0,
        parameter_links: Vec::new(),
    });
    project.lanes.push(Lane {
        id: 5,
        name: "Drums".into(),
        parts: Vec::new(),
        placements: vec![Placement {
            id: 6,
            part: 4,
            start_step: 0,
            length_steps: 16,
            phase: PhaseMode::Song,
            offset_steps: 0,
            exceptions: Vec::new(),
        }],
    });
    project.bars = 1;

    let kit_path = dir.path().join("kit_audio.json");
    let lib = common::factory_sample_library(Some(&kit_path), false);
    let mut audio_app = App::new_with_library(project, kit_path, lib).unwrap();
    audio_app.execute_command("instrument Kit808").unwrap();
    let (base_report, base_pcm) = render_schedule_pcm(
        &[(0, audio_app.compiled_project())],
        0,
        16,
        audio_app.sample_bank(),
        24_000,
    )
    .unwrap();
    assert!(base_report.output.rms[0] > 0.01);

    // Edit BD zone: pan hard right + pitch up +7 st -> audible change + right-heavy stereo!
    audio_app.execute_command("zone select 1").unwrap();
    audio_app.execute_command("zone set pan R100").unwrap();
    audio_app
        .execute_command("zone set sampler.coarse +7st")
        .unwrap();
    let (panned_report, panned_pcm) = render_schedule_pcm(
        &[(0, audio_app.compiled_project())],
        0,
        16,
        audio_app.sample_bank(),
        24_000,
    )
    .unwrap();
    assert_ne!(
        base_pcm, panned_pcm,
        "zone tune/pan edits must audibly change rendered output"
    );
    assert!(
        panned_report.output.rms[1] > panned_report.output.rms[0] * 1.5,
        "R100 zone pan must bias energy to the right channel (L={}, R={})",
        panned_report.output.rms[0],
        panned_report.output.rms[1]
    );

    // Load custom sample into BD zone Sampler module and verify sample loading + controls + audio!
    let out = audio_app
        .execute_command("zone sample samples/punch.wav")
        .unwrap();
    assert!(out.samples_changed);
    assert!(
        audio_app.ready(),
        "zone sample should load with zero issues"
    );
    let (bd_zone_id, _, bd_mod_id, _) = audio_app
        .instrument(2)
        .unwrap()
        .rack
        .sampler_modules()
        .next()
        .expect("BD zone must have a Sampler module");
    assert!(
        audio_app
            .sample_bank()
            .iter()
            .any(|(k, s)| *k == module_sample_key(2, bd_zone_id, bd_mod_id) && s.frames > 0),
        "sample bank must contain decoded zone sample under module_sample_key"
    );

    // Focus the BD zone Sampler module (pill 1 after Rack overview pill 0) and nudge Coarse Tune & Loop Mode via keyboard.
    audio_app.execute_command("tab panel").unwrap();
    audio_app.press_keys("l").unwrap();
    let smp_controls = audio_app.control_descriptors();
    let coarse_idx = smp_controls
        .iter()
        .position(|c| {
            matches!(
                c.target,
                ControlTarget::Param(cadence_core::params::ParamAddress::Module {
                    param: ModuleParam::CoarseTune,
                    ..
                })
            )
        })
        .expect("Sampler CoarseTune control visible");
    let loop_idx = smp_controls
        .iter()
        .position(|c| {
            matches!(
                c.target,
                ControlTarget::Param(cadence_core::params::ParamAddress::Module {
                    param: ModuleParam::LoopMode,
                    ..
                })
            )
        })
        .expect("Sampler LoopMode control visible");
    audio_app.control_cursor = coarse_idx;
    audio_app.press_keys("<m-l>").unwrap();
    audio_app.control_cursor = loop_idx;
    audio_app.press_keys("<m-l>").unwrap();
    {
        let rack = &audio_app.instrument(2).unwrap().rack;
        let ModuleKind::Sampler(smp) = &rack.zones[0].module(bd_mod_id).unwrap().kind else {
            panic!("expected Sampler module");
        };
        // Zone 0 already had coarse_semitones = +7 from `:zone set tune +7st` before swapping
        // sample path; nudging `l` increments it to +8.
        assert_eq!(smp.coarse_semitones, 8);
        assert_eq!(smp.loop_mode, SampleLoopMode::Forward);
    }

    // Reset loop mode to Off and render sampled zone audio; verify non-silent & distinct!
    let loop_param = InstrumentParam::Module {
        zone: bd_zone_id,
        module: bd_mod_id,
        param: ModuleParam::LoopMode,
    };
    audio_app
        .apply(Action::SetParam {
            address: cadence_core::params::ParamAddress::from_instrument(2, loop_param),
            value: SampleLoopMode::Off.to_index() as f32,
        })
        .unwrap();
    let (sample_report, sample_pcm) = render_schedule_pcm(
        &[(0, audio_app.compiled_project())],
        0,
        16,
        audio_app.sample_bank(),
        24_000,
    )
    .unwrap();
    assert!(sample_report.output.rms[1] > 0.01);
    assert_ne!(sample_pcm, panned_pcm);
}

#[test]
fn bounce_part_pattern_and_bars_to_playable_sampler_end_to_end() {
    let (dir, mut app) = make_temp_demo_app();

    // 1. :bounce part <PART> LeadBounce
    let first_part_name = app.project().parts[0].name.clone();
    let out = app
        .execute_command(&format!("bounce part \"{first_part_name}\" LeadBounce"))
        .unwrap();
    let req = out.bounce.expect("expected BounceRequest on Outcome");
    assert_eq!(req.relative_path, "samples/leadbounce.wav");
    assert!(!Path::new(&req.relative_path).is_absolute());
    assert_eq!(
        req.output_path,
        dir.path()
            .canonicalize()
            .unwrap()
            .join("samples/leadbounce.wav")
    );

    let complete = execute_bounce_request(&mut app, &req).unwrap();
    assert!(complete.changed && complete.samples_changed);
    assert!(req.output_path.is_file(), "bounced WAV must exist on disk");
    assert!(app.sample_issues().is_empty());
    assert!(app.ready());

    // Inspect written WAV directly with hound to verify real stereo 48 kHz non-silent audio.
    let mut reader = hound::WavReader::open(&req.output_path).unwrap();
    let spec = reader.spec();
    assert_eq!(spec.channels, 2);
    assert_eq!(spec.sample_rate, 48_000);
    assert_eq!(spec.bits_per_sample, 16);
    let peak = reader
        .samples::<i16>()
        .map(|s| s.unwrap().unsigned_abs())
        .max()
        .unwrap_or(0);
    assert!(
        peak > 500,
        "bounced part WAV must be non-silent (peak={peak})"
    );

    // Verify the new Sampler instrument is open in `gi` and uses the portable relative path.
    let Detail::Instrument(bounced_inst_id) = app.detail else {
        panic!(
            "expected new Sampler instrument open in gi, got {:?}",
            app.detail
        );
    };
    let inst = app.instrument(bounced_inst_id).unwrap();
    assert_eq!(inst.name, "LeadBounce");
    let (z_id, _, m_id, patch) = inst
        .rack
        .sampler_modules()
        .next()
        .expect("expected Sampler module in bounced Rack");
    assert_eq!(patch.path.as_deref(), Some("samples/leadbounce.wav"));
    assert_eq!(patch.root_midi, 60);
    assert!(app
        .instrument_sample(module_sample_key(bounced_inst_id, z_id, m_id))
        .is_some());

    // Collision avoidance: bouncing again with same name writes samples/leadbounce-2.wav!
    let out2 = app
        .execute_command(&format!("bounce part \"{first_part_name}\" LeadBounce"))
        .unwrap();
    let req2 = out2.bounce.expect("second bounce request");
    assert_eq!(req2.relative_path, "samples/leadbounce-2.wav");
    execute_bounce_request(&mut app, &req2).unwrap();
    assert!(dir.path().join("samples/leadbounce-2.wav").is_file());

    // 2. :bounce pattern <PAT> PatBounce
    let note_pat_name = app
        .project()
        .patterns
        .iter()
        .find(|p| matches!(p.kind, PatternKind::Notes { .. }))
        .unwrap()
        .name
        .clone();
    let out_pat = app
        .execute_command(&format!("bounce pattern \"{note_pat_name}\" PatBounce"))
        .unwrap();
    let req_pat = out_pat.bounce.expect("pattern BounceRequest");
    assert!(matches!(req_pat.scope, BounceScope::Pattern(_)));
    execute_bounce_request(&mut app, &req_pat).unwrap();
    assert!(req_pat.output_path.is_file());
    assert!(app.ready());

    // 3. :bounce bars 1..2 MixBounce --part
    let parts_before = app.project().parts.len();
    let patterns_before = app.project().patterns.len();
    let out_bars = app
        .execute_command("bounce bars 1..2 MixBounce --part")
        .unwrap();
    let req_bars = out_bars.bounce.expect("range BounceRequest");
    assert_eq!(
        req_bars.scope,
        BounceScope::Range {
            start_bar: 1,
            end_bar: 2,
        }
    );
    assert_eq!(req_bars.start_step, 0);
    assert_eq!(req_bars.steps, 32);
    assert!(req_bars.create_part);

    execute_bounce_request(&mut app, &req_bars).unwrap();
    assert!(req_bars.output_path.is_file());
    assert_eq!(app.project().parts.len(), parts_before + 1);
    assert_eq!(app.project().patterns.len(), patterns_before + 1);
    assert!(app.ready());

    let new_part = app.project().parts.last().unwrap().clone();
    assert_eq!(new_part.name, "MixBounce");
    let new_pat = app.pattern(new_part.pattern).unwrap();
    match &new_pat.kind {
        PatternKind::Notes { steps } => {
            assert_eq!(steps.len(), 32);
            assert!(steps[0].active);
            assert_eq!(
                app.project()
                    .pitch_midi(steps[0].degree, steps[0].accidental, 0),
                60
            );
            assert!(steps[1..].iter().all(|s| !s.active));
        }
        other => panic!("expected Notes trigger pattern, got {other:?}"),
    }

    // Play ONLY the newly bounced Sampler part in a 2-bar render and confirm non-silent audio!
    let mut playback_proj = app.project().clone();
    playback_proj.bars = 2;
    playback_proj.sections.clear();
    let lane_id = playback_proj.lanes.iter().map(|l| l.id).max().unwrap_or(0) + 100;
    playback_proj.lanes = vec![Lane {
        id: lane_id,
        name: "BouncedOnly".into(),
        parts: Vec::new(),
        placements: vec![Placement {
            id: lane_id + 1,
            part: new_part.id,
            start_step: 0,
            length_steps: 32,
            phase: PhaseMode::Restart,
            offset_steps: 0,
            exceptions: Vec::new(),
        }],
    }];
    let compiled = playback_proj.compile().unwrap();
    let play_report = render_measure_report(&compiled, app.sample_bank(), 2, 24_000).unwrap();
    assert!(
        play_report.output.peak[0] > 0.02 && play_report.output.peak[1] > 0.02,
        "playing the newly created bounced Sampler must produce non-silent stereo audio: {:?}",
        play_report.output
    );

    // 4. Test bounce_selection() helper defaults cleanly.
    app.press_keys("ga").unwrap();
    let sel_out = app.bounce_selection().unwrap();
    assert!(sel_out.bounce.is_some());
}

#[test]
fn sampler_source_row_and_rack_zone_sample_open_sample_browser() {
    use cadence_core::sampler::SampleLibrary;

    let dir = tempfile::tempdir().unwrap();
    let samples_dir = dir.path().join("samples");
    std::fs::create_dir_all(&samples_dir).unwrap();
    write_test_wav(&samples_dir.join("kick-fixture.wav"), 220.0, 0.05, 48_000);

    let save_path = dir.path().join("kit.json");
    let lib = SampleLibrary::from_roots(&[samples_dir], Some(&save_path));
    let mut app = App::new_with_library(Project::empty(), save_path, lib).unwrap();

    let wait_browser_close = |app: &mut App| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while app.mode_name() == "SAMPLES" && std::time::Instant::now() < deadline {
            app.poll_browser_updates().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };

    // Standalone Sampler Rack: `SOURCE · Sample` top row opens Sample Browser directly on Enter.
    app.execute_command("sampler SoloSamp").unwrap();
    let samp_id = app.selected_instrument.unwrap();
    app.press_keys("gi").unwrap();
    assert_eq!(app.detail, Detail::Instrument(samp_id));
    assert_eq!(app.rack_ui.tab, RackTab::Panel);

    app.press_keys("l").unwrap();
    let controls = app.control_descriptors();
    assert_eq!(controls[0].group, "SOURCE");
    assert_eq!(controls[0].label, "Sample");
    assert!(matches!(
        controls[0].target,
        ControlTarget::SamplerSource {
            instrument,
            zone: 1,
            module: 1,
        } if instrument == samp_id
    ));
    assert_eq!(controls[0].display, "(none · Enter to browse)");

    app.control_cursor = 0;
    app.press_keys("<Enter>").unwrap();
    assert_eq!(app.mode_name(), "SAMPLES");
    app.press_keys("<Enter>").unwrap();
    wait_browser_close(&mut app);
    assert_eq!(app.mode_name(), "NORMAL");
    assert_eq!(app.control_descriptors()[0].display, "kick-fixture.wav");

    // Multi-Zone Rack `ZoneControl::SamplePath`: auto-adds a Sampler module while preserving zone metadata & undo.
    app.execute_command("instrument Kit kit").unwrap();
    let kit_id = app.selected_instrument.unwrap();
    app.execute_command("zone add BD 36").unwrap();
    let z_idx = app.rack_ui.zone_cursor;
    app.execute_command("zone range 36 40 38").unwrap();
    app.execute_command("zone choke 2").unwrap();
    app.execute_command("zone set gain -3").unwrap();
    app.execute_command("zone set pan 25%").unwrap();
    app.press_keys("gi").unwrap();
    assert_eq!(app.detail, Detail::Instrument(kit_id));

    app.press_keys("H").unwrap();
    assert_eq!(app.rack_ui.tab, RackTab::Zones);

    let path_idx = find_control_index(&app, ZoneControl::SamplePath);
    assert_eq!(
        app.control_descriptors()[path_idx].display,
        "<add Sampler> (Enter)"
    );
    assert!(app.instrument(kit_id).unwrap().rack.zones[z_idx]
        .modules
        .iter()
        .any(|m| matches!(m.kind, ModuleKind::Vco(_))));
    assert!(app.instrument(kit_id).unwrap().rack.zones[z_idx]
        .modules
        .iter()
        .all(|m| !matches!(m.kind, ModuleKind::Sampler(_))));

    app.control_cursor = path_idx;
    app.press_keys("i<Enter>").unwrap();
    assert_eq!(
        app.mode_name(),
        "SAMPLES",
        "Enter in EDIT on ZoneControl::SamplePath must open Sample Browser directly"
    );
    app.press_keys("<Enter>").unwrap();
    wait_browser_close(&mut app);
    assert_eq!(app.mode_name(), "NORMAL");

    let z = &app.instrument(kit_id).unwrap().rack.zones[z_idx];
    assert_eq!(z.name, "BD");
    assert_eq!(z.low_midi, 36);
    assert_eq!(z.high_midi, 40);
    assert_eq!(z.root_midi, 38);
    assert_eq!(z.choke_group, Some(2));
    assert!((z.gain_db - (-3.0)).abs() < 1e-3);
    assert!((z.pan - 0.25).abs() < 1e-3);
    assert!(
        z.modules
            .iter()
            .any(|m| matches!(m.kind, ModuleKind::Vco(_))),
        "existing VCO module in modular zone must be preserved when Sampler is auto-added"
    );
    let smp = z
        .modules
        .iter()
        .find_map(|m| match &m.kind {
            ModuleKind::Sampler(s) => Some(s),
            _ => None,
        })
        .expect("Sampler module must be auto-added to zone");
    assert!(smp
        .path
        .as_deref()
        .unwrap_or("")
        .contains("kick-fixture.wav"));
    assert_eq!(
        app.control_descriptors()[path_idx].display,
        "kick-fixture.wav"
    );

    // Pressing `B` on Zones tab also opens Sample Browser cleanly.
    app.press_keys("B").unwrap();
    assert_eq!(app.mode_name(), "SAMPLES");
    app.press_keys("<Esc>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");

    // `u` cleanly undoes the auto-added Sampler while preserving zone metadata.
    app.press_keys("u").unwrap();
    let z_undone = &app.instrument(kit_id).unwrap().rack.zones[z_idx];
    assert!(z_undone
        .modules
        .iter()
        .all(|m| !matches!(m.kind, ModuleKind::Sampler(_))));
    assert_eq!(z_undone.choke_group, Some(2));
    assert!((z_undone.gain_db - (-3.0)).abs() < 1e-3);
}

#[test]
fn rack_zone_repeat_nudges_coalesce_without_sample_resync() {
    use cadence_app::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    let (_dir, mut app) = make_temp_demo_app();
    app.execute_command("kit KitTest tr808").unwrap();
    let kit_id = app
        .project()
        .instruments
        .iter()
        .find(|i| i.name == "KitTest")
        .unwrap()
        .id;
    let init_gain = app.instrument(kit_id).unwrap().rack.zones[0].gain_db;
    app.execute_command("instrument KitTest").unwrap();
    app.rack_ui.tab = RackTab::Zones;
    let gain_row = app
        .control_descriptors()
        .iter()
        .position(|c| c.group == "ZONE" && c.label.eq_ignore_ascii_case("Zone Gain"))
        .expect("zone gain control");
    app.control_cursor = gain_row;
    let first = app
        .handle_key(KeyEvent::new_with_kind(
            KeyCode::Char('l'),
            KeyModifiers::ALT,
            KeyEventKind::Press,
        ))
        .unwrap();
    assert!(
        !first.samples_changed,
        "zone gain nudge must not resync SampleBank"
    );
    for _ in 0..4 {
        let rep = app
            .handle_key(KeyEvent::new_with_kind(
                KeyCode::Char('l'),
                KeyModifiers::ALT,
                KeyEventKind::Repeat,
            ))
            .unwrap();
        assert!(!rep.samples_changed);
    }
    let nudged_gain = app.instrument(kit_id).unwrap().rack.zones[0].gain_db;
    assert!(nudged_gain > init_gain + 0.5);
    app.apply(Action::Undo).unwrap();
    let undone_gain = app.instrument(kit_id).unwrap().rack.zones[0].gain_db;
    assert!(
        (undone_gain - init_gain).abs() < 1e-4,
        "5 repeat zone nudges must coalesce into 1 undo step"
    );
}

#[test]
fn bounce_to_rack_zone_and_incremental_register_without_full_rescan() {
    use cadence_app::BounceTarget;
    use cadence_core::sampler::SampleLibrary;

    let tmp = tempfile::tempdir().unwrap();
    let proj_dir = tmp.path().join("session");
    let extra_root = tmp.path().join("extra-samples");
    std::fs::create_dir_all(&proj_dir).unwrap();
    std::fs::create_dir_all(&extra_root).unwrap();
    write_test_wav(&extra_root.join("initial_hat.wav"), 220.0, 0.025, 48_000);

    let proj_path = proj_dir.join("song.json");
    let lib = SampleLibrary::from_roots_only(std::slice::from_ref(&extra_root), Some(&proj_path));
    assert_eq!(lib.total_count(), 1);

    let mut app = App::new_with_library(Project::demo(), proj_path, lib).unwrap();
    app.execute_command("kit BounceKit tr808").unwrap();
    let kit_id = app
        .project()
        .instruments
        .iter()
        .find(|i| i.name == "BounceKit")
        .unwrap()
        .id;
    let (orig_z0_name, orig_z0_modules) = {
        let z0 = &app.instrument(kit_id).unwrap().rack.zones[0];
        (z0.name.clone(), z0.modules.clone())
    };

    // Drop an unregistered wav into `extra_root` after discovery to ensure `complete_bounce`
    // registers incrementally rather than rescanning unrelated roots.
    write_test_wav(
        &extra_root.join("sneaky_unscanned.wav"),
        220.0,
        0.025,
        48_000,
    );

    // 1. `:bounce 1 zone BounceKit z1` targets zone index 0 (`id = 1`).
    let out = app.execute_command("bounce 1 zone BounceKit z1").unwrap();
    let req = out.bounce.expect("BounceRequest emitted");
    assert!(matches!(
        req.target,
        BounceTarget::RackZone {
            instrument,
            zone_index: Some(0),
            ..
        } if instrument == kit_id
    ));
    let report = cadence_engine::audio::render::render_bounce_wav(
        &req.score,
        app.sample_bank(),
        req.start_step,
        req.steps,
        &req.output_path,
        req.sample_rate,
    )
    .unwrap();
    let secs = report.frames as f64 / report.sample_rate as f64;
    let done = app.complete_bounce(&req, secs).unwrap();
    assert!(done.samples_changed);

    let kit = &app.instrument(kit_id).unwrap().rack;
    let z0 = &kit.zones[0];
    let (smp_mod_id, smp) = z0
        .modules
        .iter()
        .find_map(|m| match &m.kind {
            ModuleKind::Sampler(p) => Some((m.id, p)),
            _ => None,
        })
        .expect("zone 0 must have a Sampler module");
    assert_eq!(smp.path.as_deref(), Some(req.relative_path.as_str()));
    assert!(smp.one_shot);
    assert!(smp.sample_end > 0);
    assert!(app
        .sample_bank()
        .iter()
        .any(|(k, _)| *k == module_sample_key(kit_id, z0.id, smp_mod_id)));

    assert!(
        app.assets
            .library
            .visible_entries()
            .any(|(_, e)| e.path.ends_with(req.output_path.file_name().unwrap())),
        "Newly bounced WAV must be registered in SampleLibrary"
    );
    assert!(
        app.assets
            .library
            .visible_entries()
            .all(|(_, e)| e.name != "sneaky_unscanned"),
        "complete_bounce must not trigger a full library.rescan() across unrelated roots"
    );

    // Undo restores zone 0 cleanly.
    app.apply(Action::Undo).unwrap();
    let kit_after_undo = &app.instrument(kit_id).unwrap().rack;
    assert_eq!(kit_after_undo.zones[0].name, orig_z0_name);
    assert_eq!(kit_after_undo.zones[0].modules, orig_z0_modules);

    // 2. `:bounce 1 zone BounceKit C5 HighBounce` creates a brand-new zone at MIDI 72 (C5).
    let zones_before = kit_after_undo.zones.len();
    let out2 = app
        .execute_command("bounce 1 zone BounceKit C5 HighBounce")
        .unwrap();
    let req2 = out2.bounce.expect("BounceRequest for new zone");
    let report2 = cadence_engine::audio::render::render_bounce_wav(
        &req2.score,
        app.sample_bank(),
        req2.start_step,
        req2.steps,
        &req2.output_path,
        req2.sample_rate,
    )
    .unwrap();
    let secs2 = report2.frames as f64 / report2.sample_rate as f64;
    app.complete_bounce(&req2, secs2).unwrap();
    let kit2 = &app.instrument(kit_id).unwrap().rack;
    assert_eq!(kit2.zones.len(), zones_before + 1);
    let new_z = kit2.zones.last().unwrap();
    assert_eq!(new_z.name, "HighBounce");
    assert_eq!(new_z.low_midi, 72);
    assert_eq!(new_z.high_midi, 72);
    assert_eq!(new_z.root_midi, 72);

    // 3. Focusing the multi-zone Rack instrument deck and pressing `tb` opens pre-filled Bounce dialog;
    //    pressing `Enter` immediately commits targeting the active zone (`tb → Enter` 2-key fast)!
    app.execute_command("instrument BounceKit").unwrap();
    app.rack_ui.zone_cursor = 1;
    app.press_keys("tb").unwrap();
    assert_eq!(app.mode_name(), "WORKFLOW");
    let out3 = app.press_keys("<Enter>").unwrap();
    let req3 = out3
        .into_iter()
        .find_map(|o| o.bounce)
        .expect("tb + Enter from focused multi-zone Rack emits bounce");
    assert!(matches!(
        req3.target,
        BounceTarget::RackZone {
            instrument,
            zone_index: Some(1),
            ..
        } if instrument == kit_id
    ));
}

#[test]
fn bounce_dialog_respects_arrange_loop_from_zones_preset_browser_save_reload_and_assets_reload() {
    use cadence_app::BounceTarget;

    let dir = tempfile::tempdir().unwrap();
    let samples_dir = dir.path().join("samples");
    let presets_dir = dir.path().join("presets");
    std::fs::create_dir_all(&samples_dir).unwrap();
    std::fs::create_dir_all(&presets_dir).unwrap();
    write_test_wav(&samples_dir.join("hit.wav"), 180.0, 0.05, 48_000);

    let save_path = dir.path().join("studio.json");
    let lib = cadence_core::sampler::SampleLibrary::from_roots(&[samples_dir], Some(&save_path));
    let mut app = App::new_with_library(Project::demo(), save_path, lib).unwrap();

    // 1. Active Arrange loop is respected when pressing `tb` from `gi → Zones`!
    app.press_keys("ga").unwrap();
    app.press_keys("l[l]\\").unwrap(); // bar 2 start, bar 3 end, `\` activates loop -> bars 2..3
    assert_eq!(app.playing_loop(), Some((16, 48)));
    let inst_id = app.project().instruments[0].id;
    app.apply(Action::SelectInstrument {
        instrument: inst_id,
    })
    .unwrap();
    app.press_keys("LL").unwrap();
    assert_eq!(app.rack_ui.tab, RackTab::Zones);
    app.press_keys("tb").unwrap();
    assert_eq!(app.mode_name(), "WORKFLOW");
    let draft_view = app.authoring_view().expect("Bounce dialog view");
    assert_eq!(draft_view.title, "BOUNCE / RESAMPLE");
    assert!(
        draft_view.summary.contains("Bars 2..3"),
        "Bounce dialog from gi → Zones must pre-fill active Arrange loop Bars 2..3, got: {}",
        draft_view.summary
    );
    // `Enter` commits immediately (`tb → Enter` 2-key fast!) with the loop range and active zone!
    let outs = app.press_keys("<Enter>").unwrap();
    let req = outs
        .into_iter()
        .find_map(|o| o.bounce)
        .expect("tb → Enter must emit BounceRequest");
    assert_eq!(
        req.scope,
        BounceScope::Range {
            start_bar: 2,
            end_bar: 3
        }
    );
    assert!(matches!(
        req.target,
        BounceTarget::RackZone { instrument, zone_index: Some(0), .. } if instrument == inst_id
    ));

    // 2. Customize Target (`New Zone + MIDI`), MIDI (`C5`), Name (`"LoopSlice"`), and `--part` (`y`) in Bounce modal!
    app.press_keys("tbt").unwrap(); // Bounce -> Change target
    app.press_keys("n").unwrap(); // New Zone + MIDI
    app.press_keys("<Enter>").unwrap(); // accept pre-selected Rack
    app.press_keys("C5<Enter>").unwrap(); // MIDI note C5 (72)
    app.press_keys("LoopSlice<Enter>").unwrap(); // Name
    let custom_outs = app.press_keys("y").unwrap(); // Trigger part = Yes -> submits!
    let custom_req = custom_outs
        .into_iter()
        .find_map(|o| o.bounce)
        .expect("Customized Bounce dialog must emit BounceRequest");
    assert!(custom_req.create_part);
    assert!(matches!(
        custom_req.target,
        BounceTarget::RackZone {
            instrument,
            zone_index: None,
            midi: Some(72),
            ..
        } if instrument == inst_id
    ));

    // 3. PresetBrowser `R` reloads newly added external presets, and `s` saves original patch with Category & Name!
    let orig_patch = app.instrument(inst_id).unwrap().rack.clone();
    app.press_keys("P").unwrap();
    assert_eq!(app.mode_name(), "PRESETS");
    // Drop a brand-new external preset onto disk while PresetBrowser is open and press `R`!
    let ext_preset = cadence_core::presets::InstrumentPreset {
        version: cadence_core::presets::INSTRUMENT_PRESET_VERSION,
        name: "LiveReloadedSynth".into(),
        category: Some("Synth".into()),
        patch: cadence_core::rack::RackPatch::default_synth(),
    };
    std::fs::write(
        presets_dir.join("live-reloaded.preset.json"),
        ext_preset.to_json_bytes().unwrap(),
    )
    .unwrap();
    app.press_keys("R").unwrap();
    assert_eq!(app.mode_name(), "PRESETS");
    assert!(
        app.preset_browser_view()
            .unwrap()
            .items
            .iter()
            .any(|i| i.name == "LiveReloadedSynth"),
        "Pressing R inside PresetBrowser must reload external presets immediately"
    );
    // Move cursor to preview a different preset, then press `s` to save the ORIGINAL untouched patch!
    app.press_keys("js").unwrap();
    assert_eq!(app.mode_name(), "WORKFLOW");
    assert_eq!(app.authoring_view().unwrap().title, "SAVE PRESET");
    assert_eq!(
        app.instrument(inst_id).unwrap().rack,
        orig_patch,
        "Pressing s inside PresetBrowser must restore original patch before saving"
    );
    app.press_keys("MyCustomLead<Enter>").unwrap(); // Name
    app.press_keys("u").unwrap(); // Category = Modular -> submits!
    assert_eq!(app.mode_name(), "NORMAL");
    let saved_file = presets_dir.join("mycustomlead.preset.json");
    assert!(
        saved_file.exists(),
        "Save Preset must write <asset_base>/presets/<slug>.preset.json"
    );
    let parsed: cadence_core::presets::InstrumentPreset =
        serde_json::from_slice(&std::fs::read(&saved_file).unwrap()).unwrap();
    assert_eq!(parsed.name, "MyCustomLead");
    assert_eq!(parsed.category.as_deref(), Some("Modular"));
    assert_eq!(parsed.patch, orig_patch);

    // 4. Assets overlay (`Mode::Assets`) `r` triggers Action::ReloadSamples!
    app.execute_command("assets").unwrap();
    assert_eq!(app.mode_name(), "ASSETS");
    let reload_outs = app.press_keys("r").unwrap();
    assert!(
        reload_outs.iter().any(|o| o.samples_changed)
            || app.status.to_ascii_lowercase().contains("sample"),
        "Pressing r in Assets overlay must trigger Action::ReloadSamples (status: {})",
        app.status
    );
    app.press_keys("<Esc>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
}
