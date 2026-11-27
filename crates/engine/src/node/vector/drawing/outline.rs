//! Outlines flattened into polygons, for nodes that build solids from them.

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use super::super::drawable::MIN_SCALE;
use super::ShapeGeometryKind;
use super::collect::{Outline, Source};
use super::path::{POINT_CLOSED, parse_path_points};
use super::text::TextGlyphs;

/// Fewest sides a flattened ellipse has, and most, whatever its size.
const MIN_CIRCLE_SIDES: usize = 16;
const MAX_CIRCLE_SIDES: usize = 256;

/// One contour of a flattened outline, in the pixels the outline's transform maps to.
pub(crate) struct Contour {
    pub(crate) points: Vec<[f64; 2]>,
    /// Whether the outline closes it, as a shape's contour or a path's closed subpath is.
    pub(crate) closed: bool,
}

/// Sides of a regular polygon that stays within `tolerance` of a circle of `radius`, both in the
/// same units.
pub(crate) fn circle_sides(radius: f64, tolerance: f64) -> usize {
    let angle = (1.0 - tolerance / radius).clamp(-1.0, 1.0).acos();
    let sides = if angle > 0.0 { PI / angle } else { f64::MAX };
    (sides.ceil() as usize).clamp(MIN_CIRCLE_SIDES, MAX_CIRCLE_SIDES)
}

/// The contours of `outline`'s source, laid out and transformed to pixels, with curves flattened
/// to within `tolerance` world units when a pixel is `scale` world units across. An ellipse, or the
/// corner of a rounded rectangle, has as many sides as [`circle_sides`] gives its radius. Flattening
/// a path or a text coarsens the tolerance until it has at most `max_points` points. A text's
/// glyphs are `text`, which it is shaped as. Empty when the transform scales an axis to nothing,
/// or a shape has no area. The error says what is wrong with the path data, as that of a
/// `path_2d` is, or which limit the text passes.
pub(crate) fn flatten(
    outline: &Outline,
    text: Option<&TextGlyphs>,
    scale: f64,
    tolerance: f64,
    max_points: usize,
) -> Result<Vec<Contour>, String> {
    let transform = &outline.transform;
    if transform.scale.0.abs() < MIN_SCALE || transform.scale.1.abs() < MIN_SCALE {
        return Ok(Vec::new());
    }
    let (x, y) = transform.position;
    let place = |[px, py]: [f64; 2]| {
        let (x, y) = transform.map((px + x, py + y));
        [x, y]
    };
    let largest = transform.scale.0.abs().max(transform.scale.1.abs());
    let world = scale * largest;
    if !(world > 0.0 && world.is_finite()) {
        return Ok(Vec::new());
    }
    let local = match &outline.source {
        Source::Path { data, .. } => {
            let pixels = (tolerance / world).min(f64::from(f32::MAX)) as f32;
            let points = parse_path_points(data, pixels, max_points)
                .map_err(|details| format!("`data` {details}"))?;
            let mut contours: Vec<(Vec<[f64; 2]>, bool)> = Vec::new();
            for (index, point) in points.iter().enumerate() {
                if point.contour_start as usize == index {
                    contours.push((Vec::new(), false));
                }
                if let Some((points, closed)) = contours.last_mut() {
                    points.push(point.position.map(f64::from));
                    *closed = point.flags & POINT_CLOSED != 0;
                }
            }
            contours
        }
        Source::Shape {
            kind,
            size,
            border_radius,
        } => {
            let [width, height] = size.map(f64::from);
            if width <= 0.0 || height <= 0.0 {
                return Ok(Vec::new());
            }
            let points = match kind {
                ShapeGeometryKind::Ellipse => {
                    let sides = circle_sides(width.max(height) * 0.5 * world, tolerance);
                    (0..sides)
                        .map(|side| {
                            let angle = TAU * side as f64 / sides as f64;
                            [
                                (1.0 + angle.cos()) * width * 0.5,
                                (1.0 + angle.sin()) * height * 0.5,
                            ]
                        })
                        .collect()
                }
                ShapeGeometryKind::Rectangle => {
                    let radius = f64::from(*border_radius).min(width * 0.5).min(height * 0.5);
                    rounded_rectangle(width, height, radius, world, tolerance)
                }
            };
            vec![(points, true)]
        }
        Source::Text { .. } => {
            let Some(text) = text else {
                unreachable!("a text is shaped before it is flattened")
            };
            let pixels = (tolerance / world).min(f64::from(f32::MAX)) as f32;
            text.contours(pixels, max_points)?
                .into_iter()
                .map(|contour| {
                    (
                        contour.into_iter().map(|p| p.map(f64::from)).collect(),
                        true,
                    )
                })
                .collect()
        }
    };
    Ok(local
        .into_iter()
        .map(|(points, closed)| Contour {
            points: points.into_iter().map(place).collect(),
            closed,
        })
        .collect())
}

/// The corners of a `width` by `height` rectangle whose corners are rounded by `radius`, from the
/// top left one clockwise on screen.
fn rounded_rectangle(
    width: f64,
    height: f64,
    radius: f64,
    world: f64,
    tolerance: f64,
) -> Vec<[f64; 2]> {
    if radius <= 0.0 {
        return vec![[0.0, 0.0], [width, 0.0], [width, height], [0.0, height]];
    }
    let steps = circle_sides(radius * world, tolerance).div_ceil(4);
    let corners = [
        ([radius, radius], PI),
        ([width - radius, radius], PI + FRAC_PI_2),
        ([width - radius, height - radius], 0.0),
        ([radius, height - radius], FRAC_PI_2),
    ];
    corners
        .into_iter()
        .flat_map(|(center, start)| {
            (0..=steps).map(move |step| {
                let angle = start + FRAC_PI_2 * step as f64 / steps as f64;
                [
                    center[0] + radius * angle.cos(),
                    center[1] + radius * angle.sin(),
                ]
            })
        })
        .collect()
}
