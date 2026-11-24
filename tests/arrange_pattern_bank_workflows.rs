//! End-to-end integration tests for Bank row-oriented Launchpad CRUD, Pattern/Part/Arrange
//! editing & deletion workflows, Follow suspension on Tier 3/4 keys, Live transport controls,
//! Take deck file flows, and File Hub project/import/export semantics.
use cadence_app::view::{DeckView, OverlayView};
use cadence_app::{
    Action, App, Detail, EditScope, FileDialogRequest, FileHubTab, KeyCode, KeyEvent, KeyModifiers,
};
use cadence_core::follow::FollowMode;
use cadence_core::material::{PatternKind, PhaseMode, Project};
use cadence_core::music::StepCondition;
use cadence_core::params::ValueMode;
use cadence_core::runtime::AudioSnapshot;
use cadence_core::sampler::SampleLibrary;
use std::path::PathBuf;

fn hermetic_app(project: Project, path: PathBuf) -> App {
    let lib = SampleLibrary::from_roots(&[], Some(&path));
    App::new_with_library(project, path, lib).unwrap()
}

fn snap_view(app: &App) -> cadence_app::view::FrameView {
    let snap = AudioSnapshot {
        playing: app.playing(),
        absolute_step: app.transport_step(),
        sample_rate: 48_000,
        ..AudioSnapshot::default()
    };
    app.frame_view(&snap, "Test · 48 kHz")
}

#[test]
fn bank_launchpad_crud_adds_edits_phase_drops_and_immediate_launches() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = hermetic_app(Project::demo(), dir.path().join("bank_crud.json"));

    // Focus Bank Parts (`gb`).
    app.press_keys("gb").unwrap();
    let lane0_id = app.project().lanes[0].id;
    let initial_parts = app.project().lanes[0].parts.len();
    assert!(initial_parts >= 1);

    // `a` opens the Bank Add-Part Picker listing existing Parts not yet on lane 0 + `[+] Create new Part…`.
    app.press_keys("a").unwrap();
    assert_eq!(app.mode_name(), "PICK");
    let picker = match snap_view(&app).overlay {
        Some(OverlayView::Picker(p)) => p,
        other => panic!("expected Picker overlay on Bank Parts `a`, got {other:?}"),
    };
    assert!(
        picker
            .items
            .iter()
            .any(|(name, _, _)| name.contains("Create new Part")),
        "picker must offer a Create new Part entry"
    );
    // Select first existing Part (e.g., Bass) with Enter -> added to lane 0!
    app.press_keys("<Enter>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert_eq!(
        app.project().lanes[0].parts.len(),
        initial_parts + 1,
        "selecting an existing Part in Bank picker appends it to lane.parts"
    );

    // `r` edits the focused LanePart's `phase` and `offset_steps` via modal workflow.
    app.press_keys("r").unwrap();
    assert_eq!(app.mode_name(), "WORKFLOW");
    // Toggle Phase from Song -> Restart (`j<Enter>`), then enter offset `4<Enter>`.
    app.press_keys("j<Enter>4<Enter>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    let edited_slot = app.view_state.bank_part_idx;
    let lp = app.project().lanes[0].parts[edited_slot];
    assert_eq!(lp.phase, PhaseMode::Restart);
    assert_eq!(lp.offset_steps, 4);

    // BankDeckView exposes `phase` & `offset_steps` on `BankPartEntry` for GPU card badges.
    let fv = snap_view(&app);
    let bank_view = fv
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Bank(b) => Some(b),
            _ => None,
        })
        .expect("Bank deck view");
    let rendered_entry = bank_view.lanes[0]
        .parts
        .iter()
        .find(|p| p.part_id == lp.part)
        .expect("added part rendered on lane 0");
    assert_eq!(rendered_entry.phase, PhaseMode::Restart);
    assert_eq!(rendered_entry.offset_steps, 4);

    // `Shift+Enter` while playing launches the focused Part immediately (`Boundary::Now`, at current step 0).
    app.press_keys(" ").unwrap();
    assert!(app.playing());
    app.press_keys("<s-enter>").unwrap();
    let queued_state = app.live_state_json();
    assert!(
        queued_state["pending"]
            .as_array()
            .is_some_and(|arr| arr.iter().any(|p| p["at"] == 0)),
        "Shift+Enter must schedule at step 0 (Boundary::Now): {queued_state}"
    );
    app.apply(Action::Advance { steps: 1 }).unwrap();
    let live_state = app.live_state_json();
    assert!(
        live_state["active"].as_array().is_some_and(|arr| arr
            .iter()
            .any(|o| o["lane"] == lane0_id && o["part"]["part"] == lp.part)),
        "Shift+Enter must land an immediate live override: {live_state}"
    );
    app.press_keys("<Home>").unwrap();

    // `X` drops the focused Part from `lane.parts` while keeping the `Part` definition intact.
    let part_count_before = app.project().parts.len();
    app.press_keys("X").unwrap();
    assert_eq!(app.project().lanes[0].parts.len(), initial_parts);
    assert_eq!(
        app.project().parts.len(),
        part_count_before,
        "Bank X drops from lane.parts without deleting the underlying Part"
    );

    // Empty project guard: with 0 lanes in Bank Parts, navigation/drop keys do not leak to Material.
    let mut empty = hermetic_app(Project::empty(), dir.path().join("empty_bank.json"));
    empty.press_keys("gb").unwrap();
    empty.press_keys("jklhX").unwrap();
    assert_eq!(empty.mode_name(), "NORMAL");
    empty.press_keys("a").unwrap();
    assert_eq!(empty.mode_name(), "WORKFLOW");
}

#[test]
fn pattern_step_conditions_and_placement_reinherit_and_clear_all_exceptions() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = hermetic_app(Project::demo(), dir.path().join("pat_cond_exc.json"));

    // 1. `c` / `C` on literal Notes steps cycle `StepCondition` forward & backward.
    let motif_id = app
        .project()
        .patterns
        .iter()
        .find(|p| matches!(p.kind, PatternKind::Notes { .. }))
        .unwrap()
        .id;
    app.apply(Action::SelectPattern { pattern: motif_id })
        .unwrap();
    app.press_keys("gp0").unwrap();
    let step0_before = match &app.pattern(motif_id).unwrap().kind {
        PatternKind::Notes { steps } => steps[0].clone(),
        _ => unreachable!(),
    };
    assert!(step0_before.active);
    assert_eq!(step0_before.condition, StepCondition::ALWAYS);

    app.press_keys("c").unwrap();
    let step0_fwd = match &app.pattern(motif_id).unwrap().kind {
        PatternKind::Notes { steps } => steps[0].condition,
        _ => unreachable!(),
    };
    assert_eq!(step0_fwd, StepCondition::ALWAYS.cycle(1));

    app.press_keys("C").unwrap();
    let step0_back = match &app.pattern(motif_id).unwrap().kind {
        PatternKind::Notes { steps } => steps[0].condition,
        _ => unreachable!(),
    };
    assert_eq!(step0_back, StepCondition::ALWAYS);

    // 2. `Backspace` in `EditScope::Placement` re-inherits the focused step cleanly.
    let pl_id = app.project().lanes[0].placements[0].id;
    app.execute_command(&format!("local {pl_id}")).unwrap();
    assert_eq!(app.scope, EditScope::Placement(pl_id));
    app.press_keys("0").unwrap();
    // Create two local exceptions at steps 0 and 1.
    app.press_keys("<m-k>l<m-k>").unwrap();
    assert_eq!(app.placement(pl_id).unwrap().1.exceptions.len(), 2);

    // `Backspace` on step 1 removes only step 1's exception while staying in Placement scope.
    app.press_keys("<Backspace>").unwrap();
    assert_eq!(app.scope, EditScope::Placement(pl_id));
    let remaining = &app.placement(pl_id).unwrap().1.exceptions;
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].step, 0);

    // Pressing `Backspace` again on an already-inherited step keeps scope cleanly.
    app.press_keys("<Backspace>").unwrap();
    assert_eq!(app.scope, EditScope::Placement(pl_id));

    // 3. `X` on `Detail::Placement` clears all remaining local exceptions (undoable with `u`).
    app.press_keys("<Esc>").unwrap();
    app.press_keys("ga").unwrap();
    app.apply(Action::SelectPlacement { placement: pl_id })
        .unwrap();
    assert_eq!(app.detail, Detail::Placement(pl_id));
    app.press_keys("X").unwrap();
    assert!(app.placement(pl_id).unwrap().1.exceptions.is_empty());
    app.press_keys("u").unwrap();
    assert_eq!(app.placement(pl_id).unwrap().1.exceptions.len(), 1);
}

#[test]
fn literal_pattern_metadata_form_rename_commands_freeze_fork_and_inspect() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = hermetic_app(Project::demo(), dir.path().join("pat_meta.json"));

    // 1. `r` on literal Notes edits Name & Length while preserving active note steps.
    let motif_id = app
        .project()
        .patterns
        .iter()
        .find(|p| p.name == "Motif")
        .unwrap()
        .id;
    let first_degree = match &app.pattern(motif_id).unwrap().kind {
        PatternKind::Notes { steps } => steps[0].degree,
        _ => unreachable!(),
    };
    app.apply(Action::SelectPattern { pattern: motif_id })
        .unwrap();
    app.press_keys("gpr").unwrap();
    assert_eq!(app.mode_name(), "AUTHORING");
    app.press_keys("LeadMotif<Enter>8<Enter>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    let updated = app.pattern(motif_id).unwrap();
    assert_eq!(updated.name, "LeadMotif");
    match &updated.kind {
        PatternKind::Notes { steps } => {
            assert_eq!(steps.len(), 8);
            assert_eq!(steps[0].degree, first_degree);
        }
        other => panic!("expected literal Notes, got {other:?}"),
    }

    // 2. `r` on normalized Values edits Name, Period, and ValueMode (`Hold`/`Ramp`) while preserving points.
    let energy_id = app
        .project()
        .patterns
        .iter()
        .find(|p| p.name == "Energy")
        .unwrap()
        .id;
    app.apply(Action::SelectPattern { pattern: energy_id })
        .unwrap();
    app.press_keys("gpr").unwrap();
    assert_eq!(app.mode_name(), "AUTHORING");
    // Keep name (`<Enter>`), set period 32 (`32<Enter>`), switch ValueMode to Hold (`h`).
    app.press_keys("<Enter>32<Enter>h").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    match &app.pattern(energy_id).unwrap().kind {
        PatternKind::Values {
            period,
            mode,
            points,
            ..
        } => {
            assert_eq!(*period, 32);
            assert_eq!(*mode, ValueMode::Hold);
            assert!(!points.is_empty());
        }
        other => panic!("expected literal Values, got {other:?}"),
    }

    // 3. `:rename pattern|part|lane` Ex commands update names cleanly.
    let part0_id = app.project().parts[0].id;
    let lane0_id = app.project().lanes[0].id;
    app.execute_command(&format!("rename pattern {motif_id} RenamedMotif"))
        .unwrap();
    assert_eq!(app.pattern(motif_id).unwrap().name, "RenamedMotif");
    app.execute_command(&format!("rename part {part0_id} MainIntro"))
        .unwrap();
    assert_eq!(app.part(part0_id).unwrap().name, "MainIntro");
    app.execute_command(&format!("rename lane {lane0_id} LeadRow"))
        .unwrap();
    assert_eq!(app.lane(lane0_id).unwrap().name, "LeadRow");

    // 4. `f` standalone Fork and `I` Mode::Inspect from `gp`.
    let pat_count = app.project().patterns.len();
    app.apply(Action::SelectPattern { pattern: motif_id })
        .unwrap();
    app.press_keys("gpf").unwrap();
    assert_eq!(app.project().patterns.len(), pat_count + 1);
    app.press_keys("I").unwrap();
    assert_eq!(app.mode_name(), "INSPECT");
    assert!(matches!(
        snap_view(&app).overlay,
        Some(OverlayView::Inspect { .. })
    ));
    app.press_keys("<Esc>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");

    // 5. `F` freezes a derived pattern via modal workflow into literal steps.
    let derived_id = app
        .project()
        .patterns
        .iter()
        .find(|p| !p.kind.sources().is_empty())
        .unwrap()
        .id;
    let count_before_freeze = app.project().patterns.len();
    app.apply(Action::SelectPattern {
        pattern: derived_id,
    })
    .unwrap();
    app.press_keys("gpF").unwrap();
    assert_eq!(app.mode_name(), "WORKFLOW");
    // Accept prefilled Name, Start (`0`), Length (`16<Enter>`).
    app.press_keys("<Enter><Enter>16<Enter>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert_eq!(app.project().patterns.len(), count_before_freeze + 1);
    let frozen_id = app.selected_pattern.unwrap();
    assert!(
        matches!(app.pattern(frozen_id).unwrap().kind, PatternKind::Notes { ref steps } if steps.len() == 16),
        "frozen derived Notes pattern must become a 16-step literal Notes pattern"
    );
}

#[test]
fn confirm_delete_for_pattern_part_and_lane_cascades_and_undoes_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = hermetic_app(Project::demo(), dir.path().join("confirm_delete.json"));

    // 1. Delete Pattern with dependencies from Bank Material (`gb` -> `v` -> `X` -> `y`), undo restores all.
    let before_proj = app.project().clone();
    let motif_id = before_proj
        .patterns
        .iter()
        .find(|p| p.name == "Motif")
        .unwrap()
        .id;
    app.apply(Action::SelectPattern { pattern: motif_id })
        .unwrap();
    app.press_keys("gbv").unwrap(); // Bank Material mode
    app.press_keys("X").unwrap();
    assert_eq!(app.mode_name(), "CONFIRM");
    let confirm_msg = match snap_view(&app).overlay {
        Some(OverlayView::Confirm { title, message }) => {
            assert_eq!(title, "DELETE PATTERN");
            message
        }
        other => panic!("expected Confirm overlay on Material X, got {other:?}"),
    };
    assert!(confirm_msg.contains("Motif"));
    app.press_keys("y").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert!(app.pattern(motif_id).is_none());
    app.press_keys("u").unwrap();
    assert_eq!(app.project().patterns.len(), before_proj.patterns.len());
    assert_eq!(app.project().parts.len(), before_proj.parts.len());

    // 2. Delete Part from `Detail::Part` (`X` -> `y`) removes dependent placements & lane.parts, undo restores.
    let part_id = app.project().parts[0].id;
    let placements_before: usize = app.project().lanes.iter().map(|l| l.placements.len()).sum();
    app.press_keys("ga").unwrap();
    let pl0 = app.project().lanes[0].placements[0].id;
    app.apply(Action::SelectPlacement { placement: pl0 })
        .unwrap();
    // Row 0 on Detail::Placement is Part -> press Enter to open Detail::Part.
    app.press_keys("<Enter>").unwrap();
    assert_eq!(app.detail, Detail::Part(part_id));
    app.press_keys("X").unwrap();
    assert_eq!(app.mode_name(), "CONFIRM");
    app.press_keys("y").unwrap();
    assert!(app.part(part_id).is_none());
    let placements_after: usize = app.project().lanes.iter().map(|l| l.placements.len()).sum();
    assert!(placements_after < placements_before);
    app.press_keys("u").unwrap();
    assert!(app.part(part_id).is_some());

    // 3. Delete Lane from Arrange (`ga`) when NO section is under cursor (`X` -> `y`).
    app.execute_command("bars 24").unwrap();
    app.press_keys("ga$").unwrap(); // Move past both Seed (bars 1-4) and Motion (bars 5-16) sections.
    let lane_count = app.project().lanes.len();
    let del_lane_id = app.selected_lane.unwrap();
    app.press_keys("X").unwrap();
    assert_eq!(app.mode_name(), "CONFIRM");
    app.press_keys("y").unwrap();
    assert_eq!(app.project().lanes.len(), lane_count - 1);
    assert!(app.lane(del_lane_id).is_none());
    app.press_keys("u").unwrap();
    assert_eq!(app.project().lanes.len(), lane_count);
}

#[test]
fn detail_part_rebind_rename_exact_values_and_lane_rename_reorder() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = hermetic_app(Project::demo(), dir.path().join("part_and_lane.json"));

    // 1. Open Detail::Part for part 0.
    let pl0 = app.project().lanes[0].placements[0].id;
    let part0 = app.project().lanes[0].placements[0].part;
    let orig_pat = app.part(part0).unwrap().pattern;
    let orig_inst = app.part(part0).unwrap().instrument;
    app.apply(Action::SelectPlacement { placement: pl0 })
        .unwrap();
    app.press_keys("<Enter>").unwrap();
    assert_eq!(app.detail, Detail::Part(part0));

    // Row 0 = Pattern -> `r` opens Rebind Pattern picker, `j<Enter>` rebinds to another Notes pattern.
    app.press_keys("r").unwrap();
    assert_eq!(app.mode_name(), "PICK");
    app.press_keys("j<Enter>").unwrap();
    assert_ne!(app.part(part0).unwrap().pattern, orig_pat);

    // Row 1 = Instrument -> `r` opens Rebind Instrument picker, `j<Enter>` rebinds instrument.
    app.press_keys("jr").unwrap();
    assert_eq!(app.mode_name(), "PICK");
    app.press_keys("j<Enter>").unwrap();
    assert_ne!(app.part(part0).unwrap().instrument, orig_inst);

    // Row 2 = Transpose -> `Enter` exact ValueInput (`+12<Enter>`), `r` renames Part.
    app.press_keys("j<Enter>+12<Enter>").unwrap();
    assert_eq!(app.part(part0).unwrap().transpose, 12);
    app.press_keys("rLeadPartRenamed<Enter>").unwrap();
    assert_eq!(app.part(part0).unwrap().name, "LeadPartRenamed");

    // Row 3 = Velocity -> `Enter` exact ValueInput (`75%<Enter>`).
    app.press_keys("j<Enter>75%<Enter>").unwrap();
    assert!((app.part(part0).unwrap().velocity_scale - 0.75).abs() < 1e-4);

    // 2. Arrange `r` when no section is under cursor renames focused Lane; `Alt+j` reorders Lane!
    app.execute_command("bars 24").unwrap();
    app.press_keys("ga$").unwrap();
    let lane0_id = app.project().lanes[0].id;
    let lane1_id = app.project().lanes[1].id;
    app.selected_lane = Some(lane0_id);
    app.press_keys("rSynthLead<Enter>").unwrap();
    assert_eq!(app.lane(lane0_id).unwrap().name, "SynthLead");

    // On empty bar past placements, `Alt+j` moves lane 0 down to row 1 via Edit::ReorderLane.
    app.press_keys("<m-j>").unwrap();
    assert_eq!(app.project().lanes[0].id, lane1_id);
    assert_eq!(app.project().lanes[1].id, lane0_id);

    // On Bank Parts, `Alt+k` moves focused lane back up to row 0!
    app.press_keys("gb<m-k>").unwrap();
    assert_eq!(app.project().lanes[0].id, lane0_id);
}

#[test]
fn follow_suspends_on_modeless_tier3_tier4_and_live_transport_cue_and_cancel_all() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = hermetic_app(Project::demo(), dir.path().join("follow_and_live.json"));

    // 1. Follow suspends cleanly on Tier 4 `,` and Tier 3 `Alt+k` on Pattern canvas.
    let motif_id = app
        .project()
        .patterns
        .iter()
        .find(|p| matches!(p.kind, PatternKind::Notes { .. }))
        .unwrap()
        .id;
    app.apply(Action::SelectPattern { pattern: motif_id })
        .unwrap();
    app.press_keys("gp ").unwrap();
    app.apply(Action::SetFollow { enabled: true }).unwrap();
    assert_eq!(app.follow_mode(), FollowMode::Active);
    app.press_keys(".").unwrap();
    assert_eq!(
        app.follow_mode(),
        FollowMode::Suspended,
        "Tier 4 `.` scalar adjustment on Pattern must suspend Follow"
    );

    app.apply(Action::SetFollow { enabled: true }).unwrap();
    assert_eq!(app.follow_mode(), FollowMode::Active);
    app.press_keys("<m-k>").unwrap();
    assert_eq!(
        app.follow_mode(),
        FollowMode::Suspended,
        "Tier 3 `Alt+k` pitch edit on Pattern must suspend Follow"
    );

    // 2. Normal `Backspace` cancels a queued lane launch AND `tX` cancels all pending live changes.
    let pending_base = app.live_state_json()["pending"].as_array().unwrap().len();
    let lane0_id = app.project().lanes[0].id;
    let lane1_id = app.project().lanes[1].id;
    let lp0 = app.project().lanes[0].parts[0];
    let lp1 = app.project().lanes[1].parts[0];
    app.apply(Action::Launch {
        lane: lane0_id,
        part: lp0,
        at: None,
    })
    .unwrap();
    assert_eq!(
        app.live_state_json()["pending"].as_array().unwrap().len(),
        pending_base + 1
    );
    app.press_keys("<Backspace>").unwrap();
    assert_eq!(
        app.live_state_json()["pending"].as_array().unwrap().len(),
        pending_base,
        "Normal Backspace must cancel queued lane launches"
    );

    // Queue two lane launches, then cancel all at once via `tX`.
    app.apply(Action::Launch {
        lane: lane0_id,
        part: lp0,
        at: None,
    })
    .unwrap();
    app.apply(Action::Launch {
        lane: lane1_id,
        part: lp1,
        at: None,
    })
    .unwrap();
    assert_eq!(
        app.live_state_json()["pending"].as_array().unwrap().len(),
        pending_base + 2
    );
    app.press_keys("tX").unwrap();
    assert_eq!(
        app.live_state_json()["pending"].as_array().unwrap().len(),
        pending_base,
        "tX must cancel all pending live changes"
    );

    // 3. `tg` opens Cue-to-Bar ValueInput and executes Action::CueBar cleanly.
    app.press_keys("<Home>").unwrap();
    app.press_keys("tg").unwrap();
    assert_eq!(app.mode_name(), "VALUE ENTRY");
    app.press_keys("5<Enter>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert_eq!(
        app.song_step_heard(),
        64,
        "tg 5<Enter> must cue song position to bar 5 (step 64)"
    );
}

#[test]
fn take_deck_save_load_shortcuts_and_file_hub_new_project_and_export_copy() {
    let dir = tempfile::tempdir().unwrap();
    let takes_dir = dir.path().join("takes");
    let mut app = hermetic_app(Project::demo(), dir.path().join("take_hub.json"));
    app.set_session_takes_dir(takes_dir);

    // Record a short take so Save Take has data.
    app.press_keys("tr ").unwrap();
    app.apply(Action::Advance { steps: 16 }).unwrap();
    app.execute_command("bpm 140").unwrap();
    app.apply(Action::Advance { steps: 16 }).unwrap();
    app.press_keys("tr<Home>gt").unwrap();
    assert!(app.take_focused());

    // Bare `s` on Take deck must NOT save the take (Ctrl+s is Save Take).
    let saved_dir = dir.path().join("takes");
    let count_takes = || -> usize {
        std::fs::read_dir(&saved_dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".take.jsonl"))
            .count()
    };
    let takes_before = count_takes();
    app.press_keys("s").unwrap();
    assert_eq!(
        count_takes(),
        takes_before,
        "bare `s` in Take deck must not trigger SaveTake"
    );

    // `Ctrl+s` saves the take directly into `takes/`.
    app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .unwrap();
    assert!(
        app.status.starts_with("Saved take:"),
        "Ctrl+s in Take deck must save the active take: {}",
        app.status
    );
    assert_eq!(
        count_takes(),
        takes_before + 1,
        "Ctrl+s must write a new .take.jsonl file"
    );

    // `Ctrl+Shift+s` on Take deck opens File Hub Save tab pre-positioned on row 5 (`Save Active Live Take`).
    app.handle_key(KeyEvent::new(
        KeyCode::Char('S'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    ))
    .unwrap();
    assert_eq!(app.mode_name(), "FILES");
    match snap_view(&app).overlay {
        Some(OverlayView::FileHub(hub)) => {
            assert_eq!(hub.active_tab, FileHubTab::Save);
            assert_eq!(
                hub.cursor, 5,
                "Take Ctrl+Shift+s must focus Save Active Live Take row"
            );
        }
        other => panic!("expected FileHub Save overlay, got {other:?}"),
    }
    app.press_keys("<Esc>").unwrap();

    // `o` on Take deck opens File Hub Open tab focused on Takes.
    app.press_keys("o").unwrap();
    assert_eq!(app.mode_name(), "FILES");
    match snap_view(&app).overlay {
        Some(OverlayView::FileHub(hub)) => {
            assert_eq!(hub.active_tab, FileHubTab::Open);
        }
        other => panic!("expected FileHub Open overlay on Take `o`, got {other:?}"),
    }

    // `"New Blank Project"` row sits at the top of File Hub Open tab and respects `dirty()` Confirm guard!
    app.press_keys("<Esc>gf").unwrap();
    let fh = match snap_view(&app).overlay {
        Some(OverlayView::FileHub(hub)) => hub,
        other => panic!("expected FileHub Open overlay, got {other:?}"),
    };
    assert_eq!(fh.rows[0].label, "New Blank Project");
    assert_eq!(fh.rows[0].badge, "NEW");
    assert!(app.dirty());
    app.press_keys("<Enter>").unwrap();
    assert_eq!(
        app.mode_name(),
        "CONFIRM",
        "New Blank Project while dirty must trigger Confirm guard"
    );
    app.press_keys("y").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert!(app.project().patterns.is_empty());

    // File Hub Import tab `F` always emits unified `FileDialogRequest::ImportAsset`.
    app.open_file_hub(FileHubTab::Import);
    let dlg = app
        .handle_key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT))
        .unwrap();
    assert_eq!(dlg.file_dialog, Some(FileDialogRequest::ImportAsset));
    app.press_keys("<Esc>").unwrap();

    // Export Palette entry advertises 16-bit PCM WAV (never 24-bit or 32-bit float WAV output).
    app.press_keys(":Export WAV").unwrap();
    let pal = match snap_view(&app).overlay {
        Some(OverlayView::Palette(p)) => p,
        other => panic!("expected Palette overlay, got {other:?}"),
    };
    let export_item = pal
        .items
        .iter()
        .find(|it| it.label == "Export WAV / Stems")
        .expect("Export WAV / Stems palette item");
    assert!(
        export_item.detail.contains("16-bit PCM"),
        "Export palette detail must advertise 16-bit PCM, got: {}",
        export_item.detail
    );
    assert!(
        !export_item.detail.contains("24-bit") && !export_item.detail.contains("32-bit"),
        "Export palette detail must not claim 24-bit or 32-bit WAV output: {}",
        export_item.detail
    );
}
