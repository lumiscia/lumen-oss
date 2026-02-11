use std::{env, io::Cursor, sync::Arc, time::Duration};

use ac_ffmpeg::{format::io::IO, time::TimeBase};
use axum::body::Bytes;
use lumen::{compiler::compile_sequence, plan::RenderPlan, sequence::Sequence};
use tokio::{task::spawn_blocking, time::timeout};
use tracing::{error, info, instrument};

use crate::{
    app_state::AppState,
    jobs::ObjectBlob,
    video::{encode::H264Encoder, media::AssetMediaProvider, render::FFmpegRenderer},
};

const DEFAULT_RENDER_TIMEOUT_SECS: u64 = 600;
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
    if let Err(err) = state.job_store.mark_running(&job_id).await {
        error!(job_id, "failed to mark job running: {err}");
        return Ok(());
    }

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

    let result = timeout(
        render_timeout,
        spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
            let sequence: Sequence = serde_json::from_value(record.payload)?;
            let plan = Arc::new(compile_sequence(&sequence)?);
            render_mp4(plan, sequence.assets.clone())
        }),
    )
    .await;

    match result {
        Ok(Ok(Ok(bytes))) => {
            let artifact_key = format!("jobs/{job_id}/artifact.mp4");
            state
                .object_store
                .put(
                    artifact_key.clone(),
                    ObjectBlob {
                        content_type: "video/mp4".to_string(),
                        bytes: Bytes::from(bytes),
                    },
                )
                .await?;
            state
                .job_store
                .mark_completed(&job_id, artifact_key)
                .await?;
        }
        Ok(Ok(Err(err))) => {
            state
                .job_store
                .mark_failed(&job_id, "render_failed", err.to_string())
                .await?;
        }
        Ok(Err(err)) => {
            state
                .job_store
                .mark_failed(&job_id, "worker_join_failed", err.to_string())
                .await?;
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
        }
    }

    Ok(())
}

fn render_mp4(
    plan: Arc<RenderPlan>,
    assets: Vec<lumen::sequence::Asset>,
) -> anyhow::Result<Vec<u8>> {
    let time_base = TimeBase::new(plan.fps.den as i32, plan.fps.num as i32);
    let media = AssetMediaProvider::new(assets, plan.fps)?;
    let mut renderer = FFmpegRenderer::new(plan.clone(), media, time_base)?;

    let output = Cursor::new(Vec::new());
    let mut encoder = H264Encoder::new(
        plan.canvas.width as usize,
        plan.canvas.height as usize,
        time_base,
        IO::from_seekable_write_stream(output),
    )?;

    for frame in 0..plan.total_frames {
        let frame = renderer.draw_frame(frame as usize)?;
        encoder.encode_frame(frame)?;
    }

    encoder.finish()?;

    let io = encoder.close()?;
    let output = io.into_stream();

    Ok(output.into_inner())
}
