//! Pure GPU curve, envelope, sampler peak, modal spectrum, and Out telemetry plot painters
//! composed by `decks::instrument::visuals`.
use super::super::{draw_badge_header, draw_card_panel};
use crate::decks::vis::{draw_log_freq_grid as vis_log_freq_grid, draw_zero_db_line};
use crate::display::{build_curve_pts, sample_curve_y, trim_label, DisplayList, Rect, TextAlign};
use crate::theme::{Color, Theme};
use cadence_core::params::format_pan;
use cadence_core::sampler::SampleLoopMode;

/// Subtle log-frequency vertical reference lines (100 Hz, 1 kHz, 10 kHz) via shared `decks::vis` SPOT.
fn draw_log_freq_grid(dl: &mut DisplayList, plot: Rect, theme: &Theme) {
    vis_log_freq_grid(dl, plot, 20_000.0, false, theme);
}

pub(super) fn draw_wave_plot(
    dl: &mut DisplayList,
    plot: Rect,
    values: &[f32],
    bounds: [f32; 2],
    color: Color,
    active: bool,
    theme: &Theme,
) {
    if values.len() < 2 || plot.is_empty() {
        return;
    }
    dl.push_clip(plot);
    let span = (bounds[1] - bounds[0]).max(1e-4);
    let zero_norm = ((0.0 - bounds[0]) / span).clamp(0.0, 1.0);
    let zero_y = plot.bottom() - zero_norm * plot.h;
    dl.fill(
        Rect::new(plot.x, zero_y, plot.w, 1.0),
        theme.border.grid_step,
    );

    let pts = build_curve_pts(values, bounds, plot);
    dl.polyline(
        &pts,
        1.8,
        color,
        if active { theme.glow.scope * 0.75 } else { 0.0 },
        Some((zero_y, if active { 0.16 } else { 0.04 })),
    );
    dl.pop_clip();
}

pub(super) fn draw_filter_plot(
    dl: &mut DisplayList,
    plot: Rect,
    values: &[f32],
    cutoff_norm: f32,
    mod_cutoff_norm: Option<f32>,
    show_grid: bool,
    theme: &Theme,
) {
    if values.len() < 2 || plot.is_empty() {
        return;
    }
    let bounds = [-48.0_f32, 18.0_f32];
    dl.push_clip(plot);

    if show_grid {
        draw_log_freq_grid(dl, plot, theme);
    }
    let _ = draw_zero_db_line(dl, plot, bounds, theme);

    let pts = build_curve_pts(values, bounds, plot);
    dl.polyline(
        &pts,
        1.85,
        theme.accent.warning_key,
        theme.glow.scope * 0.75,
        Some((plot.bottom() - 2.0, 0.14)),
    );

    let base_x = plot.x + plot.w * cutoff_norm.clamp(0.0, 1.0);
    let base_y = sample_curve_y(values, cutoff_norm, bounds, plot);

    if let Some(mod_norm) = mod_cutoff_norm {
        let mod_x = plot.x + plot.w * mod_norm.clamp(0.0, 1.0);
        let mod_y = sample_curve_y(values, mod_norm, bounds, plot);
        let x0 = base_x.min(mod_x);
        let x1 = base_x.max(mod_x);
        if x1 - x0 >= 1.5 {
            dl.fill(
                Rect::new(x0, plot.y, x1 - x0, plot.h),
                theme.accent.modulation.with_alpha(0.16),
            );
        }
        dl.fill(
            Rect::new(mod_x - 0.5, plot.y, 1.2, plot.h),
            theme.accent.modulation.with_alpha(0.70),
        );
        dl.rounded_glow(
            Rect::new(mod_x - 4.0, mod_y - 4.0, 8.0, 8.0),
            theme.accent.modulation,
            4.0,
            0.55,
        );
    }

    dl.fill(
        Rect::new(base_x - 0.5, plot.y, 1.0, plot.h),
        theme.accent.warning_key.with_alpha(0.45),
    );
    dl.rounded_glow(
        Rect::new(base_x - 3.0, base_y - 3.0, 6.0, 6.0),
        theme.accent.warning_key,
        3.0,
        0.35,
    );
    dl.pop_clip();
}

pub(super) fn draw_modal_spectrum_plot(
    dl: &mut DisplayList,
    plot: Rect,
    mode_bars: &[(f32, f32)],
    brightness: f32,
    absorption: f32,
    accent: Color,
    theme: &Theme,
) {
    if plot.is_empty() {
        return;
    }
    dl.push_clip(plot);
    draw_log_freq_grid(dl, plot, theme);
    let baseline = plot.bottom() - 4.0;
    let usable_h = (plot.h - 10.0).max(4.0);
    dl.fill(
        Rect::new(plot.x, baseline, plot.w, 1.0),
        theme.border.grid_beat,
    );

    if !mode_bars.is_empty() {
        let count = mode_bars.len();
        let (density_scale, max_bar_w) = if count > 24 {
            (4.2, 2.8)
        } else if count > 16 {
            (3.4, 3.8)
        } else {
            (2.8, 5.5)
        };
        let bar_w = (plot.w / (count.max(8) as f32 * density_scale)).clamp(1.5, max_bar_w);
        let half_w = bar_w * 0.5;
        let cap_r = (bar_w * 0.55).clamp(1.2, 2.2);
        let cap_d = cap_r * 2.0;
        let damp_norm = (absorption / 0.5).clamp(0.0, 1.0);
        let bright_norm = brightness.clamp(0.0, 1.0);
        let high_col = accent.mix(
            theme.accent.focus,
            (0.25 + 0.50 * bright_norm - 0.25 * damp_norm).clamp(0.0, 0.85),
        );
        let mut env_pts: Vec<(f32, f32)> = Vec::with_capacity(count);
        for (idx, &(freq_norm, amp_norm)) in mode_bars.iter().enumerate() {
            let x = plot.x + 4.0 + freq_norm.clamp(0.0, 1.0) * (plot.w - 8.0).max(1.0);
            let h = (amp_norm.clamp(0.0, 1.0) * usable_h).max(2.0);
            let top_y = baseline - h;
            let t = if count > 1 {
                idx as f32 / (count - 1) as f32
            } else {
                0.0
            };
            let bar_col = accent.mix(high_col, t);
            dl.rounded(
                Rect::new(x - half_w, top_y, bar_w, h),
                bar_col.with_alpha(0.78),
                (bar_w * 0.35).min(1.5),
            );
            dl.rounded_glow(
                Rect::new(x - cap_r, top_y - cap_r, cap_d, cap_d),
                bar_col,
                cap_r,
                0.35,
            );
            env_pts.push((x, top_y));
        }
        if env_pts.len() >= 2 {
            env_pts.sort_by(|a, b| a.0.total_cmp(&b.0));
            dl.polyline(
                &env_pts,
                1.15,
                accent.with_alpha(0.42),
                theme.glow.scope * 0.32,
                Some((baseline, 0.08)),
            );
        }
    }
    dl.pop_clip();
}

pub(super) fn draw_noise_band_plot(
    dl: &mut DisplayList,
    plot: Rect,
    band_curve: &[f32],
    highpass_norm: f32,
    lowpass_norm: f32,
    color: Color,
    theme: &Theme,
) {
    if band_curve.len() < 2 || plot.is_empty() {
        return;
    }
    let bounds = [-48.0_f32, 12.0_f32];
    dl.push_clip(plot);

    draw_log_freq_grid(dl, plot, theme);
    let hp_x = plot.x + plot.w * highpass_norm.clamp(0.0, 1.0);
    let lp_x = plot.x + plot.w * lowpass_norm.clamp(0.0, 1.0);
    if lp_x > hp_x + 1.0 {
        dl.fill(
            Rect::new(hp_x, plot.y, lp_x - hp_x, plot.h),
            color.with_alpha(0.08),
        );
    }

    let _ = draw_zero_db_line(dl, plot, bounds, theme);

    let pts = build_curve_pts(band_curve, bounds, plot);
    dl.polyline(
        &pts,
        1.85,
        color,
        theme.glow.scope * 0.65,
        Some((plot.bottom() - 2.0, 0.14)),
    );

    let hp_y = sample_curve_y(band_curve, highpass_norm, bounds, plot);
    let lp_y = sample_curve_y(band_curve, lowpass_norm, bounds, plot);
    dl.fill(
        Rect::new(hp_x - 0.5, plot.y, 1.0, plot.h),
        theme.accent.focus.with_alpha(0.65),
    );
    dl.rounded_glow(
        Rect::new(hp_x - 3.0, hp_y - 3.0, 6.0, 6.0),
        theme.accent.focus,
        3.0,
        0.35,
    );
    dl.fill(
        Rect::new(lp_x - 0.5, plot.y, 1.0, plot.h),
        theme.accent.warning_key.with_alpha(0.65),
    );
    dl.rounded_glow(
        Rect::new(lp_x - 3.0, lp_y - 3.0, 6.0, 6.0),
        theme.accent.warning_key,
        3.0,
        0.35,
    );
    dl.pop_clip();
}

pub(super) fn draw_env_plot(dl: &mut DisplayList, plot: Rect, curve: &[(f32, f32)], theme: &Theme) {
    if curve.len() < 2 || plot.is_empty() {
        return;
    }
    dl.push_clip(plot);
    let base_y = plot.bottom() - 4.0;
    let mid_y = plot.y + plot.h * 0.5;
    dl.fill(
        Rect::new(plot.x, mid_y, plot.w, 1.0),
        theme.border.grid_step,
    );
    dl.fill(
        Rect::new(plot.x, base_y, plot.w, 1.0),
        theme.border.grid_beat,
    );

    let pts: Vec<(f32, f32)> = curve
        .iter()
        .map(|&(nx, ny)| {
            let x = plot.x + 4.0 + nx.clamp(0.0, 1.0) * (plot.w - 8.0).max(1.0);
            let y = base_y - ny.clamp(0.0, 1.0) * (plot.h - 8.0).max(1.0);
            (x, y)
        })
        .collect();
    dl.polyline(
        &pts,
        1.9,
        theme.accent.playhead,
        theme.glow.scope * 0.72,
        Some((base_y, 0.18)),
    );
    // Only highlight sparse piecewise breakpoints; dense exponential curves stay clean & silky.
    if pts.len() <= 8 {
        for &(px, py) in &pts {
            dl.rounded_glow(
                Rect::new(px - 2.5, py - 2.5, 5.0, 5.0),
                theme.accent.playhead,
                2.5,
                0.35,
            );
        }
    }
    dl.pop_clip();
}

pub(super) struct SamplerPlotSpec<'a> {
    pub peaks: &'a [(f32, f32)],
    pub sample_start: f32,
    pub sample_end: f32,
    pub loop_mode: SampleLoopMode,
    pub loop_start: f32,
    pub loop_end: f32,
}

pub(super) fn draw_sampler_plot(
    dl: &mut DisplayList,
    plot: Rect,
    spec: &SamplerPlotSpec<'_>,
    theme: &Theme,
) {
    if plot.is_empty() {
        return;
    }
    let SamplerPlotSpec {
        peaks,
        sample_start,
        sample_end,
        loop_mode,
        loop_start,
        loop_end,
    } = *spec;

    dl.push_clip(plot);
    dl.fill(
        Rect::new(plot.x, plot.y + plot.h * 0.5, plot.w, 1.0),
        theme.border.grid_step,
    );
    if peaks.is_empty() {
        dl.text(
            plot,
            "No audio loaded · press B to browse samples",
            theme.font.small - 1.0,
            false,
            theme.text.muted,
            TextAlign::Center,
        );
        dl.pop_clip();
        return;
    }

    dl.peak_envelope(plot, peaks, theme.accent.focus, theme.glow.scope * 0.55);

    let sx = plot.x + plot.w * sample_start.clamp(0.0, 1.0);
    let ex = plot.x + plot.w * sample_end.clamp(0.0, 1.0);
    if sx > plot.x {
        dl.fill(
            Rect::new(plot.x, plot.y, sx - plot.x, plot.h),
            theme.surface.canvas.with_alpha(0.62),
        );
        dl.fill(
            Rect::new(sx, plot.y, 1.2, plot.h),
            theme.accent.warning_key.with_alpha(0.85),
        );
    }
    if ex < plot.right() {
        dl.fill(
            Rect::new(ex, plot.y, plot.right() - ex, plot.h),
            theme.surface.canvas.with_alpha(0.62),
        );
        dl.fill(
            Rect::new(ex - 1.2, plot.y, 1.2, plot.h),
            theme.accent.warning_key.with_alpha(0.85),
        );
    }
    if loop_mode != SampleLoopMode::Off {
        let lx0 = plot.x + plot.w * loop_start.clamp(0.0, 1.0);
        let lx1 = plot.x + plot.w * loop_end.clamp(0.0, 1.0);
        if lx1 > lx0 {
            dl.panel(
                Rect::new(lx0, plot.y, lx1 - lx0, plot.h),
                theme.accent.playhead.with_alpha(0.12),
                theme.accent.playhead.with_alpha(0.75),
                0.0,
                1.0,
                0.18,
            );
        }
    }
    dl.pop_clip();
}

pub(super) fn draw_bipolar_transfer_plot(
    dl: &mut DisplayList,
    plot: Rect,
    transfer_curve: &[f32],
    y_bounds: [f32; 2],
    color: Color,
    theme: &Theme,
) {
    if transfer_curve.len() < 2 || plot.is_empty() {
        return;
    }
    dl.push_clip(plot);
    let span = (y_bounds[1] - y_bounds[0]).max(1e-4);
    let px_of = |x: f32| {
        let nx = ((x + 1.0) * 0.5).clamp(0.0, 1.0);
        plot.x + 4.0 + nx * (plot.w - 8.0).max(1.0)
    };
    let py_of = |y: f32| {
        let ny = ((y - y_bounds[0]) / span).clamp(0.0, 1.0);
        plot.bottom() - 4.0 - ny * (plot.h - 8.0).max(1.0)
    };

    // Grid axes at x=0, y=0, y=±1 and unity diagonal y = x.
    dl.fill(
        Rect::new(plot.x, py_of(1.0), plot.w, 1.0),
        theme.border.grid_step,
    );
    dl.fill(
        Rect::new(plot.x, py_of(-1.0), plot.w, 1.0),
        theme.border.grid_step,
    );
    dl.fill(
        Rect::new(plot.x, py_of(0.0), plot.w, 1.0),
        theme.border.grid_beat,
    );
    dl.fill(
        Rect::new(px_of(0.0), plot.y, 1.0, plot.h),
        theme.border.grid_beat,
    );
    dl.polyline(
        &[(px_of(-1.0), py_of(-1.0)), (px_of(1.0), py_of(1.0))],
        1.1,
        theme.border.grid_bar,
        0.0,
        None,
    );

    let n = transfer_curve.len();
    let pts: Vec<(f32, f32)> = transfer_curve
        .iter()
        .enumerate()
        .map(|(i, &y)| {
            let x = (i as f32 / (n - 1) as f32) * 2.0 - 1.0;
            (px_of(x), py_of(y))
        })
        .collect();
    dl.polyline(
        &pts,
        2.0,
        color,
        theme.glow.scope * 0.80,
        Some((py_of(0.0), 0.15)),
    );
    dl.pop_clip();
}

pub(super) fn draw_vca_transfer_plot(
    dl: &mut DisplayList,
    plot: Rect,
    transfer_curve: &[f32],
    ring_mod: bool,
    color: Color,
    theme: &Theme,
) {
    if transfer_curve.len() < 2 || plot.is_empty() {
        return;
    }
    let bounds: [f32; 2] = if ring_mod { [-2.1, 2.1] } else { [-0.1, 2.1] };
    let span = (bounds[1] - bounds[0]).max(1e-4);
    let py_of = |y: f32| {
        let ny = ((y - bounds[0]) / span).clamp(0.0, 1.0);
        plot.bottom() - 4.0 - ny * (plot.h - 8.0).max(1.0)
    };
    dl.push_clip(plot);
    // Unity gain + zero reference lines.
    dl.fill(
        Rect::new(plot.x, py_of(1.0), plot.w, 1.0),
        theme.border.grid_step,
    );
    if ring_mod {
        dl.fill(
            Rect::new(plot.x, py_of(-1.0), plot.w, 1.0),
            theme.border.grid_step,
        );
        dl.fill(
            Rect::new(plot.x + plot.w * 0.5, plot.y, 1.0, plot.h),
            theme.border.grid_beat,
        );
    }
    let zero_y = py_of(0.0);
    dl.fill(
        Rect::new(plot.x, zero_y, plot.w, 1.0),
        theme.border.grid_beat,
    );

    let n = transfer_curve.len();
    let pts: Vec<(f32, f32)> = transfer_curve
        .iter()
        .enumerate()
        .map(|(i, &y)| {
            let x = plot.x + 4.0 + (i as f32 / (n - 1) as f32) * (plot.w - 8.0).max(1.0);
            (x, py_of(y))
        })
        .collect();
    dl.polyline(
        &pts,
        2.0,
        color,
        theme.glow.scope * 0.75,
        Some((zero_y, 0.16)),
    );
    dl.pop_clip();
}

pub(super) struct OutSummarySpec<'a> {
    pub label: &'a str,
    pub gain_db: f32,
    pub pan: f32,
    pub spread: f32,
    pub voice_summary: &'a str,
}

pub(super) fn draw_out_summary_card(
    dl: &mut DisplayList,
    card: Rect,
    spec: &OutSummarySpec<'_>,
    theme: &Theme,
) {
    let OutSummarySpec {
        label,
        gain_db,
        pan,
        spread,
        voice_summary,
    } = *spec;

    let inner = draw_card_panel(dl, card, theme);
    let (hdr, body) = inner.split_top(18.0);
    draw_badge_header(
        dl,
        hdr,
        &[("OUT", theme.accent.focus)],
        label,
        None,
        theme.accent.focus,
        theme,
    );
    if body.h < 28.0 {
        return;
    }
    dl.push_clip(body);
    let font = theme.font.small - 1.5;
    let bar_rows = if body.h >= 78.0 { 3 } else { 2 };
    let (bars_area, voice_area) = if body.h >= 66.0 {
        body.split_bottom(22.0)
    } else {
        (body, Rect::default())
    };
    let row_h = (bars_area.h / bar_rows as f32).clamp(15.0, 26.0);
    let bars_total = row_h * bar_rows as f32;
    let bars_y0 = bars_area.y + ((bars_area.h - bars_total) * 0.5).max(0.0);

    // Gain (-36..+12 dB)
    let gain_norm = ((gain_db + 36.0) / 48.0).clamp(0.0, 1.0);
    draw_mini_telemetry_bar(
        dl,
        Rect::new(bars_area.x, bars_y0, bars_area.w, row_h - 2.0),
        "GAIN",
        &format!("{gain_db:+.1} dB"),
        gain_norm,
        theme.accent.focus,
        theme,
    );
    // Pan (-1..+1 centered)
    draw_mini_pan_bar(
        dl,
        Rect::new(bars_area.x, bars_y0 + row_h, bars_area.w, row_h - 2.0),
        &format_pan(pan),
        pan,
        theme,
    );
    // Stereo Spread (0..100%)
    if bar_rows >= 3 {
        draw_mini_telemetry_bar(
            dl,
            Rect::new(bars_area.x, bars_y0 + row_h * 2.0, bars_area.w, row_h - 2.0),
            "SPREAD",
            &format!("{:.0}%", spread * 100.0),
            spread.clamp(0.0, 1.0),
            theme.accent.routing,
            theme,
        );
    }

    if !voice_area.is_empty() && !voice_summary.is_empty() {
        let vr = Rect::new(
            voice_area.x,
            voice_area.y + 3.0,
            voice_area.w,
            (voice_area.h - 3.0).max(14.0),
        );
        dl.panel(
            vr,
            theme.surface.elevated,
            theme.border.subtle,
            theme.radii.badge,
            1.0,
            0.0,
        );
        let cw = theme.char_width(font);
        let max_c = ((vr.w - 10.0) / cw).floor().max(4.0) as usize;
        dl.text(
            vr.inset(5.0, 0.0),
            trim_label(voice_summary, max_c),
            font,
            false,
            theme.text.secondary,
            TextAlign::Left,
        );
    }
    dl.pop_clip();
}

fn draw_mini_telemetry_bar(
    dl: &mut DisplayList,
    rect: Rect,
    label: &str,
    value: &str,
    norm: f32,
    color: Color,
    theme: &Theme,
) {
    if rect.h < 10.0 || rect.w < 60.0 {
        return;
    }
    let font = theme.font.small - 1.5;
    let lbl_w = 46.0;
    let val_w = 54.0;
    dl.text(
        Rect::new(rect.x, rect.y, lbl_w, rect.h),
        label,
        font,
        true,
        theme.text.muted,
        TextAlign::Left,
    );
    dl.text(
        Rect::new(rect.right() - val_w, rect.y, val_w, rect.h),
        value,
        font,
        true,
        theme.text.primary,
        TextAlign::Right,
    );
    let bx = rect.x + lbl_w + 4.0;
    let bw = (rect.w - lbl_w - val_w - 8.0).max(0.0);
    if bw >= 16.0 {
        let track = Rect::new(bx, rect.y + rect.h * 0.5 - 2.5, bw, 5.0);
        dl.rounded(track, theme.meter.track_bg, 2.5);
        let fw = (track.w * norm.clamp(0.0, 1.0)).max(0.0);
        if fw > 0.5 {
            dl.rounded_glow(Rect::new(track.x, track.y, fw, track.h), color, 2.5, 0.16);
        }
    }
}

fn draw_mini_pan_bar(dl: &mut DisplayList, rect: Rect, value: &str, pan: f32, theme: &Theme) {
    if rect.h < 10.0 || rect.w < 60.0 {
        return;
    }
    let font = theme.font.small - 1.5;
    let lbl_w = 46.0;
    let val_w = 54.0;
    dl.text(
        Rect::new(rect.x, rect.y, lbl_w, rect.h),
        "PAN",
        font,
        true,
        theme.text.muted,
        TextAlign::Left,
    );
    dl.text(
        Rect::new(rect.right() - val_w, rect.y, val_w, rect.h),
        value,
        font,
        true,
        theme.text.primary,
        TextAlign::Right,
    );
    let bx = rect.x + lbl_w + 4.0;
    let bw = (rect.w - lbl_w - val_w - 8.0).max(0.0);
    if bw >= 16.0 {
        let track = Rect::new(bx, rect.y + rect.h * 0.5 - 2.5, bw, 5.0);
        dl.rounded(track, theme.meter.track_bg, 2.5);
        let mid_x = track.x + track.w * 0.5;
        dl.fill(
            Rect::new(mid_x - 0.5, track.y - 1.5, 1.0, track.h + 3.0),
            theme.border.grid_bar,
        );
        let px = track.x + track.w * ((pan.clamp(-1.0, 1.0) + 1.0) * 0.5);
        let x0 = mid_x.min(px);
        let x1 = mid_x.max(px);
        if x1 - x0 > 0.5 {
            dl.rounded(
                Rect::new(x0, track.y, x1 - x0, track.h),
                theme.accent.warning_key,
                2.0,
            );
        }
        dl.rounded_glow(
            Rect::new(px - 3.0, track.y - 1.5, 6.0, track.h + 3.0),
            theme.accent.warning_key,
            3.0,
            0.22,
        );
    }
}
