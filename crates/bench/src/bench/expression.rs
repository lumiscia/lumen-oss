//! Times expression evaluation on its own, the per-frame cost a render pays for each expression
//! param, and parsing, which runs once per expression. No GPU or media is involved.

use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use anyhow::Context;
use lumen_engine::{
    expr::{Expression, ExpressionContext, FunctionScope},
    graph::Graph,
    node::{
        Deferred, NodeId, NodeKind,
        source::text::{Text, TextParamsDelegate},
    },
};

const CASES: [(&str, &str); 9] = [
    ("globals", "frame * 2 + time + width / height"),
    ("tween", "tween(1200, 540, 0, 18, 'out_expo')"),
    (
        "tween-vec2-sum",
        "vec2(960, 1200) + tween(vec2(0, 0), vec2(0, -660), 0, 18, 'out_expo') \
         + tween(vec2(0, 0), vec2(-1360, 0), 90, 14, 'in_back')",
    ),
    (
        "repeated-subtree",
        "min(1, max(0, (time - 0.5) / 0.5)) * min(1, max(0, (time - 0.5) / 0.5))",
    ),
    ("spring", "spring((frame - 12) / fps, 170, 26)"),
    (
        "conditional",
        "if(frame > 48 && frame % 2 == 0, 10 / (frame + 1), 3)",
    ),
    ("function-call", "settle(frame, 2)"),
    (
        "property-read",
        "node(1, 'font_size') * 2 + node(1, 'max_width')",
    ),
    ("measure-literal", "text_width('Hello', 32, 300, 'Roboto')"),
];

const FUNCTIONS: [(&str, &str); 2] = [
    ("rise(start)", "out_expo(progress(start, start + 16)) * 60"),
    ("settle(start, scale)", "rise(start) * scale + rise(start)"),
];

pub fn run() -> anyhow::Result<()> {
    let mut iterations = 200_000_u32;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--iterations" => {
                iterations = args
                    .next()
                    .context("--iterations requires a value")?
                    .parse()?;
            }
            "-h" | "--help" => {
                println!("lumen-bench-expression [--iterations N]");
                return Ok(());
            }
            other => anyhow::bail!("unknown argument `{other}`"),
        }
    }

    let functions: BTreeMap<_, _> = FUNCTIONS
        .iter()
        .map(|(signature, body)| (signature.to_string(), body.to_string()))
        .collect();
    let _scope = FunctionScope::enter(&functions)?;
    let graph = graph()?;

    println!("{:<18} {:>12} {:>12}", "case", "parse_ns", "evaluate_ns");
    for (name, source) in CASES {
        // Each repetition is a fresh parse, so the fastest of them is the least disturbed.
        let parse = fastest(5, || {
            let start = Instant::now();
            for _ in 0..1_000 {
                std::hint::black_box(Expression::parse(source)?);
            }
            Ok(start.elapsed() / 1_000)
        })?;
        let expression = Expression::parse(source)?;
        let evaluate = fastest(5, || {
            let start = Instant::now();
            for index in 0..iterations {
                let ctx = ExpressionContext {
                    frame: f64::from(index % 120),
                    fps: 30.0,
                    width: 1920,
                    height: 1080,
                    graph: Some(&graph),
                    ..Default::default()
                };
                std::hint::black_box(expression.evaluate(&ctx)?);
            }
            Ok(start.elapsed() / iterations)
        })?;
        println!(
            "{name:<18} {:>12} {:>12}",
            parse.as_nanos(),
            evaluate.as_nanos()
        );
    }
    Ok(())
}

fn fastest(
    repeats: u32,
    mut measure: impl FnMut() -> anyhow::Result<Duration>,
) -> anyhow::Result<Duration> {
    let mut best = Duration::MAX;
    for _ in 0..repeats {
        best = best.min(measure()?);
    }
    Ok(best)
}

/// A text node whose params `node(1, ..)` reads, one of them through an expression.
fn graph() -> anyhow::Result<Graph> {
    let id = NodeId::new(1);
    let mut graph = Graph::new();
    graph.nodes.insert(
        id,
        NodeKind::Text(Text {
            id,
            params: TextParamsDelegate {
                font_size: Deferred::Expr(Expression::parse("24 + frame")?),
                max_width: Deferred::value(1400.0),
                ..Default::default()
            },
        }),
    );
    Ok(graph)
}
