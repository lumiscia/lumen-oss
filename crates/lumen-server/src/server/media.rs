use std::{
    collections::HashMap,
    net::IpAddr,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde_json::Value;

use super::{RenderJobError, util::sanitize_error_message};

const MAX_REMOTE_MEDIA_BYTES: u64 = 512 * 1024 * 1024;
const MEDIA_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MEDIA_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const MEDIA_DOWNLOAD_CONCURRENCY: usize = 4;

pub(super) struct StagedMedia {
    pub project: Value,
    dir: Option<tempfile::TempDir>,
}

impl StagedMedia {
    pub fn media_root(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|dir| dir.path().to_path_buf())
    }
}

pub(super) async fn stage_remote_media(
    project: Value,
    media: HashMap<String, String>,
    allowed_hosts: &[String],
) -> Result<StagedMedia, RenderJobError> {
    reject_inline_remote_media(&project, None)?;
    tracing::info!(
        media_count = media.len(),
        allowed_media_host_count = allowed_hosts.len(),
        "staging render media manifest"
    );
    if media.is_empty() {
        return Ok(StagedMedia { project, dir: None });
    }

    let dir = tempfile::tempdir().map_err(|err| RenderJobError {
        code: "media_stage_failed".to_string(),
        message: format!("failed to create media staging directory: {err}"),
        retryable: true,
    })?;

    let client = reqwest::Client::builder()
        .connect_timeout(MEDIA_CONNECT_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .tcp_nodelay(true)
        .timeout(MEDIA_REQUEST_TIMEOUT)
        .build()
        .map_err(|err| RenderJobError {
            code: "media_download_failed".to_string(),
            message: sanitize_error_message(&err.to_string()),
            retryable: true,
        })?;
    let mut downloads = tokio::task::JoinSet::new();
    let mut media = media.into_iter();

    loop {
        while downloads.len() < MEDIA_DOWNLOAD_CONCURRENCY {
            let Some((alias, url)) = media.next() else {
                break;
            };
            validate_media_alias(&alias)?;
            let path = dir.path().join(&alias);
            let allowed_hosts = allowed_hosts.to_vec();
            let client = client.clone();
            downloads.spawn(async move {
                let bytes =
                    download_remote_media(&client, &alias, &url, &path, &allowed_hosts).await?;
                Ok::<_, RenderJobError>((alias, path, bytes))
            });
        }

        let Some(result) = downloads.join_next().await else {
            break;
        };
        let (alias, path, bytes) = result.map_err(|err| RenderJobError {
            code: "media_stage_failed".to_string(),
            message: format!("media download task failed: {err}"),
            retryable: true,
        })??;
        tracing::info!(
            alias,
            staged_path = %path.display(),
            bytes,
            "staged render media source"
        );
    }

    Ok(StagedMedia {
        project,
        dir: Some(dir),
    })
}

fn reject_inline_remote_media(value: &Value, key: Option<&str>) -> Result<(), RenderJobError> {
    match value {
        Value::String(source)
            if is_media_source_key(key)
                && (is_http_url(source) || source.starts_with("lumen:")) =>
        {
            Err(RenderJobError {
                code: "inline_media_reference_rejected".to_string(),
                message: "media references must use the render media manifest".to_string(),
                retryable: false,
            })
        }
        Value::Array(items) => {
            for item in items {
                reject_inline_remote_media(item, None)?;
            }
            Ok(())
        }
        Value::Object(object) => {
            for (key, value) in object {
                reject_inline_remote_media(value, Some(key))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn is_media_source_key(key: Option<&str>) -> bool {
    matches!(key, Some("source" | "url" | "path" | "source_id"))
}

fn is_http_url(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

fn validate_media_alias(alias: &str) -> Result<(), RenderJobError> {
    let valid = !alias.is_empty()
        && alias.len() <= 128
        && alias
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'));
    if valid {
        Ok(())
    } else {
        Err(RenderJobError {
            code: "invalid_media_alias".to_string(),
            message: "media manifest contains an invalid alias".to_string(),
            retryable: false,
        })
    }
}

async fn download_remote_media(
    client: &reqwest::Client,
    alias: &str,
    url: &str,
    path: &Path,
    allowed_hosts: &[String],
) -> Result<u64, RenderJobError> {
    if url.starts_with("lumen:") {
        return Err(RenderJobError {
            code: "media_download_unresolved_lumen_media".to_string(),
            message: "lumen media references must be resolved by the API before rendering"
                .to_string(),
            retryable: false,
        });
    }

    let parsed = reqwest::Url::parse(url).map_err(|err| RenderJobError {
        code: "media_download_failed".to_string(),
        message: format!("invalid media URL: {err}"),
        retryable: false,
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(RenderJobError {
            code: "media_download_failed".to_string(),
            message: format!("unsupported media URL scheme: {}", parsed.scheme()),
            retryable: false,
        });
    }
    tracing::info!(
        alias,
        media_url_host = parsed.host_str().unwrap_or_default(),
        media_url_path = parsed.path(),
        "downloading render media source"
    );
    validate_remote_media_host(&parsed, allowed_hosts).await?;

    let media_url_host = parsed.host_str().unwrap_or_default().to_string();
    let started = Instant::now();
    let response = client.get(parsed).send().await.map_err(|err| {
        tracing::warn!(
            alias,
            media_url_host,
            "render media download request failed: {err}"
        );
        RenderJobError {
            code: "media_download_failed".to_string(),
            message: sanitize_error_message(&err.to_string()),
            retryable: true,
        }
    })?;
    let response_headers_ms = started.elapsed().as_millis();
    let status = response.status();
    if !status.is_success() {
        return Err(RenderJobError {
            code: "media_download_failed".to_string(),
            message: sanitize_error_message(&format!("media download returned status {status}")),
            retryable: status.is_server_error() || status.as_u16() == 429,
        });
    }

    if let Some(content_length) = response.content_length()
        && content_length > MAX_REMOTE_MEDIA_BYTES
    {
        return Err(RenderJobError {
            code: "media_download_too_large".to_string(),
            message: "remote media exceeds the maximum allowed size".to_string(),
            retryable: false,
        });
    }
    if let Some(content_type) = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    {
        let content_type = content_type.to_ascii_lowercase();
        if !content_type.starts_with("image/")
            && !content_type.starts_with("video/")
            && !content_type.starts_with("audio/")
            && content_type != "application/octet-stream"
        {
            return Err(RenderJobError {
                code: "media_download_unsupported_type".to_string(),
                message: "remote media content type is not supported".to_string(),
                retryable: false,
            });
        }
    }

    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|err| RenderJobError {
            code: "media_stage_failed".to_string(),
            message: format!("failed to write staged media: {err}"),
            retryable: true,
        })?;
    let mut downloaded = 0_u64;
    let mut response = response;
    let body_started = Instant::now();
    while let Some(chunk) = response.chunk().await.map_err(|err| RenderJobError {
        code: "media_download_failed".to_string(),
        message: sanitize_error_message(&err.to_string()),
        retryable: true,
    })? {
        downloaded += chunk.len() as u64;
        if downloaded > MAX_REMOTE_MEDIA_BYTES {
            return Err(RenderJobError {
                code: "media_download_too_large".to_string(),
                message: "remote media exceeds the maximum allowed size".to_string(),
                retryable: false,
            });
        }
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .map_err(|err| RenderJobError {
                code: "media_stage_failed".to_string(),
                message: format!("failed to write staged media: {err}"),
                retryable: true,
            })?;
    }
    tracing::info!(
        alias,
        media_url_host,
        response_headers_ms,
        body_ms = body_started.elapsed().as_millis(),
        total_ms = started.elapsed().as_millis(),
        bytes = downloaded,
        "downloaded render media source"
    );
    Ok(downloaded)
}

async fn validate_remote_media_host(
    url: &reqwest::Url,
    allowed_hosts: &[String],
) -> Result<(), RenderJobError> {
    let Some(host) = url.host_str() else {
        return Err(RenderJobError {
            code: "media_download_failed".to_string(),
            message: "remote media URL requires a host".to_string(),
            retryable: false,
        });
    };

    if !is_allowed_media_host(host, allowed_hosts) {
        return Err(forbidden_media_host_error());
    }

    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err(forbidden_media_host_error());
    }

    let port = url.port_or_known_default().unwrap_or(443);
    let addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|err| RenderJobError {
            code: "media_download_failed".to_string(),
            message: sanitize_error_message(&err.to_string()),
            retryable: true,
        })?;

    for address in addresses {
        if is_private_ip(address.ip()) {
            return Err(forbidden_media_host_error());
        }
    }

    Ok(())
}

fn is_allowed_media_host(host: &str, allowed_hosts: &[String]) -> bool {
    allowed_hosts
        .iter()
        .any(|allowed| host.eq_ignore_ascii_case(allowed.trim()))
}

fn forbidden_media_host_error() -> RenderJobError {
    RenderJobError {
        code: "media_download_forbidden_host".to_string(),
        message: "remote media URL host is not allowed".to_string(),
        retryable: false,
    }
}

fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.octets()[0] == 0
                || ip.octets()[0] >= 224
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{is_allowed_media_host, is_media_source_key, reject_inline_remote_media};

    #[test]
    fn media_hosts_must_match_allowlist_exactly() {
        let allowed = vec!["test-account.r2.cloudflarestorage.com".to_string()];

        assert!(is_allowed_media_host(
            "test-account.r2.cloudflarestorage.com",
            &allowed
        ));
        assert!(is_allowed_media_host(
            "TEST-ACCOUNT.r2.cloudflarestorage.com",
            &allowed
        ));
        assert!(!is_allowed_media_host("cdn.example.com", &allowed));
        assert!(!is_allowed_media_host(
            "evil-test-account.r2.cloudflarestorage.com",
            &allowed
        ));
        assert!(!is_allowed_media_host(
            "test-account.r2.cloudflarestorage.com.evil.test",
            &allowed
        ));
    }

    #[test]
    fn audio_source_ids_are_media_references() {
        assert!(is_media_source_key(Some("source_id")));
    }

    #[test]
    fn inline_remote_media_is_rejected() {
        let project = json!({
            "nodes": [{ "properties": { "source": "https://example.com/video.mp4" } }],
            "audio": { "clips": [{ "source_id": "lumen:media_test" }] }
        });

        assert!(reject_inline_remote_media(&project, None).is_err());
    }
}
