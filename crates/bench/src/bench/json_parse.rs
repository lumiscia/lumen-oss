use std::time::{Duration, Instant};

use anyhow::anyhow;

use crate::bench::{
    fixtures,
    report::{SummaryReport, format_duration},
};

pub fn run() -> anyhow::Result<()> {
    let args = parse_args()?;
    let fixtures = selected_fixtures(&args.fixture)?;
    let mut summary = SummaryReport::new(
        "JSON parse benchmark summary",
        [
            "fixture",
            "nodes",
            "resolution",
            "cold",
            "warm (mean)",
            "iterations",
        ],
    );

    for fixture in fixtures {
        let parse = || {
            lumen_engine::json::parse(fixture.source)
                .map_err(|error| anyhow!("failed to parse {}: {error}", fixture.name))
        };

        let started = Instant::now();
        let composition = parse()?;
        let cold = started.elapsed();

        let mut warm_total = Duration::ZERO;
        for _ in 0..args.iterations {
            let started = Instant::now();
            parse()?;
            warm_total += started.elapsed();
        }
        let warm = warm_total.div_f64(args.iterations as f64);

        let nodes = composition.graph().nodes.len();
        let (width, height) = (
            composition.render_settings().width,
            composition.render_settings().height,
        );
        println!(
            "json_parse_bench fixture={} nodes={nodes} width={width} height={height} iterations={} cold_us={:.2} warm_mean_us={:.2}",
            fixture.name,
            args.iterations,
            cold.as_secs_f64() * 1_000_000.0,
            warm.as_secs_f64() * 1_000_000.0,
        );
        summary.push_row(vec![
            fixture.name.to_string(),
            nodes.to_string(),
            format!("{width}×{height}"),
            format_duration(cold),
            format_duration(warm),
            args.iterations.to_string(),
        ]);
    }

    summary.print();
    Ok(())
}

struct Args {
    fixture: String,
    iterations: usize,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut fixture = "all".to_string();
    let mut iterations = 1;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--fixture" => {
                fixture = args
                    .next()
                    .ok_or_else(|| anyhow!("--fixture requires a value"))?;
            }
            "--iterations" => {
                iterations = args
                    .next()
                    .ok_or_else(|| anyhow!("--iterations requires a value"))?
                    .parse::<usize>()
                    .ok()
                    .filter(|&count| count > 0)
                    .ok_or_else(|| anyhow!("--iterations must be a positive integer"))?;
            }
            "--list" => {
                let names: Vec<_> = fixtures::JSON_FIXTURES
                    .iter()
                    .map(|fixture| fixture.name)
                    .collect();
                println!("fixtures: all, {}", names.join(", "));
                std::process::exit(0);
            }
            "--help" | "-h" => {
                println!("usage: lumen-bench-json-parse [--fixture all|NAME] [--iterations N]");
                std::process::exit(0);
            }
            _ => return Err(anyhow!("unknown argument `{arg}`")),
        }
    }
    Ok(Args {
        fixture,
        iterations,
    })
}

fn selected_fixtures(name: &str) -> anyhow::Result<Vec<&'static fixtures::JsonFixture>> {
    if name == "all" {
        return Ok(fixtures::JSON_FIXTURES.iter().collect());
    }
    fixtures::fixture(name)
        .map(|fixture| vec![fixture])
        .ok_or_else(|| anyhow!("unknown fixture `{name}`"))
}
