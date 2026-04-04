use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::render::{
    RenderError as RenderPipelineError, RenderOptions, RenderProgress, convert_project_payload,
    render_project_mp4,
};

#[derive(Debug, Deserialize)]
pub struct RunpodJobRequest {
    pub input: RunpodRenderInput,
}

#[derive(Debug, Deserialize)]
pub struct RunpodRenderInput {
    pub job_id: String,
    pub project: Value,
    #[serde(default)]
    pub render_profile: Option<RunpodRenderProfile>,
    #[serde(default)]
    pub artifact_staging: Option<RunpodArtifactStaging>,
}

#[derive(Debug, Deserialize)]
pub struct RunpodRenderProfile {
    #[serde(default)]
    pub encoder: Option<String>,
    #[serde(default)]
    pub media_root: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RunpodArtifactStaging {
    #[serde(default)]
    pub upload_url: Option<String>,
    #[serde(default)]
    pub download_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RunpodJobResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<RunpodArtifactOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<RunpodRenderMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RunpodRenderError>,
}

#[derive(Debug, Serialize)]
pub struct RunpodArtifactOutput {
    pub download_url: String,
    pub content_type: &'static str,
    pub bytes: usize,
}

#[derive(Debug, Serialize)]
pub struct RunpodRenderMetrics {
    pub compile_ms: u128,
    pub render_ms: u128,
    pub total_frames: u64,
}

#[derive(Debug, Serialize)]
pub struct RunpodRenderError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

pub async fn handle_runpod_request(request: RunpodJobRequest) -> RunpodJobResponse {
    match execute_request(request).await {
        Ok(response) => response,
        Err(error) => RunpodJobResponse {
            ok: false,
            artifact: None,
            metrics: None,
            error: Some(error),
        },
    }
}

async fn execute_request(
    request: RunpodJobRequest,
) -> Result<RunpodJobResponse, RunpodRenderError> {
    let bundle = convert_project_payload(&request.input.project).map_err(map_execution_error)?;

    let options = RenderOptions {
        media_root: request
            .input
            .render_profile
            .as_ref()
            .and_then(|profile| profile.media_root.as_ref())
            .map(PathBuf::from),
        video_encoder: request
            .input
            .render_profile
            .as_ref()
            .and_then(|profile| profile.encoder.clone()),
    };

    validate_artifact_staging(&request.input.artifact_staging)?;

    let mut progress_callback = |event: RenderProgress| {
        tracing::info!(
            job_id = request.input.job_id,
            stage = event.stage,
            frame = event.frame,
            total_frames = event.total_frames,
            ratio = event.ratio,
            "runpod render progress"
        );
    };

    let rendered_bytes = render_project_mp4(&bundle, &options, &mut progress_callback)
        .map_err(map_execution_error)?;

    let artifact = upload_artifact(&request.input.artifact_staging, &rendered_bytes).await?;

    Ok(RunpodJobResponse {
        ok: true,
        artifact: Some(artifact),
        metrics: Some(RunpodRenderMetrics {
            compile_ms: 0,
            render_ms: 0,
            total_frames: bundle.project.duration_frames as u64,
        }),
        error: None,
    })
}

fn map_execution_error(error: RenderPipelineError) -> RunpodRenderError {
    RunpodRenderError {
        code: error.code.to_string(),
        message: sanitize_error_message(&error.message),
        retryable: error.retryable,
    }
}

fn validate_artifact_staging(
    staging: &Option<RunpodArtifactStaging>,
) -> Result<(), RunpodRenderError> {
    let Some(staging) = staging else {
        return Err(RunpodRenderError {
            code: "artifact_staging_missing".to_string(),
            message: "artifact staging details are required".to_string(),
            retryable: false,
        });
    };

    if staging.upload_url.as_deref().is_none() {
        return Err(RunpodRenderError {
            code: "artifact_upload_url_missing".to_string(),
            message: "artifact_staging.upload_url is required".to_string(),
            retryable: false,
        });
    }

    Ok(())
}

async fn upload_artifact(
    staging: &Option<RunpodArtifactStaging>,
    bytes: &[u8],
) -> Result<RunpodArtifactOutput, RunpodRenderError> {
    let Some(staging) = staging else {
        return Err(RunpodRenderError {
            code: "artifact_staging_missing".to_string(),
            message: "artifact staging details are required".to_string(),
            retryable: false,
        });
    };

    let Some(upload_url) = staging.upload_url.as_deref() else {
        return Err(RunpodRenderError {
            code: "artifact_upload_url_missing".to_string(),
            message: "artifact_staging.upload_url is required".to_string(),
            retryable: false,
        });
    };

    let client = reqwest::Client::new();
    let response = client
        .put(upload_url)
        .header(reqwest::header::CONTENT_TYPE, "video/mp4")
        .body(bytes.to_vec())
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|err| RunpodRenderError {
            code: "artifact_upload_failed".to_string(),
            message: sanitize_error_message(&err.to_string()),
            retryable: true,
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let response_text = response.text().await.unwrap_or_default();
        return Err(RunpodRenderError {
            code: "artifact_upload_failed".to_string(),
            message: sanitize_error_message(&format!(
                "artifact upload returned status {}: {}",
                status, response_text
            )),
            retryable: status.is_server_error() || status.as_u16() == 429,
        });
    }

    let download_url = staging
        .download_url
        .clone()
        .or_else(|| staging.upload_url.clone())
        .ok_or_else(|| RunpodRenderError {
            code: "artifact_download_url_missing".to_string(),
            message: "artifact_staging.download_url is required".to_string(),
            retryable: false,
        })?;

    Ok(RunpodArtifactOutput {
        download_url,
        content_type: "video/mp4",
        bytes: bytes.len(),
    })
}

fn sanitize_error_message(message: &str) -> String {
    let trimmed = message.trim();
    if trimmed.len() <= 256 {
        return trimmed.to_string();
    }
    trimmed.chars().take(256).collect()
}
