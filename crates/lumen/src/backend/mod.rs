use skia_safe::Surface;

use crate::error::RenderError;

pub(crate) mod software;

#[cfg(feature = "metal")]
pub(crate) mod metal;

#[cfg(feature = "vulkan")]
pub(crate) mod vulkan;

#[cfg(feature = "webgl")]
pub(crate) mod webgl;

pub(crate) struct SurfaceFactory {
    software: software::SoftwareSurfaceFactory,
    #[cfg(feature = "metal")]
    metal: metal::MetalSurfaceFactory,
    #[cfg(feature = "vulkan")]
    vulkan: vulkan::VulkanSurfaceFactory,
    #[cfg(feature = "webgl")]
    webgl: webgl::WebGlSurfaceFactory,
}

impl SurfaceFactory {
    pub(crate) fn new() -> Self {
        Self {
            software: software::SoftwareSurfaceFactory::new(),
            #[cfg(feature = "metal")]
            metal: metal::MetalSurfaceFactory::new(),
            #[cfg(feature = "vulkan")]
            vulkan: vulkan::VulkanSurfaceFactory::new(),
            #[cfg(feature = "webgl")]
            webgl: webgl::WebGlSurfaceFactory::new(),
        }
    }

    pub(crate) fn create_surface(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<Surface, RenderError> {
        #[cfg(feature = "metal")]
        if let Some(surface) = self.metal.create_surface(width, height) {
            return Ok(surface);
        }

        #[cfg(feature = "vulkan")]
        if let Some(surface) = self.vulkan.create_surface(width, height) {
            return Ok(surface);
        }

        #[cfg(feature = "webgl")]
        if let Some(surface) = self.webgl.create_surface(width, height) {
            return Ok(surface);
        }

        self.software
            .create_surface(width, height)
            .ok_or(RenderError::SurfaceAllocation { width, height })
    }
}

impl Default for SurfaceFactory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(feature = "metal", feature = "vulkan", feature = "webgl"))]
pub(super) fn create_gpu_surface(
    context: &mut skia_safe::gpu::DirectContext,
    width: u32,
    height: u32,
) -> Option<Surface> {
    let width = i32::try_from(width.max(1)).ok()?;
    let height = i32::try_from(height.max(1)).ok()?;
    let info = skia_safe::ImageInfo::new_n32_premul((width, height), None);
    skia_safe::gpu::surfaces::render_target(
        context,
        skia_safe::gpu::Budgeted::Yes,
        &info,
        None::<usize>,
        Some(skia_safe::gpu::SurfaceOrigin::TopLeft),
        None,
        Some(false),
        Some(false),
    )
}
