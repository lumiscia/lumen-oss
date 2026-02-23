//! Frame render orchestration and per-frame render context state.

use std::{
    collections::HashMap,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::{
    cache::AssetCache,
    capability::RuntimeCapabilityProfile,
    composition::Composition,
    error::{GraphValidationError, LumenError, RenderError},
    graph::{Connection, InputPort},
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
        self.cancelled.store(true, Ordering::Relaxed);
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
    pub node_output_cache: HashMap<(NodeId, u32), PortValue>,
    pub media_store: Arc<dyn MediaStore>,
    pub capability_profile: RuntimeCapabilityProfile,
    pub cancellation: CancellationToken,
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
            node_output_cache: HashMap::new(),
            media_store,
            capability_profile,
            cancellation: CancellationToken::new(),
        }
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
        if let Some(cached) = ctx.node_output_cache.get(&(node_id, frame)).cloned() {
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
            ctx.node_output_cache
                .insert((node_id, frame), short_circuit.clone());
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

        ctx.node_output_cache
            .insert((node_id, frame), output.clone());
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
            _ => Ok(None),
        }
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

pub struct NullMediaStore;

impl MediaStore for NullMediaStore {
    fn get_image_resolver(&self, _source: &str) -> Option<Box<dyn crate::media::ImageResolver>> {
        None
    }

    fn get_video_resolver(&self, _source: &str) -> Option<Box<dyn VideoFrameResolver>> {
        None
    }
}
