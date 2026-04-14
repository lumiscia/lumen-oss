use lumen::{
    composition::Composition,
    media::collect_frame_requirements,
    raster::RasterFrame,
    render::{
        LumenRenderer as CoreRenderer,
        surface::{DefaultSurfacePool, SurfacePool},
    },
};
use wasm_bindgen::prelude::*;

use crate::{
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
}

#[wasm_bindgen]
impl LumenRenderer {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        ensure_webgl_backend();
        Self {
            composition: None,
            surface_pool: DefaultSurfacePool::new(),
            width: 0,
            height: 0,
            duration_frames: 0,
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
    ) -> Result<RasterFrame, JsValue> {
        self.validate_frame(frame)?;
        let composition = self.require_composition()?;
        let store = media.as_wasm_store();

        let mut core = CoreRenderer::new(composition, &self.surface_pool, store)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let raster = core
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
