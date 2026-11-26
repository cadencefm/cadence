//! Performing over the arrangement through the real application session.
use cadence_app::{parse_key_sequence, Action, App};
use cadence_core::{
    material::{Boundary, Edit, Id, LanePart, PhaseMode, Project},
    take::Take,
};
use cadence_engine::audio::render_schedule_report;

fn app() -> App {
    let mut app = App::new_with_library(
        Project::demo(),
        "unused.json".into(),
        cadence_core::sampler::SampleLibrary::from_roots_only(&[], None),
    )
    .unwrap();
    app.execute_command("lane Drums add Kick restart").unwrap();
    app
}

fn lane(app: &App, name: &str) -> Id {
    app.project()
        .lanes
        .iter()
        .find(|l| l.name == name)
        .unwrap()
        .id
}

fn lane_part(app: &App, lane_name: &str, index: usize) -> LanePart {
    app.project().lane(lane(app, lane_name)).unwrap().parts[index]
}

fn part(app: &App, name: &str) -> Id {
    app.project()
        .parts
        .iter()
        .find(|b| b.name == name)
        .unwrap()
        .id
}

fn playing_live(app: &mut App) {
    app.apply(Action::Play { playing: true }).unwrap();
}

fn advance(app: &mut App, steps: u64) {
    app.apply(Action::Advance { steps }).unwrap();
}

fn keys(app: &mut App, sequence: &str) {
    for key in parse_key_sequence(sequence) {
        app.handle_script_key(key).unwrap();
    }
}

#[test]
fn mute_and_solo_are_live_state_outside_undo_and_revision() {
    let mut app = app();
    let lead = lane(&app, "Lead");
    let revision = app.revision();
    app.apply(Action::SetLaneMuted {
        lane: lead,
        muted: true,
    })
    .unwrap();
    assert_eq!(app.revision(), revision, "mutes are not song revisions");
    assert!(app.flags().lane_muted(lead));
    assert!(app.dirty(), "the saved project keeps how you left it");
    app.execute_command("bpm 140").unwrap();
    assert_eq!(app.revision(), revision + 1);
    app.apply(Action::Undo).unwrap();
    assert_eq!(app.project().bpm, 136.0);
    assert!(
        app.flags().lane_muted(lead),
        "undo restores material, not live flags"
    );
    let channel = app.project().mixer.channels[0].id;
    app.apply(Action::SetChannelMuted {
        channel: cadence_core::mixer::ChannelRef::Channel(channel),
        muted: true,
    })
    .unwrap();
    assert_eq!(
        app.revision(),
        revision + 2,
        "undo is a revision; the mute is not"
    );
}

#[test]
fn launches_land_on_the_bar_and_a_late_press_joins_the_downbeat() {
    let mut app = app();
    app.apply(Action::SetTakeArmed { armed: true }).unwrap();
    playing_live(&mut app);
    advance(&mut app, 3);
    let bass_lane = lane(&app, "Bass");
    let bass = lane_part(&app, "Bass", 0);
    app.apply(Action::Launch {
        lane: bass_lane,
        part: bass,
        at: None,
    })
    .unwrap();
    assert_eq!(app.live.pending.len(), 1);
    assert_eq!(app.live.pending[0].at, 16);
    advance(&mut app, 13);
    assert_eq!(app.live.pending.len(), 1, "step 16 has not played yet");
    advance(&mut app, 1);
    assert!(app.live.pending.is_empty());
    assert_eq!(app.live.lane_state(bass_lane).unwrap().anchor, 16);
    // The downbeat just played: a press now still lands on it.
    let drums_lane = lane(&app, "Drums");
    let kick = lane_part(&app, "Drums", 0);
    app.apply(Action::Launch {
        lane: drums_lane,
        part: kick,
        at: None,
    })
    .unwrap();
    assert!(app.live.pending.is_empty());
    assert_eq!(app.live.lane_state(drums_lane).unwrap().anchor, 16);
    // A count chooses a longer boundary for one launch.
    advance(&mut app, 4);
    let lead_lane = lane(&app, "Lead");
    let lead = lane_part(&app, "Lead", 1);
    app.apply(Action::Launch {
        lane: lead_lane,
        part: lead,
        at: Some(Boundary::Bars { count: 4 }),
    })
    .unwrap();
    assert_eq!(app.live.pending[0].at, 64);
}

#[test]
fn the_newest_launch_takes_a_shared_instrument_from_another_lane() {
    let mut app = app();
    app.execute_command("lane Bass add Intro").unwrap();
    playing_live(&mut app);
    let lead_lane = lane(&app, "Lead");
    let bass_lane = lane(&app, "Bass");
    let moving = lane_part(&app, "Lead", 1);
    let bass_intro = lane_part(&app, "Bass", 1);
    app.apply(Action::Launch {
        lane: lead_lane,
        part: moving,
        at: None,
    })
    .unwrap();
    advance(&mut app, 17);
    assert!(app.live.lane_state(lead_lane).is_some());
    app.apply(Action::Launch {
        lane: bass_lane,
        part: bass_intro,
        at: None,
    })
    .unwrap();
    advance(&mut app, 16);
    assert!(
        app.live.lane_state(lead_lane).is_none(),
        "Intro and Moving share the Saw"
    );
    assert_eq!(app.live.lane_state(bass_lane).unwrap().part, bass_intro);
}

#[test]
fn edits_on_launch_timing_change_the_document_now_and_the_sound_on_the_bar() {
    let mut app = app();
    app.execute_command("quantize edits launch").unwrap();
    playing_live(&mut app);
    let lead_lane = lane(&app, "Lead");
    let moving_lp = lane_part(&app, "Lead", 1);
    app.apply(Action::Launch {
        lane: lead_lane,
        part: moving_lp,
        at: Some(Boundary::Now),
    })
    .unwrap();
    advance(&mut app, 70);
    let moving = part(&app, "Moving");
    let mut up = app.project().part(moving).unwrap().clone();
    up.transpose = 12;
    app.apply(Action::Edit {
        edit: Edit::PutPart { part: up },
    })
    .unwrap();
    assert_eq!(app.project().part(moving).unwrap().transpose, 12);
    assert_eq!(app.live.pending.len(), 1);
    assert_eq!(app.live.pending[0].at, 80);
    let compiled = app.audio_compiled();
    let saw = compiled.instrument_index(1).unwrap();
    let reference = app.compiled_project();
    let mut checked = (false, false);
    for step in 70..112u64 {
        let live = compiled.event_for(saw, step);
        let song = reference.event_for(saw, step);
        if let (Some(live), Some(song)) = (live, song) {
            let shift = if step < 80 { 12 } else { 0 };
            assert_eq!(live.midi + shift, song.midi, "step {step}");
            if step < 80 {
                checked.0 = true
            } else {
                checked.1 = true
            }
        }
    }
    assert!(checked.0 && checked.1);
}

#[test]
fn home_ends_the_performance_and_returns_to_the_song() {
    let mut app = app();
    app.apply(Action::SetTakeArmed { armed: true }).unwrap();
    playing_live(&mut app);
    advance(&mut app, 3);
    let drums_lane = lane(&app, "Drums");
    let kick = lane_part(&app, "Drums", 0);
    app.apply(Action::Launch {
        lane: drums_lane,
        part: kick,
        at: None,
    })
    .unwrap();
    advance(&mut app, 40);
    assert_eq!(app.live.lane_state(drums_lane).unwrap().anchor, 16);
    app.apply(Action::Stop).unwrap();
    assert!(!app.overriding());
    assert!(!app.playing());
    assert_eq!(app.transport_step(), 0);
    assert!(
        app.live.active().is_empty(),
        "every lane plays the arrangement"
    );
}

#[test]
fn cues_preserve_overrides_and_audition_layers_cleanly_while_playing() {
    let mut app = app();
    app.apply(Action::SetTakeArmed { armed: true }).unwrap();
    playing_live(&mut app);
    advance(&mut app, 3);
    let drums_lane = lane(&app, "Drums");
    let kick = lane_part(&app, "Drums", 0);
    app.apply(Action::Launch {
        lane: drums_lane,
        part: kick,
        at: None,
    })
    .unwrap();
    advance(&mut app, 20);
    assert!(app.overriding());
    let pattern = app.project().patterns[0].id;
    let out = app
        .apply(Action::Audition {
            pattern,
            length: None,
        })
        .unwrap();
    assert!(
        out.transport.is_empty(),
        "layered audition never seeks or stops"
    );
    let aud = app.audition().unwrap();
    assert!(!aud.solo, "audition while playing is layered");
    assert_eq!(aud.anchor, 32, "joins on the next launch boundary");
    assert!(app.overriding(), "active overrides stay intact");

    // Cue preserves active overrides!
    app.apply(Action::CueSong {
        step: 64,
        play: true,
    })
    .unwrap();
    assert!(app.overriding(), "cues preserve active overrides");
    assert_eq!(app.song_step_heard(), 64);
}

#[test]
fn jam_keys_launch_parts_and_toggle_lanes_over_the_arrangement() {
    let mut app = app();
    app.apply(Action::SetTakeArmed { armed: true }).unwrap();
    playing_live(&mut app);
    keys(&mut app, "gj");
    assert!(app.is_jam());
    advance(&mut app, 4);
    keys(&mut app, "w1");
    assert_eq!(app.live.pending.len(), 1);
    advance(&mut app, 16);
    assert!(app.live.lane_state(lane(&app, "Bass")).is_some());
    keys(&mut app, "ww");
    assert_eq!(
        app.live.pending.len(),
        1,
        "toggling a playing lane releases it"
    );
    advance(&mut app, 16);
    assert!(app.live.lane_state(lane(&app, "Bass")).is_none());
    keys(&mut app, "ww");
    advance(&mut app, 16);
    assert!(
        app.live.lane_state(lane(&app, "Bass")).is_some(),
        "toggling again relaunches its last part"
    );
    keys(&mut app, "e1q2");
    advance(&mut app, 16);
    assert_eq!(app.live.active().len(), 3);
    keys(&mut app, "<m-X>");
    advance(&mut app, 16);
    assert!(app.live.active().is_empty(), "Alt-X releases every lane");
    keys(&mut app, "<Esc>");
    assert!(!app.is_jam());
}

#[test]
fn a_take_replays_its_changes_and_renders_identically() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("set.take.jsonl");
    let mut app = app();
    app.apply(Action::SetTakeArmed { armed: true }).unwrap();
    playing_live(&mut app);
    let drums_lane = lane(&app, "Drums");
    let kick = lane_part(&app, "Drums", 0);
    app.apply(Action::Launch {
        lane: drums_lane,
        part: kick,
        at: Some(Boundary::Now),
    })
    .unwrap();
    advance(&mut app, 8);
    assert!(
        app.take_recording().is_some(),
        "playing automatically opens the always-running take journal"
    );
    let bass_lane = lane(&app, "Bass");
    let bass = lane_part(&app, "Bass", 0);
    app.apply(Action::Launch {
        lane: bass_lane,
        part: bass,
        at: None,
    })
    .unwrap();
    advance(&mut app, 24);
    let lead_lane = lane(&app, "Lead");
    let moving = lane_part(&app, "Lead", 1);
    app.apply(Action::Launch {
        lane: lead_lane,
        part: moving,
        at: None,
    })
    .unwrap();
    app.execute_command("bpm 140").unwrap();
    advance(&mut app, 32);
    app.apply(Action::SetLaneMuted {
        lane: drums_lane,
        muted: true,
    })
    .unwrap();
    advance(&mut app, 16);
    app.apply(Action::SaveTake {
        path: Some(path.clone()),
    })
    .unwrap();
    let take = Take::load(&path).unwrap();
    assert_eq!(take.header.start_step, 0);
    assert_eq!(take.end_step, app.live.next_step);
    assert!(take.events.len() >= 4);
    let states = take.states().unwrap();
    let last = &states.last().unwrap();
    assert_eq!(last.1.active.len(), app.live.active().len());
    let scores = take.compile(1).unwrap();
    assert_eq!(
        scores.len(),
        2,
        "the tempo change needs its own engine score"
    );
    let render = |name: &str| {
        let out = folder.path().join(name);
        let schedule: Vec<_> = scores.iter().map(|(at, c)| (*at, c)).collect();
        let report = render_schedule_report(
            &schedule,
            take.header.start_step,
            take.steps() + 16,
            Vec::new(),
            &out,
            48_000,
        )
        .unwrap();
        (std::fs::read(out).unwrap(), report)
    };
    let (first, report) = render("a.wav");
    let (second, _) = render("b.wav");
    assert_eq!(first, second, "takes render deterministically");
    assert!(report.output.peak[0] > 0.01, "the take is audible");
    let _ = PhaseMode::Song;
}

#[test]
fn staging_collects_gestures_previews_them_and_fires_them_together() {
    let mut app = app();
    app.apply(Action::SetTakeArmed { armed: true }).unwrap();
    playing_live(&mut app);
    advance(&mut app, 4);
    // Alt-Enter starts staging; launches and mutes collect instead of landing.
    keys(&mut app, "<m-Enter><m-q>1<m-w><m-m>");
    assert!(app.live.pending.is_empty(), "staged gestures do not land");
    assert!(!app.flags().lane_muted(lane(&app, "Bass")));
    assert_eq!(app.staging().unwrap().len(), 2);
    keys(&mut app, "<m-Enter><Enter>");
    assert!(app.staging().is_none());
    assert!(app.live.pending.iter().all(|p| p.at == 16));
    advance(&mut app, 16);
    assert!(app.live.lane_state(lane(&app, "Lead")).is_some());
    assert!(app.flags().lane_muted(lane(&app, "Bass")));
    // Discarding leaves the staged change unapplied.
    keys(&mut app, "<m-Enter><m-w><m-m>");
    app.execute_command("stage discard").unwrap();
    assert!(app.staging().is_none());
    assert!(
        app.flags().lane_muted(lane(&app, "Bass")),
        "discarding leaves the staged unmute unapplied"
    );
}

#[test]
fn fivefold_live_rebuilds_its_parts_and_its_performance_lands_on_the_planned_bars() {
    use std::process::Command;
    let repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let folder = repo.join("examples/fivefold-live");
    let temp = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let result = Command::new(env!("CARGO_BIN_EXE_cadence"))
            .current_dir(&repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    };
    // Every curated showcase project passes --check cleanly with zero sample/validation errors.
    for proj in [
        "examples/demo/project.json",
        "examples/glass-cathedral/project.json",
        "examples/fivefold-live/project.json",
    ] {
        run(&["--project", proj, "--sample-roots-only", "--check"]);
    }
    let shipped = cadence_app::load_project(&folder.join("project.json")).unwrap();
    // Replay the performance headlessly, writing its take to a scratch path.
    let take_path = temp.path().join("set.take.jsonl");
    let proj_path = folder.join("project.json");
    let lib = cadence_core::sampler::SampleLibrary::from_roots(&[], Some(&proj_path));
    let mut app = App::new_with_library(shipped, proj_path, lib).unwrap();
    let script = std::fs::read_to_string(folder.join("perform.keys.jsonl")).unwrap();
    for line in script.lines().filter(|l| !l.trim().is_empty()) {
        let step: serde_json::Value = serde_json::from_str(line).unwrap();
        let sequence = step["keys"].as_str().unwrap().replace(
            "examples/fivefold-live/takes/set.take.jsonl",
            take_path.to_str().unwrap(),
        );
        keys(&mut app, &sequence);
        if let Some(advance) = step["advance"].as_str() {
            let steps = cadence_app::parse_duration(advance).unwrap();
            app.apply(Action::Advance { steps }).unwrap();
        }
    }
    let take = Take::load(&take_path).unwrap();
    let shipped_take = Take::load(&folder.join("takes/set.take.jsonl")).unwrap();
    let bars = |take: &Take| {
        take.events
            .iter()
            .map(|e| e.at / 16 + 1)
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(
        bars(&take),
        [5, 9, 14, 16, 17, 25, 28, 33, 36, 38, 41, 44, 49, 57].into(),
        "every gesture lands on its planned bar"
    );
    assert_eq!(
        bars(&take),
        bars(&shipped_take),
        "the real-time recording landed the same bars"
    );
    assert_eq!(take.events.len(), shipped_take.events.len());
}

fn press(app: &mut App, c: char) -> cadence_app::Outcome {
    let key = cadence_app::keys::KeyEvent::new(
        cadence_app::keys::KeyCode::Char(c),
        cadence_app::keys::KeyModifiers::NONE,
    );
    app.handle_script_key(key).unwrap()
}

#[test]
fn the_alt_layer_launches_parts_from_insert_without_leaving_it() {
    let mut app = app();
    keys(&mut app, " gp");
    keys(&mut app, ":open Motif<Enter>i");
    assert_eq!(app.mode_name(), "STEP INSERT");
    let cursor = app.cursor;
    keys(&mut app, "<m-w>1");
    assert_eq!(
        app.live.pending.len(),
        1,
        "Alt-w and then 1 launches Bass's first part"
    );
    assert_eq!(app.mode_name(), "STEP INSERT");
    assert_eq!(app.cursor, cursor, "the digit chose a part, not a degree");
}

#[test]
fn a_lane_key_then_a_note_plays_that_lane_and_caps_lock_only_accents() {
    let mut app = app();
    keys(&mut app, "gj");
    assert!(app.is_jam());
    let selected = app.selected_lane;
    keys(&mut app, "w");
    let out = press(&mut app, 'z');
    let bass = app
        .project()
        .part(lane_part(&app, "Bass", 0).part)
        .map(|p| p.instrument);
    assert_eq!(out.live_notes_on.len(), 1);
    assert_eq!(Some(out.live_notes_on[0].instrument_id), bass);
    assert!(out.live_notes_on[0].velocity < 1.0);
    assert_eq!(
        app.selected_lane, selected,
        "naming a lane never moves the selection"
    );
    assert_eq!(app.focused_lane(), Some(lane(&app, "Bass")));
    // With Caps Lock on, lane keys still name lanes and notes are accented.
    keys(&mut app, "E");
    assert_eq!(app.lane_layer(), Some(lane(&app, "Drums")));
    let out = press(&mut app, 'Z');
    assert_eq!(out.live_notes_on[0].velocity, 1.0);
    assert_eq!(
        Some(out.live_notes_on[0].instrument_id),
        app.project()
            .part(lane_part(&app, "Drums", 0).part)
            .map(|p| p.instrument)
    );
    keys(&mut app, "<c-s-j>");
    assert!(!app.is_jam());
}

#[test]
fn alt_chords_mute_named_lanes_and_alt_shift_acts_on_every_lane() {
    let mut app = app();
    let revision = app.revision();
    keys(&mut app, "<m-q><m-m><m-w><m-m>");
    assert!(app.flags().lane_muted(lane(&app, "Lead")));
    assert!(app.flags().lane_muted(lane(&app, "Bass")));
    keys(&mut app, "<m-M>");
    assert!(app
        .project()
        .lanes
        .iter()
        .all(|l| !app.flags().lane_muted(l.id)));
    keys(&mut app, "<m-e><m-s>");
    assert!(app.flags().lane_solo(lane(&app, "Drums")));
    keys(&mut app, "<m-S>");
    assert!(app
        .project()
        .lanes
        .iter()
        .all(|l| !app.flags().lane_solo(l.id)));
    assert_eq!(app.revision(), revision, "mutes and solos stay live state");
}

#[test]
fn a_part_launched_while_the_song_plays_overrides_its_lane_over_the_arrangement() {
    let mut app = app();
    let lead = lane(&app, "Lead");
    let bass = lane(&app, "Bass");
    let intro = lane_part(&app, "Lead", 0);
    let intro_part = intro.part;
    app.apply(Action::Play { playing: true }).unwrap();
    app.observe_transport(69, 70, true, false);
    keys(&mut app, "<m-q>1");
    assert_eq!(app.live.pending.len(), 1, "the launch waits for its bar");
    advance(&mut app, 16);
    assert!(
        app.overriding(),
        "once landed, the launch overrides the arrangement"
    );
    assert_eq!(app.live.lane_state(lead).map(|s| s.part), Some(intro));
    let origins = |app: &App, lane: Id, from: u32| -> Vec<Id> {
        app.audio_compiled()
            .provenance
            .iter()
            .filter(|o| o.lane == lane && o.song_step >= from && o.song_step < from + 64)
            .map(|o| o.part)
            .collect()
    };
    let lead_now = origins(&app, lead, 80);
    assert!(!lead_now.is_empty() && lead_now.iter().all(|b| *b == intro_part));
    assert!(
        !origins(&app, bass, 80).is_empty(),
        "lanes without an override keep playing the arrangement"
    );
    // Release: Lead's arrangement plays again from the next bar.
    keys(&mut app, "<m-x>");
    advance(&mut app, 16);
    assert!(app.live.lane_state(lead).is_none());
    let moving = app.project().lane(lead).unwrap().placements[1].part;
    let after = origins(&app, lead, 112);
    assert!(!after.is_empty() && after.iter().all(|b| *b == moving));
    // Keep: the override becomes a placement, splitting what it covers.
    keys(&mut app, "<m-q>1");
    advance(&mut app, 48);
    let revision = app.revision();
    keys(&mut app, "<m-c>");
    assert_eq!(app.revision(), revision + 1, "keep is one undoable edit");
    let placements = &app.project().lane(lead).unwrap().placements;
    let kept = placements
        .iter()
        .find(|p| p.part == intro_part && p.start_step >= 64)
        .expect("the kept part");
    assert_eq!(kept.start_step % 16, 0);
    for other in placements.iter().filter(|p| p.id != kept.id) {
        let overlaps = other.start_step < kept.start_step + kept.length_steps
            && kept.start_step < other.start_step + other.length_steps;
        assert!(!overlaps, "kept parts never overlap the arrangement");
    }
    keys(&mut app, "<Home>");
    assert!(!app.overriding(), "Home returns to the song");
}

fn frame(app: &App) -> cadence_app::view::FrameView {
    let snapshot = cadence_core::runtime::AudioSnapshot {
        absolute_step: app.live.next_step,
        playing: true,
        sample_rate: 48000,
        ..Default::default()
    };
    app.frame_view(&snapshot, "Test")
}

#[test]
fn arrange_draws_overrides_what_they_cover_and_what_waits_to_land() {
    use cadence_app::view::{BankPartStatus, DeckView};
    let mut app = app();
    app.apply(Action::SetTakeArmed { armed: true }).unwrap();
    playing_live(&mut app);
    advance(&mut app, 4);
    keys(&mut app, "<m-w>1");
    let waiting = frame(&app);
    assert_eq!(waiting.chrome.pending_count, 1);
    let arr_wait = waiting
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Arrange(v) => Some(v),
            _ => None,
        })
        .unwrap();
    let bass_wait = arr_wait.lanes.iter().find(|l| l.name == "Bass").unwrap();
    assert!(
        bass_wait
            .pending
            .iter()
            .any(|m| m.label.contains("Bass") && !m.is_release),
        "the launch waits for bar 2"
    );

    advance(&mut app, 16);
    let playing = frame(&app);
    assert_eq!(playing.chrome.override_count, 1);
    let arr_play = playing
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Arrange(v) => Some(v),
            _ => None,
        })
        .unwrap();
    let bass_play = arr_play.lanes.iter().find(|l| l.name == "Bass").unwrap();
    assert!(bass_play.pending.is_empty());
    assert!(
        bass_play
            .override_info
            .as_ref()
            .is_some_and(|o| o.part_name.contains("Bass")),
        "the override runs over the lane"
    );
    let bank = playing
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Bank(v) => Some(v),
            _ => None,
        })
        .unwrap();
    assert!(
        bank.lanes
            .iter()
            .flat_map(|l| &l.parts)
            .any(|p| p.status == BankPartStatus::Overriding),
        "the Bank marks the override"
    );

    keys(&mut app, "<m-w><m-x>");
    let rel = frame(&app);
    let arr_rel = rel
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Arrange(v) => Some(v),
            _ => None,
        })
        .unwrap();
    let bass_rel = arr_rel.lanes.iter().find(|l| l.name == "Bass").unwrap();
    assert!(bass_rel.pending.iter().any(|m| m.is_release));
}

#[test]
fn arrange_marks_staged_gestures_and_song_edits_waiting_for_their_bar() {
    use cadence_app::view::DeckView;
    let mut app = app();
    app.apply(Action::SetTakeArmed { armed: true }).unwrap();
    playing_live(&mut app);
    advance(&mut app, 4);
    keys(&mut app, "<m-Enter><m-w>1");
    let staged = frame(&app);
    let arr = staged
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Arrange(v) => Some(v),
            _ => None,
        })
        .unwrap();
    let bass = arr.lanes.iter().find(|l| l.name == "Bass").unwrap();
    assert!(
        bass.staged.iter().any(|m| m.label.contains("Bass")),
        "a staged launch shows where it would land"
    );
    assert!(bass.pending.is_empty());
    app.execute_command("stage discard").unwrap();
    app.execute_command("quantize edits launch").unwrap();
    let moving = part(&app, "Moving");
    let mut up = app.project().part(moving).unwrap().clone();
    up.transpose = 12;
    app.apply(Action::Edit {
        edit: Edit::PutPart { part: up },
    })
    .unwrap();
    let edited = frame(&app);
    let arr_ed = edited
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Arrange(v) => Some(v),
            _ => None,
        })
        .unwrap();
    assert!(
        arr_ed.pending_edit_bars.contains(&1),
        "the edit is marked on bar 2 (0-based 1): {:?}",
        arr_ed.pending_edit_bars
    );
}

#[test]
fn an_edit_typed_from_jam_keeps_the_view_following_the_performance() {
    use cadence_core::follow::FollowMode;
    let mut app = app();
    keys(&mut app, "tf");
    assert_eq!(app.follow_mode(), FollowMode::Active);
    playing_live(&mut app);
    let harmony = app
        .project()
        .patterns
        .iter()
        .find(|p| matches!(p.kind, cadence_core::material::PatternKind::Values { .. }))
        .expect("a values pattern")
        .name
        .clone();
    let revision = app.revision();
    keys(&mut app, &format!("gj:value \"{harmony}\" 1 3<Enter>"));
    assert!(app.revision() > revision, "the edit changed the material");
    assert!(app.is_jam(), "the command line returns to Jam");
    assert_eq!(app.follow_mode(), FollowMode::Active);
    keys(&mut app, "<Esc>");
    assert!(!app.is_jam());
    keys(&mut app, "l");
    assert_eq!(
        app.follow_mode(),
        FollowMode::Suspended,
        "outside Jam, moving still parks the view"
    );
}

#[test]
fn layered_audition_leaves_other_instruments_untouched_and_never_records_or_keeps() {
    let dir = tempfile::tempdir().unwrap();
    let take_path = dir.path().join("audition.take.jsonl");
    let mut app = app();
    // Create a distinct note pattern to audition on Lead while Bass and Drums play.
    app.execute_command("pattern notes AudTest 1 5 8 12")
        .unwrap();
    let aud_pattern = app.selected_pattern.unwrap();
    app.selected_instrument = Some(1); // Lead (instrument 1)

    let baseline = app.audio_compiled().clone();
    let bass_idx = app.audio_compiled().instrument_index(2).unwrap();
    let drums_idx = app.audio_compiled().instrument_index(3).unwrap();
    let lead_idx = app.audio_compiled().instrument_index(1).unwrap();

    app.apply(Action::SetTakeArmed { armed: true }).unwrap();
    playing_live(&mut app);
    advance(&mut app, 68); // inside Motion (bars 5..16, all 3 instruments play)

    let out = app
        .apply(Action::Audition {
            pattern: aud_pattern,
            length: Some(4),
        })
        .unwrap();
    assert!(out.changed);
    assert!(
        out.transport.is_empty(),
        "layered audition never seeks/stops"
    );
    let aud = app.audition().unwrap();
    assert!(!aud.solo);
    assert_eq!(aud.instrument, 1);
    assert_eq!(aud.anchor, 80, "lands on next bar boundary");

    // Every OTHER instrument (Bass & Drums) is 100% identical across steps 64..144!
    let layered = app.audio_project().clone();
    for step in 64..144u64 {
        let b0 = baseline.event_for(bass_idx, step);
        let b1 = layered.event_for(bass_idx, step);
        assert_eq!(b0.map(|e| e.midi), b1.map(|e| e.midi), "bass step {step}");
        let d0 = baseline.event_for(drums_idx, step);
        let d1 = layered.event_for(drums_idx, step);
        assert_eq!(d0.map(|e| e.midi), d1.map(|e| e.midi), "drums step {step}");
    }
    // Auditioned pattern sounds on Lead starting at anchor 80, not before!
    let aud_ev = layered.event_for(lead_idx, 80).unwrap();
    assert_eq!(aud_ev.part_id, None);

    // Advance while auditioning, try Keep (must refuse because audition is not an override), then stop.
    advance(&mut app, 32);
    let rev = app.revision();
    assert!(
        app.press_keys("<m-C>").is_err(),
        "Keep must never include an audition"
    );
    assert_eq!(app.revision(), rev);
    let stop_out = app.apply(Action::StopAudition).unwrap();
    assert!(stop_out.changed);
    assert!(
        stop_out.transport.is_empty(),
        "stopping layered audition keeps rolling"
    );
    assert!(app.playing());

    app.apply(Action::SaveTake {
        path: Some(take_path.clone()),
    })
    .unwrap();
    let take = Take::load(&take_path).unwrap();
    assert!(
        take.events.is_empty(),
        "audition never records events into a take: {:?}",
        take.events
    );
}

#[test]
fn arbitrary_length_take_past_256_bars_compiles_and_renders() {
    use cadence_engine::audio::render_schedule_measure;
    let dir = tempfile::tempdir().unwrap();
    let take_path = dir.path().join("long.take.jsonl");
    let mut app = app();
    app.apply(Action::SetTakeArmed { armed: true }).unwrap();
    playing_live(&mut app);
    // 260 bars = 4160 steps, exceeding the former 256-bar cap!
    for _ in 0..13 {
        advance(&mut app, 20 * 16);
    }
    app.apply(Action::SaveTake {
        path: Some(take_path.clone()),
    })
    .unwrap();
    let take = Take::load(&take_path).unwrap();
    assert_eq!(take.steps(), 260 * 16);
    let windows = take.compile(2).unwrap();
    assert!(
        windows.len() >= 5,
        "262 bars should span multiple 64-bar ring windows, got {}",
        windows.len()
    );
    assert_eq!(windows[0].0, 0);
    assert_eq!(windows[1].0, 64 * 16);
    let schedule: Vec<_> = windows.iter().map(|(at, c)| (*at, c)).collect();
    // Render across the 256-bar boundary (bars 255..261 = 6 bars) at 8 kHz to verify seamless swaps.
    let report = render_schedule_measure(&schedule, 255 * 16, 6 * 16, Vec::new(), 8_000).unwrap();
    assert!(
        report.output.peak[0] > 0.01,
        "take past 256 bars must produce audible output"
    );
}

#[test]
fn growing_keep_atomic_keep_all_and_range_keep_with_auto_forking() {
    // 1. Growing Keep in Song mode grows bar-by-bar in ONE undo entry.
    let mut a = app();
    let baseline = a.project().clone();
    let lead_lane = lane(&a, "Lead");
    let bass_lane = lane(&a, "Bass");
    let intro = lane_part(&a, "Lead", 0);
    let bass_part = lane_part(&a, "Bass", 0);

    a.apply(Action::Play { playing: true }).unwrap();
    advance(&mut a, 16); // bar 2
    a.apply(Action::Launch {
        lane: lead_lane,
        part: intro,
        at: Some(Boundary::Now),
    })
    .unwrap();
    advance(&mut a, 8); // mid bar 2
    a.press_keys("<m-q><m-c>").unwrap(); // keep Lead from bar 2 (growing!)
    let pl_bar2 = a
        .lane(lead_lane)
        .unwrap()
        .placements
        .iter()
        .find(|p| p.start_step == 16 && p.part == intro.part)
        .expect("kept placement at bar 2")
        .length_steps;
    assert_eq!(pl_bar2, 16, "initially kept 1 bar");

    // Advance 2 more bars while override stays active: placement grows automatically!
    advance(&mut a, 32);
    let pl_grown = a
        .lane(lead_lane)
        .unwrap()
        .placements
        .iter()
        .find(|p| p.start_step == 16 && p.part == intro.part)
        .expect("grown placement at bar 2")
        .length_steps;
    assert_eq!(pl_grown, 48, "growing keep extended cleanly to 3 bars");

    // Release stops further growth, and ONE single Undo reverts the entire 3-bar passage!
    a.apply(Action::Release {
        lane: lead_lane,
        at: Some(Boundary::Now),
    })
    .unwrap();
    advance(&mut a, 16);
    a.apply(Action::Undo).unwrap();
    assert_eq!(
        a.lane(lead_lane).unwrap().placements,
        baseline.lane(lead_lane).unwrap().placements,
        "one Undo reverts the entire growing keep"
    );

    // 2. Alt-K keeps multiple active overrides in ONE single undo transaction.
    let before_all = a.project().clone();
    a.apply(Action::Launch {
        lane: lead_lane,
        part: intro,
        at: Some(Boundary::Now),
    })
    .unwrap();
    a.apply(Action::Launch {
        lane: bass_lane,
        part: bass_part,
        at: Some(Boundary::Now),
    })
    .unwrap();
    advance(&mut a, 16);
    a.press_keys("<m-C>").unwrap();
    a.apply(Action::Undo).unwrap();
    assert_eq!(
        a.project(),
        &before_all,
        "Alt-K must commit all lanes in a single undoable transaction"
    );

    // 3. :keep A B keeps take bars with automatic definition forking for mid-take edits.
    let mut b = app();
    let harmony_name = b
        .project()
        .patterns
        .iter()
        .find(|p| matches!(p.kind, cadence_core::material::PatternKind::Values { .. }))
        .unwrap()
        .name
        .clone();
    let pats_before = b.project().patterns.len();
    b.apply(Action::SetTakeArmed { armed: true }).unwrap();
    b.apply(Action::Play { playing: true }).unwrap();
    advance(&mut b, 64); // bars 1..4
                         // Mid-take edit at bar 5!
    b.execute_command(&format!("value \"{harmony_name}\" 1 5"))
        .unwrap();
    advance(&mut b, 32); // bars 5..6 heard with edited Harmony!
                         // Undo back to original Harmony!
    b.apply(Action::Undo).unwrap();
    advance(&mut b, 16); // bar 7 heard with original Harmony
    b.execute_command("keep 5 6").unwrap();
    assert!(
        b.project().patterns.len() > pats_before,
        "mid-take edit must fork the edited pattern and dependents on :keep"
    );
    assert!(
        b.project().patterns.iter().any(|p| p.name.contains("take")),
        "forked definitions are named clearly"
    );
}

#[test]
fn growing_keep_preserves_pending_live_changes() {
    let mut a = app();
    a.apply(Action::Play { playing: true }).unwrap();

    let lead_lane = lane(&a, "Lead");
    let bass_lane = lane(&a, "Bass");
    let lead_part = lane_part(&a, "Lead", 1);
    let bass_part = lane_part(&a, "Bass", 0);

    // Launch a live override on Lead and let it take effect at step 16.
    a.apply(Action::Launch {
        lane: lead_lane,
        part: lead_part,
        at: None,
    })
    .unwrap();
    advance(&mut a, 17);
    assert!(a.live.lane_state(lead_lane).is_some());

    // Start a Growing Keep (`Alt-c`) on Lead.
    a.selected_lane = Some(lead_lane);
    a.press_keys("<m-c>").unwrap();

    // Queue a pending Bass launch on a 4-bar boundary (step 64) before Growing Keep extends at step 32.
    a.apply(Action::Launch {
        lane: bass_lane,
        part: bass_part,
        at: Some(Boundary::Bars { count: 4 }),
    })
    .unwrap();
    let queued_at = a
        .live
        .pending
        .iter()
        .find_map(|p| match p.change {
            cadence_core::live::Change::Launch { lane, .. } if lane == bass_lane => Some(p.at),
            _ => None,
        })
        .expect("Bass launch queued");
    assert!(queued_at > 32);

    // Advance across step 32 so `grow_active_keeps` commits a kept bar without dropping pending launches.
    advance(&mut a, 16);
    assert!(
        a.live.pending.iter().any(|p| matches!(
            p.change,
            cadence_core::live::Change::Launch { lane, .. } if lane == bass_lane && p.at == queued_at
        )),
        "Growing Keep must preserve queued pending live changes"
    );

    // Advance past `queued_at` and verify the preserved pending launch executes.
    let remaining = queued_at.saturating_sub(33) + 2;
    advance(&mut a, remaining);
    assert!(
        a.live.lane_state(bass_lane).is_some(),
        "Preserved pending launch must fire on its scheduled boundary"
    );
}

#[test]
fn cancelling_pending_flags_restores_doc_flags_from_heard_and_remaining() {
    use cadence_app::StageChoice;

    let mut a = app();
    a.apply(Action::Play { playing: true }).unwrap();
    advance(&mut a, 4);

    let lead_lane = lane(&a, "Lead");
    let bass_lane = lane(&a, "Bass");
    assert!(!a.flags().lane_muted(lead_lane));
    assert!(!a.flags().lane_muted(bass_lane));

    // Stage and fire a mute on Lead for bar 2 (step 16) -> queues Change::Flags #1.
    a.apply(Action::Stage).unwrap();
    a.apply(Action::SetLaneMuted {
        lane: lead_lane,
        muted: true,
    })
    .unwrap();
    a.apply(Action::FinishStaging {
        choice: StageChoice::Fire,
    })
    .unwrap();
    assert_eq!(a.live.pending.len(), 1);
    assert!(a.flags().lane_muted(lead_lane));

    // Advance slightly inside bar 1, then stage and fire a mute on Bass -> queues Change::Flags #2.
    advance(&mut a, 1);
    a.apply(Action::Stage).unwrap();
    a.apply(Action::SetLaneMuted {
        lane: bass_lane,
        muted: true,
    })
    .unwrap();
    a.apply(Action::FinishStaging {
        choice: StageChoice::Fire,
    })
    .unwrap();
    assert_eq!(a.live.pending.len(), 2);
    assert!(a.flags().lane_muted(lead_lane));
    assert!(a.flags().lane_muted(bass_lane));

    // Cancel only the last pending Change::Flags -> Bass mute reverts while Lead mute stays pending.
    a.apply(Action::CancelPending { all: false }).unwrap();
    assert_eq!(a.live.pending.len(), 1);
    assert!(
        a.flags().lane_muted(lead_lane),
        "Remaining pending Lead mute must stay reflected in doc.flags()"
    );
    assert!(
        !a.flags().lane_muted(bass_lane),
        "Cancelled Bass mute must revert out of doc.flags()"
    );

    // Cancel all remaining pending changes -> Lead mute reverts to live.heard_flags().
    a.apply(Action::CancelPending { all: true }).unwrap();
    assert!(a.live.pending.is_empty());
    assert!(
        !a.flags().lane_muted(lead_lane),
        "Cancelling all pending flags must restore doc.flags() to heard_flags()"
    );
    assert!(!a.flags().lane_muted(bass_lane));
}
