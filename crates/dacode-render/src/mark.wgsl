// The dacode mark: two mitred chevrons, the second a 34% echo of the first one step down.
//
// It gets its own pipeline rather than being expressed in the quad pipeline, for two reasons that
// are both about correctness rather than convenience. The strokes are diagonal, so they need
// antialiasing that axis-aligned rectangles never did; and the echo is translucent, so drawing it as
// overlapping quads would double-blend where the two arms meet and show a seam down the middle of
// the V. Evaluating the shape as a distance field solves both: coverage is computed once per pixel,
// and alpha is applied once at the end.
//
// Each chevron is turned into the hexagon its stroke actually covers — two butt caps, four offset
// edges meeting at the two miter points — and the distance to that polygon is exact. An earlier
// attempt unioned two extended rectangles instead, which was wrong in a way worth recording: the
// extensions poke out past the far arm's outer edge, and the mark rendered as an X.
//
// The geometry is the handoff's, in its 100x100 em box, and is fixed here because the mark is
// monochrome and immutable by rule — there is nothing for a caller to configure but size and color.

struct Params {
    screen: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> params: Params;

struct Instance {
    // The em box in physical pixels: x, y, width, height. Square in practice.
    @location(0) rect: vec4<f32>,
    // Linear RGBA. The mark is monochrome, so these differ only in lightness.
    @location(1) color: vec4<f32>,
    // The echo, already mixed towards the surface in OKLCH by the caller.
    @location(2) echo: vec4<f32>,
    // Non-zero selects the solo variant: one chevron, thicker, re-centred. Used below 32px, where
    // the echo turns to mud.
    @location(3) solo: f32,
};

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) echo: vec4<f32>,
    // Position within the 100x100 em box.
    @location(2) em: vec2<f32>,
    // How many em units one pixel covers, for the antialiasing width.
    @location(3) em_per_pixel: f32,
    @location(4) solo: f32,
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
    out.color = instance.color;
    out.echo = instance.echo;
    out.em = corner * 100.0;
    // Uniform scale: the box is square, so either axis gives the same answer.
    out.em_per_pixel = 100.0 / max(instance.rect.z, 1.0);
    out.solo = instance.solo;
    return out;
}

/// Signed distance to the closed hexagon a chevron's stroke covers. Negative inside.
///
/// `left`, `apex`, `right` are the path, `half_width` half the stroke. The outline is built rather
/// than approximated, which is what makes the caps butt and the joins miter — the two properties the
/// handoff is explicit about.
fn chevron(p: vec2<f32>, left: vec2<f32>, apex: vec2<f32>, right: vec2<f32>, half_width: f32) -> f32 {
    let d1 = normalize(apex - left);
    let d2 = normalize(right - apex);
    // Normals on a consistent side of both arms, so one miter formula serves both corners.
    let n1 = vec2<f32>(-d1.y, d1.x);
    let n2 = vec2<f32>(-d2.y, d2.x);

    // Where the two offset edges meet: the classic miter point. The arms are ~76 degrees apart, so
    // the denominator is nowhere near zero and no spike limit is needed.
    let bisector = normalize(n1 + n2);
    let reach = half_width / dot(bisector, n1);
    let miter_near = apex + bisector * reach;
    let miter_far = apex - bisector * reach;

    var poly = array<vec2<f32>, 6>(
        left + n1 * half_width,
        miter_near,
        right + n2 * half_width,
        right - n2 * half_width,
        miter_far,
        left - n1 * half_width,
    );

    // Distance to the polygon's edges, with an inside test by crossing count. The hexagon is not
    // convex — the inner miter is a reflex corner — so the winding test is the one that works.
    var distance = dot(p - poly[0], p - poly[0]);
    var inside = false;

    for (var i = 0u; i < 6u; i = i + 1u) {
        let j = (i + 5u) % 6u;
        let a = poly[i];
        let b = poly[j];

        let edge = b - a;
        let to_p = p - a;
        let t = clamp(dot(to_p, edge) / dot(edge, edge), 0.0, 1.0);
        let offset = to_p - edge * t;
        distance = min(distance, dot(offset, offset));

        let crosses = (p.y >= a.y) != (p.y >= b.y);
        let side = edge.x * to_p.y - edge.y * to_p.x;
        if crosses && (side > 0.0) == (edge.y > 0.0) {
            inside = !inside;
        }
    }

    let magnitude = sqrt(distance);
    return select(magnitude, -magnitude, inside);
}

/// Coverage from a signed distance, feathered over one pixel.
fn coverage(distance: f32, em_per_pixel: f32) -> f32 {
    return clamp(0.5 - distance / em_per_pixel, 0.0, 1.0);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    if in.solo > 0.5 {
        // Solo: M20 26 L50 66 L80 26, stroke 13.
        let d = chevron(
            in.em,
            vec2<f32>(20.0, 26.0),
            vec2<f32>(50.0, 66.0),
            vec2<f32>(80.0, 26.0),
            6.5,
        );
        return vec4<f32>(in.color.rgb, in.color.a * coverage(d, in.em_per_pixel));
    }

    // Primary: M20 14 L50 52 L80 14, and the echo 26 units below. Stroke 11.
    let primary = chevron(
        in.em,
        vec2<f32>(20.0, 14.0),
        vec2<f32>(50.0, 52.0),
        vec2<f32>(80.0, 14.0),
        5.5,
    );
    let echo = chevron(
        in.em,
        vec2<f32>(20.0, 40.0),
        vec2<f32>(50.0, 78.0),
        vec2<f32>(80.0, 40.0),
        5.5,
    );

    let front = coverage(primary, in.em_per_pixel);
    let back = coverage(echo, in.em_per_pixel);

    // The echo is *under* the primary, composited here rather than by the blender: two draws would
    // double-blend where the shapes overlap, which is the seam this pipeline exists to avoid.
    let rgb = mix(in.echo.rgb, in.color.rgb, front);
    let alpha = max(front, back);
    return vec4<f32>(rgb, in.color.a * alpha);
}
