use std::sync::Arc;

use crate::jobs::{JobQueue, JobStore, ObjectStore, default_job_services};
use crate::preview_cache::PreviewCache;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub secret: Arc<str>,
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub job_store: Arc<dyn JobStore>,
    pub job_queue: Arc<dyn JobQueue>,
    pub object_store: Arc<dyn ObjectStore>,
    pub preview_cache: Arc<PreviewCache>,
}

impl AppState {
    pub fn with_defaults(secret: String) -> Self {
        let (job_store, job_queue, object_store) = default_job_services();

        Self {
            config: Arc::new(AppConfig {
                secret: Arc::<str>::from(secret),
            }),
            job_store,
            job_queue,
            object_store,
            preview_cache: Arc::new(PreviewCache::from_env()),
        }
    }
}
