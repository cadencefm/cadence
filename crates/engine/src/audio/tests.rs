use super::*;
use crate::audio::engine::Engine;
use crate::audio::shared::Shared;
use cadence_core::dsp::{resample_stereo, EnvMode};
use cadence_core::material::{
    InstrumentInstance, Lane, Modulation, ModulationTarget, ParameterLink, Part, Pattern,
    PatternKind, Placement, Project, ValuePoint, ValueUnit,
};
use cadence_core::mixer::{Channel, ChannelKind, ChannelRef, ChannelSend, EffectDevice};
use cadence_core::music::{ChordTone, EndRule, Step};
use cadence_core::params::{InstrumentParam, RackParam, ValueMode, ZoneParam};
use cadence_core::rack::{
    drum_midi, module_sample_key, EnvModule, FilterMode, FilterModule, InPort, ModuleKind,
    ModuleParam, NoiseColor, NoiseModule, OutModule, PatchCable, PitchTracking, RackModule,
    RackPatch, RackZone, SamplerModule, TriggerMode, VcaModule, VcoModule, VoiceMode,
    VoiceSettings, Waveform,
};
use cadence_core::sampler::{SampleData, SampleLoopMode, SampleMetadata};
use cadence_core::score::{LiveWindow, ScoreEvent};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn note(degree: i16) -> Step {
    Step {
        active: true,
        degree,
        velocity: 0.85,
        gate: 0.5,
        ..Step::default()
    }
}

fn rack_project(steps: Vec<Step>, rack: RackPatch) -> Project {
    let mut p = Project::empty();
    p.bpm = 120.0;
    p.bars = 2;
    p.end = EndRule::Loop;
    p.mixer.master.strip.gain = 1.0;
    p.mixer
        .channels
        .push(Channel::new(1, ChannelKind::Audio, "Lead"));
    p.patterns.push(Pattern {
        id: 1,
        name: "Notes".into(),
        kind: PatternKind::Notes { steps },
    });
    p.instruments.push(InstrumentInstance {
        id: 1,
        name: "Lead".into(),
        rack,
        channel: 1,
    });
    p.parts.push(Part::new(1, "LeadPart", 1, 1));
    let mut lane = Lane::new(1, "Lead");
    lane.placements.push(Placement::new(1, 1, 0, 32));
    p.lanes.push(lane);
    p
}

/// Mutate `Amp Env` (module 4) on `RackPatch::default_synth()` or `RackPatch::default_sampler()`.
fn amp_env_mut(rack: &mut RackPatch) -> &mut EnvModule {
    match &mut rack.zones[0]
        .module_mut(4)
        .expect("default rack amp env module 4")
        .kind
    {
        ModuleKind::Env(env) => env,
        other => panic!("expected Env at module 4, got {other:?}"),
    }
}

/// Mutate `Filter` (module 2) on `RackPatch::default_synth()` or `RackPatch::default_sampler()`.
fn filter_mut(rack: &mut RackPatch) -> &mut FilterModule {
    match &mut rack.zones[0]
        .module_mut(2)
        .expect("default rack filter module 2")
        .kind
    {
        ModuleKind::Filter(flt) => flt,
        other => panic!("expected Filter at module 2, got {other:?}"),
    }
}

/// Mutate `Sampler` (module 1) on `RackPatch::default_sampler()`.
fn sampler_mut(rack: &mut RackPatch) -> &mut SamplerModule {
    match &mut rack.zones[0]
        .module_mut(1)
        .expect("default sampler module 1")
        .kind
    {
        ModuleKind::Sampler(smp) => smp,
        other => panic!("expected Sampler at module 1, got {other:?}"),
    }
}

/// Mutate `Out` on a zone (module 5 for `default_sampler`, module 6 for `default_synth`).
fn out_mut(rack: &mut RackPatch, module_id: u16) -> &mut OutModule {
    match &mut rack.zones[0]
        .module_mut(module_id)
        .expect("out module")
        .kind
    {
        ModuleKind::Out(out) => out,
        other => panic!("expected Out at module {module_id}, got {other:?}"),
    }
}

fn synthetic_sample(frames: usize, sample_rate: u32, freq_hz: f64) -> Arc<SampleData> {
    let left: Box<[f32]> = (0..frames)
        .map(|i| (std::f64::consts::TAU * freq_hz * i as f64 / f64::from(sample_rate)).sin() as f32)
        .collect::<Vec<_>>()
        .into_boxed_slice();
    Arc::new(SampleData {
        path_label: "./test.wav".into(),
        resolved_path: std::path::PathBuf::from("test.wav"),
        name: "test".into(),
        sample_rate,
        channels: 1,
        frames,
        left,
        right: None,
        peaks: vec![(-1.0, 1.0); 64].into_boxed_slice(),
        meta: SampleMetadata::default(),
    })
}

fn wait_until(pred: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !pred() {
        assert!(Instant::now() < deadline, "timed out waiting for engine");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn offline_render_is_deterministic_and_produces_non_silent_wav() {
    let score = Project::demo().compile().unwrap();
    let (r1, pcm1) = render_schedule_pcm(&[(0, &score)], 0, 2 * 16, Vec::new(), 24_000).unwrap();
    let (r2, pcm2) = render_schedule_pcm(&[(0, &score)], 0, 2 * 16, Vec::new(), 24_000).unwrap();
    assert_eq!(pcm1, pcm2);
    assert_eq!(r1, r2);
    assert!(r1.output.peak[0] > 0.05);
    assert!(r1.output.rms[0] > 0.01);

    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("demo.wav");
    render_wav_with_samples(&score, Vec::new(), &wav, 1, 24_000).unwrap();
    let reader = hound::WavReader::open(&wav).unwrap();
    assert_eq!(reader.spec().sample_rate, 24_000);
    assert_eq!(reader.spec().channels, 2);
}

#[test]
fn sub_step_frame_scheduler_fires_events_at_exact_fractional_offsets() {
    // 120 BPM @ 16 kHz -> 16th step = 0.125 s = exactly 2000 frames!
    let sr = 16_000_u32;
    let mut rack = RackPatch::default_synth();
    {
        let env = amp_env_mut(&mut rack);
        env.attack_ms = 0.2;
        env.decay_ms = 25.0;
        env.sustain = 0.0;
        env.release_ms = 10.0;
    }
    let mut score = rack_project(vec![Step::default(); 16], rack)
        .compile()
        .unwrap();

    // Inject two sub-step hits into step 0: offset 0.0 (frame 0) and offset 0.5 (frame 1000).
    let make_ev = |offset: f32, midi: u8| ScoreEvent {
        inst_idx: 0,
        part_id: Some(1),
        offset,
        degree: 1,
        accidental: 0,
        midi,
        velocity: 0.9,
        gate: 0.2,
        chord: Default::default(),
        chord_len: 0,
        lock_start: 0,
        lock_len: 0,
    };
    score.events = vec![make_ev(0.0, 60), make_ev(0.5, 72)];
    score.step_ranges[0] = (0, 2);

    let (_, pcm) = render_schedule_pcm(&[(0, &score)], 0, 2, Vec::new(), sr).unwrap();
    // Measure energy in windows around frame 0 (5..350), quiet gap (750..990), and frame 1000 (1005..1350).
    let rms = |start_frame: usize, end_frame: usize| -> f64 {
        let mut sum = 0.0;
        for frame in &pcm[start_frame..end_frame] {
            let l = f64::from(frame[0]) / 32768.0;
            sum += l * l;
        }
        (sum / (end_frame - start_frame) as f64).sqrt()
    };
    let hit1 = rms(5, 350);
    let mid_gap = rms(750, 990);
    let hit2 = rms(1005, 1350);
    assert!(hit1 > 0.02, "first hit at frame 0 must sound: {hit1}");
    assert!(
        hit2 > mid_gap * 2.0 && hit2 > 0.02,
        "sub-step hit at offset 0.5 (frame 1000) must fire cleanly: mid_gap={mid_gap}, hit2={hit2}"
    );
}

#[test]
fn silent_runtime_handles_transport_live_jam_notes_audition_and_score_hotswap() {
    let mut rack = RackPatch::default_synth();
    rack.zones[0].voice.voice_mode = VoiceMode::Poly;
    let score = rack_project(vec![note(1); 16], rack).compile().unwrap();
    let rt = AudioRuntime::start(score.clone(), true).unwrap();

    // Live Jam NoteOn produces audio even while transport is stopped!
    let inst_id = score.instruments[0].id;
    assert!(rt.live_note_on_instrument(inst_id, 64, 0.9, true, None));
    wait_until(|| rt.snapshot().peak > 0.02);
    assert!(rt.live_note_off_instrument(inst_id, 64));

    // Audition preview_rack & preview_sample work cleanly while stopped!
    rt.preview_rack(RackPatch::acid_303(), 48, 0.95).unwrap();
    wait_until(|| rt.snapshot().peak > 0.02);
    let sample = synthetic_sample(4800, 48_000, 440.0);
    rt.preview_sample(sample);
    wait_until(|| rt.snapshot().peak > 0.02);

    // Transport Play advances absolute_step and next_step monotonically.
    assert!(rt.transport(TransportIntent::Play { playing: true }));
    wait_until(|| {
        let s = rt.snapshot();
        s.playing && s.next_step >= 2
    });

    // Seek moves transport without error.
    assert!(rt.transport(TransportIntent::Seek { step: 12 }));
    wait_until(|| rt.snapshot().absolute_step >= 12);

    // Hot-swap score while running.
    let mut next = score;
    next.bpm = 140.0;
    rt.replace_project(next).unwrap();
    assert!(rt.transport(TransportIntent::Reset { step: 0 }));
    wait_until(|| {
        let s = rt.snapshot();
        !s.playing && s.absolute_step == 0
    });
}

#[test]
fn sampler_instrument_plays_prepared_sample_and_respects_looping_and_svf() {
    let mut p = Project::empty();
    p.bpm = 120.0;
    p.bars = 1;
    p.mixer.master.strip.gain = 1.0;
    p.mixer
        .channels
        .push(Channel::new(1, ChannelKind::Audio, "Sampler"));
    p.patterns.push(Pattern {
        id: 1,
        name: "Hit".into(),
        kind: PatternKind::Notes {
            steps: vec![note(1), Step::default(), Step::default(), Step::default()],
        },
    });
    let mut rack = RackPatch::default_sampler();
    {
        let smp = sampler_mut(&mut rack);
        smp.path = Some("./test.wav".into());
        smp.sample_start = 0;
        smp.sample_end = 4800;
        smp.loop_mode = SampleLoopMode::Forward;
        smp.loop_start = 200;
        smp.loop_end = 4400;
        smp.one_shot = false;
    }
    {
        let flt = filter_mut(&mut rack);
        flt.cutoff_hz = 4000.0;
        flt.resonance = 0.25;
    }
    p.instruments.push(InstrumentInstance {
        id: 1,
        name: "Smp".into(),
        rack,
        channel: 1,
    });
    p.parts.push(Part::new(1, "SmpPart", 1, 1));
    let mut lane = Lane::new(1, "Smp");
    lane.placements.push(Placement::new(1, 1, 0, 16));
    p.lanes.push(lane);

    let score = p.compile().unwrap();
    let sample = synthetic_sample(4800, 24_000, 440.0);
    let key = module_sample_key(1, 1, 1);
    let report = render_measure_report(&score, vec![(key, sample)], 1, 24_000).unwrap();
    assert!(report.output.peak[0] > 0.05);
}

#[test]
fn rack_per_hit_locks_and_continuous_modulation_both_take_effect() {
    let mut p = rack_project(vec![note(1); 16], RackPatch::default_synth());
    p.patterns.push(Pattern {
        id: 2,
        name: "CutoffRamp".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 16,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 0.1,
                },
                ValuePoint {
                    step: 15,
                    value: 0.95,
                },
            ],
        },
    });
    p.patterns.push(Pattern {
        id: 3,
        name: "PanLock".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 16,
            mode: ValueMode::Hold,
            points: vec![ValuePoint {
                step: 0,
                value: 0.0,
            }],
        },
    });
    p.patterns.push(Pattern {
        id: 4,
        name: "CableLock".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 16,
            mode: ValueMode::Hold,
            points: vec![ValuePoint {
                step: 0,
                value: 0.75,
            }],
        },
    });
    p.modulations.push(Modulation {
        id: 1,
        name: "Sweep".into(),
        target: ModulationTarget::Instrument {
            instrument: 1,
            parameter: InstrumentParam::Module {
                zone: 1,
                module: 2,
                param: ModuleParam::Cutoff,
            },
        },
        pattern: 2,
    });
    p.parts[0].parameter_links.push(ParameterLink {
        target: InstrumentParam::Module {
            zone: 1,
            module: 6,
            param: ModuleParam::Pan,
        },
        pattern: 3,
    });
    p.parts[0].parameter_links.push(ParameterLink {
        target: InstrumentParam::Cable { zone: 1, cable: 5 },
        pattern: 4,
    });

    let score = p.compile().unwrap();
    let shared = Shared::new_arc();
    shared.playing.store(true, Ordering::Release);
    let mut engine = Engine::new(score, Vec::new(), 24_000, shared);
    // Run 1 step so voice triggers with pan & cable locks while cutoff tracks ModLane.
    for _ in 0..100 {
        engine.next_frame();
    }
    let active_voice = engine
        .voices()
        .iter()
        .find(|v| v.active)
        .expect("rack synth voice active");
    assert!(
        active_voice.modular.is_module_locked(6, ModuleParam::Pan),
        "per-hit module pan lock must be recorded on triggered voice"
    );
    assert!(
        active_voice.modular.is_cable_locked(5),
        "per-hit cable lock must be recorded on triggered voice"
    );
    assert!(
        !active_voice
            .modular
            .is_module_locked(2, ModuleParam::Cutoff),
        "unlocked filter cutoff must remain free for continuous ModLane"
    );
}

#[test]
fn polyphonic_chords_and_channel_send_delay_render_cleanly() {
    let mut chord_step = note(1);
    chord_step.chord = vec![
        ChordTone {
            degree: 3,
            accidental: 0,
            velocity: 0.7,
            gate: 0.5,
        },
        ChordTone {
            degree: 5,
            accidental: 0,
            velocity: 0.7,
            gate: 0.5,
        },
    ];
    let mut p = rack_project(
        vec![
            chord_step,
            Step::default(),
            Step::default(),
            Step::default(),
        ],
        RackPatch::default_synth(),
    );
    let mut ret = Channel::new(2, ChannelKind::Return, "Echo");
    ret.inserts.push(EffectDevice::delay(1));
    p.mixer.channels.push(ret);
    p.mixer.channels[0].sends.push(ChannelSend {
        target: 2,
        amount: 0.0,
    });
    // Automate Channel 1 -> Echo return send via first-class ModulationTarget::Send!
    p.patterns.push(Pattern {
        id: 2,
        name: "Echo Send".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 16,
            mode: ValueMode::Hold,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 0.65,
                },
                ValuePoint {
                    step: 8,
                    value: 0.0,
                },
            ],
        },
    });
    p.modulations.push(Modulation {
        id: 1,
        name: "Lead Echo Send".into(),
        target: ModulationTarget::Send {
            channel: ChannelRef::Channel(1),
            target: 2,
        },
        pattern: 2,
    });

    let score = p.compile().unwrap();
    let report = render_measure_report(&score, Vec::new(), 1, 24_000).unwrap();
    assert!(report.output.peak[0] > 0.05);
    assert!(
        report
            .channels
            .iter()
            .any(|c| c.channel == ChannelRef::Channel(2) && c.output_peak_hold[0] > 0.01),
        "return delay must receive modulated Channel.sends audio"
    );
}

#[test]
fn live_window_stop_at_halts_engine_and_mono_legato_retargets_single_voice() {
    let mut rack = RackPatch::default_synth();
    rack.zones[0].voice.voice_mode = VoiceMode::Mono;
    rack.zones[0].voice.legato = true;
    let mut score = rack_project(vec![note(1), note(3), note(5), note(7)], rack)
        .compile()
        .unwrap();
    score.live_window = Some(LiveWindow {
        start: 0,
        end: 32,
        stop_at: Some(4),
    });
    let shared = Shared::new_arc();
    shared.playing.store(true, Ordering::Release);
    let mut engine = Engine::new(score, Vec::new(), 16_000, Arc::clone(&shared));
    // 4 steps at 120 BPM @ 16 kHz = 8000 frames; run 10000 frames.
    for _ in 0..10_000 {
        engine.next_frame();
    }
    assert!(
        !shared.playing.load(Ordering::Acquire),
        "live_window.stop_at must stop transport cleanly"
    );
    assert_eq!(engine.play_step(), 4);
    let active_held = engine
        .voices()
        .iter()
        .filter(|v| v.active && !v.is_releasing())
        .count();
    assert!(active_held <= 1, "mono mode keeps at most one held voice");
}

fn make_kick_zone(id: u16, midi: u8) -> RackZone {
    RackZone {
        id,
        name: "BD".into(),
        low_midi: midi,
        high_midi: midi,
        root_midi: midi,
        low_vel: 0.0,
        high_vel: 1.0,
        gain_db: 0.0,
        pan: 0.0,
        choke_group: None,
        voice: VoiceSettings {
            pitch_tracking: PitchTracking::Fixed,
            ..VoiceSettings::default()
        },
        modules: vec![
            RackModule::new(
                1,
                "Body",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    level: 0.9,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "Pitch Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::HoldDecay,
                    attack_ms: 0.2,
                    hold_ms: 0.0,
                    decay_ms: 45.0,
                    sustain: 0.0,
                    release_ms: 20.0,
                    trigger_mode: TriggerMode::OneShot,
                    delay_ms: 0.0,
                    bursts: 1,
                    burst_spacing_ms: 10.0,
                }),
            ),
            RackModule::new(
                3,
                "Amp Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::HoldDecay,
                    attack_ms: 0.3,
                    hold_ms: 0.0,
                    decay_ms: 240.0,
                    sustain: 0.0,
                    release_ms: 40.0,
                    trigger_mode: TriggerMode::OneShot,
                    delay_ms: 0.0,
                    bursts: 1,
                    burst_spacing_ms: 10.0,
                }),
            ),
            RackModule::new(4, "VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
        ],
        cables: vec![
            PatchCable::new(1, 2, 1, InPort::PitchCv, 28.0),
            PatchCable::new(2, 1, 4, InPort::In, 1.0),
            PatchCable::new(3, 3, 4, InPort::Cv, 1.0),
            PatchCable::new(4, 4, 5, InPort::In, 1.0),
        ],
    }
}

fn make_snare_zone(id: u16, midi: u8) -> RackZone {
    RackZone {
        id,
        name: "SD".into(),
        low_midi: midi,
        high_midi: midi,
        root_midi: 55,
        low_vel: 0.0,
        high_vel: 1.0,
        gain_db: 0.0,
        pan: 0.0,
        choke_group: None,
        voice: VoiceSettings {
            pitch_tracking: PitchTracking::Fixed,
            ..VoiceSettings::default()
        },
        modules: vec![
            RackModule::new(
                1,
                "Tone",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Triangle,
                    level: 0.5,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "Noise",
                ModuleKind::Noise(NoiseModule {
                    color: NoiseColor::White,
                    highpass_hz: 1_200.0,
                    lowpass_hz: 14_000.0,
                    level: 0.75,
                }),
            ),
            RackModule::new(
                3,
                "Amp Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::HoldDecay,
                    attack_ms: 0.3,
                    hold_ms: 0.0,
                    decay_ms: 150.0,
                    sustain: 0.0,
                    release_ms: 40.0,
                    trigger_mode: TriggerMode::OneShot,
                    delay_ms: 0.0,
                    bursts: 1,
                    burst_spacing_ms: 10.0,
                }),
            ),
            RackModule::new(4, "VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
        ],
        cables: vec![
            PatchCable::new(1, 1, 4, InPort::In, 1.0),
            PatchCable::new(2, 2, 4, InPort::In, 1.0),
            PatchCable::new(3, 3, 4, InPort::Cv, 1.0),
            PatchCable::new(4, 4, 5, InPort::In, 1.0),
        ],
    }
}

fn make_hat_zone(id: u16, name: &str, midi: u8, choke: Option<u8>, decay_ms: f32) -> RackZone {
    RackZone {
        id,
        name: name.into(),
        low_midi: midi,
        high_midi: midi,
        root_midi: 72,
        low_vel: 0.0,
        high_vel: 1.0,
        gain_db: 0.0,
        pan: 0.0,
        choke_group: choke,
        voice: VoiceSettings {
            pitch_tracking: PitchTracking::Fixed,
            ..VoiceSettings::default()
        },
        modules: vec![
            RackModule::new(
                1,
                "Noise",
                ModuleKind::Noise(NoiseModule {
                    color: NoiseColor::White,
                    highpass_hz: 6_500.0,
                    lowpass_hz: 18_000.0,
                    level: 0.85,
                }),
            ),
            RackModule::new(
                2,
                "HPF",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::HighPass,
                    cutoff_hz: 7_500.0,
                    resonance: 0.1,
                    drive: 0.0,
                    key_tracking: 0.0,
                    vel_amount: 0.0,
                    mix: 1.0,
                }),
            ),
            RackModule::new(
                3,
                "Amp Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::HoldDecay,
                    attack_ms: 0.2,
                    hold_ms: 0.0,
                    decay_ms,
                    sustain: 0.0,
                    release_ms: 20.0,
                    trigger_mode: TriggerMode::OneShot,
                    delay_ms: 0.0,
                    bursts: 1,
                    burst_spacing_ms: 10.0,
                }),
            ),
            RackModule::new(4, "VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
        ],
        cables: vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 2, 4, InPort::In, 1.0),
            PatchCable::new(3, 3, 4, InPort::Cv, 1.0),
            PatchCable::new(4, 4, 5, InPort::In, 1.0),
        ],
    }
}

#[test]
fn acid_303_accent_and_drumkit_rack_with_hat_choke_render_cleanly() {
    // 1. Verify AcidLowPass accented hit is brighter/hotter than unaccented hit.
    let mut acid = RackPatch::acid_303();
    {
        let flt = filter_mut(&mut acid);
        flt.cutoff_hz = 260.0;
        flt.resonance = 0.72;
        flt.drive = 0.55;
        flt.vel_amount = 2.8;
    }
    let render_single = |vel: f32| -> f64 {
        let mut s = note(1);
        s.velocity = vel;
        let p = rack_project(
            vec![s, Step::default(), Step::default(), Step::default()],
            acid.clone(),
        );
        let score = p.compile().unwrap();
        let report = render_measure_report(&score, Vec::new(), 1, 24_000).unwrap();
        report.output.peak[0]
    };
    let soft = render_single(0.45);
    let accent = render_single(0.98);
    assert!(
        accent > soft * 1.8,
        "Acid accented hit ({accent}) should punch noticeably harder than soft ({soft})"
    );

    // 2. Verify multi-zone drumkit Rack renders drums AND closed hat actually chokes open hat.
    let kit = RackPatch {
        kit_name: "Test Kit".into(),
        zones: vec![
            make_kick_zone(1, drum_midi::BD),
            make_snare_zone(2, drum_midi::SD),
            make_hat_zone(3, "CH", drum_midi::CH, Some(1), 40.0),
            make_hat_zone(4, "OH", drum_midi::OH, Some(1), 800.0),
        ],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 20_000.0,
        resonance: 0.0,
        panel_pins: Vec::new(),
    };
    kit.validate().unwrap();

    let build_kit_project = |steps: Vec<Step>| -> Project {
        let mut project = Project::empty();
        project.bpm = 128.0;
        project.bars = 1;
        project.root_midi = drum_midi::BD; // C2 = 36
        project.scale = (0..12).collect(); // chromatic so degrees map directly to kit pads
        project
            .mixer
            .channels
            .push(Channel::new(1, ChannelKind::Audio, "Drums"));
        project.instruments.push(InstrumentInstance {
            id: 1,
            name: "Test Kit".into(),
            rack: kit.clone(),
            channel: 1,
        });
        project.patterns.push(Pattern {
            id: 1,
            name: "Beat".into(),
            kind: PatternKind::Notes { steps },
        });
        project.parts.push(Part::new(1, "Kit", 1, 1));
        let mut lane = Lane::new(1, "Drums");
        lane.placements.push(Placement::new(1, 1, 0, 16));
        project.lanes.push(lane);
        project
    };

    // Full beat with BD, OH, CH (choking OH), SD.
    let mut steps = vec![Step::default(); 16];
    steps[0] = note(1); // BD (36)
    steps[1] = note(11); // OH (46)
    steps[2] = note(7); // CH (42, choke group 1)
    steps[4] = note(3); // SD (38)
    let score = build_kit_project(steps).compile().unwrap();
    let report = render_measure_report(&score, Vec::new(), 1, 24_000).unwrap();
    assert!(
        report.output.peak[0] > 0.15,
        "Drumkit Rack must render audible punch (peak={})",
        report.output.peak[0]
    );
    assert!(report.output.peak[0].is_finite());

    // Directly verify choke group 1 silences OH tail after CH triggers!
    let mut oh_only = vec![Step::default(); 16];
    oh_only[0] = note(11); // OH at step 0
    let mut oh_choked = oh_only.clone();
    oh_choked[1] = note(7); // CH at step 1 chokes OH (128 BPM -> 1 step = ~117 ms)

    let sr = 24_000u32;
    let (_, pcm_ring) = render_schedule_pcm(
        &[(0, &build_kit_project(oh_only).compile().unwrap())],
        0,
        8,
        Vec::new(),
        sr,
    )
    .unwrap();
    let (_, pcm_choked) = render_schedule_pcm(
        &[(0, &build_kit_project(oh_choked).compile().unwrap())],
        0,
        8,
        Vec::new(),
        sr,
    )
    .unwrap();
    // Inspect window from ~210..375 ms: CH (triggered at ~117 ms, 40 ms decay) is finished and OH was choked,
    // whereas unchoked OH (800 ms decay) still has strong energy!
    let win_rms = |pcm: &[[i16; 2]]| -> f64 {
        let start = 5_000;
        let end = 9_000.min(pcm.len());
        let sum: f64 = pcm[start..end]
            .iter()
            .map(|f| (f64::from(f[0]) / 32768.0).powi(2))
            .sum();
        (sum / (end - start) as f64).sqrt()
    };
    let unchoked_tail = win_rms(&pcm_ring);
    let choked_tail = win_rms(&pcm_choked);
    assert!(
        unchoked_tail > 0.003 && choked_tail < unchoked_tail * 0.1,
        "CH must choke OH in choke group 1: unchoked_tail={unchoked_tail}, choked_tail={choked_tail}"
    );
}

#[test]
fn per_channel_stem_export_writes_master_and_isolated_audio_stems() {
    let dir = tempfile::tempdir().unwrap();
    let project = Project::demo();
    let flags = cadence_core::mixer::LiveFlags::default();
    let expected_audio_channels = project
        .mixer
        .channels
        .iter()
        .filter(|c| c.kind == ChannelKind::Audio)
        .count();
    let summary =
        render_stems_with_samples_report(&project, &flags, Vec::new(), dir.path(), 0, 16, 24_000)
            .unwrap();
    assert!(summary.master_path.is_file());
    assert_eq!(summary.stem_paths.len(), expected_audio_channels);
    for stem in &summary.stem_paths {
        assert!(stem.is_file(), "Missing stem {}", stem.display());
        let reader = hound::WavReader::open(stem).unwrap();
        assert_eq!(reader.spec().channels, 2);
        assert_eq!(reader.spec().sample_rate, 24_000);
        assert!(reader.len() > 0);
    }
    assert!(summary.master_report.output.peak[0] > 0.05);
}

#[test]
fn unity_master_output_has_no_hidden_tanh_attenuation() {
    // A 0.50-amplitude sample panned hard Left with Master gain 1.0 renders at exact 0.50 peak.
    let sr = 24_000u32;
    let frames = 4800;
    let left: Box<[f32]> = (0..frames)
        .map(|i| 0.5 * (std::f64::consts::TAU * 200.0 * i as f64 / f64::from(sr)).sin() as f32)
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let sample = Arc::new(SampleData {
        path_label: "./sine.wav".into(),
        resolved_path: std::path::PathBuf::from("sine.wav"),
        name: "sine".into(),
        sample_rate: sr,
        channels: 1,
        frames,
        left,
        right: None,
        peaks: vec![(-0.5, 0.5); 64].into_boxed_slice(),
        meta: SampleMetadata::default(),
    });
    let mut p = Project::empty();
    p.bpm = 120.0;
    p.bars = 1;
    p.root_midi = 60;
    p.mixer.master.strip.gain = 1.0;
    p.mixer
        .channels
        .push(Channel::new(1, ChannelKind::Audio, "Ref"));
    let mut hit = note(1);
    hit.velocity = 1.0;
    hit.gate = 4.0;
    p.patterns.push(Pattern {
        id: 1,
        name: "Hit".into(),
        kind: PatternKind::Notes {
            steps: vec![
                hit.clone(),
                Step::default(),
                Step::default(),
                Step::default(),
            ],
        },
    });
    let mut rack = RackPatch::default_sampler();
    {
        let smp = sampler_mut(&mut rack);
        smp.path = Some("./sine.wav".into());
        smp.root_midi = 60;
        smp.one_shot = false;
    }
    {
        let flt = filter_mut(&mut rack);
        flt.cutoff_hz = 20_000.0;
        flt.resonance = 0.0;
    }
    {
        let env = amp_env_mut(&mut rack);
        // Smooth 20 ms onset (4 full cycles of 200 Hz sine) avoids causal step overshoot
        // through Out's 12 Hz DcBlocker while hitting exact 1.0 steady-state sustain.
        env.attack_ms = 20.0;
        env.decay_ms = 1.0;
        env.sustain = 1.0;
    }
    out_mut(&mut rack, 5).pan = -1.0; // pan_mono_equal_power(-1.0) = [1.0, 0.0]

    p.instruments.push(InstrumentInstance {
        id: 1,
        name: "RefSmp".into(),
        rack,
        channel: 1,
    });
    p.parts.push(Part::new(1, "RefPart", 1, 1));
    let mut lane = Lane::new(1, "Ref");
    lane.placements.push(Placement::new(1, 1, 0, 16));
    p.lanes.push(lane);

    let score = p.compile().unwrap();
    let key = module_sample_key(1, 1, 1);
    let report = render_measure_report(&score, vec![(key, sample)], 1, sr).unwrap();
    assert!(
        (report.output.peak[0] - 0.5).abs() < 0.01,
        "expected linear unity Master peak ~0.50, got {}",
        report.output.peak[0]
    );
    assert!(
        report.output.peak[1] < 1e-4,
        "hard-left pan must keep R silent"
    );
}

#[test]
fn multizone_rack_survives_live_edit_unclamped_gain_choke_and_filter_isolation() {
    let sr = 24_000u32;
    let sample = synthetic_sample(48_000, sr, 440.0);

    // Build multi-zone Rack with:
    // - Zone 1: Sample pad at MIDI 60 (choke group 1, pan -1.0, gain_db 0.0, coarse +24 st + fine 75 c)
    // - Zone 2: Layered Sample pad at MIDI 60 (choke group 1, pan +1.0, gain_db +6.0)
    // - Zone 3: HighPass Noise/Filter hat zone at MIDI 42 (cutoff 7500 Hz)
    let mut z1 = RackZone::sample_pad(1, "LayerL", 60, "./test.wav");
    z1.choke_group = Some(1);
    z1.pan = -1.0;
    if let Some(ModuleKind::Sampler(smp)) = z1.module_mut(1).map(|m| &mut m.kind) {
        smp.coarse_semitones = 24;
        smp.fine_cents = 75.0;
        smp.one_shot = true;
    }

    let mut z2 = RackZone::sample_pad(2, "LayerR", 60, "./test.wav");
    z2.choke_group = Some(1);
    z2.pan = 1.0;
    z2.gain_db = 6.0;
    if let Some(ModuleKind::Sampler(smp)) = z2.module_mut(1).map(|m| &mut m.kind) {
        smp.one_shot = true;
    }

    let z3 = make_hat_zone(3, "HatHPF", 42, Some(2), 180.0);

    let kit = RackPatch {
        kit_name: "TestKit".into(),
        zones: vec![z1, z2, z3],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 12.5,
        cutoff_hz: 20_000.0,
        resonance: 0.0,
        panel_pins: Vec::new(),
    };
    kit.validate().unwrap();

    let mut p = Project::empty();
    p.bpm = 120.0;
    p.bars = 1;
    p.root_midi = 60;
    p.mixer.master.strip.gain = 1.0;
    p.mixer
        .channels
        .push(Channel::new(1, ChannelKind::Audio, "Kit"));
    p.instruments.push(InstrumentInstance {
        id: 10,
        name: "Kit".into(),
        rack: kit,
        channel: 1,
    });
    let mut hit = note(1);
    hit.velocity = 1.0;
    hit.gate = 4.0;
    p.patterns.push(Pattern {
        id: 1,
        name: "Trig".into(),
        kind: PatternKind::Notes {
            steps: vec![
                hit.clone(),
                Step::default(),
                Step::default(),
                Step::default(),
            ],
        },
    });
    p.parts.push(Part::new(1, "KitPart", 1, 10));
    let mut lane = Lane::new(1, "Kit");
    lane.placements.push(Placement::new(1, 1, 0, 16));
    p.lanes.push(lane);

    let score = p.compile().unwrap();
    let bank = vec![
        (module_sample_key(10, 1, 1), Arc::clone(&sample)),
        (module_sample_key(10, 2, 1), Arc::clone(&sample)),
    ];
    let shared = Shared::new_arc();
    shared.playing.store(true, Ordering::Release);
    let mut engine = Engine::new(score.clone(), bank, sr, Arc::clone(&shared));

    // Render 400 frames -> LayerL (L) and LayerR (R) ring together and LayerR (+6 dB) is ~2x LayerL.
    let mut peak_l = 0.0_f32;
    let mut peak_r = 0.0_f32;
    for _ in 0..400 {
        let f = engine.next_frame();
        peak_l = peak_l.max(f[0].abs());
        peak_r = peak_r.max(f[1].abs());
    }
    assert!(
        peak_l > 0.2,
        "LayerL must not be choked by LayerR: L={peak_l}"
    );
    assert!(
        peak_r > peak_l * 1.6,
        "LayerR +6dB gain_db must boost unclamped at vel=1.0: L={peak_l}, R={peak_r}"
    );

    // Sample zone 1 tune (24.75 + 12.5 = 37.25 st) preserves full combined pitch range.
    let z1_voice = engine
        .voices()
        .iter()
        .find(|v| v.active && v.track_id == 10 && v.zone_id == 1)
        .expect("LayerL voice active");
    let expected_ratio = 2.0_f64.powf(37.25 / 12.0);
    let actual_ratio = z1_voice
        .modular
        .sampler_step_ratio(1)
        .expect("sampler module 1 ratio");
    assert!(
        (actual_ratio - expected_ratio).abs() < 0.02,
        "expected ±48st + fine_cents carry ratio {expected_ratio}, got {actual_ratio}"
    );

    // Publishing a live score edit (lowering Rack bus cutoff to 300 Hz) preserves ringing
    // sample-zone voices and leaves internal HighPass zone filter cutoffs intact.
    let mut edited = score;
    edited.bpm = 128.0;
    edited.instruments[0].rack.cutoff_hz = 300.0;
    engine.publish_score_update(edited);
    engine.poll_controls();
    let surviving_layers = engine
        .voices()
        .iter()
        .filter(|v| v.active && v.track_id == 10 && (v.zone_id == 1 || v.zone_id == 2))
        .count();
    assert_eq!(
        surviving_layers, 2,
        "live edit must keep both Rack sample-zone voices alive"
    );

    // Triggering HatHPF (MIDI 42) while rack.cutoff_hz = 300 Hz keeps internal HPF cutoff >= 7000 Hz.
    assert!(shared.push_event_for_id(1 | (42u64 << 11) | (65_535u64 << 18), 10));
    engine.poll_controls();
    for _ in 0..32 {
        engine.next_frame();
    }
    let hat_voice = engine
        .voices()
        .iter()
        .find(|v| v.active && v.track_id == 10 && v.zone_id == 3)
        .expect("HatHPF voice active");
    let hat_cutoff = hat_voice
        .modular
        .module_filter_cutoff_hz(2)
        .expect("hat HPF cutoff");
    assert!(
        hat_cutoff >= 7000.0,
        "rack bus cutoff 300 Hz must not crush internal HPF zone cutoff, got {hat_cutoff}"
    );
}

#[test]
fn end_rule_stop_sub_step_ordering_bpm_phase_and_continue_clamp() {
    // 1. EndRule::Stop does not insert a 16th-note silence gap when Play resumes.
    let sr = 16_000u32;
    let mut p = rack_project(vec![note(1); 16], RackPatch::default_synth());
    p.bars = 1;
    p.lanes[0].placements[0].length_steps = 16;
    p.end = EndRule::Stop;
    let score = p.compile().unwrap();
    let shared = Shared::new_arc();
    shared.playing.store(true, Ordering::Release);
    let mut engine = Engine::new(score, Vec::new(), sr, Arc::clone(&shared));
    // 16 steps * 2000 frames = 32000 frames -> stops right at frame 32000.
    for _ in 0..32_005 {
        engine.next_frame();
    }
    assert!(!shared.playing.load(Ordering::Acquire));
    assert_eq!(engine.play_step(), 0);
    // Resume playing -> step 0 fires on the first resumed frame.
    shared.playing.store(true, Ordering::Release);
    engine.next_frame();
    assert_eq!(
        engine.play_step(),
        1,
        "resuming after EndRule::Stop must trigger step 0 immediately with zero silence gap"
    );

    // 2. offset=0.9999 on step 0 fires strictly before step 1 downbeat on a Mono synth.
    let mut mono = RackPatch::default_synth();
    mono.zones[0].voice.voice_mode = VoiceMode::Mono;
    mono.zones[0].voice.glide_ms = 0.0;
    mono.zones[0].voice.legato = false;
    let mut s_order = rack_project(vec![Step::default(); 16], mono)
        .compile()
        .unwrap();
    let make_ev = |offset: f32, midi: u8| ScoreEvent {
        inst_idx: 0,
        part_id: Some(1),
        offset,
        degree: 1,
        accidental: 0,
        midi,
        velocity: 0.9,
        gate: 1.0,
        chord: Default::default(),
        chord_len: 0,
        lock_start: 0,
        lock_len: 0,
    };
    s_order.events = vec![make_ev(0.9999, 48), make_ev(0.0, 72)];
    s_order.step_ranges[0] = (0, 1);
    s_order.step_ranges[1] = (1, 2);
    let sh2 = Shared::new_arc();
    sh2.playing.store(true, Ordering::Release);
    let mut eng2 = Engine::new(s_order, Vec::new(), sr, Arc::clone(&sh2));
    // Step 0 at frame 0 queues offset=0.9999 for frame 1999; step 1 downbeat fires at frame 2000.
    for _ in 0..2002 {
        eng2.next_frame();
    }
    let active_mono = eng2.voices().iter().filter(|v| v.active).count();
    assert_eq!(active_mono, 1, "mono synth keeps strictly 1 voice");
    let mono_voice = eng2.voices().iter().find(|v| v.active).unwrap();
    assert!(
        (mono_voice.modular.current_note_pitch() - 72.0).abs() < 0.01,
        "step 1 downbeat (MIDI 72) must land after step 0 offset=0.9999 (MIDI 48), got {}",
        mono_voice.modular.current_note_pitch()
    );

    // 3. validate_render_schedule when schedule[0].0 > start_step renders all requested steps.
    let late_score = rack_project(vec![note(1); 16], RackPatch::default_synth())
        .compile()
        .unwrap();
    let (rep_late, _) = render_schedule_pcm(&[(8, &late_score)], 0, 16, Vec::new(), sr).unwrap();
    assert_eq!(
        rep_late.frames,
        16 * 2000,
        "schedule[0].0 > start_step must still render all requested steps"
    );

    // 4. EndRule::Continue clamps automation at len - 1 instead of wrapping to 0.
    let mut tail_rack = RackPatch::default_synth();
    {
        let env = amp_env_mut(&mut tail_rack);
        env.release_ms = 800.0;
        env.sustain = 1.0;
    }
    let mut p_cont = rack_project(vec![note(1); 16], tail_rack);
    p_cont.bars = 1;
    p_cont.lanes[0].placements[0].length_steps = 16;
    p_cont.end = EndRule::Continue;
    let mut s_cont = p_cont.compile().unwrap();
    s_cont
        .mixer
        .automations
        .push(cadence_core::mixer::MixerAutomation {
            target: cadence_core::mixer::MixerParamTarget::Channel {
                channel: ChannelRef::Channel(1),
                param: cadence_core::mixer::ChannelParam::Gain,
            },
            mode: ValueMode::Ramp,
            points: vec![
                cadence_core::mixer::MixerBreakpoint {
                    step: 0,
                    value: 0.0,
                },
                cadence_core::mixer::MixerBreakpoint {
                    step: 15,
                    value: 1.0,
                },
            ],
        });
    // Render steps 14..20 so notes from step 15 ring cleanly past arrangement end (step 16).
    let (_, pcm_past) = render_schedule_pcm(&[(0, &s_cont)], 14, 6, Vec::new(), sr).unwrap();
    // Inspect steps 17..19 (frames 6000..10000): automation holds end gain rather than wrapping to 0.
    let peak_past = pcm_past[6000..10_000]
        .iter()
        .flatten()
        .map(|s| s.unsigned_abs())
        .max()
        .unwrap_or(0);
    assert!(
        peak_past > 2000,
        "EndRule::Continue must hold end gain (1.0) past step 16 instead of wrapping to 0.0 (peak={peak_past})"
    );
}

#[test]
fn sampler_forward_loop_xfade_zero_loop_end_pingpong_and_long_oneshot() {
    let sr = 24_000u32;
    let key = module_sample_key(1, 1, 1);

    // 1. Forward loop with `loop_end == 0` loops full sample and `loop_xfade_ms > 0` wraps smoothly.
    let sample = synthetic_sample(2400, sr, 200.0); // exact 20 cycles in 2400 frames -> seamless loop
    let mut p = Project::empty();
    p.bpm = 120.0;
    p.bars = 2;
    p.root_midi = 60;
    p.mixer.master.strip.gain = 1.0;
    p.mixer
        .channels
        .push(Channel::new(1, ChannelKind::Audio, "Loop"));
    let mut hit = note(1);
    hit.velocity = 1.0;
    hit.gate = 8.0;
    p.patterns.push(Pattern {
        id: 1,
        name: "Hold".into(),
        kind: PatternKind::Notes {
            steps: {
                let mut v = vec![Step::default(); 16];
                v[0] = hit.clone();
                v
            },
        },
    });
    let mut rack = RackPatch::default_sampler();
    {
        let smp = sampler_mut(&mut rack);
        smp.path = Some("./test.wav".into());
        smp.sample_start = 0;
        smp.sample_end = 0;
        smp.loop_mode = SampleLoopMode::Forward;
        smp.loop_start = 0;
        smp.loop_end = 0; // Must default to `end` (2400), NOT collapse to 1!
        smp.loop_xfade_ms = 10.0;
        smp.one_shot = false;
    }
    {
        let env = amp_env_mut(&mut rack);
        env.attack_ms = 0.5;
        env.sustain = 1.0;
    }
    {
        let flt = filter_mut(&mut rack);
        flt.cutoff_hz = 20_000.0;
        flt.resonance = 0.0;
    }
    p.instruments.push(InstrumentInstance {
        id: 1,
        name: "LoopSmp".into(),
        rack: rack.clone(),
        channel: 1,
    });
    p.parts.push(Part::new(1, "LoopPart", 1, 1));
    let mut lane = Lane::new(1, "Loop");
    lane.placements.push(Placement::new(1, 1, 0, 32));
    p.lanes.push(lane);

    let score = p.compile().unwrap();
    let (_, pcm) =
        render_schedule_pcm(&[(0, &score)], 0, 8, vec![(key, Arc::clone(&sample))], sr).unwrap();
    // Measure max sample-to-sample delta across loop wrap regions (well after 0.5ms attack).
    let max_delta = pcm[200..pcm.len() - 400]
        .windows(2)
        .map(|w| (i32::from(w[1][0]) - i32::from(w[0][0])).abs())
        .max()
        .unwrap_or(0);
    // 200 Hz sine at 24 kHz has max step delta ~ 2*pi*200/24000 * amp ≈ 0.052 * amp (< 2000 i16).
    assert!(
        max_delta < 3000,
        "Forward loop xfade with loop_end=0 must stay click-free across wraps (max_delta={max_delta})"
    );

    // 2. PingPong with loop_end=0 and +12 st pitch shift stays finite, smooth, and oscillating.
    let mut pp_proj = p.clone();
    {
        let smp = sampler_mut(&mut pp_proj.instruments[0].rack);
        smp.loop_mode = SampleLoopMode::PingPong;
        smp.coarse_semitones = 12;
    }
    let pp_score = pp_proj.compile().unwrap();
    let (pp_rep, _) = render_schedule_pcm(
        &[(0, &pp_score)],
        0,
        8,
        vec![(key, Arc::clone(&sample))],
        sr,
    )
    .unwrap();
    assert!(pp_rep.output.rms[0] > 0.1 && pp_rep.output.peak[0].is_finite());

    // 3. One-shot sample longer than 12 seconds plays past 12 s to its true end.
    let long_sr = 8_000u32;
    let long_frames = long_sr as usize * 14; // 14 seconds
    let long_sample = synthetic_sample(long_frames, long_sr, 110.0);
    let mut long_proj = p;
    long_proj.bpm = 60.0; // 1 bar = 4 s -> 4 bars = 16 s
    long_proj.bars = 4;
    long_proj.lanes[0].placements[0].length_steps = 64;
    long_proj.patterns[0].kind = PatternKind::Notes {
        steps: {
            let mut v = vec![Step::default(); 64];
            v[0] = hit;
            v
        },
    };
    let mut long_rack = RackPatch::default_sampler();
    {
        let smp = sampler_mut(&mut long_rack);
        smp.path = Some("./test.wav".into());
        smp.one_shot = true;
        smp.loop_mode = SampleLoopMode::Off;
    }
    {
        let env = amp_env_mut(&mut long_rack);
        env.attack_ms = 1.0;
        env.sustain = 1.0;
    }
    {
        let flt = filter_mut(&mut long_rack);
        flt.cutoff_hz = 20_000.0;
        flt.resonance = 0.0;
    }
    long_proj.instruments[0].rack = long_rack;
    let long_score = long_proj.compile().unwrap();
    let (_, long_pcm) = render_schedule_pcm(
        &[(0, &long_score)],
        0,
        64,
        vec![(key, long_sample)],
        long_sr,
    )
    .unwrap();
    // At t = 13.0 s (frame 104_000), the 14 s one-shot MUST still be playing!
    let at_13s_peak = long_pcm[104_000..104_800]
        .iter()
        .map(|f| f[0].unsigned_abs())
        .max()
        .unwrap_or(0);
    // At t = 15.0 s (frame 120_000), the 14 s one-shot MUST have terminated cleanly!
    let at_15s_peak = long_pcm[120_000..120_800]
        .iter()
        .map(|f| f[0].unsigned_abs())
        .max()
        .unwrap_or(0);
    assert!(
        at_13s_peak > 4000,
        "14 s one_shot sample must play past 12 s (at 13 s peak={at_13s_peak})"
    );
    assert!(
        at_15s_peak < 10,
        "14 s one_shot sample must finish cleanly after 14 s (at 15 s peak={at_15s_peak})"
    );
}

#[test]
fn render_stems_skips_audio_channels_under_muted_parent_group() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = Project::empty();
    p.bpm = 120.0;
    p.bars = 1;
    let mut ch_a = Channel::new(1, ChannelKind::Audio, "LeadA");
    ch_a.output = ChannelRef::Channel(10);
    let ch_b = Channel::new(2, ChannelKind::Audio, "LeadB");
    let grp = Channel::new(10, ChannelKind::Group, "MutedGroup");
    p.mixer.channels = vec![ch_a, ch_b, grp];
    p.instruments.push(InstrumentInstance {
        id: 1,
        name: "A".into(),
        rack: RackPatch::default_synth(),
        channel: 1,
    });
    p.instruments.push(InstrumentInstance {
        id: 2,
        name: "B".into(),
        rack: RackPatch::default_synth(),
        channel: 2,
    });
    p.patterns.push(Pattern {
        id: 1,
        name: "P".into(),
        kind: PatternKind::Notes {
            steps: vec![note(1); 16],
        },
    });
    p.parts.push(Part::new(1, "PA", 1, 1));
    p.parts.push(Part::new(2, "PB", 1, 2));
    let mut l1 = Lane::new(1, "A");
    l1.placements.push(Placement::new(1, 1, 0, 16));
    let mut l2 = Lane::new(2, "B");
    l2.placements.push(Placement::new(2, 2, 0, 16));
    p.lanes = vec![l1, l2];

    let mut flags = cadence_core::mixer::LiveFlags::default();
    flags
        .set_channel_muted(ChannelRef::Channel(10), true)
        .unwrap();
    let summary =
        render_stems_with_samples_report(&p, &flags, Vec::new(), dir.path(), 0, 16, 24_000)
            .unwrap();
    assert_eq!(
        summary.stem_paths.len(),
        1,
        "muted parent group must exclude child Audio stem: {:?}",
        summary.stem_paths
    );
    assert!(summary.stem_paths[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .contains("LeadB"));
}

#[test]
fn render_report_records_float_channel_headroom_and_compressor_limiter_gr_stats() {
    use cadence_core::mixer::{CompressorPatch, DrivePatch, EffectKind, LimiterPatch};

    let mut p = Project::empty();
    p.bpm = 120.0;
    p.bars = 2; // 4 seconds (> 1.5s hold window) to prove session maxima never decay!
    p.mixer.master.strip.gain = 1.0;

    let comp_id = 101_u64;
    let lim_id = 102_u64;

    let mut hot_ch = Channel::new(1, ChannelKind::Audio, "HotLead");
    hot_ch.strip.gain = 1.0;
    hot_ch.inserts = vec![
        EffectDevice {
            id: 100,
            name: "Boost".into(),
            bypassed: false,
            kind: EffectKind::Drive(DrivePatch {
                drive: 0.4,
                tone_hz: 16_000.0,
                output_db: 6.0,
                mix: 1.0,
                ..DrivePatch::default()
            }),
        },
        EffectDevice {
            id: comp_id,
            name: "Lead Comp".into(),
            bypassed: false,
            kind: EffectKind::Compressor(CompressorPatch {
                threshold_db: -18.0,
                ratio: 6.0,
                attack_ms: 1.0,
                release_ms: 60.0,
                makeup_db: 14.0,
                mix: 1.0,
            }),
        },
    ];
    p.mixer.channels = vec![hot_ch];
    p.mixer.master.inserts = vec![EffectDevice {
        id: lim_id,
        name: "Master Limiter".into(),
        bypassed: false,
        kind: EffectKind::Limiter(LimiterPatch {
            ceiling_db: -1.5,
            input_drive_db: 4.0,
            release_ms: 60.0,
            mix: 1.0,
        }),
    }];
    let mut lead_rack = RackPatch::default_synth();
    {
        let env = amp_env_mut(&mut lead_rack);
        env.attack_ms = 1.0;
        env.sustain = 0.9;
    }
    p.instruments.push(InstrumentInstance {
        id: 1,
        name: "Lead".into(),
        rack: lead_rack,
        channel: 1,
    });
    // Loud hit on bar 1 only, followed by >2.5s of quiet/silence so 1.5s live hold decays!
    let mut steps = vec![Step::default(); 32];
    for step in steps.iter_mut().take(4) {
        *step = Step {
            active: true,
            degree: 1,
            velocity: 1.0,
            gate: 0.9,
            ..Step::default()
        };
    }
    p.patterns.push(Pattern {
        id: 1,
        name: "Burst".into(),
        kind: PatternKind::Notes { steps },
    });
    p.parts.push(Part::new(1, "BurstPart", 1, 1));
    let mut lane = Lane::new(1, "Lead");
    lane.placements.push(Placement::new(1, 1, 0, 32));
    p.lanes = vec![lane];

    let score = p.compile().unwrap();
    let report = render_measure_report(&score, Vec::new(), 2, 24_000).unwrap();

    // Channel report preserves session maxima even after >2.5s of silence at end of bar 2!
    let ch1 = report
        .channels
        .iter()
        .find(|c| c.channel == ChannelRef::Channel(1))
        .expect("ch1 report");
    let mst = report
        .channels
        .iter()
        .find(|c| c.channel == ChannelRef::Master)
        .expect("master report");
    assert!(
        ch1.output_peak_hold[0] > 1.0,
        "ch1 session output peak should exceed unity: {ch1:?}"
    );
    assert!(
        ch1.hot && !ch1.clipped,
        "internal float bus must be hot=true, clipped=false: {ch1:?}"
    );
    assert!(
        mst.hot && !mst.clipped,
        "Master Limiter tamed hot bus: master must be hot=true, clipped=false: {mst:?}"
    );
    assert!(
        mst.input_peak_hold[0] > 1.0 && mst.output_peak_hold[0] <= 0.86,
        "Master input > 1.0, output <= -1.5 dBFS ceiling: {mst:?}"
    );

    // Effect report captures accurate max_gr_db & avg_gr_db for BOTH Compressor and Limiter!
    let comp_rep = report
        .effects
        .iter()
        .find(|e| e.device_id == comp_id)
        .expect("compressor render effect report");
    let lim_rep = report
        .effects
        .iter()
        .find(|e| e.device_id == lim_id)
        .expect("limiter render effect report");
    assert_eq!(comp_rep.channel, ChannelRef::Channel(1));
    assert!(matches!(comp_rep.kind, EffectKind::Compressor(_)));
    assert!(
        comp_rep.max_gr_db > 2.0
            && comp_rep.avg_gr_db > 0.0
            && comp_rep.avg_gr_db <= comp_rep.max_gr_db,
        "compressor GR stats: {comp_rep:?}"
    );
    assert_eq!(lim_rep.channel, ChannelRef::Master);
    assert!(matches!(lim_rep.kind, EffectKind::Limiter(_)));
    assert!(
        lim_rep.max_gr_db > 2.0
            && lim_rep.avg_gr_db > 0.0
            && lim_rep.avg_gr_db <= lim_rep.max_gr_db,
        "limiter GR stats: {lim_rep:?}"
    );
}

#[test]
fn silent_runtime_audio_tap_ring_captures_live_stereo_and_stops_cleanly() {
    let score = Project::demo().compile().unwrap();
    let rt = AudioRuntime::start(score, true).unwrap();
    assert_eq!(rt.sample_rate(), 48_000);

    let tap = rt.tap_handle();

    // Disabled by default: draining must return zero frames even while playing.
    assert!(rt.transport(TransportIntent::Play { playing: true }));
    std::thread::sleep(Duration::from_millis(30));
    let mut buf = Vec::new();
    let dropped = tap.drain(&mut buf);
    assert_eq!(dropped, 0);
    assert!(
        buf.is_empty(),
        "disabled tap ring must produce zero frames, got {}",
        buf.len()
    );

    // Enable tap and capture live playing audio.
    tap.start();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut total_dropped = 0u64;
    while buf.len() < 4_800 && Instant::now() < deadline {
        total_dropped += tap.drain(&mut buf);
        std::thread::sleep(Duration::from_millis(10));
    }
    tap.stop();
    assert!(
        buf.len() >= 4_800,
        "expected >= 4800 tapped stereo frames, got {}",
        buf.len()
    );
    assert_eq!(total_dropped, 0, "no frames should drop during brisk drain");
    let peak = buf
        .iter()
        .flat_map(|f| [f[0].abs(), f[1].abs()])
        .fold(0.0_f32, f32::max);
    assert!(
        peak > 0.01 && peak.is_finite(),
        "expected audible finite tapped signal, got peak={peak}"
    );

    // After stop + draining any in-flight block, no further frames accumulate.
    let mut tail = Vec::new();
    std::thread::sleep(Duration::from_millis(20));
    let _ = tap.drain(&mut tail);
    tail.clear();
    std::thread::sleep(Duration::from_millis(30));
    let _ = tap.drain(&mut tail);
    assert!(
        tail.is_empty(),
        "stopped tap ring must not accumulate further frames, got {}",
        tail.len()
    );
}

#[test]
fn resample_stereo_and_write_wav_16bit_preserve_pitch_and_clamping() {
    let src_sr = 48_000u32;
    let dst_sr = 24_000u32;
    let freq = 440.0_f64;
    let src: Vec<[f32; 2]> = (0..4_800)
        .map(|i| {
            let t = i as f64 / f64::from(src_sr);
            let s = (2.0 * std::f64::consts::PI * freq * t).sin() as f32 * 0.5;
            [s, -s]
        })
        .collect();
    let resampled = resample_stereo(&src, src_sr, dst_sr);
    assert_eq!(resampled.len(), 2_400);
    // Mid-window sample accuracy against analytical 440 Hz @ 24 kHz sine.
    let max_err = (200..2_200)
        .map(|i| {
            let t = i as f64 / f64::from(dst_sr);
            let expected = (2.0 * std::f64::consts::PI * freq * t).sin() as f32 * 0.5;
            (resampled[i][0] - expected)
                .abs()
                .max((resampled[i][1] + expected).abs())
        })
        .fold(0.0_f32, f32::max);
    assert!(
        max_err < 1e-3,
        "48k->24k sinc resampler error too high: {max_err}"
    );

    let dir = tempfile::tempdir().unwrap();
    let wav_path = dir.path().join("sub/tapped.wav");
    let report = write_stereo_wav_16bit(&wav_path, dst_sr, &resampled, true).unwrap();
    assert_eq!(report.frames, 2_400);
    assert!(report.peak[0] > 0.45 && report.peak[0] <= 0.52);
    let reader = hound::WavReader::open(&wav_path).unwrap();
    assert_eq!(reader.spec().sample_rate, dst_sr);
    assert_eq!(reader.spec().channels, 2);
    assert_eq!(reader.duration(), 2_400);
}

#[test]
fn plock_vs_modlane_precedence_across_module_cable_zone_and_rack_params() {
    let mut p = Project::empty();
    p.bpm = 120.0;
    p.bars = 2;
    p.end = EndRule::Loop;
    p.mixer.master.strip.gain = 1.0;
    p.mixer
        .channels
        .push(Channel::new(1, ChannelKind::Audio, "Smp"));
    p.patterns.push(Pattern {
        id: 1,
        name: "Hit".into(),
        kind: PatternKind::Notes {
            steps: vec![note(1), Step::default(), Step::default(), Step::default()],
        },
    });
    // Per-hit P-lock on Filter.Cutoff (locks cutoff at ~500 Hz) and Zone.Pan (-0.5).
    p.patterns.push(Pattern {
        id: 2,
        name: "CutLock".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 4,
            mode: ValueMode::Hold,
            points: vec![ValuePoint {
                step: 0,
                value: ModuleParam::Cutoff.normalize(500.0),
            }],
        },
    });
    p.patterns.push(Pattern {
        id: 5,
        name: "ZonePanLock".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 4,
            mode: ValueMode::Hold,
            points: vec![ValuePoint {
                step: 0,
                value: ZoneParam::Pan.normalize(-0.5),
            }],
        },
    });
    // Continuous modulation trying to push Cutoff -> 12 kHz (must lose to P-lock!),
    // Resonance -> 0.6 (unlocked -> must win!), Zone.Pan -> +0.8 (must lose to P-lock!),
    // and Rack.Gain -> +3 dB (unlocked -> must win!).
    p.patterns.push(Pattern {
        id: 3,
        name: "CutMod".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 4,
            mode: ValueMode::Hold,
            points: vec![ValuePoint {
                step: 0,
                value: ModuleParam::Cutoff.normalize(12_000.0),
            }],
        },
    });
    p.patterns.push(Pattern {
        id: 4,
        name: "ResMod".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 4,
            mode: ValueMode::Hold,
            points: vec![ValuePoint {
                step: 0,
                value: ModuleParam::Resonance.normalize(0.6),
            }],
        },
    });
    p.patterns.push(Pattern {
        id: 6,
        name: "ZonePanMod".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 4,
            mode: ValueMode::Hold,
            points: vec![ValuePoint {
                step: 0,
                value: ZoneParam::Pan.normalize(0.8),
            }],
        },
    });
    p.patterns.push(Pattern {
        id: 7,
        name: "RackGainMod".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 4,
            mode: ValueMode::Hold,
            points: vec![ValuePoint {
                step: 0,
                value: RackParam::Gain.normalize(3.0),
            }],
        },
    });

    let mut rack = RackPatch::default_sampler();
    {
        let smp = sampler_mut(&mut rack);
        smp.path = Some("./test.wav".into());
        smp.one_shot = true;
    }
    {
        let flt = filter_mut(&mut rack);
        flt.cutoff_hz = 16_000.0;
        flt.resonance = 0.1;
    }
    out_mut(&mut rack, 5).pan = -0.25;

    p.instruments.push(InstrumentInstance {
        id: 1,
        name: "Smp".into(),
        rack,
        channel: 1,
    });
    let mut part = Part::new(1, "HitPart", 1, 1);
    part.parameter_links.push(ParameterLink {
        target: InstrumentParam::Module {
            zone: 1,
            module: 2,
            param: ModuleParam::Cutoff,
        },
        pattern: 2,
    });
    part.parameter_links.push(ParameterLink {
        target: InstrumentParam::Zone {
            zone: 1,
            param: ZoneParam::Pan,
        },
        pattern: 5,
    });
    p.parts.push(part);
    p.modulations.push(Modulation {
        id: 1,
        name: "CutMod".into(),
        target: ModulationTarget::Instrument {
            instrument: 1,
            parameter: InstrumentParam::Module {
                zone: 1,
                module: 2,
                param: ModuleParam::Cutoff,
            },
        },
        pattern: 3,
    });
    p.modulations.push(Modulation {
        id: 2,
        name: "ResMod".into(),
        target: ModulationTarget::Instrument {
            instrument: 1,
            parameter: InstrumentParam::Module {
                zone: 1,
                module: 2,
                param: ModuleParam::Resonance,
            },
        },
        pattern: 4,
    });
    p.modulations.push(Modulation {
        id: 3,
        name: "ZonePanMod".into(),
        target: ModulationTarget::Instrument {
            instrument: 1,
            parameter: InstrumentParam::Zone {
                zone: 1,
                param: ZoneParam::Pan,
            },
        },
        pattern: 6,
    });
    p.modulations.push(Modulation {
        id: 4,
        name: "RackGainMod".into(),
        target: ModulationTarget::Instrument {
            instrument: 1,
            parameter: InstrumentParam::Rack {
                param: RackParam::Gain,
            },
        },
        pattern: 7,
    });
    let mut lane = Lane::new(1, "Smp");
    lane.placements.push(Placement::new(1, 1, 0, 32));
    p.lanes = vec![lane];

    let score = p.compile().unwrap();
    let shared = Shared::new_arc();
    shared.playing.store(true, Ordering::Release);
    let key = module_sample_key(1, 1, 1);
    let bank = vec![(key, synthetic_sample(24_000, 24_000, 220.0))];
    let mut engine = Engine::new(score, bank, 24_000, shared);
    // Run across multiple control blocks so both trigger and continuous modulation execute.
    for _ in 0..512 {
        engine.next_frame();
    }
    let active_voice = engine
        .voices()
        .iter()
        .find(|v| v.active)
        .expect("sampler voice active");

    // Locked Filter.Cutoff (~500 Hz) beats continuous ModLane (12 kHz)!
    let flt_cutoff = active_voice
        .modular
        .module_filter_cutoff_hz(2)
        .expect("filter cutoff");
    assert!(
        (flt_cutoff - 500.0).abs() < 5.0,
        "locked filter cutoff must stay ~500 Hz, got {flt_cutoff}"
    );
    // Unlocked Filter.Resonance tracks continuous ModLane (0.6)!
    let flt_params = active_voice
        .modular
        .module_target_params(2)
        .expect("filter target params");
    match flt_params {
        crate::modular_voice::ModuleParams::Filter(f) => {
            assert!(
                (f.resonance - 0.6).abs() < 1e-3,
                "unlocked resonance must track 0.6, got {}",
                f.resonance
            );
        }
        other => panic!("expected Filter params, got {other:?}"),
    }
    // Locked Zone.Pan (-0.5) beats continuous Zone.Pan ModLane (+0.8)!
    assert!(
        (active_voice.zone_pan + 0.5).abs() < 1e-3,
        "locked zone pan must stay -0.5, got {}",
        active_voice.zone_pan
    );
    // Unlocked Rack.Gain tracks continuous Rack.Gain ModLane (+3 dB -> ~1.4125 lin)!
    let expected_gain = cadence_core::dsp::db_to_lin(3.0);
    assert!(
        (active_voice.gain_scale.target() as f32 - expected_gain).abs() < 0.02,
        "unlocked Rack.Gain must track +3 dB ({expected_gain}), got {}",
        active_voice.gain_scale.target()
    );
    // Base Out.Pan (-0.25) untouched!
    match active_voice
        .modular
        .module_target_params(5)
        .expect("out params")
    {
        crate::modular_voice::ModuleParams::Out(o) => {
            assert!((o.pan + 0.25).abs() < 1e-6);
        }
        other => panic!("expected Out params, got {other:?}"),
    }
}

#[test]
fn audio_engine_next_frame_is_strictly_zero_heap_alloc_under_triggers_chokes_and_modlanes() {
    // Combine Project::demo() (multi-track modular synth + bass + drum kit with chokes + ModLanes)
    // and verify Engine::next_frame() performs ZERO heap allocations / frees / reallocs!
    let score = Project::demo().compile().unwrap();
    let shared = Shared::new_arc();
    shared.playing.store(true, Ordering::Release);
    let mut engine = Engine::new(score, Vec::new(), 24_000, shared);

    crate::mix_engine::tests::assert_no_heap_operations(|| {
        for _ in 0..8_000 {
            let frame = engine.next_frame();
            assert!(frame[0].is_finite() && frame[1].is_finite());
        }
    });
}

#[test]
fn callback_watchdog_overload_fades_silences_latches_and_recovers_on_play_and_reset() {
    use crate::audio::runtime::{
        overload_fade_frames, render_output_slice_with_elapsed, CallbackWatchdog,
    };
    use cadence_core::mixer::{DelayPatch, EffectKind, ReverbPatch};
    use cadence_core::runtime::{AudioErrorCode, AudioStreamStatus};

    let sr = 48_000u32;
    let frames = 512usize;
    let channels = 2usize;
    let budget = Duration::from_secs_f64(frames as f64 / f64::from(sr));

    let mut rack = RackPatch::default_synth();
    {
        let env = amp_env_mut(&mut rack);
        env.attack_ms = 1.0;
        env.sustain = 1.0;
        env.release_ms = 200.0;
    }
    let mut p = rack_project(vec![note(1); 16], rack);
    p.mixer.master.inserts.push(EffectDevice {
        id: 10,
        name: "Delay".into(),
        bypassed: false,
        kind: EffectKind::Delay(DelayPatch {
            mix: 0.4,
            feedback: 0.65,
            ..DelayPatch::default()
        }),
    });
    p.mixer.master.inserts.push(EffectDevice {
        id: 11,
        name: "Verb".into(),
        bypassed: false,
        kind: EffectKind::Reverb(ReverbPatch {
            mix: 0.45,
            decay_seconds: 2.0,
            ..ReverbPatch::default()
        }),
    });
    let score = p.compile().unwrap();

    let shared = Shared::new_arc();
    shared.playing.store(true, Ordering::Release);
    let mut engine = Engine::new(score, Vec::new(), sr, Arc::clone(&shared));
    let mut watchdog = CallbackWatchdog::new();
    let mut buf = vec![0.0_f32; frames * channels];

    // Warm up normal callbacks under budget: audible output, running health, zero xruns.
    for _ in 0..4 {
        render_output_slice_with_elapsed(
            &mut engine,
            &mut watchdog,
            &shared,
            &mut buf,
            channels,
            sr,
            || budget.mul_f64(0.25),
        );
    }
    let warm_peak = buf.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
    assert!(warm_peak > 0.05, "expected audible playback: {warm_peak}");
    let h = shared.read_health();
    assert_eq!(h.status, AudioStreamStatus::Running);
    assert_eq!(h.xrun_count, 0);
    assert!(!h.overloaded);
    assert_eq!(h.buffer_frames, frames as u32);
    assert!(h.cpu_load > 0.15 && h.peak_cpu_load >= h.cpu_load);

    // Four consecutive overruns followed by one normal block must NOT trip the circuit breaker.
    for _ in 0..4 {
        render_output_slice_with_elapsed(
            &mut engine,
            &mut watchdog,
            &shared,
            &mut buf,
            channels,
            sr,
            || budget.mul_f64(1.35),
        );
    }
    assert_eq!(shared.read_health().xrun_count, 4);
    assert!(!shared.read_health().overloaded);
    render_output_slice_with_elapsed(
        &mut engine,
        &mut watchdog,
        &shared,
        &mut buf,
        channels,
        sr,
        || budget.mul_f64(0.2),
    );
    assert!(!shared.read_health().overloaded);

    // Five consecutive overruns MUST trip the circuit breaker with click-free fade-out to exact 0.0.
    // Run the tripping and fast-path blocks under strict zero-heap-allocation checking.
    let mut max_fade_step = 0.0_f32;
    crate::mix_engine::tests::assert_no_heap_operations(|| {
        for _ in 0..5 {
            render_output_slice_with_elapsed(
                &mut engine,
                &mut watchdog,
                &shared,
                &mut buf,
                channels,
                sr,
                || budget.mul_f64(1.4),
            );
        }
        let fade_start = (frames - overload_fade_frames(sr)) * channels;
        for f in (fade_start..buf.len() - channels).step_by(channels) {
            let dl = (buf[f + channels] - buf[f]).abs();
            let dr = (buf[f + channels + 1] - buf[f + 1]).abs();
            max_fade_step = max_fade_step.max(dl).max(dr);
        }
        // Final frame of the tripping block must be exact silence (0.0, 0.0).
        assert_eq!(buf[buf.len() - 2], 0.0);
        assert_eq!(buf[buf.len() - 1], 0.0);

        // Next callback while latched must overwrite stale DMA contents with exact 0.0 (fast path).
        buf.fill(0.77);
        render_output_slice_with_elapsed(
            &mut engine,
            &mut watchdog,
            &shared,
            &mut buf,
            channels,
            sr,
            || Duration::from_micros(5),
        );
        assert!(buf.iter().all(|&s| s == 0.0));
    });

    assert!(
        max_fade_step < 0.25,
        "overload tail fade should stay smooth without a hard click, max_step={max_fade_step}"
    );
    let tripped = shared.read_health();
    assert_eq!(tripped.status, AudioStreamStatus::Overloaded);
    assert!(tripped.overloaded);
    assert!(!tripped.failed);
    assert_eq!(tripped.error_code, AudioErrorCode::OverloadCircuitBreaker);
    assert_eq!(tripped.xrun_count, 9);
    assert!(!shared.playing.load(Ordering::Acquire));
    assert!(
        engine.voices().iter().all(|v| !v.active),
        "overload silence_in_place must clear all active voices"
    );

    // Recover via TransportIntent::Play { playing: true } -> clears latch and resumes audible audio!
    assert!(shared.push_transport(TransportIntent::Play { playing: true }));
    // First block right after resume (before next 16th-note step arrives) confirms reverb/delay tails were silenced.
    render_output_slice_with_elapsed(
        &mut engine,
        &mut watchdog,
        &shared,
        &mut buf,
        channels,
        sr,
        || budget.mul_f64(0.2),
    );
    let immediate_tail_peak = buf.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
    assert!(
        immediate_tail_peak < 1e-4,
        "silence_in_place must clear ringing delay/reverb tails: {immediate_tail_peak}"
    );
    // Advance past one 16th-note step (6000 frames @ 120 BPM / 48 kHz) so the next step fires cleanly.
    let mut resumed_peak = 0.0_f32;
    for _ in 0..14 {
        render_output_slice_with_elapsed(
            &mut engine,
            &mut watchdog,
            &shared,
            &mut buf,
            channels,
            sr,
            || budget.mul_f64(0.2),
        );
        resumed_peak = resumed_peak.max(buf.iter().map(|s| s.abs()).fold(0.0_f32, f32::max));
    }
    let resumed = shared.read_health();
    assert_eq!(resumed.status, AudioStreamStatus::Running);
    assert!(!resumed.overloaded);
    assert_eq!(resumed.error_code, AudioErrorCode::None);
    assert!(shared.playing.load(Ordering::Acquire));
    assert!(
        resumed_peak > 0.05,
        "resumed playback after overload must be audible: {resumed_peak}"
    );

    // Severe single-block stall (> 4x budget) while playing trips immediately on a single callback.
    render_output_slice_with_elapsed(
        &mut engine,
        &mut watchdog,
        &shared,
        &mut buf,
        channels,
        sr,
        || budget.mul_f64(4.5),
    );
    assert!(shared.read_health().overloaded);
    assert_eq!(buf[buf.len() - 2], 0.0);
    assert_eq!(buf[buf.len() - 1], 0.0);

    // Recover via TransportIntent::Reset { step: 0 } -> clears latch cleanly while staying paused.
    assert!(shared.push_transport(TransportIntent::Reset { step: 0 }));
    render_output_slice_with_elapsed(
        &mut engine,
        &mut watchdog,
        &shared,
        &mut buf,
        channels,
        sr,
        || budget.mul_f64(0.1),
    );
    let after_reset = shared.read_health();
    assert_eq!(after_reset.status, AudioStreamStatus::Running);
    assert!(!after_reset.overloaded);
    assert_eq!(after_reset.error_code, AudioErrorCode::None);
    assert!(!shared.playing.load(Ordering::Acquire));

    // A single >4x stall while stopped and completely silent must NOT false-trip (sleep/wake safety).
    for _ in 0..2 {
        render_output_slice_with_elapsed(
            &mut engine,
            &mut watchdog,
            &shared,
            &mut buf,
            channels,
            sr,
            || budget.mul_f64(0.05),
        );
    }
    render_output_slice_with_elapsed(
        &mut engine,
        &mut watchdog,
        &shared,
        &mut buf,
        channels,
        sr,
        || budget.mul_f64(5.0),
    );
    assert!(
        !shared.read_health().overloaded,
        "single idle silent stall must not false-trip circuit breaker"
    );
}

#[test]
fn render_output_slice_handles_odd_block_sizes_mono_downmix_and_multichannel_zero_padding() {
    use crate::audio::runtime::{render_output_slice_with_elapsed, CallbackWatchdog};

    let sr = 48_000u32;
    let mut rack = RackPatch::default_synth();
    rack.pan = -0.5;
    let score = rack_project(vec![note(1); 16], rack).compile().unwrap();

    // 1. Variable & odd block sizes (32, 64, 137, 512, 2048) stay finite and track buffer_frames.
    let shared = Shared::new_arc();
    shared.playing.store(true, Ordering::Release);
    let mut engine = Engine::new(score.clone(), Vec::new(), sr, Arc::clone(&shared));
    let mut watchdog = CallbackWatchdog::new();
    let mut total_energy = 0.0_f64;
    for &frames in &[32usize, 64, 137, 512, 2048, 137, 32] {
        let mut data = vec![0.99_f32; frames * 2];
        let budget = Duration::from_secs_f64(frames as f64 / f64::from(sr));
        render_output_slice_with_elapsed(
            &mut engine,
            &mut watchdog,
            &shared,
            &mut data,
            2,
            sr,
            || budget.mul_f64(0.15),
        );
        assert_eq!(shared.read_health().buffer_frames, frames as u32);
        for &s in &data {
            assert!(s.is_finite() && (-1.0..=1.0).contains(&s));
            total_energy += f64::from(s * s);
        }
    }
    assert!(
        total_energy > 1.0,
        "variable block sequence must produce audible energy"
    );

    // Tiny 32-frame block severe stall still fades smoothly to exact 0.0 at final frame.
    let mut tiny = vec![0.5_f32; 32 * 2];
    let tiny_budget = Duration::from_secs_f64(32.0 / f64::from(sr));
    render_output_slice_with_elapsed(
        &mut engine,
        &mut watchdog,
        &shared,
        &mut tiny,
        2,
        sr,
        || tiny_budget.mul_f64(5.0),
    );
    assert!(shared.read_health().overloaded);
    assert_eq!(tiny[tiny.len() - 2], 0.0);
    assert_eq!(tiny[tiny.len() - 1], 0.0);

    // 2. Mono downmix (1 ch) equals exact (L + R) * 0.5 of stereo render; 8 ch zero-pads channels 2..8.
    let make_pair = |score: ScoreWindow| {
        let sh = Shared::new_arc();
        sh.playing.store(true, Ordering::Release);
        let eng = Engine::new(score, Vec::new(), sr, Arc::clone(&sh));
        (sh, eng, CallbackWatchdog::new())
    };
    let (sh_st, mut eng_st, mut wd_st) = make_pair(score.clone());
    let (sh_mo, mut eng_mo, mut wd_mo) = make_pair(score.clone());
    let (sh_8, mut eng_8, mut wd_8) = make_pair(score.clone());
    let (sh_pcm, mut eng_i16, mut wd_i16) = make_pair(score);
    let frames = 137usize;
    let mut stereo = vec![0.0_f32; frames * 2];
    let mut mono = vec![0.0_f32; frames];
    // Pre-fill 8-channel buffer (+ 3 partial trailing samples) with non-zero garbage.
    let mut multi = vec![0.88_f32; frames * 8 + 3];
    let mut pcm16 = vec![0_i16; frames * 2];
    let fast = || Duration::from_micros(50);

    render_output_slice_with_elapsed(&mut eng_st, &mut wd_st, &sh_st, &mut stereo, 2, sr, fast);
    render_output_slice_with_elapsed(&mut eng_mo, &mut wd_mo, &sh_mo, &mut mono, 1, sr, fast);
    render_output_slice_with_elapsed(&mut eng_8, &mut wd_8, &sh_8, &mut multi, 8, sr, fast);
    render_output_slice_with_elapsed(&mut eng_i16, &mut wd_i16, &sh_pcm, &mut pcm16, 2, sr, fast);
    let pcm16_peak = pcm16.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    assert!(
        pcm16_peak > 500,
        "i16 render must carry audible signal: {pcm16_peak}"
    );

    let mut max_stereo_diff = 0.0_f32;
    for f in 0..frames {
        let l = stereo[f * 2];
        let r = stereo[f * 2 + 1];
        max_stereo_diff = max_stereo_diff.max((l - r).abs());
        assert!(
            (mono[f] - (l + r) * 0.5).abs() < 1e-6,
            "mono frame {f} mismatch: {} vs {}",
            mono[f],
            (l + r) * 0.5
        );
        assert_eq!(multi[f * 8], l);
        assert_eq!(multi[f * 8 + 1], r);
        for ch in 2..8 {
            assert_eq!(
                multi[f * 8 + ch],
                0.0,
                "channel {ch} on frame {f} must be zero-padded"
            );
        }
    }
    // Partial trailing remainder beyond `frames * 8` must also be zeroed!
    assert!(multi[frames * 8..].iter().all(|&s| s == 0.0));
    assert!(
        max_stereo_diff > 0.01,
        "panned synth should have distinct L/R channels for mono downmix check"
    );
}

#[test]
fn engine_and_mixer_multi_sample_rate_stability_pdc_and_zero_rt_allocs() {
    use cadence_core::mixer::{CompressorPatch, DelayPatch, EffectKind, LimiterPatch, ReverbPatch};

    // Build a full mix with Synth + Sampler + Group + Return + Delay + Reverb + Compressor + Limiter (PDC!).
    let mut p = Project::empty();
    p.bpm = 128.0;
    p.bars = 2;
    p.end = EndRule::Loop;
    p.mixer.master.strip.gain = 1.0;

    let mut ch_synth = Channel::new(1, ChannelKind::Audio, "Synth");
    ch_synth.output = ChannelRef::Channel(3);
    ch_synth.inserts.push(EffectDevice {
        id: 10,
        name: "Comp".into(),
        bypassed: false,
        kind: EffectKind::Compressor(CompressorPatch {
            threshold_db: -18.0,
            ratio: 4.0,
            ..CompressorPatch::default()
        }),
    });
    ch_synth.sends.push(ChannelSend {
        target: 4,
        amount: 0.35,
    });
    let mut ch_smp = Channel::new(2, ChannelKind::Audio, "Sampler");
    ch_smp.output = ChannelRef::Channel(3);
    ch_smp.inserts.push(EffectDevice {
        id: 11,
        name: "ChannelLim".into(),
        bypassed: false,
        kind: EffectKind::Limiter(LimiterPatch {
            ceiling_db: -1.0,
            input_drive_db: 2.0,
            ..LimiterPatch::default()
        }),
    });
    let mut ch_grp = Channel::new(3, ChannelKind::Group, "Bus");
    ch_grp.inserts.push(EffectDevice {
        id: 12,
        name: "Delay".into(),
        bypassed: false,
        kind: EffectKind::Delay(DelayPatch {
            mix: 0.35,
            feedback: 0.55,
            ..DelayPatch::default()
        }),
    });
    let mut ch_ret = Channel::new(4, ChannelKind::Return, "Verb");
    ch_ret.inserts.push(EffectDevice {
        id: 13,
        name: "Reverb".into(),
        bypassed: false,
        kind: EffectKind::Reverb(ReverbPatch {
            mix: 1.0,
            decay_seconds: 1.8,
            ..ReverbPatch::default()
        }),
    });
    p.mixer.channels = vec![ch_synth, ch_smp, ch_grp, ch_ret];
    p.mixer.master.inserts.push(EffectDevice {
        id: 14,
        name: "MasterLim".into(),
        bypassed: false,
        kind: EffectKind::Limiter(LimiterPatch {
            ceiling_db: -0.5,
            input_drive_db: 3.0,
            ..LimiterPatch::default()
        }),
    });

    p.patterns.push(Pattern {
        id: 1,
        name: "Notes".into(),
        kind: PatternKind::Notes {
            steps: vec![note(1), Step::default(), note(5), Step::default()],
        },
    });
    let mut smp_rack = RackPatch::default_sampler();
    {
        let smp = sampler_mut(&mut smp_rack);
        smp.path = Some("./test.wav".into());
        smp.one_shot = true;
    }
    p.instruments.push(InstrumentInstance {
        id: 1,
        name: "Lead".into(),
        rack: RackPatch::default_synth(),
        channel: 1,
    });
    p.instruments.push(InstrumentInstance {
        id: 2,
        name: "Hit".into(),
        rack: smp_rack,
        channel: 2,
    });
    p.parts.push(Part::new(1, "LeadPart", 1, 1));
    p.parts.push(Part::new(2, "HitPart", 1, 2));
    let mut l1 = Lane::new(1, "Lead");
    l1.placements.push(Placement::new(1, 1, 0, 32));
    let mut l2 = Lane::new(2, "Hit");
    l2.placements.push(Placement::new(2, 2, 0, 32));
    p.lanes = vec![l1, l2];

    let score = p.compile().unwrap();
    let key = module_sample_key(2, 1, 1);

    for &sr in &[44_100u32, 48_000, 88_200, 96_000, 192_000] {
        let sample = synthetic_sample(4_800, 48_000, 330.0);
        let decoy = synthetic_sample(256, 48_000, 110.0);
        let bank = vec![(999_999, decoy), (key, sample)];
        let shared = Shared::new_arc();
        shared.playing.store(true, Ordering::Release);
        let mut engine = Engine::new(score.clone(), bank, sr, Arc::clone(&shared));

        let mut peak = 0.0_f32;
        crate::mix_engine::tests::assert_no_heap_operations(|| {
            for _ in 0..4_096 {
                let frame = engine.next_frame();
                assert!(
                    frame[0].is_finite() && frame[1].is_finite(),
                    "non-finite frame at {sr} Hz: {frame:?}"
                );
                assert!(
                    frame[0].abs() < 4.0 && frame[1].abs() < 4.0,
                    "unstable amplitude at {sr} Hz: {frame:?}"
                );
                peak = peak.max(frame[0].abs()).max(frame[1].abs());
            }
        });
        assert!(peak > 0.05, "expected audible mix at {sr} Hz, got {peak}");
    }

    // Verify SamplerNode cached_bank_idx seamlessly recovers if the SampleBank order changes mid-voice.
    let mut smp_zone = RackPatch::default_sampler().zones.remove(0);
    if let ModuleKind::Sampler(ref mut smp) = smp_zone.modules[0].kind {
        smp.path = Some("./test.wav".into());
        smp.one_shot = true;
    }
    let smp_graph = cadence_core::rack::compile_zone_graph(&smp_zone).unwrap();
    let sample = synthetic_sample(4_800, 48_000, 440.0);
    let decoy = synthetic_sample(256, 48_000, 110.0);
    let smp_key = module_sample_key(2, 1, 1);
    let bank_a = vec![(111, Arc::clone(&decoy)), (smp_key, Arc::clone(&sample))];
    let bank_b = vec![(smp_key, Arc::clone(&sample)), (111, Arc::clone(&decoy))];
    let mut voice = crate::modular_voice::ModularVoice::new_boxed(
        &crate::modular_voice::ZoneTriggerSpec {
            inst_id: 2,
            zone: &smp_zone,
            graph: &smp_graph,
            sample_rate: 48_000.0,
            bpm: 120.0,
            zone_pan: 0.0,
            rack_tune: 0.0,
            is_preview: false,
        },
        crate::modular_voice::VoiceStart {
            midi: 60,
            velocity: 0.9,
            gate_frames: Some(4800),
            seed: 1,
            clock_seconds: 0.0,
            glide_from_pitch: None,
        },
    );
    let mut peak_before = 0.0_f32;
    for _ in 0..256 {
        let f = voice.next_frame(&bank_a, None);
        peak_before = peak_before.max(f[0].abs());
    }
    let mut peak_after = 0.0_f32;
    crate::mix_engine::tests::assert_no_heap_operations(|| {
        for _ in 0..256 {
            let f = voice.next_frame(&bank_b, None);
            assert!(f[0].is_finite() && f[1].is_finite());
            peak_after = peak_after.max(f[0].abs());
        }
    });
    assert!(peak_before > 0.05 && peak_after > 0.05);
}

#[test]
fn event_ring_and_scheduler_backpressure_and_live_note_off_release_all_fallback() {
    use crate::audio::scheduler::{StepScheduler, SUB_EVENT_QUEUE};
    use crate::audio::shared::MAILBOXES;
    use std::sync::atomic::AtomicU64;

    let score = rack_project(vec![note(1); 16], RackPatch::default_synth())
        .compile()
        .unwrap();
    let inst_id = score.instruments[0].id;
    let shared = Shared::new_arc();
    shared.publish_input_tracks(&score);

    // 1. Live event mailbox overflow increments `event_overflows` cleanly without blocking.
    let packed_audition = 1 | (60_u64 << 11) | (32_768_u64 << 18);
    for i in 0..MAILBOXES {
        assert!(
            shared.push_event_for_id(packed_audition, inst_id),
            "slot {i} must accept before mailbox fills"
        );
    }
    assert!(!shared.push_event_for_id(packed_audition, inst_id));
    assert_eq!(shared.event_overflows.load(Ordering::Relaxed), 1);

    // 2. When mailboxes are saturated, `live_note_off_instrument` triggers `release_all_requested`
    // fallback so a live Jam voice can NEVER stay permanently stuck.
    let rt = AudioRuntime::start(score.clone(), true).unwrap();
    assert!(rt.live_note_on_instrument(inst_id, 64, 0.85, true, None));
    wait_until(|| rt.snapshot().peak > 0.02);
    // Saturate live event mailboxes faster than one silent block (~5.3 ms) and send NoteOff.
    let mut overflowed = false;
    for _ in 0..(MAILBOXES * 3) {
        if !rt.audition_instrument(inst_id, 60, 0.5) {
            overflowed = true;
            break;
        }
    }
    assert!(rt.live_note_off_instrument(inst_id, 64));
    assert!(
        overflowed || rt.snapshot().event_overflows > 0,
        "expected mailbox saturation under burst audition pressure"
    );
    wait_until(|| rt.snapshot().peak < 0.02);

    // 3. StepScheduler sub-event overflow (> 256 sub-step events on one step) increments overflows cleanly.
    let mut dense = score;
    let overflow_extra = 12usize;
    let total_events = SUB_EVENT_QUEUE + overflow_extra;
    dense.events = (0..total_events)
        .map(|i| ScoreEvent {
            inst_idx: 0,
            part_id: Some(1),
            offset: 0.25 + (i as f32 * 0.001).min(0.6),
            degree: 1,
            accidental: 0,
            midi: 60,
            velocity: 0.8,
            gate: 0.25,
            chord: Default::default(),
            chord_len: 0,
            lock_start: 0,
            lock_len: 0,
        })
        .collect();
    dense.step_ranges[0] = (0, total_events as u32);
    let mut scheduler = StepScheduler::new();
    let overflows = AtomicU64::new(0);
    scheduler.schedule_step(&dense, 0, 48_000.0, &overflows, |_, _, _, _| {});
    assert_eq!(scheduler.pending_count(), SUB_EVENT_QUEUE);
    assert_eq!(overflows.load(Ordering::Relaxed), overflow_extra as u64);
}

#[test]
fn audio_tap_handle_overflow_tracks_dropped_frames_and_recovers() {
    let score = rack_project(vec![note(1); 16], RackPatch::default_synth())
        .compile()
        .unwrap();
    let shared = Shared::new_arc();
    shared.playing.store(true, Ordering::Release);
    let mut engine = Engine::new(score, Vec::new(), 48_000, Arc::clone(&shared));
    let tap = AudioTapHandle::from_shared(Arc::clone(&shared));
    tap.start();

    let extra_dropped = 512usize;
    for _ in 0..(AUDIO_TAP_CAPACITY + extra_dropped) {
        let _ = engine.next_frame();
    }

    let mut captured = Vec::new();
    let dropped = tap.drain(&mut captured);
    assert_eq!(dropped, extra_dropped as u64);
    assert_eq!(captured.len(), AUDIO_TAP_CAPACITY);
    assert!(captured
        .iter()
        .all(|f| f[0].is_finite() && f[1].is_finite()));

    // Subsequent frames drain cleanly with zero drops once consumer catches up.
    captured.clear();
    for _ in 0..64 {
        let _ = engine.next_frame();
    }
    let dropped_after = tap.drain(&mut captured);
    assert_eq!(dropped_after, 0);
    assert_eq!(captured.len(), 64);
    tap.stop();
}

#[test]
fn step_scheduler_idle_fast_path_and_exact_sub_step_firing_order() {
    use crate::audio::scheduler::StepScheduler;
    use std::sync::atomic::AtomicU64;

    let sr = 16_000.0; // 120 BPM -> 1 step = 0.125 s = 2000 frames exact.
    let mut p = rack_project(vec![Step::default(); 16], RackPatch::default_synth());
    p.instruments.push(InstrumentInstance {
        id: 2,
        name: "Bass".into(),
        rack: RackPatch::default_synth(),
        channel: 1,
    });
    let mut score = p.compile().unwrap();
    let make_ev = |inst_idx: u8, offset: f32, midi: u8| ScoreEvent {
        inst_idx,
        part_id: Some(1),
        offset,
        degree: 1,
        accidental: 0,
        midi,
        velocity: 0.85,
        gate: 0.25,
        chord: Default::default(),
        chord_len: 0,
        lock_start: 0,
        lock_len: 0,
    };
    // Intentionally out of chronological order + tie at offset=0.25 across two instruments.
    score.events = vec![
        make_ev(0, 0.75, 72), // frame 1500
        make_ev(1, 0.25, 48), // frame 500, inst 1
        make_ev(0, 0.50, 67), // frame 1000
        make_ev(0, 0.25, 60), // frame 500, inst 0 (must fire before inst 1!)
    ];
    score.step_ranges[0] = (0, 4);

    let mut scheduler = StepScheduler::new();
    assert_eq!(scheduler.pending_count(), 0);
    assert_eq!(scheduler.earliest_fire_frame(), u64::MAX);

    let overflows = AtomicU64::new(0);
    let mut immediate_count = 0usize;
    scheduler.schedule_step(&score, 0, sr, &overflows, |_, _, _, _| {
        immediate_count += 1;
    });
    assert_eq!(immediate_count, 0);
    assert_eq!(overflows.load(Ordering::Relaxed), 0);
    assert_eq!(scheduler.pending_count(), 4);
    assert_eq!(scheduler.earliest_fire_frame(), 500);

    let mut fired: Vec<(u64, u8, u8)> = Vec::new();
    for frame in 0..=1600u64 {
        scheduler.fire_due(|ev, _, _, _| {
            fired.push((frame, ev.inst_idx, ev.midi));
        });
        if frame < 500 {
            assert_eq!(scheduler.pending_count(), 4);
            assert_eq!(scheduler.earliest_fire_frame(), 500);
        } else if frame < 1000 {
            assert_eq!(scheduler.pending_count(), 2);
            assert_eq!(scheduler.earliest_fire_frame(), 1000);
        } else if frame < 1500 {
            assert_eq!(scheduler.pending_count(), 1);
            assert_eq!(scheduler.earliest_fire_frame(), 1500);
        } else {
            assert_eq!(scheduler.pending_count(), 0);
            assert_eq!(scheduler.earliest_fire_frame(), u64::MAX);
        }
        scheduler.advance_frame(score.step_seconds(), sr);
    }

    assert_eq!(
        fired,
        vec![(500, 0, 60), (500, 1, 48), (1000, 0, 67), (1500, 0, 72)]
    );

    // clear_pending / seek / reset restore exact O(1) idle invariants.
    scheduler.schedule_step(&score, 0, sr, &overflows, |_, _, _, _| {});
    assert_eq!(scheduler.pending_count(), 4);
    scheduler.seek(4);
    assert_eq!(scheduler.pending_count(), 0);
    assert_eq!(scheduler.earliest_fire_frame(), u64::MAX);
}

#[test]
fn buffer_size_clamping_device_matching_inventory_panic_silence_and_change_summaries() {
    use crate::audio::runtime::{match_device_name_index, resolve_target_buffer_frames};
    use cadence_core::runtime::{
        default_auto_buffer_frames, AudioStreamRequest, AudioStreamStatus,
    };

    // 1. Buffer size resolution and hardware range clamping (including high-SR auto scaling).
    let auto_48 = default_auto_buffer_frames(48_000);
    assert_eq!(
        resolve_target_buffer_frames(None, Some((64, 2048)), 48_000),
        Some(auto_48)
    );
    assert_eq!(
        resolve_target_buffer_frames(None, Some((64, 8192)), 192_000),
        Some(default_auto_buffer_frames(192_000))
    );
    assert_eq!(
        resolve_target_buffer_frames(None, Some((64, 256)), 48_000),
        Some(256)
    );
    assert_eq!(
        resolve_target_buffer_frames(Some(64), Some((128, 1024)), 48_000),
        Some(128)
    );
    assert_eq!(
        resolve_target_buffer_frames(Some(4096), Some((128, 1024)), 48_000),
        Some(1024)
    );
    // Inverted hardware range bounds are handled safely without panic.
    assert_eq!(
        resolve_target_buffer_frames(Some(512), Some((1024, 128)), 48_000),
        Some(512)
    );
    assert_eq!(resolve_target_buffer_frames(Some(256), None, 48_000), None);

    // 2. Device matching prefers exact match over case-insensitive substring.
    let devices = ["Built-in Output", "Scarlett 2i2 USB", "Scarlett 2i2"];
    assert_eq!(match_device_name_index(&devices, "Scarlett 2i2"), Some(2));
    assert_eq!(match_device_name_index(&devices, "scarlett"), Some(1));
    assert_eq!(match_device_name_index(&devices, "   "), None);
    assert_eq!(match_device_name_index(&devices, "NonExistent"), None);

    // 3. AudioDeviceChange summary formatting covers all 5 variants.
    assert!(AudioDeviceChange::DefaultOutputChanged {
        from: "Speakers".into(),
        to: "Headphones".into(),
    }
    .summary()
    .contains("Headphones"));
    assert!(AudioDeviceChange::SampleRateChanged {
        device: "DAC".into(),
        from: 44_100,
        to: 48_000,
    }
    .summary()
    .contains("48000"));
    assert!(AudioDeviceChange::FormatChanged {
        device: "DAC".into(),
        from_channels: 2,
        to_channels: 8,
    }
    .summary()
    .contains("8 ch"));
    assert!(AudioDeviceChange::DeviceLost {
        device: "USB DAC".into(),
    }
    .summary()
    .contains("USB DAC"));
    assert!(AudioDeviceChange::StreamFailed {
        message: "halted".into(),
    }
    .summary()
    .contains("halted"));

    // 4. Silent runtime start_with_config, inventory, active_request, detect_device_change,
    // panic_silence, and non-destructive reconfigure_device error handling.
    let score = rack_project(vec![note(1); 16], RackPatch::default_synth())
        .compile()
        .unwrap();
    let inst_id = score.instruments[0].id;
    let req = AudioStreamRequest {
        output_device: None,
        sample_rate: Some(48_000),
        buffer_frames: Some(512),
    };
    let mut rt =
        AudioRuntime::start_with_config(score.clone(), Vec::new(), true, req.clone()).unwrap();
    assert_eq!(rt.active_request(), &req);
    assert_eq!(rt.snapshot().health.status, AudioStreamStatus::Silent);
    assert_eq!(rt.detect_device_change(), None);
    assert_eq!(rt.last_error(), None);

    let inv = rt.enumerate_inventory();
    assert!(inv.silent_mode);
    assert_eq!(inv.active_sample_rate, 48_000);
    assert_eq!(inv.active_format_label, "Silent");

    // panic_silence immediately kills held live note and pauses transport.
    assert!(rt.live_note_on_instrument(inst_id, 60, 0.9, true, None));
    assert!(rt.transport(TransportIntent::Play { playing: true }));
    wait_until(|| {
        let snap = rt.snapshot();
        snap.playing && snap.peak > 0.02
    });
    rt.panic_silence();
    wait_until(|| {
        let snap = rt.snapshot();
        !snap.playing && snap.peak < 1e-4
    });

    // Requesting a non-existent explicit output device returns Err without killing the existing runtime.
    let bad_req = AudioStreamRequest {
        output_device: Some("__Cadence_NonExistent_Device_9999__".into()),
        sample_rate: None,
        buffer_frames: None,
    };
    assert!(rt.reconfigure_device(score, Vec::new(), bad_req).is_err());
    assert!(!rt.is_failed());
    assert!(rt.tap_handle().is_alive());
}

#[test]
fn input_routing_metering_clip_clear_and_capture_ring_overflow() {
    use crate::audio::input::{process_input_slice, InputMeterState};
    use crate::audio::shared::{AUDIO_CAPTURE_CAPACITY, INPUT_DIGITAL_SILENCE_SEC};
    use cadence_core::runtime::{AudioInputMode, AudioInputStatus, CapturePhase};

    let shared = Shared::new_arc();
    let mut state = InputMeterState::new(48_000);

    // 1. Stereo routing + non-finite sanitization + peak/hold/clip metering.
    shared
        .input_mode
        .store(AudioInputMode::Stereo.to_u8(), Ordering::Release);
    shared
        .capture_phase
        .store(CapturePhase::Recording as u8, Ordering::Release);
    let stereo_in: Vec<f32> = vec![0.25, -0.5, f32::NAN, 0.75, 1.0, -0.1, f32::INFINITY, 0.4];
    process_input_slice(&mut state, &shared, &stereo_in, 2, 48_000);

    let tel = shared.read_input_telemetry();
    assert_eq!(tel.status, AudioInputStatus::Running);
    assert_eq!(tel.sample_rate, 48_000);
    assert_eq!(tel.channels, 2);
    assert_eq!(tel.buffer_frames, 4);
    assert!(tel.clipped, "full-scale 1.0 sample must latch input clip");
    assert!(tel.peak_hold[0] >= 0.99);
    assert!(tel.peak_hold[1] >= 0.74);
    assert_eq!(tel.captured_frames, 4);

    // Verify captured stereo frames match sanitized input exactly.
    let mut drained = Vec::new();
    let read = shared.capture_read.load(Ordering::Relaxed);
    let write = shared.capture_write.load(Ordering::Acquire);
    assert_eq!(write.wrapping_sub(read), 4);
    for i in 0..4 {
        let slot = ((read + i) as usize) & crate::audio::shared::AUDIO_CAPTURE_MASK;
        drained.push([
            f32::from_bits(shared.capture_ring[slot][0].load(Ordering::Relaxed)),
            f32::from_bits(shared.capture_ring[slot][1].load(Ordering::Relaxed)),
        ]);
    }
    shared.capture_read.store(write, Ordering::Release);
    assert_eq!(
        drained,
        vec![[0.25, -0.5], [0.0, 0.75], [1.0, -0.1], [0.0, 0.4]]
    );

    // 2. MonoLeft and MonoRight duplicate the chosen channel onto both L and R.
    shared
        .input_mode
        .store(AudioInputMode::MonoLeft.to_u8(), Ordering::Release);
    let interleaved = vec![0.3_f32, -0.8_f32, 0.6, -0.2];
    process_input_slice(&mut state, &shared, &interleaved, 2, 48_000);
    let r = shared.capture_read.load(Ordering::Relaxed);
    let w = shared.capture_write.load(Ordering::Acquire);
    assert_eq!(w.wrapping_sub(r), 2);
    let slot0 = (r as usize) & crate::audio::shared::AUDIO_CAPTURE_MASK;
    assert_eq!(
        [
            f32::from_bits(shared.capture_ring[slot0][0].load(Ordering::Relaxed)),
            f32::from_bits(shared.capture_ring[slot0][1].load(Ordering::Relaxed)),
        ],
        [0.3, 0.3]
    );
    shared.capture_read.store(w, Ordering::Release);

    shared
        .input_mode
        .store(AudioInputMode::MonoRight.to_u8(), Ordering::Release);
    process_input_slice(&mut state, &shared, &interleaved, 2, 48_000);
    let r = shared.capture_read.load(Ordering::Relaxed);
    let w = shared.capture_write.load(Ordering::Acquire);
    let slot0 = (r as usize) & crate::audio::shared::AUDIO_CAPTURE_MASK;
    assert_eq!(
        [
            f32::from_bits(shared.capture_ring[slot0][0].load(Ordering::Relaxed)),
            f32::from_bits(shared.capture_ring[slot0][1].load(Ordering::Relaxed)),
        ],
        [-0.8, -0.8]
    );
    shared.capture_read.store(w, Ordering::Release);

    // 3. Clearing input meters resets clip latch and peak holds on next callback.
    shared.input_clear_meters.store(true, Ordering::Release);
    shared.input_clipped.store(false, Ordering::Relaxed);
    let quiet = vec![0.05_f32, 0.05_f32];
    process_input_slice(&mut state, &shared, &quiet, 2, 48_000);
    let cleared = shared.read_input_telemetry();
    assert!(!cleared.clipped);
    assert!(cleared.peak_hold[0] < 0.1);

    // 4. Overflowing the capture ring without draining increments dropped counter instead of panicking.
    let head = shared.capture_write.load(Ordering::Acquire);
    shared.capture_read.store(
        head.wrapping_sub(AUDIO_CAPTURE_CAPACITY as u64 - 2),
        Ordering::Release,
    );
    let burst = vec![0.1_f32; 16]; // 8 stereo frames -> 2 fit, 6 dropped
    process_input_slice(&mut state, &shared, &burst, 2, 48_000);
    assert_eq!(shared.capture_dropped.load(Ordering::Relaxed), 6);

    // 5. Sustained bit-for-bit 0.0 audio triggers `digital_silence` after 0.25 s across sample
    //    rates and callback sizes, and a single non-zero sample clears it immediately.
    for (sr, block_frames) in [
        (44_100u32, 256usize),
        (48_000, 64),
        (96_000, 256),
        (192_000, 512),
    ] {
        shared.reset_input_stream_state();
        state.clear();
        let threshold_frames = (f64::from(sr) * INPUT_DIGITAL_SILENCE_SEC).round() as usize;
        let zeros = vec![0.0_f32; block_frames * 2];
        let mut fed = 0usize;
        while fed + block_frames < threshold_frames {
            process_input_slice(&mut state, &shared, &zeros, 2, sr);
            fed += block_frames;
        }
        assert!(
            !shared.read_input_telemetry().digital_silence,
            "sr={sr} block={block_frames}: must not flag digital_silence before 0.25 s ({fed}/{threshold_frames} frames)"
        );
        // Feed the remaining frames to cross the 0.25 s boundary.
        let rem = threshold_frames.saturating_sub(fed);
        let rem_zeros = vec![0.0_f32; rem * 2];
        process_input_slice(&mut state, &shared, &rem_zeros, 2, sr);
        assert!(
            shared.read_input_telemetry().digital_silence,
            "sr={sr} block={block_frames}: must flag digital_silence once >= 0.25 s of zeros arrive"
        );
        shared
            .input_mode
            .store(AudioInputMode::Stereo.to_u8(), Ordering::Release);
        let noise_floor = vec![1e-5_f32, 0.0];
        process_input_slice(&mut state, &shared, &noise_floor, 2, sr);
        assert!(
            !shared.read_input_telemetry().digital_silence,
            "sr={sr}: single non-zero sample must clear digital_silence immediately"
        );
    }
}

#[test]
fn input_monitor_drain_modes_resampling_drift_guard_and_ramp_out() {
    use crate::audio::input::{process_input_slice, InputMeterState, InputMonitorDrain};
    use cadence_core::runtime::{CapturePhase, InputMonitorMode};

    let shared = Shared::new_arc();
    let mut in_state = InputMeterState::new(44_100);
    let mut drain = InputMonitorDrain::new();

    // 1. InputMonitorMode::Off keeps monitoring silent.
    shared
        .input_monitor_mode
        .store(InputMonitorMode::Off.to_u8(), Ordering::Release);
    let tone_44k: Vec<f32> = (0..1024)
        .flat_map(|i| {
            let s = (std::f64::consts::TAU * 440.0 * i as f64 / 44_100.0).sin() as f32 * 0.5;
            [s, -s]
        })
        .collect();
    process_input_slice(&mut in_state, &shared, &tone_44k, 2, 44_100);
    assert!(!shared.read_input_telemetry().monitoring);
    for _ in 0..128 {
        assert_eq!(drain.next_frame(&shared, 48_000.0), [0.0, 0.0]);
    }

    // 2. InputMonitorMode::Auto stays silent while Idle, activates when Armed/Recording.
    shared
        .input_monitor_mode
        .store(InputMonitorMode::Auto.to_u8(), Ordering::Release);
    shared
        .capture_phase
        .store(CapturePhase::Idle as u8, Ordering::Release);
    process_input_slice(&mut in_state, &shared, &tone_44k[..128], 2, 44_100);
    assert!(!shared.read_input_telemetry().monitoring);

    shared
        .capture_phase
        .store(CapturePhase::Armed as u8, Ordering::Release);
    process_input_slice(&mut in_state, &shared, &tone_44k, 2, 44_100);
    assert!(shared.read_input_telemetry().monitoring);

    // Cross-rate linear resampling (44.1 kHz input -> 48 kHz output) produces finite non-zero stereo audio.
    let mut peak = 0.0_f32;
    for _ in 0..512 {
        let [l, r] = drain.next_frame(&shared, 48_000.0);
        assert!(l.is_finite() && r.is_finite());
        peak = peak.max(l.abs()).max(r.abs());
    }
    assert!(
        peak > 0.35,
        "resampled live monitor must carry audible input signal: peak={peak}"
    );

    // 3. Clock-drift guard trims excess buffered monitor frames when depth exceeds high-water mark,
    //    preserving at least one input callback buffer (`target_depth`) to prevent immediate underrun.
    shared
        .input_monitor_mode
        .store(InputMonitorMode::On.to_u8(), Ordering::Release);
    for _ in 0..4 {
        process_input_slice(&mut in_state, &shared, &tone_44k, 2, 44_100);
    }
    let (high_water, target_depth) = crate::audio::input::monitor_watermarks(44_100.0, 1024);
    let queued_before = shared
        .monitor_write
        .load(Ordering::Acquire)
        .wrapping_sub(shared.monitor_read.load(Ordering::Relaxed));
    assert!(queued_before > high_water);
    let _ = drain.next_frame(&shared, 48_000.0);
    let queued_after = shared
        .monitor_write
        .load(Ordering::Acquire)
        .wrapping_sub(shared.monitor_read.load(Ordering::Relaxed));
    assert!(
        queued_after <= target_depth && queued_after + 4 >= target_depth,
        "drift guard must trim excess monitor latency back to target depth ({target_depth}): got {queued_after}"
    );

    // 4. Switching monitoring Off ramps smoothly down to zero (no instant click).
    shared
        .input_monitor_mode
        .store(InputMonitorMode::Off.to_u8(), Ordering::Release);
    shared.refresh_monitoring_active();
    let mut prev = drain.next_frame(&shared, 48_000.0);
    let mut max_step = 0.0_f32;
    for _ in 0..128 {
        let cur = drain.next_frame(&shared, 48_000.0);
        max_step = max_step
            .max((cur[0] - prev[0]).abs())
            .max((cur[1] - prev[1]).abs());
        prev = cur;
    }
    assert_eq!(
        prev,
        [0.0, 0.0],
        "monitor must reach exact silence after ramp-out"
    );
    assert!(
        max_step < 0.25,
        "monitor ramp-out should be smooth: max_step={max_step}"
    );
}

#[test]
fn input_monitor_drain_scales_watermarks_and_ramp_for_high_sr_and_large_buffers() {
    use crate::audio::input::{
        monitor_watermarks, process_input_slice, InputMeterState, InputMonitorDrain,
    };
    use cadence_core::runtime::InputMonitorMode;

    // At 192 kHz with 1024-frame callbacks (5.33 ms/callback), two stacked callbacks (2048 frames)
    // must NOT trip the drift guard, and draining 1024 frames after a drift trim must not underrun.
    for (sr, buf_frames) in [
        (44_100u32, 256usize),
        (48_000, 256),
        (96_000, 512),
        (192_000, 1024),
        (192_000, 2048),
    ] {
        let shared = Shared::new_arc();
        shared
            .input_monitor_mode
            .store(InputMonitorMode::On.to_u8(), Ordering::Release);
        let mut in_state = InputMeterState::new(sr);
        let mut drain = InputMonitorDrain::new();
        let (high_water, target_depth) = monitor_watermarks(f64::from(sr), buf_frames as u64);
        assert!(
            high_water >= (buf_frames as u64) * 3,
            "sr={sr} buf={buf_frames}: high_water={high_water} must exceed 3x callback size"
        );
        assert!(
            target_depth >= buf_frames as u64,
            "sr={sr} buf={buf_frames}: target_depth={target_depth} must cover at least one callback"
        );

        let chunk: Vec<f32> = (0..buf_frames)
            .flat_map(|i| {
                let s =
                    (std::f64::consts::TAU * 440.0 * i as f64 / f64::from(sr)).sin() as f32 * 0.5;
                [s, -s]
            })
            .collect();

        // Push 2 callbacks of jitter (2 * buf_frames <= high_water): must NOT trim or drop frames.
        process_input_slice(&mut in_state, &shared, &chunk, 2, sr);
        process_input_slice(&mut in_state, &shared, &chunk, 2, sr);
        let write_before = shared.monitor_write.load(Ordering::Acquire);
        let _ = drain.next_frame(&shared, f64::from(sr));
        let queued = write_before.wrapping_sub(shared.monitor_read.load(Ordering::Relaxed));
        assert!(
            queued >= (buf_frames * 2 - 4) as u64,
            "sr={sr} buf={buf_frames}: normal 2-buffer jitter falsely triggered drift trim (queued={queued})"
        );

        // Overflow past high_water -> drift guard trims cleanly to target_depth and sustains one full callback.
        while shared
            .monitor_write
            .load(Ordering::Acquire)
            .wrapping_sub(shared.monitor_read.load(Ordering::Relaxed))
            <= high_water
        {
            process_input_slice(&mut in_state, &shared, &chunk, 2, sr);
        }
        let mut min_abs_tail = 0.0_f32;
        for i in 0..buf_frames {
            let y = drain.next_frame(&shared, f64::from(sr));
            assert!(y[0].is_finite() && y[1].is_finite());
            if i + 32 >= buf_frames {
                min_abs_tail = min_abs_tail.max(y[0].abs()).max(y[1].abs());
            }
        }
        assert!(
            min_abs_tail > 0.05,
            "sr={sr} buf={buf_frames}: monitor underran within one callback after drift trim"
        );
    }

    // Asymmetric output buffer & 4:1 downsampling (e.g. 48k/256 mic -> 48k/2048 DAC, or
    // 192k/512 input -> 48k/512 output) must size high_water/target_depth for `out_buf * step`.
    for (in_sr, in_buf, out_sr, out_buf) in [
        (48_000u32, 256usize, 48_000.0_f64, 2048usize),
        (192_000u32, 512usize, 48_000.0_f64, 512usize),
    ] {
        let shared = Shared::new_arc();
        shared
            .buffer_frames
            .store(out_buf as u32, Ordering::Release);
        shared
            .input_monitor_mode
            .store(InputMonitorMode::On.to_u8(), Ordering::Release);
        let mut in_state = InputMeterState::new(in_sr);
        let mut drain = InputMonitorDrain::new();
        let step = (f64::from(in_sr) / out_sr).clamp(0.25, 4.0);
        let out_buf_in = (out_buf as f64 * step).ceil() as u64;
        let (high_water, target_depth) =
            monitor_watermarks(f64::from(in_sr), (in_buf as u64).max(out_buf_in));
        assert!(target_depth >= out_buf_in);

        let chunk: Vec<f32> = (0..in_buf)
            .flat_map(|i| {
                let s = (std::f64::consts::TAU * 440.0 * i as f64 / f64::from(in_sr)).sin() as f32
                    * 0.5;
                [s, -s]
            })
            .collect();
        while shared
            .monitor_write
            .load(Ordering::Acquire)
            .wrapping_sub(shared.monitor_read.load(Ordering::Relaxed))
            <= high_water
        {
            process_input_slice(&mut in_state, &shared, &chunk, 2, in_sr);
        }
        // Draining one full synchronous output callback after drift trim must not starve.
        let mut tail_peak = 0.0_f32;
        for i in 0..out_buf {
            let y = drain.next_frame(&shared, out_sr);
            assert!(y[0].is_finite() && y[1].is_finite());
            if i + 32 >= out_buf {
                tail_peak = tail_peak.max(y[0].abs()).max(y[1].abs());
            }
        }
        assert!(
            tail_peak > 0.05,
            "in={in_sr}/{in_buf} out={out_sr}/{out_buf}: starved mid-callback after drift trim"
        );
    }
}

#[test]
fn quantized_capture_gate_and_24bit_wav_latency_finalization() {
    use crate::audio::input::{process_input_slice, InputMeterState};
    use cadence_core::runtime::{AudioInputStatus, CaptureArmSpec, CapturePhase};

    // 120 BPM @ 16 kHz -> 16th step = 2000 frames exact.
    let sr = 16_000_u32;
    let frames_per_step = 2000_usize;
    let score = rack_project(vec![Step::default(); 16], RackPatch::default_synth())
        .compile()
        .unwrap();
    let shared = Shared::new_arc();
    shared.playing.store(true, Ordering::Release);
    let mut engine = Engine::new(score, Vec::new(), sr, Arc::clone(&shared));
    let mut in_state = InputMeterState::new(sr);

    // Configure quantized punch-in at step 4, punch-out at step 8, 64 tail frames.
    let spec = CaptureArmSpec {
        quantized: true,
        punch_in_step: 4,
        punch_out_step: Some(8),
        tail_frames: 64,
    };
    shared
        .capture_quantized
        .store(spec.quantized, Ordering::Relaxed);
    shared
        .capture_punch_in_step
        .store(spec.punch_in_step, Ordering::Release);
    shared
        .capture_has_punch_out
        .store(spec.punch_out_step.is_some(), Ordering::Release);
    shared
        .capture_punch_out_step
        .store(spec.punch_out_step.unwrap_or(0), Ordering::Release);
    shared
        .capture_tail_frames
        .store(spec.tail_frames, Ordering::Release);
    shared
        .capture_phase
        .store(CapturePhase::Armed as u8, Ordering::Release);

    // Prime input stream status as Running with pre-punch audio (must NOT be captured).
    let pre_audio = vec![0.9_f32; 256 * 2];
    process_input_slice(&mut in_state, &shared, &pre_audio, 2, sr);
    assert_eq!(
        shared.read_input_telemetry().status,
        AudioInputStatus::Running
    );
    assert_eq!(shared.read_input_telemetry().captured_frames, 0);

    // Advance steps 0..4 (must stay Armed and capture zero frames).
    for _ in 0..(4 * frames_per_step - 10) {
        let _ = engine.next_frame();
    }
    assert_eq!(
        shared.read_input_telemetry().capture_phase,
        CapturePhase::Armed
    );
    process_input_slice(&mut in_state, &shared, &pre_audio, 2, sr);
    assert_eq!(shared.read_input_telemetry().captured_frames, 0);

    // Cross into step 4 -> engine transitions Armed -> Recording!
    for _ in 0..20 {
        let _ = engine.next_frame();
    }
    assert_eq!(
        shared.read_input_telemetry().capture_phase,
        CapturePhase::Recording
    );

    // Feed 400 stereo frames during steps 4..8.
    let active_in: Vec<f32> = (0..400)
        .flat_map(|i| {
            let s = (std::f64::consts::TAU * 440.0 * i as f64 / f64::from(sr)).sin() as f32 * 0.6;
            [s, s * 0.5]
        })
        .collect();
    process_input_slice(&mut in_state, &shared, &active_in, 2, sr);
    assert_eq!(shared.read_input_telemetry().captured_frames, 400);

    // Advance across step 8 -> engine transitions Recording -> Finishing!
    for _ in 0..(4 * frames_per_step + 20) {
        let _ = engine.next_frame();
    }
    assert_eq!(
        shared.read_input_telemetry().capture_phase,
        CapturePhase::Finishing
    );

    // Feed 100 more stereo frames: only 64 tail frames are captured, then Finishing -> Idle!
    let tail_in = vec![0.25_f32; 100 * 2];
    process_input_slice(&mut in_state, &shared, &tail_in, 2, sr);
    let done_tel = shared.read_input_telemetry();
    assert_eq!(done_tel.capture_phase, CapturePhase::Idle);
    assert_eq!(done_tel.captured_frames, 464);

    // Test finalize_recording_wav: positive trim, negative pad, edge ramps, atomic 24-bit WAV + Symphonia decode.
    let dir = tempfile::tempdir().unwrap();
    let raw: Vec<[f32; 2]> = (0..4800)
        .map(|i| {
            let s = (std::f64::consts::TAU * 440.0 * i as f64 / 48_000.0).sin() as f32 * 0.5;
            [s, -s * 0.75]
        })
        .collect();

    // Positive latency trim (200 frames trimmed from head).
    let trimmed_wav = dir.path().join("take_trimmed.wav");
    let stats_trim = finalize_recording_wav(&raw, 48_000, 200, &trimmed_wav).unwrap();
    assert_eq!(stats_trim.frames, 4600);
    assert_eq!(stats_trim.sample_rate, 48_000);
    assert!(
        stats_trim.peak_left_db > -7.0 && stats_trim.peak_left_db < -5.5,
        "expected ~-6.0 dBFS peak on 0.5 sine: got {} dB",
        stats_trim.peak_left_db
    );
    assert_eq!(stats_trim.full_scale_samples, 0);

    let reader = hound::WavReader::open(&trimmed_wav).unwrap();
    assert_eq!(reader.spec().channels, 2);
    assert_eq!(reader.spec().sample_rate, 48_000);
    assert_eq!(reader.spec().bits_per_sample, 24);
    assert_eq!(reader.spec().sample_format, hound::SampleFormat::Int);

    // Verify cadence-core's Symphonia loader decodes the 24-bit WAV cleanly with fade-in/out edge ramps!
    let loaded =
        cadence_core::sampler::load_sample_file(&trimmed_wav, Some("./take_trimmed.wav".into()))
            .unwrap();
    assert_eq!(loaded.frames, 4600);
    assert_eq!(loaded.channels, 2);
    assert_eq!(loaded.sample_rate, 48_000);
    assert!(
        loaded.left[0].abs() < 1e-4,
        "first frame must be ramped from 0 to avoid clicks: {}",
        loaded.left[0]
    );
    assert!(
        loaded.left[loaded.frames - 1].abs() < 1e-4,
        "final frame must be ramped to 0 to avoid clicks: {}",
        loaded.left[loaded.frames - 1]
    );

    // Negative latency trim (-128 frames prepends 128 silent frames).
    let padded_wav = dir.path().join("take_padded.wav");
    let stats_pad = finalize_recording_wav(&raw, 48_000, -128, &padded_wav).unwrap();
    assert_eq!(stats_pad.frames, 4928);
    let loaded_pad =
        cadence_core::sampler::load_sample_file(&padded_wav, Some("./take_padded.wav".into()))
            .unwrap();
    assert_eq!(loaded_pad.frames, 4928);
    assert!(loaded_pad.left[..128].iter().all(|&s| s.abs() < 1e-5));
}

#[test]
fn rack_bus_wide_open_cutoff_bypasses_svf_at_all_sample_rates() {
    use cadence_core::rack::{
        InPort, ModuleKind, OutModule, PatchCable, PitchTracking, RackModule, RackPatch, RackZone,
        VcoModule, VoiceSettings, Waveform,
    };

    // High-frequency sine routed directly VCO -> Out with RackPatch.cutoff_hz = 20_000 Hz, res = 0.0.
    // Must bypass the Rack-bus SVF cleanly even when sr * 0.45 < 19_900 Hz (24 kHz and 44.1 kHz).
    for (sr, sine_hz) in [
        (24_000u32, 8_500.0_f32),
        (44_100, 16_000.0),
        (48_000, 16_000.0),
        (96_000, 16_000.0),
    ] {
        let make_rack = |rack_cutoff: f32| RackPatch {
            zones: vec![RackZone {
                id: 1,
                name: "Sine".into(),
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
                        "Osc",
                        ModuleKind::Vco(VcoModule {
                            waveform: Waveform::Sine,
                            pitch_tracking: PitchTracking::Fixed,
                            fixed_hz: sine_hz,
                            level: 0.5,
                            ..VcoModule::default()
                        }),
                    ),
                    RackModule::new(2, "Out", ModuleKind::Out(OutModule::default())),
                ],
                cables: vec![PatchCable::new(1, 1, 2, InPort::In, 1.0)],
            }],
            gain_db: 0.0,
            pan: 0.0,
            tune_semitones: 0.0,
            cutoff_hz: rack_cutoff,
            resonance: 0.0,
            kit_name: String::new(),
            panel_pins: Vec::new(),
        };

        let render_rms = |rack_cutoff: f32| -> (f64, bool) {
            let held = Step {
                active: true,
                degree: 1,
                velocity: 1.0,
                gate: 1.0,
                ..Step::default()
            };
            let mut p = rack_project(vec![held], make_rack(rack_cutoff));
            p.bars = 1;
            p.lanes[0].placements[0].length_steps = 16;
            let score = p.compile().unwrap();
            let shared = Shared::new_arc();
            shared.playing.store(true, Ordering::Release);
            let mut engine = Engine::new(score, Vec::new(), sr, Arc::clone(&shared));
            let warmup = (sr as usize) / 50; // 20 ms (well inside 125 ms step gate)
            let measure = (sr as usize) / 20; // 50 ms
            let mut sum_sq = 0.0_f64;
            let mut svf_active = false;
            for i in 0..(warmup + measure) {
                let frame = engine.next_frame();
                if i == warmup {
                    svf_active = engine
                        .voices()
                        .iter()
                        .any(|v| v.active && v.rack_svf_was_active);
                }
                if i >= warmup {
                    sum_sq += f64::from(frame[0]) * f64::from(frame[0]);
                }
            }
            ((sum_sq / measure as f64).sqrt(), svf_active)
        };

        let (rms_open, svf_open) = render_rms(20_000.0);
        let (rms_closed, svf_closed) = render_rms(2_000.0);
        assert!(
            !svf_open,
            "sr={sr}: wide-open rack cutoff (20 kHz, res 0) must bypass Rack-bus SVF"
        );
        assert!(
            svf_closed,
            "sr={sr}: closed rack cutoff (2 kHz) must engage Rack-bus SVF"
        );
        // Expected RMS of 0.5-amplitude mono sine centered by equal-power pan (1/sqrt(2)): 0.5 * 0.5 = 0.25.
        let diff_db = (20.0 * (rms_open / 0.25).log10()).abs();
        assert!(
            diff_db < 0.2,
            "sr={sr}: wide-open sine must stay at unity (got rms={rms_open:.4}, diff={diff_db:.2} dB)"
        );
        assert!(
            rms_closed < rms_open * 0.2,
            "sr={sr}: 2 kHz rack LPF must attenuate {sine_hz} Hz sine strongly (open={rms_open:.4}, closed={rms_closed:.4})"
        );
    }
}

#[test]
fn mono_portamento_glides_across_detached_rests_after_voice_retirement_and_clears_on_reset() {
    let sr = 16_000_u32;
    let mut rack = RackPatch::default_synth();
    rack.zones[0].voice.voice_mode = VoiceMode::Mono;
    rack.zones[0].voice.legato = true;
    rack.zones[0].voice.glide_ms = 60.0;
    let env = amp_env_mut(&mut rack);
    env.attack_ms = 1.0;
    env.decay_ms = 10.0;
    env.sustain = 0.8;
    env.release_ms = 15.0;

    let mut n1 = note(1);
    n1.gate = 0.25;
    let mut n2 = note(8); // +1 octave above degree 1
    n2.gate = 0.75;
    let mut steps = vec![Step::default(); 16];
    steps[0] = n1;
    steps[8] = n2;

    let score = rack_project(steps, rack).compile().unwrap();
    let shared = Shared::new_arc();
    shared.playing.store(true, Ordering::Release);
    let mut engine = Engine::new(score, Vec::new(), sr, Arc::clone(&shared));

    // Frame 0 triggers step 0 (very first note after init): must start cleanly at target pitch.
    let _ = engine.next_frame();
    let first_voice = engine
        .voices()
        .iter()
        .find(|v| v.active)
        .expect("step 0 note must be active");
    let pitch1 = first_voice.modular.target_note_pitch();
    assert_eq!(
        first_voice.modular.current_note_pitch(),
        pitch1,
        "first mono note after reset must not phantom-glide"
    );

    // Advance 6 steps (750 ms @ 120 BPM): short gate + 15 ms release must retire completely.
    for _ in 1..12_000 {
        let _ = engine.next_frame();
    }
    assert!(
        engine.voices().iter().all(|v| !v.active),
        "mono voice must retire cleanly before the detached second note"
    );

    // Advance across step 8 (frame 16_000) under zero-alloc guard: detached note 2 must glide from pitch1.
    crate::mix_engine::tests::assert_no_heap_operations(|| {
        for _ in 12_000..16_016 {
            let _ = engine.next_frame();
        }
    });
    let second_voice = engine
        .voices()
        .iter()
        .find(|v| v.active)
        .expect("step 8 detached note must be active");
    let pitch2 = second_voice.modular.target_note_pitch();
    let curr2 = second_voice.modular.current_note_pitch();
    assert!(
        (pitch2 - (pitch1 + 12.0)).abs() < 1e-6,
        "step 8 should target one octave above step 0 (p1={pitch1}, p2={pitch2})"
    );
    assert!(
        curr2 > pitch1 && curr2 < pitch2 - 6.0,
        "detached mono note after voice retirement must glide smoothly from prior pitch (p1={pitch1}, curr={curr2}, p2={pitch2})"
    );

    // After silencing and seeking transport back to step 0 (which clears voice pool),
    // note 1 starts cleanly at pitch1 again with no stale glide memory.
    engine.silence_in_place();
    engine.seek_to_step(0);
    let _ = engine.next_frame();
    let reset_voice = engine
        .voices()
        .iter()
        .find(|v| v.active)
        .expect("step 0 after seek must be active");
    assert_eq!(
        reset_voice.modular.current_note_pitch(),
        pitch1,
        "transport seek/clear_all must clear mono portamento pitch memory"
    );
}

#[test]
fn watchdog_ema_time_based_overload_sr_scaled_fade_and_engine_pdc_telemetry() {
    use crate::audio::runtime::{
        overload_fade_frames, render_output_slice_with_elapsed, CallbackWatchdog,
        OVERLOAD_FADE_BUF_CAPACITY,
    };
    use cadence_core::dsp::limiter_lookahead_frames;
    use cadence_core::mixer::{EffectKind, LimiterPatch};
    use cadence_core::runtime::AudioInputTelemetry;

    // 1. Overload fade frame count scales proportionally with sample rate and stays within buffer capacity.
    assert_eq!(overload_fade_frames(48_000), 128);
    assert_eq!(overload_fade_frames(96_000), 256);
    assert!((overload_fade_frames(192_000) as isize - 512).abs() <= 2);
    assert_eq!(overload_fade_frames(384_000), OVERLOAD_FADE_BUF_CAPACITY);

    // 2. CallbackWatchdog EMA step response stays sample-rate- and buffer-size-invariant
    //    between 48 kHz / 512 frames and 192 kHz / 64 frames (120 ms tau -> ~63.2% at 120 ms).
    let measure_step_ema = |sr: u32, frames: usize| -> f32 {
        let score = rack_project(vec![Step::default(); 16], RackPatch::default_synth())
            .compile()
            .unwrap();
        let shared = Shared::new_arc();
        let mut engine = Engine::new(score, Vec::new(), sr, Arc::clone(&shared));
        let mut watchdog = CallbackWatchdog::new();
        let mut buf = vec![0.0_f32; frames * 2];
        let budget = Duration::from_secs_f64(frames as f64 / f64::from(sr));

        // Seed EMA at 0.0 on first callback.
        render_output_slice_with_elapsed(
            &mut engine,
            &mut watchdog,
            &shared,
            &mut buf,
            2,
            sr,
            || Duration::ZERO,
        );
        // Run 120 ms of callbacks at 80% budget load -> EMA should reach 0.8 * (1 - 1/e) ~= 0.5057.
        let blocks = (0.120 / budget.as_secs_f64()).round() as usize;
        for _ in 0..blocks {
            render_output_slice_with_elapsed(
                &mut engine,
                &mut watchdog,
                &shared,
                &mut buf,
                2,
                sr,
                || budget.mul_f64(0.8),
            );
        }
        shared.read_health().cpu_load
    };

    let ema_48k = measure_step_ema(48_000, 512);
    let ema_192k_64 = measure_step_ema(192_000, 64);
    let expected_ema = 0.8 * (1.0 - (-1.0_f32).exp());
    assert!(
        (ema_48k - expected_ema).abs() < 0.03,
        "48k/512 EMA={ema_48k}, expected ~{expected_ema}"
    );
    assert!(
        (ema_192k_64 - expected_ema).abs() < 0.03,
        "192k/64 EMA={ema_192k_64}, expected ~{expected_ema}"
    );
    assert!(
        (ema_48k - ema_192k_64).abs() < 0.03,
        "EMA drifted across sample rate / buffer size: 48k={ema_48k} vs 192k/64={ema_192k_64}"
    );

    // 3. Time-based sustained overload at 192 kHz / 64 frames (0.333 ms/callback):
    //    10 consecutive overruns (3.33 ms) must NOT false-trip, while >= 50 ms sustained MUST trip.
    //    Also verify 192 kHz overload fade-out ramps smoothly over ~512 frames with zero heap ops.
    let mut rack = RackPatch::default_synth();
    amp_env_mut(&mut rack).sustain = 1.0;
    let score_192 = rack_project(vec![note(1); 16], rack).compile().unwrap();
    let shared_192 = Shared::new_arc();
    shared_192.playing.store(true, Ordering::Release);
    let mut eng_192 = Engine::new(
        score_192.clone(),
        Vec::new(),
        192_000,
        Arc::clone(&shared_192),
    );
    let mut wd_192 = CallbackWatchdog::new();
    let mut buf_64 = vec![0.0_f32; 64 * 2];
    let budget_64 = Duration::from_secs_f64(64.0 / 192_000.0);
    for _ in 0..10 {
        render_output_slice_with_elapsed(
            &mut eng_192,
            &mut wd_192,
            &shared_192,
            &mut buf_64,
            2,
            192_000,
            || budget_64.mul_f64(1.3),
        );
    }
    assert!(
        !shared_192.read_health().overloaded,
        "10 brief overruns (3.3 ms) at 192 kHz / 64 fr must not false-trip"
    );
    for _ in 0..145 {
        render_output_slice_with_elapsed(
            &mut eng_192,
            &mut wd_192,
            &shared_192,
            &mut buf_64,
            2,
            192_000,
            || budget_64.mul_f64(1.3),
        );
    }
    assert!(
        shared_192.read_health().overloaded,
        "sustained >= 50 ms of overruns at 192 kHz / 64 fr must trip overload circuit breaker"
    );

    // Verify 192 kHz large-block overload fade uses SR-scaled ~512-frame Hann ramp with zero heap ops.
    let shared_fade = Shared::new_arc();
    shared_fade.playing.store(true, Ordering::Release);
    let mut eng_fade = Engine::new(score_192, Vec::new(), 192_000, Arc::clone(&shared_fade));
    let mut wd_fade = CallbackWatchdog::new();
    let fade_frames = overload_fade_frames(192_000);
    let block_192 = 1024usize;
    let mut buf_192 = vec![0.0_f32; block_192 * 2];
    let budget_192 = Duration::from_secs_f64(block_192 as f64 / 192_000.0);
    for _ in 0..4 {
        render_output_slice_with_elapsed(
            &mut eng_fade,
            &mut wd_fade,
            &shared_fade,
            &mut buf_192,
            2,
            192_000,
            || budget_192.mul_f64(0.2),
        );
    }
    let mut max_step_192 = 0.0_f32;
    crate::mix_engine::tests::assert_no_heap_operations(|| {
        render_output_slice_with_elapsed(
            &mut eng_fade,
            &mut wd_fade,
            &shared_fade,
            &mut buf_192,
            2,
            192_000,
            || budget_192.mul_f64(4.5),
        );
        let fade_start = (block_192 - fade_frames) * 2;
        for f in (fade_start..buf_192.len() - 2).step_by(2) {
            max_step_192 = max_step_192
                .max((buf_192[f + 2] - buf_192[f]).abs())
                .max((buf_192[f + 3] - buf_192[f + 1]).abs());
        }
        assert_eq!(buf_192[buf_192.len() - 2], 0.0);
        assert_eq!(buf_192[buf_192.len() - 1], 0.0);
    });
    assert!(shared_fade.read_health().overloaded);
    assert!(
        max_step_192 < 0.15,
        "192 kHz overload fade-out must be smooth over {fade_frames} frames: max_step={max_step_192}"
    );

    // 4. Engine publishes MixerEngine::total_output_latency_frames() to AudioHealth::pdc_latency_frames
    //    on Engine::new and on every live adopt(), feeding round_trip_latency_ms across sample rates.
    for sr in [44_100u32, 48_000, 96_000, 192_000] {
        let lookahead = limiter_lookahead_frames(f64::from(sr)) as u32;
        let mut p = rack_project(vec![Step::default(); 16], RackPatch::default_synth());
        let dry_score = p.compile().unwrap();
        let sh = Shared::new_arc();
        sh.buffer_frames.store(512, Ordering::Release);
        let mut eng = Engine::new(dry_score, Vec::new(), sr, Arc::clone(&sh));
        assert_eq!(sh.read_health().pdc_latency_frames, 0, "sr={sr}");

        // Live-adopt a Master Limiter -> pdc_latency_frames updates to `lookahead`.
        p.mixer.master.inserts.push(EffectDevice {
            id: 901,
            name: "Master Limiter".into(),
            bypassed: false,
            kind: EffectKind::Limiter(LimiterPatch::default()),
        });
        eng.publish_score_update(p.compile().unwrap());
        eng.poll_controls();
        let health = sh.read_health();
        assert_eq!(
            health.pdc_latency_frames, lookahead,
            "sr={sr}: Engine adopt must publish Master Limiter PDC latency"
        );

        let inp = AudioInputTelemetry {
            sample_rate: sr,
            buffer_frames: 256,
            ..AudioInputTelemetry::default()
        };
        let rt_ms = inp.round_trip_latency_ms(health, sr, 0.0).unwrap();
        let expected_ms = (256 + 512 + lookahead) as f32 * 1000.0 / sr as f32;
        assert!(
            (rt_ms - expected_ms).abs() < 1e-3,
            "sr={sr}: round_trip_latency_ms={rt_ms} expected={expected_ms}"
        );
    }

    // 5. Verify sample-rate-invariant meter publication cadence & Rack-bus SVF control stride masks.
    for (sr, expected_meter_stride, expected_svf_mask) in [
        (44_100u32, 256usize, 7u32),
        (48_000, 256, 7),
        (96_000, 512, 15),
        (192_000, 1024, 31),
    ] {
        let score = rack_project(vec![Step::default(); 16], RackPatch::default_synth())
            .compile()
            .unwrap();
        let sh = Shared::new_arc();
        let mut eng = Engine::new(score, Vec::new(), sr, Arc::clone(&sh));
        assert_eq!(
            eng.voices()[0].rack_svf_mask,
            expected_svf_mask,
            "sr={sr}: rack_svf_mask mismatch"
        );
        let gen_start = sh.mixer_meter_generation.load(Ordering::Acquire);
        for _ in 0..(expected_meter_stride - 1) {
            let _ = eng.next_frame();
        }
        assert_eq!(
            sh.mixer_meter_generation.load(Ordering::Acquire),
            gen_start,
            "sr={sr}: meter must not publish before stride {expected_meter_stride}"
        );
        let _ = eng.next_frame();
        assert_eq!(
            sh.mixer_meter_generation.load(Ordering::Acquire),
            gen_start + 2,
            "sr={sr}: meter must publish on exact stride {expected_meter_stride}"
        );
    }
}

#[test]
fn mono_dac_downmix_sums_before_clamping_and_master_meters_live_monitor() {
    use crate::audio::input::{process_input_slice, InputMeterState};
    use crate::audio::runtime::{
        overload_fade_frames, render_output_slice_with_elapsed, CallbackWatchdog,
    };
    use crate::audio::shared::SharedChannelMeter;
    use cadence_core::runtime::{AudioInputMode, InputMonitorMode};

    let sr = 48_000u32;
    let score = rack_project(vec![Step::default(); 16], RackPatch::default_synth())
        .compile()
        .unwrap();

    let make_monitored_engine = || {
        let shared = Shared::new_arc();
        shared
            .input_mode
            .store(AudioInputMode::Stereo.to_u8(), Ordering::Release);
        shared
            .input_monitor_mode
            .store(InputMonitorMode::On.to_u8(), Ordering::Release);
        let engine = Engine::new(score.clone(), Vec::new(), sr, Arc::clone(&shared));
        let in_state = InputMeterState::new(sr);
        let watchdog = CallbackWatchdog::new();
        (shared, engine, in_state, watchdog)
    };

    let feed_constant_stereo =
        |in_state: &mut InputMeterState, shared: &Shared, frame: [f32; 2], count: usize| {
            let mut interleaved = Vec::with_capacity(count * 2);
            for _ in 0..count {
                interleaved.push(frame[0]);
                interleaved.push(frame[1]);
            }
            process_input_slice(in_state, shared, &interleaved, 2, sr);
        };

    // 1. [1.2, -0.4]: mono DAC (1 ch) sums BEFORE clamping -> exact 0.4 (NOT pre-clamped 0.3),
    //    while stereo DAC (2 ch) clamps each channel independently -> [1.0, -0.4].
    //    Also confirm live monitor exceeding 1.0 latches Master meter `clipped` end-to-end.
    let (sh_mo, mut eng_mo, mut in_mo, mut wd_mo) = make_monitored_engine();
    let (sh_st, mut eng_st, mut in_st, mut wd_st) = make_monitored_engine();
    feed_constant_stereo(&mut in_mo, &sh_mo, [1.2, -0.4], 320);
    feed_constant_stereo(&mut in_st, &sh_st, [1.2, -0.4], 320);

    let frames = 256usize;
    let mut mono = vec![0.0_f32; frames];
    let mut stereo = vec![0.0_f32; frames * 2];
    let fast = || Duration::from_micros(50);

    render_output_slice_with_elapsed(&mut eng_mo, &mut wd_mo, &sh_mo, &mut mono, 1, sr, fast);
    render_output_slice_with_elapsed(&mut eng_st, &mut wd_st, &sh_st, &mut stereo, 2, sr, fast);

    // Past the 1.5 ms (~72-frame) monitor ramp-in, frames 100..256 carry full-gain [1.2, -0.4].
    for f in 100..frames {
        assert!(
            (mono[f] - 0.4).abs() < 1e-5,
            "mono frame {f} must downmix [1.2, -0.4] before clamping to 0.4, got {}",
            mono[f]
        );
        assert!(
            (stereo[f * 2] - 1.0).abs() < 1e-6 && (stereo[f * 2 + 1] - (-0.4)).abs() < 1e-5,
            "stereo frame {f} must clamp per channel to [1.0, -0.4], got [{}, {}]",
            stereo[f * 2],
            stereo[f * 2 + 1]
        );
    }

    let master_meter = sh_mo
        .mixer_meters
        .iter()
        .filter_map(SharedChannelMeter::read)
        .find(|m| m.channel == ChannelRef::Master)
        .expect("master meter published");
    assert!(
        master_meter.clipped && master_meter.output_peak_hold[0] >= 1.19,
        "Master meter must observe live monitor pushing final output > 1.0: {master_meter:?}"
    );

    // 2. Overload tail fade on mono DAC also stores unclipped stereo in watchdog.tail_stereo,
    //    ramping smoothly from 0.4 down to exact 0.0 without stepping down to 0.3 first.
    feed_constant_stereo(&mut in_mo, &sh_mo, [1.2, -0.4], 320);
    let mut fade_mono = vec![0.0_f32; frames];
    let budget = Duration::from_secs_f64(frames as f64 / f64::from(sr));
    render_output_slice_with_elapsed(
        &mut eng_mo,
        &mut wd_mo,
        &sh_mo,
        &mut fade_mono,
        1,
        sr,
        || budget.mul_f64(5.0),
    );
    assert!(sh_mo.read_health().overloaded);
    let tail_start = frames - overload_fade_frames(sr);
    assert!(
        (fade_mono[tail_start - 1] - 0.4).abs() < 1e-5,
        "pre-fade mono sample must be 0.4, got {}",
        fade_mono[tail_start - 1]
    );
    assert!(
        fade_mono[tail_start] > 0.38,
        "first mono overload-fade sample must start near 0.4 (unclipped tail), got {}",
        fade_mono[tail_start]
    );
    assert_eq!(fade_mono[frames - 1], 0.0);

    // 3. Mono sums exceeding +/-1.0 still clamp cleanly to [-1.0, 1.0].
    for (pair, expected) in [
        ([1.5_f32, 1.5_f32], 1.0_f32),
        ([-1.5_f32, -1.5_f32], -1.0_f32),
    ] {
        let (sh, mut eng, mut in_st, mut wd) = make_monitored_engine();
        feed_constant_stereo(&mut in_st, &sh, pair, 320);
        let mut out = vec![0.0_f32; frames];
        render_output_slice_with_elapsed(&mut eng, &mut wd, &sh, &mut out, 1, sr, fast);
        for (f, &sample) in out.iter().enumerate().take(frames).skip(100) {
            assert_eq!(
                sample, expected,
                "mono sum of {pair:?} at frame {f} must clamp to {expected}, got {sample}"
            );
        }
    }
}

#[test]
fn voice_steal_micro_fade_contributes_to_track_scope() {
    use crate::audio::shared::VOICES;
    use crate::audio::voice_pool::{TriggerRequest, VoicePool, ZoneTriggerCtx};
    use cadence_core::dsp::stereo_mid_f32;

    let sr = 48_000u32;
    let sr_f = f64::from(sr);

    // Track 0: Stereo sampler playing a constant asymmetric [0.8, 0.2] waveform with instant attack.
    let mut track0_rack = RackPatch::default_sampler();
    {
        let smp = sampler_mut(&mut track0_rack);
        smp.path = Some("./dc_stereo.wav".into());
        smp.sample_start = 0;
        smp.sample_end = 512;
        smp.loop_mode = SampleLoopMode::Forward;
        smp.loop_start = 0;
        smp.loop_end = 512;
        smp.one_shot = false;
    }
    {
        let flt = filter_mut(&mut track0_rack);
        flt.cutoff_hz = 20_000.0;
        flt.resonance = 0.0;
    }
    {
        let env = amp_env_mut(&mut track0_rack);
        env.attack_ms = 0.1;
        env.sustain = 1.0;
        env.release_ms = 200.0;
    }

    // Track 1: Synth with 100 ms envelope delay so newly stolen slots output exact silence initially.
    let mut track1_rack = RackPatch::default_synth();
    {
        let env = amp_env_mut(&mut track1_rack);
        env.delay_ms = 100.0;
        env.attack_ms = 50.0;
        env.sustain = 1.0;
    }

    let mut project = Project::empty();
    project.bars = 1;
    project
        .mixer
        .channels
        .push(Channel::new(1, ChannelKind::Audio, "Sampler"));
    project
        .mixer
        .channels
        .push(Channel::new(2, ChannelKind::Audio, "Delayed"));
    project.instruments.push(InstrumentInstance {
        id: 1,
        name: "Sampler".into(),
        rack: track0_rack,
        channel: 1,
    });
    project.instruments.push(InstrumentInstance {
        id: 2,
        name: "Delayed".into(),
        rack: track1_rack,
        channel: 2,
    });
    let score = project.compile().unwrap();

    let stereo_dc = Arc::new(SampleData {
        path_label: "./dc_stereo.wav".into(),
        resolved_path: std::path::PathBuf::from("dc_stereo.wav"),
        name: "dc_stereo".into(),
        sample_rate: sr,
        channels: 2,
        frames: 512,
        left: vec![0.8_f32; 512].into_boxed_slice(),
        right: Some(vec![0.2_f32; 512].into_boxed_slice()),
        peaks: vec![(0.2, 0.8); 64].into_boxed_slice(),
        meta: SampleMetadata::default(),
    });
    let samples = vec![(module_sample_key(1, 1, 1), stereo_dc)];

    let mut pool = VoicePool::new(sr_f);
    let ctx = ZoneTriggerCtx {
        project: &score,
        samples: &samples,
        sample_rate: sr_f,
        rendered_frames: 0,
    };

    // Trigger one sustained voice on track 0 and advance 64 frames so it is oldest with loud last_output.
    pool.trigger_on_track(
        &ctx,
        &TriggerRequest {
            inst_index: 0,
            live_key: None,
            midi: 60,
            velocity: 1.0,
            gate_seconds: 10.0,
            seed: 1,
            mod_pos: None,
            locks: &[],
        },
    );
    let mut pre_steal = pool.render_frame(&samples, None);
    for _ in 1..64 {
        pre_steal = pool.render_frame(&samples, None);
    }
    assert!(
        pre_steal.sources[0][0] > 0.2 && pre_steal.sources[0][0] > pre_steal.sources[0][1] * 2.0,
        "expected asymmetric stereo output on track 0 before steal: {:?}",
        pre_steal.sources[0]
    );
    assert!(
        (pre_steal.track_scope[0] - stereo_mid_f32(pre_steal.sources[0])).abs() < 1e-6,
        "active voice track_scope must match stereo_mid_f32"
    );

    // Fill the remaining VOICES - 1 slots with younger voices on track 1, then trigger one more
    // voice on track 1 to steal track 0's oldest voice and arm a track-0 steal micro-fade.
    for i in 0..VOICES {
        pool.trigger_on_track(
            &ctx,
            &TriggerRequest {
                inst_index: 1,
                live_key: None,
                midi: 60,
                velocity: 1.0,
                gate_seconds: 10.0,
                seed: (i as u32) + 2,
                mod_pos: None,
                locks: &[],
            },
        );
    }
    assert!(
        pool.voices().iter().all(|v| !v.active || v.track != 0),
        "track 0 voice must have been stolen by track 1"
    );

    let expected_fade_frames = usize::from(super::voice_pool::steal_fade_frames(sr_f));
    let mut prev_scope = f32::INFINITY;
    for f in 0..expected_fade_frames {
        let out = pool.render_frame(&samples, None);
        let expected_mid = stereo_mid_f32(out.sources[0]);
        assert!(
            out.track_scope[0] > 0.0,
            "frame {f}: track_scope[0] must remain non-zero throughout steal micro-fade"
        );
        assert!(
            (out.track_scope[0] - expected_mid).abs() < 1e-6,
            "frame {f}: steal fade track_scope[0]={} must equal stereo_mid_f32(sources[0])={expected_mid}",
            out.track_scope[0]
        );
        assert!(
            out.track_scope[0] < prev_scope,
            "frame {f}: steal fade track_scope[0] must decay monotonically ({} vs {prev_scope})",
            out.track_scope[0]
        );
        if f == 0 {
            assert!(
                (out.track_scope[0] - stereo_mid_f32(pre_steal.sources[0])).abs() < 1e-5,
                "first steal-fade frame must start seamlessly at stolen voice mid output"
            );
        }
        prev_scope = out.track_scope[0];
    }

    // After the micro-fade completes, track 0 audio and track_scope both settle to exact 0.0.
    let done = pool.render_frame(&samples, None);
    assert_eq!(done.sources[0], [0.0, 0.0]);
    assert_eq!(done.track_scope[0], 0.0);

    // Live track reorder and removal mid-fade must remap or drop in-flight steal fades cleanly.
    pool.clear_all();
    pool.trigger_on_track(
        &ctx,
        &TriggerRequest {
            inst_index: 0,
            live_key: None,
            midi: 60,
            velocity: 1.0,
            gate_seconds: 10.0,
            seed: 101,
            mod_pos: None,
            locks: &[],
        },
    );
    for _ in 0..16 {
        pool.render_frame(&samples, None);
    }
    for i in 0..VOICES {
        pool.trigger_on_track(
            &ctx,
            &TriggerRequest {
                inst_index: 1,
                live_key: None,
                midi: 60,
                velocity: 1.0,
                gate_seconds: 10.0,
                seed: 200 + i as u32,
                mod_pos: None,
                locks: &[],
            },
        );
    }
    let mut swapped = score.clone();
    swapped.instruments.swap(0, 1);
    pool.remap_tracks(&swapped);
    let after_swap = pool.render_frame(&samples, None);
    assert!(
        after_swap.track_scope[1] > 0.0
            && (after_swap.track_scope[1] - stereo_mid_f32(after_swap.sources[1])).abs() < 1e-6,
        "reordered steal fade must follow its track_id to index 1"
    );

    let mut removed = swapped;
    removed.instruments.pop();
    pool.remap_tracks(&removed);
    let after_remove = pool.render_frame(&samples, None);
    assert_eq!(after_remove.sources[1], [0.0, 0.0]);
    assert_eq!(after_remove.track_scope[1], 0.0);
}
