//! End-to-end integration tests for live audio input recording:
//! input configuration, software monitoring, quantized & count-in & free-time capture,
//! atomic 24-bit WAV writing, Sampler Rack + Arrange Placement + RackZone landing,
//! undo/redo, Record workflow dialog (`t A`), and control protocol telemetry.
use cadence::host::{
    apply_outcome_with_hooks, execute_host_request_bytes, HostRecorder, HostRequestContext,
    HostRequestOutcome, OutcomeHooks,
};
use cadence_app::{App, Outcome, RecordingHostCommand};
use cadence_core::material::Project;
use cadence_core::rack::ModuleKind;
use cadence_core::runtime::{AudioInputMode, AudioInputStatus, CapturePhase, InputMonitorMode};
use cadence_engine::audio::{render_measure_report, AudioRuntime};
use std::time::{Duration, Instant};

#[derive(Default)]
struct RecordingTestHooks {
    recorder: HostRecorder,
}

impl OutcomeHooks for RecordingTestHooks {
    fn handle_recording(
        &mut self,
        audio: &mut AudioRuntime,
        cmd: &RecordingHostCommand,
    ) -> anyhow::Result<()> {
        self.recorder.handle_command(cmd, audio)
    }
}

fn synthetic_stereo_sine(frames: usize, sample_rate: u32, freq_hz: f64, amp: f32) -> Vec<[f32; 2]> {
    (0..frames)
        .map(|i| {
            let s = (std::f64::consts::TAU * freq_hz * i as f64 / f64::from(sample_rate)).sin()
                as f32
                * amp;
            [s, s * 0.8]
        })
        .collect()
}

fn press_merged(app: &mut App, keys: &str) -> Outcome {
    let mut merged = Outcome::default();
    for out in app.press_keys(keys).unwrap() {
        merged.merge(out);
    }
    merged
}

fn dispatch(
    app: &mut App,
    engine: &mut AudioRuntime,
    hooks: &mut RecordingTestHooks,
    out: Outcome,
) {
    apply_outcome_with_hooks(app, Some(engine), &out, hooks).unwrap();
}

fn poll_until_recorded(
    app: &mut App,
    engine: &mut AudioRuntime,
    hooks: &mut RecordingTestHooks,
    inject_chunk: &[[f32; 2]],
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    while hooks.recorder.is_active() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for HostRecorder finalization (phase={:?}, step={})",
            engine.input_telemetry().capture_phase,
            engine.snapshot().absolute_step
        );
        engine.inject_input_frames(inject_chunk, 48_000);
        let snap = engine.snapshot();
        app.observe_snapshot(&snap);
        let (_, follow_up) = hooks.recorder.poll(app, engine);
        if let Some(out) = follow_up {
            dispatch(app, engine, hooks, out);
        }
        std::thread::sleep(Duration::from_millis(8));
    }
}

#[test]
fn quantized_audio_recording_lands_24bit_wav_sampler_placement_audibly_and_undoes() {
    let dir = tempfile::tempdir().unwrap();
    let project_path = dir.path().join("session.json");
    let mut project = Project::empty();
    project.bpm = 300.0; // Fast tempo: 1 bar (16 steps) = 0.8s wall clock.
    project.bars = 4;

    let lib = cadence_core::sampler::SampleLibrary::from_roots(&[], Some(&project_path));
    let mut app = App::new_with_library(project, project_path, lib).unwrap();
    let mut engine =
        AudioRuntime::start_with_samples(app.audio_project().clone(), app.sample_bank(), true)
            .unwrap();
    let mut hooks = RecordingTestHooks::default();

    // 1. Pressing `t a` before selecting an audio input device gives actionable guidance.
    let no_input_out = press_merged(&mut app, "ta");
    assert!(no_input_out.recording_command.is_none());
    assert!(
        app.status.contains("Select an Audio Input") || app.status.contains("Ctrl+d"),
        "expected actionable no-input guidance, got {:?}",
        app.status
    );

    // 2. Configure input device, channel mode, software monitor (`t i`), and latency trim.
    let out = app.execute_command("rec input Test Interface").unwrap();
    assert!(out.audio_reconfigure.is_none());
    assert!(out.audio_input_reconfigure.is_some());
    dispatch(&mut app, &mut engine, &mut hooks, out);
    assert_eq!(
        app.config.audio_input_device.as_deref(),
        Some("Test Interface")
    );

    let out = app.execute_command("rec mode stereo").unwrap();
    dispatch(&mut app, &mut engine, &mut hooks, out);
    assert_eq!(app.config.audio_input_mode, AudioInputMode::Stereo);

    // `t i` cycles Off -> Auto -> On -> Off without disturbing output.
    let out = press_merged(&mut app, "ti");
    assert!(out.audio_reconfigure.is_none());
    assert!(out.audio_input_reconfigure.is_some());
    dispatch(&mut app, &mut engine, &mut hooks, out);
    assert_eq!(app.config.audio_input_monitor, InputMonitorMode::Auto);

    let out = app.execute_command("rec latency 2.5").unwrap();
    dispatch(&mut app, &mut engine, &mut hooks, out);
    assert!((app.config.audio_input_latency_ms - 2.5).abs() < 1e-4);

    // Prime synthetic input stream so telemetry reports Running at 48 kHz.
    let chunk = synthetic_stereo_sine(512, 48_000, 440.0, 0.55);
    engine.inject_input_frames(&chunk, 48_000);
    app.observe_snapshot(&engine.snapshot());
    assert_eq!(engine.input_telemetry().status, AudioInputStatus::Running);

    // 3. Arm quantized 1-bar recording (`:rec bars 1 LeadTake`) from stopped state.
    let arm_out = app.execute_command("rec bars 1 LeadTake").unwrap();
    assert!(matches!(
        arm_out.recording_command,
        Some(RecordingHostCommand::Arm { .. })
    ));
    dispatch(&mut app, &mut engine, &mut hooks, arm_out);
    assert!(hooks.recorder.is_active());

    // Drive input frames across the 1-bar capture until punch-out + tail finalize cleanly.
    poll_until_recorded(
        &mut app,
        &mut engine,
        &mut hooks,
        &chunk,
        Duration::from_secs(6),
    );
    assert_eq!(app.recording.phase, CapturePhase::Idle);

    // 4. Verify 24-bit stereo WAV on disk, Sampler Rack + Part + Placement landed at step 0.
    let rec_dir = dir.path().join("samples").join("recordings");
    let wavs: Vec<_> = std::fs::read_dir(&rec_dir)
        .expect("samples/recordings directory must exist")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "wav"))
        .collect();
    assert_eq!(wavs.len(), 1, "expected 1 recorded WAV file, got {wavs:?}");

    let wav_reader = hound::WavReader::open(&wavs[0]).unwrap();
    assert_eq!(wav_reader.spec().channels, 2);
    assert_eq!(wav_reader.spec().sample_rate, 48_000);
    assert_eq!(wav_reader.spec().bits_per_sample, 24);
    assert_eq!(wav_reader.spec().sample_format, hound::SampleFormat::Int);

    let inst = app
        .project()
        .instruments
        .iter()
        .find(|i| i.name == "LeadTake")
        .expect("LeadTake Sampler instrument must be created");
    let sampler_path = inst.rack.zones[0]
        .modules
        .iter()
        .find_map(|m| match &m.kind {
            ModuleKind::Sampler(smp) => smp.path.clone(),
            _ => None,
        })
        .expect("LeadTake zone must hold Sampler module with sample path");
    assert!(sampler_path.contains("samples/recordings/"));
    assert!(
        !app.sample_bank().is_empty(),
        "recorded WAV must be loaded into live SampleBank"
    );

    // Verify arranged Placement landed on bar 1 (step 0).
    let part = app
        .project()
        .parts
        .iter()
        .find(|p| p.instrument == inst.id)
        .expect("trigger Part must be created for LeadTake");
    let placement = app
        .project()
        .lanes
        .iter()
        .flat_map(|l| l.placements.iter())
        .find(|pl| pl.part == part.id)
        .expect("arranged Placement must exist for LeadTake");
    assert_eq!(placement.start_step, 0);

    // 5. Offline-render the project and confirm the recorded clip plays back audibly!
    let compiled = app.project().compile().unwrap();
    let report = render_measure_report(&compiled, app.sample_bank(), 1, 24_000).unwrap();
    assert!(
        report.output.peak[0] > 0.05 && report.output.peak[1] > 0.05,
        "recorded 24-bit clip must play back audibly through Sampler Rack: peak={:?}",
        report.output.peak
    );

    // 6. Verify single-transaction Undo (`u`) and Redo (`U`).
    let undo_out = press_merged(&mut app, "u");
    dispatch(&mut app, &mut engine, &mut hooks, undo_out);
    assert!(
        app.project()
            .instruments
            .iter()
            .all(|i| i.name != "LeadTake"),
        "Undo must cleanly remove recorded LeadTake instrument"
    );
    let redo_out = press_merged(&mut app, "U");
    dispatch(&mut app, &mut engine, &mut hooks, redo_out);
    assert!(
        app.project()
            .instruments
            .iter()
            .any(|i| i.name == "LeadTake"),
        "Redo must cleanly restore recorded LeadTake instrument"
    );

    // 7. Verify control protocol `meters` & command responses expose `audio_input` JSON.
    let mut ctx = HostRequestContext {
        audio: Some(&mut engine),
        default_ear_path: dir.path().join("ear.wav"),
        live_tap_busy: None,
    };
    let meters_val =
        match execute_host_request_bytes(&mut app, br#"{"meters":true}"#, &mut ctx, &mut hooks) {
            HostRequestOutcome::Immediate { response, .. } => response,
            _ => panic!("expected Immediate meters response"),
        };
    assert_eq!(meters_val["ok"], true);
    assert_eq!(meters_val["audio_input"]["status"], "running");
    assert_eq!(meters_val["audio_input"]["sample_rate"], 48_000);
    assert_eq!(meters_val["audio_input"]["mode"], "stereo");
    assert_eq!(meters_val["audio_input"]["monitor_mode"], "auto");

    let cmd_val = match execute_host_request_bytes(
        &mut app,
        br#"{"cmd":"rec monitor on"}"#,
        &mut ctx,
        &mut hooks,
    ) {
        HostRequestOutcome::Immediate { response, .. } => response,
        _ => panic!("expected Immediate cmd response"),
    };
    assert_eq!(cmd_val["ok"], true);
    assert_eq!(cmd_val["transport"]["audio_input"]["status"], "running");
    assert_eq!(cmd_val["transport"]["audio_input"]["monitor_mode"], "on");
    assert_eq!(cmd_val["transport"]["audio_input"]["capture_phase"], "idle");
}

#[test]
fn free_time_count_in_rack_zone_and_record_workflow_dialog_all_work() {
    let dir = tempfile::tempdir().unwrap();
    let project_path = dir.path().join("modes.json");
    let mut project = Project::empty();
    project.bpm = 300.0;
    project.bars = 4;

    let lib = cadence_core::sampler::SampleLibrary::from_roots(&[], Some(&project_path));
    let mut app = App::new_with_library(project, project_path, lib).unwrap();
    let mut engine =
        AudioRuntime::start_with_samples(app.audio_project().clone(), app.sample_bank(), true)
            .unwrap();
    let mut hooks = RecordingTestHooks::default();

    let out = app.execute_command("rec input Test Interface").unwrap();
    dispatch(&mut app, &mut engine, &mut hooks, out);
    // Set large manual latency trim (40 ms) to verify Free mode ignores latency trim.
    let out = app.execute_command("rec latency 40").unwrap();
    dispatch(&mut app, &mut engine, &mut hooks, out);

    let chunk = synthetic_stereo_sine(600, 48_000, 330.0, 0.5);
    engine.inject_input_frames(&chunk, 48_000);
    app.observe_snapshot(&engine.snapshot());

    // 1. Free-time recording (`:rec free FreeClip`) does NOT start transport and preserves exact frames.
    assert!(!app.playing());
    let out = app.execute_command("rec free FreeClip").unwrap();
    dispatch(&mut app, &mut engine, &mut hooks, out);
    assert!(
        !app.playing(),
        "free-time recording must not force transport play"
    );
    assert_eq!(
        engine.input_telemetry().capture_phase,
        CapturePhase::Recording
    );

    // Inject 4 exact chunks (2400 frames) then press `t a` to stop!
    for _ in 0..4 {
        engine.inject_input_frames(&chunk, 48_000);
        let (_, fu) = hooks.recorder.poll(&mut app, &mut engine);
        if let Some(o) = fu {
            dispatch(&mut app, &mut engine, &mut hooks, o);
        }
    }
    let stop_out = press_merged(&mut app, "ta");
    assert!(matches!(
        stop_out.recording_command,
        Some(RecordingHostCommand::Stop)
    ));
    dispatch(&mut app, &mut engine, &mut hooks, stop_out);

    // Drain any Finishing tail cleanly and finalize.
    engine.force_seal_capture();
    let (_, fu) = hooks.recorder.poll(&mut app, &mut engine);
    if let Some(o) = fu {
        dispatch(&mut app, &mut engine, &mut hooks, o);
    }
    assert!(!hooks.recorder.is_active());
    let free_inst = app
        .project()
        .instruments
        .iter()
        .find(|i| i.name == "FreeClip")
        .expect("FreeClip Sampler instrument must be created");
    let free_smp_frames = free_inst.rack.zones[0]
        .modules
        .iter()
        .find_map(|m| match &m.kind {
            ModuleKind::Sampler(smp) => Some(smp.sample_end),
            _ => None,
        })
        .expect("FreeClip sampler frames");
    assert_eq!(
        free_smp_frames, 2400,
        "Free mode must preserve full captured frame count without latency trimming"
    );

    // Reset latency trim back to 0 for subsequent tests.
    let out = app.execute_command("rec latency 0").unwrap();
    dispatch(&mut app, &mut engine, &mut hooks, out);

    // 2. Count-in recording (`:rec countin 1 CountTake`): stays Armed across 1 bar, then records 1 bar.
    let out = app.execute_command("rec countin 1 CountTake").unwrap();
    dispatch(&mut app, &mut engine, &mut hooks, out);
    assert_eq!(engine.input_telemetry().capture_phase, CapturePhase::Armed);
    poll_until_recorded(
        &mut app,
        &mut engine,
        &mut hooks,
        &chunk,
        Duration::from_secs(8),
    );
    assert!(
        app.project()
            .instruments
            .iter()
            .any(|i| i.name == "CountTake"),
        "CountTake must land after 1-bar count-in + 1-bar capture"
    );
    let stop_transport = press_merged(&mut app, "<Space>");
    dispatch(&mut app, &mut engine, &mut hooks, stop_transport);

    // 3. Target RackZone recording (`:rec zone FreeClip z1`) replaces the existing zone sample in place.
    let inst_count_before = app.project().instruments.len();
    let zone_out = app.execute_command("rec zone FreeClip z1").unwrap();
    dispatch(&mut app, &mut engine, &mut hooks, zone_out);
    // Inject frames and stop after a few blocks.
    for _ in 0..8 {
        engine.inject_input_frames(&chunk, 48_000);
        app.observe_snapshot(&engine.snapshot());
        let (_, fu) = hooks.recorder.poll(&mut app, &mut engine);
        if let Some(o) = fu {
            dispatch(&mut app, &mut engine, &mut hooks, o);
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    let stop_out = app.execute_command("rec stop").unwrap();
    dispatch(&mut app, &mut engine, &mut hooks, stop_out);
    engine.force_seal_capture();
    let (_, fu) = hooks.recorder.poll(&mut app, &mut engine);
    if let Some(o) = fu {
        dispatch(&mut app, &mut engine, &mut hooks, o);
    }
    assert!(!hooks.recorder.is_active());
    assert_eq!(
        app.project().instruments.len(),
        inst_count_before,
        "recording into an existing RackZone must update in place without spawning a new instrument"
    );
    let stop_transport = press_merged(&mut app, "<Space>");
    dispatch(&mut app, &mut engine, &mut hooks, stop_transport);

    // 4. `t A` opens the Record workflow dialog (`WorkflowKind::Record`) and Enter arms cleanly!
    let dlg_out = press_merged(&mut app, "tA");
    assert!(dlg_out.recording_command.is_none());
    assert_eq!(app.mode_name(), "WORKFLOW");
    let commit_out = press_merged(&mut app, "<Enter>");
    assert!(matches!(
        commit_out.recording_command,
        Some(RecordingHostCommand::Arm { .. })
    ));
    dispatch(&mut app, &mut engine, &mut hooks, commit_out);
    assert!(hooks.recorder.is_active());
    // Cancel via `:rec cancel` discards cleanly.
    let cancel_out = app.execute_command("rec cancel").unwrap();
    dispatch(&mut app, &mut engine, &mut hooks, cancel_out);
    assert!(!hooks.recorder.is_active());
    assert_eq!(engine.input_telemetry().capture_phase, CapturePhase::Idle);
}

#[test]
fn stopped_transport_recording_preserves_active_loop_region_across_quantized_and_count_in() {
    let dir = tempfile::tempdir().unwrap();
    let project_path = dir.path().join("loop_rec.json");
    let mut project = Project::empty();
    project.bpm = 300.0;
    project.bars = 4;

    let lib = cadence_core::sampler::SampleLibrary::from_roots(&[], Some(&project_path));
    let mut app = App::new_with_library(project, project_path, lib).unwrap();
    let mut engine =
        AudioRuntime::start_with_samples(app.audio_project().clone(), app.sample_bank(), true)
            .unwrap();
    let mut hooks = RecordingTestHooks::default();

    let out = app.execute_command("rec input Test Interface").unwrap();
    dispatch(&mut app, &mut engine, &mut hooks, out);
    let chunk = synthetic_stereo_sine(512, 48_000, 440.0, 0.5);
    engine.inject_input_frames(&chunk, 48_000);
    app.observe_snapshot(&engine.snapshot());

    // 1. Set and activate a 1-bar loop on bar 2 ([16, 32)) via Arrange `[` / `]` + `\` while stopped,
    //    then arm quantized recording (`t a`).
    let loop_out = press_merged(&mut app, "gaL[]\\");
    dispatch(&mut app, &mut engine, &mut hooks, loop_out);
    assert_eq!(app.playing_loop(), Some((16, 32)));
    assert!(!app.playing());

    let arm_out = press_merged(&mut app, "ta");
    let req = match &arm_out.recording_command {
        Some(RecordingHostCommand::Arm { request }) => (**request).clone(),
        other => panic!("expected RecordingHostCommand::Arm, got {other:?}"),
    };
    assert_eq!(req.punch_in_song_step, 16);
    assert_eq!(req.punch_out_abs_step, Some(req.punch_in_abs_step + 16));
    assert_eq!(
        app.playing_loop(),
        Some((16, 32)),
        "starting quantized recording from stopped transport must preserve active loop region"
    );
    dispatch(&mut app, &mut engine, &mut hooks, arm_out);

    poll_until_recorded(
        &mut app,
        &mut engine,
        &mut hooks,
        &chunk,
        Duration::from_secs(8),
    );
    assert_eq!(
        app.playing_loop(),
        Some((16, 32)),
        "active loop region must remain intact after quantized loop recording finishes"
    );
    let landed_placement = app
        .project()
        .lanes
        .iter()
        .flat_map(|l| l.placements.iter())
        .next()
        .expect("landed loop recording placement");
    assert_eq!(landed_placement.start_step, 16);

    // Stop transport cleanly before testing 1-bar count-in on bar 2.
    let stop_out = press_merged(&mut app, "<Space>");
    dispatch(&mut app, &mut engine, &mut hooks, stop_out);
    assert!(!app.playing());
    assert_eq!(app.playing_loop(), Some((16, 32)));

    // 2. Count-in recording on bar 2 leads in from bar 1 (step 0), preserves loop [16, 32),
    //    and punches in right at bar 2 (step 16).
    let ci_out = app.execute_command("rec countin LoopCountIn").unwrap();
    let ci_req = match &ci_out.recording_command {
        Some(RecordingHostCommand::Arm { request }) => (**request).clone(),
        other => panic!("expected RecordingHostCommand::Arm, got {other:?}"),
    };
    assert_eq!(ci_req.punch_in_song_step, 16);
    assert_eq!(
        ci_req.punch_out_abs_step,
        Some(ci_req.punch_in_abs_step + 16)
    );
    assert_eq!(app.playing_loop(), Some((16, 32)));
    assert_eq!(
        app.song_step_heard(),
        0,
        "count-in before bar 2 loop must cue bar 1 (step 0) as lead-in"
    );
    let cancel_out = app.execute_command("rec cancel").unwrap();
    dispatch(&mut app, &mut engine, &mut hooks, cancel_out);
    let stop_out = press_merged(&mut app, "<Space>");
    dispatch(&mut app, &mut engine, &mut hooks, stop_out);

    // 3. Count-in recording on bar 1–2 loop ([0, 32)) cues bar 2 (step 16) so the 1-bar count-in
    //    wraps directly onto bar 1 (step 0) while preserving loop [0, 32).
    let loop_bar1 = press_merged(&mut app, "ga0[L]");
    dispatch(&mut app, &mut engine, &mut hooks, loop_bar1);
    assert_eq!(app.playing_loop(), Some((0, 32)));
    assert!(!app.playing());

    let bar1_ci = app.execute_command("rec countin Bar1Loop").unwrap();
    let bar1_req = match &bar1_ci.recording_command {
        Some(RecordingHostCommand::Arm { request }) => (**request).clone(),
        other => panic!("expected RecordingHostCommand::Arm, got {other:?}"),
    };
    assert_eq!(app.playing_loop(), Some((0, 32)));
    assert_eq!(
        app.song_step_heard(),
        16,
        "count-in into bar 1–2 loop must cue bar 2 (step 16) to wrap onto bar 1"
    );
    assert_eq!(bar1_req.punch_in_song_step, 0);
    assert_eq!(
        bar1_req.punch_out_abs_step,
        Some(bar1_req.punch_in_abs_step + 32)
    );
    let cancel_out = app.execute_command("rec cancel").unwrap();
    dispatch(&mut app, &mut engine, &mut hooks, cancel_out);
    let stop_out = press_merged(&mut app, "<Space>");
    dispatch(&mut app, &mut engine, &mut hooks, stop_out);

    // 4. All-zero capture surfaces an actionable OS Microphone permission warning,
    //    and opening Sample Browser from the landed Sampler jumps straight to the file.
    let out = app.execute_command("rec free SilentTake").unwrap();
    dispatch(&mut app, &mut engine, &mut hooks, out);
    let zeros = vec![[0.0_f32, 0.0_f32]; 512];
    for _ in 0..4 {
        engine.inject_input_frames(&zeros, 48_000);
        let (_, fu) = hooks.recorder.poll(&mut app, &mut engine);
        if let Some(o) = fu {
            dispatch(&mut app, &mut engine, &mut hooks, o);
        }
    }
    let stop_rec = app.execute_command("rec stop").unwrap();
    dispatch(&mut app, &mut engine, &mut hooks, stop_rec);
    engine.force_seal_capture();
    let (_, fu) = hooks.recorder.poll(&mut app, &mut engine);
    if let Some(o) = fu {
        dispatch(&mut app, &mut engine, &mut hooks, o);
    }
    assert!(
        app.status.contains("all zeros") && app.status.contains("Microphone"),
        "expected actionable all-zeros microphone permission warning, got {:?}",
        app.status
    );
    // `B` on the opened Sampler Rack opens Sample Browser positioned on the recorded WAV!
    let _ = press_merged(&mut app, "B");
    let bv = app.browser_view().expect("Sample Browser must open");
    let selected_label = bv
        .entries
        .iter()
        .find(|e| e.selected)
        .map(|e| e.label.clone())
        .expect("recorded WAV must be highlighted in Sample Browser");
    assert!(
        selected_label.contains("silenttake"),
        "expected Sample Browser cursor on silenttake WAV, got {:?}",
        selected_label
    );
}
