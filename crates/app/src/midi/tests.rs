use crate::*;
use cadence_core::material::Project;
use cadence_core::midi_map::{FeedbackProtocol, MidiFeedback, MidiTarget, TransportLedKind};
use cadence_core::runtime::MidiMessage;

#[test]
fn midi_learn_soft_pickup_and_unlearn_work_end_to_end() {
    let mut app = App::new_in_memory(Project::demo(), PathBuf::from("test.json")).unwrap();
    let inst_id = app.project().instruments[0].id;
    app.open_instrument(inst_id).unwrap();
    // Focus Cutoff or first parameter on the Instrument Rack deck.
    app.control_cursor = 0;
    let target_addr = match app.control_descriptors()[0].target {
        ControlTarget::Param(addr) => addr,
        _ => panic!("expected param control"),
    };

    // 1. Arm MIDI Learn with `M`, send CC 74 on ch 0 -> binds immediately!
    app.press_keys("M").unwrap();
    assert_eq!(
        app.midi.learn,
        Some(MidiTarget::Param {
            address: target_addr
        })
    );
    let learn_out = app
        .handle_midi(MidiMessage::Control {
            channel: 0,
            controller: 74,
            value: 64,
        })
        .unwrap();
    assert!(app.midi.learn.is_none());
    assert_eq!(
        app.midi_badge_for_param(target_addr).as_deref(),
        Some("CC 74")
    );
    assert!(learn_out
        .midi_feedback
        .iter()
        .any(|f| matches!(f, MidiFeedback::Display { .. })));

    // 2. Soft pickup prevents jumping when CC starts far away from current value.
    app.config.midi_pickup = true;
    let before = app.read_param_normalized(target_addr).unwrap();
    let far_cc: u8 = if before > 0.5 { 2 } else { 125 };
    let pickup_out = app
        .handle_midi(MidiMessage::Control {
            channel: 0,
            controller: 74,
            value: far_cc,
        })
        .unwrap();
    assert!((app.read_param_normalized(target_addr).unwrap() - before).abs() < 1e-4);
    assert!(pickup_out.midi_feedback.iter().any(|f| matches!(
        f,
        MidiFeedback::Display { line1, .. } if line1.contains("Pickup")
    )));

    // 3. Move CC onto current normalized value to latch pickup, then sweep!
    let latch_cc = (before * 127.0).round() as u8;
    app.handle_midi(MidiMessage::Control {
        channel: 0,
        controller: 74,
        value: latch_cc,
    })
    .unwrap();
    let sweep_cc: u8 = if before > 0.5 { 20 } else { 110 };
    let sweep_out = app
        .handle_midi(MidiMessage::Control {
            channel: 0,
            controller: 74,
            value: sweep_cc,
        })
        .unwrap();
    assert!(sweep_out.changed);
    assert!((app.read_param_normalized(target_addr).unwrap() - before).abs() > 0.05);

    // 4. Arm `M` then press `x` -> unlearns!
    app.press_keys("Mx").unwrap();
    assert!(app.config.midi_mappings.is_empty());
}

#[test]
fn minilab3_daw_profile_macros_transport_pads_and_mmc_emit_feedback() {
    let mut app = App::new_in_memory(Project::demo(), PathBuf::from("test.json")).unwrap();
    let out = app.execute_command("midi profile minilab3_daw").unwrap();
    assert_eq!(
        app.active_midi_feedback_protocol(),
        FeedbackProtocol::ArturiaDaw
    );
    assert!(out
        .midi_feedback
        .iter()
        .any(|f| matches!(f, MidiFeedback::InitDawMode(true))));
    assert!(out
        .midi_feedback
        .iter()
        .any(|f| matches!(f, MidiFeedback::PadRgb { .. })));

    // Disable pickup so CC 86 immediately sweeps Focused Macro 1.
    app.config.midi_pickup = false;
    let mac_addr = app.focused_macro_targets()[0].1;
    let out_knob = app
        .handle_midi(MidiMessage::Control {
            channel: 0,
            controller: 86,
            value: 100,
        })
        .unwrap();
    assert!(out_knob.changed);
    assert!((app.read_param_normalized(mac_addr).unwrap() - (100.0 / 127.0)).abs() < 0.05);
    assert!(out_knob.midi_feedback.iter().any(|f| matches!(
        f,
        MidiFeedback::Display {
            value_bar: Some(_),
            ..
        }
    )));

    // Universal MMC Play / Stop works cleanly!
    let play_out = app.handle_midi(MidiMessage::Mmc { command: 0x02 }).unwrap();
    assert!(app.transport.playing);
    assert!(play_out.midi_feedback.iter().any(|f| matches!(
        f,
        MidiFeedback::TransportLed {
            kind: TransportLedKind::Play,
            active: true
        }
    )));
    let stop_out = app.handle_midi(MidiMessage::Mmc { command: 0x01 }).unwrap();
    assert!(!app.transport.playing);
    assert!(stop_out.midi_feedback.iter().any(|f| matches!(
        f,
        MidiFeedback::TransportLed {
            kind: TransportLedKind::Stop,
            active: true
        }
    )));

    // Pad Mode: Sections -> Pad 2 (Note 37 ch 10) selects section 2!
    app.execute_command("midi pads sections").unwrap();
    let pad_out = app
        .handle_midi(MidiMessage::NoteOn {
            channel: 9,
            note: 37,
            velocity: 110,
        })
        .unwrap();
    assert!(pad_out.changed);
}

#[test]
fn live_midi_note_recording_step_entry_and_polyphonic_chord_stacking() {
    let mut app = App::new_in_memory(Project::demo(), PathBuf::from("test.json")).unwrap();
    // Find a literal Notes pattern and select it.
    let note_pat_id = app
        .project()
        .patterns
        .iter()
        .find(|p| matches!(p.kind, PatternKind::Notes { .. }))
        .unwrap()
        .id;
    app.select_pattern(note_pat_id).unwrap();
    app.cursor = 0;

    // Enable MIDI record via `R`.
    app.press_keys("R").unwrap();
    assert!(app.midi.record);

    // Stopped: step entry writes step 0 and advances cursor to 1!
    let out1 = app
        .handle_midi(MidiMessage::NoteOn {
            channel: 0,
            note: 62,
            velocity: 100,
        })
        .unwrap();
    assert!(out1.changed);
    assert_eq!(app.cursor, 1);

    // Playing: two simultaneous notes on the same heard step stack into a real chord!
    app.apply(Action::Play { playing: true }).unwrap();
    app.handle_midi(MidiMessage::NoteOn {
        channel: 0,
        note: 62,
        velocity: 100,
    })
    .unwrap();
    app.handle_midi(MidiMessage::NoteOn {
        channel: 0,
        note: 65,
        velocity: 96,
    })
    .unwrap();
    let step_idx = (app.song_step_heard() as usize) % 16;
    let PatternKind::Notes { steps } = &app.pattern(note_pat_id).unwrap().kind else {
        panic!("expected notes pattern");
    };
    assert!(steps[step_idx].active);
    assert_eq!(steps[step_idx].chord.len(), 1);
}

#[test]
fn midi_cc_channel_gain_uses_headroom_taper_live_and_level_scale_when_recording() {
    use cadence_core::dsp::{norm_to_channel_gain, MAX_CHANNEL_GAIN};
    use cadence_core::midi_map::{MidiMapping, MidiSource, MidiValueMode};
    use cadence_core::mixer::{ChannelParam, ChannelRef};

    let mut app = App::new_in_memory(Project::demo(), PathBuf::from("test.json")).unwrap();
    let ch_id = app.project().mixer.channels[0].id;
    let ch_ref = ChannelRef::Channel(ch_id);
    app.config.midi_pickup = false;
    app.config.midi_mappings.push(MidiMapping {
        id: 501,
        label: "Ch0 Fader".into(),
        source: MidiSource::Cc {
            channel: None,
            cc: 7,
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

    // 1. Live non-recording CC 127 -> norm 1.0 -> MAX_CHANNEL_GAIN (2.0 = +6.0 dB) with dB display.
    let out_max = app
        .handle_midi(MidiMessage::Control {
            channel: 0,
            controller: 7,
            value: 127,
        })
        .unwrap();
    assert!((app.channel(ch_id).unwrap().strip.gain - MAX_CHANNEL_GAIN).abs() < 1e-5);
    assert!(out_max.midi_feedback.iter().any(|f| matches!(
        f,
        MidiFeedback::Display { line2, .. } if line2 == "+6.0 dB"
    )));

    // 2. Live non-recording CC 95 -> norm 95/127 -> norm_to_channel_gain(95/127) (~1.0 / +0.0 dB).
    let expected_mid = norm_to_channel_gain(95.0 / 127.0);
    app.handle_midi(MidiMessage::Control {
        channel: 0,
        controller: 7,
        value: 95,
    })
    .unwrap();
    assert!((app.channel(ch_id).unwrap().strip.gain - expected_mid).abs() < 1e-5);

    // 3. Armed & playing live CC recording resets strip.gain = 1.0, records 0..1 scale, and displays %.
    app.apply(Action::Play { playing: true }).unwrap();
    app.press_keys("R").unwrap();
    let out_rec = app
        .handle_midi(MidiMessage::Control {
            channel: 0,
            controller: 7,
            value: 64,
        })
        .unwrap();
    assert!((app.channel(ch_id).unwrap().strip.gain - 1.0).abs() < 1e-6);
    assert!(out_rec.midi_feedback.iter().any(|f| matches!(
        f,
        MidiFeedback::Display { line2, .. } if line2 == "50%"
    )));
}
