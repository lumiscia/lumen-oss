use super::{ArtifactOutput, ArtifactStaging, RenderJobError, util::sanitize_error_message};

pub(super) fn validate_artifact_staging(
    staging: &Option<ArtifactStaging>,
) -> Result<(), RenderJobError> {
    let Some(staging) = staging else {
        return Err(RenderJobError {
            code: "artifact_staging_missing".to_string(),
            message: "artifact staging details are required".to_string(),
            retryable: false,
        });
    };

    if staging.upload_url.as_deref().is_none() {
        return Err(RenderJobError {
            code: "artifact_upload_url_missing".to_string(),
            message: "artifact_staging.upload_url is required".to_string(),
            retryable: false,
        });
    }

    Ok(())
}

pub(super) async fn upload_artifact(
    staging: &Option<ArtifactStaging>,
    bytes: &[u8],
) -> Result<ArtifactOutput, RenderJobError> {
    let Some(staging) = staging else {
        return Err(RenderJobError {
            code: "artifact_staging_missing".to_string(),
            message: "artifact staging details are required".to_string(),
            retryable: false,
        });
    };

    let Some(upload_url) = staging.upload_url.as_deref() else {
        return Err(RenderJobError {
            code: "artifact_upload_url_missing".to_string(),
            message: "artifact_staging.upload_url is required".to_string(),
            retryable: false,
        });
    };

    tracing::info!(
        artifact_bytes = bytes.len(),
        upload_url,
        has_download_url = staging.download_url.is_some(),
        "uploading render artifact"
    );

    let client = reqwest::Client::new();
    let mut request = client
        .put(upload_url)
        .header(reqwest::header::CONTENT_TYPE, "video/mp4")
        .body(bytes.to_vec())
        .timeout(std::time::Duration::from_secs(60));
    if let Some(token) = staging.token.as_deref() {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|err| {
        tracing::warn!(
            upload_url,
            artifact_bytes = bytes.len(),
            "render artifact upload request failed: {err}"
        );
        RenderJobError {
            code: "artifact_upload_failed".to_string(),
            message: sanitize_error_message(&err.to_string()),
            retryable: true,
        }
    })?;

    if !response.status().is_success() {
        let status = response.status();
        let response_text = response.text().await.unwrap_or_default();
        tracing::warn!(
            upload_url,
            artifact_bytes = bytes.len(),
            status = %status,
            response_body = %response_text,
            "render artifact upload returned non-success status"
        );
        return Err(RenderJobError {
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
        .ok_or_else(|| RenderJobError {
            code: "artifact_download_url_missing".to_string(),
            message: "artifact_staging.download_url is required".to_string(),
            retryable: false,
        })?;

    tracing::info!(
        artifact_bytes = bytes.len(),
        download_url,
        "uploaded render artifact"
    );

    Ok(ArtifactOutput {
        download_url,
        content_type: "video/mp4",
        bytes: bytes.len(),
    })
}
