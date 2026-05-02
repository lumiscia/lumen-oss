use std::sync::Arc;

use lumen::{
    composition::{Composition, RenderSettings, TimelineSettings},
    error::MediaError,
    gpu_image::{AlphaMode, GpuImageFrame, RectI},
    media::{
        ImageResolver, MediaStore, VideoFrameResolver, VideoMetadata, collect_frame_requirements,
    },
    node::{
        NodeId, NodeKind, NodeProperty, PortRef,
        compositing::switch::Switch,
        media_output::MediaOutput,
        processing::time_remap::TimeRemap,
        source::{media_in::MediaIn, solid_color::SolidColor},
    },
};

#[derive(Debug, Default)]
struct TestMediaStore {
    video_frame_count: u32,
}

impl MediaStore for TestMediaStore {
    fn get_image_resolver(&self, _source: &str) -> Option<Box<dyn ImageResolver>> {
        None
    }

    fn get_video_resolver(&self, stream_id: &str) -> Option<Box<dyn VideoFrameResolver>> {
        Some(Box::new(TestVideoResolver {
            id: stream_id.to_string(),
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

    fn frame(&self, frame: u32) -> Result<Arc<GpuImageFrame>, MediaError> {
        if frame >= self.frame_count {
            return Err(MediaError::FrameOutOfRange {
                media_source: self.id.clone(),
                frame,
                frame_count: self.frame_count,
            });
        }

        Ok(Arc::new(test_frame()))
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
        NodeId::new(3),
        NodeKind::MediaIn(MediaIn {
            id: NodeId::new(3),
            kind: NodeProperty::Int(0),
            source: NodeProperty::String("unused-image".to_string()),
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
    assert_eq!(requirements.videos[0].stream_id, "intro-video");
    assert_eq!(requirements.videos[0].frames, vec![12]);
}

#[test]
fn time_remap_collects_requirements_from_remapped_frame() {
    let media_id = NodeId::new(1);
    let remap_id = NodeId::new(2);
    let output_id = NodeId::new(3);
    let mut graph = lumen::graph::Graph::new();
    graph.nodes.insert(
        media_id,
        NodeKind::MediaIn(MediaIn {
            id: media_id,
            kind: NodeProperty::Int(1),
            source: NodeProperty::String("remapped-video".to_string()),
            ..MediaIn::default()
        }),
    );
    graph.nodes.insert(
        remap_id,
        NodeKind::TimeRemap(TimeRemap {
            id: remap_id,
            frame: NodeProperty::Float(7.0),
            loop_enabled: NodeProperty::Bool(false),
            loop_start: NodeProperty::Int(0),
            loop_end: NodeProperty::Int(0),
            source: PortRef::new(media_id, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(remap_id, "output".to_string()),
        }),
    );
    connect(&mut graph, media_id, "output", remap_id, "source");
    connect(&mut graph, remap_id, "output", output_id, "source");

    let requirements = collect_frame_requirements(
        &base_composition(graph),
        &TestMediaStore {
            video_frame_count: 60,
        },
        15,
    )
    .expect("collect requirements");

    assert_eq!(requirements.videos[0].stream_id, "remapped-video");
    assert_eq!(requirements.videos[0].frames, vec![7]);
}

#[test]
fn switch_collects_only_selected_branch_requirements() {
    let red_id = NodeId::new(1);
    let video_id = NodeId::new(2);
    let switch_id = NodeId::new(3);
    let output_id = NodeId::new(4);
    let mut graph = lumen::graph::Graph::new();
    graph.nodes.insert(
        red_id,
        NodeKind::SolidColor(SolidColor {
            id: red_id,
            color: NodeProperty::Color([255, 0, 0, 255]),
            width: NodeProperty::Int(1),
            height: NodeProperty::Int(1),
        }),
    );
    graph.nodes.insert(
        video_id,
        NodeKind::MediaIn(MediaIn {
            id: video_id,
            kind: NodeProperty::Int(1),
            source: NodeProperty::String("selected-video".to_string()),
            ..MediaIn::default()
        }),
    );
    graph.nodes.insert(
        switch_id,
        NodeKind::Switch(Switch {
            id: switch_id,
            map: [(0, 0..10), (1, 10..20)].into_iter().collect(),
            layers: vec![
                PortRef::new(red_id, "output".to_string()),
                PortRef::new(video_id, "output".to_string()),
            ],
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(switch_id, "output".to_string()),
        }),
    );
    connect(&mut graph, red_id, "output", switch_id, "layers");
    connect(&mut graph, video_id, "output", switch_id, "layers");
    connect(&mut graph, switch_id, "output", output_id, "source");

    let frame_5 = collect_frame_requirements(
        &base_composition(graph),
        &TestMediaStore {
            video_frame_count: 60,
        },
        5,
    )
    .expect("collect frame 5 requirements");

    assert!(frame_5.videos.is_empty());
}

fn connect(
    graph: &mut lumen::graph::Graph,
    from_node: NodeId,
    from_port: &str,
    to_node: NodeId,
    to_port: &str,
) {
    graph
        .connect(lumen::graph::Connection {
            from_node,
            from_port: from_port.to_string(),
            to_node,
            to_port: to_port.to_string(),
        })
        .expect("connect nodes");
}

fn test_frame() -> GpuImageFrame {
    GpuImageFrame::from_cpu_decoded_rgba(
        &[0, 0, 0, 255],
        1,
        1,
        4,
        AlphaMode::Premultiplied,
        RectI::from_size(1, 1),
        RectI::from_size(1, 1),
    )
    .expect("test frame")
}
