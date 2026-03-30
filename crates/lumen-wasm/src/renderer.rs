use lumen::{
    composition::Composition,
    media::collect_frame_requirements,
    render::{LumenRenderer as CoreRenderer, surface::DefaultSurfacePool},
};
use wasm_bindgen::prelude::*;

use crate::{
    media::LumenMediaStore, types::FrameRequirementsPayload, utils::project_bytes_to_composition,
};

#[wasm_bindgen]
pub struct LumenRenderer {
    width: usize,
    height: usize,
    composition: Composition,
    surface_pool: DefaultSurfacePool,
    pixels: Vec<u8>,
}

#[wasm_bindgen]
impl LumenRenderer {
    /// Load a composition from JSON.
    #[wasm_bindgen(js_name = "load")]
    pub fn load(project: &str, scale: f32) -> Result<Self, JsValue> {
        let composition = project_bytes_to_composition(project.as_bytes(), scale)
            .map_err(|e| JsValue::from_str(&e))?;
        let w = composition.render_settings.width as usize;
        let h = composition.render_settings.height as usize;
        Ok(Self {
            width: w,
            height: h,
            composition,
            surface_pool: DefaultSurfacePool::new(),
            pixels: vec![0u8; w * h * 4],
        })
    }

    pub fn width(&self) -> u32 {
        self.composition.render_settings.width
    }

    pub fn height(&self) -> u32 {
        self.composition.render_settings.height
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width as usize;
        self.height = height as usize;
        self.pixels
            .resize((self.width * self.height * 4) as usize, 0);
    }

    /// Render a frame and return premultiplied RGBA pixels.
    #[wasm_bindgen(js_name = "renderFrame")]
    pub fn render_frame(&mut self, frame: u32, media: &LumenMediaStore) -> Result<Vec<u8>, JsValue> {
        let store = media.as_wasm_store();
        let mut core = CoreRenderer::new(&self.composition, &self.surface_pool, store)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let raster = core
            .render(frame)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let (w, h) = raster.storage_dimensions();

        let needed = (w as usize) * (h as usize) * 4;
        if self.pixels.len() != needed {
            self.pixels.resize(needed, 0);
        }
        raster
            .read_pixels_into(&mut self.pixels, (w as usize) * 4)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        Ok(self.pixels.clone())
    }

    /// Returns frame media requirements as a JSON string.
    #[wasm_bindgen(js_name = "frameRequirements")]
    pub fn frame_requirements(
        &mut self,
        frame: u32,
        media: &LumenMediaStore,
    ) -> Result<String, JsValue> {
        let store = media.as_wasm_store();
        let payload = collect_frame_requirements(&self.composition, store, frame)
            .map(FrameRequirementsPayload::from)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        serde_json::to_string(&payload).map_err(|e| JsValue::from_str(&e.to_string()))
    }
}
