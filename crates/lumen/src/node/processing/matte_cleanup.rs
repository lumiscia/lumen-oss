use crate::{
    node::{
        NodeId, NodeProperty, PortRef,
        processing::gpu_shader::{ShaderUniform, apply_runtime_shader},
        processing::raster_map::unit_to_byte,
    },
    raster::RasterFrame,
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Node)]
pub struct MatteCleanup {
    pub id: NodeId,

    #[property(expected = Float)]
    pub threshold: NodeProperty,
    #[property(expected = Int)]
    pub shrink: NodeProperty,
    #[property(expected = Int)]
    pub grow: NodeProperty,

    #[input(kind = Raster)]
    pub source: PortRef,
}

impl Default for MatteCleanup {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            threshold: NodeProperty::Float(0.0),
            shrink: NodeProperty::Int(0),
            grow: NodeProperty::Int(0),
            source: PortRef::empty(),
        }
    }
}

#[node_impl]
impl MatteCleanup {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<RasterFrame> {
        let threshold = self.resolve_threshold(ctx)? as f32;
        let shrink = self.resolve_shrink(ctx)?.max(0) as u32;
        let grow = self.resolve_grow(ctx)?.max(0) as u32;
        let source_result = ctx.eval(&self.source)?;
        let source = source_result.as_raster()?;
        if threshold <= 0.0 && shrink == 0 && grow == 0 {
            return source.snapshot();
        }
        let threshold_uniform = [threshold.clamp(0.0, 1.0)];
        let shader = matte_cleanup_shader(shrink, grow);

        apply_runtime_shader(
            source,
            &shader,
            &[ShaderUniform {
                name: "threshold",
                values: &threshold_uniform,
            }],
            source.alpha_mode(),
            self.id,
            "MatteCleanup",
            ctx.frame,
            ctx,
        )
    }
}

fn matte_cleanup_shader(shrink: u32, grow: u32) -> String {
    format!(
        r#"
uniform shader source;
uniform float threshold;
uniform float2 resolution;

float threshold_alpha(float alpha) {{
    if (threshold <= 0.0) {{
        return alpha;
    }}
    return alpha >= threshold ? 1.0 : 0.0;
}}

bool inside_bounds(float2 coord) {{
    return coord.x >= 0.0 && coord.y >= 0.0 && coord.x < resolution.x && coord.y < resolution.y;
}}

float source_alpha(float2 coord, float outside) {{
    if (!inside_bounds(coord)) {{
        return outside;
    }}
    return threshold_alpha(float(source.eval(coord).a));
}}

float eroded_alpha(float2 coord) {{
    if (!inside_bounds(coord)) {{
        return 0.0;
    }}
    float result = 1.0;
    for (int y = -{shrink}; y <= {shrink}; ++y) {{
        for (int x = -{shrink}; x <= {shrink}; ++x) {{
            result = min(result, source_alpha(coord + float2(float(x), float(y)), 1.0));
        }}
    }}
    return result;
}}

float cleaned_alpha(float2 coord) {{
    float result = 0.0;
    for (int y = -{grow}; y <= {grow}; ++y) {{
        for (int x = -{grow}; x <= {grow}; ++x) {{
            result = max(result, eroded_alpha(coord + float2(float(x), float(y))));
        }}
    }}
    return result;
}}

half4 main(float2 coord) {{
    coord = floor(coord) + float2(0.5);
    half4 color = source.eval(coord);
    color.a = half(cleaned_alpha(coord));
    return color;
}}
"#
    )
}

pub(crate) fn apply_matte_cleanup_bytes(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    row_bytes: usize,
    threshold: f32,
    shrink: u32,
    grow: u32,
) {
    if width == 0 || height == 0 {
        return;
    }

    let mut alpha = extract_alpha(pixels, width, height, row_bytes);
    if threshold > 0.0 {
        let threshold = unit_to_byte(threshold);
        for value in &mut alpha {
            *value = if *value >= threshold { 255 } else { 0 };
        }
    }
    if shrink > 0 {
        alpha = morphology_alpha(&alpha, width, height, shrink, Morphology::Erode);
    }
    if grow > 0 {
        alpha = morphology_alpha(&alpha, width, height, grow, Morphology::Dilate);
    }
    write_alpha(pixels, width, height, row_bytes, &alpha);
}

fn extract_alpha(pixels: &[u8], width: u32, height: u32, row_bytes: usize) -> Vec<u8> {
    let mut alpha = Vec::with_capacity((width as usize).saturating_mul(height as usize));
    for y in 0..height as usize {
        let row_start = y * row_bytes;
        for x in 0..width as usize {
            alpha.push(pixels[row_start + x * 4 + 3]);
        }
    }
    alpha
}

fn write_alpha(pixels: &mut [u8], width: u32, height: u32, row_bytes: usize, alpha: &[u8]) {
    for y in 0..height as usize {
        let row_start = y * row_bytes;
        for x in 0..width as usize {
            pixels[row_start + x * 4 + 3] = alpha[y * width as usize + x];
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Morphology {
    Erode,
    Dilate,
}

fn morphology_alpha(
    alpha: &[u8],
    width: u32,
    height: u32,
    radius: u32,
    mode: Morphology,
) -> Vec<u8> {
    let width_usize = width as usize;
    let height_usize = height as usize;
    let radius = radius as i32;
    let mut output = vec![0; alpha.len()];

    for y in 0..height_usize {
        for x in 0..width_usize {
            let mut result = match mode {
                Morphology::Erode => 255,
                Morphology::Dilate => 0,
            };

            for sample_y in y as i32 - radius..=y as i32 + radius {
                if sample_y < 0 || sample_y >= height as i32 {
                    continue;
                }
                for sample_x in x as i32 - radius..=x as i32 + radius {
                    if sample_x < 0 || sample_x >= width as i32 {
                        continue;
                    }
                    let value = alpha[sample_y as usize * width_usize + sample_x as usize];
                    match mode {
                        Morphology::Erode => result = result.min(value),
                        Morphology::Dilate => result = result.max(value),
                    }
                }
            }

            output[y * width_usize + x] = result;
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{node::processing::test_support, raster::AlphaMode};

    #[test]
    fn matte_cleanup_threshold_and_grow_affect_alpha_geometry() {
        let mut pixels = vec![10, 10, 10, 0, 10, 10, 10, 200, 10, 10, 10, 0];

        apply_matte_cleanup_bytes(&mut pixels, 3, 1, 12, 0.5, 0, 1);

        let alphas: Vec<u8> = pixels.chunks_exact(4).map(|pixel| pixel[3]).collect();
        assert_eq!(alphas, vec![255, 255, 255]);
    }

    #[test]
    fn matte_cleanup_shrink_erodes_alpha() {
        let mut pixels = vec![
            0, 0, 0, 0, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 0,
        ];

        apply_matte_cleanup_bytes(&mut pixels, 5, 1, 20, 0.0, 1, 0);

        let alphas: Vec<u8> = pixels.chunks_exact(4).map(|pixel| pixel[3]).collect();
        assert_eq!(alphas, vec![0, 0, 255, 0, 0]);
    }

    #[test]
    fn matte_cleanup_sksl_thresholds_alpha() {
        let source = test_support::frame_from_pixel([10, 20, 30, 128], AlphaMode::Premultiplied);
        let threshold = [0.5];
        let shader = matte_cleanup_shader(0, 0);

        let output = test_support::with_test_context(1, 1, |ctx| {
            apply_runtime_shader(
                &source,
                &shader,
                &[ShaderUniform {
                    name: "threshold",
                    values: &threshold,
                }],
                source.alpha_mode(),
                NodeId::new(1),
                "MatteCleanup",
                ctx.frame,
                ctx,
            )
        })
        .expect("shader output");

        assert_eq!(test_support::read_first_pixel(&output), [10, 20, 30, 255]);
    }

    #[test]
    fn matte_cleanup_sksl_erodes_and_dilates_alpha() {
        let source = test_support::frame_from_rgba(
            &[
                0, 0, 0, 0, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 0,
            ],
            5,
            1,
            AlphaMode::Premultiplied,
        );
        let threshold = [0.0];
        let erode_shader = matte_cleanup_shader(1, 0);

        let eroded = test_support::with_test_context(5, 1, |ctx| {
            apply_runtime_shader(
                &source,
                &erode_shader,
                &[ShaderUniform {
                    name: "threshold",
                    values: &threshold,
                }],
                source.alpha_mode(),
                NodeId::new(1),
                "MatteCleanup",
                ctx.frame,
                ctx,
            )
        })
        .expect("eroded output");
        let eroded_alphas: Vec<u8> = test_support::read_pixels(&eroded)
            .chunks_exact(4)
            .map(|pixel| pixel[3])
            .collect();
        assert_eq!(eroded_alphas, vec![0, 0, 255, 0, 0]);

        let grow_shader = matte_cleanup_shader(0, 1);
        let grown = test_support::with_test_context(5, 1, |ctx| {
            apply_runtime_shader(
                &eroded,
                &grow_shader,
                &[ShaderUniform {
                    name: "threshold",
                    values: &threshold,
                }],
                eroded.alpha_mode(),
                NodeId::new(1),
                "MatteCleanup",
                ctx.frame,
                ctx,
            )
        })
        .expect("grown output");
        let grown_alphas: Vec<u8> = test_support::read_pixels(&grown)
            .chunks_exact(4)
            .map(|pixel| pixel[3])
            .collect();
        assert_eq!(grown_alphas, vec![0, 255, 255, 255, 0]);
    }
}
