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
