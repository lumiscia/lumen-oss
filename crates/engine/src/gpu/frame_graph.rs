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

use std::{
    collections::hash_map::Entry,
    sync::{Arc, Mutex},
};

use foldhash::{HashMap, HashSet};
use lumen_gpu::{
    Dispatch, LayoutEntry, LayoutKind, ProgramRef, Recorder, Res, SamplerRef, Size, TexRef,
    TextureDesc, TextureDomain, wgpu,
};

use crate::{
    composition::Composition,
    error::RenderError,
    expr::ExpressionContext,
    media::{
        CpuMediaFrame, MediaFrame, MediaStore, PlaneShape, YuvChroma, YuvFormat, YuvFrame,
        YuvPlanes, YuvUniform,
    },
    node::{
        Deferred, Node, NodeId, NodeKind,
        compositing::clip::Clip,
        required_input,
        source::text::{TextDraw, TextInstance},
    },
    scope::{FrameTime, Routed, Scope},
};

#[cfg(feature = "ffmpeg")]
use super::media::{GpuMediaFrameImporter, GpuMediaImports};
use super::{
    BoundFrame,
    params::{CropParams, copyable_texture_desc, dispatch_for},
    renderer::{MediaTextureCache, fit_frame_to_rgba8},
};

const RGBA8: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
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
const YUV_TO_RGBA_SHADER: &str = include_str!("shaders/yuv_to_rgba.wgsl");
const YUV_TO_RGBA_LAYOUT: &[LayoutEntry] = &[
    LayoutEntry::compute(0, LayoutKind::UintTexture),
    LayoutEntry::compute(1, LayoutKind::UintTexture),
    LayoutEntry::compute(2, LayoutKind::UintTexture),
    LayoutEntry::compute(3, LayoutKind::UintTexture),
    LayoutEntry::compute(4, LayoutKind::Uniform),
    LayoutEntry::compute(5, LayoutKind::StorageTexture(RGBA8)),
];
/// Frames a retained output stays without being read.
const RETAINED_IDLE_FRAMES: u64 = 600;
/// Bytes retained outputs may hold before the least recently used are dropped.
const RETAINED_BUDGET_BYTES: u64 = 1 << 30;
/// Distinct runtime shader sources kept, parsed and assembled.
const MAX_SHADERS: usize = 64;
/// Frames the import texture of a video source stays without being used.
#[cfg(feature = "ffmpeg")]
const VIDEO_IMPORT_IDLE_FRAMES: u64 = 600;

pub(crate) const RECORD_STORAGE_TEXTURE: LayoutKind = LayoutKind::StorageTexture(RGBA8);
pub(crate) const SPATIAL_LAYOUT: &[LayoutEntry] = &[
    LayoutEntry::compute(0, LayoutKind::Texture),
    LayoutEntry::compute(1, LayoutKind::Uniform),
    LayoutEntry::compute(2, LayoutKind::StorageTexture(RGBA8)),
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

    /// The program `FrameCx::yuv_to_rgba` records with.
    pub(crate) fn yuv_to_rgba(&mut self) -> crate::Result<()> {
        self.compute(
            "yuv-to-rgba",
            YUV_TO_RGBA_SHADER,
            "cs_main",
            YUV_TO_RGBA_LAYOUT,
        )
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
        label: &str,
        source: &str,
        inputs: usize,
        build: impl FnOnce() -> crate::Result<ShaderParts>,
    ) -> crate::Result<CachedShader> {
        let shader = self.st.cached_shader(source, inputs, build)?;
        dynamic_program(self.rec, label, &shader)?;
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

fn dynamic_program(
    rec: &mut Recorder,
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
    .map_err(gpu_error)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PathId(u32);

/// A node reached along one path of time remaps and clips, in one graph.
type InstanceKey = (usize, PathId, NodeId);
/// A node of one graph, wherever it is reached from.
type GraphNodeKey = (usize, NodeId);

struct Slot {
    raster: Raster,
    /// Passes recording it took, which each read of it skips.
    passes: u64,
    /// Its copy at full frame, once something has read it that way.
    full: Option<TexRef>,
    last_used: u64,
    bytes: u64,
}

struct ShaderEntry {
    shader: CachedShader,
    last_used: u64,
}

/// What a recorded shader needs, cached by its source.
#[derive(Clone)]
pub(crate) struct CachedShader {
    pub(crate) id: u64,
    pub(crate) interface: Arc<dyn std::any::Any + Send + Sync>,
    pub(crate) assembled: Arc<str>,
    pub(crate) layout: Arc<[LayoutEntry]>,
}

/// A plane of a CPU YUV frame, written to its texture before the frame's passes run.
struct YuvUpload {
    tex: TexRef,
    frame: Arc<YuvFrame>,
    offset: usize,
    stride: u32,
    shape: PlaneShape,
}

#[derive(Default)]
struct RecordState {
    fingerprint: Option<u64>,
    font_generation: Option<u64>,
    tick: u64,
    paths: HashMap<(PathId, NodeId), PathId>,
    memo: HashMap<InstanceKey, Raster>,
    full_frames: HashMap<(TexRef, u32, u32), Raster>,
    statics: HashMap<GraphNodeKey, bool>,
    slots: HashMap<GraphNodeKey, Slot>,
    /// Slots retained this frame, which hold nothing until it is submitted.
    pending: Vec<GraphNodeKey>,
    retained_bytes: u64,
    /// The textures of external targets, by the address of the texture.
    externals: HashMap<usize, TexRef>,
    texts: HashMap<InstanceKey, Arc<TextInstance>>,
    shaders: HashMap<(Arc<str>, usize), ShaderEntry>,
    next_shader: u64,
    transparent: Option<TexRef>,
    dummy_float: Option<TexRef>,
    atlas: Option<TexRef>,
    /// Adopted images by their texture, and the tick that last bound each.
    media: HashMap<usize, (TexRef, u64)>,
    video_uploads: Vec<(TexRef, Arc<CpuMediaFrame>, Size)>,
    yuv_uploads: Vec<YuvUpload>,
    /// The slot each video source's GPU frames are imported into, and when it was last used.
    #[cfg(feature = "ffmpeg")]
    video_imports: HashMap<InstanceKey, (u32, u64)>,
    #[cfg(feature = "ffmpeg")]
    next_video_import: u32,
    output: Option<Raster>,
    /// The font resolver of each family text names, which retained text was drawn with.
    fonts: HashMap<String, Option<String>>,
    /// Passes reads of retained outputs skipped.
    skipped_passes: u64,
}

pub(crate) struct FrameGraph {
    pub(crate) recorder: Recorder,
    state: RecordState,
    /// Bytes of retained outputs kept once frames stop using them.
    retained_budget: u64,
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
    pub(crate) full_frame_sources: bool,
    pub(crate) retain_static: bool,
    pub(crate) media_cache: &'a mut MediaTextureCache,
    pub(crate) text_atlas: &'a Arc<Mutex<lumen_text::GlyphAtlas>>,
    pub(crate) atlas_texture: &'a mut Option<Arc<wgpu::Texture>>,
    pub(crate) max_rebinds: usize,
    /// The renderer whose device GPU video frames were decoded on.
    #[cfg(feature = "ffmpeg")]
    pub(crate) renderer: &'a lumen_gpu::Renderer,
    #[cfg(feature = "ffmpeg")]
    pub(crate) importer: &'a mut GpuMediaFrameImporter,
}

impl RecordState {
    fn cached_shader(
        &mut self,
        source: &str,
        inputs: usize,
        build: impl FnOnce() -> crate::Result<ShaderParts>,
    ) -> crate::Result<CachedShader> {
        let tick = self.tick;
        let key = (Arc::<str>::from(source), inputs);
        let cached = self.shaders.len();
        let entry = match self.shaders.entry(key) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(vacant) => {
                let (interface, assembled, layout) = build()?;
                self.next_shader += 1;
                vacant.insert(ShaderEntry {
                    shader: CachedShader {
                        id: self.next_shader,
                        interface,
                        assembled: assembled.into(),
                        layout: layout.into(),
                    },
                    last_used: tick,
                })
            }
        };
        entry.last_used = tick;
        let shader = entry.shader.clone();
        // Only a new source evicts, and not what this frame used: a frame using more than the limit
        // keeps them all, rather than compile again each frame.
        if self.shaders.len() > cached && self.shaders.len() > MAX_SHADERS {
            let oldest = self
                .shaders
                .iter()
                .filter(|(_, entry)| entry.last_used < tick)
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.shaders.remove(&oldest);
            }
        }
        Ok(shader)
    }
}

impl FrameGraph {
    pub(crate) fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self {
            recorder: Recorder::new(device, queue),
            state: RecordState::default(),
            retained_budget: RETAINED_BUDGET_BYTES,
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
            full_frame_sources,
            retain_static,
            media_cache,
            text_atlas,
            atlas_texture,
            max_rebinds,
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
            if self.state.font_generation != generation || self.fonts_changed(media) {
                self.drop_slots();
                self.state.font_generation = generation;
            }
            self.state.tick += 1;
            self.state.memo.clear();
            self.state.full_frames.clear();
            self.state.video_uploads.clear();
            self.state.yuv_uploads.clear();
            self.recorder.begin_frame();
            let mut cx = FrameCx {
                rec: &mut self.recorder,
                st: &mut self.state,
                media_cache: &mut *media_cache,
                composition,
                scope: Scope::root(composition),
                frame: frame.into(),
                path: PathId(0),
                media,
                output_format,
                full_frame_sources,
                retain_static,
                static_depth: 0,
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
                return Ok(bound);
            }
            if rebinds == max_rebinds {
                return Err(RenderError::Gpu {
                    details: format!(
                        "fonts kept loading while frame {frame} was recorded; its text bounds could not settle"
                    ),
                }
                .into());
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
        for key in &self.state.pending {
            let slot = self.state.slots.get_mut(key).expect("pending slot");
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
        self.state.video_uploads.clear();
        self.state.yuv_uploads.clear();
        self.state.output = None;
        let tick = self.state.tick;
        let recorder = &mut self.recorder;
        self.state.media.retain(|_, (tex, last_used)| {
            let kept = *last_used != tick;
            if !kept {
                recorder.forget(*tex);
            }
            kept
        });
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
            node_programs(node, &mut programs)?;
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
        for (tex, frame, size) in self.state.video_uploads.drain(..) {
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
        for upload in self.state.yuv_uploads.drain(..) {
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
        self.recorder.upload();
        self.forget_unbound_media();
        Ok(())
    }

    /// The media cache frees what only it holds, so images this frame doesn't bind are let go.
    fn forget_unbound_media(&mut self) {
        let tick = self.state.tick;
        let recorder = &mut self.recorder;
        self.state.media.retain(|_, (tex, last_used)| {
            let kept = *last_used == tick;
            if !kept {
                recorder.forget(*tex);
            }
            kept
        });
    }

    /// Commits what the frame retained after submitting its passes, and sweeps what idled.
    pub(crate) fn finish_frame(&mut self) {
        self.state.pending.clear();
        self.evict_slots();
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
        self.state.media.retain(|_, (tex, _)| unheld(tex));
        self.state.externals.retain(|_, tex| unheld(tex));
        #[cfg(feature = "ffmpeg")]
        {
            let tick = self.state.tick;
            let idle_before = tick.saturating_sub(VIDEO_IMPORT_IDLE_FRAMES);
            let imports = &mut self.imports;
            self.state.video_imports.retain(|_, (slot, last_used)| {
                let kept = *last_used > idle_before;
                if !kept {
                    imports.remove(*slot);
                }
                kept
            });
        }
    }

    pub(crate) fn output(&self) -> Option<Raster> {
        self.state.output
    }

    /// The domain `node` of the root graph had in the frame last recorded.
    pub(crate) fn node_domain(
        &self,
        composition: &Composition,
        node: NodeId,
    ) -> Option<TextureDomain> {
        let graph = std::ptr::from_ref(composition.graph()) as usize;
        let recorded =
            self.state.memo.iter().find_map(|(key, raster)| {
                (key.0 == graph && key.2 == node).then_some(raster.domain)
            });
        recorded.or_else(|| Some(self.state.slots.get(&(graph, node))?.raster.domain))
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
        self.drop_slots();
        // A caller may hold the last frame of what is being replaced, so nothing writes it again.
        if let Some(output) = self.state.output {
            self.recorder.disown(output.tex);
        }
        let state = &mut self.state;
        state.fingerprint = None;
        state.video_uploads.clear();
        state.yuv_uploads.clear();
        #[cfg(feature = "ffmpeg")]
        {
            state.video_imports.clear();
            self.imports = GpuMediaImports::default();
        }
        for (_, (tex, _)) in state.media.drain() {
            self.recorder.forget(tex);
        }
        for (_, tex) in state.externals.drain() {
            self.recorder.forget(tex);
        }
        state.statics.clear();
        state.texts.clear();
        state.shaders.clear();
        state.memo.clear();
        state.full_frames.clear();
        state.output = None;
        if let Some(atlas) = state.atlas.take() {
            self.recorder.forget(atlas);
        }
    }

    /// Frees everything recording holds, as `reset` alone keeps its pool and programs for the next
    /// composition.
    pub(crate) fn release(&mut self) {
        self.reset();
        self.drop_pending();
        self.state.transparent = None;
        self.state.dummy_float = None;
        self.recorder.release_all();
    }

    /// Whether a family text named now resolves differently. Retained text doesn't shape again,
    /// so this is what notices a font arriving for it.
    fn fonts_changed(&mut self, media: Option<&dyn MediaStore>) -> bool {
        let mut changed = false;
        for (family, resolver) in &mut self.state.fonts {
            let current = media.and_then(|media| media.font_resolver_id(family));
            if *resolver != current {
                *resolver = current;
                changed = true;
            }
        }
        changed
    }

    pub(crate) fn set_retained_budget(&mut self, bytes: u64) {
        self.retained_budget = bytes;
    }

    pub(crate) fn skipped_passes(&self) -> u64 {
        self.state.skipped_passes
    }

    fn release_slot(recorder: &mut Recorder, state: &mut RecordState, key: GraphNodeKey) {
        if let Some(slot) = state.slots.remove(&key) {
            recorder.release(slot.raster.tex);
            if let Some(full) = slot.full {
                recorder.release(full);
            }
            state.retained_bytes -= slot.bytes;
        }
    }

    fn drop_pending(&mut self) {
        for key in std::mem::take(&mut self.state.pending) {
            Self::release_slot(&mut self.recorder, &mut self.state, key);
        }
    }

    fn drop_slots(&mut self) {
        self.state.pending.clear();
        let keys: Vec<_> = self.state.slots.keys().copied().collect();
        for key in keys {
            Self::release_slot(&mut self.recorder, &mut self.state, key);
        }
    }

    fn evict_slots(&mut self) {
        let tick = self.state.tick;
        let idle_before = tick.saturating_sub(RETAINED_IDLE_FRAMES);
        let over_budget = self.state.retained_bytes > self.retained_budget;
        if !over_budget && !tick.is_multiple_of(60) {
            return;
        }
        let mut idle: Vec<_> = self
            .state
            .slots
            .iter()
            .filter(|(_, slot)| slot.last_used != tick)
            .map(|(key, slot)| (slot.last_used, *key))
            .collect();
        idle.sort_unstable_by_key(|(last_used, _)| *last_used);
        for (last_used, key) in idle {
            if last_used > idle_before && self.state.retained_bytes <= self.retained_budget {
                break;
            }
            Self::release_slot(&mut self.recorder, &mut self.state, key);
        }
    }

    pub(crate) fn retained_bytes(&self) -> u64 {
        self.state.retained_bytes
    }
}

fn font_generation(media: Option<&dyn MediaStore>) -> Option<u64> {
    crate::node::source::text::text_system(media)
        .ok()
        .map(|text_system| text_system.font_generation())
}

/// A raster's domain within `display`, rounded out to a size class so that bounds moving from
/// frame to frame keep finding the same textures, and the bind groups cached on them.
pub(crate) fn quantized_domain(display: Size, data: lumen_gpu::RectI) -> TextureDomain {
    let domain = TextureDomain::bounded(display, data);
    let rect = domain.data_rect;
    if data.is_empty() {
        return domain;
    }
    let class = |extent: u32, limit: u32| {
        let step = match extent {
            0..256 => 16,
            256..512 => 32,
            512..1024 => 64,
            _ => 128,
        };
        extent.next_multiple_of(step).min(limit.max(1))
    };
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
    media: Option<&'a dyn MediaStore>,
    output_format: wgpu::TextureFormat,
    full_frame_sources: bool,
    retain_static: bool,
    /// How many static nodes the walk is inside, whose readers retain them rather than this.
    static_depth: u32,
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

fn gpu_error(error: impl std::fmt::Display) -> crate::error::LumenError {
    RenderError::Gpu {
        details: error.to_string(),
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
        text.bind(expr, self.media, &mut self.bound, self.rec)
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

    pub(crate) fn frame_number(&self) -> u32 {
        self.frame.value() as u32
    }

    pub(crate) fn frame_time(&self) -> FrameTime {
        self.frame
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
        let is_static = self.retain_static && self.is_static(node);
        if is_static && let Some(slot) = self.st.slots.get_mut(&(graph, node_id)) {
            slot.last_used = self.st.tick;
            self.st.skipped_passes += slot.passes;
            return Ok(slot.raster);
        }
        let key = (graph, self.path, node_id);
        let raster = match self.st.memo.get(&key) {
            Some(raster) => *raster,
            None => {
                let passes = self.rec.pass_count();
                self.static_depth += u32::from(is_static);
                let raster = record_node(node, self);
                self.static_depth -= u32::from(is_static);
                let raster = raster?;
                self.st.memo.insert(key, raster);
                if is_static && self.static_depth == 0 {
                    let passes = (self.rec.pass_count() - passes) as u64;
                    self.retain(graph, node_id, raster, passes);
                }
                raster
            }
        };
        Ok(raster)
    }

    fn retain(&mut self, graph: usize, node: NodeId, raster: Raster, passes: u64) {
        self.rec.retain(raster.tex);
        let bytes = self.rec.texture_bytes(raster.tex);
        self.st.retained_bytes += bytes;
        self.st.slots.insert(
            (graph, node),
            Slot {
                raster,
                passes,
                full: None,
                last_used: self.st.tick,
                bytes,
            },
        );
        self.st.pending.push((graph, node));
    }

    /// Records `node_id` stored at its full display size, for readers that read texel for texel.
    pub(crate) fn input_full(&mut self, node_id: NodeId) -> crate::Result<Raster> {
        let raster = self.input(node_id)?;
        if raster.domain.is_full_frame() {
            return Ok(raster);
        }
        let slot_key = (self.graph_key(), node_id);
        if let Some(slot) = self.st.slots.get(&slot_key)
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
            .slots
            .get(&slot_key)
            .is_some_and(|slot| slot.raster.tex == raster.tex)
        {
            self.rec.retain(full.tex);
            let bytes = self.rec.texture_bytes(full.tex);
            self.st.retained_bytes += bytes;
            let slot = self.st.slots.get_mut(&slot_key).expect("slot");
            slot.full = Some(full.tex);
            slot.bytes += bytes;
            if !self.st.pending.contains(&slot_key) {
                self.st.pending.push(slot_key);
            }
        }
        Ok(full)
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
            .acquire(size, RGBA8, copyable_texture_desc(size).usage);
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
            .acquire(size, RGBA8, copyable_texture_desc(size).usage);
        self.rec.clear(tex);
        tex
    }

    pub(crate) fn storage_target(&mut self, size: Size) -> TexRef {
        self.rec
            .acquire(size, RGBA8, TextureDesc::storage(size, RGBA8).usage)
    }

    pub(crate) fn float_target(&mut self, size: Size) -> TexRef {
        let format = wgpu::TextureFormat::Rgba16Float;
        self.rec
            .acquire(size, format, TextureDesc::storage(size, format).usage)
    }

    pub(crate) fn render_target(&mut self, size: Size) -> TexRef {
        self.rec
            .acquire(size, RGBA8, TextureDesc::render_target(size, RGBA8).usage)
    }

    pub(crate) fn output_target(&mut self, size: Size, format: wgpu::TextureFormat) -> TexRef {
        let usage = if format == RGBA8 {
            copyable_texture_desc(size).usage
        } else {
            wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
        };
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
                    format: RGBA8,
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

    /// A one-texel float texture for gaussian levels a pass binds but does not read.
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
                .map_err(|_| RenderError::Gpu {
                    details: "text atlas lock was poisoned".to_string(),
                })?
                .begin_frame();
            self.bound.text_atlas = Some(Arc::clone(self.text_atlas));
        }
        if let Some(atlas) = self.st.atlas {
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
        self.st.atlas = Some(atlas);
        Ok(atlas)
    }

    /// Remembers the resolver of a family text names.
    pub(crate) fn note_font(&mut self, family: &str) {
        if !self.st.fonts.contains_key(family) {
            let resolver = self.media.and_then(|media| media.font_resolver_id(family));
            self.st.fonts.insert(family.to_string(), resolver);
        }
    }

    /// The text instance's state across frames, built once.
    pub(crate) fn text_instance(
        &mut self,
        node_id: NodeId,
        build: impl FnOnce(&Self) -> crate::Result<TextInstance>,
    ) -> crate::Result<Arc<TextInstance>> {
        let key = (self.graph_key(), self.path, node_id);
        if let Some(text) = self.st.texts.get(&key) {
            return Ok(Arc::clone(text));
        }
        let text = Arc::new(build(self)?);
        self.st.texts.insert(key, Arc::clone(&text));
        Ok(text)
    }

    /// The parsed and assembled form of a runtime shader source, built once per source.
    pub(crate) fn cached_shader(
        &mut self,
        source: &str,
        inputs: usize,
        build: impl FnOnce() -> crate::Result<ShaderParts>,
    ) -> crate::Result<CachedShader> {
        self.st.cached_shader(source, inputs, build)
    }

    pub(crate) fn dynamic_program(
        &mut self,
        label: &str,
        shader: &CachedShader,
    ) -> crate::Result<ProgramRef> {
        dynamic_program(self.rec, label, shader)
    }

    /// An image's texture, uploaded once and kept in the renderer's media cache.
    pub(crate) fn image_texture(
        &mut self,
        key: super::MediaTextureKey,
        frame: &MediaFrame,
        size: Size,
    ) -> crate::Result<TexRef> {
        let cpu = match frame {
            MediaFrame::CpuRgba(cpu) => cpu,
            MediaFrame::Yuv(_) => {
                return Err(RenderError::Gpu {
                    details: "an image resolver returned a YUV frame".to_string(),
                }
                .into());
            }
            #[cfg(feature = "ffmpeg")]
            MediaFrame::GpuVideo(_) => {
                return Err(RenderError::Gpu {
                    details: "an image resolver returned a GPU frame".to_string(),
                }
                .into());
            }
        };
        let texture = match self.media_cache.get(&key) {
            Some(texture) => texture,
            None => {
                let texture = super::renderer::create_media_texture(self.rec.device(), size);
                let rgba = fit_frame_to_rgba8(cpu, size.width, size.height);
                self.rec.queue().write_texture(
                    texture.as_image_copy(),
                    &rgba,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(size.width * 4),
                        rows_per_image: Some(size.height),
                    },
                    size.as_extent(),
                );
                self.media_cache.insert(key, Arc::clone(&texture));
                texture
            }
        };
        Ok(self.adopted_media(texture))
    }

    fn adopted_media(&mut self, texture: Arc<wgpu::Texture>) -> TexRef {
        let rec = &mut *self.rec;
        let tick = self.st.tick;
        let (tex, last_used) = self
            .st
            .media
            .entry(Arc::as_ptr(&texture) as usize)
            .or_insert_with(|| (rec.adopt(texture), tick));
        *last_used = tick;
        *tex
    }

    /// A pooled texture a CPU video frame is written to before the frame's passes run.
    fn cpu_video_texture(&mut self, frame: Arc<CpuMediaFrame>, size: Size) -> TexRef {
        let tex =
            self.rec
                .acquire_written_early(size, RGBA8, TextureDesc::sampled(size, RGBA8).usage);
        self.st.video_uploads.push((tex, frame, size));
        tex
    }

    /// The texture of a frame decoded on the CPU, either as RGBA or as YUV planes.
    fn cpu_frame_texture(&mut self, frame: MediaFrame, size: Size) -> crate::Result<TexRef> {
        match frame {
            MediaFrame::CpuRgba(frame) => Ok(self.cpu_video_texture(frame, size)),
            MediaFrame::Yuv(frame) => self.yuv_video_texture(frame, size),
            #[cfg(feature = "ffmpeg")]
            MediaFrame::GpuVideo(_) => Err(RenderError::Gpu {
                details: "a CPU frame was expected, not a GPU one".to_string(),
            }
            .into()),
        }
    }

    /// Writes a CPU YUV frame's planes to pooled textures before the frame's passes run, and
    /// converts them to RGBA.
    fn yuv_video_texture(&mut self, frame: Arc<YuvFrame>, size: Size) -> crate::Result<TexRef> {
        frame.validate().map_err(|details| RenderError::Gpu {
            details: format!("a YUV frame can't be read: {details}"),
        })?;
        let max = self.rec.device().limits().max_texture_dimension_2d;
        if frame.format.width > max || frame.format.height > max {
            return Err(RenderError::Gpu {
                details: format!(
                    "a {}x{} YUV frame is larger than this device's {max}-pixel texture limit",
                    frame.format.width, frame.format.height
                ),
            }
            .into());
        }
        let wide = frame.format.bytes_per_sample() == 2;
        let rec = &mut *self.rec;
        let uploads = &mut self.st.yuv_uploads;
        let planes = frame.layout.map_with_shape(&frame.format, |plane, shape| {
            let format = match (shape.channels, wide) {
                (1, false) => wgpu::TextureFormat::R8Uint,
                (_, false) => wgpu::TextureFormat::Rg8Uint,
                (1, true) => wgpu::TextureFormat::R16Uint,
                (_, true) => wgpu::TextureFormat::Rg16Uint,
            };
            let plane_size = Size::new(shape.width, shape.height);
            let tex = rec.acquire_written_early(
                plane_size,
                format,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            );
            uploads.push(YuvUpload {
                tex,
                frame: Arc::clone(&frame),
                offset: plane.offset,
                // `validate` rejects strides that don't fit.
                stride: plane.stride as u32,
                shape,
            });
            tex
        });
        self.yuv_to_rgba(&planes, &frame.format, size)
    }

    /// Converts YUV planes into a premultiplied RGBA8 texture of `size`, showing the frame
    /// `format` describes upright and stretched to it by nearest neighbour. Plane textures hold
    /// unsigned-integer samples (`R8Uint`, `Rg8Uint` for interleaved chroma, or their 16-bit forms)
    /// with the frame at their top left, and any texture this frame's passes can read will do:
    /// pooled ones written before the passes, or adopted ones a decoder filled on the GPU.
    pub(crate) fn yuv_to_rgba(
        &mut self,
        planes: &YuvPlanes<TexRef>,
        format: &YuvFormat,
        size: Size,
    ) -> crate::Result<TexRef> {
        let program = self.program(
            "yuv-to-rgba",
            YUV_TO_RGBA_SHADER,
            "cs_main",
            YUV_TO_RGBA_LAYOUT,
        )?;
        let (u, v, interleaved) = match planes.chroma {
            YuvChroma::Interleaved(chroma) => (chroma, chroma, true),
            YuvChroma::Planar { u, v } => (u, v, false),
        };
        // An absent alpha plane binds luma in its place, which the shader doesn't read.
        let alpha = planes.alpha.unwrap_or(planes.luma);
        let uniform = self.rec.uniform(&YuvUniform::new(
            format,
            interleaved,
            planes.alpha.is_some(),
        ));
        let target = self.storage_target(size);
        self.rec.compute(
            program,
            &[
                (0, Res::Tex(planes.luma)),
                (1, Res::Tex(u)),
                (2, Res::Tex(v)),
                (3, Res::Tex(alpha)),
                (4, uniform),
                (5, Res::Tex(target)),
            ],
            dispatch_for(size),
        );
        Ok(target)
    }

    /// The texture a video frame shows through. A GPU frame is imported now, ahead of this frame's
    /// submission in queue order, which settles its source on the GPU. One the renderer can't use
    /// moves its source to the CPU, unless the source has already imported a GPU frame, even earlier
    /// in this output frame, which fails the render instead.
    pub(crate) fn video_texture(
        &mut self,
        #[cfg_attr(not(feature = "ffmpeg"), allow(unused_variables))] node_id: NodeId,
        frame: MediaFrame,
        size: Size,
    ) -> crate::Result<TexRef> {
        match frame {
            frame @ (MediaFrame::CpuRgba(_) | MediaFrame::Yuv(_)) => {
                self.cpu_frame_texture(frame, size)
            }
            #[cfg(feature = "ffmpeg")]
            MediaFrame::GpuVideo(frame) => {
                let key = (self.graph_key(), self.path, node_id);
                let tick = self.st.tick;
                let next = &mut self.st.next_video_import;
                let slot = self.st.video_imports.entry(key).or_insert_with(|| {
                    *next += 1;
                    (*next, tick)
                });
                slot.1 = tick;
                let slot = slot.0;
                let imported = self
                    .importer
                    .import(self.renderer, self.imports, slot, &frame)
                    .and_then(|texture| {
                        if frame.settle_on_gpu() {
                            Ok(texture)
                        } else {
                            Err("its source moved to the CPU".to_string())
                        }
                    });
                match imported {
                    Ok(texture) => Ok(self.adopted_media(texture)),
                    Err(reason) => {
                        let cpu = frame.decode_on_cpu(&reason).map_err(|error| RenderError::Gpu {
                            details: format!(
                                "failed to import GPU media frame: {reason}; decoding it on the CPU also failed: {error}"
                            ),
                        })?;
                        self.cpu_frame_texture(cpu, size)
                    }
                }
            }
        }
    }

    fn is_static(&mut self, node: &NodeKind) -> bool {
        let key = (self.graph_key(), node.id());
        if let Some(known) = self.st.statics.get(&key) {
            return *known;
        }
        self.st.statics.insert(key, false);
        let result = is_static_candidate(node)
            && !matches!(node, NodeKind::Clip(_) | NodeKind::TimeRemap(_))
            && node.input_ports().into_iter().flatten().all(|input| {
                self.scope
                    .node(input)
                    .is_some_and(|input| self.is_static(input))
            });
        self.st.statics.insert(key, result);
        result
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
        let settings = &routed.scope.scene.render_settings;
        let content = Size::new(settings.width.max(1), settings.height.max(1));
        let raster = self.record_routed(routed)?;
        let canvas = self.canvas_size();
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
fn is_static_candidate(node: &NodeKind) -> bool {
    match node {
        // Media changes with the frame and the media store, shaders see the built-in time, and
        // the output is written into external textures that replace its own.
        NodeKind::MediaIn(_) | NodeKind::WgslShader(_) | NodeKind::MediaOutput(_) => false,
        _ => !node.has_expressions(),
    }
}

fn node_programs(node: &NodeKind, programs: &mut Programs<'_>) -> crate::Result<()> {
    match node {
        NodeKind::MediaIn(node) => node.programs(programs),
        NodeKind::Background(node) => node.programs(programs),
        NodeKind::Text(node) => node.programs(programs),
        NodeKind::Path(node) => node.programs(programs),
        NodeKind::Shape(node) => node.programs(programs),
        NodeKind::Boolean(node) => node.programs(programs),
        NodeKind::Merge(node) => node.programs(programs),
        NodeKind::RasterMultiMerge(node) => node.programs(programs),
        NodeKind::Blur(node) => node.programs(programs),
        NodeKind::ColorGrade(node) => node.programs(programs),
        NodeKind::Exposure(node) => node.programs(programs),
        NodeKind::HueSaturation(node) => node.programs(programs),
        NodeKind::Levels(node) => node.programs(programs),
        NodeKind::Opacity(node) => node.programs(programs),
        NodeKind::TimeRemap(_) => Ok(()),
        NodeKind::Transform(node) => node.programs(programs),
        NodeKind::Crop(node) => node.programs(programs),
        NodeKind::Resize(node) => node.programs(programs),
        NodeKind::Shadow(node) => node.programs(programs),
        NodeKind::WgslShader(node) => node.programs(programs),
        NodeKind::Switch(node) => node.programs(programs),
        NodeKind::Clip(_) => programs.spatial("clip-crop", CLIP_CROP_SHADER),
        NodeKind::MediaOutput(node) => node.programs(programs),
    }
}

fn record_node(node: &NodeKind, cx: &mut FrameCx<'_>) -> crate::Result<Raster> {
    match node {
        NodeKind::MediaIn(node) => node.record(cx),
        NodeKind::Background(node) => node.record(cx),
        NodeKind::Text(node) => node.record(cx),
        NodeKind::Path(node) => node.record(cx),
        NodeKind::Shape(node) => node.record(cx),
        NodeKind::Boolean(node) => node.record(cx),
        NodeKind::Merge(node) => node.record(cx),
        NodeKind::RasterMultiMerge(node) => node.record(cx),
        NodeKind::Blur(node) => node.record(cx),
        NodeKind::ColorGrade(node) => node.record(cx),
        NodeKind::Exposure(node) => node.record(cx),
        NodeKind::HueSaturation(node) => node.record(cx),
        NodeKind::Levels(node) => node.record(cx),
        NodeKind::Opacity(node) => node.record(cx),
        NodeKind::TimeRemap(node) => cx.record_time_remap(node),
        NodeKind::Transform(node) => node.record(cx),
        NodeKind::Crop(node) => node.record(cx),
        NodeKind::Resize(node) => node.record(cx),
        NodeKind::Shadow(node) => node.record(cx),
        NodeKind::WgslShader(node) => node.record(cx),
        NodeKind::Switch(node) => node.record(cx),
        NodeKind::Clip(node) => cx.record_clip(node),
        NodeKind::MediaOutput(node) => node.record(cx),
    }
}
