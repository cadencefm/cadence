//! Shared pure-theme GPU visualizer card, header, badge, log-frequency grid,
//! and telemetry meter primitives (SPOT).
//!
//! Composed by both [`crate::decks::effect`] and [`crate::decks::instrument`] so
//! studio visualizer chrome stays visually consistent across insert FX and
//! Eurorack Rack modules with zero duplicated layout/grid code and zero hardcoded
//! RGBA outside [`crate::theme`].
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::{Color, Theme};
use cadence_core::dsp::log_to_norm;

/// Split a visualizer row into a primary left card and an optional right-side
/// telemetry/scope card separated by a crisp 6 px gutter when `area.w >= min_total_w`.
#[inline]
pub(crate) fn split_side_card(area: Rect, min_total_w: f32, side_w: f32) -> (Rect, Option<Rect>) {
    if area.w >= min_total_w {
        let (left, right) = area.split_right(side_w);
        (
            Rect::new(left.x, left.y, (left.w - 6.0).max(0.0), left.h),
            Some(right),
        )
    } else {
        (area, None)
    }
}

/// Draw a standard recessed visualizer card container and return its padded inner rect.
#[inline]
pub(crate) fn draw_card_panel(dl: &mut DisplayList, rect: Rect, theme: &Theme) -> Rect {
    dl.panel(
        rect,
        theme.surface.recessed,
        theme.border.subtle,
        theme.radii.pill,
        1.0,
        0.0,
    );
    let pad_y = if rect.h < 96.0 { 4.0 } else { 5.0 };
    rect.inset(8.0, pad_y)
}

/// Draw a clip-safe visualizer card header with a left-aligned bold title and optional
/// right-aligned summary label (strictly styled from `&Theme`, zero text overlap).
pub(crate) fn draw_card_header(
    dl: &mut DisplayList,
    hdr: Rect,
    title: &str,
    right_summary: Option<&str>,
    title_col: Color,
    right_bold: bool,
    theme: &Theme,
) {
    if hdr.is_empty() {
        return;
    }
    dl.push_clip(hdr);
    let font = theme.font.small - 1.0;
    let cw = theme.char_width(font);
    let natural_title_w = (title.chars().count() as f32 * cw + 6.0).min(hdr.w);
    let right_w = right_summary
        .filter(|s| !s.is_empty())
        .map(|s| {
            let max_r = (hdr.w - natural_title_w.min(hdr.w * 0.52) - 6.0).max(0.0);
            (s.chars().count() as f32 * cw + 6.0).min(max_r)
        })
        .unwrap_or(0.0);
    let title_w = (hdr.w - right_w - if right_w > 0.0 { 6.0 } else { 0.0 }).max(0.0);
    if title_w >= 8.0 {
        let max_t = (title_w / cw).floor().max(1.0) as usize;
        dl.text(
            Rect::new(hdr.x, hdr.y, title_w, hdr.h),
            trim_label(title, max_t),
            font,
            true,
            title_col,
            TextAlign::Left,
        );
    }
    if let Some(summary) = right_summary.filter(|s| !s.is_empty()) {
        if right_w >= 28.0 {
            let max_r = (right_w / cw).floor().max(2.0) as usize;
            dl.text(
                Rect::new(hdr.right() - right_w, hdr.y, right_w, hdr.h),
                trim_label(summary, max_r),
                font,
                right_bold,
                theme.text.secondary,
                TextAlign::Right,
            );
        }
    }
    dl.pop_clip();
}

/// Draw a clip-safe visualizer card header featuring 1..N semantic badge pills on the left,
/// followed by the card title and an optional right-aligned summary label.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_badge_header(
    dl: &mut DisplayList,
    hdr: Rect,
    badges: &[(&str, Color)],
    title: &str,
    right_summary: Option<&str>,
    title_color: Color,
    bypassed: bool,
    theme: &Theme,
) {
    if hdr.is_empty() {
        return;
    }
    dl.push_clip(hdr);
    let bfont = theme.font.small - 1.5;
    let bcw = theme.char_width(bfont);
    let bh = (hdr.h - 2.0).clamp(12.0, 16.0);
    let by = hdr.y + ((hdr.h - bh) * 0.5).max(0.0);

    let mut x = hdr.x;
    for &(text, col) in badges {
        if text.is_empty() {
            continue;
        }
        let bw = (text.chars().count() as f32 * bcw + 10.0).min((hdr.right() - x).max(0.0));
        if bw < 20.0 || x + bw > hdr.right() - 28.0 {
            break;
        }
        let br = Rect::new(x, by, bw, bh);
        dl.panel(
            br,
            col.with_alpha(if bypassed { 0.10 } else { 0.18 }),
            col.with_alpha(if bypassed { 0.45 } else { 0.82 }),
            theme.radii.badge,
            1.0,
            if bypassed { 0.0 } else { 0.12 },
        );
        dl.text(br, text, bfont, true, col, TextAlign::Center);
        x += bw + 6.0;
    }

    let rest_w = (hdr.right() - x).max(0.0);
    if rest_w >= 24.0 {
        let show_right = right_summary.filter(|s| !s.is_empty() && rest_w >= 96.0);
        draw_card_header(
            dl,
            Rect::new(x, hdr.y, rest_w, hdr.h),
            title,
            show_right,
            title_color,
            false,
            theme,
        );
    }
    dl.pop_clip();
}

/// Draw vertical logarithmic frequency reference lines across `[20 Hz .. freq_high_hz]`
/// via `cadence_core::dsp::log_to_norm`.
pub(crate) fn draw_log_freq_grid(
    dl: &mut DisplayList,
    plot: Rect,
    freq_high_hz: f32,
    include_minor: bool,
    theme: &Theme,
) {
    let high_hz = freq_high_hz.clamp(2_000.0, 48_000.0);
    let freqs: &[f32] = if include_minor {
        &[50.0, 100.0, 500.0, 1_000.0, 5_000.0, 10_000.0]
    } else {
        &[100.0, 1_000.0, 10_000.0]
    };
    for &hz in freqs {
        if hz >= high_hz * 0.96 {
            continue;
        }
        let x = plot.x + plot.w * log_to_norm(hz, 20.0, high_hz);
        let major = include_minor && (hz == 100.0 || hz == 1_000.0 || hz == 10_000.0);
        dl.fill(
            Rect::new(x, plot.y, 1.0, plot.h),
            if major {
                theme.border.grid_beat
            } else {
                theme.border.grid_step
            },
        );
    }
}

/// Draw logarithmic frequency reference gridlines plus formatted x-axis frequency tick labels
/// across `[20 Hz .. freq_high_hz]`.
pub(crate) fn draw_freq_ticks_and_grid(
    dl: &mut DisplayList,
    plot: Rect,
    x_axis: Rect,
    freq_high_hz: f32,
    theme: &Theme,
) {
    let high_hz = freq_high_hz.clamp(2_000.0, 48_000.0);
    draw_log_freq_grid(dl, plot, high_hz, true, theme);
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
        let nx = log_to_norm(hz, 20.0, high_hz);
        let rx = match align {
            TextAlign::Left => Rect::new(plot.x, x_axis.y, 38.0, x_axis.h),
            TextAlign::Right => Rect::new(plot.right() - 38.0, x_axis.y, 38.0, x_axis.h),
            TextAlign::Center => Rect::new(plot.x + plot.w * nx - 20.0, x_axis.y, 40.0, x_axis.h),
        };
        dl.text(rx, label, font_sz, false, theme.text.muted, align);
    }
}

/// Draw a horizontal `0 dB` reference line inside `plot` for vertical dB `bounds = [min, max]`
/// (with 3 px top/bottom plot inset matching `build_curve_pts`) and return its `y` coordinate.
#[inline]
pub(crate) fn draw_zero_db_line(
    dl: &mut DisplayList,
    plot: Rect,
    bounds: [f32; 2],
    theme: &Theme,
) -> f32 {
    let span = (bounds[1] - bounds[0]).max(1e-4);
    let zero_norm = ((0.0 - bounds[0]) / span).clamp(0.0, 1.0);
    let zero_y = plot.bottom() - 3.0 - zero_norm * (plot.h - 6.0).max(1.0);
    dl.fill(
        Rect::new(plot.x, zero_y, plot.w, 1.0),
        theme.border.grid_beat,
    );
    zero_y
}

/// Draw a stacked label/value readout above a glowing horizontal normalized meter bar.
pub(crate) fn draw_meter_bar_row(
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

/// Draw a stacked label/value readout above a centered bipolar (`-1..+1`) horizontal meter bar.
pub(crate) fn draw_bipolar_meter_bar_row(
    dl: &mut DisplayList,
    rect: Rect,
    label: &str,
    value_text: &str,
    bipolar: f32,
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
    let mid_x = track.x + track.w * 0.5;
    dl.fill(
        Rect::new(mid_x - 0.5, track.y - 1.0, 1.0, track.h + 2.0),
        theme.border.grid_bar,
    );
    let v = bipolar.clamp(-1.0, 1.0);
    let px = track.x + track.w * ((v + 1.0) * 0.5);
    let x0 = mid_x.min(px);
    let x1 = mid_x.max(px);
    if x1 - x0 > 0.5 {
        dl.rounded_glow(
            Rect::new(x0, track.y, x1 - x0, track.h),
            color,
            bar_h * 0.5,
            0.20,
        );
    }
}
