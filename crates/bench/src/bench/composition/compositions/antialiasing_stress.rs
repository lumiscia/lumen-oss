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
            paint::{GradientPaint, GradientStop, Paint, PaintKind},
            path::{Path, PathParamsDelegate},
            shape::{Shape, ShapeParamsDelegate},
        },
    },
};

use super::{render_1080p, timeline};
use crate::bench::CompositionFixture;

pub struct AntialiasingStress {
    pub edge_antialias: bool,
}

impl CompositionFixture for AntialiasingStress {
    fn name(&self) -> &'static str {
        if self.edge_antialias {
            "antialiasing_stress_aa"
        } else {
            "antialiasing_stress_noaa"
        }
    }

    fn default_frames(&self, composition: &Composition) -> u32 {
        composition.timeline.duration_frames.min(120)
    }

    fn build(&self) -> Composition {
        let background = NodeId::new(1);
        let diagonal_a = NodeId::new(2);
        let diagonal_b = NodeId::new(3);
        let diagonal_c = NodeId::new(4);
        let hard_gradient = NodeId::new(5);
        let label = NodeId::new(6);
        let merge = NodeId::new(7);
        let output = NodeId::new(8);
        let mut graph = Graph::new();

        let background_paint = Paint::Gradient(GradientPaint {
            kind: PaintKind::LinearGradient,
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: [7, 9, 13, 255],
                },
                GradientStop {
                    offset: 0.498,
                    color: [7, 9, 13, 255],
                },
                GradientStop {
                    offset: 0.502,
                    color: [75, 27, 58, 255],
                },
                GradientStop {
                    offset: 1.0,
                    color: [20, 28, 40, 255],
                },
            ],
            start: [240.35, 84.65],
            end: [1710.85, 988.15],
            ..Default::default()
        });

        graph.nodes.insert(
            background,
            NodeKind::Background(Background {
                id: background,
                params: BackgroundParamsDelegate {
                    paint: background_paint.into(),
                    width: Deferred::value(1920),
                    height: Deferred::value(1080),
                    paint_supersample: Deferred::value(true),
                },
            }),
        );

        let path =
            |id: NodeId, data: &str, position: (f64, f64), fill: Paint, stroke_enabled: bool| {
                NodeKind::Path(Path {
                    id,
                    params: PathParamsDelegate {
                        data: Deferred::value(data.to_string()),
                        position: Deferred::value(position),
                        fill_enabled: Deferred::value(true),
                        fill_paint: fill.into(),
                        stroke_enabled: Deferred::value(stroke_enabled),
                        stroke_paint: Paint::solid([255, 255, 255, 210]).into(),
                        stroke_width: Deferred::value(2.0),
                        edge_antialias: Deferred::value(self.edge_antialias),
                    },
                })
            };

        graph.nodes.insert(
            diagonal_a,
            path(
                diagonal_a,
                "0,0 1510.25,486.75 1507.75,494.25 -2.5,7.5",
                (204.35, 250.65),
                Paint::solid([255, 255, 255, 230]),
                false,
            ),
        );
        graph.nodes.insert(
            diagonal_b,
            path(
                diagonal_b,
                "0,0 1220.5,-340.25 1222.25,-333.75 1.75,6.5",
                (352.4, 830.35),
                Paint::Gradient(GradientPaint {
                    kind: PaintKind::LinearGradient,
                    stops: vec![
                        GradientStop {
                            offset: 0.0,
                            color: [96, 165, 250, 245],
                        },
                        GradientStop {
                            offset: 1.0,
                            color: [250, 204, 21, 245],
                        },
                    ],
                    start: [350.0, 492.0],
                    end: [1575.0, 830.0],
                    ..Default::default()
                }),
                false,
            ),
        );
        graph.nodes.insert(
            diagonal_c,
            path(
                diagonal_c,
                "0,0 1002.75,118.35 1001.85,125.85 -0.9,7.5",
                (465.55, 526.25),
                Paint::solid([248, 113, 113, 230]),
                true,
            ),
        );
        graph.nodes.insert(
            hard_gradient,
            NodeKind::Shape(Shape {
                id: hard_gradient,
                params: ShapeParamsDelegate {
                    width: Deferred::value(460),
                    height: Deferred::value(220),
                    position: Deferred::value((222.45, 690.35)),
                    fill_enabled: Deferred::value(true),
                    fill_paint: Paint::Gradient(GradientPaint {
                        kind: PaintKind::LinearGradient,
                        stops: vec![
                            GradientStop {
                                offset: 0.0,
                                color: [14, 165, 233, 255],
                            },
                            GradientStop {
                                offset: 0.495,
                                color: [14, 165, 233, 255],
                            },
                            GradientStop {
                                offset: 0.505,
                                color: [244, 63, 94, 255],
                            },
                            GradientStop {
                                offset: 1.0,
                                color: [244, 63, 94, 255],
                            },
                        ],
                        start: [222.45, 690.35],
                        end: [682.45, 910.35],
                        ..Default::default()
                    })
                    .into(),
                    stroke_enabled: Deferred::value(true),
                    stroke_paint: Paint::solid([255, 255, 255, 220]).into(),
                    stroke_width: Deferred::value(3.0),
                    edge_antialias: Deferred::value(self.edge_antialias),
                    ..Default::default()
                },
            }),
        );
        graph.nodes.insert(
            label,
            NodeKind::Text(Text {
                id: label,
                params: TextParamsDelegate {
                    content: Deferred::value("subpixel diagonals".to_string()),
                    font_size: Deferred::value(68.0),
                    font_weight: Deferred::value(800),
                    max_width: Deferred::value(760.0),
                    position: Deferred::value((1120.35, 770.65)),
                    color: Paint::Gradient(GradientPaint {
                        kind: PaintKind::LinearGradient,
                        stops: vec![
                            GradientStop {
                                offset: 0.0,
                                color: [255, 77, 109, 255],
                            },
                            GradientStop {
                                offset: 0.5,
                                color: [63, 220, 255, 255],
                            },
                            GradientStop {
                                offset: 1.0,
                                color: [255, 226, 89, 255],
                            },
                        ],
                        start: [1120.0, 770.0],
                        end: [1800.0, 860.0],
                        ..Default::default()
                    })
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

        for layer in [
            background,
            diagonal_a,
            diagonal_b,
            diagonal_c,
            hard_gradient,
            label,
        ] {
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

        Composition::new(graph, timeline(30.0, 120), render_1080p([7, 9, 13, 255]))
    }
}
