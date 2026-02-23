use std::{
    env,
    time::{Duration, Instant},
};

use axum::body::Bytes;
use serde::Serialize;
use tokio::{sync::mpsc, task::spawn_blocking, time::timeout};
use tracing::{error, info, instrument, warn};

use crate::{
    app_state::AppState,
    jobs::{ObjectBlob, RenderJobState, StorageError},
    render::{
        RenderMetrics, RenderOptions, RenderProgress, convert_project_payload, render_project_mp4,
    },
};

const DEFAULT_RENDER_TIMEOUT_SECS: u64 = 900;
const DEFAULT_WORKER_CONCURRENCY: usize = 1;

pub fn spawn_render_worker(state: AppState) {
    let workers = env::var("LUMEN_WORKER_CONCURRENCY")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_WORKER_CONCURRENCY);

    for worker_index in 0..workers {
        let worker_state = state.clone();
        tokio::spawn(async move {
            info!(worker_index, "render worker started");
            loop {
                let job_id = match worker_state.job_queue.reserve().await {
                    Ok(job_id) => job_id,
                    Err(err) => {
                        error!(worker_index, "failed to reserve job: {err}");
                        continue;
                    }
                };

                if let Err(err) = process_job(worker_state.clone(), job_id.clone()).await {
                    error!(worker_index, job_id, "failed to process job: {err}");
                }
            }
        });
    }
}

#[instrument(skip(state))]
async fn process_job(state: AppState, job_id: String) -> anyhow::Result<()> {
    let job_started = Instant::now();
    info!(job_id, "render job started");

    if let Err(err) = state.job_store.mark_running(&job_id).await {
        match err {
            StorageError::InvalidState { .. } => {
                info!(job_id, "skipping job due to invalid state: {err}");
            }
            _ => {
                error!(job_id, "failed to mark job running: {err}");
            }
        }
        return Ok(());
    }

    let _ = state
        .job_store
        .set_progress(&job_id, 0.05, "deserializing")
        .await;

    let record = match state.job_store.get_record(&job_id).await {
        Ok(Some(record)) => record,
        Ok(None) => {
            error!(job_id, "job disappeared before processing");
            return Ok(());
        }
        Err(err) => {
            error!(job_id, "failed to load job record: {err}");
            return Ok(());
        }
    };

    let render_timeout = Duration::from_secs(
        env::var("LUMEN_RENDER_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_RENDER_TIMEOUT_SECS),
    );

    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<ProgressUpdate>();
    let progress_store = state.job_store.clone();
    let progress_job_id = job_id.clone();
    let progress_task = tokio::spawn(async move {
        while let Some(update) = progress_rx.recv().await {
            let _ = progress_store
                .set_progress(&progress_job_id, update.progress, &update.stage)
                .await;
        }
    });

    let result = timeout(
        render_timeout,
        spawn_blocking(
            move || -> Result<RenderOutput, crate::render::RenderError> {
                let emit = |progress: f32, stage: &str| {
                    let _ = progress_tx.send(ProgressUpdate {
                        progress,
                        stage: stage.to_string(),
                    });
                };

                emit(0.10, "converting");
                let convert_started = Instant::now();
                let bundle = convert_project_payload(&record.payload)?;
                let convert_ms = convert_started.elapsed().as_millis();

                emit(0.16, "rendering");
                let render_started = Instant::now();
                let options = RenderOptions::default();
                let bytes =
                    render_project_mp4(&bundle, &options, &mut |update: RenderProgress| {
                        let progress = 0.16 + (0.78 * update.ratio.clamp(0.0, 1.0));
                        emit(progress, update.stage);
                    })?;
                let render_ms = render_started.elapsed().as_millis();

                emit(0.95, "storing_artifact");
                Ok(RenderOutput {
                    bytes,
                    metrics: RenderMetrics {
                        convert_ms,
                        render_ms,
                        total_frames: bundle.project.duration_frames,
                    },
                })
            },
        ),
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), progress_task).await;

    match result {
        Ok(Ok(Ok(output))) => {
            let _ = state
                .job_store
                .set_progress(&job_id, 0.95, "storing_artifact")
                .await;

            let artifact_key = format!("jobs/{job_id}/artifact.mp4");
            let store_started = Instant::now();
            state
                .object_store
                .put(
                    artifact_key.clone(),
                    ObjectBlob {
                        content_type: "video/mp4".to_string(),
                        bytes: Bytes::from(output.bytes),
                    },
                )
                .await?;
            let store_ms = store_started.elapsed().as_millis();

            state
                .job_store
                .mark_completed(&job_id, artifact_key)
                .await?;
            info!(
                job_id,
                convert_ms = output.metrics.convert_ms,
                render_ms = output.metrics.render_ms,
                total_frames = output.metrics.total_frames,
                store_ms,
                total_job_ms = job_started.elapsed().as_millis(),
                "render job completed"
            );
            notify_webhook(&state, &job_id).await;
        }
        Ok(Ok(Err(err))) => {
            state
                .job_store
                .mark_failed(&job_id, err.code, err.message)
                .await?;
            error!(
                job_id,
                elapsed_ms = job_started.elapsed().as_millis(),
                "render job failed"
            );
            notify_webhook(&state, &job_id).await;
        }
        Ok(Err(err)) => {
            state
                .job_store
                .mark_failed(&job_id, "worker_join_failed", err.to_string())
                .await?;
            error!(
                job_id,
                elapsed_ms = job_started.elapsed().as_millis(),
                "render worker join failed"
            );
            notify_webhook(&state, &job_id).await;
        }
        Err(_) => {
            state
                .job_store
                .mark_failed(
                    &job_id,
                    "render_timeout",
                    format!("render exceeded {}s timeout", render_timeout.as_secs()),
                )
                .await?;
            error!(
                job_id,
                elapsed_ms = job_started.elapsed().as_millis(),
                "render job timeout"
            );
            notify_webhook(&state, &job_id).await;
        }
    }

    Ok(())
}

struct ProgressUpdate {
    progress: f32,
    stage: String,
}

struct RenderOutput {
    bytes: Vec<u8>,
    metrics: RenderMetrics,
}

#[derive(Serialize)]
struct JobWebhookPayload {
    event: &'static str,
    job: crate::jobs::RenderJobStatus,
}

async fn notify_webhook(state: &AppState, job_id: &str) {
    let webhook_url = match env::var("LUMEN_JOB_WEBHOOK_URL") {
        Ok(url) if !url.trim().is_empty() => url,
        _ => return,
    };

    let status = match state.job_store.get_status(job_id).await {
        Ok(Some(status)) => status,
        Ok(None) => return,
        Err(err) => {
            warn!(job_id, "failed to load job status for webhook: {err}");
            return;
        }
    };

    let event = match status.state {
        RenderJobState::Completed => "render.completed",
        RenderJobState::Failed => "render.failed",
        RenderJobState::Canceled => "render.canceled",
        RenderJobState::Queued => "render.queued",
        RenderJobState::Running => "render.running",
    };

    let payload = JobWebhookPayload { event, job: status };
    let client = reqwest::Client::new();
    if let Err(err) = client
        .post(webhook_url)
        .json(&payload)
        .timeout(Duration::from_secs(5))
        .send()
        .await
    {
        warn!(job_id, "failed to notify webhook: {err}");
    }
}
