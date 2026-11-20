//! What recording keeps from frame to frame, split by what it serves: the walk of the graph,
//! retained outputs, text, media, runtime shaders and 3D.

use std::sync::Arc;

use foldhash::{HashMap, HashSet};
use lumen_gpu::{Recorder, Size, TexRef};

#[cfg(feature = "ffmpeg")]
use crate::gpu::media::GpuMediaImports;
use crate::{
    gpu::{CacheBudgets, MediaTextureKey},
    media::{AssetKind, CpuMediaFrame, MediaIdentity, MediaStore, PlaneShape, YuvFrame},
    node::{NodeId, source::text::TextInstance},
};

use super::{
    CachedShader, GraphNodeKey, InstanceKey, MemoKey, PathId, Raster, ShaderKey, ShaderParts,
    SlotKey, Staticness, VideoFrameKey,
    retention::{HoldTrack, source_changed},
};

/// Frames a retained output stays without being read.
const RETAINED_IDLE_FRAMES: u64 = 600;
/// Runtime shaders kept, parsed and assembled, by source and by composition shader.
const MAX_SHADERS: usize = 64;
/// Frames the import texture of a video source stays without being used.
#[cfg(feature = "ffmpeg")]
const VIDEO_IMPORT_IDLE_FRAMES: u64 = 600;

/// A warning's place, kind and [`crate::error::WarningKind::identity`].
pub(super) type WarningKey = (
    crate::error::At,
    std::mem::Discriminant<crate::error::WarningKind>,
    Option<String>,
);

#[derive(Default)]
pub(super) struct RecordState {
    pub(super) fingerprint: Option<u64>,
    pub(super) font_generation: Option<u64>,
    pub(super) tick: u64,
    pub(super) paths: HashMap<(PathId, NodeId), PathId>,
    /// The rasters this frame recorded, by the output of the node they are.
    pub(super) memo: HashMap<(MemoKey, crate::node::OutputPort), Raster>,
    pub(super) full_frames: HashMap<(TexRef, u32, u32), Raster>,
    pub(super) statics: HashMap<GraphNodeKey, Staticness>,
    /// The textures of external targets, by the address of the texture.
    pub(super) externals: HashMap<usize, TexRef>,
    pub(super) transparent: Option<TexRef>,
    pub(super) dummy_float: Option<TexRef>,
    pub(super) output: Option<Raster>,
    /// Warnings not yet taken.
    pub(super) diagnostics: Vec<crate::diagnostic::Diagnostic>,
    /// Where, of what kind and about what each warning raised for this composition is.
    pub(super) warned: HashSet<WarningKey>,
    pub(super) retained: Retained,
    pub(super) text: TextState,
    pub(super) media: MediaState,
    pub(super) shaders: ShaderCache,
    /// The walks `render_3d` nodes have made of their scenes.
    #[cfg(feature = "test-hooks")]
    pub(super) scene_walks: u64,
    #[cfg(feature = "3d")]
    pub(super) scene: SceneState,
    pub(super) luts: crate::node::processing::color_grade::LutCache,
    /// Mip chains of the rasters nodes show smaller than their size.
    pub(super) mips: crate::gpu::mips::MipCache,
    pub(super) drawings: crate::node::vector::drawing::DrawingCache,
    pub(super) profile: super::profile::NodeProfile,
    /// What `render_3d` nodes placed in the frames recorded and not yet taken.
    pub(super) captures: Vec<crate::gpu::SceneCapture>,
    /// What they placed in the frame being recorded, which is kept once it is.
    pub(super) pending_captures: Vec<crate::gpu::SceneCapture>,
    /// The `render_3d`s, by node and the path that reached them, that `pending_captures` holds.
    pub(super) pending_capture_renders: HashSet<(PathId, NodeId)>,
}

impl RecordState {
    /// Starts recording a frame, dropping what only the last frame used.
    pub(super) fn begin_frame(&mut self) {
        self.tick += 1;
        self.memo.clear();
        self.full_frames.clear();
        self.media.begin_frame();
        self.retained.begin_frame();
        self.drawings.begin_frame(self.tick);
        #[cfg(feature = "3d")]
        self.scene.begin_frame(self.tick);
    }

    /// Trims the caches recording fills to `budgets`, keeping what retained outputs drew.
    pub(super) fn trim(&mut self, recorder: &mut Recorder, budgets: &CacheBudgets) {
        self.luts.trim(recorder, self.tick, budgets.luts);
        #[cfg(feature = "3d")]
        self.scene.trim(recorder, self.tick);
        #[cfg(feature = "3d")]
        self.scene
            .cache
            .trim(recorder, self.tick, self.retained.models(), budgets);
        #[cfg(feature = "3d")]
        self.scene.cache.trim_environments(
            recorder,
            self.tick,
            self.retained.models(),
            budgets.environments,
        );
        self.mips.trim(recorder, self.tick, budgets.mip_chains);
    }

    /// Forgets what recording kept of the composition, but for the retained outputs, which the
    /// caller drops first.
    pub(super) fn reset(&mut self, recorder: &mut Recorder) {
        self.fingerprint = None;
        self.media.reset(recorder);
        for (_, tex) in self.externals.drain() {
            recorder.forget(tex);
        }
        self.statics.clear();
        self.paths.clear();
        self.drawings.clear();
        #[cfg(feature = "3d")]
        self.scene.reset(recorder);
        self.shaders.clear();
        self.memo.clear();
        self.full_frames.clear();
        self.output = None;
        self.warned.clear();
        self.text.reset(recorder);
    }

    /// Frees the textures and caches that outlive compositions.
    pub(super) fn release(&mut self, recorder: &mut Recorder) {
        self.transparent = None;
        self.dummy_float = None;
        #[cfg(feature = "3d")]
        self.scene.reset(recorder);
        #[cfg(feature = "3d")]
        self.scene.cache.release(recorder);
        self.luts.release(recorder);
        self.mips.release(recorder);
    }
}

pub(super) struct Slot {
    pub(super) raster: Raster,
    /// Passes recording it took, which each read of it skips.
    pub(super) passes: u64,
    /// Its copy at full frame, once something has read it that way.
    pub(super) full: Option<TexRef>,
    pub(super) last_used: u64,
    pub(super) bytes: u64,
    /// Tells this slot from any other kept in its place.
    pub(super) generation: u64,
    /// The frames it may be read on, both ends included.
    pub(super) frames: (f64, f64),
    /// The models and environment maps it drew, which the scene cache keeps while it is kept.
    pub(super) models: Vec<String>,
    /// The values the params reading the frame took when it was recorded, which a read must find
    /// again. Empty for an output that doesn't read the frame.
    pub(super) values: Vec<crate::expr::ExpressionValue>,
}

/// The outputs of static subtrees kept across frames, and what they read.
#[derive(Default)]
pub(super) struct Retained {
    pub(super) slots: HashMap<SlotKey, Slot>,
    /// Slots made so far.
    pub(super) generation: u64,
    /// Slots retained this frame, which hold nothing until it is submitted.
    pub(super) pending: Vec<SlotKey>,
    pub(super) bytes: u64,
    /// The identity each source had, as the kind it was read as, when something kept across frames
    /// read it, or `None` if it didn't resolve. Kept outputs don't read their sources again, so
    /// these are what notices a source changing under them.
    pub(super) sources: HashMap<(AssetKind, String), Option<MediaIdentity>>,
    /// Models drawn while the static output being recorded records, which its slot keeps.
    pub(super) static_models: Vec<String>,
    /// What each output that is static while its models hold a pose or its params' values hold
    /// found out when it was recorded this frame, for the outputs reading it again.
    pub(super) held_memo: HashMap<(MemoKey, crate::node::OutputPort), HoldTrack>,
    /// Passes reads of retained outputs skipped.
    pub(super) skipped_passes: u64,
}

impl Retained {
    fn begin_frame(&mut self) {
        self.static_models.clear();
        self.held_memo.clear();
    }

    /// The models and environment maps the kept outputs drew.
    #[cfg(feature = "3d")]
    fn models(&self) -> impl Iterator<Item = &String> {
        self.slots.values().flat_map(|slot| &slot.models)
    }

    pub(super) fn release(&mut self, recorder: &mut Recorder, key: SlotKey) {
        if let Some(slot) = self.slots.remove(&key) {
            recorder.release(slot.raster.tex);
            if let Some(full) = slot.full {
                recorder.release(full);
            }
            self.bytes -= slot.bytes;
        }
    }

    pub(super) fn drop_pending(&mut self, recorder: &mut Recorder) {
        for key in std::mem::take(&mut self.pending) {
            self.release(recorder, key);
        }
    }

    pub(super) fn drop_all(&mut self, recorder: &mut Recorder) {
        self.pending.clear();
        self.sources.clear();
        let keys: Vec<_> = self.slots.keys().copied().collect();
        for key in keys {
            self.release(recorder, key);
        }
    }

    pub(super) fn evict(&mut self, recorder: &mut Recorder, tick: u64, budget: u64) {
        let idle_before = tick.saturating_sub(RETAINED_IDLE_FRAMES);
        let over_budget = self.bytes > budget;
        if !over_budget && !tick.is_multiple_of(60) {
            return;
        }
        let mut idle: Vec<_> = self
            .slots
            .iter()
            .filter(|(_, slot)| slot.last_used != tick)
            .map(|(key, slot)| (slot.last_used, *key))
            .collect();
        idle.sort_unstable_by_key(|(last_used, _)| *last_used);
        for (last_used, key) in idle {
            if last_used > idle_before && self.bytes <= budget {
                break;
            }
            self.release(recorder, key);
        }
    }

    /// Whether a source something kept across frames read now has another identity, or resolves
    /// where it didn't or the other way round, as when a host replaces its bytes or a font arrives.
    pub(super) fn sources_changed(&self, media: Option<&dyn MediaStore>) -> bool {
        self.sources
            .iter()
            .any(|((kind, source), seen)| source_changed(media, *kind, source, seen.as_ref()))
    }
}

#[derive(Default)]
pub(super) struct TextState {
    pub(super) instances: HashMap<InstanceKey, Arc<TextInstance>>,
    pub(super) atlas: Option<TexRef>,
}

impl TextState {
    fn reset(&mut self, recorder: &mut Recorder) {
        self.instances.clear();
        if let Some(atlas) = self.atlas.take() {
            recorder.forget(atlas);
        }
    }
}

/// A CPU RGBA video frame, written to its texture before the frame's passes run.
pub(super) struct VideoUpload {
    pub(super) tex: TexRef,
    pub(super) frame: Arc<CpuMediaFrame>,
    pub(super) size: Size,
}

/// A plane of a CPU YUV frame, written to its texture before the frame's passes run.
pub(super) struct YuvUpload {
    pub(super) tex: TexRef,
    pub(super) frame: Arc<YuvFrame>,
    pub(super) offset: usize,
    pub(super) stride: u32,
    pub(super) shape: PlaneShape,
}

#[derive(Default)]
pub(super) struct MediaState {
    /// Adopted images and video planes by their texture, the tick that last bound each, and the
    /// key of an image's pixels.
    pub(super) textures: HashMap<usize, (TexRef, u64, Option<MediaTextureKey>)>,
    pub(super) video_uploads: Vec<VideoUpload>,
    pub(super) yuv_uploads: Vec<YuvUpload>,
    /// The texture of each video frame shown this frame, which samples of a `motion_blur` landing
    /// on the same source frame share.
    pub(super) video_frames: HashMap<VideoFrameKey, TexRef>,
    /// The slot each video source's GPU frames are imported into, by the source and how many
    /// distinct frames it imported earlier in the frame, and when it was last used.
    #[cfg(feature = "ffmpeg")]
    pub(super) video_imports: HashMap<(InstanceKey, u32), (u32, u64)>,
    /// Distinct GPU frames each video source has imported this frame.
    #[cfg(feature = "ffmpeg")]
    pub(super) frame_imports: HashMap<InstanceKey, u32>,
    #[cfg(feature = "ffmpeg")]
    pub(super) next_video_import: u32,
    /// How large this composition can show each image, found once it shows one.
    pub(super) shown_sizes: Option<Arc<crate::gpu::image_size::ShownSizes>>,
}

impl MediaState {
    fn begin_frame(&mut self) {
        self.video_uploads.clear();
        self.yuv_uploads.clear();
        self.video_frames.clear();
        #[cfg(feature = "ffmpeg")]
        self.frame_imports.clear();
    }

    /// Lets go of the textures `forget` picks by the tick that last bound each, so the media cache
    /// can free them.
    pub(super) fn forget_textures(
        &mut self,
        recorder: &mut Recorder,
        forget: impl Fn(u64) -> bool,
    ) {
        self.textures.retain(|_, (tex, last_used, _)| {
            let kept = !forget(*last_used);
            if !kept {
                recorder.forget(*tex);
            }
            kept
        });
    }

    /// Drops the import slots of video sources idle since long before `tick`.
    #[cfg(feature = "ffmpeg")]
    pub(super) fn evict_imports(&mut self, tick: u64, imports: &mut GpuMediaImports) {
        let idle_before = tick.saturating_sub(VIDEO_IMPORT_IDLE_FRAMES);
        self.video_imports.retain(|_, (slot, last_used)| {
            let kept = *last_used > idle_before;
            if !kept {
                imports.remove(*slot);
            }
            kept
        });
    }

    fn reset(&mut self, recorder: &mut Recorder) {
        self.video_uploads.clear();
        self.yuv_uploads.clear();
        self.video_frames.clear();
        #[cfg(feature = "ffmpeg")]
        {
            self.video_imports.clear();
            self.frame_imports.clear();
        }
        for (_, (tex, ..)) in self.textures.drain() {
            recorder.forget(tex);
        }
        self.shown_sizes = None;
    }
}

struct ShaderEntry {
    shader: CachedShader,
    last_used: u64,
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum ShaderCacheKey {
    Shared(u64),
    Source(Arc<str>),
}

/// Runtime shaders, parsed and assembled, by the composition shader or source and the inputs.
#[derive(Default)]
pub(super) struct ShaderCache {
    entries: HashMap<(ShaderCacheKey, usize), ShaderEntry>,
    next_shader: u64,
    /// Lookups that hashed a shader's source.
    #[cfg(feature = "test-hooks")]
    pub(super) source_lookups: u64,
}

impl ShaderCache {
    /// A composition shader is found by its id after its first lookup, which finds it by its
    /// source, so composition shaders of the same source share what was built from it.
    pub(super) fn cached_shader(
        &mut self,
        tick: u64,
        key: ShaderKey<'_>,
        inputs: usize,
        build: impl FnOnce() -> crate::Result<ShaderParts>,
    ) -> crate::Result<CachedShader> {
        if let ShaderKey::Shared(id, _) = key
            && let Some(shader) = self.lookup(tick, &(ShaderCacheKey::Shared(id), inputs))
        {
            return Ok(shader);
        }
        #[cfg(feature = "test-hooks")]
        {
            self.source_lookups += 1;
        }
        let source = match key {
            ShaderKey::Shared(_, source) => Arc::clone(source),
            ShaderKey::Source(source) => Arc::from(source),
        };
        let source_key = (ShaderCacheKey::Source(source), inputs);
        let shader = match self.lookup(tick, &source_key) {
            Some(shader) => shader,
            None => {
                let (interface, assembled, layout) = build()?;
                self.next_shader += 1;
                let shader = CachedShader {
                    id: self.next_shader,
                    interface,
                    assembled: assembled.into(),
                    layout: layout.into(),
                };
                self.insert(tick, source_key, shader.clone());
                shader
            }
        };
        if let ShaderKey::Shared(id, _) = key {
            self.insert(tick, (ShaderCacheKey::Shared(id), inputs), shader.clone());
        }
        Ok(shader)
    }

    fn lookup(&mut self, tick: u64, key: &(ShaderCacheKey, usize)) -> Option<CachedShader> {
        let entry = self.entries.get_mut(key)?;
        entry.last_used = tick;
        Some(entry.shader.clone())
    }

    fn insert(&mut self, tick: u64, key: (ShaderCacheKey, usize), shader: CachedShader) {
        self.entries.insert(
            key,
            ShaderEntry {
                shader,
                last_used: tick,
            },
        );
        // Only a new entry evicts, and not what this frame used: a frame using more than the limit
        // keeps them all, rather than compile again each frame.
        if self.entries.len() > MAX_SHADERS {
            let oldest = self
                .entries
                .iter()
                .filter(|(_, entry)| entry.last_used < tick)
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.entries.remove(&oldest);
            }
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
    }
}

/// The 3D scene cache.
#[cfg(feature = "3d")]
#[derive(Default)]
pub(super) struct SceneState {
    pub(super) cache: crate::node::scene::collect::SceneCache,
    /// The copies' placements of each `instance_3d` whose params don't read the frame.
    pub(super) placements: HashMap<GraphNodeKey, KeptPlacements>,
    /// Copies of copies an `instance_3d` holds, by the node and which of its lists they are.
    pub(super) flattened: HashMap<CopyListKey, KeptFlattened>,
    /// The instance buffers of lists later frames draw again, by the node and list.
    pub(super) buffers: HashMap<CopyListKey, KeptBuffer>,
    /// What `particles_3d` nodes sample their surfaces from and place their particles into.
    pub(super) particles: crate::node::scene::collect::ParticleCache,
    /// The scenes walked this frame for nodes recording more than one output from a walk, with
    /// what each walk found out for retention.
    pub(super) walks: HashMap<MemoKey, (Arc<crate::node::scene::collect::Collected>, HoldTrack)>,
    /// The colour and mattes drawn this frame for nodes recording both from one draw.
    pub(super) draws: HashMap<MemoKey, (Raster, Raster)>,
}

/// An `instance_3d` in a graph, and which of the lists of copies it draws from.
#[cfg(feature = "3d")]
pub(super) type CopyListKey = (GraphNodeKey, u32);

#[cfg(feature = "3d")]
type Placements = Arc<crate::node::scene::collect::Placements>;

/// The placements of each copy an `instance_3d` placed, for the count it placed.
#[cfg(feature = "3d")]
pub(super) struct KeptPlacements {
    pub(super) count: u32,
    pub(super) placements: Placements,
    pub(super) last_used: u64,
}

/// The placements of `outer`'s copies of the copies `inner` places from `first` on, `count` of
/// them, placed by `between` within each outer copy.
#[cfg(feature = "3d")]
pub(super) struct KeptFlattened {
    pub(super) outer: Placements,
    pub(super) between: lumen_3d::glam::Mat4,
    pub(super) inner: Placements,
    pub(super) range: (u32, u32),
    pub(super) placements: Placements,
    pub(super) last_used: u64,
}

/// An instance buffer holding `placements`.
#[cfg(feature = "3d")]
pub(super) struct KeptBuffer {
    pub(super) placements: Placements,
    pub(super) buffer: lumen_gpu::BufRef,
    pub(super) last_used: u64,
}

#[cfg(feature = "3d")]
impl SceneState {
    /// Drops the placements that neither this frame nor the last one read.
    fn begin_frame(&mut self, tick: u64) {
        self.walks.clear();
        self.draws.clear();
        self.placements
            .retain(|_, kept| kept.last_used.saturating_add(1) >= tick);
        self.flattened
            .retain(|_, kept| kept.last_used.saturating_add(1) >= tick);
    }

    /// Frees the instance buffers that neither this frame nor the last one drew.
    fn trim(&mut self, recorder: &mut Recorder, tick: u64) {
        self.buffers.retain(|_, kept| {
            let used = kept.last_used.saturating_add(1) >= tick;
            if !used {
                recorder.forget_buffer(kept.buffer);
            }
            used
        });
        self.particles.trim(recorder, tick);
    }

    fn reset(&mut self, recorder: &mut Recorder) {
        self.walks.clear();
        self.draws.clear();
        self.placements.clear();
        self.flattened.clear();
        for (_, kept) in self.buffers.drain() {
            recorder.forget_buffer(kept.buffer);
        }
        self.particles.release(recorder);
    }
}
