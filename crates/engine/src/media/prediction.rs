use std::collections::{BTreeMap, BTreeSet};

use crate::{
    composition::Composition,
    error::{LumenError, MediaError},
    expr::ExpressionContext,
    node::{NodeKind, PortRef, source::media_in},
    scope::{Routed, Scope},
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
    let scope = Scope::root(composition);
    let output_port = PortRef::new(scope.media_output()?, "output".to_string());
    let mut collector = RenderRequirements::default();
    let context = RequirementContext {
        scope,
        media_store,
        frame: f64::from(frame),
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
    scope: Scope<'a>,
    media_store: &'a M,
    frame: f64,
}

impl<'a, M: MediaStore> RequirementContext<'a, M> {
    fn collect_port(
        &self,
        port: &PortRef,
        collector: &mut RenderRequirements,
    ) -> Result<(), LumenError> {
        if port.is_empty() {
            return Ok(());
        }

        let Some(node) = self.scope.node(port.id) else {
            return Ok(());
        };
        if let NodeKind::MediaIn(media_in_node) = node {
            return self.collect_media_in(media_in_node, collector);
        }
        for Routed {
            port, frame, scope, ..
        } in crate::scope::route(&self.scope, node, self.frame, Some(self.media_store))?
        {
            let context = RequirementContext {
                scope,
                media_store: self.media_store,
                frame,
            };
            context.collect_port(&port, collector)?;
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
                    self.scope.timeline.fps,
                    metadata.fps,
                    metadata.frame_count,
                    range.as_ref(),
                    speed,
                    loop_mode,
                )
                .ok_or_else(|| MediaError::FrameOutOfRange {
                    media_source: stream_id.clone(),
                    frame: self.frame as u32,
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

    fn expr_context(&self, path: &str) -> ExpressionContext<'_> {
        self.scope
            .expr_context(self.frame, path.to_string(), Some(self.media_store))
    }
}
