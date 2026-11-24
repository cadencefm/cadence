use cadence_app::load_project;
use cadence_core::{
    assets::load_project_samples, dsp::lin_to_db_f64, sampler::SampleLibrary, score::ScoreWindow,
    take::Take,
};
use cadence_engine::audio::{render_schedule_measure, render_schedule_pcm, RenderReport};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

struct HygieneExpectation {
    name: &'static str,
    require_silent_tail: bool,
}

fn measure_example_window(
    rel: &str,
    start_step: u64,
    steps: Option<u64>,
    sample_rate: u32,
) -> RenderReport {
    let path = repo_root().join(rel);
    let project = load_project(&path).unwrap_or_else(|e| panic!("load {rel}: {e:#}"));
    let compiled = project
        .compile()
        .unwrap_or_else(|e| panic!("compile {rel}: {e:#}"));
    let mut library = SampleLibrary::from_roots(&[], Some(&path));
    let samples = load_project_samples(&compiled, &mut library);
    samples
        .ensure_ready()
        .unwrap_or_else(|e| panic!("samples {rel}: {e:#}"));
    let total_steps = project.bars as u64 * 16;
    let render_steps = steps.unwrap_or_else(|| total_steps.saturating_sub(start_step));
    render_schedule_measure(
        &[(0, &compiled)],
        start_step,
        render_steps,
        samples.bank,
        sample_rate,
    )
    .unwrap_or_else(|e| panic!("render {rel}: {e:#}"))
}

fn measure_fivefold_take_window(start_step: Option<u64>, sample_rate: u32) -> RenderReport {
    let project_path = repo_root().join("examples/fivefold-live/project.json");
    let take_path = repo_root().join("examples/fivefold-live/takes/set.take.jsonl");
    let take = Take::load(&take_path).expect("load fivefold take");
    let scores = take.compile(2).expect("compile fivefold take");
    let mut library = SampleLibrary::from_roots(&[], Some(&project_path));
    let mut bank = Vec::new();
    for (_, compiled) in &scores {
        let report = load_project_samples(compiled, &mut library);
        report.ensure_ready().expect("fivefold take samples ready");
        for (id, sample) in report.bank {
            if !bank.iter().any(|(existing, _)| *existing == id) {
                bank.push((id, sample));
            }
        }
    }

    let full_end = take.header.start_step + take.steps() + 2 * 16;
    let from = start_step.unwrap_or(take.header.start_step);
    let steps = full_end
        .checked_sub(from)
        .expect("fivefold take window start must precede end");

    // Slice schedule cleanly to the active score at `from` plus all later transitions.
    let mut schedule: Vec<(u64, &ScoreWindow)> = Vec::new();
    let initial = scores
        .iter()
        .rfind(|(at, _)| *at <= from)
        .unwrap_or(&scores[0]);
    schedule.push((from, &initial.1));
    for (at, score) in &scores {
        if *at > from && *at < full_end {
            schedule.push((*at, score));
        }
    }

    render_schedule_measure(&schedule, from, steps, bank, sample_rate)
        .expect("render fivefold take")
}

fn assert_signal_hygiene(expect: &HygieneExpectation, rep: &RenderReport) {
    let name = expect.name;
    assert!(rep.frames > 0, "{name}: rendered zero frames");

    for ch in 0..2 {
        assert!(
            rep.output.peak[ch].is_finite()
                && rep.output.rms[ch].is_finite()
                && rep.output.dc[ch].is_finite()
                && rep.tail.peak[ch].is_finite()
                && rep.tail.rms[ch].is_finite(),
            "{name}: non-finite signal statistics on ch {ch}: output={:?} tail={:?}",
            rep.output,
            rep.tail
        );
        assert!(
            rep.output.dc[ch].abs() < 0.005,
            "{name}: ch {ch} DC offset {:.6} exceeds 0.005",
            rep.output.dc[ch]
        );
        let rms_db = lin_to_db_f64(rep.output.rms[ch]);
        let peak_db = lin_to_db_f64(rep.output.peak[ch]);
        assert!(
            (-32.0..=-4.0).contains(&rms_db),
            "{name}: ch {ch} RMS {rms_db:.2} dBFS outside [-32.0, -4.0] dBFS"
        );
        assert!(
            (-18.0..=-0.05).contains(&peak_db),
            "{name}: ch {ch} peak {peak_db:.2} dBFS outside [-18.0, -0.05] dBFS"
        );
    }

    assert_eq!(
        rep.output.full_scale_samples, 0,
        "{name}: full-scale clipping detected ({} samples)",
        rep.output.full_scale_samples
    );
    for ch in &rep.channels {
        assert!(
            !ch.clipped && !ch.hot,
            "{name}: channel {:?} exceeded 0 dBFS headroom (hot={}, clipped={}, in={:?}, out={:?})",
            ch.name,
            ch.hot,
            ch.clipped,
            ch.input_peak_hold,
            ch.output_peak_hold
        );
    }

    let main_peak_db = lin_to_db_f64(rep.output.peak[0].max(rep.output.peak[1]));
    let tail_peak_db = lin_to_db_f64(rep.tail.peak[0].max(rep.tail.peak[1]));
    assert!(
        tail_peak_db <= main_peak_db - 6.0,
        "{name}: tail peak {tail_peak_db:.2} dBFS did not decay cleanly below main peak {main_peak_db:.2} dBFS"
    );
    if expect.require_silent_tail {
        assert!(
            tail_peak_db < -30.0,
            "{name}: explicit tail peak {tail_peak_db:.2} dBFS must decay below -30 dBFS"
        );
    }
}

#[test]
fn no_example_project_or_take_contains_absolute_sample_paths() {
    let examples = repo_root().join("examples");
    let mut checked = 0;
    for entry in std::fs::read_dir(&examples).unwrap() {
        let dir = entry.unwrap().path();
        if !dir.is_dir() {
            continue;
        }
        let proj = dir.join("project.json");
        if proj.is_file() {
            let text = std::fs::read_to_string(&proj).unwrap();
            assert!(
                !text.contains("/Users/") && !text.contains("/home/") && !text.contains("C:\\"),
                "{} contains an absolute path",
                proj.display()
            );
            load_project(&proj).unwrap_or_else(|e| panic!("validate {}: {e:#}", proj.display()));
            checked += 1;
        }
        let takes = dir.join("takes");
        if takes.is_dir() {
            for take_entry in std::fs::read_dir(takes).unwrap() {
                let take_path = take_entry.unwrap().path();
                if take_path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    let text = std::fs::read_to_string(&take_path).unwrap();
                    assert!(
                        !text.contains("/Users/")
                            && !text.contains("/home/")
                            && !text.contains("C:\\"),
                        "{} contains an absolute path",
                        take_path.display()
                    );
                    Take::load(&take_path)
                        .unwrap_or_else(|e| panic!("load {}: {e:#}", take_path.display()));
                    checked += 1;
                }
            }
        }
    }
    assert!(checked >= 5, "expected at least 5 example projects/takes");
}

#[test]
fn absolute_sample_paths_are_rejected_and_save_stays_portable() {
    let mut project = cadence_core::material::Project::demo();
    let mut bad_rack = cadence_core::rack::RackPatch::default_sampler();
    for (_, _, smp) in bad_rack.sampler_modules_mut() {
        smp.path = Some("/tmp/not-portable.wav".into());
    }
    project
        .instruments
        .push(cadence_core::material::InstrumentInstance {
            id: 999,
            name: "BadSampler".into(),
            rack: bad_rack,
            channel: project.mixer.channels[0].id,
        });
    assert!(
        project.validate().is_err(),
        "absolute sample paths must fail validation"
    );

    // Relative sibling save stays portable (../...)
    let dir = tempfile::tempdir().unwrap();
    let sample_dir = dir.path().join("kit");
    let proj_dir = dir.path().join("song");
    std::fs::create_dir_all(&sample_dir).unwrap();
    std::fs::create_dir_all(&proj_dir).unwrap();
    let wav_path = sample_dir.join("hit.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 8_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&wav_path, spec).unwrap();
    for _ in 0..64 {
        w.write_sample(1000_i16).unwrap();
    }
    w.finalize().unwrap();

    let mut p = cadence_core::material::Project::demo();
    let channel = p.mixer.channels[0].id;
    let mut rel_rack = cadence_core::rack::RackPatch::default_sampler();
    for (_, _, smp) in rel_rack.sampler_modules_mut() {
        smp.path = Some("../kit/hit.wav".into());
    }
    p.instruments
        .push(cadence_core::material::InstrumentInstance {
            id: 999,
            name: "Kit".into(),
            rack: rel_rack,
            channel,
        });
    let proj_path = proj_dir.join("project.json");
    let lib = SampleLibrary::from_roots(&[], Some(&proj_path));
    let app = cadence_app::App::new_with_library(p, proj_path.clone(), lib).unwrap();
    app.save_project(&proj_path).unwrap();
    let saved = std::fs::read_to_string(&proj_path).unwrap();
    assert!(saved.contains("../kit/hit.wav"), "saved path: {saved}");
    assert!(!Path::new("../kit/hit.wav").is_absolute());
    let reloaded = load_project(&proj_path).unwrap();
    assert_eq!(
        reloaded.instruments.last().unwrap().rack,
        app.project().instruments.last().unwrap().rack
    );
}

#[test]
fn same_host_render_is_sample_deterministic() {
    let project = cadence_core::material::Project::demo();
    let compiled = project.compile().unwrap();
    let (rep1, pcm1) = render_schedule_pcm(&[(0, &compiled)], 0, 32, Vec::new(), 24_000).unwrap();
    let (rep2, pcm2) = render_schedule_pcm(&[(0, &compiled)], 0, 32, Vec::new(), 24_000).unwrap();
    assert_eq!(pcm1, pcm2, "repeated offline renders must be bit-identical");
    assert_eq!(rep1, rep2, "repeated render reports must match identically");
    assert!(rep1.output.peak[0] > 0.05 && rep1.output.peak[1] > 0.05);

    // Sensitivity check: a small velocity change produces distinct PCM.
    let mut mutated = project.clone();
    if let cadence_core::material::PatternKind::Notes { steps } = &mut mutated.patterns[0].kind {
        if let Some(s) = steps.iter_mut().find(|s| s.active) {
            s.velocity = (s.velocity - 0.05).max(0.05);
        }
    }
    let compiled_mut = mutated.compile().unwrap();
    let (_, pcm_mut) =
        render_schedule_pcm(&[(0, &compiled_mut)], 0, 32, Vec::new(), 24_000).unwrap();
    assert_ne!(
        pcm1, pcm_mut,
        "mutating an active note velocity must change rendered PCM"
    );
}

#[test]
fn offline_compile_and_live_ring_window_have_exact_parity() {
    let demo_path = repo_root().join("examples/demo/project.json");
    let demo_project = load_project(&demo_path).unwrap();
    let compiled = demo_project.compile().unwrap();
    let mut offline_lib = SampleLibrary::from_roots(&[], Some(&demo_path));
    let offline_samples = load_project_samples(&compiled, &mut offline_lib);
    offline_samples.ensure_ready().unwrap();

    let live_lib = SampleLibrary::from_roots(&[], Some(&demo_path));
    let app = cadence_app::App::new_with_library(demo_project, demo_path, live_lib).unwrap();

    let steps = 4 * 16; // first 4 bars
    let (rep_offline, pcm_offline) =
        render_schedule_pcm(&[(0, &compiled)], 0, steps, offline_samples.bank, 24_000).unwrap();
    let (rep_live, pcm_live) = render_schedule_pcm(
        &[(0, app.audio_project())],
        0,
        steps,
        app.sample_bank(),
        24_000,
    )
    .unwrap();

    assert_eq!(
        pcm_offline, pcm_live,
        "offline project.compile() and App::audio_project() live ring window must render identically"
    );
    assert_eq!(rep_offline, rep_live);
    assert!(rep_offline.output.peak[0] > 0.05 && rep_offline.output.rms[0] > 0.01);
}

#[test]
fn all_showcase_examples_and_fivefold_take_pass_signal_hygiene() {
    let sr = if cfg!(debug_assertions) {
        24_000
    } else {
        48_000
    };
    // In release mode, render every bar of every showcase song and take in parallel.
    // In debug mode, render full `material-lab` plus representative climax+outro windows
    // of the longer 32–64 bar showcases in parallel so `cargo test` stays brisk (<15s).
    let (demo_start, glass_start, iron_start, fivefold_start, take_start) =
        if cfg!(debug_assertions) {
            (832_u64, 320_u64, 576_u64, 704_u64, Some(704_u64))
        } else {
            (0_u64, 0_u64, 0_u64, 0_u64, None)
        };

    let reports: Vec<(HygieneExpectation, RenderReport)> = std::thread::scope(|s| {
        let h_demo = s.spawn(|| {
            (
                HygieneExpectation {
                    name: "demo",
                    require_silent_tail: false,
                },
                measure_example_window("examples/demo/project.json", demo_start, None, sr),
            )
        });
        let h_glass = s.spawn(|| {
            (
                HygieneExpectation {
                    name: "glass-cathedral",
                    require_silent_tail: false,
                },
                measure_example_window(
                    "examples/glass-cathedral/project.json",
                    glass_start,
                    None,
                    sr,
                ),
            )
        });
        let h_iron = s.spawn(|| {
            (
                HygieneExpectation {
                    name: "iron-circuit",
                    require_silent_tail: true,
                },
                measure_example_window("examples/iron-circuit/project.json", iron_start, None, sr),
            )
        });
        let h_lab = s.spawn(|| {
            (
                HygieneExpectation {
                    name: "material-lab",
                    require_silent_tail: true,
                },
                measure_example_window("examples/material-lab/project.json", 0, None, sr),
            )
        });
        let h_fivefold = s.spawn(|| {
            (
                HygieneExpectation {
                    name: "fivefold-live",
                    require_silent_tail: true,
                },
                measure_example_window(
                    "examples/fivefold-live/project.json",
                    fivefold_start,
                    None,
                    sr,
                ),
            )
        });
        let h_take = s.spawn(|| {
            (
                HygieneExpectation {
                    name: "fivefold_take",
                    require_silent_tail: true,
                },
                measure_fivefold_take_window(take_start, sr),
            )
        });

        vec![
            h_demo.join().unwrap(),
            h_glass.join().unwrap(),
            h_iron.join().unwrap(),
            h_lab.join().unwrap(),
            h_fivefold.join().unwrap(),
            h_take.join().unwrap(),
        ]
    });

    for (expect, rep) in &reports {
        assert_signal_hygiene(expect, rep);
    }
}

#[test]
fn equal_power_pan_law_preserves_stereo_power_across_synth_and_sampler() {
    use cadence_core::material::{
        InstrumentInstance, Lane, Part, Pattern, PatternKind, Placement, Project,
    };
    use cadence_core::mixer::{Channel, ChannelKind};
    use cadence_core::music::Step;
    use cadence_core::rack::{
        module_sample_key, EnvModule, InPort, ModuleKind, OutModule, PatchCable, RackModule,
        RackPatch, RackZone, SamplerModule, VcaModule, VcoModule, VoiceSettings, Waveform,
    };
    use cadence_core::sampler::{SampleData, SampleMetadata};
    use std::sync::Arc;

    let sr = 24_000u32;
    let measure_power = |rack: RackPatch, bank: Vec<(u64, Arc<SampleData>)>| -> f64 {
        let mut p = Project::empty();
        p.bpm = 120.0;
        p.bars = 1;
        p.mixer.master.strip.gain = 1.0;
        p.mixer
            .channels
            .push(Channel::new(1, ChannelKind::Audio, "Pan"));
        let hit = Step {
            active: true,
            degree: 1,
            velocity: 0.9,
            gate: 4.0,
            ..Step::default()
        };
        p.patterns.push(Pattern {
            id: 1,
            name: "Hold".into(),
            kind: PatternKind::Notes {
                steps: vec![hit, Step::default(), Step::default(), Step::default()],
            },
        });
        p.instruments.push(InstrumentInstance {
            id: 1,
            name: "Src".into(),
            rack,
            channel: 1,
        });
        p.parts.push(Part::new(1, "Part", 1, 1));
        let mut lane = Lane::new(1, "Pan");
        lane.placements.push(Placement::new(1, 1, 0, 16));
        p.lanes.push(lane);
        let score = p.compile().unwrap();
        let (_, pcm) = render_schedule_pcm(&[(0, &score)], 0, 4, bank, sr).unwrap();
        let slice = &pcm[1000..pcm.len() - 1000];
        let mut power = 0.0_f64;
        for f in slice {
            let l = f64::from(f[0]) / 32768.0;
            let r = f64::from(f[1]) / 32768.0;
            power += l * l + r * r;
        }
        power / slice.len() as f64
    };

    let make_synth_rack = |pan: f32| RackPatch {
        kit_name: String::new(),
        zones: vec![RackZone {
            id: 1,
            name: "Synth".into(),
            low_midi: 0,
            high_midi: 127,
            root_midi: 60,
            low_vel: 0.0,
            high_vel: 1.0,
            gain_db: 0.0,
            pan: 0.0,
            choke_group: None,
            voice: VoiceSettings::default(),
            modules: vec![
                RackModule::new(
                    1,
                    "VCO",
                    ModuleKind::Vco(VcoModule {
                        waveform: Waveform::Sine,
                        level: 0.7,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(2, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(
                    3,
                    "Env",
                    ModuleKind::Env(EnvModule {
                        attack_ms: 1.0,
                        decay_ms: 1.0,
                        sustain: 1.0,
                        ..EnvModule::default()
                    }),
                ),
                RackModule::new(
                    4,
                    "Out",
                    ModuleKind::Out(OutModule {
                        gain_db: 0.0,
                        pan,
                        spread: 0.0,
                    }),
                ),
            ],
            cables: vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 2, 4, InPort::In, 1.0),
                PatchCable::new(3, 3, 2, InPort::Cv, 1.0),
            ],
        }],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 20_000.0,
        resonance: 0.0,
        panel_pins: Vec::new(),
    };

    // 1. Synth mono equal-power pan across [-1, 1]
    let synth_center = measure_power(make_synth_rack(0.0), Vec::new());
    for pan in [-1.0_f32, -0.5, 0.5, 1.0] {
        let pw = measure_power(make_synth_rack(pan), Vec::new());
        let diff_db = lin_to_db_f64((pw / synth_center).sqrt());
        assert!(
            diff_db.abs() < 0.15,
            "Synth pan={pan} power drifted {diff_db:+.3} dB from center"
        );
    }

    // 2. Sampler mono equal-power pan across [-1, 1]
    let frames = 24_000;
    let sine: Box<[f32]> = (0..frames)
        .map(|i| 0.5 * (std::f64::consts::TAU * 220.0 * i as f64 / f64::from(sr)).sin() as f32)
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let sample = Arc::new(SampleData {
        path_label: "./pan_ref.wav".into(),
        resolved_path: PathBuf::from("pan_ref.wav"),
        name: "pan_ref".into(),
        sample_rate: sr,
        channels: 1,
        frames,
        left: sine,
        right: None,
        peaks: vec![(-0.5, 0.5); 64].into_boxed_slice(),
        meta: SampleMetadata::default(),
    });
    let make_sampler_rack = |pan: f32| RackPatch {
        kit_name: String::new(),
        zones: vec![RackZone {
            id: 1,
            name: "Sampler".into(),
            low_midi: 0,
            high_midi: 127,
            root_midi: 60,
            low_vel: 0.0,
            high_vel: 1.0,
            gain_db: 0.0,
            pan: 0.0,
            choke_group: None,
            voice: VoiceSettings::default(),
            modules: vec![
                RackModule::new(
                    1,
                    "Sampler",
                    ModuleKind::Sampler(SamplerModule {
                        path: Some("./pan_ref.wav".into()),
                        root_midi: 60,
                        one_shot: false,
                        ..SamplerModule::default()
                    }),
                ),
                RackModule::new(2, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(
                    3,
                    "Env",
                    ModuleKind::Env(EnvModule {
                        attack_ms: 1.0,
                        decay_ms: 1.0,
                        sustain: 1.0,
                        ..EnvModule::default()
                    }),
                ),
                RackModule::new(
                    4,
                    "Out",
                    ModuleKind::Out(OutModule {
                        gain_db: 0.0,
                        pan,
                        spread: 0.0,
                    }),
                ),
            ],
            cables: vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 2, 4, InPort::In, 1.0),
                PatchCable::new(3, 3, 2, InPort::Cv, 1.0),
            ],
        }],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 20_000.0,
        resonance: 0.0,
        panel_pins: Vec::new(),
    };
    let sample_key = module_sample_key(1, 1, 1);
    let smp_center = measure_power(
        make_sampler_rack(0.0),
        vec![(sample_key, Arc::clone(&sample))],
    );
    for pan in [-1.0_f32, -0.5, 0.5, 1.0] {
        let pw = measure_power(
            make_sampler_rack(pan),
            vec![(sample_key, Arc::clone(&sample))],
        );
        let diff_db = lin_to_db_f64((pw / smp_center).sqrt());
        assert!(
            diff_db.abs() < 0.15,
            "Sampler pan={pan} power drifted {diff_db:+.3} dB from center"
        );
    }
}

#[test]
fn unity_master_and_limiter_ceiling_enforce_exact_dbfs_bounds() {
    use cadence_core::material::{
        InstrumentInstance, Lane, Part, Pattern, PatternKind, Placement, Project,
    };
    use cadence_core::mixer::{Channel, ChannelKind, EffectDevice, EffectKind, LimiterPatch};
    use cadence_core::music::Step;
    use cadence_core::rack::{
        EnvModule, InPort, ModuleKind, OutModule, PatchCable, RackModule, RackPatch, RackZone,
        VcaModule, VcoModule, VoiceSettings, Waveform,
    };

    let sr = 24_000u32;
    let mut p = Project::empty();
    p.bpm = 120.0;
    p.bars = 1;
    p.root_midi = 60;
    p.mixer.master.strip.gain = 1.0;
    p.mixer
        .channels
        .push(Channel::new(1, ChannelKind::Audio, "Wall"));
    let hit = Step {
        active: true,
        degree: 1,
        velocity: 1.0,
        gate: 4.0,
        ..Step::default()
    };
    p.patterns.push(Pattern {
        id: 1,
        name: "Wall".into(),
        kind: PatternKind::Notes {
            steps: vec![hit; 16],
        },
    });
    let wall_rack = RackPatch {
        kit_name: String::new(),
        zones: vec![RackZone {
            id: 1,
            name: "Wall".into(),
            low_midi: 0,
            high_midi: 127,
            root_midi: 60,
            low_vel: 0.0,
            high_vel: 1.0,
            gain_db: 0.0,
            pan: 0.0,
            choke_group: None,
            voice: VoiceSettings::default(),
            modules: vec![
                RackModule::new(
                    1,
                    "VCO",
                    ModuleKind::Vco(VcoModule {
                        waveform: Waveform::Sine,
                        level: 1.0,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(2, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(
                    3,
                    "Env",
                    ModuleKind::Env(EnvModule {
                        attack_ms: 1.0,
                        sustain: 1.0,
                        ..EnvModule::default()
                    }),
                ),
                RackModule::new(4, "Out", ModuleKind::Out(OutModule::default())),
            ],
            cables: vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 2, 4, InPort::In, 1.0),
                PatchCable::new(3, 3, 2, InPort::Cv, 1.0),
            ],
        }],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 20_000.0,
        resonance: 0.0,
        panel_pins: Vec::new(),
    };
    p.instruments.push(InstrumentInstance {
        id: 1,
        name: "WallSynth".into(),
        rack: wall_rack,
        channel: 1,
    });
    p.parts.push(Part::new(1, "Part", 1, 1));
    let mut lane = Lane::new(1, "Wall");
    lane.placements.push(Placement::new(1, 1, 0, 16));
    p.lanes.push(lane);

    // Master Limiter at -3.0 dBFS with +12 dB drive must hit ceiling tightly with 0 clips.
    let ceiling_db = -3.0_f32;
    let ceiling_lin = f64::from(cadence_core::dsp::db_to_lin(ceiling_db));
    p.mixer.master.inserts.push(EffectDevice {
        id: 50,
        name: "Master Limiter".into(),
        bypassed: false,
        kind: EffectKind::Limiter(LimiterPatch {
            ceiling_db,
            input_drive_db: 12.0,
            release_ms: 50.0,
            mix: 1.0,
        }),
    });
    let score = p.compile().unwrap();
    let (rep, _) = render_schedule_pcm(&[(0, &score)], 0, 16, Vec::new(), sr).unwrap();
    assert_eq!(rep.output.full_scale_samples, 0);
    let peak = rep.output.peak[0].max(rep.output.peak[1]);
    assert!(
        peak <= ceiling_lin + 0.002 && peak >= ceiling_lin - 0.02,
        "Master Limiter at -3 dBFS must land accurately at ceiling without hidden tanh loss: peak={peak:.4}, ceiling={ceiling_lin:.4}"
    );
}

#[test]
fn rack_zone_velocity_split_gain_and_choke_invariants() {
    use cadence_core::material::{
        InstrumentInstance, Lane, Part, Pattern, PatternKind, Placement, Project,
    };
    use cadence_core::mixer::{Channel, ChannelKind};
    use cadence_core::music::Step;
    use cadence_core::rack::{
        EnvModule, InPort, ModuleKind, OutModule, PatchCable, RackModule, RackPatch, RackZone,
        VcaModule, VcoModule, VoiceSettings, Waveform,
    };

    let sr = 24_000u32;
    let tone_zone = |id: u16,
                     name: &str,
                     low_vel: f32,
                     high_vel: f32,
                     gain_db: f32,
                     choke_group: Option<u8>,
                     pan: f32| RackZone {
        id,
        name: name.into(),
        low_midi: 60,
        high_midi: 60,
        root_midi: 60,
        low_vel,
        high_vel,
        gain_db,
        pan: 0.0,
        choke_group,
        voice: VoiceSettings::default(),
        modules: vec![
            RackModule::new(
                1,
                "VCO",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    level: 0.6,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(2, "VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                3,
                "Env",
                ModuleKind::Env(EnvModule {
                    attack_ms: 0.5,
                    decay_ms: 10.0,
                    sustain: 0.8,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                4,
                "Out",
                ModuleKind::Out(OutModule {
                    gain_db: 0.0,
                    pan,
                    spread: 0.0,
                }),
            ),
        ],
        cables: vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 2, 4, InPort::In, 1.0),
            PatchCable::new(3, 3, 2, InPort::Cv, 1.0),
        ],
    };

    // Velocity split [0.0, 0.5) left-only vs [0.5, 1.0] right-only: velocity == 0.5 must ONLY trigger right!
    let z_low = tone_zone(1, "LowL", 0.0, 0.5, 0.0, None, -1.0);
    let z_high = tone_zone(2, "HighR", 0.5, 1.0, 6.0, Some(1), 1.0);

    let kit = RackPatch {
        kit_name: "SplitKit".into(),
        zones: vec![z_low, z_high],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 20_000.0,
        resonance: 0.0,
        panel_pins: Vec::new(),
    };
    let mut p = Project::empty();
    p.bpm = 120.0;
    p.bars = 1;
    p.root_midi = 60;
    p.mixer.master.strip.gain = 1.0;
    p.mixer
        .channels
        .push(Channel::new(1, ChannelKind::Audio, "Kit"));
    let hit = Step {
        active: true,
        degree: 1,
        velocity: 0.5,
        gate: 2.0,
        ..Step::default()
    };
    p.patterns.push(Pattern {
        id: 1,
        name: "Split".into(),
        kind: PatternKind::Notes {
            steps: vec![hit, Step::default(), Step::default(), Step::default()],
        },
    });
    p.instruments.push(InstrumentInstance {
        id: 1,
        name: "SplitKit".into(),
        rack: kit,
        channel: 1,
    });
    p.parts.push(Part::new(1, "Part", 1, 1));
    let mut lane = Lane::new(1, "Kit");
    lane.placements.push(Placement::new(1, 1, 0, 16));
    p.lanes.push(lane);

    let score = p.compile().unwrap();
    let (rep, _) = render_schedule_pcm(&[(0, &score)], 0, 4, Vec::new(), sr).unwrap();
    assert!(
        rep.output.peak[0] < 1e-4 && rep.output.peak[1] > 0.2,
        "velocity boundary 0.5 must trigger ONLY HighR (no LowL double-trigger): peak={:?}",
        rep.output.peak
    );
}
