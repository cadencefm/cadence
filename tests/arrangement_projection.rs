use cadence_core::{
    arrangement_view::{
        adjacent_event, arrangement_notes, event_at, event_by_key, inspect_event,
        ArrangementRequest, EventCursor, EventKey, MAX_VISIBLE_NOTES, MAX_VISIBLE_STEPS,
    },
    material::{
        Exception, InstrumentInstance, Lane, LanePart, LiveFlags, ParameterLink, Part, Pattern,
        PatternKind, PatternValue, PhaseMode, Placement, Project, ValuePoint, ValueUnit,
    },
    mixer::{Channel, ChannelKind, ChannelRef},
    music::{ChordTone, Step, StepCondition},
    params::{InstrumentParam, RackParam, ValueMode},
    rack::RackPatch,
};
fn note(degree: i16, gate: f32) -> Step {
    Step {
        active: true,
        degree: degree + 1,
        velocity: 0.8,
        gate,
        ..Step::default()
    }
}
fn fixture() -> Project {
    let mut project = Project::empty();
    project.bars = 8;
    project.patterns = vec![
        Pattern {
            id: 1,
            name: "Motif".into(),
            kind: PatternKind::Notes {
                steps: [0, 2, 4, 7, 6].into_iter().map(|d| note(d, 3.5)).collect(),
            },
        },
        Pattern {
            id: 2,
            name: "Harmony".into(),
            kind: PatternKind::Values {
                unit: ValueUnit::Degrees,
                period: 64,
                mode: ValueMode::Hold,
                points: [(0, 0.), (24, -2.), (32, 2.), (56, -1.)]
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
        Pattern {
            id: 4,
            name: "Higher".into(),
            kind: PatternKind::Transpose {
                source: 3,
                semitones: 12,
            },
        },
    ];
    for (channel, instrument, part, lane, placement, pattern) in
        [(20, 21, 22, 23, 24, 4), (30, 31, 32, 33, 34, 1)]
    {
        project.mixer.channels.push(Channel::new(
            channel,
            ChannelKind::Audio,
            format!("Audio{channel}"),
        ));
        project.instruments.push(InstrumentInstance {
            id: instrument,
            name: format!("Synth{instrument}"),
            rack: RackPatch::default_synth(),
            channel,
        });
        project
            .parts
            .push(Part::new(part, format!("Part{part}"), pattern, instrument));
        project.lanes.push(Lane {
            id: lane,
            name: format!("Lane{lane}"),
            parts: vec![LanePart::new(part)],
            placements: vec![Placement::new(placement, part, 0, 128)],
        });
    }
    project.parts[0].transpose = -3;
    project.parts[0].velocity_scale = 0.5;
    let placement = &mut project.lanes[0].placements[0];
    placement.start_step = 16;
    placement.length_steps = 48;
    placement.phase = PhaseMode::Restart;
    placement.offset_steps = -3;
    project
}
fn request(start_step: u32, step_count: usize, lane: Option<u64>) -> ArrangementRequest {
    ArrangementRequest {
        start_step,
        step_count,
        lane,
    }
}

#[test]
fn viewport_notes_match_compiler_with_independent_clocks_and_binding_transforms() {
    let project = fixture();
    let flags = LiveFlags::default();
    let before = project.clone();
    let compiled = project.compile().unwrap();
    let projected = arrangement_notes(&project, &compiled, &flags, request(40, 16, None)).unwrap();
    assert_eq!(
        project, before,
        "viewing cannot rewrite or freeze the source graph"
    );
    let event = projected
        .notes
        .iter()
        .find(|n| n.lane == 23 && n.key.song_step == 43)
        .unwrap();
    // Restart local27 - offset3 =24; Motif[4]=6; Harmony[24]=-2;
    // scale degree4 at D4 is A4, then source+12 and part-3 give F#5.
    assert_eq!(
        (
            event.pattern_time,
            event.source_step,
            event.local_step,
            event.midi
        ),
        (24, 24, 27, 78)
    );
    assert_eq!(event.velocity, 0.4);
    for event in projected.notes {
        let sample = project
            .resolve_placement(event.key.placement, event.local_step)
            .unwrap();
        assert!(sample.eligible);
        assert_eq!(
            event.midi,
            project.pitch_midi(sample.value.degree, sample.value.accidental, 0)
        );
        assert_eq!(event.velocity, sample.value.velocity);
        assert_eq!(event.gate, sample.value.gate);
    }
    let source_event = event_by_key(
        &project,
        &compiled,
        &flags,
        EventKey {
            placement: 24,
            song_step: 16,
            sub_step: 0,
            tone: 0,
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(source_event.pattern_time, -3);
    assert_eq!(source_event.source_step, 317);
    let inspection = inspect_event(&project, &compiled, &flags, source_event.key).unwrap();
    assert_eq!(
        inspection
            .sources
            .iter()
            .find(|s| s.pattern == 1)
            .unwrap()
            .step,
        2
    );
    assert_eq!(
        inspection
            .sources
            .iter()
            .find(|s| s.pattern == 2)
            .unwrap()
            .step,
        61
    );
}

#[test]
fn incoming_chord_gates_keep_true_onset_and_independent_voice_lengths() {
    let mut project = fixture();
    let flags = LiveFlags::default();
    let PatternKind::Notes { steps } = &mut project.patterns[0].kind else {
        panic!()
    };
    for s in steps.iter_mut() {
        s.active = false;
    }
    steps[0] = note(0, 1.0);
    steps[0].chord = vec![
        ChordTone {
            degree: 3,
            accidental: 0,
            velocity: 0.6,
            gate: 7.5,
        },
        ChordTone {
            degree: 5,
            accidental: 0,
            velocity: 0.3,
            gate: 16.0,
        },
    ];
    project.lanes[1].placements[0].length_steps = 16;
    let compiled = project.compile().unwrap();
    let page = arrangement_notes(&project, &compiled, &flags, request(16, 16, Some(33))).unwrap();
    assert!(page.notes.iter().all(|n| n.key.song_step < 16));
    assert!(page.notes.iter().all(|n| n.key.tone > 0));
    let incoming = page
        .notes
        .iter()
        .find(|n| n.key.song_step == 15 && n.key.tone == 2)
        .unwrap();
    assert_eq!((incoming.gate, incoming.end_step()), (16.0, 31.0));
    assert!(page
        .notes
        .iter()
        .any(|n| n.key.song_step == 5 && n.key.tone == 2));
    assert!(
        !page.notes.iter().any(|n| n.key.song_step == 0),
        "gate ending at16 does not overlap page16"
    );
    let at = event_at(&project, &compiled, &flags, 33, 17, Some(incoming.midi))
        .unwrap()
        .unwrap();
    assert_eq!(
        at.key, incoming.key,
        "tail selects its actual owning placement and onset"
    );
    let first = arrangement_notes(&project, &compiled, &flags, request(0, 1, Some(33))).unwrap();
    assert!(
        first.notes.iter().all(|n| n.key.song_step == 0),
        "do not invent prior-loop tails"
    );
}

#[test]
fn local_override_replaces_one_occurrence_and_inspection_marks_displaced_sources() {
    let mut project = fixture();
    let flags = LiveFlags::default();
    project.patterns.push(Pattern {
        id: 5,
        name: "Pan".into(),
        kind: PatternKind::SeededValues {
            unit: ValueUnit::Normalized,
            period: 16,
            seed: 17,
            min: 0.2,
            max: 0.8,
        },
    });
    project.parts[0].parameter_links.push(ParameterLink {
        target: InstrumentParam::Rack {
            param: RackParam::Pan,
        },
        pattern: 5,
    });
    project.lanes[0].placements[0].exceptions.push(Exception {
        step: 27,
        value: Step {
            chord: vec![ChordTone {
                degree: 8,
                accidental: 0,
                velocity: 0.9,
                gate: 9.0,
            }],
            ..note(0, 12.0)
        },
    });
    let compiled = project.compile().unwrap();
    let key = EventKey {
        placement: 24,
        song_step: 43,
        sub_step: 0,
        tone: 1,
    };
    let inspection = inspect_event(&project, &compiled, &flags, key).unwrap();
    assert!(inspection.note.local_exception);
    assert_eq!(
        (
            inspection.note.midi,
            inspection.note.velocity,
            inspection.note.gate
        ),
        (71, 0.45, 9.0)
    );
    assert!(inspection
        .sources
        .iter()
        .filter(|s| s.pattern != 5)
        .all(|s| s.displaced));
    let pan = inspection.sources.iter().find(|s| s.pattern == 5).unwrap();
    assert!(!pan.displaced);
    assert!(pan.role.starts_with("Per-note"));
    assert_eq!((pan.time, pan.step), (24, 8));
    assert_eq!(pan.value, project.evaluate_pattern(5, 24).unwrap());
    assert!(
        !event_by_key(
            &project,
            &compiled,
            &flags,
            EventKey {
                placement: 24,
                song_step: 42,
                sub_step: 0,
                tone: 0
            }
        )
        .unwrap()
        .unwrap()
        .local_exception
    );
}

#[test]
fn conditions_masks_and_lane_activation_match_the_finite_scheduled_song() {
    let mut project = fixture();
    let mut flags = LiveFlags::default();
    let PatternKind::Notes { steps } = &mut project.patterns[0].kind else {
        panic!()
    };
    steps[0].condition = StepCondition { turn: 2, every: 2 };
    project.patterns.push(Pattern {
        id: 5,
        name: "Mask".into(),
        kind: PatternKind::Rhythm {
            steps: vec![true, false],
        },
    });
    project.patterns.push(Pattern {
        id: 6,
        name: "Gated".into(),
        kind: PatternKind::Mask {
            notes: 1,
            rhythm: 5,
        },
    });
    project.parts[1].pattern = 6;
    let compiled = project.compile().unwrap();
    let events = arrangement_notes(&project, &compiled, &flags, request(0, 16, Some(33))).unwrap();
    assert!(!events.notes.iter().any(|n| n.key.song_step == 0));
    assert!(events.notes.iter().all(|n| n.key.song_step % 2 == 0));
    assert!(event_by_key(
        &project,
        &compiled,
        &flags,
        EventKey {
            placement: 34,
            song_step: 5,
            sub_step: 0,
            tone: 0
        }
    )
    .unwrap()
    .is_none());
    flags
        .set_channel_muted(ChannelRef::Channel(project.mixer.channels[1].id), true)
        .unwrap();
    let muted = project.compile_with_flags(&flags).unwrap();
    let muted_events =
        arrangement_notes(&project, &muted, &flags, request(0, 16, Some(33))).unwrap();
    assert_eq!(events.notes.len(), muted_events.notes.len());
    assert!(muted_events.notes.iter().all(|n| n.channel_muted));
    for (before, after) in events.notes.iter().zip(&muted_events.notes) {
        assert_eq!(before.key, after.key);
    }
    flags.set_lane_muted(project.lanes[1].id, true);
    let compiled = project.compile_with_flags(&flags).unwrap();
    let projection =
        arrangement_notes(&project, &compiled, &flags, request(0, 16, Some(33))).unwrap();
    assert!(projection.notes.is_empty());
    assert!(!projection.lanes[0].enabled);
    flags.set_lane_muted(project.lanes[1].id, false);
    flags.set_lane_solo(project.lanes[0].id, true);
    let projection = arrangement_notes(
        &project,
        &project.compile_with_flags(&flags).unwrap(),
        &flags,
        request(0, 16, Some(33)),
    )
    .unwrap();
    assert!(projection.notes.is_empty());
    assert!(!projection.lanes[0].enabled);
}

#[test]
fn event_navigation_reaches_individual_chord_voices_across_silent_gaps() {
    let mut project = fixture();
    let flags = LiveFlags::default();
    let PatternKind::Notes { steps } = &mut project.patterns[0].kind else {
        panic!()
    };
    for s in steps.iter_mut() {
        s.active = false;
    }
    steps[0] = note(0, 1.0);
    steps[0].chord.push(ChordTone {
        degree: 5,
        accidental: 0,
        velocity: 0.7,
        gate: 1.0,
    });
    let compiled = project.compile().unwrap();
    let cursor = |song_step, tone| EventCursor {
        lane: 33,
        song_step,
        sub_step: None,
        tone,
    };
    let first = adjacent_event(&project, &compiled, &flags, cursor(0, None), true)
        .unwrap()
        .unwrap();
    assert_eq!((first.key.song_step, first.key.tone), (0, 0));
    let chord = adjacent_event(&project, &compiled, &flags, cursor(0, Some(0)), true)
        .unwrap()
        .unwrap();
    assert_eq!((chord.key.song_step, chord.key.tone), (0, 1));
    let next = adjacent_event(&project, &compiled, &flags, cursor(0, Some(1)), true)
        .unwrap()
        .unwrap();
    assert_eq!((next.key.song_step, next.key.tone), (5, 0));
    let previous = adjacent_event(&project, &compiled, &flags, cursor(5, Some(0)), false)
        .unwrap()
        .unwrap();
    assert_eq!(previous.key, chord.key);
    let prior = adjacent_event(&project, &compiled, &flags, cursor(4, None), false)
        .unwrap()
        .unwrap();
    assert_eq!(prior.key, chord.key);
    assert!(
        adjacent_event(&project, &compiled, &flags, cursor(0, Some(0)), false)
            .unwrap()
            .is_none()
    );
    assert!(event_by_key(
        &project,
        &compiled,
        &flags,
        EventKey {
            tone: 2,
            ..chord.key
        }
    )
    .unwrap()
    .is_none());
    assert!(event_at(&project, &compiled, &flags, 33, 3, None)
        .unwrap()
        .is_none());
}

#[test]
fn shared_source_dags_are_traced_as_bounded_references() {
    let mut project = fixture();
    let flags = LiveFlags::default();
    project.patterns.push(Pattern {
        id: 5,
        name: "Shared mask".into(),
        kind: PatternKind::Mask {
            notes: 1,
            rhythm: 1,
        },
    });
    project.parts[1].pattern = 5;
    let compiled = project.compile().unwrap();
    let inspection = inspect_event(
        &project,
        &compiled,
        &flags,
        EventKey {
            placement: 34,
            song_step: 5,
            sub_step: 0,
            tone: 0,
        },
    )
    .unwrap();
    assert_eq!(inspection.sources.len(), 3);
    assert_eq!(
        inspection
            .sources
            .iter()
            .filter(|s| s.reference_only)
            .count(),
        1
    );
    assert_eq!(inspection.sources[1].role, "Notes");
    assert_eq!(inspection.sources[2].role, "Mask");
    assert_eq!(inspection.sources[1].value, inspection.sources[2].value);
    assert!(matches!(
        inspection.sources[0].value,
        PatternValue::Notes(_)
    ));
}

#[test]
fn dense_supported_view_and_invalid_requests_are_bounded_without_silent_truncation() {
    let mut project = fixture();
    let flags = LiveFlags::default();
    project.bars = 32;
    project.lanes.clear();
    project.instruments.clear();
    project.parts.clear();
    project.mixer.channels.clear();
    let mut dense = note(0, 16.0);
    dense.chord = (1..=5)
        .map(|i| ChordTone {
            degree: i + 1,
            accidental: 0,
            velocity: 0.5,
            gate: 16.0,
        })
        .collect();
    project.patterns = vec![Pattern {
        id: 1,
        name: "Dense".into(),
        kind: PatternKind::Notes { steps: vec![dense] },
    }];
    for i in 0..16 {
        let id = 10 + i * 5;
        project
            .mixer
            .channels
            .push(Channel::new(id, ChannelKind::Audio, format!("Audio{i}")));
        project.instruments.push(InstrumentInstance {
            id: id + 1,
            name: format!("Synth{i}"),
            rack: RackPatch::default_synth(),
            channel: id,
        });
        project
            .parts
            .push(Part::new(id + 2, format!("Part{i}"), 1, id + 1));
        project.lanes.push(Lane {
            id: id + 3,
            name: format!("Lane{i}"),
            parts: vec![LanePart::new(id + 2)],
            placements: vec![Placement::new(id + 4, id + 2, 0, 512)],
        });
    }
    let compiled = project.compile().unwrap();
    let projection = arrangement_notes(
        &project,
        &compiled,
        &flags,
        request(128, MAX_VISIBLE_STEPS, None),
    )
    .unwrap();
    assert_eq!(projection.step_count, 512 - 128);
    assert_eq!(
        projection.notes.len(),
        16 * (projection.step_count + 15) * 6
    );
    assert!(projection.notes.len() < MAX_VISIBLE_NOTES);
    const {
        assert!(MAX_VISIBLE_STEPS >= 2048);
        assert!(MAX_VISIBLE_NOTES >= 250_000);
    }
    assert!(arrangement_notes(&project, &compiled, &flags, request(0, 0, None)).is_err());
    assert!(arrangement_notes(
        &project,
        &compiled,
        &flags,
        request(0, MAX_VISIBLE_STEPS + 1, None)
    )
    .is_err());
    assert!(arrangement_notes(&project, &compiled, &flags, request(512, 1, None)).is_err());
    assert!(arrangement_notes(&project, &compiled, &flags, request(0, 1, Some(999))).is_err());
    let end = arrangement_notes(&project, &compiled, &flags, request(510, 16, None)).unwrap();
    assert_eq!(end.step_count, 2);
    let mut stale = project.clone();
    stale.bars = 31;
    assert!(arrangement_notes(&stale, &compiled, &flags, request(0, 1, None)).is_err());
    let mut stale = project.clone();
    stale.lanes[0].placements[0].offset_steps = 1;
    assert!(event_by_key(
        &stale,
        &compiled,
        &flags,
        EventKey {
            placement: 14,
            song_step: 0,
            sub_step: 0,
            tone: 0
        }
    )
    .is_err());
}

#[test]
fn sub_step_offsets_and_wide_zoom_windows_preserve_every_note() {
    let mut project = fixture();
    project.bars = 64; // 1024 steps (full Afterglow horizon)
    project.patterns.push(Pattern {
        id: 10,
        name: "Mini Triplets".into(),
        kind: PatternKind::Mini {
            code: "[c4 e4 g4] d4 [f4 a4] ~".into(),
            steps: 4,
        },
    });
    project.patterns.push(Pattern {
        id: 11,
        name: "Ratchet 3x".into(),
        kind: PatternKind::Ratchet {
            source: 1,
            count: 3,
        },
    });
    project.patterns.push(Pattern {
        id: 12,
        name: "Swung Motif".into(),
        kind: PatternKind::Swing {
            source: 1,
            amount: 0.35,
        },
    });
    project.parts[0].pattern = 10;
    project.parts[0].transpose = 0;
    project.parts[0].velocity_scale = 1.0;
    project.lanes[0].placements = vec![Placement::new(24, 22, 0, 1024)];
    project.parts[1].pattern = 11;
    project.lanes[1].placements = vec![Placement::new(34, 32, 0, 1024)];

    let flags = LiveFlags::default();
    let compiled = project.compile().unwrap();
    // Request the full 64-bar (1024-step) window in one shot without error or truncation.
    let wide = arrangement_notes(&project, &compiled, &flags, request(0, 1024, None)).unwrap();
    assert_eq!(wide.step_count, 1024);

    // Step 0 of Mini `[c4 e4 g4]` must produce 3 distinct pitches at 3 distinct offsets!
    let step0_mini: Vec<_> = wide
        .notes
        .iter()
        .filter(|n| n.lane == 23 && n.key.song_step == 0)
        .collect();
    assert_eq!(step0_mini.len(), 3, "expected 3 sub-step mini notes");
    assert_eq!(
        step0_mini.iter().map(|n| n.midi).collect::<Vec<_>>(),
        vec![60, 64, 67]
    );
    assert!(step0_mini[0].offset < 0.01);
    assert!((step0_mini[1].offset - 1.0 / 3.0).abs() < 0.02);
    assert!((step0_mini[2].offset - 2.0 / 3.0).abs() < 0.02);
    assert!(step0_mini[2].start_step() > step0_mini[1].start_step());
    assert!(step0_mini[1].start_step() > step0_mini[0].start_step());

    // Step 0 of Ratchet 3x must produce 3 distinct offsets on lane 33!
    let step0_ratchet: Vec<_> = wide
        .notes
        .iter()
        .filter(|n| n.lane == 33 && n.key.song_step == 0)
        .collect();
    assert_eq!(step0_ratchet.len(), 3, "expected 3 sub-step ratchet hits");
    assert!(step0_ratchet[0].offset < 0.01);
    assert!((step0_ratchet[1].offset - 1.0 / 3.0).abs() < 0.02);
    assert!((step0_ratchet[2].offset - 2.0 / 3.0).abs() < 0.02);

    // Verify Swing preserves fractional offset on odd 16th steps.
    project.parts[1].pattern = 12;
    let swung_compiled = project.compile().unwrap();
    let swung =
        arrangement_notes(&project, &swung_compiled, &flags, request(0, 8, Some(33))).unwrap();
    let odd_hit = swung.notes.iter().find(|n| n.key.song_step == 1).unwrap();
    assert!(
        (odd_hit.offset - 0.35).abs() < 0.01,
        "expected swung odd step offset ~0.35, got {}",
        odd_hit.offset
    );
}

#[test]
fn substep_event_keys_navigation_and_temporal_source_inspection() {
    let mut project = fixture();
    let flags = LiveFlags::default();
    project.patterns.push(Pattern {
        id: 10,
        name: "Mini Triplets".into(),
        kind: PatternKind::Mini {
            code: "[c4 e4 g4] d4 ~ ~".into(),
            steps: 4,
        },
    });
    project.patterns.push(Pattern {
        id: 11,
        name: "Shifted Motif".into(),
        kind: PatternKind::Shift {
            source: 1,
            steps: 2,
        },
    });
    project.patterns.push(Pattern {
        id: 12,
        name: "Reversed Motif".into(),
        kind: PatternKind::Reverse { source: 1 },
    });
    project.parts[0].pattern = 10;
    project.parts[0].transpose = 0;
    project.parts[0].velocity_scale = 1.0;
    project.lanes[0].placements = vec![Placement::new(24, 22, 0, 16)];
    project.parts[1].pattern = 11;
    project.lanes[1].placements = vec![Placement::new(34, 32, 0, 16)];

    let compiled = project.compile().unwrap();
    let page = arrangement_notes(&project, &compiled, &flags, request(0, 4, Some(23))).unwrap();
    let step0: Vec<_> = page.notes.iter().filter(|n| n.key.song_step == 0).collect();
    assert_eq!(step0.len(), 3);
    assert_eq!(
        step0.iter().map(|n| n.key.sub_step).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(
        step0.iter().map(|n| n.midi).collect::<Vec<_>>(),
        vec![60, 64, 67]
    );

    // event_by_key disambiguates every sub-step on the same song step and rejects out-of-range sub_step.
    for (sub_step, expected_midi) in [(0, 60), (1, 64), (2, 67)] {
        let resolved = event_by_key(
            &project,
            &compiled,
            &flags,
            EventKey {
                placement: 24,
                song_step: 0,
                sub_step,
                tone: 0,
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(resolved.key.sub_step, sub_step);
        assert_eq!(resolved.midi, expected_midi);
    }
    assert!(event_by_key(
        &project,
        &compiled,
        &flags,
        EventKey {
            placement: 24,
            song_step: 0,
            sub_step: 3,
            tone: 0,
        },
    )
    .unwrap()
    .is_none());

    // adjacent_event navigates forward and backward across every sub-step on step 0 and into step 1.
    let cursor = |song_step, sub_step, tone| EventCursor {
        lane: 23,
        song_step,
        sub_step: Some(sub_step),
        tone: Some(tone),
    };
    let n1 = adjacent_event(&project, &compiled, &flags, cursor(0, 0, 0), true)
        .unwrap()
        .unwrap();
    assert_eq!((n1.key.song_step, n1.key.sub_step, n1.midi), (0, 1, 64));
    let n2 = adjacent_event(&project, &compiled, &flags, cursor(0, 1, 0), true)
        .unwrap()
        .unwrap();
    assert_eq!((n2.key.song_step, n2.key.sub_step, n2.midi), (0, 2, 67));
    let n3 = adjacent_event(&project, &compiled, &flags, cursor(0, 2, 0), true)
        .unwrap()
        .unwrap();
    assert_eq!((n3.key.song_step, n3.key.sub_step, n3.midi), (1, 0, 62));

    let p2 = adjacent_event(&project, &compiled, &flags, cursor(1, 0, 0), false)
        .unwrap()
        .unwrap();
    assert_eq!((p2.key.song_step, p2.key.sub_step, p2.midi), (0, 2, 67));
    let p1 = adjacent_event(&project, &compiled, &flags, cursor(0, 2, 0), false)
        .unwrap()
        .unwrap();
    assert_eq!((p1.key.song_step, p1.key.sub_step, p1.midi), (0, 1, 64));
    let p0 = adjacent_event(&project, &compiled, &flags, cursor(0, 1, 0), false)
        .unwrap()
        .unwrap();
    assert_eq!((p0.key.song_step, p0.key.sub_step, p0.midi), (0, 0, 60));

    // inspect_event traces exact transformed child time and step through Shift (+2).
    let shift_insp = inspect_event(
        &project,
        &compiled,
        &flags,
        EventKey {
            placement: 34,
            song_step: 1,
            sub_step: 0,
            tone: 0,
        },
    )
    .unwrap();
    let shift_src = shift_insp.sources.iter().find(|s| s.pattern == 1).unwrap();
    assert_eq!(
        (shift_src.time, shift_src.step),
        (-1, 4),
        "Shift(steps=2) at t=1 must sample source Motif (period 5) at t=-1 (step 4)"
    );

    // inspect_event traces exact transformed child time and step through Reverse (period 5).
    project.parts[1].pattern = 12;
    let rev_compiled = project.compile().unwrap();
    let rev_insp = inspect_event(
        &project,
        &rev_compiled,
        &flags,
        EventKey {
            placement: 34,
            song_step: 1,
            sub_step: 0,
            tone: 0,
        },
    )
    .unwrap();
    let rev_src = rev_insp.sources.iter().find(|s| s.pattern == 1).unwrap();
    assert_eq!(
        (rev_src.time, rev_src.step),
        (3, 3),
        "Reverse on Motif (period 5) at t=1 must sample source Motif at step 3 (P - 1 - 1)"
    );
}
