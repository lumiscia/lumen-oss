use std::sync::Arc;

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::JsonRejection, ws::WebSocketUpgrade},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use lumen_engine::{composition::Composition, media::AssetResolver};
use lumen_ffmpeg::QualityRequest;
use lumen_server::render::{check_composition, resolve_encoder};

use crate::server::{
    ApiRenderJob, Artifact, CheckInput, CheckResponse, CreateRenderResponse, GetRenderResponse,
    RenderJob, RenderJobError, RenderJobInput, RenderProgressResponse, RenderQueueState,
    RenderRecord, RenderStatus, current_timestamp, execute_render_job, input_hash, new_render_id,
    sanitized_diagnostics, validate_media_manifest,
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
    // Resolving leases each asset, so the sweep keeps it until the render ends.
    let assets = Arc::new(state.assets.job_assets());
    let (input, composition) = tokio::task::spawn_blocking({
        let assets = Arc::clone(&assets);
        move || {
            let composition = load_composition(&input.composition, assets.as_ref());
            (input, composition)
        }
    })
    .await
    .map_err(|error| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "check_failed",
            format!("check worker join failed: {error}"),
        )
    })?;
    let composition = composition?;

    // Resolved now with the resolver the render uses, so a request it would fail is refused here.
    let invalid_quality = |message: String| {
        ApiError::rejected(
            StatusCode::UNPROCESSABLE_ENTITY,
            RenderJobError::invalid_quality(&message),
        )
    };
    let codec = input
        .codec
        .as_deref()
        .map(str::parse)
        .transpose()
        .map_err(invalid_quality)?;
    let request = QualityRequest {
        preset: input.quality,
        codec,
        ..QualityRequest::default()
    };
    let encoder =
        resolve_encoder(&state.quality, &composition, request).map_err(invalid_quality)?;

    let id = new_render_id();
    let record = state.insert(RenderRecord::new(
        id.clone(),
        current_timestamp(),
        input_hash(&input.composition, &input.media, &encoder),
    ));
    let render = ApiRenderJob::new(&record, &RenderStatus::Queued);

    spawn_render_task(
        state,
        record,
        RenderJob {
            id,
            composition,
            media: input.media,
            encoder,
            max_text_glyphs: state.max_text_glyphs,
            assets,
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

fn load_composition(
    value: &serde_json::Value,
    assets: &dyn AssetResolver,
) -> Result<Composition, ApiError> {
    let composition = lumen_engine::json::parse_value(value)
        .map_err(|error| ApiError::invalid_composition(&error))?;
    let asset_errors =
        lumen_engine::json::check_composition(&composition, value, None, Some(assets));
    if asset_errors.iter().any(|diagnostic| diagnostic.is_error()) {
        return Err(ApiError::invalid_diagnostics(&asset_errors));
    }
    Ok(composition)
}

/// What is wrong with a composition, without rendering it. Fonts resolve as a render's would,
/// counting the manifest's media, and `asset:` references resolve in the media store.
pub(super) async fn check_render(
    State(state): State<AppState>,
    input: Result<Json<CheckInput>, JsonRejection>,
) -> Result<Json<CheckResponse>, ApiError> {
    let Json(input) = input?;
    validate_media_manifest(&input.media)
        .map_err(|error| ApiError::rejected(StatusCode::UNPROCESSABLE_ENTITY, error))?;
    let assets = state.assets;
    let diagnostics = tokio::task::spawn_blocking(move || {
        check_composition(
            &input.composition,
            &|name| input.media.contains_key(name),
            &assets,
        )
    })
    .await
    .map_err(|error| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "check_failed",
            format!("check worker join failed: {error}"),
        )
    })?;
    Ok(Json(CheckResponse {
        ok: !diagnostics.iter().any(|diagnostic| diagnostic.is_error()),
        diagnostics: sanitized_diagnostics(&diagnostics),
    }))
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
