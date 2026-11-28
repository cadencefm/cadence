//! Multi-mode cubic Drive (`AtomicJet` / `ShockPort`) transfer curve and warmth tone filter visualizer.
use super::{
    draw_card_panel, draw_lp_response_card, draw_mode_badge_header, split_side_card,
    LpResponseSpec, ModeBadgeHeader,
};
use crate::display::{DisplayList, Rect, TextAlign};
use crate::theme::Theme;
use cadence_app::view::EffectVisualView;
use cadence_core::mixer::DriveMode;
use cadence_core::params::format_hz;

pub(super) fn draw_drive_visual(
    dl: &mut DisplayList,
    area: Rect,
    visual: &EffectVisualView,
    bypassed: bool,
    theme: &Theme,
) {
    let EffectVisualView::Drive {
        mode,
        freq_high_hz,
        curve,
        tone_curve_db,
        tone_norm,
        drive,
        asymmetry: _,
        tone_hz,
        output_db,
        mix,
        summary,
    } = visual
    else {
        return;
    };
    let (mode, freq_high_hz, tone_norm, drive, tone_hz, output_db, mix) = (
        *mode,
        *freq_high_hz,
        *tone_norm,
        *drive,
        *tone_hz,
        *output_db,
        *mix,
    );
    let tone_w = (area.w * 0.42).clamp(180.0, 360.0);
    let (left_card, right_card) = split_side_card(area, tone_w);

    // Left: Waveshaper transfer curve (x in [-1, 1] -> saturated output).
    let inner = draw_card_panel(dl, left_card, theme);
    let (hdr, body) = inner.split_top(18.0);
    let mode_badge = match mode {
        DriveMode::AtomicJet => "ATOMIC JET",
        DriveMode::ShockPort => "SHOCK PORT",
    };
    let shaper_col = if bypassed {
        theme.text.muted
    } else {
        match mode {
            DriveMode::AtomicJet => theme
                .accent
                .warning_key
                .mix(theme.accent.edit, drive * 0.40),
            DriveMode::ShockPort => theme.accent.alert,
        }
    };
    draw_mode_badge_header(
        dl,
        hdr,
        &ModeBadgeHeader {
            badge: mode_badge,
            badge_col: shaper_col,
            title: "CUBIC DRIVE  ·  TRANSFER",
            summary,
            title_col: shaper_col,
            bypassed,
        },
        theme,
    );
    if curve.len() >= 2 && body.h >= 40.0 {
        let (upper, x_axis_full) = body.split_bottom(14.0);
        let (y_axis, plot) = upper.split_left(28.0);
        let x_axis = Rect::new(plot.x, x_axis_full.y, plot.w, x_axis_full.h);
        if !plot.is_empty() {
            let px_of = |x: f32| {
                let nx = ((x + 1.0) * 0.5).clamp(0.0, 1.0);
                plot.x + 4.0 + nx * (plot.w - 8.0).max(1.0)
            };
            let py_of = |y: f32| {
                let ny = ((y + 1.25) / 2.5).clamp(0.0, 1.0);
                plot.bottom() - 4.0 - ny * (plot.h - 8.0).max(1.0)
            };
            let font_sz = theme.font.small - 2.0;
            for (val, label) in [(1.0_f32, "+1"), (0.0, "0"), (-1.0, "-1")] {
                let y = py_of(val);
                let is_zero = val == 0.0;
                dl.fill(
                    Rect::new(plot.x, y, plot.w, if is_zero { 1.2 } else { 1.0 }),
                    if is_zero {
                        theme.border.grid_bar
                    } else {
                        theme.border.grid_step
                    },
                );
                dl.text(
                    Rect::new(y_axis.x, y - 6.0, y_axis.w - 4.0, 12.0),
                    label,
                    font_sz,
                    is_zero,
                    theme.text.muted,
                    TextAlign::Right,
                );
            }
            dl.fill(
                Rect::new(px_of(0.0), plot.y, 1.0, plot.h),
                theme.border.grid_bar,
            );
            for (val, label, align) in [
                (-1.0_f32, "-1.0", TextAlign::Left),
                (0.0, "0.0", TextAlign::Center),
                (1.0, "+1.0", TextAlign::Right),
            ] {
                let rx = match align {
                    TextAlign::Left => Rect::new(plot.x, x_axis.y, 34.0, x_axis.h),
                    TextAlign::Right => Rect::new(plot.right() - 34.0, x_axis.y, 34.0, x_axis.h),
                    TextAlign::Center => Rect::new(px_of(val) - 18.0, x_axis.y, 36.0, x_axis.h),
                };
                dl.text(rx, label, font_sz, false, theme.text.muted, align);
            }

            dl.push_clip(plot);
            // Unity linear reference diagonal y = x.
            dl.polyline(
                &[(px_of(-1.0), py_of(-1.0)), (px_of(1.0), py_of(1.0))],
                1.1,
                theme.border.grid_bar,
                0.0,
                None,
            );
            let pts: Vec<(f32, f32)> = curve.iter().map(|&(x, y)| (px_of(x), py_of(y))).collect();
            dl.polyline(
                &pts,
                2.1,
                shaper_col,
                if bypassed {
                    0.0
                } else {
                    theme.glow.scope * 0.85
                },
                Some((py_of(0.0), if bypassed { 0.04 } else { 0.16 })),
            );
            dl.pop_clip();
        }
    }

    // Right: Warmth Tone Cutoff Response Card.
    if let Some(rcard) = right_card {
        let tone_col = if bypassed {
            theme.text.muted
        } else {
            theme.accent.routing
        };
        let marker_line_col =
            theme
                .accent
                .warning_key
                .with_alpha(if bypassed { 0.25 } else { 0.65 });
        let marker_dot_col = if bypassed {
            theme.text.muted
        } else {
            theme.accent.warning_key
        };
        let tone_summary = format!(
            "{} · {:+.1}dB · {:.0}%",
            format_hz(tone_hz),
            output_db,
            mix * 100.0
        );
        draw_lp_response_card(
            dl,
            rcard,
            &LpResponseSpec {
                title: "TONE FILTER",
                summary: &tone_summary,
                freq_high_hz,
                curve_db: tone_curve_db,
                cutoff_norm: tone_norm,
                curve_col: tone_col,
                marker_line_col,
                marker_dot_col,
                bypassed,
            },
            theme,
        );
    }
}
