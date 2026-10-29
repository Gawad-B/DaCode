// A picture: an image file or a rendered PDF page, sampled from a texture of its own.
//
// The only primitive here that shows someone else's pixels. Each instance carries the part of its
// texture to show (`uv`), so a picture larger than the view is cut to the view by the coordinates
// rather than by a scissor, the way `Rect::intersect` cuts a rectangle.

struct Params {
    screen: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(1) @binding(0) var picture: texture_2d<f32>;
@group(1) @binding(1) var picture_sampler: sampler;

struct Instance {
    @location(0) rect: vec4<f32>,
    // x0, y0, x1, y1 in the texture, 0 to 1.
    @location(1) uv: vec4<f32>,
    // x: opacity.
    @location(2) style: vec4<f32>,
};

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) opacity: f32,
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
    out.uv = mix(instance.uv.xy, instance.uv.zw, corner);
    out.opacity = instance.style.x;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let color = textureSample(picture, picture_sampler, in.uv);
    let alpha = color.a * in.opacity;
    if (alpha <= 0.0) {
        discard;
    }
    return vec4<f32>(color.rgb, alpha);
}
