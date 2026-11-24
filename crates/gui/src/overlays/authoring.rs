//! Create / Workflow / Pattern Authoring draft, Value Input, and Confirm modal overlays.
use super::{overlay_frame, scroll_window_centered};
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::Theme;
use cadence_app::authoring::{DraftField, DraftInput, DraftView};
use cadence_app::ValueInputView;

pub(super) fn draw_authoring(dl: &mut DisplayList, rect: Rect, draft: &DraftView, theme: &Theme) {
    let inner = overlay_frame(
        dl,
        rect,
        &format!("{}  ·  DRAFT", draft.title),
        theme.accent.focus,
        theme,
    );
    dl.push_clip(inner);
    let (top, footer) = inner.split_bottom(78.0);
    let (left, right) = if draft.fields.is_empty() {
        (Rect::default(), top)
    } else {
        let (l, r) = top.split_left((top.w * 0.34).clamp(180.0, 260.0));
        (l, Rect::new(r.x + 12.0, r.y, (r.w - 12.0).max(0.0), r.h))
    };

    // Left field list (vertically scrolled around the active field so deep nested PlacedPart drafts never clip).
    if !left.is_empty() {
        dl.rounded(left, theme.surface.recessed, theme.radii.pill);
        dl.push_clip(left);
        let row_h = 24.0;
        let total = draft.fields.len();
        let active_idx = draft.fields.iter().position(|f| f.active).unwrap_or(0);
        let full_visible = ((left.h - 8.0) / row_h).floor().max(1.0) as usize;
        let needs_scroll = total > full_visible;
        let (list_rect, scroll_foot) = if needs_scroll && left.h > 48.0 {
            let (body, foot) = left.split_bottom(18.0);
            (body, Some(foot))
        } else {
            (left, None)
        };
        let visible = ((list_rect.h - 8.0) / row_h).floor().max(1.0) as usize;
        let first = scroll_window_centered(active_idx, total, visible);
        for (vis_i, field) in draft.fields.iter().skip(first).take(visible).enumerate() {
            let y = list_rect.y + 4.0 + vis_i as f32 * row_h;
            let r = Rect::new(list_rect.x + 4.0, y, list_rect.w - 8.0, row_h - 2.0);
            if field.active {
                dl.rounded(r, theme.surface.selection, theme.radii.badge);
            }
            let val = if field.value.is_empty() {
                "—"
            } else {
                field.value.as_str()
            };
            let max_chars = ((r.w - 10.0) / theme.char_width(theme.font.small)) as usize;
            dl.text(
                r.inset(6.0, 0.0),
                trim_label(
                    &format!(
                        "{} {}: {val}",
                        if field.active { "›" } else { " " },
                        field.label
                    ),
                    max_chars,
                ),
                theme.font.small,
                field.active,
                if field.active {
                    theme.accent.focus
                } else {
                    theme.text.secondary
                },
                TextAlign::Left,
            );
        }
        if let Some(foot) = scroll_foot {
            let above = first;
            let below = total.saturating_sub(first + visible);
            let indicator = match (above > 0, below > 0) {
                (true, true) => format!("▲ {above} · {}/{total} · ▼ {below}", active_idx + 1),
                (true, false) => format!("▲ {above} · {}/{total}", active_idx + 1),
                (false, true) => format!("{}/{total} · ▼ {below}", active_idx + 1),
                (false, false) => format!("{}/{total}", active_idx + 1),
            };
            dl.text(
                foot.inset(6.0, 0.0),
                indicator,
                theme.font.small - 1.5,
                false,
                theme.text.muted,
                TextAlign::Center,
            );
        }
        dl.pop_clip();
    }

    // Right active field input / candidates.
    let (label_row, body) = right.split_top(24.0);
    dl.text(
        label_row,
        draft.field_label.as_str(),
        theme.font.body,
        true,
        theme.accent.warning_key,
        TextAlign::Left,
    );

    match draft.input {
        DraftInput::Text | DraftInput::Number => {
            let (input_box, hint_area) = body.split_top(34.0);
            let bg = if draft.replace_on_type {
                theme.accent.warning_key.with_alpha(0.22)
            } else {
                theme.surface.recessed
            };
            dl.panel(
                input_box,
                bg,
                theme.accent.focus,
                theme.radii.badge,
                1.0,
                0.10,
            );
            dl.text(
                input_box.inset(10.0, 0.0),
                format!("> {}▏", draft.text),
                theme.font.body,
                true,
                theme.text.primary,
                TextAlign::Left,
            );
            let help = if draft.field == DraftField::SampleFolder {
                "Type folder path · F (when empty) or Ctrl+f opens folder picker · Enter accepts"
            } else if draft.replace_on_type {
                "Type to replace the selected default · Enter keeps it"
            } else {
                "Type value · Backspace edits · Ctrl+u clears"
            };
            dl.text(
                Rect::new(hint_area.x, hint_area.y + 8.0, hint_area.w, 22.0),
                help,
                theme.font.small,
                false,
                theme.text.muted,
                TextAlign::Left,
            );
        }
        DraftInput::Choice | DraftInput::Reference => {
            let (hdr_row, list_area) = body.split_top(22.0);
            let row_h = 24.0;
            let visible = (list_area.h / row_h).floor().max(1.0) as usize;
            let sel = draft.selected.unwrap_or(0);
            let hdr = if draft.field == DraftField::Sample {
                "Enter/b opens sample browser · s leaves Sampler blank".into()
            } else if draft.input == DraftInput::Reference {
                format!(
                    "/{}{} · {} compatible ({}/{})",
                    draft.query,
                    if draft.searching { "▏" } else { "" },
                    draft.candidates.len(),
                    (sel + 1).min(draft.candidates.len()),
                    draft.candidates.len()
                )
            } else if draft.candidates.len() > visible {
                format!(
                    "Press shortcut key or j/k + Enter  ({}/{})",
                    (sel + 1).min(draft.candidates.len()),
                    draft.candidates.len()
                )
            } else {
                "Press shortcut key or j/k + Enter".into()
            };
            dl.text(
                hdr_row,
                hdr,
                theme.font.small,
                false,
                theme.text.muted,
                TextAlign::Left,
            );

            let first = scroll_window_centered(sel, draft.candidates.len(), visible);
            for (idx, cand) in draft
                .candidates
                .iter()
                .enumerate()
                .skip(first)
                .take(visible)
            {
                let y = list_area.y + (idx - first) as f32 * row_h;
                let row_rect = Rect::new(list_area.x, y, list_area.w, row_h - 2.0);
                let active = draft.selected == Some(idx);
                if active {
                    dl.rounded(row_rect, theme.surface.selection, theme.radii.badge);
                }
                let key = cand.key.map_or_else(|| " ".into(), |k| k.to_string());
                dl.text(
                    Rect::new(row_rect.x + 6.0, row_rect.y, 26.0, row_rect.h),
                    key,
                    theme.font.small,
                    true,
                    theme.accent.warning_key,
                    TextAlign::Left,
                );
                let small_cw = theme.char_width(theme.font.small);
                let name_w = (row_rect.w * 0.34).clamp(104.0, 168.0);
                let max_name = ((name_w - 4.0) / small_cw).max(6.0) as usize;
                dl.text(
                    Rect::new(row_rect.x + 32.0, row_rect.y, name_w, row_rect.h),
                    trim_label(&cand.label, max_name),
                    theme.font.small,
                    active,
                    if active {
                        theme.accent.focus
                    } else {
                        theme.text.primary
                    },
                    TextAlign::Left,
                );
                let detail_w = (row_rect.w - 40.0 - name_w).max(0.0);
                let max_detail = ((detail_w - 4.0) / small_cw).max(6.0) as usize;
                dl.text(
                    Rect::new(row_rect.x + 36.0 + name_w, row_rect.y, detail_w, row_rect.h),
                    trim_label(&cand.detail, max_detail),
                    theme.font.small,
                    false,
                    theme.text.muted,
                    TextAlign::Left,
                );
            }
        }
    }

    // Footer summary + error + action legend.
    dl.fill(
        Rect::new(footer.x, footer.y, footer.w, 1.0),
        theme.border.subtle,
    );
    dl.text(
        Rect::new(footer.x, footer.y + 6.0, footer.w, 20.0),
        draft.summary.as_str(),
        theme.font.small,
        false,
        theme.accent.routing,
        TextAlign::Left,
    );
    if let Some(err) = &draft.error {
        dl.text(
            Rect::new(footer.x, footer.y + 28.0, footer.w, 22.0),
            format!("! {err}"),
            theme.font.small,
            true,
            theme.accent.alert,
            TextAlign::Left,
        );
    } else if let Some(prev) = draft.preview.first() {
        dl.text(
            Rect::new(footer.x, footer.y + 28.0, footer.w, 22.0),
            prev.as_str(),
            theme.font.small,
            false,
            theme.text.secondary,
            TextAlign::Left,
        );
    }
    dl.text(
        Rect::new(footer.x, footer.bottom() - 20.0, footer.w, 20.0),
        "Enter accept  ·  Shift+Tab back  ·  Esc cancel",
        theme.font.small,
        true,
        theme.accent.warning_key,
        TextAlign::Left,
    );
    dl.pop_clip();
}

pub(super) fn draw_value_input(
    dl: &mut DisplayList,
    rect: Rect,
    input: &ValueInputView,
    theme: &Theme,
) {
    let frame_accent = if input.error.is_some() {
        theme.accent.alert
    } else {
        theme.accent.edit
    };
    let inner = overlay_frame(
        dl,
        rect,
        &format!("EXACT VALUE  ·  {}", input.title),
        frame_accent,
        theme,
    );
    dl.push_clip(inner);
    let cw = theme.char_width(theme.font.small);
    let max_chars = ((inner.w - 4.0) / cw).max(16.0) as usize;
    dl.text(
        Rect::new(inner.x, inner.y, inner.w, 20.0),
        trim_label(
            &format!("{}  ·  Current: {}", input.subtitle, input.current_display),
            max_chars,
        ),
        theme.font.body,
        true,
        theme.text.primary,
        TextAlign::Left,
    );
    dl.text(
        Rect::new(inner.x, inner.y + 24.0, inner.w, 18.0),
        trim_label(&format!("Accepted: {}", input.units_hint), max_chars),
        theme.font.small,
        false,
        theme.text.muted,
        TextAlign::Left,
    );

    let box_rect = Rect::new(inner.x, inner.y + 52.0, inner.w, 36.0);
    let box_border = if input.error.is_some() {
        theme.accent.alert
    } else if input.live_preview.is_some() {
        theme.accent.playhead
    } else {
        theme.accent.edit
    };
    dl.panel(
        box_rect,
        theme.surface.recessed,
        box_border,
        theme.radii.pill,
        1.5,
        if input.error.is_some() {
            0.25
        } else {
            theme.glow.edit_border * 0.40
        },
    );
    dl.text(
        box_rect.inset(10.0, 0.0),
        format!("> {}▏", input.input),
        theme.font.body,
        true,
        theme.text.primary,
        TextAlign::Left,
    );

    let status_y = box_rect.bottom() + 12.0;
    if let Some(err) = &input.error {
        dl.text(
            Rect::new(inner.x, status_y, inner.w, 22.0),
            trim_label(&format!("! {err}"), max_chars),
            theme.font.small,
            true,
            theme.accent.alert,
            TextAlign::Left,
        );
    } else if let Some(preview) = &input.live_preview {
        dl.text(
            Rect::new(inner.x, status_y, inner.w, 22.0),
            trim_label(&format!("✓ Will apply:  {preview}"), max_chars),
            theme.font.small,
            true,
            theme.accent.playhead,
            TextAlign::Left,
        );
    } else {
        dl.text(
            Rect::new(inner.x, status_y, inner.w, 22.0),
            "Type an exact value or press Enter to keep current",
            theme.font.small,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
    }

    dl.text(
        Rect::new(inner.x, inner.bottom() - 20.0, inner.w, 20.0),
        "Enter commit  ·  Ctrl+u clear  ·  Esc cancel",
        theme.font.small,
        true,
        theme.accent.warning_key,
        TextAlign::Left,
    );
    dl.pop_clip();
}

pub(super) fn draw_confirm(
    dl: &mut DisplayList,
    rect: Rect,
    title: &str,
    message: &str,
    theme: &Theme,
) {
    let inner = overlay_frame(dl, rect, title, theme.accent.alert, theme);
    dl.push_clip(inner);
    let max_body_chars = ((inner.w - 4.0) / theme.char_width(theme.font.body))
        .floor()
        .max(16.0) as usize;
    let max_small_chars = ((inner.w - 4.0) / theme.char_width(theme.font.small))
        .floor()
        .max(20.0) as usize;
    let lines: Vec<&str> = message.lines().collect();
    if let Some(first) = lines.first() {
        dl.text(
            Rect::new(inner.x, inner.y + 6.0, inner.w, 24.0),
            trim_label(first, max_body_chars),
            theme.font.body,
            lines.len() > 1,
            theme.text.primary,
            TextAlign::Left,
        );
    }
    for (idx, line) in lines.iter().skip(1).take(2).enumerate() {
        dl.text(
            Rect::new(inner.x, inner.y + 34.0 + idx as f32 * 20.0, inner.w, 18.0),
            trim_label(line, max_small_chars),
            theme.font.small,
            false,
            theme.text.secondary,
            TextAlign::Left,
        );
    }
    dl.text(
        Rect::new(inner.x, inner.bottom() - 20.0, inner.w, 20.0),
        "y / Enter confirm  ·  Esc cancel",
        theme.font.small,
        true,
        theme.accent.warning_key,
        TextAlign::Left,
    );
    dl.pop_clip();
}
