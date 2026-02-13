use skia_safe::surfaces;

use crate::backend::RenderError;

pub(super) fn create_surface(
    width: u32,
    height: u32,
) -> Result<skia_safe::Surface, RenderError> {
    surfaces::raster_n32_premul((width as i32, height as i32))
        .ok_or_else(|| RenderError::SurfaceCreation("failed to create raster surface".into()))
}
