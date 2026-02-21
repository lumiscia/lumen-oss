use std::collections::{HashMap, HashSet};

use crate::model::{BaseStyle, ClipStyle, StyleValue};

use super::dependency::DependencyNode;
use super::scalar::{CompiledScalarValue, ScalarHandle, compile_optional_scalar};
use super::{
    CompileError, CompiledBaseStyle, CompiledClipStyle, CompiledExpressionBinding,
    CompiledShadowStyle, CompiledStrokeStyle, CompiledTransformStyle,
};

#[derive(Default)]
pub(super) struct PropertyRegistry {
    seen_paths: HashSet<String>,
    pub(super) path_indices: HashMap<String, usize>,
    pub(super) literal_scalars: Vec<(usize, f32)>,
    pub(super) expression_scalars: Vec<CompiledExpressionBinding>,
    dependency_nodes: Vec<DependencyNode>,
}

impl PropertyRegistry {
    pub(super) fn dependency_nodes(&self) -> &[DependencyNode] {
        self.dependency_nodes.as_slice()
    }

    pub(super) fn register(
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

pub(super) fn compile_group_style(
    owner_id: &str,
    style: &BaseStyle,
    registry: &mut PropertyRegistry,
    scale: f32,
) -> Result<CompiledBaseStyle, CompileError> {
    compile_base_style(owner_id, owner_id, style, registry, scale)
}

pub(super) fn compile_clip_style(
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

pub(super) fn register_scalar(
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
