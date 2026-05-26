use crate::ffi::AvFrame;
use crate::gpu::GpuBackend;
use crate::video::PixelFormat;

pub const BACKEND: GpuBackend = GpuBackend::Cuda;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CudaExternalMemoryHandle {
    OpaqueFd(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CudaExternalSemaphoreHandle {
    OpaqueFd(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VulkanToCudaExport {
    pub memory: CudaExternalMemoryHandle,
    pub ready_semaphore: Option<CudaExternalSemaphoreHandle>,
    pub complete_semaphore: Option<CudaExternalSemaphoreHandle>,
    pub allocation_size: u64,
    pub row_pitch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CudaVideoFrame {
    device_ptr: u64,
    width: u32,
    height: u32,
    pitch: u64,
    pts: Option<i64>,
}

pub struct CudaDecodedFrame {
    _frame: AvFrame,
    device_ptr: u64,
    width: u32,
    height: u32,
    pitch: u64,
    pixel_format: PixelFormat,
    pts: Option<i64>,
}

// CUDA decoded frames retain their FFmpeg frame reference, and the exposed device pointer is only
// used by CUDA operations after a context is made current on the consuming thread.
unsafe impl Send for CudaDecodedFrame {}
unsafe impl Sync for CudaDecodedFrame {}

impl std::fmt::Debug for CudaDecodedFrame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CudaDecodedFrame")
            .field("device_ptr", &self.device_ptr)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("pitch", &self.pitch)
            .field("pixel_format", &self.pixel_format)
            .field("pts", &self.pts)
            .finish_non_exhaustive()
    }
}

impl CudaDecodedFrame {
    pub(crate) fn from_av_frame(
        frame: AvFrame,
        device_ptr: u64,
        width: u32,
        height: u32,
        pitch: u64,
        pixel_format: PixelFormat,
        pts: Option<i64>,
    ) -> Self {
        Self {
            _frame: frame,
            device_ptr,
            width,
            height,
            pitch,
            pixel_format,
            pts,
        }
    }

    pub fn device_ptr(&self) -> u64 {
        self.device_ptr
    }

    pub fn pitch(&self) -> u64 {
        self.pitch
    }

    pub fn pixel_format(&self) -> PixelFormat {
        self.pixel_format
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn pts(&self) -> Option<i64> {
        self.pts
    }

    pub fn backend(&self) -> GpuBackend {
        GpuBackend::Cuda
    }
}

impl CudaVideoFrame {
    pub fn from_device_ptr(
        device_ptr: u64,
        width: u32,
        height: u32,
        pitch: u64,
        pts: Option<i64>,
    ) -> Self {
        Self {
            device_ptr,
            width,
            height,
            pitch,
            pts,
        }
    }

    pub fn device_ptr(&self) -> u64 {
        self.device_ptr
    }

    pub fn pitch(&self) -> u64 {
        self.pitch
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn pts(&self) -> Option<i64> {
        self.pts
    }

    pub fn backend(&self) -> GpuBackend {
        GpuBackend::Cuda
    }
}
