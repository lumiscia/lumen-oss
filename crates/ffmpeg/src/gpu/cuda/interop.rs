#![cfg(target_os = "linux")]

use std::{
    ffi::CStr,
    os::fd::{AsRawFd, IntoRawFd, OwnedFd},
    sync::Arc,
};

use cudarc::driver::{result, sys};

use super::{CudaContext, CudaVideoFrame};

/// Vulkan memory mapped into CUDA as linear device memory.
#[derive(Clone)]
pub struct ImportedCudaExternalBuffer {
    inner: Arc<ImportedBuffer>,
}

struct ImportedBuffer {
    context: CudaContext,
    external_memory: sys::CUexternalMemory,
    /// Zero until the external memory has been mapped.
    device_ptr: sys::CUdeviceptr,
    size: u64,
}

// SAFETY: The handles are plain CUDA driver handles, usable from any thread with the context bound,
// and `Drop` binds it.
unsafe impl Send for ImportedBuffer {}
unsafe impl Sync for ImportedBuffer {}

impl ImportedCudaExternalBuffer {
    /// A view of the buffer as an NV12 frame laid out as [`CudaVideoFrame`] describes, which keeps
    /// the import alive.
    pub fn nv12_frame(
        &self,
        width: u32,
        height: u32,
        pitch: u64,
        pts: Option<i64>,
    ) -> Result<CudaVideoFrame, String> {
        let rows = u64::from(height) + u64::from(height.div_ceil(2));
        if pitch < u64::from(width.next_multiple_of(2)) || pitch * rows > self.inner.size {
            return Err(format!(
                "a {width}x{height} NV12 frame with pitch {pitch} doesn't fit the {}-byte imported buffer",
                self.inner.size
            ));
        }
        Ok(CudaVideoFrame::from_retained_device_ptr(
            self.inner.device_ptr,
            width,
            height,
            pitch,
            pts,
            self.inner.clone(),
        ))
    }
}

impl Drop for ImportedBuffer {
    fn drop(&mut self) {
        let _ = self.context.set_current();
        if self.device_ptr != 0 {
            let _ = unsafe { sys::cuMemFree_v2(self.device_ptr) };
        }
        let _ = unsafe { result::external_memory::destroy_external_memory(self.external_memory) };
    }
}

/// Maps the first `size` bytes of Vulkan memory exported as an opaque fd into `context`. CUDA owns
/// `fd` once the import succeeds; on failure it is closed here.
pub fn import_owned_vulkan_opaque_fd_buffer(
    context: &CudaContext,
    fd: OwnedFd,
    allocation_size: u64,
    size: u64,
) -> Result<ImportedCudaExternalBuffer, String> {
    context.set_current()?;
    let external_memory = unsafe {
        result::external_memory::import_external_memory_opaque_fd(fd.as_raw_fd(), allocation_size)
    }
    .map_err(driver_error)?;
    let _ = fd.into_raw_fd();
    let mut buffer = ImportedBuffer {
        context: context.clone(),
        external_memory,
        device_ptr: 0,
        size,
    };
    buffer.device_ptr =
        unsafe { result::external_memory::get_mapped_buffer(external_memory, 0, size) }
            .map_err(driver_error)?;
    Ok(ImportedCudaExternalBuffer {
        inner: Arc::new(buffer),
    })
}

fn driver_error(error: cudarc::driver::result::DriverError) -> String {
    error.to_string()
}

pub fn device_pci_bus_id(device: sys::CUdevice) -> Result<String, String> {
    let mut pci_bus_id = [0i8; 64];
    unsafe {
        sys::cuDeviceGetPCIBusId(pci_bus_id.as_mut_ptr(), pci_bus_id.len() as i32, device)
            .result()
            .map_err(driver_error)?;
    }
    Ok(unsafe { CStr::from_ptr(pci_bus_id.as_ptr()) }
        .to_string_lossy()
        .into_owned())
}
