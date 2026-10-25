use cosmic_text::{CacheKey, CacheKeyFlags, FontSystem, SubpixelBin, SwashCache};
use fdsm::{
    bezier::{Point, Segment, scanline::FillRule},
    correct_error::{DistanceCheckMode, ErrorCorrectionConfig, correct_error_msdf},
    generate::generate_msdf,
    render::correct_sign_msdf,
    shape::{Contour, Shape},
};
use image::{ImageBuffer, Rgb};
use swash::zeno::{Command, Vector};

use crate::{GLYPH_MODE_MSDF, GlyphPixels, MSDF_EM_SIZE, MSDF_PX_RANGE, RenderedGlyph};

/// Field texels kept around the outline: half the range plus one texel that
/// the drawn quad insets by half a texel.
const PADDING: f32 = MSDF_PX_RANGE / 2.0 + 1.0;

/// The size-independent key that an MSDF field is generated and cached for.
pub(crate) fn msdf_key(key: CacheKey) -> CacheKey {
    CacheKey {
        font_size_bits: MSDF_EM_SIZE.to_bits(),
        x_bin: SubpixelBin::Zero,
        y_bin: SubpixelBin::Zero,
        flags: (key.flags & CacheKeyFlags::FAKE_ITALIC) | CacheKeyFlags::DISABLE_HINTING,
        ..key
    }
}

/// Generates the field for a key from [`msdf_key`]. The outline comes from the
/// same Swash scaler as raster glyphs, so variable-font weight and fake italic
/// match. Colour, bitmap and overlapping-contour glyphs are left to raster.
pub(crate) fn render_msdf_glyph(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    key: CacheKey,
) -> RenderedGlyph {
    let Some(font) = font_system.get_font(key.font_id, key.font_weight) else {
        return RenderedGlyph::RasterOnly;
    };
    if [b"COLR", b"CBDT", b"sbix", b"EBDT"]
        .iter()
        .any(|tag| font.as_swash().table(u32::from_be_bytes(**tag)).is_some())
    {
        return RenderedGlyph::RasterOnly;
    }
    let Some(commands) = swash_cache.get_outline_commands_uncached(font_system, key) else {
        return RenderedGlyph::RasterOnly;
    };
    let Some((min, max)) = bounds(&commands) else {
        return RenderedGlyph::Empty;
    };

    let left = (min.x - PADDING).floor();
    let top = (max.y + PADDING).ceil();
    let width = ((max.x + PADDING).ceil() - left) as u32;
    let height = (top - (min.y - PADDING).floor()) as u32;
    let contours = contours(&commands, |p| {
        Point::new(f64::from(p.x - left), f64::from(top - p.y))
    });
    if contours_overlap(&contours) {
        return RenderedGlyph::RasterOnly;
    }

    let shape = Shape::edge_coloring_simple(Shape { contours }, 0.03, 0);
    let prepared = shape.prepare();
    let range = f64::from(MSDF_PX_RANGE);
    let mut field = ImageBuffer::<Rgb<f32>, _>::new(width, height);
    generate_msdf(&prepared, range, &mut field);
    let mut correction = ErrorCorrectionConfig::default();
    correction.distance_check = DistanceCheckMode::Never;
    correct_error_msdf(&mut field, &shape, &prepared, range, &correction);
    correct_sign_msdf(&mut field, &prepared, FillRule::Nonzero);

    let pixels = field
        .pixels()
        .flat_map(|pixel| {
            let [r, g, b] = pixel
                .0
                .map(|value| (value * 255.0).round().clamp(0.0, 255.0) as u8);
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

fn bounds(commands: &[Command]) -> Option<(Vector, Vector)> {
    let mut points = commands.iter().flat_map(|command| match *command {
        Command::MoveTo(p) | Command::LineTo(p) => vec![p],
        Command::QuadTo(c, p) => vec![c, p],
        Command::CurveTo(c0, c1, p) => vec![c0, c1, p],
        Command::Close => Vec::new(),
    });
    let first = points.next()?;
    Some(points.fold((first, first), |(min, max), p| {
        (
            Vector::new(min.x.min(p.x), min.y.min(p.y)),
            Vector::new(max.x.max(p.x), max.y.max(p.y)),
        )
    }))
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
/// seam. Samples the nonzero winding along rows one unit apart: every
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
        y += 1.0;
    }
    false
}
