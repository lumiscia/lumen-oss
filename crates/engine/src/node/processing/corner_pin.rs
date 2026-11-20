use bytemuck::{Pod, Zeroable};

use crate::error::RenderError;
use crate::gpu::frame_graph::ComputeProgram;
use crate::gpu::mips::full_levels;
use crate::node::motion::{self, Lerp, MAX_BOUNDED_MAGNITUDE, ROUNDING_ERROR_FACTOR, Shutter};
use crate::node::{
    DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, OutputRef,
    processing::{
        RasterSampling,
        minify::{self, Minification},
    },
};

pub(crate) const SHADER: &str = concat!(
    include_str!("corner_pin.wgsl"),
    include_str!("../../gpu/shaders/minify.wgsl")
);
const CORNER_PIN: ComputeProgram = ComputeProgram::spatial("corner-pin", SHADER);

/// The names of the corner params, clockwise from the top left.
const CORNERS: [&str; 4] = ["top_left", "top_right", "bottom_right", "bottom_left"];

/// Pixels past the quad's bounds that its antialiased edge and the filtering of the source reach,
/// with a pixel to spare.
const BOUNDS_PAD: f64 = 2.0;

/// Points per side of the grid over a quad at which the source's footprint is measured.
const FOOTPRINT_GRID: usize = 5;

/// The smallest `|cross|` of two consecutive edges of a quad, as a fraction of the square of its
/// largest side, below which it counts as flat.
const FLATNESS: f64 = 1e-6;

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct CornerPinParams {
    /// Where the source's top-left corner lands, in canvas pixels.
    #[meta(name = "Top left", step = 1, pixels)]
    pub top_left: (f64, f64),
    /// Where the source's top-right corner lands, in canvas pixels.
    #[meta(name = "Top right", step = 1, pixels)]
    pub top_right: (f64, f64),
    /// Where the source's bottom-right corner lands, in canvas pixels.
    #[meta(name = "Bottom right", step = 1, pixels)]
    pub bottom_right: (f64, f64),
    /// Where the source's bottom-left corner lands, in canvas pixels.
    #[meta(name = "Bottom left", step = 1, pixels)]
    pub bottom_left: (f64, f64),
    /// Motion blur shutter angle in degrees, centred on the frame. 0 is off, 180 is the
    /// usual film look and 360 smears over the whole frame.
    #[meta(name = "Motion blur", min = 0, max = 360, step = 1)]
    pub motion_blur: f64,
    /// Sampling filter used when pinning.
    #[meta()]
    pub sampling: RasterSampling,
}

impl Default for CornerPinParams {
    fn default() -> Self {
        Self {
            top_left: (0.0, 0.0),
            top_right: (100.0, 0.0),
            bottom_right: (100.0, 100.0),
            bottom_left: (0.0, 100.0),
            motion_blur: 0.0,
            sampling: RasterSampling::Linear,
        }
    }
}

/// Pins a raster's four corners to four points of the canvas, with the perspective that takes.
///
/// `top_left`, `top_right`, `bottom_right` and `bottom_left` are where the source's own corners
/// land, in canvas pixels, which run clockwise on screen from the top left when the source is
/// pinned upright. The source is stretched across the quad they make, with perspective, and the
/// canvas outside the quad is transparent. The corners can't default to the source's own, so
/// they default to a 100 pixel square at the canvas's top left; set all four.
///
/// The corners must make a convex quad with some area. Corners that cross each other, bend a
/// corner inwards or lie in a line fail the render, naming the corner at fault when it can. Run
/// the other way round, the corners mirror the source. They are checked at every frame, so
/// animated corners must stay convex throughout.
///
/// The quad's edges are antialiased, and the source is filtered for how much the pin shrinks it
/// at each pixel, so the far end of a steep pin does not alias. With `motion_blur`, animated
/// corners smear along their motion.
///
/// For example, pinning a 1920 by 1080 video onto a tilted screen in a plate:
/// `{ "top_left": [412, 210], "top_right": [905, 262], "bottom_right": [893, 540], "bottom_left": [405, 498] }`.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "corner_pin", name = "Corner Pin", category = "processing")]
pub struct CornerPin {
    pub id: NodeId,
    #[params]
    pub params: CornerPinParamsDelegate,
    #[input()]
    pub source: Option<OutputRef>,
}

/// The unit square's corners, clockwise from the top left, which a quad's corners are the images
/// of.
const UNIT_CORNERS: [[f64; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

/// The four points, in device pixels, that the source's corners land on, clockwise from the top
/// left.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Quad([[f64; 2]; 4]);

impl Lerp for Quad {
    fn lerp(&self, other: &Self, t: f64) -> Self {
        Self(std::array::from_fn(|corner| {
            let (a, b) = (self.0[corner], other.0[corner]);
            [motion::lerp(a[0], b[0], t), motion::lerp(a[1], b[1], t)]
        }))
    }
}

/// What the shader needs of a quad.
struct Pin {
    /// The rows of the matrix that takes a canvas pixel to the source, as the shader reads them.
    rows: [[f64; 3]; 3],
    /// How many texels of the source a pixel covers along its axes, at the most, over the quad.
    footprint: [f64; 2],
}

impl Quad {
    /// The quad of evaluated `params`, whose corners are already in device pixels.
    fn of(params: &CornerPinParams) -> Self {
        let corner = |(x, y): (f64, f64)| [x, y];
        Self([
            corner(params.top_left),
            corner(params.top_right),
            corner(params.bottom_right),
            corner(params.bottom_left),
        ])
    }

    /// The point at `(u, v)` of the unit square, when its corners are carried to this quad's
    /// without perspective.
    fn at_unit(&self, [u, v]: [f64; 2]) -> [f64; 2] {
        let [top_left, top_right, bottom_right, bottom_left] = self.0;
        [0, 1].map(|axis| {
            let top = motion::lerp(top_left[axis], top_right[axis], u);
            let bottom = motion::lerp(bottom_left[axis], bottom_right[axis], u);
            motion::lerp(top, bottom, v)
        })
    }

    /// Why this quad can't be pinned to, or `None` when it is convex with some area.
    fn problem(&self) -> Option<String> {
        if self.0.iter().flatten().any(|value| !value.is_finite()) {
            return Some("corner_pin corners must be finite numbers".to_string());
        }
        let corner = |index: usize| self.0[index % 4];
        let side = |index: usize| {
            let (from, to) = (corner(index), corner(index + 1));
            [to[0] - from[0], to[1] - from[1]]
        };
        // The turn at each corner, from the side into it to the side out of it.
        let turns: [f64; 4] = std::array::from_fn(|index| {
            let (into, out) = (side(index + 3), side(index));
            into[0] * out[1] - into[1] * out[0]
        });
        let longest = (0..4)
            .map(|index| side(index)[0].hypot(side(index)[1]))
            .fold(0.0, f64::max);
        let flat = FLATNESS * longest * longest;
        if let Some(index) = turns.iter().position(|turn| turn.abs() <= flat) {
            return Some(format!(
                "corner_pin corners must form a convex quad with some area; {} is in line with \
                 its neighbours",
                CORNERS[index]
            ));
        }
        let positive = turns.iter().filter(|turn| **turn > 0.0).count();
        match positive {
            0 | 4 => None,
            1 | 3 => {
                let odd = turns
                    .iter()
                    .position(|turn| (*turn > 0.0) == (positive == 1))
                    .unwrap_or_default();
                Some(format!(
                    "corner_pin corners must form a convex quad; {} is bent inwards",
                    CORNERS[odd]
                ))
            }
            _ => Some(
                "corner_pin corners must form a convex quad; the corners cross each other, so \
                 check that they run clockwise from top_left"
                    .to_string(),
            ),
        }
    }

    /// The matrix, as rows, that takes a canvas pixel `(x, y, 1)` to the source point
    /// `(a, b) / c` for the product `(a, b, c)`, for a source of `size`. The quad must be convex.
    fn inverse_homography(&self, size: [f64; 2]) -> [[f64; 3]; 3] {
        // The map from the unit square to the quad, by Heckbert's construction.
        let [[x0, y0], [x1, y1], [x2, y2], [x3, y3]] = self.0;
        let (dx1, dx2, dx3) = (x1 - x2, x3 - x2, x0 - x1 + x2 - x3);
        let (dy1, dy2, dy3) = (y1 - y2, y3 - y2, y0 - y1 + y2 - y3);
        let (g, h) = if dx3 == 0.0 && dy3 == 0.0 {
            (0.0, 0.0)
        } else {
            let denominator = dx1 * dy2 - dx2 * dy1;
            (
                (dx3 * dy2 - dx2 * dy3) / denominator,
                (dx1 * dy3 - dx3 * dy1) / denominator,
            )
        };
        let [a, b, c] = [x1 - x0 + g * x1, x3 - x0 + h * x3, x0];
        let [d, e, f] = [y1 - y0 + g * y1, y3 - y0 + h * y3, y0];
        let determinant = a * (e - f * h) - b * (d - f * g) + c * (d * h - e * g);
        let inverse = [
            [e - f * h, c * h - b, b * f - c * e],
            [f * g - d, a - c * g, c * d - a * f],
            [d * h - e * g, b * g - a * h, a * e - b * d],
        ]
        .map(|row| row.map(|value| value / determinant));
        // Source pixels are the unit square scaled up by the source's size.
        [
            inverse[0].map(|value| value * size[0]),
            inverse[1].map(|value| value * size[1]),
            inverse[2],
        ]
    }

    /// The lines of the quad's four sides as `(nx, ny, c)`, where `nx * x + ny * y + c` is the
    /// distance in pixels of `(x, y)` inside the side, negative outside it.
    fn edges(&self) -> [[f32; 4]; 4] {
        let centre = [0, 1].map(|axis| self.0.iter().map(|corner| corner[axis]).sum::<f64>() / 4.0);
        std::array::from_fn(|index| {
            let (from, to) = (self.0[index], self.0[(index + 1) % 4]);
            let length = (to[0] - from[0]).hypot(to[1] - from[1]);
            let normal = [-(to[1] - from[1]) / length, (to[0] - from[0]) / length];
            let inward = normal[0] * (centre[0] - from[0]) + normal[1] * (centre[1] - from[1]);
            let sign = inward.signum();
            let offset = -(normal[0] * from[0] + normal[1] * from[1]);
            [
                (sign * normal[0]) as f32,
                (sign * normal[1]) as f32,
                (sign * offset) as f32,
                0.0,
            ]
        })
    }

    fn pin(&self, size: [f64; 2]) -> Pin {
        let rows = self.inverse_homography(size);
        // The corners, and a grid over the quad's interior, since a steep pin can cover the most
        // texels per pixel away from its corners.
        let steps = FOOTPRINT_GRID - 1;
        let footprint = (0..FOOTPRINT_GRID * FOOTPRINT_GRID)
            .map(|index| {
                let unit = |step: usize| step as f64 / steps as f64;
                let at = self.at_unit([unit(index % FOOTPRINT_GRID), unit(index / FOOTPRINT_GRID)]);
                footprint(&rows, at)
            })
            .fold([0.0_f64; 2], |most, here| {
                [most[0].max(here[0]), most[1].max(here[1])]
            });
        Pin { rows, footprint }
    }
}

/// How many texels of the source one pixel at `at` covers along each of the source's axes: the
/// length of each row of the Jacobian of the map `rows` makes from the canvas to the source.
fn footprint(rows: &[[f64; 3]; 3], at: [f64; 2]) -> [f64; 2] {
    let apply = |row: &[f64; 3]| row[0] * at[0] + row[1] * at[1] + row[2];
    let divisor = apply(&rows[2]);
    let derivative = |row: &[f64; 3]| {
        let source = apply(row) / divisor;
        [
            (row[0] - source * rows[2][0]) / divisor,
            (row[1] - source * rows[2][1]) / divisor,
        ]
    };
    [&rows[0], &rows[1]].map(|row| {
        let [along_x, along_y] = derivative(row);
        along_x.hypot(along_y)
    })
}

impl CornerPin {
    /// The quads to average over the shutter, or `None` when motion blur is off or nothing moves.
    fn taps(
        &self,
        evaluated: &CornerPinParams,
        expr: &crate::expr::ExpressionContext<'_>,
        pixel_scale: f64,
    ) -> crate::Result<Option<Vec<Quad>>> {
        let Some(offset) = motion::shutter_offset(evaluated.motion_blur) else {
            return Ok(None);
        };
        let params = &self.params;
        let at = |subframe: f64| -> crate::Result<Quad> {
            let expr = expr.at_subframe(subframe);
            let corner = |value: &crate::node::Deferred<(f64, f64)>, name: &str| {
                value
                    .eval(self.id, name, &expr)
                    .map(|(x, y)| [x * pixel_scale, y * pixel_scale])
            };
            Ok(Quad([
                corner(&params.top_left, CORNERS[0])?,
                corner(&params.top_right, CORNERS[1])?,
                corner(&params.bottom_right, CORNERS[2])?,
                corner(&params.bottom_left, CORNERS[3])?,
            ]))
        };
        let shutter = Shutter {
            open: at(-offset)?,
            frame: Quad::of(evaluated),
            close: at(offset)?,
        };
        Ok((!shutter.is_static())
            .then(|| shutter.taps(&UNIT_CORNERS, |quad, corner| quad.at_unit(corner))))
    }
}

/// The box that `quads` cover with their antialiased edges, or `None` when it can't be worked out
/// in single precision.
fn bounds_of_quads(quads: &[Quad]) -> Option<[f32; 4]> {
    let mut bounds = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    let mut magnitude = 0.0_f64;
    for corner in quads.iter().flat_map(|quad| quad.0) {
        bounds = [
            bounds[0].min(corner[0]),
            bounds[1].min(corner[1]),
            bounds[2].max(corner[0]),
            bounds[3].max(corner[1]),
        ];
        magnitude = magnitude.max(corner[0].abs()).max(corner[1].abs());
    }
    if magnitude > MAX_BOUNDED_MAGNITUDE {
        return None;
    }
    // Single-precision rounding in the shader grows with the largest value it handles.
    let pad = BOUNDS_PAD + ROUNDING_ERROR_FACTOR * f64::from(f32::EPSILON) * magnitude;
    let [min_x, min_y, max_x, max_y] = bounds;
    Some([min_x - pad, min_y - pad, max_x + pad, max_y + pad].map(|value| value as f32))
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "CornerPinParams", source = SHADER)]
struct CornerPinUniforms {
    sampling: u32,
    /// Quads to average over the shutter; at least 1.
    tap_count: u32,
    /// Canvas pixel of the output's first texel.
    output_origin: [i32; 2],
    /// Source pixel of the source's first texel.
    source_origin: [i32; 2],
    _pad: [u32; 2],
    taps: [CornerPinTap; crate::node::motion::MAX_MOTION_TAPS],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "CornerPinTap", source = SHADER)]
struct CornerPinTap {
    /// The rows of the matrix from canvas pixels to the source, each in `xyz`.
    rows: [[f32; 4]; 3],
    /// The lines of the quad's sides.
    edges: [[f32; 4]; 4],
}

impl crate::gpu::frame_graph::RecordNode for CornerPin {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        crate::gpu::mips::programs(programs)?;
        programs.compute(&CORNER_PIN)
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let source = cx.required(self.id, "source", self.source)?;
        let shown = source.shown_size();
        let size = [shown.width, shown.height].map(f64::from);
        let pixel_scale = cx.pixel_scale();
        let (evaluated, taps) = {
            let expr = cx.expr();
            let mut evaluated = self.params.eval(&DelegateEvalContext {
                node_id: self.id,
                expr: &expr,
                pixel_scale,
            })?;
            if !cx.analytic_motion_blur() {
                evaluated.motion_blur = 0.0;
            }
            let taps = self.taps(&evaluated, &expr, pixel_scale)?;
            (evaluated, taps)
        };
        let frame_quad = Quad::of(&evaluated);
        let quads = taps.as_deref().unwrap_or(std::slice::from_ref(&frame_quad));
        if let Some(problem) = std::iter::once(&frame_quad)
            .chain(taps.iter().flatten())
            .find_map(Quad::problem)
        {
            return Err(RenderError::evaluating::<CornerPin>(self.id, problem).into());
        }
        let bounds = cx.bounds_of(bounds_of_quads(quads));
        let domain = cx.source_domain(bounds);
        let mut params = CornerPinUniforms {
            sampling: evaluated.sampling as u32,
            tap_count: quads.len() as u32,
            output_origin: [domain.data_rect.x, domain.data_rect.y],
            source_origin: source.origin(),
            ..Zeroable::zeroed()
        };
        let out_size = domain.storage_size;
        let tex = cx.storage_target(out_size);
        // The source needs the levels that the pixels farthest from reading it at its own size
        // read. The footprint is measured at the corners and over a grid, so a level of margin
        // covers the pixels between the samples.
        let mips = Minification::mips_fit(cx, source.domain.storage_size);
        let mut levels = 1;
        for (tap, quad) in params.taps.iter_mut().zip(quads) {
            let pin = quad.pin(size);
            *tap = CornerPinTap {
                rows: pin
                    .rows
                    .map(|row| [row[0] as f32, row[1] as f32, row[2] as f32, 0.0]),
                edges: quad.edges(),
            };
            if evaluated.sampling == RasterSampling::Linear {
                let footprint = pin.footprint.map(|texels| texels as f32);
                let filter = Minification::new(footprint, source.domain.storage_size, mips)
                    .filter(|filter| filter.levels() > 1);
                if let Some(filter) = filter {
                    let margin = (filter.levels() + 1).min(full_levels(source.domain.storage_size));
                    levels = levels.max(margin);
                }
            }
        }
        let input = minify::source(cx, self.source, source, None, levels)?;
        cx.spatial_pass(&CORNER_PIN, input, &params, tex, out_size)?;
        Ok(crate::gpu::frame_graph::Raster {
            tex,
            domain,
            bounds: None,
            reduced: None,
        })
    }

    fn analytic_shutter(&self) -> Option<&crate::node::Deferred<f64>> {
        Some(&self.params.motion_blur)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumen_gpu::Size;

    #[test]
    fn the_footprint_counts_the_pixels_between_the_corners() {
        let quad = Quad([
            [75.77, 79.13],
            [56.15, 46.75],
            [64.71, 53.38],
            [122.41, 137.28],
        ]);
        let size = Size::new(1024, 1024);
        let levels = |footprint: [f64; 2]| {
            Minification::new(footprint.map(|texels| texels as f32), size, true)
                .map_or(1, Minification::levels)
        };
        let corners = quad
            .0
            .map(|corner| footprint(&quad.inverse_homography([1024.0; 2]), corner))
            .into_iter()
            .fold([0.0_f64; 2], |most, here| {
                [most[0].max(here[0]), most[1].max(here[1])]
            });
        // The pixel at (61.5, 55.5) reads a level the corners' footprint does not reach.
        assert!(levels(quad.pin([1024.0; 2]).footprint) > levels(corners));
    }
}
