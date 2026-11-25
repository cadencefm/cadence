use cadence_core::{
    follow::{arrange_page, FollowMode, FollowState, PageOrigin},
    material::{LiveFlags, PatternKind, PhaseMode, Project, ValuePoint, ValueUnit},
    mixer::ChannelRef,
    playback::{PlaybackClock, PlaybackRequest, PlaybackScope},
};
fn request(pattern: u64, step: f64) -> PlaybackRequest {
    PlaybackRequest {
        pattern,
        scope: PlaybackScope::Definition,
        preferred_placement: Some(2),
        clock: PlaybackClock::Song { step },
    }
}
fn active() -> FollowState {
    let mut state = FollowState::default();
    state.set_enabled(true);
    state
}

#[test]
fn follow_modes_are_session_only_and_manual_suspension_returns_to_editing_page() {
    let project = Project::demo();
    let flags = LiveFlags::default();
    let before = project.clone();
    let mut state = FollowState::default();
    let req = request(4, 83.0);
    let edit_cursor = 2;
    assert_eq!(
        state
            .pattern_page(&project, &flags, req, edit_cursor)
            .unwrap()
            .origin,
        PageOrigin::Editing
    );
    assert_eq!(state.toggle(), FollowMode::Active);
    assert_eq!(
        state
            .pattern_page(&project, &flags, req, edit_cursor)
            .unwrap()
            .start,
        80
    );
    assert_eq!(state.suspend(), FollowMode::Suspended);
    assert_eq!(
        state
            .pattern_page(&project, &flags, req, edit_cursor)
            .unwrap()
            .start,
        0
    );
    assert_eq!(state.toggle(), FollowMode::Active);
    assert_eq!(
        state
            .pattern_page(&project, &flags, req, edit_cursor)
            .unwrap()
            .anchor,
        Some(83)
    );
    assert_eq!(state.toggle(), FollowMode::Off);
    assert_eq!(state.suspend(), FollowMode::Off);
    assert_eq!(project, before);
    assert_eq!(edit_cursor, 2);
}

#[test]
fn headless_follow_uses_independent_five_and_sixty_four_step_clocks() {
    let project = Project::demo();
    let flags = LiveFlags::default();
    let state = active();
    let motif = state
        .pattern_page(&project, &flags, request(1, 83.0), 0)
        .unwrap();
    let harmony = state
        .pattern_page(&project, &flags, request(2, 83.0), 0)
        .unwrap();
    let moving = state
        .pattern_page(&project, &flags, request(4, 83.0), 0)
        .unwrap();
    assert_eq!((motif.start, motif.count, motif.anchor), (0, 5, Some(3)));
    assert_eq!(
        (harmony.start, harmony.count, harmony.anchor),
        (16, 16, Some(19))
    );
    assert_eq!((moving.start, moving.anchor), (80, Some(83)));
    assert_eq!(moving.origin, PageOrigin::Transport);
    // A parked observed clock retains exactly the same page; no wall-clock inference.
    assert_eq!(
        moving,
        state
            .pattern_page(&project, &flags, request(4, 83.0), 0)
            .unwrap()
    );
}

#[test]
fn restart_offset_and_local_clocks_have_distinct_follow_positions() {
    let mut project = Project::demo();
    let flags = LiveFlags::default();
    let place = project.placement_mut(2).unwrap();
    place.phase = PhaseMode::Restart;
    place.offset_steps = -3;
    let state = active();
    assert_eq!(
        state
            .pattern_page(&project, &flags, request(4, 80.0), 0)
            .unwrap()
            .anchor,
        Some(13)
    );
    assert_eq!(
        state
            .pattern_page(&project, &flags, request(1, 80.0), 0)
            .unwrap()
            .anchor,
        Some(3)
    );
    let local = PlaybackRequest {
        scope: PlaybackScope::Placement(2),
        ..request(4, 80.0)
    };
    assert_eq!(
        state
            .pattern_page(&project, &flags, local, 0)
            .unwrap()
            .anchor,
        Some(16)
    );
    let negative = state
        .pattern_page(&project, &flags, request(4, 64.5), 0)
        .unwrap();
    assert_eq!((negative.start, negative.anchor), (304, Some(317)));
}

#[test]
fn gaps_hold_only_the_same_song_pattern_and_scope_and_preview_never_leaks() {
    let project = Project::demo();
    let flags = LiveFlags::default();
    let mut state = active();
    state.observe(&project, &flags, request(4, 83.0)).unwrap();
    let gap = state
        .pattern_page(&project, &flags, request(4, 20.0), 0)
        .unwrap();
    assert_eq!(
        (gap.start, gap.anchor, gap.origin),
        (80, Some(83), PageOrigin::Held)
    );
    let other = state
        .pattern_page(&project, &flags, request(2, 20.0), 40)
        .unwrap();
    assert_eq!((other.start, other.origin), (32, PageOrigin::Editing));
    let local = state
        .pattern_page(
            &project,
            &flags,
            PlaybackRequest {
                scope: PlaybackScope::Placement(2),
                ..request(4, 20.0)
            },
            5,
        )
        .unwrap();
    assert_eq!(local.origin, PageOrigin::Editing);
    let preview = PlaybackRequest {
        clock: PlaybackClock::Audition {
            pattern: 4,
            step: 160.0,
            length: 320,
        },
        ..request(4, 0.0)
    };
    state.observe(&project, &flags, preview).unwrap();
    assert_eq!(
        state
            .pattern_page(&project, &flags, preview, 0)
            .unwrap()
            .start,
        160
    );
    assert_eq!(
        state
            .pattern_page(&project, &flags, request(4, 20.0), 0)
            .unwrap(),
        gap
    );
    let unrelated = PlaybackRequest {
        clock: PlaybackClock::Audition {
            pattern: 1,
            step: 3.0,
            length: 5,
        },
        ..request(4, 0.0)
    };
    assert_eq!(
        state
            .pattern_page(&project, &flags, unrelated, 17)
            .unwrap()
            .origin,
        PageOrigin::Editing
    );
    assert_eq!(
        state
            .pattern_page(&project, &flags, unrelated, 17)
            .unwrap()
            .start,
        16
    );
}

#[test]
fn a_shared_definition_follows_the_preferred_active_use_without_erasing_others() {
    let mut project = Project::demo();
    let mut flags = LiveFlags::default();
    project.part_mut(3).unwrap().pattern = 4;
    let place = project.placement_mut(3).unwrap();
    place.phase = PhaseMode::Restart;
    place.offset_steps = 9;
    let state = active();
    let a = state
        .pattern_page(&project, &flags, request(4, 80.0), 0)
        .unwrap();
    let second = PlaybackRequest {
        preferred_placement: Some(3),
        ..request(4, 80.0)
    };
    let b = state.pattern_page(&project, &flags, second, 0).unwrap();
    assert_eq!((a.start, a.anchor), (80, Some(80)));
    assert_eq!((b.start, b.anchor), (16, Some(25)));
    flags
        .set_channel_muted(ChannelRef::Channel(2), true)
        .unwrap();
    assert_eq!(state.pattern_page(&project, &flags, second, 0).unwrap(), b);
    flags.set_lane_muted(2, true);
    assert_eq!(state.pattern_page(&project, &flags, second, 0).unwrap(), a);
}

#[test]
fn cached_definition_clock_remaps_periods_and_local_clock_clamps_after_shrink() {
    let mut project = Project::demo();
    let flags = LiveFlags::default();
    let mut state = active();
    state.observe(&project, &flags, request(2, 100.0)).unwrap(); // source time100, old wrapped step36
    project.pattern_mut(2).unwrap().kind = PatternKind::Values {
        unit: ValueUnit::Degrees,
        period: 7,
        mode: cadence_core::params::ValueMode::Hold,
        points: vec![ValuePoint {
            step: 0,
            value: 0.0,
        }],
    };
    let held = state
        .pattern_page(&project, &flags, request(2, 20.0), 0)
        .unwrap();
    assert_eq!((held.anchor, held.origin), (Some(2), PageOrigin::Held)); //100%7, not36%7
    let local = PlaybackRequest {
        scope: PlaybackScope::Placement(2),
        ..request(4, 164.0)
    };
    state.observe(&project, &flags, local).unwrap();
    project.placement_mut(2).unwrap().length_steps = 32;
    let held = state.pattern_page(&project, &flags, local, 0).unwrap();
    assert_eq!(
        (held.start, held.anchor, held.origin),
        (16, Some(31), PageOrigin::Held)
    );
}

#[test]
fn arrange_follow_respects_song_boundaries_and_keeps_preview_on_editing_page() {
    let page = arrange_page(
        FollowMode::Active,
        PlaybackClock::Song { step: 80.0 },
        16,
        None,
        4,
        16,
    )
    .unwrap();
    assert_eq!((page.start, page.count, page.anchor), (64, 64, Some(80)));
    let looped = arrange_page(
        FollowMode::Active,
        PlaybackClock::Song { step: 336.0 },
        16,
        None,
        4,
        16,
    )
    .unwrap();
    assert_eq!(looped, page);
    let rewind = arrange_page(
        FollowMode::Active,
        PlaybackClock::Song { step: 0.0 },
        112,
        None,
        4,
        16,
    )
    .unwrap();
    assert_eq!(rewind.start, 0);
    for mode in [FollowMode::Off, FollowMode::Suspended] {
        assert_eq!(
            arrange_page(mode, PlaybackClock::Song { step: 80.0 }, 16, None, 4, 16)
                .unwrap()
                .start,
            0
        );
    }
    let preview = arrange_page(
        FollowMode::Active,
        PlaybackClock::Audition {
            pattern: 4,
            step: 240.0,
            length: 320,
        },
        16,
        None,
        4,
        16,
    )
    .unwrap();
    assert_eq!((preview.start, preview.origin), (0, PageOrigin::Editing));
}

#[test]
fn cache_clear_and_deleted_scope_pruning_prevent_stale_viewports() {
    let mut project = Project::demo();
    let flags = LiveFlags::default();
    let mut state = active();
    let local = PlaybackRequest {
        scope: PlaybackScope::Placement(2),
        ..request(4, 164.0)
    };
    state.observe(&project, &flags, local).unwrap();
    let removed = project.lanes[0].placements.remove(1);
    state.observe(&project, &flags, request(1, 0.0)).unwrap();
    project.lanes[0].placements.push(removed);
    let gap = PlaybackRequest {
        clock: PlaybackClock::Song { step: 20.0 },
        ..local
    };
    assert_eq!(
        state.pattern_page(&project, &flags, gap, 0).unwrap().origin,
        PageOrigin::Editing
    );
    state.observe(&project, &flags, request(4, 83.0)).unwrap();
    state.clear_anchors();
    assert_eq!(state.mode(), FollowMode::Active);
    assert_eq!(
        state
            .pattern_page(&project, &flags, request(4, 20.0), 0)
            .unwrap()
            .origin,
        PageOrigin::Editing
    );
    state.reset();
    assert_eq!(state.mode(), FollowMode::Off);
}

#[test]
fn invalid_clock_and_viewport_requests_reject_without_panics() {
    let project = Project::demo();
    let flags = LiveFlags::default();
    let state = active();
    assert!(state
        .pattern_page(&project, &flags, request(4, f64::NAN), 0)
        .is_err());
    assert!(state
        .pattern_page_sized(&project, &flags, request(4, 80.0), 0, 0)
        .is_err());
    assert!(arrange_page(
        FollowMode::Active,
        PlaybackClock::Song { step: 0.0 },
        0,
        None,
        0,
        16
    )
    .is_err());
    assert!(arrange_page(
        FollowMode::Active,
        PlaybackClock::Audition {
            pattern: 1,
            step: 0.0,
            length: 0
        },
        0,
        None,
        4,
        16
    )
    .is_err());
}
