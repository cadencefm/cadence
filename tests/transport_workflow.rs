use cadence_app::keys::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use cadence_app::{Action, App, Request};
use cadence_core::{live::Change, material::Project, music::EndRule, runtime::TransportIntent};

fn app() -> App {
    let mut app = App::new_in_memory(Project::empty(), "unused.json".into()).unwrap();
    app.execute_command("part new Lead init 0 2 4 6 7").unwrap();
    app.arrange_ui.step = 64;
    app.cursor = 3;
    app.observe_transport(43, 44, true, false);
    app
}

fn press(app: &mut App, key: char, kind: KeyEventKind) -> cadence_app::Outcome {
    app.handle_key(KeyEvent::new_with_kind(
        KeyCode::Char(key),
        KeyModifiers::NONE,
        kind,
    ))
    .unwrap()
}

#[test]
fn stop_key_command_and_api_restore_song_start_without_editing_music() {
    for path in 0..3 {
        let mut a = app();
        let project = a.project().clone();
        let revision = a.revision();
        a.apply(Action::Audition {
            pattern: a.selected_pattern.unwrap(),
            length: Some(5),
        })
        .unwrap();
        a.press_keys("gj").unwrap();
        assert_eq!(
            press(&mut a, 'z', KeyEventKind::Press).live_notes_on.len(),
            1
        );
        let out = match path {
            0 => a.press_keys("<C-Home>").unwrap().pop().unwrap(),
            1 => a.execute_command("stop").unwrap(),
            _ => {
                a.execute_request(Request {
                    id: None,
                    expected_revision: Some(revision),
                    compact: false,
                    action: Action::Stop,
                })
                .1
            }
        };
        assert_eq!(
            out.transport,
            vec![
                TransportIntent::Play { playing: false },
                TransportIntent::Seek { step: 0 }
            ]
        );
        assert_eq!(out.live_notes_off.len(), 1);
        assert!(!a.is_auditioning());
        assert!(!a.playing());
        assert_eq!(a.playing_loop(), None);
        assert_eq!(a.transport_step(), 0);
        assert_eq!((a.arrange_ui.step, a.cursor), (64, 3));
        assert_eq!(a.project(), &project);
        assert_eq!(a.revision(), revision);
        assert!(a.release_held().live_notes_off.is_empty());
    }
}

#[test]
fn two_press_home_pauses_at_loop_start_first_then_returns_to_song_start() {
    let dir = tempfile::tempdir().unwrap();
    let save_path = dir.path().join("demo.json");
    let lib = cadence_core::sampler::SampleLibrary::from_roots(&[], Some(&save_path));
    let mut a = App::new_with_library(Project::demo(), save_path, lib).unwrap();
    a.set_session_takes_dir(dir.path().join("takes"));
    a.execute_command("lane Lead add Intro").unwrap();
    a.execute_command("loop Motion").unwrap(); // bars 5..16 (steps 64..256)
    assert_eq!(a.playing_loop(), Some((64, 256)));
    a.apply(Action::SetTakeArmed { armed: true }).unwrap();
    a.apply(Action::Play { playing: true }).unwrap();
    let take_path = a.active_take_path().unwrap().to_path_buf();
    a.press_keys("<m-q>1").unwrap();
    a.apply(Action::Advance { steps: 32 }).unwrap();
    assert!(a.overriding(), "override landed inside the loop");
    assert_eq!(a.song_step_heard(), 95);

    // 1st Home: pauses at loop start (64), keeping loop + active override + open take!
    let first = a.press_keys("<Home>").unwrap().pop().unwrap();
    assert!(!a.playing());
    assert_eq!(
        a.playing_loop(),
        Some((64, 256)),
        "loop preserved on 1st Home"
    );
    assert!(a.overriding(), "overrides preserved on 1st Home");
    assert_eq!(a.song_step_heard(), 64, "snapped to loop start");
    assert!(a.take_recording().is_some(), "take stays open on 1st Home");
    assert!(
        first
            .transport
            .iter()
            .all(|t| !matches!(t, TransportIntent::Reset { .. })),
        "Home never hard-resets voices/tails"
    );

    // 2nd Home: seals take, clears overrides & loop, returns to song start 0!
    let second = a.press_keys("<Home>").unwrap().pop().unwrap();
    assert!(!a.playing());
    assert_eq!(a.playing_loop(), None, "loop cleared on 2nd Home");
    assert!(!a.overriding(), "overrides cleared on 2nd Home");
    assert_eq!(a.transport_step(), 0);
    assert_eq!(a.song_step_heard(), 0);
    assert!(a.take_recording().is_none(), "take sealed on 2nd Home");
    assert!(cadence_core::take::Take::load(&take_path).is_ok());
    assert_eq!(
        second.transport,
        vec![
            TransportIntent::Play { playing: false },
            TransportIntent::Seek { step: 0 }
        ]
    );
}

#[test]
fn restart_preserves_active_loop_overrides_and_solo_audition_parked_context() {
    let mut a = App::new_in_memory(Project::demo(), "unused.json".into()).unwrap();
    a.execute_command("lane Lead add Intro").unwrap();
    a.execute_command("loop Motion").unwrap();
    a.apply(Action::Play { playing: true }).unwrap();
    a.press_keys("<m-q>1").unwrap();
    a.apply(Action::Advance { steps: 32 }).unwrap();
    assert!(a.overriding());
    let revision = a.revision();
    let out = a.press_keys("tR").unwrap().pop().unwrap();
    assert_eq!(
        out.transport,
        vec![TransportIntent::Seek {
            step: a.transport_step()
        }]
    );
    assert_eq!(a.song_step_heard(), 64, "restarted at loop start");
    assert!(a.playing());
    assert!(a.overriding(), "restart preserves active overrides");
    assert_eq!(a.playing_loop(), Some((64, 256)));

    // Solo audition restart keeps parked song position intact
    a.observe_transport(37, 38, false, false);
    a.apply(Action::Audition {
        pattern: a.selected_pattern.unwrap(),
        length: Some(5),
    })
    .unwrap();
    a.apply(Action::Play { playing: false }).unwrap();
    let out = a.execute_command("restart").unwrap();
    assert_eq!(out.transport, vec![TransportIntent::Seek { step: 0 }]);
    assert!(a.is_solo_auditioning());
    assert!(!a.playing());
    assert_eq!(a.transport_step(), 0);
    let out = a.apply(Action::StopAudition).unwrap();
    assert_eq!(
        out.transport,
        vec![
            TransportIntent::Play { playing: false },
            TransportIntent::Seek { step: 37 }
        ]
    );
    assert_eq!(a.playing_loop(), Some((64, 256)));
    assert_eq!(a.revision(), revision);
}

#[test]
fn cue_uses_arrange_position_and_explicit_bar_is_validated_before_audition_exit() {
    let mut a = app();
    a.press_keys("gp").unwrap();
    a.cursor = 3;
    a.observe_transport(43, 44, false, false);
    let before = a.project().clone();
    let revision = a.revision();
    let out = a.press_keys("tc").unwrap().pop().unwrap();
    assert_eq!(
        out.transport,
        vec![
            TransportIntent::Seek { step: 44 },
            TransportIntent::Play { playing: false }
        ]
    );
    assert_eq!(a.song_step_heard(), 64);
    assert_eq!(a.cursor, 3);
    let out = a.press_keys("tp").unwrap().pop().unwrap();
    assert_eq!(
        out.transport,
        vec![
            TransportIntent::Seek { step: 44 },
            TransportIntent::Play { playing: true }
        ]
    );
    assert!(a.playing());
    a.observe_transport(44, 45, false, false);
    a.apply(Action::Audition {
        pattern: a.selected_pattern.unwrap(),
        length: Some(5),
    })
    .unwrap();
    assert!(a.is_solo_auditioning());
    assert!(a.execute_command("cue 0").is_err());
    assert!(a.is_solo_auditioning());
    assert!(a.execute_command("cue 9999").is_err());
    assert!(a.is_solo_auditioning());
    let out = a.execute_command("cue 8").unwrap();
    assert!(out.changed);
    assert!(!a.is_auditioning());
    assert_eq!(a.song_step_heard(), 112);
    assert_eq!(a.playing_loop(), None);
    assert_eq!(a.arrange_ui.step, 64);
    assert_eq!(a.project(), &before);
    assert_eq!(a.revision(), revision);
}

#[test]
fn prefixes_cancel_ignore_repeats_and_leave_text_entry_intact() {
    let mut a = app();
    let before = a.project().clone();
    let revision = a.revision();
    press(&mut a, 't', KeyEventKind::Press);
    assert!(a.is_transport_prefix());
    assert_eq!(a.mode_name(), "TRANSPORT");
    assert!(press(&mut a, 's', KeyEventKind::Repeat)
        .transport
        .is_empty());
    assert!(a.is_transport_prefix());
    a.press_keys("<Esc>").unwrap();
    assert!(!a.is_transport_prefix());
    assert!(a.playing());
    a.press_keys(":title draft").unwrap();
    let out = a.press_keys("<C-Home>").unwrap().pop().unwrap();
    assert_eq!(
        out.transport,
        vec![
            TransportIntent::Play { playing: false },
            TransportIntent::Seek { step: 0 }
        ]
    );
    assert_eq!(a.command_line(), Some("title draft"));
    assert!(a
        .handle_key(KeyEvent::new_with_kind(
            KeyCode::Home,
            KeyModifiers::CONTROL,
            KeyEventKind::Repeat
        ))
        .unwrap()
        .transport
        .is_empty());
    a.press_keys("<Esc>").unwrap();
    assert_eq!(a.project(), &before);
    assert_eq!(a.revision(), revision);
}

#[test]
fn monitor_keys_commands_and_api_are_session_preferences() {
    let mut a = app();
    let before = a.project().clone();
    let revision = a.revision();
    let dirty = a.dirty();
    assert!(a.view_state.monitor_visible);
    a.press_keys("go").unwrap();
    assert!(!a.view_state.monitor_visible);
    a.execute_command("monitor on").unwrap();
    assert!(a.view_state.monitor_visible);
    a.apply(Action::SetMonitor { visible: false }).unwrap();
    assert!(!a.view_state.monitor_visible);
    a.execute_command("monitor").unwrap();
    assert!(a.view_state.monitor_visible);
    assert!(a.execute_command("monitor maybe").is_err());
    assert!(a.view_state.monitor_visible);
    assert_eq!(a.project(), &before);
    assert_eq!(a.revision(), revision);
    assert_eq!(a.dirty(), dirty);
}

#[test]
fn backslash_and_loop_command_share_the_exact_same_timeline_path() {
    let mut a = App::new_in_memory(Project::demo(), "unused.json".into()).unwrap();
    a.press_keys("ga4l[3l]").unwrap();
    assert_eq!(a.loop_region(), Some((64, 128)), "bars 5 to 8");
    a.press_keys("\\").unwrap();
    assert_eq!(a.playing_loop(), Some((64, 128)));
    a.press_keys("\\").unwrap();
    assert_eq!(a.playing_loop(), None);

    // Playing outside the loop region schedules landing on the launch boundary at region start
    a.press_keys("<Home>").unwrap();
    a.apply(Action::Play { playing: true }).unwrap();
    a.apply(Action::Advance { steps: 20 }).unwrap(); // bar 2, step 4 (outside 64..128)
    a.press_keys("\\").unwrap();
    let (at, seg) = a.live.pending_timeline().expect("pending loop transition");
    assert_eq!(at, 32, "lands on next bar boundary");
    assert_eq!((seg.song, seg.region), (64, Some((64, 64))));
    a.apply(Action::Advance { steps: 16 }).unwrap();
    assert_eq!(a.playing_loop(), Some((64, 128)));
    assert_eq!(a.song_step_heard(), 64 + 3);

    // `:loop song` exits through the same path; `:loop Seed` from outside schedules on boundary
    a.execute_command("loop song").unwrap();
    assert_eq!(a.playing_loop(), None);
    a.execute_command("loop Seed").unwrap();
    let (_, seg) = a
        .live
        .pending_timeline()
        .expect(":loop from outside region queues on launch boundary");
    assert_eq!((seg.song, seg.region), (0, Some((0, 64))));

    let revision = a.revision();
    assert_eq!(a.end_rule(), EndRule::Loop);
    a.press_keys("te").unwrap();
    assert_eq!(a.end_rule(), EndRule::Stop);
    a.press_keys("te").unwrap();
    assert_eq!(a.end_rule(), EndRule::Continue);
    assert_eq!(a.revision(), revision + 2, "the end rule is a song edit");
    a.press_keys("u").unwrap();
    assert_eq!(a.end_rule(), EndRule::Stop);
    // A new, empty song keeps playing past its end.
    let fresh = App::new_in_memory(Project::empty(), "unused.json".into()).unwrap();
    assert_eq!(fresh.end_rule(), EndRule::Continue);
}

#[test]
fn end_rule_stop_auto_pauses_and_space_restarts_from_song_start() {
    let mut a = App::new_in_memory(Project::demo(), "unused.json".into()).unwrap();
    a.execute_command("end stop").unwrap();
    let window = a.audio_project().live_window.unwrap();
    assert_eq!(window.stop_at, Some(256));
    a.apply(Action::Play { playing: true }).unwrap();
    a.apply(Action::Advance { steps: 270 }).unwrap();
    assert!(!a.playing(), "auto-paused at song end");
    assert_eq!(a.transport_step(), 256);

    // Pressing Space while parked at stop_at restarts from song start 0 cleanly
    let out = a.apply(Action::Play { playing: true }).unwrap();
    assert!(a.playing());
    assert!(out.changed);
    assert_eq!(a.song_step_heard(), 0);
}

#[test]
fn section_keys_always_use_timeline_segments_and_backspace_cancels() {
    let sections = |a: &mut App| {
        // The demo has Seed (bars 1–4) and Motion (5–16); add a Coda (bars 13–16).
        a.execute_command("section Coda 13 4").unwrap();
    };
    let mut a = App::new_in_memory(Project::demo(), "unused.json".into()).unwrap();
    sections(&mut a);
    assert!(a.uses_sections());
    a.apply(Action::Play { playing: true }).unwrap();
    a.observe_transport(20, 21, true, false);
    // 1st press in song mode inside Seed (0..64) queues Coda (192) at end of Seed (64)
    a.press_keys("<m-3>").unwrap();
    let (at, seg) = a.live.pending_timeline().expect("pending timeline remap");
    assert_eq!(at, 64, "end of Seed");
    assert_eq!((seg.from, seg.song, seg.region), (64, 192, None));

    // Backspace cancels the pending section jump cleanly
    a.press_keys("<BS>").unwrap();
    assert!(a.live.pending_timeline().is_none(), "Backspace cancels");

    // 2nd press of the same section key brings it forward to the next launch boundary (bar 3 = 32)
    a.press_keys("<m-3>").unwrap();
    a.press_keys("<m-3>").unwrap();
    let (at, seg) = a.live.pending_timeline().expect("brought forward");
    assert_eq!(at, 32, "next bar boundary");
    assert_eq!(seg.song, 192);

    // Outside any section -> next launch boundary immediately on 1st press
    let mut b = App::new_in_memory(Project::demo(), "unused.json".into()).unwrap();
    b.execute_command("bars 20").unwrap(); // bars 17..20 (steps 256..320) have no section
    b.apply(Action::CueSong {
        step: 260,
        play: true,
    })
    .unwrap();
    b.press_keys("<m-1>").unwrap();
    let (at, seg) = b.live.pending_timeline().expect("outside section -> bar");
    assert_eq!(at, b.transport_next().div_ceil(16) * 16);
    assert_eq!(seg.song, 0);
}

#[test]
fn a_section_key_while_overriding_remaps_the_arrangement_on_its_boundary() {
    let mut a = App::new_in_memory(Project::demo(), "unused.json".into()).unwrap();
    a.execute_command("lane Lead add Intro").unwrap();
    a.apply(Action::Play { playing: true }).unwrap();
    a.observe_transport(20, 21, true, false);
    a.press_keys("<m-q>1").unwrap();
    a.press_keys("<m-2>").unwrap();
    let remap = a
        .live
        .pending
        .iter()
        .find_map(|p| match p.change {
            Change::Timeline { segment } => Some((p.at, segment)),
            _ => None,
        })
        .expect("a pending remap");
    assert_eq!(remap.0, 64, "the end of Seed");
    assert_eq!((remap.1.from, remap.1.song), (64, 64));
}

#[test]
fn steps_still_to_come_map_through_a_pending_section_change() {
    let mut a = App::new_in_memory(Project::demo(), "unused.json".into()).unwrap();
    a.execute_command("lane Lead add Intro").unwrap();
    let end = a
        .project()
        .sections
        .iter()
        .map(|s| s.start_step + s.length_steps)
        .max()
        .unwrap_or(0);
    a.execute_command("bars 32").unwrap();
    a.execute_command(&format!("section Far {} 2", end / 16 + 3))
        .unwrap();
    let sections: Vec<(u32, u32)> = a
        .project()
        .sections
        .iter()
        .map(|s| (s.start_step, s.length_steps))
        .collect();
    assert!(sections.len() >= 3);
    a.press_keys("tl").unwrap();
    a.apply(Action::Play { playing: true }).unwrap();
    a.press_keys("<m-q>1").unwrap();
    assert!(a.playing_loop().is_some());
    a.apply(Action::Advance { steps: 20 }).unwrap();
    assert!(a.overriding());
    a.press_keys("<m-3>").unwrap();
    let pass_end = u64::from(sections[0].1);
    let third = u64::from(sections[2].0);
    assert_eq!(
        a.song_position(pass_end - 1),
        pass_end - 1,
        "still looping the first"
    );
    assert_eq!(
        a.song_position(pass_end),
        third,
        "the change lands at the end of the pass"
    );
    assert_eq!(a.song_position(pass_end + 16), third + 16);
}
