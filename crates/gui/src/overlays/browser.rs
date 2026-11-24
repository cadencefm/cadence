//! Sample Browser (`B`/`gS`, with GPU waveform preview) and Picker modal overlays.
use super::{overlay_frame, scroll_window_centered};
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::Theme;
use cadence_app::{BrowserView, PickerView};
use cadence_core::music::note_name;

pub(super) fn draw_picker(dl: &mut DisplayList, rect: Rect, picker: &PickerView, theme: &Theme) {
    let inner = overlay_frame(dl, rect, &picker.title, theme.accent.focus, theme);
    dl.push_clip(inner);
    let (filter_row, rest) = inner.split_top(26.0);
    let (list_area, foot_row) = rest.split_bottom(22.0);
    let filter_border = if picker.searching {
        theme.accent.edit
    } else if picker.filter.is_some() {
        theme.accent.focus
    } else {
        theme.border.subtle
    };
    dl.panel(
        filter_row,
        theme.surface.recessed,
        filter_border,
        theme.radii.badge,
        if picker.searching { 1.4 } else { 1.0 },
        if picker.searching {
            theme.glow.edit_border * 0.45
        } else if picker.filter.is_some() {
            0.14
        } else {
            0.0
        },
    );
    let filter_text = if picker.searching {
        format!("/{}▏", picker.filter.as_deref().unwrap_or(""))
    } else if let Some(f) = &picker.filter {
        format!("/{f}")
    } else {
        "/ filter…".into()
    };
    dl.text(
        filter_row.inset(8.0, 0.0),
        filter_text,
        theme.font.small,
        picker.searching || picker.filter.is_some(),
        if picker.searching {
            theme.accent.edit
        } else if picker.filter.is_some() {
            theme.accent.focus
        } else {
            theme.text.muted
        },
        TextAlign::Left,
    );

    let row_h = 24.0;
    let visible = ((list_area.h - 8.0) / row_h).floor().max(1.0) as usize;
    if picker.items.is_empty() {
        let empty_msg = match &picker.filter {
            Some(f) if !f.is_empty() => format!("No matches for '/{f}'"),
            _ => "No choices available".into(),
        };
        dl.text(
            Rect::new(
                list_area.x + 8.0,
                list_area.y + 12.0,
                list_area.w - 16.0,
                24.0,
            ),
            empty_msg,
            theme.font.small,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
    }
    let first = scroll_window_centered(picker.cursor, picker.items.len(), visible);
    for (idx, (name, detail, current)) in picker.items.iter().enumerate().skip(first).take(visible)
    {
        let y = list_area.y + 6.0 + (idx - first) as f32 * row_h;
        let r = Rect::new(list_area.x, y, list_area.w, row_h - 2.0);
        let sel = idx == picker.cursor;
        if sel {
            dl.rounded(r, theme.surface.selection, theme.radii.badge);
        }
        if idx < 10 {
            let hotkey = if idx == 9 {
                "0".to_string()
            } else {
                format!("{}", idx + 1)
            };
            dl.text(
                Rect::new(r.x + 6.0, r.y, 20.0, r.h),
                hotkey,
                theme.font.small,
                true,
                theme.accent.warning_key,
                TextAlign::Left,
            );
        }
        let cw = theme.char_width(theme.font.small);
        let name_w = (r.w * 0.56).max(80.0);
        let detail_x = r.x + 28.0 + name_w + 6.0;
        let detail_w = (r.right() - detail_x - 26.0).max(40.0);
        dl.text(
            Rect::new(r.x + 28.0, r.y, name_w, r.h),
            trim_label(name.as_str(), (name_w / cw).max(8.0) as usize),
            theme.font.small,
            sel,
            if sel {
                theme.accent.focus
            } else {
                theme.text.primary
            },
            TextAlign::Left,
        );
        dl.text(
            Rect::new(detail_x, r.y, detail_w, r.h),
            trim_label(detail.as_str(), (detail_w / cw).max(6.0) as usize),
            theme.font.small,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
        if *current {
            dl.text(
                Rect::new(r.right() - 24.0, r.y, 20.0, r.h),
                "●",
                theme.font.small,
                true,
                theme.accent.playhead,
                TextAlign::Right,
            );
        }
    }

    dl.text(
        foot_row,
        if picker.audition {
            "j/k move · / filter (Enter apply · Esc clear) · A audition · Enter choose · Esc back"
        } else {
            "j/k move · / filter (Enter apply · Esc clear) · Enter choose · Esc back"
        },
        theme.font.small,
        false,
        theme.text.muted,
        TextAlign::Left,
    );
    dl.pop_clip();
}

pub(super) fn draw_browser(dl: &mut DisplayList, rect: Rect, browser: &BrowserView, theme: &Theme) {
    let inner = overlay_frame(dl, rect, &browser.title, theme.accent.focus, theme);
    dl.push_clip(inner);
    let (top, rest) = inner.split_top(50.0);
    let (body, bottom) = rest.split_bottom(46.0);

    dl.text(
        Rect::new(top.x, top.y, top.w, 22.0),
        browser.destination.as_str(),
        theme.font.small,
        true,
        theme.accent.warning_key,
        TextAlign::Left,
    );
    let search_rect = Rect::new(top.x, top.y + 24.0, top.w, 24.0);
    dl.panel(
        search_rect,
        theme.surface.recessed,
        if browser.searching {
            theme.accent.focus
        } else {
            theme.border.subtle
        },
        theme.radii.badge,
        1.0,
        if browser.searching { 0.20 } else { 0.0 },
    );
    let query_display = if browser.searching {
        format!("/{}▏", browser.query)
    } else if !browser.query.is_empty() {
        format!("/{}", browser.query)
    } else {
        "/ filter…".into()
    };
    dl.text(
        search_rect.inset(8.0, 0.0),
        format!(
            "{query_display}  ·  {} / {} files",
            browser.filtered, browser.total
        ),
        theme.font.small,
        browser.searching,
        if browser.searching || !browser.query.is_empty() {
            theme.accent.focus
        } else {
            theme.text.secondary
        },
        TextAlign::Left,
    );

    let (list_rect, prev_rect) = body.split_left((body.w * 0.44).clamp(220.0, 380.0));
    let prev_rect = Rect::new(
        prev_rect.x + 12.0,
        prev_rect.y + 6.0,
        (prev_rect.w - 12.0).max(0.0),
        (prev_rect.h - 12.0).max(0.0),
    );

    // Sample list
    let list_inner = Rect::new(
        list_rect.x,
        list_rect.y + 6.0,
        list_rect.w,
        (list_rect.h - 12.0).max(0.0),
    );
    dl.rounded(list_inner, theme.surface.recessed, theme.radii.pill);
    dl.push_clip(list_inner);
    let row_h = 22.0;
    let visible = ((list_inner.h - 8.0) / row_h).floor().max(1.0) as usize;
    let selected = browser.entries.iter().position(|e| e.selected).unwrap_or(0);
    let first = scroll_window_centered(selected, browser.entries.len(), visible);
    for (i, entry) in browser.entries.iter().enumerate().skip(first).take(visible) {
        let y = list_inner.y + 4.0 + (i - first) as f32 * row_h;
        let r = Rect::new(list_inner.x + 4.0, y, list_inner.w - 8.0, row_h - 1.0);
        if entry.selected {
            dl.rounded(r, theme.surface.selection, theme.radii.badge);
        }
        let max_c = ((r.w - 12.0) / theme.char_width(theme.font.small)) as usize;
        dl.text(
            r.inset(6.0, 0.0),
            trim_label(
                &format!("{} {}", if entry.selected { "›" } else { " " }, entry.label),
                max_c,
            ),
            theme.font.small,
            entry.selected,
            if entry.selected {
                theme.accent.focus
            } else {
                theme.text.primary
            },
            TextAlign::Left,
        );
    }
    dl.pop_clip();

    // GPU waveform preview pane on right!
    dl.panel(
        prev_rect,
        theme.surface.recessed,
        theme.border.hairline,
        theme.radii.pill,
        1.0,
        0.0,
    );
    let pin = prev_rect.inset(10.0, 8.0);
    dl.push_clip(pin);
    if let Some(sample) = &browser.sample {
        let (meta_r, wave_r) = pin.split_top(44.0);
        dl.text(
            Rect::new(meta_r.x, meta_r.y, meta_r.w, 20.0),
            sample.name.as_str(),
            theme.font.body,
            true,
            theme.accent.focus,
            TextAlign::Left,
        );
        dl.text(
            Rect::new(meta_r.x, meta_r.y + 22.0, meta_r.w, 18.0),
            format!(
                "{:.2}s  ·  {} Hz  ·  {}ch  ·  root {}",
                sample.duration_seconds(),
                sample.sample_rate,
                sample.channels,
                note_name(sample.meta.root_midi)
            ),
            theme.font.small,
            false,
            theme.text.secondary,
            TextAlign::Left,
        );
        let wave_box = wave_r.inset(0.0, 8.0);
        dl.fill(
            Rect::new(wave_box.x, wave_box.y + wave_box.h * 0.5, wave_box.w, 1.0),
            theme.border.subtle,
        );
        dl.peak_envelope(
            wave_box,
            &sample.peaks,
            theme.accent.routing,
            theme.glow.scope * 0.6,
        );
    } else {
        dl.text(
            pin,
            if browser.loading {
                "Decoding sample preview…"
            } else {
                "Select a sample to preview its waveform"
            },
            theme.font.small,
            false,
            theme.text.muted,
            TextAlign::Center,
        );
    }
    dl.pop_clip();

    let status_text = browser
        .error
        .as_ref()
        .map_or_else(|| browser.status.clone(), |e| format!("! {e}"));
    dl.text(
        Rect::new(bottom.x, bottom.y + 2.0, bottom.w, 20.0),
        status_text,
        theme.font.small,
        browser.error.is_some(),
        if browser.error.is_some() {
            theme.accent.alert
        } else {
            theme.text.secondary
        },
        TextAlign::Left,
    );
    dl.text(
        Rect::new(bottom.x, bottom.y + 24.0, bottom.w, 20.0),
        if browser.searching {
            "type filter  ·  ↑/↓ select  ·  Enter apply filter  ·  Ctrl+u clear  ·  Esc clear search"
        } else if !browser.query.is_empty() {
            "j/k select  ·  / search  ·  A audition  ·  Enter choose  ·  Esc clear filter"
        } else {
            "j/k select  ·  / search  ·  A audition  ·  Enter choose  ·  r reload  ·  Esc back"
        },
        theme.font.small,
        true,
        theme.accent.warning_key,
        TextAlign::Left,
    );
    dl.pop_clip();
}
