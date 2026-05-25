#![allow(clippy::enum_variant_names)]

pub(crate) const MAX_GRADIENT_STOPS: usize = 8;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, lumen_macros::Delegate)]
pub enum PaintKind {
    #[default]
    LinearGradient,
    RadialGradient,
    ConicGradient,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, lumen_macros::Delegate)]
pub enum GradientUnits {
    #[default]
    ObjectBoundingBox,
    UserSpace,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, lumen_macros::Delegate)]
pub enum GradientSpread {
    #[default]
    Pad,
    Repeat,
    Reflect,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, lumen_macros::Delegate)]
pub enum GradientInterpolation {
    #[default]
    Srgb,
    LinearSrgb,
}

#[derive(Debug, Clone, Default, PartialEq, lumen_macros::Delegate)]
pub struct GradientStop {
    #[meta(min = 0, max = 1, step = 0.01)]
    pub offset: f32,
    #[meta()]
    pub color: [u8; 4],
}

#[derive(Debug, Clone, PartialEq, lumen_macros::Delegate)]
pub struct GradientPaint {
    #[meta()]
    pub kind: PaintKind,
    #[meta()]
    pub units: GradientUnits,
    #[meta()]
    pub spread: GradientSpread,
    #[meta()]
    pub interpolation: GradientInterpolation,
    #[meta()]
    pub start: [f32; 2],
    #[meta()]
    pub end: [f32; 2],
    #[meta()]
    pub center: [f32; 2],
    #[meta()]
    pub radius: [f32; 2],
    #[meta(step = 1)]
    pub angle: f32,
    #[meta()]
    pub stops: Vec<GradientStop>,
}

#[derive(Debug, Clone, PartialEq, lumen_macros::Delegate)]
#[delegate(kind = "paint")]
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
