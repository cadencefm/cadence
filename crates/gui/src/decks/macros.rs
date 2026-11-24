//! GPU Assignable Project Macros Deck (`gk`):
//! 8-slot studio Macro console (`M1..M8`) with tactile 270° rotary arc gauges,
//! glowing live modulated ghost arcs when automated via `ModulationTarget::Macro`,
//! MIDI CC badges, and a bottom multi-target bindings matrix highlighting inverted `↓` ranges.
use crate::chrome::{draw_deck_frame, DeckFrameSpec};
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::{Color, Theme};
use cadence_app::view::{
    MacroBindingView, MacroSlotView, MacroTargetKind, MacrosDeckView, UiModeKind,
};
use std::f32::consts::PI;

const ARC_START_RAD: f32 = 0.75 * PI; // 135° (7:30 clock position)
const ARC_SWEEP_RAD: f32 = 1.50 * PI; // 270° clockwise sweep to 405° (4:30 clock position)
const ARC_SEGMENTS: usize = 32;

fn sample_arc(cx: f32, cy: f32, radius: f32, t0: f32, t1: f32, steps: usize) -> Vec<(f32, f32)> {
    let n = steps.max(2);
    let a0 = ARC_START_RAD + t0.clamp(0.0, 1.0) * ARC_SWEEP_RAD;
    let a1 = ARC_START_RAD + t1.clamp(0.0, 1.0) * ARC_SWEEP_RAD;
    (0..=n)
        .map(|i| {
            let f = i as f32 / n as f32;
            let ang = a0 + (a1 - a0) * f;
            (cx + radius * ang.cos(), cy + radius * ang.sin())
        })
        .collect()
}

fn arc_point(cx: f32, cy: f32, radius: f32, norm: f32) -> (f32, f32) {
    let ang = ARC_START_RAD + norm.clamp(0.0, 1.0) * ARC_SWEEP_RAD;
    (cx + radius * ang.cos(), cy + radius * ang.sin())
}

pub fn draw_macros(
    dl: &mut DisplayList,
    area: Rect,
    view: &MacrosDeckView,
    focused: bool,
    mode: UiModeKind,
    theme: &Theme,
) {
    let subtitle = format!(
        "{}/8 active  ·  {} binding{}{}",
        view.active_count,
        view.total_bindings,
        if view.total_bindings == 1 { "" } else { "s" },
        if view.recording { "  ·  ● REC" } else { "" }
    );
    let inner = draw_deck_frame(
        dl,
        area,
        &DeckFrameSpec {
            title: "PROJECT MACROS",
            right_badge: Some(&subtitle),
            accent: theme.accent.modulation,
            focused,
            mode,
        },
        theme,
    );
    if inner.w < 180.0 || inner.h < 100.0 {
        return;
    }
    dl.push_clip(inner);

    let body = inner;

    // Split top 1D horizontal Macro strip (M1–M8) and bottom scrollable Selected Macro Bindings panel.
    let console_h = if body.h >= 240.0 {
        (body.h * 0.44).clamp(148.0, 224.0)
    } else {
        body.h
    };
    let (console_rect, lower_raw) = body.split_top(console_h);

    let dial_focused = focused && view.selected_binding.is_none();
    draw_macro_console(dl, console_rect, view, dial_focused, theme);

    if lower_raw.h >= 72.0 {
        let bindings_rect = Rect::new(
            lower_raw.x,
            lower_raw.y + 8.0,
            lower_raw.w,
            (lower_raw.h - 8.0).max(0.0),
        );
        if let Some(selected) = view.slots.get(view.selected_index) {
            draw_bindings_panel(dl, bindings_rect, selected, theme);
        }
    }

    dl.pop_clip();
}

/// Compute horizontal `(first_slot, visible_slots)` window keeping `selected` in view
/// across a single horizontal row of `total` slots without wrapping into a 2D grid.
fn macro_strip_window(
    area_w: f32,
    total: usize,
    selected: usize,
    min_card_w: f32,
    gap: f32,
    gutter_w: f32,
) -> (usize, usize, bool) {
    if total == 0 {
        return (0, 0, false);
    }
    let all_fit_w = total as f32 * min_card_w + (total.saturating_sub(1)) as f32 * gap;
    if area_w >= all_fit_w {
        return (0, total, false);
    }
    let inner_w = (area_w - gutter_w * 2.0).max(min_card_w);
    let visible = (((inner_w + gap) / (min_card_w + gap)).floor() as usize).clamp(1, total);
    let sel = selected.min(total.saturating_sub(1));
    let first = sel
        .saturating_sub(visible / 2)
        .min(total.saturating_sub(visible));
    (first, visible, visible < total)
}

/// Compute vertical `(first_row, visible_rows)` window for the Bindings list keeping
/// `selected` centered/visible when the list exceeds the panel viewport.
fn bindings_scroll_window(total: usize, selected: Option<usize>, visible: usize) -> usize {
    if total <= visible {
        return 0;
    }
    match selected {
        Some(sel) => sel
            .min(total.saturating_sub(1))
            .saturating_sub(visible / 2)
            .min(total.saturating_sub(visible)),
        None => 0,
    }
}

fn draw_macro_console(
    dl: &mut DisplayList,
    area: Rect,
    view: &MacrosDeckView,
    dial_focused: bool,
    theme: &Theme,
) {
    const MIN_CARD_W: f32 = 100.0;
    const GUTTER_W: f32 = 18.0;
    let total = view.slots.len().min(8);
    if total == 0 {
        return;
    }
    let gap = if area.w >= 900.0 { 8.0 } else { 6.0 };
    let (first, visible, scrollable) = macro_strip_window(
        area.w,
        total,
        view.selected_index,
        MIN_CARD_W,
        gap,
        GUTTER_W,
    );
    let strip = if scrollable {
        Rect::new(
            area.x + GUTTER_W,
            area.y,
            (area.w - GUTTER_W * 2.0).max(40.0),
            area.h,
        )
    } else {
        area
    };

    if scrollable {
        let has_left = first > 0;
        let has_right = first + visible < total;
        let left_r = Rect::new(area.x, area.y, GUTTER_W - 2.0, area.h);
        let right_r = Rect::new(
            area.right() - GUTTER_W + 2.0,
            area.y,
            GUTTER_W - 2.0,
            area.h,
        );
        dl.text(
            left_r,
            "◀",
            theme.font.small,
            has_left,
            if has_left {
                theme.accent.focus
            } else {
                theme.text.muted.with_alpha(0.32)
            },
            TextAlign::Center,
        );
        dl.text(
            right_r,
            "▶",
            theme.font.small,
            has_right,
            if has_right {
                theme.accent.focus
            } else {
                theme.text.muted.with_alpha(0.32)
            },
            TextAlign::Center,
        );
    }

    let card_w = ((strip.w - gap * (visible.saturating_sub(1)) as f32) / visible as f32).max(40.0);
    let card_h = strip.h.max(52.0);

    for (vis_col, slot) in view
        .slots
        .iter()
        .take(total)
        .skip(first)
        .take(visible)
        .enumerate()
    {
        let rect = Rect::new(
            strip.x + vis_col as f32 * (card_w + gap),
            strip.y,
            card_w,
            card_h,
        );
        draw_macro_card(dl, rect, slot, slot.selected && dial_focused, theme);
    }
}

fn draw_macro_card(
    dl: &mut DisplayList,
    rect: Rect,
    slot: &MacroSlotView,
    dial_focused: bool,
    theme: &Theme,
) {
    let accent = theme.track_color(usize::from(slot.index));
    let automated = slot.effective_value.is_some();
    let card_bg = if slot.learning {
        theme.accent.warning_key.with_alpha(0.16)
    } else if slot.selected {
        accent.with_alpha(0.14)
    } else if slot.configured {
        theme.surface.recessed
    } else {
        theme.surface.recessed.with_alpha(0.62)
    };
    let border_col = if slot.learning {
        theme.accent.warning_key
    } else if dial_focused {
        theme.accent.focus
    } else if slot.selected {
        accent.with_alpha(0.70)
    } else if automated {
        theme.accent.modulation.with_alpha(0.70)
    } else if slot.configured {
        theme.border.hairline
    } else {
        theme.border.subtle
    };
    let border_w = if slot.selected || slot.learning {
        1.8
    } else {
        1.0
    };
    let glow = if slot.learning {
        0.34
    } else if dial_focused {
        0.26
    } else if slot.selected {
        0.14
    } else if automated {
        0.12
    } else {
        0.0
    };
    dl.panel(rect, card_bg, border_col, theme.radii.panel, border_w, glow);

    let inner = rect.inset(7.0, 6.0);
    if inner.w < 24.0 || inner.h < 34.0 {
        return;
    }

    // 1. Top row: M1..M8 index pill on left, LEARN / CC badge on right.
    let (top_row, rest) = inner.split_top(16.0);
    dl.text(
        top_row,
        format!("M{}", usize::from(slot.index) + 1),
        theme.font.small,
        true,
        if slot.configured || slot.selected {
            accent
        } else {
            theme.text.muted
        },
        TextAlign::Left,
    );
    let badge_cw = theme.char_width(theme.font.small - 1.5);
    let max_badge = ((top_row.w - 28.0) / badge_cw).max(3.0) as usize;
    if slot.learning {
        dl.text(
            top_row,
            trim_label("◉ LEARN", max_badge),
            theme.font.small - 1.5,
            true,
            theme.accent.warning_key,
            TextAlign::Right,
        );
    } else if let Some(cc) = &slot.midi_badge {
        dl.text(
            top_row,
            trim_label(cc, max_badge),
            theme.font.small - 1.5,
            false,
            theme.accent.warning_key,
            TextAlign::Right,
        );
    } else if automated {
        dl.text(
            top_row,
            "MOD",
            theme.font.small - 1.5,
            true,
            theme.accent.modulation,
            TextAlign::Right,
        );
    }

    // 2. Bottom text block: custom name + target count / mod badge.
    let footer_h = if rest.h >= 84.0 { 34.0 } else { 24.0 };
    let (dial_area, footer) = rest.split_bottom(footer_h);

    // 3. Center tactile 270° rotary arc gauge (falls back cleanly to horizontal bar if very short).
    let display_val = slot.effective_value.unwrap_or(slot.value);
    if dial_area.h >= 56.0 && dial_area.w >= 54.0 {
        let cx = dial_area.x + dial_area.w * 0.5;
        let cy = dial_area.y + dial_area.h * 0.52;
        let radius = ((dial_area.w.min(dial_area.h)) * 0.40).clamp(18.0, 44.0);
        let stroke = (radius * 0.14).clamp(3.2, 5.4);

        // Background track rail (270°).
        let track_pts = sample_arc(cx, cy, radius, 0.0, 1.0, ARC_SEGMENTS);
        dl.polyline(&track_pts, stroke, theme.border.hairline, 0.0, None);

        // Authored static arc.
        if slot.value > 0.004 {
            let val_steps = ((slot.value * ARC_SEGMENTS as f32).ceil() as usize).max(2);
            let val_pts = sample_arc(cx, cy, radius, 0.0, slot.value, val_steps);
            let static_col = if automated {
                accent.with_alpha(0.42)
            } else {
                accent
            };
            let static_glow = if automated { 0.08 } else { 0.30 };
            dl.polyline(&val_pts, stroke, static_col, static_glow, None);
        }

        // Static knob pointer tick on inner perimeter.
        let p_in = arc_point(cx, cy, (radius - stroke * 1.45).max(4.0), slot.value);
        let p_out = arc_point(cx, cy, radius + 0.5, slot.value);
        dl.polyline(
            &[p_in, p_out],
            2.0,
            if slot.configured {
                theme
                    .text
                    .primary
                    .with_alpha(if automated { 0.55 } else { 0.92 })
            } else {
                theme.text.muted
            },
            0.0,
            None,
        );

        // Glowing live modulated ghost arc + dot when automated via `ModulationTarget::Macro`!
        if let Some(eff) = slot.effective_value {
            if eff > 0.004 {
                let eff_steps = ((eff * ARC_SEGMENTS as f32).ceil() as usize).max(2);
                let eff_pts = sample_arc(cx, cy, radius + stroke * 0.72, 0.0, eff, eff_steps);
                dl.polyline(
                    &eff_pts,
                    (stroke * 0.62).max(2.2),
                    theme.accent.modulation,
                    0.52,
                    None,
                );
            }
            let (gx, gy) = arc_point(cx, cy, radius + stroke * 0.72, eff);
            dl.rounded_glow(
                Rect::new(gx - 3.5, gy - 3.5, 7.0, 7.0),
                theme.accent.modulation,
                3.5,
                0.70,
            );
        }

        // Centered live percentage readout inside the dial.
        let pct_h = 16.0;
        let pct_rect = Rect::new(
            dial_area.x,
            cy - pct_h * 0.5 - if automated { 3.0 } else { 0.0 },
            dial_area.w,
            pct_h,
        );
        dl.text(
            pct_rect,
            format!("{:.0}%", display_val * 100.0),
            if radius >= 30.0 {
                theme.font.body
            } else {
                theme.font.small
            },
            true,
            if automated {
                theme.accent.modulation
            } else if slot.configured {
                theme.text.primary
            } else {
                theme.text.muted
            },
            TextAlign::Center,
        );
        if automated {
            dl.text(
                Rect::new(dial_area.x, cy + 7.0, dial_area.w, 12.0),
                "MOD",
                theme.font.small - 2.0,
                true,
                theme.accent.modulation,
                TextAlign::Center,
            );
        }
    } else {
        // Ultra-compact horizontal bar fallback for tiny split heights.
        let bar_y = dial_area.y + (dial_area.h - 8.0).max(0.0) * 0.5;
        let rail = Rect::new(dial_area.x, bar_y, dial_area.w, 6.0);
        dl.rounded(rail, theme.meter.track_bg, 3.0);
        let fw = rail.w * slot.value.clamp(0.0, 1.0);
        if fw > 0.5 {
            dl.rounded_glow(Rect::new(rail.x, rail.y, fw, rail.h), accent, 3.0, 0.20);
        }
        if let Some(eff) = slot.effective_value {
            let gx = rail.x + rail.w * eff.clamp(0.0, 1.0);
            dl.rounded_glow(
                Rect::new(gx - 2.5, rail.y - 2.0, 5.0, rail.h + 4.0),
                theme.accent.modulation,
                2.5,
                0.55,
            );
        }
    }

    // 4. Footer: Custom Macro Name + binding/mod summary.
    let name_cw = theme.char_width(theme.font.small);
    let max_name = (footer.w / name_cw).max(3.0) as usize;
    let (name_row, sub_row) = footer.split_top(17.0);
    dl.text(
        name_row,
        trim_label(&slot.name, max_name),
        theme.font.small,
        slot.configured || slot.selected,
        if slot.configured || slot.selected {
            theme.text.primary
        } else {
            theme.text.muted
        },
        TextAlign::Center,
    );
    if sub_row.h >= 12.0 {
        let sub_text = if let Some(pat) = &slot.mod_pattern_name {
            format!("∾ {} · {}t", pat, slot.bindings.len())
        } else if !slot.bindings.is_empty() {
            format!(
                "{} target{}",
                slot.bindings.len(),
                if slot.bindings.len() == 1 { "" } else { "s" }
            )
        } else if slot.configured {
            "0 targets".into()
        } else {
            "empty".into()
        };
        let sub_cw = theme.char_width(theme.font.small - 1.5);
        let max_sub = (sub_row.w / sub_cw).max(4.0) as usize;
        dl.text(
            sub_row,
            trim_label(&sub_text, max_sub),
            theme.font.small - 1.5,
            automated,
            if automated {
                theme.accent.modulation
            } else if !slot.bindings.is_empty() {
                theme.text.secondary
            } else {
                theme.text.muted
            },
            TextAlign::Center,
        );
    }
}

fn target_kind_color(kind: MacroTargetKind, theme: &Theme) -> Color {
    match kind {
        MacroTargetKind::Rack | MacroTargetKind::Module => theme.accent.routing,
        MacroTargetKind::Cable => theme.accent.jam,
        MacroTargetKind::Zone => theme.accent.focus,
        MacroTargetKind::Effect => theme.accent.modulation,
        MacroTargetKind::Channel => theme.accent.playhead,
        MacroTargetKind::Send => theme.accent.warning_key,
    }
}

fn draw_bindings_panel(dl: &mut DisplayList, area: Rect, slot: &MacroSlotView, theme: &Theme) {
    let accent = theme.track_color(usize::from(slot.index));
    dl.panel(
        area,
        theme.surface.recessed,
        theme.border.subtle,
        theme.radii.panel,
        1.0,
        0.0,
    );
    let inner = area.inset(10.0, 7.0);
    if inner.w < 120.0 || inner.h < 36.0 {
        return;
    }

    // Header row inside bindings matrix.
    let (hdr, list_area) = inner.split_top(22.0);
    let hdr_left = format!(
        "M{}  ·  {}   ({} BINDING{})",
        usize::from(slot.index) + 1,
        slot.name.to_uppercase(),
        slot.bindings.len(),
        if slot.bindings.len() == 1 { "" } else { "S" }
    );
    let (hdr_l, hdr_r) = hdr.split_left(hdr.w * 0.62);
    let cw = theme.char_width(theme.font.small);
    dl.text(
        hdr_l,
        trim_label(&hdr_left, (hdr_l.w / cw).max(8.0) as usize),
        theme.font.small,
        true,
        accent,
        TextAlign::Left,
    );

    let right_info =
        if let Some((eff, pat)) = slot.effective_value.zip(slot.mod_pattern_name.as_deref()) {
            format!(
                "MOD: {}  ·  LIVE {:.0}% (BASE {:.0}%)",
                pat,
                eff * 100.0,
                slot.value * 100.0
            )
        } else {
            format!("STATIC VALUE  ·  {:.0}%", slot.value * 100.0)
        };
    dl.text(
        hdr_r,
        trim_label(&right_info, (hdr_r.w / cw).max(8.0) as usize),
        theme.font.small - 0.5,
        slot.effective_value.is_some(),
        if slot.effective_value.is_some() {
            theme.accent.modulation
        } else {
            theme.text.secondary
        },
        TextAlign::Right,
    );

    // Subtle hairline separator under header.
    dl.fill(
        Rect::new(inner.x, hdr.bottom() + 1.0, inner.w, 1.0),
        theme.border.subtle,
    );

    let rows_rect = Rect::new(
        list_area.x,
        list_area.y + 5.0,
        list_area.w,
        (list_area.h - 5.0).max(0.0),
    );
    if slot.bindings.is_empty() {
        let idx1 = usize::from(slot.index) + 1;
        dl.text(
            rows_rect,
            format!(
                "No parameter bindings on M{idx1} · press a to bind a parameter · r to rename · e for modulation"
            ),
            theme.font.small,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
        return;
    }

    let row_h = 26.0;
    let total = slot.bindings.len();
    let max_rows_full = (rows_rect.h / row_h).floor().max(1.0) as usize;
    let needs_scroll = total > max_rows_full;
    let (list_rect, footer_rect) = if needs_scroll && rows_rect.h >= row_h + 16.0 {
        let (top, bot) = rows_rect.split_bottom(15.0);
        (top, Some(bot))
    } else {
        (rows_rect, None)
    };
    let visible = (list_rect.h / row_h).floor().max(1.0) as usize;
    let selected_idx = slot.bindings.iter().position(|b| b.selected);
    let first = bindings_scroll_window(total, selected_idx, visible);
    let automated = slot.effective_value.is_some();

    for (vis_i, b) in slot.bindings.iter().skip(first).take(visible).enumerate() {
        let r = Rect::new(
            list_rect.x,
            list_rect.y + vis_i as f32 * row_h,
            list_rect.w,
            row_h - 3.0,
        );
        draw_binding_row(
            dl,
            r,
            b,
            BindingRowStyle {
                vis_index: first + vis_i,
                accent,
                automated,
            },
            theme,
        );
    }

    if let Some(foot) = footer_rect {
        let shown_end = (first + visible).min(total);
        let up = if first > 0 { "▲ " } else { "" };
        let dn = if shown_end < total { " ▼" } else { "" };
        dl.text(
            foot,
            format!("{up}showing #{}–#{shown_end} of {total}{dn}", first + 1),
            theme.font.small - 1.5,
            false,
            theme.text.muted,
            TextAlign::Right,
        );
    }
}

#[derive(Clone, Copy)]
struct BindingRowStyle {
    vis_index: usize,
    accent: Color,
    automated: bool,
}

fn draw_binding_row(
    dl: &mut DisplayList,
    r: Rect,
    b: &MacroBindingView,
    style: BindingRowStyle,
    theme: &Theme,
) {
    let BindingRowStyle {
        vis_index: vis_i,
        accent,
        automated,
    } = style;
    if b.selected {
        dl.panel(
            r,
            theme.surface.selection,
            theme.accent.focus,
            theme.radii.badge,
            1.4,
            theme.glow.focus_border,
        );
    } else if vis_i % 2 == 1 {
        dl.rounded(
            r,
            theme.surface.elevated.with_alpha(0.50),
            theme.radii.badge,
        );
    } else {
        dl.rounded(
            r,
            theme.surface.elevated.with_alpha(0.24),
            theme.radii.badge,
        );
    }

    let cw = theme.char_width(theme.font.small);
    let idx_w = 30.0;
    let kind_w = 64.0;
    let val_w = 88.0;
    let range_w = if r.w >= 720.0 { 196.0 } else { 164.0 };
    let bar_w = if r.w >= 640.0 {
        (r.w * 0.20).clamp(110.0, 210.0)
    } else {
        0.0
    };
    let name_x = r.x + idx_w + kind_w + 14.0;
    let right_block_w = range_w + bar_w + val_w + if bar_w > 0.0 { 28.0 } else { 18.0 };
    let name_w = (r.right() - name_x - right_block_w).max(60.0);

    // 1. 1-based binding index (`#1`).
    dl.text(
        Rect::new(r.x + 6.0, r.y, idx_w, r.h),
        format!("#{}", b.index + 1),
        theme.font.small - 0.5,
        false,
        theme.text.muted,
        TextAlign::Left,
    );

    // 2. Kind badge pill (`RACK`, `MOD`, `CABLE`, `ZONE`, `FX`, `CH`, `SEND`).
    let kcol = target_kind_color(b.target_kind, theme);
    let krect = Rect::new(r.x + idx_w + 4.0, r.y + 3.0, kind_w, r.h - 6.0);
    dl.panel(
        krect,
        kcol.with_alpha(0.16),
        kcol.with_alpha(0.65),
        theme.radii.badge,
        1.0,
        0.0,
    );
    dl.text(
        krect,
        b.target_kind.badge(),
        theme.font.small - 1.5,
        true,
        kcol,
        TextAlign::Center,
    );

    // 3. Target parameter label (`Saw · Cutoff`).
    dl.text(
        Rect::new(name_x, r.y, name_w, r.h),
        trim_label(&b.target_name, (name_w / cw).max(6.0) as usize),
        theme.font.small,
        true,
        theme.text.primary,
        TextAlign::Left,
    );

    // 4. Native-unit range readout (`45.82 Hz → 14.16 kHz` or inverted `R84 ↓ L84  INV`).
    let mut rx = r.right() - right_block_w + 4.0;
    let range_text = if b.inverted {
        format!("{} ↓ {}  INV", b.min_display, b.max_display)
    } else {
        format!("{} → {}", b.min_display, b.max_display)
    };
    let range_font = theme.font.small - 0.5;
    let range_cw = theme.char_width(range_font);
    let range_col = if b.inverted {
        theme.accent.warning_key
    } else {
        theme.accent.focus
    };
    if b.inverted {
        let pill_w = (range_text.chars().count() as f32 * range_cw + 16.0).clamp(112.0, range_w);
        let inv_pill = Rect::new(rx + range_w - pill_w, r.y + 2.5, pill_w, r.h - 5.0);
        let max_chars = ((pill_w - 8.0) / range_cw).max(6.0) as usize;
        dl.panel(
            inv_pill,
            theme.accent.warning_key.with_alpha(0.14),
            theme.accent.warning_key.with_alpha(0.75),
            theme.radii.badge,
            1.0,
            0.12,
        );
        dl.text(
            inv_pill.inset(4.0, 0.0),
            trim_label(&range_text, max_chars),
            range_font,
            true,
            range_col,
            TextAlign::Center,
        );
    } else {
        let max_chars = (range_w / range_cw).max(6.0) as usize;
        dl.text(
            Rect::new(rx, r.y, range_w, r.h),
            trim_label(&range_text, max_chars),
            range_font,
            false,
            range_col,
            TextAlign::Right,
        );
    }
    rx += range_w + 10.0;

    // 5. Mini horizontal range sweep rail showing `[min..max]` span + live evaluated tick.
    if bar_w > 0.0 {
        let track = Rect::new(rx, r.y + r.h * 0.5 - 3.5, bar_w, 7.0);
        dl.rounded(track, theme.meter.track_bg, 3.5);
        let lo = b.min_norm.min(b.max_norm).clamp(0.0, 1.0);
        let hi = b.min_norm.max(b.max_norm).clamp(0.0, 1.0);
        let sx = track.x + track.w * lo;
        let sw = (track.w * (hi - lo)).max(2.0);
        let span_col = if b.inverted {
            theme.accent.warning_key.with_alpha(0.42)
        } else {
            accent.with_alpha(0.38)
        };
        dl.rounded(Rect::new(sx, track.y, sw, track.h), span_col, 3.5);

        let cur_x = track.x + track.w * b.current_norm.clamp(0.0, 1.0);
        let dot_col = if automated {
            theme.accent.modulation
        } else if b.inverted {
            theme.accent.warning_key
        } else {
            accent
        };
        dl.rounded_glow(
            Rect::new(cur_x - 3.5, track.y - 2.0, 7.0, track.h + 4.0),
            dot_col,
            3.5,
            if automated { 0.55 } else { 0.30 },
        );
        rx += bar_w + 8.0;
    }

    // 6. Right-aligned live evaluated native value readout (`2.45 kHz`, `L45`, `72%`).
    dl.text(
        Rect::new(rx, r.y, (r.right() - rx - 6.0).max(32.0), r.h),
        trim_label(&b.current_display, (val_w / cw).max(5.0) as usize),
        theme.font.small,
        true,
        if automated {
            theme.accent.modulation
        } else {
            theme.text.primary
        },
        TextAlign::Right,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_kind_color_maps_all_binding_kinds() {
        for theme in [
            Theme::cadence_dark(),
            Theme::midnight_neon(),
            Theme::studio_light(),
        ] {
            assert_eq!(
                target_kind_color(MacroTargetKind::Rack, &theme),
                theme.accent.routing
            );
            assert_eq!(
                target_kind_color(MacroTargetKind::Module, &theme),
                theme.accent.routing
            );
            assert_eq!(
                target_kind_color(MacroTargetKind::Cable, &theme),
                theme.accent.jam
            );
            assert_eq!(
                target_kind_color(MacroTargetKind::Zone, &theme),
                theme.accent.focus
            );
            assert_eq!(
                target_kind_color(MacroTargetKind::Effect, &theme),
                theme.accent.modulation
            );
            assert_eq!(
                target_kind_color(MacroTargetKind::Channel, &theme),
                theme.accent.playhead
            );
            assert_eq!(
                target_kind_color(MacroTargetKind::Send, &theme),
                theme.accent.warning_key
            );
            // Every MacroTargetKind maps to a semantic accent color and non-empty badge.
            for kind in MacroTargetKind::ALL {
                assert!(!kind.badge().is_empty());
                assert_ne!(
                    target_kind_color(kind, &theme),
                    theme.text.secondary,
                    "{kind:?} must map to a semantic accent color in {}",
                    theme.name
                );
            }
        }
    }

    fn sample_macros_view(selected_slot: usize, selected_binding: Option<usize>) -> MacrosDeckView {
        let slots = (0..8u8)
            .map(|i| {
                let bindings = if usize::from(i) == selected_slot {
                    (0..12)
                        .map(|b| MacroBindingView {
                            index: b,
                            target_kind: MacroTargetKind::Module,
                            target_name: format!("TargetParam{b:02}"),
                            min_norm: 0.1,
                            max_norm: 0.9,
                            inverted: false,
                            current_norm: 0.5,
                            current_display: "50%".into(),
                            min_display: "10%".into(),
                            max_display: "90%".into(),
                            selected: selected_binding == Some(b),
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                MacroSlotView {
                    index: i,
                    configured: true,
                    name: format!("Slot{}", i + 1),
                    value: 0.5,
                    effective_value: None,
                    mod_pattern_name: None,
                    midi_badge: None,
                    learning: false,
                    selected: usize::from(i) == selected_slot,
                    bindings,
                }
            })
            .collect();
        MacrosDeckView {
            slots,
            selected_index: selected_slot,
            selected_binding,
            active_count: 8,
            total_bindings: 12,
            recording: false,
        }
    }

    #[test]
    fn narrow_macros_deck_stays_single_horizontal_row_and_scrolls_to_selected_slot() {
        let theme = Theme::cadence_dark();
        let view = sample_macros_view(6, None); // M7 selected
        let mut dl = DisplayList::new(560.0, 420.0, 1.0);
        draw_macros(
            &mut dl,
            Rect::new(0.0, 0.0, 560.0, 420.0),
            &view,
            true,
            UiModeKind::Normal,
            &theme,
        );
        // Visible M-slot labels must all share the exact same top Y coordinate (single 1D row!).
        let m_labels: Vec<_> = dl
            .base
            .texts
            .iter()
            .filter(|t| {
                t.text.starts_with('M')
                    && t.text.len() == 2
                    && t.text[1..].chars().all(|c| c.is_ascii_digit())
            })
            .collect();
        assert!(
            m_labels.len() >= 3 && m_labels.len() < 8,
            "expected horizontally scrolled subset of 8 slots at 560px, got {:?}",
            m_labels.iter().map(|t| &t.text).collect::<Vec<_>>()
        );
        assert!(
            m_labels.iter().any(|t| t.text == "M7"),
            "selected slot M7 must be scrolled into view"
        );
        let y0 = m_labels[0].rect.y;
        for lbl in &m_labels {
            assert!(
                (lbl.rect.y - y0).abs() < 0.5,
                "slot {} wrapped vertically (y={} vs y0={y0}); Macros strip must remain 1D horizontal",
                lbl.text,
                lbl.rect.y
            );
        }
        assert!(
            dl.base.texts.iter().any(|t| t.text == "◀")
                && dl.base.texts.iter().any(|t| t.text == "▶"),
            "horizontal scroll indicators must render when < 8 slots fit"
        );
    }

    #[test]
    fn bindings_panel_scrolls_vertically_to_keep_selected_binding_visible() {
        let theme = Theme::cadence_dark();
        let view = sample_macros_view(0, Some(9)); // binding #10 selected out of 12
        let mut dl = DisplayList::new(980.0, 360.0, 1.0);
        draw_macros(
            &mut dl,
            Rect::new(0.0, 0.0, 980.0, 360.0),
            &view,
            true,
            UiModeKind::Normal,
            &theme,
        );
        let texts: Vec<&str> = dl.base.texts.iter().map(|t| t.text.as_str()).collect();
        assert!(
            texts.iter().any(|t| t.contains("TargetParam09")),
            "selected binding #10 (TargetParam09) must be scrolled into view: {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains("TargetParam00")),
            "binding #1 (TargetParam00) should have scrolled above viewport: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("▲") && t.contains("of 12")),
            "vertical scroll status footer must render when bindings exceed viewport: {texts:?}"
        );
    }
}
