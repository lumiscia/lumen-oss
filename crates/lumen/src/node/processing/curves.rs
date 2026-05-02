use crate::{
    error::{LumenError, PropertyError},
    gpu_image::{AlphaMode, GpuImageFrame, skia_image_from_rgba_upload},
    node::{
        NodeId, NodeProperty, PortRef,
        processing::color_table::unit_to_byte,
        processing::gpu_shader::{ChildShader, apply_runtime_shader_with_children},
    },
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};
use skia_safe::{SamplingOptions, TileMode};

#[derive(Debug, Clone, Node)]
pub struct Curves {
    pub id: NodeId,

    #[property(expected = String)]
    pub curve: NodeProperty,
    #[property(expected = String)]
    pub red_curve: NodeProperty,
    #[property(expected = String)]
    pub green_curve: NodeProperty,
    #[property(expected = String)]
    pub blue_curve: NodeProperty,

    #[input(kind = Raster)]
    pub source: PortRef,
}

impl Default for Curves {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            curve: NodeProperty::String("0:0,1:1".to_string()),
            red_curve: NodeProperty::String(String::new()),
            green_curve: NodeProperty::String(String::new()),
            blue_curve: NodeProperty::String(String::new()),
            source: PortRef::empty(),
        }
    }
}

#[node_impl]
impl Curves {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<GpuImageFrame> {
        let curve = parse_curve_property(self.id, "curve", &self.resolve_curve(ctx)?)?;
        let red_curve = parse_optional_curve_property(
            self.id,
            "red_curve",
            &self.resolve_red_curve(ctx)?,
            curve.clone(),
        )?;
        let green_curve = parse_optional_curve_property(
            self.id,
            "green_curve",
            &self.resolve_green_curve(ctx)?,
            curve.clone(),
        )?;
        let blue_curve = parse_optional_curve_property(
            self.id,
            "blue_curve",
            &self.resolve_blue_curve(ctx)?,
            curve,
        )?;
        let source_result = ctx.eval(&self.source)?;
        let source = source_result.as_raster()?;
        let curve_image = curve_image([&red_curve, &green_curve, &blue_curve], self.id, ctx.frame)?;
        let curve_shader = curve_image
            .to_shader(
                Some((TileMode::Clamp, TileMode::Clamp)),
                SamplingOptions::default(),
                None,
            )
            .ok_or_else(|| {
                invalid_curve_shader(self.id, ctx.frame, "curve shader creation failed")
            })?;

        apply_runtime_shader_with_children(
            source,
            CURVES_SHADER,
            &[],
            &[ChildShader {
                name: "curves",
                shader: curve_shader,
            }],
            source.alpha_mode(),
            self.id,
            "Curves",
            ctx.frame,
            ctx,
        )
    }
}

const CURVE_TABLE_SIZE: usize = 256;

const CURVES_SHADER: &str = r#"
uniform shader source;
uniform shader curves;

half4 main(float2 coord) {
    half4 color = source.eval(coord);
    return half4(
        curves.eval(float2(clamp(float(color.r), 0.0, 1.0) * 255.0, 0.0)).r,
        curves.eval(float2(clamp(float(color.g), 0.0, 1.0) * 255.0, 0.0)).g,
        curves.eval(float2(clamp(float(color.b), 0.0, 1.0) * 255.0, 0.0)).b,
        color.a
    );
}
"#;

#[derive(Debug, Clone)]
pub(crate) struct Curve {
    points: Vec<(f32, f32)>,
}

impl Curve {
    fn evaluate(&self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        if self.points.is_empty() {
            return x;
        }

        let first = self.points[0];
        if x <= first.0 {
            return first.1;
        }

        for pair in self.points.windows(2) {
            let (x0, y0) = pair[0];
            let (x1, y1) = pair[1];
            if x <= x1 {
                let span = (x1 - x0).max(f32::EPSILON);
                let t = ((x - x0) / span).clamp(0.0, 1.0);
                return y0 + (y1 - y0) * t;
            }
        }

        self.points.last().map(|(_, y)| *y).unwrap_or(x)
    }
}

fn curve_image(
    curves: [&Curve; 3],
    node_id: NodeId,
    frame: u32,
) -> crate::Result<skia_safe::Image> {
    let mut pixels = [0; CURVE_TABLE_SIZE * 4];
    for index in 0..CURVE_TABLE_SIZE {
        let value = index as f32 / (CURVE_TABLE_SIZE - 1) as f32;
        let offset = index * 4;
        pixels[offset] = unit_to_byte(curves[0].evaluate(value));
        pixels[offset + 1] = unit_to_byte(curves[1].evaluate(value));
        pixels[offset + 2] = unit_to_byte(curves[2].evaluate(value));
        pixels[offset + 3] = 255;
    }
    skia_image_from_rgba_upload(
        &pixels,
        CURVE_TABLE_SIZE as u32,
        1,
        CURVE_TABLE_SIZE * 4,
        AlphaMode::Unpremultiplied,
    )
    .ok_or_else(|| invalid_curve_shader(node_id, frame, "curve image creation failed"))
}

pub(crate) fn parse_curve(spec: &str) -> Option<Curve> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Some(identity_curve());
    }

    let mut points = Vec::new();
    for token in spec
        .split([',', ';'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let (x, y) = token
            .split_once(':')
            .or_else(|| token.split_once('='))
            .or_else(|| token.split_once('/'))?;
        let x = x.trim().parse::<f32>().ok()?.clamp(0.0, 1.0);
        let y = y.trim().parse::<f32>().ok()?.clamp(0.0, 1.0);
        points.push((x, y));
    }

    if points.len() < 2 {
        return None;
    }

    points.sort_by(|a, b| a.0.total_cmp(&b.0));
    points.dedup_by(|a, b| {
        if (a.0 - b.0).abs() <= f32::EPSILON {
            b.1 = a.1;
            true
        } else {
            false
        }
    });

    Some(Curve { points })
}

fn identity_curve() -> Curve {
    Curve {
        points: vec![(0.0, 0.0), (1.0, 1.0)],
    }
}

fn parse_curve_property(node_id: NodeId, property_path: &str, spec: &str) -> crate::Result<Curve> {
    parse_curve(spec).ok_or_else(|| invalid_curve(node_id, property_path))
}

fn parse_optional_curve_property(
    node_id: NodeId,
    property_path: &str,
    spec: &str,
    fallback: Curve,
) -> crate::Result<Curve> {
    if spec.trim().is_empty() {
        Ok(fallback)
    } else {
        parse_curve_property(node_id, property_path, spec)
    }
}

fn invalid_curve(node_id: NodeId, property_path: &str) -> LumenError {
    PropertyError::InvalidType {
        node_id,
        property_path: property_path.to_string(),
        expected: "curve points like `0:0,0.5:0.7,1:1`",
        actual: "String",
    }
    .into()
}

fn invalid_curve_shader(node_id: NodeId, frame: u32, details: impl Into<String>) -> LumenError {
    crate::error::RenderError::NodeEvaluation {
        frame,
        node_id,
        node_kind: "Curves",
        details: details.into(),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{gpu_image::AlphaMode, node::processing::test_support};
    #[test]
    fn curves_sksl_samples_curve_texture_per_channel() {
        let red = parse_curve("0:0,0.5:1,1:1").unwrap();
        let green = parse_curve("0:1,1:0").unwrap();
        let blue = parse_curve("").unwrap();
        let source = test_support::frame_from_pixel([128, 64, 192, 200], AlphaMode::Premultiplied);
        let curve_image = curve_image([&red, &green, &blue], NodeId::new(1), 0).unwrap();
        let curve_shader = curve_image
            .to_shader(
                Some((TileMode::Clamp, TileMode::Clamp)),
                SamplingOptions::default(),
                None,
            )
            .expect("curve shader");

        let output = test_support::with_test_context(1, 1, |ctx| {
            apply_runtime_shader_with_children(
                &source,
                CURVES_SHADER,
                &[],
                &[ChildShader {
                    name: "curves",
                    shader: curve_shader,
                }],
                source.alpha_mode(),
                NodeId::new(1),
                "Curves",
                ctx.frame,
                ctx,
            )
        })
        .expect("shader output");

        test_support::assert_pixel_near(
            test_support::read_first_pixel(&output),
            [255, 191, 192, 200],
            1,
        );
    }
}
