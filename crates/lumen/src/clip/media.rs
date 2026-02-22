use std::ops::Range;

use skia_safe::{Color, Paint, Rect};

use crate::clip::{Clip, ClipMeta, style::BaseStyle};
use crate::render::backend::RenderError;
use crate::render::context::{FrameContext, RendererContext};
use crate::time::Rational;

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

        self.style
            .draw(frame, frame_ctx, renderer_ctx, |renderer_ctx, _resolved| {
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
            })
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

impl VideoClip {
    fn visible_duration_frames(&self) -> u64 {
        u64::from(self.end().saturating_sub(self.start()).saturating_add(1))
    }

    fn trim_range_frames(
        &self,
        fps: Rational,
        source_duration_frames: Option<u64>,
    ) -> Option<(u64, u64)> {
        match (&self.trim, source_duration_frames) {
            (Some(trim), _) => {
                let fps = fps.as_f32();
                if !fps.is_finite() || fps <= 0.0 {
                    return None;
                }
                let start = (trim.start.max(0.0) * fps).floor() as u64;
                let end = (trim.end.max(0.0) * fps).floor() as u64;
                if end <= start {
                    return None;
                }
                Some((start, end - start))
            }
            (None, Some(duration)) if duration > 0 => Some((0, duration)),
            (None, _) => {
                let duration = self.visible_duration_frames();
                if duration == 0 {
                    None
                } else {
                    Some((0, duration))
                }
            }
        }
    }

    pub fn map_to_source_frame(
        &self,
        timeline_frame: u32,
        fps: Rational,
        source_duration_frames: Option<u64>,
    ) -> Option<u32> {
        let clip_frame = u64::from(timeline_frame.saturating_sub(self.start()));
        let speed = if self.speed.is_finite() && self.speed != 0.0 {
            self.speed as f64
        } else {
            1.0
        };
        let reverse = speed.is_sign_negative();
        let stepped = (clip_frame as f64 * speed.abs()).floor().max(0.0) as u64;

        let (trim_start, trim_duration) = self.trim_range_frames(fps, source_duration_frames)?;
        if trim_duration == 0 {
            return None;
        }

        let forward_offset = match self.r#loop {
            LoopMode::None => stepped.min(trim_duration.saturating_sub(1)),
            LoopMode::Repeat => stepped % trim_duration,
            LoopMode::PingPong => {
                let cycle = trim_duration.saturating_mul(2);
                let pos = if cycle == 0 { 0 } else { stepped % cycle };
                if pos < trim_duration {
                    pos
                } else {
                    cycle.saturating_sub(pos).saturating_sub(1)
                }
            }
        };

        let offset = if reverse {
            trim_duration
                .saturating_sub(1)
                .saturating_sub(forward_offset)
        } else {
            forward_offset
        };
        let source_frame = trim_start.saturating_add(offset);
        u32::try_from(source_frame).ok()
    }
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

        self.style
            .draw(frame, frame_ctx, renderer_ctx, |renderer_ctx, _resolved| {
                let mapped_frame = self.map_to_source_frame(frame, renderer_ctx.frame_rate, None);
                let Some(mapped_frame) = mapped_frame else {
                    return Ok(());
                };
                let media_store = renderer_ctx
                    .media_store_mut()
                    .ok_or_else(|| RenderError::MissingSource(format!("video:{}", self.source)))?;
                let mut resolver = media_store
                    .get_video_resolver(self.source.as_str())
                    .ok_or_else(|| RenderError::MissingSource(format!("video:{}", self.source)))?;

                let _ = resolver.resolve_frame(mapped_frame);
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
            })
    }
}

#[cfg(test)]
mod tests {
    use skia_safe::BlendMode;

    use super::{LoopMode, VideoClip};
    use crate::clip::{
        ClipMeta,
        style::{BaseStyle, StyleProperty, StyleValue, TransformStyle},
    };
    use crate::time::Rational;

    fn literal<T>(value: T) -> StyleProperty<T> {
        StyleProperty::Value(StyleValue::Literal(value))
    }

    fn base_style() -> BaseStyle {
        BaseStyle {
            visible: literal(true),
            opacity: literal(1.0),
            blend_mode: BlendMode::SrcOver,
            blur: literal(0.0),
            shadow: None,
            transform: TransformStyle {
                translate: literal(0.0),
                scale: literal(1.0),
            },
            alignment: [literal(0.0), literal(0.0)],
        }
    }

    fn video_clip() -> VideoClip {
        VideoClip {
            meta: ClipMeta {
                id: Some("video".to_owned()),
                start_frame: 10,
                end_frame: 19,
            },
            source: "video".to_owned(),
            style: base_style(),
            trim: None,
            speed: 1.0,
            r#loop: LoopMode::None,
        }
    }

    #[test]
    fn map_to_source_frame_defaults_to_clip_duration_window() {
        let clip = video_clip();
        let fps = Rational::new(30, 1);

        assert_eq!(clip.map_to_source_frame(10, fps, None), Some(0));
        assert_eq!(clip.map_to_source_frame(15, fps, None), Some(5));
        assert_eq!(clip.map_to_source_frame(19, fps, None), Some(9));
        assert_eq!(clip.map_to_source_frame(25, fps, None), Some(9));
    }

    #[test]
    fn map_to_source_frame_applies_trim_in_seconds() {
        let mut clip = video_clip();
        clip.trim = Some(1.0..2.0);
        let fps = Rational::new(30, 1);

        assert_eq!(clip.map_to_source_frame(10, fps, Some(300)), Some(30));
        assert_eq!(clip.map_to_source_frame(19, fps, Some(300)), Some(39));
    }

    #[test]
    fn map_to_source_frame_applies_speed_multiplier() {
        let mut clip = video_clip();
        clip.speed = 2.0;
        let fps = Rational::new(24, 1);

        assert_eq!(clip.map_to_source_frame(10, fps, Some(100)), Some(0));
        assert_eq!(clip.map_to_source_frame(11, fps, Some(100)), Some(2));
        assert_eq!(clip.map_to_source_frame(13, fps, Some(100)), Some(6));
    }

    #[test]
    fn map_to_source_frame_supports_repeat_loop() {
        let mut clip = video_clip();
        clip.r#loop = LoopMode::Repeat;
        let fps = Rational::new(30, 1);

        assert_eq!(clip.map_to_source_frame(19, fps, Some(4)), Some(1));
        assert_eq!(clip.map_to_source_frame(20, fps, Some(4)), Some(2));
    }

    #[test]
    fn map_to_source_frame_supports_ping_pong_loop() {
        let mut clip = video_clip();
        clip.r#loop = LoopMode::PingPong;
        let fps = Rational::new(30, 1);

        let mapped = (10..18)
            .map(|frame| {
                clip.map_to_source_frame(frame, fps, Some(3))
                    .expect("mapped")
            })
            .collect::<Vec<_>>();

        assert_eq!(mapped, vec![0, 1, 2, 2, 1, 0, 0, 1]);
    }

    #[test]
    fn map_to_source_frame_supports_negative_speed_reverse_playback() {
        let mut clip = video_clip();
        clip.speed = -1.0;
        let fps = Rational::new(30, 1);

        assert_eq!(clip.map_to_source_frame(10, fps, Some(5)), Some(4));
        assert_eq!(clip.map_to_source_frame(11, fps, Some(5)), Some(3));
        assert_eq!(clip.map_to_source_frame(20, fps, Some(5)), Some(0));
    }

    #[test]
    fn map_to_source_frame_returns_none_for_invalid_trim_or_fps() {
        let mut clip = video_clip();
        clip.trim = Some(2.0..1.0);
        assert_eq!(
            clip.map_to_source_frame(10, Rational::new(30, 1), Some(300)),
            None
        );

        clip.trim = Some(0.0..1.0);
        assert_eq!(
            clip.map_to_source_frame(10, Rational::new(0, 1), Some(300)),
            None
        );
    }
}
