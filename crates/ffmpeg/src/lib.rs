//! Small, Lumen-owned FFmpeg abstraction.
//!
//! This crate intentionally depends only on `ffmpeg-sys-next` for FFmpeg
//! bindings and keeps raw FFmpeg pointers private to the implementation.

pub mod audio;
pub mod encode;
pub(crate) mod ffi;
pub mod format;
pub mod gpu;
pub mod video;

pub use audio::{AudioDecoder, AudioFrame, AudioResampler, AudioResamplerConfig, SampleFormat};
pub use encode::{
    AudioEncoder, AudioEncoderConfig, GpuEncodeEvent, GpuEncodeOutcome, GpuEncodeStage,
    GpuEncodeTelemetry, GpuTextureEncodeSupport, GpuUploadDescriptor, MuxedEncoder, OutputContext,
    VideoEncoder, VideoEncoderConfig, gpu_texture_encode_support,
};
pub use format::{AudioStreamInfo, InputContext, MediaInfo, Rational, VideoStreamInfo};
#[cfg(feature = "cuda")]
pub use gpu::CudaDecodedFrame;
#[cfg(feature = "vulkan")]
pub use gpu::VulkanVideoFrame;
#[cfg(all(feature = "cuda", target_os = "linux"))]
pub use gpu::{
    CudaContext, CudaDeviceAllocation, CudaDriver, CudaNv12ToRgbaConverter,
    ImportedCudaExternalImage, import_owned_vulkan_opaque_fd_image, import_vulkan_opaque_fd_image,
};
#[cfg(feature = "cuda")]
pub use gpu::{
    CudaExternalMemoryHandle, CudaExternalSemaphoreHandle, CudaVideoFrame, VulkanToCudaExport,
};
pub use gpu::{GpuBackend, GpuVideoFrame, GpuVideoInput};
#[cfg(feature = "metal")]
pub use gpu::{
    MetalDecodedFrame, MetalPixelBufferFrame, MetalPixelBufferPool, MetalTextureCache,
    Objc2MetalDevice, Objc2MetalTexture,
};
pub use video::{
    CpuVideoFrame, DecodeMode, EncodeMode, PixelFormat, VideoCodec, VideoDecoder,
    VideoDecoderConfig,
};

pub type Result<T> = std::result::Result<T, FfmpegError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfmpegError {
    pub operation: &'static str,
    pub path: Option<String>,
    pub code: Option<i32>,
    pub message: String,
    pub backend: Option<GpuBackend>,
    pub codec: Option<VideoCodec>,
    pub stream_index: Option<usize>,
}

impl FfmpegError {
    pub fn new(operation: &'static str, message: impl Into<String>) -> Self {
        Self {
            operation,
            path: None,
            code: None,
            message: message.into(),
            backend: None,
            codec: None,
            stream_index: None,
        }
    }

    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn with_backend(mut self, backend: GpuBackend) -> Self {
        self.backend = Some(backend);
        self
    }

    pub fn with_codec(mut self, codec: VideoCodec) -> Self {
        self.codec = Some(codec);
        self
    }

    pub fn with_stream_index(mut self, stream_index: usize) -> Self {
        self.stream_index = Some(stream_index);
        self
    }
}

impl std::fmt::Display for FfmpegError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} failed: {}", self.operation, self.message)?;
        if let Some(code) = self.code {
            write!(f, " ({code})")?;
        }
        if let Some(path) = &self.path {
            write!(f, " [{path}]")?;
        }
        Ok(())
    }
}

impl std::error::Error for FfmpegError {}
