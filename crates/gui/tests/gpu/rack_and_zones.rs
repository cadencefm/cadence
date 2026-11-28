use crate::common::{
    build_snapshot, load_demo_project, render_checked, repo_root, with_gpu, write_test_wav,
    TempTestDir, DEFAULT_H, DEFAULT_W, STATUS,
};
use cadence_app::view::{DeckView, InstrumentVisualView, ModuleVisualView, OverlayView, RackTab};
use cadence_app::{Action, App};
use cadence_core::config::ThemeId;
use cadence_core::dsp::env::EnvMode;
use cadence_core::material::{
    Modulation, ModulationTarget, Pattern, PatternKind, Project, ValuePoint, ValueUnit,
};
use cadence_core::mixer::DriveMode;
use cadence_core::params::{InstrumentParam, ValueMode};
use cadence_core::presets::PresetCatalog;
use cadence_core::rack::{
    DriveModule, EnvModule, FilterMode, FilterModule, InPort, LfoModule, LfoShape, LoFiModule,
    ModuleKind, ModuleParam, NoiseColor, NoiseModule, OutModule, PanelPin, PatchCable,
    PitchTracking, RackModule, RackPatch, RackZone, ResonatorMode, ResonatorModule, SamplerModule,
    TriggerMode, VcaModule, VcoModule, VoiceSettings, Waveform,
};
use cadence_core::runtime::MidiMessage;
use cadence_core::sampler::{SampleLibrary, SampleLoopMode};
use cadence_gui::decks::build_display_list;
use cadence_gui::theme::Theme;

/// Showcase Eurorack modular Rack containing all 11 module kinds + a feedback `z-1` cable.
fn build_full_modular_rack(sample_rel_path: &str) -> RackPatch {
    let zone = RackZone {
        id: 1,
        name: "Lead Voice".into(),
        low_midi: 0,
        high_midi: 127,
        root_midi: 60,
        low_vel: 0.0,
        high_vel: 1.0,
        gain_db: 0.0,
        pan: 0.0,
        choke_group: None,
        voice: VoiceSettings::default(),
        modules: vec![
            RackModule::new(
                1,
                "VCO 1",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Saw,
                    tune_semitones: 0.0,
                    fine_cents: 0.0,
                    pulse_width: 0.5,
                    phase_deg: 0.0,
                    level: 0.9,
                    unison_voices: 3,
                    unison_detune_cents: 16.0,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "FM Mod",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    tune_semitones: 12.0,
                    level: 0.7,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                3,
                "Brass Smp",
                ModuleKind::Sampler(SamplerModule {
                    path: Some(sample_rel_path.into()),
                    sample_start: 12000,
                    sample_end: 58000,
                    loop_mode: SampleLoopMode::Forward,
                    loop_start: 18000,
                    loop_end: 48000,
                    level: 0.75,
                    ..SamplerModule::default()
                }),
            ),
            RackModule::new(
                4,
                "Pink Air",
                ModuleKind::Noise(NoiseModule {
                    color: NoiseColor::Pink,
                    highpass_hz: 320.0,
                    lowpass_hz: 8200.0,
                    level: 0.25,
                }),
            ),
            RackModule::new(
                5,
                "Clap Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::HoldDecay,
                    attack_ms: 1.5,
                    hold_ms: 4.0,
                    decay_ms: 140.0,
                    sustain: 0.0,
                    release_ms: 60.0,
                    trigger_mode: TriggerMode::OneShot,
                    delay_ms: 0.0,
                    bursts: 4,
                    burst_spacing_ms: 12.0,
                }),
            ),
            RackModule::new(
                6,
                "Sync LFO",
                ModuleKind::Lfo(LfoModule {
                    shape: LfoShape::Triangle,
                    rate_hz: 2.4,
                    sync: true,
                    division_steps: 4.0,
                    retrigger: true,
                    bipolar: true,
                }),
            ),
            RackModule::new(
                7,
                "Diode Filter",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::DiodeLadder,
                    cutoff_hz: 1150.0,
                    resonance: 0.62,
                    drive: 0.25,
                    kerr_semitones: 4.0,
                    spread: 0.55,
                    key_tracking: 0.4,
                    vel_amount: 0.2,
                    mix: 0.92,
                }),
            ),
            RackModule::new(
                8,
                "Shock Drive",
                ModuleKind::Drive(DriveModule {
                    mode: DriveMode::ShockPort,
                    drive: 0.68,
                    asymmetry: 0.35,
                    tone_hz: 9500.0,
                    output_db: -1.5,
                    mix: 0.85,
                }),
            ),
            RackModule::new(
                9,
                "Crush LoFi",
                ModuleKind::LoFi(LoFiModule {
                    bits: 3.5,
                    rate_hz: 8000.0,
                    mix: 0.90,
                }),
            ),
            RackModule::new(
                10,
                "Ring VCA",
                ModuleKind::Vca(VcaModule {
                    gain: 1.0,
                    bias: 0.0,
                }),
            ),
            RackModule::new(
                12,
                "Bronze Plate",
                ModuleKind::Resonator(ResonatorModule {
                    mode: ResonatorMode::Plate,
                    pitch_tracking: PitchTracking::Fixed,
                    tune_semitones: 0.0,
                    fine_cents: 0.0,
                    fixed_hz: 520.0,
                    structure: 0.48,
                    decay_ms: 1_250.0,
                    brightness: 0.74,
                    position: 0.58,
                    mix: 0.80,
                }),
            ),
            RackModule::new(
                11,
                "Out",
                ModuleKind::Out(OutModule {
                    gain_db: -1.2,
                    pan: -0.15,
                    spread: 0.35,
                }),
            ),
        ],
        cables: vec![
            PatchCable::new(1, 1, 7, InPort::In, 0.90),
            PatchCable::new(2, 2, 1, InPort::FmIn, 0.45),
            PatchCable::new(3, 3, 7, InPort::In, 0.60),
            PatchCable::new(4, 4, 8, InPort::In, 0.22),
            PatchCable::new(5, 6, 1, InPort::PitchCv, 2.0),
            PatchCable::new(6, 5, 7, InPort::Cv, 2.5),
            PatchCable::new(7, 6, 5, InPort::TrigIn, 1.0),
            PatchCable::new(8, 7, 8, InPort::In, 1.0),
            PatchCable::new(9, 8, 9, InPort::In, 1.0),
            PatchCable::new(10, 9, 10, InPort::In, 1.0),
            PatchCable::new(11, 2, 10, InPort::Cv, 0.85),
            PatchCable::new(14, 10, 12, InPort::In, 0.85),
            PatchCable::new(12, 12, 11, InPort::In, 1.0),
            // Cycle back-edge rendered as dashed `z-1` feedback cable
            PatchCable::new(13, 8, 1, InPort::FmIn, -0.32),
        ],
    };
    RackPatch {
        kit_name: String::new(),
        zones: vec![zone],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 18_500.0,
        resonance: 0.0,
        panel_pins: vec![
            PanelPin {
                label: "Cutoff".into(),
                target: InstrumentParam::Module {
                    zone: 1,
                    module: 7,
                    param: ModuleParam::Cutoff,
                },
            },
            PanelPin {
                label: "Res".into(),
                target: InstrumentParam::Module {
                    zone: 1,
                    module: 7,
                    param: ModuleParam::Resonance,
                },
            },
            PanelPin {
                label: "FM Depth".into(),
                target: InstrumentParam::Cable { zone: 1, cable: 2 },
            },
            PanelPin {
                label: "Drive".into(),
                target: InstrumentParam::Module {
                    zone: 1,
                    module: 8,
                    param: ModuleParam::Drive,
                },
            },
            PanelPin {
                label: "Bits".into(),
                target: InstrumentParam::Module {
                    zone: 1,
                    module: 9,
                    param: ModuleParam::Bits,
                },
            },
            PanelPin {
                label: "Out Gain".into(),
                target: InstrumentParam::Module {
                    zone: 1,
                    module: 11,
                    param: ModuleParam::GainDb,
                },
            },
        ],
    }
}

#[test]
fn gpu_renders_rack_panel_all_modules_patchbay_and_midi_learn_badges() {
    with_gpu(DEFAULT_W, DEFAULT_H, |gpu| {
        let dark = Theme::cadence_dark();
        let light = Theme::studio_light();
        let (mut project, demo_path) = load_demo_project();

        let inst0_id = project.instruments[0].id;
        project.instruments[0].name = "Eurorack Lead".into();
        project.instruments[0].rack = build_full_modular_rack("samples/brass-pad.wav");
        project.modulations.retain(|m| match m.target {
            ModulationTarget::Instrument { instrument, .. } => instrument != inst0_id,
            _ => true,
        });
        for part in &mut project.parts {
            if part.instrument == inst0_id {
                part.parameter_links.clear();
            }
        }
        for m in &mut project.macros {
            m.bindings
                .retain(|b| b.target.as_instrument().map(|(id, _)| id) != Some(inst0_id));
        }
        let curve_id = project.max_id() + 1;
        let mod_id = curve_id + 1;
        project.patterns.push(Pattern {
            id: curve_id,
            name: "FilterSweep".into(),
            kind: PatternKind::Values {
                unit: ValueUnit::Normalized,
                period: 32,
                mode: ValueMode::Ramp,
                points: vec![
                    ValuePoint {
                        step: 0,
                        value: 0.25,
                    },
                    ValuePoint {
                        step: 16,
                        value: 0.82,
                    },
                ],
            },
        });
        project.modulations.push(Modulation {
            id: mod_id,
            name: "LeadSweep".into(),
            pattern: curve_id,
            target: ModulationTarget::Instrument {
                instrument: inst0_id,
                parameter: InstrumentParam::Module {
                    zone: 1,
                    module: 7,
                    param: ModuleParam::Cutoff,
                },
            },
        });
        project.validate().expect("modular showcase validates");

        let snap = build_snapshot(&project, 16);
        let lib = SampleLibrary::from_roots(&[], Some(&demo_path));
        let mut app = App::new_with_library(project, demo_path.clone(), lib).expect("init app");

        // 1. PinnedOverview (Dark & Light) + clean H/L Instrument hints (no redundant `-/=`).
        app.apply(Action::SelectInstrument {
            instrument: inst0_id,
        })
        .unwrap();
        app.press_keys("zz").unwrap();
        app.rack_ui.tab = RackTab::Panel;
        app.rack_ui.panel_slot = 0;

        let fv_pinned = app.frame_view(&snap, STATUS);
        let iv_pinned = fv_pinned
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Instrument(iv) => Some(iv),
                _ => None,
            })
            .expect("InstrumentDeckView");
        match &iv_pinned.visual {
            InstrumentVisualView::Rack(r) => match &r.visual {
                ModuleVisualView::PinnedOverview {
                    pin_count,
                    filter_mod_cutoff_norm,
                    ..
                } => {
                    assert_eq!(*pin_count, 6);
                    assert!(
                        filter_mod_cutoff_norm.is_some(),
                        "PinnedOverview Filter curve must display live modulated cutoff"
                    );
                }
                other => panic!("expected PinnedOverview, got {other:?}"),
            },
            other => panic!("expected Rack, got {other:?}"),
        }
        let dl_pinned =
            build_display_list(&fv_pinned, DEFAULT_W as f32, DEFAULT_H as f32, 1.0, &dark);
        assert!(dl_pinned.base.texts.iter().any(|t| t.text.contains("H/L")));
        assert!(!dl_pinned.base.texts.iter().any(|t| t.text.contains("-/=")));

        let px_pinned_dark = render_checked(
            gpu,
            "rack-01-panel-pinned-dark.png",
            &fv_pinned,
            &dark,
            0.20,
        );
        let mut fv_pinned_light = fv_pinned;
        fv_pinned_light.chrome.theme = ThemeId::StudioLight;
        let px_pinned_light = render_checked(
            gpu,
            "rack-02-panel-pinned-light.png",
            &fv_pinned_light,
            &light,
            0.20,
        );
        assert_ne!(px_pinned_dark, px_pinned_light);

        // 2. All 11 module faceplates render distinctly (including FM-active VCO & Resonator).
        let module_cases: &[(&str, &str)] = &[
            ("VCO 1", "rack-03-panel-vco-fm.png"),
            ("Brass Smp", "rack-04-panel-sampler.png"),
            ("Pink Air", "rack-05-panel-noise.png"),
            ("Clap Env", "rack-06-panel-env-burst.png"),
            ("Sync LFO", "rack-07-panel-lfo.png"),
            ("Diode Filter", "rack-08-panel-filter-mod.png"),
            ("Shock Drive", "rack-09-panel-drive.png"),
            ("Crush LoFi", "rack-10-panel-lofi.png"),
            ("Ring VCA", "rack-11-panel-vca-ringmod.png"),
            ("Bronze Plate", "rack-12-panel-resonator.png"),
            ("Out", "rack-13-panel-out.png"),
        ];
        let mut prev_px = px_pinned_dark;
        for &(mod_name, file) in module_cases {
            let slot = {
                let probe = app.frame_view(&snap, STATUS);
                probe
                    .decks
                    .iter()
                    .find_map(|d| match &d.view {
                        DeckView::Instrument(iv) => match &iv.visual {
                            InstrumentVisualView::Rack(r) => {
                                r.modules.iter().position(|p| p.label == mod_name)
                            }
                            _ => None,
                        },
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("module pill '{mod_name}' missing"))
            };
            app.rack_ui.panel_slot = slot;
            let fv = app.frame_view(&snap, STATUS);
            if mod_name == "VCO 1" {
                let is_fm = fv.decks.iter().any(|d| match &d.view {
                    DeckView::Instrument(iv) => match &iv.visual {
                        InstrumentVisualView::Rack(r) => matches!(
                            &r.visual,
                            ModuleVisualView::Vco { fm_active: true, label, curve, .. }
                                if label.contains("FM") && curve.len() >= 96
                        ),
                        _ => false,
                    },
                    _ => false,
                });
                assert!(is_fm, "VCO 1 must display active FM curve & FM label");
            }
            let px = render_checked(gpu, file, &fv, &dark, 0.20);
            assert_ne!(px, prev_px, "{file} must render distinctly from previous");
            prev_px = px;
        }

        // 3. Eurorack Patchbay with live module/cable activity + selected dashed `z-1` feedback cable (Dark & Light).
        app.rack_ui.tab = RackTab::Patchbay;
        app.rack_ui.patchbay_cable_id = Some(13);
        let mut pb_snap = snap.clone();
        if let Some((inst_id, zone_id)) = app.active_patchbay_target() {
            let mut tel = cadence_core::runtime::RackActivityTelemetry::for_zone(inst_id, zone_id);
            for (i, peak) in [
                0.88_f32, 0.72, 0.45, 0.92, 0.64, 0.78, 1.12, 0.56, 0.68, 0.74, 0.82,
            ]
            .into_iter()
            .enumerate()
            {
                tel.set_module_peak((i + 1) as u16, peak);
            }
            for (i, peak) in [
                0.84_f32, 0.66, 0.42, 0.75, 0.58, 0.88, 0.72, 0.61, 0.52, 0.79, 0.64, 0.70, 0.68,
            ]
            .into_iter()
            .enumerate()
            {
                tel.set_cable_peak((i + 1) as u16, peak);
            }
            pb_snap.set_rack_activity(tel);
        }
        let fv_pb = app.frame_view(&pb_snap, STATUS);
        let dl_pb = build_display_list(&fv_pb, DEFAULT_W as f32, DEFAULT_H as f32, 1.0, &dark);
        assert!(
            dl_pb.base.texts.iter().any(|t| t.text.contains("z-1")),
            "Patchbay must display ASCII z-1 feedback indicator"
        );
        match &fv_pb
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Instrument(iv) => Some(iv),
                _ => None,
            })
            .expect("Patchbay InstrumentDeckView")
            .visual
        {
            InstrumentVisualView::Rack(r) => {
                assert_eq!(r.active_tab, RackTab::Patchbay);
                assert!(r.patchbay.has_feedback);
                assert_eq!(r.patchbay.feedback_badge, "z-1");
                assert!(r.patchbay.cables.iter().any(|c| c.feedback && c.selected));
                assert!(
                    r.patchbay.nodes.iter().any(|n| n.activity > 0.5)
                        && r.patchbay.cables.iter().any(|c| c.activity > 0.5),
                    "Patchbay view must surface live module and cable activity peaks"
                );
            }
            other => panic!("expected Rack Patchbay, got {other:?}"),
        }
        let px_pb_dark = render_checked(gpu, "rack-14-patchbay-dark.png", &fv_pb, &dark, 0.20);
        let mut fv_pb_light = fv_pb;
        fv_pb_light.chrome.theme = ThemeId::StudioLight;
        let px_pb_light = render_checked(
            gpu,
            "rack-15-patchbay-light.png",
            &fv_pb_light,
            &light,
            0.20,
        );
        assert_ne!(px_pb_dark, px_pb_light);

        // 4. Rack Instrument Deck with live `[M1·CC86]` badges + armed `[LEARN…]` pill (`M`).
        let (midi_proj, _) = load_demo_project();
        let midi_lib = SampleLibrary::from_roots(&[], Some(&demo_path));
        let mut midi_app = App::new_with_library(midi_proj, demo_path, midi_lib).unwrap();
        midi_app
            .execute_command("midi profile minilab3_daw")
            .unwrap();
        midi_app.config.midi_pickup = false;
        let _ = midi_app.handle_midi(MidiMessage::Control {
            channel: 0,
            controller: 86,
            value: 92,
        });
        let _ = midi_app.handle_midi(MidiMessage::Control {
            channel: 0,
            controller: 87,
            value: 74,
        });
        midi_app
            .execute_command("instrument \"Anthem Lead\"")
            .unwrap();
        midi_app.press_keys("M").unwrap();
        render_checked(
            gpu,
            "rack-16-midi-learn-badges.png",
            &midi_app.frame_view(&snap, STATUS),
            &dark,
            0.25,
        );
    });
}

#[test]
fn gpu_renders_rack_zones_tr909_full_keyboard_and_preset_browser() {
    with_gpu(DEFAULT_W, DEFAULT_H, |gpu| {
        let dark = Theme::cadence_dark();
        let light = Theme::studio_light();
        let tmp = TempTestDir::new("rack-zones");
        let save_path = tmp.path().join("project.json");
        write_test_wav(&tmp.path().join("samples/synth-kick.wav"), 4_410, 44_100);
        let lib = SampleLibrary::from_prefixed_roots(
            &[(Some("factory-samples"), repo_root().join("factory-samples"))],
            Some(&save_path),
            true,
        );
        let mut app = App::new_with_library(Project::demo(), save_path, lib).unwrap();
        app.set_preset_catalog(PresetCatalog::from_roots(&[repo_root().join("presets")]));

        // 1. Multi-zone TR-909 kit (Split + Zoomed Dark & Light).
        app.execute_command("kit Kit909 tr-909").unwrap();
        app.execute_command("instrument Kit909").unwrap();
        app.rack_ui.tab = RackTab::Zones;
        app.execute_command("zone select CH").unwrap();
        let kit_snap = build_snapshot(app.project(), 16);

        let px_split = render_checked(
            gpu,
            "rack-17-zones-tr909-split.png",
            &app.frame_view(&kit_snap, STATUS),
            &dark,
            0.20,
        );
        app.press_keys("zz").unwrap();
        let fv_z = app.frame_view(&kit_snap, STATUS);
        match &fv_z
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Instrument(iv) => Some(iv),
                _ => None,
            })
            .expect("Zones InstrumentDeckView")
            .visual
        {
            InstrumentVisualView::Rack(r) => {
                assert_eq!(r.active_tab, RackTab::Zones);
                assert!(r.zone_count >= 8, "TR-909 kit should have >= 8 zones");
                assert!(r.zones.iter().any(|z| z.selected && z.name == "CH"));
            }
            other => panic!("expected Rack Zones, got {other:?}"),
        }
        let px_z_dark = render_checked(gpu, "rack-18-zones-tr909-dark.png", &fv_z, &dark, 0.20);
        let mut fv_z_light = fv_z;
        fv_z_light.chrome.theme = ThemeId::StudioLight;
        let px_z_light = render_checked(
            gpu,
            "rack-19-zones-tr909-light.png",
            &fv_z_light,
            &light,
            0.20,
        );
        assert_ne!(px_split, px_z_dark);
        assert_ne!(px_z_dark, px_z_light);

        // 2. Full-keyboard (0..127) overlapping velocity + key split instrument (Dark & Light).
        app.execute_command("instrument FullKeys synth").unwrap();
        app.rack_ui.tab = RackTab::Zones;
        app.execute_command("zone name WarmPad").unwrap();
        app.execute_command("zone range 0 127 60").unwrap();
        app.execute_command("zone vel 0% 55%").unwrap();
        app.execute_command("zone add SubBass 36 synth").unwrap();
        app.execute_command("zone range C0 B2 36").unwrap();
        app.execute_command("zone vel 50% 100%").unwrap();
        app.execute_command("zone add LeadKeys 60 synth").unwrap();
        app.execute_command("zone range C3 B5 60").unwrap();
        app.execute_command("zone vel 55% 100%").unwrap();
        app.execute_command("zone add AirBell 96 sample samples/synth-kick.wav")
            .unwrap();
        app.execute_command("zone range C6 G9 96").unwrap();
        app.execute_command("zone vel 40% 100%").unwrap();
        app.execute_command("zone select LeadKeys").unwrap();

        let fv_full = app.frame_view(&kit_snap, STATUS);
        let px_full_dark = render_checked(
            gpu,
            "rack-20-zones-full-keyboard-dark.png",
            &fv_full,
            &dark,
            0.25,
        );
        let px_full_light = render_checked(
            gpu,
            "rack-21-zones-full-keyboard-light.png",
            &fv_full,
            &light,
            0.25,
        );
        assert_ne!(px_full_dark, px_full_light);

        // 3. Preset Browser (`Pl`) with all 6 category filter pills (`All..Modular`, Dark & Light).
        app.press_keys("Pl").unwrap();
        let fv_pres = app.frame_view(&kit_snap, STATUS);
        match &fv_pres.overlay {
            Some(OverlayView::PresetBrowser(pb)) => {
                assert_eq!(pb.engine_label, "RACK");
                assert_eq!(pb.active_category, "Synth");
                assert_eq!(pb.categories.len(), 6);
            }
            other => panic!("expected PresetBrowser overlay, got {other:?}"),
        }
        let px_pres_dark = render_checked(
            gpu,
            "rack-22-preset-browser-dark.png",
            &fv_pres,
            &dark,
            0.20,
        );
        let mut fv_pres_light = fv_pres;
        fv_pres_light.chrome.theme = ThemeId::StudioLight;
        let px_pres_light = render_checked(
            gpu,
            "rack-23-preset-browser-light.png",
            &fv_pres_light,
            &light,
            0.20,
        );
        assert_ne!(px_pres_dark, px_pres_light);
    });
}
