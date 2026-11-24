use cadence_app::keys::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use cadence_app::{Action, App, ControlTarget, Detail, EditScope, Focus, RackTab, Workspace};
use cadence_core::{
    material::{Edit, Project},
    params::InstrumentParam,
    runtime::TransportIntent,
};

fn app() -> App {
    let mut app = App::new(Project::empty(), "unused.json".into()).unwrap();
    app.execute_command("part new One init 0 2 4 6 7").unwrap();
    app.execute_command("part new Two pulse_lead 0 1 2 3 4 5 6")
        .unwrap();
    app.execute_command("open \"One motif\"").unwrap();
    app
}
fn instrument(app: &App, name: &str) -> u64 {
    app.project()
        .instruments
        .iter()
        .find(|i| i.name == name)
        .unwrap()
        .id
}

#[test]
fn home_stops_globally_and_preserves_entry_cursor_and_score() {
    for token in ["<Home>", "<C-Home>"] {
        let mut a = app();
        let before = a.project().clone();
        let revision = a.revision();
        a.cursor = 3;
        a.arrange_ui.step = 64;
        a.observe_transport(51, 51 + 1, true, false);
        a.press_keys(":title unfinished").unwrap();
        let out = a.press_keys(token).unwrap().pop().unwrap();
        assert_eq!(
            out.transport,
            vec![
                TransportIntent::Play { playing: false },
                TransportIntent::Seek { step: 0 }
            ]
        );
        assert_eq!(a.command_line(), Some("title unfinished"));
        assert_eq!((a.cursor, a.arrange_ui.step), (3, 64));
        assert_eq!(a.playing_loop(), None);
        assert!(!a.playing());
        assert_eq!(a.project(), &before);
        assert_eq!(a.revision(), revision);
        let repeated = a
            .handle_key(KeyEvent::new_with_kind(
                KeyCode::Home,
                KeyModifiers::NONE,
                KeyEventKind::Repeat,
            ))
            .unwrap();
        assert!(repeated.transport.is_empty());
    }
}

#[test]
fn end_and_start_navigation_follow_focus_without_seeking_transport() {
    let mut a = app();
    a.observe_transport(33, 33 + 1, true, false);
    let before = a.project().clone();
    let revision = a.revision();
    a.press_keys("G").unwrap();
    assert_eq!(a.cursor, 4);
    a.press_keys("gg").unwrap();
    assert_eq!(a.cursor, 0);
    a.press_keys("ge").unwrap();
    assert_eq!(a.cursor, 4);
    a.press_keys("0$").unwrap();
    assert_eq!(a.cursor, 4);
    a.press_keys("gaG").unwrap();
    assert_eq!(a.arrange_ui.step, (a.project().bars as u32 - 1) * 16);
    a.press_keys("gg").unwrap();
    assert_eq!(a.arrange_ui.step, 0);
    a.press_keys("/<Enter>G").unwrap();
    assert_eq!(
        a.selected_pattern,
        a.project().patterns.last().map(|p| p.id)
    );
    a.press_keys("gg").unwrap();
    assert_eq!(
        a.selected_pattern,
        a.project().patterns.first().map(|p| p.id)
    );
    a.press_keys("gmG").unwrap();
    assert_eq!(a.selected_channel, cadence_core::mixer::ChannelRef::Master);
    a.press_keys("gg").unwrap();
    assert_eq!(a.selected_channel, a.channels()[0]);
    a.execute_command("instrument \"One sound\"").unwrap();
    a.press_keys("G").unwrap();
    assert_eq!(a.control_cursor, a.control_descriptors().len() - 1);
    a.press_keys("gg").unwrap();
    assert_eq!(a.control_cursor, 0);
    assert_eq!(a.transport_step(), 33);
    assert!(a.playing());
    assert_eq!(a.project(), &before);
    assert_eq!(a.revision(), revision);
}

#[test]
fn window_prefix_pins_zooms_closes_and_cycles_decks_without_touching_the_project() {
    let mut a = app();
    let before = a.project().clone();
    let revision = a.revision();
    a.execute_command("instrument \"One sound\"").unwrap();
    let target = a.detail;
    assert!(matches!(target, Detail::Instrument(_)));
    assert_eq!(a.focus(), Focus::Inspector);
    a.press_keys("<C-w>").unwrap();
    assert!(a.is_window_prefix());
    assert_eq!(a.mode_name(), "WINDOW");
    a.press_keys("p").unwrap();
    assert!(
        a.view_state.deck_pinned,
        "the instrument deck is pinned to its sound"
    );
    a.press_keys("gp").unwrap();
    assert_eq!(
        (a.workspace(), a.focus()),
        (Workspace::Pattern, Focus::Canvas)
    );
    assert_eq!(a.detail, target, "the pinned deck keeps its sound");
    a.press_keys("l").unwrap();
    assert_eq!(a.cursor, 1);
    // Tab visits every deck on screen and comes back.
    let mut seen = Vec::new();
    for _ in 0..4 {
        a.press_keys("<Tab>").unwrap();
        seen.push(a.focus());
    }
    for focus in [Focus::Canvas, Focus::Inspector, Focus::Library] {
        assert!(seen.contains(&focus), "{focus:?} in {seen:?}");
    }
    let one = instrument(&a, "One sound");
    a.apply(Action::SelectInstrument { instrument: one })
        .unwrap();
    assert_eq!(
        a.focus(),
        Focus::Inspector,
        "showing its sound focuses the pin"
    );
    assert_eq!(
        a.layout().focused().unwrap().target,
        a.layout().pinned()[3].target
    );
    a.press_keys("<C-w>z").unwrap();
    assert!(a.view_state.deck_zoomed && !a.view_state.zoomed);
    a.press_keys("<Esc>").unwrap();
    assert!(!a.view_state.deck_zoomed && a.deck_active());
    a.press_keys("<C-w>x").unwrap();
    assert!(!a.view_state.deck_visible);
    assert!(
        a.execute_command("param cutoff 400").is_err(),
        "a closed deck's controls must not accept invisible edits"
    );
    a.execute_command("deck on").unwrap();
    assert!(a.deck_active());
    assert_eq!(a.detail, target);
    a.press_keys("<C-w><Esc>").unwrap();
    assert!(!a.is_window_prefix());
    a.press_keys(":title draft<C-w>").unwrap();
    assert_eq!(a.command_line(), Some("title draft"));
    assert!(!a.is_window_prefix());
    a.press_keys("<Esc>").unwrap();
    assert_eq!(a.project(), &before);
    assert_eq!(a.revision(), revision);
}

#[test]
fn pinned_sound_controls_and_jam_keep_their_identity_when_canvas_changes() {
    let mut a = app();
    let one = instrument(&a, "One sound");
    let two = instrument(&a, "Two sound");
    let two_lane = a
        .project()
        .lanes
        .iter()
        .find(|l| l.name == "Two")
        .unwrap()
        .id;
    a.apply(Action::SelectInstrument { instrument: one })
        .unwrap();
    a.apply(Action::PinDeck { pinned: true }).unwrap();
    a.apply(Action::SelectLane { lane: two_lane }).unwrap();
    a.press_keys("gp").unwrap();
    assert_eq!(a.selected_instrument, Some(two));
    assert_eq!(a.detail, Detail::Instrument(one));
    assert_eq!(a.scope_instrument(), Some(one));
    assert!(a.chain().contains("Two motif"));
    assert!(a
        .control_descriptors()
        .iter()
        .filter_map(|c| match c.target {
            ControlTarget::Param(
                cadence_core::params::ParamAddress::Module { instrument: id, .. }
                | cadence_core::params::ParamAddress::Rack { instrument: id, .. }
                | cadence_core::params::ParamAddress::Zone { instrument: id, .. }
                | cadence_core::params::ParamAddress::Cable { instrument: id, .. },
            ) => Some(id),
            _ => None,
        })
        .all(|id| id == one));
    a.apply(Action::SetDeck { visible: true }).unwrap();
    a.execute_command("param cutoff 2.4khz").unwrap();
    let rack = &a.project().instrument(one).unwrap().rack;
    let cutoff_param = InstrumentParam::parse_for_rack("cutoff", rack, false).unwrap();
    assert!((rack.read_param_native(cutoff_param).unwrap() - 2400.0).abs() < 0.1);
    assert_eq!(a.selected_instrument, Some(two));
    a.press_keys("gj").unwrap();
    let out = a
        .handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(out.live_notes_on[0].instrument_id, one);
    assert_eq!(
        out.live_notes_on[0].part_id, None,
        "the other part must not drive this audition"
    );
    a.release_held();
    a.press_keys("<c-s-j>").unwrap();
    assert!(a.chain().contains("One sound"));
}

#[test]
fn nested_back_restores_complete_canvas_and_control_context() {
    let mut a = app();
    let one = instrument(&a, "One sound");
    a.cursor = 3;
    a.arrange_ui.step = 48;
    a.source_cursor = 1;
    a.query = "motif".into();
    a.press_keys("zz").unwrap();
    assert!(a.view_state.zoomed);
    let initial = (
        a.workspace(),
        a.focus(),
        a.scope,
        a.selected_pattern,
        a.selected_lane,
        a.selected_placement,
        a.selected_part,
        a.selected_instrument,
        a.selected_channel,
        a.cursor,
        a.arrange_ui.step,
        a.query.clone(),
    );
    a.apply(Action::SelectInstrument { instrument: one })
        .unwrap();
    assert!(
        !a.view_state.zoomed && !a.view_state.deck_zoomed,
        "opening from a zoomed deck shows the new deck unzoomed"
    );
    a.press_keys("zzzz").unwrap();
    assert!(!a.view_state.deck_zoomed);
    a.rack_ui.tab = RackTab::Panel;
    a.press_keys("l").unwrap(); // focus VCO 1 module pill
    a.apply(Action::PinDeck { pinned: true }).unwrap();
    a.press_keys("zz").unwrap();
    assert!(a.view_state.deck_zoomed);
    a.control_cursor = a
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::Output(_)))
        .unwrap();
    let control = a.control_cursor;
    a.press_keys("<Enter>").unwrap();
    assert!(matches!(a.detail, Detail::Channel(_)));
    a.press_keys("<Esc>").unwrap();
    assert_eq!(a.detail, Detail::Instrument(one));
    assert_eq!(a.control_cursor, control);
    assert_eq!(a.rack_ui.tab, RackTab::Panel);
    assert!(a.view_state.deck_pinned && a.view_state.deck_zoomed);
    a.press_keys("<Esc>").unwrap();
    assert!(!a.view_state.deck_zoomed);
    a.press_keys("<Esc>").unwrap();
    assert_eq!(
        (
            a.workspace(),
            a.focus(),
            a.scope,
            a.selected_pattern,
            a.selected_lane,
            a.selected_placement,
            a.selected_part,
            a.selected_instrument,
            a.selected_channel,
            a.cursor,
            a.arrange_ui.step,
            a.query.clone()
        ),
        initial
    );
    assert!(a.view_state.zoomed);
    assert!(
        !a.view_state.deck_visible,
        "the pin came after this point in the trail"
    );
    assert_eq!(a.source_cursor, 1);
}

#[test]
fn local_source_navigation_keeps_pinned_deck_and_deletion_sanitizes_return_state() {
    use cadence_app::layout::Target;
    let mut a = app();
    let one = instrument(&a, "One sound");
    let placement = a.project().lanes[0].placements[0].id;
    a.apply(Action::SelectInstrument { instrument: one })
        .unwrap();
    a.apply(Action::PinDeck { pinned: true }).unwrap();
    a.execute_command(&format!("local {placement}")).unwrap();
    assert_eq!(a.scope, EditScope::Placement(placement));
    assert_eq!(a.detail, Detail::Instrument(one));
    assert!(a.deck_active());
    a.press_keys("gs").unwrap();
    assert_eq!(a.scope, EditScope::Definition);
    assert_eq!(a.detail, Detail::Instrument(one));
    a.execute_command("instrument Unused init").unwrap();
    let unused = instrument(&a, "Unused");
    a.execute_command("deck pin").unwrap();
    a.execute_command("open \"One motif\"").unwrap();
    a.apply(Action::Edit {
        edit: Edit::RemoveInstrument { id: unused },
    })
    .unwrap();
    let shows_unused = |a: &App| {
        a.layout()
            .pinned()
            .iter()
            .chain(a.layout().working())
            .any(|d| d.target == Target::Instrument(unused))
    };
    assert!(!shows_unused(&a), "the deleted sound's deck is gone");
    assert_eq!(a.detail, Detail::Instrument(one), "the other pin remains");
    a.press_keys("<Esc>").unwrap();
    assert!(!shows_unused(&a), "back never returns to a deleted object");
}

#[test]
fn normal_uppercase_u_redoes_the_same_musical_transaction() {
    let mut a = app();
    let before = a.project().clone();
    a.press_keys("x").unwrap();
    let edited = a.project().clone();
    assert_ne!(before, edited);
    a.press_keys("u").unwrap();
    assert_eq!(a.project(), &before);
    a.press_keys("U").unwrap();
    assert_eq!(a.project(), &edited);
}

#[test]
fn library_browse_search_and_edges_leave_local_scope_when_material_changes() {
    use cadence_core::material::PatternValue;
    for keys in ["<Tab>j", "<Tab>G", "<Tab>ge", "/Two"] {
        let mut a = app();
        let placement = a.project().lanes[0].placements[0].id;
        a.execute_command(&format!("local {placement}")).unwrap();
        a.cursor = 37;
        let before = a.project().clone();
        let revision = a.revision();
        a.press_keys(keys).unwrap();
        assert_eq!(a.scope, EditScope::Definition, "{keys}");
        assert_eq!(a.selected_pattern_object().unwrap().name, "Two motif");
        assert_eq!(a.current_length(), 7);
        let PatternValue::Notes(note) = a.current_value(1).unwrap() else {
            panic!()
        };
        assert_eq!(
            note.degree, 2,
            "new header/selection must read the new definition"
        );
        a.press_keys("<Esc>").unwrap();
        assert_eq!(a.scope, EditScope::Placement(placement));
        assert_eq!(a.selected_pattern_object().unwrap().name, "One motif");
        assert_eq!(a.cursor, 37);
        assert_eq!(a.current_length(), a.project().bars * 16);
        assert_eq!(a.project(), &before);
        assert_eq!(a.revision(), revision);
    }
    let mut a = app();
    let placement = a.project().lanes[1].placements[0].id;
    a.execute_command(&format!("local {placement}")).unwrap();
    a.press_keys("<Tab>gg").unwrap();
    assert_eq!(a.scope, EditScope::Definition);
    assert_eq!(a.selected_pattern_object().unwrap().name, "One motif");
    a.execute_command(&format!("local {placement}")).unwrap();
    a.press_keys("/no matches anywhere").unwrap();
    assert_eq!(a.scope, EditScope::Definition);
    assert!(a.selected_pattern.is_none());
    assert!(a.current_value(0).is_err());
    a.press_keys("<Esc>").unwrap();
    assert_eq!(a.scope, EditScope::Placement(placement));
}

#[test]
fn derived_source_navigation_works_directly_with_jk_enter_and_tab_cycles_decks() {
    let mut a = app();
    a.execute_command("pattern degrees Harmony 16 0:0 8:2")
        .unwrap();
    let harmony = a.selected_pattern.unwrap();
    a.execute_command("derive Moving add 'One motif' Harmony")
        .unwrap();
    let moving = a.selected_pattern.unwrap();
    let revision = a.revision();
    // Derived pattern immediately shows source 0 selected; j moves to source 1, Enter opens it!
    a.press_keys("j<Enter>").unwrap();
    assert_eq!(a.selected_pattern, Some(harmony));
    a.press_keys("<Esc>").unwrap();
    assert_eq!(a.selected_pattern, Some(moving));
    assert_eq!(a.source_cursor, 1);
    assert_eq!(a.revision(), revision);
    // Tab cycles directly out to the next deck instead of trapping inside a hidden source bar.
    a.press_keys("<Tab>").unwrap();
    assert_ne!(
        a.focused_deck_kind(),
        Some(cadence_app::layout::DeckKind::Pattern)
    );
}

#[test]
fn a_pinned_pattern_keeps_its_target_while_its_source_opens_below_with_a_trail() {
    use cadence_app::layout::{DeckKind, Target};
    let mut a = App::new(Project::demo(), "unused.json".into()).unwrap();
    let id = |a: &App, name: &str| {
        a.project()
            .patterns
            .iter()
            .find(|p| p.name == name)
            .unwrap()
            .id
    };
    let moving = id(&a, "Moving lead");
    // Arrange, the Bank and Output start pinned; nothing is working yet.
    assert!(a.layout().working().is_none());
    a.execute_command("open \"Moving lead\"").unwrap();
    a.press_keys("<C-w>p").unwrap();
    assert!(a
        .layout()
        .pinned()
        .iter()
        .any(|d| d.kind == DeckKind::Pattern && d.target == Target::Pattern(moving)));
    a.press_keys("gs").unwrap();
    let source = a.layout().working().unwrap().target;
    assert_eq!(
        source,
        Target::Pattern(id(&a, "Motif")),
        "gs opens the first source"
    );
    assert!(
        a.layout()
            .pinned()
            .iter()
            .any(|d| d.target == Target::Pattern(moving)),
        "the pinned deck keeps its pattern"
    );
    assert!(a.trail().last().unwrap().contains("Motif"));
    a.press_keys("<Esc>").unwrap();
    assert!(a.layout().working().is_none_or(|d| d.target != source));
    assert_eq!(a.revision(), 0);
}

#[test]
fn a_placement_deck_replaces_forks_reshapes_and_deletes_only_that_placement() {
    let mut a = App::new(Project::demo(), "unused.json".into()).unwrap();
    a.press_keys("ga").unwrap();
    // Bass lane: one placement from bar 5.
    a.press_keys("jllll<Enter>").unwrap();
    let Detail::Placement(id) = a.detail else {
        panic!("Enter opens the placement deck, got {:?}", a.detail)
    };
    assert!(a.deck_focused());
    let part = |a: &App| a.placement(id).unwrap().1.part;
    let pattern_of = |a: &App| a.part(part(a)).unwrap().pattern;
    let original = (part(&a), pattern_of(&a));
    let others: Vec<_> = a
        .project()
        .lanes
        .iter()
        .flat_map(|l| &l.placements)
        .filter(|p| p.id != id)
        .cloned()
        .collect();

    a.press_keys("r").unwrap();
    assert_eq!(a.mode_name(), "PICK");
    let view = a.picker_view().unwrap();
    assert!(
        view.items.iter().any(|(_, _, current)| *current),
        "the current pattern is marked"
    );
    a.press_keys("1").unwrap();
    assert_eq!(a.mode_name(), "NORMAL");
    let motif = a.project().patterns[0].id;
    assert_eq!(pattern_of(&a), motif);
    let after: Vec<_> = a
        .project()
        .lanes
        .iter()
        .flat_map(|l| &l.placements)
        .filter(|p| p.id != id)
        .cloned()
        .collect();
    assert_eq!(after, others, "other placements are untouched");
    a.press_keys("u").unwrap();
    assert_eq!((part(&a), pattern_of(&a)), original);

    a.press_keys("f").unwrap();
    let forked = pattern_of(&a);
    assert_ne!(forked, original.1);
    assert!(a.pattern(forked).unwrap().name.ends_with("fork"));
    a.press_keys("u").unwrap();

    let (start, length) = {
        let p = a.placement(id).unwrap().1;
        (p.start_step, p.length_steps)
    };
    // Reshape Placement Start and Length directly via Tier 3 (`Alt+H` = -16 steps) on the Placement deck.
    let start_idx = a
        .control_descriptors()
        .iter()
        .position(|c| c.label == "Starts")
        .unwrap();
    let len_idx = a
        .control_descriptors()
        .iter()
        .position(|c| c.label == "Length")
        .unwrap();
    a.control_cursor = start_idx;
    a.press_keys("<m-H>").unwrap();
    a.control_cursor = len_idx;
    a.press_keys("<m-H>").unwrap();
    let p = a.placement(id).unwrap().1;
    assert_eq!((p.start_step, p.length_steps), (start - 16, length - 16));
    a.press_keys("x").unwrap();
    assert!(a.placement(id).is_none());
    a.press_keys("uuu").unwrap();
    let p = a.placement(id).unwrap().1;
    assert_eq!((p.start_step, p.length_steps), (start, length));
}

#[test]
fn instrument_controls_use_hl_wb_edit_and_even_fine_steps() {
    use cadence_core::rack::ModuleParam;

    let mut a = App::new(Project::demo(), "unused.json".into()).unwrap();
    a.press_keys("gi").unwrap();
    assert!(a.deck_focused());
    assert!(matches!(a.detail, Detail::Instrument(_)));
    let Detail::Instrument(inst_id) = a.detail else {
        unreachable!()
    };

    // Switch Rack tabs with zero-pinky H / L in NORMAL (Panel -> Patchbay -> Zones).
    assert_eq!(a.rack_ui.tab, RackTab::Panel);
    a.press_keys("L").unwrap();
    assert_eq!(a.rack_ui.tab, RackTab::Patchbay);
    a.press_keys("L").unwrap();
    assert_eq!(a.rack_ui.tab, RackTab::Zones);
    a.press_keys("H").unwrap();
    assert_eq!(a.rack_ui.tab, RackTab::Patchbay);
    a.press_keys("H").unwrap();
    assert_eq!(a.rack_ui.tab, RackTab::Panel);

    // w / b jump between parameter groups on Quick Panel (QUICK -> RACK -> ROUTING).
    let start_group = a.control_descriptors()[a.control_cursor].group;
    assert_eq!(start_group, "QUICK");
    a.press_keys("w").unwrap();
    assert_eq!(a.control_descriptors()[a.control_cursor].group, "RACK");
    a.press_keys("b").unwrap();
    assert_eq!(a.control_descriptors()[a.control_cursor].group, "QUICK");

    // Cycle Panel module pills with h / l: Quick -> VCO A (1) -> VCO B (2) -> Sub (3).
    a.press_keys("lll").unwrap();
    let sub_level_idx = a
        .control_descriptors()
        .iter()
        .position(|c| {
            matches!(
                c.target,
                ControlTarget::Param(cadence_core::params::ParamAddress::Module {
                    module: 3,
                    param: ModuleParam::Level,
                    ..
                })
            )
        })
        .expect("Sub module Level control visible");
    a.control_cursor = sub_level_idx;

    // Set Sub Level to 0% and verify direct Alt+h/l step 1% and Alt+H/L step 5%.
    a.execute_command("param sub.level 0").unwrap();
    let sub_param = InstrumentParam::Module {
        zone: 1,
        module: 3,
        param: ModuleParam::Level,
    };
    let read_sub = |app: &App| {
        app.instrument(inst_id)
            .unwrap()
            .rack
            .read_param_native(sub_param)
            .unwrap()
    };
    a.press_keys("<m-l>").unwrap();
    assert_eq!(a.mode_name(), "NORMAL", "Alt+l stays modeless in NORMAL");
    assert!((read_sub(&a) - 0.01).abs() < 1e-5, "got {}", read_sub(&a));
    a.press_keys("<m-l>").unwrap();
    assert!((read_sub(&a) - 0.02).abs() < 1e-5, "got {}", read_sub(&a));
    a.press_keys("<m-h>").unwrap();
    assert!(
        a.deck_focused(),
        "Alt+h adjusts slider without leaving deck"
    );
    assert!((read_sub(&a) - 0.01).abs() < 1e-5, "got {}", read_sub(&a));
    a.press_keys("<m-L>").unwrap();
    assert!((read_sub(&a) - 0.06).abs() < 1e-5, "got {}", read_sub(&a));
    a.press_keys("<m-H>").unwrap();
    assert!((read_sub(&a) - 0.01).abs() < 1e-5, "got {}", read_sub(&a));
    assert_eq!(a.mode_name(), "NORMAL");

    // Step back to VCO A (pill 1) and check cents snapping from -1.5 ct in EDIT mode.
    a.press_keys("hh").unwrap();
    let fine_idx = a
        .control_descriptors()
        .iter()
        .position(|c| {
            matches!(
                c.target,
                ControlTarget::Param(cadence_core::params::ParamAddress::Module {
                    module: 1,
                    param: ModuleParam::Fine,
                    ..
                })
            )
        })
        .expect("VCO A Fine control visible");
    a.control_cursor = fine_idx;
    a.execute_command("param vco_a.fine -1.5").unwrap();
    let fine_param = InstrumentParam::Module {
        zone: 1,
        module: 1,
        param: ModuleParam::Fine,
    };
    let read_fine = |app: &App| {
        app.instrument(inst_id)
            .unwrap()
            .rack
            .read_param_native(fine_param)
            .unwrap()
    };
    a.press_keys("<m-l>").unwrap();
    assert!((read_fine(&a) - (-1.0)).abs() < 1e-5);
    a.press_keys("<m-l>").unwrap();
    assert!((read_fine(&a) - 0.0).abs() < 1e-5);
    a.press_keys("<m-l>").unwrap();
    assert!((read_fine(&a) - 1.0).abs() < 1e-5);
    a.press_keys("<m-L>").unwrap();
    assert!((read_fine(&a) - 11.0).abs() < 1e-5);

    // Enter opens unified ValueInput exact entry; Esc returns to NORMAL on the deck.
    a.press_keys("<Enter>").unwrap();
    assert_eq!(a.mode_name(), "VALUE ENTRY");
    let vi = a.value_input_view().expect("ValueInput view");
    assert!(vi.title.to_ascii_lowercase().contains("fine"));
    assert_eq!(vi.input, "");
    a.press_keys("<Esc>").unwrap();
    assert_eq!(a.mode_name(), "NORMAL");
    assert!(a.deck_focused());
}

#[test]
fn take_deck_bank_launchpad_from_here_and_layout_presets_work_end_to_end() {
    use cadence_app::layout::DeckKind;
    use cadence_app::BankMode;

    let mut a = App::new(Project::demo(), "unused.json".into()).unwrap();

    // 1. Bank Launchpad (gb): hjkl navigates lanes/parts, Enter launches, x releases, v switches to Material.
    a.execute_command("lane Lead add Intro").unwrap();
    a.press_keys("gb").unwrap();
    assert_eq!(a.view_state.bank_mode, BankMode::Parts);
    a.press_keys("<Enter>").unwrap();
    assert!(
        a.overriding(),
        "Enter in Bank Parts launches the selected part"
    );
    a.press_keys("x").unwrap();
    assert!(
        !a.overriding(),
        "x in Bank Parts releases the lane's override"
    );
    a.press_keys("v").unwrap();
    assert_eq!(a.view_state.bank_mode, BankMode::Material);
    a.press_keys("v").unwrap();
    assert_eq!(a.view_state.bank_mode, BankMode::Parts);

    // 2. Layout presets z1..z4 switch deck layouts cleanly.
    a.press_keys("z1").unwrap();
    assert!(a
        .layout()
        .working()
        .is_some_and(|d| d.kind == DeckKind::Pattern));
    a.press_keys("z4").unwrap();
    assert!(a
        .layout()
        .working()
        .is_some_and(|d| d.kind == DeckKind::Mixer));
    a.press_keys("z2").unwrap();
    assert!(a.deck_visible_kind(DeckKind::Bank));

    // 3. ' ("from here") forks edited pattern & splits straddling placements from cursor bar in 1 undo!
    let mut b = App::new(Project::demo(), "unused.json".into()).unwrap();
    let baseline = b.project().clone();
    let harmony_id = b
        .project()
        .patterns
        .iter()
        .find(|p| matches!(p.kind, cadence_core::material::PatternKind::Values { .. }))
        .unwrap()
        .id;
    // Place cursor at bar 9 (inside Motion bars 5..16, steps 64..256).
    b.arrange_ui.step = 8 * 16;
    b.selected_pattern = Some(harmony_id);
    b.press_keys("'").unwrap();
    assert!(b.from_here);
    b.execute_command("value Harmony 1 4").unwrap();
    assert!(!b.from_here, "from_here disarms automatically after edit");
    assert!(
        b.project().patterns.len() > baseline.patterns.len(),
        "from_here forked Harmony and its dependent derived patterns"
    );
    // Original Harmony stays completely untouched!
    assert_eq!(
        b.project().pattern(harmony_id),
        baseline.pattern(harmony_id)
    );
    // One single undo restores everything cleanly!
    b.press_keys("u").unwrap();
    assert_eq!(b.project(), &baseline);

    // 4. Take deck (gt): arm take, navigate bars, mark range, keep range with k!
    let mut c = App::new(Project::demo(), "unused.json".into()).unwrap();
    c.apply(Action::SetTakeArmed { armed: true }).unwrap();
    c.apply(Action::Play { playing: true }).unwrap();
    c.advance(16);
    c.press_keys("<m-q>2").unwrap();
    c.advance(48);
    c.press_keys("gt").unwrap();
    assert!(c.take_focused());
    c.press_keys("0lvlv").unwrap();
    assert_eq!(c.takes.mark_start, Some(1));
    assert_eq!(c.takes.mark_end, Some(2));
    let rev_before = c.revision();
    c.press_keys("<Enter>").unwrap();
    assert!(
        c.revision() > rev_before,
        "Enter in Take deck keeps marked range"
    );
}

#[test]
fn rack_tab_hints_and_replace_pattern_picker_filtering() {
    use cadence_app::view::OverlayView;
    use cadence_core::runtime::AudioSnapshot;

    let mut app = App::new(Project::demo(), "unused.json".into()).unwrap();

    // 1. Replace pattern picker (`r` on a Placement) filters strictly to PatternType::Notes.
    app.execute_command("pattern values NonNotesVal 16 ramp 0:0.1 8:0.9")
        .unwrap();
    app.execute_command("pattern rhythm NonNotesRhy 1 . . .")
        .unwrap();
    app.press_keys("ga<Enter>r").unwrap();
    let fv_pick = app.frame_view(&AudioSnapshot::default(), "silent");
    match fv_pick.overlay {
        Some(OverlayView::Picker(p)) => {
            assert!(
                !p.items
                    .iter()
                    .any(|i| i.0 == "NonNotesVal" || i.0 == "NonNotesRhy"),
                "Replace pattern picker must filter strictly to PatternType::Notes, got {:?}",
                p.items.iter().map(|i| &i.0).collect::<Vec<_>>()
            );
        }
        other => panic!("expected Picker overlay, got {other:?}"),
    }
    app.press_keys("<Esc>").unwrap();

    // 2. Tab-aware Detail::Instrument hints across Panel, Patchbay, and Zones.
    app.execute_command("instrument HintSynth synth init")
        .unwrap();
    assert_eq!(app.rack_ui.tab, RackTab::Panel);
    let panel_hints = app.hints();
    assert!(panel_hints.iter().any(|(k, v)| *k == "H/L" && *v == "tab"));
    assert!(panel_hints
        .iter()
        .any(|(k, v)| *k == "h/l" && *v == "module"));
    assert!(panel_hints.iter().any(|(k, v)| *k == "B" && *v == "sample"));

    app.press_keys("L").unwrap();
    assert_eq!(app.rack_ui.tab, RackTab::Patchbay);
    let pb_hints = app.hints();
    assert!(pb_hints.iter().any(|(k, v)| *k == "H/L" && *v == "tab"));
    assert!(pb_hints.iter().any(|(k, v)| *k == "hjkl" && *v == "node"));
    assert!(pb_hints
        .iter()
        .any(|(k, v)| *k == "c/C/x" && *v == "patch/amt/cut"));

    app.press_keys("L").unwrap();
    assert_eq!(app.rack_ui.tab, RackTab::Zones);
    let zone_hints = app.hints();
    assert!(zone_hints.iter().any(|(k, v)| *k == "H/L" && *v == "tab"));
    assert!(zone_hints.iter().any(|(k, v)| *k == "h/l" && *v == "zone"));
    assert!(zone_hints.iter().any(|(k, v)| *k == "j/k" && *v == "ctrl"));
}

#[test]
fn bank_32_slot_scroll_window_tracks_cursor_and_centers_unselected_lane_override() {
    use cadence_app::view::DeckView;
    use cadence_core::material::{Boundary, LanePart, PhaseMode};
    use cadence_core::runtime::AudioSnapshot;

    let mut proj = Project::demo();
    let base_pat = proj.patterns[0].id;
    let lane0_id = proj.lanes[0].id;
    let inst0_id = proj.instruments[0].id;
    let next_start = proj.parts.iter().map(|p| p.id).max().unwrap_or(100) + 10;
    for i in 0..22u64 {
        let pid = next_start + i;
        proj.parts.push(cadence_core::material::Part {
            id: pid,
            name: format!("Slot {:02}", i + 1),
            pattern: base_pat,
            instrument: inst0_id,
            transpose: 0,
            velocity_scale: 1.0,
            parameter_links: Vec::new(),
        });
        if proj.lanes[0].parts.len() < 22 {
            proj.lanes[0].parts.push(LanePart::new(pid));
        }
    }
    proj.lanes[0].parts.truncate(22);
    proj.validate().unwrap();

    let mut app = App::new(proj, "bank_scroll.json".into()).unwrap();
    app.press_keys("gb").unwrap();
    let snap = AudioSnapshot::default();
    let bank_lane0 = |app: &App| {
        let fv = app.frame_view(&snap, "ok");
        fv.decks
            .into_iter()
            .find_map(|d| match d.view {
                DeckView::Bank(b) => b.lanes.into_iter().find(|l| l.lane_id == lane0_id),
                _ => None,
            })
            .expect("Bank deck with lane 0")
    };

    // Selected lane cursor tracking across 22 slots (9 visible rows).
    let lg0 = bank_lane0(&app);
    assert_eq!(lg0.total_parts, 22);
    assert_eq!(lg0.part_window_start, 0);
    assert_eq!(lg0.parts.len(), 9);
    assert_eq!(lg0.parts[0].slot_num, 1);

    for _ in 0..14 {
        app.press_keys("l").unwrap();
    }
    let lg14 = bank_lane0(&app);
    assert_eq!(lg14.total_parts, 22);
    assert_eq!(lg14.parts.len(), 9);
    assert!(lg14.part_window_start > 0);
    assert!(
        lg14.parts.iter().any(|p| p.slot_num == 15),
        "slot 15 must be visible in centered window (start={})",
        lg14.part_window_start
    );

    for _ in 0..7 {
        app.press_keys("l").unwrap();
    }
    let lg21 = bank_lane0(&app);
    assert_eq!(lg21.part_window_start, 13);
    assert_eq!(lg21.parts.len(), 9);
    assert_eq!(lg21.parts.last().unwrap().slot_num, 22);

    // Unselected lane with >9 parts centers its window on the active override part.
    let lane1_id = app.project().lanes[1].id;
    let pat_id = app.project().patterns[0].id;
    let inst_id = app.project().instruments[0].id;
    let mut extra_parts = Vec::new();
    for i in 0..12 {
        let pname = format!("ExtraBank{i}");
        app.apply(Action::CreatePart {
            name: pname.clone(),
            pattern: pat_id,
            instrument: inst_id,
            transpose: None,
            velocity_scale: None,
        })
        .unwrap();
        let pid = app
            .project()
            .parts
            .iter()
            .find(|p| p.name == pname)
            .unwrap()
            .id;
        extra_parts.push(pid);
    }
    let mut lane1 = app.lane(lane1_id).unwrap().clone();
    for &pid in &extra_parts {
        lane1.parts.push(LanePart {
            part: pid,
            phase: PhaseMode::Song,
            offset_steps: 0,
        });
    }
    let target_part = *extra_parts.last().unwrap();
    let target_slot_idx = lane1.parts.len() - 1;
    assert!(target_slot_idx >= 12);
    app.apply(Action::Edit {
        edit: Edit::PutLane { lane: lane1 },
    })
    .unwrap();

    app.apply(Action::Launch {
        lane: lane1_id,
        part: LanePart {
            part: target_part,
            phase: PhaseMode::Song,
            offset_steps: 0,
        },
        at: Some(Boundary::Now),
    })
    .unwrap();
    app.press_keys("gb").unwrap();
    app.view_state.bank_lane_idx = 0; // Keep lane 1 unselected.
    let fv_bank = app.frame_view(&snap, "ok");
    let bank = fv_bank
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Bank(b) => Some(b),
            _ => None,
        })
        .expect("BankDeckView");
    let lane1_view = &bank.lanes[1];
    assert!(
        lane1_view.part_window_start > 0,
        "Unselected lane >9 parts window must scroll to center active override, got start=0"
    );
    assert!(
        lane1_view.parts.iter().any(|p| p.part_id == target_part),
        "Active override part at slot {target_slot_idx} must be visible in unselected lane's window"
    );
}
