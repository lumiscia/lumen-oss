//! Media resolver traits and shared metadata for image/video sources.

use std::{
    collections::{BTreeSet, HashMap},
    fmt::Debug,
    sync::Arc,
};

use crate::{
    audio::{AudioResolver, AudioSourceProvider},
    composition::Composition,
    error::{GraphValidationError, LumenError, MediaError},
    expr::ExpressionContext,
    node::{NodeId, NodeKind, source::media_in},
    raster::ImageFrame,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct ImageMetadata {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct VideoMetadata {
    pub width: u32,
    pub height: u32,
    pub frame_count: u32,
}

pub trait ImageResolver: Send + Sync {
    fn id(&self) -> &str;

    fn metadata(&self) -> ImageMetadata;

    fn resolve_image(&self) -> Result<Arc<ImageFrame>, MediaError>;
}

pub trait VideoFrameResolver: Send + Sync {
    fn id(&self) -> &str;

    fn metadata(&self) -> VideoMetadata;

    fn resolve_frame_image(&self, frame: u32) -> Result<Arc<ImageFrame>, MediaError>;
}

pub trait MediaStore: Send + Sync + Debug {
    fn get_image_resolver(&self, source: &str) -> Option<Box<dyn ImageResolver>>;

    fn get_video_resolver(&self, stream_id: &str) -> Option<Box<dyn VideoFrameResolver>>;

    fn get_audio_resolver(&self, _source_id: &str) -> Option<Box<dyn AudioResolver>> {
        None
    }
}

impl<T: MediaStore + ?Sized> AudioSourceProvider for T {
    fn get_audio_resolver(&self, source_id: &str) -> Option<Box<dyn AudioResolver>> {
        MediaStore::get_audio_resolver(self, source_id)
    }
}

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

pub fn collect_frame_requirements<M: MediaStore>(
    composition: &Composition,
    media_store: &M,
    frame: u32,
) -> Result<FrameRequirements, LumenError> {
    let expr_context = ExpressionContext {
        frame,
        fps: composition.timeline.fps,
        width: composition.render_settings.width,
        height: composition.render_settings.height,
        duration_frames: composition.timeline.duration_frames,
        path: Some("media_requirements".to_string()),
        graph: Some(&composition.graph),
    };

    let mut images = BTreeSet::new();
    let mut videos: HashMap<String, BTreeSet<u32>> = HashMap::new();

    let required_nodes = reachable_output_nodes(composition)?;

    for node_id in required_nodes {
        let Some(node) = composition.graph.nodes.get(&node_id) else {
            continue;
        };
        let NodeKind::MediaIn(media_in_node) = node else {
            continue;
        };

        match media_in::resolve_for_context(media_in_node, &expr_context)? {
            media_in::MediaInKind::Image { image_id } => {
                images.insert(image_id);
            }
            media_in::MediaInKind::Video {
                stream_id,
                range,
                speed,
                loop_mode,
            } => {
                let resolver = media_store.get_video_resolver(&stream_id).ok_or_else(|| {
                    MediaError::SourceNotFound {
                        media_source: stream_id.clone(),
                    }
                })?;
                let metadata = resolver.metadata();
                let source_frame = media_in::map_to_source_frame(
                    frame,
                    metadata.frame_count,
                    range.as_ref(),
                    speed,
                    loop_mode,
                )
                .ok_or_else(|| MediaError::FrameOutOfRange {
                    media_source: stream_id.clone(),
                    frame,
                    frame_count: metadata.frame_count,
                })?;
                videos.entry(stream_id).or_default().insert(source_frame);
            }
        }
    }

    let videos = videos
        .into_iter()
        .map(|(stream_id, frames)| VideoFrameRequirement {
            stream_id,
            frames: frames.into_iter().collect(),
        })
        .collect();

    Ok(FrameRequirements {
        images: images.into_iter().collect(),
        videos,
    })
}

fn reachable_output_nodes(composition: &Composition) -> Result<BTreeSet<NodeId>, LumenError> {
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

    let mut visited = BTreeSet::new();
    visit_upstream(composition, output_node_id, &mut visited);
    Ok(visited)
}

fn visit_upstream(composition: &Composition, node_id: NodeId, visited: &mut BTreeSet<NodeId>) {
    if !visited.insert(node_id) {
        return;
    }

    for connection in composition
        .graph
        .connections
        .iter()
        .filter(|connection| connection.to_node == node_id)
    {
        visit_upstream(composition, connection.from_node, visited);
    }
}

pub fn premultiply_rgba_in_place_if_needed(pixels: &mut [u8]) {
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = u16::from(pixel[3]);
        if alpha == u16::from(u8::MAX) {
            continue;
        }
        if alpha == 0 {
            pixel[0] = 0;
            pixel[1] = 0;
            pixel[2] = 0;
            continue;
        }
        for channel in &mut pixel[..3] {
            *channel = ((u16::from(*channel) * alpha) + 127)
                .checked_div(u16::from(u8::MAX))
                .unwrap_or(0) as u8;
        }
    }
}
