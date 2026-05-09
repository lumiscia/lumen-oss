use std::{collections::HashMap, future::Future, path::PathBuf, pin::Pin, time::Instant};

use serde_json::Value;

pub type ServiceResult<T> = Result<T, ServiceError>;
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone)]
pub struct RenderJob {
    pub id: RenderJobId,
    pub project: Value,
    pub media_root: Option<PathBuf>,
    pub video_encoder: Option<String>,
    pub metadata: HashMap<String, String>,
}

impl RenderJob {
    pub fn new(id: impl Into<String>, project: Value) -> Self {
        Self {
            id: RenderJobId(id.into()),
            project,
            media_root: None,
            video_encoder: None,
            metadata: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RenderJobId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RenderLeaseId(pub String);

#[derive(Debug, Clone)]
pub struct RenderLease {
    pub id: RenderLeaseId,
    pub job: RenderJob,
    pub leased_until: Instant,
}

#[derive(Debug, Clone)]
pub struct WorkerId(pub String);

#[derive(Debug, Clone)]
pub struct RenderOutput {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
    pub metrics: RenderMetrics,
}

#[derive(Debug, Clone)]
pub struct RenderMetrics {
    pub render_ms: u128,
    pub total_frames: u64,
}

#[derive(Debug, Clone)]
pub struct ArtifactWrite {
    pub job_id: RenderJobId,
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
}

#[derive(Debug, Clone)]
pub struct ArtifactRef {
    pub id: String,
    pub content_type: &'static str,
    pub bytes: usize,
    pub uri: String,
}

#[derive(Debug, Clone)]
pub struct ProgressEvent {
    pub job_id: RenderJobId,
    pub stage: String,
    pub ratio: f32,
    pub frame: Option<u32>,
    pub total_frames: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct ServiceError {
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ServiceError {}

impl From<crate::render::RenderError> for ServiceError {
    fn from(error: crate::render::RenderError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            retryable: error.retryable,
        }
    }
}
