//! End-to-end controller guarantees: retained music, explicit scope, and adapters.
use cadence_app::keys::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use cadence_app::{Action, App, ControlTarget, Detail, EditScope, Focus, Request, Workspace};
use cadence_core::{
    material::{Edit, PatternKind, PatternValue, Project},
    mixer::{ChannelRef, EffectParam},
    rack::{ModuleKind, ModuleParam, RackTemplate},
    runtime::TransportIntent,
};
use std::path::PathBuf;
fn empty() -> App {
    App::new_with_library(
        Project::empty(),
        PathBuf::from("/tmp/cadence-material-test.json"),
        cadence_core::sampler::SampleLibrary::from_roots_only(&[], None),
    )
    .unwrap()
}
fn key(app: &mut App, c: char, kind: KeyEventKind) -> cadence_app::Outcome {
    app.handle_key(KeyEvent::new_with_kind(
        KeyCode::Char(c),
        KeyModifiers::NONE,
        kind,
    ))
    .unwrap()
}
fn alt(app: &mut App, c: char, kind: KeyEventKind) -> cadence_app::Outcome {
    app.handle_key(KeyEvent::new_with_kind(
        KeyCode::Char(c),
        KeyModifiers::ALT,
        kind,
    ))
    .unwrap()
}
fn cmd(app: &mut App, s: &str) {
    app.execute_command(s)
        .unwrap_or_else(|e| panic!("{s}: {e:#}"));
}
fn note(app: &App, id: u64, t: i64) -> cadence_core::music::Step {
    let PatternValue::Notes(s) = app.project().evaluate_pattern(id, t).unwrap() else {
        panic!()
    };
    s
}

#[test]
fn playable_creation_is_one_undo_and_ids_never_reuse_after_undo() {
    let mut app = empty();
    cmd(&mut app, "part new Lead pulse_lead 0 2 4 6 7");
    assert_eq!(app.project().patterns.len(), 1);
    assert_eq!(app.project().instruments.len(), 1);
    assert_eq!(app.project().parts.len(), 1);
    assert_eq!(app.project().lanes[0].placements.len(), 1);
    assert_eq!(app.project().mixer.channels.len(), 1);
    let prior = app.project().clone();
    let max = prior.max_id();
    app.apply(Action::Undo).unwrap();
    assert_eq!(app.project(), &Project::empty());
    app.apply(Action::Redo).unwrap();
    assert_eq!(app.project(), &prior);
    app.apply(Action::Undo).unwrap();
    cmd(&mut app, "pattern notes Fresh 0");
    assert!(app.project().patterns[0].id > max);
}
#[test]
fn failed_playable_and_stale_api_edits_are_atomic() {
    let mut app = empty();
    let initial = app.project().clone();
    let result = app.apply(Action::PlacedPart {
        name: "Oops".into(),
        sound: cadence_app::workflow::SoundChoice::Template(RackTemplate::Synth),
        notes: vec![Default::default()],
        start_step: u32::MAX,
        length_steps: None,
        transpose: None,
        velocity_scale: None,
    });
    assert!(result.is_err());
    assert_eq!(app.project(), &initial);
    assert_eq!(app.revision(), 0);
    let (reply, out) = app.execute_request(Request {
        id: None,
        expected_revision: Some(1),
        compact: false,
        action: Action::CreateLane {
            name: "stale".into(),
        },
    });
    assert_eq!(reply["ok"], false);
    assert!(!out.changed);
    assert_eq!(app.project(), &initial);
}
#[test]
fn key_and_typed_creation_paths_produce_identical_music() {
    let mut keyed = empty();
    keyed
        .press_keys(":pattern notes Motif 0 2 4 7 6<Enter>")
        .unwrap();
    let mut typed = empty();
    typed
        .apply(Action::CreatePattern {
            name: "Motif".into(),
            kind: keyed.project().patterns[0].kind.clone(),
        })
        .unwrap();
    assert_eq!(keyed.project(), typed.project());
    let prior = keyed.project().clone();
    let rev = keyed.revision();
    keyed.apply(Action::Inspect { pattern: None }).unwrap();
    assert_eq!(keyed.project(), &prior);
    assert_eq!(keyed.revision(), rev);
    assert_eq!(keyed.mode_name(), "NORMAL");
}
#[test]
fn sources_stay_shared_and_local_exceptions_are_placement_relative_and_undoable() {
    let mut app = empty();
    cmd(&mut app, "pattern notes Motif 0 2 4 6 7");
    let source = app.selected_pattern.unwrap();
    cmd(&mut app, "pattern degrees Chords 16 hold 0:0 8:3");
    cmd(&mut app, "derive Lift add Motif Chords");
    let derived = app.selected_pattern.unwrap();
    cmd(&mut app, "instrument Lead pulse_lead");
    cmd(&mut app, "part Part Lift Lead");
    cmd(&mut app, "lane A");
    cmd(&mut app, "place Part A 2 1 song");
    let placement = app.selected_placement.unwrap();
    cmd(&mut app, "local");
    assert_eq!(app.scope, EditScope::Placement(placement));
    let before = app.current_value(0).unwrap();
    cmd(&mut app, "note 1 9 70% 2");
    assert_eq!(note(&app, source, 0).degree, 1);
    assert_eq!(app.placement(placement).unwrap().1.exceptions[0].step, 0);
    let local = app.current_value(0).unwrap();
    assert_ne!(local, before);
    app.press_keys(".").unwrap();
    let PatternValue::Notes(n) = app.current_value(0).unwrap() else {
        panic!()
    };
    assert_eq!(n.degree, 10);
    assert!((n.velocity - 0.75).abs() < 1e-6);
    app.apply(Action::Undo).unwrap();
    assert_eq!(app.current_value(0).unwrap(), local);
    cmd(&mut app, "inherit");
    assert!(app.placement(placement).unwrap().1.exceptions.is_empty());
    cmd(&mut app, "open Motif");
    cmd(&mut app, "note 1 5");
    assert_eq!(note(&app, derived, 0).degree, 6);
    assert!(matches!(
        app.pattern(derived).unwrap().kind,
        PatternKind::AddDegrees { .. }
    ));
}
#[test]
fn derived_navigation_fork_and_freeze_preserve_the_relationship() {
    let mut app = empty();
    cmd(&mut app, "pattern notes Motif 0 2 4 6 7");
    cmd(&mut app, "pattern degrees Chords 16 hold 0:0 8:3");
    let degrees = app.selected_pattern.unwrap();
    cmd(&mut app, "derive Lift add Motif Chords");
    let derived = app.selected_pattern.unwrap();
    // j selects the second source directly in the derived Pattern deck; Enter opens it.
    app.press_keys("j<Enter>").unwrap();
    assert_eq!(app.selected_pattern, Some(degrees));
    app.press_keys("<Esc>").unwrap();
    assert_eq!(app.selected_pattern, Some(derived));
    cmd(&mut app, "fork Branch");
    assert!(matches!(
        app.selected_pattern_object().unwrap().kind,
        PatternKind::AddDegrees { .. }
    ));
    cmd(&mut app, "freeze Frozen 3 21 Lift");
    let frozen = app.selected_pattern.unwrap();
    assert!(matches!(
        app.pattern(frozen).unwrap().kind,
        PatternKind::Notes { .. }
    ));
    assert_eq!(
        app.project().preview_note_steps(frozen, 0, 21).unwrap(),
        app.project().preview_note_steps(derived, 3, 21).unwrap()
    );
}
#[test]
fn solo_audition_uses_exact_period_and_restores_non_bar_song_position() {
    let mut app = empty();
    cmd(&mut app, "pattern notes Motif 0 2 4 6 7");
    let id = app.selected_pattern.unwrap();
    let original = app.project().clone();
    app.observe_transport(37, 38, false, false);
    let out = app
        .apply(Action::Audition {
            pattern: id,
            length: None,
        })
        .unwrap();
    let aud = app.audition().unwrap();
    assert!(aud.solo);
    assert_eq!(aud.length, 5);
    assert_eq!(aud.anchor, 0);
    assert_eq!(
        out.transport,
        vec![
            TransportIntent::Seek { step: 0 },
            TransportIntent::Play { playing: true }
        ]
    );
    assert_eq!(app.project(), &original);
    assert_eq!(app.project().instruments.len(), 0);
    app.observe_transport(2, 3, true, false);
    let out = app.apply(Action::StopAudition).unwrap();
    assert_eq!(
        out.transport,
        vec![
            TransportIntent::Play { playing: false },
            TransportIntent::Seek { step: 37 }
        ]
    );
    assert!(!app.playing());
    assert_eq!(app.transport_step(), 37);
    assert_eq!(app.project(), &original);
}
#[test]
fn undo_and_deleting_audition_source_return_transport_to_song() {
    let mut app = empty();
    cmd(&mut app, "pattern notes Motif 0 2 4 6 7");
    let id = app.selected_pattern.unwrap();
    app.observe_transport(23, 24, false, false);
    app.apply(Action::Audition {
        pattern: id,
        length: None,
    })
    .unwrap();
    let out = app.apply(Action::Undo).unwrap();
    assert_eq!(
        out.transport,
        vec![
            TransportIntent::Play { playing: false },
            TransportIntent::Seek { step: 23 }
        ]
    );
    assert!(app.audition_pattern().is_none());
    app.apply(Action::Redo).unwrap();
    app.observe_transport(19, 20, false, false);
    app.apply(Action::Audition {
        pattern: id,
        length: None,
    })
    .unwrap();
    let out = app
        .apply(Action::Edit {
            edit: Edit::RemovePattern { id },
        })
        .unwrap();
    assert!(out.changed);
    assert!(app.audition_pattern().is_none());
    assert_eq!(
        out.transport,
        vec![
            TransportIntent::Play { playing: false },
            TransportIntent::Seek { step: 19 }
        ]
    );
}
#[test]
fn mute_and_solo_latch_and_ignore_repeated_keydown() {
    let mut app = empty();
    cmd(&mut app, "part new Lead Init 0 2 4");
    app.press_keys("ga").unwrap();
    let lane = app.selected_lane.unwrap();
    alt(&mut app, 'm', KeyEventKind::Press);
    alt(&mut app, 'm', KeyEventKind::Repeat);
    alt(&mut app, 'm', KeyEventKind::Release);
    assert!(app.flags().lane_muted(lane));
    app.press_keys("gp").unwrap();
    cmd(&mut app, "note 1 2");
    assert!(app.flags().lane_muted(lane));
    app.press_keys("gmh").unwrap();
    alt(&mut app, 's', KeyEventKind::Press);
    alt(&mut app, 's', KeyEventKind::Repeat);
    let ch = ChannelRef::Channel(app.project().mixer.channels[0].id);
    assert!(app.flags().channel_solo(ch));
    app.press_keys("ga").unwrap();
    assert!(app.flags().channel_solo(ch));
}
#[test]
fn jam_release_owns_original_instrument_and_non_release_notes_are_finite() {
    let mut app = empty();
    cmd(&mut app, "instrument First Init");
    let first = app.selected_instrument.unwrap();
    app.press_keys("gj").unwrap();
    let on = key(&mut app, 'z', KeyEventKind::Press);
    assert_eq!(on.live_notes_on[0].instrument_id, first);
    assert!(on.live_notes_on[0].sustain);
    assert!(key(&mut app, 'z', KeyEventKind::Repeat)
        .live_notes_on
        .is_empty());
    cmd(&mut app, "instrument Second Init");
    let second = app.selected_instrument.unwrap();
    assert_ne!(first, second);
    let off = key(&mut app, 'z', KeyEventKind::Release);
    assert_eq!(off.live_notes_off[0].instrument_id, first);
    let out = app
        .handle_script_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(out.live_notes_on[0].instrument_id, second);
    assert!(!out.live_notes_on[0].sustain);
}
#[test]
fn global_navigation_leaves_deck_focus_but_retains_its_target_and_context() {
    let mut app = empty();
    cmd(&mut app, "part new Lead Init 0 2 4");
    let lead = app.project().instruments[0].id;
    app.press_keys("gizzgm").unwrap();
    assert_eq!(app.workspace(), Workspace::Mixer);
    assert!(!app.deck_focused());
    assert!(!app.view_state.deck_zoomed);
    assert_eq!(app.focus(), Focus::Canvas);
    // The sound's deck is one step back along the trail, still zoomed.
    app.press_keys("<Esc>").unwrap();
    assert_eq!(app.detail, Detail::Instrument(lead));
    assert!(app.deck_focused() && app.view_state.deck_zoomed);
    app.press_keys("<Tab>").unwrap();
    assert!(
        !app.view_state.zoomed && !app.view_state.deck_zoomed,
        "moving focus ends a zoom"
    );
    cmd(&mut app, "tips off");
    assert!(!app.view_state.tips);
    app.press_keys(":").unwrap();
    assert!(!app.hints().is_empty());
    app.press_keys("<Esc>gpgi").unwrap();
    assert!(matches!(app.detail, Detail::Instrument(_)));
}
#[test]
fn synth_unison_controls_are_registry_backed_and_parameters_keep_native_units() {
    let mut app = empty();
    cmd(&mut app, "instrument Lead Init");
    cmd(&mut app, "tab panel");
    app.press_keys("l").unwrap(); // focus VCO 1 module panel
    assert!(app.control_descriptors().iter().any(|c| matches!(
        c.target,
        ControlTarget::Param(cadence_core::params::ParamAddress::Module {
            param: ModuleParam::UnisonVoices,
            ..
        })
    )));
    cmd(&mut app, "param unison_voices 4");
    cmd(&mut app, "param cutoff 1500");
    let rack = &app.selected_instrument_object().unwrap().rack;
    let ModuleKind::Vco(vco) = rack.zones[0].module(1).unwrap().kind else {
        panic!("expected VCO 1")
    };
    let ModuleKind::Filter(flt) = rack.zones[0].module(2).unwrap().kind else {
        panic!("expected Filter")
    };
    assert_eq!(vco.unison_voices, 4);
    assert!((flt.cutoff_hz - 1500.0).abs() < 1.0);
}
#[test]
fn instrument_links_resolve_pan_to_the_instrument_and_retained_shapes_stay_visible() {
    let mut app = empty();
    cmd(&mut app, "part new Lead pulse_lead 0 2 4");
    cmd(&mut app, "pattern seed Pan 16 303 0.2 0.8");
    cmd(&mut app, "link Lead pan Pan");
    assert!(matches!(
        app.project().parts[0].parameter_links[0].target,
        cadence_core::params::InstrumentParam::Module {
            param: ModuleParam::Pan,
            ..
        }
    ));
    cmd(&mut app, "mod Drift instrument 'Lead sound' pan Pan");
    assert_eq!(app.project().modulations.len(), 1);
    assert!(app.project().compile().is_ok());
}
#[test]
fn mixer_routing_reverb_controls_and_bypass_are_independent_of_pattern_ownership() {
    let mut app = empty();
    cmd(&mut app, "part new Lead Init 0 2 4");
    let source = app.project().clone().patterns;
    cmd(&mut app, "channel group Music");
    cmd(&mut app, "channel return Space");
    cmd(&mut app, "route Lead Music");
    cmd(&mut app, "send Lead Space 25%");
    cmd(&mut app, "fx add reverb Space");
    let id = app
        .project()
        .mixer
        .channels
        .iter()
        .flat_map(|c| &c.inserts)
        .next()
        .unwrap()
        .id;
    cmd(&mut app, &format!("fx {id}"));
    app.control_cursor = 1;
    let before = app.effect(id).unwrap().1.clone();
    key(&mut app, 'm', KeyEventKind::Press);
    assert!(app.effect(id).unwrap().1.bypassed);
    assert_eq!(
        app.effect(id).unwrap().1.read_normalized(EffectParam::Size),
        before.read_normalized(EffectParam::Size)
    );
    app.apply(Action::SetParam {
        address: cadence_core::params::ParamAddress::Effect {
            device: id,
            param: EffectParam::Size,
        },
        value: 0.75,
    })
    .unwrap();
    assert_eq!(
        app.effect(id).unwrap().1.read_normalized(EffectParam::Size),
        Some(0.75)
    );
    assert_eq!(app.project().patterns, source);
    let audio = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "Lead")
        .unwrap();
    let group = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "Music")
        .unwrap();
    assert_eq!(audio.output, ChannelRef::Channel(group.id));
    assert_eq!(audio.sends[0].amount, 0.25);
}
#[test]
fn asset_diagnostics_and_reload_do_not_mutate_the_score() {
    let mut app = empty();
    cmd(&mut app, "sampler Missing ./does-not-exist.wav");
    let score = app.project().clone();
    let revision = app.revision();
    cmd(&mut app, "assets");
    assert!(app.show_assets());
    assert!(app.inspect_assets()["issues"].as_array().unwrap().len() == 1);
    let out = app.apply(Action::ReloadSamples).unwrap();
    assert!(out.samples_changed);
    assert_eq!(app.project(), &score);
    assert_eq!(app.revision(), revision);
    assert!(app.ready(), "unused sampler does not block playback");
}

#[test]
fn local_edit_scope_never_compounds_binding_pitch_or_velocity() {
    let mut app = empty();
    cmd(&mut app, "part new Lead Init 0 2 4");
    let mut part = app.project().parts[0].clone();
    part.transpose = 5;
    part.velocity_scale = 0.5;
    app.apply(Action::Edit {
        edit: Edit::PutPart { part },
    })
    .unwrap();
    let placement = app.selected_placement.unwrap();
    cmd(&mut app, "local");
    let PatternValue::Notes(before) = app.current_value(0).unwrap() else {
        panic!()
    };
    app.press_keys("..").unwrap();
    let raw = &app.placement(placement).unwrap().1.exceptions[0].value;
    assert_eq!(raw.degree, before.degree);
    assert!((raw.velocity - (before.velocity + 0.1)).abs() < 1e-5);
    let heard = app.project().resolve_placement(placement, 0).unwrap().value;
    assert!((heard.velocity - raw.velocity * 0.5).abs() < 1e-6);
    assert_eq!(
        app.project().pitch_midi(heard.degree, heard.accidental, 0),
        app.project().pitch_midi(raw.degree, raw.accidental, 5)
    );
}
#[test]
fn command_clear_and_search_keep_hidden_focus_reachable() {
    let mut app = empty();
    cmd(&mut app, "part new Lead Init 0 2");
    let before = app.project().clone();
    app.press_keys(":bpm 999<C-u>title Cleared<Enter>").unwrap();
    assert_eq!(app.project().bpm, before.bpm);
    assert_eq!(app.project().title, "Cleared");
    app.press_keys("gpZ/").unwrap();
    assert!(!app.view_state.zoomed);
    assert_eq!(app.focus(), Focus::Library);
    assert!(app.search_line().is_some());
}

#[test]
fn binding_inspector_can_edit_performance_and_rebind_without_copying_material() {
    let mut app = empty();
    cmd(&mut app, "part new Lead Init 0 2 4");
    let source = app.project().patterns[0].clone();
    cmd(&mut app, "part Lead transpose 5");
    cmd(&mut app, "part Lead velocity 80%");
    cmd(&mut app, "part Lead");
    app.control_cursor = 2;
    app.press_keys("<m-l>").unwrap();
    assert_eq!(app.project().parts[0].transpose, 6);
    assert_eq!(app.project().parts[0].velocity_scale, 0.8);
    assert_eq!(app.project().patterns[0], source);
    cmd(&mut app, "pattern seed Pan 16 303 0.2 0.8");
    cmd(&mut app, "link Lead pan Pan");
    cmd(&mut app, "unlink Lead pan");
    assert!(app.project().parts[0].parameter_links.is_empty());
    cmd(&mut app, "instrument Other pluck");
    cmd(&mut app, "part Lead instrument Other");
    assert_eq!(
        app.project().parts[0].instrument,
        app.selected_instrument.unwrap()
    );
    assert_eq!(app.project().patterns[0], source);
}

#[test]
fn literal_editor_keeps_conditional_notes_and_its_stored_length() {
    let mut app = empty();
    cmd(&mut app, "pattern notes Conditional 0 2 4 6 7");
    let id = app.selected_pattern.unwrap();
    let mut step = note(&app, id, 0);
    step.condition = cadence_core::music::StepCondition { turn: 2, every: 3 };
    app.apply(Action::Edit {
        edit: Edit::SetNote {
            pattern: id,
            step: 0,
            value: step.clone(),
        },
    })
    .unwrap();
    assert_eq!(app.current_length(), 5);
    assert_eq!(app.current_value(0).unwrap(), PatternValue::Notes(step));
    assert!(!note(&app, id, 0).active);
    app.press_keys("$k").unwrap();
    assert_eq!(app.cursor, 4);
}

#[test]
fn local_nudges_copy_an_occurrence_without_its_source_repeat_condition() {
    let mut app = empty();
    cmd(&mut app, "part new Lead Init 0 2 4");
    let pattern = app.selected_pattern.unwrap();
    let placement = app.selected_placement.unwrap();
    let mut source = note(&app, pattern, 0);
    source.condition = cadence_core::music::StepCondition { turn: 1, every: 2 };
    app.apply(Action::Edit {
        edit: Edit::SetNote {
            pattern,
            step: 0,
            value: source.clone(),
        },
    })
    .unwrap();
    cmd(&mut app, "local");
    app.press_keys(".<m-k>").unwrap();
    let local = &app.placement(placement).unwrap().1.exceptions[0].value;
    assert_eq!(local.condition, cadence_core::music::StepCondition::ALWAYS);
    assert!(local.active);
    assert_eq!(local.degree, source.degree + 1);
    assert_eq!(
        app.pattern(pattern).unwrap().kind,
        PatternKind::Notes {
            steps: vec![source, note(&app, pattern, 1), note(&app, pattern, 2)]
        }
    );
}

#[test]
fn insert_writes_one_octave_with_shift_accents_and_octave_keys() {
    let mut app = empty();
    cmd(&mut app, "part new Lead Init 0 0 0 0");
    let id = app.selected_pattern.unwrap();
    app.press_keys("gpi").unwrap();
    assert_eq!(app.mode_name(), "STEP INSERT");
    let before = note(&app, id, 0).velocity;
    // Unmapped keys (`9` and `q`) write nothing and keep the cursor in place.
    app.press_keys("z9q>Z.").unwrap();
    let first = note(&app, id, 0);
    let second = note(&app, id, 1);
    let project = app.project();
    assert_eq!(
        project.pitch_midi(second.degree, second.accidental, 0),
        project.pitch_midi(first.degree, first.accidental, 0) + 12,
        "> moved the octave up"
    );
    assert_eq!(first.velocity, before);
    assert_eq!(second.velocity, 1.0, "Shift accents");
    assert!(!note(&app, id, 2).active, ". writes a rest");
}

#[test]
fn yank_and_paste_duplicate_a_placement_after_itself_and_copy_a_note() {
    let mut app = empty();
    cmd(&mut app, "part new Lead Init 0 2 4");
    app.press_keys("ga").unwrap();
    let lane = app.selected_lane.unwrap();
    let original = app.lane(lane).unwrap().placements[0].clone();
    app.press_keys("yp").unwrap();
    let placements = &app.lane(lane).unwrap().placements;
    assert_eq!(placements.len(), 2);
    let copy = placements.iter().find(|p| p.id != original.id).unwrap();
    assert_eq!(copy.part, original.part);
    assert_eq!(copy.start_step, original.start_step + original.length_steps);
    assert_eq!(
        app.selected_placement,
        Some(copy.id),
        "the cursor moves onto the copy"
    );
    app.press_keys("u").unwrap();
    assert_eq!(app.lane(lane).unwrap().placements.len(), 1);
    let pattern = app.project().parts[0].pattern;
    cmd(&mut app, "open \"Lead motif\"");
    assert_eq!(app.selected_pattern, Some(pattern));
    let first = note(&app, pattern, 0);
    app.press_keys("yp").unwrap();
    assert_eq!(app.cursor, 1);
    assert_eq!(note(&app, pattern, 1).degree, first.degree);
}
