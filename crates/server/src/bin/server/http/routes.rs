use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State, ws::WebSocketUpgrade},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::IntoResponse,
};

use crate::server::{
    ApiRenderJob, CreateRenderResponse, GetRenderResponse, RenderJobError, RenderJobInput,
    RenderJobResponse, RenderProgressResponse, RenderQueueState, current_timestamp,
    execute_render_job, input_hash, new_render_id,
};

use super::{
    auth::authorize_response,
    errors::not_found_response,
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
    headers: HeaderMap,
    Json(mut input): Json<RenderJobInput>,
) -> impl IntoResponse {
    if let Some(response) = authorize_response(&state, &headers) {
        return response;
    }

    if input.job_id.is_none() {
        input.job_id = Some(new_render_id());
    }
    let id = input.job_id.clone().unwrap_or_else(new_render_id);
    let render = ApiRenderJob {
        cost_cents: 0,
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
        renders.insert(
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

    spawn_render_task(state.clone(), id.clone(), input);

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
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Some(response) = authorize_response(&state, &headers) {
        return response;
    }

    let render = state
        .renders
        .read()
        .ok()
        .and_then(|renders| renders.get(&id).map(|stored| stored.render.clone()));
    match render {
        Some(render) => (StatusCode::OK, Json(GetRenderResponse { render })).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(RenderJobResponse {
                ok: false,
                artifact: None,
                metrics: None,
                error: Some(RenderJobError {
                    code: "render_not_found".to_string(),
                    message: "render was not found".to_string(),
                    retryable: false,
                }),
            }),
        )
            .into_response(),
    }
}

pub(super) async fn get_render_progress(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Some(response) = authorize_response(&state, &headers) {
        return response;
    }

    let result = state.renders.read().ok().and_then(|renders| {
        renders.get(&id).map(|stored| RenderProgressResponse {
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
    headers: HeaderMap,
    Path(id): Path<String>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    if let Some(response) = authorize_response(&state, &headers) {
        return response;
    }

    ws.on_upgrade(move |socket| render_socket_session(socket, state, id))
}

pub(super) async fn get_render_artifact(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Some(response) = authorize_response(&state, &headers) {
        return response;
    }

    let bytes = state
        .renders
        .read()
        .ok()
        .and_then(|renders| renders.get(&id).and_then(|stored| stored.bytes.clone()));
    match bytes {
        Some(bytes) => {
            let mut headers = HeaderMap::new();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("video/mp4"));
            headers.insert(
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&format!("attachment; filename=\"{id}.mp4\""))
                    .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
            );
            (StatusCode::OK, headers, bytes.as_ref().clone()).into_response()
        }
        None => not_found_response(),
    }
}

fn spawn_render_task(state: AppState, id: String, input: RenderJobInput) {
    tokio::spawn(async move {
        let artifact_url = format!("/renders/{id}/artifact");
        update_render_progress(
            &state,
            &id,
            "progress",
            RenderProgressPatch {
                progress: Some(0.01),
                stage: Some("accepted"),
                state: Some("processing"),
                ..RenderProgressPatch::default()
            },
        );
        let progress_state = state.clone();
        let progress_id = id.clone();
        let result = execute_render_job(input, artifact_url, state.verbose_debug, move |event| {
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
        })
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
        && let Some(stored) = renders.get_mut(id)
    {
        stored.bytes = Some(Arc::new(completed.bytes));
        stored.render = completed.render;
        stored.render.status = "succeeded";
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
        && let Some(stored) = renders.get_mut(id)
    {
        stored.render.status = "failed";
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
