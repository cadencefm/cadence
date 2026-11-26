//! Headless Metal/wgpu GPU integration test for 32-Bit Float Metering:
//! 1. Mixer (`gm`) + Output (`go`) + Chrome with amber `HOT` internal float buses,
//!    pre-fader input bars & hold ticks, `-60..+6 dBFS` scale with `0 dB` hairline,
//!    and live Compressor/Limiter GR micro-meters + badges (`CMP` / `LIM`)
//! 2. Master `!CLIP` state across Chrome, Output deck, and Mixer Master strip (`MST` + `!CLIP`)
//! 3. Effect deck Compressor and Limiter GR bars with distinct `gr_hold_db` peak ticks
//! 4. Studio Light theme parity check for semantic metering tokens
//!
//! Set `CADENCE_DUMP_METERING_DIR=/path/to/dir` to save 1440×860 PNG screenshots for visual inspection.

mod common;

use cadence_app::App;
use cadence_core::dsp::db_to_lin;
use cadence_core::material::Project;
use cadence_core::mixer::{
    ChannelKind, ChannelRef, CompressorPatch, EffectDevice, EffectKind, LimiterPatch,
};
use cadence_core::runtime::{AudioSnapshot, ChannelMeter};
use cadence_gui::headless::{distinct_pixel_ratio, HeadlessGpu};
use cadence_gui::theme::Theme;
use common::reset_inserts_and_channel_gain_automation;
use std::path::PathBuf;

fn maybe_save_png(name: &str, width: u32, height: u32, rgba: &[u8]) {
    common::maybe_save_png("CADENCE_DUMP_METERING_DIR", name, width, height, rgba);
}

#[test]
fn gpu_renders_float_metering_mixer_output_chrome_and_effect_gr_holds() {
    let Ok(mut gpu) = HeadlessGpu::new(1440, 860) else {
        return;
    };

    let demo_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples/demo/project.json");
    let mut project = cadence_app::load_project(&demo_path).unwrap_or_else(|_| Project::demo());
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

    // Showcase -6.0 dB attenuation (Kick, pre-fader HOT tick stands prominently above post-fader green bar),
    // exact +0.0 dB unity detent (Snare), and +4.0 dB headroom boost (Hats) side-by-side!
    project.mixer.channels[0].strip.gain = db_to_lin(-6.0);
    project.mixer.channels[1].strip.gain = 1.0;
    project.mixer.channels[2].strip.gain = db_to_lin(4.0);

    // Attach Compressor on ch1 and Limiter on Master so live GR micro-meters + badges show on Mixer & Output.
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

    let wave: Vec<f32> = (0..256)
        .map(|i| {
            let t = i as f32 / 256.0 * std::f32::consts::TAU;
            (t * 2.0).sin() * 0.52 + (t * 5.0).sin() * 0.28 + (t * 9.0).sin() * 0.12
        })
        .collect();

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
            // Master receiving +3.0 dBFS float sum, cleanly tamed to -0.5 dBFS by Master Limiter!
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
                // Hot pre-fader (+4.2 dBFS), fader pulled down to -6 dB -> clean post-fader green + amber pre tick + HOT!
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
                // Hot 32-bit float group bus (+2.4 dBFS output -> amber fill above 0 dB hairline, NOT red clip!)
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
        playing: true,
        absolute_step: 64,
        step_fraction: 0.35,
        sample_rate: 48_000,
        peak: db_to_lin(-0.8),
        peak_left: db_to_lin(-0.8),
        peak_right: db_to_lin(-1.1),
        waveform: wave.clone(),
        channel_meters: build_meters(false),
        ..AudioSnapshot::default()
    };
    snap_hot.set_effect_meter(strip_comp_id, 5.8, 9.4);
    snap_hot.set_effect_meter(grp_comp_id, 3.2, 6.7);
    snap_hot.set_effect_meter(master_lim_id, 3.4, 6.1);

    let mut snap_clip = AudioSnapshot {
        playing: true,
        absolute_step: 64,
        step_fraction: 0.35,
        sample_rate: 48_000,
        peak: db_to_lin(2.2),
        peak_left: db_to_lin(2.2),
        peak_right: db_to_lin(1.9),
        waveform: wave,
        channel_meters: build_meters(true),
        ..AudioSnapshot::default()
    };
    snap_clip.set_effect_meter(strip_comp_id, 5.8, 9.4);
    snap_clip.set_effect_meter(grp_comp_id, 3.2, 6.7);
    snap_clip.set_effect_meter(master_lim_id, 1.2, 4.8);

    let dark = Theme::cadence_dark();
    let light = Theme::studio_light();
    let status = "Metal · 48 kHz";
    project
        .validate()
        .expect("normalized float metering fixture must validate");
    let lib = cadence_core::sampler::SampleLibrary::from_roots(&[], Some(&demo_path));
    let mut app = App::new_with_library(project, demo_path, lib).unwrap();

    // 1. Zoomed Mixer (`gm zz`) showing ALL 14 dense console strips (Kick HOT pre-fader, Snare CMP,
    //    Drums GRP HOT + CMP, Master HOT + LIM) side-by-side at studio strip width!
    app.press_keys("gmzz").unwrap();
    let px_mixer_zoom_hot = gpu
        .render_rgba8(&app.frame_view(&snap_hot, status), &dark)
        .unwrap();
    assert!(distinct_pixel_ratio(&px_mixer_zoom_hot, dark.surface.canvas) > 0.25);
    maybe_save_png(
        "01-mixer-zoomed-all-channels-hot.png",
        1440,
        860,
        &px_mixer_zoom_hot,
    );

    // 2. Unzoomed Mixer (`gm` with Output `go` + Chrome) with Master `!CLIP` (>0 dBFS DAC/WAV ceiling).
    app.press_keys("zz").unwrap();
    let px_mixer_clip = gpu
        .render_rgba8(&app.frame_view(&snap_clip, status), &dark)
        .unwrap();
    assert!(distinct_pixel_ratio(&px_mixer_clip, dark.surface.canvas) > 0.25);
    assert_ne!(px_mixer_zoom_hot, px_mixer_clip);
    maybe_save_png(
        "02-mixer-output-master-clip-dark.png",
        1440,
        860,
        &px_mixer_clip,
    );

    // 3. Unzoomed Mixer Page 1 (explicitly selecting ch0) + Output (`go`) + Chrome in Dark & Studio Light!
    app.selected_channel = ChannelRef::Channel(ch0_id);
    let px_mixer_p1_dark = gpu
        .render_rgba8(&app.frame_view(&snap_hot, status), &dark)
        .unwrap();
    assert!(distinct_pixel_ratio(&px_mixer_p1_dark, dark.surface.canvas) > 0.25);
    maybe_save_png(
        "03-mixer-page1-output-hot-dark.png",
        1440,
        860,
        &px_mixer_p1_dark,
    );

    let px_mixer_light = gpu
        .render_rgba8(&app.frame_view(&snap_hot, status), &light)
        .unwrap();
    assert!(distinct_pixel_ratio(&px_mixer_light, light.surface.canvas) > 0.20);
    maybe_save_png(
        "04-mixer-page1-output-hot-light.png",
        1440,
        860,
        &px_mixer_light,
    );

    // 4. Effect deck Compressor with live GR (-5.8 dB) + distinct peak-hold tick (-9.4 dB).
    app.execute_command(&format!("fx {strip_comp_id}")).unwrap();
    let px_comp = gpu
        .render_rgba8(&app.frame_view(&snap_hot, status), &dark)
        .unwrap();
    assert!(distinct_pixel_ratio(&px_comp, dark.surface.canvas) > 0.25);
    maybe_save_png("05-fx-compressor-gr-hold.png", 1440, 860, &px_comp);

    // 5. Effect deck Limiter with live GR (-3.4 dB) + distinct peak-hold tick (-6.1 dB).
    app.press_keys("<Esc>").unwrap();
    app.execute_command(&format!("fx {master_lim_id}")).unwrap();
    let px_lim = gpu
        .render_rgba8(&app.frame_view(&snap_hot, status), &dark)
        .unwrap();
    assert!(distinct_pixel_ratio(&px_lim, dark.surface.canvas) > 0.25);
    maybe_save_png("06-fx-limiter-gr-hold.png", 1440, 860, &px_lim);

    // 6. Top-bar DSP CPU pill (nominal 18% & hot 88% overload) + Auto compact stereo meter
    //    (Output Scope hidden via `go`) + Arrange per-lane vertical meter pills (Dark & Light)!
    app.press_keys("<Esc>ga").unwrap();
    app.press_keys("go").unwrap(); // Hide Output Scope -> Auto compact top-bar stereo meter appears
    app.config.show_arrange_lane_meters = true;
    let snap_compact = AudioSnapshot {
        health: cadence_core::runtime::AudioHealth {
            cpu_load: 0.18,
            peak_cpu_load: 0.34,
            overloaded: false,
            failed: false,
            xrun_count: 0,
            ..Default::default()
        },
        ..snap_hot.clone()
    };
    let fv_compact = app.frame_view(&snap_compact, status);
    assert!(fv_compact.chrome.show_dsp_cpu);
    assert!(fv_compact.chrome.show_compact_meter);
    let px_compact_dark = gpu.render_rgba8(&fv_compact, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px_compact_dark, dark.surface.canvas) > 0.20);
    maybe_save_png(
        "07-compact-chrome-cpu-and-lane-meters-dark.png",
        1440,
        860,
        &px_compact_dark,
    );

    let px_compact_light = gpu.render_rgba8(&fv_compact, &light).unwrap();
    assert!(distinct_pixel_ratio(&px_compact_light, light.surface.canvas) > 0.20);
    maybe_save_png(
        "08-compact-chrome-cpu-and-lane-meters-light.png",
        1440,
        860,
        &px_compact_light,
    );

    let snap_warn = AudioSnapshot {
        health: cadence_core::runtime::AudioHealth {
            cpu_load: 0.88,
            peak_cpu_load: 0.96,
            overloaded: true,
            failed: false,
            xrun_count: 2,
            ..Default::default()
        },
        ..snap_clip.clone()
    };
    let px_warn = gpu
        .render_rgba8(&app.frame_view(&snap_warn, status), &dark)
        .unwrap();
    assert_ne!(px_compact_dark, px_warn);
    maybe_save_png(
        "09-chrome-cpu-overload-and-clip-dark.png",
        1440,
        860,
        &px_warn,
    );

    // 7. Smooth Mixer console scrolling: mid-bank scroll showing BOTH `<` and `>` overflow rails
    //    beside pinned Master + fixed-height 3-row detail stack with `↓1-3/N` overflow indicator
    //    (Dark & Light), plus zoomed `zz` full-screen Mixer showing all 14 channels & 5-row detail!
    app.press_keys("go").unwrap(); // Restore Output Scope
    let mid_ch = app.channels()[5];
    for kind in ["eq", "filter", "drive", "chorus"] {
        let _ = app.apply(cadence_app::Action::AddEffect {
            channel: mid_ch,
            kind: kind.into(),
        });
    }
    app.press_keys("gm").unwrap();
    app.selected_channel = mid_ch;
    app.view_state.mixer_page_size = 10;
    app.view_state.mixer_scroll.set(2);
    let fv_scrolled = app.frame_view(&snap_hot, status);
    let px_scrolled_dark = gpu.render_rgba8(&fv_scrolled, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px_scrolled_dark, dark.surface.canvas) > 0.25);
    maybe_save_png(
        "10-mixer-scrolled-pinned-master-dark.png",
        1440,
        860,
        &px_scrolled_dark,
    );
    let px_scrolled_light = gpu.render_rgba8(&fv_scrolled, &light).unwrap();
    assert!(distinct_pixel_ratio(&px_scrolled_light, light.surface.canvas) > 0.20);
    maybe_save_png(
        "11-mixer-scrolled-pinned-master-light.png",
        1440,
        860,
        &px_scrolled_light,
    );

    // Zoomed `zz` Mixer: all 14 channels fit without overflow rails + 5-row fixed detail stack!
    app.press_keys("zz").unwrap();
    let fv_zoomed = app.frame_view(&snap_hot, status);
    let px_zoomed_dark = gpu.render_rgba8(&fv_zoomed, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px_zoomed_dark, dark.surface.canvas) > 0.25);
    maybe_save_png(
        "12-mixer-zoomed-5row-detail-dark.png",
        1440,
        860,
        &px_zoomed_dark,
    );
}
