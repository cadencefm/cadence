//! Headless Metal/wgpu GPU visual tests for the Modeless Input Architecture:
//! 1. Semantic accents (`accent.focus` cyan for Normal decks, `accent.edit` warm amber for
//!    overlay inputs like `ValueInput`/`MiniEditor`, `accent.jam` violet for Jam HUD,
//!    `accent.playhead` green for Step Insert) across Dark, Neon, and Studio Light themes.
//! 2. Modeless direct-manipulation rendering across decks and overlays:
//!    - Instrument Panel with direct `Alt+L` sculpting + `Enter` exact `ValueInput` overlay
//!    - Polyphonic Pattern chords (`o` stays in `NORMAL`, `Alt+L` lengthens Voice 2/3, `i` Step Insert)
//!    - Focusable Mixer strip rows (`Fader`, `Pan`, `Insert(0)`) with direct `Alt` sculpting
//!    - Jam violet HUD (`gj` / `Ctrl+Shift+j`)
//!    - Macros dial & binding rows, Settings (`g,`), Song Properties (`gP`), and Picker search vs committed filter
//!
//! Set `CADENCE_DUMP_MODELESS_GPU_DIR=/path/to/dir` to export PNG screenshots for visual inspection.

mod common;

use cadence_app::view::{DeckView, OverlayView, PatternContentView, UiModeKind};
use cadence_app::{App, KeyCode, KeyEvent, KeyModifiers, MixerSlot};
use cadence_core::material::Project;
use cadence_gui::decks::build_display_list;
use cadence_gui::headless::{distinct_pixel_ratio, HeadlessGpu};
use cadence_gui::theme::{Color, Theme};
use common::frame_diff_ratio;
use std::path::PathBuf;

fn maybe_save_png(name: &str, width: u32, height: u32, rgba: &[u8]) {
    common::maybe_save_png("CADENCE_DUMP_MODELESS_GPU_DIR", name, width, height, rgba);
}

fn color_distance(a: Color, b: Color) -> f32 {
    let dr = (a.r as f32 - b.r as f32) / 255.0;
    let dg = (a.g as f32 - b.g as f32) / 255.0;
    let db = (a.b as f32 - b.b as f32) / 255.0;
    (dr * dr + dg * dg + db * db).sqrt()
}

fn send_key(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
        .unwrap();
}

#[test]
fn theme_tokens_expose_distinct_focus_overlay_and_jam_accents() {
    for (name, theme) in [
        ("Cadence Dark", Theme::cadence_dark()),
        ("Midnight Neon", Theme::midnight_neon()),
        ("Studio Light", Theme::studio_light()),
    ] {
        let d_fe = color_distance(theme.accent.focus, theme.accent.edit);
        let d_fj = color_distance(theme.accent.focus, theme.accent.jam);
        let d_ej = color_distance(theme.accent.edit, theme.accent.jam);
        assert!(
            d_fe > 0.30,
            "{name}: focus vs overlay-edit accent too close ({d_fe:.3})"
        );
        assert!(
            d_fj > 0.30,
            "{name}: focus vs jam accent too close ({d_fj:.3})"
        );
        assert!(
            d_ej > 0.30,
            "{name}: overlay-edit vs jam accent too close ({d_ej:.3})"
        );
        assert!(
            theme.glow.jam_border > 0.0,
            "{name}: jam_border glow must be positive"
        );
    }
}

#[test]
fn gpu_modeless_decks_value_input_chords_mixer_jam_and_overlays() {
    let Ok(mut gpu) = HeadlessGpu::new(1440, 860) else {
        // Gracefully skip in GPU-less CI containers.
        return;
    };
    let dark = Theme::cadence_dark();
    let neon = Theme::midnight_neon();
    let light = Theme::studio_light();
    let status = "Metal · 48 kHz";

    // -----------------------------------------------------------------------
    // 1. Instrument Deck: Modeless cyan focus + Enter ValueInput amber overlay
    // -----------------------------------------------------------------------
    let mut app = App::new(Project::demo(), PathBuf::from("modeless_gpu.json")).unwrap();
    let snap = common::build_snapshot(app.project(), 18);
    app.press_keys("gizz").unwrap();
    for _ in 0..16 {
        let fv = app.frame_view(&snap, status);
        if fv.decks.iter().any(|d| match &d.view {
            DeckView::Instrument(iv) => iv
                .controls
                .iter()
                .any(|c| c.selected && c.label.eq_ignore_ascii_case("Cutoff")),
            _ => false,
        }) {
            break;
        }
        app.press_keys("j").unwrap();
    }
    // Direct Tier 3 adjustment stays in Normal mode with cyan focus chrome.
    app.press_keys("<m-L>").unwrap();
    let fv_inst = app.frame_view(&snap, status);
    assert_eq!(fv_inst.chrome.mode_kind, UiModeKind::Normal);
    let dl_inst = build_display_list(&fv_inst, 1440.0, 860.0, 1.0, &dark);
    assert!(
        !dl_inst.base.texts.iter().any(|t| t.text == "EDIT"),
        "Modeless Instrument frame must stay in Normal focus chrome without an EDIT badge"
    );
    let px_inst_dark = gpu.render_rgba8(&fv_inst, &dark).unwrap();
    let px_inst_light = gpu.render_rgba8(&fv_inst, &light).unwrap();
    let px_inst_neon = gpu.render_rgba8(&fv_inst, &neon).unwrap();
    assert!(distinct_pixel_ratio(&px_inst_dark, dark.surface.canvas) > 0.20);
    maybe_save_png("01-instrument-modeless-dark.png", 1440, 860, &px_inst_dark);
    maybe_save_png(
        "02-instrument-modeless-light.png",
        1440,
        860,
        &px_inst_light,
    );
    maybe_save_png("03-instrument-modeless-neon.png", 1440, 860, &px_inst_neon);

    // `Enter` opens exact `ValueInput` overlay directly!
    send_key(&mut app, KeyCode::Enter);
    let fv_val = app.frame_view(&snap, status);
    assert_eq!(fv_val.chrome.mode_kind, UiModeKind::Overlay);
    let px_val = gpu.render_rgba8(&fv_val, &dark).unwrap();
    assert!(
        frame_diff_ratio(&px_inst_dark, &px_val) > 0.005,
        "ValueInput overlay must visibly render over Instrument deck"
    );
    maybe_save_png("04-instrument-value-input-dark.png", 1440, 860, &px_val);
    send_key(&mut app, KeyCode::Esc);

    // -----------------------------------------------------------------------
    // 2. Pattern 2D Polyphonic Chords (modeless sculpting + `i` Step Insert)
    // -----------------------------------------------------------------------
    app.execute_command("pattern notes ChordVisual 0 . 2 . 4 . 7 . 0 . 3 . 5 . 7 .")
        .unwrap();
    app.execute_command("open ChordVisual").unwrap();
    app.press_keys("gp").unwrap();
    app.press_keys("oo").unwrap(); // stays in NORMAL!
    app.press_keys("j").unwrap(); // Voice 3/3 -> Voice 2/3
    app.press_keys("<m-L><m-L>").unwrap(); // lengthen Voice 2/3 gate directly!
    let fv_pat = app.frame_view(&snap, status);
    assert_eq!(fv_pat.chrome.mode_kind, UiModeKind::Normal);
    let pat_ok = fv_pat.decks.iter().any(|d| match &d.view {
        DeckView::Pattern(pv) => {
            let readout_ok = pv.voice_readout.as_deref().unwrap_or("").contains("2/3");
            let single_selected = match &pv.content {
                PatternContentView::Notes { steps, .. } => {
                    steps[0].voices.len() == 3
                        && steps[0].voices.iter().filter(|v| v.selected).count() == 1
                        && steps[0].voices[1].selected
                }
                _ => false,
            };
            readout_ok && single_selected
        }
        _ => false,
    });
    assert!(pat_ok, "Pattern view must highlight ONLY Voice 2/3");
    let px_pat_dark = gpu.render_rgba8(&fv_pat, &dark).unwrap();
    let px_pat_light = gpu.render_rgba8(&fv_pat, &light).unwrap();
    maybe_save_png("05-pattern-chord-dark.png", 1440, 860, &px_pat_dark);
    maybe_save_png("06-pattern-chord-light.png", 1440, 860, &px_pat_light);

    // `i` enters green Step Insert directly!
    app.press_keys("i").unwrap();
    let fv_ins = app.frame_view(&snap, status);
    let px_ins = gpu.render_rgba8(&fv_ins, &dark).unwrap();
    assert!(
        frame_diff_ratio(&px_pat_dark, &px_ins) > 0.001,
        "Step Insert pill & column highlight must differ from Normal"
    );
    maybe_save_png("07-pattern-step-insert-dark.png", 1440, 860, &px_ins);
    send_key(&mut app, KeyCode::Esc);

    // -----------------------------------------------------------------------
    // 3. Focusable Mixer Strip Rows: Fader / Pan / Insert
    // -----------------------------------------------------------------------
    app.execute_command("channel return ReverbRet").unwrap();
    let audio_id = app.project().mixer.channels[0].id;
    let ret_id = app
        .project()
        .mixer
        .channels
        .iter()
        .find(|c| c.name == "ReverbRet")
        .unwrap()
        .id;
    let part_id = app.project().parts[0].id;
    app.execute_command(&format!("fx add filter {audio_id}"))
        .unwrap();
    app.execute_command(&format!("fx add delay {audio_id}"))
        .unwrap();
    app.execute_command(&format!("send {audio_id} {ret_id} 0.42"))
        .unwrap();
    app.execute_command(&format!("duck Pump {part_id} {audio_id} 0.35 150ms"))
        .unwrap();

    let snap_mix = common::build_snapshot(app.project(), 18);
    app.press_keys("gm").unwrap();
    let fv_mix_fader = app.frame_view(&snap_mix, status);
    assert!(fv_mix_fader.decks.iter().any(|d| matches!(
        &d.view,
        DeckView::Mixer(mv) if mv.focused_slot == MixerSlot::Fader
    )));
    let px_mix_fader = gpu.render_rgba8(&fv_mix_fader, &dark).unwrap();
    maybe_save_png("08-mixer-fader-dark.png", 1440, 860, &px_mix_fader);

    app.press_keys("j<m-L>").unwrap(); // Pan + coarse pan right directly!
    let fv_mix_pan = app.frame_view(&snap_mix, status);
    assert!(fv_mix_pan.decks.iter().any(|d| matches!(
        &d.view,
        DeckView::Mixer(mv) if mv.focused_slot == MixerSlot::Pan
    )));
    let px_mix_pan_dark = gpu.render_rgba8(&fv_mix_pan, &dark).unwrap();
    let px_mix_pan_light = gpu.render_rgba8(&fv_mix_pan, &light).unwrap();
    assert!(frame_diff_ratio(&px_mix_fader, &px_mix_pan_dark) > 0.002);
    maybe_save_png("09-mixer-pan-dark.png", 1440, 860, &px_mix_pan_dark);
    maybe_save_png("10-mixer-pan-light.png", 1440, 860, &px_mix_pan_light);

    app.press_keys("j").unwrap(); // Insert(0)
    let fv_mix_insert = app.frame_view(&snap_mix, status);
    assert!(fv_mix_insert.decks.iter().any(|d| matches!(
        &d.view,
        DeckView::Mixer(mv) if mv.focused_slot == MixerSlot::Insert(0)
    )));
    let px_mix_insert = gpu.render_rgba8(&fv_mix_insert, &dark).unwrap();
    maybe_save_png("11-mixer-insert-dark.png", 1440, 860, &px_mix_insert);

    // -----------------------------------------------------------------------
    // 4. JAM Mode via `gj`: Electric Violet Deck Border + Jam HUD
    // -----------------------------------------------------------------------
    app.press_keys("gp").unwrap();
    app.press_keys("gj").unwrap();
    let fv_jam = app.frame_view(&snap_mix, status);
    assert_eq!(fv_jam.chrome.mode_kind, UiModeKind::Jam);
    let px_jam_dark = gpu.render_rgba8(&fv_jam, &dark).unwrap();
    let px_jam_light = gpu.render_rgba8(&fv_jam, &light).unwrap();
    maybe_save_png("12-jam-violet-dark.png", 1440, 860, &px_jam_dark);
    maybe_save_png("13-jam-violet-light.png", 1440, 860, &px_jam_light);
    send_key(&mut app, KeyCode::Esc);

    // -----------------------------------------------------------------------
    // 5. Macros, Settings, Song Properties & Picker Search/Committed Filter
    // -----------------------------------------------------------------------
    let synth_name = app.project().instruments[0].name.clone();
    app.execute_command(&format!(
        "macro 1 bind instrument {synth_name} cutoff 0.15 0.85"
    ))
    .unwrap();
    app.execute_command(&format!(
        "macro 1 bind instrument {synth_name} resonance 0.10 0.70"
    ))
    .unwrap();
    app.press_keys("gkj").unwrap();
    let fv_macros = app.frame_view(&snap_mix, status);
    assert_eq!(fv_macros.chrome.mode_kind, UiModeKind::Normal);
    let px_macros = gpu.render_rgba8(&fv_macros, &dark).unwrap();
    maybe_save_png("14-macros-binding-dark.png", 1440, 860, &px_macros);

    app.press_keys("g,").unwrap();
    let fv_settings = app.frame_view(&snap_mix, status);
    assert_eq!(fv_settings.chrome.mode_kind, UiModeKind::Overlay);
    let px_settings = gpu.render_rgba8(&fv_settings, &dark).unwrap();
    maybe_save_png("15-settings-dark.png", 1440, 860, &px_settings);
    send_key(&mut app, KeyCode::Esc);

    app.press_keys("gP").unwrap();
    let fv_song_props = app.frame_view(&snap_mix, status);
    assert_eq!(fv_song_props.chrome.mode_kind, UiModeKind::Overlay);
    let px_song_props = gpu.render_rgba8(&fv_song_props, &dark).unwrap();
    maybe_save_png("16-song-props-dark.png", 1440, 860, &px_song_props);
    send_key(&mut app, KeyCode::Esc);

    // Picker active searching vs committed filter
    app.press_keys("tQ/bar").unwrap();
    let fv_pick_search = app.frame_view(&snap_mix, status);
    assert!(matches!(
        fv_pick_search.overlay,
        Some(OverlayView::Picker(ref p)) if p.searching && p.filter.as_deref() == Some("bar")
    ));
    let px_pick_search = gpu.render_rgba8(&fv_pick_search, &dark).unwrap();
    maybe_save_png("17-picker-searching-dark.png", 1440, 860, &px_pick_search);

    send_key(&mut app, KeyCode::Enter);
    let fv_pick_committed = app.frame_view(&snap_mix, status);
    assert!(matches!(
        fv_pick_committed.overlay,
        Some(OverlayView::Picker(ref p)) if !p.searching && p.filter.as_deref() == Some("bar")
    ));
    let px_pick_committed = gpu.render_rgba8(&fv_pick_committed, &dark).unwrap();
    assert!(
        frame_diff_ratio(&px_pick_search, &px_pick_committed) > 0.0002,
        "Picker active search pill vs committed filter pill must visually differ"
    );
    maybe_save_png(
        "18-picker-committed-dark.png",
        1440,
        860,
        &px_pick_committed,
    );
    send_key(&mut app, KeyCode::Esc);
    send_key(&mut app, KeyCode::Esc);

    // -----------------------------------------------------------------------
    // 6. Values Pattern Deck: Literal VALUES pill vs Derived DERIVED pill
    // -----------------------------------------------------------------------
    app.execute_command("pattern values SweepCurve 16 ramp 0:0.15 8:0.85 12:0.45")
        .unwrap();
    app.execute_command("open SweepCurve").unwrap();
    app.press_keys("gpzz").unwrap();
    let fv_val_lit = app.frame_view(&snap_mix, status);
    assert!(fv_val_lit.decks.iter().any(|d| matches!(
        &d.view,
        DeckView::Pattern(pv)
            if pv.editable_literal && matches!(pv.content, PatternContentView::Values { .. })
    )));
    let px_val_lit = gpu.render_rgba8(&fv_val_lit, &dark).unwrap();
    maybe_save_png("19-values-literal-dark.png", 1440, 860, &px_val_lit);

    app.execute_command("derive ShiftedSweep shift SweepCurve 4")
        .unwrap();
    app.execute_command("open ShiftedSweep").unwrap();
    let fv_val_drv = app.frame_view(&snap_mix, status);
    assert!(fv_val_drv.decks.iter().any(|d| matches!(
        &d.view,
        DeckView::Pattern(pv)
            if !pv.editable_literal && matches!(pv.content, PatternContentView::Values { .. })
    )));
    let px_val_drv = gpu.render_rgba8(&fv_val_drv, &dark).unwrap();
    assert!(
        frame_diff_ratio(&px_val_lit, &px_val_drv) > 0.002,
        "Literal vs derived Values curves must visually differ"
    );
    maybe_save_png("20-values-derived-dark.png", 1440, 860, &px_val_drv);
}
