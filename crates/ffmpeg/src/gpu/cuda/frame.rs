use crate::gpu::GpuBackend;
use std::sync::Arc;

pub const BACKEND: GpuBackend = GpuBackend::Cuda;

#[derive(Clone)]
pub struct CudaVideoFrame {
    device_ptr: u64,
    width: u32,
    height: u32,
    pitch: u64,
    pts: Option<i64>,
    // Keeps library-owned CUDA memory alive while FFmpeg retains a submitted frame.
    retention: Option<Arc<dyn Send + Sync>>,
}

impl std::fmt::Debug for CudaVideoFrame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CudaVideoFrame")
            .field("device_ptr", &self.device_ptr)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("pitch", &self.pitch)
            .field("pts", &self.pts)
            .field("retained", &self.retention.is_some())
            .finish()
    }
}

impl CudaVideoFrame {
    /// Creates a frame view over externally owned CUDA device memory.
    ///
    /// Prefer [`CudaDeviceAllocation::as_video_frame`](super::CudaDeviceAllocation::as_video_frame)
    /// for allocations created by this crate; those frames retain their allocation automatically.
    ///
    /// # Safety
    ///
    /// `device_ptr` must point to a CUDA allocation matching `width`, `height`, and `pitch`. The
    /// allocation must remain valid and must not be mutated incompatibly until every clone of this
    /// frame has been dropped and every encoder it was submitted to has been flushed or dropped.
    pub unsafe fn from_device_ptr(
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
            retention: None,
        }
    }

    pub(crate) fn from_retained_device_ptr(
        device_ptr: u64,
        width: u32,
        height: u32,
        pitch: u64,
        pts: Option<i64>,
        retention: Arc<dyn Send + Sync>,
    ) -> Self {
        Self {
            device_ptr,
            width,
            height,
            pitch,
            pts,
            retention: Some(retention),
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
