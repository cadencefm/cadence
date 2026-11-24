//! Headless Metal/Vulkan GPU integration tests for:
//! 1. Zoomed-out Evaluated Overview (`vo`, 25% & 50% zoom across 64-bar Afterglow),
//! 2. Evaluated Lane Roll (`vr`) with fractional sub-step notes (`:mini` / `Ratchet`) & live override,
//! 3. Signal Graph (`gG`) with Reason/ReWire back-panel offscreen cables & scroll indicators.

mod common;

use cadence_app::view::DeckView;
use cadence_app::{Action, App, ArrangeView};
use cadence_core::material::{Boundary, LanePart, Part, Pattern, PatternKind, Placement};
use cadence_core::runtime::AudioSnapshot;
use cadence_gui::headless::{distinct_pixel_ratio, HeadlessGpu};
use cadence_gui::theme::Theme;
use std::path::PathBuf;

fn maybe_save_png(name: &str, w: u32, h: u32, px: &[u8]) {
    common::maybe_save_png("CADENCE_DUMP_ARRANGE_GRAPH_DIR", name, w, h, px);
}

#[test]
fn gpu_renders_zoomed_out_overview_substep_roll_and_offscreen_graph_wires() {
    let Ok(mut gpu) = HeadlessGpu::new(1440, 860) else {
        // Skip gracefully only on GPU-less CI runners.
        return;
    };
    let theme = Theme::cadence_dark();
    let status = "Metal · 48 kHz";

    let demo_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/demo/project.json");
    let project = cadence_app::load_project(&demo_path).expect("load Afterglow project");
    let snap = common::build_snapshot(&project, 324);

    // 1. Zoomed-out Evaluated Overview (25% zoom = full 64 bars / 1024 steps on Afterglow)
    let mut app = App::new(project.clone(), demo_path.clone()).expect("init Afterglow app");
    app.press_keys("vozz").unwrap();
    app.arrange_ui.zoom = 0.25;
    let fv_over_25 = app.frame_view(&snap, status);
    let px_over_25 = gpu.render_rgba8(&fv_over_25, &theme).unwrap();
    assert!(
        distinct_pixel_ratio(&px_over_25, theme.surface.canvas) > 0.20,
        "25% zoomed-out Overview rendered too few pixels"
    );
    maybe_save_png("01-afterglow-overview-25pct.png", 1440, 860, &px_over_25);

    // 2. Zoomed-out Evaluated Overview (50% zoom = 32 bars / 512 steps) + live override on lane 1!
    app.arrange_ui.zoom = 0.5;
    app.observe_transport(128, 129, true, false);
    if let Some(lane0) = app.project().lanes.first().cloned() {
        if let Some(&lp) = lane0.parts.last() {
            let _ = app.apply(Action::Launch {
                lane: lane0.id,
                part: lp,
                at: Some(Boundary::Now),
            });
            app.observe_transport(324, 325, true, false);
        }
    }
    let fv_over_50 = app.frame_view(&snap, status);
    let px_over_50 = gpu.render_rgba8(&fv_over_50, &theme).unwrap();
    assert!(distinct_pixel_ratio(&px_over_50, theme.surface.canvas) > 0.20);
    assert_ne!(px_over_25, px_over_50);
    maybe_save_png(
        "02-afterglow-overview-50pct-live.png",
        1440,
        860,
        &px_over_50,
    );

    // 3. Evaluated Lane Roll (`vr`) with fractional sub-step notes (`:mini` triplets + `Ratchet`)
    //    AND an active live override band + live notes!
    let mut roll_project = project.clone();
    let base_pat = 950;
    let mini_pat = 951;
    let live_pat = 952;
    roll_project.patterns.push(Pattern {
        id: base_pat,
        name: "Sparkle Mini".into(),
        kind: PatternKind::Mini {
            code: "c4 [e4 g4] [b4 d5 c5] g4 [a4 c5 e5 g5] f4 [e4 g4 b4] d4".into(),
            steps: 8,
        },
    });
    roll_project.patterns.push(Pattern {
        id: mini_pat,
        name: "Sparkle Swung".into(),
        kind: PatternKind::Swing {
            source: base_pat,
            amount: 0.30,
        },
    });
    roll_project.patterns.push(Pattern {
        id: live_pat,
        name: "Live Arp".into(),
        kind: PatternKind::Mini {
            code: "[c5 e5 g5] [b4 d5 f5] [a4 c5 e5] [g4 b4 d5]".into(),
            steps: 4,
        },
    });
    let inst_id = roll_project.instruments[0].id;
    let part_main = 960;
    let part_live = 961;
    roll_project
        .parts
        .push(Part::new(part_main, "Sparkle", mini_pat, inst_id));
    roll_project
        .parts
        .push(Part::new(part_live, "Live Arp", live_pat, inst_id));
    let lane_id = roll_project.lanes[0].id;
    roll_project.lanes[0].parts.push(LanePart::new(part_main));
    roll_project.lanes[0].parts.push(LanePart::new(part_live));
    roll_project.lanes[0].placements = vec![Placement::new(970, part_main, 0, 128)];

    let mut roll_app = App::new(roll_project, demo_path.clone()).unwrap();
    roll_app.selected_lane = Some(lane_id);
    roll_app.arrange_ui.view = ArrangeView::Roll;
    roll_app.arrange_ui.zoom = 4.0; // zoomed in cleanly (4 bars = 64 steps) so sub-step triplets pop!
    roll_app.press_keys("zz").unwrap();
    roll_app.observe_transport(32, 33, true, false);
    roll_app
        .apply(Action::Launch {
            lane: lane_id,
            part: LanePart::new(part_live),
            at: Some(Boundary::Now),
        })
        .unwrap();
    roll_app.observe_transport(36, 37, true, false);
    let roll_snap = AudioSnapshot {
        playing: true,
        absolute_step: 36,
        step_fraction: 0.25,
        ..snap.clone()
    };
    let fv_roll = roll_app.frame_view(&roll_snap, status);
    let px_roll = gpu.render_rgba8(&fv_roll, &theme).unwrap();
    assert!(distinct_pixel_ratio(&px_roll, theme.surface.canvas) > 0.20);
    maybe_save_png("03-lane-roll-substep-and-live.png", 1440, 860, &px_roll);

    // 4. Signal Graph (`gG zz`) with overflowing columns (both above & below) and
    //    highlighted + dim Reason/ReWire back-panel offscreen cables!
    let mut graph_project = project;
    let base_src_id = graph_project
        .patterns
        .iter()
        .find(|p| matches!(p.kind, PatternKind::Notes { .. } | PatternKind::Mini { .. }))
        .expect("note pattern in Afterglow")
        .id;
    let inst0_id = graph_project.instruments[0].id;
    // Add extra Source patterns, Derived patterns, and Parts wired across top and bottom
    // of the columns so every scroll edge carries active & background patch cables!
    for i in 0..16u64 {
        let src_id = 800 + i * 2;
        let drv_id = 801 + i * 2;
        graph_project.patterns.push(Pattern {
            id: src_id,
            name: format!("Layer Src {:02}", i + 1),
            kind: PatternKind::Mini {
                code: "c4 e4 g4 b4".into(),
                steps: 4,
            },
        });
        graph_project.patterns.push(Pattern {
            id: drv_id,
            name: format!("Layer Drv {:02}", i + 1),
            kind: PatternKind::Transpose {
                source: if i % 2 == 0 { base_src_id } else { src_id },
                semitones: 12,
            },
        });
        let pid = 860 + i;
        graph_project.parts.push(Part::new(
            pid,
            format!("Layer Part {:02}", i + 1),
            drv_id,
            inst0_id,
        ));
    }
    let mut graph_app = App::new(graph_project, demo_path).unwrap();
    // Open full-screen Signal Graph and explicitly select the INSTRUMENTS column so its selected
    // instrument illuminates upstream PARTS scrolled BOTH above and below + SOURCES below!
    graph_app.press_keys("gGzz").unwrap();
    let (inst_col, parts_col, parts_count) = {
        let fv_init = graph_app.frame_view(&snap, status);
        let gv = fv_init
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Graph(g) => Some(g),
                _ => None,
            })
            .expect("GraphDeckView");
        let inst_col = gv
            .columns
            .iter()
            .position(|c| c.title == "INSTRUMENTS")
            .expect("INSTRUMENTS column");
        let parts_col = gv
            .columns
            .iter()
            .position(|c| c.title == "PARTS")
            .expect("PARTS column");
        (inst_col, parts_col, gv.columns[parts_col].nodes.len())
    };
    graph_app.view_state.graph_col = inst_col;
    graph_app.view_state.graph_row = 0;
    let px_graph_inst = gpu
        .render_rgba8(&graph_app.frame_view(&snap, status), &theme)
        .unwrap();
    assert!(distinct_pixel_ratio(&px_graph_inst, theme.surface.canvas) > 0.20);
    maybe_save_png(
        "04-signal-graph-offscreen-wires.png",
        1440,
        860,
        &px_graph_inst,
    );

    // Also render scrolled mid-column in PARTS so BOTH `▲ N above` and `▼ N below` pills appear
    // simultaneously with live back-panel cables!
    graph_app.view_state.graph_col = parts_col;
    graph_app.view_state.graph_row = parts_count / 2;
    let px_graph_mid = gpu
        .render_rgba8(&graph_app.frame_view(&snap, status), &theme)
        .unwrap();
    assert!(distinct_pixel_ratio(&px_graph_mid, theme.surface.canvas) > 0.20);
    assert_ne!(px_graph_inst, px_graph_mid);
    maybe_save_png(
        "05-signal-graph-mid-scroll-both-edges.png",
        1440,
        860,
        &px_graph_mid,
    );
}
