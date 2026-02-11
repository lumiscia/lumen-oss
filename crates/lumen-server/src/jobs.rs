use std::{
    collections::{HashMap, VecDeque},
    env,
    fmt::{Display, Formatter},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use axum::body::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{Mutex, Notify, RwLock};

const DEFAULT_MAX_JOBS: usize = 2_048;
const DEFAULT_MAX_QUEUE: usize = 2_048;
const DEFAULT_MAX_OBJECTS: usize = 1_024;
const DEFAULT_MAX_OBJECT_BYTES: usize = 512 * 1024 * 1024;
const DEFAULT_TERMINAL_TTL_MS: u64 = 30 * 60 * 1_000;

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
    pub bytes: Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageError {
    CapacityExceeded {
        resource: &'static str,
        limit: usize,
    },
    NotFound {
        resource: &'static str,
        id: String,
    },
}

impl Display for StorageError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CapacityExceeded { resource, limit } => {
                write!(f, "{resource} capacity exceeded (limit: {limit})")
            }
            Self::NotFound { resource, id } => write!(f, "{resource} not found: {id}"),
        }
    }
}

impl std::error::Error for StorageError {}

pub type StorageResult<T> = Result<T, StorageError>;

#[async_trait]
pub trait JobStore: Send + Sync {
    async fn create(&self, payload: Value) -> StorageResult<RenderJobStatus>;
    async fn get_status(&self, job_id: &str) -> StorageResult<Option<RenderJobStatus>>;
    async fn get_record(&self, job_id: &str) -> StorageResult<Option<RenderJobRecord>>;
    async fn mark_running(&self, job_id: &str) -> StorageResult<()>;
    async fn mark_completed(&self, job_id: &str, artifact_key: String) -> StorageResult<()>;
    async fn mark_failed(&self, job_id: &str, code: &str, message: String) -> StorageResult<()>;
}

#[async_trait]
pub trait JobQueue: Send + Sync {
    async fn enqueue(&self, job_id: String) -> StorageResult<()>;
    async fn reserve(&self) -> StorageResult<String>;
}

#[async_trait]
pub trait ObjectStore: Send + Sync {
    async fn put(&self, key: String, blob: ObjectBlob) -> StorageResult<()>;
    async fn get(&self, key: &str) -> StorageResult<Option<ObjectBlob>>;
}

pub struct InMemoryJobStore {
    id_counter: AtomicU64,
    jobs: RwLock<HashMap<String, RenderJobRecord>>,
    max_jobs: usize,
    terminal_ttl_ms: u64,
}

impl InMemoryJobStore {
    pub fn new() -> Self {
        Self::with_limits(
            env_usize("LUMEN_MAX_JOBS", DEFAULT_MAX_JOBS),
            env_u64("LUMEN_TERMINAL_TTL_MS", DEFAULT_TERMINAL_TTL_MS),
        )
    }

    pub fn with_limits(max_jobs: usize, terminal_ttl_ms: u64) -> Self {
        Self {
            id_counter: AtomicU64::new(1),
            jobs: RwLock::new(HashMap::new()),
            max_jobs: max_jobs.max(1),
            terminal_ttl_ms,
        }
    }

    async fn cleanup_terminal_expired(&self) {
        if self.terminal_ttl_ms == 0 {
            return;
        }

        let now = now_ms();
        let mut jobs = self.jobs.write().await;
        jobs.retain(|_, record| {
            !is_terminal(record.status.state)
                || now.saturating_sub(record.status.updated_at_ms) <= self.terminal_ttl_ms
        });
    }
}

#[async_trait]
impl JobStore for InMemoryJobStore {
    async fn create(&self, payload: Value) -> StorageResult<RenderJobStatus> {
        self.cleanup_terminal_expired().await;

        let mut jobs = self.jobs.write().await;
        if jobs.len() >= self.max_jobs {
            return Err(StorageError::CapacityExceeded {
                resource: "jobs",
                limit: self.max_jobs,
            });
        }

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

        jobs.insert(
            status.job_id.clone(),
            RenderJobRecord {
                status: status.clone(),
                payload,
            },
        );

        Ok(status)
    }

    async fn get_status(&self, job_id: &str) -> StorageResult<Option<RenderJobStatus>> {
        self.cleanup_terminal_expired().await;
        Ok(self
            .jobs
            .read()
            .await
            .get(job_id)
            .map(|record| record.status.clone()))
    }

    async fn get_record(&self, job_id: &str) -> StorageResult<Option<RenderJobRecord>> {
        self.cleanup_terminal_expired().await;
        Ok(self.jobs.read().await.get(job_id).cloned())
    }

    async fn mark_running(&self, job_id: &str) -> StorageResult<()> {
        self.cleanup_terminal_expired().await;
        let mut jobs = self.jobs.write().await;
        let record = jobs.get_mut(job_id).ok_or_else(|| StorageError::NotFound {
            resource: "job",
            id: job_id.to_string(),
        })?;
        record.status.state = RenderJobState::Running;
        record.status.updated_at_ms = now_ms();
        record.status.error = None;
        Ok(())
    }

    async fn mark_completed(&self, job_id: &str, artifact_key: String) -> StorageResult<()> {
        self.cleanup_terminal_expired().await;
        let mut jobs = self.jobs.write().await;
        let record = jobs.get_mut(job_id).ok_or_else(|| StorageError::NotFound {
            resource: "job",
            id: job_id.to_string(),
        })?;
        record.status.state = RenderJobState::Completed;
        record.status.updated_at_ms = now_ms();
        record.status.artifact_key = Some(artifact_key);
        record.status.error = None;
        Ok(())
    }

    async fn mark_failed(&self, job_id: &str, code: &str, message: String) -> StorageResult<()> {
        self.cleanup_terminal_expired().await;
        let mut jobs = self.jobs.write().await;
        let record = jobs.get_mut(job_id).ok_or_else(|| StorageError::NotFound {
            resource: "job",
            id: job_id.to_string(),
        })?;
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
    max_queue: usize,
}

impl InMemoryJobQueue {
    pub fn new() -> Self {
        Self::with_limit(env_usize("LUMEN_MAX_QUEUE", DEFAULT_MAX_QUEUE))
    }

    pub fn with_limit(max_queue: usize) -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            notify: Notify::new(),
            max_queue: max_queue.max(1),
        }
    }
}

#[async_trait]
impl JobQueue for InMemoryJobQueue {
    async fn enqueue(&self, job_id: String) -> StorageResult<()> {
        let mut queue = self.queue.lock().await;
        if queue.len() >= self.max_queue {
            return Err(StorageError::CapacityExceeded {
                resource: "queue",
                limit: self.max_queue,
            });
        }

        queue.push_back(job_id);
        drop(queue);
        self.notify.notify_one();
        Ok(())
    }

    async fn reserve(&self) -> StorageResult<String> {
        loop {
            if let Some(job_id) = self.queue.lock().await.pop_front() {
                return Ok(job_id);
            }

            self.notify.notified().await;
        }
    }
}

struct StoredObject {
    blob: ObjectBlob,
    created_at_ms: u64,
}

pub struct InMemoryObjectStore {
    objects: RwLock<HashMap<String, StoredObject>>,
    max_objects: usize,
    max_total_bytes: usize,
    ttl_ms: u64,
}

impl InMemoryObjectStore {
    pub fn new() -> Self {
        Self::with_limits(
            env_usize("LUMEN_MAX_OBJECTS", DEFAULT_MAX_OBJECTS),
            env_usize("LUMEN_MAX_OBJECT_BYTES", DEFAULT_MAX_OBJECT_BYTES),
            env_u64("LUMEN_TERMINAL_TTL_MS", DEFAULT_TERMINAL_TTL_MS),
        )
    }

    pub fn with_limits(max_objects: usize, max_total_bytes: usize, ttl_ms: u64) -> Self {
        Self {
            objects: RwLock::new(HashMap::new()),
            max_objects: max_objects.max(1),
            max_total_bytes: max_total_bytes.max(1),
            ttl_ms,
        }
    }

    async fn cleanup_expired(&self) {
        if self.ttl_ms == 0 {
            return;
        }

        let now = now_ms();
        let mut objects = self.objects.write().await;
        objects.retain(|_, stored| now.saturating_sub(stored.created_at_ms) <= self.ttl_ms);
    }
}

#[async_trait]
impl ObjectStore for InMemoryObjectStore {
    async fn put(&self, key: String, blob: ObjectBlob) -> StorageResult<()> {
        self.cleanup_expired().await;
        let mut objects = self.objects.write().await;

        let existing_size = objects
            .get(&key)
            .map(|stored| stored.blob.bytes.len())
            .unwrap_or(0);
        if existing_size == 0 && objects.len() >= self.max_objects {
            return Err(StorageError::CapacityExceeded {
                resource: "objects",
                limit: self.max_objects,
            });
        }

        let current_bytes: usize = objects.values().map(|stored| stored.blob.bytes.len()).sum();
        let projected_bytes = current_bytes
            .saturating_sub(existing_size)
            .saturating_add(blob.bytes.len());
        if projected_bytes > self.max_total_bytes {
            return Err(StorageError::CapacityExceeded {
                resource: "object_bytes",
                limit: self.max_total_bytes,
            });
        }

        objects.insert(
            key,
            StoredObject {
                blob,
                created_at_ms: now_ms(),
            },
        );
        Ok(())
    }

    async fn get(&self, key: &str) -> StorageResult<Option<ObjectBlob>> {
        self.cleanup_expired().await;
        Ok(self
            .objects
            .read()
            .await
            .get(key)
            .map(|stored| stored.blob.clone()))
    }
}

pub fn default_job_services() -> (Arc<dyn JobStore>, Arc<dyn JobQueue>, Arc<dyn ObjectStore>) {
    (
        Arc::new(InMemoryJobStore::new()),
        Arc::new(InMemoryJobQueue::new()),
        Arc::new(InMemoryObjectStore::new()),
    )
}

fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default)
}

fn is_terminal(state: RenderJobState) -> bool {
    matches!(state, RenderJobState::Completed | RenderJobState::Failed)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
