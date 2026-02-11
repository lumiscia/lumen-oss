use std::{path::PathBuf, sync::Arc, time::Instant};

use lumen::{Project, compile_project};

use crate::video::{FfmpegRenderBackend, RenderBackendOptions};

#[derive(Debug, Clone, Default)]
pub struct RenderExecutionOptions {
    pub media_root: Option<PathBuf>,
    pub video_encoder: Option<String>,
    pub encode_queue: Option<usize>,
    pub max_decoded_source_frames: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct RenderExecutionProgress {
    pub stage: &'static str,
    pub frame: u64,
    pub total_frames: u64,
    pub ratio: f32,
}

#[derive(Debug, Clone)]
pub struct RenderExecutionMetrics {
    pub compile_ms: u128,
    pub render_ms: u128,
    pub total_frames: u64,
}

#[derive(Debug, Clone)]
pub struct RenderExecutionResult {
    pub bytes: Vec<u8>,
    pub metrics: RenderExecutionMetrics,
}

#[derive(Debug, Clone)]
pub struct RenderExecutionError {
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
}

impl std::fmt::Display for RenderExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RenderExecutionError {}

pub fn execute_render(
    project: &Project,
    options: &RenderExecutionOptions,
    on_progress: &mut dyn FnMut(RenderExecutionProgress),
) -> Result<RenderExecutionResult, RenderExecutionError> {
    let compile_started = Instant::now();
    let timeline = compile_project(project).map_err(|err| RenderExecutionError {
        code: "compile_failed",
        message: err.to_string(),
        retryable: false,
    })?;
    let compile_ms = compile_started.elapsed().as_millis();

    let total_frames = timeline.total_frames();
    on_progress(RenderExecutionProgress {
        stage: "compiled",
        frame: 0,
        total_frames,
        ratio: 0.05,
    });

    let backend_options = RenderBackendOptions {
        media_root: options.media_root.clone(),
        video_encoder: options.video_encoder.clone(),
        encode_queue: options.encode_queue,
        max_decoded_source_frames: options.max_decoded_source_frames,
    };

    let render_started = Instant::now();
    let backend = FfmpegRenderBackend::new_with_options(Arc::new(timeline), backend_options);
    let bytes = backend
        .render_to_mp4(&mut |frame, total| {
            let ratio = if total == 0 {
                0.0
            } else {
                (frame as f32 / total as f32).clamp(0.0, 1.0)
            };
            on_progress(RenderExecutionProgress {
                stage: "rendering",
                frame,
                total_frames: total,
                ratio,
            });
        })
        .map_err(|err| RenderExecutionError {
            code: "render_failed",
            message: err.to_string(),
            retryable: true,
        })?;
    let render_ms = render_started.elapsed().as_millis();

    Ok(RenderExecutionResult {
        bytes,
        metrics: RenderExecutionMetrics {
            compile_ms,
            render_ms,
            total_frames,
        },
    })
}
