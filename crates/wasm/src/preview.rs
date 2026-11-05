use std::cell::{Ref, RefCell, RefMut};

use lumen_engine::{
    composition::Composition,
    media::{
        VideoMetadata, YuvLayout, YuvMatrix, YuvPlane, collect_frame_requirements, yuv_to_rgba,
    },
};
use wasm_bindgen::prelude::*;

use crate::{
    install_panic_hook,
    media::WasmMediaStore,
    renderer::SurfaceCompositionRenderer,
    types::{AudioTimelinePayload, FrameRequirementsPayload},
    utils::{composition_json_to_composition, image_frame_from_rgba, validate_rgba_len},
};

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
    }

    /// Replaces the loaded composition and parks the playhead on frame 0.
    fn install_composition(&mut self, composition: Composition) {
        self.width = composition.render_settings().width as usize;
        self.height = composition.render_settings().height as usize;
        self.duration_frames = composition.timeline().duration_frames;
        self.fps = f64::from(composition.timeline().fps.max(1.0));
        self.composition = Some(composition);
        self.invalidate_renderer();
        self.current_frame = 0;
    }

    /// Replaces the media store's protected frames with what the playhead needs. Requirements that
    /// cannot be resolved yet (e.g. unregistered streams) protect nothing.
    fn refresh_pins(&self, media: &WasmMediaStore) -> Result<(), JsValue> {
        let protected = self
            .composition
            .as_ref()
            .and_then(|composition| {
                collect_frame_requirements(composition, media, self.current_frame).ok()
            })
            .map_or_else(Vec::new, |requirements| requirements.videos);
        media
            .protect_video_frames(&protected)
            .map_err(JsValue::from_str)
    }

    fn ready(&self) -> bool {
        self.composition.is_some()
    }

    fn invalidate_renderer(&mut self) {
        if let GpuSlot::Ready(renderer) = &mut self.gpu {
            renderer.invalidate();
        }
    }

    fn clamp_frame(&self, frame: u32) -> u32 {
        if self.duration_frames == 0 {
            0
        } else {
            frame.min(self.duration_frames - 1)
        }
    }
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

        self.update_state(|state| state.install_composition(composition))?;
        let state = self.state()?;
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

    /// The audio timeline the engine parsed from the loaded composition, as JSON.
    #[wasm_bindgen(js_name = "audioTimeline")]
    pub fn audio_timeline(&self) -> Result<Option<String>, JsValue> {
        let state = self.state()?;
        let Some(timeline) = state.composition.as_ref().and_then(Composition::audio) else {
            return Ok(None);
        };
        let payload = AudioTimelinePayload {
            timeline,
            duration_frames: state.duration_frames,
            fps: state.fps,
        };
        serde_json::to_string(&payload)
            .map(Some)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    #[wasm_bindgen(js_name = "setLogLevel")]
    pub fn set_log_level(&self, level: &str) -> Result<(), JsValue> {
        crate::set_log_level(level).map_err(|error| JsValue::from_str(&error))?;
        tracing::debug!(target: "lumen_wasm", level, "set log level");
        Ok(())
    }

    /// Unloads the composition and media and destroys the GPU device.
    pub fn clear(&self) -> Result<(), JsValue> {
        self.update_state(PreviewState::clear)?;
        self.change_media(WasmMediaStore::clear)
    }

    #[wasm_bindgen(js_name = "setFrame")]
    pub fn set_frame(&self, frame: u32) -> Result<(), JsValue> {
        self.update_state(|state| state.current_frame = state.clamp_frame(frame))
    }

    #[wasm_bindgen(js_name = "currentFrame")]
    pub fn current_frame(&self) -> Result<u32, JsValue> {
        Ok(self.state()?.current_frame)
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

    #[wasm_bindgen(js_name = "renderNow")]
    pub async fn render_now(&self, canvas: JsValue) -> Result<(), JsValue> {
        if !self.state()?.ready() {
            return Err(JsValue::from_str("composition not loaded"));
        }
        self.render_current_frame(&canvas).await
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

    #[wasm_bindgen(js_name = "clearMedia")]
    pub fn clear_media(&self) -> Result<(), JsValue> {
        self.change_media(WasmMediaStore::clear)?;
        self.reset_renderer()
    }

    #[wasm_bindgen(js_name = "clearVideos")]
    pub fn clear_videos(&self) -> Result<(), JsValue> {
        self.change_media(WasmMediaStore::clear_video_frames)
    }

    #[wasm_bindgen(js_name = "clearVideoSource")]
    pub fn clear_video_source(&self, stream_id: &str) -> Result<(), JsValue> {
        self.change_media(|media| media.clear_video_frames_for_stream(stream_id))
    }

    #[wasm_bindgen(js_name = "removeImageSource")]
    pub fn remove_image_source(&self, image_id: &str) -> Result<(), JsValue> {
        self.change_media(|media| media.remove_image(image_id))?;
        self.reset_renderer()
    }

    #[wasm_bindgen(js_name = "removeVideoSource")]
    pub fn remove_video_source(&self, stream_id: &str) -> Result<(), JsValue> {
        self.change_media(|media| media.remove_video(stream_id))?;
        self.reset_renderer()
    }

    #[wasm_bindgen(js_name = "removeFontFamily")]
    pub fn remove_font_family(&self, font_family: &str) -> Result<(), JsValue> {
        self.change_media(|media| media.remove_font(font_family))?;
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
        self.change_media(|media| media.set_video_frame_cache_bytes(bytes as usize))
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
        self.change_media(|media| media.set_image(image_id.to_string(), frame))?;
        self.reset_renderer()
    }

    #[wasm_bindgen(js_name = "setFont")]
    pub fn set_font(&self, font_family: &str, data: Vec<u8>) -> Result<(), JsValue> {
        self.change_media(|media| media.set_font(font_family.to_string(), data))?;
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
        self.change_media(|media| media.set_video_frame(stream_id.to_string(), frame, image))
    }

    /// Stores a frame copied from a `VideoFrame` in its native `NV12`, `I420` or `I420A` format, with
    /// the plane offsets and strides `copyTo` returned. `matrix` is the source's tagged WebCodecs
    /// matrix; without one, the engine's rule for untagged video picks it from the height.
    /// `full_range` describes the copied samples.
    #[wasm_bindgen(js_name = "setVideoFrameYuv")]
    #[allow(clippy::too_many_arguments)]
    pub fn set_video_frame_yuv(
        &self,
        stream_id: &str,
        frame: u32,
        width: u32,
        height: u32,
        format: &str,
        data: Vec<u8>,
        offsets: Vec<u32>,
        strides: Vec<u32>,
        matrix: Option<String>,
        full_range: bool,
    ) -> Result<(), JsValue> {
        if width == 0 || height == 0 {
            return Err(JsValue::from_str("video frame dimensions must be > 0"));
        }
        let layout = yuv_layout(format, &offsets, &strides).ok_or_else(|| {
            JsValue::from_str(&format!("unsupported video frame layout {format}"))
        })?;
        let matrix = match matrix.as_deref() {
            Some("bt709") => YuvMatrix::Bt709,
            Some("bt470bg" | "smpte170m") => YuvMatrix::Bt601,
            Some("bt2020-ncl") => YuvMatrix::Bt2020,
            _ => YuvMatrix::untagged(height),
        };
        let rgba = yuv_to_rgba(&data, width, height, layout, matrix, full_range)
            .map_err(|e| JsValue::from_str(&e))?;
        let image =
            image_frame_from_rgba(width, height, rgba).map_err(|e| JsValue::from_str(&e))?;
        self.change_media(|media| media.set_video_frame(stream_id.to_string(), frame, image))
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
        let metadata = VideoMetadata {
            width,
            height,
            frame_count,
            fps,
        };
        self.change_media(|media| media.set_video_metadata(stream_id.to_string(), metadata))?;
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

    /// Applies a playback state change, then re-derives the protected frames from it.
    fn update_state<T>(&self, update: impl FnOnce(&mut PreviewState) -> T) -> Result<T, JsValue> {
        let mut state = self.state_mut()?;
        let result = update(&mut state);
        state.refresh_pins(&self.media)?;
        Ok(result)
    }

    /// Applies a media change with protected frames derived from the playback state both before
    /// (so uploads evict against the current playhead) and after (so registration, removal and
    /// timing changes take effect).
    fn change_media(
        &self,
        change: impl FnOnce(&WasmMediaStore) -> Result<(), &'static str>,
    ) -> Result<(), JsValue> {
        let state = self.state()?;
        state.refresh_pins(&self.media)?;
        change(&self.media).map_err(JsValue::from_str)?;
        state.refresh_pins(&self.media)
    }

    fn reset_renderer(&self) -> Result<(), JsValue> {
        self.state_mut()?.invalidate_renderer();
        Ok(())
    }

    /// Renders the latest state to the most recently requested canvas. The GPU device is
    /// requested without holding the state borrow; if `clear` ran meanwhile the new device is
    /// destroyed and nothing is rendered. Nothing is rendered either while another call is still
    /// creating the device.
    async fn render_current_frame(&self, canvas: &JsValue) -> Result<(), JsValue> {
        let pending_generation = {
            let mut state = self.state_mut()?;
            state.render_canvas = Some(canvas.clone());
            match state.gpu {
                GpuSlot::Ready(_) => None,
                GpuSlot::Pending => return Ok(()),
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
                return Ok(());
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
        let canvas = guard
            .render_canvas
            .clone()
            .unwrap_or_else(|| canvas.clone());
        let state = &mut *guard;
        let GpuSlot::Ready(renderer) = &mut state.gpu else {
            return Ok(());
        };
        let composition = state
            .composition
            .as_ref()
            .ok_or_else(|| JsValue::from_str("composition not loaded"))?;
        tracing::trace!(target: "lumen_wasm", frame, "render preview frame");
        let size = renderer.render_frame(&canvas, composition, frame, &self.media)?;
        state.width = size.width as usize;
        state.height = size.height as usize;
        tracing::trace!(
            target: "lumen_wasm",
            frame,
            width = state.width,
            height = state.height,
            "rendered preview frame"
        );
        Ok(())
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

fn yuv_layout(format: &str, offsets: &[u32], strides: &[u32]) -> Option<YuvLayout> {
    let plane = |index: usize| {
        Some(YuvPlane {
            offset: *offsets.get(index)? as usize,
            stride: *strides.get(index)? as usize,
        })
    };
    match (format, offsets.len().min(strides.len())) {
        ("NV12", 2) => Some(YuvLayout::Nv12 {
            y: plane(0)?,
            uv: plane(1)?,
        }),
        ("I420", 3) | ("I420A", 4) => Some(YuvLayout::I420 {
            y: plane(0)?,
            u: plane(1)?,
            v: plane(2)?,
            a: if format == "I420A" {
                Some(plane(3)?)
            } else {
                None
            },
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
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
    fn audio_timeline_is_what_the_engine_parsed() {
        let controller = controller(8);
        assert_eq!(controller.audio_timeline().expect("no composition"), None);
        controller
            .load_composition(COMPOSITION_JSON)
            .expect("load composition");
        assert_eq!(controller.audio_timeline().expect("no audio"), None);

        let mut composition: serde_json::Value =
            serde_json::from_str(COMPOSITION_JSON).expect("composition json");
        composition["audio"] = serde_json::json!({
            "tracks": [{ "id": "music", "volume": 0.5 }],
            "clips": [{
                "id": "intro",
                "source_id": "song",
                "track_id": "music",
                "start_ms": 33,
                "duration_ms": 1500,
                "source_start_ms": 250
            }]
        });
        controller
            .load_composition(&composition.to_string())
            .expect("load composition with audio");

        let timeline: serde_json::Value = serde_json::from_str(
            &controller
                .audio_timeline()
                .expect("audio timeline")
                .expect("composition has audio"),
        )
        .expect("timeline json");
        assert_eq!(
            timeline,
            serde_json::json!({
                "tracks": [{
                    "id": "music",
                    "muted": false,
                    "solo": false,
                    "volume": 0.5,
                }],
                "clips": [{
                    "id": "intro",
                    "sourceId": "song",
                    "trackId": "music",
                    "startMs": 33,
                    "durationMs": 1500,
                    "sourceStartMs": 250,
                    "volume": 1.0,
                }],
                "durationFrames": 200,
                "fps": 30.0,
            })
        );
    }

    #[test]
    fn reloading_with_an_occupied_cache_pins_frame_zero() {
        // Room for two frames: one per stream.
        let controller = controller(8);
        register(&controller, "a", 30.0);
        register(&controller, "b", 30.0);
        controller
            .load_composition(COMPOSITION_JSON)
            .expect("load composition");
        controller.set_frame(100).expect("seek");
        upload(&controller.media, 100);

        controller
            .load_composition(COMPOSITION_JSON)
            .expect("reload composition");
        assert_eq!(controller.current_frame().expect("current frame"), 0);
        upload_one(&controller, "a", 0);
        upload_one(&controller, "b", 0);

        assert!(ready_at_playhead(&controller));
        assert!(!controller.media.has_video_frame("a", 100));
        assert!(!controller.media.has_video_frame("b", 100));
    }

    #[test]
    fn clearing_videos_keeps_the_playhead_pinned() {
        // Room for one frame, but the playhead needs one frame from each of the two streams.
        let controller = controller(4);
        register(&controller, "a", 30.0);
        register(&controller, "b", 30.0);
        controller
            .load_composition(COMPOSITION_JSON)
            .expect("load composition");
        upload_one(&controller, "a", 0);
        upload_one(&controller, "b", 0);

        controller.clear_videos().expect("clear video frames");
        upload_one(&controller, "a", 0);
        upload_one(&controller, "b", 0);

        assert!(ready_at_playhead(&controller));
    }

    fn controller(budget_bytes: usize) -> LumenPreviewController {
        let controller = LumenPreviewController {
            state: RefCell::new(PreviewState::default()),
            media: WasmMediaStore::default(),
        };
        controller
            .media
            .set_video_frame_cache_bytes(budget_bytes)
            .expect("set budget");
        controller
    }

    fn register(controller: &LumenPreviewController, stream_id: &str, fps: f32) {
        controller
            .set_video_metadata(stream_id, 1, 1, 200, fps)
            .expect("set metadata");
    }

    fn upload_one(controller: &LumenPreviewController, stream_id: &str, frame: u32) {
        controller
            .set_video_frame(stream_id, frame, 1, 1, vec![255, 0, 0, 255])
            .expect("set frame");
    }

    fn ready_at_playhead(controller: &LumenPreviewController) -> bool {
        let state = controller.state.borrow();
        let composition = state.composition.as_ref().expect("composition");
        let requirements =
            collect_frame_requirements(composition, &controller.media, state.current_frame)
                .expect("frame requirements");
        requirements.videos.iter().all(|video| {
            video
                .frames
                .iter()
                .all(|frame| controller.media.has_video_frame(&video.stream_id, *frame))
        })
    }

    #[test]
    fn registering_streams_after_load_pins_the_playhead() {
        let controller = controller(4);
        controller
            .load_composition(COMPOSITION_JSON)
            .expect("load composition");
        register(&controller, "a", 30.0);
        register(&controller, "b", 30.0);
        upload_one(&controller, "a", 0);
        upload_one(&controller, "b", 0);

        assert!(ready_at_playhead(&controller));
    }

    #[test]
    fn re_adding_a_removed_stream_pins_its_frames() {
        let controller = controller(4);
        register(&controller, "a", 30.0);
        register(&controller, "b", 30.0);
        controller
            .load_composition(COMPOSITION_JSON)
            .expect("load composition");
        upload_one(&controller, "a", 0);

        controller.remove_video_source("b").expect("remove stream");
        register(&controller, "b", 30.0);
        upload_one(&controller, "b", 0);
        upload_one(&controller, "a", 0);

        assert!(ready_at_playhead(&controller));
    }

    #[test]
    fn changing_stream_fps_pins_the_remapped_frames() {
        let controller = controller(4);
        register(&controller, "a", 30.0);
        register(&controller, "b", 30.0);
        controller
            .load_composition(COMPOSITION_JSON)
            .expect("load composition");
        controller.set_frame(30).expect("seek");

        register(&controller, "a", 15.0);
        register(&controller, "b", 15.0);
        upload_one(&controller, "a", 15);
        upload_one(&controller, "b", 15);

        assert!(ready_at_playhead(&controller));
    }
}
