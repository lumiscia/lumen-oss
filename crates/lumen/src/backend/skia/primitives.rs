use skia_safe::canvas::SrcRectConstraint;
use skia_safe::{
    AlphaType, BlendMode as SkiaBlendMode, Canvas, ClipOp, Color4f, ColorType, Data, Font, FontMgr,
    FontStyle, ImageInfo, Paint, PaintStyle, PathBuilder, RRect, Rect, Vector, color_filters,
    font_style::{Slant, Weight, Width},
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
    frame_state: &RuntimeFrameContext,
    frame_image: Option<&FrameImage>,
    bounds: Rect,
    opacity: f32,
) -> Result<(), RenderError> {
    match &operation.kind {
        CompiledOperationKind::Solid => {
            if let Some(fill) = operation.style.fill {
                draw_solid(
                    canvas,
                    bounds,
                    fill,
                    opacity,
                    operation.style.base.blend_mode,
                    operation,
                )
            } else {
                Ok(())
            }
        }
        CompiledOperationKind::Shape(shape) => draw_shape(
            canvas,
            &shape.geometry,
            operation,
            frame_state,
            bounds,
            opacity,
        ),
        CompiledOperationKind::Text(text) => draw_text(
            canvas,
            text.content.as_str(),
            operation,
            frame_state,
            bounds,
            opacity,
        ),
        CompiledOperationKind::Image(_) | CompiledOperationKind::Video(_) => {
            if let Some(image) = frame_image {
                draw_image(
                    canvas,
                    bounds,
                    image,
                    operation.style.fit,
                    opacity,
                    operation.style.base.blend_mode,
                    operation.style.color_matrix.as_ref(),
                    resolved_corner_radius(operation, frame_state),
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
    operation: &CompiledOperation,
) -> Result<(), RenderError> {
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_color4f(to_color4f(fill, opacity), None);
    paint.set_style(PaintStyle::Fill);
    paint.set_blend_mode(to_skia_blend_mode(blend_mode));
    apply_color_matrix(&mut paint, operation.style.color_matrix.as_ref());
    canvas.draw_rect(bounds, &paint);
    Ok(())
}

fn draw_shape(
    canvas: &Canvas,
    geometry: &ShapeGeometry,
    operation: &CompiledOperation,
    frame_state: &RuntimeFrameContext,
    bounds: Rect,
    opacity: f32,
) -> Result<(), RenderError> {
    let corner_radius = resolved_corner_radius(operation, frame_state);
    let polygon_path = match geometry {
        ShapeGeometry::Polygon { vertices, closed } => {
            Some(build_polygon_path(vertices, *closed, bounds))
        }
        _ => None,
    };

    if let Some(fill) = operation.style.fill {
        let mut fill_paint = Paint::default();
        fill_paint.set_anti_alias(true);
        fill_paint.set_style(PaintStyle::Fill);
        fill_paint.set_color4f(to_color4f(fill, opacity), None);
        fill_paint.set_blend_mode(to_skia_blend_mode(operation.style.base.blend_mode));
        apply_color_matrix(&mut fill_paint, operation.style.color_matrix.as_ref());

        match geometry {
            ShapeGeometry::Rect => draw_rect_or_rrect(canvas, bounds, corner_radius, &fill_paint),
            ShapeGeometry::Ellipse => {
                canvas.draw_oval(bounds, &fill_paint);
            }
            ShapeGeometry::Polygon { .. } => {
                if let Some(path) = polygon_path.as_ref() {
                    canvas.draw_path(path, &fill_paint);
                }
            }
        }
    }

    if let Some(stroke) = &operation.style.stroke {
        let mut stroke_paint = Paint::default();
        stroke_paint.set_anti_alias(true);
        stroke_paint.set_style(PaintStyle::Stroke);
        stroke_paint.set_stroke_width(stroke.width.resolve(frame_state).max(0.0));
        stroke_paint.set_color4f(to_color4f(stroke.color, opacity), None);
        stroke_paint.set_blend_mode(to_skia_blend_mode(operation.style.base.blend_mode));
        apply_color_matrix(&mut stroke_paint, operation.style.color_matrix.as_ref());
        apply_stroke_dash_effect(&mut stroke_paint, stroke, frame_state);

        match geometry {
            ShapeGeometry::Rect => {
                draw_rect_or_rrect(canvas, bounds, corner_radius, &stroke_paint);
            }
            ShapeGeometry::Ellipse => {
                canvas.draw_oval(bounds, &stroke_paint);
            }
            ShapeGeometry::Polygon { .. } => {
                if let Some(path) = polygon_path.as_ref() {
                    canvas.draw_path(path, &stroke_paint);
                }
            }
        }
    }

    Ok(())
}

fn draw_text(
    canvas: &Canvas,
    content: &str,
    operation: &CompiledOperation,
    frame_state: &RuntimeFrameContext,
    bounds: Rect,
    opacity: f32,
) -> Result<(), RenderError> {
    if content.is_empty() {
        return Ok(());
    }

    let font = resolve_font(operation, frame_state);

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
    apply_color_matrix(&mut paint, operation.style.color_matrix.as_ref());

    let line_height_factor = operation.style.line_height.resolve(frame_state).max(0.1);
    let font_size = font.size();
    let line_step = font_size * line_height_factor;
    let lines: Vec<&str> = content.split('\n').collect();
    let letter_spacing = operation.style.letter_spacing.resolve(frame_state);
    let total_text_height = line_step * (lines.len() as f32 - 1.0) + font_size;

    let first_baseline = match operation.style.vertical_align {
        crate::model::VerticalAlign::Top => bounds.top + font_size,
        crate::model::VerticalAlign::Middle => {
            bounds.top + (bounds.height() - total_text_height) * 0.5 + font_size
        }
        crate::model::VerticalAlign::Bottom => bounds.bottom - total_text_height + font_size,
    };

    for (i, line) in lines.iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        let line_width = measure_line_width(line, &font, &paint, letter_spacing);
        let x = match operation.style.align {
            crate::model::TextAlign::Left => bounds.left,
            crate::model::TextAlign::Center => bounds.left + (bounds.width() - line_width) * 0.5,
            crate::model::TextAlign::Right => bounds.right - line_width,
        };
        let baseline = first_baseline + (i as f32) * line_step;
        draw_text_line(canvas, line, x, baseline, &font, &paint, letter_spacing);
    }
    Ok(())
}

fn apply_stroke_dash_effect(
    paint: &mut Paint,
    stroke: &crate::compile::CompiledStrokeStyle,
    frame_state: &RuntimeFrameContext,
) {
    if stroke.dash_pattern.is_empty() {
        return;
    }

    let mut intervals = stroke
        .dash_pattern
        .iter()
        .map(|value| value.resolve(frame_state).abs())
        .filter(|value| *value > 0.0)
        .collect::<Vec<_>>();
    if intervals.len() < 2 {
        return;
    }
    if intervals.len() % 2 == 1 {
        let first = intervals[0];
        intervals.push(first);
    }

    let phase = stroke.dash_offset.resolve(frame_state);
    paint.set_path_effect(skia_safe::PathEffect::dash(intervals.as_slice(), phase));
}

fn resolve_font(operation: &CompiledOperation, frame_state: &RuntimeFrameContext) -> Font {
    let mut font = Font::default();
    font.set_size(operation.style.font_size.resolve(frame_state).max(1.0));

    let font_mgr = FontMgr::new();
    let font_weight = operation.style.font_weight.resolve(frame_state);
    if let Some(family) = operation.style.font_family.as_deref() {
        let requested = font_style_for_weight(font_weight);
        if let Some(typeface) = font_mgr.match_family_style(family, requested) {
            font.set_typeface(typeface);
            return font;
        }
    }

    if let Some(typeface) = font_mgr.legacy_make_typeface(None, font_style_for_weight(font_weight))
    {
        font.set_typeface(typeface);
    }

    font
}

fn font_style_for_weight(weight: f32) -> FontStyle {
    let normalized = weight.clamp(1.0, 1000.0).round() as i32;
    FontStyle::new(Weight::from(normalized), Width::NORMAL, Slant::Upright)
}

fn measure_line_width(line: &str, font: &Font, paint: &Paint, letter_spacing: f32) -> f32 {
    let width = font.measure_str(line, Some(paint)).0;
    let count = line.chars().count();
    if count <= 1 {
        width
    } else {
        width + letter_spacing * (count.saturating_sub(1) as f32)
    }
}

fn draw_text_line(
    canvas: &Canvas,
    line: &str,
    x: f32,
    baseline: f32,
    font: &Font,
    paint: &Paint,
    letter_spacing: f32,
) {
    let count = line.chars().count();
    if count <= 1 || letter_spacing.abs() <= f32::EPSILON {
        canvas.draw_str(line, (x, baseline), font, paint);
        return;
    }

    let mut cursor = x;
    for glyph in line.chars() {
        let glyph = glyph.to_string();
        canvas.draw_str(glyph.as_str(), (cursor, baseline), font, paint);
        let advance = font.measure_str(glyph.as_str(), Some(paint)).0;
        cursor += advance + letter_spacing;
    }
}
fn draw_image(
    canvas: &Canvas,
    bounds: Rect,
    frame_image: &FrameImage,
    fit: FitMode,
    opacity: f32,
    blend_mode: BlendMode,
    color_matrix: Option<&[[f32; 5]; 4]>,
    corner_radius: [f32; 4],
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
    apply_color_matrix(&mut paint, color_matrix);

    let has_radius = corner_radius.iter().any(|r| *r > 0.0);
    if has_radius {
        canvas.save();
        let radii = [
            Vector::new(corner_radius[0], corner_radius[0]),
            Vector::new(corner_radius[1], corner_radius[1]),
            Vector::new(corner_radius[2], corner_radius[2]),
            Vector::new(corner_radius[3], corner_radius[3]),
        ];
        let rrect = RRect::new_rect_radii(bounds, &radii);
        canvas.clip_rrect(rrect, ClipOp::Intersect, true);
    }

    canvas.draw_image_rect(image, Some((&src, SrcRectConstraint::Strict)), &dst, &paint);

    if has_radius {
        canvas.restore();
    }
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

fn resolved_corner_radius(
    operation: &CompiledOperation,
    frame_state: &RuntimeFrameContext,
) -> [f32; 4] {
    let Some(corner_radius) = &operation.style.corner_radius else {
        return [0.0; 4];
    };
    [
        corner_radius[0].resolve(frame_state).max(0.0),
        corner_radius[1].resolve(frame_state).max(0.0),
        corner_radius[2].resolve(frame_state).max(0.0),
        corner_radius[3].resolve(frame_state).max(0.0),
    ]
}

fn draw_rect_or_rrect(canvas: &Canvas, bounds: Rect, corner_radius: [f32; 4], paint: &Paint) {
    if corner_radius.iter().any(|value| *value > 0.0) {
        let radii = [
            Vector::new(corner_radius[0], corner_radius[0]),
            Vector::new(corner_radius[1], corner_radius[1]),
            Vector::new(corner_radius[2], corner_radius[2]),
            Vector::new(corner_radius[3], corner_radius[3]),
        ];
        let rrect = RRect::new_rect_radii(bounds, &radii);
        canvas.draw_rrect(rrect, paint);
    } else {
        canvas.draw_rect(bounds, paint);
    }
}

fn build_polygon_path(
    vertices: &[crate::model::PolygonVertex],
    closed: bool,
    bounds: Rect,
) -> skia_safe::Path {
    let mut builder = PathBuilder::new();
    let Some(first) = vertices.first() else {
        return builder.detach();
    };
    let first_point = (
        bounds.left + style_literal(&first.x),
        bounds.top + style_literal(&first.y),
    );
    builder.move_to(first_point);

    for index in 1..vertices.len() {
        add_polygon_segment(&mut builder, &vertices[index - 1], &vertices[index], bounds);
    }

    if closed {
        let last_index = vertices.len().saturating_sub(1);
        if last_index > 0 {
            add_polygon_segment(&mut builder, &vertices[last_index], first, bounds);
        }
        builder.close();
    }

    builder.detach()
}

fn add_polygon_segment(
    builder: &mut PathBuilder,
    from: &crate::model::PolygonVertex,
    to: &crate::model::PolygonVertex,
    bounds: Rect,
) {
    let end = (
        bounds.left + style_literal(&to.x),
        bounds.top + style_literal(&to.y),
    );
    let cp_out = from.cp_out.as_ref().map(|handle| {
        (
            bounds.left + style_literal(&handle[0]),
            bounds.top + style_literal(&handle[1]),
        )
    });
    let cp_in = to.cp_in.as_ref().map(|handle| {
        (
            bounds.left + style_literal(&handle[0]),
            bounds.top + style_literal(&handle[1]),
        )
    });

    if cp_out.is_some() || cp_in.is_some() {
        let from_point = (
            bounds.left + style_literal(&from.x),
            bounds.top + style_literal(&from.y),
        );
        builder.cubic_to(cp_out.unwrap_or(from_point), cp_in.unwrap_or(end), end);
    } else {
        builder.line_to(end);
    }
}

fn apply_color_matrix(paint: &mut Paint, matrix: Option<&[[f32; 5]; 4]>) {
    let Some(matrix) = matrix else {
        return;
    };
    let row_major = flatten_color_matrix(matrix);
    paint.set_color_filter(color_filters::matrix_row_major(&row_major, None));
}

fn flatten_color_matrix(matrix: &[[f32; 5]; 4]) -> [f32; 20] {
    [
        matrix[0][0],
        matrix[0][1],
        matrix[0][2],
        matrix[0][3],
        matrix[0][4],
        matrix[1][0],
        matrix[1][1],
        matrix[1][2],
        matrix[1][3],
        matrix[1][4],
        matrix[2][0],
        matrix[2][1],
        matrix[2][2],
        matrix[2][3],
        matrix[2][4],
        matrix[3][0],
        matrix[3][1],
        matrix[3][2],
        matrix[3][3],
        matrix[3][4],
    ]
}

#[cfg(test)]
mod tests {
    use skia_safe::{BlendMode as SkiaBlendMode, Rect};

    use crate::model::{BlendMode, FitMode};

    use super::{fit_rect, to_skia_blend_mode};

    #[test]
    fn fit_rect_contain_preserves_aspect_ratio_inside_bounds() {
        let src = Rect::from_xywh(0.0, 0.0, 1920.0, 1080.0);
        let dst = Rect::from_xywh(100.0, 100.0, 800.0, 800.0);
        let fitted = fit_rect(src, dst, FitMode::Contain);

        assert_eq!(fitted.width(), 800.0);
        assert_eq!(fitted.height(), 450.0);
        assert_eq!(fitted.left, 100.0);
        assert_eq!(fitted.top, 275.0);
    }

    #[test]
    fn fit_rect_cover_expands_to_fill_bounds() {
        let src = Rect::from_xywh(0.0, 0.0, 1920.0, 1080.0);
        let dst = Rect::from_xywh(0.0, 0.0, 800.0, 800.0);
        let fitted = fit_rect(src, dst, FitMode::Cover);

        assert!((fitted.width() - 1422.2222).abs() < 0.01);
        assert!((fitted.height() - 800.0).abs() < 0.01);
        assert!((fitted.left + 311.1111).abs() < 0.01);
    }

    #[test]
    fn blend_mode_mapping_is_complete() {
        let mappings = [
            (BlendMode::Normal, SkiaBlendMode::SrcOver),
            (BlendMode::Multiply, SkiaBlendMode::Multiply),
            (BlendMode::Screen, SkiaBlendMode::Screen),
            (BlendMode::Overlay, SkiaBlendMode::Overlay),
            (BlendMode::Darken, SkiaBlendMode::Darken),
            (BlendMode::Lighten, SkiaBlendMode::Lighten),
            (BlendMode::ColorDodge, SkiaBlendMode::ColorDodge),
            (BlendMode::ColorBurn, SkiaBlendMode::ColorBurn),
            (BlendMode::HardLight, SkiaBlendMode::HardLight),
            (BlendMode::SoftLight, SkiaBlendMode::SoftLight),
            (BlendMode::Difference, SkiaBlendMode::Difference),
            (BlendMode::Exclusion, SkiaBlendMode::Exclusion),
            (BlendMode::Hue, SkiaBlendMode::Hue),
            (BlendMode::Saturation, SkiaBlendMode::Saturation),
            (BlendMode::Color, SkiaBlendMode::Color),
            (BlendMode::Luminosity, SkiaBlendMode::Luminosity),
        ];

        for (input, expected) in mappings {
            assert_eq!(to_skia_blend_mode(input), expected);
        }
    }
}
