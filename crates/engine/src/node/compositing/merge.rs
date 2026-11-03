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
        let texture = ctx.builder_mut().texture(
            Some(format!("merge:{}:output", self.id.0)),
            lumen_gpu::TextureDesc::storage(
                domain.storage_size,
                lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            )
            .with_domain(domain),
        );
        let params = ctx.builder_mut().buffer(
            Some(format!("merge:{}:params", self.id.0)),
            lumen_gpu::BufferDesc::uniform(std::mem::size_of::<compiler::MergeParams>() as u64),
        );
        let program = ctx.builder_mut().program(lumen_gpu::ProgramDesc::Compute(
            lumen_gpu::ComputeProgramDesc {
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
            },
        ));
        ctx.builder_mut().compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("merge:{}:blend", self.id.0)),
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
            dispatch: compiler::dispatch_for(domain.storage_size),
        });
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
            bounds: None,
        }))
    }
}

const RECORD_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(1, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(3, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(5, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(
        4,
        lumen_gpu::LayoutKind::StorageTexture(lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
    ),
];

impl crate::gpu::frame_graph::RecordNode for Merge {
    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let base = cx.input(&self.base)?;
        let overlay = cx.input(&self.overlay)?;
        let evaluated = self.params.eval(&NodeParamEvalContext {
            node_id: self.id,
            expr: &cx.expr(self.id, "params"),
        })?;
        // The blur reads the mask at full frame.
        let feathered = evaluated.feather > 0.0;
        let mask = if self.mask.is_empty() {
            None
        } else if feathered {
            Some(cx.input_full(&self.mask)?)
        } else {
            Some(cx.input(&self.mask)?)
        };
        let size = base.domain.display_size();
        // Compositing transparent over transparent stays transparent, so only the texels
        // of the base or the overlay are written.
        let domain = cx.domain_within(
            size,
            Some(base.domain.data_rect.union(overlay.domain.data_rect)),
        );
        // The blur spans the mask and the base, so a mask that extends past the base still
        // feathers the base's edge and a smaller one still feathers into the base.
        let feather = match mask.filter(|_| feathered) {
            Some(mask) => {
                let mask_size = mask.domain.display_size();
                let union = lumen_gpu::Size::new(
                    size.width.max(mask_size.width),
                    size.height.max(mask_size.height),
                );
                Some(gaussian::record_passes(
                    cx,
                    mask,
                    lumen_gpu::TextureDomain::full_frame(union),
                    false,
                    &gaussian::GaussianSettings {
                        radius: evaluated.feather.max(0.0),
                        straight_source: mask.straight(),
                        transparent_edge: true,
                        offset: [0.0, 0.0],
                    },
                )?)
            }
            None => None,
        };
        let params = compiler::MergeParams {
            opacity: evaluated.opacity as f32,
            blend_mode: evaluated.blend_mode as u32,
            mask_source: match feather {
                _ if mask.is_none() => MASK_NONE,
                Some(_) => MASK_FEATHERED,
                None => MASK_RAW,
            },
            straight_inputs: u32::from(base.straight())
                | (u32::from(overlay.straight()) << 1)
                | (u32::from(mask.is_some_and(|mask| mask.straight())) << 2),
            mask_mode: evaluated.mask_mode as u32,
            invert: u32::from(evaluated.invert),
            origins: [
                [domain.data_rect.x, domain.data_rect.y],
                base.origin(),
                overlay.origin(),
                mask.map_or(base.origin(), |mask| mask.origin()),
            ],
            _pad: [0; 2],
        };
        let tex = cx.storage_target(domain.storage_size);
        let program = cx.program("merge", SHADER, "cs_main", RECORD_LAYOUT)?;
        let uniform = cx.rec.uniform(&params);
        cx.dispatch(
            program,
            &[
                (0, lumen_gpu::Res::Tex(base.tex)),
                (1, lumen_gpu::Res::Tex(overlay.tex)),
                (2, lumen_gpu::Res::Tex(feather.unwrap_or(base.tex))),
                (
                    5,
                    lumen_gpu::Res::Tex(mask.map_or(base.tex, |mask| mask.tex)),
                ),
                (3, uniform),
                (4, lumen_gpu::Res::Tex(tex)),
            ],
            compiler::dispatch_for(domain.storage_size),
        );
        Ok(crate::gpu::frame_graph::Raster {
            tex,
            domain,
            metadata: RasterMetadata {
                alpha_mode: AlphaMode::Premultiplied,
                ..base.metadata
            },
            bounds: None,
        })
    }
}
