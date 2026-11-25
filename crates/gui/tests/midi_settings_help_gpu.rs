//! Headless Metal/wgpu GPU visual tests for the Devices Deck (`gd` / `Ctrl+d`),
//! Settings (`g,`), and `?` Help overlay across Dark and Studio Light themes.
//!
//! Set `CADENCE_DUMP_MIDI_SETTINGS_HELP_DIR=/path/to/dir` to save 1440×860 PNG screenshots.

mod common;

use cadence_app::view::{DeckView, DevicesTab, OverlayView};
use cadence_app::{Action, App};
use cadence_core::material::Project;
use cadence_core::midi_map::{MidiMapping, MidiPadMode, MidiSource, MidiTarget, MidiValueMode};
use cadence_core::mixer::{ChannelParam, ChannelRef};
use cadence_core::params::ParamAddress;
use cadence_core::runtime::{
    AudioDeviceInfo, AudioErrorCode, AudioHealth, AudioInputMode, AudioInputStatus,
    AudioInputTelemetry, AudioInventory, AudioStreamStatus, CapturePhase, InputMonitorMode,
    MidiMessage,
};
use cadence_gui::decks::build_display_list;
use cadence_gui::headless::{distinct_pixel_ratio, HeadlessGpu};
use cadence_gui::theme::Theme;
use std::path::PathBuf;

fn maybe_save_png(name: &str, width: u32, height: u32, rgba: &[u8]) {
    common::maybe_save_png(
        "CADENCE_DUMP_MIDI_SETTINGS_HELP_DIR",
        name,
        width,
        height,
        rgba,
    );
}

#[test]
fn gpu_renders_midi_deck_settings_and_help_dark_and_light() {
    let Ok(mut gpu) = HeadlessGpu::new(1440, 860) else {
        return;
    };
    let dark = Theme::cadence_dark();
    let light = Theme::studio_light();
    let status = "Metal · 48 kHz";

    let mut app = App::new(Project::demo(), PathBuf::from("midi_settings_help.json")).unwrap();
    app.apply(Action::Play { playing: true }).unwrap();
    app.midi.in_ports = vec!["Arturia MiniLab 3 MIDI".into(), "Launchpad X".into()];
    app.midi.out_ports = vec!["Arturia MiniLab 3 DAW".into()];
    app.midi.port = Some("Arturia MiniLab 3 MIDI".into());
    app.midi.out_port = Some("Arturia MiniLab 3 DAW".into());
    app.config.midi_in_port = Some("Arturia MiniLab 3 MIDI".into());
    app.config.midi_out_port = Some("Arturia MiniLab 3 DAW".into());
    app.config.midi_pad_mode = MidiPadMode::Sections;
    app.config.midi_pickup = true;
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

    let snap = common::build_snapshot(app.project(), 16);

    // 1. Devices · MIDI Deck (`gd zz`) with DEVICE & PROFILE focused (Dark & Light).
    app.press_keys("gdzz").unwrap();
    let fv_midi = app.frame_view(&snap, status);
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
    let dl_midi = build_display_list(&fv_midi, 1440.0, 860.0, 1.0, &dark);
    assert_eq!(
        gpu.first_missing_glyph(&dl_midi, &dark),
        None,
        "MIDI Deck emitted a glyph missing from JetBrains Mono"
    );
    let px_midi_dark = gpu.render_rgba8(&fv_midi, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px_midi_dark, dark.surface.canvas) > 0.20);
    maybe_save_png(
        "01-midi-deck-device-profile-dark.png",
        1440,
        860,
        &px_midi_dark,
    );

    let px_midi_light = gpu.render_rgba8(&fv_midi, &light).unwrap();
    assert!(distinct_pixel_ratio(&px_midi_light, light.surface.canvas) > 0.20);
    maybe_save_png(
        "02-midi-deck-device-profile-light.png",
        1440,
        860,
        &px_midi_light,
    );

    // 2. MIDI Deck with `w` jumped into MAPPINGS + MIDI Learn armed (`Enter`).
    app.press_keys("w<Enter>").unwrap();
    let fv_learn = app.frame_view(&snap, status);
    let dl_learn = build_display_list(&fv_learn, 1440.0, 860.0, 1.0, &dark);
    assert_eq!(gpu.first_missing_glyph(&dl_learn, &dark), None);
    let px_learn = gpu.render_rgba8(&fv_learn, &dark).unwrap();
    assert_ne!(px_learn, px_midi_dark);
    maybe_save_png("03-midi-deck-mappings-learn-dark.png", 1440, 860, &px_learn);
    app.press_keys("<Esc>zz").unwrap();

    // 3. Settings (`g,`) showing Contextual Key Hints & MONITORING rows (Dark & Light).
    app.press_keys("g,").unwrap();
    app.settings_ui.cursor = 2; // Contextual Key Hints row — shows APPEARANCE, WORKSPACE, MONITORING
    let fv_settings = app.frame_view(&snap, status);
    assert!(matches!(
        fv_settings.overlay,
        Some(OverlayView::Settings(_))
    ));
    let dl_settings = build_display_list(&fv_settings, 1440.0, 860.0, 1.0, &dark);
    assert_eq!(
        gpu.first_missing_glyph(&dl_settings, &dark),
        None,
        "Settings overlay emitted a glyph missing from JetBrains Mono"
    );
    let px_settings_dark = gpu.render_rgba8(&fv_settings, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px_settings_dark, dark.surface.canvas) > 0.20);
    maybe_save_png(
        "04-settings-hints-scale-dark.png",
        1440,
        860,
        &px_settings_dark,
    );

    let px_settings_light = gpu.render_rgba8(&fv_settings, &light).unwrap();
    assert!(distinct_pixel_ratio(&px_settings_light, light.surface.canvas) > 0.20);
    maybe_save_png(
        "05-settings-hints-scale-light.png",
        1440,
        860,
        &px_settings_light,
    );
    app.press_keys("<Esc>").unwrap();

    // 4. `?` Help overlay on Live & Jam tab (Dark) and Navigate tab (Light).
    app.press_keys("?LLLLL").unwrap(); // All -> Navigate -> Compose -> Mini -> Sound -> Live & Jam
    let fv_help_live = app.frame_view(&snap, status);
    assert!(matches!(fv_help_live.overlay, Some(OverlayView::Help(_))));
    let dl_help_live = build_display_list(&fv_help_live, 1440.0, 860.0, 1.0, &dark);
    assert_eq!(gpu.first_missing_glyph(&dl_help_live, &dark), None);
    let px_help_dark = gpu.render_rgba8(&fv_help_live, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px_help_dark, dark.surface.canvas) > 0.20);
    maybe_save_png(
        "06-help-overlay-live-midi-dark.png",
        1440,
        860,
        &px_help_dark,
    );

    app.press_keys("HHHH").unwrap(); // Navigate tab
    let fv_help_nav = app.frame_view(&snap, status);
    let dl_help_nav = build_display_list(&fv_help_nav, 1440.0, 860.0, 1.0, &light);
    assert_eq!(gpu.first_missing_glyph(&dl_help_nav, &light), None);
    let px_help_light = gpu.render_rgba8(&fv_help_nav, &light).unwrap();
    assert!(distinct_pixel_ratio(&px_help_light, light.surface.canvas) > 0.20);
    maybe_save_png(
        "07-help-overlay-navigate-light.png",
        1440,
        860,
        &px_help_light,
    );
    app.press_keys("<Esc>").unwrap();

    // 5. DEVICES · AUDIO (Healthy Dark & Studio Light) with active Audio Input, live meters,
    //    monitoring, and active `● REC` Chrome indicator.
    app.config.audio_input_device = Some("Focusrite Scarlett 4i4 In".into());
    app.config.audio_input_mode = AudioInputMode::Stereo;
    app.config.audio_input_monitor = InputMonitorMode::Auto;
    app.config.audio_input_latency_ms = 2.5;
    app.execute_command("audio").unwrap();
    app.press_keys("zz").unwrap();
    let mut healthy_snap = common::build_snapshot(app.project(), 24);
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
    let dl_audio_dark = build_display_list(&fv_audio_ok, 1440.0, 860.0, 1.0, &dark);
    assert_eq!(
        gpu.first_missing_glyph(&dl_audio_dark, &dark),
        None,
        "Devices · Audio (Dark) emitted a glyph missing from JetBrains Mono"
    );
    let px_audio_dark = gpu.render_rgba8(&fv_audio_ok, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px_audio_dark, dark.surface.canvas) > 0.20);
    maybe_save_png(
        "08-devices-audio-healthy-dark.png",
        1440,
        860,
        &px_audio_dark,
    );

    let px_audio_light = gpu.render_rgba8(&fv_audio_ok, &light).unwrap();
    assert!(distinct_pixel_ratio(&px_audio_light, light.surface.canvas) > 0.20);
    maybe_save_png(
        "09-devices-audio-healthy-light.png",
        1440,
        860,
        &px_audio_light,
    );

    // 6. DEVICES · AUDIO in OVERLOAD PAUSED alert state (error card + top Chrome !OVERLOAD badge + `○ ARM` + input CLIP!).
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
    app.press_keys("ww").unwrap(); // Focus Audio Panic row (group 3)
    let fv_overload = app.frame_view(&overload_snap, "CoreAudio · 48 kHz");
    assert!(fv_overload.chrome.audio_health.overloaded);
    assert!(fv_overload.chrome.status_alert);
    let dl_overload = build_display_list(&fv_overload, 1440.0, 860.0, 1.0, &dark);
    assert_eq!(
        gpu.first_missing_glyph(&dl_overload, &dark),
        None,
        "Devices · Audio (Overload) emitted a glyph missing from JetBrains Mono"
    );
    let px_overload = gpu.render_rgba8(&fv_overload, &dark).unwrap();
    assert_ne!(px_overload, px_audio_dark);
    maybe_save_png(
        "10-devices-audio-overload-alert-dark.png",
        1440,
        860,
        &px_overload,
    );

    // 7. DEVICES · AUDIO in STREAM ERROR alert state (top Chrome !AUDIO ERR badge + OS error box).
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
    app.press_keys("j").unwrap(); // Focus Restart Audio row
    let fv_failed = app.frame_view(&failed_snap, "CoreAudio · error");
    assert!(fv_failed.chrome.audio_health.failed);
    let dl_failed = build_display_list(&fv_failed, 1440.0, 860.0, 1.0, &dark);
    assert_eq!(gpu.first_missing_glyph(&dl_failed, &dark), None);
    let px_failed = gpu.render_rgba8(&fv_failed, &dark).unwrap();
    assert_ne!(px_failed, px_overload);
    maybe_save_png(
        "11-devices-audio-stream-error-dark.png",
        1440,
        860,
        &px_failed,
    );

    // 8. DEVICES · AUDIO with active input delivering pure 0.0 digital silence (OS mic permission hint).
    let mut sil_inv = app.devices.inventory.clone();
    sil_inv.last_error = None;
    app.set_audio_inventory(sil_inv);
    let mut sil_snap = common::build_snapshot(app.project(), 24);
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
    let fv_sil = app.frame_view(&sil_snap, "CoreAudio · 48 kHz");
    let dl_sil = build_display_list(&fv_sil, 1440.0, 860.0, 1.0, &dark);
    assert_eq!(gpu.first_missing_glyph(&dl_sil, &dark), None);
    let px_sil = gpu.render_rgba8(&fv_sil, &dark).unwrap();
    maybe_save_png(
        "12-devices-audio-digital-silence-dark.png",
        1440,
        860,
        &px_sil,
    );
}
