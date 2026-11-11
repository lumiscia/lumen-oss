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
    error::{LumenError, RenderError},
    expr::ExpressionContext,
    media::{
        CpuMediaFrame, MediaFrame, MediaStore, PlaneShape, YuvChroma, YuvFormat, YuvFrame,
        YuvPlanes, YuvUniform, premultiply_rgba_in_place_if_needed,
    },
    node::{
        Deferred, MediaValue, Node, NodeId, NodeKind,
        compositing::clip::Clip,
        required_input,
        source::{
            media_in::{AlphaMode, MediaInSourceKindDelegate},
            text::{TextDraw, TextInstance},
        },
    },
    scope::{FrameTime, MAX_GRAPH_DEPTH, Routed, Scope},
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
/// A node reached along one path, in one sample of the `motion_blur` that path samples, or in
/// sample 0 outside one.
type MemoKey = (InstanceKey, u32);
/// A video frame a `media_in` showed this frame: the node's instance, the ID of the resolver it
/// read, the source frame, its size and whether its colour is premultiplied.
type VideoFrameKey = (InstanceKey, String, u32, (u32, u32), bool);

struct Slot {
    raster: Raster,
    /// Passes recording it took, which each read of it skips.
    passes: u64,
    /// Its copy at full frame, once something has read it that way.
    full: Option<TexRef>,
    last_used: u64,
    bytes: u64,
    /// The models and environment maps it drew, which the scene cache keeps while it is kept.
    #[cfg(feature = "3d")]
    models: Vec<String>,
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

/// A CPU RGBA video frame, written to its texture before the frame's passes run.
struct VideoUpload {
    tex: TexRef,
    frame: Arc<CpuMediaFrame>,
    size: Size,
    /// The pixels' colour isn't multiplied by alpha yet.
    premultiply: bool,
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
    memo: HashMap<MemoKey, Raster>,
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
    dummy_uint: Option<TexRef>,
    atlas: Option<TexRef>,
    /// Adopted images by their texture, and the tick that last bound each.
    media: HashMap<usize, (TexRef, u64)>,
    video_uploads: Vec<VideoUpload>,
    yuv_uploads: Vec<YuvUpload>,
    /// The texture of each video frame shown this frame, which samples of a `motion_blur` landing
    /// on the same source frame share.
    video_frames: HashMap<VideoFrameKey, TexRef>,
    /// The slot each video source's GPU frames are imported into, by the source and how many
    /// distinct frames it imported earlier in the frame, and when it was last used.
    #[cfg(feature = "ffmpeg")]
    video_imports: HashMap<(InstanceKey, u32), (u32, u64)>,
    /// Distinct GPU frames each video source has imported this frame.
    #[cfg(feature = "ffmpeg")]
    frame_imports: HashMap<InstanceKey, u32>,
    #[cfg(feature = "ffmpeg")]
    next_video_import: u32,
    output: Option<Raster>,
    /// The font resolver of each family text names, which retained text was drawn with.
    fonts: HashMap<String, Option<String>>,
    /// The texture of each image source that retained output was drawn from.
    images: HashMap<String, Option<super::MediaTextureKey>>,
    /// The template, node and family of each font fallback reported for this composition.
    font_fallbacks: HashSet<(Option<String>, NodeId, String)>,
    /// The template and `render_3d` node of each shadow limit reported for this composition.
    #[cfg(feature = "3d")]
    shadow_limits: HashSet<(Option<String>, NodeId)>,
    /// The template and `text_3d` node of each report of glyphs without outlines for this
    /// composition.
    #[cfg(feature = "3d")]
    outline_gaps: HashSet<(Option<String>, NodeId)>,
    /// Warnings not yet taken.
    diagnostics: Vec<crate::diagnostic::Diagnostic>,
    /// Passes reads of retained outputs skipped.
    skipped_passes: u64,
    #[cfg(feature = "3d")]
    scene: crate::node::scene::collect::SceneCache,
    /// Models drawn while the static output being recorded records, which its slot keeps.
    #[cfg(feature = "3d")]
    static_models: Vec<String>,
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
    pub(crate) single_tile_repeats: bool,
    pub(crate) retain_static: bool,
    pub(crate) media_cache: &'a mut MediaTextureCache,
    pub(crate) text_atlas: &'a Arc<Mutex<lumen_text::GlyphAtlas>>,
    pub(crate) atlas_texture: &'a mut Option<Arc<wgpu::Texture>>,
    pub(crate) max_rebinds: usize,
    pub(crate) max_text_glyphs: usize,
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
            single_tile_repeats,
            retain_static,
            media_cache,
            text_atlas,
            atlas_texture,
            max_rebinds,
            max_text_glyphs,
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
            let images_changed = self.images_changed(media);
            if self.state.font_generation != generation
                || self.fonts_changed(media)
                || images_changed
            {
                self.drop_slots();
                self.state.font_generation = generation;
            }
            self.state.tick += 1;
            self.state.memo.clear();
            self.state.full_frames.clear();
            self.state.video_uploads.clear();
            self.state.yuv_uploads.clear();
            self.state.video_frames.clear();
            #[cfg(feature = "ffmpeg")]
            self.state.frame_imports.clear();
            #[cfg(feature = "3d")]
            self.state.static_models.clear();
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
                full_frame_sources,
                single_tile_repeats,
                retain_static,
                static_depth: 0,
                depth: 0,
                bound: BoundFrame::new(),
                max_text_glyphs,
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
                #[cfg(feature = "3d")]
                self.state.scene.trim(
                    &mut self.recorder,
                    self.state.tick,
                    self.state.slots.values().flat_map(|slot| &slot.models),
                );
                #[cfg(feature = "3d")]
                self.state.scene.trim_environments(
                    &mut self.recorder,
                    self.state.tick,
                    self.state.slots.values().flat_map(|slot| &slot.models),
                );
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
        for VideoUpload {
            tex,
            frame,
            size,
            premultiply,
        } in self.state.video_uploads.drain(..)
        {
            let mut rgba = fit_frame_to_rgba8(&frame, size.width, size.height);
            if premultiply {
                premultiply_rgba_in_place_if_needed(rgba.to_mut());
            }
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
        self.recorder.upload().map_err(gpu_error)?;
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
        let recorded = self.state.memo.iter().find_map(|((key, _), raster)| {
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
        state.video_frames.clear();
        #[cfg(feature = "ffmpeg")]
        {
            state.video_imports.clear();
            state.frame_imports.clear();
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
        state.paths.clear();
        state.fonts.clear();
        state.font_fallbacks.clear();
        #[cfg(feature = "3d")]
        state.shadow_limits.clear();
        #[cfg(feature = "3d")]
        state.outline_gaps.clear();
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
        self.state.dummy_uint = None;
        #[cfg(feature = "3d")]
        self.state.scene.release(&mut self.recorder);
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

    /// Whether an image source retained output was drawn from now resolves to another texture, as
    /// when a host replaces the image under the same source or with one of another size.
    fn images_changed(&mut self, media: Option<&dyn MediaStore>) -> bool {
        let mut changed = false;
        for (source, key) in &mut self.state.images {
            let current = media
                .and_then(|media| media.get_image_resolver(source).ok())
                .map(|resolver| super::MediaTextureKey::of_image(&*resolver));
            if *key != current {
                *key = current;
                changed = true;
            }
        }
        changed
    }

    /// The warnings recording has raised since the last call.
    pub(crate) fn take_diagnostics(&mut self) -> Vec<crate::diagnostic::Diagnostic> {
        std::mem::take(&mut self.state.diagnostics)
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
        self.state.images.clear();
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
    full_frame_sources: bool,
    /// Whether repeats draw from one tile, as `set_single_tile_repeats` sets.
    pub(crate) single_tile_repeats: bool,
    retain_static: bool,
    /// How many static nodes the walk is inside, whose readers retain them rather than this.
    static_depth: u32,
    /// How many nodes the walk is recording inside.
    depth: u32,
    pub(crate) bound: BoundFrame,
    max_text_glyphs: usize,
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
        if self.st.shadow_limits.contains(&key) {
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
        self.st.shadow_limits.insert(key);
    }

    /// Warns once per composition and `text_3d` node `node_id` that `glyphs` of its glyphs have
    /// no outlines.
    #[cfg(feature = "3d")]
    pub(crate) fn note_missing_outlines(&mut self, node_id: NodeId, glyphs: usize) {
        let key = (self.scope.template_name().map(str::to_string), node_id);
        if self.st.outline_gaps.contains(&key) {
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
        self.st.outline_gaps.insert(key);
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
        if self.st.font_fallbacks.contains(&key) {
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
        self.st.font_fallbacks.insert(key);
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

    #[cfg(feature = "3d")]
    pub(crate) fn node(&self, id: NodeId) -> Option<&'a NodeKind> {
        self.scope.node(id)
    }

    /// Counts frames recorded, for caches that keep what recent frames used.
    #[cfg(feature = "3d")]
    pub(crate) fn tick(&self) -> u64 {
        self.st.tick
    }

    /// Notes that the model or environment map `id` was drawn, so that a static output recording
    /// now keeps it.
    #[cfg(feature = "3d")]
    pub(crate) fn drew_model(&mut self, id: &str) {
        if self.static_depth > 0 {
            self.st.static_models.push(id.to_string());
        }
    }

    #[cfg(feature = "3d")]
    pub(crate) fn scene_cache(
        &mut self,
    ) -> (&mut Recorder, &mut crate::node::scene::collect::SceneCache) {
        (&mut *self.rec, &mut self.st.scene)
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
        let key = ((graph, self.path, node_id), self.sample);
        let raster = match self.st.memo.get(&key) {
            Some(raster) => *raster,
            None => {
                if self.depth == MAX_GRAPH_DEPTH {
                    return Err(self.scope.too_deep(node_id));
                }
                let passes = self.rec.pass_count();
                self.static_depth += u32::from(is_static);
                self.depth += 1;
                let raster = record_node(node, self);
                self.depth -= 1;
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
        // An image's own texture is let go of by frames that don't bind it, so it isn't kept.
        if self.st.media.values().any(|(tex, _)| *tex == raster.tex) {
            return;
        }
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
                #[cfg(feature = "3d")]
                models: std::mem::take(&mut self.st.static_models),
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

    /// Remembers the texture of an image source when what reads it is retained.
    pub(crate) fn note_image(&mut self, source: &str, key: &super::MediaTextureKey) {
        if self.static_depth > 0 && !self.st.images.contains_key(source) {
            self.st.images.insert(source.to_string(), Some(key.clone()));
        }
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

    /// The param `param` of node `node` in the graph being recorded.
    #[cfg(feature = "3d")]
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

    /// An image's texture, uploaded once and kept in the renderer's media cache.
    pub(crate) fn image_texture(
        &mut self,
        key: super::MediaTextureKey,
        frame: &MediaFrame,
        size: Size,
    ) -> crate::Result<TexRef> {
        let cpu = match frame {
            MediaFrame::CpuRgba(cpu) => cpu,
            MediaFrame::RawRgba(_) | MediaFrame::Yuv(_) => {
                return Err(RenderError::Gpu {
                    details: "an image resolver returned a video frame".to_string(),
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
    fn cpu_video_texture(
        &mut self,
        frame: Arc<CpuMediaFrame>,
        size: Size,
        premultiply: bool,
    ) -> TexRef {
        let tex =
            self.rec
                .acquire_written_early(size, RGBA8, TextureDesc::sampled(size, RGBA8).usage);
        self.st.video_uploads.push(VideoUpload {
            tex,
            frame,
            size,
            premultiply,
        });
        tex
    }

    /// The texture of a frame decoded on the CPU, either as RGBA or as YUV planes.
    fn cpu_frame_texture(
        &mut self,
        frame: MediaFrame,
        size: Size,
        alpha: AlphaMode,
    ) -> crate::Result<TexRef> {
        match frame {
            MediaFrame::CpuRgba(frame) => Ok(self.cpu_video_texture(frame, size, false)),
            MediaFrame::RawRgba(frame) => {
                Ok(self.cpu_video_texture(frame, size, alpha == AlphaMode::Straight))
            }
            MediaFrame::Yuv(frame) => self.yuv_video_texture(frame, size, alpha),
            #[cfg(feature = "ffmpeg")]
            MediaFrame::GpuVideo(_) => Err(RenderError::Gpu {
                details: "a CPU frame was expected, not a GPU one".to_string(),
            }
            .into()),
        }
    }

    /// Writes a CPU YUV frame's planes to pooled textures before the frame's passes run, and
    /// converts them to RGBA.
    fn yuv_video_texture(
        &mut self,
        frame: Arc<YuvFrame>,
        size: Size,
        alpha: AlphaMode,
    ) -> crate::Result<TexRef> {
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
        self.yuv_to_rgba(&planes, &frame.format, size, alpha)
    }

    /// Converts YUV planes into a premultiplied RGBA8 texture of `size`, showing the frame
    /// `format` describes upright and stretched to it by nearest neighbour. Colour that `alpha`
    /// says is already premultiplied is left as it is. Plane textures hold
    /// unsigned-integer samples (`R8Uint`, `Rg8Uint` for interleaved chroma, or their 16-bit forms)
    /// with the frame at their top left, and any texture this frame's passes can read will do:
    /// pooled ones written before the passes, or adopted ones a decoder filled on the GPU.
    pub(crate) fn yuv_to_rgba(
        &mut self,
        planes: &YuvPlanes<TexRef>,
        format: &YuvFormat,
        size: Size,
        alpha: AlphaMode,
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
        let alpha_plane = planes.alpha.unwrap_or(planes.luma);
        let uniform = self.rec.uniform(&YuvUniform::new(
            format,
            interleaved,
            planes.alpha.is_some(),
            alpha == AlphaMode::Premultiplied,
        ));
        let target = self.storage_target(size);
        self.rec.compute(
            program,
            &[
                (0, Res::Tex(planes.luma)),
                (1, Res::Tex(u)),
                (2, Res::Tex(v)),
                (3, Res::Tex(alpha_plane)),
                (4, uniform),
                (5, Res::Tex(target)),
            ],
            dispatch_for(size),
        );
        Ok(target)
    }

    /// The texture `media_in` node `node_id` shows `source_frame` of the video `resolver` names
    /// through, taking the frame from `frame` the first time this frame shows it.
    pub(crate) fn video_texture(
        &mut self,
        node_id: NodeId,
        resolver: &str,
        source_frame: u32,
        frame: impl FnOnce() -> crate::Result<MediaFrame>,
        size: Size,
        alpha: AlphaMode,
    ) -> crate::Result<TexRef> {
        let instance = (self.graph_key(), self.path, node_id);
        let key = (
            instance,
            resolver.to_string(),
            source_frame,
            (size.width, size.height),
            alpha == AlphaMode::Premultiplied,
        );
        if let Some(tex) = self.st.video_frames.get(&key) {
            return Ok(*tex);
        }
        let tex = self.decoded_video_texture(instance, frame()?, size, alpha)?;
        self.st.video_frames.insert(key, tex);
        Ok(tex)
    }

    /// The texture a video frame shows through. A GPU frame is imported now, ahead of this frame's
    /// submission in queue order, which settles its source on the GPU. One the renderer can't use
    /// moves its source to the CPU, unless the source has already imported a GPU frame, even earlier
    /// in this output frame, which fails the render instead.
    fn decoded_video_texture(
        &mut self,
        #[cfg_attr(not(feature = "ffmpeg"), allow(unused_variables))] instance: InstanceKey,
        frame: MediaFrame,
        size: Size,
        alpha: AlphaMode,
    ) -> crate::Result<TexRef> {
        match frame {
            frame @ (MediaFrame::CpuRgba(_) | MediaFrame::RawRgba(_) | MediaFrame::Yuv(_)) => {
                self.cpu_frame_texture(frame, size, alpha)
            }
            #[cfg(feature = "ffmpeg")]
            MediaFrame::GpuVideo(frame) => {
                // Imports copy into their slot's textures before the frame's passes run, so each
                // distinct frame a source shows this frame needs a slot of its own.
                let imported = self.st.frame_imports.entry(instance).or_default();
                let key = (instance, *imported);
                *imported += 1;
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
                    .and_then(|imported| {
                        if frame.settle_on_gpu() {
                            Ok(imported)
                        } else {
                            Err("its source moved to the CPU".to_string())
                        }
                    });
                match imported {
                    Ok(imported) => {
                        let planes = imported
                            .planes
                            .map_with_shape(&imported.format, |texture, _| {
                                self.adopted_media(Arc::clone(texture))
                            });
                        self.yuv_to_rgba(&planes, &imported.format, size, alpha)
                    }
                    Err(reason) => {
                        let cpu = frame.decode_on_cpu(&reason).map_err(|error| RenderError::Gpu {
                            details: format!(
                                "failed to import GPU media frame: {reason}; decoding it on the CPU also failed: {error}"
                            ),
                        })?;
                        self.cpu_frame_texture(cpu, size, alpha)
                    }
                }
            }
        }
    }

    /// Whether analytic motion blur applies. It is off inside a `motion_blur` that samples, whose
    /// samples already spread motion over the shutter.
    pub(crate) fn analytic_motion_blur(&self) -> bool {
        !self.sampling
    }

    /// Records `source` at each of `offsets` frames from this one and averages the samples in
    /// premultiplied space. Each sample is summed into a running target as soon as it is recorded,
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
    /// sample is `blank`: a running sum, or the average of `samples` once `last`.
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
            None => self.unused_uint(),
        };
        let size = domain.storage_size;
        let (program, tex) = if last {
            let program = self.program(
                "motion-blur-resolve",
                blur::RESOLVE_SHADER,
                "cs_main",
                blur::RESOLVE_LAYOUT,
            )?;
            (program, self.storage_target(size))
        } else {
            let program = self.program(
                "motion-blur-accumulate",
                blur::ACCUMULATE_SHADER,
                "cs_main",
                blur::ACCUMULATE_LAYOUT,
            )?;
            let format = blur::SUM_FORMAT;
            let tex = self
                .rec
                .acquire(size, format, TextureDesc::storage(size, format).usage);
            (program, tex)
        };
        let uniform = self.rec.uniform(&params);
        self.rec.compute(
            program,
            &[
                (0, Res::Tex(sum_tex)),
                (1, Res::Tex(sample.tex)),
                (2, uniform),
                (3, Res::Tex(tex)),
            ],
            dispatch_for(size),
        );
        Ok(Raster {
            tex,
            domain,
            bounds: None,
        })
    }

    /// A one-texel unsigned-integer texture for a running sum a pass binds but does not read.
    fn unused_uint(&mut self) -> TexRef {
        if let Some(tex) = self.st.dummy_uint {
            return tex;
        }
        let texture = self.rec.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("lumen frame graph unused sum"),
            size: Size::new(1, 1).as_extent(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: crate::node::processing::motion_blur::SUM_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let tex = self.rec.adopt(Arc::new(texture));
        self.st.dummy_uint = Some(tex);
        tex
    }

    /// Whether `node` and everything it reads are static candidates. Walks its inputs depth first
    /// with a stack of its own, so a long chain of nodes can't run out of call stack.
    fn is_static(&mut self, node: &NodeKind) -> bool {
        let graph = self.graph_key();
        let mut pending = vec![node.id()];
        // Nodes whose inputs have been put on `pending`, which are decided when next reached.
        let mut entered = HashSet::default();
        while let Some(&id) = pending.last() {
            if self.st.statics.contains_key(&(graph, id)) {
                pending.pop();
                continue;
            }
            let candidate = self.scope.node(id).filter(|node| {
                is_static_candidate(node)
                    && !matches!(node, NodeKind::Clip(_) | NodeKind::TimeRemap(_))
            });
            let Some(node) = candidate else {
                self.st.statics.insert((graph, id), false);
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
            let result = inputs
                .into_iter()
                .all(|input| self.st.statics.get(&(graph, input)) == Some(&true));
            self.st.statics.insert((graph, id), result);
            pending.pop();
        }
        self.st.statics.get(&(graph, node.id())) == Some(&true)
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
fn is_static_candidate(node: &NodeKind) -> bool {
    match node {
        // An image changes only with the media store, which `images_changed` watches.
        NodeKind::MediaIn(media) => {
            matches!(media.params.kind, MediaInSourceKindDelegate::Image) && !node.has_expressions()
        }
        // Video changes with the frame, shaders see the built-in time, and the output is written
        // into external textures that replace its own.
        NodeKind::WgslShader(_) | NodeKind::MediaOutput(_) => false,
        // A model playing a clip poses differently with the frame.
        NodeKind::Model3d(model) => !model.animates() && !node.has_expressions(),
        NodeKind::Instance3d(instance) => instance.is_frame_independent(),
        _ => !node.has_expressions(),
    }
}

fn node_programs(node: &NodeKind, programs: &mut Programs<'_>) -> crate::Result<()> {
    match node {
        NodeKind::MediaIn(node) => node.programs(programs),
        NodeKind::Background(node) => node.programs(programs),
        NodeKind::FractalNoise(node) => node.programs(programs),
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
        NodeKind::MotionBlur(node) => node.programs(programs),
        NodeKind::Crop(node) => node.programs(programs),
        NodeKind::Resize(node) => node.programs(programs),
        NodeKind::Shadow(node) => node.programs(programs),
        NodeKind::Glow(node) => node.programs(programs),
        NodeKind::DirectionalBlur(node) => node.programs(programs),
        NodeKind::RadialBlur(node) => node.programs(programs),
        NodeKind::ZoomBlur(node) => node.programs(programs),
        NodeKind::DisplacementMap(node) => node.programs(programs),
        NodeKind::WgslShader(node) => node.programs(programs),
        NodeKind::Switch(node) => node.programs(programs),
        NodeKind::Clip(_) => programs.spatial("clip-crop", CLIP_CROP_SHADER),
        NodeKind::MediaOutput(node) => node.programs(programs),
        #[cfg(feature = "3d")]
        NodeKind::Render3d(node) => node.programs(programs),
        #[cfg(not(feature = "3d"))]
        NodeKind::Render3d(_) => Ok(()),
        #[cfg(feature = "3d")]
        NodeKind::Light3d(light) => crate::node::scene::render::light_programs(light, programs),
        #[cfg(feature = "3d")]
        NodeKind::Shape3d(shape) => crate::node::scene::render::catcher_programs(
            &shape.params.material.shadow_catcher,
            programs,
        ),
        #[cfg(feature = "3d")]
        NodeKind::ImagePlane3d(plane) => crate::node::scene::render::catcher_programs(
            &plane.params.material.shadow_catcher,
            programs,
        ),
        #[cfg(feature = "3d")]
        NodeKind::Text3d(text) => crate::node::scene::render::catcher_programs(
            &text.params.material.shadow_catcher,
            programs,
        ),
        #[cfg(not(feature = "3d"))]
        NodeKind::Shape3d(_)
        | NodeKind::ImagePlane3d(_)
        | NodeKind::Text3d(_)
        | NodeKind::Light3d(_) => Ok(()),
        NodeKind::Model3d(_)
        | NodeKind::Camera3d(_)
        | NodeKind::Merge3d(_)
        | NodeKind::Instance3d(_) => Ok(()),
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
    let raster =
        record_kind(node, cx).map_err(|error| locate_media_failure(node, &cx.scope, error));
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

fn record_kind(node: &NodeKind, cx: &mut FrameCx<'_>) -> crate::Result<Raster> {
    match node {
        NodeKind::MediaIn(node) => node.record(cx),
        NodeKind::Background(node) => node.record(cx),
        NodeKind::FractalNoise(node) => node.record(cx),
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
        NodeKind::MotionBlur(node) => node.record(cx),
        NodeKind::Crop(node) => node.record(cx),
        NodeKind::Resize(node) => node.record(cx),
        NodeKind::Shadow(node) => node.record(cx),
        NodeKind::Glow(node) => node.record(cx),
        NodeKind::DirectionalBlur(node) => node.record(cx),
        NodeKind::RadialBlur(node) => node.record(cx),
        NodeKind::ZoomBlur(node) => node.record(cx),
        NodeKind::DisplacementMap(node) => node.record(cx),
        NodeKind::WgslShader(node) => node.record(cx),
        NodeKind::Switch(node) => node.record(cx),
        NodeKind::Clip(node) => cx.record_clip(node),
        NodeKind::MediaOutput(node) => node.record(cx),
        #[cfg(feature = "3d")]
        NodeKind::Render3d(node) => node.record(cx),
        #[cfg(not(feature = "3d"))]
        NodeKind::Render3d(node) => Err(RenderError::NodeEvaluation {
            node_id: node.id,
            node_kind: "Render3d",
            details: crate::error::NO_3D_SUPPORT.to_string(),
        }
        .into()),
        // Validation keeps a scene out of every raster input, and `render_3d` reads its scene
        // without recording it.
        NodeKind::Shape3d(_)
        | NodeKind::ImagePlane3d(_)
        | NodeKind::Text3d(_)
        | NodeKind::Model3d(_)
        | NodeKind::Camera3d(_)
        | NodeKind::Light3d(_)
        | NodeKind::Merge3d(_)
        | NodeKind::Instance3d(_) => Err(RenderError::NodeEvaluation {
            node_id: node.id(),
            node_kind: "Scene",
            details: "a scene node outputs a scene, not a raster; draw it through a render_3d"
                .to_string(),
        }
        .into()),
    }
}
