//! Records each frame's passes directly from the graph.
//!
//! A frame walks from the media output, evaluating each node once and recording its passes into
//! pooled textures, sized to what the node draws this frame. Outputs of static subtrees are
//! retained, so frames after the first skip those subtrees entirely.
//!
//! An encoder never holds a pooled texture. It holds an external target or what a target copied
//! out of a frame's output, and neither joins the pool, so eviction and reuse can't reach them.
//! A frame's output stays valid for whatever the host submits next in queue order, since the
//! next frame's writes to it come after that.

use std::sync::{Arc, Mutex};

use bytemuck::{Pod, Zeroable};
use foldhash::HashSet;
use lumen_gpu::{
    LayoutEntry, LayoutKind, ProgramRef, Recorder, Size, TexRef, TextureDesc, TextureDomain, wgpu,
};

use crate::{
    composition::{Composition, SceneId},
    error::{LumenError, RenderError, Warning, WarningKind},
    expr::{ExpressionContext, HeldCache},
    media::{MediaIdentity, MediaStore},
    node::{
        Deferred, MediaValue, Node, NodeId, NodeKind,
        compositing::clip::Clip,
        source::text::{TextDraw, TextInstance},
    },
    scope::{FrameTime, Routed, Scope},
};

#[cfg(feature = "ffmpeg")]
use super::media::{GpuMediaFrameImporter, GpuMediaImports};
use super::{
    BoundFrame,
    color::WORKING_COLOR_FORMAT as WORKING,
    renderer::{MediaTextureCache, OutputEncoding, fit_frame_to_rgba8},
};
use retention::Retention;
use state::{RecordState, VideoUpload};
pub(crate) use targets::{quantized_domain, size_class};

mod ingest;
#[cfg(test)]
pub(crate) use ingest::YUV_TO_RGBA_SHADER;
mod profile;
pub(crate) use profile::{NodeTimes, Timed};
mod retention;
mod state;
mod targets;

/// Copies the visible part of a clip's raster into a texture of the parent canvas.
/// The most warnings one composition raises, so that a composition of many nodes that warn
/// doesn't fill the diagnostics.
const MAX_WARNINGS: usize = 32;

const CLIP_CROP_SHADER: &str = "
struct Params {
    offset: vec2<i32>,
    _pad: vec2<i32>,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba16float, write>;

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
pub(crate) const RECORD_STORAGE_TEXTURE: LayoutKind = LayoutKind::StorageTexture(WORKING);
pub(crate) const SPATIAL_LAYOUT: &[LayoutEntry] = &[
    LayoutEntry::compute(0, LayoutKind::Texture),
    LayoutEntry::compute(1, LayoutKind::Uniform),
    LayoutEntry::compute(2, LayoutKind::StorageTexture(WORKING)),
];
pub(crate) fn dispatch_for(size: lumen_gpu::Size) -> lumen_gpu::Dispatch {
    lumen_gpu::Dispatch {
        x: size.width.div_ceil(8),
        y: size.height.div_ceil(8),
        z: 1,
    }
}

const CLIP_CROP: ComputeProgram = ComputeProgram::spatial("clip-crop", CLIP_CROP_SHADER);

/// A compute program a node records with, named once and used by both `RecordNode` methods.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ComputeProgram {
    pub(crate) label: &'static str,
    pub(crate) shader: &'static str,
    pub(crate) entry: &'static str,
    pub(crate) layout: &'static [LayoutEntry],
}

impl ComputeProgram {
    /// A program with the one-input layout `SPATIAL_LAYOUT` and the `cs_main` entry point.
    pub(crate) const fn spatial(label: &'static str, shader: &'static str) -> Self {
        Self {
            label,
            shader,
            entry: "cs_main",
            layout: SPATIAL_LAYOUT,
        }
    }
}

/// A render program without its target, which is only known at a frame.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RenderProgram {
    pub(crate) label: &'static str,
    pub(crate) shader: &'static str,
    pub(crate) vertex: &'static str,
    pub(crate) fragment: &'static str,
    pub(crate) layout: &'static [LayoutEntry],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "SourceBounds", source = crate::node::processing::resize::SHADER)]
pub(crate) struct CropParams {
    pub(crate) origin: [i32; 2],
    pub(crate) size: [u32; 2],
}

/// A recorded node's output.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Raster {
    pub(crate) tex: TexRef,
    pub(crate) domain: TextureDomain,
    /// The raster's logical bounds within its texture, when they are not all of it.
    pub(crate) bounds: Option<CropParams>,
    /// For an image decoded at fewer texels than the device pixels it covers, the size it covers,
    /// which its whole texture spans. Only readers that take their input with
    /// [`FrameCx::input_reduced`] or its siblings are handed such a raster; any other read of one
    /// fails.
    pub(crate) reduced: Option<Size>,
}

impl Raster {
    pub(crate) fn origin(&self) -> [i32; 2] {
        [self.domain.data_rect.x, self.domain.data_rect.y]
    }

    /// An image decoded at `texels` to cover `shown` device pixels, which are no fewer.
    pub(crate) fn image(tex: TexRef, texels: Size, shown: Size) -> Self {
        debug_assert!(texels.width <= shown.width && texels.height <= shown.height);
        Self {
            tex,
            domain: TextureDomain::full_frame(texels),
            bounds: None,
            reduced: (texels != shown).then_some(shown),
        }
    }

    /// The device pixels the raster covers.
    pub(crate) fn shown_size(&self) -> Size {
        self.reduced.unwrap_or(self.domain.display_size())
    }

    /// Texels per device pixel along each axis: 1 unless the raster is reduced.
    pub(crate) fn density(&self) -> [f32; 2] {
        match self.reduced {
            Some(shown) => {
                let texels = self.domain.storage_size;
                [
                    texels.width as f32 / shown.width.max(1) as f32,
                    texels.height as f32 / shown.height.max(1) as f32,
                ]
            }
            None => [1.0; 2],
        }
    }
}

/// What the renderer and the loader ask of a node kind. A rule about one kind of node is a method
/// here that the kind overrides, not a `match` on `NodeKind` elsewhere.
pub(crate) trait RecordNode {
    fn record(&self, cx: &mut FrameCx<'_>) -> crate::Result<Raster>;

    /// Builds every program this node may record with, whatever its params are at a frame.
    fn programs(&self, programs: &mut Programs<'_>) -> crate::Result<()>;

    /// Whether the node renders the same output on every frame as long as its inputs do.
    /// `reads_frame` is whether any of its expressions depends on the frame.
    fn staticness(&self, reads_frame: bool) -> Staticness {
        held(reads_frame)
    }

    /// The shutter angle of a node that blurs its own motion, evaluating its params at either end
    /// of the shutter outside a `motion_blur` that samples.
    fn analytic_shutter(&self) -> Option<&Deferred<f64>> {
        None
    }

    /// How much the node magnifies the raster it reads; infinite when it can't say.
    fn magnification(&self) -> f64 {
        f64::INFINITY
    }

    /// How the node shows a source decoded at fewer texels than its pixels, when it reads every
    /// input with [`FrameCx::input_reduced`] or its siblings; `None` when it can't read one.
    fn reads_reduced(&self) -> Option<crate::gpu::image_size::ReducedRead> {
        None
    }

    /// The params whose literal values are wrong, with what is wrong, for checks that run before
    /// a render.
    fn literal_param_errors(&self) -> Vec<(&'static str, String)> {
        Vec::new()
    }

    /// The families the node names as literals, each with its param path, for checks that run
    /// before a render.
    fn literal_font_families(&self) -> Vec<(String, &str)> {
        Vec::new()
    }
}

/// How static a node is whose output depends on the frame only through its own expressions.
pub(crate) fn held(reads_frame: bool) -> Staticness {
    if reads_frame {
        Staticness::WhileEqual
    } else {
        Staticness::Always
    }
}

/// Builds the programs a composition may record with, so that a shader that doesn't compile
/// fails the job before its first frame.
pub(crate) struct Programs<'a> {
    rec: &'a mut Recorder,
    st: &'a mut RecordState,
    output_format: wgpu::TextureFormat,
}

impl Programs<'_> {
    pub(crate) fn output_format(&self) -> wgpu::TextureFormat {
        self.output_format
    }

    pub(crate) fn limits(&self) -> wgpu::Limits {
        self.rec.device().limits()
    }

    pub(crate) fn compute(&mut self, program: &ComputeProgram) -> crate::Result<()> {
        self.rec
            .compute_program(program.label, program.shader, program.entry, program.layout)
            .map_err(gpu_error)?;
        Ok(())
    }

    /// The program `FrameCx::record_clip` crops a clip's content with.
    pub(crate) fn clip_crop(&mut self) -> crate::Result<()> {
        self.compute(&CLIP_CROP)
    }

    pub(crate) fn render(
        &mut self,
        program: &RenderProgram,
        target: lumen_gpu::RenderTargetSpec,
        variant: u64,
    ) -> crate::Result<()> {
        self.rec
            .render_program(
                program.label,
                program.shader,
                program.vertex,
                program.fragment,
                program.layout,
                target,
                variant,
            )
            .map_err(gpu_error)?;
        Ok(())
    }

    /// Builds a shader from runtime source, which recording it again then finds.
    pub(crate) fn dynamic(
        &mut self,
        node: NodeId,
        label: &str,
        key: ShaderKey<'_>,
        inputs: usize,
        build: impl FnOnce() -> crate::Result<ShaderParts>,
    ) -> crate::Result<CachedShader> {
        let shader = self
            .st
            .shaders
            .cached_shader(self.st.tick, key, inputs, build)?;
        dynamic_program(self.rec, node, label, &shader)?;
        Ok(shader)
    }
}

/// What a runtime shader is cached by.
#[derive(Clone, Copy)]
pub(crate) enum ShaderKey<'a> {
    /// A shader the composition holds, by its id, and its source.
    Shared(u64, &'a Arc<str>),
    /// Source an expression computed.
    Source(&'a str),
}

/// What a shader from runtime source is cached as: its parsed interface, the assembled shader
/// and its layout.
pub(crate) type ShaderParts = (
    Arc<dyn std::any::Any + Send + Sync>,
    String,
    Vec<LayoutEntry>,
);

/// The program of `node`'s shader. The source is the composition's, so a shader the device
/// refuses is the node's failure rather than the device's.
fn dynamic_program(
    rec: &mut Recorder,
    node: NodeId,
    label: &str,
    shader: &CachedShader,
) -> crate::Result<ProgramRef> {
    rec.dynamic_compute_program(
        label,
        shader.id,
        || shader.assembled.to_string(),
        "cs_main",
        &shader.layout,
    )
    .map_err(|error| {
        RenderError::evaluating::<crate::node::processing::wgsl_shader::WgslShader>(
            node,
            error.to_string(),
        )
        .into()
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PathId(u32);

/// A node reached along one path of time remaps and clips, in one graph.
type InstanceKey = (SceneId, PathId, NodeId);
/// A node of one graph, wherever it is reached from.
type GraphNodeKey = (SceneId, NodeId);
/// A node reached along one path, in one sample of the `motion_blur` that path samples, or in
/// sample 0 outside one.
type MemoKey = (InstanceKey, u32);
/// A video frame a `media_in` showed this frame: the node's instance, the identity of the video it
/// read, the source frame, its size and whether its colour is premultiplied.
type VideoFrameKey = (InstanceKey, MediaIdentity, u32, (u32, u32), bool);

/// Whether a node's output is the same on every frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Staticness {
    Never,
    /// Whatever the frame.
    Always,
    /// On the frames where every model it draws holds one pose, which recording finds out.
    WhileHeld,
    /// On the frames where the params reading the frame, its own and those of what it reads, take
    /// the values they took when it was kept, which `frame_values` finds, and where every model
    /// it draws holds one pose.
    WhileEqual,
}

impl Staticness {
    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::Never, _) | (_, Self::Never) => Self::Never,
            (Self::WhileEqual, _) | (_, Self::WhileEqual) => Self::WhileEqual,
            (Self::WhileHeld, _) | (_, Self::WhileHeld) => Self::WhileHeld,
            _ => Self::Always,
        }
    }
}

/// What a recorded shader needs, cached by its source.
#[derive(Clone)]
pub(crate) struct CachedShader {
    pub(crate) id: u64,
    pub(crate) interface: Arc<dyn std::any::Any + Send + Sync>,
    pub(crate) assembled: Arc<str>,
    pub(crate) layout: Arc<[LayoutEntry]>,
}

pub(crate) struct FrameGraph {
    pub(crate) recorder: Recorder,
    state: RecordState,
    /// The values of expressions recording evaluated, kept while they hold.
    held: HeldCache,
    /// What the caches recording fills keep of what frames stop using.
    budgets: super::CacheBudgets,
    /// The target of the next frame recorded, which holds its output until it is submitted.
    external: Option<lumen_gpu::ExternalTexture>,
    #[cfg(feature = "ffmpeg")]
    imports: GpuMediaImports,
}

/// How a frame is recorded, the same for every node of it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RecordSettings {
    pub(crate) output_format: wgpu::TextureFormat,
    pub(crate) output_encoding: OutputEncoding,
    pub(crate) full_frame_sources: bool,
    /// Whether repeats draw from one tile, as `set_single_tile_repeats` sets.
    pub(crate) single_tile_repeats: bool,
    pub(crate) retain_static: bool,
    /// How many times a frame is recorded again while fonts its walk loads change its text.
    pub(crate) max_rebinds: usize,
    pub(crate) max_text_glyphs: usize,
    /// The longest side an image is decoded at, below the device's own limit.
    pub(crate) max_image_side: u32,
    /// Whether to time each node's own recording, which `take_node_timing` returns.
    pub(crate) profile_nodes: bool,
    /// Device pixels per canvas pixel, which every raster is recorded at.
    pub(crate) render_scale: f64,
}

pub(crate) struct RecordInput<'a> {
    pub(crate) composition: &'a Composition,
    pub(crate) frame: f64,
    pub(crate) media: Option<&'a dyn MediaStore>,
    pub(crate) settings: RecordSettings,
    pub(crate) media_cache: &'a mut MediaTextureCache,
    pub(crate) text_atlas: &'a Arc<Mutex<lumen_text::GlyphAtlas>>,
    pub(crate) atlas_texture: &'a mut Option<Arc<wgpu::Texture>>,
    /// The renderer whose device GPU video frames were decoded on.
    #[cfg(feature = "ffmpeg")]
    pub(crate) renderer: &'a lumen_gpu::Renderer,
    #[cfg(feature = "ffmpeg")]
    pub(crate) importer: &'a mut GpuMediaFrameImporter,
}

impl FrameGraph {
    pub(crate) fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self {
            recorder: Recorder::new(device, queue),
            state: RecordState::default(),
            held: HeldCache::default(),
            budgets: super::CacheBudgets::default(),
            external: None,
            #[cfg(feature = "ffmpeg")]
            imports: GpuMediaImports::default(),
        }
    }

    /// Records the frame, returning what it wrote for later upload.
    pub(crate) fn record(&mut self, input: RecordInput<'_>) -> crate::Result<BoundFrame> {
        let RecordInput {
            composition,
            frame,
            media,
            settings,
            media_cache,
            text_atlas,
            atlas_texture,
            #[cfg(feature = "ffmpeg")]
            renderer,
            #[cfg(feature = "ffmpeg")]
            importer,
        } = input;
        let fingerprint = composition.fingerprint();
        if self.state.fingerprint != Some(fingerprint) {
            self.reset();
            self.state.fingerprint = Some(fingerprint);
        }
        if settings.profile_nodes {
            self.state.profile.begin_frame();
        }
        let mut rebinds = 0;
        loop {
            self.drop_pending();
            let generation = font_generation(media);
            if self.state.font_generation != generation
                || self.state.retained.sources_changed(media)
            {
                self.state.retained.drop_all(&mut self.recorder);
                // Values measured with the fonts before are evaluated again.
                self.held.clear();
                self.state.font_generation = generation;
            }
            self.state.begin_frame();
            self.held.begin_frame();
            self.recorder.begin_frame();
            let mut cx = FrameCx {
                rec: &mut self.recorder,
                st: &mut self.state,
                held: settings.retain_static.then_some(&self.held),
                media_cache: &mut *media_cache,
                composition,
                scope: Scope::root(composition),
                frame: frame.into(),
                path: PathId(0),
                sample: 0,
                sampling: false,
                media,
                settings,
                retention: Retention::default(),
                depth: 0,
                bound: BoundFrame::new(),
                text_atlas,
                atlas_texture: &mut *atlas_texture,
                #[cfg(feature = "ffmpeg")]
                renderer,
                #[cfg(feature = "ffmpeg")]
                importer: &mut *importer,
                #[cfg(feature = "ffmpeg")]
                imports: &mut self.imports,
            };
            let output = cx.scope.output;
            let raster = cx.input(output);
            let bound = cx.bound;
            // A font loaded during the walk may have changed what it measured.
            if font_generation(media) == generation {
                let raster = raster?;
                if bound.text_dropped_glyphs {
                    self.drop_pending();
                }
                self.resolve(raster)?;
                self.state.trim(&mut self.recorder, &self.budgets);
                return Ok(bound);
            }
            if rebinds == settings.max_rebinds {
                return Err(internal(&format!(
                    "fonts kept loading while frame {frame} was recorded; its text bounds could not settle"
                )));
            }
            rebinds += 1;
        }
    }

    /// Gives the frame's textures pooled ones, with the frame's output in its target if it has
    /// one, and points what outlives the walk at them.
    fn resolve(&mut self, raster: Raster) -> crate::Result<()> {
        let external = self.external.as_ref().map(|external| {
            let texture = external.texture_arc();
            let recorder = &mut self.recorder;
            *self
                .state
                .externals
                .entry(Arc::as_ptr(&texture) as usize)
                .or_insert_with(|| recorder.adopt(texture))
        });
        let tex = self
            .recorder
            .resolve(raster.tex, external)
            .map_err(gpu_error)?;
        self.state.output = Some(Raster { tex, ..raster });
        for key in &self.state.retained.pending {
            let slot = self
                .state
                .retained
                .slots
                .get_mut(key)
                .expect("pending slot");
            slot.raster.tex = self.recorder.physical(slot.raster.tex);
            if let Some(full) = &mut slot.full {
                *full = self.recorder.physical(*full);
            }
        }
        Ok(())
    }

    /// Drops what a frame that failed recorded, and lets go of the images and target it bound so
    /// the media cache can free them.
    pub(crate) fn abandon_frame(&mut self) {
        self.drop_pending();
        self.recorder.begin_frame();
        self.state.media.video_uploads.clear();
        self.state.media.yuv_uploads.clear();
        self.state.output = None;
        let tick = self.state.tick;
        self.state
            .media
            .forget_textures(&mut self.recorder, |last_used| last_used == tick);
        if let Some(external) = &self.external {
            let key = Arc::as_ptr(&external.texture_arc()) as usize;
            if let Some(tex) = self.state.externals.remove(&key) {
                self.recorder.forget(tex);
            }
        }
    }

    /// Builds every program a frame of `composition` may record with, following the output through
    /// its inputs and the templates of its clips.
    pub(crate) fn prepare(
        &mut self,
        composition: &Composition,
        output_format: wgpu::TextureFormat,
    ) -> crate::Result<()> {
        let fingerprint = composition.fingerprint();
        if self.state.fingerprint != Some(fingerprint) {
            self.reset();
            self.state.fingerprint = Some(fingerprint);
        }
        // What an earlier compile built stays cached, and no frame in hand still needs it pinned.
        self.recorder.begin_frame();
        let mut programs = Programs {
            rec: &mut self.recorder,
            st: &mut self.state,
            output_format,
        };
        let mut pending = vec![(
            SceneId::Root,
            None,
            composition.graph(),
            composition.output(),
        )];
        let mut seen = HashSet::default();
        while let Some((scene, template, graph, id)) = pending.pop() {
            if !seen.insert((scene, id)) {
                continue;
            }
            let Some(node) = graph.nodes.get(&id) else {
                continue;
            };
            node.programs(&mut programs)
                .map_err(|error| in_scene(error, template))?;
            match node {
                NodeKind::Clip(clip) if clip.template_name().is_some() => {
                    let name = clip.template_name();
                    if let Some(template) = name.and_then(|name| composition.template(name)) {
                        pending.push((
                            template.id,
                            name,
                            &template.template.scene.graph,
                            template.output,
                        ));
                    }
                }
                NodeKind::Switch(switch) => {
                    let layers = match &switch.params.selected_layer {
                        Deferred::Value(layer) => usize::try_from(*layer)
                            .ok()
                            .and_then(|layer| switch.layers.get(layer..=layer))
                            .unwrap_or_default(),
                        Deferred::Expr(_) => &switch.layers,
                    };
                    pending.extend(layers.iter().map(|&layer| (scene, template, graph, layer)));
                }
                _ => pending.extend(
                    node.input_ports()
                        .into_iter()
                        .flatten()
                        .map(|input| (scene, template, graph, input)),
                ),
            }
        }
        Ok(())
    }

    /// Makes the next frame recorded render into `external`.
    pub(crate) fn set_external(&mut self, external: lumen_gpu::ExternalTexture) {
        self.external = Some(external);
    }

    /// Takes the target the frame was recorded for.
    pub(crate) fn take_external(&mut self) -> Option<lumen_gpu::ExternalTexture> {
        self.external.take()
    }

    /// Writes what the frame recorded to the GPU.
    pub(crate) fn upload(
        &mut self,
        bound: &BoundFrame,
        atlas_texture: Option<&wgpu::Texture>,
    ) -> crate::Result<()> {
        for VideoUpload { tex, frame, size } in self.state.media.video_uploads.drain(..) {
            let rgba = fit_frame_to_rgba8(&frame, size.width, size.height);
            self.recorder.queue().write_texture(
                self.recorder.texture(tex).as_image_copy(),
                &rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size.width * 4),
                    rows_per_image: Some(size.height),
                },
                size.as_extent(),
            );
        }
        for upload in self.state.media.yuv_uploads.drain(..) {
            let shape = upload.shape;
            self.recorder.queue().write_texture(
                self.recorder.texture(upload.tex).as_image_copy(),
                &upload.frame.data,
                wgpu::TexelCopyBufferLayout {
                    offset: upload.offset as u64,
                    bytes_per_row: Some(upload.stride),
                    rows_per_image: Some(shape.height),
                },
                Size::new(shape.width, shape.height).as_extent(),
            );
        }
        if let Some(atlas) = atlas_texture {
            bound.write_atlas_regions(self.recorder.queue(), atlas);
        }
        bound.mark_uploaded()?;
        self.recorder.upload().map_err(gpu_error)?;
        self.forget_unbound_media();
        Ok(())
    }

    /// The media cache frees what only it holds, so images this frame doesn't bind are let go.
    fn forget_unbound_media(&mut self) {
        let tick = self.state.tick;
        self.state
            .media
            .forget_textures(&mut self.recorder, |last_used| last_used != tick);
    }

    /// Commits what the frame retained after submitting its passes, and sweeps what idled.
    pub(crate) fn finish_frame(&mut self) {
        self.state.retained.pending.clear();
        self.state.mips.commit();
        self.state.retained.evict(
            &mut self.recorder,
            self.state.tick,
            self.budgets.retained_outputs,
        );
        self.recorder.end_frame();
        let recorder = &mut self.recorder;
        // Nothing else holding one means nothing will bind it again. A submission still using it
        // holds it through wgpu.
        let mut unheld = |tex: &mut TexRef| {
            let kept = Arc::strong_count(recorder.texture(*tex)) > 1;
            if !kept {
                recorder.forget(*tex);
            }
            kept
        };
        self.state.media.textures.retain(|_, (tex, ..)| unheld(tex));
        self.state.externals.retain(|_, tex| unheld(tex));
        #[cfg(feature = "ffmpeg")]
        self.state
            .media
            .evict_imports(self.state.tick, &mut self.imports);
    }

    pub(crate) fn output(&self) -> Option<Raster> {
        self.state.output
    }

    #[cfg(feature = "test-hooks")]
    pub(crate) fn shader_source_lookups(&self) -> u64 {
        self.state.shaders.source_lookups
    }

    /// The domain `node` of the root graph had in the frame last recorded.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn node_domain(
        &self,
        _composition: &Composition,
        node: NodeId,
    ) -> Option<TextureDomain> {
        let graph = SceneId::Root;
        let recorded = self.state.memo.iter().find_map(|((key, _), raster)| {
            (key.0 == graph && key.2 == node).then_some(raster.domain)
        });
        recorded.or_else(|| Some(self.state.retained.slots.get(&(graph, node))?.raster.domain))
    }

    pub(crate) fn output_desc(&self) -> Option<TextureDesc> {
        let raster = self.state.output?;
        let texture = self.recorder.texture(raster.tex);
        Some(TextureDesc {
            domain: raster.domain,
            format: texture.format(),
            usage: texture.usage(),
        })
    }

    pub(crate) fn reset(&mut self) {
        self.state.retained.drop_all(&mut self.recorder);
        // A caller may hold the last frame of what is being replaced, so nothing writes it again.
        if let Some(output) = self.state.output {
            self.recorder.disown(output.tex);
        }
        #[cfg(feature = "ffmpeg")]
        {
            self.imports = GpuMediaImports::default();
        }
        self.state.reset(&mut self.recorder);
        self.held.clear();
    }

    /// Frees everything recording holds, as `reset` alone keeps its pool and programs for the next
    /// composition.
    pub(crate) fn release(&mut self) {
        self.reset();
        self.drop_pending();
        self.state.release(&mut self.recorder);
        self.recorder.release_all();
    }

    /// The warnings recording has raised since the last call.
    pub(crate) fn take_diagnostics(&mut self) -> Vec<crate::diagnostic::Diagnostic> {
        std::mem::take(&mut self.state.diagnostics)
    }

    /// The time nodes have spent recording since the last call, slowest first.
    pub(crate) fn take_node_timing(&mut self) -> Vec<crate::gpu::types::NodeTiming> {
        self.state.profile.take()
    }

    /// Sets what each cache recording fills keeps, which takes effect as frames trim them.
    pub(crate) fn set_budgets(&mut self, budgets: super::CacheBudgets) {
        self.budgets = budgets;
        self.recorder.set_pool_budget(budgets.pooled_textures);
    }

    /// The bytes the caches recording fills hold.
    pub(crate) fn cache_stats(&self) -> super::CacheStats {
        #[cfg(feature = "3d")]
        let (models, environments, text_meshes) = self.state.scene.cache.held_bytes();
        #[cfg(not(feature = "3d"))]
        let (models, environments, text_meshes) = (0, 0, 0);
        super::CacheStats {
            pooled_textures: self.recorder.pool_bytes(),
            retained_outputs: self.state.retained.bytes,
            luts: self.state.luts.bytes(),
            mip_chains: self.state.mips.bytes(),
            models,
            environments,
            text_meshes,
            ..super::CacheStats::default()
        }
    }

    pub(crate) fn skipped_passes(&self) -> u64 {
        self.state.retained.skipped_passes
    }

    fn drop_pending(&mut self) {
        self.state.mips.discard_pending();
        self.state.retained.drop_pending(&mut self.recorder);
    }

    pub(crate) fn retained_bytes(&self) -> u64 {
        self.state.retained.bytes
    }
}

fn font_generation(media: Option<&dyn MediaStore>) -> Option<u64> {
    crate::node::source::text_fonts::text_system(media)
        .ok()
        .map(|text_system| text_system.font_generation())
}

/// The context expressions of `scope`, reached along `path`, are evaluated in at `frame`.
fn held_context<'s>(
    scope: &'s Scope<'_>,
    path: PathId,
    held: Option<&'s HeldCache>,
    frame: FrameTime,
    media: Option<&'s dyn MediaStore>,
) -> ExpressionContext<'s> {
    ExpressionContext {
        held: held.map(|held| held.scope(scope.id, path.0, scope.props.as_ref())),
        ..scope.expr_context(frame, media)
    }
}

/// One walk of the graph for one frame.
pub(crate) struct FrameCx<'a> {
    pub(crate) rec: &'a mut Recorder,
    st: &'a mut RecordState,
    /// Where expressions keep their values while they hold, unless outputs are recorded anew on
    /// every frame.
    held: Option<&'a HeldCache>,
    media_cache: &'a mut MediaTextureCache,
    composition: &'a Composition,
    scope: Scope<'a>,
    frame: FrameTime,
    path: PathId,
    /// The sample of the `motion_blur` that `path` samples, or 0 outside one.
    sample: u32,
    /// Whether the walk is inside a `motion_blur` that samples, where analytic motion blur is off.
    sampling: bool,
    media: Option<&'a dyn MediaStore>,
    settings: RecordSettings,
    retention: Retention,
    /// How many nodes the walk is recording inside.
    depth: u32,
    pub(crate) bound: BoundFrame,
    text_atlas: &'a Arc<Mutex<lumen_text::GlyphAtlas>>,
    atlas_texture: &'a mut Option<Arc<wgpu::Texture>>,
    #[cfg(feature = "ffmpeg")]
    renderer: &'a lumen_gpu::Renderer,
    #[cfg(feature = "ffmpeg")]
    importer: &'a mut GpuMediaFrameImporter,
    #[cfg(feature = "ffmpeg")]
    imports: &'a mut GpuMediaImports,
}

/// The error for a failure of the recorder: `render.limit` when the frame's data doesn't fit the
/// device, `render.internal` when the recorder was misused, and `render.gpu` otherwise.
pub(crate) fn gpu_error(error: lumen_gpu::FrameError) -> crate::error::LumenError {
    let details = error.to_string();
    match error {
        lumen_gpu::FrameError::TooLarge(_) => RenderError::Limit { details },
        lumen_gpu::FrameError::Misuse(_) => RenderError::Internal { details },
        lumen_gpu::FrameError::Device(_) => RenderError::Gpu { details },
    }
    .into()
}

fn internal(details: &str) -> crate::error::LumenError {
    RenderError::Internal {
        details: details.to_string(),
    }
    .into()
}

impl<'a> FrameCx<'a> {
    /// Runs `run`, which evaluates `node`, and with node profiling on, counts the time it takes
    /// less that of the nodes it evaluates inside it as `node`'s own.
    pub(crate) fn time_node<T>(&mut self, node: &NodeKind, run: impl FnOnce(&mut Self) -> T) -> T {
        if !self.settings.profile_nodes {
            return run(self);
        }
        let timer = self.st.profile.start(self.scope.id);
        let result = run(self);
        self.st.profile.finish(timer, node, self.composition);
        result
    }

    /// Starts timers for nodes of the scene being recorded, for code that can't run them through
    /// [`Self::time_node`].
    pub(crate) fn node_times(&self) -> NodeTimes<'_> {
        NodeTimes::new(
            self.settings.profile_nodes,
            &self.st.profile,
            self.scope.id,
            self.composition,
        )
    }

    pub(crate) fn expr(&self) -> ExpressionContext<'_> {
        self.expr_at(self.frame)
    }

    /// The context expressions of this scope instance are evaluated in at `frame`.
    fn expr_at(&self, frame: FrameTime) -> ExpressionContext<'_> {
        held_context(&self.scope, self.path, self.held, frame, self.media)
    }

    /// Evaluates `text` at this frame, placing its glyphs in the atlas and writing what it draws
    /// to the frame's buffers.
    pub(crate) fn bind_text(&mut self, text: &TextInstance) -> crate::Result<TextDraw> {
        let expr = held_context(&self.scope, self.path, self.held, self.frame, self.media);
        let mut fallbacks = Vec::new();
        let draw = text.bind(
            expr,
            self.media,
            &mut self.bound,
            self.rec,
            &mut fallbacks,
            self.settings.max_text_glyphs,
        )?;
        for family in fallbacks {
            self.warn(text.node_id, WarningKind::FontFallback { family });
        }
        Ok(draw)
    }

    /// Warns, once per composition, node and what `kind` is about, that node `node_id` renders
    /// but not as the composition asks. A composition raises at most [`MAX_WARNINGS`].
    pub(crate) fn warn(&mut self, node_id: NodeId, kind: WarningKind) {
        let warning = Warning {
            at: self.scope.at(node_id),
            kind,
        };
        let key = (
            warning.at.clone(),
            std::mem::discriminant(&warning.kind),
            warning.kind.identity().map(str::to_string),
        );
        if self.st.warned.len() >= MAX_WARNINGS || !self.st.warned.insert(key) {
            return;
        }
        tracing::warn!(target: "lumen_render", code = warning.code(), "{warning}");
        let graph = &self.scope.scene.graph;
        let path =
            crate::diagnostic::pointer(&warning.at, "", |id| Some((graph.position(id)?, None)));
        self.st
            .diagnostics
            .push(crate::diagnostic::Diagnostic::from_warning(&warning, path));
    }

    /// The most glyphs a text node may draw.
    #[cfg(feature = "3d")]
    pub(crate) fn max_text_glyphs(&self) -> usize {
        self.settings.max_text_glyphs
    }

    pub(crate) fn media(&self) -> Option<&'a dyn MediaStore> {
        self.media
    }

    pub(crate) fn composition(&self) -> &'a Composition {
        self.composition
    }

    pub(crate) fn output_format(&self) -> wgpu::TextureFormat {
        self.settings.output_format
    }

    pub(crate) fn output_encoding(&self) -> OutputEncoding {
        self.settings.output_encoding
    }

    /// Whether repeats draw from one tile, as `set_single_tile_repeats` sets.
    pub(crate) fn single_tile_repeats(&self) -> bool {
        self.settings.single_tile_repeats
    }

    pub(crate) fn frame_number(&self) -> u32 {
        self.frame.value() as u32
    }

    pub(crate) fn frame_time(&self) -> FrameTime {
        self.frame
    }

    pub(crate) fn node(&self, id: NodeId) -> Option<&'a NodeKind> {
        self.scope.node(id)
    }

    /// The scene being recorded, the frame count `tick` gives, the paths `render_2d` keeps, and
    /// timers for its nodes.
    pub(crate) fn drawing_cache(
        &mut self,
    ) -> (
        crate::composition::SceneId,
        u64,
        &mut crate::node::vector::drawing::DrawingCache,
        NodeTimes<'_>,
    ) {
        let times = NodeTimes::new(
            self.settings.profile_nodes,
            &self.st.profile,
            self.scope.id,
            self.composition,
        );
        (self.scope.id, self.st.tick, &mut self.st.drawings, times)
    }

    /// Counts frames recorded, for caches that keep what recent frames used.
    pub(crate) fn tick(&self) -> u64 {
        self.st.tick
    }

    pub(crate) fn lut_cache(
        &mut self,
    ) -> (
        &mut Recorder,
        &mut crate::node::processing::color_grade::LutCache,
    ) {
        (&mut *self.rec, &mut self.st.luts)
    }

    pub(crate) fn mip_cache(&mut self) -> (&mut Recorder, &mut crate::gpu::mips::MipCache) {
        (&mut *self.rec, &mut self.st.mips)
    }

    #[cfg(feature = "3d")]
    pub(crate) fn scene_cache(
        &mut self,
    ) -> (&mut Recorder, &mut crate::node::scene::collect::SceneCache) {
        (&mut *self.rec, &mut self.st.scene.cache)
    }

    /// The placements kept for the copies of `instance_3d` node `node`, if they were placed for
    /// `count` copies.
    #[cfg(feature = "3d")]
    pub(crate) fn kept_placements(
        &mut self,
        node: NodeId,
        count: u32,
    ) -> Option<Arc<crate::node::scene::collect::Placements>> {
        let key = (self.scope.id, node);
        let kept = self.st.scene.placements.get_mut(&key)?;
        if kept.count != count {
            return None;
        }
        kept.last_used = self.st.tick;
        Some(kept.placements.clone())
    }

    /// Keeps `placements`, those of the `count` copies of `instance_3d` node `node`, for later
    /// frames.
    #[cfg(feature = "3d")]
    pub(crate) fn keep_placements(
        &mut self,
        node: NodeId,
        count: u32,
        placements: Arc<crate::node::scene::collect::Placements>,
    ) {
        let key = (self.scope.id, node);
        self.st.scene.placements.insert(
            key,
            state::KeptPlacements {
                count,
                placements,
                last_used: self.st.tick,
            },
        );
    }

    /// The placements of `outer`'s copies of the copies `inner` places in `range`, its first and
    /// count, placed by `between` within each outer copy, as `instance_3d` node `node`'s list
    /// `list`. They are made by `flatten` unless an earlier collection made them from the same
    /// three, which the second value says.
    #[cfg(feature = "3d")]
    pub(crate) fn flattened(
        &mut self,
        (node, list): (NodeId, u32),
        outer: &Arc<crate::node::scene::collect::Placements>,
        (between, inner, range): (
            lumen_3d::glam::Mat4,
            &Arc<crate::node::scene::collect::Placements>,
            (u32, u32),
        ),
        flatten: impl FnOnce() -> crate::node::scene::collect::Placements,
    ) -> (Arc<crate::node::scene::collect::Placements>, bool) {
        let key = ((self.scope.id, node), list);
        let tick = self.st.tick;
        if let Some(kept) = self.st.scene.flattened.get_mut(&key)
            && Arc::ptr_eq(&kept.outer, outer)
            && Arc::ptr_eq(&kept.inner, inner)
            && kept.between == between
            && kept.range == range
        {
            kept.last_used = tick;
            return (kept.placements.clone(), true);
        }
        let placements = Arc::new(flatten());
        self.st.scene.flattened.insert(
            key,
            state::KeptFlattened {
                outer: outer.clone(),
                between,
                inner: inner.clone(),
                range,
                placements: placements.clone(),
                last_used: tick,
            },
        );
        (placements, false)
    }

    /// The instance buffer kept for `instance_3d` node `node`'s list `list`, holding `placements`:
    /// the one an earlier frame uploaded for them, or a new one holding `records`, which replaces
    /// it. `None` when the buffer this frame already drew holds other placements.
    #[cfg(feature = "3d")]
    pub(crate) fn copy_buffer<R: bytemuck::Pod>(
        &mut self,
        (node, list): (NodeId, u32),
        placements: &Arc<crate::node::scene::collect::Placements>,
        records: impl FnOnce() -> Vec<R>,
    ) -> Option<lumen_gpu::BufRef> {
        let key = ((self.scope.id, node), list);
        let tick = self.st.tick;
        let buffers = &mut self.st.scene.buffers;
        if let Some(kept) = buffers.get_mut(&key) {
            if Arc::ptr_eq(&kept.placements, placements) {
                kept.last_used = tick;
                return Some(kept.buffer);
            }
            if kept.last_used == tick {
                return None;
            }
        }
        let buffer = crate::node::scene::collect::storage_buffer(
            self.rec,
            "lumen 3d instance buffer",
            bytemuck::cast_slice(&records()),
        );
        let replaced = buffers.insert(
            key,
            state::KeptBuffer {
                placements: placements.clone(),
                buffer,
                last_used: tick,
            },
        );
        if let Some(replaced) = replaced {
            self.rec.forget_buffer(replaced.buffer);
        }
        Some(buffer)
    }

    /// The graph being recorded, the frame count `tick` gives, and the caches `particles_3d`
    /// nodes read: the scene cache, for the meshes their surfaces draw, and their own.
    #[cfg(feature = "3d")]
    pub(crate) fn particle_cache(
        &mut self,
    ) -> (
        crate::composition::SceneId,
        u64,
        &mut Recorder,
        &mut crate::node::scene::collect::SceneCache,
        &mut crate::node::scene::collect::ParticleCache,
    ) {
        let scene = &mut self.st.scene;
        (
            self.scope.id,
            self.st.tick,
            &mut *self.rec,
            &mut scene.cache,
            &mut scene.particles,
        )
    }

    /// The canvas of the scope being recorded, in device pixels.
    pub(crate) fn canvas_size(&self) -> Size {
        self.device_size(self.logical_canvas_size())
    }

    /// The canvas of the scope being recorded, in canvas pixels, as expressions see it.
    pub(crate) fn logical_canvas_size(&self) -> Size {
        let settings = &self.scope.scene.render_settings;
        Size::new(settings.width.max(1), settings.height.max(1))
    }

    /// Device pixels per canvas pixel.
    pub(crate) fn pixel_scale(&self) -> f64 {
        self.settings.render_scale
    }

    /// The device pixels a `logical` size of canvas pixels covers, each side rounded up.
    pub(crate) fn device_size(&self, logical: Size) -> Size {
        crate::gpu::render_scale::device_size(logical.width, logical.height, self.pixel_scale())
    }

    /// A node's explicit `width` or `height` in device pixels, 0 still meaning the canvas's.
    pub(crate) fn explicit_extent(&self, logical: u32) -> u32 {
        crate::gpu::render_scale::explicit_extent(logical, self.pixel_scale())
    }

    /// `node_id` stored at its full display size, where a clip keeps the canvas its content is
    /// drawn on rather than being cropped to this one, for readers that place it anywhere, such
    /// as 3D. Content that draws less than its canvas is padded with transparency to it, and
    /// content that draws more is cropped.
    #[cfg(feature = "3d")]
    pub(crate) fn input_native(&mut self, node_id: NodeId) -> crate::Result<Raster> {
        match self.clip_native(node_id)? {
            Some(raster) => Ok(raster),
            None => self.input_full(node_id),
        }
    }

    /// [`Self::input_native`] for a reader that shows a reduced raster at its density.
    #[cfg(feature = "3d")]
    pub(crate) fn input_native_reduced(&mut self, node_id: NodeId) -> crate::Result<Raster> {
        match self.clip_native(node_id)? {
            Some(raster) => Ok(raster),
            None => self.input_full_reduced(node_id),
        }
    }

    /// What clip `node_id` shows at its full display size, or `None` when it is not a clip.
    #[cfg(feature = "3d")]
    fn clip_native(&mut self, node_id: NodeId) -> crate::Result<Option<Raster>> {
        if let Some(NodeKind::Clip(clip)) = self.scope.node(node_id) {
            let canvas = self.device_size(clip.content_canvas(&self.scope)?);
            let max = self.rec.device().limits().max_texture_dimension_2d;
            if canvas.width > max || canvas.height > max {
                return Err(
                    RenderError::evaluating::<crate::node::compositing::clip::Clip>(
                        node_id,
                        format!(
                            "a {}x{} canvas is larger than this device's {max}-pixel texture limit",
                            canvas.width, canvas.height
                        ),
                    )
                    .into(),
                );
            }
            return Ok(Some(
                match clip.route(&self.scope, self.frame, self.media)? {
                    Some(routed) => {
                        let raster = self.place_routed(routed, canvas)?;
                        self.full_frame(raster)
                    }
                    None => self.transparent(canvas),
                },
            ));
        }
        Ok(None)
    }

    /// The shared text atlas, begun for this frame.
    pub(crate) fn atlas(&mut self) -> crate::Result<TexRef> {
        if self.bound.text_atlas.is_none() {
            self.bound.text_atlas_frame = self
                .text_atlas
                .lock()
                .map_err(|_| internal("text atlas lock was poisoned"))?
                .begin_frame();
            self.bound.text_atlas = Some(Arc::clone(self.text_atlas));
        }
        if let Some(atlas) = self.st.text.atlas {
            return Ok(atlas);
        }
        let texture = match self.atlas_texture {
            Some(texture) => Arc::clone(texture),
            None => {
                let texture = Arc::new(super::types::create_text_atlas_texture(self.rec.device()));
                *self.atlas_texture = Some(Arc::clone(&texture));
                texture
            }
        };
        let atlas = self.rec.adopt(texture);
        self.st.text.atlas = Some(atlas);
        Ok(atlas)
    }

    /// The text instance's state across frames, built once.
    pub(crate) fn text_instance(
        &mut self,
        node_id: NodeId,
        build: impl FnOnce(&Self) -> crate::Result<TextInstance>,
    ) -> crate::Result<Arc<TextInstance>> {
        let key = (self.scope.id, self.path, node_id);
        if let Some(text) = self.st.text.instances.get(&key) {
            return Ok(Arc::clone(text));
        }
        let text = Arc::new(build(self)?);
        self.st.text.instances.insert(key, Arc::clone(&text));
        Ok(text)
    }

    /// Whether analytic motion blur applies. It is off inside a `motion_blur` that samples, whose
    /// samples already spread motion over the shutter.
    pub(crate) fn analytic_motion_blur(&self) -> bool {
        !self.sampling
    }

    /// Runs `record` over the samples of `motion_blur` node `node_id`, on a path of their own and
    /// with analytic motion blur off, and returns to this frame after.
    pub(crate) fn record_samples(
        &mut self,
        node_id: NodeId,
        record: impl FnOnce(&mut Self) -> crate::Result<Raster>,
    ) -> crate::Result<Raster> {
        let (frame, path) = (self.frame, self.path);
        let next = PathId(self.st.paths.len() as u32 + 1);
        self.path = *self.st.paths.entry((path, node_id)).or_insert(next);
        self.sampling = true;
        let result = record(self);
        self.sampling = false;
        self.sample = 0;
        self.path = path;
        self.frame = frame;
        result
    }

    /// Records `node_id` as sample `index` of the `motion_blur` being recorded, at `frame`.
    pub(crate) fn input_sample(
        &mut self,
        node_id: NodeId,
        index: u32,
        frame: FrameTime,
    ) -> crate::Result<Raster> {
        self.sample = index;
        self.frame = frame;
        self.input(node_id)
    }

    fn record_routed(&mut self, routed: Routed<'a>) -> crate::Result<Raster> {
        let frame = std::mem::replace(&mut self.frame, routed.frame);
        let scope = std::mem::replace(&mut self.scope, routed.scope);
        let path = self.path;
        if let Some(instance) = routed.instance {
            let next = PathId(self.st.paths.len() as u32 + 1);
            self.path = *self.st.paths.entry((path, instance)).or_insert(next);
        }
        let result = self.input(routed.node);
        self.path = path;
        self.scope = scope;
        self.frame = frame;
        result
    }

    pub(crate) fn record_time_remap(
        &mut self,
        node: &crate::node::processing::time_remap::TimeRemap,
    ) -> crate::Result<Raster> {
        let routed = crate::scope::route_time_remap(&self.scope, node, self.frame, self.media)?;
        self.record_routed(routed)
    }

    /// What a clip shows at this frame, cropped to its content's canvas and placed on this
    /// composition's, or a transparent raster while it is hidden.
    pub(crate) fn record_clip(&mut self, clip: &Clip) -> crate::Result<Raster> {
        let routed = clip.route(&self.scope, self.frame, self.media)?;
        let Some(routed) = routed else {
            return Ok(self.transparent(self.canvas_size()));
        };
        self.place_routed(routed, self.canvas_size())
    }

    /// What a clip's content shows, cropped to its canvas and the `canvas` it is placed on.
    fn place_routed(&mut self, routed: Routed<'a>, canvas: Size) -> crate::Result<Raster> {
        let settings = &routed.scope.scene.render_settings;
        let content = self.device_size(Size::new(settings.width, settings.height));
        let raster = self.record_routed(routed)?;
        let canvas_rect = lumen_gpu::RectI::from_size(canvas);
        let data = raster.domain.data_rect;
        let visible = canvas_rect
            .intersection(lumen_gpu::RectI::from_size(content))
            .intersection(data);
        if visible.is_empty() {
            return Ok(self.transparent(canvas));
        }
        if visible == data {
            return Ok(if raster.domain.display_size() == canvas {
                raster
            } else {
                Raster {
                    domain: TextureDomain {
                        storage_size: raster.domain.storage_size,
                        display_rect: canvas_rect,
                        data_rect: data,
                    },
                    ..raster
                }
            });
        }
        let domain = TextureDomain::bounded(canvas, visible);
        let size = domain.storage_size;
        let tex = self.storage_target(size);
        let offset = [
            domain.data_rect.x - data.x,
            domain.data_rect.y - data.y,
            0,
            0,
        ];
        self.spatial_pass(&CLIP_CROP, raster.tex, &offset, tex, size)?;
        Ok(Raster {
            tex,
            domain,
            bounds: None,
            reduced: None,
        })
    }
}

/// Fails, blaming `node`, when `size` is larger than the device's `max` texture dimension.
pub(crate) fn ensure_texture_fits(
    node: &NodeKind,
    size: Size,
    canvas: Size,
    max: u32,
) -> crate::Result<()> {
    if size.width <= max && size.height <= max {
        return Ok(());
    }
    let what = if size == canvas { "canvas" } else { "texture" };
    Err(RenderError::evaluating_node(
        node,
        format!(
            "a {}x{} {what} is larger than this device's {max}-pixel texture limit",
            size.width, size.height
        ),
    )
    .into())
}

/// Records `node`, or fails when the textures it acquires itself are larger than the device
/// allows. The textures its inputs acquire are theirs to check. A failure names the template
/// `node` is in.
fn record_node(node: &NodeKind, cx: &mut FrameCx<'_>) -> crate::Result<Raster> {
    let outer = cx.rec.replace_largest_acquired_texture(Size::new(0, 0));
    let raster = node
        .record(cx)
        .map_err(|error| locate_media_failure(node, &cx.scope, error));
    let largest = cx.rec.replace_largest_acquired_texture(outer);
    let raster = raster.and_then(|raster| {
        let max = cx.rec.device().limits().max_texture_dimension_2d;
        ensure_texture_fits(node, largest, cx.canvas_size(), max).map(|()| raster)
    });
    raster.map_err(|error| in_scene(error, cx.scope.template_name()))
}

/// `error` placed in `template`, or left in the root for `None`.
fn in_scene(error: LumenError, template: Option<&str>) -> LumenError {
    match template {
        Some(name) => error.in_template(name),
        None => error,
    }
}

/// Says where `node` names the source `error` failed on, when `error` is a media failure that
/// doesn't say yet and the node names the source in one param.
pub(crate) fn locate_media_failure(
    node: &NodeKind,
    scope: &Scope<'_>,
    error: LumenError,
) -> LumenError {
    let Some(failure) = error.media_failure().filter(|failure| failure.at.is_none()) else {
        return error;
    };
    let (mut named, mut params) = (None, Vec::new());
    node.for_each_media_source(&mut |media| {
        if media.value == MediaValue::Literal(&failure.source) && named.is_none() {
            named = Some((media.param.to_string(), media.kinds.clone()));
        }
        params.push((media.param, media.kinds));
    });
    if named.is_none() {
        named = node
            .literal_font_families()
            .into_iter()
            .find(|(_, family)| *family == failure.source)
            .map(|(param, _)| (param, vec![crate::media::AssetKind::Font]));
    }
    let Some((param, kinds)) = named.or_else(|| match params.as_slice() {
        [(only, kinds)] => Some(((*only).to_string(), kinds.clone())),
        _ => None,
    }) else {
        return error;
    };
    let at = crate::media::NodeParamAt {
        template: scope.template_name().map(str::to_string),
        node: node.id(),
        param,
    };
    error.media_used_as(&kinds, || crate::media::MediaLocation::Node(at))
}
