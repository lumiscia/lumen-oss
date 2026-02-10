use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    response::Response,
};
use serde_json::Value;

use crate::{
    api_error::ApiError,
    app_state::AppState,
    jobs::{ObjectBlob, RenderJobState},
};

pub async fn create_render(
    State(state): State<AppState>,
    Json(sequence): Json<Value>,
) -> Result<(StatusCode, Json<crate::jobs::RenderJobStatus>), ApiError> {
    let status = state
        .job_store
        .create(sequence)
        .await
        .map_err(ApiError::internal)?;

    state
        .job_queue
        .enqueue(status.job_id.clone())
        .await
        .map_err(ApiError::internal)?;

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
        .map_err(ApiError::internal)?
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
        .map_err(ApiError::internal)?
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
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found("artifact object not found"))?;

    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = StatusCode::OK;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_str(&content_type)?);

    Ok(response)
}

pub async fn get_frame(
    Path((job_id, _frame_index)): Path<(String, u64)>,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let frame_key = format!("jobs/{job_id}/frames/0");

    let frame = state
        .object_store
        .get(&frame_key)
        .await
        .map_err(ApiError::internal)?;

    match frame {
        Some(ObjectBlob {
            content_type,
            bytes,
        }) => {
            let mut response = Response::new(Body::from(bytes));
            *response.status_mut() = StatusCode::OK;
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_str(&content_type)?);
            Ok(response)
        }
        None => Err(ApiError::not_found("frame is not available")),
    }
}
