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
        AssetKind, CpuMediaFrame, MediaFrame, MediaIdentity, MediaStore, PlaneShape, YuvChroma,
        YuvFormat, YuvFrame, YuvPlanes, YuvUniform,
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
    color::WORKING_COLOR_FORMAT as WORKING,
    params::{CropParams, copyable_texture_desc, dispatch_for},
    renderer::{MediaTextureCache, OutputEncoding, fit_frame_to_rgba8},
};

/// The format CPU RGBA frames are uploaded in, before `INGEST_SHADER` brings them into the working
/// space.
const UPLOAD_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Brings an image or CPU video texture into the working space: an sRGB-encoded one is decoded,
/// its linear light goes from its primaries to BT.709 with colour outside BT.709 clipped at zero,
/// and it is premultiplied. Colour already premultiplied is divided back out first.
const INGEST_SHADER: &str = concat!(
    include_str!("shaders/alpha.wgsl"),
    include_str!("shaders/color.wgsl"),
    "
struct Params {
    premultiplied: u32,
    // Non-zero when the texture holds sRGB-encoded colour rather than linear light.
    encoded: u32,
    _pad0: u32,
    _pad1: u32,
    // The texture's primaries to BT.709, in linear light.
    gamut: mat3x3<f32>,
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
    var color = textureLoad(input_tex, vec2<i32>(id.xy), 0);
    if (params.premultiplied != 0u) {
        color = unpremultiply(color);
    }
    var linear = color.rgb;
    if (params.encoded != 0u) {
        linear = lumen_srgb_to_linear(linear);
    }
    linear = max(params.gamut * linear, vec3<f32>(0.0));
    textureStore(output_tex, vec2<i32>(id.xy), premultiply(vec4<f32>(linear, color.a)));
}
"
);
/// `INGEST_SHADER`'s parameters.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct IngestParams {
    premultiplied: u32,
    encoded: u32,
    _pad: [u32; 2],
    /// Columns of the gamut matrix, each padded to four floats as WGSL lays out a `mat3x3`.
    gamut: [[f32; 4]; 3],
}

impl IngestParams {
    fn new(premultiplied: bool, encoded: bool, gamut: Option<crate::media::GamutMatrix>) -> Self {
        let rows = gamut.map_or(
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            |gamut| gamut.0,
        );
        Self {
            premultiplied: u32::from(premultiplied),
            encoded: u32::from(encoded),
            _pad: [0; 2],
            gamut: [0, 1, 2].map(|column| [rows[0][column], rows[1][column], rows[2][column], 0.0]),
        }
    }
}

/// An image frame's pixels as they are uploaded.
#[derive(Clone, Copy)]
enum ImageUpload<'a> {
    /// RGBA8, sRGB-encoded.
    Srgb8(&'a CpuMediaFrame),
    /// Straight linear half floats.
    Linear(&'a crate::media::LinearMediaFrame),
}

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
const YUV_TO_RGBA_SHADER: &str = concat!(
    include_str!("shaders/color.wgsl"),
    include_str!("shaders/yuv_to_rgba.wgsl")
);
const YUV_TO_RGBA_LAYOUT: &[LayoutEntry] = &[
    LayoutEntry::compute(0, LayoutKind::UintTexture),
    LayoutEntry::compute(1, LayoutKind::UintTexture),
    LayoutEntry::compute(2, LayoutKind::UintTexture),
    LayoutEntry::compute(3, LayoutKind::UintTexture),
    LayoutEntry::compute(4, LayoutKind::Uniform),
    LayoutEntry::compute(5, LayoutKind::StorageTexture(WORKING)),
];
/// Frames a retained output stays without being read.
const RETAINED_IDLE_FRAMES: u64 = 600;
/// Distinct runtime shader sources kept, parsed and assembled.
const MAX_SHADERS: usize = 64;
/// Frames the import texture of a video source stays without being used.
#[cfg(feature = "ffmpeg")]
const VIDEO_IMPORT_IDLE_FRAMES: u64 = 600;

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

    /// The program that brings CPU images and video frames into the working space.
    pub(crate) fn ingest(&mut self) -> crate::Result<()> {
        self.spatial("ingest", INGEST_SHADER)
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
        node: NodeId,
        label: &str,
        source: &str,
        inputs: usize,
        build: impl FnOnce() -> crate::Result<ShaderParts>,
    ) -> crate::Result<CachedShader> {
        let shader = self.st.cached_shader(source, inputs, build)?;
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
}

impl Staticness {
    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::Never, _) | (_, Self::Never) => Self::Never,
            (Self::WhileHeld, _) | (_, Self::WhileHeld) => Self::WhileHeld,
            _ => Self::Always,
        }
    }
}

/// The frames a kept output stays correct for, and what it drew.
#[cfg(feature = "3d")]
#[derive(Debug, Clone)]
struct HoldTrack {
    /// Both ends included.
    frames: (f64, f64),
    models: Vec<String>,
}

#[cfg(feature = "3d")]
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

    /// Whether some frame other than `frame`, among those a composition has, is also in it.
    fn contains_another(&self, frame: f64) -> bool {
        let next = frame.floor() + 1.0;
        let previous = frame.ceil() - 1.0;
        self.contains(next) || (previous >= 0.0 && self.contains(previous))
    }
}

struct Slot {
    raster: Raster,
    /// Passes recording it took, which each read of it skips.
    passes: u64,
    /// Its copy at full frame, once something has read it that way.
    full: Option<TexRef>,
    last_used: u64,
    bytes: u64,
    /// Tells this slot from any other kept in its place.
    generation: u64,
    /// The frames it may be read on, both ends included.
    frames: (f64, f64),
    /// The models and environment maps it drew, which the scene cache keeps while it is kept.
    #[cfg_attr(
        not(feature = "3d"),
        allow(dead_code, reason = "read by the scene cache")
    )]
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
    statics: HashMap<GraphNodeKey, Staticness>,
    slots: HashMap<GraphNodeKey, Slot>,
    /// Slots made so far.
    slot_generation: u64,
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
    /// Adopted images and video planes by their texture, the tick that last bound each, and the
    /// key of an image's pixels.
    media: HashMap<usize, (TexRef, u64, Option<super::MediaTextureKey>)>,
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
    /// The identity each source had, as the kind it was read as, when something kept across frames
    /// read it, or `None` if it didn't resolve. Kept outputs don't read their sources again, so
    /// these are what notices a source changing under them.
    kept_sources: HashMap<(AssetKind, String), Option<MediaIdentity>>,
    /// The template, node and family of each font fallback reported for this composition.
    font_fallbacks: HashSet<(Option<String>, NodeId, String)>,
    /// The template, node and source of each unsupported colour reported for this composition.
    media_colors: HashSet<(Option<String>, NodeId, String)>,
    /// How large this composition can show each image, found once it shows one.
    shown_sizes: Option<Arc<super::image_size::ShownSizes>>,
    /// Each image reported as reduced to fit the size limit for this composition.
    reduced_images: HashSet<String>,
    /// The template and `render_3d` node of each shadow limit reported for this composition.
    #[cfg(feature = "3d")]
    shadow_limits: HashSet<(Option<String>, NodeId)>,
    /// The template and `text_3d` node of each report of glyphs without outlines for this
    /// composition.
    #[cfg(feature = "3d")]
    outline_gaps: HashSet<(Option<String>, NodeId)>,
    /// The template and `model_3d` node of each model with skipped primitives reported for this
    /// composition.
    #[cfg(feature = "3d")]
    skipped_primitives: HashSet<(Option<String>, NodeId)>,
    /// Warnings not yet taken.
    diagnostics: Vec<crate::diagnostic::Diagnostic>,
    /// Passes reads of retained outputs skipped.
    skipped_passes: u64,
    #[cfg(feature = "3d")]
    scene: crate::node::scene::collect::SceneCache,
    luts: crate::node::processing::color_grade::LutCache,
    /// Mip chains of the rasters nodes show smaller than their size.
    mips: crate::gpu::mips::MipCache,
    /// Models drawn while the static output being recorded records, which its slot keeps.
    static_models: Vec<String>,
    /// What each output that is static while its models hold a pose found out when it was recorded
    /// this frame, for the outputs reading it again.
    #[cfg(feature = "3d")]
    held_memo: HashMap<MemoKey, HoldTrack>,
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
            if self.state.font_generation != generation || self.kept_sources_changed(media) {
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
            self.state.static_models.clear();
            #[cfg(feature = "3d")]
            self.state.held_memo.clear();
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
                #[cfg(feature = "3d")]
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
                self.state
                    .luts
                    .trim(&mut self.recorder, self.state.tick, self.budgets.luts);
                #[cfg(feature = "3d")]
                self.state.scene.trim(
                    &mut self.recorder,
                    self.state.tick,
                    self.state.slots.values().flat_map(|slot| &slot.models),
                    &self.budgets,
                );
                #[cfg(feature = "3d")]
                self.state.scene.trim_environments(
                    &mut self.recorder,
                    self.state.tick,
                    self.state.slots.values().flat_map(|slot| &slot.models),
                    self.budgets.environments,
                );
                self.state
                    .mips
                    .trim(&mut self.recorder, self.state.tick, self.budgets.mip_chains);
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
        self.state.media.retain(|_, (tex, last_used, _)| {
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
        for VideoUpload { tex, frame, size } in self.state.video_uploads.drain(..) {
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
        self.recorder.upload().map_err(gpu_error)?;
        self.forget_unbound_media();
        Ok(())
    }

    /// The media cache frees what only it holds, so images this frame doesn't bind are let go.
    fn forget_unbound_media(&mut self) {
        let tick = self.state.tick;
        let recorder = &mut self.recorder;
        self.state.media.retain(|_, (tex, last_used, _)| {
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
        self.state.mips.commit();
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
        self.state.media.retain(|_, (tex, ..)| unheld(tex));
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
        for (_, (tex, ..)) in state.media.drain() {
            self.recorder.forget(tex);
        }
        for (_, tex) in state.externals.drain() {
            self.recorder.forget(tex);
        }
        state.statics.clear();
        state.texts.clear();
        state.paths.clear();
        state.kept_sources.clear();
        state.font_fallbacks.clear();
        state.media_colors.clear();
        state.shown_sizes = None;
        state.reduced_images.clear();
        #[cfg(feature = "3d")]
        state.shadow_limits.clear();
        #[cfg(feature = "3d")]
        state.outline_gaps.clear();
        #[cfg(feature = "3d")]
        state.skipped_primitives.clear();
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
        #[cfg(feature = "3d")]
        self.state.scene.release(&mut self.recorder);
        self.state.luts.release(&mut self.recorder);
        self.state.mips.release(&mut self.recorder);
        self.recorder.release_all();
    }

    /// Whether a source something kept across frames read now has another identity, or resolves
    /// where it didn't or the other way round, as when a host replaces its bytes or a font arrives.
    fn kept_sources_changed(&self, media: Option<&dyn MediaStore>) -> bool {
        self.state
            .kept_sources
            .iter()
            .any(|((kind, source), seen)| source_changed(media, *kind, source, seen.as_ref()))
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
        let (models, environments, text_meshes) = self.state.scene.held_bytes();
        #[cfg(not(feature = "3d"))]
        let (models, environments, text_meshes) = (0, 0, 0);
        super::CacheStats {
            pooled_textures: self.recorder.pool_bytes(),
            retained_outputs: self.state.retained_bytes,
            luts: self.state.luts.bytes(),
            mip_chains: self.state.mips.bytes(),
            models,
            environments,
            text_meshes,
            ..super::CacheStats::default()
        }
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
        self.state.mips.discard_pending();
        for key in std::mem::take(&mut self.state.pending) {
            Self::release_slot(&mut self.recorder, &mut self.state, key);
        }
    }

    fn drop_slots(&mut self) {
        self.state.pending.clear();
        self.state.kept_sources.clear();
        let keys: Vec<_> = self.state.slots.keys().copied().collect();
        for key in keys {
            Self::release_slot(&mut self.recorder, &mut self.state, key);
        }
    }

    fn evict_slots(&mut self) {
        let tick = self.state.tick;
        let idle_before = tick.saturating_sub(RETAINED_IDLE_FRAMES);
        let budget = self.budgets.retained_outputs;
        let over_budget = self.state.retained_bytes > budget;
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
            if last_used > idle_before && self.state.retained_bytes <= budget {
                break;
            }
            Self::release_slot(&mut self.recorder, &mut self.state, key);
        }
    }

    pub(crate) fn retained_bytes(&self) -> u64 {
        self.state.retained_bytes
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
    /// What the nodes that are static while their models hold a pose, which the walk is inside,
    /// have found out so far, innermost last.
    #[cfg(feature = "3d")]
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

/// The media failure of decoding a frame on the CPU after importing it on the GPU failed for
/// `reason`.
#[cfg(feature = "ffmpeg")]
fn cpu_retry_failed(error: crate::error::MediaError, reason: &str) -> crate::error::LumenError {
    use crate::error::MediaError;
    match error {
        MediaError::Failed(mut failure) => {
            let details = format!(
                "failed to import GPU media frame: {reason}; decoding it on the CPU also failed: {}",
                failure.reason.details()
            );
            failure.reason = failure.reason.with_details(details);
            MediaError::Failed(failure).into()
        }
        error => error.into(),
    }
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

    /// Warns once per composition and `model_3d` node `node_id` that its model `source` has
    /// `count` primitives that aren't drawn.
    #[cfg(feature = "3d")]
    pub(crate) fn note_skipped_primitives(&mut self, node_id: NodeId, source: &str, count: usize) {
        let key = (self.scope.template_name().map(str::to_string), node_id);
        if self.st.skipped_primitives.contains(&key) {
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
        self.st.skipped_primitives.insert(key);
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
        if self.st.media_colors.contains(&key) {
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
        self.st.media_colors.insert(key);
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
            self.st.static_models.push(id.to_string());
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
        if let Some(slot) = self.st.slots.get(&(graph, node))
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
        let staticness = if self.retain_static {
            self.staticness(node)
        } else {
            Staticness::Never
        };
        let is_static = staticness == Staticness::Always;
        let frame = self.frame.value();
        if staticness != Staticness::Never
            && let Some(slot) = self.st.slots.get_mut(&(graph, node_id))
            && slot.frames.0 <= frame
            && frame <= slot.frames.1
        {
            slot.last_used = self.st.tick;
            self.st.skipped_passes += slot.passes;
            let raster = slot.raster;
            #[cfg(feature = "3d")]
            self.hold_slot(graph, node_id);
            return Ok(raster);
        }
        let key = ((graph, self.path, node_id), self.sample);
        let raster = match self.st.memo.get(&key) {
            Some(raster) => {
                let raster = *raster;
                #[cfg(feature = "3d")]
                if let Some(read) = self.st.held_memo.get(&key)
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
                #[cfg(feature = "3d")]
                if staticness == Staticness::WhileHeld {
                    self.holds.push(HoldTrack::new());
                }
                self.depth += 1;
                let raster = record_node(node, self);
                self.depth -= 1;
                self.static_depth -= u32::from(is_static);
                #[cfg(feature = "3d")]
                let track = (staticness == Staticness::WhileHeld)
                    .then(|| self.holds.pop())
                    .flatten();
                let raster = raster?;
                self.st.memo.insert(key, raster);
                let passes = (self.rec.pass_count() - passes) as u64;
                if is_static && self.static_depth == 0 {
                    let frames = (f64::NEG_INFINITY, f64::INFINITY);
                    let models = std::mem::take(&mut self.st.static_models);
                    self.retain(graph, node_id, raster, passes, frames, models);
                }
                #[cfg(feature = "3d")]
                if let Some(track) = track {
                    self.finish_hold(key, raster, passes, track);
                }
                raster
            }
        };
        Ok(raster)
    }

    /// Keeps `raster`, the output of `node`, for the `frames` it stays correct on, along with the
    /// `models` it drew.
    fn retain(
        &mut self,
        graph: usize,
        node: NodeId,
        raster: Raster,
        passes: u64,
        frames: (f64, f64),
        models: Vec<String>,
    ) {
        // A slot kept for other frames makes way.
        if let Some(old) = self.st.slots.remove(&(graph, node)) {
            self.rec.release(old.raster.tex);
            if let Some(full) = old.full {
                self.rec.release(full);
            }
            self.st.retained_bytes -= old.bytes;
        }
        self.rec.retain(raster.tex);
        let bytes = self.rec.texture_bytes(raster.tex);
        self.st.retained_bytes += bytes;
        self.st.slot_generation += 1;
        self.st.slots.insert(
            (graph, node),
            Slot {
                raster,
                passes,
                full: None,
                last_used: self.st.tick,
                bytes,
                generation: self.st.slot_generation,
                frames,
                models,
            },
        );
        self.st.pending.push((graph, node));
    }

    /// Notes in the output being recorded that it reads the kept output of `node`, which holds
    /// only on the frames the slot is correct on.
    #[cfg(feature = "3d")]
    fn hold_slot(&mut self, graph: usize, node: NodeId) {
        let Some(track) = self.holds.last_mut() else {
            return;
        };
        if let Some(slot) = self.st.slots.get(&(graph, node)) {
            track.narrow(slot.frames);
            track.models.extend(slot.models.iter().cloned());
        }
    }

    /// Keeps `raster`, the output of `node` just recorded, if the models it drew hold their poses
    /// on this frame and on another too, and tells the output being recorded around it what it
    /// found.
    #[cfg(feature = "3d")]
    fn finish_hold(&mut self, key: MemoKey, raster: Raster, passes: u64, track: HoldTrack) {
        let ((graph, _, node), _) = key;
        if let Some(outer) = self.holds.last_mut() {
            outer.narrow(track.frames);
            outer.models.extend(track.models.iter().cloned());
        }
        self.st.held_memo.insert(key, track.clone());
        let frame = self.frame.value();
        if self.static_depth == 0 && track.contains(frame) && track.contains_another(frame) {
            self.retain(graph, node, raster, passes, track.frames, track.models);
        }
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

    /// Notes that `source` was read as `kind` and had `identity`, so that what is kept of this
    /// frame is dropped once it has another. It counts while a kept output records, including one
    /// kept while its models hold a pose; font families count whenever text names them, as text
    /// instances are kept across frames whatever reads them.
    pub(crate) fn read_source(
        &mut self,
        kind: AssetKind,
        source: &str,
        identity: Option<&MediaIdentity>,
    ) {
        #[cfg(feature = "3d")]
        let held = !self.holds.is_empty();
        #[cfg(not(feature = "3d"))]
        let held = false;
        let kept = self.static_depth > 0 || held || kind == AssetKind::Font;
        if kept
            && !self
                .st
                .kept_sources
                .contains_key(&(kind, source.to_string()))
        {
            self.st
                .kept_sources
                .insert((kind, source.to_string()), identity.cloned());
        }
    }

    /// Notes the font family text names, as [`Self::read_source`] does.
    pub(crate) fn note_font(&mut self, family: &str) {
        if !self
            .st
            .kept_sources
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
        if self.st.reduced_images.len() >= WARNED_SOURCES
            || !self.st.reduced_images.insert(source.to_string())
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

    /// An image in the working space at `size`, the size of `key`. Its texture, 8-bit for
    /// sRGB-encoded frames and half float for linear ones, is uploaded once and kept in the
    /// renderer's media cache, and each frame that shows it brings it into the working space.
    pub(crate) fn image_texture(
        &mut self,
        key: super::MediaTextureKey,
        frame: &MediaFrame,
        size: Size,
    ) -> crate::Result<TexRef> {
        let (upload, premultiplied, gamut) = match frame {
            MediaFrame::CpuRgba(cpu) => (ImageUpload::Srgb8(cpu), true, None),
            MediaFrame::RawRgba(cpu) => (ImageUpload::Srgb8(cpu), false, None),
            MediaFrame::WideRgba { frame, gamut } => {
                (ImageUpload::Srgb8(frame), false, Some(*gamut))
            }
            MediaFrame::LinearRgba(linear) => (ImageUpload::Linear(linear), false, None),
            MediaFrame::Yuv(_) => {
                return Err(internal("an image resolver returned a video frame"));
            }
            #[cfg(feature = "ffmpeg")]
            MediaFrame::GpuVideo(_) => {
                return Err(internal("an image resolver returned a GPU frame"));
            }
        };
        let native = match upload {
            ImageUpload::Srgb8(cpu) => (cpu.width, cpu.height),
            ImageUpload::Linear(linear) => (linear.width, linear.height),
        };
        // A copy at another size is stored straight, which is known before the cache is asked.
        let stored_premultiplied = premultiplied && native == (size.width, size.height);
        let encoded = matches!(upload, ImageUpload::Srgb8(_));
        let texture = match self.media_cache.get(&key) {
            Some(texture) => texture,
            None => {
                let format = if encoded {
                    UPLOAD_FORMAT
                } else {
                    wgpu::TextureFormat::Rgba16Float
                };
                let texture =
                    super::renderer::create_media_texture(self.rec.device(), size, format);
                let write = |bytes: &[u8]| {
                    self.rec.queue().write_texture(
                        texture.as_image_copy(),
                        bytes,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(
                                size.width * format.block_copy_size(None).unwrap_or(4),
                            ),
                            rows_per_image: Some(size.height),
                        },
                        size.as_extent(),
                    );
                };
                match upload {
                    ImageUpload::Srgb8(cpu) => {
                        write(&super::image_size::rgba8_at(cpu, premultiplied, size));
                    }
                    ImageUpload::Linear(linear) => {
                        write(bytemuck::cast_slice(&super::image_size::linear_at(
                            linear, size,
                        )));
                    }
                }
                self.media_cache.insert(key.clone(), Arc::clone(&texture));
                texture
            }
        };
        let tex = self.adopted_media(texture, Some(key));
        let params = IngestParams::new(stored_premultiplied, encoded, gamut);
        self.ingest(tex, size, &params)
    }

    /// `tex` of `size` in the working space, as `params` says it is stored.
    fn ingest(&mut self, tex: TexRef, size: Size, params: &IngestParams) -> crate::Result<TexRef> {
        let target = self.storage_target(size);
        self.spatial_pass("ingest", INGEST_SHADER, tex, params, target, size)?;
        Ok(target)
    }

    /// The texture of an image, which holds the pixels `image` names, or of a video plane.
    fn adopted_media(
        &mut self,
        texture: Arc<wgpu::Texture>,
        image: Option<super::MediaTextureKey>,
    ) -> TexRef {
        let rec = &mut *self.rec;
        let tick = self.st.tick;
        let (tex, last_used, _) = self
            .st
            .media
            .entry(Arc::as_ptr(&texture) as usize)
            .or_insert_with(|| (rec.adopt(texture), tick, image));
        *last_used = tick;
        *tex
    }

    /// A CPU video frame in the working space, from a pooled 8-bit texture it is written to before
    /// the frame's passes run.
    fn cpu_video_texture(
        &mut self,
        frame: Arc<CpuMediaFrame>,
        size: Size,
        premultiplied: bool,
    ) -> crate::Result<TexRef> {
        let usage = TextureDesc::sampled(size, UPLOAD_FORMAT).usage;
        let tex = self.rec.acquire_written_early(size, UPLOAD_FORMAT, usage);
        self.st.video_uploads.push(VideoUpload { tex, frame, size });
        self.ingest(tex, size, &IngestParams::new(premultiplied, true, None))
    }

    /// The texture of a frame decoded on the CPU, either as RGBA or as YUV planes.
    fn cpu_frame_texture(
        &mut self,
        frame: MediaFrame,
        size: Size,
        alpha: AlphaMode,
    ) -> crate::Result<TexRef> {
        match frame {
            MediaFrame::CpuRgba(frame) => self.cpu_video_texture(frame, size, true),
            MediaFrame::RawRgba(frame) => {
                self.cpu_video_texture(frame, size, alpha == AlphaMode::Premultiplied)
            }
            MediaFrame::Yuv(frame) => self.yuv_video_texture(frame, size, alpha),
            MediaFrame::WideRgba { .. } | MediaFrame::LinearRgba(_) => {
                Err(internal("a video resolver returned an image frame"))
            }
            #[cfg(feature = "ffmpeg")]
            MediaFrame::GpuVideo(_) => Err(internal("a CPU frame was expected, not a GPU one")),
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
        frame
            .validate()
            .map_err(|details| internal(&format!("a YUV frame can't be read: {details}")))?;
        let max = self.rec.device().limits().max_texture_dimension_2d;
        if frame.format.width > max || frame.format.height > max {
            return Err(RenderError::Limit {
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

    /// Converts YUV planes into a texture of `size` in the working space, showing the frame
    /// `format` describes upright and stretched to it by nearest neighbour. Colour that `alpha`
    /// says is already premultiplied is divided by alpha before decoding. Plane textures hold
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
        identity: &MediaIdentity,
        source_frame: u32,
        frame: impl FnOnce() -> crate::Result<MediaFrame>,
        size: Size,
        alpha: AlphaMode,
    ) -> crate::Result<TexRef> {
        let instance = (self.graph_key(), self.path, node_id);
        let key = (
            instance,
            identity.clone(),
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
            frame @ (MediaFrame::CpuRgba(_)
            | MediaFrame::RawRgba(_)
            | MediaFrame::WideRgba { .. }
            | MediaFrame::LinearRgba(_)
            | MediaFrame::Yuv(_)) => self.cpu_frame_texture(frame, size, alpha),
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
                                self.adopted_media(Arc::clone(texture), None)
                            });
                        self.yuv_to_rgba(&planes, &imported.format, size, alpha)
                    }
                    Err(reason) => {
                        let cpu = frame
                            .decode_on_cpu(&reason)
                            .map_err(|error| cpu_retry_failed(error, &reason))?;
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
    let always = |candidate: bool| {
        if candidate {
            Staticness::Always
        } else {
            Staticness::Never
        }
    };
    match node {
        // An image changes only with the media store, which `images_changed` watches.
        NodeKind::MediaIn(media) => always(
            matches!(media.params.kind, MediaInSourceKindDelegate::Image)
                && !node.has_expressions(),
        ),
        // Video changes with the frame, and shaders see the built-in time.
        NodeKind::WgslShader(_) => Staticness::Never,
        // A model playing a clip poses differently with the frame, until its clip is over or
        // before it starts, which `add_model` finds out from the clip.
        NodeKind::Model3d(model) if model.animates() => {
            if cfg!(feature = "3d") && !node.has_expressions() {
                Staticness::WhileHeld
            } else {
                Staticness::Never
            }
        }
        NodeKind::Model3d(_) => always(!node.has_expressions()),
        NodeKind::Instance3d(instance) => always(instance.is_frame_independent()),
        _ => always(!node.has_expressions()),
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
