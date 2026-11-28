//! Zero-delay 96-mode ambient modal room RT60 decay, diffusion density, and air-absorption visualizer.
use super::{
    draw_card_header, draw_card_panel, draw_lp_response_card, split_side_card, LpResponseSpec,
};
use crate::display::{DisplayList, Rect, TextAlign};
use crate::theme::Theme;
use cadence_app::view::EffectVisualView;
use cadence_core::params::format_hz;

pub(super) fn draw_reverb_visual(
    dl: &mut DisplayList,
    area: Rect,
    visual: &EffectVisualView,
    bypassed: bool,
    theme: &Theme,
) {
    let EffectVisualView::Reverb {
        freq_high_hz,
        decay_curve,
        hf_decay_curve,
        damping_curve_db,
        damping_norm,
        window_sec,
        size,
        decay_seconds,
        damping_hz,
        diffusion,
        width,
        mix,
        summary,
    } = visual
    else {
        return;
    };
    let (
        freq_high_hz,
        damping_norm,
        window_sec,
        size,
        decay_seconds,
        damping_hz,
        diffusion,
        width,
        mix,
    ) = (
        *freq_high_hz,
        *damping_norm,
        *window_sec,
        *size,
        *decay_seconds,
        *damping_hz,
        *diffusion,
        *width,
        *mix,
    );
    let damp_w = (area.w * 0.38).clamp(170.0, 340.0);
    let (left_card, right_card) = split_side_card(area, damp_w);

    // Left: Modal Room Diffusion Density + Stereo RT60 Decay Tail Envelope.
    let inner = draw_card_panel(dl, left_card, theme);
    let (hdr, body) = inner.split_top(18.0);
    let tail_col = if bypassed {
        theme.text.muted
    } else {
        theme.accent.routing
    };
    let diff_col = if bypassed {
        theme.text.muted
    } else {
        theme.accent.focus
    };
    let hf_col = if bypassed {
        theme.text.muted
    } else {
        theme.accent.modulation
    };
    draw_card_header(dl, hdr, "MODAL ROOM  ·  RT60", summary, tail_col, theme);
    if decay_curve.len() >= 2 && body.h >= 40.0 {
        let (plot_outer, x_axis) = body.split_bottom(14.0);
        let plot = plot_outer.inset(4.0, 4.0);
        if !plot.is_empty() {
            let mid_y = plot.y + plot.h * 0.5;
            let half_h = (plot.h * 0.44).max(1.0);
            let w = width.clamp(0.0, 1.0);
            let diff = diffusion.clamp(0.0, 1.0);
            // Symmetric stereo width envelope: crisp centered mono lobe at width == 0.0,
            // opening symmetrically above and below mid_y as width increases to 1.0.
            let lobe_h = half_h * (0.40 + 0.60 * w);
            let lower_alpha = 1.0 - 0.15 * w;

            // Time axis labels.
            let font_sz = theme.font.small - 2.0;
            dl.text(
                Rect::new(plot.x, x_axis.y, 38.0, x_axis.h),
                "0.0s",
                font_sz,
                false,
                theme.text.muted,
                TextAlign::Left,
            );
            dl.text(
                Rect::new(plot.x + plot.w * 0.5 - 24.0, x_axis.y, 48.0, x_axis.h),
                format!("{:.1}s", window_sec * 0.5),
                font_sz,
                false,
                theme.text.muted,
                TextAlign::Center,
            );
            dl.text(
                Rect::new(plot.right() - 42.0, x_axis.y, 42.0, x_axis.h),
                format!("{window_sec:.1}s"),
                font_sz,
                false,
                theme.text.muted,
                TextAlign::Right,
            );

            dl.push_clip(plot);
            dl.fill(Rect::new(plot.x, mid_y, plot.w, 1.0), theme.border.grid_bar);

            // Subtle RT60 (-60 dB) reference hairline along the decay timeline.
            let rt60_norm = (decay_seconds / window_sec.max(1e-3)).clamp(0.0, 1.0);
            let rt60_x = plot.x + plot.w * rt60_norm;
            dl.fill(
                Rect::new(rt60_x, plot.y, 1.0, plot.h),
                theme.border.grid_step.with_alpha(0.85),
            );

            let n = decay_curve.len();

            // Even/Odd modal diffusion scatter filaments bounded by the full-band RT60 envelope:
            // sparse distinct eigenmode lines at diffusion == 0, densifying into a smooth diffuse
            // modal cloud as diffusion approaches 1.0.
            let filament_count = (8.0 + diff * 28.0).round() as usize;
            let size_phase = size.clamp(0.0, 1.0) * 0.25;
            let golden_step = 0.5 * (5.0_f32.sqrt() - 1.0);
            for k in 0..filament_count {
                let frac = (k as f32 + 0.5) / filament_count as f32;
                // Deterministic golden-ratio jitter for natural acoustic mode clustering.
                let jitter = ((k as f32 * golden_step + size_phase).fract() - 0.5)
                    * (0.65 / filament_count as f32);
                // Early-weighted spacing reflects rapid diffuse onset thickening into the tail.
                let xn = (frac.powf(1.25 - 0.30 * diff) + jitter).clamp(0.01, 0.99);
                let idx = ((xn * (n - 1) as f32).round() as usize).min(n - 1);
                let env = decay_curve[idx].clamp(0.0, 1.0);
                if env <= 0.015 {
                    continue;
                }
                let fx = plot.x + plot.w * xn;
                let h = (env * lobe_h * (0.72 + 0.26 * diff)).max(2.0);
                let is_even = k % 2 == 0;
                let base_col = if is_even { diff_col } else { tail_col };
                let alpha = if bypassed {
                    0.06
                } else {
                    (0.10 + 0.24 * diff) * (0.45 + 0.55 * env)
                };
                let bar_w = 1.0 + 0.8 * diff;
                dl.fill(
                    Rect::new(fx - bar_w * 0.5, mid_y - h, bar_w, h * 2.0),
                    base_col.with_alpha(alpha),
                );
            }

            let upper_pts: Vec<(f32, f32)> = decay_curve
                .iter()
                .enumerate()
                .map(|(i, &a)| {
                    let x = plot.x + plot.w * (i as f32 / (n - 1) as f32);
                    (x, mid_y - a.clamp(0.0, 1.0) * lobe_h)
                })
                .collect();
            let lower_pts: Vec<(f32, f32)> = decay_curve
                .iter()
                .enumerate()
                .map(|(i, &a)| {
                    let x = plot.x + plot.w * (i as f32 / (n - 1) as f32);
                    (x, mid_y + a.clamp(0.0, 1.0) * lobe_h)
                })
                .collect();
            let fill_boost = if bypassed { 0.04 } else { 0.12 + 0.08 * diff };
            dl.polyline(
                &upper_pts,
                1.9,
                tail_col,
                if bypassed {
                    0.0
                } else {
                    theme.glow.scope * (0.60 + 0.25 * diff)
                },
                Some((mid_y, fill_boost)),
            );
            dl.polyline(
                &lower_pts,
                1.9,
                tail_col.with_alpha(lower_alpha),
                if bypassed {
                    0.0
                } else {
                    theme.glow.scope * (0.60 + 0.20 * diff - 0.10 * w)
                },
                Some((mid_y, (fill_boost - 0.02 * w).max(0.02))),
            );

            // Inner HF air-absorption decay envelope overlay.
            if hf_decay_curve.len() == n {
                let hf_up: Vec<(f32, f32)> = hf_decay_curve
                    .iter()
                    .enumerate()
                    .map(|(i, &a)| {
                        let x = plot.x + plot.w * (i as f32 / (n - 1) as f32);
                        (x, mid_y - a.clamp(0.0, 1.0) * lobe_h)
                    })
                    .collect();
                let hf_dn: Vec<(f32, f32)> = hf_decay_curve
                    .iter()
                    .enumerate()
                    .map(|(i, &a)| {
                        let x = plot.x + plot.w * (i as f32 / (n - 1) as f32);
                        (x, mid_y + a.clamp(0.0, 1.0) * lobe_h)
                    })
                    .collect();
                dl.polyline(
                    &hf_up,
                    1.4,
                    hf_col,
                    if bypassed { 0.0 } else { 0.35 },
                    Some((mid_y, if bypassed { 0.02 } else { 0.10 })),
                );
                dl.polyline(
                    &hf_dn,
                    1.4,
                    hf_col.with_alpha(lower_alpha),
                    if bypassed { 0.0 } else { 0.35 - 0.05 * w },
                    Some((mid_y, if bypassed { 0.02 } else { 0.10 - 0.02 * w })),
                );
            }
            dl.pop_clip();
        }
    }

    // Right: Physical Air-Absorption RT60 Curve (20 Hz..freq_high_hz).
    if let Some(rcard) = right_card {
        let damp_summary = format!(
            "{} · Sz {:.0}% · Mix {:.0}%",
            format_hz(damping_hz),
            size * 100.0,
            mix * 100.0
        );
        draw_lp_response_card(
            dl,
            rcard,
            &LpResponseSpec {
                title: "AIR ABSORPTION",
                summary: &damp_summary,
                freq_high_hz,
                curve_db: damping_curve_db,
                cutoff_norm: damping_norm,
                curve_col: hf_col,
                marker_line_col: hf_col.with_alpha(0.65),
                marker_dot_col: hf_col,
                bypassed,
            },
            theme,
        );
    }
}
