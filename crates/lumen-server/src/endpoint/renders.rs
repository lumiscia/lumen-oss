use std::sync::Arc;
use std::{convert::Infallible, time::Duration};

use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{
        Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use lumen::{Project, compile_project};
use serde::{Deserialize, Serialize};
use tokio::task::spawn_blocking;

use crate::{
    api_error::ApiError,
    app_state::AppState,
    jobs::{ObjectBlob, RenderJobState, RenderJobStatus},
    preview_cache::{CompiledPreview, PreviewCache},
    video::FfmpegRenderBackend,
};

const MAX_SOURCES: usize = 512;
const MAX_LAYERS: usize = 256;
const MAX_TOTAL_CLIPS: usize = 8_192;
const MAX_TOTAL_FRAMES: u64 = 216_000;
const MAX_CANVAS_DIMENSION: u32 = 7_680;
const DEFAULT_LIST_LIMIT: usize = 50;
const MAX_LIST_LIMIT: usize = 500;
const DEFAULT_EVENTS_INTERVAL_MS: u64 = 250;
const MIN_EVENTS_INTERVAL_MS: u64 = 100;
const MAX_EVENTS_INTERVAL_MS: u64 = 2_000;

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

#[derive(Debug, Deserialize)]
pub struct RenderEventsQuery {
    pub interval_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct RenderProgressEvent {
    pub job_id: String,
    pub state: RenderJobState,
    pub progress: f32,
    pub percentage: u8,
    pub stage: Option<String>,
    pub updated_at_ms: u64,
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
    Json(project): Json<Project>,
) -> Result<(StatusCode, Json<crate::jobs::RenderJobStatus>), ApiError> {
    let timeline =
        compile_project(&project).map_err(|err| ApiError::bad_request(err.to_string()))?;
    validate_project_limits(&project, timeline.total_frames())?;

    let payload = serde_json::to_value(project).map_err(ApiError::internal)?;

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

pub async fn stream_render_events(
    Path(job_id): Path<String>,
    Query(query): Query<RenderEventsQuery>,
    State(state): State<AppState>,
) -> Result<Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    if state
        .job_store
        .get_status(&job_id)
        .await
        .map_err(ApiError::from)?
        .is_none()
    {
        return Err(ApiError::not_found("render job not found"));
    }

    let interval_ms = query
        .interval_ms
        .unwrap_or(DEFAULT_EVENTS_INTERVAL_MS)
        .clamp(MIN_EVENTS_INTERVAL_MS, MAX_EVENTS_INTERVAL_MS);

    let stream_state = state.clone();
    let stream_job_id = job_id.clone();
    let stream = async_stream::stream! {
        let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_updated_at: Option<u64> = None;

        loop {
            interval.tick().await;

            let status = match stream_state.job_store.get_status(&stream_job_id).await {
                Ok(Some(status)) => status,
                Ok(None) => break,
                Err(_) => break,
            };

            let should_emit = last_updated_at
                .map(|updated_at| updated_at != status.updated_at_ms)
                .unwrap_or(true);
            if should_emit {
                let payload = to_progress_event(&status);
                let data = match serde_json::to_string(&payload) {
                    Ok(data) => data,
                    Err(_) => break,
                };
                last_updated_at = Some(status.updated_at_ms);

                yield Ok(Event::default()
                    .event("progress")
                    .id(status.updated_at_ms.to_string())
                    .data(data));
            }

            if is_terminal(status.state) {
                break;
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(10))
            .text("keep-alive"),
    ))
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
            let project: Project =
                serde_json::from_value(job.payload).map_err(ApiError::internal)?;
            let timeline =
                compile_project(&project).map_err(|err| ApiError::bad_request(err.to_string()))?;
            let compiled = Arc::new(CompiledPreview {
                timeline: Arc::new(timeline),
            });
            state
                .preview_cache
                .put_compiled(compiled_cache_key, compiled.clone())
                .await;
            compiled
        }
    };

    if frame_index >= compiled.timeline.total_frames() {
        return Err(ApiError::bad_request("requested frame is out of range"));
    }

    let timeline = compiled.timeline.clone();
    let png = spawn_blocking(move || -> Result<Vec<u8>, ApiError> {
        let backend = FfmpegRenderBackend::new(timeline);
        backend
            .render_frame_png(frame_index)
            .map_err(ApiError::internal)
    })
    .await
    .map_err(ApiError::internal)??;

    let png_bytes = axum::body::Bytes::from(png);
    state
        .preview_cache
        .put_frame(frame_cache_key, png_bytes.clone())
        .await;

    png_response(png_bytes)
}

fn validate_project_limits(project: &Project, total_frames: u64) -> Result<(), ApiError> {
    if project.sources.len() > MAX_SOURCES {
        return Err(ApiError::bad_request(format!(
            "project has {} sources, limit is {MAX_SOURCES}",
            project.sources.len()
        )));
    }

    if project.layers.len() > MAX_LAYERS {
        return Err(ApiError::bad_request(format!(
            "project has {} layers, limit is {MAX_LAYERS}",
            project.layers.len()
        )));
    }

    let total_clips: usize = project.layers.iter().map(|layer| layer.clips.len()).sum();
    if total_clips > MAX_TOTAL_CLIPS {
        return Err(ApiError::bad_request(format!(
            "project has {total_clips} clips, limit is {MAX_TOTAL_CLIPS}"
        )));
    }

    if total_frames > MAX_TOTAL_FRAMES {
        return Err(ApiError::bad_request(format!(
            "timeline resolves to {total_frames} frames, limit is {MAX_TOTAL_FRAMES}"
        )));
    }

    if project.canvas.width > MAX_CANVAS_DIMENSION || project.canvas.height > MAX_CANVAS_DIMENSION {
        return Err(ApiError::bad_request(format!(
            "canvas dimensions {}x{} exceed limit {MAX_CANVAS_DIMENSION}",
            project.canvas.width, project.canvas.height
        )));
    }

    Ok(())
}

fn to_progress_event(status: &RenderJobStatus) -> RenderProgressEvent {
    let progress = status.progress.unwrap_or(0.0).clamp(0.0, 1.0);
    let percentage = (progress * 100.0).round() as u8;

    RenderProgressEvent {
        job_id: status.job_id.clone(),
        state: status.state,
        progress,
        percentage,
        stage: status.stage.clone(),
        updated_at_ms: status.updated_at_ms,
    }
}

fn is_terminal(state: RenderJobState) -> bool {
    matches!(
        state,
        RenderJobState::Completed | RenderJobState::Failed | RenderJobState::Canceled
    )
}

fn png_response(bytes: axum::body::Bytes) -> Result<Response, ApiError> {
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = StatusCode::OK;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));

    Ok(response)
}
