use cadence_app::{Action, App, EditScope, Request};
use cadence_core::{
    follow::{FollowMode, PageOrigin},
    material::{Edit, Project},
    mixer::ChannelRef,
    params::InstrumentParam,
    playback::PlaybackClock,
};

fn app(pattern: u64) -> App {
    let mut app = App::new(Project::demo(), "unused.json".into()).unwrap();
    app.apply(Action::SelectPattern { pattern }).unwrap();
    app.selected_placement = Some(2);
    app
}
fn follow(app: &mut App) {
    app.execute_command("follow on").unwrap();
}

#[test]
fn follow_queries_and_toggles_do_not_move_editors_or_create_undo_entries() {
    let mut a = app(4);
    a.cursor = 3;
    a.arrange_ui.step = 16;
    let project = a.project().clone();
    let selected = (
        a.selected_pattern,
        a.selected_lane,
        a.selected_placement,
        a.scope,
    );
    follow(&mut a);
    let page = a.pattern_view_page(83).unwrap();
    assert_eq!((page.start, page.anchor), (80, Some(83)));
    assert_eq!(a.arrange_view_page(83, 4).start, 64);
    assert_eq!((a.cursor, a.arrange_ui.step), (3, 16));
    assert_eq!(
        selected,
        (
            a.selected_pattern,
            a.selected_lane,
            a.selected_placement,
            a.scope
        )
    );
    a.press_keys("tf").unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Off);
    a.apply(Action::ToggleFollow).unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Active);
    assert!(!a.apply(Action::Undo).unwrap().changed);
    assert_eq!(a.revision(), 0);
    assert_eq!(a.project(), &project);
    assert!(!a.dirty());
    let (reply, _) = a.execute_request(Request {
        id: None,
        expected_revision: Some(0),
        compact: false,
        action: Action::GetState,
    });
    assert_eq!(reply["follow"], "active");
    assert!(a.execute_command("follow sideways").is_err());
    assert_eq!(a.follow_mode(), FollowMode::Active);
}

#[test]
fn manual_navigation_returns_to_saved_editing_page_and_tf_resumes() {
    for keys in ["l", "<PageDown>", "ge", "gg", "G", "$", "0"] {
        let mut a = app(4);
        a.cursor = 2;
        follow(&mut a);
        a.observe_transport(83, 83 + 1, true, false);
        a.press_keys(keys).unwrap();
        assert_eq!(a.follow_mode(), FollowMode::Suspended, "{keys}");
        let page = a.pattern_view_page(83).unwrap();
        assert_eq!(page.origin, PageOrigin::Editing, "{keys}");
        assert_eq!(page.start, a.cursor / 16 * 16, "{keys}");
        a.press_keys("tf").unwrap();
        assert_eq!(a.pattern_view_page(83).unwrap().start, 80);
        assert_eq!(a.revision(), 0);
    }
    for keys in ["w", "b", "j", "k"] {
        let mut a = app(2);
        a.cursor = 32;
        follow(&mut a);
        a.press_keys(keys).unwrap();
        assert_eq!(a.follow_mode(), FollowMode::Suspended, "{keys}");
        assert_eq!(a.pattern_view_page(83).unwrap().origin, PageOrigin::Editing);
    }
    let mut a = app(4);
    a.press_keys("ga").unwrap();
    a.arrange_ui.step = 16;
    follow(&mut a);
    a.observe_transport(200, 200 + 1, true, false);
    // Playhead-seeded handoff: pressing `l` in Arrange Placements seeds from bar 13 (step 192)
    // and advances 1 bar to bar 14 (step 208) instead of snapping back to parked step 16!
    a.press_keys("l").unwrap();
    assert_eq!(a.arrange_ui.step, 208);
    assert_eq!(a.arrange_view_page(200, 4).start, 192);
    assert_eq!(a.follow_mode(), FollowMode::Suspended);
    a.execute_command("follow resume").unwrap();
    assert_eq!(a.arrange_view_page(200, 4).start, 192);
}

#[test]
fn playhead_seeded_follow_handoff_in_arrange_pattern_and_jam() {
    // 1. Arrange Placements: h/l/j/k seed to current heard bar before moving.
    let mut a = app(4);
    a.press_keys("ga").unwrap();
    a.arrange_ui.step = 0;
    a.press_keys("tf").unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Active);
    a.observe_transport(135, 136, true, false); // bar 9 (bar start = 128)
    a.press_keys("h").unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Suspended);
    assert_eq!(a.arrange_ui.step, 112, "bar 9 (128) minus 1 bar = 112");

    // Resume with `tf`, advance playhead to bar 11 (step 165 -> bar start 160), press `j` (lane move).
    a.press_keys("tf").unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Active);
    a.observe_transport(165, 166, true, false);
    a.press_keys("j").unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Suspended);
    assert_eq!(a.arrange_ui.step, 160, "lane switch keeps playhead bar 11");

    // 2. Arrange Overview & Lane Roll: exact heard step seeding + w/b event jump around playhead.
    a.press_keys("vo").unwrap();
    a.arrange_ui.step = 0;
    a.press_keys("tf").unwrap();
    a.observe_transport(98, 99, true, false);
    a.press_keys("l").unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Suspended);
    assert_eq!(
        a.arrange_ui.step, 99,
        "overview `l` steps +1 from playhead 98"
    );

    a.press_keys("vr").unwrap();
    a.arrange_ui.step = 0;
    a.press_keys("tf").unwrap();
    a.observe_transport(100, 101, true, false);
    a.press_keys("w").unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Suspended);
    assert!(
        a.arrange_ui.step >= 100,
        "roll `w` searches forward from playhead 100, got {}",
        a.arrange_ui.step
    );

    // 3. Pattern canvas: seeds `cursor` to active playback step inside pattern's period!
    let mut p = app(4);
    p.cursor = 2;
    p.press_keys("tf").unwrap();
    p.observe_transport(83, 84, true, false);
    p.press_keys("l").unwrap();
    assert_eq!(p.follow_mode(), FollowMode::Suspended);
    assert_eq!(
        p.cursor, 84,
        "pattern `l` stepped from playhead step 83 -> 84"
    );
    p.press_keys("tf").unwrap();
    assert_eq!(p.follow_mode(), FollowMode::Active);
    p.press_keys("h").unwrap();
    assert_eq!(
        p.cursor, 82,
        "pattern `h` stepped from playhead step 83 -> 82"
    );

    // 4. Jam mode MUST NOT suspend follow or overwrite parked arrange_step.
    let mut j = app(4);
    j.press_keys("ga").unwrap();
    j.arrange_ui.step = 16;
    j.press_keys("tf").unwrap();
    j.observe_transport(144, 145, true, false);
    j.press_keys("gj").unwrap();
    assert!(j.is_jam());
    j.press_keys("l").unwrap();
    assert_eq!(j.follow_mode(), FollowMode::Active);
    assert_eq!(j.arrange_ui.step, 16);
}

#[test]
fn point_entry_edits_saved_position_and_invalid_transactions_do_not_suspend() {
    let mut a = app(2);
    a.cursor = 32;
    follow(&mut a);
    a.observe_transport(83, 83 + 1, true, false);
    assert_eq!(a.pattern_view_page(83).unwrap().start, 16);
    a.press_keys("<Enter>").unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Suspended);
    assert_eq!(a.pattern_view_page(83).unwrap().start, 32);
    a.press_keys("<Esc>").unwrap();
    assert_eq!(a.revision(), 0);
    follow(&mut a);
    assert!(a
        .apply(Action::Edit {
            edit: Edit::SetValue {
                pattern: 2,
                step: 99,
                value: Some(4.0)
            }
        })
        .is_err());
    assert_eq!(a.follow_mode(), FollowMode::Active);
    a.apply(Action::Edit {
        edit: Edit::SetValue {
            pattern: 2,
            step: 32,
            value: Some(3.0),
        },
    })
    .unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Suspended);
    assert_eq!(a.cursor, 32);
    assert_eq!(a.revision(), 1);
    follow(&mut a);
    a.apply(Action::Undo).unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Suspended);
    assert_eq!(a.revision(), 2);
    follow(&mut a);
    // Even a valid no-op point command explicitly targets the editing material.
    a.apply(Action::Edit {
        edit: Edit::SetValue {
            pattern: 2,
            step: 32,
            value: Some(2.0),
        },
    })
    .unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Suspended);
    assert_eq!(a.revision(), 2);
}

#[test]
fn sound_deck_mixer_and_performance_toggles_keep_follow_active() {
    let mut a = app(4);
    follow(&mut a);
    a.apply(Action::SelectInstrument { instrument: 1 }).unwrap();
    a.press_keys("jklgegg").unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Active);
    let cutoff_param =
        InstrumentParam::parse_for_rack("cutoff", &a.project().instrument(1).unwrap().rack, false)
            .unwrap();
    a.apply(Action::SetParam {
        address: cadence_core::params::ParamAddress::from_instrument(1, cutoff_param),
        value: 800.0,
    })
    .unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Active);
    a.apply(Action::Undo).unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Active);
    a.press_keys("gm").unwrap();
    a.selected_channel = ChannelRef::Channel(1);
    a.press_keys("jkhl<PageDown><PageUp>ms").unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Active);
    a.apply(Action::SetLaneMuted {
        lane: 1,
        muted: true,
    })
    .unwrap();
    a.apply(Action::SetLaneSolo {
        lane: 2,
        solo: true,
    })
    .unwrap();
    a.apply(Action::Play { playing: true }).unwrap();
    a.apply(Action::RestartLoop).unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Active);
}

#[test]
fn source_local_navigation_and_back_do_not_restore_old_follow_mode() {
    let mut a = app(4);
    follow(&mut a);
    a.execute_command("local 2").unwrap();
    assert_eq!(a.scope, EditScope::Placement(2));
    assert_eq!(a.follow_mode(), FollowMode::Suspended);
    a.execute_command("follow resume").unwrap();
    assert_eq!(a.pattern_view_page(83).unwrap().anchor, Some(19));
    a.press_keys("gs").unwrap();
    assert_eq!(a.scope, EditScope::Definition);
    assert_eq!(a.follow_mode(), FollowMode::Suspended);
    a.press_keys("<Backspace>").unwrap();
    assert_eq!(a.scope, EditScope::Placement(2));
    assert_eq!(a.follow_mode(), FollowMode::Suspended);
    a.execute_command("follow resume").unwrap();
    a.apply(Action::SelectPattern { pattern: 2 }).unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Suspended);
    a.execute_command("follow resume").unwrap();
    a.press_keys("gbv").unwrap();
    a.press_keys("j").unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Suspended);
}

#[test]
fn cached_song_gaps_and_preview_clocks_remain_separate() {
    let mut a = app(4);
    a.cursor = 3;
    a.arrange_ui.step = 16;
    follow(&mut a);
    a.observe_transport(83, 83 + 1, false, false);
    a.observe_transport(20, 20 + 1, false, false);
    let held = a.pattern_view_page(20).unwrap();
    assert_eq!((held.start, held.origin), (80, PageOrigin::Held));
    a.apply(Action::Audition {
        pattern: 4,
        length: Some(320),
    })
    .unwrap();
    assert!(matches!(
        a.playback_request(51).unwrap().clock,
        PlaybackClock::Audition { .. }
    ));
    a.observe_transport(51, 51 + 1, false, false);
    assert_eq!(a.pattern_view_page(51).unwrap().start, 48);
    assert_eq!(a.arrange_view_page(200, 4).start, 0);
    a.apply(Action::StopAudition).unwrap();
    assert_eq!(a.pattern_view_page(20).unwrap(), held);
    a.press_keys("<Home>").unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Active);
    assert_eq!(a.transport_step(), 0);
    assert_eq!(a.pattern_view_page(0).unwrap().origin, PageOrigin::Editing);
    assert_eq!(a.arrange_view_page(0, 4).start, 0);
    assert_eq!((a.cursor, a.arrange_ui.step), (3, 16));
}

#[test]
fn off_mixer_and_hidden_pattern_views_do_not_record_invisible_clocks() {
    for hidden in ["off", "mixer", "deck"] {
        let mut a = app(4);
        if hidden != "off" {
            follow(&mut a);
        }
        if hidden == "mixer" {
            a.press_keys("gm").unwrap();
        }
        if hidden == "deck" {
            a.apply(Action::SelectInstrument { instrument: 1 }).unwrap();
            a.press_keys("zz").unwrap();
        }
        a.observe_transport(83, 83 + 1, true, false);
        a.press_keys("gp").unwrap();
        follow(&mut a);
        assert_eq!(
            a.pattern_view_page(20).unwrap().origin,
            PageOrigin::Editing,
            "{hidden}"
        );
    }
}

#[test]
fn document_boundaries_clear_old_anchors_but_keep_the_preference() {
    let mut a = app(4);
    follow(&mut a);
    a.observe_transport(83, 83 + 1, true, false);
    assert_eq!(a.pattern_view_page(20).unwrap().origin, PageOrigin::Held);
    a.apply(Action::New {
        title: Some("Another".into()),
    })
    .unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Active);
    a.apply(Action::Undo).unwrap();
    assert_eq!(a.follow_mode(), FollowMode::Active);
    a.selected_pattern = Some(4);
    a.scope = EditScope::Definition;
    a.cursor = 3;
    assert_eq!(a.pattern_view_page(20).unwrap().origin, PageOrigin::Editing);
}
