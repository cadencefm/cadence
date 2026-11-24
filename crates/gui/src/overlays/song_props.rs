//! Song / Project Properties (`gP` / `Ctrl+Shift+I`) modal overlay — styled cohesively
//! with Settings (modeless direct adjust + inline Title editor).
use super::overlay_frame;
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::Theme;
use cadence_app::SongPropertiesView;

pub(super) fn draw_song_properties(
    dl: &mut DisplayList,
    rect: Rect,
    view: &SongPropertiesView,
    theme: &Theme,
) {
    let active_edit = view.title_editing.is_some();
    let title = match (active_edit, view.dirty) {
        (true, true) => "CADENCE  ·  SONG PROPERTIES [RENAME]  *",
        (true, false) => "CADENCE  ·  SONG PROPERTIES [RENAME]",
        (false, true) => "CADENCE  ·  SONG PROPERTIES  *",
        (false, false) => "CADENCE  ·  SONG PROPERTIES",
    };
    let frame_accent = if active_edit {
        theme.accent.edit
    } else {
        theme.accent.focus
    };
    let inner = overlay_frame(dl, rect, title, frame_accent, theme);
    dl.push_clip(inner);

    let (summary_bar, rest) = inner.split_top(26.0);
    let footer_h = if view.title_editing.is_some() {
        76.0
    } else {
        36.0
    };
    let (body, footer) = rest.split_bottom(footer_h);

    // Compact live project summary ribbon.
    dl.panel(
        Rect::new(summary_bar.x, summary_bar.y, summary_bar.w, 22.0),
        theme.surface.recessed,
        theme.border.subtle,
        theme.radii.badge,
        1.0,
        0.0,
    );
    dl.text(
        Rect::new(
            summary_bar.x + 8.0,
            summary_bar.y,
            summary_bar.w - 16.0,
            22.0,
        ),
        view.summary.as_str(),
        theme.font.small,
        true,
        theme.text.secondary,
        TextAlign::Left,
    );

    // Compute vertical layout with smooth centering around selected row.
    enum Item<'a> {
        Header(&'static str),
        Row(&'a cadence_app::SongPropRowView),
    }
    let mut items: Vec<(f32, f32, Item<'_>)> = Vec::with_capacity(view.rows.len() + 4);
    let mut cursor_y = 4.0_f32;
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
        if selected_bottom - desired > body.h {
            (selected_bottom - body.h + 4.0).clamp(0.0, max_scroll)
        } else if selected_top < desired {
            selected_top.clamp(0.0, max_scroll)
        } else {
            desired
        }
    };

    dl.push_clip(body);
    let small_cw = theme.char_width(theme.font.small);
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
                let editing_row = row.selected && active_edit;
                let row_accent = if editing_row {
                    theme.accent.edit
                } else {
                    theme.accent.focus
                };
                dl.panel(
                    r,
                    if editing_row {
                        theme.accent.edit.with_alpha(0.14)
                    } else if row.selected {
                        theme.surface.selection
                    } else {
                        theme.surface.recessed.with_alpha(0.70)
                    },
                    if row.selected {
                        row_accent
                    } else {
                        theme.border.subtle
                    },
                    theme.radii.pill,
                    if editing_row {
                        2.0
                    } else if row.selected {
                        1.4
                    } else {
                        1.0
                    },
                    if editing_row {
                        theme.glow.edit_border * 0.65
                    } else if row.selected {
                        0.18
                    } else {
                        0.0
                    },
                );
                let (left, right) = r.inset(10.0, 4.0).split_right(260.0);
                dl.text(
                    Rect::new(left.x, left.y, left.w, 18.0),
                    row.label,
                    theme.font.body,
                    row.selected,
                    if editing_row {
                        theme.accent.edit
                    } else if row.selected {
                        theme.accent.focus
                    } else {
                        theme.text.primary
                    },
                    TextAlign::Left,
                );
                let max_detail = ((left.w - 8.0) / small_cw).max(10.0) as usize;
                dl.text(
                    Rect::new(left.x, left.y + 18.0, left.w, 16.0),
                    trim_label(row.detail, max_detail),
                    theme.font.small - 0.5,
                    false,
                    theme.text.muted,
                    TextAlign::Left,
                );

                let val_rect =
                    Rect::new(right.x + 8.0, right.y + 4.0, right.w - 8.0, right.h - 8.0);
                let val_text = if editing_row && row.adjustable {
                    format!("◀ {} ▶", row.value)
                } else {
                    row.value.clone()
                };
                dl.panel(
                    val_rect,
                    if editing_row {
                        theme.accent.edit.with_alpha(0.22)
                    } else if row.selected {
                        theme.accent.focus.with_alpha(0.16)
                    } else {
                        theme.surface.elevated
                    },
                    if editing_row {
                        theme.accent.edit
                    } else if row.selected {
                        theme.accent.focus
                    } else {
                        theme.border.subtle
                    },
                    theme.radii.badge,
                    if editing_row { 1.4 } else { 1.0 },
                    if editing_row { 0.25 } else { 0.0 },
                );
                let max_val = ((val_rect.w - 12.0) / small_cw).max(8.0) as usize;
                dl.text(
                    val_rect.inset(6.0, 0.0),
                    trim_label(&val_text, max_val),
                    theme.font.small,
                    true,
                    if editing_row {
                        theme.accent.edit
                    } else if row.selected {
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

    // Footer + optional inline Title edit input box.
    dl.fill(
        Rect::new(footer.x, footer.y, footer.w, 1.0),
        theme.border.subtle,
    );
    if let Some(buf) = &view.title_editing {
        dl.text(
            Rect::new(footer.x, footer.y + 6.0, footer.w, 16.0),
            "EDIT SONG TITLE (1–120 printable characters)",
            theme.font.small,
            true,
            theme.accent.edit,
            TextAlign::Left,
        );
        let box_rect = Rect::new(footer.x, footer.y + 24.0, footer.w, 28.0);
        dl.panel(
            box_rect,
            theme.surface.recessed,
            theme.accent.edit,
            theme.radii.badge,
            1.4,
            theme.glow.edit_border * 0.45,
        );
        dl.text(
            box_rect.inset(8.0, 0.0),
            format!("> {buf}▏"),
            theme.font.body,
            true,
            theme.text.primary,
            TextAlign::Left,
        );
        dl.text(
            Rect::new(footer.x, footer.bottom() - 18.0, footer.w, 18.0),
            "Enter rename song  ·  Ctrl+u clear  ·  Esc cancel",
            theme.font.small,
            true,
            theme.accent.warning_key,
            TextAlign::Left,
        );
    } else {
        dl.text(
            Rect::new(footer.x, footer.y + 10.0, footer.w, 20.0),
            "j/k row  ·  w/b group  ·  Alt+h/l ,/. adjust  ·  Enter exact/rename  ·  Alt+Bksp reset  ·  u/U undo  ·  Esc close",
            theme.font.small,
            true,
            theme.accent.warning_key,
            TextAlign::Left,
        );
    }
    dl.pop_clip();
}
