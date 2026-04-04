pub mod boolean;
pub mod merge;
pub mod raster_multimerge;
pub mod switch;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
#[non_exhaustive]
pub enum BlendMode {
    Normal = 0,
    Multiply = 1,
    Screen = 2,
    Overlay = 3,
    Darken = 4,
    Lighten = 5,
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

impl TryFrom<usize> for BlendMode {
    type Error = &'static str;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(BlendMode::Normal),
            1 => Ok(BlendMode::Multiply),
            2 => Ok(BlendMode::Screen),
            3 => Ok(BlendMode::Overlay),
            4 => Ok(BlendMode::Darken),
            5 => Ok(BlendMode::Lighten),
            _ => Err("failed to convert usize into BlendMode"),
        }
    }
}
