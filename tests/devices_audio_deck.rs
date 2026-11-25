//! Integration tests for the unified tabbed Devices Deck (`gd` / `Ctrl+d` — `Audio | MIDI`),
//! audio interface rows, fuzzy pickers, persistence, recovery actions, global key invariants,
//! rising-edge `AudioHealth` alerts, and `midi_out_error` view projection.
use cadence_app::view::{DeckView, FrameView, OverlayView};
use cadence_app::{
    Action, App, AudioDeckRow, DevicesTab, HostIo, KeyCode, KeyEvent, KeyModifiers, MemoryHostIo,
    Outcome,
};
use cadence_core::config::UserConfig;
use cadence_core::material::Project;
use cadence_core::runtime::{
    AudioDeviceInfo, AudioErrorCode, AudioHealth, AudioInputMode, AudioInventory, AudioSnapshot,
    AudioStreamStatus, InputMonitorMode, TransportIntent, STANDARD_BUFFER_SIZES,
    STANDARD_SAMPLE_RATES,
};
use std::sync::Arc;

fn sample_inventory() -> AudioInventory {
    AudioInventory {
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
                supported_sample_rates: vec![44_100, 48_000, 88_200, 96_000, 192_000],
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
        active_input: None,
        active_input_sample_rate: 0,
        active_input_channels: 0,
        active_input_buffer_frames: 0,
        silent_mode: false,
        last_error: None,
        input_last_error: None,
    }
}

fn memory_app() -> (App, Arc<MemoryHostIo>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let io = Arc::new(MemoryHostIo::new(dir.path()));
    let mut app = App::new_with_host_io(
        Project::demo(),
        dir.path().join("devices.json"),
        Arc::clone(&io) as Arc<dyn HostIo>,
    )
    .unwrap();
    app.apply_config(UserConfig::default());
    app.set_audio_inventory(sample_inventory());
    (app, io, dir)
}

fn devices_view(app: &App, snap: &AudioSnapshot) -> cadence_app::view::MidiDeckView {
    let fv: FrameView = app.frame_view(snap, "CoreAudio · 48 kHz");
    fv.decks
        .into_iter()
        .find_map(|d| match d.view {
            DeckView::Midi(m) => Some(m),
            _ => None,
        })
        .expect("expected Devices (Midi) deck view")
}

#[test]
fn devices_tab_switching_gd_ctrl_d_ex_command_and_palette() {
    let (mut app, _io, _dir) = memory_app();

    // Default tab is MIDI on fresh launch.
    assert_eq!(app.devices.tab, DevicesTab::Midi);

    // `gd` opens Devices; `H` and `L` toggle Audio <-> MIDI.
    app.press_keys("gd").unwrap();
    assert!(app.midi_focused());
    assert_eq!(app.devices.tab, DevicesTab::Midi);

    app.press_keys("H").unwrap();
    assert_eq!(app.devices.tab, DevicesTab::Audio);
    app.press_keys("L").unwrap();
    assert_eq!(app.devices.tab, DevicesTab::Midi);
    app.press_keys("L").unwrap();
    assert_eq!(app.devices.tab, DevicesTab::Audio);

    // Leaving Devices (`ga`) and pressing `Ctrl+d` or `gd` remembers Audio tab!
    app.press_keys("ga").unwrap();
    assert!(!app.midi_focused());
    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL))
        .unwrap();
    assert!(app.midi_focused());
    assert_eq!(
        app.devices.tab,
        DevicesTab::Audio,
        "Ctrl+d must remember the active DevicesTab"
    );

    app.press_keys("gagd").unwrap();
    assert!(app.midi_focused());
    assert_eq!(
        app.devices.tab,
        DevicesTab::Audio,
        "gd must match Ctrl+d and remember the active DevicesTab"
    );

    // `:midi` jumps directly to Devices · MIDI; `:audio` jumps directly to Devices · Audio.
    app.press_keys("ga").unwrap();
    app.execute_command("midi").unwrap();
    assert!(app.midi_focused());
    assert_eq!(app.devices.tab, DevicesTab::Midi);

    app.press_keys("ga").unwrap();
    app.execute_command("audio").unwrap();
    assert!(app.midi_focused());
    assert_eq!(app.devices.tab, DevicesTab::Audio);

    // Action Palette "Devices · Audio" jumps directly to Devices · Audio from MIDI tab.
    app.execute_command("midi").unwrap();
    assert_eq!(app.devices.tab, DevicesTab::Midi);
    app.press_keys(":Devices Audio<Enter>").unwrap();
    assert!(app.midi_focused());
    assert_eq!(app.devices.tab, DevicesTab::Audio);

    // Tab-aware hints reflect Audio vs MIDI.
    let audio_hints = app.hints();
    assert!(audio_hints.iter().any(|(k, v)| k == "H/L" && v == "tab"));
    assert!(audio_hints.iter().any(|(k, v)| k == "r" && v == "rescan"));
    app.press_keys("L").unwrap();
    let midi_hints = app.hints();
    assert!(midi_hints.iter().any(|(k, v)| k == "H/L" && v == "tab"));
    assert!(midi_hints.iter().any(|(k, v)| k == "a" && v == "add map"));
}

#[test]
fn audio_tab_navigation_cycling_pickers_reset_recovery_and_persistence() {
    let (mut app, io, _dir) = memory_app();
    app.execute_command("audio").unwrap();
    assert_eq!(app.devices.tab, DevicesTab::Audio);
    assert_eq!(app.devices.cursor, 0);

    // Verify all 11 typed rows across 3 groups project cleanly into AudioDeckView.
    let snap = AudioSnapshot {
        sample_rate: 48_000,
        health: AudioHealth {
            status: AudioStreamStatus::Running,
            cpu_load: 0.24,
            peak_cpu_load: 0.38,
            buffer_frames: 512,
            channels: 2,
            bit_depth: 32,
            sample_format_is_float: true,
            ..AudioHealth::default()
        },
        ..AudioSnapshot::default()
    };
    let dv = devices_view(&app, &snap);
    assert_eq!(dv.active_tab, DevicesTab::Audio);
    assert_eq!(dv.audio.rows.len(), AudioDeckRow::ALL.len());
    assert_eq!(dv.audio.rows[0].label, "Output");
    assert!(dv.audio.rows[0].detail.contains("MacBook Pro Speakers"));
    assert!(dv.audio.rows[0].detail.contains("2 ch"));
    assert_eq!(dv.audio.rows[1].label, "Sample Rate");
    assert_eq!(dv.audio.rows[2].label, "Buffer Size");
    assert!(
        dv.audio.rows[2].value.contains("512 fr · 10.7 ms"),
        "expected exact ms latency on Buffer Size row, got {:?}",
        dv.audio.rows[2].value
    );
    assert_eq!(dv.audio.rows[3].label, "Cue Out");
    assert!(dv.audio.rows[3].detail.contains("Reserved"));
    assert_eq!(dv.audio.rows[4].label, "Input");
    assert_eq!(dv.audio.rows[5].label, "Input Mode");
    assert_eq!(dv.audio.rows[6].label, "Input Monitor");
    assert_eq!(dv.audio.rows[7].label, "Latency Trim");
    assert_eq!(dv.audio.rows[8].label, "Audio Panic");

    // Navigation: j/k, w/b 3-group jump, 0/$/G/End/gg/ge/PgUp/PgDn.
    app.press_keys("jj").unwrap();
    assert_eq!(app.devices.cursor, 2); // Buffer Size
    app.press_keys("k").unwrap();
    assert_eq!(app.devices.cursor, 1); // Sample Rate
    app.press_keys("w").unwrap();
    assert_eq!(app.devices.cursor, 4); // Input (AUDIO INPUT & RECORDING start)
    app.press_keys("w").unwrap();
    assert_eq!(app.devices.cursor, 8); // Audio Panic (ENGINE & RECOVERY start)
    app.press_keys("b").unwrap();
    assert_eq!(app.devices.cursor, 4); // Input
    app.press_keys("b").unwrap();
    assert_eq!(app.devices.cursor, 0); // Output (AUDIO INTERFACE start)
    app.press_keys("G").unwrap();
    assert_eq!(app.devices.cursor, 10); // Rescan Devices
    app.press_keys("0").unwrap();
    assert_eq!(app.devices.cursor, 0);
    app.press_keys("ge").unwrap();
    assert_eq!(app.devices.cursor, 10);
    app.press_keys("gg").unwrap();
    assert_eq!(app.devices.cursor, 0);
    app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.devices.cursor, 6);
    app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.devices.cursor, 0);

    // Row 0 (Output): Alt+l cycles to MacBook Pro Speakers, emitting audio_reconfigure & persisting config!
    let out = app
        .handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::ALT))
        .unwrap();
    assert_eq!(
        app.config.audio_output_device.as_deref(),
        Some("MacBook Pro Speakers")
    );
    let req = out
        .audio_reconfigure
        .expect("Output change must emit audio_reconfigure");
    assert_eq!(req.output_device.as_deref(), Some("MacBook Pro Speakers"));
    assert_eq!(
        io.saved_config().unwrap().audio_output_device.as_deref(),
        Some("MacBook Pro Speakers")
    );

    // `.` cycles forward to Focusrite Scarlett 4i4; `Enter` opens fuzzy picker; Alt+Backspace resets to System Default.
    let out = app
        .handle_key(KeyEvent::new(KeyCode::Char('.'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(
        app.config.audio_output_device.as_deref(),
        Some("Focusrite Scarlett 4i4")
    );
    assert!(out.audio_reconfigure.is_some());

    let out = app
        .handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT))
        .unwrap();
    assert_eq!(app.config.audio_output_device, None);
    assert_eq!(out.audio_reconfigure.unwrap().output_device, None);

    // Output Enter picker chooses Scarlett via `/` filter.
    app.press_keys("<Enter>").unwrap();
    assert_eq!(app.mode_name(), "PICK");
    app.press_keys("/Scarlett<Enter><Enter>").unwrap();
    assert_eq!(app.mode_name(), "NORMAL");
    assert_eq!(
        app.config.audio_output_device.as_deref(),
        Some("Focusrite Scarlett 4i4")
    );

    // Row 1 (Sample Rate): respects Scarlett's supported_sample_rates, emits audio_reconfigure.
    app.press_keys("j").unwrap();
    assert_eq!(app.devices.cursor, 1);
    let out = app
        .handle_key(KeyEvent::new(KeyCode::Char('.'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.config.audio_sample_rate, Some(44_100));
    assert_eq!(out.audio_reconfigure.unwrap().sample_rate, Some(44_100));
    app.press_keys("<Enter>").unwrap();
    let rate_items = match app.frame_view(&snap, "test").overlay {
        Some(OverlayView::Picker(pv)) => pv.items,
        other => panic!("expected Sample Rate picker, got {other:?}"),
    };
    assert!(rate_items
        .iter()
        .any(|(name, _, _)| name == "Auto (Device)"));
    assert!(rate_items.iter().any(|(name, _, _)| name == "192 kHz"));
    app.press_keys("<Esc>").unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT))
        .unwrap();
    assert_eq!(app.config.audio_sample_rate, None);

    // Row 2 (Buffer Size): filtered to device min/max when on MacBook Speakers (128..=1024).
    app.press_keys("0").unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT))
        .unwrap(); // Back to System Default (MacBook Speakers: min 128, max 1024)
    app.press_keys("2j").unwrap();
    assert_eq!(app.devices.cursor, 2);
    app.press_keys("<Enter>").unwrap();
    let buf_items = match app.frame_view(&snap, "test").overlay {
        Some(OverlayView::Picker(pv)) => pv.items,
        other => panic!("expected Buffer Size picker, got {other:?}"),
    };
    assert!(buf_items.iter().any(|(name, _, _)| name.contains("Auto")));
    assert!(!buf_items
        .iter()
        .any(|(name, _, _)| name.starts_with("64 fr")));
    assert!(buf_items
        .iter()
        .any(|(name, _, _)| name.starts_with("128 fr")));
    assert!(buf_items
        .iter()
        .any(|(name, _, _)| name.starts_with("512 fr · 10.7 ms")));
    assert!(!buf_items
        .iter()
        .any(|(name, _, _)| name.starts_with("2048 fr")));
    // Select 512 fr (Auto=0, 128=1, 256=2, 512=3 -> 4th item = key '4').
    let out = app
        .handle_key(KeyEvent::new(KeyCode::Char('4'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.config.audio_buffer_size, Some(512));
    assert_eq!(out.audio_reconfigure.unwrap().buffer_frames, Some(512));
    assert_eq!(io.saved_config().unwrap().audio_buffer_size, Some(512));

    // Row 3 (Cue Out): Enter picker selects MacBook Pro Speakers, persists config.
    app.press_keys("j<Enter>j<Enter>").unwrap();
    assert_eq!(app.devices.cursor, 3);
    assert_eq!(
        app.config.audio_cue_device.as_deref(),
        Some("MacBook Pro Speakers")
    );
    assert_eq!(
        io.saved_config().unwrap().audio_cue_device.as_deref(),
        Some("MacBook Pro Speakers")
    );
    app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT))
        .unwrap();
    assert_eq!(app.config.audio_cue_device, None);

    // Row 4 (Input): `.` selects input, persists config, emits ONLY `audio_input_reconfigure` (zero output interruption).
    app.press_keys("j").unwrap();
    assert_eq!(app.devices.cursor, 4);
    let out = app
        .handle_key(KeyEvent::new(KeyCode::Char('.'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(
        app.config.audio_input_device.as_deref(),
        Some("Focusrite Scarlett 4i4 In")
    );
    assert!(out.audio_reconfigure.is_none());
    let in_req = out
        .audio_input_reconfigure
        .expect("Input change must emit audio_input_reconfigure");
    assert_eq!(
        in_req.input_device.as_deref(),
        Some("Focusrite Scarlett 4i4 In")
    );
    assert_eq!(
        io.saved_config().unwrap().audio_input_device.as_deref(),
        Some("Focusrite Scarlett 4i4 In")
    );

    // Row 5 (Input Mode): `.` cycles Stereo -> Mono (Ch 1), emits audio_input_reconfigure without touching output.
    app.press_keys("j").unwrap();
    assert_eq!(app.devices.cursor, 5);
    let out = app
        .handle_key(KeyEvent::new(KeyCode::Char('.'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.config.audio_input_mode, AudioInputMode::MonoLeft);
    assert!(out.audio_reconfigure.is_none());
    assert_eq!(
        out.audio_input_reconfigure.unwrap().mode,
        AudioInputMode::MonoLeft
    );
    app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT))
        .unwrap();
    assert_eq!(app.config.audio_input_mode, AudioInputMode::Stereo);

    // Row 6 (Input Monitor): `.` cycles Off -> Auto (Armed) -> On, emits audio_input_reconfigure.
    app.press_keys("j").unwrap();
    assert_eq!(app.devices.cursor, 6);
    let out = app
        .handle_key(KeyEvent::new(KeyCode::Char('.'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.config.audio_input_monitor, InputMonitorMode::Auto);
    assert!(out.audio_reconfigure.is_none());
    assert_eq!(
        out.audio_input_reconfigure.unwrap().monitor,
        InputMonitorMode::Auto
    );
    app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT))
        .unwrap();
    assert_eq!(app.config.audio_input_monitor, InputMonitorMode::Off);

    // Row 7 (Latency Trim): `.` nudges by +1.0 ms, `Enter` opens value editor, Alt+Backspace resets to 0.0.
    app.press_keys("j").unwrap();
    assert_eq!(app.devices.cursor, 7);
    let out = app
        .handle_key(KeyEvent::new(KeyCode::Char('.'), KeyModifiers::NONE))
        .unwrap();
    assert!((app.config.audio_input_latency_ms - 1.0).abs() < 1e-4);
    assert!(out.audio_reconfigure.is_none());
    assert!(out.audio_input_reconfigure.is_none());
    app.press_keys("<Enter>12.5<Enter>").unwrap();
    assert!((app.config.audio_input_latency_ms - 12.5).abs() < 1e-4);
    assert!((io.saved_config().unwrap().audio_input_latency_ms - 12.5).abs() < 1e-4);
    app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT))
        .unwrap();
    assert!(app.config.audio_input_latency_ms.abs() < 1e-4);

    // Reset Input back to None cleanly.
    app.press_keys("3k").unwrap();
    assert_eq!(app.devices.cursor, 4);
    let reset_in = app
        .handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT))
        .unwrap();
    assert_eq!(app.config.audio_input_device, None);
    assert_eq!(reset_in.audio_input_reconfigure.unwrap().input_device, None);

    // Row 8 (Audio Panic): Enter sends TransportIntent::Reset at current step.
    app.observe_transport(32, 33, true, false);
    app.press_keys("w").unwrap();
    assert_eq!(app.devices.cursor, 8);
    let panic_out = app
        .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    assert!(panic_out
        .transport
        .contains(&TransportIntent::Reset { step: 32 }));
    assert!(panic_out.stop_audition);
    assert!(!app.playing());

    // Row 9 (Restart Audio): Enter emits audio_restart.
    app.press_keys("j").unwrap();
    assert_eq!(app.devices.cursor, 9);
    let restart_out = app
        .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    assert!(restart_out.audio_restart);

    // Row 10 (Rescan Devices): Enter emits audio_rescan + midi_rescan.
    app.press_keys("j").unwrap();
    assert_eq!(app.devices.cursor, 10);
    let rescan_out = app
        .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    assert!(rescan_out.audio_rescan && rescan_out.midi_rescan);

    // `r` anywhere on Audio tab emits audio_rescan + midi_rescan.
    app.press_keys("0").unwrap();
    let r_out = app
        .handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE))
        .unwrap();
    assert!(r_out.audio_rescan && r_out.midi_rescan);

    // `Ctrl+s` saves config.json cleanly.
    app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .unwrap();
    assert!(!app.settings_ui.config_dirty);

    // Empty inventory gracefully falls back to STANDARD_SAMPLE_RATES & STANDARD_BUFFER_SIZES.
    app.set_audio_inventory(AudioInventory::default());
    app.press_keys("0j<Enter>").unwrap();
    let fallback_rates = match app.frame_view(&AudioSnapshot::default(), "test").overlay {
        Some(OverlayView::Picker(pv)) => pv.items,
        other => panic!("expected fallback Sample Rate picker, got {other:?}"),
    };
    assert_eq!(fallback_rates.len(), 1 + STANDARD_SAMPLE_RATES.len());
    app.press_keys("<Esc>j<Enter>").unwrap();
    let fallback_bufs = match app.frame_view(&AudioSnapshot::default(), "test").overlay {
        Some(OverlayView::Picker(pv)) => pv.items,
        other => panic!("expected fallback Buffer Size picker, got {other:?}"),
    };
    assert_eq!(fallback_bufs.len(), 1 + STANDARD_BUFFER_SIZES.len());
    app.press_keys("<Esc>").unwrap();
}

#[test]
fn audio_tab_bare_hl_noop_and_global_keys_inviolable() {
    let (mut app, _io, _dir) = memory_app();
    app.execute_command("audio").unwrap();

    // Bare `h`/`l` and Left/Right are strictly no-op on Audio tab.
    let cfg_before = app.config.clone();
    app.press_keys("hl<Left><Right>").unwrap();
    assert_eq!(app.devices.cursor, 0);
    assert_eq!(app.config, cfg_before);

    // Global `t` opens Transport prefix.
    app.press_keys("t").unwrap();
    assert_eq!(app.mode_name(), "TRANSPORT");
    app.press_keys("<Esc>").unwrap();

    // Global `i` is not hijacked by Audio tab.
    app.press_keys("i").unwrap();
    assert_eq!(app.devices.tab, DevicesTab::Audio);
    assert_eq!(app.config, cfg_before);

    // Global `S` starts staging.
    app.press_keys("S").unwrap();
    assert!(app.staging().is_some());
    app.press_keys("<Esc>").unwrap();
    assert!(app.staging().is_none());

    // Global `R` toggles MIDI recording.
    assert!(!app.midi.record);
    app.press_keys("R").unwrap();
    assert!(app.midi.record);
    app.press_keys("R").unwrap();
    assert!(!app.midi.record);

    // Global `Backspace` unzooms / navigates back without mutating audio config.
    app.press_keys("zz").unwrap();
    assert!(app.view_state.zoomed);
    app.press_keys("<BS>").unwrap();
    assert!(!app.view_state.zoomed);
    assert!(app.midi_focused());
    assert_eq!(app.config, cfg_before);

    // Global `Ctrl+<deck>` and `Ctrl+Shift+<modal>` work from Audio tab.
    app.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::CONTROL))
        .unwrap();
    assert!(app.mixer_canvas_focused());
    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL))
        .unwrap();
    assert!(app.midi_focused());
    assert_eq!(app.devices.tab, DevicesTab::Audio);
    app.handle_key(KeyEvent::new(
        KeyCode::Char('P'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    ))
    .unwrap();
    assert_eq!(app.mode_name(), "PALETTE");
    app.press_keys("<Esc>").unwrap();
}

#[test]
fn rising_edge_audio_health_alerts_midi_out_error_and_outcome_merge() {
    let (mut app, _io, _dir) = memory_app();
    app.status = "Steady".into();

    // 1. Healthy snapshot leaves status untouched.
    let healthy = AudioSnapshot {
        sample_rate: 48_000,
        health: AudioHealth {
            status: AudioStreamStatus::Running,
            cpu_load: 0.22,
            peak_cpu_load: 0.30,
            buffer_frames: 512,
            channels: 2,
            bit_depth: 32,
            sample_format_is_float: true,
            ..AudioHealth::default()
        },
        ..AudioSnapshot::default()
    };
    app.observe_snapshot(&healthy);
    assert_eq!(app.status, "Steady");

    // 2. Rising edge `overloaded: false -> true` sets actionable status alert.
    let overloaded = AudioSnapshot {
        sample_rate: 48_000,
        health: AudioHealth {
            status: AudioStreamStatus::Overloaded,
            cpu_load: 1.04,
            peak_cpu_load: 1.18,
            xrun_count: 3,
            buffer_frames: 256,
            channels: 2,
            bit_depth: 32,
            sample_format_is_float: true,
            overloaded: true,
            failed: false,
            error_code: AudioErrorCode::OverloadCircuitBreaker,
            ..AudioHealth::default()
        },
        ..AudioSnapshot::default()
    };
    app.observe_snapshot(&overloaded);
    assert!(app.status.contains("Audio overload — paused safely"));
    assert!(app.status.contains("Buffer Size"));
    if cfg!(debug_assertions) {
        assert!(app.status.contains("--release"));
    }

    // Staying overloaded does NOT overwrite subsequent user status messages (strictly rising-edge).
    app.status = "User inspected Devices".into();
    app.observe_snapshot(&overloaded);
    assert_eq!(app.status, "User inspected Devices");

    // ChromeView carries AudioHealth and flags status_alert while overloaded.
    let fv_ov = app.frame_view(&overloaded, "CoreAudio · 48 kHz");
    assert!(fv_ov.chrome.audio_health.overloaded);
    assert_eq!(fv_ov.chrome.audio_health.xrun_count, 3);
    assert!(fv_ov.chrome.status_alert);

    // 3. Rising edge `failed: false -> true` sets stream error status alert.
    let failed = AudioSnapshot {
        sample_rate: 48_000,
        health: AudioHealth {
            status: AudioStreamStatus::Failed,
            failed: true,
            error_code: AudioErrorCode::DeviceUnavailable,
            ..AudioHealth::default()
        },
        ..AudioSnapshot::default()
    };
    app.observe_snapshot(&failed);
    assert!(app.status.contains("Audio stream error"));
    assert!(app.status.contains("Ctrl+d (Audio)"));

    // 4. `set_midi_out_error` projects onto `MidiDeckView` and marks the `OUT` row with `alert`.
    app.execute_command("midi").unwrap();
    app.set_midi_out_error(Some(
        "MIDI OUT disconnected: MiniLab 3 DAW unplugged".into(),
    ));
    let mv = devices_view(&app, &healthy);
    assert_eq!(
        mv.midi_out_error.as_deref(),
        Some("MIDI OUT disconnected: MiniLab 3 DAW unplugged")
    );
    let out_row = mv.device_rows.iter().find(|r| r.label == "OUT").unwrap();
    assert!(out_row.alert);
    assert!(out_row.detail.contains("OUT ERROR"));

    // Clearing error restores normal OUT row state.
    app.set_midi_out_error(None);
    let mv_clear = devices_view(&app, &healthy);
    assert!(mv_clear.midi_out_error.is_none());
    assert!(
        !mv_clear
            .device_rows
            .iter()
            .find(|r| r.label == "OUT")
            .unwrap()
            .alert
    );

    // 5. Ex commands `:panic`, `:audio restart`, `:audio rescan` & Palette actions work cleanly.
    assert!(app
        .execute_command("panic")
        .unwrap()
        .transport
        .iter()
        .any(|t| matches!(t, TransportIntent::Reset { .. })));
    assert!(app.execute_command("audio restart").unwrap().audio_restart);
    let rescan = app.execute_command("audio rescan").unwrap();
    assert!(rescan.audio_rescan && rescan.midi_rescan);
    assert!(app.apply(Action::AudioPanic).is_ok());
    assert!(app.apply(Action::RestartAudio).unwrap().audio_restart);
    assert!(app.apply(Action::RescanAudio).unwrap().audio_rescan);

    // 6. Outcome::merge preserves audio_reconfigure, audio_input_reconfigure, recording_command, audio_restart, and audio_rescan.
    let mut dst = Outcome::default();
    let src = Outcome {
        audio_reconfigure: Some(app.config.audio_stream_request()),
        audio_input_reconfigure: Some(app.config.audio_input_request()),
        recording_command: Some(cadence_app::RecordingHostCommand::Cancel),
        audio_restart: true,
        audio_rescan: true,
        ..Outcome::default()
    };
    dst.merge(src);
    assert!(dst.audio_reconfigure.is_some());
    assert!(dst.audio_input_reconfigure.is_some());
    assert!(matches!(
        dst.recording_command,
        Some(cadence_app::RecordingHostCommand::Cancel)
    ));
    assert!(dst.audio_restart);
    assert!(dst.audio_rescan);
}

#[test]
fn observe_transport_recompiles_live_window_when_step_rewinds_below_base() {
    use cadence_core::material::LIVE_RING_STEPS;

    let (mut app, _io, _dir) = memory_app();
    // Advance transport past half the ring so the live score window rebases past 0.
    let far_step = LIVE_RING_STEPS / 2 + 32;
    let out_fwd = app.observe_transport(far_step - 1, far_step, true, false);
    assert!(out_fwd.changed);
    let fwd_win = app
        .audio_project()
        .live_window
        .expect("rebased live window");
    assert!(fwd_win.start >= LIVE_RING_STEPS / 2);
    assert!(fwd_win.contains(far_step));

    // If transport ever observes a step before `live_window.start`, observe_transport must
    // immediately recompile the ring window so playback cannot stall in a silent dead zone.
    let out_back = app.observe_transport(0, 0, false, false);
    assert!(
        out_back.changed,
        "rewinding below live_window.start must trigger audio score recompile"
    );
    let back_win = app
        .audio_project()
        .live_window
        .expect("recompiled live window");
    assert_eq!(back_win.start, 0);
    assert!(back_win.contains(0));
}
