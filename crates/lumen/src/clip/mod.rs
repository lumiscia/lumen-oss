pub mod group;
pub mod layout;
pub mod media;
pub mod shape;
pub mod style;
pub mod text;

use skia_safe::{
    BlurStyle, ClipOp, Color, Data, Image, ImageInfo, MaskFilter, Paint, PathBuilder,
    PathDirection, Point, RRect, Rect, canvas::SaveLayerRec, color_filters, images,
};

use crate::clip::style::{
    BaseStyle, ResolvedBaseStyle, ResolvedMask, ResolvedMaskShape, ResolvedMaskSource, Sequence,
    StyleContext, StyleProperty,
};
use crate::render::backend::RenderError;
use crate::render::context::{FrameContext, RendererContext};

#[derive(Debug, Clone)]
pub struct ClipMeta {
    pub id: Option<String>,
    pub start_frame: u32,
    pub end_frame: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClipGeometry {
    pub x: StyleProperty<f32>,
    pub y: StyleProperty<f32>,
    pub width: StyleProperty<f32>,
    pub height: StyleProperty<f32>,
    pub anchor_x: StyleProperty<f32>,
    pub anchor_y: StyleProperty<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedClipGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub anchor_x: f32,
    pub anchor_y: f32,
}

impl ResolvedClipGeometry {
    pub fn left(&self) -> f32 {
        self.x - self.width * self.anchor_x
    }

    pub fn top(&self) -> f32 {
        self.y - self.height * self.anchor_y
    }

    pub fn rect(&self) -> Rect {
        Rect::from_xywh(self.left(), self.top(), self.width, self.height)
    }

    pub fn rect_for(&self, width: f32, height: f32) -> Rect {
        let width = width.max(1.0);
        let height = height.max(1.0);
        Rect::from_xywh(
            self.x - width * self.anchor_x,
            self.y - height * self.anchor_y,
            width,
            height,
        )
    }
}

impl ClipGeometry {
    pub fn resolve_with_defaults(
        &self,
        frame: u32,
        default_x: f32,
        default_y: f32,
        default_width: f32,
        default_height: f32,
        default_anchor_x: f32,
        default_anchor_y: f32,
    ) -> ResolvedClipGeometry {
        let ctx = StyleContext::new(frame);
        self.resolve_with_context(
            &ctx,
            default_x,
            default_y,
            default_width,
            default_height,
            default_anchor_x,
            default_anchor_y,
        )
    }

    pub fn resolve_with_context(
        &self,
        ctx: &StyleContext<'_>,
        default_x: f32,
        default_y: f32,
        default_width: f32,
        default_height: f32,
        default_anchor_x: f32,
        default_anchor_y: f32,
    ) -> ResolvedClipGeometry {
        ResolvedClipGeometry {
            x: self.x.resolve_or(&ctx, default_x),
            y: self.y.resolve_or(&ctx, default_y),
            width: self.width.resolve_or(&ctx, default_width).max(1.0),
            height: self.height.resolve_or(&ctx, default_height).max(1.0),
            anchor_x: self
                .anchor_x
                .resolve_or(&ctx, default_anchor_x)
                .clamp(0.0, 1.0),
            anchor_y: self
                .anchor_y
                .resolve_or(&ctx, default_anchor_y)
                .clamp(0.0, 1.0),
        }
    }
}

impl Default for ClipGeometry {
    fn default() -> Self {
        let unset = || StyleProperty::Sequence(Sequence::new(Vec::new()));
        Self {
            x: unset(),
            y: unset(),
            width: unset(),
            height: unset(),
            anchor_x: unset(),
            anchor_y: unset(),
        }
    }
}

pub trait Clip {
    fn meta(&self) -> &ClipMeta;

    fn id(&self) -> Option<&str> {
        self.meta().id.as_deref()
    }

    fn start(&self) -> u32 {
        self.meta().start_frame
    }

    fn end(&self) -> u32 {
        self.meta().end_frame
    }

    fn contains_frame(&self, frame: u32) -> bool {
        frame >= self.start() && frame <= self.end()
    }

    fn draw(
        &self,
        frame: u32,
        frame_ctx: &FrameContext,
        renderer_ctx: &mut RendererContext,
    ) -> Result<(), RenderError>;
}

#[derive(Debug)]
pub enum ClipType {
    Group(group::GroupClip),
    Layout(layout::LayoutClip),
    Image(media::ImageClip),
    Video(media::VideoClip),
    Shape(shape::ShapeClip),
    Text(text::TextClip),
}

impl Clip for ClipType {
    fn meta(&self) -> &ClipMeta {
        match self {
            Self::Group(clip) => clip.meta(),
            Self::Layout(clip) => clip.meta(),
            Self::Image(clip) => clip.meta(),
            Self::Video(clip) => clip.meta(),
            Self::Shape(clip) => clip.meta(),
            Self::Text(clip) => clip.meta(),
        }
    }

    fn draw(
        &self,
        frame: u32,
        frame_ctx: &FrameContext,
        renderer_ctx: &mut RendererContext,
    ) -> Result<(), RenderError> {
        match self {
            Self::Group(clip) => clip.draw(frame, frame_ctx, renderer_ctx),
            Self::Layout(clip) => clip.draw(frame, frame_ctx, renderer_ctx),
            Self::Image(clip) => clip.draw(frame, frame_ctx, renderer_ctx),
            Self::Video(clip) => clip.draw(frame, frame_ctx, renderer_ctx),
            Self::Shape(clip) => clip.draw(frame, frame_ctx, renderer_ctx),
            Self::Text(clip) => clip.draw(frame, frame_ctx, renderer_ctx),
        }
        .map_err(|err| err.with_clip_context(self.id(), frame))
    }
}

impl BaseStyle {
    pub(crate) fn draw(
        &self,
        frame: u32,
        frame_ctx: &FrameContext,
        renderer_ctx: &mut RendererContext,
        draw: impl Fn(&mut RendererContext, &ResolvedBaseStyle) -> Result<(), RenderError>,
    ) -> Result<(), RenderError> {
        let expression_scope = renderer_ctx.expression_scope().clone();
        let style_ctx = StyleContext::with_scope(frame, &expression_scope);
        let resolved = self.resolve(&style_ctx);
        if !resolved.visible {
            return Ok(());
        }

        for shadow in resolved.shadows.iter().filter(|shadow| !shadow.inset) {
            let canvas = renderer_ctx.canvas();
            canvas.save();
            resolved.apply_transform(canvas, frame_ctx, (shadow.offset_x, shadow.offset_y));
            resolved.apply_clip_radius(canvas, frame_ctx);
            resolved.apply_shadow_spread(canvas, frame_ctx, shadow.spread);

            let mut shadow_layer = Paint::default();
            shadow_layer.set_blend_mode(resolved.blend_mode);
            shadow_layer.set_alpha_f(resolved.opacity.clamp(0.0, 1.0));
            shadow_layer.set_anti_alias(true);
            shadow_layer.set_color_filter(color_filters::blend(
                Color::from_argb(
                    shadow.color[3],
                    shadow.color[0],
                    shadow.color[1],
                    shadow.color[2],
                ),
                skia_safe::BlendMode::SrcIn,
            ));
            if let Some(mask_filter) = resolved.shadow_mask_filter(shadow) {
                shadow_layer.set_mask_filter(mask_filter);
            }

            canvas.save_layer(&SaveLayerRec::default().paint(&shadow_layer));
            draw(renderer_ctx, &resolved)?;
            renderer_ctx.canvas().restore();
            renderer_ctx.canvas().restore();
        }

        {
            let canvas = renderer_ctx.canvas();
            canvas.save();
            resolved.apply_transform(canvas, frame_ctx, (0.0, 0.0));
            resolved.apply_clip_radius(canvas, frame_ctx);
            if let Some(mask) = resolved.mask.as_ref() {
                resolved.apply_shape_mask(canvas, frame_ctx, mask);
            }

            let mut layer = Paint::default();
            layer.set_blend_mode(resolved.blend_mode);
            layer.set_alpha_f(resolved.opacity);
            canvas.save_layer(&SaveLayerRec::default().paint(&layer));
        }

        draw(renderer_ctx, &resolved)?;
        if let Some(mask) = resolved.mask.as_ref() {
            resolved.apply_alpha_mask(renderer_ctx, frame_ctx, mask)?;
        }
        renderer_ctx.canvas().restore();
        renderer_ctx.canvas().restore();

        for shadow in resolved.shadows.iter().filter(|shadow| shadow.inset) {
            let mut inset_layer = Paint::default();
            inset_layer.set_anti_alias(true);
            inset_layer.set_color_filter(color_filters::blend(
                Color::from_argb(
                    shadow.color[3],
                    shadow.color[0],
                    shadow.color[1],
                    shadow.color[2],
                ),
                skia_safe::BlendMode::SrcIn,
            ));
            if let Some(mask_filter) = resolved.shadow_mask_filter(shadow) {
                inset_layer.set_mask_filter(mask_filter);
            }

            {
                let canvas = renderer_ctx.canvas();
                canvas.save();
                resolved.apply_transform(canvas, frame_ctx, (0.0, 0.0));
                resolved.apply_clip_radius(canvas, frame_ctx);
                canvas.save_layer(&SaveLayerRec::default());
                canvas.save();
                resolved.apply_shadow_spread(canvas, frame_ctx, shadow.spread);
                canvas.translate((shadow.offset_x, shadow.offset_y));
                canvas.save_layer(&SaveLayerRec::default().paint(&inset_layer));
            }

            draw(renderer_ctx, &resolved)?;
            renderer_ctx.canvas().restore();
            renderer_ctx.canvas().restore();

            let mut mask_layer = Paint::default();
            mask_layer.set_blend_mode(skia_safe::BlendMode::DstIn);
            renderer_ctx
                .canvas()
                .save_layer(&SaveLayerRec::default().paint(&mask_layer));
            draw(renderer_ctx, &resolved)?;
            renderer_ctx.canvas().restore();
            renderer_ctx.canvas().restore();
            renderer_ctx.canvas().restore();
        }

        Ok(())
    }
}

impl ResolvedBaseStyle {
    fn apply_transform(
        &self,
        canvas: &skia_safe::Canvas,
        frame_ctx: &FrameContext,
        extra_offset: (f32, f32),
    ) {
        let frame_width = frame_ctx.width as f32;
        let frame_height = frame_ctx.height as f32;
        let origin_x = frame_width * self.origin_x;
        let origin_y = frame_height * self.origin_y;
        let translate_x = frame_width * self.align_x + self.translate_x + extra_offset.0;
        let translate_y = frame_height * self.align_y + self.translate_y + extra_offset.1;

        canvas.translate((translate_x + origin_x, translate_y + origin_y));
        if self.rotation_degrees != 0.0 {
            canvas.rotate(self.rotation_degrees, None);
        }
        if self.scale_x != 1.0 || self.scale_y != 1.0 {
            canvas.scale((self.scale_x, self.scale_y));
        }
        if self.skew_x_degrees != 0.0 || self.skew_y_degrees != 0.0 {
            canvas.skew((
                self.skew_x_degrees.to_radians().tan(),
                self.skew_y_degrees.to_radians().tan(),
            ));
        }
        canvas.translate((-origin_x, -origin_y));
    }

    fn apply_clip_radius(&self, canvas: &skia_safe::Canvas, frame_ctx: &FrameContext) {
        if !self.clip_radius.iter().any(|radius| *radius > 0.0) {
            return;
        }

        let rect = Rect::from_xywh(0.0, 0.0, frame_ctx.width as f32, frame_ctx.height as f32);
        let max_radius = (frame_ctx.width.min(frame_ctx.height) as f32) * 0.5;
        let radii = [
            Point::new(
                self.clip_radius[0].clamp(0.0, max_radius),
                self.clip_radius[0].clamp(0.0, max_radius),
            ),
            Point::new(
                self.clip_radius[1].clamp(0.0, max_radius),
                self.clip_radius[1].clamp(0.0, max_radius),
            ),
            Point::new(
                self.clip_radius[2].clamp(0.0, max_radius),
                self.clip_radius[2].clamp(0.0, max_radius),
            ),
            Point::new(
                self.clip_radius[3].clamp(0.0, max_radius),
                self.clip_radius[3].clamp(0.0, max_radius),
            ),
        ];
        let rrect = RRect::new_rect_radii(rect, &radii);
        canvas.clip_rrect(rrect, None, true);
    }

    fn apply_shape_mask(
        &self,
        canvas: &skia_safe::Canvas,
        frame_ctx: &FrameContext,
        mask: &ResolvedMask,
    ) {
        let ResolvedMaskSource::Shape(shape) = &mask.source else {
            return;
        };

        let mut builder = PathBuilder::new();
        match shape {
            ResolvedMaskShape::Rectangle {
                x,
                y,
                width,
                height,
                corner_radius,
            } => {
                let rect = Rect::from_xywh(*x, *y, width.max(0.0), height.max(0.0));
                if corner_radius.iter().any(|radius| *radius > 0.0) {
                    let max_radius = width.min(*height) * 0.5;
                    let radii = [
                        Point::new(
                            corner_radius[0].clamp(0.0, max_radius),
                            corner_radius[0].clamp(0.0, max_radius),
                        ),
                        Point::new(
                            corner_radius[1].clamp(0.0, max_radius),
                            corner_radius[1].clamp(0.0, max_radius),
                        ),
                        Point::new(
                            corner_radius[2].clamp(0.0, max_radius),
                            corner_radius[2].clamp(0.0, max_radius),
                        ),
                        Point::new(
                            corner_radius[3].clamp(0.0, max_radius),
                            corner_radius[3].clamp(0.0, max_radius),
                        ),
                    ];
                    let rrect = RRect::new_rect_radii(rect, &radii);
                    builder.add_rrect(rrect, Some(PathDirection::CW), Some(0));
                } else {
                    builder.add_rect(rect, Some(PathDirection::CW), Some(0));
                }
            }
            ResolvedMaskShape::Ellipse { cx, cy, rx, ry } => {
                let rect = Rect::from_xywh(*cx - *rx, *cy - *ry, *rx * 2.0, *ry * 2.0);
                builder.add_oval(rect, Some(PathDirection::CW), Some(0));
            }
            ResolvedMaskShape::Path { data } => {
                for command in data {
                    match command {
                        crate::clip::style::PathCommand::MoveTo { x, y } => {
                            builder.move_to((*x, *y));
                        }
                        crate::clip::style::PathCommand::LineTo { x, y } => {
                            builder.line_to((*x, *y));
                        }
                        crate::clip::style::PathCommand::QuadTo { x1, y1, x, y } => {
                            builder.quad_to((*x1, *y1), (*x, *y));
                        }
                        crate::clip::style::PathCommand::CubicTo {
                            x1,
                            y1,
                            x2,
                            y2,
                            x,
                            y,
                        } => {
                            builder.cubic_to((*x1, *y1), (*x2, *y2), (*x, *y));
                        }
                        crate::clip::style::PathCommand::Close => {
                            builder.close();
                        }
                    }
                }
            }
        }

        let path = builder.detach();
        let op = if mask.inverted {
            ClipOp::Difference
        } else {
            ClipOp::Intersect
        };
        canvas.clip_path(&path, Some(op), Some(true));

        if let ResolvedMaskShape::Rectangle { width, height, .. } = shape {
            if *width == 0.0 || *height == 0.0 {
                canvas.clip_rect(Rect::new_empty(), Some(ClipOp::Intersect), Some(false));
            }
        }

        if let ResolvedMaskShape::Ellipse { rx, ry, .. } = shape {
            if *rx == 0.0 || *ry == 0.0 {
                canvas.clip_rect(Rect::new_empty(), Some(ClipOp::Intersect), Some(false));
            }
        }

        let _ = frame_ctx;
    }

    fn apply_alpha_mask(
        &self,
        renderer_ctx: &mut RendererContext,
        frame_ctx: &FrameContext,
        mask: &ResolvedMask,
    ) -> Result<(), RenderError> {
        let source = match &mask.source {
            ResolvedMaskSource::Bitmap { source } => source.as_str(),
            ResolvedMaskSource::Clip { clip_id } => clip_id.as_str(),
            ResolvedMaskSource::Shape(_) => return Ok(()),
        };
        let Some((_, _, image)) = resolve_mask_image(renderer_ctx, source)? else {
            return Err(RenderError::MissingSource(source.to_owned()));
        };

        let mut paint = Paint::default();
        paint.set_anti_alias(false);
        paint.set_blend_mode(if mask.inverted {
            skia_safe::BlendMode::DstOut
        } else {
            skia_safe::BlendMode::DstIn
        });
        renderer_ctx.canvas().draw_image_rect(
            image,
            None,
            Rect::from_xywh(0.0, 0.0, frame_ctx.width as f32, frame_ctx.height as f32),
            &paint,
        );

        Ok(())
    }
    fn apply_shadow_spread(
        &self,
        canvas: &skia_safe::Canvas,
        frame_ctx: &FrameContext,
        spread: f32,
    ) {
        if spread == 0.0 {
            return;
        }

        let width = frame_ctx.width.max(1) as f32;
        let height = frame_ctx.height.max(1) as f32;
        let scale_x = ((width + spread * 2.0) / width).max(0.01);
        let scale_y = ((height + spread * 2.0) / height).max(0.01);
        let center_x = width * 0.5;
        let center_y = height * 0.5;

        canvas.translate((center_x, center_y));
        canvas.scale((scale_x, scale_y));
        canvas.translate((-center_x, -center_y));
    }

    fn shadow_mask_filter(
        &self,
        shadow: &crate::clip::style::ResolvedShadowStyle,
    ) -> Option<MaskFilter> {
        let sigma = ((shadow.blur + shadow.spread.abs() * 2.0) / 2.0).max(0.0);
        if sigma == 0.0 {
            return None;
        }

        MaskFilter::blur(BlurStyle::Normal, sigma, Some(false))
    }
}

fn resolve_mask_image(
    renderer_ctx: &mut RendererContext,
    source: &str,
) -> Result<Option<(u32, u32, Image)>, RenderError> {
    if let Some((width, height, image)) = renderer_ctx.cached_image_by_source(source) {
        return Ok(Some((width, height, image)));
    }

    let fetched = if let Some(media_store) = renderer_ctx.media_store_mut() {
        if let Some(mut resolver) = media_store.get_image_resolver(source) {
            let width = resolver.width().max(1);
            let height = resolver.height().max(1);
            let pixels = resolver.resolve();
            Some((width, height, pixels))
        } else {
            None
        }
    } else {
        None
    };

    if let Some((width, height, pixels)) = fetched {
        let image = raster_image_from_rgba(width, height, pixels.as_slice())?;
        renderer_ctx.cache_image(source.to_owned(), width, height, image.clone());
        return Ok(Some((width, height, image)));
    }

    Ok(None)
}

fn raster_image_from_rgba(width: u32, height: u32, pixels: &[u8]) -> Result<Image, RenderError> {
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
    images::raster_from_data(&info, data, width as usize * 4).ok_or(RenderError::Unsupported(
        "failed to create image from RGBA pixels",
    ))
}

#[cfg(test)]
mod tests {
    use skia_safe::Rect;

    use super::ClipGeometry;
    use crate::clip::style::{StyleProperty, StyleValue};

    fn literal(value: f32) -> StyleProperty<f32> {
        StyleProperty::Value(StyleValue::Literal(value))
    }

    #[test]
    fn geometry_resolves_anchor_position_into_top_left_rect() {
        let geometry = ClipGeometry {
            x: literal(60.0),
            y: literal(40.0),
            width: literal(20.0),
            height: literal(10.0),
            anchor_x: literal(0.5),
            anchor_y: literal(1.0),
        };

        let resolved = geometry.resolve_with_defaults(0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0);

        assert_eq!(resolved.left(), 50.0);
        assert_eq!(resolved.top(), 30.0);
        assert_eq!(resolved.rect(), Rect::from_xywh(50.0, 30.0, 20.0, 10.0));
    }

    #[test]
    fn geometry_uses_fallbacks_and_clamps_anchor() {
        let geometry = ClipGeometry {
            anchor_x: literal(-1.0),
            anchor_y: literal(2.0),
            ..ClipGeometry::default()
        };

        let resolved = geometry.resolve_with_defaults(0, 10.0, 20.0, 30.0, 40.0, 0.25, 0.75);

        assert_eq!(resolved.x, 10.0);
        assert_eq!(resolved.y, 20.0);
        assert_eq!(resolved.width, 30.0);
        assert_eq!(resolved.height, 40.0);
        assert_eq!(resolved.anchor_x, 0.0);
        assert_eq!(resolved.anchor_y, 1.0);
    }
}
