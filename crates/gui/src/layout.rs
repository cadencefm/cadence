//! HiDPI pixel layout for Cadence's deck workspace.
//!
//! Mirrors `App::layout()` cleanly:
//! - Top transport header bar
//! - Main column: pinned wide decks stacked above the working deck (with breadcrumb trail)
//! - Right tall dock: Bank / Mixer pinned decks
//! - Bottom wide strip inside main column: Output oscilloscope deck
//! - Bottom status/command/hints footer bar
use crate::chrome::DECK_FRAME_INSET_X;
use crate::display::Rect;
use cadence_app::layout::{DeckKind, Layout, Shape, Spot};
use cadence_app::view::{DeckView, FrameView};

const GAP: f32 = 6.0;
const HEADER_H: f32 = 54.0;
const FOOTER_H: f32 = 56.0;
const TRAIL_H: f32 = 22.0;
const OUTPUT_H: f32 = 104.0;
const DOCK_MIN_W: f32 = 760.0;

pub struct FrameLayoutRects {
    pub header: Rect,
    pub body: Rect,
    pub footer: Rect,
    pub trail: Option<Rect>,
    pub decks: Vec<(usize, Rect)>,
}

/// Actual logical inner pane dimensions derived from the live deck `Layout`,
/// used to keep keyboard paging (`mixer_page_size`) aligned with rendered GPU
/// deck geometry across resizes, zoom toggles, and DPI changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewportPaneMetrics {
    pub mixer_inner_w: f32,
}

fn dock_width_for_kinds(body_w: f32, tall_kinds: impl IntoIterator<Item = DeckKind>) -> f32 {
    if body_w < DOCK_MIN_W {
        return 0.0;
    }
    let mut width = 0.0_f32;
    for kind in tall_kinds {
        let w = match kind {
            DeckKind::Mixer => (body_w * 0.36).clamp(320.0, 540.0),
            _ => (body_w * 0.24).clamp(240.0, 320.0),
        };
        width = width.max(w);
    }
    width.min(body_w * 0.48)
}

/// Compute actual logical inner width of the Mixer pane from `Layout`.
pub fn compute_pane_metrics(layout: &Layout, viewport: Rect) -> ViewportPaneMetrics {
    let (_, rest) = viewport.split_top(HEADER_H);
    let (body_outer, _) = rest.split_bottom(FOOTER_H);
    let body = body_outer.inset(GAP, GAP);

    if layout.zoomed() {
        let inner_w = (body.w - DECK_FRAME_INSET_X).max(0.0);
        return ViewportPaneMetrics {
            mixer_inner_w: inner_w,
        };
    }

    let tall_kinds: Vec<DeckKind> = layout
        .pinned()
        .iter()
        .filter(|d| d.kind.shape() == Shape::Tall)
        .map(|d| d.kind)
        .collect();
    let dock_w = dock_width_for_kinds(body.w, tall_kinds.iter().copied());
    let main_w = if dock_w > 0.0 {
        (body.w - dock_w - GAP).max(0.0)
    } else {
        body.w
    };
    let mixer_docked = dock_w > 0.0 && tall_kinds.contains(&DeckKind::Mixer);
    let mixer_outer_w = if mixer_docked { dock_w } else { main_w };

    ViewportPaneMetrics {
        mixer_inner_w: (mixer_outer_w - DECK_FRAME_INSET_X).max(0.0),
    }
}

pub fn compute_layout(frame: &FrameView, viewport: Rect) -> FrameLayoutRects {
    let (header, rest) = viewport.split_top(HEADER_H);
    let (body_outer, footer) = rest.split_bottom(FOOTER_H);
    let body = body_outer.inset(GAP, GAP);

    if frame.zoomed {
        let decks = frame
            .decks
            .iter()
            .enumerate()
            .find(|(_, d)| d.spot == frame.focused)
            .map(|(i, _)| vec![(i, body)])
            .unwrap_or_default();
        return FrameLayoutRects {
            header,
            body,
            footer,
            trail: None,
            decks,
        };
    }

    let tall: Vec<usize> = frame
        .decks
        .iter()
        .enumerate()
        .filter(|(_, d)| d.pinned && d.shape == Shape::Tall)
        .map(|(i, _)| i)
        .collect();
    let dock_w = dock_width_for_kinds(body.w, tall.iter().map(|&i| frame.decks[i].kind));
    let docking = dock_w > 0.0;

    let (mut main, dock_rect) = if dock_w > 0.0 {
        let (l, r) = body.split_right(dock_w);
        (Rect::new(l.x, l.y, (l.w - GAP).max(0.0), l.h), Some(r))
    } else {
        (body, None)
    };

    let mut decks = Vec::new();

    // Bottom Output deck inside the main column.
    if let Some((idx, _)) = frame
        .decks
        .iter()
        .enumerate()
        .find(|(_, d)| d.pinned && d.kind == DeckKind::Output)
    {
        if main.w >= 460.0 && main.h >= 260.0 {
            let (top, out_rect) = main.split_bottom(OUTPUT_H);
            main = Rect::new(top.x, top.y, top.w, (top.h - GAP).max(0.0));
            decks.push((idx, out_rect));
        }
    }

    // Docked right tall decks.
    if let Some(dock) = dock_rect {
        let count = tall.len() as f32;
        let total_gaps = GAP * (count - 1.0).max(0.0);
        let share = ((dock.h - total_gaps) / count.max(1.0)).max(0.0);
        let mut y = dock.y;
        for (n, &idx) in tall.iter().enumerate() {
            let h = if n + 1 == tall.len() {
                (dock.bottom() - y).max(0.0)
            } else {
                share
            };
            decks.push((idx, Rect::new(dock.x, y, dock.w, h)));
            y += h + GAP;
        }
    }

    // Stacked pinned wide decks + optional working deck in main column.
    let stacked: Vec<usize> = frame
        .decks
        .iter()
        .enumerate()
        .filter(|(_, d)| d.pinned && d.shape != Shape::Tall && d.kind != DeckKind::Output)
        .map(|(i, _)| i)
        .collect();

    let working_idx = frame
        .decks
        .iter()
        .enumerate()
        .find(|(_, d)| !d.pinned && d.spot == Spot::Working)
        .map(|(i, _)| i)
        .or_else(|| {
            if !docking {
                tall.iter()
                    .copied()
                    .find(|&i| frame.decks[i].spot == frame.focused)
            } else {
                None
            }
        });

    let mut trail = None;
    if let Some(w_idx) = working_idx {
        let mut y = main.y;
        if !stacked.is_empty() {
            let max_stack = (main.h * 0.48).max(120.0);
            let per_deck: Vec<f32> = stacked
                .iter()
                .map(|&i| match &frame.decks[i].view {
                    DeckView::Arrange(a) if a.evaluated.is_none() => {
                        let lane_rows = a.lanes.len() as f32
                            + a.lanes.iter().filter(|l| l.override_info.is_some()).count() as f32;
                        (62.0 + lane_rows * 28.0).clamp(130.0_f32.min(max_stack), max_stack)
                    }
                    _ => max_stack / stacked.len() as f32,
                })
                .collect();
            let total_wanted: f32 = per_deck.iter().sum::<f32>() + GAP * (stacked.len() as f32);
            let scale = if total_wanted > max_stack {
                max_stack / total_wanted
            } else {
                1.0
            };
            for (&idx, &wanted) in stacked.iter().zip(&per_deck) {
                let h = (wanted * scale).max(72.0);
                decks.push((idx, Rect::new(main.x, y, main.w, h)));
                y += h + GAP;
            }
        }
        let remaining = (main.bottom() - y).max(0.0);
        if remaining > TRAIL_H + 40.0 {
            trail = Some(Rect::new(main.x, y, main.w, TRAIL_H));
            y += TRAIL_H + 2.0;
            decks.push((
                w_idx,
                Rect::new(main.x, y, main.w, (main.bottom() - y).max(0.0)),
            ));
        } else {
            decks.push((w_idx, Rect::new(main.x, y, main.w, remaining)));
        }
    } else if !stacked.is_empty() {
        let count = stacked.len() as f32;
        let total_gaps = GAP * (count - 1.0).max(0.0);
        let share = ((main.h - total_gaps) / count.max(1.0)).max(0.0);
        let mut y = main.y;
        for (n, &idx) in stacked.iter().enumerate() {
            let h = if n + 1 == stacked.len() {
                (main.bottom() - y).max(0.0)
            } else {
                share
            };
            decks.push((idx, Rect::new(main.x, y, main.w, h)));
            y += h + GAP;
        }
    }

    FrameLayoutRects {
        header,
        body,
        footer,
        trail,
        decks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decks::mixer::MIXER_MIN_STRIP_W;
    use cadence_app::layout::Deck;

    #[test]
    fn pane_metrics_reflect_docked_and_zoomed_mixer_widths() {
        let viewport = Rect::new(0.0, 0.0, 1440.0, 900.0);
        let mut layout = Layout::new(
            vec![
                Deck::song(DeckKind::Arrange),
                Deck::song(DeckKind::Mixer),
                Deck::song(DeckKind::Output),
            ],
            None,
        );

        let docked = compute_pane_metrics(&layout, viewport);
        // Body is 1440 - 12 = 1428; docked Mixer is 36% = 514.08 -> inner 499.08 -> 5 strips.
        let docked_slots =
            ((docked.mixer_inner_w / MIXER_MIN_STRIP_W).floor() as usize).clamp(1, 16);
        assert_eq!(docked_slots, 5);

        layout.toggle_zoom();
        let zoomed = compute_pane_metrics(&layout, viewport);
        // Zoomed Mixer gets full body width 1428 - 15 = 1413 -> 16 strips.
        let zoomed_slots =
            ((zoomed.mixer_inner_w / MIXER_MIN_STRIP_W).floor() as usize).clamp(1, 16);
        assert_eq!(zoomed_slots, 16);
        assert!(zoomed.mixer_inner_w > docked.mixer_inner_w);
    }
}
