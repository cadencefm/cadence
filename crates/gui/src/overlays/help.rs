//! Searchable `?` Help & Reference overlay with categorized tabs, live `/` filtering,
//! and dynamically sized multi-line word-wrapped rows.
use super::overlay_frame;
use crate::display::{DisplayList, Rect, TextAlign};
use crate::theme::Theme;
use cadence_app::view::{HelpEntry, HelpView};

const ROW_PAD_Y: f32 = 12.0;
const MIN_ROW_H: f32 = 30.0;
const CAT_BADGE_H: f32 = 20.0;
const KEY_PILL_PAD_X: f32 = 6.0;
const KEY_PILL_PAD_Y: f32 = 4.0;
const MIN_KEY_PILL_H: f32 = 22.0;

#[derive(Clone, Debug, PartialEq)]
struct HelpRowLayout {
    key_lines: Vec<String>,
    desc_lines: Vec<String>,
    height: f32,
}

#[derive(Clone, Copy, Debug)]
struct HelpColumns {
    cat_x: f32,
    cat_w: f32,
    key_x: f32,
    key_w: f32,
    desc_x: f32,
    desc_w: f32,
    text_size: f32,
    line_h: f32,
    max_key_chars: usize,
    max_desc_chars: usize,
}

impl HelpColumns {
    fn compute(body: Rect, theme: &Theme) -> Self {
        let text_size = theme.font.small - 0.5;
        let char_w = theme.char_width(text_size).max(1.0);
        let line_h = (text_size * 1.42).ceil();

        let cat_x = body.x + 4.0;
        let cat_w = 104.0_f32.min((body.w * 0.16).max(80.0));
        let key_x = cat_x + cat_w + 8.0;
        let max_key_w = (body.w - cat_w - 160.0).max(120.0);
        let key_w = (body.w * 0.33).clamp(220.0, 360.0).min(max_key_w);
        let desc_x = key_x + key_w + 10.0;
        let desc_w = (body.right() - desc_x - 6.0).max(char_w);

        let key_inner_w = (key_w - KEY_PILL_PAD_X * 2.0 - 2.0).max(char_w);
        let desc_inner_w = (desc_w - 2.0).max(char_w);
        let max_key_chars = (key_inner_w / char_w).floor().max(1.0) as usize;
        let max_desc_chars = (desc_inner_w / char_w).floor().max(1.0) as usize;

        Self {
            cat_x,
            cat_w,
            key_x,
            key_w,
            desc_x,
            desc_w,
            text_size,
            line_h,
            max_key_chars,
            max_desc_chars,
        }
    }
}

/// Word-wrap `text` into lines of at most `max_chars` Unicode scalar values.
///
/// Splits cleanly on whitespace boundaries; overlong single tokens (such as
/// `eq|filter|comp|...` or `:param/:mod/:link/:macro`) break at natural delimiters
/// (`|`, `/`, `,`, `:`, `-`) before falling back to safe char-boundary splits.
pub(super) fn wrap_text(text: &str, max_chars: usize) -> Vec<String> {
    let limit = max_chars.max(1);
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut cur_len = 0usize;

    for word in text.split_whitespace() {
        let wlen = word.chars().count();
        if cur_len > 0 && cur_len + 1 + wlen <= limit {
            cur.push(' ');
            cur.push_str(word);
            cur_len += 1 + wlen;
            continue;
        }
        if cur_len > 0 {
            lines.push(std::mem::take(&mut cur));
            cur_len = 0;
        }
        push_word_chunked(&mut lines, &mut cur, &mut cur_len, word, limit);
    }

    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

fn push_word_chunked(
    lines: &mut Vec<String>,
    cur: &mut String,
    cur_len: &mut usize,
    mut rest: &str,
    limit: usize,
) {
    loop {
        let rest_len = rest.chars().count();
        if rest_len <= limit {
            cur.push_str(rest);
            *cur_len = rest_len;
            return;
        }
        // Prefer breaking immediately after a natural delimiter within `1..=limit`.
        let mut delim_split: Option<usize> = None;
        let mut hard_split = rest.len();
        for (ci, (bi, ch)) in rest.char_indices().enumerate() {
            let next_bi = bi + ch.len_utf8();
            if ci < limit && matches!(ch, '|' | '/' | ',' | ':' | '-') {
                delim_split = Some(next_bi);
            }
            if ci + 1 == limit {
                hard_split = next_bi;
                break;
            }
        }
        let split_byte = delim_split.unwrap_or(hard_split);
        let (head, tail) = rest.split_at(split_byte);
        lines.push(head.to_owned());
        rest = tail;
        if rest.is_empty() {
            *cur_len = 0;
            return;
        }
    }
}

fn layout_entries(entries: &[HelpEntry], cols: &HelpColumns) -> Vec<HelpRowLayout> {
    entries
        .iter()
        .map(|entry| {
            let key_lines = wrap_text(entry.keys, cols.max_key_chars);
            let desc_lines = wrap_text(entry.summary, cols.max_desc_chars);
            let lines = key_lines.len().max(desc_lines.len()).max(1);
            let height = (ROW_PAD_Y + lines as f32 * cols.line_h).max(MIN_ROW_H);
            HelpRowLayout {
                key_lines,
                desc_lines,
                height,
            }
        })
        .collect()
}

/// Compute the clamped visible slice `start..end` for variable-height rows inside `viewport_h`.
///
/// Pins the final page cleanly to the bottom (matching fixed-height `scroll_window_clamped`)
/// and only includes complete rows (always rendering at least 1 row on tiny viewports).
fn compute_visible_window(
    rows: &[HelpRowLayout],
    scroll: usize,
    viewport_h: f32,
) -> (usize, usize) {
    if rows.is_empty() {
        return (0, 0);
    }
    let last_idx = rows.len() - 1;
    let mut last_page_start = last_idx;
    let mut tail_h = rows[last_idx].height;
    while last_page_start > 0 {
        let prev_h = rows[last_page_start - 1].height;
        if tail_h + prev_h <= viewport_h + 0.5 {
            last_page_start -= 1;
            tail_h += prev_h;
        } else {
            break;
        }
    }

    let start = scroll.min(last_page_start);
    let mut end = start;
    let mut used_h = 0.0;
    while end < rows.len() {
        let h = rows[end].height;
        if end > start && used_h + h > viewport_h + 0.5 {
            break;
        }
        used_h += h;
        end += 1;
    }
    (start, end)
}

pub(super) fn draw_help(dl: &mut DisplayList, rect: Rect, view: &HelpView, theme: &Theme) {
    let title = format!(
        "CADENCE  ·  HELP & REFERENCE  ({}/{})",
        view.entries.len(),
        view.total_entries
    );
    let inner = overlay_frame(dl, rect, &title, theme.accent.focus, theme);
    dl.push_clip(inner);

    // 1. Top tab pills (matching Rack deck tabs).
    let (tab_row, rest) = inner.split_top(28.0);
    let mut tx = tab_row.x;
    let cw = theme.char_width(theme.font.small);
    let filtering = !view.query.trim().is_empty();
    for (label, active) in &view.tabs {
        let tw = label.chars().count() as f32 * cw + 18.0;
        if tx + tw > tab_row.right() {
            break;
        }
        let tr = Rect::new(tx, tab_row.y + 1.0, tw, 22.0);
        let is_active = *active && !filtering;
        dl.panel(
            tr,
            if is_active {
                theme.accent.focus.with_alpha(0.22)
            } else {
                theme.surface.recessed
            },
            if is_active {
                theme.accent.focus
            } else {
                theme.border.subtle
            },
            theme.radii.badge,
            1.0,
            if is_active { 0.16 } else { 0.0 },
        );
        dl.text(
            tr,
            *label,
            theme.font.small,
            is_active,
            if is_active {
                theme.accent.focus
            } else {
                theme.text.secondary
            },
            TextAlign::Center,
        );
        tx += tw + 5.0;
    }

    // 2. Live `/` search filter bar right below tabs.
    let (search_row, rest) = rest.split_top(30.0);
    let search_box = Rect::new(search_row.x, search_row.y + 2.0, search_row.w, 24.0);
    dl.panel(
        search_box,
        theme.surface.recessed,
        if view.searching {
            theme.accent.focus
        } else if filtering {
            theme.accent.warning_key
        } else {
            theme.border.hairline
        },
        theme.radii.badge,
        if view.searching { 1.4 } else { 1.0 },
        if view.searching { 0.18 } else { 0.0 },
    );
    let search_text = if view.searching {
        format!("/ {}▏", view.query)
    } else if filtering {
        format!("/ {}   (searching all tabs · Esc clears)", view.query)
    } else {
        "/  Press / to search keybindings, commands, or algebra ops...".into()
    };
    dl.text(
        search_box.inset(8.0, 0.0),
        search_text,
        theme.font.small,
        view.searching || filtering,
        if view.searching {
            theme.accent.focus
        } else if filtering {
            theme.accent.warning_key
        } else {
            theme.text.muted
        },
        TextAlign::Left,
    );

    // 3. Multi-line word-wrapped filtered entry list.
    let (body, footer) = rest.split_bottom(24.0);
    let cols = HelpColumns::compute(body, theme);
    let layouts = layout_entries(&view.entries, &cols);
    let (start, end) = compute_visible_window(&layouts, view.scroll, body.h);

    dl.push_clip(body);
    if view.entries.is_empty() {
        dl.text(
            body.inset(8.0, 12.0),
            "No matching help entries · press Esc to clear filter",
            theme.font.body,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
    } else {
        let mut y = body.y;
        for (idx, (entry, layout)) in view.entries[start..end]
            .iter()
            .zip(&layouts[start..end])
            .enumerate()
        {
            let abs_idx = start + idx;
            let row_h = layout.height;
            if abs_idx % 2 == 1 {
                dl.rounded(
                    Rect::new(body.x, y + 1.0, body.w, row_h - 2.0),
                    theme.surface.recessed.with_alpha(0.45),
                    theme.radii.badge,
                );
            }

            // Category badge vertically centered within the multi-line row.
            let badge_y = y + (row_h - CAT_BADGE_H) * 0.5;
            let badge = Rect::new(cols.cat_x, badge_y, cols.cat_w, CAT_BADGE_H);
            dl.rounded(badge, theme.surface.recessed, theme.radii.badge);
            dl.text(
                badge,
                entry.category,
                theme.font.small - 1.0,
                true,
                theme.accent.routing,
                TextAlign::Center,
            );

            // Keybinding pill expands vertically when wrapped and centers within the row.
            let key_text_h = layout.key_lines.len() as f32 * cols.line_h;
            let key_pill_h = (key_text_h + KEY_PILL_PAD_Y * 2.0).max(MIN_KEY_PILL_H);
            let key_pill_y = y + (row_h - key_pill_h) * 0.5;
            let krect = Rect::new(cols.key_x, key_pill_y, cols.key_w, key_pill_h);
            dl.rounded(
                krect,
                theme.accent.warning_key.with_alpha(0.12),
                theme.radii.badge,
            );
            let key_top = krect.y + (krect.h - key_text_h) * 0.5;
            let key_inner_w = (krect.w - KEY_PILL_PAD_X * 2.0).max(0.0);
            for (li, line) in layout.key_lines.iter().enumerate() {
                dl.text(
                    Rect::new(
                        krect.x + KEY_PILL_PAD_X,
                        key_top + li as f32 * cols.line_h,
                        key_inner_w,
                        cols.line_h,
                    ),
                    line.as_str(),
                    cols.text_size,
                    true,
                    theme.accent.warning_key,
                    TextAlign::Left,
                );
            }

            // Description block vertically centered within the multi-line row.
            let desc_text_h = layout.desc_lines.len() as f32 * cols.line_h;
            let desc_top = y + (row_h - desc_text_h) * 0.5;
            for (li, line) in layout.desc_lines.iter().enumerate() {
                dl.text(
                    Rect::new(
                        cols.desc_x,
                        desc_top + li as f32 * cols.line_h,
                        cols.desc_w,
                        cols.line_h,
                    ),
                    line.as_str(),
                    cols.text_size,
                    false,
                    theme.text.primary,
                    TextAlign::Left,
                );
            }

            y += row_h;
        }
    }
    dl.pop_clip();

    dl.text(
        footer,
        "H/L or Tab switch tab  ·  / search  ·  j/k scroll  ·  ? or Esc close",
        theme.font.small,
        true,
        theme.accent.warning_key,
        TextAlign::Left,
    );
    if !view.entries.is_empty() {
        let range_label = format!("rows {}–{} / {}", start + 1, end, view.entries.len());
        dl.text(
            footer,
            range_label,
            theme.font.small,
            false,
            theme.text.muted,
            TextAlign::Right,
        );
    }
    dl.pop_clip();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlays::centered;
    use cadence_app::view::{HelpTab, HELP_ENTRIES};

    #[test]
    fn wrap_text_breaks_cleanly_on_words_and_long_delimiters_without_overflow() {
        let prose = "Interactive 5/6-column Signal Graph incl. FX nodes: open Channel/Effect, route bus, add/edit/unlink Send/Mod/Duck cables";
        let lines = wrap_text(prose, 42);
        assert!(lines.len() >= 3, "long summary must wrap into >= 3 lines");
        assert!(lines.iter().all(|l| l.chars().count() <= 42));
        assert_eq!(lines.join(" "), prose);

        // Long unspaced pipe/slash token breaks cleanly at delimiters with zero overflow.
        let fx_cmd = "f or :fx eq|filter|comp|limiter|drive|delay|chorus|reverb";
        let fx_lines = wrap_text(fx_cmd, 24);
        assert!(fx_lines.len() >= 3);
        assert!(fx_lines.iter().all(|l| l.chars().count() <= 24));
        assert!(
            fx_lines.iter().any(|l| l.ends_with('|')),
            "delimiter-aware wrap should split cleanly after '|': {fx_lines:?}"
        );

        // Unicode middle dots and arrows never panic or exceed limit.
        let uni = "ga · gb · gp · gm · gt · gG · gd · gk · gi · gf · g, → jump";
        let uni_lines = wrap_text(uni, 20);
        assert!(uni_lines.iter().all(|l| l.chars().count() <= 20));
        assert_eq!(uni_lines.join(" "), uni);
    }

    #[test]
    fn variable_height_scroll_clamps_last_page_and_preserves_complete_rows() {
        let rows = vec![
            HelpRowLayout {
                key_lines: vec!["a".into()],
                desc_lines: vec!["short".into()],
                height: 30.0,
            },
            HelpRowLayout {
                key_lines: vec!["b1".into(), "b2".into()],
                desc_lines: vec!["l1".into(), "l2".into(), "l3".into()],
                height: 60.0,
            },
            HelpRowLayout {
                key_lines: vec!["c".into()],
                desc_lines: vec!["l1".into(), "l2".into()],
                height: 44.0,
            },
            HelpRowLayout {
                key_lines: vec!["d".into()],
                desc_lines: vec!["l1".into(), "l2".into()],
                height: 44.0,
            },
            HelpRowLayout {
                key_lines: vec!["e".into()],
                desc_lines: vec!["short".into()],
                height: 30.0,
            },
        ];

        // Viewport 100px:
        // Tail: row 4 (30) + row 3 (44) = 74 <= 100; adding row 2 (44) = 118 > 100 -> last_page_start = 3.
        assert_eq!(compute_visible_window(&rows, 0, 100.0), (0, 2)); // 30 + 60 = 90 <= 100
        assert_eq!(compute_visible_window(&rows, 1, 100.0), (1, 2)); // 60 + 44 = 104 > 100 -> only row 1
        assert_eq!(compute_visible_window(&rows, 2, 100.0), (2, 4)); // 44 + 44 = 88 <= 100
        assert_eq!(compute_visible_window(&rows, 3, 100.0), (3, 5)); // 44 + 30 = 74 <= 100
        assert_eq!(
            compute_visible_window(&rows, 99, 100.0),
            (3, 5),
            "overscrolling must clamp cleanly to last page"
        );

        // Tall viewport where every row fits -> always pinned to 0..5 regardless of scroll.
        assert_eq!(compute_visible_window(&rows, 4, 300.0), (0, 5));

        // Tiny viewport smaller than a single row still renders 1 row cleanly.
        assert_eq!(compute_visible_window(&rows, 0, 20.0), (0, 1));
    }

    #[test]
    fn draw_help_wraps_long_entries_with_zero_ellipsis_truncation_at_1440_and_1100() {
        let theme = Theme::cadence_dark();

        for (vw, vh) in [(1440.0_f32, 860.0_f32), (1100.0_f32, 720.0_f32)] {
            let viewport = Rect::new(0.0, 0.0, vw, vh);
            let modal = centered(viewport, 1120.0, 590.0);

            // Check Navigate tab (DECKS, GRAPH) and Sound & FX tab (RACK PANEL, PATCHBAY, RACK ZONES, STUDIO FX).
            for tab in [HelpTab::Navigate, HelpTab::SoundFx, HelpTab::LiveJam] {
                let entries: Vec<HelpEntry> = HELP_ENTRIES
                    .iter()
                    .copied()
                    .filter(|e| e.tab == tab)
                    .collect();
                let view = HelpView {
                    tabs: HelpTab::ALL
                        .iter()
                        .map(|t| (t.label(), *t == tab))
                        .collect(),
                    active_tab: 0,
                    query: String::new(),
                    searching: false,
                    scroll: 0,
                    total_entries: HELP_ENTRIES.len(),
                    entries: entries.clone(),
                };

                let mut dl = DisplayList::new(vw, vh, 1.0);
                dl.begin_overlay();
                draw_help(&mut dl, modal, &view, &theme);

                // Zero horizontal ellipsis clipping on any rendered help text run!
                for run in &dl.overlay.texts {
                    assert!(
                        !run.text.contains('…'),
                        "Help entry text must wrap instead of truncating with ellipsis at {vw}x{vh} ({tab:?}): {:?}",
                        run.text
                    );
                }

                // Every word of every entry's summary must appear in emitted text runs (across scroll
                // pages whenever a tab has more wrapped rows than fit in one viewport).
                let body_rect = Rect::new(0.0, 0.0, modal.w - 27.0, modal.h - 133.0);
                let cols_check = HelpColumns::compute(body_rect, &theme);
                let layouts_check = layout_entries(&entries, &cols_check);
                let (_, first_end) = compute_visible_window(&layouts_check, 0, body_rect.h);
                let mut all_text: String = dl
                    .overlay
                    .texts
                    .iter()
                    .map(|t| t.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                if first_end < entries.len() {
                    let mut scrolled_view = view.clone();
                    scrolled_view.scroll = entries.len();
                    let mut dl_bot = DisplayList::new(vw, vh, 1.0);
                    dl_bot.begin_overlay();
                    draw_help(&mut dl_bot, modal, &scrolled_view, &theme);
                    for run in &dl_bot.overlay.texts {
                        assert!(
                            !run.text.contains('…'),
                            "Scrolled Help entry truncated at {vw}x{vh} ({tab:?}): {:?}",
                            run.text
                        );
                        all_text.push(' ');
                        all_text.push_str(&run.text);
                    }
                }
                for entry in &entries {
                    for token in entry.summary.split_whitespace() {
                        assert!(
                            all_text.contains(token),
                            "missing summary token {token:?} in {tab:?} at {vw}x{vh}"
                        );
                    }
                }

                // Specifically verify long entries wrap into multiple lines (> 1 text run for summary).
                if tab == HelpTab::Navigate {
                    let cols = HelpColumns::compute(
                        Rect::new(0.0, 0.0, modal.w - 27.0, modal.h - 133.0),
                        &theme,
                    );
                    let graph = entries.iter().find(|e| e.category == "GRAPH").unwrap();
                    let graph_layout = &layout_entries(&[*graph], &cols)[0];
                    assert!(
                        graph_layout.desc_lines.len() >= 2,
                        "GRAPH summary should wrap into >= 2 lines at {vw}x{vh}, got {:?}",
                        graph_layout.desc_lines
                    );
                    assert!(graph_layout.height > MIN_ROW_H);
                }
            }
        }
    }
}
