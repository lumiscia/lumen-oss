use skia_safe::BlendMode;

use crate::clip::style::{StyleContext, StyleProperty};

#[derive(Debug, Clone)]
pub struct BaseStyle {
    pub visible: StyleProperty<bool>,
    pub opacity: StyleProperty<f32>,
    pub blend_mode: BlendMode,
    pub blur: StyleProperty<f32>,
    pub shadows: Vec<ShadowStyle>,
    pub clip_radius: [StyleProperty<f32>; 4],
    pub transform: TransformStyle,
    pub alignment: [StyleProperty<f32>; 2],
    pub mask: Option<Mask>,
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
    pub spread: StyleProperty<f32>,
    pub inset: bool,
    pub color: [StyleProperty<u8>; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub enum PathCommand {
    MoveTo {
        x: f32,
        y: f32,
    },
    LineTo {
        x: f32,
        y: f32,
    },
    QuadTo {
        x1: f32,
        y1: f32,
        x: f32,
        y: f32,
    },
    CubicTo {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        x: f32,
        y: f32,
    },
    Close,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MaskShape {
    Rectangle {
        x: StyleProperty<f32>,
        y: StyleProperty<f32>,
        width: StyleProperty<f32>,
        height: StyleProperty<f32>,
        corner_radius: [StyleProperty<f32>; 4],
    },
    Ellipse {
        cx: StyleProperty<f32>,
        cy: StyleProperty<f32>,
        rx: StyleProperty<f32>,
        ry: StyleProperty<f32>,
    },
    Path {
        data: Vec<PathCommand>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum MaskSource {
    Shape(MaskShape),
    Bitmap { source: String },
    Clip { clip_id: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mask {
    pub source: MaskSource,
    pub inverted: bool,
}

#[derive(Debug, Clone)]
pub struct ResolvedBaseStyle {
    pub visible: bool,
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub blur: f32,
    pub clip_radius: [f32; 4],
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
    pub shadows: Vec<ResolvedShadowStyle>,
    pub mask: Option<ResolvedMask>,
}

#[derive(Debug, Clone, Copy)]
pub struct ResolvedShadowStyle {
    pub offset_x: f32,
    pub offset_y: f32,
    pub blur: f32,
    pub spread: f32,
    pub inset: bool,
    pub color: [u8; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedMaskShape {
    Rectangle {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        corner_radius: [f32; 4],
    },
    Ellipse {
        cx: f32,
        cy: f32,
        rx: f32,
        ry: f32,
    },
    Path {
        data: Vec<PathCommand>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedMaskSource {
    Shape(ResolvedMaskShape),
    Bitmap { source: String },
    Clip { clip_id: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedMask {
    pub source: ResolvedMaskSource,
    pub inverted: bool,
}

impl BaseStyle {
    pub fn resolve(&self, ctx: &StyleContext<'_>) -> ResolvedBaseStyle {
        let shadows = self
            .shadows
            .iter()
            .map(|shadow| ResolvedShadowStyle {
                offset_x: shadow.offset_x.resolve_or(ctx, 0.0),
                offset_y: shadow.offset_y.resolve_or(ctx, 0.0),
                blur: shadow.blur.resolve_or(ctx, 0.0).max(0.0),
                spread: shadow.spread.resolve_or(ctx, 0.0),
                inset: shadow.inset,
                color: [
                    shadow.color[0].resolve_or(ctx, 0),
                    shadow.color[1].resolve_or(ctx, 0),
                    shadow.color[2].resolve_or(ctx, 0),
                    shadow.color[3].resolve_or(ctx, 0),
                ],
            })
            .collect::<Vec<_>>();
        let mask = self.resolve_mask(ctx);

        ResolvedBaseStyle {
            visible: self.visible.resolve_or(ctx, true),
            opacity: self.opacity.resolve_or(ctx, 1.0).clamp(0.0, 1.0),
            blend_mode: self.blend_mode,
            blur: self.blur.resolve_or(ctx, 0.0).max(0.0),
            clip_radius: [
                self.clip_radius[0].resolve_or(ctx, 0.0).max(0.0),
                self.clip_radius[1].resolve_or(ctx, 0.0).max(0.0),
                self.clip_radius[2].resolve_or(ctx, 0.0).max(0.0),
                self.clip_radius[3].resolve_or(ctx, 0.0).max(0.0),
            ],
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
            shadows,
            mask,
        }
    }

    fn resolve_mask(&self, ctx: &StyleContext<'_>) -> Option<ResolvedMask> {
        let mask = self.mask.as_ref()?;
        let source = match &mask.source {
            MaskSource::Shape(shape) => ResolvedMaskSource::Shape(match shape {
                MaskShape::Rectangle {
                    x,
                    y,
                    width,
                    height,
                    corner_radius,
                } => ResolvedMaskShape::Rectangle {
                    x: x.resolve_or(ctx, 0.0),
                    y: y.resolve_or(ctx, 0.0),
                    width: width.resolve_or(ctx, 0.0).max(0.0),
                    height: height.resolve_or(ctx, 0.0).max(0.0),
                    corner_radius: [
                        corner_radius[0].resolve_or(ctx, 0.0).max(0.0),
                        corner_radius[1].resolve_or(ctx, 0.0).max(0.0),
                        corner_radius[2].resolve_or(ctx, 0.0).max(0.0),
                        corner_radius[3].resolve_or(ctx, 0.0).max(0.0),
                    ],
                },
                MaskShape::Ellipse { cx, cy, rx, ry } => ResolvedMaskShape::Ellipse {
                    cx: cx.resolve_or(ctx, 0.0),
                    cy: cy.resolve_or(ctx, 0.0),
                    rx: rx.resolve_or(ctx, 0.0).max(0.0),
                    ry: ry.resolve_or(ctx, 0.0).max(0.0),
                },
                MaskShape::Path { data } => ResolvedMaskShape::Path { data: data.clone() },
            }),
            MaskSource::Bitmap { source } => ResolvedMaskSource::Bitmap {
                source: source.clone(),
            },
            MaskSource::Clip { clip_id } => ResolvedMaskSource::Clip {
                clip_id: clip_id.clone(),
            },
        };

        Some(ResolvedMask {
            source,
            inverted: mask.inverted,
        })
    }
}

#[cfg(test)]
mod tests {
    use skia_safe::BlendMode;

    use super::{
        BaseStyle, Mask, MaskShape, MaskSource, ResolvedMaskShape, ShadowStyle, TransformStyle,
    };
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
            shadows: Vec::new(),
            clip_radius: [literal(-1.0), literal(2.0), literal(3.0), literal(4.0)],
            transform: TransformStyle {
                translate: [literal(12.0), literal(-3.0)],
                scale: [literal(-2.0), literal(0.5)],
                rotation: literal(15.0),
                skew: [literal(5.0), literal(-10.0)],
                origin: [literal(0.25), literal(0.75)],
            },
            alignment: [literal(0.25), literal(0.75)],
            mask: None,
        };

        let resolved = style.resolve(&StyleContext::new(0));

        assert!(resolved.visible);
        assert_eq!(resolved.opacity, 1.0);
        assert_eq!(resolved.blur, 0.0);
        assert_eq!(resolved.clip_radius, [0.0, 2.0, 3.0, 4.0]);
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
            shadows: vec![ShadowStyle {
                offset_x: literal(4.0),
                offset_y: literal(6.0),
                blur: literal(8.0),
                spread: literal(2.0),
                inset: true,
                color: [literal(10), literal(20), literal(30), literal(40)],
            }],
            clip_radius: [literal(0.0), literal(0.0), literal(0.0), literal(0.0)],
            transform: TransformStyle {
                translate: [literal(0.0), literal(0.0)],
                scale: [literal(1.0), literal(1.0)],
                rotation: literal(0.0),
                skew: [literal(0.0), literal(0.0)],
                origin: [literal(0.0), literal(0.0)],
            },
            alignment: [literal(0.0), literal(0.0)],
            mask: None,
        };

        let resolved = style.resolve(&StyleContext::new(0));
        let shadow = resolved.shadows.first().expect("shadow should resolve");

        assert_eq!(shadow.offset_x, 4.0);
        assert_eq!(shadow.offset_y, 6.0);
        assert_eq!(shadow.blur, 8.0);
        assert_eq!(shadow.spread, 2.0);
        assert!(shadow.inset);
        assert_eq!(shadow.color, [10, 20, 30, 40]);
    }

    #[test]
    fn resolve_mask_shape_properties() {
        let style = BaseStyle {
            visible: literal(true),
            opacity: literal(1.0),
            blend_mode: BlendMode::SrcOver,
            blur: literal(0.0),
            shadows: Vec::new(),
            clip_radius: [literal(0.0), literal(0.0), literal(0.0), literal(0.0)],
            transform: TransformStyle {
                translate: [literal(0.0), literal(0.0)],
                scale: [literal(1.0), literal(1.0)],
                rotation: literal(0.0),
                skew: [literal(0.0), literal(0.0)],
                origin: [literal(0.0), literal(0.0)],
            },
            alignment: [literal(0.0), literal(0.0)],
            mask: Some(Mask {
                source: MaskSource::Shape(MaskShape::Rectangle {
                    x: literal(10.0),
                    y: literal(12.0),
                    width: literal(40.0),
                    height: literal(20.0),
                    corner_radius: [literal(1.0), literal(2.0), literal(3.0), literal(4.0)],
                }),
                inverted: true,
            }),
        };

        let resolved = style.resolve(&StyleContext::new(0));
        let Some(mask) = resolved.mask else {
            panic!("mask should resolve")
        };
        assert!(mask.inverted);
        let super::ResolvedMaskSource::Shape(ResolvedMaskShape::Rectangle {
            x,
            y,
            width,
            height,
            corner_radius,
        }) = mask.source
        else {
            panic!("expected rectangle mask")
        };

        assert_eq!(x, 10.0);
        assert_eq!(y, 12.0);
        assert_eq!(width, 40.0);
        assert_eq!(height, 20.0);
        assert_eq!(corner_radius, [1.0, 2.0, 3.0, 4.0]);
    }
}
