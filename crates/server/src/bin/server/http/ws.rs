use axum::extract::ws::{Message, WebSocket};
use futures_util::SinkExt;

use crate::server::RenderNotification;

use super::state::AppState;

pub(super) async fn render_socket_session(mut socket: WebSocket, state: AppState, id: String) {
    let snapshot = state.renders.read().ok().and_then(|renders| {
        renders
            .records
            .get(&id)
            .and_then(|stored| stored.progress.clone())
    });
    let _ = send_notification(
        &mut socket,
        &RenderNotification {
            state: snapshot,
            kind: "snapshot",
        },
    )
    .await;

    let mut rx = state.progress_tx.subscribe();
    loop {
        tokio::select! {
            message = socket.recv() => {
                match message {
                    Some(Ok(Message::Close(_))) => {
                        let _ = socket.close().await;
                        break;
                    }
                    None => break,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
            notification = rx.recv() => {
                match notification {
                    Ok(notification)
                        if notification
                            .state
                            .as_ref()
                            .is_some_and(|progress| progress.render_id == id) =>
                    {
                        if send_notification(&mut socket, &notification).await.is_err() {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

async fn send_notification(
    socket: &mut WebSocket,
    notification: &RenderNotification,
) -> Result<(), axum::Error> {
    let Some(event) = render_event(notification) else {
        return Ok(());
    };
    let Ok(text) = serde_json::to_string(&event) else {
        return Ok(());
    };
    socket.send(Message::Text(text.into())).await
}

fn render_event(notification: &RenderNotification) -> Option<serde_json::Value> {
    let Some(state) = notification.state.as_ref() else {
        return None;
    };

    Some(match state.state {
        "queued" => serde_json::json!({
            "type": "render.queued",
            "renderId": state.render_id,
        }),
        "succeeded" => serde_json::json!({
            "type": "render.completed",
            "renderId": state.render_id,
            "url": state.artifact_url,
        }),
        "failed" => serde_json::json!({
            "type": "render.failed",
            "renderId": state.render_id,
            "error": {
                "code": state.error_code.as_deref().unwrap_or("render_failed"),
                "message": state.error.as_deref().unwrap_or("Render failed."),
                "retryable": state.retryable.unwrap_or(false),
            },
        }),
        "processing" if notification.kind == "started" => serde_json::json!({
            "type": "render.started",
            "renderId": state.render_id,
        }),
        _ => serde_json::json!({
            "type": "render.progress",
            "renderId": state.render_id,
            "progress": state.progress,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::RenderQueueState;

    #[test]
    fn a_failed_render_event_carries_the_failure_code() {
        let event = render_event(&RenderNotification {
            kind: "completed",
            state: Some(RenderQueueState {
                artifact_url: None,
                duration_ms: None,
                error: Some("media 'hero': not found".to_string()),
                error_code: Some("media_download_failed".to_string()),
                organization_id: String::new(),
                output_bytes: None,
                progress: 1.0,
                render_id: "r_1".to_string(),
                resolution: None,
                retryable: Some(true),
                stage: None,
                state: "failed",
                updated_at: String::new(),
            }),
        })
        .unwrap();
        assert_eq!(
            event["error"],
            serde_json::json!({
                "code": "media_download_failed",
                "message": "media 'hero': not found",
                "retryable": true,
            })
        );
    }
}
