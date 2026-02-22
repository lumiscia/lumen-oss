pub mod group;
pub mod layout;
pub mod media;
pub mod shape;
pub mod style;
pub mod text;

use skia_safe::{Paint, Point, RRect, Rect, canvas::SaveLayerRec};

use crate::clip::style::{
    BaseStyle, ResolvedBaseStyle, Sequence, StyleContext, StyleProperty,
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
        let resolved = self.resolve(&StyleContext::new(frame));
        if !resolved.visible {
            return Ok(());
        }

        if let Some(shadow) = resolved.shadow {
            let canvas = renderer_ctx.canvas();
            canvas.save();
            apply_resolved_transform(
                canvas,
                frame_ctx,
                &resolved,
                (shadow.offset_x, shadow.offset_y),
            );
            apply_clip_radius(canvas, frame_ctx, &resolved);

            let mut shadow_layer = Paint::default();
            shadow_layer.set_blend_mode(resolved.blend_mode);
            shadow_layer.set_alpha_f(
                (resolved.opacity * (shadow.color[3] as f32 / 255.0) / (1.0 + shadow.blur * 0.1))
                    .clamp(0.0, 1.0),
            );
            canvas.save_layer(&SaveLayerRec::default().paint(&shadow_layer));
            draw(renderer_ctx, &resolved)?;
            renderer_ctx.canvas().restore();
            renderer_ctx.canvas().restore();
        }

        let canvas = renderer_ctx.canvas();
        canvas.save();
        apply_resolved_transform(canvas, frame_ctx, &resolved, (0.0, 0.0));
        apply_clip_radius(canvas, frame_ctx, &resolved);

        let mut layer = Paint::default();
        layer.set_blend_mode(resolved.blend_mode);
        layer.set_alpha_f(resolved.opacity);
        canvas.save_layer(&SaveLayerRec::default().paint(&layer));
        draw(renderer_ctx, &resolved)?;
        renderer_ctx.canvas().restore();
        renderer_ctx.canvas().restore();

        Ok(())
    }
}

fn apply_resolved_transform(
    canvas: &skia_safe::Canvas,
    frame_ctx: &FrameContext,
    resolved: &ResolvedBaseStyle,
    extra_offset: (f32, f32),
) {
    let frame_width = frame_ctx.width as f32;
    let frame_height = frame_ctx.height as f32;
    let origin_x = frame_width * resolved.origin_x;
    let origin_y = frame_height * resolved.origin_y;
    let translate_x = frame_width * resolved.align_x + resolved.translate_x + extra_offset.0;
    let translate_y = frame_height * resolved.align_y + resolved.translate_y + extra_offset.1;

    canvas.translate((translate_x + origin_x, translate_y + origin_y));
    if resolved.rotation_degrees != 0.0 {
        canvas.rotate(resolved.rotation_degrees, None);
    }
    if resolved.scale_x != 1.0 || resolved.scale_y != 1.0 {
        canvas.scale((resolved.scale_x, resolved.scale_y));
    }
    if resolved.skew_x_degrees != 0.0 || resolved.skew_y_degrees != 0.0 {
        canvas.skew((
            resolved.skew_x_degrees.to_radians().tan(),
            resolved.skew_y_degrees.to_radians().tan(),
        ));
    }
    canvas.translate((-origin_x, -origin_y));
}

fn apply_clip_radius(
    canvas: &skia_safe::Canvas,
    frame_ctx: &FrameContext,
    resolved: &ResolvedBaseStyle,
) {
    if !resolved.clip_radius.iter().any(|radius| *radius > 0.0) {
        return;
    }

    let rect = Rect::from_xywh(0.0, 0.0, frame_ctx.width as f32, frame_ctx.height as f32);
    let max_radius = (frame_ctx.width.min(frame_ctx.height) as f32) * 0.5;
    let radii = [
        Point::new(
            resolved.clip_radius[0].clamp(0.0, max_radius),
            resolved.clip_radius[0].clamp(0.0, max_radius),
        ),
        Point::new(
            resolved.clip_radius[1].clamp(0.0, max_radius),
            resolved.clip_radius[1].clamp(0.0, max_radius),
        ),
        Point::new(
            resolved.clip_radius[2].clamp(0.0, max_radius),
            resolved.clip_radius[2].clamp(0.0, max_radius),
        ),
        Point::new(
            resolved.clip_radius[3].clamp(0.0, max_radius),
            resolved.clip_radius[3].clamp(0.0, max_radius),
        ),
    ];
    let rrect = RRect::new_rect_radii(rect, &radii);
    canvas.clip_rrect(rrect, None, true);
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
