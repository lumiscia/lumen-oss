use std::collections::{HashMap, HashSet};

use crate::model::{LayoutNode, LayoutNodeKind, SourceMedia, StyleValue};

use super::scalar::{ScalarHandle, compile_optional_scalar};
use super::sources::CompiledSourceRef;
use super::style::{PropertyRegistry, register_scalar};
use super::{CompileError, CompiledLayoutNode, CompiledLayoutNodeKind, CompiledLayoutNodeStyle};

pub(super) fn compile_layout_node(
    node: &LayoutNode,
    layout_ids: &mut HashSet<String>,
    source_lookup: &HashMap<String, CompiledSourceRef>,
    registry: &mut PropertyRegistry,
    scale: f32,
    owner_id: &str,
) -> Result<CompiledLayoutNode, CompileError> {
    if !layout_ids.insert(node.id.clone()) {
        return Err(CompileError::DuplicateLayoutNodeId(node.id.clone()));
    }

    let width = compile_optional_layout_scalar(
        owner_id,
        node.id.as_str(),
        "width",
        node.style.width.as_ref(),
        0.0,
        true,
        registry,
        scale,
    )?;
    let height = compile_optional_layout_scalar(
        owner_id,
        node.id.as_str(),
        "height",
        node.style.height.as_ref(),
        0.0,
        true,
        registry,
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
            registry,
            scale,
        )?,
        min_height: compile_optional_layout_scalar(
            owner_id,
            node.id.as_str(),
            "min_height",
            node.style.min_height.as_ref(),
            0.0,
            true,
            registry,
            scale,
        )?,
        max_width: compile_optional_layout_scalar(
            owner_id,
            node.id.as_str(),
            "max_width",
            node.style.max_width.as_ref(),
            0.0,
            true,
            registry,
            scale,
        )?,
        max_height: compile_optional_layout_scalar(
            owner_id,
            node.id.as_str(),
            "max_height",
            node.style.max_height.as_ref(),
            0.0,
            true,
            registry,
            scale,
        )?,
        padding_left: compile_optional_layout_scalar(
            owner_id,
            node.id.as_str(),
            "padding_left",
            node.style.padding_left.as_ref(),
            0.0,
            true,
            registry,
            scale,
        )?,
        padding_top: compile_optional_layout_scalar(
            owner_id,
            node.id.as_str(),
            "padding_top",
            node.style.padding_top.as_ref(),
            0.0,
            true,
            registry,
            scale,
        )?,
        padding_right: compile_optional_layout_scalar(
            owner_id,
            node.id.as_str(),
            "padding_right",
            node.style.padding_right.as_ref(),
            0.0,
            true,
            registry,
            scale,
        )?,
        padding_bottom: compile_optional_layout_scalar(
            owner_id,
            node.id.as_str(),
            "padding_bottom",
            node.style.padding_bottom.as_ref(),
            0.0,
            true,
            registry,
            scale,
        )?,
        gap: compile_optional_layout_scalar(
            owner_id,
            node.id.as_str(),
            "gap",
            node.style.gap.as_ref(),
            0.0,
            true,
            registry,
            scale,
        )?,
        grow: compile_optional_layout_scalar(
            owner_id,
            node.id.as_str(),
            "grow",
            node.style.grow.as_ref(),
            0.0,
            false,
            registry,
            scale,
        )?,
        shrink: compile_optional_layout_scalar(
            owner_id,
            node.id.as_str(),
            "shrink",
            node.style.shrink.as_ref(),
            0.0,
            false,
            registry,
            scale,
        )?,
        basis: compile_optional_layout_scalar(
            owner_id,
            node.id.as_str(),
            "basis",
            node.style.basis.as_ref(),
            0.0,
            true,
            registry,
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
            registry,
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
            registry,
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
        registry,
        scale,
    )?;
    register_scalar(
        owner_id,
        node.id.as_str(),
        "y",
        Some(&StyleValue::Value(0.0)),
        0.0,
        true,
        registry,
        scale,
    )?;

    let kind = match &node.kind {
        LayoutNodeKind::Container { children } => {
            let mut compiled_children = Vec::with_capacity(children.len());
            for child in children {
                compiled_children.push(compile_layout_node(
                    child,
                    layout_ids,
                    source_lookup,
                    registry,
                    scale,
                    owner_id,
                )?);
            }
            CompiledLayoutNodeKind::Container {
                children: compiled_children,
            }
        }
        LayoutNodeKind::Text { content } => CompiledLayoutNodeKind::Text {
            content: content.clone(),
        },
        LayoutNodeKind::Image { source } => {
            let source_ref = resolve_source(source_lookup, source.as_str(), SourceMedia::Image)?;
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

fn resolve_source<'a>(
    source_lookup: &'a HashMap<String, CompiledSourceRef>,
    source_id: &str,
    expected: SourceMedia,
) -> Result<&'a CompiledSourceRef, CompileError> {
    let source = source_lookup
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
