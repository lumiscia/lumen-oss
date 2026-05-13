use std::time::Instant;

use crate::render::{RenderOptions, convert_project_payload, render_project_mp4};

use super::{
    BoxFuture, ProgressEvent, ProgressSink, RenderExecutor, RenderJob, RenderMetrics, RenderOutput,
    ServiceError, ServiceResult,
};

#[derive(Debug, Default)]
pub struct LocalRenderExecutor;

impl RenderExecutor for LocalRenderExecutor {
    fn execute<'a>(
        &'a self,
        job: RenderJob,
        progress: &'a dyn ProgressSink,
    ) -> BoxFuture<'a, ServiceResult<RenderOutput>> {
        Box::pin(async move {
            let bundle = convert_project_payload(&job.project).map_err(ServiceError::from)?;
            let total_frames = bundle.project.duration_frames;
            let options = RenderOptions {
                media_root: job.media_root,
                verbose_debug: false,
                video_encoder: job.video_encoder,
            };
            let job_id = job.id.clone();
            let started = Instant::now();
            progress
                .publish(ProgressEvent {
                    job_id: job_id.clone(),
                    stage: "accepted".to_string(),
                    ratio: 0.0,
                    frame: None,
                    total_frames: Some(total_frames),
                })
                .await?;
            let rendered = tokio::task::spawn_blocking(move || {
                let mut ignore_progress = |_event| {};
                render_project_mp4(&bundle, &options, &mut ignore_progress)
            })
            .await
            .map_err(|err| ServiceError {
                code: "render_worker_failed",
                message: format!("render worker join failed: {err}"),
                retryable: true,
            })?
            .map_err(ServiceError::from)?;
            progress
                .publish(ProgressEvent {
                    job_id,
                    stage: "completed".to_string(),
                    ratio: 1.0,
                    frame: Some(total_frames),
                    total_frames: Some(total_frames),
                })
                .await?;

            Ok(RenderOutput {
                bytes: rendered,
                content_type: "video/mp4",
                metrics: RenderMetrics {
                    render_ms: started.elapsed().as_millis(),
                    total_frames: total_frames as u64,
                },
            })
        })
    }
}
