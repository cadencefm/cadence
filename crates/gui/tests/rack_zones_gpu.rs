//! Headless Metal/wgpu GPU visual tests for the Unified Eurorack Multi-Zone Kit Editor.
//!
//! Set `CADENCE_DUMP_ZONES_DIR=/path/to/dir` to save 1440×860 PNG screenshots for visual inspection.

mod common;

use cadence_app::view::{DeckView, InstrumentVisualView, ModuleVisualView, RackTab};
use cadence_app::App;
use cadence_core::material::Project;
use cadence_core::runtime::AudioSnapshot;
use cadence_core::sampler::SampleLibrary;
use cadence_gui::headless::{distinct_pixel_ratio, HeadlessGpu};
use cadence_gui::theme::Theme;
use std::path::PathBuf;

fn maybe_save_png(name: &str, width: u32, height: u32, rgba: &[u8]) {
    common::maybe_save_png("CADENCE_DUMP_ZONES_DIR", name, width, height, rgba);
}

#[test]
fn gpu_renders_rack_zone_editor_synth_sample_split_and_light() {
    let Ok(mut gpu) = HeadlessGpu::new(1440, 860) else {
        return;
    };

    let tmp = common::TempTestDir::new("rack-zones");
    let save_path = tmp.path().join("project.json");
    common::write_test_wav(&tmp.path().join("samples/synth-kick.wav"), 4_410, 44_100);
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let lib = SampleLibrary::from_prefixed_roots(
        &[(Some("factory-samples"), repo_root.join("factory-samples"))],
        Some(&save_path),
        true,
    );
    let mut app = App::new_with_library(Project::demo(), save_path, lib).unwrap();

    // Create a hybrid TR-808 Rack Kit with a synthetic Sample zone + velocity layers + choke group.
    app.execute_command("kit Studio808 tr808").unwrap();
    app.execute_command("zone add PunchKick 52 sample samples/synth-kick.wav")
        .unwrap();
    app.execute_command("zone range C2 C3 48").unwrap();
    app.execute_command("zone vel 20% 95%").unwrap();
    app.execute_command("zone set gain +1.5dB").unwrap();
    app.execute_command("zone set pan L20").unwrap();
    app.execute_command("zone choke 2").unwrap();
    app.rack_ui.tab = RackTab::Zones;
    assert!(app.ready(), "hybrid Rack Kit sample zone must load cleanly");

    let wave: Vec<f32> = (0..256)
        .map(|i| {
            let t = i as f32 / 256.0 * std::f32::consts::TAU;
            (t * 3.0).sin() * 0.55 * (1.0 - i as f32 / 256.0)
        })
        .collect();
    let snap = AudioSnapshot {
        playing: true,
        absolute_step: 4,
        step_fraction: 0.3,
        sample_rate: 48_000,
        peak: 0.72,
        peak_left: 0.72,
        peak_right: 0.68,
        waveform: wave,
        ..AudioSnapshot::default()
    };
    let dark = Theme::cadence_dark();
    let light = Theme::studio_light();
    let status = "Metal · 48 kHz";

    // 1. Compact split-pane view (Arrange + Bank + Output + Rack Zones tab) on Sample zone!
    let fv_split = app.frame_view(&snap, status);
    let inst_view = fv_split
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Instrument(iv) => Some(iv),
            _ => None,
        })
        .expect("InstrumentDeckView in split pane");
    match &inst_view.visual {
        InstrumentVisualView::Rack(rack) => {
            assert_eq!(rack.active_tab, RackTab::Zones);
            let sel = rack
                .zones
                .iter()
                .find(|z| z.selected)
                .expect("selected zone");
            assert_eq!(sel.name, "PunchKick");
            assert!(sel.has_sampler);
            assert_eq!(sel.source_tag, "SAMPLER");
            assert!(rack.selected_zone_summary.contains("PunchKick"));
            assert!(rack.selected_zone_summary.contains("Choke 2"));
        }
        other => panic!("expected Rack visual, got {other:?}"),
    }
    let px_split = gpu.render_rgba8(&fv_split, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px_split, dark.surface.canvas) > 0.25);
    maybe_save_png("01-rack-zones-split-pane.png", 1440, 860, &px_split);

    // 2. Zoomed Rack Panel view on Sample zone (pill 1 = Sampler after pill 0 = Rack overview!).
    app.press_keys("zz").unwrap();
    app.rack_ui.tab = RackTab::Panel;
    app.rack_ui.panel_slot = 1;
    let fv_sample = app.frame_view(&snap, status);
    let inst_sample = fv_sample
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Instrument(iv) => Some(iv),
            _ => None,
        })
        .expect("InstrumentDeckView in sample Panel");
    match &inst_sample.visual {
        InstrumentVisualView::Rack(rack) => {
            assert!(
                matches!(rack.visual, ModuleVisualView::Sampler { .. }),
                "expected Sampler visual on sample zone panel, got {:?}",
                rack.visual
            );
        }
        other => panic!("expected Rack visual, got {other:?}"),
    }
    let px_sample = gpu.render_rgba8(&fv_sample, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px_sample, dark.surface.canvas) > 0.25);
    maybe_save_png(
        "02-rack-zones-sample-zone-zoomed.png",
        1440,
        860,
        &px_sample,
    );

    // 3. Zoomed Rack Zones view on Synth zone (CH with choke group 1).
    app.rack_ui.tab = RackTab::Zones;
    app.execute_command("zone select CH").unwrap();
    let zone_group_idx = {
        let fv_probe = app.frame_view(&snap, status);
        let iv = fv_probe
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Instrument(iv) => Some(iv),
                _ => None,
            })
            .expect("InstrumentDeckView");
        iv.controls
            .iter()
            .position(|c| c.module == "ZONE")
            .expect("ZONE control group in Synth zone")
    };
    app.control_cursor = zone_group_idx;
    let fv_synth = app.frame_view(&snap, status);
    let inst_synth = fv_synth
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Instrument(iv) => Some(iv),
            _ => None,
        })
        .expect("InstrumentDeckView");
    match &inst_synth.visual {
        InstrumentVisualView::Rack(rack) => {
            let sel = rack
                .zones
                .iter()
                .find(|z| z.selected)
                .expect("selected zone");
            assert_eq!(sel.name, "CH");
            assert!(rack.selected_zone_summary.contains("CH"));
            assert!(rack.selected_zone_summary.contains("Choke 1"));
        }
        other => panic!("expected Rack visual, got {other:?}"),
    }
    let px_synth = gpu.render_rgba8(&fv_synth, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px_synth, dark.surface.canvas) > 0.25);
    assert_ne!(
        px_synth, px_sample,
        "Zones view and Sample Panel view must render distinctly"
    );
    maybe_save_png("03-rack-zones-synth-zone-zoomed.png", 1440, 860, &px_synth);

    // 4. Studio Light theme verification (strict &Theme token usage).
    let px_light = gpu.render_rgba8(&fv_synth, &light).unwrap();
    assert!(distinct_pixel_ratio(&px_light, light.surface.canvas) > 0.25);
    assert_ne!(px_light, px_synth);
    maybe_save_png("04-rack-zones-studio-light.png", 1440, 860, &px_light);

    // 5. Rack Patchbay view (`:tab patchbay`) showing the selected zone's Eurorack module graph!
    app.execute_command("tab patchbay").unwrap();
    let fv_pb = app.frame_view(&snap, status);
    let inst_pb = fv_pb
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Instrument(iv) => Some(iv),
            _ => None,
        })
        .expect("InstrumentDeckView on patchbay tab");
    match &inst_pb.visual {
        InstrumentVisualView::Rack(rack) => {
            assert_eq!(rack.active_tab, RackTab::Patchbay);
            assert!(
                rack.tabs
                    .iter()
                    .any(|&(label, active)| label == "Patchbay" && active),
                "Patchbay tab must be active: {:?}",
                rack.tabs
            );
        }
        other => panic!("expected Rack visual, got {other:?}"),
    }
    let px_pb = gpu.render_rgba8(&fv_pb, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px_pb, dark.surface.canvas) > 0.25);
    assert_ne!(px_pb, px_synth);
    maybe_save_png("05-rack-zones-zone-patchbay.png", 1440, 860, &px_pb);

    // 6. Full-keyboard (0..127) multi-zone velocity + key split instrument (Dark & Light).
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

    let fv_full = app.frame_view(&snap, status);
    let px_full_dark = gpu.render_rgba8(&fv_full, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px_full_dark, dark.surface.canvas) > 0.25);
    maybe_save_png(
        "06-rack-zones-full-keyboard-dark.png",
        1440,
        860,
        &px_full_dark,
    );
    let px_full_light = gpu.render_rgba8(&fv_full, &light).unwrap();
    assert!(distinct_pixel_ratio(&px_full_light, light.surface.canvas) > 0.25);
    maybe_save_png(
        "07-rack-zones-full-keyboard-light.png",
        1440,
        860,
        &px_full_light,
    );
}
