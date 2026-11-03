use std::time::{Duration, Instant};

use anyhow::{Context, anyhow};
use lumen_engine::node::source::text_content::TextContentDelegate;
use lumen_engine::{
    composition::{CompositionBuilder, RenderSettings, TimelineSettings},
    error::MediaError,
    expr::{Expression, ExpressionContext},
    gpu::GpuCompositionRenderer,
    graph::Graph,
    media::{FontResolver, MediaStore},
    node::{
        Deferred, NodeId, NodeKind,
        media_output::MediaOutput,
        source::text::{Text, TextParamsDelegate, TextRenderMode},
    },
};

use crate::bench::report::{SummaryReport, format_duration};

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

    fn is_measurement(self) -> bool {
        matches!(
            self,
            Self::MeasureLiteral | Self::MeasureExpression | Self::MeasureNestedReference
        )
    }

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

pub fn run() -> anyhow::Result<()> {
    let mut iterations = 100usize;
    let mut repeats = None;
    let mut selection = "all".to_string();
    let mut fonts = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--iterations" => {
                iterations = positive_count(args.next(), "--iterations")?;
            }
            "--text-repeats" => {
                repeats = Some(positive_count(args.next(), "--text-repeats")?);
            }
            "--case" => selection = args.next().context("--case requires a value")?,
            "--font" => {
                let path = args.next().context("--font requires a path")?;
                fonts.push(std::fs::read(&path).with_context(|| format!("reading {path}"))?);
            }
            "--help" | "-h" | "--list" => {
                println!(
                    "lumen-bench-text [--case all|layout|raster|msdf|emoji|measure-literal|measure-expression|measure-nested-reference] [--iterations N] [--text-repeats N] [--font PATH]...\n\nOnly the bundled Roboto is loaded by default; pass --font with fonts covering emoji, Arabic and Hebrew to measure fallback. The emoji case is skipped when no loaded font covers it. The measure-* cases run a fixed workload, so --font and --text-repeats are rejected with them and ignored by `--case all`."
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
    if BenchCase::ALL
        .iter()
        .any(|case| case.name() == selection && case.is_measurement())
        && (repeats.is_some() || !fonts.is_empty())
    {
        return Err(anyhow!(
            "--font and --text-repeats do not apply to the fixed `{selection}` workload"
        ));
    }
    let repeats = repeats.unwrap_or(1);
    let media = BenchMedia { fonts };
    let mut summary = SummaryReport::new(
        "Text benchmark summary",
        [
            "case",
            "workload",
            "iterations",
            "cold",
            "total",
            "mean/iter",
        ],
    );
    for case in BenchCase::ALL {
        if selection != "all" && selection != case.name() {
            continue;
        }
        match run_case(case, iterations, repeats, &media)? {
            CaseOutcome::Ran(result) => summary.push_row(vec![
                case.name().to_string(),
                result.workload,
                iterations.to_string(),
                format_duration(result.cold),
                format_duration(result.elapsed),
                format_duration(result.elapsed / iterations as u32),
            ]),
            CaseOutcome::Skipped(reason) => summary.push_row(vec![
                case.name().to_string(),
                format!("skipped: {reason}"),
                "-".to_string(),
                "-".to_string(),
                "-".to_string(),
                "-".to_string(),
            ]),
        }
    }
    summary.print();
    Ok(())
}

fn positive_count(value: Option<String>, option: &str) -> anyhow::Result<usize> {
    value
        .with_context(|| format!("{option} requires a value"))?
        .parse::<usize>()
        .ok()
        .filter(|&count| count > 0)
        .ok_or_else(|| anyhow!("{option} must be a positive integer"))
}

struct BenchResult {
    cold: Duration,
    elapsed: Duration,
    workload: String,
}

enum CaseOutcome {
    Ran(BenchResult),
    Skipped(String),
}

/// Media with no images or video, whose `--font` files load for any font family.
#[derive(Debug)]
struct BenchMedia {
    fonts: Vec<Vec<u8>>,
}

impl MediaStore for BenchMedia {
    fn font_resolver_id(&self, _: &str) -> Option<String> {
        (!self.fonts.is_empty()).then(|| "bench".to_string())
    }
    fn get_font_resolver(&self, _: &str) -> Option<Box<dyn FontResolver>> {
        (!self.fonts.is_empty())
            .then(|| Box::new(BenchFonts(self.fonts.clone())) as Box<dyn FontResolver>)
    }
}

struct BenchFonts(Vec<Vec<u8>>);

impl FontResolver for BenchFonts {
    fn id(&self) -> &str {
        "bench"
    }
    fn data(&self) -> Result<Vec<Vec<u8>>, MediaError> {
        Ok(self.0.clone())
    }
}

fn run_case(
    case: BenchCase,
    iterations: usize,
    repeats: usize,
    media: &BenchMedia,
) -> anyhow::Result<CaseOutcome> {
    if case.is_measurement() {
        return run_measurement_case(case, iterations).map(CaseOutcome::Ran);
    }
    let content = if case == BenchCase::Emoji { "🍋✨🚀🎬 " } else {
        "Lumen: AV fi — مرحبا بالعالم שלום 123 — a\u{301} e\u{308} n\u{303}. Wrapping varied text across lines. "
    }.repeat(repeats);
    let mut request = lumen_text::TextLayoutRequest::new(content.clone());
    request.max_width = Some(1300.0);
    let mut system = lumen_text::TextSystem::new();
    for font in &media.fonts {
        system.load_font_data(font.clone());
    }
    let layout = system.shape(&request).place([0.0; 2]);
    // Glyph id 0 is the font's missing-glyph box.
    if case == BenchCase::Emoji && layout.glyphs.iter().any(|glyph| glyph.key.0.glyph_id == 0) {
        return Ok(CaseOutcome::Skipped(
            "no loaded font covers the emoji (pass --font)".to_string(),
        ));
    }
    let glyph_count = layout.glyphs.len();
    let mut graph = Graph::new();
    graph.nodes.insert(
        NodeId::new(1),
        NodeKind::Text(Text {
            id: NodeId::new(1),
            params: TextParamsDelegate {
                content: TextContentDelegate::Text(Deferred::value(content)),
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
            source: Some(NodeId::new(1)),
        }),
    );
    let composition = CompositionBuilder::new(
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
    )
    .build()
    .expect("valid benchmark composition");
    let mut renderer = if case == BenchCase::Layout {
        None
    } else {
        Some(pollster::block_on(GpuCompositionRenderer::new())?)
    };
    let mut run = |frame| -> anyhow::Result<()> {
        if let Some(renderer) = &mut renderer {
            let (_, submission) = renderer.render_frame_submitted(&composition, frame, media)?;
            renderer
                .gpu_renderer()
                .device
                .poll(lumen_gpu::wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: None,
                })?;
        } else {
            request.spans[0].font_size = 24.0 + (frame % 96) as f32;
            std::hint::black_box(system.shape(&request).place([0.0; 2]));
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
    Ok(CaseOutcome::Ran(BenchResult {
        cold,
        elapsed: start.elapsed(),
        workload: format!("{glyph_count} glyphs"),
    }))
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
                            content: TextContentDelegate::Text(Deferred::Expr(Expression::parse(
                                "\"Hello from Lumen measurement.\"",
                            )?)),
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
                            content: TextContentDelegate::Text(Deferred::value(
                                "Referenced text measurement.".to_string(),
                            )),
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
                            content: TextContentDelegate::Text(Deferred::Expr(Expression::parse(
                                "node(7, \"content\")",
                            )?)),
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
            graph: self.graph.as_ref(),
            media: None,
            glyph: None,
            props: None,
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
        workload: "2 measurements/iteration".to_string(),
    })
}
