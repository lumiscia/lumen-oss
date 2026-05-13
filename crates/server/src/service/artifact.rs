use std::{sync::Arc, time::Duration};

use super::{ArtifactRef, ArtifactStore, ArtifactWrite, BoxFuture, ServiceError, ServiceResult};

pub trait PresignedUrlResolver: Send + Sync {
    fn resolve<'a>(&'a self, artifact: &'a ArtifactWrite) -> BoxFuture<'a, ServiceResult<String>>;
}

#[derive(Debug, Clone)]
pub struct StaticPresignedUrlResolver {
    upload_url: String,
}

impl StaticPresignedUrlResolver {
    pub fn new(upload_url: impl Into<String>) -> Self {
        Self {
            upload_url: upload_url.into(),
        }
    }
}

impl PresignedUrlResolver for StaticPresignedUrlResolver {
    fn resolve<'a>(&'a self, _artifact: &'a ArtifactWrite) -> BoxFuture<'a, ServiceResult<String>> {
        Box::pin(async move { Ok(self.upload_url.clone()) })
    }
}

#[derive(Clone)]
pub struct PresignedUrlArtifactStore<R> {
    resolver: Arc<R>,
    artifact_uri_prefix: String,
    bearer_token: Option<String>,
    timeout: Duration,
}

impl<R> PresignedUrlArtifactStore<R>
where
    R: PresignedUrlResolver,
{
    pub fn new(resolver: R, artifact_uri_prefix: impl Into<String>) -> Self {
        Self {
            resolver: Arc::new(resolver),
            artifact_uri_prefix: artifact_uri_prefix.into(),
            bearer_token: None,
            timeout: Duration::from_secs(60),
        }
    }

    pub fn with_bearer_token(mut self, bearer_token: impl Into<String>) -> Self {
        self.bearer_token = Some(bearer_token.into());
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl<R> ArtifactStore for PresignedUrlArtifactStore<R>
where
    R: PresignedUrlResolver + 'static,
{
    fn put<'a>(&'a self, artifact: ArtifactWrite) -> BoxFuture<'a, ServiceResult<ArtifactRef>> {
        Box::pin(async move {
            let upload_url = self.resolver.resolve(&artifact).await?;
            let mut request = reqwest::Client::new()
                .put(&upload_url)
                .header(reqwest::header::CONTENT_TYPE, artifact.content_type)
                .body(artifact.bytes.clone())
                .timeout(self.timeout);
            if let Some(token) = self.bearer_token.as_deref() {
                request = request.bearer_auth(token);
            }
            let response = request.send().await.map_err(|err| ServiceError {
                code: "artifact_upload_failed",
                message: err.to_string(),
                retryable: true,
            })?;
            if !response.status().is_success() {
                return Err(ServiceError {
                    code: "artifact_upload_failed",
                    message: format!("artifact upload returned status {}", response.status()),
                    retryable: response.status().is_server_error()
                        || response.status().as_u16() == 429,
                });
            }

            let id = artifact.job_id.0;
            Ok(ArtifactRef {
                id: id.clone(),
                content_type: artifact.content_type,
                bytes: artifact.bytes.len(),
                uri: format!("{}/{}", self.artifact_uri_prefix.trim_end_matches('/'), id),
            })
        })
    }
}
