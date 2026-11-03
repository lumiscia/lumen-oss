use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::JsonRejection, ws::WebSocketUpgrade},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use lumen_server::render::parse_composition;

use crate::server::{
    ApiRenderJob, CreateRenderResponse, GetRenderResponse, RenderJob, RenderJobError,
    RenderJobInput, RenderProgressResponse, RenderQueueState, current_timestamp,
    execute_render_job, input_hash, map_execution_error, new_render_id, validate_media_manifest,
};

use super::{
    errors::ApiError,
    progress::{
        RenderProgressPatch, apply_render_progress, broadcast_progress, update_render_progress,
    },
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

    let id = new_render_id();
    let render = ApiRenderJob {
        cost_micros: 0,
        created_at: current_timestamp(),
        error: None,
        id: id.clone(),
        input_hash: input_hash(&input.composition, &input.media),
        organization_id: "self-hosted".to_string(),
        status: "queued",
    };
    let progress = RenderQueueState {
        artifact_url: None,
        duration_ms: None,
        error: None,
        error_code: None,
        organization_id: "self-hosted".to_string(),
        output_bytes: None,
        progress: 0.0,
        render_id: id.clone(),
        resolution: None,
        retryable: None,
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
            composition,
            media: input.media,
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
) -> impl IntoResponse {
    let render = state
        .renders
        .read()
        .ok()
        .and_then(|renders| renders.records.get(&id).map(|stored| stored.render.clone()));
    match render {
        Some(render) => (StatusCode::OK, Json(GetRenderResponse { render })).into_response(),
        None => ApiError::not_found().into_response(),
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
        None => ApiError::not_found().into_response(),
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
        return ApiError::not_found().into_response();
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
        None => ApiError::not_found().into_response(),
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
        let result = execute_render_job(job, &state.media_client, artifact_url, move |event| {
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
    let patch = RenderProgressPatch {
        artifact_url: Some(completed.artifact.download_url),
        duration_ms: Some(completed.metrics.render_ms),
        output_bytes: Some(completed.artifact.bytes),
        progress: Some(1.0),
        stage: Some("completed"),
        state: Some("succeeded"),
        ..RenderProgressPatch::default()
    };
    finish_render(
        state,
        id,
        "succeeded",
        Some(Bytes::from(completed.bytes)),
        patch,
    );
}

fn fail_render(state: &AppState, id: &str, error: RenderJobError) {
    let patch = RenderProgressPatch {
        failure: Some(error),
        progress: Some(1.0),
        stage: Some("failed"),
        state: Some("failed"),
        ..RenderProgressPatch::default()
    };
    finish_render(state, id, "failed", None, patch);
}

/// The terminal state and its event are captured under the lock that makes the render evictable, so
/// another render finishing right after can't drop the record before the event exists.
fn finish_render(
    state: &AppState,
    id: &str,
    status: &'static str,
    bytes: Option<Bytes>,
    patch: RenderProgressPatch,
) {
    let notification = state.renders.write().ok().and_then(|mut renders| {
        let stored = renders.records.get_mut(id)?;
        stored.bytes = bytes;
        stored.render.status = status;
        stored.render.error = patch.failure.clone();
        let notification = apply_render_progress(&mut renders, state, id, "completed", patch);
        renders.retain_finished(id, state.retained_renders);
        notification
    });
    if let Some(notification) = notification {
        broadcast_progress(state, notification.kind, notification.state);
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, sync::Barrier, thread};

    use clap::Parser;

    use super::*;
    use crate::server::http::Args;

    fn insert_render(state: &AppState, id: &str) {
        let render = ApiRenderJob {
            cost_micros: 0,
            created_at: String::new(),
            error: None,
            id: id.to_string(),
            input_hash: String::new(),
            organization_id: "self-hosted".to_string(),
            status: "queued",
        };
        state.renders.write().unwrap().records.insert(
            id.to_string(),
            StoredRender {
                bytes: None,
                last_progress_broadcast: None,
                progress: None,
                render,
            },
        );
    }

    #[test]
    fn renders_finishing_together_all_send_their_terminal_event() {
        let args = Args::parse_from(["lumen-server", "--retained-renders", "1"]);
        let state = AppState::new(args).unwrap();
        let mut events = state.progress_tx.subscribe();
        let failure = || RenderJobError {
            code: "render_failed".to_string(),
            message: "failed".to_string(),
            retryable: false,
        };

        for iteration in 0..5_000 {
            let ids = [format!("a{iteration}"), format!("b{iteration}")];
            insert_render(&state, &ids[0]);
            insert_render(&state, &ids[1]);
            let barrier = Barrier::new(2);
            thread::scope(|scope| {
                for id in &ids {
                    let (state, barrier) = (&state, &barrier);
                    scope.spawn(move || {
                        barrier.wait();
                        fail_render(state, id, failure());
                    });
                }
            });

            let mut finished = HashSet::new();
            while let Ok(event) = events.try_recv() {
                finished.insert(event.state.unwrap().render_id);
            }
            assert_eq!(finished.len(), 2, "a terminal event was lost: {finished:?}");
        }
    }
}
