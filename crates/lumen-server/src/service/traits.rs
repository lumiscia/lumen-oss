use std::time::Duration;

use super::{
    ArtifactRef, ArtifactWrite, BoxFuture, ProgressEvent, RenderJob, RenderJobId, RenderLease,
    RenderLeaseId, RenderOutput, ServiceError, ServiceResult, WorkerId,
};

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
