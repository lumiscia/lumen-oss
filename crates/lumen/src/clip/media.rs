use std::ops::Range;

use skia_safe::{Color, Paint, Rect};

use crate::clip::{Clip, ClipMeta, draw_with_base_style, style::BaseStyle};
use crate::render::backend::RenderError;
use crate::render::context::{FrameContext, RendererContext};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopMode {
    None,
    Repeat,
    PingPong,
}

#[derive(Debug, Clone)]
pub struct ImageClip {
    pub meta: ClipMeta,
    pub source: String,
    pub style: BaseStyle,
}

impl Clip for ImageClip {
    fn meta(&self) -> &ClipMeta {
        &self.meta
    }

    fn draw(
        &self,
        frame: u32,
        frame_ctx: &FrameContext,
        renderer_ctx: &mut RendererContext,
    ) -> Result<(), RenderError> {
        if !self.contains_frame(frame) {
            return Ok(());
        }

        draw_with_base_style(
            &self.style,
            frame_ctx,
            renderer_ctx,
            |renderer_ctx, _resolved| {
                let (width, height, color) = match renderer_ctx.media_store_mut() {
                    Some(media_store) => match media_store.get_image_resolver(self.source.as_str())
                    {
                        Some(resolver) => (
                            resolver.width() as f32,
                            resolver.height() as f32,
                            Color::from_argb(255, 90, 220, 140),
                        ),
                        None => (
                            frame_ctx.width as f32 * 0.4,
                            frame_ctx.height as f32 * 0.3,
                            Color::from_argb(255, 110, 170, 255),
                        ),
                    },
                    None => (
                        frame_ctx.width as f32 * 0.4,
                        frame_ctx.height as f32 * 0.3,
                        Color::from_argb(255, 110, 170, 255),
                    ),
                };

                let mut paint = Paint::default();
                paint.set_anti_alias(true);
                paint.set_color(color);

                renderer_ctx.canvas().draw_rect(
                    Rect::from_xywh(
                        frame_ctx.width as f32 * 0.1,
                        frame_ctx.height as f32 * 0.1,
                        width.max(1.0),
                        height.max(1.0),
                    ),
                    &paint,
                );

                Ok(())
            },
        )
    }
}

#[derive(Debug, Clone)]
pub struct VideoClip {
    pub meta: ClipMeta,
    pub source: String,
    pub style: BaseStyle,
    pub trim: Option<Range<f32>>,
    pub speed: f32,
    pub r#loop: LoopMode,
}

impl Clip for VideoClip {
    fn meta(&self) -> &ClipMeta {
        &self.meta
    }

    fn draw(
        &self,
        frame: u32,
        frame_ctx: &FrameContext,
        renderer_ctx: &mut RendererContext,
    ) -> Result<(), RenderError> {
        if !self.contains_frame(frame) {
            return Ok(());
        }

        draw_with_base_style(
            &self.style,
            frame_ctx,
            renderer_ctx,
            |renderer_ctx, _resolved| {
                let media_store = renderer_ctx
                    .media_store_mut()
                    .ok_or_else(|| RenderError::MissingSource(format!("video:{}", self.source)))?;
                let mut resolver = media_store
                    .get_video_resolver(self.source.as_str())
                    .ok_or_else(|| RenderError::MissingSource(format!("video:{}", self.source)))?;

                let _ = resolver.resolve_frame(frame);
                let width = resolver.width() as f32;
                let height = resolver.height() as f32;

                let mut body = Paint::default();
                body.set_anti_alias(true);
                body.set_color(Color::from_argb(255, 180, 120, 255));

                let x = frame_ctx.width as f32 * 0.1;
                let y = frame_ctx.height as f32 * 0.5;
                renderer_ctx.canvas().draw_rect(
                    Rect::from_xywh(x, y, width.max(1.0), height.max(1.0)),
                    &body,
                );

                let progress = if self.end() > self.start() {
                    (frame.saturating_sub(self.start()) as f32 / (self.end() - self.start()) as f32)
                        .clamp(0.0, 1.0)
                } else {
                    0.0
                };

                let mut progress_paint = Paint::default();
                progress_paint.set_color(Color::from_argb(255, 240, 80, 80));
                renderer_ctx.canvas().draw_rect(
                    Rect::from_xywh(x, y + height.max(1.0) - 8.0, width.max(1.0) * progress, 8.0),
                    &progress_paint,
                );

                Ok(())
            },
        )
    }
}
