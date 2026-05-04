use crate::node::{NodeId, NodeProperty, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
    RasterHandle, compiler,
};

pub(crate) const SHADER: &str = include_str!("boolean.wgsl");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum BooleanOperation {
    Union = 0,
    Intersect = 1,
    Subtract = 2,
    Xor = 3,
}

impl BooleanOperation {
    pub fn from_int(value: i64) -> Self {
        match value {
            1 => Self::Intersect,
            2 => Self::Subtract,
            3 => Self::Xor,
            _ => Self::Union,
        }
    }
}

#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "boolean",
    label = "Boolean",
    description = "Combines two raster alpha masks with boolean operations.",
    category = "compositing"
)]
pub struct Boolean {
    pub id: NodeId,
    #[property(kind = "int")]
    pub operation: NodeProperty,
    #[property(kind = "float")]
    pub threshold: NodeProperty,
    #[input()]
    pub a: PortRef,
    #[input()]
    pub b: PortRef,
}

impl Default for Boolean {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            operation: NodeProperty::Int(BooleanOperation::Union as i64),
            threshold: NodeProperty::Float(0.0),
            a: PortRef::empty(),
            b: PortRef::empty(),
        }
    }
}

impl GpuCompileNode for Boolean {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        if port.port != "output" {
            return Err(ctx.missing_output(self.id, &port.port));
        }

        let a = ctx
            .compile_port(&self.a)?
            .into_raster(self.a.id, &self.a.port)?;
        let b = ctx
            .compile_port(&self.b)?
            .into_raster(self.b.id, &self.b.port)?;
        let size = a.domain.storage_size;
        let texture = ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("boolean:{}:output", self.id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let params = ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("boolean:{}:params", self.id.0)),
            lumen_gpu::BufferDesc::uniform(std::mem::size_of::<compiler::BooleanParams>() as u64),
        );
        let program = ctx.builder_mut().program_for(
            lumen_gpu::NodeKey(self.id.0),
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some("boolean".to_string()),
                shader: SHADER.to_string(),
                entry: "cs_main".to_string(),
                bind_groups: lumen_gpu::BindGroupLayoutSpec::single(vec![
                    lumen_gpu::BindingLayoutEntry::texture(
                        0,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                    ),
                    lumen_gpu::BindingLayoutEntry::texture(
                        1,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                    ),
                    lumen_gpu::BindingLayoutEntry::uniform(
                        2,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                    ),
                    lumen_gpu::BindingLayoutEntry::storage_texture(
                        3,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                        lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                        lumen_gpu::wgpu::StorageTextureAccess::WriteOnly,
                    ),
                ]),
            }),
        );
        ctx.builder_mut().compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("boolean:{}:apply", self.id.0)),
            owner: Some(lumen_gpu::NodeKey(self.id.0)),
            program,
            bindings: vec![
                lumen_gpu::Binding::sampled_texture(0, 0, a.texture),
                lumen_gpu::Binding::sampled_texture(0, 1, b.texture),
                lumen_gpu::Binding::uniform(0, 2, params),
                lumen_gpu::Binding::storage_texture(0, 3, texture),
            ],
            dispatch: compiler::dispatch_for(size),
        });
        ctx.builder_mut().param(
            lumen_gpu::ParamKey {
                owner: lumen_gpu::NodeKey(self.id.0),
                slot: 0,
            },
            lumen_gpu::ParamTarget::Buffer(params),
        );
        ctx.push_frame_binding(FrameBinding::Boolean {
            node_id: self.id,
            operation: self.operation.clone(),
            threshold: self.threshold.clone(),
            buffer: params,
        });

        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            metadata: a.metadata,
        }))
    }
}

impl GpuFrameBindNode for Boolean {
    fn bind_gpu_frame(
        &self,
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        let FrameBinding::Boolean {
            node_id,
            operation,
            threshold,
            buffer,
        } = binding
        else {
            return Ok(());
        };
        let params = compiler::BooleanParams {
            values: [
                BooleanOperation::from_int(operation.resolve_int(
                    *node_id,
                    "operation",
                    &ctx.expr_context(*node_id, "operation"),
                )?) as u32 as f32,
                threshold.resolve_float(
                    *node_id,
                    "threshold",
                    &ctx.expr_context(*node_id, "threshold"),
                )? as f32,
                0.0,
                0.0,
            ],
        };
        bound.write_buffer(*buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}
