//! 2D geometry, scissor stack, and GPU-ready batched `DisplayList`.
use crate::theme::Color;
use bytemuck::{Pod, Zeroable};

/// Style parameters for [`DisplayList::quad_full`].
#[derive(Clone, Copy, Debug)]
pub struct QuadStyle {
    pub color_top: Color,
    pub color_bottom: Color,
    pub radius: f32,
    pub border_color: Color,
    pub border_width: f32,
    pub emissive: f32,
    pub hatch_period: f32,
}

impl QuadStyle {
    pub fn solid(color: Color) -> Self {
        Self {
            color_top: color,
            color_bottom: color,
            radius: 0.0,
            border_color: color.with_alpha(0.0),
            border_width: 0.0,
            emissive: 0.0,
            hatch_period: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(self) -> f32 {
        self.y + self.h
    }

    pub fn is_empty(self) -> bool {
        self.w <= 0.5 || self.h <= 0.5
    }

    pub fn inset(self, dx: f32, dy: f32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            w: (self.w - dx * 2.0).max(0.0),
            h: (self.h - dy * 2.0).max(0.0),
        }
    }

    pub fn split_top(self, height: f32) -> (Self, Self) {
        let h = height.clamp(0.0, self.h);
        (
            Self::new(self.x, self.y, self.w, h),
            Self::new(self.x, self.y + h, self.w, (self.h - h).max(0.0)),
        )
    }

    pub fn split_bottom(self, height: f32) -> (Self, Self) {
        let h = height.clamp(0.0, self.h);
        let top_h = (self.h - h).max(0.0);
        (
            Self::new(self.x, self.y, self.w, top_h),
            Self::new(self.x, self.y + top_h, self.w, h),
        )
    }

    pub fn split_left(self, width: f32) -> (Self, Self) {
        let w = width.clamp(0.0, self.w);
        (
            Self::new(self.x, self.y, w, self.h),
            Self::new(self.x + w, self.y, (self.w - w).max(0.0), self.h),
        )
    }

    pub fn split_right(self, width: f32) -> (Self, Self) {
        let w = width.clamp(0.0, self.w);
        let left_w = (self.w - w).max(0.0);
        (
            Self::new(self.x, self.y, left_w, self.h),
            Self::new(self.x + left_w, self.y, w, self.h),
        )
    }

    pub fn intersect(self, other: Self) -> Self {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self.right().min(other.right());
        let y1 = self.bottom().min(other.bottom());
        Self::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
    }

    pub fn scale(self, factor: f32) -> Self {
        Self::new(
            self.x * factor,
            self.y * factor,
            self.w * factor,
            self.h * factor,
        )
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct QuadInstance {
    pub rect: [f32; 4],
    pub color_top: [f32; 4],
    pub color_bottom: [f32; 4],
    pub border_color: [f32; 4],
    /// `[radius_px, border_width_px, emissive_weight, hatch_period_px]`
    pub params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct CurveVertex {
    pub pos: [f32; 2],
    /// Signed perpendicular offset in `[-1.0, 1.0]` for line AA, or `0.0` for fill triangles.
    pub edge: f32,
    /// Half stroke width in pixels (`<= 0.0` signals a solid/gradient area fill vertex).
    pub half_width: f32,
    pub color: [f32; 4],
    pub emissive: f32,
    pub _pad: [f32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug)]
pub struct TextRun {
    pub rect: Rect,
    pub text: String,
    pub size: f32,
    pub bold: bool,
    pub color: Color,
    pub align: TextAlign,
    pub clip: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawBatch {
    pub clip: Rect,
    pub start: u32,
    pub count: u32,
}

/// Ordered GPU draw command preserving exact painter's order across quads and curves
/// while coalescing adjacent same-kind, same-clip batches.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DrawCmd {
    Quads(DrawBatch),
    Curves(DrawBatch),
}

#[derive(Default)]
pub struct DisplayLayer {
    pub quads: Vec<QuadInstance>,
    pub curve_vertices: Vec<CurveVertex>,
    pub commands: Vec<DrawCmd>,
    pub texts: Vec<TextRun>,
    pub emissive_count: u32,
}

impl DisplayLayer {
    pub fn is_empty(&self) -> bool {
        self.quads.is_empty() && self.curve_vertices.is_empty() && self.texts.is_empty()
    }

    /// Clear all geometry and text runs while retaining allocated `Vec` capacities.
    pub fn clear(&mut self) {
        self.quads.clear();
        self.curve_vertices.clear();
        self.commands.clear();
        self.texts.clear();
        self.emissive_count = 0;
    }

    fn push_quad_cmd(&mut self, clip: Rect, start: u32) {
        if let Some(DrawCmd::Quads(last)) = self.commands.last_mut() {
            if last.clip == clip && last.start + last.count == start {
                last.count += 1;
                return;
            }
        }
        self.commands.push(DrawCmd::Quads(DrawBatch {
            clip,
            start,
            count: 1,
        }));
    }

    fn push_curve_cmd(&mut self, clip: Rect, start: u32, count: u32) {
        if count == 0 {
            return;
        }
        if let Some(DrawCmd::Curves(last)) = self.commands.last_mut() {
            if last.clip == clip && last.start + last.count == start {
                last.count += count;
                return;
            }
        }
        self.commands
            .push(DrawCmd::Curves(DrawBatch { clip, start, count }));
    }
}

/// Ordered GPU display list built cleanly each frame in logical coordinates and
/// scaled seamlessly to physical pixels at upload time. Split into `base` (decks
/// & chrome) and `overlay` (modal popups rendered strictly on top of base text).
pub struct DisplayList {
    pub scale: f32,
    pub viewport: Rect,
    clip_stack: Vec<Rect>,
    in_overlay: bool,
    polyline_scratch: Vec<(f32, f32)>,
    pub base: DisplayLayer,
    pub overlay: DisplayLayer,
}

impl DisplayList {
    pub fn new(logical_width: f32, logical_height: f32, scale: f32) -> Self {
        let viewport = Rect::new(0.0, 0.0, logical_width, logical_height);
        Self {
            scale: scale.max(1.0),
            viewport,
            clip_stack: vec![viewport],
            in_overlay: false,
            polyline_scratch: Vec::with_capacity(256),
            base: DisplayLayer {
                quads: Vec::with_capacity(2048),
                curve_vertices: Vec::with_capacity(4096),
                commands: Vec::with_capacity(96),
                texts: Vec::with_capacity(512),
                emissive_count: 0,
            },
            overlay: DisplayLayer::default(),
        }
    }

    /// Clear all recorded commands, geometry, and text while preserving `Vec` capacities.
    pub fn clear(&mut self) {
        self.in_overlay = false;
        self.clip_stack.clear();
        self.clip_stack.push(self.viewport);
        self.polyline_scratch.clear();
        self.base.clear();
        self.overlay.clear();
    }

    /// Reset viewport dimensions and scale for a new frame while retaining all `Vec` capacities.
    pub fn reset(&mut self, logical_width: f32, logical_height: f32, scale: f32) {
        self.scale = scale.max(1.0);
        self.viewport = Rect::new(0.0, 0.0, logical_width, logical_height);
        self.clear();
    }

    pub fn begin_overlay(&mut self) {
        self.in_overlay = true;
        self.clip_stack.clear();
        self.clip_stack.push(self.viewport);
    }

    fn layer_mut(&mut self) -> &mut DisplayLayer {
        if self.in_overlay {
            &mut self.overlay
        } else {
            &mut self.base
        }
    }

    pub fn current_clip(&self) -> Rect {
        *self.clip_stack.last().unwrap_or(&self.viewport)
    }

    pub fn push_clip(&mut self, rect: Rect) {
        let clipped = self.current_clip().intersect(rect);
        self.clip_stack.push(clipped);
    }

    pub fn pop_clip(&mut self) {
        if self.clip_stack.len() > 1 {
            self.clip_stack.pop();
        }
    }

    /// Draw a solid or gradient rounded rectangle with optional border, hatch pattern, and emissive glow.
    pub fn quad_full(&mut self, rect: Rect, style: QuadStyle) {
        if rect.is_empty() {
            return;
        }
        let clip = self.current_clip();
        if clip.is_empty() {
            return;
        }
        let s = self.scale;
        let phys_clip = clip.scale(s);
        let layer = self.layer_mut();
        let idx = layer.quads.len() as u32;
        if style.emissive > 0.0 {
            layer.emissive_count = layer.emissive_count.saturating_add(1);
        }
        layer.quads.push(QuadInstance {
            rect: [rect.x * s, rect.y * s, rect.w * s, rect.h * s],
            color_top: style.color_top.to_linear(),
            color_bottom: style.color_bottom.to_linear(),
            border_color: style.border_color.to_linear(),
            params: [
                style.radius * s,
                style.border_width * s,
                style.emissive,
                style.hatch_period * s,
            ],
        });
        layer.push_quad_cmd(phys_clip, idx);
    }

    pub fn fill(&mut self, rect: Rect, color: Color) {
        self.quad_full(rect, QuadStyle::solid(color));
    }

    pub fn rounded(&mut self, rect: Rect, color: Color, radius: f32) {
        self.quad_full(
            rect,
            QuadStyle {
                radius,
                ..QuadStyle::solid(color)
            },
        );
    }

    pub fn rounded_glow(&mut self, rect: Rect, color: Color, radius: f32, emissive: f32) {
        self.quad_full(
            rect,
            QuadStyle {
                radius,
                emissive,
                ..QuadStyle::solid(color)
            },
        );
    }

    pub fn panel(
        &mut self,
        rect: Rect,
        fill: Color,
        border: Color,
        radius: f32,
        border_width: f32,
        emissive: f32,
    ) {
        self.quad_full(
            rect,
            QuadStyle {
                radius,
                border_color: border,
                border_width,
                emissive,
                ..QuadStyle::solid(fill)
            },
        );
    }

    pub fn hatched_band(
        &mut self,
        rect: Rect,
        color: Color,
        border: Color,
        radius: f32,
        hatch_period: f32,
        emissive: f32,
    ) {
        self.quad_full(
            rect,
            QuadStyle {
                radius,
                border_color: border,
                border_width: 1.0,
                emissive,
                hatch_period,
                ..QuadStyle::solid(color)
            },
        );
    }

    pub fn gradient_v(
        &mut self,
        rect: Rect,
        top: Color,
        bottom: Color,
        radius: f32,
        emissive: f32,
    ) {
        self.quad_full(
            rect,
            QuadStyle {
                color_top: top,
                color_bottom: bottom,
                radius,
                emissive,
                ..QuadStyle::solid(top)
            },
        );
    }

    /// Draw an antialiased GPU polyline with bevel/miter vertex joins and optional
    /// translucent gradient fill to `baseline_y`.
    pub fn polyline(
        &mut self,
        points: &[(f32, f32)],
        width: f32,
        color: Color,
        emissive: f32,
        fill_to_y: Option<(f32, f32)>,
    ) {
        if points.len() < 2 {
            return;
        }
        let clip = self.current_clip();
        if clip.is_empty() {
            return;
        }
        let s = self.scale;
        let phys_clip = clip.scale(s);

        // Filter consecutive near-duplicate points into reusable physical-pixel scratch buffer.
        self.polyline_scratch.clear();
        for &(x, y) in points {
            let p = (x * s, y * s);
            if let Some(&(lx, ly)) = self.polyline_scratch.last() {
                let dx = p.0 - lx;
                let dy = p.1 - ly;
                if dx * dx + dy * dy < 1e-8 {
                    continue;
                }
            }
            self.polyline_scratch.push(p);
        }
        if self.polyline_scratch.len() < 2 {
            return;
        }

        let pts = self.polyline_scratch.as_slice();
        let layer = if self.in_overlay {
            &mut self.overlay
        } else {
            &mut self.base
        };
        let start = layer.curve_vertices.len() as u32;
        let linear = color.to_linear();

        // Optional translucent gradient fill under/to the baseline first.
        if let Some((base_y, fill_alpha)) = fill_to_y {
            let top_col = color.with_alpha(fill_alpha).to_linear();
            let bot_col = color.with_alpha(0.02).to_linear();
            let by = base_y * s;
            for seg in pts.windows(2) {
                let (x0, y0) = seg[0];
                let (x1, y1) = seg[1];
                let v0 = CurveVertex {
                    pos: [x0, y0],
                    edge: 0.0,
                    half_width: 0.0,
                    color: top_col,
                    emissive: emissive * 0.25,
                    _pad: [0.0; 3],
                };
                let v1 = CurveVertex {
                    pos: [x1, y1],
                    edge: 0.0,
                    half_width: 0.0,
                    color: top_col,
                    emissive: emissive * 0.25,
                    _pad: [0.0; 3],
                };
                let b0 = CurveVertex {
                    pos: [x0, by],
                    edge: 0.0,
                    half_width: 0.0,
                    color: bot_col,
                    emissive: 0.0,
                    _pad: [0.0; 3],
                };
                let b1 = CurveVertex {
                    pos: [x1, by],
                    edge: 0.0,
                    half_width: 0.0,
                    color: bot_col,
                    emissive: 0.0,
                    _pad: [0.0; 3],
                };
                layer
                    .curve_vertices
                    .extend_from_slice(&[v0, v1, b0, v1, b1, b0]);
            }
        }

        // Antialiased thick stroke quads with 1px AA feather and bowtie-safe miter/bevel joins.
        let half_w = (width * s * 0.5).max(0.75);
        let pad_w = half_w + 1.0;
        let edge_scale = pad_w / half_w;
        let seg_count = pts.len() - 1;

        let make_vert = |pos: [f32; 2], edge: f32| CurveVertex {
            pos,
            edge,
            half_width: half_w,
            color: linear,
            emissive,
            _pad: [0.0; 3],
        };

        let mut prev_frame: Option<SegFrame> = None;
        let mut curr_frame = seg_frame(pts[0], pts[1]);

        for i in 0..seg_count {
            let p0 = pts[i];
            let p1 = pts[i + 1];
            let next_frame = if i + 1 < seg_count {
                Some(seg_frame(pts[i + 1], pts[i + 2]))
            } else {
                None
            };

            // Determine start miter (or perpendicular start + bevel join from previous segment).
            let start_off = if let Some(prev) = prev_frame {
                if let Some(m) = compute_miter(prev, curr_frame, pad_w) {
                    m
                } else {
                    // Emit clean bevel join (or chamfered bevel cap on near-180° hairpins) at `p0`.
                    let dot = prev.tangent.0 * curr_frame.tangent.0
                        + prev.tangent.1 * curr_frame.tangent.1;
                    let cross = prev.tangent.0 * curr_frame.tangent.1
                        - prev.tangent.1 * curr_frame.tangent.0;
                    let vc = make_vert([p0.0, p0.1], 0.0);
                    if dot < -0.96 {
                        let tip = [p0.0 + prev.tangent.0 * pad_w, p0.1 + prev.tangent.1 * pad_w];
                        let vl = make_vert(
                            [p0.0 + prev.normal.0 * pad_w, p0.1 + prev.normal.1 * pad_w],
                            edge_scale,
                        );
                        let vr = make_vert(
                            [p0.0 - prev.normal.0 * pad_w, p0.1 - prev.normal.1 * pad_w],
                            -edge_scale,
                        );
                        let vtl = make_vert(tip, edge_scale);
                        let vtr = make_vert(tip, -edge_scale);
                        layer
                            .curve_vertices
                            .extend_from_slice(&[vc, vl, vtl, vc, vtr, vr]);
                    } else if cross.abs() > 1e-5 {
                        let outer_sign = if cross > 0.0 { -1.0 } else { 1.0 };
                        let edge = outer_sign * edge_scale;
                        let va = make_vert(
                            [
                                p0.0 + outer_sign * prev.normal.0 * pad_w,
                                p0.1 + outer_sign * prev.normal.1 * pad_w,
                            ],
                            edge,
                        );
                        let vb = make_vert(
                            [
                                p0.0 + outer_sign * curr_frame.normal.0 * pad_w,
                                p0.1 + outer_sign * curr_frame.normal.1 * pad_w,
                            ],
                            edge,
                        );
                        layer.curve_vertices.extend_from_slice(&[vc, va, vb]);
                    }
                    (curr_frame.normal.0 * pad_w, curr_frame.normal.1 * pad_w)
                }
            } else {
                (curr_frame.normal.0 * pad_w, curr_frame.normal.1 * pad_w)
            };

            // Determine end miter (or perpendicular end).
            let end_off = if let Some(next) = next_frame {
                compute_miter(curr_frame, next, pad_w)
                    .unwrap_or((curr_frame.normal.0 * pad_w, curr_frame.normal.1 * pad_w))
            } else {
                (curr_frame.normal.0 * pad_w, curr_frame.normal.1 * pad_w)
            };

            let a = make_vert([p0.0 + start_off.0, p0.1 + start_off.1], edge_scale);
            let b = make_vert([p0.0 - start_off.0, p0.1 - start_off.1], -edge_scale);
            let c = make_vert([p1.0 + end_off.0, p1.1 + end_off.1], edge_scale);
            let d = make_vert([p1.0 - end_off.0, p1.1 - end_off.1], -edge_scale);
            layer.curve_vertices.extend_from_slice(&[a, b, c, b, d, c]);

            prev_frame = Some(curr_frame);
            if let Some(nf) = next_frame {
                curr_frame = nf;
            }
        }

        let count = layer.curve_vertices.len() as u32 - start;
        if count == 0 {
            return;
        }
        if emissive > 0.0 {
            layer.emissive_count = layer.emissive_count.saturating_add(1);
        }
        layer.push_curve_cmd(phys_clip, start, count);
    }

    /// Draw a filled symmetric peak envelope (used by Sampler & Sample Browser waveforms).
    pub fn peak_envelope(&mut self, rect: Rect, peaks: &[(f32, f32)], color: Color, emissive: f32) {
        if peaks.is_empty() || rect.is_empty() {
            return;
        }
        let clip = self.current_clip();
        if clip.is_empty() {
            return;
        }
        let s = self.scale;
        let phys_clip = clip.scale(s);
        let layer = self.layer_mut();
        let start = layer.curve_vertices.len() as u32;
        let col_top = color.with_alpha(0.75).to_linear();
        let col_mid = color.with_alpha(0.38).to_linear();
        let mid_y = (rect.y + rect.h * 0.5) * s;
        let half_h = rect.h * 0.46 * s;
        let n = peaks.len();
        for i in 0..n.saturating_sub(1) {
            let x0 = (rect.x + rect.w * (i as f32 / (n - 1).max(1) as f32)) * s;
            let x1 = (rect.x + rect.w * ((i + 1) as f32 / (n - 1).max(1) as f32)) * s;
            let (lo0, hi0) = peaks[i];
            let (lo1, hi1) = peaks[i + 1];
            let yt0 = mid_y - hi0.clamp(-1.0, 1.0) * half_h;
            let yb0 = mid_y - lo0.clamp(-1.0, 1.0) * half_h;
            let yt1 = mid_y - hi1.clamp(-1.0, 1.0) * half_h;
            let yb1 = mid_y - lo1.clamp(-1.0, 1.0) * half_h;
            let v_t0 = CurveVertex {
                pos: [x0, yt0],
                edge: 0.0,
                half_width: 0.0,
                color: col_top,
                emissive,
                _pad: [0.0; 3],
            };
            let v_t1 = CurveVertex {
                pos: [x1, yt1],
                edge: 0.0,
                half_width: 0.0,
                color: col_top,
                emissive,
                _pad: [0.0; 3],
            };
            let v_b0 = CurveVertex {
                pos: [x0, yb0],
                edge: 0.0,
                half_width: 0.0,
                color: col_mid,
                emissive: emissive * 0.5,
                _pad: [0.0; 3],
            };
            let v_b1 = CurveVertex {
                pos: [x1, yb1],
                edge: 0.0,
                half_width: 0.0,
                color: col_mid,
                emissive: emissive * 0.5,
                _pad: [0.0; 3],
            };
            layer
                .curve_vertices
                .extend_from_slice(&[v_t0, v_t1, v_b0, v_t1, v_b1, v_b0]);
        }
        let count = layer.curve_vertices.len() as u32 - start;
        if count == 0 {
            return;
        }
        if emissive > 0.0 {
            layer.emissive_count = layer.emissive_count.saturating_add(1);
        }
        layer.push_curve_cmd(phys_clip, start, count);
    }

    pub fn text(
        &mut self,
        rect: Rect,
        text: impl Into<String>,
        size: f32,
        bold: bool,
        color: Color,
        align: TextAlign,
    ) {
        let text = text.into();
        if text.is_empty() || rect.is_empty() {
            return;
        }
        let clip = self.current_clip().intersect(rect);
        if clip.is_empty() {
            return;
        }
        self.layer_mut().texts.push(TextRun {
            rect,
            text,
            size,
            bold,
            color,
            align,
            clip,
        });
    }
}

/// Maximum miter-to-half-width ratio before falling back to a bevel join.
const MITER_LIMIT: f32 = 2.0;

#[derive(Clone, Copy)]
struct SegFrame {
    tangent: (f32, f32),
    normal: (f32, f32),
    len: f32,
}

#[inline]
fn seg_frame(p0: (f32, f32), p1: (f32, f32)) -> SegFrame {
    let dx = p1.0 - p0.0;
    let dy = p1.1 - p0.1;
    let len = (dx * dx + dy * dy).sqrt();
    let tx = dx / len;
    let ty = dy / len;
    SegFrame {
        tangent: (tx, ty),
        normal: (-ty, tx),
        len,
    }
}

#[inline]
fn compute_miter(prev: SegFrame, curr: SegFrame, pad_w: f32) -> Option<(f32, f32)> {
    let bx = prev.normal.0 + curr.normal.0;
    let by = prev.normal.1 + curr.normal.1;
    let blen = (bx * bx + by * by).sqrt();
    if blen < 1e-4 {
        return None;
    }
    let ux = bx / blen;
    let uy = by / blen;
    let cos_half = ux * prev.normal.0 + uy * prev.normal.1;
    if cos_half <= 1.0 / MITER_LIMIT {
        return None;
    }
    let miter_len = pad_w / cos_half;
    let mx = ux * miter_len;
    let my = uy * miter_len;
    let proj0 = (mx * prev.tangent.0 + my * prev.tangent.1).abs();
    let proj1 = (mx * curr.tangent.0 + my * curr.tangent.1).abs();
    if proj0 > prev.len * 0.45 || proj1 > curr.len * 0.45 {
        return None;
    }
    Some((mx, my))
}

/// Trim text safely on char boundaries with an ellipsis when exceeding `max_chars`.
pub fn trim_label(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_owned();
    }
    if max_chars == 1 {
        return "…".into();
    }
    let mut out: String = text.chars().take(max_chars - 1).collect();
    out.push('…');
    out
}

/// Linearly interpolate `values` at normalized horizontal position `norm_x` and map into `plot` Y coordinates.
pub(crate) fn sample_curve_y(values: &[f32], norm_x: f32, bounds: [f32; 2], plot: Rect) -> f32 {
    let fidx = norm_x.clamp(0.0, 1.0) * (values.len() - 1) as f32;
    let i0 = (fidx.floor() as usize).min(values.len() - 1);
    let i1 = (i0 + 1).min(values.len() - 1);
    let t = fidx - i0 as f32;
    let v = values[i0] * (1.0 - t) + values[i1] * t;
    let span = (bounds[1] - bounds[0]).max(1e-4);
    let yn = ((v - bounds[0]) / span).clamp(0.0, 1.0);
    plot.bottom() - 3.0 - yn * (plot.h - 6.0).max(1.0)
}

/// Map a uniformly sampled scalar curve (`values.len() >= 2`) into polyline vertices inside `plot`.
pub(crate) fn build_curve_pts(values: &[f32], bounds: [f32; 2], plot: Rect) -> Vec<(f32, f32)> {
    let n = values.len();
    let span = (bounds[1] - bounds[0]).max(1e-4);
    values
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let x = plot.x + plot.w * (i as f32 / (n - 1) as f32);
            let yn = ((v - bounds[0]) / span).clamp(0.0, 1.0);
            let y = plot.bottom() - 3.0 - yn * (plot.h - 6.0).max(1.0);
            (x, y)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    #[test]
    fn draw_commands_preserve_painters_order_and_coalesce_adjacent_batches() {
        let theme = Theme::cadence_dark();
        let mut dl = DisplayList::new(800.0, 600.0, 1.0);
        let full = Rect::new(0.0, 0.0, 800.0, 600.0);

        // Two adjacent quads (same clip) -> coalesced into 1 Quads command of count 2.
        dl.fill(Rect::new(0.0, 0.0, 100.0, 100.0), theme.surface.recessed);
        dl.fill(Rect::new(10.0, 10.0, 80.0, 80.0), theme.surface.elevated);

        // Two adjacent curves (same clip) -> coalesced into 1 Curves command.
        dl.polyline(
            &[(10.0, 50.0), (150.0, 50.0)],
            2.0,
            theme.accent.routing,
            0.4,
            None,
        );
        dl.polyline(
            &[(10.0, 70.0), (150.0, 70.0)],
            2.0,
            theme.accent.focus,
            0.0,
            None,
        );
        let curve_verts_after_first = dl.base.curve_vertices.len() as u32;

        // Two quads drawn AFTER curves -> MUST start a NEW Quads command (never merged into batch 0!).
        dl.panel(
            Rect::new(40.0, 30.0, 60.0, 60.0),
            theme.surface.elevated,
            theme.accent.focus,
            4.0,
            1.5,
            0.0,
        );
        dl.rounded_glow(
            Rect::new(65.0, 55.0, 8.0, 8.0),
            theme.accent.playhead,
            4.0,
            0.5,
        );

        // Quad under a different clip rect -> MUST split into a separate Quads command.
        let sub = Rect::new(200.0, 200.0, 100.0, 100.0);
        dl.push_clip(sub);
        dl.fill(Rect::new(210.0, 210.0, 50.0, 50.0), theme.surface.selection);
        dl.pop_clip();

        assert_eq!(
            dl.base.commands,
            vec![
                DrawCmd::Quads(DrawBatch {
                    clip: full,
                    start: 0,
                    count: 2,
                }),
                DrawCmd::Curves(DrawBatch {
                    clip: full,
                    start: 0,
                    count: curve_verts_after_first,
                }),
                DrawCmd::Quads(DrawBatch {
                    clip: full,
                    start: 2,
                    count: 2,
                }),
                DrawCmd::Quads(DrawBatch {
                    clip: sub,
                    start: 4,
                    count: 1,
                }),
            ]
        );
        assert_eq!(dl.base.emissive_count, 2);
    }

    #[test]
    fn display_list_clear_and_reset_retain_vec_capacities() {
        let theme = Theme::cadence_dark();
        let mut dl = DisplayList::new(640.0, 480.0, 1.0);
        for i in 0..64 {
            dl.fill(
                Rect::new(i as f32 * 4.0, 10.0, 10.0, 10.0),
                theme.surface.elevated,
            );
        }
        dl.polyline(
            &[(0.0, 0.0), (100.0, 50.0), (200.0, 10.0)],
            2.0,
            theme.accent.focus,
            0.3,
            None,
        );
        dl.text(
            Rect::new(10.0, 10.0, 100.0, 20.0),
            "Label",
            12.0,
            false,
            theme.text.primary,
            TextAlign::Left,
        );
        let q_cap = dl.base.quads.capacity();
        let c_cap = dl.base.curve_vertices.capacity();
        let cmd_cap = dl.base.commands.capacity();
        let t_cap = dl.base.texts.capacity();

        dl.reset(1280.0, 720.0, 2.0);

        assert!(dl.base.is_empty());
        assert!(dl.overlay.is_empty());
        assert_eq!(dl.base.commands.len(), 0);
        assert_eq!(dl.base.emissive_count, 0);
        assert_eq!(dl.viewport, Rect::new(0.0, 0.0, 1280.0, 720.0));
        assert_eq!(dl.scale, 2.0);
        assert_eq!(dl.current_clip(), dl.viewport);
        assert!(dl.base.quads.capacity() >= q_cap);
        assert!(dl.base.curve_vertices.capacity() >= c_cap);
        assert!(dl.base.commands.capacity() >= cmd_cap);
        assert!(dl.base.texts.capacity() >= t_cap);
    }

    #[test]
    fn polyline_emits_clean_miter_bevel_and_hairpin_joins() {
        let theme = Theme::cadence_dark();

        // 1. 90-degree corner on long segments -> exact connected miter join (2 quads = 12 verts).
        let mut dl = DisplayList::new(400.0, 400.0, 1.0);
        dl.polyline(
            &[(50.0, 200.0), (200.0, 200.0), (200.0, 50.0)],
            4.0,
            theme.accent.focus,
            0.0,
            None,
        );
        assert_eq!(dl.base.curve_vertices.len(), 12);
        // End of segment 0 (vertex index 2 = c, 4 = d) matches start of segment 1 (vertex 6 = a, 7 = b).
        assert_eq!(dl.base.curve_vertices[2].pos, dl.base.curve_vertices[6].pos);
        assert_eq!(dl.base.curve_vertices[4].pos, dl.base.curve_vertices[7].pos);

        // 2. Acute 150-degree bend (exceeds MITER_LIMIT) -> 2 quads + 1 bevel triangle = 15 verts.
        dl.clear();
        dl.polyline(
            &[(50.0, 200.0), (200.0, 200.0), (70.0, 130.0)],
            4.0,
            theme.accent.warning_key,
            0.0,
            None,
        );
        assert_eq!(dl.base.curve_vertices.len(), 15);

        // 3. 180-degree hairpin reversal -> 2 quads + 2-triangle beveled tip = 18 verts.
        dl.clear();
        dl.polyline(
            &[(50.0, 200.0), (200.0, 200.0), (50.0, 200.0)],
            4.0,
            theme.accent.alert,
            0.0,
            None,
        );
        assert_eq!(dl.base.curve_vertices.len(), 18);

        // 4. Short-segment 90-degree bend -> bowtie-safe fallback to bevel join (15 verts).
        dl.clear();
        dl.polyline(
            &[(100.0, 100.0), (102.0, 100.0), (102.0, 98.0)],
            4.0,
            theme.accent.playhead,
            0.0,
            None,
        );
        assert_eq!(dl.base.curve_vertices.len(), 15);
    }
}
