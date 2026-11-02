use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, ws::WebSocketUpgrade},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::IntoResponse,
};
use lumen_server::render::convert_project_payload;

use crate::server::{
    ApiRenderJob, CreateRenderResponse, GetRenderResponse, RenderJob, RenderJobError,
    RenderJobInput, RenderProgressResponse, RenderQueueState, current_timestamp,
    execute_render_job, input_hash, map_execution_error, new_render_id, validate_media_manifest,
};

use super::{
    errors::{error_response, not_found_response},
    progress::{RenderProgressPatch, broadcast_progress, update_render_progress},
    state::{AppState, StoredRender},
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
    Json(input): Json<RenderJobInput>,
) -> impl IntoResponse {
    if input.webhook_url.is_some() {
        return error_response(
            StatusCode::BAD_REQUEST,
            RenderJobError {
                code: "webhooks_unsupported".to_string(),
                message: "lumen-server does not deliver webhooks; remove webhookUrl and poll the render or use its socket".to_string(),
                retryable: false,
            },
        );
    }
    if let Err(error) = validate_media_manifest(&input.media) {
        return error_response(StatusCode::UNPROCESSABLE_ENTITY, error);
    }
    let bundle = match convert_project_payload(&input.composition) {
        Ok(bundle) => bundle,
        Err(error) => {
            return error_response(StatusCode::UNPROCESSABLE_ENTITY, map_execution_error(error));
        }
    };

    let id = new_render_id();
    let render = ApiRenderJob {
        cost_micros: 0,
        created_at: current_timestamp(),
        id: id.clone(),
        input_hash: input_hash(&input.composition, &input.media),
        organization_id: "self-hosted".to_string(),
        status: "queued",
    };
    let progress = RenderQueueState {
        artifact_url: None,
        duration_ms: None,
        error: None,
        organization_id: "self-hosted".to_string(),
        output_bytes: None,
        progress: 0.0,
        render_id: id.clone(),
        resolution: None,
        stage: Some("queued"),
        state: "queued",
        updated_at: current_timestamp(),
    };
    if let Ok(mut renders) = state.renders.write() {
        renders.records.insert(
            id.clone(),
            StoredRender {
                bytes: None,
                last_progress_broadcast: Some(progress.clone()),
                progress: Some(progress.clone()),
                render: render.clone(),
            },
        );
    }
    broadcast_progress(&state, "started", Some(progress));

    spawn_render_task(
        state.clone(),
        RenderJob {
            id,
            bundle,
            media: input.media,
        },
    );

    (
        StatusCode::ACCEPTED,
        Json(CreateRenderResponse {
            cached: false,
            render,
        }),
    )
        .into_response()
}

pub(super) async fn get_render(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let render = state
        .renders
        .read()
        .ok()
        .and_then(|renders| renders.records.get(&id).map(|stored| stored.render.clone()));
    match render {
        Some(render) => (StatusCode::OK, Json(GetRenderResponse { render })).into_response(),
        None => not_found_response(),
    }
}

pub(super) async fn get_render_progress(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let result = state.renders.read().ok().and_then(|renders| {
        renders
            .records
            .get(&id)
            .map(|stored| RenderProgressResponse {
                progress: stored.progress.clone(),
                render: stored.render.clone(),
            })
    });
    match result {
        Some(result) => (StatusCode::OK, Json(result)).into_response(),
        None => not_found_response(),
    }
}

pub(super) async fn render_socket(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let known = state
        .renders
        .read()
        .is_ok_and(|renders| renders.records.contains_key(&id));
    if !known {
        return not_found_response();
    }

    ws.on_upgrade(move |socket| render_socket_session(socket, state, id))
}

pub(super) async fn get_render_artifact(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let bytes = state.renders.read().ok().and_then(|renders| {
        renders
            .records
            .get(&id)
            .and_then(|stored| stored.bytes.clone())
    });
    match bytes {
        Some(bytes) => {
            let mut headers = HeaderMap::new();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("video/mp4"));
            headers.insert(
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&format!("attachment; filename=\"{id}.mp4\""))
                    .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
            );
            (StatusCode::OK, headers, bytes).into_response()
        }
        None => not_found_response(),
    }
}

fn spawn_render_task(state: AppState, job: RenderJob) {
    tokio::spawn(async move {
        let _render_slot = state
            .render_slots
            .acquire()
            .await
            .expect("render slots are never closed");
        let id = job.id.clone();
        let artifact_url = format!("/renders/{id}/artifact");
        update_render_progress(
            &state,
            &id,
            "started",
            RenderProgressPatch {
                progress: Some(0.01),
                stage: Some("accepted"),
                state: Some("processing"),
                ..RenderProgressPatch::default()
            },
        );
        let progress_state = state.clone();
        let progress_id = id.clone();
        let result = execute_render_job(
            job,
            &state.media_client,
            artifact_url,
            state.verbose_debug,
            move |event| {
                update_render_progress(
                    &progress_state,
                    &progress_id,
                    "progress",
                    RenderProgressPatch {
                        progress: Some((0.05 + 0.90 * event.ratio).clamp(0.0, 0.99)),
                        stage: Some(event.stage),
                        state: Some("processing"),
                        ..RenderProgressPatch::default()
                    },
                );
            },
        )
        .await;

        match result {
            Ok(completed) => complete_render(&state, &id, completed),
            Err(error) => fail_render(&state, &id, error),
        }
    });
}

fn complete_render(state: &AppState, id: &str, completed: crate::server::CompletedRender) {
    let artifact_url = completed.artifact.download_url.clone();
    let output_bytes = completed.artifact.bytes;
    let duration_ms = completed.metrics.render_ms;
    if let Ok(mut renders) = state.renders.write()
        && let Some(stored) = renders.records.get_mut(id)
    {
        stored.bytes = Some(Bytes::from(completed.bytes));
        stored.render.status = "succeeded";
        renders.retain_finished(id, state.retained_renders);
    }
    update_render_progress(
        state,
        id,
        "completed",
        RenderProgressPatch {
            artifact_url: Some(artifact_url),
            duration_ms: Some(duration_ms),
            output_bytes: Some(output_bytes),
            progress: Some(1.0),
            stage: Some("completed"),
            state: Some("succeeded"),
            ..RenderProgressPatch::default()
        },
    );
}

fn fail_render(state: &AppState, id: &str, error: RenderJobError) {
    if let Ok(mut renders) = state.renders.write()
        && let Some(stored) = renders.records.get_mut(id)
    {
        stored.render.status = "failed";
        renders.retain_finished(id, state.retained_renders);
    }
    update_render_progress(
        state,
        id,
        "completed",
        RenderProgressPatch {
            error: Some(error.message),
            progress: Some(1.0),
            stage: Some("failed"),
            state: Some("failed"),
            ..RenderProgressPatch::default()
        },
    );
}
