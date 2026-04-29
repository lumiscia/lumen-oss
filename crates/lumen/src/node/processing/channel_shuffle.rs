use crate::{
    error::{LumenError, PropertyError},
    node::{
        NodeId, NodeProperty, PortRef,
        processing::gpu_shader::{ShaderUniform, apply_runtime_shader},
    },
    raster::RasterFrame,
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Node)]
pub struct ChannelShuffle {
    pub id: NodeId,

    #[property(expected = String)]
    pub red: NodeProperty,
    #[property(expected = String)]
    pub green: NodeProperty,
    #[property(expected = String)]
    pub blue: NodeProperty,
    #[property(expected = String)]
    pub alpha: NodeProperty,

    #[input(kind = Raster)]
    pub source: PortRef,
}

impl Default for ChannelShuffle {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            red: NodeProperty::String("red".to_string()),
            green: NodeProperty::String("green".to_string()),
            blue: NodeProperty::String("blue".to_string()),
            alpha: NodeProperty::String("alpha".to_string()),
            source: PortRef::empty(),
        }
    }
}

#[node_impl]
impl ChannelShuffle {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<RasterFrame> {
        let selectors = [
            parse_selector(self.id, "red", &self.resolve_red(ctx)?)?,
            parse_selector(self.id, "green", &self.resolve_green(ctx)?)?,
            parse_selector(self.id, "blue", &self.resolve_blue(ctx)?)?,
            parse_selector(self.id, "alpha", &self.resolve_alpha(ctx)?)?,
        ];
        let source_result = ctx.eval(&self.source)?;
        let source = source_result.as_raster()?;
        let selector_indices = selectors.map(|selector| selector.shader_index());
        let selector_values = selectors.map(|selector| selector.shader_value());

        apply_runtime_shader(
            source,
            CHANNEL_SHUFFLE_SHADER,
            &[
                ShaderUniform {
                    name: "selector_indices",
                    values: &selector_indices,
                },
                ShaderUniform {
                    name: "selector_values",
                    values: &selector_values,
                },
            ],
            source.alpha_mode(),
            self.id,
            "ChannelShuffle",
            ctx.frame,
            ctx,
        )
    }
}

const CHANNEL_SHUFFLE_SHADER: &str = r#"
uniform shader source;
uniform float selector_indices[4];
uniform float selector_values[4];

float select_channel(float4 color, float selectorIndex, float selectorValue) {
    if (selectorIndex < 0.5) {
        return color.r;
    }
    if (selectorIndex < 1.5) {
        return color.g;
    }
    if (selectorIndex < 2.5) {
        return color.b;
    }
    if (selectorIndex < 3.5) {
        return color.a;
    }
    return selectorValue;
}

half4 main(float2 coord) {
    half4 color = source.eval(coord);
    float4 rgba = float4(color);
    return half4(
        select_channel(rgba, selector_indices[0], selector_values[0]),
        select_channel(rgba, selector_indices[1], selector_values[1]),
        select_channel(rgba, selector_indices[2], selector_values[2]),
        select_channel(rgba, selector_indices[3], selector_values[3])
    );
}
"#;

#[derive(Debug, Clone, Copy)]
pub(crate) enum ChannelSelector {
    Red,
    Green,
    Blue,
    Alpha,
    Constant(u8),
}

impl ChannelSelector {
    fn shader_index(self) -> f32 {
        match self {
            Self::Red => 0.0,
            Self::Green => 1.0,
            Self::Blue => 2.0,
            Self::Alpha => 3.0,
            Self::Constant(_) => 4.0,
        }
    }

    fn shader_value(self) -> f32 {
        match self {
            Self::Constant(value) => f32::from(value) / 255.0,
            _ => 0.0,
        }
    }
}

pub(crate) fn apply_channel_shuffle_bytes(pixels: &mut [u8], selectors: [ChannelSelector; 4]) {
    for pixel in pixels.chunks_exact_mut(4) {
        let input = [pixel[0], pixel[1], pixel[2], pixel[3]];
        for (index, selector) in selectors.iter().enumerate() {
            pixel[index] = selector.value(input);
        }
    }
}

impl ChannelSelector {
    fn value(self, input: [u8; 4]) -> u8 {
        match self {
            Self::Red => input[0],
            Self::Green => input[1],
            Self::Blue => input[2],
            Self::Alpha => input[3],
            Self::Constant(value) => value,
        }
    }
}

fn parse_selector(
    node_id: NodeId,
    property_path: &str,
    spec: &str,
) -> crate::Result<ChannelSelector> {
    let normalized = spec.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "r" | "red" => Ok(ChannelSelector::Red),
        "g" | "green" => Ok(ChannelSelector::Green),
        "b" | "blue" => Ok(ChannelSelector::Blue),
        "a" | "alpha" => Ok(ChannelSelector::Alpha),
        "zero" => Ok(ChannelSelector::Constant(0)),
        "one" => Ok(ChannelSelector::Constant(255)),
        _ => normalized
            .parse::<f32>()
            .ok()
            .map(|value| {
                if value <= 1.0 {
                    ChannelSelector::Constant((value.clamp(0.0, 1.0) * 255.0).round() as u8)
                } else {
                    ChannelSelector::Constant(value.clamp(0.0, 255.0).round() as u8)
                }
            })
            .ok_or_else(|| invalid_selector(node_id, property_path)),
    }
}

fn invalid_selector(node_id: NodeId, property_path: &str) -> LumenError {
    PropertyError::InvalidType {
        node_id,
        property_path: property_path.to_string(),
        expected: "channel name or numeric constant",
        actual: "String",
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{node::processing::test_support, raster::AlphaMode};

    #[test]
    fn channel_shuffle_maps_channels_and_constants() {
        let mut pixels = vec![10, 20, 30, 40];

        apply_channel_shuffle_bytes(
            &mut pixels,
            [
                ChannelSelector::Blue,
                ChannelSelector::Green,
                ChannelSelector::Red,
                ChannelSelector::Constant(128),
            ],
        );

        assert_eq!(pixels, vec![30, 20, 10, 128]);
    }

    #[test]
    fn channel_shuffle_sksl_maps_channels_and_constants() {
        let source = test_support::frame_from_pixel([10, 20, 30, 40], AlphaMode::Premultiplied);
        let selectors = [
            ChannelSelector::Blue,
            ChannelSelector::Green,
            ChannelSelector::Red,
            ChannelSelector::Constant(128),
        ];
        let selector_indices = selectors.map(|selector| selector.shader_index());
        let selector_values = selectors.map(|selector| selector.shader_value());

        let output = test_support::with_test_context(1, 1, |ctx| {
            apply_runtime_shader(
                &source,
                CHANNEL_SHUFFLE_SHADER,
                &[
                    ShaderUniform {
                        name: "selector_indices",
                        values: &selector_indices,
                    },
                    ShaderUniform {
                        name: "selector_values",
                        values: &selector_values,
                    },
                ],
                source.alpha_mode(),
                NodeId::new(1),
                "ChannelShuffle",
                ctx.frame,
                ctx,
            )
        })
        .expect("shader output");

        assert_eq!(test_support::read_first_pixel(&output), [30, 20, 10, 128]);
    }
}
