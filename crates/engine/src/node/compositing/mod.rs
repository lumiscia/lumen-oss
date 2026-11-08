pub mod boolean;
pub mod clip;
pub mod merge;
pub mod raster_multimerge;
pub mod switch;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(u8)]
#[non_exhaustive]
#[delegate(kind = "enum", expressions)]
pub enum BlendMode {
    #[default]
    Normal = 0,
    Multiply = 1,
    Screen = 2,
    Overlay = 3,
    Darken = 4,
    Lighten = 5,
    Add = 6,
    Difference = 7,
    Exclusion = 8,
    ColorDodge = 9,
    ColorBurn = 10,
    HardLight = 11,
    SoftLight = 12,
    Hue = 13,
    Saturation = 14,
    Color = 15,
    Luminosity = 16,
}
