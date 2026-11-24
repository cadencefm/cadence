//! GPU Live Relationship / Patchbay Graph deck (`gG`):
//! 5 signal-flow columns (`SOURCES` → `DERIVED` → `PARTS` → `INSTRUMENTS` → `MIXER / FX`)
//! linked by smooth cubic-Bezier GPU patch cables with upstream + downstream path illumination
//! and Reason/ReWire back-panel offscreen cable routing + scroll indicators.
//!
//! All multi-column layout, node card rendering, offscreen scroll stubs, and cubic-Bezier
//! cable rendering are delegated to the shared [`crate::decks::cable_graph`] SPOT module.
use crate::chrome::{draw_deck_frame, DeckFrameSpec};
use crate::decks::cable_graph::{
    audio_activity_norm, draw_cable_graph, draw_cable_legend, CableColumn, CableEdge, CableGraph,
    CableNode, CableRouting,
};
use crate::display::{DisplayList, Rect};
use crate::theme::{Color, Theme};
use cadence_app::view::{GraphDeckView, GraphEdgeKind, GraphNode, GraphTarget, UiModeKind};

fn edge_base_color(kind: GraphEdgeKind, color_index: usize, theme: &Theme) -> Color {
    match kind {
        GraphEdgeKind::Pattern => theme.track_color(color_index).mix(theme.accent.focus, 0.35),
        GraphEdgeKind::Signal => theme.accent.routing,
        GraphEdgeKind::Routing => theme.accent.playhead,
        GraphEdgeKind::Modulation => theme.accent.modulation,
        GraphEdgeKind::Macro => theme
            .track_color(color_index)
            .mix(theme.accent.audition, 0.45),
        GraphEdgeKind::Sidechain => theme.accent.warning_key,
    }
}

fn node_accent(node: &GraphNode, theme: &Theme) -> Color {
    if node.is_effect {
        theme.accent.modulation.mix(theme.accent.routing, 0.45)
    } else if node.is_master {
        theme.accent.playhead
    } else {
        theme.track_color(node.color_index)
    }
}

pub fn draw_graph(
    dl: &mut DisplayList,
    area: Rect,
    view: &GraphDeckView,
    focused: bool,
    mode: UiModeKind,
    theme: &Theme,
) {
    let inner = draw_deck_frame(
        dl,
        area,
        &DeckFrameSpec {
            title: "SIGNAL GRAPH",
            right_badge: Some(&view.selected_summary),
            accent: theme.accent.routing,
            focused,
            mode,
        },
        theme,
    );
    if inner.w < 240.0 || inner.h < 120.0 {
        return;
    }
    dl.push_clip(inner);

    let (legend_row, canvas) = inner.split_top(22.0);
    draw_cable_legend(
        dl,
        legend_row,
        &[
            ("PATTERN", theme.accent.focus),
            ("SIGNAL", theme.accent.routing),
            ("MOD", theme.accent.modulation),
            ("MACRO", theme.accent.audition),
            ("DUCK", theme.accent.warning_key),
            ("BUS", theme.accent.playhead),
        ],
        (!view.selected_cables.is_empty()).then_some(view.selected_cables.as_str()),
        theme,
    );

    let columns: Vec<CableColumn<'_>> = view
        .columns
        .iter()
        .map(|col| {
            let nodes = col
                .nodes
                .iter()
                .map(|node| {
                    let activity = match node.target {
                        GraphTarget::Instrument(_) | GraphTarget::Channel(_) => {
                            audio_activity_norm(node.activity)
                        }
                        _ => node.activity.clamp(0.0, 1.0),
                    };
                    let mut cn = CableNode::new(
                        &node.name,
                        &node.badge,
                        &node.detail,
                        node_accent(node, theme),
                    );
                    cn.activity = activity;
                    cn.hot = node.hot || node.activity > 1.0;
                    cn.clipped = node.clipped;
                    cn.bypassed = node.is_effect && node.is_bypassed;
                    cn.selected = node.selected;
                    cn.in_path = node.in_path;
                    cn.indented = node.is_effect;
                    cn
                })
                .collect();
            CableColumn::new(col.title, nodes)
        })
        .collect();

    let edges: Vec<CableEdge<'_>> = view
        .edges
        .iter()
        .map(|edge| {
            let routing = if edge.kind == GraphEdgeKind::Signal {
                CableRouting::InsertSpine
            } else {
                CableRouting::Bezier
            };
            CableEdge::new(
                edge.from_col,
                edge.from_row,
                edge.to_col,
                edge.to_row,
                edge_base_color(edge.kind, edge.color_index, theme),
                edge.highlighted,
            )
            .with_routing(routing)
        })
        .collect();

    let cable_graph = CableGraph::new(&columns, &edges, Some(view.selected_col), view.selected_row);
    draw_cable_graph(dl, canvas, &cable_graph, theme);

    dl.pop_clip();
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadence_app::view::{GraphColumn, GraphEdge, GraphNode};

    fn dummy_node(col: usize, row: usize, selected: bool, in_path: bool) -> GraphNode {
        GraphNode {
            col,
            row,
            target: GraphTarget::Pattern((col * 100 + row + 1) as u64),
            name: format!("Node {col}:{row}"),
            badge: "PAT".into(),
            detail: "16 steps".into(),
            color_index: row,
            activity: 0.0,
            hot: false,
            clipped: false,
            selected,
            in_path,
            is_master: false,
            is_effect: false,
            is_bypassed: false,
        }
    }

    #[test]
    fn offscreen_scroll_indicators_and_backpanel_cables_are_emitted() {
        // Column 0 has 18 nodes (overflows a 400px tall deck), centered around row 9 so
        // nodes exist BOTH above and below the visible slice; column 1 has 2 visible nodes
        // patched from offscreen-above (row 0) and offscreen-below (row 17) in column 0!
        let col0_nodes: Vec<_> = (0..18)
            .map(|r| dummy_node(0, r, r == 9, r == 0 || r == 9 || r == 17))
            .collect();
        let col1_nodes = vec![dummy_node(1, 0, false, true), dummy_node(1, 1, false, true)];
        let view = GraphDeckView {
            columns: vec![
                GraphColumn {
                    title: "SOURCES",
                    nodes: col0_nodes,
                },
                GraphColumn {
                    title: "DERIVED",
                    nodes: col1_nodes,
                },
                GraphColumn {
                    title: "PARTS",
                    nodes: vec![],
                },
                GraphColumn {
                    title: "INSTRUMENTS",
                    nodes: vec![],
                },
                GraphColumn {
                    title: "MIXER / FX",
                    nodes: vec![],
                },
            ],
            edges: vec![
                // Highlighted offscreen-above source -> visible derived node 0
                GraphEdge {
                    from_col: 0,
                    from_row: 0,
                    to_col: 1,
                    to_row: 0,
                    kind: GraphEdgeKind::Pattern,
                    highlighted: true,
                    color_index: 0,
                },
                // Dim offscreen-below source -> visible derived node 1
                GraphEdge {
                    from_col: 0,
                    from_row: 17,
                    to_col: 1,
                    to_row: 1,
                    kind: GraphEdgeKind::Modulation,
                    highlighted: false,
                    color_index: 3,
                },
                // Highlighted offscreen-below source -> visible derived node 0
                GraphEdge {
                    from_col: 0,
                    from_row: 17,
                    to_col: 1,
                    to_row: 0,
                    kind: GraphEdgeKind::Signal,
                    highlighted: true,
                    color_index: 2,
                },
            ],
            selected_col: 0,
            selected_row: 9,
            selected_summary: "Node 0:9 [PAT]".into(),
            selected_cables: "0 IN · 0 OUT".into(),
        };

        let mut dl = DisplayList::new(1000.0, 400.0, 1.0);
        let theme = Theme::cadence_dark();
        draw_graph(
            &mut dl,
            Rect::new(0.0, 0.0, 1000.0, 400.0),
            &view,
            true,
            UiModeKind::Normal,
            &theme,
        );

        // Both offscreen cables must emit curve geometry (not be dropped!).
        assert!(
            dl.base.curve_vertices.len() > 100,
            "expected offscreen cubic-Bezier cables to emit curve vertices, got {}",
            dl.base.curve_vertices.len()
        );
        let texts: Vec<&str> = dl.base.texts.iter().map(|t| t.text.as_str()).collect();
        assert!(
            texts
                .iter()
                .any(|t| t.starts_with('▲') && t.contains("above")),
            "missing top scroll indicator in {texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| t.starts_with('▼') && t.contains("below")),
            "missing bottom scroll indicator in {texts:?}"
        );
    }

    #[test]
    fn non_overflowing_columns_emit_zero_scroll_indicators() {
        let view = GraphDeckView {
            columns: vec![
                GraphColumn {
                    title: "SOURCES",
                    nodes: vec![dummy_node(0, 0, true, true)],
                },
                GraphColumn {
                    title: "PARTS",
                    nodes: vec![dummy_node(1, 0, false, true)],
                },
            ],
            edges: vec![GraphEdge {
                from_col: 0,
                from_row: 0,
                to_col: 1,
                to_row: 0,
                kind: GraphEdgeKind::Pattern,
                highlighted: true,
                color_index: 0,
            }],
            selected_col: 0,
            selected_row: 0,
            selected_summary: "Node 0:0".into(),
            selected_cables: "0 IN · 1 OUT".into(),
        };
        let mut dl = DisplayList::new(1000.0, 500.0, 1.0);
        let theme = Theme::cadence_dark();
        draw_graph(
            &mut dl,
            Rect::new(0.0, 0.0, 1000.0, 500.0),
            &view,
            true,
            UiModeKind::Normal,
            &theme,
        );
        let texts: Vec<&str> = dl.base.texts.iter().map(|t| t.text.as_str()).collect();
        assert!(
            !texts
                .iter()
                .any(|t| t.contains("above") || t.contains("below")),
            "unexpected scroll indicator on fitting columns: {texts:?}"
        );
        assert!(!dl.base.curve_vertices.is_empty());
    }

    #[test]
    fn audio_node_shimmer_bars_use_db_norm_and_hot_clip_theme_tokens() {
        use cadence_core::mixer::ChannelRef;

        let make_ch =
            |row: usize, peak: f32, hot: bool, clipped: bool, is_master: bool| GraphNode {
                col: 0,
                row,
                target: GraphTarget::Channel(if is_master {
                    ChannelRef::Master
                } else {
                    ChannelRef::Channel(row as u64 + 1)
                }),
                name: format!("Ch{row}"),
                badge: "AUDIO".into(),
                detail: "+0.0 dB".into(),
                color_index: row,
                activity: peak,
                hot,
                clipped,
                selected: false,
                in_path: true,
                is_master,
                is_effect: false,
                is_bypassed: false,
            };
        let view = GraphDeckView {
            columns: vec![GraphColumn {
                title: "MIXER / FX",
                nodes: vec![
                    make_ch(0, 0.5, false, false, false),
                    make_ch(1, 1.4, true, false, false),
                    make_ch(2, 1.8, true, true, true),
                ],
            }],
            edges: vec![],
            selected_col: 0,
            selected_row: 0,
            selected_summary: "Ch0".into(),
            selected_cables: String::new(),
        };
        let mut dl = DisplayList::new(400.0, 400.0, 1.0);
        let theme = Theme::cadence_dark();
        draw_graph(
            &mut dl,
            Rect::new(0.0, 0.0, 400.0, 400.0),
            &view,
            true,
            UiModeKind::Normal,
            &theme,
        );
        let hot_rgba = theme.meter.hot.to_linear();
        let clip_rgba = theme.meter.clip.to_linear();
        let play_rgba = theme.accent.playhead.to_linear();
        let eq_rgba = |a: [f32; 4], b: [f32; 4]| {
            (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4 && (a[2] - b[2]).abs() < 1e-4
        };
        let shimmer_quads: Vec<_> = dl
            .base
            .quads
            .iter()
            .filter(|q| (q.rect[3] - 2.0).abs() < 0.01)
            .collect();
        assert_eq!(shimmer_quads.len(), 3, "expected 3 node meter shimmer bars");
        assert!(eq_rgba(shimmer_quads[0].color_top, play_rgba));
        assert!(eq_rgba(shimmer_quads[1].color_top, hot_rgba));
        assert!(eq_rgba(shimmer_quads[2].color_top, clip_rgba));
        // Float headroom: +3 dB (1.4) bar is wider than -6 dB (0.5) bar,
        // and +5.1 dB (1.8) bar is wider still!
        assert!(shimmer_quads[1].rect[2] > shimmer_quads[0].rect[2]);
        assert!(shimmer_quads[2].rect[2] > shimmer_quads[1].rect[2]);
    }
}
