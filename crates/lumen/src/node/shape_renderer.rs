use std::sync::Arc;

use skia_safe::{Color, Paint, PaintStyle, Path, RRect, Rect};

use crate::{
    error::LumenError,
    node::{
        InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue, ShapeGeometry,
        VectorData, VectorPosition, VectorStyle, VectorTextData,
        pixel_utils::{make_skia_image, read_surface_rgba, render_with_skia, to_skia_color},
    },
    raster::{BitmapFrame, RasterFrame, RectI},
    render::RenderContext,
};

#[derive(Debug, Clone, PartialEq)]
pub struct ShapeRenderer {
    pub fill_color: [u8; 4],
    pub stroke_color: [u8; 4],
    pub stroke_width: f32,
    pub fill_enabled: bool,
    pub stroke_enabled: bool,
}

impl Default for ShapeRenderer {
    fn default() -> Self {
        Self {
            fill_color: [255, 255, 255, 255],
            stroke_color: [0, 0, 0, 255],
            stroke_width: 1.0,
            fill_enabled: true,
            stroke_enabled: false,
        }
    }
}

impl NodeEval for ShapeRenderer {
    fn input_port_defs(&self) -> &'static [InputPortDef] {
        &[InputPortDef {
            name: "vector",
            kind: PortKind::Vector,
            optional: false,
        }]
    }

    fn output_port_defs(&self) -> &'static [OutputPortDef] {
        &[OutputPortDef {
            name: "output",
            kind: PortKind::RasterFrame,
        }]
    }

    fn evaluate(
        &self,
        inputs: &NodeInputs,
        ctx: &mut RenderContext,
    ) -> Result<PortValue, LumenError> {
        let vector = inputs.get_vector("vector")?;
        let raster = rasterize_vector(vector, self, ctx);
        Ok(PortValue::RasterFrame(raster))
    }
}

#[derive(Debug, Clone, Copy)]
struct ResolvedVectorStyle {
    fill_color: [u8; 4],
    fill_enabled: bool,
    stroke_color: [u8; 4],
    stroke_width: f32,
    stroke_enabled: bool,
}

pub(crate) fn rasterize_vector(
    vector: &VectorData,
    renderer: &ShapeRenderer,
    ctx: &mut RenderContext,
) -> RasterFrame {
    match vector {
        VectorData::Shape {
            geometry,
            style,
            position,
        } => rasterize_geometry(geometry, *position, style, renderer, ctx),
        VectorData::Text(text) => rasterize_text(text, renderer, ctx),
        VectorData::Group { children, position } => {
            rasterize_group(children, *position, renderer, ctx)
        }
    }
}

fn rasterize_geometry(
    geometry: &ShapeGeometry,
    position: VectorPosition,
    style: &VectorStyle,
    renderer: &ShapeRenderer,
    ctx: &mut RenderContext,
) -> RasterFrame {
    let style = resolve_style(style, renderer);
    let (path, width, height) = build_path(geometry);
    let width = width.max(1);
    let height = height.max(1);
    let pad = draw_padding(style);
    let bounds = positioned_bounds(width, height, position, pad);

    let pool = Arc::clone(&ctx.surface_pool);
    if let Ok(mut surface_ref) = pool.acquire_raster(bounds.width, bounds.height) {
        if let Some(surface) = surface_ref.surface_mut() {
            let canvas = surface.canvas();
            canvas.restore_to_count(1);
            canvas.reset_matrix();
            canvas.clear(Color::TRANSPARENT);
            canvas.save();
            canvas.translate((bounds.draw_x, bounds.draw_y));
            draw_shape(canvas, &path, style);
            canvas.restore();
            let bytes = read_surface_rgba(surface, bounds.width, bounds.height, Some(ctx));
            return bitmap_with_bounds(bytes, &bounds);
        }
    }

    // Fallback: allocate a fresh surface
    let Some(mut surface) =
        skia_safe::surfaces::raster_n32_premul((bounds.width as i32, bounds.height as i32))
    else {
        return RasterFrame::bitmap(Arc::new(vec![0; 4]), 1, 1);
    };

    surface.canvas().clear(Color::TRANSPARENT);
    surface.canvas().save();
    surface.canvas().translate((bounds.draw_x, bounds.draw_y));
    draw_shape(surface.canvas(), &path, style);
    surface.canvas().restore();

    bitmap_with_bounds(
        read_surface_rgba(&mut surface, bounds.width, bounds.height, Some(ctx)),
        &bounds,
    )
}

fn resolve_style(style: &VectorStyle, renderer: &ShapeRenderer) -> ResolvedVectorStyle {
    let fill_color = style.color.unwrap_or(renderer.fill_color);
    let fill_enabled = if style.color.is_some() {
        true
    } else {
        renderer.fill_enabled
    };

    let (stroke_color, stroke_width, stroke_enabled) = match style.stroke {
        Some(stroke) => (stroke.color, stroke.width.max(0.0), stroke.width > 0.0),
        None => (
            renderer.stroke_color,
            renderer.stroke_width.max(0.0),
            renderer.stroke_enabled && renderer.stroke_width > 0.0,
        ),
    };

    ResolvedVectorStyle {
        fill_color,
        fill_enabled,
        stroke_color,
        stroke_width,
        stroke_enabled,
    }
}

fn rasterize_text(
    text: &VectorTextData,
    renderer: &ShapeRenderer,
    ctx: &mut RenderContext,
) -> RasterFrame {
    let style = resolve_style(&text.style, renderer);
    let text_color = if style.fill_enabled {
        style.fill_color
    } else if style.stroke_enabled {
        // Skia paragraph text is currently rasterized as fill only here.
        // If no fill is specified, fall back to the resolved stroke color.
        style.stroke_color
    } else {
        [0, 0, 0, 0]
    };

    let raster_text = crate::node::text::Text {
        content: text.content.clone(),
        font_family: text.font_family.clone(),
        font_size: text.font_size,
        font_weight: text.font_weight,
        font_style: text.font_style,
        max_width: text.max_width,
        color: text_color,
        alignment: text.alignment,
    };

    match raster_text.evaluate(&NodeInputs::new(), ctx) {
        Ok(PortValue::RasterFrame(frame)) => {
            let (text_w, text_h) = frame.dimensions();
            let pad = draw_padding(style);
            let bounds = positioned_bounds(text_w.max(1), text_h.max(1), text.position, pad);
            offset_raster_into_bounds(frame, &bounds, ctx)
        }
        Ok(PortValue::Vector(_)) | Err(_) => RasterFrame::bitmap(Arc::new(vec![0; 4]), 1, 1),
    }
}

fn rasterize_group(
    children: &[VectorData],
    group_position: VectorPosition,
    renderer: &ShapeRenderer,
    ctx: &mut RenderContext,
) -> RasterFrame {
    let mut layers = Vec::with_capacity(children.len());
    for child in children {
        layers.push(rasterize_vector(child, renderer, ctx));
    }

    if layers.is_empty() {
        return RasterFrame::bitmap(Arc::new(vec![0; 4]), 1, 1);
    }
    if layers.len() == 1 {
        let single = layers.pop().expect("length checked");
        if group_position == VectorPosition::default() {
            return single;
        }
        let (w, h) = single.dimensions();
        let bounds = positioned_bounds(w.max(1), h.max(1), group_position, 0.0);
        return offset_raster_into_bounds(single, &bounds, ctx);
    }

    let union = layers
        .iter()
        .map(RasterFrame::format_rect)
        .reduce(union_rect)
        .unwrap_or(RectI::from_size(1, 1));
    let translated_union = RectI::new(
        union.x + group_position.x.floor() as i32,
        union.y + group_position.y.floor() as i32,
        union.width,
        union.height,
    );

    let rendered = render_with_skia(
        union.width.max(1),
        union.height.max(1),
        Some(ctx),
        |canvas| {
            canvas.clear(Color::TRANSPARENT);
            for layer in &layers {
                let (bytes, width, height) = layer.clone().into_parts();
                let Some(image) = make_skia_image(
                    &bytes,
                    width,
                    height,
                    (width as usize) * 4,
                    layer.alpha_mode(),
                ) else {
                    continue;
                };
                let layer_rect = layer.format_rect();
                let offset_x = (layer_rect.x - union.x) as f32;
                let offset_y = (layer_rect.y - union.y) as f32;
                canvas.draw_image(&image, (offset_x, offset_y), None);
            }
        },
    );

    RasterFrame::Bitmap(BitmapFrame::with_domain(
        Arc::new(rendered),
        union.width.max(1),
        union.height.max(1),
        translated_union,
        translated_union,
    ))
}

fn draw_shape(canvas: &skia_safe::Canvas, path: &Path, style: ResolvedVectorStyle) {
    if style.fill_enabled {
        let mut fill = Paint::default();
        fill.set_anti_alias(true);
        fill.set_style(PaintStyle::Fill);
        fill.set_color(to_skia_color(style.fill_color));
        canvas.draw_path(path, &fill);
    }

    if style.stroke_enabled && style.stroke_width > 0.0 {
        let mut stroke = Paint::default();
        stroke.set_anti_alias(true);
        stroke.set_style(PaintStyle::Stroke);
        stroke.set_stroke_width(style.stroke_width);
        stroke.set_color(to_skia_color(style.stroke_color));
        canvas.draw_path(path, &stroke);
    }
}

#[derive(Debug, Clone, Copy)]
struct PositionedRasterBounds {
    width: u32,
    height: u32,
    draw_x: f32,
    draw_y: f32,
    format_rect: RectI,
}

fn draw_padding(style: ResolvedVectorStyle) -> f32 {
    let stroke_pad = if style.stroke_enabled {
        style.stroke_width.max(0.0) * 0.5
    } else {
        0.0
    };
    1.0 + stroke_pad
}

fn positioned_bounds(
    content_w: u32,
    content_h: u32,
    position: VectorPosition,
    pad: f32,
) -> PositionedRasterBounds {
    let min_x = (position.x - pad).floor() as i32;
    let min_y = (position.y - pad).floor() as i32;
    let max_x = (position.x + content_w as f32 + pad).ceil() as i32;
    let max_y = (position.y + content_h as f32 + pad).ceil() as i32;
    let width = (max_x - min_x).max(1) as u32;
    let height = (max_y - min_y).max(1) as u32;
    PositionedRasterBounds {
        width,
        height,
        draw_x: position.x - min_x as f32,
        draw_y: position.y - min_y as f32,
        format_rect: RectI::new(min_x, min_y, width, height),
    }
}

fn bitmap_with_bounds(bytes: Vec<u8>, bounds: &PositionedRasterBounds) -> RasterFrame {
    RasterFrame::Bitmap(BitmapFrame::with_domain(
        Arc::new(bytes),
        bounds.width,
        bounds.height,
        bounds.format_rect,
        bounds.format_rect,
    ))
}

fn offset_raster_into_bounds(
    frame: RasterFrame,
    bounds: &PositionedRasterBounds,
    ctx: &mut RenderContext,
) -> RasterFrame {
    let (bytes, width, height) = frame.clone().into_parts();
    if width == 0 || height == 0 {
        return RasterFrame::bitmap(Arc::new(Vec::new()), 0, 0);
    }

    let rendered = render_with_skia(bounds.width, bounds.height, Some(ctx), |canvas| {
        canvas.clear(Color::TRANSPARENT);
        let Some(image) = make_skia_image(
            &bytes,
            width,
            height,
            (width as usize) * 4,
            frame.alpha_mode(),
        ) else {
            return;
        };
        canvas.draw_image(&image, (bounds.draw_x, bounds.draw_y), None);
    });

    bitmap_with_bounds(rendered, bounds)
}

fn union_rect(left: RectI, right: RectI) -> RectI {
    let min_x = left.x.min(right.x);
    let min_y = left.y.min(right.y);
    let max_x = left.right().max(right.right());
    let max_y = left.bottom().max(right.bottom());
    let width = (max_x - i64::from(min_x)).max(1) as u32;
    let height = (max_y - i64::from(min_y)).max(1) as u32;
    RectI::new(min_x, min_y, width, height)
}

fn build_path(geometry: &ShapeGeometry) -> (Path, u32, u32) {
    match geometry {
        ShapeGeometry::Rectangle {
            width,
            height,
            border_radius,
        } => {
            let width = (*width).max(1);
            let height = (*height).max(1);
            let border_radius = (*border_radius)
                .max(0.0)
                .min(width.min(height) as f32 * 0.5);
            let rect = Rect::from_xywh(0.0, 0.0, width as f32, height as f32);
            (
                if border_radius > 0.0 {
                    Path::rrect(RRect::new_rect_xy(rect, border_radius, border_radius), None)
                } else {
                    Path::rect(rect, None)
                },
                width,
                height,
            )
        }
        ShapeGeometry::Ellipse { width, height } => {
            let width = (*width).max(1);
            let height = (*height).max(1);
            (
                Path::oval(Rect::from_xywh(0.0, 0.0, width as f32, height as f32), None),
                width,
                height,
            )
        }
        ShapeGeometry::Polygon { points } => polygon_path(points),
    }
}

fn polygon_path(points: &[(f32, f32)]) -> (Path, u32, u32) {
    if points.is_empty() {
        return (Path::rect(Rect::from_xywh(0.0, 0.0, 1.0, 1.0), None), 1, 1);
    }

    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for &(x, y) in points {
        if x.is_finite() && y.is_finite() {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }

    if !min_x.is_finite() || !min_y.is_finite() || !max_x.is_finite() || !max_y.is_finite() {
        return (Path::rect(Rect::from_xywh(0.0, 0.0, 1.0, 1.0), None), 1, 1);
    }

    let width = (max_x - min_x).ceil().max(1.0) as u32;
    let height = (max_y - min_y).ceil().max(1.0) as u32;
    let normalized_points: Vec<skia_safe::Point> = points
        .iter()
        .map(|(x, y)| skia_safe::Point::new(*x - min_x, *y - min_y))
        .collect();

    (
        Path::polygon(&normalized_points, true, None, None),
        width,
        height,
    )
}
