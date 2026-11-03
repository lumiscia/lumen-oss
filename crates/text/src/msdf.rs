use cosmic_text::{CacheKey, CacheKeyFlags, FontSystem, SubpixelBin, SwashCache};
use fdsm::{
    bezier::{Point, Segment, scanline::FillRule},
    correct_error::{ErrorCorrectionConfig, correct_error_msdf},
    generate::{generate_msdf, generate_sdf},
    render::{correct_sign_msdf, correct_sign_sdf},
    shape::{Contour, Shape},
};
use image::{ImageBuffer, Luma, Rgb};
use swash::zeno::{Command, Vector};

use crate::{GLYPH_MODE_MSDF, GlyphPixels, MSDF_PX_RANGE, RenderedGlyph, msdf_em_size};

/// The key that an MSDF field is generated and cached for, shared by every size
/// that [`msdf_em_size`] gives the same resolution.
pub(crate) fn msdf_key(key: CacheKey) -> CacheKey {
    CacheKey {
        font_size_bits: msdf_em_size(f32::from_bits(key.font_size_bits)).to_bits(),
        x_bin: SubpixelBin::Zero,
        y_bin: SubpixelBin::Zero,
        flags: (key.flags & CacheKeyFlags::FAKE_ITALIC) | CacheKeyFlags::DISABLE_HINTING,
        ..key
    }
}

/// The highest range tier. Its field's range, 12288 texels, is wider than the largest atlas, so
/// strokes that need more are dropped as too large.
const MAX_STROKE_TIER: u32 = 10;
const _: () = assert!(MAX_STROKE_TIER <= crate::GLYPH_RANGE_MASK);

/// The distance range in texels of a stroke field of range `tier`.
fn stroke_range(tier: u32) -> f32 {
    2.0 * MSDF_PX_RANGE * (1 << tier) as f32
}

/// Field texels a stroke may reach from the outline in a field of range `tier`. It leaves the
/// widest antialiasing, half of `MSDF_PX_RANGE`, and then a texel and a half, past the farthest
/// texel that filtering blends, √2 texels away, so no texel that a covered pixel reads
/// saturates; keep in sync with `text.wgsl`.
fn field_reach(tier: u32) -> f32 {
    stroke_range(tier) / 2.0 - MSDF_PX_RANGE / 2.0 - 1.5
}

/// The key of the true distance field that strokes `key`'s outline `half_width` pixels to
/// each side, and its range tier. It has [`msdf_key`]'s resolution, so its outline is sampled
/// as finely as the fill's and animating the width never moves its texels. Its range doubles
/// until the stroke reaches no more than [`field_reach`] texels.
pub(crate) fn stroke_key(key: CacheKey, half_width: f32) -> (CacheKey, u32) {
    let font_size = f32::from_bits(key.font_size_bits);
    let em = msdf_em_size(font_size);
    let mut tier = 0;
    while tier < MAX_STROKE_TIER && half_width * em / font_size > field_reach(tier) {
        tier += 1;
    }
    (msdf_key(key), tier)
}

/// Generates the field for a key from [`msdf_key`], or with a `stroke_tier` the field of that
/// range tier for a key from [`stroke_key`]. That holds the true distance to the outline in 16
/// bits split across red and green, which the text shader decodes before filtering.
/// The outline comes from the same Swash scaler as raster glyphs, so variable-font weight
/// and fake italic match. Colour, bitmap and overlapping-contour glyphs are left to raster,
/// and a field wider or taller than `max_size` is reported before it is generated.
pub(crate) fn render_msdf_glyph(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    key: CacheKey,
    stroke_tier: Option<u32>,
    max_size: u32,
) -> RenderedGlyph {
    let Some(font) = font_system.get_font(key.font_id, key.font_weight) else {
        return RenderedGlyph::RasterOnly;
    };
    if has_colour_tables(&font) {
        return RenderedGlyph::RasterOnly;
    }
    let Some(commands) = swash_cache.get_outline_commands_uncached(font_system, key) else {
        return RenderedGlyph::RasterOnly;
    };
    let Some([min_x, min_y, max_x, max_y]) = outline_bounds(&commands) else {
        return RenderedGlyph::Empty;
    };

    let range = stroke_tier.map_or(MSDF_PX_RANGE, stroke_range);
    // Field texels kept around the outline: half the range plus one texel that the drawn quad
    // insets by half a texel.
    let padding = range / 2.0 + 1.0;
    let left = (min_x - padding).floor();
    let top = (max_y + padding).ceil();
    let width = ((max_x + padding).ceil() - left) as u32;
    let height = (top - (min_y - padding).floor()) as u32;
    let contours = contours(&commands, |p| {
        Point::new(f64::from(p.x - left), f64::from(top - p.y))
    });
    if contours_overlap(&contours) {
        return RenderedGlyph::RasterOnly;
    }
    if width > max_size || height > max_size {
        return RenderedGlyph::Oversized;
    }

    let range = f64::from(range);
    let texel = |value: f32| (value * 255.0).round().clamp(0.0, 255.0) as u8;
    if stroke_tier.is_some() {
        let shape = Shape { contours };
        let prepared = shape.prepare();
        let mut field = ImageBuffer::<Luma<f32>, _>::new(width, height);
        generate_sdf(&prepared, range, &mut field);
        correct_sign_sdf(&mut field, &prepared, FillRule::Nonzero);
        return RenderedGlyph::Pixels(GlyphPixels {
            size: [width, height],
            left,
            top,
            mode: GLYPH_MODE_MSDF,
            pixels: field
                .pixels()
                .flat_map(|pixel| {
                    let distance = (pixel.0[0] * 65535.0).round().clamp(0.0, 65535.0) as u16;
                    let [high, low] = distance.to_be_bytes();
                    [high, low, 0, 255]
                })
                .collect(),
        });
    }
    let shape = Shape::edge_coloring_simple(Shape { contours }, 0.03, 0);
    let prepared = shape.prepare();
    let mut field = ImageBuffer::<Rgb<f32>, _>::new(width, height);
    generate_msdf(&prepared, range, &mut field);
    correct_error_msdf(
        &mut field,
        &shape,
        &prepared,
        range,
        &ErrorCorrectionConfig::default(),
    );
    correct_sign_msdf(&mut field, &prepared, FillRule::Nonzero);

    let pixels = field
        .pixels()
        .flat_map(|pixel| {
            let [r, g, b] = pixel.0.map(texel);
            [r, g, b, 255]
        })
        .collect();
    RenderedGlyph::Pixels(GlyphPixels {
        size: [width, height],
        left,
        top,
        mode: GLYPH_MODE_MSDF,
        pixels,
    })
}

/// Whether `font` holds colour or bitmap glyphs, which are drawn from those tables rather than
/// their outlines.
pub(crate) fn has_colour_tables(font: &cosmic_text::Font) -> bool {
    [b"COLR", b"CBDT", b"sbix", b"EBDT"]
        .iter()
        .any(|tag| font.as_swash().table(u32::from_be_bytes(**tag)).is_some())
}

/// `[min_x, min_y, max_x, max_y]` of an outline's control points, or `None` when it is empty.
pub(crate) fn outline_bounds(commands: &[Command]) -> Option<[f32; 4]> {
    let mut bounds: Option<[f32; 4]> = None;
    let mut include = |point: Vector| {
        bounds = Some(match bounds {
            Some([min_x, min_y, max_x, max_y]) => [
                min_x.min(point.x),
                min_y.min(point.y),
                max_x.max(point.x),
                max_y.max(point.y),
            ],
            None => [point.x, point.y, point.x, point.y],
        });
    };
    for command in commands {
        match *command {
            Command::MoveTo(p) | Command::LineTo(p) => include(p),
            Command::QuadTo(c, p) => {
                include(c);
                include(p);
            }
            Command::CurveTo(c0, c1, p) => {
                include(c0);
                include(c1);
                include(p);
            }
            Command::Close => {}
        }
    }
    bounds
}

/// Splits Swash outline commands into closed contours, mapping each point.
pub(crate) fn contours(commands: &[Command], map: impl Fn(Vector) -> Point) -> Vec<Contour> {
    let mut contours = Vec::new();
    let mut segments = Vec::new();
    let mut start = Point::origin();
    let mut current = start;
    let mut close = |segments: &mut Vec<Segment>, current: Point, start: Point| {
        if current != start {
            segments.push(Segment::line(current, start));
        }
        if !segments.is_empty() {
            contours.push(Contour {
                segments: std::mem::take(segments),
            });
        }
    };
    for command in commands {
        match *command {
            Command::MoveTo(p) => {
                close(&mut segments, current, start);
                start = map(p);
                current = start;
            }
            Command::LineTo(p) => {
                let p = map(p);
                if p != current {
                    segments.push(Segment::line(current, p));
                }
                current = p;
            }
            Command::QuadTo(c, p) => {
                let p = map(p);
                segments.push(Segment::quad(current, map(c), p));
                current = p;
            }
            Command::CurveTo(c0, c1, p) => {
                let p = map(p);
                segments.push(Segment::cubic(current, map(c0), map(c1), p));
                current = p;
            }
            Command::Close => {
                close(&mut segments, current, start);
                current = start;
            }
        }
    }
    close(&mut segments, current, start);
    contours
}

/// Whether any edge lies inside the filled area, where the field would show a
/// seam. Samples the nonzero winding along rows half a unit apart: every
/// crossing of a true outline edge has zero winding on one side.
pub(crate) fn contours_overlap(contours: &[Contour]) -> bool {
    let segments: Vec<_> = contours
        .iter()
        .flat_map(|contour| &contour.segments)
        .collect();
    let (min_y, max_y) = segments
        .iter()
        .flat_map(|segment| (0..=segment.order() as usize + 1).map(|i| segment.control_point(i).y))
        .fold((f64::MAX, f64::MIN), |(min, max), y| {
            (min.min(y), max.max(y))
        });
    let mut crossings = Vec::new();
    let mut y = min_y.floor() + 0.5;
    while y < max_y {
        crossings.clear();
        for segment in &segments {
            let p0 = segment.start().y;
            let p1 = segment.control_point(1).y;
            let coefficients = match segment.order() {
                fdsm::bezier::Order::Linear => [p0 - y, p1 - p0, 0.0, 0.0],
                fdsm::bezier::Order::Quadratic => {
                    let p2 = segment.end().y;
                    [p0 - y, 2.0 * (p1 - p0), p0 - 2.0 * p1 + p2, 0.0]
                }
                fdsm::bezier::Order::Cubic => {
                    let p2 = segment.control_point(2).y;
                    let p3 = segment.end().y;
                    [
                        p0 - y,
                        3.0 * (p1 - p0),
                        3.0 * (p0 - 2.0 * p1 + p2),
                        -p0 + 3.0 * p1 - 3.0 * p2 + p3,
                    ]
                }
            };
            for t in kurbo::common::solve_cubic(
                coefficients[0],
                coefficients[1],
                coefficients[2],
                coefficients[3],
            ) {
                if !(0.0..=1.0).contains(&t) {
                    continue;
                }
                let dy = segment.direction_at(t).y;
                // Half-open crossings count a shared vertex once.
                if dy.abs() < 1e-9 || (t == 0.0 && dy < 0.0) || (t == 1.0 && dy > 0.0) {
                    continue;
                }
                crossings.push((segment.get(t).x, if dy > 0.0 { 1 } else { -1 }));
            }
        }
        crossings.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        let mut winding = 0;
        for (_, delta) in &crossings {
            let next = winding + delta;
            if winding != 0 && next != 0 {
                return true;
            }
            winding = next;
        }
        y += 0.5;
    }
    false
}
