use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::Instant,
};

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
    #[serde(default)]
    pub progress_callback: Option<RunpodProgressCallback>,
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

#[derive(Debug, Clone, Deserialize)]
pub struct RunpodProgressCallback {
    pub url: String,
    #[serde(default)]
    pub token: Option<String>,
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunpodProgressPayload<'a> {
    progress: f32,
    stage: &'a str,
    state: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact_url: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolution: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct RunpodProgressMetadata<'a> {
    artifact_url: Option<&'a str>,
    duration_ms: Option<u128>,
    output_bytes: Option<usize>,
    resolution: Option<String>,
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
    let stage_started = Instant::now();
    let staged_media = stage_remote_media(request.input.project).await?;
    post_progress_async(
        &request.input.progress_callback,
        0.02,
        "media_staged",
        "processing",
        None,
        None,
    )
    .await;

    let bundle = convert_project_payload(&staged_media.project).map_err(map_execution_error)?;
    validate_artifact_staging(&request.input.artifact_staging)?;

    let options = RenderOptions {
        media_root: staged_media.media_root().or_else(|| {
            request
                .input
                .render_profile
                .as_ref()
                .and_then(|profile| profile.media_root.as_ref())
                .map(PathBuf::from)
        }),
        video_encoder: request
            .input
            .render_profile
            .as_ref()
            .and_then(|profile| profile.encoder.clone()),
    };

    post_progress_sync(
        &request.input.progress_callback,
        0.05,
        "accepted",
        "processing",
        None,
        None,
    );

    let callback = request.input.progress_callback.clone();
    let render_started = Instant::now();
    let mut progress_callback = |event: RenderProgress| {
        tracing::info!(
            job_id = request.input.job_id,
            stage = event.stage,
            frame = event.frame,
            total_frames = event.total_frames,
            ratio = event.ratio,
            "runpod render progress"
        );
        let progress = 0.05 + (0.90 * event.ratio.clamp(0.0, 1.0));
        post_progress_sync(&callback, progress, event.stage, "processing", None, None);
    };

    let rendered_bytes = render_project_mp4(&bundle, &options, &mut progress_callback)
        .map_err(map_execution_error)?;
    let render_ms = render_started.elapsed().as_millis();

    let artifact = upload_artifact(&request.input.artifact_staging, &rendered_bytes).await?;
    post_progress_async(
        &request.input.progress_callback,
        1.0,
        "completed",
        "succeeded",
        Some(RunpodProgressMetadata {
            artifact_url: Some(&artifact.download_url),
            duration_ms: Some(render_ms),
            output_bytes: Some(artifact.bytes),
            resolution: Some(format!(
                "{}x{}",
                bundle.project.width, bundle.project.height
            )),
        }),
        None,
    )
    .await;

    Ok(RunpodJobResponse {
        ok: true,
        artifact: Some(artifact),
        metrics: Some(RunpodRenderMetrics {
            compile_ms: stage_started.elapsed().as_millis(),
            render_ms,
            total_frames: bundle.project.duration_frames as u64,
        }),
        error: None,
    })
}

struct StagedMedia {
    project: Value,
    _dir: Option<tempfile::TempDir>,
}

impl StagedMedia {
    fn media_root(&self) -> Option<PathBuf> {
        self._dir.as_ref().map(|dir| dir.path().to_path_buf())
    }
}

async fn stage_remote_media(mut project: Value) -> Result<StagedMedia, RunpodRenderError> {
    let mut urls = HashSet::new();
    collect_remote_media_urls(&project, None, &mut urls);
    if urls.is_empty() {
        return Ok(StagedMedia {
            project,
            _dir: None,
        });
    }

    let dir = tempfile::tempdir().map_err(|err| RunpodRenderError {
        code: "media_stage_failed".to_string(),
        message: format!("failed to create media staging directory: {err}"),
        retryable: true,
    })?;
    let mut replacements = HashMap::new();

    for (index, url) in urls.into_iter().enumerate() {
        let filename = staged_media_filename(index, &url);
        let path = dir.path().join(filename);
        download_remote_media(&url, &path).await?;
        replacements.insert(url, path.file_name().unwrap().to_string_lossy().to_string());
    }

    rewrite_remote_media_urls(&mut project, None, &replacements);
    Ok(StagedMedia {
        project,
        _dir: Some(dir),
    })
}

fn collect_remote_media_urls(value: &Value, key: Option<&str>, urls: &mut HashSet<String>) {
    match value {
        Value::String(source) if is_media_source_key(key) && is_http_url(source) => {
            urls.insert(source.clone());
        }
        Value::Array(items) => {
            for item in items {
                collect_remote_media_urls(item, None, urls);
            }
        }
        Value::Object(object) => {
            for (key, value) in object {
                collect_remote_media_urls(value, Some(key), urls);
            }
        }
        _ => {}
    }
}

fn rewrite_remote_media_urls(
    value: &mut Value,
    key: Option<&str>,
    replacements: &HashMap<String, String>,
) {
    match value {
        Value::String(source) if is_media_source_key(key) => {
            if let Some(replacement) = replacements.get(source) {
                *source = replacement.clone();
            }
        }
        Value::Array(items) => {
            for item in items {
                rewrite_remote_media_urls(item, None, replacements);
            }
        }
        Value::Object(object) => {
            for (key, value) in object {
                rewrite_remote_media_urls(value, Some(key), replacements);
            }
        }
        _ => {}
    }
}

fn is_media_source_key(key: Option<&str>) -> bool {
    matches!(key, Some("source" | "url" | "path"))
}

fn is_http_url(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

fn staged_media_filename(index: usize, url: &str) -> String {
    let extension = reqwest::Url::parse(url)
        .ok()
        .and_then(|url| {
            Path::new(url.path())
                .extension()
                .map(|extension| extension.to_string_lossy().to_string())
        })
        .filter(|extension| {
            !extension.is_empty()
                && extension.len() <= 8
                && extension
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        })
        .unwrap_or_else(|| "bin".to_string());
    format!("remote-{index}.{extension}")
}

async fn download_remote_media(url: &str, path: &Path) -> Result<(), RunpodRenderError> {
    let parsed = reqwest::Url::parse(url).map_err(|err| RunpodRenderError {
        code: "media_download_failed".to_string(),
        message: format!("invalid media URL: {err}"),
        retryable: false,
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(RunpodRenderError {
            code: "media_download_failed".to_string(),
            message: format!("unsupported media URL scheme: {}", parsed.scheme()),
            retryable: false,
        });
    }

    let response = reqwest::Client::new()
        .get(parsed)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await
        .map_err(|err| RunpodRenderError {
            code: "media_download_failed".to_string(),
            message: sanitize_error_message(&err.to_string()),
            retryable: true,
        })?
        .error_for_status()
        .map_err(|err| RunpodRenderError {
            code: "media_download_failed".to_string(),
            message: sanitize_error_message(&err.to_string()),
            retryable: err
                .status()
                .is_some_and(|status| status.is_server_error() || status.as_u16() == 429),
        })?;

    let bytes = response.bytes().await.map_err(|err| RunpodRenderError {
        code: "media_download_failed".to_string(),
        message: sanitize_error_message(&err.to_string()),
        retryable: true,
    })?;
    tokio::fs::write(path, bytes)
        .await
        .map_err(|err| RunpodRenderError {
            code: "media_stage_failed".to_string(),
            message: format!("failed to write staged media: {err}"),
            retryable: true,
        })
}

fn post_progress_sync(
    callback: &Option<RunpodProgressCallback>,
    progress: f32,
    stage: &str,
    state: &str,
    artifact_url: Option<&str>,
    error: Option<&str>,
) {
    let Some(callback) = callback else {
        return;
    };

    let callback = callback.clone();
    let mut payload = serde_json::Map::from_iter([
        (
            "progress".to_string(),
            serde_json::json!(progress.clamp(0.0, 1.0)),
        ),
        ("stage".to_string(), serde_json::json!(stage)),
        ("state".to_string(), serde_json::json!(state)),
    ]);
    if let Some(artifact_url) = artifact_url {
        payload.insert("artifactUrl".to_string(), serde_json::json!(artifact_url));
    }
    if let Some(error) = error {
        payload.insert("error".to_string(), serde_json::json!(error));
    }
    let payload = serde_json::Value::Object(payload);
    let result = std::thread::spawn(move || post_progress_payload_blocking(&callback, payload))
        .join()
        .unwrap_or_else(|_| Err("progress callback thread panicked".to_string()));

    if let Err(err) = result {
        tracing::warn!("failed to post runpod progress callback: {err}");
    }
}

async fn post_progress_async(
    callback: &Option<RunpodProgressCallback>,
    progress: f32,
    stage: &str,
    state: &str,
    metadata: Option<RunpodProgressMetadata<'_>>,
    error: Option<&str>,
) {
    let Some(callback) = callback else {
        return;
    };

    let metadata = metadata.unwrap_or_default();
    let payload = RunpodProgressPayload {
        progress: progress.clamp(0.0, 1.0),
        stage,
        state,
        artifact_url: metadata.artifact_url,
        duration_ms: metadata.duration_ms,
        error,
        output_bytes: metadata.output_bytes,
        resolution: metadata.resolution,
    };
    if let Err(err) = post_progress_payload(callback, &payload).await {
        tracing::warn!("failed to post runpod progress callback: {err}");
    }
}

async fn post_progress_payload(
    callback: &RunpodProgressCallback,
    payload: &RunpodProgressPayload<'_>,
) -> reqwest::Result<()> {
    let mut request = reqwest::Client::new()
        .post(&callback.url)
        .json(payload)
        .timeout(std::time::Duration::from_secs(5));

    if let Some(token) = callback.token.as_deref() {
        request = request.bearer_auth(token);
    }

    request.send().await?.error_for_status()?;
    Ok(())
}

fn post_progress_payload_blocking(
    callback: &RunpodProgressCallback,
    payload: serde_json::Value,
) -> Result<(), String> {
    let mut request = reqwest::blocking::Client::new()
        .post(&callback.url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(5));

    if let Some(token) = callback.token.as_deref() {
        request = request.bearer_auth(token);
    }

    request
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map(|_| ())
        .map_err(|err| err.to_string())
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
