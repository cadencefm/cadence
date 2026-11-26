//! Reusable GPU multi-column node-and-cable graph renderer (SPOT).
//!
//! Composed by both:
//! - [`crate::decks::graph`] (Project Signal Graph `gG`), and
//! - [`crate::decks::instrument`] (Eurorack Rack `Patchbay` tab).
//!
//! Renders left-to-right signal-flow columns with overflow scroll regions (`▲ N above` /
//! `▼ N below`), Reason/ReWire back-panel offscreen cable routing & port jack stubs, smooth
//! cubic-Bezier GPU patch cables (solid or arc-length dashed for `z-1` 1-sample feedback),
//! optional inline attenuverter midpoint pill badges, upstream/downstream path illumination,
//! extra-bright focused cable highlighting, and themed node cards.
//!
//! All colors come strictly from caller-provided [`Color`] tokens or [`Theme`] — zero
//! hardcoded RGBA values live here. Every [`DisplayList::push_clip`] is strictly balanced
//! with [`DisplayList::pop_clip`].
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::{Color, Theme};

/// Right-side gutter reserved for same-column right-shoulder bus/feedback arcs.
const RIGHT_GUTTER: f32 = 26.0;

const DASH_LEN_PX: f32 = 7.0;
const DASH_GAP_PX: f32 = 4.5;
const LABEL_CANDIDATE_FRACS: [f32; 11] = [
    0.50, 0.42, 0.58, 0.34, 0.66, 0.26, 0.74, 0.20, 0.80, 0.14, 0.86,
];

/// Map an unclamped linear audio peak through `-60..+6 dBFS` (`decks::output::db_norm`)
/// for 32-bit float activity shimmer bars matching the Mixer and Output decks.
pub fn audio_activity_norm(peak: f32) -> f32 {
    crate::decks::output::db_norm(peak)
}

/// Single node card rendered inside a [`CableColumn`].
#[derive(Clone, Copy, Debug)]
pub struct CableNode<'a> {
    pub title: &'a str,
    pub badge: &'a str,
    pub detail: &'a str,
    /// Accent color for the card's left identity strip, path border, and sub-label.
    /// Automatically muted to `theme.text.muted` when [`Self::bypassed`] is `true`.
    pub accent: Color,
    /// Normalized `0.0..=1.0` live activity shimmer bar fill along the bottom edge.
    /// For linear audio float peaks, map through [`audio_activity_norm`].
    pub activity: f32,
    /// Informational 32-bit float headroom flag (`> 0 dBFS`) — tints shimmer bar `theme.meter.hot`.
    pub hot: bool,
    /// True digital clipping flag — tints shimmer bar `theme.meter.clip` with clip glow.
    pub clipped: bool,
    /// Bypassed/inactive state — mutes [`Self::accent`] to `theme.text.muted`.
    pub bypassed: bool,
    /// Primary focused node (bright warning-key border & glow).
    pub selected: bool,
    /// Upstream/downstream illuminated signal-path member.
    pub in_path: bool,
    /// Visually indent the card slightly to the right (e.g., insert FX cards under a channel).
    pub indented: bool,
}

impl<'a> CableNode<'a> {
    /// Construct a node card with default (inactive, unselected, non-indented) flags.
    pub const fn new(title: &'a str, badge: &'a str, detail: &'a str, accent: Color) -> Self {
        Self {
            title,
            badge,
            detail,
            accent,
            activity: 0.0,
            hot: false,
            clipped: false,
            bypassed: false,
            selected: false,
            in_path: false,
            indented: false,
        }
    }
}

/// One vertical signal-flow column in a [`CableGraph`].
#[derive(Clone, Debug)]
pub struct CableColumn<'a> {
    pub title: &'a str,
    pub nodes: Vec<CableNode<'a>>,
}

impl<'a> CableColumn<'a> {
    pub fn new(title: &'a str, nodes: Vec<CableNode<'a>>) -> Self {
        Self { title, nodes }
    }
}

/// Geometric routing shape for a [`CableEdge`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CableRouting {
    /// Smooth cubic-Bezier patch cable (cross-column S-curve, same-column right-shoulder arc,
    /// backward feedback drape, or self-feedback loop).
    #[default]
    Bezier,
    /// Crisp left-side orthogonal insert spine bracket for same-column parent -> indented-child
    /// chains (falls back to [`Self::Bezier`] when linking different columns).
    InsertSpine,
}

/// Directed patch cable linking `(from_col, from_row)` to `(to_col, to_row)`.
#[derive(Clone, Copy, Debug)]
pub struct CableEdge<'a> {
    pub from_col: usize,
    pub from_row: usize,
    pub to_col: usize,
    pub to_row: usize,
    /// Semantic wire color (sourced from `&Theme`).
    pub color: Color,
    /// Member of the illuminated upstream/downstream signal path.
    pub highlighted: bool,
    /// Curve routing style (`Bezier` by default; `InsertSpine` for same-column FX insert chains).
    pub routing: CableRouting,
    /// Optional inline pill badge at the cable midpoint (e.g., `"+23.2 st"`, `"-6 dB"`, `"0.85"`).
    /// Rendered crisply when selected/highlighted or when non-overlapping with cards and pills.
    pub label: Option<&'a str>,
    /// Render as an arc-length dashed curve (distinguishes `z-1` 1-sample feedback cables).
    pub dashed: bool,
    /// Extra-bright focused cable highlight (when focusing a cable to edit attenuverter or cut).
    pub selected: bool,
    /// Normalized `0.0..=1.0` live signal energy flowing across this cable (`0.0` = idle).
    /// Audio cables should be mapped through [`audio_activity_norm`].
    pub activity: f32,
    /// Normalized vertical jack position on the source card (`0.0..=1.0`, default `0.5` = center).
    pub from_port_y: f32,
    /// Normalized vertical jack position on the target card (`0.0..=1.0`, default `0.5` = center).
    pub to_port_y: f32,
}

impl<'a> CableEdge<'a> {
    /// Construct a standard solid, unselected cubic-Bezier cable with centered jacks and no label.
    pub const fn new(
        from_col: usize,
        from_row: usize,
        to_col: usize,
        to_row: usize,
        color: Color,
        highlighted: bool,
    ) -> Self {
        Self {
            from_col,
            from_row,
            to_col,
            to_row,
            color,
            highlighted,
            routing: CableRouting::Bezier,
            label: None,
            dashed: false,
            selected: false,
            activity: 0.0,
            from_port_y: 0.5,
            to_port_y: 0.5,
        }
    }

    pub const fn with_routing(mut self, routing: CableRouting) -> Self {
        self.routing = routing;
        self
    }

    pub const fn with_label(mut self, label: Option<&'a str>) -> Self {
        self.label = label;
        self
    }

    pub const fn with_dashed(mut self, dashed: bool) -> Self {
        self.dashed = dashed;
        self
    }

    pub const fn with_selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub const fn with_activity(mut self, activity: f32) -> Self {
        self.activity = activity;
        self
    }

    pub const fn with_ports(mut self, from_port_y: f32, to_port_y: f32) -> Self {
        self.from_port_y = from_port_y;
        self.to_port_y = to_port_y;
        self
    }
}

/// Complete multi-column node-and-cable graph scene specification.
#[derive(Clone, Debug)]
pub struct CableGraph<'a> {
    pub columns: &'a [CableColumn<'a>],
    pub edges: &'a [CableEdge<'a>],
    /// Focused column for active header styling and overflow scroll centering.
    pub selected_col: Option<usize>,
    /// Focused row inside `selected_col` used to center overflow scroll slices.
    pub selected_row: usize,
}

impl<'a> CableGraph<'a> {
    pub const fn new(
        columns: &'a [CableColumn<'a>],
        edges: &'a [CableEdge<'a>],
        selected_col: Option<usize>,
        selected_row: usize,
    ) -> Self {
        Self {
            columns,
            edges,
            selected_col,
            selected_row,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum NodeSlot {
    Visible(Rect),
    OffscreenAbove,
    OffscreenBelow,
}

#[derive(Clone, Copy, Debug)]
struct ColLayout {
    rect: Rect,
    above_count: usize,
    below_count: usize,
    top_strip: Rect,
    bot_strip: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EdgeTier {
    Dim,
    Highlighted,
    Selected,
}

impl EdgeTier {
    fn of(edge: &CableEdge<'_>) -> Self {
        if edge.selected {
            Self::Selected
        } else if edge.highlighted {
            Self::Highlighted
        } else {
            Self::Dim
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct PortStub {
    pos: (f32, f32),
    color: Color,
    highlighted: bool,
    selected: bool,
    above: bool,
}

#[derive(Clone, Copy, Debug)]
struct VisibleJack {
    pos: (f32, f32),
    color: Color,
    selected: bool,
}

#[derive(Clone, Debug)]
struct PendingLabel<'a> {
    pts: Vec<(f32, f32)>,
    text: &'a str,
    color: Color,
    tier: EdgeTier,
}

struct OffscreenTracker {
    strip_has_dim_above: Vec<bool>,
    strip_has_dim_below: Vec<bool>,
    strip_hl_above: Vec<Option<Color>>,
    strip_hl_below: Vec<Option<Color>>,
    port_stubs: Vec<PortStub>,
    visible_jacks: Vec<VisibleJack>,
}

impl OffscreenTracker {
    fn new(col_count: usize) -> Self {
        Self {
            strip_has_dim_above: vec![false; col_count],
            strip_has_dim_below: vec![false; col_count],
            strip_hl_above: vec![None; col_count],
            strip_hl_below: vec![None; col_count],
            port_stubs: Vec::new(),
            visible_jacks: Vec::new(),
        }
    }
}

/// Draw the compact color-coded wire legend on the left of `area` and an optional
/// right-aligned status pill (e.g., `"2 IN · 1 OUT"` or focused cable readout).
pub fn draw_cable_legend(
    dl: &mut DisplayList,
    area: Rect,
    items: &[(&str, Color)],
    right_pill: Option<&str>,
    theme: &Theme,
) {
    if area.is_empty() {
        return;
    }
    dl.push_clip(area);

    let font = theme.font.small - 1.0;
    let cw = theme.char_width(font);
    let mut lx = area.x;
    for &(lbl, col) in items {
        let pill_w = lbl.chars().count() as f32 * cw + 22.0;
        if lx + pill_w > area.right() {
            break;
        }
        dl.rounded_glow(Rect::new(lx, area.y + 6.0, 8.0, 8.0), col, 4.0, 0.30);
        dl.text(
            Rect::new(lx + 11.0, area.y, pill_w - 11.0, area.h),
            lbl,
            font,
            true,
            theme.text.secondary,
            TextAlign::Left,
        );
        lx += pill_w + 6.0;
    }

    if let Some(pill) = right_pill.filter(|s| !s.is_empty()) {
        let pill_w =
            (pill.chars().count() as f32 * cw + 16.0).min((area.right() - lx - 6.0).max(0.0));
        if pill_w >= 64.0 {
            let pr = Rect::new(area.right() - pill_w, area.y + 1.0, pill_w, 18.0);
            dl.panel(
                pr,
                theme.surface.elevated,
                theme.accent.routing.with_alpha(0.55),
                theme.radii.badge,
                1.0,
                0.0,
            );
            let max_p = ((pr.w - 10.0) / cw).max(4.0) as usize;
            dl.text(
                pr.inset(5.0, 0.0),
                trim_label(pill, max_p),
                font,
                true,
                theme.accent.warning_key,
                TextAlign::Center,
            );
        }
    }

    dl.pop_clip();
}

/// Render a complete multi-column node-and-cable graph into `canvas`.
pub fn draw_cable_graph(dl: &mut DisplayList, canvas: Rect, graph: &CableGraph<'_>, theme: &Theme) {
    if canvas.w < 80.0 || canvas.h < 48.0 || graph.columns.is_empty() {
        return;
    }
    dl.push_clip(canvas);

    let col_count = graph.columns.len();
    let usable_w = (canvas.w - RIGHT_GUTTER).max(160.0_f32.min(canvas.w));
    let col_ratio = if col_count > 5 {
        0.72 / col_count as f32
    } else {
        0.148
    };
    let max_per_col = ((usable_w - 40.0).max(32.0)) / col_count as f32;
    let col_w = (usable_w * col_ratio).clamp(108.0, 210.0).min(max_per_col);
    let gap_x = if col_count > 1 {
        ((usable_w - col_count as f32 * col_w) / (col_count - 1) as f32).max(16.0)
    } else {
        0.0
    };

    // Precompute card rects and offscreen scroll slices for every column.
    let mut node_slots: Vec<Vec<NodeSlot>> = Vec::with_capacity(col_count);
    let mut col_layouts: Vec<ColLayout> = Vec::with_capacity(col_count);
    for (c, col) in graph.columns.iter().enumerate() {
        let cx = canvas.x + c as f32 * (col_w + gap_x);
        let crect = Rect::new(cx, canvas.y, col_w, canvas.h);
        let (layout, mut slots) = layout_column(
            crect,
            col.nodes.len(),
            graph.selected_col == Some(c),
            graph.selected_row,
        );
        for (slot, node) in slots.iter_mut().zip(&col.nodes) {
            if node.indented {
                if let NodeSlot::Visible(r) = slot {
                    let indent = 10.0_f32.min((r.w - 48.0).max(0.0));
                    *r = Rect::new(r.x + indent, r.y, r.w - indent, r.h);
                }
            }
        }
        col_layouts.push(layout);
        node_slots.push(slots);
    }

    // Column headers & subtle vertical column backplates.
    for (c, (col, lay)) in graph.columns.iter().zip(&col_layouts).enumerate() {
        let crect = lay.rect;
        let active_col = graph.selected_col == Some(c);
        dl.rounded(
            Rect::new(crect.x - 3.0, crect.y, crect.w + 6.0, crect.h),
            if active_col {
                theme.surface.elevated.with_alpha(0.32)
            } else {
                theme.surface.recessed.with_alpha(0.24)
            },
            theme.radii.panel,
        );
        let hdr = format!("{} ({})", col.title, col.nodes.len());
        dl.text(
            Rect::new(crect.x + 2.0, crect.y + 2.0, crect.w - 4.0, 18.0),
            hdr,
            theme.font.small,
            true,
            if active_col {
                theme.accent.focus
            } else {
                theme.text.muted
            },
            TextAlign::Center,
        );
    }

    // Pass 1 (dim background), Pass 2 (illuminated path), & Pass 2b (focused selected cable)
    // for both visible-to-visible and visible-to-offscreen cables.
    let mut tracker = OffscreenTracker::new(col_count);
    let mut pending_labels: Vec<PendingLabel<'_>> = Vec::new();
    let edge_ctx = GraphEdgeCtx {
        canvas,
        col_layouts: &col_layouts,
        gap_x,
    };

    for tier in [EdgeTier::Dim, EdgeTier::Highlighted, EdgeTier::Selected] {
        for edge in graph.edges.iter().filter(|e| EdgeTier::of(e) == tier) {
            let styled = StyledEdge { edge, tier };
            let from_slot = node_slots
                .get(edge.from_col)
                .and_then(|c| c.get(edge.from_row))
                .copied();
            let to_slot = node_slots
                .get(edge.to_col)
                .and_then(|c| c.get(edge.to_row))
                .copied();
            let pts_opt = match (from_slot, to_slot) {
                (Some(NodeSlot::Visible(from)), Some(NodeSlot::Visible(to))) => {
                    Some(draw_visible_edge(
                        dl,
                        &edge_ctx,
                        VisibleEndpoints { from, to },
                        styled,
                        &mut tracker.visible_jacks,
                        theme,
                    ))
                }
                (Some(NodeSlot::Visible(from)), Some(NodeSlot::OffscreenAbove)) => {
                    draw_offscreen_edge(
                        dl,
                        &edge_ctx,
                        OffscreenEndpoint {
                            vis_rect: from,
                            vis_col: edge.from_col,
                            off_col: edge.to_col,
                            vis_port_y: edge.from_port_y,
                            above: true,
                        },
                        styled,
                        &mut tracker,
                        theme,
                    )
                }
                (Some(NodeSlot::Visible(from)), Some(NodeSlot::OffscreenBelow)) => {
                    draw_offscreen_edge(
                        dl,
                        &edge_ctx,
                        OffscreenEndpoint {
                            vis_rect: from,
                            vis_col: edge.from_col,
                            off_col: edge.to_col,
                            vis_port_y: edge.from_port_y,
                            above: false,
                        },
                        styled,
                        &mut tracker,
                        theme,
                    )
                }
                (Some(NodeSlot::OffscreenAbove), Some(NodeSlot::Visible(to))) => {
                    draw_offscreen_edge(
                        dl,
                        &edge_ctx,
                        OffscreenEndpoint {
                            vis_rect: to,
                            vis_col: edge.to_col,
                            off_col: edge.from_col,
                            vis_port_y: edge.to_port_y,
                            above: true,
                        },
                        styled,
                        &mut tracker,
                        theme,
                    )
                }
                (Some(NodeSlot::OffscreenBelow), Some(NodeSlot::Visible(to))) => {
                    draw_offscreen_edge(
                        dl,
                        &edge_ctx,
                        OffscreenEndpoint {
                            vis_rect: to,
                            vis_col: edge.to_col,
                            off_col: edge.from_col,
                            vis_port_y: edge.to_port_y,
                            above: false,
                        },
                        styled,
                        &mut tracker,
                        theme,
                    )
                }
                _ => None,
            };

            if let (Some(pts), Some(label)) = (pts_opt, edge.label.filter(|s| !s.is_empty())) {
                if pts.len() >= 2 {
                    pending_labels.push(PendingLabel {
                        pts,
                        text: label,
                        color: edge.color,
                        tier,
                    });
                }
            }
        }
    }

    // Pass 3: crisp themed node cards on top of cables.
    draw_node_cards(dl, graph.columns, &node_slots, theme);

    // Crisp top/bottom column scroll indicators (`▲ N above` / `▼ N below`).
    draw_scroll_indicators(dl, &col_layouts, &tracker, theme);

    // Draw port jack sockets on top of card/pill edges (dim stubs, visible jacks, active stubs).
    draw_port_jacks(dl, &tracker, theme);

    // Pass 4: optional inline midpoint attenuverter pill badges (selected -> highlighted -> dim).
    if !pending_labels.is_empty() {
        draw_cable_labels(
            dl,
            canvas,
            &col_layouts,
            &node_slots,
            &pending_labels,
            theme,
        );
    }

    dl.pop_clip();
}

fn layout_column(
    crect: Rect,
    node_count: usize,
    is_selected_col: bool,
    selected_row: usize,
) -> (ColLayout, Vec<NodeSlot>) {
    let full_body = Rect::new(crect.x, crect.y + 22.0, crect.w, (crect.h - 26.0).max(1.0));
    let mut slots = vec![NodeSlot::OffscreenBelow; node_count];
    if node_count == 0 {
        return (
            ColLayout {
                rect: crect,
                above_count: 0,
                below_count: 0,
                top_strip: Rect::new(full_body.x, full_body.y, full_body.w, 0.0),
                bot_strip: Rect::new(full_body.x, full_body.bottom(), full_body.w, 0.0),
            },
            slots,
        );
    }

    let min_h = 26.0;
    let max_h = 46.0;
    let gap_y = 6.0;
    let max_fit_all = ((full_body.h + gap_y) / (min_h + gap_y)).floor().max(1.0) as usize;

    let (cards_body, top_strip, bot_strip) = if node_count <= max_fit_all {
        (
            full_body,
            Rect::new(full_body.x, full_body.y, full_body.w, 0.0),
            Rect::new(full_body.x, full_body.bottom(), full_body.w, 0.0),
        )
    } else {
        let strip_h = 17.0_f32.min(((full_body.h - min_h) * 0.5).max(0.0));
        let top = Rect::new(full_body.x, full_body.y, full_body.w, strip_h);
        let bot = Rect::new(
            full_body.x,
            full_body.bottom() - strip_h,
            full_body.w,
            strip_h,
        );
        let pad = 4.0_f32.min(((full_body.h - 2.0 * strip_h - min_h) * 0.5).max(0.0));
        let inner_h = (full_body.h - 2.0 * (strip_h + pad)).max(min_h);
        let cards = Rect::new(
            full_body.x,
            full_body.y + strip_h + pad,
            full_body.w,
            inner_h,
        );
        (cards, top, bot)
    };

    let max_visible = ((cards_body.h + gap_y) / (min_h + gap_y)).floor().max(1.0) as usize;
    let vis_count = node_count.min(max_visible);
    let start_idx = if node_count <= vis_count {
        0
    } else if is_selected_col {
        selected_row
            .saturating_sub(vis_count / 2)
            .min(node_count - vis_count)
    } else {
        0
    };
    let end_idx = start_idx + vis_count;
    let card_h = ((cards_body.h - gap_y * (vis_count.saturating_sub(1)) as f32) / vis_count as f32)
        .clamp(min_h, max_h);
    let total_h = vis_count as f32 * card_h + vis_count.saturating_sub(1) as f32 * gap_y;
    let top_y = cards_body.y + ((cards_body.h - total_h) * 0.5).max(0.0);

    for (row_idx, slot) in slots.iter_mut().enumerate() {
        if row_idx < start_idx {
            *slot = NodeSlot::OffscreenAbove;
        } else if row_idx >= end_idx {
            *slot = NodeSlot::OffscreenBelow;
        } else {
            let vis_i = row_idx - start_idx;
            let y = top_y + vis_i as f32 * (card_h + gap_y);
            *slot = NodeSlot::Visible(Rect::new(cards_body.x, y, cards_body.w, card_h));
        }
    }

    (
        ColLayout {
            rect: crect,
            above_count: start_idx,
            below_count: node_count.saturating_sub(end_idx),
            top_strip,
            bot_strip,
        },
        slots,
    )
}

struct GraphEdgeCtx<'a> {
    canvas: Rect,
    col_layouts: &'a [ColLayout],
    gap_x: f32,
}

#[derive(Clone, Copy)]
struct StyledEdge<'a> {
    edge: &'a CableEdge<'a>,
    tier: EdgeTier,
}

#[derive(Clone, Copy)]
struct VisibleEndpoints {
    from: Rect,
    to: Rect,
}

#[derive(Clone, Copy)]
struct OffscreenEndpoint {
    vis_rect: Rect,
    vis_col: usize,
    off_col: usize,
    vis_port_y: f32,
    above: bool,
}

fn draw_visible_edge(
    dl: &mut DisplayList,
    ctx: &GraphEdgeCtx<'_>,
    endpoints: VisibleEndpoints,
    styled: StyledEdge<'_>,
    visible_jacks: &mut Vec<VisibleJack>,
    theme: &Theme,
) -> Vec<(f32, f32)> {
    let VisibleEndpoints { from, to } = endpoints;
    let StyledEdge { edge, tier } = styled;
    let canvas = ctx.canvas;
    let gap_x = ctx.gap_x;
    let from_y = from.y + from.h * port_y_clamped(edge.from_port_y);
    let to_y = to.y + to.h * port_y_clamped(edge.to_port_y);

    let pts = if edge.from_col == edge.to_col && edge.from_row == edge.to_row {
        // Self-feedback loop (e.g. VCO -> VCO.FmIn via z-1): crisp right-shoulder loop.
        let x0 = from.right();
        let ly0 = from.y + from.h * 0.28;
        let ly1 = from.y + from.h * 0.72;
        let bow = (gap_x * 0.48).clamp(18.0, 32.0);
        sample_cubic(
            (x0, ly0),
            (x0 + bow, from.y - 4.0),
            (x0 + bow, from.bottom() + 4.0),
            (x0, ly1),
            16,
        )
    } else if edge.from_col == edge.to_col && edge.routing == CableRouting::InsertSpine {
        // Insert FX chain spine: crisp left-side bracket from owning Channel/Effect down into indented FX card.
        let spine_x = (to.x - 5.0).max(from.x + 4.0);
        let y0 = from.bottom();
        vec![(spine_x, y0), (spine_x, to_y), (to.x, to_y)]
    } else if edge.from_col == edge.to_col {
        // Same-column bus/derivation/modulation curve bows neatly out along the right shoulder.
        let x0 = from.right();
        let x1 = to.right();
        let bow = (gap_x * 0.48).clamp(14.0, 32.0);
        sample_cubic(
            (x0, from_y),
            (x0 + bow, from_y),
            (x1 + bow, to_y),
            (x1, to_y),
            14,
        )
    } else {
        let forward = edge.to_col > edge.from_col;
        let x0 = if forward { from.right() } else { from.x };
        let x1 = if forward { to.x } else { to.right() };
        let dx = (x1 - x0).abs() * 0.48;
        let arch = if !forward && (to_y - from_y).abs() < 28.0 {
            // Subtle gravity drape on backward feedback wires at matching heights so they
            // stay cleanly separated from horizontal forward audio cables.
            if from_y.max(to_y) + 30.0 <= canvas.bottom() {
                22.0
            } else {
                -22.0
            }
        } else {
            0.0
        };
        let c0 = if forward {
            (x0 + dx, from_y)
        } else {
            (x0 - dx, from_y + arch)
        };
        let c1 = if forward {
            (x1 - dx, to_y)
        } else {
            (x1 + dx, to_y + arch)
        };
        sample_cubic((x0, from_y), c0, c1, (x1, to_y), 16)
    };

    emit_edge_stroke(
        dl,
        &pts,
        edge.color,
        tier,
        edge.dashed,
        edge.activity,
        false,
        theme,
    );

    match tier {
        EdgeTier::Dim => {}
        EdgeTier::Highlighted => {
            if let (Some(&start_pt), Some(&end_pt)) = (pts.first(), pts.last()) {
                visible_jacks.push(VisibleJack {
                    pos: start_pt,
                    color: edge.color,
                    selected: false,
                });
                visible_jacks.push(VisibleJack {
                    pos: end_pt,
                    color: edge.color,
                    selected: false,
                });
            }
        }
        EdgeTier::Selected => {
            let jack_col = edge.color.mix(theme.accent.warning_key, 0.35);
            if let (Some(&start_pt), Some(&end_pt)) = (pts.first(), pts.last()) {
                visible_jacks.push(VisibleJack {
                    pos: start_pt,
                    color: jack_col,
                    selected: true,
                });
                visible_jacks.push(VisibleJack {
                    pos: end_pt,
                    color: jack_col,
                    selected: true,
                });
            }
        }
    }

    pts
}

fn draw_offscreen_edge(
    dl: &mut DisplayList,
    ctx: &GraphEdgeCtx<'_>,
    end: OffscreenEndpoint,
    styled: StyledEdge<'_>,
    tracker: &mut OffscreenTracker,
    theme: &Theme,
) -> Option<Vec<(f32, f32)>> {
    let OffscreenEndpoint {
        vis_rect,
        vis_col,
        off_col,
        vis_port_y,
        above,
    } = end;
    let StyledEdge { edge, tier } = styled;
    let col_layouts = ctx.col_layouts;
    let gap_x = ctx.gap_x;
    let off_lay = col_layouts.get(off_col)?;
    let strip = if above {
        off_lay.top_strip
    } else {
        off_lay.bot_strip
    };

    let vy = vis_rect.y + vis_rect.h * port_y_clamped(vis_port_y);
    let py = strip.y + strip.h * 0.5;
    let (vx, px, pts) = if vis_col == off_col {
        // Same-column offscreen bus/derivation cable bows neatly along the right shoulder.
        let vx = vis_rect.right();
        let px = strip.right();
        let bow = (gap_x * 0.48).clamp(14.0, 32.0);
        let pts = sample_cubic((vx, vy), (vx + bow, vy), (px + bow, py), (px, py), 14);
        (vx, px, pts)
    } else {
        let to_right = off_col > vis_col;
        let vx = if to_right {
            vis_rect.right()
        } else {
            vis_rect.x
        };
        let px = if to_right { strip.x } else { strip.right() };
        let dx = (px - vx).abs() * 0.50;
        let c0 = if to_right {
            (vx + dx, vy)
        } else {
            (vx - dx, vy)
        };
        let c1 = if to_right {
            (px - dx, py)
        } else {
            (px + dx, py)
        };
        let pts = sample_cubic((vx, vy), c0, c1, (px, py), 16);
        (vx, px, pts)
    };

    emit_edge_stroke(
        dl,
        &pts,
        edge.color,
        tier,
        edge.dashed,
        edge.activity,
        true,
        theme,
    );

    match tier {
        EdgeTier::Dim => {
            if above {
                tracker.strip_has_dim_above[off_col] = true;
            } else {
                tracker.strip_has_dim_below[off_col] = true;
            }
            tracker.port_stubs.push(PortStub {
                pos: (px, py),
                color: edge.color,
                highlighted: false,
                selected: false,
                above,
            });
        }
        EdgeTier::Highlighted => {
            if above {
                tracker.strip_hl_above[off_col].get_or_insert(edge.color);
            } else {
                tracker.strip_hl_below[off_col].get_or_insert(edge.color);
            }
            tracker.visible_jacks.push(VisibleJack {
                pos: (vx, vy),
                color: edge.color,
                selected: false,
            });
            tracker.port_stubs.push(PortStub {
                pos: (px, py),
                color: edge.color,
                highlighted: true,
                selected: false,
                above,
            });
        }
        EdgeTier::Selected => {
            let sel_col = edge.color.mix(theme.accent.warning_key, 0.35);
            if above {
                tracker.strip_hl_above[off_col] = Some(sel_col);
            } else {
                tracker.strip_hl_below[off_col] = Some(sel_col);
            }
            tracker.visible_jacks.push(VisibleJack {
                pos: (vx, vy),
                color: sel_col,
                selected: true,
            });
            tracker.port_stubs.push(PortStub {
                pos: (px, py),
                color: sel_col,
                highlighted: true,
                selected: true,
                above,
            });
        }
    }

    Some(pts)
}

#[allow(clippy::too_many_arguments)]
fn emit_edge_stroke(
    dl: &mut DisplayList,
    pts: &[(f32, f32)],
    base_col: Color,
    tier: EdgeTier,
    dashed: bool,
    activity: f32,
    offscreen: bool,
    theme: &Theme,
) {
    let act = if activity.is_finite() {
        activity.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let live = act > 0.02;
    match tier {
        EdgeTier::Dim => {
            let base_alpha = if offscreen { 0.22 } else { 0.18 };
            if live {
                let alpha = (base_alpha + act * 0.44).min(0.68);
                let width = 1.1 + act * 0.75;
                let glow = act * 0.34;
                stroke_curve(dl, pts, width, base_col.with_alpha(alpha), glow, dashed);
            } else {
                stroke_curve(dl, pts, 1.1, base_col.with_alpha(base_alpha), 0.0, dashed);
            }
        }
        EdgeTier::Highlighted => {
            let base_glow = if offscreen { 0.46 } else { 0.42 };
            if live {
                let halo_col = base_col.with_alpha(0.14 + act * 0.18);
                stroke_curve(
                    dl,
                    pts,
                    2.4 + act * 2.2,
                    halo_col,
                    0.32 + act * 0.24,
                    dashed,
                );
                let core = base_col.mix(theme.text.primary, act * 0.16);
                stroke_curve(
                    dl,
                    pts,
                    2.2 + act * 0.55,
                    core,
                    base_glow + act * 0.24,
                    dashed,
                );
            } else {
                stroke_curve(dl, pts, 2.2, base_col, base_glow, dashed);
            }
        }
        EdgeTier::Selected => {
            // Soft outer luminous halo + bright core stroke so focused cables pop cleanly
            // across both dark phosphor and studio-light themes.
            let halo_alpha = if live { 0.32 + act * 0.16 } else { 0.32 };
            let halo = base_col
                .mix(theme.accent.warning_key, 0.35)
                .with_alpha(halo_alpha);
            let core = base_col.mix(
                theme.text.primary,
                if live { 0.30 + act * 0.14 } else { 0.30 },
            );
            let halo_w = if live { 5.2 + act * 1.6 } else { 5.2 };
            let core_glow = if live { 0.72 + act * 0.22 } else { 0.72 };
            stroke_curve(dl, pts, halo_w, halo, 0.55 + act * 0.15, dashed);
            stroke_curve(dl, pts, 2.8 + act * 0.35, core, core_glow, dashed);
        }
    }
}

fn stroke_curve(
    dl: &mut DisplayList,
    pts: &[(f32, f32)],
    width: f32,
    color: Color,
    glow: f32,
    dashed: bool,
) {
    if !dashed {
        dl.polyline(pts, width, color, glow, None);
    } else {
        draw_dashed_polyline(dl, pts, width, color, glow, DASH_LEN_PX, DASH_GAP_PX);
    }
}

/// Draw an arc-length-parameterized dashed polyline along `pts` so cubic-Bezier curves
/// maintain even dash spacing regardless of curvature.
fn draw_dashed_polyline(
    dl: &mut DisplayList,
    pts: &[(f32, f32)],
    width: f32,
    color: Color,
    glow: f32,
    dash_len: f32,
    gap_len: f32,
) {
    if pts.len() < 2 {
        return;
    }
    if dash_len <= 0.0 || gap_len <= 0.0 {
        dl.polyline(pts, width, color, glow, None);
        return;
    }

    let mut in_dash = true;
    let mut remaining = dash_len;
    let mut current_dash: Vec<(f32, f32)> = Vec::with_capacity(8);
    current_dash.push(pts[0]);

    for seg in pts.windows(2) {
        let a = seg[0];
        let b = seg[1];
        let dx = b.0 - a.0;
        let dy = b.1 - a.1;
        let mut seg_len = dx.hypot(dy);
        if seg_len < 1e-4 {
            continue;
        }
        let ux = dx / seg_len;
        let uy = dy / seg_len;
        let mut cur = a;

        while seg_len > 1e-4 {
            if seg_len >= remaining {
                let step = remaining;
                let next = (cur.0 + ux * step, cur.1 + uy * step);
                if in_dash {
                    current_dash.push(next);
                    if current_dash.len() >= 2 {
                        dl.polyline(&current_dash, width, color, glow, None);
                    }
                    current_dash.clear();
                    in_dash = false;
                    remaining = gap_len;
                } else {
                    in_dash = true;
                    remaining = dash_len;
                    current_dash.clear();
                    current_dash.push(next);
                }
                cur = next;
                seg_len -= step;
            } else {
                if in_dash {
                    current_dash.push(b);
                }
                remaining -= seg_len;
                seg_len = 0.0;
            }
        }
    }

    if in_dash && current_dash.len() >= 2 {
        dl.polyline(&current_dash, width, color, glow, None);
    }
}

fn draw_node_cards(
    dl: &mut DisplayList,
    columns: &[CableColumn<'_>],
    node_slots: &[Vec<NodeSlot>],
    theme: &Theme,
) {
    let name_cw = theme.char_width(theme.font.small);
    let sub_cw = theme.char_width(theme.font.small - 1.5);
    for (c, col) in columns.iter().enumerate() {
        for (r, node) in col.nodes.iter().enumerate() {
            let NodeSlot::Visible(rect) = node_slots[c][r] else {
                continue;
            };
            let accent = if node.bypassed {
                theme.text.muted
            } else {
                node.accent
            };
            let bg = if node.selected {
                theme.surface.elevated.mix(accent, 0.24)
            } else if node.in_path {
                theme.surface.elevated
            } else {
                theme.surface.recessed
            };
            let border = if node.selected {
                theme.accent.warning_key
            } else if node.in_path {
                accent.with_alpha(0.85)
            } else {
                theme.border.subtle
            };
            let bw = if node.selected {
                1.8
            } else if node.in_path {
                1.2
            } else {
                1.0
            };
            let glow = if node.selected {
                0.32
            } else if node.in_path {
                0.14
            } else {
                0.0
            };
            dl.panel(rect, bg, border, theme.radii.pill, bw, glow);

            // Colored vertical identity bar on card's left edge.
            dl.rounded(
                Rect::new(rect.x + 2.0, rect.y + 3.0, 3.0, (rect.h - 6.0).max(4.0)),
                if node.in_path || node.selected {
                    accent
                } else {
                    accent.with_alpha(0.45)
                },
                1.5,
            );

            // Live activity meter shimmer along bottom edge.
            let meter_fill = if node.activity.is_finite() {
                node.activity.clamp(0.0, 1.0)
            } else {
                0.0
            };
            let bar_fill = if node.clipped && meter_fill <= 0.02 {
                1.0
            } else {
                meter_fill
            };
            if bar_fill > 0.02 {
                let mw = (rect.w - 10.0).max(0.0) * bar_fill.clamp(0.0, 1.0);
                let bar_col = if node.clipped {
                    theme.meter.clip
                } else if node.hot {
                    theme.meter.hot
                } else {
                    theme.accent.playhead
                };
                dl.rounded_glow(
                    Rect::new(rect.x + 6.0, rect.bottom() - 3.5, mw, 2.0),
                    bar_col,
                    1.0,
                    if node.clipped { theme.glow.clip } else { 0.28 },
                );
            }

            let content = Rect::new(rect.x + 8.0, rect.y + 2.0, rect.w - 12.0, rect.h - 5.0);
            if content.h >= 26.0 {
                let (top_r, bot_r) = content.split_top(content.h * 0.54);
                let max_name = (top_r.w / name_cw) as usize;
                dl.text(
                    top_r,
                    trim_label(node.title, max_name),
                    theme.font.small,
                    node.selected || node.in_path,
                    if node.selected || node.in_path {
                        theme.text.primary
                    } else {
                        theme.text.secondary
                    },
                    TextAlign::Left,
                );
                let sub = if node.detail.is_empty() {
                    node.badge.to_owned()
                } else if node.badge.is_empty() {
                    node.detail.to_owned()
                } else {
                    format!("{} · {}", node.badge, node.detail)
                };
                let max_sub = (bot_r.w / sub_cw) as usize;
                dl.text(
                    bot_r,
                    trim_label(&sub, max_sub),
                    theme.font.small - 1.5,
                    false,
                    if node.selected {
                        theme.accent.warning_key
                    } else if node.in_path {
                        accent
                    } else {
                        theme.text.muted
                    },
                    TextAlign::Left,
                );
            } else {
                let lbl = if node.badge.is_empty() {
                    node.title.to_owned()
                } else {
                    format!("{} · {}", node.title, node.badge)
                };
                let max_c = (content.w / name_cw) as usize;
                dl.text(
                    content,
                    trim_label(&lbl, max_c),
                    theme.font.small - 0.5,
                    node.selected,
                    if node.selected || node.in_path {
                        theme.text.primary
                    } else {
                        theme.text.secondary
                    },
                    TextAlign::Left,
                );
            }
        }
    }
}

fn draw_scroll_indicators(
    dl: &mut DisplayList,
    col_layouts: &[ColLayout],
    tracker: &OffscreenTracker,
    theme: &Theme,
) {
    let ind_font = theme.font.small - 1.5;
    let ind_cw = theme.char_width(ind_font);
    for (c, lay) in col_layouts.iter().enumerate() {
        for (count, strip, hl_col, has_dim, prefix) in [
            (
                lay.above_count,
                lay.top_strip,
                tracker.strip_hl_above[c],
                tracker.strip_has_dim_above[c],
                "▲",
            ),
            (
                lay.below_count,
                lay.bot_strip,
                tracker.strip_hl_below[c],
                tracker.strip_has_dim_below[c],
                "▼",
            ),
        ] {
            if count == 0 || strip.h < 10.0 {
                continue;
            }
            let (bg, border, fg, bw, glow, bold) = if let Some(accent) = hl_col {
                (
                    accent.with_alpha(0.22),
                    accent,
                    theme.text.primary,
                    1.2,
                    0.30,
                    true,
                )
            } else if has_dim {
                (
                    theme.surface.elevated.with_alpha(0.92),
                    theme.border.hairline,
                    theme.text.secondary,
                    1.0,
                    0.0,
                    false,
                )
            } else {
                (
                    theme.surface.recessed.with_alpha(0.86),
                    theme.border.subtle,
                    theme.text.muted,
                    1.0,
                    0.0,
                    false,
                )
            };
            dl.panel(strip, bg, border, theme.radii.badge, bw, glow);
            let label = if prefix == "▲" {
                format!("▲ {count} above")
            } else {
                format!("▼ {count} below")
            };
            let max_c = ((strip.w - 10.0) / ind_cw).max(1.0) as usize;
            dl.text(
                strip.inset(4.0, 0.0),
                trim_label(&label, max_c),
                ind_font,
                bold,
                fg,
                TextAlign::Center,
            );
        }
    }
}

fn draw_port_jacks(dl: &mut DisplayList, tracker: &OffscreenTracker, theme: &Theme) {
    for stub in tracker.port_stubs.iter().filter(|s| !s.highlighted) {
        let (px, py) = stub.pos;
        dl.rounded(
            Rect::new(px - 2.0, py - 2.0, 4.0, 4.0),
            stub.color.with_alpha(0.38),
            2.0,
        );
    }
    for jack in tracker.visible_jacks.iter().filter(|j| !j.selected) {
        let (vx, vy) = jack.pos;
        dl.rounded_glow(
            Rect::new(vx - 3.0, vy - 3.0, 6.0, 6.0),
            jack.color,
            3.0,
            0.45,
        );
    }
    for jack in tracker.visible_jacks.iter().filter(|j| j.selected) {
        let (vx, vy) = jack.pos;
        dl.rounded_glow(
            Rect::new(vx - 4.0, vy - 4.0, 8.0, 8.0),
            theme.accent.warning_key,
            4.0,
            0.65,
        );
        dl.rounded(
            Rect::new(vx - 2.0, vy - 2.0, 4.0, 4.0),
            theme.text.primary,
            2.0,
        );
    }
    for stub in tracker.port_stubs.iter().filter(|s| s.highlighted) {
        let (px, py) = stub.pos;
        // Subtle directional conduit tick toward the offscreen edge + glowing port socket.
        let tick_y = if stub.above { py - 5.5 } else { py + 1.5 };
        let socket_col = if stub.selected {
            theme.accent.warning_key
        } else {
            stub.color
        };
        dl.rounded_glow(Rect::new(px - 1.0, tick_y, 2.0, 4.0), socket_col, 1.0, 0.48);
        dl.rounded_glow(
            Rect::new(px - 3.5, py - 3.5, 7.0, 7.0),
            socket_col,
            3.5,
            if stub.selected { 0.68 } else { 0.55 },
        );
        dl.rounded(
            Rect::new(px - 1.5, py - 1.5, 3.0, 3.0),
            theme.text.primary,
            1.5,
        );
    }
}

fn draw_cable_labels(
    dl: &mut DisplayList,
    canvas: Rect,
    col_layouts: &[ColLayout],
    node_slots: &[Vec<NodeSlot>],
    pending_labels: &[PendingLabel<'_>],
    theme: &Theme,
) {
    if canvas.w < 40.0 || canvas.h < 24.0 {
        return;
    }

    // Collect visible node cards & scroll strips as label obstacles so midpoint badges
    // naturally settle into open cable gutters between columns.
    let mut obstacles: Vec<Rect> = Vec::new();
    for col in node_slots {
        for &slot in col {
            if let NodeSlot::Visible(r) = slot {
                obstacles.push(r);
            }
        }
    }
    for lay in col_layouts {
        if lay.above_count > 0 && lay.top_strip.h >= 10.0 {
            obstacles.push(lay.top_strip);
        }
        if lay.below_count > 0 && lay.bot_strip.h >= 10.0 {
            obstacles.push(lay.bot_strip);
        }
    }

    let pill_font = (theme.font.small - 2.0).max(8.0);
    let pill_cw = theme.char_width(pill_font);
    let pill_h = 15.0_f32;
    let bounds = canvas.inset(2.0, 2.0);
    let max_pill_w = (bounds.w - 4.0).clamp(24.0, 124.0);
    let mut placed_pills: Vec<Rect> = Vec::new();

    for tier in [EdgeTier::Selected, EdgeTier::Highlighted, EdgeTier::Dim] {
        for pending in pending_labels.iter().filter(|l| l.tier == tier) {
            let shown = trim_label(pending.text, 16);
            let pill_w = (shown.chars().count() as f32 * pill_cw + 10.0).clamp(24.0, max_pill_w);
            let make_pill = |(mx, my): (f32, f32)| {
                let max_x = (bounds.right() - pill_w).max(bounds.x);
                let max_y = (bounds.bottom() - pill_h).max(bounds.y);
                Rect::new(
                    (mx - pill_w * 0.5).clamp(bounds.x, max_x),
                    (my - pill_h * 0.5).clamp(bounds.y, max_y),
                    pill_w,
                    pill_h,
                )
            };

            let hits_pill =
                |r: Rect, placed: &[Rect]| placed.iter().any(|&p| rects_overlap_padded(r, p, 2.0));
            let hits_obstacle =
                |r: Rect| obstacles.iter().any(|&o| rects_overlap_padded(r, o, 1.5));

            // 1. Prefer a candidate directly on the cable completely clear of both cards and pills.
            let mut chosen = LABEL_CANDIDATE_FRACS.iter().find_map(|&frac| {
                let r = make_pill(polyline_point_at_frac(&pending.pts, frac));
                (!hits_pill(r, &placed_pills) && !hits_obstacle(r)).then_some(r)
            });

            // 2. For highlighted/selected cables, try a small vertical nudge inside open gutters
            //    so parallel wires in the same column gap stack cleanly without touching node cards.
            if chosen.is_none() && tier != EdgeTier::Dim {
                chosen = LABEL_CANDIDATE_FRACS.iter().find_map(|&frac| {
                    let (mx, my) = polyline_point_at_frac(&pending.pts, frac);
                    [9.0_f32, -9.0_f32].into_iter().find_map(|dy| {
                        let r = make_pill((mx, my + dy));
                        (!hits_pill(r, &placed_pills) && !hits_obstacle(r)).then_some(r)
                    })
                });
            }

            // 3. Selected cable (actively focused for editing) always renders even on ultra-tight
            //    layouts where no obstacle-free gutter exists.
            if chosen.is_none() && tier == EdgeTier::Selected {
                chosen = LABEL_CANDIDATE_FRACS
                    .iter()
                    .find_map(|&frac| {
                        let r = make_pill(polyline_point_at_frac(&pending.pts, frac));
                        (!hits_pill(r, &placed_pills)).then_some(r)
                    })
                    .or_else(|| Some(make_pill(polyline_point_at_frac(&pending.pts, 0.50))));
            }

            let Some(pill_rect) = chosen else {
                continue;
            };

            let (bg, border, fg, bw, glow, bold) = match tier {
                EdgeTier::Selected => (
                    theme.surface.elevated.mix(pending.color, 0.28),
                    theme.accent.warning_key,
                    theme.text.primary,
                    1.4,
                    0.38,
                    true,
                ),
                EdgeTier::Highlighted => (
                    theme.surface.elevated.with_alpha(0.95),
                    pending.color.with_alpha(0.85),
                    theme.text.primary,
                    1.0,
                    0.22,
                    true,
                ),
                EdgeTier::Dim => (
                    theme.surface.recessed.with_alpha(0.88),
                    pending.color.with_alpha(0.42),
                    theme.text.secondary,
                    1.0,
                    0.0,
                    false,
                ),
            };
            dl.panel(pill_rect, bg, border, theme.radii.badge, bw, glow);
            let inner_chars = ((pill_rect.w - 6.0) / pill_cw).floor().max(1.0) as usize;
            dl.text(
                pill_rect.inset(3.0, 0.0),
                trim_label(&shown, inner_chars),
                pill_font,
                bold,
                fg,
                TextAlign::Center,
            );
            placed_pills.push(pill_rect);
        }
    }
}

fn port_y_clamped(y: f32) -> f32 {
    if y.is_finite() {
        y.clamp(0.18, 0.82)
    } else {
        0.5
    }
}

fn rects_overlap_padded(a: Rect, b: Rect, pad: f32) -> bool {
    a.x - pad < b.right()
        && a.right() + pad > b.x
        && a.y - pad < b.bottom()
        && a.bottom() + pad > b.y
}

fn polyline_point_at_frac(pts: &[(f32, f32)], frac: f32) -> (f32, f32) {
    if pts.is_empty() {
        return (0.0, 0.0);
    }
    if pts.len() == 1 {
        return pts[0];
    }
    let total_len: f32 = pts
        .windows(2)
        .map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1))
        .sum();
    if total_len <= 1e-4 {
        return pts[0];
    }
    let target = total_len * frac.clamp(0.0, 1.0);
    let mut acc = 0.0_f32;
    for seg in pts.windows(2) {
        let len = (seg[1].0 - seg[0].0).hypot(seg[1].1 - seg[0].1);
        if len > 1e-4 && acc + len >= target {
            let t = ((target - acc) / len).clamp(0.0, 1.0);
            return (
                seg[0].0 + (seg[1].0 - seg[0].0) * t,
                seg[0].1 + (seg[1].1 - seg[0].1) * t,
            );
        }
        acc += len;
    }
    *pts.last().unwrap_or(&pts[0])
}

fn sample_cubic(
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    p3: (f32, f32),
    segments: usize,
) -> Vec<(f32, f32)> {
    let n = segments.max(2);
    let mut out = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let u = 1.0 - t;
        let x =
            u * u * u * p0.0 + 3.0 * u * u * t * p1.0 + 3.0 * u * t * t * p2.0 + t * t * t * p3.0;
        let y =
            u * u * u * p0.1 + 3.0 * u * u * t * p1.1 + 3.0 * u * t * t * p2.1 + t * t * t * p3.1;
        out.push((x, y));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_eurorack_scene<'a>(
        theme: &Theme,
    ) -> (
        Vec<CableColumn<'a>>,
        Vec<CableEdge<'a>>,
        Vec<(&'a str, Color)>,
    ) {
        let mut vco1 = CableNode::new("VCO 1", "SAW", "0 st · 440 Hz", theme.accent.focus);
        vco1.selected = true;
        vco1.in_path = true;
        vco1.activity = audio_activity_norm(0.72);

        let mut noise = CableNode::new("Noise", "WHITE", "HP 1.2 kHz", theme.accent.focus);
        noise.activity = audio_activity_norm(0.25);

        let mut env_amp = CableNode::new(
            "Amp Env",
            "ADSR",
            "A 2ms · D 180ms",
            theme.accent.modulation,
        );
        env_amp.in_path = true;
        env_amp.activity = 0.65;

        let mut env_flt = CableNode::new(
            "Flt Env",
            "HOLD",
            "H 12ms · D 90ms",
            theme.accent.modulation,
        );
        env_flt.in_path = true;
        env_flt.activity = 0.80;

        let mut lfo = CableNode::new("LFO 1", "SINE", "3.2 Hz · Free", theme.accent.modulation);
        lfo.bypassed = true;

        let mut flt = CableNode::new("Filter", "LP12", "1.85 kHz · Q 0.42", theme.accent.routing);
        flt.in_path = true;
        flt.activity = audio_activity_norm(1.25);
        flt.hot = true;

        let mut drive = CableNode::new("Drive", "TAPE", "+6.0 dB · Asym", theme.accent.warning_key);
        drive.in_path = true;
        drive.activity = audio_activity_norm(0.88);

        let mut vca = CableNode::new("VCA", "AMP", "Gain 0.0 dB", theme.accent.playhead);
        vca.in_path = true;
        vca.activity = audio_activity_norm(0.76);

        let mut out = CableNode::new("Out", "STEREO", "+0.0 dB", theme.accent.playhead);
        out.in_path = true;
        out.activity = audio_activity_norm(0.70);

        let columns = vec![
            CableColumn::new("SOURCES", vec![vco1, noise]),
            CableColumn::new("MODULATORS", vec![env_amp, env_flt, lfo]),
            CableColumn::new("PROCESSORS", vec![flt, drive]),
            CableColumn::new("VCA / OUT", vec![vca, out]),
        ];

        let edges = vec![
            // VCO 1 -> Filter (audio, highlighted)
            CableEdge::new(0, 0, 2, 0, theme.accent.routing, true)
                .with_label(Some("0.0 dB"))
                .with_ports(0.45, 0.35),
            // Flt Env -> Filter.Cv (SELECTED cable to edit attenuverter!)
            CableEdge::new(1, 1, 2, 0, theme.accent.modulation, true)
                .with_label(Some("+23.2 st"))
                .with_selected(true)
                .with_ports(0.50, 0.68),
            // Filter -> Drive (same-column audio)
            CableEdge::new(2, 0, 2, 1, theme.accent.routing, true).with_label(Some("1.00")),
            // Drive -> VCA (audio)
            CableEdge::new(2, 1, 3, 0, theme.accent.routing, true).with_label(Some("-6 dB")),
            // Amp Env -> VCA.Cv (modulation)
            CableEdge::new(1, 0, 3, 0, theme.accent.modulation, true).with_label(Some("0.85")),
            // VCA -> Out (same-column audio)
            CableEdge::new(3, 0, 3, 1, theme.accent.playhead, true),
            // Filter -> VCO 1.FmIn (z-1 1-sample backward feedback cable, dashed!)
            CableEdge::new(2, 0, 0, 0, theme.accent.alert, true)
                .with_dashed(true)
                .with_label(Some("z-1 · 0.35")),
            // Noise -> VCA (dim background wire with label)
            CableEdge::new(0, 1, 3, 0, theme.accent.routing, false).with_label(Some("-12 dB")),
            // Self-feedback on VCO 1 (dashed self-loop)
            CableEdge::new(0, 0, 0, 0, theme.accent.warning_key, false)
                .with_dashed(true)
                .with_label(Some("z-1 · 0.15")),
        ];

        let legend = vec![
            ("AUDIO", theme.accent.routing),
            ("PITCH", theme.accent.warning_key),
            ("CV / MOD", theme.accent.modulation),
            ("FEEDBACK z-1", theme.accent.alert),
            ("OUT", theme.accent.playhead),
        ];

        (columns, edges, legend)
    }

    #[test]
    fn clip_stack_stays_strictly_balanced_for_normal_and_tiny_rects() {
        let theme = Theme::cadence_dark();
        let (cols, edges, legend) = sample_eurorack_scene(&theme);
        let graph = CableGraph::new(&cols, &edges, Some(0), 0);

        let mut dl = DisplayList::new(1000.0, 600.0, 1.0);
        let outer = Rect::new(20.0, 30.0, 960.0, 540.0);
        dl.push_clip(outer);
        assert_eq!(dl.current_clip(), outer);

        let (leg_rect, canvas) = outer.split_top(22.0);
        draw_cable_legend(
            &mut dl,
            leg_rect,
            &legend,
            Some("CABLE #2 · +23.2 st"),
            &theme,
        );
        assert_eq!(dl.current_clip(), outer, "legend left unbalanced clip");

        draw_cable_graph(&mut dl, canvas, &graph, &theme);
        assert_eq!(dl.current_clip(), outer, "cable graph left unbalanced clip");

        // Tiny / empty rects must also keep clip stack untouched.
        draw_cable_legend(
            &mut dl,
            Rect::new(0.0, 0.0, 0.0, 0.0),
            &legend,
            None,
            &theme,
        );
        draw_cable_graph(&mut dl, Rect::new(0.0, 0.0, 20.0, 10.0), &graph, &theme);
        assert_eq!(dl.current_clip(), outer);

        dl.pop_clip();
        assert_eq!(dl.current_clip(), dl.viewport);
    }

    #[test]
    fn dashed_and_selected_edges_emit_expected_geometry_and_midpoint_pills() {
        let theme = Theme::cadence_dark();
        let (cols, edges, _) = sample_eurorack_scene(&theme);
        let graph = CableGraph::new(&cols, &edges, Some(0), 0);

        let mut dl = DisplayList::new(1100.0, 520.0, 1.0);
        draw_cable_graph(&mut dl, Rect::new(0.0, 0.0, 1100.0, 520.0), &graph, &theme);

        let texts: Vec<&str> = dl.base.texts.iter().map(|t| t.text.as_str()).collect();
        // Selected & highlighted cable pills must be emitted crisply.
        assert!(
            texts.contains(&"+23.2 st"),
            "missing selected cable attenuverter pill in {texts:?}"
        );
        assert!(
            texts.contains(&"0.85"),
            "missing highlighted cable pill `0.85` in {texts:?}"
        );
        assert!(
            texts.contains(&"-6 dB"),
            "missing highlighted cable pill `-6 dB` in {texts:?}"
        );
        assert!(
            texts.contains(&"z-1 · 0.35"),
            "missing dashed feedback cable pill in {texts:?}"
        );

        // Verify cable label pills never overlap any visible node card (height 26..=46px).
        let card_rects: Vec<Rect> = dl
            .base
            .quads
            .iter()
            .filter(|q| q.rect[2] > 100.0 && q.rect[3] >= 25.0 && q.rect[3] <= 47.0)
            .map(|q| Rect::new(q.rect[0], q.rect[1], q.rect[2], q.rect[3]))
            .collect();
        let pill_rects: Vec<Rect> = dl
            .base
            .quads
            .iter()
            .filter(|q| (q.rect[3] - 15.0).abs() < 0.01)
            .map(|q| Rect::new(q.rect[0], q.rect[1], q.rect[2], q.rect[3]))
            .collect();
        assert!(
            pill_rects.len() >= 5,
            "expected at least 5 cable pills, got {}",
            pill_rects.len()
        );
        for &pill in &pill_rects {
            for &card in &card_rects {
                assert!(
                    !rects_overlap_padded(pill, card, 0.0),
                    "cable pill {pill:?} overlapped node card {card:?}"
                );
            }
        }

        // Compare a single solid cable vs a dashed cable to verify dashed emits multiple segments
        // and strictly fewer continuous-fill vertices (gaps removed).
        let simple_cols = vec![
            CableColumn::new(
                "A",
                vec![CableNode::new("VCO", "SAW", "", theme.accent.focus)],
            ),
            CableColumn::new(
                "B",
                vec![CableNode::new("FLT", "LP12", "", theme.accent.routing)],
            ),
        ];
        let solid_edge = [CableEdge::new(0, 0, 1, 0, theme.accent.routing, true)];
        let dashed_edge = [CableEdge::new(0, 0, 1, 0, theme.accent.alert, true).with_dashed(true)];
        let selected_edge =
            [CableEdge::new(0, 0, 1, 0, theme.accent.modulation, false).with_selected(true)];

        let mut dl_solid = DisplayList::new(600.0, 260.0, 1.0);
        let mut dl_dashed = DisplayList::new(600.0, 260.0, 1.0);
        let mut dl_selected = DisplayList::new(600.0, 260.0, 1.0);
        let rect = Rect::new(0.0, 0.0, 600.0, 260.0);

        draw_cable_graph(
            &mut dl_solid,
            rect,
            &CableGraph::new(&simple_cols, &solid_edge, Some(0), 0),
            &theme,
        );
        draw_cable_graph(
            &mut dl_dashed,
            rect,
            &CableGraph::new(&simple_cols, &dashed_edge, Some(0), 0),
            &theme,
        );
        draw_cable_graph(
            &mut dl_selected,
            rect,
            &CableGraph::new(&simple_cols, &selected_edge, Some(0), 0),
            &theme,
        );

        assert!(!dl_dashed.base.curve_vertices.is_empty());
        assert_ne!(
            dl_solid.base.curve_vertices.len(),
            dl_dashed.base.curve_vertices.len(),
            "dashed cable should emit segmented dash geometry distinct from solid cable"
        );
        // Selected cable draws halo + core (= 2x solid cable curve vertices) even when highlighted=false!
        assert_eq!(
            dl_selected.base.curve_vertices.len(),
            dl_solid.base.curve_vertices.len() * 2,
            "selected cable should render both halo and bright core curves"
        );
    }

    #[test]
    fn bypassed_node_mutes_accent_to_theme_muted() {
        let theme = Theme::cadence_dark();
        let mut active = CableNode::new("LFO 1", "SINE", "2 Hz", theme.accent.modulation);
        active.in_path = true;
        let mut bypassed = active;
        bypassed.bypassed = true;

        let cols = vec![CableColumn::new("MODULATORS", vec![active, bypassed])];
        let graph = CableGraph::new(&cols, &[], Some(0), 0);

        let mut dl = DisplayList::new(400.0, 300.0, 1.0);
        draw_cable_graph(&mut dl, Rect::new(0.0, 0.0, 400.0, 300.0), &graph, &theme);

        // Left identity bars have width == 3.0.
        let bars: Vec<_> = dl
            .base
            .quads
            .iter()
            .filter(|q| (q.rect[2] - 3.0).abs() < 0.01)
            .collect();
        assert_eq!(bars.len(), 2);
        let mod_rgba = theme.accent.modulation.to_linear();
        let muted_rgba = theme.text.muted.to_linear();
        assert!((bars[0].color_top[0] - mod_rgba[0]).abs() < 1e-4);
        assert!((bars[1].color_top[0] - muted_rgba[0]).abs() < 1e-4);
    }

    #[test]
    fn offscreen_dashed_and_selected_cables_emit_strips_stubs_and_pills() {
        let theme = Theme::cadence_dark();
        let col0: Vec<CableNode<'_>> = (0..18)
            .map(|i| {
                let mut n = CableNode::new("Mod", "VCO", "440 Hz", theme.accent.focus);
                n.selected = i == 9;
                n.in_path = i == 0 || i == 9 || i == 17;
                n
            })
            .collect();
        let mut out = CableNode::new("Filter", "LP12", "1.2 kHz", theme.accent.routing);
        out.in_path = true;
        let cols = vec![
            CableColumn::new("SOURCES", col0),
            CableColumn::new("PROCESSORS", vec![out]),
        ];
        let edges = vec![
            // Selected offscreen-above cable with attenuverter label
            CableEdge::new(0, 0, 1, 0, theme.accent.modulation, true)
                .with_selected(true)
                .with_label(Some("+12.0 st")),
            // Dashed offscreen-below feedback cable with label
            CableEdge::new(1, 0, 0, 17, theme.accent.alert, true)
                .with_dashed(true)
                .with_label(Some("z-1 · 0.40")),
        ];
        let graph = CableGraph::new(&cols, &edges, Some(0), 9);

        let mut dl = DisplayList::new(800.0, 360.0, 1.0);
        draw_cable_graph(&mut dl, Rect::new(0.0, 0.0, 800.0, 360.0), &graph, &theme);

        let texts: Vec<&str> = dl.base.texts.iter().map(|t| t.text.as_str()).collect();
        assert!(texts
            .iter()
            .any(|t| t.starts_with('▲') && t.contains("above")));
        assert!(texts
            .iter()
            .any(|t| t.starts_with('▼') && t.contains("below")));
        assert!(
            texts.contains(&"+12.0 st"),
            "missing selected offscreen cable pill in {texts:?}"
        );
        assert!(
            texts.contains(&"z-1 · 0.40"),
            "missing dashed offscreen feedback cable pill in {texts:?}"
        );
        assert!(dl.base.curve_vertices.len() > 100);
    }
}
