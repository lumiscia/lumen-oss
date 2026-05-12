use crate::node::{NodeId, NodeProperty, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
    RasterHandle, compiler,
};

pub(crate) const SHADER: &str = include_str!("resize.wgsl");

#[derive(Debug, Clone, Copy, PartialEq, Eq, lumen_macros::NodeEnum)]
#[repr(i64)]
pub enum ResizeMode {
    Stretch = 0,
    Fit = 1,
    Fill = 2,
}

impl ResizeMode {
    pub fn from_int(value: i64) -> Self {
        match value {
            1 => Self::Fit,
            2 => Self::Fill,
            _ => Self::Stretch,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, lumen_macros::NodeEnum)]
#[repr(i64)]
pub enum ResizeSampling {
    Nearest = 0,
    Linear = 1,
}

impl ResizeSampling {
    pub fn from_int(value: i64) -> Self {
        if value == Self::Nearest as i64 {
            Self::Nearest
        } else {
            Self::Linear
        }
    }
}

/// Resamples a raster into static output bounds.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "resize", name = "Resize", category = "processing")]
pub struct Resize {
    pub id: NodeId,
    /// Output width in pixels.
    #[property(kind = "int", min = 1, step = 1)]
    pub width: NodeProperty,
    /// Output height in pixels.
    #[property(kind = "int", min = 1, step = 1)]
    pub height: NodeProperty,
    /// How the source raster should fit the output bounds.
    #[property(kind = "enum", enum_type = ResizeMode)]
    pub mode: NodeProperty,
    /// Sampling filter used when resizing.
    #[property(kind = "enum", enum_type = ResizeSampling)]
    pub sampling: NodeProperty,
    #[input()]
    pub source: PortRef,
}

impl Default for Resize {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            width: NodeProperty::Int(1),
            height: NodeProperty::Int(1),
            mode: NodeProperty::Int(ResizeMode::Stretch as i64),
            sampling: NodeProperty::Int(ResizeSampling::Linear as i64),
            source: PortRef::empty(),
        }
    }
}

impl GpuCompileNode for Resize {
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
            ctx.static_dimension(&self.width, self.id, "width")?,
            ctx.static_dimension(&self.height, self.id, "height")?,
        );
        let texture = ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("resize:{}:output", self.id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let params = ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("resize:{}:params", self.id.0)),
            lumen_gpu::BufferDesc::uniform(std::mem::size_of::<compiler::ResizeParams>() as u64),
        );
        let program = ctx.spatial_program(self.id, "resize", SHADER);
        ctx.builder_mut().compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("resize:{}:apply", self.id.0)),
            owner: Some(lumen_gpu::NodeKey(self.id.0)),
            program,
            bindings: compiler::spatial_bindings(source.texture, params, texture),
            dispatch: compiler::dispatch_for(size),
        });
        ctx.builder_mut().param(
            lumen_gpu::ParamKey {
                owner: lumen_gpu::NodeKey(self.id.0),
                slot: 0,
            },
            lumen_gpu::ParamTarget::Buffer(params),
        );
        ctx.push_frame_binding(FrameBinding::Resize {
            node_id: self.id,
            width: self.width.clone(),
            height: self.height.clone(),
            mode: self.mode.clone(),
            sampling: self.sampling.clone(),
            buffer: params,
        });

        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            metadata: source.metadata,
        }))
    }
}

impl GpuFrameBindNode for Resize {
    fn bind_gpu_frame(
        &self,
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        let FrameBinding::Resize {
            node_id,
            width,
            height,
            mode,
            sampling,
            buffer,
        } = binding
        else {
            return Ok(());
        };
        let params = compiler::ResizeParams {
            size: [
                width
                    .resolve_int(*node_id, "width", &ctx.expr_context(*node_id, "width"))?
                    .max(1) as u32,
                height
                    .resolve_int(*node_id, "height", &ctx.expr_context(*node_id, "height"))?
                    .max(1) as u32,
            ],
            mode: ResizeMode::from_int(mode.resolve_int(
                *node_id,
                "mode",
                &ctx.expr_context(*node_id, "mode"),
            )?) as u32,
            sampling: ResizeSampling::from_int(sampling.resolve_int(
                *node_id,
                "sampling",
                &ctx.expr_context(*node_id, "sampling"),
            )?) as u32,
        };
        bound.write_buffer(*buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}
