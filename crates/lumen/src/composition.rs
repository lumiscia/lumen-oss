//! Composition root model holding graph, timeline, render settings, animation tracks, and expressions.

use std::{
    collections::{HashMap, HashSet},
    hash::{DefaultHasher, Hash, Hasher},
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    animation::{AnimatableType, Extrapolation, InterpolationMode, KeyframeTrack},
    capability::RuntimeCapabilityProfile,
    error::{LumenError, PropertyError, Warning},
    expr::{Expression, expression_value_to_property_value},
    graph::{Graph, InputPort, OutputPort},
    node::{
        NodeId, NodeKind, PropertyValue, merge::Merge, solid_color::SolidColor,
        transform::Transform,
    },
    render::RenderContext,
};

#[derive(Debug, Clone)]
pub struct TimelineSettings {
    pub fps: f32,
    pub duration_frames: u32,
}

impl TimelineSettings {
    pub fn time_seconds(&self, frame: u32) -> f64 {
        if self.fps <= 0.0 {
            return 0.0;
        }
        frame as f64 / self.fps as f64
    }
}

#[derive(Debug, Clone)]
pub struct RenderSettings {
    pub width: u32,
    pub height: u32,
    pub background_color: [u8; 4],
}

#[derive(Debug, Clone, Default)]
pub struct CompositionMetadata {
    pub name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Composition {
    pub graph: Graph,
    pub timeline: TimelineSettings,
    pub render_settings: RenderSettings,
    pub tracks: Vec<KeyframeTrack>,
    pub expressions: HashMap<NodeId, HashMap<String, Expression>>,
    pub metadata: Option<CompositionMetadata>,
    /// Cached graph revision. 0 means not computed yet.
    cached_graph_revision: std::sync::Arc<AtomicU64>,
    /// Cached media output node ID. 0 means not computed yet.
    pub(crate) cached_media_output: std::sync::Arc<AtomicU64>,
}

impl Composition {
    pub fn new(graph: Graph, timeline: TimelineSettings, render_settings: RenderSettings) -> Self {
        Self {
            graph,
            timeline,
            render_settings,
            tracks: Vec::new(),
            expressions: HashMap::new(),
            metadata: None,
            cached_graph_revision: std::sync::Arc::new(AtomicU64::new(0)),
            cached_media_output: std::sync::Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn add_track(&mut self, track: KeyframeTrack) {
        self.cached_graph_revision.store(0, Ordering::Relaxed);
        self.tracks.push(track)
    }

    pub fn set_expression(
        &mut self,
        node_id: NodeId,
        property_path: impl Into<String>,
        expression: Expression,
    ) {
        self.cached_graph_revision.store(0, Ordering::Relaxed);
        self.expressions
            .entry(node_id)
            .or_default()
            .insert(property_path.into(), expression);
    }

    /// Invalidate the cached graph revision. Call after mutating the graph directly.
    pub fn invalidate_revision(&self) {
        self.cached_graph_revision.store(0, Ordering::Relaxed);
    }

    /// Get the graph revision hash, computing and caching it if needed.
    pub fn graph_revision(&self) -> u64 {
        let cached = self.cached_graph_revision.load(Ordering::Relaxed);
        if cached != 0 {
            return cached;
        }
        let revision = self.compute_graph_revision();
        // Avoid storing 0 (sentinel) as a valid revision
        let revision = if revision == 0 { 1 } else { revision };
        self.cached_graph_revision
            .store(revision, Ordering::Relaxed);
        revision
    }

    pub fn validate(
        &self,
        profile: &RuntimeCapabilityProfile,
    ) -> Result<Vec<Warning>, Vec<LumenError>> {
        let mut warnings = Vec::new();
        let mut errors = Vec::new();

        match self.validate_structure() {
            Ok(graph_warnings) => warnings.extend(graph_warnings),
            Err(graph_errors) => errors.extend(graph_errors),
        }

        match self.validate_against_profile(profile) {
            Ok(capability_warnings) => warnings.extend(capability_warnings),
            Err(capability_errors) => errors.extend(capability_errors),
        }

        if errors.is_empty() {
            Ok(warnings)
        } else {
            Err(errors)
        }
    }

    pub fn validate_structure(&self) -> Result<Vec<Warning>, Vec<LumenError>> {
        let mut warnings = Vec::new();
        let mut errors = Vec::new();
        let mut seen_track_targets: HashSet<(NodeId, String)> = HashSet::new();

        match self.graph.validate() {
            Ok(graph_warnings) => warnings.extend(graph_warnings),
            Err(graph_errors) => errors.extend(graph_errors),
        }

        for track in &self.tracks {
            let Some(node) = self.graph.nodes.get(&track.node_id) else {
                errors.push(
                    PropertyError::MissingProperty {
                        node_id: track.node_id,
                        property_path: track.property_path.0.clone(),
                    }
                    .into(),
                );
                continue;
            };

            if track.property_path.0.trim().is_empty() {
                errors.push(
                    PropertyError::MissingProperty {
                        node_id: track.node_id,
                        property_path: track.property_path.0.clone(),
                    }
                    .into(),
                );
            }

            if track.keys.is_empty() {
                errors.push(
                    PropertyError::MissingProperty {
                        node_id: track.node_id,
                        property_path: track.property_path.0.clone(),
                    }
                    .into(),
                );
            }

            if !is_valid_property_path(&node.kind, &track.property_path.0) {
                errors.push(
                    PropertyError::InvalidType {
                        node_id: track.node_id,
                        property_path: track.property_path.0.clone(),
                        expected: "known animatable property",
                        actual: "unsupported property path",
                    }
                    .into(),
                );
            }

            let canonical_track_path =
                canonical_property_path(&node.kind, &track.property_path.0).to_string();
            if !seen_track_targets.insert((track.node_id, canonical_track_path)) {
                errors.push(
                    PropertyError::InvalidType {
                        node_id: track.node_id,
                        property_path: track.property_path.0.clone(),
                        expected: "single track per node/property target",
                        actual: "duplicate track target",
                    }
                    .into(),
                );
            }

            if let Some(expected_type) =
                expected_animatable_type(&node.kind, &track.property_path.0)
                && track.value_type != expected_type
            {
                errors.push(
                    PropertyError::InvalidType {
                        node_id: track.node_id,
                        property_path: track.property_path.0.clone(),
                        expected: expected_animatable_type_name(expected_type),
                        actual: animatable_type_name(track.value_type),
                    }
                    .into(),
                );
            }

            let mut seen_frames = HashSet::new();
            let mut previous_frame = None;
            for key in &track.keys {
                if let Some(previous) = previous_frame
                    && key.time_frame < previous
                {
                    errors.push(
                        PropertyError::InvalidType {
                            node_id: track.node_id,
                            property_path: track.property_path.0.clone(),
                            expected: "sorted ascending frame times",
                            actual: "unsorted frame time",
                        }
                        .into(),
                    );
                }
                previous_frame = Some(key.time_frame);

                if !seen_frames.insert(key.time_frame) {
                    errors.push(
                        PropertyError::InvalidType {
                            node_id: track.node_id,
                            property_path: track.property_path.0.clone(),
                            expected: "unique frame times",
                            actual: "duplicate frame time",
                        }
                        .into(),
                    );
                }

                if matches!(key.interpolation, InterpolationMode::Linear)
                    && !matches!(
                        track.value_type,
                        AnimatableType::Float | AnimatableType::Int | AnimatableType::Color
                    )
                {
                    errors.push(
                        PropertyError::InvalidType {
                            node_id: track.node_id,
                            property_path: track.property_path.0.clone(),
                            expected: "linear interpolation on Float, Int, or Color track",
                            actual: "unsupported linear interpolation type",
                        }
                        .into(),
                    );
                }
            }
        }

        for (node_id, expression_map) in &self.expressions {
            let Some(node) = self.graph.nodes.get(node_id) else {
                errors.push(
                    PropertyError::MissingProperty {
                        node_id: *node_id,
                        property_path: "expression target node".to_string(),
                    }
                    .into(),
                );
                continue;
            };

            for property_path in expression_map.keys() {
                if !is_valid_property_path(&node.kind, property_path) {
                    errors.push(
                        PropertyError::InvalidType {
                            node_id: *node_id,
                            property_path: property_path.clone(),
                            expected: "known animatable property",
                            actual: "unsupported property path",
                        }
                        .into(),
                    );
                }
            }
        }

        if errors.is_empty() {
            Ok(warnings)
        } else {
            Err(errors)
        }
    }

    pub fn sample_property(
        &self,
        node_id: NodeId,
        property_path: &str,
        frame: u32,
        context: &RenderContext,
    ) -> Result<PropertyValue, LumenError> {
        if let Some(expression) = self
            .expressions
            .get(&node_id)
            .and_then(|map| map.get(property_path))
        {
            let evaluated = expression.evaluate_with_context(
                context,
                Some(self),
                Some(node_id),
                Some(property_path.to_string()),
            )?;
            return Ok(expression_value_to_property_value(&evaluated));
        }

        self.sample_property_without_expressions(node_id, property_path, frame)
    }

    pub fn sample_property_without_expressions(
        &self,
        node_id: NodeId,
        property_path: &str,
        frame: u32,
    ) -> Result<PropertyValue, LumenError> {
        let node = self
            .graph
            .nodes
            .get(&node_id)
            .ok_or(PropertyError::MissingProperty {
                node_id,
                property_path: property_path.to_string(),
            })?;
        let canonical_path = canonical_property_path(&node.kind, property_path);

        if let Some(track) = self.tracks.iter().find(|track| {
            track.node_id == node_id
                && canonical_property_path(&node.kind, &track.property_path.0) == canonical_path
        }) {
            if track.keys.len() > 1 {
                if let Some(first_key) = track.keys.first()
                    && frame <= first_key.time_frame
                    && matches!(track.before_extrapolation, Extrapolation::DefaultValue)
                    && let Some(default_value) = static_property_value(&node.kind, canonical_path)
                {
                    return Ok(default_value);
                }

                if let Some(last_key) = track.keys.last()
                    && frame >= last_key.time_frame
                    && matches!(track.after_extrapolation, Extrapolation::DefaultValue)
                    && let Some(default_value) = static_property_value(&node.kind, canonical_path)
                {
                    return Ok(default_value);
                }
            }

            return track.sample(frame);
        }

        static_property_value(&node.kind, canonical_path).ok_or(
            PropertyError::MissingProperty {
                node_id,
                property_path: property_path.to_string(),
            }
            .into(),
        )
    }

    pub fn apply_animated_properties(
        &self,
        node_id: NodeId,
        frame: u32,
        node_kind: &mut NodeKind,
        context: &RenderContext,
    ) -> Result<(), LumenError> {
        let mut paths: HashSet<String> = self
            .tracks
            .iter()
            .filter(|track| track.node_id == node_id)
            .map(|track| track.property_path.0.clone())
            .collect();
        if let Some(expression_paths) = self.expressions.get(&node_id) {
            paths.extend(expression_paths.keys().cloned());
        }

        for property_path in paths {
            let sampled = self.sample_property(node_id, &property_path, frame, context)?;
            apply_property(node_kind, &property_path, sampled);
        }
        Ok(())
    }

    fn compute_graph_revision(&self) -> u64 {
        let mut hasher = DefaultHasher::new();

        let mut node_ids: Vec<NodeId> = self.graph.nodes.keys().copied().collect();
        node_ids.sort_unstable();
        for node_id in node_ids {
            node_id.hash(&mut hasher);
            if let Some(node) = self.graph.nodes.get(&node_id) {
                node.kind.kind_name().hash(&mut hasher);
                node.kind.hash_content(&mut hasher);
            }
        }

        let mut connections: Vec<_> = self.graph.connections.iter().collect();
        connections.sort_by(|left, right| {
            (
                left.from_node,
                left.to_node,
                sortable_output_port_key(&left.from_port),
                sortable_input_port_key(&left.to_port),
            )
                .cmp(&(
                    right.from_node,
                    right.to_node,
                    sortable_output_port_key(&right.from_port),
                    sortable_input_port_key(&right.to_port),
                ))
        });
        for connection in connections {
            connection.from_node.hash(&mut hasher);
            connection.to_node.hash(&mut hasher);
            hash_output_port(&connection.from_port, &mut hasher);
            hash_input_port(&connection.to_port, &mut hasher);
        }

        let mut tracks: Vec<_> = self.tracks.iter().collect();
        tracks.sort_by(|left, right| {
            (left.id, left.node_id, left.property_path.0.as_str()).cmp(&(
                right.id,
                right.node_id,
                right.property_path.0.as_str(),
            ))
        });
        for track in tracks {
            track.node_id.hash(&mut hasher);
            track.id.hash(&mut hasher);
            track.property_path.0.hash(&mut hasher);
            for key in &track.keys {
                key.time_frame.hash(&mut hasher);
                format!("{:?}", key.value).hash(&mut hasher);
            }
        }

        let mut expression_node_ids: Vec<_> = self.expressions.keys().copied().collect();
        expression_node_ids.sort_unstable();
        for node_id in expression_node_ids {
            node_id.hash(&mut hasher);
            if let Some(expressions) = self.expressions.get(&node_id) {
                let mut entries: Vec<_> = expressions.iter().collect();
                entries.sort_by(|(left_path, _), (right_path, _)| left_path.cmp(right_path));
                for (path, expression) in entries {
                    path.hash(&mut hasher);
                    expression.source.hash(&mut hasher);
                }
            }
        }

        hasher.finish()
    }
}

pub(crate) fn hash_input_port(port: &InputPort, hasher: &mut DefaultHasher) {
    match port {
        InputPort::Named(name) => {
            "named".hash(hasher);
            name.hash(hasher);
        }
        InputPort::Indexed(index) => {
            "indexed".hash(hasher);
            index.hash(hasher);
        }
    }
}

pub(crate) fn hash_output_port(port: &OutputPort, hasher: &mut DefaultHasher) {
    match port {
        OutputPort::Named(name) => {
            "named".hash(hasher);
            name.hash(hasher);
        }
        OutputPort::Indexed(index) => {
            "indexed".hash(hasher);
            index.hash(hasher);
        }
    }
}

pub(crate) fn sortable_input_port_key(port: &InputPort) -> (u8, String) {
    match port {
        InputPort::Named(name) => (0, name.clone()),
        InputPort::Indexed(index) => (1, index.to_string()),
    }
}

pub(crate) fn sortable_output_port_key(port: &OutputPort) -> (u8, String) {
    match port {
        OutputPort::Named(name) => (0, name.clone()),
        OutputPort::Indexed(index) => (1, index.to_string()),
    }
}

fn is_valid_property_path(node_kind: &NodeKind, property_path: &str) -> bool {
    let property_path = canonical_property_path(node_kind, property_path);
    match node_kind {
        NodeKind::Transform(_) => matches!(
            property_path,
            "scale_x"
                | "scale_y"
                | "translate_x"
                | "translate_y"
                | "rotate"
                | "pivot_x"
                | "pivot_y"
        ),
        NodeKind::SolidColor(_) => matches!(property_path, "width" | "height"),
        NodeKind::Merge(_) => matches!(property_path, "opacity"),
        _ => false,
    }
}

fn expected_animatable_type(node_kind: &NodeKind, property_path: &str) -> Option<AnimatableType> {
    let property_path = canonical_property_path(node_kind, property_path);
    match node_kind {
        NodeKind::Transform(_) => match property_path {
            "scale_x" | "scale_y" | "translate_x" | "translate_y" | "rotate" | "pivot_x"
            | "pivot_y" => Some(AnimatableType::Float),
            _ => None,
        },
        NodeKind::SolidColor(_) => match property_path {
            "width" | "height" => Some(AnimatableType::Int),
            _ => None,
        },
        NodeKind::Merge(_) if property_path == "opacity" => Some(AnimatableType::Float),
        _ => None,
    }
}

fn animatable_type_name(value_type: AnimatableType) -> &'static str {
    match value_type {
        AnimatableType::Float => "Float",
        AnimatableType::Int => "Int",
        AnimatableType::Boolean => "Boolean",
        AnimatableType::Color => "Color",
        AnimatableType::Vector2 => "Vector2",
        AnimatableType::String => "String",
    }
}

fn expected_animatable_type_name(value_type: AnimatableType) -> &'static str {
    animatable_type_name(value_type)
}

fn static_property_value(node_kind: &NodeKind, property_path: &str) -> Option<PropertyValue> {
    let property_path = canonical_property_path(node_kind, property_path);
    match node_kind {
        NodeKind::Transform(Transform {
            scale_x,
            scale_y,
            translate_x,
            translate_y,
            rotate,
            pivot_x,
            pivot_y,
            ..
        }) => match property_path {
            "scale_x" => Some(PropertyValue::Float(f64::from(*scale_x))),
            "scale_y" => Some(PropertyValue::Float(f64::from(*scale_y))),
            "translate_x" => Some(PropertyValue::Float(f64::from(*translate_x))),
            "translate_y" => Some(PropertyValue::Float(f64::from(*translate_y))),
            "rotate" => Some(PropertyValue::Float(f64::from(*rotate))),
            "pivot_x" => Some(PropertyValue::Float(f64::from(*pivot_x))),
            "pivot_y" => Some(PropertyValue::Float(f64::from(*pivot_y))),
            _ => None,
        },
        NodeKind::SolidColor(SolidColor { width, height, .. }) => match property_path {
            "width" => width.map(|value| PropertyValue::Int(i64::from(value))),
            "height" => height.map(|value| PropertyValue::Int(i64::from(value))),
            _ => None,
        },
        NodeKind::Merge(Merge { opacity, .. }) if property_path == "opacity" => {
            Some(PropertyValue::Float(f64::from(*opacity)))
        }
        _ => None,
    }
}

fn apply_property(node_kind: &mut NodeKind, property_path: &str, value: PropertyValue) {
    let property_path = canonical_property_path(node_kind, property_path);
    match node_kind {
        NodeKind::Transform(transform) => {
            if let PropertyValue::Float(number) = value {
                let value = number as f32;
                match property_path {
                    "scale_x" => transform.scale_x = value,
                    "scale_y" => transform.scale_y = value,
                    "translate_x" => transform.translate_x = value,
                    "translate_y" => transform.translate_y = value,
                    "rotate" => transform.rotate = value,
                    "pivot_x" => transform.pivot_x = value,
                    "pivot_y" => transform.pivot_y = value,
                    _ => {}
                }
            }
        }
        NodeKind::Merge(merge) if property_path == "opacity" => {
            if let PropertyValue::Float(number) = value {
                merge.opacity = number as f32;
            }
        }
        _ => {}
    }
}

fn canonical_property_path<'a>(node_kind: &NodeKind, property_path: &'a str) -> &'a str {
    match node_kind {
        NodeKind::Transform(_) => property_path
            .strip_prefix("transform.")
            .unwrap_or(property_path),
        NodeKind::SolidColor(_) => property_path
            .strip_prefix("solid_color.")
            .or_else(|| property_path.strip_prefix("solidColor."))
            .unwrap_or(property_path),
        NodeKind::Merge(_) => property_path
            .strip_prefix("merge.")
            .unwrap_or(property_path),
        _ => property_path,
    }
}
