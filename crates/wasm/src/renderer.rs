use lumen_engine::{
    composition::Composition, gpu::GpuCompositionRenderer, media::collect_frame_requirements,
};
use wasm_bindgen::prelude::*;

use crate::{
    debug_error, install_panic_hook, media::LumenMediaStore, types::FrameRequirementsPayload,
    utils::composition_json_to_composition,
};
use wasm_bindgen::JsCast;
use web_sys::{HtmlCanvasElement, OffscreenCanvas};

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
        self.renderer = None;
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
        self.render_frame_to_canvas(frame, media, RenderCanvas::from_js_value(&canvas)?)
            .await?;
        Ok(())
    }

    #[wasm_bindgen(js_name = "renderFrameToOffscreenCanvas")]
    pub async fn render_frame_offscreen_canvas(
        &mut self,
        frame: u32,
        media: &LumenMediaStore,
        #[wasm_bindgen(unchecked_param_type = "OffscreenCanvas")] canvas: JsValue,
    ) -> Result<(), JsValue> {
        self.render_frame_to_canvas(frame, media, RenderCanvas::from_js_value(&canvas)?)
            .await?;
        Ok(())
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

    async fn ensure_renderer(
        &mut self,
        media: &LumenMediaStore,
        canvas: RenderCanvas,
    ) -> Result<(), JsValue> {
        if self.renderer.is_some() {
            return Ok(());
        }
        let composition = self
            .composition
            .as_ref()
            .ok_or_else(|| JsValue::from_str("composition not loaded"))?;
        self.renderer = Some(
            create_surface_composition_renderer(
                canvas,
                composition,
                media.as_wasm_store(),
                self.width as u32,
                self.height as u32,
            )
            .await?,
        );
        Ok(())
    }

    async fn render_frame_to_canvas(
        &mut self,
        frame: u32,
        media: &LumenMediaStore,
        canvas: RenderCanvas,
    ) -> Result<(), JsValue> {
        tracing::trace!(target: "lumen_wasm", frame, "render frame to canvas");
        self.validate_frame(frame)?;
        self.ensure_renderer(media, canvas).await?;
        let composition = self
            .composition
            .as_ref()
            .ok_or_else(|| JsValue::from_str("composition not loaded"))?;
        let renderer = self
            .renderer
            .as_mut()
            .ok_or_else(|| JsValue::from_str("GPU renderer is unavailable"))?;
        let size = renderer
            .render_frame(composition, frame, media.as_wasm_store())
            .map_err(|e| {
                debug_error(&format!("[lumen-wasm] render frame={frame} error: {e}"));
                JsValue::from_str(&e.to_string())
            })?;
        let _ = renderer.precompile_frame_window(
            composition,
            frame.saturating_add(1),
            self.lookahead_count,
            media.as_wasm_store(),
        );
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

pub(crate) struct SurfaceCompositionRenderer {
    renderer: GpuCompositionRenderer,
    surface: lumen_gpu::wgpu::Surface<'static>,
}

impl SurfaceCompositionRenderer {
    pub fn render_frame<M: lumen_engine::media::MediaStore>(
        &mut self,
        composition: &Composition,
        frame: u32,
        media: &M,
    ) -> Result<lumen_gpu::Size, lumen_engine::error::LumenError> {
        let (raster, _render_submission) =
            self.renderer
                .render_frame_submitted(composition, frame, media)?;
        let surface_texture = current_surface_texture(&self.surface)?;
        self.renderer
            .gpu_renderer()
            .copy_texture_to_external(raster.texture, &surface_texture.texture)
            .map_err(|error| lumen_engine::error::RenderError::Gpu {
                details: error.to_string(),
            })?;
        surface_texture.present();
        Ok(raster.domain.storage_size)
    }

    pub fn precompile_frame_window<M: lumen_engine::media::MediaStore>(
        &mut self,
        composition: &Composition,
        start_frame: u32,
        frame_count: u32,
        media: &M,
    ) -> Result<(), lumen_engine::error::LumenError> {
        self.renderer
            .precompile_frame_window(composition, start_frame, frame_count, media)
    }
}

#[allow(dead_code)]
pub(crate) enum RenderCanvas {
    Html(HtmlCanvasElement),
    Offscreen(OffscreenCanvas),
}

impl RenderCanvas {
    pub(crate) fn from_js_value(canvas: &JsValue) -> Result<Self, JsValue> {
        if let Some(canvas) = canvas.dyn_ref::<HtmlCanvasElement>() {
            return Ok(Self::Html(canvas.clone()));
        }

        if let Some(canvas) = canvas.dyn_ref::<OffscreenCanvas>() {
            return Ok(Self::Offscreen(canvas.clone()));
        }

        Err(JsValue::from_str(
            "expected an HTMLCanvasElement or OffscreenCanvas",
        ))
    }
}

fn current_surface_texture(
    surface: &lumen_gpu::wgpu::Surface<'static>,
) -> Result<lumen_gpu::wgpu::SurfaceTexture, lumen_engine::error::LumenError> {
    match surface.get_current_texture() {
        lumen_gpu::wgpu::CurrentSurfaceTexture::Success(texture)
        | lumen_gpu::wgpu::CurrentSurfaceTexture::Suboptimal(texture) => Ok(texture),
        other => Err(lumen_engine::error::RenderError::Gpu {
            details: format!("surface texture unavailable: {other:?}"),
        }
        .into()),
    }
}

pub(crate) async fn create_surface_composition_renderer<M: lumen_engine::media::MediaStore>(
    canvas: RenderCanvas,
    composition: &Composition,
    media: &M,
    width: u32,
    height: u32,
) -> Result<SurfaceCompositionRenderer, JsValue> {
    let width = width.max(1);
    let height = height.max(1);
    let (device, queue, surface, format) = create_surface_device(canvas, width, height).await?;
    let mut renderer = GpuCompositionRenderer::from_device(device, queue);
    renderer
        .compile_with_media(composition, media, format)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    Ok(SurfaceCompositionRenderer { renderer, surface })
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
async fn create_surface_device(
    canvas: RenderCanvas,
    width: u32,
    height: u32,
) -> Result<
    (
        lumen_gpu::wgpu::Device,
        lumen_gpu::wgpu::Queue,
        lumen_gpu::wgpu::Surface<'static>,
        lumen_gpu::wgpu::TextureFormat,
    ),
    JsValue,
> {
    let instance = lumen_gpu::wgpu::util::new_instance_with_webgpu_detection(
        lumen_gpu::wgpu::InstanceDescriptor::new_without_display_handle()
            .with_display_handle(Box::new(WebDisplayHandle)),
    )
    .await;
    let surface = match canvas {
        RenderCanvas::Html(canvas) => {
            instance.create_surface(lumen_gpu::wgpu::SurfaceTarget::Canvas(canvas))
        }
        RenderCanvas::Offscreen(canvas) => {
            instance.create_surface(lumen_gpu::wgpu::SurfaceTarget::OffscreenCanvas(canvas))
        }
    }
    .map_err(|error| JsValue::from_str(&format!("canvas GPU surface failed: {error}")))?;
    let adapter = instance
        .request_adapter(&lumen_gpu::wgpu::RequestAdapterOptions {
            power_preference: lumen_gpu::wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        })
        .await
        .map_err(|error| JsValue::from_str(&format!("no compatible GPU adapter: {error}")))?;
    let mut config = surface
        .get_default_config(&adapter, width, height)
        .ok_or_else(|| JsValue::from_str("GPU surface is not compatible with the adapter"))?;
    config.usage |= lumen_gpu::wgpu::TextureUsages::COPY_DST;
    config.desired_maximum_frame_latency = 1;
    let format = config.format;
    let (device, queue) = adapter
        .request_device(&lumen_gpu::wgpu::DeviceDescriptor::default())
        .await
        .map_err(|error| JsValue::from_str(&format!("create GPU device failed: {error}")))?;
    surface.configure(&device, &config);
    Ok((device, queue, surface, format))
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
async fn create_surface_device(
    _canvas: RenderCanvas,
    _width: u32,
    _height: u32,
) -> Result<
    (
        lumen_gpu::wgpu::Device,
        lumen_gpu::wgpu::Queue,
        lumen_gpu::wgpu::Surface<'static>,
        lumen_gpu::wgpu::TextureFormat,
    ),
    JsValue,
> {
    Err(JsValue::from_str(
        "canvas GPU surfaces are only available on wasm",
    ))
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
