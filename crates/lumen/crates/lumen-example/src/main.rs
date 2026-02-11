use std::{path::PathBuf, sync::Arc};

use anyhow::Context;
use lumen::{
    Clip, ClipContent, ColorRgba, FitMode, Layer, LoopMode, Project, Shape, ShapeClip, Source,
    SourceKind, SourceMediaType, SourcePipeline, TextAlign, TextClip, Timeline, Transform,
    TrimRange, VideoClip, compile_project, time::Rational,
};
use lumen_server::video::FfmpegRenderBackend;

fn main() -> anyhow::Result<()> {
    let output_dir = PathBuf::from("tmp/lumen-example");
    std::fs::create_dir_all(&output_dir).context("failed to create example output directory")?;

    let output_path = output_dir.join("short_form_vello.mp4");

    let project = short_form_project()?;
    let timeline =
        Arc::new(compile_project(&project).context("failed to compile short-form project")?);

    let backend = FfmpegRenderBackend::new(timeline.clone());
    let bytes = backend
        .render_to_mp4(&mut |frame, total| {
            if frame % 30 == 0 || frame == total {
                println!("rendered frame {frame}/{total}");
            }
        })
        .context("failed to render short-form video")?;

    std::fs::write(&output_path, bytes)
        .with_context(|| format!("failed to write {}", output_path.display()))?;

    println!("Wrote short-form example to {}", output_path.display());
    Ok(())
}

fn short_form_project() -> anyhow::Result<Project> {
    let fps = Rational::new(30, 1)?;

    Ok(Project {
        canvas: lumen::Canvas {
            width: 720,
            height: 1280,
            background: ColorRgba(12, 15, 22, 255),
        },
        timeline: Timeline {
            fps,
            total_frames: 180,
        },
        sources: vec![
            Source {
                id: "bg_generator".to_string(),
                kind: SourceKind::Generator {
                    media: SourceMediaType::Video,
                    filter: "testsrc2=size=720x1280:rate=30".to_string(),
                },
            },
            Source {
                id: "accent_generator".to_string(),
                kind: SourceKind::Generator {
                    media: SourceMediaType::Video,
                    filter: "color=c=0x1d9bf0:size=720x1280:rate=30".to_string(),
                },
            },
        ],
        layers: vec![
            Layer {
                id: "background_video".to_string(),
                z_index: 0,
                clips: vec![Clip {
                    id: "background_full".to_string(),
                    start_frame: 0,
                    duration_frames: 180,
                    opacity: 1.0,
                    transform: Transform {
                        x: 0.0,
                        y: 0.0,
                        width: Some(720.0),
                        height: Some(1280.0),
                        rotation_degrees: 0.0,
                    },
                    content: ClipContent::Video(VideoClip {
                        source: "bg_generator".to_string(),
                        pipeline: SourcePipeline {
                            trim: Some(TrimRange {
                                start_frame: 0,
                                end_frame: Some(90),
                            }),
                            speed: 1.0,
                            reverse: false,
                            looping: LoopMode::Infinite,
                        },
                        fit: FitMode::Fill,
                    }),
                }],
            },
            Layer {
                id: "accent_video".to_string(),
                z_index: 1,
                clips: vec![Clip {
                    id: "accent_window".to_string(),
                    start_frame: 30,
                    duration_frames: 120,
                    opacity: 0.28,
                    transform: Transform {
                        x: 120.0,
                        y: 280.0,
                        width: Some(480.0),
                        height: Some(720.0),
                        rotation_degrees: -5.0,
                    },
                    content: ClipContent::Video(VideoClip {
                        source: "accent_generator".to_string(),
                        pipeline: SourcePipeline {
                            trim: Some(TrimRange {
                                start_frame: 0,
                                end_frame: Some(60),
                            }),
                            speed: 1.5,
                            reverse: true,
                            looping: LoopMode::Infinite,
                        },
                        fit: FitMode::Cover,
                    }),
                }],
            },
            Layer {
                id: "shape_overlay".to_string(),
                z_index: 2,
                clips: vec![
                    Clip {
                        id: "top_bar".to_string(),
                        start_frame: 0,
                        duration_frames: 180,
                        opacity: 0.85,
                        transform: Transform {
                            x: 0.0,
                            y: 0.0,
                            width: Some(720.0),
                            height: Some(180.0),
                            rotation_degrees: 0.0,
                        },
                        content: ClipContent::Shape(ShapeClip {
                            shape: Shape::Rectangle {
                                fill: ColorRgba(6, 7, 10, 255),
                                radius: 0.0,
                            },
                        }),
                    },
                    Clip {
                        id: "pill".to_string(),
                        start_frame: 30,
                        duration_frames: 130,
                        opacity: 0.9,
                        transform: Transform {
                            x: 110.0,
                            y: 920.0,
                            width: Some(500.0),
                            height: Some(180.0),
                            rotation_degrees: 0.0,
                        },
                        content: ClipContent::Shape(ShapeClip {
                            shape: Shape::Rectangle {
                                fill: ColorRgba(20, 27, 39, 255),
                                radius: 90.0,
                            },
                        }),
                    },
                ],
            },
            Layer {
                id: "text".to_string(),
                z_index: 3,
                clips: vec![
                    Clip {
                        id: "headline".to_string(),
                        start_frame: 0,
                        duration_frames: 180,
                        opacity: 1.0,
                        transform: Transform {
                            x: 40.0,
                            y: 52.0,
                            width: Some(640.0),
                            height: Some(120.0),
                            rotation_degrees: 0.0,
                        },
                        content: ClipContent::Text(TextClip {
                            text: "Lumen Vello Pipeline".to_string(),
                            font_size: 54.0,
                            color: ColorRgba(255, 255, 255, 255),
                            align: TextAlign::Center,
                        }),
                    },
                    Clip {
                        id: "subheadline".to_string(),
                        start_frame: 30,
                        duration_frames: 140,
                        opacity: 1.0,
                        transform: Transform {
                            x: 150.0,
                            y: 980.0,
                            width: Some(420.0),
                            height: Some(90.0),
                            rotation_degrees: 0.0,
                        },
                        content: ClipContent::Text(TextClip {
                            text: "GPU decode + GPU raster + ffmpeg encode".to_string(),
                            font_size: 28.0,
                            color: ColorRgba(189, 206, 255, 255),
                            align: TextAlign::Center,
                        }),
                    },
                ],
            },
        ],
        audio: Default::default(),
    })
}
