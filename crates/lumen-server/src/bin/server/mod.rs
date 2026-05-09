pub mod http;
mod media_staging;
mod types;
mod util;

use std::time::Instant;

pub use types::{
    ApiRenderJob, ArtifactOutput, CreateRenderResponse, GetRenderResponse, RenderJobError,
    RenderJobInput, RenderJobMetrics, RenderJobResponse, RenderNotification,
    RenderProgressResponse, RenderQueueState,
};

use lumen_server::render::{
    RenderError as RenderPipelineError, RenderOptions, RenderProgress, convert_project_payload,
    render_project_mp4,
};

pub(super) use util::{current_timestamp, input_hash, new_render_id};

use self::{media_staging::stage_remote_media, util::sanitize_error_message};

pub struct CompletedRender {
    pub artifact: ArtifactOutput,
    pub bytes: Vec<u8>,
    pub metrics: RenderJobMetrics,
    pub render: ApiRenderJob,
}

pub async fn execute_render_job<F>(
    input: RenderJobInput,
    artifact_url: String,
    verbose_debug: bool,
    mut on_progress: F,
) -> Result<CompletedRender, RenderJobError>
where
    F: FnMut(RenderProgress) + Send + 'static,
{
    let stage_started = Instant::now();
    let job_id = input.job_id.unwrap_or_else(new_render_id);
    tracing::info!(
        job_id,
        media_count = input.media.len(),
        has_webhook = input.webhook_url.is_some(),
        "starting render request"
    );
    let input_hash = input_hash(&input.composition, &input.media);
    let staged_media = stage_remote_media(input.composition, input.media).await?;
    let media_stage_ms = stage_started.elapsed().as_millis();
    tracing::info!(job_id, media_stage_ms, "staged render media");

    let bundle = convert_project_payload(&staged_media.project).map_err(map_execution_error)?;
    tracing::info!(
        job_id,
        width = bundle.project.width,
        height = bundle.project.height,
        total_frames = bundle.project.duration_frames,
        "converted project payload"
    );

    let options = RenderOptions {
        media_root: staged_media.media_root(),
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
    let render = ApiRenderJob {
        cost_cents: 0,
        created_at: current_timestamp(),
        id: job_id,
        input_hash,
        organization_id: "self-hosted".to_string(),
        status: "succeeded",
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
        render,
    })
}

fn map_execution_error(error: RenderPipelineError) -> RenderJobError {
    RenderJobError {
        code: error.code.to_string(),
        message: sanitize_error_message(&error.message),
        retryable: error.retryable,
    }
}
