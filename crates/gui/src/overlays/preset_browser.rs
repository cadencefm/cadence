//! Instrument Preset Browser (`P` on Instrument deck) GPU modal overlay with live
//! non-destructive preview indicator, `/` search bar, engine badge, and patch inspector.
use super::{overlay_frame, scroll_window_centered};
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::Theme;
use cadence_app::PresetBrowserView;

pub(super) fn draw_preset_browser(
    dl: &mut DisplayList,
    rect: Rect,
    view: &PresetBrowserView,
    theme: &Theme,
) {
    let accent = if view.searching {
        theme.accent.edit
    } else {
        theme.accent.focus
    };
    let title = format!(
        "INSTRUMENT PRESETS  ·  {}  [{}]",
        view.instrument_name, view.engine_label
    );
    let inner = overlay_frame(dl, rect, &title, accent, theme);
    dl.push_clip(inner);

    let has_cats = !view.categories.is_empty();
    let (top, rest) = inner.split_top(if has_cats { 86.0 } else { 58.0 });
    let (body, footer) = rest.split_bottom(24.0);

    // Row 1: Live preview status pill + match count.
    let status_row = Rect::new(top.x, top.y, top.w, 24.0);
    let (pill_text, pill_color, pill_glow) = if let Some(name) = &view.live_preview_name {
        (
            format!("● LIVE PREVIEW: {name}  (non-destructive · Esc restores original)"),
            theme.accent.edit,
            theme.glow.edit_border * 0.40,
        )
    } else {
        (
            "● ORIGINAL PATCH  (j/k previews live · zero undo until Enter)".into(),
            theme.accent.playhead,
            0.16,
        )
    };
    let scope_lbl = if view.active_category == "All" || view.active_category.is_empty() {
        view.engine_label
    } else {
        view.active_category
    };
    let count_text = format!("{}/{} {scope_lbl}", view.items.len(), view.total_count);
    let count_w = (count_text.chars().count() as f32 * theme.char_width(theme.font.small) + 16.0)
        .clamp(84.0, 196.0);
    let (pill_rect, count_rect) = status_row.split_right(count_w);
    let pill_rect = Rect::new(
        pill_rect.x,
        pill_rect.y,
        (pill_rect.w - 8.0).max(0.0),
        pill_rect.h,
    );
    dl.panel(
        pill_rect,
        pill_color.with_alpha(0.14),
        pill_color,
        theme.radii.badge,
        1.1,
        pill_glow,
    );
    let max_pill = ((pill_rect.w - 16.0) / theme.char_width(theme.font.small)).max(12.0) as usize;
    dl.text(
        pill_rect.inset(8.0, 0.0),
        trim_label(&pill_text, max_pill),
        theme.font.small,
        true,
        pill_color,
        TextAlign::Left,
    );
    dl.panel(
        count_rect,
        theme.surface.recessed,
        theme.border.subtle,
        theme.radii.badge,
        1.0,
        0.0,
    );
    dl.text(
        count_rect,
        count_text,
        theme.font.small,
        true,
        theme.text.secondary,
        TextAlign::Center,
    );

    // Row 2 (optional): Category filter pills (`All / Synth / Sampler / Drums / Kit / Modular`).
    if has_cats {
        let cat_row = Rect::new(top.x, top.y + 29.0, top.w, 22.0);
        dl.push_clip(cat_row);
        let cw = theme.char_width(theme.font.small);
        let mut cx = cat_row.x;
        for &(label, active) in &view.categories {
            let pw = label.chars().count() as f32 * cw + 18.0;
            if cx + pw > cat_row.right() {
                break;
            }
            let pr = Rect::new(cx, cat_row.y + 1.0, pw, cat_row.h - 2.0);
            dl.panel(
                pr,
                if active {
                    theme.accent.focus.with_alpha(0.22)
                } else {
                    theme.surface.recessed
                },
                if active {
                    theme.accent.focus
                } else {
                    theme.border.subtle
                },
                theme.radii.badge,
                if active { 1.2 } else { 1.0 },
                if active { 0.16 } else { 0.0 },
            );
            dl.text(
                pr,
                label,
                theme.font.small,
                active,
                if active {
                    theme.accent.focus
                } else {
                    theme.text.secondary
                },
                TextAlign::Center,
            );
            cx += pw + 6.0;
        }
        let hint_w = (cat_row.right() - cx - 8.0).max(0.0);
        if hint_w >= 120.0 {
            let hfont = theme.font.small - 1.0;
            let max_h = (hint_w / theme.char_width(hfont)).floor().max(6.0) as usize;
            dl.text(
                Rect::new(cx + 6.0, cat_row.y, hint_w - 6.0, cat_row.h),
                trim_label("h/l or Tab: category", max_h),
                hfont,
                false,
                theme.text.muted,
                TextAlign::Right,
            );
        }
        dl.pop_clip();
    }

    // Row 3 (or 2): Universal `/` search bar.
    let search_y = if has_cats { top.y + 57.0 } else { top.y + 30.0 };
    let search_rect = Rect::new(top.x, search_y, top.w, 26.0);
    let search_border = if view.searching {
        theme.accent.edit
    } else if !view.query.is_empty() {
        theme.accent.focus
    } else {
        theme.border.subtle
    };
    dl.panel(
        search_rect,
        theme.surface.recessed,
        search_border,
        theme.radii.badge,
        if view.searching { 1.5 } else { 1.0 },
        if view.searching {
            theme.glow.edit_border * 0.45
        } else if !view.query.is_empty() {
            0.14
        } else {
            0.0
        },
    );
    let search_text = if view.searching {
        format!("/{}▏", view.query)
    } else if !view.query.is_empty() {
        format!("/{}", view.query)
    } else {
        "/ filter presets by name, category, or sound…".into()
    };
    dl.text(
        search_rect.inset(8.0, 0.0),
        search_text,
        theme.font.small,
        view.searching || !view.query.is_empty(),
        if view.searching {
            theme.accent.edit
        } else if !view.query.is_empty() {
            theme.accent.focus
        } else {
            theme.text.muted
        },
        TextAlign::Left,
    );

    // Main split: left preset list (60%) + right live patch inspector (40%).
    let body_area = body.inset(0.0, 6.0);
    let inspector_w = (body_area.w * 0.39).clamp(240.0, 360.0);
    let (list_pane, inspector_pane) = body_area.split_right(inspector_w);
    let list_pane = Rect::new(
        list_pane.x,
        list_pane.y,
        (list_pane.w - 10.0).max(0.0),
        list_pane.h,
    );

    dl.panel(
        list_pane,
        theme.surface.recessed.with_alpha(0.55),
        theme.border.subtle,
        theme.radii.pill,
        1.0,
        0.0,
    );
    let list_inner = list_pane.inset(6.0, 6.0);
    dl.push_clip(list_inner);
    if view.items.is_empty() {
        let msg = if view.query.is_empty() {
            if view.active_category == "All" || view.active_category.is_empty() {
                format!(
                    "No {} presets found in configured preset dirs",
                    view.engine_label
                )
            } else {
                format!(
                    "No {} presets in '{}' category · h/l switches category",
                    view.engine_label, view.active_category
                )
            }
        } else {
            format!(
                "No {} presets match '/{}' in '{}'",
                view.engine_label, view.query, view.active_category
            )
        };
        dl.text(
            Rect::new(
                list_inner.x + 8.0,
                list_inner.y + 12.0,
                list_inner.w - 16.0,
                22.0,
            ),
            msg,
            theme.font.small,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
    } else {
        let row_h = 26.0;
        let visible = ((list_inner.h / row_h).floor() as usize).max(1);
        let first = scroll_window_centered(view.cursor, view.items.len(), visible);
        let cw = theme.char_width(theme.font.small);
        for (idx, item) in view.items.iter().enumerate().skip(first).take(visible) {
            let y = list_inner.y + (idx - first) as f32 * row_h;
            let row_rect = Rect::new(list_inner.x, y, list_inner.w, row_h - 2.0);
            if item.selected {
                dl.panel(
                    row_rect,
                    theme.surface.selection,
                    theme.accent.focus,
                    theme.radii.badge,
                    1.2,
                    0.16,
                );
            }
            let cat_w = 78.0;
            let cat_rect = Rect::new(row_rect.x + 4.0, row_rect.y + 3.0, cat_w, row_rect.h - 6.0);
            dl.rounded(
                cat_rect,
                theme.accent.routing.with_alpha(0.16),
                theme.radii.badge,
            );
            let max_cat = ((cat_w - 6.0) / cw).max(4.0) as usize;
            dl.text(
                cat_rect,
                trim_label(&item.category, max_cat),
                theme.font.small - 0.5,
                true,
                theme.accent.routing,
                TextAlign::Center,
            );

            let name_x = cat_rect.right() + 8.0;
            let name_w = (row_rect.w * 0.42).clamp(110.0, 190.0);
            let max_name = ((name_w - 4.0) / cw).max(6.0) as usize;
            let name_label = if item.is_original {
                format!("{} ●", item.name)
            } else {
                item.name.clone()
            };
            dl.text(
                Rect::new(name_x, row_rect.y, name_w, row_rect.h),
                trim_label(&name_label, max_name),
                theme.font.small,
                item.selected,
                if item.selected {
                    theme.accent.focus
                } else {
                    theme.text.primary
                },
                TextAlign::Left,
            );

            let sum_x = name_x + name_w + 6.0;
            let sum_w = (row_rect.right() - sum_x - 6.0).max(0.0);
            if sum_w > 36.0 {
                let max_sum = (sum_w / cw).max(4.0) as usize;
                dl.text(
                    Rect::new(sum_x, row_rect.y, sum_w, row_rect.h),
                    trim_label(&item.summary, max_sum),
                    theme.font.small - 0.5,
                    false,
                    theme.text.muted,
                    TextAlign::Right,
                );
            }
        }
    }
    dl.pop_clip();

    // Right inspector panel: selected preset details + error if any.
    dl.panel(
        inspector_pane,
        theme.surface.recessed.with_alpha(0.70),
        theme.border.subtle,
        theme.radii.pill,
        1.0,
        0.0,
    );
    let insp_inner = inspector_pane.inset(10.0, 8.0);
    dl.push_clip(insp_inner);
    if let Some(sel) = view.items.get(view.cursor) {
        dl.text(
            Rect::new(insp_inner.x, insp_inner.y, insp_inner.w, 20.0),
            sel.name.as_str(),
            theme.font.body,
            true,
            theme.accent.focus,
            TextAlign::Left,
        );
        dl.text(
            Rect::new(insp_inner.x, insp_inner.y + 22.0, insp_inner.w, 16.0),
            format!("{}  ·  {}", sel.category, view.engine_label),
            theme.font.small,
            false,
            theme.accent.routing,
            TextAlign::Left,
        );
        dl.fill(
            Rect::new(insp_inner.x, insp_inner.y + 42.0, insp_inner.w, 1.0),
            theme.border.hairline,
        );
        let cw = theme.char_width(theme.font.small);
        let max_chars = ((insp_inner.w - 4.0) / cw).max(10.0) as usize;
        let mut y = insp_inner.y + 50.0;
        for line in &view.selected_details {
            if y + 20.0 > insp_inner.bottom() - 36.0 {
                break;
            }
            dl.text(
                Rect::new(insp_inner.x, y, insp_inner.w, 20.0),
                trim_label(line, max_chars),
                theme.font.small,
                false,
                theme.text.secondary,
                TextAlign::Left,
            );
            y += 22.0;
        }
    } else {
        dl.text(
            Rect::new(insp_inner.x, insp_inner.y, insp_inner.w, 20.0),
            "No preset selected",
            theme.font.small,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
    }
    if let Some(err) = &view.preview_error {
        dl.text(
            Rect::new(insp_inner.x, insp_inner.bottom() - 34.0, insp_inner.w, 32.0),
            format!("! {err}"),
            theme.font.small,
            true,
            theme.accent.alert,
            TextAlign::Left,
        );
    }
    dl.pop_clip();

    // Footer legend.
    let footer_text = if view.searching {
        "type filter  ·  ↑/↓ preview  ·  Enter apply filter  ·  Ctrl+u clear  ·  Esc cancel search"
    } else if !view.query.is_empty() {
        "j/k preview  ·  h/l category  ·  s save  ·  R reload  ·  A audition  ·  Enter load  ·  Esc clear filter"
    } else {
        "j/k preview  ·  h/l category  ·  s save  ·  R reload  ·  A audition  ·  Space play  ·  / filter  ·  Enter load  ·  Esc restore"
    };
    dl.text(
        footer,
        footer_text,
        theme.font.small,
        true,
        theme.accent.warning_key,
        TextAlign::Left,
    );
    dl.pop_clip();
}
