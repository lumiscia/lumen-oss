//! Composition root model holding graph, timeline, render settings, animation tracks, and expressions.

use std::{
    collections::{HashMap, HashSet},
    hash::{DefaultHasher, Hash, Hasher},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use crate::{
    animation::{
        AnimatableType, DynamicAnimationTrack, DynamicBinding, DynamicBindingSource,
        DynamicKeyValue, Extrapolation, InterpolationMode, KeyframeTrack, PropertyTarget,
        VirtualPropertyId,
    },
    capability::RuntimeCapabilityProfile,
    error::{LumenError, PropertyError, Warning},
    expr::{Expression, expression_value_to_property_value},
    graph::{Graph, InputPort, OutputPort},
    node::{
        NodeId, NodeKind, PropertyValue, merge::Merge, shape::Shape, solid_color::SolidColor,
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
    /// Unified property dynamics keyed by compiled property target.
    pub dynamic_bindings: HashMap<PropertyTarget, DynamicBinding>,
    pub tracks: Vec<KeyframeTrack>,
    pub expressions: HashMap<NodeId, HashMap<String, Expression>>,
    pub metadata: Option<CompositionMetadata>,
    /// Cached graph revision. 0 means not computed yet.
    cached_graph_revision: Arc<AtomicU64>,
    /// Cached media output node ID. 0 means not computed yet.
    pub(crate) cached_media_output: Arc<AtomicU64>,
    next_virtual_property_id: u64,
}

impl Composition {
    pub fn new(graph: Graph, timeline: TimelineSettings, render_settings: RenderSettings) -> Self {
        Self {
            graph,
            timeline,
            render_settings,
            dynamic_bindings: HashMap::new(),
            tracks: Vec::new(),
            expressions: HashMap::new(),
            metadata: None,
            cached_graph_revision: Arc::new(AtomicU64::new(0)),
            cached_media_output: Arc::new(AtomicU64::new(0)),
            next_virtual_property_id: 1,
        }
    }

    pub fn allocate_virtual_property_id(&mut self) -> VirtualPropertyId {
        let id = VirtualPropertyId::new(self.next_virtual_property_id);
        self.next_virtual_property_id = self.next_virtual_property_id.saturating_add(1);
        id
    }

    pub fn set_dynamic_binding(&mut self, binding: DynamicBinding) {
        self.cached_graph_revision.store(0, Ordering::Relaxed);
        self.dynamic_bindings.insert(binding.target.clone(), binding);
    }

    pub(crate) fn set_next_virtual_property_id_seed(&mut self, next_id: u64) {
        self.next_virtual_property_id = next_id.max(1);
    }

    pub fn add_track(&mut self, track: KeyframeTrack) {
        self.cached_graph_revision.store(0, Ordering::Relaxed);
        let target = PropertyTarget::node_property(track.node_id, track.property_path.0.clone());
        self.dynamic_bindings.insert(
            target.clone(),
            DynamicBinding {
                target,
                value_type: track.value_type,
                source: DynamicBindingSource::Animation(dynamic_track_from_legacy(&track)),
                debug_name: None,
            },
        );
        self.tracks.push(track)
    }

    pub fn set_expression(
        &mut self,
        node_id: NodeId,
        property_path: impl Into<String>,
        expression: Expression,
    ) {
        self.cached_graph_revision.store(0, Ordering::Relaxed);
        let property_path = property_path.into();
        if let Some(node) = self.graph.nodes.get(&node_id)
            && let Some(value_type) = expected_animatable_type(&node.kind, &property_path)
        {
            let target = PropertyTarget::node_property(node_id, property_path.clone());
            self.dynamic_bindings.insert(
                target.clone(),
                DynamicBinding {
                    target,
                    value_type,
                    source: DynamicBindingSource::Expression(expression.clone()),
                    debug_name: None,
                },
            );
        }
        self.expressions
            .entry(node_id)
            .or_default()
            .insert(property_path, expression);
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

        for (target, binding) in &self.dynamic_bindings {
            match target {
                PropertyTarget::NodeProperty {
                    node_id,
                    property_path,
                } => {
                    let Some(node) = self.graph.nodes.get(node_id) else {
                        errors.push(
                            PropertyError::MissingProperty {
                                node_id: *node_id,
                                property_path: property_path.0.clone(),
                            }
                            .into(),
                        );
                        continue;
                    };
                    if !is_valid_property_path(&node.kind, &property_path.0) {
                        errors.push(
                            PropertyError::InvalidType {
                                node_id: *node_id,
                                property_path: property_path.0.clone(),
                                expected: "known animatable property",
                                actual: "unsupported property path",
                            }
                            .into(),
                        );
                    }
                    if let Some(expected_type) =
                        expected_animatable_type(&node.kind, &property_path.0)
                        && expected_type != binding.value_type
                    {
                        errors.push(
                            PropertyError::InvalidType {
                                node_id: *node_id,
                                property_path: property_path.0.clone(),
                                expected: expected_animatable_type_name(expected_type),
                                actual: animatable_type_name(binding.value_type),
                            }
                            .into(),
                        );
                    }
                }
                PropertyTarget::VirtualProperty { .. } => {}
            }

            validate_dynamic_binding(binding, &mut errors);
        }

        if let Some(cycle) = dynamic_binding_cycle(&self.dynamic_bindings) {
            let details = cycle
                .into_iter()
                .map(|target| match target {
                    PropertyTarget::NodeProperty {
                        node_id,
                        property_path,
                    } => format!("{node_id}.{}", property_path.0),
                    PropertyTarget::VirtualProperty { id } => format!("virtual.{}", id.0),
                })
                .collect::<Vec<_>>()
                .join(" -> ");
            errors.push(
                crate::error::ExpressionError::Evaluate {
                    node_id: None,
                    property_path: None,
                    details: format!("dynamic dependency cycle: {details}"),
                }
                .into(),
            );
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
        let mut active_targets = HashSet::new();
        self.sample_node_property_internal(
            node_id,
            property_path,
            frame,
            context,
            None,
            &mut active_targets,
        )
    }

    pub(crate) fn sample_node_property_from_expression(
        &self,
        node_id: NodeId,
        property_path: &str,
        frame: u32,
        context: &RenderContext,
        frame_override: Option<u32>,
        active_targets: &mut HashSet<PropertyTarget>,
    ) -> Result<PropertyValue, LumenError> {
        self.sample_node_property_internal(
            node_id,
            property_path,
            frame,
            context,
            frame_override,
            active_targets,
        )
    }

    pub(crate) fn sample_virtual_property_from_expression(
        &self,
        id: VirtualPropertyId,
        frame: u32,
        context: &RenderContext,
        frame_override: Option<u32>,
        active_targets: &mut HashSet<PropertyTarget>,
    ) -> Result<PropertyValue, LumenError> {
        self.sample_target_internal(
            &PropertyTarget::VirtualProperty { id },
            frame,
            context,
            frame_override,
            active_targets,
        )
    }

    fn sample_node_property_internal(
        &self,
        node_id: NodeId,
        property_path: &str,
        frame: u32,
        context: &RenderContext,
        frame_override: Option<u32>,
        active_targets: &mut HashSet<PropertyTarget>,
    ) -> Result<PropertyValue, LumenError> {
        let node = self
            .graph
            .nodes
            .get(&node_id)
            .ok_or(PropertyError::MissingProperty {
                node_id,
                property_path: property_path.to_string(),
            })?;
        let canonical_path = canonical_property_path(&node.kind, property_path).to_string();
        let target = PropertyTarget::node_property(node_id, canonical_path.clone());
        if self.dynamic_bindings.contains_key(&target) {
            return self.sample_target_internal(
                &target,
                frame,
                context,
                frame_override,
                active_targets,
            );
        }

        if let Some(expression) = self
            .expressions
            .get(&node_id)
            .and_then(|map| map.get(&canonical_path).or_else(|| map.get(property_path)))
        {
            let evaluated = expression.evaluate_with_context(
                context,
                Some(self),
                Some(node_id),
                Some(canonical_path.clone()),
            )?;
            return Ok(expression_value_to_property_value(&evaluated));
        }

        self.sample_property_without_expressions(node_id, &canonical_path, frame)
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

    fn sample_target_internal(
        &self,
        target: &PropertyTarget,
        frame: u32,
        context: &RenderContext,
        frame_override: Option<u32>,
        active_targets: &mut HashSet<PropertyTarget>,
    ) -> Result<PropertyValue, LumenError> {
        if !active_targets.insert(target.clone()) {
            let debug_name = match target {
                PropertyTarget::NodeProperty {
                    node_id,
                    property_path,
                } => format!("{node_id}.{}", property_path.0),
                PropertyTarget::VirtualProperty { id } => format!("virtual.{}", id.0),
            };
            return Err(crate::error::ExpressionError::Evaluate {
                node_id: None,
                property_path: None,
                details: format!("dynamic property dependency cycle detected at `{debug_name}`"),
            }
            .into());
        }

        let result = (|| {
            if let Some(binding) = self.dynamic_bindings.get(target) {
                return self.sample_dynamic_binding_source(
                    target,
                    binding,
                    frame,
                    context,
                    frame_override,
                    active_targets,
                );
            }

            match target {
                PropertyTarget::NodeProperty {
                    node_id,
                    property_path,
                } => {
                    let node =
                        self.graph
                            .nodes
                            .get(node_id)
                            .ok_or(PropertyError::MissingProperty {
                                node_id: *node_id,
                                property_path: property_path.0.clone(),
                            })?;
                    static_property_value(&node.kind, &property_path.0).ok_or(
                        PropertyError::MissingProperty {
                            node_id: *node_id,
                            property_path: property_path.0.clone(),
                        }
                        .into(),
                    )
                }
                PropertyTarget::VirtualProperty { id } => Err(PropertyError::MissingProperty {
                    node_id: NodeId(0),
                    property_path: format!("virtual.{}", id.0),
                }
                .into()),
            }
        })();

        active_targets.remove(target);
        result
    }

    fn sample_dynamic_binding_source(
        &self,
        target: &PropertyTarget,
        binding: &DynamicBinding,
        frame: u32,
        context: &RenderContext,
        frame_override: Option<u32>,
        active_targets: &mut HashSet<PropertyTarget>,
    ) -> Result<PropertyValue, LumenError> {
        match &binding.source {
            DynamicBindingSource::Literal(value) => Ok(value.clone()),
            DynamicBindingSource::Expression(expression) => {
                let (node_id, property_path) = target_debug_context(target);
                let evaluated = expression.evaluate_with_context_and_frame(
                    context,
                    Some(self),
                    node_id,
                    property_path,
                    frame_override,
                    active_targets,
                )?;
                coerce_expression_value_to_animatable_type(&evaluated, binding.value_type)
            }
            DynamicBindingSource::Animation(track) => {
                self.sample_dynamic_animation_track(target, track, frame, context, active_targets)
            }
        }
    }

    fn sample_dynamic_animation_track(
        &self,
        target: &PropertyTarget,
        track: &DynamicAnimationTrack,
        frame: u32,
        context: &RenderContext,
        active_targets: &mut HashSet<PropertyTarget>,
    ) -> Result<PropertyValue, LumenError> {
        if track.keys.is_empty() {
            return Err(PropertyError::MissingProperty {
                node_id: NodeId(0),
                property_path: "animation.keys".to_string(),
            }
            .into());
        }

        if track.keys.len() == 1 {
            return self.sample_dynamic_key_value(
                target,
                &track.keys[0].value,
                track.value_type,
                track.keys[0].time_frame,
                context,
                active_targets,
            );
        }

        let first = &track.keys[0];
        let last = &track.keys[track.keys.len() - 1];

        if frame <= first.time_frame {
            return match track.before_extrapolation {
                Extrapolation::Hold => self.sample_dynamic_key_value(
                    target,
                    &first.value,
                    track.value_type,
                    first.time_frame,
                    context,
                    active_targets,
                ),
                Extrapolation::DefaultValue => self.default_value_for_target(target),
            };
        }

        if frame >= last.time_frame {
            return match track.after_extrapolation {
                Extrapolation::Hold => self.sample_dynamic_key_value(
                    target,
                    &last.value,
                    track.value_type,
                    last.time_frame,
                    context,
                    active_targets,
                ),
                Extrapolation::DefaultValue => self.default_value_for_target(target),
            };
        }

        let mut right_index = 1usize;
        while right_index < track.keys.len() && track.keys[right_index].time_frame < frame {
            right_index += 1;
        }
        let left = &track.keys[right_index - 1];
        let right = &track.keys[right_index];

        if matches!(right.interpolation, InterpolationMode::Step) {
            return self.sample_dynamic_key_value(
                target,
                &left.value,
                track.value_type,
                left.time_frame,
                context,
                active_targets,
            );
        }

        let left_value = self.sample_dynamic_key_value(
            target,
            &left.value,
            track.value_type,
            left.time_frame,
            context,
            active_targets,
        )?;
        let right_value = self.sample_dynamic_key_value(
            target,
            &right.value,
            track.value_type,
            right.time_frame,
            context,
            active_targets,
        )?;
        let range = (right.time_frame - left.time_frame) as f64;
        let t = if range == 0.0 {
            0.0
        } else {
            (frame - left.time_frame) as f64 / range
        };
        Ok(interpolate_property_value(&left_value, &right_value, t))
    }

    fn sample_dynamic_key_value(
        &self,
        target: &PropertyTarget,
        key_value: &DynamicKeyValue,
        expected_type: AnimatableType,
        key_frame: u32,
        context: &RenderContext,
        active_targets: &mut HashSet<PropertyTarget>,
    ) -> Result<PropertyValue, LumenError> {
        match key_value {
            DynamicKeyValue::Literal(value) => Ok(value.clone()),
            DynamicKeyValue::Expression(expression) => {
                let (node_id, property_path) = target_debug_context(target);
                let evaluated = expression.evaluate_with_context_and_frame(
                    context,
                    Some(self),
                    node_id,
                    property_path,
                    Some(key_frame),
                    active_targets,
                )?;
                coerce_expression_value_to_animatable_type(&evaluated, expected_type)
            }
        }
    }

    fn default_value_for_target(&self, target: &PropertyTarget) -> Result<PropertyValue, LumenError> {
        match target {
            PropertyTarget::NodeProperty {
                node_id,
                property_path,
            } => {
                let node = self.graph.nodes.get(node_id).ok_or(PropertyError::MissingProperty {
                    node_id: *node_id,
                    property_path: property_path.0.clone(),
                })?;
                static_property_value(&node.kind, &property_path.0).ok_or(
                    PropertyError::MissingProperty {
                        node_id: *node_id,
                        property_path: property_path.0.clone(),
                    }
                    .into(),
                )
            }
            PropertyTarget::VirtualProperty { id } => {
                Err(PropertyError::MissingProperty {
                    node_id: NodeId(0),
                    property_path: format!("virtual.{}", id.0),
                }
                .into())
            }
        }
    }

    pub fn apply_animated_properties(
        &self,
        node_id: NodeId,
        frame: u32,
        node_kind: &mut NodeKind,
        context: &RenderContext,
    ) -> Result<(), LumenError> {
        let mut paths: HashSet<String> = self
            .dynamic_bindings
            .keys()
            .filter_map(|target| match target {
                PropertyTarget::NodeProperty {
                    node_id: target_node_id,
                    property_path,
                } if *target_node_id == node_id => Some(property_path.0.clone()),
                _ => None,
            })
            .collect();
        paths.extend(
            self.tracks
                .iter()
                .filter(|track| track.node_id == node_id)
                .map(|track| track.property_path.0.clone()),
        );
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

        let mut dynamic_bindings: Vec<_> = self.dynamic_bindings.iter().collect();
        dynamic_bindings.sort_by(|(left_target, _), (right_target, _)| {
            dynamic_target_sort_key(left_target).cmp(&dynamic_target_sort_key(right_target))
        });
        for (target, binding) in dynamic_bindings {
            dynamic_target_sort_key(target).hash(&mut hasher);
            animatable_type_name(binding.value_type).hash(&mut hasher);
            hash_dynamic_binding_source(&binding.source, &mut hasher);
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

    pub(crate) fn dynamic_animated_node_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.dynamic_bindings.keys().filter_map(|target| match target {
            PropertyTarget::NodeProperty { node_id, .. } => Some(*node_id),
            PropertyTarget::VirtualProperty { .. } => None,
        })
    }
}

pub(crate) fn canonicalize_property_path_for_node(
    node_kind: &NodeKind,
    property_path: &str,
) -> Option<String> {
    let canonical = canonical_property_path(node_kind, property_path);
    is_valid_property_path(node_kind, canonical).then(|| canonical.to_string())
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
        NodeKind::Shape(_) => matches!(
            property_path,
            "position.x"
                | "position.y"
                | "geometry.width"
                | "geometry.height"
                | "geometry.border_radius"
        ),
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
        NodeKind::Shape(_) => match property_path {
            "position.x" | "position.y" | "geometry.border_radius" => Some(AnimatableType::Float),
            "geometry.width" | "geometry.height" => Some(AnimatableType::Int),
            _ => None,
        },
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
        NodeKind::Shape(Shape {
            geometry,
            position,
            ..
        }) => match property_path {
            "position.x" => Some(PropertyValue::Float(f64::from(position.x))),
            "position.y" => Some(PropertyValue::Float(f64::from(position.y))),
            "geometry.width" => match geometry {
                crate::node::ShapeGeometry::Rectangle { width, .. }
                | crate::node::ShapeGeometry::Ellipse { width, .. } => {
                    Some(PropertyValue::Int(i64::from(*width)))
                }
                crate::node::ShapeGeometry::Polygon { .. } => None,
            },
            "geometry.height" => match geometry {
                crate::node::ShapeGeometry::Rectangle { height, .. }
                | crate::node::ShapeGeometry::Ellipse { height, .. } => {
                    Some(PropertyValue::Int(i64::from(*height)))
                }
                crate::node::ShapeGeometry::Polygon { .. } => None,
            },
            "geometry.border_radius" => match geometry {
                crate::node::ShapeGeometry::Rectangle { border_radius, .. } => {
                    Some(PropertyValue::Float(f64::from(*border_radius)))
                }
                _ => None,
            },
            _ => None,
        },
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
        NodeKind::Shape(shape) => match (property_path, value) {
            ("position.x", PropertyValue::Float(number)) => shape.position.x = number as f32,
            ("position.y", PropertyValue::Float(number)) => shape.position.y = number as f32,
            ("geometry.width", PropertyValue::Int(number)) => match &mut shape.geometry {
                crate::node::ShapeGeometry::Rectangle { width, .. }
                | crate::node::ShapeGeometry::Ellipse { width, .. } => {
                    if let Ok(parsed) = u32::try_from(number.max(0)) {
                        *width = parsed;
                    }
                }
                crate::node::ShapeGeometry::Polygon { .. } => {}
            },
            ("geometry.height", PropertyValue::Int(number)) => match &mut shape.geometry {
                crate::node::ShapeGeometry::Rectangle { height, .. }
                | crate::node::ShapeGeometry::Ellipse { height, .. } => {
                    if let Ok(parsed) = u32::try_from(number.max(0)) {
                        *height = parsed;
                    }
                }
                crate::node::ShapeGeometry::Polygon { .. } => {}
            },
            ("geometry.border_radius", PropertyValue::Float(number)) => {
                if let crate::node::ShapeGeometry::Rectangle { border_radius, .. } =
                    &mut shape.geometry
                {
                    *border_radius = number as f32;
                }
            }
            _ => {}
        },
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
        NodeKind::Shape(_) => property_path
            .strip_prefix("shape.")
            .or_else(|| property_path.strip_prefix("geometry.pos_x").map(|_| "position.x"))
            .or_else(|| property_path.strip_prefix("geometry.pos_y").map(|_| "position.y"))
            .unwrap_or(property_path),
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

fn dynamic_track_from_legacy(track: &KeyframeTrack) -> DynamicAnimationTrack {
    let mut dynamic = DynamicAnimationTrack::new(track.value_type);
    dynamic.before_extrapolation = track.before_extrapolation;
    dynamic.after_extrapolation = track.after_extrapolation;
    dynamic.keys = track
        .keys
        .iter()
        .map(|key| crate::animation::DynamicKeyframe {
            time_frame: key.time_frame,
            value: DynamicKeyValue::Literal(key.value.clone()),
            interpolation: key.interpolation,
        })
        .collect();
    dynamic
}

fn target_debug_context(target: &PropertyTarget) -> (Option<NodeId>, Option<String>) {
    match target {
        PropertyTarget::NodeProperty {
            node_id,
            property_path,
        } => (Some(*node_id), Some(property_path.0.clone())),
        PropertyTarget::VirtualProperty { id } => (None, Some(format!("virtual.{}", id.0))),
    }
}

fn dynamic_target_sort_key(target: &PropertyTarget) -> (u8, u64, String) {
    match target {
        PropertyTarget::NodeProperty {
            node_id,
            property_path,
        } => (0, node_id.0, property_path.0.clone()),
        PropertyTarget::VirtualProperty { id } => (1, id.0, String::new()),
    }
}

fn hash_dynamic_binding_source(source: &DynamicBindingSource, hasher: &mut DefaultHasher) {
    match source {
        DynamicBindingSource::Literal(value) => {
            "literal".hash(hasher);
            format!("{value:?}").hash(hasher);
        }
        DynamicBindingSource::Expression(expression) => {
            "expr".hash(hasher);
            expression.source.hash(hasher);
        }
        DynamicBindingSource::Animation(track) => {
            "anim".hash(hasher);
            animatable_type_name(track.value_type).hash(hasher);
            for key in &track.keys {
                key.time_frame.hash(hasher);
                (key.interpolation as u8).hash(hasher);
                match &key.value {
                    DynamicKeyValue::Literal(value) => {
                        "literal".hash(hasher);
                        format!("{value:?}").hash(hasher);
                    }
                    DynamicKeyValue::Expression(expression) => {
                        "expr".hash(hasher);
                        expression.source.hash(hasher);
                    }
                }
            }
        }
    }
}

fn interpolate_property_value(
    left: &PropertyValue,
    right: &PropertyValue,
    t: f64,
) -> PropertyValue {
    match (left, right) {
        (PropertyValue::Float(a), PropertyValue::Float(b)) => PropertyValue::Float(a + (b - a) * t),
        (PropertyValue::Int(a), PropertyValue::Int(b)) => {
            let value = *a as f64 + ((*b as f64 - *a as f64) * t);
            PropertyValue::Int(value.round() as i64)
        }
        (PropertyValue::Color(a), PropertyValue::Color(b)) => {
            let lerp = |x: u8, y: u8| -> u8 {
                (x as f64 + (y as f64 - x as f64) * t)
                    .round()
                    .clamp(0.0, 255.0) as u8
            };
            PropertyValue::Color([
                lerp(a[0], b[0]),
                lerp(a[1], b[1]),
                lerp(a[2], b[2]),
                lerp(a[3], b[3]),
            ])
        }
        (PropertyValue::Vector2(ax, ay), PropertyValue::Vector2(bx, by)) => {
            PropertyValue::Vector2(ax + (bx - ax) * t, ay + (by - ay) * t)
        }
        _ => left.clone(),
    }
}

fn coerce_expression_value_to_animatable_type(
    value: &crate::expr::ExpressionValue,
    value_type: AnimatableType,
) -> Result<PropertyValue, LumenError> {
    match value_type {
        AnimatableType::Float => match value {
            crate::expr::ExpressionValue::Number(number) => Ok(PropertyValue::Float(*number)),
            crate::expr::ExpressionValue::Boolean(boolean) => {
                Ok(PropertyValue::Float(if *boolean { 1.0 } else { 0.0 }))
            }
            crate::expr::ExpressionValue::String(text) => text
                .parse::<f64>()
                .map(PropertyValue::Float)
                .map_err(|_| {
                    PropertyError::InvalidType {
                        node_id: NodeId(0),
                        property_path: "expression".to_string(),
                        expected: "numeric expression result",
                        actual: "non-numeric",
                    }
                    .into()
                }),
        },
        AnimatableType::Int => match value {
            crate::expr::ExpressionValue::Number(number) => Ok(PropertyValue::Int(number.round() as i64)),
            crate::expr::ExpressionValue::Boolean(boolean) => {
                Ok(PropertyValue::Int(if *boolean { 1 } else { 0 }))
            }
            crate::expr::ExpressionValue::String(text) => text
                .parse::<i64>()
                .map(PropertyValue::Int)
                .map_err(|_| {
                    PropertyError::InvalidType {
                        node_id: NodeId(0),
                        property_path: "expression".to_string(),
                        expected: "integer expression result",
                        actual: "non-integer",
                    }
                    .into()
                }),
        },
        AnimatableType::Boolean => match value {
            crate::expr::ExpressionValue::Boolean(boolean) => Ok(PropertyValue::Bool(*boolean)),
            crate::expr::ExpressionValue::Number(number) => {
                Ok(PropertyValue::Bool(number.abs() > f64::EPSILON))
            }
            crate::expr::ExpressionValue::String(text) => Ok(PropertyValue::Bool(!text.is_empty())),
        },
        AnimatableType::String => match value {
            crate::expr::ExpressionValue::String(text) => Ok(PropertyValue::String(text.clone())),
            crate::expr::ExpressionValue::Number(number) => {
                Ok(PropertyValue::String(number.to_string()))
            }
            crate::expr::ExpressionValue::Boolean(boolean) => {
                Ok(PropertyValue::String(boolean.to_string()))
            }
        },
        AnimatableType::Color | AnimatableType::Vector2 => Err(PropertyError::InvalidType {
            node_id: NodeId(0),
            property_path: "expression".to_string(),
            expected: "scalar expression result",
            actual: "unsupported target type for expression coercion",
        }
        .into()),
    }
}

fn validate_dynamic_binding(binding: &DynamicBinding, errors: &mut Vec<LumenError>) {
    match &binding.source {
        DynamicBindingSource::Literal(_) => {}
        DynamicBindingSource::Expression(expression) => {
            if expression_references_contain_symbolic_paths(expression) {
                errors.push(
                    crate::error::ExpressionError::Parse {
                        node_id: None,
                        property_path: Some(binding_debug_path(binding)),
                        details: "unresolved symbolic expression reference".to_string(),
                    }
                    .into(),
                );
            }
        }
        DynamicBindingSource::Animation(track) => {
            if track.keys.is_empty() {
                errors.push(
                    PropertyError::MissingProperty {
                        node_id: NodeId(0),
                        property_path: format!("{}.anim.keys", binding_debug_path(binding)),
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
                            node_id: NodeId(0),
                            property_path: binding_debug_path(binding),
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
                            node_id: NodeId(0),
                            property_path: binding_debug_path(binding),
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
                            node_id: NodeId(0),
                            property_path: binding_debug_path(binding),
                            expected: "linear interpolation on Float, Int, or Color track",
                            actual: "unsupported linear interpolation type",
                        }
                        .into(),
                    );
                }
                if let DynamicKeyValue::Expression(expression) = &key.value
                    && expression_references_contain_symbolic_paths(expression)
                {
                    errors.push(
                        crate::error::ExpressionError::Parse {
                            node_id: None,
                            property_path: Some(binding_debug_path(binding)),
                            details: "unresolved symbolic expression reference".to_string(),
                        }
                        .into(),
                    );
                }
            }
        }
    }
}

fn binding_debug_path(binding: &DynamicBinding) -> String {
    binding.debug_name.clone().unwrap_or_else(|| match &binding.target {
        PropertyTarget::NodeProperty {
            node_id,
            property_path,
        } => format!("{node_id}.{}", property_path.0),
        PropertyTarget::VirtualProperty { id } => format!("virtual.{}", id.0),
    })
}

fn expression_references_contain_symbolic_paths(expression: &Expression) -> bool {
    expression.references.iter().any(|reference| {
        matches!(
            reference,
            crate::expr::ExpressionReference::SymbolicPath { .. }
        )
    })
}

fn dynamic_binding_cycle(
    bindings: &HashMap<PropertyTarget, DynamicBinding>,
) -> Option<Vec<PropertyTarget>> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum VisitState {
        Visiting,
        Done,
    }

    let mut state: HashMap<PropertyTarget, VisitState> = HashMap::new();
    let mut stack = Vec::new();

    fn dfs(
        node: &PropertyTarget,
        bindings: &HashMap<PropertyTarget, DynamicBinding>,
        state: &mut HashMap<PropertyTarget, VisitState>,
        stack: &mut Vec<PropertyTarget>,
    ) -> Option<Vec<PropertyTarget>> {
        if let Some(VisitState::Visiting) = state.get(node) {
            if let Some(start_index) = stack.iter().position(|entry| entry == node) {
                let mut cycle = stack[start_index..].to_vec();
                cycle.push(node.clone());
                return Some(cycle);
            }
            return Some(vec![node.clone()]);
        }
        if matches!(state.get(node), Some(VisitState::Done)) {
            return None;
        }

        state.insert(node.clone(), VisitState::Visiting);
        stack.push(node.clone());
        if let Some(binding) = bindings.get(node) {
            for dep in dynamic_binding_dependencies(binding) {
                if bindings.contains_key(&dep)
                    && let Some(cycle) = dfs(&dep, bindings, state, stack)
                {
                    return Some(cycle);
                }
            }
        }
        stack.pop();
        state.insert(node.clone(), VisitState::Done);
        None
    }

    for target in bindings.keys() {
        if let Some(cycle) = dfs(target, bindings, &mut state, &mut stack) {
            return Some(cycle);
        }
    }
    None
}

fn dynamic_binding_dependencies(binding: &DynamicBinding) -> Vec<PropertyTarget> {
    let mut dependencies = Vec::new();
    match &binding.source {
        DynamicBindingSource::Literal(_) => {}
        DynamicBindingSource::Expression(expression) => {
            collect_expression_target_dependencies(expression, &mut dependencies);
        }
        DynamicBindingSource::Animation(track) => {
            for key in &track.keys {
                if let DynamicKeyValue::Expression(expression) = &key.value {
                    collect_expression_target_dependencies(expression, &mut dependencies);
                }
            }
        }
    }
    dependencies
}

fn collect_expression_target_dependencies(expression: &Expression, out: &mut Vec<PropertyTarget>) {
    for reference in &expression.references {
        match reference {
            crate::expr::ExpressionReference::NodeProperty {
                node_id,
                property_path,
            } => out.push(PropertyTarget::NodeProperty {
                node_id: *node_id,
                property_path: property_path.clone(),
            }),
            crate::expr::ExpressionReference::VirtualProperty { id } => {
                out.push(PropertyTarget::VirtualProperty { id: *id })
            }
            crate::expr::ExpressionReference::SymbolicPath { .. } => {}
        }
    }
}
