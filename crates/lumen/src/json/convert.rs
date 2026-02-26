use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};

use crate::{
    AnimatableType, BlendMode, Composition, CompositionMetadata, Connection, Extrapolation, Graph,
    InputPort, InterpolationMode, Keyframe, KeyframeTrack, LumenError, Node, NodeId, NodeKind,
    OutputPort, PropertyValue, TrackId,
    animation::{
        DynamicAnimationTrack, DynamicBinding, DynamicBindingSource, DynamicKeyValue,
        DynamicKeyframe, PropertyPath, PropertyTarget, VirtualPropertyId,
    },
    error::{ExpressionError, PropertyError},
    expr::ExprNode,
    node::{
        VectorStroke, VectorStyle,
        blur::Blur,
        boolean::{Boolean, MaskKind},
        crop::Crop,
        frame_hold::FrameHold,
        media_in::{LoopMode, MediaIn, MediaInKind},
        media_output::MediaOutput,
        memo::Memo,
        merge::Merge,
        raster_multimerge::RasterMultiMerge,
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
        transform::TransformSampling,
        vector_merge::VectorMerge,
        vector_multimerge::VectorMultiMerge,
        vector_text::VectorText,
        {PropertyValue::Bool, PropertyValue::Color as PropertyColor, PropertyValue::Float},
    },
};

use super::schema::{
    JsonAnimatableType, JsonBlendMode, JsonComposition, JsonComponentDef, JsonComponentInputRef,
    JsonConnectionSource, JsonExtrapolation,
    JsonInterpolationMode, JsonKeyframeTrack, JsonLoopMode, JsonMaskKind, JsonMediaInKind,
    JsonNode, JsonNodeKind, JsonNodeSourceRef, JsonPort, JsonResizeMode, JsonResizeSampling, JsonShapeGeometry,
    JsonTextAlignmentHorizontal, JsonTextAlignmentVertical, JsonTextFontStyle,
    JsonTransformSampling, JsonVectorStroke,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct SymbolicSourceRef {
    node_path: String,
    port: JsonPort,
}

#[derive(Debug)]
struct PendingConnection {
    source: SymbolicSourceRef,
    target_node: NodeId,
    target_port: String,
}

#[derive(Debug)]
struct PendingDynamicBinding {
    binding: DynamicBinding,
    scope_prefix: Vec<String>,
    component_props: HashMap<String, VirtualPropertyId>,
}

#[derive(Debug, Default)]
struct LoweredGraphState {
    graph: Graph,
    pending_connections: Vec<PendingConnection>,
    /// Symbol path + output port key -> final flattened built-in source ref.
    output_sources: HashMap<(String, String), SymbolicSourceRef>,
    /// Absolute symbol path -> runtime node id.
    node_ids_by_symbol_path: HashMap<String, NodeId>,
    pending_bindings: Vec<PendingDynamicBinding>,
}

pub fn convert_json_composition(payload: JsonComposition) -> Result<Composition, Vec<LumenError>> {
    let mut errors = Vec::new();
    let mut lowered = LoweredGraphState::default();

    if let Err(mut lower_errors) = lower_root_graph(
        &payload.graph,
        &payload.components,
        &mut lowered,
    ) {
        errors.append(&mut lower_errors);
    }

    for pending in lowered.pending_connections {
        let source = match resolve_output_source_alias(&pending.source, &lowered.output_sources) {
            Ok(source) => source,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        let Some(from_node_id) = lowered.node_ids_by_symbol_path.get(&source.node_path).copied() else {
            errors.push(
                PropertyError::MissingProperty {
                    node_id: NodeId(0),
                    property_path: format!("connection source `{}`", source.node_path),
                }
                .into(),
            );
            continue;
        };
        if let Err(err) = lowered.graph.connect(Connection {
            from_node: from_node_id,
            from_port: convert_output_port(source.port),
            to_node: pending.target_node,
            to_port: convert_input_port(parse_input_port_key(&pending.target_port)),
        }) {
            errors.push(err);
        }
    }

    let mut composition = Composition::new(
        lowered.graph,
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
    let next_virtual_seed = lowered
        .pending_bindings
        .iter()
        .filter_map(|pending| match pending.binding.target {
            PropertyTarget::VirtualProperty { id } => Some(id.0),
            _ => None,
        })
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    composition.set_next_virtual_property_id_seed(next_virtual_seed);

    for mut pending in lowered.pending_bindings {
        if let Err(err) = resolve_expression_symbols_in_binding(
            &mut pending.binding,
            &pending.scope_prefix,
            &pending.component_props,
            &composition,
            &lowered.node_ids_by_symbol_path,
        ) {
            errors.push(err);
            continue;
        }
        composition.set_dynamic_binding(pending.binding);
    }

    if errors.is_empty() {
        Ok(composition)
    } else {
        Err(errors)
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

fn parse_input_port_key(port: &str) -> JsonPort {
    port.parse::<u16>()
        .map(JsonPort::Indexed)
        .unwrap_or_else(|_| JsonPort::Named(port.to_string()))
}

fn port_key(port: &JsonPort) -> String {
    match port {
        JsonPort::Named(name) => format!("n:{name}"),
        JsonPort::Indexed(index) => format!("i:{index}"),
    }
}

fn join_symbol_path(parts: &[String]) -> String {
    parts.join(".")
}

fn qualify_symbol(scope_prefix: &[String], local_id: &str) -> String {
    if scope_prefix.is_empty() {
        local_id.to_string()
    } else {
        let mut parts = scope_prefix.to_vec();
        parts.push(local_id.to_string());
        join_symbol_path(&parts)
    }
}

fn is_expression_safe_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn lower_root_graph(
    root_nodes: &[JsonNode],
    components: &HashMap<String, JsonComponentDef>,
    lowered: &mut LoweredGraphState,
) -> Result<(), Vec<LumenError>> {
    let mut active_component_stack = Vec::new();
    lower_graph_scope(
        root_nodes,
        components,
        lowered,
        &[],
        &HashMap::new(),
        &HashMap::new(),
        &mut active_component_stack,
    )
}

fn lower_graph_scope(
    nodes: &[JsonNode],
    components: &HashMap<String, JsonComponentDef>,
    lowered: &mut LoweredGraphState,
    scope_prefix: &[String],
    component_props: &HashMap<String, VirtualPropertyId>,
    component_inputs: &HashMap<String, SymbolicSourceRef>,
    active_component_stack: &mut Vec<String>,
) -> Result<(), Vec<LumenError>> {
    let mut errors = Vec::new();
    let mut seen_local_ids = HashSet::new();

    for node in nodes {
        if !is_expression_safe_identifier(&node.id) {
            errors.push(
                PropertyError::InvalidType {
                    node_id: NodeId(0),
                    property_path: "id".to_string(),
                    expected: "expression-safe identifier",
                    actual: "invalid identifier",
                }
                .into(),
            );
            continue;
        }
        if !seen_local_ids.insert(node.id.clone()) {
            errors.push(
                PropertyError::InvalidType {
                    node_id: NodeId(0),
                    property_path: "id".to_string(),
                    expected: "unique node id within scope",
                    actual: "duplicate id",
                }
                .into(),
            );
            continue;
        }

        let kind_type = raw_kind_type(&node.kind).unwrap_or_default();
        if kind_type == "component" {
            if let Err(err) = lower_component_instance_node(
                node,
                components,
                lowered,
                scope_prefix,
                component_props,
                component_inputs,
                active_component_stack,
            ) {
                errors.push(err);
            }
            continue;
        }

        if let Err(err) = lower_builtin_node(
            node,
            lowered,
            scope_prefix,
            component_props,
            component_inputs,
        ) {
            errors.push(err);
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn lower_builtin_node(
    json_node: &JsonNode,
    lowered: &mut LoweredGraphState,
    scope_prefix: &[String],
    component_props: &HashMap<String, VirtualPropertyId>,
    component_inputs: &HashMap<String, SymbolicSourceRef>,
) -> Result<(), LumenError> {
    let absolute_symbol = qualify_symbol(scope_prefix, &json_node.id);
    let (sanitized_kind, raw_bindings) = extract_dynamic_bindings_from_raw_kind(
        &json_node.kind,
        component_props,
        scope_prefix,
    )?;
    let parsed_kind: JsonNodeKind = serde_json::from_value(sanitized_kind).map_err(|_| {
        PropertyError::InvalidType {
            node_id: NodeId(0),
            property_path: format!("{absolute_symbol}.kind"),
            expected: "valid node kind payload",
            actual: "invalid node kind json",
        }
    })?;
    let node_kind = convert_node_kind(parsed_kind)?;
    let runtime_id = lowered.graph.add_node(Node::new(NodeId(0), node_kind));
    lowered
        .node_ids_by_symbol_path
        .insert(absolute_symbol.clone(), runtime_id);
    let inserted_node = lowered
        .graph
        .nodes
        .get(&runtime_id)
        .expect("node inserted")
        .clone();
    register_builtin_node_outputs(
        lowered,
        &absolute_symbol,
        runtime_id,
        &inserted_node,
    );

    for raw_binding in raw_bindings {
        lowered.pending_bindings.push(PendingDynamicBinding {
            binding: DynamicBinding {
                target: PropertyTarget::node_property(runtime_id, raw_binding.property_path),
                value_type: raw_binding.value_type,
                source: raw_binding.source,
                debug_name: Some(raw_binding.debug_name),
            },
            scope_prefix: scope_prefix.to_vec(),
            component_props: component_props.clone(),
        });
    }

    for (target_port, source) in &json_node.inputs {
        let symbolic_source = lower_connection_source_for_scope(
            source,
            scope_prefix,
            component_inputs,
            &absolute_symbol,
        )?;
        lowered.pending_connections.push(PendingConnection {
            source: symbolic_source,
            target_node: runtime_id,
            target_port: target_port.clone(),
        });
    }

    Ok(())
}

fn lower_component_instance_node(
    json_node: &JsonNode,
    components: &HashMap<String, JsonComponentDef>,
    lowered: &mut LoweredGraphState,
    scope_prefix: &[String],
    inherited_component_props: &HashMap<String, VirtualPropertyId>,
    component_inputs: &HashMap<String, SymbolicSourceRef>,
    active_component_stack: &mut Vec<String>,
) -> Result<(), LumenError> {
    let absolute_symbol = qualify_symbol(scope_prefix, &json_node.id);
    let parsed = parse_component_instance_kind(&json_node.kind)?;
    let component_def = components.get(&parsed.component).ok_or(PropertyError::MissingProperty {
        node_id: NodeId(0),
        property_path: format!("{absolute_symbol}.kind.component"),
    })?;

    if active_component_stack.contains(&parsed.component) {
        return Err(ExpressionError::Parse {
            node_id: None,
            property_path: Some(format!("{absolute_symbol}.kind.component")),
            details: format!("recursive component reference detected: {}", parsed.component),
        }
        .into());
    }

    let mut instance_scope_prefix = scope_prefix.to_vec();
    instance_scope_prefix.push(json_node.id.clone());

    let mut instance_inputs = HashMap::new();
    for (input_name, source) in &json_node.inputs {
        let symbolic = lower_connection_source_for_scope(
            source,
            scope_prefix,
            component_inputs,
            &absolute_symbol,
        )?;
        instance_inputs.insert(input_name.clone(), symbolic);
    }

    let mut instance_props = HashMap::new();
    for (prop_name, prop_def) in &component_def.props {
        if !is_expression_safe_identifier(prop_name) {
            return Err(PropertyError::InvalidType {
                node_id: NodeId(0),
                property_path: format!("{absolute_symbol}.props.{prop_name}"),
                expected: "expression-safe identifier",
                actual: "invalid identifier",
            }
            .into());
        }

        let virtual_id = allocate_lowering_virtual_property_id(lowered);
        let raw_value = parsed
            .props
            .get(prop_name)
            .cloned()
            .unwrap_or_else(|| prop_def.default.clone());
        let source = parse_dynamic_source_value(
            &raw_value,
            convert_animatable_type(prop_def.value_type),
            &format!("{absolute_symbol}.props.{prop_name}"),
        )?
        .unwrap_or_else(|| DynamicBindingSource::Literal(
            convert_key_value(&raw_value, convert_animatable_type(prop_def.value_type))
                .unwrap_or(PropertyValue::Int(0)),
        ));
        lowered.pending_bindings.push(PendingDynamicBinding {
            binding: DynamicBinding {
                target: PropertyTarget::VirtualProperty { id: virtual_id },
                value_type: convert_animatable_type(prop_def.value_type),
                source,
                debug_name: Some(format!("{absolute_symbol}.component.{prop_name}")),
            },
            scope_prefix: scope_prefix.to_vec(),
            component_props: inherited_component_props.clone(),
        });
        instance_props.insert(prop_name.clone(), virtual_id);
    }

    active_component_stack.push(parsed.component.clone());
    let lower_result = lower_graph_scope(
        &component_def.graph,
        components,
        lowered,
        &instance_scope_prefix,
        &instance_props,
        &instance_inputs,
        active_component_stack,
    );
    active_component_stack.pop();
    if let Err(errors) = lower_result {
        return Err(errors.into_iter().next().unwrap_or_else(|| {
            PropertyError::InvalidType {
                node_id: NodeId(0),
                property_path: format!("{absolute_symbol}.graph"),
                expected: "valid component graph",
                actual: "lowering failed",
            }
            .into()
        }));
    }

    for (output_name, output_def) in &component_def.outputs {
        let local_source = SymbolicSourceRef {
            node_path: qualify_symbol(&instance_scope_prefix, &output_def.source.node),
            port: output_def.source.port.clone(),
        };
        let resolved = resolve_output_source_alias(&local_source, &lowered.output_sources)?;
        lowered
            .output_sources
            .insert((absolute_symbol.clone(), port_key(&JsonPort::Named(output_name.clone()))), resolved);
    }

    Ok(())
}

#[derive(Debug)]
struct ParsedComponentInstanceKind {
    component: String,
    props: HashMap<String, serde_json::Value>,
}

fn parse_component_instance_kind(kind: &serde_json::Value) -> Result<ParsedComponentInstanceKind, LumenError> {
    let Some(obj) = kind.as_object() else {
        return Err(property_value_error("object node kind"));
    };
    let Some(component) = obj.get("component").and_then(|value| value.as_str()) else {
        return Err(property_value_error("component instance kind"));
    };
    let props = obj
        .get("props")
        .and_then(|value| value.as_object())
        .map(|map| map.iter().map(|(key, value)| (key.clone(), value.clone())).collect())
        .unwrap_or_default();
    Ok(ParsedComponentInstanceKind {
        component: component.to_string(),
        props,
    })
}

fn lower_connection_source_for_scope(
    source: &JsonConnectionSource,
    scope_prefix: &[String],
    component_inputs: &HashMap<String, SymbolicSourceRef>,
    current_node_symbol: &str,
) -> Result<SymbolicSourceRef, LumenError> {
    match source {
        JsonConnectionSource::Node(JsonNodeSourceRef { node, port }) => Ok(SymbolicSourceRef {
            node_path: qualify_symbol(scope_prefix, node),
            port: port.clone(),
        }),
        JsonConnectionSource::ComponentInput(JsonComponentInputRef { component_input }) => {
            component_inputs.get(component_input).cloned().ok_or(
                PropertyError::MissingProperty {
                    node_id: NodeId(0),
                    property_path: format!("{current_node_symbol}.inputs.{component_input}"),
                }
                .into(),
            )
        }
    }
}

fn resolve_output_source_alias(
    source: &SymbolicSourceRef,
    output_sources: &HashMap<(String, String), SymbolicSourceRef>,
) -> Result<SymbolicSourceRef, LumenError> {
    let mut current = source.clone();
    let mut guard = 0usize;
    while let Some(next) = output_sources.get(&(current.node_path.clone(), port_key(&current.port))) {
        if *next == current {
            break;
        }
        current = next.clone();
        guard += 1;
        if guard > 1024 {
            return Err(ExpressionError::Parse {
                node_id: None,
                property_path: Some("components.outputs".to_string()),
                details: "output alias resolution exceeded recursion limit".to_string(),
            }
            .into());
        }
    }
    Ok(current)
}

fn register_builtin_node_outputs(
    lowered: &mut LoweredGraphState,
    symbol_path: &str,
    _runtime_id: NodeId,
    node: &Node,
) {
    for (index, output) in node.kind.output_port_defs().iter().enumerate() {
        let port = JsonPort::Named(output.name.to_string());
        lowered.output_sources.insert(
            (symbol_path.to_string(), port_key(&port)),
            SymbolicSourceRef {
                node_path: symbol_path.to_string(),
                port,
            },
        );
        lowered.output_sources.insert(
            (symbol_path.to_string(), port_key(&JsonPort::Indexed(index as u16))),
            SymbolicSourceRef {
                node_path: symbol_path.to_string(),
                port: JsonPort::Indexed(index as u16),
            },
        );
    }
}

fn raw_kind_type(kind: &serde_json::Value) -> Option<String> {
    kind.as_object()
        .and_then(|obj| obj.get("type"))
        .and_then(|value| value.as_str())
        .map(ToString::to_string)
}

fn allocate_lowering_virtual_property_id(lowered: &mut LoweredGraphState) -> VirtualPropertyId {
    let next = lowered
        .pending_bindings
        .iter()
        .filter_map(|binding| match binding.binding.target {
            PropertyTarget::VirtualProperty { id } => Some(id.0),
            _ => None,
        })
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    VirtualPropertyId(next)
}

#[derive(Debug)]
struct RawPendingNodeBinding {
    property_path: String,
    value_type: AnimatableType,
    source: DynamicBindingSource,
    debug_name: String,
}

fn extract_dynamic_bindings_from_raw_kind(
    raw_kind: &serde_json::Value,
    _component_props: &HashMap<String, VirtualPropertyId>,
    scope_prefix: &[String],
) -> Result<(serde_json::Value, Vec<RawPendingNodeBinding>), LumenError> {
    let Some(kind_type) = raw_kind_type(raw_kind) else {
        return Err(PropertyError::InvalidType {
            node_id: NodeId(0),
            property_path: "kind.type".to_string(),
            expected: "string",
            actual: "missing or invalid",
        }
        .into());
    };

    if kind_type == "component" {
        return Ok((raw_kind.clone(), Vec::new()));
    }

    let mut sanitized = raw_kind.clone();
    let mut bindings = Vec::new();
    let dynamic_specs = supported_dynamic_specs_for_kind(&kind_type);
    for (property_path, value_type) in dynamic_specs {
        if let Some(source) =
            extract_dynamic_binding_at_path(&mut sanitized, property_path, *value_type)?
        {
            let debug_name = if scope_prefix.is_empty() {
                property_path.to_string()
            } else {
                format!("{}.{}", join_symbol_path(scope_prefix), property_path)
            };
            bindings.push(RawPendingNodeBinding {
                property_path: property_path.to_string(),
                value_type: *value_type,
                source,
                debug_name,
            });
        }
    }

    Ok((sanitized, bindings))
}

fn supported_dynamic_specs_for_kind(kind_type: &str) -> &'static [(&'static str, AnimatableType)] {
    match kind_type {
        "shape" => &[
            ("position.x", AnimatableType::Float),
            ("position.y", AnimatableType::Float),
            ("geometry.width", AnimatableType::Int),
            ("geometry.height", AnimatableType::Int),
            ("geometry.border_radius", AnimatableType::Float),
        ],
        "transform" => &[
            ("scale_x", AnimatableType::Float),
            ("scale_y", AnimatableType::Float),
            ("translate_x", AnimatableType::Float),
            ("translate_y", AnimatableType::Float),
            ("rotate", AnimatableType::Float),
            ("pivot_x", AnimatableType::Float),
            ("pivot_y", AnimatableType::Float),
        ],
        "merge" => &[("opacity", AnimatableType::Float)],
        "solid_color" => &[
            ("width", AnimatableType::Int),
            ("height", AnimatableType::Int),
        ],
        _ => &[],
    }
}

fn extract_dynamic_binding_at_path(
    root: &mut serde_json::Value,
    property_path: &str,
    value_type: AnimatableType,
) -> Result<Option<DynamicBindingSource>, LumenError> {
    let mut current = root;
    let mut segments = property_path.split('.').peekable();
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            let Some(obj) = current.as_object_mut() else {
                return Ok(None);
            };
            let Some(value) = obj.get_mut(segment) else {
                return Ok(None);
            };
            let parsed = parse_dynamic_source_value(value, value_type, property_path)?;
            if parsed.is_some() {
                *value = default_json_value_for_animatable_type(value_type);
            }
            return Ok(parsed);
        }

        let Some(obj) = current.as_object_mut() else {
            return Ok(None);
        };
        let Some(next) = obj.get_mut(segment) else {
            return Ok(None);
        };
        current = next;
    }
    Ok(None)
}

fn parse_dynamic_source_value(
    value: &serde_json::Value,
    value_type: AnimatableType,
    property_path: &str,
) -> Result<Option<DynamicBindingSource>, LumenError> {
    let Some(obj) = value.as_object() else {
        return Ok(None);
    };
    if let Some(expr_value) = obj.get("expr") {
        let Some(source) = expr_value.as_str() else {
            return Err(PropertyError::InvalidType {
                node_id: NodeId(0),
                property_path: property_path.to_string(),
                expected: "expression string",
                actual: "non-string expr",
            }
            .into());
        };
        let parsed = crate::Expression::parse(source).map_err(|error| -> LumenError {
            match error {
                ExpressionError::Parse { details, .. } => ExpressionError::Parse {
                    node_id: None,
                    property_path: Some(property_path.to_string()),
                    details,
                }
                .into(),
                other => other.into(),
            }
        })?;
        return Ok(Some(DynamicBindingSource::Expression(parsed)));
    }
    if let Some(anim_value) = obj.get("anim") {
        return Ok(Some(DynamicBindingSource::Animation(parse_inline_animation(
            anim_value,
            value_type,
            property_path,
        )?)));
    }
    Ok(None)
}

fn parse_inline_animation(
    value: &serde_json::Value,
    value_type: AnimatableType,
    property_path: &str,
) -> Result<DynamicAnimationTrack, LumenError> {
    let Some(obj) = value.as_object() else {
        return Err(PropertyError::InvalidType {
            node_id: NodeId(0),
            property_path: property_path.to_string(),
            expected: "animation object",
            actual: "non-object",
        }
        .into());
    };
    let keys_value = obj.get("keys").ok_or(PropertyError::MissingProperty {
        node_id: NodeId(0),
        property_path: format!("{property_path}.anim.keys"),
    })?;
    let Some(keys) = keys_value.as_array() else {
        return Err(property_value_error("animation.keys[]"));
    };

    let mut track = DynamicAnimationTrack::new(value_type);
    if let Some(before) = obj.get("before_extrapolation").and_then(|v| v.as_str()) {
        track.before_extrapolation = parse_extrapolation_str(before, property_path)?;
    }
    if let Some(after) = obj.get("after_extrapolation").and_then(|v| v.as_str()) {
        track.after_extrapolation = parse_extrapolation_str(after, property_path)?;
    }

    for key in keys {
        let Some(key_obj) = key.as_object() else {
            return Err(property_value_error("animation.key"));
        };
        let frame = key_obj
            .get("frame")
            .and_then(|v| v.as_u64())
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| property_value_error("animation.key.frame"))?;
        let interpolation = key_obj
            .get("interpolation")
            .and_then(|v| v.as_str())
            .map(parse_interpolation_str)
            .transpose()?
            .unwrap_or(InterpolationMode::Linear);
        let value = key_obj
            .get("value")
            .ok_or_else(|| property_value_error("animation.key.value"))?;
        let key_value = parse_dynamic_key_value(value, value_type, property_path)?;
        track.keys.push(DynamicKeyframe {
            time_frame: frame,
            value: key_value,
            interpolation,
        });
    }
    track.keys.sort_by_key(|key| key.time_frame);
    Ok(track)
}

fn parse_dynamic_key_value(
    value: &serde_json::Value,
    value_type: AnimatableType,
    property_path: &str,
) -> Result<DynamicKeyValue, LumenError> {
    if let Some(obj) = value.as_object()
        && let Some(expr_value) = obj.get("expr")
    {
        let Some(source) = expr_value.as_str() else {
            return Err(PropertyError::InvalidType {
                node_id: NodeId(0),
                property_path: property_path.to_string(),
                expected: "expression string",
                actual: "non-string expr",
            }
            .into());
        };
        let parsed = crate::Expression::parse(source).map_err(|error| -> LumenError {
            match error {
                ExpressionError::Parse { details, .. } => ExpressionError::Parse {
                    node_id: None,
                    property_path: Some(property_path.to_string()),
                    details,
                }
                .into(),
                other => other.into(),
            }
        })?;
        return Ok(DynamicKeyValue::Expression(parsed));
    }
    Ok(DynamicKeyValue::Literal(convert_key_value(value, value_type)?))
}

fn default_json_value_for_animatable_type(value_type: AnimatableType) -> serde_json::Value {
    match value_type {
        AnimatableType::Float => serde_json::json!(0.0),
        AnimatableType::Int => serde_json::json!(0),
        AnimatableType::Boolean => serde_json::json!(false),
        AnimatableType::Color => serde_json::json!([0, 0, 0, 0]),
        AnimatableType::Vector2 => serde_json::json!([0.0, 0.0]),
        AnimatableType::String => serde_json::json!(""),
    }
}

fn parse_interpolation_str(value: &str) -> Result<InterpolationMode, LumenError> {
    match value {
        "step" => Ok(InterpolationMode::Step),
        "linear" => Ok(InterpolationMode::Linear),
        _ => Err(property_value_error("interpolation(step|linear)")),
    }
}

fn parse_extrapolation_str(value: &str, _property_path: &str) -> Result<Extrapolation, LumenError> {
    match value {
        "hold" => Ok(Extrapolation::Hold),
        "default_value" => Ok(Extrapolation::DefaultValue),
        _ => Err(property_value_error("extrapolation(hold|default_value)")),
    }
}

fn resolve_expression_symbols_in_binding(
    binding: &mut DynamicBinding,
    scope_prefix: &[String],
    component_props: &HashMap<String, VirtualPropertyId>,
    composition: &Composition,
    node_ids_by_symbol_path: &HashMap<String, NodeId>,
) -> Result<(), LumenError> {
    match &mut binding.source {
        DynamicBindingSource::Literal(_) => Ok(()),
        DynamicBindingSource::Expression(expression) => {
            resolve_expression_symbols(
                expression,
                scope_prefix,
                component_props,
                composition,
                node_ids_by_symbol_path,
            )
        }
        DynamicBindingSource::Animation(track) => {
            for key in &mut track.keys {
                if let DynamicKeyValue::Expression(expression) = &mut key.value {
                    resolve_expression_symbols(
                        expression,
                        scope_prefix,
                        component_props,
                        composition,
                        node_ids_by_symbol_path,
                    )?;
                }
            }
            Ok(())
        }
    }
}

fn resolve_expression_symbols(
    expression: &mut crate::Expression,
    scope_prefix: &[String],
    component_props: &HashMap<String, VirtualPropertyId>,
    composition: &Composition,
    node_ids_by_symbol_path: &HashMap<String, NodeId>,
) -> Result<(), LumenError> {
    resolve_expr_node_symbols(
        &mut expression.ast,
        scope_prefix,
        component_props,
        composition,
        node_ids_by_symbol_path,
    )?;
    expression.references.clear();
    collect_expression_references(&expression.ast, &mut expression.references);
    Ok(())
}

fn resolve_expr_node_symbols(
    node: &mut ExprNode,
    scope_prefix: &[String],
    component_props: &HashMap<String, VirtualPropertyId>,
    composition: &Composition,
    node_ids_by_symbol_path: &HashMap<String, NodeId>,
) -> Result<(), LumenError> {
    match node {
        ExprNode::Literal(_) | ExprNode::Global(_) | ExprNode::NodeProperty(_, _) | ExprNode::VirtualProperty(_) => {}
        ExprNode::SymbolicPath(segments) => {
            let resolved = resolve_symbolic_property_reference(
                segments,
                scope_prefix,
                component_props,
                composition,
                node_ids_by_symbol_path,
            )?;
            *node = resolved;
        }
        ExprNode::Unary(_, inner) => {
            resolve_expr_node_symbols(
                inner,
                scope_prefix,
                component_props,
                composition,
                node_ids_by_symbol_path,
            )?;
        }
        ExprNode::Binary(left, _, right) => {
            resolve_expr_node_symbols(
                left,
                scope_prefix,
                component_props,
                composition,
                node_ids_by_symbol_path,
            )?;
            resolve_expr_node_symbols(
                right,
                scope_prefix,
                component_props,
                composition,
                node_ids_by_symbol_path,
            )?;
        }
        ExprNode::Builtin(_, args) => {
            for arg in args {
                resolve_expr_node_symbols(
                    arg,
                    scope_prefix,
                    component_props,
                    composition,
                    node_ids_by_symbol_path,
                )?;
            }
        }
        ExprNode::Conditional(condition, when_true, when_false) => {
            resolve_expr_node_symbols(
                condition,
                scope_prefix,
                component_props,
                composition,
                node_ids_by_symbol_path,
            )?;
            resolve_expr_node_symbols(
                when_true,
                scope_prefix,
                component_props,
                composition,
                node_ids_by_symbol_path,
            )?;
            resolve_expr_node_symbols(
                when_false,
                scope_prefix,
                component_props,
                composition,
                node_ids_by_symbol_path,
            )?;
        }
    }
    Ok(())
}

fn resolve_symbolic_property_reference(
    segments: &[String],
    scope_prefix: &[String],
    component_props: &HashMap<String, VirtualPropertyId>,
    composition: &Composition,
    node_ids_by_symbol_path: &HashMap<String, NodeId>,
) -> Result<ExprNode, LumenError> {
    if segments.first().is_some_and(|segment| segment == "component") {
        if segments.len() != 2 {
            return Err(ExpressionError::Parse {
                node_id: None,
                property_path: None,
                details: format!(
                    "`component` references must be `component.<prop>`; got `{}`",
                    segments.join(".")
                ),
            }
            .into());
        }
        let Some(id) = component_props.get(&segments[1]) else {
            return Err(ExpressionError::Parse {
                node_id: None,
                property_path: None,
                details: format!("unknown component prop `{}`", segments[1]),
            }
            .into());
        };
        return Ok(ExprNode::VirtualProperty(*id));
    }

    if segments.len() < 2 {
        return Err(ExpressionError::Parse {
            node_id: None,
            property_path: None,
            details: format!(
                "unresolved identifier `{}` (expected property reference like node.path)",
                segments.join(".")
            ),
        }
        .into());
    }

    // Prefer local scope, then absolute scope.
    for use_relative in [true, false] {
        for split_index in (1..segments.len()).rev() {
            let node_segments = &segments[..split_index];
            let property_segments = &segments[split_index..];
            let mut absolute_segments = Vec::new();
            if use_relative {
                absolute_segments.extend_from_slice(scope_prefix);
            }
            absolute_segments.extend(node_segments.iter().cloned());
            let symbol_path = join_symbol_path(&absolute_segments);
            let Some(node_id) = node_ids_by_symbol_path.get(&symbol_path).copied() else {
                continue;
            };
            let Some(node_kind) = composition.graph.nodes.get(&node_id).map(|node| &node.kind) else {
                continue;
            };
            let property_path = property_segments.join(".");
            if let Some(canonical) =
                crate::composition::canonicalize_property_path_for_node(node_kind, &property_path)
            {
                return Ok(ExprNode::NodeProperty(node_id, PropertyPath::new(canonical)));
            }
        }
    }

    Err(ExpressionError::Parse {
        node_id: None,
        property_path: None,
        details: format!("unresolved property reference `{}`", segments.join(".")),
    }
    .into())
}

fn collect_expression_references(
    node: &ExprNode,
    out: &mut Vec<crate::ExpressionReference>,
) {
    match node {
        ExprNode::Literal(_) | ExprNode::Global(_) => {}
        ExprNode::SymbolicPath(segments) => out.push(crate::ExpressionReference::SymbolicPath {
            segments: segments.clone(),
        }),
        ExprNode::NodeProperty(node_id, property_path) => out.push(
            crate::ExpressionReference::NodeProperty {
                node_id: *node_id,
                property_path: property_path.clone(),
            },
        ),
        ExprNode::VirtualProperty(id) => {
            out.push(crate::ExpressionReference::VirtualProperty { id: *id });
        }
        ExprNode::Unary(_, inner) => collect_expression_references(inner, out),
        ExprNode::Binary(left, _, right) => {
            collect_expression_references(left, out);
            collect_expression_references(right, out);
        }
        ExprNode::Builtin(_, args) => {
            for arg in args {
                collect_expression_references(arg, out);
            }
        }
        ExprNode::Conditional(condition, when_true, when_false) => {
            collect_expression_references(condition, out);
            collect_expression_references(when_true, out);
            collect_expression_references(when_false, out);
        }
    }
}

fn convert_node_kind(kind: JsonNodeKind) -> Result<NodeKind, LumenError> {
    Ok(match kind {
        JsonNodeKind::Shape {
            geometry,
            position,
            color,
            stroke,
        } => NodeKind::Shape(Shape {
            geometry: match geometry {
                JsonShapeGeometry::Rectangle {
                    width,
                    height,
                    border_radius,
                } => crate::node::ShapeGeometry::Rectangle {
                    width,
                    height,
                    border_radius,
                },
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
            style: VectorStyle {
                color,
                stroke: stroke
                    .map(|JsonVectorStroke { color, width }| VectorStroke { color, width }),
            },
            position: crate::node::VectorPosition {
                x: position.x,
                y: position.y,
            },
        }),
        JsonNodeKind::VectorText {
            content,
            font_family,
            font_size,
            font_weight,
            font_style,
            max_width,
            position,
            color,
            stroke,
            alignment,
        } => NodeKind::VectorText(VectorText {
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
            position: crate::node::VectorPosition {
                x: position.x,
                y: position.y,
            },
            style: VectorStyle {
                color,
                stroke: stroke
                    .map(|JsonVectorStroke { color, width }| VectorStroke { color, width }),
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
            sampling,
        } => NodeKind::Transform(Transform {
            scale_x,
            scale_y,
            translate_x,
            translate_y,
            rotate,
            pivot_x,
            pivot_y,
            sampling: match sampling {
                JsonTransformSampling::Nearest => TransformSampling::Nearest,
                JsonTransformSampling::Bilinear => TransformSampling::Linear,
            },
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
        } => NodeKind::Shadow(Shadow {
            color,
            blur_radius,
            offset_x: offset_x.round() as i32,
            offset_y: offset_y.round() as i32,
        }),
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
        JsonNodeKind::RasterMultiMerge {
            blend_mode,
            opacity,
            input_count,
        } => NodeKind::RasterMultiMerge(RasterMultiMerge {
            opacity,
            input_count,
            blend_mode: match blend_mode {
                JsonBlendMode::Normal => BlendMode::Normal,
                JsonBlendMode::Multiply => BlendMode::Multiply,
                JsonBlendMode::Screen => BlendMode::Screen,
                JsonBlendMode::Overlay => BlendMode::Overlay,
                JsonBlendMode::Darken => BlendMode::Darken,
                JsonBlendMode::Lighten => BlendMode::Lighten,
            },
        }),
        JsonNodeKind::VectorMerge {} => NodeKind::VectorMerge(VectorMerge),
        JsonNodeKind::VectorMultiMerge { input_count } => {
            NodeKind::VectorMultiMerge(VectorMultiMerge { input_count })
        }
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
