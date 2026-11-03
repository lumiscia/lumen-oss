pub mod alpha_premultiply;
pub mod blur;
pub mod color_grade;
pub mod crop;
pub mod exposure;
pub(crate) mod gaussian;
pub mod hue_saturation;
pub mod levels;
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
    /// Blends the four nearest texels.
    #[default]
    Linear = 1,
}
