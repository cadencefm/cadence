//! GPU Object decks (Placement, Part, Channel): grouped property bars.
use crate::chrome::{draw_deck_frame, DeckFrameSpec};
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::Theme;
use cadence_app::view::{ControlView, ObjectDeckView, UiModeKind};

pub fn draw_object(
    dl: &mut DisplayList,
    area: Rect,
    view: &ObjectDeckView,
    focused: bool,
    mode: UiModeKind,
    theme: &Theme,
) {
    let inner = draw_deck_frame(
        dl,
        area,
        &DeckFrameSpec {
            title: &view.title,
            right_badge: Some(view.kind.label()),
            accent: theme.accent.focus,
            focused,
            mode,
        },
        theme,
    );
    if inner.w < 120.0 || inner.h < 48.0 {
        return;
    }
    dl.push_clip(inner);
    draw_control_list(dl, inner, &view.controls, view.control_cursor, theme);
    dl.pop_clip();
}

const CONTROL_LIST_TWO_COL_MIN_WIDTH: f32 = 540.0;
const CONTROL_LIST_TWO_COL_MIN_ENTRIES: usize = 7;

fn control_list_entry_count(controls: &[ControlView]) -> usize {
    let multi_group = controls.windows(2).any(|w| w[0].module != w[1].module);
    let header_count = if multi_group {
        1 + controls
            .windows(2)
            .filter(|w| w[0].module != w[1].module)
            .count()
    } else {
        0
    };
    controls.len() + header_count
}

fn control_list_columns(width: f32, total_entries: usize) -> usize {
    if width >= CONTROL_LIST_TWO_COL_MIN_WIDTH && total_entries >= CONTROL_LIST_TWO_COL_MIN_ENTRIES
    {
        2
    } else {
        1
    }
}

/// Preferred height needed to render all `controls` (including module group headers and any
/// 2-column group continuation header) at `width` without clipping or pagination.
pub(crate) fn control_list_preferred_height(controls: &[ControlView], width: f32) -> f32 {
    let total_entries = control_list_entry_count(controls);
    let cols = control_list_columns(width, total_entries);
    let multi_group = controls.windows(2).any(|w| w[0].module != w[1].module);
    // Reserve 1 extra visual slot when a multi-group list splits across 2 columns so
    // keeping a group intact or inserting a `GROUP (CONT.)` header never clips column 2.
    let padded_entries = total_entries + usize::from(cols == 2 && multi_group);
    let needed_rows = padded_entries.div_ceil(cols).max(1);
    let row_h = if needed_rows >= 7 { 21.0 } else { 25.0 };
    needed_rows as f32 * row_h + 2.0
}

/// Shared keyboard-navigated parameter bar list with clear module group headers,
/// Shared keyboard-navigated parameter bar list (with optional Quick Panel pinned badges),
/// used by both Object and Instrument decks.
pub fn draw_control_list(
    dl: &mut DisplayList,
    area: Rect,
    controls: &[ControlView],
    cursor: usize,
    theme: &Theme,
) {
    if controls.is_empty() {
        dl.text(
            area,
            "No editable controls",
            theme.font.small,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
        return;
    }

    // Build visual entries interleaving module group headers when multiple groups exist.
    #[derive(Clone, Copy)]
    enum RowEntry<'a> {
        Header(&'static str),
        ContHeader(&'static str),
        Control(&'a ControlView),
    }
    let multi_group = controls.windows(2).any(|w| w[0].module != w[1].module);
    let mut entries: Vec<RowEntry<'_>> = Vec::with_capacity(controls.len() + 8);
    let mut last_mod: Option<&'static str> = None;
    let mut selected_entry_idx = 0usize;
    let sel = cursor.min(controls.len() - 1);
    for (idx, ctrl) in controls.iter().enumerate() {
        if multi_group && last_mod != Some(ctrl.module) {
            entries.push(RowEntry::Header(ctrl.module));
            last_mod = Some(ctrl.module);
        }
        if idx == sel {
            selected_entry_idx = entries.len();
        }
        entries.push(RowEntry::Control(ctrl));
    }

    let cols = control_list_columns(area.w, entries.len());
    let mut row_h = 25.0_f32;
    let mut rows_per_col = (area.h / row_h).floor().max(1.0) as usize;
    let padded_total = entries.len() + usize::from(cols == 2 && multi_group);
    if cols == 2 && padded_total > rows_per_col * cols {
        let needed = padded_total.div_ceil(cols).max(1);
        let compact = (area.h / needed as f32).floor();
        if compact >= 20.0 {
            row_h = compact.min(25.0);
            rows_per_col = (area.h / row_h).floor().max(1.0) as usize;
        }
    }
    let capacity = rows_per_col * cols;
    let page_step = if cols == 2 && multi_group && capacity > 2 {
        capacity - 1
    } else {
        capacity
    };
    let first = (selected_entry_idx / page_step) * page_step;
    let col_gap = 12.0;
    let col_w = (area.w - col_gap * (cols - 1) as f32) / cols as f32;

    // Build per-column visual rows so column 2 never begins with an orphaned unheaded group:
    // either pull a small split group whole into column 2 (when it fits cleanly), or insert a
    // clear `GROUP (CONT.)` continuation header at row 0 of column 2.
    let mut placements: Vec<(usize, usize, RowEntry<'_>)> = Vec::with_capacity(capacity);
    if cols == 1 {
        for (row, &entry) in entries.iter().skip(first).take(rows_per_col).enumerate() {
            placements.push((0, row, entry));
        }
    } else {
        let page = &entries[first..entries.len().min(first + capacity)];
        let mut split = page.len().min(rows_per_col);
        let mut cont_header: Option<&'static str> = None;
        if split > 0 && split < page.len() {
            if matches!(page[split - 1], RowEntry::Header(_)) {
                split -= 1;
            } else if multi_group {
                if let RowEntry::Control(ctrl) = page[split] {
                    // Try keeping the split group intact in column 1 if it starts after row 0
                    // and pulling it over does not overflow column 1's height.
                    let grp_start = page[..split]
                        .iter()
                        .rposition(|e| matches!(e, RowEntry::Header(_)));
                    if let Some(gs) = grp_start {
                        if gs > 0 && page.len().saturating_sub(gs) <= rows_per_col {
                            split = gs;
                        } else {
                            cont_header = Some(ctrl.module);
                        }
                    } else {
                        cont_header = Some(ctrl.module);
                    }
                }
            }
        }
        for (row, &entry) in page[..split].iter().enumerate() {
            placements.push((0, row, entry));
        }
        let mut col1_row = 0usize;
        if let Some(module) = cont_header {
            if col1_row < rows_per_col {
                placements.push((1, col1_row, RowEntry::ContHeader(module)));
                col1_row += 1;
            }
        }
        for &entry in &page[split..] {
            if col1_row >= rows_per_col {
                break;
            }
            // Never strand a section header on the final line of column 2 with zero controls under it.
            if col1_row + 1 == rows_per_col && matches!(entry, RowEntry::Header(_)) {
                break;
            }
            placements.push((1, col1_row, entry));
            col1_row += 1;
        }
    }

    for (col, row, entry) in placements {
        let x = area.x + col as f32 * (col_w + col_gap);
        let y = area.y + row as f32 * row_h;
        match entry {
            RowEntry::Header(module) | RowEntry::ContHeader(module) => {
                let is_cont = matches!(entry, RowEntry::ContHeader(_));
                let hr = Rect::new(x, y + 4.0, col_w, row_h - 6.0);
                dl.fill(
                    Rect::new(hr.x, hr.bottom() - 1.0, hr.w, 1.0),
                    theme.border.subtle.with_alpha(0.75),
                );
                let label = if is_cont {
                    format!("{module} (CONT.)")
                } else {
                    module.to_string()
                };
                dl.text(
                    Rect::new(hr.x + 2.0, hr.y, hr.w - 4.0, hr.h - 2.0),
                    label,
                    theme.font.small - 1.0,
                    true,
                    if is_cont {
                        theme.accent.routing.with_alpha(0.85)
                    } else {
                        theme.accent.routing
                    },
                    TextAlign::Left,
                );
            }
            RowEntry::Control(ctrl) => {
                let is_pinned = ctrl.pinned;
                let r = Rect::new(x, y, col_w, row_h - 3.0);
                if ctrl.selected {
                    dl.panel(
                        r,
                        theme.surface.selection,
                        theme.accent.focus.with_alpha(0.75),
                        theme.radii.badge,
                        1.2,
                        0.12,
                    );
                } else {
                    dl.rounded(
                        r,
                        theme.surface.recessed.with_alpha(0.65),
                        theme.radii.badge,
                    );
                }

                // Optional modulation indicator dot.
                if ctrl.controlled {
                    dl.rounded_glow(
                        Rect::new(r.x + 6.0, r.y + r.h * 0.5 - 3.0, 6.0, 6.0),
                        theme.accent.modulation,
                        3.0,
                        0.55,
                    );
                }

                let label_x = r.x + 18.0;
                let label_w = (r.w * 0.36).clamp(90.0, 190.0);
                let val_cw = theme.char_width(theme.font.small);
                let max_c = (label_w / val_cw) as usize;
                dl.text(
                    Rect::new(label_x, r.y, label_w, r.h),
                    trim_label(&ctrl.label, max_c),
                    theme.font.small,
                    ctrl.selected,
                    if ctrl.selected {
                        theme.accent.focus
                    } else {
                        theme.text.primary
                    },
                    TextAlign::Left,
                );

                // Optional inline driver pill (`M1`, Mod name, `Link:Part`) and MIDI badge (`CC 74` / `LEARN…`).
                let badge_font = theme.font.small - 1.5;
                let badge_cw = theme.char_width(badge_font);
                let badge_text = if ctrl.midi_learning {
                    Some("LEARN…")
                } else {
                    ctrl.midi_badge.as_deref()
                };
                let badge_w = if let Some(bt) = badge_text {
                    (bt.chars().count() as f32 * badge_cw + 10.0).min(78.0)
                } else {
                    0.0
                };
                let badge_gap = if badge_w > 0.0 { 6.0 } else { 0.0 };

                let driver_text = if !ctrl.driver_pills.is_empty() && r.w >= 260.0 {
                    if ctrl.driver_pills.len() == 1 {
                        Some(trim_label(&ctrl.driver_pills[0], 14))
                    } else {
                        Some(trim_label(
                            &format!(
                                "{} +{}",
                                trim_label(&ctrl.driver_pills[0], 10),
                                ctrl.driver_pills.len() - 1
                            ),
                            15,
                        ))
                    }
                } else {
                    None
                };
                let driver_w = if let Some(dt) = driver_text.as_deref() {
                    (dt.chars().count() as f32 * badge_cw + 10.0).min(96.0)
                } else {
                    0.0
                };
                let driver_gap = if driver_w > 0.0 { 5.0 } else { 0.0 };
                // Optional Quick Panel pinned glyph badge (`✶`) on module/zone/rack controls.
                let pin_w = if is_pinned { 18.0 } else { 0.0 };
                let pin_gap = if is_pinned { 5.0 } else { 0.0 };
                let pills_total_w = badge_w + badge_gap + driver_w + driver_gap + pin_w + pin_gap;

                // Text-only rows reclaim the unused bar region so long labels (e.g.
                // "Pat: Pad Gate Groove") stay on one line; numeric rows flex up cleanly
                // for live modulation spans while reserving bar width.
                let needed_val_w = ctrl.display.chars().count() as f32 * val_cw + 8.0;
                let max_text_val_w =
                    (r.right() - (label_x + label_w + 12.0) - pills_total_w - 6.0).max(76.0);
                let val_w = if ctrl.normalized.is_none() {
                    needed_val_w.clamp(76.0, max_text_val_w)
                } else {
                    let bar_val_cap = (r.w * 0.32).clamp(112.0, 164.0).min(max_text_val_w);
                    needed_val_w.clamp(76.0, bar_val_cap.max(76.0))
                };

                // Smooth GPU normalized parameter fill bar when parameter is numeric.
                let bar_x = label_x + label_w + 8.0;
                let bar_w = (r.right() - val_w - pills_total_w - bar_x - 10.0).max(0.0);
                if let Some(norm) = ctrl.normalized {
                    if bar_w >= 24.0 {
                        let clamped_norm = norm.clamp(0.0, 1.0);
                        let track = Rect::new(bar_x, r.y + r.h * 0.5 - 3.0, bar_w, 6.0);
                        dl.rounded(track, theme.meter.track_bg, 3.0);
                        let fill_w = (track.w * clamped_norm).max(0.0);
                        if fill_w > 0.5 {
                            let bar_col = if ctrl.midi_learning {
                                theme.accent.warning_key
                            } else if ctrl.controlled {
                                theme.accent.modulation
                            } else if ctrl.selected {
                                theme.accent.focus
                            } else {
                                theme.accent.routing
                            };
                            dl.rounded_glow(
                                Rect::new(track.x, track.y, fill_w, track.h),
                                bar_col,
                                3.0,
                                if ctrl.selected || ctrl.midi_learning {
                                    0.30
                                } else {
                                    0.10
                                },
                            );
                        }
                    }
                }

                let mut pill_right = r.right() - val_w - 10.0;
                if is_pinned {
                    let pr = Rect::new(pill_right - pin_w, r.y + 3.0, pin_w, (r.h - 6.0).max(12.0));
                    dl.panel(
                        pr,
                        theme.accent.warning_key.with_alpha(0.18),
                        theme.accent.warning_key.with_alpha(0.70),
                        theme.radii.badge,
                        1.0,
                        0.08,
                    );
                    dl.text(
                        pr,
                        "✶",
                        theme.font.small - 1.5,
                        true,
                        theme.accent.warning_key,
                        TextAlign::Center,
                    );
                    pill_right -= pin_w + pin_gap;
                }
                if let Some(bt) = badge_text {
                    let br = Rect::new(
                        pill_right - badge_w,
                        r.y + 3.0,
                        badge_w,
                        (r.h - 6.0).max(12.0),
                    );
                    let col = if ctrl.midi_learning {
                        theme.accent.warning_key
                    } else {
                        theme.accent.routing
                    };
                    dl.panel(
                        br,
                        col.with_alpha(if ctrl.midi_learning { 0.25 } else { 0.14 }),
                        col.with_alpha(0.75),
                        theme.radii.badge,
                        1.0,
                        if ctrl.midi_learning { 0.35 } else { 0.0 },
                    );
                    dl.text(
                        br,
                        bt,
                        badge_font,
                        ctrl.midi_learning,
                        col,
                        TextAlign::Center,
                    );
                    pill_right -= badge_w + badge_gap;
                }
                if let Some(dt) = driver_text.as_deref() {
                    let dr = Rect::new(
                        pill_right - driver_w,
                        r.y + 3.0,
                        driver_w,
                        (r.h - 6.0).max(12.0),
                    );
                    let col = theme.accent.modulation;
                    dl.panel(
                        dr,
                        col.with_alpha(0.18),
                        col.with_alpha(0.80),
                        theme.radii.badge,
                        1.0,
                        0.08,
                    );
                    dl.text(dr, dt, badge_font, true, col, TextAlign::Center);
                }

                let max_val_c = (val_w / val_cw).floor().max(4.0) as usize;
                dl.text(
                    Rect::new(r.right() - val_w - 6.0, r.y, val_w, r.h),
                    trim_label(&ctrl.display, max_val_c),
                    theme.font.small,
                    ctrl.selected,
                    if ctrl.selected {
                        theme.accent.warning_key
                    } else {
                        theme.text.secondary
                    },
                    TextAlign::Right,
                );
            }
        }
    }
}
