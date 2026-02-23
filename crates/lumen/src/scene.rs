use crate::clip::ClipType;
use crate::clip::style::StyleProperty;
use crate::time::Rational;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendMode {
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
}

impl Default for BlendMode {
    fn default() -> Self {
        Self::Normal
    }
}

impl From<BlendMode> for skia_safe::BlendMode {
    fn from(value: BlendMode) -> Self {
        match value {
            BlendMode::Normal => Self::SrcOver,
            BlendMode::Multiply => Self::Multiply,
            BlendMode::Screen => Self::Screen,
            BlendMode::Overlay => Self::Overlay,
            BlendMode::Darken => Self::Darken,
            BlendMode::Lighten => Self::Lighten,
        }
    }
}

#[derive(Debug)]
pub struct Layer {
    pub id: String,
    pub clips: Vec<ClipType>,
    pub blend_mode: BlendMode,
    pub opacity: StyleProperty<f32>,
    pub visible: bool,
}

#[derive(Debug)]
pub struct Scene {
    pub width: u32,
    pub height: u32,
    pub frame_rate: Rational,
    pub duration_frames: u32,
    pub layers: Vec<Layer>,
}
