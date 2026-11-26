use crate::common::{maybe_save_png, with_gpu};
use cadence_gui::display::{DisplayList, Rect, TextAlign};
use cadence_gui::headless::distinct_pixel_ratio;
use cadence_gui::theme::{Color, Theme};

#[test]
fn gpu_z_order_occludes_cables_behind_cards_and_draws_handles_over_curves() {
    with_gpu(256, 256, |gpu| {
        let theme = Theme::cadence_dark();
        let pixel_at = |rgba: &[u8], x: u32, y: u32| -> [u8; 3] {
            let off = ((y * 256 + x) * 4) as usize;
            [rgba[off], rgba[off + 1], rgba[off + 2]]
        };
        let close_rgb = |actual: [u8; 3], expected: Color, tol: i16| -> bool {
            (i16::from(actual[0]) - i16::from(expected.r)).abs() <= tol
                && (i16::from(actual[1]) - i16::from(expected.g)).abs() <= tol
                && (i16::from(actual[2]) - i16::from(expected.b)).abs() <= tol
        };

        // 1. Painter's order:
        //    Top half: bright emissive red cable BEHIND opaque elevated card.
        //    Bottom half: bright green handle ON TOP OF bright emissive red curve.
        let mut dl = DisplayList::new(256.0, 256.0, 1.0);
        dl.polyline(
            &[(16.0, 80.0), (240.0, 80.0)],
            8.0,
            theme.accent.alert,
            0.60,
            None,
        );
        dl.panel(
            Rect::new(80.0, 48.0, 96.0, 64.0),
            theme.surface.elevated,
            theme.accent.focus,
            6.0,
            1.5,
            0.0,
        );
        dl.polyline(
            &[(16.0, 180.0), (240.0, 180.0)],
            8.0,
            theme.accent.alert,
            0.50,
            None,
        );
        dl.rounded_glow(
            Rect::new(120.0, 172.0, 16.0, 16.0),
            theme.accent.playhead,
            8.0,
            0.40,
        );

        let rgba = gpu.render_display_list_rgba8(&dl, &theme).unwrap();

        let exposed = pixel_at(&rgba, 40, 80);
        assert!(
            exposed[0] > exposed[1].saturating_add(60) && exposed[0] > 160,
            "expected exposed cable pixel to be alert red, got {exposed:?}"
        );
        let occluded = pixel_at(&rgba, 128, 80);
        assert!(
            close_rgb(occluded, theme.surface.elevated, 10),
            "opaque card must occlude cable behind it: expected {:?}, got {occluded:?}",
            [
                theme.surface.elevated.r,
                theme.surface.elevated.g,
                theme.surface.elevated.b
            ]
        );
        let handle = pixel_at(&rgba, 128, 180);
        assert!(
            handle[1] > handle[0].saturating_add(40) && handle[1] > 150,
            "expected handle pixel on top of curve to be playhead green, got {handle:?}"
        );

        // 2. Zero-bloom fast path (`emissive_count == 0`): skips blur passes cleanly.
        dl.clear();
        dl.polyline(
            &[(24.0, 200.0), (128.0, 48.0), (232.0, 200.0)],
            4.0,
            theme.accent.focus,
            0.0,
            None,
        );
        assert_eq!(dl.base.emissive_count, 0);
        let rgba_no_bloom = gpu.render_display_list_rgba8(&dl, &theme).unwrap();
        assert!(distinct_pixel_ratio(&rgba_no_bloom, theme.surface.canvas) > 0.01);
        let apex = pixel_at(&rgba_no_bloom, 128, 48);
        assert!(
            !close_rgb(apex, theme.surface.canvas, 10),
            "polyline vertex join apex must be filled, got canvas {apex:?}"
        );
    });
}

#[test]
fn gpu_embedded_font_regular_bold_alignment_and_unicode_glyph_coverage() {
    with_gpu(960, 420, |gpu| {
        let theme = Theme::cadence_dark();
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
            gpu.first_missing_glyph(&card, &theme),
            None,
            "all UI symbols must shape directly in embedded JetBrains Mono with zero .notdef glyphs"
        );
        let px_grid = gpu.render_display_list_rgba8(&card, &theme).unwrap();
        assert!(distinct_pixel_ratio(&px_grid, theme.surface.canvas) > 0.25);
        maybe_save_png("shaders-font-alignment-grid.png", 960, 420, &px_grid);
    });
}
