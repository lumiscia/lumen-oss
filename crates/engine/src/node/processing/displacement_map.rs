use bytemuck::{Pod, Zeroable};

use crate::gpu::params;
use crate::node::{
    DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, processing::RasterSampling,
};

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/sampling.wgsl"),
    include_str!("displacement_map.wgsl")
);

/// The map channel that drives one axis of a displacement.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum", expressions)]
pub enum DisplacementChannel {
    #[default]
    Red = 0,
    Green = 1,
    Blue = 2,
    /// Rec. 709 luma.
    Luma = 3,
}

/// What a raster reads as beyond its edges.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum", expressions)]
pub enum EdgeMode {
    /// Repeats the edge pixels.
    #[default]
    Clamp = 0,
    /// Reads as transparent.
    Transparent = 1,
}

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct DisplacementMapParams {
    /// Pixels the source moves horizontally at full strength: a channel byte of 0 moves it this
    /// far left, 255 nearly as far right, and 128 leaves it in place. Negative amounts reverse it.
    #[meta(name = "Amount X", step = 1)]
    pub amount_x: f64,
    /// Pixels the source moves vertically at full strength: a channel byte of 0 moves it this far
    /// up, 255 nearly as far down, and 128 leaves it in place. Negative amounts reverse it.
    #[meta(name = "Amount Y", step = 1)]
    pub amount_y: f64,
    /// Map channel that drives horizontal displacement.
    #[meta(name = "Channel X")]
    pub channel_x: DisplacementChannel,
    /// Map channel that drives vertical displacement.
    #[meta(name = "Channel Y")]
    pub channel_y: DisplacementChannel,
    /// What the source reads as beyond its edges.
    #[meta()]
    pub edge: EdgeMode,
    /// Sampling filter used to read the displaced source.
    #[meta(kind = "enum", enum_type = RasterSampling)]
    pub sampling: RasterSampling,
}

impl Default for DisplacementMapParams {
    fn default() -> Self {
        Self {
            amount_x: 10.0,
            amount_y: 10.0,
            channel_x: DisplacementChannel::Red,
            channel_y: DisplacementChannel::Green,
            edge: EdgeMode::Clamp,
            sampling: RasterSampling::Linear,
        }
    }
}

/// Moves each pixel of `source` by the `map` pixel over it, in canvas pixels. Map bytes are
/// premultiplied by alpha, so transparent areas of the map, and everything outside it, leave
/// the source in place.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "displacement_map",
    name = "Displacement Map",
    category = "processing"
)]
pub struct DisplacementMap {
    pub id: NodeId,
    #[params]
    pub params: DisplacementMapParamsDelegate,
    #[input()]
    pub source: Option<NodeId>,
    #[input()]
    pub map: Option<NodeId>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuDisplacementParams {
    amount: [f32; 4],
    modes: [u32; 4],
    origins: [i32; 4],
    source: [i32; 4],
}

const RECORD_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(1, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(3, crate::gpu::frame_graph::RECORD_STORAGE_TEXTURE),
];

/// Amounts beyond this move every pixel off any canvas, so they clamp to it.
const MAX_AMOUNT: f64 = 1e6;

fn amount(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(-MAX_AMOUNT, MAX_AMOUNT)
    } else {
        0.0
    }
}

/// `rect` grown by `reach` pixels on each axis, as the box `FrameCx::bounds_of` takes.
fn grown(rect: lumen_gpu::RectI, reach: [f64; 2]) -> [f32; 4] {
    let (x, y) = (f64::from(rect.x), f64::from(rect.y));
    [
        x - reach[0],
        y - reach[1],
        x + f64::from(rect.width) + reach[0],
        y + f64::from(rect.height) + reach[1],
    ]
    .map(|value| value as f32)
}

impl crate::gpu::frame_graph::RecordNode for DisplacementMap {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute("displacement-map", SHADER, "cs_main", RECORD_LAYOUT)
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let source = cx.required(self.id, "source", self.source)?;
        let map = cx.required(self.id, "map", self.map)?;
        let params = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: &cx.expr(),
        })?;
        let amount = [amount(params.amount_x), amount(params.amount_y)];
        // A pixel moves at most `amount`, and linear sampling reads one pixel further. Only
        // pixels under the map move, so the source grows only where it overlaps the map.
        let reach = amount.map(|amount| amount.abs().ceil() + 1.0);
        let data = source.domain.data_rect;
        let bounds = cx
            .bounds_of(Some(grown(data, reach)))
            .map(|reached| data.union(reached.intersection(map.domain.data_rect)));
        let display = source.domain.display_size();
        let domain = cx.domain_within(display, bounds);
        let gpu_params = GpuDisplacementParams {
            amount: [
                (amount[0] / 128.0) as f32,
                (amount[1] / 128.0) as f32,
                0.0,
                0.0,
            ],
            modes: [
                params.channel_x as u32,
                params.channel_y as u32,
                params.edge as u32,
                params.sampling as u32,
            ],
            origins: [
                domain.data_rect.x,
                domain.data_rect.y,
                map.domain.data_rect.x,
                map.domain.data_rect.y,
            ],
            source: [data.x, data.y, display.width as i32, display.height as i32],
        };
        let size = domain.storage_size;
        let tex = cx.storage_target(size);
        let program = cx.program("displacement-map", SHADER, "cs_main", RECORD_LAYOUT)?;
        let uniform = cx.rec.uniform(&gpu_params);
        cx.dispatch(
            program,
            &[
                (0, lumen_gpu::Res::Tex(source.tex)),
                (1, lumen_gpu::Res::Tex(map.tex)),
                (2, uniform),
                (3, lumen_gpu::Res::Tex(tex)),
            ],
            params::dispatch_for(size),
        );
        Ok(crate::gpu::frame_graph::Raster {
            tex,
            domain,
            bounds: None,
        })
    }
}
