use crate::node::{Deferred, NodeId, NodeParams, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuFrameBinding, RasterHandle,
    compiler,
};

pub(crate) const SHADER: &str = include_str!("crop.wgsl");

/// Extracts a fixed raster region into static output bounds.
#[derive(Debug, Clone, lumen_macros::NodeParams)]
#[params(evaluated = EvaluatedCropParams)]
#[cfg_attr(feature = "json", derive(serde::Deserialize), serde(default))]
pub struct CropParams {
    /// Left edge of the crop region in pixels.
    #[param(kind = "int", step = 1)]
    pub x: Deferred<i64>,
    /// Top edge of the crop region in pixels.
    #[param(kind = "int", step = 1)]
    pub y: Deferred<i64>,
    /// Width of the crop region in pixels.
    #[param(kind = "int", min = 0, step = 1)]
    pub width: Deferred<i64>,
    /// Height of the crop region in pixels.
    #[param(kind = "int", min = 0, step = 1)]
    pub height: Deferred<i64>,
}

impl Default for CropParams {
    fn default() -> Self {
        Self {
            x: Deferred::value(0),
            y: Deferred::value(0),
            width: Deferred::value(1),
            height: Deferred::value(1),
        }
    }
}

/// Extracts a fixed raster region into static output bounds.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "crop", name = "Crop", category = "processing")]
pub struct Crop {
    pub id: NodeId,
    #[params]
    pub params: CropParams,

    #[input()]
    pub source: PortRef,
}

impl Default for Crop {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: CropParams::default(),
            source: PortRef::empty(),
        }
    }
}

impl GpuCompileNode for Crop {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        if port.port != "output" {
            return Err(ctx.missing_output(self.id, &port.port));
        }

        let source = ctx
            .compile_port(&self.source)?
            .into_raster(self.source.id, &self.source.port)?;
        let size = lumen_gpu::Size::new(
            ctx.static_dimension(&self.params.width, self.id, "width")?,
            ctx.static_dimension(&self.params.height, self.id, "height")?,
        );
        let texture = ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("crop:{}:output", self.id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let params = ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("crop:{}:params", self.id.0)),
            lumen_gpu::BufferDesc::uniform(std::mem::size_of::<compiler::CropParams>() as u64),
        );
        let program = ctx.spatial_program(self.id, "crop", SHADER);
        ctx.builder_mut().compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("crop:{}:apply", self.id.0)),
            owner: Some(lumen_gpu::NodeKey(self.id.0)),
            program,
            bindings: compiler::spatial_bindings(source.texture, params, texture),
            dispatch: compiler::dispatch_for(size).into(),
        });
        ctx.builder_mut().param(
            lumen_gpu::ParamKey {
                owner: lumen_gpu::NodeKey(self.id.0),
                slot: 0,
            },
            lumen_gpu::ParamTarget::Buffer(params),
        );
        ctx.push_frame_binding(CropFrameBinding {
            node_id: self.id,
            x: self.params.x.clone(),
            y: self.params.y.clone(),
            width: self.params.width.clone(),
            height: self.params.height.clone(),
            buffer: params,
        });

        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            metadata: source.metadata,
        }))
    }
}

#[derive(Debug, Clone)]
struct CropFrameBinding {
    node_id: NodeId,
    x: Deferred<i64>,
    y: Deferred<i64>,
    width: Deferred<i64>,
    height: Deferred<i64>,
    buffer: lumen_gpu::BufferId,
}

impl GpuFrameBinding for CropFrameBinding {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let params = compiler::CropParams {
            origin: [
                self.x
                    .resolve_int(self.node_id, "x", &ctx.expr_context(self.node_id, "x"))?
                    as i32,
                self.y
                    .resolve_int(self.node_id, "y", &ctx.expr_context(self.node_id, "y"))?
                    as i32,
            ],
            size: [
                self.width
                    .resolve_int(
                        self.node_id,
                        "width",
                        &ctx.expr_context(self.node_id, "width"),
                    )?
                    .max(0) as u32,
                self.height
                    .resolve_int(
                        self.node_id,
                        "height",
                        &ctx.expr_context(self.node_id, "height"),
                    )?
                    .max(0) as u32,
            ],
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}
