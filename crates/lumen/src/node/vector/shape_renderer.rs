use std::cell::RefCell;

#[cfg(feature = "embed-roboto")]
use skia_safe::textlayout::TypefaceFontProvider;
use skia_safe::{
    FontMgr, FontStyle, Paint, PaintStyle, Path, RRect, Rect,
    font_style::Weight,
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

thread_local! {
    static VECTOR_TEXT_FONT_MGR: RefCell<Option<FontMgr>> = const { RefCell::new(None) };
    static VECTOR_TEXT_FONT_COLLECTION: RefCell<Option<FontCollection>> = const { RefCell::new(None) };
}

#[cfg(feature = "embed-roboto")]
const EMBEDDED_ROBOTO_REGULAR: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/roboto/Roboto-Regular.ttf"
));

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
        let vector = ctx.eval(self.vector.clone())?;
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
        Ok(rasterize_vector_with_style(
            vector_data,
            renderer_style,
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
    match vector {
        VectorData::Shape {
            geometry,
            style,
            position,
        } => rasterize_geometry(geometry, *position, style, renderer_style, ctx),
        VectorData::Text(text) => rasterize_text(text, renderer_style, ctx),
        VectorData::Group { children, position } => {
            rasterize_group(children, *position, renderer_style, ctx)
        }
    }
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

fn with_vector_text_font_mgr<R>(f: impl FnOnce(&FontMgr) -> R) -> R {
    VECTOR_TEXT_FONT_MGR.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let mgr = borrow.get_or_insert_with(FontMgr::default);
        f(mgr)
    })
}

fn with_vector_text_font_collection<R>(f: impl FnOnce(FontCollection) -> R) -> R {
    VECTOR_TEXT_FONT_COLLECTION.with(|cell| {
        if cell.borrow().is_none() {
            let font_collection = with_vector_text_font_mgr(new_vector_text_font_collection);
            *cell.borrow_mut() = Some(font_collection);
        }

        let font_collection = cell
            .borrow()
            .as_ref()
            .expect("vector text font collection should be initialized")
            .clone();

        f(font_collection)
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

fn rasterize_geometry<S: SurfacePool, M: MediaStore>(
    geometry: &ShapeGeometry,
    position: VectorPosition,
    style: &VectorStyle,
    renderer_style: ResolvedRendererStyle,
    ctx: &mut RenderContext<'_, S, M>,
) -> RasterFrame {
    let style = resolve_style(style, renderer_style);
    let (path, width, height) = build_path(geometry);
    let width = width.max(1);
    let height = height.max(1);
    let pad = draw_padding(style);
    let bounds = positioned_bounds(width, height, position, pad);
    render_to_surface_ephemeral(
        bounds.width,
        bounds.height,
        ctx,
        bounds.format_rect,
        bounds.format_rect,
        AlphaMode::Premultiplied,
        ClearMode::Transparent,
        |canvas| {
            canvas.save();
            canvas.translate((bounds.draw_x, bounds.draw_y));
            draw_shape(canvas, &path, style);
            canvas.restore();
        },
    )
    .unwrap_or_else(|_| {
        RasterFrame::transparent(
            1,
            1,
            RectI::from_size(1, 1),
            RectI::from_size(1, 1),
            AlphaMode::Premultiplied,
        )
        .unwrap_or_else(|_| {
            RasterFrame::Image(ImageFrame::new(
                skia_safe::surfaces::raster_n32_premul((1, 1))
                    .expect("1x1 raster surface")
                    .image_snapshot(),
            ))
        })
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

fn rasterize_text<S: SurfacePool, M: MediaStore>(
    text: &VectorTextData,
    renderer_style: ResolvedRendererStyle,
    ctx: &mut RenderContext<'_, S, M>,
) -> RasterFrame {
    let style = resolve_style(&text.style, renderer_style);
    let text_color = if style.fill_enabled {
        style.fill_color
    } else if style.stroke_enabled {
        // Skia paragraph text is currently rasterized as fill only here.
        // If no fill is specified, fall back to the resolved stroke color.
        style.stroke_color
    } else {
        [0, 0, 0, 0]
    };

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
    let layout_width = text
        .max_width
        .unwrap_or(ctx.renderer.composition.render_settings.width as f32)
        .clamp(1.0, u32::MAX as f32);
    let paragraph = with_vector_text_font_collection(|font_collection| {
        let mut builder = ParagraphBuilder::new(&paragraph_style, font_collection);
        builder.push_style(&text_style);
        builder.add_text(&text.content);
        let mut paragraph = builder.build();
        paragraph.layout(layout_width);
        paragraph
    });

    let width = layout_width.ceil().max(1.0) as u32;
    let height = paragraph.height().ceil().max(1.0) as u32;
    let vertical_offset = match text.alignment.vertical {
        crate::node::source::text::TextAlignmentVertical::Top => 0.0,
        crate::node::source::text::TextAlignmentVertical::Middle => {
            (height as f32 - paragraph.height()).max(0.0) * 0.5
        }
        crate::node::source::text::TextAlignmentVertical::Bottom => {
            (height as f32 - paragraph.height()).max(0.0)
        }
    };
    let frame = render_to_surface_ephemeral(
        width,
        height,
        ctx,
        RectI::from_size(width, height),
        RectI::from_size(width, height),
        AlphaMode::Premultiplied,
        ClearMode::Transparent,
        |canvas| {
            paragraph.paint(canvas, (0.0, vertical_offset));
        },
    )
    .unwrap_or_else(|_| transparent_frame(RectI::from_size(1, 1)));

    let (text_w, text_h) = frame.dimensions();
    let pad = draw_padding(style);
    let bounds = positioned_bounds(text_w.max(1), text_h.max(1), text.position, pad);
    offset_raster_into_bounds(frame, &bounds, ctx)
}

fn to_slant(style: crate::node::source::text::TextFontStyle) -> skia_safe::font_style::Slant {
    match style {
        crate::node::source::text::TextFontStyle::Normal => skia_safe::font_style::Slant::Upright,
        crate::node::source::text::TextFontStyle::Italic => skia_safe::font_style::Slant::Italic,
        crate::node::source::text::TextFontStyle::Oblique => skia_safe::font_style::Slant::Oblique,
    }
}

fn rasterize_group<S: SurfacePool, M: MediaStore>(
    children: &[VectorData],
    group_position: VectorPosition,
    renderer_style: ResolvedRendererStyle,
    ctx: &mut RenderContext<'_, S, M>,
) -> RasterFrame {
    let mut layers = Vec::with_capacity(children.len());
    for child in children {
        layers.push(rasterize_vector_with_style(child, renderer_style, ctx));
    }

    if layers.is_empty() {
        return transparent_frame(RectI::from_size(1, 1));
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

    render_to_surface_ephemeral(
        union.width.max(1),
        union.height.max(1),
        ctx,
        translated_union,
        translated_union,
        AlphaMode::Premultiplied,
        ClearMode::Transparent,
        |canvas| {
            for layer in &layers {
                let Some(image) = layer.to_skia_image() else {
                    continue;
                };
                let layer_rect = layer.format_rect();
                let offset_x = (layer_rect.x - union.x) as f32;
                let offset_y = (layer_rect.y - union.y) as f32;
                canvas.draw_image(&image, (offset_x, offset_y), None);
            }
        },
    )
    .unwrap_or_else(|_| transparent_frame(translated_union))
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

fn transparent_frame(format_rect: RectI) -> RasterFrame {
    RasterFrame::transparent(
        format_rect.width,
        format_rect.height,
        format_rect,
        format_rect,
        AlphaMode::Premultiplied,
    )
    .unwrap_or_else(|_| {
        RasterFrame::Image(ImageFrame::new(
            skia_safe::surfaces::raster_n32_premul((1, 1))
                .expect("1x1 raster surface")
                .image_snapshot(),
        ))
    })
}

fn offset_raster_into_bounds<S: SurfacePool, M: MediaStore>(
    frame: RasterFrame,
    bounds: &PositionedRasterBounds,
    ctx: &mut RenderContext<'_, S, M>,
) -> RasterFrame {
    let Some((image, width, height)) = frame.image_parts() else {
        return transparent_frame(RectI::from_size(0, 0));
    };
    if width == 0 || height == 0 {
        return transparent_frame(RectI::from_size(0, 0));
    }

    render_to_surface_ephemeral(
        bounds.width,
        bounds.height,
        ctx,
        bounds.format_rect,
        bounds.format_rect,
        frame.alpha_mode(),
        ClearMode::Transparent,
        |canvas| {
            canvas.draw_image(&image, (bounds.draw_x, bounds.draw_y), None);
        },
    )
    .unwrap_or_else(|_| transparent_frame(bounds.format_rect))
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
