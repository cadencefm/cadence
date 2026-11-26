//! Live clock compilation and boundary arithmetic, independent of the UI.
use cadence_core::live::{Change, Performance};
use cadence_core::material::{
    Boundary, Edit, LanePart, LiveFlags, LiveSegment, Override, PhaseMode, Project, LIVE_RING_STEPS,
};
use std::sync::Arc;

const LEAD: u64 = 1;
const BASS: u64 = 2;
const DRUMS: u64 = 3;
const LEAD_PART: LanePart = LanePart {
    part: 2,
    phase: PhaseMode::Song,
    offset_steps: 0,
};
const BASS_PART: LanePart = LanePart {
    part: 3,
    phase: PhaseMode::Song,
    offset_steps: 0,
};
const KICK_PART: LanePart = LanePart {
    part: 4,
    phase: PhaseMode::Restart,
    offset_steps: 0,
};
const INTRO_PART: LanePart = LanePart {
    part: 1,
    phase: PhaseMode::Song,
    offset_steps: 0,
};

fn demo_with_parts() -> Project {
    let mut project = Project::demo();
    for (lane, part) in [
        (LEAD, LEAD_PART),
        (BASS, BASS_PART),
        (DRUMS, KICK_PART),
        (LEAD, INTRO_PART),
    ] {
        project
            .apply(Edit::AddLanePart { lane, entry: part })
            .unwrap();
    }
    project
}

fn track_of(
    project: &Project,
    compiled: &cadence_core::score::ScoreWindow,
    instrument: u64,
) -> usize {
    let _ = project;
    compiled.instrument_index(instrument).unwrap()
}

#[test]
fn song_phase_slots_play_the_same_notes_as_song_phase_placements() {
    let project = demo_with_parts();
    let flags = LiveFlags::default();
    let arrangement = project.compile().unwrap();
    let active = [Override {
        lane: LEAD,
        part: LEAD_PART,
        anchor: 0,
    }];
    let live = project
        .compile_live(
            &flags,
            0,
            &[LiveSegment {
                from: 0,
                song: &project,
                flags: &flags,
                active: &active,
                timeline: None,
                audition: None,
            }],
        )
        .unwrap();
    let window = live.live_window.unwrap();
    assert_eq!((window.start, window.end), (0, LIVE_RING_STEPS));
    let track = track_of(&project, &live, 1);
    let arranged = track_of(&project, &arrangement, 1);
    // Placement 2 plays part 2 (Moving) in Song phase from step 64.
    for step in 64..256u64 {
        let a = arrangement.event_for(arranged, step);
        let b = live.event_for(track, step);
        assert_eq!(a.map(|e| e.midi), b.map(|e| e.midi), "step {step}");
    }
    assert!(live
        .provenance
        .iter()
        .all(|p| p.lane == LEAD && p.part == LEAD_PART.part));
}

#[test]
fn a_pending_launch_starts_on_its_boundary_and_restart_phase_counts_from_there() {
    let project = demo_with_parts();
    let flags = LiveFlags::default();
    let heard: [Override; 0] = [];
    let launched = [Override {
        lane: DRUMS,
        part: KICK_PART,
        anchor: 32,
    }];
    let live = project
        .compile_live(
            &flags,
            0,
            &[
                LiveSegment {
                    from: 0,
                    song: &project,
                    flags: &flags,
                    active: &heard,
                    timeline: None,
                    audition: None,
                },
                LiveSegment {
                    from: 32,
                    song: &project,
                    flags: &flags,
                    active: &launched,
                    timeline: None,
                    audition: None,
                },
            ],
        )
        .unwrap();
    let kick = track_of(&project, &live, 3);
    assert!((0..32).all(|s| live.event_for(kick, s).is_none()));
    assert!(live.event_for(kick, 32).is_some());
    let first = live.provenance.iter().find(|p| p.song_step == 32).unwrap();
    assert_eq!(first.pattern_time, 0);
    assert_eq!(first.part, KICK_PART.part);
}

#[test]
fn the_ring_holds_steps_beyond_the_arrangement_and_is_silent_outside_its_window() {
    let project = demo_with_parts();
    let flags = LiveFlags::default();
    let active = [Override {
        lane: DRUMS,
        part: KICK_PART,
        anchor: 0,
    }];
    let base = 5 * LIVE_RING_STEPS + 64;
    let live = project
        .compile_live(
            &flags,
            base,
            &[LiveSegment {
                from: 0,
                song: &project,
                flags: &flags,
                active: &active,
                timeline: None,
                audition: None,
            }],
        )
        .unwrap();
    let window = live.live_window.unwrap();
    assert!(window.contains(base) && !window.contains(base - 1));
    // Restart phase from anchor 0: source time equals the live step.
    let event = live
        .provenance
        .iter()
        .find(|p| u64::from(p.song_step) == base)
        .expect("kick on the bar");
    assert_eq!(event.pattern_time, base as i64);
}

#[test]
fn boundaries_round_up_and_forgive_a_late_press() {
    let project = demo_with_parts();
    let mut session = Performance::new(&project);
    session.playing = true;
    session.next_step = 18;
    assert_eq!(session.boundary(&project, Boundary::Bar).unwrap(), 32);
    assert_eq!(session.boundary(&project, Boundary::Beat).unwrap(), 20);
    assert_eq!(session.boundary(&project, Boundary::Now).unwrap(), 18);
    assert_eq!(
        session
            .boundary(&project, Boundary::Bars { count: 4 })
            .unwrap(),
        64
    );
    // The downbeat just played: within one step of tolerance it still counts.
    session.next_step = 17;
    assert_eq!(session.boundary(&project, Boundary::Bar).unwrap(), 16);
    // The step about to play is itself the boundary.
    session.next_step = 16;
    assert_eq!(session.boundary(&project, Boundary::Bar).unwrap(), 16);
    // Harmony repeats every 64 steps.
    session.next_step = 70;
    assert_eq!(
        session
            .boundary(&project, Boundary::Cycle { pattern: 2 })
            .unwrap(),
        128
    );
    session.playing = false;
    assert_eq!(session.boundary(&project, Boundary::Bar).unwrap(), 70);
}

#[test]
fn pending_changes_land_in_order_and_newest_launch_wins_a_lane() {
    let project = demo_with_parts();
    let mut session = Performance::new(&project);
    session.playing = true;
    session.next_step = 3;
    session.enqueue(
        16,
        Change::Launch {
            lane: LEAD,
            part: LEAD_PART,
        },
        "lead".into(),
    );
    session.enqueue(
        32,
        Change::Launch {
            lane: LEAD,
            part: INTRO_PART,
        },
        "intro".into(),
    );
    assert_eq!(
        session.pending.len(),
        1,
        "a second launch replaces the first"
    );
    session.enqueue(
        16,
        Change::Launch {
            lane: BASS,
            part: BASS_PART,
        },
        "bass".into(),
    );
    session.next_step = 17;
    let landed = session.land_due();
    assert_eq!(landed.len(), 1);
    assert_eq!(session.lane_state(BASS).unwrap().anchor, 16);
    session.next_step = 33;
    session.land_due();
    assert_eq!(session.lane_state(LEAD).unwrap().part, INTRO_PART);
    assert!(session.pending.is_empty());
}

#[test]
fn song_changes_never_overtake_each_other_and_compile_from_their_step() {
    let project = demo_with_parts();
    let flags = LiveFlags::default();
    let mut session = Performance::new(&project);
    session.playing = true;
    session.next_step = 5;
    session.enqueue(
        0,
        Change::Launch {
            lane: LEAD,
            part: LEAD_PART,
        },
        "lead".into(),
    );
    session.land_due();
    let mut transposed = project.clone();
    let mut part = transposed.part(2).unwrap().clone();
    part.transpose = 12;
    transposed.apply(Edit::PutPart { part }).unwrap();
    let transposed = Arc::new(transposed);
    session.enqueue(
        128,
        Change::Song {
            project: Arc::clone(&transposed),
            edits: None,
        },
        "up an octave".into(),
    );
    let later = session.enqueue(
        8,
        Change::Song {
            project: transposed,
            edits: None,
        },
        "same".into(),
    );
    assert_eq!(
        session.pending.iter().find(|p| p.id == later).unwrap().at,
        128
    );
    let compiled = session.compile(&project, &flags, None).unwrap();
    let lead = compiled.instrument_index(1).unwrap();
    let reference = project.compile().unwrap();
    let r = reference.instrument_index(1).unwrap();
    // Song phase: live step s and song step s read the same source time.
    let mut compared = (0, 0);
    for step in 64..256u64 {
        let live = compiled.event_for(lead, step);
        let song = reference.event_for(r, step);
        assert_eq!(live.is_some(), song.is_some(), "step {step}");
        if let (Some(live), Some(song)) = (live, song) {
            let shift = if step < 128 { 0 } else { 12 };
            assert_eq!(live.midi, song.midi + shift, "step {step}");
            if step < 128 {
                compared.0 += 1
            } else {
                compared.1 += 1
            }
        }
    }
    assert!(compared.0 > 0 && compared.1 > 0);
}

#[test]
fn the_engine_plays_a_live_window_far_past_the_arrangement_and_is_silent_after_it() {
    use cadence_engine::audio::render_schedule_report;
    let project = demo_with_parts();
    let flags = LiveFlags::default();
    let active = [Override {
        lane: DRUMS,
        part: KICK_PART,
        anchor: 0,
    }];
    let base = 7 * LIVE_RING_STEPS + 32;
    let live = project
        .compile_live(
            &flags,
            base,
            &[LiveSegment {
                from: 0,
                song: &project,
                flags: &flags,
                active: &active,
                timeline: None,
                audition: None,
            }],
        )
        .unwrap();
    let folder = tempfile::tempdir().unwrap();
    let render = |start: u64| {
        let out = folder.path().join(format!("{start}.wav"));
        render_schedule_report(&[(start, &live)], start, 32, Vec::new(), &out, 48_000)
            .unwrap()
            .output
            .peak[0]
    };
    assert!(render(base) > 0.01, "inside the window, past 256 bars");
    assert_eq!(
        render(base + LIVE_RING_STEPS),
        0.0,
        "a missed extension is silence, never a repeat"
    );
}

#[test]
fn song_phase_overrides_and_modulations_follow_timeline_loop_passes_and_continuing_ends() {
    use cadence_core::music::EndRule;
    use cadence_core::timeline::Segment;
    let project = demo_with_parts();
    let flags = LiveFlags::default();
    // Loop bars 5..9 (steps 64..128) while Lead plays Moving (5-step motif + 64-step harmony) in Song phase.
    // Because 64 % 5 != 0, reading transport step vs timeline song_step produces DIFFERENT notes on pass 2!
    let loop_seg = Segment::new(0, 64, Some((64, 64)), EndRule::Loop);
    let active = [Override {
        lane: LEAD,
        part: LEAD_PART,
        anchor: 0,
    }];
    let live = project
        .compile_live(
            &flags,
            0,
            &[LiveSegment {
                from: 0,
                song: &project,
                flags: &flags,
                active: &active,
                timeline: Some(loop_seg),
                audition: None,
            }],
        )
        .unwrap();
    let lead = track_of(&project, &live, 1);
    for offset in 0..64u64 {
        let pass1 = live.event_for(lead, offset);
        let pass2 = live.event_for(lead, 64 + offset);
        assert_eq!(
            pass1.map(|e| e.midi),
            pass2.map(|e| e.midi),
            "song-phase override must repeat identically across loop passes (offset {offset})"
        );
    }

    // Past a continuing end, continuous modulation holds the song's final step.
    let mut cont = project.clone();
    cont.end = EndRule::Continue;
    let song_len = cont.arrangement_steps() as u64;
    let cont_seg = Segment::new(0, 0, None, EndRule::Continue);
    let compiled = cont
        .compile_live(
            &flags,
            0,
            &[LiveSegment {
                from: 0,
                song: &cont,
                flags: &flags,
                active: &[],
                timeline: Some(cont_seg),
                audition: None,
            }],
        )
        .unwrap();
    let target = cadence_core::mixer::MixerParamTarget::ChannelGainScale {
        channel: cadence_core::mixer::ChannelRef::Channel(4),
    };
    let at_end = compiled.resolve_mixer_param(target, (song_len - 1) as f64);
    let past_end = compiled.resolve_mixer_param(target, (song_len + 32) as f64);
    assert!((at_end - past_end).abs() < 1e-6);
}
