use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use serde_json::Value;

use crate::render::{RenderOptions, convert_project_payload, render_project_mp4};

pub type ServiceResult<T> = Result<T, ServiceError>;
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone)]
pub struct RenderJob {
    pub id: RenderJobId,
    pub project: Value,
    pub media_root: Option<std::path::PathBuf>,
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

pub trait RenderQueue: Send + Sync {
    fn enqueue<'a>(&'a self, job: RenderJob) -> BoxFuture<'a, ServiceResult<RenderJobId>>;
    fn lease<'a>(
        &'a self,
        worker_id: &'a WorkerId,
        ttl: Duration,
    ) -> BoxFuture<'a, ServiceResult<Option<RenderLease>>>;
    fn ack<'a>(&'a self, lease_id: RenderLeaseId) -> BoxFuture<'a, ServiceResult<()>>;
    fn nack<'a>(
        &'a self,
        lease_id: RenderLeaseId,
        reason: ServiceError,
    ) -> BoxFuture<'a, ServiceResult<()>>;
    fn heartbeat<'a>(
        &'a self,
        lease_id: &'a RenderLeaseId,
        ttl: Duration,
    ) -> BoxFuture<'a, ServiceResult<()>>;
}

pub trait RenderExecutor: Send + Sync {
    fn execute<'a>(
        &'a self,
        job: RenderJob,
        progress: &'a dyn ProgressSink,
    ) -> BoxFuture<'a, ServiceResult<RenderOutput>>;
}

pub trait ArtifactStore: Send + Sync {
    fn put<'a>(&'a self, artifact: ArtifactWrite) -> BoxFuture<'a, ServiceResult<ArtifactRef>>;
}

pub trait ProgressSink: Send + Sync {
    fn publish<'a>(&'a self, event: ProgressEvent) -> BoxFuture<'a, ServiceResult<()>>;
}

#[derive(Debug, Default)]
pub struct NoopProgressSink;

impl ProgressSink for NoopProgressSink {
    fn publish<'a>(&'a self, _event: ProgressEvent) -> BoxFuture<'a, ServiceResult<()>> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Debug, Default)]
pub struct InMemoryRenderQueue {
    next_lease: AtomicU64,
    inner: Mutex<InMemoryRenderQueueState>,
}

#[derive(Debug, Default)]
struct InMemoryRenderQueueState {
    pending: VecDeque<RenderJob>,
    leased: HashMap<RenderLeaseId, RenderLease>,
}

impl InMemoryRenderQueue {
    pub fn new() -> Self {
        Self::default()
    }
}

impl RenderQueue for InMemoryRenderQueue {
    fn enqueue<'a>(&'a self, job: RenderJob) -> BoxFuture<'a, ServiceResult<RenderJobId>> {
        Box::pin(async move {
            let id = job.id.clone();
            self.inner
                .lock()
                .map_err(lock_error)?
                .pending
                .push_back(job);
            Ok(id)
        })
    }

    fn lease<'a>(
        &'a self,
        _worker_id: &'a WorkerId,
        ttl: Duration,
    ) -> BoxFuture<'a, ServiceResult<Option<RenderLease>>> {
        Box::pin(async move {
            let Some(job) = self.inner.lock().map_err(lock_error)?.pending.pop_front() else {
                return Ok(None);
            };
            let lease = RenderLease {
                id: RenderLeaseId(format!(
                    "lease-{}",
                    self.next_lease.fetch_add(1, Ordering::Relaxed)
                )),
                job,
                leased_until: Instant::now() + ttl,
            };
            self.inner
                .lock()
                .map_err(lock_error)?
                .leased
                .insert(lease.id.clone(), lease.clone());
            Ok(Some(lease))
        })
    }

    fn ack<'a>(&'a self, lease_id: RenderLeaseId) -> BoxFuture<'a, ServiceResult<()>> {
        Box::pin(async move {
            self.inner
                .lock()
                .map_err(lock_error)?
                .leased
                .remove(&lease_id);
            Ok(())
        })
    }

    fn nack<'a>(
        &'a self,
        lease_id: RenderLeaseId,
        _reason: ServiceError,
    ) -> BoxFuture<'a, ServiceResult<()>> {
        Box::pin(async move {
            let mut inner = self.inner.lock().map_err(lock_error)?;
            if let Some(lease) = inner.leased.remove(&lease_id) {
                inner.pending.push_back(lease.job);
            }
            Ok(())
        })
    }

    fn heartbeat<'a>(
        &'a self,
        lease_id: &'a RenderLeaseId,
        ttl: Duration,
    ) -> BoxFuture<'a, ServiceResult<()>> {
        Box::pin(async move {
            if let Some(lease) = self
                .inner
                .lock()
                .map_err(lock_error)?
                .leased
                .get_mut(lease_id)
            {
                lease.leased_until = Instant::now() + ttl;
            }
            Ok(())
        })
    }
}

#[derive(Debug, Default)]
pub struct LocalRenderExecutor;

impl RenderExecutor for LocalRenderExecutor {
    fn execute<'a>(
        &'a self,
        job: RenderJob,
        progress: &'a dyn ProgressSink,
    ) -> BoxFuture<'a, ServiceResult<RenderOutput>> {
        Box::pin(async move {
            let bundle = convert_project_payload(&job.project).map_err(ServiceError::from)?;
            let total_frames = bundle.project.duration_frames;
            let options = RenderOptions {
                media_root: job.media_root,
                video_encoder: job.video_encoder,
            };
            let job_id = job.id.clone();
            let started = Instant::now();
            progress
                .publish(ProgressEvent {
                    job_id: job_id.clone(),
                    stage: "accepted".to_string(),
                    ratio: 0.0,
                    frame: None,
                    total_frames: Some(total_frames),
                })
                .await?;
            let rendered = tokio::task::spawn_blocking(move || {
                let mut ignore_progress = |_event| {};
                render_project_mp4(&bundle, &options, &mut ignore_progress)
            })
            .await
            .map_err(|err| ServiceError {
                code: "render_worker_failed",
                message: format!("render worker join failed: {err}"),
                retryable: true,
            })?
            .map_err(ServiceError::from)?;
            progress
                .publish(ProgressEvent {
                    job_id,
                    stage: "completed".to_string(),
                    ratio: 1.0,
                    frame: Some(total_frames),
                    total_frames: Some(total_frames),
                })
                .await?;

            Ok(RenderOutput {
                bytes: rendered,
                content_type: "video/mp4",
                metrics: RenderMetrics {
                    render_ms: started.elapsed().as_millis(),
                    total_frames: total_frames as u64,
                },
            })
        })
    }
}

pub struct RenderService<Q, E, A, P> {
    pub queue: Arc<Q>,
    pub executor: Arc<E>,
    pub artifacts: Arc<A>,
    pub progress: Arc<P>,
}

impl<Q, E, A, P> RenderService<Q, E, A, P>
where
    Q: RenderQueue,
    E: RenderExecutor,
    A: ArtifactStore,
    P: ProgressSink,
{
    pub fn new(queue: Q, executor: E, artifacts: A, progress: P) -> Self {
        Self {
            queue: Arc::new(queue),
            executor: Arc::new(executor),
            artifacts: Arc::new(artifacts),
            progress: Arc::new(progress),
        }
    }
}

impl From<crate::render::RenderError> for ServiceError {
    fn from(error: crate::render::RenderError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            retryable: error.retryable,
        }
    }
}

fn lock_error<T>(_err: std::sync::PoisonError<T>) -> ServiceError {
    ServiceError {
        code: "queue_lock_poisoned",
        message: "render queue lock was poisoned".to_string(),
        retryable: true,
    }
}

#[cfg(test)]
mod tests {
    use super::{InMemoryRenderQueue, RenderJob, RenderQueue, WorkerId};

    #[tokio::test]
    async fn in_memory_queue_requeues_nacked_jobs() {
        let queue = InMemoryRenderQueue::new();
        queue
            .enqueue(RenderJob::new("job-1", serde_json::json!({})))
            .await
            .expect("enqueue");

        let worker = WorkerId("worker-1".to_string());
        let lease = queue
            .lease(&worker, std::time::Duration::from_secs(30))
            .await
            .expect("lease")
            .expect("lease present");
        assert_eq!(lease.job.id.0, "job-1");

        queue
            .nack(
                lease.id,
                super::ServiceError {
                    code: "test_failure",
                    message: "test failure".to_string(),
                    retryable: true,
                },
            )
            .await
            .expect("nack");

        let lease = queue
            .lease(&worker, std::time::Duration::from_secs(30))
            .await
            .expect("lease")
            .expect("lease present");
        assert_eq!(lease.job.id.0, "job-1");
    }
}
