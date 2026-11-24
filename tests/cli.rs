use cadence_app::{load_project, App};
use cadence_core::material::Project;
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};
use tempfile::tempdir;

fn exported_demo() -> Project {
    Project::demo()
}
fn save(project: &Project, path: &Path) {
    App::new(project.clone(), path.to_path_buf())
        .unwrap()
        .save_project(path)
        .unwrap();
}

fn cadence(directory: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cadence"));
    command.current_dir(directory).stdin(Stdio::null());
    command
}

fn write_script(path: &Path, requests: &[Value]) {
    let mut script = fs::File::create(path).unwrap();
    for request in requests {
        serde_json::to_writer(&mut script, request).unwrap();
        writeln!(script).unwrap();
    }
}

fn responses(output: &Output) -> Vec<Value> {
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn stdio(directory: &Path, input: Vec<u8>) -> Output {
    let mut child = cadence(directory)
        .args(["--stdio", "--silent"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    // Read responses while feeding large requests so neither pipe can deadlock.
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let output = child.wait_with_output().unwrap();
    writer.join().unwrap().unwrap();
    output
}

#[test]
fn script_conflicts_and_invalid_edits_preserve_the_last_valid_revision() {
    let directory = tempdir().unwrap();
    let script = directory.path().join("requests.jsonl");
    let saved = directory.path().join("recovered.json");
    write_script(
        &script,
        &[
            json!({"id":"tempo","expected_revision":0,"action":{"op":"edit","edit":{"op":"set_tempo","bpm":128.0}}}),
            json!({"id":"stale","expected_revision":0,"action":{"op":"edit","edit":{"op":"set_tempo","bpm":100.0}}}),
            json!({"id":"invalid","expected_revision":1,"action":{"op":"edit","edit":{"op":"set_bars","bars":0}}}),
            json!({"id":"state","expected_revision":1,"action":{"op":"get_state"}}),
            json!({"id":"save","expected_revision":1,"action":{"op":"save","path":saved}}),
        ],
    );
    let output = cadence(directory.path())
        .arg("--script")
        .arg(&script)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let lines = responses(&output);
    assert_eq!(lines.len(), 5);
    assert_eq!(lines[0]["revision"], 1);
    assert_eq!(lines[1]["id"], "stale");
    assert_eq!(lines[1]["ok"], false);
    assert!(lines[1]["error"]
        .as_str()
        .unwrap()
        .contains("Revision conflict"));
    assert_eq!(lines[2]["id"], "invalid");
    assert_eq!(lines[2]["ok"], false);
    assert_eq!(lines[2]["revision"], 1);
    assert_eq!(lines[3]["ok"], true);
    assert_eq!(lines[3]["revision"], 1);
    assert_eq!(lines[4]["ok"], true);

    let mut expected = Project::demo();
    expected.bpm = 128.0;
    let mut saved_expected = exported_demo();
    saved_expected.bpm = 128.0;
    assert_eq!(load_project(&saved).unwrap(), saved_expected);
    assert_eq!(
        serde_json::from_value::<Project>(lines[3]["project"].clone()).unwrap(),
        expected
    );
}

#[test]
fn script_save_load_and_output_round_trip_the_edited_score() {
    let directory = tempdir().unwrap();
    let original = directory.path().join("demo.json");
    let checkpoint = directory.path().join("checkpoint.json");
    let final_score = directory.path().join("final.json");
    let script = directory.path().join("requests.jsonl");
    let output = cadence(directory.path())
        .arg("--write-demo")
        .arg(&original)
        .output()
        .unwrap();
    assert_success(&output);
    assert_eq!(load_project(&original).unwrap(), exported_demo());
    write_script(
        &script,
        &[
            json!({"action":{"op":"edit","edit":{"op":"set_tempo","bpm":132.0}}}),
            json!({"action":{"op":"edit","edit":{"op":"rename_project","title":"Retained relationships"}}}),
            json!({"action":{"op":"save","path":checkpoint}}),
            json!({"action":{"op":"edit","edit":{"op":"set_tempo","bpm":99.0}}}),
            json!({"action":{"op":"load","path":checkpoint}}),
            json!({"expected_revision":4,"action":{"op":"get_state"}}),
        ],
    );
    let output = cadence(directory.path())
        .arg("--project")
        .arg(&original)
        .arg("--script")
        .arg(&script)
        .arg("--output")
        .arg(&final_score)
        .output()
        .unwrap();
    assert_success(&output);
    let lines = responses(&output);
    assert_eq!(lines.len(), 6);
    assert!(lines.iter().all(|line| line["ok"] == true));
    assert_eq!(lines[5]["revision"], 4);
    let mut expected = exported_demo();
    expected.bpm = 132.0;
    expected.title = "Retained relationships".into();
    assert_eq!(load_project(&checkpoint).unwrap(), expected);
    assert_eq!(load_project(&final_score).unwrap(), expected);
    assert_eq!(load_project(&original).unwrap(), exported_demo());
}

#[test]
fn offline_render_works_without_a_terminal_or_audio_device() {
    let directory = tempdir().unwrap();
    let project_path = directory.path().join("score.json");
    let wav_path = directory.path().join("score.wav");
    let mut project = exported_demo();
    project.bpm = 120.0;
    save(&project, &project_path);
    let output = cadence(directory.path())
        .arg("--project")
        .arg(&project_path)
        .arg("--render")
        .arg(&wav_path)
        .args(["--bars", "1", "--sample-rate", "8000", "--check"])
        .output()
        .unwrap();
    assert_success(&output);
    let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["valid"], true);
    assert_eq!(summary["bpm"], 120.0);
    let mut wav = hound::WavReader::open(&wav_path).unwrap();
    assert_eq!(wav.spec().sample_rate, 8_000);
    assert_eq!(wav.spec().channels, 2);
    assert_eq!(wav.spec().bits_per_sample, 16);
    assert_eq!(wav.duration(), 16_000);
    assert!(wav.samples::<i16>().any(|sample| sample.unwrap() != 0));
}

#[test]
fn stdio_play_and_pause_report_live_transport_without_changing_the_score() {
    use std::{
        io::{BufRead, BufReader},
        sync::mpsc,
        time::{Duration, Instant},
    };
    struct EngineProcess(std::process::Child);
    impl Drop for EngineProcess {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let directory = tempdir().unwrap();
    let mut child = EngineProcess(
        cadence(directory.path())
            .args(["--stdio", "--silent"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut input = child.0.stdin.take().unwrap();
    let output = child.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            let response: Value = serde_json::from_str(&line.unwrap()).unwrap();
            if tx.send(response).is_err() {
                break;
            }
        }
    });
    let mut request = |action: Value| {
        serde_json::to_writer(&mut input, &json!({"id":"transport-check","action":action}))
            .unwrap();
        writeln!(input).unwrap();
        input.flush().unwrap();
        let response = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("engine response deadline");
        assert_eq!(response["id"], "transport-check");
        assert_eq!(response["ok"], true);
        assert_eq!(response["revision"], 0);
        assert_eq!(
            serde_json::from_value::<Project>(response["project"].clone()).unwrap(),
            Project::demo()
        );
        response
    };
    for playing in [true, false] {
        let accepted = request(json!({"op":"play","playing":playing}));
        assert!(accepted.get("audio_error").is_none());
        // Transport is ordered at the audio boundary. Acceptance may precede
        // observation; wait for the actual snapshot rather than assuming an
        // immediate write to the engine's playing flag.
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let response = request(json!({"op":"get_state"}));
            if response["transport"]["playing"] == playing
                && response["transport"]["pending"] == false
            {
                assert!(response["transport"]["absolute_step"].is_u64());
                break;
            }
            assert!(
                Instant::now() < deadline,
                "transport adoption deadline: {response}"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            request(json!({"op":"get_state"}))["transport"]["playing"],
            playing
        );
    }
    drop(input);
    assert!(child.0.wait().unwrap().success());
    reader.join().unwrap();
}

#[test]
fn stdio_recovers_after_oversized_and_invalid_lines_and_accepts_eof() {
    let directory = tempdir().unwrap();
    let mut input =
        serde_json::to_vec(&json!({"id":"at-limit","action":{"op":"get_state"}})).unwrap();
    input.resize(65_536, b' ');
    input.push(b'\n');
    input.extend(std::iter::repeat_n(b' ', 2 * 1024 * 1024));
    input.extend_from_slice(b"\n\xff\n \t\r\n");
    input.extend_from_slice(
        &serde_json::to_vec(&json!({"id":"after","action":{"op":"get_state"}})).unwrap(),
    );
    let output = stdio(directory.path(), input);
    assert_eq!(output.status.code(), Some(1));
    let lines = responses(&output);
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0]["id"], "at-limit");
    assert_eq!(lines[0]["ok"], true);
    assert_eq!(lines[1]["ok"], false);
    assert_eq!(lines[1]["error"], "JSONL request exceeds 64 KiB");
    assert_eq!(lines[2]["ok"], false);
    assert_eq!(lines[3]["id"], "after");
    assert_eq!(lines[3]["ok"], true);
    assert_eq!(lines[3]["revision"], 0);
    assert_eq!(lines[3]["transport"]["playing"], false);

    // A final oversized line must still report its error without an LF terminator.
    let output = stdio(directory.path(), vec![b' '; 65_537]);
    assert_eq!(output.status.code(), Some(1));
    let lines = responses(&output);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["error"], "JSONL request exceeds 64 KiB");
}

#[test]
fn conflicting_modes_are_rejected_before_any_file_is_written() {
    let directory = tempdir().unwrap();
    let output = cadence(directory.path())
        .args(["--stdio", "--check"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
    for arguments in [
        vec!["--stdio", "--output", "ignored.json"],
        vec!["--write-demo", "ignored.json", "--check"],
        vec!["--write-demo", "ignored.json", "--script", "missing.jsonl"],
        vec!["--write-demo", "ignored.json", "--render", "ignored.wav"],
    ] {
        let output = cadence(directory.path()).args(arguments).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
    }
    assert!(!directory.path().join("ignored.json").exists());
    assert!(!directory.path().join("ignored.wav").exists());
}

#[test]
fn key_scripts_build_real_material_and_reject_failed_expectations() {
    let directory = tempdir().unwrap();
    let script = directory.path().join("keys.jsonl");
    let score = directory.path().join("score.json");
    write_script(
        &script,
        &[
            json!({"keys":":pattern notes Motif 0 2 4 7 6<Enter>","expect":{"patterns":1,"instruments":0}}),
            json!({"keys":":pattern degrees Harmony 64 0:0 24:-2 32:2 56:-1<Enter>:derive Moving add Motif Harmony<Enter>","expect":{"patterns":3}}),
            json!({"keys":":instrument Lead reese<Enter>:part Part Moving Lead<Enter>:lane Sequence<Enter>:place Part Sequence 1 4 song<Enter>","expect":{"parts":1,"placements":1,"ready":true}}),
        ],
    );
    let output = cadence(directory.path())
        .arg("--empty")
        .arg("--key-script")
        .arg(&script)
        .arg("--output")
        .arg(&score)
        .output()
        .unwrap();
    assert_success(&output);
    let project = load_project(&score).unwrap();
    assert_eq!(project.pattern_period(project.patterns[2].id).unwrap(), 320);
    assert_eq!(project.instruments.len(), 1);
    assert_eq!(project.compile().unwrap().bars() * 16, project.bars * 16);
    write_script(
        &script,
        &[json!({"keys":":pattern notes A 0<Enter>","expect":{"patterns":99}})],
    );
    let output = cadence(directory.path())
        .arg("--empty")
        .arg("--key-script")
        .arg(&script)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("expected patterns=99"));
}

#[test]
fn unknown_actions_and_fields_do_not_mutate_state() {
    let directory = tempdir().unwrap();
    let script = directory.path().join("invalid.jsonl");
    write_script(
        &script,
        &[
            json!({"action":{"op":"unknown_operation","track":0}}),
            json!({"action":{"op":"create_lane","name":"Nope","unexpected":true}}),
            json!({"action":{"op":"get_state"}}),
        ],
    );
    let output = cadence(directory.path())
        .arg("--empty")
        .arg("--script")
        .arg(&script)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let lines = responses(&output);
    assert_eq!(lines[0]["ok"], false);
    assert_eq!(lines[1]["ok"], false);
    assert_eq!(lines[2]["revision"], 0);
    assert_eq!(lines[2]["project"]["lanes"], json!([]));
}

#[test]
fn documented_modal_journeys_replay_with_an_isolated_sample_catalog() {
    let directory = tempdir().unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for script in ["authoring.keys.jsonl", "sounds.keys.jsonl"] {
        let output = directory.path().join(format!("{script}.json"));
        let result = cadence(directory.path())
            .args(["--empty", "--sample-roots-only", "--sample-dir"])
            .arg(root.join("examples/demo/samples"))
            .arg("--key-script")
            .arg(root.join("examples/material-lab").join(script))
            .arg("--output")
            .arg(&output)
            .output()
            .unwrap();
        assert_success(&result);
        let project = load_project(&output).unwrap();
        assert_eq!(project.instruments.len(), 1);
        assert_eq!(project.parts.len(), 1);
        assert_eq!(project.lanes[0].placements.len(), 1);
        assert!(project.compile().is_ok());
        let reopened = cadence(directory.path())
            .arg("--project")
            .arg(&output)
            .args(["--sample-roots-only", "--sample-dir"])
            .arg(root.join("examples/demo/samples"))
            .arg("--check")
            .output()
            .unwrap();
        assert_success(&reopened);
        assert_eq!(responses(&reopened)[0]["ready"], true);
    }
}

#[test]
fn empty_project_resolves_relative_samples_at_its_save_origin() {
    let directory = tempdir().unwrap();
    let samples = directory.path().join("samples");
    fs::create_dir(&samples).unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/demo/samples/cyber-hat.wav"),
        samples.join("local.wav"),
    )
    .unwrap();
    let script = directory.path().join("relative.jsonl");
    write_script(
        &script,
        &[json!({"keys":":sampler Hat ./samples/local.wav<Enter>","expect":{"ready":true}})],
    );
    let result = cadence(directory.path())
        .args(["--empty", "--sample-roots-only", "--key-script"])
        .arg(&script)
        .arg("--check")
        .output()
        .unwrap();
    assert_success(&result);
    assert_eq!(responses(&result).last().unwrap()["ready"], true);
}

#[test]
fn evaluated_performance_session_preserves_the_authored_project_after_undo() {
    let folder = tempdir().unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = root.join("examples/material-lab/project.json");
    let output = folder.path().join("inspected.json");
    let result = cadence(folder.path())
        .arg("--project")
        .arg(&source)
        .arg("--sample-roots-only")
        .arg("--key-script")
        .arg(root.join("examples/material-lab/performance.keys.jsonl"))
        .arg("--output")
        .arg(&output)
        .arg("--check")
        .output()
        .unwrap();
    assert_success(&result);
    assert_eq!(
        load_project(&source).unwrap(),
        load_project(&output).unwrap()
    );
    assert_eq!(responses(&result).last().unwrap()["ready"], true);
}

#[test]
fn stdio_idle_loop_lands_pending_launch_without_additional_stdin() {
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    struct EngineGuard(std::process::Child);
    impl Drop for EngineGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let directory = tempdir().unwrap();
    // Use a fast tempo (300 BPM -> 1 bar = 800 ms) so a pending bar-quantized launch
    // lands rapidly while stdin is completely idle.
    let mut child = EngineGuard(
        cadence(directory.path())
            .args(["--stdio", "--silent"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut input = child.0.stdin.take().unwrap();
    let output = child.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            let response: Value = serde_json::from_str(&line.unwrap()).unwrap();
            if tx.send(response).is_err() {
                break;
            }
        }
    });
    let mut send = |action: Value| -> Value {
        serde_json::to_writer(&mut input, &json!({"action": action})).unwrap();
        writeln!(input).unwrap();
        input.flush().unwrap();
        let response = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("stdio response");
        assert_eq!(response["ok"], true, "unexpected error: {response}");
        response
    };

    send(json!({"op":"edit","edit":{"op":"set_tempo","bpm":300.0}}));
    send(json!({"op":"play","playing":true}));
    // Wait until playback has started (next_step > 0) so a bar-quantized launch is pending.
    let start_deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let state = send(json!({"op":"get_state"}));
        if state["transport"]["playing"] == true
            && state["transport"]["next_step"].as_u64().unwrap_or(0) >= 2
        {
            break;
        }
        assert!(
            Instant::now() < start_deadline,
            "engine never started: {state}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let queued = send(json!({
        "op": "launch",
        "lane": 1,
        "part": {"part": 1, "phase": "song", "offset_steps": 0}
    }));
    assert_eq!(
        queued["live"]["pending"].as_array().map(Vec::len),
        Some(1),
        "launch should be pending on the next bar: {queued}"
    );

    // Stay completely idle on stdin for > 1 bar at 300 BPM (800 ms) so the stdio
    // idle tick loop observes the transport and lands the launch by itself!
    std::thread::sleep(Duration::from_millis(1_100));

    let after = send(json!({"op":"get_state"}));
    assert_eq!(
        after["live"]["pending"].as_array().map(Vec::len),
        Some(0),
        "idle stdio loop must have landed the pending launch: {after}"
    );
    assert_eq!(
        after["live"]["active"].as_array().map(Vec::len),
        Some(1),
        "launched override must be active after idle wait: {after}"
    );

    drop(input);
    assert!(child.0.wait().unwrap().success());
    reader.join().unwrap();
}

#[cfg(feature = "rbs")]
#[test]
fn cli_imports_rebirth_rbs_checks_saves_and_renders_wav() {
    let dir = tempdir().unwrap();
    let rbs_path = dir.path().join("acid_fixture.rbs");
    let json_path = dir.path().join("acid_fixture.json");
    let wav_path = dir.path().join("acid_fixture.wav");

    // Build a minimal valid RBS 4.2 IFF song (136 BPM, 2 bars, 303#1 + 808 + Delay + Master Comp).
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
    glob[0] = 1; // song mode
    glob[1] = 1; // loop on
    glob[2..6].copy_from_slice(&136_000u32.to_be_bytes());
    glob[10..14].copy_from_slice(&(2 * 768u32).to_be_bytes());
    let mut usri = vec![0u8; 712];
    usri[0..13].copy_from_slice(b"CLI Acid Test");
    let mut mixr = vec![0u8; 64];
    mixr[0] = 110;
    mixr[1] = 1;
    for c in 0..4 {
        let b = 16 + 12 * c;
        mixr[b] = 1;
        mixr[b + 1] = 105;
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
        t808[32 + s * 12 + 1] = 1; // BD
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
    let rbs = cat(
        b"RB40",
        &[
            chunk(b"HEAD", &head),
            chunk(b"GLOB", &glob),
            chunk(b"USRI", &usri),
            devl,
            trkl,
        ],
    );
    fs::write(&rbs_path, &rbs).unwrap();

    let output = cadence(dir.path())
        .arg("--import-rbs")
        .arg(&rbs_path)
        .arg("--check")
        .arg("--output")
        .arg(&json_path)
        .arg("--render")
        .arg(&wav_path)
        .args(["--bars", "2", "--sample-rate", "24000"])
        .output()
        .unwrap();
    assert_success(&output);
    let summary: Value = serde_json::from_str(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(summary["valid"], true);
    assert_eq!(summary["ready"], true);
    assert_eq!(summary["bars"], 2);
    assert_eq!(summary["instruments"], 4);

    // Converted JSON must reopen cleanly and rendered WAV must be non-empty audio.
    let reopened = load_project(&json_path).unwrap();
    assert_eq!(reopened.instruments.len(), 4);
    let mut reader = hound::WavReader::open(&wav_path).unwrap();
    let peak = reader
        .samples::<i16>()
        .map(|s| s.unwrap().unsigned_abs())
        .max()
        .unwrap_or(0);
    assert!(
        peak > 1000,
        "rendered RBS WAV must be clearly audible (peak={peak})"
    );
}

#[test]
fn cli_empty_honors_user_config_default_bpm_and_root_midi() {
    use cadence_core::{config::UserConfig, material::Project};

    let dir = tempdir().unwrap();
    let cfg_home = dir.path().join("xdg");
    let cfg_path = cfg_home.join("cadence").join("config.json");
    let custom_cfg = UserConfig {
        default_bpm: 118.0,
        default_root_midi: 65, // F4
        ..UserConfig::default()
    };
    custom_cfg.save_to(&cfg_path).unwrap();
    let empty_proj = Project::empty_with_config(&custom_cfg);
    assert_eq!(empty_proj.bpm, 118.0);
    assert_eq!(empty_proj.root_midi, 65);

    let out_json = dir.path().join("cli-empty.json");
    let output = cadence(dir.path())
        .env("XDG_CONFIG_HOME", &cfg_home)
        .args(["--empty", "--check", "--output"])
        .arg(&out_json)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "cadence --empty failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let loaded = load_project(&out_json).unwrap();
    assert_eq!(loaded.bpm, 118.0);
    assert_eq!(loaded.root_midi, 65);
}

#[test]
fn cli_renders_take_stems_and_atomic_render_report() {
    use cadence_core::mixer::LiveFlags;
    use cadence_core::take::TakeJournal;

    let dir = tempdir().unwrap();
    let project = exported_demo();
    let project_path = dir.path().join("song.json");
    save(&project, &project_path);

    // 1. `--render` + `--render-report` into a nested non-existent directory writes atomically.
    let wav_path = dir.path().join("exports/mix.wav");
    let report_path = dir.path().join("reports/nested/mix-report.json");
    let render_out = cadence(dir.path())
        .arg("--project")
        .arg(&project_path)
        .arg("--render")
        .arg(&wav_path)
        .arg("--render-report")
        .arg(&report_path)
        .args(["--bars", "1", "--sample-rate", "8000"])
        .output()
        .unwrap();
    assert_success(&render_out);
    assert!(wav_path.is_file());
    let report_raw = fs::read(&report_path).unwrap();
    assert!(
        report_raw.ends_with(b"\n"),
        "atomic render report should end with newline"
    );
    let report_json: Value = serde_json::from_slice(&report_raw).unwrap();
    assert_eq!(report_json["bars"], 1);
    assert_eq!(report_json["sample_rate"], 8_000);
    assert!(report_json["frames"].as_u64().unwrap() > 0);
    assert!(report_json["channels"].is_array());

    // 2. `--render --take` + `--render-report` renders a recorded take journal cleanly.
    let take_path = dir.path().join("live.take.jsonl");
    let seg = cadence_core::timeline::Segment::new(0, 0, None, project.end);
    let mut journal = TakeJournal::open(
        Some(take_path.clone()),
        &project,
        &LiveFlags::default(),
        &[],
        0,
        Some(seg),
    )
    .unwrap();
    journal.advance_to(16);
    journal.record_flags(16, LiveFlags::default(), "mark");
    journal.seal(32).unwrap().unwrap();

    let take_wav = dir.path().join("exports/take.wav");
    let take_report = dir.path().join("reports/take-report.json");
    let take_out = cadence(dir.path())
        .arg("--project")
        .arg(&project_path)
        .arg("--render")
        .arg(&take_wav)
        .arg("--take")
        .arg(&take_path)
        .args(["--take-tail-bars", "1", "--sample-rate", "8000"])
        .arg("--render-report")
        .arg(&take_report)
        .output()
        .unwrap();
    assert_success(&take_out);
    assert!(take_wav.is_file());
    let take_report_json: Value = serde_json::from_slice(&fs::read(&take_report).unwrap()).unwrap();
    assert_eq!(take_report_json["bars"], 3);
    assert_eq!(take_report_json["sample_rate"], 8_000);
    let mut take_reader = hound::WavReader::open(&take_wav).unwrap();
    assert!(take_reader.samples::<i16>().any(|s| s.unwrap() != 0));

    // 3. `--stems` exports `00-Master.wav` plus isolated per-channel stereo WAV stems.
    let stems_dir = dir.path().join("stems-out");
    let stems_out = cadence(dir.path())
        .arg("--project")
        .arg(&project_path)
        .arg("--stems")
        .arg(&stems_dir)
        .args(["--bars", "1", "--sample-rate", "8000"])
        .output()
        .unwrap();
    assert_success(&stems_out);
    let master_wav = stems_dir.join("00-Master.wav");
    assert!(master_wav.is_file());
    let wav_files: Vec<_> = fs::read_dir(&stems_dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "wav"))
        .collect();
    assert!(
        wav_files.len() >= 2,
        "expected Master plus at least one Audio channel stem, got {wav_files:?}"
    );
    let mut master_reader = hound::WavReader::open(&master_wav).unwrap();
    assert_eq!(master_reader.spec().channels, 2);
    assert_eq!(master_reader.spec().sample_rate, 8_000);
    assert!(master_reader.samples::<i16>().any(|s| s.unwrap() != 0));
}

#[test]
fn cli_ctl_reports_clean_errors_for_missing_socket_and_conflicting_session_flags() {
    let dir = tempdir().unwrap();

    // Missing Unix socket path exits non-zero with a clean connection error.
    let missing_sock = dir.path().join("missing.sock");
    let missing = cadence(dir.path())
        .args(["ctl", "--sock"])
        .arg(&missing_sock)
        .args(["state", "--compact"])
        .output()
        .unwrap();
    assert!(!missing.status.success());
    let stderr = String::from_utf8_lossy(&missing.stderr);
    assert!(
        stderr.contains("Connecting to Cadence control socket")
            || stderr.contains("only supported on Unix"),
        "expected clean ctl socket error, got: {stderr}"
    );

    // Combining top-level session flags with `cadence ctl` is rejected cleanly.
    let conflict = cadence(dir.path())
        .args(["--empty", "ctl", "--sock"])
        .arg(&missing_sock)
        .args(["state", "--compact"])
        .output()
        .unwrap();
    assert!(!conflict.status.success());
    let conflict_stderr = String::from_utf8_lossy(&conflict.stderr);
    assert!(
        conflict_stderr.contains("Top-level session flags cannot be combined"),
        "expected conflict error, got: {conflict_stderr}"
    );
}

#[test]
fn cli_preserves_corrupt_user_config_and_warns_on_stderr() {
    let dir = tempdir().unwrap();
    let corrupt_cfg = dir.path().join("corrupt-config.json");
    let raw_corrupt = b"{ not valid json !!! }";
    fs::write(&corrupt_cfg, raw_corrupt).unwrap();

    let out = cadence(dir.path())
        .env("CADENCE_CONFIG_PATH", &corrupt_cfg)
        .args(["--empty", "--check"])
        .output()
        .unwrap();
    assert_success(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("could not be loaded") && stderr.contains("without overwriting"),
        "expected corrupt config warning on stderr, got: {stderr}"
    );
    assert_eq!(
        fs::read(&corrupt_cfg).unwrap(),
        raw_corrupt,
        "corrupt config.json must remain untouched on disk"
    );
}
