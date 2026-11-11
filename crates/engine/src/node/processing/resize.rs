use crate::node::{
    DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, processing::RasterSampling,
};

use crate::gpu::params;

pub(crate) const SHADER: &str = include_str!("resize.wgsl");

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum")]
pub enum ResizeMode {
    #[default]
    Stretch = 0,
    Fit = 1,
    Fill = 2,
}

/// Resamples a raster into a rectangle placed on the composition canvas.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct ResizeParams {
    /// Left edge of the output rectangle in pixels.
    #[meta(step = 1)]
    pub x: i64,
    /// Top edge of the output rectangle in pixels.
    #[meta(step = 1)]
    pub y: i64,
    /// Output width in pixels.
    #[meta(min = 1, step = 1)]
    pub width: i64,
    /// Output height in pixels.
    #[meta(min = 1, step = 1)]
    pub height: i64,
    /// How the source raster should fit the output bounds.
    #[meta(kind = "enum", enum_type = ResizeMode)]
    pub mode: ResizeMode,
    /// Sampling filter used when resizing.
    #[meta(kind = "enum", enum_type = RasterSampling)]
    pub sampling: RasterSampling,
}

impl Default for ResizeParams {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            mode: ResizeMode::Stretch,
            sampling: RasterSampling::Linear,
        }
    }
}

/// Resamples a raster into a rectangle placed on the composition canvas.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "resize", name = "Resize", category = "processing")]
pub struct Resize {
    pub id: NodeId,
    #[params]
    pub params: ResizeParamsDelegate,

    #[input()]
    pub source: Option<NodeId>,
}

const RECORD_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(1, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(
        2,
        crate::gpu::frame_graph::RECORD_STORAGE_TEXTURE,
    ),
    lumen_gpu::LayoutEntry::compute(3, lumen_gpu::LayoutKind::Uniform),
];

impl crate::gpu::frame_graph::RecordNode for Resize {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute("resize", SHADER, "cs_main", RECORD_LAYOUT)
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let source = cx.required_full(self.id, "source", self.source)?;
        let evaluated = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: &cx.expr(),
        })?;
        let params = params::ResizeParams {
            origin: [
                params::pixel_origin(self.id, "x", evaluated.x)?,
                params::pixel_origin(self.id, "y", evaluated.y)?,
            ],
            size: [
                params::pixel_extent(self.id, "width", evaluated.width, 1)?,
                params::pixel_extent(self.id, "height", evaluated.height, 1)?,
            ],
            mode: evaluated.mode as u32,
            sampling: evaluated.sampling as u32,
            _pad: [0; 2],
        };
        // Sources that don't publish bounds fill their whole storage texture.
        let bounds = source.bounds.unwrap_or(params::CropParams {
            origin: [0, 0],
            size: [
                source.domain.storage_size.width,
                source.domain.storage_size.height,
            ],
        });
        let size = cx.canvas_size();
        let tex = cx.storage_target(size);
        let program = cx.program("resize", SHADER, "cs_main", RECORD_LAYOUT)?;
        let uniform = cx.rec.uniform(&params);
        let bounds = cx.rec.uniform(&bounds);
        cx.dispatch(
            program,
            &[
                (0, lumen_gpu::Res::Tex(source.tex)),
                (1, uniform),
                (2, lumen_gpu::Res::Tex(tex)),
                (3, bounds),
            ],
            params::dispatch_for(size),
        );
        Ok(crate::gpu::frame_graph::Raster {
            tex,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            bounds: None,
        })
    }
}
