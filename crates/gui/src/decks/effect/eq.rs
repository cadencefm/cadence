//! Parametric Studio EQ response visualizer for the GPU Effect deck.
use super::{
    build_curve_pts, draw_card_header, draw_card_panel, draw_freq_ticks_and_grid, sample_curve_y,
};
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::Theme;
use cadence_app::view::EffectVisualView;
use cadence_core::params::format_hz;

pub(super) fn draw_eq_visual(
    dl: &mut DisplayList,
    area: Rect,
    visual: &EffectVisualView,
    bypassed: bool,
    theme: &Theme,
) {
    let EffectVisualView::Eq {
        freq_high_hz,
        curve_db,
        live_curve_db,
        hpf_norm,
        hpf_active,
        mid_db,
        mid_hz,
        mid_q,
        mid_hz_norm,
        mid_q_span,
        mod_mid_hz_norm,
        band_norms,
        summary,
        ..
    } = visual
    else {
        return;
    };
    let (
        freq_high_hz,
        live_curve_db,
        hpf_norm,
        hpf_active,
        mid_db,
        mid_hz,
        mid_q,
        mid_hz_norm,
        mid_q_span,
        mod_mid_hz_norm,
        band_norms,
    ) = (
        *freq_high_hz,
        live_curve_db.as_deref(),
        *hpf_norm,
        *hpf_active,
        *mid_db,
        *mid_hz,
        *mid_q,
        *mid_hz_norm,
        *mid_q_span,
        *mod_mid_hz_norm,
        *band_norms,
    );
    let inner = draw_card_panel(dl, area, theme);
    let (hdr, body) = inner.split_top(18.0);
    let curve_col = if bypassed {
        theme.text.muted
    } else {
        theme.accent.focus
    };
    draw_card_header(dl, hdr, "STUDIO EQ  ·  RESPONSE", summary, curve_col, theme);
    if curve_db.len() < 2 || body.h < 40.0 {
        return;
    }

    let (upper, x_axis_full) = body.split_bottom(14.0);
    let (y_axis, plot) = upper.split_left(32.0);
    let x_axis = Rect::new(plot.x, x_axis_full.y, plot.w, x_axis_full.h);
    if plot.is_empty() {
        return;
    }

    let bounds = [-18.0_f32, 18.0_f32];
    let y_of_db = |db: f32| -> f32 {
        let yn = ((db - bounds[0]) / (bounds[1] - bounds[0])).clamp(0.0, 1.0);
        plot.bottom() - 3.0 - yn * (plot.h - 6.0).max(1.0)
    };

    // dB gridlines + left labels.
    let font_sz = theme.font.small - 2.0;
    for (db, label, show_label) in [
        (18.0_f32, "+18", true),
        (12.0, "+12", true),
        (6.0, "+6", false),
        (0.0, "0dB", true),
        (-6.0, "-6", false),
        (-12.0, "-12", true),
        (-18.0, "-18", true),
    ] {
        let y = y_of_db(db);
        let is_zero = db == 0.0;
        dl.fill(
            Rect::new(plot.x, y, plot.w, if is_zero { 1.2 } else { 1.0 }),
            if is_zero {
                theme.border.grid_bar
            } else {
                theme.border.grid_step
            },
        );
        if show_label {
            dl.text(
                Rect::new(y_axis.x, y - 6.0, y_axis.w - 4.0, 12.0),
                label,
                font_sz,
                is_zero,
                if is_zero {
                    theme.text.secondary
                } else {
                    theme.text.muted
                },
                TextAlign::Right,
            );
        }
    }

    draw_freq_ticks_and_grid(dl, plot, x_axis, freq_high_hz, theme);

    dl.push_clip(plot);
    let zero_y = y_of_db(0.0);

    if hpf_active {
        let hx = plot.x + plot.w * hpf_norm.clamp(0.0, 1.0);
        dl.fill(
            Rect::new(plot.x, plot.y, (hx - plot.x).max(0.0), plot.h),
            theme
                .accent
                .warning_key
                .with_alpha(if bypassed { 0.03 } else { 0.08 }),
        );
        dl.fill(
            Rect::new(hx, plot.y, 1.0, plot.h),
            theme
                .accent
                .warning_key
                .with_alpha(if bypassed { 0.25 } else { 0.65 }),
        );
    }

    // Parametric Mid-band Q bandwidth ribbon + edge markers.
    let qx0 = plot.x + plot.w * mid_q_span.0.clamp(0.0, 1.0);
    let qx1 = plot.x + plot.w * mid_q_span.1.clamp(0.0, 1.0);
    let mid_x = plot.x + plot.w * mid_hz_norm.clamp(0.0, 1.0);
    let mid_y = sample_curve_y(curve_db, mid_hz_norm, bounds, plot);
    let mid_col = if bypassed {
        theme.text.muted
    } else {
        theme.accent.focus
    };
    if qx1 > qx0 {
        let band_alpha = if bypassed {
            0.03
        } else if mid_db.abs() > 0.1 {
            0.12
        } else {
            0.07
        };
        dl.fill(
            Rect::new(qx0, plot.y, qx1 - qx0, plot.h),
            mid_col.with_alpha(band_alpha),
        );
        dl.fill(
            Rect::new(qx0, plot.y, 1.0, plot.h),
            mid_col.with_alpha(if bypassed { 0.14 } else { 0.28 }),
        );
        dl.fill(
            Rect::new(qx1, plot.y, 1.0, plot.h),
            mid_col.with_alpha(if bypassed { 0.14 } else { 0.28 }),
        );
    }
    dl.fill(
        Rect::new(mid_x - 0.5, plot.y, 1.0, plot.h),
        mid_col.with_alpha(if bypassed { 0.22 } else { 0.50 }),
    );

    let pts = build_curve_pts(curve_db, bounds, plot);
    dl.polyline(
        &pts,
        2.0,
        curve_col,
        if bypassed {
            0.0
        } else {
            theme.glow.scope * 0.8
        },
        Some((zero_y, if bypassed { 0.05 } else { 0.18 })),
    );

    if let Some(live) = live_curve_db {
        if live.len() >= 2 && !bypassed {
            let lpts = build_curve_pts(live, bounds, plot);
            dl.polyline(&lpts, 1.8, theme.accent.modulation, 0.45, None);
            if let Some(mod_nx) = mod_mid_hz_norm {
                let mx = plot.x + plot.w * mod_nx.clamp(0.0, 1.0);
                let my = sample_curve_y(live, mod_nx, bounds, plot);
                dl.rounded_glow(
                    Rect::new(mx - 4.0, my - 4.0, 8.0, 8.0),
                    theme.accent.modulation,
                    4.0,
                    0.50,
                );
            }
        }
    }

    // Band anchor dots (HPF + Low/High shelves; Mid gets the dedicated interactive handle below).
    if hpf_active {
        let hx = plot.x + plot.w * hpf_norm.clamp(0.0, 1.0);
        let hy = sample_curve_y(curve_db, hpf_norm, bounds, plot);
        dl.rounded_glow(
            Rect::new(hx - 3.0, hy - 3.0, 6.0, 6.0),
            if bypassed {
                theme.text.muted
            } else {
                theme.accent.warning_key
            },
            3.0,
            if bypassed { 0.0 } else { 0.4 },
        );
    }
    for (idx, col_base) in [
        (0usize, theme.accent.routing),
        (2usize, theme.accent.playhead),
    ] {
        let bn = band_norms[idx];
        let bx = plot.x + plot.w * bn.clamp(0.0, 1.0);
        let by = sample_curve_y(curve_db, bn, bounds, plot);
        let col = if bypassed { theme.text.muted } else { col_base };
        dl.rounded_glow(
            Rect::new(bx - 3.0, by - 3.0, 6.0, 6.0),
            col,
            3.0,
            if bypassed { 0.0 } else { 0.35 },
        );
    }

    // Dedicated Mid-band interactive handle + Q bandwidth bracket + crisp badge.
    if qx1 - qx0 >= 8.0 {
        let bw_y = mid_y.clamp(plot.y + 8.0, plot.bottom() - 8.0);
        dl.fill(
            Rect::new(qx0, bw_y - 0.75, qx1 - qx0, 1.5),
            mid_col.with_alpha(if bypassed { 0.30 } else { 0.65 }),
        );
        dl.fill(
            Rect::new(qx0, bw_y - 4.0, 1.5, 8.0),
            mid_col.with_alpha(if bypassed { 0.35 } else { 0.80 }),
        );
        dl.fill(
            Rect::new(qx1 - 1.5, bw_y - 4.0, 1.5, 8.0),
            mid_col.with_alpha(if bypassed { 0.35 } else { 0.80 }),
        );
    }
    dl.panel(
        Rect::new(mid_x - 6.0, mid_y - 6.0, 12.0, 12.0),
        theme.surface.elevated.with_alpha(0.90),
        mid_col,
        6.0,
        1.5,
        if bypassed { 0.0 } else { 0.42 },
    );
    dl.rounded_glow(
        Rect::new(mid_x - 3.0, mid_y - 3.0, 6.0, 6.0),
        mid_col,
        3.0,
        if bypassed { 0.0 } else { 0.55 },
    );

    if plot.w >= 140.0 && plot.h >= 44.0 {
        let tag = if mid_db.abs() >= 0.05 {
            format!("M {} · {:+.1}dB · Q{:.1}", format_hz(mid_hz), mid_db, mid_q)
        } else {
            format!("M {} · Q{:.1}", format_hz(mid_hz), mid_q)
        };
        let tag_font = theme.font.small - 2.0;
        let tag_w = (tag.chars().count() as f32 * theme.char_width(tag_font) + 10.0)
            .min((plot.w - 8.0).max(40.0));
        let tag_h = 15.0;
        let tag_x = (mid_x - tag_w * 0.5)
            .clamp(plot.x + 4.0, (plot.right() - tag_w - 4.0).max(plot.x + 4.0));
        let tag_y = if mid_y - plot.y >= tag_h + 10.0 {
            mid_y - tag_h - 8.0
        } else {
            (mid_y + 9.0).min(plot.bottom() - tag_h - 3.0)
        };
        let tag_rect = Rect::new(tag_x, tag_y, tag_w, tag_h);
        dl.panel(
            tag_rect,
            theme.surface.elevated.with_alpha(0.92),
            mid_col.with_alpha(0.75),
            theme.radii.badge,
            1.0,
            if bypassed { 0.0 } else { 0.18 },
        );
        let max_chars = ((tag_w - 6.0) / theme.char_width(tag_font)).max(1.0) as usize;
        dl.text(
            tag_rect,
            trim_label(&tag, max_chars),
            tag_font,
            true,
            mid_col,
            TextAlign::Center,
        );
    }
    dl.pop_clip();
}
