use std::{
    env,
    num::NonZeroUsize,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::body::Bytes;
use lru::LruCache;
use lumen::{plan::RenderPlan, sequence::Asset};
use tokio::sync::Mutex;

const DEFAULT_PLAN_CACHE_ITEMS: usize = 128;
const DEFAULT_FRAME_CACHE_ITEMS: usize = 1_024;
const DEFAULT_CACHE_TTL_MS: u64 = 5 * 60 * 1_000;

pub struct CompiledPreview {
    pub plan: Arc<RenderPlan>,
    pub assets: Vec<Asset>,
}

struct CachedFrame {
    bytes: Bytes,
    created_at_ms: u64,
}

pub struct PreviewCache {
    plans: Mutex<LruCache<String, Arc<CompiledPreview>>>,
    frames: Mutex<LruCache<String, CachedFrame>>,
    ttl_ms: u64,
}

impl PreviewCache {
    pub fn from_env() -> Self {
        Self::new(
            env_usize("LUMEN_PREVIEW_PLAN_CACHE_ITEMS", DEFAULT_PLAN_CACHE_ITEMS),
            env_usize("LUMEN_PREVIEW_FRAME_CACHE_ITEMS", DEFAULT_FRAME_CACHE_ITEMS),
            env_u64("LUMEN_PREVIEW_CACHE_TTL_MS", DEFAULT_CACHE_TTL_MS),
        )
    }

    pub fn new(plan_capacity: usize, frame_capacity: usize, ttl_ms: u64) -> Self {
        let plan_capacity = non_zero(plan_capacity.max(1));
        let frame_capacity = non_zero(frame_capacity.max(1));

        Self {
            plans: Mutex::new(LruCache::new(plan_capacity)),
            frames: Mutex::new(LruCache::new(frame_capacity)),
            ttl_ms,
        }
    }

    pub fn compiled_key(job_id: &str, version: u64) -> String {
        format!("{job_id}:{version}")
    }

    pub fn frame_key(job_id: &str, version: u64, frame_index: u64) -> String {
        format!("{job_id}:{version}:{frame_index}")
    }

    pub async fn get_compiled(&self, key: &str) -> Option<Arc<CompiledPreview>> {
        self.plans.lock().await.get(key).cloned()
    }

    pub async fn put_compiled(&self, key: String, compiled: Arc<CompiledPreview>) {
        self.plans.lock().await.put(key, compiled);
    }

    pub async fn get_frame(&self, key: &str) -> Option<Bytes> {
        let mut frames = self.frames.lock().await;
        let stale = frames
            .get(key)
            .map(|entry| now_ms().saturating_sub(entry.created_at_ms) > self.ttl_ms)
            .unwrap_or(false);
        if stale {
            frames.pop(key);
            return None;
        }

        frames.get(key).map(|entry| entry.bytes.clone())
    }

    pub async fn put_frame(&self, key: String, bytes: Bytes) {
        self.frames.lock().await.put(
            key,
            CachedFrame {
                bytes,
                created_at_ms: now_ms(),
            },
        );
    }
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

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn non_zero(value: usize) -> NonZeroUsize {
    match NonZeroUsize::new(value) {
        Some(value) => value,
        None => NonZeroUsize::MIN,
    }
}
