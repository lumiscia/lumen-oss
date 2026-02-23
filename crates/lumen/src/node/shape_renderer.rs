use std::sync::Arc;

use skia_safe::{AlphaType, Color, ColorType, ImageInfo, Paint, PaintStyle, Path, Rect, surfaces};

use crate::{
    error::LumenError,
    node::{
        InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue, ShapeGeometry,
        VectorData,
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
        _ctx: &mut RenderContext,
    ) -> Result<PortValue, LumenError> {
        let vector = inputs.get_vector("vector")?;
        let raster = match vector {
            VectorData::Shape(geometry) => rasterize_geometry(geometry, self),
        };
        Ok(PortValue::RasterFrame(raster))
    }
}

fn rasterize_geometry(geometry: &ShapeGeometry, renderer: &ShapeRenderer) -> RasterFrame {
    let (path, width, height) = build_path(geometry);
    let width = width.max(1);
    let height = height.max(1);

    let Some(mut surface) = surfaces::raster_n32_premul((width as i32, height as i32)) else {
        return RasterFrame::Bitmap(Arc::new(vec![0; 4]), 1, 1);
    };

    let canvas = surface.canvas();
    canvas.clear(Color::TRANSPARENT);

    if renderer.fill_enabled {
        let mut fill = Paint::default();
        fill.set_anti_alias(true);
        fill.set_style(PaintStyle::Fill);
        fill.set_color(to_color(renderer.fill_color));
        canvas.draw_path(&path, &fill);
    }

    if renderer.stroke_enabled && renderer.stroke_width > 0.0 {
        let mut stroke = Paint::default();
        stroke.set_anti_alias(true);
        stroke.set_style(PaintStyle::Stroke);
        stroke.set_stroke_width(renderer.stroke_width);
        stroke.set_color(to_color(renderer.stroke_color));
        canvas.draw_path(&path, &stroke);
    }

    RasterFrame::Bitmap(
        Arc::new(read_surface_rgba(&mut surface, width, height)),
        width,
        height,
    )
}

fn build_path(geometry: &ShapeGeometry) -> (Path, u32, u32) {
    match geometry {
        ShapeGeometry::Rectangle { width, height } => {
            let width = (*width).max(1);
            let height = (*height).max(1);
            (
                Path::rect(Rect::from_xywh(0.0, 0.0, width as f32, height as f32), None),
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

fn read_surface_rgba(surface: &mut skia_safe::Surface, width: u32, height: u32) -> Vec<u8> {
    let byte_len = rgba_byte_len(width, height).unwrap_or(4);
    let mut bytes = vec![0_u8; byte_len];
    let info = ImageInfo::new(
        (width as i32, height as i32),
        ColorType::RGBA8888,
        AlphaType::Premul,
        None,
    );
    if surface.read_pixels(&info, bytes.as_mut_slice(), (width * 4) as usize, (0, 0)) {
        bytes
    } else {
        vec![0_u8; byte_len]
    }
}

fn rgba_byte_len(width: u32, height: u32) -> Option<usize> {
    let pixels = u64::from(width).checked_mul(u64::from(height))?;
    let bytes = pixels.checked_mul(4)?;
    usize::try_from(bytes).ok()
}

fn to_color(color: [u8; 4]) -> Color {
    Color::from_argb(color[3], color[0], color[1], color[2])
}
