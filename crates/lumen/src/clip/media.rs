use std::ops::Range;

use skia_safe::{Color, Data, ImageInfo, Paint, Rect, images};

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
                let x = frame_ctx.width as f32 * 0.1;
                let y = frame_ctx.height as f32 * 0.1;

                if let Some(media_store) = renderer_ctx.media_store_mut() {
                    if let Some(mut resolver) = media_store.get_image_resolver(self.source.as_str())
                    {
                        let width = resolver.width();
                        let height = resolver.height();
                        let pixels = resolver.resolve();
                        draw_rgba_image(
                            renderer_ctx,
                            x,
                            y,
                            width.max(1),
                            height.max(1),
                            pixels.as_slice(),
                        )?;
                        return Ok(());
                    }
                }

                let width = frame_ctx.width as f32 * 0.4;
                let height = frame_ctx.height as f32 * 0.3;
                let color = Color::from_argb(255, 110, 170, 255);

                let mut paint = Paint::default();
                paint.set_anti_alias(true);
                paint.set_color(color);

                renderer_ctx.canvas().draw_rect(
                    Rect::from_xywh(x, y, width.max(1.0), height.max(1.0)),
                    &paint,
                );

                Ok(())
            })
    }
}

fn draw_rgba_image(
    renderer_ctx: &mut RendererContext,
    x: f32,
    y: f32,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Result<(), RenderError> {
    let expected_len = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(4);
    if pixels.len() != expected_len {
        return Err(RenderError::Unsupported("invalid image buffer length"));
    }

    let info = ImageInfo::new(
        (width as i32, height as i32),
        skia_safe::ColorType::RGBA8888,
        skia_safe::AlphaType::Unpremul,
        None,
    );
    let data = Data::new_copy(pixels);
    let image = images::raster_from_data(&info, data, width as usize * 4).ok_or(
        RenderError::Unsupported("failed to create image from RGBA pixels"),
    )?;

    let mut paint = Paint::default();
    paint.set_anti_alias(false);
    renderer_ctx.canvas().draw_image_rect(
        image,
        None,
        Rect::from_xywh(x, y, width as f32, height as f32),
        &paint,
    );
    Ok(())
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

                let x = frame_ctx.width as f32 * 0.1;
                let y = frame_ctx.height as f32 * 0.5;
                let width = resolver.width().max(1);
                let height = resolver.height().max(1);
                let pixels = resolver.resolve_frame(mapped_frame);
                draw_rgba_image(renderer_ctx, x, y, width, height, pixels.as_slice())?;

                let progress = if self.end() > self.start() {
                    (frame.saturating_sub(self.start()) as f32 / (self.end() - self.start()) as f32)
                        .clamp(0.0, 1.0)
                } else {
                    0.0
                };

                let mut progress_paint = Paint::default();
                progress_paint.set_color(Color::from_argb(255, 240, 80, 80));
                renderer_ctx.canvas().draw_rect(
                    Rect::from_xywh(x, y + height as f32 - 8.0, width as f32 * progress, 8.0),
                    &progress_paint,
                );

                Ok(())
            })
    }
}

#[cfg(test)]
mod tests {
    use skia_safe::BlendMode;

    use super::{ImageClip, LoopMode, VideoClip};
    use crate::clip::{
        Clip, ClipMeta,
        style::{BaseStyle, StyleProperty, StyleValue, TransformStyle},
    };
    use crate::media::{ImageResolver, MediaStore, VideoResolver};
    use crate::render::{
        backend::{RenderError, read_surface_rgba},
        context::{FrameContext, RendererContext},
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
            clip_radius: [literal(0.0), literal(0.0), literal(0.0), literal(0.0)],
            transform: TransformStyle {
                translate: [literal(0.0), literal(0.0)],
                scale: [literal(1.0), literal(1.0)],
                rotation: literal(0.0),
                skew: [literal(0.0), literal(0.0)],
                origin: [literal(0.0), literal(0.0)],
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

    struct TestImageResolver {
        id: String,
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    }

    impl ImageResolver for TestImageResolver {
        fn id(&self) -> String {
            self.id.clone()
        }

        fn width(&self) -> u32 {
            self.width
        }

        fn height(&self) -> u32 {
            self.height
        }

        fn resolve(&mut self) -> Vec<u8> {
            self.pixels.clone()
        }
    }

    struct TestVideoResolver {
        width: u32,
        height: u32,
        pixels: Vec<u8>,
        last_requested_frame: Option<u32>,
    }

    impl VideoResolver for TestVideoResolver {
        fn id(&self) -> String {
            "video".to_owned()
        }

        fn width(&self) -> u32 {
            self.width
        }

        fn height(&self) -> u32 {
            self.height
        }

        fn resolve_frame(&mut self, frame: u32) -> Vec<u8> {
            self.last_requested_frame = Some(frame);
            self.pixels.clone()
        }
    }

    struct TestMediaStore {
        image: Option<TestImageResolver>,
        video: Option<TestVideoResolver>,
    }

    impl MediaStore for TestMediaStore {
        fn get_image_resolver(&mut self, id: &str) -> Option<Box<dyn ImageResolver>> {
            let resolver = self.image.take()?;
            if resolver.id == id {
                Some(Box::new(resolver))
            } else {
                None
            }
        }

        fn get_video_resolver(&mut self, _id: &str) -> Option<Box<dyn VideoResolver>> {
            self.video
                .take()
                .map(|resolver| Box::new(resolver) as Box<dyn VideoResolver>)
        }
    }

    #[test]
    fn image_clip_draws_rgba_pixels_from_resolver() {
        let mut renderer_ctx =
            RendererContext::new(100, 100, Rational::new(30, 1)).expect("renderer context");
        renderer_ctx.set_media_store(Box::new(TestMediaStore {
            image: Some(TestImageResolver {
                id: "img".to_owned(),
                width: 2,
                height: 2,
                pixels: vec![
                    255, 0, 0, 255, 0, 255, 0, 255, // row 0
                    0, 0, 255, 255, 255, 255, 255, 255, // row 1
                ],
            }),
            video: None,
        }));
        renderer_ctx.clear();

        let clip = ImageClip {
            meta: ClipMeta {
                id: Some("img".to_owned()),
                start_frame: 0,
                end_frame: 10,
            },
            source: "img".to_owned(),
            style: base_style(),
        };
        let frame_ctx = FrameContext {
            frame: 0,
            time_seconds: 0.0,
            width: 100,
            height: 100,
            device_scale: 1.0,
        };

        clip.draw(0, &frame_ctx, &mut renderer_ctx)
            .expect("image clip should draw");

        let pixels = read_surface_rgba(&mut renderer_ctx).expect("readback");
        let idx = |x: usize, y: usize| (y * 100 + x) * 4;

        let tl = &pixels[idx(10, 10)..idx(10, 10) + 4];
        let br = &pixels[idx(11, 11)..idx(11, 11) + 4];

        assert_eq!(tl, &[255, 0, 0, 255]);
        assert_eq!(br, &[255, 255, 255, 255]);
    }

    #[test]
    fn image_clip_falls_back_to_placeholder_when_resolver_missing() {
        let mut renderer_ctx =
            RendererContext::new(100, 100, Rational::new(30, 1)).expect("renderer context");
        renderer_ctx.set_media_store(Box::new(TestMediaStore {
            image: None,
            video: None,
        }));
        renderer_ctx.clear();

        let clip = ImageClip {
            meta: ClipMeta {
                id: Some("img".to_owned()),
                start_frame: 0,
                end_frame: 10,
            },
            source: "img".to_owned(),
            style: base_style(),
        };
        let frame_ctx = FrameContext {
            frame: 0,
            time_seconds: 0.0,
            width: 100,
            height: 100,
            device_scale: 1.0,
        };

        clip.draw(0, &frame_ctx, &mut renderer_ctx)
            .expect("placeholder image clip should draw");

        let pixels = read_surface_rgba(&mut renderer_ctx).expect("readback");
        let idx = (10usize + 10usize * 100) * 4;
        assert_eq!(&pixels[idx..idx + 4], &[110, 170, 255, 255]);
    }

    #[test]
    fn image_clip_respects_per_axis_translation_transform() {
        let mut renderer_ctx =
            RendererContext::new(100, 100, Rational::new(30, 1)).expect("renderer context");
        renderer_ctx.set_media_store(Box::new(TestMediaStore {
            image: Some(TestImageResolver {
                id: "img".to_owned(),
                width: 1,
                height: 1,
                pixels: vec![200, 10, 20, 255],
            }),
            video: None,
        }));
        renderer_ctx.clear();

        let mut style = base_style();
        style.transform.translate = [literal(5.0), literal(7.0)];

        let clip = ImageClip {
            meta: ClipMeta {
                id: Some("img".to_owned()),
                start_frame: 0,
                end_frame: 10,
            },
            source: "img".to_owned(),
            style,
        };
        let frame_ctx = FrameContext {
            frame: 0,
            time_seconds: 0.0,
            width: 100,
            height: 100,
            device_scale: 1.0,
        };

        clip.draw(0, &frame_ctx, &mut renderer_ctx)
            .expect("translated image clip should draw");

        let pixels = read_surface_rgba(&mut renderer_ctx).expect("readback");
        let idx = |x: usize, y: usize| (y * 100 + x) * 4;
        assert_eq!(&pixels[idx(15, 17)..idx(15, 17) + 4], &[200, 10, 20, 255]);
    }

    #[test]
    fn image_clip_rotation_respects_transform_origin() {
        let mut renderer_ctx =
            RendererContext::new(100, 100, Rational::new(30, 1)).expect("renderer context");
        renderer_ctx.set_media_store(Box::new(TestMediaStore {
            image: Some(TestImageResolver {
                id: "img".to_owned(),
                width: 1,
                height: 1,
                pixels: vec![25, 200, 50, 255],
            }),
            video: None,
        }));
        renderer_ctx.clear();

        let mut style = base_style();
        style.transform.rotation = literal(90.0);
        style.transform.origin = [literal(0.1), literal(0.1)];

        let clip = ImageClip {
            meta: ClipMeta {
                id: Some("img".to_owned()),
                start_frame: 0,
                end_frame: 10,
            },
            source: "img".to_owned(),
            style,
        };
        let frame_ctx = FrameContext {
            frame: 0,
            time_seconds: 0.0,
            width: 100,
            height: 100,
            device_scale: 1.0,
        };

        clip.draw(0, &frame_ctx, &mut renderer_ctx)
            .expect("rotated image clip should draw");

        let pixels = read_surface_rgba(&mut renderer_ctx).expect("readback");
        let idx = |x: usize, y: usize| (y * 100 + x) * 4;

        assert_eq!(&pixels[idx(9, 10)..idx(9, 10) + 4], &[25, 200, 50, 255]);
    }

    #[test]
    fn image_clip_base_style_clip_radius_clips_frame_corner() {
        let mut renderer_ctx =
            RendererContext::new(100, 100, Rational::new(30, 1)).expect("renderer context");
        renderer_ctx.set_media_store(Box::new(TestMediaStore {
            image: Some(TestImageResolver {
                id: 'i'.to_string(),
                width: 40,
                height: 40,
                pixels: vec![150, 60, 30, 255]
                    .into_iter()
                    .cycle()
                    .take(40 * 40 * 4)
                    .collect(),
            }),
            video: None,
        }));
        renderer_ctx.clear();

        let mut style = base_style();
        style.clip_radius = [literal(40.0), literal(0.0), literal(0.0), literal(0.0)];

        let clip = ImageClip {
            meta: ClipMeta {
                id: Some('i'.to_string()),
                start_frame: 0,
                end_frame: 10,
            },
            source: 'i'.to_string(),
            style,
        };
        let frame_ctx = FrameContext {
            frame: 0,
            time_seconds: 0.0,
            width: 100,
            height: 100,
            device_scale: 1.0,
        };

        clip.draw(0, &frame_ctx, &mut renderer_ctx)
            .expect("clipped image clip should draw");

        let pixels = read_surface_rgba(&mut renderer_ctx).expect("readback");
        let idx = |x: usize, y: usize| (y * 100 + x) * 4;

        assert_eq!(pixels[idx(10, 10) + 3], 0);
        assert_eq!(&pixels[idx(30, 30)..idx(30, 30) + 4], &[150, 60, 30, 255]);
    }

    #[test]
    fn video_clip_draws_rgba_pixels_from_resolver() {
        let mut renderer_ctx =
            RendererContext::new(100, 100, Rational::new(30, 1)).expect("renderer context");
        renderer_ctx.set_media_store(Box::new(TestMediaStore {
            image: None,
            video: Some(TestVideoResolver {
                width: 2,
                height: 2,
                pixels: vec![
                    10, 20, 30, 255, 40, 50, 60, 255, // row 0
                    70, 80, 90, 255, 100, 110, 120, 255, // row 1
                ],
                last_requested_frame: None,
            }),
        }));
        renderer_ctx.clear();

        let clip = VideoClip {
            meta: ClipMeta {
                id: Some("video".to_owned()),
                start_frame: 0,
                end_frame: 10,
            },
            source: "video".to_owned(),
            style: base_style(),
            trim: None,
            speed: 1.0,
            r#loop: LoopMode::None,
        };
        let frame_ctx = FrameContext {
            frame: 0,
            time_seconds: 0.0,
            width: 100,
            height: 100,
            device_scale: 1.0,
        };

        clip.draw(0, &frame_ctx, &mut renderer_ctx)
            .expect("video clip should draw");

        let pixels = read_surface_rgba(&mut renderer_ctx).expect("readback");
        let idx = |x: usize, y: usize| (y * 100 + x) * 4;

        let tl = &pixels[idx(10, 50)..idx(10, 50) + 4];
        let br = &pixels[idx(11, 51)..idx(11, 51) + 4];

        assert_eq!(tl, &[10, 20, 30, 255]);
        assert_eq!(br, &[100, 110, 120, 255]);
    }

    #[test]
    fn video_clip_errors_when_media_store_missing() {
        let mut renderer_ctx =
            RendererContext::new(100, 100, Rational::new(30, 1)).expect("renderer context");
        renderer_ctx.clear();

        let clip = VideoClip {
            meta: ClipMeta {
                id: Some("video".to_owned()),
                start_frame: 0,
                end_frame: 10,
            },
            source: "video".to_owned(),
            style: base_style(),
            trim: None,
            speed: 1.0,
            r#loop: LoopMode::None,
        };
        let frame_ctx = FrameContext {
            frame: 0,
            time_seconds: 0.0,
            width: 100,
            height: 100,
            device_scale: 1.0,
        };

        let err = clip
            .draw(0, &frame_ctx, &mut renderer_ctx)
            .expect_err("missing media store should error");
        assert!(matches!(err, RenderError::MissingSource(source) if source == "video:video"));
    }

    #[test]
    fn video_clip_errors_when_resolver_missing() {
        let mut renderer_ctx =
            RendererContext::new(100, 100, Rational::new(30, 1)).expect("renderer context");
        renderer_ctx.set_media_store(Box::new(TestMediaStore {
            image: None,
            video: None,
        }));
        renderer_ctx.clear();

        let clip = VideoClip {
            meta: ClipMeta {
                id: Some("video".to_owned()),
                start_frame: 0,
                end_frame: 10,
            },
            source: "video".to_owned(),
            style: base_style(),
            trim: None,
            speed: 1.0,
            r#loop: LoopMode::None,
        };
        let frame_ctx = FrameContext {
            frame: 0,
            time_seconds: 0.0,
            width: 100,
            height: 100,
            device_scale: 1.0,
        };

        let err = clip
            .draw(0, &frame_ctx, &mut renderer_ctx)
            .expect_err("missing resolver should error");
        assert!(matches!(err, RenderError::MissingSource(source) if source == "video:video"));
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
