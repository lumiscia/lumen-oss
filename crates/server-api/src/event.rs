use serde::{Deserialize, Serialize};

use crate::RenderFailure;

/// A message on a render's socket, `GET /renders/{id}/socket`, named by its `type`. The socket
/// sends the render's current state when it opens, then each change, and closes after
/// `render.completed`, `render.failed` or `render.cancelled`. These are `lumen-server`'s events;
/// another host can send others.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum RenderEvent {
    /// The render is waiting for a slot.
    #[serde(rename = "render.queued")]
    Queued { render_id: String },
    /// The render has a slot and is staging its media.
    #[serde(rename = "render.started")]
    Started { render_id: String },
    /// The render is drawing frames.
    #[serde(rename = "render.progress")]
    Progress {
        render_id: String,
        /// From 0 to 1, as the render's `progress`.
        progress: f32,
    },
    /// The render succeeded.
    #[serde(rename = "render.completed")]
    Completed {
        render_id: String,
        /// Where the output downloads from, as the render's `url`.
        url: String,
    },
    /// The render failed.
    #[serde(rename = "render.failed")]
    Failed {
        render_id: String,
        error: RenderFailure,
    },
    /// The render was cancelled.
    #[serde(rename = "render.cancelled")]
    Cancelled { render_id: String },
}
