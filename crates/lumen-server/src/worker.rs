use std::{
    collections::HashMap, env, io::Cursor, path::PathBuf, process::Command, sync::Arc,
    time::Duration,
};

use ac_ffmpeg::{format::io::IO, time::TimeBase};
use anyhow::anyhow;
use axum::body::Bytes;
use lumen::{
    compiler::compile_sequence,
    plan::RenderPlan,
    sequence::{Asset, AssetKind, Sequence},
    time::Time,
};
use serde::Serialize;
use tempfile::tempdir;
use tokio::{task::spawn_blocking, time::timeout};
use tracing::{error, info, instrument, warn};

use crate::{
    app_state::AppState,
    jobs::{ObjectBlob, RenderJobState, StorageError},
    video::{
        encode::H264Encoder,
        media::{AssetMediaProvider, resolve_asset_source_path},
        render::FFmpegRenderer,
    },
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

    let _ = state
        .job_store
        .set_progress(&job_id, 0.10, "rendering")
        .await;

    let result = timeout(
        render_timeout,
        spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
            let sequence: Sequence = serde_json::from_value(record.payload)?;
            render_sequence_to_mp4(sequence)
        }),
    )
    .await;

    match result {
        Ok(Ok(Ok(bytes))) => {
            let _ = state
                .job_store
                .set_progress(&job_id, 0.95, "storing_artifact")
                .await;

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
            notify_webhook(&state, &job_id).await;
        }
        Ok(Ok(Err(err))) => {
            state
                .job_store
                .mark_failed(&job_id, "render_failed", err.to_string())
                .await?;
            notify_webhook(&state, &job_id).await;
        }
        Ok(Err(err)) => {
            state
                .job_store
                .mark_failed(&job_id, "worker_join_failed", err.to_string())
                .await?;
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
            notify_webhook(&state, &job_id).await;
        }
    }

    Ok(())
}

fn render_sequence_to_mp4(sequence: Sequence) -> anyhow::Result<Vec<u8>> {
    let plan = Arc::new(compile_sequence(&sequence)?);
    let video_bytes = render_mp4(plan, sequence.assets.clone())?;
    mux_audio_if_needed(video_bytes, &sequence)
}

fn render_mp4(plan: Arc<RenderPlan>, assets: Vec<Asset>) -> anyhow::Result<Vec<u8>> {
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

fn mux_audio_if_needed(video_bytes: Vec<u8>, sequence: &Sequence) -> anyhow::Result<Vec<u8>> {
    let clips = resolve_audio_clips(sequence)?;
    if clips.is_empty() {
        return Ok(video_bytes);
    }

    let tmp = tempdir()?;
    let video_path = tmp.path().join("video.mp4");
    let output_path = tmp.path().join("output.mp4");
    std::fs::write(&video_path, video_bytes)?;

    let mut command = Command::new("ffmpeg");
    command
        .arg("-y")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-i")
        .arg(&video_path);

    for clip in &clips {
        command.arg("-i").arg(&clip.path);
    }

    let filter_graph = build_audio_filter_graph(&clips);
    command
        .arg("-filter_complex")
        .arg(filter_graph)
        .arg("-map")
        .arg("0:v:0")
        .arg("-map")
        .arg("[aout]")
        .arg("-c:v")
        .arg("copy")
        .arg("-c:a")
        .arg("aac")
        .arg("-shortest")
        .arg(&output_path);

    let output = command.output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "ffmpeg audio mux failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    std::fs::read(&output_path).map_err(|err| anyhow!("failed to read muxed output: {err}"))
}

#[derive(Debug)]
struct ResolvedAudioClip {
    path: PathBuf,
    start_ms: u64,
    source_in_secs: f64,
    duration_secs: f64,
    volume: f32,
}

fn resolve_audio_clips(sequence: &Sequence) -> anyhow::Result<Vec<ResolvedAudioClip>> {
    let assets: HashMap<&str, &Asset> = sequence
        .assets
        .iter()
        .map(|asset| (asset.id.as_str(), asset))
        .collect();

    let mut resolved = Vec::new();
    for track in &sequence.audio.tracks {
        for clip in &track.clips {
            let asset = assets.get(clip.asset_id.as_str()).ok_or_else(|| {
                anyhow!(
                    "missing audio asset `{}` for audio graph clip",
                    clip.asset_id
                )
            })?;

            if asset.kind != AssetKind::Audio {
                return Err(anyhow!(
                    "audio graph clip references non-audio asset `{}`",
                    clip.asset_id
                ));
            }

            let source_in = clip.source_in.unwrap_or(Time::ZERO);
            resolved.push(ResolvedAudioClip {
                path: resolve_asset_source_path(&asset.source)
                    .map_err(|err| anyhow!(err.to_string()))?,
                start_ms: millis(clip.start),
                source_in_secs: seconds(source_in),
                duration_secs: seconds(clip.duration),
                volume: clip.volume.max(0.0),
            });
        }
    }

    resolved.sort_by_key(|clip| clip.start_ms);
    Ok(resolved)
}

fn build_audio_filter_graph(clips: &[ResolvedAudioClip]) -> String {
    let mut stages = Vec::new();
    let mut labels = Vec::new();

    for (index, clip) in clips.iter().enumerate() {
        let input_index = index + 1;
        let label = format!("a{index}");
        stages.push(format!(
            "[{input_index}:a]atrim=start={:.6}:duration={:.6},asetpts=PTS-STARTPTS,volume={:.6},adelay={}|{}[{label}]",
            clip.source_in_secs,
            clip.duration_secs,
            clip.volume,
            clip.start_ms,
            clip.start_ms
        ));
        labels.push(format!("[{label}]"));
    }

    if labels.len() == 1 {
        stages.push(format!("{}anull[aout]", labels[0]));
    } else {
        stages.push(format!(
            "{}amix=inputs={}:duration=longest:dropout_transition=0[aout]",
            labels.join(""),
            labels.len()
        ));
    }

    stages.join(";")
}

fn seconds(time: Time) -> f64 {
    if time.timescale == 0 {
        return 0.0;
    }

    (time.value.max(0) as f64) / (time.timescale as f64)
}

fn millis(time: Time) -> u64 {
    (seconds(time) * 1_000.0).round() as u64
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
