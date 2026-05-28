use anyhow::anyhow;

use crate::bench::{
    Bench, fixtures,
    report::{SummaryReport, format_duration, phase_first, phase_mean},
    timing::PhaseTimer,
};

pub struct JsonParseBench;

impl Bench for JsonParseBench {
    fn name() -> &'static str {
        "json_parse"
    }

    fn run() -> anyhow::Result<()> {
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
            let mut setup = PhaseTimer::default();
            let composition = setup.time("json_parse_cold", || {
                lumen_engine::json::parse(fixture.source)
                    .map_err(|error| anyhow!("failed to parse {}: {error}", fixture.name))
            })?;
            setup.time("validate_structure", || {
                composition
                    .validate_structure()
                    .map_err(|errors| anyhow!("validate {} failed: {errors:?}", fixture.name))
            })?;

            let mut loop_timer = PhaseTimer::default();
            for _ in 0..args.iterations {
                loop_timer.time("json_parse", || {
                    lumen_engine::json::parse(fixture.source)
                        .map_err(|error| anyhow!("failed to parse {}: {error}", fixture.name))
                })?;
            }

            setup.print(&format!("json_parse_bench fixture={}", fixture.name));
            loop_timer.print(&format!(
                "json_parse_bench fixture={} iterations={}",
                fixture.name, args.iterations
            ));
            println!(
                "json_parse_bench fixture={} nodes={} duration_frames={} width={} height={}",
                fixture.name,
                composition.graph.nodes.len(),
                composition.timeline.duration_frames,
                composition.render_settings.width,
                composition.render_settings.height,
            );

            let cold = phase_first(&setup, "json_parse_cold").unwrap_or_default();
            let warm = phase_mean(&loop_timer, "json_parse").unwrap_or_default();
            summary.push_row(vec![
                fixture.name.to_string(),
                composition.graph.nodes.len().to_string(),
                format!(
                    "{}×{}",
                    composition.render_settings.width, composition.render_settings.height
                ),
                format_duration(cold),
                format_duration(warm),
                args.iterations.to_string(),
            ]);
        }

        summary.print();
        Ok(())
    }
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
                    .map_err(|_| anyhow!("--iterations must be a positive integer"))?;
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
