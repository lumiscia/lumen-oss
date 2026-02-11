use std::sync::Arc;

use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::Response,
};
use lumen::{compiler::compile_sequence, render::Renderer, sequence::Sequence, time::FrameIndex};
use serde::{Deserialize, Serialize};
use tokio::task::spawn_blocking;

use crate::{
    api_error::ApiError,
    app_state::AppState,
    jobs::{ObjectBlob, RenderJobState, RenderJobStatus},
    preview_cache::{CompiledPreview, PreviewCache},
    video::{ServerFontManager, media::AssetMediaProvider},
};

const MAX_ASSETS: usize = 512;
const MAX_TRACKS: usize = 128;
const MAX_TOTAL_CLIPS: usize = 4_096;
const MAX_TOTAL_FRAMES: u64 = 216_000;
const MAX_CANVAS_DIMENSION: u32 = 7680;
const DEFAULT_LIST_LIMIT: usize = 50;
const MAX_LIST_LIMIT: usize = 500;

#[derive(Debug, Deserialize)]
pub struct ListRendersQuery {
    pub state: Option<RenderJobState>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct ListRendersResponse {
    pub items: Vec<RenderJobStatus>,
}

pub async fn list_renders(
    Query(query): Query<ListRendersQuery>,
    State(state): State<AppState>,
) -> Result<Json<ListRendersResponse>, ApiError> {
    let limit = query
        .limit
        .unwrap_or(DEFAULT_LIST_LIMIT)
        .clamp(1, MAX_LIST_LIMIT);
    let offset = query.offset.unwrap_or(0);
    let items = if let Some(state_filter) = query.state {
        let mut items = state
            .job_store
            .list_statuses(usize::MAX, 0)
            .await
            .map_err(ApiError::from)?;
        items.retain(|item| item.state == state_filter);
        items.into_iter().skip(offset).take(limit).collect()
    } else {
        state
            .job_store
            .list_statuses(limit, offset)
            .await
            .map_err(ApiError::from)?
    };

    Ok(Json(ListRendersResponse { items }))
}

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

pub async fn cancel_render(
    Path(job_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<crate::jobs::RenderJobStatus>, ApiError> {
    let status = state
        .job_store
        .cancel(&job_id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(status))
}

pub async fn retry_render(
    Path(job_id): Path<String>,
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<crate::jobs::RenderJobStatus>), ApiError> {
    let status = state
        .job_store
        .retry(&job_id)
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

    let version = job.status.updated_at_ms;
    let frame_cache_key = PreviewCache::frame_key(&job_id, version, frame_index);
    if let Some(png) = state.preview_cache.get_frame(&frame_cache_key).await {
        return png_response(png);
    }

    let compiled_cache_key = PreviewCache::compiled_key(&job_id, version);
    let compiled = match state.preview_cache.get_compiled(&compiled_cache_key).await {
        Some(compiled) => compiled,
        None => {
            let sequence: Sequence =
                serde_json::from_value(job.payload).map_err(ApiError::internal)?;
            let plan = compile_sequence(&sequence)
                .map_err(|err| ApiError::bad_request(err.to_string()))?;
            let compiled = Arc::new(CompiledPreview {
                plan: Arc::new(plan),
                assets: sequence.assets,
            });
            state
                .preview_cache
                .put_compiled(compiled_cache_key, compiled.clone())
                .await;
            compiled
        }
    };

    if frame_index >= compiled.plan.total_frames {
        return Err(ApiError::bad_request("requested frame is out of range"));
    }

    let plan = compiled.plan.clone();
    let assets = compiled.assets.clone();
    let png = spawn_blocking(move || -> Result<Vec<u8>, ApiError> {
        let media = AssetMediaProvider::new(assets, plan.fps).map_err(ApiError::internal)?;
        let mut renderer =
            Renderer::new(plan, ServerFontManager::new(), media).map_err(ApiError::internal)?;
        renderer
            .draw_frame(FrameIndex(frame_index))
            .map_err(ApiError::internal)?;
        renderer.encode_png().map_err(ApiError::internal)
    })
    .await
    .map_err(ApiError::internal)??;

    let png = axum::body::Bytes::from(png);
    state
        .preview_cache
        .put_frame(frame_cache_key, png.clone())
        .await;

    png_response(png)
}

fn png_response(png: axum::body::Bytes) -> Result<Response, ApiError> {
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
