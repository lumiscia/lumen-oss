use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ErrorCode, RenderOutput, VideoBitDepth, VideoCodec};

/// The body of `POST /renders`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct RenderRequest {
    /// The composition document, as `composition.schema.json` describes it.
    #[cfg_attr(
        feature = "metadata",
        schemars(schema_with = "crate::external::composition")
    )]
    pub composition: Value,
    /// Remote media by the alias the composition uses, each an `http` or `https` URL.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub media: HashMap<String, String>,
    /// A URL to notify when the render ends. `lumen-server` delivers no webhooks and refuses it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook_url: Option<String>,
    /// A quality preset name. Every host has `draft`, `standard`, `high` and `lossless`, and its
    /// quality config can add others. Without one the host's default applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    /// Without one a MOV or WebM gets its own codec and an MP4 the host's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<VideoCodec>,
    /// Without one the codec's default applies: 8, or 10 for ProRes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bit_depth: Option<VideoBitDepth>,
    /// Keeps transparency as an alpha channel, in a MOV or WebM only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpha: Option<bool>,
    /// What to render: the format and the frames. Without it, an MP4 of the whole timeline.
    /// `quality`, `codec`, `bitDepth` and `alpha` apply to the video formats only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<RenderOutput>,
}

/// The body of `POST /renders/check`. A render request is one too.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct CheckRequest {
    /// The composition document, as `composition.schema.json` describes it.
    #[cfg_attr(
        feature = "metadata",
        schemars(schema_with = "crate::external::composition")
    )]
    pub composition: Value,
    /// The media manifest a render would stage, whose aliases count as present.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub media: HashMap<String, String>,
}

/// The answer to `POST /renders/check`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct CheckReport {
    /// Whether the composition has no errors. Warnings don't count.
    pub ok: bool,
    /// Everything wrong with the composition, each a `Diagnostic` of the runtime schema.
    #[cfg_attr(
        feature = "metadata",
        schemars(schema_with = "crate::external::diagnostics")
    )]
    pub diagnostics: Vec<Value>,
}

/// Where a render is. `lumen-server` sends these five; another host can send others, or not all
/// of them (the hosted API has no `cancelled`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "metadata", schemars(extend("x-lumen-open" = true)))]
#[serde(rename_all = "lowercase")]
pub enum RenderStatus {
    /// Waiting for a render slot.
    Queued,
    /// Staging its media or rendering.
    Processing,
    /// Finished, with an output to download.
    Succeeded,
    /// Ended with an error.
    Failed,
    /// Stopped on request.
    Cancelled,
}

impl RenderStatus {
    /// Whether the render has ended and won't change again.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

/// Why a render failed or stopped, kept with it and returned by every read of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct RenderFailure {
    pub code: ErrorCode,
    pub message: String,
    /// Whether the same render may succeed when tried again.
    pub retryable: bool,
    /// The media source that failed, when one did: the runtime schema's `MediaFailure`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "metadata",
        schemars(schema_with = "crate::external::media_failure")
    )]
    pub media: Option<Value>,
}

/// A render, as creating, reading, cancelling and polling it return it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Render {
    /// `r_` and 32 hex digits on `lumen-server`.
    pub id: String,
    pub status: RenderStatus,
    /// From 0 to 1. Rendering fills 0.05 to 0.99, and only success reaches 1.
    pub progress: f32,
    /// Where the output downloads from, once the render has succeeded. `lumen-server`'s is
    /// relative to the server: `/renders/{id}/artifact`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Why the render failed or stopped, when `status` is `failed` or `cancelled`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RenderFailure>,
    /// When the render was requested, as RFC 3339 in UTC.
    pub created_at: String,
    /// A hash of what was requested.
    pub input_hash: String,
    /// `self-hosted` on `lumen-server`.
    pub organization_id: String,
    /// What the render cost, in millionths of a US dollar. `lumen-server` reports 0.
    pub cost_micros: u64,
    /// The output's size in bytes, once the render has succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_bytes: Option<u64>,
    /// How long rendering took, in milliseconds, once the render has succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// The answer to `POST /renders`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct CreateRenderResponse {
    /// Whether an earlier render with the same input answered the request. Always `false` on
    /// `lumen-server`.
    pub cached: bool,
    pub render: Render,
}

/// The answer to `GET /renders/{id}`, `GET /renders/{id}/progress` and
/// `POST /renders/{id}/cancel`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct RenderResponse {
    pub render: Render,
}
