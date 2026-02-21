use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use thiserror::Error;

use crate::expression::ExprParseError;
use crate::model::{
    BaseStyle, ClipContent, ClipItem, ClipStyle, Layer, LayerItem, LayoutNode, LayoutNodeKind,
    Source, SourceKind, SourceMedia, StyleValue,
};

mod dependency;
mod operation;
mod scalar;

use dependency::{DependencyError, DependencyNode, build_eval_order};
pub use operation::{
    CompiledBaseStyle, CompiledClipNode, CompiledClipStyle, CompiledExpressionBinding,
    CompiledGroupNode, CompiledImage, CompiledLayer, CompiledLayerItem, CompiledLayoutClip,
    CompiledLayoutNode, CompiledLayoutNodeKind, CompiledLayoutNodeStyle, CompiledOperation,
    CompiledOperationKind, CompiledShadowStyle, CompiledShape, CompiledSource, CompiledStrokeStyle,
    CompiledText, CompiledTimeline, CompiledTransformStyle, CompiledVideo, ResolvedTransform,
    RuntimeEvalError, RuntimeFrameContext,
};
pub use scalar::ScalarHandle;
use scalar::{CompiledScalarValue, compile_optional_scalar};

/// Upper bound on timeline duration to prevent OOM from malicious/malformed projects.
/// 1_000_000 frames at 30 fps ≈ 9.25 hours — generous for any legitimate use.
const MAX_DURATION_FRAMES: u64 = 1_000_000;

#[derive(Debug, Error)]
pub enum CompileError {
    #[error("invalid scale `{0}`")]
    InvalidScale(f32),
    #[error("invalid canvas: {0}")]
    InvalidCanvas(String),
    #[error("invalid timeline: {0}")]
    InvalidTimeline(String),
    #[error("duplicate source id `{0}`")]
    DuplicateSourceId(String),
    #[error("duplicate item id `{0}`")]
    DuplicateItemId(String),
    #[error("mask item id `{mask_id}` collides with layer item id")]
    MaskIdCollision { mask_id: String },
    #[error("source `{source_id}` is url-based and must be materialized to file before compile")]
    UrlSourceUnsupported { source_id: String },
    #[error("missing source `{source_id}`")]
    MissingSource { source_id: String },
    #[error("source `{source_id}` has media `{found:?}` but expected `{expected:?}`")]
    SourceTypeMismatch {
        source_id: String,
        expected: SourceMedia,
        found: SourceMedia,
    },
    #[error("clip `{clip_id}` is invalid: {reason}")]
    InvalidClip { clip_id: String, reason: String },
    #[error("layout node id `{0}` is duplicated")]
    DuplicateLayoutNodeId(String),
    #[error(
        "failed to parse expression for `{owner_id}` ({property_path}) in `{expression}`: {source}"
    )]
    ExprParse {
        owner_id: String,
        property_path: String,
        expression: String,
        source: ExprParseError,
    },
    #[error("circular dependency detected: {nodes:?}")]
    CircularDependency { nodes: Vec<String> },
    #[error("unsupported project version `{0}`")]
    UnsupportedVersion(String),
}

pub fn compile_project(
    project: &crate::model::Project,
) -> Result<Arc<CompiledTimeline>, CompileError> {
    compile_project_with_scale(project, 1.0)
}

pub fn compile_project_with_scale(
    project: &crate::model::Project,
    scale: f32,
) -> Result<Arc<CompiledTimeline>, CompileError> {
    if !scale.is_finite() || scale <= 0.0 {
        return Err(CompileError::InvalidScale(scale));
    }

    if project.version != "1" {
        return Err(CompileError::UnsupportedVersion(project.version.clone()));
    }

    if project.canvas.width == 0 || project.canvas.height == 0 {
        return Err(CompileError::InvalidCanvas(
            "canvas width and height must be > 0".to_string(),
        ));
    }

    if project.timeline.fps.num == 0 || project.timeline.fps.den == 0 {
        return Err(CompileError::InvalidTimeline(
            "timeline fps numerator and denominator must be > 0".to_string(),
        ));
    }

    if project.timeline.duration_frames == 0 {
        return Err(CompileError::InvalidTimeline(
            "timeline duration_frames must be > 0".to_string(),
        ));
    }

    if project.timeline.duration_frames > MAX_DURATION_FRAMES {
        return Err(CompileError::InvalidTimeline(format!(
            "timeline duration_frames {} exceeds maximum {}",
            project.timeline.duration_frames, MAX_DURATION_FRAMES
        )));
    }

    let mut scaled = project.clone();
    scale_project_canvas(&mut scaled, scale);

    validate_item_ids(scaled.layers.as_slice())?;

    let source_lookup = compile_sources(scaled.sources.as_slice())?;

    let mut ctx = CompileContext {
        source_lookup,
        registry: PropertyRegistry::default(),
        layout_ids: HashSet::new(),
        operations: Vec::new(),
    };

    let mut layers = Vec::with_capacity(scaled.layers.len());
    for layer in scaled.layers.iter().enumerate() {
        layers.push(compile_layer(layer.0, layer.1, &mut ctx, scale)?);
    }

    let dependency_nodes = ctx.registry.dependency_nodes();
    let eval_order = build_eval_order(dependency_nodes).map_err(|err| match err {
        DependencyError::CircularDependency { nodes } => CompileError::CircularDependency { nodes },
    })?;

    let total_frames = scaled.timeline.duration_frames;
    let mut frame_index = vec![Vec::new(); total_frames as usize];
    for (operation_index, operation) in ctx.operations.iter().enumerate() {
        if operation.is_mask {
            continue;
        }
        let start = operation.start_frame.min(total_frames);
        let end = operation.end_frame.min(total_frames);
        for frame in start..end {
            frame_index[frame as usize].push(operation_index);
        }
    }

    let scalar_count = ctx.registry.path_indices.len();
    let path_indices = Arc::new(ctx.registry.path_indices);

    let timeline = CompiledTimeline {
        canvas: scaled.canvas,
        timeline: scaled.timeline,
        sources: sorted_sources(&ctx.source_lookup),
        layers,
        operations: ctx.operations,
        frame_index,
        literal_scalars: ctx.registry.literal_scalars,
        expression_scalars: ctx.registry.expression_scalars,
        eval_order,
        path_indices,
        scalar_count,
    };

    Ok(Arc::new(timeline))
}

fn sorted_sources(lookup: &HashMap<String, CompiledSourceRef>) -> Vec<CompiledSource> {
    let mut entries = lookup.values().cloned().collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.index);
    entries
        .into_iter()
        .map(|entry| CompiledSource {
            id: entry.id,
            media: entry.media,
            path: entry.path,
        })
        .collect()
}

fn scale_project_canvas(project: &mut crate::model::Project, scale: f32) {
    if (scale - 1.0).abs() <= f32::EPSILON {
        return;
    }

    let width = (project.canvas.width as f32 * scale).round().max(1.0);
    let height = (project.canvas.height as f32 * scale).round().max(1.0);
    project.canvas.width = width as u32;
    project.canvas.height = height as u32;
}

fn validate_item_ids(layers: &[Layer]) -> Result<(), CompileError> {
    let mut layer_ids = HashSet::new();
    let mut mask_ids = HashSet::new();

    for layer in layers {
        for item in &layer.items {
            collect_item_ids(item, false, &mut layer_ids, &mut mask_ids)?;
        }
    }

    Ok(())
}

fn collect_item_ids(
    item: &LayerItem,
    in_mask: bool,
    layer_ids: &mut HashSet<String>,
    mask_ids: &mut HashSet<String>,
) -> Result<(), CompileError> {
    let id = item.id().to_string();
    if in_mask {
        if layer_ids.contains(id.as_str()) {
            return Err(CompileError::MaskIdCollision { mask_id: id });
        }
        if !mask_ids.insert(id.clone()) {
            return Err(CompileError::DuplicateItemId(id));
        }
    } else if !layer_ids.insert(id.clone()) {
        return Err(CompileError::DuplicateItemId(id));
    }

    match item {
        LayerItem::Clip(clip) => {
            if let Some(mask) = &clip.mask {
                collect_item_ids(mask, true, layer_ids, mask_ids)?;
            }
        }
        LayerItem::Group(group) => {
            for child in &group.items {
                collect_item_ids(child, in_mask, layer_ids, mask_ids)?;
            }
            if let Some(mask) = &group.mask {
                collect_item_ids(mask, true, layer_ids, mask_ids)?;
            }
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct CompiledSourceRef {
    index: usize,
    id: String,
    media: SourceMedia,
    path: String,
}

fn compile_sources(sources: &[Source]) -> Result<HashMap<String, CompiledSourceRef>, CompileError> {
    let mut lookup = HashMap::with_capacity(sources.len());
    for (index, source) in sources.iter().enumerate() {
        if lookup.contains_key(source.id.as_str()) {
            return Err(CompileError::DuplicateSourceId(source.id.clone()));
        }

        let path = match &source.kind {
            SourceKind::File { path } => path.clone(),
            SourceKind::Url { .. } => {
                return Err(CompileError::UrlSourceUnsupported {
                    source_id: source.id.clone(),
                });
            }
            SourceKind::Generator { filter } => format!("generator:{filter}"),
        };

        lookup.insert(
            source.id.clone(),
            CompiledSourceRef {
                index,
                id: source.id.clone(),
                media: source.media,
                path,
            },
        );
    }
    Ok(lookup)
}

#[derive(Default)]
struct PropertyRegistry {
    seen_paths: HashSet<String>,
    path_indices: HashMap<String, usize>,
    literal_scalars: Vec<(usize, f32)>,
    expression_scalars: Vec<CompiledExpressionBinding>,
    dependency_nodes: Vec<DependencyNode>,
}

impl PropertyRegistry {
    fn dependency_nodes(&self) -> &[DependencyNode] {
        self.dependency_nodes.as_slice()
    }

    fn register(
        &mut self,
        owner_id: &str,
        target_id: &str,
        property: &str,
        scalar: CompiledScalarValue,
    ) -> Result<ScalarHandle, CompileError> {
        let path = format!("{}.{}", target_id, property);
        if !self.seen_paths.insert(path.clone()) {
            return Err(CompileError::DuplicateItemId(path));
        }
        let index = self.path_indices.len();
        self.path_indices.insert(path.clone(), index);

        match scalar {
            CompiledScalarValue::Literal(value) => {
                self.literal_scalars.push((index, value));
                Ok(ScalarHandle::new(index, value))
            }
            CompiledScalarValue::Expr(parsed) => {
                let fallback = 0.0;
                self.expression_scalars.push(CompiledExpressionBinding {
                    index,
                    path: path.clone(),
                    owner_id: owner_id.to_string(),
                    expression: parsed.source().to_string(),
                    expr: parsed.clone(),
                });
                self.dependency_nodes.push(DependencyNode {
                    path: path.clone(),
                    refs: parsed.references().to_vec(),
                });
                Ok(ScalarHandle::new(index, fallback))
            }
        }
    }
}

struct CompileContext {
    source_lookup: HashMap<String, CompiledSourceRef>,
    registry: PropertyRegistry,
    layout_ids: HashSet<String>,
    operations: Vec<CompiledOperation>,
}

fn compile_layer(
    layer_index: usize,
    layer: &Layer,
    ctx: &mut CompileContext,
    scale: f32,
) -> Result<CompiledLayer, CompileError> {
    let mut compiled = CompiledLayer::from(layer);
    for item in &layer.items {
        compiled.items.push(compile_layer_item(
            item,
            layer_index,
            layer.id.as_str(),
            ctx,
            false,
            scale,
        )?);
    }
    Ok(compiled)
}

fn compile_layer_item(
    item: &LayerItem,
    layer_index: usize,
    layer_id: &str,
    ctx: &mut CompileContext,
    is_mask: bool,
    scale: f32,
) -> Result<CompiledLayerItem, CompileError> {
    match item {
        LayerItem::Clip(clip) => {
            let op_index = compile_clip(layer_index, layer_id, clip, ctx, is_mask, scale)?;
            let mask = clip.mask.as_deref().map(|mask| {
                compile_layer_item(mask, layer_index, layer_id, ctx, true, scale).map(Box::new)
            });
            let mask = match mask {
                Some(result) => Some(result?),
                None => None,
            };
            Ok(CompiledLayerItem::Clip(CompiledClipNode {
                operation_index: op_index,
                mask,
            }))
        }
        LayerItem::Group(group) => {
            let style =
                compile_group_style(group.id.as_str(), &group.style, &mut ctx.registry, scale)?;
            let mut children = Vec::with_capacity(group.items.len());
            for child in &group.items {
                children.push(compile_layer_item(
                    child,
                    layer_index,
                    layer_id,
                    ctx,
                    is_mask,
                    scale,
                )?);
            }
            let mask = group.mask.as_deref().map(|mask| {
                compile_layer_item(mask, layer_index, layer_id, ctx, true, scale).map(Box::new)
            });
            let mask = match mask {
                Some(result) => Some(result?),
                None => None,
            };
            Ok(CompiledLayerItem::Group(CompiledGroupNode {
                id: group.id.clone(),
                style,
                items: children,
                mask,
            }))
        }
    }
}

fn compile_clip(
    layer_index: usize,
    layer_id: &str,
    clip: &ClipItem,
    ctx: &mut CompileContext,
    is_mask: bool,
    scale: f32,
) -> Result<usize, CompileError> {
    if clip.duration_frames == 0 {
        return Err(CompileError::InvalidClip {
            clip_id: clip.id.clone(),
            reason: "duration_frames must be > 0".to_string(),
        });
    }

    let style = compile_clip_style(clip.id.as_str(), &clip.style, &mut ctx.registry, scale)?;

    let kind = match &clip.content {
        ClipContent::Solid => CompiledOperationKind::Solid,
        ClipContent::Shape { geometry } => CompiledOperationKind::Shape(CompiledShape {
            geometry: geometry.clone(),
        }),
        ClipContent::Text { content } => CompiledOperationKind::Text(CompiledText {
            content: content.clone(),
        }),
        ClipContent::Image { source } => {
            let source_ref = resolve_source(ctx, source.as_str(), SourceMedia::Image)?;
            CompiledOperationKind::Image(CompiledImage {
                source_index: source_ref.index,
            })
        }
        ClipContent::Video { source, pipeline } => {
            let source_ref = resolve_source(ctx, source.as_str(), SourceMedia::Video)?;
            CompiledOperationKind::Video(CompiledVideo {
                source_index: source_ref.index,
                pipeline: pipeline.clone(),
            })
        }
        ClipContent::Layout { root } => CompiledOperationKind::Layout(CompiledLayoutClip {
            root: compile_layout_node(root, ctx, scale, clip.id.as_str())?,
        }),
    };

    let operation = CompiledOperation {
        id: clip.id.clone(),
        layer_id: layer_id.to_string(),
        start_frame: clip.start_frame,
        end_frame: clip.start_frame.saturating_add(clip.duration_frames),
        z_index: layer_index,
        style,
        kind,
        is_mask,
    };

    let index = ctx.operations.len();
    ctx.operations.push(operation);
    Ok(index)
}

fn resolve_source<'a>(
    ctx: &'a CompileContext,
    source_id: &str,
    expected: SourceMedia,
) -> Result<&'a CompiledSourceRef, CompileError> {
    let source = ctx
        .source_lookup
        .get(source_id)
        .ok_or_else(|| CompileError::MissingSource {
            source_id: source_id.to_string(),
        })?;

    if source.media != expected {
        return Err(CompileError::SourceTypeMismatch {
            source_id: source_id.to_string(),
            expected,
            found: source.media,
        });
    }

    Ok(source)
}

fn compile_layout_node(
    node: &LayoutNode,
    ctx: &mut CompileContext,
    scale: f32,
    owner_id: &str,
) -> Result<CompiledLayoutNode, CompileError> {
    if !ctx.layout_ids.insert(node.id.clone()) {
        return Err(CompileError::DuplicateLayoutNodeId(node.id.clone()));
    }

    let width = compile_optional_layout_scalar(
        owner_id,
        node.id.as_str(),
        "width",
        node.style.width.as_ref(),
        0.0,
        true,
        &mut ctx.registry,
        scale,
    )?;
    let height = compile_optional_layout_scalar(
        owner_id,
        node.id.as_str(),
        "height",
        node.style.height.as_ref(),
        0.0,
        true,
        &mut ctx.registry,
        scale,
    )?;

    let style = CompiledLayoutNodeStyle {
        width,
        height,
        min_width: compile_optional_layout_scalar(
            owner_id,
            node.id.as_str(),
            "min_width",
            node.style.min_width.as_ref(),
            0.0,
            true,
            &mut ctx.registry,
            scale,
        )?,
        min_height: compile_optional_layout_scalar(
            owner_id,
            node.id.as_str(),
            "min_height",
            node.style.min_height.as_ref(),
            0.0,
            true,
            &mut ctx.registry,
            scale,
        )?,
        max_width: compile_optional_layout_scalar(
            owner_id,
            node.id.as_str(),
            "max_width",
            node.style.max_width.as_ref(),
            0.0,
            true,
            &mut ctx.registry,
            scale,
        )?,
        max_height: compile_optional_layout_scalar(
            owner_id,
            node.id.as_str(),
            "max_height",
            node.style.max_height.as_ref(),
            0.0,
            true,
            &mut ctx.registry,
            scale,
        )?,
        padding_left: compile_optional_layout_scalar(
            owner_id,
            node.id.as_str(),
            "padding_left",
            node.style.padding_left.as_ref(),
            0.0,
            true,
            &mut ctx.registry,
            scale,
        )?,
        padding_top: compile_optional_layout_scalar(
            owner_id,
            node.id.as_str(),
            "padding_top",
            node.style.padding_top.as_ref(),
            0.0,
            true,
            &mut ctx.registry,
            scale,
        )?,
        padding_right: compile_optional_layout_scalar(
            owner_id,
            node.id.as_str(),
            "padding_right",
            node.style.padding_right.as_ref(),
            0.0,
            true,
            &mut ctx.registry,
            scale,
        )?,
        padding_bottom: compile_optional_layout_scalar(
            owner_id,
            node.id.as_str(),
            "padding_bottom",
            node.style.padding_bottom.as_ref(),
            0.0,
            true,
            &mut ctx.registry,
            scale,
        )?,
        gap: compile_optional_layout_scalar(
            owner_id,
            node.id.as_str(),
            "gap",
            node.style.gap.as_ref(),
            0.0,
            true,
            &mut ctx.registry,
            scale,
        )?,
        grow: compile_optional_layout_scalar(
            owner_id,
            node.id.as_str(),
            "grow",
            node.style.grow.as_ref(),
            0.0,
            false,
            &mut ctx.registry,
            scale,
        )?,
        shrink: compile_optional_layout_scalar(
            owner_id,
            node.id.as_str(),
            "shrink",
            node.style.shrink.as_ref(),
            0.0,
            false,
            &mut ctx.registry,
            scale,
        )?,
        basis: compile_optional_layout_scalar(
            owner_id,
            node.id.as_str(),
            "basis",
            node.style.basis.as_ref(),
            0.0,
            true,
            &mut ctx.registry,
            scale,
        )?,
        justify: node.style.justify,
        align: node.style.align,
        direction: node.style.direction,
    };

    if style.width.is_none() {
        register_scalar(
            owner_id,
            node.id.as_str(),
            "width",
            Some(&StyleValue::Value(0.0)),
            0.0,
            true,
            &mut ctx.registry,
            scale,
        )?;
    }
    if style.height.is_none() {
        register_scalar(
            owner_id,
            node.id.as_str(),
            "height",
            Some(&StyleValue::Value(0.0)),
            0.0,
            true,
            &mut ctx.registry,
            scale,
        )?;
    }

    // register runtime-ref layout outputs
    register_scalar(
        owner_id,
        node.id.as_str(),
        "x",
        Some(&StyleValue::Value(0.0)),
        0.0,
        true,
        &mut ctx.registry,
        scale,
    )?;
    register_scalar(
        owner_id,
        node.id.as_str(),
        "y",
        Some(&StyleValue::Value(0.0)),
        0.0,
        true,
        &mut ctx.registry,
        scale,
    )?;

    let kind = match &node.kind {
        LayoutNodeKind::Container { children } => {
            let mut compiled_children = Vec::with_capacity(children.len());
            for child in children {
                compiled_children.push(compile_layout_node(child, ctx, scale, owner_id)?);
            }
            CompiledLayoutNodeKind::Container {
                children: compiled_children,
            }
        }
        LayoutNodeKind::Text { content } => CompiledLayoutNodeKind::Text {
            content: content.clone(),
        },
        LayoutNodeKind::Image { source } => {
            let source_ref = resolve_source(ctx, source.as_str(), SourceMedia::Image)?;
            CompiledLayoutNodeKind::Image {
                source_index: source_ref.index,
            }
        }
    };

    Ok(CompiledLayoutNode {
        id: node.id.clone(),
        style,
        kind,
    })
}

fn compile_optional_layout_scalar(
    owner_id: &str,
    target_id: &str,
    property: &str,
    value: Option<&StyleValue>,
    default: f32,
    spatial: bool,
    registry: &mut PropertyRegistry,
    scale: f32,
) -> Result<Option<ScalarHandle>, CompileError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let compiled =
        compile_optional_scalar(Some(value), default, scale, spatial).map_err(|source| {
            CompileError::ExprParse {
                owner_id: owner_id.to_string(),
                property_path: format!("{}.{}", target_id, property),
                expression: value.as_expr().unwrap_or_default().to_string(),
                source,
            }
        })?;
    let handle = registry.register(owner_id, target_id, property, compiled)?;
    Ok(Some(handle))
}

fn compile_group_style(
    owner_id: &str,
    style: &BaseStyle,
    registry: &mut PropertyRegistry,
    scale: f32,
) -> Result<CompiledBaseStyle, CompileError> {
    compile_base_style(owner_id, owner_id, style, registry, scale)
}

fn compile_clip_style(
    owner_id: &str,
    style: &ClipStyle,
    registry: &mut PropertyRegistry,
    scale: f32,
) -> Result<CompiledClipStyle, CompileError> {
    let base = compile_base_style(owner_id, owner_id, &style.base, registry, scale)?;

    let stroke = if let Some(stroke) = &style.stroke {
        let width = register_scalar(
            owner_id,
            owner_id,
            "stroke_width",
            Some(&stroke.width),
            1.0,
            true,
            registry,
            scale,
        )?;
        let dash_offset = register_scalar(
            owner_id,
            owner_id,
            "stroke_dash_offset",
            stroke.dash.as_ref().map(|dash| &dash.offset),
            0.0,
            true,
            registry,
            scale,
        )?;
        let mut dash_pattern = Vec::new();
        if let Some(dash) = &stroke.dash {
            for (index, value) in dash.pattern.iter().enumerate() {
                dash_pattern.push(register_scalar(
                    owner_id,
                    owner_id,
                    format!("stroke_dash_{}", index).as_str(),
                    Some(value),
                    0.0,
                    true,
                    registry,
                    scale,
                )?);
            }
        }

        Some(CompiledStrokeStyle {
            color: stroke.color,
            width,
            dash_pattern,
            dash_offset,
        })
    } else {
        None
    };

    let corner_radius = if let Some(corners) = &style.corner_radius {
        Some([
            register_scalar(
                owner_id,
                owner_id,
                "corner_radius_tl",
                Some(&corners[0]),
                0.0,
                true,
                registry,
                scale,
            )?,
            register_scalar(
                owner_id,
                owner_id,
                "corner_radius_tr",
                Some(&corners[1]),
                0.0,
                true,
                registry,
                scale,
            )?,
            register_scalar(
                owner_id,
                owner_id,
                "corner_radius_br",
                Some(&corners[2]),
                0.0,
                true,
                registry,
                scale,
            )?,
            register_scalar(
                owner_id,
                owner_id,
                "corner_radius_bl",
                Some(&corners[3]),
                0.0,
                true,
                registry,
                scale,
            )?,
        ])
    } else {
        None
    };

    let font_size = register_scalar(
        owner_id,
        owner_id,
        "font_size",
        style.font_size.as_ref(),
        48.0,
        true,
        registry,
        scale,
    )?;
    let font_weight = register_scalar(
        owner_id,
        owner_id,
        "font_weight",
        style.font_weight.as_ref(),
        400.0,
        false,
        registry,
        scale,
    )?;
    let letter_spacing = register_scalar(
        owner_id,
        owner_id,
        "letter_spacing",
        style.letter_spacing.as_ref(),
        0.0,
        true,
        registry,
        scale,
    )?;
    let line_height = register_scalar(
        owner_id,
        owner_id,
        "line_height",
        style.line_height.as_ref(),
        1.2,
        false,
        registry,
        scale,
    )?;

    Ok(CompiledClipStyle {
        base,
        fill: style.fill,
        stroke,
        corner_radius,
        font_family: style.font_family.clone(),
        font_size,
        font_weight,
        color: style.color,
        align: style.align.unwrap_or_default(),
        vertical_align: style.vertical_align.unwrap_or_default(),
        letter_spacing,
        line_height,
        fit: style.fit.unwrap_or_default(),
        color_matrix: style.color_matrix,
    })
}

fn compile_base_style(
    owner_id: &str,
    target_id: &str,
    style: &BaseStyle,
    registry: &mut PropertyRegistry,
    scale: f32,
) -> Result<CompiledBaseStyle, CompileError> {
    let opacity = register_scalar(
        owner_id,
        target_id,
        "opacity",
        Some(&style.opacity),
        1.0,
        false,
        registry,
        scale,
    )?;
    let blur = register_scalar(
        owner_id,
        target_id,
        "blur",
        Some(&style.blur),
        0.0,
        true,
        registry,
        scale,
    )?;

    let shadow = if let Some(shadow) = &style.shadow {
        Some(CompiledShadowStyle {
            offset_x: register_scalar(
                owner_id,
                target_id,
                "shadow_offset_x",
                Some(&shadow.offset_x),
                4.0,
                true,
                registry,
                scale,
            )?,
            offset_y: register_scalar(
                owner_id,
                target_id,
                "shadow_offset_y",
                Some(&shadow.offset_y),
                4.0,
                true,
                registry,
                scale,
            )?,
            blur: register_scalar(
                owner_id,
                target_id,
                "shadow_blur",
                Some(&shadow.blur),
                12.0,
                true,
                registry,
                scale,
            )?,
            color: shadow.color,
        })
    } else {
        None
    };

    let transform = CompiledTransformStyle {
        x: register_scalar(
            owner_id,
            target_id,
            "x",
            Some(&style.transform.x),
            0.0,
            true,
            registry,
            scale,
        )?,
        y: register_scalar(
            owner_id,
            target_id,
            "y",
            Some(&style.transform.y),
            0.0,
            true,
            registry,
            scale,
        )?,
        width: register_scalar(
            owner_id,
            target_id,
            "width",
            Some(&style.transform.width),
            0.0,
            true,
            registry,
            scale,
        )?,
        height: register_scalar(
            owner_id,
            target_id,
            "height",
            Some(&style.transform.height),
            0.0,
            true,
            registry,
            scale,
        )?,
        rotation: register_scalar(
            owner_id,
            target_id,
            "rotation",
            Some(&style.transform.rotation),
            0.0,
            false,
            registry,
            scale,
        )?,
        anchor_x: register_scalar(
            owner_id,
            target_id,
            "anchor_x",
            Some(&style.transform.anchor_x),
            0.5,
            false,
            registry,
            scale,
        )?,
        anchor_y: register_scalar(
            owner_id,
            target_id,
            "anchor_y",
            Some(&style.transform.anchor_y),
            0.5,
            false,
            registry,
            scale,
        )?,
        scale_x: register_scalar(
            owner_id,
            target_id,
            "scale_x",
            Some(&style.transform.scale_x),
            1.0,
            false,
            registry,
            scale,
        )?,
        scale_y: register_scalar(
            owner_id,
            target_id,
            "scale_y",
            Some(&style.transform.scale_y),
            1.0,
            false,
            registry,
            scale,
        )?,
        skew_x: register_scalar(
            owner_id,
            target_id,
            "skew_x",
            Some(&style.transform.skew_x),
            0.0,
            false,
            registry,
            scale,
        )?,
        skew_y: register_scalar(
            owner_id,
            target_id,
            "skew_y",
            Some(&style.transform.skew_y),
            0.0,
            false,
            registry,
            scale,
        )?,
    };

    let alignment = [
        register_scalar(
            owner_id,
            target_id,
            "alignment_x",
            Some(&style.alignment[0]),
            0.0,
            false,
            registry,
            scale,
        )?,
        register_scalar(
            owner_id,
            target_id,
            "alignment_y",
            Some(&style.alignment[1]),
            0.0,
            false,
            registry,
            scale,
        )?,
    ];

    Ok(CompiledBaseStyle {
        visible: style.visible,
        opacity,
        blend_mode: style.blend_mode,
        blur,
        shadow,
        transform,
        alignment,
    })
}

fn register_scalar(
    owner_id: &str,
    target_id: &str,
    property: &str,
    value: Option<&StyleValue>,
    default: f32,
    spatial: bool,
    registry: &mut PropertyRegistry,
    scale: f32,
) -> Result<ScalarHandle, CompileError> {
    let compiled = compile_optional_scalar(value, default, scale, spatial).map_err(|source| {
        CompileError::ExprParse {
            owner_id: owner_id.to_string(),
            property_path: format!("{}.{}", target_id, property),
            expression: value
                .and_then(StyleValue::as_expr)
                .unwrap_or_default()
                .to_string(),
            source,
        }
    })?;
    registry.register(owner_id, target_id, property, compiled)
}

#[cfg(test)]
mod tests {
    use crate::compile::{compile_project, compile_project_with_scale};
    use crate::model::{
        Canvas, ClipContent, ClipItem, Layer, LayerItem, Project, StyleValue, Timeline,
    };

    #[test]
    fn rejects_expression_cycles() {
        let project = Project {
            version: "1".to_string(),
            canvas: Canvas {
                width: 100,
                height: 100,
                background: [0, 0, 0, 255],
            },
            timeline: Timeline {
                fps: crate::Rational::new(30, 1),
                duration_frames: 10,
            },
            sources: Vec::new(),
            layers: vec![Layer {
                id: "layer_0".to_string(),
                items: vec![
                    LayerItem::Clip(ClipItem {
                        id: "a".to_string(),
                        start_frame: 0,
                        duration_frames: 10,
                        content: ClipContent::Solid,
                        style: crate::model::ClipStyle {
                            base: crate::model::BaseStyle {
                                transform: crate::model::TransformStyle {
                                    x: StyleValue::Expr("b.x".to_string()),
                                    ..Default::default()
                                },
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                        mask: None,
                    }),
                    LayerItem::Clip(ClipItem {
                        id: "b".to_string(),
                        start_frame: 0,
                        duration_frames: 10,
                        content: ClipContent::Solid,
                        style: crate::model::ClipStyle {
                            base: crate::model::BaseStyle {
                                transform: crate::model::TransformStyle {
                                    x: StyleValue::Expr("a.x".to_string()),
                                    ..Default::default()
                                },
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                        mask: None,
                    }),
                ],
            }],
            audio: Default::default(),
        };

        let error = compile_project(&project).expect_err("cycle expected");
        assert!(error.to_string().contains("circular dependency"));
    }

    #[test]
    fn scaled_canvas_expression_resolves_once() {
        let mut clip = ClipItem {
            id: "clip_a".to_string(),
            start_frame: 0,
            duration_frames: 10,
            content: ClipContent::Solid,
            style: Default::default(),
            mask: None,
        };
        clip.style.base.transform.width = StyleValue::Expr("canvas.width * 0.5".to_string());

        let project = Project {
            version: "1".to_string(),
            canvas: Canvas {
                width: 200,
                height: 100,
                background: [0, 0, 0, 255],
            },
            timeline: Timeline {
                fps: crate::Rational::new(30, 1),
                duration_frames: 10,
            },
            sources: Vec::new(),
            layers: vec![Layer {
                id: "layer_0".to_string(),
                items: vec![LayerItem::Clip(clip)],
            }],
            audio: Default::default(),
        };

        let timeline =
            compile_project_with_scale(&project, 0.5).expect("scaled compile should succeed");
        let frame_state = timeline
            .resolve_frame_context(0)
            .expect("frame context should resolve");
        assert_eq!(frame_state.get("clip_a.width"), Some(50.0));
    }
    #[test]
    fn rejects_unsupported_project_version() {
        let project = crate::model::Project {
            version: "99".to_string(),
            canvas: crate::model::Canvas {
                width: 100,
                height: 100,
                background: [0, 0, 0, 255],
            },
            timeline: crate::model::Timeline {
                fps: crate::Rational::new(30, 1),
                duration_frames: 10,
            },
            sources: Vec::new(),
            layers: vec![crate::model::Layer {
                id: "layer_0".to_string(),
                items: Vec::new(),
            }],
            audio: crate::model::AudioMix::default(),
        };

        let error = compile_project(&project).expect_err("version should be rejected");
        assert!(matches!(
            error,
            crate::compile::CompileError::UnsupportedVersion(_)
        ));
    }
}
