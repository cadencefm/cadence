//! GPU Bank deck: row-oriented Parts launchpad (Lanes as horizontal rows `j`/`k`,
//! Parts as horizontal cards left-to-right `h`/`l` in both compact and zoomed `zz` views),
//! section launcher strip (⌥1..0 / 1..0), and searchable material library.
use crate::chrome::{draw_deck_frame, DeckFrameSpec};
use crate::decks::arrange::draw_dashed_rect;
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::Theme;
use cadence_app::view::{BankDeckView, BankLaneGroup, BankPartEntry, BankPartStatus, UiModeKind};
use cadence_app::BankMode;
use cadence_core::material::PhaseMode;

pub fn draw_bank(
    dl: &mut DisplayList,
    area: Rect,
    view: &BankDeckView,
    focused: bool,
    mode: UiModeKind,
    theme: &Theme,
) {
    let title = match &view.search_query {
        Some(q) => format!("BANK  /{q}"),
        None => match view.mode {
            BankMode::Parts if area.w >= 480.0 => "BANK · PARTS LAUNCHPAD".into(),
            BankMode::Parts => "BANK · PARTS".into(),
            BankMode::Material => "BANK · MATERIAL".into(),
        },
    };
    let badge = if let Some(n) = view.staging_count {
        format!("◇ STAGED ({n}) · S fire")
    } else if area.w >= 480.0 {
        format!(
            "{} · {} defs · v switch",
            if view.mode == BankMode::Parts {
                "PARTS"
            } else {
                "MATERIAL"
            },
            view.total_patterns
        )
    } else {
        format!("{} defs · v", view.total_patterns)
    };
    let inner = draw_deck_frame(
        dl,
        area,
        &DeckFrameSpec {
            title: &title,
            right_badge: Some(&badge),
            accent: theme.accent.focus,
            focused,
            mode,
        },
        theme,
    );
    if inner.w < 80.0 || inner.h < 60.0 {
        return;
    }
    dl.push_clip(inner);
    let mut y = inner.y;

    // 1. Sections quick-jump strip (⌥1..0 in Normal, bright 1..0 in Jam).
    if !view.sections.is_empty() && inner.h >= 160.0 {
        dl.text(
            Rect::new(inner.x, y, inner.w, 16.0),
            if view.jam_active {
                "SECTIONS  (1..0 JAM)"
            } else {
                "SECTIONS  (⌥1..0)"
            },
            theme.font.small,
            true,
            if view.jam_active {
                theme.accent.warning_key
            } else {
                theme.text.muted
            },
            TextAlign::Left,
        );
        y += 18.0;
        let mut sx = inner.x;
        let pill_h = 20.0;
        let cw = theme.char_width(theme.font.small);
        for sec in &view.sections {
            let key_prefix = match sec.key_char {
                Some(k) if view.jam_active => format!("{k} "),
                Some(k) => format!("⌥{k} "),
                None => String::new(),
            };
            let staged_prefix = if sec.staged { "◇ " } else { "" };
            let label = format!("{staged_prefix}{key_prefix}{}", trim_label(&sec.name, 10));
            let pw = (label.chars().count() as f32 * cw + 14.0).clamp(44.0, inner.w * 0.48);
            if sx + pw > inner.right() {
                sx = inner.x;
                y += pill_h + 4.0;
                if y + pill_h > inner.y + 84.0 {
                    break;
                }
            }
            let r = Rect::new(sx, y, pw, pill_h);
            let (bg, border, fg, glow) = if sec.staged {
                (
                    theme.accent.modulation.with_alpha(0.24),
                    theme.accent.modulation,
                    theme.accent.modulation,
                    0.28,
                )
            } else if sec.queued {
                (
                    theme.accent.warning_key.with_alpha(0.22),
                    theme.accent.warning_key,
                    theme.accent.warning_key,
                    0.35,
                )
            } else if sec.active {
                (
                    theme.accent.playhead.with_alpha(0.20),
                    theme.accent.playhead,
                    theme.accent.playhead,
                    0.25,
                )
            } else {
                (
                    theme.surface.recessed,
                    theme.border.subtle,
                    theme.text.secondary,
                    0.0,
                )
            };
            if sec.staged {
                dl.rounded_glow(r, bg, theme.radii.badge, glow);
                draw_dashed_rect(dl, r, border, 4.0, 2.5, 1.2);
            } else {
                dl.panel(r, bg, border, theme.radii.badge, 1.0, glow);
            }
            dl.text(
                r.inset(5.0, 0.0),
                label,
                theme.font.small,
                sec.active || sec.queued || sec.staged,
                fg,
                TextAlign::Left,
            );
            sx += pw + 4.0;
        }
        y += pill_h + 8.0;
    }

    // 2. Zoomed (`zz`) full-screen row-oriented Launchpad Matrix (Lanes = rows, Parts = horizontal cards).
    if view.zoomed && view.mode == BankMode::Parts && inner.w >= 480.0 {
        let matrix = Rect::new(inner.x, y, inner.w, (inner.bottom() - y).max(0.0));
        draw_bank_parts_rows(dl, matrix, view, focused, true, true, theme);
        dl.pop_clip();
        return;
    }

    // 3. Compact row-oriented Lane × Part launchpad strip (matching Arrange 1:1).
    let parts_active = view.mode == BankMode::Parts;
    if !view.lanes.is_empty() && inner.bottom() - y >= 92.0 {
        let max_parts_y = if parts_active {
            inner.y + inner.h * 0.70
        } else {
            inner.y + inner.h * 0.45
        };
        let parts_area = Rect::new(inner.x, y, inner.w, (max_parts_y - y).max(0.0));
        let used_h =
            draw_bank_parts_rows(dl, parts_area, view, focused, parts_active, false, theme);
        y += used_h + 6.0;
    }

    // 4. Material pattern library below.
    dl.fill(Rect::new(inner.x, y, inner.w, 1.0), theme.border.subtle);
    y += 5.0;
    let mat_active = view.mode == BankMode::Material;
    let mat_hdr = if mat_active {
        "▸ MATERIAL  (Enter open · / filter · v parts)"
    } else {
        "MATERIAL  (v focus · / filter)"
    };
    let max_mat_hdr = (inner.w / theme.char_width(theme.font.small)) as usize;
    dl.text(
        Rect::new(inner.x, y, inner.w, 16.0),
        trim_label(mat_hdr, max_mat_hdr),
        theme.font.small,
        mat_active,
        if mat_active {
            theme.accent.focus
        } else {
            theme.text.muted
        },
        TextAlign::Left,
    );
    y += 18.0;

    if view.patterns.is_empty() {
        dl.text(
            Rect::new(inner.x, y + 6.0, inner.w, 40.0),
            if view.total_patterns == 0 {
                "No material yet · press n to create"
            } else {
                "No matching material · Esc clears"
            },
            theme.font.small,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
        dl.pop_clip();
        return;
    }

    let item_h = 34.0;
    let visible = ((inner.bottom() - y) / item_h).floor().max(1.0) as usize;
    let sel_idx = view.patterns.iter().position(|p| p.selected).unwrap_or(0);
    let first = sel_idx.saturating_sub(visible / 2);
    for pat in view.patterns.iter().skip(first).take(visible) {
        if y + item_h > inner.bottom() + 2.0 {
            break;
        }
        let r = Rect::new(inner.x, y, inner.w, item_h - 3.0);
        let col = theme.track_color(pat.id as usize);
        if pat.selected && mat_active {
            dl.panel(
                r,
                theme.surface.selection,
                col.with_alpha(0.75),
                theme.radii.badge,
                1.0,
                0.12,
            );
        } else if pat.selected {
            dl.rounded(r, theme.surface.active_row, theme.radii.badge);
        } else {
            dl.rounded(
                r,
                theme.surface.recessed.with_alpha(0.55),
                theme.radii.badge,
            );
        }
        dl.fill(Rect::new(r.x + 1.0, r.y + 3.0, 3.0, r.h - 6.0), col);
        let max_c = ((r.w - 16.0) / theme.char_width(theme.font.small)) as usize;
        dl.text(
            Rect::new(r.x + 10.0, r.y + 1.0, r.w - 14.0, 16.0),
            trim_label(&pat.name, max_c),
            theme.font.small,
            pat.selected,
            if pat.selected {
                col
            } else {
                theme.text.primary
            },
            TextAlign::Left,
        );
        dl.text(
            Rect::new(r.x + 10.0, r.y + 16.0, r.w - 14.0, 14.0),
            format!("{} · {} steps", pat.kind_label, pat.period),
            theme.font.small - 1.0,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
        y += item_h;
    }
    dl.pop_clip();
}

/// Shared row-oriented Launchpad renderer for both compact and zoomed (`zz`) Bank Parts.
/// Lanes are horizontal rows (`j`/`k`); each Lane's Parts are horizontal cards left-to-right (`h`/`l`).
/// Returns the vertical height consumed inside `area`.
fn draw_bank_parts_rows(
    dl: &mut DisplayList,
    area: Rect,
    view: &BankDeckView,
    focused: bool,
    parts_active: bool,
    zoomed: bool,
    theme: &Theme,
) -> f32 {
    if area.h < 36.0 || area.w < 80.0 {
        return 0.0;
    }
    let mut y = area.y;

    let lane_hdr_w = if zoomed {
        (area.w * 0.19).clamp(156.0, 220.0)
    } else {
        (area.w * 0.30).clamp(84.0, 134.0)
    };
    let cards_x = area.x + lane_hdr_w + if zoomed { 8.0 } else { 6.0 };
    let cards_w = (area.right() - cards_x).max(0.0);
    let card_gap = if zoomed { 6.0 } else { 4.0 };
    let min_card_w = if zoomed { 104.0 } else { 66.0 };
    let max_card_w = if zoomed { 196.0 } else { 152.0 };

    // Header strip with axis-aligned horizontal scroll indicator (`◀`/`▶`) reflecting the actual visible slice.
    let sel_lane_scroll = view.lanes.get(view.selected_lane_idx).and_then(|l| {
        if l.parts.is_empty() {
            return None;
        }
        let (vis, has_left, has_right) = visible_part_slice(
            l,
            true,
            view.selected_part_idx,
            cards_w,
            min_card_w,
            card_gap,
        );
        if has_left || has_right {
            let first = vis.first().map_or(1, |p| p.slot_num);
            let last = vis.last().map_or(first, |p| p.slot_num);
            let lt = if has_left { "◀ " } else { "" };
            let rt = if has_right { " ▶" } else { "" };
            Some(format!("{lt}{first}–{last}/{}{rt}", l.total_parts))
        } else {
            None
        }
    });
    let scroll_w = if sel_lane_scroll.is_some() { 96.0 } else { 0.0 };
    let hdr_txt = if zoomed {
        "▸ LAUNCHPAD  (j/k lanes · h/l parts · Enter/Shift+Enter launch · a add · r phase · X drop · x rel)"
    } else if parts_active {
        if area.w < 380.0 {
            "▸ PARTS  (hjkl · Enter · a/r/X)"
        } else {
            "▸ PARTS  (j/k lanes · h/l parts · Enter launch · a/r/X)"
        }
    } else if view.jam_active {
        "PARTS  (q..p + 1..9)"
    } else {
        "PARTS  (⌥q..p + 1..9 · v focus)"
    };
    let max_hdr = ((area.w - scroll_w).max(0.0) / theme.char_width(theme.font.small)) as usize;
    dl.text(
        Rect::new(area.x, y, (area.w - scroll_w).max(0.0), 16.0),
        trim_label(hdr_txt, max_hdr),
        theme.font.small,
        parts_active || zoomed,
        if parts_active || zoomed {
            theme.accent.focus
        } else {
            theme.text.muted
        },
        TextAlign::Left,
    );
    if let Some(scroll_txt) = sel_lane_scroll {
        dl.text(
            Rect::new(area.right() - scroll_w, y, scroll_w, 16.0),
            scroll_txt,
            theme.font.small - 1.0,
            parts_active || zoomed,
            if parts_active || zoomed {
                theme.accent.focus
            } else {
                theme.text.secondary
            },
            TextAlign::Right,
        );
    }
    y += if zoomed { 22.0 } else { 18.0 };

    if view.lanes.is_empty() {
        dl.text(
            Rect::new(area.x, y + 8.0, area.w, 22.0),
            "No lanes yet · press a or n to create a lane",
            theme.font.small,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
        return (y + 30.0 - area.y).min(area.h);
    }

    let body_h = (area.bottom() - y).max(0.0);
    let row_h: f32 = if zoomed {
        56.0
    } else if parts_active && body_h >= 120.0 {
        38.0
    } else {
        26.0
    };
    let row_gap: f32 = if zoomed { 6.0 } else { 4.0 };
    let card_h = (row_h - row_gap).max(20.0);

    let avail_lanes = (body_h / row_h).floor().max(1.0) as usize;
    let total_lanes = view.lanes.len();
    let first_lane = if total_lanes <= avail_lanes {
        0
    } else {
        view.selected_lane_idx
            .saturating_sub(avail_lanes / 2)
            .min(total_lanes - avail_lanes)
    };

    for (li, lane) in view
        .lanes
        .iter()
        .enumerate()
        .skip(first_lane)
        .take(avail_lanes)
    {
        if y + card_h > area.bottom() + 1.0 {
            break;
        }
        let lane_col = theme.track_color(li);
        let lane_sel = (focused || zoomed) && parts_active && li == view.selected_lane_idx;
        let hdr = Rect::new(area.x, y, lane_hdr_w, card_h);
        draw_lane_row_header(
            dl,
            hdr,
            lane,
            lane_col,
            lane_sel,
            view.jam_active,
            zoomed,
            theme,
        );

        let strip = Rect::new(cards_x, y, cards_w, card_h);
        if strip.w >= 32.0 {
            draw_lane_part_cards(
                dl,
                strip,
                lane,
                lane_sel,
                focused && parts_active,
                view.selected_part_idx,
                min_card_w,
                max_card_w,
                card_gap,
                zoomed,
                theme,
            );
        }
        y += row_h;
    }
    (y - area.y).min(area.h)
}

#[allow(clippy::too_many_arguments)]
fn draw_lane_row_header(
    dl: &mut DisplayList,
    hdr: Rect,
    lane: &BankLaneGroup,
    lane_col: crate::theme::Color,
    lane_sel: bool,
    jam_active: bool,
    zoomed: bool,
    theme: &Theme,
) {
    let hdr_staged =
        lane.staged_mute.is_some() || lane.staged_solo.is_some() || lane.staged_release;
    let bg = if hdr_staged {
        theme.accent.modulation.with_alpha(0.18)
    } else if lane_sel {
        lane_col.with_alpha(0.22)
    } else {
        theme.surface.recessed
    };
    let border = if hdr_staged {
        theme.accent.modulation
    } else if lane_sel {
        lane_col
    } else {
        theme.border.subtle
    };
    if hdr_staged {
        dl.rounded_glow(hdr, bg, theme.radii.badge, 0.16);
        draw_dashed_rect(dl, hdr, border, 5.0, 3.0, 1.2);
    } else {
        dl.panel(
            hdr,
            bg,
            border,
            theme.radii.badge,
            if lane_sel { 1.2 } else { 1.0 },
            if lane_sel { 0.14 } else { 0.0 },
        );
    }
    dl.fill(
        Rect::new(hdr.x + 1.0, hdr.y + 3.0, 3.0, (hdr.h - 6.0).max(0.0)),
        if lane.muted {
            theme.text.muted
        } else {
            lane_col
        },
    );

    let kw = if jam_active { 18.0 } else { 24.0 };
    let key_h = 16.0_f32.min((hdr.h - 4.0).max(12.0));
    let key_y = if zoomed || hdr.h >= 32.0 {
        hdr.y + 4.0
    } else {
        hdr.y + (hdr.h - key_h) * 0.5
    };
    let key_r = Rect::new(hdr.x + 7.0, key_y, kw, key_h);
    let klbl = if jam_active {
        lane.key_char.to_string()
    } else {
        format!("⌥{}", lane.key_char)
    };
    dl.rounded(
        key_r,
        lane_col.with_alpha(if jam_active { 0.28 } else { 0.16 }),
        theme.radii.badge,
    );
    dl.text(
        key_r,
        klbl,
        theme.font.small - if jam_active { 0.0 } else { 1.0 },
        jam_active || lane_sel,
        lane_col,
        TextAlign::Center,
    );

    let mut flags = String::new();
    if let Some(m) = lane.staged_mute {
        flags.push_str(if m { " ◇M" } else { " ◇!M" });
    } else if lane.muted {
        flags.push_str(" [M]");
    }
    if let Some(s) = lane.staged_solo {
        flags.push_str(if s { " ◇S" } else { " ◇!S" });
    } else if lane.solo {
        flags.push_str(" [S]");
    }
    if lane.staged_release {
        flags.push_str(" □REL");
    }

    let text_x = key_r.right() + 5.0;
    let text_w = (hdr.right() - text_x - 4.0).max(0.0);
    let max_c = (text_w / theme.char_width(theme.font.small)) as usize;
    let name_color = if hdr_staged {
        theme.accent.modulation
    } else if lane.muted {
        theme.text.muted
    } else if lane_sel {
        theme.text.primary
    } else {
        theme.text.secondary
    };
    let name_h = if zoomed {
        17.0
    } else if hdr.h >= 32.0 {
        15.0
    } else {
        hdr.h
    };
    let name_y = if zoomed {
        hdr.y + 5.0
    } else if hdr.h >= 32.0 {
        hdr.y + 2.0
    } else {
        hdr.y
    };
    dl.text(
        Rect::new(text_x, name_y, text_w, name_h),
        trim_label(&format!("{}{flags}", lane.lane_name), max_c),
        theme.font.small,
        hdr_staged || lane_sel,
        name_color,
        TextAlign::Left,
    );

    if zoomed || hdr.h >= 32.0 {
        let sub = if lane.total_parts == 0 {
            "empty".into()
        } else if zoomed && lane.total_parts > lane.parts.len() && !lane.parts.is_empty() {
            let first = lane.parts.first().map_or(1, |p| p.slot_num);
            let last = lane.parts.last().map_or(first, |p| p.slot_num);
            let lt = if lane.part_window_start > 0 {
                "◀ "
            } else {
                ""
            };
            let rt = if lane.part_window_start + lane.parts.len() < lane.total_parts {
                " ▶"
            } else {
                ""
            };
            format!("{lt}{first}–{last}/{}{rt}", lane.total_parts)
        } else {
            format!(
                "{} part{}",
                lane.total_parts,
                if lane.total_parts == 1 { "" } else { "s" }
            )
        };
        let sub_max = (text_w / theme.char_width(theme.font.small - 1.0)) as usize;
        let sub_y = if zoomed { hdr.y + 24.0 } else { hdr.y + 17.0 };
        dl.text(
            Rect::new(
                text_x,
                sub_y,
                text_w,
                (hdr.bottom() - sub_y - 2.0).max(11.0),
            ),
            trim_label(&sub, sub_max),
            theme.font.small - 1.0,
            false,
            if lane_sel {
                theme.text.secondary
            } else {
                theme.text.muted
            },
            TextAlign::Left,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_lane_part_cards(
    dl: &mut DisplayList,
    strip: Rect,
    lane: &BankLaneGroup,
    lane_sel: bool,
    cursor_active: bool,
    selected_part_idx: usize,
    min_card_w: f32,
    max_card_w: f32,
    gap: f32,
    zoomed: bool,
    theme: &Theme,
) {
    if lane.parts.is_empty() {
        let empty_r = Rect::new(strip.x, strip.y, strip.w.min(max_card_w * 1.4), strip.h);
        dl.rounded(
            empty_r,
            theme.surface.recessed.with_alpha(0.5),
            theme.radii.badge,
        );
        draw_dashed_rect(
            dl,
            empty_r,
            if lane_sel {
                theme.accent.focus
            } else {
                theme.border.subtle
            },
            4.0,
            3.0,
            1.0,
        );
        let msg = if lane_sel {
            "empty · press a to add part"
        } else {
            "(empty)"
        };
        let max_c = ((empty_r.w - 10.0).max(0.0) / theme.char_width(theme.font.small)) as usize;
        dl.text(
            empty_r.inset(6.0, 0.0),
            trim_label(msg, max_c),
            theme.font.small,
            lane_sel,
            if lane_sel {
                theme.accent.focus
            } else {
                theme.text.muted
            },
            TextAlign::Left,
        );
        return;
    }

    let clamped_cursor = selected_part_idx.min(lane.total_parts.saturating_sub(1));
    let arrow_w = 12.0;
    let (visible, has_left, has_right) =
        visible_part_slice(lane, lane_sel, selected_part_idx, strip.w, min_card_w, gap);

    let left_pad = if has_left { arrow_w } else { 0.0 };
    let right_pad = if has_right { arrow_w } else { 0.0 };
    let cards_x = strip.x + left_pad;
    let avail_w = (strip.w - left_pad - right_pad).max(24.0);
    let count = visible.len().max(1);
    let card_w = ((avail_w - gap * (count - 1) as f32) / count as f32).clamp(36.0, max_card_w);

    if has_left {
        dl.text(
            Rect::new(strip.x, strip.y, arrow_w, strip.h),
            "◀",
            theme.font.small - 1.0,
            lane_sel,
            if lane_sel {
                theme.accent.focus
            } else {
                theme.text.muted
            },
            TextAlign::Center,
        );
    }
    if has_right {
        dl.text(
            Rect::new(strip.right() - arrow_w, strip.y, arrow_w, strip.h),
            "▶",
            theme.font.small - 1.0,
            lane_sel,
            if lane_sel {
                theme.accent.focus
            } else {
                theme.text.muted
            },
            TextAlign::Center,
        );
    }

    for (idx, part) in visible.iter().enumerate() {
        let cx = cards_x + idx as f32 * (card_w + gap);
        let cr = Rect::new(cx, strip.y, card_w, strip.h);
        let is_cur = cursor_active && lane_sel && part.slot_num.saturating_sub(1) == clamped_cursor;
        draw_part_card(dl, cr, part, is_cur, zoomed, theme);
    }
}

fn draw_part_card(
    dl: &mut DisplayList,
    cr: Rect,
    part: &BankPartEntry,
    is_cur: bool,
    zoomed: bool,
    theme: &Theme,
) {
    let (bg, border, glow, st_long, st_short, is_staged) = if part.staged_launch {
        (
            theme.accent.modulation.with_alpha(0.22),
            theme.accent.modulation,
            0.30,
            "◇ STAGED".to_string(),
            "◇STG".to_string(),
            true,
        )
    } else if part.staged_release {
        (
            theme.accent.alert.with_alpha(0.20),
            theme.accent.alert,
            0.26,
            "□ RELEASE".to_string(),
            "□REL".to_string(),
            true,
        )
    } else {
        match part.status {
            BankPartStatus::Overriding => (
                theme.accent.override_bar.with_alpha(0.24),
                theme.accent.override_bar,
                0.32,
                "▶ LIVE".to_string(),
                "▶LIVE".to_string(),
                false,
            ),
            BankPartStatus::Arranged => (
                theme.accent.playhead.with_alpha(0.18),
                theme.accent.playhead,
                0.18,
                "▶ ARR".to_string(),
                "▶ARR".to_string(),
                false,
            ),
            BankPartStatus::PendingLaunch { bar } => (
                theme.accent.warning_key.with_alpha(0.22),
                theme.accent.warning_key,
                0.28,
                format!("◆ B{bar:02}"),
                format!("◆{bar}"),
                false,
            ),
            BankPartStatus::Idle => (
                if is_cur {
                    theme.surface.selection
                } else {
                    theme.surface.recessed
                },
                if is_cur {
                    theme.accent.focus
                } else {
                    theme.border.subtle
                },
                if is_cur { 0.14 } else { 0.0 },
                "ready".to_string(),
                String::new(),
                false,
            ),
        }
    };

    let radius = if zoomed {
        theme.radii.pill
    } else {
        theme.radii.badge
    };
    if is_staged {
        dl.rounded_glow(cr, bg, radius, glow);
        draw_dashed_rect(
            dl,
            cr,
            if is_cur { theme.accent.focus } else { border },
            5.0,
            3.0,
            if is_cur { 1.6 } else { 1.2 },
        );
    } else {
        dl.panel(
            cr,
            bg,
            if is_cur { theme.accent.focus } else { border },
            radius,
            if is_cur { 1.6 } else { 1.0 },
            glow,
        );
    }

    let phase_tag = format_phase_offset_badge(part.phase, part.offset_steps);
    let pad = 6.0;
    let inner_w = (cr.w - pad * 2.0).max(0.0);

    if cr.h >= 30.0 {
        let top_h = if zoomed { 19.0 } else { 15.0 };
        let top_y = cr.y + if zoomed { 5.0 } else { 2.0 };
        let max_c = (inner_w / theme.char_width(theme.font.small)) as usize;
        dl.text(
            Rect::new(cr.x + pad, top_y, inner_w, top_h),
            trim_label(&format!("{} {}", part.slot_num, part.part_name), max_c),
            theme.font.small,
            true,
            if is_cur {
                theme.accent.focus
            } else {
                theme.text.primary
            },
            TextAlign::Left,
        );

        let bot_y = top_y + top_h + if zoomed { 2.0 } else { 0.0 };
        let bot_h = (cr.bottom() - bot_y - 2.0).max(11.0);
        let status_lbl = if zoomed { &st_long } else { &st_short };
        let status_txt = if status_lbl.is_empty() {
            if let Some(pt) = &phase_tag {
                pt.as_str()
            } else {
                "ready"
            }
        } else {
            status_lbl.as_str()
        };
        let show_right_phase = zoomed && phase_tag.is_some() && !status_lbl.is_empty();
        let phase_w = if show_right_phase {
            let pt = phase_tag.as_deref().unwrap_or("");
            (pt.chars().count() as f32 * theme.char_width(theme.font.small - 1.0) + 4.0)
                .min(inner_w * 0.45)
        } else {
            0.0
        };
        let st_max =
            ((inner_w - phase_w).max(0.0) / theme.char_width(theme.font.small - 1.0)) as usize;
        dl.text(
            Rect::new(cr.x + pad, bot_y, (inner_w - phase_w).max(0.0), bot_h),
            trim_label(status_txt, st_max),
            theme.font.small - 1.0,
            is_staged || part.status != BankPartStatus::Idle,
            if !is_staged && part.status == BankPartStatus::Idle {
                theme.text.muted
            } else {
                border
            },
            TextAlign::Left,
        );
        if let (true, Some(pt)) = (show_right_phase, phase_tag.as_deref()) {
            dl.text(
                Rect::new(cr.right() - pad - phase_w, bot_y, phase_w, bot_h),
                pt,
                theme.font.small - 1.0,
                false,
                theme.text.secondary,
                TextAlign::Right,
            );
        }
    } else {
        let badge = if !st_short.is_empty() {
            Some((st_short.as_str(), border))
        } else {
            phase_tag.as_deref().map(|t| (t, theme.text.muted))
        };
        let badge_w = badge.map_or(0.0, |(txt, _)| {
            (txt.chars().count() as f32 * theme.char_width(theme.font.small - 1.0) + 4.0)
                .min(inner_w * 0.45)
        });
        let name_w = (inner_w - badge_w).max(0.0);
        let max_c = (name_w / theme.char_width(theme.font.small)) as usize;
        dl.text(
            Rect::new(cr.x + pad, cr.y, name_w, cr.h),
            trim_label(&format!("{} {}", part.slot_num, part.part_name), max_c),
            theme.font.small,
            is_cur || part.status != BankPartStatus::Idle,
            if is_cur {
                theme.accent.focus
            } else {
                theme.text.primary
            },
            TextAlign::Left,
        );
        if let Some((txt, col)) = badge {
            dl.text(
                Rect::new(cr.right() - pad - badge_w, cr.y, badge_w, cr.h),
                txt,
                theme.font.small - 1.0,
                true,
                col,
                TextAlign::Right,
            );
        }
    }
}

fn visible_part_slice(
    lane: &BankLaneGroup,
    lane_sel: bool,
    selected_part_idx: usize,
    strip_w: f32,
    min_card_w: f32,
    gap: f32,
) -> (&[BankPartEntry], bool, bool) {
    let clamped_cursor = selected_part_idx.min(lane.total_parts.saturating_sub(1));
    let focus_local = if lane_sel {
        lane.parts
            .iter()
            .position(|p| p.slot_num.saturating_sub(1) == clamped_cursor)
            .unwrap_or(0)
    } else {
        lane.parts
            .iter()
            .position(|p| {
                p.staged_launch
                    || p.staged_release
                    || matches!(
                        p.status,
                        BankPartStatus::PendingLaunch { .. }
                            | BankPartStatus::Overriding
                            | BankPartStatus::Arranged
                    )
            })
            .unwrap_or(0)
    };
    let arrow_w = 12.0;
    let raw_fit = ((strip_w + gap) / (min_card_w + gap)).floor().max(1.0) as usize;
    let needs_scroll = lane.parts.len() > raw_fit
        || lane.part_window_start > 0
        || lane.part_window_start + lane.parts.len() < lane.total_parts;
    let inner_w = if needs_scroll {
        (strip_w - arrow_w * 2.0).max(min_card_w * 0.75)
    } else {
        strip_w
    };
    let fit_slots = ((inner_w + gap) / (min_card_w + gap)).floor().max(1.0) as usize;
    let local_start = if lane.parts.len() <= fit_slots {
        0
    } else {
        focus_local
            .saturating_sub(fit_slots / 2)
            .min(lane.parts.len() - fit_slots)
    };
    let visible = &lane.parts[local_start..(local_start + fit_slots).min(lane.parts.len())];
    let has_left = lane.part_window_start + local_start > 0;
    let has_right = lane.part_window_start + local_start + visible.len() < lane.total_parts;
    (visible, has_left, has_right)
}

fn format_phase_offset_badge(phase: PhaseMode, offset_steps: i32) -> Option<String> {
    match (phase, offset_steps) {
        (PhaseMode::Song, 0) => None,
        (PhaseMode::Restart, 0) => Some("RST".into()),
        (PhaseMode::Song, off) => Some(format!("{off:+}s")),
        (PhaseMode::Restart, off) => Some(format!("RST {off:+}s")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadence_app::view::BankPatternItem;

    fn sample_part(slot: usize, name: &str, status: BankPartStatus) -> BankPartEntry {
        BankPartEntry {
            slot_num: slot,
            part_id: slot as u64,
            part_name: name.into(),
            phase: PhaseMode::Song,
            offset_steps: 0,
            status,
            staged_launch: false,
            staged_release: false,
        }
    }

    fn sample_lane(id: u64, key: char, name: &str, parts: Vec<BankPartEntry>) -> BankLaneGroup {
        let total = parts.len();
        BankLaneGroup {
            lane_id: id,
            lane_name: name.into(),
            key_char: key,
            muted: false,
            solo: false,
            staged_mute: None,
            staged_solo: None,
            staged_release: false,
            total_parts: total,
            part_window_start: 0,
            parts,
        }
    }

    fn sample_bank_view(mode: BankMode, zoomed: bool, lanes: Vec<BankLaneGroup>) -> BankDeckView {
        BankDeckView {
            mode,
            zoomed,
            jam_active: false,
            staging_count: None,
            selected_lane_idx: 0,
            selected_part_idx: 0,
            sections: Vec::new(),
            lanes,
            patterns: vec![BankPatternItem {
                id: 1,
                name: "Lead Hook".into(),
                kind_label: "Notes",
                period: 16,
                selected: true,
            }],
            total_patterns: 1,
            search_query: None,
        }
    }

    #[test]
    fn format_phase_offset_badge_covers_all_phase_and_offset_combinations() {
        assert_eq!(format_phase_offset_badge(PhaseMode::Song, 0), None);
        assert_eq!(
            format_phase_offset_badge(PhaseMode::Restart, 0).as_deref(),
            Some("RST")
        );
        assert_eq!(
            format_phase_offset_badge(PhaseMode::Song, 4).as_deref(),
            Some("+4s")
        );
        assert_eq!(
            format_phase_offset_badge(PhaseMode::Song, -2).as_deref(),
            Some("-2s")
        );
        assert_eq!(
            format_phase_offset_badge(PhaseMode::Restart, 8).as_deref(),
            Some("RST +8s")
        );
        assert_eq!(
            format_phase_offset_badge(PhaseMode::Restart, -4).as_deref(),
            Some("RST -4s")
        );
    }

    #[test]
    fn visible_part_slice_scrolls_around_cursor_or_active_part_and_reports_edges() {
        let mut parts: Vec<BankPartEntry> = (1..=10)
            .map(|i| sample_part(i, &format!("P{i}"), BankPartStatus::Idle))
            .collect();
        parts[7].status = BankPartStatus::Overriding;
        let lane = sample_lane(1, 'q', "Lead", parts);

        // Wide strip fits all 10 cards with no scroll arrows.
        let (all, left, right) = visible_part_slice(&lane, true, 0, 1200.0, 80.0, 4.0);
        assert_eq!(all.len(), 10);
        assert!(!left && !right);

        // Narrow strip (280 px, min 80 + gap 4 -> fits 3 cards after 24 px arrow reserve).
        let (start_vis, l0, r0) = visible_part_slice(&lane, true, 0, 280.0, 80.0, 4.0);
        assert!(!l0 && r0);
        assert_eq!(start_vis.first().unwrap().slot_num, 1);

        let (mid_vis, lm, rm) = visible_part_slice(&lane, true, 5, 280.0, 80.0, 4.0);
        assert!(lm && rm);
        assert!(mid_vis.iter().any(|p| p.slot_num == 6));

        let (end_vis, le, re) = visible_part_slice(&lane, true, 9, 280.0, 80.0, 4.0);
        assert!(le && !re);
        assert_eq!(end_vis.last().unwrap().slot_num, 10);

        // Unselected lane centers on active Overriding part (slot 8) instead of cursor 0.
        let (unsel_vis, lu, ru) = visible_part_slice(&lane, false, 0, 280.0, 80.0, 4.0);
        assert!(lu && ru);
        assert!(unsel_vis.iter().any(|p| p.slot_num == 8));

        // Pre-windowed lane (`part_window_start > 0` / `total_parts > len`) still reports outer edges.
        let mut windowed = sample_lane(
            2,
            'w',
            "Bass",
            vec![
                sample_part(5, "P5", BankPartStatus::Idle),
                sample_part(6, "P6", BankPartStatus::Idle),
            ],
        );
        windowed.part_window_start = 4;
        windowed.total_parts = 12;
        let (_, wl, wr) = visible_part_slice(&windowed, true, 4, 400.0, 80.0, 4.0);
        assert!(wl && wr);
    }

    #[test]
    fn compact_vs_zoomed_card_height_threshold_and_phase_badge_layout() {
        let theme = Theme::cadence_dark();
        let mut p1 = sample_part(1, "Drop Lead", BankPartStatus::Overriding);
        p1.phase = PhaseMode::Restart;
        p1.offset_steps = 4;
        let lane = sample_lane(1, 'q', "Lead", vec![p1]);

        // Zoomed full-screen matrix (row_h = 56 -> card_h = 50 >= 30): 2-line layout with long status + right phase tag.
        let zoomed_view = sample_bank_view(BankMode::Parts, true, vec![lane.clone()]);
        let mut dl_zoom = DisplayList::new(1100.0, 520.0, 1.0);
        draw_bank(
            &mut dl_zoom,
            Rect::new(0.0, 0.0, 1100.0, 520.0),
            &zoomed_view,
            true,
            UiModeKind::Normal,
            &theme,
        );
        assert_eq!(dl_zoom.current_clip(), dl_zoom.viewport);
        assert!(dl_zoom.base.texts.iter().any(|t| t.text == "▶ LIVE"));
        assert!(dl_zoom
            .base
            .texts
            .iter()
            .any(|t| t.text == "RST +4s" && t.align == TextAlign::Right));

        // Compact Material-focused Bank (parts_active = false -> row_h = 26 -> card_h = 22 < 30):
        // single-line card layout with compact status badge `▶LIVE` aligned right on the same row.
        let compact_view = sample_bank_view(BankMode::Material, false, vec![lane]);
        let mut dl_compact = DisplayList::new(320.0, 460.0, 1.0);
        draw_bank(
            &mut dl_compact,
            Rect::new(0.0, 0.0, 320.0, 460.0),
            &compact_view,
            true,
            UiModeKind::Normal,
            &theme,
        );
        assert_eq!(dl_compact.current_clip(), dl_compact.viewport);
        let title_txt = dl_compact
            .base
            .texts
            .iter()
            .find(|t| t.text.contains("Drop"))
            .expect("compact part card title");
        let badge_txt = dl_compact
            .base
            .texts
            .iter()
            .find(|t| t.text == "▶LIVE")
            .expect("compact single-line status badge");
        assert!(
            (title_txt.rect.y - badge_txt.rect.y).abs() < 0.5,
            "compact card (< 30 px tall) must place title and status badge on a single line"
        );
    }

    #[test]
    fn empty_lane_rows_and_tiny_viewports_balance_clip_stack() {
        let theme = Theme::cadence_dark();
        let lanes = vec![
            sample_lane(1, 'q', "Drums", Vec::new()),
            sample_lane(2, 'w', "Bass", Vec::new()),
        ];
        let view = sample_bank_view(BankMode::Parts, true, lanes);
        let mut dl = DisplayList::new(960.0, 400.0, 1.0);
        draw_bank(
            &mut dl,
            Rect::new(0.0, 0.0, 960.0, 400.0),
            &view,
            true,
            UiModeKind::Normal,
            &theme,
        );
        assert_eq!(dl.current_clip(), dl.viewport);
        assert!(dl
            .base
            .texts
            .iter()
            .any(|t| t.text.contains("empty · press a to add part")));
        assert!(dl.base.texts.iter().any(|t| t.text == "(empty)"));

        // Zero lanes in zoomed view renders clean empty-lanes prompt and balances clips.
        let mut zero_lanes = sample_bank_view(BankMode::Parts, true, Vec::new());
        zero_lanes.patterns.clear();
        zero_lanes.total_patterns = 0;
        let mut dl_zero = DisplayList::new(960.0, 400.0, 1.0);
        draw_bank(
            &mut dl_zero,
            Rect::new(0.0, 0.0, 960.0, 400.0),
            &zero_lanes,
            true,
            UiModeKind::Normal,
            &theme,
        );
        assert_eq!(dl_zero.current_clip(), dl_zero.viewport);
        assert!(dl_zero
            .base
            .texts
            .iter()
            .any(|t| t.text.contains("No lanes yet")));

        // Tiny area early-return and empty material mode both leave clip stack balanced.
        let mut dl_tiny = DisplayList::new(60.0, 40.0, 1.0);
        draw_bank(
            &mut dl_tiny,
            Rect::new(0.0, 0.0, 60.0, 40.0),
            &view,
            true,
            UiModeKind::Normal,
            &theme,
        );
        assert_eq!(dl_tiny.current_clip(), dl_tiny.viewport);

        let mut empty_mat = sample_bank_view(BankMode::Material, false, Vec::new());
        empty_mat.patterns.clear();
        empty_mat.total_patterns = 0;
        let mut dl_mat = DisplayList::new(320.0, 360.0, 1.0);
        draw_bank(
            &mut dl_mat,
            Rect::new(0.0, 0.0, 320.0, 360.0),
            &empty_mat,
            true,
            UiModeKind::Normal,
            &theme,
        );
        assert_eq!(dl_mat.current_clip(), dl_mat.viewport);
        assert!(dl_mat
            .base
            .texts
            .iter()
            .any(|t| t.text.contains("No material yet")));
    }
}
