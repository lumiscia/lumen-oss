use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use thiserror::Error;

use crate::expression::ExprParseError;
use crate::model::{ClipContent, ClipItem, Layer, LayerItem, SourceMedia};

mod dependency;
mod frame_index;
mod layout;
mod operation;
mod scalar;
mod sources;
mod style;
mod validate;

use dependency::{DependencyError, build_eval_order};
use frame_index::build_frame_index;
use layout::compile_layout_node;
pub use operation::{
    CompiledBaseStyle, CompiledClipNode, CompiledClipStyle, CompiledExpressionBinding,
    CompiledGroupNode, CompiledImage, CompiledLayer, CompiledLayerItem, CompiledLayoutClip,
    CompiledLayoutNode, CompiledLayoutNodeKind, CompiledLayoutNodeStyle, CompiledOperation,
    CompiledOperationKind, CompiledShadowStyle, CompiledShape, CompiledSource, CompiledStrokeStyle,
    CompiledText, CompiledTimeline, CompiledTransformStyle, CompiledVideo, ResolvedTransform,
    RuntimeEvalError, RuntimeFrameContext,
};
pub use scalar::ScalarHandle;
use sources::{CompiledSourceRef, compile_sources, sorted_sources};
use style::{PropertyRegistry, compile_clip_style, compile_group_style};
use validate::{validate_item_ids, validate_project};

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
    validate_project(project, scale)?;

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
    let frame_index = build_frame_index(total_frames, &ctx.operations);

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

fn scale_project_canvas(project: &mut crate::model::Project, scale: f32) {
    if (scale - 1.0).abs() <= f32::EPSILON {
        return;
    }

    let width = (project.canvas.width as f32 * scale).round().max(1.0);
    let height = (project.canvas.height as f32 * scale).round().max(1.0);
    project.canvas.width = width as u32;
    project.canvas.height = height as u32;
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
            root: compile_layout_node(
                root,
                &mut ctx.layout_ids,
                &ctx.source_lookup,
                &mut ctx.registry,
                scale,
                clip.id.as_str(),
            )?,
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
