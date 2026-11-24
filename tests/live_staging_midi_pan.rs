//! Integration tests for live staging, MIDI CC automation recording, MIDI Learn
//! parity, Smart Macro banks, MIDI pad LEDs/config persistence, and stereo pan automation.
use cadence_app::host_io::{HostIo, MemoryHostIo};
use cadence_app::view::{DeckView, GraphEdgeKind, GraphTarget};
use cadence_app::{parse_key_sequence, Action, App, ControlTarget};
use cadence_core::live::{Change, StagedChange};
use cadence_core::material::{
    Boundary, Id, Modulation, ModulationTarget, Pattern, PatternKind, Project, ValuePoint,
    ValueUnit,
};
use cadence_core::midi_map::{MidiMapping, MidiPadMode, MidiSource, MidiTarget, MidiValueMode};
use cadence_core::mixer::{ChannelParam, ChannelRef, EffectParam, MixerParamTarget};
use cadence_core::params::{InstrumentParam, ModuleParam, ParamAddress, ValueMode};
use cadence_core::runtime::{AudioSnapshot, MidiMessage};
use cadence_engine::audio::render_schedule_pcm;
use std::path::PathBuf;
use std::sync::Arc;

fn demo_app() -> App {
    let mut app = App::new(Project::demo(), PathBuf::from("live_staging.json")).unwrap();
    app.execute_command("lane Drums add Kick restart").unwrap();
    app
}

fn lane_id(app: &App, name: &str) -> Id {
    app.project()
        .lanes
        .iter()
        .find(|l| l.name == name)
        .unwrap()
        .id
}

fn keys(app: &mut App, seq: &str) {
    for key in parse_key_sequence(seq) {
        app.handle_script_key(key).unwrap();
    }
}

fn frame(app: &App) -> cadence_app::view::FrameView {
    let snap = AudioSnapshot {
        absolute_step: app.live.next_step,
        playing: app.transport.playing,
        sample_rate: 48_000,
        ..AudioSnapshot::default()
    };
    app.frame_view(&snap, "test")
}

fn slice_rms_lr(pcm: &[[i16; 2]], start_frac: f64, end_frac: f64) -> (f32, f32) {
    let a = ((pcm.len() as f64) * start_frac) as usize;
    let b = (((pcm.len() as f64) * end_frac) as usize).min(pcm.len());
    let mut sum_l = 0.0_f64;
    let mut sum_r = 0.0_f64;
    let n = (b.saturating_sub(a)).max(1) as f64;
    for &[l, r] in &pcm[a..b] {
        let lf = f64::from(l) / 32768.0;
        let rf = f64::from(r) / 32768.0;
        sum_l += lf * lf;
        sum_r += rf * rf;
    }
    ((sum_l / n).sqrt() as f32, (sum_r / n).sqrt() as f32)
}

#[test]
fn live_staging_s_key_queues_launches_releases_mutes_solos_section_jump_and_fires_atomically() {
    let mut app = demo_app();
    let lead = lane_id(&app, "Lead");
    let bass = lane_id(&app, "Bass");
    let drums = lane_id(&app, "Drums");
    let motion_id = app
        .project()
        .sections
        .iter()
        .find(|s| s.name == "Motion")
        .unwrap()
        .id;

    // 1. First `S` enters staging; second `S` with 0 changes exits cleanly with "Exited staging".
    keys(&mut app, "S");
    assert_eq!(app.staging().map(<[_]>::len), Some(0));
    keys(&mut app, "S");
    assert!(app.staging().is_none());
    assert_eq!(app.status, "Exited staging");

    // Start playing and launch Drums override now so we can stage releasing it.
    app.apply(Action::Play { playing: true }).unwrap();
    let kick_lp = app.project().lane(drums).unwrap().parts[0];
    app.apply(Action::Launch {
        lane: drums,
        part: kick_lp,
        at: Some(Boundary::Now),
    })
    .unwrap();
    app.apply(Action::Advance { steps: 4 }).unwrap();
    assert!(app.live.lane_state(drums).is_some());
    assert!(app.live.pending.is_empty());

    // 2. Enter staging via `S`, queue Launch (Lead part 2), Release (Drums), Mute (Bass),
    // Solo (Lead), and Section jump (Alt-2 -> Motion).
    let rev_before = app.revision();
    keys(&mut app, "S<m-q>2<m-e><m-x><m-w><m-m><m-q><m-s><m-2>");
    assert_eq!(app.revision(), rev_before, "staging never alters song rev");
    assert!(
        app.live.pending.is_empty(),
        "staged gestures must not enqueue pending until fired"
    );
    assert!(!app.flags().lane_muted(bass));
    assert!(!app.flags().lane_solo(lead));
    assert_eq!(app.staging().unwrap().len(), 5);
    assert_eq!(app.staged_section(), Some(motion_id));

    // 3. `u` pops the most recent staged change (JumpSection) instead of undoing song edits!
    keys(&mut app, "u");
    assert_eq!(app.revision(), rev_before);
    assert_eq!(app.staging().unwrap().len(), 4);
    assert_eq!(app.staged_section(), None);

    // Re-stage section 2 and inspect semantic view projections in Chrome, Arrange, and Bank!
    keys(&mut app, "<m-2>");
    assert_eq!(app.staging().unwrap().len(), 5);
    assert_eq!(app.staged_section(), Some(motion_id));

    let fv = frame(&app);
    assert_eq!(fv.chrome.staged_count, Some(5));
    let arr = fv
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Arrange(v) => Some(v),
            _ => None,
        })
        .expect("Arrange view");
    assert!(arr.staging_active);
    assert!(arr.sections.iter().any(|s| s.id == motion_id && s.staged));
    let arr_lead = arr.lanes.iter().find(|l| l.id == lead).unwrap();
    assert_eq!(arr_lead.staged_solo, Some(true));
    assert!(!arr_lead.staged.is_empty());
    let arr_bass = arr.lanes.iter().find(|l| l.id == bass).unwrap();
    assert_eq!(arr_bass.staged_mute, Some(true));
    let arr_drums = arr.lanes.iter().find(|l| l.id == drums).unwrap();
    assert!(arr_drums.staged.iter().any(|m| m.is_release));

    let bank = fv
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Bank(v) => Some(v),
            _ => None,
        })
        .expect("Bank view");
    assert_eq!(bank.staging_count, Some(5));
    assert!(bank.sections.iter().any(|s| s.name == "Motion" && s.staged));
    let b_lead = bank.lanes.iter().find(|l| l.lane_id == lead).unwrap();
    assert_eq!(b_lead.staged_solo, Some(true));
    assert!(b_lead.parts[1].staged_launch);
    let b_drums = bank.lanes.iter().find(|l| l.lane_id == drums).unwrap();
    assert!(b_drums.staged_release);
    assert!(b_drums.parts[0].staged_release);

    // 4. Second `S` fires ALL 5 staged changes atomically on bar 2 (step 16)!
    keys(&mut app, "S");
    assert!(app.staging().is_none());
    assert!(
        !app.live.pending.is_empty(),
        "fired staged changes wait for bar 2"
    );
    assert!(
        app.live.pending.iter().all(|p| p.at == 16),
        "all staged changes including section jump must share exact boundary step 16: {:?}",
        app.live
            .pending
            .iter()
            .map(|p| (p.at, &p.label))
            .collect::<Vec<_>>()
    );
    assert!(app
        .live
        .pending
        .iter()
        .any(|p| matches!(p.change, Change::Timeline { segment } if segment.song == 64)));

    // 5. Advance across step 16 -> every staged change lands together atomically!
    app.apply(Action::Advance { steps: 13 }).unwrap();
    assert!(app.live.pending.is_empty());
    assert!(app.live.lane_state(lead).is_some());
    assert!(app.live.lane_state(drums).is_none());
    assert!(app.flags().lane_muted(bass));
    assert!(app.flags().lane_solo(lead));
    assert_eq!(
        app.song_step_heard(),
        64,
        "staged section jump landed at bar 2 (step 16) -> Motion starts at song step 64"
    );
}

#[test]
fn live_staging_esc_unstage_jam_s_and_alt_enter_prefix_all_work() {
    let mut app = demo_app();
    let bass = lane_id(&app, "Bass");
    let lead = lane_id(&app, "Lead");
    app.apply(Action::Play { playing: true }).unwrap();
    app.apply(Action::Advance { steps: 4 }).unwrap();

    // 1. Normal mode `Esc` discards staged changes without leaving Arrange or applying them.
    keys(&mut app, "S<m-w><m-m><m-2>");
    assert_eq!(app.staging().unwrap().len(), 2);
    keys(&mut app, "<Esc>");
    assert!(app.staging().is_none());
    assert!(!app.flags().lane_muted(bass));
    assert!(app.live.pending.is_empty());

    // 2. `:unstage` discards staged changes as alias for `:stage discard`.
    keys(&mut app, "S<m-q>1");
    assert_eq!(app.staging().unwrap().len(), 1);
    app.execute_command("unstage").unwrap();
    assert!(app.staging().is_none());
    assert!(app.live.pending.is_empty());

    // 3. In Jam mode, `S` plays accented C# (never shadowing NOTE_KEYS!), while `Alt-Enter`
    // enters staging, collects section jump `2` + lane launch `q1`, and `Enter` fires!
    keys(&mut app, "gj");
    assert!(app.is_jam());
    let accented_cs = app
        .handle_key(cadence_app::KeyEvent::new(
            cadence_app::KeyCode::Char('S'),
            cadence_app::KeyModifiers::SHIFT,
        ))
        .unwrap();
    assert!(
        app.staging().is_none(),
        "Jam Shift+S must play accented C#, NOT shadow NOTE_KEYS with staging"
    );
    assert_eq!(accented_cs.live_notes_on.len(), 1);
    assert_eq!(accented_cs.live_notes_on[0].midi % 12, 1); // C#
    assert!((accented_cs.live_notes_on[0].velocity - 1.0).abs() < 1e-6);

    keys(&mut app, "<m-Enter>2q1");
    assert!(app.is_jam());
    assert_eq!(
        app.staging().unwrap(),
        &[
            StagedChange::JumpSection {
                section: app.project().sections[1].id
            },
            StagedChange::Launch {
                lane: lead,
                part: app.project().lane(lead).unwrap().parts[0],
            },
        ]
    );
    keys(&mut app, "<Enter>");
    assert!(app.is_jam());
    assert!(app.staging().is_none());
    assert!(app.live.pending.iter().all(|p| p.at == 16));
    assert_eq!(app.live.pending.len(), 2);
    keys(&mut app, "<Esc>");
    assert!(!app.is_jam());
}

#[test]
fn live_midi_cc_recording_creates_overdubs_and_undoes_synth_effect_gain_pan_and_pitchbend() {
    let mut app = demo_app();
    app.config.midi_pickup = false;
    let inst_id = app.project().instruments[0].id;
    let delay_dev_id = app.project().mixer.channels[4].inserts[0].id;
    let lead_ch = ChannelRef::Channel(app.project().mixer.channels[0].id);
    let bass_ch = ChannelRef::Channel(app.project().mixer.channels[1].id);
    let orig_bass_gain = app
        .project()
        .channel(app.project().mixer.channels[1].id)
        .unwrap()
        .strip
        .gain;

    // Bind explicit MIDI CC & PitchBend mappings across Rack Module, Effect, ChannelGain, ChannelPan, and MasterGain.
    // On `Project::demo()`, instrument 0 ("Saw") has Filter module #4 in zone #1.
    let saw_cutoff_param = InstrumentParam::Module {
        zone: 1,
        module: 4,
        param: ModuleParam::Cutoff,
    };
    let saw_res_param = InstrumentParam::Module {
        zone: 1,
        module: 4,
        param: ModuleParam::Resonance,
    };
    assert_eq!(
        InstrumentParam::parse_for_rack("cutoff", &app.project().instruments[0].rack, true),
        Some(saw_cutoff_param)
    );
    app.config.midi_mappings = vec![
        MidiMapping::new(
            1,
            MidiSource::Cc {
                channel: Some(0),
                cc: 74,
            },
            MidiValueMode::Absolute,
            MidiTarget::FocusedMacro { slot: 0 }, // Saw Filter Cutoff
        ),
        MidiMapping::new(
            2,
            MidiSource::Cc {
                channel: Some(0),
                cc: 20,
            },
            MidiValueMode::Absolute,
            MidiTarget::Param {
                address: ParamAddress::Effect {
                    device: delay_dev_id,
                    param: EffectParam::Feedback,
                },
            },
        ),
        MidiMapping::new(
            3,
            MidiSource::Cc {
                channel: Some(0),
                cc: 21,
            },
            MidiValueMode::Absolute,
            MidiTarget::LaneGain { lane_index: 1 }, // Bass ChannelGain
        ),
        MidiMapping::new(
            4,
            MidiSource::Cc {
                channel: Some(0),
                cc: 22,
            },
            MidiValueMode::Absolute,
            MidiTarget::Param {
                address: ParamAddress::Channel {
                    channel: lead_ch,
                    param: ChannelParam::Pan,
                },
            },
        ),
        MidiMapping::new(
            5,
            MidiSource::Cc {
                channel: Some(0),
                cc: 23,
            },
            MidiValueMode::Absolute,
            MidiTarget::Param {
                address: ParamAddress::Channel {
                    channel: ChannelRef::Master,
                    param: ChannelParam::Gain,
                },
            },
        ),
        MidiMapping::new(
            6,
            MidiSource::PitchBend { channel: Some(0) },
            MidiValueMode::Absolute,
            MidiTarget::Param {
                address: ParamAddress::from_instrument(inst_id, saw_res_param),
            },
        ),
    ];

    // Arm MIDI Record (`R`) while STOPPED: CC adjusts live param without creating a modulation lane.
    keys(&mut app, "R");
    assert!(app.midi.record);
    let mods_initial = app.project().modulations.len();
    app.handle_midi(MidiMessage::Control {
        channel: 0,
        controller: 74,
        value: 64,
    })
    .unwrap();
    assert_eq!(
        app.project().modulations.len(),
        mods_initial,
        "stopped CC moves must not create automation lanes"
    );

    // Start playback!
    app.apply(Action::Play { playing: true }).unwrap();

    // 1. Record FocusedMacro 0 (Saw Cutoff) across steps 0, 4, 8 -> single lazy Values pattern + Modulation!
    app.handle_midi(MidiMessage::Control {
        channel: 0,
        controller: 74,
        value: 25,
    })
    .unwrap();
    app.apply(Action::Advance { steps: 5 }).unwrap(); // transport.step = 4
    app.handle_midi(MidiMessage::Control {
        channel: 0,
        controller: 74,
        value: 95,
    })
    .unwrap();
    app.apply(Action::Advance { steps: 4 }).unwrap(); // transport.step = 8
    app.handle_midi(MidiMessage::Control {
        channel: 0,
        controller: 74,
        value: 127,
    })
    .unwrap();

    let cutoff_target = ModulationTarget::Instrument {
        instrument: inst_id,
        parameter: saw_cutoff_param,
    };
    let cutoff_mod = app
        .project()
        .modulations
        .iter()
        .find(|m| m.target == cutoff_target)
        .expect("lazy Rack Filter Cutoff modulation created");
    let PatternKind::Values {
        unit,
        period,
        mode,
        points,
    } = &app.project().pattern(cutoff_mod.pattern).unwrap().kind
    else {
        panic!("expected Values pattern");
    };
    assert_eq!(*unit, ValueUnit::Normalized);
    assert_eq!(*mode, ValueMode::Ramp);
    assert_eq!(*period, app.project().arrangement_steps() as u32);
    assert_eq!(points.len(), 3);
    assert_eq!(points[0].step, 0);
    assert_eq!(points[1].step, 4);
    assert_eq!(points[2].step, 8);
    assert!((points[2].value - 1.0).abs() < 1e-4);
    // Compiled audio score must contain the live updated ModLane!
    assert!(app
        .audio_compiled()
        .mod_lanes
        .iter()
        .any(|l| { l.target == saw_cutoff_param && (l.values[8] - 1.0).abs() < 1e-3 }));

    // Undo once: coalesced CC pass reverts atomically (removes lazy pattern + modulation)!
    app.apply(Action::Undo).unwrap();
    assert!(
        !app.project()
            .modulations
            .iter()
            .any(|m| m.target == cutoff_target),
        "single atomic undo must revert the coalesced CC pass"
    );

    // 2. Effect parameter CC recording (Delay Feedback).
    app.handle_midi(MidiMessage::Control {
        channel: 0,
        controller: 20,
        value: 100,
    })
    .unwrap();
    assert!(app.project().modulations.iter().any(|m| {
        m.target
            == ModulationTarget::Effect {
                device: delay_dev_id,
                parameter: EffectParam::Feedback,
            }
    }));

    // 3. LaneGain (Bass ChannelGain) sets unity trim 1.0 + lazy ChannelGain automation; Undo restores trim!
    app.handle_midi(MidiMessage::Control {
        channel: 0,
        controller: 21,
        value: 64,
    })
    .unwrap();
    let bass_ch_id = app.project().mixer.channels[1].id;
    assert!((app.project().channel(bass_ch_id).unwrap().strip.gain - 1.0).abs() < 1e-6);
    assert!(app
        .project()
        .modulations
        .iter()
        .any(|m| { m.target == ModulationTarget::ChannelGain { channel: bass_ch } }));
    app.apply(Action::Undo).unwrap();
    assert!(
        (app.project().channel(bass_ch_id).unwrap().strip.gain - orig_bass_gain).abs() < 1e-6,
        "undoing lazy ChannelGain recording must restore original strip trim"
    );
    assert!(!app
        .project()
        .modulations
        .iter()
        .any(|m| { m.target == ModulationTarget::ChannelGain { channel: bass_ch } }));

    // 4. MasterGain CC recording.
    app.handle_midi(MidiMessage::Control {
        channel: 0,
        controller: 23,
        value: 102,
    })
    .unwrap();
    assert!(app.project().modulations.iter().any(|m| {
        m.target
            == ModulationTarget::ChannelGain {
                channel: ChannelRef::Master,
            }
    }));

    // 5. PitchBend mapped to Rack Filter Resonance records cleanly.
    app.handle_midi(MidiMessage::PitchBend {
        channel: 0,
        value: 4096,
    })
    .unwrap();
    assert!(app.project().modulations.iter().any(|m| {
        m.target
            == ModulationTarget::Instrument {
                instrument: inst_id,
                parameter: saw_res_param,
            }
    }));

    // 6. ChannelPan CC recording + Chrome & MIDI Deck view indicators!
    app.handle_midi(MidiMessage::Control {
        channel: 0,
        controller: 22,
        value: 0, // hard left
    })
    .unwrap();
    app.apply(Action::Advance { steps: 4 }).unwrap();
    app.handle_midi(MidiMessage::Control {
        channel: 0,
        controller: 22,
        value: 127, // hard right
    })
    .unwrap();
    let pan_mod = app
        .project()
        .modulations
        .iter()
        .find(|m| m.target == ModulationTarget::ChannelPan { channel: lead_ch })
        .expect("lazy ChannelPan modulation created");
    let PatternKind::Values { points, .. } = &app.project().pattern(pan_mod.pattern).unwrap().kind
    else {
        panic!("expected Values pattern");
    };
    assert_eq!(points.len(), 2);
    assert!(points[0].value < 0.01);
    assert!(points[1].value > 0.99);

    // Chrome and MIDI Deck expose REC + last recorded CC target ("Lead Pan")!
    keys(&mut app, "gd");
    let fv = frame(&app);
    assert!(fv.chrome.midi_recording);
    assert_eq!(fv.chrome.midi_cc_last_recorded.as_deref(), Some("Lead Pan"));
    let midi_view = fv
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Midi(m) => Some(m),
            _ => None,
        })
        .expect("MidiDeckView");
    assert!(midi_view.recording);
    assert_eq!(midi_view.cc_last_recorded.as_deref(), Some("Lead Pan"));
}

#[test]
fn channel_pan_modulation_audibly_shifts_stereo_left_to_right_and_projects_to_mixer_and_graph() {
    let mut project = Project::demo();
    // Mute Echo send so stereo measurement isolates Lead's direct channel pan cleanly.
    if let Some(music) = project.mixer.channels.iter_mut().find(|c| c.id == 4) {
        music.sends.clear();
    }
    let lead_ch = ChannelRef::Channel(1);
    let pat_id = project.max_id() + 1;
    let mod_id = pat_id + 1;
    project.patterns.push(Pattern {
        id: pat_id,
        name: "Lead Pan Sweep".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 64,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 0.0, // hard Left (-1.0)
                },
                ValuePoint {
                    step: 16,
                    value: 0.0, // hold hard Left through bar 1
                },
                ValuePoint {
                    step: 32,
                    value: 1.0, // hard Right (+1.0)
                },
                ValuePoint {
                    step: 63,
                    value: 1.0, // hold hard Right through bar 4
                },
            ],
        },
    });
    project.modulations.push(Modulation {
        id: mod_id,
        name: "Lead Auto Pan".into(),
        target: ModulationTarget::ChannelPan { channel: lead_ch },
        pattern: pat_id,
    });

    // 1. Validate rejects missing channel IDs on ChannelPan.
    let mut bad = project.clone();
    bad.modulations.last_mut().unwrap().target = ModulationTarget::ChannelPan {
        channel: ChannelRef::Channel(9999),
    };
    assert!(bad.validate().is_err());

    // 2. Compile produces MixerAutomation targeting ChannelParam::Pan with 0..1 points.
    let score = project.compile().unwrap();
    let pan_auto = score
        .mixer
        .automations
        .iter()
        .find(|a| {
            a.target
                == MixerParamTarget::Channel {
                    channel: lead_ch,
                    param: ChannelParam::Pan,
                }
        })
        .expect("compiled ChannelPan MixerAutomation");
    assert!(pan_auto.points[0].value < 0.01);
    assert!(pan_auto.points[32].value > 0.99);

    // 3. Real 24 kHz audio render verifies L-dominant energy in bar 1 and R-dominant energy in bar 3-4!
    let (rep, pcm) = render_schedule_pcm(&[(0, &score)], 0, 64, Vec::new(), 24_000).unwrap();
    assert_eq!(rep.output.full_scale_samples, 0);
    let (early_l, early_r) = slice_rms_lr(&pcm, 0.02, 0.24);
    let (late_l, late_r) = slice_rms_lr(&pcm, 0.55, 0.95);
    assert!(
        early_l > 0.02 && early_l > early_r * 2.5,
        "bar 1 must pan hard Left (L={early_l}, R={early_r})"
    );
    assert!(
        late_r > 0.02 && late_r > late_l * 2.5,
        "bars 3-4 must pan hard Right (L={late_l}, R={late_r})"
    );

    // 4. MixerDeckView projects live effective_pan + pan_modulators, and GraphDeckView wires the edge!
    let mut app = App::new(project, PathBuf::from("pan_test.json")).unwrap();
    keys(&mut app, "gm");
    let snap_left = AudioSnapshot {
        playing: true,
        absolute_step: 4,
        sample_rate: 48_000,
        ..AudioSnapshot::default()
    };
    let fv_l = app.frame_view(&snap_left, "test");
    let mix_l = fv_l
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Mixer(m) => Some(m),
            _ => None,
        })
        .unwrap();
    let ch_l = mix_l
        .channels
        .iter()
        .find(|c| c.channel == lead_ch)
        .unwrap();
    assert_eq!(ch_l.pan_modulators, vec!["Lead Pan Sweep".to_string()]);
    assert!(ch_l.effective_pan.unwrap() < -0.95);

    let snap_right = AudioSnapshot {
        playing: true,
        absolute_step: 40,
        sample_rate: 48_000,
        ..AudioSnapshot::default()
    };
    let fv_r = app.frame_view(&snap_right, "test");
    let mix_r = fv_r
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Mixer(m) => Some(m),
            _ => None,
        })
        .unwrap();
    let ch_r = mix_r
        .channels
        .iter()
        .find(|c| c.channel == lead_ch)
        .unwrap();
    assert!(ch_r.effective_pan.unwrap() > 0.95);

    // Graph deck (`gG`) connects Lead Pan Sweep pattern -> Lead Channel via Modulation edge!
    keys(&mut app, "gG");
    let fv_g = app.frame_view(&snap_right, "test");
    let graph = fv_g
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Graph(g) => Some(g),
            _ => None,
        })
        .unwrap();
    let pat_pos = graph
        .columns
        .iter()
        .flat_map(|c| &c.nodes)
        .find(|n| n.target == GraphTarget::Pattern(pat_id))
        .map(|n| (n.col, n.row))
        .expect("pan pattern node");
    let ch_pos = graph
        .columns
        .iter()
        .flat_map(|c| &c.nodes)
        .find(|n| n.target == GraphTarget::Channel(lead_ch))
        .map(|n| (n.col, n.row))
        .expect("lead channel node");
    assert!(
        graph.edges.iter().any(|e| {
            e.kind == GraphEdgeKind::Modulation
                && (e.from_col, e.from_row) == pat_pos
                && (e.to_col, e.to_row) == ch_pos
        }),
        "Graph must wire ChannelPan modulation pattern -> Channel"
    );
}

#[test]
fn midi_learn_automatable_parity_and_sampler_smart_macros() {
    let mut app = App::new(Project::demo(), "unused.json".into()).unwrap();
    app.execute_command("instrument SampMacro sampler").unwrap();

    // 1. Default Sampler Rack 8-knob Smart Macro bank maps continuous automatable params.
    let mut samp_app = App::new(Project::empty(), "unused.json".into()).unwrap();
    samp_app
        .execute_command("instrument SampMacro sampler")
        .unwrap();
    let samp_only_id = samp_app.selected_instrument.unwrap();
    samp_app.press_keys("ga").unwrap();
    let targets = samp_app.focused_macro_targets();
    let expected = [
        (2u16, ModuleParam::Cutoff),
        (2, ModuleParam::Resonance),
        (4, ModuleParam::Attack),
        (4, ModuleParam::Decay),
        (4, ModuleParam::Sustain),
        (4, ModuleParam::Release),
        (5, ModuleParam::Pan),
        (1, ModuleParam::Fine),
    ];
    assert_eq!(targets.len(), 8);
    for ((_, addr), (exp_mod, exp_param)) in targets.iter().zip(expected) {
        assert_eq!(
            *addr,
            ParamAddress::Module {
                instrument: samp_only_id,
                zone: 1,
                module: exp_mod,
                param: exp_param,
            }
        );
        assert!(addr.is_automatable());
    }

    // 2. MIDI Learn rejects non-automatable controls on Instrument Deck (e.g. Sampler LoopMode).
    app.execute_command("instrument SampMacro").unwrap();
    app.press_keys("l").unwrap();
    let loop_row = app
        .control_descriptors()
        .iter()
        .position(|c| {
            matches!(
                c.target,
                ControlTarget::Param(ParamAddress::Module {
                    param: ModuleParam::LoopMode,
                    ..
                })
            )
        })
        .expect("LoopMode row on Sampler module faceplate");
    app.control_cursor = loop_row;
    assert!(
        app.learn_target_from_focus().is_none(),
        "MIDI Learn must reject non-automatable ModuleParam::LoopMode"
    );

    // Step back to Quick pins pill (slot 0) where Cutoff lives.
    app.press_keys("h").unwrap();
    let cutoff_row = app
        .control_descriptors()
        .iter()
        .position(|c| {
            matches!(
                c.target,
                ControlTarget::Param(ParamAddress::Module {
                    param: ModuleParam::Cutoff,
                    ..
                })
            )
        })
        .expect("Cutoff row on Quick panel");
    app.control_cursor = cutoff_row;
    assert!(app.learn_target_from_focus().is_some());
}

#[test]
fn staged_release_all_stop_clears_staging_and_alt_enter_exits_empty() {
    let mut app = demo_app();
    app.apply(Action::Play { playing: true }).unwrap();

    // Launch Lead override so we have an active override to release.
    let lead_lane = lane_id(&app, "Lead");
    let lead_part = app.project().lane(lead_lane).unwrap().parts[1];
    app.apply(Action::Launch {
        lane: lead_lane,
        part: lead_part,
        at: None,
    })
    .unwrap();
    app.apply(Action::Advance { steps: 17 }).unwrap();
    assert!(app.live.lane_state(lead_lane).is_some());

    // Enter staging (`S`) -> `Action::ReleaseAll` stages a Release instead of releasing immediately.
    app.press_keys("S").unwrap();
    assert!(app.staging().is_some());
    app.apply(Action::ReleaseAll { at: None }).unwrap();
    assert!(
        app.live.lane_state(lead_lane).is_some(),
        "release_all while staging must stage Release, not release immediately"
    );
    assert_eq!(app.staging().unwrap().len(), 1);

    // Full Action::Stop clears staging.
    app.apply(Action::Stop).unwrap();
    assert!(
        app.staging().is_none(),
        "Full Action::Stop must clear active staging"
    );

    // Alt-Enter aligns with `S`: entering empty staging then pressing Alt-Enter exits cleanly.
    app.press_keys("<m-Enter>").unwrap();
    assert!(app.staging().is_some());
    app.press_keys("<m-Enter>").unwrap();
    assert!(
        app.staging().is_none(),
        "Second Alt-Enter with empty staging exits cleanly just like S"
    );
}

#[test]
fn midi_cc_overdub_resets_channel_strip_gain_to_unity() {
    let mut overdub_proj = Project::demo();
    let ch_id = overdub_proj.mixer.channels[0].id;
    let ch_ref = ChannelRef::Channel(ch_id);
    let pat_id = overdub_proj.max_id() + 1;
    let mod_id = overdub_proj.max_id() + 2;
    overdub_proj.mixer.channels[0].strip.gain = 0.5;
    overdub_proj.patterns.push(Pattern {
        id: pat_id,
        name: "GainMod".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 16,
            mode: ValueMode::Ramp,
            points: vec![ValuePoint {
                step: 0,
                value: 0.8,
            }],
        },
    });
    overdub_proj.modulations.push(Modulation {
        id: mod_id,
        name: "Ch0 Gain".into(),
        pattern: pat_id,
        target: ModulationTarget::ChannelGain { channel: ch_ref },
    });
    let mut overdub_app = App::new(overdub_proj, "overdub.json".into()).unwrap();
    overdub_app.config.midi_mappings.push(MidiMapping {
        id: 99,
        label: "Ch0 Gain".into(),
        source: MidiSource::Cc {
            channel: None,
            cc: 74,
        },
        mode: MidiValueMode::Absolute,
        target: MidiTarget::Param {
            address: ParamAddress::Channel {
                channel: ch_ref,
                param: ChannelParam::Gain,
            },
        },
        min: 0.0,
        max: 1.0,
        invert: false,
        pickup: false,
    });
    overdub_app.apply(Action::Play { playing: true }).unwrap();
    overdub_app.press_keys("R").unwrap();
    let _ = overdub_app.handle_midi_event(MidiMessage::Control {
        channel: 0,
        controller: 74,
        value: 96,
    });
    assert!(
        (overdub_app.channel(ch_id).unwrap().strip.gain - 1.0).abs() < 1e-4,
        "CC-overdubbing an existing ChannelGain modulation must reset strip.gain to 1.0"
    );
}

#[test]
fn midi_pad_mutes_reflect_staged_state_profile_validation_and_config_persistence() {
    let io = Arc::new(MemoryHostIo::new("/virtual"));
    let mut app = App::new_with_host_io(
        Project::demo(),
        "/virtual/midi.json".into(),
        Arc::clone(&io) as Arc<dyn HostIo>,
    )
    .unwrap();

    // 1. MIDI pad Mutes LEDs reflect staged mute state.
    app.config.midi_pad_mode = MidiPadMode::Mutes;
    app.config.midi_profile = "arturia_minilab3".into();
    app.press_keys("S").unwrap();
    let lane0 = app.project().lanes[0].id;
    app.apply(Action::SetLaneMuted {
        lane: lane0,
        muted: true,
    })
    .unwrap();
    let pads = app.compute_pad_states();
    assert_eq!(
        pads[0].1,
        (127, 16, 16),
        "MIDI pad Mutes LED must turn muted red when lane mute is staged"
    );
    assert!(!pads[0].2);
    app.apply(Action::Stop).unwrap();

    // 2. `:midi profile <unknown>` rejects bogus profile names.
    assert!(
        app.execute_command("midi profile nonexistent_hardware_9000")
            .is_err(),
        "`:midi profile` must validate profile names"
    );

    // 3. `:midi pads` & `:midi pickup` persist config via `HostIo`.
    app.apply_config(cadence_core::config::UserConfig::default());
    app.execute_command("midi pads sections").unwrap();
    assert_eq!(
        io.saved_config().unwrap().midi_pad_mode,
        MidiPadMode::Sections
    );
    app.execute_command("midi pickup off").unwrap();
    assert!(!io.saved_config().unwrap().midi_pickup);
}

#[test]
fn midi_deck_device_profile_navigation_crud_confirm_and_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let io = Arc::new(MemoryHostIo::new(dir.path()));
    let mut app = App::new_with_host_io(
        Project::demo(),
        dir.path().join("midi_deck.json"),
        Arc::clone(&io) as Arc<dyn HostIo>,
    )
    .unwrap();
    app.apply_config(cadence_core::config::UserConfig {
        midi_profile: "standard_cc".into(),
        ..cadence_core::config::UserConfig::default()
    });
    app.midi.in_ports = vec!["MiniLab 3 MIDI".into(), "Launchpad X".into()];
    app.midi.out_ports = vec!["MiniLab 3 MIDI".into()];

    // Push Arrange onto trail then open Devices · MIDI Deck (`gd`).
    keys(&mut app, "gagd");
    assert!(app.midi_focused());

    // 1. DEVICE & PROFILE vertical group (rows 0..5) above MAPPINGS (5..):
    //    bare `h`/`l` strictly no-op; `j`/`k` walks rows smoothly; `w`/`b` jumps groups; `0`/`G` jump ends.
    let mv0 = match &frame(&app)
        .decks
        .into_iter()
        .find(|d| matches!(d.view, DeckView::Midi(_)))
        .unwrap()
        .view
    {
        DeckView::Midi(m) => m.clone(),
        _ => unreachable!(),
    };
    assert_eq!(mv0.device_rows.len(), 5);
    assert_eq!(mv0.device_rows[0].label, "IN");
    assert_eq!(mv0.device_rows[1].label, "OUT");
    assert_eq!(mv0.device_rows[2].label, "Profile");
    assert_eq!(mv0.device_rows[3].label, "Pads");
    assert_eq!(mv0.device_rows[4].label, "Pickup");
    assert!(mv0.device_rows[0].selected);

    // Bare `h`/`l` is strictly a no-op on MIDI Deck.
    let pad_before = app.config.midi_pad_mode;
    let pickup_before = app.config.midi_pickup;
    keys(&mut app, "hl");
    assert_eq!(app.midi.cursor, 0);
    assert_eq!(app.config.midi_pad_mode, pad_before);
    assert_eq!(app.config.midi_pickup, pickup_before);

    // `j` steps through all 5 DEVICE & PROFILE rows and directly into MAPPINGS row 5.
    keys(&mut app, "jjjjj");
    assert_eq!(app.midi.cursor, 5);
    // `b` jumps back to DEVICE & PROFILE (row 0); `w` jumps forward to MAPPINGS (row 5).
    keys(&mut app, "b");
    assert_eq!(app.midi.cursor, 0);
    keys(&mut app, "w");
    assert_eq!(app.midi.cursor, 5);
    keys(&mut app, "0");
    assert_eq!(app.midi.cursor, 0);
    keys(&mut app, "G");
    assert!(app.midi.cursor >= 5);
    keys(&mut app, "0");

    // 2. Restored global `t` (Transport prefix), `i` (does not open IN port), and `Backspace` (back trail).
    let in_before = app.config.midi_in_port.clone();
    keys(&mut app, "i");
    assert_eq!(
        app.config.midi_in_port, in_before,
        "`i` must not hijack MIDI IN port on gd"
    );
    keys(&mut app, "t");
    assert_eq!(
        app.mode_name(),
        "TRANSPORT",
        "`t` must open global Transport prefix on gd"
    );
    keys(&mut app, "<Esc>");
    assert!(app.midi_focused());
    // `Backspace` invokes global back (unzooms `zz`) instead of cycling Pads or deleting mappings.
    keys(&mut app, "zz");
    assert!(app.view_state.zoomed);
    keys(&mut app, "<BS>");
    assert!(
        !app.view_state.zoomed,
        "`Backspace` must restore global back navigation (unzoom) on gd"
    );
    assert_eq!(app.config.midi_pad_mode, pad_before);
    assert!(app.midi_focused());

    // 3. `Alt+h/l` & `,/.` cycle values on DEVICE & PROFILE; `Enter` opens pickers / toggles Pickup.
    keys(&mut app, "0<Enter>");
    assert_eq!(app.mode_name(), "PICK");
    keys(&mut app, "j<Enter>"); // Choose "MiniLab 3 MIDI"
    assert_eq!(app.config.midi_in_port.as_deref(), Some("MiniLab 3 MIDI"));
    // Row 3 (Pads): `.` cycles pad mode; Row 4 (Pickup): `Enter` toggles pickup.
    keys(&mut app, "jjj.");
    assert_ne!(app.config.midi_pad_mode, pad_before);
    keys(&mut app, "j<Enter>");
    assert_ne!(app.config.midi_pickup, pickup_before);
    // `r` rescans MIDI ports cleanly.
    let rescan_out = app
        .handle_key(cadence_app::KeyEvent::new(
            cadence_app::KeyCode::Char('r'),
            cadence_app::KeyModifiers::NONE,
        ))
        .unwrap();
    assert!(rescan_out.midi_rescan);

    // 4. `a` "Add MIDI Mapping..." picker covers Transport / Sections / Lanes / Smart & Project Macros
    //    and arms MIDI Learn so the next incoming CC creates a user mapping!
    keys(&mut app, "a");
    assert_eq!(app.mode_name(), "PICK");
    let picker_labels: Vec<String> = match frame(&app).overlay {
        Some(cadence_app::view::OverlayView::Picker(pv)) => {
            pv.items.into_iter().map(|(label, _, _)| label).collect()
        }
        other => panic!("expected Add MIDI Mapping picker, got {other:?}"),
    };
    assert!(picker_labels.iter().any(|l| l.contains("Transport")));
    assert!(picker_labels.iter().any(|l| l.contains("Section")));
    assert!(picker_labels.iter().any(|l| l.contains("Lane")));
    assert!(picker_labels.iter().any(|l| l.contains("Smart Macro 1")));
    assert!(picker_labels.iter().any(|l| l.contains("Project Macro M1")));
    keys(&mut app, "<Enter>"); // Arm Learn on first target (Transport: Play / Pause)
    assert!(app.midi.learn.is_some());
    app.handle_midi(MidiMessage::Control {
        channel: 0,
        controller: 64,
        value: 127,
    })
    .unwrap();
    assert!(app.midi.learn.is_none());
    assert_eq!(app.config.midi_mappings.len(), 1);
    let user_id = app.config.midi_mappings[0].id;

    // 5. Navigate onto the user mapping (user mappings appear first at row 5), toggle invert (`v`),
    //    then verify Profile switch Confirm guard!
    app.midi.cursor = 5;
    assert!(!app.config.midi_mappings[0].invert);
    keys(&mut app, "v");
    assert!(
        app.config
            .midi_mappings
            .iter()
            .find(|m| m.id == user_id)
            .unwrap()
            .invert,
        "`v` must toggle invert on selected user mapping"
    );

    // Switching built-in Profile while user mappings exist triggers `Mode::Confirm`!
    app.midi.cursor = 2; // Profile row
    keys(&mut app, ".");
    assert_eq!(
        app.mode_name(),
        "CONFIRM",
        "Switching built-in Profile with user mappings must open Mode::Confirm"
    );
    keys(&mut app, "<Esc>");
    assert_eq!(app.mode_name(), "NORMAL");
    assert_eq!(
        app.config.midi_mappings.len(),
        1,
        "Cancelling profile switch confirm must preserve user mappings"
    );

    // 6. Save custom `.midi.json` profile, persist `midi_profile_path` in UserConfig, and reload cleanly!
    let custom_path = dir.path().join("my-rig.midi.json");
    app.execute_command(&format!("midi profile-save {}", custom_path.display()))
        .unwrap();
    assert!(io.exists(&custom_path));
    assert_eq!(
        app.config.midi_profile_path.as_deref(),
        Some(custom_path.as_path()),
        "Saving custom profile must persist midi_profile_path in UserConfig"
    );
    let persisted_cfg = io.saved_config().expect("persisted UserConfig");
    assert_eq!(
        persisted_cfg.midi_profile_path.as_deref(),
        Some(custom_path.as_path())
    );
    // Clean round-trip via serde JSON + reload into fresh App via HostIo.
    let cfg_json = serde_json::to_string(&persisted_cfg).unwrap();
    let cfg_back: cadence_core::config::UserConfig = serde_json::from_str(&cfg_json).unwrap();
    assert_eq!(cfg_back.midi_profile_path, persisted_cfg.midi_profile_path);
    let mut reloaded = App::new_with_host_io(
        Project::demo(),
        dir.path().join("reloaded.json"),
        Arc::clone(&io) as Arc<dyn HostIo>,
    )
    .unwrap();
    reloaded.apply_config(cfg_back);
    assert_eq!(
        reloaded.config.midi_profile_path.as_deref(),
        Some(custom_path.as_path())
    );
    assert!(
        reloaded.active_midi_profile().mappings.iter().any(|m| {
            matches!(
                m.source,
                MidiSource::Cc {
                    channel: Some(0),
                    cc: 64
                }
            )
        }),
        "apply_config must reload custom .midi.json mappings automatically"
    );

    // 7. `X` opens Mode::Confirm to clear all user mappings; confirming clears them; `x` deletes individual user mapping.
    keys(&mut app, "X");
    assert_eq!(app.mode_name(), "CONFIRM");
    keys(&mut app, "y");
    assert_eq!(app.mode_name(), "NORMAL");
    assert!(app.config.midi_mappings.is_empty());

    // Add a user mapping and delete it with `x`.
    app.config.midi_mappings.push(MidiMapping::new(
        777,
        MidiSource::Cc {
            channel: Some(0),
            cc: 91,
        },
        MidiValueMode::Absolute,
        MidiTarget::Param {
            address: ParamAddress::Channel {
                channel: ChannelRef::Master,
                param: ChannelParam::Gain,
            },
        },
    ));
    app.midi.cursor = 5;
    keys(&mut app, "x");
    assert!(
        app.config.midi_mappings.is_empty(),
        "`x` must delete the focused user mapping"
    );

    // 8. Built-in `standard_cc` routes Master Gain strictly through `ParamAddress::Channel { Master, Gain }`.
    let std_profile = cadence_core::midi_map::MidiProfile::standard_cc();
    let master_cc = std_profile
        .mappings
        .iter()
        .find(|m| {
            matches!(
                m.target,
                MidiTarget::Param {
                    address: ParamAddress::Channel {
                        channel: ChannelRef::Master,
                        param: ChannelParam::Gain,
                    }
                }
            )
        })
        .expect("standard_cc must bind Master Gain via ParamAddress::Channel { Master, Gain }");
    let MidiSource::Cc { cc, .. } = master_cc.source else {
        panic!("expected CC source for Master Gain");
    };
    app.apply_config(cadence_core::config::UserConfig {
        midi_profile: "standard_cc".into(),
        midi_pickup: false,
        ..cadence_core::config::UserConfig::default()
    });
    let before_master = app.project().mixer.master.strip.gain;
    app.handle_midi(MidiMessage::Control {
        channel: 0,
        controller: cc,
        value: 64,
    })
    .unwrap();
    let after_master = app.project().mixer.master.strip.gain;
    assert!(
        (after_master - before_master).abs() > 0.1 && after_master > 0.05 && after_master < before_master,
        "standard_cc Master Gain CC=64 must lower Master strip gain cleanly (before={before_master}, after={after_master})"
    );
}
