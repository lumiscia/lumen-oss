// Declarations shared by the shape and path rasterizers.

// Bits of `flags` in the shape and path uniforms.
const STYLE_FILL_ENABLED: u32 = 1u;
const STYLE_STROKE_ENABLED: u32 = 2u;
const STYLE_EDGE_ANTIALIAS: u32 = 4u;

fn edge_coverage(distance: f32, edge_antialias: bool) -> f32 {
    return select(
        select(0.0, 1.0, distance <= 0.0),
        clamp(0.5 - distance, 0.0, 1.0),
        edge_antialias,
    );
}

// Signed distance to an ellipse centred on the origin, negative inside, and the outward
// normal at its nearest point, not normalized.
struct EllipseDistance {
    distance: f32,
    normal: vec2<f32>,
}

// Finds the nearest point by repeatedly projecting onto the circle of curvature at the
// current guess (Chatfield's method), with each projection written so it does not cancel
// when that circle is far larger than the ellipse. The iteration only converges quickly
// from a guess near the answer, which for an eccentric ellipse the 45 degree point is not,
// so it starts from whichever of two guesses lies closer: the radial projection of the
// point, exact for the tips and for points far from the outline, and the exact solution on
// the major axis, where it leaves the axis for a point inside the evolute. Six steps are
// accurate to within 1/100 pixel everywhere, for ellipses up to 100,000 times longer than
// wide.
fn ellipse_distance(p: vec2<f32>, radii: vec2<f32>) -> EllipseDistance {
    if (radii.x == radii.y) {
        return EllipseDistance(length(p) - radii.x, p);
    }
    let q = abs(p);
    let r = max(radii, vec2<f32>(max(max(radii.x, radii.y) * 1e-7, 1e-6)));
    let k = r.x * r.x - r.y * r.y;
    let scaled = q / r;
    let scaled_length = dot(scaled, scaled);
    let radial = scaled * inverseSqrt(max(scaled_length, 1e-30));
    let major_is_x = r.x > r.y;
    let along = min(
        max(r.x, r.y) * select(q.y, q.x, major_is_x) / max(abs(k), 1e-30),
        1.0,
    );
    let across = sqrt(max(1.0 - along * along, 0.0));
    let axis = select(vec2<f32>(across, along), vec2<f32>(along, across), major_is_x);
    // Near the evolute cusp the two guesses are equally close and rounding picks one at
    // random, so the radial guess has to win by a margin to replace the axis solution.
    let margin = max(r.x, r.y) * 1e-4;
    let radial_distance = select(1e30, length(q - r * radial), scaled_length > 1e-30);
    let axis_distance = length(q - r * axis);
    let start = select(axis, radial, radial_distance < axis_distance - margin);
    var t = start;
    for (var step = 0; step < 6; step = step + 1) {
        let point = r * t;
        let centre = vec2<f32>(k / r.x, -k / r.y) * t * t * t;
        let radius = length(point - centre);
        let distance = length(q - centre);
        // `1 - radius / distance`, from the difference of their squares.
        let shrink = dot(q - point, q + point - 2.0 * centre)
            / max(distance * (radius + distance), 1e-30);
        let next = clamp((q + (centre - q) * shrink) / r, vec2<f32>(0.0), vec2<f32>(1.0));
        // A step that clamps both components to zero has no direction; keep the last guess.
        t = select(t, next * inverseSqrt(max(dot(next, next), 1e-30)), dot(next, next) > 1e-12);
    }
    // Every guess is a point on the ellipse, so the nearest of them bounds the distance even
    // where six steps fall short, as at the tip of an ellipse tens of thousands of times
    // longer than wide.
    var distance = length(q - r * t);
    if (radial_distance < distance) {
        distance = radial_distance;
        t = radial;
    }
    if (axis_distance < distance) {
        distance = axis_distance;
        t = axis;
    }
    let inside = scaled_length < 1.0;
    let side = select(vec2<f32>(1.0), vec2<f32>(-1.0), p < vec2<f32>(0.0));
    // Off the outline the displacement to the nearest point is the direction the distance
    // grows in, and stays so when the iteration falls short and a seed wins, where the
    // seed's own surface normal can point well away from it.
    var normal = t / r;
    if (distance > 1e-3) {
        normal = (q - r * t) * select(1.0, -1.0, inside);
    }
    return EllipseDistance(select(distance, -distance, inside), side * normal);
}
