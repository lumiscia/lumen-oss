use std::sync::mpsc;

use anyhow::anyhow;
use lumen_gpu::Renderer;

use super::profile::{PhaseTimes, timed};

pub fn read_texture_rgba8(
    renderer: &Renderer,
    id: lumen_gpu::TextureId,
    size: lumen_gpu::Size,
    phases: &mut Option<&mut PhaseTimes>,
) -> anyhow::Result<Vec<u8>> {
    let bytes_per_pixel = 4;
    let unpadded_bytes_per_row = size.width.saturating_mul(bytes_per_pixel);
    let padded_bytes_per_row = align_to(
        unpadded_bytes_per_row,
        lumen_gpu::wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
    );
    let output_size = u64::from(padded_bytes_per_row).saturating_mul(u64::from(size.height));
    let output = timed(phases, "readback_create_buffer", || {
        renderer
            .device
            .create_buffer(&lumen_gpu::wgpu::BufferDescriptor {
                label: Some("lumen composition benchmark readback"),
                size: output_size.max(1),
                usage: lumen_gpu::wgpu::BufferUsages::COPY_DST
                    | lumen_gpu::wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
    });
    timed(phases, "readback_encode_copy", || {
        let mut encoder =
            renderer
                .device
                .create_command_encoder(&lumen_gpu::wgpu::CommandEncoderDescriptor {
                    label: Some("lumen composition benchmark readback encoder"),
                });
        let texture = renderer
            .texture(id)
            .ok_or_else(|| anyhow!("unknown texture"))?;
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            lumen_gpu::wgpu::TexelCopyBufferInfo {
                buffer: &output,
                layout: lumen_gpu::wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(size.height),
                },
            },
            size.as_extent(),
        );
        renderer.queue.submit([encoder.finish()]);
        anyhow::Ok(())
    })?;

    let slice = output.slice(..);
    timed(phases, "readback_map_wait", || {
        let (tx, rx) = mpsc::channel();
        slice.map_async(lumen_gpu::wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        renderer
            .device
            .poll(lumen_gpu::wgpu::PollType::wait_indefinitely())?;
        rx.recv()
            .map_err(|_| anyhow!("GPU readback callback closed"))??;
        anyhow::Ok(())
    })?;

    timed(phases, "readback_copy_rows", || {
        let mapped = slice
            .get_mapped_range()
            .map_err(|error| anyhow!("GPU readback map range failed: {error}"))?;
        let mut pixels = vec![
            0;
            (size.width as usize)
                .saturating_mul(size.height as usize)
                .saturating_mul(bytes_per_pixel as usize)
        ];
        for row in 0..size.height as usize {
            let src_start = row.saturating_mul(padded_bytes_per_row as usize);
            let src_end = src_start.saturating_add(unpadded_bytes_per_row as usize);
            let dst_start = row.saturating_mul(unpadded_bytes_per_row as usize);
            let dst_end = dst_start.saturating_add(unpadded_bytes_per_row as usize);
            pixels[dst_start..dst_end].copy_from_slice(&mapped[src_start..src_end]);
        }
        drop(mapped);
        output.unmap();
        anyhow::Ok(pixels)
    })
}

fn align_to(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}
