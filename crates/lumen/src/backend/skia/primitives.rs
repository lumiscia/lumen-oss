use skia_safe::canvas::SrcRectConstraint;
use skia_safe::{
    AlphaType, BlendMode as SkiaBlendMode, Canvas, Color4f, ColorType, Data, Font, ImageInfo,
    Paint, PaintStyle, PathBuilder, Rect,
};

use crate::backend::{FrameImage, RenderError};
use crate::compile::{CompiledOperation, CompiledOperationKind, RuntimeFrameContext};
use crate::model::{BlendMode, FitMode, ShapeGeometry, StyleValue};

pub fn to_skia_blend_mode(mode: BlendMode) -> SkiaBlendMode {
    match mode {
        BlendMode::Normal => SkiaBlendMode::SrcOver,
        BlendMode::Multiply => SkiaBlendMode::Multiply,
        BlendMode::Screen => SkiaBlendMode::Screen,
        BlendMode::Overlay => SkiaBlendMode::Overlay,
        BlendMode::Darken => SkiaBlendMode::Darken,
        BlendMode::Lighten => SkiaBlendMode::Lighten,
        BlendMode::ColorDodge => SkiaBlendMode::ColorDodge,
        BlendMode::ColorBurn => SkiaBlendMode::ColorBurn,
        BlendMode::HardLight => SkiaBlendMode::HardLight,
        BlendMode::SoftLight => SkiaBlendMode::SoftLight,
        BlendMode::Difference => SkiaBlendMode::Difference,
        BlendMode::Exclusion => SkiaBlendMode::Exclusion,
        BlendMode::Hue => SkiaBlendMode::Hue,
        BlendMode::Saturation => SkiaBlendMode::Saturation,
        BlendMode::Color => SkiaBlendMode::Color,
        BlendMode::Luminosity => SkiaBlendMode::Luminosity,
    }
}

pub fn to_color4f(color: [u8; 4], opacity: f32) -> Color4f {
    let alpha = (color[3] as f32 / 255.0) * opacity.clamp(0.0, 1.0);
    Color4f::new(
        color[0] as f32 / 255.0,
        color[1] as f32 / 255.0,
        color[2] as f32 / 255.0,
        alpha,
    )
}

pub fn draw_operation_content(
    canvas: &Canvas,
    operation: &CompiledOperation,
    _frame_state: &RuntimeFrameContext,
    frame_image: Option<&FrameImage>,
    bounds: Rect,
    opacity: f32,
) -> Result<(), RenderError> {
    match &operation.kind {
        CompiledOperationKind::Solid => {
            let fill = operation.style.fill.unwrap_or([0, 0, 0, 255]);
            draw_solid(
                canvas,
                bounds,
                fill,
                opacity,
                operation.style.base.blend_mode,
            )
        }
        CompiledOperationKind::Shape(shape) => {
            draw_shape(canvas, &shape.geometry, operation, bounds, opacity)
        }
        CompiledOperationKind::Text(text) => {
            draw_text(canvas, text.content.as_str(), operation, bounds, opacity)
        }
        CompiledOperationKind::Image(_) | CompiledOperationKind::Video(_) => {
            if let Some(image) = frame_image {
                draw_image(
                    canvas,
                    bounds,
                    image,
                    operation.style.fit,
                    opacity,
                    operation.style.base.blend_mode,
                )?;
            }
            Ok(())
        }
        CompiledOperationKind::Layout(_) => Ok(()),
    }
}

fn draw_solid(
    canvas: &Canvas,
    bounds: Rect,
    fill: [u8; 4],
    opacity: f32,
    blend_mode: BlendMode,
) -> Result<(), RenderError> {
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_color4f(to_color4f(fill, opacity), None);
    paint.set_style(PaintStyle::Fill);
    paint.set_blend_mode(to_skia_blend_mode(blend_mode));
    canvas.draw_rect(bounds, &paint);
    Ok(())
}

fn draw_shape(
    canvas: &Canvas,
    geometry: &ShapeGeometry,
    operation: &CompiledOperation,
    bounds: Rect,
    opacity: f32,
) -> Result<(), RenderError> {
    let fill = operation.style.fill.unwrap_or([255, 255, 255, 255]);

    let mut fill_paint = Paint::default();
    fill_paint.set_anti_alias(true);
    fill_paint.set_style(PaintStyle::Fill);
    fill_paint.set_color4f(to_color4f(fill, opacity), None);
    fill_paint.set_blend_mode(to_skia_blend_mode(operation.style.base.blend_mode));

    match geometry {
        ShapeGeometry::Rect => {
            canvas.draw_rect(bounds, &fill_paint);
        }
        ShapeGeometry::Ellipse => {
            canvas.draw_oval(bounds, &fill_paint);
        }
        ShapeGeometry::Polygon { vertices, closed } => {
            let mut builder = PathBuilder::new();
            if let Some(first) = vertices.first() {
                builder.move_to((
                    bounds.left + style_literal(&first.x),
                    bounds.top + style_literal(&first.y),
                ));
                for vertex in vertices.iter().skip(1) {
                    builder.line_to((
                        bounds.left + style_literal(&vertex.x),
                        bounds.top + style_literal(&vertex.y),
                    ));
                }
                if *closed {
                    builder.close();
                }
            }
            let path = builder.detach();
            canvas.draw_path(&path, &fill_paint);
        }
    }

    if let Some(stroke) = &operation.style.stroke {
        let mut stroke_paint = Paint::default();
        stroke_paint.set_anti_alias(true);
        stroke_paint.set_style(PaintStyle::Stroke);
        stroke_paint.set_stroke_width(stroke.width.fallback().max(0.0));
        stroke_paint.set_color4f(to_color4f(stroke.color, opacity), None);
        stroke_paint.set_blend_mode(to_skia_blend_mode(operation.style.base.blend_mode));

        match geometry {
            ShapeGeometry::Rect => {
                canvas.draw_rect(bounds, &stroke_paint);
            }
            ShapeGeometry::Ellipse => {
                canvas.draw_oval(bounds, &stroke_paint);
            }
            ShapeGeometry::Polygon { vertices, closed } => {
                let mut builder = PathBuilder::new();
                if let Some(first) = vertices.first() {
                    builder.move_to((
                        bounds.left + style_literal(&first.x),
                        bounds.top + style_literal(&first.y),
                    ));
                    for vertex in vertices.iter().skip(1) {
                        builder.line_to((
                            bounds.left + style_literal(&vertex.x),
                            bounds.top + style_literal(&vertex.y),
                        ));
                    }
                    if *closed {
                        builder.close();
                    }
                }
                let path = builder.detach();
                canvas.draw_path(&path, &stroke_paint);
            }
        }
    }

    Ok(())
}

fn draw_text(
    canvas: &Canvas,
    content: &str,
    operation: &CompiledOperation,
    bounds: Rect,
    opacity: f32,
) -> Result<(), RenderError> {
    if content.is_empty() {
        return Ok(());
    }

    let mut font = Font::default();
    font.set_size(operation.style.font_size.fallback().max(1.0));

    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_blend_mode(to_skia_blend_mode(operation.style.base.blend_mode));
    paint.set_color4f(
        to_color4f(
            operation.style.color.unwrap_or([255, 255, 255, 255]),
            opacity,
        ),
        None,
    );

    let text_width = font.measure_str(content, Some(&paint)).0;
    let x = match operation.style.align {
        crate::model::TextAlign::Left => bounds.left,
        crate::model::TextAlign::Center => bounds.left + (bounds.width() - text_width) * 0.5,
        crate::model::TextAlign::Right => bounds.right - text_width,
    };

    let baseline = match operation.style.vertical_align {
        crate::model::VerticalAlign::Top => bounds.top + font.size(),
        crate::model::VerticalAlign::Middle => bounds.top + bounds.height() * 0.5,
        crate::model::VerticalAlign::Bottom => bounds.bottom,
    };

    canvas.draw_str(content, (x, baseline), &font, &paint);
    Ok(())
}

fn draw_image(
    canvas: &Canvas,
    bounds: Rect,
    frame_image: &FrameImage,
    fit: FitMode,
    opacity: f32,
    blend_mode: BlendMode,
) -> Result<(), RenderError> {
    let info = ImageInfo::new(
        (frame_image.width as i32, frame_image.height as i32),
        ColorType::RGBA8888,
        AlphaType::Unpremul,
        None,
    );
    let row_bytes = frame_image.width as usize * 4;
    let data = Data::new_copy(frame_image.rgba.as_slice());
    let image = skia_safe::images::raster_from_data(&info, data, row_bytes)
        .ok_or_else(|| RenderError::Failed("failed to create image from rgba".to_string()))?;

    let src = Rect::from_xywh(
        0.0,
        0.0,
        frame_image.width as f32,
        frame_image.height as f32,
    );
    let dst = fit_rect(src, bounds, fit);

    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_alpha_f(opacity.clamp(0.0, 1.0));
    paint.set_blend_mode(to_skia_blend_mode(blend_mode));

    canvas.draw_image_rect(image, Some((&src, SrcRectConstraint::Strict)), &dst, &paint);
    Ok(())
}

fn fit_rect(src: Rect, dst: Rect, fit: FitMode) -> Rect {
    match fit {
        FitMode::Fill => dst,
        FitMode::None => Rect::from_xywh(dst.left, dst.top, src.width(), src.height()),
        FitMode::Contain | FitMode::Cover => {
            let src_ratio = src.width() / src.height().max(1.0);
            let dst_ratio = dst.width() / dst.height().max(1.0);

            let scale = match fit {
                FitMode::Contain => {
                    if src_ratio > dst_ratio {
                        dst.width() / src.width().max(1.0)
                    } else {
                        dst.height() / src.height().max(1.0)
                    }
                }
                FitMode::Cover => {
                    if src_ratio > dst_ratio {
                        dst.height() / src.height().max(1.0)
                    } else {
                        dst.width() / src.width().max(1.0)
                    }
                }
                _ => 1.0,
            };

            let width = src.width() * scale;
            let height = src.height() * scale;
            let x = dst.left + (dst.width() - width) * 0.5;
            let y = dst.top + (dst.height() - height) * 0.5;
            Rect::from_xywh(x, y, width, height)
        }
    }
}

fn style_literal(value: &StyleValue) -> f32 {
    value.as_literal().unwrap_or(0.0)
}
