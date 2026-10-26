use crate::node::{NodeId, NodeParamEvalContext, NodeParams, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode, RasterHandle,
    compiler,
};

pub(crate) const SHADER: &str = include_str!("resize.wgsl");

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum")]
pub enum ResizeMode {
    #[default]
    Stretch = 0,
    Fit = 1,
    Fill = 2,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum")]
pub enum ResizeSampling {
    Nearest = 0,
    #[default]
    Linear = 1,
}

/// Resamples a raster into a rectangle placed on the composition canvas.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct ResizeParams {
    /// Left edge of the output rectangle in pixels.
    #[meta(step = 1)]
    pub x: i64,
    /// Top edge of the output rectangle in pixels.
    #[meta(step = 1)]
    pub y: i64,
    /// Output width in pixels.
    #[meta(min = 1, step = 1)]
    pub width: i64,
    /// Output height in pixels.
    #[meta(min = 1, step = 1)]
    pub height: i64,
    /// How the source raster should fit the output bounds.
    #[meta(kind = "enum", enum_type = ResizeMode)]
    pub mode: ResizeMode,
    /// Sampling filter used when resizing.
    #[meta(kind = "enum", enum_type = ResizeSampling)]
    pub sampling: ResizeSampling,
}

impl Default for ResizeParams {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            mode: ResizeMode::Stretch,
            sampling: ResizeSampling::Linear,
        }
    }
}

/// Resamples a raster into a rectangle placed on the composition canvas.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "resize", name = "Resize", category = "processing")]
pub struct Resize {
    pub id: NodeId,
    #[params]
    pub params: ResizeParamsDelegate,

    #[input()]
    pub source: PortRef,
}

impl Default for Resize {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: ResizeParamsDelegate::default(),
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
            ctx.composition().render_settings.width.max(1),
            ctx.composition().render_settings.height.max(1),
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
        // Sources that don't publish bounds fill their whole storage texture.
        let (bounds, default_bounds) = match ctx.raster_bounds(source.texture) {
            Some(bounds) => (bounds, None),
            None => {
                let bounds = ctx.builder_mut().buffer_for(
                    lumen_gpu::NodeKey(self.id.0),
                    Some(format!("resize:{}:source_bounds", self.id.0)),
                    lumen_gpu::BufferDesc::uniform(
                        std::mem::size_of::<compiler::CropParams>() as u64
                    ),
                );
                (bounds, Some(bounds))
            }
        };
        let program = ctx.builder_mut().program_for(
            lumen_gpu::NodeKey(self.id.0),
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some("resize".to_string()),
                shader: SHADER.to_string(),
                entry: "cs_main".to_string(),
                bind_groups: lumen_gpu::BindGroupLayoutSpec::single(vec![
                    lumen_gpu::BindingLayoutEntry::texture(
                        0,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                    ),
                    lumen_gpu::BindingLayoutEntry::uniform(
                        1,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                    ),
                    lumen_gpu::BindingLayoutEntry::storage_texture(
                        2,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                        lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                        lumen_gpu::wgpu::StorageTextureAccess::WriteOnly,
                    ),
                    lumen_gpu::BindingLayoutEntry::uniform(
                        3,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                    ),
                ]),
            }),
        );
        let mut bindings = compiler::spatial_bindings(source.texture, params, texture);
        bindings.push(lumen_gpu::Binding::uniform(0, 3, bounds));
        ctx.builder_mut().compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("resize:{}:apply", self.id.0)),
            owner: Some(lumen_gpu::NodeKey(self.id.0)),
            program,
            bindings,
            dispatch: compiler::dispatch_for(size).into(),
        });
        ctx.builder_mut().param(
            lumen_gpu::ParamKey {
                owner: lumen_gpu::NodeKey(self.id.0),
                slot: 0,
            },
            lumen_gpu::ParamTarget::Buffer(params),
        );
        ctx.register_compiled_node(CompiledResize {
            node_id: self.id,
            params: self.params.clone(),
            buffer: params,
            default_bounds: default_bounds.map(|buffer| (buffer, source.domain.storage_size)),
        });

        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            metadata: source.metadata,
        }))
    }
}

#[derive(Debug, Clone)]
struct CompiledResize {
    node_id: NodeId,
    params: ResizeParamsDelegate,
    buffer: lumen_gpu::BufferId,
    /// Full-source bounds written each frame when the source publishes none.
    default_bounds: Option<(lumen_gpu::BufferId, lumen_gpu::Size)>,
}

impl GpuCompiledNode for CompiledResize {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let evaluated = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        let params = compiler::ResizeParams {
            origin: [
                compiler::pixel_origin(self.node_id, "x", evaluated.x)?,
                compiler::pixel_origin(self.node_id, "y", evaluated.y)?,
            ],
            size: [
                compiler::pixel_extent(self.node_id, "width", evaluated.width, 1)?,
                compiler::pixel_extent(self.node_id, "height", evaluated.height, 1)?,
            ],
            mode: evaluated.mode as u32,
            sampling: evaluated.sampling as u32,
            _pad: [0; 2],
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&params));
        if let Some((buffer, size)) = self.default_bounds {
            let bounds = compiler::CropParams {
                origin: [0, 0],
                size: [size.width, size.height],
            };
            bound.write_buffer(buffer, 0, bytemuck::bytes_of(&bounds));
        }
        Ok(())
    }
}
