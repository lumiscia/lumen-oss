use std::{
    collections::VecDeque,
    sync::mpsc::{self, TryRecvError},
};

use anyhow::{Context, anyhow};
use lumen_gpu::{
    Size,
    wgpu::{
        self, Buffer, BufferAsyncError, MapMode, PollType, SubmissionIndex, TexelCopyBufferInfo,
        TexelCopyBufferLayout, Texture,
    },
};

/// Frames whose pixels can be on their way to the CPU at once. Each holds a frame-sized buffer.
const RING_SLOTS: usize = 3;

type MapResult = Result<(), BufferAsyncError>;

struct Pending {
    frame: u32,
    buffer: Buffer,
    submission: SubmissionIndex,
    mapped: mpsc::Receiver<MapResult>,
}

/// Copies rendered frames into a fixed ring of buffers and maps them while the GPU renders on, so
/// the CPU reads a frame back [`RING_SLOTS`] frames after it was rendered instead of waiting for
/// it. Frames come out in the order they went in.
pub(super) struct ReadbackRing {
    size: Size,
    row: usize,
    padded_row: usize,
    free: Vec<Buffer>,
    pending: VecDeque<Pending>,
}

impl ReadbackRing {
    pub(super) fn new(device: &wgpu::Device, size: Size) -> Self {
        let row = size.width as usize * 4;
        let padded_row = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize);
        let free = (0..RING_SLOTS)
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("lumen readback ring"),
                    size: (padded_row * size.height as usize).max(1) as u64,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                })
            })
            .collect();
        Self {
            size,
            row,
            padded_row,
            free,
            pending: VecDeque::with_capacity(RING_SLOTS),
        }
    }

    /// Queues the copy of `texture`, the render of `frame`, behind the work already submitted. With
    /// every slot in use it first waits for the oldest frame and returns its pixels.
    pub(super) fn push(
        &mut self,
        renderer: &lumen_gpu::Renderer,
        texture: &Texture,
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
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            TexelCopyBufferInfo {
                buffer: &buffer,
                layout: TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_row as u32),
                    rows_per_image: Some(self.size.height),
                },
            },
            self.size.as_extent(),
        );
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

    /// Waits for the oldest queued frame and returns its pixels with tightly packed rows, or
    /// `None` when nothing is queued.
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
        let mut pixels = Vec::with_capacity(self.row * self.size.height as usize);
        for padded in mapped.chunks(self.padded_row) {
            pixels.extend_from_slice(&padded[..self.row]);
        }
        drop(mapped);
        pending.buffer.unmap();
        self.free.push(pending.buffer);
        Ok(Some((pending.frame, pixels)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(frame: u32, size: Size) -> Vec<u8> {
        (0..size.width * size.height * 4)
            .map(|index| (index.wrapping_mul(31) ^ frame.wrapping_mul(97)) as u8)
            .collect()
    }

    #[test]
    fn frames_come_back_in_order_and_match_a_synchronous_readback() {
        let Ok(renderer) = pollster::block_on(lumen_gpu::Renderer::new()) else {
            eprintln!("skipping readback ring test: no GPU adapter");
            return;
        };
        // A width whose rows need padding, and a single texture reused by every frame.
        let size = Size::new(37, 5);
        let texture = renderer.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: size.as_extent(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut ring = ReadbackRing::new(&renderer.device, size);
        let frames = 3 * RING_SLOTS as u32 + 1;

        let mut read = Vec::new();
        for frame in 0..frames {
            renderer.queue.write_texture(
                texture.as_image_copy(),
                &pattern(frame, size),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size.width * 4),
                    rows_per_image: Some(size.height),
                },
                size.as_extent(),
            );
            read.extend(ring.push(&renderer, &texture, frame).unwrap());
            assert!(ring.pending.len() <= RING_SLOTS);
        }
        while let Some(next) = ring.pop(&renderer).unwrap() {
            read.push(next);
        }

        assert_eq!(read.len(), frames as usize);
        for (index, (frame, pixels)) in read.into_iter().enumerate() {
            assert_eq!(frame, index as u32);
            assert_eq!(pixels, pattern(frame, size), "frame {frame}");
        }
        assert_eq!(ring.free.len(), RING_SLOTS);
        assert_eq!(
            renderer.read_texture_rgba8(&texture, size).unwrap(),
            pattern(frames - 1, size),
            "the ring reads what a synchronous readback does"
        );
    }
}
