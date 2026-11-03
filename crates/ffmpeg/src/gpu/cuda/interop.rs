#![cfg(target_os = "linux")]

use std::{
    ffi::CStr,
    os::fd::{AsRawFd, IntoRawFd, OwnedFd},
    ptr,
};

use cudarc::driver::{result, sys};

use super::CudaContext;

use cudarc::driver::sys::CUarray_format_enum::CU_AD_FORMAT_UNSIGNED_INT8;
const CUDA_ARRAY3D_COLOR_ATTACHMENT: u32 = 0x20;
use cudarc::driver::sys::CUmemorytype_enum::{CU_MEMORYTYPE_ARRAY, CU_MEMORYTYPE_DEVICE};

pub struct ImportedCudaExternalImage {
    context: CudaContext,
    external_memory: sys::CUexternalMemory,
    /// Null until the external memory has been mapped.
    mipmapped_array: sys::CUmipmappedArray,
    level_zero: sys::CUarray,
}

impl ImportedCudaExternalImage {
    pub fn mipmapped_array_raw(&self) -> usize {
        self.mipmapped_array as usize
    }

    pub fn level_zero_raw(&self) -> usize {
        self.level_zero as usize
    }
}

impl Drop for ImportedCudaExternalImage {
    fn drop(&mut self) {
        let _ = self.context.set_current();
        if !self.mipmapped_array.is_null() {
            let _ = unsafe { sys::cuMipmappedArrayDestroy(self.mipmapped_array) };
        }
        let _ = unsafe { result::external_memory::destroy_external_memory(self.external_memory) };
    }
}

/// Maps Vulkan memory exported as an opaque fd into `context`. CUDA owns `fd` once the import
/// succeeds; on failure it is closed here.
pub fn import_owned_vulkan_opaque_fd_image(
    context: &CudaContext,
    fd: OwnedFd,
    allocation_size: u64,
    width: u32,
    height: u32,
) -> Result<ImportedCudaExternalImage, String> {
    context.set_current()?;
    let external_memory = unsafe {
        result::external_memory::import_external_memory_opaque_fd(fd.as_raw_fd(), allocation_size)
    }
    .map_err(driver_error)?;
    let _ = fd.into_raw_fd();
    let mut image = ImportedCudaExternalImage {
        context: context.clone(),
        external_memory,
        mipmapped_array: ptr::null_mut(),
        level_zero: ptr::null_mut(),
    };

    let array_desc = sys::CUDA_EXTERNAL_MEMORY_MIPMAPPED_ARRAY_DESC {
        offset: 0,
        arrayDesc: sys::CUDA_ARRAY3D_DESCRIPTOR {
            Width: width as usize,
            Height: height as usize,
            Depth: 0,
            Format: CU_AD_FORMAT_UNSIGNED_INT8 as _,
            NumChannels: 4,
            Flags: CUDA_ARRAY3D_COLOR_ATTACHMENT,
        },
        numLevels: 1,
        reserved: [0; 16],
    };
    unsafe {
        sys::cuExternalMemoryGetMappedMipmappedArray(
            &mut image.mipmapped_array,
            external_memory,
            &array_desc,
        )
        .result()
        .map_err(driver_error)?;
        sys::cuMipmappedArrayGetLevel(&mut image.level_zero, image.mipmapped_array, 0)
            .result()
            .map_err(driver_error)?;
    }
    Ok(image)
}

pub fn copy_image_to_rgba_frame(
    source: &ImportedCudaExternalImage,
    destination_device_ptr: sys::CUdeviceptr,
    destination_pitch: usize,
    width: u32,
    height: u32,
) -> Result<(), String> {
    let copy = sys::CUDA_MEMCPY2D {
        srcXInBytes: 0,
        srcY: 0,
        srcMemoryType: CU_MEMORYTYPE_ARRAY,
        srcHost: std::ptr::null(),
        srcDevice: 0,
        srcArray: source.level_zero,
        srcPitch: 0,
        dstXInBytes: 0,
        dstY: 0,
        dstMemoryType: CU_MEMORYTYPE_DEVICE,
        dstHost: std::ptr::null_mut(),
        dstDevice: destination_device_ptr,
        dstArray: std::ptr::null_mut(),
        dstPitch: destination_pitch,
        WidthInBytes: width as usize * 4,
        Height: height as usize,
    };
    unsafe { sys::cuMemcpy2D_v2(&copy) }
        .result()
        .map_err(driver_error)
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
