// Separable 9-tap phosphor bloom blur and final additive composite shader.
// Text is rendered via glyphon AFTER composite so typography stays razor-sharp.

struct BlurParams {
    texel_dir: vec2<f32>,  // (dx, 0) for horizontal or (0, dy) for vertical
    strength: f32,
    _pad: f32,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;
@group(0) @binding(2) var<uniform> params: BlurParams;

// Second texture used only by fs_composite (bloom texture).
@group(1) @binding(0) var bloom_tex: texture_2d<f32>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) vid: u32) -> VsOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    let p = positions[vid];
    var out: VsOut;
    out.pos = vec4<f32>(p, 0.0, 1.0);
    out.uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    return out;
}

@fragment
fn fs_blur(in: VsOut) -> @location(0) vec4<f32> {
    let d = params.texel_dir;
    var acc = textureSample(src_tex, src_sampler, in.uv) * 0.22702703;
    acc += textureSample(src_tex, src_sampler, in.uv + d * 1.3846154) * 0.31621622;
    acc += textureSample(src_tex, src_sampler, in.uv - d * 1.3846154) * 0.31621622;
    acc += textureSample(src_tex, src_sampler, in.uv + d * 3.2307692) * 0.07027027;
    acc += textureSample(src_tex, src_sampler, in.uv - d * 3.2307692) * 0.07027027;
    return acc;
}

@fragment
fn fs_composite(in: VsOut) -> @location(0) vec4<f32> {
    let base = textureSample(src_tex, src_sampler, in.uv);
    if params.strength <= 0.0 {
        return vec4<f32>(base.rgb, 1.0);
    }
    let glow = textureSample(bloom_tex, src_sampler, in.uv);
    let rgb = base.rgb + glow.rgb * params.strength;
    return vec4<f32>(rgb, 1.0);
}
