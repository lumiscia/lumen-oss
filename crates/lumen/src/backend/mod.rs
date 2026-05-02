use skia_safe::Surface;

use crate::error::RenderError;

#[cfg(feature = "metal")]
pub(crate) mod metal;

#[cfg(feature = "vulkan")]
pub(crate) mod vulkan;

#[cfg(all(feature = "webgl", target_arch = "wasm32", target_os = "unknown"))]
pub(crate) mod webgl;

pub(crate) struct SurfaceFactory {
    #[cfg(feature = "metal")]
    metal: metal::MetalSurfaceFactory,
    #[cfg(feature = "vulkan")]
    vulkan: vulkan::VulkanSurfaceFactory,
    #[cfg(all(feature = "webgl", target_arch = "wasm32", target_os = "unknown"))]
    webgl: webgl::WebGlSurfaceFactory,
}

impl SurfaceFactory {
    pub(crate) fn new() -> Self {
        Self {
            #[cfg(feature = "metal")]
            metal: metal::MetalSurfaceFactory::new(),
            #[cfg(feature = "vulkan")]
            vulkan: vulkan::VulkanSurfaceFactory::new(),
            #[cfg(all(feature = "webgl", target_arch = "wasm32", target_os = "unknown"))]
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

        #[cfg(all(feature = "webgl", target_arch = "wasm32", target_os = "unknown"))]
        if let Some(surface) = self.webgl.create_surface(width, height) {
            return Ok(surface);
        }

        Err(RenderError::SurfaceAllocation { width, height })
    }

    pub(crate) fn flush(&mut self) {
        #[cfg(feature = "metal")]
        self.metal.flush();

        #[cfg(feature = "vulkan")]
        self.vulkan.flush();

        #[cfg(all(feature = "webgl", target_arch = "wasm32", target_os = "unknown"))]
        self.webgl.flush();
    }
}

impl Default for SurfaceFactory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(
    feature = "metal",
    feature = "vulkan",
    all(feature = "webgl", target_arch = "wasm32", target_os = "unknown")
))]
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
