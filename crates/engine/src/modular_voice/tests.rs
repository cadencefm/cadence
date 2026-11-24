use super::*;
use crate::mix_engine::tests::assert_no_heap_operations;
use cadence_core::dsp::acid_filter_decay_ms;
use cadence_core::rack::{
    compile_zone_graph, DriveMode, EnvMode, NoiseColor, PatchCable, RackModule, RackPatch,
    VoiceMode, Waveform,
};
use cadence_core::sampler::SampleMetadata;
use std::f64::consts::TAU;

fn start(midi: u8, gate_frames: Option<u32>) -> VoiceStart {
    VoiceStart {
        midi,
        velocity: 0.9,
        gate_frames,
        seed: 0x1234_abcd,
        clock_seconds: 1.25,
    }
}

fn spec<'a>(
    zone: &'a RackZone,
    graph: &'a CompiledZoneGraph,
    sample_rate: f64,
    bpm: f64,
) -> ZoneTriggerSpec<'a> {
    ZoneTriggerSpec {
        inst_id: 1,
        zone,
        graph,
        sample_rate,
        bpm,
        zone_pan: zone.pan,
        rack_tune: 0.0,
        is_preview: false,
    }
}

fn synthetic_sample(frames: usize, sample_rate: u32, freq_hz: f64) -> Arc<SampleData> {
    let left: Box<[f32]> = (0..frames)
        .map(|i| (TAU * freq_hz * i as f64 / f64::from(sample_rate)).sin() as f32)
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

fn dft_power(samples: &[f64], freq_hz: f64, sr: f64) -> f64 {
    let n = samples.len();
    let (mut re, mut im) = (0.0, 0.0);
    for (i, &val) in samples.iter().enumerate() {
        let win = 0.5 - 0.5 * (TAU * i as f64 / (n.saturating_sub(1).max(1)) as f64).cos();
        let phase = TAU * freq_hz * i as f64 / sr;
        re += val * win * phase.cos();
        im += val * win * phase.sin();
    }
    (re * re + im * im) / (n as f64).powi(2)
}

fn build_zone(modules: Vec<RackModule>, cables: Vec<PatchCable>) -> RackZone {
    let zone = RackZone {
        id: 1,
        name: "Test".into(),
        low_midi: 0,
        high_midi: 127,
        root_midi: 60,
        low_vel: 0.0,
        high_vel: 1.0,
        gain_db: 0.0,
        pan: 0.0,
        choke_group: None,
        voice: VoiceSettings::default(),
        modules,
        cables,
    };
    zone.validate().expect("test zone must validate");
    zone
}

#[test]
fn dag_topological_order_is_independent_of_authored_module_order() {
    let forward = RackPatch::default_synth().zones.remove(0);
    let mut reversed = forward.clone();
    reversed.modules.reverse();
    reversed.cables.reverse();

    let g_fwd = compile_zone_graph(&forward).unwrap();
    let g_rev = compile_zone_graph(&reversed).unwrap();
    assert_eq!(g_fwd.ops.len(), g_rev.ops.len());
    for (a, b) in g_fwd.ops.iter().zip(g_rev.ops.iter()) {
        assert_eq!(a.module_id, b.module_id);
        assert_eq!(a.kind, b.kind);
    }

    let mut v_fwd = ModularVoice::new_boxed(
        &spec(&forward, &g_fwd, 48_000.0, 120.0),
        start(60, Some(2400)),
    );
    let mut v_rev = ModularVoice::new_boxed(
        &spec(&reversed, &g_rev, 48_000.0, 120.0),
        start(60, Some(2400)),
    );
    let bank = Vec::new();
    let mut energy = 0.0;
    for _ in 0..8000 {
        let a = v_fwd.next_frame(&bank, None);
        let b = v_rev.next_frame(&bank, None);
        assert_eq!(a, b);
        energy += f64::from(a[0] * a[0]);
    }
    assert!(energy > 1.0);
}

#[test]
fn zero_heap_allocations_during_trigger_retarget_update_and_processing() {
    let rack = RackPatch::acid_303();
    let zone = &rack.zones[0];
    let graph = compile_zone_graph(zone).unwrap();
    let mut voice = ModularVoice::new_boxed_inactive(48_000.0);
    let bank = Vec::new();

    assert_no_heap_operations(|| {
        let onset = start(48, Some(4800));
        voice.begin_trigger_from_zone(&spec(zone, &graph, 48_000.0, 120.0));
        voice.apply_trigger_module_param(2, ModuleParam::Cutoff, 0.4, 0, true);
        voice.apply_trigger_cable_norm(5, 0.75, true);
        voice.finalize_trigger(onset, false, false);
        for _ in 0..1024 {
            let f = voice.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
        }
        voice.update_continuous_from_zone(zone, 0.0, 0.0, 128.0);
        voice.retarget_from_zone(&spec(zone, &graph, 48_000.0, 128.0), start(55, None), true);
        for _ in 0..1024 {
            let f = voice.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
        }
        voice.note_off();
        voice.choke(48_000.0);
        for _ in 0..512 {
            let f = voice.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
        }
    });
}

#[test]
fn feedback_z1_self_fm_and_resonant_drive_loop_remain_stable_and_finite() {
    // 1. Self-FM (`VCO -> VCO.FmIn` feedback edge) adds harmonics over pure sine and stays bounded.
    let pure_zone = build_zone(
        vec![
            RackModule::new(
                1,
                "VCO",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    level: 0.85,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(2, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![PatchCable::new(1, 1, 2, InPort::In, 1.0)],
    );
    let mut self_fm_zone = pure_zone.clone();
    // Musical DX7 feedback regime (`amt = 0.25` -> ~0.83 rad feedback index -> rich saw-like harmonics).
    self_fm_zone
        .cables
        .push(PatchCable::new(2, 1, 1, InPort::FmIn, 0.25));
    let mut chaotic_fm_zone = pure_zone.clone();
    // Extreme chaotic self-FM (`amt = 0.85`) must remain strictly finite and bounded!
    chaotic_fm_zone
        .cables
        .push(PatchCable::new(2, 1, 1, InPort::FmIn, 0.85));

    let g_pure = compile_zone_graph(&pure_zone).unwrap();
    let g_fm = compile_zone_graph(&self_fm_zone).unwrap();
    let g_chaos = compile_zone_graph(&chaotic_fm_zone).unwrap();
    assert!(g_fm.has_feedback());
    assert!(g_chaos.has_feedback());

    let bank = Vec::new();
    let mut v_pure =
        ModularVoice::new_boxed(&spec(&pure_zone, &g_pure, 48_000.0, 120.0), start(69, None));
    let mut v_fm = ModularVoice::new_boxed(
        &spec(&self_fm_zone, &g_fm, 48_000.0, 120.0),
        start(69, None),
    );
    let mut v_chaos = ModularVoice::new_boxed(
        &spec(&chaotic_fm_zone, &g_chaos, 48_000.0, 120.0),
        start(69, None),
    );

    let mut pure_buf = Vec::with_capacity(4800);
    let mut fm_buf = Vec::with_capacity(4800);
    for i in 0..9600 {
        let p = v_pure.next_frame(&bank, None);
        let f = v_fm.next_frame(&bank, None);
        let c = v_chaos.next_frame(&bank, None);
        assert!(f[0].is_finite() && f[1].is_finite() && f[0].abs() <= 1.0);
        assert!(c[0].is_finite() && c[1].is_finite() && c[0].abs() <= 1.0);
        if i >= 4800 {
            pure_buf.push(f64::from(p[0]));
            fm_buf.push(f64::from(f[0]));
        }
    }
    let h2_pure = dft_power(&pure_buf, 880.0, 48_000.0);
    let h2_fm = dft_power(&fm_buf, 880.0, 48_000.0);
    assert!(
        h2_fm > h2_pure * 1000.0 && h2_fm > 1e-3,
        "z-1 self-FM must generate rich 2nd harmonic: pure={h2_pure}, fm={h2_fm}"
    );

    // 2. Extreme positive feedback loop (`Filter -> Drive -> Filter.In` at +2.0 gain, res 0.95)
    // must never blow up to NaN/Inf across sample rates and must silence on force_release().
    let fb_zone = build_zone(
        vec![
            RackModule::new(1, "VCO", ModuleKind::Vco(VcoModule::default())),
            RackModule::new(
                2,
                "Filter",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::LowPass,
                    cutoff_hz: 1_200.0,
                    resonance: 0.95,
                    drive: 0.6,
                    ..FilterModule::default()
                }),
            ),
            RackModule::new(
                3,
                "Drive",
                ModuleKind::Drive(DriveModule {
                    mode: DriveMode::Fold,
                    drive: 0.9,
                    asymmetry: 0.4,
                    output_db: 6.0,
                    ..DriveModule::default()
                }),
            ),
            RackModule::new(4, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 2, 3, InPort::In, 1.0),
            PatchCable::new(3, 3, 2, InPort::In, 2.0), // z-1 +200% feedback!
            PatchCable::new(4, 3, 4, InPort::In, 1.0),
        ],
    );
    let g_fb = compile_zone_graph(&fb_zone).unwrap();
    assert!(g_fb.has_feedback());
    for sr in [8_000.0, 44_100.0, 48_000.0, 96_000.0] {
        let mut v = ModularVoice::new_boxed(&spec(&fb_zone, &g_fb, sr, 120.0), start(60, None));
        for _ in 0..4096 {
            let f = v.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite() && f[0].abs() <= 1.0);
        }
        v.force_release();
        for _ in 0..(sr * 2.1) as usize {
            let f = v.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
        }
        assert!(
            !v.is_active(),
            "feedback voice must terminate after force_release at sr={sr}"
        );
    }
}

#[test]
fn audio_rate_fm_and_four_quadrant_ring_mod_generate_exact_sidebands() {
    let sr = 48_000.0;
    let bank = Vec::new();

    // 1. Audio-rate 2x through-zero linear FM: 660 Hz Sine modulating 440 Hz Sine.
    let fm_zone = build_zone(
        vec![
            RackModule::new(
                1,
                "Carrier",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    pitch_tracking: PitchTracking::Fixed,
                    fixed_hz: 440.0,
                    level: 0.9,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "Mod",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    pitch_tracking: PitchTracking::Fixed,
                    fixed_hz: 660.0,
                    level: 1.0,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(3, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 2, 1, InPort::FmIn, 0.55),
            PatchCable::new(2, 1, 3, InPort::In, 1.0),
        ],
    );
    let g_fm = compile_zone_graph(&fm_zone).unwrap();
    let mut v_fm = ModularVoice::new_boxed(&spec(&fm_zone, &g_fm, sr, 120.0), start(60, None));
    let mut fm_samples = Vec::with_capacity(9600);
    for i in 0..14_400 {
        let f = v_fm.next_frame(&bank, None);
        assert!(f[0].is_finite() && f[0].abs() <= 1.0);
        if i >= 4800 {
            fm_samples.push(f64::from(f[0]));
        }
    }
    // Sidebands at |440 - 660| = 220 Hz and 440 + 660 = 1100 Hz must be strong!
    let sb_lo = dft_power(&fm_samples, 220.0, sr);
    let sb_hi = dft_power(&fm_samples, 1100.0, sr);
    assert!(
        sb_lo > 1e-3 && sb_hi > 1e-3,
        "FM sidebands at 220 Hz ({sb_lo}) and 1100 Hz ({sb_hi}) must be prominent"
    );

    // 2. 4-quadrant Ring Modulator via VCA (`bias = 0.0`, `VCO1 (500 Hz) -> VCA.In`, `VCO2 (150 Hz) -> VCA.Cv`).
    let rm_zone = build_zone(
        vec![
            RackModule::new(
                1,
                "Carrier",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    pitch_tracking: PitchTracking::Fixed,
                    fixed_hz: 500.0,
                    level: 1.0,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "Mod",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    pitch_tracking: PitchTracking::Fixed,
                    fixed_hz: 150.0,
                    level: 1.0,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                3,
                "RingVCA",
                ModuleKind::Vca(VcaModule {
                    gain: 1.0,
                    bias: 0.0,
                }),
            ),
            RackModule::new(4, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 1, 3, InPort::In, 1.0),
            PatchCable::new(2, 2, 3, InPort::Cv, 1.0),
            PatchCable::new(3, 3, 4, InPort::In, 1.0),
        ],
    );
    let g_rm = compile_zone_graph(&rm_zone).unwrap();
    let mut v_rm = ModularVoice::new_boxed(&spec(&rm_zone, &g_rm, sr, 120.0), start(60, None));
    let mut rm_samples = Vec::with_capacity(9600);
    for i in 0..14_400 {
        let f = v_rm.next_frame(&bank, None);
        if i >= 4800 {
            rm_samples.push(f64::from(f[0]));
        }
    }
    let carrier = dft_power(&rm_samples, 500.0, sr);
    let diff = dft_power(&rm_samples, 350.0, sr);
    let sum = dft_power(&rm_samples, 650.0, sr);
    assert!(
        diff > 1e-3 && sum > 1e-3,
        "Ring mod must create 350 Hz ({diff}) and 650 Hz ({sum}) sidebands"
    );
    assert!(
        carrier < diff * 1e-4,
        "4-quadrant ring mod must suppress carrier 500 Hz ({carrier}) vs sideband ({diff})"
    );
}

#[test]
fn post_filter_noise_bypasses_lowpass_and_outlives_tonal_envelope_and_pink_tilts_low() {
    let sr = 48_000.0;
    let bank = Vec::new();

    // Parallel topology: VCO -> 200 Hz LP -> Body VCA -> Out (5 ms decay)
    //                    Noise (HP 4 kHz) -> Noise VCA -> Out (300 ms decay, post-filter!)
    let zone = build_zone(
        vec![
            RackModule::new(
                1,
                "Body",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    level: 0.8,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "LP",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::LowPass,
                    cutoff_hz: 200.0,
                    ..FilterModule::default()
                }),
            ),
            RackModule::new(3, "BodyVCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                4,
                "BodyEnv",
                ModuleKind::Env(EnvModule {
                    attack_ms: 0.2,
                    decay_ms: 5.0,
                    sustain: 0.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                5,
                "Noise",
                ModuleKind::Noise(NoiseModule {
                    color: NoiseColor::White,
                    highpass_hz: 4_000.0,
                    lowpass_hz: 18_000.0,
                    level: 0.8,
                }),
            ),
            RackModule::new(6, "NoiseVCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                7,
                "NoiseEnv",
                ModuleKind::Env(EnvModule {
                    attack_ms: 0.2,
                    decay_ms: 300.0,
                    sustain: 0.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(8, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 2, 3, InPort::In, 1.0),
            PatchCable::new(3, 4, 3, InPort::Cv, 1.0),
            PatchCable::new(4, 3, 8, InPort::In, 1.0),
            PatchCable::new(5, 5, 6, InPort::In, 1.0),
            PatchCable::new(6, 7, 6, InPort::Cv, 1.0),
            PatchCable::new(7, 6, 8, InPort::In, 1.0),
        ],
    );
    let g = compile_zone_graph(&zone).unwrap();
    assert_eq!(
        g.lifetime_envs.len(),
        2,
        "both BodyEnv and NoiseEnv gate lifetime"
    );

    let mut voice = ModularVoice::new_boxed(&spec(&zone, &g, sr, 120.0), start(48, Some(100)));
    // Advance 50 ms (2400 frames): BodyEnv (5 ms) is done, NoiseEnv (300 ms) is still active!
    let mut late_energy = 0.0;
    let mut hf_diff = 0.0;
    let mut prev = 0.0;
    for i in 0..4800 {
        let f = voice.next_frame(&bank, None);
        if i >= 2400 {
            let s = f64::from(f[0]);
            late_energy += s * s;
            hf_diff += (s - prev).powi(2);
            prev = s;
        }
    }
    assert!(
        voice.is_active(),
        "post-filter NoiseEnv must keep voice alive"
    );
    assert!(
        late_energy > 1.0,
        "post-filter noise must remain audible: {late_energy}"
    );
    assert!(
        hf_diff > late_energy * 0.8,
        "4 kHz HPF post-filter noise must preserve HF crispness: hf_diff={hf_diff}, energy={late_energy}"
    );

    // Pink vs White noise spectral tilt check (mean squared sample delta / energy).
    let measure_hf_ratio = |color: NoiseColor| -> f64 {
        let nz = build_zone(
            vec![
                RackModule::new(
                    1,
                    "N",
                    ModuleKind::Noise(NoiseModule {
                        color,
                        highpass_hz: 20.0,
                        lowpass_hz: 20_000.0,
                        level: 0.8,
                    }),
                ),
                RackModule::new(2, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![PatchCable::new(1, 1, 2, InPort::In, 1.0)],
        );
        let gn = compile_zone_graph(&nz).unwrap();
        let mut vn = ModularVoice::new_boxed(&spec(&nz, &gn, sr, 120.0), start(60, None));
        let (mut e, mut d, mut last) = (0.0, 0.0, 0.0);
        for _ in 0..16_000 {
            let s = f64::from(vn.next_frame(&bank, None)[0]);
            e += s * s;
            d += (s - last).powi(2);
            last = s;
        }
        d / e.max(1e-12)
    };
    let white_tilt = measure_hf_ratio(NoiseColor::White);
    let pink_tilt = measure_hf_ratio(NoiseColor::Pink);
    assert!(
        pink_tilt < white_tilt * 0.45,
        "Pink noise must have much warmer HF/LF ratio than White: pink={pink_tilt:.3} vs white={white_tilt:.3}"
    );
}

#[test]
fn sampler_in_rack_supports_pitch_cv_filter_and_trig_in_retrigger() {
    let sr = 24_000.0;
    let sample = synthetic_sample(4800, sr as u32, 220.0);
    let inst_id = 7u64;
    let key = module_sample_key(inst_id, 1, 1);
    let bank: SampleBank = vec![(key, Arc::clone(&sample))];

    // 1. Sampler with +12 st PitchCv from HoldDecay Env -> shifts 220 Hz sample up to 440 Hz!
    let pitched_zone = build_zone(
        vec![
            RackModule::new(
                1,
                "Smp",
                ModuleKind::Sampler(SamplerModule {
                    path: Some("./test.wav".into()),
                    root_midi: 60,
                    sample_end: 4800,
                    one_shot: true,
                    ..SamplerModule::default()
                }),
            ),
            RackModule::new(
                2,
                "PEnv",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::HoldDecay,
                    hold_ms: 250.0, // stays at +1.0 for entire 200ms sample!
                    decay_ms: 50.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                3,
                "LP",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::LowPass,
                    cutoff_hz: 12_000.0,
                    ..FilterModule::default()
                }),
            ),
            RackModule::new(4, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 2, 1, InPort::PitchCv, 12.0), // +12 st octave up!
            PatchCable::new(2, 1, 3, InPort::In, 1.0),
            PatchCable::new(3, 3, 4, InPort::In, 1.0),
        ],
    );
    let g = compile_zone_graph(&pitched_zone).unwrap();
    let mut voice = ModularVoice::new_boxed(
        &ZoneTriggerSpec {
            inst_id,
            ..spec(&pitched_zone, &g, sr, 120.0)
        },
        start(60, None),
    );
    let mut buf = Vec::with_capacity(2000);
    for i in 0..2200 {
        let f = voice.next_frame(&bank, None);
        if i >= 200 {
            buf.push(f64::from(f[0]));
        }
    }
    let p220 = dft_power(&buf, 220.0, sr);
    let p440 = dft_power(&buf, 440.0, sr);
    assert!(
        p440 > p220 * 50.0 && p440 > 1e-2,
        "Sampler +12 st PitchCv must play 220 Hz sample at 440 Hz: p220={p220}, p440={p440}"
    );
    // Let one-shot sample finish: voice must deactivate cleanly!
    for _ in 0..10_000 {
        voice.next_frame(&bank, None);
    }
    assert!(
        !voice.is_active(),
        "one-shot sampler zone must terminate after sample_end"
    );

    // 2. TrigIn retrigger: Square LFO at 20 Hz (50ms period) retriggers a short 10ms (exact 5-cycle) sample twice!
    let short_sample = synthetic_sample(240, sr as u32, 500.0); // 10 ms, exact 5 cycles (zero DC integral)
    let short_bank: SampleBank = vec![(key, short_sample)];
    let trig_zone = build_zone(
        vec![
            RackModule::new(
                1,
                "Smp",
                ModuleKind::Sampler(SamplerModule {
                    path: Some("./short.wav".into()),
                    root_midi: 60,
                    sample_end: 240,
                    one_shot: true,
                    ..SamplerModule::default()
                }),
            ),
            RackModule::new(
                2,
                "Clock",
                ModuleKind::Lfo(LfoModule {
                    shape: LfoShape::Square,
                    rate_hz: 20.0, // 1200 frames/cycle at 24 kHz
                    retrigger: true,
                    bipolar: true,
                    ..LfoModule::default()
                }),
            ),
            RackModule::new(3, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 2, 1, InPort::TrigIn, 1.0),
            PatchCable::new(2, 1, 3, InPort::In, 1.0),
        ],
    );
    let gt = compile_zone_graph(&trig_zone).unwrap();
    let mut vt = ModularVoice::new_boxed(
        &ZoneTriggerSpec {
            inst_id,
            ..spec(&trig_zone, &gt, sr, 120.0)
        },
        start(60, None),
    );
    let mut frames = Vec::with_capacity(1600);
    for _ in 0..1600 {
        frames.push(vt.next_frame(&short_bank, None)[0].abs());
    }
    let burst1 = frames[10..200].iter().copied().fold(0.0_f32, f32::max);
    let gap = frames[800..1150].iter().copied().fold(0.0_f32, f32::max);
    let burst2 = frames[1210..1400].iter().copied().fold(0.0_f32, f32::max);
    assert!(
        burst1 > 0.2,
        "initial LFO rising edge triggers sampler: {burst1}"
    );
    assert!(
        gap < 0.002,
        "silence between LFO retriggers after DC settle: {gap}"
    );
    assert!(
        burst2 > 0.2,
        "second LFO rising edge retriggers sampler: {burst2}"
    );
}

#[test]
fn multi_burst_predelay_env_and_lofi_quantizer_and_drive_asymmetry() {
    let sr = 48_000.0;
    let bank = Vec::new();

    // 1. Multi-burst + pre-delay Env (4 bursts spaced 10ms, 10ms pre-delay — TR-909 clap contour!).
    let clap_zone = build_zone(
        vec![
            RackModule::new(
                1,
                "VCO",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    pitch_tracking: PitchTracking::Fixed,
                    fixed_hz: 1_000.0,
                    level: 1.0,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(2, "VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                3,
                "ClapEnv",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 0.5,
                    decay_ms: 40.0,
                    sustain: 0.0,
                    release_ms: 10.0,
                    trigger_mode: TriggerMode::OneShot,
                    delay_ms: 10.0,
                    bursts: 4,
                    burst_spacing_ms: 10.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(4, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 3, 2, InPort::Cv, 1.0),
            PatchCable::new(3, 2, 4, InPort::In, 1.0),
        ],
    );
    let gc = compile_zone_graph(&clap_zone).unwrap();
    let mut vc = ModularVoice::new_boxed(&spec(&clap_zone, &gc, sr, 120.0), start(60, None));
    let mut env_trace = Vec::with_capacity(4800);
    for _ in 0..4800 {
        let _ = vc.next_frame(&bank, None);
        let (_, lvl) = vc.env_stage_and_level(3).unwrap();
        env_trace.push(lvl);
    }
    // First 9.5 ms (~456 frames) must be silent during pre-delay.
    assert!(
        env_trace[..450].iter().all(|&v| v == 0.0),
        "pre-delay must hold silence for 10 ms"
    );
    // Each of the 4 bursts at ~10, 20, 30, 40 ms (480, 960, 1440, 1920) peaks near 1.0!
    for b in 0..4 {
        let center = 480 * (b + 1);
        let pk = env_trace[center..center + 160]
            .iter()
            .copied()
            .fold(0.0_f64, f64::max);
        assert!(pk > 0.85, "burst {b} must peak > 0.85, got {pk}");
    }
    for _ in 0..10_000 {
        vc.next_frame(&bank, None);
    }
    assert!(
        !vc.is_active(),
        "multi-burst OneShot env must finish cleanly"
    );

    // 2. LoFi quantizer: 2-bit / 4 kHz clock produces stepped quantized output vs dry mix=0.
    let lofi_zone = build_zone(
        vec![
            RackModule::new(
                1,
                "VCO",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    pitch_tracking: PitchTracking::Fixed,
                    fixed_hz: 300.0,
                    level: 0.9,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "LoFi",
                ModuleKind::LoFi(LoFiModule {
                    bits: 2.0,
                    rate_hz: 4_000.0,
                    mix: 1.0,
                }),
            ),
            RackModule::new(3, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 2, 3, InPort::In, 1.0),
        ],
    );
    let gl = compile_zone_graph(&lofi_zone).unwrap();
    let mut vl = ModularVoice::new_boxed(&spec(&lofi_zone, &gl, sr, 120.0), start(60, None));
    let mut dry_zone = lofi_zone.clone();
    if let ModuleKind::LoFi(ref mut lf) = dry_zone.modules[1].kind {
        lf.mix = 0.0;
    }
    let mut vdry = ModularVoice::new_boxed(&spec(&dry_zone, &gl, sr, 120.0), start(60, None));
    let mut diff_energy = 0.0;
    let mut lofi_buf = Vec::with_capacity(4800);
    let mut dry_buf = Vec::with_capacity(4800);
    for i in 0..6400 {
        let l = vl.next_frame(&bank, None);
        let d = vdry.next_frame(&bank, None);
        assert!(l[0].is_finite());
        if i >= 1600 {
            diff_energy += f64::from(l[0] - d[0]).powi(2);
            lofi_buf.push(f64::from(l[0]));
            dry_buf.push(f64::from(d[0]));
        }
    }
    assert!(
        diff_energy > 5.0,
        "LoFi 2-bit/4kHz must audibly crush signal: {diff_energy}"
    );
    // Strong odd harmonic 3*300 = 900 Hz from bit quantization + 4000-300 = 3700 Hz ZOH alias!
    let lofi_h3 = dft_power(&lofi_buf, 900.0, sr);
    let dry_h3 = dft_power(&dry_buf, 900.0, sr);
    let lofi_alias = dft_power(&lofi_buf, 3_700.0, sr);
    assert!(
        lofi_h3 > dry_h3 * 1e6 && lofi_h3 > 5e-5,
        "2-bit quantization must generate strong 3rd harmonic: lofi_h3={lofi_h3}, dry_h3={dry_h3}"
    );
    assert!(
        lofi_alias > 1e-4,
        "4 kHz sample-rate hold must generate 3700 Hz alias image: {lofi_alias}"
    );

    // 3. Drive module with HardClip + positive asymmetry (moderate drive so positive rail clips
    // asymmetrically against the unclipped negative half-cycle) generates strong 2nd harmonic & zero DC!
    let drv_zone = build_zone(
        vec![
            RackModule::new(
                1,
                "VCO",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    pitch_tracking: PitchTracking::Fixed,
                    fixed_hz: 200.0,
                    level: 0.6,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "Drive",
                ModuleKind::Drive(DriveModule {
                    mode: DriveMode::HardClip,
                    drive: 0.25,
                    asymmetry: 0.85,
                    tone_hz: 16_000.0,
                    output_db: 0.0,
                    mix: 1.0,
                }),
            ),
            RackModule::new(3, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 2, 3, InPort::In, 1.0),
        ],
    );
    let gd = compile_zone_graph(&drv_zone).unwrap();
    let mut vd = ModularVoice::new_boxed(&spec(&drv_zone, &gd, sr, 120.0), start(60, None));
    let mut sym_zone = drv_zone.clone();
    if let ModuleKind::Drive(ref mut d) = sym_zone.modules[1].kind {
        d.asymmetry = 0.0;
    }
    let mut vsym = ModularVoice::new_boxed(&spec(&sym_zone, &gd, sr, 120.0), start(60, None));
    let mut drv_buf = Vec::with_capacity(4800);
    let mut sym_buf = Vec::with_capacity(4800);
    let mut dc = 0.0;
    for i in 0..14_400 {
        let f = vd.next_frame(&bank, None);
        let sym = vsym.next_frame(&bank, None);
        if i >= 9600 {
            drv_buf.push(f64::from(f[0]));
            sym_buf.push(f64::from(sym[0]));
            dc += f64::from(f[0]);
        }
    }
    let h1 = dft_power(&drv_buf, 200.0, sr);
    let h2 = dft_power(&drv_buf, 400.0, sr);
    let h2_sym = dft_power(&sym_buf, 400.0, sr);
    assert!(
        h2 > h1 * 0.005 && h2 > h2_sym * 1000.0,
        "asymmetric HardClip must create strong 2nd harmonic vs symmetric: h1={h1}, h2={h2}, h2_sym={h2_sym}"
    );
    assert!(
        (dc / 4800.0).abs() < 2e-3,
        "Drive + Out DcBlocker must remove DC: {dc}"
    );
}

#[test]
fn unison_detune_stereo_width_and_equal_power_pan_law() {
    let sr = 48_000.0;
    let bank = Vec::new();

    let mut zone = RackPatch::default_synth().zones.remove(0);
    if let ModuleKind::Vco(ref mut vco) = zone.modules[0].kind {
        vco.waveform = Waveform::Sine;
        vco.level = 0.8;
        vco.unison_voices = 1;
        vco.unison_detune_cents = 40.0;
        vco.unison_width = 0.0;
    }
    if let ModuleKind::Filter(ref mut flt) = zone.modules[1].kind {
        flt.cutoff_hz = 20_000.0;
        flt.resonance = 0.0;
        flt.drive = 0.0;
    }
    if let ModuleKind::Env(ref mut env) = zone.modules[3].kind {
        env.attack_ms = 1.0;
        env.decay_ms = 1.0;
        env.sustain = 1.0;
    }
    let g = compile_zone_graph(&zone).unwrap();

    // 1. Equal-power pan preserves constant L² + R² across [-1.0, +1.0].
    let measure_power = |pan: f32| -> f64 {
        let mut z = zone.clone();
        if let ModuleKind::Out(ref mut out) = z.modules[5].kind {
            out.pan = pan;
        }
        let mut v = ModularVoice::new_boxed(&spec(&z, &g, sr, 120.0), start(69, None));
        for _ in 0..4800 {
            v.next_frame(&bank, None);
        }
        let mut sum = 0.0;
        for _ in 0..4800 {
            let f = v.next_frame(&bank, None);
            sum += f64::from(f[0]).powi(2) + f64::from(f[1]).powi(2);
        }
        sum / 4800.0
    };
    let p_center = measure_power(0.0);
    for pan in [-1.0_f32, -0.5, 0.5, 1.0] {
        let p = measure_power(pan);
        let diff_db = 10.0 * (p / p_center).log10();
        assert!(
            diff_db.abs() < 0.1,
            "pan={pan}: equal-power drifted {diff_db:+.3} dB from center"
        );
    }

    // 2. 7-voice unison produces detuned sidebands & stereo width when width=1.0, mono when width=0.0.
    let mut uni_zone = zone.clone();
    if let ModuleKind::Vco(ref mut vco) = uni_zone.modules[0].kind {
        vco.unison_voices = 7;
        vco.unison_detune_cents = 40.0;
        vco.unison_width = 1.0;
    }
    let mut v_wide = ModularVoice::new_boxed(&spec(&uni_zone, &g, sr, 120.0), start(69, None));
    let mut stereo_diff = 0.0;
    let mut wide_l = Vec::with_capacity(24_000);
    for i in 0..28_800 {
        let f = v_wide.next_frame(&bank, None);
        assert!(f[0].is_finite() && f[1].is_finite());
        stereo_diff += f64::from((f[0] - f[1]).abs());
        if i >= 4800 {
            wide_l.push(f64::from(f[0]));
        }
    }
    assert!(
        stereo_diff > 100.0,
        "7-voice wide unison must produce stereo image"
    );
    let lower_side = 440.0 * 2.0_f64.powf(-40.0 / 1200.0);
    assert!(
        dft_power(&wide_l, lower_side, sr) > 1e-4,
        "unison lower -40c sideband must be audible"
    );

    if let ModuleKind::Vco(ref mut vco) = uni_zone.modules[0].kind {
        vco.unison_width = 0.0;
    }
    let mut v_mono = ModularVoice::new_boxed(&spec(&uni_zone, &g, sr, 120.0), start(69, None));
    for _ in 0..4800 {
        let f = v_mono.next_frame(&bank, None);
        assert!(
            (f[0] - f[1]).abs() < 1e-6,
            "width=0 unison must stay centered mono"
        );
    }
}

#[test]
fn mono_legato_glide_adopts_envelope_progress_and_non_legato_retriggers_click_free() {
    let sr = 48_000.0;
    let bank = Vec::new();
    let mut zone = RackPatch::default_synth().zones.remove(0);
    zone.voice.voice_mode = VoiceMode::Mono;
    zone.voice.legato = true;
    zone.voice.glide_ms = 60.0;
    if let ModuleKind::Env(ref mut amp) = zone.modules[3].kind {
        amp.attack_ms = 100.0;
        amp.decay_ms = 200.0;
        amp.sustain = 0.8;
        amp.release_ms = 400.0;
    }
    let g = compile_zone_graph(&zone).unwrap();

    let mut voice = ModularVoice::new_boxed(&spec(&zone, &g, sr, 120.0), start(38, None));
    // Run 1200 frames = 25 ms = 25% of 100 ms attack.
    for _ in 0..1200 {
        voice.next_frame(&bank, None);
    }
    let (stage_before, lvl_before) = voice.env_stage_and_level(4).unwrap();
    assert_eq!(stage_before, AdsrStage::Attack);
    assert!((lvl_before - 0.25).abs() < 1e-6);

    // Legato retarget to MIDI 50 with new 200 ms attack and 20 ms release:
    // adopts 25% level and 75% * 200ms = 150ms (7200 frames) remaining attack, plus pitch glides!
    let mut next_zone = zone.clone();
    if let ModuleKind::Env(ref mut amp) = next_zone.modules[3].kind {
        amp.attack_ms = 200.0;
        amp.release_ms = 20.0;
    }
    voice.retarget_from_zone(&spec(&next_zone, &g, sr, 120.0), start(50, None), true);
    let (stage_after, lvl_after) = voice.env_stage_and_level(4).unwrap();
    assert_eq!(stage_after, AdsrStage::Attack);
    assert!((lvl_after - lvl_before).abs() < 1e-9);
    assert_eq!(voice.current_note_pitch(), 38.0);

    voice.next_frame(&bank, None);
    assert!(voice.current_note_pitch() > 38.0 && voice.current_note_pitch() < 50.0);

    for _ in 0..7199 {
        voice.next_frame(&bank, None);
    }
    let (stage_peak, lvl_peak) = voice.env_stage_and_level(4).unwrap();
    assert_eq!(stage_peak, AdsrStage::Decay);
    assert!((lvl_peak - 1.0).abs() < 1e-6);
    assert!((voice.current_note_pitch() - 50.0).abs() < 0.1);

    // Release into Release stage, then retarget while releasing -> inherits glide & retriggers smoothly from current level!
    voice.note_off();
    for _ in 0..200 {
        voice.next_frame(&bank, None);
    }
    assert!(voice.is_releasing());
    let rel_lvl = voice.env_stage_and_level(4).unwrap().1;
    assert!(rel_lvl > 0.1);
    voice.retarget_from_zone(&spec(&zone, &g, sr, 120.0), start(38, None), true);
    assert!(!voice.is_releasing());
    let (st_retrig, lvl_retrig) = voice.env_stage_and_level(4).unwrap();
    assert_eq!(st_retrig, AdsrStage::Attack);
    assert!(
        (lvl_retrig - rel_lvl).abs() < 1e-9,
        "non-legato/releasing retrigger must preserve level (no drop to 0 click)"
    );
    assert!(voice.current_note_pitch() > 49.0);
    voice.next_frame(&bank, None);
    assert!(voice.current_note_pitch() < 50.0 && voice.current_note_pitch() > 38.0);
}

#[test]
fn acid_303_accent_boosts_cutoff_and_snaps_filter_env_decay() {
    let sr = 48_000.0;
    let bank = Vec::new();
    let rack = RackPatch::acid_303();
    let zone = &rack.zones[0];
    let g = compile_zone_graph(zone).unwrap();

    let mut v_norm = ModularVoice::new_boxed(
        &spec(zone, &g, sr, 120.0),
        VoiceStart {
            velocity: 0.70,
            ..start(45, Some(4800))
        },
    );
    let mut v_acc = ModularVoice::new_boxed(
        &spec(zone, &g, sr, 120.0),
        VoiceStart {
            velocity: 0.98,
            ..start(45, Some(4800))
        },
    );

    // After 2 ms (96 frames), accented Acid 303 opens filter cutoff significantly higher!
    for _ in 0..96 {
        v_norm.next_frame(&bank, None);
        v_acc.next_frame(&bank, None);
    }
    let cut_norm = v_norm.effective_filter_cutoff_hz();
    let cut_acc = v_acc.effective_filter_cutoff_hz();
    assert!(
        cut_acc > cut_norm * 2.0,
        "Acid accent must open cutoff dramatically: norm={cut_norm} Hz vs acc={cut_acc} Hz"
    );

    // After 140 ms (~6720 frames), accented Filter Env has decayed faster via acid_filter_decay_ms!
    for _ in 0..6720 {
        v_norm.next_frame(&bank, None);
        v_acc.next_frame(&bank, None);
    }
    let env_norm = v_norm.env_stage_and_level(5).unwrap().1;
    let env_acc = v_acc.env_stage_and_level(5).unwrap().1;
    assert!(
        env_acc < env_norm * 0.5,
        "Acid accent must snap Filter Env decay shorter: norm={env_norm:.4} vs acc={env_acc:.4}"
    );
    assert!(acid_filter_decay_ms(FilterMode::AcidLowPass, 260.0, 2.2, 0.98) < 150.0);
}

#[test]
fn continuous_updates_respect_per_voice_locks_and_on_trigger_policy() {
    let sr = 48_000.0;
    let bank = Vec::new();
    let mut zone = RackPatch::default_synth().zones.remove(0);
    let g = compile_zone_graph(&zone).unwrap();

    let mut voice = ModularVoice::new_boxed_inactive(sr);
    let onset = start(60, None);
    voice.begin_trigger_from_zone(&spec(&zone, &g, sr, 120.0));
    // Lock Filter Cutoff (module #2) at 7000 Hz and Cable #5 (Env Mod) at +2.0 oct.
    voice.apply_trigger_module_param(
        2,
        ModuleParam::Cutoff,
        ModuleParam::Cutoff.normalize(7_000.0),
        0,
        true,
    );
    voice.apply_trigger_cable_norm(
        5,
        cadence_core::rack::normalize_cable_amount(ModuleKindTag::Filter, InPort::Cv, 2.0),
        true,
    );
    voice.finalize_trigger(onset, false, false);

    // Edit base zone: lower cutoff to 200 Hz, change waveform (OnTrigger), raise resonance (unlocked Continuous), zero cable #5.
    if let ModuleKind::Vco(ref mut vco) = zone.modules[0].kind {
        vco.waveform = Waveform::Pulse;
    }
    if let ModuleKind::Filter(ref mut flt) = zone.modules[1].kind {
        flt.cutoff_hz = 200.0;
        flt.resonance = 0.75;
    }
    zone.cables[4].amount = 0.0;

    voice.update_continuous_from_zone(&zone, 0.0, 0.0, 120.0);
    for _ in 0..4800 {
        voice.next_frame(&bank, None);
    }

    // Locked Filter Cutoff stayed at 7000 Hz, unlocked Resonance moved to 0.75,
    // OnTrigger Waveform stayed Saw, and locked Cable #5 stayed +2.0 oct!
    match voice.module_target_params(2).unwrap() {
        ModuleParams::Filter(f) => {
            assert!(
                (f.cutoff_hz - 7_000.0).abs() < 2.0,
                "locked cutoff drifted: {}",
                f.cutoff_hz
            );
            assert!(
                (f.resonance - 0.75).abs() < 1e-4,
                "unlocked resonance missed: {}",
                f.resonance
            );
        }
        _ => panic!("expected Filter"),
    }
    match voice.module_target_params(1).unwrap() {
        ModuleParams::Vco(v) => assert_eq!(
            v.waveform,
            Waveform::Saw,
            "OnTrigger waveform must not mutate mid-voice"
        ),
        _ => panic!("expected Vco"),
    }
    assert!(
        (voice.cable_target_amount(5).unwrap() - 2.0).abs() < 1e-4,
        "locked cable amount must survive continuous updates"
    );
}

#[test]
fn extreme_parameters_and_un_enveloped_zones_stay_finite_and_terminate_across_sample_rates() {
    let bank = Vec::new();
    let mut zone = RackPatch::acid_303().zones.remove(0);
    if let ModuleKind::Vco(ref mut vco) = zone.modules[0].kind {
        vco.unison_voices = 7;
        vco.unison_detune_cents = 50.0;
        vco.unison_width = 1.0;
    }
    if let ModuleKind::Filter(ref mut flt) = zone.modules[1].kind {
        flt.resonance = 0.95;
        flt.drive = 1.0;
        flt.vel_amount = 4.0;
    }
    if let ModuleKind::Env(ref mut amp) = zone.modules[3].kind {
        amp.release_ms = 10.0;
    }
    let g = compile_zone_graph(&zone).unwrap();

    for sr in [8_000.0, 44_100.0, 48_000.0, 96_000.0, 192_000.0, 384_000.0] {
        let mut voice = ModularVoice::new_boxed(
            &ZoneTriggerSpec {
                rack_tune: 24.0,
                ..spec(&zone, &g, sr, 300.0)
            },
            start(127, None),
        );
        for _ in 0..1000 {
            let f = voice.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite() && f[0].abs() <= 1.0 && f[1].abs() <= 1.0);
        }
        voice.force_release();
        for _ in 0..(sr * 2.1) as usize {
            let f = voice.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
        }
        assert!(!voice.is_active(), "voice latched at sr={sr}");
    }

    // Raw un-enveloped [VCO -> Out] zone must release cleanly via fallback gate on note_off().
    let raw_zone = build_zone(
        vec![
            RackModule::new(1, "VCO", ModuleKind::Vco(VcoModule::default())),
            RackModule::new(2, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![PatchCable::new(1, 1, 2, InPort::In, 1.0)],
    );
    let gr = compile_zone_graph(&raw_zone).unwrap();
    let mut vr =
        ModularVoice::new_boxed(&spec(&raw_zone, &gr, 48_000.0, 120.0), start(60, Some(480)));
    for _ in 0..24_000 {
        let f = vr.next_frame(&bank, None);
        assert!(f[0].is_finite() && f[1].is_finite());
    }
    assert!(
        !vr.is_active(),
        "un-enveloped [VCO -> Out] must terminate after gate expires"
    );
}

#[test]
fn resonator_string_and_nonlinear_feedback_plate_ring_out_and_are_zero_alloc() {
    let sr = 48_000.0;
    let bank = SampleBank::new();

    // 1. Struck String resonator: 12 ms noise burst -> Chromatic String at A4 (MIDI 69 = 440 Hz) -> Out.
    let string_zone = build_zone(
        vec![
            RackModule::new(
                1,
                "Stick",
                ModuleKind::Noise(NoiseModule {
                    color: NoiseColor::White,
                    highpass_hz: 200.0,
                    lowpass_hz: 12_000.0,
                    level: 1.0,
                }),
            ),
            RackModule::new(
                2,
                "Stick Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 0.2,
                    decay_ms: 12.0,
                    sustain: 0.0,
                    release_ms: 5.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(3, "Exc VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                4,
                "String Res",
                ModuleKind::Resonator(ResonatorModule {
                    mode: ResonatorMode::String,
                    pitch_tracking: PitchTracking::Chromatic,
                    structure: 0.0,
                    decay_ms: 300.0,
                    brightness: 0.5,
                    position: 0.25,
                    mix: 1.0,
                    ..ResonatorModule::default()
                }),
            ),
            RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 1, 3, InPort::In, 1.0),
            PatchCable::new(2, 2, 3, InPort::Cv, 1.0),
            PatchCable::new(3, 3, 4, InPort::In, 1.0),
            PatchCable::new(4, 4, 5, InPort::In, 1.0),
        ],
    );
    let gs = compile_zone_graph(&string_zone).unwrap();
    let mut vs =
        ModularVoice::new_boxed(&spec(&string_zone, &gs, sr, 120.0), start(69, Some(2000)));

    // Measure 440 Hz vs off-harmonic 330 Hz after stick burst has decayed (40..120 ms).
    let mut mag_440 = (0.0_f64, 0.0_f64);
    let mut mag_330 = (0.0_f64, 0.0_f64);
    assert_no_heap_operations(|| {
        for i in 0..8000 {
            let f = vs.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
            if i >= 2000 {
                let s = f64::from(f[0]);
                let a440 = TAU * 440.0 * (i as f64) / sr;
                let a330 = TAU * 330.0 * (i as f64) / sr;
                mag_440.0 += s * a440.cos();
                mag_440.1 += s * a440.sin();
                mag_330.0 += s * a330.cos();
                mag_330.1 += s * a330.sin();
            }
        }
    });
    let e440 = mag_440.0.hypot(mag_440.1);
    let e330 = mag_330.0.hypot(mag_330.1);
    assert!(
        e440 > e330 * 5.0 && e440 > 1.0,
        "struck A4 String resonator must ring strongly at 440 Hz (440={e440:.2} vs 330={e330:.2})"
    );
    // Voice must remain active well past the 12 ms exciter while the 300 ms resonator rings out.
    assert!(
        vs.is_active(),
        "voice must keep ringing during resonator tail"
    );

    // 2. Nonlinear z-1 feedback Plate cymbal (`Resonator -> Drive(Tape) -> Vca(Env) -> Resonator.In`).
    let plate_zone = build_zone(
        vec![
            RackModule::new(
                1,
                "Stick",
                ModuleKind::Noise(NoiseModule {
                    color: NoiseColor::White,
                    highpass_hz: 1500.0,
                    lowpass_hz: 16_000.0,
                    level: 1.0,
                }),
            ),
            RackModule::new(
                2,
                "Exc Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 0.2,
                    decay_ms: 40.0,
                    sustain: 0.0,
                    release_ms: 10.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(3, "Exc VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                4,
                "Bronze Plate",
                ModuleKind::Resonator(ResonatorModule {
                    mode: ResonatorMode::Plate,
                    pitch_tracking: PitchTracking::Fixed,
                    fixed_hz: 620.0,
                    structure: 0.5,
                    decay_ms: 450.0,
                    brightness: 0.9,
                    position: 0.8,
                    mix: 1.0,
                    ..ResonatorModule::default()
                }),
            ),
            RackModule::new(
                5,
                "Plate NL",
                ModuleKind::Drive(DriveModule {
                    mode: DriveMode::Tape,
                    drive: 0.75,
                    tone_hz: 15_000.0,
                    ..DriveModule::default()
                }),
            ),
            RackModule::new(6, "FB VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(7, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 1, 3, InPort::In, 1.0),
            PatchCable::new(2, 2, 3, InPort::Cv, 1.0),
            PatchCable::new(3, 3, 4, InPort::In, 1.0),
            PatchCable::new(4, 4, 5, InPort::In, 1.0),
            PatchCable::new(5, 5, 6, InPort::In, 0.45),
            PatchCable::new(6, 2, 6, InPort::Cv, 1.0),
            PatchCable::new(7, 6, 4, InPort::In, 1.0), // z-1 nonlinear plate feedback!
            PatchCable::new(8, 4, 7, InPort::In, 1.0),
        ],
    );
    let gp = compile_zone_graph(&plate_zone).unwrap();
    assert!(
        gp.cables.iter().any(|c| c.feedback),
        "expected z-1 feedback edge"
    );
    let mut vp = ModularVoice::new_boxed(&spec(&plate_zone, &gp, sr, 120.0), start(44, None));
    let mut peak = 0.0_f32;
    assert_no_heap_operations(|| {
        for _ in 0..48_000 {
            let f = vp.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite() && f[0].abs() <= 1.0);
            peak = peak.max(f[0].abs());
        }
    });
    assert!(
        peak > 0.05,
        "nonlinear plate crash must produce audible signal (peak={peak})"
    );
}
