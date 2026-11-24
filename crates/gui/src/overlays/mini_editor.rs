//! Dedicated Mini-Notation Editor (`Mode::MiniEditor`) modal overlay with live
//! keystroke parse validation (`✓ Valid · N steps · M events` vs red syntax error).
use super::overlay_frame;
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::Theme;
use cadence_app::MiniEditorView;

pub(super) fn draw_mini_editor(
    dl: &mut DisplayList,
    rect: Rect,
    view: &MiniEditorView,
    theme: &Theme,
) {
    let frame_accent = if view.valid {
        theme.accent.edit
    } else {
        theme.accent.alert
    };
    let title = format!(
        "MINI-NOTATION EDITOR  ·  {}  ({} steps)",
        view.pattern_name, view.steps
    );
    let inner = overlay_frame(dl, rect, &title, frame_accent, theme);
    dl.push_clip(inner);

    // Quick syntax cheat ribbon.
    let syntax_rect = Rect::new(inner.x, inner.y, inner.w, 24.0);
    dl.panel(
        syntax_rect,
        theme.surface.recessed,
        theme.border.subtle,
        theme.radii.badge,
        1.0,
        0.0,
    );
    dl.text(
        syntax_rect.inset(8.0, 0.0),
        "Syntax:  c4 / 1–8 pitch  ·  ~ rest  ·  [a b] subdiv  ·  [c,e,g] chord  ·  <a b> alt  ·  *N /N (k,n) ? !",
        theme.font.small - 0.5,
        false,
        theme.text.muted,
        TextAlign::Left,
    );

    // Editable code input field.
    let code_label_rect = Rect::new(inner.x, inner.y + 34.0, inner.w, 18.0);
    dl.text(
        code_label_rect,
        "MINI-NOTATION EXPRESSION",
        theme.font.small,
        true,
        theme.accent.routing,
        TextAlign::Left,
    );

    let box_rect = Rect::new(inner.x, inner.y + 56.0, inner.w, 46.0);
    let box_border = if view.valid {
        theme.accent.edit
    } else {
        theme.accent.alert
    };
    dl.panel(
        box_rect,
        theme.surface.recessed,
        box_border,
        theme.radii.pill,
        1.8,
        if view.valid {
            theme.glow.edit_border * 0.45
        } else {
            0.35
        },
    );
    let cw = theme.char_width(theme.font.body);
    let max_code = ((box_rect.w - 28.0) / cw).max(16.0) as usize;
    let display_code = if view.code.chars().count() > max_code {
        let skip = view.code.chars().count().saturating_sub(max_code - 1);
        format!("…{}▏", view.code.chars().skip(skip).collect::<String>())
    } else {
        format!("{}▏", view.code)
    };
    dl.text(
        box_rect.inset(12.0, 0.0),
        display_code,
        theme.font.body,
        true,
        theme.text.primary,
        TextAlign::Left,
    );

    // Live parse validation & event count preview card.
    let status_rect = Rect::new(inner.x, box_rect.bottom() + 14.0, inner.w, 72.0);
    if view.valid {
        dl.panel(
            status_rect,
            theme.accent.playhead.with_alpha(0.12),
            theme.accent.playhead,
            theme.radii.pill,
            1.2,
            0.18,
        );
        let top_line = format!(
            "✓ Valid  ·  {} steps  ·  {} event{} in cycle 1",
            view.steps,
            view.event_count,
            if view.event_count == 1 { "" } else { "s" }
        );
        dl.text(
            Rect::new(
                status_rect.x + 12.0,
                status_rect.y + 10.0,
                status_rect.w - 24.0,
                22.0,
            ),
            top_line,
            theme.font.body,
            true,
            theme.accent.playhead,
            TextAlign::Left,
        );
        let max_prev =
            ((status_rect.w - 24.0) / theme.char_width(theme.font.small)).max(16.0) as usize;
        dl.text(
            Rect::new(
                status_rect.x + 12.0,
                status_rect.y + 38.0,
                status_rect.w - 24.0,
                22.0,
            ),
            trim_label(
                &format!("Cycle 1 preview:  {}", view.preview_summary),
                max_prev,
            ),
            theme.font.small,
            false,
            theme.text.secondary,
            TextAlign::Left,
        );
    } else {
        dl.panel(
            status_rect,
            theme.accent.alert.with_alpha(0.14),
            theme.accent.alert,
            theme.radii.pill,
            1.4,
            0.26,
        );
        dl.text(
            Rect::new(
                status_rect.x + 12.0,
                status_rect.y + 10.0,
                status_rect.w - 24.0,
                20.0,
            ),
            "! Syntax error — fix expression before pressing Enter",
            theme.font.body,
            true,
            theme.accent.alert,
            TextAlign::Left,
        );
        let err = view.error.as_deref().unwrap_or("Invalid mini-notation");
        let max_err =
            ((status_rect.w - 24.0) / theme.char_width(theme.font.small)).max(16.0) as usize;
        dl.text(
            Rect::new(
                status_rect.x + 12.0,
                status_rect.y + 38.0,
                status_rect.w - 24.0,
                22.0,
            ),
            trim_label(err, max_err),
            theme.font.small,
            false,
            theme.text.primary,
            TextAlign::Left,
        );
    }

    // Footer legend.
    dl.text(
        Rect::new(inner.x, inner.bottom() - 20.0, inner.w, 20.0),
        if view.valid {
            "Enter commit pattern  ·  Ctrl+u clear  ·  Esc cancel"
        } else {
            "Backspace / edit expression  ·  Ctrl+u clear  ·  Esc cancel"
        },
        theme.font.small,
        true,
        theme.accent.warning_key,
        TextAlign::Left,
    );
    dl.pop_clip();
}
