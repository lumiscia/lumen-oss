use std::collections::HashMap;

use crate::{
    composition::Composition,
    error::RenderError,
    expr::ExpressionContext,
    gpu::{
        BoundFrame, CompiledComposition, CompiledOutput, FrameBindContext, GpuCompiledNode,
        RasterHandle, RasterMetadata,
    },
    media::MediaStore,
    node::{Deferred, NodeId, NodeKind, PortRef},
};

pub(crate) use super::params::*;

#[derive(Debug, Clone)]
struct BackgroundClearBinding {
    node_id: NodeId,
    color: [u8; 4],
    buffer: lumen_gpu::BufferId,
}

impl GpuCompiledNode for BackgroundClearBinding {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, _ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        bound.write_buffer(
            self.buffer,
            0,
            bytemuck::bytes_of(&ColorParams::from_rgba8(self.color)),
        );
        Ok(())
    }
}

pub trait GpuCompileNode {
    fn compile_gpu(
        &self,
        ctx: &mut CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput>;
}

#[derive(Debug)]
struct CompiledPortKey {
    port: PortRef,
    frame: u32,
}

impl PartialEq for CompiledPortKey {
    fn eq(&self, other: &Self) -> bool {
        self.frame == other.frame && self.port == other.port
    }
}

impl Eq for CompiledPortKey {}

impl std::hash::Hash for CompiledPortKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.port.hash(state);
        self.frame.hash(state);
    }
}

#[derive(Debug)]
pub struct CompileContext<'a> {
    composition: &'a Composition,
    frame: u32,
    media: Option<&'a dyn MediaStore>,
    builder: lumen_gpu::RenderPlanBuilder,
    outputs: HashMap<CompiledPortKey, CompiledOutput>,
    public_outputs: HashMap<PortRef, CompiledOutput>,
    compiled_nodes: HashMap<NodeId, Box<dyn GpuCompiledNode>>,
    output_format: lumen_gpu::wgpu::TextureFormat,
}

impl<'a> CompileContext<'a> {
    pub fn new(composition: &'a Composition) -> Self {
        Self::with_output_format(composition, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm)
    }

    pub fn with_output_format(
        composition: &'a Composition,
        output_format: lumen_gpu::wgpu::TextureFormat,
    ) -> Self {
        Self::with_options(composition, 0, None, output_format)
    }

    pub fn with_frame(
        composition: &'a Composition,
        frame: u32,
        output_format: lumen_gpu::wgpu::TextureFormat,
    ) -> Self {
        Self::with_options(composition, frame, None, output_format)
    }

    pub fn with_media<M: MediaStore>(
        composition: &'a Composition,
        media: &'a M,
        output_format: lumen_gpu::wgpu::TextureFormat,
    ) -> Self {
        Self::with_media_for_frame(composition, 0, media, output_format)
    }

    pub fn with_media_for_frame<M: MediaStore>(
        composition: &'a Composition,
        frame: u32,
        media: &'a M,
        output_format: lumen_gpu::wgpu::TextureFormat,
    ) -> Self {
        Self::with_options(composition, frame, Some(media), output_format)
    }

    fn with_options(
        composition: &'a Composition,
        frame: u32,
        media: Option<&'a dyn MediaStore>,
        output_format: lumen_gpu::wgpu::TextureFormat,
    ) -> Self {
        Self {
            composition,
            frame,
            media,
            builder: lumen_gpu::RenderPlan::builder(),
            outputs: HashMap::new(),
            public_outputs: HashMap::new(),
            compiled_nodes: HashMap::new(),
            output_format,
        }
    }

    pub fn compile(mut self) -> crate::Result<CompiledComposition> {
        if let Err(mut errors) = self.composition.validate_structure() {
            // Structural validation can report several independent problems. The public compiler
            // error remains typed, so return the first one before traversing any graph edges.
            return Err(errors.remove(0));
        }
        let output_node = self.media_output_node()?;
        let output_ref = PortRef::new(output_node, "output".to_string());
        let output = self
            .compile_port(&output_ref)?
            .into_raster(output_node, "output")?;
        Ok(CompiledComposition {
            plan: self.builder.build(),
            output,
            node_outputs: self.public_outputs,
            compiled_nodes: self.compiled_nodes,
        })
    }

    pub(crate) fn composition(&self) -> &Composition {
        self.composition
    }

    pub(crate) fn media(&self) -> Option<&dyn MediaStore> {
        self.media
    }

    pub(crate) fn output_format(&self) -> lumen_gpu::wgpu::TextureFormat {
        self.output_format
    }

    pub(crate) fn builder_mut(&mut self) -> &mut lumen_gpu::RenderPlanBuilder {
        &mut self.builder
    }

    pub(crate) fn register_compiled_node<N>(&mut self, node: N)
    where
        N: GpuCompiledNode + 'static,
    {
        self.compiled_nodes.insert(node.node_id(), Box::new(node));
    }

    pub(crate) fn compile_port(&mut self, port: &PortRef) -> crate::Result<CompiledOutput> {
        let key = CompiledPortKey {
            port: port.clone(),
            frame: self.frame,
        };
        if let Some(output) = self.outputs.get(&key) {
            return Ok(output.clone());
        }

        let node = self
            .composition
            .graph
            .nodes
            .get(&port.id)
            .ok_or(RenderError::MissingNode {
                frame: 0,
                node_id: port.id,
            })?;
        let output = match node {
            NodeKind::MediaIn(node) => node.compile_gpu(self, port)?,
            NodeKind::Background(node) => node.compile_gpu(self, port)?,
            NodeKind::Text(node) => node.compile_gpu(self, port)?,
            NodeKind::Path(node) => node.compile_gpu(self, port)?,
            NodeKind::Shape(node) => node.compile_gpu(self, port)?,
            NodeKind::Boolean(node) => node.compile_gpu(self, port)?,
            NodeKind::Merge(node) => node.compile_gpu(self, port)?,
            NodeKind::RasterMultiMerge(node) => node.compile_gpu(self, port)?,
            NodeKind::AlphaPremultiply(node) => node.compile_gpu(self, port)?,
            NodeKind::Blur(node) => node.compile_gpu(self, port)?,
            NodeKind::ChannelShuffle(node) => node.compile_gpu(self, port)?,
            NodeKind::ColorGrade(node) => node.compile_gpu(self, port)?,
            NodeKind::Curves(node) => node.compile_gpu(self, port)?,
            NodeKind::Exposure(node) => node.compile_gpu(self, port)?,
            NodeKind::HueSaturation(node) => node.compile_gpu(self, port)?,
            NodeKind::Levels(node) => node.compile_gpu(self, port)?,
            NodeKind::Memo(node) => node.compile_gpu(self, port)?,
            NodeKind::Opacity(node) => node.compile_gpu(self, port)?,
            NodeKind::TimeRemap(node) => node.compile_gpu(self, port)?,
            NodeKind::Transform(node) => node.compile_gpu(self, port)?,
            NodeKind::Crop(node) => node.compile_gpu(self, port)?,
            NodeKind::Resize(node) => node.compile_gpu(self, port)?,
            NodeKind::Shadow(node) => node.compile_gpu(self, port)?,
            NodeKind::WgslShader(node) => node.compile_gpu(self, port)?,
            NodeKind::Switch(node) => node.compile_gpu(self, port)?,
            NodeKind::MediaOutput(node) => node.compile_gpu(self, port)?,
        };

        self.outputs.insert(key, output.clone());
        self.public_outputs
            .entry(port.clone())
            .or_insert_with(|| output.clone());
        Ok(output)
    }

    pub(crate) fn with_frame_context<T>(
        &mut self,
        frame: u32,
        f: impl FnOnce(&mut Self) -> crate::Result<T>,
    ) -> crate::Result<T> {
        let original_frame = self.frame;
        self.frame = frame;
        let result = f(self);
        self.frame = original_frame;
        result
    }

    pub(crate) fn compile_unary_filter(
        &mut self,
        node_id: NodeId,
        source_ref: &PortRef,
        port: &PortRef,
        label: &str,
        shader: &str,
        params_size: u64,
    ) -> crate::Result<(RasterHandle, lumen_gpu::TextureId, lumen_gpu::BufferId)> {
        if port.port != "output" {
            return Err(self.missing_output(node_id, &port.port));
        }

        let source = self
            .compile_port(source_ref)?
            .into_raster(source_ref.id, &source_ref.port)?;
        let size = source.domain.storage_size;
        let texture = self.builder.texture_for(
            lumen_gpu::NodeKey(node_id.0),
            Some(format!("{label}:{}:output", node_id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let params = self.builder.buffer_for(
            lumen_gpu::NodeKey(node_id.0),
            Some(format!("{label}:{}:params", node_id.0)),
            lumen_gpu::BufferDesc::uniform(params_size),
        );
        let program = self.builder.program_for(
            lumen_gpu::NodeKey(node_id.0),
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some(label.to_string()),
                shader: shader.to_string(),
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
                ]),
            }),
        );
        self.builder.compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("{label}:{}:apply", node_id.0)),
            owner: Some(lumen_gpu::NodeKey(node_id.0)),
            program,
            bindings: vec![
                lumen_gpu::Binding::sampled_texture(0, 0, source.texture),
                lumen_gpu::Binding::uniform(0, 1, params),
                lumen_gpu::Binding::storage_texture(0, 2, texture),
            ],
            dispatch: dispatch_for(size).into(),
        });
        self.builder.param(
            lumen_gpu::ParamKey {
                owner: lumen_gpu::NodeKey(node_id.0),
                slot: 0,
            },
            lumen_gpu::ParamTarget::Buffer(params),
        );
        Ok((source, texture, params))
    }

    pub(crate) fn compile_transparent(&mut self, node_id: NodeId) -> CompiledOutput {
        let size = lumen_gpu::Size::new(
            self.composition.render_settings.width.max(1),
            self.composition.render_settings.height.max(1),
        );
        let texture = self.builder.texture_for(
            lumen_gpu::NodeKey(node_id.0),
            Some(format!("transparent:{}:output", node_id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let params = self.builder.buffer_for(
            lumen_gpu::NodeKey(node_id.0),
            Some(format!("transparent:{}:params", node_id.0)),
            lumen_gpu::BufferDesc::uniform(std::mem::size_of::<ColorParams>() as u64),
        );
        let program = self.builder.program_for(
            lumen_gpu::NodeKey(node_id.0),
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some("transparent".to_string()),
                shader: crate::node::source::background::SHADER.to_string(),
                entry: "cs_main".to_string(),
                bind_groups: lumen_gpu::BindGroupLayoutSpec::single(vec![
                    lumen_gpu::BindingLayoutEntry::uniform(
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
        self.builder.compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("transparent:{}:fill", node_id.0)),
            owner: Some(lumen_gpu::NodeKey(node_id.0)),
            program,
            bindings: vec![
                lumen_gpu::Binding::uniform(0, 0, params),
                lumen_gpu::Binding::storage_texture(0, 1, texture),
            ],
            dispatch: dispatch_for(size).into(),
        });
        self.builder.param(
            lumen_gpu::ParamKey {
                owner: lumen_gpu::NodeKey(node_id.0),
                slot: 0,
            },
            lumen_gpu::ParamTarget::Buffer(params),
        );
        self.register_compiled_node(BackgroundClearBinding {
            node_id,
            color: [0, 0, 0, 0],
            buffer: params,
        });

        CompiledOutput::Raster(RasterHandle {
            texture,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            metadata: RasterMetadata::default(),
        })
    }

    pub(crate) fn static_dimension(
        &self,
        property: &Deferred<i64>,
        node_id: NodeId,
        property_path: &str,
    ) -> crate::Result<u32> {
        let value = property.resolve_int(
            node_id,
            property_path,
            &self.expr_context(node_id, property_path),
        )?;
        let value = if value <= 0 {
            match property_path {
                "width" => i64::from(self.composition.render_settings.width),
                "height" => i64::from(self.composition.render_settings.height),
                _ => value,
            }
        } else {
            value
        };
        Ok(value.clamp(1, i64::from(u32::MAX)) as u32)
    }

    pub(crate) fn static_dimension_value(&self, value: i64, property_path: &str) -> u32 {
        let value = if value <= 0 {
            match property_path {
                "width" => i64::from(self.composition.render_settings.width),
                "height" => i64::from(self.composition.render_settings.height),
                _ => value,
            }
        } else {
            value
        };
        value.clamp(1, i64::from(u32::MAX)) as u32
    }

    pub(crate) fn spatial_program(
        &mut self,
        node_id: NodeId,
        label: &str,
        shader: &str,
    ) -> lumen_gpu::ProgramId {
        self.builder.program_for(
            lumen_gpu::NodeKey(node_id.0),
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some(label.to_string()),
                shader: shader.to_string(),
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
                ]),
            }),
        )
    }

    #[allow(dead_code)]
    fn compile_unary_compute(
        &mut self,
        node_id: NodeId,
        port: &PortRef,
        source_port: &PortRef,
        label: &str,
        shader: &str,
        param_size: u64,
    ) -> crate::Result<(RasterHandle, lumen_gpu::TextureId, lumen_gpu::BufferId)> {
        if port.port != "output" {
            return Err(self.missing_output(node_id, &port.port));
        }

        let source = self
            .compile_port(source_port)?
            .into_raster(source_port.id, &source_port.port)?;
        let size = source.domain.storage_size;
        let texture = self.builder.texture_for(
            lumen_gpu::NodeKey(node_id.0),
            Some(format!("{label}:{}:output", node_id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let params = self.builder.buffer_for(
            lumen_gpu::NodeKey(node_id.0),
            Some(format!("{label}:{}:params", node_id.0)),
            lumen_gpu::BufferDesc::uniform(param_size),
        );
        let program = self.spatial_program(node_id, label, shader);
        self.builder.compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("{label}:{}:apply", node_id.0)),
            owner: Some(lumen_gpu::NodeKey(node_id.0)),
            program,
            bindings: spatial_bindings(source.texture, params, texture),
            dispatch: dispatch_for(size).into(),
        });
        self.builder.param(
            lumen_gpu::ParamKey {
                owner: lumen_gpu::NodeKey(node_id.0),
                slot: 0,
            },
            lumen_gpu::ParamTarget::Buffer(params),
        );
        Ok((source, texture, params))
    }

    fn media_output_node(&self) -> crate::Result<NodeId> {
        let mut outputs = self
            .composition
            .graph
            .nodes
            .iter()
            .filter_map(|(node_id, node)| {
                matches!(node, NodeKind::MediaOutput(_)).then_some(*node_id)
            });
        let Some(output) = outputs.next() else {
            return Err(crate::error::GraphValidationError::MissingMediaOutput.into());
        };
        if outputs.next().is_some() {
            return Err(
                crate::error::GraphValidationError::MultipleMediaOutputs { count: 2 }.into(),
            );
        }
        Ok(output)
    }

    pub(crate) fn expr_context(
        &self,
        node_id: NodeId,
        property_path: &str,
    ) -> ExpressionContext<'_> {
        ExpressionContext {
            frame: self.frame,
            fps: self.composition.timeline.fps,
            width: self.composition.render_settings.width,
            height: self.composition.render_settings.height,
            duration_frames: self.composition.timeline.duration_frames,
            path: Some(format!("{node_id}.{property_path}")),
            graph: Some(&self.composition.graph),
        }
    }

    pub(crate) fn missing_output(&self, node_id: NodeId, port: &str) -> crate::error::LumenError {
        crate::error::PropertyError::MissingProperty {
            node_id,
            property_path: format!("output port `{port}`"),
        }
        .into()
    }
}
