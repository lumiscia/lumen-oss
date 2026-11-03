#![cfg(target_os = "linux")]

use std::{mem::MaybeUninit, sync::Arc};

use cudarc::driver::{CudaContext as CudaContextInner, result, sys};

use super::{CudaVideoFrame, interop};
pub use interop::{ImportedCudaExternalImage, import_owned_vulkan_opaque_fd_image};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CudaDeviceInfo {
    pub ordinal: i32,
    pub name: String,
    pub uuid: [u8; 16],
    pub pci_bus_id: String,
}

#[derive(Debug, Clone, Copy)]
pub struct CudaDriver;

impl CudaDriver {
    pub fn load() -> Result<Self, String> {
        result::init().map_err(|error| error.to_string())?;
        Ok(Self)
    }

    pub fn create_primary_context(&self) -> Result<CudaContext, String> {
        self.create_primary_context_for_ordinal(0)
    }

    pub fn create_primary_context_for_ordinal(&self, ordinal: i32) -> Result<CudaContext, String> {
        let inner =
            CudaContextInner::new(ordinal.max(0) as usize).map_err(|error| error.to_string())?;
        Ok(CudaContext { inner })
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
}

/// A CUDA primary context. Allocations and imports made through it keep it alive and bind it
/// before releasing their resources.
#[derive(Clone)]
pub struct CudaContext {
    inner: Arc<CudaContextInner>,
}

impl CudaContext {
    pub fn ordinal(&self) -> i32 {
        self.inner.ordinal() as i32
    }

    pub fn set_current(&self) -> Result<(), String> {
        self.inner
            .bind_to_thread()
            .map_err(|error| error.to_string())
    }

    pub fn allocate_rgba_frame(
        &self,
        width: u32,
        height: u32,
    ) -> Result<CudaDeviceAllocation, String> {
        self.set_current()?;
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
            memory: Arc::new(CudaDeviceMemory {
                context: self.clone(),
                device_ptr: unsafe { device_ptr.assume_init() },
                width,
                height,
                pitch: unsafe { pitch.assume_init() },
            }),
        })
    }

    pub fn copy_image_to_rgba_frame(
        &self,
        source: &ImportedCudaExternalImage,
        destination: &CudaDeviceAllocation,
    ) -> Result<(), String> {
        self.set_current()?;
        interop::copy_image_to_rgba_frame(
            source,
            destination.memory.device_ptr,
            destination.memory.pitch,
            destination.memory.width,
            destination.memory.height,
        )
    }
}

pub struct CudaDeviceAllocation {
    memory: Arc<CudaDeviceMemory>,
}

struct CudaDeviceMemory {
    context: CudaContext,
    device_ptr: sys::CUdeviceptr,
    width: u32,
    height: u32,
    pitch: usize,
}

impl CudaDeviceAllocation {
    pub fn as_video_frame(&self, pts: Option<i64>) -> CudaVideoFrame {
        CudaVideoFrame::from_retained_device_ptr(
            self.memory.device_ptr,
            self.memory.width,
            self.memory.height,
            self.memory.pitch as u64,
            pts,
            self.memory.clone(),
        )
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.memory.width, self.memory.height)
    }

    pub fn device_ptr(&self) -> sys::CUdeviceptr {
        self.memory.device_ptr
    }

    pub fn pitch(&self) -> usize {
        self.memory.pitch
    }

    pub fn clear(&self, value: u8) -> Result<(), String> {
        self.memory.context.set_current()?;
        unsafe {
            sys::cuMemsetD8_v2(
                self.memory.device_ptr,
                value,
                self.memory
                    .pitch
                    .saturating_mul(self.memory.height as usize),
            )
            .result()
            .map_err(|error| error.to_string())
        }
    }
}

impl Drop for CudaDeviceMemory {
    fn drop(&mut self) {
        let _ = self.context.set_current();
        let _ = unsafe { sys::cuMemFree_v2(self.device_ptr).result() };
    }
}
