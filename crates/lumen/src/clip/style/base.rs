use skia_safe::BlendMode;

use crate::clip::style::{StyleContext, StyleProperty};

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
    pub translate: [StyleProperty<f32>; 2],
    pub scale: [StyleProperty<f32>; 2],
    pub rotation: StyleProperty<f32>,
    pub skew: [StyleProperty<f32>; 2],
    pub origin: [StyleProperty<f32>; 2],
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
    pub translate_x: f32,
    pub translate_y: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub rotation_degrees: f32,
    pub skew_x_degrees: f32,
    pub skew_y_degrees: f32,
    pub origin_x: f32,
    pub origin_y: f32,
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

impl BaseStyle {
    pub fn resolve(&self, ctx: &StyleContext<'_>) -> ResolvedBaseStyle {
        let shadow = self.shadow.as_ref().map(|shadow| ResolvedShadowStyle {
            offset_x: shadow.offset_x.resolve_or(ctx, 0.0),
            offset_y: shadow.offset_y.resolve_or(ctx, 0.0),
            blur: shadow.blur.resolve_or(ctx, 0.0),
            color: [
                shadow.color[0].resolve_or(ctx, 0),
                shadow.color[1].resolve_or(ctx, 0),
                shadow.color[2].resolve_or(ctx, 0),
                shadow.color[3].resolve_or(ctx, 0),
            ],
        });

        ResolvedBaseStyle {
            visible: self.visible.resolve_or(ctx, true),
            opacity: self.opacity.resolve_or(ctx, 1.0).clamp(0.0, 1.0),
            blend_mode: self.blend_mode,
            blur: self.blur.resolve_or(ctx, 0.0).max(0.0),
            translate_x: self.transform.translate[0].resolve_or(ctx, 0.0),
            translate_y: self.transform.translate[1].resolve_or(ctx, 0.0),
            scale_x: self.transform.scale[0].resolve_or(ctx, 1.0),
            scale_y: self.transform.scale[1].resolve_or(ctx, 1.0),
            rotation_degrees: self.transform.rotation.resolve_or(ctx, 0.0),
            skew_x_degrees: self.transform.skew[0].resolve_or(ctx, 0.0),
            skew_y_degrees: self.transform.skew[1].resolve_or(ctx, 0.0),
            origin_x: self.transform.origin[0].resolve_or(ctx, 0.0),
            origin_y: self.transform.origin[1].resolve_or(ctx, 0.0),
            align_x: self.alignment[0].resolve_or(ctx, 0.0),
            align_y: self.alignment[1].resolve_or(ctx, 0.0),
            shadow,
        }
    }
}

#[cfg(test)]
mod tests {
    use skia_safe::BlendMode;

    use super::{BaseStyle, ShadowStyle, TransformStyle};
    use crate::clip::style::{StyleContext, StyleProperty, StyleValue};

    fn literal<T>(value: T) -> StyleProperty<T> {
        StyleProperty::Value(StyleValue::Literal(value))
    }

    #[test]
    fn resolve_clamps_and_applies_defaults() {
        let style = BaseStyle {
            visible: literal(true),
            opacity: literal(5.0),
            blend_mode: BlendMode::SrcOver,
            blur: literal(-4.0),
            shadow: None,
            transform: TransformStyle {
                translate: [literal(12.0), literal(-3.0)],
                scale: [literal(-2.0), literal(0.5)],
                rotation: literal(15.0),
                skew: [literal(5.0), literal(-10.0)],
                origin: [literal(0.25), literal(0.75)],
            },
            alignment: [literal(0.25), literal(0.75)],
        };

        let resolved = style.resolve(&StyleContext::new(0));

        assert!(resolved.visible);
        assert_eq!(resolved.opacity, 1.0);
        assert_eq!(resolved.blur, 0.0);
        assert_eq!(resolved.translate_x, 12.0);
        assert_eq!(resolved.translate_y, -3.0);
        assert_eq!(resolved.scale_x, -2.0);
        assert_eq!(resolved.scale_y, 0.5);
        assert_eq!(resolved.rotation_degrees, 15.0);
        assert_eq!(resolved.skew_x_degrees, 5.0);
        assert_eq!(resolved.skew_y_degrees, -10.0);
        assert_eq!(resolved.origin_x, 0.25);
        assert_eq!(resolved.origin_y, 0.75);
        assert_eq!(resolved.align_x, 0.25);
        assert_eq!(resolved.align_y, 0.75);
    }

    #[test]
    fn resolve_uses_shadow_defaults_per_channel() {
        let style = BaseStyle {
            visible: literal(true),
            opacity: literal(1.0),
            blend_mode: BlendMode::SrcOver,
            blur: literal(0.0),
            shadow: Some(ShadowStyle {
                offset_x: literal(4.0),
                offset_y: literal(6.0),
                blur: literal(8.0),
                color: [literal(10), literal(20), literal(30), literal(40)],
            }),
            transform: TransformStyle {
                translate: [literal(0.0), literal(0.0)],
                scale: [literal(1.0), literal(1.0)],
                rotation: literal(0.0),
                skew: [literal(0.0), literal(0.0)],
                origin: [literal(0.0), literal(0.0)],
            },
            alignment: [literal(0.0), literal(0.0)],
        };

        let resolved = style.resolve(&StyleContext::new(0));
        let shadow = resolved.shadow.expect("shadow should resolve");

        assert_eq!(shadow.offset_x, 4.0);
        assert_eq!(shadow.offset_y, 6.0);
        assert_eq!(shadow.blur, 8.0);
        assert_eq!(shadow.color, [10, 20, 30, 40]);
    }
}
