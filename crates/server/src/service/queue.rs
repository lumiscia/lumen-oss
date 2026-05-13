use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use super::{
    BoxFuture, RenderJob, RenderJobId, RenderLease, RenderLeaseId, RenderQueue, ServiceError,
    ServiceResult, WorkerId,
};

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
