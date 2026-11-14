use std::{fs, path::Path, time::Duration};

use anyhow::{Context, Result};
use clap::Parser;
use lumen_engine::render::{
    CancelToken, CheckReport, ImageRender, LocalMediaStore, Mp4Render, RenderOutput,
    RenderProgress, RenderReport,
};
use lumen_engine::{
    composition::Composition,
    diagnostic::{Diagnostic, Severity},
    media::{AssetKind, MediaStore},
};
use lumen_ffmpeg::VideoEncoderChoice;
use tracing_subscriber::EnvFilter;

mod args;
mod output_plan;
mod partial;
mod preview;

use args::{CheckArgs, Cli, Command, Format, RenderArgs, SourceArgs};
use output_plan::{Files, OutputPlan};
use partial::render_to_partial;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("lumen_engine=warn")),
        )
        .with_writer(std::io::stderr)
        .init();
    lumen_ffmpeg::raise_open_file_limit();
    let result = match Cli::parse().command {
        Command::Check(args) => check(&args),
        Command::Render(args) => run(&args).map(|()| true),
        Command::Preview(args) => preview::preview(&args).map(|()| true),
    };
    match result {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(error) => {
            eprintln!("{}", failure_line(&error));
            std::process::exit(1);
        }
    }
}

/// The line a failed run prints: the engine's diagnostic code, when the engine failed, and the
/// whole chain of causes.
fn failure_line(error: &anyhow::Error) -> String {
    let engine = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<lumen_engine::error::LumenError>());
    match engine {
        Some(engine) => format!("lumen failed ({}): {error:#}", engine.code()),
        None => format!("lumen failed: {error:#}"),
    }
}

/// Prints what is wrong with the composition, and whether it renders.
fn check(args: &CheckArgs) -> Result<bool> {
    let raw = args.source.read_composition()?;
    let media_store = args.source.media_store()?;
    let known_font = |family: &str| media_store.resolve(family, AssetKind::Font).is_ok();
    let report = CheckReport::new(lumen_engine::json::check(
        &raw,
        Some(&known_font),
        Some(media_store.assets()),
    ));
    let diagnostics = &report.diagnostics;
    match args.format {
        Format::Json => println!("{}", serde_json::to_string(&report)?),
        Format::Text => {
            for diagnostic in diagnostics {
                println!("{}", diagnostic_line(diagnostic));
            }
            let errors = diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.is_error())
                .count();
            println!(
                "{}: {errors} errors, {} warnings",
                args.source.composition_name(),
                diagnostics.len() - errors
            );
        }
    }
    Ok(report.ok)
}

fn diagnostic_line(diagnostic: &Diagnostic) -> String {
    let severity = match diagnostic.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    };
    let path = if diagnostic.path.is_empty() {
        "/"
    } else {
        &diagnostic.path
    };
    format!(
        "{severity} {} at {path}: {}",
        diagnostic.code, diagnostic.message
    )
}

fn print_warnings(diagnostics: &[Diagnostic]) {
    for diagnostic in diagnostics {
        eprintln!("{}", diagnostic_line(diagnostic));
    }
}

fn run(args: &RenderArgs) -> Result<()> {
    let plan = args.plan()?;
    let composition = load_composition(&args.source)?;
    let render_plan = args.render_plan(&plan.output, &composition)?;
    let media_store = args.source.media_store()?.explaining_missing_sources();

    if let Some(choice) = render_plan.encoder {
        return render_to_partial(&args.output, |partial| {
            render_mp4(
                &composition,
                media_store,
                partial,
                choice,
                &plan.output,
                args.max_text_glyphs,
            )
        });
    }
    render_images(
        &composition,
        media_store,
        &args.output,
        &plan,
        args.max_text_glyphs,
    )
}

fn load_composition(source: &SourceArgs) -> Result<Composition> {
    let raw = source.read_composition()?;

    lumen_engine::json::parse(&raw)
        .with_context(|| format!("failed to parse composition {}", source.composition_name()))
}

/// Writes each frame of the render as a file: the output path for a still, and for a sequence the
/// output's name with the frame's number in the pattern.
fn render_images(
    composition: &Composition,
    media_store: LocalMediaStore,
    output: &Path,
    plan: &OutputPlan,
    max_text_glyphs: u32,
) -> Result<()> {
    let mut render = ImageRender::open(composition, media_store, &plan.output, CancelToken::new())?;
    render.set_max_text_glyphs(max_text_glyphs as usize);
    let last_frame = render.frames().end - 1;
    let directory = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(directory) = directory {
        fs::create_dir_all(directory)
            .with_context(|| format!("failed to create output dir {}", directory.display()))?;
    }
    let report = render.run(
        &mut |image| {
            let path = match &plan.files {
                Files::One => output.to_path_buf(),
                Files::Sequence(pattern) => {
                    output.with_file_name(pattern.file_name(image.frame, last_frame))
                }
            };
            fs::write(&path, &image.bytes)
                .with_context(|| format!("failed to write {}", path.display()))
        },
        &mut print_progress,
    )?;
    print_warnings(&report.diagnostics);
    match plan.files {
        Files::One => println!(
            "render complete output={} frame={}",
            output.display(),
            last_frame
        ),
        Files::Sequence(_) => println!(
            "render complete output={} frames={}",
            output.display(),
            report.frames
        ),
    }
    Ok(())
}

fn print_progress(progress: RenderProgress) {
    if progress.frame == 1
        || progress.frame == progress.total_frames
        || (progress.frame - 1).is_multiple_of(60)
    {
        println!(
            "progress frame={}/{}",
            progress.frame, progress.total_frames
        );
    }
}

fn render_mp4(
    composition: &Composition,
    media_store: LocalMediaStore,
    output: &Path,
    choice: VideoEncoderChoice,
    render_output: &RenderOutput,
    max_text_glyphs: u32,
) -> Result<()> {
    let mut render = Mp4Render::open(
        composition,
        media_store,
        choice,
        output,
        render_output,
        CancelToken::new(),
    )?;
    render.set_max_text_glyphs(max_text_glyphs as usize);
    println!(
        "video_encoder name={} path={}",
        render.encoder(),
        render.path()
    );
    let report = render.run(&mut print_progress)?;
    print_warnings(&report.diagnostics);
    println!("{}", render_summary(&report));
    Ok(())
}

/// The summary line, with passes per frame: those skipped because their output was kept from an
/// earlier frame, and those recorded.
fn render_summary(report: &RenderReport) -> String {
    let millis = |duration: Duration| duration.as_millis();
    let timing = &report.timing;
    let frames = f64::from(report.frames.max(1));
    format!(
        "render complete frames={} audio_mix_ms={} bind_ms={} upload_ms={} render_ms={} flush_ms={} gpu_ms={} readback_ms={} encode_send_ms={} encode_finish_ms={} total_ms={} held_bytes={} skipped_passes={:.1} submitted_passes={:.1}",
        report.frames,
        millis(timing.audio_mix),
        millis(timing.bind),
        millis(timing.upload),
        millis(timing.render),
        millis(timing.flush),
        millis(timing.gpu),
        millis(timing.readback),
        millis(timing.encode_send),
        millis(timing.encode_finish),
        millis(timing.total),
        report.held_bytes,
        report.skipped_passes as f64 / frames,
        report.submitted_passes as f64 / frames,
    )
}

#[cfg(test)]
mod tests {
    use super::failure_line;

    #[test]
    fn a_failed_run_prints_the_engine_code_of_the_failure() {
        use lumen_engine::{
            error::{LumenError, MediaFailure},
            media::AssetKind,
        };

        let media: LumenError = MediaFailure::not_found("logo.png", "no such file")
            .wanting(&[AssetKind::Image])
            .into();
        let error = anyhow::Error::new(media).context("failed to render");
        assert_eq!(
            failure_line(&error),
            "lumen failed (media.not_found): failed to render: image `logo.png` not found: no such file"
        );
        assert_eq!(
            failure_line(&anyhow::anyhow!("disk full")),
            "lumen failed: disk full"
        );
    }
}
