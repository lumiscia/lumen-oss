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
    media::{MediaError, MediaProvider, NoopMediaProvider},
    plan::{RenderOp, RenderOpKind, RenderPlan, ShapeRenderOp, SolidRenderOp, TextRenderOp},
    sequence::{BlendMode as SequenceBlendMode, ShapeContent, TextAlign},
    time::FrameIndex,
};

#[derive(Error, Debug)]
pub enum RendererError {
    #[error("Skia error: {0}")]
    SkiaError(String),
    #[error("font `{0}` was not available")]
    FontMissing(String),
    #[error("media error: {0}")]
    Media(#[from] MediaError),
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
    media: Box<dyn MediaProvider>,
    pub context: RenderContext,
}

impl Renderer {
    pub fn new(
        plan: Arc<RenderPlan>,
        font_manager: impl FontManager + 'static,
        media_provider: impl MediaProvider + 'static,
    ) -> Result<Self, RendererError> {
        let width = plan.canvas.width as usize;
        let height = plan.canvas.height as usize;
        let surface = surfaces::raster_n32_premul((width as i32, height as i32))
            .ok_or_else(|| RendererError::SkiaError("failed to create raster surface".to_string()))?;

        Ok(Self {
            image: skia_safe::ImageInfo::new_n32_premul((width as i32, height as i32), None),
            media: Box::new(media_provider),
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

    pub fn new_without_media(
        plan: Arc<RenderPlan>,
        font_manager: impl FontManager + 'static,
    ) -> Result<Self, RendererError> {
        Self::new(plan, font_manager, NoopMediaProvider)
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
            self.draw_op(frame, &op)?;
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

    fn draw_op(&mut self, frame: FrameIndex, op: &RenderOp) -> Result<(), RendererError> {
        match &op.kind {
            RenderOpKind::Text(text) => self.draw_text(op, text),
            RenderOpKind::Shape(shape) => {
                self.draw_shape(op, shape);
                Ok(())
            }
            RenderOpKind::Solid(solid) => {
                self.draw_solid(op, solid);
                Ok(())
            }
            RenderOpKind::Image(asset) => {
                if let Some(image) = self.media.image(&asset.asset_id)? {
                    self.draw_image(op, &image);
                }
                Ok(())
            }
            RenderOpKind::Video(asset) => {
                let local_frame = frame.0.saturating_sub(op.start_frame.0);
                let mut source_offset = ((local_frame as f64) * (asset.speed as f64)).floor() as u64;
                if asset.source_span_frames > 0 {
                    source_offset = source_offset.min(asset.source_span_frames.saturating_sub(1));
                }

                let source_offset = if asset.reverse {
                    asset
                        .source_span_frames
                        .saturating_sub(1)
                        .saturating_sub(source_offset)
                } else {
                    source_offset
                };

                let source_frame = FrameIndex(op.source_in_frame.0.saturating_add(source_offset));

                if let Some(image) = self.media.video_frame(&asset.asset_id, source_frame)? {
                    self.draw_image(op, &image);
                }
                Ok(())
            }
        }
    }

    fn draw_text(&mut self, op: &RenderOp, text: &TextRenderOp) -> Result<(), RendererError> {
        let family = text.font_family.as_deref().unwrap_or(FONT_ARIAL);
        let typeface = self
            .context
            .font_manager
            .named(family)
            .or_else(|| self.context.font_manager.arial())
            .ok_or_else(|| RendererError::FontMissing(family.to_string()))?;

        let mut color = text.color.as_color4f();
        color.a *= op.opacity.clamp(0.0, 1.0);

        let mut paint = Paint::new(color, None);
        paint.set_anti_alias(true);
        paint.set_blend_mode(to_blend_mode(op.blend_mode));

        let font = Font::new(typeface, text.font_size.max(1.0));
        let align = match text.align {
            TextAlign::Left => Align::Left,
            TextAlign::Center => Align::Center,
            TextAlign::Right => Align::Right,
        };

        self.context
            .surface
            .canvas()
            .draw_str_align(&text.text, (op.transform.x, op.transform.y), &font, &paint, align);

        Ok(())
    }

    fn draw_shape(&mut self, op: &RenderOp, shape: &ShapeRenderOp) {
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_blend_mode(to_blend_mode(op.blend_mode));

        match &shape.shape {
            ShapeContent::Rectangle { fill, radius: _ } => {
                let mut color = fill.as_color4f();
                color.a *= op.opacity.clamp(0.0, 1.0);
                paint.set_color4f(color, None);
                self.context
                    .surface
                    .canvas()
                    .draw_rect(op_rect(op, self.context.width as f32, self.context.height as f32), &paint);
            }
            ShapeContent::Ellipse { fill } => {
                let mut color = fill.as_color4f();
                color.a *= op.opacity.clamp(0.0, 1.0);
                paint.set_color4f(color, None);
                self.context
                    .surface
                    .canvas()
                    .draw_oval(op_rect(op, self.context.width as f32, self.context.height as f32), &paint);
            }
        }
    }

    fn draw_solid(&mut self, op: &RenderOp, solid: &SolidRenderOp) {
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_blend_mode(to_blend_mode(op.blend_mode));

        let mut color = solid.color.as_color4f();
        color.a *= op.opacity.clamp(0.0, 1.0);
        paint.set_color4f(color, None);

        self.context
            .surface
            .canvas()
            .draw_rect(op_rect(op, self.context.width as f32, self.context.height as f32), &paint);
    }

    fn draw_image(&mut self, op: &RenderOp, image: &Image) {
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_blend_mode(to_blend_mode(op.blend_mode));

        let src = Rect::from_xywh(0.0, 0.0, image.width() as f32, image.height() as f32);
        let dst = op_rect(op, image.width() as f32, image.height() as f32);

        self.context
            .surface
            .canvas()
            .draw_image_rect_with_sampling_options(
                image,
                Some((&src, skia_safe::canvas::SrcRectConstraint::Fast)),
                dst,
                skia_safe::SamplingOptions::default(),
                &paint,
            );
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

fn op_rect(op: &RenderOp, default_width: f32, default_height: f32) -> Rect {
    Rect::from_xywh(
        op.transform.x,
        op.transform.y,
        op.transform.width.unwrap_or(default_width),
        op.transform.height.unwrap_or(default_height),
    )
}
