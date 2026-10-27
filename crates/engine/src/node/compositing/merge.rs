use crate::node::{
    Deferred, NodeId, NodeParamEvalContext, NodeParams, PortRef, compositing::BlendMode,
    processing::gaussian,
};

use crate::gpu::{
    AlphaMode, BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode,
    RasterHandle, RasterMetadata, compiler,
};

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("blend.wgsl"),
    include_str!("merge.wgsl")
);

/// How the `mask` raster is turned into a per-pixel overlay weight.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(u8)]
#[non_exhaustive]
#[delegate(kind = "enum")]
pub enum MaskMode {
    /// Uses the mask's alpha channel.
    #[default]
    Alpha = 0,
    /// Uses the mask's Rec. 709 luma over black, so transparent areas are fully masked out.
    Luma = 1,
}

/// Composites an overlay raster over a base raster.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct MergeParams {
    /// Overlay opacity applied before compositing.
    #[meta(min = 0, max = 1, step = 0.05)]
    pub opacity: f64,
    /// Blend mode used when combining the overlay with the base raster.
    #[meta()]
    pub blend_mode: BlendMode,
    /// Channel of the `mask` input that controls overlay visibility.
    #[meta()]
    pub mask_mode: MaskMode,
    /// Swaps the masked and unmasked areas.
    #[meta()]
    pub invert: bool,
    /// Softens the mask edge with a Gaussian blur of this radius in pixels, applied before `invert`.
    #[meta(min = 0, step = 0.5)]
    pub feather: f64,
}

impl Default for MergeParams {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
            mask_mode: MaskMode::Alpha,
            invert: false,
            feather: 0.0,
        }
    }
}

/// Composites an overlay raster over a base raster.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "merge", name = "Merge", category = "compositing")]
pub struct Merge {
    pub id: NodeId,
    #[params]
    pub params: MergeParamsDelegate,

    #[input()]
    pub base: PortRef,
    #[input()]
    pub overlay: PortRef,
    #[input(optional)]
    pub mask: PortRef,
}

impl Default for Merge {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: MergeParamsDelegate::default(),
            base: PortRef::empty(),
            overlay: PortRef::empty(),
            mask: PortRef::empty(),
        }
    }
}

/// `MergeParams::mask_source`: no mask, the feathered mask, or the mask as is. A feather
/// that evaluates to zero reads the mask as is, the same as a literal zero.
const MASK_NONE: u32 = 0;
const MASK_FEATHERED: u32 = 1;
const MASK_RAW: u32 = 2;

#[derive(Debug, Clone)]
struct CompiledMerge {
    node_id: NodeId,
    params: MergeParamsDelegate,
    has_mask: bool,
    /// Bit 0: base, bit 1: overlay, bit 2: mask.
    straight_inputs: u32,
    /// Feathered mask passes, present when the node has a `mask` input and a feather that
    /// is not a literal zero.
    feather: Option<CompiledFeather>,
    buffer: lumen_gpu::BufferId,
    /// Canvas pixels of the first texels of the output, base, overlay and raw mask.
    origins: [[i32; 2]; 4],
}

#[derive(Debug, Clone)]
struct CompiledFeather {
    passes: gaussian::GaussianPasses,
    straight_mask: bool,
}

impl GpuCompiledNode for CompiledMerge {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let evaluated = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        let params = compiler::MergeParams {
            opacity: evaluated.opacity as f32,
            blend_mode: evaluated.blend_mode as u32,
            mask_source: match &self.feather {
                _ if !self.has_mask => MASK_NONE,
                Some(_) if evaluated.feather > 0.0 => MASK_FEATHERED,
                _ => MASK_RAW,
            },
            straight_inputs: self.straight_inputs,
            mask_mode: evaluated.mask_mode as u32,
            invert: u32::from(evaluated.invert),
            origins: self.origins,
            _pad: [0; 2],
        };
        if let Some(feather) = &self.feather {
            feather.passes.write(
                bound,
                &gaussian::GaussianSettings {
                    radius: evaluated.feather.max(0.0),
                    straight_source: feather.straight_mask,
                    transparent_edge: true,
                    offset: [0.0, 0.0],
                },
            );
        }
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}

impl GpuCompileNode for Merge {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        if port.port != "output" {
            return Err(ctx.missing_output(self.id, &port.port));
        }

        let base = ctx
            .compile_bounded_port(&self.base)?
            .into_raster(self.base.id, &self.base.port)?;
        let overlay = ctx
            .compile_bounded_port(&self.overlay)?
            .into_raster(self.overlay.id, &self.overlay.port)?;
        // A literal feather of zero or less never blurs, so its passes are left out. The
        // blur reads the mask at full frame.
        let feathered =
            !matches!(&self.params.feather, Deferred::Value(radius) if !(*radius > 0.0));
        let mask = if self.mask.is_empty() {
            None
        } else {
            let mask = ctx
                .compile_bounded_port(&self.mask)?
                .into_raster(self.mask.id, &self.mask.port)?;
            Some(if feathered {
                ctx.full_frame(mask)
            } else {
                mask
            })
        };

        let size = base.domain.display_size();
        // Compositing transparent over transparent stays transparent, so only the texels
        // of the base or the overlay are written.
        let domain = lumen_gpu::TextureDomain::bounded(
            size,
            base.domain.data_rect.union(overlay.domain.data_rect),
        );
        let straight =
            |raster: &RasterHandle| raster.metadata.alpha_mode == AlphaMode::Unpremultiplied;
        // The blur spans the mask and the base, so a mask that extends past the base still
        // feathers the base's edge and a smaller one still feathers into the base.
        let feather = mask.filter(|_| feathered).map(|mask| {
            let mask_size = mask.domain.display_size();
            let union = lumen_gpu::Size::new(
                size.width.max(mask_size.width),
                size.height.max(mask_size.height),
            );
            CompiledFeather {
                passes: gaussian::compile_passes(ctx, self.id, "merge-feather", mask, union),
                straight_mask: straight(&mask),
            }
        });
        let texture = ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("merge:{}:output", self.id.0)),
            lumen_gpu::TextureDesc::storage(
                domain.storage_size,
                lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            )
            .with_domain(domain),
        );
        let params = ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("merge:{}:params", self.id.0)),
            lumen_gpu::BufferDesc::uniform(std::mem::size_of::<compiler::MergeParams>() as u64),
        );
        let program = ctx.builder_mut().program_for(
            lumen_gpu::NodeKey(self.id.0),
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some("merge".to_string()),
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
                    lumen_gpu::BindingLayoutEntry::texture(
                        2,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                    ),
                    lumen_gpu::BindingLayoutEntry::uniform(
                        3,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                    ),
                    lumen_gpu::BindingLayoutEntry::texture(
                        5,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                    ),
                    lumen_gpu::BindingLayoutEntry::storage_texture(
                        4,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                        lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                        lumen_gpu::wgpu::StorageTextureAccess::WriteOnly,
                    ),
                ]),
            }),
        );
        ctx.builder_mut().compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("merge:{}:blend", self.id.0)),
            owner: Some(lumen_gpu::NodeKey(self.id.0)),
            program,
            bindings: vec![
                lumen_gpu::Binding::sampled_texture(0, 0, base.texture),
                lumen_gpu::Binding::sampled_texture(0, 1, overlay.texture),
                lumen_gpu::Binding::sampled_texture(
                    0,
                    2,
                    feather
                        .as_ref()
                        .map_or(base.texture, |feather| feather.passes.output),
                ),
                lumen_gpu::Binding::sampled_texture(
                    0,
                    5,
                    mask.as_ref().map_or(base.texture, |mask| mask.texture),
                ),
                lumen_gpu::Binding::uniform(0, 3, params),
                lumen_gpu::Binding::storage_texture(0, 4, texture),
            ],
            dispatch: compiler::dispatch_for(domain.storage_size).into(),
        });
        ctx.builder_mut().param(
            lumen_gpu::ParamKey {
                owner: lumen_gpu::NodeKey(self.id.0),
                slot: 1,
            },
            lumen_gpu::ParamTarget::Buffer(params),
        );
        ctx.register_compiled_node(CompiledMerge {
            node_id: self.id,
            params: self.params.clone(),
            has_mask: !self.mask.is_empty(),
            straight_inputs: u32::from(straight(&base))
                | (u32::from(straight(&overlay)) << 1)
                | (u32::from(mask.as_ref().is_some_and(straight)) << 2),
            feather,
            buffer: params,
            origins: [
                &domain,
                &base.domain,
                &overlay.domain,
                &mask.map_or(base.domain, |mask| mask.domain),
            ]
            .map(|domain| [domain.data_rect.x, domain.data_rect.y]),
        });

        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain,
            metadata: RasterMetadata {
                alpha_mode: AlphaMode::Premultiplied,
                ..base.metadata
            },
        }))
    }
}
