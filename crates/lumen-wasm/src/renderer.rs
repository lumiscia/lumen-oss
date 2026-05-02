use lumen::{
    composition::Composition,
    gpu_image::GpuImageFrame,
    media::collect_frame_requirements,
    render::{
        RenderOrchestrator, RenderOrchestratorConfig,
        surface::{DefaultSurfacePool, SurfacePool},
    },
};
use wasm_bindgen::prelude::*;

use crate::{
    install_panic_hook,
    media::LumenMediaStore,
    types::FrameRequirementsPayload,
    utils::composition_json_to_composition,
    webgl::{draw_output_frame_to_context, ensure_webgl_backend},
};
use web_sys::CanvasRenderingContext2d;

#[wasm_bindgen]
pub struct LumenRenderer {
    composition: Option<Composition>,
    surface_pool: DefaultSurfacePool,
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
        ensure_webgl_backend();
        Self {
            composition: None,
            surface_pool: DefaultSurfacePool::new(),
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
        self.composition = Some(composition);
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
        self.width = 0;
        self.height = 0;
        self.duration_frames = 0;
    }

    #[wasm_bindgen(js_name = "setLookaheadCount")]
    pub fn set_lookahead_count(&mut self, lookahead_count: u32) {
        self.lookahead_count = lookahead_count;
    }

    /// Render a frame directly into the target 2D canvas context.
    #[wasm_bindgen(js_name = "renderFrame")]
    pub fn render_frame(
        &mut self,
        frame: u32,
        media: &LumenMediaStore,
        context: CanvasRenderingContext2d,
    ) -> Result<(), JsValue> {
        let raster = self.render_frame_to_image(frame, media)?;
        let (width, height) = raster.storage_dimensions();
        draw_output_frame_to_context(&raster, &context)?;
        self.width = width as usize;
        self.height = height as usize;
        Ok(())
    }

    /// Returns frame media requirements as a JSON string.
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

impl LumenRenderer {
    fn require_composition(&self) -> Result<&Composition, JsValue> {
        self.composition
            .as_ref()
            .ok_or_else(|| JsValue::from_str("composition not loaded"))
    }

    fn render_frame_to_image(
        &mut self,
        frame: u32,
        media: &LumenMediaStore,
    ) -> Result<GpuImageFrame, JsValue> {
        self.validate_frame(frame)?;
        let composition = self.require_composition()?;
        let store = media.as_wasm_store();

        let orchestrator = RenderOrchestrator::new(
            composition,
            &self.surface_pool,
            store,
            RenderOrchestratorConfig {
                lookahead_count: self.lookahead_count,
            },
        );
        let raster = orchestrator
            .render(frame)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        self.surface_pool.flush();
        Ok(raster)
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

pub(crate) fn collect_requirement_window(
    composition: &Composition,
    media: &crate::media::WasmMediaStore,
    frame: u32,
    lookahead_count: u32,
) -> Result<FrameRequirementsPayload, JsValue> {
    let mut requirements = lumen::media::FrameRequirements::default();
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
            lumen::media::VideoFrameRequirement { stream_id, frames }
        })
        .collect();

    Ok(FrameRequirementsPayload::from(requirements))
}
