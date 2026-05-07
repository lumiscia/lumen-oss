use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub struct RenderJobInput {
    pub job_id: String,
    pub project: Value,
    #[serde(default)]
    pub media: HashMap<String, String>,
    #[serde(default)]
    pub render_profile: Option<RenderProfile>,
    #[serde(default)]
    pub artifact_staging: Option<ArtifactStaging>,
    #[serde(default)]
    pub progress_callback: Option<ProgressCallback>,
}

#[derive(Debug, Deserialize)]
pub struct RenderProfile {
    #[serde(default)]
    pub allowed_media_hosts: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ArtifactStaging {
    #[serde(default)]
    pub upload_url: Option<String>,
    #[serde(default)]
    pub download_url: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProgressCallback {
    pub url: String,
    #[serde(default)]
    pub token: Option<String>,
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
