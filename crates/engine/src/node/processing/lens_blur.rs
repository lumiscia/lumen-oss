use bytemuck::{Pod, Zeroable};

use crate::error::RenderError;
use crate::gpu::frame_graph::{ComputeProgram, RECORD_STORAGE_TEXTURE, Raster, dispatch_for};
use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, OutputRef};

const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/sampling.wgsl"),
    include_str!("lens_blur.wgsl")
);

/// The largest circle, in texels of the level it is gathered on, before the blur moves to a level
/// of half the resolution. A gather reads at most about π(6.5)² ≈ 133 texels, and about 170 while
/// it crossfades into the next level. Mirrored in `lens_blur.wgsl`.
const GATHER_TEXELS: f64 = 6.0;

/// Levels the shader's tables hold: full resolution and 14 halvings.
const MAX_LEVELS: usize = 15;

/// The widest blur, in canvas pixels.
const MAX_RADIUS: f64 = 4096.0;

/// The widest blur in device pixels, whose levels all fit the shader's tables.
const MAX_DEVICE_RADIUS: f64 = (1u32 << (MAX_LEVELS - 1)) as f64;

/// Most aperture blades; more than this looks round.
const MAX_BLADES: i64 = 16;

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct LensBlurParams {
    /// Blur radius in pixels: the blur everywhere without `depth`, and the largest blur with it.
    /// 0 leaves the source as it is. Radii above 4096 look the same.
    #[meta(min = 0, step = 0.5, pixels)]
    pub radius: f64,
    /// The depth that stays sharp, in the depth raster's 0 to 1 range. For a `render_3d`'s
    /// `depth`, 0 is at its `depth_near` and 1 at its `depth_far`, so a distance in world units
    /// is `(distance - depth_near) / (depth_far - depth_near)`. Unused without `depth`.
    #[meta(min = 0, max = 1, step = 0.01)]
    pub focus: f64,
    /// How far either side of `focus`, in the same range, depth stays sharp.
    #[meta(min = 0, max = 1, step = 0.01)]
    pub focus_range: f64,
    /// How fast the blur grows past `focus_range`: a pixel `aperture × (|depth - focus| -
    /// focus_range)` of the way to `radius`, up to all of it. Higher is a shallower depth of
    /// field.
    #[meta(min = 0, step = 0.1)]
    pub aperture: f64,
    /// Blades of the lens's aperture, which shape a blurred highlight: 0 is a circle, and 3 to
    /// 16 a polygon with that many sides.
    #[meta(min = 0, max = 16, step = 1)]
    pub blades: i64,
    /// Turn of the polygon in degrees, clockwise. At 0 one corner points up.
    #[meta(step = 1)]
    pub rotation: f64,
}

impl Default for LensBlurParams {
    fn default() -> Self {
        Self {
            radius: 8.0,
            focus: 0.5,
            focus_range: 0.05,
            aperture: 4.0,
            blades: 0,
            rotation: 0.0,
        }
    }
}

/// Blurs a raster as an out-of-focus lens does: every point spreads into an evenly lit disc, or a
/// polygon with `blades`, so highlights turn into bokeh rather than the soft glow of `blur`.
///
/// Without `depth`, everything blurs by `radius`. With it, each pixel blurs by its depth: sharp
/// within `focus_range` of `focus`, and more so further away, up to `radius`. Depth is the red
/// channel of `depth`, read where each pixel is, and 0 where it stores nothing, so a
/// `render_3d`'s `depth` output gives depth of field and any greyscale raster works, as a
/// gradient that is dark in a band across the frame gives tilt-shift. `focus` is in that 0 to 1
/// range, not in world units. A blurred pixel nearer than its neighbours spreads over them even
/// where they are sharp, and one further away doesn't spread over nearer, sharper pixels, so a
/// sharp subject keeps its edges in front of a blurred background.
///
/// The cost stays bounded at any radius: circles of more than 6 pixels are gathered from copies
/// of the source at half, a quarter or less of its resolution, so each pixel reads about 130
/// texels at most, twice that with `depth`. The widest blurs have softer bokeh edges. Blurring
/// widens the output past the source's area by the radius.
///
/// Defocusing a whole frame into hexagonal bokeh:
///
/// ```json
/// { "id": 2, "type": "lens_blur", "params": { "radius": 12, "blades": 6 }, "in": { "source": 1 } }
/// ```
///
/// ```ts
/// n.lens_blur({ source: photo, radius: 12, blades: 6 });
/// ```
///
/// Depth of field from a `render_3d`, node 9, sharp at a fifth of the way from its `depth_near`
/// to its `depth_far`:
///
/// ```json
/// { "id": 10, "type": "lens_blur", "params": { "radius": 16, "focus": 0.2 } },
/// { "from_node": 9, "from_port": "color", "to_node": 10, "to_port": "source" },
/// { "from_node": 9, "from_port": "depth", "to_node": 10, "to_port": "depth" }
/// ```
///
/// ```ts
/// const depth = outputOf(render, "depth");
/// n.lens_blur({ source: outputOf(render, "color"), depth, radius: 16, focus: 0.2 });
/// ```
///
/// Tilt-shift, sharp across the middle of the frame and blurred towards the top and bottom:
///
/// ```json
/// { "id": 2, "type": "background", "params": { "paint": { "type": "linear_gradient", "interpolation": "linear_srgb", "start": [0, 0], "end": [0, 1], "stops": [{ "offset": 0, "color": [255, 255, 255, 255] }, { "offset": 0.5, "color": [0, 0, 0, 255] }, { "offset": 1, "color": [255, 255, 255, 255] }] } } },
/// { "id": 3, "type": "lens_blur", "params": { "radius": 10, "focus": 0, "focus_range": 0.1, "aperture": 1.5 }, "in": { "source": 1, "depth": 2 } }
/// ```
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "lens_blur", name = "Lens Blur", category = "processing")]
pub struct LensBlur {
    pub id: NodeId,
    #[params]
    pub params: LensBlurParamsDelegate,
    #[input()]
    pub source: Option<OutputRef>,
    #[input(optional)]
    pub depth: Option<OutputRef>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "LensBlurParams", source = SHADER)]
struct GpuLensBlurParams {
    /// Radius in device pixels, focus, focus range and aperture.
    values: [f32; 4],
    /// Blades, rotation in radians, and 1 with a depth input.
    shape: [f32; 4],
    /// Canvas pixel of the output's first texel in `xy`, the source's in `zw`.
    origins: [i32; 4],
    /// Canvas pixel of the depth's first texel in `xy`.
    depth: [i32; 4],
    /// Levels built, and the level a reduction writes in `z`.
    levels: [i32; 4],
    /// Per level: where it starts in its texture in `xy`, and its first block on the canvas in
    /// `zw`.
    level_origin: [[i32; 4]; MAX_LEVELS],
    /// Per level: its size in texels in `xy`, and the canvas's size in its blocks in `zw`.
    level_size: [[i32; 4]; MAX_LEVELS],
}

const LEVEL_FORMAT: lumen_gpu::wgpu::TextureFormat = lumen_gpu::wgpu::TextureFormat::Rgba16Float;

const PREPARE: ComputeProgram = ComputeProgram {
    label: "lens-blur",
    shader: SHADER,
    entry: "prepare_main",
    layout: &[
        lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
        lumen_gpu::LayoutEntry::compute(1, lumen_gpu::LayoutKind::Texture),
        lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Uniform),
        lumen_gpu::LayoutEntry::compute(5, lumen_gpu::LayoutKind::StorageTexture(LEVEL_FORMAT)),
        lumen_gpu::LayoutEntry::compute(6, lumen_gpu::LayoutKind::StorageTexture(LEVEL_FORMAT)),
    ],
};
const REDUCE: ComputeProgram = ComputeProgram {
    label: "lens-blur",
    shader: SHADER,
    entry: "reduce_main",
    layout: &[
        lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Uniform),
        lumen_gpu::LayoutEntry::compute(3, lumen_gpu::LayoutKind::Texture),
        lumen_gpu::LayoutEntry::compute(4, lumen_gpu::LayoutKind::Texture),
        lumen_gpu::LayoutEntry::compute(5, lumen_gpu::LayoutKind::StorageTexture(LEVEL_FORMAT)),
        lumen_gpu::LayoutEntry::compute(6, lumen_gpu::LayoutKind::StorageTexture(LEVEL_FORMAT)),
        lumen_gpu::LayoutEntry::compute(14, lumen_gpu::LayoutKind::Texture),
        lumen_gpu::LayoutEntry::compute(15, lumen_gpu::LayoutKind::Texture),
        lumen_gpu::LayoutEntry::compute(16, lumen_gpu::LayoutKind::StorageTexture(LEVEL_FORMAT)),
        lumen_gpu::LayoutEntry::compute(17, lumen_gpu::LayoutKind::StorageTexture(LEVEL_FORMAT)),
    ],
};
const RESOLVE: ComputeProgram = ComputeProgram {
    label: "lens-blur",
    shader: SHADER,
    entry: "resolve_main",
    layout: &[
        lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Uniform),
        lumen_gpu::LayoutEntry::compute(7, lumen_gpu::LayoutKind::Texture),
        lumen_gpu::LayoutEntry::compute(8, lumen_gpu::LayoutKind::Texture),
        lumen_gpu::LayoutEntry::compute(9, lumen_gpu::LayoutKind::Texture),
        lumen_gpu::LayoutEntry::compute(10, lumen_gpu::LayoutKind::Texture),
        lumen_gpu::LayoutEntry::compute(11, lumen_gpu::LayoutKind::Texture),
        lumen_gpu::LayoutEntry::compute(12, lumen_gpu::LayoutKind::Texture),
        lumen_gpu::LayoutEntry::compute(13, RECORD_STORAGE_TEXTURE),
        lumen_gpu::LayoutEntry::compute(18, lumen_gpu::LayoutKind::Texture),
        lumen_gpu::LayoutEntry::compute(19, lumen_gpu::LayoutKind::Texture),
        lumen_gpu::LayoutEntry::compute(20, lumen_gpu::LayoutKind::Texture),
        lumen_gpu::LayoutEntry::compute(21, lumen_gpu::LayoutKind::Texture),
    ],
};

fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() { value } else { fallback }
}

/// The level a circle of `radius` device pixels is gathered on, where it is at most
/// [`GATHER_TEXELS`] texels.
fn gather_level(radius: f64) -> u32 {
    (radius / GATHER_TEXELS).log2().ceil().max(0.0) as u32
}

/// Where level `level` of a pyramid over the canvas pixels `region` of a `display` canvas sits.
/// Levels above 0 hold one more block on each side, which past the canvas's edge holds the edge
/// pixels repeated, as every block further out would.
#[derive(Debug, Clone, Copy)]
struct Level {
    first: [i32; 2],
    size: lumen_gpu::Size,
    canvas_blocks: [i32; 2],
}

impl Level {
    fn new(region: lumen_gpu::RectI, display: lumen_gpu::Size, level: u32) -> Self {
        let block = 1u32 << level;
        let pad = u32::from(level > 0);
        let span = |origin: i32, extent: u32| {
            let origin = origin.max(0) as u32;
            let first = origin / block;
            let end = (origin + extent.max(1)).div_ceil(block);
            (first as i32 - pad as i32, end - first + 2 * pad)
        };
        let (x, width) = span(region.x, region.width);
        let (y, height) = span(region.y, region.height);
        Self {
            first: [x, y],
            size: lumen_gpu::Size::new(width, height),
            canvas_blocks: [
                display.width.div_ceil(1 << level) as i32,
                display.height.div_ceil(1 << level) as i32,
            ],
        }
    }
}

impl crate::gpu::frame_graph::RecordNode for LensBlur {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute(&PREPARE)?;
        programs.compute(&REDUCE)?;
        programs.compute(&RESOLVE)
    }

    fn record(&self, cx: &mut crate::gpu::frame_graph::FrameCx<'_>) -> crate::Result<Raster> {
        let source = cx.required(self.id, "source", self.source)?;
        let params = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: &cx.expr(),
            pixel_scale: cx.pixel_scale(),
        })?;
        if params.blades != 0 && !(3..=MAX_BLADES).contains(&params.blades) {
            return Err(RenderError::evaluating::<LensBlur>(
                self.id,
                format!(
                    "`blades` is {}; it must be 0 for a circle or from 3 to {MAX_BLADES}",
                    params.blades
                ),
            )
            .into());
        }
        let radius = finite_or(params.radius, 0.0)
            .clamp(0.0, MAX_RADIUS * cx.pixel_scale())
            .min(MAX_DEVICE_RADIUS);
        if radius == 0.0 {
            return Ok(source);
        }
        let depth = match self.depth {
            Some(depth) => Some(cx.input(depth)?),
            None => None,
        };

        // The coarsest level a gather reads: the one a crossfade at the radius reads.
        let top = gather_level(radius) + 1;
        let levels = top as usize + 1;
        // Light spreads from where it is by the radius, and a circle's edge is softened over half
        // a texel of the coarsest level past that.
        let reach = radius + f64::from(1u32 << top) / 2.0 + 1.0;
        let data = source.domain.data_rect;
        let display = source.domain.display_size();
        let domain = cx.domain_within(
            display,
            cx.bounds_of(Some(
                [
                    f64::from(data.x) - reach,
                    f64::from(data.y) - reach,
                    f64::from(data.x) + f64::from(data.width) + reach,
                    f64::from(data.y) + f64::from(data.height) + reach,
                ]
                .map(|value| value as f32),
            )),
        );
        let region = domain.data_rect;

        // Level 0 has textures of its own; odd and even levels above it are stacked down one
        // texture each, so a reduction never reads the texture it writes.
        let mut level_origin = [[0; 4]; MAX_LEVELS];
        let mut level_size = [[0; 4]; MAX_LEVELS];
        let mut atlas = [lumen_gpu::Size::new(1, 1); 2];
        let mut stacked = [0u32; 2];
        let pyramid: Vec<Level> = (0..levels as u32)
            .map(|level| Level::new(region, display, level))
            .collect();
        for (index, level) in pyramid.iter().enumerate() {
            let y = if index == 0 {
                0
            } else {
                let parity = index % 2;
                let y = stacked[parity];
                stacked[parity] += level.size.height;
                atlas[parity] = lumen_gpu::Size::new(
                    atlas[parity].width.max(level.size.width),
                    stacked[parity],
                );
                y
            };
            level_origin[index] = [0, y as i32, level.first[0], level.first[1]];
            level_size[index] = [
                level.size.width as i32,
                level.size.height as i32,
                level.canvas_blocks[0],
                level.canvas_blocks[1],
            ];
        }

        let depth_domain = depth.map(|depth| depth.domain);
        let gpu_params = |written: usize| GpuLensBlurParams {
            values: [
                radius as f32,
                finite_or(params.focus, 0.5) as f32,
                finite_or(params.focus_range, 0.0).max(0.0) as f32,
                finite_or(params.aperture, 0.0).max(0.0) as f32,
            ],
            shape: [
                params.blades as f32,
                finite_or(params.rotation, 0.0)
                    .rem_euclid(360.0)
                    .to_radians() as f32,
                f32::from(u8::from(depth.is_some())),
                0.0,
            ],
            origins: [region.x, region.y, data.x, data.y],
            depth: depth_domain.map_or([0; 4], |domain| {
                [domain.data_rect.x, domain.data_rect.y, 0, 0]
            }),
            levels: [levels as i32, 0, written as i32, 0],
            level_origin,
            level_size,
        };

        let level_color = cx.float_target(pyramid[0].size);
        let level_coc = cx.float_target(pyramid[0].size);
        let prepare = cx.program(&PREPARE)?;
        let uniform = cx.rec.uniform(&gpu_params(0));
        cx.dispatch(
            prepare,
            &[
                (0, lumen_gpu::Res::Tex(source.tex)),
                (
                    1,
                    lumen_gpu::Res::Tex(depth.map_or(source.tex, |depth| depth.tex)),
                ),
                (2, uniform),
                (5, lumen_gpu::Res::Tex(level_color)),
                (6, lumen_gpu::Res::Tex(level_coc)),
            ],
            dispatch_for(pyramid[0].size),
        );

        // Even levels start at 2 and odd ones at 1. Each holds colour, CoC, and the near layer's
        // colour and CoC; level 0 has no near layer, which the shader derives from its pixels.
        let unused = cx.unused_float();
        let mut atlases = [[unused; 4]; 2];
        for parity in 0..2 {
            if levels > 2 - parity {
                atlases[parity] = [(); 4].map(|()| cx.float_target(atlas[parity]));
            }
        }
        let reduce = cx.program(&REDUCE)?;
        for (index, level) in pyramid.iter().enumerate().skip(1) {
            let input = if index == 1 {
                [level_color, level_coc, unused, unused]
            } else {
                atlases[(index - 1) % 2]
            };
            let output = atlases[index % 2];
            let uniform = cx.rec.uniform(&gpu_params(index));
            cx.dispatch(
                reduce,
                &[
                    (2, uniform),
                    (3, lumen_gpu::Res::Tex(input[0])),
                    (4, lumen_gpu::Res::Tex(input[1])),
                    (5, lumen_gpu::Res::Tex(output[0])),
                    (6, lumen_gpu::Res::Tex(output[1])),
                    (14, lumen_gpu::Res::Tex(input[2])),
                    (15, lumen_gpu::Res::Tex(input[3])),
                    (16, lumen_gpu::Res::Tex(output[2])),
                    (17, lumen_gpu::Res::Tex(output[3])),
                ],
                dispatch_for(level.size),
            );
        }

        let size = domain.storage_size;
        let tex = cx.storage_target(size);
        let resolve = cx.program(&RESOLVE)?;
        let uniform = cx.rec.uniform(&gpu_params(0));
        cx.dispatch(
            resolve,
            &[
                (2, uniform),
                (7, lumen_gpu::Res::Tex(level_color)),
                (8, lumen_gpu::Res::Tex(level_coc)),
                (9, lumen_gpu::Res::Tex(atlases[1][0])),
                (10, lumen_gpu::Res::Tex(atlases[1][1])),
                (11, lumen_gpu::Res::Tex(atlases[0][0])),
                (12, lumen_gpu::Res::Tex(atlases[0][1])),
                (13, lumen_gpu::Res::Tex(tex)),
                (18, lumen_gpu::Res::Tex(atlases[1][2])),
                (19, lumen_gpu::Res::Tex(atlases[1][3])),
                (20, lumen_gpu::Res::Tex(atlases[0][2])),
                (21, lumen_gpu::Res::Tex(atlases[0][3])),
            ],
            dispatch_for(size),
        );
        Ok(Raster {
            tex,
            domain,
            bounds: None,
            reduced: None,
        })
    }

    fn magnification(&self) -> f64 {
        1.0
    }
}
