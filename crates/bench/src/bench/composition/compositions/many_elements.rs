use lumen_engine::{
    composition::Composition,
    expr::Expression,
    graph::{Connection, Graph},
    node::{
        Deferred, NodeId, NodeKind,
        compositing::raster_multimerge::{RasterMultiMerge, RasterMultiMergeParamsDelegate},
        media_output::MediaOutput,
        processing::opacity::{Opacity, OpacityParamsDelegate},
        source::{
            background::{Background, BackgroundParamsDelegate},
            text::{Text, TextParamsDelegate},
        },
        vector::{
            paint::Paint,
            shape::{Shape, ShapeGeometryKind, ShapeParamsDelegate},
        },
    },
};

use super::{render_1080p, timeline};
use crate::bench::CompositionFixture;

const COLUMNS: usize = 25;
const ROWS: usize = 12;
const DOTS: usize = 100;

/// Hundreds of small labelled cards and dots over a 1080p background. Every card is a
/// rounded rectangle and a text label merged into a group; every fifth card slides, and
/// every seventh group fades in.
pub struct ManyElements;

impl CompositionFixture for ManyElements {
    fn name(&self) -> &'static str {
        "many_elements"
    }

    fn default_frames(&self, composition: &Composition) -> u32 {
        composition.timeline.duration_frames.min(120)
    }

    fn build(&self) -> Composition {
        let mut graph = Graph::new();
        let mut next_id = 1;
        let mut id = || {
            let node = NodeId::new(next_id);
            next_id += 1;
            node
        };
        let connect = |graph: &mut Graph, from: NodeId, to: NodeId, port: &str| {
            graph
                .connect(Connection {
                    from_node: from,
                    from_port: "output".to_string(),
                    to_node: to,
                    to_port: port.to_string(),
                })
                .expect("connect many_elements node");
        };

        let background = id();
        graph.nodes.insert(
            background,
            NodeKind::Background(Background {
                id: background,
                params: BackgroundParamsDelegate {
                    paint: Paint::solid([12, 14, 20, 255]).into(),
                    width: Deferred::value(1920),
                    height: Deferred::value(1080),
                    paint_supersample: Deferred::value(false),
                },
            }),
        );
        let mut layers = vec![background];

        for index in 0..COLUMNS * ROWS {
            let (column, row) = (index % COLUMNS, index / COLUMNS);
            let (x, y) = (24.0 + column as f64 * 75.0, 40.0 + row as f64 * 84.0);
            let card = id();
            let position = if index % 5 == 0 {
                Deferred::Expr(
                    Expression::parse(&format!("vec2({x} + 8 * sin(frame / 9), {y})")).unwrap(),
                )
            } else {
                Deferred::value((x, y))
            };
            graph.nodes.insert(
                card,
                NodeKind::Shape(Shape {
                    id: card,
                    params: ShapeParamsDelegate {
                        geometry_kind: ShapeGeometryKind::Rectangle.into(),
                        width: Deferred::value(60),
                        height: Deferred::value(36),
                        border_radius: Deferred::value(6.0),
                        position,
                        fill_enabled: Deferred::value(true),
                        fill_paint: Paint::solid([40 + (index % 7) as u8 * 20, 60, 110, 255])
                            .into(),
                        stroke_enabled: Deferred::value(true),
                        stroke_paint: Paint::solid([200, 210, 240, 255]).into(),
                        stroke_width: Deferred::value(1.5),
                        ..Default::default()
                    },
                }),
            );
            let label = id();
            graph.nodes.insert(
                label,
                NodeKind::Text(Text {
                    id: label,
                    params: TextParamsDelegate {
                        content: Deferred::value(format!("#{index:03}")),
                        font_size: Deferred::value(14.0),
                        position: Deferred::value((x + 8.0, y + 46.0)),
                        color: Paint::solid([230, 232, 240, 255]).into(),
                        ..Default::default()
                    },
                }),
            );
            let group = id();
            graph.nodes.insert(
                group,
                NodeKind::RasterMultiMerge(RasterMultiMerge {
                    id: group,
                    params: RasterMultiMergeParamsDelegate::default(),
                    ..RasterMultiMerge::default()
                }),
            );
            connect(&mut graph, card, group, "layers");
            connect(&mut graph, label, group, "layers");
            let layer = if index % 7 == 0 {
                let fade = id();
                graph.nodes.insert(
                    fade,
                    NodeKind::Opacity(Opacity {
                        id: fade,
                        params: OpacityParamsDelegate {
                            opacity: Deferred::Expr(
                                Expression::parse(&format!("smoothstep({row}, {row} + 30, frame)"))
                                    .unwrap(),
                            ),
                        },
                        ..Opacity::default()
                    }),
                );
                connect(&mut graph, group, fade, "source");
                fade
            } else {
                group
            };
            layers.push(layer);
        }

        for index in 0..DOTS {
            let dot = id();
            let (x, y) = (
                30.0 + (index * 191 % 1860) as f64,
                1050.0 - (index * 37 % 30) as f64,
            );
            graph.nodes.insert(
                dot,
                NodeKind::Shape(Shape {
                    id: dot,
                    params: ShapeParamsDelegate {
                        geometry_kind: ShapeGeometryKind::Ellipse.into(),
                        width: Deferred::value(12),
                        height: Deferred::value(12),
                        position: Deferred::value((x, y)),
                        fill_enabled: Deferred::value(true),
                        fill_paint: Paint::solid([94, 234, 212, 200]).into(),
                        ..Default::default()
                    },
                }),
            );
            layers.push(dot);
        }

        let scene = id();
        graph.nodes.insert(
            scene,
            NodeKind::RasterMultiMerge(RasterMultiMerge {
                id: scene,
                params: RasterMultiMergeParamsDelegate::default(),
                ..RasterMultiMerge::default()
            }),
        );
        for layer in layers {
            connect(&mut graph, layer, scene, "layers");
        }
        let output = id();
        graph.nodes.insert(
            output,
            NodeKind::MediaOutput(MediaOutput {
                id: output,
                ..MediaOutput::default()
            }),
        );
        connect(&mut graph, scene, output, "source");

        Composition::new(graph, timeline(60.0, 240), render_1080p([12, 14, 20, 255]))
    }
}
