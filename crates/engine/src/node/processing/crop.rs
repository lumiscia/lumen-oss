use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams};

use crate::gpu::params;

pub(crate) const SHADER: &str = include_str!("crop.wgsl");

/// Keeps a raster region in place and clears everything outside it.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct CropParams {
    /// Left edge of the crop region in pixels.
    #[meta(step = 1)]
    pub x: i64,
    /// Top edge of the crop region in pixels.
    #[meta(step = 1)]
    pub y: i64,
    /// Width of the crop region in pixels.
    #[meta(min = 0, step = 1)]
    pub width: i64,
    /// Height of the crop region in pixels.
    #[meta(min = 0, step = 1)]
    pub height: i64,
}

impl Default for CropParams {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        }
    }
}

/// Keeps a raster region in place and clears everything outside it.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "crop", name = "Crop", category = "processing")]
pub struct Crop {
    pub id: NodeId,
    #[params]
    pub params: CropParamsDelegate,

    #[input()]
    pub source: Option<NodeId>,
}

impl crate::gpu::frame_graph::RecordNode for Crop {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.spatial("crop", SHADER)
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let source = cx.required_full(self.id, "source", self.source)?;
        let params = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: &cx.expr(),
        })?;
        let x = params::pixel_origin(self.id, "x", params.x)?;
        let y = params::pixel_origin(self.id, "y", params.y)?;
        let width = params::pixel_extent(self.id, "width", params.width, 0)?;
        let height = params::pixel_extent(self.id, "height", params.height, 0)?;
        let size = source.domain.storage_size;
        let left = i64::from(x).clamp(0, i64::from(size.width));
        let top = i64::from(y).clamp(0, i64::from(size.height));
        let right = (i64::from(x) + i64::from(width)).clamp(left, i64::from(size.width));
        let bottom = (i64::from(y) + i64::from(height)).clamp(top, i64::from(size.height));
        let gpu_params = params::CropParams {
            origin: [left as i32, top as i32],
            size: [(right - left) as u32, (bottom - top) as u32],
        };
        let tex = cx.storage_target(size);
        cx.spatial_pass("crop", SHADER, source.tex, &gpu_params, tex, size)?;
        Ok(crate::gpu::frame_graph::Raster {
            tex,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            // The region is clamped to the source, so it is the output's logical bounds.
            bounds: Some(gpu_params),
        })
    }
}
