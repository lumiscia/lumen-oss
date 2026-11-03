//! Prototype of recording each frame's passes directly from the graph, behind
//! `LUMEN_FRAME_GRAPH=1`, instead of binding a compiled plan.
//!
//! A frame walks from the media output, evaluating each node once and recording its passes into
//! pooled textures, sized to what the node draws this frame. Outputs of static subtrees are
//! retained, so frames after the first skip those subtrees entirely.

use std::{
    collections::hash_map::Entry,
    sync::{Arc, Mutex},
};

use foldhash::HashMap;
use lumen_gpu::{
    Dispatch, LayoutEntry, LayoutKind, ProgramRef, Recorder, Res, SamplerRef, Size, TexRef,
    TextureDesc, TextureDomain, wgpu,
};

use crate::{
    composition::Composition,
    error::RenderError,
    expr::ExpressionContext,
    media::{MediaFrame, MediaStore},
    node::{NodeId, NodeKind, PortRef, compositing::clip::Clip, source::text::CompiledText},
    scope::{FrameTime, Routed, Scope},
};

use super::{
    AlphaMode, BoundFrame, FrameBindContext, RasterMetadata,
    cache::is_static_candidate,
    compiler::{self, CropParams, copyable_texture_desc, dispatch_for},
    renderer::{MediaTextureCache, fit_frame_to_rgba8},
};

const RGBA8: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Frames a retained output stays without being read.
const RETAINED_IDLE_FRAMES: u64 = 600;
/// Bytes retained outputs may hold before the least recently used are dropped.
const RETAINED_BUDGET_BYTES: u64 = 1 << 30;
/// Distinct runtime shader sources kept, parsed and assembled.
const MAX_SHADERS: usize = 64;

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
    pub(crate) metadata: RasterMetadata,
    /// The raster's logical bounds within its texture, when they are not all of it.
    pub(crate) bounds: Option<CropParams>,
}

impl Raster {
    pub(crate) fn straight(&self) -> bool {
        self.metadata.alpha_mode == AlphaMode::Unpremultiplied
    }

    pub(crate) fn origin(&self) -> [i32; 2] {
        [self.domain.data_rect.x, self.domain.data_rect.y]
    }
}

pub(crate) trait RecordNode {
    fn record(&self, cx: &mut FrameCx<'_>) -> crate::Result<Raster>;
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
    texts: HashMap<InstanceKey, Arc<CompiledText>>,
    shaders: HashMap<(Arc<str>, usize), ShaderEntry>,
    next_shader: u64,
    transparent: Option<TexRef>,
    dummy_float: Option<TexRef>,
    atlas: Option<TexRef>,
    media: HashMap<usize, TexRef>,
    video_uploads: Vec<(TexRef, MediaFrame, Size)>,
    output: Option<Raster>,
    /// The font resolver of each family text names, which retained text was drawn with.
    fonts: HashMap<String, Option<String>>,
    /// Passes reads of retained outputs skipped.
    skipped_passes: u64,
}

pub(crate) struct FrameGraph {
    pub(crate) recorder: Recorder,
    state: RecordState,
    recorded_output: Option<Arc<wgpu::Texture>>,
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
}

impl FrameGraph {
    pub(crate) fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self {
            recorder: Recorder::new(device, queue),
            state: RecordState::default(),
            recorded_output: None,
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
            };
            let output = PortRef::new(cx.scope.media_output()?, "output".to_string());
            let raster = cx.input(&output);
            let bound = cx.bound;
            // A font loaded during the walk may have changed what it measured.
            if font_generation(media) == generation {
                let raster = raster?;
                if bound.text_dropped_glyphs {
                    self.drop_pending();
                }
                self.state.output = Some(raster);
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

    /// Writes what the frame recorded to the GPU.
    pub(crate) fn upload(
        &mut self,
        bound: &BoundFrame,
        atlas_texture: Option<&wgpu::Texture>,
    ) -> crate::Result<()> {
        for (tex, frame, size) in self.state.video_uploads.drain(..) {
            let frame = cpu_frame(&frame)?;
            let rgba = fit_frame_to_rgba8(frame, size.width, size.height);
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
        if let Some(atlas) = atlas_texture {
            bound.write_atlas_regions(self.recorder.queue(), atlas);
        }
        bound.mark_uploaded()?;
        self.recorder.upload();
        Ok(())
    }

    /// Commits what the frame retained after submitting its passes, and sweeps what idled.
    pub(crate) fn finish_frame(&mut self) {
        self.state.pending.clear();
        self.evict_slots();
        self.recorder.end_frame();
        let recorder = &mut self.recorder;
        self.state.media.retain(|_, tex| {
            let kept = Arc::strong_count(recorder.texture(*tex)) > 1;
            if !kept {
                recorder.forget(*tex);
            }
            kept
        });
    }

    /// The frame's output, when it is not the one last read through the host renderer.
    pub(crate) fn changed_output(&mut self) -> Option<(Arc<wgpu::Texture>, TextureDesc)> {
        let raster = self.state.output?;
        let texture = Arc::clone(self.recorder.texture(raster.tex));
        if self
            .recorded_output
            .as_ref()
            .is_some_and(|output| Arc::ptr_eq(output, &texture))
        {
            return None;
        }
        self.recorded_output = Some(Arc::clone(&texture));
        let desc = TextureDesc {
            domain: raster.domain,
            format: texture.format(),
            usage: texture.usage(),
        };
        Some((texture, desc))
    }

    pub(crate) fn output(&self) -> Option<Raster> {
        self.state.output
    }

    pub(crate) fn reset(&mut self) {
        self.drop_slots();
        let state = &mut self.state;
        state.fingerprint = None;
        state.statics.clear();
        state.texts.clear();
        state.shaders.clear();
        state.memo.clear();
        state.full_frames.clear();
        state.output = None;
        self.recorded_output = None;
        if let Some(atlas) = state.atlas.take() {
            self.recorder.forget(atlas);
        }
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
        let over_budget = self.state.retained_bytes > RETAINED_BUDGET_BYTES;
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
            if last_used > idle_before && self.state.retained_bytes <= RETAINED_BUDGET_BYTES {
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
}

fn gpu_error(error: impl std::fmt::Display) -> crate::error::LumenError {
    RenderError::Gpu {
        details: error.to_string(),
    }
    .into()
}

impl<'a> FrameCx<'a> {
    pub(crate) fn expr(&self, node_id: NodeId, path: &str) -> ExpressionContext<'_> {
        self.scope
            .expr_context(self.frame, format!("{node_id}.{path}"), self.media)
    }

    pub(crate) fn bind_context(&self) -> FrameBindContext<'a> {
        FrameBindContext::recording(self.scope.clone(), self.frame, self.media)
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
        let settings = self.scope.render_settings;
        Size::new(settings.width.max(1), settings.height.max(1))
    }

    /// Records `port` as is, for readers that read a raster through its domain.
    pub(crate) fn input(&mut self, port: &PortRef) -> crate::Result<Raster> {
        if port.port != "output" {
            return Err(crate::error::PropertyError::MissingProperty {
                node_id: port.id,
                property_path: format!("output port `{}`", port.port),
            }
            .into());
        }
        let node = self.scope.node(port.id).ok_or(RenderError::MissingNode {
            frame: self.frame_number(),
            node_id: port.id,
        })?;
        let graph = std::ptr::from_ref(self.scope.graph) as usize;
        let is_static = self.retain_static && self.is_static(node);
        if is_static && let Some(slot) = self.st.slots.get_mut(&(graph, port.id)) {
            slot.last_used = self.st.tick;
            self.st.skipped_passes += slot.passes;
            return Ok(slot.raster);
        }
        let key = (graph, self.path, port.id);
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
                    self.retain(graph, port.id, raster, passes);
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

    /// Records `port` stored at its full display size, for readers that read texel for texel.
    pub(crate) fn input_full(&mut self, port: &PortRef) -> crate::Result<Raster> {
        let raster = self.input(port)?;
        if raster.domain.is_full_frame() {
            return Ok(raster);
        }
        let graph = std::ptr::from_ref(self.scope.graph) as usize;
        let slot_key = (graph, port.id);
        if let Some(slot) = self.st.slots.get(&slot_key)
            && let Some(full) = slot.full
        {
            return Ok(Raster {
                tex: full,
                domain: TextureDomain::full_frame(raster.domain.display_size()),
                bounds: None,
                ..raster
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
            metadata: raster.metadata,
            bounds: None,
        };
        self.st.full_frames.insert(key, full);
        full
    }

    /// A filter's source when it reads and writes the source's domain: transparent texels of
    /// straight sources may still carry colour, so those are read at full frame.
    pub(crate) fn premultiplied_source(&mut self, port: &PortRef) -> crate::Result<Raster> {
        let source = self.input(port)?;
        Ok(if source.straight() {
            self.full_frame(source)
        } else {
            source
        })
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
            metadata: RasterMetadata::default(),
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
        build: impl FnOnce(&Self) -> crate::Result<CompiledText>,
    ) -> crate::Result<Arc<CompiledText>> {
        let key = (
            std::ptr::from_ref(self.scope.graph) as usize,
            self.path,
            node_id,
        );
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
        build: impl FnOnce() -> crate::Result<(
            Arc<dyn std::any::Any + Send + Sync>,
            String,
            Vec<LayoutEntry>,
        )>,
    ) -> crate::Result<CachedShader> {
        let tick = self.st.tick;
        let key = (Arc::<str>::from(source), inputs);
        let entry = match self.st.shaders.entry(key) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(vacant) => {
                let (interface, assembled, layout) = build()?;
                self.st.next_shader += 1;
                vacant.insert(ShaderEntry {
                    shader: CachedShader {
                        id: self.st.next_shader,
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
        if self.st.shaders.len() > MAX_SHADERS {
            let oldest = self
                .st
                .shaders
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.st.shaders.remove(&oldest);
            }
        }
        Ok(shader)
    }

    pub(crate) fn dynamic_program(
        &mut self,
        label: &str,
        shader: &CachedShader,
    ) -> crate::Result<ProgramRef> {
        self.rec
            .dynamic_compute_program(
                label,
                shader.id,
                || shader.assembled.to_string(),
                "cs_main",
                &shader.layout,
            )
            .map_err(gpu_error)
    }

    /// An image's texture, uploaded once and kept in the renderer's media cache.
    pub(crate) fn image_texture(
        &mut self,
        key: super::MediaTextureKey,
        frame: &MediaFrame,
        size: Size,
    ) -> crate::Result<TexRef> {
        let cpu = cpu_frame(frame)?;
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
        let pointer = Arc::as_ptr(&texture) as usize;
        Ok(match self.st.media.get(&pointer) {
            Some(tex) => *tex,
            None => {
                let tex = self.rec.adopt(texture);
                self.st.media.insert(pointer, tex);
                tex
            }
        })
    }

    /// A pooled texture a video frame is written to before the frame's passes run.
    pub(crate) fn video_texture(&mut self, frame: MediaFrame, size: Size) -> crate::Result<TexRef> {
        cpu_frame(&frame)?;
        let tex = self
            .rec
            .acquire(size, RGBA8, TextureDesc::sampled(size, RGBA8).usage);
        self.st.video_uploads.push((tex, frame, size));
        Ok(tex)
    }

    fn is_static(&mut self, node: &NodeKind) -> bool {
        let graph = std::ptr::from_ref(self.scope.graph) as usize;
        let key = (graph, node.id());
        if let Some(known) = self.st.statics.get(&key) {
            return *known;
        }
        self.st.statics.insert(key, false);
        let result = is_static_candidate(node)
            && !matches!(node, NodeKind::Clip(_) | NodeKind::TimeRemap(_))
            && node
                .input_ports()
                .iter()
                .filter(|port| !port.is_empty())
                .all(|port| {
                    self.scope
                        .node(port.id)
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
        let result = self.input(&routed.port);
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
        let Some(routed) = routed.filter(|routed| !routed.port.is_empty()) else {
            return Ok(self.transparent(self.canvas_size()));
        };
        let settings = routed.scope.render_settings;
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
            compiler::clip_crop_shader(),
            raster.tex,
            &offset,
            tex,
            size,
        )?;
        Ok(Raster {
            tex,
            domain,
            metadata: raster.metadata,
            bounds: None,
        })
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
        NodeKind::Curves(node) => node.record(cx),
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
        NodeKind::Memo(_) => not_migrated("memo"),
        NodeKind::AlphaPremultiply(node) => node.record(cx),
        NodeKind::ChannelShuffle(node) => node.record(cx),
    }
}

fn cpu_frame(frame: &MediaFrame) -> crate::Result<&Arc<crate::media::CpuMediaFrame>> {
    match frame {
        MediaFrame::CpuRgba(frame) => Ok(frame),
        #[cfg(feature = "ffmpeg")]
        MediaFrame::GpuVideo(_) => not_migrated("GPU media frames").map(|_| unreachable!()),
    }
}

fn not_migrated(kind: &str) -> crate::Result<Raster> {
    Err(RenderError::Gpu {
        details: format!("frame graph: {kind} is not migrated"),
    }
    .into())
}
