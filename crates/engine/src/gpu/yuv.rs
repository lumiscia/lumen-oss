//! Conversion of rendered frames to the YUV 4:2:0 an encoder takes, at 8 or 10 bits.

use std::sync::Arc;

use lumen_gpu::wgpu;

use crate::composition::BitDepth;

/// A YUV 4:2:0 format [`YuvConverter`] writes. Samples above 8 bits are two little-endian bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum YuvFormat {
    /// 8-bit luma, then interleaved Cb, Cr pairs.
    Nv12,
    /// [`Self::Nv12`] at 10 bits, each value in the high bits of its two bytes.
    P010,
    /// 8-bit luma, then the Cb plane, then the Cr plane.
    Yuv420p,
    /// [`Self::Yuv420p`] at 10 bits, each value in the low bits of its two bytes.
    Yuv420p10,
}

impl YuvFormat {
    /// The two-plane format hardware encoders read at `depth`.
    pub fn semi_planar(depth: BitDepth) -> Self {
        match depth {
            BitDepth::Eight => Self::Nv12,
            BitDepth::Ten => Self::P010,
        }
    }

    pub fn depth(self) -> BitDepth {
        match self {
            Self::Nv12 | Self::Yuv420p => BitDepth::Eight,
            Self::P010 | Self::Yuv420p10 => BitDepth::Ten,
        }
    }

    pub fn is_planar(self) -> bool {
        matches!(self, Self::Yuv420p | Self::Yuv420p10)
    }

    pub fn bytes_per_sample(self) -> u32 {
        match self.depth() {
            BitDepth::Eight => 1,
            BitDepth::Ten => 2,
        }
    }

    fn bits(self) -> u32 {
        match self.depth() {
            BitDepth::Eight => 8,
            BitDepth::Ten => 10,
        }
    }
}

/// Where a frame's planes sit in a [`YuvConverter`]'s buffer: `height` luma rows
/// [`pitch`](Self::pitch) bytes apart, then `height.div_ceil(2)` chroma rows. Interleaved chroma
/// rows are `pitch` apart; planar ones are `pitch / 2` apart, the Cb plane and then the Cr plane.
/// The two-plane layouts match `lumen_ffmpeg::VulkanFrameLayout`'s for even sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct YuvLayout {
    pub width: u32,
    pub height: u32,
    pub format: YuvFormat,
}

impl YuvLayout {
    pub fn new(width: u32, height: u32, format: YuvFormat) -> Self {
        Self {
            width,
            height,
            format,
        }
    }

    /// Bytes between luma rows: a row's samples, padded to the 256 bytes wgpu copies need.
    pub fn pitch(&self) -> u32 {
        (self.width * self.format.bytes_per_sample()).next_multiple_of(256)
    }

    pub fn chroma_offset(&self) -> u64 {
        u64::from(self.pitch()) * u64::from(self.height)
    }

    pub fn size(&self) -> u64 {
        self.chroma_offset() + u64::from(self.pitch()) * u64::from(self.height.div_ceil(2))
    }
}

/// Converts the working space, linear and premultiplied, to BT.709 limited-range YUV of its
/// sRGB-encoded colour over black, in a buffer laid out as its [`YuvLayout`] says. Each code is
/// quantised once, straight from float colour, after chroma averaging, with the ordered dither when
/// it is on.
pub struct YuvConverter {
    pipeline: wgpu::ComputePipeline,
    params: wgpu::Buffer,
    buffer: Arc<wgpu::Buffer>,
    layout: YuvLayout,
}

impl YuvConverter {
    /// A converter writing a buffer of its own, which can be copied from.
    pub fn new(renderer: &lumen_gpu::Renderer, layout: YuvLayout, dither: bool) -> Self {
        let buffer = renderer.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lumen YUV frame"),
            size: layout.size(),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        Self::with_buffer(renderer, layout, Arc::new(buffer), dither)
    }

    /// A converter writing `buffer`, which must have `STORAGE` usage and hold `layout.size()`
    /// bytes.
    pub fn with_buffer(
        renderer: &lumen_gpu::Renderer,
        layout: YuvLayout,
        buffer: Arc<wgpu::Buffer>,
        dither: bool,
    ) -> Self {
        let device = &renderer.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("lumen RGBA to YUV"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("shaders/color.wgsl"),
                    include_str!("shaders/dither.wgsl"),
                    include_str!("shaders/rgba_to_yuv.wgsl")
                )
                .into(),
            ),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("lumen RGBA to YUV"),
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lumen YUV params"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        renderer.queue.write_buffer(
            &params,
            0,
            bytemuck::cast_slice(&[
                layout.width,
                layout.height,
                layout.pitch(),
                layout.chroma_offset() as u32,
                u32::from(dither),
                layout.format.bits(),
                u32::from(layout.format.is_planar()),
                0,
            ]),
        );
        Self {
            pipeline,
            params,
            buffer,
            layout,
        }
    }

    pub fn layout(&self) -> YuvLayout {
        self.layout
    }

    /// The buffer each conversion writes, which the next conversion reuses.
    pub fn buffer(&self) -> &Arc<wgpu::Buffer> {
        &self.buffer
    }

    /// Submits the conversion of `texture`, a frame of the layout's size.
    pub fn submit(
        &self,
        renderer: &lumen_gpu::Renderer,
        texture: &wgpu::Texture,
    ) -> wgpu::SubmissionIndex {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = renderer
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("lumen RGBA to YUV"),
                layout: &self.pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.params.as_entire_binding(),
                    },
                ],
            });
        let mut encoder = renderer
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("lumen RGBA to YUV"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("lumen RGBA to YUV"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(
                self.layout.width.div_ceil(8).div_ceil(8),
                self.layout.height.div_ceil(2).div_ceil(8),
                1,
            );
        }
        renderer.queue.submit([encoder.finish()])
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::gpu::color::srgb_to_linear;

    /// `value`, which must be zero or positive and below 65504, as a half float.
    fn half(value: f32) -> u16 {
        if value < 2f32.powi(-14) {
            return (value / 2f32.powi(-24)).round() as u16;
        }
        let bits = value.to_bits();
        let exponent = ((bits >> 23) & 0xff) + 15 - 127;
        let mantissa = bits & 0x7f_ffff;
        ((exponent << 10) | (mantissa >> 13)) as u16 + u16::from(mantissa & 0x1000 != 0)
    }

    /// A working-space texture of opaque grey, whose encoded value at each column is `encoded`.
    fn texture(
        gpu: &lumen_gpu::Renderer,
        width: u32,
        height: u32,
        encoded: impl Fn(u32) -> f32,
    ) -> wgpu::Texture {
        let size = lumen_gpu::Size::new(width, height);
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: size.as_extent(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let row: Vec<u8> = (0..width)
            .flat_map(|x| {
                let light = half(srgb_to_linear(encoded(x)));
                [light, light, light, half(1.0)]
            })
            .flat_map(u16::to_le_bytes)
            .collect();
        gpu.queue.write_texture(
            texture.as_image_copy(),
            &row.repeat(height as usize),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 8),
                rows_per_image: None,
            },
            size.as_extent(),
        );
        texture
    }

    fn convert(
        gpu: &lumen_gpu::Renderer,
        texture: &wgpu::Texture,
        format: YuvFormat,
    ) -> (YuvLayout, Vec<u8>) {
        let size = texture.size();
        let layout = YuvLayout::new(size.width, size.height, format);
        let converter = YuvConverter::new(gpu, layout, false);
        converter.submit(gpu, texture);
        let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: layout.size(),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_buffer_to_buffer(converter.buffer(), 0, &readback, 0, layout.size());
        gpu.queue.submit([encoder.finish()]);
        readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        gpu.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("wait for YUV readback");
        let bytes = readback.slice(..).get_mapped_range().expect("map").to_vec();
        (layout, bytes)
    }

    /// The samples of row `row` of the plane starting at `offset`, `count` of them.
    fn samples(
        bytes: &[u8],
        layout: YuvLayout,
        offset: u64,
        pitch: u32,
        row: u32,
        count: u32,
    ) -> Vec<u16> {
        let start = offset as usize + (row * pitch) as usize;
        let sample = layout.format.bytes_per_sample() as usize;
        (0..count as usize)
            .map(|index| {
                let at = start + index * sample;
                match sample {
                    1 => u16::from(bytes[at]),
                    _ => u16::from_le_bytes([bytes[at], bytes[at + 1]]),
                }
            })
            .collect()
    }

    fn gpu() -> Option<lumen_gpu::Renderer> {
        match pollster::block_on(lumen_gpu::Renderer::new()) {
            Ok(gpu) => Some(gpu),
            Err(error) => {
                eprintln!("no GPU ({error}); skipping YUV conversion test");
                None
            }
        }
    }

    #[test]
    fn ten_bit_codes_come_straight_from_float() {
        let Some(gpu) = gpu() else { return };
        // Black, sRGB 128 grey and white, two columns each.
        let columns = [0.0, 128.0 / 255.0, 1.0];
        let texture = texture(&gpu, 6, 2, |x| columns[x as usize / 2]);
        for format in [YuvFormat::Yuv420p10, YuvFormat::P010] {
            let (layout, bytes) = convert(&gpu, &texture, format);
            let shift = if format.is_planar() { 0 } else { 6 };
            let luma = samples(&bytes, layout, 0, layout.pitch(), 1, 6);
            assert_eq!(
                luma.iter().map(|code| code >> shift).collect::<Vec<_>>(),
                [64, 64, 504, 504, 940, 940],
                "{format:?}"
            );
            let chroma: Vec<u16> = if format.is_planar() {
                let half = layout.pitch() / 2;
                let cb = samples(&bytes, layout, layout.chroma_offset(), half, 0, 3);
                let cr_offset = layout.chroma_offset() + u64::from(half);
                cb.into_iter()
                    .chain(samples(&bytes, layout, cr_offset, half, 0, 3))
                    .collect()
            } else {
                samples(&bytes, layout, layout.chroma_offset(), layout.pitch(), 0, 6)
            };
            assert!(
                chroma.iter().all(|code| code >> shift == 512),
                "{format:?} {chroma:?}"
            );
        }
        // The 8-bit planar codes of the same colours.
        let (layout, bytes) = convert(&gpu, &texture, YuvFormat::Yuv420p);
        assert_eq!(
            samples(&bytes, layout, 0, layout.pitch(), 0, 6),
            [16, 16, 126, 126, 235, 235]
        );
    }

    #[test]
    fn a_dark_gradient_keeps_more_levels_at_ten_bits() {
        let Some(gpu) = gpu() else { return };
        let width = 2048;
        let texture = texture(&gpu, width, 2, |x| 0.3 * x as f32 / (width - 1) as f32);
        let levels = |format| {
            let (layout, bytes) = convert(&gpu, &texture, format);
            samples(&bytes, layout, 0, layout.pitch(), 0, width)
                .into_iter()
                .collect::<BTreeSet<_>>()
        };
        let eight = levels(YuvFormat::Yuv420p);
        let ten = levels(YuvFormat::Yuv420p10);
        assert_eq!(eight.len(), 67, "{eight:?}");
        assert!(ten.len() > 256, "{} levels", ten.len());
        assert_eq!((ten.first(), ten.last()), (Some(&64), Some(&327)));
    }
}
