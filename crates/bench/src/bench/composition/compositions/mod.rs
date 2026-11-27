mod animated_showcase;
mod antialiasing_stress;
mod many_elements;
mod simple_pipeline;
mod small_media_transform;

use lumen_engine::composition::{RenderSettings, TimelineSettings};
use lumen_engine::graph::{Connection, Graph};
use lumen_engine::node::vector::drawing::{
    Fill2d, Fill2dParamsDelegate, Merge2d, Render2d, Stroke2d, Stroke2dParamsDelegate,
};
use lumen_engine::node::{NodeId, NodeKind, OutputPort};

use crate::bench::CompositionFixture;

pub fn all() -> Vec<Box<dyn CompositionFixture>> {
    vec![
        Box::new(simple_pipeline::SimplePipeline),
        Box::new(small_media_transform::SmallMediaTransform),
        Box::new(small_media_transform::SmallMediaTransformExposure),
        Box::new(animated_showcase::AnimatedShowcase),
        Box::new(antialiasing_stress::AntialiasingStress {
            edge_antialias: true,
        }),
        Box::new(antialiasing_stress::AntialiasingStress {
            edge_antialias: false,
        }),
        Box::new(many_elements::ManyElements),
    ]
}

pub fn by_name(name: &str) -> Option<Box<dyn CompositionFixture>> {
    all().into_iter().find(|fixture| fixture.name() == name)
}

pub(crate) fn timeline(fps: f32, duration_frames: u32) -> TimelineSettings {
    TimelineSettings {
        fps,
        duration_frames,
    }
}

/// Inserts `geometry` filled by `fill` and, when there is one, stroked over it by `stroke`,
/// drawn by a `render_2d` of id `render`. The geometry, style and merge nodes take their ids from
/// `id`.
pub(crate) fn insert_drawn(
    graph: &mut Graph,
    render: NodeId,
    id: &mut impl FnMut() -> NodeId,
    geometry: impl FnOnce(NodeId) -> NodeKind,
    fill: Fill2dParamsDelegate,
    stroke: Option<Stroke2dParamsDelegate>,
) {
    let connect = |graph: &mut Graph, from_node, to_node, to_port: &str| {
        graph
            .connect(Connection {
                from_node,
                from_port: OutputPort::DEFAULT,
                to_node,
                to_port: to_port.to_string(),
            })
            .expect("connect a drawing");
    };
    let outline = id();
    graph.nodes.insert(outline, geometry(outline));
    let filled = id();
    graph.nodes.insert(
        filled,
        NodeKind::Fill2d(Fill2d {
            id: filled,
            params: fill,
            ..Fill2d::default()
        }),
    );
    connect(graph, outline, filled, "geometry");
    let mut drawing = filled;
    if let Some(stroke) = stroke {
        let stroked = id();
        graph.nodes.insert(
            stroked,
            NodeKind::Stroke2d(Stroke2d {
                id: stroked,
                params: stroke,
                ..Stroke2d::default()
            }),
        );
        connect(graph, outline, stroked, "geometry");
        drawing = id();
        graph.nodes.insert(
            drawing,
            NodeKind::Merge2d(Merge2d {
                id: drawing,
                ..Merge2d::default()
            }),
        );
        connect(graph, filled, drawing, "drawings");
        connect(graph, stroked, drawing, "drawings");
    }
    graph.nodes.insert(
        render,
        NodeKind::Render2d(Render2d {
            id: render,
            ..Render2d::default()
        }),
    );
    connect(graph, drawing, render, "drawing");
}

pub(crate) fn render_720p() -> RenderSettings {
    RenderSettings {
        width: 1280,
        height: 720,
    }
}

pub(crate) fn render_1080p() -> RenderSettings {
    RenderSettings {
        width: 1920,
        height: 1080,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_media_transform_fixtures_are_registered() {
        let names: Vec<_> = all().into_iter().map(|fixture| fixture.name()).collect();
        assert!(names.contains(&"small_media_transform"));
        assert!(names.contains(&"small_media_transform_exposure"));
        assert_eq!(
            by_name("small_media_transform").unwrap().name(),
            "small_media_transform"
        );
        assert_eq!(
            by_name("small_media_transform_exposure").unwrap().name(),
            "small_media_transform_exposure"
        );
    }
}
