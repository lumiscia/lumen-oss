use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    response::Response,
};
use lumen::{compiler::compile_sequence, render::Renderer, sequence::Sequence, time::FrameIndex};
use tokio::task::spawn_blocking;

use crate::{
    api_error::ApiError,
    app_state::AppState,
    jobs::{ObjectBlob, RenderJobState},
    video::{ServerFontManager, media::AssetMediaProvider},
};

const MAX_ASSETS: usize = 512;
const MAX_TRACKS: usize = 128;
const MAX_TOTAL_CLIPS: usize = 4_096;
const MAX_TOTAL_FRAMES: u64 = 216_000;
const MAX_CANVAS_DIMENSION: u32 = 7680;

pub async fn create_render(
    State(state): State<AppState>,
    Json(sequence): Json<Sequence>,
) -> Result<(StatusCode, Json<crate::jobs::RenderJobStatus>), ApiError> {
    let plan = compile_sequence(&sequence).map_err(|err| ApiError::bad_request(err.to_string()))?;
    validate_sequence_limits(&sequence, plan.total_frames)?;

    let payload = serde_json::to_value(sequence).map_err(ApiError::internal)?;

    let status = state
        .job_store
        .create(payload)
        .await
        .map_err(ApiError::from)?;
    state
        .job_queue
        .enqueue(status.job_id.clone())
        .await
        .map_err(ApiError::from)?;

    Ok((StatusCode::ACCEPTED, Json(status)))
}

pub async fn get_render(
    Path(job_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<crate::jobs::RenderJobStatus>, ApiError> {
    state
        .job_store
        .get_status(&job_id)
        .await
        .map_err(ApiError::from)?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("render job not found"))
}

pub async fn get_artifact(
    Path(job_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let job = state
        .job_store
        .get_status(&job_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("render job not found"))?;

    if job.state != RenderJobState::Completed {
        return Err(ApiError::bad_request("artifact is not ready yet"));
    }

    let artifact_key = job
        .artifact_key
        .as_deref()
        .ok_or_else(|| ApiError::not_found("artifact is missing"))?;

    let ObjectBlob {
        content_type,
        bytes,
    } = state
        .object_store
        .get(artifact_key)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("artifact object not found"))?;

    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = StatusCode::OK;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_str(&content_type)?);

    Ok(response)
}

pub async fn get_frame(
    Path((job_id, frame_index)): Path<(String, u64)>,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let job = state
        .job_store
        .get_record(&job_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("render job not found"))?;

    if job.status.state != RenderJobState::Completed {
        return Err(ApiError::bad_request("frame preview is not ready yet"));
    }

    let sequence: Sequence = serde_json::from_value(job.payload).map_err(ApiError::internal)?;
    let plan = compile_sequence(&sequence).map_err(|err| ApiError::bad_request(err.to_string()))?;

    if frame_index >= plan.total_frames {
        return Err(ApiError::bad_request("requested frame is out of range"));
    }

    let png = spawn_blocking(move || -> Result<Vec<u8>, ApiError> {
        let media = AssetMediaProvider::new(sequence.assets.clone(), plan.fps)
            .map_err(ApiError::internal)?;
        let mut renderer =
            Renderer::new(std::sync::Arc::new(plan), ServerFontManager::new(), media)
                .map_err(ApiError::internal)?;
        renderer
            .draw_frame(FrameIndex(frame_index))
            .map_err(ApiError::internal)?;
        renderer.encode_png().map_err(ApiError::internal)
    })
    .await
    .map_err(ApiError::internal)??;

    let mut response = Response::new(Body::from(png));
    *response.status_mut() = StatusCode::OK;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    Ok(response)
}

fn validate_sequence_limits(sequence: &Sequence, total_frames: u64) -> Result<(), ApiError> {
    if sequence.canvas.width > MAX_CANVAS_DIMENSION || sequence.canvas.height > MAX_CANVAS_DIMENSION
    {
        return Err(ApiError::bad_request(format!(
            "canvas dimensions exceed maximum {}",
            MAX_CANVAS_DIMENSION
        )));
    }

    if sequence.assets.len() > MAX_ASSETS {
        return Err(ApiError::bad_request(format!(
            "asset count exceeds maximum {}",
            MAX_ASSETS
        )));
    }

    if sequence.tracks.len() > MAX_TRACKS {
        return Err(ApiError::bad_request(format!(
            "track count exceeds maximum {}",
            MAX_TRACKS
        )));
    }

    let clip_count = sequence
        .tracks
        .iter()
        .map(|track| track.clips.len())
        .sum::<usize>();
    if clip_count > MAX_TOTAL_CLIPS {
        return Err(ApiError::bad_request(format!(
            "clip count exceeds maximum {}",
            MAX_TOTAL_CLIPS
        )));
    }

    if total_frames > MAX_TOTAL_FRAMES {
        return Err(ApiError::bad_request(format!(
            "timeline frame count exceeds maximum {}",
            MAX_TOTAL_FRAMES
        )));
    }

    Ok(())
}
