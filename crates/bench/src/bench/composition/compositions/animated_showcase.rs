use lumen_engine::{
    composition::Composition,
    expr::Expression,
    graph::{Connection, Graph},
    node::{
        Deferred, NodeId, NodeKind,
        compositing::merge::{Merge, MergeParamsDelegate},
        media_output::MediaOutput,
        source::{
            background::{Background, BackgroundParamsDelegate},
            text::{Text, TextParamsDelegate},
        },
        vector::shape::{Shape, ShapeGeometryKind, ShapeParamsDelegate},
    },
};

use super::{render_720p, timeline};
use crate::bench::CompositionFixture;

pub struct AnimatedShowcase;

impl CompositionFixture for AnimatedShowcase {
    fn name(&self) -> &'static str {
        "animated_showcase"
    }

    fn default_frames(&self, composition: &Composition) -> u32 {
        composition.timeline.duration_frames.min(180)
    }

    fn build(&self) -> Composition {
        let background = NodeId::new(1);
        let glow = NodeId::new(2);
        let card = NodeId::new(3);
        let headline = NodeId::new(4);
        let subhead = NodeId::new(5);
        let merge_bg_glow = NodeId::new(6);
        let merge_card = NodeId::new(7);
        let merge_headline = NodeId::new(8);
        let merge_subhead = NodeId::new(9);
        let output = NodeId::new(10);
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
            glow,
            NodeKind::Shape(Shape {
                id: glow,
                params: ShapeParamsDelegate {
                    geometry_kind: ShapeGeometryKind::Ellipse.into(),
                    width: Deferred::value(620),
                    height: Deferred::value(620),
                    position: Deferred::value((760.0, -140.0)),
                    fill_enabled: Deferred::value(true),
                    fill_paint: lumen_engine::node::vector::paint::Paint::solid([37, 99, 235, 68])
                        .into(),
                    ..Default::default()
                },
            }),
        );
        graph.nodes.insert(
            card,
            NodeKind::Shape(Shape {
                id: card,
                params: ShapeParamsDelegate {
                    width: Deferred::value(860),
                    height: Deferred::value(310),
                    border_radius: Deferred::value(28.0),
                    position: Deferred::value((210.0, 190.0)),
                    fill_enabled: Deferred::value(true),
                    fill_paint: lumen_engine::node::vector::paint::Paint::solid([15, 27, 49, 255])
                        .into(),
                    stroke_enabled: Deferred::value(true),
                    stroke_paint: lumen_engine::node::vector::paint::Paint::solid([
                        71, 85, 105, 255,
                    ])
                    .into(),
                    stroke_width: Deferred::value(1.5),
                    ..Default::default()
                },
            }),
        );
        graph.nodes.insert(
            headline,
            NodeKind::Text(Text {
                id: headline,
                params: TextParamsDelegate {
                    content: Deferred::value("GPU-native previews.".to_string()),
                    font_size: Deferred::value(42.0),
                    font_weight: Deferred::value(700),
                    max_width: Deferred::value(700.0),
                    position: Deferred::value((260.0, 255.0)),
                    color: lumen_engine::node::vector::paint::Paint::solid([248, 250, 252, 255])
                        .into(),
                    ..Default::default()
                },
            }),
        );
        graph.nodes.insert(
            subhead,
            NodeKind::Text(Text {
                id: subhead,
                params: TextParamsDelegate {
                    content: Deferred::value("Expressions update uniforms per frame.".to_string()),
                    font_size: Deferred::value(21.0),
                    font_weight: Deferred::value(500),
                    max_width: Deferred::value(730.0),
                    position: Deferred::value((260.0, 350.0)),
                    color: lumen_engine::node::vector::paint::Paint::solid([148, 163, 184, 255])
                        .into(),
                    ..Default::default()
                },
            }),
        );

        let fade_in = Expression::parse("smoothstep(0, 24, frame)").unwrap();
        let card_in = Expression::parse("smoothstep(18, 48, frame)").unwrap();
        let headline_in = Expression::parse("smoothstep(22, 54, frame)").unwrap();
        let subhead_in = Expression::parse("smoothstep(30, 60, frame)").unwrap();

        graph.nodes.insert(
            merge_bg_glow,
            NodeKind::Merge(Merge {
                id: merge_bg_glow,
                params: MergeParamsDelegate {
                    opacity: Deferred::Expr(fade_in),
                    ..Default::default()
                },
                ..Merge::default()
            }),
        );
        graph.nodes.insert(
            merge_card,
            NodeKind::Merge(Merge {
                id: merge_card,
                params: MergeParamsDelegate {
                    opacity: Deferred::Expr(card_in),
                    ..Default::default()
                },
                ..Merge::default()
            }),
        );
        graph.nodes.insert(
            merge_headline,
            NodeKind::Merge(Merge {
                id: merge_headline,
                params: MergeParamsDelegate {
                    opacity: Deferred::Expr(headline_in),
                    ..Default::default()
                },
                ..Merge::default()
            }),
        );
        graph.nodes.insert(
            merge_subhead,
            NodeKind::Merge(Merge {
                id: merge_subhead,
                params: MergeParamsDelegate {
                    opacity: Deferred::Expr(subhead_in),
                    ..Default::default()
                },
                ..Merge::default()
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
                to_node: merge_bg_glow,
                to_port: "base".to_string(),
            })
            .expect("connect background -> merge");
        graph
            .connect(Connection {
                from_node: glow,
                from_port: "output".to_string(),
                to_node: merge_bg_glow,
                to_port: "overlay".to_string(),
            })
            .expect("connect glow -> merge");
        graph
            .connect(Connection {
                from_node: merge_bg_glow,
                from_port: "output".to_string(),
                to_node: merge_card,
                to_port: "base".to_string(),
            })
            .expect("connect merge chain");
        graph
            .connect(Connection {
                from_node: card,
                from_port: "output".to_string(),
                to_node: merge_card,
                to_port: "overlay".to_string(),
            })
            .expect("connect card -> merge");
        graph
            .connect(Connection {
                from_node: merge_card,
                from_port: "output".to_string(),
                to_node: merge_headline,
                to_port: "base".to_string(),
            })
            .expect("connect merge chain");
        graph
            .connect(Connection {
                from_node: headline,
                from_port: "output".to_string(),
                to_node: merge_headline,
                to_port: "overlay".to_string(),
            })
            .expect("connect headline -> merge");
        graph
            .connect(Connection {
                from_node: merge_headline,
                from_port: "output".to_string(),
                to_node: merge_subhead,
                to_port: "base".to_string(),
            })
            .expect("connect merge chain");
        graph
            .connect(Connection {
                from_node: subhead,
                from_port: "output".to_string(),
                to_node: merge_subhead,
                to_port: "overlay".to_string(),
            })
            .expect("connect subhead -> merge");
        graph
            .connect(Connection {
                from_node: merge_subhead,
                from_port: "output".to_string(),
                to_node: output,
                to_port: "source".to_string(),
            })
            .expect("connect merge -> output");

        Composition::new(graph, timeline(30.0, 180), render_720p([9, 17, 31, 255]))
    }
}
