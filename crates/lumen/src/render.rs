//! Frame render orchestration and per-frame render context state.

use std::{
    collections::HashSet,
    hash::{DefaultHasher, Hash, Hasher},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::{
    cache::{AssetCache, NodeOutputCache},
    capability::RuntimeCapabilityProfile,
    composition::Composition,
    error::{GraphValidationError, LumenError, PropertyError, RenderError},
    expr::{ExprNode, GlobalVar},
    graph::{Connection, InputPort, OutputPort},
    media::{MediaStore, VideoFrameResolver},
    node::{NodeId, NodeInputs, NodeKind, PortValue},
    raster::RasterFrame,
    surface_pool::SurfacePool,
};

#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed)
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

pub struct RenderContext {
    pub frame: u32,
    pub fps: f32,
    pub width: u32,
    pub height: u32,
    pub duration_frames: u32,
    pub surface_pool: Arc<SurfacePool>,
    pub asset_cache: Arc<RwLock<AssetCache>>,
    pub node_output_cache: NodeOutputCache,
    pub media_store: Arc<dyn MediaStore>,
    pub capability_profile: RuntimeCapabilityProfile,
    pub cancellation: CancellationToken,
    pub graph_revision: u64,
}

impl RenderContext {
    pub fn new(
        composition: &Composition,
        surface_pool: Arc<SurfacePool>,
        asset_cache: Arc<RwLock<AssetCache>>,
        media_store: Arc<dyn MediaStore>,
        capability_profile: RuntimeCapabilityProfile,
    ) -> Self {
        Self {
            frame: 0,
            fps: composition.timeline.fps,
            width: composition.render_settings.width,
            height: composition.render_settings.height,
            duration_frames: composition.timeline.duration_frames,
            surface_pool,
            asset_cache,
            node_output_cache: NodeOutputCache::new(),
            media_store,
            capability_profile,
            cancellation: CancellationToken::new(),
            graph_revision: compute_graph_revision(composition),
        }
    }

    fn resolution_key(&self) -> u32 {
        self.width.saturating_mul(self.height)
    }
}

impl Composition {
    pub fn render_frame(
        &self,
        frame: u32,
        ctx: &mut RenderContext,
    ) -> Result<RasterFrame, LumenError> {
        if frame >= self.timeline.duration_frames {
            return Err(RenderError::FrameOutOfRange {
                frame,
                duration_frames: self.timeline.duration_frames,
            }
            .into());
        }

        ctx.frame = frame;
        ctx.fps = self.timeline.fps;
        ctx.width = self.render_settings.width;
        ctx.height = self.render_settings.height;
        ctx.duration_frames = self.timeline.duration_frames;
        ctx.graph_revision = compute_graph_revision(self);
        ctx.node_output_cache.clear();

        if ctx.cancellation.is_cancelled() {
            return Err(RenderError::Cancelled { frame }.into());
        }

        let target = self.media_output_target()?;
        let final_output = self.evaluate_node_at_frame(target, frame, ctx)?;

        match final_output {
            PortValue::RasterFrame(frame_data) => Ok(frame_data),
            _ => Err(RenderError::InvalidMediaOutputType {
                frame,
                node_id: target,
            }
            .into()),
        }
    }

    fn media_output_target(&self) -> Result<NodeId, LumenError> {
        let media_output_nodes: Vec<NodeId> = self
            .graph
            .nodes
            .values()
            .filter(|node| matches!(node.kind, NodeKind::MediaOutput(_)))
            .map(|node| node.id)
            .collect();

        match media_output_nodes.as_slice() {
            [target] => Ok(*target),
            [] => Err(GraphValidationError::MissingMediaOutput.into()),
            outputs => Err(GraphValidationError::MultipleMediaOutputs {
                count: outputs.len(),
            }
            .into()),
        }
    }

    fn evaluate_node_at_frame(
        &self,
        node_id: NodeId,
        frame: u32,
        ctx: &mut RenderContext,
    ) -> Result<PortValue, LumenError> {
        if let Some(cached) = ctx
            .node_output_cache
            .get(node_id, frame, ctx.resolution_key(), ctx.graph_revision)
            .cloned()
        {
            return Ok(cached);
        }

        if ctx.cancellation.is_cancelled() {
            return Err(RenderError::Cancelled { frame }.into());
        }

        let node = self
            .graph
            .nodes
            .get(&node_id)
            .ok_or(GraphValidationError::InvalidEvaluationTarget { node_id })?;
        let mut resolved_kind = node.kind.clone();
        self.apply_animated_properties(node_id, frame, &mut resolved_kind, ctx)?;

        if let Some(short_circuit) = self.try_short_circuit(node_id, frame, &resolved_kind, ctx)? {
            ctx.node_output_cache.insert(
                node_id,
                frame,
                ctx.resolution_key(),
                ctx.graph_revision,
                short_circuit.clone(),
            );
            return Ok(short_circuit);
        }

        let mut inputs = NodeInputs::new();
        match &resolved_kind {
            NodeKind::Switch(switch_node) => {
                let selected = switch_node
                    .map
                    .iter()
                    .find_map(|(index, range)| range.contains(&frame).then_some(*index));
                if let Some(index) = selected {
                    let input_name = format!("input_{index}");
                    if let Some(connection) = self.find_input_connection(node_id, &input_name) {
                        let output =
                            self.evaluate_node_at_frame(connection.from_node, frame, ctx)?;
                        inputs.insert(input_name, output);
                    }
                }
            }
            _ => {
                for input_def in resolved_kind.input_port_defs() {
                    let upstream = self.find_input_connection(node_id, input_def.name);
                    if let Some(connection) = upstream {
                        let output =
                            self.evaluate_node_at_frame(connection.from_node, frame, ctx)?;
                        inputs.insert(input_def.name, output);
                    } else if !input_def.optional {
                        return Err(GraphValidationError::MissingRequiredInput {
                            node_id,
                            node_kind: resolved_kind.kind_name(),
                            port: input_def.name.to_string(),
                        }
                        .into());
                    }
                }
            }
        }

        let previous_frame = ctx.frame;
        ctx.frame = frame;
        let output =
            resolved_kind
                .evaluate(&inputs, ctx)
                .map_err(|err| RenderError::NodeEvaluation {
                    frame,
                    node_id,
                    node_kind: resolved_kind.kind_name(),
                    details: err.to_string(),
                });
        ctx.frame = previous_frame;
        let output = output?;

        ctx.node_output_cache.insert(
            node_id,
            frame,
            ctx.resolution_key(),
            ctx.graph_revision,
            output.clone(),
        );
        Ok(output)
    }

    fn try_short_circuit(
        &self,
        node_id: NodeId,
        frame: u32,
        node_kind: &NodeKind,
        ctx: &mut RenderContext,
    ) -> Result<Option<PortValue>, LumenError> {
        match node_kind {
            NodeKind::FrameHold(frame_hold) => {
                let held_frame = frame_hold
                    .hold_frame
                    .min(self.timeline.duration_frames.saturating_sub(1));
                self.resolve_required_input(
                    node_id,
                    node_kind.kind_name(),
                    "source",
                    held_frame,
                    ctx,
                )
                .map(Some)
            }
            NodeKind::Transform(transform) if transform.is_identity() => self
                .resolve_required_input(node_id, node_kind.kind_name(), "source", frame, ctx)
                .map(Some),
            NodeKind::Blur(blur) if blur.is_noop() => self
                .resolve_required_input(node_id, node_kind.kind_name(), "source", frame, ctx)
                .map(Some),
            NodeKind::Memo(memo) => self.resolve_memo_node(node_id, memo, frame, ctx).map(Some),
            _ => Ok(None),
        }
    }

    fn resolve_memo_node(
        &self,
        node_id: NodeId,
        memo: &crate::node::memo::Memo,
        frame: u32,
        ctx: &mut RenderContext,
    ) -> Result<PortValue, LumenError> {
        if memo.cache_id.trim().is_empty() {
            return Err(PropertyError::MissingProperty {
                node_id,
                property_path: "cache_id".to_string(),
            }
            .into());
        }

        if !self.memo_is_eligible(node_id, memo.allow_expressions) {
            return self.resolve_required_input(node_id, "Memo", "source", frame, ctx);
        }

        let signature = self.memo_signature(node_id, memo.allow_expressions, frame);
        if let Ok(cache) = ctx.asset_cache.read()
            && let Some(cached) = cache.memo_get(&memo.cache_id, ctx.width, ctx.height, signature)
        {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                cached, ctx.width, ctx.height,
            )));
        }

        let source = self.resolve_required_input(node_id, "Memo", "source", frame, ctx)?;
        let PortValue::RasterFrame(source) = source else {
            return Err(PropertyError::InvalidType {
                node_id,
                property_path: "source".to_string(),
                expected: "RasterFrame",
                actual: "non-raster",
            }
            .into());
        };
        let raster = source.to_bitmap()?;
        let RasterFrame::Bitmap(pixels, width, height) = raster else {
            return Err(RenderError::InvalidMediaOutputType { frame, node_id }.into());
        };

        if let Ok(mut cache) = ctx.asset_cache.write() {
            cache.memo_insert(
                memo.cache_id.clone(),
                width,
                height,
                signature,
                Arc::clone(&pixels),
            );
        }

        Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
            pixels, width, height,
        )))
    }

    fn memo_is_eligible(&self, node_id: NodeId, allow_expressions: bool) -> bool {
        let upstream = self.upstream_nodes(node_id);
        if self
            .tracks
            .iter()
            .any(|track| upstream.contains(&track.node_id))
        {
            return false;
        }

        if !allow_expressions
            && self
                .expressions
                .keys()
                .any(|expression_node| upstream.contains(expression_node))
        {
            return false;
        }

        for upstream_node_id in &upstream {
            if let Some(expressions) = self.expressions.get(upstream_node_id)
                && expressions
                    .values()
                    .any(|expression| expression_depends_on_frame(&expression.ast))
            {
                return false;
            }
        }

        for upstream_node_id in &upstream {
            let Some(node) = self.graph.nodes.get(upstream_node_id) else {
                continue;
            };
            match &node.kind {
                NodeKind::MediaIn(crate::node::media_in::MediaIn {
                    kind: crate::node::media_in::MediaInKind::Video { .. },
                })
                | NodeKind::Switch(_)
                | NodeKind::FrameHold(_) => return false,
                _ => {}
            }
        }

        true
    }

    fn memo_signature(&self, node_id: NodeId, allow_expressions: bool, frame: u32) -> u64 {
        let upstream = self.upstream_nodes(node_id);
        let mut ordered_nodes: Vec<NodeId> = upstream.iter().copied().collect();
        ordered_nodes.sort_unstable();
        let mut hasher = DefaultHasher::new();
        node_id.hash(&mut hasher);
        allow_expressions.hash(&mut hasher);
        for upstream_node_id in &ordered_nodes {
            upstream_node_id.hash(&mut hasher);
            if let Some(node) = self.graph.nodes.get(upstream_node_id) {
                node.kind.kind_name().hash(&mut hasher);
                format!("{:?}", node.kind).hash(&mut hasher);
            }
            if let Some(expressions) = self.expressions.get(upstream_node_id) {
                let mut expression_entries: Vec<_> = expressions.iter().collect();
                expression_entries
                    .sort_by(|(left_path, _), (right_path, _)| left_path.cmp(right_path));
                for (path, expression) in expression_entries {
                    path.hash(&mut hasher);
                    expression.source.hash(&mut hasher);
                    if expression_depends_on_frame(&expression.ast) {
                        frame.hash(&mut hasher);
                    }
                }
            }
        }
        let mut upstream_connections: Vec<_> = self
            .graph
            .connections
            .iter()
            .filter(|connection| {
                upstream.contains(&connection.from_node) || upstream.contains(&connection.to_node)
            })
            .collect();
        upstream_connections.sort_by(|left, right| {
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
        for connection in upstream_connections {
            connection.from_node.hash(&mut hasher);
            connection.to_node.hash(&mut hasher);
            hash_output_port(&connection.from_port, &mut hasher);
            hash_input_port(&connection.to_port, &mut hasher);
        }
        hasher.finish()
    }

    fn upstream_nodes(&self, node_id: NodeId) -> HashSet<NodeId> {
        let mut visited = HashSet::new();
        let mut stack = vec![node_id];
        while let Some(current) = stack.pop() {
            for edge in self
                .graph
                .connections
                .iter()
                .filter(|edge| edge.to_node == current)
            {
                if visited.insert(edge.from_node) {
                    stack.push(edge.from_node);
                }
            }
        }
        visited
    }

    fn resolve_required_input(
        &self,
        node_id: NodeId,
        node_kind: &'static str,
        input_name: &str,
        frame: u32,
        ctx: &mut RenderContext,
    ) -> Result<PortValue, LumenError> {
        let connection = self.find_input_connection(node_id, input_name).ok_or(
            GraphValidationError::MissingRequiredInput {
                node_id,
                node_kind,
                port: input_name.to_string(),
            },
        )?;
        self.evaluate_node_at_frame(connection.from_node, frame, ctx)
    }

    fn find_input_connection(&self, node_id: NodeId, input_name: &str) -> Option<&Connection> {
        self.graph
            .connections
            .iter()
            .find(|edge| edge.to_node == node_id && input_port_matches(&edge.to_port, input_name))
    }
}

fn input_port_matches(port: &InputPort, expected_name: &str) -> bool {
    match port {
        InputPort::Named(name) => name == expected_name,
        InputPort::Indexed(index) => expected_name == format!("input_{index}"),
    }
}

fn expression_depends_on_frame(ast: &ExprNode) -> bool {
    match ast {
        ExprNode::Global(GlobalVar::Frame | GlobalVar::Time) => true,
        ExprNode::Literal(_) | ExprNode::Global(_) | ExprNode::NodeProperty(_, _) => false,
        ExprNode::Unary(_, inner) => expression_depends_on_frame(inner),
        ExprNode::Binary(left, _, right) => {
            expression_depends_on_frame(left) || expression_depends_on_frame(right)
        }
        ExprNode::Builtin(_, args) => args.iter().any(expression_depends_on_frame),
        ExprNode::Conditional(condition, when_true, when_false) => {
            expression_depends_on_frame(condition)
                || expression_depends_on_frame(when_true)
                || expression_depends_on_frame(when_false)
        }
    }
}

fn compute_graph_revision(composition: &Composition) -> u64 {
    let mut hasher = DefaultHasher::new();

    let mut node_ids: Vec<NodeId> = composition.graph.nodes.keys().copied().collect();
    node_ids.sort_unstable();
    for node_id in node_ids {
        node_id.hash(&mut hasher);
        if let Some(node) = composition.graph.nodes.get(&node_id) {
            node.kind.kind_name().hash(&mut hasher);
            format!("{:?}", node.kind).hash(&mut hasher);
        }
    }

    let mut connections: Vec<_> = composition.graph.connections.iter().collect();
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

    let mut tracks: Vec<_> = composition.tracks.iter().collect();
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

    let mut expression_node_ids: Vec<_> = composition.expressions.keys().copied().collect();
    expression_node_ids.sort_unstable();
    for node_id in expression_node_ids {
        node_id.hash(&mut hasher);
        if let Some(expressions) = composition.expressions.get(&node_id) {
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

fn hash_input_port(port: &InputPort, hasher: &mut DefaultHasher) {
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

fn hash_output_port(port: &OutputPort, hasher: &mut DefaultHasher) {
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

fn sortable_input_port_key(port: &InputPort) -> (u8, String) {
    match port {
        InputPort::Named(name) => (0, name.clone()),
        InputPort::Indexed(index) => (1, index.to_string()),
    }
}

fn sortable_output_port_key(port: &OutputPort) -> (u8, String) {
    match port {
        OutputPort::Named(name) => (0, name.clone()),
        OutputPort::Indexed(index) => (1, index.to_string()),
    }
}

pub struct NullMediaStore;

impl MediaStore for NullMediaStore {
    fn get_image_resolver(&self, _source: &str) -> Option<Box<dyn crate::media::ImageResolver>> {
        None
    }

    fn get_video_resolver(&self, _source: &str) -> Option<Box<dyn VideoFrameResolver>> {
        None
    }
}
