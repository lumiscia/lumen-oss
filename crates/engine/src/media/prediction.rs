use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::{
    composition::Composition,
    error::{LumenError, MediaError},
    expr::ExpressionContext,
    node::{NodeId, NodeKind, source::media_in},
    scope::{FrameTime, Routed, Scope},
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
struct RenderRequirements {
    images: BTreeSet<String>,
    videos: BTreeMap<String, BTreeSet<u32>>,
}

impl RenderRequirements {
    fn add_image(&mut self, image_id: impl Into<String>) {
        self.images.insert(image_id.into());
    }

    fn add_video_frame(&mut self, stream_id: impl Into<String>, frame: u32) {
        self.videos
            .entry(stream_id.into())
            .or_default()
            .insert(frame);
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
    let scope = Scope::root(composition);
    let output = scope.output;
    let mut walk = Walk::default();
    let context = RequirementContext {
        scope,
        media_store,
        frame: f64::from(frame).into(),
    };
    context.collect_node(output, &mut walk)?;
    let requirements = FrameRequirements::from(walk.requirements);
    tracing::trace!(
        target: "lumen_media",
        frame,
        images = requirements.images.len(),
        videos = requirements.videos.len(),
        "collected frame requirements"
    );
    Ok(requirements)
}

/// What a traversal has gathered and visited. The time remaps and clips above a node determine
/// its scope and frame, so reaching a node again by the same path requires nothing new.
#[derive(Default)]
struct Walk {
    requirements: RenderRequirements,
    visited: HashSet<(NodeId, Vec<NodeId>)>,
    instances: Vec<NodeId>,
}

struct RequirementContext<'a, M: MediaStore> {
    scope: Scope<'a>,
    media_store: &'a M,
    frame: FrameTime,
}

impl<'a, M: MediaStore> RequirementContext<'a, M> {
    fn collect_node(&self, node_id: NodeId, walk: &mut Walk) -> Result<(), LumenError> {
        if !walk.visited.insert((node_id, walk.instances.clone())) {
            return Ok(());
        }

        let Some(node) = self.scope.node(node_id) else {
            return Ok(());
        };
        if let NodeKind::MediaIn(media_in_node) = node {
            return self.collect_media_in(media_in_node, &mut walk.requirements);
        }
        for Routed {
            node,
            frame,
            scope,
            instance,
        } in crate::scope::route(&self.scope, node, self.frame, Some(self.media_store))?
        {
            let context = RequirementContext {
                scope,
                media_store: self.media_store,
                frame,
            };
            walk.instances.extend(instance);
            let result = context.collect_node(node, walk);
            if instance.is_some() {
                walk.instances.pop();
            }
            result?;
        }
        Ok(())
    }

    fn collect_media_in(
        &self,
        media_in_node: &media_in::MediaIn,
        collector: &mut RenderRequirements,
    ) -> Result<(), LumenError> {
        match media_in_node.resolve(&self.expr_context())? {
            media_in::MediaInKind::Image { image_id } => {
                tracing::trace!(
                    target: "lumen_media",
                    frame = self.frame.value(),
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
                let resolver = self.media_store.get_video_resolver(&stream_id)?;
                let metadata = resolver.metadata();
                let source_frame = media_in::map_to_source_frame(
                    self.frame,
                    self.scope.scene.timeline.fps,
                    metadata.fps,
                    metadata.frame_count,
                    &range,
                    speed,
                    loop_mode,
                )
                .ok_or_else(|| MediaError::FrameOutOfRange {
                    media_source: stream_id.clone(),
                    frame: self.frame.value() as u32,
                    frame_count: metadata.frame_count,
                })?;
                tracing::trace!(
                    target: "lumen_media",
                    frame = self.frame.value(),
                    stream_id = %stream_id,
                    source_frame,
                    "require video frame"
                );
                collector.add_video_frame(stream_id, source_frame);
            }
        }

        Ok(())
    }

    fn expr_context(&self) -> ExpressionContext<'_> {
        self.scope.expr_context(self.frame, Some(self.media_store))
    }
}
