use std::{env, fs, path::PathBuf};

use anyhow::{Context, Result, anyhow};
use lumen::Project;
use lumen_server::executor::{RenderExecutionOptions, execute_render};

#[derive(Debug, Default)]
struct CliArgs {
    project: Option<PathBuf>,
    output: Option<PathBuf>,
    media_root: Option<PathBuf>,
    encoder: Option<String>,
}

fn parse_args() -> Result<CliArgs> {
    let mut args = env::args().skip(1);
    let mut parsed = CliArgs::default();

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--project" => {
                let Some(value) = args.next() else {
                    return Err(anyhow!("missing value for --project"));
                };
                parsed.project = Some(PathBuf::from(value));
            }
            "--output" => {
                let Some(value) = args.next() else {
                    return Err(anyhow!("missing value for --output"));
                };
                parsed.output = Some(PathBuf::from(value));
            }
            "--media-root" => {
                let Some(value) = args.next() else {
                    return Err(anyhow!("missing value for --media-root"));
                };
                parsed.media_root = Some(PathBuf::from(value));
            }
            "--encoder" => {
                let Some(value) = args.next() else {
                    return Err(anyhow!("missing value for --encoder"));
                };
                parsed.encoder = Some(value);
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            unknown => {
                return Err(anyhow!("unknown argument: {unknown}"));
            }
        }
    }

    if parsed.project.is_none() || parsed.output.is_none() {
        return Err(anyhow!("--project and --output are required"));
    }

    Ok(parsed)
}

fn print_usage() {
    eprintln!(
        "usage: lumen-local --project <path> --output <path> [--media-root <path>] [--encoder <name>]"
    );
}

fn load_project(path: &PathBuf) -> Result<Project> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read project file {}", path.display()))?;
    let project = serde_json::from_str::<Project>(&raw)
        .with_context(|| format!("failed to parse project JSON {}", path.display()))?;
    Ok(project)
}

fn main() {
    if let Err(err) = run() {
        eprintln!("lumen-local failed: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = parse_args().inspect_err(|_| {
        print_usage();
    })?;

    let project_path = args.project.ok_or_else(|| anyhow!("missing --project"))?;
    let output_path = args.output.ok_or_else(|| anyhow!("missing --output"))?;

    let project = load_project(&project_path)?;

    let options = RenderExecutionOptions {
        media_root: args.media_root,
        video_encoder: args.encoder,
        encode_queue: None,
        max_decoded_source_frames: None,
        stream_cache_frames: None,
    };

    let mut progress = |event: lumen_server::executor::RenderExecutionProgress| {
        if event.total_frames == 0 || event.frame == event.total_frames || event.frame % 30 == 0 {
            println!(
                "progress stage={} frame={}/{} ratio={:.3}",
                event.stage, event.frame, event.total_frames, event.ratio
            );
        }
    };

    let rendered = execute_render(&project, &options, &mut progress)
        .map_err(|err| anyhow!("{} (retryable={})", err, err.retryable))?;

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create output directory {}", parent.display()))?;
    }

    fs::write(&output_path, rendered.bytes)
        .with_context(|| format!("failed to write output file {}", output_path.display()))?;

    println!(
        "render complete output={} frames={} compile_ms={} render_ms={}",
        output_path.display(),
        rendered.metrics.total_frames,
        rendered.metrics.compile_ms,
        rendered.metrics.render_ms
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use lumen::compile_project;
    use serde_json::json;

    #[test]
    fn url_source_project_json_deserializes_and_compiles() {
        let raw = json!({
            "version": "1",
            "canvas": {
                "width": 320,
                "height": 180,
                "background": [0, 0, 0, 255]
            },
            "timeline": {
                "fps": { "num": 30, "den": 1 },
                "duration_frames": 2
            },
            "sources": [{
                "id": "image_0",
                "media": "image",
                "kind": {
                    "type": "url",
                    "url": "https://cdn.example.com/media/background.png"
                }
            }],
            "layers": [{
                "id": "layer_image",
                "items": [{
                    "type": "clip",
                    "id": "clip_image_1",
                    "start_frame": 0,
                    "duration_frames": 2,
                    "content": {
                        "type": "image",
                        "source": "image_0"
                    }
                }]
            }],
            "audio": { "tracks": [] }
        })
        .to_string();

        let project: super::Project =
            serde_json::from_str(raw.as_str()).expect("project JSON should deserialize");
        let timeline = compile_project(&project).expect("project should compile");
        let source = timeline.source(0).expect("compiled source");
        assert_eq!(source.id, "image_0");
        assert_eq!(source.path, "https://cdn.example.com/media/background.png");
    }
}
