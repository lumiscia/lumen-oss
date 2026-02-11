use std::{collections::HashMap, sync::Arc};

use js_sys::{Function, Object, Promise, Reflect};
use lumen::{
    compiler::compile_sequence,
    plan::{RenderOpKind, RenderPlan, VideoRenderOp},
    sequence::{Asset, AssetKind, Sequence, Transform},
};
use serde::{Deserialize, Serialize};
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen]
pub struct LumenWasmRuntime {
    sequence: Option<Sequence>,
    plan: Option<Arc<RenderPlan>>,
    selected_clip_id: Option<String>,
    transform_overrides: HashMap<String, Transform>,
    video_backend: Option<VideoBackend>,
}

#[derive(Clone)]
struct VideoBackend {
    context: JsValue,
    decode_frame: Function,
}

#[derive(Debug, Serialize)]
struct RuntimeState {
    has_sequence: bool,
    total_frames: Option<u64>,
    selected_clip_id: Option<String>,
    selected_transform: Option<Transform>,
}

#[derive(Debug, Serialize)]
struct LoadSequenceResult {
    total_frames: u64,
    fps_num: u32,
    fps_den: u32,
    video_asset_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ClipSelection {
    clip_id: String,
    transform: Option<Transform>,
}

#[derive(Debug, Serialize)]
struct FrameSummary {
    frame_index: u64,
    operation_count: usize,
    video_decode_requests: Vec<VideoDecodeRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoDecodeRequest {
    pub asset_id: String,
    pub source: String,
    pub source_frame: u64,
    pub timeline_frame: u64,
    pub fps_num: u32,
    pub fps_den: u32,
}

#[wasm_bindgen]
impl LumenWasmRuntime {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            sequence: None,
            plan: None,
            selected_clip_id: None,
            transform_overrides: HashMap::new(),
            video_backend: None,
        }
    }

    #[wasm_bindgen(js_name = loadSequenceJson)]
    pub fn load_sequence_json(&mut self, sequence_json: &str) -> Result<JsValue, JsValue> {
        let sequence: Sequence =
            serde_json::from_str(sequence_json).map_err(|err| js_error(&err.to_string()))?;
        self.load_sequence_internal(sequence)
    }

    #[wasm_bindgen(js_name = loadSequence)]
    pub fn load_sequence(&mut self, sequence: JsValue) -> Result<JsValue, JsValue> {
        let sequence =
            serde_wasm_bindgen::from_value(sequence).map_err(|err| js_error(&err.to_string()))?;
        self.load_sequence_internal(sequence)
    }

    #[wasm_bindgen(js_name = getState)]
    pub fn get_state(&self) -> Result<JsValue, JsValue> {
        to_js_value(&RuntimeState {
            has_sequence: self.sequence.is_some(),
            total_frames: self.plan.as_ref().map(|plan| plan.total_frames),
            selected_clip_id: self.selected_clip_id.clone(),
            selected_transform: self.selected_transform(),
        })
    }

    #[wasm_bindgen(js_name = selectClip)]
    pub fn select_clip(&mut self, clip_id: String) -> Result<JsValue, JsValue> {
        self.ensure_clip_exists(&clip_id)?;
        self.selected_clip_id = Some(clip_id.clone());

        to_js_value(&ClipSelection {
            clip_id,
            transform: self.selected_transform(),
        })
    }

    #[wasm_bindgen(js_name = updateTransform)]
    pub fn update_transform(
        &mut self,
        clip_id: String,
        transform: JsValue,
    ) -> Result<JsValue, JsValue> {
        self.ensure_clip_exists(&clip_id)?;
        let transform: Transform =
            serde_wasm_bindgen::from_value(transform).map_err(|err| js_error(&err.to_string()))?;
        self.transform_overrides.insert(clip_id.clone(), transform);

        let sequence = self
            .sequence
            .as_mut()
            .ok_or_else(|| js_error("no sequence loaded"))?;
        update_sequence_transform(sequence, &clip_id, transform)?;

        let plan = compile_sequence(sequence).map_err(|err| js_error(&err.to_string()))?;
        self.plan = Some(Arc::new(plan));

        if self.selected_clip_id.is_none() {
            self.selected_clip_id = Some(clip_id.clone());
        }

        to_js_value(&ClipSelection {
            clip_id,
            transform: Some(transform),
        })
    }

    #[wasm_bindgen(js_name = frameSummary)]
    pub fn frame_summary(&self, frame_index: u64) -> Result<JsValue, JsValue> {
        let plan = self
            .plan
            .as_ref()
            .ok_or_else(|| js_error("no sequence loaded"))?;

        if frame_index >= plan.total_frames {
            return Err(js_error(&format!(
                "frame {frame_index} is out of range (max {})",
                plan.total_frames.saturating_sub(1)
            )));
        }

        let frame = lumen::time::FrameIndex(frame_index);
        let operations: Vec<_> = plan.operations_for_frame(frame).cloned().collect();
        let video_decode_requests = self.video_decode_requests(frame_index, &operations)?;

        to_js_value(&FrameSummary {
            frame_index,
            operation_count: operations.len(),
            video_decode_requests,
        })
    }

    #[wasm_bindgen(js_name = setVideoBackend)]
    pub fn set_video_backend(&mut self, backend: JsValue) -> Result<(), JsValue> {
        let object: Object = backend
            .dyn_into()
            .map_err(|_| js_error("video backend must be an object"))?;

        let decode_frame = Reflect::get(&object, &JsValue::from_str("decodeFrame"))
            .map_err(|_| js_error("failed to access `decodeFrame` on video backend"))?
            .dyn_into::<Function>()
            .map_err(|_| js_error("video backend must implement decodeFrame(request): Promise"))?;

        self.video_backend = Some(VideoBackend {
            context: object.into(),
            decode_frame,
        });
        Ok(())
    }

    #[wasm_bindgen(js_name = clearVideoBackend)]
    pub fn clear_video_backend(&mut self) {
        self.video_backend = None;
    }

    #[wasm_bindgen(js_name = decodeVideoFrame)]
    pub async fn decode_video_frame(&self, request: JsValue) -> Result<JsValue, JsValue> {
        let backend = self
            .video_backend
            .as_ref()
            .ok_or_else(|| js_error("no video backend registered"))?;

        let request: VideoDecodeRequest =
            serde_wasm_bindgen::from_value(request).map_err(|err| js_error(&err.to_string()))?;
        let request_value =
            serde_wasm_bindgen::to_value(&request).map_err(|err| js_error(&err.to_string()))?;

        let promise_value = backend
            .decode_frame
            .call1(&backend.context, &request_value)
            .map_err(|_| js_error("video backend decodeFrame call failed"))?;

        let promise: Promise = promise_value
            .dyn_into()
            .map_err(|_| js_error("video backend decodeFrame must return a Promise"))?;

        JsFuture::from(promise)
            .await
            .map_err(|err| js_error(&format!("video backend decodeFrame rejected: {err:?}")))
    }

    #[wasm_bindgen(js_name = videoDecodeRequests)]
    pub fn video_decode_requests_for_frame(&self, frame_index: u64) -> Result<JsValue, JsValue> {
        let plan = self
            .plan
            .as_ref()
            .ok_or_else(|| js_error("no sequence loaded"))?;
        if frame_index >= plan.total_frames {
            return Err(js_error(&format!(
                "frame {frame_index} is out of range (max {})",
                plan.total_frames.saturating_sub(1)
            )));
        }

        let frame = lumen::time::FrameIndex(frame_index);
        let operations: Vec<_> = plan.operations_for_frame(frame).cloned().collect();
        let requests = self.video_decode_requests(frame_index, &operations)?;
        to_js_value(&requests)
    }
}

impl LumenWasmRuntime {
    fn load_sequence_internal(&mut self, sequence: Sequence) -> Result<JsValue, JsValue> {
        let plan = compile_sequence(&sequence).map_err(|err| js_error(&err.to_string()))?;
        let result = LoadSequenceResult {
            total_frames: plan.total_frames,
            fps_num: plan.fps.num,
            fps_den: plan.fps.den,
            video_asset_ids: sequence
                .assets
                .iter()
                .filter(|asset| asset.kind == AssetKind::Video)
                .map(|asset| asset.id.clone())
                .collect(),
        };

        self.sequence = Some(sequence);
        self.plan = Some(Arc::new(plan));
        self.selected_clip_id = None;
        self.transform_overrides.clear();

        to_js_value(&result)
    }

    fn selected_transform(&self) -> Option<Transform> {
        let clip_id = self.selected_clip_id.as_ref()?;
        self.transform_overrides
            .get(clip_id)
            .copied()
            .or_else(|| lookup_sequence_transform(self.sequence.as_ref()?, clip_id))
    }

    fn ensure_clip_exists(&self, clip_id: &str) -> Result<(), JsValue> {
        let sequence = self
            .sequence
            .as_ref()
            .ok_or_else(|| js_error("no sequence loaded"))?;

        let exists = sequence
            .tracks
            .iter()
            .any(|track| track.clips.iter().any(|clip| clip.id == clip_id));

        if !exists {
            return Err(js_error(&format!("clip `{clip_id}` does not exist")));
        }

        Ok(())
    }

    fn video_decode_requests(
        &self,
        frame_index: u64,
        operations: &[lumen::plan::RenderOp],
    ) -> Result<Vec<VideoDecodeRequest>, JsValue> {
        let sequence = self
            .sequence
            .as_ref()
            .ok_or_else(|| js_error("no sequence loaded"))?;

        let video_assets = video_asset_sources(&sequence.assets);
        let fps_num = sequence.timeline.fps.num;
        let fps_den = sequence.timeline.fps.den;

        operations
            .iter()
            .filter_map(|operation| match &operation.kind {
                RenderOpKind::Video(video) => Some((operation, video)),
                _ => None,
            })
            .map(|(operation, video)| {
                build_video_decode_request(
                    frame_index,
                    operation.start_frame.0,
                    operation.source_in_frame.0,
                    video,
                    fps_num,
                    fps_den,
                    &video_assets,
                )
            })
            .collect()
    }
}

fn build_video_decode_request(
    frame_index: u64,
    op_start_frame: u64,
    source_in_frame: u64,
    video: &VideoRenderOp,
    fps_num: u32,
    fps_den: u32,
    video_assets: &HashMap<String, String>,
) -> Result<VideoDecodeRequest, JsValue> {
    let local_frame = frame_index.saturating_sub(op_start_frame);
    let mut source_offset = ((local_frame as f64) * (video.speed as f64)).floor() as u64;

    if video.source_span_frames > 0 {
        source_offset = source_offset.min(video.source_span_frames.saturating_sub(1));
    }

    let source_offset = if video.reverse {
        video
            .source_span_frames
            .saturating_sub(1)
            .saturating_sub(source_offset)
    } else {
        source_offset
    };

    let source_frame = source_in_frame.saturating_add(source_offset);
    let source = video_assets
        .get(&video.asset_id)
        .cloned()
        .ok_or_else(|| js_error(&format!("missing video asset `{}`", video.asset_id)))?;

    Ok(VideoDecodeRequest {
        asset_id: video.asset_id.clone(),
        source,
        source_frame,
        timeline_frame: frame_index,
        fps_num,
        fps_den,
    })
}

fn video_asset_sources(assets: &[Asset]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for asset in assets {
        if asset.kind == AssetKind::Video {
            out.insert(asset.id.clone(), asset.source.clone());
        }
    }

    out
}

fn update_sequence_transform(
    sequence: &mut Sequence,
    clip_id: &str,
    transform: Transform,
) -> Result<(), JsValue> {
    for track in &mut sequence.tracks {
        if let Some(clip) = track.clips.iter_mut().find(|clip| clip.id == clip_id) {
            clip.transform = transform;
            return Ok(());
        }
    }

    Err(js_error(&format!("clip `{clip_id}` does not exist")))
}

fn lookup_sequence_transform(sequence: &Sequence, clip_id: &str) -> Option<Transform> {
    sequence
        .tracks
        .iter()
        .flat_map(|track| track.clips.iter())
        .find(|clip| clip.id == clip_id)
        .map(|clip| clip.transform)
}

fn to_js_value<T: Serialize>(value: &T) -> Result<JsValue, JsValue> {
    serde_wasm_bindgen::to_value(value).map_err(|err| js_error(&err.to_string()))
}

fn js_error(message: &str) -> JsValue {
    JsValue::from_str(message)
}
