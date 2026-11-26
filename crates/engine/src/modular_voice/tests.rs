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

#[test]
fn voice_retirement_ignores_upstream_stateful_nodes_and_preserves_post_vca_tails() {
    let bank: SampleBank = Vec::new();

    // 1. Subtractive voice with Filter + Resonator + Drive upstream of an envelope-gated VCA:
    //    VCO keeps driving upstream modules forever, yet voice must retire promptly after Env ends.
    let upstream_zone = build_zone(
        vec![
            RackModule::new(1, "Osc", ModuleKind::Vco(VcoModule::default())),
            RackModule::new(
                2,
                "LPF",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::LowPass,
                    cutoff_hz: 2400.0,
                    resonance: 0.4,
                    drive: 0.25,
                    ..FilterModule::default()
                }),
            ),
            RackModule::new(
                3,
                "Res",
                ModuleKind::Resonator(ResonatorModule {
                    mode: ResonatorMode::Bar,
                    decay_ms: 4000.0,
                    mix: 0.35,
                    ..ResonatorModule::default()
                }),
            ),
            RackModule::new(
                4,
                "Drv",
                ModuleKind::Drive(DriveModule {
                    mode: DriveMode::Tape,
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
        // Must retire well under 250 ms — never waiting for the 2 s / 12 s timeout!
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

    // 2. Post-VCA Resonator tail MUST ring out naturally long after the short strike envelope ends.
    let sr = 48_000.0;
    let post_res_zone = build_zone(
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
                ModuleKind::Resonator(ResonatorModule {
                    mode: ResonatorMode::Plate,
                    decay_ms: 1200.0,
                    brightness: 0.75,
                    mix: 1.0,
                    ..ResonatorModule::default()
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
    let g_post = compile_zone_graph(&post_res_zone).unwrap();
    let plate_slot = g_post.slot_for_module(4).unwrap() as u8;
    assert!(
        g_post.post_gate_tail_slots.contains(&plate_slot),
        "downstream Plate resonator must remain in post_gate_tail_slots"
    );
    let mut ring_voice = ModularVoice::new_boxed(
        &spec(&post_res_zone, &g_post, sr, 120.0),
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
        "post-VCA plate resonator must continue ringing past 450 ms (active={}, late_peak={late_peak})",
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
fn voice_retirement_preserves_sampler_env_predelay_lfo_chopped_quiet_and_legato_resonator_tails() {
    let sr = 48_000.0_f64;
    let empty_bank: SampleBank = Vec::new();

    // 1. Sampler + upstream Resonator + pre-delayed Env: short sample finishes while Env is still
    //    in pre-delay; voice must stay alive so the delayed Env opens and plays the Resonator tail.
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
                "Res",
                ModuleKind::Resonator(ResonatorModule {
                    mode: ResonatorMode::Plate,
                    decay_ms: 1200.0,
                    mix: 1.0,
                    ..ResonatorModule::default()
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
        "pre-delayed Env on sampler_only zone must open and pass upstream Resonator tail (peak={post_delay_peak})"
    );

    // 2. LFO-chopped post-VCA Plate resonator: voice must survive >20 ms LFO-off silent gaps.
    let z_lfo_chop = build_zone(
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
                ModuleKind::Resonator(ResonatorModule {
                    mode: ResonatorMode::Plate,
                    decay_ms: 1500.0,
                    brightness: 0.75,
                    mix: 1.0,
                    ..ResonatorModule::default()
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
        "LFO-chopped post-VCA resonator must survive 50 ms LFO-off gaps (active={}, peak={second_pulse_peak})",
        v_chop.is_active()
    );

    // 3. Quiet post-VCA resonator (Out.gain_db = -36 dB, velocity = 0.05) must not cut off while ringing.
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
                ModuleKind::Resonator(ResonatorModule {
                    mode: ResonatorMode::Plate,
                    decay_ms: 1200.0,
                    mix: 1.0,
                    ..ResonatorModule::default()
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
        "quiet post-VCA resonator must remain active while modal bank is still ringing"
    );

    // 4. Legato retarget raising post-VCA resonator decay from 50 ms to 4000 ms must extend tail_frames.
    let mut z_legato = z_quiet.clone();
    if let ModuleKind::Resonator(r) = &mut z_legato.modules[3].kind {
        r.decay_ms = 50.0;
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
    if let ModuleKind::Resonator(r) = &mut z_legato_long.modules[3].kind {
        r.decay_ms = 4000.0;
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
        "legato retarget increasing post-VCA resonator decay to 4000 ms must outlive 2.0 s default tail cap"
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
    // High-frequency sine VCO (MIDI 119 ~= 7902 Hz, right in the old comb-notch band at 44.1/48 kHz)
    // through DriveNode(drive=0.0, mix=0.5, tone_hz=18000 Hz) must stay flat without a comb notch.
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
                    mode: DriveMode::Tape,
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
                    mode: FilterMode::LowPass,
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
fn resonator_high_pitch_cv_clamps_to_max_hz_without_recompute_thrash() {
    use cadence_core::dsp::{NYQUIST_SAFE_RATIO, RESONATOR_MAX_HZ};

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
                    "Res",
                    ModuleKind::Resonator(ResonatorModule {
                        mode: ResonatorMode::String,
                        pitch_tracking,
                        fixed_hz,
                        decay_ms: 250.0,
                        brightness: 0.5,
                        mix: 1.0,
                        ..ResonatorModule::default()
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
    let fixed_zone = make_zone(PitchTracking::Fixed, RESONATOR_MAX_HZ);
    let g_chrom = compile_zone_graph(&chrom_zone).unwrap();
    let g_fixed = compile_zone_graph(&fixed_zone).unwrap();
    let res_slot_c = g_chrom.slot_for_module(3).unwrap();
    let res_slot_f = g_fixed.slot_for_module(3).unwrap();

    for sr in [8_000.0_f64, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
        let expected_ceiling = (sr * NYQUIST_SAFE_RATIO).min(f64::from(RESONATOR_MAX_HZ));

        // 1. Chromatic MIDI 127 (~12.54 kHz) + sweeping +0..+48 st PitchCv stays pinned at ceiling.
        let mut vc =
            ModularVoice::new_boxed(&spec(&chrom_zone, &g_chrom, sr, 120.0), start(127, None));
        assert_no_heap_operations(|| {
            for _ in 0..512 {
                let f = vc.next_frame(&bank, None);
                assert!(f[0].is_finite() && f[1].is_finite());
            }
        });
        match vc.nodes[res_slot_c] {
            ModuleNode::Resonator(ref res) => {
                assert!(
                    (res.applied_f0_hz() - expected_ceiling).abs() < 1e-6,
                    "sr={sr}: chromatic applied_f0_hz={} != {expected_ceiling}",
                    res.applied_f0_hz()
                );
                assert_eq!(
                    res.param_update_count(),
                    1,
                    "sr={sr}: high PitchCv above {expected_ceiling} Hz must not thrash resonator recomputes"
                );
            }
            _ => panic!("expected ResonatorNode"),
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
        match vf.nodes[res_slot_f] {
            ModuleNode::Resonator(ref res) => {
                assert!(
                    (res.applied_f0_hz() - expected_ceiling).abs() < 1e-6,
                    "sr={sr}: fixed applied_f0_hz={} != {expected_ceiling}",
                    res.applied_f0_hz()
                );
                assert_eq!(
                    res.param_update_count(),
                    1,
                    "sr={sr}: fixed high PitchCv above {expected_ceiling} Hz must not thrash resonator recomputes"
                );
            }
            _ => panic!("expected ResonatorNode"),
        }
    }
}

#[test]
fn pooled_resonator_voice_reuse_clears_prior_long_decay_tail_budget() {
    let bank: SampleBank = Vec::new();
    let make_res_zone = |decay_ms: f32, brightness: f32| {
        build_zone(
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
                    ModuleKind::Resonator(ResonatorModule {
                        mode: ResonatorMode::Plate,
                        decay_ms,
                        brightness,
                        mix: 1.0,
                        ..ResonatorModule::default()
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

    let long_zone = make_res_zone(8_000.0, 1.0);
    let short_zone = make_res_zone(50.0, 0.5);
    let g_long = compile_zone_graph(&long_zone).unwrap();
    let g_short = compile_zone_graph(&short_zone).unwrap();

    for sr in [44_100.0_f64, 48_000.0, 96_000.0, 192_000.0] {
        let mut voice = ModularVoice::new_boxed_inactive(sr);
        let default_tail = (sr * DEFAULT_TAIL_TIMEOUT_SEC) as u32;
        let max_tail = (sr * MAX_RESONATOR_TAIL_SEC) as u32;

        assert_no_heap_operations(|| {
            // 1. Trigger 8000 ms bright Plate resonator: tail_frames must cover bright RT60 (> 20 s).
            voice.begin_trigger_from_zone(&spec(&long_zone, &g_long, sr, 120.0));
            voice.finalize_trigger(start(60, Some(480)), false, false);
            assert!(
                voice.tail_frames > (sr * 20.0) as u32 && voice.tail_frames <= max_tail,
                "sr={sr}: 8000 ms resonator expected > 20 s tail budget, got {}",
                voice.tail_frames
            );
            for _ in 0..256 {
                let f = voice.next_frame(&bank, None);
                assert!(f[0].is_finite() && f[1].is_finite());
            }

            // 2. Reuse inactive pooled slot for fresh 50 ms resonator: must NOT inherit 8000 ms tail!
            voice.deactivate();
            voice.begin_trigger_from_zone(&spec(&short_zone, &g_short, sr, 120.0));
            voice.finalize_trigger(start(60, Some(480)), false, false);
            assert_eq!(
                voice.tail_frames, default_tail,
                "sr={sr}: fresh 50 ms pooled slot inherited stale long tail_frames"
            );

            // 3. Re-trigger 8000 ms, then non-legato retarget while active to 50 ms: must snap tail!
            voice.begin_trigger_from_zone(&spec(&long_zone, &g_long, sr, 120.0));
            voice.finalize_trigger(start(60, Some(480)), false, false);
            assert!(voice.tail_frames > (sr * 20.0) as u32);
            voice.retarget_from_zone(
                &spec(&short_zone, &g_short, sr, 120.0),
                start(62, Some(480)),
                false,
            );
            assert_eq!(
                voice.tail_frames, default_tail,
                "sr={sr}: non-legato retarget to 50 ms inherited stale long tail_frames"
            );
        });
    }
}

#[test]
fn post_trigger_upward_resonator_decay_modulation_preserves_tail_zero_alloc() {
    let bank: SampleBank = Vec::new();
    let short_zone = build_zone(
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
                ModuleKind::Resonator(ResonatorModule {
                    mode: ResonatorMode::Plate,
                    decay_ms: 50.0,
                    brightness: 0.85,
                    mix: 1.0,
                    ..ResonatorModule::default()
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
    let graph = compile_zone_graph(&short_zone).unwrap();
    let norm_4000 = ModuleParam::ResonatorDecay.normalize(4_000.0);

    // 1. Across 44.1 / 48 / 96 / 192 kHz, raising ResonatorDecay post-trigger extends tail_frames
    //    when entering tail countdown, with zero heap allocations.
    for sr in [44_100.0_f64, 48_000.0, 96_000.0, 192_000.0] {
        let mut voice =
            ModularVoice::new_boxed(&spec(&short_zone, &graph, sr, 120.0), start(60, Some(480)));
        let default_tail = (sr * DEFAULT_TAIL_TIMEOUT_SEC) as u32;
        assert_eq!(voice.tail_frames, default_tail);

        assert_no_heap_operations(|| {
            // Run 2 ms (StrikeEnv still active), then raise ResonatorDecay to 4000 ms.
            let pre_frames = (sr * 0.002) as usize;
            for _ in 0..pre_frames {
                let f = voice.next_frame(&bank, None);
                assert!(f[0].is_finite() && f[1].is_finite());
            }
            voice.apply_continuous_module_param(4, ModuleParam::ResonatorDecay, norm_4000);

            // Run 50 ms so StrikeEnv completes and enters tail countdown.
            let enter_frames = (sr * 0.050) as usize;
            for _ in 0..enter_frames {
                let f = voice.next_frame(&bank, None);
                assert!(f[0].is_finite() && f[1].is_finite());
            }
        });
        assert!(
            voice.is_active(),
            "sr={sr}: voice should remain active during modulated 4000 ms tail"
        );
        assert!(
            voice.tail_frames > (sr * 10.0) as u32,
            "sr={sr}: tail_frames did not extend for post-trigger 4000 ms decay (got {})",
            voice.tail_frames
        );
    }

    // 2. End-to-end ringout check past 2.3 s (exceeding 2.0 s default cap) when decay is raised
    //    BOTH before and AFTER StrikeEnv finishes!
    let sr = 48_000.0_f64;
    let mut long_zone = short_zone.clone();
    if let ModuleKind::Resonator(ref mut res) = long_zone.modules[3].kind {
        res.decay_ms = 5_000.0;
    }
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
        "post-tail upward ResonatorDecay modulation must outlive 2.0 s cap (active={}, late_peak={late_peak})",
        voice_after_tail.is_active()
    );

    // 3. Even if continuous modulation repeatedly wiggles ResonatorDecay on a stuck feedback loop,
    //    tail_elapsed_frames caps total tail duration cleanly at MAX_RESONATOR_TAIL_SEC.
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
                ModuleKind::Resonator(ResonatorModule {
                    mode: ResonatorMode::Plate,
                    decay_ms: 8_000.0,
                    brightness: 1.0,
                    mix: 1.0,
                    ..ResonatorModule::default()
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
    let norm_8000 = ModuleParam::ResonatorDecay.normalize(8_000.0);
    let norm_7500 = ModuleParam::ResonatorDecay.normalize(7_500.0);
    let max_frames = (stuck_sr * (MAX_RESONATOR_TAIL_SEC + 0.5)) as usize;
    for i in 0..max_frames {
        if !v_stuck.is_active() {
            break;
        }
        if i % 256 == 0 {
            let norm = if (i / 256) % 2 == 0 {
                norm_8000
            } else {
                norm_7500
            };
            v_stuck.apply_continuous_module_param(4, ModuleParam::ResonatorDecay, norm);
        }
        let f = v_stuck.next_frame(&bank, None);
        assert!(f[0].is_finite() && f[1].is_finite());
    }
    assert!(
        !v_stuck.is_active(),
        "ungated feedback resonator with continuous decay modulation must terminate by MAX_RESONATOR_TAIL_SEC"
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
        trigger_mode: TriggerMode::Gate,
        delay_ms: 0.0,
        bursts: 1,
        burst_spacing_ms: 10.0,
    };
    let base_vco = || VcoModule {
        waveform: Waveform::Saw,
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
                        mode: FilterMode::AcidLowPass,
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
                        mode: DriveMode::Fold,
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
        ModuleKindTag::Resonator => build_zone(
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
                    "Resonator",
                    ModuleKind::Resonator(ResonatorModule {
                        mode: ResonatorMode::Plate,
                        decay_ms: 90.0,
                        brightness: 0.65,
                        structure: 0.45,
                        position: 0.3,
                        mix: 1.0,
                        ..ResonatorModule::default()
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
    // 6. Exhaustive ResonatorMode::ALL.
    for &res_mode in ResonatorMode::ALL {
        let mut z = synthetic_rack_zone_for_module_kind(ModuleKindTag::Resonator);
        if let ModuleKind::Resonator(ref mut res) = z.modules[3].kind {
            res.mode = res_mode;
        }
        cases.push((format!("ResonatorMode::{res_mode:?}"), z, false));
    }
    // 7. Feedback z^-1 loop topology (VCO self-FM + Filter -> Drive -> Filter.In).
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
                    mode: DriveMode::Tape,
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
    // 1. Post-VCA Resonator across every ResonatorMode::ALL.
    for &mode in ResonatorMode::ALL {
        let z = build_zone(
            vec![
                RackModule::new(1, "Noise", ModuleKind::Noise(NoiseModule::default())),
                RackModule::new(2, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(3, "Env", ModuleKind::Env(short_env)),
                RackModule::new(
                    4,
                    "Resonator",
                    ModuleKind::Resonator(ResonatorModule {
                        mode,
                        decay_ms: 140.0,
                        brightness: 0.5,
                        structure: 0.4,
                        mix: 1.0,
                        ..ResonatorModule::default()
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
        post_vca_zones.push((format!("PostVca::Resonator::{mode:?}"), z, true));
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
