#![cfg(target_os = "linux")]

use std::{ffi::CStr, mem::MaybeUninit, os::fd::OwnedFd};

use cudarc::driver::{result, sys};

use super::CudaExternalMemoryHandle;

use cudarc::driver::sys::CUarray_format_enum::CU_AD_FORMAT_UNSIGNED_INT8;
const CUDA_ARRAY3D_COLOR_ATTACHMENT: u32 = 0x20;
use cudarc::driver::sys::CUmemorytype_enum::{CU_MEMORYTYPE_ARRAY, CU_MEMORYTYPE_DEVICE};

pub struct ImportedCudaExternalImage {
    external_memory: sys::CUexternalMemory,
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
        let _ = unsafe { result::external_memory::destroy_external_memory(self.external_memory) };
    }
}

pub fn import_vulkan_opaque_fd_image(
    memory: CudaExternalMemoryHandle,
    allocation_size: u64,
    width: u32,
    height: u32,
) -> Result<ImportedCudaExternalImage, String> {
    let CudaExternalMemoryHandle::OpaqueFd(fd) = memory;
    let external_memory =
        unsafe { result::external_memory::import_external_memory_opaque_fd(fd, allocation_size) }
            .map_err(driver_error)?;

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

    let mut mipmapped_array = MaybeUninit::<sys::CUmipmappedArray>::uninit();
    if let Err(error) = unsafe {
        sys::cuExternalMemoryGetMappedMipmappedArray(
            mipmapped_array.as_mut_ptr(),
            external_memory,
            &array_desc,
        )
        .result()
    } {
        let _ = unsafe { result::external_memory::destroy_external_memory(external_memory) };
        return Err(driver_error(error));
    }
    let mipmapped_array = unsafe { mipmapped_array.assume_init() };

    let mut level_zero = MaybeUninit::<sys::CUarray>::uninit();
    if let Err(error) = unsafe {
        sys::cuMipmappedArrayGetLevel(level_zero.as_mut_ptr(), mipmapped_array, 0).result()
    } {
        let _ = unsafe { result::external_memory::destroy_external_memory(external_memory) };
        return Err(driver_error(error));
    }

    Ok(ImportedCudaExternalImage {
        external_memory,
        mipmapped_array,
        level_zero: unsafe { level_zero.assume_init() },
    })
}

pub fn import_owned_vulkan_opaque_fd_image(
    fd: OwnedFd,
    allocation_size: u64,
    width: u32,
    height: u32,
) -> Result<ImportedCudaExternalImage, String> {
    use std::os::fd::IntoRawFd;

    import_vulkan_opaque_fd_image(
        CudaExternalMemoryHandle::OpaqueFd(fd.into_raw_fd()),
        allocation_size,
        width,
        height,
    )
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

pub fn copy_rgba_frame_to_image(
    source_device_ptr: sys::CUdeviceptr,
    source_pitch: usize,
    destination: &ImportedCudaExternalImage,
    width: u32,
    height: u32,
) -> Result<(), String> {
    let copy = sys::CUDA_MEMCPY2D {
        srcXInBytes: 0,
        srcY: 0,
        srcMemoryType: CU_MEMORYTYPE_DEVICE,
        srcHost: std::ptr::null(),
        srcDevice: source_device_ptr,
        srcArray: std::ptr::null_mut(),
        srcPitch: source_pitch,
        dstXInBytes: 0,
        dstY: 0,
        dstMemoryType: CU_MEMORYTYPE_ARRAY,
        dstHost: std::ptr::null_mut(),
        dstDevice: 0,
        dstArray: destination.level_zero,
        dstPitch: 0,
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
