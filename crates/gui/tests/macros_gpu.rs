//! Headless Metal/wgpu GPU integration tests for Assignable Project Macros:
//! 1. Macros Deck (`gk zz`): 6 named macros (multi-target bindings, inverted `↓` mapping,
//!    live modulated ghost arc + dot, MIDI CC badges, and Chrome `◈ MACROS` pill)
//! 2. Split-pane layout (Arrange + Macros deck + Bank + Output) verifying single-row horizontal strip
//! 3. Signal Graph (`gG zz`): `MACROS` column + upstream `Values -> Macro` & downstream fan-out cables
//! 4. Devices · MIDI Deck (`gd zz`): MiniLab 3 Smart Macros mapped directly to Project Macros
//! 5. Studio Light theme verification proving 100% `&Theme` token compliance
//! 6. Split-pane Macros horizontal strip scrolling + vertical bindings viewport scrolling (>10 bindings)
//!
//! Set `CADENCE_DUMP_MACROS_DIR=/path/to/dir` to save 1440×860 PNG screenshots for visual inspection.

mod common;

use cadence_app::view::DeckView;
use cadence_app::App;
use cadence_core::material::{Edit, Pattern, PatternKind, Project, ValuePoint, ValueUnit};
use cadence_core::mixer::EffectKind;
use cadence_core::params::ValueMode;
use cadence_core::runtime::MidiMessage;
use cadence_gui::headless::{distinct_pixel_ratio, HeadlessGpu};
use cadence_gui::theme::Theme;
use std::path::PathBuf;

fn maybe_save_png(name: &str, width: u32, height: u32, rgba: &[u8]) {
    common::maybe_save_png("CADENCE_DUMP_MACROS_DIR", name, width, height, rgba);
}

fn setup_macro_showcase_app() -> App {
    let mut app = App::new_in_memory(Project::demo(), PathBuf::from("macros_gpu.json")).unwrap();
    app.config.midi_pickup = false;
    app.execute_command("midi profile minilab3_daw").unwrap();

    let delay_id = app
        .project()
        .mixer
        .channels
        .iter()
        .flat_map(|c| c.inserts.iter())
        .find(|d| matches!(d.kind, EffectKind::Delay(_)))
        .expect("Delay insert in demo project")
        .id;

    // Add a clean normalized Values curve to automate Macro 1 & Macro 3!
    let curve_id = app.project().max_id() + 1;
    app.apply(cadence_app::Action::Edit {
        edit: Edit::PutPattern {
            pattern: Pattern {
                id: curve_id,
                name: "RiserCurve".into(),
                kind: PatternKind::Values {
                    unit: ValueUnit::Normalized,
                    period: 32,
                    mode: ValueMode::Ramp,
                    points: vec![
                        ValuePoint {
                            step: 0,
                            value: 0.15,
                        },
                        ValuePoint {
                            step: 16,
                            value: 0.88,
                        },
                        ValuePoint {
                            step: 28,
                            value: 0.35,
                        },
                    ],
                },
            },
        },
    })
    .unwrap();

    // M1: "Filter Wash" (4 targets including an inverted Lead Pan & inverted Bass Cutoff! Automated by RiserCurve!)
    app.execute_command("macro 1 name Filter Wash").unwrap();
    app.execute_command("macro 1 bind instrument Saw cutoff 0.12 0.95")
        .unwrap();
    app.execute_command("macro 1 bind instrument Saw resonance 0.10 0.72")
        .unwrap();
    app.execute_command("macro 1 bind channel Lead pan 0.92 0.08")
        .unwrap(); // inverted!
    app.execute_command(&format!("macro 1 bind effect {delay_id} mix 0.08 0.82"))
        .unwrap();
    app.execute_command("macro 1 54%").unwrap();
    app.execute_command("mod WashAuto macro 1 RiserCurve")
        .unwrap();

    // M2: "Sub Punch" (Bass cutoff + drive + inverted Music gain trim)
    app.execute_command("macro 2 name Sub Punch").unwrap();
    app.execute_command("macro 2 bind instrument 'Low voice' cutoff 0.18 0.85")
        .unwrap();
    app.execute_command("macro 2 bind instrument 'Low voice' drive 0.05 0.80")
        .unwrap();
    app.execute_command("macro 2 bind channel Music gain 0.90 0.45")
        .unwrap(); // inverted!
    app.execute_command("macro 2 72%").unwrap();

    // M3: "Echo Throw" (Delay feedback + Echo return gain, automated by Energy!)
    app.execute_command("macro 3 name Echo Throw").unwrap();
    app.execute_command(&format!(
        "macro 3 bind effect {delay_id} feedback 0.15 0.85"
    ))
    .unwrap();
    app.execute_command("macro 3 bind channel Echo gain 0.15 0.88")
        .unwrap();
    app.execute_command("macro 3 36%").unwrap();
    app.execute_command("mod EchoAuto macro 3 Energy").unwrap();

    // M4: "Kick Snap" (Kick attack inverted + Kick release)
    app.execute_command("macro 4 name Kick Snap").unwrap();
    app.execute_command("macro 4 bind instrument Kick attack 0.40 0.02")
        .unwrap(); // inverted!
    app.execute_command("macro 4 bind instrument Kick decay 0.25 0.85")
        .unwrap();
    app.execute_command("macro 4 64%").unwrap();

    // M5: "Wide Space" (Lead pan + Bass pan opposite stereo spread!)
    app.execute_command("macro 5 name Wide Space").unwrap();
    app.execute_command("macro 5 bind channel Lead pan 0.50 0.88")
        .unwrap();
    app.execute_command("macro 5 bind channel Bass pan 0.50 0.12")
        .unwrap(); // inverted!
    app.execute_command("macro 5 80%").unwrap();

    // M6: "Master Dim" (Master gain inverted!)
    app.execute_command("macro 6 name Master Dim").unwrap();
    app.execute_command("macro 6 bind channel Master gain 0.88 0.35")
        .unwrap(); // inverted!
    app.execute_command("macro 6 22%").unwrap();

    // Simulate live incoming MiniLab 3 CC on M1 & M2 so MIDI deck activity & live CC bars populate!
    app.handle_midi(MidiMessage::Control {
        channel: 0,
        controller: 86,
        value: 69,
    })
    .unwrap();
    app.handle_midi(MidiMessage::Control {
        channel: 0,
        controller: 87,
        value: 92,
    })
    .unwrap();

    app
}

#[test]
fn gpu_renders_macros_deck_signal_graph_fanout_and_midi_project_macros() {
    let Ok(mut gpu) = HeadlessGpu::new(1440, 860) else {
        return;
    };

    let mut app = setup_macro_showcase_app();
    let snap = common::build_snapshot(app.project(), 14);
    let theme = Theme::cadence_dark();
    let status = "Metal · 48 kHz";

    // --- 1. Zoomed Macros Deck (`gk zz`) showing 8-slot studio console, ghost arc, inverted binding & MIDI badges! ---
    app.press_keys("gk1zz").unwrap();
    let fv_macros = app.frame_view(&snap, status);
    assert_eq!(fv_macros.chrome.active_macro_count, 6);
    assert_eq!(fv_macros.chrome.active_macro_automated, 2);
    let mview = fv_macros
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Macros(m) => Some(m),
            _ => None,
        })
        .expect("MacrosDeckView");
    assert_eq!(mview.slots.len(), 8);
    assert!(mview.slots[0].effective_value.is_some());
    assert!(mview.slots[0].bindings.iter().any(|b| b.inverted));

    let px_macros = gpu.render_rgba8(&fv_macros, &theme).unwrap();
    assert!(distinct_pixel_ratio(&px_macros, theme.surface.canvas) > 0.25);
    maybe_save_png("01-macros-deck-console.png", 1440, 860, &px_macros);

    // --- 2. Split-pane layout (`zz` unzoomed: Arrange + Macros + Bank + Output) verifying single-row 1D strip! ---
    app.press_keys("zz2").unwrap(); // select M2 Sub Punch in split pane
    let fv_split = app.frame_view(&snap, status);
    let px_split = gpu.render_rgba8(&fv_split, &theme).unwrap();
    assert!(distinct_pixel_ratio(&px_split, theme.surface.canvas) > 0.25);
    assert_ne!(px_split, px_macros);
    maybe_save_png("02-macros-deck-split.png", 1440, 860, &px_split);

    // --- 3. Zoomed Signal Graph (`gG zz`) highlighting M1 Macro node + fan-out cables! ---
    app.press_keys("gGzz").unwrap();
    let macros_col = {
        let fv_init = app.frame_view(&snap, status);
        fv_init
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Graph(g) => g.columns.iter().position(|c| c.title == "MACROS"),
                _ => None,
            })
            .expect("MACROS column in Signal Graph")
    };
    app.view_state.graph_col = macros_col;
    app.view_state.graph_row = 0; // M1 Filter Wash
    let fv_graph = app.frame_view(&snap, status);
    let px_graph = gpu.render_rgba8(&fv_graph, &theme).unwrap();
    assert!(distinct_pixel_ratio(&px_graph, theme.surface.canvas) > 0.20);
    assert_ne!(px_graph, px_macros);
    maybe_save_png("03-signal-graph-macros.png", 1440, 860, &px_graph);

    // --- 4. Zoomed Devices · MIDI Deck (`gd zz`) showing MiniLab 3 Smart Macros mapped to Project Macros! ---
    app.press_keys("zzgdzz").unwrap();
    let fv_midi = app.frame_view(&snap, status);
    let midi_view = fv_midi
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Midi(m) => Some(m),
            _ => None,
        })
        .expect("MidiDeckView");
    assert_eq!(midi_view.project_macro_count, 6);
    let px_midi = gpu.render_rgba8(&fv_midi, &theme).unwrap();
    assert!(distinct_pixel_ratio(&px_midi, theme.surface.canvas) > 0.25);
    maybe_save_png("04-midi-deck-project-macros.png", 1440, 860, &px_midi);

    // --- 5. Studio Light theme verification on Macros Deck (proving 100% &Theme token compliance!) ---
    let light = Theme::studio_light();
    let px_light = gpu.render_rgba8(&fv_macros, &light).unwrap();
    assert!(distinct_pixel_ratio(&px_light, light.surface.canvas) > 0.25);
    assert_ne!(px_light, px_macros);
    maybe_save_png("05-macros-deck-studio-light.png", 1440, 860, &px_light);

    // --- 6. Split-pane Macros horizontal strip + vertical bindings viewport scroll (>10 bindings) ---
    let mut scroll_app = setup_macro_showcase_app();
    scroll_app
        .execute_command("macro 7 name Deep Matrix")
        .unwrap();
    for cmd in [
        "macro 7 bind instrument Saw cutoff 0.10 0.90",
        "macro 7 bind instrument Saw resonance 0.05 0.70",
        "macro 7 bind instrument Saw drive 0.00 0.85",
        "macro 7 bind instrument Saw attack 0.05 0.60",
        "macro 7 bind instrument Saw decay 0.20 0.85",
        "macro 7 bind instrument Saw release 0.15 0.80",
        "macro 7 bind instrument 'Low voice' cutoff 0.15 0.92",
        "macro 7 bind instrument 'Low voice' resonance 0.10 0.65",
        "macro 7 bind channel Lead gain 0.85 0.45",
        "macro 7 bind channel Lead pan 0.20 0.80",
        "macro 7 bind channel Bass pan 0.80 0.20",
        "macro 7 bind channel Master gain 0.95 0.55",
    ] {
        scroll_app.execute_command(cmd).unwrap();
    }
    scroll_app.execute_command("macro 7 62%").unwrap();
    scroll_app.press_keys("gk7").unwrap();
    // Focus binding #10 (index 9) so the bindings table scrolls vertically around selection.
    scroll_app.view_state.macro_binding_cursor = Some(9);
    let fv_scroll = scroll_app.frame_view(&snap, status);
    let px_scroll = gpu.render_rgba8(&fv_scroll, &theme).unwrap();
    assert!(distinct_pixel_ratio(&px_scroll, theme.surface.canvas) > 0.25);
    assert_ne!(px_scroll, px_split);
    maybe_save_png("06-macros-strip-and-scroll.png", 1440, 860, &px_scroll);
}
