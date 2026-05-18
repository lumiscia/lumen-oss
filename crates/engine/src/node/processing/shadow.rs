use crate::node::{NodeId, NodeProperty, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuFrameBinding, RasterHandle,
    compiler,
};

pub(crate) const SHADER: &str = include_str!("shadow.wgsl");

/// Composites a blurred alpha shadow behind a raster.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "shadow", name = "Shadow", category = "processing")]
pub struct Shadow {
    pub id: NodeId,
    /// Horizontal shadow offset in pixels.
    #[property(kind = "float", name = "Offset X", step = 1)]
    pub offset_x: NodeProperty,
    /// Vertical shadow offset in pixels.
    #[property(kind = "float", name = "Offset Y", step = 1)]
    pub offset_y: NodeProperty,
    /// Shadow blur radius in pixels.
    #[property(kind = "float", name = "Blur radius", min = 0, step = 0.5)]
    pub radius: NodeProperty,
    /// Shadow color.
    #[property(kind = "color")]
    pub color: NodeProperty,
    /// Shadow opacity.
    #[property(kind = "float", min = 0, max = 1, step = 0.05)]
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
            dispatch: compiler::dispatch_for(size).into(),
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
            dispatch: compiler::dispatch_for(size).into(),
        });
        ctx.builder_mut().param(
            lumen_gpu::ParamKey {
                owner: lumen_gpu::NodeKey(self.id.0),
                slot: 0,
            },
            lumen_gpu::ParamTarget::Buffer(params),
        );
        ctx.push_frame_binding(ShadowFrameBinding {
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

#[derive(Debug, Clone)]
struct ShadowFrameBinding {
    node_id: NodeId,
    offset_x: NodeProperty,
    offset_y: NodeProperty,
    radius: NodeProperty,
    color: NodeProperty,
    opacity: NodeProperty,
    buffer: lumen_gpu::BufferId,
}

impl GpuFrameBinding for ShadowFrameBinding {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let color = self.color.resolve_color(
            self.node_id,
            "color",
            &ctx.expr_context(self.node_id, "color"),
        )?;
        let color = compiler::ColorParams::from_rgba8(color).color;
        let params = compiler::ShadowParams {
            color,
            values: [
                self.offset_x.resolve_float(
                    self.node_id,
                    "offset_x",
                    &ctx.expr_context(self.node_id, "offset_x"),
                )? as f32,
                self.offset_y.resolve_float(
                    self.node_id,
                    "offset_y",
                    &ctx.expr_context(self.node_id, "offset_y"),
                )? as f32,
                self.radius
                    .resolve_float(
                        self.node_id,
                        "radius",
                        &ctx.expr_context(self.node_id, "radius"),
                    )?
                    .round()
                    .clamp(0.0, 32.0) as f32,
                self.opacity.resolve_float(
                    self.node_id,
                    "opacity",
                    &ctx.expr_context(self.node_id, "opacity"),
                )? as f32,
            ],
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}
