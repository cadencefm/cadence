//! GPU Unified Eurorack Modular Instrument Deck (`Rack`):
//! - Header + mode tabs (`Panel | Patchbay | Zones`), active zone badge, channel output badge,
//!   and contextual shortcut hints.
//! - `Panel`: scrollable horizontal module pill strip (`Quick` pins + zone modules) + dedicated
//!   live GPU visualizers for all 13 `ModuleVisualView` variants (`PinnedOverview`, `Vco`,
//!   `Sampler`, `Noise`, `Env`, `Lfo`, `Filter`, `Vca`, `Drive`, `LoFi`, `Resonator`, `Mobius`, `Out`) + control list.
//! - `Patchbay`: multi-column Eurorack node-and-cable graph delegated strictly to
//!   [`crate::decks::cable_graph`] (SPOT) with semantic cable colors, inline attenuverter pills,
//!   ASCII `"z-1"` dashed feedback cables, selected-cable halo, and footer summary + live scope.
//! - `Zones`: scrollable horizontal Zone pill strip + 2D Key × Velocity map with 1:1 chromatic
//!   piano strip + live dry oscilloscope + zone/voice/rack control list.
//!
//! Zero duplicated DSP math (all curves pre-sampled via `cadence_core::dsp::*` in `cadence-app`),
//! zero hardcoded RGBA outside `theme.rs`, and strictly balanced `push_clip`/`pop_clip`.
mod panel;
mod patchbay;
mod visuals;
mod zones;

#[cfg(test)]
mod tests;

use crate::chrome::{draw_deck_frame, DeckFrameSpec};
use crate::decks::object::draw_control_list;
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::{Color, Theme};
use cadence_app::view::{
    InstrumentDeckView, InstrumentVisualView, RackDeckView, RackTab, UiModeKind,
};
use cadence_core::rack::ModuleKindTag;

pub fn draw_instrument(
    dl: &mut DisplayList,
    area: Rect,
    view: &InstrumentDeckView,
    focused: bool,
    mode: UiModeKind,
    theme: &Theme,
) {
    let badge = if view.channel_name.is_empty() {
        Some("P presets · r rename".to_string())
    } else if area.w >= 560.0 {
        Some(format!("P presets · r rename  ·  → {}", view.channel_name))
    } else {
        Some(format!("P presets · → {}", view.channel_name))
    };
    let title = format!("RACK  ·  {}", view.name);
    let inner = draw_deck_frame(
        dl,
        area,
        &DeckFrameSpec {
            title: &title,
            right_badge: badge.as_deref(),
            accent: theme.accent.focus,
            focused,
            mode,
        },
        theme,
    );
    if inner.w < 160.0 || inner.h < 90.0 {
        return;
    }
    dl.push_clip(inner);

    match &view.visual {
        InstrumentVisualView::Rack(rack) => {
            draw_rack_deck(dl, inner, view, rack, theme);
        }
        InstrumentVisualView::Missing => {
            draw_control_list(dl, inner, &view.controls, view.control_cursor, theme);
        }
    }

    dl.pop_clip();
}

fn draw_rack_deck(
    dl: &mut DisplayList,
    area: Rect,
    view: &InstrumentDeckView,
    rack: &RackDeckView,
    theme: &Theme,
) {
    // Top bar: Mode Tabs (`Panel | Patchbay | Zones`) + active zone badge (when multi-zone) + hints.
    let (tab_bar, body) = area.split_top(26.0);
    draw_rack_mode_bar(dl, tab_bar, rack, theme);

    if body.h < 56.0 {
        return;
    }

    match rack.active_tab {
        RackTab::Panel => panel::draw_panel_tab(dl, body, view, rack, theme),
        RackTab::Patchbay => patchbay::draw_patchbay_tab(dl, body, &rack.patchbay, theme),
        RackTab::Zones => zones::draw_zones_tab(dl, body, view, rack, theme),
    }
}

fn draw_rack_mode_bar(dl: &mut DisplayList, bar: Rect, rack: &RackDeckView, theme: &Theme) {
    if bar.is_empty() {
        return;
    }
    dl.push_clip(bar);
    let font = theme.font.small;
    let cw = theme.char_width(font);
    let pill_h = (bar.h - 6.0).clamp(16.0, 20.0);
    let pill_y = bar.y + 1.0;

    let mut tx = bar.x;
    for &(label, active) in &rack.tabs {
        let tw = label.chars().count() as f32 * cw + 18.0;
        let tr = Rect::new(tx, pill_y, tw, pill_h);
        dl.panel(
            tr,
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
            tr,
            label,
            font,
            active,
            if active {
                theme.accent.focus
            } else {
                theme.text.secondary
            },
            TextAlign::Center,
        );
        tx += tw + 5.0;
    }

    // Active zone badge on Panel & Patchbay whenever the Rack has multiple zones
    // (Zones tab already renders the full horizontal Zone pill strip right below this bar).
    if rack.active_tab != RackTab::Zones && rack.zone_count > 1 && !rack.active_zone_name.is_empty()
    {
        let ztext = format!("ZONE: {} ({}z)", rack.active_zone_name, rack.zone_count);
        let zfont = theme.font.small - 1.0;
        let zcw = theme.char_width(zfont);
        let avail = (bar.right() - tx - 8.0).max(0.0);
        let zw = (ztext.chars().count() as f32 * zcw + 16.0).min(avail.min(220.0));
        if zw >= 56.0 {
            tx += 4.0;
            let zr = Rect::new(tx, pill_y, zw, pill_h);
            dl.panel(
                zr,
                theme.accent.modulation.with_alpha(0.16),
                theme.accent.modulation.with_alpha(0.75),
                theme.radii.badge,
                1.0,
                0.0,
            );
            let max_c = ((zr.w - 10.0) / zcw).floor().max(4.0) as usize;
            dl.text(
                zr.inset(5.0, 0.0),
                trim_label(&ztext, max_c),
                zfont,
                true,
                theme.accent.modulation,
                TextAlign::Center,
            );
            tx += zw + 6.0;
        }
    }

    // Crisp right-aligned contextual shortcut hint strip when horizontal space allows.
    let hint = match rack.active_tab {
        RackTab::Panel => "H/L tab · h/l mod · Alt+h/l ,/. adjust · a add · x del · p pin",
        RackTab::Patchbay => "H/L tab · hjkl move · w/b cable · Alt+h/l ,/. amt · c patch · x cut",
        RackTab::Zones => "H/L tab · h/l zone · j/k ctrl · Alt+h/l ,/. adjust · a/d/x zone",
    };
    let rem_w = (bar.right() - tx - 8.0).max(0.0);
    if rem_w >= 110.0 {
        let hfont = theme.font.small - 1.0;
        let hcw = theme.char_width(hfont);
        let max_c = (rem_w / hcw).floor().max(6.0) as usize;
        dl.text(
            Rect::new(tx + 6.0, pill_y, rem_w - 6.0, pill_h),
            trim_label(hint, max_c),
            hfont,
            false,
            theme.text.muted,
            TextAlign::Right,
        );
    }

    // Subtle hairline separating top mode bar from tab content.
    dl.fill(
        Rect::new(bar.x, bar.bottom() - 2.0, bar.w, 1.0),
        theme.border.subtle.with_alpha(0.7),
    );
    dl.pop_clip();
}

// -----------------------------------------------------------------------------
// Shared Pure-Theme Semantic Colors & Card / Scope Painters
// -----------------------------------------------------------------------------

pub(super) fn module_tag_accent(tag: ModuleKindTag, theme: &Theme) -> Color {
    match tag {
        ModuleKindTag::Vco => theme.accent.routing,
        ModuleKindTag::Sampler => theme.accent.focus,
        ModuleKindTag::Noise => theme.accent.routing.mix(theme.text.secondary, 0.35),
        ModuleKindTag::Env => theme.accent.playhead,
        ModuleKindTag::Lfo => theme.accent.modulation,
        ModuleKindTag::Filter => theme.accent.warning_key,
        ModuleKindTag::Vca => theme.accent.playhead.mix(theme.accent.focus, 0.50),
        ModuleKindTag::Drive => theme.accent.alert,
        ModuleKindTag::LoFi => theme.accent.warning_key.mix(theme.accent.alert, 0.45),
        ModuleKindTag::Resonator => theme.accent.routing.mix(theme.accent.playhead, 0.48),
        ModuleKindTag::Mobius => theme.accent.modulation.mix(theme.accent.focus, 0.45),
        ModuleKindTag::Out => theme.accent.focus,
    }
}

pub(super) fn split_main_and_scope(area: Rect) -> (Rect, Option<Rect>) {
    if area.w >= 440.0 {
        let scope_w = (area.w * 0.28).clamp(140.0, 250.0);
        let (left, right) = area.split_right(scope_w);
        (
            Rect::new(left.x, left.y, (left.w - 6.0).max(0.0), left.h),
            Some(right),
        )
    } else {
        (area, None)
    }
}

pub(super) fn draw_card_panel(dl: &mut DisplayList, rect: Rect, theme: &Theme) -> Rect {
    dl.panel(
        rect,
        theme.surface.recessed,
        theme.border.subtle,
        theme.radii.pill,
        1.0,
        0.0,
    );
    rect.inset(7.0, 5.0)
}

pub(super) fn draw_simple_card_header(
    dl: &mut DisplayList,
    hdr: Rect,
    title: &str,
    right: Option<&str>,
    color: Color,
    theme: &Theme,
) {
    if hdr.is_empty() {
        return;
    }
    dl.push_clip(hdr);
    let font = theme.font.small - 1.0;
    let cw = theme.char_width(font);
    let right_w = right
        .filter(|s| !s.is_empty())
        .map(|s| (s.chars().count() as f32 * cw + 6.0).min(hdr.w * 0.45))
        .unwrap_or(0.0);
    let title_w = (hdr.w - right_w - if right_w > 0.0 { 6.0 } else { 0.0 }).max(0.0);
    let max_t = (title_w / cw).floor().max(1.0) as usize;
    dl.text(
        Rect::new(hdr.x, hdr.y, title_w, hdr.h),
        trim_label(title, max_t),
        font,
        true,
        color,
        TextAlign::Left,
    );
    if let Some(rtxt) = right.filter(|s| !s.is_empty()) {
        if right_w >= 28.0 {
            let max_r = (right_w / cw).floor().max(2.0) as usize;
            dl.text(
                Rect::new(hdr.right() - right_w, hdr.y, right_w, hdr.h),
                trim_label(rtxt, max_r),
                font,
                true,
                theme.text.secondary,
                TextAlign::Right,
            );
        }
    }
    dl.pop_clip();
}

pub(super) fn draw_badge_header(
    dl: &mut DisplayList,
    hdr: Rect,
    badges: &[(&str, Color)],
    title: &str,
    right_summary: Option<&str>,
    title_color: Color,
    theme: &Theme,
) {
    if hdr.is_empty() {
        return;
    }
    dl.push_clip(hdr);
    let bfont = theme.font.small - 1.5;
    let bcw = theme.char_width(bfont);
    let bh = (hdr.h - 2.0).clamp(12.0, 15.0);
    let by = hdr.y + ((hdr.h - bh) * 0.5).max(0.0);

    let mut x = hdr.x;
    for &(text, col) in badges {
        if text.is_empty() {
            continue;
        }
        let bw = (text.chars().count() as f32 * bcw + 10.0).min((hdr.right() - x).max(0.0));
        if bw < 20.0 || x + bw > hdr.right() - 28.0 {
            break;
        }
        let br = Rect::new(x, by, bw, bh);
        dl.panel(
            br,
            col.with_alpha(0.18),
            col.with_alpha(0.80),
            theme.radii.badge,
            1.0,
            0.0,
        );
        dl.text(br, text, bfont, true, col, TextAlign::Center);
        x += bw + 5.0;
    }

    let font = theme.font.small - 1.0;
    let cw = theme.char_width(font);
    let rem = (hdr.right() - x).max(0.0);
    let right_w = right_summary
        .filter(|s| !s.is_empty() && rem >= 100.0)
        .map(|s| (s.chars().count() as f32 * cw + 6.0).min(rem * 0.45))
        .unwrap_or(0.0);
    if let Some(rs) = right_summary.filter(|_| right_w >= 32.0) {
        let max_r = (right_w / cw).floor().max(4.0) as usize;
        dl.text(
            Rect::new(hdr.right() - right_w, hdr.y, right_w, hdr.h),
            trim_label(rs, max_r),
            font,
            false,
            theme.text.secondary,
            TextAlign::Right,
        );
    }

    let title_w = (hdr.right() - right_w - x - if right_w > 0.0 { 6.0 } else { 0.0 }).max(0.0);
    if title_w >= 24.0 {
        let max_t = (title_w / cw).floor().max(1.0) as usize;
        dl.text(
            Rect::new(x, hdr.y, title_w, hdr.h),
            trim_label(title, max_t),
            font,
            true,
            title_color,
            TextAlign::Left,
        );
    }
    dl.pop_clip();
}

struct ScopePlotStyle<'a> {
    stroke: f32,
    glow: f32,
    fill_alpha: f32,
    empty_label: &'a str,
    empty_font: f32,
}

fn draw_scope_plot_body(
    dl: &mut DisplayList,
    plot: Rect,
    waveform: &[f32],
    style: ScopePlotStyle<'_>,
    theme: &Theme,
) {
    if plot.is_empty() {
        return;
    }
    dl.push_clip(plot);
    let mid_y = plot.y + plot.h * 0.5;
    dl.fill(
        Rect::new(plot.x, mid_y, plot.w, 1.0),
        theme.border.grid_step,
    );
    if waveform.len() >= 2 {
        let n = waveform.len();
        let half_h = (plot.h * 0.44).max(1.0);
        let pts: Vec<(f32, f32)> = waveform
            .iter()
            .enumerate()
            .map(|(i, &s)| {
                let x = plot.x + plot.w * (i as f32 / (n - 1) as f32);
                let y = mid_y - s.clamp(-1.0, 1.0) * half_h;
                (x, y)
            })
            .collect();
        dl.polyline(
            &pts,
            style.stroke,
            theme.accent.focus,
            style.glow,
            Some((mid_y, style.fill_alpha)),
        );
    } else {
        dl.text(
            plot,
            style.empty_label,
            style.empty_font,
            false,
            theme.text.muted,
            TextAlign::Center,
        );
    }
    dl.pop_clip();
}

pub(super) fn draw_live_scope_card(
    dl: &mut DisplayList,
    rect: Rect,
    label: &str,
    right_badge: Option<&str>,
    waveform: &[f32],
    theme: &Theme,
) {
    let inner = draw_card_panel(dl, rect, theme);
    let (hdr, plot) = inner.split_top(16.0);
    draw_simple_card_header(dl, hdr, label, right_badge, theme.accent.focus, theme);
    draw_scope_plot_body(
        dl,
        plot,
        waveform,
        ScopePlotStyle {
            stroke: 1.6,
            glow: theme.glow.scope,
            fill_alpha: 0.14,
            empty_label: "silent",
            empty_font: theme.font.small - 1.0,
        },
        theme,
    );
}

pub(super) fn draw_compact_scope_strip(
    dl: &mut DisplayList,
    rect: Rect,
    waveform: &[f32],
    theme: &Theme,
) {
    dl.panel(
        rect,
        theme.surface.recessed,
        theme.border.subtle,
        theme.radii.pill,
        1.0,
        0.0,
    );
    draw_scope_plot_body(
        dl,
        rect.inset(6.0, 4.0),
        waveform,
        ScopePlotStyle {
            stroke: 1.45,
            glow: theme.glow.scope * 0.75,
            fill_alpha: 0.12,
            empty_label: "DRY SCOPE",
            empty_font: theme.font.small - 2.0,
        },
        theme,
    );
}
