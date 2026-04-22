use std::cell::OnceCell;

#[cfg(feature = "embed-roboto")]
use skia_safe::textlayout::TypefaceFontProvider;
use skia_safe::{
    FontMgr, FontStyle, IRect, Paint, PaintStyle, Path, RRect, Rect,
    font_style::Weight,
    image::RequiredProperties,
    textlayout::{
        FontCollection, ParagraphBuilder, ParagraphStyle, TextAlign as ParagraphTextAlign,
        TextStyle as ParagraphTextStyle,
    },
};

use crate::{
    media::MediaStore,
    node::{
        NodeId, NodeProperty, PortRef,
        pixel_utils::{ClearMode, render_to_surface_ephemeral, to_skia_color},
        vector::{ShapeGeometry, VectorData, VectorPosition, VectorStyle, VectorTextData},
    },
    raster::{AlphaMode, ImageFrame, RasterFrame, RectI},
    render::{RenderContext, surface::SurfacePool},
};
use lumen_macros::{Node, node_impl};

#[cfg(feature = "embed-roboto")]
const EMBEDDED_ROBOTO_REGULAR: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/roboto/Roboto-Regular.ttf"
));

thread_local! {
    static VECTOR_TEXT_FONT_COLLECTION: OnceCell<FontCollection> = const { OnceCell::new() };
}

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
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<RasterFrame> {
        let vector = ctx.eval(&self.vector)?;
        let vector_data = match vector.as_ref() {
            crate::node::NodeResult::Raster(_) => {
                return Err(ctx.invalid_node_output_type(self.vector.id, "Vector", "RasterFrame"));
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
) -> RasterFrame {
    match resolve_renderer_style(renderer, ctx) {
        Ok(renderer_style) => rasterize_vector_with_style(vector, renderer_style, ctx),
        Err(_) => rasterize_vector_with_style(vector, ResolvedRendererStyle::default(), ctx),
    }
}

fn rasterize_vector_with_style<S: SurfacePool, M: MediaStore>(
    vector: &VectorData,
    renderer_style: ResolvedRendererStyle,
    ctx: &mut RenderContext<'_, S, M>,
) -> RasterFrame {
    let composition_width = ctx.renderer.composition.render_settings.width as f32;
    let Some(bounds) = measure_vector_bounds(
        vector,
        renderer_style,
        composition_width,
        VectorPosition::default(),
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
            draw_vector(
                canvas,
                vector,
                renderer_style,
                composition_width,
                VectorPosition {
                    x: -(bounds.x as f32),
                    y: -(bounds.y as f32),
                },
            );
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

fn vector_text_font_collection() -> FontCollection {
    VECTOR_TEXT_FONT_COLLECTION.with(|cell| {
        cell.get_or_init(|| {
            let font_mgr = FontMgr::default();
            new_vector_text_font_collection(&font_mgr)
        })
        .clone()
    })
}

fn new_vector_text_font_collection(default_font_mgr: &FontMgr) -> FontCollection {
    let mut font_collection = FontCollection::new();
    font_collection.set_default_font_manager(default_font_mgr.clone(), None);
    #[cfg(feature = "embed-roboto")]
    attach_embedded_roboto(&mut font_collection, default_font_mgr);
    font_collection
}

#[cfg(feature = "embed-roboto")]
fn attach_embedded_roboto(font_collection: &mut FontCollection, default_font_mgr: &FontMgr) {
    let Some(roboto_typeface) = default_font_mgr.new_from_data(EMBEDDED_ROBOTO_REGULAR, None)
    else {
        return;
    };

    let mut provider = TypefaceFontProvider::new();
    provider.register_typeface(roboto_typeface, Some("Roboto"));
    font_collection.set_asset_font_manager(Some(provider.into()));
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

fn to_slant(style: crate::node::source::text::TextFontStyle) -> skia_safe::font_style::Slant {
    match style {
        crate::node::source::text::TextFontStyle::Normal => skia_safe::font_style::Slant::Upright,
        crate::node::source::text::TextFontStyle::Italic => skia_safe::font_style::Slant::Italic,
        crate::node::source::text::TextFontStyle::Oblique => skia_safe::font_style::Slant::Oblique,
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
    translation: VectorPosition,
) -> Option<RectI> {
    match vector {
        VectorData::Shape {
            geometry,
            style,
            position,
        } => {
            let style = resolve_style(style, renderer_style);
            let (_, width, height) = build_path(geometry);
            Some(
                positioned_bounds(
                    width.max(1),
                    height.max(1),
                    add_positions(translation, *position),
                    draw_padding(style),
                )
                .format_rect,
            )
        }
        VectorData::Text(text) => {
            let style = resolve_style(&text.style, renderer_style);
            let metrics = measure_text_layout(text, composition_width);
            Some(
                positioned_bounds(
                    metrics.rendered_width.max(1),
                    metrics.rendered_height.max(1),
                    add_positions(translation, text.position),
                    draw_padding(style),
                )
                .format_rect,
            )
        }
        VectorData::Group { children, position } => children
            .iter()
            .filter_map(|child| {
                measure_vector_bounds(
                    child,
                    renderer_style,
                    composition_width,
                    add_positions(translation, *position),
                )
            })
            .reduce(union_rect),
    }
}

fn draw_vector(
    canvas: &skia_safe::Canvas,
    vector: &VectorData,
    renderer_style: ResolvedRendererStyle,
    composition_width: f32,
    translation: VectorPosition,
) {
    match vector {
        VectorData::Shape {
            geometry,
            style,
            position,
        } => {
            let style = resolve_style(style, renderer_style);
            let (path, _, _) = build_path(geometry);
            let position = add_positions(translation, *position);
            canvas.save();
            canvas.translate((position.x, position.y));
            draw_shape(canvas, &path, style);
            canvas.restore();
        }
        VectorData::Text(text) => {
            draw_text(
                canvas,
                text,
                renderer_style,
                composition_width,
                add_positions(translation, text.position),
            );
        }
        VectorData::Group { children, position } => {
            let translation = add_positions(translation, *position);
            for child in children {
                draw_vector(
                    canvas,
                    child,
                    renderer_style,
                    composition_width,
                    translation,
                );
            }
        }
    }
}

fn draw_text(
    canvas: &skia_safe::Canvas,
    text: &VectorTextData,
    renderer_style: ResolvedRendererStyle,
    composition_width: f32,
    position: VectorPosition,
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
    let mut paragraph = build_text_paragraph(text, text_color, metrics.layout_width);
    paragraph.layout(metrics.layout_width);
    paragraph.paint(
        canvas,
        (
            position.x - metrics.horizontal_offset,
            position.y + metrics.vertical_offset,
        ),
    );
}

fn measure_text_layout(text: &VectorTextData, composition_width: f32) -> TextLayoutMetrics {
    let layout_width = text
        .max_width
        .unwrap_or(composition_width)
        .clamp(1.0, u32::MAX as f32);
    let mut paragraph = build_text_paragraph(text, [255, 255, 255, 255], layout_width);
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
    layout_width: f32,
) -> skia_safe::textlayout::Paragraph {
    let mut paragraph_style = ParagraphStyle::new();
    paragraph_style.set_text_align(match text.alignment.horizontal {
        crate::node::source::text::TextAlignmentHorizontal::Left => ParagraphTextAlign::Left,
        crate::node::source::text::TextAlignmentHorizontal::Center => ParagraphTextAlign::Center,
        crate::node::source::text::TextAlignmentHorizontal::Right => ParagraphTextAlign::Right,
        crate::node::source::text::TextAlignmentHorizontal::Justify => ParagraphTextAlign::Justify,
    });

    let mut text_style = ParagraphTextStyle::new();
    text_style.set_font_size(text.font_size.max(1.0));
    text_style.set_color(to_skia_color(text_color));
    text_style.set_font_style(FontStyle::new(
        Weight::from(i32::from(text.font_weight.clamp(100, 900))),
        skia_safe::font_style::Width::NORMAL,
        to_slant(text.font_style),
    ));

    let requested_font_family = text.font_family.trim();
    if requested_font_family.is_empty() {
        #[cfg(feature = "embed-roboto")]
        text_style.set_font_families(&["Roboto", "sans-serif"]);
        #[cfg(not(feature = "embed-roboto"))]
        text_style.set_font_families(&["sans-serif"]);
    } else {
        #[cfg(feature = "embed-roboto")]
        text_style.set_font_families(&[requested_font_family, "Roboto", "sans-serif"]);
        #[cfg(not(feature = "embed-roboto"))]
        text_style.set_font_families(&[requested_font_family, "sans-serif"]);
    }

    paragraph_style.set_text_style(&text_style);
    let font_collection = vector_text_font_collection();
    let mut builder = ParagraphBuilder::new(&paragraph_style, font_collection);
    builder.push_style(&text_style);
    builder.add_text(&text.content);
    let mut paragraph = builder.build();
    paragraph.layout(layout_width);
    paragraph
}

fn add_positions(left: VectorPosition, right: VectorPosition) -> VectorPosition {
    VectorPosition {
        x: left.x + right.x,
        y: left.y + right.y,
    }
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
        format_rect: RectI::new(min_x, min_y, width, height),
    }
}

fn transparent_frame<S: SurfacePool, M: MediaStore>(
    ctx: &mut RenderContext<'_, S, M>,
    format_rect: RectI,
) -> RasterFrame {
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
    .unwrap_or_else(|_| {
        ImageFrame::new(
            skia_safe::surfaces::raster_n32_premul((1, 1))
                .expect("1x1 raster surface")
                .image_snapshot(),
        )
    })
}

fn clip_raster_to_output_rect<S: SurfacePool, M: MediaStore>(
    frame: RasterFrame,
    ctx: &mut RenderContext<'_, S, M>,
) -> RasterFrame {
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
        let mut frame = ImageFrame::with_domain(
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
