//! Integration tests for Pattern authoring across all 22 `PatternKind` variants,
//! Rhythm/Values clipboard & `:value-mode`, `:mini` / `:len` guards, and complete
//! Mixer/Section/Part/Placement/LiveTiming workflows.
use cadence_app::view::{DeckView, OverlayView, PatternContentView, PatternDeckView};
use cadence_app::{
    Action, App, ControlTarget, Detail, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, Request,
};
use cadence_core::material::{
    ArpMode, Boundary, EditTiming, ModulationTarget, PatternKind, Project, ValuePoint, ValueUnit,
};
use cadence_core::mixer::{ChannelKind, ChannelRef, EffectParam};
use cadence_core::params::{ParamAddress, ValueMode};
use cadence_core::runtime::AudioSnapshot;
use std::collections::HashSet;

fn demo_app() -> App {
    App::new(Project::demo(), "demo.json".into()).unwrap()
}

fn empty_app() -> App {
    App::new(Project::empty(), "empty.json".into()).unwrap()
}

fn pattern_view(app: &mut App) -> PatternDeckView {
    app.press_keys("gp").unwrap();
    let frame = app.frame_view(&AudioSnapshot::default(), "ready");
    frame
        .decks
        .into_iter()
        .find_map(|d| match d.view {
            DeckView::Pattern(p) => Some(p),
            _ => None,
        })
        .expect("Pattern deck visible after gp")
}

fn pattern_kind_tag(kind: &PatternKind) -> &'static str {
    match kind {
        PatternKind::Notes { .. } => "Notes",
        PatternKind::Rhythm { .. } => "Rhythm",
        PatternKind::Values {
            unit: ValueUnit::Degrees,
            ..
        } => "Values(Degrees)",
        PatternKind::Values {
            unit: ValueUnit::Normalized,
            ..
        } => "Values(Normalized)",
        PatternKind::SeededValues {
            unit: ValueUnit::Normalized,
            ..
        } => "SeededValues(Normalized)",
        PatternKind::SeededValues {
            unit: ValueUnit::Degrees,
            ..
        } => "SeededValues(Degrees)",
        PatternKind::Mini { .. } => "Mini",
        PatternKind::Euclidean { .. } => "Euclidean",
        PatternKind::AddDegrees { .. } => "AddDegrees",
        PatternKind::Mask { .. } => "Mask",
        PatternKind::Transpose { .. } => "Transpose",
        PatternKind::NotesFromValues { .. } => "NotesFromValues",
        PatternKind::TimeScale { .. } => "TimeScale",
        PatternKind::Swing { .. } => "Swing",
        PatternKind::Ratchet { .. } => "Ratchet",
        PatternKind::Shift { .. } => "Shift",
        PatternKind::Reverse { .. } => "Reverse",
        PatternKind::Chance { .. } => "Chance",
        PatternKind::Every { .. } => "Every",
        PatternKind::Arp { .. } => "Arp",
        PatternKind::RangeFold { .. } => "RangeFold",
        PatternKind::Concat { .. } => "Concat",
        PatternKind::Stack { .. } => "Stack",
        PatternKind::Shape { .. } => "Shape",
    }
}

#[test]
fn all_22_pattern_kinds_authored_via_drafts_and_edited_in_place() {
    let mut app = empty_app();
    app.press_keys("gp").unwrap();

    let mut seen_tags: HashSet<&'static str> = HashSet::new();
    let record_selected = |app: &App, seen: &mut HashSet<&'static str>| {
        let pat = app.selected_pattern_object().expect("selected pattern");
        seen.insert(pattern_kind_tag(&pat.kind));
        pat.id
    };

    // 1. Notes (`nn`)
    app.press_keys("nnBaseNotes<Enter>8<Enter>").unwrap();
    let notes_id = record_selected(&app, &mut seen_tags);
    assert!(matches!(
        app.pattern(notes_id).unwrap().kind,
        PatternKind::Notes { ref steps } if steps.len() == 8
    ));
    app.execute_command("note 1 0,2,4 80% 1").unwrap();

    // Case-insensitive duplicate pattern name rejection in Draft.
    app.press_keys("nnbasenotes<Enter>").unwrap();
    assert!(app.is_authoring());
    assert!(
        app.authoring_view()
            .unwrap()
            .error
            .as_deref()
            .is_some_and(|e| e.contains("already exists")),
        "duplicate pattern name must be rejected case-insensitively"
    );
    app.press_keys("<Esc>").unwrap();

    // 2. Rhythm (`nr`)
    app.press_keys("nrBaseRhy<Enter>8<Enter>").unwrap();
    let rhy_id = record_selected(&app, &mut seen_tags);
    assert!(matches!(
        app.pattern(rhy_id).unwrap().kind,
        PatternKind::Rhythm { ref steps } if steps.len() == 8
    ));

    // 3. Degrees (`nd`)
    app.press_keys("ndBaseDeg<Enter>16<Enter>").unwrap();
    record_selected(&app, &mut seen_tags);

    // 4. Values Normalized (`nv`)
    app.press_keys("nvBaseVal<Enter>16<Enter>h").unwrap();
    record_selected(&app, &mut seen_tags);

    // 5. SeededValues Normalized (`ns`) + in-place `r` edit (starts on ValueUnit!)
    app.press_keys("nsSeedNorm<Enter>16<Enter>77<Enter>10%<Enter>90%<Enter>")
        .unwrap();
    let seed_norm_id = record_selected(&app, &mut seen_tags);
    assert!(matches!(
        app.pattern(seed_norm_id).unwrap().kind,
        PatternKind::SeededValues {
            unit: ValueUnit::Normalized,
            period: 16,
            seed: 77,
            ..
        }
    ));
    app.press_keys("rn32<Enter>99<Enter>20%<Enter>80%<Enter>")
        .unwrap();
    assert!(matches!(
        app.pattern(seed_norm_id).unwrap().kind,
        PatternKind::SeededValues {
            unit: ValueUnit::Normalized,
            period: 32,
            seed: 99,
            ..
        }
    ));

    // 6. SeededValues Degrees (`ng`) + in-place `r` edit (starts on ValueUnit!)
    app.press_keys("ngSeedDeg<Enter>16<Enter>42<Enter>-7<Enter>7<Enter>")
        .unwrap();
    let seed_deg_id = record_selected(&app, &mut seen_tags);
    assert!(matches!(
        app.pattern(seed_deg_id).unwrap().kind,
        PatternKind::SeededValues {
            unit: ValueUnit::Degrees,
            period: 16,
            seed: 42,
            ..
        }
    ));
    app.press_keys("rd8<Enter>55<Enter>-3<Enter>4<Enter>")
        .unwrap();
    assert!(matches!(
        app.pattern(seed_deg_id).unwrap().kind,
        PatternKind::SeededValues {
            unit: ValueUnit::Degrees,
            period: 8,
            seed: 55,
            ..
        }
    ));

    // 7. Mini (`nm`) + in-place `r` edit (starts on MiniCode -> Period!)
    app.press_keys("nmMiniSeq<Enter>8<Enter>c4 e4 g4 ~<Enter>")
        .unwrap();
    let mini_id = record_selected(&app, &mut seen_tags);
    assert!(matches!(
        app.pattern(mini_id).unwrap().kind,
        PatternKind::Mini { ref code, steps: 8 } if code == "c4 e4 g4 ~"
    ));
    app.press_keys("rd4 f4 a4 c5<Enter>16<Enter>").unwrap();
    assert!(matches!(
        app.pattern(mini_id).unwrap().kind,
        PatternKind::Mini { ref code, steps: 16 } if code == "d4 f4 a4 c5"
    ));

    // 8. Euclidean (`ne`) + in-place `r` edit (starts on Period -> Pulses -> Rotation!)
    app.press_keys("neEuclid5<Enter>16<Enter>5<Enter>2<Enter>")
        .unwrap();
    let euclid_id = record_selected(&app, &mut seen_tags);
    assert!(matches!(
        app.pattern(euclid_id).unwrap().kind,
        PatternKind::Euclidean {
            pulses: 5,
            steps: 16,
            rotation: 2
        }
    ));
    app.press_keys("r12<Enter>7<Enter>1<Enter>").unwrap();
    assert!(matches!(
        app.pattern(euclid_id).unwrap().kind,
        PatternKind::Euclidean {
            pulses: 7,
            steps: 12,
            rotation: 1
        }
    ));

    // Select BaseNotes before deriving so note-source pickers preselect it cleanly.
    app.apply(Action::SelectPattern { pattern: notes_id })
        .unwrap();

    // 9. AddDegrees (`d` -> 'a') + in-place `r` (starts on Operation!)
    app.press_keys("dDerAdd<Enter>a<Enter><Enter>").unwrap();
    let id = record_selected(&app, &mut seen_tags);
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::AddDegrees { .. }
    ));
    app.press_keys("r<Enter><Enter><Enter>").unwrap();
    assert!(!app.is_authoring());

    // 10. Mask (`d` -> 'm') + in-place `r`
    app.apply(Action::SelectPattern { pattern: notes_id })
        .unwrap();
    app.press_keys("dDerMask<Enter>m<Enter><Enter>").unwrap();
    let id = record_selected(&app, &mut seen_tags);
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::Mask { .. }
    ));
    app.press_keys("r<Enter><Enter><Enter>").unwrap();
    assert!(!app.is_authoring());

    // 11. Transpose (`d` -> 't') + in-place `r`
    app.apply(Action::SelectPattern { pattern: notes_id })
        .unwrap();
    app.press_keys("dDerTrans<Enter>t<Enter>7<Enter>").unwrap();
    let id = record_selected(&app, &mut seen_tags);
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::Transpose { semitones: 7, .. }
    ));
    app.press_keys("r<Enter><Enter>-5<Enter>").unwrap();
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::Transpose { semitones: -5, .. }
    ));

    // 12. NotesFromValues (`d` -> 'f') + in-place `r`
    app.press_keys("dDerFromVal<Enter>f<Enter><Enter>2<Enter>75%<Enter>0.5<Enter>")
        .unwrap();
    let id = record_selected(&app, &mut seen_tags);
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::NotesFromValues {
            degree_offset: 2,
            ..
        }
    ));
    app.press_keys("r<Enter><Enter><Enter>4<Enter>80%<Enter>0.75<Enter>")
        .unwrap();
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::NotesFromValues {
            degree_offset: 4,
            ..
        }
    ));

    // 13. TimeScale (`d` -> 'x') + in-place `r`
    app.apply(Action::SelectPattern { pattern: notes_id })
        .unwrap();
    app.press_keys("dDerTime<Enter>x<Enter>2<Enter>1<Enter>")
        .unwrap();
    let id = record_selected(&app, &mut seen_tags);
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::TimeScale { num: 2, den: 1, .. }
    ));
    app.press_keys("r<Enter><Enter>1<Enter>2<Enter>").unwrap();
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::TimeScale { num: 1, den: 2, .. }
    ));

    // 14. Swing (`d` -> 'w', amount 0..=45%) + in-place `r`
    app.apply(Action::SelectPattern { pattern: notes_id })
        .unwrap();
    app.press_keys("dDerSwing<Enter>w<Enter>28%<Enter>")
        .unwrap();
    let id = record_selected(&app, &mut seen_tags);
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::Swing { amount, .. } if (amount - 0.28).abs() < 1e-3
    ));
    app.press_keys("r<Enter><Enter>18%<Enter>").unwrap();
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::Swing { amount, .. } if (amount - 0.18).abs() < 1e-3
    ));

    // 15. Ratchet (`d` -> 'r') + in-place `r`
    app.apply(Action::SelectPattern { pattern: notes_id })
        .unwrap();
    app.press_keys("dDerRatchet<Enter>r<Enter>3<Enter>")
        .unwrap();
    let id = record_selected(&app, &mut seen_tags);
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::Ratchet { count: 3, .. }
    ));
    app.press_keys("r<Enter><Enter>4<Enter>").unwrap();
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::Ratchet { count: 4, .. }
    ));

    // 16. Shift (`d` -> 'h') + in-place `r`
    app.apply(Action::SelectPattern { pattern: notes_id })
        .unwrap();
    app.press_keys("dDerShift<Enter>h<Enter>-2<Enter>").unwrap();
    let id = record_selected(&app, &mut seen_tags);
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::Shift { steps: -2, .. }
    ));
    app.press_keys("r<Enter><Enter>3<Enter>").unwrap();
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::Shift { steps: 3, .. }
    ));

    // 17. Reverse (`d` -> 'v') + in-place `r`
    app.apply(Action::SelectPattern { pattern: notes_id })
        .unwrap();
    app.press_keys("dDerRev<Enter>v<Enter>").unwrap();
    let id = record_selected(&app, &mut seen_tags);
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::Reverse { .. }
    ));
    app.press_keys("r<Enter><Enter>").unwrap();
    assert!(!app.is_authoring());

    // 18. Chance (`d` -> 'c') + in-place `r`
    app.apply(Action::SelectPattern { pattern: notes_id })
        .unwrap();
    app.press_keys("dDerChance<Enter>c<Enter>70%<Enter>11<Enter>")
        .unwrap();
    let id = record_selected(&app, &mut seen_tags);
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::Chance { seed: 11, probability, .. } if (probability - 0.70).abs() < 1e-3
    ));
    app.press_keys("r<Enter><Enter>85%<Enter>19<Enter>")
        .unwrap();
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::Chance { seed: 19, probability, .. } if (probability - 0.85).abs() < 1e-3
    ));

    // 19. Every (`d` -> 'e', fields: EveryInterval -> Source -> AltSource) + in-place `r`
    app.apply(Action::SelectPattern { pattern: notes_id })
        .unwrap();
    app.press_keys("dDerEvery<Enter>e4<Enter><Enter>/MiniSeq<Enter><Enter>")
        .unwrap();
    let id = record_selected(&app, &mut seen_tags);
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::Every { interval: 4, .. }
    ));
    app.press_keys("r<Enter>2<Enter><Enter><Enter>").unwrap();
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::Every { interval: 2, .. }
    ));

    // 20. Arp (`d` -> 'p', ArpMode: u/d/p/r) + in-place `r`
    app.apply(Action::SelectPattern { pattern: notes_id })
        .unwrap();
    app.press_keys("dDerArp<Enter>p<Enter>d2<Enter>").unwrap();
    let id = record_selected(&app, &mut seen_tags);
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::Arp {
            mode: ArpMode::Down,
            rate: 2,
            ..
        }
    ));
    app.press_keys("r<Enter><Enter>p3<Enter>").unwrap();
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::Arp {
            mode: ArpMode::UpDown,
            rate: 3,
            ..
        }
    ));

    // 21. RangeFold (`d` -> 'o') + in-place `r`
    app.apply(Action::SelectPattern { pattern: notes_id })
        .unwrap();
    app.press_keys("dDerFold<Enter>o<Enter>C3<Enter>C5<Enter>")
        .unwrap();
    let id = record_selected(&app, &mut seen_tags);
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::RangeFold {
            low_midi: 48,
            high_midi: 72,
            ..
        }
    ));
    app.press_keys("r<Enter><Enter>36<Enter>60<Enter>").unwrap();
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::RangeFold {
            low_midi: 36,
            high_midi: 60,
            ..
        }
    ));

    // 22. Concat (`d` -> 'n') + in-place `r` appending a 3rd part via ExtraParts picker!
    app.apply(Action::SelectPattern { pattern: notes_id })
        .unwrap();
    app.press_keys("dDerConcat<Enter>n<Enter>/MiniSeq<Enter><Enter><Enter>")
        .unwrap();
    let id = record_selected(&app, &mut seen_tags);
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::Concat { ref parts } if parts.len() == 2
    ));
    // Edit in-place: keep Operation, Source, AltSource, then search `/DerRev` + Enter (lock) + Enter (append), then Enter on Done!
    app.press_keys("r<Enter><Enter><Enter>/DerRev<Enter><Enter><Enter>")
        .unwrap();
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::Concat { ref parts } if parts.len() == 3
    ));

    // 23. Stack (`d` -> 's') + in-place `r`
    app.apply(Action::SelectPattern { pattern: notes_id })
        .unwrap();
    app.press_keys("dDerStack<Enter>s<Enter>/MiniSeq<Enter><Enter><Enter>")
        .unwrap();
    let id = record_selected(&app, &mut seen_tags);
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::Stack { ref parts } if parts.len() == 2
    ));
    app.press_keys("r<Enter><Enter><Enter><Enter>").unwrap();
    assert!(!app.is_authoring());

    // 24. Shape (`na` via Create menu -> 'g' to prove zero fake KeyEvent!) + in-place `r` adding optional Gate!
    app.apply(Action::SelectPattern { pattern: notes_id })
        .unwrap();
    app.press_keys("naDerShape<Enter>g<Enter>/BaseVal<Enter><Enter><Enter>")
        .unwrap();
    let id = record_selected(&app, &mut seen_tags);
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::Shape { gate: None, .. }
    ));
    app.press_keys("r<Enter><Enter><Enter>/BaseVal<Enter><Enter>")
        .unwrap();
    assert!(matches!(
        app.pattern(id).unwrap().kind,
        PatternKind::Shape { gate: Some(_), .. }
    ));

    // Every single one of the 22 PatternKind variants (24 sub-variants counting Normalized+Degrees)
    // was authored cleanly via the UI and compiles without errors!
    assert_eq!(seen_tags.len(), 24, "seen variants: {seen_tags:?}");
    app.project().validate().unwrap();
    app.project().compile().unwrap();
}

#[test]
fn rhythm_and_values_clipboard_span_paste_unit_guard_and_value_mode() {
    let mut app = empty_app();
    app.press_keys("gp").unwrap();

    // 1. Rhythm clipboard: yank 4-step span from Euclidean, paste into literal Rhythm, undo atomically!
    app.execute_command("pattern euclid Euc4 3 8 0").unwrap();
    let euc_id = app.selected_pattern.unwrap();
    let expected_first4: Vec<bool> = (0..4)
        .map(|s| match app.resolve_pattern(euc_id, s).unwrap() {
            cadence_core::material::PatternValue::Rhythm(on) => on,
            _ => panic!("expected rhythm"),
        })
        .collect();
    app.cursor = 0;
    app.press_keys("4y").unwrap();

    app.execute_command("pattern rhythm LitRhy ........")
        .unwrap();
    let lit_rhy_id = app.selected_pattern.unwrap();
    let before_rev = app.revision();
    app.cursor = 2;
    app.press_keys("P").unwrap();
    assert_eq!(
        app.revision(),
        before_rev + 1,
        "4-step rhythm paste must commit ONE atomic undo revision"
    );
    if let PatternKind::Rhythm { steps } = &app.pattern(lit_rhy_id).unwrap().kind {
        assert_eq!(&steps[2..6], expected_first4.as_slice());
    } else {
        panic!("expected literal Rhythm");
    }
    // Single step y/p
    app.cursor = 2;
    app.press_keys("y").unwrap();
    app.cursor = 0;
    app.press_keys("p").unwrap(); // pastes at step 1
    if let PatternKind::Rhythm { steps } = &app.pattern(lit_rhy_id).unwrap().kind {
        assert_eq!(steps[1], expected_first4[0]);
    }
    // Undo twice returns LitRhy to all false.
    app.press_keys("uu").unwrap();
    if let PatternKind::Rhythm { steps } = &app.pattern(lit_rhy_id).unwrap().kind {
        assert!(steps.iter().all(|&b| !b));
    }

    // 2. Values clipboard: span yank & paste clears stale points inside span + atomic undo + unit guard.
    app.execute_command("pattern values Env 16 hold 0:0.2 2:0.9 8:0.5 9:0.7 10:0.4")
        .unwrap();
    let env_id = app.selected_pattern.unwrap();
    app.cursor = 0;
    app.press_keys("4y").unwrap(); // captures offsets 0 (0.2) and 2 (0.9) across span=4
    let rev_before_val_paste = app.revision();
    app.cursor = 8;
    app.press_keys("P").unwrap(); // writes 8:0.2, 10:0.9 and clears stale 9:0.7!
    assert_eq!(app.revision(), rev_before_val_paste + 1);
    if let PatternKind::Values { points, .. } = &app.pattern(env_id).unwrap().kind {
        assert!(points
            .iter()
            .any(|p| p.step == 8 && (p.value - 0.2).abs() < 1e-4));
        assert!(points
            .iter()
            .any(|p| p.step == 10 && (p.value - 0.9).abs() < 1e-4));
        assert!(
            !points.iter().any(|p| p.step == 9),
            "stale point inside pasted span must be removed atomically"
        );
    } else {
        panic!("expected Values");
    }
    app.press_keys("u").unwrap();
    if let PatternKind::Values { points, .. } = &app.pattern(env_id).unwrap().kind {
        assert!(points.contains(&ValuePoint {
            step: 9,
            value: 0.7
        }));
    }

    // Unit mismatch guard: pasting Normalized clipboard into Degrees pattern fails cleanly.
    app.execute_command("pattern degrees Degs 16 0:0 4:2")
        .unwrap();
    let deg_id = app.selected_pattern.unwrap();
    assert!(app.press_keys("P").is_err());

    // 3. `:value-mode` Ex command + UI `mh`/`mr`/`mm` + undo!
    assert!(app.execute_command("value-mode Degs ramp").is_err());

    // Explicit pattern name while focused on another pattern (`Degs`).
    assert_eq!(app.selected_pattern, Some(deg_id));
    app.execute_command("value-mode Env ramp").unwrap();
    assert!(matches!(
        app.pattern(env_id).unwrap().kind,
        PatternKind::Values {
            mode: ValueMode::Ramp,
            ..
        }
    ));
    // Default focused pattern + toggle.
    app.apply(Action::SelectPattern { pattern: env_id })
        .unwrap();
    app.execute_command("value-mode hold").unwrap();
    assert!(matches!(
        app.pattern(env_id).unwrap().kind,
        PatternKind::Values {
            mode: ValueMode::Hold,
            ..
        }
    ));
    app.execute_command("value-mode toggle").unwrap();
    assert!(matches!(
        app.pattern(env_id).unwrap().kind,
        PatternKind::Values {
            mode: ValueMode::Ramp,
            ..
        }
    ));
    // UI fast toggle `mm`, explicit `mh`, `mr`, and undo!
    app.press_keys("mm").unwrap();
    assert!(matches!(
        app.pattern(env_id).unwrap().kind,
        PatternKind::Values {
            mode: ValueMode::Hold,
            ..
        }
    ));
    app.press_keys("mr").unwrap();
    assert!(matches!(
        app.pattern(env_id).unwrap().kind,
        PatternKind::Values {
            mode: ValueMode::Ramp,
            ..
        }
    ));
    app.press_keys("u").unwrap();
    assert!(matches!(
        app.pattern(env_id).unwrap().kind,
        PatternKind::Values {
            mode: ValueMode::Hold,
            ..
        }
    ));
}

#[test]
fn mixer_workflows_channel_effect_send_duck_and_case_insensitive_delete() {
    let mut app = demo_app();
    app.press_keys("gm").unwrap();

    // 1. Create Group & Return channels from Mixer (`c`) and Audio channel via `n`→`c`.
    app.press_keys("cgDrumBus<Enter><Enter>").unwrap();
    let group_id = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "DrumBus" && c.kind == ChannelKind::Group)
        .expect("DrumBus group created")
        .id;
    app.press_keys("gm").unwrap();
    app.press_keys("crHallRet<Enter>").unwrap();
    let ret_id = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "HallRet" && c.kind == ChannelKind::Return)
        .expect("HallRet return created")
        .id;
    app.press_keys("gm").unwrap();
    app.press_keys("ncuExtraCh<Enter><Enter>").unwrap();
    let extra_ch_id = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "ExtraCh" && c.kind == ChannelKind::Audio)
        .expect("ExtraCh audio created")
        .id;

    // 2. Rename channel via UI `r` (`WorkflowKind::RenameChannel`) and via `:channel rename`.
    app.press_keys("rPercCh<Enter>").unwrap();
    assert_eq!(app.channel(extra_ch_id).unwrap().name, "PercCh");
    app.execute_command("channel rename PercCh AuxAudio")
        .unwrap();
    assert_eq!(app.channel(extra_ch_id).unwrap().name, "AuxAudio");

    // 3. Add all 8 EffectKinds via UI `f` (`WorkflowKind::Effect`) across channels & verify Return auto-100% Mix.
    app.apply(Action::SelectChannel {
        channel: ChannelRef::Channel(ret_id),
    })
    .unwrap();
    app.press_keys("fr<Enter>").unwrap(); // Reverb on HallRet
    let rev_id = app.channel(ret_id).unwrap().inserts[0].id;
    assert!(
        (app.effect(rev_id)
            .unwrap()
            .1
            .read_normalized(EffectParam::Mix)
            .unwrap()
            - 1.0)
            .abs()
            < 1e-4,
        "Return insert must initialize Mix=100%"
    );

    // Add remaining 7 FX kinds across group_id (`e`/`f`/`c`/`l`) and extra_ch_id (`d`/`t`/`h`) via UI `f`.
    app.apply(Action::SelectChannel {
        channel: ChannelRef::Channel(group_id),
    })
    .unwrap();
    for key in ['e', 'f', 'c', 'l'] {
        app.press_keys(&format!("f{key}<Enter>")).unwrap();
        app.apply(Action::SelectChannel {
            channel: ChannelRef::Channel(group_id),
        })
        .unwrap();
    }
    assert_eq!(app.channel(group_id).unwrap().inserts.len(), 4);

    app.apply(Action::SelectChannel {
        channel: ChannelRef::Channel(extra_ch_id),
    })
    .unwrap();
    for key in ['d', 't', 'h'] {
        app.press_keys(&format!("f{key}<Enter>")).unwrap();
        app.apply(Action::SelectChannel {
            channel: ChannelRef::Channel(extra_ch_id),
        })
        .unwrap();
    }
    assert_eq!(app.channel(extra_ch_id).unwrap().inserts.len(), 3);

    // 4. Reorder Effect within channel via `J`/`K` inside EDIT in Detail::Channel (cursor tracks moved effect!).
    let ids_before: Vec<u64> = app
        .channel(extra_ch_id)
        .unwrap()
        .inserts
        .iter()
        .map(|d| d.id)
        .collect();
    let fx0 = ids_before[0];
    let fx0_ctrl_idx = app
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::Effect(id) if id == fx0))
        .unwrap();
    app.control_cursor = fx0_ctrl_idx;
    app.press_keys("<m-j>").unwrap();
    assert_eq!(app.channel(extra_ch_id).unwrap().inserts[1].id, fx0);
    assert!(matches!(
        app.control_descriptors()[app.control_cursor].target,
        ControlTarget::Effect(id) if id == fx0
    ));
    app.press_keys("<m-k>").unwrap();
    assert_eq!(app.channel(extra_ch_id).unwrap().inserts[0].id, fx0);

    // 5. Move Effect across channels via `v` (`WorkflowKind::MoveEffect`) AND via `Detail::Effect` `EffectChannel`!
    app.press_keys("v/Master<Enter><Enter>1<Enter>").unwrap();
    assert!(app
        .project()
        .mixer
        .master
        .inserts
        .iter()
        .any(|d| d.id == fx0));
    assert!(!app
        .channel(extra_ch_id)
        .unwrap()
        .inserts
        .iter()
        .any(|d| d.id == fx0));

    // Open Detail::Effect(fx0) and cycle `ControlTarget::EffectChannel` inside EDIT + delete via `X`.
    app.execute_command(&format!("fx {fx0}")).unwrap();
    assert_eq!(app.detail, Detail::Effect(fx0));
    let ch_ctrl = app
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::EffectChannel(id) if id == fx0))
        .expect("EffectChannel control in Detail::Effect");
    app.control_cursor = ch_ctrl;
    let owner_before = app.effect(fx0).unwrap().0;
    app.press_keys("<m-l>").unwrap();
    assert_ne!(app.effect(fx0).unwrap().0, owner_before);

    // Attach a modulation to fx0, then press `X` in Detail::Effect — must prune modulation cleanly!
    let val_pat = app
        .project()
        .patterns
        .iter()
        .find(|p| {
            app.project().pattern_type(p.id).is_ok_and(|t| {
                t == cadence_core::material::PatternType::Values(ValueUnit::Normalized)
            })
        })
        .unwrap()
        .id;
    app.apply(Action::CreateModulation {
        name: "FxDriveMod".into(),
        target: ModulationTarget::Effect {
            device: fx0,
            parameter: EffectParam::Drive,
        },
        pattern: val_pat,
    })
    .unwrap();
    app.press_keys("X").unwrap();
    assert_eq!(app.mode_name(), "CONFIRM");
    app.press_keys("y").unwrap();
    assert!(app.effect(fx0).is_none());
    assert!(!app
        .project()
        .modulations
        .iter()
        .any(|m| m.name == "FxDriveMod"));

    // 6. On Detail::Channel, `w` jumps control groups while `a` opens the Mixer Add picker (`2` -> Send) + delete Send via `x` (even when modulated!).
    app.apply(Action::SelectChannel {
        channel: ChannelRef::Channel(extra_ch_id),
    })
    .unwrap();
    app.control_cursor = 0;
    app.press_keys("w").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert!(
        app.control_cursor > 0,
        "w on Detail::Channel must jump to next control group instead of opening Send workflow"
    );
    app.press_keys("a2<Enter>/HallRet<Enter><Enter>45%<Enter>")
        .unwrap();
    assert!(app
        .channel(extra_ch_id)
        .unwrap()
        .sends
        .iter()
        .any(|s| s.target == ret_id && (s.amount - 0.45).abs() < 1e-3));
    app.apply(Action::CreateModulation {
        name: "SendMod".into(),
        target: ModulationTarget::Send {
            channel: ChannelRef::Channel(extra_ch_id),
            target: ret_id,
        },
        pattern: val_pat,
    })
    .unwrap();
    let send_ctrl = app
        .control_descriptors()
        .iter()
        .position(|c| {
            matches!(
                c.target,
                ControlTarget::Param(ParamAddress::Send {
                    channel: ChannelRef::Channel(ch),
                    target,
                }) if ch == extra_ch_id && target == ret_id
            )
        })
        .expect("Send control on AuxAudio");
    app.control_cursor = send_ctrl;
    app.press_keys("x").unwrap();
    assert!(app.channel(extra_ch_id).unwrap().sends.is_empty());
    assert!(!app
        .project()
        .modulations
        .iter()
        .any(|m| m.name == "SendMod"));

    // 7. Duck workflow (`d` -> WorkflowKind::Duck), coalesced nudges, `r` EditDuck, and `x` delete!
    app.press_keys("dSidePump<Enter><Enter><Enter>60%<Enter>150<Enter>")
        .unwrap();
    let duck_id = app
        .project()
        .ducks
        .iter()
        .find(|d| d.name == "SidePump")
        .expect("SidePump duck created")
        .id;
    let depth_idx = app
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::DuckDepth(id) if id == duck_id))
        .expect("DuckDepth control");
    app.control_cursor = depth_idx;
    let rev_before_duck_nudge = app.revision();
    app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::ALT))
        .unwrap();
    app.handle_key(KeyEvent::new_with_kind(
        KeyCode::Char('l'),
        KeyModifiers::ALT,
        KeyEventKind::Repeat,
    ))
    .unwrap();
    app.handle_key(KeyEvent::new_with_kind(
        KeyCode::Char('l'),
        KeyModifiers::ALT,
        KeyEventKind::Repeat,
    ))
    .unwrap();
    let depth_after = app
        .project()
        .ducks
        .iter()
        .find(|d| d.id == duck_id)
        .unwrap()
        .depth;
    assert!((depth_after - 0.63).abs() < 1e-3);
    app.press_keys("u").unwrap();
    let depth_undone = app
        .project()
        .ducks
        .iter()
        .find(|d| d.id == duck_id)
        .unwrap()
        .depth;
    assert!(
        (depth_undone - 0.60).abs() < 1e-3,
        "coalesced DuckDepth nudges must revert in ONE undo (rev before {rev_before_duck_nudge})"
    );
    // Edit duck in place via `r` (`WorkflowKind::EditDuck`).
    app.press_keys("rDeepPump<Enter><Enter><Enter>80%<Enter>240<Enter>")
        .unwrap();
    let edited_duck = app
        .project()
        .ducks
        .iter()
        .find(|d| d.id == duck_id)
        .unwrap();
    assert_eq!(edited_duck.name, "DeepPump");
    assert!((edited_duck.depth - 0.80).abs() < 1e-3);
    assert!((edited_duck.recovery_ms - 240.0).abs() < 1e-2);

    // 8. Safe Channel delete with confirmation + atomic reroute/cleanup + undo!
    app.apply(Action::SetChannelOutput {
        channel: extra_ch_id,
        output: ChannelRef::Channel(group_id),
    })
    .unwrap();
    app.apply(Action::SelectChannel {
        channel: ChannelRef::Channel(group_id),
    })
    .unwrap();
    app.press_keys("X").unwrap();
    assert!(
        app.confirm_quit(),
        "deleting a channel with inserts & routed child must open Mode::Confirm"
    );
    app.press_keys("y").unwrap();
    assert!(app.channel(group_id).is_none());
    assert_eq!(
        app.channel(extra_ch_id).unwrap().output,
        ChannelRef::Master,
        "child channel output must be rerouted to Master atomically"
    );
    app.press_keys("u").unwrap();
    assert!(app.channel(group_id).is_some());
    assert_eq!(
        app.channel(extra_ch_id).unwrap().output,
        ChannelRef::Channel(group_id)
    );

    // 9. Case-insensitive `:delete` target kinds.
    app.execute_command("delete DUCK DeepPump").unwrap();
    assert!(!app.project().ducks.iter().any(|d| d.id == duck_id));
    app.execute_command("delete CHANNEL DrumBus").unwrap();
    assert!(app.channel(group_id).is_none());
}

#[test]
fn section_part_placement_and_live_timing_cycle_workflows() {
    let mut app = demo_app();

    // 1. Section UI workflows: extend song so bar 9 has no pre-existing demo section,
    // then create via `n→t`, edit via `r` in Arrange, delete via `X` + Undo!
    app.execute_command("bars 16").unwrap();
    app.press_keys("ga").unwrap();
    let sec_count_before = app.project().sections.len();
    app.press_keys("ntBridge<Enter>9<Enter>4<Enter>").unwrap();
    assert_eq!(app.project().sections.len(), sec_count_before + 1);
    let bridge = app
        .project()
        .sections
        .iter()
        .find(|s| s.name == "Bridge")
        .expect("Bridge section created")
        .clone();
    assert_eq!(bridge.start_step, 8 * 16);
    assert_eq!(bridge.length_steps, 4 * 16);
    // Arrange cursor is now at Bridge's start_step; edit via `r` (`WorkflowKind::EditSection`).
    app.arrange_ui.step = bridge.start_step;
    app.press_keys("rBridgeOut<Enter>10<Enter>2<Enter>")
        .unwrap();
    let edited = app
        .project()
        .sections
        .iter()
        .find(|s| s.id == bridge.id)
        .unwrap();
    assert_eq!(edited.name, "BridgeOut");
    assert_eq!(edited.start_step, 9 * 16);
    assert_eq!(edited.length_steps, 2 * 16);
    // Delete via `X` in Arrange (Confirm guarded) + undo!
    app.arrange_ui.step = edited.start_step;
    app.press_keys("X").unwrap();
    assert_eq!(app.mode_name(), "CONFIRM");
    app.press_keys("y").unwrap();
    assert!(!app.project().sections.iter().any(|s| s.id == bridge.id));
    app.press_keys("u").unwrap();
    assert!(app.project().sections.iter().any(|s| s.id == bridge.id));

    // 2. Part creation exposes `transpose` and `velocity_scale` in drafts (`nb`).
    app.press_keys("nbSubPart<Enter><Enter><Enter>-12<Enter>75%<Enter>")
        .unwrap();
    let sub_part = app
        .project()
        .parts
        .iter()
        .find(|p| p.name == "SubPart")
        .expect("SubPart created");
    assert_eq!(sub_part.transpose, -12);
    assert!((sub_part.velocity_scale - 0.75).abs() < 1e-4);

    // 3. Placement `phase` & `offset_steps` direct editing in `Detail::Placement` with undo coalescing!
    let pl_id = app.project().lanes[0].placements[0].id;
    app.apply(Action::SelectPlacement { placement: pl_id })
        .unwrap();
    assert_eq!(app.detail, Detail::Placement(pl_id));
    let phase_idx = app
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::PlacementPhase(id) if id == pl_id))
        .expect("PlacementPhase control");
    let offset_idx = app
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::PlacementOffset(id) if id == pl_id))
        .expect("PlacementOffset control");

    let phase_before = app.placement(pl_id).unwrap().1.phase;
    app.control_cursor = phase_idx;
    app.press_keys("<m-l>").unwrap();
    assert_ne!(app.placement(pl_id).unwrap().1.phase, phase_before);

    let offset_before = app.placement(pl_id).unwrap().1.offset_steps;
    app.control_cursor = offset_idx;
    app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::ALT))
        .unwrap();
    app.handle_key(KeyEvent::new_with_kind(
        KeyCode::Char('l'),
        KeyModifiers::ALT,
        KeyEventKind::Repeat,
    ))
    .unwrap();
    app.handle_key(KeyEvent::new_with_kind(
        KeyCode::Char('l'),
        KeyModifiers::ALT,
        KeyEventKind::Repeat,
    ))
    .unwrap();
    assert_eq!(
        app.placement(pl_id).unwrap().1.offset_steps,
        offset_before + 3
    );
    app.press_keys("u").unwrap();
    assert_eq!(
        app.placement(pl_id).unwrap().1.offset_steps,
        offset_before,
        "coalesced PlacementOffset nudges must revert in ONE undo"
    );
    // Enter on PlacementOffset inside EDIT opens unified ValueInput for signed step offset.
    app.press_keys("<Enter>").unwrap();
    assert_eq!(app.mode_name(), "VALUE ENTRY");
    assert!(app
        .value_input_view()
        .unwrap()
        .subtitle
        .contains("PLACEMENT"));
    app.press_keys("<Esc>").unwrap();

    // 4. LiveTiming / Boundary::Cycle selector (`tq`, `tQ`, `tE`, Settings `g,`) + Chrome projection!
    let first_pat_id = app.project().patterns[0].id;
    let first_pat_name = app.project().patterns[0].name.clone();
    assert_eq!(app.project().timing.launch, Boundary::Bar);
    app.press_keys("tq").unwrap();
    assert_eq!(app.project().timing.launch, Boundary::Bars { count: 2 });

    // `tQ` opens Cycle pattern picker -> Enter selects pattern 0!
    app.press_keys("tQ").unwrap();
    assert!(app.picker_view().is_some());
    app.press_keys("<Enter>").unwrap();
    assert_eq!(
        app.project().timing.launch,
        Boundary::Cycle {
            pattern: first_pat_id
        }
    );

    // `tE` toggles Live Edit Timing (`Now` -> `Launch`).
    assert_eq!(app.project().timing.edits, EditTiming::Now);
    app.press_keys("tE").unwrap();
    assert_eq!(app.project().timing.edits, EditTiming::Launch);

    // ChromeView projects LiveTiming label & flags cleanly.
    let fv = app.frame_view(&AudioSnapshot::default(), "silent");
    assert_eq!(
        fv.chrome.live_timing_label,
        format!("cycle {first_pat_name}")
    );
    assert!(fv.chrome.live_timing_cycle);
    assert!(fv.chrome.live_edits_on_launch);

    // Settings overlay (`g,`) exposes Contextual Key Hints (2), Monitoring rows (3..=6),
    // Auto-Record Takes (8), Default Scale (11), and LiveTiming rows 12, 13, 14 without
    // marking global `config_dirty`!
    app.settings_ui.config_dirty = false;
    app.press_keys("g,").unwrap();
    let fv_settings = app.frame_view(&AudioSnapshot::default(), "silent");
    let Some(OverlayView::Settings(sv)) = fv_settings.overlay else {
        panic!("expected Settings overlay");
    };
    assert_eq!(sv.rows.len(), 17);
    assert_eq!(sv.rows[2].group, "WORKSPACE");
    assert_eq!(sv.rows[2].label, "Contextual Key Hints");
    assert_eq!(sv.rows[3].group, "MONITORING");
    assert_eq!(sv.rows[3].label, "Output Monitor");
    assert_eq!(sv.rows[4].label, "Top-Bar DSP CPU");
    assert_eq!(sv.rows[5].label, "Top-Bar Stereo Meter");
    assert_eq!(sv.rows[6].label, "Arrange Lane Meters");
    assert_eq!(sv.rows[8].group, "TRANSPORT");
    assert_eq!(sv.rows[8].label, "Auto-Record Takes");
    assert_eq!(sv.rows[11].group, "NEW PROJECT");
    assert_eq!(sv.rows[11].label, "Default Scale");
    assert_eq!(sv.rows[12].group, "LIVE TIMING (PROJECT)");
    assert_eq!(sv.rows[12].label, "Launch Quantize");
    assert_eq!(sv.rows[13].label, "Live Edit Timing");
    assert_eq!(sv.rows[14].label, "Late Tolerance");
    assert_eq!(sv.rows[15].group, "ASSET DIRECTORIES");
    assert_eq!(sv.rows[16].group, "ASSET DIRECTORIES");

    // Navigate to row 14 (Late Tolerance) and nudge +1 -> 2 steps directly via Alt+l!
    app.press_keys(&format!("{}<m-l>", "j".repeat(14))).unwrap();
    assert_eq!(app.project().timing.late_tolerance_steps, 2);
    assert!(
        !app.settings_ui.config_dirty,
        "Project LiveTiming edits in Settings must NOT dirty global config.json"
    );
    app.press_keys("<Esc>").unwrap();
}

#[test]
fn mini_overwrite_guard_seeded_degrees_view_and_resize_patterns() {
    let mut app = empty_app();

    // 1. `:mini` rejects overwriting an existing non-Mini pattern.
    app.execute_command("pattern notes KeepMe 0 2 4 7").unwrap();
    assert!(
        app.execute_command("mini KeepMe c4 e4 g4 b4").is_err(),
        ":mini must refuse to overwrite an existing Notes pattern"
    );
    app.execute_command("mini ArpCode 8 c4 e4 g4 b4").unwrap();
    app.execute_command("mini ArpCode 16 c4 e4 g4 b4").unwrap();
    let mini_id = app
        .project()
        .patterns
        .iter()
        .find(|p| p.name == "ArpCode")
        .unwrap()
        .id;

    // 2. Edit::ResizePattern (`:len`) works on Mini and Euclidean (clamping pulses/rotation).
    app.selected_pattern = Some(mini_id);
    app.execute_command("len 32").unwrap();
    assert_eq!(
        app.pattern(mini_id).unwrap().kind,
        PatternKind::Mini {
            code: "c4 e4 g4 b4".into(),
            steps: 32,
        }
    );

    app.execute_command("pattern euclid EucBeat 11 16 12")
        .unwrap();
    let euc_id = app
        .project()
        .patterns
        .iter()
        .find(|p| p.name == "EucBeat")
        .unwrap()
        .id;
    app.selected_pattern = Some(euc_id);
    app.execute_command("len 8").unwrap();
    assert_eq!(
        app.pattern(euc_id).unwrap().kind,
        PatternKind::Euclidean {
            pulses: 8,
            steps: 8,
            rotation: 7,
        }
    );

    // 3. PatternDeckView projects SeededValues(Degrees) and derived Values(Degrees) as Degrees.
    app.apply(Action::CreatePattern {
        name: "SeedDeg".into(),
        kind: PatternKind::SeededValues {
            unit: ValueUnit::Degrees,
            period: 16,
            seed: 42,
            min: -4.0,
            max: 4.0,
        },
    })
    .unwrap();
    let seed_id = app
        .project()
        .patterns
        .iter()
        .find(|p| p.name == "SeedDeg")
        .unwrap()
        .id;
    app.selected_pattern = Some(seed_id);
    let pview = pattern_view(&mut app);
    assert!(!pview.editable_literal);
    assert!(!pview.is_placement_scope);
    match pview.content {
        PatternContentView::Values {
            unit,
            mode,
            cursor_authored,
            ..
        } => {
            assert_eq!(unit, ValueUnit::Degrees);
            assert_eq!(mode, ValueMode::Hold);
            assert!(!cursor_authored);
        }
        other => panic!("expected Values view for SeededValues(Degrees), got {other:?}"),
    }

    app.apply(Action::CreatePattern {
        name: "ShiftedDeg".into(),
        kind: PatternKind::Shift {
            source: seed_id,
            steps: 2,
        },
    })
    .unwrap();
    let shifted_id = app
        .project()
        .patterns
        .iter()
        .find(|p| p.name == "ShiftedDeg")
        .unwrap()
        .id;
    app.selected_pattern = Some(shifted_id);
    let pview2 = pattern_view(&mut app);
    assert!(!pview2.editable_literal);
    assert!(!pview2.is_placement_scope);
    match pview2.content {
        PatternContentView::Values {
            unit,
            cursor_authored,
            ..
        } => {
            assert_eq!(unit, ValueUnit::Degrees);
            assert!(!cursor_authored);
        }
        other => panic!("expected Values view for shifted Degrees, got {other:?}"),
    }
}

#[test]
fn typed_mixer_actions_delete_fx_or_macro_and_in_place_duck_command() {
    let mut app = demo_app();

    // 1. Typed mixer Actions: CreateChannel, RouteInstrument, MoveEffect (JSON Request), DeleteEffect, and `:delete fx|macro`.
    app.apply(Action::CreateChannel {
        name: "SubBus".into(),
        kind: ChannelKind::Group,
    })
    .unwrap();
    app.apply(Action::CreateChannel {
        name: "AltAudio".into(),
        kind: ChannelKind::Audio,
    })
    .unwrap();
    let bus_id = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "SubBus")
        .unwrap()
        .id;
    let alt_audio_id = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "AltAudio")
        .unwrap()
        .id;
    let inst_id = app.project().instruments[0].id;
    app.apply(Action::RouteInstrument {
        instrument: inst_id,
        channel: alt_audio_id,
    })
    .unwrap();
    assert_eq!(app.instrument(inst_id).unwrap().channel, alt_audio_id);

    app.execute_command("fx add drive master").unwrap();
    let fx_id = app.project().mixer.master.inserts.last().unwrap().id;
    let (reply, _) = app.execute_request(Request {
        id: Some("mv".into()),
        expected_revision: None,
        compact: false,
        action: Action::MoveEffect {
            device: fx_id,
            channel: ChannelRef::Channel(bus_id),
            position: 0,
        },
    });
    assert_eq!(reply["ok"], true);
    assert!(app
        .channel(bus_id)
        .unwrap()
        .inserts
        .iter()
        .any(|d| d.id == fx_id));

    app.execute_command(&format!("delete fx {fx_id}")).unwrap();
    assert!(app.effect(fx_id).is_none());
    app.apply(Action::Undo).unwrap();
    assert!(app.effect(fx_id).is_some());

    app.apply(Action::DeleteEffect { device: fx_id }).unwrap();
    assert!(app.effect(fx_id).is_none());

    app.execute_command("macro 1 name FilterSweep").unwrap();
    assert!(app.project().macro_control(0).is_some());
    app.execute_command("delete macro 1").unwrap();
    assert!(app.project().macro_control(0).is_none());

    // 2. `:duck` edits existing duck in-place when matching `(source, target)` or name.
    let src_name = app.project().parts[0].name.clone();
    let dst_name = app.project().mixer.channels[1].name.clone();
    let ducks_before = app.project().ducks.len();
    app.execute_command(&format!("duck PumpA {src_name} {dst_name} 0.5 120"))
        .unwrap();
    assert_eq!(app.project().ducks.len(), ducks_before + 1);
    let duck_id = app.project().ducks.last().unwrap().id;
    app.execute_command(&format!("duck PumpA {src_name} {dst_name} 0.85 200"))
        .unwrap();
    assert_eq!(app.project().ducks.len(), ducks_before + 1);
    let updated_duck = app
        .project()
        .ducks
        .iter()
        .find(|d| d.id == duck_id)
        .unwrap();
    assert!((updated_duck.depth - 0.85).abs() < 1e-4);
    assert!((updated_duck.recovery_ms - 200.0).abs() < 1e-4);
}

#[test]
fn clip_meter_clearing_via_c_tc_and_palette_plus_count_row_status() {
    let mut app = demo_app();
    app.press_keys("gm").unwrap();
    let ch0 = app.selected_channel;

    // 1. `C` in Mixer clears focused channel's peak/clip hold.
    let out = app.press_keys("C").unwrap();
    assert_eq!(out.last().unwrap().clear_mixer_meters, Some(Some(ch0)));

    // 2. Count prefix in Mixer advertises `j/k row` (matching the vertical detail stack).
    app.press_keys("3").unwrap();
    assert!(
        app.status.contains("j/k row"),
        "expected 'j/k row' in Mixer count status, got: {}",
        app.status
    );
    app.press_keys("<Esc>").unwrap();

    // 3. `C` in `Detail::Channel` clears that channel's peak/clip hold.
    app.press_keys("o").unwrap();
    assert_eq!(app.detail, Detail::Channel(ch0));
    let out_detail = app.press_keys("C").unwrap();
    assert_eq!(
        out_detail.last().unwrap().clear_mixer_meters,
        Some(Some(ch0))
    );

    // 4. `tC` (Transport prefix) globally clears all clipped meters from any deck.
    app.press_keys("ga").unwrap();
    let out_global = app.press_keys("tC").unwrap();
    assert_eq!(out_global.last().unwrap().clear_mixer_meters, Some(None));

    // 5. Fuzzy Action Palette exposes "Clear Clipped Meters" and executes global clear.
    app.press_keys(":Clear Clipped Meters").unwrap();
    let pv = app.palette_view().expect("palette open");
    assert!(
        pv.items.iter().any(|i| i.label == "Clear Clipped Meters"),
        "Palette must include 'Clear Clipped Meters'"
    );
    let out_pal = app.press_keys("<Enter>").unwrap();
    assert_eq!(out_pal.last().unwrap().clear_mixer_meters, Some(None));
}

#[test]
fn effect_rename_action_shortcuts_and_commands() {
    let mut app = demo_app();
    let ch0_id = app.project().mixer.channels[0].id;
    let ch0 = ChannelRef::Channel(ch0_id);
    app.apply(Action::SelectChannel { channel: ch0 }).unwrap();
    app.execute_command("fx add eq").unwrap();
    let fx_id = app.channel(ch0_id).unwrap().inserts.last().unwrap().id;
    let orig_name = app.effect(fx_id).unwrap().1.name.clone();

    // 1. Direct Action::RenameEffect + Undo / Redo + validation.
    app.apply(Action::RenameEffect {
        device: fx_id,
        name: "  Vocal Sculpt  ".into(),
    })
    .unwrap();
    assert_eq!(app.effect(fx_id).unwrap().1.name, "Vocal Sculpt");
    app.apply(Action::Undo).unwrap();
    assert_eq!(app.effect(fx_id).unwrap().1.name, orig_name);
    app.apply(Action::Redo).unwrap();
    assert_eq!(app.effect(fx_id).unwrap().1.name, "Vocal Sculpt");
    assert!(app
        .apply(Action::RenameEffect {
            device: fx_id,
            name: "   ".into(),
        })
        .is_err());
    assert!(app
        .apply(Action::RenameEffect {
            device: fx_id,
            name: "x".repeat(81),
        })
        .is_err());

    // 2. `:rename fx`, `:rename effect`, and `:fx rename`.
    app.execute_command(&format!("rename fx {fx_id} Air Shelf"))
        .unwrap();
    assert_eq!(app.effect(fx_id).unwrap().1.name, "Air Shelf");
    app.execute_command(&format!("rename effect {fx_id} Mid Bite"))
        .unwrap();
    assert_eq!(app.effect(fx_id).unwrap().1.name, "Mid Bite");
    app.execute_command(&format!("fx rename {fx_id} Tone Box"))
        .unwrap();
    assert_eq!(app.effect(fx_id).unwrap().1.name, "Tone Box");

    // 3. `r` in Detail::Effect opens WorkflowKind::RenameEffect.
    app.execute_command(&format!("fx {fx_id}")).unwrap();
    app.press_keys("rWarm EQ<Enter>").unwrap();
    assert_eq!(app.effect(fx_id).unwrap().1.name, "Warm EQ");

    // 4. `r` on focused MixerSlot::Insert in gm renames that effect.
    app.apply(Action::SelectChannel { channel: ch0 }).unwrap();
    app.press_keys("gm").unwrap();
    let insert_idx = app
        .channel(ch0_id)
        .unwrap()
        .inserts
        .iter()
        .position(|d| d.id == fx_id)
        .unwrap();
    app.view_state.mixer_slot = cadence_app::MixerSlot::Insert(insert_idx);
    app.press_keys("rPunch EQ<Enter>").unwrap();
    assert_eq!(app.effect(fx_id).unwrap().1.name, "Punch EQ");

    // 5. `r` on focused ControlTarget::Effect in Detail::Channel renames that effect.
    app.press_keys("o").unwrap();
    let fx_row = app
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::Effect(id) if id == fx_id))
        .expect("Effect row in Detail::Channel");
    app.control_cursor = fx_row;
    app.press_keys("rFinal EQ<Enter>").unwrap();
    assert_eq!(app.effect(fx_id).unwrap().1.name, "Final EQ");
}

#[test]
fn effect_bypass_and_move_symmetry_across_mixer_and_channel_detail() {
    let mut app = demo_app();
    let ch0_id = app.project().mixer.channels[0].id;
    let ch0 = ChannelRef::Channel(ch0_id);
    app.apply(Action::SelectChannel { channel: ch0 }).unwrap();
    app.execute_command("fx add drive").unwrap();
    let fx_id = app.channel(ch0_id).unwrap().inserts.last().unwrap().id;
    let insert_idx = app.channel(ch0_id).unwrap().inserts.len() - 1;

    // 1. Mixer `gm`: `b` toggles bypass (and ignores repeat); `v` opens MoveEffect.
    app.apply(Action::SelectChannel { channel: ch0 }).unwrap();
    app.press_keys("gm").unwrap();
    app.view_state.mixer_slot = cadence_app::MixerSlot::Insert(insert_idx);
    assert!(!app.effect(fx_id).unwrap().1.bypassed);
    app.press_keys("b").unwrap();
    assert!(app.effect(fx_id).unwrap().1.bypassed);
    app.handle_key(KeyEvent::new_with_kind(
        KeyCode::Char('b'),
        KeyModifiers::NONE,
        KeyEventKind::Repeat,
    ))
    .unwrap();
    assert!(
        app.effect(fx_id).unwrap().1.bypassed,
        "key repeat on `b` must not flutter bypass state"
    );
    app.press_keys("v").unwrap();
    assert_eq!(app.mode_name(), "WORKFLOW");
    app.press_keys("<Esc>").unwrap();

    // 2. Detail::Channel: `b` toggles bypass on focused ControlTarget::Effect, `v` opens MoveEffect,
    //    and `b` still jumps control group backward on non-Effect rows!
    app.press_keys("o").unwrap();
    assert_eq!(app.detail, Detail::Channel(ch0));
    let fx_row = app
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::Effect(id) if id == fx_id))
        .expect("Effect row");
    app.control_cursor = fx_row;
    app.press_keys("b").unwrap();
    assert!(!app.effect(fx_id).unwrap().1.bypassed);
    app.press_keys("v").unwrap();
    assert_eq!(app.mode_name(), "WORKFLOW");
    app.press_keys("<Esc>").unwrap();

    // Verify `b` jumps backward on non-Effect controls (e.g. from ROUTING -> STRIP).
    let routing_row = app
        .control_descriptors()
        .iter()
        .position(|c| c.group == "ROUTING")
        .expect("ROUTING row");
    app.control_cursor = routing_row;
    app.press_keys("b").unwrap();
    assert_eq!(app.control_descriptors()[app.control_cursor].group, "STRIP");
}

#[test]
fn synthetic_macro_and_mod_sends_are_focusable_learnable_and_materialize() {
    use cadence_core::macros::{MacroBinding, MacroControl};
    use cadence_core::material::{
        Modulation, ModulationTarget, Pattern, PatternKind, ValuePoint, ValueUnit,
    };
    use cadence_core::midi_map::MidiTarget;
    use cadence_core::params::ValueMode;

    let mut project = Project::demo();
    let ch0_id = project.mixer.channels[0].id;
    let ch0_ref = ChannelRef::Channel(ch0_id);
    project.mixer.channels[0].sends.clear();

    // Create two fresh Return channels with NO explicit ChannelSend on ch0.
    let ret_macro_id = project.max_id() + 1;
    let ret_mod_id = project.max_id() + 2;
    project
        .mixer
        .channels
        .push(cadence_core::mixer::Channel::new(
            ret_macro_id,
            ChannelKind::Return,
            "MacroVerb",
        ));
    project
        .mixer
        .channels
        .push(cadence_core::mixer::Channel::new(
            ret_mod_id,
            ChannelKind::Return,
            "ModDelay",
        ));

    // Bind Macro 1 to ch0 -> MacroVerb (synthetic Macro-only send).
    project.macros.push(MacroControl {
        index: 0,
        name: "VerbSend".into(),
        value: 0.4,
        bindings: vec![MacroBinding::new(
            ParamAddress::Send {
                channel: ch0_ref,
                target: ret_macro_id,
            },
            0.0,
            0.8,
        )],
    });

    // Bind global Modulation to ch0 -> ModDelay (synthetic Mod-only send).
    let pat_id = project.max_id() + 3;
    project.patterns.push(Pattern {
        id: pat_id,
        name: "DelayThrow".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 16,
            mode: ValueMode::Hold,
            points: vec![ValuePoint {
                step: 0,
                value: 0.5,
            }],
        },
    });
    let mod_id = project.max_id() + 4;
    project.modulations.push(Modulation {
        id: mod_id,
        name: "ThrowMod".into(),
        pattern: pat_id,
        target: ModulationTarget::Send {
            channel: ch0_ref,
            target: ret_mod_id,
        },
    });

    let mut app = App::new(project, "synthetic-sends.json".into()).unwrap();
    app.apply(Action::SelectChannel { channel: ch0_ref })
        .unwrap();
    app.press_keys("gm").unwrap();

    // 1. Both synthetic sends appear in `mixer_slots_for` (Return channel still omits Output).
    let slots = app.mixer_slots_for(ch0_ref);
    assert!(slots.contains(&cadence_app::MixerSlot::Send(0)));
    assert!(slots.contains(&cadence_app::MixerSlot::Send(1)));
    assert!(!app
        .mixer_slots_for(ChannelRef::Channel(ret_macro_id))
        .contains(&cadence_app::MixerSlot::Output));

    // 2. Focus synthetic Mod-only send (index 1) in gm: MIDI-learnable via `M`!
    app.view_state.mixer_slot = cadence_app::MixerSlot::Send(1);
    let learn = app.learn_target_from_focus().expect("MIDI learn target");
    assert_eq!(
        learn.0,
        MidiTarget::Param {
            address: ParamAddress::Send {
                channel: ch0_ref,
                target: ret_mod_id,
            },
        }
    );

    // 3. Nudging the synthetic Mod-only send materializes an explicit ChannelSend AND
    //    re-anchors `mixer_slot` to the newly materialized send (which moved to index 0)!
    assert!(app.channel(ch0_id).unwrap().sends.is_empty());
    app.press_keys(".").unwrap();
    let sends = &app.channel(ch0_id).unwrap().sends;
    assert_eq!(sends.len(), 1);
    assert_eq!(sends[0].target, ret_mod_id);
    assert!((sends[0].amount - 0.01).abs() < 1e-4);
    assert_eq!(
        app.view_state.mixer_slot,
        cadence_app::MixerSlot::Send(0),
        "focus must follow materialized send when it shifts into explicit sends"
    );

    // 4. Open Detail::Channel: synthetic Macro-only send is visible, MIDI-learnable, and materializes via Enter!
    app.press_keys("o").unwrap();
    let macro_send_addr = ParamAddress::Send {
        channel: ch0_ref,
        target: ret_macro_id,
    };
    let send_row = app
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::Param(a) if a == macro_send_addr))
        .expect("Macro-only synthetic send in Detail::Channel");
    app.control_cursor = send_row;
    let detail_learn = app
        .learn_target_from_focus()
        .expect("MIDI learn target in Detail::Channel");
    assert_eq!(
        detail_learn.0,
        MidiTarget::Param {
            address: macro_send_addr
        }
    );
    app.press_keys("<Enter>35%<Enter>").unwrap();
    let materialized = app
        .channel(ch0_id)
        .unwrap()
        .sends
        .iter()
        .find(|s| s.target == ret_macro_id)
        .expect("Macro-only send materialized");
    assert!((materialized.amount - 0.35).abs() < 1e-4);
}

#[test]
fn filter_env_source_picker_includes_long_patterns_and_gs_jumps_to_pattern() {
    use cadence_core::mixer::{EffectKind, FilterPatch};

    let mut app = App::new(Project::empty(), "filter-picker.json".into()).unwrap();
    app.execute_command("pattern degrees HarmDeg 16 hold 0:0 8:2")
        .unwrap();
    app.execute_command("pattern values LongSweep 128 ramp 0:0.1 64:0.9")
        .unwrap();
    let long_id = app
        .project()
        .patterns
        .iter()
        .find(|p| p.name == "LongSweep")
        .unwrap()
        .id;

    app.execute_command("fx add filter master").unwrap();
    let fx_id = app.project().mixer.master.inserts[0].id;
    assert_eq!(app.detail, Detail::Effect(fx_id));

    let src_row = app
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::FilterEnvSource(id) if id == fx_id))
        .expect("FilterEnvSource row");
    app.control_cursor = src_row;

    // 1. Pressing `gs` while on Groove Bank keeps us on Filter with an informative status.
    app.press_keys("gs").unwrap();
    assert_eq!(app.detail, Detail::Effect(fx_id));
    assert!(app.status.contains("Groove Bank"));

    // 2. Pressing `Enter` opens searchable Picker containing LongSweep (period 128) and excluding HarmDeg.
    app.press_keys("<Enter>").unwrap();
    let pv = app.picker_view().expect("Filter env source picker open");
    assert!(pv.items.iter().any(|(name, _, _)| name == "Groove Bank"));
    assert!(
        pv.items.iter().any(|(name, _, _)| name == "LongSweep"),
        "Filter env picker must include period > 64 patterns matching :param source"
    );
    assert!(
        !pv.items.iter().any(|(name, _, _)| name == "HarmDeg"),
        "Filter env picker must exclude Values(Degrees) patterns"
    );

    // Filter with `/Long<Enter>` then select with `<Enter>`.
    app.press_keys("/Long<Enter><Enter>").unwrap();
    let EffectKind::Filter(FilterPatch { env_pattern_id, .. }) = app.effect(fx_id).unwrap().1.kind
    else {
        panic!("expected Filter");
    };
    assert_eq!(env_pattern_id, Some(long_id));

    // 3. Pressing `gs` from Detail::Effect jumps directly to the linked LongSweep pattern!
    app.press_keys("gs").unwrap();
    assert_eq!(app.selected_pattern, Some(long_id));
}

#[test]
fn channel_output_picker_and_duck_exact_value_input_and_r_edit() {
    let mut app = demo_app();
    let ch0_id = app.project().mixer.channels[0].id;
    let ch0_ref = ChannelRef::Channel(ch0_id);
    app.execute_command("channel group SubBus").unwrap();
    let bus_id = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "SubBus")
        .unwrap()
        .id;

    // 1. `Enter` on Detail::Channel Output opens the bus route picker directly.
    app.press_keys("gm").unwrap();
    app.apply(Action::SelectChannel { channel: ch0_ref })
        .unwrap();
    app.press_keys("o").unwrap();
    let out_row = app
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::ChannelOutput(id) if id == ch0_id))
        .expect("ChannelOutput control");
    app.control_cursor = out_row;
    app.press_keys("<Enter>").unwrap();
    let pv = app.picker_view().expect("bus route picker open");
    assert!(pv.items.iter().any(|(name, _, _)| name == "SubBus"));
    app.press_keys("/SubBus<Enter><Enter>").unwrap();
    assert_eq!(
        app.channel(ch0_id).unwrap().output,
        ChannelRef::Channel(bus_id)
    );

    // 2. Create a Duck on ch0 and test exact ValueInput on DuckDepth & DuckRecovery in Detail::Channel.
    let src_name = app.project().parts[0].name.clone();
    let ch0_name = app.channel(ch0_id).unwrap().name.clone();
    app.execute_command(&format!("duck Pump {src_name} {ch0_name} 0.4 100"))
        .unwrap();
    let duck_id = app.project().ducks.last().unwrap().id;

    let depth_row = app
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::DuckDepth(id) if id == duck_id))
        .expect("DuckDepth control");
    app.control_cursor = depth_row;
    app.press_keys("<Enter>72%<Enter>").unwrap();
    let duck = app
        .project()
        .ducks
        .iter()
        .find(|d| d.id == duck_id)
        .unwrap();
    assert!((duck.depth - 0.72).abs() < 1e-4);

    let rec_row = app
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::DuckRecovery(id) if id == duck_id))
        .expect("DuckRecovery control");
    app.control_cursor = rec_row;
    app.press_keys("<Enter>320ms<Enter>").unwrap();
    let duck = app
        .project()
        .ducks
        .iter()
        .find(|d| d.id == duck_id)
        .unwrap();
    assert!((duck.recovery_ms - 320.0).abs() < 1e-3);

    // 3. In Mixer `gm`, `Enter` on MixerSlot::Duck opens exact DuckDepth ValueInput, and `r` opens EditDuck!
    app.apply(Action::SelectChannel { channel: ch0_ref })
        .unwrap();
    app.press_keys("gm").unwrap();
    let duck_idx = app
        .project()
        .ducks
        .iter()
        .filter(|d| d.target == ch0_ref)
        .position(|d| d.id == duck_id)
        .unwrap();
    app.view_state.mixer_slot = cadence_app::MixerSlot::Duck(duck_idx);
    app.press_keys("<Enter>55%<Enter>").unwrap();
    let duck = app
        .project()
        .ducks
        .iter()
        .find(|d| d.id == duck_id)
        .unwrap();
    assert!((duck.depth - 0.55).abs() < 1e-4);

    app.press_keys("rPumpEdited<Enter><Enter><Enter>68%<Enter>210<Enter>")
        .unwrap();
    let duck = app
        .project()
        .ducks
        .iter()
        .find(|d| d.id == duck_id)
        .unwrap();
    assert_eq!(duck.name, "PumpEdited");
    assert!((duck.depth - 0.68).abs() < 1e-4);
    assert!((duck.recovery_ms - 210.0).abs() < 1e-3);
}
