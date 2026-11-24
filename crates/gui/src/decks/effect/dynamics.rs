//! Compressor & Brickwall Limiter transfer-curve + Gain Reduction visualizers.
use super::{
    draw_card_header, draw_card_panel, draw_gr_meter_panel, draw_meter_bar_row, split_side_card,
    GrMeterSpec, TransferPlot,
};
use crate::display::{DisplayList, Rect, TextAlign};
use crate::theme::Theme;
use cadence_app::view::effect::{COMP_TRANSFER_MAX_DB, COMP_TRANSFER_MIN_DB};
use cadence_app::view::EffectVisualView;
use cadence_core::mixer::EffectParam;

pub(super) fn draw_compressor_visual(
    dl: &mut DisplayList,
    area: Rect,
    visual: &EffectVisualView,
    bypassed: bool,
    theme: &Theme,
) {
    let EffectVisualView::Compressor {
        transfer_curve,
        compression_curve,
        threshold_norm,
        knee_span,
        gain_reduction_db,
        gr_hold_db,
        ratio,
        attack_ms,
        release_ms,
        makeup_db,
        mix,
        summary,
        ..
    } = visual
    else {
        return;
    };
    let (
        threshold_norm,
        knee_span,
        gain_reduction_db,
        gr_hold_db,
        ratio,
        attack_ms,
        release_ms,
        makeup_db,
        mix,
    ) = (
        *threshold_norm,
        *knee_span,
        *gain_reduction_db,
        *gr_hold_db,
        *ratio,
        *attack_ms,
        *release_ms,
        *makeup_db,
        *mix,
    );
    let gr_w = (area.w * 0.36).clamp(196.0, 316.0);
    let (left_card, right_card) = split_side_card(area, gr_w);

    // Left: Input-vs-Output dB Transfer Curve (-48..+6 dB float headroom).
    let inner = draw_card_panel(dl, left_card, theme);
    let (hdr, body) = inner.split_top(18.0);
    let curve_col = if bypassed {
        theme.text.muted
    } else {
        theme.accent.focus
    };
    draw_card_header(
        dl,
        hdr,
        "TRANSFER  ·  IN vs OUT (dB)",
        summary,
        curve_col,
        theme,
    );

    if transfer_curve.len() >= 2 && body.h >= 40.0 {
        let (upper, x_axis_full) = body.split_bottom(14.0);
        let (y_axis, plot) = upper.split_left(30.0);
        let x_axis = Rect::new(plot.x, x_axis_full.y, plot.w, x_axis_full.h);
        if !plot.is_empty() {
            let tp = TransferPlot { plot };
            let db_span = (COMP_TRANSFER_MAX_DB - COMP_TRANSFER_MIN_DB).max(1.0);
            let db_norm = |db: f32| ((db - COMP_TRANSFER_MIN_DB) / db_span).clamp(0.0, 1.0);
            let font_sz = theme.font.small - 2.0;
            let show_plus6_y = plot.h >= 96.0;
            let dense_y = plot.h >= 66.0;
            let show_zero_x = plot.w >= 180.0;
            let dense_x = plot.w >= 260.0;

            for (db, label) in [
                (6.0_f32, "+6"),
                (0.0, "0"),
                (-12.0, "-12"),
                (-24.0, "-24"),
                (-36.0, "-36"),
                (-48.0, "-48"),
            ] {
                let n = db_norm(db);
                let y = tp.py_of(n);
                let x = tp.px_of(n);
                let is_zero = db == 0.0;
                let line_col = if is_zero {
                    theme.border.grid_bar
                } else {
                    theme.border.grid_step
                };
                dl.fill(Rect::new(plot.x, y, plot.w, 1.0), line_col);
                dl.fill(Rect::new(x, plot.y, 1.0, plot.h), line_col);
                let show_y = is_zero
                    || db == -24.0
                    || db == -48.0
                    || (db == 6.0 && show_plus6_y)
                    || ((db == -12.0 || db == -36.0) && dense_y);
                if show_y {
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
                let show_x = db == -48.0
                    || db == -24.0
                    || db == 6.0
                    || (is_zero && show_zero_x)
                    || ((db == -12.0 || db == -36.0) && dense_x);
                if show_x {
                    let (rx, align) = if db == -48.0 {
                        (Rect::new(plot.x, x_axis.y, 32.0, x_axis.h), TextAlign::Left)
                    } else if db == 6.0 {
                        (
                            Rect::new(plot.right() - 28.0, x_axis.y, 28.0, x_axis.h),
                            TextAlign::Right,
                        )
                    } else {
                        (
                            Rect::new(x - 16.0, x_axis.y, 32.0, x_axis.h),
                            TextAlign::Center,
                        )
                    };
                    dl.text(
                        rx,
                        label,
                        font_sz,
                        is_zero,
                        if is_zero {
                            theme.text.secondary
                        } else {
                            theme.text.muted
                        },
                        align,
                    );
                }
            }

            dl.push_clip(plot);
            // Subtle 32-bit float headroom zone above 0 dBFS (> 0 dB input or output lift).
            let zero_n = db_norm(0.0);
            let zero_x = tp.px_of(zero_n);
            let zero_y = tp.py_of(zero_n);
            let headroom_col =
                theme
                    .accent
                    .warning_key
                    .with_alpha(if bypassed { 0.02 } else { 0.045 });
            if zero_y > plot.y {
                dl.fill(
                    Rect::new(plot.x, plot.y, plot.w, (zero_y - plot.y).max(0.0)),
                    headroom_col,
                );
            }
            if zero_x < plot.right() {
                dl.fill(
                    Rect::new(
                        zero_x,
                        zero_y,
                        (plot.right() - zero_x).max(0.0),
                        (plot.bottom() - zero_y).max(0.0),
                    ),
                    headroom_col,
                );
            }
            // Soft-knee vertical band around threshold (±3 dB).
            let kx0 = tp.px_of(knee_span.0);
            let kx1 = tp.px_of(knee_span.1);
            if kx1 > kx0 {
                dl.fill(
                    Rect::new(kx0, plot.y, kx1 - kx0, plot.h),
                    theme
                        .accent
                        .warning_key
                        .with_alpha(if bypassed { 0.04 } else { 0.11 }),
                );
            }

            // 1:1 unity reference diagonal.
            tp.draw_unity_diagonal(dl, theme);

            // Threshold crosshair.
            let tx = tp.px_of(threshold_norm);
            let ty_unity = tp.py_of(threshold_norm);
            dl.fill(
                Rect::new(tx, plot.y, 1.0, plot.h),
                theme
                    .accent
                    .warning_key
                    .with_alpha(if bypassed { 0.25 } else { 0.50 }),
            );
            dl.fill(
                Rect::new(plot.x, ty_unity, plot.w, 1.0),
                theme
                    .accent
                    .warning_key
                    .with_alpha(if bypassed { 0.18 } else { 0.35 }),
            );

            // Faint pre-makeup compression curve when makeup gain lifts the output.
            if makeup_db.abs() > 0.25 && compression_curve.len() >= 2 {
                let cpts: Vec<(f32, f32)> = compression_curve
                    .iter()
                    .map(|&(nx, ny)| (tp.px_of(nx), tp.py_of(ny)))
                    .collect();
                dl.polyline(&cpts, 1.3, theme.text.muted.with_alpha(0.70), 0.0, None);
            }

            // Active transfer curve with makeup + mix & glowing threshold knee dot.
            tp.draw_active_curve(dl, transfer_curve, 2.1, curve_col, bypassed, theme);
            tp.draw_knee_dot(dl, transfer_curve, threshold_norm, 0.48, bypassed, theme);
            dl.pop_clip();
        }
    }

    // Right: Live Gain Reduction (GR) Meter + Dynamics Telemetry.
    if let Some(rcard) = right_card {
        let info_col = draw_gr_meter_panel(
            dl,
            rcard,
            &GrMeterSpec {
                gain_reduction_db,
                gr_hold_db,
                alert_gr_db: 9.0,
                alert_hold_db: 12.0,
                bar_glow: 0.42,
                bypassed,
            },
            theme,
        );
        let info = info_col.inset(6.0, 1.0);
        if info.w >= 64.0 && info.h >= 56.0 {
            let gap = if info.h < 78.0 { 1.5 } else { 2.0 };
            let row_h = ((info.h - gap * 3.0) / 4.0).clamp(15.0, 26.0);
            let atk_rel = if attack_ms < 10.0 {
                format!("{attack_ms:.1}/{release_ms:.0}ms")
            } else {
                format!("{attack_ms:.0}/{release_ms:.0}ms")
            };
            draw_meter_bar_row(
                dl,
                Rect::new(info.x, info.y, info.w, row_h),
                "Ratio",
                &format!("{ratio:.1}:1"),
                EffectParam::Ratio.normalize(ratio),
                theme.accent.warning_key,
                theme,
            );
            draw_meter_bar_row(
                dl,
                Rect::new(info.x, info.y + (row_h + gap), info.w, row_h),
                "Atk / Rel",
                &atk_rel,
                EffectParam::Release.normalize(release_ms),
                theme.accent.routing,
                theme,
            );
            draw_meter_bar_row(
                dl,
                Rect::new(info.x, info.y + (row_h + gap) * 2.0, info.w, row_h),
                "Makeup",
                &format!("{makeup_db:+.1} dB"),
                EffectParam::Makeup.normalize(makeup_db),
                theme.accent.playhead,
                theme,
            );
            draw_meter_bar_row(
                dl,
                Rect::new(info.x, info.y + (row_h + gap) * 3.0, info.w, row_h),
                "Mix",
                &format!("{:.0}%", mix * 100.0),
                mix,
                theme.accent.focus,
                theme,
            );
        }
    }
}

pub(super) fn draw_limiter_visual(
    dl: &mut DisplayList,
    area: Rect,
    visual: &EffectVisualView,
    bypassed: bool,
    theme: &Theme,
) {
    let EffectVisualView::Limiter {
        ceiling_db,
        input_drive_db,
        release_ms,
        mix,
        transfer_curve,
        gain_reduction_db,
        gr_hold_db,
        modulated,
        ceiling_norm,
        summary,
    } = visual
    else {
        return;
    };
    let (
        ceiling_db,
        input_drive_db,
        release_ms,
        mix,
        gain_reduction_db,
        gr_hold_db,
        modulated,
        ceiling_norm,
    ) = (
        *ceiling_db,
        *input_drive_db,
        *release_ms,
        *mix,
        *gain_reduction_db,
        *gr_hold_db,
        *modulated,
        *ceiling_norm,
    );
    let gr_w = (area.w * 0.36).clamp(196.0, 316.0);
    let (left_card, right_card) = split_side_card(area, gr_w);

    // Left: Brickwall Limiter Transfer Curve + Ceiling Line (-48..0 dB).
    let inner = draw_card_panel(dl, left_card, theme);
    let (hdr, body) = inner.split_top(18.0);
    let curve_col = if bypassed {
        theme.text.muted
    } else if modulated {
        theme.accent.modulation
    } else {
        theme.accent.focus
    };
    let ceil_col = if bypassed {
        theme.text.muted
    } else {
        theme.accent.alert
    };
    draw_card_header(
        dl,
        hdr,
        "BRICKWALL  ·  TRANSFER & CEILING",
        summary,
        curve_col,
        theme,
    );

    if transfer_curve.len() >= 2 && body.h >= 40.0 {
        let (upper, x_axis_full) = body.split_bottom(14.0);
        let (y_axis, plot) = upper.split_left(30.0);
        let x_axis = Rect::new(plot.x, x_axis_full.y, plot.w, x_axis_full.h);
        if !plot.is_empty() {
            let tp = TransferPlot { plot };
            let font_sz = theme.font.small - 2.0;
            let dense_y = plot.h >= 66.0;

            for (db, label) in [
                (0.0_f32, "0"),
                (-12.0, "-12"),
                (-24.0, "-24"),
                (-36.0, "-36"),
                (-48.0, "-48"),
            ] {
                let n = ((db + 48.0) / 48.0).clamp(0.0, 1.0);
                let y = tp.py_of(n);
                let x = tp.px_of(n);
                dl.fill(Rect::new(plot.x, y, plot.w, 1.0), theme.border.grid_step);
                dl.fill(Rect::new(x, plot.y, 1.0, plot.h), theme.border.grid_step);
                if dense_y || db == 0.0 || db == -24.0 || db == -48.0 {
                    dl.text(
                        Rect::new(y_axis.x, y - 6.0, y_axis.w - 4.0, 12.0),
                        label,
                        font_sz,
                        false,
                        theme.text.muted,
                        TextAlign::Right,
                    );
                }
                let (rx, align) = if db == -48.0 {
                    (Rect::new(plot.x, x_axis.y, 32.0, x_axis.h), TextAlign::Left)
                } else if db == 0.0 {
                    (
                        Rect::new(plot.right() - 28.0, x_axis.y, 28.0, x_axis.h),
                        TextAlign::Right,
                    )
                } else {
                    (
                        Rect::new(x - 16.0, x_axis.y, 32.0, x_axis.h),
                        TextAlign::Center,
                    )
                };
                dl.text(rx, label, font_sz, false, theme.text.muted, align);
            }

            dl.push_clip(plot);
            // Brickwall ceiling forbidden zone above `ceiling_db`.
            let ceil_y = tp.py_of(ceiling_norm);
            if ceil_y > plot.y {
                dl.fill(
                    Rect::new(plot.x, plot.y, plot.w, (ceil_y - plot.y).max(0.0)),
                    ceil_col.with_alpha(if bypassed { 0.04 } else { 0.14 }),
                );
            }

            // 1:1 unity reference diagonal.
            tp.draw_unity_diagonal(dl, theme);

            // Effective threshold where driven input meets ceiling.
            let knee_in_db = (ceiling_db - input_drive_db).clamp(-48.0, 0.0);
            let knee_norm = ((knee_in_db + 48.0) / 48.0).clamp(0.0, 1.0);
            let knee_x = tp.px_of(knee_norm);
            dl.fill(
                Rect::new(knee_x, plot.y, 1.0, plot.h),
                theme
                    .accent
                    .warning_key
                    .with_alpha(if bypassed { 0.20 } else { 0.48 }),
            );

            // Glowing horizontal brickwall ceiling line.
            dl.polyline(
                &[(plot.x, ceil_y), (plot.right(), ceil_y)],
                1.7,
                ceil_col,
                if bypassed { 0.0 } else { 0.48 },
                None,
            );

            // Active brickwall transfer curve & knee dot where input begins hitting the ceiling.
            tp.draw_active_curve(dl, transfer_curve, 2.2, curve_col, bypassed, theme);
            tp.draw_knee_dot(dl, transfer_curve, knee_norm, 0.50, bypassed, theme);

            // Compact non-overlapping ceiling badge anchored neatly inside the plot.
            if plot.w >= 130.0 && plot.h >= 38.0 {
                let ctag = format!("CEIL {ceiling_db:.1} dB");
                let c_w = (ctag.chars().count() as f32 * theme.char_width(font_sz) + 10.0)
                    .min((plot.w - 8.0).max(40.0));
                let c_h = 15.0;
                let c_y = if ceil_y + 4.0 + c_h <= plot.bottom() - 3.0 {
                    ceil_y + 4.0
                } else {
                    (ceil_y - c_h - 4.0).max(plot.y + 2.0)
                };
                let c_rect = Rect::new(plot.x + 5.0, c_y, c_w, c_h);
                dl.panel(
                    c_rect,
                    theme.surface.elevated.with_alpha(0.92),
                    ceil_col.with_alpha(0.75),
                    theme.radii.badge,
                    1.0,
                    if bypassed { 0.0 } else { 0.15 },
                );
                dl.text(c_rect, &ctag, font_sz, true, ceil_col, TextAlign::Center);
            }
            dl.pop_clip();
        }
    }

    // Right: Live Gain Reduction (GR) Bar Meter + Limiter Telemetry.
    if let Some(rcard) = right_card {
        let info_col = draw_gr_meter_panel(
            dl,
            rcard,
            &GrMeterSpec {
                gain_reduction_db,
                gr_hold_db,
                alert_gr_db: 6.0,
                alert_hold_db: 9.0,
                bar_glow: 0.45,
                bypassed,
            },
            theme,
        );
        let info = info_col.inset(6.0, 1.0);
        if info.w >= 64.0 && info.h >= 34.0 {
            let rel_norm = EffectParam::Release.normalize(release_ms);
            if info.h >= 56.0 {
                let gap = if info.h < 78.0 { 1.5 } else { 2.0 };
                let row_h = ((info.h - gap * 3.0) / 4.0).clamp(15.0, 26.0);
                draw_meter_bar_row(
                    dl,
                    Rect::new(info.x, info.y, info.w, row_h),
                    "Ceiling",
                    &format!("{ceiling_db:.1} dB"),
                    EffectParam::Ceiling.normalize(ceiling_db),
                    theme.accent.alert,
                    theme,
                );
                draw_meter_bar_row(
                    dl,
                    Rect::new(info.x, info.y + (row_h + gap), info.w, row_h),
                    "In Drive",
                    &format!("+{input_drive_db:.1} dB"),
                    EffectParam::InputDrive.normalize(input_drive_db),
                    theme.accent.warning_key,
                    theme,
                );
                draw_meter_bar_row(
                    dl,
                    Rect::new(info.x, info.y + (row_h + gap) * 2.0, info.w, row_h),
                    "Release",
                    &format!("{release_ms:.0} ms"),
                    rel_norm,
                    theme.accent.routing,
                    theme,
                );
                draw_meter_bar_row(
                    dl,
                    Rect::new(info.x, info.y + (row_h + gap) * 3.0, info.w, row_h),
                    "Mix",
                    &format!("{:.0}%", mix * 100.0),
                    mix,
                    theme.accent.focus,
                    theme,
                );
            } else {
                // Compact split pane: show the two primary mastering readouts cleanly.
                let gap = 2.0;
                let row_h = ((info.h - gap) * 0.5).clamp(16.0, 24.0);
                draw_meter_bar_row(
                    dl,
                    Rect::new(info.x, info.y, info.w, row_h),
                    "Ceiling",
                    &format!("{ceiling_db:.1} dB"),
                    EffectParam::Ceiling.normalize(ceiling_db),
                    theme.accent.alert,
                    theme,
                );
                draw_meter_bar_row(
                    dl,
                    Rect::new(info.x, info.y + row_h + gap, info.w, row_h),
                    "In Drive",
                    &format!("+{input_drive_db:.1} dB"),
                    EffectParam::InputDrive.normalize(input_drive_db),
                    theme.accent.warning_key,
                    theme,
                );
            }
        }
    }
}
