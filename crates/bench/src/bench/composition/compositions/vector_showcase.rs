use lumen_engine::{
    composition::Composition,
    graph::{Connection, Graph},
    node::{
        Deferred, NodeId, NodeKind,
        compositing::raster_multimerge::{RasterMultiMerge, RasterMultiMergeParamsDelegate},
        media_output::MediaOutput,
        source::{
            background::{Background, BackgroundParamsDelegate},
            text::{Text, TextParamsDelegate},
        },
        vector::{
            path::{Path, PathParamsDelegate},
            shape::{Shape, ShapeGeometryKind, ShapeParamsDelegate},
        },
    },
};

use super::{render_720p, timeline};
use crate::bench::CompositionFixture;

pub struct VectorShowcase;

impl CompositionFixture for VectorShowcase {
    fn name(&self) -> &'static str {
        "vector_showcase"
    }

    fn build(&self) -> Composition {
        let background = NodeId::new(1);
        let shape = NodeId::new(2);
        let path = NodeId::new(3);
        let title = NodeId::new(4);
        let subtitle = NodeId::new(5);
        let merge = NodeId::new(6);
        let output = NodeId::new(7);
        let mut graph = Graph::new();

        graph.nodes.insert(
            background,
            NodeKind::Background(Background {
                id: background,
                params: BackgroundParamsDelegate {
                    paint: lumen_engine::node::vector::paint::PaintDelegate::from(
                        lumen_engine::node::vector::paint::Paint::solid([8, 10, 18, 255]),
                    ),
                    width: Deferred::value(1280),
                    height: Deferred::value(720),
                    paint_supersample: Deferred::value(true),
                },
            }),
        );
        graph.nodes.insert(
            shape,
            NodeKind::Shape(Shape {
                id: shape,
                params: ShapeParamsDelegate {
                    geometry_kind: ShapeGeometryKind::Ellipse.into(),
                    width: Deferred::value(420),
                    height: Deferred::value(420),
                    position: Deferred::value((210.0, 150.0)),
                    fill_enabled: Deferred::value(true),
                    fill_paint: lumen_engine::node::vector::paint::Paint::solid([39, 52, 105, 255])
                        .into(),
                    stroke_enabled: Deferred::value(true),
                    stroke_paint: lumen_engine::node::vector::paint::Paint::solid([
                        94, 234, 212, 255,
                    ])
                    .into(),
                    stroke_width: Deferred::value(4.0),
                    ..Default::default()
                },
            }),
        );
        graph.nodes.insert(
            path,
            NodeKind::Path(Path {
                id: path,
                params: PathParamsDelegate {
                    data: Deferred::value(
                        "0,-115 35,-38 122,-35 52,18 75,105 0,58 -75,105 -52,18 -122,-35 -35,-38"
                            .to_string(),
                    ),
                    position: Deferred::value((420.0, 360.0)),
                    fill_enabled: Deferred::value(true),
                    fill_paint: lumen_engine::node::vector::paint::Paint::solid([
                        244, 114, 182, 220,
                    ])
                    .into(),
                    stroke_enabled: Deferred::value(true),
                    stroke_paint: lumen_engine::node::vector::paint::Paint::solid([
                        254, 240, 138, 255,
                    ])
                    .into(),
                    stroke_width: Deferred::value(5.0),
                    ..Default::default()
                },
            }),
        );
        graph.nodes.insert(
            title,
            NodeKind::Text(Text {
                id: title,
                params: TextParamsDelegate {
                    content: Deferred::value("Feature showcase".to_string()),
                    font_size: Deferred::value(48.0),
                    font_weight: Deferred::value(700),
                    max_width: Deferred::value(520.0),
                    position: Deferred::value((680.0, 245.0)),
                    color: lumen_engine::node::vector::paint::Paint::solid([248, 250, 252, 255])
                        .into(),
                    ..Default::default()
                },
            }),
        );
        graph.nodes.insert(
            subtitle,
            NodeKind::Text(Text {
                id: subtitle,
                params: TextParamsDelegate {
                    content: Deferred::value("shape, path, text, raster_multimerge".to_string()),
                    font_size: Deferred::value(22.0),
                    font_weight: Deferred::value(400),
                    max_width: Deferred::value(520.0),
                    position: Deferred::value((684.0, 320.0)),
                    color: lumen_engine::node::vector::paint::Paint::solid([186, 198, 230, 255])
                        .into(),
                    ..Default::default()
                },
            }),
        );
        graph.nodes.insert(
            merge,
            NodeKind::RasterMultiMerge(RasterMultiMerge {
                id: merge,
                params: RasterMultiMergeParamsDelegate::default(),
                ..RasterMultiMerge::default()
            }),
        );
        graph.nodes.insert(
            output,
            NodeKind::MediaOutput(MediaOutput {
                id: output,
                ..MediaOutput::default()
            }),
        );

        for layer in [background, shape, path, title, subtitle] {
            graph
                .connect(Connection {
                    from_node: layer,
                    from_port: "output".to_string(),
                    to_node: merge,
                    to_port: "layers".to_string(),
                })
                .expect("connect layer -> multimerge");
        }
        graph
            .connect(Connection {
                from_node: merge,
                from_port: "output".to_string(),
                to_node: output,
                to_port: "source".to_string(),
            })
            .expect("connect multimerge -> output");

        Composition::new(graph, timeline(30.0, 180), render_720p([8, 10, 18, 255]))
    }
}
