use lumen::{
    composition::Composition,
    media::collect_frame_requirements,
    render::{LumenRenderer as CoreRenderer, surface::DefaultSurfacePool},
};
use wasm_bindgen::{prelude::*, Clamped, JsCast};

use web_sys::{CanvasRenderingContext2d, ImageData, OffscreenCanvasRenderingContext2d};
use crate::{
    media::LumenMediaStore, types::FrameRequirementsPayload, utils::composition_json_to_composition,
};

#[wasm_bindgen]
pub struct LumenRenderer {
    composition: Option<Composition>,
    surface_pool: DefaultSurfacePool,
    pixels: Vec<u8>,
    width: usize,
    height: usize,
    duration_frames: u32,
}

#[wasm_bindgen]
impl LumenRenderer {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            composition: None,
            surface_pool: DefaultSurfacePool::new(),
            pixels: Vec::new(),
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
        self.pixels
            .resize(self.width.saturating_mul(self.height).saturating_mul(4), 0);
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
        self.pixels.clear();
    }

    /// Render a frame and draw premultiplied RGBA pixels into a 2D canvas context.
    #[wasm_bindgen(js_name = "renderFrame")]
    pub fn render_frame(
        &mut self,
        frame: u32,
        media: &LumenMediaStore,
        context: JsValue,
    ) -> Result<(), JsValue> {
        let (width, height) = self.render_frame_into_pixels(frame, media)?;
        draw_pixels_to_context(&context, width, height, &self.pixels)
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

    fn render_frame_into_pixels(
        &mut self,
        frame: u32,
        media: &LumenMediaStore,
    ) -> Result<(u32, u32), JsValue> {
        self.validate_frame(frame)?;
        let composition = self.require_composition()?;
        let store = media.as_wasm_store();

        let mut core = CoreRenderer::new(composition, &self.surface_pool, store)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let mut raster = core
            .render(frame)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let (w, h) = raster.storage_dimensions();

        let needed = (w as usize).saturating_mul(h as usize).saturating_mul(4);
        if self.pixels.len() != needed {
            self.pixels.resize(needed, 0);
        }
        raster
            .read_pixels_into(&mut self.pixels, (w as usize) * 4)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        Ok((w, h))
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

fn draw_pixels_to_context(
    context: &JsValue,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Result<(), JsValue> {
    let image = ImageData::new_with_u8_clamped_array_and_sh(Clamped(pixels), width, height)?;
    if let Some(context_2d) = context.dyn_ref::<CanvasRenderingContext2d>() {
        context_2d.put_image_data(&image, 0.0, 0.0)?;
        return Ok(());
    }
    if let Some(context_2d) = context.dyn_ref::<OffscreenCanvasRenderingContext2d>() {
        context_2d.put_image_data(&image, 0.0, 0.0)?;
        return Ok(());
    }
    Err(JsValue::from_str(
        "unsupported canvas context: expected 2d rendering context",
    ))
}
