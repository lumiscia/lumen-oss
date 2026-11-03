use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, RwLock},
};

use axum::body::Bytes;
use tokio::sync::Semaphore;

use crate::server::{ApiRenderJob, MediaClient, RenderNotification, RenderQueueState};

use super::Args;

#[derive(Clone)]
pub(super) struct AppState {
    pub api_token: Option<String>,
    pub media_client: MediaClient,
    pub progress_min_delta: f32,
    pub render_slots: Arc<Semaphore>,
    pub renders: Arc<RwLock<RenderRegistry>>,
    pub retained_renders: usize,
    pub progress_tx: tokio::sync::broadcast::Sender<RenderNotification>,
}

impl AppState {
    pub fn new(args: Args) -> anyhow::Result<Self> {
        Ok(Self {
            api_token: args.token,
            media_client: MediaClient::new(args.allowed_media_hosts)?,
            progress_min_delta: args.progress_min_delta.clamp(0.0, 1.0),
            render_slots: Arc::new(Semaphore::new(args.max_concurrent_renders.get())),
            renders: Arc::new(RwLock::new(RenderRegistry::default())),
            retained_renders: args.retained_renders.get(),
            progress_tx: tokio::sync::broadcast::channel(256).0,
        })
    }
}

#[derive(Default)]
pub(super) struct RenderRegistry {
    pub records: HashMap<String, StoredRender>,
    finished: VecDeque<String>,
}

impl RenderRegistry {
    /// Marks a render finished and drops the oldest finished renders beyond `retained`.
    pub fn retain_finished(&mut self, id: &str, retained: usize) {
        self.finished.push_back(id.to_string());
        while self.finished.len() > retained {
            if let Some(oldest) = self.finished.pop_front() {
                self.records.remove(&oldest);
            }
        }
    }
}

pub(super) struct StoredRender {
    pub bytes: Option<Bytes>,
    pub last_progress_broadcast: Option<RenderQueueState>,
    pub progress: Option<RenderQueueState>,
    pub render: ApiRenderJob,
}
