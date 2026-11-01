#![allow(clippy::enum_variant_names)]

pub(crate) const MAX_GRADIENT_STOPS: usize = 8;

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "json", serde(rename_all = "snake_case"))]
#[delegate(kind = "enum")]
pub enum PaintKind {
    #[default]
    #[cfg_attr(feature = "json", serde(alias = "linear"))]
    LinearGradient,
    #[cfg_attr(feature = "json", serde(alias = "radial"))]
    RadialGradient,
    #[cfg_attr(feature = "json", serde(alias = "conic"))]
    ConicGradient,
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "json", serde(rename_all = "snake_case"))]
#[delegate(kind = "enum")]
pub enum GradientUnits {
    #[default]
    ObjectBoundingBox,
    #[cfg_attr(feature = "json", serde(alias = "userSpaceOnUse"))]
    UserSpace,
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "json", serde(rename_all = "snake_case"))]
#[delegate(kind = "enum")]
pub enum GradientSpread {
    #[default]
    Pad,
    Repeat,
    Reflect,
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "json", serde(rename_all = "snake_case"))]
#[delegate(kind = "enum")]
pub enum GradientInterpolation {
    #[default]
    #[enum_option(label = "sRGB")]
    Srgb,
    #[cfg_attr(feature = "json", serde(alias = "linear"))]
    #[enum_option(label = "Linear sRGB")]
    LinearSrgb,
}

/// A color at a point along a gradient. Gradient fields take literals only.
#[derive(Debug, Clone, Default, PartialEq, lumen_macros::Delegate)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
pub struct GradientStop {
    /// Position along the gradient from 0 to 1. Values outside that range are clamped.
    #[meta(step = 0.01, constant)]
    pub offset: f32,
    /// Color at the offset.
    #[meta(constant)]
    pub color: [u8; 4],
}

/// A linear, radial or conic gradient. Gradient fields take literals only.
#[derive(Debug, Clone, PartialEq, lumen_macros::Delegate)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
pub struct GradientPaint {
    /// Gradient shape. Required.
    #[meta(id = "type", name = "Type")]
    pub kind: PaintKind,
    /// Whether points and radii are fractions of the shape's bounding box or pixels.
    #[meta()]
    pub units: GradientUnits,
    /// How the gradient continues past its first and last stops.
    #[meta()]
    pub spread: GradientSpread,
    /// Color space the stops blend in.
    #[meta()]
    pub interpolation: GradientInterpolation,
    /// Start point of a linear gradient.
    #[meta(constant)]
    pub start: [f32; 2],
    /// End point of a linear gradient.
    #[meta(constant)]
    pub end: [f32; 2],
    /// Center of a radial or conic gradient.
    #[meta(constant)]
    pub center: [f32; 2],
    /// Horizontal and vertical radius of a radial gradient. A number sets both.
    #[meta(constant)]
    pub radius: [f32; 2],
    /// Start angle of a conic gradient in degrees.
    #[meta(step = 1, constant)]
    pub angle: f32,
    /// Colors along the gradient. Required, with at least one stop.
    #[meta()]
    pub stops: Vec<GradientStop>,
}

#[derive(Debug, Clone, PartialEq, lumen_macros::Delegate)]
#[cfg_attr(feature = "json", derive(serde::Serialize, serde::Deserialize))]
#[delegate(kind = "paint", expressions)]
pub enum Paint {
    SolidColor(u8, u8, u8, u8),
    Gradient(GradientPaint),
}

impl Default for Paint {
    fn default() -> Self {
        Self::solid([0, 0, 0, 255])
    }
}

impl Default for GradientPaint {
    fn default() -> Self {
        Self {
            kind: PaintKind::LinearGradient,
            units: GradientUnits::ObjectBoundingBox,
            spread: GradientSpread::Pad,
            interpolation: GradientInterpolation::Srgb,
            start: [0.0, 0.0],
            end: [1.0, 0.0],
            center: [0.5, 0.5],
            radius: [0.5, 0.5],
            angle: 0.0,
            stops: Vec::new(),
        }
    }
}
