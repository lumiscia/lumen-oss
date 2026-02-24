//! Frame render orchestration and per-frame render context state.

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    hash::{DefaultHasher, Hash, Hasher},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::{
    cache::{AssetCache, CachedBitmap, NodeOutputCache},
    capability::RuntimeCapabilityProfile,
    composition::{
        Composition, hash_input_port, hash_output_port, sortable_input_port_key,
        sortable_output_port_key,
    },
    error::{GraphValidationError, LumenError, PropertyError, RenderError},
    expr::{ExprNode, GlobalVar},
    graph::{Connection, InputPort},
    media::{MediaStore, VideoFrameResolver},
    node::{NodeId, NodeInputs, NodeKind, PortValue},
    raster::{RasterFrame, RectI},
    surface_pool::{SurfacePool, SurfacePoolStats},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum RenderQuality {
    Draft,
    #[default]
    Final,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderRequest {
    pub frame: u32,
    pub output_rect: RectI,
    pub roi: Option<RectI>,
    pub proxy_scale: u32,
    pub quality: RenderQuality,
}

impl RenderRequest {
    pub fn full_frame(frame: u32, width: u32, height: u32) -> Self {
        Self {
            frame,
            output_rect: RectI::from_size(width, height),
            roi: None,
            proxy_scale: 1,
            quality: RenderQuality::Final,
        }
    }

    pub const fn width(&self) -> u32 {
        self.output_rect.width
    }

    pub const fn height(&self) -> u32 {
        self.output_rect.height
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenderInstrumentation {
    pub node_evaluations: u64,
    pub node_output_cache_hits: u64,
    pub node_output_cache_misses: u64,
    pub memo_cache_hits: u64,
    pub memo_cache_misses: u64,
    pub pixel_allocation_bytes: u64,
    pub surface_acquires: u64,
    pub surface_reuses: u64,
    pub surface_fresh_allocations: u64,
    pub surface_fresh_allocation_bytes: u64,
    pub surface_acquires_by_size: HashMap<(u32, u32), u64>,
}

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
    pub request: RenderRequest,
    request_cache_key: u64,
    pub fps: f32,
    pub duration_frames: u32,
    pub surface_pool: Arc<SurfacePool>,
    pub asset_cache: Arc<RwLock<AssetCache>>,
    pub node_output_cache: NodeOutputCache,
    pub media_store: Arc<dyn MediaStore>,
    pub capability_profile: RuntimeCapabilityProfile,
    pub cancellation: CancellationToken,
    pub graph_revision: u64,
    pub instrumentation: RenderInstrumentation,
    animated_nodes: HashSet<NodeId>,
    animated_nodes_revision: u64,
    surface_pool_baseline: SurfacePoolStats,
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
            request: RenderRequest::full_frame(
                0,
                composition.render_settings.width,
                composition.render_settings.height,
            ),
            request_cache_key: 0,
            fps: composition.timeline.fps,
            duration_frames: composition.timeline.duration_frames,
            surface_pool: Arc::clone(&surface_pool),
            asset_cache,
            node_output_cache: NodeOutputCache::new(),
            media_store,
            capability_profile,
            cancellation: CancellationToken::new(),
            graph_revision: composition.graph_revision(),
            instrumentation: RenderInstrumentation::default(),
            animated_nodes: HashSet::new(),
            animated_nodes_revision: 0,
            surface_pool_baseline: surface_pool.stats(),
        }
    }

    pub fn reset_instrumentation(&mut self) {
        self.instrumentation = RenderInstrumentation::default();
        self.surface_pool_baseline = self.surface_pool.stats();
    }

    pub fn instrumentation_snapshot(&self) -> RenderInstrumentation {
        let mut snapshot = self.instrumentation.clone();
        let surface_delta = self
            .surface_pool
            .stats()
            .delta_from(&self.surface_pool_baseline);
        snapshot.surface_acquires = surface_delta.total_acquires;
        snapshot.surface_reuses = surface_delta.reused_acquires;
        snapshot.surface_fresh_allocations = surface_delta.fresh_allocations;
        snapshot.surface_fresh_allocation_bytes = surface_delta.fresh_allocation_bytes;
        snapshot.surface_acquires_by_size = surface_delta.acquires_by_size;
        snapshot
    }

    pub fn record_pixel_allocation_bytes(&mut self, bytes: usize) {
        self.instrumentation.pixel_allocation_bytes = self
            .instrumentation
            .pixel_allocation_bytes
            .saturating_add(bytes as u64);
    }

    fn request_cache_key(&self) -> u64 {
        self.request_cache_key
    }

    fn refresh_request_cache_key(&mut self) {
        let mut hasher = DefaultHasher::new();
        self.request.output_rect.hash(&mut hasher);
        self.request.roi.hash(&mut hasher);
        self.request.proxy_scale.hash(&mut hasher);
        self.request.quality.hash(&mut hasher);
        self.request_cache_key = hasher.finish()
    }

    fn refresh_animated_nodes(&mut self, composition: &Composition) {
        if self.animated_nodes_revision == self.graph_revision {
            return;
        }
        self.animated_nodes.clear();
        self.animated_nodes
            .extend(composition.tracks.iter().map(|track| track.node_id));
        self.animated_nodes
            .extend(composition.expressions.keys().copied());
        self.animated_nodes_revision = self.graph_revision;
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

        ctx.reset_instrumentation();
        ctx.request = RenderRequest::full_frame(
            frame,
            self.render_settings.width,
            self.render_settings.height,
        );
        ctx.refresh_request_cache_key();
        ctx.fps = self.timeline.fps;
        ctx.duration_frames = self.timeline.duration_frames;
        ctx.graph_revision = self.graph_revision();
        ctx.refresh_animated_nodes(self);
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
        let cached = self
            .cached_media_output
            .load(std::sync::atomic::Ordering::Relaxed);
        if cached != 0 {
            return Ok(NodeId(cached));
        }

        let media_output_nodes: Vec<NodeId> = self
            .graph
            .nodes
            .values()
            .filter(|node| matches!(node.kind, NodeKind::MediaOutput(_)))
            .map(|node| node.id)
            .collect();

        match media_output_nodes.as_slice() {
            [target] => {
                self.cached_media_output
                    .store(target.0, std::sync::atomic::Ordering::Relaxed);
                Ok(*target)
            }
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
            .get(node_id, frame, ctx.request_cache_key(), ctx.graph_revision)
            .cloned()
        {
            ctx.instrumentation.node_output_cache_hits =
                ctx.instrumentation.node_output_cache_hits.saturating_add(1);
            return Ok(cached);
        }
        ctx.instrumentation.node_output_cache_misses = ctx
            .instrumentation
            .node_output_cache_misses
            .saturating_add(1);

        if ctx.cancellation.is_cancelled() {
            return Err(RenderError::Cancelled { frame }.into());
        }

        let node = self
            .graph
            .nodes
            .get(&node_id)
            .ok_or(GraphValidationError::InvalidEvaluationTarget { node_id })?;

        let has_animations = ctx.animated_nodes.contains(&node_id);

        let resolved_kind: Cow<'_, NodeKind> = if has_animations {
            let mut cloned = node.kind.clone();
            self.apply_animated_properties(node_id, frame, &mut cloned, ctx)?;
            Cow::Owned(cloned)
        } else {
            Cow::Borrowed(&node.kind)
        };

        if let Some(short_circuit) = self.try_short_circuit(node_id, frame, &resolved_kind, ctx)? {
            ctx.node_output_cache.insert(
                node_id,
                frame,
                ctx.request_cache_key(),
                ctx.graph_revision,
                short_circuit.clone(),
            );
            return Ok(short_circuit);
        }

        let mut inputs = NodeInputs::new();
        match resolved_kind.as_ref() {
            NodeKind::Switch(switch_node) => {
                let selected = switch_node
                    .map
                    .iter()
                    .find_map(|(index, range)| range.contains(&frame).then_some(*index));
                if let Some(index) = selected {
                    if let Some(connection) = self.find_indexed_input_connection(node_id, index) {
                        let output =
                            self.evaluate_node_at_frame(connection.from_node, frame, ctx)?;
                        inputs.insert(format!("input_{index}"), output);
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

        let previous_frame = ctx.request.frame;
        ctx.request.frame = frame;
        ctx.instrumentation.node_evaluations =
            ctx.instrumentation.node_evaluations.saturating_add(1);
        let output =
            resolved_kind
                .evaluate(&inputs, ctx)
                .map_err(|err| RenderError::NodeEvaluation {
                    frame,
                    node_id,
                    node_kind: resolved_kind.kind_name(),
                    details: err.to_string(),
                });
        ctx.request.frame = previous_frame;
        let output = output?;

        ctx.node_output_cache.insert(
            node_id,
            frame,
            ctx.request_cache_key(),
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
            NodeKind::Merge(merge) if merge.opacity <= 0.0 => self
                .resolve_required_input(node_id, node_kind.kind_name(), "base", frame, ctx)
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
        let request_hash = ctx.request_cache_key();
        if let Ok(cache) = ctx.asset_cache.read()
            && let Some(cached) = cache.memo_get(
                &memo.cache_id,
                ctx.request.width(),
                ctx.request.height(),
                request_hash,
                signature,
            )
        {
            ctx.instrumentation.memo_cache_hits =
                ctx.instrumentation.memo_cache_hits.saturating_add(1);
            return Ok(PortValue::RasterFrame(RasterFrame::bitmap(
                cached.pixels,
                cached.width,
                cached.height,
            )));
        }
        ctx.instrumentation.memo_cache_misses =
            ctx.instrumentation.memo_cache_misses.saturating_add(1);

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
        let bitmap = source.into_bitmap_frame()?;
        let pixels = bitmap.pixels;
        let width = bitmap.storage_width;
        let height = bitmap.storage_height;

        if let Ok(mut cache) = ctx.asset_cache.write() {
            cache.memo_insert(
                memo.cache_id.clone(),
                width,
                height,
                request_hash,
                signature,
                CachedBitmap {
                    pixels: Arc::clone(&pixels),
                    width,
                    height,
                },
            );
        }

        Ok(PortValue::RasterFrame(RasterFrame::bitmap(
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

        if !allow_expressions {
            for upstream_node_id in &upstream {
                if let Some(expressions) = self.expressions.get(upstream_node_id)
                    && expressions
                        .values()
                        .any(|expression| expression_depends_on_frame(&expression.ast))
                {
                    return false;
                }
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
                node.kind.hash_content(&mut hasher);
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
            for edge in self.graph.connections_to(current) {
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

    fn find_indexed_input_connection(&self, node_id: NodeId, index: u16) -> Option<&Connection> {
        self.graph
            .connections_to(node_id)
            .find(|edge| match &edge.to_port {
                InputPort::Indexed(port_index) => *port_index == index,
                InputPort::Named(name) => name
                    .strip_prefix("input_")
                    .and_then(|suffix| suffix.parse::<u16>().ok())
                    .is_some_and(|parsed| parsed == index),
            })
    }

    fn find_input_connection(&self, node_id: NodeId, input_name: &str) -> Option<&Connection> {
        self.graph
            .connections_to(node_id)
            .find(|edge| input_port_matches(&edge.to_port, input_name))
    }
}

fn input_port_matches(port: &InputPort, expected_name: &str) -> bool {
    match port {
        InputPort::Named(name) => name == expected_name,
        InputPort::Indexed(index) => {
            // Parse "input_N" without allocating
            expected_name
                .strip_prefix("input_")
                .and_then(|suffix| suffix.parse::<u16>().ok())
                .is_some_and(|parsed| parsed == *index)
        }
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

pub struct NullMediaStore;

impl MediaStore for NullMediaStore {
    fn get_image_resolver(&self, _source: &str) -> Option<Box<dyn crate::media::ImageResolver>> {
        None
    }

    fn get_video_resolver(&self, _source: &str) -> Option<Box<dyn VideoFrameResolver>> {
        None
    }
}
