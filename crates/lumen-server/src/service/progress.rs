use std::time::Duration;

use serde::Serialize;

use super::{BoxFuture, ProgressEvent, ProgressSink, ServiceError, ServiceResult};

#[derive(Debug, Default)]
pub struct NoopProgressSink;

impl ProgressSink for NoopProgressSink {
    fn publish<'a>(&'a self, _event: ProgressEvent) -> BoxFuture<'a, ServiceResult<()>> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Debug, Clone)]
pub struct ProgressCallbackTarget {
    pub url: String,
    pub token: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CallbackProgressSink {
    target: ProgressCallbackTarget,
    timeout: Duration,
}

impl CallbackProgressSink {
    pub fn new(target: ProgressCallbackTarget) -> Self {
        Self {
            target,
            timeout: Duration::from_secs(5),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl ProgressSink for CallbackProgressSink {
    fn publish<'a>(&'a self, event: ProgressEvent) -> BoxFuture<'a, ServiceResult<()>> {
        Box::pin(async move {
            let payload = ProgressPayload {
                job_id: &event.job_id.0,
                progress: event.ratio.clamp(0.0, 1.0),
                stage: &event.stage,
                frame: event.frame,
                total_frames: event.total_frames,
            };
            let mut request = reqwest::Client::new()
                .post(&self.target.url)
                .json(&payload)
                .timeout(self.timeout);
            if let Some(token) = self.target.token.as_deref() {
                request = request.bearer_auth(token);
            }
            let response = request.send().await.map_err(|err| ServiceError {
                code: "progress_callback_failed",
                message: err.to_string(),
                retryable: true,
            })?;
            if !response.status().is_success() {
                return Err(ServiceError {
                    code: "progress_callback_failed",
                    message: format!("progress callback returned status {}", response.status()),
                    retryable: response.status().is_server_error()
                        || response.status().as_u16() == 429,
                });
            }
            Ok(())
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressPayload<'a> {
    job_id: &'a str,
    progress: f32,
    stage: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_frames: Option<u32>,
}
