use axum::extract::ws::{Message, WebSocket};
use futures_util::SinkExt;
use lumen_server_api::RenderEvent;
use tokio::sync::watch;

use crate::RenderStatus;

/// Sends the render's current status and then each change, and closes once it has ended.
pub(super) async fn render_socket_session(
    mut socket: WebSocket,
    mut status: watch::Receiver<RenderStatus>,
    id: String,
) {
    loop {
        let current = status.borrow_and_update().clone();
        let text =
            serde_json::to_string(&render_event(&id, &current)).expect("a render event serialises");
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

fn render_event(id: &str, status: &RenderStatus) -> RenderEvent {
    let render_id = id.to_string();
    match status {
        RenderStatus::Queued => RenderEvent::Queued { render_id },
        RenderStatus::Staging => RenderEvent::Started { render_id },
        RenderStatus::Rendering { .. } => RenderEvent::Progress {
            render_id,
            progress: status.progress(),
        },
        RenderStatus::Succeeded { artifact } => RenderEvent::Completed {
            render_id,
            url: artifact.url.clone(),
        },
        RenderStatus::Failed { error, .. } => RenderEvent::Failed {
            render_id,
            error: error.clone().into(),
        },
        RenderStatus::Cancelled { .. } => RenderEvent::Cancelled { render_id },
    }
}

#[cfg(test)]
mod tests {
    use lumen_server_api::ErrorCode;

    use super::*;
    use crate::RenderJobError;

    #[test]
    fn a_failed_render_event_carries_the_failure_code() {
        let event = render_event(
            "r_1",
            &RenderStatus::Failed {
                error: RenderJobError::new(
                    ErrorCode::MediaDownloadFailed,
                    "media 'hero': not found",
                    true,
                ),
                progress: 0.01,
            },
        );
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            serde_json::json!({
                "type": "render.failed",
                "renderId": "r_1",
                "error": {
                    "code": "media_download_failed",
                    "message": "media 'hero': not found",
                    "retryable": true,
                },
            })
        );
    }
}
