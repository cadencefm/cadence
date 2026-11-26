//! Integration tests for the interactive Signal Graph (`gG`), including
//! Effect/Channel node selection, direct cable authoring/unlinking, and
//! Filter envelope trigger routing.
use cadence_app::view::{DeckView, GraphDeckView, GraphEdgeKind, GraphTarget};
use cadence_app::{Action, App, ControlTarget, Detail, KeyCode, KeyEvent, KeyModifiers};
use cadence_core::material::{ModulationTarget, Project};
use cadence_core::mixer::{ChannelKind, ChannelRef, EffectKind, FilterPatch};
use cadence_core::runtime::AudioSnapshot;

fn graph_view(app: &App) -> GraphDeckView {
    let fv = app.frame_view(&AudioSnapshot::default(), "test");
    fv.decks
        .into_iter()
        .find_map(|d| match d.view {
            DeckView::Graph(g) => Some(g),
            _ => None,
        })
        .expect("GraphDeckView")
}

fn select_graph_target(app: &mut App, target: GraphTarget) {
    let g = graph_view(app);
    for (c, col) in g.columns.iter().enumerate() {
        for (r, node) in col.nodes.iter().enumerate() {
            if node.target == target {
                app.view_state.graph_col = c;
                app.view_state.graph_row = r;
                return;
            }
        }
    }
    panic!("GraphTarget {target:?} not found in Signal Graph");
}

#[test]
fn signal_graph_effect_and_channel_selection_and_direct_cable_workflows() {
    let mut app = App::new_in_memory(Project::demo(), "graph-demo.json".into()).unwrap();

    // Add a Group bus, a Return channel, and an insert Effect on Lead channel so all node kinds exist.
    let lead_ch = app.project().mixer.channels[0].id;
    let lead_inst = app.project().instruments[0].id;
    app.apply(Action::CreateChannel {
        name: "MusicGrp".into(),
        kind: ChannelKind::Group,
    })
    .unwrap();
    let grp_id = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "MusicGrp")
        .unwrap()
        .id;
    app.apply(Action::CreateChannel {
        name: "VerbRet".into(),
        kind: ChannelKind::Return,
    })
    .unwrap();
    let ret_id = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "VerbRet")
        .unwrap()
        .id;
    app.apply(Action::AddEffect {
        channel: ChannelRef::Channel(lead_ch),
        kind: "filter".into(),
    })
    .unwrap();
    let fx_id = app.channel(lead_ch).unwrap().inserts.last().unwrap().id;

    // Open Signal Graph (`gG`).
    app.press_keys("gG").unwrap();
    assert!(app.graph_focused());

    // 1. Verify GraphTarget::Effect(fx_id) exists in MIXER / FX column and Enter opens Detail::Effect(fx_id).
    let g = graph_view(&app);
    let fx_node = g
        .columns
        .iter()
        .flat_map(|c| c.nodes.iter())
        .find(|n| n.target == GraphTarget::Effect(fx_id))
        .expect("Insert FX must appear as a first-class GraphTarget::Effect node");
    assert!(fx_node.is_effect);
    select_graph_target(&mut app, GraphTarget::Effect(fx_id));
    app.press_keys("<Enter>").unwrap();
    assert_eq!(app.detail, Detail::Effect(fx_id));

    // Return to Signal Graph and verify GraphTarget::Channel(lead_ch) Enter opens Detail::Channel.
    app.press_keys("gG").unwrap();
    select_graph_target(&mut app, GraphTarget::Channel(ChannelRef::Channel(lead_ch)));
    app.press_keys("<Enter>").unwrap();
    assert_eq!(app.detail, Detail::Channel(ChannelRef::Channel(lead_ch)));

    // Also Master channel opens Detail::Channel(Master).
    app.press_keys("gG").unwrap();
    select_graph_target(&mut app, GraphTarget::Channel(ChannelRef::Master));
    app.press_keys("<Enter>").unwrap();
    assert_eq!(app.detail, Detail::Channel(ChannelRef::Master));

    // 2. Route Channel output (`o`) from gG -> MusicGrp, then Undo restores original output.
    app.press_keys("gG").unwrap();
    select_graph_target(&mut app, GraphTarget::Channel(ChannelRef::Channel(lead_ch)));
    let orig_lead_output = app.channel(lead_ch).unwrap().output;
    app.press_keys("o").unwrap();
    assert_eq!(app.mode_name(), "PICK");
    app.press_keys("/MusicGrp<Enter><Enter>").unwrap();
    assert_eq!(
        app.channel(lead_ch).unwrap().output,
        ChannelRef::Channel(grp_id)
    );
    assert!(app.graph_focused(), "routing from gG stays in gG");
    app.apply(Action::Undo).unwrap();
    assert_eq!(app.channel(lead_ch).unwrap().output, orig_lead_output);

    // 3. Re-route Instrument output (`r`) from gG to another Audio channel, then Undo.
    let alt_audio = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.kind == ChannelKind::Audio && c.id != lead_ch)
        .unwrap()
        .id;
    let alt_name = app.channel(alt_audio).unwrap().name.clone();
    select_graph_target(&mut app, GraphTarget::Instrument(lead_inst));
    app.press_keys("r").unwrap();
    assert_eq!(app.mode_name(), "PICK");
    app.press_keys(&format!("/{alt_name}<Enter><Enter>"))
        .unwrap();
    assert_eq!(app.instrument(lead_inst).unwrap().channel, alt_audio);
    assert!(
        app.graph_focused(),
        "instrument routing from gG stays in gG"
    );
    app.apply(Action::Undo).unwrap();
    assert_eq!(app.instrument(lead_inst).unwrap().channel, lead_ch);

    // 4. Create a Return Send (`s`) directly from gG on Lead channel -> VerbRet.
    select_graph_target(&mut app, GraphTarget::Channel(ChannelRef::Channel(lead_ch)));
    app.press_keys("s").unwrap();
    assert_eq!(app.mode_name(), "WORKFLOW");
    app.press_keys("<Enter>").unwrap(); // accept prefilled Lead source
    app.press_keys("/VerbRet<Enter><Enter>").unwrap(); // lock filter -> accept VerbRet -> advance to Amount
    app.press_keys("45%<Enter>").unwrap(); // amount 45% -> commit
    assert_eq!(app.mode_name(), "NORMAL");
    assert!(app.graph_focused(), "Send workflow from gG stays in gG");
    let send_amt = app
        .channel(lead_ch)
        .unwrap()
        .sends
        .iter()
        .find(|s| s.target == ret_id)
        .map(|s| s.amount)
        .unwrap_or(0.0);
    assert!((send_amt - 0.45).abs() < 0.01);

    // 5. Create a Modulation cable (`m`) targeting the Filter FX node (`GraphTarget::Effect(fx_id)`).
    let mods_before = app.project().modulations.len();
    select_graph_target(&mut app, GraphTarget::Effect(fx_id));
    app.press_keys("m").unwrap();
    assert_eq!(app.mode_name(), "WORKFLOW");
    // 4 fields: ModPattern -> ModTarget (prefilled Effect) -> ModParam -> Name -> commit
    app.press_keys("<Enter><Enter><Enter><Enter>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert!(
        app.graph_focused(),
        "Modulation workflow from gG stays in gG"
    );
    assert_eq!(app.project().modulations.len(), mods_before + 1);
    let new_mod = app.project().modulations.last().unwrap();
    assert!(matches!(
        new_mod.target,
        ModulationTarget::Effect { device, .. } if device == fx_id
    ));
    let mod_id = new_mod.id;

    // Verify selected_cables pill shows on FX node, and `x` on FX unlinks only the FX modulation
    // without touching the parent channel's Return send.
    select_graph_target(&mut app, GraphTarget::Effect(fx_id));
    let g_fx = graph_view(&app);
    assert!(
        g_fx.selected_cables.contains("1 EDIT") && g_fx.selected_cables.contains("1 UNLINK"),
        "expected 1 EDIT / 1 UNLINK in selected_cables, got: {}",
        g_fx.selected_cables
    );
    assert!(g_fx.edges.iter().any(|e| {
        e.kind == GraphEdgeKind::Modulation
            && g_fx.columns[e.to_col].nodes[e.to_row].target == GraphTarget::Effect(fx_id)
    }));

    // Press `e` on FX node -> edits that modulation in-place.
    app.press_keys("e").unwrap();
    assert_eq!(app.mode_name(), "WORKFLOW");
    app.press_keys("<Esc>").unwrap();

    // Press `x` on FX node -> unlinks the modulation; parent channel's VerbRet send stays untouched.
    app.press_keys("x").unwrap();
    assert!(app.project().modulations.iter().all(|m| m.id != mod_id));
    assert!(
        app.channel(lead_ch)
            .unwrap()
            .sends
            .iter()
            .any(|s| s.target == ret_id && s.amount > 0.40),
        "Unlinking a cable on an Effect node must not remove parent channel sends"
    );
    app.apply(Action::Undo).unwrap();
    assert!(app.project().modulations.iter().any(|m| m.id == mod_id));

    // 6. Create a Sidechain Duck (`d`) targeting Lead channel from gG.
    let ducks_before = app.project().ducks.len();
    select_graph_target(&mut app, GraphTarget::Channel(ChannelRef::Channel(lead_ch)));
    app.press_keys("d").unwrap();
    assert_eq!(app.mode_name(), "WORKFLOW");
    app.press_keys("<Enter><Enter><Enter><Enter><Enter>")
        .unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert!(app.graph_focused(), "Duck workflow from gG stays in gG");
    assert_eq!(app.project().ducks.len(), ducks_before + 1);
    let duck_id = app.project().ducks.last().unwrap().id;

    // Lead channel has both Return send (VerbRet) and Duck cable.
    // Pressing `X` (unlink all) removes all removable cables on Lead channel in one atomic Undo step.
    select_graph_target(&mut app, GraphTarget::Channel(ChannelRef::Channel(lead_ch)));
    let g_ch = graph_view(&app);
    assert!(
        g_ch.selected_cables.contains("EDIT") && g_ch.selected_cables.contains("UNLINK"),
        "expected EDIT / UNLINK indicators on channel, got: {}",
        g_ch.selected_cables
    );
    app.press_keys("X").unwrap();
    assert!(app.project().ducks.iter().all(|d| d.id != duck_id));
    assert!(app
        .channel(lead_ch)
        .unwrap()
        .sends
        .iter()
        .all(|s| s.target != ret_id));

    // Single Undo restores both the Duck and the Send.
    app.apply(Action::Undo).unwrap();
    assert!(app.project().ducks.iter().any(|d| d.id == duck_id));
    assert!(app
        .channel(lead_ch)
        .unwrap()
        .sends
        .iter()
        .any(|s| s.target == ret_id && (s.amount - 0.45).abs() < 0.01));
}

#[test]
fn filter_env_source_cycler_skips_degrees_and_renders_graph_cable() {
    let mut app = App::new_in_memory(Project::empty(), "unused.json".into()).unwrap();
    app.execute_command("pattern notes Melody 0 2 4 7").unwrap();
    app.execute_command("pattern degrees Harmony 16 hold 0:0 8:2")
        .unwrap();
    app.execute_command("pattern rhythm Pulse x...x...x...x...")
        .unwrap();

    let harmony_id = app
        .project()
        .patterns
        .iter()
        .find(|p| p.name == "Harmony")
        .unwrap()
        .id;
    let pulse_id = app
        .project()
        .patterns
        .iter()
        .find(|p| p.name == "Pulse")
        .unwrap()
        .id;

    // Add a Filter insert on Master and open its Effect deck.
    app.execute_command("fx add filter master").unwrap();
    let fx_id = app.project().mixer.master.inserts[0].id;
    app.execute_command("channel master").unwrap();
    let fx_row = app
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::Effect(id) if id == fx_id))
        .expect("Filter insert row on Master channel");
    app.control_cursor = fx_row;
    app.press_keys("<Enter>").unwrap();
    assert_eq!(app.detail, Detail::Effect(fx_id));

    let src_row = app
        .control_descriptors()
        .iter()
        .position(|c| c.group == "ENVELOPE" && c.label == "Source")
        .expect("Filter env Source control");
    app.control_cursor = src_row;

    // Cycle through every candidate directly with Alt+l — never errors and skips Values(Degrees).
    let mut seen = Vec::new();
    for _ in 0..8 {
        app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::ALT))
            .unwrap();
        let EffectKind::Filter(FilterPatch { env_pattern_id, .. }) =
            app.effect(fx_id).unwrap().1.kind
        else {
            panic!("expected Filter");
        };
        assert_ne!(
            env_pattern_id,
            Some(harmony_id),
            "FilterEnvSource cycler must skip Values(Degrees) patterns"
        );
        seen.push(env_pattern_id);
    }
    assert!(seen.contains(&Some(pulse_id)));

    // Set Filter env source to Pulse and verify Signal Graph renders a Modulation cable directly to the Filter FX node.
    app.execute_command("param source Pulse").unwrap();
    app.press_keys("gG").unwrap();
    let g = graph_view(&app);
    assert!(
        g.edges.iter().any(|e| {
            e.kind == GraphEdgeKind::Modulation
                && g.columns[e.from_col].nodes[e.from_row].target == GraphTarget::Pattern(pulse_id)
                && g.columns[e.to_col].nodes[e.to_row].target == GraphTarget::Effect(fx_id)
        }),
        "Signal Graph must render FilterPatch.env_pattern_id trigger cable to Effect node"
    );

    // Pressing `e` on Filter FX node edits the FilterEnv cable cleanly (opens Detail::Effect on FilterEnvSource).
    select_graph_target(&mut app, GraphTarget::Effect(fx_id));
    app.press_keys("e").unwrap();
    assert_eq!(app.detail, Detail::Effect(fx_id));
    assert!(matches!(
        app.control_descriptors()
            .get(app.control_cursor)
            .map(|c| c.target.clone()),
        Some(ControlTarget::FilterEnvSource(id)) if id == fx_id
    ));
}

#[test]
fn signal_graph_macro_cables_param_link_bus_route_and_zeroed_sends() {
    use cadence_core::material::Edit;

    let mut app = App::new_in_memory(Project::demo(), "graph-parity.json".into()).unwrap();
    let lead_ch = app.project().mixer.channels[0].id;
    let echo_ret = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.kind == ChannelKind::Return)
        .unwrap()
        .id;
    let part_with_link = app
        .project()
        .parts
        .iter()
        .find(|p| !p.parameter_links.is_empty())
        .expect("demo part with parameter_link")
        .id;

    // 1. Bind M1 to Saw Filter Cutoff and Lead Pan; verify Macro cables in `e`/`x`/`X`.
    app.execute_command("macro 1 name GraphMacro").unwrap();
    app.execute_command("macro 1 bind instrument Saw cutoff 0.1 0.9")
        .unwrap();
    app.execute_command("macro 1 bind channel Lead pan 0.2 0.8")
        .unwrap();

    app.press_keys("gG").unwrap();
    select_graph_target(&mut app, GraphTarget::Macro(0));
    let g_mac = graph_view(&app);
    assert!(
        g_mac.selected_cables.contains("2 EDIT") && g_mac.selected_cables.contains("2 UNLINK"),
        "expected 2 EDIT / 2 UNLINK on M1 with 2 bindings, got: {}",
        g_mac.selected_cables
    );

    // Press `e` on M1 -> picker lists both Macro bindings; choosing Lead Pan jumps straight to `gk` binding #1!
    app.press_keys("e/Lead<Enter><Enter>").unwrap();
    assert!(app.macros_focused());
    assert_eq!(app.view_state.macro_cursor, 0);
    assert_eq!(app.view_state.macro_binding_cursor, Some(1));

    // Return to gG on M1 -> `x` unbinds one binding; `X` unlinks all remaining cables atomically.
    app.press_keys("gG").unwrap();
    select_graph_target(&mut app, GraphTarget::Macro(0));
    app.press_keys("x/Lead<Enter><Enter>").unwrap();
    assert_eq!(app.project().macro_control(0).unwrap().bindings.len(), 1);
    app.apply(Action::Undo).unwrap();
    assert_eq!(app.project().macro_control(0).unwrap().bindings.len(), 2);

    select_graph_target(&mut app, GraphTarget::Macro(0));
    app.press_keys("X").unwrap();
    assert!(app.project().macro_control(0).unwrap().bindings.is_empty());
    app.apply(Action::Undo).unwrap();
    assert_eq!(app.project().macro_control(0).unwrap().bindings.len(), 2);

    // 2. Part `ParamLink` is editable via `e` in Signal Graph -> jumps directly to Detail::Part link row.
    select_graph_target(&mut app, GraphTarget::Part(part_with_link));
    app.press_keys("e").unwrap();
    assert_eq!(app.detail, Detail::Part(part_with_link));
    assert!(matches!(
        app.control_descriptors()
            .get(app.control_cursor)
            .map(|c| c.target.clone()),
        Some(ControlTarget::PartParamLink { part, index: 0, .. }) if part == part_with_link
    ));

    // 3. Non-Master `BusRoute` on Lead (-> Music) is editable via `e` -> opens bus route picker!
    app.press_keys("gG").unwrap();
    select_graph_target(&mut app, GraphTarget::Channel(ChannelRef::Channel(lead_ch)));
    // Lead has BusRoute (Lead -> Music) and MacroBinding (M1 -> Lead Pan).
    app.press_keys("e/ROUTE<Enter><Enter>").unwrap();
    assert_eq!(app.mode_name(), "PICK");
    app.press_keys("<Esc>").unwrap();

    // 4. Zeroed explicit Send (`amount == 0.0`) still appears in Graph edges, node detail, and `e`/`x` pickers!
    let mut sends = app.channel(lead_ch).unwrap().sends.clone();
    sends.push(cadence_core::mixer::ChannelSend {
        target: echo_ret,
        amount: 0.0,
    });
    app.apply(Action::Edit {
        edit: Edit::UpdateChannelMeta {
            id: lead_ch,
            name: None,
            output: None,
            sends: Some(sends),
        },
    })
    .unwrap();
    let g_zero = graph_view(&app);
    let lead_node = g_zero
        .columns
        .iter()
        .flat_map(|c| c.nodes.iter())
        .find(|n| n.target == GraphTarget::Channel(ChannelRef::Channel(lead_ch)))
        .expect("Lead node");
    assert!(
        lead_node.detail.contains("1s"),
        "zeroed explicit send must still count in channel node detail, got: {}",
        lead_node.detail
    );
    assert!(
        g_zero.edges.iter().any(|e| {
            e.kind == GraphEdgeKind::Routing
                && g_zero.columns[e.from_col].nodes[e.from_row].target
                    == GraphTarget::Channel(ChannelRef::Channel(lead_ch))
                && g_zero.columns[e.to_col].nodes[e.to_row].target
                    == GraphTarget::Channel(ChannelRef::Channel(echo_ret))
        }),
        "zeroed explicit send must still render a Routing edge in Signal Graph"
    );
    select_graph_target(&mut app, GraphTarget::Channel(ChannelRef::Channel(lead_ch)));
    app.press_keys("e/SEND<Enter><Enter>").unwrap();
    assert_eq!(app.mode_name(), "WORKFLOW");
    app.press_keys("<Esc>").unwrap();
}
