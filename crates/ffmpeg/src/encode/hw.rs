use std::ptr;

use crate::ffi::{self, sys};
use crate::gpu::GpuBackend;
use crate::video::EncodeMode;
use crate::{FfmpegError, Result};
use sys::AVHWDeviceType::{AV_HWDEVICE_TYPE_CUDA, AV_HWDEVICE_TYPE_VULKAN};
use sys::AVPixelFormat::{AV_PIX_FMT_CUDA, AV_PIX_FMT_NV12, AV_PIX_FMT_RGBA, AV_PIX_FMT_VULKAN};

use super::video::VideoEncoderConfig;

pub(crate) struct EncoderHwContexts {
    pub backend: GpuBackend,
    pub device: AvBufferRef,
    pub frames: Option<AvBufferRef>,
    pub hw_format: sys::AVPixelFormat,
    /// Software format the encoder reads from its hardware frames.
    pub sw_format: sys::AVPixelFormat,
}

pub(crate) fn create_encoder_hw_contexts(
    config: &VideoEncoderConfig,
    codec: *const sys::AVCodec,
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
                backend: GpuBackend::Cuda,
                device,
                frames: Some(frames),
                hw_format: AV_PIX_FMT_CUDA,
                sw_format: AV_PIX_FMT_RGBA,
            }))
        }
        #[cfg(feature = "vulkan")]
        EncodeMode::GpuTexture(GpuBackend::Vulkan) => {
            let device = config
                .vulkan_device
                .as_ref()
                .ok_or_else(missing_vulkan_device)?
                .hw_device()?;
            let frames = AvBufferRef::new_hw_frames(
                &device,
                GpuBackend::Vulkan,
                AV_PIX_FMT_VULKAN,
                AV_PIX_FMT_NV12,
                config.width,
                config.height,
            )?;
            Ok(Some(EncoderHwContexts {
                backend: GpuBackend::Vulkan,
                device,
                frames: Some(frames),
                hw_format: AV_PIX_FMT_VULKAN,
                sw_format: AV_PIX_FMT_NV12,
            }))
        }
        // Vulkan Video encoders only accept Vulkan frames, so CPU frames are uploaded as NV12.
        EncodeMode::CpuUpload if takes_hw_frames(codec, AV_PIX_FMT_VULKAN) => {
            let device = AvBufferRef::new_hw_device(AV_HWDEVICE_TYPE_VULKAN, GpuBackend::Vulkan)?;
            let frames = AvBufferRef::new_hw_frames(
                &device,
                GpuBackend::Vulkan,
                AV_PIX_FMT_VULKAN,
                AV_PIX_FMT_NV12,
                config.width,
                config.height,
            )?;
            Ok(Some(EncoderHwContexts {
                backend: GpuBackend::Vulkan,
                device,
                frames: Some(frames),
                hw_format: AV_PIX_FMT_VULKAN,
                sw_format: AV_PIX_FMT_NV12,
            }))
        }
        _ => Ok(None),
    }
}

#[cfg(feature = "vulkan")]
pub(crate) fn missing_vulkan_device() -> FfmpegError {
    FfmpegError::new(
        "VideoEncoder::create",
        "Vulkan texture encode needs the shared device in `VideoEncoderConfig::vulkan_device`",
    )
    .with_backend(GpuBackend::Vulkan)
}

fn takes_hw_frames(codec: *const sys::AVCodec, pixel_format: sys::AVPixelFormat) -> bool {
    let mut index = 0;
    loop {
        let config = unsafe { sys::avcodec_get_hw_config(codec, index) };
        if config.is_null() {
            return false;
        }
        let uses_frames = unsafe {
            ((*config).methods & sys::AV_CODEC_HW_CONFIG_METHOD_HW_FRAMES_CTX as i32) != 0
        };
        if uses_frames && unsafe { (*config).pix_fmt } == pixel_format {
            return true;
        }
        index += 1;
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
