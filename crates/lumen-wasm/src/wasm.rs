use std::{collections::HashMap, sync::Arc};

use js_sys::{Function, Object, Promise, Reflect};
use lumen::{
    ClipContent, CompiledOperationKind, CompiledTimeline, Project, Transform, compile_project,
};
use serde::Serialize;
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen]
pub struct LumenWasmRuntime {
    project: Option<Project>,
    timeline: Option<Arc<CompiledTimeline>>,
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
    has_project: bool,
    total_frames: Option<u64>,
    selected_clip_id: Option<String>,
    selected_transform: Option<Transform>,
}

#[derive(Debug, Serialize)]
struct LoadProjectResult {
    total_frames: u64,
    fps_num: u32,
    fps_den: u32,
    video_source_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ClipSelection {
    clip_id: String,
    transform: Option<Transform>,
}

#[derive(Debug, Serialize)]
struct FramePlan {
    frame_index: u64,
    operation_count: usize,
    operations: Vec<OperationSummary>,
    video_decode_requests: Vec<VideoDecodeRequest>,
}

#[derive(Debug, Serialize)]
struct OperationSummary {
    id: String,
    layer_id: String,
    kind: &'static str,
    z_index: i32,
    start_frame: u64,
    end_frame: u64,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct VideoDecodeRequest {
    pub source_id: String,
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
            project: None,
            timeline: None,
            selected_clip_id: None,
            transform_overrides: HashMap::new(),
            video_backend: None,
        }
    }

    #[wasm_bindgen(js_name = loadProjectJson)]
    pub fn load_project_json(&mut self, project_json: &str) -> Result<JsValue, JsValue> {
        let project: Project =
            serde_json::from_str(project_json).map_err(|err| js_error(&err.to_string()))?;
        self.load_project_internal(project)
    }

    #[wasm_bindgen(js_name = loadProject)]
    pub fn load_project(&mut self, project: JsValue) -> Result<JsValue, JsValue> {
        let project =
            serde_wasm_bindgen::from_value(project).map_err(|err| js_error(&err.to_string()))?;
        self.load_project_internal(project)
    }

    #[wasm_bindgen(js_name = getState)]
    pub fn get_state(&self) -> Result<JsValue, JsValue> {
        to_js_value(&RuntimeState {
            has_project: self.project.is_some(),
            total_frames: self
                .timeline
                .as_ref()
                .map(|timeline| timeline.total_frames()),
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

        let project = self
            .project
            .as_mut()
            .ok_or_else(|| js_error("no project loaded"))?;

        update_clip_transform(project, &clip_id, transform)?;
        self.transform_overrides.insert(clip_id.clone(), transform);

        let timeline = compile_project(project).map_err(|err| js_error(&err.to_string()))?;
        self.timeline = Some(Arc::new(timeline));

        if self.selected_clip_id.is_none() {
            self.selected_clip_id = Some(clip_id.clone());
        }

        to_js_value(&ClipSelection {
            clip_id,
            transform: Some(transform),
        })
    }

    #[wasm_bindgen(js_name = framePlan)]
    pub fn frame_plan(&self, frame_index: u64) -> Result<JsValue, JsValue> {
        let timeline = self
            .timeline
            .as_ref()
            .ok_or_else(|| js_error("no project loaded"))?;

        if frame_index >= timeline.total_frames() {
            return Err(js_error(&format!(
                "frame {frame_index} is out of range (max {})",
                timeline.total_frames().saturating_sub(1)
            )));
        }

        let operation_indices = timeline
            .operation_indices_for_frame(frame_index)
            .map_err(|err| js_error(&err.to_string()))?;

        let mut operations = Vec::new();
        let mut decode_requests = Vec::new();

        for operation_index in operation_indices {
            let operation = timeline
                .operation(*operation_index)
                .ok_or_else(|| js_error("operation index was missing"))?;

            operations.push(OperationSummary {
                id: operation.id.clone(),
                layer_id: operation.layer_id.clone(),
                kind: op_kind_label(&operation.kind),
                z_index: operation.z_index,
                start_frame: operation.start_frame,
                end_frame: operation.end_frame,
            });

            if let CompiledOperationKind::Video(video) = &operation.kind {
                if let Some(source_frame) = operation
                    .resolve_video_source_frame(frame_index)
                    .map_err(|err| js_error(&err.to_string()))?
                {
                    decode_requests.push(VideoDecodeRequest {
                        source_id: video.source_id.clone(),
                        source_frame,
                        timeline_frame: frame_index,
                        fps_num: timeline.timeline.fps.num,
                        fps_den: timeline.timeline.fps.den,
                    });
                }
            }
        }

        to_js_value(&FramePlan {
            frame_index,
            operation_count: operations.len(),
            operations,
            video_decode_requests: decode_requests,
        })
    }

    #[wasm_bindgen(js_name = videoDecodeRequests)]
    pub fn video_decode_requests_for_frame(&self, frame_index: u64) -> Result<JsValue, JsValue> {
        let plan = self.frame_plan(frame_index)?;
        let plan: FramePlanValue = serde_wasm_bindgen::from_value(plan)
            .map_err(|err| js_error(&format!("failed to read frame plan: {err}")))?;
        to_js_value(&plan.video_decode_requests)
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
}

#[derive(Debug, serde::Deserialize)]
struct FramePlanValue {
    video_decode_requests: Vec<VideoDecodeRequest>,
}

impl LumenWasmRuntime {
    fn load_project_internal(&mut self, project: Project) -> Result<JsValue, JsValue> {
        let timeline = compile_project(&project).map_err(|err| js_error(&err.to_string()))?;

        let result = LoadProjectResult {
            total_frames: timeline.total_frames(),
            fps_num: timeline.timeline.fps.num,
            fps_den: timeline.timeline.fps.den,
            video_source_ids: project
                .sources
                .iter()
                .filter(|source| {
                    matches!(
                        source.kind,
                        lumen::SourceKind::File {
                            media: lumen::SourceMediaType::Video,
                            ..
                        } | lumen::SourceKind::Generator {
                            media: lumen::SourceMediaType::Video,
                            ..
                        }
                    )
                })
                .map(|source| source.id.clone())
                .collect(),
        };

        self.project = Some(project);
        self.timeline = Some(Arc::new(timeline));
        self.selected_clip_id = None;
        self.transform_overrides.clear();

        to_js_value(&result)
    }

    fn selected_transform(&self) -> Option<Transform> {
        let clip_id = self.selected_clip_id.as_ref()?;
        self.transform_overrides
            .get(clip_id)
            .copied()
            .or_else(|| lookup_clip_transform(self.project.as_ref()?, clip_id))
    }

    fn ensure_clip_exists(&self, clip_id: &str) -> Result<(), JsValue> {
        let project = self
            .project
            .as_ref()
            .ok_or_else(|| js_error("no project loaded"))?;

        let exists = project
            .layers
            .iter()
            .any(|layer| layer.clips.iter().any(|clip| clip.id == clip_id));

        if !exists {
            return Err(js_error(&format!("clip `{clip_id}` does not exist")));
        }

        Ok(())
    }
}

fn update_clip_transform(
    project: &mut Project,
    clip_id: &str,
    transform: Transform,
) -> Result<(), JsValue> {
    for layer in &mut project.layers {
        if let Some(clip) = layer.clips.iter_mut().find(|clip| clip.id == clip_id) {
            clip.transform = transform;
            return Ok(());
        }
    }

    Err(js_error(&format!(
        "clip `{clip_id}` does not exist in loaded project"
    )))
}

fn lookup_clip_transform(project: &Project, clip_id: &str) -> Option<Transform> {
    project
        .layers
        .iter()
        .flat_map(|layer| layer.clips.iter())
        .find(|clip| clip.id == clip_id)
        .map(|clip| clip.transform)
}

fn op_kind_label(kind: &CompiledOperationKind) -> &'static str {
    match kind {
        CompiledOperationKind::Solid { .. } => "solid",
        CompiledOperationKind::Shape(_) => "shape",
        CompiledOperationKind::Text(_) => "text",
        CompiledOperationKind::Image(_) => "image",
        CompiledOperationKind::Video(_) => "video",
    }
}

fn to_js_value<T: Serialize>(value: &T) -> Result<JsValue, JsValue> {
    serde_wasm_bindgen::to_value(value).map_err(|err| js_error(&err.to_string()))
}

fn js_error(message: &str) -> JsValue {
    js_sys::Error::new(message).into()
}
