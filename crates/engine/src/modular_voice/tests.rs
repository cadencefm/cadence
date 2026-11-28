use super::*;
use crate::mix_engine::tests::assert_no_heap_operations;
use cadence_core::dsp::{
    acid_filter_decay_ms, db_to_lin_f64, pan_mono_equal_power_f64, VOICE_GUARD_CEILING,
    VOICE_GUARD_KNEE,
};
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
        glide_from_pitch: None,
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
                    mode: FilterMode::ImLpf2,
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
                    mode: DriveMode::ShockPort,
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
            assert!(
                f[0].is_finite()
                    && f[1].is_finite()
                    && f[0].abs() < VOICE_GUARD_CEILING as f32
                    && f[1].abs() < VOICE_GUARD_CEILING as f32
            );
        }
        v.force_release();
        for _ in 0..(sr * 2.1) as usize {
            let f = v.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
            if !v.is_active() {
                break;
            }
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

    // 1. Audio-rate 1x through-zero linear FM: 660 Hz Sine modulating 440 Hz Sine.
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
                    mode: FilterMode::ImLpf2,
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
                    mode: FilterMode::ImLpf2,
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

    // 3. Drive module with ShockPort + positive asymmetry (moderate drive so positive rail
    // contracts asymmetrically against the linear negative half-cycle) generates strong 2nd harmonic & zero DC!
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
                    mode: DriveMode::ShockPort,
                    drive: 0.8,
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
        "asymmetric ShockPort must create strong 2nd harmonic vs symmetric: h1={h1}, h2={h2}, h2_sym={h2_sym}"
    );
    assert!(
        (dc / 4800.0).abs() < 2e-3,
        "Drive + Out DcBlocker must remove DC: {dc}"
    );
}

#[test]
fn unison_detune_and_stereo_width() {
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

    // 7-voice unison produces detuned sidebands & stereo width when width=1.0, mono when width=0.0.
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

    // After 220 ms (~10560 frames), accented Capacitor Filter Env has decayed faster via acid_filter_decay_ms!
    for _ in 0..10_560 {
        v_norm.next_frame(&bank, None);
        v_acc.next_frame(&bank, None);
    }
    let env_norm = v_norm.env_stage_and_level(5).unwrap().1;
    let env_acc = v_acc.env_stage_and_level(5).unwrap().1;
    assert!(
        env_acc < env_norm * 0.5,
        "Acid accent must snap Filter Env decay shorter: norm={env_norm:.4} vs acc={env_acc:.4}"
    );
    assert!(acid_filter_decay_ms(FilterMode::DiodeLadder, 260.0, 2.2, 0.98) < 150.0);
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
        vco.waveform = Waveform::MobiusPwm;
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
    // OnTrigger Waveform stayed MobiusJet, and locked Cable #5 stayed +2.0 oct!
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
            Waveform::MobiusJet,
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
            if !voice.is_active() {
                break;
            }
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
fn modal_harmonic_wave_drive_and_nonlinear_feedback_plate_ring_out_and_zero_alloc() {
    let sr = 48_000.0;
    let bank = SampleBank::new();

    // 1. Wave-excited Harmonic modal body: 12 ms noise burst -> Chromatic Modal at A4 (MIDI 69 = 440 Hz) -> Out.
    let harmonic_zone = build_zone(
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
                    decay_ms: 20.0,
                    sustain: 0.0,
                    release_ms: 5.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(3, "Exc VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                4,
                "Harmonic Modal",
                ModuleKind::Modal(ModalModule {
                    geometry: BodyGeometry::Harmonic,
                    atoms: 16,
                    pitch_tracking: PitchTracking::Chromatic,
                    pair_topology: PairTopology::Swarm,
                    coupling: 0.0,
                    absorption: 0.12,
                    radiation: 0.2,
                    brightness: 0.5,
                    headroom: 0.0,
                    strike_drive: 0.0,
                    mix: 1.0,
                    ..ModalModule::default()
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
    let gs = compile_zone_graph(&harmonic_zone).unwrap();
    let mut vs =
        ModularVoice::new_boxed(&spec(&harmonic_zone, &gs, sr, 120.0), start(69, Some(2000)));

    // Measure 440 Hz vs off-harmonic 330 Hz after stick burst has decayed (40..165 ms).
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
        e440 > e330 * 10.0 && e440 > 0.5,
        "excited A4 Harmonic modal body must ring strongly at 440 Hz (440={e440:.2} vs 330={e330:.2})"
    );
    // Voice must remain active well past the 12 ms exciter while the modal body rings out.
    assert!(vs.is_active(), "voice must keep ringing during modal tail");

    // 2. Nonlinear z-1 feedback Plate cymbal (`Modal -> Drive(AtomicJet) -> Vca(Env) -> Modal.In`).
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
                ModuleKind::Modal(ModalModule {
                    geometry: BodyGeometry::Plate,
                    atoms: 24,
                    pitch_tracking: PitchTracking::Fixed,
                    fixed_hz: 280.0,
                    pair_topology: PairTopology::Swarm,
                    coupling: 0.35,
                    absorption: 0.015,
                    radiation: 0.25,
                    brightness: 0.9,
                    triad_amount: 0.4,
                    triad_axis: TriadAxis::Manley,
                    spread: 0.75,
                    strike_drive: 1.8,
                    strike_hardness: 0.85,
                    mix: 1.0,
                    ..ModalModule::default()
                }),
            ),
            RackModule::new(
                5,
                "Plate NL",
                ModuleKind::Drive(DriveModule {
                    mode: DriveMode::AtomicJet,
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
            peak = peak.max(f[0].abs()).max(f[1].abs());
        }
    });
    assert!(
        peak > 0.03,
        "nonlinear plate crash must produce audible signal (peak={peak})"
    );
}

#[test]
fn voice_retirement_ignores_upstream_stateful_nodes_and_preserves_post_vca_tails() {
    let bank: SampleBank = Vec::new();

    // 1. Subtractive voice with Filter + Modal + Drive upstream of an envelope-gated VCA:
    //    VCO keeps driving upstream modules forever, yet voice must retire promptly after Env ends.
    let upstream_zone = build_zone(
        vec![
            RackModule::new(1, "Osc", ModuleKind::Vco(VcoModule::default())),
            RackModule::new(
                2,
                "LPF",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::ImLpf2,
                    cutoff_hz: 2400.0,
                    resonance: 0.4,
                    drive: 0.25,
                    ..FilterModule::default()
                }),
            ),
            RackModule::new(
                3,
                "Modal",
                ModuleKind::Modal(ModalModule {
                    geometry: BodyGeometry::Bar,
                    atoms: 16,
                    radiation: 0.1,
                    strike_drive: 0.0,
                    mix: 0.35,
                    ..ModalModule::default()
                }),
            ),
            RackModule::new(
                4,
                "Drv",
                ModuleKind::Drive(DriveModule {
                    mode: DriveMode::AtomicJet,
                    drive: 0.45,
                    asymmetry: 0.15,
                    ..DriveModule::default()
                }),
            ),
            RackModule::new(5, "AmpVca", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                6,
                "AmpEnv",
                ModuleKind::Env(EnvModule {
                    attack_ms: 1.0,
                    decay_ms: 20.0,
                    sustain: 0.8,
                    release_ms: 25.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(7, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 2, 3, InPort::In, 1.0),
            PatchCable::new(3, 3, 4, InPort::In, 1.0),
            PatchCable::new(4, 4, 5, InPort::In, 1.0),
            PatchCable::new(5, 6, 5, InPort::Cv, 1.0),
            PatchCable::new(6, 5, 7, InPort::In, 1.0),
        ],
    );
    let g_up = compile_zone_graph(&upstream_zone).unwrap();
    assert_eq!(g_up.env_gated_vcas.len(), 1);
    assert_eq!(
        g_up.post_gate_tail_slots.as_ref(),
        &[g_up.out_slot],
        "only Out sits downstream of the closed AmpVca"
    );

    for sr in [44_100.0_f64, 48_000.0, 96_000.0] {
        let gate_frames = (sr * 0.03) as u32;
        let mut voice = ModularVoice::new_boxed(
            &spec(&upstream_zone, &g_up, sr, 136.0),
            start(69, Some(gate_frames)),
        );
        // Gate (30 ms) + release (25 ms to -60 dB => ~55 ms to -120 dB) + Out DcBlocker settle.
        // Must retire well under 250 ms — never waiting for the 2 s / 18 s timeout!
        let max_frames = (sr * 0.25) as usize;
        let mut active_frames = 0usize;
        let mut peak = 0.0_f32;
        for _ in 0..max_frames {
            if !voice.is_active() {
                break;
            }
            let f = voice.next_frame(&bank, None);
            peak = peak.max(f[0].abs()).max(f[1].abs());
            active_frames += 1;
        }
        assert!(peak > 0.1, "voice must produce audible audio at sr={sr}");
        assert!(
            !voice.is_active(),
            "upstream stateful modules behind closed VCA must not trap voice alive at sr={sr} (still active after {active_frames} frames)"
        );
    }

    // 2. Post-VCA Modal tail MUST ring out naturally long after the short strike envelope ends.
    let sr = 48_000.0;
    let post_modal_zone = build_zone(
        vec![
            RackModule::new(1, "Strike", ModuleKind::Vco(VcoModule::default())),
            RackModule::new(2, "StrikeVca", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                3,
                "StrikeEnv",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::HoldDecay,
                    attack_ms: 0.5,
                    hold_ms: 0.0,
                    decay_ms: 25.0,
                    sustain: 0.0,
                    release_ms: 10.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                4,
                "Body",
                ModuleKind::Modal(ModalModule {
                    geometry: BodyGeometry::Harmonic,
                    atoms: 16,
                    pair_topology: PairTopology::Swarm,
                    coupling: 0.0,
                    absorption: 0.08,
                    radiation: 0.16,
                    brightness: 0.75,
                    headroom: 0.0,
                    strike_drive: 0.0,
                    mix: 1.0,
                    ..ModalModule::default()
                }),
            ),
            RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 3, 2, InPort::Cv, 1.0),
            PatchCable::new(3, 2, 4, InPort::In, 1.0),
            PatchCable::new(4, 4, 5, InPort::In, 1.0),
        ],
    );
    let g_post = compile_zone_graph(&post_modal_zone).unwrap();
    let plate_slot = g_post.slot_for_module(4).unwrap() as u8;
    assert!(
        g_post.post_gate_tail_slots.contains(&plate_slot),
        "downstream Plate modal must remain in post_gate_tail_slots"
    );
    let mut ring_voice = ModularVoice::new_boxed(
        &spec(&post_modal_zone, &g_post, sr, 120.0),
        start(60, Some(480)),
    );
    let mut late_peak = 0.0_f32;
    for i in 0..(sr * 0.45) as usize {
        let f = ring_voice.next_frame(&bank, None);
        if i >= (sr * 0.25) as usize {
            late_peak = late_peak.max(f[0].abs()).max(f[1].abs());
        }
    }
    assert!(
        ring_voice.is_active() && late_peak > 1e-3,
        "post-VCA plate modal must continue ringing past 450 ms (active={}, late_peak={late_peak})",
        ring_voice.is_active()
    );

    // 3. Env-gated VCA with non-zero DC bias bleeds audible audio and must NOT retire prematurely.
    let mut biased_zone = upstream_zone.clone();
    if let ModuleKind::Vca(v) = &mut biased_zone.modules[4].kind {
        v.bias = 0.25;
    }
    let g_bias = compile_zone_graph(&biased_zone).unwrap();
    let mut biased_voice = ModularVoice::new_boxed(
        &spec(&biased_zone, &g_bias, sr, 120.0),
        start(69, Some(1440)),
    );
    let mut bleed_peak = 0.0_f32;
    for i in 0..(sr * 0.4) as usize {
        let f = biased_voice.next_frame(&bank, None);
        if i >= (sr * 0.3) as usize {
            bleed_peak = bleed_peak.max(f[0].abs()).max(f[1].abs());
        }
    }
    assert!(
        biased_voice.is_active() && bleed_peak > 1e-2,
        "non-zero VCA bias bleed must not be prematurely cut off (active={}, bleed_peak={bleed_peak})",
        biased_voice.is_active()
    );
}

#[test]
fn sampler_voice_reuse_and_non_legato_retarget_preserve_pitch_and_sample_rate_ratio() {
    let engine_sr = 48_000.0_f64;
    let sample_sr = 44_100_u32;
    let base_hz = 440.0_f64;
    // 44.1 kHz sample played on a 48 kHz engine one octave above root (midi 72 vs root 60):
    // expected played frequency is 880 Hz (step_ratio = (44100/48000) * 2.0 != 1.0).
    let sample = synthetic_sample(sample_sr as usize, sample_sr, base_hz);
    let zone = build_zone(
        vec![
            RackModule::new(
                1,
                "Smp",
                ModuleKind::Sampler(SamplerModule {
                    root_midi: 60,
                    one_shot: false,
                    ..SamplerModule::default()
                }),
            ),
            RackModule::new(2, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![PatchCable::new(1, 1, 2, InPort::In, 1.0)],
    );
    let graph = compile_zone_graph(&zone).unwrap();
    let sample_key = module_sample_key(1, zone.id, 1);
    let bank: SampleBank = vec![(sample_key, sample)];

    let mut voice = ModularVoice::new_boxed(
        &spec(&zone, &graph, engine_sr, 120.0),
        start(72, Some(2400)),
    );
    let mut first_buf = Vec::with_capacity(2048);
    for _ in 0..2048 {
        first_buf.push(f64::from(voice.next_frame(&bank, None)[0]));
    }
    let expected_hz = base_hz * 2.0; // 880 Hz
    let wrong_unity_hz = base_hz * (engine_sr / f64::from(sample_sr)); // ~478.9 Hz if step_ratio==1.0
    let p1_expected = dft_power(&first_buf, expected_hz, engine_sr);
    let p1_wrong = dft_power(&first_buf, wrong_unity_hz, engine_sr);
    assert!(
        p1_expected > p1_wrong * 50.0,
        "first trigger should play at {expected_hz} Hz (p_exp={p1_expected}, p_wrong={p1_wrong})"
    );

    // Non-legato retarget at the exact same MIDI note & sample rate must NOT fall back to ratio 1.0.
    voice.retarget_from_zone(
        &spec(&zone, &graph, engine_sr, 120.0),
        start(72, Some(2400)),
        false,
    );
    let mut second_buf = Vec::with_capacity(2048);
    for _ in 0..2048 {
        second_buf.push(f64::from(voice.next_frame(&bank, None)[0]));
    }
    let p2_expected = dft_power(&second_buf, expected_hz, engine_sr);
    let p2_wrong = dft_power(&second_buf, wrong_unity_hz, engine_sr);
    assert!(
        p2_expected > p2_wrong * 50.0,
        "reused non-legato trigger must keep {expected_hz} Hz ratio (p_exp={p2_expected}, p_wrong={p2_wrong})"
    );
}

#[test]
fn voice_retirement_preserves_sampler_env_predelay_lfo_chopped_quiet_and_legato_modal_tails() {
    let sr = 48_000.0_f64;
    let empty_bank: SampleBank = Vec::new();

    // 1. Sampler + upstream Modal + pre-delayed Env: short sample finishes while Env is still
    //    in pre-delay; voice must stay alive so the delayed Env opens and plays the Modal tail.
    let short_sample = synthetic_sample(480, 48_000, 440.0); // 10 ms impulse
    let z_predelay = build_zone(
        vec![
            RackModule::new(
                1,
                "Smp",
                ModuleKind::Sampler(SamplerModule {
                    one_shot: true,
                    ..SamplerModule::default()
                }),
            ),
            RackModule::new(
                2,
                "Modal",
                ModuleKind::Modal(ModalModule {
                    geometry: BodyGeometry::Plate,
                    atoms: 24,
                    pair_topology: PairTopology::Swarm,
                    coupling: 0.0,
                    absorption: 0.25,
                    radiation: 0.25,
                    headroom: 0.0,
                    strike_drive: 0.0,
                    mix: 1.0,
                    ..ModalModule::default()
                }),
            ),
            RackModule::new(3, "Vca", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                4,
                "DelayEnv",
                ModuleKind::Env(EnvModule {
                    delay_ms: 40.0,
                    attack_ms: 2.0,
                    decay_ms: 150.0,
                    sustain: 0.0,
                    release_ms: 50.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 2, 3, InPort::In, 1.0),
            PatchCable::new(3, 4, 3, InPort::Cv, 1.0),
            PatchCable::new(4, 3, 5, InPort::In, 1.0),
        ],
    );
    let g_pre = compile_zone_graph(&z_predelay).unwrap();
    let smp_bank: SampleBank = vec![(module_sample_key(1, z_predelay.id, 1), short_sample)];
    let mut v_pre =
        ModularVoice::new_boxed(&spec(&z_predelay, &g_pre, sr, 120.0), start(60, Some(9600)));
    let mut post_delay_peak = 0.0_f32;
    for i in 0..(sr * 0.12) as usize {
        let f = v_pre.next_frame(&smp_bank, None);
        if i >= (sr * 0.05) as usize {
            post_delay_peak = post_delay_peak.max(f[0].abs()).max(f[1].abs());
        }
    }
    assert!(
        post_delay_peak > 1e-3,
        "pre-delayed Env on sampler_only zone must open and pass upstream Modal tail (peak={post_delay_peak})"
    );

    // 2. LFO-chopped post-VCA Plate modal: voice must survive >20 ms LFO-off silent gaps.
    let z_lfo_chop = build_zone(
        vec![
            RackModule::new(1, "Exc", ModuleKind::Vco(VcoModule::default())),
            RackModule::new(2, "StrikeVca", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                3,
                "StrikeEnv",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::HoldDecay,
                    attack_ms: 0.5,
                    decay_ms: 10.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                4,
                "Plate",
                ModuleKind::Modal(ModalModule {
                    geometry: BodyGeometry::Plate,
                    atoms: 24,
                    pair_topology: PairTopology::Swarm,
                    coupling: 0.0,
                    absorption: 0.02,
                    radiation: 0.18,
                    brightness: 0.75,
                    headroom: 0.5,
                    strike_drive: 1.0,
                    mix: 1.0,
                    ..ModalModule::default()
                }),
            ),
            RackModule::new(5, "GateVca", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                6,
                "ChopLfo",
                ModuleKind::Lfo(LfoModule {
                    shape: LfoShape::Square,
                    rate_hz: 10.0, // 50 ms high, 50 ms exact zero
                    sync: false,
                    retrigger: true,
                    bipolar: false,
                    ..LfoModule::default()
                }),
            ),
            RackModule::new(7, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 3, 2, InPort::Cv, 1.0),
            PatchCable::new(3, 2, 4, InPort::In, 1.0),
            PatchCable::new(4, 4, 5, InPort::In, 1.0),
            PatchCable::new(5, 6, 5, InPort::Cv, 1.0),
            PatchCable::new(6, 5, 7, InPort::In, 1.0),
        ],
    );
    let g_chop = compile_zone_graph(&z_lfo_chop).unwrap();
    let mut v_chop =
        ModularVoice::new_boxed(&spec(&z_lfo_chop, &g_chop, sr, 120.0), start(60, Some(480)));
    let mut second_pulse_peak = 0.0_f32;
    for i in 0..(sr * 0.25) as usize {
        let f = v_chop.next_frame(&empty_bank, None);
        if i >= (sr * 0.12) as usize {
            second_pulse_peak = second_pulse_peak.max(f[0].abs()).max(f[1].abs());
        }
    }
    assert!(
        v_chop.is_active() && second_pulse_peak > 1e-3,
        "LFO-chopped post-VCA modal must survive 50 ms LFO-off gaps (active={}, peak={second_pulse_peak})",
        v_chop.is_active()
    );

    // 3. Quiet post-VCA modal (Out.gain_db = -36 dB, velocity = 0.05) must not cut off while ringing.
    let z_quiet = build_zone(
        vec![
            RackModule::new(1, "Noise", ModuleKind::Noise(NoiseModule::default())),
            RackModule::new(2, "StrikeVca", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                3,
                "StrikeEnv",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::HoldDecay,
                    attack_ms: 0.5,
                    decay_ms: 10.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                4,
                "Plate",
                ModuleKind::Modal(ModalModule {
                    geometry: BodyGeometry::Plate,
                    atoms: 24,
                    pair_topology: PairTopology::Swarm,
                    coupling: 0.0,
                    absorption: 0.25,
                    radiation: 0.2,
                    headroom: 0.0,
                    strike_drive: 0.0,
                    mix: 1.0,
                    ..ModalModule::default()
                }),
            ),
            RackModule::new(
                5,
                "Out",
                ModuleKind::Out(OutModule {
                    gain_db: -36.0,
                    ..OutModule::default()
                }),
            ),
        ],
        vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 3, 2, InPort::Cv, 1.0),
            PatchCable::new(3, 2, 4, InPort::In, 1.0),
            PatchCable::new(4, 4, 5, InPort::In, 1.0),
        ],
    );
    let g_quiet = compile_zone_graph(&z_quiet).unwrap();
    let mut low_start = start(60, Some(480));
    low_start.velocity = 0.05;
    let mut v_quiet = ModularVoice::new_boxed(&spec(&z_quiet, &g_quiet, sr, 120.0), low_start);
    for _ in 0..(sr * 0.2) as usize {
        let _ = v_quiet.next_frame(&empty_bank, None);
    }
    assert!(
        v_quiet.is_active(),
        "quiet post-VCA modal must remain active while modal bank is still ringing"
    );

    // 4. Legato retarget lowering post-VCA modal radiation from 0.95 to 0.12 preserves long ringout past 2.3 s.
    let mut z_legato = z_quiet.clone();
    if let ModuleKind::Modal(m) = &mut z_legato.modules[3].kind {
        m.radiation = 0.95;
    }
    if let ModuleKind::Out(o) = &mut z_legato.modules[4].kind {
        o.gain_db = 0.0;
    }
    let g_leg = compile_zone_graph(&z_legato).unwrap();
    let mut v_leg =
        ModularVoice::new_boxed(&spec(&z_legato, &g_leg, sr, 120.0), start(60, Some(480)));
    for _ in 0..128 {
        let _ = v_leg.next_frame(&empty_bank, None);
    }
    let mut z_legato_long = z_legato.clone();
    if let ModuleKind::Modal(m) = &mut z_legato_long.modules[3].kind {
        m.radiation = 0.12;
    }
    let g_leg_long = compile_zone_graph(&z_legato_long).unwrap();
    v_leg.retarget_from_zone(
        &spec(&z_legato_long, &g_leg_long, sr, 120.0),
        start(64, Some(480)),
        true,
    );
    for _ in 0..(sr * 2.3) as usize {
        let _ = v_leg.next_frame(&empty_bank, None);
    }
    assert!(
        v_leg.is_active(),
        "legato retarget lowering post-VCA modal radiation must outlive 2.0 s default tail cap"
    );

    // 5. DriveNode and NoiseNode legato retarget across a sample-rate change refreshes filter coeffs.
    let z_sr = build_zone(
        vec![
            RackModule::new(
                1,
                "Noise",
                ModuleKind::Noise(NoiseModule {
                    highpass_hz: 1_500.0,
                    lowpass_hz: 4_000.0,
                    ..NoiseModule::default()
                }),
            ),
            RackModule::new(
                2,
                "Drive",
                ModuleKind::Drive(DriveModule {
                    drive: 0.4,
                    tone_hz: 3_000.0,
                    mix: 1.0,
                    ..DriveModule::default()
                }),
            ),
            RackModule::new(3, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 2, 3, InPort::In, 1.0),
        ],
    );
    let g_sr = compile_zone_graph(&z_sr).unwrap();
    let mut v_sr = ModularVoice::new_boxed(&spec(&z_sr, &g_sr, 48_000.0, 120.0), start(60, None));
    for _ in 0..64 {
        let _ = v_sr.next_frame(&empty_bank, None);
    }
    v_sr.retarget_from_zone(&spec(&z_sr, &g_sr, 96_000.0, 120.0), start(62, None), true);
    // Run enough frames to pass control_tick; NoiseNode & DriveNode cached keys must refresh for 96 kHz.
    let mut energy = 0.0_f64;
    for _ in 0..256 {
        let f = v_sr.next_frame(&empty_bank, None);
        assert!(f[0].is_finite() && f[1].is_finite());
        energy += f64::from(f[0] * f[0]);
    }
    assert!(energy > 0.0);
    // Verify NoiseNode and DriveNode filter caches refreshed for 96 kHz.
    let noise_slot = g_sr.slot_for_module(1).unwrap();
    let drive_slot = g_sr.slot_for_module(2).unwrap();
    if let ModuleNode::Noise(ref n) = v_sr.nodes[noise_slot] {
        assert_eq!(
            n.cached_filter_sr_bits(),
            (96_000.0_f64.to_bits(), 96_000.0_f64.to_bits())
        );
    } else {
        panic!("expected NoiseNode at slot {noise_slot}");
    }
    if let ModuleNode::Drive(ref d) = v_sr.nodes[drive_slot] {
        assert_eq!(d.cached_tone_sr_bits(), 96_000.0_f64.to_bits());
    } else {
        panic!("expected DriveNode at slot {drive_slot}");
    }
}

#[test]
fn drive_node_parallel_mix_stays_phase_aligned_across_sample_rates() {
    let bank: SampleBank = Vec::new();
    // High-frequency sine VCO (MIDI 119 ~= 7902 Hz) through DriveNode(drive=0.0, mix=0.5,
    // tone_hz=18000 Hz) must stay flat without a comb notch.
    let dry_zone = build_zone(
        vec![
            RackModule::new(
                1,
                "Sine",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(2, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![PatchCable::new(1, 1, 2, InPort::In, 1.0)],
    );
    let par_zone = build_zone(
        vec![
            RackModule::new(
                1,
                "Sine",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "ParDrive",
                ModuleKind::Drive(DriveModule {
                    mode: DriveMode::AtomicJet,
                    drive: 0.0,
                    asymmetry: 0.0,
                    tone_hz: 18_000.0,
                    output_db: 0.0,
                    mix: 0.5,
                }),
            ),
            RackModule::new(3, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 2, 3, InPort::In, 1.0),
        ],
    );
    let g_dry = compile_zone_graph(&dry_zone).unwrap();
    let g_par = compile_zone_graph(&par_zone).unwrap();

    for sr in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
        let mut v_dry =
            ModularVoice::new_boxed(&spec(&dry_zone, &g_dry, sr, 120.0), start(119, None));
        let mut v_par =
            ModularVoice::new_boxed(&spec(&par_zone, &g_par, sr, 120.0), start(119, None));
        let warmup = (sr * 0.02) as usize;
        let measure = (sr * 0.02) as usize;
        let mut dry_sq = 0.0_f64;
        let mut par_sq = 0.0_f64;
        for i in 0..(warmup + measure) {
            let yd = v_dry.next_frame(&bank, None);
            let yp = v_par.next_frame(&bank, None);
            if i >= warmup {
                dry_sq += f64::from(yd[0]).powi(2);
                par_sq += f64::from(yp[0]).powi(2);
            }
        }
        let diff_db = cadence_core::dsp::lin_to_db_f64((par_sq / dry_sq).sqrt());
        assert!(
            diff_db > -1.5 && diff_db < 0.25,
            "sr={sr}: parallel DriveNode combed 7.9 kHz sine to {diff_db:+.2} dB"
        );
    }
}

#[test]
fn control_stride_and_filter_cv_subrate_scale_across_sample_rates() {
    let bank: SampleBank = Vec::new();
    let zone = build_zone(
        vec![
            RackModule::new(1, "VCO", ModuleKind::Vco(VcoModule::default())),
            RackModule::new(
                2,
                "LPF",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::ImLpf2,
                    cutoff_hz: 1_200.0,
                    resonance: 0.35,
                    mix: 0.75,
                    ..FilterModule::default()
                }),
            ),
            RackModule::new(
                3,
                "ModLfo",
                ModuleKind::Lfo(LfoModule {
                    shape: LfoShape::Sine,
                    rate_hz: 6.0,
                    retrigger: true,
                    bipolar: true,
                    ..LfoModule::default()
                }),
            ),
            RackModule::new(4, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 3, 2, InPort::Cv, 1.5),
            PatchCable::new(3, 2, 4, InPort::In, 1.0),
        ],
    );
    let graph = compile_zone_graph(&zone).unwrap();
    let filter_slot = graph.slot_for_module(2).unwrap();

    let expected_cases = [
        (8_000.0_f64, 1_u32, 0_u32),
        (44_100.0, 15, 3),
        (48_000.0, 15, 3),
        (96_000.0, 31, 7),
        (192_000.0, 63, 15),
    ];

    for (sr, exp_ctrl_mask, exp_flt_mask) in expected_cases {
        let inactive = ModularVoice::new_boxed_inactive(sr);
        assert_eq!(
            inactive.control_mask, exp_ctrl_mask,
            "sr={sr}: unexpected inactive control_mask"
        );

        let mut voice = ModularVoice::new_boxed(&spec(&zone, &graph, sr, 120.0), start(60, None));
        assert_eq!(
            voice.control_mask, exp_ctrl_mask,
            "sr={sr}: unexpected triggered control_mask"
        );
        match voice.nodes[filter_slot] {
            ModuleNode::Filter(ref flt) => assert_eq!(
                flt.cv_subrate_mask(),
                exp_flt_mask,
                "sr={sr}: unexpected FilterNode cv_subrate_mask"
            ),
            _ => panic!("expected FilterNode at slot {filter_slot}"),
        }

        // Stepping one full control stride wraps control_counter back to 0 cleanly.
        let stride = (exp_ctrl_mask + 1) as usize;
        for step in 0..(stride * 2) {
            assert_eq!(
                voice.control_counter,
                (step as u32) & exp_ctrl_mask,
                "sr={sr} step={step}: control_counter cadence mismatch"
            );
            let f = voice.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
        }
    }

    // Retargeting across sample-rate switches refreshes both voice and filter masks with zero allocs.
    let mut voice = ModularVoice::new_boxed(&spec(&zone, &graph, 48_000.0, 120.0), start(60, None));
    assert_no_heap_operations(|| {
        for (sr, exp_ctrl_mask, exp_flt_mask) in [
            (96_000.0, 31_u32, 7_u32),
            (192_000.0, 63, 15),
            (44_100.0, 15, 3),
            (48_000.0, 15, 3),
        ] {
            voice.retarget_from_zone(&spec(&zone, &graph, sr, 120.0), start(64, None), true);
            assert_eq!(voice.control_mask, exp_ctrl_mask);
            if let ModuleNode::Filter(ref flt) = voice.nodes[filter_slot] {
                assert_eq!(flt.cv_subrate_mask(), exp_flt_mask);
            } else {
                panic!("expected FilterNode");
            }
            for _ in 0..128 {
                let f = voice.next_frame(&bank, None);
                assert!(f[0].is_finite() && f[1].is_finite());
            }
        }
    });
}

#[test]
fn modal_high_pitch_cv_clamps_to_max_hz_without_recompute_thrash() {
    use cadence_core::dsp::cubic::MODAL_MAX_HZ;

    let bank: SampleBank = Vec::new();
    let make_zone = |pitch_tracking: PitchTracking, fixed_hz: f32| {
        build_zone(
            vec![
                RackModule::new(1, "Noise", ModuleKind::Noise(NoiseModule::default())),
                RackModule::new(
                    2,
                    "PitchLfo",
                    ModuleKind::Lfo(LfoModule {
                        shape: LfoShape::Sine,
                        rate_hz: 12.0,
                        retrigger: true,
                        bipolar: false,
                        ..LfoModule::default()
                    }),
                ),
                RackModule::new(
                    3,
                    "Modal",
                    ModuleKind::Modal(ModalModule {
                        geometry: BodyGeometry::Harmonic,
                        atoms: 16,
                        pitch_tracking,
                        fixed_hz,
                        radiation: 0.4,
                        brightness: 0.5,
                        strike_drive: 0.0,
                        mix: 1.0,
                        ..ModalModule::default()
                    }),
                ),
                RackModule::new(4, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![
                PatchCable::new(1, 1, 3, InPort::In, 1.0),
                PatchCable::new(2, 2, 3, InPort::PitchCv, 48.0), // sweeps +0..+48 st above base!
                PatchCable::new(3, 3, 4, InPort::In, 1.0),
            ],
        )
    };

    let chrom_zone = make_zone(PitchTracking::Chromatic, 440.0);
    let fixed_zone = make_zone(PitchTracking::Fixed, MODAL_MAX_HZ);
    let g_chrom = compile_zone_graph(&chrom_zone).unwrap();
    let g_fixed = compile_zone_graph(&fixed_zone).unwrap();
    let modal_slot_c = g_chrom.slot_for_module(3).unwrap();
    let modal_slot_f = g_fixed.slot_for_module(3).unwrap();

    for sr in [8_000.0_f64, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
        let expected_ceiling = (sr * 0.36).min(f64::from(MODAL_MAX_HZ));

        // 1. Chromatic MIDI 127 (~12.54 kHz) + sweeping +0..+48 st PitchCv stays pinned at ceiling.
        let mut vc =
            ModularVoice::new_boxed(&spec(&chrom_zone, &g_chrom, sr, 120.0), start(127, None));
        assert_no_heap_operations(|| {
            for _ in 0..512 {
                let f = vc.next_frame(&bank, None);
                assert!(f[0].is_finite() && f[1].is_finite());
            }
        });
        match vc.nodes[modal_slot_c] {
            ModuleNode::Modal(ref modal) => {
                assert!(
                    (modal.applied_f0_hz() - expected_ceiling).abs() < 1e-6,
                    "sr={sr}: chromatic applied_f0_hz={} != {expected_ceiling}",
                    modal.applied_f0_hz()
                );
                assert_eq!(
                    modal.configure_count(),
                    1,
                    "sr={sr}: high PitchCv above {expected_ceiling} Hz must not thrash modal configure()"
                );
                assert_eq!(
                    modal.glide_update_count(),
                    0,
                    "sr={sr}: clamped high PitchCv must not thrash modal glide updates"
                );
            }
            _ => panic!("expected ModalNode"),
        }

        // 2. Fixed 12 kHz + sweeping +0..+48 st PitchCv stays pinned at ceiling.
        let mut vf =
            ModularVoice::new_boxed(&spec(&fixed_zone, &g_fixed, sr, 120.0), start(60, None));
        assert_no_heap_operations(|| {
            for _ in 0..512 {
                let f = vf.next_frame(&bank, None);
                assert!(f[0].is_finite() && f[1].is_finite());
            }
        });
        match vf.nodes[modal_slot_f] {
            ModuleNode::Modal(ref modal) => {
                assert!(
                    (modal.applied_f0_hz() - expected_ceiling).abs() < 1e-6,
                    "sr={sr}: fixed applied_f0_hz={} != {expected_ceiling}",
                    modal.applied_f0_hz()
                );
                assert_eq!(
                    modal.configure_count(),
                    1,
                    "sr={sr}: fixed high PitchCv above {expected_ceiling} Hz must not thrash modal configure()"
                );
                assert_eq!(
                    modal.glide_update_count(),
                    0,
                    "sr={sr}: fixed clamped high PitchCv must not thrash modal glide updates"
                );
            }
            _ => panic!("expected ModalNode"),
        }
    }
}

#[test]
fn pooled_modal_voice_reuse_and_radiation_modulation_preserve_tail_and_cap_feedback_zero_alloc() {
    let bank: SampleBank = Vec::new();
    let make_modal_zone = |radiation: f32, brightness: f32| {
        build_zone(
            vec![
                RackModule::new(1, "Strike", ModuleKind::Vco(VcoModule::default())),
                RackModule::new(2, "StrikeVca", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(
                    3,
                    "StrikeEnv",
                    ModuleKind::Env(EnvModule {
                        mode: EnvMode::HoldDecay,
                        attack_ms: 0.5,
                        hold_ms: 0.0,
                        decay_ms: 10.0,
                        sustain: 0.0,
                        release_ms: 10.0,
                        trigger_mode: TriggerMode::OneShot,
                        ..EnvModule::default()
                    }),
                ),
                RackModule::new(
                    4,
                    "Body",
                    ModuleKind::Modal(ModalModule {
                        geometry: BodyGeometry::Harmonic,
                        atoms: 16,
                        pair_topology: PairTopology::Swarm,
                        coupling: 0.0,
                        absorption: 0.06,
                        radiation,
                        brightness,
                        headroom: 0.0,
                        strike_drive: 0.0,
                        mix: 1.0,
                        ..ModalModule::default()
                    }),
                ),
                RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 3, 2, InPort::Cv, 1.0),
                PatchCable::new(3, 2, 4, InPort::In, 1.0),
                PatchCable::new(4, 4, 5, InPort::In, 1.0),
            ],
        )
    };

    let modal_zone = make_modal_zone(0.12, 0.85);
    let synth_zone = RackPatch::default_synth().zones.remove(0);
    let g_modal = compile_zone_graph(&modal_zone).unwrap();
    let g_synth = compile_zone_graph(&synth_zone).unwrap();

    for sr in [44_100.0_f64, 48_000.0, 96_000.0, 192_000.0] {
        let mut voice = ModularVoice::new_boxed_inactive(sr);
        let default_tail = (sr * DEFAULT_TAIL_TIMEOUT_SEC) as u32;
        let max_tail = (sr * MAX_MODAL_TAIL_SEC) as u32;

        assert_no_heap_operations(|| {
            // 1. Trigger post-VCA Plate modal: tail_frames must cover MAX_MODAL_TAIL_SEC.
            voice.begin_trigger_from_zone(&spec(&modal_zone, &g_modal, sr, 120.0));
            voice.finalize_trigger(start(60, Some(480)), false, false);
            assert_eq!(
                voice.tail_frames, max_tail,
                "sr={sr}: post-VCA modal expected MAX_MODAL_TAIL_SEC tail budget"
            );
            for _ in 0..256 {
                let f = voice.next_frame(&bank, None);
                assert!(f[0].is_finite() && f[1].is_finite());
            }

            // 2. Reuse inactive pooled slot for non-modal synth: must NOT inherit MAX_MODAL_TAIL_SEC!
            voice.deactivate();
            voice.begin_trigger_from_zone(&spec(&synth_zone, &g_synth, sr, 120.0));
            voice.finalize_trigger(start(60, Some(480)), false, false);
            assert_eq!(
                voice.tail_frames, default_tail,
                "sr={sr}: reused pooled non-modal slot inherited stale modal tail_frames"
            );
        });
    }

    // 3. Lowering Radiation post-tail preserves long ringout past 2.3 s (> 2.0 s default cap) with zero allocs.
    let sr = 48_000.0_f64;
    let short_zone = make_modal_zone(0.55, 0.85);
    let long_zone = make_modal_zone(0.06, 0.85);
    let graph = compile_zone_graph(&short_zone).unwrap();
    let mut voice_after_tail =
        ModularVoice::new_boxed(&spec(&short_zone, &graph, sr, 120.0), start(60, Some(480)));
    let mut late_peak = 0.0_f32;
    assert_no_heap_operations(|| {
        // Run 25 ms so StrikeEnv (10 ms) is already Done and tail countdown has begun.
        for _ in 0..(sr * 0.025) as usize {
            let f = voice_after_tail.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
        }
        assert!(
            voice_after_tail.tail_elapsed_frames > 0,
            "expected voice to have entered tail countdown before modulation"
        );
        voice_after_tail.update_continuous_from_zone(&long_zone, 0.0, 0.0, 120.0);

        let total = (sr * 2.3) as usize;
        let window_start = (sr * 2.1) as usize;
        for i in 0..total {
            let f = voice_after_tail.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
            if i >= window_start {
                late_peak = late_peak.max(f[0].abs()).max(f[1].abs());
            }
        }
    });
    assert!(
        voice_after_tail.is_active() && late_peak > 1e-4,
        "post-tail downward Radiation modulation must outlive 2.0 s cap (active={}, late_peak={late_peak})",
        voice_after_tail.is_active()
    );

    // 4. Even if continuous modulation repeatedly wiggles Radiation on an ungated self-feedback loop,
    //    tail_elapsed_frames caps total tail duration cleanly at MAX_MODAL_TAIL_SEC.
    let stuck_sr = 8_000.0_f64;
    let stuck_zone = build_zone(
        vec![
            RackModule::new(1, "Strike", ModuleKind::Noise(NoiseModule::default())),
            RackModule::new(2, "StrikeVca", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                3,
                "StrikeEnv",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::HoldDecay,
                    attack_ms: 0.5,
                    hold_ms: 0.0,
                    decay_ms: 10.0,
                    sustain: 0.0,
                    release_ms: 10.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                4,
                "Plate",
                ModuleKind::Modal(ModalModule {
                    geometry: BodyGeometry::Plate,
                    atoms: 16,
                    radiation: 0.05,
                    brightness: 1.0,
                    strike_drive: 1.0,
                    mix: 1.0,
                    ..ModalModule::default()
                }),
            ),
            RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 3, 2, InPort::Cv, 1.0),
            PatchCable::new(3, 2, 4, InPort::In, 1.0),
            PatchCable::new(4, 4, 4, InPort::In, 2.0), // Ungated self-oscillating feedback!
            PatchCable::new(5, 4, 5, InPort::In, 1.0),
        ],
    );
    let g_stuck = compile_zone_graph(&stuck_zone).unwrap();
    let mut v_stuck = ModularVoice::new_boxed(
        &spec(&stuck_zone, &g_stuck, stuck_sr, 120.0),
        start(60, Some(80)),
    );
    let norm_a = ModuleParam::Radiation.normalize(0.05);
    let norm_b = ModuleParam::Radiation.normalize(0.08);
    let max_frames = (stuck_sr * (MAX_MODAL_TAIL_SEC + 0.5)) as usize;
    for i in 0..max_frames {
        if !v_stuck.is_active() {
            break;
        }
        if i % 256 == 0 {
            let norm = if (i / 256) % 2 == 0 { norm_a } else { norm_b };
            v_stuck.apply_continuous_module_param(4, ModuleParam::Radiation, norm);
        }
        let f = v_stuck.next_frame(&bank, None);
        assert!(f[0].is_finite() && f[1].is_finite());
    }
    assert!(
        !v_stuck.is_active(),
        "ungated feedback modal with continuous radiation modulation must terminate by MAX_MODAL_TAIL_SEC"
    );
}

#[test]
fn out_gain_boost_passes_unclipped_headroom_into_float_bus() {
    let sr = 48_000.0_f64;
    let bank: SampleBank = Vec::new();
    let make_zone = |gain_db: f32, pan: f32| {
        build_zone(
            vec![
                RackModule::new(
                    1,
                    "Sine",
                    ModuleKind::Vco(VcoModule {
                        waveform: Waveform::Sine,
                        pitch_tracking: PitchTracking::Fixed,
                        fixed_hz: 440.0,
                        level: 1.0,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(
                    2,
                    "Out",
                    ModuleKind::Out(OutModule {
                        gain_db,
                        pan,
                        spread: 0.0,
                    }),
                ),
            ],
            vec![PatchCable::new(1, 1, 2, InPort::In, 1.0)],
        )
    };

    let full_vel = VoiceStart {
        velocity: 1.0,
        ..start(69, None)
    };
    let warmup = 4_800usize;
    let measure = 4_800usize;

    let run_case = |gain_db: f32, pan: f32| -> (f64, f64, f64, Vec<f64>) {
        let zone = make_zone(gain_db, pan);
        let graph = compile_zone_graph(&zone).unwrap();
        let mut voice = ModularVoice::new_boxed(&spec(&zone, &graph, sr, 120.0), full_vel);
        let mut peak_l = 0.0_f64;
        let mut peak_r = 0.0_f64;
        let mut sq_l = 0.0_f64;
        let mut buf_l = Vec::with_capacity(measure);
        for i in 0..(warmup + measure) {
            let f = voice.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
            if i >= warmup {
                let l = f64::from(f[0]);
                let r = f64::from(f[1]);
                peak_l = peak_l.max(l.abs());
                peak_r = peak_r.max(r.abs());
                sq_l += l * l;
                buf_l.push(l);
            }
        }
        (peak_l, peak_r, (sq_l / measure as f64).sqrt(), buf_l)
    };

    let (_, _, rms_0, _) = run_case(0.0, 0.0);
    let (peak_12_l, peak_12_r, rms_12, buf_12) = run_case(12.0, 0.0);
    let expected_gain = db_to_lin_f64(12.0);
    let expected_center_peak = expected_gain * pan_mono_equal_power_f64(0.0)[0];

    assert!(
        peak_12_l > 2.5 && peak_12_r > 2.5,
        "Out.gain_db=+12 dB must flow > 2.5 linear unclipped into float bus: L={peak_12_l}, R={peak_12_r}"
    );
    assert!(
        (peak_12_l - expected_center_peak).abs() < 0.02
            && (peak_12_r - expected_center_peak).abs() < 0.02,
        "center +12 dB peak expected ~{expected_center_peak:.4}, got L={peak_12_l:.4}, R={peak_12_r:.4}"
    );
    assert!(
        (rms_12 / rms_0 - expected_gain).abs() < 0.01,
        "RMS ratio expected db_to_lin(12.0)={expected_gain:.4}, got {:.4}",
        rms_12 / rms_0
    );
    let h1 = dft_power(&buf_12, 440.0, sr);
    let h3 = dft_power(&buf_12, 1_320.0, sr);
    assert!(
        h3 < h1 * 1e-6,
        "Out.gain_db=+12 dB must stay linear with zero clipping harmonics: h1={h1}, h3={h3}"
    );

    // Hard-left pan at +12 dB peaks near 3.981 (< VOICE_GUARD_KNEE = 4.0) — still completely linear.
    let (hard_l, hard_r, _, buf_hard) = run_case(12.0, -1.0);
    assert!(
        (hard_l - expected_gain).abs() < 0.02 && hard_r < 1e-6,
        "hard-left +12 dB expected L~{expected_gain:.4} R=0, got L={hard_l:.4} R={hard_r:.4}"
    );
    let hard_h1 = dft_power(&buf_hard, 440.0, sr);
    let hard_h3 = dft_power(&buf_hard, 1_320.0, sr);
    assert!(
        hard_h3 < hard_h1 * 1e-6,
        "hard-left +12 dB (< 4.0 knee) must stay linear: h1={hard_h1}, h3={hard_h3}"
    );
}

#[test]
fn runaway_feedback_patches_stay_finite_and_bounded_by_voice_guard_ceiling() {
    let bank: SampleBank = Vec::new();
    let make_runaway_zone = |out_gain_db: f32, out_pan: f32| {
        build_zone(
            vec![
                RackModule::new(
                    1,
                    "Sine",
                    ModuleKind::Vco(VcoModule {
                        waveform: Waveform::Sine,
                        pitch_tracking: PitchTracking::Fixed,
                        fixed_hz: 220.0,
                        level: 1.0,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(
                    2,
                    "LoopVCA",
                    ModuleKind::Vca(VcaModule {
                        gain: 2.0,
                        bias: 0.0,
                    }),
                ),
                RackModule::new(
                    3,
                    "Out",
                    ModuleKind::Out(OutModule {
                        gain_db: out_gain_db,
                        pan: out_pan,
                        spread: 0.0,
                    }),
                ),
            ],
            vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 2, 2, InPort::In, 2.0), // z-1 +200% self-feedback (4x loop gain)
                PatchCable::new(3, 2, 3, InPort::In, 1.0),
            ],
        )
    };

    let full_vel = VoiceStart {
        velocity: 1.0,
        ..start(60, None)
    };
    let knee_f32 = VOICE_GUARD_KNEE as f32;
    let ceiling_f32 = VOICE_GUARD_CEILING as f32;

    // 1. Runaway 4x self-feedback into unity centered Out hits soft knee (> 4.0) and stays strictly inside (-8.0, 8.0).
    let zone = make_runaway_zone(0.0, 0.0);
    let graph = compile_zone_graph(&zone).unwrap();
    assert!(graph.has_feedback());
    for sr in [8_000.0_f64, 44_100.0, 48_000.0, 96_000.0] {
        let mut voice = ModularVoice::new_boxed(&spec(&zone, &graph, sr, 120.0), full_vel);
        let mut peak = 0.0_f32;
        assert_no_heap_operations(|| {
            for _ in 0..4_096 {
                let f = voice.next_frame(&bank, None);
                assert!(
                    f[0].is_finite()
                        && f[1].is_finite()
                        && f[0].abs() < ceiling_f32
                        && f[1].abs() < ceiling_f32,
                    "sr={sr}: runaway feedback exceeded strict (-{ceiling_f32}, {ceiling_f32}) bound: {f:?}"
                );
                peak = peak.max(f[0].abs()).max(f[1].abs());
            }
        });
        assert!(
            peak > knee_f32,
            "sr={sr}: expected runaway patch to engage voice_guard_f64 above knee {knee_f32}, got peak={peak}"
        );
        voice.force_release();
        for _ in 0..(sr * 2.1) as usize {
            let f = voice.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
            if !voice.is_active() {
                break;
            }
        }
        assert!(
            !voice.is_active(),
            "sr={sr}: runaway voice failed to retire"
        );
    }

    // 2. Extreme +12 dB hard-panned runaway feedback still stays strictly finite and <= VOICE_GUARD_CEILING.
    let extreme_zone = make_runaway_zone(12.0, -1.0);
    let extreme_graph = compile_zone_graph(&extreme_zone).unwrap();
    let mut extreme_voice = ModularVoice::new_boxed(
        &spec(&extreme_zone, &extreme_graph, 48_000.0, 120.0),
        full_vel,
    );
    let mut extreme_peak = 0.0_f32;
    for _ in 0..4_096 {
        let f = extreme_voice.next_frame(&bank, None);
        assert!(
            f[0].is_finite()
                && f[1].is_finite()
                && f[0].abs() <= ceiling_f32
                && f[1].abs() <= ceiling_f32,
            "extreme runaway exceeded ceiling {ceiling_f32}: {f:?}"
        );
        extreme_peak = extreme_peak.max(f[0].abs());
    }
    assert!(extreme_peak > knee_f32);
}

#[test]
fn out_pan_preserves_constant_stereo_power_for_mono_and_stereo_sources() {
    let sr = 48_000.0_f64;
    let empty_bank: SampleBank = Vec::new();
    let pans = [-1.0_f32, -0.75, -0.5, -0.25, 0.0, 0.25, 0.5, 0.75, 1.0];
    let warmup = 4_800usize;
    let measure = 4_800usize; // exact integer cycles for 300 Hz, 440 Hz, and 500 Hz at 48 kHz

    // 1. Mono VCO source panned across [-1.0, 1.0].
    let measure_mono = |pan: f32| -> (f64, f64, f64) {
        let zone = build_zone(
            vec![
                RackModule::new(
                    1,
                    "Sine",
                    ModuleKind::Vco(VcoModule {
                        waveform: Waveform::Sine,
                        pitch_tracking: PitchTracking::Fixed,
                        fixed_hz: 440.0,
                        level: 0.8,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(
                    2,
                    "Out",
                    ModuleKind::Out(OutModule {
                        gain_db: 0.0,
                        pan,
                        spread: 0.0,
                    }),
                ),
            ],
            vec![PatchCable::new(1, 1, 2, InPort::In, 1.0)],
        );
        let graph = compile_zone_graph(&zone).unwrap();
        let mut voice = ModularVoice::new_boxed(&spec(&zone, &graph, sr, 120.0), start(69, None));
        let (mut l_sq, mut r_sq) = (0.0_f64, 0.0_f64);
        for i in 0..(warmup + measure) {
            let f = voice.next_frame(&empty_bank, None);
            if i >= warmup {
                l_sq += f64::from(f[0]).powi(2);
                r_sq += f64::from(f[1]).powi(2);
            }
        }
        (
            (l_sq + r_sq) / measure as f64,
            l_sq / measure as f64,
            r_sq / measure as f64,
        )
    };

    let (mono_center_p, mono_center_l, mono_center_r) = measure_mono(0.0);
    assert!((mono_center_l - mono_center_r).abs() < 1e-6);
    for &pan in &pans {
        let (p, pl, pr) = measure_mono(pan);
        let diff_db = 10.0 * (p / mono_center_p).log10();
        assert!(
            diff_db.abs() < 0.05,
            "mono pan={pan}: stereo power drifted {diff_db:+.4} dB from center"
        );
        if pan == -1.0 {
            assert!(pr < 1e-10 && pl > 0.1);
        } else if pan == 1.0 {
            assert!(pl < 1e-10 && pr > 0.1);
        }
    }

    // 2. Uncorrelated equal-RMS stereo source (L = sin 300 Hz, R = cos 500 Hz) panned across [-1.0, 1.0].
    let frames = 14_400usize;
    let left: Box<[f32]> = (0..frames)
        .map(|i| (TAU * 300.0 * i as f64 / sr).sin() as f32 * 0.8)
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let right: Box<[f32]> = (0..frames)
        .map(|i| (TAU * 500.0 * i as f64 / sr).cos() as f32 * 0.8)
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let stereo_sample = Arc::new(SampleData {
        path_label: "./stereo_test.wav".into(),
        resolved_path: std::path::PathBuf::from("stereo_test.wav"),
        name: "stereo_test".into(),
        sample_rate: sr as u32,
        channels: 2,
        frames,
        left,
        right: Some(right),
        peaks: vec![(-1.0, 1.0); 64].into_boxed_slice(),
        meta: SampleMetadata::default(),
    });
    let stereo_bank: SampleBank = vec![(module_sample_key(1, 1, 1), stereo_sample)];

    let measure_stereo = |pan: f32, spread: f32| -> (f64, f64, f64, f64, f64) {
        let zone = build_zone(
            vec![
                RackModule::new(
                    1,
                    "StereoSmp",
                    ModuleKind::Sampler(SamplerModule {
                        path: Some("./stereo_test.wav".into()),
                        root_midi: 60,
                        sample_end: frames,
                        one_shot: false,
                        loop_mode: SampleLoopMode::Forward,
                        loop_start: 0,
                        loop_end: frames,
                        ..SamplerModule::default()
                    }),
                ),
                RackModule::new(
                    2,
                    "Out",
                    ModuleKind::Out(OutModule {
                        gain_db: 0.0,
                        pan,
                        spread,
                    }),
                ),
            ],
            vec![PatchCable::new(1, 1, 2, InPort::In, 1.0)],
        );
        let graph = compile_zone_graph(&zone).unwrap();
        let mut voice = ModularVoice::new_boxed(&spec(&zone, &graph, sr, 120.0), start(60, None));
        let (mut l_sq, mut r_sq, mut mid_sq, mut side_sq) = (0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64);
        for i in 0..(warmup + measure) {
            let f = voice.next_frame(&stereo_bank, None);
            if i >= warmup {
                let l = f64::from(f[0]);
                let r = f64::from(f[1]);
                l_sq += l * l;
                r_sq += r * r;
                let [m, s] = cadence_core::dsp::stereo_ms_encode_f64([l, r]);
                mid_sq += m * m;
                side_sq += s * s;
            }
        }
        let n = measure as f64;
        (
            (l_sq + r_sq) / n,
            l_sq / n,
            r_sq / n,
            (mid_sq / n).sqrt(),
            (side_sq / n).sqrt(),
        )
    };

    let (st_center_p, _, _, mid_0, side_0) = measure_stereo(0.0, 0.0);
    for &pan in &pans {
        let (p, pl, pr, _, _) = measure_stereo(pan, 0.0);
        let diff_db = 10.0 * (p / st_center_p).log10();
        assert!(
            diff_db.abs() < 0.05,
            "stereo pan={pan}: stereo power drifted {diff_db:+.4} dB from center"
        );
        if pan == -1.0 {
            assert!(pr < 1e-10 && pl > 0.1);
        } else if pan == 1.0 {
            assert!(pl < 1e-10 && pr > 0.1);
        }
    }

    // 3. Out.spread = 1.0 preserves Mid and doubles Side via stereo_ms_encode/decode_f64.
    let (_, _, _, mid_1, side_1) = measure_stereo(0.0, 1.0);
    assert!(
        (mid_1 / mid_0 - 1.0).abs() < 1e-3,
        "Out.spread=1.0 must preserve Mid: mid_0={mid_0}, mid_1={mid_1}"
    );
    assert!(
        (side_1 / side_0 - 2.0).abs() < 1e-3,
        "Out.spread=1.0 must double Side: side_0={side_0}, side_1={side_1}"
    );
}

#[test]
fn out_node_clamps_gain_db_to_spot_bounds_and_sanitizes_nan() {
    use cadence_core::rack::{OUT_MAX_GAIN_DB, OUT_MIN_GAIN_DB};
    let sr = 48_000.0;
    let bank = Vec::new();
    let measure_peak = |gain_db: f32| -> f32 {
        let mut z = build_zone(
            vec![
                RackModule::new(
                    1,
                    "VCO",
                    ModuleKind::Vco(VcoModule {
                        waveform: Waveform::Sine,
                        pitch_tracking: PitchTracking::Fixed,
                        fixed_hz: 220.0,
                        level: 0.5,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(2, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![PatchCable::new(1, 1, 2, InPort::In, 1.0)],
        );
        let g = compile_zone_graph(&z).unwrap();
        // Inject raw out-of-range / NaN gain_db directly into the runtime zone after graph compile.
        if let ModuleKind::Out(ref mut out) = z.modules[1].kind {
            out.gain_db = gain_db;
        }
        let mut v = ModularVoice::new_boxed(&spec(&z, &g, sr, 120.0), start(60, None));
        let mut peak = 0.0_f32;
        for i in 0..4800 {
            let f = v.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
            if i >= 2400 {
                peak = peak.max(f[0].abs());
            }
        }
        peak
    };

    let peak_max = measure_peak(OUT_MAX_GAIN_DB);
    let peak_over = measure_peak(OUT_MAX_GAIN_DB + 40.0);
    let peak_min = measure_peak(OUT_MIN_GAIN_DB);
    let peak_under = measure_peak(OUT_MIN_GAIN_DB - 40.0);
    let peak_zero = measure_peak(0.0);
    let peak_nan = measure_peak(f32::NAN);

    assert!(
        (peak_over - peak_max).abs() < 1e-5,
        "gain_db above OUT_MAX_GAIN_DB must clamp to max ({peak_over} vs {peak_max})"
    );
    assert!(
        (peak_under - peak_min).abs() < 1e-5,
        "gain_db below OUT_MIN_GAIN_DB must clamp to min ({peak_under} vs {peak_min})"
    );
    assert!(
        (peak_nan - peak_zero).abs() < 1e-5,
        "NaN gain_db must sanitize to 0.0 dB ({peak_nan} vs {peak_zero})"
    );
}

fn synthetic_rack_zone_for_module_kind(tag: ModuleKindTag) -> RackZone {
    let fast_env = || EnvModule {
        mode: EnvMode::Adsr,
        attack_ms: 1.0,
        hold_ms: 0.0,
        decay_ms: 40.0,
        sustain: 0.65,
        release_ms: 20.0,
        cubic_limit: 0.0,
        trigger_mode: TriggerMode::Gate,
        delay_ms: 0.0,
        bursts: 1,
        burst_spacing_ms: 10.0,
    };
    let base_vco = || VcoModule {
        waveform: Waveform::MobiusJet,
        unison_voices: 3,
        unison_detune_cents: 14.0,
        unison_width: 0.8,
        level: 0.75,
        ..VcoModule::default()
    };
    match tag {
        ModuleKindTag::Vco => build_zone(
            vec![
                RackModule::new(1, "VCO", ModuleKind::Vco(base_vco())),
                RackModule::new(2, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(3, "Env", ModuleKind::Env(fast_env())),
                RackModule::new(4, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 3, 2, InPort::Cv, 1.0),
                PatchCable::new(3, 2, 4, InPort::In, 1.0),
            ],
        ),
        ModuleKindTag::Aero => build_zone(
            vec![
                RackModule::new(
                    1,
                    "Aero",
                    ModuleKind::Aero(AeroModule {
                        mode: AeroMode::BrassReed,
                        geometry: BodyGeometry::Harmonic,
                        atoms: 10,
                        drive: 0.8,
                        embouchure: 0.55,
                        p_beat_pa: 1_500.0,
                        brightness: 0.6,
                        radiation: 0.35,
                        spread: 0.4,
                        level: 0.9,
                        ..AeroModule::default()
                    }),
                ),
                RackModule::new(2, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![PatchCable::new(1, 1, 2, InPort::In, 1.0)],
        ),
        ModuleKindTag::Sampler => build_zone(
            vec![
                RackModule::new(
                    1,
                    "Sampler",
                    ModuleKind::Sampler(SamplerModule {
                        path: Some("./test.wav".into()),
                        sample_start: 0,
                        sample_end: 2048,
                        loop_mode: SampleLoopMode::Forward,
                        loop_start: 128,
                        loop_end: 1800,
                        one_shot: false,
                        ..SamplerModule::default()
                    }),
                ),
                RackModule::new(2, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(3, "Env", ModuleKind::Env(fast_env())),
                RackModule::new(4, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 3, 2, InPort::Cv, 1.0),
                PatchCable::new(3, 2, 4, InPort::In, 1.0),
            ],
        ),
        ModuleKindTag::Noise => build_zone(
            vec![
                RackModule::new(
                    1,
                    "Noise",
                    ModuleKind::Noise(NoiseModule {
                        color: NoiseColor::Pink,
                        highpass_hz: 120.0,
                        lowpass_hz: 9_000.0,
                        level: 0.7,
                    }),
                ),
                RackModule::new(2, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(3, "Env", ModuleKind::Env(fast_env())),
                RackModule::new(4, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 3, 2, InPort::Cv, 1.0),
                PatchCable::new(3, 2, 4, InPort::In, 1.0),
            ],
        ),
        ModuleKindTag::Env => build_zone(
            vec![
                RackModule::new(1, "VCO", ModuleKind::Vco(base_vco())),
                RackModule::new(2, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(
                    3,
                    "Env",
                    ModuleKind::Env(EnvModule {
                        mode: EnvMode::HoldDecay,
                        hold_ms: 4.0,
                        decay_ms: 45.0,
                        delay_ms: 2.0,
                        bursts: 3,
                        burst_spacing_ms: 8.0,
                        trigger_mode: TriggerMode::OneShot,
                        ..fast_env()
                    }),
                ),
                RackModule::new(4, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 3, 2, InPort::Cv, 1.0),
                PatchCable::new(3, 2, 4, InPort::In, 1.0),
            ],
        ),
        ModuleKindTag::Lfo => build_zone(
            vec![
                RackModule::new(1, "VCO", ModuleKind::Vco(base_vco())),
                RackModule::new(
                    2,
                    "LFO",
                    ModuleKind::Lfo(LfoModule {
                        shape: LfoShape::SampleHold,
                        rate_hz: 9.0,
                        sync: false,
                        retrigger: true,
                        ..LfoModule::default()
                    }),
                ),
                RackModule::new(3, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(4, "Env", ModuleKind::Env(fast_env())),
                RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![
                PatchCable::new(1, 1, 3, InPort::In, 1.0),
                PatchCable::new(2, 2, 1, InPort::PitchCv, 0.35),
                PatchCable::new(3, 4, 3, InPort::Cv, 1.0),
                PatchCable::new(4, 3, 5, InPort::In, 1.0),
            ],
        ),
        ModuleKindTag::Filter => build_zone(
            vec![
                RackModule::new(1, "VCO", ModuleKind::Vco(base_vco())),
                RackModule::new(
                    2,
                    "Filter",
                    ModuleKind::Filter(FilterModule {
                        mode: FilterMode::DiodeLadder,
                        cutoff_hz: 700.0,
                        resonance: 0.65,
                        drive: 0.4,
                        vel_amount: 2.5,
                        ..FilterModule::default()
                    }),
                ),
                RackModule::new(3, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(4, "Env", ModuleKind::Env(fast_env())),
                RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 4, 2, InPort::Cv, 2.0),
                PatchCable::new(3, 2, 3, InPort::In, 1.0),
                PatchCable::new(4, 4, 3, InPort::Cv, 1.0),
                PatchCable::new(5, 3, 5, InPort::In, 1.0),
            ],
        ),
        ModuleKindTag::Vca => build_zone(
            vec![
                RackModule::new(1, "Carrier", ModuleKind::Vco(base_vco())),
                RackModule::new(
                    2,
                    "Mod",
                    ModuleKind::Vco(VcoModule {
                        waveform: Waveform::Sine,
                        tune_semitones: 7.0,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(
                    3,
                    "Ring",
                    ModuleKind::Vca(VcaModule {
                        bias: 0.15,
                        ..VcaModule::default()
                    }),
                ),
                RackModule::new(4, "Amp", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(5, "Env", ModuleKind::Env(fast_env())),
                RackModule::new(6, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![
                PatchCable::new(1, 1, 3, InPort::In, 1.0),
                PatchCable::new(2, 2, 3, InPort::Cv, 1.0),
                PatchCable::new(3, 3, 4, InPort::In, 1.0),
                PatchCable::new(4, 5, 4, InPort::Cv, 1.0),
                PatchCable::new(5, 4, 6, InPort::In, 1.0),
            ],
        ),
        ModuleKindTag::Drive => build_zone(
            vec![
                RackModule::new(1, "VCO", ModuleKind::Vco(base_vco())),
                RackModule::new(
                    2,
                    "Drive",
                    ModuleKind::Drive(DriveModule {
                        mode: DriveMode::ShockPort,
                        drive: 0.72,
                        asymmetry: 0.4,
                        tone_hz: 6_500.0,
                        output_db: -2.0,
                        mix: 0.8,
                    }),
                ),
                RackModule::new(3, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(4, "Env", ModuleKind::Env(fast_env())),
                RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 2, 3, InPort::In, 1.0),
                PatchCable::new(3, 4, 3, InPort::Cv, 1.0),
                PatchCable::new(4, 3, 5, InPort::In, 1.0),
            ],
        ),
        ModuleKindTag::LoFi => build_zone(
            vec![
                RackModule::new(1, "VCO", ModuleKind::Vco(base_vco())),
                RackModule::new(
                    2,
                    "LoFi",
                    ModuleKind::LoFi(LoFiModule {
                        bits: 6.5,
                        rate_hz: 5_400.0,
                        mix: 0.85,
                    }),
                ),
                RackModule::new(3, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(4, "Env", ModuleKind::Env(fast_env())),
                RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 2, 3, InPort::In, 1.0),
                PatchCable::new(3, 4, 3, InPort::Cv, 1.0),
                PatchCable::new(4, 3, 5, InPort::In, 1.0),
            ],
        ),
        ModuleKindTag::Modal => build_zone(
            vec![
                RackModule::new(1, "Noise", ModuleKind::Noise(NoiseModule::default())),
                RackModule::new(2, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(
                    3,
                    "Env",
                    ModuleKind::Env(EnvModule {
                        attack_ms: 0.5,
                        decay_ms: 12.0,
                        sustain: 0.0,
                        release_ms: 10.0,
                        trigger_mode: TriggerMode::OneShot,
                        ..fast_env()
                    }),
                ),
                RackModule::new(
                    4,
                    "Modal",
                    ModuleKind::Modal(ModalModule {
                        geometry: BodyGeometry::Plate,
                        atoms: 20,
                        radiation: 0.65,
                        brightness: 0.65,
                        coupling: 0.25,
                        triad_amount: 0.3,
                        spread: 0.45,
                        strike_drive: 0.6,
                        mix: 1.0,
                        ..ModalModule::default()
                    }),
                ),
                RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 3, 2, InPort::Cv, 1.0),
                PatchCable::new(3, 2, 4, InPort::In, 1.0),
                PatchCable::new(4, 4, 5, InPort::In, 1.0),
            ],
        ),
        ModuleKindTag::Out => build_zone(
            vec![
                RackModule::new(1, "VCO", ModuleKind::Vco(base_vco())),
                RackModule::new(2, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(3, "Env", ModuleKind::Env(fast_env())),
                RackModule::new(
                    4,
                    "Out",
                    ModuleKind::Out(OutModule {
                        gain_db: 3.0,
                        pan: -0.35,
                        spread: 0.6,
                    }),
                ),
            ],
            vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 3, 2, InPort::Cv, 1.0),
                PatchCable::new(3, 2, 4, InPort::In, 1.0),
            ],
        ),
    }
}

#[test]
fn all_module_kinds_and_modes_satisfy_render_block_parity_and_zero_alloc() {
    let sr = 48_000.0;
    let sample = synthetic_sample(2048, sr as u32, 330.0);
    let bank = vec![(module_sample_key(1, 1, 1), sample)];

    let mut cases: Vec<(String, RackZone, bool)> = Vec::new();
    // 1. Exhaustive ModuleKindTag::ALL.
    for &tag in ModuleKindTag::ALL {
        cases.push((
            format!("ModuleKind::{tag:?}"),
            synthetic_rack_zone_for_module_kind(tag),
            false,
        ));
    }
    // 2. Exhaustive FilterMode::ALL.
    for &mode in FilterMode::ALL {
        let mut z = synthetic_rack_zone_for_module_kind(ModuleKindTag::Filter);
        if let ModuleKind::Filter(ref mut flt) = z.modules[1].kind {
            flt.mode = mode;
        }
        cases.push((format!("FilterMode::{mode:?}"), z, false));
    }
    // 3. Exhaustive DriveMode::ALL.
    for mode in DriveMode::ALL {
        let mut z = synthetic_rack_zone_for_module_kind(ModuleKindTag::Drive);
        if let ModuleKind::Drive(ref mut drv) = z.modules[1].kind {
            drv.mode = mode;
        }
        cases.push((format!("DriveMode::{mode:?}"), z, false));
    }
    // 4. Exhaustive SampleLoopMode::ALL.
    for &loop_mode in SampleLoopMode::ALL {
        let mut z = synthetic_rack_zone_for_module_kind(ModuleKindTag::Sampler);
        if let ModuleKind::Sampler(ref mut smp) = z.modules[0].kind {
            smp.loop_mode = loop_mode;
            smp.one_shot = loop_mode == SampleLoopMode::Off;
        }
        cases.push((format!("SampleLoopMode::{loop_mode:?}"), z, false));
    }
    // 5. Exhaustive VoiceMode::ALL (including Mono legato glide).
    for &voice_mode in VoiceMode::ALL {
        let mut z = synthetic_rack_zone_for_module_kind(ModuleKindTag::Filter);
        z.voice.voice_mode = voice_mode;
        z.voice.glide_ms = if voice_mode == VoiceMode::Mono {
            24.0
        } else {
            0.0
        };
        cases.push((
            format!("VoiceMode::{voice_mode:?}"),
            z,
            voice_mode == VoiceMode::Mono,
        ));
    }
    // 6. Exhaustive BodyGeometry::ALL, PairTopology::ALL, and TriadAxis::ALL on Modal.
    for &geom in BodyGeometry::ALL {
        let mut z = synthetic_rack_zone_for_module_kind(ModuleKindTag::Modal);
        if let ModuleKind::Modal(ref mut m) = z.modules[3].kind {
            m.geometry = geom;
        }
        cases.push((format!("Modal::Geometry::{geom:?}"), z, false));
    }
    for &topo in PairTopology::ALL {
        let mut z = synthetic_rack_zone_for_module_kind(ModuleKindTag::Modal);
        if let ModuleKind::Modal(ref mut m) = z.modules[3].kind {
            m.pair_topology = topo;
            m.coupling = 0.4;
        }
        cases.push((format!("Modal::PairTopology::{topo:?}"), z, false));
    }
    for &axis in TriadAxis::ALL {
        let mut z = synthetic_rack_zone_for_module_kind(ModuleKindTag::Modal);
        if let ModuleKind::Modal(ref mut m) = z.modules[3].kind {
            m.triad_axis = axis;
            m.triad_amount = 0.5;
        }
        cases.push((format!("Modal::TriadAxis::{axis:?}"), z, false));
    }
    // 7. Exhaustive AeroMode::ALL (including Mono legato retarget).
    for &aero_mode in AeroMode::ALL {
        let mut z = synthetic_rack_zone_for_module_kind(ModuleKindTag::Aero);
        if let ModuleKind::Aero(ref mut aero) = z.modules[0].kind {
            aero.mode = aero_mode;
        }
        cases.push((format!("AeroMode::{aero_mode:?}"), z, false));
    }
    // 8. Exhaustive Waveform::ALL and EnvMode::ALL.
    for &waveform in Waveform::ALL {
        let mut z = synthetic_rack_zone_for_module_kind(ModuleKindTag::Vco);
        if let ModuleKind::Vco(ref mut vco) = z.modules[0].kind {
            vco.waveform = waveform;
            vco.chirality = 0.45;
        }
        cases.push((format!("Waveform::{waveform:?}"), z, false));
    }
    for &env_mode in EnvMode::ALL {
        let mut z = synthetic_rack_zone_for_module_kind(ModuleKindTag::Env);
        if let ModuleKind::Env(ref mut env) = z.modules[2].kind {
            env.mode = env_mode;
            env.cubic_limit = 0.75;
            env.trigger_mode = if env_mode == EnvMode::Capacitor {
                TriggerMode::Additive
            } else {
                TriggerMode::OneShot
            };
        }
        cases.push((format!("EnvMode::{env_mode:?}"), z, false));
    }
    // 9. Feedback z^-1 loop topology (VCO self-FM + Filter -> Drive -> Filter.In).
    let fb_zone = build_zone(
        vec![
            RackModule::new(1, "VCO", ModuleKind::Vco(VcoModule::default())),
            RackModule::new(
                2,
                "Filter",
                ModuleKind::Filter(FilterModule {
                    cutoff_hz: 1_100.0,
                    resonance: 0.72,
                    ..FilterModule::default()
                }),
            ),
            RackModule::new(
                3,
                "Drive",
                ModuleKind::Drive(DriveModule {
                    mode: DriveMode::AtomicJet,
                    drive: 0.7,
                    ..DriveModule::default()
                }),
            ),
            RackModule::new(4, "VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(5, "Env", ModuleKind::Env(EnvModule::default())),
            RackModule::new(6, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 1, 1, InPort::FmIn, 0.3),
            PatchCable::new(2, 1, 2, InPort::In, 1.0),
            PatchCable::new(3, 2, 3, InPort::In, 1.0),
            PatchCable::new(4, 3, 2, InPort::In, 0.8),
            PatchCable::new(5, 3, 4, InPort::In, 1.0),
            PatchCable::new(6, 5, 4, InPort::Cv, 1.0),
            PatchCable::new(7, 4, 6, InPort::In, 1.0),
        ],
    );
    cases.push(("FeedbackTopology".into(), fb_zone, false));

    let mut frame_voice = ModularVoice::new_boxed_inactive(sr);
    let mut block_voice = ModularVoice::new_boxed_inactive(sr);
    let mut blk_buf = [[0.0_f32; 2]; 64];
    let mut ref_buf = [[0.0_f32; 2]; 64];

    for (name, zone, test_legato) in &cases {
        let graph = compile_zone_graph(zone).unwrap_or_else(|e| panic!("{name} compile: {e}"));
        let zspec = spec(zone, &graph, sr, 124.0);
        let mut peak = 0.0_f32;

        assert_no_heap_operations(|| {
            let onset = start(60, Some(640));
            frame_voice.begin_trigger_from_zone(&zspec);
            frame_voice.finalize_trigger(onset, false, false);
            block_voice.begin_trigger_from_zone(&zspec);
            block_voice.finalize_trigger(onset, false, false);

            let chunks = [1usize, 7, 13, 64, 31, 64, 19, 64, 64, 64];
            for (ci, &chunk) in chunks.iter().enumerate() {
                if ci == 3 {
                    frame_voice.update_continuous_from_zone(zone, 0.1, 0.0, 128.0);
                    block_voice.update_continuous_from_zone(zone, 0.1, 0.0, 128.0);
                }
                if ci == 5 && *test_legato {
                    let next_note = start(67, Some(320));
                    frame_voice.retarget_from_zone(&zspec, next_note, true);
                    block_voice.retarget_from_zone(&zspec, next_note, true);
                }
                if ci == 7 {
                    frame_voice.note_off();
                    block_voice.note_off();
                }

                let mut ref_written = 0usize;
                for slot in ref_buf.iter_mut().take(chunk) {
                    if !frame_voice.is_active() {
                        break;
                    }
                    *slot = frame_voice.next_frame(&bank, None);
                    ref_written += 1;
                }
                let blk_written = block_voice.render_block(&bank, None, &mut blk_buf[..chunk]);
                assert_eq!(
                    blk_written, ref_written,
                    "{name} chunk {ci} ({chunk}): written mismatch {blk_written} vs {ref_written}"
                );
                assert_eq!(
                    block_voice.is_active(),
                    frame_voice.is_active(),
                    "{name} chunk {ci}: active flag mismatch"
                );
                for i in 0..blk_written {
                    let b = blk_buf[i];
                    let r = ref_buf[i];
                    assert!(
                        b[0].is_finite()
                            && b[1].is_finite()
                            && b[0].abs() < VOICE_GUARD_CEILING as f32 * 2.0
                            && b[1].abs() < VOICE_GUARD_CEILING as f32 * 2.0,
                        "{name} chunk {ci} frame {i}: non-finite or unbounded {b:?}"
                    );
                    assert_eq!(
                        [b[0].to_bits(), b[1].to_bits()],
                        [r[0].to_bits(), r[1].to_bits()],
                        "{name} chunk {ci} frame {i}: render_block {b:?} != next_frame {r:?}"
                    );
                    peak = peak.max(b[0].abs()).max(b[1].abs());
                }
            }
        });

        assert!(
            peak > 1e-4,
            "{name}: synthetic rack produced silent output (peak={peak})"
        );
    }
}

#[test]
fn all_post_vca_tail_topologies_ring_out_and_retire_cleanly() {
    let sr = 48_000.0;
    let bank = Vec::new();
    let short_env = EnvModule {
        mode: EnvMode::Adsr,
        attack_ms: 0.5,
        decay_ms: 8.0,
        sustain: 0.0,
        release_ms: 5.0,
        trigger_mode: TriggerMode::OneShot,
        ..EnvModule::default()
    };

    let mut post_vca_zones: Vec<(String, RackZone, bool)> = Vec::new();
    // 1. Post-VCA Modal across every BodyGeometry::ALL.
    for &geom in BodyGeometry::ALL {
        let z = build_zone(
            vec![
                RackModule::new(1, "Noise", ModuleKind::Noise(NoiseModule::default())),
                RackModule::new(2, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(3, "Env", ModuleKind::Env(short_env)),
                RackModule::new(
                    4,
                    "Modal",
                    ModuleKind::Modal(ModalModule {
                        geometry: geom,
                        atoms: 20,
                        pair_topology: PairTopology::Swarm,
                        coupling: 0.0,
                        absorption: 0.25,
                        radiation: 0.65,
                        brightness: 0.55,
                        headroom: 0.0,
                        strike_drive: 0.0,
                        mix: 1.0,
                        ..ModalModule::default()
                    }),
                ),
                RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 3, 2, InPort::Cv, 1.0),
                PatchCable::new(3, 2, 4, InPort::In, 1.0),
                PatchCable::new(4, 4, 5, InPort::In, 1.0),
            ],
        );
        post_vca_zones.push((format!("PostVca::Modal::{geom:?}"), z, true));
    }
    // 2. Post-VCA resonant Filter across every FilterMode::ALL.
    for &mode in FilterMode::ALL {
        let z = build_zone(
            vec![
                RackModule::new(1, "VCO", ModuleKind::Vco(VcoModule::default())),
                RackModule::new(2, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(3, "Env", ModuleKind::Env(short_env)),
                RackModule::new(
                    4,
                    "Filter",
                    ModuleKind::Filter(FilterModule {
                        mode,
                        cutoff_hz: 600.0,
                        resonance: 0.88,
                        ..FilterModule::default()
                    }),
                ),
                RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 3, 2, InPort::Cv, 1.0),
                PatchCable::new(3, 2, 4, InPort::In, 1.0),
                PatchCable::new(4, 4, 5, InPort::In, 1.0),
            ],
        );
        let expect_ring = mode != FilterMode::LowPass6;
        post_vca_zones.push((format!("PostVca::Filter::{mode:?}"), z, expect_ring));
    }
    // 3. Post-VCA Drive across every DriveMode::ALL + LoFi.
    for mode in DriveMode::ALL {
        let z = build_zone(
            vec![
                RackModule::new(1, "VCO", ModuleKind::Vco(VcoModule::default())),
                RackModule::new(2, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(3, "Env", ModuleKind::Env(short_env)),
                RackModule::new(
                    4,
                    "Drive",
                    ModuleKind::Drive(DriveModule {
                        mode,
                        drive: 0.6,
                        ..DriveModule::default()
                    }),
                ),
                RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 3, 2, InPort::Cv, 1.0),
                PatchCable::new(3, 2, 4, InPort::In, 1.0),
                PatchCable::new(4, 4, 5, InPort::In, 1.0),
            ],
        );
        post_vca_zones.push((format!("PostVca::Drive::{mode:?}"), z, false));
    }

    let mut voice = ModularVoice::new_boxed_inactive(sr);
    let mut blk = [[0.0_f32; 2]; 64];
    let env_done_frames = (sr * 0.012) as usize;
    let max_retire_frames = (sr * 1.5) as usize;

    for (name, zone, expect_post_env_ring) in &post_vca_zones {
        let graph = compile_zone_graph(zone).unwrap();
        let zspec = spec(zone, &graph, sr, 120.0);
        let mut total = 0usize;
        let mut post_env_peak = 0.0_f32;

        assert_no_heap_operations(|| {
            voice.begin_trigger_from_zone(&zspec);
            voice.finalize_trigger(start(60, Some(240)), false, false);
            while voice.is_active() && total < max_retire_frames {
                let n = voice.render_block(&bank, None, &mut blk);
                for frame in &blk[..n] {
                    assert!(frame[0].is_finite() && frame[1].is_finite());
                    if total >= env_done_frames {
                        post_env_peak = post_env_peak.max(frame[0].abs()).max(frame[1].abs());
                    }
                    total += 1;
                }
            }
        });

        assert!(
            !voice.is_active(),
            "{name}: voice failed to retire within {max_retire_frames} frames"
        );
        if *expect_post_env_ring {
            assert!(
                post_env_peak > 1e-4,
                "{name}: expected audible post-VCA tail past envelope end, got {post_env_peak}"
            );
        }
    }
}

#[test]
fn filter_node_supports_cubic_poles_diode_kerr_spread_and_click_free_mode_switch() {
    use cadence_core::dsp::{filter_magnitude_db, lin_to_db_f64};
    let sr = 48_000.0;
    let bank: SampleBank = Vec::new();

    let make_filter_zone = |freq_hz: f32, vco_wave: Waveform, level: f32, flt: FilterModule| {
        let mut zone = build_zone(
            vec![
                RackModule::new(
                    1,
                    "VCO",
                    ModuleKind::Vco(VcoModule {
                        waveform: vco_wave,
                        pitch_tracking: PitchTracking::Fixed,
                        fixed_hz: freq_hz,
                        level,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(2, "Filter", ModuleKind::Filter(flt)),
                RackModule::new(3, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 2, 3, InPort::In, 1.0),
            ],
        );
        zone.voice.voice_mode = VoiceMode::Mono;
        zone.voice.legato = true;
        zone
    };

    // 1. ImLpf2 / ImLpf3 / ImLpf4 realize exact -12 / -18 / -24 dB/oct Butterworth stopband slopes
    //    matching `cadence_core::dsp::filter_magnitude_db` at 2 octaves above cutoff.
    let dry_zone = build_zone(
        vec![
            RackModule::new(
                1,
                "VCO",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    pitch_tracking: PitchTracking::Fixed,
                    fixed_hz: 1_600.0,
                    level: 0.7,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(2, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![PatchCable::new(1, 1, 2, InPort::In, 1.0)],
    );
    let g_dry = compile_zone_graph(&dry_zone).unwrap();
    let mut v_dry = ModularVoice::new_boxed(&spec(&dry_zone, &g_dry, sr, 120.0), start(60, None));
    let warmup = 4_800usize;
    let measure = 4_800usize;
    let mut dry_sq = 0.0_f64;
    for i in 0..(warmup + measure) {
        let y = v_dry.next_frame(&bank, None);
        if i >= warmup {
            dry_sq += f64::from(y[0]).powi(2);
        }
    }
    let dry_rms = (dry_sq / measure as f64).sqrt();

    let mut mode_rms = [0.0_f64; 3];
    for (idx, mode) in [FilterMode::ImLpf2, FilterMode::ImLpf3, FilterMode::ImLpf4]
        .into_iter()
        .enumerate()
    {
        let z = make_filter_zone(
            1_600.0,
            Waveform::Sine,
            0.7,
            FilterModule {
                mode,
                cutoff_hz: 400.0,
                resonance: 0.0,
                drive: 0.0,
                kerr_semitones: 0.0,
                spread: 0.0,
                ..FilterModule::default()
            },
        );
        let g = compile_zone_graph(&z).unwrap();
        let mut v = ModularVoice::new_boxed(&spec(&z, &g, sr, 120.0), start(60, None));
        let mut out_sq = 0.0_f64;
        for i in 0..(warmup + measure) {
            let y = v.next_frame(&bank, None);
            assert!(y[0].is_finite() && y[1].is_finite());
            if i >= warmup {
                out_sq += f64::from(y[0]).powi(2);
            }
        }
        let rms = (out_sq / measure as f64).sqrt();
        mode_rms[idx] = rms;
        let measured_db = lin_to_db_f64(rms / dry_rms);
        let expected_db = filter_magnitude_db(mode, 400.0, 0.0, 1_600.0, sr);
        assert!(
            (measured_db - expected_db).abs() < 0.75,
            "{mode:?}: measured {measured_db:.2} dB vs analytical {expected_db:.2} dB"
        );
    }
    assert!(
        mode_rms[2] < mode_rms[1] * 0.35 && mode_rms[1] < mode_rms[0] * 0.35,
        "ImLpf4 < ImLpf3 < ImLpf2 stopband RMS violated: {mode_rms:?}"
    );

    // 2. DiodeLadder driven resonant sweep stays bounded and DC-free.
    let z_diode = make_filter_zone(
        180.0,
        Waveform::MobiusJet,
        0.8,
        FilterModule {
            mode: FilterMode::DiodeLadder,
            cutoff_hz: 650.0,
            resonance: 0.72,
            drive: 0.65,
            ..FilterModule::default()
        },
    );
    let g_diode = compile_zone_graph(&z_diode).unwrap();
    let mut v_diode =
        ModularVoice::new_boxed(&spec(&z_diode, &g_diode, sr, 120.0), start(60, None));
    let mut diode_peak = 0.0_f32;
    let mut diode_dc = 0.0_f64;
    for i in 0..(warmup + measure) {
        let y = v_diode.next_frame(&bank, None);
        assert!(y[0].is_finite() && y[1].is_finite());
        diode_peak = diode_peak.max(y[0].abs()).max(y[1].abs());
        if i >= warmup {
            diode_dc += f64::from(y[0]);
        }
    }
    assert!(
        diode_peak > 0.15 && diode_peak < VOICE_GUARD_CEILING as f32,
        "DiodeLadder peak out of range: {diode_peak}"
    );
    assert!(
        (diode_dc / measure as f64).abs() < 2e-3,
        "DiodeLadder DC offset too high: {}",
        diode_dc / measure as f64
    );

    // 3. State-Kerr dispersion (`kerr_semitones` in -12..+12) bends resonant peak pitch and
    //    responds cleanly to continuous updates & P-locks.
    let capture_kerr = |kerr_semitones: f32| -> Vec<f64> {
        let z = make_filter_zone(
            700.0,
            Waveform::Sine,
            0.85,
            FilterModule {
                mode: FilterMode::ImLpf2,
                cutoff_hz: 700.0,
                resonance: 0.78,
                drive: 0.1,
                kerr_semitones,
                spread: 0.0,
                ..FilterModule::default()
            },
        );
        let g = compile_zone_graph(&z).unwrap();
        let mut v = ModularVoice::new_boxed(&spec(&z, &g, sr, 120.0), start(60, None));
        let mut out = Vec::with_capacity(measure);
        for i in 0..(warmup + measure) {
            let y = v.next_frame(&bank, None);
            assert!(y[0].is_finite() && y[1].is_finite());
            if i >= warmup {
                out.push(f64::from(y[0]));
            }
        }
        out
    };
    let k_neg = capture_kerr(-12.0);
    let k_zero = capture_kerr(0.0);
    let k_pos = capture_kerr(12.0);
    let l2_diff = |a: &[f64], b: &[f64]| -> f64 {
        (a.iter()
            .zip(b.iter())
            .map(|(x, y)| (x - y).powi(2))
            .sum::<f64>()
            / a.len() as f64)
            .sqrt()
    };
    let diff_neg = l2_diff(&k_neg, &k_zero);
    let diff_pos = l2_diff(&k_pos, &k_zero);
    let diff_span = l2_diff(&k_neg, &k_pos);
    assert!(
        diff_neg > 0.05 && diff_pos > 0.05 && diff_span > 0.1,
        "State-Kerr must audibly bend resonant response: neg={diff_neg:.4}, pos={diff_pos:.4}, span={diff_span:.4}"
    );

    // 4. Quadrature M/S `spread` widens stereo cleanly without Mid-channel comb notches,
    //    and continuous updates smooth both `kerr_semitones` and `spread` while honoring locks.
    let measure_spread = |spread: f32| -> (f64, f64, f64) {
        let z = make_filter_zone(
            450.0,
            Waveform::Sine,
            0.7,
            FilterModule {
                mode: FilterMode::ImLpf2,
                cutoff_hz: 900.0,
                resonance: 0.35,
                drive: 0.0,
                kerr_semitones: 0.0,
                spread,
                ..FilterModule::default()
            },
        );
        let g = compile_zone_graph(&z).unwrap();
        let mut v = ModularVoice::new_boxed(&spec(&z, &g, sr, 120.0), start(60, None));
        let mut mid_sq = 0.0_f64;
        let mut side_sq = 0.0_f64;
        let mut max_diff = 0.0_f64;
        for i in 0..(warmup + measure) {
            let y = v.next_frame(&bank, None);
            assert!(y[0].is_finite() && y[1].is_finite());
            if i >= warmup {
                let l = f64::from(y[0]);
                let r = f64::from(y[1]);
                let mid = 0.5 * (l + r);
                let side = 0.5 * (l - r);
                mid_sq += mid * mid;
                side_sq += side * side;
                max_diff = max_diff.max((l - r).abs());
            }
        }
        (
            (mid_sq / measure as f64).sqrt(),
            (side_sq / measure as f64).sqrt(),
            max_diff,
        )
    };
    let (mid_mono, side_mono, diff_mono) = measure_spread(0.0);
    let (mid_wide, side_wide, _diff_wide) = measure_spread(1.0);
    assert!(
        diff_mono < 1e-6 && side_mono < 1e-6,
        "spread=0 must remain centered mono: diff={diff_mono}"
    );
    assert!(
        side_wide > 0.08,
        "spread=1 must generate quadrature stereo side energy: side_rms={side_wide}"
    );
    let mid_ratio_db = lin_to_db_f64(mid_wide / mid_mono);
    assert!(
        (mid_ratio_db + 3.01).abs() < 0.2,
        "spread=1 Mid collapse must follow exact cos(pi/4) = -3.01 dB without comb notches: {mid_ratio_db:+.2} dB"
    );

    // Verify continuous updates respect per-voice locks on `Kerr` and `Spread`.
    let mut z_live = make_filter_zone(
        440.0,
        Waveform::MobiusJet,
        0.7,
        FilterModule {
            mode: FilterMode::ImLpf2,
            cutoff_hz: 1_200.0,
            resonance: 0.4,
            kerr_semitones: 2.0,
            spread: 0.2,
            ..FilterModule::default()
        },
    );
    let g_live = compile_zone_graph(&z_live).unwrap();
    let mut v_live = ModularVoice::new_boxed_inactive(sr);
    v_live.begin_trigger_from_zone(&spec(&z_live, &g_live, sr, 120.0));
    v_live.apply_trigger_module_param(
        2,
        ModuleParam::Kerr,
        ModuleParam::Kerr.normalize(6.0),
        0,
        true,
    );
    v_live.finalize_trigger(start(60, None), false, false);
    if let ModuleKind::Filter(ref mut flt) = z_live.modules[1].kind {
        flt.kerr_semitones = -9.0;
        flt.spread = 0.85;
    }
    v_live.update_continuous_from_zone(&z_live, 0.0, 0.0, 120.0);
    let ModuleParams::Filter(target_after) = v_live.module_target_params(2).unwrap() else {
        panic!("expected Filter target params");
    };
    assert!(
        (target_after.kerr_semitones - 6.0).abs() < 1e-4,
        "locked Kerr must ignore continuous update: got {}",
        target_after.kerr_semitones
    );
    assert!(
        (target_after.spread - 0.85).abs() < 1e-4,
        "unlocked Spread must follow continuous update: got {}",
        target_after.spread
    );

    // 5. Click-free mode switching across topologies on active mono legato retarget (zero RT allocs).
    let mut z_sw = make_filter_zone(
        220.0,
        Waveform::Sine,
        0.5,
        FilterModule {
            mode: FilterMode::ImLpf2,
            cutoff_hz: 900.0,
            resonance: 0.45,
            drive: 0.2,
            ..FilterModule::default()
        },
    );
    let g_sw = compile_zone_graph(&z_sw).unwrap();
    let mut v_sw = ModularVoice::new_boxed(&spec(&z_sw, &g_sw, sr, 120.0), start(60, None));
    for _ in 0..2_048 {
        let _ = v_sw.next_frame(&bank, None);
    }
    let mut prev = v_sw.next_frame(&bank, None);
    let mut max_step = 0.0_f32;
    let modes = [
        FilterMode::DiodeLadder,
        FilterMode::HighPass,
        FilterMode::ImLpf4,
        FilterMode::LowPass6,
        FilterMode::BandPass,
        FilterMode::ImLpf3,
    ];
    for mode in modes {
        if let ModuleKind::Filter(ref mut flt) = z_sw.modules[1].kind {
            flt.mode = mode;
        }
        let zspec = spec(&z_sw, &g_sw, sr, 120.0);
        assert_no_heap_operations(|| {
            v_sw.retarget_from_zone(&zspec, start(60, None), true);
            for _ in 0..512 {
                let cur = v_sw.next_frame(&bank, None);
                assert!(cur[0].is_finite() && cur[1].is_finite());
                max_step = max_step
                    .max((cur[0] - prev[0]).abs())
                    .max((cur[1] - prev[1]).abs());
                prev = cur;
            }
        });
    }
    assert!(
        max_step < 0.08,
        "legato mode crossfade across FilterMode topologies produced click step {max_step}"
    );
}

#[test]
fn drive_node_native_rate_atomic_jet_and_shock_port_match_core_shaper() {
    let sr = 48_000.0;
    let bank: SampleBank = Vec::new();
    let warmup = 4_800usize;
    let measure = 4_800usize;

    let run_drive =
        |level: f32, drv: DriveModule, cv_amount: Option<f32>| -> (f64, f64, f64, f64) {
            let mut modules = vec![
                RackModule::new(
                    1,
                    "VCO",
                    ModuleKind::Vco(VcoModule {
                        waveform: Waveform::Sine,
                        pitch_tracking: PitchTracking::Fixed,
                        fixed_hz: 200.0,
                        level,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(2, "Drive", ModuleKind::Drive(drv)),
                RackModule::new(3, "Out", ModuleKind::Out(OutModule::default())),
            ];
            let mut cables = vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 2, 3, InPort::In, 1.0),
            ];
            if let Some(amt) = cv_amount {
                modules.push(RackModule::new(
                    4,
                    "CvEnv",
                    ModuleKind::Env(EnvModule {
                        mode: EnvMode::HoldDecay,
                        attack_ms: 0.1,
                        hold_ms: 500.0,
                        decay_ms: 100.0,
                        trigger_mode: TriggerMode::OneShot,
                        ..EnvModule::default()
                    }),
                ));
                cables.push(PatchCable::new(3, 4, 2, InPort::Cv, amt));
            }
            let z = build_zone(modules, cables);
            let g = compile_zone_graph(&z).unwrap();
            let mut v = ModularVoice::new_boxed(&spec(&z, &g, sr, 120.0), start(60, None));
            let mut buf = Vec::with_capacity(measure);
            let mut dc_sum = 0.0_f64;
            for i in 0..(warmup + measure) {
                let y = v.next_frame(&bank, None);
                assert!(y[0].is_finite() && y[1].is_finite());
                if i >= warmup {
                    let s = f64::from(y[0]);
                    buf.push(s);
                    dc_sum += s;
                }
            }
            (
                dft_power(&buf, 200.0, sr),
                dft_power(&buf, 400.0, sr),
                dft_power(&buf, 600.0, sr),
                (dc_sum / measure as f64).abs(),
            )
        };

    // 1. Exact identity at drive=0.0 across both DriveMode variants.
    for mode in DriveMode::ALL {
        let (h1, h2, h3, dc) = run_drive(
            0.6,
            DriveModule {
                mode,
                drive: 0.0,
                asymmetry: 0.5,
                tone_hz: 18_000.0,
                output_db: 0.0,
                mix: 1.0,
            },
            None,
        );
        assert!(
            h1 > 5e-3 && h2 < h1 * 1e-6 && h3 < h1 * 1e-6 && dc < 1e-4,
            "{mode:?} drive=0 must be clean identity: h1={h1}, h2={h2}, h3={h3}, dc={dc}"
        );
    }

    // 2. ShockPort maintains a clean linear interior below headroom threshold and strongly
    //    clamps above threshold with odd harmonics.
    let (lin_h1, lin_h2, lin_h3, _) = run_drive(
        0.15,
        DriveModule {
            mode: DriveMode::ShockPort,
            drive: 0.25,
            asymmetry: 0.0,
            tone_hz: 18_000.0,
            output_db: 0.0,
            mix: 1.0,
        },
        None,
    );
    assert!(
        lin_h1 > 5e-4 && lin_h2 < lin_h1 * 1e-6 && lin_h3 < lin_h1 * 1e-6,
        "ShockPort linear interior must remain clean: h1={lin_h1}, h2={lin_h2}, h3={lin_h3}"
    );
    let (sat_h1, sat_h2, sat_h3, _) = run_drive(
        0.9,
        DriveModule {
            mode: DriveMode::ShockPort,
            drive: 0.85,
            asymmetry: 0.0,
            tone_hz: 18_000.0,
            output_db: 0.0,
            mix: 1.0,
        },
        None,
    );
    assert!(
        sat_h3 > sat_h1 * 5e-3 && sat_h2 < sat_h3 * 1e-4,
        "symmetric driven ShockPort must create strong odd harmonics: h1={sat_h1}, h2={sat_h2}, h3={sat_h3}"
    );

    // 3. AtomicJet generates smooth odd harmonics symmetrically and strong 2nd harmonic with asymmetry.
    let (jet_sym_h1, jet_sym_h2, jet_sym_h3, _) = run_drive(
        0.75,
        DriveModule {
            mode: DriveMode::AtomicJet,
            drive: 0.75,
            asymmetry: 0.0,
            tone_hz: 18_000.0,
            output_db: 0.0,
            mix: 1.0,
        },
        None,
    );
    assert!(
        jet_sym_h3 > jet_sym_h1 * 1e-3 && jet_sym_h2 < jet_sym_h3 * 1e-4,
        "symmetric AtomicJet must produce odd harmonics: h1={jet_sym_h1}, h2={jet_sym_h2}, h3={jet_sym_h3}"
    );
    let (jet_asym_h1, jet_asym_h2, _, jet_asym_dc) = run_drive(
        0.75,
        DriveModule {
            mode: DriveMode::AtomicJet,
            drive: 0.75,
            asymmetry: 0.75,
            tone_hz: 18_000.0,
            output_db: 0.0,
            mix: 1.0,
        },
        None,
    );
    assert!(
        jet_asym_h2 > jet_asym_h1 * 5e-3 && jet_asym_dc < 2e-3,
        "asymmetric AtomicJet must produce 2nd harmonic with zero DC: h1={jet_asym_h1}, h2={jet_asym_h2}, dc={jet_asym_dc}"
    );

    // 4. Drive.Cv dynamically drives saturation from zero base drive.
    let (cv_h1, _, cv_h3, _) = run_drive(
        0.75,
        DriveModule {
            mode: DriveMode::AtomicJet,
            drive: 0.0,
            asymmetry: 0.0,
            tone_hz: 18_000.0,
            output_db: 0.0,
            mix: 1.0,
        },
        Some(0.75),
    );
    assert!(
        cv_h3 > cv_h1 * 1e-3 && (cv_h3 - jet_sym_h3).abs() / jet_sym_h3 < 0.05,
        "Drive.Cv (+0.75) from drive=0 must match drive=0.75 saturation: cv_h3={cv_h3}, ref_h3={jet_sym_h3}"
    );

    // 5. Click-free mode crossfade between AtomicJet and ShockPort on mono legato retarget.
    let mut z_sw = build_zone(
        vec![
            RackModule::new(
                1,
                "VCO",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    pitch_tracking: PitchTracking::Fixed,
                    fixed_hz: 220.0,
                    level: 0.7,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "Drive",
                ModuleKind::Drive(DriveModule {
                    mode: DriveMode::AtomicJet,
                    drive: 0.75,
                    asymmetry: 0.25,
                    tone_hz: 14_000.0,
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
    z_sw.voice.voice_mode = VoiceMode::Mono;
    z_sw.voice.legato = true;
    let g_sw = compile_zone_graph(&z_sw).unwrap();
    let mut v_sw = ModularVoice::new_boxed(&spec(&z_sw, &g_sw, sr, 120.0), start(60, None));
    for _ in 0..2_048 {
        let _ = v_sw.next_frame(&bank, None);
    }
    let mut prev = v_sw.next_frame(&bank, None);
    let mut max_step = 0.0_f32;
    for next_mode in [DriveMode::ShockPort, DriveMode::AtomicJet] {
        if let ModuleKind::Drive(ref mut d) = z_sw.modules[1].kind {
            d.mode = next_mode;
        }
        let zspec = spec(&z_sw, &g_sw, sr, 120.0);
        assert_no_heap_operations(|| {
            v_sw.retarget_from_zone(&zspec, start(60, None), true);
            for _ in 0..512 {
                let cur = v_sw.next_frame(&bank, None);
                assert!(cur[0].is_finite() && cur[1].is_finite());
                max_step = max_step
                    .max((cur[0] - prev[0]).abs())
                    .max((cur[1] - prev[1]).abs());
                prev = cur;
            }
        });
    }
    assert!(
        max_step < 0.08,
        "legato mode crossfade between AtomicJet and ShockPort produced click step {max_step}"
    );
}

#[test]
fn vco_node_cubic_waveforms_chirality_dual_pw_cv_and_native_tz_fm() {
    use cadence_core::dsp::{lin_to_db_f64, VCO_MAX_REACH};
    let sr = 48_000.0;
    let bank: SampleBank = Vec::new();
    let warmup = 4_800usize;
    let measure = 4_800usize;

    // 1. All 8 cubic/Hardy waveforms render finite, non-silent output with zero RT allocations.
    let mut reusable = ModularVoice::new_boxed_inactive(sr);
    for &wf in Waveform::ALL {
        let z = build_zone(
            vec![
                RackModule::new(
                    1,
                    "VCO",
                    ModuleKind::Vco(VcoModule {
                        waveform: wf,
                        reach: 0.9,
                        shape: 0.5,
                        chirality: 0.0,
                        pulse_width: 0.35,
                        pitch_tracking: PitchTracking::Fixed,
                        fixed_hz: 220.0,
                        level: 0.8,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(2, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![PatchCable::new(1, 1, 2, InPort::In, 1.0)],
        );
        let g = compile_zone_graph(&z).unwrap();
        let zspec = spec(&z, &g, sr, 120.0);
        let mut peak = 0.0_f32;
        assert_no_heap_operations(|| {
            reusable.begin_trigger_from_zone(&zspec);
            reusable.finalize_trigger(start(60, None), false, false);
            for _ in 0..2_048 {
                let f = reusable.next_frame(&bank, None);
                assert!(f[0].is_finite() && f[1].is_finite(), "{wf:?}: non-finite");
                assert!(
                    (f[0] - f[1]).abs() < 1e-6,
                    "{wf:?}: chirality=0 must stay centered mono"
                );
                peak = peak.max(f[0].abs());
            }
        });
        assert!(peak > 0.15, "{wf:?}: expected audible output, got {peak}");
    }

    // 2. Chirality (-1, 0, +1) produces constant stereo RMS power, opposite Side polarity,
    //    and smooth continuous updates honoring per-voice VoiceLocks on Reach/Shape/Chirality.
    let measure_chi = |chirality: f32| -> (f64, f64, Vec<f64>) {
        let z = build_zone(
            vec![
                RackModule::new(
                    1,
                    "VCO",
                    ModuleKind::Vco(VcoModule {
                        waveform: Waveform::Szego,
                        reach: 0.88,
                        shape: 0.5,
                        chirality,
                        pitch_tracking: PitchTracking::Fixed,
                        fixed_hz: 250.0,
                        level: 0.75,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(2, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![PatchCable::new(1, 1, 2, InPort::In, 1.0)],
        );
        let g = compile_zone_graph(&z).unwrap();
        let mut v = ModularVoice::new_boxed(&spec(&z, &g, sr, 120.0), start(60, None));
        let mut stereo_e = 0.0_f64;
        let mut side_e = 0.0_f64;
        let mut side_trace = Vec::with_capacity(measure);
        for i in 0..(warmup + measure) {
            let f = v.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
            if i >= warmup {
                let l = f64::from(f[0]);
                let r = f64::from(f[1]);
                let side = 0.5 * (l - r);
                stereo_e += 0.5 * (l * l + r * r);
                side_e += side * side;
                side_trace.push(side);
            }
        }
        (
            (stereo_e / measure as f64).sqrt(),
            (side_e / measure as f64).sqrt(),
            side_trace,
        )
    };
    let (rms_0, side_0, _) = measure_chi(0.0);
    let (rms_pos, side_pos, trace_pos) = measure_chi(1.0);
    let (rms_neg, side_neg, trace_neg) = measure_chi(-1.0);
    assert!(side_0 < 1e-6, "chirality=0 must have zero Side: {side_0}");
    assert!(
        side_pos > 0.1 && side_neg > 0.1,
        "chirality=±1 must generate quadrature Side energy: +{side_pos}, -{side_neg}"
    );
    assert!(
        lin_to_db_f64(rms_pos / rms_0).abs() < 0.1 && lin_to_db_f64(rms_neg / rms_0).abs() < 0.1,
        "chirality must preserve constant stereo RMS: rms_0={rms_0}, rms_pos={rms_pos}, rms_neg={rms_neg}"
    );
    let polarity_err: f64 = trace_pos
        .iter()
        .zip(&trace_neg)
        .map(|(a, b)| (a + b).abs())
        .fold(0.0_f64, f64::max);
    assert!(
        polarity_err < 1e-4,
        "chirality +1 and -1 must invert Side polarity: max err={polarity_err}"
    );

    // Continuous updates smooth Reach/Shape/Chirality and honor per-voice locks.
    let mut z_lock = build_zone(
        vec![
            RackModule::new(
                1,
                "VCO",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::MobiusJet,
                    reach: 0.5,
                    shape: 0.25,
                    chirality: 0.1,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(2, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![PatchCable::new(1, 1, 2, InPort::In, 1.0)],
    );
    let g_lock = compile_zone_graph(&z_lock).unwrap();
    let mut v_lock = ModularVoice::new_boxed_inactive(sr);
    v_lock.begin_trigger_from_zone(&spec(&z_lock, &g_lock, sr, 120.0));
    v_lock.apply_trigger_module_param(
        1,
        ModuleParam::Reach,
        ModuleParam::Reach.normalize(0.82),
        0,
        true,
    );
    v_lock.finalize_trigger(start(60, None), false, false);
    if let ModuleKind::Vco(ref mut vco) = z_lock.modules[0].kind {
        vco.reach = 0.2;
        vco.shape = 0.85;
        vco.chirality = -0.75;
    }
    v_lock.update_continuous_from_zone(&z_lock, 0.0, 0.0, 120.0);
    let ModuleParams::Vco(vco_target) = v_lock.module_target_params(1).unwrap() else {
        panic!("expected Vco target");
    };
    assert!(
        (vco_target.reach - 0.82).abs() < 1e-3,
        "locked Reach must survive continuous update: {}",
        vco_target.reach
    );
    assert!(
        (vco_target.shape - 0.85).abs() < 1e-4 && (vco_target.chirality + 0.75).abs() < 1e-4,
        "unlocked Shape/Chirality must follow continuous update: {vco_target:?}"
    );

    // 3. Dual-purpose PwCv:
    //    - On MobiusPwm: modulates pulse_width (shifts 50% square -> asymmetric PWM with strong H2).
    //    - On non-PWM waveforms (MobiusJet): modulates reach (boosts H2/H3 brightness from low reach).
    let measure_pw_cv = |wf: Waveform, reach: f32, pw: f32, pw_cv_amt: f32| -> (f64, f64, f64) {
        let mut modules = vec![
            RackModule::new(
                1,
                "VCO",
                ModuleKind::Vco(VcoModule {
                    waveform: wf,
                    reach,
                    shape: 0.4,
                    pulse_width: pw,
                    pitch_tracking: PitchTracking::Fixed,
                    fixed_hz: 200.0,
                    level: 0.8,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(2, "Out", ModuleKind::Out(OutModule::default())),
        ];
        let mut cables = vec![PatchCable::new(1, 1, 2, InPort::In, 1.0)];
        if pw_cv_amt.abs() > 0.0 {
            modules.push(RackModule::new(
                3,
                "CvEnv",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::HoldDecay,
                    attack_ms: 0.1,
                    hold_ms: 500.0,
                    decay_ms: 100.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ));
            cables.push(PatchCable::new(2, 3, 1, InPort::PwCv, pw_cv_amt));
        }
        let z = build_zone(modules, cables);
        let g = compile_zone_graph(&z).unwrap();
        let mut v = ModularVoice::new_boxed(&spec(&z, &g, sr, 120.0), start(60, None));
        let mut buf = Vec::with_capacity(measure);
        for i in 0..(warmup + measure) {
            let f = v.next_frame(&bank, None);
            if i >= warmup {
                buf.push(f64::from(f[0]));
            }
        }
        (
            dft_power(&buf, 200.0, sr),
            dft_power(&buf, 400.0, sr),
            dft_power(&buf, 600.0, sr),
        )
    };
    let (pwm0_h1, pwm0_h2, _) = measure_pw_cv(Waveform::MobiusPwm, 0.9, 0.5, 0.0);
    let (pwm1_h1, pwm1_h2, _) = measure_pw_cv(Waveform::MobiusPwm, 0.9, 0.5, 0.30);
    assert!(
        pwm0_h2 < pwm0_h1 * 1e-6 && pwm1_h2 > pwm1_h1 * 0.01 && pwm1_h2 > pwm0_h2 * 1e6,
        "MobiusPwm PwCv must shift duty from symmetric square to asymmetric PWM: base={pwm0_h2}/{pwm0_h1}, mod={pwm1_h2}/{pwm1_h1}"
    );
    let (jet0_h1, _, jet0_h3) = measure_pw_cv(Waveform::MobiusJet, 0.2, 0.5, 0.0);
    let (jet1_h1, _, jet1_h3) = measure_pw_cv(Waveform::MobiusJet, 0.2, 0.5, 0.45);
    let (jet_max_h1, _, jet_max_h3) =
        measure_pw_cv(Waveform::MobiusJet, VCO_MAX_REACH as f32, 0.5, 0.45);
    assert!(
        (jet1_h3 / jet1_h1) > (jet0_h3 / jet0_h1) * 10.0,
        "non-PWM PwCv must boost harmonic reach brightness: base={}/{}, mod={}/{}",
        jet0_h3,
        jet0_h1,
        jet1_h3,
        jet1_h1
    );
    assert!(
        jet_max_h1.is_finite() && jet_max_h3.is_finite() && jet_max_h1 > 1e-3,
        "PwCv above VCO_MAX_REACH must clamp cleanly without instability"
    );

    // 4. Native 1x through-zero linear FM generates symmetric sidebands with zero RT allocations.
    //    Use incommensurate carrier (3000 Hz) and modulator (700 Hz) so lower sidebands do not
    //    fold across 0 Hz onto the k = ±1 sidebands at 2300 Hz and 3700 Hz.
    let fm_zone = build_zone(
        vec![
            RackModule::new(
                1,
                "Carrier",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    pitch_tracking: PitchTracking::Fixed,
                    fixed_hz: 3_000.0,
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
                    fixed_hz: 700.0,
                    level: 1.0,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(3, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 2, 1, InPort::FmIn, 0.35),
            PatchCable::new(2, 1, 3, InPort::In, 1.0),
        ],
    );
    let g_fm = compile_zone_graph(&fm_zone).unwrap();
    let fm_spec = spec(&fm_zone, &g_fm, sr, 120.0);
    let mut fm_buf = Vec::with_capacity(measure);
    assert_no_heap_operations(|| {
        reusable.begin_trigger_from_zone(&fm_spec);
        reusable.finalize_trigger(start(60, None), false, false);
        for i in 0..(warmup + measure) {
            let f = reusable.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
            if i >= warmup {
                fm_buf.push(f64::from(f[0]));
            }
        }
    });
    let sb_lo = dft_power(&fm_buf, 2_300.0, sr);
    let sb_hi = dft_power(&fm_buf, 3_700.0, sr);
    let sym_db = lin_to_db_f64((sb_hi / sb_lo).sqrt()).abs();
    assert!(
        sb_lo > 1e-3 && sb_hi > 1e-3 && sym_db < 0.25,
        "1x TZ-FM sidebands at 2300/3700 Hz must be strong and symmetric: lo={sb_lo}, hi={sb_hi}, sym={sym_db:.3} dB"
    );
}

#[test]
fn env_node_capacitor_additive_stacking_and_continuous_cubic_limit() {
    let sr = 48_000.0;
    let bank: SampleBank = Vec::new();

    // 1. EnvMode::Capacitor + TriggerMode::Additive stacks voltage on rapid TrigIn pulses,
    //    and cubic_limit self-limits stacked peaks cleanly vs linear RC (cubic_limit = 0.0).
    let run_cap_stack = |cubic_limit: f32| -> (f64, f64) {
        let z = build_zone(
            vec![
                RackModule::new(
                    1,
                    "Clock",
                    ModuleKind::Lfo(LfoModule {
                        shape: LfoShape::Square,
                        rate_hz: 20.0, // 50 ms period: triggers at 0 ms and 50 ms
                        retrigger: true,
                        bipolar: true,
                        ..LfoModule::default()
                    }),
                ),
                RackModule::new(
                    2,
                    "CapEnv",
                    ModuleKind::Env(EnvModule {
                        mode: EnvMode::Capacitor,
                        attack_ms: 2.0,
                        hold_ms: 4.0,
                        decay_ms: 700.0,
                        sustain: 0.0,
                        release_ms: 50.0,
                        cubic_limit,
                        trigger_mode: TriggerMode::Additive,
                        ..EnvModule::default()
                    }),
                ),
                RackModule::new(3, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![PatchCable::new(1, 1, 2, InPort::TrigIn, 1.0)],
        );
        let g = compile_zone_graph(&z).unwrap();
        let mut v = ModularVoice::new_boxed(&spec(&z, &g, sr, 120.0), start(60, None));
        let mut peak1 = 0.0_f64;
        let mut peak2 = 0.0_f64;
        // 50 ms = 2400 frames; 1st pulse peaks around frame 200, 2nd pulse peaks around frame 2600.
        for i in 0..3_600 {
            let _ = v.next_frame(&bank, None);
            let (_, lvl) = v.env_stage_and_level(2).unwrap();
            assert!(lvl.is_finite() && lvl >= 0.0);
            if i < 1_200 {
                peak1 = peak1.max(lvl);
            } else if i >= 2_400 {
                peak2 = peak2.max(lvl);
            }
        }
        (peak1, peak2)
    };
    let (lin_p1, lin_p2) = run_cap_stack(0.0);
    let (lim_p1, lim_p2) = run_cap_stack(6.0);
    assert!(
        lin_p1 > 0.7 && lin_p2 > lin_p1 + 0.45 && lin_p2 > 1.2,
        "Additive Capacitor must stack charge above 1.0 on rapid pulses: p1={lin_p1:.4}, p2={lin_p2:.4}"
    );
    assert!(
        lim_p2 > lim_p1 && lim_p2 < lin_p2 - 0.1,
        "cubic_limit must self-limit stacked Capacitor peak: linear={lin_p2:.4}, limited={lim_p2:.4}"
    );

    // 2. Continuous CubicLimit updates smooth live, respect per-voice VoiceLocks,
    //    and TriggerMode::Additive ignores normal note_off while choking on force_release.
    let mut cap_zone = build_zone(
        vec![
            RackModule::new(
                1,
                "VCO",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    level: 0.8,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(2, "VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                3,
                "AmpCap",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Capacitor,
                    attack_ms: 2.0,
                    hold_ms: 4.0,
                    decay_ms: 900.0,
                    cubic_limit: 0.0,
                    trigger_mode: TriggerMode::Additive,
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
    let g_cap = compile_zone_graph(&cap_zone).unwrap();

    // Unlocked continuous update raises CubicLimit -> accelerates discharge compared to locked 0.0!
    let mut v_locked = ModularVoice::new_boxed_inactive(sr);
    v_locked.begin_trigger_from_zone(&spec(&cap_zone, &g_cap, sr, 120.0));
    v_locked.apply_trigger_module_param(
        3,
        ModuleParam::CubicLimit,
        ModuleParam::CubicLimit.normalize(0.0),
        0,
        true,
    );
    v_locked.finalize_trigger(start(60, None), false, false);

    let mut v_live = ModularVoice::new_boxed_inactive(sr);
    v_live.begin_trigger_from_zone(&spec(&cap_zone, &g_cap, sr, 120.0));
    v_live.finalize_trigger(start(60, None), false, false);

    // Charge for 6 ms, then raise zone cubic_limit to 8.0 and push continuous update.
    for _ in 0..288 {
        v_locked.next_frame(&bank, None);
        v_live.next_frame(&bank, None);
    }
    if let ModuleKind::Env(ref mut env) = cap_zone.modules[2].kind {
        env.cubic_limit = 8.0;
    }
    assert_no_heap_operations(|| {
        v_locked.update_continuous_from_zone(&cap_zone, 0.0, 0.0, 120.0);
        v_live.update_continuous_from_zone(&cap_zone, 0.0, 0.0, 120.0);
        for _ in 0..4_800 {
            v_locked.next_frame(&bank, None);
            v_live.next_frame(&bank, None);
        }
    });
    let ModuleParams::Env(locked_t) = v_locked.module_target_params(3).unwrap() else {
        panic!("expected Env");
    };
    let ModuleParams::Env(live_t) = v_live.module_target_params(3).unwrap() else {
        panic!("expected Env");
    };
    assert!(locked_t.cubic_limit < 1e-4, "locked CubicLimit drifted");
    assert!(
        (live_t.cubic_limit - 8.0).abs() < 1e-4,
        "unlocked CubicLimit missed continuous update"
    );
    let lvl_locked = v_locked.env_stage_and_level(3).unwrap().1;
    let lvl_live = v_live.env_stage_and_level(3).unwrap().1;
    assert!(
        lvl_live < lvl_locked * 0.75,
        "continuous CubicLimit=8.0 must discharge capacitor faster than locked 0.0: live={lvl_live:.4}, locked={lvl_locked:.4}"
    );

    // Normal note_off preserves TriggerMode::Additive natural decay; force_release chokes cleanly!
    v_locked.note_off();
    for _ in 0..480 {
        v_locked.next_frame(&bank, None);
    }
    assert!(
        v_locked.is_active() && v_locked.env_stage_and_level(3).unwrap().1 > 0.3,
        "normal note_off must honor TriggerMode::Additive one-shot decay"
    );
    v_locked.force_release();
    for _ in 0..1_000 {
        v_locked.next_frame(&bank, None);
    }
    assert_eq!(
        v_locked.env_stage_and_level(3).unwrap().0,
        AdsrStage::Done,
        "force_release must choke TriggerMode::Additive envelope within ~20 ms"
    );
    for _ in 0..24_000 {
        v_locked.next_frame(&bank, None);
        if !v_locked.is_active() {
            break;
        }
    }
    assert!(
        !v_locked.is_active(),
        "force_release must retire TriggerMode::Additive voice cleanly after Out DC settle"
    );
}

#[test]
fn aero_node_flue_jet_and_brass_reed_oscillate_retire_on_bore_tail_and_zero_alloc() {
    let sr = 48_000.0;
    let bank: SampleBank = Vec::new();

    // 1. Both FlueJet and BrassReed in `[Aero -> Out]` (zero Env/Vca) self-oscillate at the
    //    tracked fundamental (A4 = 440 Hz, MIDI 69), bypass Out's 15 ms fallback gate chop on
    //    note_off so the physical bore tail rings out, retire automatically on `is_quiet()`,
    //    and run with zero RT heap allocations.
    let mut reusable = ModularVoice::new_boxed_inactive(sr);
    for &mode in AeroMode::ALL {
        let (radiation, atoms) = match mode {
            AeroMode::FlueJet => (0.15, 6),
            AeroMode::BrassReed => (0.5, 10),
        };
        let z = build_zone(
            vec![
                RackModule::new(
                    1,
                    "Aero",
                    ModuleKind::Aero(AeroModule {
                        mode,
                        geometry: BodyGeometry::Harmonic,
                        atoms,
                        pitch_tracking: PitchTracking::Chromatic,
                        drive: 0.8,
                        embouchure: 0.6,
                        p_beat_pa: 1_500.0,
                        brightness: 0.55,
                        absorption: 0.045,
                        radiation,
                        kerr_semitones: 0.0,
                        triad_amount: 0.2,
                        spread: 0.4,
                        level: 0.9,
                        ..AeroModule::default()
                    }),
                ),
                RackModule::new(2, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![PatchCable::new(1, 1, 2, InPort::In, 1.0)],
        );
        let g = compile_zone_graph(&z).unwrap();
        assert_eq!(g.aero_slots.len(), 1);
        assert!(g.lifetime_envs.is_empty());

        let zspec = spec(&z, &g, sr, 120.0);
        let mut osc_buf = Vec::with_capacity(8_000);
        let mut stereo_diff = 0.0_f64;
        let mut post_40ms_peak = 0.0_f32;
        let mut retired_after_release = None;

        assert_no_heap_operations(|| {
            reusable.begin_trigger_from_zone(&zspec);
            reusable.finalize_trigger(start(69, None), false, false);

            // Blow for 350 ms (16,800 frames) and capture steady oscillation window.
            for i in 0..16_800 {
                let f = reusable.next_frame(&bank, None);
                assert!(
                    f[0].is_finite() && f[1].is_finite(),
                    "{mode:?}: non-finite held output"
                );
                if i >= 8_800 {
                    osc_buf.push(f64::from(f[0]));
                    stereo_diff += f64::from((f[0] - f[1]).abs());
                }
            }
            assert!(
                reusable.is_active(),
                "{mode:?}: voice must stay active while held"
            );

            // Release blowing gate: Out's 15 ms fallback gate must be bypassed so bore rings > 40 ms!
            reusable.note_off();
            for step in 0..(sr as usize * 8) {
                if !reusable.is_active() {
                    retired_after_release = Some(step);
                    break;
                }
                let f = reusable.next_frame(&bank, None);
                assert!(
                    f[0].is_finite() && f[1].is_finite(),
                    "{mode:?}: non-finite tail output"
                );
                if (1_920..4_800).contains(&step) {
                    post_40ms_peak = post_40ms_peak.max(f[0].abs()).max(f[1].abs());
                }
            }
        });

        // Allow realistic physical jet/reed pitch pulling (±15 Hz around 440 Hz).
        let p_fund = (425..=455)
            .step_by(3)
            .map(|hz| dft_power(&osc_buf, f64::from(hz), sr))
            .fold(0.0_f64, f64::max);
        let p_off = dft_power(&osc_buf, 311.0, sr);
        assert!(
            p_fund > 1e-4 && p_fund > p_off * 10.0,
            "{mode:?}: expected strong ~440 Hz oscillation (fund={p_fund:.3e} vs off={p_off:.3e})"
        );
        assert!(
            stereo_diff > 1.0,
            "{mode:?}: spread=0.4 must produce quadrature stereo image (diff={stereo_diff})"
        );
        assert!(
            post_40ms_peak > 1e-3,
            "{mode:?}: natural bore tail must outlive 15 ms fallback gate past 40 ms (peak={post_40ms_peak})"
        );
        assert!(
            retired_after_release.is_some(),
            "{mode:?}: voice failed to retire automatically after bore tail decayed"
        );
    }

    // 2. TrigIn gates wind excitation, Cv modulates drive, PitchCv shifts pitch,
    //    and choke() / force_release() terminate smoothly with zero RT allocations.
    let trig_zone = build_zone(
        vec![
            RackModule::new(
                1,
                "GateEnv",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::HoldDecay,
                    attack_ms: 0.1,
                    hold_ms: 120.0,
                    decay_ms: 20.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                2,
                "DriveEnv",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::HoldDecay,
                    attack_ms: 0.1,
                    hold_ms: 200.0,
                    decay_ms: 20.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                3,
                "Reed",
                ModuleKind::Aero(AeroModule {
                    mode: AeroMode::BrassReed,
                    drive: 0.35,
                    radiation: 0.5,
                    level: 0.9,
                    ..AeroModule::default()
                }),
            ),
            RackModule::new(4, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 1, 3, InPort::TrigIn, 1.0),
            PatchCable::new(2, 2, 3, InPort::Cv, 0.5),
            PatchCable::new(3, 2, 3, InPort::PitchCv, 12.0), // +1 octave -> A3 (220) -> A4 (440)
            PatchCable::new(4, 3, 4, InPort::In, 1.0),
        ],
    );
    let g_trig = compile_zone_graph(&trig_zone).unwrap();
    let tspec = spec(&trig_zone, &g_trig, sr, 120.0);
    let mut trig_buf = Vec::with_capacity(4_000);
    assert_no_heap_operations(|| {
        reusable.begin_trigger_from_zone(&tspec);
        reusable.finalize_trigger(start(57, None), false, false);
        for i in 0..5_500 {
            let f = reusable.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
            if i >= 1_500 {
                trig_buf.push(f64::from(f[0]));
            }
        }
        // Choke must fade smoothly to near-silence within ~20 ms and retire cleanly (zero allocs).
        reusable.choke(sr);
        let mut post_choke_peak = 0.0_f32;
        for step in 0..24_000 {
            let f = reusable.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
            if step >= 1_000 {
                post_choke_peak = post_choke_peak.max(f[0].abs()).max(f[1].abs());
            }
            if !reusable.is_active() {
                break;
            }
        }
        assert!(
            post_choke_peak < 1e-3,
            "choke() must attenuate Aero output within ~20 ms (post_choke_peak={post_choke_peak})"
        );
    });
    let p440 = dft_power(&trig_buf, 440.0, sr);
    let p220 = dft_power(&trig_buf, 220.0, sr);
    assert!(
        p440 > p220 * 5.0 && p440 > 1e-4,
        "Aero PitchCv (+12 st) + TrigIn + Cv must oscillate at 440 Hz (p440={p440:.3e} vs p220={p220:.3e})"
    );
    assert!(
        !reusable.is_active(),
        "choke() must silence and retire Aero voice promptly"
    );

    // 3. Continuous parameter updates on Aero honor per-voice VoiceLocks and force_release retires promptly.
    let mut lock_zone = build_zone(
        vec![
            RackModule::new(1, "Aero", ModuleKind::Aero(AeroModule::default())),
            RackModule::new(2, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![PatchCable::new(1, 1, 2, InPort::In, 1.0)],
    );
    let g_lock = compile_zone_graph(&lock_zone).unwrap();
    let lspec = spec(&lock_zone, &g_lock, sr, 120.0);
    assert_no_heap_operations(|| {
        reusable.begin_trigger_from_zone(&lspec);
        reusable.apply_trigger_module_param(
            1,
            ModuleParam::Drive,
            ModuleParam::Drive.normalize(0.85),
            0,
            true,
        );
        reusable.finalize_trigger(start(60, None), false, false);
        for _ in 0..2_048 {
            let _ = reusable.next_frame(&bank, None);
        }
    });
    if let ModuleKind::Aero(ref mut a) = lock_zone.modules[0].kind {
        a.drive = 0.15;
        a.embouchure = 0.9;
        a.radiation = 0.0;
    }
    assert_no_heap_operations(|| {
        reusable.update_continuous_from_zone(&lock_zone, 0.0, 0.0, 120.0);
        reusable.force_release();
        for _ in 0..24_000 {
            let f = reusable.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
            if !reusable.is_active() {
                break;
            }
        }
    });
    let ModuleParams::Aero(aero_target) = reusable.module_target_params(1).unwrap() else {
        panic!("expected Aero target");
    };
    assert!(
        (aero_target.drive - 0.85).abs() < 1e-4,
        "locked Aero Drive must survive continuous update: {}",
        aero_target.drive
    );
    assert!(
        (aero_target.embouchure - 0.9).abs() < 1e-4,
        "unlocked Aero Embouchure must follow continuous update: {}",
        aero_target.embouchure
    );
    assert!(
        !reusable.is_active(),
        "force_release must retire Aero voice promptly even when radiation=0.0"
    );
}

#[test]
fn modal_self_struck_membrane_kerr_kick_rings_retires_and_chokes_cleanly() {
    let sr = 48_000.0;
    let bank: SampleBank = Vec::new();

    let make_kick_zone = |kerr_semitones: f32| {
        build_zone(
            vec![
                RackModule::new(
                    1,
                    "Kick",
                    ModuleKind::Modal(ModalModule {
                        geometry: BodyGeometry::Membrane,
                        atoms: 12,
                        pitch_tracking: PitchTracking::Fixed,
                        fixed_hz: 65.0,
                        absorption: 0.04,
                        radiation: 0.65,
                        brightness: 0.35,
                        kerr_semitones,
                        spread: 0.1,
                        strike_drive: 1.4,
                        strike_hardness: 0.55,
                        mix: 1.0,
                        ..ModalModule::default()
                    }),
                ),
                RackModule::new(2, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![PatchCable::new(1, 1, 2, InPort::In, 1.0)],
        )
    };

    let measure_kick = |kerr_semitones: f32| -> (f64, f32, Option<usize>) {
        let z = make_kick_zone(kerr_semitones);
        let g = compile_zone_graph(&z).unwrap();
        assert_eq!(g.modal_slots.len(), 1);
        assert!(g.lifetime_envs.is_empty());

        let mut v = ModularVoice::new_boxed_inactive(sr);
        let zspec = spec(&z, &g, sr, 120.0);
        let mut onset_buf = Vec::with_capacity(4_800);
        let mut post_gate_peak = 0.0_f32;
        let mut retired_at = None;

        assert_no_heap_operations(|| {
            v.begin_trigger_from_zone(&zspec);
            // Short 10 ms gate (480 frames): self-struck kick must ring naturally past note_off!
            v.finalize_trigger(start(36, Some(480)), false, false);
            assert!(v.self_gated_only);
            for step in 0..(sr as usize * 3) {
                if !v.is_active() {
                    retired_at = Some(step);
                    break;
                }
                let f = v.next_frame(&bank, None);
                assert!(f[0].is_finite() && f[1].is_finite());
                if (240..5_040).contains(&step) {
                    onset_buf.push(f64::from(f[0]));
                }
                if (1_920..5_760).contains(&step) {
                    post_gate_peak = post_gate_peak.max(f[0].abs()).max(f[1].abs());
                }
            }
        });

        let peak_hz = (30..=110)
            .max_by(|&a, &b| {
                dft_power(&onset_buf, f64::from(a), sr).total_cmp(&dft_power(
                    &onset_buf,
                    f64::from(b),
                    sr,
                ))
            })
            .unwrap() as f64;
        (peak_hz, post_gate_peak, retired_at)
    };

    let (hz_neg, peak_neg, retired_neg) = measure_kick(-9.0);
    let (hz_zero, peak_zero, retired_zero) = measure_kick(0.0);
    let (hz_pos, peak_pos, retired_pos) = measure_kick(9.0);

    assert!(
        peak_neg > 0.02 && peak_zero > 0.02 && peak_pos > 0.02,
        "self-struck Membrane kicks must ring audibly past 10 ms note_off (neg={peak_neg}, zero={peak_zero}, pos={peak_pos})"
    );
    assert!(
        hz_neg < hz_zero - 2.0 && hz_pos > hz_zero + 2.0,
        "Kerr dispersion must bend kick onset pitch downward for negative Kerr and upward for positive Kerr: neg={hz_neg} Hz, zero={hz_zero} Hz, pos={hz_pos} Hz"
    );
    assert!(
        retired_neg.is_some() && retired_zero.is_some() && retired_pos.is_some(),
        "self-struck kicks must retire automatically on is_quiet(): neg={retired_neg:?}, zero={retired_zero:?}, pos={retired_pos:?}"
    );

    // 2. Drum choke (~3.5 ms CHOKE_MS) silences a ringing Membrane kick smoothly with zero allocs.
    let choke_zone = make_kick_zone(-9.0);
    let g_choke = compile_zone_graph(&choke_zone).unwrap();
    let modal_slot = g_choke.slot_for_module(1).unwrap();
    let mut v_choke = ModularVoice::new_boxed_inactive(sr);
    let cspec = spec(&choke_zone, &g_choke, sr, 120.0);
    assert_no_heap_operations(|| {
        v_choke.begin_trigger_from_zone(&cspec);
        v_choke.finalize_trigger(start(36, None), false, false);
        for _ in 0..480 {
            let f = v_choke.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
        }
        assert!(v_choke.current_level() > 0.02);

        v_choke.choke(sr);
        // After 10 ms (480 frames, > 2.8 * CHOKE_MS), the modal choke fade must reach exact silence.
        for _ in 0..480 {
            let f = v_choke.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
        }
        match v_choke.nodes[modal_slot] {
            ModuleNode::Modal(ref modal) => {
                assert!(
                    modal.is_quiet(&v_choke.modal_banks),
                    "ModalNode choke fade must complete within ~10 ms"
                );
                assert_eq!(modal.current_level(&v_choke.modal_banks), 0.0);
            }
            _ => panic!("expected ModalNode"),
        }
        // Voice retires cleanly after Out's 5 Hz DcBlocker (tau ≈ 32 ms) settles from the choked half-cycle.
        let mut late_peak = 0.0_f32;
        for step in 0..24_000 {
            if !v_choke.is_active() {
                break;
            }
            let f = v_choke.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
            if step >= 6_000 {
                late_peak = late_peak.max(f[0].abs()).max(f[1].abs());
            }
        }
        assert!(
            late_peak < 1e-3,
            "choked kick must decay to near-silence once DcBlocker settles (late_peak={late_peak})"
        );
    });
    assert!(
        !v_choke.is_active(),
        "choked self-struck Modal voice must retire cleanly"
    );
}

#[test]
fn modal_self_struck_plate_cymbal_with_manley_triads_produces_stereo_and_retires() {
    let sr = 48_000.0;
    let bank: SampleBank = Vec::new();
    let cymbal_zone = build_zone(
        vec![
            RackModule::new(
                1,
                "Cymbal",
                ModuleKind::Modal(ModalModule {
                    geometry: BodyGeometry::Plate,
                    atoms: 40,
                    pitch_tracking: PitchTracking::Fixed,
                    fixed_hz: 240.0,
                    absorption: 0.01,
                    radiation: 0.22,
                    brightness: 0.85,
                    headroom: 0.6,
                    pair_topology: PairTopology::Swarm,
                    coupling: 0.35,
                    triad_amount: 0.7,
                    triad_axis: TriadAxis::Manley,
                    spread: 0.85,
                    strike_drive: 1.8,
                    strike_hardness: 0.85,
                    mix: 1.0,
                    ..ModalModule::default()
                }),
            ),
            RackModule::new(2, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![PatchCable::new(1, 1, 2, InPort::In, 1.0)],
    );
    let g = compile_zone_graph(&cymbal_zone).unwrap();
    let mut voice = ModularVoice::new_boxed_inactive(sr);
    let zspec = spec(&cymbal_zone, &g, sr, 120.0);

    let mut peak = 0.0_f32;
    let mut post_gate_peak = 0.0_f32;
    let mut stereo_diff = 0.0_f64;
    let mut retired_at = None;

    assert_no_heap_operations(|| {
        voice.begin_trigger_from_zone(&zspec);
        voice.finalize_trigger(start(49, Some(480)), false, false);
        for step in 0..(sr as usize * 8) {
            if !voice.is_active() {
                retired_at = Some(step);
                break;
            }
            let f = voice.next_frame(&bank, None);
            assert!(
                f[0].is_finite()
                    && f[1].is_finite()
                    && f[0].abs() < VOICE_GUARD_CEILING as f32
                    && f[1].abs() < VOICE_GUARD_CEILING as f32
            );
            peak = peak.max(f[0].abs()).max(f[1].abs());
            if (2_400..9_600).contains(&step) {
                post_gate_peak = post_gate_peak.max(f[0].abs()).max(f[1].abs());
                stereo_diff += f64::from((f[0] - f[1]).abs());
            }
        }
    });

    assert!(
        peak > 0.05 && post_gate_peak > 0.005,
        "40-mode Manley Plate cymbal must produce audible ringout past 10 ms gate (peak={peak}, post_gate={post_gate_peak})"
    );
    assert!(
        stereo_diff > 1.0,
        "Plate cymbal with spread=0.85 must produce wide stereo image (stereo_diff={stereo_diff})"
    );
    assert!(
        retired_at.is_some(),
        "self-struck 40-mode Plate cymbal must retire automatically on is_quiet()"
    );
}

#[test]
fn modal_patched_ms_quadrature_drive_trig_in_cv_pitch_glide_sr_change_and_zero_alloc() {
    let sr = 48_000.0;
    let bank: SampleBank = Vec::new();
    let mut reusable = ModularVoice::new_boxed_inactive(sr);

    // 1. Patched M/S quadrature Vco(Szego, chi=+1.0) -> Modal.In vs mono chi=0.0 and counter-rotating chi=-1.0.
    let mut run_szego_drive = |chirality: f32| -> (Vec<f64>, f64) {
        let z = build_zone(
            vec![
                RackModule::new(
                    1,
                    "Szego",
                    ModuleKind::Vco(VcoModule {
                        waveform: Waveform::Szego,
                        reach: 0.65,
                        shape: 0.5,
                        chirality,
                        pitch_tracking: PitchTracking::Fixed,
                        fixed_hz: 220.0,
                        level: 0.6,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(
                    2,
                    "Body",
                    ModuleKind::Modal(ModalModule {
                        geometry: BodyGeometry::Harmonic,
                        atoms: 16,
                        pitch_tracking: PitchTracking::Fixed,
                        fixed_hz: 220.0,
                        pair_topology: PairTopology::Swarm,
                        coupling: 0.0,
                        absorption: 0.15,
                        radiation: 0.25,
                        brightness: 0.65,
                        headroom: 2.0,
                        spread: 0.45,
                        strike_drive: 0.0,
                        mix: 1.0,
                        ..ModalModule::default()
                    }),
                ),
                RackModule::new(3, "Out", ModuleKind::Out(OutModule::default())),
            ],
            vec![
                PatchCable::new(1, 1, 2, InPort::In, 1.0),
                PatchCable::new(2, 2, 3, InPort::In, 1.0),
            ],
        );
        let g = compile_zone_graph(&z).unwrap();
        let zspec = spec(&z, &g, sr, 120.0);
        let mut buf = Vec::with_capacity(4_800);
        let mut sq = 0.0_f64;
        assert_no_heap_operations(|| {
            reusable.begin_trigger_from_zone(&zspec);
            reusable.finalize_trigger(start(57, None), false, false);
            for i in 0..7_200 {
                let f = reusable.next_frame(&bank, None);
                assert!(f[0].is_finite() && f[1].is_finite());
                if i >= 2_400 {
                    let s = f64::from(f[0]);
                    sq += s * s;
                    buf.push(s);
                }
            }
        });
        (buf, (sq / 4_800.0).sqrt())
    };
    let (mono_buf, mono_rms) = run_szego_drive(0.0);
    let (quad_pos_buf, quad_pos_rms) = run_szego_drive(1.0);
    let (_, quad_neg_rms) = run_szego_drive(-1.0);
    let quad_diff = (mono_buf
        .iter()
        .zip(&quad_pos_buf)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f64>()
        / mono_buf.len() as f64)
        .sqrt();
    assert!(
        quad_pos_rms > mono_rms * 2.0 && quad_pos_rms > quad_neg_rms * 1.4 && quad_diff > 0.02,
        "M/S quadrature Vco(Szego, chi=+1) -> Modal.In must drive stronger co-rotating resonance than mono chi=0 and counter-rotating chi=-1: pos={quad_pos_rms:.4}, mono={mono_rms:.4}, neg={quad_neg_rms:.4}, l2_diff={quad_diff:.4}"
    );

    // 2. TrigIn rising-edge retrigger on self-struck Modal clocked by a 10 Hz Square LFO.
    let trig_zone = build_zone(
        vec![
            RackModule::new(
                1,
                "Clock",
                ModuleKind::Lfo(LfoModule {
                    shape: LfoShape::Square,
                    rate_hz: 10.0, // 100 ms period = 4800 frames at 48 kHz
                    retrigger: true,
                    bipolar: true,
                    ..LfoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "Bell",
                ModuleKind::Modal(ModalModule {
                    geometry: BodyGeometry::Bar,
                    atoms: 12,
                    pitch_tracking: PitchTracking::Fixed,
                    fixed_hz: 440.0,
                    radiation: 0.95,
                    absorption: 0.06,
                    strike_drive: 1.2,
                    strike_hardness: 0.75,
                    mix: 1.0,
                    ..ModalModule::default()
                }),
            ),
            RackModule::new(3, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 1, 2, InPort::TrigIn, 1.0),
            PatchCable::new(2, 2, 3, InPort::In, 1.0),
        ],
    );
    let g_trig = compile_zone_graph(&trig_zone).unwrap();
    let tspec = spec(&trig_zone, &g_trig, sr, 120.0);
    let mut trig_env = Vec::with_capacity(6_000);
    assert_no_heap_operations(|| {
        reusable.begin_trigger_from_zone(&tspec);
        reusable.finalize_trigger(start(69, None), false, false);
        for _ in 0..6_000 {
            let f = reusable.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
            trig_env.push(f[0].abs().max(f[1].abs()));
        }
    });
    let burst1 = trig_env[10..600].iter().copied().fold(0.0_f32, f32::max);
    let pre_retrig = trig_env[3_600..4_750]
        .iter()
        .copied()
        .fold(0.0_f32, f32::max);
    let burst2 = trig_env[4_810..5_400]
        .iter()
        .copied()
        .fold(0.0_f32, f32::max);
    assert!(
        burst1 > 0.05 && pre_retrig < burst1 * 0.25 && burst2 > pre_retrig * 3.0,
        "TrigIn rising edge must re-strike Modal cleanly: burst1={burst1}, pre_retrig={pre_retrig}, burst2={burst2}"
    );

    // 3. Cv continuously modulates Modal brightness (boosting HF spectral tilt vs unmodulated dark base).
    let mut measure_tilt = |patch_cv: bool| -> f64 {
        let mut modules = vec![
            RackModule::new(
                1,
                "Body",
                ModuleKind::Modal(ModalModule {
                    geometry: BodyGeometry::Harmonic,
                    atoms: 20,
                    pitch_tracking: PitchTracking::Fixed,
                    fixed_hz: 220.0,
                    brightness: 0.05,
                    radiation: 0.4,
                    strike_drive: 1.2,
                    strike_hardness: 0.85,
                    mix: 1.0,
                    ..ModalModule::default()
                }),
            ),
            RackModule::new(2, "Out", ModuleKind::Out(OutModule::default())),
        ];
        let mut cables = vec![PatchCable::new(1, 1, 2, InPort::In, 1.0)];
        if patch_cv {
            modules.push(RackModule::new(
                3,
                "BrightEnv",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::HoldDecay,
                    attack_ms: 0.1,
                    hold_ms: 250.0,
                    decay_ms: 50.0,
                    trigger_mode: TriggerMode::OneShot,
                    ..EnvModule::default()
                }),
            ));
            cables.push(PatchCable::new(2, 3, 1, InPort::Cv, 0.9));
        }
        let z = build_zone(modules, cables);
        let g = compile_zone_graph(&z).unwrap();
        let zspec = spec(&z, &g, sr, 120.0);
        let (mut energy, mut hf_diff, mut prev) = (0.0_f64, 0.0_f64, 0.0_f64);
        assert_no_heap_operations(|| {
            reusable.begin_trigger_from_zone(&zspec);
            reusable.finalize_trigger(start(57, None), false, false);
            for i in 0..4_800 {
                let s = f64::from(reusable.next_frame(&bank, None)[0]);
                if i >= 240 {
                    energy += s * s;
                    hf_diff += (s - prev).powi(2);
                }
                prev = s;
            }
        });
        hf_diff / energy.max(1e-12)
    };
    let dark_tilt = measure_tilt(false);
    let bright_tilt = measure_tilt(true);
    assert!(
        bright_tilt > dark_tilt * 3.0,
        "Modal.Cv (+0.9) must audibly brighten modal spectrum: dark={dark_tilt:.4}, bright={bright_tilt:.4}"
    );

    // 4. Legato Pitch glide uses zero-trig rational Cayley `update_gliding_freq` without re-striking
    //    or calling `configure()`, and sample-rate change preserves ringing modal state with zero allocs!
    let mut glide_zone = build_zone(
        vec![
            RackModule::new(
                1,
                "GlideModal",
                ModuleKind::Modal(ModalModule {
                    geometry: BodyGeometry::Harmonic,
                    atoms: 12,
                    pitch_tracking: PitchTracking::Chromatic,
                    radiation: 0.25,
                    absorption: 0.03,
                    brightness: 0.5,
                    strike_drive: 1.2,
                    strike_hardness: 0.65,
                    mix: 1.0,
                    ..ModalModule::default()
                }),
            ),
            RackModule::new(2, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![PatchCable::new(1, 1, 2, InPort::In, 1.0)],
    );
    glide_zone.voice.voice_mode = VoiceMode::Mono;
    glide_zone.voice.legato = true;
    glide_zone.voice.glide_ms = 50.0;
    let g_glide = compile_zone_graph(&glide_zone).unwrap();
    let modal_slot = g_glide.slot_for_module(1).unwrap();
    let gspec_48 = spec(&glide_zone, &g_glide, 48_000.0, 120.0);
    let gspec_96 = spec(&glide_zone, &g_glide, 96_000.0, 120.0);

    let mut glided_buf = Vec::with_capacity(4_800);
    assert_no_heap_operations(|| {
        reusable.begin_trigger_from_zone(&gspec_48);
        reusable.finalize_trigger(start(57, None), false, false); // A3 = 220 Hz
        for _ in 0..960 {
            let f = reusable.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
        }
        // Legato retarget to A4 = 440 Hz: preserves ringing modes and glides via update_gliding_freq!
        reusable.retarget_from_zone(&gspec_48, start(69, None), true);
        for i in 0..9_600 {
            let f = reusable.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
            if i >= 4_800 {
                glided_buf.push(f64::from(f[0]));
            }
        }
    });
    match reusable.nodes[modal_slot] {
        ModuleNode::Modal(ref modal) => {
            assert_eq!(
                modal.configure_count(),
                1,
                "pitch-only legato glide must not trigger full bank.configure()"
            );
            assert!(
                modal.glide_update_count() > 20,
                "pitch-only legato glide must take update_gliding_freq() fast path (count={})",
                modal.glide_update_count()
            );
            assert!(
                (modal.applied_f0_hz() - 440.0).abs() < 5.0,
                "glided f0_hz should converge near 440 Hz, got {}",
                modal.applied_f0_hz()
            );
        }
        _ => panic!("expected ModalNode"),
    }
    let p440 = dft_power(&glided_buf, 440.0, 48_000.0);
    let p220 = dft_power(&glided_buf, 220.0, 48_000.0);
    assert!(
        p440 > p220 * 5.0 && p440 > 1e-6,
        "glided modal tail must ring at 440 Hz without re-striking (p440={p440:.3e}, p220={p220:.3e})"
    );

    // Patched PitchCv sweep (`Env -> Modal.PitchCv`) also takes `update_gliding_freq` with zero configure() thrash!
    let pcv_zone = build_zone(
        vec![
            RackModule::new(
                1,
                "PitchEnv",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 40.0,
                    decay_ms: 100.0,
                    sustain: 1.0,
                    release_ms: 40.0,
                    ..EnvModule::default()
                }),
            ),
            RackModule::new(
                2,
                "Body",
                ModuleKind::Modal(ModalModule {
                    geometry: BodyGeometry::Harmonic,
                    atoms: 10,
                    pitch_tracking: PitchTracking::Fixed,
                    fixed_hz: 220.0,
                    radiation: 0.3,
                    strike_drive: 1.2,
                    mix: 1.0,
                    ..ModalModule::default()
                }),
            ),
            RackModule::new(3, "Out", ModuleKind::Out(OutModule::default())),
        ],
        vec![
            PatchCable::new(1, 1, 2, InPort::PitchCv, 12.0), // sweeps +0 -> +12 st (220 -> 440 Hz)
            PatchCable::new(2, 2, 3, InPort::In, 1.0),
        ],
    );
    let g_pcv = compile_zone_graph(&pcv_zone).unwrap();
    let pcv_modal_slot = g_pcv.slot_for_module(2).unwrap();
    let pcv_spec = spec(&pcv_zone, &g_pcv, sr, 120.0);
    assert_no_heap_operations(|| {
        reusable.begin_trigger_from_zone(&pcv_spec);
        reusable.finalize_trigger(start(57, None), false, false);
        for _ in 0..3_200 {
            let f = reusable.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
        }
    });
    match reusable.nodes[pcv_modal_slot] {
        ModuleNode::Modal(ref modal) => {
            assert_eq!(
                modal.configure_count(),
                1,
                "PitchCv sweep must not trigger full bank.configure()"
            );
            assert!(
                modal.glide_update_count() > 20,
                "PitchCv sweep must use update_gliding_freq() fast path (count={})",
                modal.glide_update_count()
            );
            assert!(
                (modal.applied_f0_hz() - 440.0).abs() < 2.0,
                "PitchCv +12 st should settle at 440 Hz, got {}",
                modal.applied_f0_hz()
            );
        }
        _ => panic!("expected ModalNode"),
    }

    // Re-trigger glide_zone at 48 kHz and test sample-rate switch during legato retarget (48 kHz -> 96 kHz)!
    assert_no_heap_operations(|| {
        reusable.begin_trigger_from_zone(&gspec_48);
        reusable.finalize_trigger(start(69, None), false, false);
        for _ in 0..512 {
            let f = reusable.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
        }
    });

    // Sample-rate switch during legato retarget (48 kHz -> 96 kHz): zero heap allocs & preserves ringing energy!
    let bidx = match reusable.nodes[modal_slot] {
        ModuleNode::Modal(ref modal) => modal.bank_idx(),
        _ => panic!("expected ModalNode"),
    };
    let energy_before_sr = reusable.modal_banks[bidx].bank.energy();
    assert!(energy_before_sr > 1e-6);
    assert_no_heap_operations(|| {
        reusable.retarget_from_zone(&gspec_96, start(69, None), true);
        assert_eq!(reusable.modal_banks[bidx].bank.sample_rate(), 96_000.0);
        assert!(
            reusable.modal_banks[bidx].bank.energy() > 1e-6,
            "sample-rate update must preserve ringing modal state"
        );
        for _ in 0..512 {
            let f = reusable.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
        }
    });

    // 5. Continuous updates honor per-voice VoiceLocks and preserve OnTrigger structural params.
    let mut lock_zone = glide_zone.clone();
    let lspec = spec(&lock_zone, &g_glide, sr, 120.0);
    assert_no_heap_operations(|| {
        reusable.begin_trigger_from_zone(&lspec);
        reusable.apply_trigger_module_param(
            1,
            ModuleParam::Kerr,
            ModuleParam::Kerr.normalize(6.0),
            0,
            true,
        );
        reusable.finalize_trigger(start(60, None), false, false);
    });
    if let ModuleKind::Modal(ref mut m) = lock_zone.modules[0].kind {
        m.geometry = BodyGeometry::Plate;
        m.pair_topology = PairTopology::Swarm;
        m.triad_axis = TriadAxis::Manley;
        m.kerr_semitones = -9.0;
        m.coupling = 0.75;
        m.triad_amount = 0.6;
        m.spread = 0.9;
        m.mix = 0.5;
    }
    assert_no_heap_operations(|| {
        reusable.update_continuous_from_zone(&lock_zone, 0.0, 0.0, 120.0);
        for _ in 0..512 {
            let f = reusable.next_frame(&bank, None);
            assert!(f[0].is_finite() && f[1].is_finite());
        }
    });
    let ModuleParams::Modal(target_after) = reusable.module_target_params(1).unwrap() else {
        panic!("expected Modal target");
    };
    assert_eq!(
        target_after.geometry,
        BodyGeometry::Harmonic,
        "OnTrigger Geometry must not mutate mid-voice"
    );
    assert_eq!(
        target_after.pair_topology,
        PairTopology::Star,
        "OnTrigger PairTopology must not mutate mid-voice"
    );
    assert_eq!(
        target_after.triad_axis,
        TriadAxis::Sum,
        "OnTrigger TriadAxis must not mutate mid-voice"
    );
    assert!(
        (target_after.kerr_semitones - 6.0).abs() < 1e-4,
        "locked Modal Kerr must survive continuous update: {}",
        target_after.kerr_semitones
    );
    assert!(
        (target_after.coupling - 0.75).abs() < 1e-4
            && (target_after.triad_amount - 0.6).abs() < 1e-4
            && (target_after.spread - 0.9).abs() < 1e-4
            && (target_after.mix - 0.5).abs() < 1e-4,
        "unlocked continuous Modal params must follow updates: {target_after:?}"
    );
}
