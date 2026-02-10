use std::{io::Cursor, sync::Arc};

use ac_ffmpeg::{format::io::IO, time::TimeBase};
use lumen::{compiler::compile_sequence, plan::RenderPlan, sequence::Sequence};
use tracing::{error, info, instrument};

use crate::{
    app_state::AppState,
    jobs::ObjectBlob,
    video::{encode::H264Encoder, render::FFmpegRenderer},
};

pub fn spawn_render_worker(state: AppState) {
    tokio::spawn(async move {
        info!("render worker started");
        loop {
            let job_id = match state.job_queue.reserve().await {
                Ok(job_id) => job_id,
                Err(err) => {
                    error!("failed to reserve job: {err:#}");
                    continue;
                }
            };

            if let Err(err) = process_job(state.clone(), job_id.clone()).await {
                error!("failed to process job {}: {err:#}", job_id);
            }
        }
    });
}

#[instrument(skip(state))]
async fn process_job(state: AppState, job_id: String) -> anyhow::Result<()> {
    state.job_store.mark_running(&job_id).await?;

    let result = async {
        let record = state
            .job_store
            .get_record(&job_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("missing job {job_id}"))?;

        let sequence: Sequence = serde_json::from_value(record.payload)?;
        let plan = Arc::new(compile_sequence(&sequence)?);
        let bytes = render_mp4(plan)?;

        let artifact_key = format!("jobs/{job_id}/artifact.mp4");
        state
            .object_store
            .put(
                artifact_key.clone(),
                ObjectBlob {
                    content_type: "video/mp4".to_string(),
                    bytes,
                },
            )
            .await?;

        state.job_store.mark_completed(&job_id, artifact_key).await?;

        anyhow::Ok(())
    }
    .await;

    if let Err(err) = result {
        state
            .job_store
            .mark_failed(&job_id, "render_failed", err.to_string())
            .await?;
    }

    Ok(())
}

fn render_mp4(plan: Arc<RenderPlan>) -> anyhow::Result<Vec<u8>> {
    let time_base = TimeBase::new(plan.fps.den as i32, plan.fps.num as i32);
    let mut renderer = FFmpegRenderer::new(plan.clone(), time_base)?;

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
