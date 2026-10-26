use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use lumen_engine::{
    composition::{Composition, RenderSettings, TimelineSettings},
    expr::{Expression, ExpressionContext},
    gpu::GpuCompositionRenderer,
    graph::Graph,
    media::{ImageResolver, MediaStore, VideoFrameResolver},
    node::{
        Deferred, NodeId, NodeKind,
        media_output::MediaOutput,
        source::text::{Text, TextParamsDelegate, TextRenderMode},
    },
};

use crate::bench::{
    Bench,
    report::{SummaryReport, format_duration},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BenchCase {
    Layout,
    Raster,
    Msdf,
    Emoji,
    MeasureLiteral,
    MeasureExpression,
    MeasureNestedReference,
}

impl BenchCase {
    const ALL: [Self; 7] = [
        Self::Layout,
        Self::Raster,
        Self::Msdf,
        Self::Emoji,
        Self::MeasureLiteral,
        Self::MeasureExpression,
        Self::MeasureNestedReference,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Layout => "layout",
            Self::Raster => "raster",
            Self::Msdf => "msdf",
            Self::Emoji => "emoji",
            Self::MeasureLiteral => "measure-literal",
            Self::MeasureExpression => "measure-expression",
            Self::MeasureNestedReference => "measure-nested-reference",
        }
    }
}

pub struct TextBench;
impl Bench for TextBench {
    fn name() -> &'static str {
        "text"
    }
    fn run() -> anyhow::Result<()> {
        let mut iterations = 100usize;
        let mut repeats = 1usize;
        let mut selection = "all".to_string();
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--iterations" => {
                    iterations = args
                        .next()
                        .context("--iterations requires a value")?
                        .parse()?
                }
                "--text-repeats" => {
                    repeats = args
                        .next()
                        .context("--text-repeats requires a value")?
                        .parse()?
                }
                "--case" => selection = args.next().context("--case requires a value")?,
                "--help" | "-h" | "--list" => {
                    println!(
                        "lumen-bench-text [--case all|layout|raster|msdf|emoji|measure-literal|measure-expression|measure-nested-reference] [--iterations N] [--text-repeats N]"
                    );
                    return Ok(());
                }
                _ => return Err(anyhow!("unknown argument `{arg}`")),
            }
        }
        if selection == "gpu-msdf" {
            selection = "msdf".to_string();
        }
        if selection != "all" && !BenchCase::ALL.iter().any(|case| case.name() == selection) {
            return Err(anyhow!("unknown text case `{selection}`"));
        }
        let mut summary = SummaryReport::new(
            "Text benchmark summary",
            ["case", "glyphs", "iterations", "cold", "total", "mean/iter"],
        );
        for case in BenchCase::ALL {
            if selection != "all" && selection != case.name() {
                continue;
            }
            let result = run_case(case, iterations, repeats)?;
            summary.push_row(vec![
                case.name().to_string(),
                result.glyph_count.to_string(),
                iterations.to_string(),
                format_duration(result.cold),
                format_duration(result.elapsed),
                format_duration(result.elapsed / iterations.max(1) as u32),
            ]);
        }
        summary.print();
        Ok(())
    }
}

struct BenchResult {
    cold: Duration,
    elapsed: Duration,
    glyph_count: usize,
}

#[derive(Debug)]
struct EmptyMedia;
impl MediaStore for EmptyMedia {
    fn get_image_resolver(&self, _: &str) -> Option<Box<dyn ImageResolver>> {
        None
    }
    fn get_video_resolver(&self, _: &str) -> Option<Box<dyn VideoFrameResolver>> {
        None
    }
}

fn run_case(case: BenchCase, iterations: usize, repeats: usize) -> anyhow::Result<BenchResult> {
    if matches!(
        case,
        BenchCase::MeasureLiteral
            | BenchCase::MeasureExpression
            | BenchCase::MeasureNestedReference
    ) {
        return run_measurement_case(case, iterations);
    }
    let content = if case == BenchCase::Emoji { "🍋✨🚀🎬 " } else {
        "Lumen: AV fi — مرحبا بالعالم שלום 123 — a\u{301} e\u{308} n\u{303}. Wrapping varied text across lines. "
    }.repeat(repeats.max(1));
    let mut request = lumen_text::TextLayoutRequest::new(content.clone());
    request.max_width = Some(1300.0);
    let mut system = lumen_text::TextSystem::new();
    let glyph_count = system.layout(&request).glyphs.len();
    let mut graph = Graph::new();
    graph.nodes.insert(
        NodeId::new(1),
        NodeKind::Text(Text {
            id: NodeId::new(1),
            params: TextParamsDelegate {
                content: Deferred::value(content),
                font_size: Deferred::Expr(Expression::parse("24 + frame % 96")?),
                max_width: Deferred::value(1300.0),
                position: Deferred::value((16.0, 16.0)),
                render_mode: if case == BenchCase::Msdf {
                    TextRenderMode::Msdf
                } else {
                    TextRenderMode::Raster
                }
                .into(),
                ..Default::default()
            },
        }),
    );
    graph.nodes.insert(
        NodeId::new(2),
        NodeKind::MediaOutput(MediaOutput {
            id: NodeId::new(2),
            source: lumen_engine::node::PortRef::new(NodeId::new(1), "output".to_string()),
        }),
    );
    let composition = Composition::new(
        graph,
        TimelineSettings {
            fps: 30.0,
            duration_frames: iterations as u32 + 1,
        },
        RenderSettings {
            width: 1400,
            height: 900,
            background_color: [0; 4],
        },
    );
    let mut renderer = if case == BenchCase::Layout {
        None
    } else {
        Some(pollster::block_on(GpuCompositionRenderer::new())?)
    };
    let mut run = |frame| -> anyhow::Result<()> {
        if let Some(renderer) = &mut renderer {
            let (_, submission) =
                renderer.render_frame_submitted(&composition, frame, &EmptyMedia)?;
            renderer
                .gpu_renderer()
                .device
                .poll(lumen_gpu::wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: None,
                })?;
        } else {
            request.font_size = 24.0 + (frame % 96) as f32;
            std::hint::black_box(system.layout(&request));
        }
        Ok(())
    };
    let start = Instant::now();
    run(0)?;
    let cold = start.elapsed();
    let start = Instant::now();
    for frame in 1..=iterations {
        run(frame as u32)?;
    }
    Ok(BenchResult {
        cold,
        elapsed: start.elapsed(),
        glyph_count,
    })
}

struct MeasurementFixture {
    graph: Option<Graph>,
    width: Expression,
    height: Expression,
}

impl MeasurementFixture {
    fn new(case: BenchCase) -> anyhow::Result<Self> {
        let mut graph = Graph::new();
        let target_id = NodeId::new(8);

        match case {
            BenchCase::MeasureLiteral => Ok(Self {
                graph: None,
                width: Expression::parse(
                    "text_width(\"Hello from Lumen measurement.\", 64, 1400, \"Roboto\")",
                )?,
                height: Expression::parse(
                    "text_height(\"Hello from Lumen measurement.\", 64, 1400, \"Roboto\")",
                )?,
            }),
            BenchCase::MeasureExpression => {
                graph.nodes.insert(
                    target_id,
                    NodeKind::Text(Text {
                        id: target_id,
                        params: lumen_engine::node::source::text::TextParamsDelegate {
                            content: Deferred::Expr(Expression::parse(
                                "\"Hello from Lumen measurement.\"",
                            )?),
                            font_family: Deferred::Expr(Expression::parse("\"Roboto\"")?),
                            font_size: Deferred::Expr(Expression::parse("32 + frame")?),
                            max_width: Deferred::Expr(Expression::parse("width")?),
                            ..Default::default()
                        },
                    }),
                );
                Ok(Self::for_node(graph, target_id)?)
            }
            BenchCase::MeasureNestedReference => {
                let source_id = NodeId::new(7);
                graph.nodes.insert(
                    source_id,
                    NodeKind::Text(Text {
                        id: source_id,
                        params: lumen_engine::node::source::text::TextParamsDelegate {
                            content: Deferred::value("Referenced text measurement.".to_string()),
                            font_family: Deferred::value("Roboto".to_string()),
                            font_size: Deferred::value(24.0),
                            max_width: Deferred::value(700.0),
                            ..Default::default()
                        },
                    }),
                );
                graph.nodes.insert(
                    target_id,
                    NodeKind::Text(Text {
                        id: target_id,
                        params: lumen_engine::node::source::text::TextParamsDelegate {
                            content: Deferred::Expr(Expression::parse("node(7, \"content\")")?),
                            font_family: Deferred::Expr(Expression::parse(
                                "node(7, \"font_family\")",
                            )?),
                            font_size: Deferred::Expr(Expression::parse("text_height(node(7))")?),
                            max_width: Deferred::Expr(Expression::parse("node(7, \"max_width\")")?),
                            ..Default::default()
                        },
                    }),
                );
                Ok(Self::for_node(graph, target_id)?)
            }
            _ => unreachable!("only measurement cases are accepted here"),
        }
    }

    fn for_node(
        graph: Graph,
        node_id: NodeId,
    ) -> Result<Self, lumen_engine::error::ExpressionError> {
        Ok(Self {
            graph: Some(graph),
            width: Expression::parse(&format!("text_width(node({}))", node_id.0))?,
            height: Expression::parse(&format!("text_height(node({}))", node_id.0))?,
        })
    }

    fn run_once(&self) -> anyhow::Result<()> {
        let ctx = ExpressionContext {
            frame: 48.0,
            fps: 24.0,
            width: 1400,
            height: 900,
            duration_frames: 240,
            path: Some("text_benchmark".to_string()),
            graph: self.graph.as_ref(),
            media: None,
        };
        std::hint::black_box(self.width.evaluate(&ctx)?);
        std::hint::black_box(self.height.evaluate(&ctx)?);
        Ok(())
    }
}

fn run_measurement_case(case: BenchCase, iterations: usize) -> anyhow::Result<BenchResult> {
    let fixture = MeasurementFixture::new(case)?;
    let cold_started = Instant::now();
    fixture.run_once()?;
    let cold = cold_started.elapsed();

    let started = Instant::now();
    for _ in 0..iterations {
        fixture.run_once()?;
    }
    Ok(BenchResult {
        cold,
        elapsed: started.elapsed(),
        glyph_count: 2,
    })
}
