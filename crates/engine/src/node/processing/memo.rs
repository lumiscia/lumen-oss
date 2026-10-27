use crate::node::{Deferred, NodeId, NodeParams, PortRef};

use crate::gpu::{CompiledOutput, GpuCompileNode, RasterHandle, compiler};

/// Renders its source again only when `key` changes.
#[derive(Debug, Clone, Default, lumen_macros::Delegate)]
pub struct MemoParams {
    /// Expression whose value decides when the source renders again: while it evaluates to the
    /// same value as on the frame last rendered, the memo shows that frame's output and its
    /// source is not rendered. A literal key adds nothing to the caching every node without
    /// frame-dependent params already gets.
    #[meta(name = "Key")]
    pub key: f64,
}

/// Renders its source again only when `key` changes.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "memo", name = "Memo", category = "processing")]
pub struct Memo {
    pub id: NodeId,
    #[params]
    pub params: MemoParamsDelegate,

    #[input()]
    pub source: PortRef,
}

impl Default for Memo {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: MemoParamsDelegate::default(),
            source: PortRef::empty(),
        }
    }
}

impl GpuCompileNode for Memo {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        if port.port != "output" {
            return Err(ctx.missing_output(self.id, &port.port));
        }
        let output = ctx.compile_bounded_port(&self.source)?;
        let CompiledOutput::Raster(source) = output else {
            return Ok(output);
        };
        // The copy keeps the source's texels but not bounds published for its texture, which the
        // source may move while the memo holds, so such sources pass through uncached.
        if !matches!(self.params.key, Deferred::Expr(_))
            || ctx.raster_bounds(source.texture).is_some()
            || ctx
                .builder_mut()
                .texture_desc(source.texture)
                .map(|desc| desc.format)
                != Some(lumen_gpu::wgpu::TextureFormat::Rgba8Unorm)
        {
            return Ok(output);
        }
        let size = source.domain.storage_size;
        let texture = ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("memo:{}:output", self.id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm)
                .with_domain(source.domain),
        );
        let program = ctx.builder_mut().program_for(
            lumen_gpu::NodeKey(self.id.0),
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some("memo".to_string()),
                shader: crate::node::media_output::SHADER.to_string(),
                entry: "cs_main".to_string(),
                bind_groups: lumen_gpu::BindGroupLayoutSpec::single(vec![
                    lumen_gpu::BindingLayoutEntry::texture(
                        0,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                    ),
                    lumen_gpu::BindingLayoutEntry::storage_texture(
                        1,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                        lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                        lumen_gpu::wgpu::StorageTextureAccess::WriteOnly,
                    ),
                ]),
            }),
        );
        // Loads rather than copies texels, since media textures may be replaced by ones of
        // another format.
        let copy = ctx.builder_mut().compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("memo:{}:copy", self.id.0)),
            owner: Some(lumen_gpu::NodeKey(self.id.0)),
            program,
            bindings: vec![
                lumen_gpu::Binding::sampled_texture(0, 0, source.texture),
                lumen_gpu::Binding::storage_texture(0, 1, texture),
            ],
            dispatch: compiler::dispatch_for(size).into(),
        });
        ctx.record_memo(texture, copy);
        Ok(CompiledOutput::Raster(RasterHandle { texture, ..source }))
    }
}
