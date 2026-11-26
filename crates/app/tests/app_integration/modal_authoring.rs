use cadence_app::{
    authoring::{DraftField, DraftInput},
    Action, App, EditScope, Focus, Workspace,
};
use cadence_core::{
    material::{Edit, PatternKind, Project, ValuePoint, ValueUnit},
    mixer::ChannelParam,
    params::{ParamAddress, ValueMode},
};

fn app() -> App {
    App::new_in_memory(Project::empty(), "unused.json".into()).unwrap()
}
fn values(app: &App, name: &str) -> Vec<ValuePoint> {
    let PatternKind::Values { points, .. } = &app
        .project()
        .patterns
        .iter()
        .find(|p| p.name == name)
        .unwrap()
        .kind
    else {
        panic!("Expected values");
    };
    points.clone()
}
fn harmony(app: &mut App) {
    app.press_keys(
        "gpndHarmony<Enter>64<Enter>24l<Enter>-2<Enter>8l<Enter>2<Enter>24l<Enter>-1<Enter>",
    )
    .unwrap();
}

#[test]
fn modal_harmony_matches_ex_and_typed_gestures_with_meaningful_undo() {
    let mut keys = app();
    harmony(&mut keys);
    assert_eq!(keys.cursor, 56);
    assert_eq!(keys.value_cursor().unwrap().step, 57);
    assert_eq!(
        keys.revision(),
        4,
        "one creation plus three completed fields"
    );
    let mut ex = app();
    ex.execute_command("pattern degrees Harmony 64 hold 0:0 24:-2 32:2 56:-1")
        .unwrap();
    assert_eq!(keys.project(), ex.project());
    let mut typed = app();
    typed
        .apply(Action::CreatePattern {
            name: "Harmony".into(),
            kind: PatternKind::Values {
                unit: ValueUnit::Degrees,
                period: 64,
                mode: ValueMode::Hold,
                points: vec![ValuePoint {
                    step: 0,
                    value: 0.0,
                }],
            },
        })
        .unwrap();
    let id = typed.selected_pattern.unwrap();
    for (step, value) in [(24, -2.0), (32, 2.0), (56, -1.0)] {
        typed
            .apply(Action::Edit {
                edit: Edit::SetValue {
                    pattern: id,
                    step,
                    value: Some(value),
                },
            })
            .unwrap();
    }
    assert_eq!(keys.project(), typed.project());
    assert_eq!(keys.revision(), typed.revision());
    keys.press_keys("u").unwrap();
    assert_eq!(values(&keys, "Harmony").len(), 3);
    keys.press_keys("U").unwrap();
    assert_eq!(keys.project(), ex.project());
}

#[test]
fn source_pickers_create_moving_atomically_and_edit_preserves_identity() {
    let mut a = app();
    a.press_keys("gpnnMotif<Enter>5<Enter>i13587<Esc>").unwrap();
    let motif = a.selected_pattern.unwrap();
    harmony(&mut a);
    let harmony = a.selected_pattern.unwrap();
    let before = a.project().clone();
    let revision = a.revision();
    a.press_keys("dMoving<Enter>a").unwrap();
    let choice = a.authoring_view().unwrap();
    assert_eq!(choice.input, DraftInput::Reference);
    assert_eq!(choice.field, DraftField::Notes);
    assert!(choice.candidates.iter().any(|c| c.id == Some(motif)));
    assert!(!choice.candidates.iter().any(|c| c.id == Some(harmony)));
    a.press_keys("<Enter>").unwrap();
    assert_eq!(a.project(), &before);
    assert_eq!(a.revision(), revision);
    assert_eq!(a.authoring_view().unwrap().field, DraftField::Degrees);
    a.press_keys("<Enter>").unwrap();
    assert!(a.authoring_view().is_none());
    assert_eq!(a.revision(), revision + 1);
    let moving = a.selected_pattern.unwrap();
    assert_eq!(
        a.project().pattern(moving).unwrap().kind,
        PatternKind::AddDegrees {
            notes: motif,
            values: harmony
        }
    );
    assert_eq!(a.project().pattern_period(moving).unwrap(), 320);
    a.press_keys("rt<Enter>12<Enter>").unwrap();
    assert_eq!(a.project().pattern(moving).unwrap().name, "Moving");
    assert_eq!(
        a.project().pattern(moving).unwrap().kind,
        PatternKind::Transpose {
            source: motif,
            semitones: 12
        }
    );
    a.press_keys("u").unwrap();
    assert_eq!(
        a.project().pattern(moving).unwrap().kind,
        PatternKind::AddDegrees {
            notes: motif,
            values: harmony
        }
    );
}

#[test]
fn cancelling_and_invalid_creation_preserve_score_and_navigation() {
    let mut a = app();
    harmony(&mut a);
    let before = a.project().clone();
    let revision = a.revision();
    let cursor = a.cursor;
    a.press_keys("ndScratch<Enter>0<Enter>").unwrap();
    assert!(a.authoring_view().unwrap().error.is_some());
    assert_eq!(a.project(), &before);
    a.press_keys("<Home>").unwrap();
    assert!(
        a.authoring_view().is_some(),
        "transport must preserve the field"
    );
    a.press_keys("<Esc>").unwrap();
    assert_eq!(a.cursor, cursor);
    assert_eq!(a.workspace(), Workspace::Pattern);
    assert_eq!(a.focus(), Focus::Canvas);
    assert_eq!(a.scope, EditScope::Definition);
    assert_eq!(a.project(), &before);
    assert_eq!(a.revision(), revision);
    a.press_keys("ndDraft<Enter>64<BackTab><C-u>Renamed<Enter>32<Enter>")
        .unwrap();
    let new = a.project().pattern(a.selected_pattern.unwrap()).unwrap();
    assert_eq!(new.name, "Renamed");
    assert!(matches!(new.kind, PatternKind::Values { period: 32, .. }));
}

#[test]
fn value_navigation_never_creates_points_and_entry_errors_remain_editable() {
    let mut a = app();
    harmony(&mut a);
    a.press_keys("gg").unwrap();
    let before = a.project().clone();
    let revision = a.revision();
    a.press_keys("w").unwrap();
    assert_eq!(a.cursor, 24);
    a.press_keys("j").unwrap();
    assert_eq!(a.cursor, 32);
    a.press_keys("k").unwrap();
    assert_eq!(a.cursor, 24);
    a.press_keys("2w").unwrap();
    assert_eq!(a.cursor, 56);
    a.press_keys("2b").unwrap();
    assert_eq!(a.cursor, 24);
    a.press_keys("l").unwrap();
    let view = a.value_cursor().unwrap();
    assert!(!view.authored);
    assert_eq!(view.held_from, 24);
    assert_eq!(view.value, -2.0);
    assert_eq!(a.project(), &before);
    assert_eq!(a.revision(), revision);
    a.press_keys("<Enter>1.5<Enter>").unwrap();
    assert_eq!(a.value_input_view().unwrap().input, "1.5");
    assert!(a.value_input_view().unwrap().error.is_some());
    assert_eq!(a.project(), &before);
    a.press_keys("<C-u>-4<Enter>").unwrap();
    assert!(a.value_input_view().is_none());
    assert_eq!(a.cursor, 25);
    assert_eq!(a.value_cursor().unwrap().value, -4.0);
    assert_eq!(a.revision(), revision + 1);
    a.press_keys("l<Enter>9<Esc>").unwrap();
    assert!(!a.value_cursor().unwrap().authored);
    assert_eq!(a.revision(), revision + 1);
    a.press_keys(";").unwrap();
    assert_eq!(
        a.value_cursor().unwrap().value,
        -4.0,
        "repeat retargets the assigned value"
    );
}

#[test]
fn normalized_point_entry_nudges_interpolation_and_delete_have_explicit_scope() {
    let mut a = app();
    a.press_keys("gpnvSweep<Enter>16<Enter>r8l<Enter>80%<Enter>4h")
        .unwrap();
    let initial = a.project().clone();
    assert!((a.value_cursor().unwrap().value - 0.4).abs() < 1e-6);
    assert!(!a.value_cursor().unwrap().authored);
    let revision = a.revision();
    a.press_keys("<m-k><m-k><m-k>").unwrap();
    assert!((a.value_cursor().unwrap().value - 0.43).abs() < 1e-6);
    assert_eq!(a.revision(), revision + 3);
    a.press_keys("uuu").unwrap();
    assert_eq!(a.project(), &initial);
    a.press_keys("mh").unwrap();
    assert_eq!(a.value_cursor().unwrap().mode, ValueMode::Hold);
    assert_eq!(a.value_cursor().unwrap().value, 0.0);
    a.press_keys("u").unwrap();
    assert_eq!(a.project(), &initial);
    a.press_keys("wx").unwrap();
    assert_eq!(values(&a, "Sweep").len(), 1);
    a.press_keys("gg").unwrap();
    let before = a.project().clone();
    let revision = a.revision();
    assert!(a.press_keys("x").is_err());
    assert_eq!(a.project(), &before);
    assert_eq!(a.revision(), revision);
}

#[test]
fn counts_and_unmodified_workspace_leader_do_not_steal_text_or_mute() {
    let mut a = app();
    a.execute_command("part new Tone init 0").unwrap();
    harmony(&mut a);
    let before = a.project().clone();
    let revision = a.revision();
    a.press_keys("24").unwrap();
    assert_eq!(a.pending_count(), Some(24));
    a.press_keys("<Esc>").unwrap();
    assert_eq!(a.pending_count(), None);
    assert_eq!(a.cursor, 56);
    a.press_keys("m").unwrap();
    assert!(
        !a.flags().lane_muted(a.project().lanes[0].id),
        "degree mode key cannot mute a lane"
    );
    a.press_keys("z").unwrap();
    assert!(a.is_window_prefix());
    a.press_keys("z").unwrap();
    assert!(a.view_state.zoomed);
    a.press_keys("zz").unwrap();
    assert!(!a.view_state.zoomed);
    a.press_keys("ndzmwjk<Enter>16<Enter>").unwrap();
    assert_eq!(
        a.project()
            .pattern(a.selected_pattern.unwrap())
            .unwrap()
            .name,
        "zmwjk"
    );
    assert_eq!(a.revision(), revision + 1);
    a.press_keys("u").unwrap();
    assert_eq!(a.project(), &before);
}

#[test]
fn mixer_strip_keys_use_decibels_and_viewport_pages_without_muting() {
    let mut a = app();
    a.execute_command("part new One init 0").unwrap();
    a.execute_command("part new Two init 0").unwrap();
    a.press_keys("gmgg").unwrap();
    let first = a.selected_channel;
    let before = a.project().clone();
    let revision = a.revision();
    // Direct Tier 3 Mixer Fader: Alt+J drops -3 dB cleanly without muting.
    a.press_keys("<m-J>").unwrap();
    let channel = match first {
        cadence_core::mixer::ChannelRef::Channel(id) => a.project().channel(id).unwrap(),
        _ => panic!(),
    };
    assert!((cadence_core::dsp::lin_to_db(channel.strip.gain) + 3.0).abs() < 1e-4);
    assert!(!a.flags().channel_muted(first));
    assert_eq!(a.revision(), revision + 1);
    a.press_keys("u").unwrap();
    assert_eq!(a.project(), &before);
    a.view_state.mixer_page_size = 2;
    a.press_keys("<PageDown>").unwrap();
    assert_eq!(a.selected_channel, cadence_core::mixer::ChannelRef::Master);
    a.press_keys("<PageUp>").unwrap();
    assert_eq!(a.selected_channel, first);
    let revision = a.revision();
    a.press_keys("lh").unwrap();
    assert_eq!(a.selected_channel, first);
    assert_eq!(a.revision(), revision);
    a.apply(Action::SetParam {
        address: ParamAddress::Channel {
            channel: first,
            param: ChannelParam::Gain,
        },
        value: 0.001,
    })
    .unwrap();
    a.press_keys("<m-j>").unwrap();
    let gain = |a: &App| match first {
        cadence_core::mixer::ChannelRef::Channel(id) => a.project().channel(id).unwrap().strip.gain,
        _ => panic!(),
    };
    assert_eq!(gain(&a), 0.0);
    a.press_keys("<m-k>").unwrap();
    assert!((gain(&a) - 0.001).abs() < 1e-8);
    a.press_keys("<m-m><m-k><m-k>").unwrap();
    assert!((cadence_core::dsp::lin_to_db(gain(&a)) + 58.0).abs() < 1e-4);
    assert!(a.flags().channel_muted(first));
}

#[test]
fn mixer_fader_and_detail_channel_nudge_headroom_and_param_db_parsing() {
    use cadence_core::dsp::{
        channel_gain_to_norm, db_to_lin, lin_to_db, CHANNEL_GAIN_UNITY_NORM, MAX_CHANNEL_GAIN,
    };

    let mut a = app();
    a.execute_command("part new One init 0").unwrap();
    a.press_keys("gmgg").unwrap();
    let ch = a.selected_channel;
    let gain = |a: &App| match ch {
        cadence_core::mixer::ChannelRef::Channel(id) => a.project().channel(id).unwrap().strip.gain,
        _ => panic!("expected audio channel"),
    };

    // 1. Direct Mixer Fader: `Alt+K` (+3 dB) / `Alt+k` (+1 dB) nudge cleanly past unity up to +6.0 dB!
    assert_eq!(gain(&a), 1.0);
    a.press_keys("<m-K><m-k>").unwrap(); // +4.0 dB
    assert!((lin_to_db(gain(&a)) - 4.0).abs() < 1e-4);
    a.press_keys("<m-K><m-K>").unwrap(); // clamps at +6.0 dB
    assert!((lin_to_db(gain(&a)) - 6.0).abs() < 1e-4);
    assert!(gain(&a) <= MAX_CHANNEL_GAIN);

    // Stepping down 6 dB (`Alt+J` x2) snaps back to exact 1.0 unity!
    a.press_keys("<m-J><m-J>").unwrap();
    assert_eq!(gain(&a), 1.0);

    // 2. Open Detail::Channel (`o`), verify Trim control display & 3/4-throw unity normalized bar.
    a.press_keys("o").unwrap();
    let trim_ctrl = &a.control_descriptors()[0];
    assert_eq!(trim_ctrl.label, "Trim");
    assert_eq!(trim_ctrl.display, "+0.0 dB");
    assert_eq!(trim_ctrl.normalized, Some(CHANNEL_GAIN_UNITY_NORM));

    // Direct Tier 3: Fine (`Alt+l`/`Alt+h`: ±1 dB) and coarse (`Alt+L`/`Alt+H`: ±3 dB) nudging up to +6.0 dB and back.
    a.press_keys("<m-L>").unwrap(); // +3.0 dB
    assert!((lin_to_db(gain(&a)) - 3.0).abs() < 1e-4);
    assert_eq!(a.control_descriptors()[0].display, "+3.0 dB");
    a.press_keys("<m-L>").unwrap(); // +6.0 dB
    assert!((lin_to_db(gain(&a)) - 6.0).abs() < 1e-4);
    a.press_keys("<m-l>").unwrap(); // stays clamped at +6.0 dB
    assert!((lin_to_db(gain(&a)) - 6.0).abs() < 1e-4);
    a.press_keys("<m-H>").unwrap(); // +3.0 dB
    a.press_keys("<m-H>").unwrap(); // exact 0.0 dB unity snap
    assert_eq!(gain(&a), 1.0);

    // 3. `:param trim` accepts dB, %, scalar, and `-inf`, and rejects out-of-range values cleanly.
    a.execute_command("param trim +3.5dB").unwrap();
    assert!((gain(&a) - db_to_lin(3.5)).abs() < 1e-5);
    assert_eq!(a.control_descriptors()[0].display, "+3.5 dB");
    assert!(
        (a.control_descriptors()[0].normalized.unwrap() - channel_gain_to_norm(db_to_lin(3.5)))
            .abs()
            < 1e-5
    );

    a.execute_command("param trim 150%").unwrap();
    assert!((gain(&a) - 1.5).abs() < 1e-5);
    a.execute_command("param trim 2.0").unwrap();
    assert_eq!(gain(&a), MAX_CHANNEL_GAIN);
    assert_eq!(a.control_descriptors()[0].normalized, Some(1.0));
    a.execute_command("param trim -inf").unwrap();
    assert_eq!(gain(&a), 0.0);
    assert_eq!(a.control_descriptors()[0].display, "-inf dB");
    assert_eq!(a.control_descriptors()[0].normalized, Some(0.0));

    // Stepping up from -inf inside Detail::Channel enters at -60 dB cleanly!
    a.press_keys("<m-l>").unwrap();
    assert!((gain(&a) - 0.001).abs() < 1e-8);
    a.press_keys("<m-h>").unwrap();
    assert_eq!(gain(&a), 0.0);

    // Out-of-range inputs are rejected without mutating state.
    assert!(a.execute_command("param trim +7dB").is_err());
    assert!(a.execute_command("param trim 2.1").is_err());
    assert!(a.execute_command("param trim 205%").is_err());
    assert_eq!(gain(&a), 0.0);
}

#[test]
fn unchanged_explicit_value_is_repeatable_without_a_spurious_undo() {
    let mut a = app();
    a.press_keys("gpndZeros<Enter>16<Enter><Enter>0<Enter>")
        .unwrap();
    assert_eq!(a.revision(), 1, "same-value entry adds no musical revision");
    a.press_keys("l;").unwrap();
    assert_eq!(a.revision(), 2);
    assert_eq!(
        values(&a, "Zeros"),
        vec![
            ValuePoint {
                step: 0,
                value: 0.0
            },
            ValuePoint {
                step: 1,
                value: 0.0
            }
        ]
    );
    a.press_keys("u").unwrap();
    assert_eq!(values(&a, "Zeros").len(), 1);
}

#[test]
fn value_entry_keeps_home_global_and_blocks_unrelated_musical_history() {
    let mut a = app();
    harmony(&mut a);
    let before = a.project().clone();
    let revision = a.revision();
    a.press_keys("<Enter>-9<Home><C-r>").unwrap();
    assert_eq!(a.value_input_view().unwrap().input, "-9");
    assert!(!a.playing());
    assert_eq!(a.transport_step(), 0);
    assert_eq!(a.revision(), revision);
    a.press_keys("<Esc>").unwrap();
    assert_eq!(a.project(), &before);
}

#[test]
fn stop_clears_pending_counts_and_consumed_motion_status_is_current() {
    let mut a = app();
    a.execute_command("part new One init 0").unwrap();
    a.press_keys("gmgg").unwrap();
    let channel = a.selected_channel;
    for home in [true, false] {
        a.apply(Action::SetParam {
            address: ParamAddress::Channel {
                channel,
                param: ChannelParam::Gain,
            },
            value: 0.1,
        })
        .unwrap();
        a.press_keys("24").unwrap();
        assert_eq!(a.pending_count(), Some(24));
        if home {
            a.press_keys("<Home>").unwrap();
        } else {
            a.apply(Action::Stop).unwrap();
        }
        assert_eq!(a.pending_count(), None);
        a.press_keys("<m-k>").unwrap();
        let cadence_core::mixer::ChannelRef::Channel(id) = channel else {
            panic!()
        };
        let gain = a.project().channel(id).unwrap().strip.gain;
        assert!(
            (cadence_core::dsp::lin_to_db(gain) + 19.0).abs() < 1e-4,
            "Stop must consume the old 24-count"
        );
    }
    harmony(&mut a);
    a.press_keys("gg24l").unwrap();
    assert_eq!(a.pending_count(), None);
    assert!(a.status.contains("offset 24"));
    assert!(!a.status.starts_with("Count 24"));
}

#[test]
fn new_document_and_cross_document_undo_clear_pending_entry_state() {
    let mut a = app();
    harmony(&mut a);
    let before = a.project().clone();
    a.press_keys("<Enter>-9").unwrap();
    a.apply(Action::New {
        title: Some("Fresh".into()),
    })
    .unwrap();
    assert!(!a.is_value_input());
    assert!(!a.is_authoring());
    assert_eq!(a.pending_count(), None);
    assert_eq!(a.mode_name(), "NORMAL");
    a.apply(Action::Undo).unwrap();
    assert_eq!(a.project(), &before);
    a.press_keys("ndUnfinished").unwrap();
    assert!(a.is_authoring());
    a.apply(Action::New { title: None }).unwrap();
    assert!(!a.is_authoring());
    assert_eq!(a.mode_name(), "NORMAL");
    a.apply(Action::Undo).unwrap();
    a.press_keys("24").unwrap();
    a.apply(Action::New { title: None }).unwrap();
    assert_eq!(a.pending_count(), None);
}
