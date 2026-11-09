pub mod http;
mod media_staging;
mod types;
mod util;

use std::{collections::HashMap, path::Path, time::Instant};

pub use types::{
    ApiRenderJob, Artifact, CheckInput, CheckResponse, CreateRenderResponse, GetRenderResponse,
    RenderJobError, RenderJobInput, RenderProgressResponse, RenderQueueState, RenderRecord,
    RenderStatus, sanitized_diagnostics,
};

use lumen_engine::composition::Composition;
use lumen_ffmpeg::VideoEncoderChoice;
use lumen_server::render::{
    RenderError as RenderPipelineError, RenderOptions, RenderProgress, render_project_mp4,
};

pub(super) use media_staging::{MediaClient, validate_media_manifest};
pub(super) use util::{current_timestamp, input_hash, new_render_id};

use self::{media_staging::stage_remote_media, util::sanitize_error_message};

pub struct RenderJob {
    pub id: String,
    pub composition: Composition,
    pub media: HashMap<String, String>,
    pub encoder: VideoEncoderChoice,
}

pub struct CompletedRender {
    pub bytes: Vec<u8>,
    pub render_ms: u128,
}

pub async fn execute_render_job<F>(
    job: RenderJob,
    media_client: &MediaClient,
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
    } = job;
    tracing::info!(job_id, media_count = media.len(), "starting render request");
    let staged_media = stage_remote_media(media_client, media).await?;
    let media_stage_ms = stage_started.elapsed().as_millis();
    tracing::info!(job_id, media_stage_ms, "staged render media");

    let staged_path = staged_media.path().to_path_buf();
    let options = RenderOptions {
        media_root: Some(staged_path.clone()),
        encoder: Some(encoder),
    };

    let output_width = composition.render_settings().width;
    let output_height = composition.render_settings().height;
    let total_frames = composition.timeline().duration_frames;
    let render_started = Instant::now();
    tracing::info!(
        job_id,
        width = output_width,
        height = output_height,
        total_frames,
        "starting mp4 render"
    );
    let render_span = tracing::info_span!("render_job", job_id);
    let rendered_bytes = tokio::task::spawn_blocking(move || {
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

        render_project_mp4(&composition, &options, &mut progress_callback)
    })
    .await
    .map_err(|err| RenderJobError {
        code: "render_worker_failed".to_string(),
        message: format!("render worker join failed: {err}"),
        retryable: true,
    })?
    .map_err(|mut error| {
        tracing::warn!(job_id, code = error.code, error = %error.message, "render failed");
        error.message = without_staging_path(&error.message, &staged_path);
        map_execution_error(error)
    })?;
    let render_ms = render_started.elapsed().as_millis();
    tracing::info!(
        job_id,
        render_ms,
        output_bytes = rendered_bytes.len(),
        "finished mp4 render"
    );

    tracing::info!(
        width = output_width,
        height = output_height,
        "completed self-hosted render"
    );

    Ok(CompletedRender {
        bytes: rendered_bytes,
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

pub(super) fn map_execution_error(error: RenderPipelineError) -> RenderJobError {
    RenderJobError {
        code: error.code.to_string(),
        message: sanitize_error_message(&error.message),
        retryable: error.retryable,
    }
}
