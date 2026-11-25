//! Headless Metal/wgpu GPU integration tests for the Unified Eurorack Modular Rack
//! Instrument Deck (`Panel`, `Patchbay`, `Zones`), Preset Browser category pills, and
//! Project Signal Graph (`gG`).
//!
//! Set `CADENCE_DUMP_RACK_DIR=/path/to/dir` to export 1440×860 PNG screenshots for visual inspection.

mod common;

use cadence_app::view::{
    DeckView, InstrumentVisualView, ModuleVisualView, OverlayView, RackTab, UiModeKind,
};
use cadence_app::App;
use cadence_core::config::ThemeId;
use cadence_core::dsp::env::EnvMode;
use cadence_core::material::{
    Modulation, ModulationTarget, Pattern, PatternKind, ValuePoint, ValueUnit,
};
use cadence_core::mixer::DriveMode;
use cadence_core::params::{InstrumentParam, ValueMode};
use cadence_core::presets::PresetCatalog;
use cadence_core::rack::{
    DriveModule, EnvModule, FilterMode, FilterModule, InPort, LfoModule, LfoShape, LoFiModule,
    MobiusDispersion, MobiusExciter, MobiusModule, MobiusTopology, ModuleKind, ModuleParam,
    NoiseColor, NoiseModule, OutModule, PanelPin, PatchCable, PitchTracking, RackModule, RackPatch,
    RackZone, ResonatorMode, ResonatorModule, SamplerModule, TriggerMode, VcaModule, VcoModule,
    VoiceSettings, Waveform,
};
use cadence_core::runtime::AudioSnapshot;
use cadence_core::sampler::SampleLoopMode;
use cadence_gui::decks::build_display_list;
use cadence_gui::headless::{distinct_pixel_ratio, HeadlessGpu};
use cadence_gui::theme::Theme;
use std::path::PathBuf;

const W: u32 = 1440;
const H: u32 = 860;
const STATUS: &str = "Metal · 48 kHz";

fn maybe_save_png(name: &str, rgba: &[u8]) {
    common::maybe_save_png("CADENCE_DUMP_RACK_DIR", name, W, H, rgba);
}

/// Build a showcase Eurorack modular Rack containing ALL 11 module kinds + a feedback `z-1` cable
/// so every `ModuleVisualView` variant and every Patchbay cable kind renders from live state!
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
                "Acid Filter",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::AcidLowPass,
                    cutoff_hz: 1150.0,
                    resonance: 0.62,
                    drive: 0.25,
                    key_tracking: 0.4,
                    vel_amount: 0.2,
                    mix: 0.92,
                }),
            ),
            RackModule::new(
                8,
                "Fold Drive",
                ModuleKind::Drive(DriveModule {
                    mode: DriveMode::Fold,
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
                13,
                "Mobius Choir",
                ModuleKind::Mobius(MobiusModule {
                    exciter: MobiusExciter::Dipole,
                    brightness: 0.84,
                    shape: 0.68,
                    chirality: 0.22,
                    texture: 0.30,
                    atoms: 4,
                    dispersion: MobiusDispersion::Formant,
                    structure: 0.50,
                    stiffness: 0.22,
                    stiffness_tilt: -1.8,
                    kerr_semitones: 2.2,
                    kerr_spread: -0.65,
                    topology: MobiusTopology::Star,
                    coupling: 0.84,
                    distance_ms: 24.0,
                    sympathy: 0.55,
                    feedback: 0.0,
                    ..MobiusModule::default()
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
            PatchCable::new(1, 1, 7, InPort::In, 0.90), // VCO 1 -> Acid Filter (Audio)
            PatchCable::new(2, 2, 1, InPort::FmIn, 0.45), // FM Mod -> VCO 1 (FM)
            PatchCable::new(3, 3, 7, InPort::In, 0.60), // Brass Smp -> Acid Filter (Audio)
            PatchCable::new(4, 4, 8, InPort::In, 0.22), // Pink Air -> Fold Drive (Audio)
            PatchCable::new(5, 6, 1, InPort::PitchCv, 2.0), // Sync LFO -> VCO 1 (Pitch)
            PatchCable::new(6, 5, 7, InPort::Cv, 2.5),  // Clap Env -> Acid Filter (CV)
            PatchCable::new(7, 6, 5, InPort::TrigIn, 1.0), // Sync LFO -> Clap Env (Gate)
            PatchCable::new(8, 7, 8, InPort::In, 1.0),  // Acid Filter -> Fold Drive (Audio)
            PatchCable::new(9, 8, 9, InPort::In, 1.0),  // Fold Drive -> Crush LoFi (Audio)
            PatchCable::new(10, 9, 10, InPort::In, 1.0), // Crush LoFi -> Ring VCA (Audio)
            PatchCable::new(11, 2, 10, InPort::Cv, 0.85), // FM Mod -> Ring VCA (RingMod CV!)
            PatchCable::new(14, 10, 12, InPort::In, 0.85), // Ring VCA -> Bronze Plate (Audio)
            PatchCable::new(15, 13, 12, InPort::In, 0.55), // Mobius Choir -> Bronze Plate (Audio)
            PatchCable::new(12, 12, 11, InPort::In, 1.0), // Bronze Plate -> Out (Audio)
            // Cross-column feedback loop: Fold Drive (col 2) -> VCO 1.FmIn (col 0) (cycle back-edge -> z-1 dashed!)
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

fn render_checked(
    gpu: &mut HeadlessGpu,
    app: &App,
    snap: &AudioSnapshot,
    theme_id: ThemeId,
    filename: &str,
) -> Vec<u8> {
    let mut fv = app.frame_view(snap, STATUS);
    fv.chrome.theme = theme_id;
    let theme = Theme::from_id(theme_id);
    let dl = build_display_list(&fv, W as f32, H as f32, 1.0, &theme);
    assert_eq!(
        dl.current_clip(),
        dl.viewport,
        "{filename}: unbalanced clip stack"
    );
    // Embedded JetBrains Mono lacks superscript minus U+207B; ensure zero tofu glyphs!
    for t in dl.base.texts.iter().chain(dl.overlay.texts.iter()) {
        assert!(
            !t.text.contains('\u{207B}'),
            "{filename}: emitted unsupported U+207B glyph in {:?}",
            t.text
        );
    }
    let rgba = gpu.render_rgba8(&fv, &theme).expect("render_rgba8");
    assert!(
        distinct_pixel_ratio(&rgba, theme.surface.canvas) > 0.20,
        "{filename} rendered too few visible pixels"
    );
    maybe_save_png(filename, &rgba);
    rgba
}

#[test]
fn gpu_renders_rack_panel_patchbay_zones_preset_categories_and_signal_graph() {
    let Ok(mut gpu) = HeadlessGpu::new(W, H) else {
        // Gracefully skip on GPU-less CI containers.
        return;
    };

    let demo_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/demo/project.json");
    let presets_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../presets");
    let mut project = cadence_app::load_project(&demo_path).expect("load demo project");

    // Replace instrument 0's Rack with our full 11-module Eurorack showcase + live cutoff mod!
    let inst0_id = project.instruments[0].id;
    project.instruments[0].name = "Eurorack Lead".into();
    project.instruments[0].rack = build_full_modular_rack("samples/brass-pad.wav");
    // Remove any stale modulations referencing instrument 0's previous module IDs, then add fresh cutoff modulation.
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

    let snap = common::build_snapshot(&project, 16);
    let mut app = App::new(project, demo_path).expect("init app");
    app.set_preset_catalog(PresetCatalog::from_roots(std::slice::from_ref(
        &presets_root,
    )));

    // -------------------------------------------------------------------------
    // 1. Instrument Deck `Panel`: PinnedOverview + ALL individual module faceplates
    // -------------------------------------------------------------------------
    app.apply(cadence_app::Action::SelectInstrument {
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
    let px_pinned_dark = render_checked(
        &mut gpu,
        &app,
        &snap,
        ThemeId::CadenceDark,
        "01-rack-panel-pinned-dark.png",
    );
    let px_pinned_light = render_checked(
        &mut gpu,
        &app,
        &snap,
        ThemeId::StudioLight,
        "02-rack-panel-pinned-light.png",
    );
    assert_ne!(px_pinned_dark, px_pinned_light);

    // Step through every module pill (VCO, Sampler, Noise, Env, LFO, Filter, Drive, LoFi, VCA, Resonator, Out)!
    let module_cases: &[(&str, &str)] = &[
        ("VCO 1", "03-rack-panel-vco-fm.png"),
        ("Brass Smp", "04-rack-panel-sampler.png"),
        ("Pink Air", "05-rack-panel-noise.png"),
        ("Clap Env", "06-rack-panel-env-burst.png"),
        ("Sync LFO", "07-rack-panel-lfo.png"),
        ("Acid Filter", "08-rack-panel-filter-mod.png"),
        ("Fold Drive", "09-rack-panel-drive.png"),
        ("Crush LoFi", "10-rack-panel-lofi.png"),
        ("Ring VCA", "11-rack-panel-vca-ringmod.png"),
        ("Bronze Plate", "12b-rack-panel-resonator.png"),
        ("Mobius Choir", "12c-rack-panel-mobius.png"),
        ("Out", "12-rack-panel-out.png"),
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
        let px = render_checked(&mut gpu, &app, &snap, ThemeId::CadenceDark, file);
        assert_ne!(px, prev_px, "{file} must render distinctly from previous");
        prev_px = px;
    }

    // -------------------------------------------------------------------------
    // 2. Instrument Deck `Patchbay`: multi-column Eurorack graph + attenuverter pills
    //    + selected cable halo + dashed `z-1` feedback cable (Dark & Light)
    // -------------------------------------------------------------------------
    app.rack_ui.tab = RackTab::Patchbay;
    app.rack_ui.patchbay_cable_id = Some(13); // Select the `z-1` feedback cable!
    let fv_pb = app.frame_view(&snap, STATUS);
    let dl_pb = build_display_list(&fv_pb, W as f32, H as f32, 1.0, &Theme::cadence_dark());
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
        }
        other => panic!("expected Rack Patchbay, got {other:?}"),
    }
    let px_pb_dark = render_checked(
        &mut gpu,
        &app,
        &snap,
        ThemeId::CadenceDark,
        "13-rack-patchbay-dark.png",
    );
    let px_pb_light = render_checked(
        &mut gpu,
        &app,
        &snap,
        ThemeId::StudioLight,
        "14-rack-patchbay-light.png",
    );
    assert_ne!(px_pb_dark, px_pb_light);

    // -------------------------------------------------------------------------
    // 3. Instrument Deck `Zones`: multi-zone TR-909 kit Rack (Split + Zoomed Dark & Light)
    // -------------------------------------------------------------------------
    app.execute_command("kit Kit909 tr-909").unwrap();
    app.execute_command("instrument Kit909").unwrap();
    app.rack_ui.tab = RackTab::Zones;
    app.execute_command("zone select CH").unwrap();
    let kit_snap = common::build_snapshot(app.project(), 16);
    // 3a. Compact split-pane view verifying tight height & clean 2-col controls (zero orphan headers).
    let px_z_split = render_checked(
        &mut gpu,
        &app,
        &kit_snap,
        ThemeId::CadenceDark,
        "15a-rack-zones-tr909-split-dark.png",
    );
    // 3b. Zoomed view showing full TR-909 multi-zone Rack table + scroll indicator + live scope!
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
    let px_z_dark = render_checked(
        &mut gpu,
        &app,
        &kit_snap,
        ThemeId::CadenceDark,
        "15-rack-zones-tr909-dark.png",
    );
    let px_z_light = render_checked(
        &mut gpu,
        &app,
        &kit_snap,
        ThemeId::StudioLight,
        "16-rack-zones-tr909-light.png",
    );
    assert_ne!(px_z_split, px_z_dark);
    assert_ne!(px_z_dark, px_z_light);

    // -------------------------------------------------------------------------
    // 4. Preset Browser (`P`) with Category Filter Pills (`All / Synth / Sampler / Drums / Kit / Modular`)
    // -------------------------------------------------------------------------
    app.press_keys("Pl").unwrap(); // Open Preset Browser + cycle to `Synth` category pill!
    let fv_pres = app.frame_view(&snap, STATUS);
    match &fv_pres.overlay {
        Some(OverlayView::PresetBrowser(pb)) => {
            assert_eq!(pb.engine_label, "RACK");
            assert_eq!(pb.active_category, "Synth");
            assert_eq!(pb.categories.len(), 6);
        }
        other => panic!("expected PresetBrowser overlay, got {other:?}"),
    }
    let px_pres_dark = render_checked(
        &mut gpu,
        &app,
        &snap,
        ThemeId::CadenceDark,
        "17-preset-browser-categories-dark.png",
    );
    let px_pres_light = render_checked(
        &mut gpu,
        &app,
        &snap,
        ThemeId::StudioLight,
        "18-preset-browser-categories-light.png",
    );
    assert_ne!(px_pres_dark, px_pres_light);
    app.press_keys("<Esc>").unwrap();

    // -------------------------------------------------------------------------
    // 5. Project Signal Graph (`gG`)
    // -------------------------------------------------------------------------
    app.press_keys("gG").unwrap();
    let fv_graph = app.frame_view(&snap, STATUS);
    assert_eq!(fv_graph.chrome.mode_kind, UiModeKind::Normal);
    assert!(
        fv_graph
            .decks
            .iter()
            .any(|d| matches!(d.view, DeckView::Graph(_))),
        "gG must focus Project Signal Graph deck"
    );
    let px_graph = render_checked(
        &mut gpu,
        &app,
        &snap,
        ThemeId::CadenceDark,
        "19-signal-graph.png",
    );
    assert_ne!(px_graph, px_pb_dark);
}
