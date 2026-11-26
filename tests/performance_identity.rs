use cadence_app::keys::{KeyCode, KeyEvent, KeyModifiers};
use cadence_app::{Action, App};
use cadence_core::{
    material::{Edit, PatternKind, Project},
    music::Step,
};
fn app() -> App {
    let mut a = App::new_with_library(
        Project::empty(),
        "unused.json".into(),
        cadence_core::sampler::SampleLibrary::from_roots_only(&[], None),
    )
    .unwrap();
    a.execute_command("part new Tone init 0").unwrap();
    a
}
fn hold_note(a: &mut App) {
    if !a.is_jam() {
        a.press_keys("gj").unwrap();
    }
    let out = a
        .handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(out.live_notes_on.len(), 1);
}
fn pattern() -> Action {
    Action::CreatePattern {
        name: "Another".into(),
        kind: PatternKind::Notes {
            steps: vec![Step::default()],
        },
    }
}
#[test]
fn ids_are_never_reused_after_undo_or_loaded_project_history() {
    let mut a = app();
    a.apply(pattern()).unwrap();
    let id = a.project().patterns.last().unwrap().id;
    a.apply(Action::Undo).unwrap();
    a.apply(pattern()).unwrap();
    assert!(a.project().patterns.last().unwrap().id > id);
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("other.json");
    let mut other = Project::empty();
    other.patterns.push(cadence_core::material::Pattern {
        id: 1000,
        name: "High".into(),
        kind: PatternKind::Notes {
            steps: vec![Step::default()],
        },
    });
    std::fs::write(&path, serde_json::to_vec(&other).unwrap()).unwrap();
    a.apply(Action::Load { path }).unwrap();
    a.apply(Action::Undo).unwrap();
    a.apply(Action::CreatePattern {
        name: "After load".into(),
        kind: PatternKind::Notes {
            steps: vec![Step::default()],
        },
    })
    .unwrap();
    assert!(a.project().patterns.last().unwrap().id > 1000);
}
#[test]
fn document_boundaries_release_input_and_reset_but_ordinary_edits_preserve_transport() {
    let mut a = app();
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("same-ids.json");
    let mut other = a.project().clone();
    other.title = "Other project".into();
    std::fs::write(&path, serde_json::to_vec(&other).unwrap()).unwrap();
    hold_note(&mut a);
    let load = a.apply(Action::Load { path: path.clone() }).unwrap();
    assert!(load.changed && load.project_boundary);
    assert_eq!(load.live_notes_off.len(), 1);
    assert!(a.release_held().live_notes_off.is_empty());
    assert!(load
        .transport
        .contains(&cadence_core::runtime::TransportIntent::Reset { step: 0 }));
    a.apply(Action::Edit {
        edit: Edit::SetTempo { bpm: 140.0 },
    })
    .unwrap();
    let ordinary = a.apply(Action::Undo).unwrap();
    assert!(!ordinary.project_boundary);
    hold_note(&mut a);
    let across = a.apply(Action::Undo).unwrap();
    assert!(across.project_boundary);
    assert_eq!(across.live_notes_off.len(), 1);
    assert!(across
        .transport
        .contains(&cadence_core::runtime::TransportIntent::Reset { step: 0 }));
    assert!(a.apply(Action::Redo).unwrap().project_boundary);
    assert!(a.apply(Action::Load { path }).unwrap().project_boundary);
}
#[test]
fn rejected_load_and_exhausted_ids_leave_score_and_input_unchanged() {
    let mut a = app();
    hold_note(&mut a);
    let before = a.project().clone();
    let revision = a.revision();
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("bad.json");
    std::fs::write(&path, b"{invalid}").unwrap();
    assert!(a.apply(Action::Load { path }).is_err());
    assert_eq!(a.project(), &before);
    assert_eq!(a.revision(), revision);
    assert_eq!(a.release_held().live_notes_off.len(), 1);
    let mut p = Project::empty();
    p.patterns.push(cadence_core::material::Pattern {
        id: u64::MAX,
        name: "Last".into(),
        kind: PatternKind::Notes {
            steps: vec![Step::default()],
        },
    });
    let mut exhausted = App::new_in_memory(p.clone(), "unused.json".into()).unwrap();
    assert!(exhausted.apply(pattern()).is_err());
    assert_eq!(exhausted.project(), &p);
}
