use crate::gpu::GpuBackend;
use std::sync::Arc;

pub const BACKEND: GpuBackend = GpuBackend::Cuda;

/// An NV12 frame in CUDA device memory: `height` luma rows of `pitch` bytes, directly followed by
/// `height.div_ceil(2)` rows of interleaved CbCr pairs with the same pitch.
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
    /// `device_ptr` must point to a CUDA allocation holding an NV12 frame laid out as the type
    /// describes for `width`, `height`, and `pitch`. The allocation must remain valid and
    /// unmodified until the call that submits the frame to an encoder returns.
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

    /// Copies this frame's luma and chroma planes into the planes of `destination`, a CUDA NV12
    /// frame of the same dimensions given as a pointer and pitch per plane, in the context that
    /// owns the frame, and waits for the copies to finish.
    pub(crate) fn copy_to_device(&self, destination: [(u64, usize); 2]) -> Result<(), String> {
        use cudarc::driver::sys;
        use sys::CUmemorytype_enum::CU_MEMORYTYPE_DEVICE;

        let mut context: sys::CUcontext = std::ptr::null_mut();
        unsafe {
            sys::cuPointerGetAttribute(
                (&mut context as *mut sys::CUcontext).cast(),
                sys::CUpointer_attribute::CU_POINTER_ATTRIBUTE_CONTEXT,
                self.device_ptr,
            )
            .result()
            .map_err(|error| error.to_string())?;
            sys::cuCtxPushCurrent_v2(context)
                .result()
                .map_err(|error| error.to_string())?;
        }
        let pitch = self.pitch as usize;
        let planes = [
            (self.device_ptr, self.width as usize, self.height as usize),
            (
                self.device_ptr + self.pitch * u64::from(self.height),
                self.width.next_multiple_of(2) as usize,
                self.height.div_ceil(2) as usize,
            ),
        ];
        let copy_plane = |(source, width_bytes, rows): (u64, usize, usize),
                          (destination, destination_pitch): (u64, usize)| {
            let copy = sys::CUDA_MEMCPY2D {
                srcXInBytes: 0,
                srcY: 0,
                srcMemoryType: CU_MEMORYTYPE_DEVICE,
                srcHost: std::ptr::null(),
                srcDevice: source,
                srcArray: std::ptr::null_mut(),
                srcPitch: pitch,
                dstXInBytes: 0,
                dstY: 0,
                dstMemoryType: CU_MEMORYTYPE_DEVICE,
                dstHost: std::ptr::null_mut(),
                dstDevice: destination,
                dstArray: std::ptr::null_mut(),
                dstPitch: destination_pitch,
                WidthInBytes: width_bytes,
                Height: rows,
            };
            unsafe { sys::cuMemcpy2DAsync_v2(&copy, std::ptr::null_mut()) }.result()
        };
        // FFmpeg's NVENC reads the destination from its own context, whose stream isn't ordered
        // against this one, so the copies have to finish here.
        let copied = copy_plane(planes[0], destination[0])
            .and_then(|()| copy_plane(planes[1], destination[1]))
            .and_then(|()| unsafe { sys::cuStreamSynchronize(std::ptr::null_mut()) }.result());
        let mut popped: sys::CUcontext = std::ptr::null_mut();
        let popped_result = unsafe { sys::cuCtxPopCurrent_v2(&mut popped) }.result();
        copied.and(popped_result).map_err(|error| error.to_string())
    }
}
