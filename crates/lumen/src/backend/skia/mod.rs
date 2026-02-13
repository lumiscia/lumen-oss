#[cfg(all(feature = "skia-metal", target_os = "macos"))]
pub mod metal;
pub mod software;
#[cfg(feature = "skia-vulkan")]
pub mod vulkan;

use std::collections::HashMap;

use skia_safe::{
    Canvas, Color, ColorType, Data, Font, FontMgr, IPoint, ImageInfo, Paint, Point, RRect, Rect,
    Typeface, images, paint::Style as PaintStyle,
};

use crate::{
    backend::{FrameImage, FrameProvider, RenderBackend, RenderError, pixel_len},
    compile::{CompiledOperationKind, CompiledTimeline, VideoSourceRef},
    model::{ColorRgba, FitMode, Shape, ShapeClip, TextAlign, TextClip, Transform},
};

const EMBEDDED_FONT: &[u8] = include_bytes!("../../../assets/roboto/Roboto-Regular.ttf");

// -- GPU backend state (only compiled with a GPU feature) ---------------------

#[cfg(any(feature = "skia-metal", feature = "skia-vulkan"))]
#[allow(dead_code)]
enum GpuBackend {
    #[cfg(all(feature = "skia-metal", target_os = "macos"))]
    Metal(metal::MetalState),
    #[cfg(feature = "skia-vulkan")]
    Vulkan(vulkan::VulkanState),
}

#[cfg(any(feature = "skia-metal", feature = "skia-vulkan"))]
struct GpuState {
    context: skia_safe::gpu::DirectContext,
    _backend: GpuBackend,
}

// -- SkiaRenderer -------------------------------------------------------------

pub struct SkiaRenderer {
    surface: skia_safe::Surface,
    #[cfg(any(feature = "skia-metal", feature = "skia-vulkan"))]
    gpu: Option<GpuState>,
    typeface: Typeface,
    font_cache: HashMap<u32, Font>,
    width: u32,
    height: u32,
}

// Safety: SkiaRenderer is used single-threaded; the owner controls access.
// The GPU context and surface are not shared across threads concurrently.
unsafe impl Send for SkiaRenderer {}

impl SkiaRenderer {
    pub fn new(width: u32, height: u32) -> Result<Self, RenderError> {
        let typeface = load_typeface()?;

        // Try GPU backends (Metal on macOS, Vulkan on Linux), fall back to CPU raster.
        #[cfg(any(feature = "skia-metal", feature = "skia-vulkan"))]
        {
            if let Some((surface, gpu_state)) = try_create_gpu(width, height) {
                return Ok(Self {
                    surface,
                    gpu: Some(gpu_state),
                    typeface,
                    font_cache: HashMap::new(),
                    width,
                    height,
                });
            }
        }

        let surface = software::create_surface(width, height)?;

        Ok(Self {
            surface,
            #[cfg(any(feature = "skia-metal", feature = "skia-vulkan"))]
            gpu: None,
            typeface,
            font_cache: HashMap::new(),
            width,
            height,
        })
    }

    pub fn is_gpu_accelerated(&self) -> bool {
        #[cfg(any(feature = "skia-metal", feature = "skia-vulkan"))]
        {
            return self.gpu.is_some();
        }
        #[cfg(not(any(feature = "skia-metal", feature = "skia-vulkan")))]
        {
            false
        }
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<(), RenderError> {
        #[cfg(any(feature = "skia-metal", feature = "skia-vulkan"))]
        if let Some(ref mut gpu) = self.gpu {
            self.surface = create_gpu_surface(&mut gpu.context, width, height)?;
            self.width = width;
            self.height = height;
            return Ok(());
        }

        self.surface = software::create_surface(width, height)?;
        self.width = width;
        self.height = height;
        Ok(())
    }

    fn readback_rgba(&mut self) -> Result<Vec<u8>, RenderError> {
        // Flush GPU work before readback
        #[cfg(any(feature = "skia-metal", feature = "skia-vulkan"))]
        if let Some(ref mut gpu) = self.gpu {
            gpu.context.flush_and_submit();
        }

        let info = ImageInfo::new(
            (self.width as i32, self.height as i32),
            ColorType::RGBA8888,
            skia_safe::AlphaType::Unpremul,
            None,
        );
        let row_bytes = self.width as usize * 4;
        let mut pixels = vec![0u8; pixel_len(self.width, self.height)?];

        let success = self
            .surface
            .read_pixels(&info, &mut pixels, row_bytes, IPoint::new(0, 0));
        if !success {
            return Err(RenderError::SurfaceCreation("readPixels failed".into()));
        }

        Ok(pixels)
    }
}

impl RenderBackend for SkiaRenderer {
    fn render_frame(
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

        // Clear with background color
        let bg = timeline.canvas.background;
        self.surface.canvas().clear(to_sk_color(bg, 1.0));

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
                    draw_solid(self.surface.canvas(), operation.transform, opacity, *color);
                }
                CompiledOperationKind::Shape(shape) => {
                    draw_shape(self.surface.canvas(), operation.transform, opacity, shape);
                }
                CompiledOperationKind::Text(text) => {
                    draw_text(
                        self.surface.canvas(),
                        &self.typeface,
                        &mut self.font_cache,
                        operation.transform,
                        opacity,
                        text,
                    );
                }
                CompiledOperationKind::Image(image) => {
                    if let Some(frame_image) = provider.image(image.source_id.as_str())? {
                        draw_image(
                            self.surface.canvas(),
                            operation.transform,
                            opacity,
                            image.fit,
                            image.corner_radius,
                            &frame_image,
                        );
                    }
                }
                CompiledOperationKind::Video(video) => {
                    if let Some(source_frame) = resolve_video_frame(operation, video, frame)? {
                        if let Some(frame_image) =
                            provider.video_frame(video.source_id.as_str(), source_frame)?
                        {
                            draw_image(
                                self.surface.canvas(),
                                operation.transform,
                                opacity,
                                video.fit,
                                video.corner_radius,
                                &frame_image,
                            );
                        }
                    }
                }
            }
        }

        self.readback_rgba()
    }
}

// -- GPU context creation (only with GPU features) ----------------------------

#[cfg(any(feature = "skia-metal", feature = "skia-vulkan"))]
fn try_create_gpu(width: u32, height: u32) -> Option<(skia_safe::Surface, GpuState)> {
    #[cfg(all(feature = "skia-metal", target_os = "macos"))]
    {
        if let Some(result) = metal::try_create(width, height) {
            return Some(result);
        }
    }

    #[cfg(feature = "skia-vulkan")]
    {
        if let Some(result) = vulkan::try_create(width, height) {
            return Some(result);
        }
    }

    None
}

#[cfg(any(feature = "skia-metal", feature = "skia-vulkan"))]
fn create_gpu_surface(
    context: &mut skia_safe::gpu::DirectContext,
    width: u32,
    height: u32,
) -> Result<skia_safe::Surface, RenderError> {
    use skia_safe::gpu;
    let info = ImageInfo::new_n32_premul((width as i32, height as i32), None);
    gpu::surfaces::render_target(
        context,
        gpu::Budgeted::Yes,
        &info,
        None,
        gpu::SurfaceOrigin::TopLeft,
        None,
        false,
        None,
    )
    .ok_or_else(|| RenderError::SurfaceCreation("failed to create GPU render target".into()))
}

// -- Helpers ------------------------------------------------------------------

fn load_typeface() -> Result<Typeface, RenderError> {
    let font_mgr = FontMgr::new();
    let font_data = Data::new_copy(EMBEDDED_FONT);
    font_mgr
        .new_from_data(&font_data, None)
        .ok_or_else(|| RenderError::Text("failed to load embedded Roboto font".into()))
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

// -- Drawing primitives -------------------------------------------------------

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
        .map(|v| v as f64)
        .unwrap_or(source_width)
        .max(1.0);
    let target_height = transform
        .height
        .map(|v| v as f64)
        .unwrap_or(source_height)
        .max(1.0);

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

    let x = transform.x as f64 + (target_width - draw_width) / 2.0;
    let y = transform.y as f64 + (target_height - draw_height) / 2.0;

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

fn to_sk_color(c: ColorRgba, opacity: f32) -> Color {
    Color::from_argb(alpha_scaled(c.a(), opacity), c.r(), c.g(), c.b())
}

fn draw_solid(canvas: &Canvas, transform: Transform, opacity: f32, color: ColorRgba) {
    let rect = layout_rect(transform, 1.0, 1.0, FitMode::Fill);

    let mut paint = Paint::default();
    paint.set_color(to_sk_color(color, opacity));
    paint.set_anti_alias(true);
    paint.set_style(PaintStyle::Fill);

    canvas.draw_rect(
        Rect::from_xywh(
            rect.x as f32,
            rect.y as f32,
            rect.width as f32,
            rect.height as f32,
        ),
        &paint,
    );
}

fn draw_shape(canvas: &Canvas, transform: Transform, opacity: f32, shape: &ShapeClip) {
    let rect = layout_rect(transform, 1.0, 1.0, FitMode::Fill);
    let sk_rect = Rect::from_xywh(
        rect.x as f32,
        rect.y as f32,
        rect.width as f32,
        rect.height as f32,
    );

    match shape.shape {
        Shape::Rectangle { fill, radius } => {
            let mut paint = Paint::default();
            paint.set_color(to_sk_color(fill, opacity));
            paint.set_anti_alias(true);
            paint.set_style(PaintStyle::Fill);

            if radius > 0.0 {
                let rrect = RRect::new_rect_xy(sk_rect, radius, radius);
                canvas.draw_rrect(rrect, &paint);
            } else {
                canvas.draw_rect(sk_rect, &paint);
            }
        }
        Shape::Ellipse { fill } => {
            let mut paint = Paint::default();
            paint.set_color(to_sk_color(fill, opacity));
            paint.set_anti_alias(true);
            paint.set_style(PaintStyle::Fill);

            canvas.draw_oval(sk_rect, &paint);
        }
    }
}

fn draw_text(
    canvas: &Canvas,
    typeface: &Typeface,
    font_cache: &mut HashMap<u32, Font>,
    transform: Transform,
    opacity: f32,
    text: &TextClip,
) {
    let font_size = text.font_size.max(1.0);
    let font = font_cache
        .entry(font_size.to_bits())
        .or_insert_with(|| Font::from_typeface(typeface, font_size));

    let mut paint = Paint::default();
    paint.set_color(to_sk_color(text.color, opacity));
    paint.set_anti_alias(true);

    let (_, metrics) = font.metrics();
    let line_height = (metrics.descent - metrics.ascent + metrics.leading).max(font_size);

    let lines: Vec<&str> = text.text.lines().collect();
    let lines = if lines.is_empty() { vec![""] } else { lines };

    let line_widths: Vec<f32> = lines
        .iter()
        .map(|line| {
            let (width, _) = font.measure_str(line, None);
            width
        })
        .collect();

    let width_max = line_widths.iter().cloned().fold(0.0f32, f32::max);
    let target_width = transform.width.unwrap_or(width_max.max(1.0));

    let mut y_cursor = transform.y;
    let has_rotation = transform.rotation_degrees != 0.0;

    if has_rotation {
        canvas.save();
        let cx = transform.x + target_width / 2.0;
        let cy = transform.y;
        canvas.translate(Point::new(cx, cy));
        canvas.rotate(transform.rotation_degrees, None);
        canvas.translate(Point::new(-cx, -cy));
    }

    for (i, line) in lines.iter().enumerate() {
        let x = match text.align {
            TextAlign::Left => transform.x,
            TextAlign::Center => transform.x + (target_width - line_widths[i]) / 2.0,
            TextAlign::Right => transform.x + (target_width - line_widths[i]),
        };

        canvas.draw_str(line, Point::new(x, y_cursor + font_size), font, &paint);
        y_cursor += line_height;
    }

    if has_rotation {
        canvas.restore();
    }
}

fn draw_image(
    canvas: &Canvas,
    transform: Transform,
    opacity: f32,
    fit: FitMode,
    corner_radius: f32,
    image: &FrameImage,
) {
    let info = ImageInfo::new(
        (image.width as i32, image.height as i32),
        ColorType::RGBA8888,
        skia_safe::AlphaType::Unpremul,
        None,
    );

    let row_bytes = image.width as usize * 4;
    // SAFETY: `image.rgba` is borrowed from the FrameProvider and outlives
    // both the `Data` and the `sk_image` created from it – they are local
    // to this function and dropped before it returns.
    let data = unsafe { Data::new_bytes(&image.rgba) };
    let sk_image = match images::raster_from_data(&info, data, row_bytes) {
        Some(img) => img,
        None => return,
    };

    let rect = layout_rect(transform, image.width as f64, image.height as f64, fit);

    let scale_x = rect.width as f32 / image.width as f32;
    let scale_y = rect.height as f32 / image.height as f32;

    canvas.save();

    if transform.rotation_degrees != 0.0 {
        let cx = rect.x as f32 + rect.width as f32 / 2.0;
        let cy = rect.y as f32 + rect.height as f32 / 2.0;
        canvas.translate(Point::new(cx, cy));
        canvas.rotate(transform.rotation_degrees, None);
        canvas.translate(Point::new(-cx, -cy));
    }

    canvas.translate(Point::new(rect.x as f32, rect.y as f32));
    canvas.scale((scale_x, scale_y));

    if corner_radius > 0.0 {
        let min_scale = scale_x.abs().min(scale_y.abs()).max(f32::EPSILON);
        let source_radius =
            (corner_radius / min_scale).min((image.width.min(image.height) as f32) * 0.5);
        let clip = RRect::new_rect_xy(
            Rect::from_xywh(0.0, 0.0, image.width as f32, image.height as f32),
            source_radius,
            source_radius,
        );
        canvas.clip_rrect(clip, None, Some(true));
    }

    let mut paint = Paint::default();
    paint.set_alpha_f(opacity);

    canvas.draw_image(&sk_image, Point::new(0.0, 0.0), Some(&paint));
    canvas.restore();
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn renderer_creates_successfully() {
        let renderer = SkiaRenderer::new(320, 240);
        assert!(renderer.is_ok());
    }
}
