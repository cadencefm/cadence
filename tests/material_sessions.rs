use cadence_app::{load_project, Action, App};
use cadence_core::{
    material::{Edit, Part, PatternKind, Project, ValuePoint, ValueUnit},
    music::Step,
    params::ValueMode,
};
use std::{fs, path::PathBuf, process::Command};

#[test]
fn modal_keys_and_typed_actions_build_the_same_retained_project() {
    let mut keys = App::new(Project::empty(), "unused.json".into()).unwrap();
    let mut api = App::new(Project::empty(), "unused.json".into()).unwrap();
    keys.press_keys(":bars 8<Enter>:part new Lead pulse_lead 0 4 2 6 7<Enter>:pattern degrees Harmony 64 hold 0:0 24:-2 32:2 56:-1<Enter>:derive Moving add \"Lead motif\" Harmony<Enter>:part Lead pattern Moving<Enter>").unwrap();
    for action in [
        Action::Edit {
            edit: Edit::SetBars { bars: 8 },
        },
        Action::PlacedPart {
            name: "Lead".into(),
            sound: cadence_app::workflow::SoundChoice::Preset("pulse-lead".into()),
            start_step: 0,
            length_steps: Some(128),
            transpose: None,
            velocity_scale: None,
            notes: [0, 4, 2, 6, 7]
                .map(|degree| Step {
                    active: true,
                    degree: degree + 1,
                    velocity: 0.8,
                    gate: 0.75,
                    ..Step::default()
                })
                .into(),
        },
        Action::CreatePattern {
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
        Action::CreatePattern {
            name: "Moving".into(),
            kind: PatternKind::AddDegrees {
                notes: 1,
                values: 7,
            },
        },
        Action::Edit {
            edit: Edit::PutPart {
                part: Part::new(5, "Lead", 8, 3),
            },
        },
    ] {
        api.apply(action).unwrap();
    }
    assert_eq!(keys.project(), api.project());
    assert_eq!(keys.revision(), api.revision());
    keys.press_keys(
        ":local 6<Enter>:note 128 7 85% 1.5<Enter>:open Harmony<Enter>:value 25 -3<Enter>",
    )
    .unwrap();
    api.apply(Action::Edit {
        edit: Edit::SetException {
            placement: 6,
            step: 127,
            value: Some(Step {
                active: true,
                degree: 8,
                velocity: 0.85,
                gate: 1.5,
                ..Step::default()
            }),
        },
    })
    .unwrap();
    api.apply(Action::Edit {
        edit: Edit::SetValue {
            pattern: 7,
            step: 24,
            value: Some(-3.0),
        },
    })
    .unwrap();
    assert_eq!(keys.project(), api.project());
    assert_eq!(
        keys.project().compile().unwrap().provenance,
        api.project().compile().unwrap().provenance
    );
}

#[test]
fn shipped_key_sessions_rebuild_the_checked_in_authored_documents() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let temp = tempfile::tempdir().unwrap();
    let name = "material-lab";
    {
        let folder = repo.join("examples").join(name);
        let output = temp.path().join(format!("{name}.json"));
        let result = Command::new(env!("CARGO_BIN_EXE_cadence"))
            .current_dir(&folder)
            .args([
                "--empty",
                "--sample-roots-only",
                "--key-script",
                "build.keys.jsonl",
                "--check",
                "--output",
            ])
            .arg(&output)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        let rebuilt = load_project(&output).unwrap();
        let shipped = App::new(
            load_project(&folder.join("project.json")).unwrap(),
            folder.join("project.json"),
        )
        .unwrap();
        let comparison = temp.path().join(format!("{name}-expected.json"));
        shipped.save_project(&comparison).unwrap();
        assert_eq!(
            rebuilt,
            load_project(&comparison).unwrap(),
            "{name} source/session drift"
        );
        assert!(!fs::read_to_string(output).unwrap().contains("FL Studio"));
        // Saving elsewhere rebases the original kit; it never copies a library.
        assert!(!temp.path().join("samples").exists());
    }
}
