use std::sync::Arc;

use skia_safe::{
    BlendMode, Color, EncodedImageFormat, Font, Paint, Rect, Surface,
    image::{CachingHint, Image},
    surfaces,
    utils::text_utils::Align,
};
use thiserror::Error;

use crate::{
    font::{FONT_ARIAL, FontManager},
    plan::{RenderOpKind, RenderPlan, SolidRenderOp, TextRenderOp},
    sequence::{BlendMode as SequenceBlendMode, TextAlign},
    time::FrameIndex,
};

#[derive(Error, Debug)]
pub enum RendererError {
    #[error("Skia error: {0}")]
    SkiaError(String),
    #[error("font `{0}` was not available")]
    FontMissing(String),
    #[error("frame {frame} is out of range for total frames {total_frames}")]
    FrameOutOfRange { frame: u64, total_frames: u64 },
    #[error("the provided buffer length ({provided}) was not expected length ({expected})")]
    MismatchedBufferLength { provided: usize, expected: usize },
    #[error("failed to encode frame as png")]
    PngEncodeFailed,
}

pub struct RenderContext {
    pub width: usize,
    pub height: usize,
    pub rate: u16,
    pub surface: Surface,
    pub font_manager: Box<dyn FontManager>,
}

pub struct Renderer {
    plan: Arc<RenderPlan>,
    image: skia_safe::ImageInfo,
    pub context: RenderContext,
}

impl Renderer {
    pub fn new(
        plan: Arc<RenderPlan>,
        font_manager: impl FontManager + 'static,
    ) -> Result<Self, RendererError> {
        let width = plan.canvas.width as usize;
        let height = plan.canvas.height as usize;
        let surface = surfaces::raster_n32_premul((width as i32, height as i32))
            .ok_or_else(|| RendererError::SkiaError("failed to create raster surface".to_string()))?;

        Ok(Self {
            image: skia_safe::ImageInfo::new_n32_premul((width as i32, height as i32), None),
            context: RenderContext {
                width,
                height,
                rate: plan.fps.as_f64() as u16,
                surface,
                font_manager: Box::new(font_manager),
            },
            plan,
        })
    }

    pub fn draw_frame(&mut self, frame: FrameIndex) -> Result<(), RendererError> {
        if frame.0 >= self.plan.total_frames {
            return Err(RendererError::FrameOutOfRange {
                frame: frame.0,
                total_frames: self.plan.total_frames,
            });
        }

        let frame_ops: Vec<_> = self.plan.operations_for_frame(frame).cloned().collect();

        let canvas = self.context.surface.canvas();
        canvas.clear(to_color(self.plan.canvas.background));

        for op in frame_ops {
            match &op.kind {
                RenderOpKind::Text(text) => self.draw_text(text, op.opacity, op.blend_mode)?,
                RenderOpKind::Solid(solid) => self.draw_solid(solid, op.opacity, op.blend_mode),
                RenderOpKind::Image(_) => {
                    self.draw_placeholder(op.opacity, op.blend_mode, Color::from_argb(255, 60, 120, 220))
                }
                RenderOpKind::Video(_) => {
                    self.draw_placeholder(op.opacity, op.blend_mode, Color::from_argb(255, 220, 120, 40))
                }
            }
        }

        Ok(())
    }

    pub fn read_rgba(&mut self, buffer: &mut [u8]) -> Result<(), RendererError> {
        let expected = self.context.width * self.context.height * 4;
        if buffer.len() != expected {
            return Err(RendererError::MismatchedBufferLength {
                provided: buffer.len(),
                expected,
            });
        }

        self.context.surface.image_snapshot().read_pixels(
            &self.image,
            buffer,
            self.context.width * 4,
            (0, 0),
            CachingHint::Disallow,
        );

        Ok(())
    }

    pub fn encode_png(&mut self) -> Result<Vec<u8>, RendererError> {
        let data = self
            .context
            .surface
            .image_snapshot()
            .encode(None, EncodedImageFormat::PNG, 95)
            .ok_or(RendererError::PngEncodeFailed)?;
        Ok(data.to_vec())
    }

    pub fn snapshot(&mut self) -> Image {
        self.context.surface.image_snapshot()
    }

    fn draw_text(
        &mut self,
        text: &TextRenderOp,
        opacity: f32,
        blend_mode: SequenceBlendMode,
    ) -> Result<(), RendererError> {
        let family = text.font_family.as_deref().unwrap_or(FONT_ARIAL);
        let typeface = self
            .context
            .font_manager
            .named(family)
            .or_else(|| self.context.font_manager.arial())
            .ok_or_else(|| RendererError::FontMissing(family.to_string()))?;

        let mut color = text.color.as_color4f();
        color.a *= opacity.clamp(0.0, 1.0);

        let mut paint = Paint::new(color, None);
        paint.set_anti_alias(true);
        paint.set_blend_mode(to_blend_mode(blend_mode));

        let font = Font::new(typeface, text.font_size.max(1.0));
        let align = match text.align {
            TextAlign::Left => Align::Left,
            TextAlign::Center => Align::Center,
            TextAlign::Right => Align::Right,
        };

        self.context.surface.canvas().draw_str_align(
            &text.text,
            (
                self.context.width as i32 / 2,
                self.context.height as i32 / 2,
            ),
            &font,
            &paint,
            align,
        );

        Ok(())
    }

    fn draw_solid(&mut self, solid: &SolidRenderOp, opacity: f32, blend_mode: SequenceBlendMode) {
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_blend_mode(to_blend_mode(blend_mode));

        let mut color = solid.color.as_color4f();
        color.a *= opacity.clamp(0.0, 1.0);
        paint.set_color4f(color, None);

        let rect = Rect::from_xywh(
            0.0,
            0.0,
            self.context.width as f32,
            self.context.height as f32,
        );
        self.context.surface.canvas().draw_rect(rect, &paint);
    }

    fn draw_placeholder(&mut self, opacity: f32, blend_mode: SequenceBlendMode, color: Color) {
        let mut paint = Paint::default();
        paint.set_color(color.with_a((255.0 * opacity.clamp(0.0, 1.0)) as u8));
        paint.set_blend_mode(to_blend_mode(blend_mode));
        paint.set_anti_alias(true);

        let rect = Rect::from_xywh(
            self.context.width as f32 * 0.1,
            self.context.height as f32 * 0.1,
            self.context.width as f32 * 0.8,
            self.context.height as f32 * 0.8,
        );
        self.context.surface.canvas().draw_rect(rect, &paint);
    }
}

fn to_color(color: crate::sequence::ColorRGBA) -> Color {
    Color::from_argb(color.a(), color.r(), color.g(), color.b())
}

fn to_blend_mode(mode: SequenceBlendMode) -> BlendMode {
    match mode {
        SequenceBlendMode::Normal => BlendMode::SrcOver,
        SequenceBlendMode::Multiply => BlendMode::Multiply,
        SequenceBlendMode::Screen => BlendMode::Screen,
    }
}
