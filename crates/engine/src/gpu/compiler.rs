use std::{collections::HashMap, sync::Arc};

use crate::{
    composition::Composition,
    error::RenderError,
    expr::ExpressionContext,
    gpu::{
        BoundFrame, CompiledComposition, CompiledOutput, FrameBindContext, GpuCompiledNode,
        RasterHandle, RasterMetadata, TimeRemapPath,
    },
    media::MediaStore,
    node::{
        ClipPropValue, NodeId, NodeKind, NodeParams, PortRef, PropertyExpression,
        compositing::clip::Clip,
    },
    scope::{PropValues, Routed, Scope},
};

pub(crate) use super::params::*;

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
        matches!(
            params.get_property(def.id),
            Some(PropertyExpression::Expr(expr)) if expr_reads_outside_node(&expr)
        )
    })
}

fn expr_reads_outside_node(expr: &crate::expr::Expression) -> bool {
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
}

/// Whether a clip's timing or prop expressions read state outside the node, which edits can
/// change without changing its definition.
fn clip_reads_outside_node(clip: &Clip) -> bool {
    reads_outside_node(&clip.params)
        || clip.props().0.values().any(
            |value| matches!(value, ClipPropValue::Expr(expr) if expr_reads_outside_node(expr)),
        )
}

/// Copies the visible part of a clip's raster into a texture of the parent canvas.
const CLIP_CROP_SHADER: &str = "
struct Params {
    offset: vec2<i32>,
    _pad: vec2<i32>,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let texel = textureLoad(input_tex, vec2<i32>(id.xy) + params.offset, 0);
    textureStore(output_tex, vec2<i32>(id.xy), texel);
}
";

#[derive(Debug, Clone)]
struct ClipCropBinding {
    node_id: NodeId,
    offset: [i32; 2],
    buffer: lumen_gpu::BufferId,
}

impl GpuCompiledNode for ClipCropBinding {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, _ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let [x, y] = self.offset;
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&[x, y, 0, 0]));
        Ok(())
    }
}

/// A frame a plan may bind a scope at, with the props the scope then has.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlanFrame {
    frame: f64,
    props: Option<Arc<PropValues>>,
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct CompiledPortKey {
    port: PortRef,
    time_remaps: TimeRemapPath,
}

#[derive(Debug)]
pub struct CompileContext<'a> {
    composition: &'a Composition,
    /// The composition the nodes being compiled belong to.
    scope: Scope<'a>,
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
    /// Every frame the plan may bind the current scope at, so animated sources may be sized to
    /// their bounds over them; `None` when that is unknown.
    bound_frames: Option<Arc<[PlanFrame]>>,
    /// Full-frame copies of bounded rasters, by the bounded texture and display size.
    full_frames: HashMap<(lumen_gpu::TextureId, u32, u32), RasterHandle>,
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
        let duration = composition.timeline.duration_frames;
        let bound_frames =
            (timeline_bounds && on_timeline(composition, frame) && duration <= MAX_UNION_FRAMES)
                .then(|| {
                    (0..duration)
                        .map(|frame| PlanFrame {
                            frame: f64::from(frame),
                            props: None,
                        })
                        .collect()
                });
        Self {
            composition,
            scope: Scope::root(composition),
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
            bound_frames,
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
        let output_node = self.scope.media_output()?;
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

        let node = self.scope.node(port.id).ok_or(RenderError::MissingNode {
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
            NodeKind::Clip(node) => node.compile_gpu(self, port)?,
            NodeKind::MediaOutput(node) => node.compile_gpu(self, port)?,
        };

        self.outputs.insert(key, output.clone());
        if std::ptr::eq(self.scope.graph, &self.composition.graph) {
            self.public_outputs
                .entry(port.clone())
                .or_insert_with(|| output.clone());
        }
        Ok(output)
    }

    /// Compiles the input `routed` names, in its frame, scope and instance. `bound_frames` are
    /// the frames the plan may bind it at.
    fn compile_routed(
        &mut self,
        routed: Routed<'a>,
        bound_frames: Option<Arc<[PlanFrame]>>,
    ) -> crate::Result<CompiledOutput> {
        let frame = std::mem::replace(&mut self.frame, routed.frame);
        let scope = std::mem::replace(&mut self.scope, routed.scope);
        let outer_frames = std::mem::replace(&mut self.bound_frames, bound_frames);
        self.time_remaps.extend(routed.instance);
        let result = self.compile_bounded_port(&routed.port);
        if routed.instance.is_some() {
            self.time_remaps.pop();
        }
        self.bound_frames = outer_frames;
        self.scope = scope;
        self.frame = frame;
        result
    }

    /// Compiles a time remap's source at the frame it maps to. The frames that plan may bind it
    /// at are not tracked, so its animated sources fill the canvas.
    pub(crate) fn compile_time_remap(
        &mut self,
        node: &crate::node::processing::time_remap::TimeRemap,
    ) -> crate::Result<CompiledOutput> {
        let routed = crate::scope::route_time_remap(&self.scope, node, self.frame, self.media)?;
        self.compile_routed(routed, None)
    }

    /// Compiles what a clip shows at this frame, cropped to its content's canvas and placed on
    /// this composition's, or a transparent raster while it is hidden.
    pub(crate) fn compile_clip(&mut self, clip: &Clip) -> crate::Result<CompiledOutput> {
        let routed = clip.route(&self.scope, self.frame, self.media)?;
        let Some(routed) = routed.filter(|routed| !routed.port.is_empty()) else {
            return Ok(self.compile_transparent(clip.id));
        };
        let settings = routed.scope.render_settings;
        let content = lumen_gpu::Size::new(settings.width.max(1), settings.height.max(1));
        let bound_frames = self.clip_bound_frames(clip);
        let raster = self
            .compile_routed(routed, bound_frames)?
            .into_raster(clip.id, "output")?;
        Ok(self.place_on_canvas(clip.id, raster, content))
    }

    /// The frames a clip's content may be bound at in this plan: each frame the plan may bind
    /// this scope at where the clip shows, in the content's time and with its props. `None`
    /// when they are unknown, as when the clip's expressions read state outside of it.
    fn clip_bound_frames(&self, clip: &Clip) -> Option<Arc<[PlanFrame]>> {
        if clip_reads_outside_node(clip) {
            return None;
        }
        let frames = self.bound_frames.as_ref()?;
        let mut scope = self.scope.clone();
        let mut local = Vec::new();
        for frame in frames.iter() {
            scope.props.clone_from(&frame.props);
            if let Some(routed) = clip.route(&scope, frame.frame, self.media).ok()? {
                local.push(PlanFrame {
                    frame: routed.frame,
                    props: routed.scope.props,
                });
            }
        }
        local.dedup();
        Some(local.into())
    }

    /// `raster`, drawn on a canvas of size `content` from its top-left corner, as a raster of
    /// this canvas. It keeps its texture when all of its data is on both canvases, and is
    /// cropped to them otherwise.
    fn place_on_canvas(
        &mut self,
        node_id: NodeId,
        raster: RasterHandle,
        content: lumen_gpu::Size,
    ) -> CompiledOutput {
        let canvas = self.canvas_size();
        let canvas_rect = lumen_gpu::RectI::from_size(canvas);
        let data = raster.domain.data_rect;
        let visible = canvas_rect
            .intersection(lumen_gpu::RectI::from_size(content))
            .intersection(data);
        if visible.is_empty() {
            return self.compile_transparent(node_id);
        }
        if visible == data && raster.domain.display_size() == canvas {
            return CompiledOutput::Raster(raster);
        }
        if visible == data {
            return CompiledOutput::Raster(RasterHandle {
                domain: lumen_gpu::TextureDomain {
                    storage_size: raster.domain.storage_size,
                    display_rect: canvas_rect,
                    data_rect: data,
                },
                ..raster
            });
        }
        let domain = lumen_gpu::TextureDomain::bounded(canvas, visible);
        let size = domain.storage_size;
        let texture = self.builder.texture_for(
            lumen_gpu::NodeKey(node_id.0),
            Some(format!("clip:{}:crop", node_id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm)
                .with_domain(domain),
        );
        let params = self.builder.buffer_for(
            lumen_gpu::NodeKey(node_id.0),
            Some(format!("clip:{}:crop-params", node_id.0)),
            lumen_gpu::BufferDesc::uniform(16),
        );
        let program = self.spatial_program(node_id, "clip-crop", CLIP_CROP_SHADER);
        self.builder.compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("clip:{}:crop", node_id.0)),
            owner: Some(lumen_gpu::NodeKey(node_id.0)),
            program,
            bindings: spatial_bindings(raster.texture, params, texture),
            dispatch: dispatch_for(size).into(),
        });
        self.builder.param(
            lumen_gpu::ParamKey {
                owner: lumen_gpu::NodeKey(node_id.0),
                slot: 0,
            },
            lumen_gpu::ParamTarget::Buffer(params),
        );
        self.register_compiled_node(ClipCropBinding {
            node_id,
            offset: [domain.data_rect.x - data.x, domain.data_rect.y - data.y],
            buffer: params,
        });
        CompiledOutput::Raster(RasterHandle {
            texture,
            domain,
            metadata: raster.metadata,
        })
    }

    /// `raster` stored at its full display size, copied there when it is bounded. The copy's
    /// texels outside the bounded data are never written, so they stay transparent.
    pub(crate) fn full_frame(&mut self, raster: RasterHandle) -> RasterHandle {
        if raster.domain.is_full_frame() {
            return raster;
        }
        let size = raster.domain.display_size();
        if let Some(full) = self
            .full_frames
            .get(&(raster.texture, size.width, size.height))
        {
            return *full;
        }
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
        self.full_frames
            .insert((raster.texture, size.width, size.height), full);
        full
    }

    /// The canvas pixels a source may draw to: the box `bounds_at` gives at the frame when its
    /// `motion` is fixed, or the union of its boxes over every frame the plan may bind it at.
    ///
    /// `None` when the source should fill the canvas instead: `bounds_at` failed or gave
    /// `None` for some frame, the bounds are not finite, the bounds read state outside the node,
    /// there are more than `MAX_UNION_FRAMES` frames, or the frames are unknown, as in a plan
    /// that may be bound at fractional frames, under a time remap or for a frame off the
    /// timeline, since plans are reused across the frames they are keyed for. Inside a clip the
    /// frames are the clip's own, with the props it has at each.
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
        let frames = self.bound_frames.as_deref()?;
        if frames.len() > MAX_UNION_FRAMES as usize {
            return None;
        }
        let mut union = lumen_gpu::RectI::new(0, 0, 0, 0);
        for frame in frames {
            expr.frame = frame.frame;
            expr.props = frame.props.as_deref();
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

    /// The canvas size of the composition being compiled, which its sources are drawn at.
    pub(crate) fn canvas_size(&self) -> lumen_gpu::Size {
        let settings = self.scope.render_settings;
        lumen_gpu::Size::new(settings.width.max(1), settings.height.max(1))
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

    /// A canvas-sized transparent raster, which stores a single texel. Textures start zeroed
    /// and nothing writes this one, so it needs no pass.
    pub(crate) fn compile_transparent(&mut self, node_id: NodeId) -> CompiledOutput {
        let domain = lumen_gpu::TextureDomain::bounded(
            self.canvas_size(),
            lumen_gpu::RectI::new(0, 0, 0, 0),
        );
        let texture = self.builder.texture_for(
            lumen_gpu::NodeKey(node_id.0),
            Some(format!("transparent:{}:output", node_id.0)),
            lumen_gpu::TextureDesc::storage(
                domain.storage_size,
                lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            )
            .with_domain(domain),
        );
        CompiledOutput::Raster(RasterHandle {
            texture,
            domain,
            metadata: RasterMetadata::default(),
        })
    }

    pub(crate) fn static_dimension_value(&self, value: i64, property_path: &str) -> u32 {
        let value = if value <= 0 {
            match property_path {
                "width" => i64::from(self.scope.render_settings.width),
                "height" => i64::from(self.scope.render_settings.height),
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

    pub(crate) fn expr_context(
        &self,
        node_id: NodeId,
        property_path: &str,
    ) -> ExpressionContext<'_> {
        self.scope
            .expr_context(self.frame, format!("{node_id}.{property_path}"), self.media)
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
