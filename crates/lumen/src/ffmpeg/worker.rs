use std::{
    env,
    sync::mpsc::{self, Receiver, SyncSender},
    thread::{self, JoinHandle},
};

use crate::render::backend::FrameImage;

use super::{FfmpegError, LibavStreamDecoder};

const DEFAULT_LIBAV_PREFETCH_QUEUE: usize = 8;
const DEFAULT_LIBAV_PREFETCH_FRAMES: u64 = 4;
const DEFAULT_ENCODE_QUEUE: usize = 8;

pub struct DecodeRequest {
    pub source_frame: u64,
    pub reply: SyncSender<Result<Option<FrameImage>, FfmpegError>>,
}

/// `VideoDecodeWorker` is the only access path to a `LibavStreamDecoder`.
///
/// Thread-safety contract:
/// - `LibavStreamDecoder` is moved into the worker thread at spawn time.
/// - All decode access happens via this bounded channel.
/// - `FrameImage` uses `Arc<Vec<u8>>`, so frame replies are cheap to clone between threads.
/// - Skia surfaces must remain render-thread-local and are never sent through this worker.
pub struct VideoDecodeWorker {
    source_id: String,
    tx: Option<SyncSender<DecodeRequest>>,
    handle: Option<JoinHandle<()>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Forward,
    Reverse,
    Random,
}

impl VideoDecodeWorker {
    pub fn spawn(source_id: &str, decoder: LibavStreamDecoder) -> Self {
        let queue_cap = env::var("LUMEN_LIBAV_PREFETCH_QUEUE")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_LIBAV_PREFETCH_QUEUE);
        let prefetch_frames = env::var("LUMEN_LIBAV_PREFETCH_FRAMES")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_LIBAV_PREFETCH_FRAMES);

        let (tx, rx) = mpsc::sync_channel(queue_cap);
        let source_id_owned = source_id.to_owned();
        let worker_source = source_id_owned.clone();
        let handle =
            thread::spawn(move || run_decode_worker(worker_source, decoder, rx, prefetch_frames));

        Self {
            source_id: source_id_owned,
            tx: Some(tx),
            handle: Some(handle),
        }
    }

    pub fn get_frame(&self, source_frame: u64) -> Result<Option<FrameImage>, FfmpegError> {
        let tx = self
            .tx
            .as_ref()
            .ok_or_else(|| FfmpegError::WorkerUnavailable(self.source_id.clone()))?;
        let (reply_tx, reply_rx) = mpsc::sync_channel::<Result<Option<FrameImage>, FfmpegError>>(1);
        tx.send(DecodeRequest {
            source_frame,
            reply: reply_tx,
        })
        .map_err(|_| FfmpegError::WorkerUnavailable(self.source_id.clone()))?;
        reply_rx
            .recv()
            .map_err(|_| FfmpegError::WorkerResponseDropped(self.source_id.clone()))?
    }
}

impl Drop for VideoDecodeWorker {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn run_decode_worker(
    _source_id: String,
    mut decoder: LibavStreamDecoder,
    rx: Receiver<DecodeRequest>,
    prefetch_frames: u64,
) {
    let mut last_requested: Option<u64> = None;

    while let Ok(request) = rx.recv() {
        let frame = request.source_frame;
        let direction = match last_requested {
            Some(last) if frame == last.saturating_add(1) => Direction::Forward,
            Some(last) if frame.saturating_add(1) == last => Direction::Reverse,
            _ => Direction::Random,
        };
        let result = decoder.get_frame(frame);
        let should_prefetch = prefetch_frames > 0 && matches!(result, Ok(Some(_)));
        let _ = request.reply.send(result);

        if should_prefetch && direction != Direction::Random {
            for step in 1..=prefetch_frames {
                let next = match direction {
                    Direction::Forward => frame.checked_add(step),
                    Direction::Reverse => frame.checked_sub(step),
                    Direction::Random => None,
                };

                match next {
                    Some(next_frame) => match decoder.get_frame(next_frame) {
                        Ok(Some(_)) => {}
                        Ok(None) | Err(_) => break,
                    },
                    None => break,
                }
            }
        }

        last_requested = Some(frame);
    }
}

pub fn render_to_mp4<R, E, P>(
    total_frames: u32,
    mut render_frame: R,
    encode_rgba_stream: E,
    mut on_progress: P,
) -> Result<(), FfmpegError>
where
    R: FnMut(u32) -> Result<Vec<u8>, FfmpegError>,
    E: FnOnce(Receiver<Vec<u8>>) -> Result<(), FfmpegError> + Send + 'static,
    P: FnMut(u32, u32),
{
    let queue_cap = env::var("LUMEN_ENCODE_QUEUE")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_ENCODE_QUEUE);
    let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(queue_cap);
    let encode_handle = thread::spawn(move || encode_rgba_stream(rx));

    for frame in 0..total_frames {
        let rgba = render_frame(frame)?;
        tx.send(rgba)
            .map_err(|_| FfmpegError::EncodeChannelClosed)?;
        on_progress(frame.saturating_add(1), total_frames);
    }

    drop(tx);
    let encode_result = encode_handle
        .join()
        .map_err(|_| FfmpegError::EncodeThreadPanic)?;
    encode_result
}
