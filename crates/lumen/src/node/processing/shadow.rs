use crate::node::{NodeId, NodeProperty, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
    RasterHandle, compiler,
};

pub(crate) const SHADER: &str = include_str!("shadow.wgsl");

#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "shadow",
    label = "Shadow",
    description = "Composites a blurred alpha shadow behind a raster.",
    category = "processing"
)]
pub struct Shadow {
    pub id: NodeId,
    #[property(kind = "float")]
    pub offset_x: NodeProperty,
    #[property(kind = "float")]
    pub offset_y: NodeProperty,
    #[property(kind = "float")]
    pub radius: NodeProperty,
    #[property(kind = "color")]
    pub color: NodeProperty,
    #[property(kind = "float")]
    pub opacity: NodeProperty,
    #[input()]
    pub source: PortRef,
}

impl Default for Shadow {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            offset_x: NodeProperty::Float(8.0),
            offset_y: NodeProperty::Float(8.0),
            radius: NodeProperty::Float(8.0),
            color: NodeProperty::Color([0, 0, 0, 255]),
            opacity: NodeProperty::Float(0.5),
            source: PortRef::empty(),
        }
    }
}

impl GpuCompileNode for Shadow {
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
        let size = source.domain.storage_size;
        let temp = ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("shadow:{}:horizontal", self.id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let texture = ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("shadow:{}:output", self.id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let params = ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("shadow:{}:params", self.id.0)),
            lumen_gpu::BufferDesc::uniform(std::mem::size_of::<compiler::ShadowParams>() as u64),
        );
        let bind_groups = lumen_gpu::BindGroupLayoutSpec::single(vec![
            lumen_gpu::BindingLayoutEntry::texture(0, lumen_gpu::wgpu::ShaderStages::COMPUTE),
            lumen_gpu::BindingLayoutEntry::texture(1, lumen_gpu::wgpu::ShaderStages::COMPUTE),
            lumen_gpu::BindingLayoutEntry::uniform(2, lumen_gpu::wgpu::ShaderStages::COMPUTE),
            lumen_gpu::BindingLayoutEntry::storage_texture(
                3,
                lumen_gpu::wgpu::ShaderStages::COMPUTE,
                lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                lumen_gpu::wgpu::StorageTextureAccess::WriteOnly,
            ),
        ]);
        let horizontal = ctx.builder_mut().program_for(
            lumen_gpu::NodeKey(self.id.0),
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some("shadow-horizontal".to_string()),
                shader: SHADER.to_string(),
                entry: "horizontal_main".to_string(),
                bind_groups: bind_groups.clone(),
            }),
        );
        let vertical = ctx.builder_mut().program_for(
            lumen_gpu::NodeKey(self.id.0),
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some("shadow-vertical".to_string()),
                shader: SHADER.to_string(),
                entry: "vertical_main".to_string(),
                bind_groups,
            }),
        );
        ctx.builder_mut().compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("shadow:{}:horizontal", self.id.0)),
            owner: Some(lumen_gpu::NodeKey(self.id.0)),
            program: horizontal,
            bindings: vec![
                lumen_gpu::Binding::sampled_texture(0, 0, source.texture),
                lumen_gpu::Binding::sampled_texture(0, 1, source.texture),
                lumen_gpu::Binding::uniform(0, 2, params),
                lumen_gpu::Binding::storage_texture(0, 3, temp),
            ],
            dispatch: compiler::dispatch_for(size),
        });
        ctx.builder_mut().compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("shadow:{}:vertical", self.id.0)),
            owner: Some(lumen_gpu::NodeKey(self.id.0)),
            program: vertical,
            bindings: vec![
                lumen_gpu::Binding::sampled_texture(0, 0, source.texture),
                lumen_gpu::Binding::sampled_texture(0, 1, temp),
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
        ctx.push_frame_binding(FrameBinding::Shadow {
            node_id: self.id,
            offset_x: self.offset_x.clone(),
            offset_y: self.offset_y.clone(),
            radius: self.radius.clone(),
            color: self.color.clone(),
            opacity: self.opacity.clone(),
            buffer: params,
        });
        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: source.metadata,
        }))
    }
}

impl GpuFrameBindNode for Shadow {
    fn bind_gpu_frame(
        &self,
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        let FrameBinding::Shadow {
            node_id,
            offset_x,
            offset_y,
            radius,
            color,
            opacity,
            buffer,
        } = binding
        else {
            return Ok(());
        };
        let color = color.resolve_color(*node_id, "color", &ctx.expr_context(*node_id, "color"))?;
        let color = compiler::ColorParams::from_rgba8(color).color;
        let params = compiler::ShadowParams {
            color,
            values: [
                offset_x.resolve_float(
                    *node_id,
                    "offset_x",
                    &ctx.expr_context(*node_id, "offset_x"),
                )? as f32,
                offset_y.resolve_float(
                    *node_id,
                    "offset_y",
                    &ctx.expr_context(*node_id, "offset_y"),
                )? as f32,
                radius
                    .resolve_float(*node_id, "radius", &ctx.expr_context(*node_id, "radius"))?
                    .round()
                    .clamp(0.0, 32.0) as f32,
                opacity.resolve_float(
                    *node_id,
                    "opacity",
                    &ctx.expr_context(*node_id, "opacity"),
                )? as f32,
            ],
        };
        bound.write_buffer(*buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}
