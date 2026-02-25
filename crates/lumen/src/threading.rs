//! Multithreaded frame-parallel render orchestration.

use std::{collections::BTreeMap, ops::Range, sync::Arc, thread};

use crossbeam_channel::{Receiver, Sender, bounded};

use crate::{
    composition::Composition,
    error::{LumenError, RenderError},
    raster::RasterFrame,
    render::RenderContext,
    sink::Sink,
};

#[derive(Debug, Clone)]
pub struct RenderWorkerPool {
    pub worker_count: usize,
}

impl RenderWorkerPool {
    pub fn new(worker_count: usize) -> Self {
        Self {
            worker_count: worker_count.max(1),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RenderOrchestrator {
    pub workers: RenderWorkerPool,
}

impl RenderOrchestrator {
    pub fn new(worker_count: usize) -> Self {
        Self {
            workers: RenderWorkerPool::new(worker_count),
        }
    }

    fn spawn_workers(
        &self,
        composition: Arc<Composition>,
        template_context: &RenderContext,
        job_rx: Receiver<u32>,
        result_tx: Sender<WorkerResult>,
    ) -> Vec<thread::JoinHandle<()>> {
        let mut handles = Vec::with_capacity(self.workers.worker_count);
        for _ in 0..self.workers.worker_count {
            let composition = Arc::clone(&composition);
            let job_rx = job_rx.clone();
            let result_tx = result_tx.clone();
            let asset_cache = Arc::clone(&template_context.asset_cache);
            let media_store = Arc::clone(&template_context.media_store);
            let capability_profile = template_context.capability_profile.clone();
            let cancellation = template_context.cancellation.clone();

            handles.push(thread::spawn(move || {
                #[allow(clippy::arc_with_non_send_sync)]
                let surface_pool = Arc::new(crate::surface_pool::SurfacePool::new());
                let mut context = RenderContext::new(
                    &composition,
                    surface_pool,
                    asset_cache,
                    media_store,
                    capability_profile,
                );
                context.cancellation = cancellation.clone();

                while let Ok(frame) = job_rx.recv() {
                    if cancellation.is_cancelled() {
                        break;
                    }
                    match composition.render_frame(frame, &mut context) {
                        Ok(rendered) => match rendered.to_bitmap() {
                            Ok(RasterFrame::Bitmap(bitmap)) => {
                                if result_tx
                                    .send(WorkerResult::Frame {
                                        frame,
                                        pixels: bitmap.pixels,
                                        width: bitmap.storage_width,
                                        height: bitmap.storage_height,
                                    })
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Ok(RasterFrame::Surface(_)) => {
                                cancellation.cancel();
                                let _ = result_tx.send(WorkerResult::Error(
                                    frame,
                                    RenderError::InvalidMediaOutputType {
                                        frame,
                                        node_id: crate::node::NodeId(0),
                                    }
                                    .into(),
                                ));
                                break;
                            }
                            Err(error) => {
                                cancellation.cancel();
                                let _ = result_tx.send(WorkerResult::Error(frame, error));
                                break;
                            }
                        },
                        Err(error) => {
                            cancellation.cancel();
                            let _ = result_tx.send(WorkerResult::Error(frame, error));
                            break;
                        }
                    }
                }
            }));
        }
        handles
    }
}

impl Composition {
    pub fn render_sequence(
        &self,
        frame_range: Range<u32>,
        context: RenderContext,
        mut sink: Box<dyn Sink>,
        worker_count: usize,
    ) -> Result<(), LumenError> {
        let frames: Vec<u32> = frame_range.collect();
        if frames.is_empty() {
            sink.finalize()?;
            return Ok(());
        }

        let orchestrator = RenderOrchestrator::new(worker_count.min(frames.len()));
        let composition = Arc::new(self.clone());
        let (job_tx, job_rx) = bounded::<u32>(orchestrator.workers.worker_count * 2);
        let (result_tx, result_rx) = bounded::<WorkerResult>(orchestrator.workers.worker_count * 2);

        let handles = orchestrator.spawn_workers(
            Arc::clone(&composition),
            &context,
            job_rx,
            result_tx.clone(),
        );
        drop(result_tx);

        let scheduled_frames = frames.clone();
        let producer_cancellation = context.cancellation.clone();
        let producer = thread::spawn(move || {
            for frame in scheduled_frames {
                if producer_cancellation.is_cancelled() {
                    break;
                }
                if job_tx.send(frame).is_err() {
                    break;
                }
            }
        });

        let mut written = 0usize;
        let mut next_frame = *frames.first().unwrap_or(&0);
        let last_frame = *frames.last().unwrap_or(&0);
        let mut reorder_buffer: BTreeMap<u32, BufferedFrame> = BTreeMap::new();
        let mut first_error: Option<LumenError> = None;

        let mut sink_error: Option<LumenError> = None;

        'results: while written < frames.len() {
            let result = match result_rx.recv() {
                Ok(result) => result,
                Err(_) => break,
            };

            match result {
                WorkerResult::Frame {
                    frame,
                    pixels,
                    width,
                    height,
                } => {
                    if frame == next_frame {
                        if let Err(error) = sink.write_frame(
                            frame,
                            &RasterFrame::bitmap(Arc::clone(&pixels), width, height),
                        ) {
                            context.cancellation.cancel();
                            sink_error = Some(error.into());
                            break 'results;
                        }
                        written += 1;
                        if next_frame >= last_frame {
                            break;
                        }
                        next_frame += 1;
                        while let Some(buffered) = reorder_buffer.remove(&next_frame) {
                            if let Err(error) = sink.write_frame(
                                next_frame,
                                &RasterFrame::bitmap(
                                    Arc::clone(&buffered.pixels),
                                    buffered.width,
                                    buffered.height,
                                ),
                            ) {
                                context.cancellation.cancel();
                                sink_error = Some(error.into());
                                break 'results;
                            }
                            written += 1;
                            if next_frame >= last_frame {
                                break;
                            }
                            next_frame += 1;
                        }
                    } else {
                        reorder_buffer.insert(
                            frame,
                            BufferedFrame {
                                pixels,
                                width,
                                height,
                            },
                        );
                    }
                }
                WorkerResult::Error(frame, error) => {
                    context.cancellation.cancel();
                    first_error.get_or_insert(match error {
                        LumenError::Render(RenderError::Cancelled { .. }) => {
                            RenderError::Cancelled { frame }.into()
                        }
                        other => other,
                    });
                    break;
                }
            }
        }

        drop(result_rx);
        let _ = producer.join();
        for handle in handles {
            let _ = handle.join();
        }

        let finalize_result = sink.finalize();

        if let Some(error) = sink_error {
            let _ = finalize_result;
            Err(error)
        } else if let Some(error) = first_error {
            let _ = finalize_result;
            Err(error)
        } else if context.cancellation.is_cancelled() {
            let _ = finalize_result;
            Err(RenderError::Cancelled { frame: next_frame }.into())
        } else {
            finalize_result?;
            Ok(())
        }
    }
}

struct BufferedFrame {
    pixels: Arc<Vec<u8>>,
    width: u32,
    height: u32,
}

enum WorkerResult {
    Frame {
        frame: u32,
        pixels: Arc<Vec<u8>>,
        width: u32,
        height: u32,
    },
    Error(u32, LumenError),
}
