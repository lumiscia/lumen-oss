use std::cell::RefCell;

use lumen::{
    composition::Composition,
    media::{VideoMetadata, collect_frame_requirements},
    render::{
        LumenRenderer as CoreRenderer,
        surface::{DefaultSurfacePool, SurfacePool},
    },
};
use serde::Serialize;
use wasm_bindgen::prelude::*;

use crate::{
    media::WasmMediaStore,
    types::FrameRequirementsPayload,
    utils::{composition_json_to_composition, image_frame_from_rgba, validate_rgba_len},
    webgl::{draw_output_frame_to_context, ensure_webgl_backend, image_frame_from_video_frame},
};
use web_sys::CanvasRenderingContext2d;

#[derive(Debug, Clone, Copy, Default)]
struct RenderMetrics {
    last_ms: f64,
    avg_ms: f64,
    max_ms: f64,
    sample_count: u32,
}

impl RenderMetrics {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn update(&mut self, elapsed_ms: f64) {
        self.last_ms = elapsed_ms;
        self.sample_count = self.sample_count.saturating_add(1);
        self.avg_ms = if self.sample_count == 1 {
            elapsed_ms
        } else {
            ((self.avg_ms * f64::from(self.sample_count - 1)) + elapsed_ms)
                / f64::from(self.sample_count)
        };
        self.max_ms = self.max_ms.max(elapsed_ms);
    }
}

#[derive(Debug)]
struct PreviewState {
    composition: Option<Composition>,
    surface_pool: DefaultSurfacePool,
    width: usize,
    height: usize,
    duration_frames: u32,
    fps: f64,
    current_frame: u32,
    playing: bool,
    dirty: bool,
    last_tick_ms: Option<f64>,
    frame_accumulator_ms: f64,
    composition_sync_ms: f64,
    render_metrics: RenderMetrics,
}

impl Default for PreviewState {
    fn default() -> Self {
        Self {
            composition: None,
            surface_pool: DefaultSurfacePool::new(),
            width: 0,
            height: 0,
            duration_frames: 0,
            fps: 30.0,
            current_frame: 0,
            playing: false,
            dirty: false,
            last_tick_ms: None,
            frame_accumulator_ms: 0.0,
            composition_sync_ms: 0.0,
            render_metrics: RenderMetrics::default(),
        }
    }
}

impl PreviewState {
    fn clear(&mut self) {
        self.composition = None;
        self.width = 0;
        self.height = 0;
        self.duration_frames = 0;
        self.current_frame = 0;
        self.playing = false;
        self.dirty = false;
        self.last_tick_ms = None;
        self.frame_accumulator_ms = 0.0;
        self.composition_sync_ms = 0.0;
        self.render_metrics.reset();
    }

    fn ready(&self) -> bool {
        self.composition.is_some()
    }

    fn target_frame_duration_ms(&self) -> f64 {
        1000.0 / self.fps.max(1.0)
    }

    fn clamp_frame(&self, frame: u32) -> u32 {
        if self.duration_frames == 0 {
            0
        } else {
            frame.min(self.duration_frames - 1)
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewSnapshot {
    width: u32,
    height: u32,
    duration_frames: u32,
    fps: f64,
    current_frame: u32,
    playing: bool,
    composition_sync_ms: f64,
    last_render_ms: f64,
    avg_render_ms: f64,
    max_render_ms: f64,
    render_sample_count: u32,
    target_frame_duration_ms: f64,
    ready: bool,
}

#[wasm_bindgen]
pub struct LumenPreviewController {
    state: RefCell<PreviewState>,
    media: WasmMediaStore,
}

#[wasm_bindgen]
impl LumenPreviewController {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        ensure_webgl_backend();
        Self {
            state: RefCell::new(PreviewState::default()),
            media: WasmMediaStore::default(),
        }
    }

    #[wasm_bindgen(js_name = "loadComposition")]
    pub fn load_composition(&self, composition_json: &str, fps: f64) -> Result<(), JsValue> {
        let composition =
            composition_json_to_composition(composition_json).map_err(|e| JsValue::from_str(&e))?;

        let mut state = self
            .state
            .try_borrow_mut()
            .map_err(|_| JsValue::from_str("preview controller is busy"))?;
        state.width = composition.render_settings.width as usize;
        state.height = composition.render_settings.height as usize;
        state.duration_frames = composition.timeline.duration_frames;
        state.fps = fps.max(1.0);
        state.current_frame = 0;
        state.playing = false;
        state.dirty = true;
        state.last_tick_ms = None;
        state.frame_accumulator_ms = 0.0;
        state.render_metrics.reset();
        state.composition = Some(composition);
        state.composition_sync_ms = 0.0;
        Ok(())
    }

    pub fn clear(&self) {
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.clear();
        }
        let _ = self.media.clear();
    }

    #[wasm_bindgen(js_name = "setFps")]
    pub fn set_fps(&self, fps: f64) {
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.fps = fps.max(1.0);
            state.last_tick_ms = None;
        }
    }

    pub fn play(&self) {
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.playing = true;
            state.last_tick_ms = None;
        }
    }

    pub fn pause(&self) {
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.playing = false;
            state.last_tick_ms = None;
        }
    }

    #[wasm_bindgen(js_name = "togglePlay")]
    pub fn toggle_play(&self) -> bool {
        let Ok(mut state) = self.state.try_borrow_mut() else {
            return false;
        };
        state.playing = !state.playing;
        state.last_tick_ms = None;
        state.playing
    }

    #[wasm_bindgen(js_name = "isPlaying")]
    pub fn is_playing(&self) -> bool {
        self.state.borrow().playing
    }

    #[wasm_bindgen(js_name = "setFrame")]
    pub fn set_frame(&self, frame: u32) {
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.current_frame = state.clamp_frame(frame);
            state.dirty = state.ready();
            state.last_tick_ms = None;
            state.frame_accumulator_ms = 0.0;
        }
    }

    #[wasm_bindgen(js_name = "currentFrame")]
    pub fn current_frame(&self) -> u32 {
        self.state.borrow().current_frame
    }

    pub fn width(&self) -> u32 {
        self.state.borrow().width as u32
    }

    pub fn height(&self) -> u32 {
        self.state.borrow().height as u32
    }

    #[wasm_bindgen(js_name = "durationFrames")]
    pub fn duration_frames(&self) -> u32 {
        self.state.borrow().duration_frames
    }

    pub fn snapshot(&self) -> Result<String, JsValue> {
        let state = self
            .state
            .try_borrow()
            .map_err(|_| JsValue::from_str("preview controller is busy"))?;
        let snapshot = PreviewSnapshot {
            width: state.width as u32,
            height: state.height as u32,
            duration_frames: state.duration_frames.max(1),
            fps: state.fps,
            current_frame: state.current_frame,
            playing: state.playing,
            composition_sync_ms: state.composition_sync_ms,
            last_render_ms: state.render_metrics.last_ms,
            avg_render_ms: state.render_metrics.avg_ms,
            max_render_ms: state.render_metrics.max_ms,
            render_sample_count: state.render_metrics.sample_count,
            target_frame_duration_ms: state.target_frame_duration_ms(),
            ready: state.ready(),
        };

        serde_json::to_string(&snapshot).map_err(|e| JsValue::from_str(&e.to_string()))
    }

    pub fn tick(&self, now_ms: f64, context: CanvasRenderingContext2d) -> Result<bool, JsValue> {
        let Ok(mut state) = self.state.try_borrow_mut() else {
            return Ok(false);
        };
        if !state.ready() {
            state.last_tick_ms = Some(now_ms);
            drop(state);
            return Ok(false);
        }

        if state.playing {
            let last_tick_ms = state.last_tick_ms.replace(now_ms).unwrap_or(now_ms);
            let elapsed_ms = (now_ms - last_tick_ms).max(0.0);
            let target_frame_duration_ms = state.target_frame_duration_ms();

            if target_frame_duration_ms > 0.0 && state.duration_frames > 0 {
                state.frame_accumulator_ms += elapsed_ms;
                if state.frame_accumulator_ms >= target_frame_duration_ms {
                    let steps =
                        (state.frame_accumulator_ms / target_frame_duration_ms).floor() as u32;
                    let wrapped_steps = steps % state.duration_frames.max(1);
                    let next_accumulator =
                        state.frame_accumulator_ms - target_frame_duration_ms * f64::from(steps);
                    let next_frame =
                        (state.current_frame + wrapped_steps) % state.duration_frames.max(1);

                    if next_frame != state.current_frame {
                        if frame_ready(&state, &self.media, next_frame)? {
                            state.frame_accumulator_ms = next_accumulator;
                            state.current_frame = next_frame;
                            state.dirty = true;
                        } else {
                            // Backpressure playback to match media upload/decode speed.
                            state.last_tick_ms = Some(now_ms);
                            state.frame_accumulator_ms = 0.0;
                            drop(state);
                            return Ok(false);
                        }
                    } else {
                        state.frame_accumulator_ms = next_accumulator;
                    }
                }
            }
        } else {
            state.last_tick_ms = Some(now_ms);
        }

        if !state.dirty {
            drop(state);
            return Ok(false);
        }

        if !frame_ready(&state, &self.media, state.current_frame)? {
            drop(state);
            return Ok(false);
        }

        render_state(&mut state, &self.media, &context)?;
        drop(state);
        Ok(true)
    }

    #[wasm_bindgen(js_name = "renderNow")]
    pub fn render_now(&self, context: CanvasRenderingContext2d) -> Result<(), JsValue> {
        let mut state = self
            .state
            .try_borrow_mut()
            .map_err(|_| JsValue::from_str("preview controller is busy"))?;
        if !state.ready() {
            return Err(JsValue::from_str("composition not loaded"));
        }

        state.dirty = true;
        render_state(&mut state, &self.media, &context)?;
        drop(state);
        Ok(())
    }

    #[wasm_bindgen(js_name = "setCompositionSyncMs")]
    pub fn set_composition_sync_ms(&self, elapsed_ms: f64) {
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.composition_sync_ms = elapsed_ms.max(0.0);
        }
    }

    #[wasm_bindgen(js_name = "recordRenderTiming")]
    pub fn record_render_timing(&self, elapsed_ms: f64) {
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.render_metrics.update(elapsed_ms.max(0.0));
        }
    }

    #[wasm_bindgen(js_name = "frameRequirements")]
    pub fn frame_requirements(&self, frame: u32) -> Result<String, JsValue> {
        let state = self
            .state
            .try_borrow()
            .map_err(|_| JsValue::from_str("preview controller is busy"))?;
        validate_frame(&state, frame)?;
        let composition = state
            .composition
            .as_ref()
            .ok_or_else(|| JsValue::from_str("composition not loaded"))?;

        let payload = collect_frame_requirements(composition, &self.media, frame)
            .map(FrameRequirementsPayload::from)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        serde_json::to_string(&payload).map_err(|e| JsValue::from_str(&e.to_string()))
    }

    #[wasm_bindgen(js_name = "clearMedia")]
    pub fn clear_media(&self) -> Result<(), JsValue> {
        self.media.clear().map_err(JsValue::from_str)?;
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.dirty = true;
        }
        Ok(())
    }

    #[wasm_bindgen(js_name = "clearVideos")]
    pub fn clear_videos(&self) -> Result<(), JsValue> {
        self.media.clear_video_frames().map_err(JsValue::from_str)?;
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.dirty = true;
        }
        Ok(())
    }

    #[wasm_bindgen(js_name = "clearVideoSource")]
    pub fn clear_video_source(&self, stream_id: &str) -> Result<(), JsValue> {
        self.media
            .clear_video_frames_for_stream(stream_id)
            .map_err(JsValue::from_str)?;
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.dirty = true;
        }
        Ok(())
    }

    #[wasm_bindgen(js_name = "removeImageSource")]
    pub fn remove_image_source(&self, image_id: &str) -> Result<(), JsValue> {
        self.media.remove_image(image_id).map_err(JsValue::from_str)?;
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.dirty = true;
        }
        Ok(())
    }

    #[wasm_bindgen(js_name = "removeVideoSource")]
    pub fn remove_video_source(&self, stream_id: &str) -> Result<(), JsValue> {
        self.media.remove_video(stream_id).map_err(JsValue::from_str)?;
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.dirty = true;
        }
        Ok(())
    }

    #[wasm_bindgen(js_name = "hasImage")]
    pub fn has_image(&self, image_id: &str) -> bool {
        self.media.has_image(image_id)
    }

    #[wasm_bindgen(js_name = "hasVideoFrame")]
    pub fn has_video_frame(&self, stream_id: &str, frame: u32) -> bool {
        self.media.has_video_frame(stream_id, frame)
    }

    #[wasm_bindgen(js_name = "setImage")]
    pub fn set_image(
        &self,
        image_id: &str,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<(), JsValue> {
        if width == 0 || height == 0 {
            return Err(JsValue::from_str("image dimensions must be > 0"));
        }
        if !validate_rgba_len(width, height, rgba.len()) {
            return Err(JsValue::from_str("invalid image rgba buffer length"));
        }

        let frame = image_frame_from_rgba(width, height, rgba.to_vec())
            .map_err(|e| JsValue::from_str(&e))?;
        self.media
            .set_image(image_id.to_string(), frame)
            .map_err(JsValue::from_str)?;
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.dirty = true;
        }
        Ok(())
    }

    #[wasm_bindgen(js_name = "setVideoFrame")]
    pub fn set_video_frame(
        &self,
        stream_id: &str,
        frame: u32,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<(), JsValue> {
        if width == 0 || height == 0 {
            return Err(JsValue::from_str("video frame dimensions must be > 0"));
        }
        if !validate_rgba_len(width, height, rgba.len()) {
            return Err(JsValue::from_str("invalid video rgba buffer length"));
        }

        let image = image_frame_from_rgba(width, height, rgba.to_vec())
            .map_err(|e| JsValue::from_str(&e))?;
        self.media
            .set_video_frame(stream_id.to_string(), frame, image)
            .map_err(JsValue::from_str)?;
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.dirty = true;
        }
        Ok(())
    }

    #[wasm_bindgen(js_name = "setVideoFrameObject")]
    pub fn set_video_frame_object(
        &self,
        stream_id: &str,
        frame: u32,
        video_frame: &web_sys::VideoFrame,
        width: u32,
        height: u32,
    ) -> Result<(), JsValue> {
        if width == 0 || height == 0 {
            return Err(JsValue::from_str("video frame dimensions must be > 0"));
        }

        let image = image_frame_from_video_frame(video_frame, width, height)?;
        self.media
            .set_video_frame(stream_id.to_string(), frame, image)
            .map_err(JsValue::from_str)?;
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.dirty = true;
        }
        Ok(())
    }

    #[wasm_bindgen(js_name = "setVideoMetadata")]
    pub fn set_video_metadata(
        &self,
        stream_id: &str,
        width: u32,
        height: u32,
        frame_count: u32,
    ) -> Result<(), JsValue> {
        if width == 0 || height == 0 {
            return Err(JsValue::from_str("video dimensions must be > 0"));
        }

        self.media
            .set_video_metadata(
                stream_id.to_string(),
                VideoMetadata {
                    width,
                    height,
                    frame_count,
                },
            )
            .map_err(JsValue::from_str)?;
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.dirty = true;
        }
        Ok(())
    }
}

fn validate_frame(state: &PreviewState, frame: u32) -> Result<(), JsValue> {
    if state.duration_frames == 0 {
        return Ok(());
    }
    if frame >= state.duration_frames {
        return Err(JsValue::from_str("frame index out of range"));
    }
    Ok(())
}

fn frame_ready(
    state: &PreviewState,
    media: &WasmMediaStore,
    frame: u32,
) -> Result<bool, JsValue> {
    let composition = state
        .composition
        .as_ref()
        .ok_or_else(|| JsValue::from_str("composition not loaded"))?;
    let requirements = collect_frame_requirements(composition, media, frame)
        .map_err(|error| JsValue::from_str(&error.to_string()))?;

    let images_ready = requirements.images.iter().all(|image_id| media.has_image(image_id));
    let videos_ready = requirements.videos.iter().all(|video| {
        video.frames
            .iter()
            .all(|required_frame| media.has_video_frame(&video.stream_id, *required_frame))
    });

    Ok(images_ready && videos_ready)
}

fn render_state(
    state: &mut PreviewState,
    media: &WasmMediaStore,
    context: &CanvasRenderingContext2d,
) -> Result<(), JsValue> {
    let composition = state
        .composition
        .as_ref()
        .ok_or_else(|| JsValue::from_str("composition not loaded"))?;

    let mut core = CoreRenderer::new(composition, &state.surface_pool, media)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let raster = core
        .render(state.current_frame)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    state.surface_pool.flush();
    let (w, h) = raster.storage_dimensions();
    draw_output_frame_to_context(&raster, context)?;

    state.width = w as usize;
    state.height = h as usize;
    state.dirty = false;
    Ok(())
}
