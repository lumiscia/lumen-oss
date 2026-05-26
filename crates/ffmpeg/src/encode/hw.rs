use std::ptr;

use crate::ffi::{self, sys};
use crate::gpu::GpuBackend;
use crate::video::EncodeMode;
use crate::{FfmpegError, Result};
use sys::AVHWDeviceType::AV_HWDEVICE_TYPE_CUDA;
use sys::AVPixelFormat::{AV_PIX_FMT_CUDA, AV_PIX_FMT_RGBA};

use super::video::VideoEncoderConfig;

pub(crate) struct EncoderHwContexts {
    pub device: AvBufferRef,
    pub frames: Option<AvBufferRef>,
}

pub(crate) fn create_encoder_hw_contexts(
    config: &VideoEncoderConfig,
) -> Result<Option<EncoderHwContexts>> {
    match config.mode {
        EncodeMode::GpuTexture(GpuBackend::Cuda) => {
            let device = AvBufferRef::new_hw_device(AV_HWDEVICE_TYPE_CUDA, GpuBackend::Cuda)?;
            let frames = AvBufferRef::new_hw_frames(
                &device,
                GpuBackend::Cuda,
                AV_PIX_FMT_CUDA,
                AV_PIX_FMT_RGBA,
                config.width,
                config.height,
            )?;
            Ok(Some(EncoderHwContexts {
                device,
                frames: Some(frames),
            }))
        }
        _ => Ok(None),
    }
}

pub(crate) struct AvBufferRef {
    pub ptr: *mut sys::AVBufferRef,
}

impl AvBufferRef {
    fn new_hw_device(device_type: sys::AVHWDeviceType, backend: GpuBackend) -> Result<Self> {
        let mut ptr = ptr::null_mut();
        unsafe {
            ffi::check(
                sys::av_hwdevice_ctx_create(&mut ptr, device_type, ptr::null(), ptr::null_mut(), 0),
                "av_hwdevice_ctx_create",
            )
            .map_err(|error| error.with_backend(backend))?;
        }
        Ok(Self { ptr })
    }

    fn new_hw_frames(
        device: &Self,
        backend: GpuBackend,
        format: sys::AVPixelFormat,
        sw_format: sys::AVPixelFormat,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let ptr = unsafe { sys::av_hwframe_ctx_alloc(device.ptr) };
        if ptr.is_null() {
            return Err(FfmpegError::new(
                "av_hwframe_ctx_alloc",
                "failed to allocate encoder hardware frames context",
            )
            .with_backend(backend));
        }
        unsafe {
            let frames = (*ptr).data.cast::<sys::AVHWFramesContext>();
            (*frames).format = format;
            (*frames).sw_format = sw_format;
            (*frames).width = width as i32;
            (*frames).height = height as i32;
            (*frames).initial_pool_size = 8;
            if let Err(error) = ffi::check(sys::av_hwframe_ctx_init(ptr), "av_hwframe_ctx_init")
                .map_err(|error| error.with_backend(backend))
            {
                let mut ptr = ptr;
                sys::av_buffer_unref(&mut ptr);
                return Err(error);
            }
        }
        Ok(Self { ptr })
    }

    pub(crate) fn clone_ref(&self) -> Option<Self> {
        let ptr = unsafe { sys::av_buffer_ref(self.ptr) };
        (!ptr.is_null()).then_some(Self { ptr })
    }
}

impl Drop for AvBufferRef {
    fn drop(&mut self) {
        unsafe { sys::av_buffer_unref(&mut self.ptr) };
    }
}
