use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::{
    composition::Composition,
    error::RenderError,
    gpu::{BoundFrame, MediaTextureKey, RasterHandle, frame_graph::FrameGraph},
    media::{CpuMediaFrame, MediaStore},
    node::NodeId,
};

#[cfg(feature = "ffmpeg")]
use super::media::GpuMediaFrameImporter;

/// Estimated GPU memory image textures nothing binds may hold before the least recently used are
/// freed.
pub const DEFAULT_MEDIA_TEXTURE_CACHE_BUDGET_BYTES: u64 = 256 << 20;

struct CachedMediaTexture {
    texture: Arc<lumen_gpu::wgpu::Texture>,
    last_used: u64,
}

/// Image textures, which are never written once cached. Replaced images get new keys, so textures
/// nothing binds are freed least recently used first once they exceed the budget; textures something
/// binds don't count against it and always stay.
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
    /// after anything that may have released a binding.
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

/// How an 8-bit output holds alpha. Float outputs take the working space as it is: linear light,
/// premultiplied, which a reader that drops alpha shows over black.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OutputAlpha {
    /// sRGB-encoded colour multiplied by alpha, as canvases and compositors take it. No channel is
    /// above alpha, and a pixel of alpha 0 is transparent black.
    #[default]
    Premultiplied,
    /// sRGB-encoded colour apart from its alpha, as PNG holds it. A pixel of alpha 0 is
    /// transparent black.
    Straight,
    /// Shown over black at full alpha, for encoders and formats without alpha.
    Opaque,
}

/// How `media_output` writes a frame's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputEncoding {
    pub(crate) alpha: OutputAlpha,
    /// Dithers the quantisation to 8 bits.
    pub(crate) dither: bool,
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

/// Most glyph, stroke and decoration instances one text node draws unless the host sets another
/// limit: a full 1920 × 1080 frame of 16 px monospace text.
pub const DEFAULT_MAX_TEXT_GLYPHS: usize = 16_384;

/// Largest text limit a host can set. The instances of a text node are bound as one storage
/// buffer, and this many fill the 128 MiB that WebGPU guarantees a binding.
pub const MAX_TEXT_GLYPHS_CEILING: usize = 1 << 20;

pub struct GpuCompositionRenderer {
    renderer: lumen_gpu::Renderer,
    graph: FrameGraph,
    bind_sequence: u64,
    text_atlas: Arc<Mutex<lumen_text::GlyphAtlas>>,
    text_atlas_texture: Option<Arc<lumen_gpu::wgpu::Texture>>,
    output_format: lumen_gpu::wgpu::TextureFormat,
    output_encoding: OutputEncoding,
    reuse_static_output: bool,
    full_frame_sources: bool,
    single_tile_repeats: bool,
    max_text_glyphs: usize,
    max_image_side: u32,
    media_texture_cache: MediaTextureCache,
    #[cfg(feature = "ffmpeg")]
    gpu_media_importer: GpuMediaFrameImporter,
}

impl GpuCompositionRenderer {
    /// Creates a renderer with [`lumen_gpu::GpuSettings::from_env`].
    pub async fn new() -> crate::Result<Self> {
        Self::new_with_settings(&lumen_gpu::GpuSettings::from_env()).await
    }

    /// Creates a renderer that reads no environment variables.
    pub async fn new_with_settings(settings: &lumen_gpu::GpuSettings) -> crate::Result<Self> {
        let renderer = lumen_gpu::Renderer::new_with_settings(settings)
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
            graph: FrameGraph::new(renderer.device.clone(), renderer.queue.clone()),
            renderer,
            bind_sequence: 0,
            text_atlas: Arc::new(Mutex::new(lumen_text::GlyphAtlas::new(
                super::types::TEXT_ATLAS_SIZE,
            ))),
            text_atlas_texture: None,
            output_format: lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            output_encoding: OutputEncoding {
                alpha: OutputAlpha::Premultiplied,
                dither: true,
            },
            reuse_static_output: true,
            full_frame_sources: false,
            single_tile_repeats: false,
            max_text_glyphs: DEFAULT_MAX_TEXT_GLYPHS,
            max_image_side: u32::MAX,
            media_texture_cache: MediaTextureCache::new(),
            #[cfg(feature = "ffmpeg")]
            gpu_media_importer: GpuMediaFrameImporter::default(),
        }
    }

    /// Sets the most glyph, stroke and decoration instances one text node may draw, which bounds
    /// the work a composition can ask of its text. A text over it is a render error. The default is
    /// [`DEFAULT_MAX_TEXT_GLYPHS`], and values are held to [`MAX_TEXT_GLYPHS_CEILING`]. Set it
    /// before rendering: frames kept from before are not checked again.
    pub fn set_max_text_glyphs(&mut self, glyphs: usize) {
        self.max_text_glyphs = glyphs.min(MAX_TEXT_GLYPHS_CEILING);
    }

    /// Sets the longest side an image is decoded at where its uses let it be reduced (only literal
    /// `resize` nodes read it). Such an image past it, or past the device's texture limit, is shown
    /// from a copy reduced to fit, with a `media.image_reduced` warning. Any other image keeps its
    /// own size. There is no limit but the device's by default. Drops what frames recorded so far
    /// when it changes.
    pub fn set_max_image_side(&mut self, pixels: u32) {
        let pixels = pixels.max(1);
        if self.max_image_side != pixels {
            self.max_image_side = pixels;
            self.reset_recorded(self.output_format);
        }
    }

    /// Bounds what recorded frames keep in pooled textures.
    #[doc(hidden)]
    pub fn set_frame_graph_pool_budget(&mut self, bytes: u64) {
        self.graph.recorder.set_pool_budget(bytes);
    }

    /// Holds the bytes of storage a frame's data may bind and take in all below the device's.
    #[doc(hidden)]
    pub fn limit_frame_storage(&mut self, max_binding: u64, max_buffer: u64) {
        self.graph.recorder.limit_storage(max_binding, max_buffer);
    }

    /// Bounds what recorded frames keep of static outputs once frames stop using them.
    #[doc(hidden)]
    pub fn set_frame_graph_retained_budget(&mut self, bytes: u64) {
        self.graph.set_retained_budget(bytes);
    }

    /// The size, format and usage of the output of the frame last rendered.
    pub fn output_desc(&self) -> Option<lumen_gpu::TextureDesc> {
        self.graph.output_desc()
    }

    /// The domain a node of the root graph had in the frame last recorded.
    #[doc(hidden)]
    pub fn recorded_domain(
        &self,
        composition: &Composition,
        node: NodeId,
    ) -> Option<lumen_gpu::TextureDomain> {
        self.graph.node_domain(composition, node)
    }

    /// The shared text atlas, once a frame has needed one.
    #[doc(hidden)]
    pub fn text_atlas_texture(&self) -> Option<Arc<lumen_gpu::wgpu::Texture>> {
        self.text_atlas_texture.clone()
    }

    /// The passes the frame last recorded.
    #[doc(hidden)]
    pub fn recorded_pass_count(&self) -> usize {
        self.graph.recorder.pass_count()
    }

    /// What recording has cost so far.
    pub fn frame_graph_stats(&self) -> FrameGraphStats {
        let recorder = &self.graph.recorder;
        FrameGraphStats {
            recorder: recorder.stats(),
            held_bytes: recorder.held_bytes(),
            adopted_bytes: recorder.adopted_bytes(),
            retained_bytes: self.graph.retained_bytes(),
            bind_groups: recorder.bind_group_count(),
            programs: recorder.program_count(),
        }
    }

    /// Sets how an 8-bit output holds alpha, [`OutputAlpha::Premultiplied`] by default. Drops
    /// what frames recorded so far when it changes.
    pub fn set_output_alpha(&mut self, alpha: OutputAlpha) {
        if self.output_encoding.alpha != alpha {
            self.output_encoding.alpha = alpha;
            self.reset_recorded(self.output_format);
        }
    }

    /// Sets whether quantising the output to 8 bits adds an ordered dither, which breaks up banding
    /// in smooth gradients and keeps the mean of flat areas. It is on by default. A value within a
    /// tenth of a code of one keeps it, so 8-bit content passes through exactly. Float outputs are
    /// never quantised. Drops what frames recorded so far when it changes.
    pub fn set_output_dither(&mut self, enabled: bool) {
        if self.output_encoding.dither != enabled {
            self.output_encoding.dither = enabled;
            self.reset_recorded(self.output_format);
        }
    }

    /// Builds every program `composition` may record with, so that a literal shader that doesn't
    /// compile fails here, before any frame renders.
    pub fn compile(&mut self, composition: &Composition) -> crate::Result<()> {
        self.compile_with_output_format(composition, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm)
    }

    /// `compile`, for frames whose output is in `output_format`. An 8-bit format takes the output
    /// sRGB-encoded and quantised once, and an `*Srgb` format encodes it as it stores. A float
    /// format takes the working space unchanged, linear and premultiplied, for a converter that
    /// encodes and quantises it itself.
    pub fn compile_with_output_format(
        &mut self,
        composition: &Composition,
        output_format: lumen_gpu::wgpu::TextureFormat,
    ) -> crate::Result<()> {
        self.reset_recorded(output_format);
        self.graph.prepare(composition, output_format)
    }

    /// Sets whether nodes without expressions or media keep their output from earlier frames
    /// instead of rendering it again, which is on by default. Turning it off disables that
    /// automatic reuse, as when measuring render cost. Drops what frames recorded so far when it
    /// changes.
    pub fn set_static_output_reuse(&mut self, enabled: bool) {
        if self.reuse_static_output != enabled {
            self.reuse_static_output = enabled;
            self.reset_recorded(self.output_format);
        }
    }

    /// Sets whether every source fills the canvas instead of being sized to its bounds, which
    /// gives tests a reference render for those bounds. Drops what frames recorded so far when it
    /// changes.
    #[doc(hidden)]
    pub fn set_full_frame_sources(&mut self, enabled: bool) {
        if self.full_frame_sources != enabled {
            self.full_frame_sources = enabled;
            self.reset_recorded(self.output_format);
        }
    }

    /// Sets whether repeated shapes and paths draw their copies from one tile covering their
    /// texture instead of from tiles that each list the copies reaching them, which gives tests
    /// an untiled reference. Drops what frames recorded so far when it changes.
    #[doc(hidden)]
    pub fn set_single_tile_repeats(&mut self, enabled: bool) {
        if self.single_tile_repeats != enabled {
            self.single_tile_repeats = enabled;
            self.reset_recorded(self.output_format);
        }
    }

    /// Estimated GPU memory held by pooled textures, excluding the shared text atlas and media
    /// cache.
    pub fn held_bytes(&self) -> u64 {
        self.graph.recorder.held_bytes()
    }

    /// Number of programs built, rather than reused from an earlier frame or composition.
    pub fn created_program_count(&self) -> u64 {
        self.graph.recorder.stats().programs_created
    }

    /// Number of passes submitted frames recorded.
    pub fn submitted_pass_count(&self) -> u64 {
        self.graph.recorder.stats().passes
    }

    /// Number of passes frames skipped by reading output kept from an earlier frame: that of nodes
    /// without frame-dependent params.
    pub fn skipped_pass_count(&self) -> u64 {
        self.graph.skipped_passes()
    }

    /// The warnings frames have raised since the last call, each once per composition: a text
    /// family that fell back, for one. Their paths point at the root or the template the node is
    /// in.
    pub fn take_diagnostics(&mut self) -> Vec<crate::diagnostic::Diagnostic> {
        self.graph.take_diagnostics()
    }

    /// Number of image textures kept in the media cache.
    #[doc(hidden)]
    pub fn cached_media_texture_count(&self) -> usize {
        self.media_texture_cache.textures.len()
    }

    /// Number of image textures created and uploaded, rather than reused from the cache.
    #[doc(hidden)]
    pub fn media_texture_upload_count(&self) -> u64 {
        self.media_texture_cache.upload_count
    }

    /// Sets the estimated GPU memory image textures nothing binds may hold, which textures
    /// something binds don't count against. The least recently used unbound ones past it are
    /// freed as frames release them.
    pub fn set_media_texture_cache_budget(&mut self, bytes: u64) {
        self.media_texture_cache.budget = bytes;
        self.media_texture_cache.evict();
    }

    /// Drops what frames recorded so far and the cached media textures, destroying the GPU
    /// textures this renderer exclusively owns instead of waiting for them to be garbage collected.
    /// Retains the shared text atlas (64 MiB once allocated) and cached glyphs.
    pub fn release_resources(&mut self) {
        self.graph.release();
        self.bind_sequence += 1;
        self.media_texture_cache.destroy_all();
    }

    /// `release_resources`, along with the text atlas and its cached glyphs.
    pub fn release_text_atlas(&mut self) {
        self.release_resources();
        if let Some(texture) = self.text_atlas_texture.take() {
            texture.destroy();
        }
        self.text_atlas = Arc::new(Mutex::new(lumen_text::GlyphAtlas::new(
            super::types::TEXT_ATLAS_SIZE,
        )));
    }

    fn reset_recorded(&mut self, output_format: lumen_gpu::wgpu::TextureFormat) {
        tracing::debug!(
            target: "lumen_render",
            ?output_format,
            media_textures = self.media_texture_cache.textures.len(),
            "reset recorded composition"
        );
        self.graph.reset();
        self.bind_sequence += 1;
        self.output_format = output_format;
        self.media_texture_cache.evict();
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
        self.submit_bound_frame(&bound)
    }

    /// Renders a frame whose output is `external`, which then stays with the caller: the graph
    /// holds no other claim on it once it is the only one left holding the texture.
    pub fn render_frame_into_external<M: MediaStore>(
        &mut self,
        composition: &Composition,
        frame: u32,
        media: &M,
        external: lumen_gpu::ExternalTexture,
    ) -> crate::Result<lumen_gpu::SubmittedExternalTexture> {
        let span = tracing::trace_span!(target: "lumen_render", "render frame into external texture", frame);
        let _entered = span.enter();
        let texture = external.texture_arc();
        self.graph.set_external(external);
        let bound = self.bind_frame(composition, frame, media);
        self.graph.take_external();
        self.upload_bound_frame(&bound?)?;
        let submission = self
            .graph
            .recorder
            .submit()
            .map_err(super::frame_graph::gpu_error)?;
        self.graph.finish_frame();
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

    /// Records a frame at a fractional frame time. Expressions see the fractional `frame`/`time`;
    /// video inputs show the source frame displayed at that instant.
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
        let bound = self.graph.record(super::frame_graph::RecordInput {
            composition,
            frame,
            media: Some(media),
            output_format: self.output_format,
            output_encoding: self.output_encoding,
            full_frame_sources: self.full_frame_sources,
            single_tile_repeats: self.single_tile_repeats,
            retain_static: self.reuse_static_output,
            media_cache: &mut self.media_texture_cache,
            text_atlas: &self.text_atlas,
            atlas_texture: &mut self.text_atlas_texture,
            max_rebinds: max_font_rebinds(composition),
            max_text_glyphs: self.max_text_glyphs,
            max_image_side: self.max_image_side,
            #[cfg(feature = "ffmpeg")]
            renderer: &self.renderer,
            #[cfg(feature = "ffmpeg")]
            importer: &mut self.gpu_media_importer,
        });
        let mut bound = bound.inspect_err(|_| {
            self.graph.abandon_frame();
            self.media_texture_cache.evict();
        })?;
        bound.sequence = self.bind_sequence;
        Ok(bound)
    }

    pub fn submit_bound_frame(
        &mut self,
        bound: &BoundFrame,
    ) -> crate::Result<(RasterHandle, lumen_gpu::wgpu::SubmissionIndex)> {
        self.upload_bound_frame(bound)?;
        self.submit_render()
    }

    /// Writes what `bound` recorded to the GPU, which only the frame recorded last can do.
    pub fn upload_bound_frame(&mut self, bound: &BoundFrame) -> crate::Result<()> {
        if bound.sequence != self.bind_sequence {
            return Err(RenderError::Internal {
                details: "stale recorded frame; record it again before uploading".to_string(),
            }
            .into());
        }
        let result = self.graph.upload(bound, self.text_atlas_texture.as_deref());
        self.media_texture_cache.evict();
        result
    }

    pub fn submit_render(
        &mut self,
    ) -> crate::Result<(RasterHandle, lumen_gpu::wgpu::SubmissionIndex)> {
        let output = self.graph.output().ok_or_else(|| RenderError::Internal {
            details: "no recorded frame".to_string(),
        })?;
        let texture = Arc::clone(self.graph.recorder.texture(output.tex));
        let submission = self
            .graph
            .recorder
            .submit()
            .map_err(super::frame_graph::gpu_error)?;
        self.graph.finish_frame();
        Ok((
            RasterHandle {
                texture,
                domain: output.domain,
            },
            submission,
        ))
    }

    pub fn gpu_renderer(&self) -> &lumen_gpu::Renderer {
        &self.renderer
    }

    /// Fails with the error a render of `composition` gives when its canvas is larger than the
    /// device's texture limit, for callers that create the output texture themselves.
    pub fn check_output_size(&self, composition: &Composition) -> crate::Result<()> {
        let settings = composition.render_settings();
        let canvas = lumen_gpu::Size::new(settings.width.max(1), settings.height.max(1));
        let output = &composition.graph().nodes[&composition.output()];
        let max = self.renderer.device.limits().max_texture_dimension_2d;
        super::frame_graph::ensure_texture_fits(output, canvas, canvas, max)
    }
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

pub(super) fn fit_frame_to_rgba8(frame: &CpuMediaFrame, width: u32, height: u32) -> Cow<'_, [u8]> {
    if frame.width == width && frame.height == height && frame.row_bytes == width as usize * 4 {
        return Cow::Borrowed(frame.rgba.as_slice());
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
    Cow::Owned(out)
}
