use crate::node::{NodeId, NodeParamEvalContext, NodeParams, PortRef, vector::paint::Paint};

use crate::gpu::{
    AlphaMode, BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode,
    RasterHandle, RasterMetadata, compiler,
};

use super::gaussian;

const COMPOSITE_SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("shadow.wgsl")
);

/// Composites a blurred alpha shadow behind a raster.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct ShadowParams {
    /// Horizontal shadow offset in pixels.
    #[meta(name = "Offset X", step = 1)]
    pub offset_x: f64,
    /// Vertical shadow offset in pixels.
    #[meta(name = "Offset Y", step = 1)]
    pub offset_y: f64,
    /// Shadow blur radius in pixels. Fractional radii blend smoothly.
    #[meta(name = "Blur radius", min = 0, step = 0.5)]
    pub radius: f64,
    /// Shadow color.
    #[meta(role = "color")]
    pub color: Paint,
    /// Shadow opacity.
    #[meta(min = 0, max = 1, step = 0.05)]
    pub opacity: f64,
}

impl Default for ShadowParams {
    fn default() -> Self {
        Self {
            offset_x: 8.0,
            offset_y: 8.0,
            radius: 8.0,
            color: Paint::solid([0, 0, 0, 255]),
            opacity: 0.5,
        }
    }
}

/// Composites a blurred alpha shadow behind a raster.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "shadow", name = "Shadow", category = "processing")]
pub struct Shadow {
    pub id: NodeId,
    #[params]
    pub params: ShadowParamsDelegate,

    #[input()]
    pub source: PortRef,
}

impl Default for Shadow {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: ShadowParamsDelegate::default(),
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
        let blurred = gaussian::compile_passes(ctx, self.id, "shadow", source.texture, size);
        let owner = lumen_gpu::NodeKey(self.id.0);
        let builder = ctx.builder_mut();
        let output = builder.texture_for(
            owner,
            Some(format!("shadow:{}:output", self.id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let params = builder.buffer_for(
            owner,
            Some(format!("shadow:{}:params", self.id.0)),
            lumen_gpu::BufferDesc::uniform(std::mem::size_of::<compiler::ShadowParams>() as u64),
        );
        let stage = lumen_gpu::wgpu::ShaderStages::COMPUTE;
        let program = builder.program_for(
            owner,
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some("shadow-composite".to_string()),
                shader: COMPOSITE_SHADER.to_string(),
                entry: "composite_main".to_string(),
                bind_groups: lumen_gpu::BindGroupLayoutSpec::single(vec![
                    lumen_gpu::BindingLayoutEntry::texture(0, stage),
                    lumen_gpu::BindingLayoutEntry::texture(1, stage),
                    lumen_gpu::BindingLayoutEntry::uniform(2, stage),
                    lumen_gpu::BindingLayoutEntry::storage_texture(
                        3,
                        stage,
                        lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                        lumen_gpu::wgpu::StorageTextureAccess::WriteOnly,
                    ),
                ]),
            }),
        );
        builder.compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("shadow:{}:composite", self.id.0)),
            owner: Some(owner),
            program,
            bindings: vec![
                lumen_gpu::Binding::sampled_texture(0, 0, blurred.output),
                lumen_gpu::Binding::sampled_texture(0, 1, source.texture),
                lumen_gpu::Binding::uniform(0, 2, params),
                lumen_gpu::Binding::storage_texture(0, 3, output),
            ],
            dispatch: compiler::dispatch_for(size).into(),
        });
        builder.param(
            lumen_gpu::ParamKey { owner, slot: 1 },
            lumen_gpu::ParamTarget::Buffer(params),
        );
        ctx.register_compiled_node(CompiledShadow {
            node_id: self.id,
            params: self.params.clone(),
            blurred,
            buffer: params,
            straight_source: source.metadata.alpha_mode == AlphaMode::Unpremultiplied,
        });
        Ok(CompiledOutput::Raster(RasterHandle {
            texture: output,
            domain: source.domain,
            metadata: RasterMetadata {
                alpha_mode: AlphaMode::Premultiplied,
                ..source.metadata
            },
        }))
    }
}

#[derive(Debug, Clone)]
struct CompiledShadow {
    node_id: NodeId,
    params: ShadowParamsDelegate,
    blurred: gaussian::GaussianPasses,
    buffer: lumen_gpu::BufferId,
    straight_source: bool,
}

impl GpuCompiledNode for CompiledShadow {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let evaluated = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        let color = evaluated.color.to_gpu([0, 0, 0, 255]).colors[0];
        // The color alpha is folded into the opacity so `color.w` can carry the
        // source's straight-alpha flag.
        self.blurred.write(
            bound,
            &gaussian::GaussianSettings {
                radius: evaluated.radius,
                straight_source: self.straight_source,
                transparent_edge: true,
                offset: [evaluated.offset_x, evaluated.offset_y],
            },
        );
        let opacity = evaluated.opacity.clamp(0.0, 1.0) as f32 * color[3];
        let gpu_params = compiler::ShadowParams {
            color: [
                color[0],
                color[1],
                color[2],
                if self.straight_source { 1.0 } else { 0.0 },
            ],
            values: [0.0, 0.0, 0.0, opacity],
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&gpu_params));
        Ok(())
    }
}
