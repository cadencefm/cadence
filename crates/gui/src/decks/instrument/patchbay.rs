//! Instrument `Patchbay` tab: Eurorack multi-column node-and-cable graph delegated
//! strictly to [`crate::decks::cable_graph`] (SPOT) plus footer summary & compact live scope.
use super::{draw_compact_scope_strip, module_tag_accent};
use crate::decks::cable_graph::{
    draw_cable_graph, draw_cable_legend, CableColumn, CableEdge, CableGraph, CableNode,
};
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::{Color, Theme};
use cadence_app::view::{PatchbayCableKind, PatchbayDeckView};

pub(super) fn draw_patchbay_tab(
    dl: &mut DisplayList,
    body: Rect,
    pb: &PatchbayDeckView,
    theme: &Theme,
) {
    let footer_h = if body.h >= 240.0 { 44.0 } else { 34.0 };
    let (legend_row, rest) = body.split_top(22.0);
    let (graph_canvas, footer_area) = rest.split_bottom(footer_h);
    let graph_canvas = Rect::new(
        graph_canvas.x,
        graph_canvas.y + 2.0,
        graph_canvas.w,
        (graph_canvas.h - 6.0).max(0.0),
    );

    // Semantic color-coded wire legend strictly from `&Theme` + ASCII `"z-1"` feedback indicator.
    let mut legend: Vec<(&str, Color)> = vec![
        ("AUDIO", cable_kind_color(PatchbayCableKind::Audio, theme)),
        ("CV", cable_kind_color(PatchbayCableKind::Cv, theme)),
        ("PITCH", cable_kind_color(PatchbayCableKind::Pitch, theme)),
        ("FM", cable_kind_color(PatchbayCableKind::Fm, theme)),
        ("GATE", cable_kind_color(PatchbayCableKind::Gate, theme)),
    ];
    let fb_legend = format!("FB {}", pb.feedback_badge);
    if pb.has_feedback {
        legend.push((fb_legend.as_str(), theme.accent.alert));
    }
    let right_pill = if pb.has_feedback {
        format!(
            "{} MOD · {} CABLES · {} FB",
            pb.nodes.len(),
            pb.cables.len(),
            pb.feedback_badge
        )
    } else {
        format!("{} MOD · {} CABLES", pb.nodes.len(), pb.cables.len())
    };
    draw_cable_legend(dl, legend_row, &legend, Some(&right_pill), theme);

    // Map `PatchbayDeckView` cleanly onto `cable_graph.rs` SPOT types.
    let columns: Vec<CableColumn<'_>> = pb
        .columns
        .iter()
        .map(|col| {
            let nodes = col
                .nodes
                .iter()
                .map(|node| {
                    let mut cn = CableNode::new(
                        &node.name,
                        node.badge,
                        &node.detail,
                        module_tag_accent(node.kind, theme),
                    );
                    cn.selected = node.selected;
                    cn.in_path = node.in_path;
                    cn
                })
                .collect();
            CableColumn::new(col.title, nodes)
        })
        .collect();

    let edges: Vec<CableEdge<'_>> = pb
        .cables
        .iter()
        .map(|cable| {
            CableEdge::new(
                cable.from_col,
                cable.from_row,
                cable.to_col,
                cable.to_row,
                cable_kind_color(cable.kind, theme),
                cable.highlighted,
            )
            .with_label(Some(&cable.label))
            .with_dashed(cable.feedback)
            .with_selected(cable.selected)
            .with_ports(0.5, cable.to_port_y)
        })
        .collect();

    let cable_graph = CableGraph::new(&columns, &edges, Some(pb.selected_col), pb.selected_row);
    draw_cable_graph(dl, graph_canvas, &cable_graph, theme);

    // Bottom footer strip: selected module/cable summary + compact live oscilloscope strip.
    draw_patchbay_footer(dl, footer_area, pb, theme);
}

fn draw_patchbay_footer(dl: &mut DisplayList, area: Rect, pb: &PatchbayDeckView, theme: &Theme) {
    if area.is_empty() {
        return;
    }
    let (info_rect, scope_rect) = if area.w >= 460.0 {
        let sw = (area.w * 0.28).clamp(130.0, 240.0);
        let (l, r) = area.split_right(sw);
        (Rect::new(l.x, l.y, (l.w - 6.0).max(0.0), l.h), Some(r))
    } else {
        (area, None)
    };

    let cable_focused = pb.selected_cable.is_some();
    let accent = if cable_focused {
        theme.accent.warning_key
    } else {
        theme.accent.focus
    };
    dl.panel(
        info_rect,
        if cable_focused {
            accent.with_alpha(0.14)
        } else {
            theme.surface.recessed
        },
        accent.with_alpha(if cable_focused { 0.85 } else { 0.45 }),
        theme.radii.pill,
        1.0,
        0.0,
    );
    dl.rounded(
        Rect::new(
            info_rect.x + 2.0,
            info_rect.y + 5.0,
            3.0,
            (info_rect.h - 10.0).max(4.0),
        ),
        accent,
        1.5,
    );

    let inner = Rect::new(
        info_rect.x + 11.0,
        info_rect.y + 3.0,
        (info_rect.w - 17.0).max(0.0),
        (info_rect.h - 6.0).max(0.0),
    );
    dl.push_clip(inner);
    let mut right_reserve = 0.0;
    if pb.has_feedback && inner.w >= 160.0 {
        let fb_text = format!("{} FEEDBACK", pb.feedback_badge);
        let ffont = theme.font.small - 1.5;
        let fw = (fb_text.chars().count() as f32 * theme.char_width(ffont) + 12.0).min(110.0);
        let fr = Rect::new(
            inner.right() - fw,
            inner.y + ((inner.h - 16.0) * 0.5).max(0.0),
            fw,
            16.0_f32.min(inner.h),
        );
        dl.panel(
            fr,
            theme.accent.alert.with_alpha(0.18),
            theme.accent.alert.with_alpha(0.85),
            theme.radii.badge,
            1.0,
            0.0,
        );
        dl.text(
            fr,
            fb_text,
            ffont,
            true,
            theme.accent.alert,
            TextAlign::Center,
        );
        right_reserve = fw + 8.0;
    }

    let text_w = (inner.w - right_reserve).max(0.0);
    let cw = theme.char_width(theme.font.small);
    let max_c = (text_w / cw).floor().max(4.0) as usize;
    dl.text(
        Rect::new(inner.x, inner.y, text_w, inner.h),
        trim_label(&pb.selected_summary, max_c),
        theme.font.small,
        true,
        if cable_focused {
            accent
        } else {
            theme.text.primary
        },
        TextAlign::Left,
    );
    dl.pop_clip();

    if let Some(sr) = scope_rect {
        draw_compact_scope_strip(dl, sr, &pb.live_waveform, theme);
    }
}

fn cable_kind_color(kind: PatchbayCableKind, theme: &Theme) -> Color {
    match kind {
        PatchbayCableKind::Audio => theme.accent.routing,
        PatchbayCableKind::Cv => theme.accent.modulation,
        PatchbayCableKind::Pitch => theme.accent.playhead,
        PatchbayCableKind::Fm => theme.accent.warning_key,
        PatchbayCableKind::Gate => theme.accent.focus,
    }
}
