//! Headless Metal/wgpu GPU integration tests for Live Staging, Automated Channel Pan, and MIDI CC Recording:
//! 1. Active Live Staging (`STAGED` badge in Chrome + Arrange & Bank dashed staged previews)
//! 2. Zoomed Bank Launchpad Matrix with staged launches/releases/mutes/section jump
//! 3. Mixer (`gm`) with live automated Pan ghost markers + Fader ghost markers
//! 4. Devices · MIDI Deck (`gd`) with `REC · NOTE+CC` armed + last recorded CC target
//!
//! Set `CADENCE_DUMP_LIVE_STAGING_DIR=/path/to/dir` to save 1440×860 PNG screenshots for visual inspection.

mod common;

use cadence_app::view::DeckView;
use cadence_app::{Action, App};
use cadence_core::material::{
    Boundary, Modulation, ModulationTarget, Pattern, PatternKind, Project, ValuePoint, ValueUnit,
};
use cadence_core::midi_map::{MidiMapping, MidiSource, MidiTarget, MidiValueMode};
use cadence_core::mixer::ChannelRef;
use cadence_core::params::ValueMode;
use cadence_core::runtime::MidiMessage;
use cadence_gui::headless::{distinct_pixel_ratio, HeadlessGpu};
use cadence_gui::theme::Theme;
use std::path::PathBuf;

fn maybe_save_png(name: &str, width: u32, height: u32, rgba: &[u8]) {
    common::maybe_save_png("CADENCE_DUMP_LIVE_STAGING_DIR", name, width, height, rgba);
}

#[test]
fn gpu_renders_live_staging_automated_channel_pan_and_midi_rec_cc() {
    let Ok(mut gpu) = HeadlessGpu::new(1440, 860) else {
        return;
    };

    let mut project = Project::demo();
    // Add live ChannelGain + ChannelPan automations across channels to showcase BOTH
    // glowing automated Pan ghost markers and Fader ghost markers on the Mixer deck!
    let mut next_id = project.max_id() + 1;
    let ch_ids: Vec<u64> = project
        .mixer
        .channels
        .iter()
        .take(4)
        .map(|c| c.id)
        .collect();
    for (idx, ch_id) in ch_ids.iter().copied().enumerate() {
        if let Some(ch) = project.mixer.channels.iter_mut().find(|c| c.id == ch_id) {
            if idx < 2 {
                ch.strip.gain = 0.90;
                ch.strip.pan = if idx == 0 { -0.25 } else { 0.30 };
            }
        }
        // ChannelPan automation on channels 0, 1, 2 (and channel 4 Echo return!).
        let pan_pat = next_id;
        next_id += 1;
        let pan_mod = next_id;
        next_id += 1;
        let target_pan_norm = match idx {
            0 => 0.14, // swept hard Left
            1 => 0.86, // swept hard Right
            2 => 0.28, // swept Left-mid
            _ => 0.74, // swept Right-mid
        };
        project.patterns.push(Pattern {
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
                        value: target_pan_norm,
                    },
                    ValuePoint {
                        step: 24,
                        value: 1.0 - target_pan_norm,
                    },
                ],
            },
        });
        project.modulations.push(Modulation {
            id: pan_mod,
            name: format!("Ch{} Pan", idx + 1),
            target: ModulationTarget::ChannelPan {
                channel: ChannelRef::Channel(ch_id),
            },
            pattern: pan_pat,
        });
    }
    // Also add ChannelGain automation on channel 0 & 1 so both fader + pan ghost markers show together!
    for (idx, ch_id) in ch_ids.iter().copied().take(2).enumerate() {
        let g_pat = next_id;
        next_id += 1;
        let g_mod = next_id;
        next_id += 1;
        project.patterns.push(Pattern {
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
        project.modulations.push(Modulation {
            id: g_mod,
            name: format!("Ch{} Gain", idx + 1),
            target: ModulationTarget::ChannelGain {
                channel: ChannelRef::Channel(ch_id),
            },
            pattern: g_pat,
        });
    }

    let snap = common::build_snapshot(&project, 12);
    let theme = Theme::cadence_dark();
    let status = "Metal · 48 kHz";
    let mut app = App::new(project, PathBuf::from("live_staging_gpu.json")).unwrap();
    app.execute_command("lane Drums add Kick restart").unwrap();

    // --- 1. Live Staging active: Chrome STAGED badge + Arrange & Bank previews! ---
    app.apply(Action::Play { playing: true }).unwrap();
    let drums_lane = app
        .project()
        .lanes
        .iter()
        .find(|l| l.name == "Drums")
        .expect("Drums lane in demo project");
    let drums_id = drums_lane.id;
    let kick_lp = drums_lane.parts[0];
    app.apply(Action::Launch {
        lane: drums_id,
        part: kick_lp,
        at: Some(Boundary::Now),
    })
    .unwrap();
    app.apply(Action::Advance { steps: 12 }).unwrap();
    // Ensure Bank is in Parts mode alongside Arrange, then stage Launch + Release + Mute + Solo + Section!
    app.press_keys("gbgaS<m-q>2<m-e><m-x><m-w><m-m><m-q><m-s><m-2>")
        .unwrap();

    let fv_stage = app.frame_view(&snap, status);
    assert_eq!(fv_stage.chrome.staged_count, Some(5));
    let px_stage = gpu.render_rgba8(&fv_stage, &theme).unwrap();
    assert!(distinct_pixel_ratio(&px_stage, theme.surface.canvas) > 0.25);
    maybe_save_png("01-staging-arrange-bank.png", 1440, 860, &px_stage);

    // --- 2. Zoomed Bank Launchpad Matrix (`gb zz`) showing dashed staged cells & section! ---
    app.press_keys("gbzz").unwrap();
    let fv_bank = app.frame_view(&snap, status);
    assert!(fv_bank
        .decks
        .iter()
        .any(|d| matches!(d.view, DeckView::Bank(ref b) if b.staging_count == Some(5))));
    let px_bank = gpu.render_rgba8(&fv_bank, &theme).unwrap();
    assert!(distinct_pixel_ratio(&px_bank, theme.surface.canvas) > 0.20);
    assert_ne!(px_bank, px_stage);
    maybe_save_png("02-staging-bank-matrix-zoomed.png", 1440, 860, &px_bank);

    // Discard staging cleanly before Mixer & MIDI shots.
    app.press_keys("zz<Esc>").unwrap();
    assert!(app.staging().is_none());

    // --- 3. Mixer (`gm zz`) showing glowing live automated Pan ghost markers + Fader ghost markers! ---
    app.press_keys("gmzz").unwrap();
    app.selected_channel = ChannelRef::Channel(ch_ids[0]);
    let fv_mix = app.frame_view(&snap, status);
    let mix_view = fv_mix
        .decks
        .iter()
        .find_map(|d| match &d.view {
            DeckView::Mixer(m) => Some(m),
            _ => None,
        })
        .expect("MixerDeckView");
    assert!(
        mix_view
            .channels
            .iter()
            .filter(|c| c.effective_pan.is_some())
            .count()
            >= 4
    );
    let px_mix = gpu.render_rgba8(&fv_mix, &theme).unwrap();
    assert!(distinct_pixel_ratio(&px_mix, theme.surface.canvas) > 0.25);
    maybe_save_png("03-mixer-automated-pan-faders.png", 1440, 860, &px_mix);

    // --- 4. Devices · MIDI Deck (`gd`) with REC · NOTE+CC armed and live recorded CC target! ---
    app.config.midi_pickup = false;
    app.execute_command("midi profile minilab3_daw").unwrap();
    app.config.midi_mappings.push(MidiMapping::new(
        101,
        MidiSource::Cc {
            channel: Some(0),
            cc: 74,
        },
        MidiValueMode::Absolute,
        MidiTarget::FocusedMacro { slot: 0 },
    ));
    app.press_keys("zzgdzzR").unwrap();
    app.handle_midi(MidiMessage::Control {
        channel: 0,
        controller: 74,
        value: 96,
    })
    .unwrap();
    let fv_midi = app.frame_view(&snap, status);
    assert!(fv_midi.chrome.midi_recording);
    assert!(fv_midi.chrome.midi_cc_last_recorded.is_some());
    let px_midi = gpu.render_rgba8(&fv_midi, &theme).unwrap();
    assert!(distinct_pixel_ratio(&px_midi, theme.surface.canvas) > 0.25);
    maybe_save_png("04-midi-deck-rec-cc.png", 1440, 860, &px_midi);

    // Verify Studio Light theme renders cleanly too (strict &Theme token verification).
    let light = Theme::studio_light();
    let px_light = gpu.render_rgba8(&fv_mix, &light).unwrap();
    assert!(distinct_pixel_ratio(&px_light, light.surface.canvas) > 0.25);
    assert_ne!(px_light, px_mix);
}

#[test]
fn gpu_renders_bank_32_slot_scroll_window_compact_and_zoomed() {
    let Ok(mut gpu) = HeadlessGpu::new(1440, 860) else {
        return;
    };

    let mut project = Project::demo();
    let base_pat = project.patterns[0].id;
    let inst0_id = project.instruments[0].id;
    let next_start = project.max_id() + 1;
    for i in 0..22u64 {
        let pid = next_start + i;
        project.parts.push(cadence_core::material::Part {
            id: pid,
            name: format!("Acid {:02}", i + 1),
            pattern: base_pat,
            instrument: inst0_id,
            transpose: (i % 7) as i16,
            velocity_scale: 1.0,
            parameter_links: Vec::new(),
        });
        if project.lanes[0].parts.len() < 22 {
            project.lanes[0]
                .parts
                .push(cadence_core::material::LanePart::new(pid));
        }
    }
    project.lanes[0].parts.truncate(22);
    project.validate().unwrap();

    let snap = common::build_snapshot(&project, 12);
    let theme = Theme::cadence_dark();
    let status = "Metal · 48 kHz";
    let mut app = App::new(project, PathBuf::from("bank_32_gpu.json")).unwrap();
    app.press_keys("gb").unwrap();
    // Scroll part cursor right (`l`) to slot 15 (0-indexed 14) so both ◀ and ▶ scroll indicators appear!
    for _ in 0..14 {
        app.press_keys("l").unwrap();
    }

    let fv_compact = app.frame_view(&snap, status);
    let px_compact = gpu.render_rgba8(&fv_compact, &theme).unwrap();
    assert!(distinct_pixel_ratio(&px_compact, theme.surface.canvas) > 0.20);
    maybe_save_png("05-bank-32slot-scroll-compact.png", 1440, 860, &px_compact);

    app.press_keys("zz").unwrap();
    let fv_zoomed = app.frame_view(&snap, status);
    let px_zoomed = gpu.render_rgba8(&fv_zoomed, &theme).unwrap();
    assert!(distinct_pixel_ratio(&px_zoomed, theme.surface.canvas) > 0.20);
    assert_ne!(px_zoomed, px_compact);
    maybe_save_png("06-bank-32slot-scroll-zoomed.png", 1440, 860, &px_zoomed);
}
