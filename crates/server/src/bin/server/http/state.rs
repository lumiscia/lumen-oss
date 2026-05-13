use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use crate::server::{ApiRenderJob, RenderNotification, RenderQueueState};

#[derive(Clone)]
pub(super) struct AppState {
    pub api_token: Option<String>,
    pub progress_min_delta: f32,
    pub renders: Arc<RwLock<HashMap<String, StoredRender>>>,
    pub progress_tx: tokio::sync::broadcast::Sender<RenderNotification>,
    pub verbose_debug: bool,
}

impl AppState {
    pub fn new(api_token: Option<String>, progress_min_delta: f32, verbose_debug: bool) -> Self {
        Self {
            api_token,
            progress_min_delta: progress_min_delta.clamp(0.0, 1.0),
            renders: Arc::new(RwLock::new(HashMap::new())),
            progress_tx: tokio::sync::broadcast::channel(256).0,
            verbose_debug,
        }
    }
}

#[derive(Clone)]
pub(super) struct StoredRender {
    pub bytes: Option<Arc<Vec<u8>>>,
    pub last_progress_broadcast: Option<RenderQueueState>,
    pub progress: Option<RenderQueueState>,
    pub render: ApiRenderJob,
}
