use std::cell::{Ref, RefCell, RefMut};

use lumen_engine::{
    composition::Composition,
    media::{
        ChromaSubsampling, Rotation, VideoMetadata, YuvChroma, YuvColor, YuvFormat, YuvFrame,
        YuvLayout, YuvMatrix, YuvPlane, collect_frame_requirements,
    },
};
use wasm_bindgen::prelude::*;

use crate::{
    install_panic_hook,
    media::WasmMediaStore,
    renderer::SurfaceCompositionRenderer,
    types::{AudioTimelinePayload, FrameRequirementsPayload},
    utils::{
        composition_json_to_composition, parse_failure, raw_frame_from_rgba, validate_rgba_len,
    },
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
        let composition = composition_json_to_composition(composition_json)
            .map_err(|error| parse_failure(&error))?;

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

    /// Fails as `loadComposition` would on a composition the engine rejects, without loading it.
    #[wasm_bindgen(js_name = "validateComposition")]
    pub fn validate_composition(&self, composition_json: &str) -> Result<(), JsValue> {
        composition_json_to_composition(composition_json)
            .map(drop)
            .map_err(|error| parse_failure(&error))
    }

    /// What the loaded composition's audio plays over its timeline, as the JSON of an
    /// [`AudioTimelinePayload`], or `None` when it has no audio.
    #[wasm_bindgen(js_name = "audioTimeline")]
    pub fn audio_timeline(&self) -> Result<Option<String>, JsValue> {
        let state = self.state()?;
        let Some(composition) = state.composition.as_ref().filter(|c| c.has_audio()) else {
            return Ok(None);
        };
        let plan = lumen_engine::audio::preview_plan(composition, Some(&self.media))
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let payload = AudioTimelinePayload {
            plan: &plan,
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

    /// Forgets the model `source` names. The next render frees the GPU copy of every model and
    /// loads again those it still draws.
    #[wasm_bindgen(js_name = "removeModelSource")]
    pub fn remove_model_source(&self, source: &str) -> Result<(), JsValue> {
        self.change_media(|media| media.remove_model(source))?;
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

    #[wasm_bindgen(js_name = "hasModel")]
    pub fn has_model(&self, source: &str) -> bool {
        self.media.has_model(source)
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
        let frame = raw_frame_from_rgba(width, height, rgba).map_err(|e| JsValue::from_str(&e))?;
        self.change_media(|media| media.set_image(image_id.to_string(), frame))?;
        self.reset_renderer()
    }

    /// Stores a binary glTF (`.glb`) file under the `source` a `model_3d` names. It is read when a
    /// render first draws it, so a file that isn't a model fails that render.
    #[wasm_bindgen(js_name = "setModel")]
    pub fn set_model(&self, source: &str, data: Vec<u8>) -> Result<(), JsValue> {
        self.change_media(|media| media.set_model(source.to_string(), data))?;
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
        let image = raw_frame_from_rgba(width, height, rgba).map_err(|e| JsValue::from_str(&e))?;
        self.change_media(|media| media.set_video_frame(stream_id.to_string(), frame, image))
    }

    /// Stores a frame copied from a `VideoFrame` in its native YUV `format` (a WebCodecs pixel
    /// format name), for the renderer to convert. `width` and `height` are the copied size before
    /// `rotation`, in degrees clockwise. `offsets` and `strides` are what `copyTo` returned.
    /// `matrix` is the source's tagged WebCodecs matrix; without a known one, the engine's rule for
    /// untagged video picks it from the height. `full_range` describes the copied samples.
    /// `display_width` and `display_height` are the size the frame shows at, after rotation and
    /// the source's pixel aspect ratio.
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
        rotation: u32,
        display_width: u32,
        display_height: u32,
    ) -> Result<(), JsValue> {
        if display_width == 0 || display_height == 0 {
            return Err(JsValue::from_str("video dimensions must be > 0"));
        }
        let yuv = yuv_frame(
            YuvInput {
                width,
                height,
                format,
                offsets: &offsets,
                strides: &strides,
                matrix: matrix.as_deref(),
                full_range,
                rotation,
            },
            data,
        )
        .map_err(|e| JsValue::from_str(&e))?;
        self.change_media(|media| {
            media.set_video_frame_yuv(
                stream_id.to_string(),
                frame,
                yuv,
                (display_width, display_height),
            )
        })
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

struct YuvInput<'a> {
    width: u32,
    height: u32,
    format: &'a str,
    offsets: &'a [u32],
    strides: &'a [u32],
    matrix: Option<&'a str>,
    full_range: bool,
    rotation: u32,
}

/// How a WebCodecs YUV pixel format lays out its planes.
struct YuvKind {
    subsampling: ChromaSubsampling,
    depth: u8,
    interleaved: bool,
    alpha: bool,
}

/// The layout of a WebCodecs YUV pixel format. Samples deeper than 8 bits are 16-bit little-endian
/// with the value in the low bits.
fn yuv_kind(format: &str) -> Option<YuvKind> {
    if format == "NV12" {
        return Some(YuvKind {
            subsampling: ChromaSubsampling::Yuv420,
            depth: 8,
            interleaved: true,
            alpha: false,
        });
    }
    let rest = format.strip_prefix('I')?;
    let subsampling = match rest.get(..3)? {
        "420" => ChromaSubsampling::Yuv420,
        "422" => ChromaSubsampling::Yuv422,
        "444" => ChromaSubsampling::Yuv444,
        _ => return None,
    };
    let (alpha, depth) = match &rest[3..] {
        "" => (false, 8),
        "A" => (true, 8),
        "P10" => (false, 10),
        "P12" => (false, 12),
        "AP10" => (true, 10),
        "AP12" => (true, 12),
        _ => return None,
    };
    Some(YuvKind {
        subsampling,
        depth,
        interleaved: false,
        alpha,
    })
}

/// The matrix a WebCodecs `VideoMatrixCoefficients` tag names, or the engine's untagged rule when the
/// tag is missing or not one the engine converts with.
fn yuv_matrix(tag: Option<&str>, height: u32) -> YuvMatrix {
    match tag {
        Some("bt709") => YuvMatrix::Bt709,
        Some("bt470bg" | "smpte170m") => YuvMatrix::Bt601,
        Some("bt2020-ncl") => YuvMatrix::Bt2020,
        _ => YuvMatrix::untagged(height),
    }
}

fn yuv_frame(input: YuvInput<'_>, data: Vec<u8>) -> Result<YuvFrame, String> {
    let YuvInput {
        width,
        height,
        format,
        offsets,
        strides,
        matrix,
        full_range,
        rotation,
    } = input;
    let kind =
        yuv_kind(format).ok_or_else(|| format!("unsupported video frame format {format}"))?;
    let rotation = Rotation::from_degrees(rotation)
        .ok_or_else(|| format!("unsupported video frame rotation {rotation}"))?;
    let planes = 2 + usize::from(!kind.interleaved) + usize::from(kind.alpha);
    if offsets.len() != planes || strides.len() != planes {
        return Err(format!(
            "{format} has {planes} planes, got {} offsets and {} strides",
            offsets.len(),
            strides.len()
        ));
    }
    let mut places = offsets
        .iter()
        .zip(strides)
        .map(|(offset, stride)| YuvPlane {
            offset: *offset as usize,
            stride: *stride as usize,
        });
    let mut next = || places.next().expect("plane count is checked");
    let luma = next();
    let chroma = if kind.interleaved {
        YuvChroma::Interleaved(next())
    } else {
        YuvChroma::Planar {
            u: next(),
            v: next(),
        }
    };
    let alpha = kind.alpha.then(&mut next);
    let frame = YuvFrame {
        format: YuvFormat {
            width,
            height,
            subsampling: kind.subsampling,
            depth: kind.depth,
            msb_aligned: false,
            color: YuvColor {
                matrix: yuv_matrix(matrix, height),
                full_range,
            },
            rotation,
        },
        layout: YuvLayout {
            luma,
            chroma,
            alpha,
        },
        data: std::sync::Arc::new(data),
    };
    frame.validate()?;
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use lumen_engine::media::{MediaFrame, VideoFrameResolver, resolve_as};

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
            let image = raw_frame_from_rgba(1, 1, vec![255, 0, 0, 255]).expect("frame");
            media
                .set_video_frame(stream_id.to_string(), frame, image)
                .expect("set frame");
        }
    }

    #[test]
    fn audio_timeline_is_the_plan_of_the_engine_parsed_audio() {
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
                "voices": [{
                    "sourceId": "song",
                    "sourceStart": 12_000.0,
                    "duration": 72_000.0,
                    "fadeIn": 0.0,
                    "fadeOut": 0.0,
                }],
                "segments": [{
                    "voice": 0,
                    "start": 1_584,
                    "frames": 72_000,
                    "position": 0.0,
                    "step": 1.0,
                    "from": 0.0,
                    "to": 72_000.0,
                    "gain": [0.5, 0.5],
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

    fn yuv_input<'a>(format: &'a str, offsets: &'a [u32], strides: &'a [u32]) -> YuvInput<'a> {
        YuvInput {
            width: 4,
            height: 2,
            format,
            offsets,
            strides,
            matrix: None,
            full_range: false,
            rotation: 0,
        }
    }

    #[test]
    fn webcodecs_formats_map_to_their_planes() {
        let nv12 = yuv_frame(yuv_input("NV12", &[0, 8], &[4, 4]), vec![0; 12]).unwrap();
        assert!(matches!(nv12.layout.chroma, YuvChroma::Interleaved(_)));
        assert_eq!(nv12.format.depth, 8);

        let i422_10a = yuv_frame(
            yuv_input("I422AP10", &[0, 16, 24, 32], &[8, 4, 4, 8]),
            vec![0; 48],
        )
        .unwrap();
        assert!(matches!(i422_10a.layout.chroma, YuvChroma::Planar { .. }));
        assert!(i422_10a.layout.alpha.is_some());
        assert_eq!(i422_10a.format.depth, 10);
        assert_eq!(i422_10a.format.subsampling, ChromaSubsampling::Yuv422);

        for format in [
            "I420", "I420A", "I420P10", "I420P12", "I420AP10", "I420AP12", "I422", "I422A",
            "I422P10", "I422P12", "I422AP12", "I444", "I444A", "I444P10", "I444P12", "I444AP10",
        ] {
            assert!(yuv_kind(format).is_some(), "{format}");
        }
        for format in ["RGBA", "NV21", "I410", "I420P8", "I420AP", ""] {
            assert!(yuv_kind(format).is_none(), "{format}");
        }
    }

    #[test]
    fn untagged_or_unknown_matrix_follows_the_engine_rule() {
        assert_eq!(yuv_matrix(Some("bt709"), 100), YuvMatrix::Bt709);
        assert_eq!(yuv_matrix(Some("smpte170m"), 1080), YuvMatrix::Bt601);
        assert_eq!(yuv_matrix(Some("rgb"), 720), YuvMatrix::Bt709);
        assert_eq!(yuv_matrix(None, 576), YuvMatrix::Bt601);
    }

    #[test]
    fn malformed_yuv_frames_are_errors() {
        let bad = |input: YuvInput<'_>, bytes: usize| yuv_frame(input, vec![0; bytes]).is_err();
        assert!(bad(yuv_input("YV12", &[0, 8], &[4, 4]), 12));
        assert!(bad(yuv_input("NV12", &[0], &[4]), 12), "missing plane");
        assert!(bad(yuv_input("NV12", &[0, 8], &[4, 4]), 11), "short data");
        assert!(bad(yuv_input("NV12", &[0, 8], &[3, 4]), 12), "short stride");
        assert!(bad(yuv_input("NV12", &[0, u32::MAX], &[4, 4]), 12));
        assert!(bad(
            YuvInput {
                rotation: 45,
                ..yuv_input("NV12", &[0, 8], &[4, 4])
            },
            12
        ));
        assert!(bad(
            YuvInput {
                width: 0,
                ..yuv_input("NV12", &[0, 8], &[4, 4])
            },
            12
        ));
    }

    fn upload_one(controller: &LumenPreviewController, stream_id: &str, frame: u32) {
        controller
            .set_video_frame(stream_id, frame, 1, 1, vec![255, 0, 0, 255])
            .expect("set frame");
    }

    #[test]
    fn rgba_video_frames_reach_the_engine_as_decoded() {
        let controller = controller(1 << 20);
        let source = vec![100, 50, 24, 128];
        controller
            .set_video_frame("a", 0, 1, 1, source.clone())
            .expect("set frame");

        let (_, resolver) =
            resolve_as::<dyn VideoFrameResolver>(&controller.media, "a").expect("resolver");
        let Ok(MediaFrame::RawRgba(frame)) = resolver.frame(0) else {
            panic!("an RGBA video frame should stay unpremultiplied");
        };
        assert_eq!(*frame.rgba, source);
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
