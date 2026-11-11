use std::{
    collections::VecDeque,
    sync::mpsc::{self, TryRecvError},
};

use anyhow::{Context, anyhow};
use lumen_gpu::wgpu::{self, Buffer, BufferAsyncError, MapMode, PollType, SubmissionIndex};

/// Frames whose bytes can be on their way to the CPU at once. Each holds a frame-sized buffer.
const RING_SLOTS: usize = 3;

type MapResult = Result<(), BufferAsyncError>;

struct Pending {
    frame: u32,
    buffer: Buffer,
    submission: SubmissionIndex,
    mapped: mpsc::Receiver<MapResult>,
}

/// Copies converted frames into a fixed ring of buffers and maps them while the GPU renders on,
/// so the CPU reads a frame back [`RING_SLOTS`] frames after it was rendered instead of waiting
/// for it. Frames come out in the order they went in.
pub(super) struct ReadbackRing {
    size: u64,
    free: Vec<Buffer>,
    pending: VecDeque<Pending>,
}

impl ReadbackRing {
    /// A ring for frames of `size` bytes.
    pub(super) fn new(device: &wgpu::Device, size: u64) -> Self {
        let free = (0..RING_SLOTS)
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("lumen readback ring"),
                    size: size.max(4),
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                })
            })
            .collect();
        Self {
            size,
            free,
            pending: VecDeque::with_capacity(RING_SLOTS),
        }
    }

    /// Queues the copy of `source`, which holds `frame`, behind the work already submitted, so
    /// later work may rewrite `source`. With every slot in use it first waits for the oldest frame
    /// and returns its bytes.
    pub(super) fn push(
        &mut self,
        renderer: &lumen_gpu::Renderer,
        source: &Buffer,
        frame: u32,
    ) -> anyhow::Result<Option<(u32, Vec<u8>)>> {
        let oldest = if self.free.is_empty() {
            self.pop(renderer)?
        } else {
            None
        };
        let buffer = self.free.pop().expect("popping the oldest frees a slot");
        let mut encoder = renderer
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("lumen readback ring encoder"),
            });
        encoder.copy_buffer_to_buffer(source, 0, &buffer, 0, self.size);
        let submission = renderer.queue.submit([encoder.finish()]);
        let (sender, mapped) = mpsc::channel();
        buffer.slice(..).map_async(MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.pending.push_back(Pending {
            frame,
            buffer,
            submission,
            mapped,
        });
        Ok(oldest)
    }

    /// Waits for the oldest queued frame and returns its bytes, or `None` when nothing is queued.
    pub(super) fn pop(
        &mut self,
        renderer: &lumen_gpu::Renderer,
    ) -> anyhow::Result<Option<(u32, Vec<u8>)>> {
        let Some(pending) = self.pending.pop_front() else {
            return Ok(None);
        };
        renderer
            .device
            .poll(PollType::Wait {
                submission_index: Some(pending.submission),
                timeout: None,
            })
            .context("GPU readback poll failed")?;
        let result = match pending.mapped.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                renderer
                    .device
                    .poll(PollType::wait_indefinitely())
                    .context("GPU readback poll failed")?;
                pending
                    .mapped
                    .recv()
                    .map_err(|_| anyhow!("GPU readback callback closed"))?
            }
            Err(TryRecvError::Disconnected) => return Err(anyhow!("GPU readback callback closed")),
        };
        result.context("GPU readback map failed")?;
        let mapped = pending
            .buffer
            .slice(..)
            .get_mapped_range()
            .context("GPU readback map range failed")?;
        let bytes = mapped[..self.size as usize].to_vec();
        drop(mapped);
        pending.buffer.unmap();
        self.free.push(pending.buffer);
        Ok(Some((pending.frame, bytes)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(frame: u32, size: u64) -> Vec<u8> {
        (0..size as u32)
            .map(|index| (index.wrapping_mul(31) ^ frame.wrapping_mul(97)) as u8)
            .collect()
    }

    #[test]
    fn frames_come_back_in_order_from_a_reused_source() {
        let _gpu = crate::render::gpu_test_guard();
        let Ok(renderer) = pollster::block_on(lumen_gpu::Renderer::new()) else {
            eprintln!("skipping readback ring test: no GPU adapter");
            return;
        };
        let size = 37 * 4 * 5;
        let source = renderer.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut ring = ReadbackRing::new(&renderer.device, size);
        let frames = 3 * RING_SLOTS as u32 + 1;

        let mut read = Vec::new();
        for frame in 0..frames {
            renderer
                .queue
                .write_buffer(&source, 0, &pattern(frame, size));
            read.extend(ring.push(&renderer, &source, frame).unwrap());
            assert!(ring.pending.len() <= RING_SLOTS);
        }
        while let Some(next) = ring.pop(&renderer).unwrap() {
            read.push(next);
        }

        assert_eq!(read.len(), frames as usize);
        for (index, (frame, bytes)) in read.into_iter().enumerate() {
            assert_eq!(frame, index as u32);
            assert_eq!(bytes, pattern(frame, size), "frame {frame}");
        }
        assert_eq!(ring.free.len(), RING_SLOTS);
    }
}
