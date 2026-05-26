#[cfg(target_os = "linux")]
mod driver;
#[cfg(target_os = "linux")]
mod interop;
#[cfg(target_os = "linux")]
mod kernel;

mod frame;

pub use frame::{
    BACKEND, CudaDecodedFrame, CudaExternalMemoryHandle, CudaExternalSemaphoreHandle,
    CudaVideoFrame, VulkanToCudaExport,
};

#[cfg(target_os = "linux")]
pub use driver::{
    CudaContext, CudaDeviceAllocation, CudaDeviceInfo, CudaDriver, CudaNv12ToRgbaConverter,
    ImportedCudaExternalImage, import_owned_vulkan_opaque_fd_image, import_vulkan_opaque_fd_image,
};
