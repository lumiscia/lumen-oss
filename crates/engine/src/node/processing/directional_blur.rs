//! Blurs that average a raster along a path through each pixel: a line, an arc about a centre,
//! or a ray from it. Each path is a one-parameter family of moves that compose by adding their
//! parameters, so a box along it splits into two passes: a short box, then a comb of its copies.
//! `n` taps in each pass sample the path as finely as `n²` taps in one, but for the comb's
//! bilinear reads of the stored first pass, which soften fine detail. Short streaks, where that
//! shows, and blurs whose first pass would take too much storage, take every tap from the source
//! at once instead, and the two crossfade between them so that animating a blur never jumps.

use bytemuck::{Pod, Zeroable};

use crate::gpu::{frame_graph, params};
use crate::node::motion::MAX_BOUNDED_MAGNITUDE;
use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams};

use super::displacement_map::EdgeMode;

const SHADER: &str = include_str!("directional_blur.wgsl");

/// Most taps of the short box, and of the comb. A streak up to `MAX_SWEEP_TAPS²` pixels long is
/// sampled with a tap per pixel, and longer streaks with taps that far apart.
pub const MAX_SWEEP_TAPS: usize = 64;
/// Streak lengths in pixels over which two passes take over from every tap at once.
const TWO_PASS_STREAKS: [f64; 2] = [64.0, 256.0];
/// Canvases' worth of texels, and fractions of the largest texture side, that a full-frame first
/// pass would take, over which two passes hand back to every tap at once.
const TWO_PASS_CANVASES: [f64; 2] = [2.0, 3.0];
const TWO_PASS_EXTENTS: [f64; 2] = [0.75, 1.0];

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct DirectionalBlurParams {
    /// Direction of the blur in degrees, clockwise from horizontal.
    #[meta(step = 1)]
    pub angle: f64,
    /// Length in pixels of the line each pixel is spread along, centred on it.
    #[meta(min = 0, step = 1)]
    pub distance: f64,
    /// What the source reads as beyond the canvas.
    #[meta()]
    pub edge: EdgeMode,
}

impl Default for DirectionalBlurParams {
    fn default() -> Self {
        Self {
            angle: 0.0,
            distance: 20.0,
            edge: EdgeMode::Clamp,
        }
    }
}

/// Blurs a raster along a line, as a straight motion blur.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "directional_blur",
    name = "Directional Blur",
    category = "processing"
)]
pub struct DirectionalBlur {
    pub id: NodeId,
    #[params]
    pub params: DirectionalBlurParamsDelegate,
    #[input()]
    pub source: Option<NodeId>,
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct RadialBlurParams {
    /// Centre of the spin in canvas pixels.
    #[meta()]
    pub center: (f64, f64),
    /// Arc in degrees each pixel is spread along about `center`, centred on it, up to 360.
    #[meta(min = 0, max = 360, step = 1)]
    pub amount: f64,
    /// What the source reads as beyond the canvas.
    #[meta()]
    pub edge: EdgeMode,
}

impl Default for RadialBlurParams {
    fn default() -> Self {
        Self {
            center: (960.0, 540.0),
            amount: 10.0,
            edge: EdgeMode::Clamp,
        }
    }
}

/// Blurs a raster along circles about a centre, as a spin.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "radial_blur", name = "Radial Blur", category = "processing")]
pub struct RadialBlur {
    pub id: NodeId,
    #[params]
    pub params: RadialBlurParamsDelegate,
    #[input()]
    pub source: Option<NodeId>,
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct ZoomBlurParams {
    /// Centre of the zoom in canvas pixels.
    #[meta()]
    pub center: (f64, f64),
    /// Length of the streak through each pixel, as a fraction of its distance from `center`.
    /// The streak runs both ways along the ray, sampled in equal ratios of scale, so it reaches
    /// farther away from `center` than towards it.
    #[meta(min = 0, step = 0.01)]
    pub amount: f64,
    /// What the source reads as beyond the canvas.
    #[meta()]
    pub edge: EdgeMode,
}

impl Default for ZoomBlurParams {
    fn default() -> Self {
        Self {
            center: (960.0, 540.0),
            amount: 0.2,
            edge: EdgeMode::Clamp,
        }
    }
}

/// Blurs a raster along rays from a centre, as a zoom.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "zoom_blur", name = "Zoom Blur", category = "processing")]
pub struct ZoomBlur {
    pub id: NodeId,
    #[params]
    pub params: ZoomBlurParamsDelegate,
    #[input()]
    pub source: Option<NodeId>,
}

/// `[min_x, min_y, max_x, max_y]` in canvas pixels.
type Bounds = [f64; 4];

fn grow(bounds: Bounds, by: f64) -> Bounds {
    [
        bounds[0] - by,
        bounds[1] - by,
        bounds[2] + by,
        bounds[3] + by,
    ]
}

fn intersect(a: Bounds, b: Bounds) -> Bounds {
    [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ]
}

fn corners(bounds: Bounds) -> [[f64; 2]; 4] {
    let [x0, y0, x1, y1] = bounds;
    [[x0, y0], [x1, y0], [x0, y1], [x1, y1]]
}

fn enclosing(points: impl IntoIterator<Item = [f64; 2]>) -> Bounds {
    points.into_iter().fold(
        [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ],
        |[x0, y0, x1, y1], [x, y]| [x0.min(x), y0.min(y), x1.max(x), y1.max(y)],
    )
}

fn finite_or_zero(value: f64) -> f64 {
    if value.is_finite() { value } else { 0.0 }
}

/// The moves a blur averages over, each a function of one parameter `u`.
#[derive(Debug, Clone, Copy)]
enum Path {
    /// Moves by `u` pixels along `direction`.
    Line { direction: [f64; 2] },
    /// Turns by `u` radians clockwise about `center`.
    Spin { center: [f64; 2] },
    /// Scales by `e^u` about `center`.
    Zoom { center: [f64; 2] },
}

#[derive(Debug, Clone, Copy)]
struct Sweep {
    path: Path,
    /// The range of `u` each pixel averages over, centred on 0.
    span: f64,
    edge: EdgeMode,
}

impl Sweep {
    /// The move at `u` as `[m00, m01, m10, m11, tx, ty]`, taking `p` to `M p + t`.
    fn affine(&self, u: f64) -> [f64; 6] {
        match self.path {
            Path::Line { direction } => [1.0, 0.0, 0.0, 1.0, u * direction[0], u * direction[1]],
            Path::Spin { center: [cx, cy] } => {
                let (sin, cos) = u.sin_cos();
                [
                    cos,
                    -sin,
                    sin,
                    cos,
                    cx - (cos * cx - sin * cy),
                    cy - (sin * cx + cos * cy),
                ]
            }
            Path::Zoom { center: [cx, cy] } => {
                let scale = u.exp();
                [
                    scale,
                    0.0,
                    0.0,
                    scale,
                    cx * (1.0 - scale),
                    cy * (1.0 - scale),
                ]
            }
        }
    }

    fn farthest(center: [f64; 2], bounds: Bounds) -> f64 {
        corners(bounds)
            .iter()
            .map(|[x, y]| (x - center[0]).hypot(y - center[1]))
            .fold(0.0, f64::max)
    }

    /// The longest streak, in pixels, of any pixel in `display`.
    fn longest_streak(&self, display: Bounds) -> f64 {
        match self.path {
            Path::Line { .. } => self.span,
            Path::Spin { center } => self.span * Self::farthest(center, display),
            Path::Zoom { center } => {
                2.0 * (self.span / 2.0).sinh() * Self::farthest(center, display)
            }
        }
    }

    /// Bounds of everything `bounds` is moved to by the moves of `|u| <= half`.
    fn swept(&self, bounds: Bounds, half: f64) -> Bounds {
        match self.path {
            Path::Line { direction } => {
                let [dx, dy] = direction.map(|axis| axis.abs() * half);
                [
                    bounds[0] - dx,
                    bounds[1] - dy,
                    bounds[2] + dx,
                    bounds[3] + dy,
                ]
            }
            Path::Zoom { .. } => {
                let [low, high] = [-half, half].map(|u| self.affine(u));
                enclosing(
                    corners(bounds)
                        .into_iter()
                        .flat_map(|[x, y]| [low, high].map(|m| [m[0] * x + m[4], m[3] * y + m[5]])),
                )
            }
            Path::Spin { center: [cx, cy] } => {
                // Rotated copies of a box reach farthest at their corners, so the corners' arcs
                // bound them: each arc's ends, and the points where it crosses an axis.
                let arcs = corners(bounds).into_iter().flat_map(|[x, y]| {
                    let radius = (x - cx).hypot(y - cy);
                    let angle = (y - cy).atan2(x - cx);
                    let ends = [angle - half, angle + half];
                    let axes = (0..4).map(|k| f64::from(k) * std::f64::consts::FRAC_PI_2);
                    let crossed = axes.filter(move |&axis| {
                        let turn = (axis - angle + std::f64::consts::PI)
                            .rem_euclid(std::f64::consts::TAU)
                            - std::f64::consts::PI;
                        turn.abs() <= half
                    });
                    ends.into_iter()
                        .chain(crossed)
                        .map(move |at| [cx + radius * at.cos(), cy + radius * at.sin()])
                });
                enclosing(arcs.chain(corners(bounds)))
            }
        }
    }
}

/// Midpoints of `count` equal parts of `u` in `-range / 2..range / 2`.
fn taps(sweep: &Sweep, count: usize, range: f64) -> [GpuSweepTap; MAX_SWEEP_TAPS] {
    let mut taps = [GpuSweepTap::zeroed(); MAX_SWEEP_TAPS];
    for (index, tap) in taps.iter_mut().take(count).enumerate() {
        let u = ((index as f64 + 0.5) / count as f64 - 0.5) * range;
        let m = sweep.affine(u).map(|value| value as f32);
        *tap = GpuSweepTap {
            m: [m[0], m[1], m[2], m[3]],
            t: [m[4], m[5], 0.0, 0.0],
        };
    }
    taps
}

/// Taps of the short box and of the comb: about one tap per pixel of the longest streak.
fn tap_counts(longest_streak: f64) -> (usize, usize) {
    // `min` and `max` take a NaN streak to the most taps.
    let total = longest_streak
        .ceil()
        .min((MAX_SWEEP_TAPS * MAX_SWEEP_TAPS) as f64)
        .max(1.0) as usize;
    let comb = total.isqrt() + usize::from(total.isqrt().pow(2) < total);
    (total.div_ceil(comb), comb)
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuSweepTap {
    m: [f32; 4],
    t: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuSweepParams {
    /// Taps of the short box, 1 when reads of the source clamp to the canvas, and taps of the
    /// comb.
    header: [u32; 4],
    /// Canvas pixel of the source's first texel in `xy`, and the canvas size in `zw`.
    source: [i32; 4],
    /// Canvas pixel of the first texel of the stored short boxes in `xy`, and of the output in
    /// `zw`.
    origins: [i32; 4],
    /// How much of the output the comb of stored short boxes makes, against every tap at once.
    blend: [f32; 4],
    taps: [GpuSweepTap; MAX_SWEEP_TAPS],
    comb: [GpuSweepTap; MAX_SWEEP_TAPS],
}

const BOXES_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(1, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Sampler),
    lumen_gpu::LayoutEntry::compute(
        3,
        lumen_gpu::LayoutKind::StorageTexture(lumen_gpu::wgpu::TextureFormat::Rgba16Float),
    ),
];
const COMB_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(1, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Sampler),
    lumen_gpu::LayoutEntry::compute(4, crate::gpu::frame_graph::RECORD_STORAGE_TEXTURE),
    lumen_gpu::LayoutEntry::compute(5, lumen_gpu::LayoutKind::Texture),
];

fn programs(programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
    programs.compute("directional-blur", SHADER, "boxes_main", BOXES_LAYOUT)?;
    programs.compute("directional-blur", SHADER, "comb_main", COMB_LAYOUT)
}

/// How much of a blur the comb of stored short boxes takes, against every tap at once: none for
/// streaks up to the first of `TWO_PASS_STREAKS` pixels long, all from the second, and back to
/// none as `read`, the first pass a full-frame output reads, grows through `TWO_PASS_CANVASES`
/// canvases or `TWO_PASS_EXTENTS` of the largest texture side, measured unrounded. It is
/// continuous in the blur's params, and depends only on them and the canvas.
fn two_pass_weight(longest: f64, read: Bounds, display: lumen_gpu::Size, max_extent: u32) -> f32 {
    let rise = |value: f64, [from, to]: [f64; 2]| ((value - from) / (to - from)).clamp(0.0, 1.0);
    let [width, height] = [read[2] - read[0], read[3] - read[1]];
    if !(width.abs() < MAX_BOUNDED_MAGNITUDE && height.abs() < MAX_BOUNDED_MAGNITUDE) {
        return 0.0;
    }
    let canvases = width.max(0.0) * height.max(0.0) / area(display).max(1) as f64;
    // Stored boxes are rounded out to whole texels, by up to 2 a side.
    let extent = (width.max(height) + 2.0) / f64::from(max_extent);
    let weight = rise(longest, TWO_PASS_STREAKS)
        * (1.0 - rise(canvases, TWO_PASS_CANVASES))
        * (1.0 - rise(extent, TWO_PASS_EXTENTS));
    // NaN, from a NaN streak, takes every tap at once.
    if weight > 0.0 { weight as f32 } else { 0.0 }
}

fn rect_covering(bounds: Bounds) -> Option<lumen_gpu::RectI> {
    if bounds
        .iter()
        .any(|value| value.abs() >= MAX_BOUNDED_MAGNITUDE)
    {
        return None;
    }
    lumen_gpu::RectI::covering(bounds.map(|value| value as f32))
}

fn bounds_of(rect: lumen_gpu::RectI) -> Bounds {
    let (x, y) = (f64::from(rect.x), f64::from(rect.y));
    [x, y, x + f64::from(rect.width), y + f64::from(rect.height)]
}

fn area(size: lumen_gpu::Size) -> u64 {
    u64::from(size.width) * u64::from(size.height)
}

/// Records `sweep` of `source` as short boxes along the path, then a comb of them, crossfaded
/// with every tap at once by `two_pass_weight`, which sources sized to their bounds share with
/// full-frame sources.
fn record(
    cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    source: crate::gpu::frame_graph::Raster,
    sweep: Sweep,
) -> crate::Result<crate::gpu::frame_graph::Raster> {
    use lumen_gpu::Res;

    if sweep.span <= 0.0 {
        return Ok(source);
    }
    let display = source.domain.display_size();
    let canvas = [
        0.0,
        0.0,
        f64::from(display.width),
        f64::from(display.height),
    ];
    let longest = sweep.longest_streak(canvas);
    let (inner, comb) = tap_counts(longest);
    let inner_half = sweep.span / (2 * comb) as f64;
    let comb_half = sweep.span / 2.0;
    let clamps = sweep.edge == EdgeMode::Clamp;

    // What the source reads as: clamped edges continue it past the canvas, as far as any tap of
    // a canvas pixel reaches.
    let data = source.domain.data_rect;
    let mut extended = bounds_of(data);
    if clamps && !data.is_empty() {
        let reach = longest + 2.0;
        for side in 0..2 {
            if extended[side] <= 0.0 {
                extended[side] -= reach;
            }
            if extended[side + 2] >= canvas[side + 2] {
                extended[side + 2] += reach;
            }
        }
    }
    // Each bound takes in the bilinear footprint of the taps that read into it.
    let inner_bounds = grow(sweep.swept(grow(extended, 1.0), inner_half), 1.0);
    let output_bounds = grow(sweep.swept(grow(inner_bounds, 1.0), comb_half), 1.0);
    let domain = cx.domain_within(
        display,
        cx.bounds_of(Some(output_bounds.map(|value| value as f32))),
    );

    // The short boxes are stored wherever the comb reads them, off the canvas too, as only the
    // source's reads clamp.
    let read_of = |bounds: Bounds| grow(sweep.swept(bounds, comb_half), 1.0);
    let max_extent = cx.rec.device().limits().max_texture_dimension_2d;
    let blend = two_pass_weight(longest, read_of(canvas), display, max_extent);
    let boxes = (blend > 0.0)
        .then(|| {
            rect_covering(intersect(
                read_of(bounds_of(domain.data_rect)),
                grow(inner_bounds, 1.0),
            ))
        })
        .flatten()
        .map(|rect| {
            let size = lumen_gpu::Size::new(
                frame_graph::size_class(rect.width.max(1)).min(max_extent),
                frame_graph::size_class(rect.height.max(1)).min(max_extent),
            );
            (rect, size)
        });
    let boxes_origin = boxes.map_or([0, 0], |(rect, _)| [rect.x, rect.y]);
    let uniform = cx.rec.uniform(&GpuSweepParams {
        header: [inner as u32, u32::from(clamps), comb as u32, 0],
        source: [data.x, data.y, display.width as i32, display.height as i32],
        origins: [
            boxes_origin[0],
            boxes_origin[1],
            domain.data_rect.x,
            domain.data_rect.y,
        ],
        blend: [if boxes.is_some() { blend } else { 0.0 }, 0.0, 0.0, 0.0],
        taps: taps(&sweep, inner, sweep.span / comb as f64),
        comb: taps(&sweep, comb, sweep.span),
    });
    let sampler = Res::Sampler(cx.sampler(1, || lumen_gpu::wgpu::SamplerDescriptor {
        mag_filter: lumen_gpu::wgpu::FilterMode::Linear,
        min_filter: lumen_gpu::wgpu::FilterMode::Linear,
        ..Default::default()
    }));

    // Without stored boxes the comb pass reads only the source, which stands in for them.
    let mut boxes_tex = source.tex;
    if let Some((_, size)) = boxes {
        boxes_tex = cx.float_target(size);
        let program = cx.program("directional-blur", SHADER, "boxes_main", BOXES_LAYOUT)?;
        cx.dispatch(
            program,
            &[
                (0, Res::Tex(source.tex)),
                (1, uniform),
                (2, sampler),
                (3, Res::Tex(boxes_tex)),
            ],
            params::dispatch_for(size),
        );
    }
    let tex = cx.storage_target(domain.storage_size);
    let program = cx.program("directional-blur", SHADER, "comb_main", COMB_LAYOUT)?;
    cx.dispatch(
        program,
        &[
            (0, Res::Tex(source.tex)),
            (1, uniform),
            (2, sampler),
            (4, Res::Tex(tex)),
            (5, Res::Tex(boxes_tex)),
        ],
        params::dispatch_for(domain.storage_size),
    );
    Ok(frame_graph::Raster {
        tex,
        domain,
        bounds: None,
    })
}

macro_rules! sweep_node {
    ($node:ty, |$params:ident| $sweep:expr) => {
        impl crate::gpu::frame_graph::RecordNode for $node {
            fn programs(
                &self,
                programs: &mut crate::gpu::frame_graph::Programs<'_>,
            ) -> crate::Result<()> {
                self::programs(programs)
            }

            fn record(
                &self,
                cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
            ) -> crate::Result<crate::gpu::frame_graph::Raster> {
                let source = cx.required(self.id, "source", self.source)?;
                let $params = self.params.eval(&DelegateEvalContext {
                    node_id: self.id,
                    expr: &cx.expr(),
                })?;
                self::record(cx, source, $sweep)
            }
        }
    };
}

fn center(center: (f64, f64)) -> [f64; 2] {
    [center.0, center.1]
        .map(|axis| finite_or_zero(axis).clamp(-MAX_BOUNDED_MAGNITUDE, MAX_BOUNDED_MAGNITUDE))
}

sweep_node!(DirectionalBlur, |params| {
    let (sin, cos) = finite_or_zero(params.angle).to_radians().sin_cos();
    Sweep {
        path: Path::Line {
            direction: [cos, sin],
        },
        span: finite_or_zero(params.distance)
            .abs()
            .min(MAX_BOUNDED_MAGNITUDE),
        edge: params.edge,
    }
});

sweep_node!(RadialBlur, |params| Sweep {
    path: Path::Spin {
        center: center(params.center),
    },
    span: finite_or_zero(params.amount).abs().min(360.0).to_radians(),
    edge: params.edge,
});

sweep_node!(ZoomBlur, |params| Sweep {
    path: Path::Zoom {
        center: center(params.center),
    },
    // A span of `2 asinh(amount / 2)` in log scale reaches `amount` times the distance.
    span: 2.0 * (finite_or_zero(params.amount).abs().min(1e3) / 2.0).asinh(),
    edge: params.edge,
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_counts_cover_the_streak_and_cap() {
        for (streak, expected) in [
            (0.0, (1, 1)),
            (9.0, (3, 3)),
            (10.0, (3, 4)),
            (4096.0, (64, 64)),
            (1e9, (64, 64)),
        ] {
            let (inner, comb) = tap_counts(streak);
            assert_eq!((inner, comb), expected, "streak {streak}");
            assert!(inner * comb >= streak.min(4096.0) as usize);
        }
    }

    #[test]
    fn swept_bounds_hold_every_tap_and_stay_tight() {
        let bounds = [100.0, 40.0, 130.0, 70.0];
        let center = [200.0, 50.0];
        for (path, span) in [
            (Path::Spin { center }, 0.3),
            (Path::Spin { center }, 7.0),
            (Path::Zoom { center }, 0.8),
            (
                Path::Line {
                    direction: [0.6, 0.8],
                },
                25.0,
            ),
        ] {
            let sweep = Sweep {
                path,
                span,
                edge: EdgeMode::Clamp,
            };
            let swept = sweep.swept(bounds, span / 2.0);
            let mut reached = [
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ];
            for step in 0..=200 {
                let m = sweep.affine((f64::from(step) / 200.0 - 0.5) * span);
                for [x, y] in corners(bounds) {
                    let [px, py] = [m[0] * x + m[1] * y + m[4], m[2] * x + m[3] * y + m[5]];
                    assert!(
                        px >= swept[0] - 1e-9 && py >= swept[1] - 1e-9,
                        "{path:?}: ({px}, {py}) is outside {swept:?}"
                    );
                    assert!(px <= swept[2] + 1e-9 && py <= swept[3] + 1e-9);
                    reached = [
                        reached[0].min(px),
                        reached[1].min(py),
                        reached[2].max(px),
                        reached[3].max(py),
                    ];
                }
            }
            for (bound, reached) in swept.iter().zip(reached) {
                assert!(
                    (bound - reached).abs() < 0.5,
                    "{path:?}: {swept:?} against {reached:?}"
                );
            }
        }
    }
}
