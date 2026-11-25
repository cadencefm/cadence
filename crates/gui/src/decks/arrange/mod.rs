//! GPU Arrange deck: Placements, Evaluated Overview, and Evaluated Lane Roll,
//! with continuous zoom, smooth 60/120fps laser playhead, glowing override double-bars,
//! pending/staged beacons, loop bands, and Strudel-style live playhead note illumination.
mod overview;
mod placements;
mod roll;

use crate::chrome::{draw_deck_frame, DeckFrameSpec};
use crate::decks::output::{db_norm, METER_ZERO_DB_NORM};
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::{Color, Theme};
use cadence_app::view::{ArrangeDeckView, UiModeKind};
use cadence_app::ArrangeView;

/// Shared horizontal timeline geometry passed to the Arrange sub-views.
#[derive(Clone, Copy, Debug)]
pub(super) struct TimelineGeom {
    pub gutter_w: f32,
    pub timeline_x: f32,
    pub timeline_w: f32,
    pub start_step: f64,
    pub span_steps: f64,
}

impl TimelineGeom {
    pub(super) fn step_to_x(self, step: f64) -> f32 {
        self.timeline_x + ((step - self.start_step) / self.span_steps) as f32 * self.timeline_w
    }

    /// Clamp a timeline block's horizontal label box to `[timeline_x, timeline_x + timeline_w]`
    /// so partially scrolled placement and section titles remain cleanly visible inside the timeline.
    pub(super) fn visible_label_rect(self, block: Rect, pad_x: f32) -> Option<Rect> {
        let vis_x0 = block.x.max(self.timeline_x);
        let vis_x1 = block.right().min(self.timeline_x + self.timeline_w);
        let vis_w = vis_x1 - vis_x0;
        (vis_w >= 26.0).then(|| {
            Rect::new(
                vis_x0 + pad_x,
                block.y,
                (vis_w - pad_x * 2.0).max(0.0),
                block.h,
            )
        })
    }
}

/// Draw a crisp dashed outline around `rect` (used for live staging previews across Arrange & Bank).
pub(crate) fn draw_dashed_rect(
    dl: &mut DisplayList,
    rect: Rect,
    color: Color,
    dash: f32,
    gap: f32,
    thickness: f32,
) {
    if rect.w < 2.0 || rect.h < 2.0 {
        return;
    }
    let step = (dash + gap).max(2.0);
    let mut x = rect.x;
    while x < rect.right() {
        let w = dash.min(rect.right() - x);
        dl.fill(Rect::new(x, rect.y, w, thickness), color);
        dl.fill(
            Rect::new(x, (rect.bottom() - thickness).max(rect.y), w, thickness),
            color,
        );
        x += step;
    }
    let mut y = rect.y;
    while y < rect.bottom() {
        let h = dash.min(rect.bottom() - y);
        dl.fill(Rect::new(rect.x, y, thickness, h), color);
        dl.fill(
            Rect::new((rect.right() - thickness).max(rect.x), y, thickness, h),
            color,
        );
        y += step;
    }
}

/// Draw a vertical dashed line (used for the live staging landing boundary).
pub(crate) fn draw_dashed_vline(dl: &mut DisplayList, x: f32, y0: f32, y1: f32, color: Color) {
    let mut y = y0;
    while y < y1 {
        let h = 5.0_f32.min(y1 - y);
        dl.fill(Rect::new(x, y, 1.5, h), color);
        y += 9.0;
    }
}

/// Draw a compact vertical dual-channel (L/R) peak meter pill inside an Arrange lane gutter.
pub(super) fn draw_lane_meter_pill(
    dl: &mut DisplayList,
    rect: Rect,
    peak: Option<[f32; 2]>,
    hot: bool,
    clipped: bool,
    muted: bool,
    theme: &Theme,
) {
    if rect.w < 6.0 || rect.h < 10.0 {
        return;
    }
    let border = if clipped {
        theme.meter.clip
    } else if hot {
        theme.meter.hot.with_alpha(0.75)
    } else {
        theme.border.subtle
    };
    dl.panel(rect, theme.meter.track_bg, border, 2.0, 0.75, 0.0);
    let inner = rect.inset(1.5, 1.5);
    if inner.w < 3.0 || inner.h < 4.0 {
        return;
    }
    let bar_w = ((inner.w - 1.0) * 0.5).max(1.5);
    let y_zero = inner.bottom() - inner.h * METER_ZERO_DB_NORM;
    if let Some([pl, pr]) = peak {
        for (i, lvl) in [pl, pr].into_iter().enumerate() {
            let bx = inner.x + i as f32 * (bar_w + 1.0);
            let h = (inner.h * db_norm(lvl)).clamp(0.0, inner.h);
            if h > 0.5 {
                let mut col = if clipped || lvl > 1.0 {
                    theme.meter.clip
                } else if lvl >= 0.5 {
                    theme.meter.high
                } else {
                    theme.meter.mid
                };
                if muted {
                    col = col.with_alpha(0.40);
                }
                dl.fill(Rect::new(bx, inner.bottom() - h, bar_w, h), col);
            }
        }
    }
    dl.fill(
        Rect::new(inner.x, y_zero, inner.w, 1.0),
        theme.meter.zero_db.with_alpha(0.45),
    );
}

pub fn draw_arrange(
    dl: &mut DisplayList,
    area: Rect,
    view: &ArrangeDeckView,
    mode: UiModeKind,
    theme: &Theme,
) {
    let clock = if view.solo_audition {
        "solo audition".into()
    } else if let Some(pos) = view.playhead_step {
        let s = pos.floor().max(0.0) as usize;
        format!(
            "{} {:03}.{}.{}",
            if view.playing { "▶" } else { "∥" },
            s / 16 + 1,
            (s % 16) / 4 + 1,
            s % 4 + 1
        )
    } else {
        format!("{} end", if view.playing { "▶" } else { "∥" })
    };
    let badge = format!(
        "{} · zoom {:.0}% · {clock}",
        view.mode.label(),
        view.zoom * 100.0
    );
    let inner = draw_deck_frame(
        dl,
        area,
        &DeckFrameSpec {
            title: "ARRANGEMENT",
            right_badge: Some(&badge),
            accent: theme.accent.focus,
            focused: view.focused,
            mode,
        },
        theme,
    );
    if inner.w < 160.0 || inner.h < 48.0 {
        return;
    }
    dl.push_clip(inner);

    let gutter_w = if view.mode == ArrangeView::Roll {
        56.0
    } else {
        (inner.w * 0.18).clamp(124.0, 184.0)
    };
    let has_sections = !view.sections.is_empty() && inner.h >= 110.0;
    let sec_h = if has_sections { 20.0 } else { 0.0 };
    let ruler_h = 22.0;

    let (sec_row, rest) = inner.split_top(sec_h);
    let (ruler_row, body) = rest.split_top(ruler_h);
    let timeline_x = inner.x + gutter_w + 4.0;
    let timeline_w = (inner.right() - timeline_x).max(1.0);
    let geom = TimelineGeom {
        gutter_w,
        timeline_x,
        timeline_w,
        start_step: view.page_start_step as f64,
        span_steps: view.page_steps.max(1) as f64,
    };

    // 1. Optional section bands strip.
    if has_sections {
        draw_section_strip(dl, sec_row, inner.right(), view, geom, theme);
    }

    // 2. Bar ruler + active/pending loop bands + pending edit markers.
    draw_bar_ruler(dl, ruler_row, inner, view, geom, theme);

    // 3. Body by mode (each view paints its row backgrounds, then vertical bar/beat/step grid,
    //    then foreground blocks/notes so opaque row fills never cover the time grid).
    match view.mode {
        ArrangeView::Placements => {
            placements::draw_placements(dl, body, view, geom, theme);
        }
        ArrangeView::Overview => {
            if let Some(eval) = &view.evaluated {
                overview::draw_overview(dl, body, view, eval, geom, theme);
            }
        }
        ArrangeView::Roll => {
            if let Some(eval) = &view.evaluated {
                roll::draw_roll(dl, body, view, eval, geom, theme);
            }
        }
    }

    // 4. Full-height vertical user edit/cue cursor bar across ruler + body (consistent across
    //    Placements, Overview, and Roll; painted before playhead so live playback rides cleanly on top).
    draw_user_cursor_bar(dl, ruler_row, body, view, geom, theme);

    // 5. Vertical dashed live staging boundary beacon when staging is active.
    if view.staging_active {
        if let Some(boundary) = view.staged_boundary_step {
            let bpos = boundary as f64;
            if bpos >= geom.start_step && bpos <= geom.start_step + geom.span_steps {
                let bx = geom.step_to_x(bpos);
                dl.rounded_glow(
                    Rect::new(bx - 3.5, ruler_row.y + 1.0, 7.0, 5.0),
                    theme.accent.modulation,
                    2.0,
                    0.35,
                );
                draw_dashed_vline(
                    dl,
                    bx,
                    ruler_row.y + 6.0,
                    body.bottom(),
                    theme.accent.modulation.with_alpha(0.78),
                );
            }
        }
    }

    // 6. Smooth 60/120fps phosphor laser playhead across ruler + body!
    if let Some(pos) = view.playhead_step {
        if pos >= geom.start_step && pos <= geom.start_step + geom.span_steps {
            let px = geom.step_to_x(pos);
            // Glowing cap triangle-like beacon on ruler.
            dl.rounded_glow(
                Rect::new(px - 4.0, ruler_row.y + 1.0, 8.0, 6.0),
                theme.accent.playhead,
                2.0,
                theme.glow.playhead,
            );
            // Laser line spanning full height.
            dl.rounded_glow(
                Rect::new(
                    px - 1.0,
                    ruler_row.y + 6.0,
                    2.0,
                    (body.bottom() - ruler_row.y - 6.0).max(0.0),
                ),
                theme.accent.playhead,
                0.0,
                theme.glow.playhead,
            );
        }
    }

    dl.pop_clip();
}

/// Full-height vertical user cursor bar spanning the Arrange bar ruler and lane/roll body.
///
/// Uses a bottom-anchored ruler cap so it stays distinct from the top-anchored green playhead
/// beacon even when both sit on the exact same step.
fn draw_user_cursor_bar(
    dl: &mut DisplayList,
    ruler_row: Rect,
    body: Rect,
    view: &ArrangeDeckView,
    geom: TimelineGeom,
    theme: &Theme,
) {
    let cpos = f64::from(view.cursor_step);
    if cpos < geom.start_step || cpos > geom.start_step + geom.span_steps {
        return;
    }
    let cx = geom.step_to_x(cpos);
    let tl_right = geom.timeline_x + geom.timeline_w;
    if cx < geom.timeline_x - 2.0 || cx > tl_right + 2.0 {
        return;
    }

    let line_alpha = if view.focused { 0.88 } else { 0.42 };
    let cap_alpha = if view.focused { 0.95 } else { 0.50 };
    let line_w = if view.focused { 1.5 } else { 1.0 };
    let glow = if view.focused {
        theme.glow.focus_border * 0.72
    } else {
        0.0
    };
    let cursor_col = theme.accent.edit_cursor;

    dl.push_clip(Rect::new(
        (geom.timeline_x - 2.0).max(ruler_row.x),
        ruler_row.y,
        (geom.timeline_w + 4.0).min(ruler_row.right() - (geom.timeline_x - 2.0).max(ruler_row.x)),
        (body.bottom() - ruler_row.y).max(0.0),
    ));

    // Subtle ruler tick from top to bottom of ruler_row + crisp bottom-anchored cap.
    dl.fill(
        Rect::new(cx - 0.5, ruler_row.y, 1.0, ruler_row.h),
        cursor_col.with_alpha(line_alpha * 0.55),
    );
    dl.rounded_glow(
        Rect::new(cx - 3.5, ruler_row.bottom() - 4.5, 7.0, 4.5),
        cursor_col.with_alpha(cap_alpha),
        1.5,
        glow,
    );

    // Full-height vertical cursor line through the entire Arrange body.
    dl.rounded_glow(
        Rect::new(
            cx - line_w * 0.5,
            ruler_row.bottom() - 1.0,
            line_w,
            (body.bottom() - ruler_row.bottom() + 1.0).max(0.0),
        ),
        cursor_col.with_alpha(line_alpha),
        0.0,
        glow,
    );

    dl.pop_clip();
}

fn draw_section_strip(
    dl: &mut DisplayList,
    sec_row: Rect,
    inner_right: f32,
    view: &ArrangeDeckView,
    geom: TimelineGeom,
    theme: &Theme,
) {
    dl.fill(
        Rect::new(geom.timeline_x, sec_row.y, geom.timeline_w, sec_row.h - 2.0),
        theme.surface.recessed,
    );
    dl.push_clip(Rect::new(
        geom.timeline_x,
        sec_row.y,
        geom.timeline_w,
        sec_row.h,
    ));
    for sec in &view.sections {
        let x0 = geom.step_to_x(f64::from(sec.start_step));
        let x1 = geom.step_to_x(f64::from(sec.start_step + sec.length_steps));
        if x1 <= geom.timeline_x || x0 >= inner_right {
            continue;
        }
        let r = Rect::new(
            x0 + 1.0,
            sec_row.y + 1.0,
            (x1 - x0 - 2.0).max(2.0),
            sec_row.h - 4.0,
        );
        let (bg, border, fg) = if sec.staged {
            (
                theme.accent.modulation.with_alpha(0.24),
                theme.accent.modulation,
                theme.accent.modulation,
            )
        } else if sec.queued {
            (
                theme.accent.warning_key.with_alpha(0.24),
                theme.accent.warning_key,
                theme.accent.warning_key,
            )
        } else if sec.active {
            (
                theme.accent.focus.with_alpha(0.18),
                theme.accent.focus,
                theme.accent.focus,
            )
        } else {
            (
                theme.surface.elevated,
                theme.border.subtle,
                theme.text.secondary,
            )
        };
        if sec.staged {
            dl.rounded_glow(r, bg, theme.radii.badge, 0.24);
            draw_dashed_rect(dl, r, border, 5.0, 3.0, 1.2);
        } else {
            dl.panel(
                r,
                bg,
                border,
                theme.radii.badge,
                1.0,
                if sec.queued { 0.28 } else { 0.0 },
            );
        }
        if let Some(lbl_rect) = geom.visible_label_rect(r, 4.0) {
            let prefix = if sec.staged { "◇ " } else { "" };
            let lbl = match sec.key_char {
                Some(k) if view.jam_active => format!("{prefix}{k} {}", sec.name),
                Some(k) => format!("{prefix}⌥{k} {}", sec.name),
                None => format!("{prefix}{}", sec.name),
            };
            let max_c = (lbl_rect.w / theme.char_width(theme.font.small)) as usize;
            dl.text(
                lbl_rect,
                trim_label(&lbl, max_c),
                theme.font.small - 0.5,
                sec.active || sec.queued || sec.staged,
                fg,
                TextAlign::Left,
            );
        }
    }
    dl.pop_clip();
}

/// Draw vertical bar, beat, and (when zoomed in) 16th-step grid lines across `area`.
/// Called by each Arrange sub-view AFTER its row backgrounds and BEFORE foreground blocks/notes.
pub(super) fn draw_vertical_time_grid(
    dl: &mut DisplayList,
    area: Rect,
    view: &ArrangeDeckView,
    geom: TimelineGeom,
    theme: &Theme,
) {
    let first_bar = view.page_start_step / 16;
    let last_bar = (view.page_start_step + view.page_steps)
        .div_ceil(16)
        .min(view.song_bars);
    let px_per_bar = (geom.timeline_w / (geom.span_steps as f32 / 16.0)).max(1.0);
    let right = geom.timeline_x + geom.timeline_w;
    for bar in first_bar..last_bar {
        let x = geom.step_to_x((bar * 16) as f64);
        if x >= geom.timeline_x - 1.0 && x <= right {
            dl.fill(
                Rect::new(x, area.y, 1.0, area.h),
                theme.border.grid_bar.with_alpha(0.65),
            );
        }
        if px_per_bar >= 64.0 {
            for beat in 1..4 {
                let bx = geom.step_to_x((bar * 16 + beat * 4) as f64);
                if bx >= geom.timeline_x && bx < right {
                    dl.fill(
                        Rect::new(bx, area.y, 1.0, area.h),
                        theme.border.grid_beat.with_alpha(0.50),
                    );
                }
            }
        }
        if px_per_bar >= 240.0 {
            for sub in 1..16 {
                if sub % 4 == 0 {
                    continue;
                }
                let sx = geom.step_to_x((bar * 16 + sub) as f64);
                if sx >= geom.timeline_x && sx < right {
                    dl.fill(
                        Rect::new(sx, area.y, 1.0, area.h),
                        theme.border.grid_step.with_alpha(0.36),
                    );
                }
            }
        }
    }
}

fn draw_bar_ruler(
    dl: &mut DisplayList,
    ruler_row: Rect,
    inner: Rect,
    view: &ArrangeDeckView,
    geom: TimelineGeom,
    theme: &Theme,
) {
    dl.fill(ruler_row, theme.surface.recessed);
    dl.text(
        Rect::new(inner.x + 6.0, ruler_row.y, geom.gutter_w - 8.0, ruler_row.h),
        if view.mode == ArrangeView::Roll {
            "PITCH"
        } else if view.jam_active {
            "LANE (q..p JAM)"
        } else {
            "LANE (⌥q..p)"
        },
        theme.font.small,
        true,
        theme.text.muted,
        TextAlign::Left,
    );

    // Loop bands on ruler.
    if let Some((from, to)) = view.loop_region {
        let x0 = geom.step_to_x(f64::from(from)).max(geom.timeline_x);
        let x1 = geom.step_to_x(f64::from(to)).min(inner.right());
        if x1 > x0 {
            let active = view.playing_loop == Some((from, to));
            let r = Rect::new(x0, ruler_row.y + 1.0, x1 - x0, ruler_row.h - 2.0);
            if active {
                dl.rounded_glow(r, theme.accent.routing.with_alpha(0.32), 2.0, 0.20);
                dl.fill(Rect::new(r.x, r.y, r.w, 2.0), theme.accent.routing);
            } else {
                dl.hatched_band(
                    r,
                    theme.accent.routing.with_alpha(0.18),
                    theme.accent.routing.with_alpha(0.70),
                    2.0,
                    8.0,
                    0.0,
                );
            }
        }
    }

    // Bar ticks & numbers.
    let first_bar = view.page_start_step / 16;
    let last_bar = (view.page_start_step + view.page_steps)
        .div_ceil(16)
        .min(view.song_bars);
    let px_per_bar = (geom.timeline_w / (geom.span_steps as f32 / 16.0)).max(1.0);
    let label_stride = if px_per_bar < 26.0 {
        4
    } else if px_per_bar < 44.0 {
        2
    } else {
        1
    };
    for bar in first_bar..last_bar {
        let x = geom.step_to_x((bar * 16) as f64);
        if x < geom.timeline_x - 1.0 || x > inner.right() {
            continue;
        }
        dl.fill(
            Rect::new(x, ruler_row.y, 1.0, ruler_row.h),
            theme.border.grid_bar,
        );
        if bar % label_stride == 0 {
            let pending_edit = view.pending_edit_bars.contains(&(bar as u64));
            let text = if pending_edit {
                format!("{:02}✶", bar + 1)
            } else {
                format!("{:02}", bar + 1)
            };
            dl.text(
                Rect::new(
                    x + 4.0,
                    ruler_row.y,
                    (px_per_bar - 4.0).max(24.0),
                    ruler_row.h,
                ),
                text,
                theme.font.small,
                pending_edit,
                if pending_edit {
                    theme.accent.warning_key
                } else {
                    theme.text.secondary
                },
                TextAlign::Left,
            );
        }
    }
}
