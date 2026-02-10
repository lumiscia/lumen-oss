use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::anyhow;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{Mutex, Notify, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderJobState {
    Queued,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobFailure {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderJobStatus {
    pub job_id: String,
    pub state: RenderJobState,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub artifact_key: Option<String>,
    pub error: Option<JobFailure>,
}

#[derive(Debug, Clone)]
pub struct RenderJobRecord {
    pub status: RenderJobStatus,
    pub payload: Value,
}

#[derive(Debug, Clone)]
pub struct ObjectBlob {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

#[async_trait]
pub trait JobStore: Send + Sync {
    async fn create(&self, payload: Value) -> anyhow::Result<RenderJobStatus>;
    async fn get_status(&self, job_id: &str) -> anyhow::Result<Option<RenderJobStatus>>;
    async fn get_record(&self, job_id: &str) -> anyhow::Result<Option<RenderJobRecord>>;
    async fn mark_running(&self, job_id: &str) -> anyhow::Result<()>;
    async fn mark_completed(&self, job_id: &str, artifact_key: String) -> anyhow::Result<()>;
    async fn mark_failed(&self, job_id: &str, code: &str, message: String) -> anyhow::Result<()>;
}

#[async_trait]
pub trait JobQueue: Send + Sync {
    async fn enqueue(&self, job_id: String) -> anyhow::Result<()>;
    async fn reserve(&self) -> anyhow::Result<String>;
}

#[async_trait]
pub trait ObjectStore: Send + Sync {
    async fn put(&self, key: String, blob: ObjectBlob) -> anyhow::Result<()>;
    async fn get(&self, key: &str) -> anyhow::Result<Option<ObjectBlob>>;
}

pub struct InMemoryJobStore {
    id_counter: AtomicU64,
    jobs: RwLock<HashMap<String, RenderJobRecord>>,
}

impl InMemoryJobStore {
    pub fn new() -> Self {
        Self {
            id_counter: AtomicU64::new(1),
            jobs: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl JobStore for InMemoryJobStore {
    async fn create(&self, payload: Value) -> anyhow::Result<RenderJobStatus> {
        let id = self.id_counter.fetch_add(1, Ordering::Relaxed);
        let now = now_ms();
        let status = RenderJobStatus {
            job_id: format!("render_{id:016x}"),
            state: RenderJobState::Queued,
            created_at_ms: now,
            updated_at_ms: now,
            artifact_key: None,
            error: None,
        };

        let record = RenderJobRecord {
            status: status.clone(),
            payload,
        };

        self.jobs.write().await.insert(status.job_id.clone(), record);

        Ok(status)
    }

    async fn get_status(&self, job_id: &str) -> anyhow::Result<Option<RenderJobStatus>> {
        Ok(self
            .jobs
            .read()
            .await
            .get(job_id)
            .map(|record| record.status.clone()))
    }

    async fn get_record(&self, job_id: &str) -> anyhow::Result<Option<RenderJobRecord>> {
        Ok(self.jobs.read().await.get(job_id).cloned())
    }

    async fn mark_running(&self, job_id: &str) -> anyhow::Result<()> {
        let mut jobs = self.jobs.write().await;
        let record = jobs
            .get_mut(job_id)
            .ok_or_else(|| anyhow!("job not found: {job_id}"))?;
        record.status.state = RenderJobState::Running;
        record.status.updated_at_ms = now_ms();
        record.status.error = None;
        Ok(())
    }

    async fn mark_completed(&self, job_id: &str, artifact_key: String) -> anyhow::Result<()> {
        let mut jobs = self.jobs.write().await;
        let record = jobs
            .get_mut(job_id)
            .ok_or_else(|| anyhow!("job not found: {job_id}"))?;
        record.status.state = RenderJobState::Completed;
        record.status.updated_at_ms = now_ms();
        record.status.artifact_key = Some(artifact_key);
        record.status.error = None;
        Ok(())
    }

    async fn mark_failed(&self, job_id: &str, code: &str, message: String) -> anyhow::Result<()> {
        let mut jobs = self.jobs.write().await;
        let record = jobs
            .get_mut(job_id)
            .ok_or_else(|| anyhow!("job not found: {job_id}"))?;
        record.status.state = RenderJobState::Failed;
        record.status.updated_at_ms = now_ms();
        record.status.error = Some(JobFailure {
            code: code.to_string(),
            message,
        });
        Ok(())
    }
}

pub struct InMemoryJobQueue {
    queue: Mutex<VecDeque<String>>,
    notify: Notify,
}

impl InMemoryJobQueue {
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            notify: Notify::new(),
        }
    }
}

#[async_trait]
impl JobQueue for InMemoryJobQueue {
    async fn enqueue(&self, job_id: String) -> anyhow::Result<()> {
        self.queue.lock().await.push_back(job_id);
        self.notify.notify_one();
        Ok(())
    }

    async fn reserve(&self) -> anyhow::Result<String> {
        loop {
            if let Some(job_id) = self.queue.lock().await.pop_front() {
                return Ok(job_id);
            }

            self.notify.notified().await;
        }
    }
}

pub struct InMemoryObjectStore {
    objects: RwLock<HashMap<String, ObjectBlob>>,
}

impl InMemoryObjectStore {
    pub fn new() -> Self {
        Self {
            objects: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl ObjectStore for InMemoryObjectStore {
    async fn put(&self, key: String, blob: ObjectBlob) -> anyhow::Result<()> {
        self.objects.write().await.insert(key, blob);
        Ok(())
    }

    async fn get(&self, key: &str) -> anyhow::Result<Option<ObjectBlob>> {
        Ok(self.objects.read().await.get(key).cloned())
    }
}

pub fn default_job_services() -> (
    Arc<dyn JobStore>,
    Arc<dyn JobQueue>,
    Arc<dyn ObjectStore>,
) {
    (
        Arc::new(InMemoryJobStore::new()),
        Arc::new(InMemoryJobQueue::new()),
        Arc::new(InMemoryObjectStore::new()),
    )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
