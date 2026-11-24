//! Deck dispatch and top-level `FrameView` -> `DisplayList` scene builder.
pub mod arrange;
pub mod bank;
pub mod cable_graph;
pub mod effect;
pub mod graph;
pub mod instrument;
pub mod macros;
pub mod midi;
pub mod mixer;
pub mod object;
pub mod output;
pub mod pattern;
pub mod take;

use crate::chrome::{draw_footer, draw_header, draw_trail};
use crate::display::{DisplayList, Rect};
use crate::layout::compute_layout;
use crate::overlays::{draw_jam_hud, draw_lane_layer_hud, draw_overlay};
use crate::theme::Theme;
use cadence_app::view::{DeckView, FrameView, UiModeKind};

/// Populate an existing `DisplayList` for one `FrameView` in logical coordinates,
/// reusing its backing `Vec` capacities across frames.
pub fn populate_display_list(
    dl: &mut DisplayList,
    frame: &FrameView,
    logical_width: f32,
    logical_height: f32,
    scale_factor: f32,
    theme: &Theme,
) {
    dl.reset(logical_width, logical_height, scale_factor);
    let viewport = Rect::new(0.0, 0.0, logical_width, logical_height);
    let rects = compute_layout(frame, viewport);

    draw_header(dl, rects.header, &frame.chrome, theme);
    if let Some(trail_rect) = rects.trail {
        draw_trail(dl, trail_rect, &frame.chrome.trail, theme);
    }

    let mode = if frame.overlay.is_some() {
        UiModeKind::Overlay
    } else {
        frame.chrome.mode_kind
    };
    for (idx, rect) in &rects.decks {
        let slot = &frame.decks[*idx];
        match &slot.view {
            DeckView::Arrange(v) => arrange::draw_arrange(dl, *rect, v, mode, theme),
            DeckView::Pattern(v) => pattern::draw_pattern(dl, *rect, v, mode, theme),
            DeckView::Bank(v) => bank::draw_bank(dl, *rect, v, slot.focused, mode, theme),
            DeckView::Mixer(v) => mixer::draw_mixer(dl, *rect, v, slot.focused, mode, theme),
            DeckView::Take(v) => take::draw_take(dl, *rect, v, slot.focused, mode, theme),
            DeckView::Graph(v) => graph::draw_graph(dl, *rect, v, slot.focused, mode, theme),
            DeckView::Midi(v) => midi::draw_midi(dl, *rect, v, slot.focused, mode, theme),
            DeckView::Macros(v) => macros::draw_macros(dl, *rect, v, slot.focused, mode, theme),
            DeckView::Instrument(v) => {
                instrument::draw_instrument(dl, *rect, v, slot.focused, mode, theme)
            }
            DeckView::Effect(v) => effect::draw_effect(dl, *rect, v, slot.focused, mode, theme),
            DeckView::Object(v) => object::draw_object(dl, *rect, v, slot.focused, mode, theme),
            DeckView::Output(v) => output::draw_output(dl, *rect, v, theme),
        }
    }

    draw_footer(dl, rects.footer, &frame.chrome, theme);

    if frame.jam_hud.is_some() || frame.lane_layer.is_some() || frame.overlay.is_some() {
        dl.begin_overlay();
        if let Some(jam) = &frame.jam_hud {
            draw_jam_hud(dl, viewport, rects.footer.h, jam, theme);
        }
        if let Some(layer) = &frame.lane_layer {
            let extra = if frame.jam_hud.is_some() { 132.0 } else { 0.0 };
            draw_lane_layer_hud(dl, viewport, rects.footer.h + extra, layer, theme);
        }
        if let Some(overlay) = &frame.overlay {
            draw_overlay(dl, viewport, overlay, theme);
        }
    }
}

/// Build a fresh GPU `DisplayList` for one `FrameView` in logical coordinates.
pub fn build_display_list(
    frame: &FrameView,
    logical_width: f32,
    logical_height: f32,
    scale_factor: f32,
    theme: &Theme,
) -> DisplayList {
    let mut dl = DisplayList::new(logical_width, logical_height, scale_factor);
    populate_display_list(
        &mut dl,
        frame,
        logical_width,
        logical_height,
        scale_factor,
        theme,
    );
    dl
}
