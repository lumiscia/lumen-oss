use cosmic_text::CacheKey;
use fdsm::{
    shape::{Contour, Shape},
    transform::Transform,
};
use nalgebra::{Affine2, Matrix3};
use skrifa::{FontRef, GlyphId, MetadataProvider, prelude::Size, raw::TableProvider};

use crate::{AtlasConfig, GpuMsdfSegment};

const MSDF_SEGMENT_LINE: u32 = 0;
const MSDF_SEGMENT_QUAD: u32 = 1;
const MSDF_SEGMENT_CUBIC: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MsdfGlyphPlacement {
    pub left: f32,
    pub top: f32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct MsdfGlyphJob {
    pub placement: MsdfGlyphPlacement,
    pub segments: Vec<GpuMsdfSegment>,
}

struct MsdfGlyphMetrics {
    placement: MsdfGlyphPlacement,
    scale: f64,
    range: f64,
    bbox_x_min: f32,
    bbox_y_max: f32,
}

pub fn generate_msdf_job(
    font_data: &[u8],
    face_index: u32,
    key: CacheKey,
    config: AtlasConfig,
) -> Option<MsdfGlyphJob> {
    let font = FontRef::from_index(font_data, face_index).ok()?;
    let axes = font
        .axes()
        .location(std::iter::empty::<skrifa::setting::VariationSetting>());
    let glyph_id = GlyphId::from(key.glyph_id);
    let metrics = glyph_metrics(&font, &axes, glyph_id, key, config)?;
    let shape = Shape::edge_coloring_simple(
        load_transformed_shape(&font, &axes, glyph_id, &metrics)?,
        0.03,
        u64::from(key.glyph_id),
    );
    let mut segments = Vec::new();
    for contour in shape.contours {
        for colored_segment in contour.segments {
            let segment = colored_segment.segment;
            let order = segment.order();
            let kind = match order {
                fdsm::bezier::Order::Linear => MSDF_SEGMENT_LINE,
                fdsm::bezier::Order::Quadratic => MSDF_SEGMENT_QUAD,
                fdsm::bezier::Order::Cubic => MSDF_SEGMENT_CUBIC,
            };
            let p0 = segment.control_point(0);
            let p1 = segment.control_point(1);
            let p2 = if kind >= MSDF_SEGMENT_QUAD {
                segment.control_point(2)
            } else {
                p1
            };
            let p3 = if kind == MSDF_SEGMENT_CUBIC {
                segment.control_point(3)
            } else {
                p2
            };
            segments.push(GpuMsdfSegment {
                p0: [p0.x as f32, p0.y as f32],
                p1: [p1.x as f32, p1.y as f32],
                p2: [p2.x as f32, p2.y as f32],
                p3: [p3.x as f32, p3.y as f32],
                kind,
                channels: u32::from(colored_segment.color.value()),
                _padding: [0; 2],
            });
        }
    }
    (!segments.is_empty()).then_some(MsdfGlyphJob {
        placement: metrics.placement,
        segments,
    })
}

fn glyph_metrics(
    font: &FontRef<'_>,
    axes: &skrifa::instance::Location,
    glyph_id: GlyphId,
    key: CacheKey,
    config: AtlasConfig,
) -> Option<MsdfGlyphMetrics> {
    let bbox = font
        .glyph_metrics(Size::unscaled(), axes)
        .bounds(glyph_id)?;
    if bbox.x_min >= bbox.x_max || bbox.y_min >= bbox.y_max {
        return None;
    }

    let font_size = f32::from_bits(key.font_size_bits).max(1.0) as f64;
    let units_per_em = f64::from(font.head().ok()?.units_per_em());
    if units_per_em <= 0.0 {
        return None;
    }
    let scale = font_size / units_per_em;
    let range = f64::from(config.px_range.max(1));
    let left = f64::from(bbox.x_min) * scale - range;
    let top = f64::from(bbox.y_max) * scale + range;
    let width = ((f64::from(bbox.x_max - bbox.x_min) * scale) + 2.0 * range).ceil() as u32;
    let height = ((f64::from(bbox.y_max - bbox.y_min) * scale) + 2.0 * range).ceil() as u32;
    if width == 0 || height == 0 || width > config.width || height > config.height {
        return None;
    }

    Some(MsdfGlyphMetrics {
        placement: MsdfGlyphPlacement {
            left: left as f32,
            top: top as f32,
            width,
            height,
        },
        scale,
        range,
        bbox_x_min: bbox.x_min,
        bbox_y_max: bbox.y_max,
    })
}

fn load_transformed_shape(
    font: &FontRef<'_>,
    axes: &skrifa::instance::Location,
    glyph_id: GlyphId,
    metrics: &MsdfGlyphMetrics,
) -> Option<Shape<Contour>> {
    let (mut shape, _) = fdsm_skrifa::load_shape_from_face(font, glyph_id, axes).ok()?;
    shape.transform(&Affine2::from_matrix_unchecked(Matrix3::new(
        metrics.scale,
        0.0,
        metrics.range - f64::from(metrics.bbox_x_min) * metrics.scale,
        0.0,
        -metrics.scale,
        metrics.range + f64::from(metrics.bbox_y_max) * metrics.scale,
        0.0,
        0.0,
        1.0,
    )));
    Some(shape)
}
