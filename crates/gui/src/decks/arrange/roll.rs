//! Single-lane evaluated chromatic/folded note roll for the GPU Arrange deck.
use super::{draw_vertical_time_grid, TimelineGeom};
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::Theme;
use cadence_app::view::{ArrangeDeckView, ArrangeEvaluatedView};
use cadence_core::music::note_name;

pub(super) fn draw_roll(
    dl: &mut DisplayList,
    body: Rect,
    view: &ArrangeDeckView,
    eval: &ArrangeEvaluatedView,
    geom: TimelineGeom,
    theme: &Theme,
) {
    let (lo, hi) = eval.pitch_range;
    let rows = (hi.saturating_sub(lo) as usize + 1).max(1);
    let row_h = (body.h / rows as f32).max(4.0);

    // Pitch rows with subtle scale-degree tinting & note names in gutter.
    for i in 0..rows {
        let midi = hi.saturating_sub(i as u8);
        let y = body.y + i as f32 * row_h;
        if y >= body.bottom() {
            break;
        }
        let pc = midi % 12;
        let bg = if pc == eval.root_pc {
            theme.surface.key_root
        } else if eval.scale_pcs[pc as usize] {
            theme.surface.key_scale
        } else if matches!(pc, 1 | 3 | 6 | 8 | 10) {
            theme.surface.key_black
        } else {
            theme.surface.key_white
        };
        dl.fill(Rect::new(geom.timeline_x, y, geom.timeline_w, row_h), bg);
        dl.fill(
            Rect::new(geom.timeline_x, y + row_h - 1.0, geom.timeline_w, 1.0),
            theme.border.grid_step.with_alpha(0.65),
        );
        if row_h >= 12.0 || pc == eval.root_pc || pc == 0 {
            dl.text(
                Rect::new(body.x + 4.0, y, geom.gutter_w - 8.0, row_h.max(13.0)),
                note_name(midi),
                (theme.font.small - 1.0).min(row_h * 0.85).max(8.5),
                pc == eval.root_pc,
                if pc == eval.root_pc {
                    theme.accent.focus
                } else {
                    theme.text.muted
                },
                TextAlign::Right,
            );
        }
    }

    dl.push_clip(Rect::new(geom.timeline_x, body.y, geom.timeline_w, body.h));
    draw_vertical_time_grid(dl, body, view, geom, theme);

    // Subtle edit cursor step column wash (full-height edit cursor line is drawn in arrange/mod.rs).
    let cx0 = geom.step_to_x(f64::from(view.cursor_step));
    let cx1 = geom.step_to_x(f64::from(view.cursor_step + 1));
    if cx1 > geom.timeline_x && cx0 < body.right() {
        dl.fill(
            Rect::new(cx0, body.y, (cx1 - cx0).max(2.0), body.h),
            theme.accent.focus.with_alpha(0.10),
        );
    }

    // Subtle live override background wash & top/bottom rails for the selected lane.
    let selected_ov = view
        .lanes
        .iter()
        .find(|l| l.selected)
        .and_then(|l| l.override_info.as_ref());
    if let Some(ov) = selected_ov {
        let ox0 = geom.step_to_x(ov.from_step as f64).max(geom.timeline_x);
        let ox1 = geom.step_to_x(ov.end_step as f64).min(body.right());
        if ox1 > ox0 + 2.0 {
            let band = Rect::new(ox0, body.y, ox1 - ox0, body.h);
            dl.fill(band, theme.accent.override_bar.with_alpha(0.06));
            dl.rounded_glow(
                Rect::new(band.x, band.y, band.w, 2.0),
                theme.accent.override_bar,
                0.5,
                theme.glow.override_bar * 0.70,
            );
            dl.rounded_glow(
                Rect::new(band.x, band.bottom() - 2.0, band.w, 2.0),
                theme.accent.override_bar,
                0.5,
                theme.glow.override_bar * 0.70,
            );
        }
    }

    // Draw arranged notes first, then live override notes cleanly on top.
    for draw_live_pass in [false, true] {
        for note in eval
            .notes
            .iter()
            .filter(|n| n.live_override == draw_live_pass)
        {
            if note.midi < lo || note.midi > hi {
                continue;
            }
            let onset = f64::from(note.step) + f64::from(note.offset);
            let x0 = geom.step_to_x(onset);
            let x1 = geom.step_to_x(onset + f64::from(note.gate.max(0.15)));
            if x1 <= geom.timeline_x || x0 >= body.right() {
                continue;
            }
            let row_idx = hi.saturating_sub(note.midi) as f32;
            let ny = body.y + row_idx * row_h + 1.0;
            let nr = Rect::new(
                x0 + 0.5,
                ny,
                (x1 - x0 - 0.5).max(3.0),
                (row_h - 2.0).max(3.0),
            );
            let displaced = !note.live_override
                && selected_ov.is_some_and(|ov| {
                    let s = u64::from(note.step);
                    s >= ov.from_step && s < ov.end_step
                });
            let base = if note.live_override {
                theme.accent.override_bar
            } else if note.local_exception {
                theme.accent.modulation
            } else {
                theme.track_color(note.lane_index)
            };
            let mut fill = base.scale_rgb(0.48 + 0.52 * note.velocity.clamp(0.0, 1.0));
            let mut border = if note.selected {
                theme.accent.warning_key
            } else {
                base
            };
            if displaced {
                fill = fill.with_alpha(0.24);
                border = border.with_alpha(0.35);
            } else if note.channel_muted {
                fill = fill.with_alpha(0.50);
                border = border.with_alpha(0.65);
            }
            let sounding = !displaced
                && view.playing
                && view
                    .playhead_step
                    .is_some_and(|ph| ph >= onset && ph <= onset + f64::from(note.gate));
            dl.panel(
                nr,
                if sounding { base.scale_rgb(1.25) } else { fill },
                border,
                theme.radii.note,
                if note.selected {
                    1.5
                } else if note.live_override {
                    1.1
                } else {
                    0.8
                },
                if sounding {
                    theme.glow.active_note
                } else if note.selected {
                    0.25
                } else if note.live_override {
                    theme.glow.override_bar * 0.45
                } else {
                    0.0
                },
            );
            if !displaced && nr.w >= 26.0 && nr.h >= 12.0 {
                dl.text(
                    nr.inset(3.0, 0.0),
                    note.note_name.as_str(),
                    theme.font.small - 1.5,
                    true,
                    theme.text.inverted,
                    TextAlign::Left,
                );
            }
        }
    }

    // Crisp live override badge pinned cleanly at the top-left of the override span.
    if let Some(ov) = selected_ov {
        let ox0 = geom.step_to_x(ov.from_step as f64).max(geom.timeline_x);
        let ox1 = geom.step_to_x(ov.end_step as f64).min(body.right());
        let avail = ox1 - ox0 - 8.0;
        if avail >= 48.0 {
            let lbl = format!("▶ LIVE · {}", ov.part_name);
            let font_sz = theme.font.small - 1.0;
            let max_c = ((avail - 10.0) / theme.char_width(font_sz)) as usize;
            let text = trim_label(&lbl, max_c);
            let pill_w = (text.chars().count() as f32 * theme.char_width(font_sz) + 12.0)
                .min(avail)
                .max(40.0);
            let pill = Rect::new(ox0 + 4.0, body.y + 4.0, pill_w, 16.0);
            dl.panel(
                pill,
                theme.surface.overlay.with_alpha(0.92),
                theme.accent.override_bar,
                theme.radii.badge,
                1.0,
                theme.glow.override_bar * 0.50,
            );
            dl.text(
                pill.inset(5.0, 0.0),
                text,
                font_sz,
                true,
                theme.accent.override_bar,
                TextAlign::Left,
            );
        }
    }

    dl.pop_clip();
}
