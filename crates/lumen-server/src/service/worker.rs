use std::{sync::Arc, time::Duration};

use super::{
    ArtifactStore, ArtifactWrite, ProgressSink, RenderExecutor, RenderQueue, ServiceError,
    ServiceResult, WorkerId,
};

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

    pub async fn process_next(
        &self,
        worker_id: &WorkerId,
        lease_ttl: Duration,
    ) -> ServiceResult<ProcessNextOutcome> {
        let Some(lease) = self.queue.lease(worker_id, lease_ttl).await? else {
            return Ok(ProcessNextOutcome::NoJob);
        };
        let lease_id = lease.id.clone();
        let job_id = lease.job.id.clone();

        match self
            .executor
            .execute(lease.job, self.progress.as_ref())
            .await
        {
            Ok(output) => {
                let artifact = match self
                    .artifacts
                    .put(ArtifactWrite {
                        job_id,
                        bytes: output.bytes,
                        content_type: output.content_type,
                    })
                    .await
                {
                    Ok(artifact) => artifact,
                    Err(error) => {
                        self.queue.nack(lease_id, error.clone()).await?;
                        return Err(error);
                    }
                };
                self.queue.ack(lease_id).await?;
                Ok(ProcessNextOutcome::Completed {
                    artifact_uri: artifact.uri,
                })
            }
            Err(error) => {
                self.queue.nack(lease_id, error.clone()).await?;
                Err(error)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessNextOutcome {
    NoJob,
    Completed { artifact_uri: String },
}

impl From<anyhow::Error> for ServiceError {
    fn from(error: anyhow::Error) -> Self {
        Self {
            code: "service_error",
            message: error.to_string(),
            retryable: true,
        }
    }
}
