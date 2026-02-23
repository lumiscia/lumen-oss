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
    graph::InputPort,
    media::{MediaStore, VideoFrameResolver},
    node::{NodeId, NodeInputs, PortValue},
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
    pub node_output_cache: HashMap<NodeId, PortValue>,
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

        let media_output_nodes: Vec<NodeId> = self
            .graph
            .nodes
            .values()
            .filter(|node| matches!(node.kind, crate::node::NodeKind::MediaOutput(_)))
            .map(|node| node.id)
            .collect();

        let target = match media_output_nodes.as_slice() {
            [target] => *target,
            [] => return Err(GraphValidationError::MissingMediaOutput.into()),
            outputs => {
                return Err(GraphValidationError::MultipleMediaOutputs {
                    count: outputs.len(),
                }
                .into());
            }
        };

        let order = self.graph.evaluation_order(target)?;
        for node_id in order {
            if ctx.cancellation.is_cancelled() {
                return Err(RenderError::Cancelled { frame }.into());
            }

            let node = self
                .graph
                .nodes
                .get(&node_id)
                .ok_or(GraphValidationError::InvalidEvaluationTarget { node_id })?;
            let mut inputs = NodeInputs::new();

            let mut resolved_kind = node.kind.clone();
            self.apply_animated_properties(node_id, frame, &mut resolved_kind)?;

            for input_def in resolved_kind.input_port_defs() {
                let upstream_edge = self.graph.connections.iter().find(|edge| {
                    edge.to_node == node_id && input_port_matches(&edge.to_port, input_def.name)
                });

                if let Some(connection) = upstream_edge {
                    let output = ctx
                        .node_output_cache
                        .get(&connection.from_node)
                        .ok_or(RenderError::MissingNodeOutput {
                            frame,
                            node_id: connection.from_node,
                        })?
                        .clone();
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

            let output = resolved_kind
                .evaluate(&inputs, ctx)
                .map_err(|err| RenderError::NodeEvaluation {
                    frame,
                    node_id,
                    node_kind: resolved_kind.kind_name(),
                    details: err.to_string(),
                })?;

            ctx.node_output_cache.insert(node_id, output);
        }

        let final_output =
            ctx.node_output_cache
                .get(&target)
                .ok_or(RenderError::MissingNodeOutput {
                    frame,
                    node_id: target,
                })?;

        match final_output {
            PortValue::RasterFrame(frame_data) => Ok(frame_data.clone()),
            _ => Err(RenderError::InvalidMediaOutputType {
                frame,
                node_id: target,
            }
            .into()),
        }
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
