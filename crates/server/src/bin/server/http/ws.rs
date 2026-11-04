use axum::extract::ws::{Message, WebSocket};
use futures_util::SinkExt;
use tokio::sync::watch;

use crate::server::RenderStatus;

/// Sends the render's current status and then each change, and closes once it has ended.
pub(super) async fn render_socket_session(
    mut socket: WebSocket,
    mut status: watch::Receiver<RenderStatus>,
    id: String,
) {
    loop {
        let current = status.borrow_and_update().clone();
        let text = render_event(&id, &current).to_string();
        if socket.send(Message::Text(text.into())).await.is_err() {
            return;
        }
        if current.is_terminal() {
            let _ = socket.close().await;
            return;
        }

        loop {
            tokio::select! {
                message = socket.recv() => match message {
                    Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                    Some(Ok(_)) => {}
                },
                changed = status.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    break;
                }
            }
        }
    }
}

fn render_event(id: &str, status: &RenderStatus) -> serde_json::Value {
    match status {
        RenderStatus::Queued => serde_json::json!({
            "type": "render.queued",
            "renderId": id,
        }),
        RenderStatus::Staging => serde_json::json!({
            "type": "render.started",
            "renderId": id,
        }),
        RenderStatus::Rendering { .. } => serde_json::json!({
            "type": "render.progress",
            "renderId": id,
            "progress": status.progress(),
        }),
        RenderStatus::Succeeded { artifact } => serde_json::json!({
            "type": "render.completed",
            "renderId": id,
            "url": artifact.url,
        }),
        RenderStatus::Failed { error, .. } => serde_json::json!({
            "type": "render.failed",
            "renderId": id,
            "error": {
                "code": error.code,
                "message": error.message,
                "retryable": error.retryable,
            },
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::RenderJobError;

    #[test]
    fn a_failed_render_event_carries_the_failure_code() {
        let event = render_event(
            "r_1",
            &RenderStatus::Failed {
                error: RenderJobError {
                    code: "media_download_failed".to_string(),
                    message: "media 'hero': not found".to_string(),
                    retryable: true,
                },
                progress: 0.01,
            },
        );
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
