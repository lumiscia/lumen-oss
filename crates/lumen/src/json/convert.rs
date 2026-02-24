use std::{collections::HashSet, ops::Range};

use crate::{
    AnimatableType, BlendMode, Composition, CompositionMetadata, Connection, Extrapolation, Graph,
    InputPort, InterpolationMode, Keyframe, KeyframeTrack, LumenError, Node, NodeId, NodeKind,
    OutputPort, PropertyValue, TrackId,
    animation::PropertyPath,
    error::{ExpressionError, PropertyError},
    node::{
        blur::Blur,
        boolean::{Boolean, MaskKind},
        crop::Crop,
        frame_hold::FrameHold,
        media_in::{LoopMode, MediaIn, MediaInKind},
        media_output::MediaOutput,
        memo::Memo,
        merge::Merge,
        resize::{Resize, ResizeMode, ResizeSampling},
        shadow::Shadow,
        shape::Shape,
        shape_renderer::ShapeRenderer,
        solid_color::SolidColor,
        switch::Switch,
        text::{
            Text, TextAlignment, TextAlignmentHorizontal, TextAlignmentVertical, TextFontStyle,
        },
        transform::Transform,
        {PropertyValue::Bool, PropertyValue::Color as PropertyColor, PropertyValue::Float},
    },
};

use super::schema::{
    JsonAnimatableType, JsonBlendMode, JsonComposition, JsonConnection, JsonExtrapolation,
    JsonInterpolationMode, JsonKeyframeTrack, JsonLoopMode, JsonMaskKind, JsonMediaInKind,
    JsonNodeKind, JsonPort, JsonResizeMode, JsonResizeSampling, JsonShapeGeometry,
    JsonTextAlignmentHorizontal, JsonTextAlignmentVertical, JsonTextFontStyle,
};

pub fn convert_json_composition(payload: JsonComposition) -> Result<Composition, Vec<LumenError>> {
    let mut errors = Vec::new();
    let mut graph = Graph::new();
    let mut seen_ids = HashSet::new();

    for json_node in payload.graph.nodes {
        if json_node.id == 0 || !seen_ids.insert(json_node.id) {
            errors.push(
                PropertyError::InvalidType {
                    node_id: NodeId(json_node.id),
                    property_path: "id".to_string(),
                    expected: "unique non-zero node id",
                    actual: "duplicate or zero id",
                }
                .into(),
            );
            continue;
        }

        let node_kind = match convert_node_kind(json_node.kind) {
            Ok(kind) => kind,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        graph.add_node(Node::new(NodeId(json_node.id), node_kind));
    }

    for connection in payload.graph.connections {
        if let Err(error) = graph.connect(convert_connection(connection)) {
            errors.push(error);
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    let mut composition = Composition::new(
        graph,
        crate::composition::TimelineSettings {
            fps: payload.timeline.fps,
            duration_frames: payload.timeline.duration_frames,
        },
        crate::composition::RenderSettings {
            width: payload.render_settings.width,
            height: payload.render_settings.height,
            background_color: payload.render_settings.background_color,
        },
    );
    if let Some(metadata) = payload.metadata {
        composition.metadata = Some(CompositionMetadata {
            name: metadata.name,
        });
    }

    for json_track in payload.tracks {
        match convert_track(json_track) {
            Ok(track) => composition.add_track(track),
            Err(error) => errors.push(error),
        }
    }

    for expression in payload.expressions {
        let node_id = NodeId(expression.node_id);
        match crate::Expression::parse(&expression.source) {
            Ok(parsed) => {
                composition.set_expression(node_id, expression.property_path, parsed);
            }
            Err(ExpressionError::Parse { details, .. }) => errors.push(
                ExpressionError::Parse {
                    node_id: Some(node_id),
                    property_path: Some(expression.property_path),
                    details,
                }
                .into(),
            ),
            Err(error) => errors.push(error.into()),
        }
    }

    if errors.is_empty() {
        Ok(composition)
    } else {
        Err(errors)
    }
}

fn convert_connection(connection: JsonConnection) -> Connection {
    Connection {
        from_node: NodeId(connection.from_node),
        from_port: convert_output_port(connection.from_port),
        to_node: NodeId(connection.to_node),
        to_port: convert_input_port(connection.to_port),
    }
}

fn convert_input_port(port: JsonPort) -> InputPort {
    match port {
        JsonPort::Named(name) => InputPort::Named(name),
        JsonPort::Indexed(index) => InputPort::Indexed(index),
    }
}

fn convert_output_port(port: JsonPort) -> OutputPort {
    match port {
        JsonPort::Named(name) => OutputPort::Named(name),
        JsonPort::Indexed(index) => OutputPort::Indexed(index),
    }
}

fn convert_node_kind(kind: JsonNodeKind) -> Result<NodeKind, LumenError> {
    Ok(match kind {
        JsonNodeKind::Shape { geometry } => NodeKind::Shape(Shape {
            geometry: match geometry {
                JsonShapeGeometry::Rectangle { width, height } => {
                    crate::node::ShapeGeometry::Rectangle { width, height }
                }
                JsonShapeGeometry::Ellipse { width, height } => {
                    crate::node::ShapeGeometry::Ellipse { width, height }
                }
                JsonShapeGeometry::Polygon { points } => crate::node::ShapeGeometry::Polygon {
                    points: points
                        .into_iter()
                        .map(|point| (point[0], point[1]))
                        .collect(),
                },
            },
        }),
        JsonNodeKind::ShapeRenderer {
            fill_color,
            stroke_color,
            stroke_width,
            fill_enabled,
            stroke_enabled,
        } => NodeKind::ShapeRenderer(ShapeRenderer {
            fill_color,
            stroke_color,
            stroke_width,
            fill_enabled,
            stroke_enabled,
        }),
        JsonNodeKind::MediaIn { kind } => NodeKind::MediaIn(MediaIn {
            kind: match kind {
                JsonMediaInKind::Image { source } => MediaInKind::Image { source },
                JsonMediaInKind::Video {
                    source,
                    range,
                    speed,
                    loop_mode,
                } => MediaInKind::Video {
                    source,
                    range: range.map(|range| Range {
                        start: range.start,
                        end: range.end,
                    }),
                    speed,
                    loop_mode: match loop_mode {
                        JsonLoopMode::None => LoopMode::None,
                        JsonLoopMode::Repeat => LoopMode::Repeat,
                        JsonLoopMode::PingPong => LoopMode::PingPong,
                    },
                },
            },
        }),
        JsonNodeKind::SolidColor {
            color,
            width,
            height,
        } => NodeKind::SolidColor(SolidColor {
            color,
            width,
            height,
        }),
        JsonNodeKind::Text {
            content,
            font_family,
            font_size,
            font_weight,
            font_style,
            max_width,
            color,
            alignment,
        } => NodeKind::Text(Text {
            content,
            font_family,
            font_size,
            font_weight,
            font_style: match font_style {
                JsonTextFontStyle::Normal => TextFontStyle::Normal,
                JsonTextFontStyle::Italic => TextFontStyle::Italic,
                JsonTextFontStyle::Oblique => TextFontStyle::Oblique,
            },
            max_width,
            color,
            alignment: TextAlignment {
                horizontal: match alignment.horizontal {
                    JsonTextAlignmentHorizontal::Left => TextAlignmentHorizontal::Left,
                    JsonTextAlignmentHorizontal::Center => TextAlignmentHorizontal::Center,
                    JsonTextAlignmentHorizontal::Right => TextAlignmentHorizontal::Right,
                    JsonTextAlignmentHorizontal::Justify => TextAlignmentHorizontal::Justify,
                },
                vertical: match alignment.vertical {
                    JsonTextAlignmentVertical::Top => TextAlignmentVertical::Top,
                    JsonTextAlignmentVertical::Middle => TextAlignmentVertical::Middle,
                    JsonTextAlignmentVertical::Bottom => TextAlignmentVertical::Bottom,
                },
            },
        }),
        JsonNodeKind::Transform {
            scale_x,
            scale_y,
            translate_x,
            translate_y,
            rotate,
            pivot_x,
            pivot_y,
        } => NodeKind::Transform(Transform {
            scale_x,
            scale_y,
            translate_x,
            translate_y,
            rotate,
            pivot_x,
            pivot_y,
        }),
        JsonNodeKind::Crop {
            x,
            y,
            width,
            height,
        } => NodeKind::Crop(Crop {
            x: i32::try_from(x).map_err(|_| PropertyError::InvalidType {
                node_id: NodeId(0),
                property_path: "crop.x".to_string(),
                expected: "i32",
                actual: "value out of range",
            })?,
            y: i32::try_from(y).map_err(|_| PropertyError::InvalidType {
                node_id: NodeId(0),
                property_path: "crop.y".to_string(),
                expected: "i32",
                actual: "value out of range",
            })?,
            width,
            height,
        }),
        JsonNodeKind::Resize {
            width,
            height,
            mode,
            sampling,
        } => NodeKind::Resize(Resize {
            width,
            height,
            mode: match mode {
                JsonResizeMode::Stretch => ResizeMode::Stretch,
                JsonResizeMode::Fit => ResizeMode::Fit,
                JsonResizeMode::Fill => ResizeMode::Fill,
            },
            sampling: match sampling {
                JsonResizeSampling::Nearest => ResizeSampling::Nearest,
                JsonResizeSampling::Bilinear => ResizeSampling::Linear,
            },
        }),
        JsonNodeKind::Blur { radius } => NodeKind::Blur(Blur { radius }),
        JsonNodeKind::Shadow {
            color,
            blur_radius,
            offset_x,
            offset_y,
        } => {
            let _ = blur_radius;
            NodeKind::Shadow(Shadow {
                color,
                offset_x: offset_x.round() as i32,
                offset_y: offset_y.round() as i32,
            })
        }
        JsonNodeKind::Boolean { mask_kind, invert } => NodeKind::Boolean(Boolean {
            mask_kind: match mask_kind {
                JsonMaskKind::Alpha => MaskKind::Alpha,
                JsonMaskKind::Luma => MaskKind::Luma,
            },
            invert,
        }),
        JsonNodeKind::Merge {
            blend_mode,
            opacity,
        } => NodeKind::Merge(Merge {
            opacity,
            blend_mode: match blend_mode {
                JsonBlendMode::Normal => BlendMode::Normal,
                JsonBlendMode::Multiply => BlendMode::Multiply,
                JsonBlendMode::Screen => BlendMode::Screen,
                JsonBlendMode::Overlay => BlendMode::Overlay,
                JsonBlendMode::Darken => BlendMode::Darken,
                JsonBlendMode::Lighten => BlendMode::Lighten,
            },
        }),
        JsonNodeKind::Switch { map } => {
            let mut parsed = std::collections::HashMap::new();
            for (index, range) in map {
                let input_index = index
                    .parse::<u16>()
                    .map_err(|_| PropertyError::InvalidType {
                        node_id: NodeId(0),
                        property_path: "switch.map".to_string(),
                        expected: "u16 index key",
                        actual: "non-numeric key",
                    })?;
                parsed.insert(input_index, range.start..range.end);
            }
            NodeKind::Switch(Switch { map: parsed })
        }
        JsonNodeKind::FrameHold { hold_frame } => NodeKind::FrameHold(FrameHold { hold_frame }),
        JsonNodeKind::MediaOutput {} => NodeKind::MediaOutput(MediaOutput),
        JsonNodeKind::Memo {
            cache_id,
            allow_expressions,
        } => NodeKind::Memo(Memo {
            cache_id,
            allow_expressions,
        }),
    })
}

fn convert_track(track: JsonKeyframeTrack) -> Result<KeyframeTrack, LumenError> {
    let value_type = convert_animatable_type(track.value_type);
    let mut keyframe_track = KeyframeTrack {
        id: TrackId(track.id),
        node_id: NodeId(track.node_id),
        property_path: PropertyPath::new(track.property_path),
        value_type,
        keys: Vec::with_capacity(track.keys.len()),
        before_extrapolation: convert_extrapolation(track.before_extrapolation),
        after_extrapolation: convert_extrapolation(track.after_extrapolation),
    };

    for key in track.keys {
        keyframe_track.keys.push(Keyframe {
            time_frame: key.time_frame,
            value: convert_key_value(&key.value, value_type)?,
            interpolation: convert_interpolation(key.interpolation),
        });
    }

    Ok(keyframe_track)
}

fn convert_key_value(
    value: &serde_json::Value,
    value_type: AnimatableType,
) -> Result<PropertyValue, LumenError> {
    match value_type {
        AnimatableType::Float => value
            .as_f64()
            .map(Float)
            .ok_or_else(|| property_value_error("float")),
        AnimatableType::Int => value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
            .map(PropertyValue::Int)
            .ok_or_else(|| property_value_error("int")),
        AnimatableType::Boolean => value
            .as_bool()
            .map(Bool)
            .ok_or_else(|| property_value_error("boolean")),
        AnimatableType::String => value
            .as_str()
            .map(|text| PropertyValue::String(text.to_string()))
            .ok_or_else(|| property_value_error("string")),
        AnimatableType::Color => {
            let Some(array) = value.as_array() else {
                return Err(property_value_error("color[4]"));
            };
            if array.len() != 4 {
                return Err(property_value_error("color[4]"));
            }
            let mut color = [0_u8; 4];
            for (index, component) in array.iter().enumerate() {
                let Some(number) = component
                    .as_u64()
                    .and_then(|number| u8::try_from(number).ok())
                else {
                    return Err(property_value_error("color[4]"));
                };
                color[index] = number;
            }
            Ok(PropertyColor(color))
        }
        AnimatableType::Vector2 => {
            let Some(array) = value.as_array() else {
                return Err(property_value_error("vector2[2]"));
            };
            if array.len() != 2 {
                return Err(property_value_error("vector2[2]"));
            }
            let Some(x) = array[0].as_f64() else {
                return Err(property_value_error("vector2[2]"));
            };
            let Some(y) = array[1].as_f64() else {
                return Err(property_value_error("vector2[2]"));
            };
            Ok(PropertyValue::Vector2(x, y))
        }
    }
}

fn property_value_error(expected: &'static str) -> LumenError {
    PropertyError::InvalidType {
        node_id: NodeId(0),
        property_path: "keyframe.value".to_string(),
        expected,
        actual: "invalid json value",
    }
    .into()
}

fn convert_interpolation(mode: JsonInterpolationMode) -> InterpolationMode {
    match mode {
        JsonInterpolationMode::Step => InterpolationMode::Step,
        JsonInterpolationMode::Linear => InterpolationMode::Linear,
    }
}

fn convert_extrapolation(mode: JsonExtrapolation) -> Extrapolation {
    match mode {
        JsonExtrapolation::Hold => Extrapolation::Hold,
        JsonExtrapolation::DefaultValue => Extrapolation::DefaultValue,
    }
}

fn convert_animatable_type(value_type: JsonAnimatableType) -> AnimatableType {
    match value_type {
        JsonAnimatableType::Float => AnimatableType::Float,
        JsonAnimatableType::Int => AnimatableType::Int,
        JsonAnimatableType::Boolean => AnimatableType::Boolean,
        JsonAnimatableType::Color => AnimatableType::Color,
        JsonAnimatableType::Vector2 => AnimatableType::Vector2,
        JsonAnimatableType::String => AnimatableType::String,
    }
}
