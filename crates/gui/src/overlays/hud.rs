//! Floating performance HUDs & inspection overlays: WhichKey Prefix Menu, Jam HUD,
//! Lane Layer HUD, Event Provenance Inspection, Inspect/Assets text viewer, and Tutorial.
use super::{overlay_frame, scroll_window_clamped};
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::Theme;
use cadence_app::view::{JamHudView, LaneLayerView};
use cadence_app::EventInspectionView;
use cadence_core::music::note_name;

/// Crisp floating bottom-right WhichKey HUD for `g` / `t` / `z` / `v` / Stage prefixes.
pub(super) fn draw_prefix_menu(
    dl: &mut DisplayList,
    viewport: Rect,
    title: &str,
    items: &[(String, String)],
    theme: &Theme,
) {
    let cols = if items.len() > 6 { 2 } else { 1 };
    let rows = items.len().div_ceil(cols);
    let w = (if cols == 2 { 520.0_f32 } else { 290.0_f32 }).min(viewport.w - 32.0);
    let h = (44.0 + rows as f32 * 24.0).min(viewport.h - 140.0);
    let rect = Rect::new(
        viewport.right() - w - 16.0,
        viewport.bottom() - h - 70.0,
        w,
        h,
    );
    let inner = overlay_frame(dl, rect, title, theme.accent.warning_key, theme);
    dl.push_clip(inner);
    let col_w = inner.w / cols as f32;
    for (idx, (key, desc)) in items.iter().enumerate() {
        let col = idx / rows.max(1);
        let row = idx % rows.max(1);
        let y = inner.y + row as f32 * 24.0;
        let x = inner.x + col as f32 * col_w;
        let kw = 64.0_f32.min(col_w * 0.35);
        let krect = Rect::new(x, y + 2.0, kw, 20.0);
        dl.rounded(krect, theme.surface.recessed, theme.radii.badge);
        dl.text(
            krect,
            key.as_str(),
            theme.font.small,
            true,
            theme.accent.warning_key,
            TextAlign::Center,
        );
        dl.text(
            Rect::new(krect.right() + 8.0, y, (col_w - kw - 14.0).max(0.0), 24.0),
            desc.as_str(),
            theme.font.small,
            false,
            theme.text.primary,
            TextAlign::Left,
        );
    }
    dl.pop_clip();
}

pub(super) fn draw_event_inspection(
    dl: &mut DisplayList,
    rect: Rect,
    insp: &EventInspectionView,
    theme: &Theme,
) {
    let inner = overlay_frame(
        dl,
        rect,
        "EVENT PROVENANCE  ·  SOURCE GRAPH",
        theme.accent.modulation,
        theme,
    );
    dl.push_clip(inner);
    if let Some(n) = &insp.event {
        dl.text(
            Rect::new(inner.x, inner.y, inner.w, 22.0),
            format!(
                "{}  ·  vel {:.0}%  ·  gate {:.2}  ·  {}",
                note_name(n.midi),
                n.velocity * 100.0,
                n.gate,
                if n.local_exception {
                    "LOCAL EXCEPTION"
                } else {
                    "SOURCE RESULT"
                }
            ),
            theme.font.body,
            true,
            theme.accent.focus,
            TextAlign::Left,
        );
    }
    if let Some(summary) = insp.summary.first() {
        dl.text(
            Rect::new(inner.x, inner.y + 24.0, inner.w, 20.0),
            summary.as_str(),
            theme.font.small,
            false,
            theme.text.secondary,
            TextAlign::Left,
        );
    }

    let list_y = inner.y + 54.0;
    let row_h = 24.0;
    for (i, src) in insp.sources.iter().enumerate() {
        let y = list_y + i as f32 * row_h;
        if y + row_h > inner.bottom() - 26.0 {
            break;
        }
        let r = Rect::new(inner.x, y, inner.w, row_h - 2.0);
        let sel = i == insp.selected;
        if sel {
            dl.rounded(r, theme.surface.selection, theme.radii.badge);
        }
        let indent = "  ".repeat(src.depth.min(6));
        dl.text(
            r.inset(8.0, 0.0),
            format!(
                "{}{} {}  ({})  ·  step {}  →  {}",
                if sel { "› " } else { "  " },
                indent,
                src.name,
                src.role,
                src.step + 1,
                src.value
            ),
            theme.font.small,
            sel,
            if sel {
                theme.accent.focus
            } else {
                theme.text.primary
            },
            TextAlign::Left,
        );
    }

    dl.text(
        Rect::new(inner.x, inner.bottom() - 20.0, inner.w, 20.0),
        "j/k source  ·  Enter/s follow source  ·  o local occurrence  ·  i instrument  ·  Esc back",
        theme.font.small,
        true,
        theme.accent.warning_key,
        TextAlign::Left,
    );
    dl.pop_clip();
}

fn wrap_inspect_line(line: &str, max_chars: usize) -> Vec<String> {
    let limit = max_chars.max(1);
    if line.chars().count() <= limit {
        return vec![line.to_owned()];
    }
    let trimmed = line.trim_start_matches(' ');
    if trimmed.is_empty() {
        return vec![String::new()];
    }
    let raw_indent = line.len() - trimmed.len();
    let indent_len = raw_indent.min(limit / 3);
    let indent = &line[..indent_len];
    let content_limit = (limit - indent_len).max(1);
    super::help::wrap_text(trimmed, content_limit)
        .into_iter()
        .map(|seg| format!("{indent}{seg}"))
        .collect()
}

pub(super) fn draw_inspect(
    dl: &mut DisplayList,
    rect: Rect,
    title: &str,
    lines: &[String],
    scroll: usize,
    theme: &Theme,
) {
    let inner = overlay_frame(dl, rect, title, theme.accent.routing, theme);
    dl.push_clip(inner);
    let row_h = 18.0;
    let cw = theme.char_width(theme.font.small).max(1.0);
    let max_chars = ((inner.w - 4.0) / cw).floor().max(1.0) as usize;
    let wrapped: Vec<String> = lines
        .iter()
        .flat_map(|line| wrap_inspect_line(line, max_chars))
        .collect();
    let visible = ((inner.h - 26.0) / row_h).floor().max(1.0) as usize;
    let start = scroll_window_clamped(scroll, wrapped.len(), visible);
    let end = (start + visible).min(wrapped.len());
    for (i, line) in wrapped[start..end].iter().enumerate() {
        dl.text(
            Rect::new(inner.x, inner.y + i as f32 * row_h, inner.w, row_h),
            line.as_str(),
            theme.font.small,
            false,
            theme.text.primary,
            TextAlign::Left,
        );
    }
    let footer = if title.starts_with("ASSETS") {
        "j/k scroll  ·  r reload samples  ·  Esc back"
    } else {
        "j/k scroll  ·  Esc back"
    };
    let footer_rect = Rect::new(inner.x, inner.bottom() - 20.0, inner.w, 20.0);
    dl.text(
        footer_rect,
        footer,
        theme.font.small,
        true,
        theme.accent.warning_key,
        TextAlign::Left,
    );
    if !wrapped.is_empty() && inner.w >= 340.0 {
        dl.text(
            footer_rect,
            format!("lines {}–{} / {}", start + 1, end, wrapped.len()),
            theme.font.small,
            false,
            theme.text.muted,
            TextAlign::Right,
        );
    }
    dl.pop_clip();
}

pub(super) struct TutorialView<'a> {
    pub step: usize,
    pub total: usize,
    pub title: &'a str,
    pub body: &'a str,
    pub command: &'a str,
}

pub(super) fn draw_tutorial(
    dl: &mut DisplayList,
    rect: Rect,
    tutorial: &TutorialView<'_>,
    theme: &Theme,
) {
    let inner = overlay_frame(
        dl,
        rect,
        &format!(
            "TUTORIAL  {}/{}  ·  {}",
            tutorial.step, tutorial.total, tutorial.title
        ),
        theme.accent.focus,
        theme,
    );
    dl.push_clip(inner);
    dl.text(
        Rect::new(inner.x, inner.y + 6.0, inner.w, 60.0),
        tutorial.body,
        theme.font.body,
        false,
        theme.text.primary,
        TextAlign::Left,
    );
    let cmd_box = Rect::new(inner.x, inner.y + 78.0, inner.w, 34.0);
    dl.panel(
        cmd_box,
        theme.surface.recessed,
        theme.accent.focus,
        theme.radii.badge,
        1.0,
        0.10,
    );
    dl.text(
        cmd_box.inset(10.0, 0.0),
        format!(":{}", tutorial.command),
        theme.font.body,
        true,
        theme.accent.focus,
        TextAlign::Left,
    );
    dl.text(
        Rect::new(inner.x, inner.bottom() - 20.0, inner.w, 20.0),
        "Enter load into action palette  ·  j/k prev/next lesson  ·  Esc close",
        theme.font.small,
        true,
        theme.accent.warning_key,
        TextAlign::Left,
    );
    dl.pop_clip();
}

/// Dedicated translucent GPU Jam drawer anchored above the footer while in Jam mode:
/// displays `1..0` sections, `q..p` lanes, and a live 13-key piano keyboard (`z..m ,` + black keys)
/// that lights up with phosphor bloom as notes are pressed!
pub fn draw_jam_hud(
    dl: &mut DisplayList,
    viewport: Rect,
    footer_h: f32,
    jam: &JamHudView,
    theme: &Theme,
) {
    let w = (viewport.w - 32.0).clamp(520.0, 1140.0);
    let h = 122.0;
    let rect = Rect::new(
        (viewport.w - w) * 0.5,
        viewport.bottom() - footer_h - h - 8.0,
        w,
        h,
    );
    dl.panel(
        rect,
        theme.surface.overlay.with_alpha(0.95),
        theme.accent.jam,
        theme.radii.panel,
        1.8,
        theme.glow.jam_border * 0.65,
    );
    let inner = rect.inset(12.0, 8.0);
    dl.push_clip(inner);
    let (left, kb_area) = inner.split_right(308.0);
    let left = Rect::new(left.x, left.y, (left.w - 14.0).max(0.0), left.h);

    // Header row inside Jam HUD.
    let (hdr, rows) = left.split_top(20.0);
    let bank_suffix = if jam.bank_count > 1 {
        format!(
            "  ·  Bank {}/{}  (Tab bank · </> oct · Shift accent · Esc leave)",
            jam.bank + 1,
            jam.bank_count
        )
    } else {
        "  (</> shift · Shift accent · Esc leave)".into()
    };
    dl.text(
        hdr,
        format!(
            "JAM SURFACE  ·  {}  ·  Octave {}{bank_suffix}",
            jam.instrument_name, jam.octave
        ),
        theme.font.small,
        true,
        theme.accent.jam,
        TextAlign::Left,
    );

    // Row 1: Sections (1..0).
    let (sec_row, lane_row) = rows.split_top(38.0);
    let cw = theme.char_width(theme.font.small - 0.5);
    let mut sx = sec_row.x;
    for (k, name, active, queued) in &jam.sections {
        let lbl = format!("{k} {}", trim_label(name, 9));
        let pw = lbl.chars().count() as f32 * cw + 14.0;
        if sx + pw > sec_row.right() {
            break;
        }
        let r = Rect::new(sx, sec_row.y + 6.0, pw, 24.0);
        let (bg, border, fg, glow) = if *queued {
            (
                theme.accent.warning_key.with_alpha(0.28),
                theme.accent.warning_key,
                theme.accent.warning_key,
                0.38,
            )
        } else if *active {
            (
                theme.accent.playhead.with_alpha(0.24),
                theme.accent.playhead,
                theme.accent.playhead,
                0.26,
            )
        } else {
            (
                theme.surface.recessed,
                theme.border.hairline,
                theme.text.secondary,
                0.0,
            )
        };
        dl.panel(r, bg, border, theme.radii.badge, 1.0, glow);
        dl.text(
            r.inset(5.0, 0.0),
            lbl,
            theme.font.small - 0.5,
            *active || *queued,
            fg,
            TextAlign::Left,
        );
        sx += pw + 5.0;
    }

    // Row 2: Lanes (q..p).
    let mut lx = lane_row.x;
    for (idx, (k, name, focused, overriding, muted, solo)) in jam.lanes.iter().enumerate() {
        let col = theme.track_color(jam.bank * 10 + idx);
        let tag = if *overriding {
            " LIVE"
        } else if *muted {
            " M"
        } else if *solo {
            " S"
        } else {
            ""
        };
        let lbl = format!("{k} {}{tag}", trim_label(name, 8));
        let pw = lbl.chars().count() as f32 * cw + 14.0;
        if lx + pw > lane_row.right() {
            break;
        }
        let r = Rect::new(lx, lane_row.y + 6.0, pw, 26.0);
        let border = if *overriding {
            theme.accent.override_bar
        } else if *focused {
            col
        } else {
            theme.border.subtle
        };
        let bg = if *overriding {
            theme.accent.override_bar.with_alpha(0.22)
        } else if *focused {
            col.with_alpha(0.22)
        } else {
            theme.surface.recessed
        };
        dl.panel(
            r,
            bg,
            border,
            theme.radii.badge,
            if *focused || *overriding { 1.4 } else { 1.0 },
            if *overriding {
                0.35
            } else if *focused {
                0.18
            } else {
                0.0
            },
        );
        dl.text(
            r.inset(5.0, 0.0),
            lbl,
            theme.font.small - 0.5,
            *focused || *overriding,
            if *overriding {
                theme.accent.override_bar
            } else if *focused {
                col
            } else if *muted {
                theme.text.muted
            } else {
                theme.text.secondary
            },
            TextAlign::Left,
        );
        lx += pw + 5.0;
    }

    // Right side: Interactive 13-key GPU piano keyboard (z..m , + s d g h j).
    let base_midi = ((i16::from(jam.octave) + 1) * 12).clamp(0, 115) as u8;
    let white_keys: [(char, u8); 8] = [
        ('z', 0),
        ('x', 2),
        ('c', 4),
        ('v', 5),
        ('b', 7),
        ('n', 9),
        ('m', 11),
        (',', 12),
    ];
    let black_keys: [(char, u8, f32); 5] = [
        ('s', 1, 0.68),
        ('d', 3, 1.68),
        ('g', 6, 3.68),
        ('h', 8, 4.68),
        ('j', 10, 5.68),
    ];
    let ww = (kb_area.w - 14.0) / 8.0;
    for (i, (key_ch, semi)) in white_keys.iter().enumerate() {
        let midi = base_midi.saturating_add(*semi);
        let held = jam.held_midi.contains(&midi);
        let kr = Rect::new(
            kb_area.x + i as f32 * (ww + 2.0),
            kb_area.y + 4.0,
            ww,
            kb_area.h - 6.0,
        );
        dl.panel(
            kr,
            if held {
                theme.accent.playhead.with_alpha(0.85)
            } else {
                theme.surface.elevated
            },
            if held {
                theme.accent.playhead
            } else {
                theme.border.hairline
            },
            theme.radii.badge,
            1.0,
            if held { 0.65 } else { 0.0 },
        );
        dl.text(
            Rect::new(kr.x, kr.bottom() - 20.0, kr.w, 18.0),
            key_ch.to_string(),
            theme.font.small,
            true,
            if held {
                theme.text.inverted
            } else {
                theme.text.secondary
            },
            TextAlign::Center,
        );
    }
    let bw = ww * 0.64;
    let bh = (kb_area.h - 6.0) * 0.56;
    for (key_ch, semi, pos) in black_keys {
        let midi = base_midi.saturating_add(semi);
        let held = jam.held_midi.contains(&midi);
        let bx = kb_area.x + pos * (ww + 2.0);
        let kr = Rect::new(bx, kb_area.y + 4.0, bw, bh);
        dl.panel(
            kr,
            if held {
                theme.accent.focus
            } else {
                theme.surface.canvas
            },
            if held {
                theme.accent.focus
            } else {
                theme.border.subtle
            },
            theme.radii.badge,
            1.0,
            if held { 0.70 } else { 0.0 },
        );
        dl.text(
            Rect::new(kr.x, kr.bottom() - 18.0, kr.w, 16.0),
            key_ch.to_string(),
            theme.font.small - 1.0,
            true,
            if held {
                theme.text.inverted
            } else {
                theme.text.muted
            },
            TextAlign::Center,
        );
    }
    dl.pop_clip();
}

/// Floating lane-layer HUD popup shown whenever `Alt+q..p` (or Jam `q..p`) opens a lane's part picker.
pub fn draw_lane_layer_hud(
    dl: &mut DisplayList,
    viewport: Rect,
    bottom_offset: f32,
    layer: &LaneLayerView,
    theme: &Theme,
) {
    let w = (viewport.w - 48.0).clamp(440.0, 880.0);
    let h = 82.0;
    let rect = Rect::new(
        (viewport.w - w) * 0.5,
        viewport.bottom() - bottom_offset - h - 10.0,
        w,
        h,
    );
    let lane_col = theme.track_color(layer.lane_index);
    dl.panel(
        rect,
        theme.surface.overlay.with_alpha(0.96),
        lane_col,
        theme.radii.panel,
        1.5,
        0.30,
    );
    let inner = rect.inset(12.0, 8.0);
    dl.push_clip(inner);
    let (hdr, rest) = inner.split_top(20.0);
    let (parts_row, hint_row) = rest.split_top(30.0);

    let toggle_desc = if layer.overriding {
        format!("{} release override", layer.key_char)
    } else {
        format!("{} launch last part", layer.key_char)
    };
    dl.text(
        hdr,
        format!(
            "LANE LAYER  ·  {} ({})  ·  1–9 launch live override",
            layer.lane_name, layer.key_char
        ),
        theme.font.small,
        true,
        lane_col,
        TextAlign::Left,
    );

    let cw = theme.char_width(theme.font.small);
    let mut px = parts_row.x;
    for (num, name, active, is_ov) in &layer.parts {
        let lbl = format!(
            "{num} {}{}",
            trim_label(name, 14),
            if *is_ov {
                " ▶ LIVE"
            } else if *active {
                " ▶"
            } else {
                ""
            }
        );
        let pw = lbl.chars().count() as f32 * cw + 18.0;
        if px + pw > parts_row.right() {
            break;
        }
        let r = Rect::new(px, parts_row.y + 3.0, pw, 24.0);
        let (bg, border, fg, glow) = if *is_ov {
            (
                theme.accent.override_bar.with_alpha(0.26),
                theme.accent.override_bar,
                theme.accent.override_bar,
                0.35,
            )
        } else if *active {
            (
                theme.accent.playhead.with_alpha(0.20),
                theme.accent.playhead,
                theme.accent.playhead,
                0.20,
            )
        } else {
            (
                theme.surface.recessed,
                theme.border.hairline,
                theme.text.primary,
                0.0,
            )
        };
        dl.panel(r, bg, border, theme.radii.badge, 1.0, glow);
        dl.text(
            r.inset(6.0, 0.0),
            lbl,
            theme.font.small,
            *is_ov || *active,
            fg,
            TextAlign::Left,
        );
        px += pw + 6.0;
    }

    dl.text(
        hint_row,
        format!("{toggle_desc}  ·  Alt+x release  ·  Alt+c keep into arrangement  ·  Esc close"),
        theme.font.small - 0.5,
        false,
        theme.text.muted,
        TextAlign::Left,
    );
    dl.pop_clip();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jam_hud_renders_tab_bank_only_when_multiple_banks_exist() {
        let theme = Theme::cadence_dark();
        let viewport = Rect::new(0.0, 0.0, 1440.0, 860.0);

        let single_bank = JamHudView {
            instrument_name: "Anthem Lead".into(),
            octave: 4,
            bank: 0,
            bank_count: 1,
            held_midi: vec![],
            sections: vec![('1', "Intro".into(), true, false)],
            lanes: vec![('q', "Lead".into(), true, false, false, false)],
        };
        let mut dl_single = DisplayList::new(1440.0, 860.0, 1.0);
        draw_jam_hud(&mut dl_single, viewport, 56.0, &single_bank, &theme);
        assert!(
            !dl_single
                .base
                .texts
                .iter()
                .any(|t| t.text.contains("Tab bank")),
            "Single-bank Jam HUD must not show Tab bank"
        );

        let multi_bank = JamHudView {
            bank: 1,
            bank_count: 2,
            ..single_bank
        };
        let mut dl_multi = DisplayList::new(1440.0, 860.0, 1.0);
        draw_jam_hud(&mut dl_multi, viewport, 56.0, &multi_bank, &theme);
        assert!(
            dl_multi
                .base
                .texts
                .iter()
                .any(|t| t.text.contains("Bank 2/2") && t.text.contains("Tab bank")),
            "Multi-bank Jam HUD must surface Bank indicator and Tab bank"
        );
    }

    #[test]
    fn draw_inspect_word_wraps_long_lines_preserves_indent_and_clamps_scroll() {
        let theme = Theme::cadence_dark();
        let rect = Rect::new(40.0, 40.0, 380.0, 260.0);
        let lines = vec![
            "{".to_string(),
            "  \"asset_path\": \"/studio/projects/afterglow/factory-samples/tr-808/vintage_kick_long_name.wav\",".to_string(),
            "  \"summary\": \"This long inspection description wraps across multiple lines without abrupt truncation on narrow panes.\"".to_string(),
            "}".to_string(),
        ];

        let mut dl = DisplayList::new(600.0, 400.0, 1.0);
        draw_inspect(
            &mut dl,
            rect,
            "ASSETS · paths / readiness",
            &lines,
            0,
            &theme,
        );
        assert_eq!(dl.current_clip(), dl.viewport);

        let body_lines: Vec<&str> = dl
            .base
            .texts
            .iter()
            .filter(|t| t.rect.y > rect.y + 30.0 && t.rect.y < rect.bottom() - 32.0)
            .map(|t| t.text.as_str())
            .collect();
        assert!(
            body_lines.len() > lines.len(),
            "long inspect lines must wrap into multiple visual rows, got: {body_lines:?}"
        );
        let joined = body_lines.join(" ");
        assert!(
            joined.contains("vintage_kick_long_name.wav")
                && joined.contains("narrow panes."),
            "wrapped inspect output must preserve trailing path/sentence tails, got: {body_lines:?}"
        );
        assert!(
            body_lines
                .iter()
                .any(|l| l.starts_with("  ") && l.contains("\"asset_path\"")),
            "wrapped JSON lines must preserve leading indentation, got: {body_lines:?}"
        );

        // Overscrolling clamps cleanly without panicking and keeps clip stack balanced.
        let mut dl_scrolled = DisplayList::new(600.0, 400.0, 1.0);
        draw_inspect(
            &mut dl_scrolled,
            rect,
            "INSPECT · definitions",
            &lines,
            999,
            &theme,
        );
        assert_eq!(dl_scrolled.current_clip(), dl_scrolled.viewport);
        assert!(dl_scrolled.base.texts.iter().any(|t| t.text == "}"));
    }

    #[test]
    fn lane_layer_hud_uses_plus_modifier_syntax() {
        let theme = Theme::cadence_dark();
        let viewport = Rect::new(0.0, 0.0, 1280.0, 760.0);
        let layer = LaneLayerView {
            lane_index: 0,
            lane_name: "Lead".into(),
            key_char: 'q',
            overriding: true,
            parts: vec![(1, "Lead A".into(), true, true)],
        };
        let mut dl = DisplayList::new(1280.0, 760.0, 1.0);
        draw_lane_layer_hud(&mut dl, viewport, 56.0, &layer, &theme);
        assert_eq!(dl.current_clip(), dl.viewport);
        assert!(dl
            .base
            .texts
            .iter()
            .any(|t| t.text.contains("Alt+x release") && t.text.contains("Alt+c keep")));
        assert!(!dl
            .base
            .texts
            .iter()
            .any(|t| t.text.contains("Alt-x") || t.text.contains("Alt-c")));
    }
}
