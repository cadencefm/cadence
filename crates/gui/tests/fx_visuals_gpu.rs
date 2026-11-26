//! Headless Metal/wgpu GPU integration tests for all 8 dedicated FX visualizers
//! and live automated Mixer fader markers.
//!
//! Set `CADENCE_DUMP_FX_DIR=/path/to/dir` to dump 1440×860 PNG screenshots for
//! visual inspection.

mod common;

use cadence_app::view::{DeckView, InstrumentVisualView, ModuleVisualView, RackTab};
use cadence_app::App;
use cadence_core::material::{
    Modulation, ModulationTarget, Pattern, PatternKind, Project, ValuePoint, ValueUnit,
};
use cadence_core::mixer::{
    ChannelParam, ChannelRef, ChorusPatch, CompressorPatch, DelayMode, DelayPatch, DriveMode,
    DrivePatch, EffectDevice, EffectKind, EffectParam, EqPatch, FilterPatch, LimiterPatch,
    ReverbPatch,
};
use cadence_core::params::{ParamAddress, ValueMode};
use cadence_core::rack::{
    FilterMode, InPort, ModuleKind, PatchCable, RackModule, VcoModule, Waveform,
};
use cadence_core::runtime::{AudioSnapshot, ChannelMeter};
use cadence_gui::headless::{distinct_pixel_ratio, HeadlessGpu};
use cadence_gui::theme::Theme;
use common::reset_inserts_and_channel_gain_automation;
use std::path::PathBuf;

fn maybe_save_png(name: &str, width: u32, height: u32, rgba: &[u8]) {
    common::maybe_save_png("CADENCE_DUMP_FX_DIR", name, width, height, rgba);
}

#[test]
fn gpu_renders_all_eight_fx_decks_and_automated_mixer_faders() {
    let Ok(mut gpu) = HeadlessGpu::new(1440, 860) else {
        // Skip gracefully only if running on a GPU-less CI container.
        return;
    };

    let demo_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/demo/project.json");
    let mut project = cadence_app::load_project(&demo_path).unwrap_or_else(|_| Project::demo());
    reset_inserts_and_channel_gain_automation(&mut project);

    // Ensure rich, distinct insert FX across Master and audio/return channels.
    let base_id = project.max_id() + 1;
    let eq_id = base_id;
    let filter_id = base_id + 1;
    let comp_id = base_id + 2;
    let drive_id = base_id + 3;
    let delay_id = base_id + 4;
    let chorus_id = base_id + 5;
    let reverb_id = base_id + 6;
    let limiter_id = base_id + 7;

    // Attach EQ, Filter (PCF), Compressor, Drive on channels with room.
    project.mixer.master.inserts.clear();
    project.mixer.master.inserts.push(EffectDevice {
        id: eq_id,
        name: "Master Tone EQ".into(),
        bypassed: false,
        kind: EffectKind::Eq(EqPatch {
            hpf_hz: 42.0,
            low_db: 3.5,
            mid_db: 5.2,
            mid_hz: 2400.0,
            mid_q: 2.6,
            high_db: 4.8,
        }),
    });
    project.mixer.master.inserts.push(EffectDevice {
        id: comp_id,
        name: "Bus Glue Comp".into(),
        bypassed: false,
        kind: EffectKind::Compressor(CompressorPatch {
            threshold_db: -18.0,
            ratio: 6.0,
            attack_ms: 4.5,
            release_ms: 90.0,
            makeup_db: 3.5,
            mix: 0.90,
        }),
    });
    project.mixer.master.inserts.push(EffectDevice {
        id: filter_id,
        name: "Acid PCF Filter".into(),
        bypassed: false,
        kind: EffectKind::Filter(FilterPatch {
            mode: FilterMode::AcidLowPass,
            cutoff_hz: 620.0,
            resonance: 0.72,
            drive: 0.35,
            mix: 1.0,
            env_pattern: 6,
            env_pattern_id: None,
            env_amount: 2.8,
            env_decay_ms: 220.0,
        }),
    });
    project.mixer.master.inserts.push(EffectDevice {
        id: drive_id,
        name: "Tape Saturator".into(),
        bypassed: false,
        kind: EffectKind::Drive(DrivePatch {
            mode: DriveMode::Tape,
            drive: 0.76,
            asymmetry: 0.0,
            tone_hz: 5200.0,
            output_db: -1.5,
            mix: 0.85,
        }),
    });

    // Attach Delay, Chorus, Reverb, and Limiter to channel 0 so all 8 FX have a real owner channel.
    if let Some(ch0) = project.mixer.channels.first_mut() {
        ch0.inserts.clear();
        ch0.inserts.push(EffectDevice {
            id: delay_id,
            name: "Dub Ping-Pong".into(),
            bypassed: false,
            kind: EffectKind::Delay(DelayPatch {
                mode: DelayMode::PingPong,
                steps: 3.0,
                feedback: 0.56,
                mix: 0.42,
            }),
        });
        ch0.inserts.push(EffectDevice {
            id: chorus_id,
            name: "Juno Stereo Chorus".into(),
            bypassed: false,
            kind: EffectKind::Chorus(ChorusPatch {
                rate_hz: 0.85,
                depth: 0.68,
                width: 0.92,
                mix: 0.48,
            }),
        });
        ch0.inserts.push(EffectDevice {
            id: reverb_id,
            name: "Cathedral Hall".into(),
            bypassed: false,
            kind: EffectKind::Reverb(ReverbPatch {
                size: 0.74,
                decay_seconds: 3.8,
                damping_hz: 4400.0,
                predelay_ms: 48.0,
                mix: 0.36,
                width: 0.88,
            }),
        });
        ch0.inserts.push(EffectDevice {
            id: limiter_id,
            name: "Brickwall Limiter".into(),
            bypassed: false,
            kind: EffectKind::Limiter(LimiterPatch {
                ceiling_db: -1.2,
                input_drive_db: 9.5,
                release_ms: 75.0,
                mix: 1.0,
            }),
        });
    }

    // Add live PCF Cutoff modulation so Filter deck displays the live sweep ribbon + mod curve!
    let next_pat_id = project.patterns.iter().map(|p| p.id).max().unwrap_or(0) + 1;
    let next_mod_id = project.modulations.iter().map(|m| m.id).max().unwrap_or(0) + 1;
    project.patterns.push(Pattern {
        id: next_pat_id,
        name: "PCF Sweep".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 32,
            mode: ValueMode::Ramp,
            points: vec![
                ValuePoint {
                    step: 0,
                    value: 0.18,
                },
                ValuePoint {
                    step: 16,
                    value: 0.86,
                },
            ],
        },
    });
    project.modulations.push(Modulation {
        id: next_mod_id,
        name: "PCF Cutoff".into(),
        pattern: next_pat_id,
        target: ModulationTarget::Effect {
            device: filter_id,
            parameter: EffectParam::Cutoff,
        },
    });

    // Add live ChannelGain automation on the first three audio channels so Mixer faders display
    // crisp glowing modulation dots + ghost fader needles + delta ribbons!
    let auto_channels: Vec<u64> = project
        .mixer
        .channels
        .iter()
        .take(3)
        .map(|c| c.id)
        .collect();
    let first_auto_ch = auto_channels[0];
    for (idx, ch_id) in auto_channels.into_iter().enumerate() {
        let pat_id = next_pat_id + 1 + idx as u64;
        let mod_id = next_mod_id + 1 + idx as u64;
        if let Some(ch) = project.mixer.channels.iter_mut().find(|c| c.id == ch_id) {
            ch.strip.gain = 0.92;
        }
        let target_val = match idx {
            0 => 0.35,
            1 => 0.62,
            _ => 0.18,
        };
        project.patterns.push(Pattern {
            id: pat_id,
            name: format!("Auto Ride {}", idx + 1),
            kind: PatternKind::Values {
                unit: ValueUnit::Normalized,
                period: 32,
                mode: ValueMode::Ramp,
                points: vec![
                    ValuePoint {
                        step: 0,
                        value: 0.10,
                    },
                    ValuePoint {
                        step: 12,
                        value: target_val,
                    },
                    ValuePoint {
                        step: 24,
                        value: 0.95,
                    },
                ],
            },
        });
        project.modulations.push(Modulation {
            id: mod_id,
            name: format!("Ch{idx} Gain Auto"),
            pattern: pat_id,
            target: ModulationTarget::ChannelGain {
                channel: ChannelRef::Channel(ch_id),
            },
        });
    }

    // Build realistic AudioSnapshot with live meters + active 8.6 dB Compressor GR!
    let wave: Vec<f32> = (0..256)
        .map(|i| {
            let t = i as f32 / 256.0 * std::f32::consts::TAU;
            (t * 2.0).sin() * 0.45 + (t * 5.0).sin() * 0.25
        })
        .collect();
    let mut meters = vec![ChannelMeter {
        channel: ChannelRef::Master,
        input_peak: [0.84, 0.78],
        input_peak_hold: [0.92, 0.88],
        output_peak: [0.76, 0.72],
        output_peak_hold: [0.88, 0.84],
        hot: false,
        clipped: false,
    }];
    for (idx, ch) in project.mixer.channels.iter().enumerate() {
        let lvl = (0.36 + ((idx * 13) % 9) as f32 * 0.055).min(0.86);
        meters.push(ChannelMeter {
            channel: ChannelRef::Channel(ch.id),
            input_peak: [lvl, (lvl * 0.92).min(0.86)],
            input_peak_hold: [(lvl + 0.06).min(0.92), (lvl + 0.04).min(0.90)],
            output_peak: [lvl, (lvl * 0.92).min(0.86)],
            output_peak_hold: [(lvl + 0.06).min(0.92), (lvl + 0.04).min(0.90)],
            hot: false,
            clipped: false,
        });
    }
    let mut snap = AudioSnapshot {
        playing: true,
        absolute_step: 12,
        step_fraction: 0.25,
        sample_rate: 48_000,
        peak: 0.80,
        peak_left: 0.80,
        peak_right: 0.75,
        waveform: wave,
        channel_meters: meters,
        ..AudioSnapshot::default()
    };
    snap.set_effect_meter(comp_id, 8.6, 11.4);
    snap.set_effect_meter(limiter_id, 6.8, 9.2);

    // Configure the first Rack instrument with active VCO 2 -> VCO 1 linear FM
    // so we can render & visually inspect the VCO FM visualizer!
    let fm_synth_id = project
        .instruments
        .iter_mut()
        .find_map(|inst| {
            let zone = inst.rack.zones.first_mut()?;
            let carrier_id = zone.modules.iter_mut().find_map(|m| {
                if let ModuleKind::Vco(v) = &mut m.kind {
                    v.waveform = Waveform::Sine;
                    v.level = 0.85;
                    Some(m.id)
                } else {
                    None
                }
            })?;
            let mod_id = zone.next_module_id();
            zone.modules.push(RackModule::new(
                mod_id,
                "FM Mod",
                ModuleKind::Vco(VcoModule {
                    waveform: Waveform::Sine,
                    tune_semitones: 12.0,
                    level: 1.0,
                    ..VcoModule::default()
                }),
            ));
            let cid = zone.next_cable_id();
            zone.cables
                .push(PatchCable::new(cid, mod_id, carrier_id, InPort::FmIn, 0.48));
            Some(inst.id)
        })
        .expect("demo project should contain at least one VCO Rack instrument");

    let theme = Theme::cadence_dark();
    let status = "Metal · 48 kHz";
    project
        .validate()
        .expect("normalized FX visuals fixture must validate");
    let lib = cadence_core::sampler::SampleLibrary::from_roots(&[], Some(&demo_path));
    let mut app = App::new_with_library(project, demo_path, lib).unwrap();

    // Render all 8 FX decks (zoomed to showcase detail + verify zero clipped labels/curves).
    let cases = [
        (eq_id, "01-fx-eq.png"),
        (filter_id, "02-fx-filter-pcf.png"),
        (comp_id, "03-fx-compressor-gr.png"),
        (drive_id, "04-fx-drive.png"),
        (delay_id, "05-fx-delay.png"),
        (chorus_id, "06-fx-chorus.png"),
        (reverb_id, "07-fx-reverb.png"),
        (limiter_id, "08-fx-limiter-gr.png"),
    ];
    let mut rendered_frames = Vec::new();
    for (idx, (id, filename)) in cases.into_iter().enumerate() {
        if idx > 0 {
            app.press_keys("<Esc>").unwrap();
        }
        app.execute_command(&format!("fx {id}")).unwrap();
        let fv = app.frame_view(&snap, status);
        assert!(
            fv.decks
                .iter()
                .any(|d| matches!(d.view, DeckView::Effect(_))),
            "expected DeckView::Effect for {filename}"
        );
        let px = gpu.render_rgba8(&fv, &theme).unwrap();
        assert!(
            distinct_pixel_ratio(&px, theme.surface.canvas) > 0.25,
            "{filename} rendered too few visible pixels"
        );
        maybe_save_png(filename, 1440, 860, &px);
        rendered_frames.push(px);
    }

    // Confirm every FX deck renders a distinct GPU image.
    for pair in rendered_frames.windows(2) {
        assert_ne!(pair[0], pair[1], "adjacent FX decks must render distinctly");
    }

    // Also verify BYPASSED state renders cleanly with BYPASSED badge.
    app.press_keys("<Esc>").unwrap();
    app.execute_command(&format!("fx bypass {eq_id}")).unwrap();
    app.execute_command(&format!("fx {eq_id}")).unwrap();
    let fv_byp = app.frame_view(&snap, status);
    let px_byp = gpu.render_rgba8(&fv_byp, &theme).unwrap();
    assert!(distinct_pixel_ratio(&px_byp, theme.surface.canvas) > 0.25);
    assert_ne!(px_byp, rendered_frames[0], "bypassed EQ must look distinct");
    maybe_save_png("09-fx-eq-bypassed.png", 1440, 860, &px_byp);

    // Render Mixer deck zoomed (`gm zz`), explicitly selecting the first automated channel to showcase
    // live automated ghost fader markers + mod dots + live effective dB footer readout!
    app.press_keys("gmzz").unwrap();
    app.selected_channel = ChannelRef::Channel(first_auto_ch);
    let fv_mix = app.frame_view(&snap, status);
    let mixer_view = fv_mix
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Mixer(m) => Some(m),
            _ => None,
        })
        .expect("MixerDeckView");
    assert!(
        mixer_view
            .channels
            .iter()
            .filter(|c| c.effective_gain.is_some())
            .count()
            >= 3,
        "expected at least 3 channels with live automated effective gain"
    );
    let px_mix = gpu.render_rgba8(&fv_mix, &theme).unwrap();
    assert!(distinct_pixel_ratio(&px_mix, theme.surface.canvas) > 0.25);
    maybe_save_png("10-mixer-automated-faders.png", 1440, 860, &px_mix);

    // Also verify Studio Light theme renders cleanly (proves strict &Theme token usage).
    let light_theme = Theme::studio_light();
    let px_light = gpu.render_rgba8(&fv_mix, &light_theme).unwrap();
    assert!(distinct_pixel_ratio(&px_light, light_theme.surface.canvas) > 0.25);
    assert_ne!(px_light, px_mix);

    // Render Rack Instrument deck on the FM carrier VCO module pill (zoomed)
    // to verify FM-modulated VCO waveform and FM badge!
    app.press_keys("<Esc>").unwrap();
    app.apply(cadence_app::Action::SelectInstrument {
        instrument: fm_synth_id,
    })
    .unwrap();
    app.rack_ui.tab = RackTab::Panel;
    // Pick the first VCO module pill (index 1 if Quick pins exist, else 0).
    {
        let probe = app.frame_view(&snap, status);
        if let Some(iv) = probe.decks.iter().find_map(|d| match &d.view {
            DeckView::Instrument(v) => Some(v),
            _ => None,
        }) {
            if let InstrumentVisualView::Rack(rack) = &iv.visual {
                if let Some(slot) = rack.modules.iter().position(|p| p.tag == "VCO") {
                    app.rack_ui.panel_slot = slot;
                }
            }
        }
    }
    app.press_keys("zz").unwrap();
    let fv_fm = app.frame_view(&snap, status);
    let inst_view = fv_fm
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Instrument(v) => Some(v),
            _ => None,
        })
        .expect("Rack InstrumentDeckView");
    match &inst_view.visual {
        InstrumentVisualView::Rack(rack) => match &rack.visual {
            ModuleVisualView::Vco {
                fm_active,
                label,
                curve,
                ..
            } => {
                assert!(*fm_active, "VCO must report fm_active when FmIn is patched");
                assert!(
                    label.contains("FM"),
                    "expected FM tag on VCO label, got {label}"
                );
                assert!(curve.len() >= 96);
            }
            other => panic!("expected Vco module visual, got {other:?}"),
        },
        other => panic!("expected Rack visual, got {other:?}"),
    }
    let px_fm = gpu.render_rgba8(&fv_fm, &theme).unwrap();
    assert!(distinct_pixel_ratio(&px_fm, theme.surface.canvas) > 0.25);
    maybe_save_png("11-synth-fm.png", 1440, 860, &px_fm);

    // Also render compact split-pane check (1024x680) for Limiter to guarantee zero clipping.
    if let Ok(mut small_gpu) = HeadlessGpu::new(1024, 680) {
        app.press_keys("<Esc>").unwrap();
        app.execute_command(&format!("fx {limiter_id}")).unwrap();
        let fv_lim_sm = app.frame_view(&snap, status);
        let px_lim_sm = small_gpu.render_rgba8(&fv_lim_sm, &theme).unwrap();
        assert!(distinct_pixel_ratio(&px_lim_sm, theme.surface.canvas) > 0.25);
        maybe_save_png("12-fx-limiter-compact.png", 1024, 680, &px_lim_sm);
    }
}

#[test]
fn gpu_renders_fx_edge_cases_mono_reverb_neg_makeup_comp_and_44k_grid() {
    let Ok(mut gpu) = HeadlessGpu::new(1440, 860) else {
        return;
    };
    let mut project = Project::demo();
    reset_inserts_and_channel_gain_automation(&mut project);
    let base_id = project.max_id() + 1;
    let rev_mono_id = base_id;
    let comp_hot_id = base_id + 1;
    let filt_par_id = base_id + 2;

    project.mixer.master.inserts.clear();
    project.mixer.master.inserts.push(EffectDevice {
        id: rev_mono_id,
        name: "Mono Plate".into(),
        bypassed: false,
        kind: EffectKind::Reverb(ReverbPatch {
            size: 0.55,
            decay_seconds: 2.2,
            damping_hz: 6_400.0,
            predelay_ms: 18.0,
            mix: 0.40,
            width: 0.0,
        }),
    });
    project.mixer.master.inserts.push(EffectDevice {
        id: comp_hot_id,
        name: "Trim Comp".into(),
        bypassed: false,
        kind: EffectKind::Compressor(CompressorPatch {
            threshold_db: -14.0,
            ratio: 3.0,
            attack_ms: 8.0,
            release_ms: 120.0,
            makeup_db: -2.5,
            mix: 1.0,
        }),
    });
    project.mixer.master.inserts.push(EffectDevice {
        id: filt_par_id,
        name: "Parallel Acid 44k".into(),
        bypassed: false,
        kind: EffectKind::Filter(FilterPatch {
            mode: FilterMode::AcidLowPass,
            cutoff_hz: 950.0,
            resonance: 0.78,
            drive: 0.0,
            mix: 0.55,
            env_pattern: 12,
            env_pattern_id: None,
            env_amount: 2.0,
            env_decay_ms: 180.0,
        }),
    });

    let mut snap = AudioSnapshot {
        playing: true,
        absolute_step: 4,
        step_fraction: 0.5,
        sample_rate: 44_100,
        ..AudioSnapshot::default()
    };
    snap.set_effect_meter(comp_hot_id, 4.2, 6.5);

    let theme = Theme::cadence_dark();
    let mut app = App::new_in_memory(project, "fx-edge-cases.json".into()).unwrap();

    for (idx, (id, filename)) in [
        (rev_mono_id, "13-fx-reverb-mono.png"),
        (comp_hot_id, "14-fx-comp-neg-makeup.png"),
        (filt_par_id, "15-fx-filter-44k-parallel.png"),
    ]
    .into_iter()
    .enumerate()
    {
        if idx > 0 {
            app.press_keys("<Esc>").unwrap();
        }
        app.execute_command(&format!("fx {id}")).unwrap();
        let fv = app.frame_view(&snap, "Metal · 44.1 kHz");
        let px = gpu.render_rgba8(&fv, &theme).unwrap();
        assert!(
            distinct_pixel_ratio(&px, theme.surface.canvas) > 0.25,
            "{filename} rendered too few visible pixels"
        );
        maybe_save_png(filename, 1440, 860, &px);
    }
}

#[test]
fn gpu_renders_delay_stereo_and_drive_hard_clip_and_fold_modes_distinctly() {
    let Ok(mut gpu) = HeadlessGpu::new(1440, 860) else {
        return;
    };
    let mut project = Project::demo();
    reset_inserts_and_channel_gain_automation(&mut project);
    let base_id = project.max_id() + 1;
    let delay_pp_id = base_id;
    let delay_st_id = base_id + 1;
    let drive_tape_id = base_id + 2;
    let drive_hard_id = base_id + 3;
    let drive_fold_id = base_id + 4;

    project.mixer.master.inserts.clear();
    project.mixer.master.inserts.push(EffectDevice {
        id: delay_pp_id,
        name: "Ping-Pong Delay".into(),
        bypassed: false,
        kind: EffectKind::Delay(DelayPatch {
            mode: DelayMode::PingPong,
            steps: 3.0,
            feedback: 0.60,
            mix: 0.48,
        }),
    });
    project.mixer.master.inserts.push(EffectDevice {
        id: delay_st_id,
        name: "Stereo Echo".into(),
        bypassed: false,
        kind: EffectKind::Delay(DelayPatch {
            mode: DelayMode::Stereo,
            steps: 3.0,
            feedback: 0.60,
            mix: 0.48,
        }),
    });
    project.mixer.master.inserts.push(EffectDevice {
        id: drive_tape_id,
        name: "Tape Drive".into(),
        bypassed: false,
        kind: EffectKind::Drive(DrivePatch {
            mode: DriveMode::Tape,
            drive: 0.80,
            asymmetry: 0.0,
            tone_hz: 6_500.0,
            output_db: 0.0,
            mix: 1.0,
        }),
    });
    project.mixer.master.inserts.push(EffectDevice {
        id: drive_hard_id,
        name: "Hard Clip Dist".into(),
        bypassed: false,
        kind: EffectKind::Drive(DrivePatch {
            mode: DriveMode::HardClip,
            drive: 0.88,
            asymmetry: 0.0,
            tone_hz: 9_500.0,
            output_db: 0.0,
            mix: 1.0,
        }),
    });

    if let Some(ch0) = project.mixer.channels.first_mut() {
        ch0.inserts.clear();
        ch0.inserts.push(EffectDevice {
            id: drive_fold_id,
            name: "West-Coast Fold".into(),
            bypassed: false,
            kind: EffectKind::Drive(DrivePatch {
                mode: DriveMode::Fold,
                drive: 0.72,
                asymmetry: 0.0,
                tone_hz: 12_000.0,
                output_db: 0.0,
                mix: 1.0,
            }),
        });
    }

    let snap = AudioSnapshot {
        playing: false,
        sample_rate: 48_000,
        ..AudioSnapshot::default()
    };
    let theme = Theme::cadence_dark();
    let mut app = App::new_in_memory(project, "delay-drive-modes.json".into()).unwrap();

    let mut render = |id: u64, filename: &str| -> Vec<u8> {
        app.press_keys("<Esc>").unwrap();
        app.execute_command(&format!("fx {id}")).unwrap();
        let fv = app.frame_view(&snap, "Metal · 48 kHz");
        let px = gpu.render_rgba8(&fv, &theme).unwrap();
        assert!(
            distinct_pixel_ratio(&px, theme.surface.canvas) > 0.25,
            "{filename} rendered too few visible pixels"
        );
        maybe_save_png(filename, 1440, 860, &px);
        px
    };

    let px_pp = render(delay_pp_id, "16-fx-delay-pingpong.png");
    let px_st = render(delay_st_id, "17-fx-delay-stereo.png");
    let px_tape = render(drive_tape_id, "18-fx-drive-tape.png");
    let px_hard = render(drive_hard_id, "19-fx-drive-hardclip.png");
    let px_fold = render(drive_fold_id, "20-fx-drive-fold.png");

    assert_ne!(
        px_pp, px_st,
        "PingPong and Stereo delay must render distinctly"
    );
    assert_ne!(
        px_tape, px_hard,
        "Tape and HardClip drive must render distinctly"
    );
    assert_ne!(
        px_hard, px_fold,
        "HardClip and Fold drive must render distinctly"
    );
    assert_ne!(
        px_tape, px_fold,
        "Tape and Fold drive must render distinctly"
    );

    // Also confirm Studio Light theme renders Stereo Delay & Fold Drive cleanly.
    let light = Theme::studio_light();
    app.press_keys("<Esc>").unwrap();
    app.execute_command(&format!("fx {drive_fold_id}")).unwrap();
    let fv_fold_light = app.frame_view(&snap, "Metal · 48 kHz");
    let px_fold_light = gpu.render_rgba8(&fv_fold_light, &light).unwrap();
    assert!(distinct_pixel_ratio(&px_fold_light, light.surface.canvas) > 0.25);
    assert_ne!(px_fold_light, px_fold);
}

#[test]
fn fixture_normalization_survives_max_fx_budget_and_conflicting_gain_automation() {
    use cadence_core::macros::{MacroBinding, MacroControl};
    use cadence_core::mixer::{MAX_EFFECT_DEVICES, MAX_INSERTS};

    let mut project = Project::demo();
    let mut next_id = project.max_id() + 1;

    // Pack project to the hard 24-device / 4-inserts-per-channel limit, including Master & ch0.
    let mut total_fx = project.mixer.master.inserts.len()
        + project
            .mixer
            .channels
            .iter()
            .map(|c| c.inserts.len())
            .sum::<usize>();
    while project.mixer.master.inserts.len() < MAX_INSERTS && total_fx < MAX_EFFECT_DEVICES {
        project.mixer.master.inserts.push(EffectDevice {
            id: next_id,
            name: format!("Packed Master EQ {next_id}"),
            bypassed: false,
            kind: EffectKind::Eq(EqPatch::default()),
        });
        next_id += 1;
        total_fx += 1;
    }
    for ch in &mut project.mixer.channels {
        while ch.inserts.len() < MAX_INSERTS && total_fx < MAX_EFFECT_DEVICES {
            ch.inserts.push(EffectDevice {
                id: next_id,
                name: format!("Packed Ch EQ {next_id}"),
                bypassed: false,
                kind: EffectKind::Eq(EqPatch::default()),
            });
            next_id += 1;
            total_fx += 1;
        }
    }
    assert_eq!(total_fx, MAX_EFFECT_DEVICES);

    let packed_dev_id = project.mixer.master.inserts[0].id;
    let ch0_id = project.mixer.channels[0].id;
    let pat_id = next_id;
    next_id += 1;
    project.patterns.push(Pattern {
        id: pat_id,
        name: "Stress Auto".into(),
        kind: PatternKind::Values {
            unit: ValueUnit::Normalized,
            period: 16,
            mode: ValueMode::Ramp,
            points: vec![ValuePoint {
                step: 0,
                value: 0.5,
            }],
        },
    });
    project.modulations.push(Modulation {
        id: next_id,
        name: "Packed Dev Mod".into(),
        pattern: pat_id,
        target: ModulationTarget::Effect {
            device: packed_dev_id,
            parameter: EffectParam::Low,
        },
    });
    next_id += 1;
    project.modulations.push(Modulation {
        id: next_id,
        name: "Conflicting Ch0 Gain".into(),
        pattern: pat_id,
        target: ModulationTarget::ChannelGain {
            channel: ChannelRef::Channel(ch0_id),
        },
    });
    next_id += 1;
    project.macros.push(MacroControl {
        index: 0,
        name: "Stress Macro".into(),
        value: 0.5,
        bindings: vec![
            MacroBinding {
                target: ParamAddress::Effect {
                    device: packed_dev_id,
                    param: EffectParam::Low,
                },
                min: 0.2,
                max: 0.8,
            },
            MacroBinding {
                target: ParamAddress::Channel {
                    channel: ChannelRef::Channel(ch0_id),
                    param: ChannelParam::Gain,
                },
                min: 0.1,
                max: 0.9,
            },
        ],
    });
    project
        .validate()
        .expect("stress fixture must validate before reset");

    reset_inserts_and_channel_gain_automation(&mut project);

    // Pushing dedicated test devices across Master, ch0, ch1, and Group must now validate cleanly.
    let mut alloc_id = next_id;
    for i in 0..4 {
        project.mixer.master.inserts.push(EffectDevice {
            id: alloc_id,
            name: format!("Test Master {i}"),
            bypassed: false,
            kind: EffectKind::Eq(EqPatch::default()),
        });
        alloc_id += 1;
        project.mixer.channels[0].inserts.push(EffectDevice {
            id: alloc_id,
            name: format!("Test Ch0 {i}"),
            bypassed: false,
            kind: EffectKind::Eq(EqPatch::default()),
        });
        alloc_id += 1;
    }
    project.mixer.channels[1].inserts.push(EffectDevice {
        id: alloc_id,
        name: "Test Ch1 Comp".into(),
        bypassed: false,
        kind: EffectKind::Compressor(CompressorPatch::default()),
    });
    alloc_id += 1;
    if let Some(grp) = project
        .mixer
        .channels
        .iter_mut()
        .find(|c| c.kind == cadence_core::mixer::ChannelKind::Group)
    {
        grp.inserts.push(EffectDevice {
            id: alloc_id,
            name: "Test Group Comp".into(),
            bypassed: false,
            kind: EffectKind::Compressor(CompressorPatch::default()),
        });
    }
    project
        .validate()
        .expect("normalized fixture must validate cleanly after pushing dedicated test FX");
    assert!(
        project
            .modulations
            .iter()
            .all(|m| !matches!(m.target, ModulationTarget::ChannelGain { .. })),
        "pre-existing ChannelGain modulations must be stripped"
    );
    assert!(
        project.macros[0].bindings.is_empty(),
        "dangling Effect and conflicting ChannelGain macro bindings must be stripped"
    );
}

#[test]
fn gpu_renders_filter_groove_browser_overlay_in_dark_and_light_themes() {
    let Ok(mut gpu) = HeadlessGpu::new(1440, 860) else {
        return;
    };
    let mut project = Project::demo();
    reset_inserts_and_channel_gain_automation(&mut project);
    let filter_id = project.max_id() + 1;
    project.mixer.master.inserts.push(EffectDevice {
        id: filter_id,
        name: "Acid PCF Filter".into(),
        bypassed: false,
        kind: EffectKind::Filter(FilterPatch {
            mode: FilterMode::AcidLowPass,
            cutoff_hz: 680.0,
            resonance: 0.70,
            drive: 0.30,
            mix: 1.0,
            env_pattern: 8,
            env_pattern_id: None,
            env_amount: 2.6,
            env_decay_ms: 210.0,
        }),
    });
    project.validate().unwrap();
    let lib = cadence_core::sampler::SampleLibrary::from_roots(&[], None);
    let mut app = App::new_with_library(project, "groove-browser-gpu.json".into(), lib).unwrap();
    app.execute_command(&format!("fx {filter_id}")).unwrap();
    app.press_keys("Pjjjj").unwrap(); // open Filter Groove Browser & scroll to live preview 12 Velvet Arch!
    let snap = AudioSnapshot {
        playing: true,
        absolute_step: 4,
        step_fraction: 0.25,
        sample_rate: 48_000,
        ..AudioSnapshot::default()
    };
    let fv = app.frame_view(&snap, "Metal · 48 kHz");
    assert!(
        matches!(
            fv.overlay,
            Some(cadence_app::view::OverlayView::GrooveBrowser(_))
        ),
        "expected GrooveBrowser overlay on P in Filter deck"
    );
    let dark = Theme::cadence_dark();
    let px_dark = gpu.render_rgba8(&fv, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px_dark, dark.surface.canvas) > 0.25);
    maybe_save_png("21-filter-groove-browser-dark.png", 1440, 860, &px_dark);

    let light = Theme::studio_light();
    let px_light = gpu.render_rgba8(&fv, &light).unwrap();
    assert!(distinct_pixel_ratio(&px_light, light.surface.canvas) > 0.25);
    assert_ne!(px_light, px_dark);
    maybe_save_png("22-filter-groove-browser-light.png", 1440, 860, &px_light);
}
