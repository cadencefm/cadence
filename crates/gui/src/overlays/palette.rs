//! Fuzzy Action Palette (`:` and `Ctrl+Shift+p` -> `Mode::Palette`) GPU modal overlay.
//! Categorized live-filtered actions with shortcut pills + power-user Ex command passthrough.
use super::{overlay_frame, scroll_window_centered};
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::{Color, Theme};
use cadence_app::PaletteView;

fn category_color(category: &str, theme: &Theme) -> Color {
    match category {
        "CMD" => theme.accent.warning_key,
        "SONG" | "AUTHOR" | "PATTERN" => theme.accent.routing,
        "SOUND" | "FX" | "INSTRUMENT" => theme.accent.modulation,
        "TRANSPORT" | "SECTION" => theme.accent.playhead,
        "MIXER" | "EDIT" => theme.accent.edit,
        _ => theme.accent.focus,
    }
}

pub(super) fn draw_palette(dl: &mut DisplayList, rect: Rect, view: &PaletteView, theme: &Theme) {
    let frame_accent = if view.command_passthrough && view.cursor == 0 {
        theme.accent.warning_key
    } else {
        theme.accent.focus
    };
    let inner = overlay_frame(
        dl,
        rect,
        "ACTION PALETTE  ·  : / Ctrl+Shift+p",
        frame_accent,
        theme,
    );
    dl.push_clip(inner);

    let (input_row, rest) = inner.split_top(38.0);
    let (list_area, footer) = rest.split_bottom(24.0);

    // Search / command input box.
    let box_rect = Rect::new(input_row.x, input_row.y, input_row.w, 32.0);
    let box_accent = if view.command_passthrough && view.cursor == 0 {
        theme.accent.warning_key
    } else {
        theme.accent.focus
    };
    dl.panel(
        box_rect,
        theme.surface.recessed,
        box_accent,
        theme.radii.pill,
        1.5,
        0.22,
    );
    let count_text = if view.command_passthrough {
        format!(
            "CMD · {} match{}",
            view.total_matches,
            if view.total_matches == 1 { "" } else { "es" }
        )
    } else {
        format!(
            "{} action{}",
            view.total_matches,
            if view.total_matches == 1 { "" } else { "s" }
        )
    };
    let cw_small = theme.char_width(theme.font.small);
    let count_w = (count_text.chars().count() as f32 * cw_small + 16.0).clamp(76.0, 150.0);
    let (text_rect, count_rect) = box_rect.inset(10.0, 0.0).split_right(count_w);
    if view.query.is_empty() {
        dl.text(
            text_rect,
            "> Type an action, deck, FX, section, or Ex command…",
            theme.font.body,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
    } else {
        let cw_body = theme.char_width(theme.font.body);
        let max_q = ((text_rect.w - 16.0) / cw_body).max(12.0) as usize;
        dl.text(
            text_rect,
            format!("> {}▏", trim_label(&view.query, max_q)),
            theme.font.body,
            true,
            theme.text.primary,
            TextAlign::Left,
        );
    }
    dl.text(
        count_rect,
        count_text,
        theme.font.small,
        true,
        if view.command_passthrough {
            theme.accent.warning_key
        } else {
            theme.text.muted
        },
        TextAlign::Right,
    );

    // Filtered categorized list.
    let list_box = list_area.inset(0.0, 4.0);
    dl.push_clip(list_box);
    if view.items.is_empty() {
        dl.text(
            Rect::new(list_box.x + 8.0, list_box.y + 12.0, list_box.w - 16.0, 22.0),
            format!(
                "No matching actions for '{}' · press Enter to try as Ex command",
                view.query
            ),
            theme.font.small,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
    } else {
        let row_h = 28.0;
        let visible = ((list_box.h / row_h).floor() as usize).max(1);
        let first = scroll_window_centered(view.cursor, view.items.len(), visible);
        for (idx, item) in view.items.iter().enumerate().skip(first).take(visible) {
            let y = list_box.y + (idx - first) as f32 * row_h;
            let row = Rect::new(list_box.x, y, list_box.w, row_h - 2.0);
            let cat_col = category_color(item.category, theme);

            if item.selected {
                let sel_accent = if item.is_command {
                    theme.accent.warning_key
                } else {
                    theme.accent.focus
                };
                dl.panel(
                    row,
                    if item.is_command {
                        theme.accent.warning_key.with_alpha(0.16)
                    } else {
                        theme.surface.selection
                    },
                    sel_accent,
                    theme.radii.badge,
                    1.2,
                    0.18,
                );
            } else if idx % 2 == 1 {
                dl.rounded(
                    row,
                    theme.surface.recessed.with_alpha(0.45),
                    theme.radii.badge,
                );
            }

            // Left category badge (generous width for 10-char "INSTRUMENT" / "TRANSPORT").
            let cat_w = 88.0;
            let cat_rect = Rect::new(row.x + 6.0, row.y + 4.0, cat_w, row.h - 8.0);
            dl.rounded(cat_rect, cat_col.with_alpha(0.18), theme.radii.badge);
            dl.text(
                cat_rect,
                item.category,
                theme.font.small - 0.5,
                true,
                cat_col,
                TextAlign::Center,
            );

            // Right shortcut pill inside a fixed gutter so every label & detail column aligns vertically.
            let sc_gutter = 92.0;
            let sc_w = if item.shortcut.is_empty() {
                0.0
            } else {
                (item.shortcut.chars().count() as f32 * cw_small + 14.0).clamp(38.0, sc_gutter)
            };
            if sc_w > 0.0 {
                let sc_rect = Rect::new(row.right() - sc_w - 6.0, row.y + 4.0, sc_w, row.h - 8.0);
                dl.panel(
                    sc_rect,
                    theme.surface.elevated,
                    theme.border.subtle,
                    theme.radii.badge,
                    1.0,
                    0.0,
                );
                dl.text(
                    sc_rect,
                    item.shortcut,
                    theme.font.small - 0.5,
                    true,
                    theme.accent.warning_key,
                    TextAlign::Center,
                );
            }

            // Action label + muted detail anchored on consistent vertical column guides.
            let content_x = cat_rect.right() + 10.0;
            let content_right = row.right() - sc_gutter - 14.0;
            let content_w = (content_right - content_x).max(0.0);
            let label_w = if item.is_command {
                content_w
            } else {
                (content_w * 0.48).clamp(180.0, 290.0)
            };
            let max_lbl = ((label_w - 4.0) / cw_small).max(8.0) as usize;
            dl.text(
                Rect::new(content_x, row.y, label_w, row.h),
                trim_label(&item.label, max_lbl),
                theme.font.small,
                item.selected || item.is_command,
                if item.is_command {
                    theme.accent.warning_key
                } else if item.selected {
                    theme.accent.focus
                } else {
                    theme.text.primary
                },
                TextAlign::Left,
            );

            if !item.is_command {
                let det_x = content_x + label_w + 8.0;
                let det_w = (content_right - det_x).max(0.0);
                if det_w > 40.0 {
                    let max_det = (det_w / cw_small).max(6.0) as usize;
                    dl.text(
                        Rect::new(det_x, row.y, det_w, row.h),
                        trim_label(&item.detail, max_det),
                        theme.font.small - 0.5,
                        false,
                        theme.text.muted,
                        TextAlign::Left,
                    );
                }
            }
        }
    }
    dl.pop_clip();

    // Footer hints.
    dl.fill(
        Rect::new(footer.x, footer.y, footer.w, 1.0),
        theme.border.hairline,
    );
    dl.text(
        Rect::new(footer.x, footer.y + 4.0, footer.w, footer.h - 4.0),
        "type fuzzy search or Ex command  ·  ↑/↓ Ctrl+n/p Tab select  ·  Enter run  ·  Ctrl+u clear  ·  Esc close",
        theme.font.small,
        true,
        theme.accent.warning_key,
        TextAlign::Left,
    );
    dl.pop_clip();
}
