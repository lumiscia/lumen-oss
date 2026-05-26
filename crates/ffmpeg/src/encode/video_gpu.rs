use std::ptr::{self, NonNull};
use std::time::Instant;

#[cfg(feature = "metal")]
use objc2_core_foundation::CFRetained;
#[cfg(feature = "metal")]
use objc2_core_video::CVPixelBuffer;

use crate::ffi::{self, AvFrame, sys};
use crate::gpu::{GpuBackend, GpuVideoInput};
use crate::video::EncodeMode;
use crate::{FfmpegError, Result};
use sys::AVPixelFormat::{AV_PIX_FMT_CUDA, AV_PIX_FMT_VIDEOTOOLBOX};

use super::output::OutputContext;
use super::telemetry::GpuUploadDescriptor;
use super::video::VideoEncoder;

impl VideoEncoder {
    pub(in crate::encode) fn send_gpu_frame(
        &mut self,
        output: &mut OutputContext,
        frame: &GpuVideoInput<'_>,
    ) -> Result<()> {
        if self.mode == EncodeMode::CpuUpload {
            return Err(FfmpegError::new(
                "VideoEncoder::send_gpu_frame",
                "CPU upload encoders consume CPU frames; create a hardware texture encoder to send GPU inputs",
            )
            .with_backend(frame.backend()));
        }
        let upload = GpuUploadDescriptor::from_frame(frame);
        self.gpu_telemetry.record_upload_started(&upload);
        let started = Instant::now();
        match frame {
            #[cfg(feature = "metal")]
            GpuVideoInput::MetalPixelBuffer(pixel_buffer) => {
                self.with_gpu_encode_telemetry(output, &upload, started, |encoder, output| {
                    encoder.send_metal_pixel_buffer_frame(output, pixel_buffer)
                })
            }
            #[cfg(feature = "cuda")]
            GpuVideoInput::Cuda(cuda_frame) => {
                self.with_gpu_encode_telemetry(output, &upload, started, |encoder, output| {
                    encoder.send_cuda_frame(output, cuda_frame)
                })
            }
            _ => {
                let error = FfmpegError::new(
                    "VideoEncoder::send_gpu_frame",
                    "GPU texture encode currently supports CVPixelBuffer-backed Metal frames and CUDA device pointers",
                )
                .with_backend(frame.backend());
                self.gpu_telemetry.record_upload_failed(
                    &upload,
                    started.elapsed(),
                    error.message.clone(),
                );
                Err(error)
            }
        }
    }

    fn with_gpu_encode_telemetry(
        &mut self,
        output: &mut OutputContext,
        upload: &GpuUploadDescriptor,
        upload_started: Instant,
        encode: impl FnOnce(&mut Self, &mut OutputContext) -> Result<()>,
    ) -> Result<()> {
        self.gpu_telemetry
            .record_upload_finished(upload, upload_started.elapsed());
        let encode_started = Instant::now();
        self.gpu_telemetry.record_encode_started(upload);
        match encode(self, output) {
            Ok(()) => {
                self.gpu_telemetry
                    .record_encode_finished(upload, encode_started.elapsed());
                Ok(())
            }
            Err(error) => {
                self.gpu_telemetry.record_encode_failed(
                    upload,
                    encode_started.elapsed(),
                    error.message.clone(),
                );
                Err(error)
            }
        }
    }

    #[cfg(feature = "cuda")]
    fn send_cuda_frame(
        &mut self,
        output: &mut OutputContext,
        frame: &crate::gpu::CudaVideoFrame,
    ) -> Result<()> {
        if self.mode != EncodeMode::GpuTexture(GpuBackend::Cuda) {
            return Err(FfmpegError::new(
                "VideoEncoder::send_cuda_frame",
                "CUDA frames require a CUDA/NVENC hardware texture encoder",
            )
            .with_backend(GpuBackend::Cuda));
        }
        let Some(hw_frames) = self.hw_frames.as_ref() else {
            return Err(FfmpegError::new(
                "VideoEncoder::send_cuda_frame",
                "CUDA/NVENC encoder was created without a CUDA hardware frames context",
            )
            .with_backend(GpuBackend::Cuda));
        };
        let (width, height) = frame.dimensions();
        unsafe {
            if (*self.context).width != width as i32 || (*self.context).height != height as i32 {
                return Err(FfmpegError::new(
                    "VideoEncoder::send_cuda_frame",
                    format!(
                        "CUDA frame dimensions {width}x{height} do not match encoder dimensions {}x{}",
                        (*self.context).width,
                        (*self.context).height,
                    ),
                )
                .with_backend(GpuBackend::Cuda));
            }
        }

        let mut av_frame = AvFrame::new()?;
        unsafe {
            (*av_frame.as_mut_ptr()).format = AV_PIX_FMT_CUDA as i32;
            (*av_frame.as_mut_ptr()).width = width as i32;
            (*av_frame.as_mut_ptr()).height = height as i32;
            (*av_frame.as_mut_ptr()).pts = frame.pts().unwrap_or(self.next_pts);
            (*av_frame.as_mut_ptr()).hw_frames_ctx = sys::av_buffer_ref(hw_frames.ptr);
            if (*av_frame.as_mut_ptr()).hw_frames_ctx.is_null() {
                return Err(FfmpegError::new(
                    "av_buffer_ref",
                    "failed to reference CUDA hardware frames context for frame",
                )
                .with_backend(GpuBackend::Cuda));
            }

            let data_ptr = frame.device_ptr() as usize as *mut u8;
            (*av_frame.as_mut_ptr()).data[0] = data_ptr;
            (*av_frame.as_mut_ptr()).linesize[0] = frame.pitch() as i32;
            (*av_frame.as_mut_ptr()).buf[0] = sys::av_buffer_create(
                data_ptr,
                1,
                Some(release_external_cuda_frame),
                ptr::null_mut(),
                0,
            );
            if (*av_frame.as_mut_ptr()).buf[0].is_null() {
                return Err(FfmpegError::new(
                    "av_buffer_create",
                    "failed to create external CUDA frame lifetime reference",
                )
                .with_backend(GpuBackend::Cuda));
            }
        }
        self.next_pts = self.next_pts.saturating_add(1);
        self.send_frame(output, av_frame.as_ptr())
    }

    #[cfg(feature = "metal")]
    fn send_metal_pixel_buffer_frame(
        &mut self,
        output: &mut OutputContext,
        frame: &crate::gpu::MetalPixelBufferFrame,
    ) -> Result<()> {
        if self.mode != EncodeMode::GpuTexture(GpuBackend::Metal) {
            return Err(FfmpegError::new(
                "VideoEncoder::send_metal_pixel_buffer_frame",
                "Metal pixel buffers require a Metal hardware texture encoder",
            )
            .with_backend(GpuBackend::Metal));
        }
        let mut av_frame = AvFrame::new()?;
        unsafe {
            (*av_frame.as_mut_ptr()).format = AV_PIX_FMT_VIDEOTOOLBOX as i32;
            (*av_frame.as_mut_ptr()).width = frame.dimensions().0 as i32;
            (*av_frame.as_mut_ptr()).height = frame.dimensions().1 as i32;
            (*av_frame.as_mut_ptr()).pts = frame.pts().unwrap_or(self.next_pts);

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
        self.next_pts = self.next_pts.saturating_add(1);
        self.send_frame(output, av_frame.as_ptr())
    }
}

#[cfg(feature = "cuda")]
unsafe extern "C" fn release_external_cuda_frame(_opaque: *mut libc::c_void, _data: *mut u8) {}

#[cfg(feature = "metal")]
unsafe extern "C" fn release_cv_pixel_buffer(opaque: *mut libc::c_void, _data: *mut u8) {
    if let Some(pixel_buffer) = NonNull::new(opaque.cast::<CVPixelBuffer>()) {
        let _ = unsafe { CFRetained::<CVPixelBuffer>::from_raw(pixel_buffer) };
    }
}
