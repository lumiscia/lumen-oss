use std::collections::HashMap;

use crate::{
    composition::Composition,
    error::RenderError,
    expr::ExpressionContext,
    gpu::{
        BoundFrame, CompiledComposition, CompiledOutput, FrameBindContext, GpuCompiledNode,
        RasterHandle, RasterMetadata, TimeRemapPath,
    },
    media::MediaStore,
    node::{NodeId, NodeKind, NodeParams, PortRef, PropertyExpression},
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

/// Longest timeline that animated sources are bounded over, which costs an evaluation of
/// their params per frame each time a plan compiles.
const MAX_UNION_FRAMES: u32 = 3600;

/// The domain a unary filter writes, and so how it reads its source.
#[derive(Debug, Clone, Copy)]
pub(crate) enum FilterDomain {
    /// Reads the source at full frame, texel for texel.
    FullFrame,
    /// Maps each texel on its own and keeps transparent texels transparent, so it reads
    /// and writes the source's domain.
    Source,
    /// `Source` for premultiplied sources. Transparent texels of straight sources may
    /// still carry colour, so those are read at full frame.
    PremultipliedSource,
    /// Reads the source through its domain and writes this one.
    Bounded(lumen_gpu::TextureDomain),
}

/// How a source's canvas bounds vary, which decides how they are held.
#[derive(Debug, Clone, Copy)]
pub(crate) enum SourceMotion {
    /// The bounds never change.
    Fixed,
    /// The bounds follow the frame through the node's own expressions.
    Animated,
    /// The bounds depend on state outside the node, which edits can change after the plan is
    /// cached.
    Unbounded,
}

impl SourceMotion {
    /// The motion of a node whose params are `params`, of which `animated` says whether any
    /// that move the source are expressions.
    pub(crate) fn of<P: NodeParams>(animated: bool, params: &P) -> Self {
        if !animated {
            Self::Fixed
        } else if reads_outside_node(params) {
            Self::Unbounded
        } else {
            Self::Animated
        }
    }
}

/// Whether an expression of `params` reads another node or measures text, whose values live
/// outside of the node and its plan.
fn reads_outside_node<P: NodeParams>(params: &P) -> bool {
    P::property_defs().iter().any(|def| {
        let Some(PropertyExpression::Expr(expr)) = params.get_property(def.id) else {
            return false;
        };
        let mut measures_text = false;
        crate::expr::functions::visit(&expr.ast, &mut |node| {
            measures_text |= matches!(
                node,
                crate::expr::ExprNode::Builtin(
                    crate::expr::BuiltinFn::TextWidth | crate::expr::BuiltinFn::TextHeight,
                    _
                )
            );
        });
        measures_text || !expr.references.is_empty()
    })
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct CompiledPortKey {
    port: PortRef,
    time_remaps: TimeRemapPath,
}

#[derive(Debug)]
pub struct CompileContext<'a> {
    composition: &'a Composition,
    frame: f64,
    time_remaps: TimeRemapPath,
    media: Option<&'a dyn MediaStore>,
    builder: lumen_gpu::RenderPlanBuilder,
    outputs: HashMap<CompiledPortKey, CompiledOutput>,
    public_outputs: HashMap<PortRef, CompiledOutput>,
    compiled_nodes: HashMap<NodeId, HashMap<TimeRemapPath, Box<dyn GpuCompiledNode>>>,
    output_format: lumen_gpu::wgpu::TextureFormat,
    text_atlas_texture: Option<lumen_gpu::TextureId>,
    raster_bounds: HashMap<lumen_gpu::TextureId, lumen_gpu::BufferId>,
    /// Whether the plan is only used for whole frames of the timeline, so animated sources may be
    /// sized to their bounds over it.
    timeline_bounds: bool,
    /// Full-frame copies of bounded rasters, by the bounded texture.
    full_frames: HashMap<lumen_gpu::TextureId, RasterHandle>,
}

impl<'a> CompileContext<'a> {
    pub fn new(composition: &'a Composition) -> Self {
        Self::with_output_format(composition, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm)
    }

    pub fn with_output_format(
        composition: &'a Composition,
        output_format: lumen_gpu::wgpu::TextureFormat,
    ) -> Self {
        Self::with_options(composition, 0.0, None, output_format, false)
    }

    pub fn with_frame(
        composition: &'a Composition,
        frame: u32,
        output_format: lumen_gpu::wgpu::TextureFormat,
    ) -> Self {
        Self::with_options(composition, f64::from(frame), None, output_format, false)
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
        Self::with_options(
            composition,
            f64::from(frame),
            Some(media),
            output_format,
            false,
        )
    }

    /// `timeline_bounds` sizes animated sources to their bounds over the timeline, which holds
    /// only for plans the caller keeps to whole frames of it.
    pub(crate) fn with_options(
        composition: &'a Composition,
        frame: f64,
        media: Option<&'a dyn MediaStore>,
        output_format: lumen_gpu::wgpu::TextureFormat,
        timeline_bounds: bool,
    ) -> Self {
        Self {
            composition,
            frame,
            time_remaps: TimeRemapPath::new(),
            media,
            builder: lumen_gpu::RenderPlan::builder(),
            outputs: HashMap::new(),
            public_outputs: HashMap::new(),
            compiled_nodes: HashMap::new(),
            output_format,
            text_atlas_texture: None,
            raster_bounds: HashMap::new(),
            timeline_bounds,
            full_frames: HashMap::new(),
        }
    }

    /// Compiles a plan whose text atlas must be injected with `CompiledComposition::prepare_text_atlas`.
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
            text_atlas_texture: self.text_atlas_texture,
            text_atlas: std::sync::Arc::new(std::sync::Mutex::new(lumen_text::GlyphAtlas::new(
                super::types::TEXT_ATLAS_SIZE,
            ))),
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

    pub(crate) fn text_atlas_texture(&mut self) -> lumen_gpu::TextureId {
        *self.text_atlas_texture.get_or_insert_with(|| {
            self.builder.texture(
                Some("text:shared-atlas".to_string()),
                lumen_gpu::TextureDesc::sampled(
                    lumen_gpu::Size::new(1, 1),
                    lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                ),
            )
        })
    }

    /// Records the uniform holding a raster's logical bounds (`origin: vec2<i32>`,
    /// `size: vec2<u32>`), written every frame, within its storage texture.
    ///
    /// Bounds are looked up by texture, so they survive nodes that pass the same texture through
    /// and stop at any node that writes a new texture.
    pub(crate) fn publish_raster_bounds(
        &mut self,
        texture: lumen_gpu::TextureId,
        bounds: lumen_gpu::BufferId,
    ) {
        self.raster_bounds.insert(texture, bounds);
    }

    /// The logical bounds of a raster, or `None` when it fills its whole storage texture.
    pub(crate) fn raster_bounds(
        &self,
        texture: lumen_gpu::TextureId,
    ) -> Option<lumen_gpu::BufferId> {
        self.raster_bounds.get(&texture).copied()
    }

    pub(crate) fn builder_mut(&mut self) -> &mut lumen_gpu::RenderPlanBuilder {
        &mut self.builder
    }

    pub(crate) fn register_compiled_node<N>(&mut self, node: N)
    where
        N: GpuCompiledNode + 'static,
    {
        self.compiled_nodes
            .entry(node.node_id())
            .or_default()
            .insert(self.time_remaps.clone(), Box::new(node));
    }

    /// Compiles `port` to a raster that fills its display, for consumers that read their
    /// inputs texel for texel.
    pub(crate) fn compile_port(&mut self, port: &PortRef) -> crate::Result<CompiledOutput> {
        Ok(match self.compile_bounded_port(port)? {
            CompiledOutput::Raster(raster) => CompiledOutput::Raster(self.full_frame(raster)),
            CompiledOutput::Empty => CompiledOutput::Empty,
        })
    }

    /// Compiles `port` as is, for consumers that read a raster's texels through its domain.
    pub(crate) fn compile_bounded_port(&mut self, port: &PortRef) -> crate::Result<CompiledOutput> {
        let key = CompiledPortKey {
            port: port.clone(),
            time_remaps: self.time_remaps.clone(),
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

    /// `raster` stored at its full display size, copied there when it is bounded. The copy's
    /// texels outside the bounded data are never written, so they stay transparent.
    pub(crate) fn full_frame(&mut self, raster: RasterHandle) -> RasterHandle {
        if raster.domain.is_full_frame() {
            return raster;
        }
        if let Some(full) = self.full_frames.get(&raster.texture) {
            return *full;
        }
        let size = raster.domain.display_size();
        let data = raster.domain.data_rect;
        let texture = self.builder.texture(
            Some(format!("full-frame:{}", raster.texture.0)),
            copyable_texture_desc(size),
        );
        self.builder.copy_texture(lumen_gpu::CopyTextureDesc {
            label: Some(format!("full-frame:{}:copy", raster.texture.0)),
            source: raster.texture,
            destination: texture,
            origin: lumen_gpu::wgpu::Origin3d {
                x: data.x as u32,
                y: data.y as u32,
                z: 0,
            },
            size: raster.domain.storage_size,
        });
        let full = RasterHandle {
            texture,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            metadata: raster.metadata,
        };
        self.full_frames.insert(raster.texture, full);
        full
    }

    /// The canvas pixels a source may draw to: the box `bounds_at` gives at the frame when its
    /// `motion` is fixed, or the union of its boxes over every frame of the timeline.
    ///
    /// `None` when the source should fill the canvas instead: `bounds_at` failed or gave
    /// `None` for some frame, the bounds are not finite, the bounds read state outside the node,
    /// the timeline is longer than `MAX_UNION_FRAMES`, or the union cannot be relied on, as in a plan that may be bound
    /// at fractional frames, under a time remap or for a frame off the timeline, since plans
    /// are reused across the frames they are keyed for.
    pub(crate) fn source_bounds(
        &self,
        node_id: NodeId,
        motion: SourceMotion,
        mut bounds_at: impl FnMut(&ExpressionContext<'_>) -> crate::Result<Option<[f32; 4]>>,
    ) -> Option<lumen_gpu::RectI> {
        let mut expr = self.expr_context(node_id, "params");
        match motion {
            SourceMotion::Fixed => {
                return bounds_at(&expr)
                    .ok()
                    .flatten()
                    .and_then(lumen_gpu::RectI::covering);
            }
            SourceMotion::Unbounded => return None,
            SourceMotion::Animated => {}
        }
        if !self.timeline_bounds
            || !self.time_remaps.is_empty()
            || !on_timeline(self.composition, self.frame)
            || self.composition.timeline.duration_frames > MAX_UNION_FRAMES
        {
            return None;
        }
        let mut union = lumen_gpu::RectI::new(0, 0, 0, 0);
        for frame in 0..self.composition.timeline.duration_frames {
            expr.frame = f64::from(frame);
            union = union.union(lumen_gpu::RectI::covering(bounds_at(&expr).ok()??)?);
        }
        Some(union)
    }

    /// The domain of a source drawn within `bounds` on the canvas, or across it without them.
    pub(crate) fn source_domain(
        &self,
        bounds: Option<lumen_gpu::RectI>,
    ) -> lumen_gpu::TextureDomain {
        let canvas = self.canvas_size();
        match bounds {
            Some(bounds) => lumen_gpu::TextureDomain::bounded(canvas, bounds),
            None => lumen_gpu::TextureDomain::full_frame(canvas),
        }
    }

    /// The canvas size, which sources are drawn at.
    pub(crate) fn canvas_size(&self) -> lumen_gpu::Size {
        lumen_gpu::Size::new(
            self.composition.render_settings.width.max(1),
            self.composition.render_settings.height.max(1),
        )
    }

    pub(crate) fn with_frame_context<T>(
        &mut self,
        time_remap: NodeId,
        frame: f64,
        f: impl FnOnce(&mut Self) -> crate::Result<T>,
    ) -> crate::Result<T> {
        let original_frame = self.frame;
        self.frame = frame;
        self.time_remaps.push(time_remap);
        let result = f(self);
        self.time_remaps.pop();
        self.frame = original_frame;
        result
    }

    /// Compiles a compute pass from `source_ref` into a new texture of the domain `domain`
    /// picks. Returns the source as read, which is at full frame unless `domain` reads it
    /// through its domain.
    #[allow(
        clippy::too_many_arguments,
        reason = "every filter node passes the same flat set of compile inputs"
    )]
    pub(crate) fn compile_unary_filter(
        &mut self,
        node_id: NodeId,
        source_ref: &PortRef,
        port: &PortRef,
        label: &str,
        shader: &str,
        params_size: u64,
        domain: FilterDomain,
    ) -> crate::Result<(RasterHandle, lumen_gpu::TextureId, lumen_gpu::BufferId)> {
        if port.port != "output" {
            return Err(self.missing_output(node_id, &port.port));
        }

        let source = self
            .compile_bounded_port(source_ref)?
            .into_raster(source_ref.id, &source_ref.port)?;
        let straight = source.metadata.alpha_mode == crate::gpu::AlphaMode::Unpremultiplied;
        let source = match domain {
            FilterDomain::FullFrame => self.full_frame(source),
            FilterDomain::PremultipliedSource if straight => self.full_frame(source),
            _ => source,
        };
        let domain = match domain {
            FilterDomain::Bounded(domain) => domain,
            _ => source.domain,
        };
        let size = domain.storage_size;
        let texture = self.builder.texture_for(
            lumen_gpu::NodeKey(node_id.0),
            Some(format!("{label}:{}:output", node_id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm)
                .with_domain(domain),
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

    /// A canvas-sized transparent raster, which stores a single texel.
    pub(crate) fn compile_transparent(&mut self, node_id: NodeId) -> CompiledOutput {
        let domain = lumen_gpu::TextureDomain::bounded(
            self.canvas_size(),
            lumen_gpu::RectI::new(0, 0, 0, 0),
        );
        let size = domain.storage_size;
        let texture = self.builder.texture_for(
            lumen_gpu::NodeKey(node_id.0),
            Some(format!("transparent:{}:output", node_id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm)
                .with_domain(domain),
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
            domain,
            metadata: RasterMetadata::default(),
        })
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
            media: self.media,
            glyph: None,
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

/// Whether `frame` is a whole frame of the composition's timeline, which bounds unioned over
/// the timeline hold for.
pub(crate) fn on_timeline(composition: &Composition, frame: f64) -> bool {
    frame.fract() == 0.0 && frame >= 0.0 && frame < f64::from(composition.timeline.duration_frames)
}
