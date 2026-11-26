use crate::common::{
    load_demo_project, render_checked, reset_inserts_and_channel_gain_automation, with_gpu,
    DEFAULT_H, DEFAULT_W, STATUS,
};
use cadence_app::view::{DeckView, OverlayView};
use cadence_app::App;
use cadence_core::material::{
    Modulation, ModulationTarget, Pattern, PatternKind, Project, ValuePoint, ValueUnit,
};
use cadence_core::mixer::{
    ChannelRef, ChorusPatch, CompressorPatch, DelayMode, DelayPatch, DriveMode, DrivePatch,
    EffectDevice, EffectKind, EffectParam, EqPatch, FilterPatch, LimiterPatch, ReverbPatch,
};
use cadence_core::params::ValueMode;
use cadence_core::rack::FilterMode;
use cadence_core::runtime::{AudioSnapshot, ChannelMeter};
use cadence_gui::theme::Theme;

#[test]
fn gpu_renders_all_eight_fx_decks_bypass_and_compact_limiter() {
    with_gpu(DEFAULT_W, DEFAULT_H, |gpu| {
        let (mut project, demo_path) = load_demo_project();
        reset_inserts_and_channel_gain_automation(&mut project);

        let base_id = project.max_id() + 1;
        let eq_id = base_id;
        let filter_id = base_id + 1;
        let comp_id = base_id + 2;
        let drive_id = base_id + 3;
        let delay_id = base_id + 4;
        let chorus_id = base_id + 5;
        let reverb_id = base_id + 6;
        let limiter_id = base_id + 7;

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

        // Live PCF Cutoff modulation so Filter deck displays the live sweep ribbon + mod curve.
        let next_pat_id = project.max_id() + 1;
        let next_mod_id = next_pat_id + 1;
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

        let dark = Theme::cadence_dark();
        project
            .validate()
            .expect("normalized FX visuals fixture must validate");
        let lib = cadence_core::sampler::SampleLibrary::from_roots(&[], Some(&demo_path));
        let mut app = App::new_with_library(project, demo_path, lib).unwrap();

        let cases = [
            (eq_id, "fx-01-eq.png"),
            (filter_id, "fx-02-filter-pcf.png"),
            (comp_id, "fx-03-compressor-gr.png"),
            (drive_id, "fx-04-drive-tape.png"),
            (delay_id, "fx-05-delay-pingpong.png"),
            (chorus_id, "fx-06-chorus.png"),
            (reverb_id, "fx-07-reverb.png"),
            (limiter_id, "fx-08-limiter-gr.png"),
        ];
        let mut rendered_frames = Vec::with_capacity(cases.len());
        for (idx, (id, filename)) in cases.into_iter().enumerate() {
            if idx > 0 {
                app.press_keys("<Esc>").unwrap();
            }
            app.execute_command(&format!("fx {id}")).unwrap();
            let fv = app.frame_view(&snap, STATUS);
            let ev = fv
                .decks
                .iter()
                .find_map(|d| match &d.view {
                    DeckView::Effect(e) => Some(e),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("expected DeckView::Effect for {filename}"));
            if idx == 0 {
                assert!(
                    ev.controls.iter().any(|c| c.module == "ROUTING"),
                    "Effect deck must expose ROUTING controls"
                );
            }
            rendered_frames.push(render_checked(gpu, filename, &fv, &dark, 0.25));
        }

        for pair in rendered_frames.windows(2) {
            assert_ne!(pair[0], pair[1], "adjacent FX decks must render distinctly");
        }

        // Bypassed EQ renders distinctly with BYPASSED badge.
        app.press_keys("<Esc>").unwrap();
        app.execute_command(&format!("fx bypass {eq_id}")).unwrap();
        app.execute_command(&format!("fx {eq_id}")).unwrap();
        let px_byp = render_checked(
            gpu,
            "fx-09-eq-bypassed.png",
            &app.frame_view(&snap, STATUS),
            &dark,
            0.25,
        );
        assert_ne!(px_byp, rendered_frames[0], "bypassed EQ must look distinct");

        // Compact 1024×680 Limiter split-pane check (verifies HeadlessGpu::resize + zero clipping).
        gpu.resize(1024, 680);
        app.press_keys("<Esc>").unwrap();
        app.execute_command(&format!("fx {limiter_id}")).unwrap();
        render_checked(
            gpu,
            "fx-10-limiter-compact-1024x680.png",
            &app.frame_view(&snap, STATUS),
            &dark,
            0.25,
        );
    });
}

#[test]
fn gpu_renders_fx_edge_cases_modes_and_groove_browser() {
    with_gpu(DEFAULT_W, DEFAULT_H, |gpu| {
        let dark = Theme::cadence_dark();
        let light = Theme::studio_light();

        // 1. Edge cases: Mono Reverb, negative-makeup Compressor, 44.1 kHz Parallel Acid Filter.
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

        let mut snap_44k = AudioSnapshot {
            playing: true,
            absolute_step: 4,
            step_fraction: 0.5,
            sample_rate: 44_100,
            ..AudioSnapshot::default()
        };
        snap_44k.set_effect_meter(comp_hot_id, 4.2, 6.5);

        let mut app = App::new_in_memory(project, "fx-edge-cases.json".into()).unwrap();
        for (idx, (id, filename)) in [
            (rev_mono_id, "fx-11-reverb-mono.png"),
            (comp_hot_id, "fx-12-comp-neg-makeup.png"),
            (filt_par_id, "fx-13-filter-44k-parallel.png"),
        ]
        .into_iter()
        .enumerate()
        {
            if idx > 0 {
                app.press_keys("<Esc>").unwrap();
            }
            app.execute_command(&format!("fx {id}")).unwrap();
            render_checked(
                gpu,
                filename,
                &app.frame_view(&snap_44k, "Metal · 44.1 kHz"),
                &dark,
                0.25,
            );
        }

        // Filter Groove Browser (`Pjjjj`) over Filter deck in Dark & Studio Light.
        app.press_keys("Pjjjj").unwrap();
        let fv_groove = app.frame_view(&snap_44k, "Metal · 44.1 kHz");
        assert!(
            matches!(fv_groove.overlay, Some(OverlayView::GrooveBrowser(_))),
            "expected GrooveBrowser overlay on P in Filter deck"
        );
        let px_grv_dark = render_checked(
            gpu,
            "fx-14-groove-browser-dark.png",
            &fv_groove,
            &dark,
            0.25,
        );
        let px_grv_light = render_checked(
            gpu,
            "fx-15-groove-browser-light.png",
            &fv_groove,
            &light,
            0.25,
        );
        assert_ne!(px_grv_dark, px_grv_light);

        // 2. Delay (PingPong vs Stereo) & Drive (Tape vs HardClip vs Fold + Light Fold) mode renders.
        let mut mode_proj = Project::demo();
        reset_inserts_and_channel_gain_automation(&mut mode_proj);
        let base = mode_proj.max_id() + 1;
        let (delay_pp_id, delay_st_id, drive_tape_id, drive_hard_id, drive_fold_id) =
            (base, base + 1, base + 2, base + 3, base + 4);
        mode_proj.mixer.master.inserts = vec![
            EffectDevice {
                id: delay_pp_id,
                name: "Ping-Pong Delay".into(),
                bypassed: false,
                kind: EffectKind::Delay(DelayPatch {
                    mode: DelayMode::PingPong,
                    steps: 3.0,
                    feedback: 0.60,
                    mix: 0.48,
                }),
            },
            EffectDevice {
                id: delay_st_id,
                name: "Stereo Echo".into(),
                bypassed: false,
                kind: EffectKind::Delay(DelayPatch {
                    mode: DelayMode::Stereo,
                    steps: 3.0,
                    feedback: 0.60,
                    mix: 0.48,
                }),
            },
            EffectDevice {
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
            },
            EffectDevice {
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
            },
        ];
        if let Some(ch0) = mode_proj.mixer.channels.first_mut() {
            ch0.inserts = vec![EffectDevice {
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
            }];
        }

        let snap_48k = AudioSnapshot {
            sample_rate: 48_000,
            ..AudioSnapshot::default()
        };
        let mut mode_app = App::new_in_memory(mode_proj, "fx-modes.json".into()).unwrap();
        let mut render_fx = |id: u64, name: &str| {
            mode_app.press_keys("<Esc>").unwrap();
            mode_app.execute_command(&format!("fx {id}")).unwrap();
            render_checked(
                gpu,
                name,
                &mode_app.frame_view(&snap_48k, STATUS),
                &dark,
                0.25,
            )
        };
        let px_pp = render_fx(delay_pp_id, "fx-16-delay-pingpong.png");
        let px_st = render_fx(delay_st_id, "fx-17-delay-stereo.png");
        let px_tape = render_fx(drive_tape_id, "fx-18-drive-tape.png");
        let px_hard = render_fx(drive_hard_id, "fx-19-drive-hardclip.png");
        let px_fold = render_fx(drive_fold_id, "fx-20-drive-fold-dark.png");
        assert_ne!(px_pp, px_st, "PingPong and Stereo delay must differ");
        assert_ne!(px_tape, px_hard, "Tape and HardClip drive must differ");
        assert_ne!(px_hard, px_fold, "HardClip and Fold drive must differ");
        assert_ne!(px_tape, px_fold, "Tape and Fold drive must differ");

        let px_fold_light = render_checked(
            gpu,
            "fx-21-drive-fold-light.png",
            &mode_app.frame_view(&snap_48k, STATUS),
            &light,
            0.25,
        );
        assert_ne!(px_fold, px_fold_light);
    });
}
