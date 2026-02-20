use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::{
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
    #[error("source pipeline error: {0}")]
    Pipeline(#[from] PipelineError),
}

#[derive(Debug, Clone)]
pub struct CompiledTimeline {
    pub canvas: Canvas,
    pub timeline: Timeline,
    sources: HashMap<String, Source>,
    operations: Vec<CompiledOperation>,
    frame_index: Vec<Vec<usize>>,
    layers: Vec<CompiledLayer>,
    has_compositing_nodes: bool,
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
    pub transform: GroupTransform,
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
    pub transform: Transform,
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
        let local = self.local_frame(frame);
        evaluate_scalar_track(self.opacity, &self.animation.opacity, local).clamp(0.0, 1.0)
    }

    pub fn resolved_transform(&self, frame: u64) -> Transform {
        let local = self.local_frame(frame);
        let mut transform = self.transform;
        transform.x = evaluate_scalar_track(transform.x, &self.animation.x, local);
        transform.y = evaluate_scalar_track(transform.y, &self.animation.y, local);
        transform.rotation_degrees = evaluate_scalar_track(
            transform.rotation_degrees,
            &self.animation.rotation_degrees,
            local,
        );
        if let Some(width) = transform.width {
            transform.width = Some(evaluate_scalar_track(width, &self.animation.width, local));
        }
        if let Some(height) = transform.height {
            transform.height = Some(evaluate_scalar_track(height, &self.animation.height, local));
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

#[derive(Debug, Clone, Copy)]
pub struct CompiledScalarKeyframe {
    pub frame: u64,
    pub value: f32,
    pub duration_frames: u64,
    pub easing: Easing,
}

struct CompileContext<'a> {
    total_frames: u64,
    sources: &'a HashMap<String, Source>,
    operations: Vec<CompiledOperation>,
    has_compositing_nodes: bool,
    max_depth: usize,
    scale: f32,
}

fn evaluate_scalar_track(base: f32, track: &[CompiledScalarKeyframe], local_frame: u64) -> f32 {
    let mut current = base;

    for keyframe in track {
        if local_frame < keyframe.frame {
            break;
        }

        let from = current;
        if keyframe.duration_frames == 0 {
            current = keyframe.value;
            continue;
        }

        let end = keyframe.frame.saturating_add(keyframe.duration_frames);
        if local_frame >= end {
            current = keyframe.value;
            continue;
        }

        let progress =
            (local_frame.saturating_sub(keyframe.frame)) as f32 / (keyframe.duration_frames as f32);
        let eased = apply_easing(progress, keyframe.easing);
        return from + (keyframe.value - from) * eased;
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
    Ok(CompiledTimeline {
        canvas,
        timeline: project.timeline.clone(),
        sources,
        operations,
        frame_index,
        layers,
        has_compositing_nodes,
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

            let transform = GroupTransform {
                x: group.transform.x * ctx.scale,
                y: group.transform.y * ctx.scale,
                rotation_degrees: group.transform.rotation_degrees,
            };

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

    if !group.transform.x.is_finite()
        || !group.transform.y.is_finite()
        || !group.transform.rotation_degrees.is_finite()
    {
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

    if !clip.transform.x.is_finite()
        || !clip.transform.y.is_finite()
        || !clip.transform.rotation_degrees.is_finite()
    {
        return Err(CompileError::InvalidClip {
            layer_id: layer.id.clone(),
            clip_id: clip.id.clone(),
            reason: "transform values must be finite".to_string(),
        });
    }

    if let Some(width) = clip.transform.width {
        if !width.is_finite() || width <= 0.0 {
            return Err(CompileError::InvalidClip {
                layer_id: layer.id.clone(),
                clip_id: clip.id.clone(),
                reason: "transform width must be finite and greater than 0".to_string(),
            });
        }
    }

    if let Some(height) = clip.transform.height {
        if !height.is_finite() || height <= 0.0 {
            return Err(CompileError::InvalidClip {
                layer_id: layer.id.clone(),
                clip_id: clip.id.clone(),
                reason: "transform height must be finite and greater than 0".to_string(),
            });
        }
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

    let animation = compile_clip_animation(layer.id.as_str(), clip, ctx.scale)?;
    let kind = compile_clip_content(layer.id.as_str(), clip, ctx.sources, ctx.scale)?;
    let transform = compile_operation_transform(clip.transform, ctx.scale);

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

fn compile_operation_transform(transform: Transform, scale: f32) -> Transform {
    Transform {
        x: transform.x * scale,
        y: transform.y * scale,
        width: transform.width.map(|value| value * scale),
        height: transform.height.map(|value| value * scale),
        rotation_degrees: transform.rotation_degrees,
    }
}

fn compile_clip_animation(
    layer_id: &str,
    clip: &crate::model::Clip,
    scale: f32,
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

    if clip.transform.width.is_some() {
        validate_dimension_keyframes(
            layer_id,
            clip,
            "animation.width",
            clip.animation.width.as_slice(),
        )?;
    }
    if clip.transform.height.is_some() {
        validate_dimension_keyframes(
            layer_id,
            clip,
            "animation.height",
            clip.animation.height.as_slice(),
        )?;
    }

    Ok(CompiledClipAnimation {
        opacity: compile_scalar_track(
            layer_id,
            clip,
            "animation.opacity",
            clip.animation.opacity.as_slice(),
            1.0,
        )?,
        x: compile_scalar_track(
            layer_id,
            clip,
            "animation.x",
            clip.animation.x.as_slice(),
            scale,
        )?,
        y: compile_scalar_track(
            layer_id,
            clip,
            "animation.y",
            clip.animation.y.as_slice(),
            scale,
        )?,
        width: compile_scalar_track(
            layer_id,
            clip,
            "animation.width",
            clip.animation.width.as_slice(),
            scale,
        )?,
        height: compile_scalar_track(
            layer_id,
            clip,
            "animation.height",
            clip.animation.height.as_slice(),
            scale,
        )?,
        rotation_degrees: compile_scalar_track(
            layer_id,
            clip,
            "animation.rotation_degrees",
            clip.animation.rotation_degrees.as_slice(),
            1.0,
        )?,
    })
}

fn validate_dimension_keyframes(
    layer_id: &str,
    clip: &crate::model::Clip,
    field: &str,
    keyframes: &[ScalarKeyframe],
) -> Result<(), CompileError> {
    for keyframe in keyframes {
        if !keyframe.value.is_finite() || keyframe.value <= 0.0 {
            return Err(CompileError::InvalidClip {
                layer_id: layer_id.to_string(),
                clip_id: clip.id.clone(),
                reason: format!("{field} values must be finite and > 0"),
            });
        }
    }
    Ok(())
}

fn compile_scalar_track(
    layer_id: &str,
    clip: &crate::model::Clip,
    field: &str,
    keyframes: &[ScalarKeyframe],
    scale: f32,
) -> Result<Vec<CompiledScalarKeyframe>, CompileError> {
    let mut sorted = keyframes.to_vec();
    sorted.sort_by_key(|keyframe| keyframe.frame);

    let mut compiled = Vec::with_capacity(sorted.len());
    let mut previous_end = 0u64;

    for (index, keyframe) in sorted.into_iter().enumerate() {
        if !keyframe.value.is_finite() {
            return Err(CompileError::InvalidClip {
                layer_id: layer_id.to_string(),
                clip_id: clip.id.clone(),
                reason: format!("{field}[{index}] value must be finite"),
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
            value: keyframe.value * scale,
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
            let _ =
                map_source_frame(&video.pipeline, 0).map_err(|err| CompileError::InvalidClip {
                    layer_id: layer_id.to_string(),
                    clip_id: clip.id.clone(),
                    reason: err.to_string(),
                })?;

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
            let compiled_layout =
                compile_layout_clip(layer_id, clip.id.as_str(), layout, sources, scale)?;
            Ok(CompiledOperationKind::Layout(compiled_layout))
        }
    }
}

fn compile_layout_clip(
    layer_id: &str,
    clip_id: &str,
    layout: &LayoutClip,
    sources: &HashMap<String, Source>,
    scale: f32,
) -> Result<LayoutClip, CompileError> {
    Ok(LayoutClip {
        root: compile_layout_node(
            layer_id,
            clip_id,
            "root".to_string(),
            &layout.root,
            sources,
            scale,
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
) -> Result<LayoutNode, CompileError> {
    let style = compile_layout_style(layer_id, clip_id, node_path.as_str(), &node.style, scale)?;

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

    Ok(LayoutNode { style, kind })
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
        style.width,
        scale,
    )?;
    let height = compile_optional_layout_dimension(
        layer_id,
        clip_id,
        node_path,
        "style.height",
        style.height,
        scale,
    )?;
    let min_width = compile_optional_layout_dimension(
        layer_id,
        clip_id,
        node_path,
        "style.min_width",
        style.min_width,
        scale,
    )?;
    let min_height = compile_optional_layout_dimension(
        layer_id,
        clip_id,
        node_path,
        "style.min_height",
        style.min_height,
        scale,
    )?;
    let max_width = compile_optional_layout_dimension(
        layer_id,
        clip_id,
        node_path,
        "style.max_width",
        style.max_width,
        scale,
    )?;
    let max_height = compile_optional_layout_dimension(
        layer_id,
        clip_id,
        node_path,
        "style.max_height",
        style.max_height,
        scale,
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
    value: Option<f32>,
    scale: f32,
) -> Result<Option<f32>, CompileError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if !value.is_finite() || value < 0.0 {
        return Err(CompileError::InvalidClip {
            layer_id: layer_id.to_string(),
            clip_id: clip_id.to_string(),
            reason: format!("{node_path}.{field} must be finite and >= 0"),
        });
    }
    Ok(Some(value * scale))
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

#[cfg(test)]
mod tests {
    use crate::{
        compile::{CompiledOperationKind, compile_project, compile_project_with_scale},
        model::{
            Canvas, Clip, ClipAnimation, ClipContent, ClipGroup, ColorRgba, Easing, Layer,
            LayerItem, LayoutClip, LayoutNode, LayoutNodeKind, LayoutTextNode, Project,
            ScalarKeyframe, Source, SourceKind, SourceMediaType, TextClip, Timeline, Transform,
            VideoClip,
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
                            value: 1.0,
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
                                value: 10.0,
                                duration_frames: 8,
                                easing: Easing::EaseOut,
                            },
                            ScalarKeyframe {
                                frame: 6,
                                value: 20.0,
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
                        x: 0.0,
                        y: 0.0,
                        width: None,
                        height: Some(180.0),
                        rotation_degrees: 0.0,
                    },
                    animation: Default::default(),
                    mask: None,
                    content: ClipContent::Layout(LayoutClip {
                        root: LayoutNode {
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
                        x: 10.0,
                        y: 20.0,
                        width: Some(300.0),
                        height: Some(180.0),
                        rotation_degrees: 0.0,
                    },
                    animation: Default::default(),
                    mask: None,
                    content: ClipContent::Layout(LayoutClip {
                        root: LayoutNode {
                            style: Default::default(),
                            kind: LayoutNodeKind::Container {
                                children: vec![LayoutNode {
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
}
