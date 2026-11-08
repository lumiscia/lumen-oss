#![allow(clippy::enum_variant_names)]

/// The most stops a gradient holds, which the paint shader's fixed arrays also size.
pub const MAX_GRADIENT_STOPS: usize = 8;

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    lumen_macros::NodeEnum,
    lumen_macros::Delegate,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[delegate(kind = "enum")]
pub enum PaintKind {
    #[default]
    #[serde(alias = "linear")]
    LinearGradient,
    #[serde(alias = "radial")]
    RadialGradient,
    #[serde(alias = "conic")]
    ConicGradient,
}

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    lumen_macros::NodeEnum,
    lumen_macros::Delegate,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[delegate(kind = "enum")]
pub enum GradientUnits {
    #[default]
    ObjectBoundingBox,
    #[serde(alias = "userSpaceOnUse")]
    UserSpace,
}

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    lumen_macros::NodeEnum,
    lumen_macros::Delegate,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[delegate(kind = "enum")]
pub enum GradientSpread {
    #[default]
    Pad,
    Repeat,
    Reflect,
}

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    lumen_macros::NodeEnum,
    lumen_macros::Delegate,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[delegate(kind = "enum")]
pub enum GradientInterpolation {
    #[default]
    #[enum_option(label = "sRGB")]
    Srgb,
    #[serde(alias = "linear")]
    #[enum_option(label = "Linear sRGB")]
    LinearSrgb,
}

/// A color at a point along a gradient. Gradient fields take literals only.
#[derive(
    Debug, Clone, Default, PartialEq, lumen_macros::Delegate, serde::Serialize, serde::Deserialize,
)]
pub struct GradientStop {
    /// Position along the gradient from 0 to 1. Values outside that range are clamped.
    #[meta(step = 0.01, constant, required)]
    pub offset: f32,
    /// Color at the offset.
    #[meta(constant, required)]
    pub color: [u8; 4],
}

/// A linear, radial or conic gradient. Gradient fields take literals only.
#[derive(Debug, Clone, PartialEq, lumen_macros::Delegate, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GradientPaint {
    /// Gradient shape. Required.
    #[meta(id = "type", name = "Type", required)]
    #[serde(rename = "type")]
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
    #[serde(deserialize_with = "super::json::deserialize_radius")]
    pub radius: [f32; 2],
    /// Focal point of a radial gradient, where the first stop's color sits. The gradient widens
    /// from it to the circle at `center` and `radius`, which gives an off-centre highlight.
    /// Defaults to `center`. A point on or outside that circle is moved to 99% of the way to it
    /// from `center`, along the same direction.
    #[meta(constant)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focal: Option<[f32; 2]>,
    /// Start angle of a conic gradient in degrees.
    #[meta(step = 1, constant)]
    pub angle: f32,
    /// Colors along the gradient. Required, with one to eight stops.
    #[meta(required)]
    #[serde(deserialize_with = "super::json::deserialize_stops")]
    pub stops: Vec<GradientStop>,
}

#[derive(Debug, Clone, PartialEq, lumen_macros::Delegate)]
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
            focal: None,
            angle: 0.0,
            stops: Vec::new(),
        }
    }
}
