use std::{num::NonZeroUsize, sync::Arc};

use futures_intrusive::channel::shared::oneshot_channel;
use skrifa::{
    MetadataProvider,
    raw::{FileRef, FontRef},
};
use vello::{
    AaConfig, AaSupport, Glyph, RenderParams, Renderer, RendererOptions, Scene,
    kurbo::{Affine, Ellipse, Rect, RoundedRect},
    peniko::{
        Blob, Brush, Color, Fill, FontData, ImageAlphaType, ImageBrush, ImageData, ImageFormat,
        StyleRef,
    },
    util::{RenderContext, block_on_wgpu},
    wgpu::{
        self, BufferDescriptor, BufferUsages, CommandEncoderDescriptor, Extent3d,
        TexelCopyBufferInfo, TextureDescriptor, TextureFormat, TextureUsages,
    },
};

use crate::{
    backend::{
        FrameImage, FrameProvider, RenderBackend, RenderError, pixel_len,
    },
    compile::{CompiledOperationKind, CompiledTimeline, VideoSourceRef},
    model::{ColorRgba, FitMode, Shape, ShapeClip, TextAlign, TextClip, Transform},
};

// Re-export shared types for backward compatibility.
pub use crate::backend::{NoopFrameProvider, RenderError as GpuRenderError};

pub struct GpuRenderer {
    context: RenderContext,
    device_id: usize,
    renderer: Renderer,
    target_texture: wgpu::Texture,
    target_view: wgpu::TextureView,
    readback_buffer: wgpu::Buffer,
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
    text: TextPainter,
    scene: Scene,
}

impl GpuRenderer {
    pub fn new(width: u32, height: u32) -> Result<Self, RenderError> {
        let mut context = RenderContext::new();
        let device_id =
            pollster::block_on(context.device(None)).ok_or(RenderError::MissingDevice)?;

        let device = &context.devices[device_id].device;

        let renderer = Renderer::new(
            device,
            RendererOptions {
                use_cpu: false,
                antialiasing_support: AaSupport::all(),
                num_init_threads: NonZeroUsize::new(1),
                pipeline_cache: None,
            },
        )
        .map_err(|err| RenderError::RendererInit(err.to_string()))?;

        let (target_texture, target_view, readback_buffer, padded_bytes_per_row) =
            allocate_targets(device, width, height)?;

        Ok(Self {
            context,
            device_id,
            renderer,
            target_texture,
            target_view,
            readback_buffer,
            width,
            height,
            padded_bytes_per_row,
            text: TextPainter::default(),
            scene: Scene::new(),
        })
    }

    pub fn render_frame(
        &mut self,
        timeline: &CompiledTimeline,
        frame: u64,
        provider: &mut dyn FrameProvider,
    ) -> Result<Vec<u8>, RenderError> {
        if frame >= timeline.total_frames() {
            return Err(RenderError::FrameOutOfRange {
                frame,
                total_frames: timeline.total_frames(),
            });
        }

        if self.width != timeline.canvas.width || self.height != timeline.canvas.height {
            self.resize(timeline.canvas.width, timeline.canvas.height)?;
        }

        let mut scene = std::mem::take(&mut self.scene);
        scene.reset();
        self.build_scene(timeline, frame, provider, &mut scene)?;

        let params = RenderParams {
            base_color: to_peniko_color(timeline.canvas.background),
            width: self.width,
            height: self.height,
            antialiasing_method: AaConfig::Msaa16,
        };

        let device = &self.context.devices[self.device_id].device;
        let queue = &self.context.devices[self.device_id].queue;

        self.renderer
            .render_to_texture(device, queue, &scene, &self.target_view, &params)
            .map_err(|err| RenderError::RendererInit(err.to_string()))?;

        self.scene = scene;

        copy_to_readback(
            device,
            queue,
            &self.target_texture,
            &self.readback_buffer,
            self.width,
            self.height,
            self.padded_bytes_per_row,
        );

        readback_rgba(
            device,
            &self.readback_buffer,
            self.width,
            self.height,
            self.padded_bytes_per_row,
        )
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<(), RenderError> {
        let device = &self.context.devices[self.device_id].device;
        let (target_texture, target_view, readback_buffer, padded_bytes_per_row) =
            allocate_targets(device, width, height)?;

        self.target_texture = target_texture;
        self.target_view = target_view;
        self.readback_buffer = readback_buffer;
        self.width = width;
        self.height = height;
        self.padded_bytes_per_row = padded_bytes_per_row;

        Ok(())
    }

    fn build_scene(
        &mut self,
        timeline: &CompiledTimeline,
        frame: u64,
        provider: &mut dyn FrameProvider,
        scene: &mut Scene,
    ) -> Result<(), RenderError> {
        let operation_indices = timeline.operation_indices_for_frame(frame)?;

        for operation_index in operation_indices {
            let operation = timeline
                .operation(*operation_index)
                .ok_or(RenderError::MissingOperation(*operation_index))?;

            let opacity = operation.opacity;
            if opacity <= 0.0 {
                continue;
            }

            match &operation.kind {
                CompiledOperationKind::Solid { color } => {
                    draw_solid(scene, operation.transform, opacity, *color);
                }
                CompiledOperationKind::Shape(shape) => {
                    draw_shape(scene, operation.transform, opacity, shape);
                }
                CompiledOperationKind::Text(text) => {
                    self.text
                        .draw(scene, operation.transform, opacity, text)
                        .map_err(|err| RenderError::Text(err.to_string()))?;
                }
                CompiledOperationKind::Image(image) => {
                    if let Some(frame_image) = provider.image(image.source_id.as_str())? {
                        draw_image(scene, operation.transform, opacity, image.fit, &frame_image)?;
                    }
                }
                CompiledOperationKind::Video(video) => {
                    if let Some(source_frame) = resolve_video_frame(operation, video, frame)? {
                        if let Some(frame_image) =
                            provider.video_frame(video.source_id.as_str(), source_frame)?
                        {
                            draw_image(
                                scene,
                                operation.transform,
                                opacity,
                                video.fit,
                                &frame_image,
                            )?;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

impl RenderBackend for GpuRenderer {
    fn render_frame(
        &mut self,
        timeline: &CompiledTimeline,
        frame: u64,
        provider: &mut dyn FrameProvider,
    ) -> Result<Vec<u8>, RenderError> {
        self.render_frame(timeline, frame, provider)
    }
}

fn resolve_video_frame(
    operation: &crate::compile::CompiledOperation,
    _video: &VideoSourceRef,
    frame: u64,
) -> Result<Option<u64>, RenderError> {
    operation
        .resolve_video_source_frame(frame)
        .map_err(Into::into)
}

fn draw_solid(scene: &mut Scene, transform: Transform, opacity: f32, color: ColorRgba) {
    let brush = Color::from_rgba8(
        color.r(),
        color.g(),
        color.b(),
        alpha_scaled(color.a(), opacity),
    );
    let rect = layout_rect(transform, 1.0, 1.0, FitMode::Fill);

    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        brush,
        None,
        &Rect::new(rect.x, rect.y, rect.x + rect.width, rect.y + rect.height),
    );
}

fn draw_shape(scene: &mut Scene, transform: Transform, opacity: f32, shape: &ShapeClip) {
    match shape.shape {
        Shape::Rectangle { fill, radius } => {
            let brush = Color::from_rgba8(
                fill.r(),
                fill.g(),
                fill.b(),
                alpha_scaled(fill.a(), opacity),
            );
            let rect = layout_rect(transform, 1.0, 1.0, FitMode::Fill);
            if radius > 0.0 {
                scene.fill(
                    Fill::NonZero,
                    Affine::IDENTITY,
                    brush,
                    None,
                    &RoundedRect::new(
                        rect.x,
                        rect.y,
                        rect.x + rect.width,
                        rect.y + rect.height,
                        radius as f64,
                    ),
                );
            } else {
                scene.fill(
                    Fill::NonZero,
                    Affine::IDENTITY,
                    brush,
                    None,
                    &Rect::new(rect.x, rect.y, rect.x + rect.width, rect.y + rect.height),
                );
            }
        }
        Shape::Ellipse { fill } => {
            let brush = Color::from_rgba8(
                fill.r(),
                fill.g(),
                fill.b(),
                alpha_scaled(fill.a(), opacity),
            );
            let rect = layout_rect(transform, 1.0, 1.0, FitMode::Fill);
            let ellipse = Ellipse::new(
                (rect.x + (rect.width / 2.0), rect.y + (rect.height / 2.0)),
                (rect.width / 2.0, rect.height / 2.0),
                0.0,
            );
            scene.fill(Fill::NonZero, Affine::IDENTITY, brush, None, &ellipse);
        }
    }
}

fn draw_image(
    scene: &mut Scene,
    transform: Transform,
    opacity: f32,
    fit: FitMode,
    image: &FrameImage,
) -> Result<(), RenderError> {
    let image_data = ImageData {
        data: Blob::new(image.rgba.clone()),
        format: ImageFormat::Rgba8,
        width: image.width,
        height: image.height,
        alpha_type: ImageAlphaType::Alpha,
    };

    let rect = layout_rect(transform, image.width as f64, image.height as f64, fit);
    let scale_x = rect.width / (image.width as f64);
    let scale_y = rect.height / (image.height as f64);

    let mut affine =
        Affine::translate((rect.x, rect.y)) * Affine::scale_non_uniform(scale_x, scale_y);
    if transform.rotation_degrees != 0.0 {
        let radians = (transform.rotation_degrees as f64).to_radians();
        let cx = rect.x + (rect.width / 2.0);
        let cy = rect.y + (rect.height / 2.0);
        affine = Affine::translate((cx, cy))
            * Affine::rotate(radians)
            * Affine::translate((-cx, -cy))
            * affine;
    }

    let brush = ImageBrush::new(image_data).multiply_alpha(opacity);
    scene.fill(
        Fill::NonZero,
        affine,
        &brush,
        None,
        &Rect::new(0.0, 0.0, image.width as f64, image.height as f64),
    );

    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct LayoutRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn layout_rect(
    transform: Transform,
    source_width: f64,
    source_height: f64,
    fit: FitMode,
) -> LayoutRect {
    let target_width = transform
        .width
        .map(|value| value as f64)
        .unwrap_or(source_width);
    let target_height = transform
        .height
        .map(|value| value as f64)
        .unwrap_or(source_height);

    let target_width = target_width.max(1.0);
    let target_height = target_height.max(1.0);

    let (draw_width, draw_height) = match fit {
        FitMode::Fill => (target_width, target_height),
        FitMode::Contain => {
            let scale = (target_width / source_width).min(target_height / source_height);
            (source_width * scale, source_height * scale)
        }
        FitMode::Cover => {
            let scale = (target_width / source_width).max(target_height / source_height);
            (source_width * scale, source_height * scale)
        }
    };

    let x = transform.x as f64 + ((target_width - draw_width) / 2.0);
    let y = transform.y as f64 + ((target_height - draw_height) / 2.0);

    LayoutRect {
        x,
        y,
        width: draw_width,
        height: draw_height,
    }
}

fn alpha_scaled(alpha: u8, opacity: f32) -> u8 {
    ((alpha as f32) * opacity).round() as u8
}

fn to_peniko_color(color: ColorRgba) -> Color {
    Color::from_rgba8(color.r(), color.g(), color.b(), color.a())
}

fn allocate_targets(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> Result<(wgpu::Texture, wgpu::TextureView, wgpu::Buffer, u32), RenderError> {
    let target_texture = device.create_texture(&TextureDescriptor {
        label: Some("lumen_target_texture"),
        size: Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::STORAGE_BINDING | TextureUsages::COPY_SRC,
        view_formats: &[],
    });

    let target_view = target_texture.create_view(&wgpu::TextureViewDescriptor::default());

    let padded_bytes_per_row = width.saturating_mul(4).next_multiple_of(256);
    let buffer_size = (padded_bytes_per_row as u64)
        .checked_mul(height as u64)
        .ok_or(RenderError::SizeOverflow)?;

    let readback_buffer = device.create_buffer(&BufferDescriptor {
        label: Some("lumen_readback_buffer"),
        size: buffer_size,
        usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    Ok((
        target_texture,
        target_view,
        readback_buffer,
        padded_bytes_per_row,
    ))
}

fn copy_to_readback(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target_texture: &wgpu::Texture,
    readback_buffer: &wgpu::Buffer,
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
) {
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("lumen_readback_copy"),
    });

    encoder.copy_texture_to_buffer(
        target_texture.as_image_copy(),
        TexelCopyBufferInfo {
            buffer: readback_buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: None,
            },
        },
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    queue.submit([encoder.finish()]);
}

fn readback_rgba(
    device: &wgpu::Device,
    readback_buffer: &wgpu::Buffer,
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
) -> Result<Vec<u8>, RenderError> {
    let slice = readback_buffer.slice(..);
    let (sender, receiver) = oneshot_channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });

    let receive = block_on_wgpu(device, receiver.receive()).ok_or(RenderError::BufferMap)?;
    receive.map_err(|_| RenderError::BufferMap)?;

    let mapped = slice.get_mapped_range();
    let row_len = (width as usize)
        .checked_mul(4)
        .ok_or(RenderError::SizeOverflow)?;
    let total_len = pixel_len(width, height)?;
    let padded = padded_bytes_per_row as usize;
    let height_usize = height as usize;

    // Validate bounds once upfront.
    let src_required = height_usize
        .saturating_sub(1)
        .checked_mul(padded)
        .and_then(|v| v.checked_add(row_len))
        .ok_or(RenderError::BufferRead)?;
    if src_required > mapped.len() {
        return Err(RenderError::BufferRead);
    }

    let rgba = if row_len == padded {
        // Fast path: no padding, single copy.
        mapped[..total_len].to_vec()
    } else {
        let mut buf = Vec::with_capacity(total_len);
        for row in 0..height_usize {
            let src_start = row * padded;
            buf.extend_from_slice(&mapped[src_start..src_start + row_len]);
        }
        buf
    };

    drop(mapped);
    readback_buffer.unmap();

    Ok(rgba)
}

struct TextPainter {
    font: FontData,
}

impl TextPainter {
    fn draw(
        &mut self,
        scene: &mut Scene,
        transform: Transform,
        opacity: f32,
        text: &TextClip,
    ) -> Result<(), RenderError> {
        let font_ref = to_font_ref(&self.font)
            .ok_or_else(|| RenderError::Text("failed to parse embedded font".to_string()))?;

        let brush = Brush::Solid(Color::from_rgba8(
            text.color.r(),
            text.color.g(),
            text.color.b(),
            alpha_scaled(text.color.a(), opacity),
        ));

        let font_size = text.font_size.max(1.0);
        let axes = font_ref.axes();
        let location = axes.location(std::iter::empty::<(&str, f32)>());
        let metrics = font_ref.metrics(skrifa::instance::Size::new(font_size), &location);
        let glyph_metrics =
            font_ref.glyph_metrics(skrifa::instance::Size::new(font_size), &location);
        let line_height = (metrics.ascent - metrics.descent + metrics.leading).max(font_size);

        let charmap = font_ref.charmap();
        let rotation_radians = (transform.rotation_degrees as f64).to_radians();

        // Single pass: build glyphs and measure width simultaneously per line.
        let mut line_data: Vec<(Vec<Glyph>, f32)> = Vec::new();
        let mut width_max = 0.0f32;
        for line in text.text.lines() {
            let mut pen_x = 0.0f32;
            let glyphs: Vec<Glyph> = line
                .chars()
                .map(|ch| {
                    let gid = charmap.map(ch).unwrap_or_default();
                    let advance = glyph_metrics.advance_width(gid).unwrap_or_default();
                    let glyph = Glyph {
                        id: gid.to_u32(),
                        x: pen_x,
                        y: 0.0,
                    };
                    pen_x += advance;
                    glyph
                })
                .collect();
            width_max = width_max.max(pen_x);
            line_data.push((glyphs, pen_x));
        }

        if line_data.is_empty() {
            line_data.push((Vec::new(), 0.0));
        }

        let target_width = transform.width.unwrap_or(width_max.max(1.0));
        let mut y_cursor = transform.y;

        for (glyphs, width) in &line_data {
            let x = match text.align {
                TextAlign::Left => transform.x,
                TextAlign::Center => transform.x + ((target_width - width) / 2.0),
                TextAlign::Right => transform.x + (target_width - width),
            };

            scene
                .draw_glyphs(&self.font)
                .font_size(font_size)
                .transform(
                    Affine::translate((x as f64, (y_cursor + font_size) as f64))
                        * Affine::rotate(rotation_radians),
                )
                .brush(&brush)
                .draw(
                    StyleRef::Fill(Fill::NonZero),
                    glyphs.iter().copied(),
                );

            y_cursor += line_height;
        }

        Ok(())
    }
}

impl Default for TextPainter {
    fn default() -> Self {
        Self {
            font: FontData::new(Blob::new(Arc::new(EMBEDDED_FONT)), 0),
        }
    }
}

const EMBEDDED_FONT: &[u8] = include_bytes!("../../../assets/roboto/Roboto-Regular.ttf");

fn to_font_ref(font: &FontData) -> Option<FontRef<'_>> {
    let file_ref = FileRef::new(font.data.as_ref()).ok()?;
    match file_ref {
        FileRef::Font(font) => Some(font),
        FileRef::Collection(collection) => collection.get(font.index).ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::layout_rect;
    use crate::model::{FitMode, Transform};

    #[test]
    fn contain_fit_keeps_aspect_ratio() {
        let rect = layout_rect(
            Transform {
                x: 0.0,
                y: 0.0,
                width: Some(200.0),
                height: Some(100.0),
                rotation_degrees: 0.0,
            },
            100.0,
            100.0,
            FitMode::Contain,
        );

        assert_eq!(rect.width, 100.0);
        assert_eq!(rect.height, 100.0);
        assert_eq!(rect.x, 50.0);
    }

    #[test]
    fn cover_fit_expands_aspect_ratio() {
        let rect = layout_rect(
            Transform {
                x: 0.0,
                y: 0.0,
                width: Some(200.0),
                height: Some(100.0),
                rotation_degrees: 0.0,
            },
            100.0,
            100.0,
            FitMode::Cover,
        );

        assert_eq!(rect.width, 200.0);
        assert_eq!(rect.height, 200.0);
        assert_eq!(rect.y, -50.0);
    }
}
