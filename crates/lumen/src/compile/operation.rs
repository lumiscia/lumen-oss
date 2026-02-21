use std::collections::HashMap;

use thiserror::Error;

use crate::expression::{ExprEvalContext, ExprEvalError, ParsedExpr, eval_expr};
use crate::model::{
    BaseStyle, BlendMode, Canvas, ClipStyle, FitMode, Layer, LayoutNode, LayoutNodeKind,
    LayoutNodeStyle, ShapeGeometry, SourceFrameContext, SourceMedia, TextAlign, Timeline,
    VerticalAlign, VideoPipeline,
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
    pub(crate) literal_scalars: Vec<(String, f32)>,
    pub(crate) expression_scalars: Vec<CompiledExpressionBinding>,
    pub(crate) eval_order: Vec<usize>,
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
        if frame >= self.total_frames() {
            return Err(RuntimeEvalError::FrameOutOfRange {
                frame,
                total_frames: self.total_frames(),
            });
        }

        let mut values =
            HashMap::with_capacity(self.literal_scalars.len() + self.expression_scalars.len() + 6);
        values.insert("canvas.width".to_string(), self.canvas.width as f32);
        values.insert("canvas.height".to_string(), self.canvas.height as f32);
        values.insert("timeline.frame".to_string(), frame as f32);
        values.insert(
            "timeline.duration".to_string(),
            self.timeline.duration_frames as f32,
        );
        values.insert("timeline.fps".to_string(), self.timeline.fps_f32());

        for (path, value) in &self.literal_scalars {
            values.insert(path.clone(), *value);
        }

        for binding_index in &self.eval_order {
            let binding = &self.expression_scalars[*binding_index];
            let ctx = FrameEvalMap { values: &values };
            let value =
                eval_expr(&binding.expr, &ctx).map_err(|source| RuntimeEvalError::Expr {
                    owner_id: binding.owner_id.clone(),
                    property_path: binding.path.clone(),
                    expression: binding.expression.clone(),
                    source,
                })?;
            values.insert(binding.path.clone(), value);
        }

        Ok(RuntimeFrameContext { values })
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeFrameContext {
    values: HashMap<String, f32>,
}

impl RuntimeFrameContext {
    pub fn get(&self, path: &str) -> Option<f32> {
        self.values.get(path).copied()
    }

    pub fn resolve(&self, target: &str, property: &str) -> Option<f32> {
        let path = format!("{}.{}", target, property);
        self.get(path.as_str())
    }
}

#[derive(Debug)]
struct FrameEvalMap<'a> {
    values: &'a HashMap<String, f32>,
}

impl ExprEvalContext for FrameEvalMap<'_> {
    fn resolve(&self, target: &str, property: &str) -> Option<f32> {
        let path = format!("{}.{}", target, property);
        self.values.get(path.as_str()).copied()
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

impl CompiledBaseStyle {
    pub fn from_base(base: &BaseStyle, defaults: BaseStyleDefaults) -> Self {
        Self {
            visible: base.visible,
            opacity: defaults.opacity,
            blend_mode: base.blend_mode,
            blur: defaults.blur,
            shadow: defaults.shadow,
            transform: defaults.transform,
            alignment: defaults.alignment,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BaseStyleDefaults {
    pub opacity: ScalarHandle,
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
        state.get(self.path()).unwrap_or(self.fallback())
    }
}

impl From<&LayoutNodeStyle> for CompiledLayoutNodeStyle {
    fn from(_: &LayoutNodeStyle) -> Self {
        Self {
            width: None,
            height: None,
            min_width: None,
            min_height: None,
            max_width: None,
            max_height: None,
            padding_left: None,
            padding_top: None,
            padding_right: None,
            padding_bottom: None,
            gap: None,
            grow: None,
            shrink: None,
            basis: None,
        }
    }
}

impl From<&LayoutNode> for CompiledLayoutNode {
    fn from(node: &LayoutNode) -> Self {
        let kind = match &node.kind {
            LayoutNodeKind::Container { children } => CompiledLayoutNodeKind::Container {
                children: children.iter().map(Self::from).collect(),
            },
            LayoutNodeKind::Text { content } => CompiledLayoutNodeKind::Text {
                content: content.clone(),
            },
            LayoutNodeKind::Image { .. } => CompiledLayoutNodeKind::Image { source_index: 0 },
        };

        Self {
            id: node.id.clone(),
            style: CompiledLayoutNodeStyle::from(&node.style),
            kind,
        }
    }
}

impl From<&ClipStyle> for CompiledClipStyle {
    fn from(style: &ClipStyle) -> Self {
        let zero = ScalarHandle::new("__default.zero".to_string(), 0.0);
        let one = ScalarHandle::new("__default.one".to_string(), 1.0);

        Self {
            base: CompiledBaseStyle {
                visible: style.base.visible,
                opacity: one.clone(),
                blend_mode: style.base.blend_mode,
                blur: zero.clone(),
                shadow: None,
                transform: CompiledTransformStyle {
                    x: zero.clone(),
                    y: zero.clone(),
                    width: one.clone(),
                    height: one.clone(),
                    rotation: zero.clone(),
                    anchor_x: zero.clone(),
                    anchor_y: zero.clone(),
                    scale_x: one.clone(),
                    scale_y: one.clone(),
                    skew_x: zero.clone(),
                    skew_y: zero.clone(),
                },
                alignment: [zero.clone(), zero.clone()],
            },
            fill: style.fill,
            stroke: None,
            corner_radius: None,
            font_family: style.font_family.clone(),
            font_size: one.clone(),
            font_weight: one.clone(),
            color: style.color,
            align: style.align.unwrap_or_default(),
            vertical_align: style.vertical_align.unwrap_or_default(),
            letter_spacing: zero.clone(),
            line_height: one,
            fit: style.fit.unwrap_or_default(),
            color_matrix: style.color_matrix,
        }
    }
}
