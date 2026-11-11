use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::{Context, anyhow};
use lumen_ffmpeg::QualityConfig;
use lumen_server::render::{RenderConfig, SequenceLimits};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::server::{
    MediaClient, QueueSpot, RenderJobError, RenderRecord, RenderStatus, assets::AssetStore,
};

use super::Args;

#[derive(Clone)]
pub(super) struct AppState {
    pub api_token: Option<String>,
    pub assets: AssetStore,
    pub media_client: MediaClient,
    pub progress_min_delta: f32,
    pub max_text_glyphs: usize,
    pub quality: Arc<QualityConfig>,
    pub sequence_limits: SequenceLimits,
    pub render_config: RenderConfig,
    pub render_slots: Arc<Semaphore>,
    queued_renders: Arc<AtomicUsize>,
    max_queued_renders: usize,
    renders: Arc<Mutex<RenderRegistry>>,
    retained_renders: usize,
}

impl AppState {
    /// Fails when the quality config doesn't load or the media directory can't be opened.
    pub fn new(args: Args, render_config: RenderConfig) -> anyhow::Result<Self> {
        let assets = AssetStore::open(
            &args.media_dir,
            args.media_ttl,
            args.media_max_bytes,
            args.media_max_pending.get(),
        )
        .with_context(|| {
            format!(
                "failed to open the media directory `{}`",
                args.media_dir.display()
            )
        })?;
        let quality = QualityConfig::load(
            args.quality_config
                .as_deref()
                .filter(|path| !path.as_os_str().is_empty()),
        )
        .map_err(|error| anyhow!(error))?;
        Ok(Self {
            api_token: args.token,
            assets,
            media_client: MediaClient::new(args.allowed_media_hosts)?,
            progress_min_delta: args.progress_min_delta,
            max_text_glyphs: args.max_text_glyphs as usize,
            quality: Arc::new(quality),
            sequence_limits: SequenceLimits {
                max_frames: args.max_sequence_frames,
                max_bytes: args.max_sequence_bytes,
            },
            render_config,
            render_slots: Arc::new(Semaphore::new(args.max_concurrent_renders.get())),
            queued_renders: Arc::new(AtomicUsize::new(0)),
            max_queued_renders: args.max_queued_renders,
            renders: Arc::new(Mutex::new(RenderRegistry::default())),
            retained_renders: args.retained_renders.get(),
        })
    }

    /// Takes a render slot if one is free, and otherwise a place in the queue for one. `None` when
    /// `--max-queued-renders` renders are already waiting.
    pub fn reserve_render(&self) -> Option<RenderTicket> {
        if let Ok(permit) = Arc::clone(&self.render_slots).try_acquire_owned() {
            return Some(RenderTicket::Running(permit));
        }
        self.queued_renders
            .try_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
                (queued < self.max_queued_renders).then_some(queued + 1)
            })
            .ok()?;
        Some(RenderTicket::Waiting(QueueSpot::new(Arc::clone(
            &self.queued_renders,
        ))))
    }

    pub fn max_queued_renders(&self) -> usize {
        self.max_queued_renders
    }

    pub fn insert(&self, record: RenderRecord) -> Arc<RenderRecord> {
        let record = Arc::new(record);
        self.registry()
            .records
            .insert(record.id.clone(), Arc::clone(&record));
        record
    }

    pub fn get(&self, id: &str) -> Option<Arc<RenderRecord>> {
        self.registry().records.get(id).cloned()
    }

    /// Ends a render in `status`, which every current and later subscriber sees, and drops the
    /// oldest finished renders beyond the retained count.
    pub fn finish(&self, record: &RenderRecord, status: RenderStatus) {
        let mut registry = self.registry();
        record.status.send_replace(status);
        registry.retain_finished(&record.id, self.retained_renders);
    }

    /// Ends a render as failed at the progress it had reached, or as cancelled when `error` says
    /// it was.
    pub fn fail(&self, record: &RenderRecord, error: RenderJobError) {
        let progress = record.status.borrow().progress();
        let status = if error.is_cancelled() {
            RenderStatus::Cancelled { error, progress }
        } else {
            RenderStatus::Failed { error, progress }
        };
        self.finish(record, status);
    }

    /// Asks a render that hasn't ended to stop. A queued render ends here and never starts, and
    /// its place in the queue is free before it ends; a running one ends when its task sees the
    /// request, at its next frame boundary.
    pub fn cancel(&self, record: &RenderRecord) {
        if record.status.borrow().is_terminal() {
            return;
        }
        record.cancellation.cancel();
        let mut registry = self.registry();
        let ended = record.status.send_if_modified(|status| {
            let queued = matches!(status, RenderStatus::Queued);
            if queued {
                record.release_queue_spot();
                *status = RenderStatus::Cancelled {
                    error: RenderJobError::cancelled(),
                    progress: 0.0,
                };
            }
            queued
        });
        if ended {
            registry.retain_finished(&record.id, self.retained_renders);
        }
    }

    fn registry(&self) -> MutexGuard<'_, RenderRegistry> {
        self.renders.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// What lets a render start: a free slot, or a place in the queue for one.
pub(super) enum RenderTicket {
    Running(OwnedSemaphorePermit),
    Waiting(QueueSpot),
}

#[derive(Default)]
struct RenderRegistry {
    records: HashMap<String, Arc<RenderRecord>>,
    finished: VecDeque<String>,
}

impl RenderRegistry {
    fn retain_finished(&mut self, id: &str, retained: usize) {
        self.finished.push_back(id.to_string());
        while self.finished.len() > retained {
            if let Some(oldest) = self.finished.pop_front() {
                self.records.remove(&oldest);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    fn state(args: &[&str]) -> AppState {
        let media = tempfile::tempdir().unwrap();
        let media = media.path().to_str().unwrap();
        AppState::new(
            Args::parse_from(
                ["lumen-server", "--media-dir", media]
                    .into_iter()
                    .chain(args.iter().copied()),
            ),
            RenderConfig::default(),
        )
        .unwrap()
    }

    fn record(id: &str) -> RenderRecord {
        RenderRecord::new(id.to_string(), String::new(), String::new())
    }

    fn failure() -> RenderJobError {
        RenderJobError {
            code: "render_failed".to_string(),
            message: "failed".to_string(),
            retryable: false,
        }
    }

    #[tokio::test]
    async fn a_subscriber_still_gets_the_end_of_a_render_that_was_evicted() {
        let state = state(&["--retained-renders", "1"]);
        let first = state.insert(record("first"));
        let mut socket = first.status.subscribe();

        state.fail(&first, failure());
        let second = state.insert(record("second"));
        state.fail(&second, failure());
        drop(first);

        assert!(state.get("first").is_none());
        socket.changed().await.unwrap();
        assert!(socket.borrow_and_update().is_terminal());
    }

    #[test]
    fn cancelling_a_queued_render_frees_its_place_in_the_queue_at_once() {
        let state = state(&["--max-concurrent-renders", "1", "--max-queued-renders", "1"]);
        let _running = state.reserve_render().unwrap();
        let Some(RenderTicket::Waiting(spot)) = state.reserve_render() else {
            panic!("the slot is taken, so the render queues");
        };
        let queued = state.insert(record("queued"));
        queued.hold_queue_spot(spot);
        assert!(state.reserve_render().is_none(), "the queue is full");

        state.cancel(&queued);
        state.cancel(&queued);

        assert!(matches!(
            state.reserve_render(),
            Some(RenderTicket::Waiting(_))
        ));
        assert!(queued.status.borrow().is_terminal());
    }

    #[test]
    fn a_failed_render_keeps_the_progress_it_reached() {
        let state = state(&[]);
        let record = state.insert(record("r"));
        let mut woken_at = 0.0;
        record.set_frame(50, 100, 0.02, &mut woken_at);
        let reached = record.status.borrow().progress();

        state.fail(&record, failure());

        assert!(reached > 0.05 && reached < 0.99, "{reached}");
        assert_eq!(record.status.borrow().progress(), reached);
    }

    #[test]
    fn frames_wake_sockets_per_stage_and_delta_but_always_update_the_status() {
        let record = record("r");
        let mut socket = record.status.subscribe();
        let mut woken_at = 0.0;

        record.set_frame(1, 100, 0.1, &mut woken_at);
        assert!(
            socket.has_changed().unwrap(),
            "the first frame changes the stage"
        );
        socket.borrow_and_update();

        record.set_frame(2, 100, 0.1, &mut woken_at);
        assert!(!socket.has_changed().unwrap(), "a small step stays quiet");
        assert!(matches!(
            *socket.borrow(),
            RenderStatus::Rendering { frame: 2, .. }
        ));

        record.set_frame(20, 100, 0.1, &mut woken_at);
        assert!(socket.has_changed().unwrap(), "a large step wakes sockets");
    }
}
