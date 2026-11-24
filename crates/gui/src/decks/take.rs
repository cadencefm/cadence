//! GPU Take deck (`gt`): horizontal bar-by-bar strip of an explicitly armed take,
//! showing heard song bars, loop passes, glowing live overrides, mid-take edits,
//! Vim-style `v` visual range selection, `X` discard, and one-key `k`/`Enter` Keep.
use crate::chrome::{draw_deck_frame, DeckFrameSpec};
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::Theme;
use cadence_app::view::{TakeDeckView, UiModeKind};

pub fn draw_take(
    dl: &mut DisplayList,
    area: Rect,
    view: &TakeDeckView,
    focused: bool,
    mode: UiModeKind,
    theme: &Theme,
) {
    let badge = if view.empty {
        if view.take_armed {
            "TAKE ARM · Space starts recording".into()
        } else {
            "UNARMED · tr arms take recording".into()
        }
    } else {
        let rec = if view.take_capped {
            "● REC · CAPPED"
        } else if view.active_recording {
            "● REC"
        } else if view.take_armed {
            "SEALED · ARM"
        } else {
            "SEALED"
        };
        let range = match view.mark_range {
            Some((a, b)) if view.visual_selecting => {
                format!(" · VISUAL B{:02}–{:02}", a + 1, b + 1)
            }
            Some((a, b)) => format!(" · mark B{:02}–{:02}", a + 1, b + 1),
            None => format!(" · bar {:02}/{}", view.cursor_bar + 1, view.total_bars),
        };
        format!("{rec} · {} bars{range}", view.total_bars)
    };
    let accent = if view.active_recording {
        theme.accent.alert
    } else if view.visual_selecting || view.take_armed {
        theme.accent.warning_key
    } else {
        theme.accent.focus
    };
    let inner = draw_deck_frame(
        dl,
        area,
        &DeckFrameSpec {
            title: "TAKE",
            right_badge: Some(&badge),
            accent,
            focused,
            mode,
        },
        theme,
    );
    if inner.w < 120.0 || inner.h < 48.0 {
        return;
    }
    dl.push_clip(inner);

    if view.empty {
        dl.text(
            inner.inset(12.0, 12.0),
            if view.take_armed {
                "Take recording is armed. Press Space to record your performance, or tr to disarm."
            } else {
                "Take recording is unarmed by default. Press tr to arm before pressing Space."
            },
            theme.font.body,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
        dl.pop_clip();
        return;
    }

    let (body, footer) = inner.split_bottom(22.0);
    let (bars_area, events_area) = if body.h >= 86.0 {
        body.split_bottom(24.0)
    } else {
        (body, Rect::new(body.x, body.bottom(), body.w, 0.0))
    };

    // Horizontal scrolling bar cards centered on cursor_bar.
    let card_w = (bars_area.w / 12.0).clamp(58.0, 108.0);
    let gap = 4.0;
    let visible = ((bars_area.w + gap) / (card_w + gap)).floor().max(1.0) as usize;
    let half = visible / 2;
    let max_first = view.total_bars.saturating_sub(visible);
    let first = view.cursor_bar.saturating_sub(half).min(max_first);

    for (vis_idx, bar) in view.bars.iter().skip(first).take(visible).enumerate() {
        let x = bars_area.x + vis_idx as f32 * (card_w + gap);
        let r = Rect::new(x, bars_area.y + 2.0, card_w, (bars_area.h - 6.0).max(24.0));
        let (bg, border, glow) = if bar.selected {
            let sel_accent = if view.visual_selecting {
                theme.accent.warning_key
            } else {
                theme.accent.focus
            };
            (sel_accent.with_alpha(0.24), sel_accent, 0.24)
        } else if bar.in_mark {
            let mark_accent = if view.visual_selecting {
                theme.accent.warning_key
            } else {
                theme.accent.routing
            };
            (mark_accent.with_alpha(0.22), mark_accent, 0.16)
        } else if !bar.overrides.is_empty() {
            (
                theme.accent.override_bar.with_alpha(0.14),
                theme.accent.override_bar.with_alpha(0.55),
                0.08,
            )
        } else {
            (theme.surface.recessed, theme.border.subtle, 0.0)
        };
        dl.panel(
            r,
            bg,
            border,
            theme.radii.pill,
            if bar.selected { 1.5 } else { 1.0 },
            glow,
        );

        let (hdr, rest) = r.inset(5.0, 3.0).split_top(16.0);
        let hdr_lbl = match (bar.song_bar, bar.in_loop) {
            (Some(sb), true) => format!("{:02} ↩B{:02}", bar.take_bar, sb),
            (Some(sb), false) => format!("{:02} B{:02}", bar.take_bar, sb),
            (None, _) => format!("{:02} ···", bar.take_bar),
        };
        dl.text(
            hdr,
            hdr_lbl,
            theme.font.small - 0.5,
            bar.selected || bar.in_mark,
            if bar.selected {
                theme.accent.focus
            } else if bar.in_loop {
                theme.accent.routing
            } else {
                theme.text.secondary
            },
            TextAlign::Left,
        );
        if bar.edited {
            dl.text(
                hdr,
                "✶",
                theme.font.small,
                true,
                theme.accent.warning_key,
                TextAlign::Right,
            );
        }

        // Active live overrides inside this bar.
        let mut cy = rest.y + 2.0;
        let max_c = (rest.w / theme.char_width(theme.font.small - 1.0)) as usize;
        for (lane_idx, part_name) in bar.overrides.iter().take(2) {
            if cy + 14.0 > r.bottom() - 2.0 {
                break;
            }
            let chip = Rect::new(rest.x, cy, rest.w, 14.0);
            let col = theme.track_color(*lane_idx);
            dl.rounded(chip, col.with_alpha(0.24), 2.0);
            dl.text(
                chip.inset(3.0, 0.0),
                trim_label(part_name, max_c),
                theme.font.small - 1.0,
                true,
                col,
                TextAlign::Left,
            );
            cy += 16.0;
        }
        if bar.overrides.is_empty() && rest.h >= 14.0 {
            dl.text(
                Rect::new(rest.x, cy, rest.w, 14.0),
                if bar.muted_lanes > 0 {
                    format!("{} muted", bar.muted_lanes)
                } else {
                    "arr.".into()
                },
                theme.font.small - 1.0,
                false,
                theme.text.muted,
                TextAlign::Left,
            );
        }
    }

    // Event marker pills for the visible bar window.
    if events_area.h >= 18.0 && !view.events.is_empty() {
        let mut ex = events_area.x;
        let cw = theme.char_width(theme.font.small - 1.0);
        for ev in &view.events {
            let b = ev.bar_offset.floor() as usize;
            if b < first || b >= first + visible {
                continue;
            }
            let lbl = format!("B{:02} {}", b + 1, trim_label(&ev.label, 18));
            let pw = lbl.chars().count() as f32 * cw + 12.0;
            if ex + pw > events_area.right() {
                break;
            }
            let pr = Rect::new(ex, events_area.y + 3.0, pw, 17.0);
            let col = if ev.is_edit {
                theme.accent.warning_key
            } else {
                theme.accent.modulation
            };
            dl.panel(
                pr,
                col.with_alpha(0.16),
                col.with_alpha(0.65),
                theme.radii.badge,
                1.0,
                0.0,
            );
            dl.text(
                pr.inset(5.0, 0.0),
                lbl,
                theme.font.small - 1.0,
                false,
                col,
                TextAlign::Left,
            );
            ex += pw + 5.0;
        }
    }

    dl.text(
        footer,
        if view.visual_selecting {
            "VISUAL  ·  h/l extend range  ·  v lock  ·  Enter/Alt+c keep  ·  Esc clear"
        } else {
            "h/l bar  ·  v mark  ·  Enter/Alt+c keep  ·  Ctrl+s save  ·  Ctrl+Shift+s save as  ·  o load  ·  x clear  ·  X discard  ·  tr arm"
        },
        theme.font.small,
        view.visual_selecting,
        if view.visual_selecting {
            theme.accent.warning_key
        } else {
            theme.text.muted
        },
        TextAlign::Left,
    );
    dl.pop_clip();
}
