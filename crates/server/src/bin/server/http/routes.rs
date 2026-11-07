use std::sync::Arc;

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::JsonRejection, ws::WebSocketUpgrade},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use lumen_server::render::parse_composition;

use crate::server::{
    ApiRenderJob, Artifact, CreateRenderResponse, GetRenderResponse, QualityInput, RenderJob,
    RenderJobInput, RenderProgressResponse, RenderQueueState, RenderRecord, RenderStatus,
    current_timestamp, execute_render_job, input_hash, map_execution_error, new_render_id,
    validate_media_manifest,
};

use super::{errors::ApiError, state::AppState, ws::render_socket_session};

#[derive(Debug, serde::Serialize)]
pub(super) struct HealthResponse {
    ok: bool,
    service: &'static str,
}

pub(super) async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        service: "lumen-server",
    })
}

pub(super) async fn create_render(
    State(state): State<AppState>,
    input: Result<Json<RenderJobInput>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(input) = input?;
    if input.webhook_url.is_some() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "webhooks_unsupported",
            "lumen-server does not deliver webhooks; remove webhookUrl and poll the render or use its socket",
        ));
    }
    validate_media_manifest(&input.media)
        .map_err(|error| ApiError::rejected(StatusCode::UNPROCESSABLE_ENTITY, error))?;
    let composition = parse_composition(&input.composition).map_err(|error| {
        ApiError::rejected(StatusCode::UNPROCESSABLE_ENTITY, map_execution_error(error))
    })?;

    let quality = input
        .quality
        .map(QualityInput::into_quality)
        .transpose()
        .map_err(|error| ApiError::rejected(StatusCode::UNPROCESSABLE_ENTITY, error))?
        .flatten();

    let id = new_render_id();
    let record = state.insert(RenderRecord::new(
        id.clone(),
        current_timestamp(),
        input_hash(&input.composition, &input.media, quality.as_ref()),
    ));
    let render = ApiRenderJob::new(&record, &RenderStatus::Queued);

    spawn_render_task(
        state,
        record,
        RenderJob {
            id,
            composition,
            media: input.media,
            quality: quality.unwrap_or_default(),
        },
    );

    Ok((
        StatusCode::ACCEPTED,
        Json(CreateRenderResponse {
            cached: false,
            render,
        }),
    )
        .into_response())
}

pub(super) async fn get_render(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<GetRenderResponse>, ApiError> {
    let record = state.get(&id).ok_or_else(ApiError::not_found)?;
    let status = record.status.borrow().clone();
    Ok(Json(GetRenderResponse {
        render: ApiRenderJob::new(&record, &status),
    }))
}

pub(super) async fn get_render_progress(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RenderProgressResponse>, ApiError> {
    let record = state.get(&id).ok_or_else(ApiError::not_found)?;
    let status = record.status.borrow().clone();
    Ok(Json(RenderProgressResponse {
        progress: RenderQueueState::new(&id, &status),
        render: ApiRenderJob::new(&record, &status),
    }))
}

pub(super) async fn render_socket(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let record = state.get(&id).ok_or_else(ApiError::not_found)?;
    let status = record.status.subscribe();
    Ok(ws.on_upgrade(move |socket| render_socket_session(socket, status, id)))
}

pub(super) async fn get_render_artifact(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let record = state.get(&id).ok_or_else(ApiError::not_found)?;
    let status = record.status.borrow().clone();
    match status {
        RenderStatus::Succeeded { artifact } => {
            let mut headers = HeaderMap::new();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("video/mp4"));
            headers.insert(
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&format!("attachment; filename=\"{id}.mp4\""))
                    .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
            );
            Ok((StatusCode::OK, headers, artifact.bytes).into_response())
        }
        RenderStatus::Failed { error, .. } => Err(ApiError::rejected(StatusCode::CONFLICT, error)),
        _ => Err(ApiError::new(
            StatusCode::CONFLICT,
            "artifact_not_ready",
            "render has not finished",
        )),
    }
}

fn spawn_render_task(state: AppState, record: Arc<RenderRecord>, job: RenderJob) {
    tokio::spawn(async move {
        let _render_slot = state
            .render_slots
            .acquire()
            .await
            .expect("render slots are never closed");
        record.status.send_replace(RenderStatus::Staging);

        let progress_record = Arc::clone(&record);
        let min_delta = state.progress_min_delta;
        let mut woken_at = 0.0;
        let result = execute_render_job(job, &state.media_client, move |event| {
            progress_record.set_frame(event.frame, event.total_frames, min_delta, &mut woken_at);
        })
        .await;

        match result {
            Ok(completed) => state.finish(
                &record,
                RenderStatus::Succeeded {
                    artifact: Artifact {
                        url: format!("/renders/{}/artifact", record.id),
                        bytes: Bytes::from(completed.bytes),
                        render_ms: completed.render_ms,
                    },
                },
            ),
            Err(error) => state.fail(&record, error),
        }
    });
}
