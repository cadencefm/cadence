//! Filter Groove Browser (`P` on Filter Effect deck / `Enter` on `env_pattern`) GPU modal overlay
//! with live non-destructive preview indicator, `/` search bar, category badges, and live
//! step-velocity bar + coupled-ODE envelope contour preview.
use super::{overlay_frame, scroll_window_centered};
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::Theme;
use cadence_app::{GrooveBrowserPreview, GrooveBrowserView};

pub(super) fn draw_groove_browser(
    dl: &mut DisplayList,
    rect: Rect,
    view: &GrooveBrowserView,
    theme: &Theme,
) {
    let accent = if view.searching {
        theme.accent.edit
    } else {
        theme.accent.focus
    };
    let title = format!(
        "FILTER GROOVES  ·  {}  [{}]",
        view.device_name, view.channel_name
    );
    let inner = overlay_frame(dl, rect, &title, accent, theme);
    dl.push_clip(inner);

    let (top, rest) = inner.split_top(58.0);
    let (body, footer) = rest.split_bottom(24.0);

    // Row 1: Live preview status pill + groove match count.
    let status_row = Rect::new(top.x, top.y, top.w, 24.0);
    let (pill_text, pill_color, pill_glow) = if let Some(name) = &view.live_preview_name {
        (
            format!("● LIVE PREVIEW: {name}  (non-destructive · Esc restores original)"),
            theme.accent.edit,
            theme.glow.edit_border * 0.40,
        )
    } else {
        (
            "● ORIGINAL GROOVE  (j/k previews live · zero undo until Enter)".into(),
            theme.accent.playhead,
            0.16,
        )
    };
    let count_text = format!("{}/{} Grooves", view.items.len(), view.total_count);
    let count_w = (count_text.chars().count() as f32 * theme.char_width(theme.font.small) + 16.0)
        .clamp(96.0, 180.0);
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

    // Row 2: Universal `/` search bar.
    let search_rect = Rect::new(top.x, top.y + 30.0, top.w, 26.0);
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
        "/ filter grooves by name, category, 1/32, smooth, or punch…".into()
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

    // Main split: left groove list (~58%) + right step-bar/envelope inspector (~42%).
    let body_area = body.inset(0.0, 6.0);
    let inspector_w = (body_area.w * 0.42).clamp(280.0, 390.0);
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
            "No filter grooves loaded · save custom grooves with S on a Filter deck".into()
        } else {
            format!("No filter grooves match '/{}'", view.query)
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
            let cat_w = 92.0;
            let cat_rect = Rect::new(row_rect.x + 4.0, row_rect.y + 3.0, cat_w, row_rect.h - 6.0);
            let cat_col = if item.smooth_attack {
                theme.accent.focus
            } else {
                theme.accent.routing
            };
            dl.rounded(cat_rect, cat_col.with_alpha(0.16), theme.radii.badge);
            let max_cat = ((cat_w - 6.0) / cw).max(4.0) as usize;
            dl.text(
                cat_rect,
                trim_label(&item.category, max_cat),
                theme.font.small - 0.5,
                true,
                cat_col,
                TextAlign::Center,
            );

            let name_x = cat_rect.right() + 8.0;
            let name_w = (row_rect.w * 0.44).clamp(115.0, 210.0);
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
            if sum_w > 40.0 {
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

    // Right inspector panel: step-velocity bars + ODE envelope contour + metadata.
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
    if let Some(prev) = &view.selected_preview {
        draw_groove_inspector(dl, insp_inner, prev, &view.bank_source, theme);
    } else {
        dl.text(
            Rect::new(insp_inner.x, insp_inner.y, insp_inner.w, 20.0),
            "No groove selected",
            theme.font.small,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
    }
    if let Some(err) = &view.preview_error {
        dl.text(
            Rect::new(insp_inner.x, insp_inner.bottom() - 30.0, insp_inner.w, 28.0),
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
        "j/k preview  ·  f fork  ·  s save  ·  o bank  ·  Space play  ·  / filter  ·  Enter load  ·  Esc clear"
    } else {
        "j/k preview  ·  f fork  ·  s save  ·  o bank  ·  Space play  ·  / filter  ·  Enter load (1 undo)  ·  Esc restore"
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

fn draw_groove_inspector(
    dl: &mut DisplayList,
    area: Rect,
    prev: &GrooveBrowserPreview,
    bank_source: &str,
    theme: &Theme,
) {
    let cw = theme.char_width(theme.font.small);
    let max_title = ((area.w - 4.0) / theme.char_width(theme.font.body)).max(8.0) as usize;
    dl.text(
        Rect::new(area.x, area.y, area.w, 20.0),
        trim_label(&prev.name, max_title),
        theme.font.body,
        true,
        theme.accent.focus,
        TextAlign::Left,
    );
    let contour = if prev.smooth_attack {
        "Smooth"
    } else {
        "Punch"
    };
    let sub = format!(
        "Slot {:02} · {} · 1/{} · {}st · {contour}",
        prev.id, prev.category, prev.resolution, prev.length
    );
    let max_sub = ((area.w - 4.0) / cw).max(10.0) as usize;
    dl.text(
        Rect::new(area.x, area.y + 22.0, area.w, 16.0),
        trim_label(&sub, max_sub),
        theme.font.small,
        false,
        theme.accent.routing,
        TextAlign::Left,
    );
    dl.fill(
        Rect::new(area.x, area.y + 42.0, area.w, 1.0),
        theme.border.hairline,
    );

    // Step-velocity bar & coupled-ODE envelope contour card.
    let card_h = (area.h - 170.0).clamp(96.0, 168.0);
    let card_rect = Rect::new(area.x, area.y + 50.0, area.w, card_h);
    dl.panel(
        card_rect,
        theme.surface.canvas.with_alpha(0.85),
        theme.border.subtle,
        theme.radii.badge,
        1.0,
        0.0,
    );
    let plot = card_rect.inset(8.0, 8.0);
    let env_col = if prev.env_amount < 0.0 {
        theme.accent.alert
    } else {
        theme.accent.modulation
    };
    crate::decks::effect::filter::draw_filter_env_plot(
        dl,
        plot,
        &crate::decks::effect::filter::FilterEnvPlotSpec {
            steps: &prev.steps,
            env_curve: &prev.env_curve,
            resolution: prev.resolution,
            smooth_attack: prev.smooth_attack,
            active_step: None,
            env_col,
            bypassed: false,
        },
        theme,
    );

    // Metadata lines below the contour plot.
    let details = [
        format!(
            "Cycle: {}  ·  Hits: {}/{}  ·  Peak: {}/127",
            prev.cycle_label, prev.active_hits, prev.length, prev.peak_velocity
        ),
        format!(
            "Attack: {}",
            if prev.smooth_attack {
                "Smooth wah slew (18 ms)"
            } else {
                "Snappy percussive trigger (1.5 ms)"
            }
        ),
        format!(
            "Filter Env: {:+.1} oct  ·  Decay: {:.0} ms",
            prev.env_amount, prev.env_decay_ms
        ),
        format!("Bank: {bank_source}"),
    ];
    let mut y = card_rect.bottom() + 10.0;
    for line in &details {
        if y + 18.0 > area.bottom() - 28.0 {
            break;
        }
        dl.text(
            Rect::new(area.x, y, area.w, 18.0),
            trim_label(line, max_sub),
            theme.font.small,
            false,
            theme.text.secondary,
            TextAlign::Left,
        );
        y += 21.0;
    }
}
