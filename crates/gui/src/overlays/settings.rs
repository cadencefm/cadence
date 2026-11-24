//! Global Settings & Project Live Timing (`g,`) modal overlay.
use super::overlay_frame;
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::Theme;

pub(super) fn draw_settings(
    dl: &mut DisplayList,
    rect: Rect,
    view: &cadence_app::view::SettingsView,
    theme: &Theme,
) {
    let title = if view.dirty {
        "CADENCE  ·  SETTINGS & LIVE TIMING  [unsaved]"
    } else {
        "CADENCE  ·  SETTINGS & LIVE TIMING"
    };
    let inner = overlay_frame(dl, rect, title, theme.accent.focus, theme);
    dl.push_clip(inner);

    let footer_h = if view.editing_prompt.is_some() {
        78.0
    } else {
        46.0
    };
    let (body, footer) = inner.split_bottom(footer_h);

    // Pre-compute vertical layout for all groups and rows so the viewport scrolls smoothly
    // around the active cursor without ever clipping the selected row or its group header.
    enum Item<'a> {
        Header(&'static str),
        Row(&'a cadence_app::view::SettingsRowView),
    }
    let mut items: Vec<(f32, f32, Item<'_>)> = Vec::with_capacity(view.rows.len() + 6);
    let mut cursor_y = 2.0_f32;
    let mut selected_top = 0.0_f32;
    let mut selected_bottom = 44.0_f32;
    let mut last_group = "";
    for row in &view.rows {
        let mut group_top = cursor_y;
        if row.group != last_group {
            if !last_group.is_empty() {
                cursor_y += 6.0;
            }
            group_top = cursor_y;
            items.push((cursor_y, 18.0, Item::Header(row.group)));
            cursor_y += 18.0;
            last_group = row.group;
        }
        if row.selected {
            selected_top = group_top;
            selected_bottom = cursor_y + 42.0;
        }
        items.push((cursor_y, 42.0, Item::Row(row)));
        cursor_y += 46.0;
    }
    let total_h = cursor_y;
    let max_scroll = (total_h - body.h).max(0.0);
    let scroll_y = if max_scroll <= 0.0 {
        0.0
    } else {
        let desired =
            ((selected_top + selected_bottom) * 0.5 - body.h * 0.5).clamp(0.0, max_scroll);
        // Ensure both selected row and its header remain inside the viewport.
        if selected_bottom - desired > body.h {
            (selected_bottom - body.h + 4.0).clamp(0.0, max_scroll)
        } else if selected_top < desired {
            selected_top.clamp(0.0, max_scroll)
        } else {
            desired
        }
    };

    dl.push_clip(body);
    for (item_y, item_h, item) in items {
        let y = body.y + item_y - scroll_y;
        if y + item_h < body.y - 2.0 || y > body.bottom() + 2.0 {
            continue;
        }
        match item {
            Item::Header(group) => {
                dl.text(
                    Rect::new(body.x + 2.0, y, body.w, 16.0),
                    group,
                    theme.font.small - 1.0,
                    true,
                    theme.accent.routing,
                    TextAlign::Left,
                );
            }
            Item::Row(row) => {
                let r = Rect::new(body.x, y, body.w, 42.0);
                dl.panel(
                    r,
                    if row.selected {
                        theme.surface.selection
                    } else {
                        theme.surface.recessed.with_alpha(0.70)
                    },
                    if row.selected {
                        theme.accent.focus
                    } else {
                        theme.border.subtle
                    },
                    theme.radii.pill,
                    if row.selected { 1.4 } else { 1.0 },
                    if row.selected { 0.18 } else { 0.0 },
                );
                let (left, right) = r.inset(10.0, 4.0).split_right(260.0);
                dl.text(
                    Rect::new(left.x, left.y, left.w, 18.0),
                    row.label.as_str(),
                    theme.font.body,
                    row.selected,
                    if row.selected {
                        theme.text.primary
                    } else {
                        theme.text.secondary
                    },
                    TextAlign::Left,
                );
                let max_detail =
                    ((left.w - 4.0) / theme.char_width(theme.font.small - 1.0)).max(8.0) as usize;
                dl.text(
                    Rect::new(left.x, left.y + 18.0, left.w, 16.0),
                    trim_label(&row.detail, max_detail),
                    theme.font.small - 1.0,
                    false,
                    theme.text.muted,
                    TextAlign::Left,
                );
                let val_pill = Rect::new(right.x + 6.0, right.y + 4.0, right.w - 6.0, 26.0);
                dl.panel(
                    val_pill,
                    if row.selected {
                        theme.accent.focus.with_alpha(0.18)
                    } else {
                        theme.surface.elevated
                    },
                    if row.selected {
                        theme.accent.focus
                    } else {
                        theme.border.hairline
                    },
                    theme.radii.badge,
                    1.0,
                    if row.selected { 0.12 } else { 0.0 },
                );
                let max_val =
                    ((val_pill.w - 12.0) / theme.char_width(theme.font.small)).max(8.0) as usize;
                dl.text(
                    val_pill.inset(6.0, 0.0),
                    trim_label(&row.value, max_val),
                    theme.font.small,
                    true,
                    if row.selected {
                        theme.accent.focus
                    } else {
                        theme.text.primary
                    },
                    TextAlign::Center,
                );
            }
        }
    }
    dl.pop_clip();

    let footer = if let Some((prompt, buf)) = &view.editing_prompt {
        let (edit_row, rest_footer) = footer.split_top(30.0);
        let edit_box = Rect::new(edit_row.x, edit_row.y + 2.0, edit_row.w, 24.0);
        dl.panel(
            edit_box,
            theme.accent.focus.with_alpha(0.16),
            theme.accent.focus,
            theme.radii.badge,
            1.2,
            0.20,
        );
        let max_edit =
            ((edit_box.w - 16.0) / theme.char_width(theme.font.small)).max(12.0) as usize;
        dl.text(
            edit_box.inset(8.0, 0.0),
            trim_label(
                &format!("{prompt}: {buf}▏  (Enter add · Ctrl+u clear · Esc cancel)"),
                max_edit,
            ),
            theme.font.small,
            true,
            theme.text.primary,
            TextAlign::Left,
        );
        rest_footer
    } else {
        footer
    };
    let (path_row, hint_row) = footer.split_top(20.0);
    let pos_text = format!("{}/{}", view.cursor + 1, view.rows.len().max(1));
    let (path_left, pos_right) = path_row.split_right(54.0);
    let max_p = (path_left.w / theme.char_width(theme.font.small - 1.0)) as usize;
    dl.text(
        path_left,
        trim_label(&format!("Config file: {}", view.config_path), max_p),
        theme.font.small - 1.0,
        false,
        theme.text.muted,
        TextAlign::Left,
    );
    dl.text(
        pos_right,
        pos_text,
        theme.font.small - 1.0,
        false,
        theme.text.muted,
        TextAlign::Right,
    );
    let max_h = (hint_row.w / theme.char_width(theme.font.small)) as usize;
    dl.text(
        hint_row,
        trim_label(
            "j/k · w/b · Alt+h/l ,/. adjust · Enter exact · Alt+Bksp reset · a/x dir · F Finder · Ctrl+s save · Esc",
            max_h,
        ),
        theme.font.small,
        true,
        theme.accent.warning_key,
        TextAlign::Left,
    );
    dl.pop_clip();
}
