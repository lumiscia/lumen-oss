use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicUsize, Ordering},
};

use axum::body::Bytes;
use lumen_engine::render::{self, CancelToken};
use lumen_server_api::{self as api, ErrorCode as ApiErrorCode};
use serde_json::Value;
use tokio::sync::{Notify, watch};

use super::util::{MAX_ERROR_MESSAGE_CHARS, sanitize_error_message};

/// Why a render failed, kept with the render and returned by every read of it.
#[derive(Debug, Clone)]
pub struct RenderJobError {
    pub code: ApiErrorCode,
    pub message: String,
    pub retryable: bool,
    /// The engine's `MediaFailure`, when a media source failed.
    pub media: Option<Value>,
}

/// `diagnostics` with messages and paths bounded like every other error sent.
pub fn sanitized_diagnostics(diagnostics: &[lumen_engine::diagnostic::Diagnostic]) -> Vec<Value> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            let bounded = lumen_engine::diagnostic::Diagnostic {
                path: bounded_pointer(&diagnostic.path),
                message: sanitize_error_message(&diagnostic.message),
                ..diagnostic.clone()
            };
            serde_json::to_value(bounded).expect("a diagnostic serialises")
        })
        .collect()
}

/// `pointer`, or past the error length limit the longest ancestor of it that fits, so every token
/// it keeps is whole.
fn bounded_pointer(pointer: &str) -> String {
    if pointer.chars().count() <= MAX_ERROR_MESSAGE_CHARS {
        return pointer.to_string();
    }
    let end = pointer
        .char_indices()
        .nth(MAX_ERROR_MESSAGE_CHARS)
        .map_or(pointer.len(), |(index, _)| index);
    let cut = if pointer[end..].starts_with('/') {
        end
    } else {
        pointer[..end].rfind('/').unwrap_or(0)
    };
    pointer[..cut].to_string()
}

impl RenderJobError {
    /// A failure with no media failure, its message bounded.
    pub fn new(code: ApiErrorCode, message: &str, retryable: bool) -> Self {
        Self {
            code,
            message: sanitize_error_message(message),
            retryable,
            media: None,
        }
    }

    pub fn invalid_composition(message: &str) -> Self {
        Self::new(ApiErrorCode::InvalidProjectPayload, message, false)
    }

    pub fn cancelled() -> Self {
        Self::new(
            ApiErrorCode::RenderCancelled,
            "the render was cancelled",
            false,
        )
    }

    pub fn is_cancelled(&self) -> bool {
        self.code == ApiErrorCode::RenderCancelled
    }

    /// A render library error that refused the request, such as an output that can't be rendered.
    pub fn refused(error: &render::Error) -> Self {
        Self::new(
            crate::wire::error_code(error.code()),
            &error.to_string(),
            false,
        )
    }
}

impl From<RenderJobError> for api::RenderFailure {
    fn from(error: RenderJobError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            retryable: error.retryable,
            media: error.media,
        }
    }
}

/// Asks one render to stop. The render library checks the token between frames, and the tasks
/// around it wait on [`Cancellation::cancelled`].
#[derive(Default)]
pub struct Cancellation {
    token: CancelToken,
    notify: Notify,
}

impl Cancellation {
    pub fn cancel(&self) {
        self.token.cancel();
        self.notify.notify_waiters();
    }

    pub fn token(&self) -> CancelToken {
        self.token.clone()
    }

    /// Completes once [`cancel`](Self::cancel) has been called, immediately if it already was.
    pub async fn cancelled(&self) {
        loop {
            let notified = self.notify.notified();
            if self.token.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

/// One place in the queue for a render slot, given back when dropped.
pub struct QueueSpot(Arc<AtomicUsize>);

impl QueueSpot {
    /// Takes a place the caller has already counted in `queued`.
    pub fn new(queued: Arc<AtomicUsize>) -> Self {
        Self(queued)
    }
}

impl Drop for QueueSpot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A render and its latest status. Sockets subscribe to the status and polling reads it, so every
/// reader sees the same value.
pub struct RenderRecord {
    pub id: String,
    pub created_at: String,
    pub input_hash: String,
    pub status: watch::Sender<RenderStatus>,
    pub cancellation: Cancellation,
    queue_spot: Mutex<Option<QueueSpot>>,
}

#[derive(Debug, Clone)]
pub enum RenderStatus {
    Queued,
    Staging,
    Rendering {
        frame: u32,
        total: u32,
    },
    Succeeded {
        artifact: Artifact,
    },
    Failed {
        error: RenderJobError,
        progress: f32,
    },
    /// Stopped on request. `error` holds the `render_cancelled` code.
    Cancelled {
        error: RenderJobError,
        progress: f32,
    },
}

#[derive(Debug, Clone)]
pub struct Artifact {
    /// Where the client downloads it from, relative to the server.
    pub url: String,
    pub bytes: Bytes,
    pub content_type: &'static str,
    /// The file extension, without the dot, that the download is named with.
    pub extension: &'static str,
    pub render_ms: u64,
}

impl RenderStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Succeeded { .. } | Self::Failed { .. } | Self::Cancelled { .. }
        )
    }

    /// `0` to `1`. Rendering fills `0.05` to `0.99`, and only success reaches `1`.
    pub fn progress(&self) -> f32 {
        match self {
            Self::Queued => 0.0,
            Self::Staging => 0.01,
            Self::Rendering { frame, total } => {
                let ratio = *frame as f32 / (*total).max(1) as f32;
                (0.05 + 0.90 * ratio).clamp(0.0, 0.99)
            }
            Self::Succeeded { .. } => 1.0,
            Self::Failed { progress, .. } | Self::Cancelled { progress, .. } => *progress,
        }
    }

    pub fn status(&self) -> api::RenderStatus {
        match self {
            Self::Queued => api::RenderStatus::Queued,
            Self::Staging | Self::Rendering { .. } => api::RenderStatus::Processing,
            Self::Succeeded { .. } => api::RenderStatus::Succeeded,
            Self::Failed { .. } => api::RenderStatus::Failed,
            Self::Cancelled { .. } => api::RenderStatus::Cancelled,
        }
    }

    fn error(&self) -> Option<&RenderJobError> {
        match self {
            Self::Failed { error, .. } | Self::Cancelled { error, .. } => Some(error),
            _ => None,
        }
    }

    fn artifact(&self) -> Option<&Artifact> {
        match self {
            Self::Succeeded { artifact } => Some(artifact),
            _ => None,
        }
    }
}

impl RenderRecord {
    pub fn new(id: String, created_at: String, input_hash: String) -> Self {
        Self {
            id,
            created_at,
            input_hash,
            status: watch::Sender::new(RenderStatus::Queued),
            cancellation: Cancellation::default(),
            queue_spot: Mutex::new(None),
        }
    }

    /// Keeps the render's place in the queue until [`release_queue_spot`](Self::release_queue_spot)
    /// or the record is dropped.
    pub fn hold_queue_spot(&self, spot: QueueSpot) {
        *self
            .queue_spot
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(spot);
    }

    /// Gives the place back. Releasing again, or without a place, does nothing.
    pub fn release_queue_spot(&self) {
        drop(
            self.queue_spot
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take(),
        );
    }

    /// Records a rendered frame. Subscribers are woken when the stage changes or the progress has
    /// grown by `min_delta` since the last time they were; the stored status is always current.
    pub fn set_frame(&self, frame: u32, total: u32, min_delta: f32, woken_at: &mut f32) {
        let next = RenderStatus::Rendering { frame, total };
        let progress = next.progress();
        self.status.send_if_modified(|status| {
            let wake = !matches!(status, RenderStatus::Rendering { .. })
                || progress - *woken_at >= min_delta;
            *status = next;
            if wake {
                *woken_at = progress;
            }
            wake
        });
    }
}

impl RenderRecord {
    /// The render as the API returns it, at `status`.
    pub fn render(&self, status: &RenderStatus) -> api::Render {
        let artifact = status.artifact();
        api::Render {
            id: self.id.clone(),
            status: status.status(),
            progress: status.progress(),
            url: artifact.map(|artifact| artifact.url.clone()),
            error: status.error().cloned().map(api::RenderFailure::from),
            created_at: self.created_at.clone(),
            input_hash: self.input_hash.clone(),
            organization_id: "self-hosted".to_string(),
            cost_micros: 0,
            output_bytes: artifact.map(|artifact| artifact.bytes.len() as u64),
            duration_ms: artifact.map(|artifact| artifact.render_ms),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_pointers_are_cut_back_to_a_whole_ancestor() {
        assert_eq!(bounded_pointer("/nodes/0/params/a "), "/nodes/0/params/a ");
        let long = format!("/nodes/0/params/{}", "k".repeat(MAX_ERROR_MESSAGE_CHARS));
        assert_eq!(bounded_pointer(&long), "/nodes/0/params");
        let exact = format!("/{}/b", "a".repeat(MAX_ERROR_MESSAGE_CHARS - 1));
        assert_eq!(
            bounded_pointer(&exact),
            format!("/{}", "a".repeat(MAX_ERROR_MESSAGE_CHARS - 1))
        );
    }
}
