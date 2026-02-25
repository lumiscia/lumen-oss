use std::sync::Arc;

use skia_safe::{Color, Paint, PaintStyle, Path, RRect, Rect};

use crate::{
    error::LumenError,
    node::{
        InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue, ShapeGeometry,
        VectorData, VectorStyle, VectorTextData,
        pixel_utils::{make_skia_image, read_surface_rgba, render_with_skia, to_skia_color},
    },
    raster::RasterFrame,
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
        VectorData::Shape { geometry, style } => rasterize_geometry(geometry, style, renderer, ctx),
        VectorData::Text(text) => rasterize_text(text, renderer, ctx),
        VectorData::Group(children) => rasterize_group(children, renderer, ctx),
    }
}

fn rasterize_geometry(
    geometry: &ShapeGeometry,
    style: &VectorStyle,
    renderer: &ShapeRenderer,
    ctx: &mut RenderContext,
) -> RasterFrame {
    let (path, width, height) = build_path(geometry);
    let style = resolve_style(style, renderer);
    let width = width.max(1);
    let height = height.max(1);

    let pool = Arc::clone(&ctx.surface_pool);
    if let Ok(mut surface_ref) = pool.acquire_raster(width, height) {
        if let Some(surface) = surface_ref.surface_mut() {
            let canvas = surface.canvas();
            canvas.restore_to_count(1);
            canvas.reset_matrix();
            canvas.clear(Color::TRANSPARENT);
            draw_shape(canvas, &path, style);
            let bytes = read_surface_rgba(surface, width, height, Some(ctx));
            return RasterFrame::bitmap(Arc::new(bytes), width, height);
        }
    }

    // Fallback: allocate a fresh surface
    let Some(mut surface) = skia_safe::surfaces::raster_n32_premul((width as i32, height as i32))
    else {
        return RasterFrame::bitmap(Arc::new(vec![0; 4]), 1, 1);
    };

    surface.canvas().clear(Color::TRANSPARENT);
    draw_shape(surface.canvas(), &path, style);

    RasterFrame::bitmap(
        Arc::new(read_surface_rgba(&mut surface, width, height, Some(ctx))),
        width,
        height,
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

fn rasterize_text(text: &VectorTextData, renderer: &ShapeRenderer, ctx: &mut RenderContext) -> RasterFrame {
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
        Ok(PortValue::RasterFrame(frame)) => frame,
        Ok(PortValue::Vector(_)) | Err(_) => RasterFrame::bitmap(Arc::new(vec![0; 4]), 1, 1),
    }
}

fn rasterize_group(children: &[VectorData], renderer: &ShapeRenderer, ctx: &mut RenderContext) -> RasterFrame {
    let mut layers = Vec::with_capacity(children.len());
    for child in children {
        layers.push(rasterize_vector(child, renderer, ctx));
    }

    if layers.is_empty() {
        return RasterFrame::bitmap(Arc::new(vec![0; 4]), 1, 1);
    }
    if layers.len() == 1 {
        return layers.pop().expect("length checked");
    }

    let (out_w, out_h) = layers.iter().fold((1_u32, 1_u32), |(max_w, max_h), frame| {
        let (w, h) = frame.dimensions();
        (max_w.max(w), max_h.max(h))
    });

    let rendered = render_with_skia(out_w, out_h, Some(ctx), |canvas| {
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
            canvas.draw_image(&image, (0.0, 0.0), None);
        }
    });

    RasterFrame::bitmap(Arc::new(rendered), out_w, out_h)
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

fn build_path(geometry: &ShapeGeometry) -> (Path, u32, u32) {
    match geometry {
        ShapeGeometry::Rectangle {
            width,
            height,
            border_radius,
        } => {
            let width = (*width).max(1);
            let height = (*height).max(1);
            let border_radius = (*border_radius).max(0.0).min(width.min(height) as f32 * 0.5);
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
