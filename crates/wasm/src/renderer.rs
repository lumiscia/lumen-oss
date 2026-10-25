use lumen_engine::{
    composition::Composition, gpu::GpuCompositionRenderer, media::collect_frame_requirements,
};
use wasm_bindgen::prelude::*;

use crate::{
    debug_error, install_panic_hook, media::LumenMediaStore, types::FrameRequirementsPayload,
    utils::composition_json_to_composition,
};

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
#[derive(Debug)]
struct WebDisplayHandle;

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
impl lumen_gpu::wgpu::rwh::HasDisplayHandle for WebDisplayHandle {
    fn display_handle(
        &self,
    ) -> Result<lumen_gpu::wgpu::rwh::DisplayHandle<'_>, lumen_gpu::wgpu::rwh::HandleError> {
        Ok(lumen_gpu::wgpu::rwh::DisplayHandle::web())
    }
}

#[wasm_bindgen]
pub struct LumenRenderer {
    composition: Option<Composition>,
    renderer: Option<SurfaceCompositionRenderer>,
    width: usize,
    height: usize,
    duration_frames: u32,
    lookahead_count: u32,
}

#[wasm_bindgen]
impl LumenRenderer {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        install_panic_hook();
        Self {
            composition: None,
            renderer: None,
            width: 0,
            height: 0,
            duration_frames: 0,
            lookahead_count: 8,
        }
    }

    #[wasm_bindgen(js_name = "loadComposition")]
    pub fn load_composition(&mut self, composition_json: &str) -> Result<(), JsValue> {
        let composition =
            composition_json_to_composition(composition_json).map_err(|e| JsValue::from_str(&e))?;

        self.width = composition.render_settings.width as usize;
        self.height = composition.render_settings.height as usize;
        self.duration_frames = composition.timeline.duration_frames;
        tracing::debug!(
            target: "lumen_wasm",
            width = self.width,
            height = self.height,
            duration_frames = self.duration_frames,
            "renderer loaded composition"
        );
        self.composition = Some(composition);
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.invalidate();
        }
        Ok(())
    }

    pub fn width(&self) -> u32 {
        self.width as u32
    }

    pub fn height(&self) -> u32 {
        self.height as u32
    }

    #[wasm_bindgen(js_name = "durationFrames")]
    pub fn duration_frames(&self) -> u32 {
        self.duration_frames
    }

    /// Unloads the composition and destroys the GPU device.
    pub fn clear(&mut self) {
        self.composition = None;
        self.renderer = None;
        self.width = 0;
        self.height = 0;
        self.duration_frames = 0;
    }

    #[wasm_bindgen(js_name = "setLookaheadCount")]
    pub fn set_lookahead_count(&mut self, lookahead_count: u32) {
        self.lookahead_count = lookahead_count;
    }

    #[wasm_bindgen(js_name = "setLogLevel")]
    pub fn set_log_level(&mut self, level: &str) -> Result<(), JsValue> {
        lumen_engine::set_log_level_from_str(level).map_err(|error| JsValue::from_str(&error))?;
        tracing::debug!(target: "lumen_wasm", level, "set log level");
        Ok(())
    }

    #[wasm_bindgen(js_name = "renderFrame")]
    pub async fn render_frame_html_canvas(
        &mut self,
        frame: u32,
        media: &LumenMediaStore,
        #[wasm_bindgen(unchecked_param_type = "HTMLCanvasElement")] canvas: JsValue,
    ) -> Result<(), JsValue> {
        self.render_frame_to_canvas(frame, media, canvas).await
    }

    #[wasm_bindgen(js_name = "renderFrameToOffscreenCanvas")]
    pub async fn render_frame_offscreen_canvas(
        &mut self,
        frame: u32,
        media: &LumenMediaStore,
        #[wasm_bindgen(unchecked_param_type = "OffscreenCanvas")] canvas: JsValue,
    ) -> Result<(), JsValue> {
        self.render_frame_to_canvas(frame, media, canvas).await
    }

    #[wasm_bindgen(js_name = "frameRequirements")]
    pub fn frame_requirements(
        &self,
        frame: u32,
        media: &LumenMediaStore,
    ) -> Result<String, JsValue> {
        self.validate_frame(frame)?;
        let composition = self.require_composition()?;
        let store = media.as_wasm_store();
        let payload = collect_frame_requirements(composition, store, frame)
            .map(FrameRequirementsPayload::from)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        serde_json::to_string(&payload).map_err(|e| JsValue::from_str(&e.to_string()))
    }

    #[wasm_bindgen(js_name = "frameRequirementsWindow")]
    pub fn frame_requirements_window(
        &self,
        frame: u32,
        media: &LumenMediaStore,
    ) -> Result<String, JsValue> {
        self.validate_frame(frame)?;
        let composition = self.require_composition()?;
        let store = media.as_wasm_store();
        let payload = collect_requirement_window(composition, store, frame, self.lookahead_count)?;
        serde_json::to_string(&payload).map_err(|e| JsValue::from_str(&e.to_string()))
    }
}

impl Default for LumenRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl LumenRenderer {
    fn require_composition(&self) -> Result<&Composition, JsValue> {
        self.composition
            .as_ref()
            .ok_or_else(|| JsValue::from_str("composition not loaded"))
    }

    async fn render_frame_to_canvas(
        &mut self,
        frame: u32,
        media: &LumenMediaStore,
        canvas: JsValue,
    ) -> Result<(), JsValue> {
        tracing::trace!(target: "lumen_wasm", frame, "render frame to canvas");
        self.validate_frame(frame)?;
        self.require_composition()?;
        if self.renderer.is_none() {
            self.renderer = Some(SurfaceCompositionRenderer::new(&canvas).await?);
        }
        let composition = self
            .composition
            .as_ref()
            .ok_or_else(|| JsValue::from_str("composition not loaded"))?;
        let renderer = self
            .renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer is unavailable"))?;
        if let Ok(requirements) =
            collect_frame_requirements(composition, media.as_wasm_store(), frame)
        {
            let _ = media.as_wasm_store().pin_video_frames(&requirements.videos);
        }
        let size = renderer.render_frame(
            &canvas,
            composition,
            frame,
            self.lookahead_count,
            media.as_wasm_store(),
        )?;
        self.width = size.width as usize;
        self.height = size.height as usize;
        tracing::trace!(
            target: "lumen_wasm",
            frame,
            width = self.width,
            height = self.height,
            "rendered frame to canvas"
        );
        Ok(())
    }

    fn validate_frame(&self, frame: u32) -> Result<(), JsValue> {
        if self.duration_frames == 0 {
            return Ok(());
        }
        if frame >= self.duration_frames {
            return Err(JsValue::from_str("frame index out of range"));
        }
        Ok(())
    }
}

/// Owns the GPU device and canvas surface for the lifetime of a preview. Composition and media
/// changes only recompile render plans on the existing device.
pub(crate) struct SurfaceCompositionRenderer {
    renderer: GpuCompositionRenderer,
    gpu: SurfaceGpu,
    needs_compile: bool,
}

impl SurfaceCompositionRenderer {
    pub(crate) async fn new(canvas: &JsValue) -> Result<Self, JsValue> {
        let gpu = SurfaceGpu::new(canvas).await?;
        let renderer = GpuCompositionRenderer::from_device(gpu.device.clone(), gpu.queue.clone());
        Ok(Self {
            renderer,
            gpu,
            needs_compile: true,
        })
    }

    /// Recompiles render plans before the next frame, keeping the device and surface.
    pub(crate) fn invalidate(&mut self) {
        self.needs_compile = true;
    }

    pub(crate) fn render_frame<M: lumen_engine::media::MediaStore>(
        &mut self,
        canvas: &JsValue,
        composition: &Composition,
        frame: u32,
        lookahead_count: u32,
        media: &M,
    ) -> Result<lumen_gpu::Size, JsValue> {
        let format = self.gpu.config.format;
        self.gpu.prepare(
            canvas,
            composition.render_settings.width,
            composition.render_settings.height,
        )?;
        if self.needs_compile || self.gpu.config.format != format {
            tracing::debug!(target: "lumen_wasm", frame, "recompile on existing GPU device");
            self.renderer.release_resources();
            self.renderer
                .compile_with_media(composition, media, self.gpu.config.format)
                .map_err(|e| render_error(frame, e))?;
            self.needs_compile = false;
        }
        let (raster, _submission) = self
            .renderer
            .render_frame_submitted(composition, frame, media)
            .map_err(|e| render_error(frame, e))?;
        let surface_texture = current_surface_texture(&self.gpu.surface)?;
        self.renderer
            .gpu_renderer()
            .copy_texture_to_external(raster.texture, &surface_texture.texture)
            .map_err(|error| JsValue::from_str(&format!("surface copy failed: {error}")))?;
        self.renderer.gpu_renderer().queue.present(surface_texture);
        let _ = self.renderer.precompile_frame_window(
            composition,
            frame.saturating_add(1),
            lookahead_count,
            media,
        );
        Ok(raster.domain.storage_size)
    }
}

impl Drop for SurfaceCompositionRenderer {
    // Dropping wgpu handles is a no-op on WebGPU; destroying the device frees its memory now.
    fn drop(&mut self) {
        self.gpu.device.destroy();
    }
}

fn render_error(frame: u32, error: lumen_engine::error::LumenError) -> JsValue {
    debug_error(&format!("[lumen-wasm] render frame={frame} error: {error}"));
    JsValue::from_str(&error.to_string())
}

struct SurfaceGpu {
    instance: lumen_gpu::wgpu::Instance,
    adapter: lumen_gpu::wgpu::Adapter,
    device: lumen_gpu::wgpu::Device,
    queue: lumen_gpu::wgpu::Queue,
    canvas: JsValue,
    surface: lumen_gpu::wgpu::Surface<'static>,
    config: lumen_gpu::wgpu::SurfaceConfiguration,
    configured: bool,
}

impl SurfaceGpu {
    /// Requests a device without configuring the canvas, so a discarded request never rebinds a
    /// canvas that a newer device already owns.
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    async fn new(canvas: &JsValue) -> Result<Self, JsValue> {
        let instance = lumen_gpu::wgpu::util::new_instance_with_webgpu_detection(
            lumen_gpu::wgpu::InstanceDescriptor::new_without_display_handle()
                .with_display_handle(Box::new(WebDisplayHandle)),
        )
        .await;
        let surface = create_surface(&instance, canvas)?;
        let adapter = instance
            .request_adapter(&lumen_gpu::wgpu::RequestAdapterOptions {
                power_preference: lumen_gpu::wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|error| JsValue::from_str(&format!("no compatible GPU adapter: {error}")))?;
        let config = surface_config(&surface, &adapter, 1, 1)?;
        let (device, queue) = adapter
            .request_device(&lumen_gpu::wgpu::DeviceDescriptor::default())
            .await
            .map_err(|error| JsValue::from_str(&format!("create GPU device failed: {error}")))?;
        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            canvas: canvas.clone(),
            surface,
            config,
            configured: false,
        })
    }

    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    async fn new(_canvas: &JsValue) -> Result<Self, JsValue> {
        Err(JsValue::from_str(
            "canvas GPU surfaces are only available on wasm",
        ))
    }

    /// Configures the canvas for this device on first use, after a canvas change, or on resize.
    fn prepare(&mut self, canvas: &JsValue, width: u32, height: u32) -> Result<(), JsValue> {
        let width = width.max(1);
        let height = height.max(1);
        if self.canvas != *canvas {
            self.surface = create_surface(&self.instance, canvas)?;
            self.config = surface_config(&self.surface, &self.adapter, width, height)?;
            self.canvas = canvas.clone();
            self.configured = false;
        }
        if self.configured && self.config.width == width && self.config.height == height {
            return Ok(());
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.configured = true;
        Ok(())
    }
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn create_surface(
    instance: &lumen_gpu::wgpu::Instance,
    canvas: &JsValue,
) -> Result<lumen_gpu::wgpu::Surface<'static>, JsValue> {
    use wasm_bindgen::JsCast;
    use web_sys::{HtmlCanvasElement, OffscreenCanvas};

    let target = if let Some(canvas) = canvas.dyn_ref::<HtmlCanvasElement>() {
        lumen_gpu::wgpu::SurfaceTarget::Canvas(canvas.clone())
    } else if let Some(canvas) = canvas.dyn_ref::<OffscreenCanvas>() {
        lumen_gpu::wgpu::SurfaceTarget::OffscreenCanvas(canvas.clone())
    } else {
        return Err(JsValue::from_str(
            "expected an HTMLCanvasElement or OffscreenCanvas",
        ));
    };
    instance
        .create_surface(target)
        .map_err(|error| JsValue::from_str(&format!("canvas GPU surface failed: {error}")))
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
fn create_surface(
    _instance: &lumen_gpu::wgpu::Instance,
    _canvas: &JsValue,
) -> Result<lumen_gpu::wgpu::Surface<'static>, JsValue> {
    Err(JsValue::from_str(
        "canvas GPU surfaces are only available on wasm",
    ))
}

fn surface_config(
    surface: &lumen_gpu::wgpu::Surface<'static>,
    adapter: &lumen_gpu::wgpu::Adapter,
    width: u32,
    height: u32,
) -> Result<lumen_gpu::wgpu::SurfaceConfiguration, JsValue> {
    let mut config = surface
        .get_default_config(adapter, width, height)
        .ok_or_else(|| JsValue::from_str("GPU surface is not compatible with the adapter"))?;
    config.usage |= lumen_gpu::wgpu::TextureUsages::COPY_DST;
    config.desired_maximum_frame_latency = 1;
    Ok(config)
}

fn current_surface_texture(
    surface: &lumen_gpu::wgpu::Surface<'static>,
) -> Result<lumen_gpu::wgpu::SurfaceTexture, JsValue> {
    match surface.get_current_texture() {
        lumen_gpu::wgpu::CurrentSurfaceTexture::Success(texture)
        | lumen_gpu::wgpu::CurrentSurfaceTexture::Suboptimal(texture) => Ok(texture),
        other => Err(JsValue::from_str(&format!(
            "surface texture unavailable: {other:?}"
        ))),
    }
}

pub(crate) fn collect_requirement_window(
    composition: &Composition,
    media: &crate::media::WasmMediaStore,
    frame: u32,
    lookahead_count: u32,
) -> Result<FrameRequirementsPayload, JsValue> {
    let mut requirements = lumen_engine::media::FrameRequirements::default();
    let duration = composition.timeline.duration_frames;
    let last_frame = if duration == 0 {
        frame
    } else {
        frame
            .saturating_add(lookahead_count)
            .min(duration.saturating_sub(1))
    };

    for predicted_frame in frame..=last_frame {
        let frame_requirements = collect_frame_requirements(composition, media, predicted_frame)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        requirements.images.extend(frame_requirements.images);
        requirements.videos.extend(frame_requirements.videos);
    }

    requirements.images.sort();
    requirements.images.dedup();
    let mut videos = std::collections::BTreeMap::<String, Vec<u32>>::new();
    for video in requirements.videos {
        videos
            .entry(video.stream_id)
            .or_default()
            .extend(video.frames);
    }
    requirements.videos = videos
        .into_iter()
        .map(|(stream_id, mut frames)| {
            frames.sort_unstable();
            frames.dedup();
            lumen_engine::media::VideoFrameRequirement { stream_id, frames }
        })
        .collect();

    Ok(FrameRequirementsPayload::from(requirements))
}
