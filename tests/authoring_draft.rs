use cadence_app::keys::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use cadence_app::{
    authoring::{Draft, DraftField, DraftInput, DraftOutcome, DraftSubmission},
    parse_key_sequence,
};
use cadence_core::{
    material::{Edit, Pattern, PatternKind, Project, ValuePoint, ValueUnit},
    music::Step,
    params::ValueMode,
};

fn fixture() -> Project {
    let mut p = Project::empty();
    p.patterns = vec![
        Pattern {
            id: 1,
            name: "Motif".into(),
            kind: PatternKind::Notes {
                steps: [0, 2, 4, 7, 6]
                    .map(|n| Step {
                        active: true,
                        degree: n + 1,
                        velocity: 0.8,
                        gate: 0.75,
                        ..Step::default()
                    })
                    .into(),
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
                    .map(|(step, value)| ValuePoint { step, value })
                    .into(),
            },
        },
        Pattern {
            id: 3,
            name: "Pan".into(),
            kind: PatternKind::Values {
                unit: ValueUnit::Normalized,
                period: 16,
                mode: ValueMode::Ramp,
                points: vec![
                    ValuePoint {
                        step: 0,
                        value: 0.2,
                    },
                    ValuePoint {
                        step: 8,
                        value: 0.8,
                    },
                ],
            },
        },
        Pattern {
            id: 4,
            name: "Pulse".into(),
            kind: PatternKind::Rhythm {
                steps: (0..16).map(|n| n % 4 == 0).collect(),
            },
        },
        Pattern {
            id: 5,
            name: "ExistingDerived".into(),
            kind: PatternKind::AddDegrees {
                notes: 1,
                values: 2,
            },
        },
        Pattern {
            id: 6,
            name: "Descendant".into(),
            kind: PatternKind::Transpose {
                source: 5,
                semitones: 12,
            },
        },
    ];
    p.validate().unwrap();
    p
}
fn keys(draft: &mut Draft, p: &Project, input: &str) -> DraftOutcome {
    let mut outcome = DraftOutcome::Continue;
    for key in parse_key_sequence(input) {
        outcome = draft.handle_key(p, key);
    }
    outcome
}
fn back(draft: &mut Draft, p: &Project) {
    draft.handle_key(p, KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
}
fn submission(outcome: DraftOutcome) -> DraftSubmission {
    match outcome {
        DraftOutcome::Submit(value) => value,
        other => panic!("Expected submission, got {other:?}"),
    }
}
fn accept(p: &mut Project, value: DraftSubmission) {
    p.apply(Edit::PutPattern {
        pattern: Pattern {
            id: value.target.unwrap_or_else(|| p.max_id() + 1),
            name: value.name,
            kind: value.kind,
        },
    })
    .unwrap();
}

#[test]
fn degrees_creation_is_a_cancellable_nonmutating_draft_with_one_final_submission() {
    let p = fixture();
    let before = p.clone();
    let mut draft = Draft::new_pattern();
    assert_eq!(draft.view(&p).input, DraftInput::Choice);
    keys(&mut draft, &p, "dNewHarmony<Enter>");
    assert_eq!(draft.view(&p).field, DraftField::Period);
    assert_eq!(draft.view(&p).text, "16");
    assert!(draft.view(&p).replace_on_type);
    let value = submission(keys(&mut draft, &p, "64<Enter>"));
    assert_eq!(value.name, "NewHarmony");
    assert_eq!(value.target, None);
    assert_eq!(
        value.kind,
        PatternKind::Values {
            unit: ValueUnit::Degrees,
            period: 64,
            mode: ValueMode::Hold,
            points: vec![ValuePoint {
                step: 0,
                value: 0.0
            }]
        }
    );
    assert_eq!(
        p, before,
        "even submission leaves persistence to the action layer"
    );
    let mut cancelled = Draft::new_pattern();
    keys(&mut cancelled, &p, "dUnfinished<Enter>64");
    assert_eq!(keys(&mut cancelled, &p, "<Esc>"), DraftOutcome::Cancel);
    assert_eq!(p, before);
}

#[test]
fn typed_inputs_preselect_compatible_material_and_keep_derived_relationships() {
    let mut p = fixture();
    let before = p.clone();
    let mut draft = Draft::derive(Some(2));
    keys(&mut draft, &p, "Moving<Enter>a");
    let notes = draft.view(&p);
    assert_eq!(notes.field, DraftField::Notes);
    assert_eq!(
        notes
            .candidates
            .iter()
            .filter_map(|c| c.id)
            .collect::<Vec<_>>(),
        vec![1, 5, 6]
    );
    assert_eq!(notes.candidates[notes.selected.unwrap()].id, Some(1));
    assert!(notes.summary.contains("320-step"));
    assert!(notes.preview[0].contains("D4"));
    keys(&mut draft, &p, "<Enter>");
    let degrees = draft.view(&p);
    assert_eq!(degrees.field, DraftField::Degrees);
    assert_eq!(degrees.candidates.len(), 1);
    assert_eq!(degrees.candidates[0].id, Some(2));
    let value = submission(keys(&mut draft, &p, "<Enter>"));
    assert_eq!(
        value.kind,
        PatternKind::AddDegrees {
            notes: 1,
            values: 2
        }
    );
    assert_eq!(p, before);
    accept(&mut p, value);
    assert_eq!(p.pattern_period(7).unwrap(), 320);
    assert_eq!(p.pattern(7).unwrap().kind.sources(), vec![1, 2]);
}

#[test]
fn search_selects_a_visible_source_by_id_and_never_accepts_a_hidden_match() {
    let p = fixture();
    let mut draft = Draft::derive(None);
    keys(&mut draft, &p, "Upper<Enter>t/Derived");
    let view = draft.view(&p);
    assert!(view.searching);
    assert_eq!(view.candidates.len(), 1);
    assert_eq!(view.candidates[0].id, Some(5));
    // First Enter locks filter (universal `/` search semantics); second Enter accepts the match.
    keys(&mut draft, &p, "<Enter>");
    assert!(!draft.view(&p).searching);
    assert_eq!(draft.view(&p).query, "Derived");
    assert_eq!(draft.view(&p).field, DraftField::Notes);
    keys(&mut draft, &p, "<Enter>");
    assert_eq!(draft.view(&p).field, DraftField::Semitones);
    let value = submission(keys(&mut draft, &p, "-12<Enter>"));
    assert_eq!(
        value.kind,
        PatternKind::Transpose {
            source: 5,
            semitones: -12
        }
    );
    let mut empty = Draft::derive(Some(1));
    keys(&mut empty, &p, "NoMatch<Enter>a/absent-source");
    assert!(empty.view(&p).candidates.is_empty());
    // Enter locks empty filter; second Enter rejects hidden match with an error.
    assert_eq!(keys(&mut empty, &p, "<Enter>"), DraftOutcome::Continue);
    assert!(!empty.view(&p).searching);
    assert_eq!(empty.view(&p).query, "absent-source");
    assert_eq!(keys(&mut empty, &p, "<Enter>"), DraftOutcome::Continue);
    assert!(empty.view(&p).error.is_some());
    assert_eq!(empty.view(&p).query, "absent-source");
    // Esc clears the locked filter first before a second Esc cancels the draft.
    assert_eq!(keys(&mut empty, &p, "<Esc>"), DraftOutcome::Continue);
    assert!(!empty.view(&p).searching);
    assert_eq!(empty.view(&p).query, "");
    assert!(!empty.view(&p).candidates.is_empty());
    assert_eq!(keys(&mut empty, &p, "<Esc>"), DraftOutcome::Cancel);
}

#[test]
fn invalid_fields_and_action_rejections_preserve_editable_draft_data() {
    let p = fixture();
    let mut draft = Draft::new_pattern();
    keys(&mut draft, &p, "dHarmony<Enter>");
    assert_eq!(draft.view(&p).field, DraftField::Name);
    assert!(draft
        .view(&p)
        .error
        .as_ref()
        .unwrap()
        .contains("already exists"));
    keys(&mut draft, &p, "<C-u>Fresh<Enter>0<Enter>");
    assert_eq!(draft.view(&p).field, DraftField::Period);
    assert_eq!(draft.view(&p).text, "0");
    assert!(draft.view(&p).error.is_some());
    keys(&mut draft, &p, "<C-u>64");
    back(&mut draft, &p);
    assert_eq!(draft.view(&p).text, "Fresh");
    keys(&mut draft, &p, "Renamed<Enter>");
    assert_eq!(
        draft.view(&p).text,
        "64",
        "back navigation retains accepted or edited fields"
    );
    let value = submission(keys(&mut draft, &p, "<Enter>"));
    assert_eq!(value.name, "Renamed");
    draft.set_error("Rejected by the action compiler".into());
    let view = draft.view(&p);
    assert_eq!(view.text, "64");
    assert_eq!(view.fields[1].value, "Renamed");
    assert!(view.error.unwrap().contains("compiler"));
    assert_eq!(submission(keys(&mut draft, &p, "<Enter>")), value);
}

#[test]
fn editing_a_derivation_filters_self_and_transitive_cycles_and_preserves_identity() {
    let mut p = fixture();
    let mut draft = Draft::edit(&p, 5).unwrap();
    assert_eq!(draft.view(&p).field, DraftField::Operation);
    keys(&mut draft, &p, "t");
    let view = draft.view(&p);
    assert_eq!(
        view.candidates
            .iter()
            .filter_map(|c| c.id)
            .collect::<Vec<_>>(),
        vec![1]
    );
    keys(&mut draft, &p, "<Enter>");
    let value = submission(keys(&mut draft, &p, "7<Enter>"));
    assert_eq!(value.target, Some(5));
    assert_eq!(value.name, "ExistingDerived");
    assert_eq!(
        value.kind,
        PatternKind::Transpose {
            source: 1,
            semitones: 7
        }
    );
    accept(&mut p, value);
    assert_eq!(p.pattern(6).unwrap().kind.sources(), vec![5]);
    // Literal patterns also support `Draft::edit` for Name + Length while preserving note steps.
    let mut lit_draft = Draft::edit(&p, 1).expect("literal Notes pattern supports metadata edit");
    let lit_sub = submission(keys(&mut lit_draft, &p, "RenamedNotes<Enter>8<Enter>"));
    assert_eq!(lit_sub.target, Some(1));
    assert_eq!(lit_sub.name, "RenamedNotes");
    match &lit_sub.kind {
        PatternKind::Notes { steps } => assert_eq!(steps.len(), 8),
        other => panic!("expected resized literal Notes, got {other:?}"),
    }
    assert!(Draft::edit(&p, 9999).is_err());
}

#[test]
fn alternate_kinds_operations_and_numeric_units_build_core_valid_definitions() {
    let mut p = fixture();
    for (script, expected) in [
        (
            "nEmptyNotes<Enter>5<Enter>",
            PatternKind::Notes {
                steps: vec![Step::default(); 5],
            },
        ),
        (
            "rEmptyRhythm<Enter>7<Enter>",
            PatternKind::Rhythm {
                steps: vec![false; 7],
            },
        ),
        (
            "vSweep<Enter>64<Enter>r",
            PatternKind::Values {
                unit: ValueUnit::Normalized,
                period: 64,
                mode: ValueMode::Ramp,
                points: vec![ValuePoint {
                    step: 0,
                    value: 0.0,
                }],
            },
        ),
        (
            "sScatter<Enter>16<Enter>123<Enter>20%<Enter>80%<Enter>",
            PatternKind::SeededValues {
                unit: ValueUnit::Normalized,
                period: 16,
                seed: 123,
                min: 0.2,
                max: 0.8,
            },
        ),
    ] {
        let mut draft = Draft::new_pattern();
        let value = submission(keys(&mut draft, &p, script));
        assert_eq!(value.kind, expected);
        accept(&mut p, value);
    }
    let mut mask = Draft::derive(Some(1));
    let value = submission(keys(
        &mut mask,
        &p,
        "Gated<Enter>m<Enter>/Pulse<Enter><Enter>",
    ));
    assert_eq!(
        value.kind,
        PatternKind::Mask {
            notes: 1,
            rhythm: 4
        }
    );
    accept(&mut p, value);
    let mut from = Draft::derive(Some(2));
    let value = submission(keys(
        &mut from,
        &p,
        "Low<Enter>f<Enter>/Pulse<Enter><Enter>-14<Enter>85%<Enter>1.25<Enter>",
    ));
    assert_eq!(
        value.kind,
        PatternKind::NotesFromValues {
            values: 2,
            rhythm: 4,
            degree_offset: -14,
            velocity: 0.85,
            gate: 1.25
        }
    );
    accept(&mut p, value);
}

#[test]
fn backtracking_changes_operation_without_reinterpreting_incompatible_sources() {
    let p = fixture();
    let mut draft = Draft::derive(Some(2));
    keys(&mut draft, &p, "Changed<Enter>a<Enter>");
    assert_eq!(draft.view(&p).field, DraftField::Degrees);
    back(&mut draft, &p);
    back(&mut draft, &p);
    assert_eq!(draft.view(&p).field, DraftField::Operation);
    keys(&mut draft, &p, "m<Enter>");
    let view = draft.view(&p);
    assert_eq!(view.field, DraftField::Rhythm);
    assert!(
        !view.candidates.iter().any(|c| c.id == Some(2)),
        "degrees cannot become a trigger by position"
    );
    let value = submission(keys(&mut draft, &p, "/Pulse<Enter><Enter>"));
    assert_eq!(
        value.kind,
        PatternKind::Mask {
            notes: 1,
            rhythm: 4
        }
    );
}

#[test]
fn empty_or_changed_source_libraries_are_recoverable_and_do_not_guess_references() {
    let mut p = Project::empty();
    let mut draft = Draft::derive(None);
    keys(&mut draft, &p, "Missing<Enter>a");
    let view = draft.view(&p);
    assert!(view.candidates.is_empty());
    assert!(view.summary.contains("No compatible"));
    assert_eq!(keys(&mut draft, &p, "<Enter>"), DraftOutcome::Continue);
    assert!(draft.view(&p).error.is_some());
    p = fixture();
    let mut draft = Draft::derive(Some(1));
    keys(&mut draft, &p, "Stale<Enter>a");
    p.patterns.clear();
    assert_eq!(keys(&mut draft, &p, "<Enter>"), DraftOutcome::Continue);
    assert!(draft.view(&p).error.unwrap().contains("missing"));
}

#[test]
fn bounds_nonfinite_numbers_and_repeated_accept_keys_do_not_escape_validation() {
    let p = fixture();
    let mut draft = Draft::new_pattern();
    keys(&mut draft, &p, "nBounded<Enter>1025<Enter>");
    assert!(draft.view(&p).error.is_some());
    keys(&mut draft, &p, "<C-u>1024");
    assert_eq!(
        draft.handle_key(
            &p,
            KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Repeat)
        ),
        DraftOutcome::Continue
    );
    assert!(matches!(
        keys(&mut draft, &p, "<Enter>"),
        DraftOutcome::Submit(_)
    ));
    let mut seed = Draft::new_pattern();
    keys(
        &mut seed,
        &p,
        "sBadRange<Enter>16<Enter>771<Enter>NaN<Enter>",
    );
    assert_eq!(seed.view(&p).field, DraftField::Minimum);
    assert!(seed.view(&p).error.is_some());
    keys(&mut seed, &p, "<C-u>80%<Enter>20%<Enter>");
    assert_eq!(seed.view(&p).field, DraftField::Maximum);
    assert!(seed.view(&p).error.unwrap().contains("minimum"));
}
