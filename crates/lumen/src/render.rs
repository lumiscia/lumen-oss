use std::{collections::HashMap, sync::Arc};

use skia_safe::{
    BlendMode, Color, EncodedImageFormat, Font, Paint, RRect, Rect, Surface,
    canvas::SaveLayerRec,
    image::{CachingHint, Image},
    surfaces,
    svg::Dom,
    utils::text_utils::Align,
};
use thiserror::Error;

use crate::{
    font::{FONT_ARIAL, FontManager},
    media::{MediaError, MediaProvider, NoopMediaProvider},
    plan::{
        RenderOp, RenderOpKind, RenderPlan, ScalarFrameKeyframe, ShapeRenderOp, SolidRenderOp,
        TextRenderOp,
    },
    sequence::{
        BlendMode as SequenceBlendMode, ShapeContent, TextAlign, Transform, TransitionKind,
    },
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
    #[error("failed to decode svg asset `{asset_id}`")]
    SvgDecode { asset_id: String },
}

const SVG_RASTER_CACHE_CAPACITY: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SvgRasterCacheKey {
    asset_id: String,
    width: i32,
    height: i32,
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
    svg_dom_cache: HashMap<String, Dom>,
    svg_raster_cache: HashMap<SvgRasterCacheKey, Image>,
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
        let surface =
            surfaces::raster_n32_premul((width as i32, height as i32)).ok_or_else(|| {
                RendererError::SkiaError("failed to create raster surface".to_string())
            })?;

        Ok(Self {
            image: skia_safe::ImageInfo::new_n32_premul((width as i32, height as i32), None),
            media: Box::new(media_provider),
            svg_dom_cache: HashMap::new(),
            svg_raster_cache: HashMap::new(),
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
        let (transform, opacity) = self.resolve_op_state(frame, op);
        match &op.kind {
            RenderOpKind::Text(text) => self.draw_text(op.blend_mode, transform, opacity, text),
            RenderOpKind::Shape(shape) => {
                self.draw_shape(op.blend_mode, transform, opacity, shape);
                Ok(())
            }
            RenderOpKind::Solid(solid) => {
                self.draw_solid(op.blend_mode, transform, opacity, solid);
                Ok(())
            }
            RenderOpKind::Image(asset) => {
                if let Some(image) = self.media.image(&asset.asset_id)? {
                    self.draw_image(op.blend_mode, transform, opacity, &image);
                }
                Ok(())
            }
            RenderOpKind::Svg(asset) => {
                self.draw_svg(op.blend_mode, transform, opacity, &asset.asset_id)
            }
            RenderOpKind::Video(asset) => {
                let local_frame = frame.0.saturating_sub(op.start_frame.0);
                let mut source_offset =
                    ((local_frame as f64) * (asset.speed as f64)).floor() as u64;
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
                    self.draw_image(op.blend_mode, transform, opacity, &image);
                }
                Ok(())
            }
        }
    }

    fn draw_text(
        &mut self,
        blend_mode: SequenceBlendMode,
        transform: Transform,
        opacity: f32,
        text: &TextRenderOp,
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
            (transform.x, transform.y),
            &font,
            &paint,
            align,
        );

        Ok(())
    }

    fn draw_shape(
        &mut self,
        blend_mode: SequenceBlendMode,
        transform: Transform,
        opacity: f32,
        shape: &ShapeRenderOp,
    ) {
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_blend_mode(to_blend_mode(blend_mode));

        match &shape.shape {
            ShapeContent::Rectangle { fill, radius } => {
                let mut color = fill.as_color4f();
                color.a *= opacity.clamp(0.0, 1.0);
                paint.set_color4f(color, None);
                let rect = op_rect(
                    transform,
                    self.context.width as f32,
                    self.context.height as f32,
                );
                if *radius > 0.0 {
                    let rrect = RRect::new_rect_xy(rect, *radius, *radius);
                    self.context.surface.canvas().draw_rrect(rrect, &paint);
                    return;
                }
                self.context.surface.canvas().draw_rect(rect, &paint);
            }
            ShapeContent::Ellipse { fill } => {
                let mut color = fill.as_color4f();
                color.a *= opacity.clamp(0.0, 1.0);
                paint.set_color4f(color, None);
                self.context.surface.canvas().draw_oval(
                    op_rect(
                        transform,
                        self.context.width as f32,
                        self.context.height as f32,
                    ),
                    &paint,
                );
            }
        }
    }

    fn draw_solid(
        &mut self,
        blend_mode: SequenceBlendMode,
        transform: Transform,
        opacity: f32,
        solid: &SolidRenderOp,
    ) {
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_blend_mode(to_blend_mode(blend_mode));

        let mut color = solid.color.as_color4f();
        color.a *= opacity.clamp(0.0, 1.0);
        paint.set_color4f(color, None);

        self.context.surface.canvas().draw_rect(
            op_rect(
                transform,
                self.context.width as f32,
                self.context.height as f32,
            ),
            &paint,
        );
    }

    fn draw_image(
        &mut self,
        blend_mode: SequenceBlendMode,
        transform: Transform,
        opacity: f32,
        image: &Image,
    ) {
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_blend_mode(to_blend_mode(blend_mode));
        paint.set_alpha_f(opacity.clamp(0.0, 1.0));

        let src = Rect::from_xywh(0.0, 0.0, image.width() as f32, image.height() as f32);
        let dst = op_rect(transform, image.width() as f32, image.height() as f32);

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

    fn draw_svg(
        &mut self,
        blend_mode: SequenceBlendMode,
        transform: Transform,
        opacity: f32,
        asset_id: &str,
    ) -> Result<(), RendererError> {
        let opacity = opacity.clamp(0.0, 1.0);
        if opacity <= 0.0 {
            return Ok(());
        }

        let dst = op_rect(
            transform,
            self.context.width as f32,
            self.context.height as f32,
        );
        if dst.width() <= 0.0 || dst.height() <= 0.0 {
            return Ok(());
        }

        let width = dst.width().ceil().max(1.0) as i32;
        let height = dst.height().ceil().max(1.0) as i32;
        let key = SvgRasterCacheKey {
            asset_id: asset_id.to_string(),
            width,
            height,
        };

        let image = if let Some(image) = self.svg_raster_cache.get(&key) {
            image.clone()
        } else {
            let Some(bytes) = self.media.svg_bytes(asset_id)? else {
                return Ok(());
            };
            let image = self.rasterize_svg(asset_id, &bytes, width, height)?;
            self.insert_svg_raster_cache(key, image.clone());
            image
        };

        self.draw_image(blend_mode, transform, opacity, &image);
        Ok(())
    }

    fn insert_svg_raster_cache(&mut self, key: SvgRasterCacheKey, image: Image) {
        if self.svg_raster_cache.len() >= SVG_RASTER_CACHE_CAPACITY {
            if let Some(evicted) = self.svg_raster_cache.keys().next().cloned() {
                self.svg_raster_cache.remove(&evicted);
            }
        }

        self.svg_raster_cache.insert(key, image);
    }

    fn rasterize_svg(
        &mut self,
        asset_id: &str,
        bytes: &[u8],
        width: i32,
        height: i32,
    ) -> Result<Image, RendererError> {
        let mut svg = self.svg_dom_for_asset(asset_id, bytes)?;
        svg.set_container_size((width as f32, height as f32));

        let mut surface = surfaces::raster_n32_premul((width, height))
            .ok_or_else(|| RendererError::SkiaError("failed to create svg raster".to_string()))?;
        let canvas = surface.canvas();
        canvas.clear(Color::from_argb(0, 0, 0, 0));
        canvas.save();
        canvas.clip_rect(
            Rect::from_xywh(0.0, 0.0, width as f32, height as f32),
            None,
            Some(true),
        );
        canvas.save_layer(&SaveLayerRec::default());
        svg.render(canvas);
        canvas.restore();
        canvas.restore();

        Ok(surface.image_snapshot())
    }

    fn svg_dom_for_asset(&mut self, asset_id: &str, bytes: &[u8]) -> Result<Dom, RendererError> {
        if !self.svg_dom_cache.contains_key(asset_id) {
            let dom =
                Dom::from_bytes(bytes, self.context.font_manager.skia().clone()).map_err(|_| {
                    RendererError::SvgDecode {
                        asset_id: asset_id.to_string(),
                    }
                })?;
            self.svg_dom_cache.insert(asset_id.to_string(), dom);
        }

        self.svg_dom_cache
            .get(asset_id)
            .cloned()
            .ok_or_else(|| RendererError::SvgDecode {
                asset_id: asset_id.to_string(),
            })
    }

    fn resolve_op_state(&self, frame: FrameIndex, op: &RenderOp) -> (Transform, f32) {
        let local_frame = frame.0.saturating_sub(op.start_frame.0);
        let mut transform = op.transform;

        transform.x = sample_keyframes(op.transform.x, &op.animation.x, local_frame);
        transform.y = sample_keyframes(op.transform.y, &op.animation.y, local_frame);
        transform.width =
            sample_optional_keyframes(op.transform.width, &op.animation.width, local_frame);
        transform.height =
            sample_optional_keyframes(op.transform.height, &op.animation.height, local_frame);

        let mut opacity = sample_keyframes(
            op.opacity.clamp(0.0, 1.0),
            &op.animation.opacity,
            local_frame,
        );

        if let Some(transition) = op.transition_in {
            apply_transition(
                &mut transform,
                &mut opacity,
                transition.kind,
                local_frame,
                transition.duration_frames,
                self.context.width as f32,
                self.context.height as f32,
                true,
            );
        }

        if let Some(transition) = op.transition_out {
            let remaining = op.end_frame.0.saturating_sub(frame.0);
            if remaining <= transition.duration_frames {
                apply_transition(
                    &mut transform,
                    &mut opacity,
                    transition.kind,
                    remaining,
                    transition.duration_frames,
                    self.context.width as f32,
                    self.context.height as f32,
                    false,
                );
            }
        }

        (transform, opacity.clamp(0.0, 1.0))
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

fn op_rect(transform: Transform, default_width: f32, default_height: f32) -> Rect {
    Rect::from_xywh(
        transform.x,
        transform.y,
        transform.width.unwrap_or(default_width),
        transform.height.unwrap_or(default_height),
    )
}

fn sample_keyframes(base: f32, keyframes: &[ScalarFrameKeyframe], frame: u64) -> f32 {
    if keyframes.is_empty() {
        return base;
    }

    let mut previous_frame = 0u64;
    let mut previous_value = base;

    for keyframe in keyframes {
        if frame < keyframe.frame_offset {
            if keyframe.frame_offset == previous_frame {
                return keyframe.value;
            }

            let t = (frame.saturating_sub(previous_frame)) as f32
                / (keyframe.frame_offset.saturating_sub(previous_frame)) as f32;
            let eased = apply_easing(t, keyframe.easing);
            return lerp(previous_value, keyframe.value, eased);
        }

        previous_frame = keyframe.frame_offset;
        previous_value = keyframe.value;
    }

    previous_value
}

fn sample_optional_keyframes(
    base: Option<f32>,
    keyframes: &[ScalarFrameKeyframe],
    frame: u64,
) -> Option<f32> {
    if keyframes.is_empty() {
        return base;
    }

    let start = base.unwrap_or(keyframes[0].value);
    Some(sample_keyframes(start, keyframes, frame))
}

fn apply_transition(
    transform: &mut Transform,
    opacity: &mut f32,
    kind: TransitionKind,
    position: u64,
    duration: u64,
    canvas_width: f32,
    canvas_height: f32,
    entering: bool,
) {
    if duration == 0 {
        return;
    }

    let progress = (position as f32 / duration as f32).clamp(0.0, 1.0);
    match kind {
        TransitionKind::Fade | TransitionKind::Dissolve => {
            *opacity *= progress;
        }
        TransitionKind::SlideLeft => {
            let offset = (1.0 - progress) * canvas_width;
            if entering {
                transform.x += offset;
            } else {
                transform.x -= offset;
            }
        }
        TransitionKind::SlideRight => {
            let offset = (1.0 - progress) * canvas_width;
            if entering {
                transform.x -= offset;
            } else {
                transform.x += offset;
            }
        }
        TransitionKind::SlideUp => {
            let offset = (1.0 - progress) * canvas_height;
            if entering {
                transform.y += offset;
            } else {
                transform.y -= offset;
            }
        }
        TransitionKind::SlideDown => {
            let offset = (1.0 - progress) * canvas_height;
            if entering {
                transform.y -= offset;
            } else {
                transform.y += offset;
            }
        }
    }
}

fn apply_easing(t: f32, easing: crate::sequence::KeyframeEasing) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match easing {
        crate::sequence::KeyframeEasing::Linear => t,
        crate::sequence::KeyframeEasing::EaseIn => t * t,
        crate::sequence::KeyframeEasing::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
        crate::sequence::KeyframeEasing::EaseInOut => {
            if t < 0.5 {
                2.0 * t * t
            } else {
                1.0 - ((-2.0 * t + 2.0).powi(2) / 2.0)
            }
        }
    }
}

fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::Renderer;
    use crate::{
        font::FontManager,
        media::MediaProvider,
        plan::{
            AssetRenderOp, CanvasSpec, ClipAnimationPlan, RenderOp, RenderOpKind, RenderPlan,
            ScalarFrameKeyframe, TransitionSpec, VideoRenderOp,
        },
        sequence::{BlendMode, ColorRGBA, KeyframeEasing, Transform, TransitionKind},
        skia::{AlphaType, ColorType, Data, FontMgr, Image, ImageInfo, Typeface, images},
        time::{FrameIndex, Rational, Time},
    };

    struct TestFontManager(FontMgr);

    impl TestFontManager {
        fn new() -> Self {
            Self(FontMgr::new())
        }
    }

    impl FontManager for TestFontManager {
        fn skia(&self) -> &FontMgr {
            &self.0
        }

        fn named(&self, _name: &str) -> Option<Typeface> {
            None
        }
    }

    struct TestMedia {
        image: Image,
        svg: Vec<u8>,
    }

    impl MediaProvider for TestMedia {
        fn image(&mut self, _asset_id: &str) -> Result<Option<Image>, crate::media::MediaError> {
            Ok(Some(self.image.clone()))
        }

        fn video_frame(
            &mut self,
            _asset_id: &str,
            _frame: FrameIndex,
        ) -> Result<Option<Image>, crate::media::MediaError> {
            Ok(Some(self.image.clone()))
        }

        fn svg_bytes(
            &mut self,
            _asset_id: &str,
        ) -> Result<Option<Vec<u8>>, crate::media::MediaError> {
            Ok(Some(self.svg.clone()))
        }
    }

    struct CountingSvgMedia {
        svg: Vec<u8>,
        requests: Arc<AtomicUsize>,
    }

    impl MediaProvider for CountingSvgMedia {
        fn svg_bytes(
            &mut self,
            _asset_id: &str,
        ) -> Result<Option<Vec<u8>>, crate::media::MediaError> {
            self.requests.fetch_add(1, Ordering::Relaxed);
            Ok(Some(self.svg.clone()))
        }
    }

    #[test]
    fn image_clip_opacity_is_applied() {
        let mut renderer = Renderer::new(
            Arc::new(test_plan(RenderOpKind::Image(AssetRenderOp {
                asset_id: "asset".to_string(),
            }))),
            TestFontManager::new(),
            TestMedia {
                image: white_image(),
                svg: white_svg(),
            },
        )
        .expect("renderer");
        renderer.draw_frame(FrameIndex(0)).expect("draw");

        let mut rgba = vec![0u8; 4];
        renderer.read_rgba(&mut rgba).expect("read");
        assert_eq!(rgba, vec![0, 0, 0, 255]);
    }

    #[test]
    fn video_clip_opacity_is_applied() {
        let mut renderer = Renderer::new(
            Arc::new(test_plan(RenderOpKind::Video(VideoRenderOp {
                asset_id: "asset".to_string(),
                speed: 1.0,
                reverse: false,
                source_span_frames: 1,
            }))),
            TestFontManager::new(),
            TestMedia {
                image: white_image(),
                svg: white_svg(),
            },
        )
        .expect("renderer");
        renderer.draw_frame(FrameIndex(0)).expect("draw");

        let mut rgba = vec![0u8; 4];
        renderer.read_rgba(&mut rgba).expect("read");
        assert_eq!(rgba, vec![0, 0, 0, 255]);
    }

    #[test]
    fn svg_clip_opacity_is_applied() {
        let mut renderer = Renderer::new(
            Arc::new(test_plan(RenderOpKind::Svg(AssetRenderOp {
                asset_id: "asset".to_string(),
            }))),
            TestFontManager::new(),
            TestMedia {
                image: white_image(),
                svg: white_svg(),
            },
        )
        .expect("renderer");
        renderer.draw_frame(FrameIndex(0)).expect("draw");

        let mut rgba = vec![0u8; 4];
        renderer.read_rgba(&mut rgba).expect("read");
        assert_eq!(rgba, vec![0, 0, 0, 255]);
    }

    #[test]
    fn svg_raster_cache_reuses_asset_bytes_for_same_size() {
        let requests = Arc::new(AtomicUsize::new(0));
        let mut renderer = Renderer::new(
            Arc::new(svg_cache_test_plan()),
            TestFontManager::new(),
            CountingSvgMedia {
                svg: white_svg(),
                requests: Arc::clone(&requests),
            },
        )
        .expect("renderer");

        renderer.draw_frame(FrameIndex(0)).expect("draw");
        renderer.draw_frame(FrameIndex(1)).expect("draw");

        assert_eq!(requests.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn keyframed_opacity_changes_over_time() {
        let plan = RenderPlan::with_operations_index(
            CanvasSpec {
                width: 1,
                height: 1,
                background: ColorRGBA(0, 0, 0, 255),
            },
            Rational { num: 30, den: 1 },
            Time {
                value: 2,
                timescale: 30,
            },
            2,
            vec![RenderOp {
                id: "clip".to_string(),
                start_frame: FrameIndex(0),
                end_frame: FrameIndex(2),
                source_in_frame: FrameIndex(0),
                z_index: 0,
                clip_index: 0,
                opacity: 1.0,
                blend_mode: BlendMode::Normal,
                transform: Transform {
                    x: 0.0,
                    y: 0.0,
                    width: Some(1.0),
                    height: Some(1.0),
                },
                animation: ClipAnimationPlan {
                    opacity: vec![
                        ScalarFrameKeyframe {
                            frame_offset: 0,
                            value: 0.0,
                            easing: KeyframeEasing::Linear,
                        },
                        ScalarFrameKeyframe {
                            frame_offset: 1,
                            value: 1.0,
                            easing: KeyframeEasing::Linear,
                        },
                    ],
                    ..ClipAnimationPlan::default()
                },
                transition_in: None,
                transition_out: None,
                kind: RenderOpKind::Image(AssetRenderOp {
                    asset_id: "asset".to_string(),
                }),
            }],
        );

        let mut renderer = Renderer::new(
            Arc::new(plan),
            TestFontManager::new(),
            TestMedia {
                image: white_image(),
                svg: white_svg(),
            },
        )
        .expect("renderer");

        let mut rgba = vec![0u8; 4];
        renderer.draw_frame(FrameIndex(0)).expect("draw");
        renderer.read_rgba(&mut rgba).expect("read");
        assert_eq!(rgba, vec![0, 0, 0, 255]);

        renderer.draw_frame(FrameIndex(1)).expect("draw");
        renderer.read_rgba(&mut rgba).expect("read");
        assert_eq!(rgba, vec![255, 255, 255, 255]);
    }

    #[test]
    fn fade_in_transition_starts_transparent() {
        let mut plan = test_plan(RenderOpKind::Image(AssetRenderOp {
            asset_id: "asset".to_string(),
        }));
        plan.operations[0].opacity = 1.0;
        plan.operations[0].transition_in = Some(TransitionSpec {
            kind: TransitionKind::Fade,
            duration_frames: 1,
        });

        let mut renderer = Renderer::new(
            Arc::new(plan),
            TestFontManager::new(),
            TestMedia {
                image: white_image(),
                svg: white_svg(),
            },
        )
        .expect("renderer");
        renderer.draw_frame(FrameIndex(0)).expect("draw");

        let mut rgba = vec![0u8; 4];
        renderer.read_rgba(&mut rgba).expect("read");
        assert_eq!(rgba, vec![0, 0, 0, 255]);
    }

    fn white_image() -> Image {
        let image_info = ImageInfo::new((1, 1), ColorType::RGBA8888, AlphaType::Premul, None);
        let data = Data::new_copy(&[255, 255, 255, 255]);
        images::raster_from_data(&image_info, data, 4).expect("image")
    }

    fn white_svg() -> Vec<u8> {
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1" viewBox="0 0 1 1"><rect width="1" height="1" fill="#ffffff"/></svg>"##
            .to_vec()
    }

    fn test_plan(kind: RenderOpKind) -> RenderPlan {
        RenderPlan::with_operations_index(
            CanvasSpec {
                width: 1,
                height: 1,
                background: ColorRGBA(0, 0, 0, 255),
            },
            Rational { num: 30, den: 1 },
            Time {
                value: 1,
                timescale: 30,
            },
            1,
            vec![RenderOp {
                id: "clip".to_string(),
                start_frame: FrameIndex(0),
                end_frame: FrameIndex(1),
                source_in_frame: FrameIndex(0),
                z_index: 0,
                clip_index: 0,
                opacity: 0.0,
                blend_mode: BlendMode::Normal,
                transform: Transform {
                    x: 0.0,
                    y: 0.0,
                    width: Some(1.0),
                    height: Some(1.0),
                },
                animation: ClipAnimationPlan::default(),
                transition_in: None,
                transition_out: None,
                kind,
            }],
        )
    }

    fn svg_cache_test_plan() -> RenderPlan {
        RenderPlan::with_operations_index(
            CanvasSpec {
                width: 1,
                height: 1,
                background: ColorRGBA(0, 0, 0, 255),
            },
            Rational { num: 30, den: 1 },
            Time {
                value: 2,
                timescale: 30,
            },
            2,
            vec![RenderOp {
                id: "svg".to_string(),
                start_frame: FrameIndex(0),
                end_frame: FrameIndex(2),
                source_in_frame: FrameIndex(0),
                z_index: 0,
                clip_index: 0,
                opacity: 1.0,
                blend_mode: BlendMode::Normal,
                transform: Transform {
                    x: 0.0,
                    y: 0.0,
                    width: Some(1.0),
                    height: Some(1.0),
                },
                animation: ClipAnimationPlan::default(),
                transition_in: None,
                transition_out: None,
                kind: RenderOpKind::Svg(AssetRenderOp {
                    asset_id: "asset".to_string(),
                }),
            }],
        )
    }
}
