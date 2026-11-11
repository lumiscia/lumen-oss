pub mod blur;
pub mod color_grade;
pub mod crop;
pub(crate) mod cube_lut;
pub mod directional_blur;
pub mod displacement_map;
pub mod exposure;
pub(crate) mod gaussian;
pub mod glow;
pub mod hue_saturation;
pub mod levels;
pub(crate) mod minify;
pub mod motion_blur;
pub mod opacity;
pub mod resize;
pub mod shadow;
pub mod time_remap;
pub mod transform;
pub mod wgsl_shader;

/// How a raster is read between its texels.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum", expressions)]
pub enum RasterSampling {
    /// Takes the nearest texel.
    Nearest = 0,
    /// Blends the four nearest texels, or averages the texels a pixel covers when it shows a
    /// source smaller than its size.
    #[default]
    Linear = 1,
}
