use std::sync::Arc;

use lumen::{
    composition::{Composition, RenderSettings, TimelineSettings},
    error::MediaError,
    media::{
        ImageResolver, MediaStore, VideoFrameResolver, VideoMetadata, collect_frame_requirements,
    },
    node::{
        NodeId, NodeKind, NodeProperty, PortRef, media_output::MediaOutput,
        source::media_in::MediaIn,
    },
    raster::ImageFrame,
};

#[derive(Debug, Default)]
struct TestMediaStore {
    video_frame_count: u32,
}

impl MediaStore for TestMediaStore {
    fn get_image_resolver(&self, _source: &str) -> Option<Box<dyn ImageResolver>> {
        None
    }

    fn get_video_resolver(&self, source: &str) -> Option<Box<dyn VideoFrameResolver>> {
        Some(Box::new(TestVideoResolver {
            id: source.to_string(),
            frame_count: self.video_frame_count,
        }))
    }
}

#[derive(Debug)]
struct TestVideoResolver {
    id: String,
    frame_count: u32,
}

impl VideoFrameResolver for TestVideoResolver {
    fn id(&self) -> &str {
        &self.id
    }

    fn metadata(&self) -> VideoMetadata {
        VideoMetadata {
            width: 1920,
            height: 1080,
            frame_count: self.frame_count,
        }
    }

    fn resolve_frame_image(&self, frame: u32) -> Result<Arc<ImageFrame>, MediaError> {
        Err(MediaError::FrameOutOfRange {
            media_source: self.id.clone(),
            frame,
            frame_count: self.frame_count,
        })
    }
}

fn base_composition(graph: lumen::graph::Graph) -> Composition {
    Composition::new(
        graph,
        TimelineSettings {
            fps: 30.0,
            duration_frames: 120,
        },
        RenderSettings {
            width: 1920,
            height: 1080,
            background_color: [0, 0, 0, 255],
        },
    )
}

#[test]
fn collects_image_requirements_from_media_nodes() {
    let media_id = NodeId::new(1);
    let output_id = NodeId::new(2);
    let mut graph = lumen::graph::Graph::new();
    graph.nodes.insert(
        media_id,
        NodeKind::MediaIn(MediaIn {
            id: media_id,
            kind: NodeProperty::Int(0),
            source: NodeProperty::String("hero-image".to_string()),
            ..MediaIn::default()
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(media_id, "output".to_string()),
        }),
    );
    graph
        .connect(lumen::graph::Connection {
            from_node: media_id,
            from_port: "output".to_string(),
            to_node: output_id,
            to_port: "source".to_string(),
        })
        .expect("connect media output");

    let requirements =
        collect_frame_requirements(&base_composition(graph), &TestMediaStore::default(), 0)
            .expect("collect requirements");

    assert_eq!(requirements.images, vec!["hero-image".to_string()]);
    assert!(requirements.videos.is_empty());
}

#[test]
fn maps_video_requirements_with_range_speed_and_looping() {
    let media_id = NodeId::new(1);
    let output_id = NodeId::new(2);
    let mut graph = lumen::graph::Graph::new();
    graph.nodes.insert(
        media_id,
        NodeKind::MediaIn(MediaIn {
            id: media_id,
            kind: NodeProperty::Int(1),
            source: NodeProperty::String("intro-video".to_string()),
            range_start: NodeProperty::Int(10),
            range_end: NodeProperty::Int(14),
            speed: NodeProperty::Float(2.0),
            loop_mode: NodeProperty::Int(1),
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(media_id, "output".to_string()),
        }),
    );
    graph
        .connect(lumen::graph::Connection {
            from_node: media_id,
            from_port: "output".to_string(),
            to_node: output_id,
            to_port: "source".to_string(),
        })
        .expect("connect media output");

    let requirements = collect_frame_requirements(
        &base_composition(graph),
        &TestMediaStore {
            video_frame_count: 60,
        },
        3,
    )
    .expect("collect requirements");

    assert!(requirements.images.is_empty());
    assert_eq!(requirements.videos.len(), 1);
    assert_eq!(requirements.videos[0].source_id, "intro-video");
    assert_eq!(requirements.videos[0].frames, vec![12]);
}
