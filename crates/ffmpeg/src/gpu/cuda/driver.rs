#![cfg(target_os = "linux")]

use std::{ffi::CStr, mem::MaybeUninit, sync::Arc};

use cudarc::driver::{CudaContext as CudaContextInner, result, sys};

use super::{CudaExternalMemoryHandle, CudaVideoFrame, interop, kernel};
use interop::ImportedCudaExternalImage;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CudaDeviceInfo {
    pub ordinal: i32,
    pub name: String,
    pub uuid: [u8; 16],
    pub pci_bus_id: String,
}

pub struct CudaDriver;

unsafe impl Send for CudaDriver {}
unsafe impl Sync for CudaDriver {}

impl CudaDriver {
    pub fn load() -> Result<Self, String> {
        result::init().map_err(|error| error.to_string())?;
        Ok(Self)
    }

    pub fn create_primary_context(&self) -> Result<CudaContext<'_>, String> {
        self.create_primary_context_for_ordinal(0)
    }

    pub fn create_primary_context_for_ordinal(
        &self,
        ordinal: i32,
    ) -> Result<CudaContext<'_>, String> {
        let inner =
            CudaContextInner::new(ordinal.max(0) as usize).map_err(|error| error.to_string())?;
        Ok(CudaContext {
            driver: self,
            inner,
        })
    }

    pub fn driver_version(&self) -> Result<i32, String> {
        result::init().map_err(|error| error.to_string())?;
        let mut version = MaybeUninit::<i32>::uninit();
        unsafe { sys::cuDriverGetVersion(version.as_mut_ptr()) }
            .result()
            .map_err(|error| error.to_string())?;
        Ok(unsafe { version.assume_init() })
    }

    pub fn devices(&self) -> Result<Vec<CudaDeviceInfo>, String> {
        result::init().map_err(|error| error.to_string())?;
        let count = result::device::get_count().map_err(|error| error.to_string())?;
        let mut devices = Vec::new();
        for ordinal in 0..count {
            let device = result::device::get(ordinal).map_err(|error| error.to_string())?;
            let name = result::device::get_name(device).map_err(|error| error.to_string())?;
            let uuid = result::device::get_uuid(device).map_err(|error| error.to_string())?;
            let pci_bus_id = interop::device_pci_bus_id(device)?;
            devices.push(CudaDeviceInfo {
                ordinal,
                name,
                uuid: uuid.bytes.map(|byte| byte as u8),
                pci_bus_id,
            });
        }
        Ok(devices)
    }

    pub fn describe_error(&self, result: sys::CUresult) -> String {
        let mut name = std::ptr::null();
        let mut description = std::ptr::null();
        let name = if unsafe { sys::cuGetErrorName(result, &mut name) }
            .result()
            .is_ok()
            && !name.is_null()
        {
            unsafe { CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned()
        } else {
            "UNKNOWN_CUDA_ERROR".to_string()
        };
        let description = if unsafe { sys::cuGetErrorString(result, &mut description) }
            .result()
            .is_ok()
            && !description.is_null()
        {
            unsafe { CStr::from_ptr(description) }
                .to_string_lossy()
                .into_owned()
        } else {
            "no CUDA error description available".to_string()
        };
        format!("{name} ({result:?}): {description}")
    }

    pub fn allocate_rgba_frame(
        &self,
        width: u32,
        height: u32,
    ) -> Result<CudaDeviceAllocation<'_>, String> {
        let width_bytes = width as usize * 4;
        let mut device_ptr = MaybeUninit::<sys::CUdeviceptr>::uninit();
        let mut pitch = MaybeUninit::<usize>::uninit();
        unsafe {
            sys::cuMemAllocPitch_v2(
                device_ptr.as_mut_ptr(),
                pitch.as_mut_ptr(),
                width_bytes,
                height as usize,
                16,
            )
            .result()
            .map_err(|error| error.to_string())?;
        }
        Ok(CudaDeviceAllocation {
            driver: self,
            device_ptr: unsafe { device_ptr.assume_init() },
            width,
            height,
            pitch: unsafe { pitch.assume_init() },
        })
    }

    pub fn copy_image_to_rgba_frame(
        &self,
        source: &ImportedCudaExternalImage,
        destination: &CudaDeviceAllocation<'_>,
    ) -> Result<(), String> {
        interop::copy_image_to_rgba_frame(
            source,
            destination.device_ptr,
            destination.pitch,
            destination.width,
            destination.height,
        )
    }

    pub fn copy_rgba_frame_to_image(
        &self,
        source: &CudaDeviceAllocation<'_>,
        destination: &ImportedCudaExternalImage,
    ) -> Result<(), String> {
        interop::copy_rgba_frame_to_image(
            source.device_ptr,
            source.pitch,
            destination,
            source.width,
            source.height,
        )
    }

    pub fn synchronize_context(&self) -> Result<(), String> {
        unsafe { sys::cuCtxSynchronize() }
            .result()
            .map_err(|error| error.to_string())
    }

    pub fn create_nv12_to_rgba_converter(
        &self,
        context: &CudaContext<'_>,
    ) -> Result<CudaNv12ToRgbaConverter<'_>, String> {
        let converter = kernel::CudaNv12ToRgbaConverter::new(context.inner.clone())?;
        Ok(CudaNv12ToRgbaConverter {
            _driver: self,
            context: context.inner.clone(),
            converter,
        })
    }
}

pub struct CudaContext<'a> {
    driver: &'a CudaDriver,
    inner: Arc<CudaContextInner>,
}

impl CudaContext<'_> {
    pub fn ordinal(&self) -> i32 {
        self.inner.ordinal() as i32
    }

    pub fn set_current(&self) -> Result<(), String> {
        self.inner
            .bind_to_thread()
            .map_err(|error| error.to_string())
    }
}

pub struct CudaDeviceAllocation<'a> {
    driver: &'a CudaDriver,
    device_ptr: sys::CUdeviceptr,
    width: u32,
    height: u32,
    pitch: usize,
}

impl CudaDeviceAllocation<'_> {
    pub fn as_video_frame(&self, pts: Option<i64>) -> CudaVideoFrame {
        CudaVideoFrame::from_device_ptr(
            self.device_ptr,
            self.width,
            self.height,
            self.pitch as u64,
            pts,
        )
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn device_ptr(&self) -> sys::CUdeviceptr {
        self.device_ptr
    }

    pub fn pitch(&self) -> usize {
        self.pitch
    }

    pub fn clear(&self, value: u8) -> Result<(), String> {
        unsafe {
            sys::cuMemsetD8_v2(
                self.device_ptr,
                value,
                self.pitch.saturating_mul(self.height as usize),
            )
            .result()
            .map_err(|error| error.to_string())
        }
    }
}

impl Drop for CudaDeviceAllocation<'_> {
    fn drop(&mut self) {
        let _ = unsafe { sys::cuMemFree_v2(self.device_ptr).result() };
    }
}

pub struct CudaNv12ToRgbaConverter<'a> {
    _driver: &'a CudaDriver,
    context: Arc<CudaContextInner>,
    converter: kernel::CudaNv12ToRgbaConverter,
}

impl CudaNv12ToRgbaConverter<'_> {
    pub fn convert(
        &self,
        source: &super::CudaDecodedFrame,
        destination: &CudaDeviceAllocation<'_>,
    ) -> Result<(), String> {
        let _ = self.context.bind_to_thread();
        self.converter.convert(source, destination)
    }
}

pub fn import_vulkan_opaque_fd_image<'a>(
    _driver: &'a CudaDriver,
    memory: CudaExternalMemoryHandle,
    allocation_size: u64,
    width: u32,
    height: u32,
) -> Result<ImportedCudaExternalImage, String> {
    interop::import_vulkan_opaque_fd_image(memory, allocation_size, width, height)
}

pub fn import_owned_vulkan_opaque_fd_image<'a>(
    _driver: &'a CudaDriver,
    fd: std::os::fd::OwnedFd,
    allocation_size: u64,
    width: u32,
    height: u32,
) -> Result<ImportedCudaExternalImage, String> {
    interop::import_owned_vulkan_opaque_fd_image(fd, allocation_size, width, height)
}
