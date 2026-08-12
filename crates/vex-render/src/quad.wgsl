// Every graphic element in the design is a rectangle — cursors, rules, tints, swatches,
// underlines. So the entire non-text renderer is one instanced quad pipeline.

struct Params {
    screen: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> params: Params;

struct Instance {
    // x, y, width, height in physical pixels, origin top-left.
    @location(0) rect: vec4<f32>,
    // Linear RGBA. Colors arrive already converted from OKLCH.
    @location(1) color: vec4<f32>,
};

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32, instance: Instance) -> VertexOut {
    // Triangle strip over the unit square: no index buffer, no vertex buffer for geometry.
    var corners = array<vec2<f32>, 4>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
    );

    let corner = corners[index];
    let pixel = instance.rect.xy + corner * instance.rect.zw;

    // Pixels (y down) -> normalized device coordinates (y up).
    let ndc = vec2<f32>(
        pixel.x / params.screen.x * 2.0 - 1.0,
        1.0 - pixel.y / params.screen.y * 2.0,
    );

    var out: VertexOut;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.color = instance.color;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return in.color;
}
