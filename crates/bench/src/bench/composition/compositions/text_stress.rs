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
            text::{Text, TextParamsDelegate, TextRenderMode},
        },
        vector::paint::Paint,
    },
};

use super::{render_1080p, timeline};
use crate::bench::CompositionFixture;

pub struct TextStress {
    pub render_mode: TextRenderMode,
}

impl CompositionFixture for TextStress {
    fn name(&self) -> &'static str {
        match self.render_mode {
            TextRenderMode::Msdf => "text_stress_msdf",
            TextRenderMode::Raster => "text_stress_raster",
        }
    }

    fn default_frames(&self, composition: &Composition) -> u32 {
        composition.timeline.duration_frames.min(120)
    }

    fn build(&self) -> Composition {
        let background = NodeId::new(1);
        let text = NodeId::new(2);
        let merge = NodeId::new(3);
        let output = NodeId::new(4);
        let mut graph = Graph::new();
        graph.nodes.insert(
            background,
            NodeKind::Background(Background {
                id: background,
                params: BackgroundParamsDelegate {
                    paint: Paint::solid([5, 8, 16, 255]).into(),
                    width: Deferred::value(1920),
                    height: Deferred::value(1080),
                    paint_supersample: Deferred::value(false),
                },
            }),
        );
        graph.nodes.insert(
            text,
            NodeKind::Text(Text {
                id: text,
                params: TextParamsDelegate {
                    content: Deferred::value(
                        "LUMEN 0123456789 — LARGE TYPE\nB8@ MWAV repeated counters and corners\nPersistent atlas animation stress"
                            .repeat(3),
                    ),
                    font_size: Deferred::Expr(
                        Expression::parse("96 + abs(sin(frame * 0.071)) * 144")
                            .expect("text stress font-size expression"),
                    ),
                    font_weight: Deferred::value(700),
                    max_width: Deferred::value(1760.0),
                    position: Deferred::value((72.25, 72.5)),
                    color: Paint::solid([245, 247, 255, 255]).into(),
                    paint_supersample: Deferred::value(false),
                    render_mode: self.render_mode.into(),
                    ..Default::default()
                },
                ..Text::default()
            }),
        );
        graph.nodes.insert(
            merge,
            NodeKind::Merge(Merge {
                id: merge,
                params: MergeParamsDelegate::default(),
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
        for (from, to, port) in [
            (background, merge, "base"),
            (text, merge, "overlay"),
            (merge, output, "source"),
        ] {
            graph
                .connect(Connection {
                    from_node: from,
                    from_port: "output".to_string(),
                    to_node: to,
                    to_port: port.to_string(),
                })
                .expect("connect text stress composition");
        }
        Composition::new(graph, timeline(30.0, 1_800), render_1080p([5, 8, 16, 255]))
    }
}
