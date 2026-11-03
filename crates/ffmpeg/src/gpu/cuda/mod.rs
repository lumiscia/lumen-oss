#[cfg(target_os = "linux")]
mod driver;
#[cfg(target_os = "linux")]
mod interop;

mod frame;

pub use frame::{BACKEND, CudaVideoFrame};

#[cfg(target_os = "linux")]
pub use driver::{
    CudaContext, CudaDeviceAllocation, CudaDeviceInfo, CudaDriver, ImportedCudaExternalImage,
    import_owned_vulkan_opaque_fd_image,
};
