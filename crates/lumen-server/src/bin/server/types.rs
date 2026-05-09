use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub struct RenderJobInput {
    #[serde(default)]
    pub job_id: Option<String>,
    #[serde(alias = "project")]
    pub composition: Value,
    #[serde(default)]
    pub media: HashMap<String, String>,
    #[serde(default)]
    #[serde(rename = "webhookUrl")]
    pub webhook_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RenderJobResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<ArtifactOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<RenderJobMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RenderJobError>,
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

#[derive(Debug, Serialize)]
pub struct RenderJobError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiRenderJob {
    #[serde(rename = "costCents")]
    pub cost_cents: u32,
    #[serde(rename = "createdAt")]
    pub created_at: String,
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
