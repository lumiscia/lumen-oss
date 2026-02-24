pub(crate) struct SoftwareSurfaceFactory;

impl SoftwareSurfaceFactory {
    pub(crate) fn new() -> Self {
        Self
    }

    pub(crate) fn create_surface(&mut self, width: u32, height: u32) -> Option<skia_safe::Surface> {
        let width = i32::try_from(width).ok()?;
        let height = i32::try_from(height).ok()?;
        skia_safe::surfaces::raster_n32_premul((width, height))
    }
}
