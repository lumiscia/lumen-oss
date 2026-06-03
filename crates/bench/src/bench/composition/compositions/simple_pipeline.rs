use lumen_engine::{
    composition::Composition,
    graph::{Connection, Graph},
    node::{
        Deferred, NodeId, NodeKind,
        media_output::MediaOutput,
        processing::exposure::{Exposure, ExposureParamsDelegate},
        source::background::{Background, BackgroundParamsDelegate},
    },
};

use super::{render_720p, timeline};
use crate::bench::CompositionFixture;

pub struct SimplePipeline;

impl CompositionFixture for SimplePipeline {
    fn name(&self) -> &'static str {
        "simple_pipeline"
    }

    fn build(&self) -> Composition {
        let background = NodeId::new(1);
        let exposure = NodeId::new(2);
        let output = NodeId::new(3);
        let mut graph = Graph::new();
        graph.nodes.insert(
            background,
            NodeKind::Background(Background {
                id: background,
                params: BackgroundParamsDelegate {
                    paint: lumen_engine::node::vector::paint::PaintDelegate::from(
                        lumen_engine::node::vector::paint::Paint::solid([9, 17, 31, 255]),
                    ),
                    width: Deferred::value(1280),
                    height: Deferred::value(720),
                    paint_supersample: Deferred::value(true),
                },
            }),
        );
        graph.nodes.insert(
            exposure,
            NodeKind::Exposure(Exposure {
                id: exposure,
                params: ExposureParamsDelegate {
                    exposure: Deferred::value(1.05),
                    contrast: Deferred::value(1.0),
                    offset: Deferred::value(0.0),
                },
                ..Exposure::default()
            }),
        );
        graph.nodes.insert(
            output,
            NodeKind::MediaOutput(MediaOutput {
                id: output,
                ..MediaOutput::default()
            }),
        );

        graph
            .connect(Connection {
                from_node: background,
                from_port: "output".to_string(),
                to_node: exposure,
                to_port: "source".to_string(),
            })
            .expect("connect background -> exposure");
        graph
            .connect(Connection {
                from_node: exposure,
                from_port: "output".to_string(),
                to_node: output,
                to_port: "source".to_string(),
            })
            .expect("connect exposure -> output");

        Composition::new(graph, timeline(30.0, 120), render_720p([9, 17, 31, 255]))
    }
}
