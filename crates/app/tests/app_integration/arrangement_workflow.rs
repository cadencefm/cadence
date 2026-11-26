use cadence_app::{Action, App, ArrangeView, EditScope, Focus, Request, Workspace};
use cadence_core::{
    arrangement_view::EventKey,
    follow::FollowMode,
    material::{
        Edit, Exception, InstrumentInstance, Lane, LanePart, Part, Pattern, PatternKind,
        PatternValue, PhaseMode, Placement, Project, ValuePoint, ValueUnit,
    },
    mixer::{Channel, ChannelKind},
    music::{ChordTone, Step},
    params::ValueMode,
    rack::RackPatch,
    sampler::SampleLibrary,
};

fn step(degree: i16, gate: f32) -> Step {
    Step {
        active: true,
        degree: degree + 1,
        velocity: 0.8,
        gate,
        ..Step::default()
    }
}
fn project() -> Project {
    let mut p = Project::empty();
    p.bars = 8;
    p.patterns = vec![
        Pattern {
            id: 1,
            name: "Motif".into(),
            kind: PatternKind::Notes {
                steps: [0, 2, 4, 7, 6].into_iter().map(|d| step(d, 0.75)).collect(),
            },
        },
        Pattern {
            id: 2,
            name: "Harmony".into(),
            kind: PatternKind::Values {
                unit: ValueUnit::Degrees,
                period: 64,
                mode: ValueMode::Hold,
                points: [(0, 0.0), (24, -2.0), (32, 2.0), (56, -1.0)]
                    .into_iter()
                    .map(|(step, value)| ValuePoint { step, value })
                    .collect(),
            },
        },
        Pattern {
            id: 3,
            name: "Moving".into(),
            kind: PatternKind::AddDegrees {
                notes: 1,
                values: 2,
            },
        },
    ];
    for (channel, instrument, part, lane, placement, pattern) in
        [(20, 21, 22, 23, 24, 3), (30, 31, 32, 33, 34, 1)]
    {
        p.mixer.channels.push(Channel::new(
            channel,
            ChannelKind::Audio,
            format!("Audio{channel}"),
        ));
        p.instruments.push(InstrumentInstance {
            id: instrument,
            name: format!("Sound{instrument}"),
            channel,
            rack: RackPatch::default_synth(),
        });
        p.parts
            .push(Part::new(part, format!("Part{part}"), pattern, instrument));
        p.lanes.push(Lane {
            id: lane,
            name: format!("Lane{lane}"),
            parts: vec![LanePart::new(part)],
            placements: vec![Placement::new(placement, part, 0, 128)],
        });
    }
    p.parts[0].transpose = 12;
    let place = &mut p.lanes[0].placements[0];
    place.start_step = 16;
    place.length_steps = 48;
    place.phase = PhaseMode::Restart;
    place.offset_steps = -3;
    p
}
fn app(p: Project) -> App {
    App::new_with_library(
        p,
        "unused.json".into(),
        SampleLibrary::from_roots_only(&[], None),
    )
    .unwrap()
}
fn select(a: &mut App, key: EventKey) {
    a.apply(Action::SelectArrangementEvent { event: key })
        .unwrap();
}
fn key(step: u32, tone: u8) -> EventKey {
    EventKey {
        placement: 24,
        song_step: step,
        sub_step: 0,
        tone,
    }
}

#[test]
fn view_menu_commands_and_typed_queries_are_observational() {
    let mut a = app(project());
    let original = a.project().clone();
    a.cursor = 3;
    a.arrange_ui.step = 41;
    for (keys, view) in [
        ("vo", ArrangeView::Overview),
        ("vr", ArrangeView::Roll),
        ("vp", ArrangeView::Placements),
    ] {
        a.press_keys(keys).unwrap();
        assert_eq!(a.arrange_ui.view, view);
        assert_eq!((a.arrange_ui.step, a.cursor), (41, 3));
    }
    a.press_keys("v<Esc>").unwrap();
    assert_eq!(a.arrange_ui.view, ArrangeView::Placements);
    a.execute_command("arrange roll").unwrap();
    assert_eq!(a.arrange_ui.view, ArrangeView::Roll);
    a.execute_command("arrange").unwrap();
    assert_eq!(a.arrange_ui.view, ArrangeView::Roll);
    assert!(a.execute_command("arrange invalid").is_err());
    a.apply(Action::SetArrangeView {
        view: ArrangeView::Overview,
    })
    .unwrap();
    let (response, _) = a.execute_request(Request {
        id: None,
        expected_revision: Some(0),
        compact: false,
        action: Action::InspectArrangementEvent {
            event: Some(key(43, 0)),
        },
    });
    assert_eq!(response["ok"], true);
    assert_eq!(response["inspection"]["note"]["pattern_time"], 24);
    assert_eq!(response["selection"]["arrange_view"], "overview");
    assert_eq!(a.project(), &original);
    assert_eq!(a.revision(), 0);
    assert!(!a.dirty());
    assert!(!a.apply(Action::Undo).unwrap().changed);
}

#[test]
fn evaluated_navigation_has_step_chord_lane_and_page_semantics() {
    let mut p = project();
    let PatternKind::Notes { steps } = &mut p.patterns[0].kind else {
        panic!()
    };
    steps[0].chord = vec![
        ChordTone {
            degree: 3,
            accidental: 0,
            velocity: 0.7,
            gate: 1.0,
        },
        ChordTone {
            degree: 5,
            accidental: 0,
            velocity: 0.6,
            gate: 2.0,
        },
    ];
    let mut a = app(p);
    a.apply(Action::SetArrangeView {
        view: ArrangeView::Roll,
    })
    .unwrap();
    a.selected_lane = Some(33);
    a.arrange_ui.step = 0;
    a.press_keys("w").unwrap();
    assert_eq!(
        (a.arrange_ui.step, a.arrange_ui.event.unwrap().tone),
        (0, 1)
    );
    a.press_keys("w").unwrap();
    assert_eq!(
        (a.arrange_ui.step, a.arrange_ui.event.unwrap().tone),
        (0, 2)
    );
    a.press_keys("w").unwrap();
    assert_eq!(
        (a.arrange_ui.step, a.arrange_ui.event.unwrap().tone),
        (1, 0)
    );
    a.press_keys("b").unwrap();
    assert_eq!(
        (a.arrange_ui.step, a.arrange_ui.event.unwrap().tone),
        (0, 2)
    );
    a.press_keys("3l").unwrap();
    assert_eq!(a.arrange_ui.step, 3);
    assert!(a.arrange_ui.event.is_none());
    a.press_keys("k").unwrap();
    assert_eq!(a.selected_lane, Some(23));
    assert_eq!(a.arrange_ui.step, 3);
    a.press_keys("j").unwrap();
    assert_eq!(a.selected_lane, Some(33));
    assert_eq!(a.arrange_ui.step, 3);
    a.set_arrange_page_size(32);
    a.press_keys("<PageDown>").unwrap();
    assert_eq!(a.arrange_ui.step, 35);
    a.press_keys("<PageUp>").unwrap();
    assert_eq!(a.arrange_ui.step, 3);
    a.press_keys("ge").unwrap();
    assert_eq!(a.arrange_ui.step, 127);
    a.press_keys("gg").unwrap();
    assert_eq!(a.arrange_ui.step, 0);
    a.press_keys("vpl").unwrap();
    assert_eq!(a.arrange_ui.step, 16);
    assert_eq!(a.revision(), 0);
}

#[test]
fn provenance_follows_each_source_clock_then_restores_the_exact_inspector() {
    let mut a = app(project());
    select(&mut a, key(43, 0));
    a.press_keys("<Enter>").unwrap();
    let inspector = a.arrangement_inspection().unwrap();
    assert_eq!(inspector.event.unwrap().midi, 81); // A4 plus binding octave.
    assert_eq!(
        (
            inspector.sources[0].pattern,
            inspector.sources[0].step,
            inspector.sources[0].period
        ),
        (3, 24, 320)
    );
    a.press_keys("j").unwrap();
    let selected = a.arrangement_inspection().unwrap();
    assert_eq!(
        (
            selected.sources[selected.selected].pattern,
            selected.sources[selected.selected].step
        ),
        (1, 4)
    );
    a.press_keys("s").unwrap();
    assert_eq!(
        (a.workspace(), a.selected_pattern, a.cursor, a.scope),
        (Workspace::Pattern, Some(1), 4, EditScope::Definition)
    );
    assert_eq!(a.arrange_ui.step, 43);
    a.press_keys("<Esc>").unwrap();
    assert!(a.is_arrangement_inspection());
    assert_eq!(a.arrangement_inspection().unwrap().selected, 1);
    assert_eq!(a.arrange_ui.event, Some(key(43, 0)));
    a.press_keys("j<Enter>").unwrap();
    assert_eq!((a.selected_pattern, a.cursor), (Some(2), 24));
    a.press_keys("<Esc><Esc>").unwrap();
    assert_eq!(
        (a.workspace(), a.focus()),
        (Workspace::Arrange, Focus::Canvas)
    );
    assert!(!a.is_arrangement_inspection());
    assert_eq!(a.arrange_ui.step, 43);
    assert_eq!(a.revision(), 0);
}

#[test]
fn local_follow_uses_occurrence_coordinates_and_pre_binding_note_values() {
    let mut p = project();
    p.lanes[0].placements[0].exceptions.push(Exception {
        step: 27,
        value: step(0, 3.0),
    });
    let mut a = app(p);
    select(&mut a, key(43, 0));
    a.press_keys("<Enter>").unwrap();
    let v = a.arrangement_inspection().unwrap();
    assert_eq!(v.event.unwrap().midi, 74); // D4 local intent plus binding +12.
    assert!(v
        .sources
        .iter()
        .all(|s| s.role.contains("replaced locally")));
    a.press_keys("o").unwrap();
    assert_eq!(a.scope, EditScope::Placement(24));
    assert_eq!(a.cursor, 27);
    let PatternValue::Notes(note) = a.current_value(a.cursor).unwrap() else {
        panic!()
    };
    assert_eq!(
        note.degree, 1,
        "local editor stores D4 intent, not the octave-shifted event"
    );
    a.press_keys("x").unwrap();
    assert_eq!(a.revision(), 1);
    a.press_keys("<Esc>").unwrap();
    assert!(a.is_arrangement_inspection());
    assert!(
        a.arrangement_inspection().unwrap().error.is_some(),
        "removed event does not silently retarget"
    );
    a.press_keys("<Esc>u").unwrap();
    assert!(a.selected_arrangement_event().is_some());
}

#[test]
fn incoming_tail_inspects_and_edits_its_owner_across_a_different_placement() {
    let mut p = project();
    p.parts[0].pattern = 1;
    p.parts[0].transpose = 0;
    p.patterns[0].kind = PatternKind::Notes {
        steps: (0..16)
            .map(|s| {
                if s == 15 {
                    step(0, 8.0)
                } else {
                    Step::default()
                }
            })
            .collect(),
    };
    p.patterns.push(Pattern {
        id: 5,
        name: "Silence".into(),
        kind: PatternKind::Notes {
            steps: vec![Step::default(); 16],
        },
    });
    p.parts.push(Part::new(26, "Quiet", 5, 21));
    p.lanes[0].placements = vec![
        Placement::new(24, 22, 0, 16),
        Placement::new(25, 26, 16, 16),
    ];
    let mut a = app(p);
    a.execute_command("arrange roll").unwrap();
    a.press_keys("17l").unwrap();
    assert_eq!(a.selected_placement, Some(25));
    assert_eq!(a.arrange_ui.step, 17);
    let note = a.selected_arrangement_event().unwrap();
    assert_eq!(note.key, key(15, 0));
    a.press_keys("<Enter>o").unwrap();
    assert_eq!(a.scope, EditScope::Placement(24));
    assert_eq!(a.cursor, 15);
    assert_eq!(a.arrange_ui.step, 17);
    a.press_keys("<Esc>").unwrap();
    assert!(a.is_arrangement_inspection());
    assert_eq!(
        a.arrangement_inspection().unwrap().event.unwrap().key,
        key(15, 0)
    );
    a.press_keys("<Esc>").unwrap();
    assert_eq!(a.selected_placement, Some(25));
    assert_eq!(a.arrange_ui.step, 17);
}

#[test]
fn follow_home_and_instrument_controls_preserve_parked_event_selection() {
    let mut a = app(project());
    select(&mut a, key(43, 0));
    a.press_keys("tf").unwrap();
    a.observe_transport(100, 100 + 1, true, false);
    assert_eq!(a.arrange_view_page(100, 4).start, 64);
    assert_eq!(a.arrange_ui.step, 43);
    a.press_keys("<Enter>i").unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Active);
    a.press_keys("=<Esc>").unwrap();
    assert!(a.is_arrangement_inspection());
    assert_eq!(a.arrange_ui.event, Some(key(43, 0)));
    assert_eq!(a.arrange_ui.step, 43);
    a.press_keys("<Home>").unwrap();
    assert_eq!(a.transport_step(), 0);
    assert_eq!(a.arrange_ui.step, 43);
    assert!(a.is_arrangement_inspection());
    a.press_keys("<Esc>w").unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Suspended);
    a.press_keys("tf").unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Active);
}

#[test]
fn deleted_or_replaced_objects_cannot_reuse_an_old_event_selection() {
    let mut a = app(project());
    select(&mut a, key(43, 0));
    a.apply(Action::Edit {
        edit: Edit::RemovePlacement { id: 24 },
    })
    .unwrap();
    assert!(a.arrange_ui.event.is_none());
    assert!(a.selected_arrangement_event().is_none());
    assert!(a
        .apply(Action::SelectArrangementEvent { event: key(43, 0) })
        .is_err());
    a.apply(Action::Undo).unwrap();
    select(&mut a, key(43, 0));
    a.apply(Action::New { title: None }).unwrap();
    assert!(a.arrange_ui.event.is_none());
    a.apply(Action::Undo).unwrap();
    assert!(
        a.arrange_ui.event.is_none(),
        "document changes clear ephemeral note identities"
    );
}

#[test]
fn pitch_bank_anchor_stays_parked_through_rests_and_resets_for_another_lane() {
    let mut a = app(project());
    select(&mut a, key(43, 0));
    let pitch = a.selected_arrangement_event().unwrap().midi;
    assert_eq!(a.arrange_ui.pitch_anchor, Some(pitch));
    let original = a.project().clone();
    a.press_keys("vr22l").unwrap();
    assert_eq!(a.arrange_ui.step, 65);
    assert!(a.selected_arrangement_event().is_none());
    assert_eq!(a.arrange_ui.pitch_anchor, Some(pitch));
    a.observe_transport(20, 20 + 1, true, false);
    assert_eq!(
        a.arrange_ui.pitch_anchor,
        Some(pitch),
        "playback cannot drag the pitch bank"
    );
    a.press_keys("j").unwrap();
    assert_eq!(
        a.arrange_ui.pitch_anchor,
        Some(a.selected_arrangement_event().unwrap().midi)
    );
    a.apply(Action::SelectLane { lane: 23 }).unwrap();
    assert!(
        a.arrange_ui.pitch_anchor.is_none(),
        "empty new lane does not inherit another lane's pitch"
    );
    select(&mut a, key(43, 0));
    a.press_keys("<Enter>js<Esc>").unwrap();
    assert_eq!(a.arrange_ui.pitch_anchor, Some(pitch));
    assert!(a.is_arrangement_inspection());
    assert_eq!(a.project(), &original);
    assert_eq!(a.revision(), 0);
    a.apply(Action::New { title: None }).unwrap();
    assert!(a.arrange_ui.pitch_anchor.is_none());
}
