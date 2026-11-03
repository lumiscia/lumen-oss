use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub struct RenderJobInput {
    pub composition: Value,
    #[serde(default)]
    pub media: HashMap<String, String>,
    #[serde(default)]
    #[serde(rename = "webhookUrl")]
    pub webhook_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ArtifactOutput {
    pub download_url: String,
    pub content_type: &'static str,
    pub bytes: usize,
}

#[derive(Debug, Serialize)]
pub struct RenderJobMetrics {
    pub stage_ms: u128,
    pub render_ms: u128,
    pub total_frames: u64,
}

/// Why a render failed, kept with the render and returned by every read of it.
#[derive(Debug, Clone, Serialize)]
pub struct RenderJobError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiRenderJob {
    #[serde(rename = "costMicros")]
    pub cost_micros: u64,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RenderJobError>,
    pub id: String,
    #[serde(rename = "inputHash")]
    pub input_hash: String,
    #[serde(rename = "organizationId")]
    pub organization_id: String,
    pub status: &'static str,
}

#[derive(Debug, Serialize)]
pub struct CreateRenderResponse {
    pub cached: bool,
    pub render: ApiRenderJob,
}

#[derive(Debug, Serialize)]
pub struct GetRenderResponse {
    pub render: ApiRenderJob,
}

#[derive(Debug, Clone, Serialize)]
pub struct RenderQueueState {
    #[serde(rename = "artifactUrl", skip_serializing_if = "Option::is_none")]
    pub artifact_url: Option<String>,
    #[serde(rename = "durationMs", skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(rename = "errorCode", skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(rename = "organizationId")]
    pub organization_id: String,
    #[serde(rename = "outputBytes", skip_serializing_if = "Option::is_none")]
    pub output_bytes: Option<usize>,
    pub progress: f32,
    #[serde(rename = "renderId")]
    pub render_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<&'static str>,
    pub state: &'static str,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
pub struct RenderProgressResponse {
    pub progress: Option<RenderQueueState>,
    pub render: ApiRenderJob,
}

#[derive(Debug, Clone, Serialize)]
pub struct RenderNotification {
    pub state: Option<RenderQueueState>,
    #[serde(rename = "type")]
    pub kind: &'static str,
}
