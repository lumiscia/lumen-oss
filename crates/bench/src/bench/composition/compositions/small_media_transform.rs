use lumen_engine::{
    composition::Composition,
    graph::{Connection, Graph},
    node::{
        Deferred, NodeId, NodeKind,
        media_output::MediaOutput,
        processing::{
            exposure::Exposure,
            transform::{Transform, TransformParamsDelegate},
        },
        source::media_in::{MediaIn, MediaInParamsDelegate},
    },
};

use super::{render_1080p, timeline};
use crate::bench::{
    CompositionFixture,
    media::{BenchmarkMediaStore, InMemoryImage},
};

const MEDIA_ID: &str = "small-transform-input";
const MEDIA_WIDTH: u32 = 320;
const MEDIA_HEIGHT: u32 = 180;

pub struct SmallMediaTransform;
pub struct SmallMediaTransformExposure;

impl CompositionFixture for SmallMediaTransform {
    fn name(&self) -> &'static str {
        "small_media_transform"
    }

    fn build(&self) -> Composition {
        build_small_media_transform(false)
    }

    fn media_store(&self) -> BenchmarkMediaStore {
        small_media_store()
    }
}

impl CompositionFixture for SmallMediaTransformExposure {
    fn name(&self) -> &'static str {
        "small_media_transform_exposure"
    }

    fn build(&self) -> Composition {
        build_small_media_transform(true)
    }

    fn media_store(&self) -> BenchmarkMediaStore {
        small_media_store()
    }
}

fn build_small_media_transform(with_exposure: bool) -> Composition {
    let media = NodeId::new(1);
    let transform = NodeId::new(2);
    let exposure = NodeId::new(3);
    let output = NodeId::new(4);
    let mut graph = Graph::new();
    graph.nodes.insert(
        media,
        NodeKind::MediaIn(MediaIn {
            id: media,
            params: MediaInParamsDelegate {
                kind: Deferred::value(0),
                source: Deferred::value(MEDIA_ID.to_string()),
                ..Default::default()
            },
        }),
    );
    graph.nodes.insert(
        transform,
        NodeKind::Transform(Transform {
            id: transform,
            params: TransformParamsDelegate {
                translate_x: Deferred::value(800.0),
                translate_y: Deferred::value(450.0),
                ..Default::default()
            },
            ..Transform::default()
        }),
    );
    if with_exposure {
        graph.nodes.insert(
            exposure,
            NodeKind::Exposure(Exposure {
                id: exposure,
                ..Exposure::default()
            }),
        );
    }
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            ..MediaOutput::default()
        }),
    );
    graph
        .connect(Connection {
            from_node: media,
            from_port: "output".to_string(),
            to_node: transform,
            to_port: "source".to_string(),
        })
        .expect("connect media -> transform");
    let output_source = if with_exposure {
        graph
            .connect(Connection {
                from_node: transform,
                from_port: "output".to_string(),
                to_node: exposure,
                to_port: "source".to_string(),
            })
            .expect("connect transform -> exposure");
        exposure
    } else {
        transform
    };
    graph
        .connect(Connection {
            from_node: output_source,
            from_port: "output".to_string(),
            to_node: output,
            to_port: "source".to_string(),
        })
        .expect("connect pipeline -> output");

    Composition::new(graph, timeline(30.0, 1_200), render_1080p([0, 0, 0, 255]))
}

fn small_media_store() -> BenchmarkMediaStore {
    BenchmarkMediaStore::Image(InMemoryImage::checkerboard(
        MEDIA_ID,
        MEDIA_WIDTH,
        MEDIA_HEIGHT,
    ))
}

#[cfg(test)]
mod tests {
    use lumen_engine::{media::MediaStore, node::NodeKind};

    use super::*;

    #[test]
    fn fixture_is_small_media_transformed_into_a_1080p_canvas() {
        let fixture = SmallMediaTransform;
        let composition = fixture.build();
        let media = fixture.media_store();

        assert_eq!(composition.render_settings.width, 1920);
        assert_eq!(composition.render_settings.height, 1080);
        let resolver = media.get_image_resolver(MEDIA_ID).unwrap();
        assert_eq!(resolver.metadata().width, MEDIA_WIDTH);
        assert_eq!(resolver.metadata().height, MEDIA_HEIGHT);
        assert!(
            composition
                .graph
                .nodes
                .values()
                .any(|node| matches!(node, NodeKind::Transform(_)))
        );
        let media_source_matches = composition.graph.nodes.values().any(|node| match node {
            NodeKind::MediaIn(node) => {
                matches!(&node.params.source, Deferred::Value(source) if source == MEDIA_ID)
            }
            _ => false,
        });
        assert!(media_source_matches);
    }

    #[test]
    fn exposure_fixture_adds_exactly_one_downstream_filter() {
        let direct = SmallMediaTransform.build();
        let filtered = SmallMediaTransformExposure.build();

        assert_eq!(
            (
                filtered.render_settings.width,
                filtered.render_settings.height,
                filtered.render_settings.background_color,
            ),
            (
                direct.render_settings.width,
                direct.render_settings.height,
                direct.render_settings.background_color,
            )
        );
        assert_eq!(filtered.timeline.fps, direct.timeline.fps);
        assert_eq!(
            filtered.timeline.duration_frames,
            direct.timeline.duration_frames
        );
        assert_eq!(filtered.graph.nodes.len(), direct.graph.nodes.len() + 1);
        assert!(
            !direct
                .graph
                .nodes
                .values()
                .any(|node| matches!(node, NodeKind::Exposure(_)))
        );
        assert!(
            filtered
                .graph
                .nodes
                .values()
                .any(|node| matches!(node, NodeKind::Exposure(_)))
        );
        let NodeKind::MediaOutput(direct_output) = direct.graph.nodes.get(&NodeId::new(4)).unwrap()
        else {
            panic!("expected direct media output")
        };
        let NodeKind::Exposure(filtered_exposure) =
            filtered.graph.nodes.get(&NodeId::new(3)).unwrap()
        else {
            panic!("expected downstream exposure")
        };
        let NodeKind::MediaOutput(filtered_output) =
            filtered.graph.nodes.get(&NodeId::new(4)).unwrap()
        else {
            panic!("expected filtered media output")
        };
        assert_eq!(direct_output.source.id, NodeId::new(2));
        assert_eq!(filtered_exposure.source.id, NodeId::new(2));
        assert_eq!(filtered_output.source.id, NodeId::new(3));
    }
}
