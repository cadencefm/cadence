use crate::common::{
    build_snapshot, frame_diff_ratio, load_demo_project, render_checked,
    reset_inserts_and_channel_gain_automation, with_gpu, DEFAULT_H, DEFAULT_W, STATUS,
};
use cadence_app::view::DeckView;
use cadence_app::{Action, App, MixerSlot};
use cadence_core::dsp::db_to_lin;
use cadence_core::material::{
    Modulation, ModulationTarget, Pattern, PatternKind, Project, ValuePoint, ValueUnit,
};
use cadence_core::mixer::{
    ChannelKind, ChannelRef, CompressorPatch, EffectDevice, EffectKind, LimiterPatch,
};
use cadence_core::params::ValueMode;
use cadence_core::runtime::{AudioHealth, AudioSnapshot, ChannelMeter};
use cadence_gui::theme::Theme;

#[test]
fn gpu_renders_float_metering_console_scroll_chrome_cpu_and_automated_strips() {
    with_gpu(DEFAULT_W, DEFAULT_H, |gpu| {
        let dark = Theme::cadence_dark();
        let light = Theme::studio_light();
        let (mut project, demo_path) = load_demo_project();
        reset_inserts_and_channel_gain_automation(&mut project);

        let mut next_id = project.max_id() + 1;
        let ch0_id = project.mixer.channels[0].id;
        let ch1_id = project.mixer.channels[1].id;
        let ch2_id = project.mixer.channels[2].id;
        let grp_id = project
            .mixer
            .channels
            .iter()
            .find(|c| c.kind == ChannelKind::Group)
            .map(|c| c.id)
            .unwrap_or(ch2_id);

        project.mixer.channels[0].strip.gain = db_to_lin(-6.0);
        project.mixer.channels[1].strip.gain = 1.0;
        project.mixer.channels[2].strip.gain = db_to_lin(4.0);

        let strip_comp_id = next_id;
        next_id += 1;
        project.mixer.channels[1].inserts.push(EffectDevice {
            id: strip_comp_id,
            name: "Bus Glue".into(),
            bypassed: false,
            kind: EffectKind::Compressor(CompressorPatch {
                threshold_db: -18.0,
                ratio: 4.5,
                attack_ms: 4.0,
                release_ms: 85.0,
                makeup_db: 3.0,
                mix: 1.0,
            }),
        });

        let grp_comp_id = next_id;
        next_id += 1;
        if let Some(grp) = project.mixer.channels.iter_mut().find(|c| c.id == grp_id) {
            grp.inserts.push(EffectDevice {
                id: grp_comp_id,
                name: "Group Smash".into(),
                bypassed: false,
                kind: EffectKind::Compressor(CompressorPatch {
                    threshold_db: -14.0,
                    ratio: 6.0,
                    attack_ms: 2.0,
                    release_ms: 60.0,
                    makeup_db: 4.0,
                    mix: 1.0,
                }),
            });
        }

        let master_lim_id = next_id;
        project.mixer.master.inserts.push(EffectDevice {
            id: master_lim_id,
            name: "Master Limiter".into(),
            bypassed: false,
            kind: EffectKind::Limiter(LimiterPatch {
                ceiling_db: -0.5,
                input_drive_db: 4.0,
                release_ms: 65.0,
                mix: 1.0,
            }),
        });

        let base_snap = build_snapshot(&project, 64);
        let build_meters = |master_clipped: bool| -> Vec<ChannelMeter> {
            let mut meters = vec![if master_clipped {
                ChannelMeter {
                    channel: ChannelRef::Master,
                    input_peak: [db_to_lin(3.5), db_to_lin(3.1)],
                    input_peak_hold: [db_to_lin(4.2), db_to_lin(3.8)],
                    output_peak: [db_to_lin(2.2), db_to_lin(1.9)],
                    output_peak_hold: [db_to_lin(2.8), db_to_lin(2.4)],
                    hot: true,
                    clipped: true,
                }
            } else {
                ChannelMeter {
                    channel: ChannelRef::Master,
                    input_peak: [db_to_lin(2.6), db_to_lin(2.2)],
                    input_peak_hold: [db_to_lin(3.2), db_to_lin(2.9)],
                    output_peak: [db_to_lin(-0.8), db_to_lin(-1.1)],
                    output_peak_hold: [db_to_lin(-0.5), db_to_lin(-0.7)],
                    hot: true,
                    clipped: false,
                }
            }];
            for (idx, ch) in project.mixer.channels.iter().enumerate() {
                let meter = if ch.id == ch0_id {
                    ChannelMeter {
                        channel: ChannelRef::Channel(ch.id),
                        input_peak: [db_to_lin(3.6), db_to_lin(3.2)],
                        input_peak_hold: [db_to_lin(4.2), db_to_lin(3.9)],
                        output_peak: [db_to_lin(-2.4), db_to_lin(-2.8)],
                        output_peak_hold: [db_to_lin(-1.8), db_to_lin(-2.1)],
                        hot: true,
                        clipped: false,
                    }
                } else if ch.id == grp_id {
                    ChannelMeter {
                        channel: ChannelRef::Channel(ch.id),
                        input_peak: [db_to_lin(1.8), db_to_lin(1.5)],
                        input_peak_hold: [db_to_lin(2.6), db_to_lin(2.2)],
                        output_peak: [db_to_lin(2.1), db_to_lin(1.7)],
                        output_peak_hold: [db_to_lin(2.4), db_to_lin(2.0)],
                        hot: true,
                        clipped: false,
                    }
                } else if ch.id == ch1_id {
                    ChannelMeter {
                        channel: ChannelRef::Channel(ch.id),
                        input_peak: [db_to_lin(-3.0), db_to_lin(-3.6)],
                        input_peak_hold: [db_to_lin(-1.4), db_to_lin(-2.0)],
                        output_peak: [db_to_lin(-5.2), db_to_lin(-5.8)],
                        output_peak_hold: [db_to_lin(-3.6), db_to_lin(-4.2)],
                        hot: false,
                        clipped: false,
                    }
                } else {
                    let db = -18.0 + ((idx * 7) % 14) as f32;
                    let lin = db_to_lin(db);
                    let hold = db_to_lin(db + 2.2);
                    ChannelMeter {
                        channel: ChannelRef::Channel(ch.id),
                        input_peak: [lin * 1.15, lin * 1.05],
                        input_peak_hold: [hold * 1.15, hold * 1.05],
                        output_peak: [lin, lin * 0.92],
                        output_peak_hold: [hold, hold * 0.92],
                        hot: false,
                        clipped: false,
                    }
                };
                meters.push(meter);
            }
            meters
        };

        let mut snap_hot = AudioSnapshot {
            peak: db_to_lin(-0.8),
            peak_left: db_to_lin(-0.8),
            peak_right: db_to_lin(-1.1),
            channel_meters: build_meters(false),
            ..base_snap.clone()
        };
        snap_hot.set_effect_meter(strip_comp_id, 5.8, 9.4);
        snap_hot.set_effect_meter(grp_comp_id, 3.2, 6.7);
        snap_hot.set_effect_meter(master_lim_id, 3.4, 6.1);

        let mut snap_clip = AudioSnapshot {
            peak: db_to_lin(2.2),
            peak_left: db_to_lin(2.2),
            peak_right: db_to_lin(1.9),
            channel_meters: build_meters(true),
            ..base_snap
        };
        snap_clip.set_effect_meter(strip_comp_id, 5.8, 9.4);
        snap_clip.set_effect_meter(grp_comp_id, 3.2, 6.7);
        snap_clip.set_effect_meter(master_lim_id, 1.2, 4.8);

        project.validate().expect("float metering fixture");
        let lib = cadence_core::sampler::SampleLibrary::from_roots(&[], Some(&demo_path));
        let mut app = App::new_with_library(project, demo_path, lib).unwrap();

        // 1. Zoomed Mixer HOT vs Unzoomed Master !CLIP
        app.press_keys("gmzz").unwrap();
        let px_zoom_hot = render_checked(
            gpu,
            "mixer-01-zoomed-hot-dark.png",
            &app.frame_view(&snap_hot, STATUS),
            &dark,
            0.25,
        );
        app.press_keys("zz").unwrap();
        let px_clip = render_checked(
            gpu,
            "mixer-02-master-clip-dark.png",
            &app.frame_view(&snap_clip, STATUS),
            &dark,
            0.25,
        );
        assert_ne!(px_zoom_hot, px_clip);

        // 2. Smooth console scroll with `<` and `>` rails beside pinned Master + 3-row detail (Dark & Light)
        let mid_ch = app.channels()[5];
        for kind in ["eq", "filter", "drive", "chorus"] {
            let _ = app.apply(Action::AddEffect {
                channel: mid_ch,
                kind: kind.into(),
            });
        }
        app.selected_channel = mid_ch;
        app.view_state.mixer_page_size = 10;
        app.view_state.mixer_scroll.set(2);
        let fv_scrolled = app.frame_view(&snap_hot, STATUS);
        let px_scr_dark = render_checked(
            gpu,
            "mixer-03-scrolled-pinned-master-dark.png",
            &fv_scrolled,
            &dark,
            0.25,
        );
        let px_scr_light = render_checked(
            gpu,
            "mixer-04-scrolled-pinned-master-light.png",
            &fv_scrolled,
            &light,
            0.20,
        );
        assert_ne!(px_scr_dark, px_scr_light);

        // 3. Top-bar DSP CPU pill (nominal & overload) + compact stereo meter + Arrange lane meters
        app.press_keys("gago").unwrap();
        app.config.show_arrange_lane_meters = true;
        let snap_compact = AudioSnapshot {
            health: AudioHealth {
                cpu_load: 0.18,
                peak_cpu_load: 0.34,
                overloaded: false,
                failed: false,
                xrun_count: 0,
                ..Default::default()
            },
            ..snap_hot.clone()
        };
        let fv_compact = app.frame_view(&snap_compact, STATUS);
        assert!(fv_compact.chrome.show_dsp_cpu);
        assert!(fv_compact.chrome.show_compact_meter);
        let px_compact_dark = render_checked(
            gpu,
            "mixer-05-compact-chrome-lane-meters-dark.png",
            &fv_compact,
            &dark,
            0.20,
        );
        render_checked(
            gpu,
            "mixer-06-compact-chrome-lane-meters-light.png",
            &fv_compact,
            &light,
            0.20,
        );

        let snap_warn = AudioSnapshot {
            health: AudioHealth {
                cpu_load: 0.88,
                peak_cpu_load: 0.96,
                overloaded: true,
                failed: false,
                xrun_count: 2,
                ..Default::default()
            },
            ..snap_clip
        };
        let px_warn = render_checked(
            gpu,
            "mixer-07-chrome-cpu-overload-clip.png",
            &app.frame_view(&snap_warn, STATUS),
            &dark,
            0.20,
        );
        assert_ne!(px_compact_dark, px_warn);

        // 4. Automated ChannelPan + ChannelGain ghost markers + focusable strip rows (Fader / Pan / Insert)
        let mut auto_proj = Project::demo();
        let mut next = auto_proj.max_id() + 1;
        let ch_ids: Vec<u64> = auto_proj
            .mixer
            .channels
            .iter()
            .take(4)
            .map(|c| c.id)
            .collect();
        for (idx, ch_id) in ch_ids.iter().copied().enumerate() {
            if let Some(ch) = auto_proj.mixer.channels.iter_mut().find(|c| c.id == ch_id) {
                if idx < 2 {
                    ch.strip.gain = 0.90;
                    ch.strip.pan = if idx == 0 { -0.25 } else { 0.30 };
                }
            }
            let pan_pat = next;
            next += 1;
            let pan_mod = next;
            next += 1;
            let target_pan = [0.14, 0.86, 0.28, 0.74][idx];
            auto_proj.patterns.push(Pattern {
                id: pan_pat,
                name: format!("Auto Pan {}", idx + 1),
                kind: PatternKind::Values {
                    unit: ValueUnit::Normalized,
                    period: 32,
                    mode: ValueMode::Ramp,
                    points: vec![
                        ValuePoint {
                            step: 0,
                            value: 0.5,
                        },
                        ValuePoint {
                            step: 12,
                            value: target_pan,
                        },
                        ValuePoint {
                            step: 24,
                            value: 1.0 - target_pan,
                        },
                    ],
                },
            });
            auto_proj.modulations.push(Modulation {
                id: pan_mod,
                name: format!("Ch{} Pan", idx + 1),
                target: ModulationTarget::ChannelPan {
                    channel: ChannelRef::Channel(ch_id),
                },
                pattern: pan_pat,
            });
        }
        for (idx, ch_id) in ch_ids.iter().copied().take(2).enumerate() {
            let g_pat = next;
            next += 1;
            let g_mod = next;
            next += 1;
            auto_proj.patterns.push(Pattern {
                id: g_pat,
                name: format!("Auto Ride {}", idx + 1),
                kind: PatternKind::Values {
                    unit: ValueUnit::Normalized,
                    period: 32,
                    mode: ValueMode::Ramp,
                    points: vec![
                        ValuePoint {
                            step: 0,
                            value: 0.25,
                        },
                        ValuePoint {
                            step: 12,
                            value: if idx == 0 { 0.42 } else { 0.68 },
                        },
                        ValuePoint {
                            step: 24,
                            value: 0.95,
                        },
                    ],
                },
            });
            auto_proj.modulations.push(Modulation {
                id: g_mod,
                name: format!("Ch{} Gain", idx + 1),
                target: ModulationTarget::ChannelGain {
                    channel: ChannelRef::Channel(ch_id),
                },
                pattern: g_pat,
            });
        }
        let mut auto_app = App::new_in_memory(auto_proj, "mixer-auto.json".into()).unwrap();
        let audio_id = auto_app.project().mixer.channels[0].id;
        auto_app
            .execute_command(&format!("fx add filter {audio_id}"))
            .unwrap();
        let auto_snap = build_snapshot(auto_app.project(), 12);

        auto_app.press_keys("gmzz").unwrap();
        auto_app.selected_channel = ChannelRef::Channel(ch_ids[0]);
        let fv_fader = auto_app.frame_view(&auto_snap, STATUS);
        let mv = fv_fader
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Mixer(m) => Some(m),
                _ => None,
            })
            .expect("MixerDeckView");
        assert_eq!(mv.focused_slot, MixerSlot::Fader);
        assert!(
            mv.channels
                .iter()
                .filter(|c| c.effective_pan.is_some())
                .count()
                >= 4
        );
        let px_fader = render_checked(
            gpu,
            "mixer-08-automated-pan-fader-dark.png",
            &fv_fader,
            &dark,
            0.25,
        );

        auto_app.press_keys("j<m-L>").unwrap();
        let fv_pan = auto_app.frame_view(&auto_snap, STATUS);
        assert!(fv_pan.decks.iter().any(|d| matches!(
            &d.view,
            DeckView::Mixer(m) if m.focused_slot == MixerSlot::Pan
        )));
        let px_pan = render_checked(gpu, "mixer-09-focused-pan-dark.png", &fv_pan, &dark, 0.25);
        assert!(frame_diff_ratio(&px_fader, &px_pan) > 0.002);
        render_checked(gpu, "mixer-10-focused-pan-light.png", &fv_pan, &light, 0.20);

        auto_app.press_keys("j").unwrap();
        let fv_ins = auto_app.frame_view(&auto_snap, STATUS);
        assert!(fv_ins.decks.iter().any(|d| matches!(
            &d.view,
            DeckView::Mixer(m) if m.focused_slot == MixerSlot::Insert(0)
        )));
        render_checked(
            gpu,
            "mixer-11-focused-insert-dark.png",
            &fv_ins,
            &dark,
            0.25,
        );
    });
}
