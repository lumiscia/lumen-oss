//! Conversion of rendered frames to the YUV an encoder takes: 4:2:0 at 8 or 10 bits, 10-bit 4:4:4,
//! and either with an alpha plane.

use std::sync::Arc;

use lumen_gpu::wgpu;

use crate::composition::BitDepth;

/// A YUV format [`YuvConverter`] writes. Samples above 8 bits are two little-endian bytes.
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
    /// [`Self::Yuv420p`], then a full-size alpha plane.
    Yuva420p,
    /// 10-bit luma, then full-size Cb and Cr planes, each value in the low bits of its two bytes.
    Yuv444p10,
    /// [`Self::Yuv444p10`], then a full-size alpha plane.
    Yuva444p10,
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
            Self::Nv12 | Self::Yuv420p | Self::Yuva420p => BitDepth::Eight,
            Self::P010 | Self::Yuv420p10 | Self::Yuv444p10 | Self::Yuva444p10 => BitDepth::Ten,
        }
    }

    pub fn is_planar(self) -> bool {
        !matches!(self, Self::Nv12 | Self::P010)
    }

    /// Whether chroma is full size, 4:4:4, rather than 4:2:0.
    pub fn is_full_chroma(self) -> bool {
        matches!(self, Self::Yuv444p10 | Self::Yuva444p10)
    }

    /// Whether an alpha plane follows the chroma. Its colour is then straight, not over black.
    pub fn has_alpha(self) -> bool {
        matches!(self, Self::Yuva420p | Self::Yuva444p10)
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
/// [`pitch`](Self::pitch) bytes apart, then the chroma, then for a format with alpha `height`
/// alpha rows `pitch` apart. Interleaved 4:2:0 chroma is `height.div_ceil(2)` rows `pitch` apart;
/// planar 4:2:0 chroma is the Cb plane and then the Cr plane, each that many rows `pitch / 2`
/// apart; 4:4:4 chroma is a Cb and a Cr plane laid out as luma is. The two-plane layouts match
/// `lumen_ffmpeg::VulkanFrameLayout`'s for even sizes.
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

    /// Bytes between the rows of one chroma plane.
    pub fn chroma_pitch(&self) -> u32 {
        if self.format.is_planar() && !self.format.is_full_chroma() {
            self.pitch() / 2
        } else {
            self.pitch()
        }
    }

    pub fn chroma_rows(&self) -> u32 {
        if self.format.is_full_chroma() {
            self.height
        } else {
            self.height.div_ceil(2)
        }
    }

    /// Where the alpha plane starts, which is where the chroma ends.
    pub fn alpha_offset(&self) -> u64 {
        let planes = if self.format.is_planar() { 2 } else { 1 };
        self.chroma_offset()
            + planes * u64::from(self.chroma_pitch()) * u64::from(self.chroma_rows())
    }

    pub fn size(&self) -> u64 {
        let alpha = if self.format.has_alpha() {
            self.chroma_offset()
        } else {
            0
        };
        self.alpha_offset() + alpha
    }
}

/// Converts the working space, linear and premultiplied, to BT.709 limited-range YUV in a buffer
/// laid out as its [`YuvLayout`] says. Without alpha the colour is the frame over black; with it,
/// the colour is straight, unpremultiplied in linear light and black where alpha is 0, and alpha
/// is stored full range. sRGB-encoded colour is converted to Y'CbCr. 4:2:0 chroma is the average
/// of each 2x2 block, weighted by alpha when there is any. Each code is quantised once, straight
/// from float colour, after chroma averaging, with the ordered dither when it is on; alpha is
/// rounded without dither.
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
            size: 48,
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
                u32::from(layout.format.is_full_chroma()),
                u32::from(layout.format.has_alpha()),
                layout.alpha_offset() as u32,
                0,
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

    /// A working-space texture of linear premultiplied `pixels`, row by row.
    fn premultiplied(gpu: &lumen_gpu::Renderer, width: u32, pixels: &[[f32; 4]]) -> wgpu::Texture {
        let height = pixels.len() as u32 / width;
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
        let bytes: Vec<u8> = pixels
            .iter()
            .flatten()
            .map(|&value| half(value))
            .flat_map(u16::to_le_bytes)
            .collect();
        gpu.queue.write_texture(
            texture.as_image_copy(),
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 8),
                rows_per_image: None,
            },
            size.as_extent(),
        );
        texture
    }

    fn encode_srgb(linear: f64) -> f64 {
        if linear <= 0.0031308 {
            linear * 12.92
        } else {
            1.055 * linear.powf(1.0 / 2.4) - 0.055
        }
    }

    /// The straight sRGB-encoded colour of a linear premultiplied pixel, black where its alpha
    /// rounds to code 0 at `bits`.
    fn straight(pixel: [f32; 4], bits: u32) -> [f64; 3] {
        let alpha = f64::from(pixel[3]);
        if (alpha * f64::from((1 << bits) - 1)).round() == 0.0 {
            return [0.0; 3];
        }
        [0, 1, 2].map(|channel| encode_srgb(f64::from(pixel[channel]) / alpha).clamp(0.0, 1.0))
    }

    /// BT.709 limited-range Y', Cb, Cr codes of encoded colour at `bits`, rounded.
    fn ycbcr(color: [f64; 3], bits: u32) -> [u16; 3] {
        let (kr, kb) = (0.2126, 0.0722);
        let y = kr * color[0] + (1.0 - kr - kb) * color[1] + kb * color[2];
        let scale = f64::from(1 << (bits - 8));
        [
            16.0 + 219.0 * y,
            128.0 + 224.0 * (color[2] - y) / (2.0 * (1.0 - kb)),
            128.0 + 224.0 * (color[0] - y) / (2.0 * (1.0 - kr)),
        ]
        .map(|code| (code * scale).round() as u16)
    }

    fn alpha_code(pixel: [f32; 4], bits: u32) -> u16 {
        (f64::from(pixel[3]) * f64::from((1 << bits) - 1)).round() as u16
    }

    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    const HALF_WHITE: [f32; 4] = [0.5, 0.5, 0.5, 0.5];
    const QUARTER_BLUE: [f32; 4] = [0.0, 0.0, 0.25, 0.25];
    /// Transparent, with colour a premultiplied pixel can't have, which must not show.
    const CLEAR_GARBAGE: [f32; 4] = [0.7, 0.2, 0.9, 0.0];

    #[test]
    fn alpha_formats_store_straight_colour_and_full_range_alpha() {
        let Some(gpu) = gpu() else { return };
        let pixels = [RED, HALF_WHITE, QUARTER_BLUE, CLEAR_GARBAGE];

        // 10-bit 4:4:4: every pixel keeps its own chroma.
        let texture = premultiplied(&gpu, 4, &pixels);
        let (layout, bytes) = convert(&gpu, &texture, YuvFormat::Yuva444p10);
        let plane = |offset: u64| samples(&bytes, layout, offset, layout.pitch(), 0, 4);
        let (luma, cb, cr, alpha) = (
            plane(0),
            plane(layout.chroma_offset()),
            plane(layout.chroma_offset() + u64::from(layout.pitch())),
            plane(layout.alpha_offset()),
        );
        for (index, pixel) in pixels.into_iter().enumerate() {
            let [y, b, r] = ycbcr(straight(pixel, 10), 10);
            assert_eq!(
                [luma[index], cb[index], cr[index], alpha[index]],
                [y, b, r, alpha_code(pixel, 10)],
                "pixel {index}: {pixel:?}"
            );
        }
        // Spelled out: red, white at half alpha, blue at a quarter, and transparent black.
        assert_eq!(luma, [250, 940, 127, 64]);
        assert_eq!(alpha, [1023, 512, 256, 0]);
        assert_eq!((cb[3], cr[3]), (512, 512));

        // The opaque 4:4:4 format is the same colour over black, with no alpha plane.
        let (opaque_layout, opaque) = convert(&gpu, &texture, YuvFormat::Yuv444p10);
        assert_eq!(opaque_layout.size(), opaque_layout.alpha_offset());
        assert_eq!(
            samples(&opaque, opaque_layout, 0, layout.pitch(), 0, 1),
            [luma[0]]
        );
    }

    #[test]
    fn alpha_weights_four_two_zero_chroma_so_clear_texels_do_not_darken_edges() {
        let Some(gpu) = gpu() else { return };
        // Two 2x2 blocks: the four kinds of pixel, then red beside three clear ones.
        let rows = [
            [RED, HALF_WHITE, RED, CLEAR_GARBAGE],
            [QUARTER_BLUE, CLEAR_GARBAGE, CLEAR_GARBAGE, CLEAR_GARBAGE],
        ];
        let texture = premultiplied(&gpu, 4, rows.as_flattened());
        let (layout, bytes) = convert(&gpu, &texture, YuvFormat::Yuva420p);
        let half = layout.chroma_pitch();
        let chroma_size = u64::from(half) * u64::from(layout.chroma_rows());
        let cb = samples(&bytes, layout, layout.chroma_offset(), half, 0, 2);
        let cr = samples(
            &bytes,
            layout,
            layout.chroma_offset() + chroma_size,
            half,
            0,
            2,
        );
        for row in 0..2 {
            let luma = samples(&bytes, layout, 0, layout.pitch(), row, 4);
            let alpha = samples(
                &bytes,
                layout,
                layout.alpha_offset(),
                layout.pitch(),
                row,
                4,
            );
            for (x, pixel) in rows[row as usize].into_iter().enumerate() {
                assert_eq!(luma[x], ycbcr(straight(pixel, 8), 8)[0], "Y at {x}, {row}");
                assert_eq!(alpha[x], alpha_code(pixel, 8), "A at {x}, {row}");
            }
        }
        for block in 0..2 {
            let texels = [0, 1].map(|row| [0, 1].map(|x| rows[row][block * 2 + x]));
            let texels = texels.as_flattened();
            // sum(alpha * straight colour) / sum(alpha).
            let weight: f64 = texels.iter().map(|pixel| f64::from(pixel[3])).sum();
            let average = [0, 1, 2].map(|channel| {
                texels
                    .iter()
                    .map(|&pixel| f64::from(pixel[3]) * straight(pixel, 8)[channel])
                    .sum::<f64>()
                    / weight
            });
            let [_, b, r] = ycbcr(average, 8);
            assert_eq!([cb[block], cr[block]], [b, r], "block {block}");
        }
        // Red beside clear texels keeps red's chroma; an unweighted average would pull it to grey.
        assert_eq!([cb[1], cr[1]], [102, 240]);
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
