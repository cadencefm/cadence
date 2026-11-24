//! Multi-mode Delay (`PingPong` / `Stereo`) tap timeline and Chorus quadrature LFO visualizers.
use super::{
    build_curve_pts, draw_card_header, draw_card_panel, draw_meter_bar_row, draw_mode_badge_header,
    split_side_card, ModeBadgeHeader,
};
use crate::display::{DisplayList, Rect, TextAlign};
use crate::theme::Theme;
use cadence_app::view::EffectVisualView;
use cadence_core::mixer::{DelayMode, EffectParam};

pub(super) fn draw_delay_visual(
    dl: &mut DisplayList,
    area: Rect,
    visual: &EffectVisualView,
    bypassed: bool,
    theme: &Theme,
) {
    let EffectVisualView::Delay {
        mode,
        steps,
        delay_ms,
        feedback,
        mix,
        grid_steps,
        taps,
        summary,
    } = visual
    else {
        return;
    };
    let (mode, steps, delay_ms, feedback, mix, grid_steps) =
        (*mode, *steps, *delay_ms, *feedback, *mix, *grid_steps);
    let side_w = (area.w * 0.28).clamp(156.0, 236.0);
    let (left_card, right_card) = split_side_card(area, side_w);

    // Left: Rhythmic Step-Grid Feedback Tap Bars.
    let inner = draw_card_panel(dl, left_card, theme);
    let (hdr, body) = inner.split_top(18.0);
    let tap_l_col = if bypassed {
        theme.text.muted
    } else {
        theme.accent.focus
    };
    let tap_r_col = if bypassed {
        theme.text.muted
    } else {
        theme.accent.routing
    };
    let (mode_badge, badge_col) = match mode {
        DelayMode::PingPong => ("PING-PONG", tap_l_col),
        DelayMode::Stereo => ("STEREO", tap_r_col),
    };
    draw_mode_badge_header(
        dl,
        hdr,
        &ModeBadgeHeader {
            badge: mode_badge,
            badge_col,
            title: "DELAY  ·  STEP TAPS",
            summary,
            title_col: tap_l_col,
            bypassed,
        },
        theme,
    );
    if body.h >= 40.0 {
        let (plot_outer, x_axis) = body.split_bottom(14.0);
        let plot = plot_outer.inset(6.0, 4.0);
        if !plot.is_empty() {
            let total_steps = grid_steps.max(4.0);
            let font_sz = theme.font.small - 2.0;

            // Beat / bar vertical grid lines + clean non-overlapping step labels.
            let label_stride = if total_steps <= 20.0 {
                4
            } else if total_steps <= 40.0 {
                8
            } else {
                16
            };
            let max_step = total_steps.round() as i32;
            for s in (0..=max_step).step_by(2) {
                let nx = (s as f32 / total_steps).clamp(0.0, 1.0);
                let x = plot.x + plot.w * nx;
                let is_bar = s % 16 == 0;
                let is_beat = s % 4 == 0;
                dl.fill(
                    Rect::new(x, plot.y, 1.0, plot.h),
                    if is_bar {
                        theme.border.grid_bar
                    } else if is_beat {
                        theme.border.grid_beat
                    } else {
                        theme.border.grid_step
                    },
                );
                if s % label_stride == 0 {
                    let (rx, align) = if s == 0 {
                        (Rect::new(plot.x, x_axis.y, 34.0, x_axis.h), TextAlign::Left)
                    } else if s == max_step {
                        (
                            Rect::new(plot.right() - 34.0, x_axis.y, 34.0, x_axis.h),
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
                        if s == 0 { "0".into() } else { format!("{s}st") },
                        font_sz,
                        is_bar,
                        theme.text.muted,
                        align,
                    );
                }
            }
            // Baseline hairline.
            dl.fill(
                Rect::new(plot.x, plot.bottom() - 1.0, plot.w, 1.0),
                theme.border.grid_bar,
            );

            dl.push_clip(plot);
            // Exponential echo decay envelope across non-dry taps.
            let env_pts: Vec<(f32, f32)> = taps
                .iter()
                .filter(|t| !t.is_dry)
                .map(|t| {
                    let x = plot.x + 8.0 + t.x_norm * (plot.w - 16.0).max(1.0);
                    let y = plot.bottom()
                        - 2.0
                        - t.amplitude.clamp(0.0, 1.0) * (plot.h - 10.0).max(1.0);
                    (x, y)
                })
                .collect();
            if env_pts.len() >= 2 {
                dl.polyline(
                    &env_pts,
                    1.3,
                    tap_r_col.with_alpha(if bypassed { 0.25 } else { 0.50 }),
                    0.0,
                    None,
                );
            }

            let bar_w = (plot.w / 32.0).clamp(6.0, 14.0);
            let sub_w = (bar_w * 0.52).clamp(3.0, 7.0);
            let pair_gap = 1.5;
            for tap in taps {
                let cx = plot.x + 8.0 + tap.x_norm * (plot.w - 16.0).max(1.0);
                let bh = (tap.amplitude.clamp(0.0, 1.0) * (plot.h - 10.0)).max(0.0);
                if bh < 1.5 {
                    continue;
                }
                let top_y = plot.bottom() - 2.0 - bh;
                if !tap.is_dry && mode == DelayMode::Stereo {
                    // Aligned dual L + R stereo tap bars at each echo step.
                    let left_bar = Rect::new(cx - sub_w - pair_gap * 0.5, top_y, sub_w, bh);
                    let right_bar = Rect::new(cx + pair_gap * 0.5, top_y, sub_w, bh);
                    for (bar, col) in [(left_bar, tap_l_col), (right_bar, tap_r_col)] {
                        dl.gradient_v(
                            bar,
                            col,
                            col.with_alpha(0.28),
                            2.0,
                            if bypassed { 0.0 } else { 0.28 },
                        );
                        dl.rounded_glow(
                            Rect::new(bar.x, bar.y, bar.w, 3.0),
                            col,
                            1.5,
                            if bypassed { 0.0 } else { 0.55 },
                        );
                    }
                } else {
                    let col = if tap.is_dry {
                        theme.text.secondary
                    } else if tap.left_channel {
                        tap_l_col
                    } else {
                        tap_r_col
                    };
                    let bar = Rect::new(cx - bar_w * 0.5, top_y, bar_w, bh);
                    dl.gradient_v(
                        bar,
                        col,
                        col.with_alpha(0.28),
                        2.5,
                        if bypassed || tap.is_dry { 0.0 } else { 0.28 },
                    );
                    // Crisp glowing cap.
                    dl.rounded_glow(
                        Rect::new(bar.x, bar.y, bar.w, 3.0),
                        col,
                        1.5,
                        if bypassed || tap.is_dry { 0.0 } else { 0.55 },
                    );
                }
            }
            if plot.w >= 140.0 && plot.h >= 34.0 {
                let legend = match mode {
                    DelayMode::PingPong => "L / R BOUNCE",
                    DelayMode::Stereo => "L + R STEREO",
                };
                dl.text(
                    Rect::new(plot.right() - 96.0, plot.y + 2.0, 90.0, 12.0),
                    legend,
                    font_sz,
                    true,
                    badge_col.with_alpha(0.85),
                    TextAlign::Right,
                );
            }
            dl.pop_clip();
        }
    }

    // Right: Timing, Feedback, and Dry/Wet Balance Card.
    if let Some(rcard) = right_card {
        let rinner = draw_card_panel(dl, rcard, theme);
        let (rhdr, rbody) = rinner.split_top(16.0);
        draw_card_header(
            dl,
            rhdr,
            "TIMING & MIX",
            &format!("{delay_ms:.0} ms"),
            theme.accent.routing,
            theme,
        );
        let gap = if rbody.h < 64.0 { 2.0 } else { 4.0 };
        let top_pad = if rbody.h < 64.0 { 1.0 } else { 4.0 };
        let row_h = ((rbody.h - top_pad - gap * 2.0) / 3.0).clamp(16.0, 34.0);
        draw_meter_bar_row(
            dl,
            Rect::new(rbody.x, rbody.y + top_pad, rbody.w, row_h),
            "Sync Steps",
            &format!("{steps:.2} st"),
            EffectParam::Time.normalize(steps),
            theme.accent.focus,
            theme,
        );
        draw_meter_bar_row(
            dl,
            Rect::new(rbody.x, rbody.y + top_pad + row_h + gap, rbody.w, row_h),
            "Feedback",
            &format!("{:.0}%", feedback * 100.0),
            EffectParam::Feedback.normalize(feedback),
            theme.accent.warning_key,
            theme,
        );
        // Split Dry / Wet balance bar.
        let mix_rect = Rect::new(
            rbody.x,
            rbody.y + top_pad + (row_h + gap) * 2.0,
            rbody.w,
            row_h,
        );
        let label_h = (row_h - 6.0).clamp(11.0, 13.0);
        let (mtop, mbot) = mix_rect.split_top(label_h);
        let font_sz = theme.font.small - 1.5;
        dl.text(
            mtop,
            format!("Dry {:.0}%", (1.0 - mix) * 100.0),
            font_sz,
            false,
            theme.text.secondary,
            TextAlign::Left,
        );
        dl.text(
            mtop,
            format!("Wet {:.0}%", mix * 100.0),
            font_sz,
            true,
            theme.accent.focus,
            TextAlign::Right,
        );
        let bar_h = if row_h < 22.0 { 4.0 } else { 6.0 };
        let track = Rect::new(mbot.x, mbot.y + 1.0, mbot.w, bar_h);
        dl.rounded(track, theme.meter.track_bg, bar_h * 0.5);
        let dry_w = track.w * (1.0 - mix).clamp(0.0, 1.0);
        if dry_w > 1.0 {
            dl.rounded(
                Rect::new(track.x, track.y, dry_w, track.h),
                theme.text.secondary.with_alpha(0.55),
                bar_h * 0.5,
            );
        }
        let wet_w = track.w * mix.clamp(0.0, 1.0);
        if wet_w > 1.0 {
            dl.rounded_glow(
                Rect::new(track.right() - wet_w, track.y, wet_w, track.h),
                if bypassed {
                    theme.text.muted
                } else {
                    theme.accent.focus
                },
                bar_h * 0.5,
                if bypassed { 0.0 } else { 0.25 },
            );
        }
    }
}

pub(super) fn draw_chorus_visual(
    dl: &mut DisplayList,
    area: Rect,
    visual: &EffectVisualView,
    bypassed: bool,
    theme: &Theme,
) {
    let EffectVisualView::Chorus {
        left_lfo,
        right_lfo,
        rate_hz,
        depth,
        width,
        mix,
        phase_deg,
        summary,
    } = visual
    else {
        return;
    };
    let (rate_hz, depth, width, mix, phase_deg) = (*rate_hz, *depth, *width, *mix, *phase_deg);
    let side_w = (area.w * 0.28).clamp(156.0, 236.0);
    let (left_card, right_card) = split_side_card(area, side_w);

    // Left: Dual Quadrature LFO Phase/Depth Sine Waves.
    let inner = draw_card_panel(dl, left_card, theme);
    let (hdr, body) = inner.split_top(18.0);
    let l_col = if bypassed {
        theme.text.muted
    } else {
        theme.accent.focus
    };
    let r_col = if bypassed {
        theme.text.muted
    } else {
        theme.accent.modulation
    };
    draw_card_header(
        dl,
        hdr,
        "QUADRATURE LFO  ·  L / R",
        &format!("L 0° · R +{phase_deg:.0}° · {summary}"),
        l_col,
        theme,
    );
    if left_lfo.len() >= 2 && right_lfo.len() >= 2 && body.h >= 40.0 {
        let (y_axis, plot) = body.inset(0.0, 4.0).split_left(28.0);
        if !plot.is_empty() {
            let bounds = [-1.1_f32, 1.1_f32];
            let mid_y = plot.y + plot.h * 0.5;
            let font_sz = theme.font.small - 2.0;
            for (v, label) in [(1.0_f32, "+"), (0.0, "0"), (-1.0, "-")] {
                let yn = ((v - bounds[0]) / (bounds[1] - bounds[0])).clamp(0.0, 1.0);
                let y = plot.bottom() - 3.0 - yn * (plot.h - 6.0).max(1.0);
                dl.fill(
                    Rect::new(plot.x, y, plot.w, if v == 0.0 { 1.2 } else { 1.0 }),
                    if v == 0.0 {
                        theme.border.grid_bar
                    } else {
                        theme.border.grid_step
                    },
                );
                dl.text(
                    Rect::new(y_axis.x, y - 6.0, y_axis.w - 4.0, 12.0),
                    label,
                    font_sz,
                    v == 0.0,
                    theme.text.muted,
                    TextAlign::Right,
                );
            }
            dl.push_clip(plot);
            let lpts = build_curve_pts(left_lfo, bounds, plot);
            let rpts = build_curve_pts(right_lfo, bounds, plot);
            dl.polyline(
                &lpts,
                2.0,
                l_col,
                if bypassed {
                    0.0
                } else {
                    theme.glow.scope * 0.75
                },
                Some((mid_y, if bypassed { 0.04 } else { 0.14 })),
            );
            dl.polyline(
                &rpts,
                2.0,
                r_col,
                if bypassed {
                    0.0
                } else {
                    theme.glow.scope * 0.75
                },
                Some((mid_y, if bypassed { 0.03 } else { 0.10 })),
            );
            dl.pop_clip();
        }
    }

    // Right: Rate, Depth, Stereo Width Spread, and Mix.
    if let Some(rcard) = right_card {
        let rinner = draw_card_panel(dl, rcard, theme);
        let (rhdr, rbody) = rinner.split_top(18.0);
        draw_card_header(
            dl,
            rhdr,
            "STEREO MOD",
            &format!("Δ {phase_deg:.0}°"),
            r_col,
            theme,
        );
        let gap = 2.0;
        let row_h = ((rbody.h - 4.0 - gap * 3.0) / 4.0).clamp(16.0, 26.0);
        draw_meter_bar_row(
            dl,
            Rect::new(rbody.x, rbody.y + 2.0, rbody.w, row_h),
            "Rate",
            &format!("{rate_hz:.2} Hz"),
            EffectParam::Rate.normalize(rate_hz),
            theme.accent.focus,
            theme,
        );
        draw_meter_bar_row(
            dl,
            Rect::new(rbody.x, rbody.y + 2.0 + (row_h + gap), rbody.w, row_h),
            "Depth",
            &format!("{:.0}%", depth * 100.0),
            depth,
            theme.accent.modulation,
            theme,
        );
        // Centered stereo width L<->R indicator bar.
        let wrect = Rect::new(rbody.x, rbody.y + 2.0 + (row_h + gap) * 2.0, rbody.w, row_h);
        let label_h = (row_h - 6.0).clamp(11.0, 13.0);
        let (wtop, wbot) = wrect.split_top(label_h);
        let font_sz = theme.font.small - 1.5;
        dl.text(
            wtop,
            "Width",
            font_sz,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
        dl.text(
            wtop,
            format!("{:.0}%", width * 100.0),
            font_sz,
            true,
            theme.text.primary,
            TextAlign::Right,
        );
        let bar_h = if row_h < 22.0 { 4.0 } else { 5.0 };
        let track = Rect::new(wbot.x, wbot.y + 1.0, wbot.w, bar_h);
        dl.rounded(track, theme.meter.track_bg, bar_h * 0.5);
        let spread_w = (track.w * width.clamp(0.0, 1.0)).max(2.0);
        let sx = track.x + (track.w - spread_w) * 0.5;
        dl.rounded_glow(
            Rect::new(sx, track.y, spread_w, track.h),
            theme.accent.routing,
            bar_h * 0.5,
            0.22,
        );
        draw_meter_bar_row(
            dl,
            Rect::new(rbody.x, rbody.y + 2.0 + (row_h + gap) * 3.0, rbody.w, row_h),
            "Mix",
            &format!("{:.0}%", mix * 100.0),
            mix,
            theme.accent.playhead,
            theme,
        );
    }
}
