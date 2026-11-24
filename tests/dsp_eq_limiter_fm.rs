//! Integration tests for Parametric Studio EQ, Studio Brickwall Limiter,
//! Modular Rack linear through-zero FM, core stereo resampling, and 32-step Delay.

use cadence_app::view::{DeckView, EffectVisualView, InstrumentVisualView, ModuleVisualView};
use cadence_app::App;
use cadence_core::dsp::{Eq, Limiter};
use cadence_core::material::{
    InstrumentInstance, Lane, Modulation, ModulationTarget, Part, Pattern, PatternKind, Placement,
    Project, ValuePoint, ValueUnit,
};
use cadence_core::mixer::{
    Channel, ChannelKind, EffectDevice, EffectKind, EffectParam, EqPatch, LimiterPatch,
};
use cadence_core::music::{EndRule, Step};
use cadence_core::params::{InstrumentParam, ValueMode};
use cadence_core::rack::{
    cable_amount_range, compile_zone_graph, format_cable_amount, normalize_cable_amount, EnvModule,
    InPort, ModuleKind, ModuleKindTag, OutModule, PanelPin, PatchCable, RackModule, RackPatch,
    RackZone, VcaModule, VcoModule, VoiceSettings, Waveform,
};
use cadence_core::runtime::{AudioSnapshot, TransportIntent};
use cadence_engine::audio::{render_schedule_pcm, AudioRuntime};
use cadence_engine::modular_voice::{ModularVoice, VoiceStart, ZoneTriggerSpec};
use std::f64::consts::TAU;
use std::time::{Duration, Instant};

fn eq_sine_rms(eq: &mut Eq, freq_hz: f64, sr: f64) -> f64 {
    eq.reset();
    let n = 8192;
    let mut sum = 0.0;
    for i in 0..n {
        let x = (TAU * freq_hz * i as f64 / sr).sin() as f32;
        let y = eq.process([x, x], false);
        if i >= n / 2 {
            sum += f64::from(y[0]) * f64::from(y[0]);
        }
    }
    (sum / (n / 2) as f64).sqrt()
}

fn dft_power(samples: &[f64], freq_hz: f64, sr: f64) -> f64 {
    let n = samples.len();
    let (mut re, mut im) = (0.0, 0.0);
    for (i, &s) in samples.iter().enumerate() {
        let w = 0.5 - 0.5 * (TAU * i as f64 / (n - 1) as f64).cos();
        let angle = TAU * freq_hz * i as f64 / sr;
        re += s * w * angle.cos();
        im += s * w * angle.sin();
    }
    (re * re + im * im) / (n as f64 * n as f64)
}

#[test]
fn parametric_eq_sweeps_mid_freq_and_q() {
    // 1. Parameter metadata, aliases, log normalization, and ordering on EffectParam::EQ.
    assert_eq!(
        EffectParam::EQ,
        [
            EffectParam::Hpf,
            EffectParam::Low,
            EffectParam::Mid,
            EffectParam::MidFreq,
            EffectParam::MidQ,
            EffectParam::High,
        ]
    );
    assert_eq!(EffectParam::parse("mid_hz"), Some(EffectParam::MidFreq));
    assert_eq!(EffectParam::parse("mid_freq"), Some(EffectParam::MidFreq));
    assert_eq!(EffectParam::parse("mid_q"), Some(EffectParam::MidQ));
    let mut eq_dev = EffectDevice::eq(1);
    for param in [EffectParam::MidFreq, EffectParam::MidQ] {
        for n in [0.0, 0.35, 0.5, 0.8, 1.0] {
            eq_dev.write_normalized(param, n).unwrap();
            let got = eq_dev.read_normalized(param).unwrap();
            assert!((got - n).abs() < 1e-5, "{param:?}: {got} vs {n}");
        }
    }

    // 3. Sweeping mid_hz from 250 Hz to 3000 Hz visibly & measurably shifts peak resonance.
    let sr = 48_000.0;
    let mut eq = Eq::new(sr);
    eq.set_parameters(EqPatch {
        mid_db: 12.0,
        mid_hz: 250.0,
        mid_q: 2.0,
        ..EqPatch::default()
    });
    let low_at_250 = eq_sine_rms(&mut eq, 250.0, sr);
    let low_at_3k = eq_sine_rms(&mut eq, 3000.0, sr);
    assert!(
        low_at_250 > low_at_3k * 3.0,
        "250 Hz boost must dominate 3 kHz: {low_at_250} vs {low_at_3k}"
    );

    eq.set_parameters(EqPatch {
        mid_db: 12.0,
        mid_hz: 3000.0,
        mid_q: 2.0,
        ..EqPatch::default()
    });
    let high_at_250 = eq_sine_rms(&mut eq, 250.0, sr);
    let high_at_3k = eq_sine_rms(&mut eq, 3000.0, sr);
    assert!(
        high_at_3k > high_at_250 * 3.0,
        "3 kHz boost must dominate 250 Hz: {high_at_3k} vs {high_at_250}"
    );

    // 4. Varying mid_q from 0.5 (wide) to 6.0 (narrow surgical) tightens off-center skirt dramatically.
    eq.set_parameters(EqPatch {
        mid_db: 12.0,
        mid_hz: 1000.0,
        mid_q: 0.5,
        ..EqPatch::default()
    });
    let wide_center = eq_sine_rms(&mut eq, 1000.0, sr);
    let wide_skirt = eq_sine_rms(&mut eq, 1600.0, sr);

    eq.set_parameters(EqPatch {
        mid_db: 12.0,
        mid_hz: 1000.0,
        mid_q: 6.0,
        ..EqPatch::default()
    });
    let narrow_center = eq_sine_rms(&mut eq, 1000.0, sr);
    let narrow_skirt = eq_sine_rms(&mut eq, 1600.0, sr);

    assert!(
        (wide_center - narrow_center).abs() < 0.05,
        "peak gain at center should remain ~+12 dB regardless of Q"
    );
    assert!(
        wide_skirt > narrow_skirt * 1.8,
        "wide Q=0.5 must boost 1.6 kHz skirt far more than surgical Q=6.0: {wide_skirt} vs {narrow_skirt}"
    );
}

#[test]
fn limiter_enforces_brickwall_ceiling_reports_live_gr_and_wires_fx_add_command() {
    // 1. Direct DSP brickwall ceiling enforcement on extreme hot bursts & transients.
    let sr = 48_000.0;
    let mut lim = Limiter::new(sr);
    let ceiling_db = -2.0_f32;
    let ceiling_lin = 10.0_f32.powf(ceiling_db / 20.0);
    lim.set_parameters(LimiterPatch {
        ceiling_db,
        input_drive_db: 18.0,
        release_ms: 45.0,
        mix: 1.0,
    });
    let mut max_peak = 0.0_f32;
    let mut max_gr = 0.0_f32;
    for i in 0..9600 {
        let sig = if i % 600 == 100 {
            1.5_f32
        } else {
            (TAU * 180.0 * i as f64 / sr).sin() as f32 * 0.85
        };
        let y = lim.process([sig, -sig * 0.9], false);
        assert!(y[0].is_finite() && y[1].is_finite());
        max_peak = max_peak.max(y[0].abs()).max(y[1].abs());
        max_gr = max_gr.max(lim.gain_reduction_db());
    }
    assert!(
        max_peak <= ceiling_lin + 1e-5,
        "brickwall ceiling violated: {max_peak} > {ceiling_lin}"
    );
    assert!(max_gr > 12.0, "expected >12 dB GR, got {max_gr}");

    // Constant-latency bypass flushes lookahead cleanly & resets GR without stale bursts.
    let flush_frames = lim.latency_frames() * 3 + (sr * 0.02) as usize;
    let mut dry = [0.0_f32; 2];
    for _ in 0..flush_frames {
        dry = lim.process([0.77, -0.33], true);
    }
    assert_eq!(dry, [0.77, -0.33]);
    assert_eq!(lim.gain_reduction_db(), 0.0);

    // 2. End-to-end `:fx add limiter` command + Signal Graph "LIM" tag + EffectDeckView.
    let mut app = App::new(Project::demo(), "demo.json".into()).unwrap();
    app.execute_command("fx add limiter master").unwrap();
    let lim_dev = app
        .project()
        .mixer
        .master
        .inserts
        .last()
        .expect("limiter added to master");
    assert_eq!(lim_dev.kind.short_tag(), "LIM");
    assert!(matches!(lim_dev.kind, EffectKind::Limiter(_)));
    let lim_id = lim_dev.id;

    app.execute_command(&format!("fx {lim_id}")).unwrap();
    let mut snap = AudioSnapshot {
        sample_rate: 48_000,
        ..AudioSnapshot::default()
    };
    snap.set_effect_meter(lim_id, 5.2, 7.6);
    let fv = app.frame_view(&snap, "silent");
    let deck = fv
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Effect(v) => Some(v),
            _ => None,
        })
        .expect("Limiter EffectDeckView");
    assert_eq!(deck.kind_label, "LIMITER");
    match &deck.visual {
        EffectVisualView::Limiter {
            ceiling_db,
            gain_reduction_db,
            gr_hold_db,
            transfer_curve,
            ..
        } => {
            assert!((*ceiling_db - (-0.3)).abs() < 1e-4);
            assert!((*gain_reduction_db - 5.2).abs() < 1e-4);
            assert!((*gr_hold_db - 7.6).abs() < 1e-4);
            assert_eq!(transfer_curve.len(), 96);
        }
        other => panic!("expected EffectVisualView::Limiter, got {other:?}"),
    }

    // 3. Live AudioRuntime publishes positive Limiter GR dB into AudioSnapshot::effect_meters,
    //    and clears back to 0.0 dB when bypassed!
    let mut p = Project::empty();
    p.bpm = 120.0;
    p.bars = 2;
    p.end = EndRule::Loop;
    p.mixer.master.strip.gain = 1.0;
    let rt_lim_id = 77_u64;
    p.mixer.master.inserts.push(EffectDevice {
        id: rt_lim_id,
        name: "Master Limiter".into(),
        bypassed: false,
        kind: EffectKind::Limiter(LimiterPatch {
            ceiling_db: -6.0,
            input_drive_db: 14.0,
            release_ms: 60.0,
            mix: 1.0,
        }),
    });
    p.mixer
        .channels
        .push(Channel::new(1, ChannelKind::Audio, "HotLead"));
    let hit = Step {
        active: true,
        degree: 1,
        velocity: 1.0,
        gate: 0.95,
        ..Step::default()
    };
    p.patterns.push(Pattern {
        id: 1,
        name: "Wall".into(),
        kind: PatternKind::Notes {
            steps: vec![hit; 16],
        },
    });
    let mut hot_rack = RackPatch::default_synth();
    if let Some(ModuleKind::Env(env)) = hot_rack.zones[0].module_mut(4).map(|m| &mut m.kind) {
        env.attack_ms = 1.0;
        env.sustain = 1.0;
    }
    p.instruments.push(InstrumentInstance {
        id: 1,
        name: "HotLead".into(),
        rack: hot_rack,
        channel: 1,
    });
    p.parts.push(Part::new(1, "HotPart", 1, 1));
    let mut lane = Lane::new(1, "Hot");
    lane.placements.push(Placement::new(1, 1, 0, 32));
    p.lanes.push(lane);

    // Offline render confirms strict ceiling enforcement in full audio engine too.
    let compiled = p.compile().unwrap();
    let (_report, pcm) = render_schedule_pcm(&[(0, &compiled)], 0, 16, Vec::new(), 24_000).unwrap();
    let max_sample = pcm
        .iter()
        .flatten()
        .copied()
        .map(|s| f32::from(s).abs() / 32768.0)
        .fold(0.0_f32, f32::max);
    let rt_ceil_lin = 10.0_f32.powf(-6.0 / 20.0);
    assert!(
        max_sample <= rt_ceil_lin + 2e-3,
        "engine render violated -6 dBFS limiter ceiling: {max_sample} > {rt_ceil_lin}"
    );
    assert!(max_sample > rt_ceil_lin * 0.70, "max_sample={max_sample}");

    // Real-time silent AudioRuntime publishes live GR > 2.0 dB via EffectMeter.
    let rt = AudioRuntime::start(compiled, true).unwrap();
    assert!(rt.transport(TransportIntent::Play { playing: true }));
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut observed_gr = 0.0_f32;
    while Instant::now() < deadline {
        let snap = rt.snapshot();
        let gr = snap
            .effect_meter(rt_lim_id)
            .map_or(0.0, |m| m.gain_reduction_db);
        if gr > 2.0 {
            observed_gr = gr;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        observed_gr > 2.0 && observed_gr.is_finite(),
        "expected live limiter GR > 2 dB in AudioSnapshot, got {observed_gr}"
    );

    // Bypass resets GR to 0.0 dB.
    p.mixer.master.inserts[0].bypassed = true;
    rt.replace_project(p.compile().unwrap()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut bypassed_gr = observed_gr;
    while Instant::now() < deadline {
        let snap = rt.snapshot();
        bypassed_gr = snap
            .effect_meter(rt_lim_id)
            .map_or(0.0, |m| m.gain_reduction_db);
        if bypassed_gr == 0.0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(bypassed_gr, 0.0, "bypassed limiter must report 0.0 dB GR");
}

#[test]
fn rack_linear_fm_silent_modulator_sidebands_automation_and_serde_roundtrip() {
    let build_fm_rack = |fm_amount: f32| {
        // Carrier VCO A (440 Hz Sine) -> VCA -> Out; Amp Env -> VCA.Cv.
        // Modulator VCO B (880 Hz Sine, +12 st) has NO audio cable to VCA/Out (pure silent modulator!),
        // patched ONLY into VCO A.FmIn via cable #4.
        let zone = RackZone {
            id: 1,
            name: "FM".into(),
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
                    "VCO A",
                    ModuleKind::Vco(VcoModule {
                        waveform: Waveform::Sine,
                        level: 0.8,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(
                    2,
                    "VCO B",
                    ModuleKind::Vco(VcoModule {
                        waveform: Waveform::Sine,
                        tune_semitones: 12.0,
                        level: 1.0,
                        ..VcoModule::default()
                    }),
                ),
                RackModule::new(3, "VCA", ModuleKind::Vca(VcaModule::default())),
                RackModule::new(
                    4,
                    "Amp Env",
                    ModuleKind::Env(EnvModule {
                        attack_ms: 1.0,
                        decay_ms: 1.0,
                        sustain: 1.0,
                        ..EnvModule::default()
                    }),
                ),
                RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
            ],
            cables: vec![
                PatchCable::new(1, 1, 3, InPort::In, 1.0),
                PatchCable::new(2, 3, 5, InPort::In, 1.0),
                PatchCable::new(3, 4, 3, InPort::Cv, 1.0),
                PatchCable::new(4, 2, 1, InPort::FmIn, fm_amount),
            ],
        };
        let rack = RackPatch {
            kit_name: String::new(),
            zones: vec![zone],
            gain_db: 0.0,
            pan: 0.0,
            tune_semitones: 0.0,
            cutoff_hz: 20_000.0,
            resonance: 0.0,
            panel_pins: vec![PanelPin::cable(1, 4, "FM")],
        };
        rack.validate().unwrap();
        rack
    };

    let base_rack = build_fm_rack(0.0);
    let fm_cable_param = InstrumentParam::Cable { zone: 1, cable: 4 };

    // 1. Serde defaults & round-trip: PatchCable omitting `from_port` defaults to OutPort::Out,
    //    and RackPatch round-trips FM topology losslessly.
    let mut cable_val = serde_json::to_value(base_rack.zones[0].cables[3]).unwrap();
    cable_val.as_object_mut().unwrap().remove("from_port");
    let decoded_cable: PatchCable =
        serde_json::from_value(cable_val).expect("PatchCable JSON without from_port");
    assert_eq!(decoded_cable, base_rack.zones[0].cables[3]);
    let json = serde_json::to_string(&base_rack).unwrap();
    let decoded_rack: RackPatch = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded_rack, base_rack);

    // 2. `InPort::FmIn` & `InstrumentParam::Cable` metadata, aliases, pinned shorthand, automatability.
    assert_eq!(InPort::parse("fm"), Some(InPort::FmIn));
    assert_eq!(InPort::parse("fm_in"), Some(InPort::FmIn));
    assert!(ModuleKindTag::Vco.accepts_in_port(InPort::FmIn));
    assert_eq!(
        InstrumentParam::parse_for_rack("fm", &base_rack, true),
        Some(fm_cable_param)
    );
    assert_eq!(
        InstrumentParam::parse_for_rack("c4", &base_rack, true),
        Some(fm_cable_param)
    );
    assert!(fm_cable_param.is_automatable());
    assert_eq!(
        cable_amount_range(ModuleKindTag::Vco, InPort::FmIn),
        (-1.0, 1.0)
    );
    assert_eq!(
        format_cable_amount(ModuleKindTag::Vco, InPort::FmIn, 0.45),
        "+45%"
    );

    // 3. Pure modulator mode (`VCO B` has NO audio cable to Out, only `VCO B -> VCO A.FmIn`):
    //    Carrier = 440 Hz Sine (A4), Modulator = 880 Hz Sine (+12 st).
    //    FM generates rich sidebands at f_c ± k*f_m (e.g. 1320 Hz, 2200 Hz) while
    //    `fm_amount == 0.0` remains a pure 440 Hz sine with negligible 1320/2200 Hz power!
    let sr = 48_000.0;
    let render_voice = |rack: &RackPatch| {
        let zone = &rack.zones[0];
        let graph = compile_zone_graph(zone).unwrap();
        let mut v = ModularVoice::new_boxed(
            &ZoneTriggerSpec {
                inst_id: 1,
                zone,
                graph: &graph,
                sample_rate: sr,
                bpm: 120.0,
                zone_pan: zone.pan,
                rack_tune: 0.0,
                is_preview: false,
            },
            VoiceStart {
                midi: 69,
                velocity: 0.9,
                gate_frames: None,
                seed: 42,
                clock_seconds: 0.0,
            },
        );
        let bank = Vec::new();
        for _ in 0..2400 {
            v.next_frame(&bank, None);
        }
        (0..16_384)
            .map(|_| {
                let frame = v.next_frame(&bank, None);
                assert!(frame[0].is_finite() && frame[1].is_finite());
                assert!(frame[0].abs() <= 1.0 && frame[1].abs() <= 1.0);
                f64::from(frame[0])
            })
            .collect::<Vec<_>>()
    };

    let pure_sine = render_voice(&base_rack);
    let pure_sine_repeat = render_voice(&base_rack);
    assert_eq!(pure_sine, pure_sine_repeat, "fm=0 must be bit-identical");

    let fm_rack = build_fm_rack(0.55);
    let fm_sig = render_voice(&fm_rack);

    let base_sideband_1 = dft_power(&pure_sine, 1320.0, sr);
    let fm_sideband_1 = dft_power(&fm_sig, 1320.0, sr);
    let fm_sideband_2 = dft_power(&fm_sig, 2200.0, sr);
    assert!(
        fm_sideband_1 > base_sideband_1 * 1000.0 && fm_sideband_1 > 1e-4,
        "FM must create strong 1320 Hz sideband with silent modulator: base={base_sideband_1}, fm={fm_sideband_1}"
    );
    assert!(
        fm_sideband_2 > 1e-4,
        "FM must create strong 2200 Hz higher sideband: {fm_sideband_2}"
    );

    // 4. ModulationTarget::Instrument { parameter: Cable { zone: 1, cable: 4 } } automation changes rendered audio!
    let make_project = |with_fm_mod: bool| {
        let mut p = Project::empty();
        p.bpm = 120.0;
        p.bars = 1;
        p.mixer.master.strip.gain = 0.9;
        p.mixer
            .channels
            .push(Channel::new(1, ChannelKind::Audio, "FM"));
        let hit = Step {
            active: true,
            degree: 1,
            velocity: 0.9,
            gate: 0.95,
            ..Step::default()
        };
        p.patterns.push(Pattern {
            id: 1,
            name: "Hold".into(),
            kind: PatternKind::Notes {
                steps: vec![hit; 16],
            },
        });
        p.instruments.push(InstrumentInstance {
            id: 1,
            name: "FM Bell".into(),
            rack: base_rack.clone(),
            channel: 1,
        });
        p.parts.push(Part::new(1, "FMPart", 1, 1));
        let mut lane = Lane::new(1, "FM");
        lane.placements.push(Placement::new(1, 1, 0, 16));
        p.lanes.push(lane);

        if with_fm_mod {
            let norm_zero = normalize_cable_amount(ModuleKindTag::Vco, InPort::FmIn, 0.0);
            let norm_high = normalize_cable_amount(ModuleKindTag::Vco, InPort::FmIn, 0.75);
            p.patterns.push(Pattern {
                id: 2,
                name: "FM Ramp".into(),
                kind: PatternKind::Values {
                    unit: ValueUnit::Normalized,
                    period: 16,
                    mode: ValueMode::Ramp,
                    points: vec![
                        ValuePoint {
                            step: 0,
                            value: norm_zero,
                        },
                        ValuePoint {
                            step: 8,
                            value: norm_high,
                        },
                    ],
                },
            });
            p.modulations.push(Modulation {
                id: 1,
                name: "FM Depth".into(),
                pattern: 2,
                target: ModulationTarget::Instrument {
                    instrument: 1,
                    parameter: fm_cable_param,
                },
            });
        }
        p
    };

    let static_score = make_project(false).compile().unwrap();
    let mod_score = make_project(true).compile().unwrap();
    let (rep_static, pcm_static) =
        render_schedule_pcm(&[(0, &static_score)], 0, 16, Vec::new(), 24_000).unwrap();
    let (rep_mod, pcm_mod) =
        render_schedule_pcm(&[(0, &mod_score)], 0, 16, Vec::new(), 24_000).unwrap();
    assert!(rep_static.output.rms[0] > 0.05 && rep_mod.output.rms[0] > 0.05);
    assert_ne!(
        pcm_static, pcm_mod,
        "FM cable modulation lane must measurably alter rendered audio"
    );

    // 5. Rack InstrumentDeckView reflects the FM cable on both PinnedOverview and VCO A module faceplate!
    let mut proj_view = make_project(false);
    proj_view.instruments[0].rack = build_fm_rack(0.45);
    let mut app = App::new(proj_view, "fm.json".into()).unwrap();
    app.apply(cadence_app::Action::SelectInstrument { instrument: 1 })
        .unwrap();

    // Quick PinnedOverview pill (slot 0) shows FM badge and finite FM waveform curve.
    let fv_pin = app.frame_view(&AudioSnapshot::default(), "silent");
    let iv_pin = fv_pin
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Instrument(v) => Some(v),
            _ => None,
        })
        .expect("InstrumentDeckView");
    match &iv_pin.visual {
        InstrumentVisualView::Rack(rv) => match &rv.visual {
            ModuleVisualView::PinnedOverview {
                osc_label,
                osc_curve,
                ..
            } => {
                assert!(
                    osc_label.as_deref().unwrap_or("").contains("FM"),
                    "PinnedOverview osc_label must show FM badge: {osc_label:?}"
                );
                assert_eq!(osc_curve.len(), 256);
                assert!(osc_curve
                    .iter()
                    .all(|v| v.is_finite() && (-1.2..=1.2).contains(v)));
            }
            other => panic!("expected PinnedOverview visual, got {other:?}"),
        },
        other => panic!("expected Rack visual, got {other:?}"),
    }

    // Focusing VCO A module pill (slot 1) shows ModuleVisualView::Vco with `fm_active == true`!
    app.press_keys("l").unwrap();
    let fv_vco = app.frame_view(&AudioSnapshot::default(), "silent");
    let iv_vco = fv_vco
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Instrument(v) => Some(v),
            _ => None,
        })
        .expect("InstrumentDeckView");
    match &iv_vco.visual {
        InstrumentVisualView::Rack(rv) => match &rv.visual {
            ModuleVisualView::Vco {
                label,
                curve,
                fm_active,
                active,
                ..
            } => {
                assert!(*fm_active, "VCO A visual must report fm_active=true");
                assert!(*active, "VCO A visual must be active");
                assert_eq!(label, "VCO A · Sine · FM");
                assert_eq!(curve.len(), 256);
                assert!(curve
                    .iter()
                    .all(|v| v.is_finite() && (-1.2..=1.2).contains(v)));
            }
            other => panic!("expected Vco visual, got {other:?}"),
        },
        other => panic!("expected Rack visual, got {other:?}"),
    }
}

#[test]
fn core_stereo_resampler_and_32_step_delay_range_and_visual_grid() {
    use cadence_core::mixer::DelayPatch;

    // 1. Pure DSP resampler in cadence-core preserves DC and output length cleanly.
    let stereo_dc = vec![[0.5_f32, -0.25_f32]; 480];
    let resampled = cadence_core::dsp::resample_stereo(&stereo_dc, 48_000, 24_000);
    assert_eq!(resampled.len(), 240);
    assert!((resampled[120][0] - 0.5).abs() < 1e-3);
    assert!((resampled[120][1] + 0.25).abs() < 1e-3);

    // 2. 32-step Delay validation, EffectParam::Time range, and delay visual span.
    assert_eq!(EffectParam::Time.range(), (0.5, 32.0));
    let mut p = Project::demo();
    let dub32 = DelayPatch {
        steps: 32.0,
        feedback: 0.45,
        mix: 0.35,
        ..DelayPatch::default()
    };
    p.mixer.master.inserts.push(EffectDevice {
        id: p.max_id() + 1,
        name: "Dub32".into(),
        bypassed: false,
        kind: EffectKind::Delay(dub32),
    });
    p.validate().unwrap();
    let vis = cadence_app::view::effect::build_delay_visual(&dub32, 120.0);
    match vis {
        EffectVisualView::Delay {
            grid_steps,
            summary,
            ..
        } => {
            assert!(
                grid_steps >= 64.0,
                "32-step delay visual grid must accommodate 32-step taps: {grid_steps}"
            );
            assert!(
                summary.contains("32.00 steps"),
                "32-step delay must report 32.00 steps: {summary}"
            );
        }
        other => panic!("expected Delay visual, got {other:?}"),
    }
}

#[test]
fn filter_custom_project_pattern_drives_rhythmic_envelope() {
    let mut project = Project::demo();
    let mini_id = 900;
    project.patterns.push(Pattern {
        id: mini_id,
        name: "Filter Ratchet".into(),
        kind: PatternKind::Mini {
            code: "1 [1 1] ~ 1".into(),
            steps: 4,
        },
    });
    let dev_id = 901;
    project.mixer.master.inserts.push(EffectDevice {
        id: dev_id,
        name: "Custom Step Filter".into(),
        bypassed: false,
        kind: EffectKind::Filter(cadence_core::mixer::FilterPatch {
            mode: cadence_core::rack::FilterMode::LowPass,
            cutoff_hz: 320.0,
            resonance: 0.6,
            drive: 0.15,
            mix: 1.0,
            env_pattern: 0,
            env_pattern_id: Some(mini_id),
            env_amount: 3.2,
            env_decay_ms: 140.0,
        }),
    });
    let score = project.compile().unwrap();
    let compiled = score
        .mixer
        .filter_env_trigger(dev_id)
        .expect("custom pattern must compile into mixer.filter_env_triggers");
    // `[1 1]` on step 1 triggers automatic 32nd-note resolution (len = 4 * 2 = 8)!
    assert_eq!(compiled.resolution, 32);
    assert_eq!(compiled.length, 8);
    assert!(compiled.steps[0] > 0);
    assert!(compiled.steps[2] > 0 && compiled.steps[3] > 0);
    assert_eq!(compiled.steps[4], 0);
    assert!(compiled.steps[6] > 0);

    let rep = cadence_engine::audio::render_measure_report(&score, Vec::new(), 2, 24_000).unwrap();
    assert_eq!(rep.output.full_scale_samples, 0);
}
