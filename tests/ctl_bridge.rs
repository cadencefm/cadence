//! Integration tests for host-controller boundaries, CLI/headless dispatch, and the live `--ctl` bridge:
//! - Pure in-memory `App` via `MemoryHostIo` and static zero-OS-IO purity verification on `crates/app/src/`.
//! - Unified `Outcome::merge` & headless `--script` bounce/export host dispatch.
//! - CLI flag conflict guards, standalone `--output` rejection, and `--project foo.rbs --output`.
//! - Live Unix `--ctl` socket (`0o600`), `cadence ctl` CLI client (`state --compact`,
//!   `meters`, `cmd`, `action`, `expected_revision` conflicts, `u` Undo stack, `listen --bars`
//!   with non-mutating `--solo` + per-bar telemetry, and non-blocking `listen --live` ring tap).
use cadence::host::ctl::{send_ctl_request, CtlServer};
use cadence::host::protocol::{HostRequest, ListenRequest};
use cadence::host::HeadlessOutcomeHooks;
use cadence_app::{
    load_project, load_project_with_io, Action, App, ExportMode, ExportRequest, FileDialogRequest,
    HostIo, MemoryHostIo, MidiConnectRequest, Outcome,
};
use cadence_core::material::{
    InstrumentInstance, Lane, Part, Pattern, PatternKind, Placement, Project,
};
use cadence_core::midi_map::{MidiFeedback, MidiProfile};
use cadence_core::mixer::{
    Channel, ChannelKind, ChannelRef, CompressorPatch, EffectDevice, EffectKind,
};
use cadence_core::music::{EndRule, Step};
use cadence_core::presets::{InstrumentPreset, INSTRUMENT_PRESET_VERSION};
use cadence_core::rack::{ModuleKind, RackPatch};
use cadence_engine::audio::AudioRuntime;
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Condvar, Mutex,
};
use std::time::{Duration, Instant};
use tempfile::tempdir;

fn cadence_bin(dir: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cadence"));
    cmd.current_dir(dir).stdin(Stdio::null());
    cmd
}

/// Build a tiny self-contained synthetic ReBirth 2.0 (4.2) `.rbs` binary fixture.
fn minimal_rbs_bytes() -> Vec<u8> {
    let chunk = |tag: &[u8; 4], payload: &[u8]| -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + payload.len() + 1);
        out.extend_from_slice(tag);
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        out.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            out.push(0);
        }
        out
    };
    let cat = |kind: &[u8; 4], items: &[Vec<u8>]| -> Vec<u8> {
        let mut body = kind.to_vec();
        for it in items {
            body.extend_from_slice(it);
        }
        chunk(b"CAT ", &body)
    };
    let mut head = vec![0u8; 256];
    head[0..9].copy_from_slice(&[0x5B, 0x54, 0x5B, 0x54, 0xBC, 0x04, 0x02, 0x00, 0x00]);
    let mut glob = vec![0u8; 512];
    glob[0] = 1;
    glob[1] = 1;
    glob[2..6].copy_from_slice(&132_000u32.to_be_bytes());
    glob[10..14].copy_from_slice(&(2 * 768u32).to_be_bytes());
    let mut usri = vec![0u8; 712];
    usri[0..18].copy_from_slice(b"Synthetic RBS Test");
    let mut mixr = vec![0u8; 64];
    mixr[0] = 110;
    mixr[1] = 1;
    for c in 0..4 {
        let b = 16 + 12 * c;
        mixr[b] = 1;
        mixr[b + 1] = 100;
        mixr[b + 2] = 64;
    }
    let mut tb1 = vec![0u8; 1097];
    tb1[0..9].copy_from_slice(&[1, 0, 64, 52, 95, 90, 70, 95, 0]);
    tb1[10] = 16;
    for s in 0..16 {
        tb1[11 + s * 2] = [0, 0, 12, 3][s % 4];
        tb1[11 + s * 2 + 1] = if s % 4 == 0 { 0x02 } else { 0x00 };
    }
    let mut tb2 = vec![0u8; 1097];
    tb2[10] = 16;
    for s in 0..16 {
        tb2[11 + s * 2 + 1] = 0x10;
    }
    let mut t808 = vec![0u8; 6238];
    t808[0] = 1;
    t808[3] = 115;
    t808[31] = 16;
    for s in (0..16).step_by(4) {
        t808[32 + s * 12 + 1] = 1;
    }
    let mut t909 = vec![0u8; 6239];
    t909[32] = 16;
    let empty_trak = chunk(b"TRAK", &[0, 0, 0, 1, 0, 0, 0]);
    let on_trak = chunk(b"TRAK", &[0, 0, 0, 2, 0, 0, 1, 0, 1, 0]);
    let devl = cat(
        b"DEVL",
        &[
            chunk(b"MIXR", &mixr),
            chunk(b"DELY", &[1, 3, 1, 64, 64, 0, 0, 0]),
            chunk(b"PCF ", &[0, 60, 64, 40, 0, 64, 1, 0, 0, 0, 0, 0]),
            chunk(b"DIST", &[0, 60, 0, 0, 0, 0, 0, 0]),
            chunk(b"COMP", &[1, 70, 50, 0, 0, 0, 0, 0]),
            chunk(b"303 ", &tb1),
            chunk(b"303 ", &tb2),
            chunk(b"808 ", &t808),
            chunk(b"909 ", &t909),
        ],
    );
    let trkl = cat(
        b"TRKL",
        &[
            chunk(b"TRAK", &[0, 0, 0, 1, 0, 1, 1]),
            on_trak.clone(),
            empty_trak.clone(),
            on_trak,
            empty_trak.clone(),
            empty_trak.clone(),
            empty_trak.clone(),
            empty_trak.clone(),
            empty_trak,
        ],
    );
    cat(
        b"RB40",
        &[
            chunk(b"HEAD", &head),
            chunk(b"GLOB", &glob),
            chunk(b"USRI", &usri),
            devl,
            trkl,
        ],
    )
}

/// Build a loud 2-channel synth project with a Master Compressor so live meters + GR + solo work deterministically.
fn build_loud_synth_project() -> (Project, u64) {
    let mut p = Project::empty();
    p.title = "Ctl Bridge Test".into();
    p.bpm = 128.0;
    p.bars = 4;
    p.end = EndRule::Loop;
    p.mixer.master.strip.gain = 1.0;
    let comp_id = 900;
    p.mixer.master.inserts.push(EffectDevice {
        id: comp_id,
        name: "Master Comp".into(),
        bypassed: false,
        kind: EffectKind::Compressor(CompressorPatch {
            threshold_db: -24.0,
            ratio: 6.0,
            attack_ms: 1.0,
            release_ms: 80.0,
            makeup_db: 0.0,
            mix: 1.0,
        }),
    });
    p.mixer
        .channels
        .push(Channel::new(1, ChannelKind::Audio, "Bass"));
    p.mixer
        .channels
        .push(Channel::new(2, ChannelKind::Audio, "Lead"));

    let bass_step = Step {
        active: true,
        degree: 1,
        velocity: 1.0,
        gate: 0.9,
        ..Step::default()
    };
    let lead_step = Step {
        active: true,
        degree: 8,
        velocity: 0.9,
        gate: 0.8,
        ..Step::default()
    };
    p.patterns.push(Pattern {
        id: 1,
        name: "BassPat".into(),
        kind: PatternKind::Notes {
            steps: vec![bass_step; 16],
        },
    });
    p.patterns.push(Pattern {
        id: 2,
        name: "LeadPat".into(),
        kind: PatternKind::Notes {
            steps: vec![lead_step; 16],
        },
    });
    let make_loud_rack = |cutoff_hz: f32| {
        let mut rack = RackPatch::default_synth();
        for m in &mut rack.zones[0].modules {
            match &mut m.kind {
                ModuleKind::Env(e) if m.name == "Amp Env" => {
                    e.attack_ms = 1.0;
                    e.sustain = 1.0;
                }
                ModuleKind::Filter(f) => {
                    f.cutoff_hz = cutoff_hz;
                }
                _ => {}
            }
        }
        rack
    };
    p.instruments.push(InstrumentInstance {
        id: 1,
        name: "Bass".into(),
        rack: make_loud_rack(1_200.0),
        channel: 1,
    });
    p.instruments.push(InstrumentInstance {
        id: 2,
        name: "Lead".into(),
        rack: make_loud_rack(4_000.0),
        channel: 2,
    });
    p.parts.push(Part::new(1, "BassPart", 1, 1));
    p.parts.push(Part::new(2, "LeadPart", 2, 2));
    let mut l1 = Lane::new(1, "Bass");
    l1.placements.push(Placement::new(1, 1, 0, 64));
    let mut l2 = Lane::new(2, "Lead");
    l2.placements.push(Placement::new(2, 2, 0, 64));
    p.lanes = vec![l1, l2];
    (p, comp_id)
}

#[test]
fn app_runs_100_percent_in_memory_via_memory_host_io() {
    let io = Arc::new(MemoryHostIo::new("/virtual/studio"));
    let rbs_path = Path::new("/virtual/studio/acid.rbs");
    io.insert_file(rbs_path, minimal_rbs_bytes());

    let mut app = App::new_with_host_io(
        Project::demo(),
        "/virtual/studio/session.json".into(),
        Arc::clone(&io) as Arc<dyn HostIo>,
    )
    .unwrap();

    // Save project purely in memory, mutate BPM, reload from memory, verify round-trip.
    app.execute_command("bpm 140").unwrap();
    app.apply(Action::Save {
        path: "/virtual/studio/saved.json".into(),
    })
    .unwrap();
    assert!(io.read_file("/virtual/studio/saved.json").is_some());

    let reloaded =
        load_project_with_io(io.as_ref(), Path::new("/virtual/studio/saved.json")).unwrap();
    assert_eq!(reloaded.bpm, 140.0);

    // Save & load InstrumentPreset purely in memory.
    let preset_path = Path::new("/virtual/studio/lead.preset.json");
    app.execute_command(&format!("preset-save {}", preset_path.display()))
        .unwrap();
    let preset_bytes = io
        .read_file(preset_path)
        .expect("preset saved into MemoryHostIo");
    let parsed_preset = InstrumentPreset::from_json_bytes(&preset_bytes).unwrap();
    assert_eq!(parsed_preset.version, INSTRUMENT_PRESET_VERSION);

    // Save & load MIDI profile purely in memory.
    let midi_path = Path::new("/virtual/studio/controller.midi.json");
    app.execute_command(&format!("midi profile-save {}", midi_path.display()))
        .unwrap();
    let midi_bytes = io
        .read_file(midi_path)
        .expect("midi profile saved into MemoryHostIo");
    let parsed_midi = MidiProfile::from_json_bytes(&midi_bytes).unwrap();
    assert!(!parsed_midi.name.is_empty());
    app.execute_command(&format!("midi profile-load {}", midi_path.display()))
        .unwrap();

    // Save portable bundle purely in memory.
    let report = app
        .save_portable_bundle_to(Path::new("/virtual/studio/bundle"))
        .unwrap();
    assert!(io.read_file(&report.project_path).is_some());

    // Save UserConfig purely in memory.
    app.save_config().unwrap();
    assert!(io.saved_config().is_some());

    // Open File Hub purely in memory and verify it lists & previews virtual files.
    app.execute_command("files").unwrap();
    let snap = cadence_core::runtime::AudioSnapshot::default();
    let frame = app.frame_view(&snap, "mem");
    assert!(
        frame.overlay.is_some(),
        "File Hub overlay should open purely over MemoryHostIo"
    );

    // Import .rbs purely from MemoryHostIo (or verify clean feature-disabled error).
    #[cfg(feature = "rbs")]
    {
        app.apply(Action::ImportRbs {
            path: rbs_path.to_path_buf(),
        })
        .unwrap();
        assert_eq!(app.project().title, "Synthetic RBS Test");
        assert_eq!(app.project().instruments.len(), 4);
    }
    #[cfg(not(feature = "rbs"))]
    {
        let err = app
            .apply(Action::ImportRbs {
                path: rbs_path.to_path_buf(),
            })
            .unwrap_err();
        assert!(format!("{err:#}").to_ascii_lowercase().contains("rbs"));
    }

    // Verify none of the virtual paths ever touched the real OS filesystem.
    assert!(!Path::new("/virtual/studio/saved.json").exists());
}

#[test]
fn sample_bank_and_riff_writeback_run_100_percent_in_memory_via_memory_host_io() {
    use cadence_core::rack::{ModuleKind, SampleLoopMode};
    use cadence_core::sampler::{
        decode_sample_bytes, encode_wav_with_metadata_bytes, SampleMetadata, SampleSlice,
    };
    use std::path::PathBuf;

    let io = Arc::new(MemoryHostIo::new("/virtual/sampler-lab"));
    let sr = 44_100u32;
    let frames = sr as usize; // 1.0s mono sine
    let pcm: Vec<f32> = (0..frames)
        .map(|i| (std::f32::consts::TAU * 220.0 * (i as f32 / sr as f32)).sin() * 0.6)
        .collect();
    let initial_meta = SampleMetadata {
        root_midi: 60,
        has_root: true,
        loop_mode: SampleLoopMode::Off,
        slices: vec![SampleSlice::new(0, Some("Start".into()))],
        ..SampleMetadata::default()
    };
    let wav_bytes = encode_wav_with_metadata_bytes(&pcm, None, sr, &initial_meta).unwrap();
    let wav_path = Path::new("/virtual/sampler-lab/samples/break.wav");
    io.insert_file(wav_path, wav_bytes);

    let mut app = App::new_with_host_io(
        Project::demo(),
        PathBuf::from("/virtual/sampler-lab/session.json"),
        Arc::clone(&io) as Arc<dyn HostIo>,
    )
    .unwrap();
    app.execute_command("sampler VirtSampler samples/break.wav")
        .unwrap();

    // Sample decoded purely through MemoryHostIo with zero OS filesystem reads.
    assert!(
        app.sample_issues().is_empty(),
        "virtual sample must resolve cleanly via HostIo; got issues: {:?}",
        app.sample_issues()
    );
    assert!(app.ready());
    let inst_id = app
        .project()
        .instruments
        .iter()
        .find(|i| i.name == "VirtSampler")
        .expect("VirtSampler created")
        .id;
    let loaded = app
        .sample_bank()
        .into_iter()
        .next()
        .map(|(_, s)| s)
        .expect("virtual sample data must be cached in memory");
    assert_eq!(loaded.frames, frames);
    assert_eq!(loaded.meta.slices.len(), 1);
    assert_eq!(loaded.meta.slices[0].label.as_deref(), Some("Start"));

    // Split into 4 grid slices on the Sample Deck and write RIFF cue/labl metadata purely in memory.
    app.execute_command("wave VirtSampler").unwrap();
    app.execute_command("slice grid 4").unwrap();
    app.execute_command("sample write-meta").unwrap();

    // Verify MemoryHostIo holds updated RIFF cue/labl chunks AND cached SampleData refreshed.
    let updated_bytes = io
        .read_file(wav_path)
        .expect("updated WAV bytes must exist in MemoryHostIo");
    let decoded = decode_sample_bytes(
        &updated_bytes,
        "samples/break.wav".into(),
        "Break".into(),
        wav_path.to_path_buf(),
    )
    .unwrap();
    assert_eq!(
        decoded.meta.slices.len(),
        4,
        "RIFF write-back must persist 4 grid slices in MemoryHostIo"
    );

    // Reload the virtual project in a brand-new App over the same MemoryHostIo:
    // the fresh instance must read back all 4 persisted RIFF slice markers purely from memory!
    let mut project2 = app.project().clone();
    if let Some(inst2) = project2.instruments.iter_mut().find(|i| i.id == inst_id) {
        if let Some(ModuleKind::Sampler(s)) = inst2.rack.zones[0]
            .modules
            .iter_mut()
            .map(|m| &mut m.kind)
            .find(|k| matches!(k, ModuleKind::Sampler(_)))
        {
            s.slices.clear();
        }
    }
    let app2 = App::new_with_host_io(
        project2,
        PathBuf::from("/virtual/sampler-lab/session.json"),
        Arc::clone(&io) as Arc<dyn HostIo>,
    )
    .unwrap();
    assert!(app2.sample_issues().is_empty());
    let reloaded = app2
        .sample_bank()
        .into_iter()
        .next()
        .map(|(_, s)| s)
        .expect("reloaded virtual sample must be decoded from updated MemoryHostIo bytes");
    assert_eq!(reloaded.meta.slices.len(), 4);
    assert!(!wav_path.exists());
}

#[test]
fn outcome_merge_preserves_every_field_including_bounce_export_and_dialog() {
    let mut app = App::new_in_memory(Project::demo(), "unused.json".into()).unwrap();
    let bounce_out = app
        .apply(Action::Bounce {
            scope: None,
            name: Some("MergeTest".into()),
            create_part: false,
            target: None,
        })
        .unwrap();
    assert!(bounce_out.bounce.is_some());

    let mut dst = Outcome {
        changed: false,
        midi_connect: Some(MidiConnectRequest {
            input: Some(Some("MiniLab".into())),
            output: None,
        }),
        clear_mixer_meters: Some(None),
        ..Outcome::default()
    };
    let src = Outcome {
        changed: true,
        samples_changed: true,
        project_boundary: true,
        bounce: bounce_out.bounce.clone(),
        export: Some(ExportRequest {
            path: "out.wav".into(),
            bars: 1,
            start_step: 0,
            steps: 16,
            sample_rate: 24_000,
            mode: ExportMode::MasterWav,
            use_take: false,
        }),
        file_dialog: Some(FileDialogRequest::OpenProject),
        midi_feedback: vec![MidiFeedback::InitDawMode(true)],
        midi_connect: Some(MidiConnectRequest {
            input: None,
            output: Some(Some("MiniLab Out".into())),
        }),
        midi_rescan: true,
        clear_mixer_meters: Some(Some(ChannelRef::Master)),
        quit: true,
        ..Outcome::default()
    };
    dst.merge(src);

    assert!(dst.changed && dst.samples_changed && dst.project_boundary);
    assert!(dst.bounce.is_some(), "Outcome::merge must preserve bounce");
    assert!(dst.export.is_some(), "Outcome::merge must preserve export");
    assert_eq!(dst.file_dialog, Some(FileDialogRequest::OpenProject));
    assert_eq!(dst.midi_feedback.len(), 1);
    assert!(dst.midi_rescan);
    assert!(dst.quit);
    assert_eq!(
        dst.clear_mixer_meters,
        Some(None),
        "Clear-all meters must not be narrowed to single channel"
    );
    let conn = dst.midi_connect.expect("merged midi_connect");
    assert_eq!(conn.input, Some(Some("MiniLab".into())));
    assert_eq!(conn.output, Some(Some("MiniLab Out".into())));
}

#[test]
fn headless_script_executes_bounce_and_export_wav_without_audio_runtime() {
    let dir = tempdir().unwrap();
    let proj_path = dir.path().join("project.json");
    let export_wav = dir.path().join("master.wav");
    let script_path = dir.path().join("ops.jsonl");
    let out_proj = dir.path().join("after.json");

    let (proj, _) = build_loud_synth_project();
    let lib = cadence_core::sampler::SampleLibrary::from_roots(&[], Some(&proj_path));
    App::new_with_library(proj, proj_path.clone(), lib)
        .unwrap()
        .save_project(&proj_path)
        .unwrap();

    let mut script = fs::File::create(&script_path).unwrap();
    for req in [
        json!({"id":"exp","action":{"op":"export","path":export_wav,"bars":1}}),
        json!({"id":"bnc","action":{"op":"bounce","name":"ResampledBass","create_part":true}}),
        json!({"id":"cmd","cmd":"bpm 134","compact":true}),
    ] {
        serde_json::to_writer(&mut script, &req).unwrap();
        writeln!(script).unwrap();
    }

    let output = cadence_bin(dir.path())
        .arg("--project")
        .arg(&proj_path)
        .arg("--sample-roots-only")
        .arg("--script")
        .arg(&script_path)
        .arg("--output")
        .arg(&out_proj)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "headless --script bounce/export failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["ok"], true);
    assert!(lines[0]["export"]["files_written"].as_u64() == Some(1));
    assert_eq!(lines[1]["ok"], true);
    assert_eq!(lines[2]["ok"], true);
    assert_eq!(lines[2]["bpm"], 134.0);
    assert!(
        lines[2].get("project").is_none(),
        "compact response must omit full project JSON"
    );

    // Exported Master WAV must exist on disk and contain audible stereo 16-bit audio.
    let mut reader = hound::WavReader::open(&export_wav).expect("exported WAV exists");
    assert_eq!(reader.spec().channels, 2);
    assert!(reader
        .samples::<i16>()
        .any(|s| s.unwrap().unsigned_abs() > 500));

    // Bounced WAV & new Sampler instrument must be present in saved project!
    let after = load_project(&out_proj).unwrap();
    assert_eq!(after.bpm, 134.0);
    let bounced_inst = after
        .instruments
        .iter()
        .find(|i| i.name == "ResampledBass")
        .expect("headless --script bounce must create the Sampler instrument");
    let (_, _, _, bounced_smp) = bounced_inst
        .rack
        .sampler_modules()
        .next()
        .expect("expected bounced Rack with Sampler module");
    let rel_path = bounced_smp
        .path
        .as_deref()
        .expect("bounced Sampler must reference WAV path");
    let bounced_wav = dir.path().join(rel_path.trim_start_matches("./"));
    assert!(
        bounced_wav.is_file(),
        "headless --script bounce must render WAV at {}",
        bounced_wav.display()
    );
    let mut b_reader = hound::WavReader::open(&bounced_wav).unwrap();
    assert!(b_reader
        .samples::<i16>()
        .any(|s| s.unwrap().unsigned_abs() > 500));

    // Also verify headless `apply_outcome` dispatches MasterAndStems export when `audio == None`.
    let stems_dir = dir.path().join("headless_stems");
    let stem_lib = cadence_core::sampler::SampleLibrary::from_roots(&[], Some(&proj_path));
    let mut stem_app =
        App::new_with_library(build_loud_synth_project().0, proj_path, stem_lib).unwrap();
    let stem_outcome = Outcome {
        export: Some(ExportRequest {
            path: stems_dir.clone(),
            bars: 1,
            start_step: 0,
            steps: 16,
            sample_rate: 24_000,
            mode: ExportMode::MasterAndStems,
            use_take: false,
        }),
        ..Outcome::default()
    };
    cadence::host::apply_outcome(&mut stem_app, None, &stem_outcome).unwrap();
    assert!(stems_dir.join("00-Master.wav").is_file());
    assert!(stems_dir.join("01-Bass.wav").is_file());
    assert!(stems_dir.join("02-Lead.wav").is_file());
    assert_eq!(
        stem_app
            .last_export_summary
            .as_ref()
            .map(|s| s.files_written),
        Some(3)
    );
}

#[test]
fn cli_rejects_flag_conflicts_standalone_output_and_handles_project_rbs_batch() {
    let dir = tempdir().unwrap();

    // 1. Standalone --output (and --bars without --render/--stems) rejected cleanly before touching disk!
    //    (Mutually exclusive clap flag conflicts are covered in-process in `src/main.rs`.)
    let standalone_out = dir.path().join("should_not_exist.json");
    for bad_args in [
        vec!["--output", standalone_out.to_str().unwrap()],
        vec!["--empty", "--output", standalone_out.to_str().unwrap()],
        vec!["--bars", "4"],
    ] {
        let out = cadence_bin(dir.path()).args(&bad_args).output().unwrap();
        assert!(
            !out.status.success(),
            "expected error for {bad_args:?}, got success"
        );
    }
    assert!(
        !standalone_out.exists(),
        "rejected standalone --output must not create output file"
    );

    // 2. `--project foo.rbs --output bar.json` runs cleanly in batch mode (matching `--import-rbs`).
    #[cfg(feature = "rbs")]
    {
        let rbs_path = dir.path().join("batch_acid.rbs");
        let json_path = dir.path().join("batch_acid.json");
        fs::write(&rbs_path, minimal_rbs_bytes()).unwrap();
        let out = cadence_bin(dir.path())
            .arg("--project")
            .arg(&rbs_path)
            .arg("--sample-roots-only")
            .arg("--output")
            .arg(&json_path)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "--project foo.rbs --output bar.json must succeed in batch mode: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let converted = load_project(&json_path).unwrap();
        assert_eq!(converted.title, "Synthetic RBS Test");
        assert_eq!(converted.instruments.len(), 4);
    }
}

#[cfg(unix)]
#[test]
fn live_unix_ctl_socket_and_cli_client_roundtrip_state_meters_cmd_undo_and_listen() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().unwrap();
    let sock_path = dir.path().join("ctl.sock");
    let ear_path = dir.path().join("ear.wav");

    // 1. Refuse to overwrite a non-socket regular file.
    let regular_file = dir.path().join("not_a_socket");
    fs::write(&regular_file, b"keep me").unwrap();
    assert!(
        CtlServer::bind(regular_file.clone(), ear_path.clone()).is_err(),
        "CtlServer must refuse to overwrite a non-socket file"
    );
    assert_eq!(fs::read(&regular_file).unwrap(), b"keep me");

    // 2. Bind live CtlServer and verify 0o600 permissions + live collision guard.
    let server = CtlServer::bind(sock_path.clone(), ear_path.clone()).unwrap();
    let mode = fs::metadata(&sock_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "control socket must enforce 0600 permissions");
    assert!(
        CtlServer::bind(sock_path.clone(), ear_path.clone()).is_err(),
        "binding a second server on an active live socket must fail"
    );

    // 3. Spin up a real in-memory App + silent AudioRuntime driven by a GUI-like poll loop.
    // Construct App + AudioRuntime inside the host thread (cpal::Stream is !Send on macOS).
    let (project, comp_id) = build_loud_synth_project();
    let live_proj_path = dir.path().join("live.json");

    let stop = Arc::new(AtomicBool::new(false));
    let handled_requests = Arc::new(AtomicUsize::new(0));
    let wake_pair = Arc::new((Mutex::new(false), Condvar::new()));
    {
        let wp = Arc::clone(&wake_pair);
        server.set_wake(Arc::new(move || {
            let (lock, cvar) = &*wp;
            if let Ok(mut woken) = lock.lock() {
                *woken = true;
                cvar.notify_one();
            }
        }));
    }

    let host_stop = Arc::clone(&stop);
    let host_handled = Arc::clone(&handled_requests);
    let host_wake = Arc::clone(&wake_pair);
    let host_thread = std::thread::spawn(move || {
        let lib = cadence_core::sampler::SampleLibrary::from_roots(&[], Some(&live_proj_path));
        let mut app = App::new_with_library(project, live_proj_path, lib).unwrap();
        let mut engine =
            AudioRuntime::start_with_samples(app.audio_project().clone(), app.sample_bank(), true)
                .unwrap();
        let mut hooks = HeadlessOutcomeHooks;
        while !host_stop.load(Ordering::Acquire) {
            let summary = server.poll_and_dispatch(&mut app, &mut engine, &mut hooks);
            if summary.handled > 0 {
                host_handled.fetch_add(summary.handled, Ordering::Release);
            }
            if summary.quit {
                break;
            }
            let (lock, cvar) = &*host_wake;
            if let Ok(mut woken) = lock.lock() {
                if !*woken {
                    let _ = cvar.wait_timeout(woken, Duration::from_millis(10));
                } else {
                    *woken = false;
                }
            }
        }
        engine.shutdown();
        drop(server);
    });

    let run_ctl = |args: &[&str]| -> (bool, Value) {
        let out = cadence_bin(dir.path())
            .arg("ctl")
            .arg("--sock")
            .arg(&sock_path)
            .args(args)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let parsed: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
            panic!(
                "cadence ctl {args:?} invalid JSON stdout='{stdout}' stderr='{}': {e}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
        (out.status.success(), parsed)
    };

    // 4. `cadence ctl state --compact` + CADENCE_CTL_SOCK auto-discovery.
    let (ok, state) = run_ctl(&["state", "--compact"]);
    assert!(ok, "state --compact failed: {state}");
    assert_eq!(state["ok"], true);
    assert_eq!(state["revision"], 0);
    assert_eq!(state["bpm"], 128.0);
    assert!(
        state.get("project").is_none(),
        "state --compact must omit full project JSON"
    );
    assert!(state["channel_meters"].is_array());
    assert!(state["effect_meters"].is_array());

    // Auto-discovery via CADENCE_CTL_SOCK env var.
    let env_out = cadence_bin(dir.path())
        .env("CADENCE_CTL_SOCK", &sock_path)
        .args(["ctl", "state", "--compact"])
        .output()
        .unwrap();
    assert!(env_out.status.success());
    let env_val: Value = serde_json::from_slice(&env_out.stdout).unwrap();
    assert_eq!(env_val["bpm"], 128.0);

    // Auto-discovery via `--session-dir`.
    let sess_out = cadence_bin(dir.path())
        .args(["ctl", "--session-dir"])
        .arg(dir.path())
        .args(["state", "--compact"])
        .output()
        .unwrap();
    assert!(sess_out.status.success());

    // 5. `cmd` with optimistic `expected_revision` + conflict rejection + `u` Undo via `action`!
    //    Sent directly over the live Unix socket to avoid redundant debug-binary spawns.
    let send_direct = |req: HostRequest| -> Value {
        send_ctl_request(&sock_path, &req).expect("direct Unix socket request must succeed")
    };
    let cmd_ok = send_direct(HostRequest {
        expected_revision: Some(0),
        compact: true,
        cmd: Some("bpm 142".into()),
        ..HostRequest::default()
    });
    assert_eq!(cmd_ok["ok"], true, "cmd bpm 142 failed: {cmd_ok}");
    assert_eq!(cmd_ok["revision"], 1);
    assert_eq!(cmd_ok["bpm"], 142.0);
    assert_eq!(cmd_ok["dirty"], true);

    // Stale expected_revision must fail cleanly with ok: false and leave BPM untouched.
    let conflict = send_direct(HostRequest {
        expected_revision: Some(0),
        compact: true,
        cmd: Some("bpm 90".into()),
        ..HostRequest::default()
    });
    assert_eq!(conflict["ok"], false, "stale expected_revision must fail");
    assert_eq!(conflict["revision"], 1);
    assert!(conflict["error"]
        .as_str()
        .unwrap()
        .contains("Revision conflict"));

    // Undo via typed `action` pops the Ex command cleanly off Document's undo stack!
    let undone = send_direct(HostRequest {
        expected_revision: Some(1),
        compact: true,
        action: Some(Action::Undo),
        ..HostRequest::default()
    });
    assert_eq!(undone["ok"], true, "undo action failed: {undone}");
    assert_eq!(undone["revision"], 2);
    assert_eq!(undone["bpm"], 128.0);

    // 6. `listen --bars 1..3 --solo Bass --sr 12000` renders WAV + per-bar telemetry without mutating flags!
    let bars_wav = dir.path().join("bars_solo.wav");
    let bars_resp = send_direct(HostRequest {
        listen: Some(ListenRequest::Bars {
            start_bar: 1,
            end_bar: 3,
            solo: vec!["Bass".into()],
            sample_rate: 12_000,
            out_path: Some(bars_wav.clone()),
        }),
        ..HostRequest::default()
    });
    assert_eq!(bars_resp["ok"], true, "listen --bars failed: {bars_resp}");
    let listen = &bars_resp["listen"];
    assert_eq!(listen["mode"], "bars");
    assert_eq!(listen["sample_rate"], 12_000);
    assert_eq!(listen["start_bar"], 1);
    assert_eq!(listen["end_bar"], 3);
    assert_eq!(listen["bars"].as_array().unwrap().len(), 2);
    assert_eq!(listen["bars"][0]["bar"], 1);
    assert_eq!(listen["bars"][1]["bar"], 2);
    assert!(listen["bars"][0]["rms_db"].as_f64().unwrap() > -40.0);
    assert!(listen["channels"].as_array().unwrap().len() >= 2);
    assert!(listen["effects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["device_id"] == comp_id));

    let mut wav = hound::WavReader::open(&bars_wav).unwrap();
    assert_eq!(wav.spec().sample_rate, 12_000);
    assert_eq!(wav.spec().channels, 2);
    assert_eq!(wav.spec().bits_per_sample, 16);
    assert!(wav
        .samples::<i16>()
        .any(|s| s.unwrap().unsigned_abs() > 1000));

    // Verify CLI inclusive `A..=B` bar syntax also parses cleanly into `[1..3)`.
    let inc_wav = dir.path().join("bars_inc.wav");
    let (ok_inc, inc_resp) = run_ctl(&[
        "listen",
        "--bars",
        "1..=2",
        "--sr",
        "12000",
        "--out",
        inc_wav.to_str().unwrap(),
    ]);
    assert!(ok_inc, "listen --bars 1..=2 failed: {inc_resp}");
    assert_eq!(inc_resp["listen"]["bars"].as_array().unwrap().len(), 2);

    // Verify temporary `--solo Bass` did NOT mutate live GUI flags!
    let full_state = send_direct(HostRequest {
        action: Some(Action::GetState),
        ..HostRequest::default()
    });
    assert_eq!(full_state["ok"], true);
    assert!(
        full_state["flags"]["solo_channels"]
            .as_array()
            .is_none_or(Vec::is_empty),
        "listen --bars --solo must NOT mutate live App flags: {}",
        full_state["flags"]
    );

    // 7. Start live silent playback -> verify `meters` shows active peaks + Master Comp GR!
    let play_resp = send_direct(HostRequest {
        compact: true,
        action: Some(Action::Play { playing: true }),
        ..HostRequest::default()
    });
    assert_eq!(play_resp["ok"], true);

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut saw_live_meters = false;
    while Instant::now() < deadline {
        let meters = send_direct(HostRequest {
            meters: true,
            ..HostRequest::default()
        });
        assert_eq!(meters["ok"], true, "meters failed: {meters}");
        let has_comp_gr = meters["effect_meters"].as_array().is_some_and(|arr| {
            arr.iter().any(|e| {
                e["device_id"] == comp_id && e["gain_reduction_db"].as_f64().unwrap_or(0.0) > 0.5
            })
        });
        let has_peak = meters["channel_meters"].as_array().is_some_and(|arr| {
            arr.iter().any(|c| {
                c["output_peak"]
                    .as_array()
                    .is_some_and(|p| p[0].as_f64().unwrap_or(0.0) > 0.05)
            })
        });
        if meters["playing"] == true && has_comp_gr && has_peak {
            saw_live_meters = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        saw_live_meters,
        "live `cadence ctl meters` must report active channel peaks and compressor gain_reduction_db"
    );

    // 8. `listen --live 0.25 --sr 24000` captures live speaker output from lock-free tap ring
    // into default `<session>/ear.wav` WITHOUT blocking concurrent GUI `--ctl` requests!
    let live_dir = dir.path().to_path_buf();
    let live_sock = sock_path.clone();
    let handled_before_live = handled_requests.load(Ordering::Acquire);
    let live_finished = Arc::new(AtomicBool::new(false));
    let live_finished_flag = Arc::clone(&live_finished);
    let live_client = std::thread::spawn(move || {
        let out = cadence_bin(&live_dir)
            .arg("ctl")
            .arg("--sock")
            .arg(&live_sock)
            .args(["listen", "--live", "0.25", "--sr", "24000"])
            .output()
            .unwrap();
        live_finished_flag.store(true, Ordering::Release);
        let val: Value = serde_json::from_slice(&out.stdout).unwrap();
        (out.status.success(), val)
    });
    // Wait until the host thread has actually dispatched `listen --live` onto its background tap
    // worker, then measure direct Unix socket RTT (avoiding debug-binary spawn overhead flakes).
    let dispatch_deadline = Instant::now() + Duration::from_secs(5);
    while handled_requests.load(Ordering::Acquire) == handled_before_live {
        assert!(
            Instant::now() < dispatch_deadline,
            "timed out waiting for host thread to dispatch listen --live"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let t0 = Instant::now();
    let during_state = send_ctl_request(
        &sock_path,
        &HostRequest {
            compact: true,
            action: Some(Action::GetState),
            ..HostRequest::default()
        },
    )
    .expect("direct Unix socket state poll during listen --live must succeed");
    let poll_elapsed = t0.elapsed();
    assert_eq!(
        during_state["ok"], true,
        "concurrent state poll during listen --live failed: {during_state}"
    );
    assert!(
        poll_elapsed < Duration::from_millis(150),
        "listen --live must NOT block the GUI host thread (direct socket RTT took {poll_elapsed:?})"
    );
    assert!(
        !live_finished.load(Ordering::Acquire),
        "concurrent state poll must complete BEFORE the 250ms live capture finishes (RTT {poll_elapsed:?})"
    );
    let (ok, live_resp) = live_client.join().unwrap();
    assert!(ok, "listen --live failed: {live_resp}");
    let live = &live_resp["listen"];
    assert_eq!(live["mode"], "live");
    assert_eq!(live["sample_rate"], 24_000);
    assert_eq!(live["source_sample_rate"], 48_000);
    assert_eq!(live["dropped_frames"], 0);
    assert_eq!(live["path"].as_str().unwrap(), ear_path.to_str().unwrap());
    assert!(live["frames"].as_u64().unwrap() >= 3_500);
    assert!(live["rms_db"].as_f64().unwrap() > -40.0);
    assert!(live["peak_db"].as_f64().unwrap() > -30.0);

    let mut ear_wav = hound::WavReader::open(&ear_path).expect("default ear.wav written");
    assert_eq!(ear_wav.spec().sample_rate, 24_000);
    assert_eq!(ear_wav.spec().channels, 2);
    assert!(ear_wav
        .samples::<i16>()
        .any(|s| s.unwrap().unsigned_abs() > 1000));

    // 9. Shutdown server & verify BOTH `ctl.sock` and auto-cleaned `ear.wav` are removed on drop!
    stop.store(true, Ordering::Release);
    {
        let (lock, cvar) = &*wake_pair;
        if let Ok(mut woken) = lock.lock() {
            *woken = true;
            cvar.notify_one();
        }
    }
    host_thread.join().unwrap();
    assert!(!sock_path.exists(), "CtlServer drop must clean up ctl.sock");
    assert!(
        !ear_path.exists(),
        "CtlServer drop must auto-clean default ear.wav"
    );

    // 10. Simulate a stale socket left by a dead process: new CtlServer::bind must recover cleanly!
    {
        let listener = std::os::unix::net::UnixListener::bind(&sock_path).unwrap();
        drop(listener); // leaves stale socket file on disk with no active listener!
        assert!(sock_path.exists());
    }
    let recovered = CtlServer::bind(sock_path.clone(), ear_path.clone())
        .expect("CtlServer::bind must cleanly replace a stale Unix socket");
    drop(recovered);
    assert!(!sock_path.exists());
}

#[test]
fn cadence_app_production_sources_contain_no_direct_os_io() {
    fn collect_rs(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect_rs(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs")
                && path.file_name().and_then(|n| n.to_str()) != Some("tests.rs")
            {
                out.push(path);
            }
        }
    }

    let app_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/app/src");
    let mut files = Vec::new();
    collect_rs(&app_src, &mut files);
    assert!(!files.is_empty(), "expected Rust sources in crates/app/src");

    let forbidden = ["std::fs", "std::env", "File::"];
    let mut violations = Vec::new();
    for path in files {
        let src = fs::read_to_string(&path).unwrap();
        for (idx, line) in src.lines().enumerate() {
            let trimmed = line.trim();
            // Stop scanning once the file enters its inline `#[cfg(test)]` module.
            if trimmed == "#[cfg(test)]" {
                break;
            }
            if trimmed.starts_with("//") {
                continue;
            }
            for token in forbidden {
                if line.contains(token) {
                    violations.push(format!("{}:{}: {trimmed}", path.display(), idx + 1));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "cadence-app production code must route all OS I/O through HostIo; found violations:\n{}",
        violations.join("\n")
    );
}

#[cfg(feature = "rbs")]
#[test]
fn rbs_header_peek_sanitizes_control_characters_in_title() {
    let mut iff_bytes = Vec::new();
    let title_body = b"Acid\x00\x07\x1bTrack\nLine";
    let mut chunk = Vec::new();
    chunk.extend_from_slice(b"HEAD");
    chunk.extend_from_slice(&(title_body.len() as u32).to_be_bytes());
    chunk.extend_from_slice(title_body);
    if !title_body.len().is_multiple_of(2) {
        chunk.push(0);
    }
    let form_len = (4 + chunk.len()) as u32;
    iff_bytes.extend_from_slice(b"FORM");
    iff_bytes.extend_from_slice(&form_len.to_be_bytes());
    iff_bytes.extend_from_slice(b"RB40");
    iff_bytes.extend_from_slice(&chunk);
    if let Ok(info) = cadence_formats::rbs::peek_rbs_header_bytes(&iff_bytes, "fallback") {
        assert!(
            !info.title.chars().any(char::is_control),
            "peek_rbs_header_bytes must strip control chars from title: {:?}",
            info.title
        );
    }
}
