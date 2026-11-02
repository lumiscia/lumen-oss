pub mod http;
mod media_staging;
mod types;
mod util;

use std::{collections::HashMap, time::Instant};

pub use types::{
    ApiRenderJob, ArtifactOutput, CreateRenderResponse, GetRenderResponse, RenderJobError,
    RenderJobInput, RenderJobMetrics, RenderJobResponse, RenderNotification,
    RenderProgressResponse, RenderQueueState,
};

use lumen_server::render::{
    ProjectBundle, RenderError as RenderPipelineError, RenderOptions, RenderProgress,
    render_project_mp4,
};

pub(super) use media_staging::{MediaClient, validate_media_manifest};
pub(super) use util::{current_timestamp, input_hash, new_render_id};

use self::{media_staging::stage_remote_media, util::sanitize_error_message};

pub struct RenderJob {
    pub id: String,
    pub bundle: ProjectBundle,
    pub media: HashMap<String, String>,
}

pub struct CompletedRender {
    pub artifact: ArtifactOutput,
    pub bytes: Vec<u8>,
    pub metrics: RenderJobMetrics,
}

pub async fn execute_render_job<F>(
    job: RenderJob,
    media_client: &MediaClient,
    artifact_url: String,
    verbose_debug: bool,
    mut on_progress: F,
) -> Result<CompletedRender, RenderJobError>
where
    F: FnMut(RenderProgress) + Send + 'static,
{
    let stage_started = Instant::now();
    let RenderJob {
        id: job_id,
        bundle,
        media,
    } = job;
    tracing::info!(job_id, media_count = media.len(), "starting render request");
    let staged_media = stage_remote_media(media_client, media).await?;
    let media_stage_ms = stage_started.elapsed().as_millis();
    tracing::info!(job_id, media_stage_ms, "staged render media");

    let options = RenderOptions {
        media_root: Some(staged_media.path().to_path_buf()),
        verbose_debug,
        video_encoder: None,
    };

    let output_width = bundle.project.width;
    let output_height = bundle.project.height;
    let total_frames = bundle.project.duration_frames;
    let render_started = Instant::now();
    tracing::info!(
        job_id,
        width = output_width,
        height = output_height,
        total_frames,
        "starting mp4 render"
    );
    let rendered_bytes = tokio::task::spawn_blocking(move || {
        let mut progress_callback = |event: RenderProgress| {
            if verbose_debug {
                tracing::debug!(
                    stage = event.stage,
                    frame = event.frame,
                    total_frames = event.total_frames,
                    ratio = event.ratio,
                    "render progress"
                );
            }
            on_progress(event);
        };

        render_project_mp4(&bundle, &options, &mut progress_callback)
    })
    .await
    .map_err(|err| RenderJobError {
        code: "render_worker_failed".to_string(),
        message: format!("render worker join failed: {err}"),
        retryable: true,
    })?
    .map_err(map_execution_error)?;
    let render_ms = render_started.elapsed().as_millis();
    tracing::info!(
        job_id,
        render_ms,
        output_bytes = rendered_bytes.len(),
        "finished mp4 render"
    );

    let artifact = ArtifactOutput {
        download_url: artifact_url,
        content_type: "video/mp4",
        bytes: rendered_bytes.len(),
    };
    let metrics = RenderJobMetrics {
        stage_ms: stage_started.elapsed().as_millis(),
        render_ms,
        total_frames: total_frames as u64,
    };

    tracing::info!(
        width = output_width,
        height = output_height,
        "completed self-hosted render"
    );

    Ok(CompletedRender {
        artifact,
        bytes: rendered_bytes,
        metrics,
    })
}

pub(super) fn map_execution_error(error: RenderPipelineError) -> RenderJobError {
    RenderJobError {
        code: error.code.to_string(),
        message: sanitize_error_message(&error.message),
        retryable: error.retryable,
    }
}
