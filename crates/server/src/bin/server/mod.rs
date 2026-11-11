mod assets;
pub mod http;
mod media_staging;
mod types;
mod util;

use std::{collections::HashMap, path::Path, sync::Arc, time::Instant};

pub use types::{
    ApiRenderJob, Artifact, Cancellation, CheckInput, CheckResponse, CreateRenderResponse,
    GetRenderResponse, QueueSpot, RenderJobError, RenderJobInput, RenderProgressResponse,
    RenderQueueState, RenderRecord, RenderStatus, sanitized_diagnostics,
};

use lumen_engine::composition::Composition;
use lumen_ffmpeg::VideoEncoderChoice;
use lumen_server::render::{
    ErrorCode, OutputFormat, RenderConfig, RenderOptions, RenderOutput, RenderProgress,
    SequenceLimits, ServerRenderError, render_project,
};

pub(super) use media_staging::{MediaClient, validate_media_manifest};
pub(super) use util::{current_timestamp, input_hash, new_render_id};

use self::{assets::JobAssets, media_staging::stage_remote_media, util::sanitize_error_message};

pub struct RenderJob {
    pub id: String,
    pub composition: Composition,
    pub media: HashMap<String, String>,
    /// The encoders an MP4 render tries. Image outputs have none.
    pub encoder: Option<VideoEncoderChoice>,
    pub output: RenderOutput,
    pub sequence_limits: SequenceLimits,
    /// The most glyph, stroke and decoration instances one text node may draw.
    pub max_text_glyphs: usize,
    /// The `asset:` references this render resolves, held until it ends.
    pub assets: Arc<JobAssets>,
}

pub struct CompletedRender {
    pub bytes: Vec<u8>,
    pub format: OutputFormat,
    pub render_ms: u128,
}

pub async fn execute_render_job<F>(
    job: RenderJob,
    media_client: &MediaClient,
    config: &RenderConfig,
    cancellation: &Cancellation,
    mut on_progress: F,
) -> Result<CompletedRender, RenderJobError>
where
    F: FnMut(RenderProgress) + Send + 'static,
{
    let stage_started = Instant::now();
    let RenderJob {
        id: job_id,
        composition,
        media,
        encoder,
        output,
        sequence_limits,
        max_text_glyphs,
        assets,
    } = job;
    tracing::info!(job_id, media_count = media.len(), "starting render request");
    let staged_media = tokio::select! {
        staged = stage_remote_media(media_client, media) => staged?,
        () = cancellation.cancelled() => return Err(RenderJobError::cancelled()),
    };
    let media_stage_ms = stage_started.elapsed().as_millis();
    tracing::info!(job_id, media_stage_ms, "staged render media");

    let staged_path = staged_media.path().to_path_buf();
    let options = RenderOptions {
        config: config.clone(),
        media_root: Some(staged_path.clone()),
        encoder,
        assets: Some(assets),
        max_text_glyphs: Some(max_text_glyphs),
        output: output.clone(),
        sequence_limits,
        cancel: cancellation.token(),
    };

    let output_width = composition.render_settings().width;
    let output_height = composition.render_settings().height;
    let total_frames = composition.timeline().duration_frames;
    let format = output.format;
    let render_started = Instant::now();
    tracing::info!(
        job_id,
        width = output_width,
        height = output_height,
        total_frames,
        ?format,
        "starting render"
    );
    let render_span = tracing::info_span!("render_job", job_id);
    let rendered = tokio::task::spawn_blocking(move || {
        let _render_span = render_span.enter();
        let mut frame_started = std::time::Instant::now();
        let mut progress_callback = |event: RenderProgress| {
            tracing::debug!(
                stage = event.stage,
                frame = event.frame,
                total_frames = event.total_frames,
                ratio = event.ratio,
                "render progress"
            );
            tracing::debug!(
                target: "lumen_frame_time",
                frame = event.frame,
                us = frame_started.elapsed().as_micros() as u64,
                "frame"
            );
            frame_started = std::time::Instant::now();
            on_progress(event);
        };

        render_project(&composition, &options, &mut progress_callback)
    })
    .await
    .map_err(|err| RenderJobError {
        code: "render_worker_failed".to_string(),
        message: format!("render worker join failed: {err}"),
        retryable: true,
    })?
    .map_err(|error| {
        let message = error_chain(&error);
        if error.code() == ErrorCode::Cancelled {
            tracing::info!(job_id, "render cancelled");
        } else {
            tracing::warn!(job_id, code = %error.code(), error = %message, "render failed");
        }
        RenderJobError {
            code: error.code().to_string(),
            message: sanitize_error_message(&without_staging_path(&message, &staged_path)),
            retryable: error.retryable(),
        }
    })?;
    let render_ms = render_started.elapsed().as_millis();
    tracing::info!(
        job_id,
        render_ms,
        output_bytes = rendered.bytes.len(),
        "finished render"
    );

    tracing::info!(
        width = output_width,
        height = output_height,
        "completed self-hosted render"
    );

    Ok(CompletedRender {
        bytes: rendered.bytes,
        format: rendered.format,
        render_ms,
    })
}

/// Leaves file names relative to the job's media, since clients know those and not where it's staged.
fn without_staging_path(message: &str, staged_path: &Path) -> String {
    let canonical = staged_path.canonicalize().ok();
    [Some(staged_path), canonical.as_deref()]
        .into_iter()
        .flatten()
        .fold(message.to_string(), |message, root| {
            message
                .replace(&format!("{}/", root.display()), "")
                .replace(&root.display().to_string(), ".")
        })
}

/// `error` and each error that caused it, as `outer: inner`.
fn error_chain(error: &ServerRenderError) -> String {
    let mut message = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}
