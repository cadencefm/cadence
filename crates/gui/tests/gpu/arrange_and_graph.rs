use crate::common::{
    build_snapshot, load_demo_project, render_checked, with_gpu, DEFAULT_H, DEFAULT_W, STATUS,
};
use cadence_app::view::{DeckView, GraphTarget};
use cadence_app::{Action, App, ArrangeView};
use cadence_core::material::{Boundary, LanePart, Part, Pattern, PatternKind, Placement, Project};
use cadence_core::mixer::{ChannelKind, ChannelRef};
use cadence_core::runtime::AudioSnapshot;
use cadence_gui::theme::Theme;

#[test]
fn gpu_renders_arrange_overview_roll_scrolloff_and_signal_graph_cables() {
    with_gpu(DEFAULT_W, DEFAULT_H, |gpu| {
        let dark = Theme::cadence_dark();
        let neon = Theme::midnight_neon();
        let light = Theme::studio_light();
        let (project, demo_path) = load_demo_project();
        let snap = build_snapshot(&project, 324);

        // 1. Zoomed-out Evaluated Overview (25% -> 50% + live override)
        let lib = cadence_core::sampler::SampleLibrary::from_roots(&[], Some(&demo_path));
        let mut app = App::new_with_library(project.clone(), demo_path.clone(), lib)
            .expect("init Afterglow app");
        app.press_keys("vozz").unwrap();
        app.arrange_ui.zoom = 0.25;
        let px_over_25 = render_checked(
            gpu,
            "arrange-01-overview-25pct.png",
            &app.frame_view(&snap, STATUS),
            &dark,
            0.20,
        );

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
        let px_over_50 = render_checked(
            gpu,
            "arrange-02-overview-50pct-live.png",
            &app.frame_view(&snap, STATUS),
            &dark,
            0.20,
        );
        assert_ne!(px_over_25, px_over_50);

        // 2. Evaluated Lane Roll (`vr`) with fractional sub-step `:mini` + Swing + live override
        let mut roll_project = project.clone();
        let (base_pat, mini_pat, live_pat) = (950, 951, 952);
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
        roll_project
            .parts
            .push(Part::new(960, "Sparkle", mini_pat, inst_id));
        roll_project
            .parts
            .push(Part::new(961, "Live Arp", live_pat, inst_id));
        let lane_id = roll_project.lanes[0].id;
        roll_project.lanes[0].parts.push(LanePart::new(960));
        roll_project.lanes[0].parts.push(LanePart::new(961));
        roll_project.lanes[0].placements = vec![Placement::new(970, 960, 0, 128)];

        let lib = cadence_core::sampler::SampleLibrary::from_roots(&[], Some(&demo_path));
        let mut roll_app = App::new_with_library(roll_project, demo_path.clone(), lib).unwrap();
        roll_app.selected_lane = Some(lane_id);
        roll_app.arrange_ui.view = ArrangeView::Roll;
        roll_app.arrange_ui.zoom = 4.0;
        roll_app.press_keys("zz").unwrap();
        roll_app.observe_transport(32, 33, true, false);
        roll_app
            .apply(Action::Launch {
                lane: lane_id,
                part: LanePart::new(961),
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
        render_checked(
            gpu,
            "arrange-03-lane-roll-substep-live.png",
            &roll_app.frame_view(&roll_snap, STATUS),
            &dark,
            0.20,
        );

        // 3. Arrange user cursor scrolloff (split vs playhead overlap) across Dark, Neon, Light
        let lib = cadence_core::sampler::SampleLibrary::from_roots(&[], Some(&demo_path));
        let mut cur_app = App::new_with_library(project.clone(), demo_path.clone(), lib).unwrap();
        cur_app.press_keys("ga").unwrap();
        for _ in 0..14 {
            cur_app.press_keys("l").unwrap();
        }
        let snap_split = AudioSnapshot {
            playing: true,
            absolute_step: 150,
            step_fraction: 0.35,
            ..AudioSnapshot::default()
        };
        cur_app.observe_transport(150, 151, true, false);
        let fv_split = cur_app.frame_view(&snap_split, STATUS);
        let arr_split = fv_split
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Arrange(a) => Some(a),
                _ => None,
            })
            .expect("Arrange deck");
        assert_eq!(arr_split.cursor_step, 14 * 16);
        assert_eq!(arr_split.page_start_step, 3 * 16);

        let px_dark = render_checked(
            gpu,
            "arrange-04-cursor-scrolloff-dark.png",
            &fv_split,
            &dark,
            0.20,
        );
        render_checked(
            gpu,
            "arrange-05-cursor-scrolloff-neon.png",
            &fv_split,
            &neon,
            0.20,
        );
        render_checked(
            gpu,
            "arrange-06-cursor-scrolloff-light.png",
            &fv_split,
            &light,
            0.20,
        );

        let snap_overlap = AudioSnapshot {
            playing: true,
            absolute_step: 224,
            step_fraction: 0.0,
            ..AudioSnapshot::default()
        };
        cur_app.observe_transport(224, 225, true, false);
        let px_overlap = render_checked(
            gpu,
            "arrange-07-cursor-playhead-overlap.png",
            &cur_app.frame_view(&snap_overlap, STATUS),
            &dark,
            0.20,
        );
        assert_ne!(px_dark, px_overlap);

        // 4. Signal Graph: dual-edge offscreen back-panel wires (`▲`/`▼`)
        let mut graph_project = project;
        let base_src_id = graph_project
            .patterns
            .iter()
            .find(|p| matches!(p.kind, PatternKind::Notes { .. } | PatternKind::Mini { .. }))
            .expect("note pattern")
            .id;
        let inst0_id = graph_project.instruments[0].id;
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
            graph_project.parts.push(Part::new(
                860 + i,
                format!("Layer Part {:02}", i + 1),
                drv_id,
                inst0_id,
            ));
        }
        let lib = cadence_core::sampler::SampleLibrary::from_roots(&[], Some(&demo_path));
        let mut graph_app = App::new_with_library(graph_project, demo_path, lib).unwrap();
        graph_app.press_keys("gGzz").unwrap();
        let (inst_col, parts_col, parts_count) = {
            let fv_init = graph_app.frame_view(&snap, STATUS);
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
        let px_graph_inst = render_checked(
            gpu,
            "graph-01-offscreen-wires.png",
            &graph_app.frame_view(&snap, STATUS),
            &dark,
            0.20,
        );
        graph_app.view_state.graph_col = parts_col;
        graph_app.view_state.graph_row = parts_count / 2;
        let px_graph_mid = render_checked(
            gpu,
            "graph-02-mid-scroll-both-edges.png",
            &graph_app.frame_view(&snap, STATUS),
            &dark,
            0.20,
        );
        assert_ne!(px_graph_inst, px_graph_mid);

        // 5. Signal Graph: indented FX inserts (active + bypassed), Send, Duck, Macro fan-out cables
        let mut fx_app = App::new_in_memory(Project::demo(), "graph-fx.json".into()).unwrap();
        let lead_ch = fx_app.project().mixer.channels[0].id;
        fx_app
            .apply(Action::CreateChannel {
                name: "VerbRet".into(),
                kind: ChannelKind::Return,
            })
            .unwrap();
        let ret_id = fx_app
            .project()
            .mixer
            .channels
            .iter()
            .find(|c| c.name == "VerbRet")
            .unwrap()
            .id;
        fx_app
            .apply(Action::SetChannelSend {
                channel: lead_ch,
                target: ret_id,
                amount: Some(0.42),
            })
            .unwrap();
        fx_app
            .apply(Action::AddEffect {
                channel: ChannelRef::Channel(lead_ch),
                kind: "filter".into(),
            })
            .unwrap();
        fx_app
            .apply(Action::AddEffect {
                channel: ChannelRef::Channel(lead_ch),
                kind: "delay".into(),
            })
            .unwrap();
        let delay_id = fx_app.channel(lead_ch).unwrap().inserts.last().unwrap().id;
        fx_app
            .apply(Action::SetEffectBypass {
                device: delay_id,
                bypassed: true,
            })
            .unwrap();
        let src_part = fx_app.project().parts[0].name.clone();
        let dst_ch = fx_app.project().mixer.channels[0].name.clone();
        fx_app
            .execute_command(&format!("duck LeadPump {src_part} {dst_ch} 0.65 140"))
            .unwrap();
        fx_app.execute_command("macro 1 name Sweep").unwrap();
        fx_app
            .execute_command("macro 1 bind instrument Saw cutoff 0.15 0.90")
            .unwrap();
        fx_app
            .execute_command("macro 1 bind channel Lead pan 0.90 0.10")
            .unwrap();

        let fx_snap = build_snapshot(fx_app.project(), 8);
        fx_app.press_keys("gGzz").unwrap();
        let (ch_pos, macro_col) = {
            let probe = fx_app.frame_view(&fx_snap, STATUS);
            let g = probe
                .decks
                .iter()
                .find_map(|d| match &d.view {
                    DeckView::Graph(g) => Some(g),
                    _ => None,
                })
                .expect("GraphDeckView");
            let mut ch_pos = (0, 0);
            for (c, col) in g.columns.iter().enumerate() {
                for (r, node) in col.nodes.iter().enumerate() {
                    if node.target == GraphTarget::Channel(ChannelRef::Channel(lead_ch)) {
                        ch_pos = (c, r);
                    }
                }
            }
            let macro_col = g
                .columns
                .iter()
                .position(|c| c.title == "MACROS")
                .expect("MACROS column");
            (ch_pos, macro_col)
        };
        fx_app.view_state.graph_col = ch_pos.0;
        fx_app.view_state.graph_row = ch_pos.1;
        let px_fx_cables = render_checked(
            gpu,
            "graph-03-fx-send-duck-cables.png",
            &fx_app.frame_view(&fx_snap, STATUS),
            &dark,
            0.20,
        );

        fx_app.view_state.graph_col = macro_col;
        fx_app.view_state.graph_row = 0;
        let px_macro_cables = render_checked(
            gpu,
            "graph-04-macros-fanout.png",
            &fx_app.frame_view(&fx_snap, STATUS),
            &dark,
            0.20,
        );
        assert_ne!(px_fx_cables, px_macro_cables);
    });
}
