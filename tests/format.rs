//! Project and `TAKE_VERSION` format closure tests: strict schema validation,
//! `LiveFlags` outside undo, `LanePart` invariants, single-newline save hygiene,
//! and strict rejection of mismatched versions, missing required fields, and unknown fields.
use cadence_app::{load_document, Action, App};
use cadence_core::{
    material::{
        parse_document_json, write_document_json, Edit, LanePart, LiveFlags, PhaseMode, Placement,
        Project, VERSION,
    },
    mixer::ChannelRef,
    take::{Take, TAKE_VERSION},
};
use serde_json::{json, Value};

#[test]
fn document_round_trips_live_flags_outside_undo_and_prunes_stale_flags() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("song.json");
    let mut app = App::new(Project::demo(), path.clone()).unwrap();
    let lead = app.project().lanes[0].id;
    let ch = ChannelRef::Channel(app.project().mixer.channels[0].id);

    let rev0 = app.revision();
    app.apply(Action::SetLaneMuted {
        lane: lead,
        muted: true,
    })
    .unwrap();
    app.apply(Action::SetChannelSolo {
        channel: ch,
        solo: true,
    })
    .unwrap();
    assert_eq!(app.revision(), rev0, "flags never bump revision");
    assert!(app.dirty(), "flags mark document dirty for save");

    // Material edit + undo preserves live flags.
    app.execute_command("bpm 144").unwrap();
    assert_eq!(app.revision(), rev0 + 1);
    app.apply(Action::Undo).unwrap();
    assert_eq!(app.project().bpm, 136.0);
    assert!(app.flags().lane_muted(lead));
    assert!(app.flags().channel_solo(ch));

    // Save writes exactly one trailing newline and reloads cleanly via load_document.
    app.save_project(&path).unwrap();
    let raw_text = std::fs::read_to_string(&path).unwrap();
    assert!(raw_text.ends_with('\n'));
    assert!(
        !raw_text.ends_with("\n\n"),
        "save_project must end with a single trailing newline"
    );
    let raw: Value = serde_json::from_str(&raw_text).unwrap();
    assert_eq!(raw["version"], VERSION);
    assert!(
        raw.get("end").is_some(),
        "end rule is serialized explicitly"
    );
    assert!(raw.get("flags").is_some());
    assert!(
        raw["instruments"][0].get("rack").is_some(),
        "instruments serialize unified `rack`"
    );

    let (loaded, flags) = load_document(&path).unwrap();
    assert_eq!(&loaded, app.project());
    assert!(flags.lane_muted(lead));
    assert!(flags.channel_solo(ch));

    // Stale flags referencing non-existent lanes/channels are rejected on load.
    let mut bad = raw.clone();
    bad["flags"]["muted_lanes"] = json!([9999]);
    assert!(parse_document_json(&serde_json::to_vec(&bad).unwrap()).is_err());

    // write_document_json prunes flags for deleted lanes/channels before writing.
    let mut stale_flags = LiveFlags::default();
    stale_flags.set_lane_muted(9999, true);
    let mut buf = Vec::new();
    write_document_json(&mut buf, &loaded, &stale_flags).unwrap();
    let (_, clean_flags) = parse_document_json(&buf).unwrap();
    assert!(clean_flags.is_empty());
}

#[test]
fn launching_parts_never_bumps_revision_and_lane_part_lifecycle_holds() {
    let mut app = App::new(Project::demo(), "unused.json".into()).unwrap();
    let lead = app.project().lanes[0].id;
    let part = app.project().lanes[0].parts[0];
    let rev = app.revision();
    app.apply(Action::Play { playing: true }).unwrap();
    app.apply(Action::Launch {
        lane: lead,
        part,
        at: None,
    })
    .unwrap();
    assert_eq!(app.revision(), rev, "launching never bumps revision");
    assert!(
        !app.apply(Action::Undo).unwrap().changed,
        "launching creates no undo entry"
    );

    // Placing a part onto a lane adds it to lane.parts if absent;
    // removing the placement keeps it in lane.parts;
    // deleting the part removes it from every lane.parts in the same transaction.
    let mut project = Project::demo();
    let bass_lane = project.lanes[1].id;
    assert!(!project
        .lane(bass_lane)
        .unwrap()
        .parts
        .iter()
        .any(|p| p.part == 4));
    let new_place = Placement::new(900, 4, 0, 16);
    project
        .apply(Edit::PutPlacement {
            lane: bass_lane,
            placement: new_place,
        })
        .unwrap();
    assert!(
        project
            .lane(bass_lane)
            .unwrap()
            .parts
            .iter()
            .any(|p| p.part == 4),
        "placing a part adds it to lane.parts"
    );
    project.apply(Edit::RemovePlacement { id: 900 }).unwrap();
    assert!(
        project
            .lane(bass_lane)
            .unwrap()
            .parts
            .iter()
            .any(|p| p.part == 4),
        "removing a placement keeps the part in lane.parts"
    );

    // Deleting a part (not referenced by any placement or duck) drops it from every lane.parts.
    let mut p2 = Project::demo();
    let extra = cadence_core::material::Part::new(901, "Spare", 1, 1);
    p2.apply(Edit::PutPart { part: extra }).unwrap();
    p2.apply(Edit::AddLanePart {
        lane: bass_lane,
        entry: LanePart {
            part: 901,
            phase: PhaseMode::Restart,
            offset_steps: 0,
        },
    })
    .unwrap();
    assert!(p2
        .lane(bass_lane)
        .unwrap()
        .parts
        .iter()
        .any(|p| p.part == 901));
    p2.apply(Edit::RemovePart { id: 901 }).unwrap();
    assert!(
        p2.lanes
            .iter()
            .all(|l| !l.parts.iter().any(|p| p.part == 901)),
        "deleting a part drops it from every lane.parts"
    );
}

#[test]
fn strict_schema_rejects_wrong_versions_missing_end_and_unknown_fields() {
    let project = Project::demo();
    let base: Value = serde_json::to_value(&project).unwrap();

    // 1. Mismatched Project::VERSION is rejected.
    for bad_ver in [VERSION - 1, VERSION + 1] {
        let mut wrong = base.clone();
        wrong["version"] = json!(bad_ver);
        assert!(
            parse_document_json(&serde_json::to_vec(&wrong).unwrap()).is_err(),
            "project version {bad_ver} must be rejected (expected {VERSION})"
        );
    }

    // 2. Missing required `end` rule is rejected.
    let mut no_end = base.clone();
    no_end.as_object_mut().unwrap().remove("end");
    assert!(parse_document_json(&serde_json::to_vec(&no_end).unwrap()).is_err());

    // 3. `deny_unknown_fields` rejects unknown keys across Project, Instrument, Lane, Placement,
    //    ChannelStrip, ChannelRef, and Edit.
    let mut bad_top = base.clone();
    bad_top["unexpected_field"] = json!(1);
    assert!(parse_document_json(&serde_json::to_vec(&bad_top).unwrap()).is_err());

    let mut bad_inst = base.clone();
    bad_inst["instruments"][0]["unexpected_field"] = json!({});
    assert!(parse_document_json(&serde_json::to_vec(&bad_inst).unwrap()).is_err());

    let mut bad_lane = base.clone();
    bad_lane["lanes"][0]["unexpected_field"] = json!(false);
    assert!(parse_document_json(&serde_json::to_vec(&bad_lane).unwrap()).is_err());

    let mut bad_place = base.clone();
    bad_place["lanes"][0]["placements"][0]["unexpected_field"] = json!([]);
    assert!(parse_document_json(&serde_json::to_vec(&bad_place).unwrap()).is_err());

    let mut bad_strip = base.clone();
    bad_strip["mixer"]["channels"][0]["strip"]["unexpected_field"] = json!(false);
    assert!(parse_document_json(&serde_json::to_vec(&bad_strip).unwrap()).is_err());

    let mut bad_ref = base.clone();
    bad_ref["mixer"]["channels"][0]["output"] = json!({"kind": "unknown_kind", "id": 4});
    assert!(parse_document_json(&serde_json::to_vec(&bad_ref).unwrap()).is_err());

    assert!(serde_json::from_value::<Edit>(json!({"op": "unknown_edit_op"})).is_err());

    // 4. Mismatched TAKE_VERSION and unknown take event variants are rejected.
    let dir = tempfile::tempdir().unwrap();
    let take_path = dir.path().join("bad.take.jsonl");

    for bad_ver in [TAKE_VERSION - 1, TAKE_VERSION + 1] {
        let wrong_header = json!({
            "take": bad_ver,
            "title": "BadVersion",
            "start_step": 0,
            "active": [],
            "project": base,
        });
        std::fs::write(&take_path, format!("{wrong_header}\n{{\"end\":16}}\n")).unwrap();
        let err = Take::load(&take_path).expect_err("wrong take version must be rejected");
        assert!(
            format!("{err:#}").contains(&format!("Unsupported take version {bad_ver}")),
            "take version {bad_ver} error must report version mismatch, got: {err:#}"
        );
    }

    let valid_header = json!({
        "take": TAKE_VERSION,
        "title": "Current",
        "start_step": 0,
        "active": [],
        "project": base,
    });
    let bad_event = json!({"at": 0, "label": "bad", "change": {"op": "unknown_change"}});
    std::fs::write(
        &take_path,
        format!("{valid_header}\n{bad_event}\n{{\"end\":16}}\n"),
    )
    .unwrap();
    assert!(
        Take::load(&take_path).is_err(),
        "unknown take event op must be rejected"
    );
}
