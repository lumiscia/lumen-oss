//! Composition root model holding graph, timeline, render settings, animation tracks, and expressions.

use std::collections::{HashMap, HashSet};

use crate::{
    animation::KeyframeTrack,
    capability::RuntimeCapabilityProfile,
    error::{LumenError, PropertyError, Warning},
    expr::{Expression, expression_value_to_property_value},
    graph::Graph,
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
        }
    }

    pub fn add_track(&mut self, track: KeyframeTrack) {
        self.tracks.push(track)
    }

    pub fn set_expression(
        &mut self,
        node_id: NodeId,
        property_path: impl Into<String>,
        expression: Expression,
    ) {
        self.expressions
            .entry(node_id)
            .or_default()
            .insert(property_path.into(), expression);
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

            let mut seen_frames = HashSet::new();
            for key in &track.keys {
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
        if let Some(track) = self
            .tracks
            .iter()
            .find(|track| track.node_id == node_id && track.property_path.0 == property_path)
        {
            return track.sample(frame);
        }

        let node = self
            .graph
            .nodes
            .get(&node_id)
            .ok_or(PropertyError::MissingProperty {
                node_id,
                property_path: property_path.to_string(),
            })?;

        static_property_value(&node.kind, property_path).ok_or(
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
}

fn is_valid_property_path(node_kind: &NodeKind, property_path: &str) -> bool {
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

fn static_property_value(node_kind: &NodeKind, property_path: &str) -> Option<PropertyValue> {
    match node_kind {
        NodeKind::Transform(Transform {
            scale_x,
            scale_y,
            translate_x,
            translate_y,
            rotate,
            pivot_x,
            pivot_y,
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
