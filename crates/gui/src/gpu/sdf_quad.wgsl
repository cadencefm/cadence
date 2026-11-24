// Instanced analytic SDF rounded-rectangle shader with gradients, crisp hairline
// borders, optional diagonal hatch bands, and dual-target (main + emissive) MRT output.

struct Globals {
    viewport: vec2<f32>,
    _pad: vec2<f32>,
}

@group(0) @binding(0)
var<uniform> globals: Globals;

struct QuadIn {
    @location(0) rect: vec4<f32>,         // x, y, w, h in physical px
    @location(1) color_top: vec4<f32>,
    @location(2) color_bottom: vec4<f32>,
    @location(3) border_color: vec4<f32>,
    @location(4) params: vec4<f32>,       // radius, border_width, emissive, hatch_period
}

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) local_pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) color_top: vec4<f32>,
    @location(3) color_bottom: vec4<f32>,
    @location(4) border_color: vec4<f32>,
    @location(5) params: vec4<f32>,
}

struct FsOut {
    @location(0) main: vec4<f32>,
    @location(1) emissive: vec4<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vid: u32, inst: QuadIn) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let uv = corners[vid];
    let pad = 1.0;
    let size = inst.rect.zw;
    let local = uv * (size + vec2<f32>(pad * 2.0)) - vec2<f32>(pad);
    let px = inst.rect.xy + local;
    let ndc = vec2<f32>(
        (px.x / globals.viewport.x) * 2.0 - 1.0,
        1.0 - (px.y / globals.viewport.y) * 2.0,
    );

    var out: VsOut;
    out.clip_pos = vec4<f32>(ndc, 0.0, 1.0);
    out.local_pos = local;
    out.size = size;
    out.color_top = inst.color_top;
    out.color_bottom = inst.color_bottom;
    out.border_color = inst.border_color;
    out.params = inst.params;
    return out;
}

fn sdf_rounded_box(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half_size + vec2<f32>(radius);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

fn shade(in: VsOut) -> FsOut {
    let half_size = in.size * 0.5;
    let p = in.local_pos - half_size;
    let radius = clamp(in.params.x, 0.0, min(half_size.x, half_size.y));
    let border_w = max(in.params.y, 0.0);
    let emissive = max(in.params.z, 0.0);
    let hatch_period = in.params.w;

    let d = sdf_rounded_box(p, half_size, radius);
    let aa = 0.75;
    let outer_alpha = 1.0 - smoothstep(-aa, aa, d);
    if outer_alpha <= 0.001 {
        discard;
    }

    let t = clamp(in.local_pos.y / max(in.size.y, 1.0), 0.0, 1.0);
    var fill = mix(in.color_top, in.color_bottom, t);

    // Optional crisp diagonal hatch band (for pending loop / override bands).
    if hatch_period > 1.0 {
        let diag = in.local_pos.x + in.local_pos.y;
        let phase = fract(diag / hatch_period);
        let stripe = smoothstep(0.42, 0.48, phase) - smoothstep(0.56, 0.62, phase);
        fill = vec4<f32>(fill.rgb, fill.a * (0.45 + 0.55 * stripe));
    }

    var final_col = fill;
    if border_w > 0.0 && in.border_color.a > 0.001 {
        let inner_d = d + border_w;
        let border_mix = smoothstep(-aa, aa, inner_d);
        let bcol = in.border_color;
        let out_a = mix(fill.a, bcol.a, border_mix);
        let out_rgb = mix(fill.rgb * fill.a, bcol.rgb * bcol.a, border_mix) / max(out_a, 1e-5);
        final_col = vec4<f32>(out_rgb, out_a);
    }

    final_col.a = final_col.a * outer_alpha;

    var out: FsOut;
    out.main = final_col;
    // Write selective emissive luminance weighted by alpha for Pass 2 bloom.
    out.emissive = vec4<f32>(final_col.rgb * (emissive * final_col.a), final_col.a);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> FsOut {
    return shade(in);
}

@fragment
fn fs_overlay(in: VsOut) -> @location(0) vec4<f32> {
    return shade(in).main;
}
