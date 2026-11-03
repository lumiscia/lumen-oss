mod compositions;
mod modes;
mod profile;
mod readback;

use anyhow::anyhow;
use std::path::PathBuf;

use crate::bench::{
    CompositionFixture,
    report::{SummaryReport, format_duration},
    timing::fps,
};
use modes::{Mode, Workload, output_path, run_mode};
use profile::PhaseTimes;

struct Args {
    composition: String,
    modes: Vec<Mode>,
    frames: Option<u32>,
    save: Option<PathBuf>,
    full_render: bool,
    profile: bool,
}

pub fn run() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run_async())
}

async fn run_async() -> anyhow::Result<()> {
    let args = parse_args()?;
    let fixtures = selected_fixtures(&args.composition)?;
    let modes = if args.modes.is_empty() {
        Mode::ALL.to_vec()
    } else {
        args.modes.clone()
    };
    let encoding_runs = fixtures.len()
        * modes
            .iter()
            .filter(|mode| mode.encodes() && !mode.requires_unsupported_platform())
            .count();
    let mut summary = SummaryReport::new(
        "Composition benchmark summary",
        [
            "composition",
            "mode",
            "frames",
            "full render",
            "setup",
            "first frame",
            "steady",
            "steady fps",
            "status",
        ],
    );

    for fixture in fixtures {
        let composition = fixture.build();
        let media = fixture.media_store();
        let frames = args
            .frames
            .unwrap_or_else(|| fixture.default_frames(&composition))
            .min(composition.timeline().duration_frames);
        let full_render = args.full_render || fixture.full_render();

        for &mode in &modes {
            let name = fixture.name();
            if mode.requires_unsupported_platform() {
                println!(
                    "composition_bench composition={name} mode={} skipped=unsupported_platform",
                    mode.name()
                );
                summary.push_row(vec![
                    name.to_string(),
                    mode.name().to_string(),
                    frames.to_string(),
                    full_render.to_string(),
                    "-".to_string(),
                    "-".to_string(),
                    "-".to_string(),
                    "-".to_string(),
                    "skipped (platform)".to_string(),
                ]);
                continue;
            }
            let output = mode
                .encodes()
                .then(|| output_path(args.save.as_deref(), name, mode, encoding_runs));
            let workload = Workload {
                composition: &composition,
                media: &media,
                frames,
                full_render,
            };
            let mut profile = args.profile.then(PhaseTimes::default);
            let times = run_mode(&workload, mode, output.as_deref(), profile.as_mut()).await?;
            if let Some(profile) = &profile {
                profile.print(mode.name(), frames);
            }
            if args.save.is_none()
                && let Some(path) = output.as_deref()
            {
                let _ = std::fs::remove_file(path);
            }
            let steady_frames = times.steady_frames(frames);
            let steady_fps = (steady_frames > 0).then(|| fps(steady_frames, times.steady));
            let first_frame_ms = times.first_frame.map_or_else(
                || "-".to_string(),
                |d| format!("{:.2}", d.as_secs_f64() * 1000.0),
            );
            println!(
                "composition_bench composition={name} mode={} frames={frames} full_render={full_render} setup_ms={:.2} first_frame_ms={} steady_ms={:.2} elapsed_ms={} fps={} output={}",
                mode.name(),
                times.setup.as_secs_f64() * 1000.0,
                first_frame_ms,
                times.steady.as_secs_f64() * 1000.0,
                times.loop_total().as_millis(),
                steady_fps.map_or_else(|| "-".to_string(), |fps| format!("{fps:.2}")),
                output
                    .as_deref()
                    .filter(|_| args.save.is_some())
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "-".to_string())
            );
            summary.push_row(vec![
                name.to_string(),
                mode.name().to_string(),
                frames.to_string(),
                full_render.to_string(),
                format_duration(times.setup),
                times
                    .first_frame
                    .map_or_else(|| "-".to_string(), format_duration),
                format_duration(times.steady),
                steady_fps.map_or_else(|| "-".to_string(), |fps| format!("{fps:.1}")),
                "ok".to_string(),
            ]);
        }
    }

    summary.print();
    println!(
        "setup creates the renderer, compiles the plan and opens any encoder. render-only and readback complete every frame: first frame includes one-off uploads and static work, and steady fps covers the remaining frames. Encoders buffer frames, so encoding modes have no first-frame time and steady covers every frame through the encoder's finish."
    );
    Ok(())
}

fn parse_args() -> anyhow::Result<Args> {
    let mut composition = "all".to_string();
    let mut modes = Vec::new();
    let mut frames = None;
    let mut save = None;
    let mut full_render = false;
    let mut profile = false;
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
                        .ok()
                        .filter(|&count| count > 0)
                        .ok_or_else(|| anyhow!("--frames must be a positive integer"))?,
                );
            }
            "--save" => {
                save = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("--save requires a path"))?,
                ));
            }
            "--full-render" => full_render = true,
            "--profile" => profile = true,
            "--list" => {
                let names: Vec<_> = compositions::all()
                    .iter()
                    .map(|fixture| fixture.name())
                    .collect();
                let modes: Vec<_> = Mode::ALL.iter().map(|mode| mode.cli_name()).collect();
                println!("compositions: all, {}", names.join(", "));
                println!("modes: all, {}", modes.join(", "));
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
        full_render,
        profile,
    })
}

fn print_help() {
    let names: Vec<_> = compositions::all()
        .iter()
        .map(|fixture| fixture.name())
        .collect();
    println!(
        "usage: lumen-bench-composition [--composition all|{}] [--mode MODE]... [--frames N] [--save PATH] [--full-render] [--profile]\n\nRun --list for the modes. --save names the file when exactly one encoding run is selected; otherwise it is a prefix and each file gets the composition and mode appended.",
        names.join("|")
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
