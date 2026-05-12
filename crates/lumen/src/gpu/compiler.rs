use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};

use crate::{
    composition::Composition,
    error::RenderError,
    expr::ExpressionContext,
    gpu::{
        BoundFrame, CompiledComposition, CompiledOutput, FrameBinding, RasterHandle, RasterMetadata,
    },
    media::MediaStore,
    node::{NodeId, NodeKind, NodeProperty, PortRef},
};

pub trait GpuCompileNode {
    fn compile_gpu(
        &self,
        ctx: &mut CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput>;
}

pub trait GpuFrameBindNode {
    fn bind_gpu_frame(
        &self,
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        bound: &mut BoundFrame,
    ) -> crate::Result<()>;
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
    frame_bindings: Vec<FrameBinding>,
    frame_binding_frames: Vec<Option<u32>>,
    frame_binding_frame_override: Option<u32>,
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
            frame_bindings: Vec::new(),
            frame_binding_frames: Vec::new(),
            frame_binding_frame_override: None,
            output_format,
        }
    }

    pub fn compile(mut self) -> crate::Result<CompiledComposition> {
        let output_node = self.media_output_node()?;
        let output_ref = PortRef::new(output_node, "output".to_string());
        let output = self
            .compile_port(&output_ref)?
            .into_raster(output_node, "output")?;
        Ok(CompiledComposition {
            plan: self.builder.build(),
            output,
            node_outputs: self.public_outputs,
            frame_bindings: self.frame_bindings,
            frame_binding_frames: self.frame_binding_frames,
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

    pub(crate) fn push_frame_binding(&mut self, binding: FrameBinding) {
        self.frame_bindings.push(binding);
        self.frame_binding_frames
            .push(self.frame_binding_frame_override);
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
            NodeKind::SolidColor(node) => node.compile_gpu(self, port)?,
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
        let original_frame_override = self.frame_binding_frame_override;
        self.frame = frame;
        self.frame_binding_frame_override = Some(frame);
        let result = f(self);
        self.frame = original_frame;
        self.frame_binding_frame_override = original_frame_override;
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
                shader: crate::node::source::solid_color::SHADER.to_string(),
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
        self.push_frame_binding(FrameBinding::SolidColor {
            node_id,
            color: NodeProperty::Color([0, 0, 0, 0]),
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
        property: &NodeProperty,
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

#[derive(Debug)]
pub struct FrameBindContext<'a> {
    composition: &'a Composition,
    frame: u32,
    media: Option<&'a dyn MediaStore>,
}

impl<'a> FrameBindContext<'a> {
    pub fn new(composition: &'a Composition, frame: u32) -> Self {
        Self {
            composition,
            frame,
            media: None,
        }
    }

    pub fn with_media<M: MediaStore>(
        composition: &'a Composition,
        frame: u32,
        media: &'a M,
    ) -> Self {
        Self {
            composition,
            frame,
            media: Some(media),
        }
    }

    pub fn bind(&self, compiled: &CompiledComposition) -> crate::Result<BoundFrame> {
        tracing::trace!(
            target: "lumen_bind",
            frame = self.frame,
            bindings = compiled.frame_bindings.len(),
            "bind compiled frame"
        );
        let mut bound = BoundFrame::new();
        for (index, binding) in compiled.frame_bindings.iter().enumerate() {
            let binding_frame = compiled
                .frame_binding_frames
                .get(index)
                .copied()
                .flatten()
                .unwrap_or(self.frame);
            let binding_context = Self {
                composition: self.composition,
                frame: binding_frame,
                media: self.media,
            };
            let node_id = binding.node_id();
            tracing::trace!(
                target: "lumen_bind",
                frame = self.frame,
                binding_frame,
                node_id = node_id.0,
                binding_index = index,
                "bind frame resource"
            );
            let node =
                self.composition
                    .graph
                    .nodes
                    .get(&node_id)
                    .ok_or(RenderError::MissingNode {
                        frame: self.frame,
                        node_id,
                    })?;
            match node {
                NodeKind::MediaIn(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
                NodeKind::SolidColor(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
                NodeKind::Text(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
                NodeKind::Path(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
                NodeKind::Shape(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
                NodeKind::Boolean(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
                NodeKind::Merge(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
                NodeKind::RasterMultiMerge(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
                NodeKind::AlphaPremultiply(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
                NodeKind::Blur(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
                NodeKind::ChannelShuffle(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
                NodeKind::ColorGrade(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
                NodeKind::Curves(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
                NodeKind::Exposure(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
                NodeKind::HueSaturation(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
                NodeKind::Levels(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
                NodeKind::Memo(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
                NodeKind::TimeRemap(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
                NodeKind::Transform(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
                NodeKind::Crop(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
                NodeKind::Resize(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
                NodeKind::Shadow(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
                NodeKind::WgslShader(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
                NodeKind::Switch(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
                NodeKind::MediaOutput(node) => {
                    node.bind_gpu_frame(&binding_context, binding, &mut bound)?
                }
            }
        }
        Ok(bound)
    }

    pub(crate) fn frame(&self) -> u32 {
        self.frame
    }

    pub(crate) fn media(&self) -> Option<&dyn MediaStore> {
        self.media
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
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct ColorParams {
    pub(crate) color: [f32; 4],
}

impl ColorParams {
    pub(crate) fn from_rgba8(color: [u8; 4]) -> Self {
        Self {
            color: [
                f32::from(color[0]) / 255.0,
                f32::from(color[1]) / 255.0,
                f32::from(color[2]) / 255.0,
                f32::from(color[3]) / 255.0,
            ],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct AlphaPremultiplyParams {
    pub(crate) values: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct ChannelShuffleParams {
    pub(crate) selector_indices: [f32; 4],
    pub(crate) selector_values: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct ColorGradeParams {
    pub(crate) strength: f32,
    pub(crate) interpolation: u32,
    pub(crate) _pad: [u32; 2],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct ColorGradeLut {
    stops: [[f32; 4]; LUT_TABLE_SIZE],
}

impl ColorGradeLut {
    pub(crate) fn parse(node_id: NodeId, frame: u32, source: &str) -> crate::Result<Self> {
        let stops = parse_lut_stops(node_id, frame, source)?;
        let mut table = [[0.0; 4]; LUT_TABLE_SIZE];
        for (index, entry) in table.iter_mut().enumerate() {
            let value = index as f32 / (LUT_TABLE_SIZE - 1) as f32;
            let scaled = value * (stops.len() - 1) as f32;
            let low = scaled.floor() as usize;
            let high = (low + 1).min(stops.len() - 1);
            let t = scaled - low as f32;
            *entry = [
                stops[low][0] + (stops[high][0] - stops[low][0]) * t,
                stops[low][1] + (stops[high][1] - stops[low][1]) * t,
                stops[low][2] + (stops[high][2] - stops[low][2]) * t,
                1.0,
            ];
        }
        Ok(Self { stops: table })
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct ExposureParams {
    pub(crate) exposure: f32,
    pub(crate) contrast: f32,
    pub(crate) offset: f32,
    pub(crate) _pad: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct HueSaturationParams {
    pub(crate) hue_offset: f32,
    pub(crate) saturation: f32,
    pub(crate) lightness: f32,
    pub(crate) _pad: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct LevelsParams {
    pub(crate) black_point: f32,
    pub(crate) white_point: f32,
    pub(crate) gamma: f32,
    pub(crate) output_black: f32,
    pub(crate) output_white: f32,
    pub(crate) _pad: [f32; 3],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct BlurParams {
    pub(crate) values: [u32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct CurvesParams {
    pub(crate) values: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct CurvesTable {
    entries: [[f32; 4]; LUT_TABLE_SIZE],
}

impl CurvesTable {
    pub(crate) fn parse(node_id: NodeId, frame: u32, source: &str) -> crate::Result<Self> {
        let stops = parse_lut_stops(node_id, frame, source)?;
        let mut entries = [[0.0; 4]; LUT_TABLE_SIZE];
        for (index, entry) in entries.iter_mut().enumerate() {
            let value = index as f32 / (LUT_TABLE_SIZE - 1) as f32;
            let scaled = value * (stops.len() - 1) as f32;
            let low = scaled.floor() as usize;
            let high = (low + 1).min(stops.len() - 1);
            let t = scaled - low as f32;
            *entry = [
                stops[low][0] + (stops[high][0] - stops[low][0]) * t,
                stops[low][1] + (stops[high][1] - stops[low][1]) * t,
                stops[low][2] + (stops[high][2] - stops[low][2]) * t,
                1.0,
            ];
        }
        Ok(Self { entries })
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct ShadowParams {
    pub(crate) color: [f32; 4],
    pub(crate) values: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct WgslShaderParams {
    pub(crate) values: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct MergeParams {
    pub(crate) opacity: f32,
    pub(crate) blend_mode: u32,
    pub(crate) has_mask: u32,
    pub(crate) _pad: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct BooleanParams {
    pub(crate) values: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct RasterMultiMergeParams {
    pub(crate) values: [f32; 4],
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ChannelSelector {
    pub(crate) index: f32,
    pub(crate) value: f32,
}

const LUT_TABLE_SIZE: usize = 256;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct TransformParams {
    pub(crate) scale: [f32; 2],
    pub(crate) translate: [f32; 2],
    pub(crate) pivot: [f32; 2],
    pub(crate) rotate_radians: f32,
    pub(crate) sampling: u32,
    pub(crate) _pad: [u32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct CropParams {
    pub(crate) origin: [i32; 2],
    pub(crate) size: [u32; 2],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct ResizeParams {
    pub(crate) size: [u32; 2],
    pub(crate) mode: u32,
    pub(crate) sampling: u32,
}

pub(crate) fn dispatch_for(size: lumen_gpu::Size) -> lumen_gpu::Dispatch {
    lumen_gpu::Dispatch {
        x: size.width.div_ceil(8),
        y: size.height.div_ceil(8),
        z: 1,
    }
}

pub(crate) fn spatial_bindings(
    input: lumen_gpu::TextureId,
    params: lumen_gpu::BufferId,
    output: lumen_gpu::TextureId,
) -> Vec<lumen_gpu::Binding> {
    vec![
        lumen_gpu::Binding::sampled_texture(0, 0, input),
        lumen_gpu::Binding::uniform(0, 1, params),
        lumen_gpu::Binding::storage_texture(0, 2, output),
    ]
}

pub(crate) fn alpha_operation(node_id: NodeId, mode: &str) -> crate::Result<f32> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "premultiply" | "premul" | "multiply" => Ok(0.0),
        "unpremultiply" | "unpremul" | "straight" | "unmultiply" => Ok(1.0),
        _ => Err(crate::error::PropertyError::InvalidType {
            node_id,
            property_path: "mode".to_string(),
            expected: "`premultiply` or `unpremultiply`",
            actual: "String",
        }
        .into()),
    }
}

pub(crate) fn channel_selector(
    node_id: NodeId,
    property_path: &str,
    spec: &str,
) -> crate::Result<ChannelSelector> {
    let normalized = spec.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "r" | "red" => Ok(ChannelSelector {
            index: 0.0,
            value: 0.0,
        }),
        "g" | "green" => Ok(ChannelSelector {
            index: 1.0,
            value: 0.0,
        }),
        "b" | "blue" => Ok(ChannelSelector {
            index: 2.0,
            value: 0.0,
        }),
        "a" | "alpha" => Ok(ChannelSelector {
            index: 3.0,
            value: 0.0,
        }),
        "zero" => Ok(ChannelSelector {
            index: 4.0,
            value: 0.0,
        }),
        "one" => Ok(ChannelSelector {
            index: 4.0,
            value: 1.0,
        }),
        _ => {
            let value = normalized.parse::<f32>().map_err(|_| {
                crate::error::PropertyError::InvalidType {
                    node_id,
                    property_path: property_path.to_string(),
                    expected: "channel name or numeric constant",
                    actual: "String",
                }
            })?;
            Ok(ChannelSelector {
                index: 4.0,
                value: if value <= 1.0 {
                    value.clamp(0.0, 1.0)
                } else {
                    (value / 255.0).clamp(0.0, 1.0)
                },
            })
        }
    }
}

fn parse_lut_stops(node_id: NodeId, frame: u32, source: &str) -> crate::Result<Vec<[f32; 3]>> {
    let source = source.trim();
    if source.is_empty()
        || source.eq_ignore_ascii_case(crate::node::processing::color_grade::IDENTITY_LUT)
    {
        return Ok(vec![[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]]);
    }

    let source = source
        .strip_prefix("rgb1d")
        .and_then(|rest| rest.strip_prefix(':'))
        .unwrap_or(source);
    let mut stops = Vec::new();
    for triplet in source.split(';') {
        let triplet = triplet.trim();
        if triplet.is_empty() {
            continue;
        }
        let components = triplet
            .split([',', ' ', '\t'])
            .filter(|part| !part.is_empty())
            .map(str::parse::<f32>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| lut_error(node_id, frame, "LUT contains a non-numeric component"))?;
        if components.len() != 3 {
            return Err(lut_error(
                node_id,
                frame,
                format!("LUT triplet `{triplet}` must contain exactly three RGB components"),
            ));
        }
        stops.push([
            normalize_lut_component(components[0]),
            normalize_lut_component(components[1]),
            normalize_lut_component(components[2]),
        ]);
    }
    if stops.len() < 2 {
        return Err(lut_error(
            node_id,
            frame,
            "LUT must contain at least two RGB triplets",
        ));
    }
    Ok(stops)
}

fn normalize_lut_component(value: f32) -> f32 {
    if value > 1.0 {
        (value / 255.0).clamp(0.0, 1.0)
    } else {
        value.clamp(0.0, 1.0)
    }
}

fn lut_error(node_id: NodeId, frame: u32, details: impl Into<String>) -> crate::error::LumenError {
    RenderError::NodeEvaluation {
        frame,
        node_id,
        node_kind: "ColorGrade",
        details: details.into(),
    }
    .into()
}

pub(crate) fn copyable_texture_desc(size: lumen_gpu::Size) -> lumen_gpu::TextureDesc {
    lumen_gpu::TextureDesc {
        domain: lumen_gpu::TextureDomain::full_frame(size),
        format: lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
        usage: lumen_gpu::wgpu::TextureUsages::COPY_DST
            | lumen_gpu::wgpu::TextureUsages::COPY_SRC
            | lumen_gpu::wgpu::TextureUsages::TEXTURE_BINDING
            | lumen_gpu::wgpu::TextureUsages::STORAGE_BINDING
            | lumen_gpu::wgpu::TextureUsages::RENDER_ATTACHMENT,
    }
}
