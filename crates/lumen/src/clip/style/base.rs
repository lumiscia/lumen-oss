use skia_safe::BlendMode;

use crate::clip::style::{StyleProperty, resolve_style_value_or};

#[derive(Debug, Clone)]
pub struct BaseStyle {
    pub visible: StyleProperty<bool>,
    pub opacity: StyleProperty<f32>,
    pub blend_mode: BlendMode,
    pub blur: StyleProperty<f32>,
    pub shadow: Option<ShadowStyle>,
    pub transform: TransformStyle,
    pub alignment: [StyleProperty<f32>; 2],
}

#[derive(Debug, Clone, PartialEq)]
pub struct TransformStyle {
    pub translate: StyleProperty<f32>,
    pub scale: StyleProperty<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShadowStyle {
    pub offset_x: StyleProperty<f32>,
    pub offset_y: StyleProperty<f32>,
    pub blur: StyleProperty<f32>,
    pub color: [StyleProperty<u8>; 4],
}

#[derive(Debug, Clone, Copy)]
pub struct ResolvedBaseStyle {
    pub visible: bool,
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub blur: f32,
    pub translate: f32,
    pub scale: f32,
    pub align_x: f32,
    pub align_y: f32,
    pub shadow: Option<ResolvedShadowStyle>,
}

#[derive(Debug, Clone, Copy)]
pub struct ResolvedShadowStyle {
    pub offset_x: f32,
    pub offset_y: f32,
    pub blur: f32,
    pub color: [u8; 4],
}

pub fn resolve_base_style(style: &BaseStyle) -> ResolvedBaseStyle {
    let shadow = style.shadow.as_ref().map(|shadow| ResolvedShadowStyle {
        offset_x: resolve_style_value_or(&shadow.offset_x, 0.0),
        offset_y: resolve_style_value_or(&shadow.offset_y, 0.0),
        blur: resolve_style_value_or(&shadow.blur, 0.0),
        color: [
            resolve_style_value_or(&shadow.color[0], 0),
            resolve_style_value_or(&shadow.color[1], 0),
            resolve_style_value_or(&shadow.color[2], 0),
            resolve_style_value_or(&shadow.color[3], 0),
        ],
    });

    ResolvedBaseStyle {
        visible: resolve_style_value_or(&style.visible, true),
        opacity: resolve_style_value_or(&style.opacity, 1.0).clamp(0.0, 1.0),
        blend_mode: style.blend_mode,
        blur: resolve_style_value_or(&style.blur, 0.0).max(0.0),
        translate: resolve_style_value_or(&style.transform.translate, 0.0),
        scale: resolve_style_value_or(&style.transform.scale, 1.0).max(0.0),
        align_x: resolve_style_value_or(&style.alignment[0], 0.0),
        align_y: resolve_style_value_or(&style.alignment[1], 0.0),
        shadow,
    }
}
