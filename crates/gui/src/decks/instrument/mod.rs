//! GPU Unified Eurorack Modular Instrument Deck (`Rack`):
//! - Header + mode tabs (`Panel | Patchbay | Zones`), active zone badge, channel output badge,
//!   and contextual shortcut hints.
//! - `Panel`: scrollable horizontal module pill strip (`Quick` pins + zone modules) + dedicated
//!   live GPU visualizers for all 12 `ModuleVisualView` variants (`PinnedOverview`, `Vco`,
//!   `Sampler`, `Noise`, `Env`, `Lfo`, `Filter`, `Vca`, `Drive`, `LoFi`, `Resonator`, `Out`) + control list.
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

use crate::chrome::{draw_deck_frame, draw_deck_tab_bar, DeckFrameSpec, DeckTabBarSpec};
use crate::decks::object::draw_control_list;
pub(super) use crate::decks::vis::draw_card_panel;
use crate::display::{DisplayList, Rect, TextAlign};
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
    let zone_badge = (rack.active_tab != RackTab::Zones
        && rack.zone_count > 1
        && !rack.active_zone_name.is_empty())
    .then(|| format!("ZONE: {} ({}z)", rack.active_zone_name, rack.zone_count));
    let hint = match rack.active_tab {
        RackTab::Panel => "H/L tab · h/l mod · Alt+h/l ,/. adjust · a add · x del · p pin",
        RackTab::Patchbay => "H/L tab · hjkl move · w/b cable · Alt+h/l ,/. amt · c patch · x cut",
        RackTab::Zones => "H/L tab · h/l zone · j/k ctrl · Alt+h/l ,/. adjust · a/d/x zone",
    };
    draw_deck_tab_bar(
        dl,
        bar,
        &DeckTabBarSpec {
            tabs: &rack.tabs,
            active_accent: theme.accent.focus,
            badge: zone_badge.as_deref().map(|b| (b, theme.accent.modulation)),
            right_hint: Some(hint),
        },
        theme,
    );
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
        ModuleKindTag::Out => theme.accent.focus,
    }
}

pub(super) fn split_main_and_scope(area: Rect) -> (Rect, Option<Rect>) {
    let scope_w = (area.w * 0.28).clamp(140.0, 250.0);
    crate::decks::vis::split_side_card(area, 440.0, scope_w)
}

pub(super) fn draw_simple_card_header(
    dl: &mut DisplayList,
    hdr: Rect,
    title: &str,
    right: Option<&str>,
    color: Color,
    theme: &Theme,
) {
    crate::decks::vis::draw_card_header(dl, hdr, title, right, color, true, theme);
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
    crate::decks::vis::draw_badge_header(
        dl,
        hdr,
        badges,
        title,
        right_summary,
        title_color,
        false,
        theme,
    );
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
