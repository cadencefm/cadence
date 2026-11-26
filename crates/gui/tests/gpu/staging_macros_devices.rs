use crate::common::{build_snapshot, render_checked, with_gpu, DEFAULT_H, DEFAULT_W, STATUS};
use cadence_app::view::{DeckView, DevicesTab};
use cadence_app::{Action, App};
use cadence_core::material::{
    Boundary, Edit, LanePart, Part, Pattern, PatternKind, Project, ValuePoint, ValueUnit,
};
use cadence_core::midi_map::{MidiMapping, MidiPadMode, MidiSource, MidiTarget, MidiValueMode};
use cadence_core::mixer::{ChannelParam, ChannelRef, EffectKind};
use cadence_core::params::{ParamAddress, ValueMode};
use cadence_core::runtime::{
    AudioDeviceInfo, AudioErrorCode, AudioHealth, AudioInputMode, AudioInputStatus,
    AudioInputTelemetry, AudioInventory, AudioStreamStatus, CapturePhase, InputMonitorMode,
    MidiMessage,
};
use cadence_gui::theme::Theme;
use std::path::PathBuf;

fn setup_macro_showcase_app() -> App {
    let mut app = App::new_in_memory(Project::demo(), PathBuf::from("macros_gpu.json")).unwrap();
    app.config.midi_pickup = false;
    app.execute_command("midi profile minilab3_daw").unwrap();

    let delay_id = app
        .project()
        .mixer
        .channels
        .iter()
        .flat_map(|c| c.inserts.iter())
        .find(|d| matches!(d.kind, EffectKind::Delay(_)))
        .expect("Delay insert in demo project")
        .id;

    let curve_id = app.project().max_id() + 1;
    app.apply(Action::Edit {
        edit: Edit::PutPattern {
            pattern: Pattern {
                id: curve_id,
                name: "RiserCurve".into(),
                kind: PatternKind::Values {
                    unit: ValueUnit::Normalized,
                    period: 32,
                    mode: ValueMode::Ramp,
                    points: vec![
                        ValuePoint {
                            step: 0,
                            value: 0.15,
                        },
                        ValuePoint {
                            step: 16,
                            value: 0.88,
                        },
                        ValuePoint {
                            step: 28,
                            value: 0.35,
                        },
                    ],
                },
            },
        },
    })
    .unwrap();

    app.execute_command("macro 1 name Filter Wash").unwrap();
    app.execute_command("macro 1 bind instrument Saw cutoff 0.12 0.95")
        .unwrap();
    app.execute_command("macro 1 bind instrument Saw resonance 0.10 0.72")
        .unwrap();
    app.execute_command("macro 1 bind channel Lead pan 0.92 0.08")
        .unwrap();
    app.execute_command(&format!("macro 1 bind effect {delay_id} mix 0.08 0.82"))
        .unwrap();
    app.execute_command("macro 1 54%").unwrap();
    app.execute_command("mod WashAuto macro 1 RiserCurve")
        .unwrap();

    app.execute_command("macro 2 name Sub Punch").unwrap();
    app.execute_command("macro 2 bind instrument 'Low voice' cutoff 0.18 0.85")
        .unwrap();
    app.execute_command("macro 2 bind instrument 'Low voice' drive 0.05 0.80")
        .unwrap();
    app.execute_command("macro 2 bind channel Music gain 0.90 0.45")
        .unwrap();
    app.execute_command("macro 2 72%").unwrap();

    app.execute_command("macro 3 name Echo Throw").unwrap();
    app.execute_command(&format!(
        "macro 3 bind effect {delay_id} feedback 0.15 0.85"
    ))
    .unwrap();
    app.execute_command("macro 3 bind channel Echo gain 0.15 0.88")
        .unwrap();
    app.execute_command("macro 3 36%").unwrap();
    app.execute_command("mod EchoAuto macro 3 Energy").unwrap();

    app.execute_command("macro 4 name Kick Snap").unwrap();
    app.execute_command("macro 4 bind instrument Kick attack 0.40 0.02")
        .unwrap();
    app.execute_command("macro 4 bind instrument Kick decay 0.25 0.85")
        .unwrap();
    app.execute_command("macro 4 64%").unwrap();

    app.execute_command("macro 5 name Wide Space").unwrap();
    app.execute_command("macro 5 bind channel Lead pan 0.50 0.88")
        .unwrap();
    app.execute_command("macro 5 bind channel Bass pan 0.50 0.12")
        .unwrap();
    app.execute_command("macro 5 80%").unwrap();

    app.execute_command("macro 6 name Master Dim").unwrap();
    app.execute_command("macro 6 bind channel Master gain 0.88 0.35")
        .unwrap();
    app.execute_command("macro 6 22%").unwrap();

    app.handle_midi(MidiMessage::Control {
        channel: 0,
        controller: 86,
        value: 69,
    })
    .unwrap();
    app.handle_midi(MidiMessage::Control {
        channel: 0,
        controller: 87,
        value: 92,
    })
    .unwrap();
    app
}

#[test]
fn gpu_renders_live_staging_and_bank_32_slot_scroll() {
    with_gpu(DEFAULT_W, DEFAULT_H, |gpu| {
        let dark = Theme::cadence_dark();

        // 1. Live Staging: STAGED badge + Arrange & Bank previews + zoomed Bank Launchpad Matrix.
        let project = Project::demo();
        let snap = build_snapshot(&project, 12);
        let mut app = App::new_in_memory(project, PathBuf::from("live_staging_gpu.json")).unwrap();
        app.execute_command("lane Drums add Kick restart").unwrap();
        app.apply(Action::Play { playing: true }).unwrap();
        let drums_lane = app
            .project()
            .lanes
            .iter()
            .find(|l| l.name == "Drums")
            .expect("Drums lane");
        let (drums_id, kick_lp) = (drums_lane.id, drums_lane.parts[0]);
        app.apply(Action::Launch {
            lane: drums_id,
            part: kick_lp,
            at: Some(Boundary::Now),
        })
        .unwrap();
        app.apply(Action::Advance { steps: 12 }).unwrap();
        app.press_keys("gbgaS<m-q>2<m-e><m-x><m-w><m-m><m-q><m-s><m-2>")
            .unwrap();

        let fv_stage = app.frame_view(&snap, STATUS);
        assert_eq!(fv_stage.chrome.staged_count, Some(5));
        let px_stage = render_checked(gpu, "stage-01-arrange-bank.png", &fv_stage, &dark, 0.25);

        app.press_keys("gbzz").unwrap();
        let fv_bank = app.frame_view(&snap, STATUS);
        assert!(fv_bank
            .decks
            .iter()
            .any(|d| matches!(d.view, DeckView::Bank(ref b) if b.staging_count == Some(5))));
        let px_bank = render_checked(
            gpu,
            "stage-02-bank-matrix-zoomed.png",
            &fv_bank,
            &dark,
            0.20,
        );
        assert_ne!(px_bank, px_stage);

        // 2. Bank 32-slot horizontal scroll window (`◀` and `▶`, compact & zoomed).
        let mut bank_proj = Project::demo();
        let base_pat = bank_proj.patterns[0].id;
        let inst0_id = bank_proj.instruments[0].id;
        let next_start = bank_proj.max_id() + 1;
        for i in 0..22u64 {
            let pid = next_start + i;
            bank_proj.parts.push(Part {
                id: pid,
                name: format!("Acid {:02}", i + 1),
                pattern: base_pat,
                instrument: inst0_id,
                transpose: (i % 7) as i16,
                velocity_scale: 1.0,
                parameter_links: Vec::new(),
            });
            if bank_proj.lanes[0].parts.len() < 22 {
                bank_proj.lanes[0].parts.push(LanePart::new(pid));
            }
        }
        bank_proj.lanes[0].parts.truncate(22);
        bank_proj.validate().unwrap();

        let bank_snap = build_snapshot(&bank_proj, 12);
        let mut bank_app = App::new_in_memory(bank_proj, PathBuf::from("bank_32.json")).unwrap();
        bank_app.press_keys("gb").unwrap();
        for _ in 0..14 {
            bank_app.press_keys("l").unwrap();
        }
        let px_compact = render_checked(
            gpu,
            "stage-03-bank-32slot-compact.png",
            &bank_app.frame_view(&bank_snap, STATUS),
            &dark,
            0.20,
        );
        bank_app.press_keys("zz").unwrap();
        let px_zoomed = render_checked(
            gpu,
            "stage-04-bank-32slot-zoomed.png",
            &bank_app.frame_view(&bank_snap, STATUS),
            &dark,
            0.20,
        );
        assert_ne!(px_compact, px_zoomed);
    });
}

#[test]
fn gpu_renders_macros_deck_console_split_and_deep_scroll() {
    with_gpu(DEFAULT_W, DEFAULT_H, |gpu| {
        let dark = Theme::cadence_dark();
        let light = Theme::studio_light();
        let mut app = setup_macro_showcase_app();
        let snap = build_snapshot(app.project(), 14);

        // 1. Zoomed 8-slot Macros console (Dark & Light)
        app.press_keys("gk1zz").unwrap();
        let fv_macros = app.frame_view(&snap, STATUS);
        assert_eq!(fv_macros.chrome.active_macro_count, 6);
        assert_eq!(fv_macros.chrome.active_macro_automated, 2);
        let mview = fv_macros
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Macros(m) => Some(m),
                _ => None,
            })
            .expect("MacrosDeckView");
        assert_eq!(mview.slots.len(), 8);
        assert!(mview.slots[0].effective_value.is_some());
        assert!(mview.slots[0].bindings.iter().any(|b| b.inverted));

        let px_dark = render_checked(gpu, "macros-01-console-dark.png", &fv_macros, &dark, 0.25);
        let px_light = render_checked(gpu, "macros-02-console-light.png", &fv_macros, &light, 0.25);
        assert_ne!(px_dark, px_light);

        // 2. Split-pane 1D strip (`zz 2`)
        app.press_keys("zz2").unwrap();
        let px_split = render_checked(
            gpu,
            "macros-03-split-strip.png",
            &app.frame_view(&snap, STATUS),
            &dark,
            0.25,
        );
        assert_ne!(px_split, px_dark);

        // 3. Deep 12-binding vertical viewport scroll
        app.execute_command("macro 7 name Deep Matrix").unwrap();
        for cmd in [
            "macro 7 bind instrument Saw cutoff 0.10 0.90",
            "macro 7 bind instrument Saw resonance 0.05 0.70",
            "macro 7 bind instrument Saw drive 0.00 0.85",
            "macro 7 bind instrument Saw attack 0.05 0.60",
            "macro 7 bind instrument Saw decay 0.20 0.85",
            "macro 7 bind instrument Saw release 0.15 0.80",
            "macro 7 bind instrument 'Low voice' cutoff 0.15 0.92",
            "macro 7 bind instrument 'Low voice' resonance 0.10 0.65",
            "macro 7 bind channel Lead gain 0.85 0.45",
            "macro 7 bind channel Lead pan 0.20 0.80",
            "macro 7 bind channel Bass pan 0.80 0.20",
            "macro 7 bind channel Master gain 0.95 0.55",
        ] {
            app.execute_command(cmd).unwrap();
        }
        app.execute_command("macro 7 62%").unwrap();
        app.press_keys("gk7").unwrap();
        app.view_state.macro_binding_cursor = Some(9);
        let px_scroll = render_checked(
            gpu,
            "macros-04-deep-bindings-scroll.png",
            &app.frame_view(&snap, STATUS),
            &dark,
            0.25,
        );
        assert_ne!(px_scroll, px_split);
    });
}

#[test]
fn gpu_renders_devices_midi_and_audio_health_states() {
    with_gpu(DEFAULT_W, DEFAULT_H, |gpu| {
        let dark = Theme::cadence_dark();
        let light = Theme::studio_light();

        let mut app =
            App::new_in_memory(Project::demo(), PathBuf::from("devices_gpu.json")).unwrap();
        app.apply(Action::Play { playing: true }).unwrap();
        app.midi.in_ports = vec!["Arturia MiniLab 3 MIDI".into(), "Launchpad X".into()];
        app.midi.out_ports = vec!["Arturia MiniLab 3 DAW".into()];
        app.midi.port = Some("Arturia MiniLab 3 MIDI".into());
        app.midi.out_port = Some("Arturia MiniLab 3 DAW".into());
        app.config.midi_in_port = Some("Arturia MiniLab 3 MIDI".into());
        app.config.midi_out_port = Some("Arturia MiniLab 3 DAW".into());
        app.config.midi_pad_mode = MidiPadMode::Sections;
        app.config.midi_pickup = false;
        app.execute_command("midi profile minilab3_daw").unwrap();
        app.config.midi_mappings.push(MidiMapping::new(
            501,
            MidiSource::Cc {
                channel: Some(0),
                cc: 74,
            },
            MidiValueMode::Absolute,
            MidiTarget::Param {
                address: ParamAddress::Channel {
                    channel: ChannelRef::Master,
                    param: ChannelParam::Gain,
                },
            },
        ));
        app.midi.record = true;
        app.set_midi_out_error(Some("MiniLab 3 DAW output port disconnected".into()));
        app.set_audio_inventory(AudioInventory {
            outputs: vec![
                AudioDeviceInfo {
                    name: "MacBook Pro Speakers".into(),
                    is_default: true,
                    default_sample_rate: 48_000,
                    supported_sample_rates: vec![44_100, 48_000, 96_000],
                    min_buffer_frames: Some(128),
                    max_buffer_frames: Some(1024),
                    channels: 2,
                },
                AudioDeviceInfo {
                    name: "Focusrite Scarlett 4i4".into(),
                    is_default: false,
                    default_sample_rate: 48_000,
                    supported_sample_rates: vec![44_100, 48_000, 88_200, 96_000],
                    min_buffer_frames: Some(64),
                    max_buffer_frames: Some(2048),
                    channels: 4,
                },
            ],
            inputs: vec![AudioDeviceInfo {
                name: "Focusrite Scarlett 4i4 In".into(),
                is_default: true,
                default_sample_rate: 48_000,
                supported_sample_rates: vec![48_000, 96_000],
                min_buffer_frames: Some(64),
                max_buffer_frames: Some(2048),
                channels: 4,
            }],
            active_output: Some("MacBook Pro Speakers".into()),
            active_output_is_default: true,
            active_sample_rate: 48_000,
            active_buffer_frames: 512,
            active_channels: 2,
            active_format_label: "32-bit float".into(),
            active_input: Some("Focusrite Scarlett 4i4 In".into()),
            active_input_sample_rate: 48_000,
            active_input_channels: 4,
            active_input_buffer_frames: 256,
            silent_mode: false,
            last_error: None,
            input_last_error: None,
        });
        app.handle_midi(MidiMessage::Control {
            channel: 0,
            controller: 74,
            value: 96,
        })
        .unwrap();
        app.handle_midi(MidiMessage::NoteOn {
            channel: 0,
            note: 60,
            velocity: 110,
        })
        .unwrap();

        let snap = build_snapshot(app.project(), 16);

        // 1. Devices · MIDI (`gd zz`): Device & Profile + REC·NOTE+CC + last recorded CC (Dark & Light)
        app.press_keys("gdzz").unwrap();
        let fv_midi = app.frame_view(&snap, STATUS);
        assert!(fv_midi.chrome.midi_recording);
        assert!(fv_midi.chrome.midi_cc_last_recorded.is_some());
        let midi_view = fv_midi
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Midi(m) => Some(m),
                _ => None,
            })
            .expect("MidiDeckView");
        assert_eq!(midi_view.device_rows.len(), 5);
        assert!(midi_view.device_rows[0].selected);

        let px_midi_dark = render_checked(gpu, "devices-01-midi-dark.png", &fv_midi, &dark, 0.20);
        let px_midi_light =
            render_checked(gpu, "devices-02-midi-light.png", &fv_midi, &light, 0.20);
        assert_ne!(px_midi_dark, px_midi_light);

        // 2. MIDI Mappings + Learn armed (`w <Enter>`)
        app.press_keys("w<Enter>").unwrap();
        let px_learn = render_checked(
            gpu,
            "devices-03-midi-learn-dark.png",
            &app.frame_view(&snap, STATUS),
            &dark,
            0.20,
        );
        assert_ne!(px_learn, px_midi_dark);
        app.press_keys("<Esc>zz").unwrap();

        // 3. Devices · Audio (Healthy Dark & Light with active `● REC` & input monitoring)
        app.config.audio_input_device = Some("Focusrite Scarlett 4i4 In".into());
        app.config.audio_input_mode = AudioInputMode::Stereo;
        app.config.audio_input_monitor = InputMonitorMode::Auto;
        app.config.audio_input_latency_ms = 2.5;
        app.execute_command("audio").unwrap();
        app.press_keys("zz").unwrap();
        let mut healthy_snap = build_snapshot(app.project(), 24);
        healthy_snap.sample_rate = 48_000;
        healthy_snap.health = AudioHealth {
            status: AudioStreamStatus::Running,
            cpu_load: 0.28,
            peak_cpu_load: 0.44,
            xrun_count: 0,
            buffer_frames: 512,
            channels: 2,
            bit_depth: 32,
            sample_format_is_float: true,
            overloaded: false,
            failed: false,
            error_code: AudioErrorCode::None,
            ..AudioHealth::default()
        };
        healthy_snap.input = AudioInputTelemetry {
            status: AudioInputStatus::Running,
            sample_rate: 48_000,
            channels: 4,
            buffer_frames: 256,
            peak: [0.62, 0.48],
            peak_hold: [0.78, 0.64],
            clipped: false,
            digital_silence: false,
            mode: AudioInputMode::Stereo,
            monitor_mode: InputMonitorMode::Auto,
            monitoring: true,
            capture_phase: CapturePhase::Recording,
            captured_frames: 48_000 * 4,
            dropped_frames: 0,
            capture_start_step: 16,
            capture_end_step: 0,
        };
        app.observe_snapshot(&healthy_snap);
        let fv_audio_ok = app.frame_view(&healthy_snap, "CoreAudio · 48 kHz");
        let audio_dv = fv_audio_ok
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Midi(m) => Some(m),
                _ => None,
            })
            .expect("Devices Audio view");
        assert_eq!(audio_dv.active_tab, DevicesTab::Audio);
        assert_eq!(audio_dv.audio.rows.len(), 11);
        assert_eq!(fv_audio_ok.chrome.recording_phase, CapturePhase::Recording);
        assert!(fv_audio_ok.chrome.input_monitoring);

        let px_audio_dark = render_checked(
            gpu,
            "devices-04-audio-healthy-dark.png",
            &fv_audio_ok,
            &dark,
            0.20,
        );
        let px_audio_light = render_checked(
            gpu,
            "devices-05-audio-healthy-light.png",
            &fv_audio_ok,
            &light,
            0.20,
        );
        assert_ne!(px_audio_dark, px_audio_light);

        // 4. OVERLOAD PAUSED alert + input CLIP + `○ ARM`
        let mut overload_snap = healthy_snap.clone();
        overload_snap.playing = false;
        overload_snap.health = AudioHealth {
            status: AudioStreamStatus::Overloaded,
            cpu_load: 0.96,
            peak_cpu_load: 1.08,
            xrun_count: 5,
            buffer_frames: 128,
            channels: 2,
            bit_depth: 32,
            sample_format_is_float: true,
            overloaded: true,
            failed: false,
            error_code: AudioErrorCode::OverloadCircuitBreaker,
            ..AudioHealth::default()
        };
        overload_snap.input.capture_phase = CapturePhase::Armed;
        overload_snap.input.clipped = true;
        overload_snap.input.peak = [0.98, 0.91];
        overload_snap.input.peak_hold = [1.0, 0.96];
        app.observe_snapshot(&overload_snap);
        app.press_keys("ww").unwrap();
        let fv_overload = app.frame_view(&overload_snap, "CoreAudio · 48 kHz");
        assert!(fv_overload.chrome.audio_health.overloaded);
        assert!(fv_overload.chrome.status_alert);
        let px_overload = render_checked(
            gpu,
            "devices-06-audio-overload-dark.png",
            &fv_overload,
            &dark,
            0.20,
        );
        assert_ne!(px_overload, px_audio_dark);

        // 5. STREAM ERROR alert (`!AUDIO ERR` + OS error box)
        let mut err_inv = app.devices.inventory.clone();
        err_inv.last_error = Some(
            "CoreAudio output device 'Focusrite Scarlett 4i4' disconnected during playback".into(),
        );
        app.set_audio_inventory(err_inv);
        let mut failed_snap = healthy_snap;
        failed_snap.playing = false;
        failed_snap.health = AudioHealth {
            status: AudioStreamStatus::Failed,
            cpu_load: 0.0,
            peak_cpu_load: 0.44,
            xrun_count: 2,
            buffer_frames: 512,
            channels: 2,
            bit_depth: 32,
            sample_format_is_float: true,
            overloaded: false,
            failed: true,
            error_code: AudioErrorCode::DeviceUnavailable,
            ..AudioHealth::default()
        };
        app.observe_snapshot(&failed_snap);
        app.press_keys("j").unwrap();
        let fv_failed = app.frame_view(&failed_snap, "CoreAudio · error");
        assert!(fv_failed.chrome.audio_health.failed);
        let px_failed = render_checked(
            gpu,
            "devices-07-audio-error-dark.png",
            &fv_failed,
            &dark,
            0.20,
        );
        assert_ne!(px_failed, px_overload);

        // 6. Active input delivering pure 0.0 digital silence (OS mic permission hint)
        let mut sil_inv = app.devices.inventory.clone();
        sil_inv.last_error = None;
        app.set_audio_inventory(sil_inv);
        let mut sil_snap = build_snapshot(app.project(), 24);
        sil_snap.sample_rate = 48_000;
        sil_snap.health = AudioHealth {
            status: AudioStreamStatus::Running,
            cpu_load: 0.12,
            peak_cpu_load: 0.22,
            buffer_frames: 512,
            channels: 2,
            bit_depth: 32,
            sample_format_is_float: true,
            ..AudioHealth::default()
        };
        sil_snap.input = AudioInputTelemetry {
            status: AudioInputStatus::Running,
            sample_rate: 48_000,
            channels: 1,
            buffer_frames: 512,
            peak: [0.0, 0.0],
            peak_hold: [0.0, 0.0],
            clipped: false,
            digital_silence: true,
            mode: AudioInputMode::MonoLeft,
            monitor_mode: InputMonitorMode::On,
            monitoring: true,
            capture_phase: CapturePhase::Idle,
            ..AudioInputTelemetry::default()
        };
        app.observe_snapshot(&sil_snap);
        render_checked(
            gpu,
            "devices-08-audio-digital-silence-dark.png",
            &app.frame_view(&sil_snap, "CoreAudio · 48 kHz"),
            &dark,
            0.20,
        );
    });
}
