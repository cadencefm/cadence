use cadence_app::keys::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use cadence_app::{
    authoring::{DraftField, DraftInput},
    workflow::{
        ChannelChoice, InstrumentChoice, LaneChoice, PartChoice, PatternChoice, SoundChoice,
        WorkflowContext, WorkflowDraft, WorkflowKind, WorkflowOutcome, WorkflowPlan,
    },
};
use cadence_core::material::{PhaseMode, Project};
use cadence_core::presets::PresetCatalog;

fn repo_presets() -> PresetCatalog {
    PresetCatalog::from_roots(&[crate::common::repo_root().join("presets")])
}
fn key(d: &mut WorkflowDraft, p: &Project, code: KeyCode) -> WorkflowOutcome {
    d.handle_key(p, KeyEvent::new(code, KeyModifiers::NONE))
}
fn text(d: &mut WorkflowDraft, p: &Project, value: &str) -> WorkflowOutcome {
    let mut out = WorkflowOutcome::Continue;
    for c in value.chars() {
        out = key(
            d,
            p,
            if c == '\n' {
                KeyCode::Enter
            } else {
                KeyCode::Char(c)
            },
        );
    }
    out
}
fn accept(d: &mut WorkflowDraft, p: &Project) -> WorkflowOutcome {
    key(d, p, KeyCode::Enter)
}
fn finish(d: &mut WorkflowDraft, p: &Project) -> WorkflowPlan {
    for _ in 0..32 {
        match accept(d, p) {
            WorkflowOutcome::Submit(plan) => return *plan,
            WorkflowOutcome::Continue => assert!(d.view(p).error.is_none(), "{:?}", d.view(p)),
            other => panic!("unexpected {other:?}"),
        }
    }
    panic!("workflow did not finish")
}
fn submitted(out: WorkflowOutcome) -> WorkflowPlan {
    let WorkflowOutcome::Submit(plan) = out else {
        panic!("not submitted: {out:?}")
    };
    *plan
}

#[test]
fn instrument_uses_registry_presets_and_stages_audio_destination_without_mutation() {
    let p = Project::empty();
    let before = p.clone();
    let presets = repo_presets();
    let mut d = WorkflowDraft::new(
        WorkflowKind::Instrument,
        &p,
        WorkflowContext::default(),
        &presets,
    );
    text(&mut d, &p, "Lead\ns");
    assert_eq!(d.view(&p).field, DraftField::Preset);
    assert_eq!(
        key(&mut d, &p, KeyCode::Char('A')),
        WorkflowOutcome::AuditionPreset(None)
    );
    text(&mut d, &p, "/pulse\n");
    assert!(!d.view(&p).searching);
    assert_eq!(d.view(&p).field, DraftField::Preset);
    accept(&mut d, &p);
    assert_eq!(d.view(&p).field, DraftField::Channel);
    accept(&mut d, &p);
    assert_eq!(d.view(&p).field, DraftField::ChannelName);
    let WorkflowPlan::Instrument(i) = submitted(accept(&mut d, &p)) else {
        panic!()
    };
    assert_eq!(i.name, "Lead");
    assert_eq!(i.sound, SoundChoice::Preset("pulse-lead".into()));
    assert_eq!(
        i.channel,
        ChannelChoice::New {
            name: "Lead".into()
        }
    );
    assert_eq!(p, before);
    assert!(p.instruments.is_empty());
}

#[test]
fn tab_keeps_the_filtered_preset_ready_to_audition_before_accepting() {
    let p = Project::empty();
    let presets = repo_presets();
    let mut d = WorkflowDraft::new(
        WorkflowKind::Instrument,
        &p,
        WorkflowContext::default(),
        &presets,
    );
    text(&mut d, &p, "Lead\ns/pulse");
    assert!(d.view(&p).searching);
    assert!(d.view(&p).hints.iter().any(|(key, _)| *key == "Tab"));
    let candidates = d.view(&p).candidates;
    key(&mut d, &p, KeyCode::Tab);
    assert!(!d.view(&p).searching);
    assert_eq!(d.view(&p).candidates, candidates);
    assert_eq!(
        key(&mut d, &p, KeyCode::Char('A')),
        WorkflowOutcome::AuditionPreset(Some("pulse-lead".into()))
    );
    assert_eq!(d.view(&p).field, DraftField::Preset);
    assert!(p.instruments.is_empty());
    let plan = finish(&mut d, &p);
    let WorkflowPlan::Instrument(instrument) = plan else {
        panic!()
    };
    assert_eq!(instrument.sound, SoundChoice::Preset("pulse-lead".into()));
    assert_eq!(instrument.name, "Lead");
}

#[test]
fn sample_handoff_keeps_exact_path_and_does_not_publish_or_decode_it() {
    let p = Project::empty();
    let presets = PresetCatalog::empty();
    let mut d = WorkflowDraft::new(
        WorkflowKind::Instrument,
        &p,
        WorkflowContext::default(),
        &presets,
    );
    text(&mut d, &p, "Kit\nm");
    assert_eq!(d.view(&p).field, DraftField::Sample);
    assert_eq!(
        key(&mut d, &p, KeyCode::Char('b')),
        WorkflowOutcome::ChooseSample
    );
    assert!(d.sample_target_label(&p).contains("Kit"));
    assert_eq!(
        d.set_sample(&p, "/virtual/library/tone.wav".into(), "tone".into()),
        WorkflowOutcome::Continue
    );
    assert_eq!(d.view(&p).field, DraftField::Channel);
    let plan = finish(&mut d, &p);
    let WorkflowPlan::Instrument(i) = &plan else {
        panic!()
    };
    assert_eq!(i.name, "Kit");
    assert_eq!(
        i.sound,
        SoundChoice::Sampler {
            path: Some("/virtual/library/tone.wav".into()),
            label: Some("tone".into()),
        }
    );
    assert_eq!(plan.new_object_count(), 2);
    assert!(p.instruments.is_empty());
    let mut pref = WorkflowDraft::instrument_with_sample(
        &p,
        WorkflowContext::default(),
        "/virtual/other.wav".into(),
        "Other".into(),
        &presets,
    );
    assert_eq!(pref.view(&p).field, DraftField::Name);
    assert_eq!(pref.view(&p).text, "Other");
    assert!(matches!(finish(&mut pref, &p), WorkflowPlan::Instrument(_)));
}

#[test]
fn binding_pickers_reuse_derived_notes_and_existing_instrument_by_stable_id() {
    let p = Project::demo();
    let before = p.clone();
    let presets = PresetCatalog::empty();
    let mut d = WorkflowDraft::new(
        WorkflowKind::Part,
        &p,
        WorkflowContext {
            pattern: Some(4),
            instrument: Some(1),
            ..Default::default()
        },
        &presets,
    );
    text(&mut d, &p, "NewPart\n");
    let view = d.view(&p);
    assert_eq!(view.field, DraftField::Pattern);
    assert_eq!(view.input, DraftInput::Reference);
    assert!(view.candidates.iter().any(|c| c.id == Some(4)));
    assert!(!view.candidates.iter().any(|c| c.id == Some(2)));
    accept(&mut d, &p);
    assert_eq!(d.view(&p).field, DraftField::Instrument);
    accept(&mut d, &p);
    assert_eq!(d.view(&p).field, DraftField::PartTranspose);
    accept(&mut d, &p);
    assert_eq!(d.view(&p).field, DraftField::PartVelocity);
    let plan = submitted(accept(&mut d, &p));
    let WorkflowPlan::Part(b) = &plan else {
        panic!()
    };
    assert_eq!(b.pattern, PatternChoice::Existing(4));
    assert_eq!(b.instrument, InstrumentChoice::Existing(1));
    assert_eq!(plan.new_object_count(), 1);
    assert_eq!(p, before);
}

#[test]
fn playable_empty_workspace_stages_the_whole_graph_as_one_plan() {
    let p = Project::empty();
    let before = p.clone();
    let presets = PresetCatalog::empty();
    let mut d = WorkflowDraft::new(
        WorkflowKind::PlacedPart,
        &p,
        WorkflowContext::default(),
        &presets,
    );
    text(&mut d, &p, "Neon\n");
    let plan = finish(&mut d, &p);
    assert_eq!(plan.new_object_count(), 6);
    plan.validate(&p).unwrap();
    let WorkflowPlan::PlacedPart(place) = plan else {
        panic!()
    };
    assert_eq!(place.start_step, 0);
    assert_eq!(place.length_steps, p.arrangement_steps() as u32);
    assert_eq!(place.phase, PhaseMode::Song);
    assert_eq!(
        place.lane,
        LaneChoice::New {
            name: "Neon".into()
        }
    );
    let PartChoice::New(b) = place.part else {
        panic!()
    };
    assert_eq!(b.name, "Neon");
    assert_eq!(
        b.pattern,
        PatternChoice::NewNotes {
            name: "Neon motif".into(),
            length: 16
        }
    );
    let InstrumentChoice::New(i) = b.instrument else {
        panic!()
    };
    assert_eq!(i.name, "Neon sound");
    assert_eq!(
        i.channel,
        ChannelChoice::New {
            name: "Neon".into()
        }
    );
    assert_eq!(p, before);
}

#[test]
fn placement_supports_exact_step_lengths_restart_and_negative_offset() {
    let p = Project::demo();
    let presets = PresetCatalog::empty();
    let mut d = WorkflowDraft::new(
        WorkflowKind::Place,
        &p,
        WorkflowContext {
            part: Some(4),
            lane: Some(3),
            ..Default::default()
        },
        &presets,
    );
    accept(&mut d, &p);
    accept(&mut d, &p);
    assert_eq!(d.view(&p).field, DraftField::Start);
    text(&mut d, &p, "2\n5s\nr");
    let WorkflowPlan::Place(place) = submitted(text(&mut d, &p, "-3\n")) else {
        panic!()
    };
    assert_eq!(place.part, PartChoice::Existing(4));
    assert_eq!(place.lane, LaneChoice::Existing(3));
    assert_eq!(place.start_step, 16);
    assert_eq!(place.length_steps, 5);
    assert_eq!(place.offset_steps, -3);
    assert_eq!(place.phase, PhaseMode::Restart);
}

#[test]
fn nested_new_objects_cancel_and_backtrack_without_reinterpreting_existing_refs() {
    let p = Project::demo();
    let before = p.clone();
    let presets = PresetCatalog::empty();
    let mut d = WorkflowDraft::new(
        WorkflowKind::Part,
        &p,
        WorkflowContext {
            pattern: Some(4),
            instrument: Some(1),
            ..Default::default()
        },
        &presets,
    );
    text(&mut d, &p, "Nested\n");
    accept(&mut d, &p);
    text(&mut d, &p, "n\nm");
    assert_eq!(d.view(&p).field, DraftField::Sample);
    d.set_sample(&p, "/virtual/file.wav".into(), "File".into());
    assert_eq!(d.view(&p).field, DraftField::Channel);
    key(&mut d, &p, KeyCode::BackTab);
    key(&mut d, &p, KeyCode::BackTab);
    assert_eq!(d.view(&p).field, DraftField::InstrumentKind);
    key(&mut d, &p, KeyCode::Char('s'));
    assert_eq!(d.view(&p).field, DraftField::Preset);
    assert_eq!(key(&mut d, &p, KeyCode::Esc), WorkflowOutcome::Cancel);
    assert_eq!(p, before);
}

#[test]
fn filtered_audio_choices_never_accept_a_hidden_or_wrong_role_destination() {
    let p = Project::demo();
    let presets = PresetCatalog::empty();
    let mut d = WorkflowDraft::new(
        WorkflowKind::Instrument,
        &p,
        WorkflowContext {
            channel: Some(1),
            ..Default::default()
        },
        &presets,
    );
    text(&mut d, &p, "Other\ns");
    accept(&mut d, &p);
    let view = d.view(&p);
    assert_eq!(view.field, DraftField::Channel);
    assert!(view.candidates.iter().any(|c| c.id == Some(1)));
    assert!(!view
        .candidates
        .iter()
        .any(|c| c.id == Some(4) || c.id == Some(5)));
    text(&mut d, &p, "/Music");
    assert!(d.view(&p).candidates.is_empty());
    // First Enter locks filter; second Enter attempts accept and reports missing match.
    assert_eq!(accept(&mut d, &p), WorkflowOutcome::Continue);
    assert!(!d.view(&p).searching);
    assert_eq!(accept(&mut d, &p), WorkflowOutcome::Continue);
    assert!(d.view(&p).error.is_some());
    // Esc clears the locked filter without closing the draft.
    key(&mut d, &p, KeyCode::Esc);
    assert!(!d.view(&p).searching);
    assert!(d.view(&p).query.is_empty());
    assert!(matches!(
        submitted(accept(&mut d, &p)),
        WorkflowPlan::Instrument(_)
    ));
}

#[test]
fn invalid_fields_and_stale_references_remain_editable_and_do_not_guess() {
    let mut p = Project::demo();
    let presets = PresetCatalog::empty();
    let mut d = WorkflowDraft::new(
        WorkflowKind::Place,
        &p,
        WorkflowContext {
            part: Some(4),
            lane: Some(3),
            ..Default::default()
        },
        &presets,
    );
    accept(&mut d, &p);
    accept(&mut d, &p);
    text(&mut d, &p, "0\n");
    assert_eq!(d.view(&p).field, DraftField::Start);
    assert_eq!(d.view(&p).text, "0");
    assert!(d.view(&p).error.is_some());
    d.handle_key(&p, KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    text(&mut d, &p, "1\n1\ns");
    assert_eq!(d.view(&p).field, DraftField::Offset);
    p.parts.retain(|b| b.id != 4);
    assert_eq!(accept(&mut d, &p), WorkflowOutcome::Continue);
    assert!(d.view(&p).error.is_some());
    assert_eq!(d.view(&p).field, DraftField::Offset);
    assert!(!d
        .view(&p)
        .fields
        .iter()
        .find(|f| f.label == "Part")
        .unwrap()
        .value
        .starts_with("+ New"));
}

#[test]
fn overlap_remains_an_error_regardless_of_live_mute_state() {
    let p = Project::demo();
    let presets = PresetCatalog::empty();
    let mut d = WorkflowDraft::new(
        WorkflowKind::Place,
        &p,
        WorkflowContext {
            part: Some(1),
            lane: Some(3),
            ..Default::default()
        },
        &presets,
    );
    accept(&mut d, &p);
    accept(&mut d, &p);
    text(&mut d, &p, "1\n1\ns");
    assert_eq!(accept(&mut d, &p), WorkflowOutcome::Continue);
    assert!(d
        .view(&p)
        .error
        .as_deref()
        .unwrap()
        .contains("instrument already plays"));
}

#[test]
fn repeated_accept_and_numeric_boundaries_do_not_skip_validation() {
    let p = Project::empty();
    let presets = PresetCatalog::empty();
    let mut d = WorkflowDraft::new(WorkflowKind::Lane, &p, WorkflowContext::default(), &presets);
    let out = d.handle_key(
        &p,
        KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Repeat),
    );
    assert_eq!(out, WorkflowOutcome::Continue);
    assert_eq!(d.view(&p).field, DraftField::Name);
    text(&mut d, &p, "Road\n");
    assert!(cadence_app::workflow::parse_position("0").is_err());
    assert!(cadence_app::workflow::parse_duration("1.5").is_err());
    assert!(cadence_app::workflow::parse_duration("4294967295b").is_err());
    assert_eq!(cadence_app::workflow::parse_position("17s").unwrap(), 16);
    assert_eq!(cadence_app::workflow::parse_duration("5s").unwrap(), 5);
}

#[test]
fn accepting_a_default_instrument_name_is_not_undone_by_sample_selection() {
    let project = Project::empty();
    let presets = PresetCatalog::empty();
    let mut draft = WorkflowDraft::new(
        WorkflowKind::Instrument,
        &project,
        WorkflowContext::default(),
        &presets,
    );
    let name = draft.view(&project).text;
    accept(&mut draft, &project);
    key(&mut draft, &project, KeyCode::Char('m'));
    draft.set_sample(&project, "/virtual/other.wav".into(), "Other sample".into());
    let WorkflowPlan::Instrument(instrument) = finish(&mut draft, &project) else {
        panic!()
    };
    assert_eq!(instrument.name, name);
}

#[test]
fn unified_sound_choice_and_rack_template_creation() {
    use cadence_app::{Action, App};
    use cadence_core::rack::RackTemplate;
    use cadence_core::sampler::SampleLibrary;

    let write_wav = |path: &std::path::Path, freq_hz: f32| {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for n in 0..800usize {
            let phase = n as f32 * freq_hz * std::f32::consts::TAU / 44_100.0;
            w.write_sample((phase.sin() * 14_000.0) as i16).unwrap();
        }
        w.finalize().unwrap();
    };

    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("strings");
    std::fs::create_dir_all(&folder).unwrap();
    write_wav(&folder.join("Strings_C3.wav"), 130.8);
    write_wav(&folder.join("Strings_C4.wav"), 261.6);

    let p = Project::empty();
    let empty_presets = PresetCatalog::empty();
    let presets = repo_presets();

    // 1. Blank Sampler via 's' shortcut on DraftField::Sample.
    let mut d = WorkflowDraft::new(
        WorkflowKind::Instrument,
        &p,
        WorkflowContext::default(),
        &empty_presets,
    );
    text(&mut d, &p, "EmptySamp\nm");
    assert_eq!(d.view(&p).field, DraftField::Sample);
    text(&mut d, &p, "s");
    assert_eq!(d.view(&p).field, DraftField::Channel);
    let WorkflowPlan::Instrument(plan) = finish(&mut d, &p) else {
        panic!("expected instrument plan");
    };
    assert_eq!(plan.sound, SoundChoice::Template(RackTemplate::Sampler));

    // 2. Blank Kit ('d' -> 'b') & Blank Modular ('u' -> Enter).
    let mut d = WorkflowDraft::new(
        WorkflowKind::Instrument,
        &p,
        WorkflowContext::default(),
        &empty_presets,
    );
    text(&mut d, &p, "BlankKit\nd");
    assert_eq!(d.view(&p).field, DraftField::KitSource);
    text(&mut d, &p, "b");
    let WorkflowPlan::Instrument(plan) = finish(&mut d, &p) else {
        panic!("expected instrument plan");
    };
    assert_eq!(plan.sound, SoundChoice::Template(RackTemplate::Kit));

    let mut d = WorkflowDraft::new(
        WorkflowKind::Instrument,
        &p,
        WorkflowContext::default(),
        &empty_presets,
    );
    text(&mut d, &p, "BlankMod\nu");
    assert_eq!(d.view(&p).field, DraftField::Preset);
    let WorkflowPlan::Instrument(plan) = finish(&mut d, &p) else {
        panic!("expected instrument plan");
    };
    assert_eq!(plan.sound, SoundChoice::Template(RackTemplate::Modular));

    // 3. External Preset Kit ('d' -> '/tr-808\n' selects `tr-808` ahead of `tr-808-cl`).
    let mut d = WorkflowDraft::new(
        WorkflowKind::Instrument,
        &p,
        WorkflowContext::default(),
        &presets,
    );
    text(&mut d, &p, "Kit808\nd");
    assert_eq!(d.view(&p).field, DraftField::KitSource);
    let kit_candidates: Vec<_> = d
        .view(&p)
        .candidates
        .iter()
        .skip(2)
        .take(6)
        .map(|c| c.label.clone())
        .collect();
    assert!(
        kit_candidates.iter().any(|l| l == "TR-808")
            && kit_candidates.iter().any(|l| l == "TR-808 Synth")
            && kit_candidates.iter().any(|l| l == "TR-909")
            && kit_candidates.iter().any(|l| l == "TR-909 Synth"),
        "KitSource must list Kit presets first; top 6 were {kit_candidates:?}"
    );
    text(&mut d, &p, "/tr-808\n");
    let WorkflowPlan::Instrument(plan) = finish(&mut d, &p) else {
        panic!("expected instrument plan");
    };
    assert_eq!(plan.sound, SoundChoice::Preset("tr-808".into()));

    let mut d_cl = WorkflowDraft::new(
        WorkflowKind::Instrument,
        &p,
        WorkflowContext::default(),
        &presets,
    );
    text(&mut d_cl, &p, "Claves\nd/tr-808-cl\n");
    let WorkflowPlan::Instrument(plan_cl) = finish(&mut d_cl, &p) else {
        panic!("expected instrument plan");
    };
    assert_eq!(plan_cl.sound, SoundChoice::Preset("tr-808-cl".into()));

    // 4. Folder Kit ('d' -> 'f' -> path).
    let mut d = WorkflowDraft::new(
        WorkflowKind::Instrument,
        &p,
        WorkflowContext::default(),
        &empty_presets,
    );
    text(&mut d, &p, "StrFolder\ndf");
    assert_eq!(d.view(&p).field, DraftField::SampleFolder);
    text(&mut d, &p, "./strings\n");
    let WorkflowPlan::Instrument(plan) = finish(&mut d, &p) else {
        panic!("expected instrument plan");
    };
    assert_eq!(
        plan.sound,
        SoundChoice::Folder {
            path: "./strings".into(),
            label: "strings".into(),
        }
    );

    // 5. Ex commands `:instrument` cover all Rack templates cleanly + `:part new` + JSON serde.
    let save_path = dir.path().join("project.json");
    let lib = SampleLibrary::from_roots_only(&[], Some(&save_path));
    let mut app = App::new_with_library(Project::empty(), save_path, lib).unwrap();

    app.execute_command("instrument Lead synth pulse_lead")
        .unwrap();
    app.execute_command("instrument BlankSamp sampler").unwrap();
    app.execute_command("instrument BlankKitInst kit").unwrap();
    app.execute_command("instrument BlankMod modular").unwrap();
    app.execute_command("instrument Kit909 tr909").unwrap();
    app.execute_command("instrument StrKit folder ./strings")
        .unwrap();
    app.execute_command("part new DrumPart edm_kit 0 . 0 .")
        .unwrap();
    app.execute_command("part new SampPart sampler 0 . . .")
        .unwrap();

    let by_name = |name: &str| {
        app.project()
            .instruments
            .iter()
            .find(|i| i.name == name)
            .unwrap()
            .rack
            .clone()
    };
    assert_eq!(by_name("Lead").zones.len(), 1);
    assert!(!by_name("Lead").has_samplers());
    assert!(by_name("BlankSamp")
        .sampler_modules()
        .any(|(_, _, _, s)| s.path.is_none()));
    assert_eq!(by_name("BlankKitInst").zones.len(), 1);
    assert!(by_name("BlankKitInst").has_samplers());
    assert_eq!(by_name("BlankMod").zones[0].modules.len(), 1);
    assert_eq!(by_name("Kit909").zones.len(), 11);
    assert!(by_name("Kit909").has_samplers());
    assert_eq!(by_name("StrKit").zones.len(), 2);

    // Serde roundtrip of unified SoundChoice inside Action::CreateInstrument.
    let action = Action::CreateInstrument {
        name: "SerdeKit".into(),
        sound: SoundChoice::Preset("tr-808".into()),
        channel: None,
    };
    let json = serde_json::to_string(&action).unwrap();
    let roundtrip: Action = serde_json::from_str(&json).unwrap();
    app.apply(roundtrip).unwrap();
    assert!(app
        .project()
        .instruments
        .iter()
        .any(|i| i.name == "SerdeKit" && i.rack.zones.len() == 16));
}
