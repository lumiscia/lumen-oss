use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use crate::{
    composition::Composition,
    error::{GraphValidationError, RenderError},
    gpu::{
        BoundFrame, CompileContext, CompiledComposition, FrameBindContext, MediaTextureKey,
        RasterHandle, TimeRemapPath,
        cache::{
            BindCache, CacheState, LayoutMemo, MemoSnapshots, composition_fingerprint,
            skipped_count,
        },
    },
    media::{CpuMediaFrame, MediaFrame, MediaStore},
    node::{Deferred, NodeId, NodeKind, NodeParamEvalContext, NodeParams, PortRef},
    scope::{FrameTime, Routed, Scope},
};

#[cfg(feature = "ffmpeg")]
use super::media::GpuMediaFrameImporter;

/// Everything a frame can change that alters the compiled plan's structure, plus the canvas size
/// its textures were allocated for.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CompiledPlanKey {
    size: (u32, u32),
    /// Whether the frame is a whole frame of the timeline, so animated sources may be sized to
    /// their bounds over it.
    on_timeline: bool,
    /// Frame count and fps bits of the timeline that bounds over it were taken from.
    timeline: (u32, u32),
    /// Hash of the templates' canvases and timelines, which their textures were sized for.
    templates: u64,
    entries: Vec<PlanKeyEntry>,
}

impl CompiledPlanKey {
    /// Whether the canvas size and timeline are unchanged and the nodes this key chose between
    /// still exist with the same kind and, for literal shader sources, the same source.
    fn matches(&self, composition: &Composition) -> bool {
        let settings = &composition.render_settings;
        if self.size != (settings.width, settings.height)
            || self.timeline != timeline_key(composition)
            || self.templates != templates_key(composition)
        {
            return false;
        }
        self.entries.iter().all(|entry| {
            let Some(graph) = Scope::graph_at(composition, &entry.time_remaps) else {
                return false;
            };
            match (&entry.choice, graph.nodes.get(&entry.node_id)) {
                (PlanChoice::SwitchSelection(_), Some(NodeKind::Switch(_))) => true,
                (PlanChoice::Clip { definition, .. }, Some(NodeKind::Clip(clip))) => {
                    clip_definition_hash(composition, clip) == *definition
                }
                (PlanChoice::TextFont(_), Some(NodeKind::Text(_))) => true,
                (PlanChoice::TextFontGeneration(_), Some(NodeKind::Text(_))) => true,
                (PlanChoice::ShaderSource(hash), Some(NodeKind::WgslShader(shader))) => {
                    match &shader.params.shader {
                        Deferred::Value(source) => source_hash(source) == *hash,
                        Deferred::Expr(_) => true,
                    }
                }
                _ => false,
            }
        })
    }
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
    /// Whether a clip shows, which decides whether anything upstream of it is compiled, and
    /// the hash of its definition, which the bounds of its content were taken over.
    Clip {
        shown: bool,
        definition: u64,
    },
    /// The resolver the media store gave a text node's font family, which its bounds were
    /// measured with.
    TextFont(String),
    /// The fonts the shared text system held when a text node's bounds were measured; loading
    /// any face may change how that text is shaped.
    TextFontGeneration(u64),
    ShaderSource(u64),
}

/// Estimated GPU memory compiled plans may hold before least recently used ones are dropped.
pub const DEFAULT_PLAN_CACHE_BUDGET_BYTES: u64 = 1 << 30;

/// Rejected plan estimates remembered before they are all forgotten.
const MAX_REJECTED_PLANS: usize = 256;

struct PreparedComposition {
    renderer: lumen_gpu::Renderer,
    compiled: CompiledComposition,
    /// Output kept from frames submitted to this plan.
    cache: CacheState,
    current_media_textures: HashMap<lumen_gpu::TextureId, MediaTextureKey>,
    estimated_bytes: u64,
    last_used: u64,
}

pub struct GpuCompositionRenderer {
    renderer: lumen_gpu::Renderer,
    compiled_plans: HashMap<CompiledPlanKey, PreparedComposition>,
    active_key: Option<CompiledPlanKey>,
    plan_cache_bytes: u64,
    plan_cache_budget: u64,
    plan_use_clock: u64,
    /// Signature of the plan-key nodes and canvas size the cache was last pruned against.
    pruned_signature: Option<u64>,
    /// Estimated sizes of lookahead plans that did not fit the budget, so later windows can skip
    /// them without compiling. Valid only for the composition inputs with this signature.
    rejected_plan_bytes: HashMap<CompiledPlanKey, u64>,
    rejected_signature: Option<u64>,
    plan_compile_count: u64,
    submitted_pass_count: u64,
    cached_pass_count: u64,
    /// Memo snapshots, which outlive the plan that took them while another plan holds a copy.
    memo_snapshots: MemoSnapshots,
    /// Plans the latest bound frame copies memo snapshots from, which stay cached until it is
    /// submitted or another frame is bound.
    pinned_plans: Vec<u64>,
    /// Prepared plans so far, which identifies each among the cached plans.
    prepared_plan_count: u64,
    bind_sequence: u64,
    text_atlas: Arc<Mutex<lumen_text::GlyphAtlas>>,
    text_atlas_texture: Option<Arc<lumen_gpu::wgpu::Texture>>,
    output_format: lumen_gpu::wgpu::TextureFormat,
    media_texture_cache: HashMap<MediaTextureKey, Arc<lumen_gpu::wgpu::Texture>>,
    #[cfg(feature = "ffmpeg")]
    gpu_media_importer: GpuMediaFrameImporter,
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
        Self {
            renderer,
            compiled_plans: HashMap::new(),
            active_key: None,
            plan_cache_bytes: 0,
            plan_cache_budget: DEFAULT_PLAN_CACHE_BUDGET_BYTES,
            plan_use_clock: 0,
            pruned_signature: None,
            rejected_plan_bytes: HashMap::new(),
            rejected_signature: None,
            plan_compile_count: 0,
            submitted_pass_count: 0,
            cached_pass_count: 0,
            memo_snapshots: MemoSnapshots::default(),
            pinned_plans: Vec::new(),
            prepared_plan_count: 0,
            bind_sequence: 0,
            text_atlas: Arc::new(Mutex::new(lumen_text::GlyphAtlas::new(
                super::types::TEXT_ATLAS_SIZE,
            ))),
            text_atlas_texture: None,
            output_format: lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            media_texture_cache: HashMap::new(),
            #[cfg(feature = "ffmpeg")]
            gpu_media_importer: GpuMediaFrameImporter::default(),
        }
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
        let key = self.compiled_plan_key(composition, 0.0, None)?;
        self.plan_compile_count += 1;
        let compiled =
            CompileContext::with_options(composition, 0.0, None, output_format, true).compile()?;
        self.prepare_compiled(key, compiled)
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
        self.plan_cache_bytes
    }

    pub fn cached_plan_count(&self) -> usize {
        self.compiled_plans.len()
    }

    /// Number of times a plan has been built on the CPU, whether or not it was kept.
    pub fn plan_compile_count(&self) -> u64 {
        self.plan_compile_count
    }

    /// Number of passes in the plans of submitted frames, whether they ran or not.
    pub fn submitted_pass_count(&self) -> u64 {
        self.submitted_pass_count
    }

    /// Number of passes submitted frames skipped because the output they would write was kept
    /// from an earlier frame: that of nodes without frame-dependent params and of what a holding
    /// `memo` reads.
    pub fn cached_pass_count(&self) -> u64 {
        self.cached_pass_count
    }

    /// Number of rejected lookahead plans whose estimated size is remembered.
    #[doc(hidden)]
    pub fn rejected_plan_count(&self) -> usize {
        self.rejected_plan_bytes.len()
    }

    pub fn compile_with_media<M: MediaStore>(
        &mut self,
        composition: &Composition,
        media: &M,
        output_format: lumen_gpu::wgpu::TextureFormat,
    ) -> crate::Result<()> {
        self.reset_compiled(output_format);
        self.ensure_compiled_for_frame(composition, 0.0, Some(media), PlanUse::Render)
            .map(drop)
    }

    /// Drops every compiled plan and cached media texture, destroying the GPU textures and
    /// buffers this renderer exclusively owns instead of waiting for them to be garbage collected.
    /// Retains the shared text atlas (64 MiB once allocated) and cached glyphs across recompilation.
    pub fn release_resources(&mut self) {
        self.bind_sequence += 1;
        for texture in self.media_texture_cache.values() {
            texture.destroy();
        }
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
        self.media_texture_cache.clear();
        self.compiled_plans.clear();
        self.memo_snapshots.clear();
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
            media_textures = self.media_texture_cache.len(),
            "reset compiled composition"
        );
        self.bind_sequence += 1;
        self.output_format = output_format;
        self.compiled_plans.clear();
        self.memo_snapshots.clear();
        self.plan_cache_bytes = 0;
        self.pruned_signature = None;
        self.rejected_plan_bytes.clear();
        self.rejected_signature = None;
        self.active_key = None;
    }

    fn prepare_compiled(
        &mut self,
        key: CompiledPlanKey,
        mut compiled: CompiledComposition,
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
        compiled.text_atlas = Arc::clone(&self.text_atlas);
        if let Some(id) = compiled.text_atlas_texture {
            self.ensure_text_atlas_texture()?;
            renderer
                .replace_texture_arc(
                    id,
                    Arc::clone(self.text_atlas_texture.as_ref().expect("atlas texture")),
                    super::types::text_atlas_desc(),
                )
                .map_err(|error| RenderError::Gpu {
                    details: error.to_string(),
                })?
                .destroy();
        }
        let estimated_bytes = estimated_plan_bytes(&compiled);
        self.plan_cache_bytes += estimated_bytes;
        self.plan_use_clock += 1;
        self.prepared_plan_count += 1;
        if let Some(replaced) = self.compiled_plans.insert(
            key.clone(),
            PreparedComposition {
                renderer,
                compiled,
                cache: CacheState::new(self.prepared_plan_count),
                current_media_textures: HashMap::new(),
                estimated_bytes,
                last_used: self.plan_use_clock,
            },
        ) {
            self.plan_cache_bytes -= replaced.estimated_bytes;
        }
        self.active_key = Some(key);
        Ok(())
    }

    /// Drops plans built from nodes or shader sources the composition no longer has, scanning the
    /// cache only when those nodes changed since the last prune. Their GPU resources are freed
    /// once nothing else holds them.
    fn prune_stale_plans(&mut self, composition: &Composition) {
        let signature = plan_node_signature(composition);
        if self.pruned_signature.replace(signature) == Some(signature) {
            return;
        }
        let cached_plans = self.compiled_plans.len();
        let plan_cache_bytes = &mut self.plan_cache_bytes;
        self.compiled_plans.retain(|key, prepared| {
            let keep = key.matches(composition);
            if !keep {
                *plan_cache_bytes -= prepared.estimated_bytes;
            }
            keep
        });
        if self.compiled_plans.len() == cached_plans {
            return;
        }
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
                .filter(|(key, prepared)| {
                    !pinned.contains(&Some(*key))
                        && !self.pinned_plans.contains(&prepared.cache.plan())
                })
                .min_by_key(|(_, prepared)| prepared.last_used)
                .map(|(key, _)| key.clone())
            else {
                return;
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
    }

    /// Compiles the plan for `frame` ahead of rendering it, unless it would not fit the plan cache
    /// budget. Never drops a cached plan, including stale ones, or changes the current one.
    pub fn precompile_frame<M: MediaStore>(
        &mut self,
        composition: &Composition,
        frame: u32,
        media: &M,
    ) -> crate::Result<()> {
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
                duration_frames: composition.timeline.duration_frames,
            }
            .into());
        }
        self.bind_sequence += 1;
        // A font loaded while the plan compiles or the frame binds, here or by another renderer,
        // can widen text past the bounds the plan measured. The plan is then keyed by the new
        // font generation, so the frame is bound again to it; a font loads once, so this settles.
        let mut rebinds = 0;
        let mut bound = loop {
            let generation = text_font_generation();
            self.ensure_compiled_for_frame(composition, frame, Some(media), PlanUse::Render)?;
            let fingerprint = if self.active_prepared()?.compiled.cache.has_memos()
                || !self.memo_snapshots.is_empty()
            {
                composition_fingerprint(composition)
            } else {
                0
            };
            self.memo_snapshots.retain_composition(fingerprint);
            let prepared = self.active_prepared()?;
            let layout = &prepared.compiled.cache;
            let epoch = layout.epoch(&prepared.renderer);
            let held =
                self.memo_snapshots
                    .held(layout, prepared.cache.plan(), epoch, |plan, at, memo| {
                        self.memo_source(prepared, plan, memo)
                            .is_some_and(|(epoch, ..)| epoch == at)
                    });
            let bound = FrameBindContext::with_media_at(composition, frame, media)
                .bind_with_cache(
                    &prepared.compiled,
                    Some(BindCache::new(
                        layout,
                        &prepared.cache,
                        epoch,
                        fingerprint,
                        held,
                    )),
                )?;
            if text_font_generation() == generation {
                break bound;
            }
            if rebinds == MAX_FONT_REBINDS {
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
        self.pinned_plans = bound
            .cache
            .iter()
            .flat_map(|cache| cache.adopted().iter().map(|(_, plan)| *plan))
            .collect();
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
        if bound.text_atlas.is_some() {
            self.ensure_text_atlas_texture()?;
        }
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
        self.copy_memo_snapshots(bound)
    }

    /// The epoch of plan `plan` and the texture and bounds buffer holding its snapshot of `to`'s
    /// memo `memo`, if `to` can copy them.
    fn memo_source<'a>(
        &'a self,
        to: &PreparedComposition,
        plan: u64,
        memo: &LayoutMemo,
    ) -> Option<(
        u64,
        &'a lumen_gpu::wgpu::Texture,
        &'a lumen_gpu::wgpu::Buffer,
    )> {
        let from = self
            .compiled_plans
            .values()
            .find(|prepared| prepared.cache.plan() == plan)?;
        let source = from.compiled.cache.memo_by_id(&memo.id)?;
        let desc = from.renderer.texture_desc(source.texture)?;
        if Some(desc) != to.renderer.texture_desc(memo.texture) {
            return None;
        }
        Some((
            from.compiled.cache.epoch(&from.renderer),
            from.renderer.texture(source.texture)?,
            from.renderer.buffer(source.bounds)?,
        ))
    }

    /// Copies the memo snapshots `bound` shows from the plans holding them into the active plan.
    fn copy_memo_snapshots(&self, bound: &BoundFrame) -> crate::Result<()> {
        let Some(cache) = bound
            .cache
            .as_ref()
            .filter(|cache| !cache.adopted().is_empty())
        else {
            return Ok(());
        };
        let to = self.active_prepared()?;
        let mut encoder =
            to.renderer
                .device
                .create_command_encoder(&lumen_gpu::wgpu::CommandEncoderDescriptor {
                    label: Some("lumen memo snapshot copy"),
                });
        for (instance, plan) in cache.adopted() {
            let memo = to.compiled.cache.memo(*instance).expect("keyed memo");
            let missing = || RenderError::Gpu {
                details: format!(
                    "memo {} snapshot is no longer held by the plan it was bound to copy from",
                    memo.id.0
                ),
            };
            let (_, texture, bounds) = self.memo_source(to, *plan, memo).ok_or_else(missing)?;
            let destination = to.renderer.texture(memo.texture).ok_or_else(missing)?;
            encoder.copy_texture_to_texture(
                texture.as_image_copy(),
                destination.as_image_copy(),
                texture.size(),
            );
            let destination = to.renderer.buffer(memo.bounds).ok_or_else(missing)?;
            encoder.copy_buffer_to_buffer(bounds, 0, destination, 0, bounds.size());
        }
        to.renderer.queue.submit([encoder.finish()]);
        Ok(())
    }

    fn ensure_text_atlas_texture(&mut self) -> crate::Result<()> {
        if self.text_atlas_texture.is_some() {
            return Ok(());
        }
        let desc = super::types::text_atlas_desc();
        let texture = Arc::new(super::types::create_text_atlas_texture(
            &self.renderer.device,
        ));
        for prepared in self.compiled_plans.values_mut() {
            if let Some(id) = prepared.compiled.text_atlas_texture {
                prepared
                    .renderer
                    .replace_texture_arc(id, Arc::clone(&texture), desc)
                    .map_err(|error| RenderError::Gpu {
                        details: error.to_string(),
                    })?;
            }
        }
        self.text_atlas_texture = Some(texture);
        Ok(())
    }

    fn upload_media_textures(&mut self, bound: &BoundFrame) -> crate::Result<()> {
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
                source = %upload.key.source,
                frame = ?upload.key.frame,
                width = upload.size.width,
                height = upload.size.height,
                "resolve media texture upload"
            );
            #[cfg(feature = "ffmpeg")]
            if let MediaFrame::GpuVideo(frame) = &upload.frame {
                let texture = gpu_media_importer
                    .import(&prepared.renderer, upload, frame)
                    .map_err(|details| RenderError::Gpu {
                        details: format!("failed to import GPU media frame: {details}"),
                    })?;
                let desc = lumen_gpu::TextureDesc::sampled(
                    upload.size,
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
                prepared.current_media_textures.remove(&upload.texture);
                continue;
            }

            let MediaFrame::CpuRgba(frame) = &upload.frame else {
                return Err(RenderError::Gpu {
                    details: "media frame backend is not supported by this GPU renderer build"
                        .to_string(),
                }
                .into());
            };

            if upload.key.frame.is_some() {
                tracing::trace!(
                    target: "lumen_render",
                    texture = ?upload.texture,
                    source = %upload.key.source,
                    frame = ?upload.key.frame,
                    "upload uncached video frame"
                );
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
                prepared.current_media_textures.remove(&upload.texture);
                continue;
            }

            if prepared
                .current_media_textures
                .get(&upload.texture)
                .is_some_and(|current| current == &upload.key)
            {
                tracing::trace!(
                    target: "lumen_render",
                    texture = ?upload.texture,
                    source = %upload.key.source,
                    frame = ?upload.key.frame,
                    "reuse current media texture binding"
                );
                continue;
            }

            let texture = if let Some(texture) = media_texture_cache.get(&upload.key) {
                tracing::trace!(
                    target: "lumen_render",
                    source = %upload.key.source,
                    frame = ?upload.key.frame,
                    "reuse cached media texture"
                );
                Arc::clone(texture)
            } else {
                tracing::debug!(
                    target: "lumen_render",
                    source = %upload.key.source,
                    frame = ?upload.key.frame,
                    width = upload.size.width,
                    height = upload.size.height,
                    "cache media texture"
                );
                let texture = Arc::new(prepared.renderer.device.create_texture(
                    &lumen_gpu::wgpu::TextureDescriptor {
                        label: Some("lumen media cached frame"),
                        size: upload.size.as_extent(),
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: lumen_gpu::wgpu::TextureDimension::D2,
                        format: lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                        usage: lumen_gpu::wgpu::TextureUsages::TEXTURE_BINDING
                            | lumen_gpu::wgpu::TextureUsages::COPY_DST
                            | lumen_gpu::wgpu::TextureUsages::COPY_SRC,
                        view_formats: &[],
                    },
                ));
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
                media_texture_cache.insert(upload.key.clone(), Arc::clone(&texture));
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
                .insert(upload.texture, upload.key.clone());
        }
        Ok(())
    }

    pub fn submit_render(
        &mut self,
    ) -> crate::Result<(RasterHandle, lumen_gpu::wgpu::SubmissionIndex)> {
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
        prepared
            .cache
            .submitted(&prepared.compiled.cache, &mut self.memo_snapshots);
        self.submitted_pass_count += passes;
        self.cached_pass_count += skipped;
        if !std::mem::take(&mut self.pinned_plans).is_empty() {
            let active = self.active_key.clone();
            self.evict_over_budget(&[active.as_ref()]);
        }
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
        let key = self.compiled_plan_key(
            composition,
            frame,
            media.map(|store| store as &dyn MediaStore),
        )?;
        self.plan_use_clock += 1;
        if plan_use == PlanUse::Render {
            self.prune_stale_plans(composition);
        }
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
            return Ok(true);
        }
        if plan_use == PlanUse::Lookahead {
            let signature = plan_node_signature(composition);
            if self.rejected_signature.replace(signature) != Some(signature) {
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
        let compiled = CompileContext::with_options(
            composition,
            frame,
            media.map(|store| store as &dyn MediaStore),
            self.output_format,
            true,
        )
        .compile()?;
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
        self.prepare_compiled(key, compiled)?;
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
        let output = PortRef::new(root.media_output()?, "output".to_string());
        let mut collector = PlanKeyCollector {
            media,
            time_remaps: TimeRemapPath::new(),
            visited: HashSet::new(),
            stack: Vec::new(),
            entries: Vec::new(),
        };
        collector.collect(&root, &output, frame.into())?;
        let mut entries = collector.entries;
        entries.sort();
        entries.dedup();
        let settings = &composition.render_settings;
        Ok(CompiledPlanKey {
            size: (settings.width, settings.height),
            on_timeline: super::compiler::on_timeline(composition, frame),
            timeline: timeline_key(composition),
            templates: templates_key(composition),
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
    visited: HashSet<(NodeId, TimeRemapPath)>,
    /// Nodes on the current traversal path, with the graph they are in; time remaps and clips
    /// extend the instance path, so `visited` alone never sees a cycle through one repeat.
    stack: Vec<(NodeId, *const crate::graph::Graph)>,
    entries: Vec<PlanKeyEntry>,
}

impl<'a> PlanKeyCollector<'a> {
    fn collect(
        &mut self,
        scope: &Scope<'_>,
        port: &PortRef,
        frame: FrameTime,
    ) -> crate::Result<()> {
        if port.is_empty() {
            return Ok(());
        }
        let entry = (port.id, std::ptr::from_ref(scope.graph));
        if let Some(start) = self.stack.iter().position(|visiting| *visiting == entry) {
            return Err(GraphValidationError::Cycle {
                path: self.stack[start..].iter().map(|(id, _)| *id).collect(),
            }
            .into());
        }
        if !self.visited.insert((port.id, self.time_remaps.clone())) {
            return Ok(());
        }
        let Some(node) = scope.node(port.id) else {
            return Ok(());
        };
        self.stack.push(entry);
        let result = self.collect_node(scope, node, frame);
        self.stack.pop();
        result
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
                let ctx = scope.expr_context(frame, format!("{}.params", switch.id), self.media);
                let selection =
                    crate::node::compositing::switch::selected_layer_for_frame(switch, &ctx)?
                        .filter(|index| *index < switch.layers.len());
                self.push_entry(switch.id, PlanChoice::SwitchSelection(selection));
            }
            NodeKind::Clip(clip) => {
                let shown = routes.first().is_some_and(|routed| !routed.port.is_empty());
                let definition = clip_definition_hash(scope.root, clip);
                self.push_entry(clip.id, PlanChoice::Clip { shown, definition });
            }
            NodeKind::Text(text) => {
                if let Deferred::Value(family) = &text.params.font_family
                    && let Some(resolver) = self.media.and_then(|m| m.font_resolver_id(family))
                {
                    self.push_entry(text.id, PlanChoice::TextFont(resolver));
                }
                if let Some(generation) = self.text_font_generation(&text.params.font_family) {
                    self.push_entry(text.id, PlanChoice::TextFontGeneration(generation));
                }
            }
            NodeKind::WgslShader(shader) => {
                let ctx = scope.expr_context(frame, format!("{}.params", shader.id), self.media);
                let params = shader.params.eval(&NodeParamEvalContext {
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
            port,
            frame,
            scope,
            instance,
        } in routes
        {
            self.time_remaps.extend(instance);
            let result = self.collect(&scope, &port, frame);
            if instance.is_some() {
                self.time_remaps.pop();
            }
            result?;
        }
        Ok(())
    }

    /// The font generation after loading the literal `family` the media store resolves, as the
    /// compile would; `None` when the text system is unavailable.
    fn text_font_generation(&self, family: &Deferred<String>) -> Option<u64> {
        let mut text_system = crate::node::source::text::text_system().ok()?;
        if let Deferred::Value(family) = family {
            crate::node::source::text::load_font_family(&mut text_system, self.media, family)
                .ok()?;
        }
        Some(text_system.font_generation())
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
/// than draw text past its bounds.
const MAX_FONT_REBINDS: u32 = 3;

fn text_font_generation() -> Option<u64> {
    crate::node::source::text::text_system()
        .ok()
        .map(|text_system| text_system.font_generation())
}

fn timeline_key(composition: &Composition) -> (u32, u32) {
    let timeline = &composition.timeline;
    (timeline.duration_frames, timeline.fps.to_bits())
}

fn templates_key(composition: &Composition) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    for (name, template) in &composition.compositions {
        let settings = &template.render_settings;
        let timeline = &template.timeline;
        std::hash::Hash::hash(
            &(
                name,
                settings.width,
                settings.height,
                timeline.duration_frames,
                timeline.fps.to_bits(),
            ),
            &mut hasher,
        );
    }
    std::hash::Hasher::finish(&hasher)
}

/// Order-independent hash of everything `CompiledPlanKey::matches` reads from a composition, plus
/// the canvas size and timeline that plan estimates depend on.
fn plan_node_signature(composition: &Composition) -> u64 {
    let settings = &composition.render_settings;
    let mut hasher = std::hash::DefaultHasher::new();
    std::hash::Hash::hash(
        &(
            settings.width,
            settings.height,
            timeline_key(composition),
            templates_key(composition),
        ),
        &mut hasher,
    );
    let initial = std::hash::Hasher::finish(&hasher);
    let graphs = std::iter::once((None, &composition.graph)).chain(
        composition
            .compositions
            .iter()
            .map(|(name, template)| (Some(name), &template.graph)),
    );
    graphs.fold(initial, |signature, (template, graph)| {
        graph
            .nodes
            .iter()
            .fold(signature, |signature: u64, (id, node)| {
                let entry = match node {
                    NodeKind::TimeRemap(_) => (id, 0, 0),
                    NodeKind::Switch(_) => (id, 1, 0),
                    NodeKind::WgslShader(shader) => match &shader.params.shader {
                        Deferred::Value(source) => (id, 2, source_hash(source)),
                        Deferred::Expr(_) => (id, 3, 0),
                    },
                    NodeKind::Clip(clip) => (id, 4, clip_definition_hash(composition, clip)),
                    _ => return signature,
                };
                let mut hasher = std::hash::DefaultHasher::new();
                std::hash::Hash::hash(&(template, entry), &mut hasher);
                signature.wrapping_add(std::hash::Hasher::finish(&hasher))
            })
    })
}

/// Hash of a clip's template, the props that template declares, and the literals and expression
/// sources of the clip's params, which decide where its content draws over the frames a plan
/// may bind it at.
fn clip_definition_hash(
    composition: &Composition,
    clip: &crate::node::compositing::clip::Clip,
) -> u64 {
    fn param<T: std::fmt::Debug>(value: &Deferred<T>) -> String {
        match value {
            Deferred::Value(value) => format!("{value:?}"),
            Deferred::Expr(expr) => format!("={}", expr.source),
        }
    }
    let params = &clip.params;
    let props = match &params.props {
        Deferred::Value(props) => props.to_json_value().to_string(),
        Deferred::Expr(expr) => format!("={}", expr.source),
    };
    let declared = clip
        .template_name()
        .and_then(|name| composition.compositions.get(name))
        .map(|template| format!("{:?}", template.props));
    let mut hasher = std::hash::DefaultHasher::new();
    std::hash::Hash::hash(
        &(
            param(&params.composition),
            [
                &params.start,
                &params.duration,
                &params.offset,
                &params.speed,
            ]
            .map(param),
            format!("{:?}", params.end),
            props,
            declared,
        ),
        &mut hasher,
    );
    std::hash::Hasher::finish(&hasher)
}

fn source_hash(source: &str) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    std::hash::Hash::hash(source, &mut hasher);
    std::hash::Hasher::finish(&hasher)
}

fn fit_frame_to_rgba8(frame: &CpuMediaFrame, width: u32, height: u32) -> Vec<u8> {
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
