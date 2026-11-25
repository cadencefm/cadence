//! Headless Metal/wgpu GPU visual tests for Overlays, Modals, Workflows & Chrome Badges:
//! 1. Instrument Preset Browser (`P`), Song Properties (`gP`),
//!    Dedicated Mini-Notation Editor (`Mode::MiniEditor` Valid vs Error), Fuzzy Action Palette
//!    (`:` / `Ctrl+p`), and Unified `Mode::ValueInput` exact entry popover (Dark & Light).
//! 2. Chrome `LiveTiming` / `Boundary::Cycle` pill, Mixer + `Detail::Channel` (Inserts/Sends/Ducks),
//!    Settings scrolled to `LIVE TIMING (PROJECT)`, Pattern Derive overlay scroll counter, and
//!    `Detail::Effect` routing controls.
//! 3. Chrome take-recording badges (`TAKE ARM`, `● REC TAKE (N)`), Take Deck with `VISUAL` range
//!    marking, Sampler `SOURCE · Sample` row + Sample Browser search, File Hub search, and
//!    Settings `Auto-Record Takes`.
//!
//! Set `CADENCE_DUMP_OVERLAYS_DIR=/path/to/dir` to export PNG screenshots for visual inspection.

mod common;

use cadence_app::view::OverlayView;
use cadence_app::{Action, App, ControlTarget, KeyCode, KeyEvent, KeyModifiers};
use cadence_core::config::ThemeId;
use cadence_core::material::{Boundary, Project};
use cadence_core::mixer::{ChannelKind, ChannelRef};
use cadence_core::params::ParamAddress;
use cadence_core::presets::{InstrumentPreset, PresetCatalog};
use cadence_core::rack::{ModuleKind, ModuleParam, RackPatch};
use cadence_core::runtime::AudioSnapshot;
use cadence_core::sampler::SampleLibrary;
use cadence_gui::headless::{distinct_pixel_ratio, HeadlessGpu};
use cadence_gui::theme::Theme;

const DUMP_ENV: &str = "CADENCE_DUMP_OVERLAYS_DIR";

fn render_frame(
    gpu: &mut HeadlessGpu,
    app: &App,
    snap: &AudioSnapshot,
    theme_id: ThemeId,
    name: &str,
) -> Vec<u8> {
    let mut fv = app.frame_view(snap, "48kHz · Metal");
    fv.chrome.theme = theme_id;
    let theme = Theme::from_id(theme_id);
    let rgba = gpu.render_rgba8(&fv, &theme).expect("render_rgba8");
    assert!(
        distinct_pixel_ratio(&rgba, theme.surface.canvas) > 0.08,
        "{name} rendered nearly blank"
    );
    common::maybe_save_png(DUMP_ENV, name, 1440, 860, &rgba);
    rgba
}

#[test]
fn gpu_renders_preset_browser_song_props_mini_editor_palette_and_value_input() {
    let Ok(mut gpu) = HeadlessGpu::new(1440, 860) else {
        eprintln!("No GPU adapter available; skipping overlays_gpu");
        return;
    };

    let tmp_root = common::TempTestDir::new("overlays-presets");
    let presets_dir = tmp_root.path().join("presets");
    std::fs::create_dir_all(&presets_dir).unwrap();
    for (slug, name, cat, cutoff, res) in [
        ("acid-303", "Acid 303", "Bass", 980.0, 0.72),
        ("pulse-lead", "Pulse Lead", "Lead", 4200.0, 0.34),
        ("warm-pad", "Warm Pad", "Pad", 2400.0, 0.22),
        ("reese-bass", "Reese Bass", "Bass", 640.0, 0.30),
        ("glass-pluck", "Glass Pluck", "Pluck", 5600.0, 0.28),
        ("sub-808", "Sub 808", "Drums", 280.0, 0.12),
    ] {
        let mut patch = RackPatch::default_synth();
        for m in &mut patch.zones[0].modules {
            if let ModuleKind::Filter(f) = &mut m.kind {
                f.cutoff_hz = cutoff;
                f.resonance = res;
            }
        }
        InstrumentPreset::new(name, Some(cat.into()), patch)
            .save(&presets_dir.join(format!("{slug}.preset.json")))
            .unwrap();
    }

    let mut app = App::new(Project::demo(), tmp_root.path().join("demo.json")).unwrap();
    app.set_preset_catalog(PresetCatalog::from_roots(&[presets_dir]));
    app.execute_command("section Intro 1 4").unwrap();
    app.execute_command("section Drop 5 4").unwrap();
    app.execute_command("mini AcidRiff 16 c4 [e4 g4] ~ [b4 c5] g4 ~ e4 d4")
        .unwrap();

    let snap = common::build_snapshot(app.project(), 32);

    // 1. Instrument Preset Browser (`P`) with live preview badge + search bar.
    let inst_name = app.project().instruments[0].name.clone();
    app.execute_command(&format!("instrument {inst_name}"))
        .unwrap();
    app.press_keys("Pj").unwrap();
    assert!(matches!(
        app.frame_view(&snap, "ok").overlay,
        Some(OverlayView::PresetBrowser(_))
    ));
    let pb_dark = render_frame(
        &mut gpu,
        &app,
        &snap,
        ThemeId::CadenceDark,
        "01-preset-browser-dark.png",
    );
    let pb_light = render_frame(
        &mut gpu,
        &app,
        &snap,
        ThemeId::StudioLight,
        "02-preset-browser-light.png",
    );
    assert!(common::frame_diff_ratio(&pb_dark, &pb_light) > 0.50);
    app.press_keys("<Esc>").unwrap();

    // 2. Song Properties modal (`gP`): default view vs direct Scale cycle (`Alt+l`).
    app.press_keys("gP").unwrap();
    assert!(matches!(
        app.frame_view(&snap, "ok").overlay,
        Some(OverlayView::SongProperties(_))
    ));
    let sp_norm = render_frame(
        &mut gpu,
        &app,
        &snap,
        ThemeId::CadenceDark,
        "03-song-props-normal-dark.png",
    );
    app.press_keys("jjj<m-l>").unwrap(); // Scale row -> cycle directly to next scale
    let sp_edit_dark = render_frame(
        &mut gpu,
        &app,
        &snap,
        ThemeId::CadenceDark,
        "04-song-props-edit-dark.png",
    );
    let sp_edit_light = render_frame(
        &mut gpu,
        &app,
        &snap,
        ThemeId::StudioLight,
        "05-song-props-edit-light.png",
    );
    assert!(common::frame_diff_ratio(&sp_norm, &sp_edit_dark) > 0.005);
    assert!(common::frame_diff_ratio(&sp_edit_dark, &sp_edit_light) > 0.50);
    app.press_keys("<Esc>").unwrap();

    // 3. Dedicated Mini-Notation Editor (`Mode::MiniEditor`): Valid green vs Error red.
    app.execute_command("open AcidRiff").unwrap();
    app.press_keys("gp<Enter>").unwrap();
    assert!(matches!(
        app.frame_view(&snap, "ok").overlay,
        Some(OverlayView::MiniEditor(ref m)) if m.valid
    ));
    let mini_valid_dark = render_frame(
        &mut gpu,
        &app,
        &snap,
        ThemeId::CadenceDark,
        "06-mini-editor-valid-dark.png",
    );
    let mini_valid_light = render_frame(
        &mut gpu,
        &app,
        &snap,
        ThemeId::StudioLight,
        "08-mini-editor-valid-light.png",
    );
    // Introduce unclosed bracket to verify red live syntax error state.
    app.press_keys(" [c5").unwrap();
    assert!(matches!(
        app.frame_view(&snap, "ok").overlay,
        Some(OverlayView::MiniEditor(ref m)) if !m.valid
    ));
    let mini_err_dark = render_frame(
        &mut gpu,
        &app,
        &snap,
        ThemeId::CadenceDark,
        "07-mini-editor-error-dark.png",
    );
    assert!(common::frame_diff_ratio(&mini_valid_dark, &mini_err_dark) > 0.01);
    assert!(common::frame_diff_ratio(&mini_valid_dark, &mini_valid_light) > 0.50);
    app.press_keys("<Esc>").unwrap();

    // 4. Fuzzy Action Palette (`:` / `Ctrl+Shift+p`): empty, fuzzy filtered, CMD passthrough, Light.
    app.handle_key(KeyEvent::new(
        KeyCode::Char('P'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    ))
    .unwrap();
    assert!(matches!(
        app.frame_view(&snap, "ok").overlay,
        Some(OverlayView::Palette(_))
    ));
    let pal_empty = render_frame(
        &mut gpu,
        &app,
        &snap,
        ThemeId::CadenceDark,
        "09-palette-empty-dark.png",
    );
    app.press_keys("song key").unwrap();
    let pal_fuzzy_dark = render_frame(
        &mut gpu,
        &app,
        &snap,
        ThemeId::CadenceDark,
        "10-palette-fuzzy-dark.png",
    );
    let pal_fuzzy_light = render_frame(
        &mut gpu,
        &app,
        &snap,
        ThemeId::StudioLight,
        "12-palette-fuzzy-light.png",
    );
    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
        .unwrap();
    app.press_keys("bpm 136").unwrap();
    let pal_cmd_dark = render_frame(
        &mut gpu,
        &app,
        &snap,
        ThemeId::CadenceDark,
        "11-palette-cmd-dark.png",
    );
    assert!(common::frame_diff_ratio(&pal_empty, &pal_fuzzy_dark) > 0.01);
    assert!(common::frame_diff_ratio(&pal_fuzzy_dark, &pal_cmd_dark) > 0.01);
    assert!(common::frame_diff_ratio(&pal_fuzzy_dark, &pal_fuzzy_light) > 0.50);
    app.press_keys("<Esc>").unwrap();

    // 5. Unified ValueInput popover on Instrument `Cutoff` with live preview.
    app.execute_command(&format!("instrument {inst_name}"))
        .unwrap();
    let cutoff_idx = app
        .control_descriptors()
        .iter()
        .position(|c| {
            matches!(
                c.target,
                ControlTarget::Param(ParamAddress::Module {
                    param: ModuleParam::Cutoff,
                    ..
                })
            )
        })
        .expect("Cutoff control");
    app.control_cursor = cutoff_idx;
    app.press_keys("<Enter>1.75khz").unwrap();
    assert!(matches!(
        app.frame_view(&snap, "ok").overlay,
        Some(OverlayView::ValueInput(ref v)) if v.live_preview.is_some()
    ));
    let vi_dark = render_frame(
        &mut gpu,
        &app,
        &snap,
        ThemeId::CadenceDark,
        "13-value-input-cutoff-dark.png",
    );
    let vi_light = render_frame(
        &mut gpu,
        &app,
        &snap,
        ThemeId::StudioLight,
        "14-value-input-cutoff-light.png",
    );
    assert!(common::frame_diff_ratio(&vi_dark, &vi_light) > 0.50);
    app.press_keys("<Esc>").unwrap();
}

#[test]
fn gpu_renders_live_timing_chrome_settings_authoring_mixer_and_effect_routing() {
    let Ok(mut gpu) = HeadlessGpu::new(1440, 860) else {
        return;
    };

    let mut app = App::new(Project::demo(), "overlays-workflows-gpu.json".into()).unwrap();
    let dark = Theme::cadence_dark();
    let status = "Metal · 48 kHz";
    let snap = AudioSnapshot {
        playing: true,
        absolute_step: 16,
        step_fraction: 0.25,
        sample_rate: 48_000,
        peak: 0.52,
        peak_left: 0.52,
        peak_right: 0.48,
        ..AudioSnapshot::default()
    };

    // Configure LiveTiming Cycle + EditTiming::Launch so Chrome displays the Q: cycle pill.
    let pat_id = app.project().patterns[0].id;
    app.press_keys("tQ<Enter>tE").unwrap();
    assert_eq!(
        app.project().timing.launch,
        Boundary::Cycle { pattern: pat_id }
    );

    // 1. Mixer + Channel Detail with Inserts, Sends, and Duck controls + Chrome Cycle pill.
    app.press_keys("gm").unwrap();
    app.apply(Action::CreateChannel {
        name: "VerbRet".into(),
        kind: ChannelKind::Return,
    })
    .unwrap();
    let ret_id = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "VerbRet")
        .unwrap()
        .id;
    let lead_ch = app.project().mixer.channels[0].id;
    app.apply(Action::SetChannelSend {
        channel: lead_ch,
        target: ret_id,
        amount: Some(0.35),
    })
    .unwrap();
    app.apply(Action::AddEffect {
        channel: ChannelRef::Channel(lead_ch),
        kind: "eq".into(),
    })
    .unwrap();
    app.apply(Action::SelectChannel {
        channel: ChannelRef::Channel(lead_ch),
    })
    .unwrap();
    let fv1 = app.frame_view(&snap, status);
    let px1 = gpu.render_rgba8(&fv1, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px1, dark.surface.canvas) > 0.20);
    common::maybe_save_png(
        DUMP_ENV,
        "15-chrome-cycle-and-channel-detail.png",
        1440,
        860,
        &px1,
    );

    // 2. Settings overlay scrolled to `LIVE TIMING (PROJECT)` rows 12–14.
    app.press_keys("g,jjjjjjjjjjjj").unwrap();
    let fv2 = app.frame_view(&snap, status);
    let px2 = gpu.render_rgba8(&fv2, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px2, dark.surface.canvas) > 0.20);
    common::maybe_save_png(
        DUMP_ENV,
        "16-settings-live-timing-scrolled.png",
        1440,
        860,
        &px2,
    );
    app.press_keys("<Esc>").unwrap();

    // 3. Pattern Derive overlay showing all 16 derived operators + scroll counter (`sel/16`).
    app.press_keys("gpdDeriveTest<Enter>jjjjjj").unwrap();
    let fv3 = app.frame_view(&snap, status);
    let px3 = gpu.render_rgba8(&fv3, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px3, dark.surface.canvas) > 0.20);
    common::maybe_save_png(DUMP_ENV, "17-derive-16-ops-overlay.png", 1440, 860, &px3);
    app.press_keys("<Esc>").unwrap();

    // 4. Effect Deck with ROUTING · Channel & Slot controls sized via `control_list_preferred_height`.
    let fx_id = app.channel(lead_ch).unwrap().inserts[0].id;
    app.execute_command(&format!("fx {fx_id}")).unwrap();
    let fv4 = app.frame_view(&snap, status);
    let px4 = gpu.render_rgba8(&fv4, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px4, dark.surface.canvas) > 0.20);
    common::maybe_save_png(
        DUMP_ENV,
        "18-effect-deck-routing-controls.png",
        1440,
        860,
        &px4,
    );
}

#[test]
fn gpu_renders_chrome_take_badges_visual_marks_browser_filehub_and_settings() {
    let Ok(mut gpu) = HeadlessGpu::new(1440, 860) else {
        return;
    };

    let dir = common::TempTestDir::new("overlays-takes");
    let samples_dir = dir.path().join("samples");
    std::fs::create_dir_all(&samples_dir).unwrap();
    common::write_test_wav(&samples_dir.join("alpha-kick.wav"), 2400, 48_000);
    common::write_test_wav(&samples_dir.join("beta-snare.wav"), 2400, 48_000);

    let save_path = dir.path().join("session.json");
    let lib = SampleLibrary::from_roots(&[samples_dir], Some(&save_path));
    let mut app = App::new_with_library(Project::demo(), save_path, lib).unwrap();
    let dark = Theme::cadence_dark();
    let status = "Metal · 48 kHz";

    // 1. Chrome `TAKE ARM` amber badge + Empty Armed Take Deck (`gt`).
    app.press_keys("trgt").unwrap();
    let fv1 = app.frame_view(
        &AudioSnapshot {
            playing: false,
            sample_rate: 48_000,
            ..AudioSnapshot::default()
        },
        status,
    );
    let px1 = gpu.render_rgba8(&fv1, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px1, dark.surface.canvas) > 0.15);
    common::maybe_save_png(
        DUMP_ENV,
        "19-chrome-take-arm-empty-deck.png",
        1440,
        860,
        &px1,
    );

    // 2. Chrome `● REC TAKE (N)` red badge + Active Take Deck with `VISUAL` range marking (`vll`).
    app.press_keys(" ").unwrap();
    app.apply(Action::Advance { steps: 16 }).unwrap();
    app.execute_command("bpm 142").unwrap();
    app.apply(Action::Advance { steps: 64 }).unwrap();
    app.press_keys("gtlvll").unwrap();
    let fv2 = app.frame_view(
        &AudioSnapshot {
            playing: true,
            absolute_step: app.transport_step(),
            sample_rate: 48_000,
            peak: 0.45,
            peak_left: 0.45,
            peak_right: 0.42,
            ..AudioSnapshot::default()
        },
        status,
    );
    let px2 = gpu.render_rgba8(&fv2, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px2, dark.surface.canvas) > 0.15);
    common::maybe_save_png(
        DUMP_ENV,
        "20-chrome-rec-take-visual-mark.png",
        1440,
        860,
        &px2,
    );

    // 3. Sampler Deck with `SOURCE · Sample` top row + Sample Browser committed search filter.
    app.press_keys("tr<Home>").unwrap();
    app.execute_command("sampler DrumBox").unwrap();
    app.press_keys("gil").unwrap();
    let fv3 = app.frame_view(&AudioSnapshot::default(), status);
    let px3 = gpu.render_rgba8(&fv3, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px3, dark.surface.canvas) > 0.15);
    common::maybe_save_png(DUMP_ENV, "21-sampler-source-row.png", 1440, 860, &px3);

    // Open Sample Browser via Enter on `Sample` row and commit `/beta` filter.
    app.control_cursor = 0;
    app.press_keys("<Enter>/beta<Enter>").unwrap();
    let fv4 = app.frame_view(&AudioSnapshot::default(), status);
    let px4 = gpu.render_rgba8(&fv4, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px4, dark.surface.canvas) > 0.15);
    common::maybe_save_png(DUMP_ENV, "22-sample-browser-search.png", 1440, 860, &px4);
    app.press_keys("<Esc><Esc>").unwrap();

    // 4. File Hub search overlay + Settings `Auto-Record Takes` row.
    app.press_keys("gf/samples<Enter>").unwrap();
    let fv5 = app.frame_view(&AudioSnapshot::default(), status);
    assert!(matches!(fv5.overlay, Some(OverlayView::FileHub(_))));
    let px5 = gpu.render_rgba8(&fv5, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px5, dark.surface.canvas) > 0.15);
    common::maybe_save_png(DUMP_ENV, "23-filehub-search.png", 1440, 860, &px5);
    app.press_keys("<Esc><Esc>").unwrap();

    app.press_keys("g,jjjjj").unwrap();
    let fv6 = app.frame_view(&AudioSnapshot::default(), status);
    assert!(matches!(fv6.overlay, Some(OverlayView::Settings(_))));
    let px6 = gpu.render_rgba8(&fv6, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px6, dark.surface.canvas) > 0.15);
    common::maybe_save_png(
        DUMP_ENV,
        "24-settings-auto-record-takes.png",
        1440,
        860,
        &px6,
    );
}

#[test]
fn gpu_renders_wrapped_inspect_left_clipped_arrange_mixer_empty_and_rhythm_pattern() {
    let Ok(mut gpu) = HeadlessGpu::new(1440, 860) else {
        return;
    };
    let Ok(mut gpu_narrow) = HeadlessGpu::new(1024, 680) else {
        return;
    };

    let dir = common::TempTestDir::new("overlays-polish-visuals");
    let nested = dir
        .path()
        .join("studio/projects/afterglow/factory-samples/tr-808");
    std::fs::create_dir_all(&nested).unwrap();
    common::write_test_wav(
        &nested.join("vintage_808_kick_long_sample.wav"),
        2400,
        48_000,
    );
    let save_path = dir.path().join("session.json");
    let lib = SampleLibrary::from_roots(&[dir.path().to_path_buf()], Some(&save_path));

    let mut project = Project::demo();
    common::reset_inserts_and_channel_gain_automation(&mut project);
    let mut app = App::new_with_library(project, save_path, lib).unwrap();
    let snap = common::build_snapshot(app.project(), 48);
    let dark = Theme::cadence_dark();
    let status = "Metal · 48 kHz";

    // 1. Wrapped `draw_inspect` on a narrow 1024×680 pane (Assets overlay with deep sample paths).
    app.execute_command("assets").unwrap();
    let fv_inspect = app.frame_view(&snap, status);
    assert!(matches!(
        fv_inspect.overlay,
        Some(OverlayView::Inspect { .. })
    ));
    let px_inspect = gpu_narrow.render_rgba8(&fv_inspect, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px_inspect, dark.surface.canvas) > 0.15);
    common::maybe_save_png(
        DUMP_ENV,
        "25-inspect-wrapped-narrow.png",
        1024,
        680,
        &px_inspect,
    );
    app.press_keys("<Esc>").unwrap();

    // 2. Left-clipped Arrange placement titles clamped cleanly inside timeline left edge.
    //    `Moving`/`Bass`/`Kick` run steps 64..128 (bars 5..8); at zoom 8.0 (32 steps/page) and
    //    cursor step 96 (bars 7..8), all three placements clip on the left edge of the timeline!
    app.press_keys("gazz").unwrap();
    app.arrange_ui.zoom = 8.0;
    app.arrange_ui.step = 96;
    let fv_arr = app.frame_view(&snap, status);
    let arr_page_start = fv_arr
        .decks
        .iter()
        .find_map(|d| match &d.view {
            cadence_app::view::DeckView::Arrange(a) => Some(a.page_start_step),
            _ => None,
        })
        .expect("ArrangeDeckView");
    assert!(
        arr_page_start > 64,
        "expected left-clipped arrange page starting after step 64, got {arr_page_start}"
    );
    let px_arr = gpu.render_rgba8(&fv_arr, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px_arr, dark.surface.canvas) > 0.20);
    common::maybe_save_png(
        DUMP_ENV,
        "26-arrange-left-clipped-placement.png",
        1440,
        860,
        &px_arr,
    );
    app.press_keys("zz").unwrap();

    // 3. Mixer Audio strip with no inserts/sends/ducks displaying `f add fx · w add send · d add duck`.
    let clean_ch_id = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| {
            c.kind == ChannelKind::Audio
                && c.inserts.is_empty()
                && c.sends.is_empty()
                && app
                    .project()
                    .ducks
                    .iter()
                    .all(|d| d.target != ChannelRef::Channel(c.id))
        })
        .expect("Audio channel with no inserts/sends/ducks")
        .id;
    app.selected_channel = ChannelRef::Channel(clean_ch_id);
    app.press_keys("gmzz").unwrap();
    let fv_mix = app.frame_view(&snap, status);
    let px_mix = gpu.render_rgba8(&fv_mix, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px_mix, dark.surface.canvas) > 0.20);
    common::maybe_save_png(
        DUMP_ENV,
        "27-mixer-empty-audio-hint.png",
        1440,
        860,
        &px_mix,
    );
    app.press_keys("zz").unwrap();

    // 4. Pattern deck on a literal `PatternKind::Rhythm` displaying `Enter / x toggle hit · r edit`,
    //    and a non-literal `Mini` pattern hiding `o add note · i step insert` on rest steps.
    app.execute_command("pattern rhythm KickPulse x . . . x . x . x . . . x . x .")
        .unwrap();
    app.execute_command("open KickPulse").unwrap();
    app.press_keys("gpzz").unwrap();
    let fv_rhy = app.frame_view(&snap, status);
    let px_rhy = gpu.render_rgba8(&fv_rhy, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px_rhy, dark.surface.canvas) > 0.20);
    common::maybe_save_png(DUMP_ENV, "28-pattern-rhythm-cue.png", 1440, 860, &px_rhy);

    app.execute_command("mini RestMini 8 ~ c4 ~ e4 ~ g4 ~ b4")
        .unwrap();
    app.execute_command("open RestMini").unwrap();
    let fv_mini = app.frame_view(&snap, status);
    let px_mini = gpu.render_rgba8(&fv_mini, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px_mini, dark.surface.canvas) > 0.20);
    common::maybe_save_png(
        DUMP_ENV,
        "29-pattern-mini-rest-cue.png",
        1440,
        860,
        &px_mini,
    );
}
