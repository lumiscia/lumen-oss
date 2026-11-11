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
use tokio::sync::OwnedSemaphorePermit;

use crate::server::{
    ApiRenderJob, Artifact, CheckInput, CheckResponse, CreateRenderResponse, GetRenderResponse,
    RenderJob, RenderJobError, RenderJobInput, RenderProgressResponse, RenderQueueState,
    RenderRecord, RenderStatus, current_timestamp, execute_render_job, input_hash, new_render_id,
    sanitized_diagnostics, validate_media_manifest,
};

use super::{
    errors::ApiError,
    state::{AppState, RenderTicket},
    ws::render_socket_session,
};

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
    // Held from here, so a refused request has taken nothing and an invalid one gives its place back.
    let ticket = state
        .reserve_render()
        .ok_or_else(|| ApiError::queue_full(state.max_queued_renders()))?;
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
    }
    .with_env_encoder();
    let encoder =
        resolve_encoder(&state.quality, &composition, request).map_err(invalid_quality)?;

    let id = new_render_id();
    let record = state.insert(RenderRecord::new(
        id.clone(),
        current_timestamp(),
        input_hash(&input.composition, &input.media, &encoder),
    ));
    let render = ApiRenderJob::new(&record, &RenderStatus::Queued);

    // A waiting render's place is held by its record, so cancelling it can give the place back.
    let slot = match ticket {
        RenderTicket::Running(permit) => Some(permit),
        RenderTicket::Waiting(spot) => {
            record.hold_queue_spot(spot);
            None
        }
    };
    let max_text_glyphs = state.max_text_glyphs;
    spawn_render_task(
        state,
        record,
        slot,
        RenderJob {
            id,
            composition,
            media: input.media,
            encoder,
            max_text_glyphs,
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
    let (assets, config) = (state.assets, state.render_config);
    let diagnostics = tokio::task::spawn_blocking(move || {
        check_composition(
            &input.composition,
            &|name| input.media.contains_key(name),
            &assets,
            &config,
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

/// Cancels a queued or running render. A queued render ends as `cancelled` before the response, so
/// it answers `200`; a running one answers `202` with its current status and ends at its next frame
/// boundary. A render that has already ended is returned as it is with `200`, so repeating the
/// request is harmless and a finished render keeps its result.
pub(super) async fn cancel_render(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<GetRenderResponse>), ApiError> {
    let record = state.get(&id).ok_or_else(ApiError::not_found)?;
    state.cancel(&record);
    let status = record.status.borrow().clone();
    let code = if status.is_terminal() {
        StatusCode::OK
    } else {
        StatusCode::ACCEPTED
    };
    Ok((
        code,
        Json(GetRenderResponse {
            render: ApiRenderJob::new(&record, &status),
        }),
    ))
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
        RenderStatus::Failed { error, .. } | RenderStatus::Cancelled { error, .. } => {
            Err(ApiError::rejected(StatusCode::CONFLICT, error))
        }
        _ => Err(ApiError::new(
            StatusCode::CONFLICT,
            "artifact_not_ready",
            "render has not finished",
        )),
    }
}

fn spawn_render_task(
    state: AppState,
    record: Arc<RenderRecord>,
    slot: Option<OwnedSemaphorePermit>,
    job: RenderJob,
) {
    tokio::spawn(async move {
        let _render_slot = match slot {
            Some(permit) => permit,
            None => {
                let permit = tokio::select! {
                    permit = Arc::clone(&state.render_slots).acquire_owned() => {
                        permit.expect("render slots are never closed")
                    }
                    () = record.cancellation.cancelled() => return,
                };
                record.release_queue_spot();
                permit
            }
        };
        // A render cancelled while queued has already ended.
        let started = record.status.send_if_modified(|status| {
            let queued = matches!(status, RenderStatus::Queued);
            if queued {
                *status = RenderStatus::Staging;
            }
            queued
        });
        if !started {
            return;
        }

        let progress_record = Arc::clone(&record);
        let min_delta = state.progress_min_delta;
        let mut woken_at = 0.0;
        let result = execute_render_job(
            job,
            &state.media_client,
            &state.render_config,
            &record.cancellation,
            move |event| {
                progress_record.set_frame(
                    event.frame,
                    event.total_frames,
                    min_delta,
                    &mut woken_at,
                );
            },
        )
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
