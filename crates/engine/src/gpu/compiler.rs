use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::{
    composition::Composition,
    error::RenderError,
    expr::ExpressionContext,
    gpu::{
        BoundFrame, CompiledComposition, FrameBindContext, GpuCompiledNode, RasterHandle,
        TimeRemapPath, types::CompiledPortKey,
    },
    media::MediaStore,
    node::{
        DelegateEvalContext, DelegateEvaluable, NodeId, compositing::clip::Clip, required_input,
    },
    scope::{FrameTime, PropValues, Routed, Scope},
};

pub(crate) use super::params::*;

pub trait GpuCompileNode {
    fn compile_gpu(&self, ctx: &mut CompileContext<'_>) -> crate::Result<RasterHandle>;
}

/// Longest timeline that animated sources are bounded over, which costs an evaluation of
/// their params per frame each time a plan compiles.
const MAX_UNION_FRAMES: u32 = 3600;

/// What `CompileContext::compute_source` added to the plan.
pub(crate) struct ComputeSource<const STORAGE: usize> {
    pub(crate) output: RasterHandle,
    pub(crate) params: lumen_gpu::BufferId,
    pub(crate) storage: [lumen_gpu::BufferId; STORAGE],
}

/// What one binding of a compute program holds. Every binding is visible to the compute stage.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Slot {
    Texture,
    Uniform,
    StorageRead,
    Sampler,
    /// A write-only `Rgba8Unorm` storage texture.
    Output,
    /// A write-only `Rgba16Float` storage texture.
    OutputFloat,
}

/// The program of a compute pass whose bindings are `slots`, each at the index it is listed
/// at in `slots`, in the entry point `entry` of `shader`.
pub(crate) fn compute_program_at(
    builder: &mut lumen_gpu::RenderPlanBuilder,
    label: &str,
    shader: &str,
    entry: &str,
    slots: &[(u32, Slot)],
) -> lumen_gpu::ProgramId {
    use lumen_gpu::{BindingLayoutEntry as Entry, wgpu};
    let stage = wgpu::ShaderStages::COMPUTE;
    let storage_texture = |binding, format| {
        Entry::storage_texture(
            binding,
            stage,
            format,
            wgpu::StorageTextureAccess::WriteOnly,
        )
    };
    let layout = slots
        .iter()
        .map(|&(binding, slot)| match slot {
            Slot::Texture => Entry::texture(binding, stage),
            Slot::Uniform => Entry::uniform(binding, stage),
            Slot::StorageRead => Entry::storage(binding, stage, true),
            Slot::Sampler => Entry::sampler(binding, stage),
            Slot::Output => storage_texture(binding, wgpu::TextureFormat::Rgba8Unorm),
            Slot::OutputFloat => storage_texture(binding, wgpu::TextureFormat::Rgba16Float),
        })
        .collect();
    builder.program(lumen_gpu::ProgramDesc::Compute(
        lumen_gpu::ComputeProgramDesc {
            label: Some(label.to_string()),
            shader: shader.to_string(),
            entry: entry.to_string(),
            bind_groups: lumen_gpu::BindGroupLayoutSpec::single(layout),
        },
    ))
}

/// The domain a unary filter writes, and so how it reads its source.
#[derive(Debug, Clone, Copy)]
pub(crate) enum FilterDomain {
    /// Maps each texel on its own and keeps transparent texels transparent, so it reads
    /// and writes the source's domain.
    Source,
    /// Reads the source through its domain and writes this one.
    Bounded(lumen_gpu::TextureDomain),
}

/// A unary filter at bind time, which writes the uniforms `pack` builds from its evaluated
/// params.
#[derive(Clone)]
struct CompiledFilter<D: DelegateEvaluable, U> {
    node_id: NodeId,
    params: D,
    buffer: lumen_gpu::BufferId,
    pack: fn(&D::Evaluated) -> U,
}

impl<D: DelegateEvaluable + std::fmt::Debug, U> std::fmt::Debug for CompiledFilter<D, U> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledFilter")
            .field("node_id", &self.node_id)
            .field("params", &self.params)
            .finish_non_exhaustive()
    }
}

impl<D, U> GpuCompiledNode for CompiledFilter<D, U>
where
    D: DelegateEvaluable + std::fmt::Debug + Send + Sync,
    U: bytemuck::Pod,
{
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let evaluated = self.params.eval(&DelegateEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(),
        })?;
        let uniforms = (self.pack)(&evaluated);
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&uniforms));
        Ok(())
    }
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
    pub(crate) fn of(animated: bool, params: &impl DelegateEvaluable) -> Self {
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
fn reads_outside_node(params: &impl DelegateEvaluable) -> bool {
    let mut outside = false;
    params.for_each_expression(&mut |expr| outside |= expr.reads.outside_node);
    outside
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

pub(crate) const fn clip_crop_shader() -> &'static str {
    CLIP_CROP_SHADER
}

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
    frame: FrameTime,
    props: Option<Arc<PropValues>>,
}

#[derive(Debug)]
pub struct CompileContext<'a> {
    /// The composition the nodes being compiled belong to.
    scope: Scope<'a>,
    frame: FrameTime,
    time_remaps: TimeRemapPath,
    media: Option<&'a dyn MediaStore>,
    builder: lumen_gpu::RenderPlanBuilder,
    outputs: HashMap<CompiledPortKey, RasterHandle>,
    compiled_nodes: HashMap<NodeId, HashMap<TimeRemapPath, Box<dyn GpuCompiledNode>>>,
    output_format: lumen_gpu::wgpu::TextureFormat,
    text_atlas_texture: Option<lumen_gpu::TextureId>,
    /// Every frame the plan may bind the current scope at, so animated sources may be sized to
    /// their bounds over them; `None` when that is unknown.
    bound_frames: Option<Arc<[PlanFrame]>>,
    /// Full-frame copies of bounded rasters, by the bounded texture and display size.
    full_frames: HashMap<(lumen_gpu::TextureId, u32, u32), RasterHandle>,
    /// Draws every source across the canvas rather than within its bounds.
    full_frame_sources: bool,
    cache: super::cache::CacheRecorder,
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

    /// Disables automatic reuse of static node output from earlier frames.
    pub(crate) fn render_static(mut self, render_static: bool) -> Self {
        self.cache.render_static = render_static;
        self
    }

    /// Makes `source_bounds` give `None` for every source, so each fills the canvas.
    pub(crate) fn full_frame_sources(mut self, full_frame_sources: bool) -> Self {
        self.full_frame_sources = full_frame_sources;
        self
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
        let duration = composition.timeline().duration_frames;
        let bound_frames =
            (timeline_bounds && on_timeline(composition, frame) && duration <= MAX_UNION_FRAMES)
                .then(|| {
                    (0..duration)
                        .map(|frame| PlanFrame {
                            frame: f64::from(frame).into(),
                            props: None,
                        })
                        .collect()
                });
        Self {
            scope: Scope::root(composition),
            frame: frame.into(),
            time_remaps: TimeRemapPath::new(),
            media,
            builder: lumen_gpu::RenderPlan::builder(),
            outputs: HashMap::new(),
            compiled_nodes: HashMap::new(),
            output_format,
            text_atlas_texture: None,
            bound_frames,
            full_frames: HashMap::new(),
            full_frame_sources: false,
            cache: super::cache::CacheRecorder::default(),
        }
    }

    /// Compiles a plan whose text atlas must be injected with `CompiledComposition::prepare_text_atlas`.
    pub fn compile(self) -> crate::Result<CompiledComposition> {
        let atlas = lumen_text::GlyphAtlas::new(super::types::TEXT_ATLAS_SIZE);
        self.compile_with_atlas(Arc::new(Mutex::new(atlas)))
    }

    /// Compiles a plan that places its text in `text_atlas`.
    pub(crate) fn compile_with_atlas(
        mut self,
        text_atlas: Arc<Mutex<lumen_text::GlyphAtlas>>,
    ) -> crate::Result<CompiledComposition> {
        let output_node = self.scope.output;
        let output = self.compile_port(output_node)?;
        let plan = self.builder.build();
        let cache = self.cache.finish(
            self.scope.root,
            &plan,
            output.texture,
            self.text_atlas_texture,
        );
        Ok(CompiledComposition {
            plan,
            cache,
            output,
            outputs: self.outputs,
            compiled_nodes: self.compiled_nodes,
            text_atlas_texture: self.text_atlas_texture,
            text_atlas,
        })
    }

    /// The root composition, whichever scope is being compiled.
    pub(crate) fn composition(&self) -> &Composition {
        self.scope.root
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

    /// Compiles `node_id` to a raster that fills its display, for consumers that read their
    /// inputs texel for texel.
    pub(crate) fn compile_port(&mut self, node_id: NodeId) -> crate::Result<RasterHandle> {
        let raster = self.compile_bounded_port(node_id)?;
        Ok(self.full_frame(raster))
    }

    /// Compiles the required input `port` of `node_id` to a raster that fills its display.
    pub(crate) fn compile_input(
        &mut self,
        node_id: NodeId,
        port: &str,
        input: Option<NodeId>,
    ) -> crate::Result<RasterHandle> {
        self.compile_port(required_input(node_id, port, input)?)
    }

    /// Compiles the required input `port` of `node_id` as is.
    pub(crate) fn compile_bounded_input(
        &mut self,
        node_id: NodeId,
        port: &str,
        input: Option<NodeId>,
    ) -> crate::Result<RasterHandle> {
        self.compile_bounded_port(required_input(node_id, port, input)?)
    }

    /// Compiles `node_id` as is, for consumers that read a raster's texels through its domain.
    pub(crate) fn compile_bounded_port(&mut self, node_id: NodeId) -> crate::Result<RasterHandle> {
        let key = CompiledPortKey {
            node: node_id,
            time_remaps: self.time_remaps.clone(),
        };
        self.cache.enter(&self.builder, node_id, &self.time_remaps);
        let output = match self.outputs.get(&key) {
            Some(&output) => Ok(output),
            None => self.compile_node(node_id),
        };
        self.cache.pop(&self.builder);
        let output = output?;
        self.outputs.insert(key, output);
        Ok(output)
    }

    fn compile_node(&mut self, node_id: NodeId) -> crate::Result<RasterHandle> {
        let node = self
            .scope
            .node(node_id)
            .ok_or(RenderError::MissingNode { node_id })?;
        node.compile_gpu(self)
    }

    /// Compiles the input `routed` names, in its frame, scope and instance. `bound_frames` are
    /// the frames the plan may bind it at.
    fn compile_routed(
        &mut self,
        routed: Routed<'a>,
        bound_frames: Option<Arc<[PlanFrame]>>,
    ) -> crate::Result<RasterHandle> {
        let frame = std::mem::replace(&mut self.frame, routed.frame);
        let scope = std::mem::replace(&mut self.scope, routed.scope);
        let outer_frames = std::mem::replace(&mut self.bound_frames, bound_frames);
        self.time_remaps.extend(routed.instance);
        let result = self.compile_bounded_port(routed.node);
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
    ) -> crate::Result<RasterHandle> {
        let routed = crate::scope::route_time_remap(&self.scope, node, self.frame, self.media)?;
        self.compile_routed(routed, None)
    }

    /// Compiles what a clip shows at this frame, cropped to its content's canvas and placed on
    /// this composition's, or a transparent raster while it is hidden.
    pub(crate) fn compile_clip(&mut self, clip: &Clip) -> crate::Result<RasterHandle> {
        let routed = clip.route(&self.scope, self.frame, self.media)?;
        let Some(routed) = routed else {
            return Ok(self.compile_transparent(clip.id));
        };
        let settings = &routed.scope.scene.render_settings;
        let content = lumen_gpu::Size::new(settings.width, settings.height);
        let bound_frames = self.clip_bound_frames(clip);
        let raster = self.compile_routed(routed, bound_frames)?;
        Ok(self.place_on_canvas(clip.id, raster, content))
    }

    /// The frames a clip's content may be bound at in this plan: each frame the plan may bind
    /// this scope at where the clip shows, in the content's time and with its props. `None`
    /// when they are unknown, as when the clip's expressions read state outside of it.
    fn clip_bound_frames(&self, clip: &Clip) -> Option<Arc<[PlanFrame]>> {
        if reads_outside_node(&clip.params) {
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
    ) -> RasterHandle {
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
            return raster;
        }
        if visible == data {
            return RasterHandle {
                domain: lumen_gpu::TextureDomain {
                    storage_size: raster.domain.storage_size,
                    display_rect: canvas_rect,
                    data_rect: data,
                },
                ..raster
            };
        }
        let domain = lumen_gpu::TextureDomain::bounded(canvas, visible);
        let crop = self.compute_source(
            node_id,
            "clip-crop",
            "apply",
            CLIP_CROP_SHADER,
            16,
            Some(raster.texture),
            [],
            domain,
        );
        self.register_compiled_node(ClipCropBinding {
            node_id,
            offset: [domain.data_rect.x - data.x, domain.data_rect.y - data.y],
            buffer: crop.params,
        });
        crop.output
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
        // The copy belongs to the raster's node, so it is kept and dispatched with the raster.
        let owner = self.cache.texture_owner(raster.texture);
        if let Some(owner) = owner {
            self.cache.push(&self.builder, owner);
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
        if owner.is_some() {
            self.cache.pop(&self.builder);
        }
        let full = RasterHandle {
            texture,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            bounds: None,
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
    /// or the frames are unknown, as in a plan on a timeline over `MAX_UNION_FRAMES` or that may
    /// be bound at fractional frames, under a time remap or for a frame off the timeline, since
    /// plans are reused across the frames they are keyed for. Inside a clip the frames are the
    /// clip's own, with the props it has at each.
    pub(crate) fn source_bounds(
        &self,
        motion: SourceMotion,
        mut bounds_at: impl FnMut(&ExpressionContext<'_>) -> crate::Result<Option<[f32; 4]>>,
    ) -> Option<lumen_gpu::RectI> {
        if self.full_frame_sources {
            return None;
        }
        let mut expr = self.expr_context();
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
        let mut union = lumen_gpu::RectI::new(0, 0, 0, 0);
        for frame in frames {
            expr.frame = frame.frame.value();
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
        let settings = &self.scope.scene.render_settings;
        lumen_gpu::Size::new(settings.width, settings.height)
    }

    /// Compiles a compute pass from the `source` input into a new texture of the domain `domain`
    /// picks. Returns the source as read.
    #[allow(
        clippy::too_many_arguments,
        reason = "every filter node passes the same flat set of compile inputs"
    )]
    pub(crate) fn compile_unary_filter(
        &mut self,
        node_id: NodeId,
        source: Option<NodeId>,
        label: &str,
        shader: &str,
        params_size: u64,
        domain: FilterDomain,
    ) -> crate::Result<(RasterHandle, lumen_gpu::TextureId, lumen_gpu::BufferId)> {
        let source = self.compile_bounded_input(node_id, "source", source)?;
        let domain = match domain {
            FilterDomain::Bounded(domain) => domain,
            _ => source.domain,
        };
        let compute = self.compute_source(
            node_id,
            label,
            "apply",
            shader,
            params_size,
            Some(source.texture),
            [],
            domain,
        );
        Ok((source, compute.output.texture, compute.params))
    }

    /// Compiles a filter that maps each texel of its `source` input with the uniforms `pack`
    /// builds from `params`. Returns the output, which has the source's domain and bounds.
    pub(crate) fn unary_filter<D, U>(
        &mut self,
        node_id: NodeId,
        source: Option<NodeId>,
        label: &str,
        shader: &str,
        params: &D,
        pack: fn(&D::Evaluated) -> U,
    ) -> crate::Result<RasterHandle>
    where
        D: DelegateEvaluable + std::fmt::Debug + Send + Sync + 'static,
        U: bytemuck::Pod,
    {
        let (source, texture, buffer) = self.compile_unary_filter(
            node_id,
            source,
            label,
            shader,
            std::mem::size_of::<U>() as u64,
            FilterDomain::Source,
        )?;
        self.register_compiled_node(CompiledFilter {
            node_id,
            params: params.clone(),
            buffer,
            pack,
        });
        Ok(RasterHandle {
            texture,
            domain: source.domain,
            bounds: source.bounds,
        })
    }

    /// Adds a compute pass that writes a new texture over the texels of `domain`. Its bindings
    /// are, in order, `source` if there is one, the uniforms, a read-only storage buffer of each
    /// name and size in `storage`, and the output.
    #[allow(
        clippy::too_many_arguments,
        reason = "every compute source passes the same flat set of compile inputs"
    )]
    pub(crate) fn compute_source<const STORAGE: usize>(
        &mut self,
        node_id: NodeId,
        label: &str,
        pass: &str,
        shader: &str,
        uniforms_size: u64,
        source: Option<lumen_gpu::TextureId>,
        storage: [(&str, u64); STORAGE],
        domain: lumen_gpu::TextureDomain,
    ) -> ComputeSource<STORAGE> {
        let size = domain.storage_size;
        let texture = self.builder.texture(
            Some(format!("{label}:{}:output", node_id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm)
                .with_domain(domain),
        );
        let params = self.builder.buffer(
            Some(format!("{label}:{}:params", node_id.0)),
            lumen_gpu::BufferDesc::uniform(uniforms_size),
        );
        let storage = storage.map(|(name, size)| {
            self.builder.buffer(
                Some(format!("{label}:{}:{name}", node_id.0)),
                lumen_gpu::BufferDesc::storage(size),
            )
        });

        let mut slots = Vec::new();
        let mut bindings = Vec::new();
        if let Some(source) = source {
            slots.push(Slot::Texture);
            bindings.push(lumen_gpu::Binding::sampled_texture(0, 0, source));
        }
        slots.push(Slot::Uniform);
        bindings.push(lumen_gpu::Binding::uniform(
            0,
            bindings.len() as u32,
            params,
        ));
        for buffer in storage {
            slots.push(Slot::StorageRead);
            bindings.push(lumen_gpu::Binding::storage_buffer(
                0,
                bindings.len() as u32,
                buffer,
            ));
        }
        slots.push(Slot::Output);
        bindings.push(lumen_gpu::Binding::storage_texture(
            0,
            bindings.len() as u32,
            texture,
        ));
        let program = self.compute_program(label, shader, &slots);
        self.builder.compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("{label}:{}:{pass}", node_id.0)),
            program,
            bindings,
            dispatch: dispatch_for(size),
        });
        ComputeSource {
            output: RasterHandle {
                texture,
                domain,
                bounds: None,
            },
            params,
            storage,
        }
    }

    /// A canvas-sized transparent raster, which stores a single texel. Textures start zeroed
    /// and nothing writes this one, so it needs no pass.
    pub(crate) fn compile_transparent(&mut self, node_id: NodeId) -> RasterHandle {
        let domain = lumen_gpu::TextureDomain::bounded(
            self.canvas_size(),
            lumen_gpu::RectI::new(0, 0, 0, 0),
        );
        let texture = self.builder.texture(
            Some(format!("transparent:{}:output", node_id.0)),
            lumen_gpu::TextureDesc::storage(
                domain.storage_size,
                lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            )
            .with_domain(domain),
        );
        RasterHandle {
            texture,
            domain,
            bounds: None,
        }
    }

    /// The program of a compute pass with entry point `cs_main` whose binding at each index is
    /// the slot at the same position in `slots`.
    pub(crate) fn compute_program(
        &mut self,
        label: &str,
        shader: &str,
        slots: &[Slot],
    ) -> lumen_gpu::ProgramId {
        let slots: Vec<_> = (0..).zip(slots.iter().copied()).collect();
        compute_program_at(&mut self.builder, label, shader, "cs_main", &slots)
    }

    pub(crate) fn expr_context(&self) -> ExpressionContext<'_> {
        self.scope.expr_context(self.frame, self.media)
    }
}

/// Whether `frame` is a whole frame of the composition's timeline, which bounds unioned over
/// the timeline hold for.
pub(crate) fn on_timeline(composition: &Composition, frame: f64) -> bool {
    frame.fract() == 0.0
        && frame >= 0.0
        && frame < f64::from(composition.timeline().duration_frames)
}
