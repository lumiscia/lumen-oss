use bytemuck::{Pod, Zeroable};

use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, OutputRef};

use crate::gpu::frame_graph::ComputeProgram;
use crate::gpu::render_scale::device_rect;

pub(crate) const SHADER: &str = include_str!("crop.wgsl");
const CROP: ComputeProgram = ComputeProgram::spatial("crop", SHADER);

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
    pub source: Option<OutputRef>,
}

/// A crop over a source stored from `source_origin`, which is where its first texel sits in the
/// display.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "CropParams", source = SHADER)]
struct CropUniforms {
    origin: [i32; 2],
    size: [u32; 2],
    source_origin: [i32; 2],
}

/// Converts a signed pixel coordinate property to the `i32` the shaders use.
pub(super) fn pixel_origin(node_id: NodeId, property_path: &str, value: i64) -> crate::Result<i32> {
    i32::try_from(value).map_err(|_| {
        crate::error::PropertyError::InvalidType {
            at: crate::error::At::param(node_id, property_path),
            expected: "an integer between -2147483648 and 2147483647",
            actual: "out-of-range integer",
        }
        .into()
    })
}

/// Converts a pixel extent property to the `u32` the shaders use; values below `min` clamp to it.
pub(super) fn pixel_extent(
    node_id: NodeId,
    property_path: &str,
    value: i64,
    min: u32,
) -> crate::Result<u32> {
    u32::try_from(value.max(i64::from(min))).map_err(|_| {
        crate::error::PropertyError::InvalidType {
            at: crate::error::At::param(node_id, property_path),
            expected: "an integer between 0 and 4294967295",
            actual: "out-of-range integer",
        }
        .into()
    })
}

impl crate::gpu::frame_graph::RecordNode for Crop {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute(&CROP)
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let source = cx.required(self.id, "source", self.source)?;
        let params = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: &cx.expr(),
            pixel_scale: cx.pixel_scale(),
        })?;
        let [x, y, width, height] = device_rect(
            [
                params.x,
                params.y,
                params.width.max(0),
                params.height.max(0),
            ],
            cx.pixel_scale(),
        );
        let x = pixel_origin(self.id, "x", x)?;
        let y = pixel_origin(self.id, "y", y)?;
        let width = pixel_extent(self.id, "width", width, 0)?;
        let height = pixel_extent(self.id, "height", height, 0)?;
        let size = source.domain.display_size();
        let left = i64::from(x).clamp(0, i64::from(size.width));
        let top = i64::from(y).clamp(0, i64::from(size.height));
        let right = (i64::from(x) + i64::from(width)).clamp(left, i64::from(size.width));
        let bottom = (i64::from(y) + i64::from(height)).clamp(top, i64::from(size.height));
        let gpu_params = crate::gpu::frame_graph::CropParams {
            origin: [left as i32, top as i32],
            size: [(right - left) as u32, (bottom - top) as u32],
        };
        let pass_params = CropUniforms {
            origin: gpu_params.origin,
            size: gpu_params.size,
            source_origin: source.origin(),
        };
        let tex = cx.storage_target(size);
        cx.spatial_pass(&CROP, source.tex, &pass_params, tex, size)?;
        Ok(crate::gpu::frame_graph::Raster {
            tex,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            // The region is clamped to the source, so it is the output's logical bounds.
            bounds: Some(gpu_params),
            reduced: None,
        })
    }

    fn magnification(&self) -> f64 {
        1.0
    }
}
