use skia_safe::{IRect, Paint, PaintStyle, Path, RRect, Rect, image::RequiredProperties};

use crate::{
    gpu_image::{AlphaMode, GpuImageFrame, RectI},
    media::MediaStore,
    node::{
        NodeId, NodeProperty, PortRef,
        pixel_utils::{ClearMode, render_to_surface_ephemeral, to_skia_color},
        source::text_layout::{TextLayoutStyle, build_paragraph},
        vector::{ShapeGeometry, VectorData, VectorStyle, VectorTextData},
    },
    render::{RenderContext, surface::SurfacePool},
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Node)]
pub struct ShapeRenderer {
    pub id: NodeId,

    #[property(expected = Color)]
    pub fill_color: NodeProperty,
    #[property(expected = Color)]
    pub stroke_color: NodeProperty,
    #[property(expected = Float)]
    pub stroke_width: NodeProperty,
    #[property(expected = Bool)]
    pub fill_enabled: NodeProperty,
    #[property(expected = Bool)]
    pub stroke_enabled: NodeProperty,

    #[input(kind = Vector)]
    pub vector: PortRef,
}

impl Default for ShapeRenderer {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            fill_color: NodeProperty::Color([255, 255, 255, 255]),
            stroke_color: NodeProperty::Color([0, 0, 0, 255]),
            stroke_width: NodeProperty::Float(1.0),
            fill_enabled: NodeProperty::Bool(true),
            stroke_enabled: NodeProperty::Bool(false),
            vector: PortRef::empty(),
        }
    }
}

#[node_impl]
impl ShapeRenderer {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<GpuImageFrame> {
        let vector = ctx.eval(&self.vector)?;
        let vector_data = match vector.as_ref() {
            crate::node::NodeResult::Raster(_) => {
                return Err(ctx.invalid_node_output_type(
                    self.vector.id,
                    "Vector",
                    "GpuImageFrame",
                ));
            }
            crate::node::NodeResult::Vector(vector_data) => vector_data,
            crate::node::NodeResult::None => {
                return Err(ctx.missing_node_output_error(self.vector.id));
            }
        };
        let renderer_style = resolve_renderer_style(self, ctx)?;
        Ok(clip_raster_to_output_rect(
            rasterize_vector_with_style(vector_data, renderer_style, ctx),
            ctx,
        ))
    }
}

#[derive(Debug, Clone, Copy)]
struct ResolvedRendererStyle {
    fill_color: [u8; 4],
    stroke_color: [u8; 4],
    stroke_width: f32,
    fill_enabled: bool,
    stroke_enabled: bool,
}

impl Default for ResolvedRendererStyle {
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

#[derive(Debug, Clone, Copy)]
struct ResolvedVectorStyle {
    fill_color: [u8; 4],
    fill_enabled: bool,
    stroke_color: [u8; 4],
    stroke_width: f32,
    stroke_enabled: bool,
}

pub(crate) fn rasterize_vector<S: SurfacePool, M: MediaStore>(
    vector: &VectorData,
    renderer: &ShapeRenderer,
    ctx: &mut RenderContext<'_, S, M>,
) -> GpuImageFrame {
    match resolve_renderer_style(renderer, ctx) {
        Ok(renderer_style) => rasterize_vector_with_style(vector, renderer_style, ctx),
        Err(_) => rasterize_vector_with_style(vector, ResolvedRendererStyle::default(), ctx),
    }
}

fn rasterize_vector_with_style<S: SurfacePool, M: MediaStore>(
    vector: &VectorData,
    renderer_style: ResolvedRendererStyle,
    ctx: &mut RenderContext<'_, S, M>,
) -> GpuImageFrame {
    let composition_width = ctx.renderer.composition.render_settings.width as f32;
    let Some(bounds) = measure_vector_bounds(
        vector,
        renderer_style,
        composition_width,
        AffineTransform::identity(),
    ) else {
        return transparent_frame(ctx, RectI::from_size(1, 1));
    };

    render_to_surface_ephemeral(
        bounds.width.max(1),
        bounds.height.max(1),
        ctx,
        bounds,
        bounds,
        AlphaMode::Premultiplied,
        ClearMode::Transparent,
        |canvas| {
            canvas.translate((-(bounds.x as f32), -(bounds.y as f32)));
            draw_vector(canvas, vector, renderer_style, composition_width);
        },
    )
    .unwrap_or_else(|_| transparent_frame(ctx, bounds))
}

fn resolve_renderer_style<S: SurfacePool, M: MediaStore>(
    renderer: &ShapeRenderer,
    ctx: &RenderContext<'_, S, M>,
) -> crate::Result<ResolvedRendererStyle> {
    Ok(ResolvedRendererStyle {
        fill_color: renderer.resolve_fill_color(ctx)?,
        stroke_color: renderer.resolve_stroke_color(ctx)?,
        stroke_width: renderer.resolve_stroke_width(ctx)? as f32,
        fill_enabled: renderer.resolve_fill_enabled(ctx)?,
        stroke_enabled: renderer.resolve_stroke_enabled(ctx)?,
    })
}

fn resolve_style(
    style: &VectorStyle,
    renderer_style: ResolvedRendererStyle,
) -> ResolvedVectorStyle {
    let fill_color = style.color.unwrap_or(renderer_style.fill_color);
    let fill_enabled = if style.color.is_some() {
        true
    } else {
        renderer_style.fill_enabled
    };

    let (stroke_color, stroke_width, stroke_enabled) = match style.stroke {
        Some(stroke) => (stroke.color, stroke.width.max(0.0), stroke.width > 0.0),
        None => (
            renderer_style.stroke_color,
            renderer_style.stroke_width.max(0.0),
            renderer_style.stroke_enabled && renderer_style.stroke_width > 0.0,
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

#[derive(Debug, Clone, Copy)]
struct TextLayoutMetrics {
    rendered_width: u32,
    rendered_height: u32,
    layout_width: f32,
    horizontal_offset: f32,
    vertical_offset: f32,
}

fn measure_vector_bounds(
    vector: &VectorData,
    renderer_style: ResolvedRendererStyle,
    composition_width: f32,
    transform: AffineTransform,
) -> Option<RectI> {
    match vector {
        VectorData::Shape {
            geometry,
            style,
            position,
        } => {
            let style = resolve_style(style, renderer_style);
            let path = build_path(geometry);
            Some(transformed_bounds(
                transform.then_translate(position.x, position.y),
                path.bounds,
                draw_padding(style),
            ))
        }
        VectorData::Text(text) => {
            let style = resolve_style(&text.style, renderer_style);
            let metrics = measure_text_layout(text, composition_width);
            Some(transformed_bounds(
                transform.then_translate(text.position.x, text.position.y),
                Rect::from_xywh(
                    -metrics.horizontal_offset,
                    metrics.vertical_offset,
                    metrics.rendered_width.max(1) as f32,
                    metrics.rendered_height.max(1) as f32,
                ),
                draw_padding(style),
            ))
        }
        VectorData::Group { children, position } => children
            .iter()
            .filter_map(|child| {
                measure_vector_bounds(
                    child,
                    renderer_style,
                    composition_width,
                    transform.then_translate(position.x, position.y),
                )
            })
            .reduce(union_rect),
        VectorData::Transformed {
            child,
            transform: child_transform,
        } => measure_vector_bounds(
            child,
            renderer_style,
            composition_width,
            transform.then_transform(*child_transform),
        ),
    }
}

fn draw_vector(
    canvas: &skia_safe::Canvas,
    vector: &VectorData,
    renderer_style: ResolvedRendererStyle,
    composition_width: f32,
) {
    match vector {
        VectorData::Shape {
            geometry,
            style,
            position,
        } => {
            let style = resolve_style(style, renderer_style);
            let path = build_path(geometry);
            canvas.save();
            canvas.translate((position.x, position.y));
            draw_shape(canvas, &path.path, style);
            canvas.restore();
        }
        VectorData::Text(text) => {
            canvas.save();
            canvas.translate((text.position.x, text.position.y));
            draw_text(canvas, text, renderer_style, composition_width);
            canvas.restore();
        }
        VectorData::Group { children, position } => {
            canvas.save();
            canvas.translate((position.x, position.y));
            for child in children {
                draw_vector(canvas, child, renderer_style, composition_width);
            }
            canvas.restore();
        }
        VectorData::Transformed { child, transform } => {
            canvas.save();
            canvas.translate((transform.translate.x, transform.translate.y));
            canvas.translate((transform.pivot.x, transform.pivot.y));
            canvas.rotate(transform.rotate, None);
            canvas.scale((transform.scale_x, transform.scale_y));
            canvas.translate((-transform.pivot.x, -transform.pivot.y));
            draw_vector(canvas, child, renderer_style, composition_width);
            canvas.restore();
        }
    }
}

fn draw_text(
    canvas: &skia_safe::Canvas,
    text: &VectorTextData,
    renderer_style: ResolvedRendererStyle,
    composition_width: f32,
) {
    let style = resolve_style(&text.style, renderer_style);
    let text_color = if style.fill_enabled {
        style.fill_color
    } else if style.stroke_enabled {
        style.stroke_color
    } else {
        [0, 0, 0, 0]
    };
    let metrics = measure_text_layout(text, composition_width);
    let mut paragraph = build_text_paragraph(text, text_color);
    paragraph.layout(metrics.layout_width);
    paragraph.paint(
        canvas,
        (-metrics.horizontal_offset, metrics.vertical_offset),
    );
}

fn measure_text_layout(text: &VectorTextData, composition_width: f32) -> TextLayoutMetrics {
    let layout_width = text
        .max_width
        .unwrap_or(composition_width)
        .clamp(1.0, u32::MAX as f32);
    let mut paragraph = build_text_paragraph(text, [255, 255, 255, 255]);
    paragraph.layout(layout_width);

    let rendered_width = if text.max_width.is_some() {
        paragraph.longest_line()
    } else {
        paragraph.max_intrinsic_width()
    }
    .max(1.0)
    .min(layout_width);
    let horizontal_offset = match text.alignment.horizontal {
        crate::node::source::text::TextAlignmentHorizontal::Left
        | crate::node::source::text::TextAlignmentHorizontal::Justify => 0.0,
        crate::node::source::text::TextAlignmentHorizontal::Center => {
            ((layout_width - rendered_width) * 0.5).max(0.0)
        }
        crate::node::source::text::TextAlignmentHorizontal::Right => {
            (layout_width - rendered_width).max(0.0)
        }
    };
    let rendered_height = paragraph.height().ceil().max(1.0);
    let vertical_offset = match text.alignment.vertical {
        crate::node::source::text::TextAlignmentVertical::Top => 0.0,
        crate::node::source::text::TextAlignmentVertical::Middle => {
            (rendered_height - paragraph.height()).max(0.0) * 0.5
        }
        crate::node::source::text::TextAlignmentVertical::Bottom => {
            (rendered_height - paragraph.height()).max(0.0)
        }
    };

    TextLayoutMetrics {
        rendered_width: rendered_width.ceil().max(1.0) as u32,
        rendered_height: rendered_height as u32,
        layout_width,
        horizontal_offset,
        vertical_offset,
    }
}

fn build_text_paragraph(
    text: &VectorTextData,
    text_color: [u8; 4],
) -> skia_safe::textlayout::Paragraph {
    let text_style = TextLayoutStyle::new(
        text.font_family.clone(),
        text.font_size,
        i32::from(text.font_weight),
        text.font_style,
    );
    build_paragraph(
        &text.content,
        &text_style,
        text_color,
        text.alignment.horizontal,
    )
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

fn draw_padding(style: ResolvedVectorStyle) -> f32 {
    let stroke_pad = if style.stroke_enabled {
        style.stroke_width.max(0.0) * 0.5
    } else {
        0.0
    };
    1.0 + stroke_pad
}

#[derive(Debug, Clone, Copy)]
struct AffineTransform {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    tx: f32,
    ty: f32,
}

impl AffineTransform {
    fn identity() -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            tx: 0.0,
            ty: 0.0,
        }
    }

    fn then_translate(self, x: f32, y: f32) -> Self {
        Self {
            tx: self.tx + self.a * x + self.c * y,
            ty: self.ty + self.b * x + self.d * y,
            ..self
        }
    }

    fn then_transform(self, transform: crate::node::VectorTransformData) -> Self {
        self.then_translate(transform.translate.x, transform.translate.y)
            .then_translate(transform.pivot.x, transform.pivot.y)
            .then_rotate(transform.rotate)
            .then_scale(transform.scale_x, transform.scale_y)
            .then_translate(-transform.pivot.x, -transform.pivot.y)
    }

    fn then_scale(self, sx: f32, sy: f32) -> Self {
        Self {
            a: self.a * sx,
            b: self.b * sx,
            c: self.c * sy,
            d: self.d * sy,
            ..self
        }
    }

    fn then_rotate(self, degrees: f32) -> Self {
        let radians = degrees.to_radians();
        let (sin, cos) = radians.sin_cos();
        Self {
            a: self.a * cos + self.c * sin,
            b: self.b * cos + self.d * sin,
            c: self.c * cos - self.a * sin,
            d: self.d * cos - self.b * sin,
            ..self
        }
    }

    fn map_point(self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.tx,
            self.b * x + self.d * y + self.ty,
        )
    }
}

fn transformed_bounds(transform: AffineTransform, bounds: Rect, pad: f32) -> RectI {
    let left = bounds.left - pad;
    let top = bounds.top - pad;
    let right = bounds.right + pad;
    let bottom = bounds.bottom + pad;
    let points = [
        transform.map_point(left, top),
        transform.map_point(right, top),
        transform.map_point(left, bottom),
        transform.map_point(right, bottom),
    ];
    let min_x = points
        .iter()
        .map(|(x, _)| *x)
        .fold(f32::INFINITY, f32::min)
        .floor() as i32;
    let min_y = points
        .iter()
        .map(|(_, y)| *y)
        .fold(f32::INFINITY, f32::min)
        .floor() as i32;
    let max_x = points
        .iter()
        .map(|(x, _)| *x)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil() as i32;
    let max_y = points
        .iter()
        .map(|(_, y)| *y)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil() as i32;
    let width = (max_x - min_x).max(1) as u32;
    let height = (max_y - min_y).max(1) as u32;
    RectI::new(min_x, min_y, width, height)
}

fn transparent_frame<S: SurfacePool, M: MediaStore>(
    ctx: &mut RenderContext<'_, S, M>,
    format_rect: RectI,
) -> GpuImageFrame {
    render_to_surface_ephemeral(
        format_rect.width.max(1),
        format_rect.height.max(1),
        ctx,
        format_rect,
        format_rect,
        AlphaMode::Premultiplied,
        ClearMode::Transparent,
        |_| {},
    )
    .expect("transparent GPU image allocation")
}

fn clip_raster_to_output_rect<S: SurfacePool, M: MediaStore>(
    frame: GpuImageFrame,
    ctx: &mut RenderContext<'_, S, M>,
) -> GpuImageFrame {
    let output_rect = RectI::from_size(
        ctx.renderer.composition.render_settings.width,
        ctx.renderer.composition.render_settings.height,
    );
    let source_format = frame.format_rect();
    if source_format.x >= output_rect.x
        && source_format.y >= output_rect.y
        && source_format.right() <= output_rect.right()
        && source_format.bottom() <= output_rect.bottom()
    {
        return frame;
    }

    let Some(clipped_rect) = source_format.intersect(&output_rect) else {
        return transparent_frame(ctx, RectI::new(output_rect.x, output_rect.y, 0, 0));
    };

    if clipped_rect == source_format {
        return frame;
    }

    let source_alpha = frame.alpha_mode();
    let Some(image) = frame.to_skia_image() else {
        return transparent_frame(ctx, clipped_rect);
    };

    let crop_left = clipped_rect.x - source_format.x;
    let crop_top = clipped_rect.y - source_format.y;
    let crop_right = crop_left + clipped_rect.width as i32;
    let crop_bottom = crop_top + clipped_rect.height as i32;

    let subset_rect = IRect::from_ltrb(crop_left, crop_top, crop_right, crop_bottom);
    if let Some(cropped_image) =
        image.make_subset(None, &subset_rect, RequiredProperties::default())
    {
        let mut frame = GpuImageFrame::with_domain(
            cropped_image,
            clipped_rect.width.max(1),
            clipped_rect.height.max(1),
            clipped_rect,
            clipped_rect,
        );
        frame.alpha_mode = source_alpha;
        return frame;
    }

    render_to_surface_ephemeral(
        clipped_rect.width.max(1),
        clipped_rect.height.max(1),
        ctx,
        clipped_rect,
        clipped_rect,
        source_alpha,
        ClearMode::None,
        |canvas| {
            canvas.draw_image(&image, (-crop_left as f32, -crop_top as f32), None);
        },
    )
    .unwrap_or_else(|_| transparent_frame(ctx, clipped_rect))
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

#[derive(Debug, Clone)]
struct BuiltPath {
    path: Path,
    bounds: Rect,
}

fn build_path(geometry: &ShapeGeometry) -> BuiltPath {
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
            BuiltPath {
                path: if border_radius > 0.0 {
                    Path::rrect(RRect::new_rect_xy(rect, border_radius, border_radius), None)
                } else {
                    Path::rect(rect, None)
                },
                bounds: rect,
            }
        }
        ShapeGeometry::Ellipse { width, height } => {
            let width = (*width).max(1);
            let height = (*height).max(1);
            let bounds = Rect::from_xywh(0.0, 0.0, width as f32, height as f32);
            BuiltPath {
                path: Path::oval(bounds, None),
                bounds,
            }
        }
        ShapeGeometry::Polygon { points } => polygon_path(points),
        ShapeGeometry::Path { commands } => svg_path(commands),
    }
}

fn polygon_path(points: &[(f32, f32)]) -> BuiltPath {
    if points.is_empty() {
        return unit_path();
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
        return unit_path();
    }

    let normalized_points: Vec<skia_safe::Point> = points
        .iter()
        .map(|(x, y)| skia_safe::Point::new(*x, *y))
        .collect();

    BuiltPath {
        path: Path::polygon(&normalized_points, true, None, None),
        bounds: Rect::from_ltrb(min_x, min_y, max_x, max_y),
    }
}

fn svg_path(commands: &str) -> BuiltPath {
    let Some(path) = Path::from_svg(commands) else {
        return unit_path();
    };
    let bounds = path.compute_tight_bounds();
    if bounds.is_empty() || !bounds.is_finite() {
        return unit_path();
    }
    BuiltPath { path, bounds }
}

fn unit_path() -> BuiltPath {
    let bounds = Rect::from_xywh(0.0, 0.0, 1.0, 1.0);
    BuiltPath {
        path: Path::rect(bounds, None),
        bounds,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        composition::{Composition, RenderSettings, TimelineSettings},
        graph::Graph,
        media::{ImageResolver, MediaStore, VideoFrameResolver},
        node::{
            VectorPosition, VectorStroke, VectorTransformData,
            vector::vector_stroke_style::apply_style_defaults,
        },
        render::{
            LumenRenderer, RenderContext,
            surface::{SurfacePool, SurfacePoolStats},
        },
    };

    #[derive(Debug)]
    struct TestSurfacePool;

    impl SurfacePool for TestSurfacePool {
        fn with_surface<T>(
            &self,
            width: u32,
            height: u32,
            f: impl FnOnce(&mut skia_safe::Surface) -> crate::Result<T>,
        ) -> crate::Result<T> {
            let mut surface =
                skia_safe::surfaces::raster_n32_premul((width.max(1) as i32, height.max(1) as i32))
                    .ok_or(crate::error::RenderError::SurfaceAllocation { width, height })?;
            f(&mut surface)
        }

        fn stats(&self) -> SurfacePoolStats {
            SurfacePoolStats::default()
        }

        fn flush(&self) {}
    }

    #[derive(Debug)]
    struct NullMediaStore;

    impl MediaStore for NullMediaStore {
        fn get_image_resolver(&self, _source: &str) -> Option<Box<dyn ImageResolver>> {
            None
        }

        fn get_video_resolver(&self, _stream_id: &str) -> Option<Box<dyn VideoFrameResolver>> {
            None
        }
    }

    fn render_vector(vector: VectorData) -> GpuImageFrame {
        let composition = Composition::new(
            Graph::new(),
            TimelineSettings {
                fps: 30.0,
                duration_frames: 60,
            },
            RenderSettings {
                width: 32,
                height: 32,
                background_color: [0, 0, 0, 0],
            },
        );
        let pool = TestSurfacePool;
        let media = NullMediaStore;
        let renderer = LumenRenderer::new(&composition, &pool, &media).unwrap();
        let mut ctx = RenderContext::new(&renderer, 0);
        rasterize_vector(&vector, &ShapeRenderer::default(), &mut ctx)
    }

    fn read_pixels(frame: &GpuImageFrame) -> Vec<u8> {
        let (w, h) = frame.storage_dimensions();
        let mut pixels = vec![0; w as usize * h as usize * 4];
        frame
            .read_pixels_into(&mut pixels, w as usize * 4)
            .expect("read pixels");
        pixels
    }

    #[test]
    fn transformed_vector_renders_scaled_offset_pixels() {
        let vector = VectorData::Transformed {
            child: Box::new(VectorData::Shape {
                geometry: ShapeGeometry::Rectangle {
                    width: 4,
                    height: 3,
                    border_radius: 0.0,
                },
                style: VectorStyle {
                    color: Some([200, 10, 20, 255]),
                    stroke: None,
                },
                position: VectorPosition::default(),
            }),
            transform: VectorTransformData {
                translate: VectorPosition { x: 5.0, y: 6.0 },
                scale_x: 2.0,
                scale_y: 2.0,
                rotate: 0.0,
                pivot: VectorPosition::default(),
            },
        };

        let frame = render_vector(vector);
        let rect = frame.format_rect();
        assert!(rect.x <= 5);
        assert!(rect.y <= 6);
        assert!(rect.width >= 8);
        assert!(rect.height >= 6);

        let red_pixels = read_pixels(&frame)
            .chunks_exact(4)
            .filter(|px| px[0] > 150 && px[1] < 40 && px[2] < 50 && px[3] > 200)
            .count();
        assert!(red_pixels >= 20, "red_pixels={red_pixels}");
    }

    #[test]
    fn path_preserves_negative_and_non_zero_local_coordinates() {
        let vector = VectorData::Shape {
            geometry: ShapeGeometry::Path {
                commands: "M -2 3 L 4 3 L 4 7 L -2 7 Z".to_string(),
            },
            style: VectorStyle {
                color: Some([10, 180, 40, 255]),
                stroke: Some(VectorStroke {
                    color: [0, 0, 0, 255],
                    width: 0.0,
                }),
            },
            position: VectorPosition { x: 6.0, y: 5.0 },
        };

        let frame = render_vector(vector);
        let rect = frame.format_rect();
        assert_eq!(rect.x, 3);
        assert_eq!(rect.y, 7);
        assert!(rect.width >= 8);
        assert!(rect.height >= 6);

        let green_pixels = read_pixels(&frame)
            .chunks_exact(4)
            .filter(|px| px[0] < 40 && px[1] > 120 && px[2] < 80 && px[3] > 200)
            .count();
        assert!(green_pixels >= 12, "green_pixels={green_pixels}");
    }

    #[test]
    fn stroke_style_defaults_render_fill_pixels() {
        let vector = VectorData::Shape {
            geometry: ShapeGeometry::Rectangle {
                width: 5,
                height: 5,
                border_radius: 0.0,
            },
            style: VectorStyle::default(),
            position: VectorPosition { x: 2.0, y: 2.0 },
        };
        let styled = apply_style_defaults(
            vector,
            &VectorStyle {
                color: Some([20, 40, 220, 255]),
                stroke: None,
            },
            false,
        );

        let frame = render_vector(styled);
        let blue_pixels = read_pixels(&frame)
            .chunks_exact(4)
            .filter(|px| px[0] < 50 && px[1] < 70 && px[2] > 180 && px[3] > 200)
            .count();
        assert!(blue_pixels >= 16, "blue_pixels={blue_pixels}");
    }
}
