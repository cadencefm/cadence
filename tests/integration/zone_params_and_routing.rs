//! Integration tests for zone-scoped parameter addressing (`ParamAddress::Zone`,
//! `InstrumentParam::Zone`), Rack zone inspector pages, cycle-checked `Group → Group`
//! bus routing, `ModulationTarget::Send`, and unified `:param` operations.
use cadence_app::view::{DeckView, GraphEdgeKind, GraphTarget};
use cadence_app::{
    Action, App, ControlTarget, Detail, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    MemoryHostIo, RackTab,
};
use cadence_core::dsp::EnvMode;
use cadence_core::material::{
    Edit, InstrumentInstance, Lane, LiveFlags, Modulation, ModulationTarget, Part, Pattern,
    PatternKind, PhaseMode, Placement, Project, ValuePoint, ValueUnit,
};
use cadence_core::midi_map::{MidiMapping, MidiSource, MidiTarget, MidiValueMode};
use cadence_core::mixer::{Channel, ChannelKind, ChannelRef, EffectParam, MixerParamTarget};
use cadence_core::music::Step;
use cadence_core::params::{InstrumentParam, ParamAddress, ValueMode};
use cadence_core::rack::{
    module_sample_key, EnvModule, FilterMode, FilterModule, InPort, ModuleKind, ModuleParam,
    OutModule, PatchCable, RackModule, RackPatch, RackZone, SamplerModule, TriggerMode, VcaModule,
    VcoModule, VoiceSettings, Waveform, ZoneParam,
};
use cadence_core::runtime::{AudioSnapshot, MidiMessage};
use cadence_core::sampler::{SampleData, SampleMetadata};
use cadence_engine::audio::{render_schedule_f32, render_stems_with_samples_report};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn write_half_silent_wav(path: &Path, sample_rate: u32, total_frames: usize, freq_hz: f32) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    let half = total_frames / 2;
    for i in 0..total_frames {
        let s = if i < half {
            0.0
        } else {
            let t = i as f32 / sample_rate as f32;
            (t * freq_hz * std::f32::consts::TAU).sin() * 0.8
        };
        let q = (s * i16::MAX as f32).round() as i16;
        writer.write_sample(q).unwrap();
        writer.write_sample(q).unwrap();
    }
    writer.finalize().unwrap();
}

fn synthetic_sample_data(frames: usize, sample_rate: u32, freq_hz: f32) -> Arc<SampleData> {
    let half = frames / 2;
    let mut ch = Vec::with_capacity(frames);
    for i in 0..frames {
        let s = if i < half {
            0.0
        } else {
            let t = i as f32 / sample_rate as f32;
            (t * freq_hz * std::f32::consts::TAU).sin() * 0.8
        };
        ch.push(s);
    }
    let left = ch.clone().into_boxed_slice();
    let right = Some(ch.into_boxed_slice());
    Arc::new(SampleData {
        path_label: "samples/half_sine.wav".into(),
        resolved_path: PathBuf::from("/virtual/samples/half_sine.wav"),
        name: "half_sine.wav".into(),
        sample_rate,
        channels: 2,
        frames,
        left,
        right,
        peaks: vec![(0.0, 0.0); 16].into_boxed_slice(),
        meta: SampleMetadata::default(),
    })
}

fn note_step(degree: i16, velocity: f32, gate: f32) -> Step {
    Step {
        active: true,
        degree,
        velocity,
        gate,
        ..Step::default()
    }
}

fn build_hybrid_kit_project(sample_rel: &str) -> Project {
    let mut p = Project::empty();
    p.bpm = 120.0;
    p.bars = 2;
    p.mixer.master.strip.gain = 1.0;
    let mut ch = Channel::new(1, ChannelKind::Audio, "KitCh");
    ch.strip.gain = 1.0;
    p.mixer.channels = vec![ch];

    let bd_zone = RackZone {
        id: 1,
        name: "BD".into(),
        low_midi: 36,
        high_midi: 36,
        root_midi: 36,
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
                    waveform: Waveform::Saw,
                    level: 0.75,
                    ..VcoModule::default()
                }),
            ),
            RackModule::new(
                2,
                "Filter",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::LowPass,
                    cutoff_hz: 6_000.0,
                    resonance: 0.1,
                    drive: 0.0,
                    key_tracking: 0.0,
                    vel_amount: 0.0,
                    mix: 1.0,
                }),
            ),
            RackModule::new(3, "VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                4,
                "Amp Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 1.0,
                    hold_ms: 0.0,
                    decay_ms: 250.0,
                    sustain: 0.6,
                    release_ms: 80.0,
                    trigger_mode: TriggerMode::Gate,
                    delay_ms: 0.0,
                    bursts: 1,
                    burst_spacing_ms: 10.0,
                }),
            ),
            RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
        ],
        cables: vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 2, 3, InPort::In, 1.0),
            PatchCable::new(3, 3, 5, InPort::In, 1.0),
            PatchCable::new(4, 4, 3, InPort::Cv, 1.0),
        ],
    };

    let smp_zone = RackZone {
        id: 2,
        name: "SMP".into(),
        low_midi: 48,
        high_midi: 48,
        root_midi: 48,
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
                    path: Some(sample_rel.into()),
                    one_shot: true,
                    root_midi: 48,
                    ..SamplerModule::default()
                }),
            ),
            RackModule::new(
                2,
                "Filter",
                ModuleKind::Filter(FilterModule {
                    mode: FilterMode::LowPass,
                    cutoff_hz: 18_500.0,
                    resonance: 0.08,
                    drive: 0.0,
                    key_tracking: 0.0,
                    vel_amount: 0.0,
                    mix: 1.0,
                }),
            ),
            RackModule::new(3, "VCA", ModuleKind::Vca(VcaModule::default())),
            RackModule::new(
                4,
                "Amp Env",
                ModuleKind::Env(EnvModule {
                    mode: EnvMode::Adsr,
                    attack_ms: 0.5,
                    hold_ms: 0.0,
                    decay_ms: 500.0,
                    sustain: 1.0,
                    release_ms: 50.0,
                    trigger_mode: TriggerMode::Gate,
                    delay_ms: 0.0,
                    bursts: 1,
                    burst_spacing_ms: 10.0,
                }),
            ),
            RackModule::new(5, "Out", ModuleKind::Out(OutModule::default())),
        ],
        cables: vec![
            PatchCable::new(1, 1, 2, InPort::In, 1.0),
            PatchCable::new(2, 2, 3, InPort::In, 1.0),
            PatchCable::new(3, 3, 5, InPort::In, 1.0),
            PatchCable::new(4, 4, 3, InPort::Cv, 1.0),
        ],
    };

    let kit = RackPatch {
        kit_name: "HybridKit".into(),
        zones: vec![bd_zone, smp_zone],
        gain_db: 0.0,
        pan: 0.0,
        tune_semitones: 0.0,
        cutoff_hz: 20_000.0,
        resonance: 0.0,
        panel_pins: Vec::new(),
    };
    p.instruments = vec![InstrumentInstance {
        id: 10,
        name: "KIT".into(),
        rack: kit,
        channel: 1,
    }];

    // Project root is C4 (60); compute exact 1-based chromatic degrees for MIDI 36 (BD) and 48 (SMP).
    p.root_midi = 60;
    p.scale = (0..12).collect();
    let (bd_deg, _) = cadence_core::music::midi_to_degree(p.root_midi, &p.scale, 36, 0);
    let (smp_deg, _) = cadence_core::music::midi_to_degree(p.root_midi, &p.scale, 48, 0);
    let mut steps = vec![Step::default(); 16];
    steps[0] = note_step(bd_deg, 0.85, 4.0);
    steps[8] = note_step(smp_deg, 0.85, 4.0);
    p.patterns = vec![
        Pattern {
            id: 20,
            name: "KitNotes".into(),
            kind: PatternKind::Notes { steps },
        },
        Pattern {
            id: 21,
            name: "Sweep".into(),
            kind: PatternKind::Values {
                unit: ValueUnit::Normalized,
                period: 16,
                mode: ValueMode::Ramp,
                points: vec![
                    ValuePoint {
                        step: 0,
                        value: 0.0,
                    },
                    ValuePoint {
                        step: 15,
                        value: 1.0,
                    },
                ],
            },
        },
        Pattern {
            id: 22,
            name: "LowVal".into(),
            kind: PatternKind::Values {
                unit: ValueUnit::Normalized,
                period: 16,
                mode: ValueMode::Hold,
                points: vec![ValuePoint {
                    step: 0,
                    value: 0.15,
                }],
            },
        },
    ];
    p.parts = vec![Part {
        id: 30,
        name: "KitPart".into(),
        pattern: 20,
        instrument: 10,
        transpose: 0,
        velocity_scale: 1.0,
        parameter_links: Vec::new(),
    }];
    p.lanes = vec![Lane {
        id: 40,
        name: "Drums".into(),
        placements: vec![Placement {
            id: 50,
            part: 30,
            start_step: 0,
            length_steps: 32,
            phase: PhaseMode::Song,
            offset_steps: 0,
            exceptions: Vec::new(),
        }],
        parts: Vec::new(),
    }];
    p.validate().unwrap();
    p
}

fn rms_stereo(frames: &[[f32; 2]]) -> (f64, f64) {
    if frames.is_empty() {
        return (0.0, 0.0);
    }
    let (mut sl, mut sr) = (0.0_f64, 0.0_f64);
    for &[l, r] in frames {
        sl += f64::from(l) * f64::from(l);
        sr += f64::from(r) * f64::from(r);
    }
    let n = frames.len() as f64;
    ((sl / n).sqrt(), (sr / n).sqrt())
}

fn high_freq_diff_rms(frames: &[[f32; 2]]) -> f64 {
    if frames.len() < 2 {
        return 0.0;
    }
    let mut sum = 0.0_f64;
    for w in frames.windows(2) {
        let dl = f64::from(w[1][0] - w[0][0]);
        let dr = f64::from(w[1][1] - w[0][1]);
        sum += 0.5 * (dl * dl + dr * dr);
    }
    (sum / (frames.len() - 1) as f64).sqrt()
}

#[test]
fn zone_params_commands_macros_mods_plocks_midi_pruning_and_audio_diffs() {
    let dir = tempfile::tempdir().unwrap();
    let sample_rel = "samples/half_sine.wav";
    let sample_abs = dir.path().join(sample_rel);
    let sample_frames = 12_000usize; // 0.25 s at 48 kHz; first 6000 frames silent, second 6000 loud 440 Hz!
    write_half_silent_wav(&sample_abs, 48_000, sample_frames, 440.0);

    let project = build_hybrid_kit_project(sample_rel);
    let save_path = dir.path().join("project.json");
    let lib = cadence_core::sampler::SampleLibrary::from_roots(&[], Some(&save_path));
    let mut app = App::new_with_library(project, save_path, lib).unwrap();
    assert!(app.ready(), "synthetic zone sample must load cleanly");

    // 1. Global dotted `:param` works from anywhere (both zone name & z-id forms).
    app.execute_command("param KIT.BD.filter.cutoff 1200")
        .unwrap();
    app.execute_command("param KIT.z1.gain +6").unwrap();
    app.execute_command("param KIT.SMP.pan R100").unwrap();
    {
        let kit = &app.instrument(10).unwrap().rack;
        assert!((kit.zones[0].gain_db - 6.0).abs() < 1e-4);
        let bd_cutoff = kit
            .read_param_native(InstrumentParam::Module {
                zone: 1,
                module: 2,
                param: ModuleParam::Cutoff,
            })
            .unwrap();
        assert!((bd_cutoff - 1200.0).abs() < 1.0);
        assert!((kit.zones[1].pan - 1.0).abs() < 1e-4);
    }

    // 2. Render audio diff: Zone Gain +6 dB, Zone Pan R100, Synth Zone Cutoff, and frame-based static Macro.
    let base_proj = build_hybrid_kit_project(sample_rel);
    let bank = app.sample_bank();
    assert!(
        bank.iter().any(|(k, _)| *k == module_sample_key(10, 2, 1)),
        "zone 2 Sampler module must be in sample bank"
    );

    let base_score = base_proj.compile().unwrap();
    let (_, base_audio) =
        render_schedule_f32(&[(0, &base_score)], 0, 16, bank.clone(), 48_000).unwrap();
    let bd_window = 0..12_000;
    let smp_start_frame = (8.0 * base_score.step_seconds() * 48_000.0).round() as usize;
    let smp_first_half = smp_start_frame..(smp_start_frame + 5_000);

    // 2a. Zone Gain +6 dB produces measurably louder BD RMS (unclamped linear gain scalar).
    let mut gain_proj = base_proj.clone();
    gain_proj
        .apply(Edit::SetParam {
            address: ParamAddress::Zone {
                instrument: 10,
                zone: 1,
                param: ZoneParam::Gain,
            },
            value: 6.0,
        })
        .unwrap();
    let gain_score = gain_proj.compile().unwrap();
    let (_, gain_audio) =
        render_schedule_f32(&[(0, &gain_score)], 0, 16, bank.clone(), 48_000).unwrap();
    let (base_l, _) = rms_stereo(&base_audio[bd_window.clone()]);
    let (gain_l, _) = rms_stereo(&gain_audio[bd_window.clone()]);
    assert!(
        gain_l > base_l * 1.7,
        "+6 dB ZoneParam::Gain must boost voice output (~2x): base={base_l}, gain={gain_l}"
    );

    // 2b. Zone Pan R100 routes BD energy strictly to Right channel.
    let mut pan_proj = base_proj.clone();
    pan_proj
        .apply(Edit::SetParam {
            address: ParamAddress::Zone {
                instrument: 10,
                zone: 1,
                param: ZoneParam::Pan,
            },
            value: 1.0,
        })
        .unwrap();
    let pan_score = pan_proj.compile().unwrap();
    let (_, pan_audio) =
        render_schedule_f32(&[(0, &pan_score)], 0, 16, bank.clone(), 48_000).unwrap();
    let (pan_l, pan_r) = rms_stereo(&pan_audio[bd_window.clone()]);
    assert!(
        pan_r > 0.05 && pan_l < pan_r * 0.05,
        "ZoneParam::Pan = R100 must pan BD hard right: L={pan_l}, R={pan_r}"
    );

    // 2c. Continuous ModuleParam::Cutoff modulation darkens BD dramatically vs bright base.
    let mut cut_proj = base_proj.clone();
    cut_proj
        .apply(Edit::PutModulation {
            modulation: Modulation {
                id: 90,
                name: "BD Dark".into(),
                target: ModulationTarget::Instrument {
                    instrument: 10,
                    parameter: InstrumentParam::Module {
                        zone: 1,
                        module: 2,
                        param: ModuleParam::Cutoff,
                    },
                },
                pattern: 22, // Hold 0.15 => low cutoff
            },
        })
        .unwrap();
    let cut_score = cut_proj.compile().unwrap();
    let (_, cut_audio) =
        render_schedule_f32(&[(0, &cut_score)], 0, 16, bank.clone(), 48_000).unwrap();
    let hf_base = high_freq_diff_rms(&base_audio[bd_window.clone()]);
    let hf_cut = high_freq_diff_rms(&cut_audio[bd_window.clone()]);
    assert!(
        hf_cut < hf_base * 0.4,
        "Modulated ModuleParam::Cutoff must measurably attenuate HF content: base={hf_base}, cut={hf_cut}"
    );

    // 2d. Static Macro bound to frame-based `ModuleParam::SampleStart` emits a constant ModLane
    // in ScoreWindow so the engine skips the silent first half of `half_sine.wav` and plays the
    // 440 Hz sine immediately at step 8.
    let mut frame_macro_proj = base_proj.clone();
    let mut mac = cadence_core::macros::MacroControl::new(0, "StartMacro");
    mac.value = 1.0;
    mac.bindings.push(cadence_core::macros::MacroBinding {
        target: ParamAddress::Module {
            instrument: 10,
            zone: 2,
            module: 1,
            param: ModuleParam::SampleStart,
        },
        min: 0.0,
        max: 0.55,
    });
    frame_macro_proj
        .apply(Edit::PutMacro { macro_control: mac })
        .unwrap();
    let frame_macro_score = frame_macro_proj.compile().unwrap();
    assert!(
        frame_macro_score.mod_lanes.iter().any(|l| {
            l.target
                == InstrumentParam::Module {
                    zone: 2,
                    module: 1,
                    param: ModuleParam::SampleStart,
                }
        }),
        "static macro on frame-based ModuleParam::SampleStart must emit a constant ModLane"
    );
    let (_, frame_macro_audio) =
        render_schedule_f32(&[(0, &frame_macro_score)], 0, 16, bank.clone(), 48_000).unwrap();
    let (base_smp_l, _) = rms_stereo(&base_audio[smp_first_half.clone()]);
    let (macro_smp_l, _) = rms_stereo(&frame_macro_audio[smp_first_half.clone()]);
    assert!(
        base_smp_l < 0.01 && macro_smp_l > 0.15,
        "static macro SampleStart must skip silent first half: base={base_smp_l}, macro={macro_smp_l}"
    );

    // 2e. Continuous ZoneParam::Pan modulation sweeps a single ringing voice smoothly L -> R!
    let mut sweep_proj = base_proj.clone();
    sweep_proj
        .apply(Edit::PutModulation {
            modulation: Modulation {
                id: 91,
                name: "BD Pan Sweep".into(),
                target: ModulationTarget::Instrument {
                    instrument: 10,
                    parameter: InstrumentParam::Zone {
                        zone: 1,
                        param: ZoneParam::Pan,
                    },
                },
                pattern: 21, // Ramp 0.0 -> 1.0 across 16 steps
            },
        })
        .unwrap();
    if let PatternKind::Notes { steps } = &mut sweep_proj.patterns[0].kind {
        steps[0].gate = 14.0;
        steps[8] = Step::default(); // isolate BD so SMP doesn't overlap centered at step 8
    }
    let sweep_score = sweep_proj.compile().unwrap();
    let (_, sweep_audio) =
        render_schedule_f32(&[(0, &sweep_score)], 0, 16, bank.clone(), 48_000).unwrap();
    let (early_l, early_r) = rms_stereo(&sweep_audio[200..4_000]);
    let (late_l, late_r) = rms_stereo(&sweep_audio[72_000..82_000]);
    assert!(
        early_l > early_r * 2.0 && late_r > late_l * 2.0,
        "Continuous ZoneParam::Pan must sweep a ringing voice from L to R: early=({early_l},{early_r}), late=({late_l},{late_r})"
    );

    // 3. Ex commands: :macro bind, :mod, :link with dotted ZONE.MODULE.PARAM paths!
    app.execute_command("macro 1 bind KIT.BD.filter.cutoff 0.1 0.9")
        .unwrap();
    app.execute_command("macro 1 0.5").unwrap();
    app.execute_command("macro 2 bind instrument KIT z2.sampler.sample_start 0.0 0.5")
        .unwrap();
    app.execute_command("macro 3 bind KIT.BD.vco.tune 0.25 0.75")
        .unwrap();
    app.execute_command("mod BDMod KIT.BD.filter.resonance Sweep")
        .unwrap();
    app.execute_command("mod BDVcoMod KIT.BD.vco.tune Sweep")
        .unwrap();
    app.execute_command("mod SMPMod zone KIT.SMP.filter cutoff LowVal")
        .unwrap();
    app.execute_command("link KitPart BD.filter.cutoff Sweep")
        .unwrap();
    app.execute_command("link KitPart BD.vco.tune Sweep")
        .unwrap();
    app.execute_command("link KitPart KIT.SMP.sampler.sample_start LowVal")
        .unwrap();
    assert_eq!(app.project().part(30).unwrap().parameter_links.len(), 3);
    assert!(app.project().modulations.len() >= 3);
    assert_eq!(app.project().macro_control(0).unwrap().bindings.len(), 1);

    // Verify Signal Graph renders Macro + Modulation edges targeting parent Rack instrument!
    let snap = AudioSnapshot::default();
    app.press_keys("gG").unwrap();
    let fv_graph = app.frame_view(&snap, "test");
    let graph_view = fv_graph
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Graph(gv) => Some(gv),
            _ => None,
        })
        .expect("GraphDeckView");
    let inst_pos = graph_view
        .columns
        .iter()
        .flat_map(|c| c.nodes.iter())
        .find(|n| n.target == GraphTarget::Instrument(10))
        .map(|n| (n.col, n.row))
        .expect("KIT node in graph");
    assert!(
        graph_view
            .edges
            .iter()
            .any(|e| e.kind == GraphEdgeKind::Macro && (e.to_col, e.to_row) == inst_pos),
        "Signal Graph must emit Macro edge from M1 to parent Rack for ParamAddress::Module"
    );
    assert!(
        graph_view
            .edges
            .iter()
            .any(|e| e.kind == GraphEdgeKind::Modulation && (e.to_col, e.to_row) == inst_pos),
        "Signal Graph must emit Modulation edge to parent Rack for InstrumentParam::Module"
    );

    // 4. MIDI Learn + Live MIDI CC Recording on a ParamAddress::Module control in the Rack Panel!
    app.execute_command("instrument KIT").unwrap();
    app.press_keys("gi").unwrap();
    app.execute_command("zone select BD").unwrap();
    app.execute_command("tab panel").unwrap();
    // Pill 0 is always Rack overview, so pill 1 = VCO (#1), pill 2 = Filter (#2).
    app.rack_ui.panel_slot = 2;
    let bd_cutoff_addr = ParamAddress::Module {
        instrument: 10,
        zone: 1,
        module: 2,
        param: ModuleParam::Cutoff,
    };
    let cutoff_idx = app
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::Param(a) if a == bd_cutoff_addr))
        .expect("Cutoff control on BD Filter Panel");
    app.control_cursor = cutoff_idx;
    // Live modulated readout in InstrumentDeckView reflects Macro 1 on BD Cutoff!
    let fv_inst = app.frame_view(&snap, "test");
    let inst_deck = fv_inst
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Instrument(iv) => Some(iv),
            _ => None,
        })
        .expect("InstrumentDeckView");
    assert!(
        inst_deck.controls[cutoff_idx].controlled,
        "BD Cutoff must report controlled=true when bound to Macro 1"
    );
    assert!(
        inst_deck.controls[cutoff_idx].display.contains('→'),
        "Live modulated BD Cutoff should show 'base → live' readout, got '{}'",
        inst_deck.controls[cutoff_idx].display
    );

    // Arm MIDI Learn (`M`) on BD Cutoff, send CC 74, verify mapping + live CC dispatch + Live CC Recording!
    app.press_keys("M").unwrap();
    app.handle_midi_event(MidiMessage::Control {
        channel: 0,
        controller: 74,
        value: 100,
    })
    .unwrap();
    assert!(
        app.midi_badge_for_param(bd_cutoff_addr).is_some(),
        "MIDI Learn must bind CC 74 to ParamAddress::Module"
    );
    // Toggle MIDI CC Recording, start transport, sweep CC 74 across current value (soft-takeover
    // pickup) then move to 40 -> records direct Rack module modulation!
    app.toggle_midi_record();
    app.apply(Action::Play { playing: true }).unwrap();
    let cur_cc = (app.read_param_normalized(bd_cutoff_addr).unwrap() * 127.0).round() as u8;
    app.handle_midi_event(MidiMessage::Control {
        channel: 0,
        controller: 74,
        value: cur_cc,
    })
    .unwrap();
    app.handle_midi_event(MidiMessage::Control {
        channel: 0,
        controller: 74,
        value: 40,
    })
    .unwrap();
    app.apply(Action::Play { playing: false }).unwrap();
    assert!(
        app.project().modulations.iter().any(|m| {
            m.target
                == ModulationTarget::Instrument {
                    instrument: 10,
                    parameter: InstrumentParam::Module {
                        zone: 1,
                        module: 2,
                        param: ModuleParam::Cutoff,
                    },
                }
        }),
        "Live MIDI CC Rec must record a ModulationTarget::Instrument with InstrumentParam::Module"
    );

    // 5. Pruning: deleting zone 2 (SMP) via `:zone del` must cleanly prune SMP's MacroBinding,
    // Modulation, and ParameterLink without failing project validation!
    app.execute_command("zone select SMP").unwrap();
    app.execute_command("zone del").unwrap();
    app.project().validate().unwrap();
    assert!(
        app.project().macro_control(1).unwrap().bindings.is_empty(),
        "Deleting SMP zone must prune its MacroBinding"
    );
    assert!(
        !app.project().modulations.iter().any(|m| matches!(
            m.target,
            ModulationTarget::Instrument {
                instrument: 10,
                parameter: InstrumentParam::Module { zone: 2, .. }
                    | InstrumentParam::Zone { zone: 2, .. }
            }
        )),
        "Deleting SMP zone must prune its Modulation"
    );
    assert!(
        !app.project()
            .part(30)
            .unwrap()
            .parameter_links
            .iter()
            .any(|l| matches!(
                l.target,
                InstrumentParam::Module { zone: 2, .. } | InstrumentParam::Zone { zone: 2, .. }
            )),
        "Deleting SMP zone must prune its ParameterLink"
    );

    // Deleting Filter module via `:mod del Filter` must cleanly prune Filter's Macro, Modulations, and Link!
    app.execute_command("zone select BD").unwrap();
    app.execute_command("mod del Filter").unwrap();
    app.project().validate().unwrap();
    assert!(
        app.project().macro_control(0).unwrap().bindings.is_empty(),
        "Deleting BD Filter module must prune its MacroBinding"
    );
    assert!(
        !app.project().modulations.iter().any(|m| matches!(
            m.target,
            ModulationTarget::Instrument {
                instrument: 10,
                parameter: InstrumentParam::Module {
                    zone: 1,
                    module: 2,
                    ..
                }
            }
        )),
        "Deleting BD Filter module must prune its Modulations"
    );

    // Swapping zone 1 from VCO synth -> Sampler via `:zone preset blank` must prune VCO-only
    // MacroBinding / Modulation / ParameterLink!
    app.execute_command("zone preset blank").unwrap();
    app.project().validate().unwrap();
    assert!(
        app.project().macro_control(2).unwrap().bindings.is_empty(),
        "Swapping BD from VCO synth to Sampler must prune VCO-only MacroBinding"
    );
    assert!(
        !app.project().modulations.iter().any(|m| matches!(
            m.target,
            ModulationTarget::Instrument {
                instrument: 10,
                parameter: InstrumentParam::Module {
                    zone: 1,
                    param: ModuleParam::Tune,
                    ..
                }
            }
        )),
        "Swapping BD from VCO synth to Sampler must prune VCO-only Modulations"
    );
    assert!(
        app.project().part(30).unwrap().parameter_links.is_empty(),
        "Swapping BD from VCO synth to Sampler must prune remaining VCO-only ParameterLinks"
    );
}

#[test]
fn rack_zone_inspector_panel_pages_and_undo_coalescing() {
    let io = Arc::new(MemoryHostIo::default());
    let sample_path = "/virtual/samples/half_sine.wav";
    io.insert_sample(sample_path, synthetic_sample_data(12_000, 48_000, 440.0));

    let project = build_hybrid_kit_project("samples/half_sine.wav");
    let mut app =
        App::new_with_host_io(project, PathBuf::from("/virtual/project.json"), io).unwrap();

    app.execute_command("instrument KIT").unwrap();
    app.press_keys("gi").unwrap();
    assert_eq!(app.rack_ui.tab, RackTab::Panel);

    // Cycle 3 Eurorack tabs cleanly with zero-pinky `L` / `H`: Panel -> Patchbay -> Zones -> Panel!
    app.press_keys("L").unwrap();
    assert_eq!(app.rack_ui.tab, RackTab::Patchbay);
    app.press_keys("L").unwrap();
    assert_eq!(app.rack_ui.tab, RackTab::Zones);
    app.press_keys("L").unwrap();
    assert_eq!(app.rack_ui.tab, RackTab::Panel);

    // Step from Rack overview (pill 0) -> VCO (pill 1) -> Filter (pill 2) on BD zone (`z1`) and verify Drive control!
    app.press_keys("ll").unwrap();
    let drive_addr = ParamAddress::Module {
        instrument: 10,
        zone: 1,
        module: 2,
        param: ModuleParam::Drive,
    };
    let drive_idx = app
        .control_descriptors()
        .iter()
        .position(|c| matches!(c.target, ControlTarget::Param(a) if a == drive_addr))
        .expect("Drive control on BD Filter Panel");
    app.control_cursor = drive_idx;

    // Undo coalescing on repeat nudges inside EDIT: 1 fresh press + 3 repeat presses => ONE undo step!
    let read_drive = |app: &App| {
        app.instrument(10)
            .unwrap()
            .rack
            .read_param_native(InstrumentParam::Module {
                zone: 1,
                module: 2,
                param: ModuleParam::Drive,
            })
            .unwrap()
    };
    let base_drive = read_drive(&app);
    app.handle_key(KeyEvent {
        code: KeyCode::Char('l'),
        modifiers: KeyModifiers::ALT,
        kind: KeyEventKind::Press,
    })
    .unwrap();
    for _ in 0..3 {
        app.handle_key(KeyEvent {
            code: KeyCode::Char('l'),
            modifiers: KeyModifiers::ALT,
            kind: KeyEventKind::Repeat,
        })
        .unwrap();
    }
    let nudged_drive = read_drive(&app);
    assert!(nudged_drive > base_drive + 0.02);
    app.press_keys("u").unwrap();
    let undone_drive = read_drive(&app);
    assert!(
        (undone_drive - base_drive).abs() < 1e-5,
        "Coalesced Rack module param edits must revert in one undo step"
    );

    // Switch to Zones tab (`H`), move to SMP zone (`l`), switch to Panel (`L`) on SMP zone!
    app.press_keys("H").unwrap();
    assert_eq!(app.rack_ui.tab, RackTab::Zones);
    app.press_keys("lL").unwrap();
    assert_eq!(app.rack_ui.tab, RackTab::Panel);
    assert_eq!(app.rack_ui.zone_cursor, 1);

    // Collect every ModuleParam exposed across all 5 module pills (slots 1..=5 after pill 0 Rack overview) of SMP (`Sampler`, `Filter`, `VCA`, `Amp Env`, `Out`).
    let mut exposed = Vec::new();
    for slot in 1..=5 {
        app.rack_ui.panel_slot = slot;
        for c in app.control_descriptors() {
            if let ControlTarget::Param(ParamAddress::Module {
                instrument: 10,
                zone: 2,
                param,
                ..
            }) = c.target
            {
                exposed.push(param);
            }
        }
    }
    for required in [
        ModuleParam::Fine,
        ModuleParam::LoopStart,
        ModuleParam::LoopEnd,
        ModuleParam::LoopXfade,
        ModuleParam::Attack,
        ModuleParam::Decay,
        ModuleParam::Sustain,
        ModuleParam::Release,
        ModuleParam::Resonance,
        ModuleParam::Pan,
    ] {
        assert!(
            exposed.contains(&required),
            "SMP zone Panel pills must expose {required:?}, saw {exposed:?}"
        );
    }

    // `H` cycles cleanly back from Panel -> Zones.
    app.press_keys("H").unwrap();
    assert_eq!(app.rack_ui.tab, RackTab::Zones);
}

#[test]
fn group_to_group_routing_ui_cycler_cycle_rejection_graph_and_stems() {
    let dir = tempfile::tempdir().unwrap();
    let p = Project::demo();
    let proj_path = dir.path().join("project.json");
    let lib = cadence_core::sampler::SampleLibrary::from_roots(&[], Some(&proj_path));
    let mut app = App::new_with_library(p, proj_path, lib).unwrap();

    // Create two Group buses: SubBus and MasterBus.
    app.execute_command("channel group SubBus").unwrap();
    app.execute_command("channel group MasterBus").unwrap();
    let sub_id = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "SubBus")
        .unwrap()
        .id;
    let master_bus_id = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "MasterBus")
        .unwrap()
        .id;
    let first_audio_id = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.kind == ChannelKind::Audio)
        .unwrap()
        .id;

    // Route Audio -> SubBus -> MasterBus -> Master via `:route`.
    let audio_name = app.channel(first_audio_id).unwrap().name.clone();
    app.execute_command(&format!("route {audio_name} SubBus"))
        .unwrap();
    app.execute_command("route SubBus MasterBus").unwrap();
    assert_eq!(
        app.channel(sub_id).unwrap().output,
        ChannelRef::Channel(master_bus_id)
    );

    // Self-loop and 2-hop cycle are rejected with clear errors!
    let self_err = app
        .execute_command("route SubBus SubBus")
        .unwrap_err()
        .to_string();
    assert!(
        self_err.to_ascii_lowercase().contains("itself")
            || self_err.to_ascii_lowercase().contains("cycle"),
        "self-loop error: {self_err}"
    );
    let cycle_err = app
        .execute_command("route MasterBus SubBus")
        .unwrap_err()
        .to_string();
    assert!(
        cycle_err.to_ascii_lowercase().contains("cycle"),
        "2-hop cycle error: {cycle_err}"
    );

    // UI ChannelOutput cycler on MasterBus excludes SubBus (would form cycle!) and MasterBus itself.
    app.execute_command("channel MasterBus").unwrap();
    app.press_keys("gd").unwrap();
    let out_idx = app
        .control_descriptors_for(Detail::Channel(ChannelRef::Channel(master_bus_id)))
        .iter()
        .position(|c| matches!(c.target, ControlTarget::ChannelOutput(id) if id == master_bus_id))
        .expect("ChannelOutput control on Group channel");
    let valid =
        cadence_core::mixer::valid_output_targets(&app.project().mixer.channels, master_bus_id);
    assert!(valid.contains(&ChannelRef::Master));
    assert!(!valid.contains(&ChannelRef::Channel(master_bus_id)));
    assert!(!valid.contains(&ChannelRef::Channel(sub_id)));
    assert!(out_idx < 16);

    // Signal Graph renders SubBus -> MasterBus and MasterBus -> Master routing cables!
    let snap = AudioSnapshot::default();
    app.press_keys("gG").unwrap();
    let fv = app.frame_view(&snap, "test");
    let gv = fv
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Graph(g) => Some(g),
            _ => None,
        })
        .expect("GraphDeckView");
    let pos_of = |target: GraphTarget| {
        gv.columns
            .iter()
            .flat_map(|c| c.nodes.iter())
            .find(|n| n.target == target)
            .map(|n| (n.col, n.row))
            .unwrap()
    };
    let sub_pos = pos_of(GraphTarget::Channel(ChannelRef::Channel(sub_id)));
    let mb_pos = pos_of(GraphTarget::Channel(ChannelRef::Channel(master_bus_id)));
    let mst_pos = pos_of(GraphTarget::Channel(ChannelRef::Master));
    assert!(gv.edges.iter().any(|e| {
        e.kind == GraphEdgeKind::Routing
            && (e.from_col, e.from_row) == sub_pos
            && (e.to_col, e.to_row) == mb_pos
    }));
    assert!(gv.edges.iter().any(|e| {
        e.kind == GraphEdgeKind::Routing
            && (e.from_col, e.from_row) == mb_pos
            && (e.to_col, e.to_row) == mst_pos
    }));

    // Stem export: muting grandparent MasterBus skips `first_audio_id` stem cleanly!
    let mut flags = LiveFlags::default();
    flags.muted_channels.insert(master_bus_id);
    let stems_dir = dir.path().join("stems");
    let report = render_stems_with_samples_report(
        app.project(),
        &flags,
        Vec::new(),
        &stems_dir,
        0,
        16,
        24_000,
    )
    .unwrap();
    assert!(
        !report
            .stem_paths
            .iter()
            .any(|p| p.to_string_lossy().contains(&audio_name)),
        "Audio channel routed through muted grandparent Group must be skipped in stems: {:?}",
        report.stem_paths
    );
}

#[test]
fn modulation_target_send_compilation_views_param_and_midi_cc() {
    let mut app = App::new_in_memory(Project::demo(), "unused.json".into()).unwrap();
    let src_ch = app.project().mixer.channels[0].id;
    let ret_ch = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.kind == ChannelKind::Return)
        .unwrap()
        .id;

    app.execute_command("pattern values SendRamp 16 ramp 0:0.1 8:0.9")
        .unwrap();
    let pat_id = app
        .project()
        .patterns
        .iter()
        .find(|p| p.name == "SendRamp")
        .unwrap()
        .id;

    // 1. Validation rejects Return as source and Audio as target.
    let mut bad = app.project().clone();
    bad.modulations.push(Modulation {
        id: 9001,
        name: "BadSrc".into(),
        target: ModulationTarget::Send {
            channel: ChannelRef::Channel(ret_ch),
            target: ret_ch,
        },
        pattern: pat_id,
    });
    assert!(bad.validate().is_err());

    let mut bad2 = app.project().clone();
    bad2.modulations.push(Modulation {
        id: 9002,
        name: "BadDst".into(),
        target: ModulationTarget::Send {
            channel: ChannelRef::Channel(src_ch),
            target: src_ch,
        },
        pattern: pat_id,
    });
    assert!(bad2.validate().is_err());

    // 2. Ex command `:mod SendAuto send <CH> <RET> SendRamp` compiles & synthesizes MixerParamTarget::Send.
    let src_name = app.channel(src_ch).unwrap().name.clone();
    let ret_name = app.channel(ret_ch).unwrap().name.clone();
    app.execute_command(&format!("mod SendAuto send {src_name} {ret_name} SendRamp"))
        .unwrap();

    let compiled = app.project().compile().unwrap();
    let target = MixerParamTarget::Send {
        channel: ChannelRef::Channel(src_ch),
        target: ret_ch,
    };
    assert!((compiled.resolve_mixer_param(target, 0.0) - 0.1).abs() < 1e-4);
    assert!((compiled.resolve_mixer_param(target, 8.0) - 0.9).abs() < 1e-4);

    // 3. Projects to MixerDeckView, Detail::Channel controls, and GraphDeckView.
    app.press_keys("gm").unwrap();
    let mview = app
        .frame_view(&AudioSnapshot::default(), "ready")
        .decks
        .into_iter()
        .find_map(|d| match d.view {
            DeckView::Mixer(m) => Some(m),
            _ => None,
        })
        .expect("Mixer deck visible after gm");
    let ch_view = mview
        .channels
        .iter()
        .find(|c| c.channel == ChannelRef::Channel(src_ch))
        .unwrap();
    assert!(
        ch_view
            .sends
            .iter()
            .any(|(n, amt)| n == &ret_name && (*amt - 0.1).abs() < 1e-3),
        "MixerDeckView must project live modulated Send amount"
    );

    app.press_keys("gG").unwrap();
    let graph = app
        .frame_view(&AudioSnapshot::default(), "ready")
        .decks
        .into_iter()
        .find_map(|d| match d.view {
            DeckView::Graph(g) => Some(g),
            _ => None,
        })
        .expect("Graph deck visible after gG");
    assert!(
        graph.edges.iter().any(|e| {
            e.kind == GraphEdgeKind::Modulation
                && graph.columns[e.from_col].nodes[e.from_row].target
                    == GraphTarget::Pattern(pat_id)
                && graph.columns[e.to_col].nodes[e.to_row].target
                    == GraphTarget::Channel(ChannelRef::Channel(src_ch))
        }),
        "GraphDeckView must show Modulation edge from SendRamp to source channel"
    );
    assert!(
        graph.edges.iter().any(|e| {
            e.kind == GraphEdgeKind::Routing
                && graph.columns[e.from_col].nodes[e.from_row].target
                    == GraphTarget::Channel(ChannelRef::Channel(src_ch))
                && graph.columns[e.to_col].nodes[e.to_row].target
                    == GraphTarget::Channel(ChannelRef::Channel(ret_ch))
        }),
        "GraphDeckView must show Routing edge from source channel to Return"
    );

    // 4. Enter on Send row in Detail::Channel opens unified ValueInput, and `:param` writes it too.
    app.execute_command(&format!("channel {src_name}")).unwrap();
    let controls = app.control_descriptors();
    let send_row = controls
        .iter()
        .position(|c| c.group == "SENDS" && c.controlled)
        .expect("modulated Send row in Detail::Channel");
    app.control_cursor = send_row;
    app.press_keys("i<Enter>").unwrap();
    assert_eq!(app.mode_name(), "VALUE ENTRY");
    assert!(app.value_input_view().unwrap().subtitle.contains("SEND"));
    app.press_keys("<Esc>").unwrap();
    app.execute_command(&format!("param send:{ret_ch} 45%"))
        .unwrap();
    let send_val = app
        .channel(src_ch)
        .unwrap()
        .sends
        .iter()
        .find(|s| s.target == ret_ch)
        .unwrap()
        .amount;
    assert!((send_val - 0.45).abs() < 1e-4);

    // 5. Live MIDI CC recording (`R`) on ParamAddress::Send records into ModulationTarget::Send.
    let ch2 = app.project().mixer.channels[1].id;
    let send_addr = ParamAddress::Send {
        channel: ChannelRef::Channel(ch2),
        target: ret_ch,
    };
    app.config.midi_mappings.push(MidiMapping {
        id: 501,
        label: "Ch2 Send".into(),
        source: MidiSource::Cc {
            channel: None,
            cc: 91,
        },
        target: MidiTarget::Param { address: send_addr },
        mode: MidiValueMode::Absolute,
        min: 0.0,
        max: 1.0,
        invert: false,
        pickup: false,
    });
    app.apply(Action::Play { playing: true }).unwrap();
    app.toggle_midi_record();
    let _ = app.handle_midi_event(MidiMessage::Control {
        channel: 0,
        controller: 91,
        value: 100,
    });
    assert!(
        app.project().modulations.iter().any(|m| {
            m.target
                == ModulationTarget::Send {
                    channel: ChannelRef::Channel(ch2),
                    target: ret_ch,
                }
        }),
        "Live MIDI CC recording must create ModulationTarget::Send"
    );
}

#[test]
fn unified_param_ops_aliases_pan_sends_effect_by_name_and_gate_ceiling() {
    let mut app = App::new_in_memory(Project::demo(), "param_ops.json".into()).unwrap();
    let ch_name = app.project().mixer.channels[0].name.clone();
    let ch_id = app.project().mixer.channels[0].id;

    // 1. Channel gain/level/trim aliases work identically in `:param`.
    app.execute_command(&format!("channel {ch_name}")).unwrap();
    app.press_keys("gm<Enter>").unwrap();
    app.execute_command("param gain 0.42").unwrap();
    assert!((app.channel(ch_id).unwrap().strip.gain - 0.42).abs() < 1e-3);
    app.execute_command("param level 55%").unwrap();
    assert!((app.channel(ch_id).unwrap().strip.gain - 0.55).abs() < 1e-3);
    app.execute_command("param trim 0.73").unwrap();
    assert!((app.channel(ch_id).unwrap().strip.gain - 0.73).abs() < 1e-3);

    // 2. Unified pan formats: C, L50, R50, -0.5, -50%.
    for (token, expected) in [
        ("L50", -0.5_f32),
        ("R50", 0.5),
        ("C", 0.0),
        ("-0.5", -0.5),
        ("-50%", -0.5),
    ] {
        app.execute_command(&format!("param pan {token}")).unwrap();
        let actual = app.channel(ch_id).unwrap().strip.pan;
        assert!(
            (actual - expected).abs() < 1e-3,
            "pan '{token}': expected {expected}, got {actual}"
        );
    }

    // 3. ParamAddress::Send via `:param send:<RETURN>` and `:param send <RETURN>`.
    app.execute_command("channel return Verb").unwrap();
    let ret_id = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "Verb")
        .unwrap()
        .id;
    app.execute_command(&format!("channel {ch_name}")).unwrap();
    app.press_keys("gm<Enter>").unwrap();
    app.execute_command("param send:Verb 40%").unwrap();
    let s1 = app
        .channel(ch_id)
        .unwrap()
        .sends
        .iter()
        .find(|s| s.target == ret_id)
        .unwrap()
        .amount;
    assert!((s1 - 0.4).abs() < 1e-3);
    app.execute_command("param send Verb 0.65").unwrap();
    let s2 = app
        .channel(ch_id)
        .unwrap()
        .sends
        .iter()
        .find(|s| s.target == ret_id)
        .unwrap()
        .amount;
    assert!((s2 - 0.65).abs() < 1e-3);

    // 4. Effect lookup by name as well as ID in `:fx` and `:mod`.
    app.execute_command(&format!("fx add filter {ch_name}"))
        .unwrap();
    let fx_dev = app.channel(ch_id).unwrap().inserts.last().unwrap().clone();
    app.execute_command(&format!("fx bypass \"{}\"", fx_dev.name))
        .unwrap();
    assert!(app.effect(fx_dev.id).unwrap().1.bypassed);
    app.execute_command(&format!("fx bypass {}", fx_dev.id))
        .unwrap();
    assert!(!app.effect(fx_dev.id).unwrap().1.bypassed);

    app.execute_command("pattern values FxSweep 16 ramp 0:0.2 8:0.9")
        .unwrap();
    app.execute_command(&format!(
        "mod FxCutMod effect \"{}\" cutoff FxSweep",
        fx_dev.name
    ))
    .unwrap();
    assert!(app.project().modulations.iter().any(|m| {
        m.target
            == ModulationTarget::Effect {
                device: fx_dev.id,
                parameter: EffectParam::Cutoff,
            }
    }));

    // 5. Step.gate reaches the 64.0-step ceiling.
    app.execute_command("pattern notes LongGate 0 . . .")
        .unwrap();
    app.press_keys("gp").unwrap();
    for _ in 0..70 {
        app.press_keys("<m-L>").unwrap();
    }
    let pat = app
        .project()
        .patterns
        .iter()
        .find(|p| p.name == "LongGate")
        .unwrap();
    let gate = match &pat.kind {
        PatternKind::Notes { steps } => steps[0].gate,
        other => panic!("expected Notes, got {other:?}"),
    };
    assert!(
        gate > 16.5 && (gate - 64.0).abs() < 1e-3,
        "Step.gate must reach 64.0 ceiling, got {gate}"
    );
}
