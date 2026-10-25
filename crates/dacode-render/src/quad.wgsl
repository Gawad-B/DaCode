// Every rectangular element in the design — cursors, rules, tints, panels, buttons — is one of these.
// A corner radius or an outline width turns on a distance field for that instance only; a plain
// rectangle keeps hard edges, so a hairline stays one crisp row of pixels.

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
    // x: corner radius, y: outline width (0 fills).
    @location(2) shape: vec4<f32>,
};

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local: vec2<f32>,
    @location(2) size: vec2<f32>,
    @location(3) shape: vec4<f32>,
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
    out.local = corner * instance.rect.zw;
    out.size = instance.rect.zw;
    out.shape = instance.shape;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let radius = in.shape.x;
    let border = in.shape.y;
    if (radius <= 0.0 && border <= 0.0) {
        return in.color;
    }

    let half = in.size * 0.5;
    let q = abs(in.local - half) - (half - vec2<f32>(radius));
    var distance = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
    if (border > 0.0) {
        distance = abs(distance + border * 0.5) - border * 0.5;
    }
    let coverage = clamp(0.5 - distance, 0.0, 1.0);
    if (coverage <= 0.0) {
        discard;
    }
    return vec4<f32>(in.color.rgb, in.color.a * coverage);
}
