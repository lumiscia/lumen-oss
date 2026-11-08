#[cfg(feature = "metal")]
use std::ptr::NonNull;

#[cfg(feature = "metal")]
use objc2_core_foundation::CFRetained;
#[cfg(feature = "metal")]
use objc2_core_video::CVPixelBuffer;

#[cfg(any(feature = "cuda", feature = "vulkan"))]
use crate::ffi;
use crate::ffi::AvFrame;
#[cfg(any(feature = "metal", feature = "cuda", feature = "vulkan"))]
use crate::ffi::sys;
#[cfg(any(feature = "metal", feature = "cuda", feature = "vulkan"))]
use crate::gpu::GpuBackend;
use crate::gpu::GpuVideoInput;
use crate::{FfmpegError, Result};
#[cfg(feature = "metal")]
use sys::AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX;

#[cfg(any(feature = "cuda", feature = "vulkan"))]
use super::hw::AvBufferRef;
use super::output::OutputContext;
use super::video::{Transport, VideoEncoder};

impl VideoEncoder {
    pub(in crate::encode) fn send_gpu_frame(
        &mut self,
        output: &mut OutputContext,
        frame: &GpuVideoInput<'_>,
    ) -> Result<()> {
        let (width, height) = frame.dimensions();
        if (width, height) != (self.width, self.height) {
            return Err(FfmpegError::new(
                "VideoEncoder::send_gpu_frame",
                format!(
                    "{:?} frame dimensions {width}x{height} do not match encoder dimensions {}x{}",
                    frame.backend(),
                    self.width,
                    self.height,
                ),
            )
            .with_backend(frame.backend()));
        }
        let mut prepared = prepare_gpu_frame(&self.transport, frame)?;
        self.send_prepared(output, prepared.as_mut_ptr(), frame.pts())
    }
}

/// Puts `frame` into an `AvFrame` the encoder can take, checking its device before any upload.
fn prepare_gpu_frame(transport: &Transport, frame: &GpuVideoInput<'_>) -> Result<AvFrame> {
    match (transport, frame) {
        #[cfg(feature = "cuda")]
        (Transport::Cuda { frames }, GpuVideoInput::Cuda(frame)) => cuda_frame(frames, frame),
        #[cfg(feature = "vulkan")]
        (Transport::Vulkan { frames, device }, GpuVideoInput::Vulkan(frame)) => {
            vulkan_frame(frames, device, frame)
        }
        #[cfg(feature = "metal")]
        (Transport::Metal, GpuVideoInput::MetalPixelBuffer(frame)) => metal_frame(frame),
        (Transport::Cpu { .. }, _) => Err(FfmpegError::new(
            "VideoEncoder::send_gpu_frame",
            "CPU upload encoders consume CPU frames; create a hardware texture encoder to send GPU inputs",
        )
        .with_backend(frame.backend())),
        #[allow(unreachable_patterns)]
        _ => Err(FfmpegError::new(
            "VideoEncoder::send_gpu_frame",
            "this encoder takes CVPixelBuffer-backed Metal frames, CUDA or Vulkan NV12 frames from its own backend",
        )
        .with_backend(frame.backend())),
    }
}

/// NVENC reads a submitted frame asynchronously, so it gets its own pool frame; the pool takes the
/// frame back once the encoder drops it.
#[cfg(feature = "cuda")]
fn cuda_frame(frames: &AvBufferRef, frame: &crate::gpu::CudaVideoFrame) -> Result<AvFrame> {
    let mut av_frame = AvFrame::new()?;
    unsafe {
        ffi::check(
            sys::av_hwframe_get_buffer(frames.ptr, av_frame.as_mut_ptr(), 0),
            "av_hwframe_get_buffer",
        )
        .map_err(|error| error.with_backend(GpuBackend::Cuda))?;
        let planes = (*av_frame.as_ptr()).data;
        let pitches = (*av_frame.as_ptr()).linesize;
        frame
            .copy_to_device([
                (planes[0] as u64, pitches[0] as usize),
                (planes[1] as u64, pitches[1] as usize),
            ])
            .map_err(|details| {
                FfmpegError::new("VideoEncoder::send_gpu_frame", details)
                    .with_backend(GpuBackend::Cuda)
            })?;
    }
    Ok(av_frame)
}

#[cfg(feature = "vulkan")]
fn vulkan_frame(
    frames: &AvBufferRef,
    device: &crate::gpu::VulkanDevice,
    frame: &crate::gpu::VulkanVideoFrame,
) -> Result<AvFrame> {
    if frame.device() != device.handle() {
        return Err(FfmpegError::new(
            "VideoEncoder::send_gpu_frame",
            format!(
                "Vulkan frame is on device {:?}, but the encoder uses device {:?}",
                frame.device(),
                device.handle(),
            ),
        )
        .with_backend(GpuBackend::Vulkan));
    }

    let mut av_frame = AvFrame::new()?;
    unsafe {
        ffi::check(
            sys::av_hwframe_get_buffer(frames.ptr, av_frame.as_mut_ptr(), 0),
            "av_hwframe_get_buffer",
        )
        .map_err(|error| error.with_backend(GpuBackend::Vulkan))?;
        let target = (*av_frame.as_ptr()).data[0].cast::<sys::AVVkFrame>();
        let frames_context = (*frames.ptr).data.cast::<sys::AVHWFramesContext>();
        let vulkan_frames = (*frames_context).hwctx.cast::<sys::AVVulkanFramesContext>();
        if let Some(lock) = (*vulkan_frames).lock_frame {
            lock(frames_context, target);
        }
        let uploaded = device.upload(frame, target);
        if let Some(unlock) = (*vulkan_frames).unlock_frame {
            unlock(frames_context, target);
        }
        uploaded?;
    }
    Ok(av_frame)
}

/// The `AvFrame` retains the pixel buffer until FFmpeg releases it.
#[cfg(feature = "metal")]
fn metal_frame(frame: &crate::gpu::MetalPixelBufferFrame) -> Result<AvFrame> {
    let mut av_frame = AvFrame::new()?;
    unsafe {
        (*av_frame.as_mut_ptr()).format = AV_PIX_FMT_VIDEOTOOLBOX as i32;
        (*av_frame.as_mut_ptr()).width = frame.dimensions().0 as i32;
        (*av_frame.as_mut_ptr()).height = frame.dimensions().1 as i32;

        let pixel_buffer = frame.pixel_buffer_ptr();
        let retained: CFRetained<CVPixelBuffer> = CFRetained::retain(pixel_buffer);
        let retained_ptr: NonNull<CVPixelBuffer> = CFRetained::into_raw(retained);
        let buffer_ref = sys::av_buffer_create(
            retained_ptr.as_ptr().cast::<u8>(),
            1,
            Some(release_cv_pixel_buffer),
            retained_ptr.as_ptr().cast(),
            0,
        );
        if buffer_ref.is_null() {
            let _ = CFRetained::<CVPixelBuffer>::from_raw(retained_ptr);
            return Err(FfmpegError::new(
                "av_buffer_create",
                "failed to create CVPixelBuffer lifetime reference",
            )
            .with_backend(GpuBackend::Metal));
        }
        (*av_frame.as_mut_ptr()).data[3] = pixel_buffer.as_ptr().cast::<u8>();
        (*av_frame.as_mut_ptr()).buf[0] = buffer_ref;
    }
    Ok(av_frame)
}

#[cfg(feature = "metal")]
unsafe extern "C" fn release_cv_pixel_buffer(opaque: *mut libc::c_void, _data: *mut u8) {
    if let Some(pixel_buffer) = NonNull::new(opaque.cast::<CVPixelBuffer>()) {
        let _ = unsafe { CFRetained::<CVPixelBuffer>::from_raw(pixel_buffer) };
    }
}
