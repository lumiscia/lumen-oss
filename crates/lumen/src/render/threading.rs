//! Multithreaded frame-parallel render orchestration.

use std::collections::HashMap;

use crossbeam_channel::bounded;

use crate::{
    composition::Composition,
    error::LumenError,
    media::MediaStore,
    raster::{BitmapFrame, RasterFrame},
    render::{LumenRenderer, SurfacePool},
    sink::Sink,
};

#[derive(Debug)]
pub struct RenderOrchestrator<S: SurfacePool, M: MediaStore> {
    composition: Composition,
    surface_pool: S,
    media_store: M,

    worker_count: usize,
}

impl<S: SurfacePool, M: MediaStore> RenderOrchestrator<S, M> {
    pub fn new(
        composition: Composition,
        surface_pool: S,
        media_store: M,
        worker_count: usize,
    ) -> Self {
        Self {
            composition,
            surface_pool,
            media_store,
            worker_count,
        }
    }

    // TODO: handle cancellation with atomic bool
    pub fn render<T: Sink>(&self, sink: &mut T) -> Result<(), LumenError> {
        let (job_tx, job_rx) = bounded::<u32>(self.worker_count);
        let (result_tx, result_rx) = bounded::<WorkerResult>(self.worker_count);

        std::thread::scope(|s| -> Result<(), LumenError> {
            for _ in 0..self.worker_count {
                let composition = &self.composition;
                let surface_pool = &self.surface_pool;
                let media_store = &self.media_store;

                let result_tx = result_tx.clone();
                let job_rx = job_rx.clone();

                s.spawn(move || {
                    // TODO: remove unwraps & panics
                    let mut renderer =
                        LumenRenderer::new(composition, surface_pool, media_store).unwrap();

                    while let Ok(frame) = job_rx.recv() {
                        match renderer.render(frame) {
                            Ok(rendered) => {
                                result_tx.send(WorkerResult::Frame(frame, rendered));
                            }
                            Err(err) => {
                                panic!("{:?}", err);
                            }
                        };
                    }
                });
            }

            if self.worker_count == 0 {
                return Err(LumenError::Threading(
                    crate::error::ThreadingError::WorkerInit {
                        details: "worker_count must be greater than zero".to_string(),
                    },
                ));
            }

            let total_frames = self.composition.timeline.duration_frames;
            let mut next_to_submit = 0;
            let mut next_to_write = 0;
            let mut in_flight = 0usize;
            let mut buffered_frames = HashMap::with_capacity(self.worker_count);

            while next_to_write < total_frames {
                while in_flight < self.worker_count && next_to_submit < total_frames {
                    job_tx.send(next_to_submit).map_err(|_| {
                        LumenError::Threading(crate::error::ThreadingError::WorkerFailure {
                            frame: Some(next_to_submit),
                            details: "job channel closed while submitting frame".to_string(),
                        })
                    })?;

                    next_to_submit += 1;
                    in_flight += 1;
                }

                let result = result_rx.recv().map_err(|_| {
                    LumenError::Threading(crate::error::ThreadingError::WorkerFailure {
                        frame: Some(next_to_write),
                        details: "result channel closed before all frames completed".to_string(),
                    })
                })?;

                in_flight = in_flight.saturating_sub(1);

                match result {
                    WorkerResult::Frame(frame, bitmap_frame) => {
                        buffered_frames.insert(frame, bitmap_frame);

                        while let Some(bitmap_frame) = buffered_frames.remove(&next_to_write) {
                            let raster_frame = RasterFrame::Bitmap(bitmap_frame);
                            sink.write_frame(next_to_write, &raster_frame)?;
                            next_to_write += 1;
                        }
                    }
                    WorkerResult::Error(_, err) => return Err(err),
                }
            }

            drop(job_tx);
            Ok(())
        })?;

        // todo: submit jobs & handle surface pool returns
        Ok(())
    }
}

enum WorkerResult {
    Frame(u32, BitmapFrame),
    Error(u32, LumenError),
}
