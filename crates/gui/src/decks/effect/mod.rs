//! GPU Effect deck: dedicated studio DSP visualizers for all 8 native insert FX
//! (EQ, Filter/PCF, Compressor + live GR meter, Limiter + ceiling/GR meter, Drive,
//! Delay, Chorus, Reverb), crisp ACTIVE/BYPASSED badge, and normalized keyboard-driven
//! parameter controls.
mod drive;
mod dynamics;
mod eq;
pub(crate) mod filter;
mod reverb;
mod time;

use crate::chrome::{draw_deck_frame, DeckFrameSpec};
use crate::decks::object::{control_list_preferred_height, draw_control_list};
pub(super) use crate::display::{build_curve_pts, sample_curve_y};
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::{Color, Theme};
use cadence_app::view::{EffectDeckView, EffectVisualView, UiModeKind};
use cadence_core::dsp::log_to_norm;

pub fn draw_effect(
    dl: &mut DisplayList,
    area: Rect,
    view: &EffectDeckView,
    focused: bool,
    mode: UiModeKind,
    theme: &Theme,
) {
    let accent = if view.bypassed {
        theme.accent.alert
    } else {
        theme.accent.focus
    };
    let raw_title = if view.channel_name.is_empty() {
        format!("EFFECT  ·  {}", view.name)
    } else {
        format!("EFFECT  ·  {}  ·  on {}", view.name, view.channel_name)
    };

    // Reserve exact width on the right of the header bar for [KIND] + [ACTIVE/BYPASSED] pills.
    let status_text = if view.bypassed { "BYPASSED" } else { "ACTIVE" };
    let cw = theme.char_width(theme.font.small - 1.0);
    let kind_w = (view.kind_label.chars().count() as f32 * cw + 14.0).clamp(36.0, 96.0);
    let status_w = (status_text.chars().count() as f32 * cw + 16.0).clamp(56.0, 88.0);
    let pills_total_w = kind_w + status_w + 14.0;
    let avail_title_w = (area.w - pills_total_w - 28.0).max(40.0);
    let title_max_chars = (avail_title_w / theme.char_width(theme.font.small)) as usize;
    let trimmed_title = trim_label(&raw_title, title_max_chars);

    let inner = draw_deck_frame(
        dl,
        area,
        &DeckFrameSpec {
            title: &trimmed_title,
            right_badge: None,
            accent,
            focused,
            mode,
        },
        theme,
    );

    // Draw crisp [KIND] + [ACTIVE / BYPASSED] pills right-aligned in the deck header strip.
    let inset_b = 1.5;
    let hdr = Rect::new(
        area.x + inset_b,
        area.y + inset_b,
        (area.w - inset_b * 2.0).max(0.0),
        24.0,
    );
    if hdr.w >= pills_total_w + 48.0 {
        let status_rect = Rect::new(hdr.right() - status_w - 6.0, hdr.y + 3.0, status_w, 18.0);
        let status_col = if view.bypassed {
            theme.accent.alert
        } else {
            theme.accent.playhead
        };
        dl.panel(
            status_rect,
            status_col.with_alpha(if view.bypassed { 0.24 } else { 0.18 }),
            status_col.with_alpha(0.85),
            theme.radii.badge,
            1.0,
            if view.bypassed { 0.24 } else { 0.18 },
        );
        dl.text(
            status_rect,
            status_text,
            theme.font.small - 1.0,
            true,
            status_col,
            TextAlign::Center,
        );

        let kind_rect = Rect::new(status_rect.x - kind_w - 6.0, hdr.y + 3.0, kind_w, 18.0);
        dl.panel(
            kind_rect,
            theme.surface.recessed,
            theme.border.hairline,
            theme.radii.badge,
            1.0,
            0.0,
        );
        dl.text(
            kind_rect,
            &view.kind_label,
            theme.font.small - 1.0,
            true,
            theme.accent.routing,
            TextAlign::Center,
        );
    }

    if inner.w < 160.0 || inner.h < 96.0 {
        return;
    }
    dl.push_clip(inner);

    let body = inner;
    // Size visualizer card so all FX controls (including STATE / ROUTING) stay visible without clipping.
    let needed_ctrl_h = control_list_preferred_height(&view.controls, body.w);
    let max_vis_h = (body.h - needed_ctrl_h).max(84.0);
    let vis_h = (body.h * 0.50).clamp(84.0, 236.0).min(max_vis_h);
    let (vis_outer, ctrl_area) = body.split_top(vis_h);
    let vis_area = Rect::new(
        vis_outer.x,
        vis_outer.y,
        vis_outer.w,
        (vis_outer.h - 5.0).max(0.0),
    );

    let bypassed = view.bypassed;
    if matches!(view.visual, EffectVisualView::Missing) {
        draw_control_list(dl, body, &view.controls, view.control_cursor, theme);
    } else {
        match &view.visual {
            EffectVisualView::Eq { .. } => {
                eq::draw_eq_visual(dl, vis_area, &view.visual, bypassed, theme);
            }
            EffectVisualView::Filter { .. } => {
                filter::draw_filter_visual(dl, vis_area, &view.visual, bypassed, theme);
            }
            EffectVisualView::Compressor { .. } => {
                dynamics::draw_compressor_visual(dl, vis_area, &view.visual, bypassed, theme);
            }
            EffectVisualView::Limiter { .. } => {
                dynamics::draw_limiter_visual(dl, vis_area, &view.visual, bypassed, theme);
            }
            EffectVisualView::Drive { .. } => {
                drive::draw_drive_visual(dl, vis_area, &view.visual, bypassed, theme);
            }
            EffectVisualView::Delay { .. } => {
                time::draw_delay_visual(dl, vis_area, &view.visual, bypassed, theme);
            }
            EffectVisualView::Chorus { .. } => {
                time::draw_chorus_visual(dl, vis_area, &view.visual, bypassed, theme);
            }
            EffectVisualView::Reverb { .. } => {
                reverb::draw_reverb_visual(dl, vis_area, &view.visual, bypassed, theme);
            }
            EffectVisualView::Missing => unreachable!(),
        }
        draw_control_list(dl, ctrl_area, &view.controls, view.control_cursor, theme);
    }

    dl.pop_clip();
}

/// Split an effect visualizer row into a primary left card and optional right telemetry card.
pub(super) fn split_side_card(area: Rect, side_w: f32) -> (Rect, Option<Rect>) {
    if area.w >= 460.0 {
        let (l, r) = area.split_right(side_w);
        (Rect::new(l.x, l.y, (l.w - 6.0).max(0.0), l.h), Some(r))
    } else {
        (area, None)
    }
}

pub(super) fn draw_card_panel(dl: &mut DisplayList, rect: Rect, theme: &Theme) -> Rect {
    dl.panel(
        rect,
        theme.surface.recessed,
        theme.border.subtle,
        theme.radii.pill,
        1.0,
        0.0,
    );
    let pad_y = if rect.h < 96.0 { 4.0 } else { 6.0 };
    rect.inset(8.0, pad_y)
}

pub(super) fn draw_card_header(
    dl: &mut DisplayList,
    hdr: Rect,
    title: &str,
    summary: &str,
    title_col: Color,
    theme: &Theme,
) {
    let font_sz = theme.font.small - 1.0;
    let cw = theme.char_width(font_sz);
    let title_w = (title.chars().count() as f32 * cw + 8.0).min(hdr.w);
    dl.text(
        Rect::new(hdr.x, hdr.y, title_w, hdr.h),
        title,
        font_sz,
        true,
        title_col,
        TextAlign::Left,
    );
    let rem_w = hdr.w - title_w - 8.0;
    if rem_w >= 48.0 && !summary.is_empty() {
        let max_chars = (rem_w / cw) as usize;
        dl.text(
            Rect::new(hdr.x + title_w + 8.0, hdr.y, rem_w, hdr.h),
            trim_label(summary, max_chars),
            font_sz,
            false,
            theme.text.secondary,
            TextAlign::Right,
        );
    }
}

pub(super) struct ModeBadgeHeader<'a> {
    pub badge: &'a str,
    pub badge_col: Color,
    pub title: &'a str,
    pub summary: &'a str,
    pub title_col: Color,
    pub bypassed: bool,
}

/// Card header featuring a crisp mode badge pill on the left, followed by the card title
/// and right-aligned summary (strictly styled from `&Theme`, zero overlap).
pub(super) fn draw_mode_badge_header(
    dl: &mut DisplayList,
    hdr: Rect,
    spec: &ModeBadgeHeader<'_>,
    theme: &Theme,
) {
    let ModeBadgeHeader {
        badge,
        badge_col,
        title,
        summary,
        title_col,
        bypassed,
    } = *spec;
    let badge_sz = theme.font.small - 1.5;
    let badge_cw = theme.char_width(badge_sz);
    let badge_w = (badge.chars().count() as f32 * badge_cw + 12.0)
        .clamp(42.0, 90.0)
        .min(hdr.w);
    let badge_h = (hdr.h - 2.0).clamp(12.0, 16.0);
    let badge_rect = Rect::new(hdr.x, hdr.y + (hdr.h - badge_h) * 0.5, badge_w, badge_h);
    dl.panel(
        badge_rect,
        badge_col.with_alpha(if bypassed { 0.10 } else { 0.18 }),
        badge_col.with_alpha(if bypassed { 0.45 } else { 0.85 }),
        theme.radii.badge,
        1.0,
        if bypassed { 0.0 } else { 0.16 },
    );
    dl.text(
        badge_rect,
        badge,
        badge_sz,
        true,
        badge_col,
        TextAlign::Center,
    );

    let rest_x = badge_rect.right() + 7.0;
    let rest_w = (hdr.right() - rest_x).max(0.0);
    if rest_w >= 40.0 {
        draw_card_header(
            dl,
            Rect::new(rest_x, hdr.y, rest_w, hdr.h),
            title,
            summary,
            title_col,
            theme,
        );
    }
}

pub(super) fn draw_freq_ticks_and_grid(
    dl: &mut DisplayList,
    plot: Rect,
    x_axis: Rect,
    freq_high_hz: f32,
    theme: &Theme,
) {
    let high_hz = freq_high_hz.clamp(2_000.0, 48_000.0);
    let norm_of = |hz: f32| log_to_norm(hz, 20.0, high_hz);
    for hz in [50.0_f32, 100.0, 500.0, 1_000.0, 5_000.0, 10_000.0] {
        if hz >= high_hz * 0.96 {
            continue;
        }
        let x = plot.x + plot.w * norm_of(hz);
        let major = hz == 100.0 || hz == 1_000.0 || hz == 10_000.0;
        dl.fill(
            Rect::new(x, plot.y, 1.0, plot.h),
            if major {
                theme.border.grid_beat
            } else {
                theme.border.grid_step
            },
        );
    }
    let font_sz = theme.font.small - 2.0;
    let show_10k = plot.w >= 320.0 && high_hz >= 14_000.0;
    let right_label = {
        let khz = high_hz / 1_000.0;
        if (khz - khz.round()).abs() < 0.05 {
            format!("{:.0}k", khz.round())
        } else {
            format!("{khz:.1}k")
        }
    };
    for (hz, label, align) in [
        (20.0_f32, "20Hz", TextAlign::Left),
        (100.0, "100", TextAlign::Center),
        (1_000.0, "1k", TextAlign::Center),
        (10_000.0, "10k", TextAlign::Center),
        (high_hz, right_label.as_str(), TextAlign::Right),
    ] {
        if hz == 10_000.0 && !show_10k {
            continue;
        }
        let nx = norm_of(hz);
        let rx = match align {
            TextAlign::Left => Rect::new(plot.x, x_axis.y, 38.0, x_axis.h),
            TextAlign::Right => Rect::new(plot.right() - 38.0, x_axis.y, 38.0, x_axis.h),
            TextAlign::Center => Rect::new(plot.x + plot.w * nx - 20.0, x_axis.y, 40.0, x_axis.h),
        };
        dl.text(rx, label, font_sz, false, theme.text.muted, align);
    }
}

pub(super) struct LpResponseSpec<'a> {
    pub title: &'a str,
    pub summary: &'a str,
    pub freq_high_hz: f32,
    pub curve_db: &'a [f32],
    pub cutoff_norm: f32,
    pub curve_col: Color,
    pub marker_line_col: Color,
    pub marker_dot_col: Color,
    pub bypassed: bool,
}

/// Shared `-24..+3 dB` low-pass frequency-response card (used by Drive Tone & Reverb HF Damping).
pub(super) fn draw_lp_response_card(
    dl: &mut DisplayList,
    card: Rect,
    spec: &LpResponseSpec<'_>,
    theme: &Theme,
) {
    let LpResponseSpec {
        title,
        summary,
        freq_high_hz,
        curve_db,
        cutoff_norm,
        curve_col,
        marker_line_col,
        marker_dot_col,
        bypassed,
    } = *spec;
    let rinner = draw_card_panel(dl, card, theme);
    let (rhdr, rbody) = rinner.split_top(18.0);
    draw_card_header(dl, rhdr, title, summary, curve_col, theme);
    if curve_db.len() >= 2 && rbody.h >= 36.0 {
        let (upper, x_axis_full) = rbody.split_bottom(14.0);
        let (y_axis, plot) = upper.split_left(28.0);
        let x_axis = Rect::new(plot.x, x_axis_full.y, plot.w, x_axis_full.h);
        if !plot.is_empty() {
            let bounds = [-24.0_f32, 3.0_f32];
            let y_of_db = |db: f32| -> f32 {
                let yn = ((db - bounds[0]) / (bounds[1] - bounds[0])).clamp(0.0, 1.0);
                plot.bottom() - 3.0 - yn * (plot.h - 6.0).max(1.0)
            };
            let font_sz = theme.font.small - 2.0;
            for (db, label) in [(0.0_f32, "0"), (-12.0, "-12"), (-24.0, "-24")] {
                let y = y_of_db(db);
                dl.fill(
                    Rect::new(plot.x, y, plot.w, 1.0),
                    if db == 0.0 {
                        theme.border.grid_bar
                    } else {
                        theme.border.grid_step
                    },
                );
                dl.text(
                    Rect::new(y_axis.x, y - 6.0, y_axis.w - 4.0, 12.0),
                    label,
                    font_sz,
                    db == 0.0,
                    theme.text.muted,
                    TextAlign::Right,
                );
            }
            draw_freq_ticks_and_grid(dl, plot, x_axis, freq_high_hz, theme);
            dl.push_clip(plot);
            let pts = build_curve_pts(curve_db, bounds, plot);
            dl.polyline(
                &pts,
                1.9,
                curve_col,
                if bypassed {
                    0.0
                } else {
                    theme.glow.scope * 0.7
                },
                Some((plot.bottom(), if bypassed { 0.04 } else { 0.14 })),
            );
            let cx = plot.x + plot.w * cutoff_norm.clamp(0.0, 1.0);
            let cy = sample_curve_y(curve_db, cutoff_norm, bounds, plot);
            dl.fill(Rect::new(cx - 0.5, plot.y, 1.0, plot.h), marker_line_col);
            dl.rounded_glow(
                Rect::new(cx - 3.5, cy - 3.5, 7.0, 7.0),
                marker_dot_col,
                3.5,
                if bypassed { 0.0 } else { 0.45 },
            );
            dl.pop_clip();
        }
    }
}

pub(super) struct GrMeterSpec {
    pub gain_reduction_db: f32,
    pub gr_hold_db: f32,
    pub alert_gr_db: f32,
    pub alert_hold_db: f32,
    pub bar_glow: f32,
    pub bypassed: bool,
}

/// Shared right-hand Gain Reduction (`0..-24 dB`) drop-bar meter + peak-hold tick panel
/// used by both Compressor and Limiter. Returns the telemetry column rect to the right of the meter.
pub(super) fn draw_gr_meter_panel(
    dl: &mut DisplayList,
    card: Rect,
    spec: &GrMeterSpec,
    theme: &Theme,
) -> Rect {
    let GrMeterSpec {
        gain_reduction_db,
        gr_hold_db,
        alert_gr_db,
        alert_hold_db,
        bar_glow,
        bypassed,
    } = *spec;
    let rinner = draw_card_panel(dl, card, theme);
    let (rhdr, rbody) = rinner.split_top(18.0);
    let active_gr = !bypassed && (gain_reduction_db > 0.05 || gr_hold_db > 0.05);
    let gr_label = if !active_gr {
        "0.0 dB".into()
    } else if gr_hold_db > gain_reduction_db + 0.15 {
        format!("-{gain_reduction_db:.1} (pk -{gr_hold_db:.1})")
    } else {
        format!("-{gain_reduction_db:.1} dB")
    };
    let gr_col = if active_gr {
        if gain_reduction_db >= alert_gr_db || gr_hold_db >= alert_hold_db {
            theme.meter.clip
        } else {
            theme.meter.gr
        }
    } else {
        theme.text.muted
    };
    draw_card_header(dl, rhdr, "GAIN REDUCTION", &gr_label, gr_col, theme);

    let (meter_col, info_col) = rbody.split_left(68.0);
    let meter_area = meter_col.inset(2.0, 2.0);
    if meter_area.h >= 36.0 {
        let (scale_col, bar_col) = meter_area.split_left(28.0);
        let track = Rect::new(bar_col.x + 6.0, bar_col.y, 18.0, bar_col.h);
        dl.panel(
            track,
            theme.meter.track_bg,
            theme.border.subtle,
            3.0,
            1.0,
            0.0,
        );
        let font_sz = theme.font.small - 2.0;
        let dense_ticks = track.h >= 68.0;
        for (db, label) in [
            (0.0_f32, "0"),
            (6.0, "-6"),
            (12.0, "-12"),
            (18.0, "-18"),
            (24.0, "-24"),
        ] {
            let y = track.y + track.h * (db / 24.0);
            if dense_ticks || db == 0.0 || db == 12.0 || db == 24.0 {
                let ty = (y - 5.5).clamp(track.y - 1.0, track.bottom() - 10.5);
                dl.text(
                    Rect::new(scale_col.x, ty, scale_col.w, 11.0),
                    label,
                    font_sz,
                    db == 0.0,
                    theme.text.muted,
                    TextAlign::Right,
                );
            }
            if db > 0.0 && db < 24.0 {
                dl.fill(
                    Rect::new(track.x + 1.0, y, track.w - 2.0, 1.0),
                    theme.border.grid_step,
                );
            }
        }
        // Top-down GR reduction drop bar + peak-hold marker tick.
        let inner_h = (track.h - 2.0).max(0.0);
        let gr_norm = (gain_reduction_db / 24.0).clamp(0.0, 1.0);
        let drop_h = (inner_h * gr_norm).clamp(0.0, inner_h);
        if !bypassed && drop_h > 0.5 {
            let fill = Rect::new(track.x + 1.0, track.y + 1.0, track.w - 2.0, drop_h);
            dl.gradient_v(fill, theme.meter.gr, theme.meter.clip, 2.0, bar_glow);
        }
        let hold_norm = (gr_hold_db / 24.0).clamp(0.0, 1.0);
        if !bypassed && hold_norm > 0.01 {
            let hy =
                (track.y + 1.0 + inner_h * hold_norm).clamp(track.y + 1.0, track.bottom() - 3.5);
            dl.fill(
                Rect::new(track.x - 1.0, hy, track.w + 2.0, 2.5),
                if gr_hold_db >= alert_hold_db {
                    theme.meter.clip
                } else {
                    theme.meter.hold_tick
                },
            );
        }
    }
    info_col
}

/// Shared normalized `[0, 1] x [0, 1]` transfer-curve coordinate helper for dynamics plots.
#[derive(Clone, Copy, Debug)]
pub(super) struct TransferPlot {
    pub plot: Rect,
}

impl TransferPlot {
    pub(super) fn px_of(self, nx: f32) -> f32 {
        self.plot.x + 3.0 + nx.clamp(0.0, 1.0) * (self.plot.w - 6.0).max(1.0)
    }

    pub(super) fn py_of(self, ny: f32) -> f32 {
        self.plot.bottom() - 3.0 - ny.clamp(0.0, 1.0) * (self.plot.h - 6.0).max(1.0)
    }

    pub(super) fn draw_unity_diagonal(self, dl: &mut DisplayList, theme: &Theme) {
        dl.polyline(
            &[
                (self.px_of(0.0), self.py_of(0.0)),
                (self.px_of(1.0), self.py_of(1.0)),
            ],
            1.1,
            theme.border.grid_bar,
            0.0,
            None,
        );
    }

    pub(super) fn draw_active_curve(
        self,
        dl: &mut DisplayList,
        transfer_curve: &[(f32, f32)],
        stroke_width: f32,
        curve_col: Color,
        bypassed: bool,
        theme: &Theme,
    ) {
        let tpts: Vec<(f32, f32)> = transfer_curve
            .iter()
            .map(|&(nx, ny)| (self.px_of(nx), self.py_of(ny)))
            .collect();
        dl.polyline(
            &tpts,
            stroke_width,
            curve_col,
            if bypassed {
                0.0
            } else {
                theme.glow.scope * 0.85
            },
            Some((self.plot.bottom() - 3.0, if bypassed { 0.04 } else { 0.14 })),
        );
    }

    pub(super) fn draw_knee_dot(
        self,
        dl: &mut DisplayList,
        transfer_curve: &[(f32, f32)],
        norm_x: f32,
        glow: f32,
        bypassed: bool,
        theme: &Theme,
    ) {
        let idx = ((norm_x * (transfer_curve.len() - 1) as f32).round() as usize)
            .min(transfer_curve.len() - 1);
        let (kx, ky) = transfer_curve[idx];
        dl.rounded_glow(
            Rect::new(self.px_of(kx) - 3.5, self.py_of(ky) - 3.5, 7.0, 7.0),
            if bypassed {
                theme.text.muted
            } else {
                theme.accent.warning_key
            },
            3.5,
            if bypassed { 0.0 } else { glow },
        );
    }
}

pub(super) fn draw_meter_bar_row(
    dl: &mut DisplayList,
    rect: Rect,
    label: &str,
    value_text: &str,
    norm: f32,
    color: Color,
    theme: &Theme,
) {
    if rect.h < 16.0 || rect.w < 40.0 {
        return;
    }
    let font_sz = theme.font.small - 1.5;
    let label_h = (rect.h - 6.0).clamp(11.0, 13.0);
    let (top, bot) = rect.split_top(label_h);
    dl.text(
        top,
        label,
        font_sz,
        false,
        theme.text.muted,
        TextAlign::Left,
    );
    dl.text(
        top,
        value_text,
        font_sz,
        true,
        theme.text.primary,
        TextAlign::Right,
    );
    let bar_h = if rect.h < 22.0 { 4.0 } else { 5.0 };
    let track = Rect::new(bot.x, bot.y + 1.0, bot.w, bar_h);
    dl.rounded(track, theme.meter.track_bg, bar_h * 0.5);
    let fw = (track.w * norm.clamp(0.0, 1.0)).max(0.0);
    if fw > 0.5 {
        dl.rounded_glow(
            Rect::new(track.x, track.y, fw, track.h),
            color,
            bar_h * 0.5,
            0.20,
        );
    }
}
