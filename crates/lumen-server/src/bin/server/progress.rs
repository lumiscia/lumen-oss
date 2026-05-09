use serde::Serialize;

use super::ProgressCallback;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressPayload<'a> {
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
pub(super) struct ProgressMetadata<'a> {
    pub artifact_url: Option<&'a str>,
    pub duration_ms: Option<u128>,
    pub output_bytes: Option<usize>,
    pub resolution: Option<String>,
}

pub(super) fn post_progress_detached(
    callback: &Option<ProgressCallback>,
    progress: f32,
    stage: &str,
    state: &str,
    artifact_url: Option<&str>,
    error: Option<&str>,
) {
    let Some(callback) = callback else {
        tracing::debug!(
            stage,
            state,
            "skipping render progress callback; callback not configured"
        );
        return;
    };

    tracing::debug!(
        callback_url = %callback.url,
        progress = progress.clamp(0.0, 1.0),
        stage,
        state,
        has_artifact_url = artifact_url.is_some(),
        has_error = error.is_some(),
        "posting render progress callback"
    );

    let callback = callback.clone();
    let stage = stage.to_string();
    let state = state.to_string();
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
    let callback_url = callback.url.clone();
    std::thread::spawn(move || {
        if let Err(err) = post_progress_payload_blocking(&callback, payload) {
            tracing::warn!(
                callback_url,
                stage,
                state,
                "failed to post detached render progress callback: {err}"
            );
        }
    });
}

pub(super) async fn post_progress_async(
    callback: &Option<ProgressCallback>,
    progress: f32,
    stage: &str,
    state: &str,
    metadata: Option<ProgressMetadata<'_>>,
    error: Option<&str>,
) -> Result<(), String> {
    let Some(callback) = callback else {
        tracing::debug!(
            stage,
            state,
            "skipping render progress callback; callback not configured"
        );
        return Ok(());
    };

    let metadata = metadata.unwrap_or_default();
    let payload = ProgressPayload {
        progress: progress.clamp(0.0, 1.0),
        stage,
        state,
        artifact_url: metadata.artifact_url,
        duration_ms: metadata.duration_ms,
        error,
        output_bytes: metadata.output_bytes,
        resolution: metadata.resolution,
    };
    tracing::debug!(
        callback_url = %callback.url,
        progress = payload.progress,
        stage = payload.stage,
        state = payload.state,
        has_artifact_url = payload.artifact_url.is_some(),
        has_error = payload.error.is_some(),
        output_bytes = payload.output_bytes,
        resolution = payload.resolution.as_deref(),
        "posting render progress callback"
    );

    post_progress_payload(callback, &payload)
        .await
        .inspect_err(|err| {
            tracing::warn!(
                callback_url = %callback.url,
                stage = payload.stage,
                state = payload.state,
                "failed to post render progress callback: {err}"
            );
        })
}

async fn post_progress_payload(
    callback: &ProgressCallback,
    payload: &ProgressPayload<'_>,
) -> Result<(), String> {
    let mut request = reqwest::Client::new()
        .post(&callback.url)
        .json(payload)
        .timeout(std::time::Duration::from_secs(5));

    if let Some(token) = callback.token.as_deref() {
        request = request.bearer_auth(token);
    }

    let response = request.send().await.map_err(|err| err.to_string())?;
    let status = response.status();
    if !status.is_success() {
        let response_text = response.text().await.unwrap_or_default();
        return Err(format!(
            "progress callback returned status {status}: {response_text}"
        ));
    }
    Ok(())
}

fn post_progress_payload_blocking(
    callback: &ProgressCallback,
    payload: serde_json::Value,
) -> Result<(), String> {
    let mut request = reqwest::blocking::Client::new()
        .post(&callback.url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(5));

    if let Some(token) = callback.token.as_deref() {
        request = request.bearer_auth(token);
    }

    let response = request.send().map_err(|err| err.to_string())?;
    let status = response.status();
    if !status.is_success() {
        let response_text = response.text().unwrap_or_default();
        return Err(format!(
            "progress callback returned status {status}: {response_text}"
        ));
    }

    Ok(())
}
