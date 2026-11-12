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

use foldhash::HashSet;
use lumen_gpu::{
    Dispatch, LayoutEntry, LayoutKind, ProgramRef, Recorder, Res, SamplerRef, Size, TexRef,
    TextureDesc, TextureDomain, wgpu,
};

use crate::{
    composition::Composition,
    error::{LumenError, RenderError},
    expr::{ExpressionContext, ExpressionValue},
    media::{AssetKind, MediaIdentity, MediaStore},
    node::{
        Deferred, DelegateEvaluable, MediaValue, Node, NodeId, NodeKind,
        compositing::clip::Clip,
        required_input,
        source::{
            media_in::MediaInSourceKindDelegate,
            text::{TextDraw, TextInstance},
        },
    },
    scope::{FrameTime, MAX_GRAPH_DEPTH, Routed, Scope},
};

#[cfg(feature = "ffmpeg")]
use super::media::{GpuMediaFrameImporter, GpuMediaImports};
use super::{
    BoundFrame,
    color::WORKING_COLOR_FORMAT as WORKING,
    params::{CropParams, copyable_texture_desc, dispatch_for},
    renderer::{MediaTextureCache, OutputEncoding, fit_frame_to_rgba8},
};
use state::{RecordState, Slot, VideoUpload};

mod ingest;
mod state;

/// Copies the visible part of a clip's raster into a texture of the parent canvas.
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

/// A recorded node's output.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Raster {
    pub(crate) tex: TexRef,
    pub(crate) domain: TextureDomain,
    /// The raster's logical bounds within its texture, when they are not all of it.
    pub(crate) bounds: Option<CropParams>,
}

impl Raster {
    pub(crate) fn origin(&self) -> [i32; 2] {
        [self.domain.data_rect.x, self.domain.data_rect.y]
    }
}

pub(crate) trait RecordNode {
    fn record(&self, cx: &mut FrameCx<'_>) -> crate::Result<Raster>;

    /// Builds every program this node may record with, whatever its params are at a frame.
    fn programs(&self, programs: &mut Programs<'_>) -> crate::Result<()>;
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

    pub(crate) fn compute(
        &mut self,
        label: &str,
        shader: &'static str,
        entry: &'static str,
        layout: &'static [LayoutEntry],
    ) -> crate::Result<()> {
        self.rec
            .compute_program(label, shader, entry, layout)
            .map_err(gpu_error)?;
        Ok(())
    }

    pub(crate) fn spatial(&mut self, label: &str, shader: &'static str) -> crate::Result<()> {
        self.compute(label, shader, "cs_main", SPATIAL_LAYOUT)
    }

    /// The program `FrameCx::record_clip` crops a clip's content with.
    pub(crate) fn clip_crop(&mut self) -> crate::Result<()> {
        self.spatial("clip-crop", CLIP_CROP_SHADER)
    }

    #[allow(clippy::too_many_arguments, reason = "a render program's parts")]
    pub(crate) fn render(
        &mut self,
        label: &str,
        shader: &'static str,
        vertex: &'static str,
        fragment: &'static str,
        layout: &'static [LayoutEntry],
        target: lumen_gpu::RenderTargetSpec,
        variant: u64,
    ) -> crate::Result<()> {
        self.rec
            .render_program(label, shader, vertex, fragment, layout, target, variant)
            .map_err(gpu_error)?;
        Ok(())
    }

    /// Builds a shader from runtime source, which recording it again then finds.
    pub(crate) fn dynamic(
        &mut self,
        node: NodeId,
        label: &str,
        source: &str,
        inputs: usize,
        build: impl FnOnce() -> crate::Result<ShaderParts>,
    ) -> crate::Result<CachedShader> {
        let shader = self
            .st
            .shaders
            .cached_shader(self.st.tick, source, inputs, build)?;
        dynamic_program(self.rec, node, label, &shader)?;
        Ok(shader)
    }
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
        RenderError::NodeEvaluation {
            node_id: node,
            node_kind: "wgsl_shader",
            details: error.to_string(),
        }
        .into()
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PathId(u32);

/// A node reached along one path of time remaps and clips, in one graph.
type InstanceKey = (usize, PathId, NodeId);
/// A node of one graph, wherever it is reached from.
type GraphNodeKey = (usize, NodeId);
/// A node reached along one path, in one sample of the `motion_blur` that path samples, or in
/// sample 0 outside one.
type MemoKey = (InstanceKey, u32);
/// A video frame a `media_in` showed this frame: the node's instance, the identity of the video it
/// read, the source frame, its size and whether its colour is premultiplied.
type VideoFrameKey = (InstanceKey, MediaIdentity, u32, (u32, u32), bool);

/// Whether a node's output is the same on every frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Staticness {
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

/// The frames a kept output stays correct for, and what it drew.
#[derive(Debug, Clone)]
struct HoldTrack {
    /// Both ends included.
    frames: (f64, f64),
    models: Vec<String>,
}

impl HoldTrack {
    fn new() -> Self {
        Self {
            frames: (f64::NEG_INFINITY, f64::INFINITY),
            models: Vec::new(),
        }
    }

    fn narrow(&mut self, frames: (f64, f64)) {
        self.frames = (self.frames.0.max(frames.0), self.frames.1.min(frames.1));
    }

    fn contains(&self, frame: f64) -> bool {
        self.frames.0 <= frame && frame <= self.frames.1
    }
}

/// The frames either side of `frame` among those a composition has.
fn neighbouring_frames(frame: f64) -> impl Iterator<Item = f64> {
    [frame.floor() + 1.0, frame.ceil() - 1.0]
        .into_iter()
        .filter(|other| *other >= 0.0)
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
    /// What the caches recording fills keep of what frames stop using.
    budgets: super::CacheBudgets,
    /// The target of the next frame recorded, which holds its output until it is submitted.
    external: Option<lumen_gpu::ExternalTexture>,
    #[cfg(feature = "ffmpeg")]
    imports: GpuMediaImports,
}

pub(crate) struct RecordInput<'a> {
    pub(crate) composition: &'a Composition,
    pub(crate) frame: f64,
    pub(crate) media: Option<&'a dyn MediaStore>,
    pub(crate) output_format: wgpu::TextureFormat,
    pub(crate) output_encoding: OutputEncoding,
    pub(crate) full_frame_sources: bool,
    pub(crate) single_tile_repeats: bool,
    pub(crate) retain_static: bool,
    pub(crate) media_cache: &'a mut MediaTextureCache,
    pub(crate) text_atlas: &'a Arc<Mutex<lumen_text::GlyphAtlas>>,
    pub(crate) atlas_texture: &'a mut Option<Arc<wgpu::Texture>>,
    pub(crate) max_rebinds: usize,
    pub(crate) max_text_glyphs: usize,
    /// The longest side an image is decoded at, below the device's own limit.
    pub(crate) max_image_side: u32,
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
            output_format,
            output_encoding,
            full_frame_sources,
            single_tile_repeats,
            retain_static,
            media_cache,
            text_atlas,
            atlas_texture,
            max_rebinds,
            max_text_glyphs,
            max_image_side,
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
        let mut rebinds = 0;
        loop {
            self.drop_pending();
            let generation = font_generation(media);
            if self.state.font_generation != generation
                || self.state.retained.sources_changed(media)
            {
                self.state.retained.drop_all(&mut self.recorder);
                self.state.font_generation = generation;
            }
            self.state.begin_frame();
            self.recorder.begin_frame();
            let mut cx = FrameCx {
                rec: &mut self.recorder,
                st: &mut self.state,
                media_cache: &mut *media_cache,
                composition,
                scope: Scope::root(composition),
                frame: frame.into(),
                path: PathId(0),
                sample: 0,
                sampling: false,
                media,
                output_format,
                output_encoding,
                full_frame_sources,
                single_tile_repeats,
                retain_static,
                static_depth: 0,
                holds: Vec::new(),
                depth: 0,
                bound: BoundFrame::new(),
                max_text_glyphs,
                max_image_side,
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
            if rebinds == max_rebinds {
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
        let root = composition.graph();
        let mut pending = vec![(root, root.output())];
        let mut seen = HashSet::default();
        while let Some((graph, id)) = pending.pop() {
            if !seen.insert((std::ptr::from_ref(graph) as usize, id)) {
                continue;
            }
            let Some(node) = graph.nodes.get(&id) else {
                continue;
            };
            node.programs(&mut programs)?;
            match node {
                NodeKind::Clip(clip) if clip.template_name().is_some() => {
                    let template = clip
                        .template_name()
                        .and_then(|name| composition.template(name));
                    if let Some((template, output)) = template {
                        pending.push((&template.scene.graph, output));
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
                    pending.extend(layers.iter().map(|&layer| (graph, layer)));
                }
                _ => pending.extend(
                    node.input_ports()
                        .into_iter()
                        .flatten()
                        .map(|input| (graph, input)),
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

    /// The domain `node` of the root graph had in the frame last recorded.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn node_domain(
        &self,
        composition: &Composition,
        node: NodeId,
    ) -> Option<TextureDomain> {
        let graph = std::ptr::from_ref(composition.graph()) as usize;
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

/// The identity of what `media` serves for `source` as `kind`, if it serves anything.
fn source_identity(
    media: Option<&dyn MediaStore>,
    kind: AssetKind,
    source: &str,
) -> Option<MediaIdentity> {
    Some(media?.resolve(source, kind).ok()?.identity)
}

/// Whether what `media` serves for `source` as `kind` is no longer what was `seen`: another
/// identity, or there where it was missing or the other way round. A source that fails for any
/// other reason than being missing counts as changed, so that reading it again reports why.
fn source_changed(
    media: Option<&dyn MediaStore>,
    kind: AssetKind,
    source: &str,
    seen: Option<&MediaIdentity>,
) -> bool {
    let Some(media) = media else {
        return seen.is_some();
    };
    match media.resolve(source, kind) {
        Ok(resolved) => Some(&resolved.identity) != seen,
        Err(error) => {
            let missing = error.failure().is_some_and(|failure| {
                matches!(failure.reason, crate::error::MediaReason::NotFound { .. })
            });
            !missing || seen.is_some()
        }
    }
}

fn font_generation(media: Option<&dyn MediaStore>) -> Option<u64> {
    crate::node::source::text::text_system(media)
        .ok()
        .map(|text_system| text_system.font_generation())
}

/// `extent` rounded up to the next size class that textures are pooled by.
pub(crate) fn size_class(extent: u32) -> u32 {
    let step = match extent {
        0..256 => 16,
        256..512 => 32,
        512..1024 => 64,
        _ => 128,
    };
    extent.next_multiple_of(step)
}

/// A raster's domain within `display`, rounded out to a size class so that bounds moving from
/// frame to frame keep finding the same textures, and the bind groups cached on them.
pub(crate) fn quantized_domain(display: Size, data: lumen_gpu::RectI) -> TextureDomain {
    let domain = TextureDomain::bounded(display, data);
    let rect = domain.data_rect;
    if data.is_empty() {
        return domain;
    }
    let class = |extent: u32, limit: u32| size_class(extent).min(limit.max(1));
    let width = class(rect.width, display.width);
    let height = class(rect.height, display.height);
    let x = rect.x.min(display.width.saturating_sub(width) as i32);
    let y = rect.y.min(display.height.saturating_sub(height) as i32);
    TextureDomain::bounded(display, lumen_gpu::RectI::new(x, y, width, height))
}

/// One walk of the graph for one frame.
pub(crate) struct FrameCx<'a> {
    pub(crate) rec: &'a mut Recorder,
    st: &'a mut RecordState,
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
    output_format: wgpu::TextureFormat,
    output_encoding: OutputEncoding,
    full_frame_sources: bool,
    /// Whether repeats draw from one tile, as `set_single_tile_repeats` sets.
    pub(crate) single_tile_repeats: bool,
    retain_static: bool,
    /// How many static nodes the walk is inside, whose readers retain them rather than this.
    static_depth: u32,
    /// What the nodes that are static while their models hold a pose or their params' values hold,
    /// which the walk is inside, have found out so far, innermost last.
    holds: Vec<HoldTrack>,
    /// How many nodes the walk is recording inside.
    depth: u32,
    pub(crate) bound: BoundFrame,
    max_text_glyphs: usize,
    max_image_side: u32,
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
pub(crate) fn gpu_error(error: anyhow::Error) -> crate::error::LumenError {
    let details = error.to_string();
    match error.downcast_ref::<lumen_gpu::FrameError>() {
        Some(lumen_gpu::FrameError::TooLarge(_)) => RenderError::Limit { details },
        Some(lumen_gpu::FrameError::Misuse(_)) => RenderError::Internal { details },
        None => RenderError::Gpu { details },
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
    pub(crate) fn expr(&self) -> ExpressionContext<'_> {
        self.scope.expr_context(self.frame, self.media)
    }

    /// Evaluates `text` at this frame, placing its glyphs in the atlas and writing what it draws
    /// to the frame's buffers.
    pub(crate) fn bind_text(&mut self, text: &TextInstance) -> crate::Result<TextDraw> {
        let expr = self.scope.expr_context(self.frame, self.media);
        let mut fallbacks = Vec::new();
        let draw = text.bind(
            expr,
            self.media,
            &mut self.bound,
            self.rec,
            &mut fallbacks,
            self.max_text_glyphs,
        )?;
        for family in fallbacks {
            self.note_font_fallback(text.node_id, family);
        }
        Ok(draw)
    }

    /// Warns once per composition and `render_3d` node `node_id` that `lights` cast no shadows.
    #[cfg(feature = "3d")]
    pub(crate) fn note_shadow_limit(&mut self, node_id: NodeId, lights: Vec<NodeId>) {
        let key = (self.scope.template_name().map(str::to_string), node_id);
        if self.st.scene.shadow_limits.contains(&key) {
            return;
        }
        let warning = crate::error::Warning::ShadowLimit {
            template: key.0.clone(),
            node_id,
            lights,
        };
        tracing::warn!(target: "lumen_3d", "{warning}");
        let path = crate::diagnostic::scene_pointer(key.0.as_deref());
        self.st
            .diagnostics
            .push(crate::diagnostic::Diagnostic::from_warning(&warning, path));
        self.st.scene.shadow_limits.insert(key);
    }

    /// Warns once per composition and `text_3d` node `node_id` that `glyphs` of its glyphs have
    /// no outlines.
    #[cfg(feature = "3d")]
    pub(crate) fn note_missing_outlines(&mut self, node_id: NodeId, glyphs: usize) {
        let key = (self.scope.template_name().map(str::to_string), node_id);
        if self.st.scene.outline_gaps.contains(&key) {
            return;
        }
        let warning = crate::error::Warning::NoOutlines {
            template: key.0.clone(),
            node_id,
            glyphs,
        };
        tracing::warn!(target: "lumen_text", "{warning}");
        let path = crate::diagnostic::scene_pointer(key.0.as_deref());
        self.st
            .diagnostics
            .push(crate::diagnostic::Diagnostic::from_warning(&warning, path));
        self.st.scene.outline_gaps.insert(key);
    }

    /// Warns once per composition and `model_3d` node `node_id` that its model `source` has
    /// `count` primitives that aren't drawn.
    #[cfg(feature = "3d")]
    pub(crate) fn note_skipped_primitives(&mut self, node_id: NodeId, source: &str, count: usize) {
        let key = (self.scope.template_name().map(str::to_string), node_id);
        if self.st.scene.skipped_primitives.contains(&key) {
            return;
        }
        let warning = crate::error::Warning::ModelPrimitivesSkipped {
            template: key.0.clone(),
            node_id,
            model: source.to_string(),
            count,
        };
        tracing::warn!(target: "lumen_3d", "{warning}");
        let path = crate::diagnostic::scene_pointer(key.0.as_deref());
        self.st
            .diagnostics
            .push(crate::diagnostic::Diagnostic::from_warning(&warning, path));
        self.st.scene.skipped_primitives.insert(key);
    }

    /// The most glyphs a text node may draw.
    #[cfg(feature = "3d")]
    pub(crate) fn max_text_glyphs(&self) -> usize {
        self.max_text_glyphs
    }

    /// Warns once per composition that `family` fell back for text node `node_id`.
    pub(crate) fn note_font_fallback(&mut self, node_id: NodeId, family: String) {
        let key = (
            self.scope.template_name().map(str::to_string),
            node_id,
            family,
        );
        if self.st.text.font_fallbacks.contains(&key) {
            return;
        }
        let warning = crate::error::Warning::FontFallback {
            template: key.0.clone(),
            node_id,
            family: key.2.clone(),
        };
        tracing::warn!(target: "lumen_text", "{warning}");
        let path = crate::diagnostic::scene_pointer(key.0.as_deref());
        self.st
            .diagnostics
            .push(crate::diagnostic::Diagnostic::from_warning(&warning, path));
        self.st.text.font_fallbacks.insert(key);
    }

    /// Warns once per composition, `media_in` node `node_id` and `media`, a `kind` source, that
    /// `details` of its colour aren't converted.
    pub(crate) fn note_media_color(
        &mut self,
        node_id: NodeId,
        kind: AssetKind,
        media: &str,
        details: &str,
    ) {
        let key = (
            self.scope.template_name().map(str::to_string),
            node_id,
            media.to_string(),
        );
        if self.st.media.colors.contains(&key) {
            return;
        }
        let warning = crate::error::Warning::UnsupportedColor {
            template: key.0.clone(),
            node_id,
            kind,
            media: key.2.clone(),
            details: details.to_string(),
        };
        tracing::warn!(target: "lumen_media", "{warning}");
        let path = crate::diagnostic::scene_pointer(key.0.as_deref());
        self.st
            .diagnostics
            .push(crate::diagnostic::Diagnostic::from_warning(&warning, path));
        self.st.media.colors.insert(key);
    }

    pub(crate) fn media(&self) -> Option<&'a dyn MediaStore> {
        self.media
    }

    pub(crate) fn composition(&self) -> &'a Composition {
        self.composition
    }

    pub(crate) fn output_format(&self) -> wgpu::TextureFormat {
        self.output_format
    }

    pub(crate) fn output_encoding(&self) -> OutputEncoding {
        self.output_encoding
    }

    pub(crate) fn frame_number(&self) -> u32 {
        self.frame.value() as u32
    }

    pub(crate) fn frame_time(&self) -> FrameTime {
        self.frame
    }

    #[cfg(feature = "3d")]
    pub(crate) fn node(&self, id: NodeId) -> Option<&'a NodeKind> {
        self.scope.node(id)
    }

    /// Counts frames recorded, for caches that keep what recent frames used.
    pub(crate) fn tick(&self) -> u64 {
        self.st.tick
    }

    /// Notes that the model, environment map or text mesh `id` was drawn, so that a kept output
    /// recording now keeps it.
    #[cfg(feature = "3d")]
    pub(crate) fn drew_model(&mut self, id: &str) {
        if self.static_depth > 0 {
            self.st.retained.static_models.push(id.to_string());
        }
        if let Some(track) = self.holds.last_mut() {
            track.models.push(id.to_string());
        }
    }

    /// Notes that the model being drawn holds one pose on `frames`, both ends included, which a
    /// kept output recording now can only be read on.
    #[cfg(feature = "3d")]
    pub(crate) fn held_on(&mut self, frames: (f64, f64)) {
        if let Some(track) = self.holds.last_mut() {
            track.narrow(frames);
        }
    }

    /// What keeps `raster`, the output of `node`, the same from frame to frame, if anything.
    pub(crate) fn stable_raster(
        &self,
        node: NodeId,
        raster: Raster,
    ) -> Option<crate::gpu::mips::Stable> {
        use crate::gpu::mips::Stable;
        let graph = self.graph_key();
        if let Some(slot) = self.st.retained.slots.get(&(graph, node))
            && (slot.raster.tex == raster.tex || slot.full == Some(raster.tex))
        {
            return Some(Stable::Slot {
                graph,
                node,
                generation: slot.generation,
            });
        }
        self.st
            .media
            .textures
            .values()
            .find(|(tex, ..)| *tex == raster.tex)
            .and_then(|(.., key)| key.clone())
            .map(Stable::Image)
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

    pub(crate) fn canvas_size(&self) -> Size {
        let settings = &self.scope.scene.render_settings;
        Size::new(settings.width.max(1), settings.height.max(1))
    }

    fn graph_key(&self) -> usize {
        std::ptr::from_ref(&self.scope.scene.graph) as usize
    }

    /// Records the required input `port` of `node_id` as is.
    pub(crate) fn required(
        &mut self,
        node_id: NodeId,
        port: &str,
        input: Option<NodeId>,
    ) -> crate::Result<Raster> {
        self.input(required_input(node_id, port, input)?)
    }

    /// Records the required input `port` of `node_id` at its full display size.
    pub(crate) fn required_full(
        &mut self,
        node_id: NodeId,
        port: &str,
        input: Option<NodeId>,
    ) -> crate::Result<Raster> {
        self.input_full(required_input(node_id, port, input)?)
    }

    /// Records `node_id` as is, for readers that read a raster through its domain.
    pub(crate) fn input(&mut self, node_id: NodeId) -> crate::Result<Raster> {
        let node = self
            .scope
            .node(node_id)
            .ok_or(RenderError::MissingNode { node_id })?;
        let graph = self.graph_key();
        let staticness = if self.retain_static {
            self.staticness(node)
        } else {
            Staticness::Never
        };
        // A value that fails to evaluate fails recording, which reports it.
        let values = match staticness {
            Staticness::WhileEqual => self.frame_values(node_id, self.frame),
            _ => Some(Vec::new()),
        };
        let (staticness, values) = match values {
            Some(values) => (staticness, values),
            None => (Staticness::Never, Vec::new()),
        };
        let is_static = staticness == Staticness::Always;
        let tracked = matches!(staticness, Staticness::WhileHeld | Staticness::WhileEqual);
        let frame = self.frame.value();
        if staticness != Staticness::Never
            && let Some(slot) = self.st.retained.slots.get_mut(&(graph, node_id))
            && slot.frames.0 <= frame
            && frame <= slot.frames.1
            && slot.values == values
        {
            slot.last_used = self.st.tick;
            self.st.retained.skipped_passes += slot.passes;
            let raster = slot.raster;
            self.hold_slot(graph, node_id);
            return Ok(raster);
        }
        let key = ((graph, self.path, node_id), self.sample);
        let raster = match self.st.memo.get(&key) {
            Some(raster) => {
                let raster = *raster;
                if let Some(read) = self.st.retained.held_memo.get(&key)
                    && let Some(track) = self.holds.last_mut()
                {
                    track.narrow(read.frames);
                    track.models.extend(read.models.iter().cloned());
                }
                raster
            }
            None => {
                if self.depth == MAX_GRAPH_DEPTH {
                    return Err(self.scope.too_deep(node_id));
                }
                let passes = self.rec.pass_count();
                self.static_depth += u32::from(is_static);
                if tracked {
                    self.holds.push(HoldTrack::new());
                }
                self.depth += 1;
                let raster = record_node(node, self);
                self.depth -= 1;
                self.static_depth -= u32::from(is_static);
                let track = tracked.then(|| self.holds.pop()).flatten();
                let raster = raster?;
                self.st.memo.insert(key, raster);
                let passes = (self.rec.pass_count() - passes) as u64;
                if is_static && self.static_depth == 0 {
                    let frames = (f64::NEG_INFINITY, f64::INFINITY);
                    let models = std::mem::take(&mut self.st.retained.static_models);
                    self.retain(graph, node_id, raster, passes, frames, models, Vec::new());
                }
                if let Some(track) = track {
                    self.finish_hold(key, raster, passes, track, values);
                }
                raster
            }
        };
        Ok(raster)
    }

    /// Keeps `raster`, the output of `node`, for the `frames` it stays correct on while the params
    /// reading the frame take `values`, along with the `models` it drew.
    #[expect(clippy::too_many_arguments, reason = "the parts of a slot")]
    fn retain(
        &mut self,
        graph: usize,
        node: NodeId,
        raster: Raster,
        passes: u64,
        frames: (f64, f64),
        models: Vec<String>,
        values: Vec<ExpressionValue>,
    ) {
        // A slot kept for other frames makes way.
        self.st.retained.release(self.rec, (graph, node));
        self.rec.retain(raster.tex);
        let bytes = self.rec.texture_bytes(raster.tex);
        self.st.retained.bytes += bytes;
        self.st.retained.generation += 1;
        self.st.retained.slots.insert(
            (graph, node),
            Slot {
                raster,
                passes,
                full: None,
                last_used: self.st.tick,
                bytes,
                generation: self.st.retained.generation,
                frames,
                models,
                values,
            },
        );
        self.st.retained.pending.push((graph, node));
    }

    /// Notes in the output being recorded that it reads the kept output of `node`, which holds
    /// only on the frames the slot is correct on.
    fn hold_slot(&mut self, graph: usize, node: NodeId) {
        let Some(track) = self.holds.last_mut() else {
            return;
        };
        if let Some(slot) = self.st.retained.slots.get(&(graph, node)) {
            track.narrow(slot.frames);
            track.models.extend(slot.models.iter().cloned());
        }
    }

    /// Keeps `raster`, the output of `node` just recorded with the params reading the frame at
    /// `values`, if it stays correct on this frame and on one either side of it too, and tells the
    /// output being recorded around it what it found.
    fn finish_hold(
        &mut self,
        key: MemoKey,
        raster: Raster,
        passes: u64,
        track: HoldTrack,
        values: Vec<ExpressionValue>,
    ) {
        let ((graph, _, node), _) = key;
        if let Some(outer) = self.holds.last_mut() {
            outer.narrow(track.frames);
            outer.models.extend(track.models.iter().cloned());
        }
        self.st.retained.held_memo.insert(key, track.clone());
        let frame = self.frame.value();
        let kept_elsewhere = |other: f64| {
            track.contains(other) && self.frame_values(node, other.into()).as_ref() == Some(&values)
        };
        if self.static_depth == 0
            && track.contains(frame)
            && neighbouring_frames(frame).any(kept_elsewhere)
        {
            self.retain(
                graph,
                node,
                raster,
                passes,
                track.frames,
                track.models,
                values,
            );
        }
    }

    /// The values that the params reading the frame take at `frame`: those of `node` and of each
    /// node it reads that is kept while they hold, in the same order on every frame. A node that
    /// blurs its own motion adds their values at either end of its shutter, where it evaluates
    /// them too. `None` when one fails to evaluate.
    fn frame_values(&self, node: NodeId, frame: FrameTime) -> Option<Vec<ExpressionValue>> {
        let graph = self.graph_key();
        let expr = self.scope.expr_context(frame, self.media);
        let mut values = Vec::new();
        let mut pending = vec![node];
        let mut seen = HashSet::default();
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            let node = self.scope.node(id)?;
            let mut contexts = vec![expr];
            if self.analytic_motion_blur()
                && let Some(shutter) = analytic_shutter(node)
                && let Some(offset) = crate::node::motion::shutter_offset(
                    shutter.eval(id, "motion_blur", &expr).ok()?,
                )
            {
                contexts.extend([expr.at_subframe(-offset), expr.at_subframe(offset)]);
            }
            let mut failed = false;
            for expr in &contexts {
                node.for_each_expression(&mut |expression| {
                    if !expression.is_frame_dependent() {
                        return;
                    }
                    match expression.evaluate(expr) {
                        Ok(value) => values.push(value),
                        Err(_) => failed = true,
                    }
                });
            }
            if failed {
                return None;
            }
            let held = |input: &NodeId| {
                self.st.statics.get(&(graph, *input)) == Some(&Staticness::WhileEqual)
            };
            pending.extend(node.input_ports().into_iter().flatten().filter(held));
        }
        Some(values)
    }

    /// Records `node_id` stored at its full display size, for readers that read texel for texel.
    pub(crate) fn input_full(&mut self, node_id: NodeId) -> crate::Result<Raster> {
        let raster = self.input(node_id)?;
        if raster.domain.is_full_frame() {
            return Ok(raster);
        }
        let slot_key = (self.graph_key(), node_id);
        if let Some(slot) = self.st.retained.slots.get(&slot_key)
            && let Some(full) = slot.full
        {
            return Ok(Raster {
                tex: full,
                domain: TextureDomain::full_frame(raster.domain.display_size()),
                bounds: None,
            });
        }
        let full = self.full_frame(raster);
        if self
            .st
            .retained
            .slots
            .get(&slot_key)
            .is_some_and(|slot| slot.raster.tex == raster.tex)
        {
            self.rec.retain(full.tex);
            let bytes = self.rec.texture_bytes(full.tex);
            self.st.retained.bytes += bytes;
            let slot = self.st.retained.slots.get_mut(&slot_key).expect("slot");
            slot.full = Some(full.tex);
            slot.bytes += bytes;
            if !self.st.retained.pending.contains(&slot_key) {
                self.st.retained.pending.push(slot_key);
            }
        }
        Ok(full)
    }

    /// `node_id` stored at its full display size, where a clip keeps the canvas its content is
    /// drawn on rather than being cropped to this one, for readers that place it anywhere, such
    /// as 3D. Content that draws less than its canvas is padded with transparency to it, and
    /// content that draws more is cropped.
    #[cfg(feature = "3d")]
    pub(crate) fn input_native(&mut self, node_id: NodeId) -> crate::Result<Raster> {
        if let Some(NodeKind::Clip(clip)) = self.scope.node(node_id) {
            let canvas = clip.content_canvas(&self.scope)?;
            let max = self.rec.device().limits().max_texture_dimension_2d;
            if canvas.width > max || canvas.height > max {
                return Err(RenderError::NodeEvaluation {
                    node_id,
                    node_kind: "Clip",
                    details: format!(
                        "a {}x{} canvas is larger than this device's {max}-pixel texture limit",
                        canvas.width, canvas.height
                    ),
                }
                .into());
            }
            return match clip.route(&self.scope, self.frame, self.media)? {
                Some(routed) => {
                    let raster = self.place_routed(routed, canvas)?;
                    Ok(self.full_frame(raster))
                }
                None => Ok(self.transparent(canvas)),
            };
        }
        self.input_full(node_id)
    }

    /// `raster` stored at its full display size, copied there when it is bounded.
    pub(crate) fn full_frame(&mut self, raster: Raster) -> Raster {
        if raster.domain.is_full_frame() {
            return raster;
        }
        let size = raster.domain.display_size();
        let key = (raster.tex, size.width, size.height);
        if let Some(full) = self.st.full_frames.get(&key) {
            return *full;
        }
        let tex = self
            .rec
            .acquire(size, WORKING, copyable_texture_desc(size).usage);
        // A reused texture still holds an earlier frame, and the copy only writes the data.
        self.rec.clear(tex);
        let data = raster.domain.data_rect;
        self.rec.copy(
            raster.tex,
            tex,
            [data.x as u32, data.y as u32],
            raster.domain.storage_size,
        );
        let full = Raster {
            tex,
            domain: TextureDomain::full_frame(size),
            bounds: None,
        };
        self.st.full_frames.insert(key, full);
        full
    }

    /// The pixels of a box a source draws in this frame, or `None` when it should fill the canvas.
    pub(crate) fn bounds_of(&self, bounds: Option<[f32; 4]>) -> Option<lumen_gpu::RectI> {
        if self.full_frame_sources {
            return None;
        }
        bounds.and_then(lumen_gpu::RectI::covering)
    }

    pub(crate) fn source_domain(&self, bounds: Option<lumen_gpu::RectI>) -> TextureDomain {
        self.domain_within(self.canvas_size(), bounds)
    }

    pub(crate) fn domain_within(
        &self,
        display: Size,
        bounds: Option<lumen_gpu::RectI>,
    ) -> TextureDomain {
        match bounds {
            Some(bounds) => quantized_domain(display, bounds),
            None => TextureDomain::full_frame(display),
        }
    }

    /// A storage target that starts transparent, for a writer that may not cover all of it.
    pub(crate) fn cleared_storage_target(&mut self, size: Size) -> TexRef {
        let tex = self
            .rec
            .acquire(size, WORKING, copyable_texture_desc(size).usage);
        self.rec.clear(tex);
        tex
    }

    pub(crate) fn storage_target(&mut self, size: Size) -> TexRef {
        self.rec
            .acquire(size, WORKING, TextureDesc::storage(size, WORKING).usage)
    }

    pub(crate) fn float_target(&mut self, size: Size) -> TexRef {
        let format = wgpu::TextureFormat::Rgba16Float;
        self.rec
            .acquire(size, format, TextureDesc::storage(size, format).usage)
    }

    pub(crate) fn render_target(&mut self, size: Size) -> TexRef {
        self.rec.acquire(
            size,
            WORKING,
            TextureDesc::render_target(size, WORKING).usage,
        )
    }

    /// The frame's output. An output kept from an earlier frame is copied into an external target,
    /// so targets made from its usage take copies.
    pub(crate) fn output_target(&mut self, size: Size, format: wgpu::TextureFormat) -> TexRef {
        let usage = wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT;
        self.rec.acquire(size, format, usage)
    }

    pub(crate) fn program(
        &mut self,
        label: &str,
        shader: &'static str,
        entry: &'static str,
        layout: &'static [LayoutEntry],
    ) -> crate::Result<ProgramRef> {
        self.rec
            .compute_program(label, shader, entry, layout)
            .map_err(gpu_error)
    }

    #[allow(clippy::too_many_arguments, reason = "a render program's parts")]
    pub(crate) fn render_program(
        &mut self,
        label: &str,
        shader: &'static str,
        vertex: &'static str,
        fragment: &'static str,
        layout: &'static [LayoutEntry],
        target: lumen_gpu::RenderTargetSpec,
        variant: u64,
    ) -> crate::Result<ProgramRef> {
        self.rec
            .render_program(label, shader, vertex, fragment, layout, target, variant)
            .map_err(gpu_error)
    }

    /// Runs `shader` from `source` into `target` over `size`.
    pub(crate) fn spatial_pass<T: bytemuck::Pod>(
        &mut self,
        label: &str,
        shader: &'static str,
        source: TexRef,
        params: &T,
        target: TexRef,
        size: Size,
    ) -> crate::Result<()> {
        let program = self.program(label, shader, "cs_main", SPATIAL_LAYOUT)?;
        let uniform = self.rec.uniform(params);
        self.rec.compute(
            program,
            &[(0, Res::Tex(source)), (1, uniform), (2, Res::Tex(target))],
            dispatch_for(size),
        );
        Ok(())
    }

    /// A filter over `source`, writing a raster of the same domain, metadata and bounds.
    pub(crate) fn filter<T: bytemuck::Pod>(
        &mut self,
        label: &str,
        shader: &'static str,
        source: Raster,
        params: &T,
    ) -> crate::Result<Raster> {
        let size = source.domain.storage_size;
        let tex = self.storage_target(size);
        self.spatial_pass(label, shader, source.tex, params, tex, size)?;
        Ok(Raster { tex, ..source })
    }

    pub(crate) fn dispatch(
        &mut self,
        program: ProgramRef,
        bindings: &[(u32, Res)],
        dispatch: Dispatch,
    ) {
        self.rec.compute(program, bindings, dispatch);
    }

    /// A transparent raster of `display` size, which stores a texel nothing writes.
    pub(crate) fn transparent(&mut self, display: Size) -> Raster {
        let tex = match self.st.transparent {
            Some(tex) => tex,
            None => {
                let texture = self.rec.device().create_texture(&wgpu::TextureDescriptor {
                    label: Some("lumen frame graph transparent"),
                    size: Size::new(1, 1).as_extent(),
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: WORKING,
                    usage: copyable_texture_desc(Size::new(1, 1)).usage,
                    view_formats: &[],
                });
                let tex = self.rec.adopt(Arc::new(texture));
                self.st.transparent = Some(tex);
                tex
            }
        };
        Raster {
            tex,
            domain: TextureDomain::bounded(display, lumen_gpu::RectI::new(0, 0, 0, 0)),
            bounds: None,
        }
    }

    /// A one-texel float texture for gaussian levels, or a running sum, a pass binds but does not
    /// read.
    pub(crate) fn unused_float(&mut self) -> TexRef {
        match self.st.dummy_float {
            Some(tex) => tex,
            None => {
                let texture = self.rec.device().create_texture(&wgpu::TextureDescriptor {
                    label: Some("lumen frame graph unused level"),
                    size: Size::new(1, 1).as_extent(),
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba16Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
                let tex = self.rec.adopt(Arc::new(texture));
                self.st.dummy_float = Some(tex);
                tex
            }
        }
    }

    pub(crate) fn sampler(
        &mut self,
        key: u64,
        desc: impl FnOnce() -> wgpu::SamplerDescriptor<'static>,
    ) -> SamplerRef {
        self.rec.sampler(key, desc)
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

    /// Notes that `source` was read as `kind` and had `identity`, so that what is kept of this
    /// frame is dropped once it has another. It counts while a kept output records, including one
    /// kept while its models hold a pose or its params' values hold; font families count whenever
    /// text names them, as text instances are kept across frames whatever reads them.
    pub(crate) fn read_source(
        &mut self,
        kind: AssetKind,
        source: &str,
        identity: Option<&MediaIdentity>,
    ) {
        let kept = self.static_depth > 0 || !self.holds.is_empty() || kind == AssetKind::Font;
        if kept
            && !self
                .st
                .retained
                .sources
                .contains_key(&(kind, source.to_string()))
        {
            self.st
                .retained
                .sources
                .insert((kind, source.to_string()), identity.cloned());
        }
    }

    /// Notes the font family text names, as [`Self::read_source`] does.
    pub(crate) fn note_font(&mut self, family: &str) {
        if !self
            .st
            .retained
            .sources
            .contains_key(&(AssetKind::Font, family.to_string()))
        {
            let identity = source_identity(self.media, AssetKind::Font, family);
            self.read_source(AssetKind::Font, family, identity.as_ref());
        }
    }

    /// The text instance's state across frames, built once.
    pub(crate) fn text_instance(
        &mut self,
        node_id: NodeId,
        build: impl FnOnce(&Self) -> crate::Result<TextInstance>,
    ) -> crate::Result<Arc<TextInstance>> {
        let key = (self.graph_key(), self.path, node_id);
        if let Some(text) = self.st.text.instances.get(&key) {
            return Ok(Arc::clone(text));
        }
        let text = Arc::new(build(self)?);
        self.st.text.instances.insert(key, Arc::clone(&text));
        Ok(text)
    }

    /// The parsed and assembled form of a runtime shader source, built once per source.
    pub(crate) fn cached_shader(
        &mut self,
        source: &str,
        inputs: usize,
        build: impl FnOnce() -> crate::Result<ShaderParts>,
    ) -> crate::Result<CachedShader> {
        self.st
            .shaders
            .cached_shader(self.st.tick, source, inputs, build)
    }

    pub(crate) fn dynamic_program(
        &mut self,
        node: NodeId,
        label: &str,
        shader: &CachedShader,
    ) -> crate::Result<ProgramRef> {
        dynamic_program(self.rec, node, label, shader)
    }

    /// The param `param` of node `node` in the graph being recorded.
    pub(crate) fn node_param(&self, node: NodeId, param: &str) -> crate::media::MediaLocation {
        crate::media::MediaLocation::Node(crate::media::NodeParamAt {
            template: self.scope.template_name().map(str::to_string),
            node,
            param: param.to_string(),
        })
    }

    /// Fails as `media.too_large` when `size`, a media source's native size, is more than this
    /// device makes a texture.
    pub(crate) fn ensure_media_fits(&self, source: &str, size: Size) -> crate::Result<()> {
        let max = self.rec.device().limits().max_texture_dimension_2d;
        let side = size.width.max(size.height);
        if side <= max {
            return Ok(());
        }
        Err(crate::error::MediaFailure::too_large(
            source,
            format!(
                "it is {}x{} pixels, over the {max}-pixel texture limit of this device",
                size.width, size.height
            ),
            u64::from(max),
            Some(u64::from(side)),
        )
        .into())
    }

    /// The size image `source`, of `native` size, is decoded at for `node_id` to show it (see
    /// [`super::image_size::decoded_size`]), with a warning once per composition when it is
    /// reduced to fit the size limit. Fails as `media.too_large` when it is past the device's
    /// limit and can't be reduced.
    pub(crate) fn image_size(
        &mut self,
        node_id: NodeId,
        source: &str,
        native: Size,
    ) -> crate::Result<Size> {
        use super::image_size::{Decoded, decoded_size};
        let shown = Arc::clone(
            self.st
                .media
                .shown_sizes
                .get_or_insert_with(|| super::image_size::ShownSizes::of(self.composition)),
        );
        let device_max = self.rec.device().limits().max_texture_dimension_2d.max(1);
        let size = match decoded_size(&shown, source, native, self.max_image_side, device_max) {
            Decoded::Native => native,
            Decoded::Reduced { size, limited } => {
                if limited {
                    self.warn_image_reduced(node_id, source, native, size);
                }
                size
            }
            Decoded::TooLarge => {
                let side = native.width.max(native.height);
                return Err(crate::error::MediaFailure::too_large(
                    source,
                    format!(
                        "it is {}x{} pixels, over the {device_max}-pixel texture limit of this \
                         device, and only an image every use of which is a `resize` to a literal \
                         size, in the root composition, is shown reduced to its aspect ratio",
                        native.width, native.height
                    ),
                    u64::from(device_max),
                    Some(u64::from(side)),
                )
                .into());
            }
        };
        Ok(size)
    }

    fn warn_image_reduced(&mut self, node_id: NodeId, source: &str, native: Size, size: Size) {
        // Warnings stop at a fixed number of sources, so a composition of many large images
        // doesn't fill the diagnostics.
        const WARNED_SOURCES: usize = 32;
        if self.st.media.reduced_images.len() >= WARNED_SOURCES
            || !self.st.media.reduced_images.insert(source.to_string())
        {
            return;
        }
        let limit = self
            .rec
            .device()
            .limits()
            .max_texture_dimension_2d
            .min(self.max_image_side)
            .max(1);
        let warning = crate::error::Warning::ImageReduced {
            template: self.scope.template_name().map(str::to_string),
            node_id,
            image: source.to_string(),
            width: native.width,
            height: native.height,
            reduced_width: size.width,
            reduced_height: size.height,
            limit,
        };
        tracing::warn!(target: "lumen_render", "{warning}");
        let path = crate::diagnostic::scene_pointer(self.scope.template_name());
        self.st
            .diagnostics
            .push(crate::diagnostic::Diagnostic::from_warning(&warning, path));
    }

    /// Whether analytic motion blur applies. It is off inside a `motion_blur` that samples, whose
    /// samples already spread motion over the shutter.
    pub(crate) fn analytic_motion_blur(&self) -> bool {
        !self.sampling
    }

    /// Records `source` at each of `offsets` frames from this one and averages the samples in
    /// linear, premultiplied colour. Each sample is summed into a running target as soon as it is recorded,
    /// so its textures go back to the pool for the next. The samples share a path of their own,
    /// keeping their text and video state apart from the source's outside the blur.
    pub(crate) fn record_motion_blur(
        &mut self,
        node_id: NodeId,
        source: NodeId,
        offsets: &[f64],
    ) -> crate::Result<Raster> {
        let (frame, path) = (self.frame, self.path);
        let next = PathId(self.st.paths.len() as u32 + 1);
        self.path = *self.st.paths.entry((path, node_id)).or_insert(next);
        self.sampling = true;
        let result = self.accumulate(source, frame, offsets);
        self.sampling = false;
        self.sample = 0;
        self.path = path;
        self.frame = frame;
        result
    }

    fn accumulate(
        &mut self,
        source: NodeId,
        frame: FrameTime,
        offsets: &[f64],
    ) -> crate::Result<Raster> {
        let mut sum: Option<Raster> = None;
        // The domain and bounds every sample had, which the average keeps when they all agree.
        let mut shared: Option<Option<(TextureDomain, Option<CropParams>)>> = None;
        for (index, offset) in offsets.iter().enumerate() {
            self.sample = index as u32;
            self.frame = frame.offset(*offset);
            let sample = self.input(source)?;
            let this = (sample.domain, sample.bounds);
            shared = Some(match shared {
                None => Some(this),
                Some(kept) => kept.filter(|kept| *kept == this),
            });
            let last = index + 1 == offsets.len();
            // A blank sample adds nothing, so the sum starts at the first sample that draws, and
            // later blank samples count only towards the divisor.
            let blank = Some(sample.tex) == self.st.transparent;
            if blank && !last {
                continue;
            }
            if blank && sum.is_none() {
                return Ok(self.transparent(sample.domain.display_size()));
            }
            sum = Some(self.add_sample(sum, sample, blank, last, offsets.len() as u32)?);
        }
        let mut average = sum.expect("motion blur takes at least two samples");
        if let Some(Some((domain, bounds))) = shared
            && domain == average.domain
        {
            average.bounds = bounds;
        }
        Ok(average)
    }

    /// `sample` added to `sum`, over the union of their domains or over the sum's alone when the
    /// sample is `blank`. The sum is `motion_blur::SUM_FORMAT` until the `last` sample, which
    /// writes the average of all `samples` to the working space.
    fn add_sample(
        &mut self,
        sum: Option<Raster>,
        sample: Raster,
        blank: bool,
        last: bool,
        samples: u32,
    ) -> crate::Result<Raster> {
        use crate::node::processing::motion_blur as blur;

        let (display, data) = match sum {
            Some(sum) if blank => (sum.domain.display_size(), sum.domain.data_rect),
            Some(sum) => {
                let [held, shown] = [sum, sample].map(|raster| raster.domain.display_size());
                (
                    Size::new(held.width.max(shown.width), held.height.max(shown.height)),
                    sum.domain.data_rect.union(sample.domain.data_rect),
                )
            }
            None => (sample.domain.display_size(), sample.domain.data_rect),
        };
        let domain = quantized_domain(display, data);
        let origin = [domain.data_rect.x, domain.data_rect.y];
        let from = |raster: &Raster| {
            let at = raster.origin();
            [origin[0] - at[0], origin[1] - at[1]]
        };
        let params = blur::AccumulateParams {
            sum_offset: sum.as_ref().map_or([0, 0], from),
            sample_offset: from(&sample),
            has_sum: u32::from(sum.is_some()),
            samples,
            _pad: [0; 2],
        };
        let sum_tex = match sum {
            Some(sum) => sum.tex,
            None => self.unused_float(),
        };
        let size = domain.storage_size;
        let (entry, layout, binding, tex) = if last {
            (
                "resolve_main",
                blur::RESOLVE_LAYOUT,
                4,
                self.storage_target(size),
            )
        } else {
            let usage = TextureDesc::storage(size, blur::SUM_FORMAT).usage;
            let tex = self.rec.acquire(size, blur::SUM_FORMAT, usage);
            ("accumulate_main", blur::ACCUMULATE_LAYOUT, 3, tex)
        };
        let program = self.program("motion-blur", blur::SHADER, entry, layout)?;
        let uniform = self.rec.uniform(&params);
        self.rec.compute(
            program,
            &[
                (0, Res::Tex(sum_tex)),
                (1, Res::Tex(sample.tex)),
                (2, uniform),
                (binding, Res::Tex(tex)),
            ],
            dispatch_for(size),
        );
        Ok(Raster {
            tex,
            domain,
            bounds: None,
        })
    }

    /// How far `node` and everything it reads are static candidates. Walks its inputs depth first
    /// with a stack of its own, so a long chain of nodes can't run out of call stack.
    fn staticness(&mut self, node: &NodeKind) -> Staticness {
        let graph = self.graph_key();
        let mut pending = vec![node.id()];
        // Nodes whose inputs have been put on `pending`, which are decided when next reached.
        let mut entered = HashSet::default();
        while let Some(&id) = pending.last() {
            if self.st.statics.contains_key(&(graph, id)) {
                pending.pop();
                continue;
            }
            let candidate = self
                .scope
                .node(id)
                .filter(|node| !matches!(node, NodeKind::Clip(_) | NodeKind::TimeRemap(_)))
                .map(|node| (node, static_candidate(node)))
                .filter(|(_, own)| *own != Staticness::Never);
            let Some((node, own)) = candidate else {
                self.st.statics.insert((graph, id), Staticness::Never);
                pending.pop();
                continue;
            };
            let inputs = node.input_ports().into_iter().flatten();
            if entered.insert(id) {
                let undecided = inputs
                    .clone()
                    .filter(|input| !self.st.statics.contains_key(&(graph, *input)));
                let before = pending.len();
                pending.extend(undecided);
                if pending.len() > before {
                    continue;
                }
            }
            // An input still undecided here is part of a cycle, which validation rejects.
            let result = inputs.into_iter().fold(own, |all, input| {
                all.and(
                    self.st
                        .statics
                        .get(&(graph, input))
                        .copied()
                        .unwrap_or(Staticness::Never),
                )
            });
            // A `motion_blur` mixes its source at frames other than this one, where the values
            // its source is kept by aren't read.
            let result = match (node, result) {
                (NodeKind::MotionBlur(_), Staticness::WhileEqual) => Staticness::Never,
                _ => result,
            };
            self.st.statics.insert((graph, id), result);
            pending.pop();
        }
        self.st
            .statics
            .get(&(graph, node.id()))
            .copied()
            .unwrap_or(Staticness::Never)
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
        let content = Size::new(settings.width.max(1), settings.height.max(1));
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
        self.spatial_pass(
            "clip-crop",
            CLIP_CROP_SHADER,
            raster.tex,
            &offset,
            tex,
            size,
        )?;
        Ok(Raster {
            tex,
            domain,
            bounds: None,
        })
    }
}

/// Whether the node renders the same output on every frame as long as its inputs do.
fn static_candidate(node: &NodeKind) -> Staticness {
    let reads_frame = node.is_frame_dependent();
    let held_values = if reads_frame {
        Staticness::WhileEqual
    } else {
        Staticness::Always
    };
    match node {
        // An image changes only with the media store, which `images_changed` watches. Video
        // changes with the frame.
        NodeKind::MediaIn(media) => match media.params.kind {
            MediaInSourceKindDelegate::Image => held_values,
            _ => Staticness::Never,
        },
        // Shaders see the built-in time.
        NodeKind::WgslShader(_) => Staticness::Never,
        // A model playing a clip poses differently with the frame, until its clip is over or
        // before it starts, which `add_model` finds out from the clip.
        NodeKind::Model3d(model) if model.animates() => {
            if cfg!(feature = "3d") && !reads_frame {
                Staticness::WhileHeld
            } else {
                Staticness::Never
            }
        }
        // Per-copy and per-glyph params are evaluated in a scope of their own, which
        // `frame_values` doesn't evaluate them in.
        NodeKind::Instance3d(_) if reads_frame => Staticness::Never,
        NodeKind::Text(text) if reads_frame_in(&text.params.glyph) => Staticness::Never,
        // A `motion_blur` mixes its source at frames other than this one.
        NodeKind::MotionBlur(_) if reads_frame => Staticness::Never,
        _ => held_values,
    }
}

/// Whether any expression in `value` may give a different value at another frame.
fn reads_frame_in(value: &impl DelegateEvaluable) -> bool {
    let mut dependent = false;
    value.for_each_expression(&mut |expression| dependent |= expression.is_frame_dependent());
    dependent
}

/// The shutter angle of a node that blurs its own motion, evaluating its params at either end of
/// the shutter outside a `motion_blur` that samples.
fn analytic_shutter(node: &NodeKind) -> Option<&Deferred<f64>> {
    match node {
        NodeKind::Shape(shape) => Some(&shape.params.placement.motion_blur),
        NodeKind::Path(path) => Some(&path.params.placement.motion_blur),
        NodeKind::Text(text) => Some(&text.params.placement.motion_blur),
        NodeKind::Transform(transform) => Some(&transform.params.motion_blur),
        _ => None,
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
    Err(RenderError::NodeEvaluation {
        node_id: node.id(),
        node_kind: node.kind_name(),
        details: format!(
            "a {}x{} {what} is larger than this device's {max}-pixel texture limit",
            size.width, size.height
        ),
    }
    .into())
}

/// Records `node`, or fails when the textures it acquires itself are larger than the device
/// allows. The textures its inputs acquire are theirs to check.
fn record_node(node: &NodeKind, cx: &mut FrameCx<'_>) -> crate::Result<Raster> {
    let outer = cx.rec.replace_largest_acquired_texture(Size::new(0, 0));
    let raster = node
        .record(cx)
        .map_err(|error| locate_media_failure(node, &cx.scope, error));
    let largest = cx.rec.replace_largest_acquired_texture(outer);
    let raster = raster?;
    let max = cx.rec.device().limits().max_texture_dimension_2d;
    ensure_texture_fits(node, largest, cx.canvas_size(), max)?;
    Ok(raster)
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
    if named.is_none()
        && let NodeKind::Text(text) = node
    {
        named = crate::node::source::text::literal_font_families(text)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::MediaError;
    use crate::media::ResolvedMedia;

    /// Fails every source the way `failure` says.
    #[derive(Debug)]
    struct Failing(fn(&str) -> MediaError);

    impl MediaStore for Failing {
        fn resolve(&self, source: &str, _kind: AssetKind) -> Result<ResolvedMedia, MediaError> {
            Err(self.0(source))
        }
    }

    #[test]
    fn a_missing_source_that_starts_failing_another_way_counts_as_changed() {
        let changed = |store: &Failing, seen: Option<&MediaIdentity>| {
            source_changed(Some(store), AssetKind::Font, "Brand", seen)
        };
        let missing = Failing(|source| MediaError::source_not_found(source, "not served"));
        let unreadable = Failing(|source| MediaError::decode(source, "permission denied"));
        assert!(!changed(&missing, None));
        assert!(changed(&unreadable, None));
        let seen = MediaIdentity::of_key("brand");
        assert!(changed(&missing, Some(&seen)) && changed(&unreadable, Some(&seen)));
        assert!(!source_changed(None, AssetKind::Font, "Brand", None));
    }
}
