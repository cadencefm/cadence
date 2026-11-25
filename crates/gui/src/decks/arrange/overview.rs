//! Multi-lane evaluated pitch-band overview for the GPU Arrange deck.
use super::{draw_vertical_time_grid, TimelineGeom};
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::Theme;
use cadence_app::view::{ArrangeDeckView, ArrangeEvaluatedView};

pub(super) fn draw_overview(
    dl: &mut DisplayList,
    body: Rect,
    view: &ArrangeDeckView,
    eval: &ArrangeEvaluatedView,
    geom: TimelineGeom,
    theme: &Theme,
) {
    if view.lanes.is_empty() {
        return;
    }
    let row_h = (body.h / view.lanes.len() as f32).clamp(24.0, 72.0);
    let visible = (body.h / row_h).floor().max(1.0) as usize;
    let sel_row = view.lanes.iter().position(|l| l.selected).unwrap_or(0);
    let first = sel_row.saturating_sub(visible / 2);

    let cx0 = geom.step_to_x(f64::from(view.cursor_step));
    let cx1 = geom.step_to_x(f64::from(view.cursor_step + 1));

    for (vis_idx, lane) in view.lanes.iter().skip(first).take(visible).enumerate() {
        let y = body.y + vis_idx as f32 * row_h;
        if y >= body.bottom() {
            break;
        }
        let lane_col = theme.track_color(lane.index);
        if lane.selected {
            dl.fill(
                Rect::new(body.x, y, body.w, row_h),
                theme.surface.active_row,
            );
        } else if vis_idx % 2 == 1 {
            dl.fill(
                Rect::new(body.x, y, body.w, row_h),
                theme.surface.recessed.with_alpha(0.32),
            );
        }
        dl.fill(
            Rect::new(body.x, y + row_h - 1.0, body.w, 1.0),
            theme.border.subtle,
        );
        dl.fill(
            Rect::new(geom.timeline_x - 3.0, y, 1.0, row_h),
            theme.border.hairline,
        );

        let live_badge_w = if lane.override_info.is_some() {
            34.0
        } else {
            0.0
        };
        let name_w = (geom.gutter_w - 14.0 - live_badge_w).max(20.0);
        let max_name = (name_w / theme.char_width(theme.font.small)) as usize;
        dl.text(
            Rect::new(body.x + 8.0, y, name_w, row_h),
            trim_label(&lane.name, max_name),
            theme.font.small,
            lane.selected,
            if lane.selected {
                lane_col
            } else if lane.muted {
                theme.text.muted
            } else {
                theme.text.primary
            },
            TextAlign::Left,
        );
        if lane.override_info.is_some() {
            let lr = Rect::new(
                body.x + geom.gutter_w - 36.0,
                y + (row_h - 15.0) * 0.5,
                30.0,
                15.0,
            );
            dl.panel(
                lr,
                theme.accent.override_bar.with_alpha(0.22),
                theme.accent.override_bar,
                theme.radii.badge,
                1.0,
                theme.glow.override_bar * 0.45,
            );
            dl.text(
                lr,
                "LIVE",
                theme.font.small - 2.0,
                true,
                theme.accent.override_bar,
                TextAlign::Center,
            );
        }

        let lane_notes: Vec<_> = eval.notes.iter().filter(|n| n.lane_id == lane.id).collect();
        let lo = lane_notes
            .iter()
            .map(|n| n.midi)
            .min()
            .unwrap_or(48)
            .saturating_sub(2);
        let hi = lane_notes
            .iter()
            .map(|n| n.midi)
            .max()
            .unwrap_or(72)
            .saturating_add(2);
        let pitch_span = f32::from((hi - lo).max(4));

        let row_tl = Rect::new(geom.timeline_x, y, geom.timeline_w, row_h);
        dl.push_clip(row_tl);
        draw_vertical_time_grid(dl, row_tl, view, geom, theme);

        // Subtle cursor step highlight on selected lane (full-height edit cursor line is drawn in arrange/mod.rs).
        if lane.selected && cx1 > geom.timeline_x && cx0 < body.right() {
            dl.fill(
                Rect::new(cx0, y + 1.0, (cx1 - cx0).max(2.0), (row_h - 2.0).max(1.0)),
                theme.accent.focus.with_alpha(0.14),
            );
        }

        // Subtle live override band across overridden span.
        if let Some(ov) = &lane.override_info {
            let ox0 = geom.step_to_x(ov.from_step as f64).max(geom.timeline_x);
            let ox1 = geom.step_to_x(ov.end_step as f64).min(body.right());
            if ox1 > ox0 + 1.0 {
                let band = Rect::new(ox0, y + 1.0, ox1 - ox0, (row_h - 3.0).max(4.0));
                dl.fill(band, theme.accent.override_bar.with_alpha(0.09));
                dl.rounded_glow(
                    Rect::new(band.x, band.y, band.w, 1.5),
                    theme.accent.override_bar.with_alpha(0.85),
                    0.5,
                    theme.glow.override_bar * 0.60,
                );
                dl.rounded_glow(
                    Rect::new(band.x, band.bottom() - 1.5, band.w, 1.5),
                    theme.accent.override_bar.with_alpha(0.85),
                    0.5,
                    theme.glow.override_bar * 0.60,
                );
            }
        }

        // Draw static arrangement notes first, then live override notes on top.
        for draw_live_pass in [false, true] {
            for note in lane_notes
                .iter()
                .copied()
                .filter(|n| n.live_override == draw_live_pass)
            {
                let onset = f64::from(note.step) + f64::from(note.offset);
                let x0 = geom.step_to_x(onset);
                let x1 = geom.step_to_x(onset + f64::from(note.gate.max(0.18)));
                if x1 <= geom.timeline_x || x0 >= body.right() {
                    continue;
                }
                let norm_y = 1.0 - f32::from(note.midi.saturating_sub(lo)) / pitch_span;
                let nh = (row_h / (pitch_span * 0.55).max(3.0)).clamp(3.0, 8.0);
                let ny = y + 3.0 + norm_y * (row_h - nh - 6.0).max(0.0);
                let displaced = !note.live_override
                    && lane.override_info.as_ref().is_some_and(|ov| {
                        let s = u64::from(note.step);
                        s >= ov.from_step && s < ov.end_step
                    });
                let base = if note.live_override {
                    theme.accent.override_bar
                } else if note.local_exception {
                    theme.accent.modulation
                } else {
                    lane_col
                };
                let mut vel_col = base.scale_rgb(0.50 + 0.50 * note.velocity.clamp(0.0, 1.0));
                if displaced {
                    vel_col = vel_col.with_alpha(0.28);
                } else if note.channel_muted {
                    vel_col = vel_col.with_alpha(0.50);
                }
                let sounding = !displaced
                    && view.playing
                    && view
                        .playhead_step
                        .is_some_and(|ph| ph >= onset && ph <= onset + f64::from(note.gate));
                let nr = Rect::new(x0, ny, (x1 - x0).max(2.5), nh);
                if note.selected {
                    dl.panel(nr, vel_col, theme.accent.warning_key, 2.0, 1.25, 0.45);
                } else {
                    let glow = if sounding {
                        theme.glow.active_note
                    } else if note.live_override {
                        theme.glow.override_bar * 0.45
                    } else {
                        0.0
                    };
                    dl.rounded_glow(
                        nr,
                        if sounding {
                            base.scale_rgb(1.25)
                        } else {
                            vel_col
                        },
                        2.0,
                        glow,
                    );
                }
            }
        }
        dl.pop_clip();
    }
}
