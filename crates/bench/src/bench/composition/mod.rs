mod compositions;
mod modes;
mod profile;
mod readback;

pub use modes::{
    Mode, output_path, requires_unsupported_platform, run_mode, selected_modes, temp_path,
};

use anyhow::anyhow;
use std::path::PathBuf;

use crate::bench::{
    Bench, CompositionFixture,
    report::{SummaryReport, format_duration, format_fps},
    timing::PhaseTimer,
    timing::fps,
};

pub struct CompositionBench;

impl Bench for CompositionBench {
    fn name() -> &'static str {
        "composition"
    }

    fn run() -> anyhow::Result<()> {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(run_async())
    }
}

struct Args {
    composition: String,
    modes: Vec<Mode>,
    frames: Option<u32>,
    save: Option<PathBuf>,
}

async fn run_async() -> anyhow::Result<()> {
    let args = parse_args()?;
    let fixtures = selected_fixtures(&args.composition)?;
    let mut summary = SummaryReport::new(
        "Composition benchmark summary",
        ["composition", "mode", "frames", "elapsed", "fps", "status"],
    );

    for fixture in fixtures {
        let mut setup = PhaseTimer::default();
        let composition = setup.time("build_composition", || fixture.build());
        let frames = args
            .frames
            .unwrap_or_else(|| fixture.default_frames(&composition))
            .min(composition.timeline.duration_frames);

        for mode in selected_modes(&args.modes) {
            if requires_unsupported_platform(mode) {
                println!(
                    "composition_bench composition={} mode={} skipped=unsupported_platform",
                    fixture.name(),
                    mode.name()
                );
                summary.push_row(vec![
                    fixture.name().to_string(),
                    mode.name().to_string(),
                    frames.to_string(),
                    "-".to_string(),
                    "-".to_string(),
                    "skipped (platform)".to_string(),
                ]);
                continue;
            }
            let output = output_path(args.save.as_deref(), fixture.name(), mode, args.modes.len())?;
            let mut run = PhaseTimer::default();
            let elapsed = run
                .time_async("mode_total", async {
                    run_mode(&composition, frames, mode, output.as_deref()).await
                })
                .await?;
            setup.print(&format!("composition_bench composition={}", fixture.name()));
            run.print(&format!(
                "composition_bench composition={} mode={}",
                fixture.name(),
                mode.name()
            ));
            if args.save.is_none()
                && let Some(path) = output.as_deref()
            {
                let _ = std::fs::remove_file(path);
            }
            println!(
                "composition_bench composition={} mode={} frames={} elapsed_ms={} fps={:.2} output={}",
                fixture.name(),
                mode.name(),
                frames,
                elapsed.as_millis(),
                fps(frames, elapsed),
                output
                    .as_deref()
                    .filter(|_| args.save.is_some())
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "-".to_string())
            );
            summary.push_row(vec![
                fixture.name().to_string(),
                mode.name().to_string(),
                frames.to_string(),
                format_duration(elapsed),
                format_fps(frames, elapsed),
                "ok".to_string(),
            ]);
        }
    }

    summary.print();
    Ok(())
}

fn parse_args() -> anyhow::Result<Args> {
    let mut composition = "all".to_string();
    let mut modes = Vec::new();
    let mut frames = None;
    let mut save = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--composition" => {
                composition = args
                    .next()
                    .ok_or_else(|| anyhow!("--composition requires a value"))?;
            }
            "--mode" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow!("--mode requires a value"))?;
                if value == "all" {
                    modes.clear();
                } else {
                    modes.push(Mode::parse(&value)?);
                }
            }
            "--frames" => {
                frames = Some(
                    args.next()
                        .ok_or_else(|| anyhow!("--frames requires a value"))?
                        .parse::<u32>()
                        .map_err(|_| anyhow!("--frames must be a positive integer"))?,
                );
            }
            "--save" => {
                save = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("--save requires a path"))?,
                ));
            }
            "--list" => {
                let names: Vec<_> = compositions::all()
                    .iter()
                    .map(|fixture| fixture.name())
                    .collect();
                println!("compositions: all, {}", names.join(", "));
                println!(
                    "modes: all, render-only, render-profile, readback, readback-profile, cpu-encode, cpu-encode-profile, metal-videotoolbox, metal-videotoolbox-profile, vk-cuda-export, vk-cuda-nvenc"
                );
                std::process::exit(0);
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            _ => return Err(anyhow!("unknown argument `{arg}`")),
        }
    }
    Ok(Args {
        composition,
        modes,
        frames,
        save,
    })
}

fn print_help() {
    println!(
        "usage: lumen-bench-composition [--composition all|simple_pipeline|vector_showcase|animated_showcase|antialiasing_stress_aa|antialiasing_stress_noaa] [--mode MODE] [--frames N] [--save PATH]"
    );
}

fn selected_fixtures(name: &str) -> anyhow::Result<Vec<Box<dyn CompositionFixture>>> {
    if name == "all" {
        return Ok(compositions::all());
    }
    compositions::by_name(name)
        .map(|fixture| vec![fixture])
        .ok_or_else(|| anyhow!("unknown composition `{name}`"))
}
