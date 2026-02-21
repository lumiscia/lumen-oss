use std::collections::HashMap;
use std::sync::Arc;

use thiserror::Error;

use crate::expression::{ExprEvalContext, ExprEvalError, ParsedExpr, eval_expr};
use crate::model::{
    BlendMode, Canvas, FitMode, Layer, LayoutAlign, LayoutDirection, LayoutJustify, ShapeGeometry,
    SourceFrameContext, SourceMedia, TextAlign, Timeline, VerticalAlign, VideoPipeline,
};

use super::scalar::ScalarHandle;

#[derive(Debug, Clone)]
pub struct CompiledTimeline {
    pub canvas: Canvas,
    pub timeline: Timeline,
    pub sources: Vec<CompiledSource>,
    pub layers: Vec<CompiledLayer>,
    pub(crate) operations: Vec<CompiledOperation>,
    pub(crate) frame_index: Vec<Vec<usize>>,
    pub(crate) literal_scalars: Vec<(usize, f32)>,
    pub(crate) expression_scalars: Vec<CompiledExpressionBinding>,
    pub(crate) eval_order: Vec<usize>,
    pub(crate) path_indices: Arc<HashMap<String, usize>>,
    pub(crate) scalar_count: usize,
}

impl CompiledTimeline {
    pub fn total_frames(&self) -> u64 {
        self.timeline.duration_frames
    }

    pub fn operation_indices_for_frame(&self, frame: u64) -> Result<&[usize], RuntimeEvalError> {
        let Some(indices) = self.frame_index.get(frame as usize) else {
            return Err(RuntimeEvalError::FrameOutOfRange {
                frame,
                total_frames: self.total_frames(),
            });
        };
        Ok(indices.as_slice())
    }

    pub fn operation(&self, index: usize) -> Option<&CompiledOperation> {
        self.operations.get(index)
    }

    pub fn source(&self, index: usize) -> Option<&CompiledSource> {
        self.sources.get(index)
    }

    pub fn resolve_frame_context(
        &self,
        frame: u64,
    ) -> Result<RuntimeFrameContext, RuntimeEvalError> {
        self.resolve_frame_context_with_overrides(frame, &HashMap::new())
    }

    pub fn resolve_frame_context_with_overrides(
        &self,
        frame: u64,
        overrides: &HashMap<String, f32>,
    ) -> Result<RuntimeFrameContext, RuntimeEvalError> {
        if frame >= self.total_frames() {
            return Err(RuntimeEvalError::FrameOutOfRange {
                frame,
                total_frames: self.total_frames(),
            });
        }

        let mut scalar_values = vec![0.0; self.scalar_count];
        for (index, value) in &self.literal_scalars {
            if let Some(slot) = scalar_values.get_mut(*index) {
                *slot = *value;
            }
        }
        for (path, value) in overrides {
            if let Some(index) = self.path_indices.get(path.as_str())
                && let Some(slot) = scalar_values.get_mut(*index)
            {
                *slot = *value;
            }
        }

        let canvas_width = self.canvas.width as f32;
        let canvas_height = self.canvas.height as f32;
        let timeline_frame = frame as f32;
        let timeline_duration = self.timeline.duration_frames as f32;
        let timeline_fps = self.timeline.fps_f32();

        for binding_index in &self.eval_order {
            let binding = &self.expression_scalars[*binding_index];
            if overrides.contains_key(binding.path.as_str()) {
                continue;
            }
            let eval_ctx = FrameEvalMap {
                scalar_values: scalar_values.as_slice(),
                path_indices: self.path_indices.as_ref(),
                overrides,
                canvas_width,
                canvas_height,
                timeline_frame,
                timeline_duration,
                timeline_fps,
            };
            let value =
                eval_expr(&binding.expr, &eval_ctx).map_err(|source| RuntimeEvalError::Expr {
                    owner_id: binding.owner_id.clone(),
                    property_path: binding.path.clone(),
                    expression: binding.expression.clone(),
                    source,
                })?;
            if let Some(slot) = scalar_values.get_mut(binding.index) {
                *slot = value;
            }
        }

        Ok(RuntimeFrameContext {
            scalar_values,
            path_indices: Arc::clone(&self.path_indices),
            canvas_width,
            canvas_height,
            timeline_frame,
            timeline_duration,
            timeline_fps,
        })
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeFrameContext {
    scalar_values: Vec<f32>,
    path_indices: Arc<HashMap<String, usize>>,
    canvas_width: f32,
    canvas_height: f32,
    timeline_frame: f32,
    timeline_duration: f32,
    timeline_fps: f32,
}

impl RuntimeFrameContext {
    pub fn get(&self, path: &str) -> Option<f32> {
        match path {
            "canvas.width" => Some(self.canvas_width),
            "canvas.height" => Some(self.canvas_height),
            "timeline.frame" => Some(self.timeline_frame),
            "timeline.duration" => Some(self.timeline_duration),
            "timeline.fps" => Some(self.timeline_fps),
            _ => self
                .path_indices
                .get(path)
                .and_then(|index| self.scalar_values.get(*index))
                .copied(),
        }
    }

    pub fn scalar(&self, index: usize) -> Option<f32> {
        self.scalar_values.get(index).copied()
    }

    pub fn resolve(&self, target: &str, property: &str) -> Option<f32> {
        match (target, property) {
            ("canvas", "width") => Some(self.canvas_width),
            ("canvas", "height") => Some(self.canvas_height),
            ("timeline", "frame") => Some(self.timeline_frame),
            ("timeline", "duration") => Some(self.timeline_duration),
            ("timeline", "fps") => Some(self.timeline_fps),
            _ => {
                let path = format!("{}.{}", target, property);
                self.get(path.as_str())
            }
        }
    }
}

#[derive(Debug)]
struct FrameEvalMap<'a> {
    scalar_values: &'a [f32],
    path_indices: &'a HashMap<String, usize>,
    overrides: &'a HashMap<String, f32>,
    canvas_width: f32,
    canvas_height: f32,
    timeline_frame: f32,
    timeline_duration: f32,
    timeline_fps: f32,
}

impl ExprEvalContext for FrameEvalMap<'_> {
    fn resolve(&self, target: &str, property: &str) -> Option<f32> {
        match (target, property) {
            ("canvas", "width") => Some(self.canvas_width),
            ("canvas", "height") => Some(self.canvas_height),
            ("timeline", "frame") => Some(self.timeline_frame),
            ("timeline", "duration") => Some(self.timeline_duration),
            ("timeline", "fps") => Some(self.timeline_fps),
            _ => {
                let path = format!("{}.{}", target, property);
                if let Some(value) = self.overrides.get(path.as_str()) {
                    return Some(*value);
                }
                self.path_indices
                    .get(path.as_str())
                    .and_then(|index| self.scalar_values.get(*index))
                    .copied()
            }
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum RuntimeEvalError {
    #[error("frame {frame} is out of range for duration {total_frames}")]
    FrameOutOfRange { frame: u64, total_frames: u64 },
    #[error(
        "expression eval failed for `{owner_id}` ({property_path}) in `{expression}`: {source}"
    )]
    Expr {
        owner_id: String,
        property_path: String,
        expression: String,
        source: ExprEvalError,
    },
}

#[derive(Debug, Clone)]
pub struct CompiledExpressionBinding {
    pub index: usize,
    pub path: String,
    pub owner_id: String,
    pub expression: String,
    pub expr: ParsedExpr,
}

#[derive(Debug, Clone)]
pub struct CompiledSource {
    pub id: String,
    pub media: SourceMedia,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct CompiledLayer {
    pub id: String,
    pub items: Vec<CompiledLayerItem>,
}

impl From<&Layer> for CompiledLayer {
    fn from(layer: &Layer) -> Self {
        Self {
            id: layer.id.clone(),
            items: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum CompiledLayerItem {
    Clip(CompiledClipNode),
    Group(CompiledGroupNode),
}

#[derive(Debug, Clone)]
pub struct CompiledClipNode {
    pub operation_index: usize,
    pub mask: Option<Box<CompiledLayerItem>>,
}

#[derive(Debug, Clone)]
pub struct CompiledGroupNode {
    pub id: String,
    pub style: CompiledBaseStyle,
    pub items: Vec<CompiledLayerItem>,
    pub mask: Option<Box<CompiledLayerItem>>,
}

#[derive(Debug, Clone)]
pub struct CompiledOperation {
    pub id: String,
    pub layer_id: String,
    pub start_frame: u64,
    pub end_frame: u64,
    pub z_index: usize,
    pub style: CompiledClipStyle,
    pub kind: CompiledOperationKind,
    pub is_mask: bool,
}

impl CompiledOperation {
    pub fn contains_frame(&self, frame: u64) -> bool {
        frame >= self.start_frame && frame < self.end_frame
    }

    pub fn local_frame(&self, frame: u64) -> u64 {
        frame.saturating_sub(self.start_frame)
    }

    pub fn source_frame_at(&self, frame: u64, source_length: u64) -> Option<u64> {
        if !self.contains_frame(frame) {
            return None;
        }

        match &self.kind {
            CompiledOperationKind::Video(video) => {
                video.pipeline.source_frame_for(SourceFrameContext {
                    local_frame: self.local_frame(frame),
                    source_length,
                })
            }
            _ => None,
        }
    }

    pub fn resolved_video_source_frame(
        &self,
        frame: u64,
        source_length: Option<u64>,
    ) -> Option<u64> {
        if !self.contains_frame(frame) {
            return None;
        }

        match &self.kind {
            CompiledOperationKind::Video(_) => source_length
                .filter(|value| *value > 0)
                .and_then(|length| self.source_frame_at(frame, length))
                .or_else(|| Some(self.local_frame(frame))),
            _ => None,
        }
    }
    pub fn resolved_opacity(&self, state: &RuntimeFrameContext) -> f32 {
        self.style.base.opacity.resolve(state).clamp(0.0, 1.0)
    }

    pub fn resolved_transform(&self, state: &RuntimeFrameContext) -> ResolvedTransform {
        self.style.base.transform.resolve(state)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ResolvedTransform {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
    pub anchor_x: f32,
    pub anchor_y: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub skew_x: f32,
    pub skew_y: f32,
}

#[derive(Debug, Clone)]
pub enum CompiledOperationKind {
    Solid,
    Shape(CompiledShape),
    Text(CompiledText),
    Image(CompiledImage),
    Video(CompiledVideo),
    Layout(CompiledLayoutClip),
}

#[derive(Debug, Clone)]
pub struct CompiledShape {
    pub geometry: ShapeGeometry,
}

#[derive(Debug, Clone)]
pub struct CompiledText {
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct CompiledImage {
    pub source_index: usize,
}

#[derive(Debug, Clone)]
pub struct CompiledVideo {
    pub source_index: usize,
    pub pipeline: VideoPipeline,
}

#[derive(Debug, Clone)]
pub struct CompiledLayoutClip {
    pub root: CompiledLayoutNode,
}

#[derive(Debug, Clone)]
pub struct CompiledLayoutNode {
    pub id: String,
    pub style: CompiledLayoutNodeStyle,
    pub kind: CompiledLayoutNodeKind,
}

#[derive(Debug, Clone)]
pub enum CompiledLayoutNodeKind {
    Container { children: Vec<CompiledLayoutNode> },
    Text { content: String },
    Image { source_index: usize },
}

#[derive(Debug, Clone)]
pub struct CompiledLayoutNodeStyle {
    pub width: Option<ScalarHandle>,
    pub height: Option<ScalarHandle>,
    pub min_width: Option<ScalarHandle>,
    pub min_height: Option<ScalarHandle>,
    pub max_width: Option<ScalarHandle>,
    pub max_height: Option<ScalarHandle>,
    pub padding_left: Option<ScalarHandle>,
    pub padding_top: Option<ScalarHandle>,
    pub padding_right: Option<ScalarHandle>,
    pub padding_bottom: Option<ScalarHandle>,
    pub gap: Option<ScalarHandle>,
    pub grow: Option<ScalarHandle>,
    pub shrink: Option<ScalarHandle>,
    pub basis: Option<ScalarHandle>,
    pub justify: Option<LayoutJustify>,
    pub align: Option<LayoutAlign>,
    pub direction: Option<LayoutDirection>,
}

#[derive(Debug, Clone)]
pub struct CompiledClipStyle {
    pub base: CompiledBaseStyle,
    pub fill: Option<[u8; 4]>,
    pub stroke: Option<CompiledStrokeStyle>,
    pub corner_radius: Option<[ScalarHandle; 4]>,
    pub font_family: Option<String>,
    pub font_size: ScalarHandle,
    pub font_weight: ScalarHandle,
    pub color: Option<[u8; 4]>,
    pub align: TextAlign,
    pub vertical_align: VerticalAlign,
    pub letter_spacing: ScalarHandle,
    pub line_height: ScalarHandle,
    pub fit: FitMode,
    pub color_matrix: Option<[[f32; 5]; 4]>,
}

#[derive(Debug, Clone)]
pub struct CompiledStrokeStyle {
    pub color: [u8; 4],
    pub width: ScalarHandle,
    pub dash_pattern: Vec<ScalarHandle>,
    pub dash_offset: ScalarHandle,
}

#[derive(Debug, Clone)]
pub struct CompiledBaseStyle {
    pub visible: bool,
    pub opacity: ScalarHandle,
    pub blend_mode: BlendMode,
    pub blur: ScalarHandle,
    pub shadow: Option<CompiledShadowStyle>,
    pub transform: CompiledTransformStyle,
    pub alignment: [ScalarHandle; 2],
}

#[derive(Debug, Clone)]
pub struct CompiledTransformStyle {
    pub x: ScalarHandle,
    pub y: ScalarHandle,
    pub width: ScalarHandle,
    pub height: ScalarHandle,
    pub rotation: ScalarHandle,
    pub anchor_x: ScalarHandle,
    pub anchor_y: ScalarHandle,
    pub scale_x: ScalarHandle,
    pub scale_y: ScalarHandle,
    pub skew_x: ScalarHandle,
    pub skew_y: ScalarHandle,
}

impl CompiledTransformStyle {
    pub fn resolve(&self, state: &RuntimeFrameContext) -> ResolvedTransform {
        ResolvedTransform {
            x: self.x.resolve(state),
            y: self.y.resolve(state),
            width: self.width.resolve(state),
            height: self.height.resolve(state),
            rotation: self.rotation.resolve(state),
            anchor_x: self.anchor_x.resolve(state),
            anchor_y: self.anchor_y.resolve(state),
            scale_x: self.scale_x.resolve(state),
            scale_y: self.scale_y.resolve(state),
            skew_x: self.skew_x.resolve(state),
            skew_y: self.skew_y.resolve(state),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompiledShadowStyle {
    pub offset_x: ScalarHandle,
    pub offset_y: ScalarHandle,
    pub blur: ScalarHandle,
    pub color: [u8; 4],
}

impl ScalarHandle {
    pub fn resolve(&self, state: &RuntimeFrameContext) -> f32 {
        state.scalar(self.index()).unwrap_or(self.fallback())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use crate::Rational;
    use crate::expression::parse_expr;
    use crate::model::{Canvas, Timeline};

    use super::{
        CompiledExpressionBinding, CompiledTimeline, RuntimeEvalError, RuntimeFrameContext,
        ScalarHandle,
    };

    fn test_timeline() -> CompiledTimeline {
        let mut path_indices = HashMap::new();
        path_indices.insert("layout_node.x".to_string(), 0);
        path_indices.insert("clip_a.x".to_string(), 1);

        CompiledTimeline {
            canvas: Canvas {
                width: 1920,
                height: 1080,
                background: [0, 0, 0, 255],
            },
            timeline: Timeline {
                fps: Rational::new(30, 1),
                duration_frames: 120,
            },
            sources: Vec::new(),
            layers: Vec::new(),
            operations: Vec::new(),
            frame_index: vec![Vec::new(); 120],
            literal_scalars: vec![(0, 0.0)],
            expression_scalars: vec![CompiledExpressionBinding {
                index: 1,
                path: "clip_a.x".to_string(),
                owner_id: "clip_a".to_string(),
                expression: "layout_node.x + 10".to_string(),
                expr: parse_expr("layout_node.x + 10").expect("parse expression"),
            }],
            eval_order: vec![0],
            path_indices: Arc::new(path_indices),
            scalar_count: 2,
        }
    }

    #[test]
    fn resolves_frame_context_with_layout_overrides() {
        let timeline = test_timeline();
        let mut overrides = HashMap::new();
        overrides.insert("layout_node.x".to_string(), 32.0);
        let frame_state = timeline
            .resolve_frame_context_with_overrides(10, &overrides)
            .expect("resolve frame context");

        assert_eq!(frame_state.get("layout_node.x"), Some(32.0));
        assert_eq!(frame_state.get("clip_a.x"), Some(42.0));
        assert_eq!(frame_state.resolve("timeline", "frame"), Some(10.0));
    }

    #[test]
    fn scalar_handle_resolves_by_index() {
        let timeline = test_timeline();
        let mut overrides = HashMap::new();
        overrides.insert("layout_node.x".to_string(), 5.0);
        let frame_state = timeline
            .resolve_frame_context_with_overrides(0, &overrides)
            .expect("resolve frame context");

        let handle = ScalarHandle::new(1, 0.0);
        assert_eq!(handle.resolve(&frame_state), 15.0);
    }

    #[test]
    fn rejects_out_of_range_frame() {
        let timeline = test_timeline();
        let error = timeline
            .resolve_frame_context(999)
            .expect_err("frame must be invalid");
        assert!(matches!(
            error,
            RuntimeEvalError::FrameOutOfRange {
                frame: 999,
                total_frames: 120
            }
        ));
    }

    #[test]
    fn runtime_frame_context_resolves_builtins() {
        let timeline = test_timeline();
        let frame_state: RuntimeFrameContext = timeline
            .resolve_frame_context(4)
            .expect("resolve frame context");
        assert_eq!(frame_state.resolve("canvas", "width"), Some(1920.0));
        assert_eq!(frame_state.resolve("timeline", "fps"), Some(30.0));
    }
}
