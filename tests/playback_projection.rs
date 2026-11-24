use cadence_core::{
    material::{
        Exception, LiveFlags, Modulation, ModulationTarget, ParameterLink, Pattern, PatternKind,
        PhaseMode, Project,
    },
    mixer::ChannelRef,
    music::{Step, StepCondition},
    params::RackParam,
    playback::{pattern_playback, PlaybackClock, PlaybackRequest, PlaybackScope, PlaybackUse},
};

fn request(pattern: u64, step: f64) -> PlaybackRequest {
    PlaybackRequest {
        pattern,
        scope: PlaybackScope::Definition,
        preferred_placement: None,
        clock: PlaybackClock::Song { step },
    }
}

#[test]
fn five_step_and_sixty_four_step_sources_keep_independent_song_coordinates() {
    let project = Project::demo();
    let flags = LiveFlags::default();
    let before = project.clone();
    let motif = pattern_playback(&project, &flags, request(1, 66.25)).unwrap();
    assert_eq!(motif.period, 5);
    assert_eq!(motif.markers.len(), 1);
    assert_eq!(motif.markers[0].position, 1.25);
    assert_eq!(motif.markers[0].source_time, 66.25);
    assert_eq!(motif.markers[0].cycle, 13);
    assert_eq!(motif.markers[0].placement, Some(2));
    let harmony = pattern_playback(&project, &flags, request(2, 66.25)).unwrap();
    assert_eq!(harmony.period, 64);
    assert_eq!(harmony.markers.len(), 2);
    assert!(harmony.markers.iter().all(|m| m.position == 2.25));
    let derived = pattern_playback(&project, &flags, request(4, 66.25)).unwrap();
    assert_eq!(derived.period, 320);
    assert_eq!(derived.markers[0].position, 66.25);
    assert_eq!(project, before, "observing playback must not edit material");
}

#[test]
fn nested_sources_observe_restart_and_negative_offset_without_song_modulo_guessing() {
    let mut project = Project::demo();
    let flags = LiveFlags::default();
    project.patterns.push(Pattern {
        id: 9,
        name: "Higher".into(),
        kind: PatternKind::Transpose {
            source: 4,
            semitones: 12,
        },
    });
    project.part_mut(2).unwrap().pattern = 9;
    let place = project.placement_mut(2).unwrap();
    place.start_step = 80;
    place.length_steps = 176;
    place.phase = PhaseMode::Restart;
    place.offset_steps = -3;
    project.validate().unwrap();
    let motif = pattern_playback(&project, &flags, request(1, 80.5)).unwrap();
    assert_eq!(motif.markers[0].source_time, -2.5);
    assert_eq!(motif.markers[0].position, 2.5);
    assert_eq!(motif.markers[0].cycle, -1);
    let harmony = pattern_playback(
        &project,
        &flags,
        PlaybackRequest {
            preferred_placement: Some(2),
            ..request(2, 80.5)
        },
    )
    .unwrap();
    assert_eq!(harmony.markers[0].position, 61.5);
    assert!(harmony.markers[0].preferred);
    let derived = pattern_playback(&project, &flags, request(4, 80.5)).unwrap();
    assert_eq!(derived.markers[0].position, 317.5);
}

#[test]
fn shared_source_keeps_multiple_use_clocks_and_prioritizes_selected_active_placement() {
    let mut project = Project::demo();
    let mut flags = LiveFlags::default();
    let bass = project.placement_mut(3).unwrap();
    bass.phase = PhaseMode::Restart;
    bass.offset_steps = 7;
    let q = PlaybackRequest {
        preferred_placement: Some(3),
        ..request(2, 66.25)
    };
    let result = pattern_playback(&project, &flags, q).unwrap();
    assert_eq!(result.markers.len(), 2);
    assert_eq!(result.markers[0].placement, Some(3));
    assert_eq!(result.markers[0].position, 9.25);
    assert_eq!(result.markers[1].placement, Some(2));
    assert_eq!(result.markers[1].position, 2.25);
    flags
        .set_channel_muted(ChannelRef::Channel(4), true)
        .unwrap();
    assert_eq!(
        pattern_playback(&project, &flags, q).unwrap(),
        result,
        "audio mute preserves musical clocks"
    );
    flags.set_lane_muted(2, true);
    let result = pattern_playback(&project, &flags, q).unwrap();
    assert_eq!(result.markers.len(), 1);
    assert_eq!(result.markers[0].placement, Some(2));
    assert!(!result.markers[0].preferred);
    flags.set_lane_solo(3, true);
    assert!(pattern_playback(&project, &flags, q)
        .unwrap()
        .markers
        .is_empty());
}

#[test]
fn placement_editor_uses_occurrence_time_and_identifies_local_replacements() {
    let mut project = Project::demo();
    let flags = LiveFlags::default();
    let place = project.placement_mut(2).unwrap();
    place.phase = PhaseMode::Restart;
    place.offset_steps = -11;
    place.exceptions.push(Exception {
        step: 2,
        value: Step::default(),
    });
    let q = PlaybackRequest {
        scope: PlaybackScope::Placement(2),
        ..request(4, 66.75)
    };
    let result = pattern_playback(&project, &flags, q).unwrap();
    assert_eq!(result.period, 192);
    assert_eq!(result.markers.len(), 1);
    assert_eq!(result.markers[0].position, 2.75);
    assert!(result.markers[0].local_exception);
    assert!(result.markers[0].preferred);
    let source = pattern_playback(&project, &flags, request(4, 66.75)).unwrap();
    assert_eq!(source.markers[0].position, 311.75);
    assert!(source.markers[0].local_exception);
    assert!(pattern_playback(
        &project,
        &flags,
        PlaybackRequest {
            scope: PlaybackScope::Placement(2),
            ..request(4, 40.0)
        }
    )
    .unwrap()
    .markers
    .is_empty());
}

#[test]
fn conditional_and_rest_positions_still_have_clocks_but_literal_editor_stays_five_steps() {
    let mut project = Project::demo();
    let flags = LiveFlags::default();
    let PatternKind::Notes { steps } = &mut project.pattern_mut(1).unwrap().kind else {
        panic!()
    };
    for step in steps {
        step.condition = StepCondition { turn: 3, every: 3 };
    }
    assert_eq!(project.pattern_period(1).unwrap(), 15);
    assert!(!project.resolve_placement(1, 6).unwrap().eligible);
    let result = pattern_playback(&project, &flags, request(1, 6.0)).unwrap();
    assert_eq!(result.period, 5);
    assert_eq!(result.markers[0].step, 1);
    assert_eq!(result.markers[0].cycle, 1);
    let PatternKind::Notes { steps } = &mut project.pattern_mut(1).unwrap().kind else {
        panic!()
    };
    for step in steps {
        step.active = false;
    }
    assert_eq!(
        pattern_playback(&project, &flags, request(1, 6.0)).unwrap(),
        result
    );
}

#[test]
fn isolated_preview_maps_only_its_sources_and_never_fabricates_song_uses() {
    let project = Project::demo();
    let flags = LiveFlags::default();
    let clock = PlaybackClock::Audition {
        pattern: 4,
        step: 36.5,
        length: 37,
    };
    let result = pattern_playback(
        &project,
        &flags,
        PlaybackRequest {
            clock,
            ..request(1, 0.0)
        },
    )
    .unwrap();
    assert_eq!(result.markers.len(), 1);
    assert_eq!(result.markers[0].kind, PlaybackUse::Audition);
    assert_eq!(result.markers[0].position, 1.5);
    assert_eq!(result.markers[0].placement, None);
    assert_eq!(result.markers[0].part, None);
    assert!(pattern_playback(
        &project,
        &flags,
        PlaybackRequest {
            clock,
            ..request(7, 0.0)
        }
    )
    .unwrap()
    .markers
    .is_empty());
    assert!(pattern_playback(
        &project,
        &flags,
        PlaybackRequest {
            scope: PlaybackScope::Placement(2),
            clock,
            ..request(4, 0.0)
        }
    )
    .unwrap()
    .markers
    .is_empty());
    let result = pattern_playback(
        &project,
        &flags,
        PlaybackRequest {
            clock: PlaybackClock::Audition {
                pattern: 4,
                step: 38.25,
                length: 37,
            },
            ..request(2, 0.0)
        },
    )
    .unwrap();
    assert_eq!(
        result.markers[0].position, 1.25,
        "preview length wraps before source clocks"
    );
}

#[test]
fn per_note_links_follow_placement_time_while_global_sources_follow_song_time() {
    let mut project = Project::demo();
    let mut flags = LiveFlags::default();
    project.placement_mut(2).unwrap().phase = PhaseMode::Restart;
    project.placement_mut(2).unwrap().offset_steps = 3;
    project
        .part_mut(3)
        .unwrap()
        .parameter_links
        .push(ParameterLink {
            target: cadence_core::params::InstrumentParam::Rack {
                param: RackParam::Pan,
            },
            pattern: 8,
        });
    project.modulations.push(Modulation {
        id: 2,
        name: "Global pan".into(),
        target: ModulationTarget::Instrument {
            instrument: 1,
            parameter: cadence_core::params::InstrumentParam::Rack {
                param: RackParam::Pan,
            },
        },
        pattern: 8,
    });
    project.validate().unwrap();
    let result = pattern_playback(&project, &flags, request(8, 66.25)).unwrap();
    assert_eq!(result.markers.len(), 3);
    let lead = result
        .markers
        .iter()
        .find(|m| m.placement == Some(2))
        .unwrap();
    assert_eq!(lead.kind, PlaybackUse::ParameterLink);
    assert_eq!(lead.position, 5.25);
    let global = result
        .markers
        .iter()
        .find(|m| m.kind == PlaybackUse::Modulation)
        .unwrap();
    assert_eq!(global.position, 2.25);
    assert_eq!(global.modulation, Some(2));
    for lane in &project.lanes {
        flags.set_lane_muted(lane.id, true);
    }
    let result = pattern_playback(&project, &flags, request(8, 66.25)).unwrap();
    assert_eq!(
        result.markers.len(),
        1,
        "song-global control has no lane activation dependency"
    );
    assert_eq!(result.markers[0].kind, PlaybackUse::Modulation);
    let energy = pattern_playback(&project, &flags, request(7, 66.25)).unwrap();
    assert_eq!(energy.markers[0].position, 66.25);
    assert_eq!(
        project.modulations[0].target,
        ModulationTarget::ChannelGain {
            channel: ChannelRef::Channel(4)
        }
    );
}

#[test]
fn placement_boundaries_are_half_open_and_song_loop_restarts_the_source_clock() {
    let project = Project::demo();
    let flags = LiveFlags::default();
    let before = pattern_playback(&project, &flags, request(1, 63.75)).unwrap();
    assert_eq!(before.markers[0].placement, Some(1));
    let after = pattern_playback(&project, &flags, request(1, 64.0)).unwrap();
    assert_eq!(after.markers.len(), 1);
    assert_eq!(after.markers[0].placement, Some(2));
    let wrapped = pattern_playback(&project, &flags, request(1, 256.0)).unwrap();
    assert_eq!(wrapped.markers[0].placement, Some(1));
    assert_eq!(wrapped.markers[0].position, 0.0);
}

#[test]
fn malformed_query_coordinates_and_scope_fail_without_guessing() {
    let project = Project::demo();
    let flags = LiveFlags::default();
    for time in [f64::NAN, f64::INFINITY, -1.0] {
        assert!(pattern_playback(&project, &flags, request(1, time)).is_err());
    }
    assert!(pattern_playback(&project, &flags, request(999, 0.0)).is_err());
    assert!(pattern_playback(
        &project,
        &flags,
        PlaybackRequest {
            scope: PlaybackScope::Placement(2),
            ..request(1, 80.0)
        }
    )
    .is_err());
    assert!(pattern_playback(
        &project,
        &flags,
        PlaybackRequest {
            clock: PlaybackClock::Audition {
                pattern: 4,
                step: 0.0,
                length: 0
            },
            ..request(1, 0.0)
        }
    )
    .is_err());
}

#[test]
fn past_end_never_wraps_arrangement_while_active_override_stays_visible() {
    use cadence_core::material::{LanePart, Override, PhaseMode};
    use cadence_core::music::EndRule;
    use cadence_core::playback::{pattern_playback_live, LiveProjection};
    use cadence_core::timeline::Segment;

    let mut project = Project::demo();
    project.end = EndRule::Continue;
    let flags = LiveFlags::default();
    let seg = Segment::new(0, 0, None, EndRule::Continue);

    // Past a continuing end with no overrides: zero wrapped song markers!
    let past_only = pattern_playback_live(
        &project,
        &flags,
        LiveProjection {
            transport_step: 300,
            segment: Some(seg),
            active: &[],
            audition: None,
        },
        PlaybackRequest {
            clock: PlaybackClock::PastEnd,
            ..request(1, 0.0)
        },
    )
    .unwrap();
    assert!(
        past_only.markers.is_empty(),
        "arrangement must never wrap past a continuing/stopping end"
    );

    // With an active override on lane 1 (part 2 uses pattern 4 which consumes pattern 1),
    // the override marker stays visible past the continuing end!
    let active = [Override {
        lane: 1,
        part: LanePart {
            part: 2,
            phase: PhaseMode::Song,
            offset_steps: 0,
        },
        anchor: 0,
    }];
    let with_override = pattern_playback_live(
        &project,
        &flags,
        LiveProjection {
            transport_step: 300,
            segment: Some(seg),
            active: &active,
            audition: None,
        },
        PlaybackRequest {
            clock: PlaybackClock::PastEnd,
            ..request(1, 0.0)
        },
    )
    .unwrap();
    assert_eq!(with_override.markers.len(), 1);
    assert_eq!(with_override.markers[0].kind, PlaybackUse::Override);
    assert_eq!(with_override.markers[0].lane, Some(1));
    assert_eq!(with_override.markers[0].source_time, 300.0);
}
