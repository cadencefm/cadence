// Antialiased GPU polyline & translucent fill shader for oscilloscopes,
// synth waveforms, filter curves, ADSR envelopes, sampler peaks, and value curves.
// Outputs dual-target MRT: main surface + selective phosphor emissive target.

struct Globals {
    viewport: vec2<f32>,
    _pad: vec2<f32>,
}

@group(0) @binding(0)
var<uniform> globals: Globals;

struct CurveIn {
    @location(0) pos: vec2<f32>,
    @location(1) edge: f32,
    @location(2) half_width: f32,
    @location(3) color: vec4<f32>,
    @location(4) emissive: f32,
}

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) edge: f32,
    @location(1) half_width: f32,
    @location(2) color: vec4<f32>,
    @location(3) emissive: f32,
}

struct FsOut {
    @location(0) main: vec4<f32>,
    @location(1) emissive: vec4<f32>,
}

@vertex
fn vs_main(v: CurveIn) -> VsOut {
    let ndc = vec2<f32>(
        (v.pos.x / globals.viewport.x) * 2.0 - 1.0,
        1.0 - (v.pos.y / globals.viewport.y) * 2.0,
    );
    var out: VsOut;
    out.clip_pos = vec4<f32>(ndc, 0.0, 1.0);
    out.edge = v.edge;
    out.half_width = v.half_width;
    out.color = v.color;
    out.emissive = v.emissive;
    return out;
}

fn shade(in: VsOut) -> FsOut {
    var cov = 1.0;
    if in.half_width > 0.0 {
        let dist_px = abs(in.edge) * in.half_width;
        cov = 1.0 - smoothstep(max(in.half_width - 0.85, 0.0), in.half_width + 0.85, dist_px);
        if cov <= 0.002 {
            discard;
        }
    }
    let alpha = in.color.a * cov;
    var out: FsOut;
    out.main = vec4<f32>(in.color.rgb, alpha);
    out.emissive = vec4<f32>(in.color.rgb * (in.emissive * alpha), alpha);
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
