use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::{
    composition::Composition,
    error::RenderError,
    gpu::{
        BoundFrame, CompileContext, CompiledComposition, FrameBindContext, MediaTextureKey,
        RasterHandle, TimeRemapPath,
        cache::{BindCache, CacheState, skipped_count},
    },
    media::{CpuMediaFrame, MediaFrame, MediaStore},
    node::{
        Deferred, DelegateEvalContext, DelegateEvaluable, NodeId, NodeKind,
        source::text_content::TextContentDelegate,
    },
    scope::{FrameTime, Routed, Scope},
};

#[cfg(feature = "ffmpeg")]
use super::media::{GpuMediaFrameImporter, GpuMediaImports};

/// Everything a frame can change that alters the compiled plan's structure, plus the composition
/// it was compiled from, whose params the plan binds.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CompiledPlanKey {
    /// `Composition::fingerprint` of the composition, covering every node's literal params and
    /// expression sources along with the canvases and timelines textures were sized for.
    composition: u64,
    /// Whether the frame is a whole frame of the timeline, so animated sources may be sized to
    /// their bounds over it.
    on_timeline: bool,
    /// The fonts the store's text system held once the literal font families of the frame's text
    /// were loaded, which text bounds are measured with; loading any face may change how text is
    /// shaped. `None` without text, or when the text system is unavailable.
    text_font_generation: Option<u64>,
    entries: Vec<PlanKeyEntry>,
}

/// A node instance's structural choice; the compiler builds one instance per path of time remaps
/// and clips.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct PlanKeyEntry {
    time_remaps: TimeRemapPath,
    node_id: NodeId,
    choice: PlanChoice,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum PlanChoice {
    SwitchSelection(Option<usize>),
    /// Whether a clip shows, which decides whether anything upstream of it is compiled.
    ClipShown(bool),
    /// The resolver the media store gave a font family of a text node or its spans, which its
    /// bounds were measured with.
    TextFont(String),
    ShaderSource(u64),
    /// The size of the media a media input shows, which its texture is compiled to.
    MediaSize(u32, u32),
}

/// Estimated GPU memory compiled plans may hold before least recently used ones are dropped.
pub const DEFAULT_PLAN_CACHE_BUDGET_BYTES: u64 = 1 << 30;

/// Rejected plan estimates remembered before they are all forgotten.
const MAX_REJECTED_PLANS: usize = 256;

/// Estimated GPU memory image textures no plan binds may hold before the least recently used are
/// freed.
pub const DEFAULT_MEDIA_TEXTURE_CACHE_BUDGET_BYTES: u64 = 256 << 20;

struct CachedMediaTexture {
    texture: Arc<lumen_gpu::wgpu::Texture>,
    last_used: u64,
}

/// Image textures plans share, which are never written once cached. Replaced images get new keys,
/// so textures no plan binds are freed least recently used first once they exceed the budget;
/// textures a plan binds don't count against it and always stay.
pub(crate) struct MediaTextureCache {
    textures: HashMap<MediaTextureKey, CachedMediaTexture>,
    budget: u64,
    clock: u64,
    upload_count: u64,
}

impl MediaTextureCache {
    fn new() -> Self {
        Self {
            textures: HashMap::new(),
            budget: DEFAULT_MEDIA_TEXTURE_CACHE_BUDGET_BYTES,
            clock: 0,
            upload_count: 0,
        }
    }

    pub(super) fn get(&mut self, key: &MediaTextureKey) -> Option<Arc<lumen_gpu::wgpu::Texture>> {
        self.clock += 1;
        let cached = self.textures.get_mut(key)?;
        cached.last_used = self.clock;
        Some(Arc::clone(&cached.texture))
    }

    pub(super) fn insert(&mut self, key: MediaTextureKey, texture: Arc<lumen_gpu::wgpu::Texture>) {
        self.upload_count += 1;
        self.clock += 1;
        self.textures.insert(
            key,
            CachedMediaTexture {
                texture,
                last_used: self.clock,
            },
        );
    }

    /// Frees unbound textures, least recently used first, until those left fit the budget. Call
    /// after anything that may have released a plan's binding.
    fn evict(&mut self) {
        let mut unbound: Vec<_> = self
            .textures
            .iter()
            .filter(|(_, cached)| Arc::strong_count(&cached.texture) == 1)
            .map(|(key, cached)| (cached.last_used, key.clone()))
            .collect();
        let mut held: u64 = unbound.iter().map(|(_, key)| texture_bytes(key)).sum();
        unbound.sort_unstable_by_key(|(last_used, _)| std::cmp::Reverse(*last_used));
        while held > self.budget {
            let Some((_, key)) = unbound.pop() else {
                return;
            };
            if let Some(evicted) = self.textures.remove(&key) {
                evicted.texture.destroy();
            }
            held -= texture_bytes(&key);
        }
    }

    fn destroy_all(&mut self) {
        for cached in self.textures.values() {
            cached.texture.destroy();
        }
        self.textures.clear();
    }
}

pub(super) fn create_media_texture(
    device: &lumen_gpu::wgpu::Device,
    size: lumen_gpu::Size,
) -> Arc<lumen_gpu::wgpu::Texture> {
    Arc::new(device.create_texture(&lumen_gpu::wgpu::TextureDescriptor {
        label: Some("lumen media frame"),
        size: size.as_extent(),
        mip_level_count: 1,
        sample_count: 1,
        dimension: lumen_gpu::wgpu::TextureDimension::D2,
        format: lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
        usage: lumen_gpu::wgpu::TextureUsages::TEXTURE_BINDING
            | lumen_gpu::wgpu::TextureUsages::COPY_DST
            | lumen_gpu::wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    }))
}

fn texture_bytes(key: &MediaTextureKey) -> u64 {
    u64::from(key.width) * u64::from(key.height) * 4
}

struct PreparedComposition {
    renderer: lumen_gpu::Renderer,
    compiled: CompiledComposition,
    /// Output kept from frames submitted to this plan.
    cache: CacheState,
    current_media_textures: HashMap<lumen_gpu::TextureId, MediaTextureKey>,
    #[cfg(feature = "ffmpeg")]
    media_imports: GpuMediaImports,
    estimated_bytes: u64,
    last_used: u64,
}

/// What recording has cost, across every frame so far.
#[derive(Debug, Clone, Copy)]
pub struct FrameGraphStats {
    /// Bytes of textures the graph adopted, such as external targets and images.
    pub adopted_bytes: u64,
    pub recorder: lumen_gpu::RecorderStats,
    pub held_bytes: u64,
    pub retained_bytes: u64,
    pub bind_groups: usize,
    pub programs: usize,
}

pub struct GpuCompositionRenderer {
    renderer: lumen_gpu::Renderer,
    compiled_plans: foldhash::HashMap<CompiledPlanKey, PreparedComposition>,
    active_key: Option<CompiledPlanKey>,
    plan_cache_bytes: u64,
    plan_cache_budget: u64,
    plan_use_clock: u64,
    /// Fingerprint of the composition the cache was last pruned against.
    pruned_fingerprint: Option<u64>,
    /// Estimated sizes of lookahead plans that did not fit the budget, so later windows can skip
    /// them without compiling. Valid only for the composition with this fingerprint.
    rejected_plan_bytes: foldhash::HashMap<CompiledPlanKey, u64>,
    rejected_fingerprint: Option<u64>,
    plan_compile_count: u64,
    submitted_pass_count: u64,
    cached_pass_count: u64,
    bind_sequence: u64,
    text_atlas: Arc<Mutex<lumen_text::GlyphAtlas>>,
    text_atlas_texture: Option<Arc<lumen_gpu::wgpu::Texture>>,
    output_format: lumen_gpu::wgpu::TextureFormat,
    reuse_static_output: bool,
    full_frame_sources: bool,
    media_texture_cache: MediaTextureCache,
    #[cfg(feature = "ffmpeg")]
    gpu_media_importer: GpuMediaFrameImporter,
    /// Records frames instead of binding compiled plans.
    frame_graph: Option<Box<super::frame_graph::FrameGraph>>,
}

impl GpuCompositionRenderer {
    pub async fn new() -> crate::Result<Self> {
        let renderer = lumen_gpu::Renderer::new()
            .await
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        tracing::debug!(target: "lumen_render", "created gpu composition renderer");
        Ok(Self::from_renderer(renderer))
    }

    pub fn from_device(device: lumen_gpu::wgpu::Device, queue: lumen_gpu::wgpu::Queue) -> Self {
        Self::from_renderer(lumen_gpu::Renderer::from_device(device, queue))
    }

    pub fn from_renderer(renderer: lumen_gpu::Renderer) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let frame_graph = std::env::var_os("LUMEN_FRAME_GRAPH").is_some_and(|value| value == "1");
        #[cfg(target_arch = "wasm32")]
        let frame_graph = false;
        let recording = frame_graph.then(|| {
            Box::new(super::frame_graph::FrameGraph::new(
                renderer.device.clone(),
                renderer.queue.clone(),
            ))
        });
        Self {
            frame_graph: recording,
            renderer,
            compiled_plans: foldhash::HashMap::default(),
            active_key: None,
            plan_cache_bytes: 0,
            plan_cache_budget: DEFAULT_PLAN_CACHE_BUDGET_BYTES,
            plan_use_clock: 0,
            pruned_fingerprint: None,
            rejected_plan_bytes: foldhash::HashMap::default(),
            rejected_fingerprint: None,
            plan_compile_count: 0,
            submitted_pass_count: 0,
            cached_pass_count: 0,
            bind_sequence: 0,
            text_atlas: Arc::new(Mutex::new(lumen_text::GlyphAtlas::new(
                super::types::TEXT_ATLAS_SIZE,
            ))),
            text_atlas_texture: None,
            output_format: lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            reuse_static_output: true,
            full_frame_sources: false,
            media_texture_cache: MediaTextureCache::new(),
            #[cfg(feature = "ffmpeg")]
            gpu_media_importer: GpuMediaFrameImporter::default(),
        }
    }

    /// Records frames rather than binding plans, or goes back to plans.
    pub fn set_frame_graph(&mut self, enabled: bool) {
        if enabled == self.frame_graph.is_some() {
            return;
        }
        self.release_resources();
        self.frame_graph = enabled.then(|| {
            Box::new(super::frame_graph::FrameGraph::new(
                self.renderer.device.clone(),
                self.renderer.queue.clone(),
            ))
        });
    }

    /// Whether frames are recorded rather than bound to plans.
    pub fn frame_graph_enabled(&self) -> bool {
        self.frame_graph.is_some()
    }

    /// Bounds what recorded frames keep in pooled textures.
    #[doc(hidden)]
    pub fn set_frame_graph_pool_budget(&mut self, bytes: u64) {
        if let Some(graph) = &mut self.frame_graph {
            graph.recorder.set_pool_budget(bytes);
        }
    }

    /// The size, format and usage of the output of the frame last rendered.
    pub fn output_desc(&self) -> Option<lumen_gpu::TextureDesc> {
        match &self.frame_graph {
            Some(graph) => graph.output_desc(),
            None => {
                let output = self.compiled()?.output;
                self.gpu_renderer().texture_desc(output.texture)
            }
        }
    }

    /// The textures the frame last recorded used, before they shared any.
    #[doc(hidden)]
    pub fn recorded_texture_sizes(&self) -> Vec<lumen_gpu::Size> {
        self.frame_graph
            .as_ref()
            .map_or_else(Vec::new, |graph| graph.recorder.frame_texture_sizes())
    }

    /// The passes the frame last recorded.
    #[doc(hidden)]
    pub fn recorded_pass_count(&self) -> usize {
        self.frame_graph
            .as_ref()
            .map_or(0, |graph| graph.recorder.pass_count())
    }

    /// What recording has cost so far, when frames are recorded.
    pub fn frame_graph_stats(&self) -> Option<FrameGraphStats> {
        self.frame_graph.as_ref().map(|graph| FrameGraphStats {
            recorder: graph.recorder.stats(),
            held_bytes: graph.recorder.held_bytes(),
            adopted_bytes: graph.recorder.adopted_bytes(),
            retained_bytes: graph.retained_bytes(),
            bind_groups: graph.recorder.bind_group_count(),
            programs: graph.recorder.program_count(),
        })
    }

    pub fn compile(&mut self, composition: &Composition) -> crate::Result<()> {
        self.compile_with_output_format(composition, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm)
    }

    pub fn compile_with_output_format(
        &mut self,
        composition: &Composition,
        output_format: lumen_gpu::wgpu::TextureFormat,
    ) -> crate::Result<()> {
        self.reset_compiled(output_format);
        if let Some(graph) = &mut self.frame_graph {
            return graph.prepare(composition, output_format);
        }
        let key = self.compiled_plan_key(composition, 0.0, None)?;
        self.plan_compile_count += 1;
        let compiled = CompileContext::with_options(composition, 0.0, None, output_format, true)
            .render_static(!self.reuse_static_output)
            .full_frame_sources(self.full_frame_sources)
            .compile_with_atlas(Arc::clone(&self.text_atlas))?;
        self.prepare_compiled(key, compiled)
    }

    /// Sets whether nodes without expressions or media keep their output from earlier frames
    /// instead of rendering it again, which is on by default. Turning it off disables that
    /// automatic reuse, as when measuring render cost.
    /// Drops the compiled plans when it changes.
    pub fn set_static_output_reuse(&mut self, enabled: bool) {
        if self.reuse_static_output != enabled {
            self.reuse_static_output = enabled;
            self.reset_compiled(self.output_format);
        }
    }

    /// Sets whether every source fills the canvas instead of being sized to its bounds, which
    /// gives tests a reference render for those bounds. Drops the compiled plans when it changes.
    #[doc(hidden)]
    pub fn set_full_frame_sources(&mut self, enabled: bool) {
        if self.full_frame_sources != enabled {
            self.full_frame_sources = enabled;
            self.reset_compiled(self.output_format);
        }
    }

    /// Sets the estimated GPU memory compiled plans may hold. Rendering drops least recently used
    /// plans past it, and lookahead skips plans that would not fit. The current plan always stays.
    pub fn set_plan_cache_budget(&mut self, bytes: u64) {
        self.plan_cache_budget = bytes;
        let active = self.active_key.clone();
        self.evict_over_budget(&[active.as_ref()]);
    }

    /// Estimated GPU memory held by compiled plans, excluding the shared text atlas and media cache.
    pub fn plan_cache_bytes(&self) -> u64 {
        match &self.frame_graph {
            Some(graph) => graph.recorder.held_bytes(),
            None => self.plan_cache_bytes,
        }
    }

    pub fn cached_plan_count(&self) -> usize {
        self.compiled_plans.len()
    }

    /// Number of times a plan has been built on the CPU, whether or not it was kept.
    pub fn plan_compile_count(&self) -> u64 {
        self.plan_compile_count
    }

    /// Number of programs built for prepared plans, rather than reused from another plan.
    pub fn created_program_count(&self) -> u64 {
        match &self.frame_graph {
            Some(graph) => graph.recorder.stats().programs_created,
            None => self.renderer.created_program_count(),
        }
    }

    /// Bind groups the plans still cached have rebuilt.
    pub fn plan_bind_group_rebuilds(&self) -> u64 {
        self.compiled_plans
            .values()
            .map(|prepared| prepared.renderer.bind_group_rebuilds())
            .sum()
    }

    /// Number of passes in the plans of submitted frames, whether they ran or not.
    pub fn submitted_pass_count(&self) -> u64 {
        match &self.frame_graph {
            Some(graph) => graph.recorder.stats().passes,
            None => self.submitted_pass_count,
        }
    }

    /// Number of passes submitted frames skipped because the output they would write was kept
    /// from an earlier frame: that of nodes without frame-dependent params.
    pub fn cached_pass_count(&self) -> u64 {
        match &self.frame_graph {
            Some(graph) => graph.skipped_passes(),
            None => self.cached_pass_count,
        }
    }

    /// Number of rejected lookahead plans whose estimated size is remembered.
    #[doc(hidden)]
    pub fn rejected_plan_count(&self) -> usize {
        self.rejected_plan_bytes.len()
    }

    /// Number of image textures kept for plans to share.
    #[doc(hidden)]
    pub fn cached_media_texture_count(&self) -> usize {
        self.media_texture_cache.textures.len()
    }

    /// Number of image textures created and uploaded, rather than reused from the cache.
    #[doc(hidden)]
    pub fn media_texture_upload_count(&self) -> u64 {
        self.media_texture_cache.upload_count
    }

    /// Sets the estimated GPU memory image textures no plan binds may hold, which textures a plan
    /// binds don't count against. The least recently used unbound ones past it are freed as plans
    /// release them.
    pub fn set_media_texture_cache_budget(&mut self, bytes: u64) {
        self.media_texture_cache.budget = bytes;
        self.media_texture_cache.evict();
    }

    pub fn compile_with_media<M: MediaStore>(
        &mut self,
        composition: &Composition,
        media: &M,
        output_format: lumen_gpu::wgpu::TextureFormat,
    ) -> crate::Result<()> {
        self.reset_compiled(output_format);
        if let Some(graph) = &mut self.frame_graph {
            return graph.prepare(composition, output_format);
        }
        // Frame 0 is only compiled ahead of time. The frames being rendered may never reach a
        // source it can't open, and those that do fail with the same error.
        match self.ensure_compiled_for_frame(composition, 0.0, Some(media), PlanUse::Render) {
            Err(crate::error::LumenError::Media(error)) => {
                tracing::debug!(target: "lumen_render", %error, "frame 0 did not compile ahead of time");
                Ok(())
            }
            result => result.map(drop),
        }
    }

    /// Drops every compiled plan and cached media texture, destroying the GPU textures and
    /// buffers this renderer exclusively owns instead of waiting for them to be garbage collected.
    /// Retains the shared text atlas (64 MiB once allocated) and cached glyphs across recompilation.
    pub fn release_resources(&mut self) {
        if let Some(graph) = &mut self.frame_graph {
            graph.reset();
        }
        self.bind_sequence += 1;
        self.media_texture_cache.destroy_all();
        for prepared in self.compiled_plans.values() {
            for resource in prepared.compiled.plan.textures() {
                if let Some(texture) = prepared.renderer.texture_arc(resource.id)
                    && Arc::strong_count(&texture) == 2
                {
                    texture.destroy();
                }
            }
            for resource in prepared.compiled.plan.buffers() {
                if let Some(buffer) = prepared.renderer.buffer(resource.id) {
                    buffer.destroy();
                }
            }
        }
        self.compiled_plans.clear();
        self.plan_cache_bytes = 0;
        self.rejected_plan_bytes.clear();
        self.active_key = None;
    }

    /// Drops compiled plans, cached media, and the text atlas, reclaiming GPU memory and cached glyphs.
    pub fn release_text_atlas(&mut self) {
        self.release_resources();
        if let Some(texture) = self.text_atlas_texture.take() {
            texture.destroy();
        }
        self.text_atlas = Arc::new(Mutex::new(lumen_text::GlyphAtlas::new(
            super::types::TEXT_ATLAS_SIZE,
        )));
    }

    fn reset_compiled(&mut self, output_format: lumen_gpu::wgpu::TextureFormat) {
        tracing::debug!(
            target: "lumen_render",
            ?output_format,
            cached_plans = self.compiled_plans.len(),
            media_textures = self.media_texture_cache.textures.len(),
            "reset compiled composition"
        );
        if let Some(graph) = &mut self.frame_graph {
            graph.reset();
        }
        self.bind_sequence += 1;
        self.output_format = output_format;
        self.compiled_plans.clear();
        self.media_texture_cache.evict();
        self.plan_cache_bytes = 0;
        self.pruned_fingerprint = None;
        self.rejected_plan_bytes.clear();
        self.rejected_fingerprint = None;
        self.active_key = None;
    }

    fn prepare_compiled(
        &mut self,
        key: CompiledPlanKey,
        compiled: CompiledComposition,
    ) -> crate::Result<()> {
        tracing::debug!(
            target: "lumen_render",
            key_entries = key.entries.len(),
            textures = compiled.plan.textures().len(),
            buffers = compiled.plan.buffers().len(),
            programs = compiled.plan.programs().len(),
            passes = compiled.plan.passes().len(),
            compiled_nodes = compiled.compiled_nodes.len(),
            estimated_bytes = estimated_plan_bytes(&compiled),
            "prepare compiled render plan"
        );
        let mut renderer = self.renderer.sibling();
        renderer
            .prepare_plan(&compiled.plan)
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        if let Some(id) = compiled.text_atlas_texture {
            let device = &self.renderer.device;
            let texture = self
                .text_atlas_texture
                .get_or_insert_with(|| Arc::new(super::types::create_text_atlas_texture(device)));
            renderer
                .replace_texture_arc(id, Arc::clone(texture), super::types::text_atlas_desc())
                .map_err(|error| RenderError::Gpu {
                    details: error.to_string(),
                })?
                .destroy();
        }
        let estimated_bytes = estimated_plan_bytes(&compiled);
        self.plan_cache_bytes += estimated_bytes;
        self.plan_use_clock += 1;
        if let Some(replaced) = self.compiled_plans.insert(
            key.clone(),
            PreparedComposition {
                renderer,
                compiled,
                cache: CacheState::default(),
                current_media_textures: HashMap::new(),
                #[cfg(feature = "ffmpeg")]
                media_imports: GpuMediaImports::default(),
                estimated_bytes,
                last_used: self.plan_use_clock,
            },
        ) {
            self.plan_cache_bytes -= replaced.estimated_bytes;
            drop(replaced);
            self.media_texture_cache.evict();
        }
        self.active_key = Some(key);
        Ok(())
    }

    /// Drops plans compiled from a composition other than the one with `fingerprint`, scanning
    /// the cache only when it changed since the last prune. Their GPU resources are freed once
    /// nothing else holds them. Rendering prunes after preparing the current plan, so it reuses
    /// the programs of the plans it replaces.
    fn prune_stale_plans(&mut self, fingerprint: u64) {
        if self.pruned_fingerprint.replace(fingerprint) == Some(fingerprint) {
            return;
        }
        let cached_plans = self.compiled_plans.len();
        let plan_cache_bytes = &mut self.plan_cache_bytes;
        self.compiled_plans.retain(|key, prepared| {
            let keep = key.composition == fingerprint;
            if !keep {
                *plan_cache_bytes -= prepared.estimated_bytes;
            }
            keep
        });
        if self.compiled_plans.len() == cached_plans {
            return;
        }
        self.media_texture_cache.evict();
        tracing::debug!(
            target: "lumen_render",
            pruned = cached_plans - self.compiled_plans.len(),
            "prune compiled plans for an edited composition"
        );
        if self
            .active_key
            .as_ref()
            .is_some_and(|key| !self.compiled_plans.contains_key(key))
        {
            self.active_key = None;
        }
    }

    /// Drops least recently used plans other than `pinned` until the cache fits its budget.
    fn evict_over_budget(&mut self, pinned: &[Option<&CompiledPlanKey>]) {
        while self.plan_cache_bytes > self.plan_cache_budget {
            let Some(key) = self
                .compiled_plans
                .iter()
                .filter(|(key, _)| !pinned.contains(&Some(*key)))
                .min_by_key(|(_, prepared)| prepared.last_used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            let evicted = self.compiled_plans.remove(&key).expect("cached plan");
            self.plan_cache_bytes -= evicted.estimated_bytes;
            tracing::debug!(
                target: "lumen_render",
                bytes = evicted.estimated_bytes,
                cached_bytes = self.plan_cache_bytes,
                budget = self.plan_cache_budget,
                "evict least recently used compiled plan"
            );
        }
        self.media_texture_cache.evict();
    }

    /// Compiles the plan for `frame` ahead of rendering it, unless it would not fit the plan cache
    /// budget. Never drops a cached plan, including stale ones, or changes the current one.
    pub fn precompile_frame<M: MediaStore>(
        &mut self,
        composition: &Composition,
        frame: u32,
        media: &M,
    ) -> crate::Result<()> {
        if self.frame_graph.is_some() {
            return Ok(());
        }
        self.ensure_compiled_for_frame(
            composition,
            f64::from(frame),
            Some(media),
            PlanUse::Lookahead,
        )
        .map(drop)
    }

    pub fn precompile_frame_window<M: MediaStore>(
        &mut self,
        composition: &Composition,
        start_frame: u32,
        frame_count: u32,
        media: &M,
    ) -> crate::Result<()> {
        if self.frame_graph.is_some() {
            return Ok(());
        }
        tracing::trace!(
            target: "lumen_render",
            start_frame,
            frame_count,
            "precompile frame window"
        );
        for offset in 0..frame_count {
            let frame = start_frame.saturating_add(offset);
            // Later frames are unlikely to fit either, so stop rather than compile and discard them.
            if !self.ensure_compiled_for_frame(
                composition,
                f64::from(frame),
                Some(media),
                PlanUse::Lookahead,
            )? {
                break;
            }
        }
        Ok(())
    }

    pub fn render_frame<M: MediaStore>(
        &mut self,
        composition: &Composition,
        frame: u32,
        media: &M,
    ) -> crate::Result<RasterHandle> {
        self.render_frame_submitted(composition, frame, media)
            .map(|(raster, _submission)| raster)
    }

    /// Renders at a fractional frame time; `frame` is in composition frames, not seconds.
    pub fn render_frame_at<M: MediaStore>(
        &mut self,
        composition: &Composition,
        frame: f64,
        media: &M,
    ) -> crate::Result<RasterHandle> {
        let bound = self.bind_frame_at(composition, frame, media)?;
        self.submit_bound_frame(&bound)
            .map(|(raster, _submission)| raster)
    }

    pub fn render_frame_submitted<M: MediaStore>(
        &mut self,
        composition: &Composition,
        frame: u32,
        media: &M,
    ) -> crate::Result<(RasterHandle, lumen_gpu::wgpu::SubmissionIndex)> {
        let span = tracing::trace_span!(target: "lumen_render", "render frame", frame);
        let _entered = span.enter();
        let bound = self.bind_frame(composition, frame, media)?;
        tracing::trace!(
            target: "lumen_render",
            frame,
            buffer_uploads = bound.buffer_upload_count(),
            texture_uploads = bound.texture_upload_count(),
            media_textures = bound.media_textures().len(),
            "bound frame"
        );
        self.submit_bound_frame(&bound)
    }

    pub fn render_frame_into_external<M: MediaStore>(
        &mut self,
        composition: &Composition,
        frame: u32,
        media: &M,
        external: lumen_gpu::ExternalTexture,
    ) -> crate::Result<lumen_gpu::SubmittedExternalTexture> {
        let span = tracing::trace_span!(target: "lumen_render", "render frame into external texture", frame);
        let _entered = span.enter();
        if self.frame_graph.is_some() {
            return self.record_into_external(composition, frame, media, external);
        }
        let bound = self.bind_frame(composition, frame, media)?;
        self.upload_bound_frame(&bound)?;
        let prepared = self.active_prepared_mut()?;
        let submitted = prepared
            .renderer
            .submit_plan_with_external_texture_skipping(
                &prepared.compiled.plan,
                prepared.compiled.output.texture,
                external,
                prepared.cache.skip(),
            )
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        self.count_submitted();
        Ok(submitted)
    }

    /// Records a frame whose output is `external`, which then stays with the caller: the graph
    /// holds no other claim on it once it is the only one left holding the texture.
    fn record_into_external<M: MediaStore>(
        &mut self,
        composition: &Composition,
        frame: u32,
        media: &M,
        external: lumen_gpu::ExternalTexture,
    ) -> crate::Result<lumen_gpu::SubmittedExternalTexture> {
        let texture = external.texture_arc();
        let graph = self.frame_graph.as_mut().expect("recording");
        graph.set_external(external);
        let bound = self.bind_frame(composition, frame, media);
        let graph = self.frame_graph.as_mut().expect("recording");
        graph.take_external();
        self.upload_bound_frame(&bound?)?;
        let graph = self.frame_graph.as_mut().expect("recording");
        let submission = graph.recorder.submit().map_err(|error| RenderError::Gpu {
            details: error.to_string(),
        })?;
        graph.finish_frame();
        Ok(lumen_gpu::SubmittedExternalTexture::new(
            submission, texture,
        ))
    }

    pub fn bind_frame<M: MediaStore>(
        &mut self,
        composition: &Composition,
        frame: u32,
        media: &M,
    ) -> crate::Result<BoundFrame> {
        self.bind_frame_at(composition, f64::from(frame), media)
    }

    /// Binds at a fractional frame time. Expressions see the fractional `frame`/`time`; video
    /// inputs show the source frame displayed at that instant.
    pub fn bind_frame_at<M: MediaStore>(
        &mut self,
        composition: &Composition,
        frame: f64,
        media: &M,
    ) -> crate::Result<BoundFrame> {
        if !(frame.is_finite() && frame >= 0.0) {
            return Err(RenderError::FrameOutOfRange {
                frame: frame.max(0.0) as u32,
                duration_frames: composition.timeline().duration_frames,
            }
            .into());
        }
        self.bind_sequence += 1;
        if let Some(graph) = &mut self.frame_graph {
            let mut bound = graph.record(super::frame_graph::RecordInput {
                composition,
                frame,
                media: Some(media),
                output_format: self.output_format,
                full_frame_sources: self.full_frame_sources,
                retain_static: self.reuse_static_output,
                media_cache: &mut self.media_texture_cache,
                text_atlas: &self.text_atlas,
                atlas_texture: &mut self.text_atlas_texture,
                max_rebinds: max_font_rebinds(composition),
                #[cfg(feature = "ffmpeg")]
                renderer: &self.renderer,
                #[cfg(feature = "ffmpeg")]
                importer: &mut self.gpu_media_importer,
            })?;
            bound.recorded = Some(self.bind_sequence);
            return Ok(bound);
        }
        // A font loaded while the plan compiles or the frame binds, here or by another renderer,
        // can widen text past the bounds and glyph counts the plan measured. The plan is then
        // keyed by the new font generation, so the frame is bound again to it; a font loads
        // once, so this settles.
        let mut rebinds = 0;
        let max_rebinds = max_font_rebinds(composition);
        let mut bound = loop {
            let generation = text_font_generation(Some(media));
            self.ensure_compiled_for_frame(composition, frame, Some(media), PlanUse::Render)?;
            let prepared = self.active_prepared()?;
            let layout = &prepared.compiled.cache;
            let fonts = text_font_generation(Some(media));
            let epoch = layout.epoch(&prepared.renderer, fonts);
            let bound = FrameBindContext::with_media_at(composition, frame, media).bind_with_cache(
                &prepared.compiled,
                Some(BindCache::new(layout, &prepared.cache, epoch)),
            );
            // A failure under fonts that changed may come from what the plan sized for the
            // old ones, so it is bound again too.
            if text_font_generation(Some(media)) == generation {
                break bound?;
            }
            if rebinds == max_rebinds {
                return Err(RenderError::Gpu {
                    details: format!(
                        "fonts kept loading while frame {frame} was bound; its text bounds could not settle"
                    ),
                }
                .into());
            }
            rebinds += 1;
        };
        bound.renderer_binding = self.active_key.clone().map(|key| (self.bind_sequence, key));
        Ok(bound)
    }

    pub fn submit_bound_frame(
        &mut self,
        bound: &BoundFrame,
    ) -> crate::Result<(RasterHandle, lumen_gpu::wgpu::SubmissionIndex)> {
        self.upload_bound_frame(bound)?;
        self.submit_render()
    }

    pub fn upload_bound_frame(&mut self, bound: &BoundFrame) -> crate::Result<()> {
        tracing::trace!(
            target: "lumen_render",
            buffer_uploads = bound.buffer_upload_count(),
            texture_uploads = bound.texture_upload_count(),
            media_textures = bound.media_textures().len(),
            "upload bound frame"
        );
        if let Some(graph) = &mut self.frame_graph {
            if bound.recorded != Some(self.bind_sequence) {
                return Err(RenderError::Gpu {
                    details: "stale recorded frame; record it again before uploading".to_string(),
                }
                .into());
            }
            graph.upload(bound, self.text_atlas_texture.as_deref())?;
            self.media_texture_cache.evict();
            return Ok(());
        }
        if bound
            .renderer_binding
            .as_ref()
            .is_none_or(|(sequence, key)| {
                *sequence != self.bind_sequence || Some(key) != self.active_key.as_ref()
            })
        {
            return Err(RenderError::Gpu {
                details: "stale bound text frame; rebind before uploading".to_string(),
            }
            .into());
        }
        let prepared = self.active_prepared_mut()?;
        prepared.cache.upload(&prepared.compiled.cache, None);
        self.upload_media_textures(bound)?;
        let update = bound.frame_update();
        let prepared = self.active_prepared_mut()?;
        prepared
            .renderer
            .apply_frame_update(&prepared.compiled.plan, &update)
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        bound.mark_uploaded()?;
        prepared
            .cache
            .upload(&prepared.compiled.cache, bound.cache.as_ref());
        Ok(())
    }

    fn upload_media_textures(&mut self, bound: &BoundFrame) -> crate::Result<()> {
        let result = self.write_media_textures(bound);
        self.media_texture_cache.evict();
        result
    }

    fn write_media_textures(&mut self, bound: &BoundFrame) -> crate::Result<()> {
        let key = self.active_key.clone().ok_or_else(|| RenderError::Gpu {
            details: "composition has not been compiled".to_string(),
        })?;
        let prepared = self
            .compiled_plans
            .get_mut(&key)
            .ok_or_else(|| RenderError::Gpu {
                details: "composition has not been compiled".to_string(),
            })?;
        let media_texture_cache = &mut self.media_texture_cache;
        #[cfg(feature = "ffmpeg")]
        let gpu_media_importer = &mut self.gpu_media_importer;
        for upload in bound.media_textures() {
            tracing::trace!(
                target: "lumen_render",
                texture = ?upload.texture,
                key = ?upload.key,
                width = upload.size.width,
                height = upload.size.height,
                "resolve media texture upload"
            );
            #[cfg(feature = "ffmpeg")]
            let cpu_retry;
            let frame = match &upload.frame {
                MediaFrame::CpuRgba(frame) => frame,
                #[cfg(feature = "ffmpeg")]
                MediaFrame::GpuVideo(frame) => {
                    match gpu_media_importer.import(
                        &prepared.renderer,
                        &mut prepared.media_imports,
                        upload,
                        frame,
                    ) {
                        Ok(texture) => {
                            let bound = prepared.renderer.texture_arc(upload.texture);
                            if bound.is_none_or(|bound| !Arc::ptr_eq(&bound, &texture)) {
                                let size = texture.size();
                                let desc = lumen_gpu::TextureDesc::sampled(
                                    lumen_gpu::Size::new(size.width, size.height),
                                    match frame.frame.backend() {
                                        #[cfg(all(target_os = "macos", feature = "metal"))]
                                        lumen_ffmpeg::GpuBackend::Metal => {
                                            lumen_gpu::wgpu::TextureFormat::Bgra8Unorm
                                        }
                                        _ => lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                                    },
                                );
                                prepared
                                    .renderer
                                    .replace_texture_arc(upload.texture, texture, desc)
                                    .map_err(|error| RenderError::Gpu {
                                        details: error.to_string(),
                                    })?;
                            }
                            prepared.current_media_textures.remove(&upload.texture);
                            continue;
                        }
                        Err(reason) => {
                            cpu_retry =
                                frame
                                    .decode_on_cpu(&reason)
                                    .map_err(|error| RenderError::Gpu {
                                        details: format!(
                                            "failed to import GPU media frame: {reason}; \
                                         decoding it on the CPU also failed: {error}"
                                        ),
                                    })?;
                            &cpu_retry
                        }
                    }
                }
            };

            let Some(key) = &upload.key else {
                tracing::trace!(
                    target: "lumen_render",
                    texture = ?upload.texture,
                    "upload uncached video frame"
                );
                let cached_was_bound = prepared
                    .current_media_textures
                    .remove(&upload.texture)
                    .is_some();
                // A source that moved to the CPU may leave an imported texture of another size or
                // format.
                let not_writable = prepared
                    .renderer
                    .texture(upload.texture)
                    .is_some_and(|bound| {
                        bound.size() != upload.size.as_extent()
                            || bound.format() != lumen_gpu::wgpu::TextureFormat::Rgba8Unorm
                    });
                if cached_was_bound || not_writable {
                    // The bound texture is a cached image's, which must not be written.
                    prepared
                        .renderer
                        .replace_texture_arc(
                            upload.texture,
                            create_media_texture(&prepared.renderer.device, upload.size),
                            lumen_gpu::TextureDesc::sampled(
                                upload.size,
                                lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                            ),
                        )
                        .map_err(|error| RenderError::Gpu {
                            details: error.to_string(),
                        })?;
                }
                let rgba = fit_frame_to_rgba8(frame, upload.size.width, upload.size.height);
                prepared.renderer.queue.write_texture(
                    lumen_gpu::wgpu::TexelCopyTextureInfo {
                        texture: prepared.renderer.texture(upload.texture).ok_or_else(|| {
                            RenderError::Gpu {
                                details: format!("unknown media texture {:?}", upload.texture),
                            }
                        })?,
                        mip_level: 0,
                        origin: lumen_gpu::wgpu::Origin3d::ZERO,
                        aspect: lumen_gpu::wgpu::TextureAspect::All,
                    },
                    &rgba,
                    lumen_gpu::wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(upload.size.width * 4),
                        rows_per_image: Some(upload.size.height),
                    },
                    upload.size.as_extent(),
                );
                continue;
            };

            if prepared
                .current_media_textures
                .get(&upload.texture)
                .is_some_and(|current| current == key)
            {
                tracing::trace!(
                    target: "lumen_render",
                    texture = ?upload.texture,
                    source = %key.source,
                    "reuse current media texture binding"
                );
                continue;
            }

            let texture = if let Some(texture) = media_texture_cache.get(key) {
                tracing::trace!(
                    target: "lumen_render",
                    source = %key.source,
                    "reuse cached media texture"
                );
                texture
            } else {
                tracing::debug!(
                    target: "lumen_render",
                    source = %key.source,
                    width = upload.size.width,
                    height = upload.size.height,
                    "cache media texture"
                );
                let texture = create_media_texture(&prepared.renderer.device, upload.size);
                let rgba = fit_frame_to_rgba8(frame, upload.size.width, upload.size.height);
                prepared.renderer.queue.write_texture(
                    texture.as_image_copy(),
                    &rgba,
                    lumen_gpu::wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(upload.size.width * 4),
                        rows_per_image: Some(upload.size.height),
                    },
                    upload.size.as_extent(),
                );
                media_texture_cache.insert(key.clone(), Arc::clone(&texture));
                texture
            };

            let desc = lumen_gpu::TextureDesc::sampled(
                upload.size,
                lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            );
            let displaced = prepared
                .renderer
                .replace_texture_arc(upload.texture, texture, desc)
                .map_err(|error| RenderError::Gpu {
                    details: error.to_string(),
                })?;
            // The plan's own placeholder is not shared, so free it now rather than at GC on web.
            if Arc::strong_count(&displaced) == 1 {
                displaced.destroy();
            }
            prepared
                .current_media_textures
                .insert(upload.texture, key.clone());
        }
        Ok(())
    }

    pub fn submit_render(
        &mut self,
    ) -> crate::Result<(RasterHandle, lumen_gpu::wgpu::SubmissionIndex)> {
        if let Some(graph) = &mut self.frame_graph {
            let output = graph.output().ok_or_else(|| RenderError::Gpu {
                details: "no recorded frame".to_string(),
            })?;
            let submission = graph.recorder.submit().map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
            if let Some((texture, desc)) = graph.changed_output() {
                self.renderer.set_recorded_output(texture, desc);
            }
            graph.finish_frame();
            return Ok((
                RasterHandle {
                    texture: lumen_gpu::TextureId(0),
                    domain: output.domain,
                    bounds: None,
                },
                submission,
            ));
        }
        let prepared = self.active_prepared_mut()?;
        tracing::trace!(
            target: "lumen_render",
            passes = prepared.compiled.plan.passes().len(),
            "submit render"
        );
        let submission = prepared
            .renderer
            .submit_plan_skipping(&prepared.compiled.plan, prepared.cache.skip())
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        let output = prepared.compiled.output;
        self.count_submitted();
        Ok((output, submission))
    }

    /// Keeps what the active plan's submitted frame rendered and counts its passes.
    fn count_submitted(&mut self) {
        let Some(prepared) = self
            .active_key
            .as_ref()
            .and_then(|key| self.compiled_plans.get_mut(key))
        else {
            return;
        };
        let passes = prepared.compiled.plan.passes().len() as u64;
        let skipped = skipped_count(prepared.cache.skip()) as u64;
        prepared.cache.submitted(&prepared.compiled.cache);
        self.submitted_pass_count += passes;
        self.cached_pass_count += skipped;
    }

    pub fn gpu_renderer(&self) -> &lumen_gpu::Renderer {
        self.active_key
            .as_ref()
            .and_then(|key| self.compiled_plans.get(key))
            .map(|prepared| &prepared.renderer)
            .unwrap_or(&self.renderer)
    }

    pub fn gpu_renderer_mut(&mut self) -> &mut lumen_gpu::Renderer {
        let Some(key) = self.active_key.clone() else {
            return &mut self.renderer;
        };
        self.compiled_plans
            .get_mut(&key)
            .map(|prepared| &mut prepared.renderer)
            .unwrap_or(&mut self.renderer)
    }

    pub fn compiled(&self) -> Option<&CompiledComposition> {
        self.active_key
            .as_ref()
            .and_then(|key| self.compiled_plans.get(key))
            .map(|prepared| &prepared.compiled)
    }

    fn active_prepared(&self) -> crate::Result<&PreparedComposition> {
        self.active_key
            .as_ref()
            .and_then(|key| self.compiled_plans.get(key))
            .ok_or_else(|| {
                RenderError::Gpu {
                    details: "composition has not been compiled".to_string(),
                }
                .into()
            })
    }

    fn active_prepared_mut(&mut self) -> crate::Result<&mut PreparedComposition> {
        let key = self.active_key.clone().ok_or_else(|| RenderError::Gpu {
            details: "composition has not been compiled".to_string(),
        })?;
        self.compiled_plans.get_mut(&key).ok_or_else(|| {
            RenderError::Gpu {
                details: "composition has not been compiled".to_string(),
            }
            .into()
        })
    }

    /// Makes the plan for `frame` resident and, when rendering, current. Returns false when
    /// lookahead skipped a plan that would not fit the budget.
    fn ensure_compiled_for_frame<M: MediaStore>(
        &mut self,
        composition: &Composition,
        frame: f64,
        media: Option<&M>,
        plan_use: PlanUse,
    ) -> crate::Result<bool> {
        let fingerprint = composition.fingerprint();
        let key = self.compiled_plan_key(
            composition,
            frame,
            media.map(|store| store as &dyn MediaStore),
        )?;
        self.plan_use_clock += 1;
        if let Some(prepared) = self.compiled_plans.get_mut(&key) {
            prepared.last_used = self.plan_use_clock;
            if plan_use == PlanUse::Lookahead {
                return Ok(true);
            }
            if self.active_key.as_ref() == Some(&key) {
                tracing::trace!(target: "lumen_render", frame, "reuse active compiled plan");
            } else {
                tracing::debug!(
                    target: "lumen_render",
                    frame,
                    key_entries = key.entries.len(),
                    "switch to cached compiled plan"
                );
                self.active_key = Some(key);
            }
            self.prune_stale_plans(fingerprint);
            return Ok(true);
        }
        if plan_use == PlanUse::Lookahead {
            if self.rejected_fingerprint.replace(fingerprint) != Some(fingerprint) {
                self.rejected_plan_bytes.clear();
            }
            if let Some(&bytes) = self.rejected_plan_bytes.get(&key)
                && self.plan_cache_bytes + bytes > self.plan_cache_budget
            {
                return Ok(false);
            }
        }

        tracing::debug!(
            target: "lumen_render",
            frame,
            key_entries = key.entries.len(),
            "compile render plan for frame"
        );
        self.plan_compile_count += 1;
        let compiled = match CompileContext::with_options(
            composition,
            frame,
            media.map(|store| store as &dyn MediaStore),
            self.output_format,
            true,
        )
        .render_static(!self.reuse_static_output)
        .full_frame_sources(self.full_frame_sources)
        .compile_with_atlas(Arc::clone(&self.text_atlas))
        {
            Ok(compiled) => compiled,
            Err(error) => {
                if plan_use == PlanUse::Render {
                    self.prune_stale_plans(fingerprint);
                }
                return Err(error);
            }
        };
        self.rejected_plan_bytes.remove(&key);
        if plan_use == PlanUse::Lookahead {
            let estimated_bytes = estimated_plan_bytes(&compiled);
            if self.plan_cache_bytes + estimated_bytes > self.plan_cache_budget {
                tracing::debug!(
                    target: "lumen_render",
                    frame,
                    bytes = estimated_bytes,
                    cached_bytes = self.plan_cache_bytes,
                    budget = self.plan_cache_budget,
                    "skip lookahead plan over the plan cache budget"
                );
                if self.rejected_plan_bytes.len() >= MAX_REJECTED_PLANS {
                    self.rejected_plan_bytes.clear();
                }
                self.rejected_plan_bytes.insert(key, estimated_bytes);
                return Ok(false);
            }
            let active = self.active_key.clone();
            let prepared = self.prepare_compiled(key, compiled);
            self.active_key = active;
            return prepared.map(|()| true);
        }
        let previous = self.active_key.clone();
        let prepared = self.prepare_compiled(key, compiled);
        self.prune_stale_plans(fingerprint);
        prepared?;
        let current = self.active_key.clone();
        self.evict_over_budget(&[previous.as_ref(), current.as_ref()]);
        Ok(true)
    }

    fn compiled_plan_key(
        &self,
        composition: &Composition,
        frame: f64,
        media: Option<&dyn MediaStore>,
    ) -> crate::Result<CompiledPlanKey> {
        let root = Scope::root(composition);
        let output = root.output;
        let mut collector = PlanKeyCollector {
            media,
            time_remaps: TimeRemapPath::new(),
            visited: foldhash::HashSet::default(),
            entries: Vec::new(),
            has_text: false,
        };
        collector.collect(&root, output, frame.into())?;
        let mut entries = collector.entries;
        entries.sort();
        entries.dedup();
        Ok(CompiledPlanKey {
            composition: composition.fingerprint(),
            on_timeline: super::compiler::on_timeline(composition, frame),
            text_font_generation: collector
                .has_text
                .then(|| text_font_generation(media))
                .flatten(),
            entries,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlanUse {
    Render,
    Lookahead,
}

/// Mirrors compilation: inputs come from `scope::route`, and a time remap or clip starts a new
/// instance path for everything upstream of it.
struct PlanKeyCollector<'a> {
    media: Option<&'a dyn MediaStore>,
    time_remaps: TimeRemapPath,
    visited: foldhash::HashSet<(NodeId, TimeRemapPath)>,
    entries: Vec<PlanKeyEntry>,
    /// Whether any text was reached, whose bounds depend on the loaded fonts.
    has_text: bool,
}

impl<'a> PlanKeyCollector<'a> {
    fn collect(
        &mut self,
        scope: &Scope<'_>,
        node_id: NodeId,
        frame: FrameTime,
    ) -> crate::Result<()> {
        if !self.visited.insert((node_id, self.time_remaps.clone())) {
            return Ok(());
        }
        let node = scope
            .node(node_id)
            .ok_or(RenderError::MissingNode { node_id })?;
        self.collect_node(scope, node, frame)
    }

    fn collect_node(
        &mut self,
        scope: &Scope<'_>,
        node: &NodeKind,
        frame: FrameTime,
    ) -> crate::Result<()> {
        let routes = crate::scope::route(scope, node, frame, self.media)?;
        match node {
            NodeKind::Switch(switch) => {
                // Out-of-range selections all compile to transparent, so they share one key.
                let ctx = scope.expr_context(frame, self.media);
                let selection =
                    crate::node::compositing::switch::selected_layer_for_frame(switch, &ctx)?
                        .filter(|index| *index < switch.layers.len());
                self.push_entry(switch.id, PlanChoice::SwitchSelection(selection));
            }
            NodeKind::Clip(clip) => {
                self.push_entry(clip.id, PlanChoice::ClipShown(!routes.is_empty()));
            }
            NodeKind::Text(text) => {
                let layout = &text.params.layout;
                let spans = match &layout.content {
                    TextContentDelegate::Spans(spans) => &spans[..],
                    TextContentDelegate::Text(_) => &[],
                };
                let families = std::iter::once(&layout.font_family).chain(
                    spans
                        .iter()
                        .filter_map(|span| span.layout.font_family.as_ref()),
                );
                // An animated family is keyed by the family it names at this frame. One that fails
                // to evaluate is left for binding to report.
                let mut ctx = None;
                for family in families {
                    match family {
                        Deferred::Value(family) => self.collect_font_family(text.id, family),
                        Deferred::Expr(_) => {
                            let ctx =
                                ctx.get_or_insert_with(|| scope.expr_context(frame, self.media));
                            if let Ok(family) = family.eval(text.id, "font_family", ctx) {
                                self.collect_font_family(text.id, &family);
                            }
                        }
                    }
                }
                self.has_text = true;
            }
            NodeKind::MediaIn(media_in) => {
                let ctx = scope.expr_context(frame, self.media);
                // A failing source is left for compiling to report, like a font family above.
                if let Ok(Some(size)) = media_in.native_size(&ctx) {
                    self.push_entry(media_in.id, PlanChoice::MediaSize(size.width, size.height));
                }
            }
            NodeKind::WgslShader(shader) => {
                let ctx = scope.expr_context(frame, self.media);
                let params = shader.params.eval(&DelegateEvalContext {
                    node_id: shader.id,
                    expr: &ctx,
                })?;
                self.push_entry(
                    shader.id,
                    PlanChoice::ShaderSource(source_hash(&params.shader)),
                );
            }
            _ => {}
        }
        for Routed {
            node,
            frame,
            scope,
            instance,
        } in routes
        {
            self.time_remaps.extend(instance);
            let result = self.collect(&scope, node, frame);
            if instance.is_some() {
                self.time_remaps.pop();
            }
            result?;
        }
        Ok(())
    }

    /// Keys the plan by the resolver the media store gives the literal `family` of text `node_id`
    /// and loads it.
    fn collect_font_family(&mut self, node_id: NodeId, family: &str) {
        if let Some(resolver) = self.media.and_then(|m| m.font_resolver_id(family)) {
            self.push_entry(node_id, PlanChoice::TextFont(resolver));
        }
        self.load_font_family(family);
    }

    /// Loads the literal `family` the media store resolves, as the compile would, so the key's
    /// font generation is taken once every font the frame's text names is loaded. Failures are
    /// left to the compile and bind, which load it again.
    fn load_font_family(&self, family: &str) {
        if let Ok(mut text_system) = crate::node::source::text::text_system(self.media) {
            let _ =
                crate::node::source::text::load_font_family(&mut text_system, self.media, family);
        }
    }

    fn push_entry(&mut self, node_id: NodeId, choice: PlanChoice) {
        self.entries.push(PlanKeyEntry {
            time_remaps: self.time_remaps.clone(),
            node_id,
            choice,
        });
    }
}

/// Bytes of the textures and buffers a plan allocates for itself; the text atlas is shared.
fn estimated_plan_bytes(compiled: &CompiledComposition) -> u64 {
    let textures: u64 = compiled
        .plan
        .textures()
        .iter()
        .filter(|resource| Some(resource.id) != compiled.text_atlas_texture)
        .map(|resource| {
            resource
                .desc
                .format
                .theoretical_memory_footprint(resource.desc.domain.storage_size.as_extent())
        })
        .sum();
    let buffers: u64 = compiled
        .plan
        .buffers()
        .iter()
        .map(|resource| resource.desc.size)
        .sum();
    textures + buffers
}

/// Times a frame is bound again after fonts load during binding, before binding fails rather
/// than draw text past its bounds. A node loads all the fonts it names at once, and a bind can
/// stop at the first text they outgrow, so this allows each node a bind of its own, with a margin
/// for fonts other renderers load meanwhile. A node evaluated many times can name more fonts than
/// this allows, and its frame then fails, but the fonts stay loaded for the next render.
fn max_font_rebinds(composition: &Composition) -> usize {
    let templates: usize = composition
        .compositions()
        .values()
        .map(|template| template.scene.graph.nodes.len())
        .sum();
    composition.graph().nodes.len() + templates + 3
}

fn text_font_generation(media: Option<&dyn MediaStore>) -> Option<u64> {
    crate::node::source::text::text_system(media)
        .ok()
        .map(|text_system| text_system.font_generation())
}

fn source_hash(source: &str) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    std::hash::Hash::hash(source, &mut hasher);
    std::hash::Hasher::finish(&hasher)
}

pub(super) fn fit_frame_to_rgba8(frame: &CpuMediaFrame, width: u32, height: u32) -> Vec<u8> {
    if frame.width == width && frame.height == height && frame.row_bytes == width as usize * 4 {
        return frame.rgba.as_ref().clone();
    }

    let mut out = vec![0; width as usize * height as usize * 4];
    for y in 0..height {
        let src_y = ((u64::from(y) * u64::from(frame.height)) / u64::from(height)) as usize;
        for x in 0..width {
            let src_x = ((u64::from(x) * u64::from(frame.width)) / u64::from(width)) as usize;
            let src = src_y
                .saturating_mul(frame.row_bytes)
                .saturating_add(src_x.saturating_mul(4));
            let dst = (y as usize)
                .saturating_mul(width as usize * 4)
                .saturating_add(x as usize * 4);
            out[dst..dst + 4].copy_from_slice(&frame.rgba[src..src + 4]);
        }
    }
    out
}
