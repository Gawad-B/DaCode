// The DaCode mark — the letter ض with its fatha and blocks — from two coverage masks.
//
// The mark is artwork, not geometry anyone would write out as strokes, so it is a texture. But not a
// picture: red is how much of a pixel the letter covers and green how much the amber block does, and
// the colours are supplied per draw. That is what lets one asset be white on a dark theme and near
// black on a light one, with the amber kept — the light card in the logo sheet is exactly that.
//
// Layer 0 is the full mark; layer 1 is the small one, where the white and amber blocks become a single
// block, as the logo sheet specifies below 64 pixels.

struct Params {
    screen: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var masks: texture_2d_array<f32>;
@group(0) @binding(2) var masks_sampler: sampler;

struct Instance {
    @location(0) rect: vec4<f32>,
    @location(1) ink: vec4<f32>,
    @location(2) accent: vec4<f32>,
    // x: which layer.
    @location(3) variant: vec4<f32>,
};

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) ink: vec4<f32>,
    @location(2) accent: vec4<f32>,
    @location(3) @interpolate(flat) layer: i32,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32, instance: Instance) -> VertexOut {
    var corners = array<vec2<f32>, 4>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
    );
    let corner = corners[index];
    let pixel = instance.rect.xy + corner * instance.rect.zw;
    let ndc = vec2<f32>(
        pixel.x / params.screen.x * 2.0 - 1.0,
        1.0 - pixel.y / params.screen.y * 2.0,
    );

    var out: VertexOut;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = corner;
    out.ink = instance.ink;
    out.accent = instance.accent;
    out.layer = i32(instance.variant.x);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let coverage = textureSample(masks, masks_sampler, in.uv, in.layer).rg;
    let letter = coverage.r * in.ink.a;
    let block = coverage.g * in.accent.a;
    // The letter is drawn over the block, as the white block overlaps the amber one in the artwork.
    let alpha = letter + block * (1.0 - letter);
    if (alpha <= 0.0) {
        discard;
    }
    let color = (in.ink.rgb * letter + in.accent.rgb * block * (1.0 - letter)) / alpha;
    return vec4<f32>(color, alpha);
}
