//! Rhythmic Filter / PCF frequency response and step-envelope visualizer for the GPU Effect deck.
use super::{
    build_curve_pts, draw_card_header, draw_card_panel, draw_freq_ticks_and_grid,
    draw_meter_bar_row, sample_curve_y, split_side_card,
};
use crate::display::{DisplayList, Rect, TextAlign};
use crate::theme::{Color, Theme};
use cadence_app::view::{EffectVisualView, FilterEnvVisual};
use cadence_core::mixer::EffectParam;

pub(super) fn draw_filter_visual(
    dl: &mut DisplayList,
    area: Rect,
    visual: &EffectVisualView,
    bypassed: bool,
    theme: &Theme,
) {
    let EffectVisualView::Filter {
        freq_high_hz,
        mode_label,
        resonance,
        drive,
        mix,
        curve_db,
        mod_curve_db,
        cutoff_norm,
        mod_cutoff_norm,
        env,
        summary,
        ..
    } = visual
    else {
        return;
    };
    let (freq_high_hz, resonance, drive, mix, mod_curve_db, cutoff_norm, mod_cutoff_norm, env) = (
        *freq_high_hz,
        *resonance,
        *drive,
        *mix,
        mod_curve_db.as_deref(),
        *cutoff_norm,
        *mod_cutoff_norm,
        env.as_ref(),
    );
    let side_w = if env.is_some() {
        (area.w * 0.42).clamp(210.0, 360.0)
    } else {
        (area.w * 0.25).clamp(150.0, 224.0)
    };
    let (left_card, right_card) = split_side_card(area, side_w);

    let inner = draw_card_panel(dl, left_card, theme);
    let (hdr, body) = inner.split_top(18.0);
    let curve_col = if bypassed {
        theme.text.muted
    } else {
        theme.accent.warning_key
    };
    draw_card_header(dl, hdr, "FILTER  ·  RESPONSE", summary, curve_col, theme);
    if curve_db.len() >= 2 && body.h >= 40.0 {
        let (upper, x_axis_full) = body.split_bottom(14.0);
        let (y_axis, plot) = upper.split_left(32.0);
        let x_axis = Rect::new(plot.x, x_axis_full.y, plot.w, x_axis_full.h);
        if !plot.is_empty() {
            let bounds = [-48.0_f32, 18.0_f32];
            let y_of_db = |db: f32| -> f32 {
                let yn = ((db - bounds[0]) / (bounds[1] - bounds[0])).clamp(0.0, 1.0);
                plot.bottom() - 3.0 - yn * (plot.h - 6.0).max(1.0)
            };
            let font_sz = theme.font.small - 2.0;
            for (db, label) in [(18.0, "+18"), (0.0, "0dB"), (-24.0, "-24"), (-48.0, "-48")] {
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
                if db == -24.0 && plot.h < 58.0 {
                    continue;
                }
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
            draw_freq_ticks_and_grid(dl, plot, x_axis, freq_high_hz, theme);

            dl.push_clip(plot);
            let pts = build_curve_pts(curve_db, bounds, plot);
            dl.polyline(
                &pts,
                2.0,
                curve_col,
                if bypassed {
                    0.0
                } else {
                    theme.glow.scope * 0.75
                },
                Some((plot.bottom(), if bypassed { 0.04 } else { 0.16 })),
            );

            let base_x = plot.x + plot.w * cutoff_norm.clamp(0.0, 1.0);
            let base_y = sample_curve_y(curve_db, cutoff_norm, bounds, plot);

            if let Some(mod_norm) = mod_cutoff_norm {
                if !bypassed {
                    let mod_x = plot.x + plot.w * mod_norm.clamp(0.0, 1.0);
                    let x0 = base_x.min(mod_x);
                    let x1 = base_x.max(mod_x);
                    if x1 - x0 >= 1.5 {
                        dl.fill(
                            Rect::new(x0, plot.y, x1 - x0, plot.h),
                            theme.accent.modulation.with_alpha(0.16),
                        );
                    }
                    if let Some(mc) = mod_curve_db {
                        if mc.len() >= 2 {
                            let mpts = build_curve_pts(mc, bounds, plot);
                            dl.polyline(
                                &mpts,
                                1.9,
                                theme.accent.modulation,
                                theme.glow.scope * 0.85,
                                Some((plot.bottom(), 0.12)),
                            );
                        }
                    }
                    let mod_y = mod_curve_db.map_or_else(
                        || sample_curve_y(curve_db, mod_norm, bounds, plot),
                        |mc| sample_curve_y(mc, mod_norm, bounds, plot),
                    );
                    dl.fill(
                        Rect::new(mod_x - 0.5, plot.y, 1.2, plot.h),
                        theme.accent.modulation.with_alpha(0.75),
                    );
                    dl.rounded_glow(
                        Rect::new(mod_x - 4.0, mod_y - 4.0, 8.0, 8.0),
                        theme.accent.modulation,
                        4.0,
                        0.55,
                    );
                }
            }

            dl.fill(
                Rect::new(base_x - 0.5, plot.y, 1.0, plot.h),
                curve_col.with_alpha(0.55),
            );
            dl.rounded_glow(
                Rect::new(base_x - 3.0, base_y - 3.0, 6.0, 6.0),
                curve_col,
                3.0,
                if bypassed { 0.0 } else { 0.35 },
            );
            dl.pop_clip();
        }
    }

    if let Some(rcard) = right_card {
        let rinner = draw_card_panel(dl, rcard, theme);
        let (rhdr, rbody) = rinner.split_top(18.0);
        if let Some(ev) = env {
            let env_col = if bypassed {
                theme.text.muted
            } else if ev.env_amount < 0.0 {
                theme.accent.alert
            } else {
                theme.accent.modulation
            };
            let badge = format!(
                "{} · 1/{} · {}st",
                ev.source_label,
                ev.resolution,
                ev.steps.len()
            );
            draw_card_header(dl, rhdr, "RHYTHMIC ENV", &badge, env_col, theme);
            draw_filter_env_steps(dl, rbody, ev, env_col, bypassed, theme);
        } else {
            draw_card_header(
                dl,
                rhdr,
                "TOPOLOGY",
                mode_label,
                theme.accent.routing,
                theme,
            );
            let row_h = ((rbody.h - 10.0) / 3.0).clamp(20.0, 34.0);
            draw_meter_bar_row(
                dl,
                Rect::new(rbody.x, rbody.y + 4.0, rbody.w, row_h),
                "Resonance",
                &format!("{:.0}%", resonance * 100.0),
                EffectParam::Resonance.normalize(resonance),
                theme.accent.warning_key,
                theme,
            );
            draw_meter_bar_row(
                dl,
                Rect::new(rbody.x, rbody.y + 4.0 + row_h + 4.0, rbody.w, row_h),
                "Pre-Drive",
                &format!("{:.0}%", drive * 100.0),
                drive,
                theme.accent.alert,
                theme,
            );
            draw_meter_bar_row(
                dl,
                Rect::new(rbody.x, rbody.y + 4.0 + (row_h + 4.0) * 2.0, rbody.w, row_h),
                "Wet Mix",
                &format!("{:.0}%", mix * 100.0),
                mix,
                theme.accent.focus,
                theme,
            );
        }
    }
}

fn draw_filter_env_steps(
    dl: &mut DisplayList,
    area: Rect,
    ev: &FilterEnvVisual,
    env_col: Color,
    bypassed: bool,
    theme: &Theme,
) {
    if area.h < 32.0 || ev.steps.is_empty() {
        return;
    }
    let (bars_area, footer) = area.split_bottom(22.0);
    let plot = Rect::new(
        bars_area.x + 2.0,
        bars_area.y + 4.0,
        (bars_area.w - 4.0).max(0.0),
        (bars_area.h - 8.0).max(1.0),
    );
    if plot.w < 16.0 || plot.h < 12.0 {
        return;
    }

    draw_filter_env_plot(
        dl,
        plot,
        &FilterEnvPlotSpec {
            steps: &ev.steps,
            env_curve: &ev.env_curve,
            resolution: ev.resolution,
            smooth_attack: ev.smooth_attack,
            active_step: Some(ev.active_step),
            env_col,
            bypassed,
        },
        theme,
    );

    // Footer: live envelope level bar + Amt / Decay / Attack readout.
    let (meta_row, meter_row) = footer.split_top(13.0);
    let atk_tag = if ev.smooth_attack { "Smooth" } else { "Punch" };
    let left_txt = format!("{:+.1} oct · {:.0}ms", ev.env_amount, ev.env_decay_ms);
    let font_sz = theme.font.small - 2.0;
    dl.text(
        meta_row,
        &left_txt,
        font_sz,
        true,
        if bypassed {
            theme.text.muted
        } else {
            theme.text.primary
        },
        TextAlign::Left,
    );
    dl.text(
        meta_row,
        atk_tag,
        font_sz,
        false,
        theme.text.muted,
        TextAlign::Right,
    );
    let track = Rect::new(meter_row.x, meter_row.y + 2.0, meter_row.w, 4.0);
    dl.rounded(track, theme.meter.track_bg, 2.0);
    let fw = (track.w * ev.env_level.clamp(0.0, 1.0)).max(0.0);
    if fw > 0.5 {
        dl.rounded_glow(
            Rect::new(track.x, track.y, fw, track.h),
            env_col,
            2.0,
            if bypassed { 0.0 } else { 0.35 },
        );
    }
}

pub(crate) struct FilterEnvPlotSpec<'a> {
    pub steps: &'a [f32],
    pub env_curve: &'a [f32],
    pub resolution: u8,
    pub smooth_attack: bool,
    pub active_step: Option<usize>,
    pub env_col: Color,
    pub bypassed: bool,
}

/// Shared SPOT rendering a rhythmic filter groove's step-velocity bars, beat-group guides,
/// optional playhead step highlight, and coupled-ODE envelope contour inside `plot`.
pub(crate) fn draw_filter_env_plot(
    dl: &mut DisplayList,
    plot: Rect,
    spec: &FilterEnvPlotSpec<'_>,
    theme: &Theme,
) {
    let FilterEnvPlotSpec {
        steps,
        env_curve,
        resolution,
        smooth_attack,
        active_step,
        env_col,
        bypassed,
    } = *spec;
    if plot.w < 16.0 || plot.h < 12.0 || steps.is_empty() {
        return;
    }
    dl.push_clip(plot);
    // Subtle baseline + half-height guide.
    dl.fill(
        Rect::new(plot.x, plot.bottom() - 1.0, plot.w, 1.0),
        theme.border.grid_bar,
    );
    dl.fill(
        Rect::new(plot.x, plot.y + plot.h * 0.5, plot.w, 1.0),
        theme.border.grid_step.with_alpha(0.55),
    );

    let n = steps.len();
    let cell_w = plot.w / n as f32;
    let gap = if cell_w >= 10.0 {
        2.0
    } else if cell_w >= 5.0 {
        1.0
    } else {
        0.5
    };
    let bar_w = (cell_w - gap).max(1.5);
    let max_h = (plot.h - 3.0).max(4.0);
    let group = if resolution == 32 { 8 } else { 4 };

    for (i, &val) in steps.iter().enumerate() {
        let bx = plot.x + i as f32 * cell_w + gap * 0.5;
        let is_active = active_step == Some(i);
        if i > 0 && i % group == 0 {
            dl.fill(
                Rect::new(plot.x + i as f32 * cell_w, plot.y, 1.0, plot.h - 1.0),
                theme.border.grid_step.with_alpha(0.45),
            );
        }
        if is_active && !bypassed {
            dl.fill(
                Rect::new(bx - 0.5, plot.y, bar_w + 1.0, plot.h - 1.0),
                theme.accent.playhead.with_alpha(0.14),
            );
        }
        if val > 0.005 {
            let bh = (val.clamp(0.0, 1.0) * max_h).max(3.0);
            let by = plot.bottom() - 1.0 - bh;
            let col = if bypassed {
                theme.text.muted
            } else if is_active {
                theme.accent.playhead
            } else if smooth_attack {
                theme.accent.focus.with_alpha(0.55 + 0.40 * val)
            } else {
                env_col.with_alpha(0.50 + 0.45 * val)
            };
            let glow = if bypassed {
                0.0
            } else if is_active {
                0.55
            } else {
                0.15 * val
            };
            dl.rounded_glow(Rect::new(bx, by, bar_w, bh), col, 1.5, glow);
        } else {
            let col = if is_active && !bypassed {
                theme.accent.playhead.with_alpha(0.85)
            } else {
                theme.border.hairline
            };
            dl.fill(Rect::new(bx, plot.bottom() - 2.5, bar_w, 1.5), col);
        }
    }

    // Smooth coupled ODE envelope contour (matching real-time RhythmicEnv decay & attack tau).
    if env_curve.len() >= 2 {
        let total = env_curve.len() as f32;
        let cpts: Vec<(f32, f32)> = env_curve
            .iter()
            .enumerate()
            .map(|(idx, &v)| {
                let x = plot.x + plot.w * ((idx as f32 + 0.5) / total);
                let y = plot.bottom() - 1.0 - v.clamp(0.0, 1.0) * max_h;
                (x, y)
            })
            .collect();
        let stroke_col = if bypassed {
            theme.text.muted
        } else {
            theme.accent.focus.mix(env_col, 0.50)
        };
        dl.polyline(
            &cpts,
            1.6,
            stroke_col,
            if bypassed { 0.0 } else { 0.40 },
            None,
        );
    }
    dl.pop_clip();
}
