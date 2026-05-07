mod artifact;
mod media;
mod progress;
mod types;
mod util;

use std::time::Instant;

pub use types::{
    ArtifactOutput, ArtifactStaging, ProgressCallback, RenderJobError, RenderJobInput,
    RenderJobMetrics, RenderJobResponse, RenderProfile,
};

use crate::render::{
    RenderError as RenderPipelineError, RenderOptions, RenderProgress, convert_project_payload,
    render_project_mp4,
};

use self::{
    artifact::{upload_artifact, validate_artifact_staging},
    media::stage_remote_media,
    progress::{ProgressMetadata, post_progress_async, post_progress_sync},
    util::sanitize_error_message,
};

pub async fn handle_render_job(input: RenderJobInput) -> RenderJobResponse {
    let job_id = input.job_id.clone();
    let progress_callback = input.progress_callback.clone();
    match execute_render_job(input).await {
        Ok(response) => {
            tracing::info!(
                job_id,
                ok = response.ok,
                artifact_bytes = response.artifact.as_ref().map(|artifact| artifact.bytes),
                render_ms = response.metrics.as_ref().map(|metrics| metrics.render_ms),
                total_frames = response
                    .metrics
                    .as_ref()
                    .map(|metrics| metrics.total_frames),
                "completed render request"
            );
            response
        }
        Err(error) => {
            let error_code = error.code.clone();
            let error_message = error.message.clone();
            let retryable = error.retryable;
            tracing::error!(
                job_id,
                code = error_code,
                retryable,
                message = %error_message,
                "render request failed"
            );
            if let Err(callback_error) = post_progress_async(
                &progress_callback,
                1.0,
                "failed",
                "failed",
                None,
                Some(&error_message),
            )
            .await
            {
                tracing::warn!(
                    job_id,
                    code = error_code,
                    "failed to post terminal failure progress callback: {callback_error}"
                );
            }

            RenderJobResponse {
                ok: false,
                artifact: None,
                metrics: None,
                error: Some(error),
            }
        }
    }
}

async fn execute_render_job(input: RenderJobInput) -> Result<RenderJobResponse, RenderJobError> {
    let stage_started = Instant::now();
    let job_id = input.job_id.clone();
    let allowed_media_hosts = input
        .render_profile
        .as_ref()
        .map(|profile| profile.allowed_media_hosts.as_slice())
        .unwrap_or(&[]);
    tracing::info!(
        job_id,
        media_count = input.media.len(),
        allowed_media_host_count = allowed_media_hosts.len(),
        has_artifact_staging = input.artifact_staging.is_some(),
        has_progress_callback = input.progress_callback.is_some(),
        "starting render request"
    );
    let staged_media = stage_remote_media(input.project, input.media, allowed_media_hosts).await?;
    let media_stage_ms = stage_started.elapsed().as_millis();
    tracing::info!(job_id, media_stage_ms, "staged render media");
    post_progress_async(
        &input.progress_callback,
        0.02,
        "media_staged",
        "processing",
        None,
        None,
    )
    .await
    .map_err(|err| RenderJobError {
        code: "progress_callback_failed".to_string(),
        message: sanitize_error_message(&format!("media_staged progress callback failed: {err}")),
        retryable: true,
    })?;

    let bundle = convert_project_payload(&staged_media.project).map_err(map_execution_error)?;
    tracing::info!(
        job_id,
        width = bundle.project.width,
        height = bundle.project.height,
        total_frames = bundle.project.duration_frames,
        "converted project payload"
    );
    validate_artifact_staging(&input.artifact_staging)?;

    let options = RenderOptions {
        media_root: staged_media.media_root(),
        video_encoder: None,
    };

    post_progress_sync(
        &input.progress_callback,
        0.05,
        "accepted",
        "processing",
        None,
        None,
    );

    let callback = input.progress_callback.clone();
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
    let mut progress_reporter = ProgressReporter::new(job_id.clone(), callback);
    let rendered_bytes = tokio::task::spawn_blocking(move || {
        let mut progress_callback = |event: RenderProgress| {
            progress_reporter.report(event);
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

    let artifact = upload_artifact(&input.artifact_staging, &rendered_bytes).await?;
    post_progress_async(
        &input.progress_callback,
        1.0,
        "completed",
        "succeeded",
        Some(ProgressMetadata {
            artifact_url: Some(&artifact.download_url),
            duration_ms: Some(render_ms),
            output_bytes: Some(artifact.bytes),
            resolution: Some(format!("{output_width}x{output_height}")),
        }),
        None,
    )
    .await
    .map_err(|err| RenderJobError {
        code: "progress_callback_failed".to_string(),
        message: sanitize_error_message(&format!(
            "terminal success progress callback failed: {err}"
        )),
        retryable: true,
    })?;

    Ok(RenderJobResponse {
        ok: true,
        artifact: Some(artifact),
        metrics: Some(RenderJobMetrics {
            stage_ms: stage_started.elapsed().as_millis(),
            render_ms,
            total_frames: total_frames as u64,
        }),
        error: None,
    })
}

fn map_execution_error(error: RenderPipelineError) -> RenderJobError {
    RenderJobError {
        code: error.code.to_string(),
        message: sanitize_error_message(&error.message),
        retryable: error.retryable,
    }
}

struct ProgressReporter {
    callback: Option<ProgressCallback>,
    job_id: String,
    last_emit: Option<Instant>,
    last_ratio: f32,
}

impl ProgressReporter {
    fn new(job_id: String, callback: Option<ProgressCallback>) -> Self {
        Self {
            callback,
            job_id,
            last_emit: None,
            last_ratio: 0.0,
        }
    }

    fn report(&mut self, event: RenderProgress) {
        if !self.should_emit(&event) {
            return;
        }

        tracing::info!(
            job_id = self.job_id,
            stage = event.stage,
            frame = event.frame,
            total_frames = event.total_frames,
            ratio = event.ratio,
            "render progress"
        );
        let progress = 0.05 + (0.90 * event.ratio.clamp(0.0, 1.0));
        post_progress_sync(
            &self.callback,
            progress,
            event.stage,
            "processing",
            None,
            None,
        );
        self.last_emit = Some(Instant::now());
        self.last_ratio = event.ratio;
    }

    fn should_emit(&self, event: &RenderProgress) -> bool {
        if event.frame <= 1 || event.frame >= event.total_frames {
            return true;
        }

        if event.ratio - self.last_ratio >= 0.05 {
            return true;
        }

        self.last_emit
            .is_some_and(|last_emit| last_emit.elapsed().as_secs() >= 1)
    }
}
