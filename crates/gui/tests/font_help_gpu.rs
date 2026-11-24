//! Headless Metal/wgpu GPU visual tests for:
//! 1. Hermetic embedded JetBrains Mono Regular (400) & Bold (700) typography
//!    (zero OS proportional fallback, identical monospace alignment, zero `≠` coding ligatures)
//! 2. Multi-line word-wrapped `?` Help overlay across tabs, scroll offsets, resolutions
//!    (`1440×860` and `1100×720`), and themes (Cadence Dark & Studio Light).
//!
//! Set `CADENCE_DUMP_FONT_HELP_DIR=/path/to/dir` to export PNG screenshots for visual inspection.

mod common;

use cadence_app::view::{DeckView, HelpTab, OverlayView};
use cadence_app::App;
use cadence_core::material::Project;
use cadence_gui::decks::build_display_list;
use cadence_gui::display::{DisplayList, Rect, TextAlign};
use cadence_gui::headless::{distinct_pixel_ratio, HeadlessGpu};
use cadence_gui::theme::Theme;
use std::path::PathBuf;

fn maybe_save_png(name: &str, width: u32, height: u32, rgba: &[u8]) {
    common::maybe_save_png("CADENCE_DUMP_FONT_HELP_DIR", name, width, height, rgba);
}

#[test]
fn gpu_instrument_bold_alignment_and_no_coding_ligatures() {
    let Ok(mut gpu) = HeadlessGpu::new(1440, 860) else {
        // Skip gracefully in GPU-less CI containers.
        return;
    };
    let theme = Theme::cadence_dark();
    let mut app = App::new(Project::demo(), PathBuf::from("font_help_gpu.json")).unwrap();
    let snap = common::build_snapshot(app.project(), 24);
    let status = "Metal · 48 kHz";

    // Open Rack Instrument deck zoomed (`gi zz`) and move selection onto `Cutoff`.
    app.press_keys("gizz").unwrap();
    for _ in 0..16 {
        let fv = app.frame_view(&snap, status);
        let on_cutoff = fv.decks.iter().any(|slot| match &slot.view {
            DeckView::Instrument(iv) => iv
                .controls
                .iter()
                .any(|row| row.selected && row.label.eq_ignore_ascii_case("Cutoff")),
            _ => false,
        });
        if on_cutoff {
            break;
        }
        app.press_keys("j").unwrap();
    }

    let fv = app.frame_view(&snap, status);
    let cutoff_selected = fv.decks.iter().any(|slot| match &slot.view {
        DeckView::Instrument(iv) => iv
            .controls
            .iter()
            .any(|row| row.selected && row.label.eq_ignore_ascii_case("Cutoff")),
        _ => false,
    });
    assert!(
        cutoff_selected,
        "Instrument deck should focus Cutoff row for bold alignment check"
    );

    // Verify Instrument hints advertise `H/L` tab navigation without redundant `-/=` bindings.
    let dl = build_display_list(&fv, 1440.0, 860.0, 1.0, &theme);
    assert!(
        dl.base.texts.iter().any(|t| t.text.contains("H/L")),
        "Instrument deck hint text should include clean 'H/L'"
    );
    assert!(
        !dl.base.texts.iter().any(|t| t.text.contains("-/=")),
        "Instrument deck hint text must not include redundant '-/=' binding"
    );

    assert_eq!(
        gpu.first_missing_glyph(&dl, &theme),
        None,
        "Instrument deck emitted a glyph missing from embedded JetBrains Mono"
    );

    let px_inst = gpu.render_rgba8(&fv, &theme).unwrap();
    assert!(distinct_pixel_ratio(&px_inst, theme.surface.canvas) > 0.20);
    maybe_save_png("01-instrument-bold-no-ligature.png", 1440, 860, &px_inst);

    // Focused Regular vs Bold alignment + ligature suppression + Unicode coverage grid.
    let Ok(mut card_gpu) = HeadlessGpu::new(960, 420) else {
        return;
    };
    let mut card = DisplayList::new(960.0, 420.0, 1.0);
    card.fill(Rect::new(0.0, 0.0, 960.0, 420.0), theme.surface.canvas);
    card.panel(
        Rect::new(20.0, 20.0, 920.0, 380.0),
        theme.surface.elevated,
        theme.accent.focus,
        6.0,
        1.5,
        0.0,
    );
    let rows = [
        (
            "REGULAR  | Cutoff      3.60 kHz  | h/l ,/. -/= fine  | 0123456789",
            false,
            theme.text.primary,
        ),
        (
            "BOLD     | Cutoff      3.60 kHz  | h/l ,/. -/= fine  | 0123456789",
            true,
            theme.accent.focus,
        ),
        (
            "REGULAR  | Resonance   18%       | != -> => <= >= </>| ABCDEFGHIJ",
            false,
            theme.text.primary,
        ),
        (
            "BOLD     | Resonance   18%       | != -> => <= >= </>| ABCDEFGHIJ",
            true,
            theme.accent.warning_key,
        ),
        (
            "REGULAR  | Symbols: · → ↪ › ▸ ● ▏ ⌥ ✓ – — … ← ↓ − ≤ ≥ ♭ ♯ ° ± Δ",
            false,
            theme.text.secondary,
        ),
        (
            "BOLD     | Symbols: · → ↪ › ▸ ● ▏ ⌥ ✓ – — … ← ↓ − ≤ ≥ ♭ ♯ ° ± Δ",
            true,
            theme.text.primary,
        ),
        (
            "REGULAR  | Native:  ∥ PAUSED · ↪ arr. · ↩ loop · ↔ swap · ∾ mod · ✶",
            false,
            theme.text.secondary,
        ),
        (
            "BOLD     | Native:  ∥ PAUSED · ↪ arr. · ↩ loop · ↔ swap · ∾ mod · ✶",
            true,
            theme.accent.modulation,
        ),
    ];
    for (i, (text, bold, color)) in rows.iter().enumerate() {
        let y = 44.0 + i as f32 * 40.0;
        if i % 2 == 1 {
            card.rounded(
                Rect::new(36.0, y - 2.0, 888.0, 32.0),
                theme.surface.selection,
                4.0,
            );
        }
        card.text(
            Rect::new(48.0, y, 864.0, 28.0),
            *text,
            13.0,
            *bold,
            *color,
            TextAlign::Left,
        );
    }
    assert_eq!(
        card_gpu.first_missing_glyph(&card, &theme),
        None,
        "all UI symbols must shape directly in embedded JetBrains Mono with zero .notdef glyphs"
    );
    let px_grid = card_gpu.render_display_list_rgba8(&card, &theme).unwrap();
    assert!(distinct_pixel_ratio(&px_grid, theme.surface.canvas) > 0.25);
    maybe_save_png("02-font-alignment-grid.png", 960, 420, &px_grid);
}

#[test]
fn gpu_help_overlay_multiline_wrap_across_tabs_resolutions_and_themes() {
    let Ok(mut gpu_1440) = HeadlessGpu::new(1440, 860) else {
        return;
    };
    let Ok(mut gpu_1100) = HeadlessGpu::new(1100, 720) else {
        return;
    };
    let dark = Theme::cadence_dark();
    let light = Theme::studio_light();
    let mut app = App::new(Project::demo(), PathBuf::from("help_gpu.json")).unwrap();
    let snap = common::build_snapshot(app.project(), 24);
    let status = "Metal · 48 kHz";

    // Open `?` Help overlay (starts on `All` tab).
    app.press_keys("?").unwrap();

    // Verify zero horizontal ellipsis truncation across EVERY Help tab at both 1440×860 & 1100×720.
    for tab_idx in 0..HelpTab::ALL.len() {
        let fv = app.frame_view(&snap, status);
        assert!(
            matches!(fv.overlay, Some(OverlayView::Help(_))),
            "expected Help overlay on tab {tab_idx}"
        );
        for (w, h) in [(1440.0_f32, 860.0_f32), (1100.0_f32, 720.0_f32)] {
            let dl = build_display_list(&fv, w, h, 1.0, &dark);
            assert_eq!(
                gpu_1440.first_missing_glyph(&dl, &dark),
                None,
                "Help tab {tab_idx} emitted a glyph missing from embedded JetBrains Mono"
            );
            for run in &dl.overlay.texts {
                assert!(
                    !run.text.contains('…'),
                    "Help tab {tab_idx} truncated with ellipsis at {w}x{h}: {:?}",
                    run.text
                );
            }
        }
        app.press_keys("l").unwrap();
    }

    // Back on `All` tab: scroll deep to verify dynamic variable-height scroll window.
    let fv_all_top = app.frame_view(&snap, status);
    let px_all_top = gpu_1440.render_rgba8(&fv_all_top, &dark).unwrap();
    app.press_keys("jjjjjjjjjjjjjjjjjjjjjjjjjjjjjj").unwrap();
    let fv_all_scrolled = app.frame_view(&snap, status);
    let px_all_scrolled = gpu_1440.render_rgba8(&fv_all_scrolled, &dark).unwrap();
    assert!(distinct_pixel_ratio(&px_all_scrolled, dark.surface.canvas) > 0.25);
    assert_ne!(
        px_all_top, px_all_scrolled,
        "scrolling multi-line Help overlay must change rendered frame"
    );
    maybe_save_png(
        "05-help-all-scrolled-1440-dark.png",
        1440,
        860,
        &px_all_scrolled,
    );

    // Tab 1: `Navigate` (long `DECKS` & `GRAPH` wrapped rows) in Dark + Studio Light.
    app.press_keys("l").unwrap();
    let fv_nav = app.frame_view(&snap, status);
    let px_nav_dark = gpu_1440.render_rgba8(&fv_nav, &dark).unwrap();
    let px_nav_light = gpu_1440.render_rgba8(&fv_nav, &light).unwrap();
    assert!(distinct_pixel_ratio(&px_nav_dark, dark.surface.canvas) > 0.25);
    assert!(distinct_pixel_ratio(&px_nav_light, light.surface.canvas) > 0.25);
    assert_ne!(px_nav_dark, px_nav_light);
    maybe_save_png("03-help-navigate-1440-dark.png", 1440, 860, &px_nav_dark);
    maybe_save_png("07-help-navigate-1440-light.png", 1440, 860, &px_nav_light);

    // Tab 4: `Sound & FX` (`RACK PANEL`, `PATCHBAY`, `RACK ZONES`, `STUDIO FX`, `BOUNCE`)
    // at BOTH `1440×860` and `1100×720`, in Dark & Studio Light.
    app.press_keys("lll").unwrap();
    let fv_sfx = app.frame_view(&snap, status);
    let px_sfx_1440_dark = gpu_1440.render_rgba8(&fv_sfx, &dark).unwrap();
    let px_sfx_1100_dark = gpu_1100.render_rgba8(&fv_sfx, &dark).unwrap();
    let px_sfx_1100_light = gpu_1100.render_rgba8(&fv_sfx, &light).unwrap();
    assert!(distinct_pixel_ratio(&px_sfx_1440_dark, dark.surface.canvas) > 0.25);
    assert!(distinct_pixel_ratio(&px_sfx_1100_dark, dark.surface.canvas) > 0.25);
    assert!(distinct_pixel_ratio(&px_sfx_1100_light, light.surface.canvas) > 0.25);
    assert_ne!(px_nav_dark, px_sfx_1440_dark);
    maybe_save_png(
        "04-help-soundfx-1440-dark.png",
        1440,
        860,
        &px_sfx_1440_dark,
    );
    maybe_save_png(
        "06-help-soundfx-1100-dark.png",
        1100,
        720,
        &px_sfx_1100_dark,
    );
    maybe_save_png(
        "08-help-soundfx-1100-light.png",
        1100,
        720,
        &px_sfx_1100_light,
    );
}
