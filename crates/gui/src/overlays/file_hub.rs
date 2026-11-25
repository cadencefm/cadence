//! 5-tab File Hub modal (`gf`) for Quick Open, Browse, Save & Bundle, Import, and Export workflows.
use super::{overlay_frame, scroll_window_centered};
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::Theme;

pub(super) fn draw_file_hub(
    dl: &mut DisplayList,
    rect: Rect,
    hub: &cadence_app::view::FileHubView,
    theme: &Theme,
) {
    let inner = overlay_frame(
        dl,
        rect,
        "PROJECT & FILE HUB  ·  SAVE · LOAD · BUNDLE · IMPORT · EXPORT",
        theme.accent.focus,
        theme,
    );
    dl.push_clip(inner);

    // 1. Tab strip + right-side native Finder badge (30px).
    let (tabs_row, rest) = inner.split_top(30.0);
    let cw = theme.char_width(theme.font.small);
    let mut tx = tabs_row.x;
    for (idx, (label, active)) in hub.tabs.iter().enumerate() {
        let text = format!("{}  {label}", idx + 1);
        let tw = text.chars().count() as f32 * cw + 20.0;
        let tr = Rect::new(tx, tabs_row.y + 2.0, tw, 24.0);
        dl.panel(
            tr,
            if *active {
                theme.accent.focus.with_alpha(0.20)
            } else {
                theme.surface.recessed
            },
            if *active {
                theme.accent.focus
            } else {
                theme.border.subtle
            },
            theme.radii.pill,
            if *active { 1.2 } else { 1.0 },
            if *active { 0.25 } else { 0.0 },
        );
        dl.text(
            tr,
            text,
            theme.font.small,
            *active,
            if *active {
                theme.accent.focus
            } else {
                theme.text.secondary
            },
            TextAlign::Center,
        );
        tx += tw + 8.0;
    }

    let finder_lbl = "[F] Native Finder";
    let fw = finder_lbl.chars().count() as f32 * cw + 18.0;
    let fr = Rect::new(tabs_row.right() - fw, tabs_row.y + 2.0, fw, 24.0);
    dl.panel(
        fr,
        theme.accent.warning_key.with_alpha(0.14),
        theme.accent.warning_key.with_alpha(0.70),
        theme.radii.pill,
        1.0,
        0.0,
    );
    dl.text(
        fr,
        finder_lbl,
        theme.font.small,
        true,
        theme.accent.warning_key,
        TextAlign::Center,
    );

    let rest = Rect::new(rest.x, rest.y + 6.0, rest.w, (rest.h - 6.0).max(0.0));

    // 2. Context bar: active inline path editor OR directory + search bar (28px).
    let (ctx_row, rest) = rest.split_top(28.0);
    if let Some(editing) = &hub.editing_field {
        dl.panel(
            ctx_row,
            theme.accent.focus.with_alpha(0.15),
            theme.accent.focus,
            theme.radii.badge,
            1.2,
            0.20,
        );
        dl.text(
            ctx_row.inset(10.0, 0.0),
            format!("EDIT PATH: {editing}▏   (Enter apply · Ctrl+u clear · Esc cancel)"),
            theme.font.small,
            true,
            theme.text.primary,
            TextAlign::Left,
        );
    } else if hub.active_tab.is_list_tab() {
        let search_w = 240.0_f32.min(ctx_row.w * 0.34);
        let dir_rect = Rect::new(
            ctx_row.x,
            ctx_row.y,
            (ctx_row.w - search_w - 8.0).max(80.0),
            ctx_row.h,
        );
        let search_rect = Rect::new(dir_rect.right() + 8.0, ctx_row.y, search_w, ctx_row.h);
        dl.rounded(dir_rect, theme.surface.recessed, theme.radii.badge);
        if hub.active_tab == cadence_app::FileHubTab::Quick {
            let max_info = ((dir_rect.w - 16.0) / cw) as usize;
            dl.text(
                dir_rect.inset(8.0, 0.0),
                trim_label(
                    "Quick Open  ·  New, Recent, Examples & Takes  ·  2 Browse folders",
                    max_info,
                ),
                theme.font.small,
                false,
                theme.text.secondary,
                TextAlign::Left,
            );
        } else {
            let jump_hint = "~ home · . proj";
            let jump_w =
                jump_hint.chars().count() as f32 * theme.char_width(theme.font.small - 1.0) + 12.0;
            let show_jump = dir_rect.w >= 460.0;
            let path_w = if show_jump {
                (dir_rect.w - jump_w - 16.0).max(80.0)
            } else {
                (dir_rect.w - 16.0).max(40.0)
            };
            let max_dir = (path_w / cw) as usize;
            dl.text(
                Rect::new(dir_rect.x + 8.0, dir_rect.y, path_w, dir_rect.h),
                trim_label(&format!("DIR: {}", hub.current_dir), max_dir),
                theme.font.small,
                false,
                theme.text.secondary,
                TextAlign::Left,
            );
            if show_jump {
                dl.text(
                    Rect::new(
                        dir_rect.right() - jump_w - 6.0,
                        dir_rect.y,
                        jump_w,
                        dir_rect.h,
                    ),
                    jump_hint,
                    theme.font.small - 1.0,
                    false,
                    theme.text.muted,
                    TextAlign::Right,
                );
            }
        }
        dl.panel(
            search_rect,
            theme.surface.recessed,
            if hub.searching {
                theme.accent.focus
            } else {
                theme.border.subtle
            },
            theme.radii.badge,
            1.0,
            if hub.searching { 0.20 } else { 0.0 },
        );
        let q_text = if hub.searching {
            format!("/{}▏", hub.query)
        } else if !hub.query.is_empty() {
            format!("/{}", hub.query)
        } else {
            match hub.active_tab {
                cadence_app::FileHubTab::Import => "/ filter · R reload".into(),
                cadence_app::FileHubTab::Quick => "/ search all…".into(),
                _ => "/ filter files…".into(),
            }
        };
        dl.text(
            search_rect.inset(8.0, 0.0),
            q_text,
            theme.font.small,
            hub.searching,
            if hub.searching || !hub.query.is_empty() {
                theme.accent.focus
            } else {
                theme.text.muted
            },
            TextAlign::Left,
        );
    } else {
        dl.rounded(ctx_row, theme.surface.recessed, theme.radii.badge);
        let tip = match hub.active_tab {
            cadence_app::FileHubTab::Save => {
                "Enter execute row  ·  e edit path inline  ·  F pick path/folder in Finder"
            }
            cadence_app::FileHubTab::Export => {
                "Alt+h/l or ,/. adjust mode/scope/rate  ·  e edit output  ·  F Finder  ·  Enter render"
            }
            _ => "",
        };
        dl.text(
            ctx_row.inset(10.0, 0.0),
            tip,
            theme.font.small,
            false,
            theme.text.secondary,
            TextAlign::Left,
        );
    }

    let rest = Rect::new(rest.x, rest.y + 8.0, rest.w, (rest.h - 8.0).max(0.0));
    let footer_h =
        if hub.active_tab == cadence_app::FileHubTab::Export && hub.export_summary.is_some() {
            86.0
        } else {
            54.0
        };
    let (list_area, bottom) = rest.split_bottom(footer_h);
    let list_rect = Rect::new(
        list_area.x,
        list_area.y,
        list_area.w,
        (list_area.h - 8.0).max(40.0),
    );

    // 3. Main rows panel.
    dl.panel(
        list_rect,
        theme.surface.recessed,
        theme.border.subtle,
        theme.radii.panel,
        1.0,
        0.0,
    );
    let list_inner = list_rect.inset(8.0, 6.0);

    enum DisplayItem<'a> {
        Section(&'static str),
        Row(&'a cadence_app::view::FileHubRowView),
    }
    let mut items: Vec<DisplayItem<'_>> = Vec::with_capacity(hub.rows.len() + 6);
    let mut last_sec: Option<&'static str> = None;
    let mut sel_item_idx = 0usize;
    for row in &hub.rows {
        if last_sec != Some(row.section) {
            items.push(DisplayItem::Section(row.section));
            last_sec = Some(row.section);
        }
        if row.selected {
            sel_item_idx = items.len();
        }
        items.push(DisplayItem::Row(row));
    }

    let is_form = matches!(
        hub.active_tab,
        cadence_app::FileHubTab::Save | cadence_app::FileHubTab::Export
    );
    let row_h = if is_form { 44.0 } else { 25.0 };
    let visible = (list_inner.h / row_h).floor().max(1.0) as usize;
    let first = scroll_window_centered(sel_item_idx, items.len(), visible);
    dl.push_clip(list_inner);

    if items.is_empty() {
        let empty_msg = if hub.query.is_empty() {
            if hub.active_tab == cadence_app::FileHubTab::Quick {
                "No recent projects or examples yet · press 2 to Browse folders or F for Finder"
                    .into()
            } else {
                "No matching files in this directory · Backspace up · ~ home · . project · F Finder"
                    .into()
            }
        } else {
            format!(
                "No files match '/{}' · press Esc to clear filter or F for Finder",
                hub.query
            )
        };
        dl.text(
            list_inner,
            empty_msg,
            theme.font.small,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
    }

    for (vis_idx, item) in items.iter().skip(first).take(visible).enumerate() {
        let y = list_inner.y + vis_idx as f32 * row_h;
        match item {
            DisplayItem::Section(sec) => {
                let hr = Rect::new(
                    list_inner.x,
                    y + (row_h - 18.0).max(2.0),
                    list_inner.w,
                    16.0,
                );
                dl.text(
                    hr,
                    *sec,
                    theme.font.small - 1.0,
                    true,
                    theme.accent.routing,
                    TextAlign::Left,
                );
            }
            DisplayItem::Row(row) => {
                let r = Rect::new(list_inner.x, y + 1.0, list_inner.w, row_h - 3.0);
                let badge_col = match row.badge {
                    "NEW" | "DEMO" | "RUN" => theme.accent.playhead,
                    "RBS" => theme.accent.override_bar,
                    "BUNDLE" | "SAVE" => theme.accent.focus,
                    "TAKE" => theme.accent.audition,
                    "PRESET" | "MIDI" => theme.accent.modulation,
                    "DIR" => theme.accent.warning_key,
                    _ => theme.accent.routing,
                };
                if row.selected {
                    dl.panel(
                        r,
                        if row.is_action {
                            badge_col.with_alpha(0.18)
                        } else {
                            theme.surface.selection
                        },
                        if row.is_action {
                            badge_col
                        } else {
                            theme.accent.focus.with_alpha(0.85)
                        },
                        theme.radii.badge,
                        1.1,
                        if row.is_action { 0.25 } else { 0.10 },
                    );
                } else {
                    dl.rounded(
                        r,
                        theme.surface.elevated.with_alpha(0.55),
                        theme.radii.badge,
                    );
                }

                let bw = 60.0;
                let br = Rect::new(r.x + 6.0, r.y + (r.h - 17.0) * 0.5, bw, 17.0);
                dl.panel(
                    br,
                    badge_col.with_alpha(0.16),
                    badge_col.with_alpha(0.65),
                    theme.radii.badge,
                    1.0,
                    0.0,
                );
                dl.text(
                    br,
                    row.badge,
                    theme.font.small - 1.5,
                    true,
                    badge_col,
                    TextAlign::Center,
                );

                let content_x = br.right() + 10.0;
                let content_w = (r.right() - content_x - 8.0).max(60.0);
                if is_form {
                    let top_r = Rect::new(content_x, r.y + 4.0, content_w, 18.0);
                    let sub_r = Rect::new(content_x, r.y + 22.0, content_w, 16.0);
                    let lbl_w = (content_w * 0.34).clamp(150.0, 260.0);
                    dl.text(
                        Rect::new(top_r.x, top_r.y, lbl_w, top_r.h),
                        &row.label,
                        theme.font.body,
                        row.selected || row.is_action,
                        if row.is_action && row.selected {
                            badge_col
                        } else if row.selected {
                            theme.accent.focus
                        } else {
                            theme.text.primary
                        },
                        TextAlign::Left,
                    );
                    let val_w = (content_w - lbl_w - 8.0).max(80.0);
                    let max_val = (val_w / cw) as usize;
                    dl.text(
                        Rect::new(top_r.x + lbl_w + 8.0, top_r.y, val_w, top_r.h),
                        trim_label(&row.value, max_val),
                        theme.font.small,
                        row.selected,
                        theme.accent.warning_key,
                        TextAlign::Right,
                    );
                    let max_det = (content_w / theme.char_width(theme.font.small - 1.0)) as usize;
                    dl.text(
                        sub_r,
                        trim_label(&row.detail, max_det),
                        theme.font.small - 1.0,
                        false,
                        theme.text.muted,
                        TextAlign::Left,
                    );
                } else {
                    let det_w = (content_w * 0.42).min(300.0);
                    let lbl_w = (content_w - det_w - 8.0).max(120.0);
                    dl.text(
                        Rect::new(content_x, r.y, lbl_w, r.h),
                        trim_label(&row.label, (lbl_w / cw) as usize),
                        theme.font.small,
                        row.selected,
                        if row.is_dir {
                            theme.accent.warning_key
                        } else if row.selected {
                            theme.accent.focus
                        } else {
                            theme.text.primary
                        },
                        TextAlign::Left,
                    );
                    dl.text(
                        Rect::new(r.right() - det_w - 8.0, r.y, det_w, r.h),
                        trim_label(&row.detail, (det_w / cw) as usize),
                        theme.font.small - 0.5,
                        false,
                        theme.text.muted,
                        TextAlign::Right,
                    );
                }
            }
        }
    }
    dl.pop_clip();

    // 4. Bottom Preview + optional Export Telemetry panel.
    dl.panel(
        bottom,
        theme.surface.elevated,
        theme.border.subtle,
        theme.radii.pill,
        1.0,
        0.0,
    );
    let b_inner = bottom.inset(10.0, 6.0);
    let max_p = (b_inner.w / cw) as usize;
    dl.text(
        Rect::new(b_inner.x, b_inner.y, b_inner.w, 18.0),
        trim_label(&hub.preview, max_p),
        theme.font.small,
        true,
        theme.accent.focus,
        TextAlign::Left,
    );

    if hub.active_tab == cadence_app::FileHubTab::Export {
        if let Some(sum) = &hub.export_summary {
            let clip_col = if sum.clips > 0 {
                theme.accent.alert
            } else {
                theme.accent.playhead
            };
            dl.text(
                Rect::new(b_inner.x, b_inner.y + 22.0, b_inner.w, 18.0),
                trim_label(
                    &format!(
                        "LAST EXPORT: {}  ·  {} file{}  ·  {:.1}s @ {} Hz",
                        sum.target,
                        sum.files_written,
                        if sum.files_written == 1 { "" } else { "s" },
                        sum.duration_sec,
                        sum.sample_rate
                    ),
                    max_p,
                ),
                theme.font.small,
                false,
                theme.text.primary,
                TextAlign::Left,
            );
            dl.text(
                Rect::new(b_inner.x, b_inner.y + 42.0, b_inner.w, 18.0),
                format!(
                    "Peak L {:+.1} dBFS  ·  Peak R {:+.1} dBFS  ·  RMS {:+.1} dBFS  ·  {} clips",
                    sum.peak_left_db, sum.peak_right_db, sum.rms_db, sum.clips
                ),
                theme.font.small,
                true,
                clip_col,
                TextAlign::Left,
            );
        }
    } else {
        let footer_text = if hub.searching {
            "type filter  ·  ↑/↓ select  ·  Enter apply filter  ·  Ctrl+u clear  ·  Esc clear search"
        } else if !hub.query.is_empty() {
            "H/L Tab 1–5 tabs  ·  j/k select  ·  Enter open/run  ·  / filter  ·  F Finder  ·  Esc clear filter"
        } else {
            match hub.active_tab {
                cadence_app::FileHubTab::Quick => {
                    "H/L Tab 1–5 tabs  ·  j/k select  ·  Enter open  ·  / search all  ·  2 Browse  ·  F Finder  ·  Esc close"
                }
                cadence_app::FileHubTab::Browse => {
                    "H/L Tab 1–5 tabs  ·  j/k select  ·  Enter open/dir  ·  Backspace up  ·  ~ home · . proj  ·  / filter  ·  F Finder"
                }
                cadence_app::FileHubTab::Import => {
                    "H/L Tab 1–5 tabs  ·  j/k select  ·  Enter import/dir  ·  Backspace up  ·  ~ home · . proj  ·  R reload  ·  F Finder"
                }
                cadence_app::FileHubTab::Save => {
                    "H/L Tab 1–5 tabs  ·  j/k select  ·  Enter run  ·  e edit path  ·  F Finder  ·  Esc close"
                }
                cadence_app::FileHubTab::Export => {
                    "H/L Tab 1–5 tabs  ·  j/k row  ·  Alt+h/l ,/. adjust  ·  e edit path  ·  Enter render  ·  F Finder  ·  Esc close"
                }
            }
        };
        dl.text(
            Rect::new(b_inner.x, b_inner.bottom() - 18.0, b_inner.w, 18.0),
            trim_label(
                footer_text,
                (b_inner.w / theme.char_width(theme.font.small - 0.5)) as usize,
            ),
            theme.font.small - 0.5,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
    }

    dl.pop_clip();
}
