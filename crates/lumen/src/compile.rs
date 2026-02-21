use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::{
    expr::{ExprEvalCtx, ExprEvalError, ExprProp, ParsedExpr, Scalar, eval_expr, parse_expr},
    model::{
        Canvas, Clip, ClipContent, ClipGroup, Easing, FitMode, GroupTransform, Layer, LayerItem,
        LayoutClip, LayoutEdges, LayoutImageNode, LayoutNode, LayoutNodeKind, LayoutNodeStyle,
        LayoutTextNode, Project, ScalarKeyframe, Shape, ShapeClip, Source, SourceMediaType,
        SourcePipeline, TextClip, Timeline, Transform,
    },
    source_pipeline::{PipelineError, map_source_frame},
};

const MAX_ITEM_TREE_DEPTH: usize = 16;

#[derive(Debug, Error)]
pub enum CompileError {
    #[error("invalid canvas: {0}")]
    InvalidCanvas(String),
    #[error("invalid timeline: {0}")]
    InvalidTimeline(String),
    #[error("duplicate source id `{0}`")]
    DuplicateSourceId(String),
    #[error("missing source `{0}`")]
    MissingSource(String),
    #[error(
        "source `{source_id}` has incompatible media type: expected {expected:?}, found {found:?}"
    )]
    SourceTypeMismatch {
        source_id: String,
        expected: SourceMediaType,
        found: SourceMediaType,
    },
    #[error("invalid clip `{clip_id}` in layer `{layer_id}`: {reason}")]
    InvalidClip {
        layer_id: String,
        clip_id: String,
        reason: String,
    },
    #[error("invalid group `{group_id}` in layer `{layer_id}`: {reason}")]
    InvalidGroup {
        layer_id: String,
        group_id: String,
        reason: String,
    },
    #[error("item tree exceeds max depth {max_depth} in layer `{layer_id}`")]
    ItemTreeDepthExceeded { layer_id: String, max_depth: usize },
    #[error("expression error in clip `{clip_id}` in layer `{layer_id}`: {reason}")]
    ExprError {
        layer_id: String,
        clip_id: String,
        reason: String,
    },
    #[error("source pipeline error: {0}")]
    Pipeline(#[from] PipelineError),
}

// ── Compiled transform types ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct CompiledTransform {
    pub x: f32,
    pub y: f32,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub rotation_degrees: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct CompiledGroupTransform {
    pub x: f32,
    pub y: f32,
    pub rotation_degrees: f32,
}

// ── Compiled timeline ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CompiledTimeline {
    pub canvas: Canvas,
    pub timeline: Timeline,
    sources: HashMap<String, Source>,
    operations: Vec<CompiledOperation>,
    frame_index: Vec<Vec<usize>>,
    layers: Vec<CompiledLayer>,
    has_compositing_nodes: bool,
    clip_index: ClipPropertyIndex,
}

impl CompiledTimeline {
    pub fn total_frames(&self) -> u64 {
        self.timeline.total_frames
    }

    pub fn operation_indices_for_frame(&self, frame: u64) -> Result<&[usize], CompileError> {
        let frame_index = self.frame_index.get(frame as usize).ok_or_else(|| {
            CompileError::InvalidTimeline(format!("frame {frame} is out of range"))
        })?;
        Ok(frame_index.as_slice())
    }

    pub fn operation(&self, index: usize) -> Option<&CompiledOperation> {
        self.operations.get(index)
    }

    pub fn source(&self, source_id: &str) -> Option<&Source> {
        self.sources.get(source_id)
    }

    pub fn sources(&self) -> impl Iterator<Item = &Source> {
        self.sources.values()
    }

    pub fn layers(&self) -> &[CompiledLayer] {
        self.layers.as_slice()
    }

    pub fn has_compositing_nodes(&self) -> bool {
        self.has_compositing_nodes
    }

    pub(crate) fn clip_index(&self) -> &ClipPropertyIndex {
        &self.clip_index
    }
}

#[derive(Debug, Clone)]
pub struct CompiledLayer {
    pub id: String,
    pub z_index: i32,
    pub items: Vec<CompiledLayerItem>,
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
    pub opacity: f32,
    pub transform: CompiledGroupTransform,
    pub items: Vec<CompiledLayerItem>,
    pub mask: Option<Box<CompiledLayerItem>>,
}

#[derive(Debug, Clone)]
pub struct CompiledOperation {
    pub id: String,
    pub layer_id: String,
    pub start_frame: u64,
    pub end_frame: u64,
    pub z_index: i32,
    pub opacity: f32,
    pub transform: CompiledTransform,
    pub animation: CompiledClipAnimation,
    pub kind: CompiledOperationKind,
}

impl CompiledOperation {
    pub fn contains_frame(&self, frame: u64) -> bool {
        frame >= self.start_frame && frame < self.end_frame
    }

    pub fn local_frame(&self, frame: u64) -> u64 {
        frame.saturating_sub(self.start_frame)
    }

    pub fn resolved_opacity(&self, frame: u64) -> f32 {
        self.resolved_opacity_with_ctx(frame, &EmptyExprCtx)
    }

    pub fn resolved_opacity_with_ctx(&self, frame: u64, expr_ctx: &dyn ExprEvalCtx) -> f32 {
        let local = self.local_frame(frame);
        evaluate_scalar_track(self.opacity, &self.animation.opacity, local, expr_ctx).clamp(0.0, 1.0)
    }

    pub fn resolved_transform(&self, frame: u64) -> CompiledTransform {
        self.resolved_transform_with_ctx(frame, &EmptyExprCtx)
    }

    pub fn resolved_transform_with_ctx(
        &self,
        frame: u64,
        expr_ctx: &dyn ExprEvalCtx,
    ) -> CompiledTransform {
        let local = self.local_frame(frame);
        let mut transform = self.transform;
        transform.x = evaluate_scalar_track(transform.x, &self.animation.x, local, expr_ctx);
        transform.y = evaluate_scalar_track(transform.y, &self.animation.y, local, expr_ctx);
        transform.rotation_degrees = evaluate_scalar_track(
            transform.rotation_degrees,
            &self.animation.rotation_degrees,
            local,
            expr_ctx,
        );
        if let Some(width) = transform.width {
            transform.width = Some(evaluate_scalar_track(width, &self.animation.width, local, expr_ctx));
        }
        if let Some(height) = transform.height {
            transform.height = Some(evaluate_scalar_track(height, &self.animation.height, local, expr_ctx));
        }
        transform
    }

    pub fn resolve_video_source_frame(&self, frame: u64) -> Result<Option<u64>, CompileError> {
        match &self.kind {
            CompiledOperationKind::Video(video) => {
                let local = self.local_frame(frame);
                map_source_frame(&video.pipeline, local).map_err(CompileError::from)
            }
            _ => Ok(None),
        }
    }
}

#[derive(Debug, Clone)]
pub enum CompiledOperationKind {
    Solid { color: crate::model::ColorRgba },
    Shape(ShapeClip),
    Text(TextClip),
    Image(ImageSourceRef),
    Video(VideoSourceRef),
    Layout(LayoutClip),
}

#[derive(Debug, Clone)]
pub struct ImageSourceRef {
    pub source_id: String,
    pub fit: FitMode,
    pub corner_radius: f32,
}

#[derive(Debug, Clone)]
pub struct VideoSourceRef {
    pub source_id: String,
    pub pipeline: SourcePipeline,
    pub fit: FitMode,
    pub corner_radius: f32,
}

#[derive(Debug, Clone, Default)]
pub struct CompiledClipAnimation {
    pub opacity: Vec<CompiledScalarKeyframe>,
    pub x: Vec<CompiledScalarKeyframe>,
    pub y: Vec<CompiledScalarKeyframe>,
    pub width: Vec<CompiledScalarKeyframe>,
    pub height: Vec<CompiledScalarKeyframe>,
    pub rotation_degrees: Vec<CompiledScalarKeyframe>,
}

#[derive(Debug, Clone)]
pub enum CompiledScalarValue {
    Literal(f32),
    DeferredExpr { parsed: ParsedExpr },
}

#[derive(Debug, Clone)]
pub struct CompiledScalarKeyframe {
    pub frame: u64,
    pub value: CompiledScalarValue,
    pub duration_frames: u64,
    pub easing: Easing,
}

// ── Compile context ────────────────────────────────────────────────────────────

struct CompileContext<'a> {
    total_frames: u64,
    sources: &'a HashMap<String, Source>,
    operations: Vec<CompiledOperation>,
    has_compositing_nodes: bool,
    max_depth: usize,
    scale: f32,
    clip_index: &'a ClipPropertyIndex,
    layout_node_ids: &'a HashSet<String>,
}

// ── Clip property index ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) struct ClipPropertyIndex {
    canvas_width: f32,
    canvas_height: f32,
    clips: HashMap<String, CompiledTransform>,
}

impl ExprEvalCtx for ClipPropertyIndex {
    fn resolve(&self, target: &str, property: ExprProp) -> Option<f32> {
        match target {
            "canvas" => match property {
                ExprProp::Width => Some(self.canvas_width),
                ExprProp::Height => Some(self.canvas_height),
                _ => None,
            },
            id => {
                let ct = self.clips.get(id)?;
                match property {
                    ExprProp::X => Some(ct.x),
                    ExprProp::Y => Some(ct.y),
                    ExprProp::Width => ct.width,
                    ExprProp::Height => ct.height,
                }
            }
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

fn prop_str(p: ExprProp) -> &'static str {
    match p {
        ExprProp::Width => "width",
        ExprProp::Height => "height",
        ExprProp::X => "x",
        ExprProp::Y => "y",
    }
}

fn eval_error_reason(e: &ExprEvalError) -> String {
    match e {
        ExprEvalError::UnresolvedRef { target, property } => {
            format!("unknown reference `{}.{}`", target, prop_str(*property))
        }
        ExprEvalError::DivisionByZero => "division by zero".to_string(),
    }
}

/// Resolve a Scalar to f32 in the context of a ClipPropertyIndex.
/// Any unresolved reference is an error (all deps must already be in the index).
fn resolve_scalar(
    layer_id: &str,
    clip_id: &str,
    scalar: &Scalar,
    scale: f32,
    index: &ClipPropertyIndex,
) -> Result<f32, CompileError> {
    match scalar {
        Scalar::Literal(v) => Ok(v * scale),
        Scalar::Expr(s) => {
            let parsed = parse_expr(s).map_err(|e| CompileError::ExprError {
                layer_id: layer_id.to_string(),
                clip_id: clip_id.to_string(),
                reason: e.to_string(),
            })?;
            eval_expr(&parsed, index)
                .map(|v| v * scale)
                .map_err(|e| CompileError::ExprError {
                    layer_id: layer_id.to_string(),
                    clip_id: clip_id.to_string(),
                    reason: eval_error_reason(&e),
                })
        }
    }
}

/// Try to resolve a Scalar during iterative index building.
/// Returns Ok(None) if a dependency is not yet in the index (signal to skip and retry).
/// Returns Ok(Some(v)) on success, Err on permanent failure.
fn try_resolve_scalar_for_index(
    layer_id: &str,
    clip_id: &str,
    scalar: &Scalar,
    index: &ClipPropertyIndex,
    all_ids: &HashSet<String>,
) -> Result<Option<f32>, CompileError> {
    match scalar {
        Scalar::Literal(v) => Ok(Some(*v)),
        Scalar::Expr(s) => {
            let parsed = parse_expr(s).map_err(|e| CompileError::ExprError {
                layer_id: layer_id.to_string(),
                clip_id: clip_id.to_string(),
                reason: e.to_string(),
            })?;
            match eval_expr(&parsed, index) {
                Ok(v) => Ok(Some(v)),
                Err(ExprEvalError::UnresolvedRef { target, property }) => {
                    // If target is a known id not yet resolved → skip for now.
                    if all_ids.contains(&target) {
                        Ok(None)
                    } else {
                        Err(CompileError::ExprError {
                            layer_id: layer_id.to_string(),
                            clip_id: clip_id.to_string(),
                            reason: format!(
                                "unknown reference `{}.{}`",
                                target,
                                prop_str(property)
                            ),
                        })
                    }
                }
                Err(e) => Err(CompileError::ExprError {
                    layer_id: layer_id.to_string(),
                    clip_id: clip_id.to_string(),
                    reason: eval_error_reason(&e),
                }),
            }
        }
    }
}

/// Collect all Clip/Group ids and their transforms from a LayerItem tree.
struct UnresolvedEntry {
    layer_id: String,
    x: Scalar,
    y: Scalar,
    width: Option<Scalar>,
    height: Option<Scalar>,
}

fn collect_layer_item_transforms(
    item: &LayerItem,
    layer_id: &str,
    unresolved: &mut HashMap<String, UnresolvedEntry>,
) -> Result<(), CompileError> {
    match item {
        LayerItem::Clip(clip) => {
            if unresolved.contains_key(&clip.id) {
                return Err(CompileError::InvalidClip {
                    layer_id: layer_id.to_string(),
                    clip_id: clip.id.clone(),
                    reason: "duplicate item id".to_string(),
                });
            }
            unresolved.insert(
                clip.id.clone(),
                UnresolvedEntry {
                    layer_id: layer_id.to_string(),
                    x: clip.transform.x.clone(),
                    y: clip.transform.y.clone(),
                    width: clip.transform.width.clone(),
                    height: clip.transform.height.clone(),
                },
            );
            if let Some(mask) = clip.mask.as_deref() {
                collect_layer_item_transforms(mask, layer_id, unresolved)?;
            }
        }
        LayerItem::Group(group) => {
            if unresolved.contains_key(&group.id) {
                return Err(CompileError::InvalidGroup {
                    layer_id: layer_id.to_string(),
                    group_id: group.id.clone(),
                    reason: "duplicate item id".to_string(),
                });
            }
            unresolved.insert(
                group.id.clone(),
                UnresolvedEntry {
                    layer_id: layer_id.to_string(),
                    x: group.transform.x.clone(),
                    y: group.transform.y.clone(),
                    width: None,
                    height: None,
                },
            );
            for child in &group.items {
                collect_layer_item_transforms(child, layer_id, unresolved)?;
            }
            if let Some(mask) = group.mask.as_deref() {
                collect_layer_item_transforms(mask, layer_id, unresolved)?;
            }
        }
    }
    Ok(())
}

fn collect_project_layout_node_ids(project: &Project) -> HashSet<String> {
    let mut node_ids: HashSet<String> = HashSet::new();
    for layer in &project.layers {
        for item in &layer.items {
            collect_layer_item_node_ids(item, &mut node_ids);
        }
    }
    node_ids
}

fn collect_layer_item_node_ids(item: &LayerItem, node_ids: &mut HashSet<String>) {
    match item {
        LayerItem::Clip(clip) => {
            if let ClipContent::Layout(layout) = &clip.content {
                node_ids.extend(collect_layout_node_ids(&layout.root));
            }
            if let Some(mask) = clip.mask.as_deref() {
                collect_layer_item_node_ids(mask, node_ids);
            }
        }
        LayerItem::Group(group) => {
            for child in &group.items {
                collect_layer_item_node_ids(child, node_ids);
            }
            if let Some(mask) = group.mask.as_deref() {
                collect_layer_item_node_ids(mask, node_ids);
            }
        }
    }
}

fn build_clip_property_index(project: &Project) -> Result<ClipPropertyIndex, CompileError> {
    let mut unresolved: HashMap<String, UnresolvedEntry> = HashMap::new();
    for layer in &project.layers {
        for item in &layer.items {
            collect_layer_item_transforms(item, &layer.id, &mut unresolved)?;
        }
    }

    let all_ids: HashSet<String> = unresolved.keys().cloned().collect();

    let mut index = ClipPropertyIndex {
        canvas_width: project.canvas.width as f32,
        canvas_height: project.canvas.height as f32,
        clips: HashMap::new(),
    };

    // Iterative resolution: keep looping until all entries are resolved or
    // no progress can be made.
    loop {
        if unresolved.is_empty() {
            break;
        }

        let mut progress = false;
        let keys: Vec<String> = unresolved.keys().cloned().collect();

        for key in keys {
            let Some(entry) = unresolved.get(&key) else {
                continue;
            };
            let layer_id = entry.layer_id.clone();

            let rx = try_resolve_scalar_for_index(&layer_id, &key, &entry.x, &index, &all_ids)?;
            let Some(rx) = rx else { continue };

            // Re-borrow after potential index mutation
            let entry = unresolved.get(&key).unwrap();
            let ry = try_resolve_scalar_for_index(&layer_id, &key, &entry.y, &index, &all_ids)?;
            let Some(ry) = ry else { continue };

            let entry = unresolved.get(&key).unwrap();
            let rw = match &entry.width {
                None => None,
                Some(w) => {
                    let resolved =
                        try_resolve_scalar_for_index(&layer_id, &key, w, &index, &all_ids)?;
                    let Some(v) = resolved else { continue };
                    Some(v)
                }
            };

            let entry = unresolved.get(&key).unwrap();
            let rh = match &entry.height {
                None => None,
                Some(h) => {
                    let resolved =
                        try_resolve_scalar_for_index(&layer_id, &key, h, &index, &all_ids)?;
                    let Some(v) = resolved else { continue };
                    Some(v)
                }
            };

            index.clips.insert(
                key.clone(),
                CompiledTransform {
                    x: rx,
                    y: ry,
                    width: rw,
                    height: rh,
                    rotation_degrees: 0.0, // not used in index
                },
            );
            unresolved.remove(&key);
            progress = true;
        }

        if !progress && !unresolved.is_empty() {
            // No progress — cycle or unresolvable reference.
            let (stuck_id, stuck_entry) = unresolved.iter().next().unwrap();
            return Err(CompileError::ExprError {
                layer_id: stuck_entry.layer_id.clone(),
                clip_id: stuck_id.clone(),
                reason: "expression cycle or unresolvable reference".to_string(),
            });
        }
    }

    Ok(index)
}

// ── Animation evaluation ───────────────────────────────────────────────────────

struct EmptyExprCtx;

impl ExprEvalCtx for EmptyExprCtx {
    fn resolve(&self, _target: &str, _property: ExprProp) -> Option<f32> {
        None
    }
}

fn resolve_compiled_scalar_value(
    value: &CompiledScalarValue,
    expr_ctx: &dyn ExprEvalCtx,
) -> Option<f32> {
    match value {
        CompiledScalarValue::Literal(v) => Some(*v),
        CompiledScalarValue::DeferredExpr { parsed } => eval_expr(parsed, expr_ctx).ok(),
    }
}

fn evaluate_scalar_track(
    base: f32,
    track: &[CompiledScalarKeyframe],
    local_frame: u64,
    expr_ctx: &dyn ExprEvalCtx,
) -> f32 {
    let mut current = base;

    for keyframe in track {
        if local_frame < keyframe.frame {
            break;
        }
        let Some(target_value) = resolve_compiled_scalar_value(&keyframe.value, expr_ctx) else {
            continue;
        };

        let from = current;
        if keyframe.duration_frames == 0 {
            current = target_value;
            continue;
        }

        let end = keyframe.frame.saturating_add(keyframe.duration_frames);
        if local_frame >= end {
            current = target_value;
            continue;
        }

        let progress =
            (local_frame.saturating_sub(keyframe.frame)) as f32 / (keyframe.duration_frames as f32);
        let eased = apply_easing(progress, keyframe.easing);
        return from + (target_value - from) * eased;
    }

    current
}

fn apply_easing(progress: f32, easing: Easing) -> f32 {
    let t = progress.clamp(0.0, 1.0);
    match easing {
        Easing::Linear => t,
        Easing::EaseIn => t * t,
        Easing::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
        Easing::EaseInOut => t * t * (3.0 - 2.0 * t),
    }
}

// ── Public entry points ────────────────────────────────────────────────────────

pub fn compile_project(project: &Project) -> Result<CompiledTimeline, CompileError> {
    compile_project_with_scale(project, 1.0)
}

pub fn compile_project_with_scale(
    project: &Project,
    scale: f32,
) -> Result<CompiledTimeline, CompileError> {
    let scale = resolve_scale(scale)?;
    validate_canvas(&project.canvas, scale)?;
    validate_timeline(&project.timeline)?;

    let sources = index_sources(&project.sources)?;

    let clip_index = build_clip_property_index(project)?;
    let layout_node_ids = collect_project_layout_node_ids(project);

    let mut ordered_layers: Vec<(usize, &Layer)> = project.layers.iter().enumerate().collect();
    ordered_layers.sort_by(|left, right| {
        left.1
            .z_index
            .cmp(&right.1.z_index)
            .then_with(|| left.0.cmp(&right.0))
    });

    let mut ctx = CompileContext {
        total_frames: project.timeline.total_frames,
        sources: &sources,
        operations: Vec::new(),
        has_compositing_nodes: false,
        max_depth: MAX_ITEM_TREE_DEPTH,
        scale,
        clip_index: &clip_index,
        layout_node_ids: &layout_node_ids,
    };

    let mut layers = Vec::with_capacity(ordered_layers.len());
    for (_, layer) in ordered_layers {
        layers.push(compile_layer(layer, &mut ctx)?);
    }

    let mut frame_index = vec![Vec::new(); project.timeline.total_frames as usize];
    for (index, operation) in ctx.operations.iter().enumerate() {
        for frame in operation.start_frame..operation.end_frame.min(project.timeline.total_frames) {
            if let Some(slot) = frame_index.get_mut(frame as usize) {
                slot.push(index);
            }
        }
    }

    let CompileContext {
        operations,
        has_compositing_nodes,
        ..
    } = ctx;

    let canvas = compile_canvas(&project.canvas, scale);
    let runtime_clip_index = scale_clip_property_index(&clip_index, scale);

    Ok(CompiledTimeline {
        canvas,
        timeline: project.timeline.clone(),
        sources,
        operations,
        frame_index,
        layers,
        has_compositing_nodes,
        clip_index: runtime_clip_index,
    })
}

fn resolve_scale(scale: f32) -> Result<f32, CompileError> {
    if !scale.is_finite() || scale <= 0.0 {
        return Err(CompileError::InvalidCanvas(
            "scale must be finite and > 0".to_string(),
        ));
    }
    Ok(scale)
}

fn compile_canvas(canvas: &Canvas, scale: f32) -> Canvas {
    Canvas {
        width: scale_dimension(canvas.width, scale),
        height: scale_dimension(canvas.height, scale),
        background: canvas.background,
    }
}

fn scale_dimension(value: u32, scale: f32) -> u32 {
    let scaled = (value as f64 * scale as f64).round();
    if scaled <= 1.0 {
        return 1;
    }
    scaled.min(u32::MAX as f64) as u32
}

fn scale_clip_property_index(index: &ClipPropertyIndex, scale: f32) -> ClipPropertyIndex {
    let mut clips: HashMap<String, CompiledTransform> = HashMap::with_capacity(index.clips.len());
    for (id, transform) in &index.clips {
        clips.insert(
            id.clone(),
            CompiledTransform {
                x: transform.x * scale,
                y: transform.y * scale,
                width: transform.width.map(|v| v * scale),
                height: transform.height.map(|v| v * scale),
                rotation_degrees: transform.rotation_degrees,
            },
        );
    }

    ClipPropertyIndex {
        canvas_width: index.canvas_width * scale,
        canvas_height: index.canvas_height * scale,
        clips,
    }
}

fn validate_canvas(canvas: &Canvas, scale: f32) -> Result<(), CompileError> {
    if canvas.width == 0 || canvas.height == 0 {
        return Err(CompileError::InvalidCanvas(
            "width and height must be greater than 0".to_string(),
        ));
    }
    let width = (canvas.width as f64 * scale as f64).round();
    let height = (canvas.height as f64 * scale as f64).round();
    if !width.is_finite() || !height.is_finite() {
        return Err(CompileError::InvalidCanvas(
            "scaled canvas width/height must be finite".to_string(),
        ));
    }
    if width <= 0.0 || height <= 0.0 {
        return Err(CompileError::InvalidCanvas(
            "scaled canvas width/height must be greater than 0".to_string(),
        ));
    }
    if width > u32::MAX as f64 || height > u32::MAX as f64 {
        return Err(CompileError::InvalidCanvas(
            "scaled canvas dimensions exceed u32 range".to_string(),
        ));
    }
    Ok(())
}

fn validate_timeline(timeline: &Timeline) -> Result<(), CompileError> {
    if timeline.fps.num == 0 || timeline.fps.den == 0 {
        return Err(CompileError::InvalidTimeline(
            "fps numerator and denominator must be greater than 0".to_string(),
        ));
    }

    if timeline.total_frames == 0 {
        return Err(CompileError::InvalidTimeline(
            "total_frames must be greater than 0".to_string(),
        ));
    }

    Ok(())
}

fn index_sources(sources: &[Source]) -> Result<HashMap<String, Source>, CompileError> {
    let mut seen = HashSet::new();
    let mut map = HashMap::new();

    for source in sources {
        if !seen.insert(source.id.clone()) {
            return Err(CompileError::DuplicateSourceId(source.id.clone()));
        }
        map.insert(source.id.clone(), source.clone());
    }

    Ok(map)
}

fn compile_layer(
    layer: &Layer,
    ctx: &mut CompileContext<'_>,
) -> Result<CompiledLayer, CompileError> {
    let mut items = Vec::with_capacity(layer.items.len());
    for item in &layer.items {
        items.push(compile_layer_item(layer, item, 1, ctx)?);
    }

    Ok(CompiledLayer {
        id: layer.id.clone(),
        z_index: layer.z_index,
        items,
    })
}

fn compile_layer_item(
    layer: &Layer,
    item: &LayerItem,
    depth: usize,
    ctx: &mut CompileContext<'_>,
) -> Result<CompiledLayerItem, CompileError> {
    if depth > ctx.max_depth {
        return Err(CompileError::ItemTreeDepthExceeded {
            layer_id: layer.id.clone(),
            max_depth: ctx.max_depth,
        });
    }

    match item {
        LayerItem::Clip(clip) => {
            let operation = compile_clip_operation(layer, clip, ctx)?;
            let operation_index = ctx.operations.len();
            ctx.operations.push(operation);

            let mask = if let Some(mask) = clip.mask.as_deref() {
                ctx.has_compositing_nodes = true;
                Some(Box::new(compile_layer_item(layer, mask, depth + 1, ctx)?))
            } else {
                None
            };

            Ok(CompiledLayerItem::Clip(CompiledClipNode {
                operation_index,
                mask,
            }))
        }
        LayerItem::Group(group) => {
            validate_group(layer, group)?;
            ctx.has_compositing_nodes = true;

            let mut items = Vec::with_capacity(group.items.len());
            for child in &group.items {
                items.push(compile_layer_item(layer, child, depth + 1, ctx)?);
            }

            let mask = if let Some(mask) = group.mask.as_deref() {
                Some(Box::new(compile_layer_item(layer, mask, depth + 1, ctx)?))
            } else {
                None
            };

            let transform = compile_group_transform(
                &layer.id,
                &group.id,
                &group.transform,
                ctx.scale,
                ctx.clip_index,
            )?;
            if !transform.x.is_finite()
                || !transform.y.is_finite()
                || !transform.rotation_degrees.is_finite()
            {
                return Err(CompileError::InvalidGroup {
                    layer_id: layer.id.clone(),
                    group_id: group.id.clone(),
                    reason: "transform values must be finite".to_string(),
                });
            }

            Ok(CompiledLayerItem::Group(CompiledGroupNode {
                id: group.id.clone(),
                opacity: group.opacity.clamp(0.0, 1.0),
                transform,
                items,
                mask,
            }))
        }
    }
}

fn validate_group(layer: &Layer, group: &ClipGroup) -> Result<(), CompileError> {
    if !group.opacity.is_finite() || group.opacity < 0.0 {
        return Err(CompileError::InvalidGroup {
            layer_id: layer.id.clone(),
            group_id: group.id.clone(),
            reason: "opacity must be a finite number >= 0".to_string(),
        });
    }

    if !group.transform.rotation_degrees.is_finite() {
        return Err(CompileError::InvalidGroup {
            layer_id: layer.id.clone(),
            group_id: group.id.clone(),
            reason: "transform values must be finite".to_string(),
        });
    }

    Ok(())
}

fn compile_clip_operation(
    layer: &Layer,
    clip: &Clip,
    ctx: &CompileContext<'_>,
) -> Result<CompiledOperation, CompileError> {
    if clip.duration_frames == 0 {
        return Err(CompileError::InvalidClip {
            layer_id: layer.id.clone(),
            clip_id: clip.id.clone(),
            reason: "duration_frames must be greater than 0".to_string(),
        });
    }

    if !clip.opacity.is_finite() || clip.opacity < 0.0 {
        return Err(CompileError::InvalidClip {
            layer_id: layer.id.clone(),
            clip_id: clip.id.clone(),
            reason: "opacity must be a finite number >= 0".to_string(),
        });
    }

    if !clip.transform.rotation_degrees.is_finite() {
        return Err(CompileError::InvalidClip {
            layer_id: layer.id.clone(),
            clip_id: clip.id.clone(),
            reason: "transform values must be finite".to_string(),
        });
    }

    let end_frame = clip
        .start_frame
        .checked_add(clip.duration_frames)
        .ok_or_else(|| CompileError::InvalidClip {
            layer_id: layer.id.clone(),
            clip_id: clip.id.clone(),
            reason: "frame range overflow".to_string(),
        })?;

    if end_frame > ctx.total_frames {
        return Err(CompileError::InvalidClip {
            layer_id: layer.id.clone(),
            clip_id: clip.id.clone(),
            reason: format!(
                "clip ends at frame {end_frame} beyond timeline {}",
                ctx.total_frames
            ),
        });
    }

    let animation = compile_clip_animation(
        layer.id.as_str(),
        clip,
        ctx.scale,
        ctx.clip_index,
        ctx.layout_node_ids,
    )?;
    let kind = compile_clip_content(
        layer.id.as_str(),
        clip,
        ctx.sources,
        ctx.scale,
        ctx.clip_index,
    )?;
    let transform = compile_operation_transform(
        &layer.id,
        &clip.id,
        &clip.transform,
        ctx.scale,
        ctx.clip_index,
    )?;

    // Validate compiled transform values.
    if !transform.x.is_finite()
        || !transform.y.is_finite()
        || !transform.rotation_degrees.is_finite()
    {
        return Err(CompileError::InvalidClip {
            layer_id: layer.id.clone(),
            clip_id: clip.id.clone(),
            reason: "transform values must be finite".to_string(),
        });
    }
    if let Some(width) = transform.width {
        if !width.is_finite() || width <= 0.0 {
            return Err(CompileError::InvalidClip {
                layer_id: layer.id.clone(),
                clip_id: clip.id.clone(),
                reason: "transform width must be finite and greater than 0".to_string(),
            });
        }
    }
    if let Some(height) = transform.height {
        if !height.is_finite() || height <= 0.0 {
            return Err(CompileError::InvalidClip {
                layer_id: layer.id.clone(),
                clip_id: clip.id.clone(),
                reason: "transform height must be finite and greater than 0".to_string(),
            });
        }
    }

    Ok(CompiledOperation {
        id: clip.id.clone(),
        layer_id: layer.id.clone(),
        start_frame: clip.start_frame,
        end_frame,
        z_index: layer.z_index,
        opacity: clip.opacity.clamp(0.0, 1.0),
        transform,
        animation,
        kind,
    })
}

fn compile_operation_transform(
    layer_id: &str,
    clip_id: &str,
    transform: &Transform,
    scale: f32,
    clip_index: &ClipPropertyIndex,
) -> Result<CompiledTransform, CompileError> {
    let x = resolve_scalar(layer_id, clip_id, &transform.x, scale, clip_index)?;
    let y = resolve_scalar(layer_id, clip_id, &transform.y, scale, clip_index)?;
    let width = match &transform.width {
        None => None,
        Some(w) => Some(resolve_scalar(layer_id, clip_id, w, scale, clip_index)?),
    };
    let height = match &transform.height {
        None => None,
        Some(h) => Some(resolve_scalar(layer_id, clip_id, h, scale, clip_index)?),
    };
    Ok(CompiledTransform {
        x,
        y,
        width,
        height,
        rotation_degrees: transform.rotation_degrees,
    })
}

fn compile_group_transform(
    layer_id: &str,
    group_id: &str,
    transform: &GroupTransform,
    scale: f32,
    clip_index: &ClipPropertyIndex,
) -> Result<CompiledGroupTransform, CompileError> {
    let x = resolve_scalar(layer_id, group_id, &transform.x, scale, clip_index)?;
    let y = resolve_scalar(layer_id, group_id, &transform.y, scale, clip_index)?;
    Ok(CompiledGroupTransform {
        x,
        y,
        rotation_degrees: transform.rotation_degrees,
    })
}

fn compile_clip_animation(
    layer_id: &str,
    clip: &crate::model::Clip,
    scale: f32,
    clip_index: &ClipPropertyIndex,
    layout_node_ids: &HashSet<String>,
) -> Result<CompiledClipAnimation, CompileError> {
    if clip.transform.width.is_none() && !clip.animation.width.is_empty() {
        return Err(CompileError::InvalidClip {
            layer_id: layer_id.to_string(),
            clip_id: clip.id.clone(),
            reason: "animation.width requires transform.width".to_string(),
        });
    }
    if clip.transform.height.is_none() && !clip.animation.height.is_empty() {
        return Err(CompileError::InvalidClip {
            layer_id: layer_id.to_string(),
            clip_id: clip.id.clone(),
            reason: "animation.height requires transform.height".to_string(),
        });
    }

    Ok(CompiledClipAnimation {
        opacity: compile_scalar_track(
            layer_id,
            clip,
            "animation.opacity",
            clip.animation.opacity.as_slice(),
            1.0,
            clip_index,
            layout_node_ids,
            false,
        )?,
        x: compile_scalar_track(
            layer_id,
            clip,
            "animation.x",
            clip.animation.x.as_slice(),
            scale,
            clip_index,
            layout_node_ids,
            false,
        )?,
        y: compile_scalar_track(
            layer_id,
            clip,
            "animation.y",
            clip.animation.y.as_slice(),
            scale,
            clip_index,
            layout_node_ids,
            false,
        )?,
        width: compile_scalar_track(
            layer_id,
            clip,
            "animation.width",
            clip.animation.width.as_slice(),
            scale,
            clip_index,
            layout_node_ids,
            true,
        )?,
        height: compile_scalar_track(
            layer_id,
            clip,
            "animation.height",
            clip.animation.height.as_slice(),
            scale,
            clip_index,
            layout_node_ids,
            true,
        )?,
        rotation_degrees: compile_scalar_track(
            layer_id,
            clip,
            "animation.rotation_degrees",
            clip.animation.rotation_degrees.as_slice(),
            1.0,
            clip_index,
            layout_node_ids,
            false,
        )?,
    })
}

fn compile_keyframe_scalar_value(
    layer_id: &str,
    clip: &crate::model::Clip,
    field: &str,
    index: usize,
    value: &Scalar,
    scale: f32,
    clip_index: &ClipPropertyIndex,
    layout_node_ids: &HashSet<String>,
) -> Result<CompiledScalarValue, CompileError> {
    match value {
        Scalar::Literal(v) => Ok(CompiledScalarValue::Literal(*v * scale)),
        Scalar::Expr(source) => {
            let parsed = parse_expr(source).map_err(|e| CompileError::ExprError {
                layer_id: layer_id.to_string(),
                clip_id: clip.id.clone(),
                reason: format!("{field}[{index}] {e}"),
            })?;

            let refs = collect_parsed_expr_refs(&parsed);
            let mut has_layout_node_ref = false;
            for r in &refs {
                let target = &r.target;
                if target == "canvas" {
                    if matches!(r.property, ExprProp::Width | ExprProp::Height) {
                        continue;
                    }
                    return Err(CompileError::ExprError {
                        layer_id: layer_id.to_string(),
                        clip_id: clip.id.clone(),
                        reason: format!("{field}[{index}] unknown reference `{}.{}`", target, prop_str(r.property)),
                    });
                }
                if clip_index.clips.contains_key(target.as_str()) {
                    if clip_index.resolve(target, r.property).is_some() {
                        continue;
                    }
                    return Err(CompileError::ExprError {
                        layer_id: layer_id.to_string(),
                        clip_id: clip.id.clone(),
                        reason: format!("{field}[{index}] unknown reference `{}.{}`", target, prop_str(r.property)),
                    });
                }
                if layout_node_ids.contains(target.as_str()) {
                    has_layout_node_ref = true;
                    continue;
                }

                // If the project contains layout nodes, allow unresolved id-like refs
                // to defer to runtime layout-node evaluation.
                if !layout_node_ids.is_empty() {
                    has_layout_node_ref = true;
                    continue;
                }

                return Err(CompileError::ExprError {
                    layer_id: layer_id.to_string(),
                    clip_id: clip.id.clone(),
                    reason: format!("{field}[{index}] unknown reference `{}.{}`", target, prop_str(r.property)),
                });
            }

            if has_layout_node_ref {
                return Ok(CompiledScalarValue::DeferredExpr {
                    parsed: scale_parsed_expr_literals(&parsed, scale),
                });
            }

            eval_expr(&parsed, clip_index)
                .map(|v| CompiledScalarValue::Literal(v * scale))
                .map_err(|e| CompileError::ExprError {
                    layer_id: layer_id.to_string(),
                    clip_id: clip.id.clone(),
                    reason: format!("{field}[{index}] {}", eval_error_reason(&e)),
                })
        }
    }
}

fn compile_scalar_track(
    layer_id: &str,
    clip: &crate::model::Clip,
    field: &str,
    keyframes: &[ScalarKeyframe],
    scale: f32,
    clip_index: &ClipPropertyIndex,
    layout_node_ids: &HashSet<String>,
    require_positive: bool,
) -> Result<Vec<CompiledScalarKeyframe>, CompileError> {
    let mut sorted = keyframes.to_vec();
    sorted.sort_by_key(|keyframe| keyframe.frame);

    let mut compiled = Vec::with_capacity(sorted.len());
    let mut previous_end = 0u64;

    for (index, keyframe) in sorted.into_iter().enumerate() {
        let value = compile_keyframe_scalar_value(
            layer_id,
            clip,
            field,
            index,
            &keyframe.value,
            scale,
            clip_index,
            layout_node_ids,
        )?;
        let resolved_for_validation = match &value {
            CompiledScalarValue::Literal(v) => Some(*v),
            CompiledScalarValue::DeferredExpr { .. } => None,
        };
        if let Some(v) = resolved_for_validation && !v.is_finite() {
            return Err(CompileError::InvalidClip {
                layer_id: layer_id.to_string(),
                clip_id: clip.id.clone(),
                reason: format!("{field}[{index}] value must be finite"),
            });
        }
        if require_positive && resolved_for_validation.is_some_and(|v| v <= 0.0) {
            return Err(CompileError::InvalidClip {
                layer_id: layer_id.to_string(),
                clip_id: clip.id.clone(),
                reason: format!("{field}[{index}] value must be > 0"),
            });
        }
        if keyframe.frame >= clip.duration_frames {
            return Err(CompileError::InvalidClip {
                layer_id: layer_id.to_string(),
                clip_id: clip.id.clone(),
                reason: format!(
                    "{field}[{index}] frame {} is out of clip range {}",
                    keyframe.frame, clip.duration_frames
                ),
            });
        }

        let end = keyframe
            .frame
            .checked_add(keyframe.duration_frames)
            .ok_or_else(|| CompileError::InvalidClip {
                layer_id: layer_id.to_string(),
                clip_id: clip.id.clone(),
                reason: format!("{field}[{index}] frame range overflow"),
            })?;
        if end > clip.duration_frames {
            return Err(CompileError::InvalidClip {
                layer_id: layer_id.to_string(),
                clip_id: clip.id.clone(),
                reason: format!(
                    "{field}[{index}] ends at {} beyond clip duration {}",
                    end, clip.duration_frames
                ),
            });
        }
        if index > 0 && keyframe.frame < previous_end {
            return Err(CompileError::InvalidClip {
                layer_id: layer_id.to_string(),
                clip_id: clip.id.clone(),
                reason: format!("{field} keyframes overlap"),
            });
        }

        previous_end = end.max(keyframe.frame);
        compiled.push(CompiledScalarKeyframe {
            frame: keyframe.frame,
            value,
            duration_frames: keyframe.duration_frames,
            easing: keyframe.easing,
        });
    }

    Ok(compiled)
}

fn compile_clip_content(
    layer_id: &str,
    clip: &crate::model::Clip,
    sources: &HashMap<String, Source>,
    scale: f32,
    clip_index: &ClipPropertyIndex,
) -> Result<CompiledOperationKind, CompileError> {
    match &clip.content {
        ClipContent::Solid { color } => Ok(CompiledOperationKind::Solid { color: *color }),
        ClipContent::Shape(shape) => {
            let mut scaled = shape.clone();
            if let Shape::Rectangle { radius, .. } = &mut scaled.shape {
                *radius = (*radius * scale).max(0.0);
            }
            Ok(CompiledOperationKind::Shape(scaled))
        }
        ClipContent::Text(text) => {
            let mut scaled = text.clone();
            scaled.font_size = (scaled.font_size * scale).max(1.0);
            Ok(CompiledOperationKind::Text(scaled))
        }
        ClipContent::Image(image) => {
            validate_source_type(
                layer_id,
                clip.id.as_str(),
                sources,
                &image.source,
                SourceMediaType::Image,
            )?;
            if !image.corner_radius.is_finite() || image.corner_radius < 0.0 {
                return Err(CompileError::InvalidClip {
                    layer_id: layer_id.to_string(),
                    clip_id: clip.id.clone(),
                    reason: "image corner_radius must be finite and >= 0".to_string(),
                });
            }
            Ok(CompiledOperationKind::Image(ImageSourceRef {
                source_id: image.source.clone(),
                fit: image.fit,
                corner_radius: image.corner_radius * scale,
            }))
        }
        ClipContent::Video(video) => {
            validate_source_type(
                layer_id,
                clip.id.as_str(),
                sources,
                &video.source,
                SourceMediaType::Video,
            )?;
            if !video.corner_radius.is_finite() || video.corner_radius < 0.0 {
                return Err(CompileError::InvalidClip {
                    layer_id: layer_id.to_string(),
                    clip_id: clip.id.clone(),
                    reason: "video corner_radius must be finite and >= 0".to_string(),
                });
            }
            map_source_frame(&video.pipeline, 0).map_err(CompileError::from)?;

            Ok(CompiledOperationKind::Video(VideoSourceRef {
                source_id: video.source.clone(),
                pipeline: video.pipeline.clone(),
                fit: video.fit,
                corner_radius: video.corner_radius * scale,
            }))
        }
        ClipContent::Layout(layout) => {
            if clip.transform.width.is_none() || clip.transform.height.is_none() {
                return Err(CompileError::InvalidClip {
                    layer_id: layer_id.to_string(),
                    clip_id: clip.id.clone(),
                    reason: "layout clips require transform.width and transform.height".to_string(),
                });
            }
            let compiled_layout = compile_layout_clip(
                layer_id,
                clip.id.as_str(),
                layout,
                sources,
                scale,
                clip_index,
            )?;
            Ok(CompiledOperationKind::Layout(compiled_layout))
        }
    }
}

// ── Layout compilation ─────────────────────────────────────────────────────────

fn compile_layout_clip(
    layer_id: &str,
    clip_id: &str,
    layout: &LayoutClip,
    sources: &HashMap<String, Source>,
    scale: f32,
    clip_index: &ClipPropertyIndex,
) -> Result<LayoutClip, CompileError> {
    let node_ids = collect_layout_node_ids(&layout.root);
    validate_layout_node_exprs(layer_id, clip_id, &layout.root, &node_ids)?;

    Ok(LayoutClip {
        root: compile_layout_node(
            layer_id,
            clip_id,
            "root".to_string(),
            &layout.root,
            sources,
            scale,
            clip_index,
            &node_ids,
        )?,
    })
}

fn compile_layout_node(
    layer_id: &str,
    clip_id: &str,
    node_path: String,
    node: &LayoutNode,
    sources: &HashMap<String, Source>,
    scale: f32,
    clip_index: &ClipPropertyIndex,
    node_ids: &HashSet<String>,
) -> Result<LayoutNode, CompileError> {
    let style = compile_layout_style(
        layer_id,
        clip_id,
        node_path.as_str(),
        &node.style,
        scale,
        clip_index,
        node_ids,
    )?;

    let kind = match &node.kind {
        LayoutNodeKind::Container { children } => {
            let mut compiled_children = Vec::with_capacity(children.len());
            for (index, child) in children.iter().enumerate() {
                compiled_children.push(compile_layout_node(
                    layer_id,
                    clip_id,
                    format!("{node_path}.children[{index}]"),
                    child,
                    sources,
                    scale,
                    clip_index,
                    node_ids,
                )?);
            }
            LayoutNodeKind::Container {
                children: compiled_children,
            }
        }
        LayoutNodeKind::Text(text) => LayoutNodeKind::Text(compile_layout_text_node(
            layer_id,
            clip_id,
            node_path.as_str(),
            text,
            scale,
        )?),
        LayoutNodeKind::Image(image) => LayoutNodeKind::Image(compile_layout_image_node(
            layer_id,
            clip_id,
            node_path.as_str(),
            image,
            sources,
            scale,
        )?),
    };

    Ok(LayoutNode {
        id: node.id.clone(),
        style,
        kind,
    })
}

fn compile_layout_text_node(
    layer_id: &str,
    clip_id: &str,
    node_path: &str,
    text: &LayoutTextNode,
    scale: f32,
) -> Result<LayoutTextNode, CompileError> {
    if !text.font_size.is_finite() || text.font_size <= 0.0 {
        return Err(CompileError::InvalidClip {
            layer_id: layer_id.to_string(),
            clip_id: clip_id.to_string(),
            reason: format!("{node_path}.font_size must be finite and > 0"),
        });
    }

    if let Some(line_height) = text.line_height
        && (!line_height.is_finite() || line_height <= 0.0)
    {
        return Err(CompileError::InvalidClip {
            layer_id: layer_id.to_string(),
            clip_id: clip_id.to_string(),
            reason: format!("{node_path}.line_height must be finite and > 0"),
        });
    }

    Ok(LayoutTextNode {
        text: text.text.clone(),
        font_size: (text.font_size * scale).max(1.0),
        color: text.color,
        align: text.align,
        line_height: text.line_height.map(|value| value * scale),
    })
}

fn compile_layout_image_node(
    layer_id: &str,
    clip_id: &str,
    node_path: &str,
    image: &LayoutImageNode,
    sources: &HashMap<String, Source>,
    scale: f32,
) -> Result<LayoutImageNode, CompileError> {
    validate_source_type(
        layer_id,
        clip_id,
        sources,
        image.source.as_str(),
        SourceMediaType::Image,
    )?;
    if !image.corner_radius.is_finite() || image.corner_radius < 0.0 {
        return Err(CompileError::InvalidClip {
            layer_id: layer_id.to_string(),
            clip_id: clip_id.to_string(),
            reason: format!("{node_path}.corner_radius must be finite and >= 0"),
        });
    }

    Ok(LayoutImageNode {
        source: image.source.clone(),
        fit: image.fit,
        corner_radius: image.corner_radius * scale,
    })
}

fn compile_layout_style(
    layer_id: &str,
    clip_id: &str,
    node_path: &str,
    style: &LayoutNodeStyle,
    scale: f32,
    clip_index: &ClipPropertyIndex,
    node_ids: &HashSet<String>,
) -> Result<LayoutNodeStyle, CompileError> {
    if !style.flex_grow.is_finite() || style.flex_grow < 0.0 {
        return Err(CompileError::InvalidClip {
            layer_id: layer_id.to_string(),
            clip_id: clip_id.to_string(),
            reason: format!("{node_path}.style.flex_grow must be finite and >= 0"),
        });
    }
    if !style.flex_shrink.is_finite() || style.flex_shrink < 0.0 {
        return Err(CompileError::InvalidClip {
            layer_id: layer_id.to_string(),
            clip_id: clip_id.to_string(),
            reason: format!("{node_path}.style.flex_shrink must be finite and >= 0"),
        });
    }
    if !style.gap.is_finite() || style.gap < 0.0 {
        return Err(CompileError::InvalidClip {
            layer_id: layer_id.to_string(),
            clip_id: clip_id.to_string(),
            reason: format!("{node_path}.style.gap must be finite and >= 0"),
        });
    }
    if !style.corner_radius.is_finite() || style.corner_radius < 0.0 {
        return Err(CompileError::InvalidClip {
            layer_id: layer_id.to_string(),
            clip_id: clip_id.to_string(),
            reason: format!("{node_path}.style.corner_radius must be finite and >= 0"),
        });
    }

    let width = compile_optional_layout_dimension(
        layer_id,
        clip_id,
        node_path,
        "style.width",
        style.width.clone(),
        scale,
        clip_index,
        node_ids,
    )?;
    let height = compile_optional_layout_dimension(
        layer_id,
        clip_id,
        node_path,
        "style.height",
        style.height.clone(),
        scale,
        clip_index,
        node_ids,
    )?;
    let min_width = compile_optional_layout_dimension(
        layer_id,
        clip_id,
        node_path,
        "style.min_width",
        style.min_width.clone(),
        scale,
        clip_index,
        node_ids,
    )?;
    let min_height = compile_optional_layout_dimension(
        layer_id,
        clip_id,
        node_path,
        "style.min_height",
        style.min_height.clone(),
        scale,
        clip_index,
        node_ids,
    )?;
    let max_width = compile_optional_layout_dimension(
        layer_id,
        clip_id,
        node_path,
        "style.max_width",
        style.max_width.clone(),
        scale,
        clip_index,
        node_ids,
    )?;
    let max_height = compile_optional_layout_dimension(
        layer_id,
        clip_id,
        node_path,
        "style.max_height",
        style.max_height.clone(),
        scale,
        clip_index,
        node_ids,
    )?;

    let padding = compile_layout_edges(
        layer_id,
        clip_id,
        node_path,
        "style.padding",
        style.padding,
        scale,
    )?;
    let margin = compile_layout_edges(
        layer_id,
        clip_id,
        node_path,
        "style.margin",
        style.margin,
        scale,
    )?;

    Ok(LayoutNodeStyle {
        display: style.display,
        flex_direction: style.flex_direction,
        justify_content: style.justify_content,
        align_items: style.align_items,
        align_self: style.align_self,
        overflow: style.overflow,
        flex_grow: style.flex_grow,
        flex_shrink: style.flex_shrink,
        width,
        height,
        min_width,
        min_height,
        max_width,
        max_height,
        padding,
        margin,
        gap: style.gap * scale,
        background: style.background,
        corner_radius: style.corner_radius * scale,
    })
}

fn compile_optional_layout_dimension(
    layer_id: &str,
    clip_id: &str,
    node_path: &str,
    field: &str,
    value: Option<Scalar>,
    scale: f32,
    clip_index: &ClipPropertyIndex,
    node_ids: &HashSet<String>,
) -> Result<Option<Scalar>, CompileError> {
    let Some(scalar) = value else {
        return Ok(None);
    };

    match scalar {
        Scalar::Literal(v) => {
            if !v.is_finite() || v < 0.0 {
                return Err(CompileError::InvalidClip {
                    layer_id: layer_id.to_string(),
                    clip_id: clip_id.to_string(),
                    reason: format!("{node_path}.{field} must be finite and >= 0"),
                });
            }
            Ok(Some(Scalar::Literal(v * scale)))
        }
        Scalar::Expr(s) => {
            let parsed = parse_expr(&s).map_err(|e| CompileError::ExprError {
                layer_id: layer_id.to_string(),
                clip_id: clip_id.to_string(),
                reason: e.to_string(),
            })?;

            // Classify all ref targets.
            let refs = collect_parsed_expr_refs(&parsed);
            let mut has_node_ref = false;
            for r in &refs {
                let target = &r.target;
                if target == "canvas" {
                    // canvas.width/height are valid; canvas.x/y would fail at eval.
                    // Just let eval handle it.
                    continue;
                }
                if clip_index.clips.contains_key(target.as_str()) {
                    continue;
                }
                if node_ids.contains(target.as_str()) {
                    has_node_ref = true;
                    continue;
                }
                // Unknown reference.
                return Err(CompileError::ExprError {
                    layer_id: layer_id.to_string(),
                    clip_id: clip_id.to_string(),
                    reason: format!("unknown reference `{}.{}`", target, prop_str(r.property)),
                });
            }

            if has_node_ref {
                let scaled = scale_parsed_expr_literals(&parsed, scale);
                return Ok(Some(Scalar::Expr(scaled.to_string())));
            }

            // All refs are canvas or clip refs — resolve now.
            let resolved = eval_expr(&parsed, clip_index).map_err(|e| CompileError::ExprError {
                layer_id: layer_id.to_string(),
                clip_id: clip_id.to_string(),
                reason: eval_error_reason(&e),
            })?;
            Ok(Some(Scalar::Literal(resolved * scale)))
        }
    }
}

/// Recursively collect all ExprRef nodes from a parsed expression.
fn collect_parsed_expr_refs(expr: &ParsedExpr) -> Vec<&crate::expr::ExprRef> {
    match expr {
        ParsedExpr::Literal(_) => vec![],
        ParsedExpr::Ref(r) => vec![r],
        ParsedExpr::UnaryOp { expr, .. } => collect_parsed_expr_refs(expr),
        ParsedExpr::BinOp { lhs, rhs, .. } => {
            let mut refs = collect_parsed_expr_refs(lhs);
            refs.extend(collect_parsed_expr_refs(rhs));
            refs
        }
    }
}

fn scale_parsed_expr_literals(expr: &ParsedExpr, scale: f32) -> ParsedExpr {
    match expr {
        ParsedExpr::Literal(v) => ParsedExpr::Literal(v * scale),
        ParsedExpr::Ref(r) => ParsedExpr::Ref(r.clone()),
        ParsedExpr::UnaryOp { op, expr } => ParsedExpr::UnaryOp {
            op: *op,
            expr: Box::new(scale_parsed_expr_literals(expr, scale)),
        },
        ParsedExpr::BinOp { op, lhs, rhs } => ParsedExpr::BinOp {
            op: *op,
            lhs: Box::new(scale_parsed_expr_literals(lhs, scale)),
            rhs: Box::new(scale_parsed_expr_literals(rhs, scale)),
        },
    }
}

fn compile_layout_edges(
    layer_id: &str,
    clip_id: &str,
    node_path: &str,
    field: &str,
    edges: LayoutEdges,
    scale: f32,
) -> Result<LayoutEdges, CompileError> {
    Ok(LayoutEdges {
        top: compile_layout_edge(layer_id, clip_id, node_path, field, "top", edges.top, scale)?,
        right: compile_layout_edge(
            layer_id,
            clip_id,
            node_path,
            field,
            "right",
            edges.right,
            scale,
        )?,
        bottom: compile_layout_edge(
            layer_id,
            clip_id,
            node_path,
            field,
            "bottom",
            edges.bottom,
            scale,
        )?,
        left: compile_layout_edge(
            layer_id, clip_id, node_path, field, "left", edges.left, scale,
        )?,
    })
}

fn compile_layout_edge(
    layer_id: &str,
    clip_id: &str,
    node_path: &str,
    field: &str,
    edge_name: &str,
    value: f32,
    scale: f32,
) -> Result<f32, CompileError> {
    if !value.is_finite() || value < 0.0 {
        return Err(CompileError::InvalidClip {
            layer_id: layer_id.to_string(),
            clip_id: clip_id.to_string(),
            reason: format!("{node_path}.{field}.{edge_name} must be finite and >= 0"),
        });
    }
    Ok(value * scale)
}

// ── Layout node ID collection and cycle detection ─────────────────────────────

fn collect_layout_node_ids(node: &LayoutNode) -> HashSet<String> {
    let mut ids = HashSet::new();
    collect_layout_node_ids_inner(node, &mut ids);
    ids
}

fn collect_layout_node_ids_inner(node: &LayoutNode, ids: &mut HashSet<String>) {
    if let Some(id) = &node.id {
        ids.insert(id.clone());
    }
    if let LayoutNodeKind::Container { children } = &node.kind {
        for child in children {
            collect_layout_node_ids_inner(child, ids);
        }
    }
}

fn validate_layout_node_exprs(
    layer_id: &str,
    clip_id: &str,
    root: &LayoutNode,
    node_ids: &HashSet<String>,
) -> Result<(), CompileError> {
    let mut adj: HashMap<String, HashSet<String>> = HashMap::new();
    build_layout_dim_dep_graph(root, node_ids, &mut adj);

    let mut visited: HashSet<String> = HashSet::new();
    let mut in_stack: HashSet<String> = HashSet::new();

    for node_id in node_ids {
        if !visited.contains(node_id) {
            if layout_has_cycle(node_id, &adj, &mut visited, &mut in_stack) {
                return Err(CompileError::ExprError {
                    layer_id: layer_id.to_string(),
                    clip_id: clip_id.to_string(),
                    reason: format!("layout expression cycle at node `{node_id}`"),
                });
            }
        }
    }
    Ok(())
}

fn build_layout_dim_dep_graph(
    node: &LayoutNode,
    node_ids: &HashSet<String>,
    adj: &mut HashMap<String, HashSet<String>>,
) {
    if let Some(id) = &node.id {
        let mut deps: HashSet<String> = HashSet::new();
        let dim_fields = [
            node.style.width.as_ref(),
            node.style.height.as_ref(),
            node.style.min_width.as_ref(),
            node.style.min_height.as_ref(),
            node.style.max_width.as_ref(),
            node.style.max_height.as_ref(),
        ];
        for scalar in dim_fields.into_iter().flatten() {
            if let Scalar::Expr(s) = scalar {
                if let Ok(parsed) = parse_expr(s) {
                    collect_layout_node_deps(&parsed, node_ids, &mut deps);
                }
            }
        }
        adj.insert(id.clone(), deps);
    }

    if let LayoutNodeKind::Container { children } = &node.kind {
        for child in children {
            build_layout_dim_dep_graph(child, node_ids, adj);
        }
    }
}

fn collect_layout_node_deps(
    expr: &ParsedExpr,
    node_ids: &HashSet<String>,
    deps: &mut HashSet<String>,
) {
    match expr {
        ParsedExpr::Literal(_) => {}
        ParsedExpr::Ref(r) => {
            if node_ids.contains(&r.target) {
                deps.insert(r.target.clone());
            }
        }
        ParsedExpr::UnaryOp { expr, .. } => {
            collect_layout_node_deps(expr, node_ids, deps);
        }
        ParsedExpr::BinOp { lhs, rhs, .. } => {
            collect_layout_node_deps(lhs, node_ids, deps);
            collect_layout_node_deps(rhs, node_ids, deps);
        }
    }
}

fn layout_has_cycle(
    node: &str,
    adj: &HashMap<String, HashSet<String>>,
    visited: &mut HashSet<String>,
    in_stack: &mut HashSet<String>,
) -> bool {
    visited.insert(node.to_string());
    in_stack.insert(node.to_string());

    if let Some(deps) = adj.get(node) {
        for dep in deps {
            if in_stack.contains(dep.as_str()) {
                return true;
            }
            if !visited.contains(dep.as_str()) && layout_has_cycle(dep, adj, visited, in_stack) {
                return true;
            }
        }
    }

    in_stack.remove(node);
    false
}

// ── Source validation ──────────────────────────────────────────────────────────

fn validate_source_type(
    layer_id: &str,
    clip_id: &str,
    sources: &HashMap<String, Source>,
    source_id: &str,
    expected: SourceMediaType,
) -> Result<(), CompileError> {
    let source = sources
        .get(source_id)
        .ok_or_else(|| CompileError::MissingSource(source_id.to_string()))?;

    let found = source.media_type();
    if found != expected {
        return Err(CompileError::SourceTypeMismatch {
            source_id: source_id.to_string(),
            expected,
            found,
        });
    }

    if let SourceMediaType::Video = expected {
        if matches!(source.kind, crate::model::SourceKind::Generator { media, .. } if media == SourceMediaType::Audio)
        {
            return Err(CompileError::InvalidClip {
                layer_id: layer_id.to_string(),
                clip_id: clip_id.to_string(),
                reason: "video clip cannot use audio generator source".to_string(),
            });
        }
    }

    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::{
        compile::{
            CompileError, CompiledOperationKind, compile_project, compile_project_with_scale,
        },
        expr::Scalar,
        model::{
            Canvas, Clip, ClipAnimation, ClipContent, ClipGroup, ColorRgba, Easing, Layer,
            LayerItem, LayoutClip, LayoutNode, LayoutNodeKind, LayoutNodeStyle, LayoutTextNode,
            Project, ScalarKeyframe, Source, SourceKind, SourceMediaType, TextClip, Timeline,
            Transform, VideoClip,
        },
        time::Rational,
    };

    #[test]
    fn compiles_basic_project() {
        let project = Project {
            canvas: Canvas {
                width: 640,
                height: 360,
                background: ColorRgba(0, 0, 0, 255),
            },
            timeline: Timeline {
                fps: Rational::new(30, 1).expect("fps"),
                total_frames: 30,
            },
            sources: vec![Source {
                id: "video_1".to_string(),
                kind: SourceKind::File {
                    media: SourceMediaType::Video,
                    path: "video.mp4".to_string(),
                },
            }],
            layers: vec![Layer {
                id: "layer_a".to_string(),
                z_index: 1,
                items: vec![LayerItem::Clip(Clip {
                    id: "clip_a".to_string(),
                    start_frame: 0,
                    duration_frames: 30,
                    opacity: 1.0,
                    transform: Default::default(),
                    animation: Default::default(),
                    mask: None,
                    content: ClipContent::Video(VideoClip {
                        source: "video_1".to_string(),
                        pipeline: Default::default(),
                        fit: Default::default(),
                        corner_radius: 0.0,
                    }),
                })],
            }],
            audio: Default::default(),
        };

        let compiled = compile_project(&project).expect("compile");
        let frame_ops = compiled.operation_indices_for_frame(0).expect("frame ops");
        assert_eq!(frame_ops.len(), 1);

        let op = compiled.operation(frame_ops[0]).expect("op");
        assert!(matches!(op.kind, CompiledOperationKind::Video(_)));
        assert_eq!(op.resolve_video_source_frame(0).expect("resolve"), Some(0));
    }

    #[test]
    fn rejects_incompatible_source_type() {
        let project = Project {
            canvas: Canvas {
                width: 640,
                height: 360,
                background: ColorRgba(0, 0, 0, 255),
            },
            timeline: Timeline {
                fps: Rational::new(30, 1).expect("fps"),
                total_frames: 30,
            },
            sources: vec![Source {
                id: "image_1".to_string(),
                kind: SourceKind::File {
                    media: SourceMediaType::Image,
                    path: "image.png".to_string(),
                },
            }],
            layers: vec![Layer {
                id: "layer_a".to_string(),
                z_index: 0,
                items: vec![LayerItem::Clip(Clip {
                    id: "clip_a".to_string(),
                    start_frame: 0,
                    duration_frames: 10,
                    opacity: 1.0,
                    transform: Default::default(),
                    animation: Default::default(),
                    mask: None,
                    content: ClipContent::Video(VideoClip {
                        source: "image_1".to_string(),
                        pipeline: Default::default(),
                        fit: Default::default(),
                        corner_radius: 0.0,
                    }),
                })],
            }],
            audio: Default::default(),
        };

        let err = compile_project(&project).expect_err("must fail");
        assert!(err.to_string().contains("incompatible media type"));
    }

    #[test]
    fn rejects_out_of_range_clip() {
        let project = Project {
            canvas: Canvas {
                width: 640,
                height: 360,
                background: ColorRgba(0, 0, 0, 255),
            },
            timeline: Timeline {
                fps: Rational::new(30, 1).expect("fps"),
                total_frames: 10,
            },
            sources: vec![],
            layers: vec![Layer {
                id: "layer_a".to_string(),
                z_index: 0,
                items: vec![LayerItem::Clip(Clip {
                    id: "clip_a".to_string(),
                    start_frame: 8,
                    duration_frames: 4,
                    opacity: 1.0,
                    transform: Default::default(),
                    animation: Default::default(),
                    mask: None,
                    content: ClipContent::Text(TextClip {
                        text: "hello".to_string(),
                        font_size: 20.0,
                        color: ColorRgba(255, 255, 255, 255),
                        align: Default::default(),
                    }),
                })],
            }],
            audio: Default::default(),
        };

        let err = compile_project(&project).expect_err("must fail");
        assert!(err.to_string().contains("beyond timeline"));
    }

    #[test]
    fn resolves_clip_animation_with_easing_and_duration() {
        let project = Project {
            canvas: Canvas {
                width: 640,
                height: 360,
                background: ColorRgba(0, 0, 0, 255),
            },
            timeline: Timeline {
                fps: Rational::new(30, 1).expect("fps"),
                total_frames: 30,
            },
            sources: vec![],
            layers: vec![Layer {
                id: "layer_a".to_string(),
                z_index: 0,
                items: vec![LayerItem::Clip(Clip {
                    id: "clip_a".to_string(),
                    start_frame: 0,
                    duration_frames: 30,
                    opacity: 0.0,
                    transform: Default::default(),
                    animation: ClipAnimation {
                        opacity: vec![ScalarKeyframe {
                            frame: 0,
                            value: Scalar::Literal(1.0),
                            duration_frames: 10,
                            easing: Easing::EaseOut,
                        }],
                        ..Default::default()
                    },
                    mask: None,
                    content: ClipContent::Text(TextClip {
                        text: "hello".to_string(),
                        font_size: 20.0,
                        color: ColorRgba(255, 255, 255, 255),
                        align: Default::default(),
                    }),
                })],
            }],
            audio: Default::default(),
        };

        let compiled = compile_project(&project).expect("compile");
        let frame_ops = compiled.operation_indices_for_frame(5).expect("frame ops");
        let op = compiled.operation(frame_ops[0]).expect("op");
        let midpoint = op.resolved_opacity(5);
        assert!(midpoint > 0.0 && midpoint < 1.0);
        assert_eq!(op.resolved_opacity(10), 1.0);
    }

    #[test]
    fn rejects_overlapping_animation_keyframes() {
        let project = Project {
            canvas: Canvas {
                width: 640,
                height: 360,
                background: ColorRgba(0, 0, 0, 255),
            },
            timeline: Timeline {
                fps: Rational::new(30, 1).expect("fps"),
                total_frames: 30,
            },
            sources: vec![],
            layers: vec![Layer {
                id: "layer_a".to_string(),
                z_index: 0,
                items: vec![LayerItem::Clip(Clip {
                    id: "clip_a".to_string(),
                    start_frame: 0,
                    duration_frames: 20,
                    opacity: 1.0,
                    transform: Default::default(),
                    animation: ClipAnimation {
                        y: vec![
                            ScalarKeyframe {
                                frame: 2,
                                value: Scalar::Literal(10.0),
                                duration_frames: 8,
                                easing: Easing::EaseOut,
                            },
                            ScalarKeyframe {
                                frame: 6,
                                value: Scalar::Literal(20.0),
                                duration_frames: 8,
                                easing: Easing::EaseIn,
                            },
                        ],
                        ..Default::default()
                    },
                    mask: None,
                    content: ClipContent::Text(TextClip {
                        text: "hello".to_string(),
                        font_size: 20.0,
                        color: ColorRgba(255, 255, 255, 255),
                        align: Default::default(),
                    }),
                })],
            }],
            audio: Default::default(),
        };

        let err = compile_project(&project).expect_err("must fail");
        assert!(err.to_string().contains("keyframes overlap"));
    }

    #[test]
    fn rejects_layout_clip_without_dimensions() {
        let project = Project {
            canvas: Canvas {
                width: 640,
                height: 360,
                background: ColorRgba(0, 0, 0, 255),
            },
            timeline: Timeline {
                fps: Rational::new(30, 1).expect("fps"),
                total_frames: 30,
            },
            sources: vec![],
            layers: vec![Layer {
                id: "layer_a".to_string(),
                z_index: 0,
                items: vec![LayerItem::Clip(Clip {
                    id: "clip_layout".to_string(),
                    start_frame: 0,
                    duration_frames: 30,
                    opacity: 1.0,
                    transform: Transform {
                        x: Scalar::Literal(0.0),
                        y: Scalar::Literal(0.0),
                        width: None,
                        height: Some(Scalar::Literal(180.0)),
                        rotation_degrees: 0.0,
                    },
                    animation: Default::default(),
                    mask: None,
                    content: ClipContent::Layout(LayoutClip {
                        root: LayoutNode {
                            id: None,
                            style: Default::default(),
                            kind: LayoutNodeKind::Container { children: vec![] },
                        },
                    }),
                })],
            }],
            audio: Default::default(),
        };

        let err = compile_project(&project).expect_err("must fail");
        assert!(
            err.to_string()
                .contains("layout clips require transform.width and transform.height")
        );
    }

    #[test]
    fn compiles_layout_clip_and_scales_text() {
        let project = Project {
            canvas: Canvas {
                width: 640,
                height: 360,
                background: ColorRgba(0, 0, 0, 255),
            },
            timeline: Timeline {
                fps: Rational::new(30, 1).expect("fps"),
                total_frames: 30,
            },
            sources: vec![],
            layers: vec![Layer {
                id: "layer_a".to_string(),
                z_index: 0,
                items: vec![LayerItem::Clip(Clip {
                    id: "clip_layout".to_string(),
                    start_frame: 0,
                    duration_frames: 30,
                    opacity: 1.0,
                    transform: Transform {
                        x: Scalar::Literal(10.0),
                        y: Scalar::Literal(20.0),
                        width: Some(Scalar::Literal(300.0)),
                        height: Some(Scalar::Literal(180.0)),
                        rotation_degrees: 0.0,
                    },
                    animation: Default::default(),
                    mask: None,
                    content: ClipContent::Layout(LayoutClip {
                        root: LayoutNode {
                            id: None,
                            style: Default::default(),
                            kind: LayoutNodeKind::Container {
                                children: vec![LayoutNode {
                                    id: None,
                                    style: Default::default(),
                                    kind: LayoutNodeKind::Text(LayoutTextNode {
                                        text: "hello".to_string(),
                                        font_size: 10.0,
                                        color: ColorRgba(255, 255, 255, 255),
                                        align: Default::default(),
                                        line_height: None,
                                    }),
                                }],
                            },
                        },
                    }),
                })],
            }],
            audio: Default::default(),
        };

        let compiled = compile_project_with_scale(&project, 2.0).expect("compile");
        let frame_ops = compiled.operation_indices_for_frame(0).expect("frame ops");
        assert_eq!(frame_ops.len(), 1);
        let operation = compiled.operation(frame_ops[0]).expect("operation");
        match &operation.kind {
            CompiledOperationKind::Layout(layout) => {
                let LayoutNodeKind::Container { children } = &layout.root.kind else {
                    panic!("expected container root");
                };
                let LayoutNodeKind::Text(text) = &children[0].kind else {
                    panic!("expected text child");
                };
                assert_eq!(text.font_size, 20.0);
            }
            _ => panic!("expected layout operation"),
        }
    }

    #[test]
    fn rejects_item_tree_depth_over_limit() {
        let mut root = LayerItem::Clip(Clip {
            id: "clip_root".to_string(),
            start_frame: 0,
            duration_frames: 30,
            opacity: 1.0,
            transform: Default::default(),
            animation: Default::default(),
            mask: None,
            content: ClipContent::Text(TextClip {
                text: "hello".to_string(),
                font_size: 20.0,
                color: ColorRgba(255, 255, 255, 255),
                align: Default::default(),
            }),
        });

        for depth in 0..16 {
            root = LayerItem::Group(ClipGroup {
                id: format!("group_{depth}"),
                opacity: 1.0,
                transform: Default::default(),
                items: vec![root],
                mask: None,
            });
        }

        let project = Project {
            canvas: Canvas {
                width: 640,
                height: 360,
                background: ColorRgba(0, 0, 0, 255),
            },
            timeline: Timeline {
                fps: Rational::new(30, 1).expect("fps"),
                total_frames: 30,
            },
            sources: vec![],
            layers: vec![Layer {
                id: "layer_a".to_string(),
                z_index: 0,
                items: vec![root],
            }],
            audio: Default::default(),
        };

        let err = compile_project(&project).expect_err("must fail");
        assert!(err.to_string().contains("item tree exceeds max depth"));
    }
    #[test]
    fn canvas_width_expr_resolves_in_transform() {
        let project = Project {
            canvas: Canvas {
                width: 640,
                height: 360,
                background: ColorRgba(0, 0, 0, 255),
            },
            timeline: Timeline {
                fps: Rational::new(30, 1).expect("fps"),
                total_frames: 30,
            },
            sources: vec![],
            layers: vec![Layer {
                id: "layer_a".to_string(),
                z_index: 0,
                items: vec![LayerItem::Clip(Clip {
                    id: "clip_a".to_string(),
                    start_frame: 0,
                    duration_frames: 30,
                    opacity: 1.0,
                    transform: Transform {
                        x: Scalar::Expr("canvas.width".to_string()),
                        y: Scalar::Literal(0.0),
                        width: None,
                        height: None,
                        rotation_degrees: 0.0,
                    },
                    animation: Default::default(),
                    mask: None,
                    content: ClipContent::Text(TextClip {
                        text: "hello".to_string(),
                        font_size: 20.0,
                        color: ColorRgba(255, 255, 255, 255),
                        align: Default::default(),
                    }),
                })],
            }],
            audio: Default::default(),
        };

        let compiled = compile_project(&project).expect("compile");
        let frame_ops = compiled.operation_indices_for_frame(0).expect("frame ops");
        let op = compiled.operation(frame_ops[0]).expect("op");
        assert_eq!(op.resolved_transform(0).x, 640.0);
    }

    #[test]
    fn cross_clip_expr_resolves_in_transform() {
        let project = Project {
            canvas: Canvas {
                width: 640,
                height: 360,
                background: ColorRgba(0, 0, 0, 255),
            },
            timeline: Timeline {
                fps: Rational::new(30, 1).expect("fps"),
                total_frames: 30,
            },
            sources: vec![],
            layers: vec![Layer {
                id: "layer_a".to_string(),
                z_index: 0,
                items: vec![
                    LayerItem::Clip(Clip {
                        id: "clip_a".to_string(),
                        start_frame: 0,
                        duration_frames: 30,
                        opacity: 1.0,
                        transform: Transform {
                            x: Scalar::Literal(100.0),
                            y: Scalar::Literal(0.0),
                            width: Some(Scalar::Literal(200.0)),
                            height: None,
                            rotation_degrees: 0.0,
                        },
                        animation: Default::default(),
                        mask: None,
                        content: ClipContent::Text(TextClip {
                            text: "a".to_string(),
                            font_size: 20.0,
                            color: ColorRgba(255, 255, 255, 255),
                            align: Default::default(),
                        }),
                    }),
                    LayerItem::Clip(Clip {
                        id: "clip_b".to_string(),
                        start_frame: 0,
                        duration_frames: 30,
                        opacity: 1.0,
                        transform: Transform {
                            x: Scalar::Expr("clip_a.x + clip_a.width".to_string()),
                            y: Scalar::Literal(0.0),
                            width: None,
                            height: None,
                            rotation_degrees: 0.0,
                        },
                        animation: Default::default(),
                        mask: None,
                        content: ClipContent::Text(TextClip {
                            text: "b".to_string(),
                            font_size: 20.0,
                            color: ColorRgba(255, 255, 255, 255),
                            align: Default::default(),
                        }),
                    }),
                ],
            }],
            audio: Default::default(),
        };

        let compiled = compile_project(&project).expect("compile");
        let frame_ops = compiled.operation_indices_for_frame(0).expect("frame ops");
        assert_eq!(frame_ops.len(), 2);
        let op_b = compiled.operation(frame_ops[1]).expect("op_b");
        assert_eq!(op_b.resolved_transform(0).x, 300.0);
    }

    #[test]
    fn transform_expr_scales_once_with_compile_scale() {
        let project = Project {
            canvas: Canvas {
                width: 640,
                height: 360,
                background: ColorRgba(0, 0, 0, 255),
            },
            timeline: Timeline {
                fps: Rational::new(30, 1).expect("fps"),
                total_frames: 30,
            },
            sources: vec![],
            layers: vec![Layer {
                id: "layer_a".to_string(),
                z_index: 0,
                items: vec![LayerItem::Clip(Clip {
                    id: "clip_a".to_string(),
                    start_frame: 0,
                    duration_frames: 30,
                    opacity: 1.0,
                    transform: Transform {
                        x: Scalar::Expr("canvas.width / 2".to_string()),
                        y: Scalar::Literal(0.0),
                        width: None,
                        height: None,
                        rotation_degrees: 0.0,
                    },
                    animation: Default::default(),
                    mask: None,
                    content: ClipContent::Text(TextClip {
                        text: "hello".to_string(),
                        font_size: 20.0,
                        color: ColorRgba(255, 255, 255, 255),
                        align: Default::default(),
                    }),
                })],
            }],
            audio: Default::default(),
        };

        let compiled = compile_project_with_scale(&project, 2.0).expect("compile");
        let frame_ops = compiled.operation_indices_for_frame(0).expect("frame ops");
        let op = compiled.operation(frame_ops[0]).expect("op");
        assert_eq!(op.resolved_transform(0).x, 640.0);
    }

    #[test]
    fn keyframe_expr_resolves_and_scales_once() {
        let project = Project {
            canvas: Canvas {
                width: 640,
                height: 360,
                background: ColorRgba(0, 0, 0, 255),
            },
            timeline: Timeline {
                fps: Rational::new(30, 1).expect("fps"),
                total_frames: 30,
            },
            sources: vec![],
            layers: vec![Layer {
                id: "layer_a".to_string(),
                z_index: 0,
                items: vec![LayerItem::Clip(Clip {
                    id: "clip_a".to_string(),
                    start_frame: 0,
                    duration_frames: 30,
                    opacity: 1.0,
                    transform: Transform {
                        x: Scalar::Literal(0.0),
                        y: Scalar::Literal(0.0),
                        width: Some(Scalar::Literal(100.0)),
                        height: None,
                        rotation_degrees: 0.0,
                    },
                    animation: ClipAnimation {
                        width: vec![ScalarKeyframe {
                            frame: 0,
                            value: Scalar::Expr("(canvas.width / 4) + 10".to_string()),
                            duration_frames: 0,
                            easing: Easing::Linear,
                        }],
                        ..Default::default()
                    },
                    mask: None,
                    content: ClipContent::Text(TextClip {
                        text: "hello".to_string(),
                        font_size: 20.0,
                        color: ColorRgba(255, 255, 255, 255),
                        align: Default::default(),
                    }),
                })],
            }],
            audio: Default::default(),
        };

        let compiled = compile_project_with_scale(&project, 2.0).expect("compile");
        let frame_ops = compiled.operation_indices_for_frame(0).expect("frame ops");
        let op = compiled.operation(frame_ops[0]).expect("op");
        assert_eq!(op.resolved_transform(0).width, Some(340.0));
    }

    #[test]
    fn keyframe_expr_unknown_ref_errors() {
        let project = Project {
            canvas: Canvas {
                width: 640,
                height: 360,
                background: ColorRgba(0, 0, 0, 255),
            },
            timeline: Timeline {
                fps: Rational::new(30, 1).expect("fps"),
                total_frames: 30,
            },
            sources: vec![],
            layers: vec![Layer {
                id: "layer_a".to_string(),
                z_index: 0,
                items: vec![LayerItem::Clip(Clip {
                    id: "clip_a".to_string(),
                    start_frame: 0,
                    duration_frames: 30,
                    opacity: 1.0,
                    transform: Transform {
                        x: Scalar::Literal(0.0),
                        y: Scalar::Literal(0.0),
                        width: Some(Scalar::Literal(100.0)),
                        height: None,
                        rotation_degrees: 0.0,
                    },
                    animation: ClipAnimation {
                        width: vec![ScalarKeyframe {
                            frame: 0,
                            value: Scalar::Expr("ghost.width + 10".to_string()),
                            duration_frames: 0,
                            easing: Easing::Linear,
                        }],
                        ..Default::default()
                    },
                    mask: None,
                    content: ClipContent::Text(TextClip {
                        text: "hello".to_string(),
                        font_size: 20.0,
                        color: ColorRgba(255, 255, 255, 255),
                        align: Default::default(),
                    }),
                })],
            }],
            audio: Default::default(),
        };

        let err = compile_project(&project).expect_err("must fail");
        assert!(matches!(err, CompileError::ExprError { .. }));
    }

    #[test]
    fn unknown_clip_ref_in_transform_errors() {
        let project = Project {
            canvas: Canvas {
                width: 640,
                height: 360,
                background: ColorRgba(0, 0, 0, 255),
            },
            timeline: Timeline {
                fps: Rational::new(30, 1).expect("fps"),
                total_frames: 30,
            },
            sources: vec![],
            layers: vec![Layer {
                id: "layer_a".to_string(),
                z_index: 0,
                items: vec![LayerItem::Clip(Clip {
                    id: "clip_a".to_string(),
                    start_frame: 0,
                    duration_frames: 30,
                    opacity: 1.0,
                    transform: Transform {
                        x: Scalar::Expr("nonexistent.width".to_string()),
                        y: Scalar::Literal(0.0),
                        width: None,
                        height: None,
                        rotation_degrees: 0.0,
                    },
                    animation: Default::default(),
                    mask: None,
                    content: ClipContent::Text(TextClip {
                        text: "hello".to_string(),
                        font_size: 20.0,
                        color: ColorRgba(255, 255, 255, 255),
                        align: Default::default(),
                    }),
                })],
            }],
            audio: Default::default(),
        };

        let err = compile_project(&project).expect_err("must fail");
        assert!(matches!(err, CompileError::ExprError { .. }));
    }

    #[test]
    fn layout_node_unknown_ref_errors() {
        let project = Project {
            canvas: Canvas {
                width: 640,
                height: 360,
                background: ColorRgba(0, 0, 0, 255),
            },
            timeline: Timeline {
                fps: Rational::new(30, 1).expect("fps"),
                total_frames: 30,
            },
            sources: vec![],
            layers: vec![Layer {
                id: "layer_a".to_string(),
                z_index: 0,
                items: vec![LayerItem::Clip(Clip {
                    id: "clip_layout".to_string(),
                    start_frame: 0,
                    duration_frames: 30,
                    opacity: 1.0,
                    transform: Transform {
                        x: Scalar::Literal(0.0),
                        y: Scalar::Literal(0.0),
                        width: Some(Scalar::Literal(300.0)),
                        height: Some(Scalar::Literal(200.0)),
                        rotation_degrees: 0.0,
                    },
                    animation: Default::default(),
                    mask: None,
                    content: ClipContent::Layout(LayoutClip {
                        root: LayoutNode {
                            id: None,
                            style: Default::default(),
                            kind: LayoutNodeKind::Container {
                                children: vec![LayoutNode {
                                    id: None,
                                    style: LayoutNodeStyle {
                                        width: Some(Scalar::Expr("node_ghost.width".to_string())),
                                        ..Default::default()
                                    },
                                    kind: LayoutNodeKind::Container { children: vec![] },
                                }],
                            },
                        },
                    }),
                })],
            }],
            audio: Default::default(),
        };

        let err = compile_project(&project).expect_err("must fail");
        assert!(matches!(err, CompileError::ExprError { .. }));
    }

    #[test]
    fn layout_node_cycle_errors() {
        let project = Project {
            canvas: Canvas {
                width: 640,
                height: 360,
                background: ColorRgba(0, 0, 0, 255),
            },
            timeline: Timeline {
                fps: Rational::new(30, 1).expect("fps"),
                total_frames: 30,
            },
            sources: vec![],
            layers: vec![Layer {
                id: "layer_a".to_string(),
                z_index: 0,
                items: vec![LayerItem::Clip(Clip {
                    id: "clip_layout".to_string(),
                    start_frame: 0,
                    duration_frames: 30,
                    opacity: 1.0,
                    transform: Transform {
                        x: Scalar::Literal(0.0),
                        y: Scalar::Literal(0.0),
                        width: Some(Scalar::Literal(300.0)),
                        height: Some(Scalar::Literal(200.0)),
                        rotation_degrees: 0.0,
                    },
                    animation: Default::default(),
                    mask: None,
                    content: ClipContent::Layout(LayoutClip {
                        root: LayoutNode {
                            id: None,
                            style: Default::default(),
                            kind: LayoutNodeKind::Container {
                                children: vec![
                                    LayoutNode {
                                        id: Some("node_a".to_string()),
                                        style: LayoutNodeStyle {
                                            width: Some(Scalar::Expr("node_b.width".to_string())),
                                            ..Default::default()
                                        },
                                        kind: LayoutNodeKind::Container { children: vec![] },
                                    },
                                    LayoutNode {
                                        id: Some("node_b".to_string()),
                                        style: LayoutNodeStyle {
                                            width: Some(Scalar::Expr("node_a.width".to_string())),
                                            ..Default::default()
                                        },
                                        kind: LayoutNodeKind::Container { children: vec![] },
                                    },
                                ],
                            },
                        },
                    }),
                })],
            }],
            audio: Default::default(),
        };

        let err = compile_project(&project).expect_err("must fail");
        assert!(matches!(err, CompileError::ExprError { .. }));
    }

    #[test]
    fn layout_node_expr_passes_compilation() {
        let project = Project {
            canvas: Canvas {
                width: 640,
                height: 360,
                background: ColorRgba(0, 0, 0, 255),
            },
            timeline: Timeline {
                fps: Rational::new(30, 1).expect("fps"),
                total_frames: 30,
            },
            sources: vec![],
            layers: vec![Layer {
                id: "layer_a".to_string(),
                z_index: 0,
                items: vec![LayerItem::Clip(Clip {
                    id: "clip_layout".to_string(),
                    start_frame: 0,
                    duration_frames: 30,
                    opacity: 1.0,
                    transform: Transform {
                        x: Scalar::Literal(0.0),
                        y: Scalar::Literal(0.0),
                        width: Some(Scalar::Literal(300.0)),
                        height: Some(Scalar::Literal(200.0)),
                        rotation_degrees: 0.0,
                    },
                    animation: Default::default(),
                    mask: None,
                    content: ClipContent::Layout(LayoutClip {
                        root: LayoutNode {
                            id: None,
                            style: Default::default(),
                            kind: LayoutNodeKind::Container {
                                children: vec![
                                    LayoutNode {
                                        id: Some("node_a".to_string()),
                                        style: Default::default(),
                                        kind: LayoutNodeKind::Container { children: vec![] },
                                    },
                                    LayoutNode {
                                        id: None,
                                        style: LayoutNodeStyle {
                                            height: Some(Scalar::Expr("node_a.width".to_string())),
                                            ..Default::default()
                                        },
                                        kind: LayoutNodeKind::Container { children: vec![] },
                                    },
                                ],
                            },
                        },
                    }),
                })],
            }],
            audio: Default::default(),
        };

        assert!(compile_project(&project).is_ok());
    }

    #[test]
    fn layout_node_expr_with_mixed_known_and_unknown_refs_errors() {
        let project = Project {
            canvas: Canvas {
                width: 640,
                height: 360,
                background: ColorRgba(0, 0, 0, 255),
            },
            timeline: Timeline {
                fps: Rational::new(30, 1).expect("fps"),
                total_frames: 30,
            },
            sources: vec![],
            layers: vec![Layer {
                id: "layer_a".to_string(),
                z_index: 0,
                items: vec![LayerItem::Clip(Clip {
                    id: "clip_layout".to_string(),
                    start_frame: 0,
                    duration_frames: 30,
                    opacity: 1.0,
                    transform: Transform {
                        x: Scalar::Literal(0.0),
                        y: Scalar::Literal(0.0),
                        width: Some(Scalar::Literal(300.0)),
                        height: Some(Scalar::Literal(200.0)),
                        rotation_degrees: 0.0,
                    },
                    animation: Default::default(),
                    mask: None,
                    content: ClipContent::Layout(LayoutClip {
                        root: LayoutNode {
                            id: None,
                            style: Default::default(),
                            kind: LayoutNodeKind::Container {
                                children: vec![
                                    LayoutNode {
                                        id: Some("node_a".to_string()),
                                        style: Default::default(),
                                        kind: LayoutNodeKind::Container { children: vec![] },
                                    },
                                    LayoutNode {
                                        id: None,
                                        style: LayoutNodeStyle {
                                            width: Some(Scalar::Expr(
                                                "node_a.width + ghost.width".to_string(),
                                            )),
                                            ..Default::default()
                                        },
                                        kind: LayoutNodeKind::Container { children: vec![] },
                                    },
                                ],
                            },
                        },
                    }),
                })],
            }],
            audio: Default::default(),
        };

        let err = compile_project(&project).expect_err("must fail");
        assert!(matches!(err, CompileError::ExprError { .. }));
    }
}
