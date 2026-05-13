use std::collections::{BTreeMap, BTreeSet};

use crate::{
    composition::Composition,
    error::{GraphValidationError, LumenError, MediaError},
    expr::ExpressionContext,
    node::{
        NodeId, NodeKind, PortRef,
        processing::time_remap::{TimeRemapSettings, remap_frame},
        source::media_in,
    },
};

use super::MediaStore;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FrameRequirements {
    pub images: Vec<String>,
    pub videos: Vec<VideoFrameRequirement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFrameRequirement {
    pub stream_id: String,
    pub frames: Vec<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct RenderRequirements {
    images: BTreeSet<String>,
    videos: BTreeMap<String, BTreeSet<u32>>,
}

impl RenderRequirements {
    pub fn add_image(&mut self, image_id: impl Into<String>) {
        self.images.insert(image_id.into());
    }

    pub fn add_video_frame(&mut self, stream_id: impl Into<String>, frame: u32) {
        self.videos
            .entry(stream_id.into())
            .or_default()
            .insert(frame);
    }

    pub fn merge(&mut self, other: FrameRequirements) {
        self.images.extend(other.images);
        for video in other.videos {
            self.videos
                .entry(video.stream_id)
                .or_default()
                .extend(video.frames);
        }
    }
}

impl From<RenderRequirements> for FrameRequirements {
    fn from(value: RenderRequirements) -> Self {
        Self {
            images: value.images.into_iter().collect(),
            videos: value
                .videos
                .into_iter()
                .map(|(stream_id, frames)| VideoFrameRequirement {
                    stream_id,
                    frames: frames.into_iter().collect(),
                })
                .collect(),
        }
    }
}

pub fn collect_frame_requirements<M: MediaStore>(
    composition: &Composition,
    media_store: &M,
    frame: u32,
) -> Result<FrameRequirements, LumenError> {
    tracing::trace!(target: "lumen_media", frame, "collect frame requirements");
    let output_port = media_output_port(composition)?;
    let mut collector = RenderRequirements::default();
    let mut context = RequirementContext {
        composition,
        media_store,
        frame,
    };
    context.collect_port(&output_port, &mut collector)?;
    let requirements = FrameRequirements::from(collector);
    tracing::trace!(
        target: "lumen_media",
        frame,
        images = requirements.images.len(),
        videos = requirements.videos.len(),
        "collected frame requirements"
    );
    Ok(requirements)
}

struct RequirementContext<'a, M: MediaStore> {
    composition: &'a Composition,
    media_store: &'a M,
    frame: u32,
}

impl<'a, M: MediaStore> RequirementContext<'a, M> {
    fn collect_port(
        &mut self,
        port: &PortRef,
        collector: &mut RenderRequirements,
    ) -> Result<(), LumenError> {
        if port.is_empty() {
            return Ok(());
        }

        let Some(node) = self.composition.graph.nodes.get(&port.id) else {
            return Ok(());
        };
        self.collect_node(port.id, node, collector)
    }

    fn collect_node(
        &mut self,
        node_id: NodeId,
        node: &NodeKind,
        collector: &mut RenderRequirements,
    ) -> Result<(), LumenError> {
        match node {
            NodeKind::MediaOutput(media_output) => {
                self.collect_port(&media_output.source, collector)?;
            }
            NodeKind::MediaIn(media_in_node) => {
                self.collect_media_in(media_in_node, collector)?;
            }
            NodeKind::TimeRemap(time_remap) => {
                let target_frame = self.remapped_frame(time_remap)?;
                self.with_frame(target_frame, |context| {
                    context.collect_port(&time_remap.source, collector)
                })?;
            }
            NodeKind::Switch(switch) => {
                if let Some(layer) = crate::node::compositing::switch::selected_layer_for_frame(
                    switch,
                    &self.expr_context("switch_requirements"),
                )?
                .and_then(|index| switch.layers.get(index))
                {
                    self.collect_port(layer, collector)?;
                }
            }
            _ => self.collect_default_inputs(node_id, collector)?,
        }

        Ok(())
    }

    fn collect_default_inputs(
        &mut self,
        node_id: NodeId,
        collector: &mut RenderRequirements,
    ) -> Result<(), LumenError> {
        let inputs: Vec<_> = self
            .composition
            .graph
            .connections
            .iter()
            .filter(|connection| connection.to_node == node_id)
            .map(|connection| PortRef::new(connection.from_node, connection.from_port.clone()))
            .collect();

        for input in inputs {
            self.collect_port(&input, collector)?;
        }

        Ok(())
    }

    fn collect_media_in(
        &self,
        media_in_node: &media_in::MediaIn,
        collector: &mut RenderRequirements,
    ) -> Result<(), LumenError> {
        match media_in::resolve_for_context(
            media_in_node,
            &self.expr_context("media_requirements"),
        )? {
            media_in::MediaInKind::Image { image_id } => {
                tracing::trace!(
                    target: "lumen_media",
                    frame = self.frame,
                    image_id = %image_id,
                    "require image"
                );
                collector.add_image(image_id);
            }
            media_in::MediaInKind::Video {
                stream_id,
                range,
                speed,
                loop_mode,
            } => {
                let resolver =
                    self.media_store
                        .get_video_resolver(&stream_id)
                        .ok_or_else(|| MediaError::SourceNotFound {
                            media_source: stream_id.clone(),
                        })?;
                let metadata = resolver.metadata();
                let source_frame = media_in::map_to_source_frame(
                    self.frame,
                    self.composition.timeline.fps,
                    metadata.fps,
                    metadata.frame_count,
                    range.as_ref(),
                    speed,
                    loop_mode,
                )
                .ok_or_else(|| MediaError::FrameOutOfRange {
                    media_source: stream_id.clone(),
                    frame: self.frame,
                    frame_count: metadata.frame_count,
                })?;
                tracing::trace!(
                    target: "lumen_media",
                    frame = self.frame,
                    stream_id = %stream_id,
                    source_frame,
                    "require video frame"
                );
                collector.add_video_frame(stream_id, source_frame);
            }
        }

        Ok(())
    }

    fn remapped_frame(
        &self,
        time_remap: &crate::node::processing::time_remap::TimeRemap,
    ) -> Result<u32, LumenError> {
        let expr_context = self.expr_context("time_remap_requirements");
        Ok(remap_frame(TimeRemapSettings {
            frame: time_remap
                .frame
                .resolve_float(time_remap.id, "frame", &expr_context)?,
            loop_enabled: time_remap.loop_enabled.resolve_bool(
                time_remap.id,
                "loop_enabled",
                &expr_context,
            )?,
            loop_start: time_remap.loop_start.resolve_int(
                time_remap.id,
                "loop_start",
                &expr_context,
            )?,
            loop_end: time_remap
                .loop_end
                .resolve_int(time_remap.id, "loop_end", &expr_context)?,
        }))
    }

    fn with_frame<T>(
        &mut self,
        frame: u32,
        f: impl FnOnce(&mut Self) -> Result<T, LumenError>,
    ) -> Result<T, LumenError> {
        let original_frame = self.frame;
        self.frame = frame;
        let result = f(self);
        self.frame = original_frame;
        result
    }

    fn expr_context(&self, path: &str) -> ExpressionContext<'_> {
        ExpressionContext {
            frame: self.frame,
            fps: self.composition.timeline.fps,
            width: self.composition.render_settings.width,
            height: self.composition.render_settings.height,
            duration_frames: self.composition.timeline.duration_frames,
            path: Some(path.to_string()),
            graph: Some(&self.composition.graph),
        }
    }
}

fn media_output_port(composition: &Composition) -> Result<PortRef, LumenError> {
    let mut media_outputs = composition
        .graph
        .nodes
        .iter()
        .filter_map(|(node_id, node)| matches!(node, NodeKind::MediaOutput(_)).then_some(*node_id));
    let Some(output_node_id) = media_outputs.next() else {
        return Err(GraphValidationError::MissingMediaOutput.into());
    };
    if media_outputs.next().is_some() {
        return Err(GraphValidationError::MultipleMediaOutputs { count: 2 }.into());
    }

    Ok(PortRef::new(output_node_id, "output".to_string()))
}
