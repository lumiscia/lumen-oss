use std::cell::{Ref, RefCell, RefMut};

use lumen_engine::{
    composition::Composition,
    media::{VideoMetadata, collect_frame_requirements},
};
use serde::Serialize;
use wasm_bindgen::prelude::*;

use crate::{
    install_panic_hook,
    media::WasmMediaStore,
    renderer::{SurfaceCompositionRenderer, collect_requirement_window},
    types::FrameRequirementsPayload,
    utils::{composition_json_to_composition, image_frame_from_rgba, validate_rgba_len},
};

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

/// The GPU device is created once per controller and kept across composition and media changes.
enum GpuSlot {
    Empty,
    Pending,
    Ready(Box<SurfaceCompositionRenderer>),
}

struct PreviewState {
    composition: Option<Composition>,
    gpu: GpuSlot,
    gpu_generation: u64,
    render_canvas: Option<JsValue>,
    width: usize,
    height: usize,
    duration_frames: u32,
    fps: f64,
    current_frame: u32,
    lookahead_count: u32,
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
            gpu: GpuSlot::Empty,
            gpu_generation: 0,
            render_canvas: None,
            width: 0,
            height: 0,
            duration_frames: 0,
            fps: 30.0,
            current_frame: 0,
            lookahead_count: 8,
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
    /// Unloads everything and destroys the GPU device; an in-flight device request is discarded.
    fn clear(&mut self) {
        self.composition = None;
        self.gpu = GpuSlot::Empty;
        self.gpu_generation = self.gpu_generation.wrapping_add(1);
        self.render_canvas = None;
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

    /// Replaces the loaded composition and parks the playhead on frame 0.
    fn install_composition(&mut self, composition: Composition, media: &WasmMediaStore) {
        self.width = composition.render_settings.width as usize;
        self.height = composition.render_settings.height as usize;
        self.duration_frames = composition.timeline.duration_frames;
        self.fps = f64::from(composition.timeline.fps.max(1.0));
        self.composition = Some(composition);
        self.playing = false;
        self.render_metrics.reset();
        self.composition_sync_ms = 0.0;
        self.invalidate_renderer();
        self.move_playhead(0, media);
    }

    /// Jumps the playhead to `frame` (clamped) and pins that frame's media needs so the frames
    /// pinned for the previous position become evictable.
    fn move_playhead(&mut self, frame: u32, media: &WasmMediaStore) {
        self.current_frame = self.clamp_frame(frame);
        self.last_tick_ms = None;
        self.frame_accumulator_ms = 0.0;
        pin_frame(self, media, self.current_frame);
    }

    fn ready(&self) -> bool {
        self.composition.is_some()
    }

    fn invalidate_renderer(&mut self) {
        if let GpuSlot::Ready(renderer) = &mut self.gpu {
            renderer.invalidate();
        }
        self.dirty = true;
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
        install_panic_hook();
        Self {
            state: RefCell::new(PreviewState::default()),
            media: WasmMediaStore::default(),
        }
    }

    #[wasm_bindgen(js_name = "loadComposition")]
    pub fn load_composition(&self, composition_json: &str) -> Result<(), JsValue> {
        let composition =
            composition_json_to_composition(composition_json).map_err(|e| JsValue::from_str(&e))?;

        let mut state = self.state_mut()?;
        state.install_composition(composition, &self.media);
        tracing::debug!(
            target: "lumen_wasm",
            width = state.width,
            height = state.height,
            duration_frames = state.duration_frames,
            fps = state.fps,
            "preview loaded composition"
        );
        Ok(())
    }

    #[wasm_bindgen(js_name = "setLookaheadCount")]
    pub fn set_lookahead_count(&self, lookahead_count: u32) -> Result<(), JsValue> {
        self.state_mut()?.lookahead_count = lookahead_count;
        Ok(())
    }

    #[wasm_bindgen(js_name = "setLogLevel")]
    pub fn set_log_level(&self, level: &str) -> Result<(), JsValue> {
        lumen_engine::set_log_level_from_str(level).map_err(|error| JsValue::from_str(&error))?;
        tracing::debug!(target: "lumen_wasm", level, "set log level");
        Ok(())
    }

    /// Unloads the composition and media and destroys the GPU device.
    pub fn clear(&self) -> Result<(), JsValue> {
        self.state_mut()?.clear();
        self.media.clear().map_err(JsValue::from_str)
    }

    pub fn play(&self) -> Result<(), JsValue> {
        let mut state = self.state_mut()?;
        state.playing = true;
        state.last_tick_ms = None;
        Ok(())
    }

    pub fn pause(&self) -> Result<(), JsValue> {
        let mut state = self.state_mut()?;
        state.playing = false;
        state.last_tick_ms = None;
        Ok(())
    }

    #[wasm_bindgen(js_name = "togglePlay")]
    pub fn toggle_play(&self) -> Result<bool, JsValue> {
        let mut state = self.state_mut()?;
        state.playing = !state.playing;
        state.last_tick_ms = None;
        Ok(state.playing)
    }

    #[wasm_bindgen(js_name = "isPlaying")]
    pub fn is_playing(&self) -> Result<bool, JsValue> {
        Ok(self.state()?.playing)
    }

    #[wasm_bindgen(js_name = "setFrame")]
    pub fn set_frame(&self, frame: u32) -> Result<(), JsValue> {
        let mut state = self.state_mut()?;
        state.move_playhead(frame, &self.media);
        state.dirty = state.ready();
        Ok(())
    }

    #[wasm_bindgen(js_name = "currentFrame")]
    pub fn current_frame(&self) -> Result<u32, JsValue> {
        Ok(self.state()?.current_frame)
    }

    #[wasm_bindgen(js_name = "targetFrameForTimeMs")]
    pub fn target_frame_for_time_ms(&self, time_ms: f64) -> Result<u32, JsValue> {
        let state = self.state()?;
        if state.duration_frames == 0 {
            return Ok(0);
        }
        let frame = ((time_ms.max(0.0) / 1_000.0) * state.fps.max(1.0)).floor() as u32;
        Ok(frame.min(state.duration_frames - 1))
    }

    pub fn width(&self) -> Result<u32, JsValue> {
        Ok(self.state()?.width as u32)
    }

    pub fn height(&self) -> Result<u32, JsValue> {
        Ok(self.state()?.height as u32)
    }

    #[wasm_bindgen(js_name = "durationFrames")]
    pub fn duration_frames(&self) -> Result<u32, JsValue> {
        Ok(self.state()?.duration_frames)
    }

    pub fn fps(&self) -> Result<f64, JsValue> {
        Ok(self.state()?.fps)
    }

    #[wasm_bindgen(js_name = "targetFrameDurationMs")]
    pub fn target_frame_duration_ms(&self) -> Result<f64, JsValue> {
        Ok(self.state()?.target_frame_duration_ms())
    }

    pub fn snapshot(&self) -> Result<String, JsValue> {
        let state = self.state()?;
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

    /// Advances playback and renders when the current frame changed. Returns whether a frame was
    /// presented; it is `false` while another call is still creating the GPU device.
    pub async fn tick(&self, now_ms: f64, canvas: JsValue) -> Result<bool, JsValue> {
        {
            let mut state = self.state_mut()?;
            if !state.ready() {
                state.last_tick_ms = Some(now_ms);
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
                        let next_accumulator = state.frame_accumulator_ms
                            - target_frame_duration_ms * f64::from(steps);
                        let next_frame =
                            (state.current_frame + wrapped_steps) % state.duration_frames.max(1);

                        if next_frame != state.current_frame {
                            // Move the pins first so the previous frame's working set can be
                            // evicted to make room for the next one.
                            pin_frame(&state, &self.media, next_frame);
                            if frame_ready(&state, &self.media, next_frame)? {
                                state.frame_accumulator_ms = next_accumulator;
                                state.current_frame = next_frame;
                                state.dirty = true;
                            } else {
                                state.last_tick_ms = Some(now_ms);
                                state.frame_accumulator_ms = 0.0;
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

            if !state.dirty || !frame_ready(&state, &self.media, state.current_frame)? {
                return Ok(false);
            }
        }

        self.render_current_frame(&canvas).await
    }

    #[wasm_bindgen(js_name = "renderNow")]
    pub async fn render_now(&self, canvas: JsValue) -> Result<(), JsValue> {
        {
            let mut state = self.state_mut()?;
            if !state.ready() {
                return Err(JsValue::from_str("composition not loaded"));
            }
            state.dirty = true;
        }
        self.render_current_frame(&canvas).await.map(|_| ())
    }

    #[wasm_bindgen(js_name = "setCompositionSyncMs")]
    pub fn set_composition_sync_ms(&self, elapsed_ms: f64) -> Result<(), JsValue> {
        self.state_mut()?.composition_sync_ms = elapsed_ms.max(0.0);
        Ok(())
    }

    #[wasm_bindgen(js_name = "recordRenderTiming")]
    pub fn record_render_timing(&self, elapsed_ms: f64) -> Result<(), JsValue> {
        self.state_mut()?.render_metrics.update(elapsed_ms.max(0.0));
        Ok(())
    }

    #[wasm_bindgen(js_name = "frameRequirements")]
    pub fn frame_requirements(&self, frame: u32) -> Result<String, JsValue> {
        let state = self.state()?;
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

    #[wasm_bindgen(js_name = "frameRequirementsWindow")]
    pub fn frame_requirements_window(&self, frame: u32) -> Result<String, JsValue> {
        let state = self.state()?;
        validate_frame(&state, frame)?;
        let composition = state
            .composition
            .as_ref()
            .ok_or_else(|| JsValue::from_str("composition not loaded"))?;
        let payload =
            collect_requirement_window(composition, &self.media, frame, state.lookahead_count)?;
        serde_json::to_string(&payload).map_err(|e| JsValue::from_str(&e.to_string()))
    }

    #[wasm_bindgen(js_name = "clearMedia")]
    pub fn clear_media(&self) -> Result<(), JsValue> {
        self.media.clear().map_err(JsValue::from_str)?;
        self.reset_renderer()
    }

    #[wasm_bindgen(js_name = "clearVideos")]
    pub fn clear_videos(&self) -> Result<(), JsValue> {
        self.media.clear_video_frames().map_err(JsValue::from_str)?;
        self.mark_dirty()
    }

    #[wasm_bindgen(js_name = "clearVideoSource")]
    pub fn clear_video_source(&self, stream_id: &str) -> Result<(), JsValue> {
        self.media
            .clear_video_frames_for_stream(stream_id)
            .map_err(JsValue::from_str)?;
        self.mark_dirty()
    }

    #[wasm_bindgen(js_name = "removeImageSource")]
    pub fn remove_image_source(&self, image_id: &str) -> Result<(), JsValue> {
        self.media
            .remove_image(image_id)
            .map_err(JsValue::from_str)?;
        self.reset_renderer()
    }

    #[wasm_bindgen(js_name = "removeVideoSource")]
    pub fn remove_video_source(&self, stream_id: &str) -> Result<(), JsValue> {
        self.media
            .remove_video(stream_id)
            .map_err(JsValue::from_str)?;
        self.reset_renderer()
    }

    #[wasm_bindgen(js_name = "removeFontFamily")]
    pub fn remove_font_family(&self, font_family: &str) -> Result<(), JsValue> {
        self.media
            .remove_font(font_family)
            .map_err(JsValue::from_str)?;
        self.reset_renderer()
    }

    #[wasm_bindgen(js_name = "hasImage")]
    pub fn has_image(&self, image_id: &str) -> bool {
        self.media.has_image(image_id)
    }

    #[wasm_bindgen(js_name = "hasVideoFrame")]
    pub fn has_video_frame(&self, stream_id: &str, frame: u32) -> bool {
        self.media.has_video_frame(stream_id, frame)
    }

    #[wasm_bindgen(js_name = "hasFont")]
    pub fn has_font(&self, font_family: &str) -> bool {
        self.media.has_font(font_family)
    }

    /// Sets the byte budget shared by all cached video frames; frames needed at the playhead are
    /// kept even when they exceed it.
    #[wasm_bindgen(js_name = "setVideoFrameCacheBytes")]
    pub fn set_video_frame_cache_bytes(&self, bytes: u32) -> Result<(), JsValue> {
        self.media
            .set_video_frame_cache_bytes(bytes as usize)
            .map_err(JsValue::from_str)
    }

    #[wasm_bindgen(js_name = "setImage")]
    pub fn set_image(
        &self,
        image_id: &str,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> Result<(), JsValue> {
        if width == 0 || height == 0 {
            return Err(JsValue::from_str("image dimensions must be > 0"));
        }
        if !validate_rgba_len(width, height, rgba.len()) {
            return Err(JsValue::from_str("invalid image rgba buffer length"));
        }
        let frame =
            image_frame_from_rgba(width, height, rgba).map_err(|e| JsValue::from_str(&e))?;
        self.media
            .set_image(image_id.to_string(), frame)
            .map_err(JsValue::from_str)?;
        self.reset_renderer()
    }

    #[wasm_bindgen(js_name = "setFont")]
    pub fn set_font(&self, font_family: &str, data: Vec<u8>) -> Result<(), JsValue> {
        self.media
            .set_font(font_family.to_string(), data)
            .map_err(JsValue::from_str)?;
        self.reset_renderer()
    }

    #[wasm_bindgen(js_name = "setVideoFrame")]
    pub fn set_video_frame(
        &self,
        stream_id: &str,
        frame: u32,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> Result<(), JsValue> {
        if width == 0 || height == 0 {
            return Err(JsValue::from_str("video frame dimensions must be > 0"));
        }
        if !validate_rgba_len(width, height, rgba.len()) {
            return Err(JsValue::from_str("invalid video rgba buffer length"));
        }
        let image =
            image_frame_from_rgba(width, height, rgba).map_err(|e| JsValue::from_str(&e))?;
        self.media
            .set_video_frame(stream_id.to_string(), frame, image)
            .map_err(JsValue::from_str)?;
        self.mark_dirty()
    }

    #[wasm_bindgen(js_name = "setVideoMetadata")]
    pub fn set_video_metadata(
        &self,
        stream_id: &str,
        width: u32,
        height: u32,
        frame_count: u32,
        fps: f32,
    ) -> Result<(), JsValue> {
        if width == 0 || height == 0 {
            return Err(JsValue::from_str("video dimensions must be > 0"));
        }
        if !fps.is_finite() || fps <= 0.0 {
            return Err(JsValue::from_str("video fps must be a finite value > 0"));
        }
        self.media
            .set_video_metadata(
                stream_id.to_string(),
                VideoMetadata {
                    width,
                    height,
                    frame_count,
                    fps,
                },
            )
            .map_err(JsValue::from_str)?;
        self.reset_renderer()
    }
}

// State borrows are never held across an `.await`, so commands always apply, even while a
// render is creating the GPU device.
impl LumenPreviewController {
    fn state(&self) -> Result<Ref<'_, PreviewState>, JsValue> {
        self.state
            .try_borrow()
            .map_err(|_| JsValue::from_str("preview controller is busy"))
    }

    fn state_mut(&self) -> Result<RefMut<'_, PreviewState>, JsValue> {
        self.state
            .try_borrow_mut()
            .map_err(|_| JsValue::from_str("preview controller is busy"))
    }

    fn mark_dirty(&self) -> Result<(), JsValue> {
        self.state_mut()?.dirty = true;
        Ok(())
    }

    fn reset_renderer(&self) -> Result<(), JsValue> {
        self.state_mut()?.invalidate_renderer();
        Ok(())
    }

    /// Renders the latest state to the most recently requested canvas. The GPU device is
    /// requested without holding the state borrow; if `clear` ran meanwhile the new device is
    /// destroyed and nothing is rendered.
    async fn render_current_frame(&self, canvas: &JsValue) -> Result<bool, JsValue> {
        let pending_generation = {
            let mut state = self.state_mut()?;
            state.render_canvas = Some(canvas.clone());
            match state.gpu {
                GpuSlot::Ready(_) => None,
                GpuSlot::Pending => return Ok(false),
                GpuSlot::Empty => {
                    state.gpu = GpuSlot::Pending;
                    Some(state.gpu_generation)
                }
            }
        };
        if let Some(generation) = pending_generation {
            tracing::debug!(target: "lumen_wasm", "create preview GPU device");
            let created = SurfaceCompositionRenderer::new(canvas).await;
            let mut state = self.state_mut()?;
            if state.gpu_generation != generation {
                return Ok(false);
            }
            state.gpu = match created {
                Ok(renderer) => GpuSlot::Ready(Box::new(renderer)),
                Err(error) => {
                    state.gpu = GpuSlot::Empty;
                    return Err(error);
                }
            };
        }

        let mut guard = self.state_mut()?;
        let frame = guard.current_frame;
        pin_frame(&guard, &self.media, frame);
        let canvas = guard
            .render_canvas
            .clone()
            .unwrap_or_else(|| canvas.clone());
        let state = &mut *guard;
        let GpuSlot::Ready(renderer) = &mut state.gpu else {
            return Ok(false);
        };
        let composition = state
            .composition
            .as_ref()
            .ok_or_else(|| JsValue::from_str("composition not loaded"))?;
        tracing::trace!(target: "lumen_wasm", frame, "render preview frame");
        let size = renderer.render_frame(
            &canvas,
            composition,
            frame,
            state.lookahead_count,
            &self.media,
        )?;
        state.width = size.width as usize;
        state.height = size.height as usize;
        state.dirty = false;
        tracing::trace!(
            target: "lumen_wasm",
            frame,
            width = state.width,
            height = state.height,
            "rendered preview frame"
        );
        Ok(true)
    }
}

impl Default for LumenPreviewController {
    fn default() -> Self {
        Self::new()
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

/// Keeps the video frames needed at `frame` out of cache eviction.
fn pin_frame(state: &PreviewState, media: &WasmMediaStore, frame: u32) {
    let Some(composition) = state.composition.as_ref() else {
        return;
    };
    if let Ok(requirements) = collect_frame_requirements(composition, media, frame) {
        let _ = media.pin_video_frames(&requirements.videos);
    }
}

fn frame_ready(state: &PreviewState, media: &WasmMediaStore, frame: u32) -> Result<bool, JsValue> {
    let composition = state
        .composition
        .as_ref()
        .ok_or_else(|| JsValue::from_str("composition not loaded"))?;
    let requirements = collect_frame_requirements(composition, media, frame)
        .map_err(|error| JsValue::from_str(&error.to_string()))?;

    let images_ready = requirements
        .images
        .iter()
        .all(|image_id| media.has_image(image_id));
    let videos_ready = requirements.videos.iter().all(|video| {
        video
            .frames
            .iter()
            .all(|required_frame| media.has_video_frame(&video.stream_id, *required_frame))
    });

    let ready = images_ready && videos_ready;
    tracing::trace!(
        target: "lumen_wasm",
        frame,
        ready,
        images = requirements.images.len(),
        videos = requirements.videos.len(),
        "checked frame readiness"
    );
    Ok(ready)
}

#[cfg(test)]
mod tests {
    use lumen_engine::media::VideoMetadata;

    use super::*;

    const COMPOSITION_JSON: &str = r#"{
        "timeline": { "fps": 30, "duration_frames": 200 },
        "render_settings": { "width": 8, "height": 4 },
        "nodes": [
            { "id": 1, "type": "media_in", "params": { "kind": "video", "source": "a" } },
            { "id": 2, "type": "media_in", "params": { "kind": "video", "source": "b" } },
            { "id": 3, "type": "merge", "params": {} },
            { "id": 4, "type": "media_output" }
        ],
        "connections": [
            { "from_node": 1, "from_port": "output", "to_node": 3, "to_port": "base" },
            { "from_node": 2, "from_port": "output", "to_node": 3, "to_port": "overlay" },
            { "from_node": 3, "from_port": "output", "to_node": 4, "to_port": "source" }
        ]
    }"#;

    fn upload(media: &WasmMediaStore, frame: u32) {
        for stream_id in ["a", "b"] {
            let image = image_frame_from_rgba(1, 1, vec![255, 0, 0, 255]).expect("frame");
            media
                .set_video_frame(stream_id.to_string(), frame, image)
                .expect("set frame");
        }
    }

    #[test]
    fn reloading_with_an_occupied_cache_pins_frame_zero() {
        let media = WasmMediaStore::default();
        for stream_id in ["a", "b"] {
            media
                .set_video_metadata(
                    stream_id.to_string(),
                    VideoMetadata {
                        width: 1,
                        height: 1,
                        frame_count: 200,
                        fps: 30.0,
                    },
                )
                .expect("set metadata");
        }
        // Room for two frames: one per stream.
        media.set_video_frame_cache_bytes(8).expect("set budget");

        let load = || composition_json_to_composition(COMPOSITION_JSON).expect("composition");
        let mut state = PreviewState::default();
        state.install_composition(load(), &media);
        state.move_playhead(100, &media);
        upload(&media, 100);

        state.install_composition(load(), &media);
        assert_eq!(state.current_frame, 0);
        upload(&media, 0);

        assert!(frame_ready(&state, &media, 0).expect("frame readiness"));
        assert!(!media.has_video_frame("a", 100));
        assert!(!media.has_video_frame("b", 100));
    }
}
