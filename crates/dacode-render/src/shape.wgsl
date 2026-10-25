// Stroked and filled geometry: segments, rings, arcs, discs and triangles, as signed distance fields.
//
// Icons are drawn out of these rather than from a font. The bundled faces have no icon set, a missing
// glyph renders as a tofu box, and an icon font would be one more asset with its own licence and its
// own idea of where the baseline is. A distance field also stays sharp at every scale factor and takes
// the theme's colour like any other ink.

struct Params {
    screen: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> params: Params;

struct Instance {
    // Two points in physical pixels: a segment's ends, or a circle's centre and (unused).
    @location(0) ab: vec4<f32>,
    // A third point for a triangle; for an arc, its start and end angle in radians.
    @location(1) c: vec4<f32>,
    // x: stroke width, y: kind, z: radius.
    @location(2) style: vec4<f32>,
    @location(3) color: vec4<f32>,
};

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) pixel: vec2<f32>,
    @location(1) ab: vec4<f32>,
    @location(2) c: vec4<f32>,
    @location(3) style: vec4<f32>,
    @location(4) color: vec4<f32>,
};

const SEGMENT: f32 = 0.0;
const RING: f32 = 1.0;
const ARC: f32 = 2.0;
const DISC: f32 = 3.0;
const TRIANGLE: f32 = 4.0;

@vertex
fn vs_main(@builtin(vertex_index) index: u32, instance: Instance) -> VertexOut {
    var corners = array<vec2<f32>, 4>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
    );

    let a = instance.ab.xy;
    let b = instance.ab.zw;
    let kind = instance.style.y;
    // Half the stroke plus a pixel, so the antialiased edge is never clipped by its own box.
    let pad = instance.style.x * 0.5 + 1.0;

    var lo: vec2<f32>;
    var hi: vec2<f32>;
    if (kind == SEGMENT) {
        lo = min(a, b) - vec2<f32>(pad);
        hi = max(a, b) + vec2<f32>(pad);
    } else if (kind == TRIANGLE) {
        lo = min(min(a, b), instance.c.xy) - vec2<f32>(1.0);
        hi = max(max(a, b), instance.c.xy) + vec2<f32>(1.0);
    } else {
        let reach = instance.style.z + pad;
        lo = a - vec2<f32>(reach);
        hi = a + vec2<f32>(reach);
    }

    let pixel = mix(lo, hi, corners[index]);
    let ndc = vec2<f32>(
        pixel.x / params.screen.x * 2.0 - 1.0,
        1.0 - pixel.y / params.screen.y * 2.0,
    );

    var out: VertexOut;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.pixel = pixel;
    out.ab = instance.ab;
    out.c = instance.c;
    out.style = instance.style;
    out.color = instance.color;
    return out;
}

fn to_segment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let ab = b - a;
    let t = clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-6), 0.0, 1.0);
    return length(p - (a + ab * t));
}

const TAU: f32 = 6.28318530718;

fn to_arc(p: vec2<f32>, centre: vec2<f32>, radius: f32, start: f32, end: f32) -> f32 {
    let offset = p - centre;
    // Angle measured the way the arc was described: from +x, increasing clockwise on screen (y down).
    var angle = atan2(offset.y, offset.x);
    // Bring it into the sweep's own turn, so a sweep that crosses 0 still compares in order.
    let turn = angle - start;
    angle = start + (turn - TAU * floor(turn / TAU));
    if (angle <= end) {
        return abs(length(offset) - radius);
    }
    let first = centre + radius * vec2<f32>(cos(start), sin(start));
    let last = centre + radius * vec2<f32>(cos(end), sin(end));
    return min(length(p - first), length(p - last));
}

fn cross2(a: vec2<f32>, b: vec2<f32>) -> f32 {
    return a.x * b.y - a.y * b.x;
}

fn to_triangle(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, c: vec2<f32>) -> f32 {
    let edge = min(min(to_segment(p, a, b), to_segment(p, b, c)), to_segment(p, c, a));
    // Inside when p is on the same side of all three edges, whichever way they wind.
    let s1 = cross2(b - a, p - a);
    let s2 = cross2(c - b, p - b);
    let s3 = cross2(a - c, p - c);
    let inside = (s1 >= 0.0 && s2 >= 0.0 && s3 >= 0.0) || (s1 <= 0.0 && s2 <= 0.0 && s3 <= 0.0);
    return select(edge, -edge, inside);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let p = in.pixel;
    let a = in.ab.xy;
    let b = in.ab.zw;
    let half = in.style.x * 0.5;
    let kind = in.style.y;
    let radius = in.style.z;

    var distance: f32;
    if (kind == SEGMENT) {
        distance = to_segment(p, a, b) - half;
    } else if (kind == RING) {
        distance = abs(length(p - a) - radius) - half;
    } else if (kind == ARC) {
        distance = to_arc(p, a, radius, in.c.x, in.c.y) - half;
    } else if (kind == DISC) {
        distance = length(p - a) - radius;
    } else {
        distance = to_triangle(p, a, b, in.c.xy);
    }

    let coverage = clamp(0.5 - distance, 0.0, 1.0);
    if (coverage <= 0.0) {
        discard;
    }
    return vec4<f32>(in.color.rgb, in.color.a * coverage);
}
