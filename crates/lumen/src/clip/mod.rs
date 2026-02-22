pub mod group;
pub mod layout;
pub mod media;
pub mod shape;
pub mod style;
pub mod text;

use skia_safe::{Paint, canvas::SaveLayerRec};

use crate::clip::style::{BaseStyle, ResolvedBaseStyle, StyleContext};
use crate::render::backend::RenderError;
use crate::render::context::{FrameContext, RendererContext};

#[derive(Debug, Clone)]
pub struct ClipMeta {
    pub id: Option<String>,
    pub start_frame: u32,
    pub end_frame: u32,
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

        let mut layer = Paint::default();
        layer.set_blend_mode(resolved.blend_mode);
        layer.set_alpha_f((resolved.opacity / (1.0 + resolved.blur * 0.05)).clamp(0.0, 1.0));
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
