use cadence_core::{
    material::{
        Edit, InstrumentInstance, Lane, LiveFlags, Modulation, ModulationTarget, Part, Pattern,
        PatternKind, PatternType, PatternValue, PhaseMode, Placement, Project, ValuePoint,
        ValueUnit,
    },
    mixer::{Channel, ChannelKind, ChannelRef, MixerParamTarget},
    music::Step,
    params::InstrumentParam,
    rack::{ModuleParam, RackPatch},
};

fn note(degree: i16) -> Step {
    Step {
        active: true,
        degree,
        velocity: 0.8,
        gate: 0.75,
        ..Step::default()
    }
}

#[test]
fn empty_workspace_has_a_real_horizon_and_no_phony_instruments() {
    let mut p = Project::empty();
    p.bars = 256;
    let prepared = p.compile().unwrap();
    assert!(prepared.instruments.is_empty());
    assert_eq!(prepared.bars(), 256);
    assert!(prepared.instruments.is_empty() && prepared.required_instrument_ids.is_empty());
    let reopened: Project = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
    assert_eq!(reopened, p);
    assert_eq!(reopened.compile().unwrap(), prepared);
}

#[test]
fn independent_clocks_and_explicit_placement_phase_survive_moves() {
    let mut p = Project::demo();
    assert_eq!(p.pattern_period(4).unwrap(), 320);
    assert_eq!(p.pattern_type(4).unwrap(), PatternType::Notes);
    let compiled = p.compile().unwrap();
    let lead = compiled.instrument_index(1).unwrap();
    for at in 64..256 {
        let PatternValue::Notes(source) = p.evaluate_pattern(4, at as i64).unwrap() else {
            panic!()
        };
        let actual = compiled.event_for(lead, at).unwrap();
        assert_eq!(
            actual.midi,
            p.pitch_midi(source.degree, source.accidental, 0)
        );
        assert_eq!(actual.part_id, Some(2));
    }
    let song = p.resolve_placement(2, 0).unwrap().value;
    p.placement_mut(2).unwrap().phase = PhaseMode::Restart;
    let restart = p.resolve_placement(2, 0).unwrap().value;
    assert_ne!(song.degree, restart.degree);
    p.placement_mut(2).unwrap().start_step = 80;
    p.placement_mut(2).unwrap().length_steps = 176;
    assert_eq!(p.resolve_placement(2, 0).unwrap().value, restart);
    p.placement_mut(2).unwrap().offset_steps = -1;
    let shifted = p.resolve_placement(2, 0).unwrap();
    assert_eq!(shifted.pattern_time, -1);
    assert_eq!(shifted.value, p.preview_note_steps(4, -1, 1).unwrap()[0]);
}

#[test]
fn shared_harmony_updates_two_instruments_without_rewriting_their_bindings() {
    let mut p = Project::demo();
    p.apply(Edit::SetException {
        placement: 2,
        step: 191,
        value: Some(note(1)),
    })
    .unwrap();
    let before = p.compile().unwrap();
    let parts = p.parts.clone();
    let instruments = p.instruments.clone();
    let values = if let PatternKind::Values { points, .. } = &p.pattern(2).unwrap().kind {
        points.clone()
    } else {
        panic!()
    };
    let edits: Vec<_> = values
        .into_iter()
        .map(|v| Edit::SetValue {
            pattern: 2,
            step: v.step,
            value: Some(v.value + 1.0),
        })
        .collect();
    let after = p.apply_transaction(&edits).unwrap();
    assert_eq!(p.parts, parts);
    assert_eq!(p.instruments, instruments);
    for id in [1, 2] {
        let index = after.instrument_index(id).unwrap();
        assert_ne!(
            after.event_for(index, 66).unwrap().midi,
            before.event_for(index, 66).unwrap().midi
        );
    }
    let lead = after.instrument_index(1).unwrap();
    assert_eq!(after.event_for(lead, 255).unwrap().midi, 62);
    assert_eq!(before.event_for(lead, 255).unwrap().midi, 62);
    assert!(matches!(
        p.pattern(4).unwrap().kind,
        PatternKind::AddDegrees {
            notes: 1,
            values: 2
        }
    ));
}

#[test]
fn local_exception_addresses_one_occurrence_of_a_short_source() {
    let mut p = Project::demo();
    let compiled = p
        .apply(Edit::SetException {
            placement: 1,
            step: 5,
            value: Some(Step::default()),
        })
        .unwrap();
    let track = compiled.instrument_index(1).unwrap();
    assert!(compiled.event_for(track, 0).is_some());
    assert!(compiled.event_for(track, 5).is_none());
    assert!(compiled.event_for(track, 10).is_some());
    assert!(p.evaluate_pattern(1, 5).unwrap().active());
    let restored = p
        .apply(Edit::SetException {
            placement: 1,
            step: 5,
            value: None,
        })
        .unwrap();
    assert!(restored.event_for(track, 5).is_some());
}

#[test]
fn fork_retains_references_and_freeze_captures_an_independent_range() {
    let mut p = Project::demo();
    p.apply_transaction(&[
        Edit::ForkPattern {
            source: 4,
            id: 9,
            name: "Alternate operation".into(),
        },
        Edit::FreezePattern {
            source: 4,
            id: 10,
            name: "Frozen passage".into(),
            start: 64,
            length: 64,
        },
    ])
    .unwrap();
    assert_eq!(p.dependencies(9).unwrap(), vec![1, 2]);
    assert!(p.dependencies(10).unwrap().is_empty());
    let frozen = p.preview_note_steps(10, 0, 64).unwrap();
    assert_eq!(frozen, p.preview_note_steps(4, 64, 64).unwrap());
    p.apply(Edit::SetValue {
        pattern: 2,
        step: 0,
        value: Some(3.0),
    })
    .unwrap();
    assert_ne!(
        p.preview_note_steps(9, 64, 1).unwrap(),
        vec![frozen[0].clone()]
    );
    assert_eq!(p.preview_note_steps(10, 0, 64).unwrap(), frozen);
}

#[test]
fn invalid_dependencies_and_transactions_leave_the_source_document_unchanged() {
    let mut p = Project::demo();
    let before = p.clone();
    for edit in [
        Edit::PutPattern {
            pattern: Pattern {
                id: 4,
                name: "Moving lead".into(),
                kind: PatternKind::Mask {
                    notes: 4,
                    rhythm: 3,
                },
            },
        },
        Edit::PutPattern {
            pattern: Pattern {
                id: 4,
                name: "Moving lead".into(),
                kind: PatternKind::AddDegrees {
                    notes: 1,
                    values: 8,
                },
            },
        },
        Edit::RemovePattern { id: 2 },
        Edit::RemoveInstrument { id: 1 },
        Edit::RemoveChannel { id: 1 },
        Edit::SetValue {
            pattern: 2,
            step: 0,
            value: Some(0.5),
        },
    ] {
        assert!(p.apply(edit).is_err());
        assert_eq!(p, before);
    }
    assert!(p
        .apply_transaction(&[
            Edit::RenameProject {
                title: "Partial mutation".into()
            },
            Edit::RemovePart { id: 1 }
        ])
        .is_err());
    assert_eq!(p, before);
}

#[test]
fn shared_instances_reject_overlap_even_when_one_lane_is_muted() {
    let mut p = Project::demo();
    let before = p.clone();
    let mut lane = Lane::new(4, "Hidden overlap");
    lane.placements.push(Placement::new(5, 1, 80, 16));
    let error = p.apply(Edit::PutLane { lane }).unwrap_err().to_string();
    assert!(error.contains("separate instance"), "{error}");
    assert_eq!(p, before);
    let mut safe = p.lane(1).unwrap().clone();
    safe.placements[1].start_step = 64;
    safe.placements[1].length_steps = 192;
    p.apply(Edit::PutLane { lane: safe }).unwrap();
}

#[test]
fn audio_mute_does_not_erase_events_but_lane_activation_does() {
    let p = Project::demo();
    let mut flags = LiveFlags::default();
    flags
        .set_channel_muted(ChannelRef::Channel(3), true)
        .unwrap();
    let muted = p.compile_with_flags(&flags).unwrap();
    let kick = muted.instrument_index(3).unwrap();
    let ev = muted.event_for(kick, 64).unwrap();
    assert_eq!(ev.part_id, Some(4));
    flags.set_lane_muted(3, true);
    let disabled = p.compile_with_flags(&flags).unwrap();
    assert!(disabled.event_for(kick, 64).is_none());
    assert!(!disabled.required_instrument_ids.contains(&3));
    assert!(p.evaluate_pattern(6, 64).unwrap().active());
    assert!(disabled.provenance.iter().all(|e| e.part != 4));
}

#[test]
fn per_note_links_do_not_leak_and_global_values_remain_separate() {
    let mut p = Project::demo();
    p.patterns.push(Pattern {
        id: 9,
        name: "Center bias".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 16,
            mode: cadence_core::params::ValueMode::Hold,
            points: vec![ValuePoint {
                step: 0,
                value: 0.1,
            }],
        },
    });
    let target = InstrumentParam::Module {
        zone: 1,
        module: 9,
        param: ModuleParam::Pan,
    };
    p.modulations.push(Modulation {
        id: 2,
        name: "Global pan".into(),
        target: ModulationTarget::Instrument {
            instrument: 1,
            parameter: target,
        },
        pattern: 9,
    });
    p.apply(Edit::SetException {
        placement: 2,
        step: 1,
        value: Some(Step::default()),
    })
    .unwrap();
    let prepared = p.compile().unwrap();
    let lead = prepared.instrument_index(1).unwrap();
    for step in 0..64 {
        if let Some(ev) = prepared.event_for(lead, step) {
            assert!(prepared.event_locks(ev).iter().all(|l| l.target != target));
        }
    }
    let ev64 = prepared.event_for(lead, 64).unwrap();
    let trigger = prepared
        .event_locks(ev64)
        .iter()
        .find(|l| l.target == target)
        .unwrap()
        .value;
    assert!(prepared.event_for(lead, 65).is_none());
    let mod_lane = prepared
        .mod_lanes
        .iter()
        .find(|l| usize::from(l.inst_idx) == lead && l.target == target)
        .unwrap();
    let continuous = mod_lane.evaluate(64.0);
    assert!((continuous - 0.1).abs() < 1e-6);
    assert!((trigger - continuous).abs() > 0.1);
    assert_eq!(trigger, p.value_at(8, 64.0).unwrap());
    assert_eq!(p.value_at(8, 64.0).unwrap(), p.value_at(8, 80.0).unwrap());
}

#[test]
fn channel_trim_is_not_flattened_into_level_shape_or_ducking() {
    let mut p = Project::demo();
    let prepared = p
        .apply(Edit::SetParam {
            address: cadence_core::params::ParamAddress::Channel {
                channel: ChannelRef::Channel(4),
                param: cadence_core::mixer::ChannelParam::Gain,
            },
            value: 0.3,
        })
        .unwrap();
    let target = MixerParamTarget::ChannelGainScale {
        channel: ChannelRef::Channel(4),
    };
    assert!((prepared.resolve_mixer_param(target, 0.0) - 0.65).abs() < 1e-6);
    assert!((prepared.channel(4).unwrap().strip.gain - 0.3).abs() < 1e-6);
    assert_eq!(prepared.mixer.ducks, p.ducks);
    assert_eq!(p.modulations[0].pattern, 7);
}

#[test]
fn unplaced_sampler_keeps_identity_without_becoming_a_required_render_asset() {
    let mut p = Project::demo();
    let prepared = p
        .apply_transaction(&[
            Edit::CreateChannel {
                channel: Channel::new(6, ChannelKind::Audio, "Sampler channel"),
            },
            Edit::CreateInstrument {
                instrument: InstrumentInstance {
                    id: 4,
                    name: "Unloaded sampler".into(),
                    rack: RackPatch::default_sampler(),
                    channel: 6,
                },
            },
        ])
        .unwrap();
    assert!(prepared.instrument_index(4).is_some());
    assert!(!prepared.required_instrument_ids.contains(&4));
    assert_eq!(
        prepared.instruments[prepared.instrument_index(4).unwrap()].id,
        4
    );
}

#[test]
fn full_horizon_compilation_has_direct_positions_and_preserves_source_bounds() {
    let mut p = Project::demo();
    p.bars = 256;
    p.lanes.truncate(1);
    p.lanes[0].placements = vec![Placement::new(1, 1, 0, 4096)];
    let prepared = p.compile().unwrap();
    let track = prepared.instrument_index(1).unwrap();
    assert_eq!(prepared.arrangement_steps(), 4096);
    assert_eq!(prepared.step_slot(4095), Some(4095));
    assert_eq!(prepared.step_slot(4096), Some(0));
    assert_eq!(
        prepared.event_for(track, 4096).unwrap().midi,
        prepared.event_for(track, 0).unwrap().midi
    );
    assert_eq!(p.pattern_period(1).unwrap(), 5);
    assert_eq!(
        p.patterns.len(),
        8,
        "compiler must not persist expanded variants"
    );
    let mut overflow = Part::new(99, "Bad part", 1, 1);
    overflow.transpose = 120;
    assert!(p.apply(Edit::PutPart { part: overflow }).is_err());
}

#[test]
fn repeating_event_period_includes_conditions_without_expanding_literal_storage() {
    let mut project = Project::empty();
    let mut steps = vec![Step::default(); 5];
    steps[0].active = true;
    steps[0].condition = cadence_core::music::StepCondition { turn: 2, every: 2 };
    steps[2].active = true;
    steps[2].condition = cadence_core::music::StepCondition { turn: 3, every: 3 };
    project.patterns.push(Pattern {
        id: 1,
        name: "Conditional five".into(),
        kind: PatternKind::Notes { steps },
    });
    assert_eq!(project.pattern_period(1).unwrap(), 30);
    assert_eq!(
        project.preview_note_steps(1, 0, 30).unwrap(),
        project.preview_note_steps(1, 30, 30).unwrap()
    );
    assert_ne!(
        project.preview_note_steps(1, 0, 5).unwrap(),
        project.preview_note_steps(1, 5, 5).unwrap()
    );
    let PatternKind::Notes { steps } = &project.pattern(1).unwrap().kind else {
        panic!("source changed");
    };
    assert_eq!(steps.len(), 5);
}

#[test]
fn shared_dependency_dag_is_evaluated_once_per_source_without_exponential_work() {
    let mut project = Project::empty();
    project.patterns.push(Pattern {
        id: 1,
        name: "Seed".into(),
        kind: PatternKind::Notes {
            steps: vec![Step {
                active: true,
                ..Step::default()
            }],
        },
    });
    for id in 2..=32 {
        project.patterns.push(Pattern {
            id,
            name: format!("Shared {id}"),
            kind: PatternKind::Mask {
                notes: id - 1,
                rhythm: id - 1,
            },
        });
    }
    project.validate().unwrap();
    assert_eq!(project.pattern_period(32).unwrap(), 1);
    assert!(project
        .preview_note_steps(32, 0, 8)
        .unwrap()
        .iter()
        .all(|s| s.active));
    project.patterns.push(Pattern {
        id: 33,
        name: "Too deep".into(),
        kind: PatternKind::Mask {
            notes: 32,
            rhythm: 1,
        },
    });
    assert!(project.validate().is_err());
}

#[test]
fn local_exceptions_reject_repeating_conditions_without_mutating_the_project() {
    let mut project = Project::demo();
    let before = project.clone();
    let condition = cadence_core::music::StepCondition { turn: 2, every: 2 };
    let error = project
        .apply(Edit::SetException {
            placement: 1,
            step: 0,
            value: Some(Step {
                active: true,
                condition,
                ..Step::default()
            }),
        })
        .unwrap_err();
    assert!(error.to_string().contains("unconditional"));
    assert_eq!(project, before);
}
