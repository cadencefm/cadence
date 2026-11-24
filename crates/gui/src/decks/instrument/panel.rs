//! Instrument `Panel` tab: scrollable horizontal module pill strip (`Quick` + zone modules),
//! dedicated module visualizer pane, and normalized parameter control list.
use super::module_tag_accent;
use super::visuals::draw_module_visual;
use crate::decks::object::draw_control_list;
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::{Color, Theme};
use cadence_app::view::{InstrumentDeckView, RackDeckView, RackModulePillView};

pub(super) fn draw_panel_tab(
    dl: &mut DisplayList,
    body: Rect,
    view: &InstrumentDeckView,
    rack: &RackDeckView,
    theme: &Theme,
) {
    let (pill_row, rest) = if !rack.modules.is_empty() && body.h >= 120.0 {
        let (p, r) = body.split_top(28.0);
        (Some(p), r)
    } else {
        (None, body)
    };

    if let Some(pr) = pill_row {
        draw_module_pill_strip(dl, pr, &rack.modules, theme);
    }

    let max_vis = (rest.h - 72.0).max(74.0);
    let vis_h = (rest.h * 0.38).clamp(90.0, 178.0).min(max_vis);
    let (vis_outer, ctrl_area) = rest.split_top(vis_h);
    let vis_area = Rect::new(
        vis_outer.x,
        vis_outer.y,
        vis_outer.w,
        (vis_outer.h - 6.0).max(0.0),
    );

    draw_module_visual(dl, vis_area, &rack.visual, theme);
    draw_control_list(dl, ctrl_area, &view.controls, view.control_cursor, theme);
}

fn draw_module_pill_strip(
    dl: &mut DisplayList,
    row: Rect,
    pills: &[RackModulePillView],
    theme: &Theme,
) {
    if row.is_empty() || pills.is_empty() {
        return;
    }
    dl.push_clip(row);

    let font = theme.font.small;
    let tag_font = theme.font.small - 2.0;
    let cw = theme.char_width(font);
    let tcw = theme.char_width(tag_font);
    let gap = 5.0;
    let pill_h = (row.h - 6.0).clamp(17.0, 22.0);
    let pill_y = row.y + 2.0;

    let widths: Vec<f32> = pills
        .iter()
        .map(|p| {
            let tag_w = p.tag.chars().count() as f32 * tcw + 8.0;
            let lbl_w = p.label.chars().count() as f32 * cw;
            (tag_w + lbl_w + 18.0).clamp(58.0, 180.0)
        })
        .collect();
    let active_idx = pills.iter().position(|p| p.active).unwrap_or(0);
    let (start, end) = pill_scroll_window(&widths, active_idx, row.w, gap);

    let mut x = row.x;
    if start > 0 {
        let r = Rect::new(x, pill_y, 14.0, pill_h);
        dl.text(r, "◀", tag_font, true, theme.text.muted, TextAlign::Center);
        x += 16.0;
    }

    let right_limit = if end < pills.len() {
        row.right() - 18.0
    } else {
        row.right()
    };

    for idx in start..end {
        let p = &pills[idx];
        let accent = pill_accent(p, theme);
        let pw = widths[idx].min((right_limit - x).max(0.0));
        if pw < 32.0 {
            break;
        }
        let pr = Rect::new(x, pill_y, pw, pill_h);
        if p.active {
            dl.panel(
                pr,
                accent.with_alpha(0.20),
                accent,
                theme.radii.badge,
                1.3,
                0.18,
            );
        } else {
            dl.panel(
                pr,
                theme.surface.recessed,
                accent.with_alpha(0.32),
                theme.radii.badge,
                1.0,
                0.0,
            );
            dl.rounded(
                Rect::new(pr.x + 1.0, pr.y + 3.0, 2.5, (pr.h - 6.0).max(4.0)),
                accent.with_alpha(0.75),
                1.0,
            );
        }

        let tag_w = (p.tag.chars().count() as f32 * tcw + 6.0).min(pw * 0.42);
        let tag_rect = Rect::new(pr.x + 6.0, pr.y + 3.0, tag_w, (pr.h - 6.0).max(10.0));
        dl.rounded(
            tag_rect,
            accent.with_alpha(if p.active { 0.30 } else { 0.14 }),
            theme.radii.badge,
        );
        dl.text(
            tag_rect,
            p.tag,
            tag_font,
            true,
            if p.active {
                accent
            } else {
                accent.with_alpha(0.90)
            },
            TextAlign::Center,
        );

        let lbl_x = tag_rect.right() + 5.0;
        let lbl_w = (pr.right() - lbl_x - 5.0).max(0.0);
        if lbl_w >= 14.0 {
            let max_c = (lbl_w / cw).floor().max(1.0) as usize;
            dl.text(
                Rect::new(lbl_x, pr.y, lbl_w, pr.h),
                trim_label(&p.label, max_c),
                font,
                p.active,
                if p.active {
                    theme.text.primary
                } else {
                    theme.text.secondary
                },
                TextAlign::Left,
            );
        }

        x += pw + gap;
    }

    if end < pills.len() {
        let r = Rect::new(row.right() - 14.0, pill_y, 14.0, pill_h);
        dl.text(r, "▶", tag_font, true, theme.text.muted, TextAlign::Center);
    }

    dl.pop_clip();
}

/// Window horizontally around `active_idx` when total pill width exceeds `row_w`.
pub(super) fn pill_scroll_window(
    widths: &[f32],
    active_idx: usize,
    row_w: f32,
    gap: f32,
) -> (usize, usize) {
    let total_w: f32 = widths.iter().sum::<f32>() + gap * widths.len().saturating_sub(1) as f32;
    if total_w <= row_w {
        return (0, widths.len());
    }
    let reserve = 36.0; // space for subtle ◀ / ▶ overflow chevrons
    let avail = (row_w - reserve).max(64.0);
    let mut start = active_idx;
    let mut end = active_idx + 1;
    let mut used = widths[active_idx];
    loop {
        let mut grew = false;
        if end < widths.len() && used + gap + widths[end] <= avail {
            used += gap + widths[end];
            end += 1;
            grew = true;
        }
        if start > 0 && used + gap + widths[start - 1] <= avail {
            start -= 1;
            used += gap + widths[start];
            grew = true;
        }
        if !grew {
            break;
        }
    }
    (start, end)
}

fn pill_accent(pill: &RackModulePillView, theme: &Theme) -> Color {
    match pill.kind {
        Some(tag) => module_tag_accent(tag, theme),
        None if pill.module_id.is_none() => theme.accent.warning_key,
        None => match pill.column {
            0 => theme.accent.routing,
            1 => theme.accent.modulation,
            2 => theme.accent.warning_key,
            _ => theme.accent.focus,
        },
    }
}
