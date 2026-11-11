//! Small, Lumen-owned FFmpeg abstraction.
//!
//! This crate intentionally depends only on `ffmpeg-sys-next` for FFmpeg
//! bindings and keeps raw FFmpeg pointers private to the implementation.

pub mod audio;
mod encode;
pub(crate) mod ffi;
pub mod format;
pub mod gpu;
pub mod video;

#[cfg(feature = "vulkan")]
pub use ash;
pub use audio::{AudioDecoder, AudioFrame, AudioRangeReader, AudioResampler, AudioResamplerConfig};
pub use encode::{
    AudioEncoderConfig, BUILT_IN_PRESETS, ContainerCrop, EncodeCodec, EncoderCandidate,
    EncoderFamily, EncoderInfo, EncoderQuality, MuxedEncoder, QUALITY_CONFIG_ENV, QualityConfig,
    QualityRequest, RateControl, VideoEncoderChoice, VideoEncoderConfig, encoder_info,
    parse_bitrate,
};
pub use format::{
    ALLOWED_INPUT_FORMATS, ALLOWED_INPUT_PROTOCOLS, AudioStreamInfo, InputContext, MediaInfo,
    Rational, VideoStreamInfo,
};
#[cfg(feature = "cuda")]
pub use gpu::CudaVideoFrame;
#[cfg(all(feature = "cuda", target_os = "linux"))]
pub use gpu::{
    CudaContext, CudaDeviceAllocation, CudaDriver, ImportedCudaExternalBuffer,
    import_owned_vulkan_opaque_fd_buffer,
};
pub use gpu::{GpuBackend, GpuVideoFrame, GpuVideoInput};
#[cfg(feature = "metal")]
pub use gpu::{
    MetalDecodedFrame, MetalPixelBufferFrame, MetalPixelBufferPool, MetalTextureCache,
    Objc2MetalDevice, Objc2MetalTexture,
};
#[cfg(feature = "vulkan")]
pub use gpu::{
    VulkanDecodedFrame, VulkanDevice, VulkanDeviceDesc, VulkanFrameLayout, VulkanRenderQueue,
    VulkanVideoFrame,
};
pub use video::{
    ColorMatrix, ColorTags, CpuVideoFrame, DecodeMode, DecodedVideoFrame, EncodeMode, PixelFormat,
    VideoCodec, VideoDecoder, VideoDecoderConfig, YuvSubsampling, YuvVideoFrame,
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

    /// Whether opening or writing the output failed, rather than the codec.
    pub fn is_output_failure(&self) -> bool {
        matches!(
            self.operation,
            "avio_open"
                | "avformat_write_header"
                | "av_interleaved_write_frame"
                | "av_write_trailer"
                | "avio_closep"
        )
    }

    /// Whether the operation ran out of memory, file descriptors or a device that is in use, which
    /// can pass. Hardware encoders report a full session limit or exhausted device memory this way.
    pub fn is_resource_exhaustion(&self) -> bool {
        const ERRNOS: [i32; 5] = [
            libc::ENOMEM,
            libc::EMFILE,
            libc::ENFILE,
            libc::EAGAIN,
            libc::EBUSY,
        ];
        self.code
            .is_some_and(|code| ERRNOS.iter().any(|&errno| code == ffi::sys::AVERROR(errno)))
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
        if let Some(backend) = self.backend {
            write!(f, " backend={backend:?}")?;
        }
        if let Some(codec) = self.codec {
            write!(f, " codec={codec:?}")?;
        }
        if let Some(stream_index) = self.stream_index {
            write!(f, " stream={stream_index}")?;
        }
        Ok(())
    }
}

impl std::error::Error for FfmpegError {}

/// Raises this process's open file limit to its hard limit. NVIDIA's Vulkan driver holds a file
/// descriptor for each fence and timeline semaphore, so a job on Vulkan decode holds about 100 per
/// video source, and a few of them outgrow the usual soft limit of 1024.
pub fn raise_open_file_limit() {
    #[cfg(target_os = "linux")]
    unsafe {
        let mut limit = std::mem::zeroed::<libc::rlimit>();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) == 0 && limit.rlim_cur < limit.rlim_max
        {
            limit.rlim_cur = limit.rlim_max;
            libc::setrlimit(libc::RLIMIT_NOFILE, &limit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(operation: &'static str, errno: i32) -> FfmpegError {
        ffi::error_from_code(operation, ffi::sys::AVERROR(errno))
    }

    #[test]
    fn resource_exhaustion_is_a_shortage_that_can_pass() {
        for errno in [
            libc::ENOMEM,
            libc::EMFILE,
            libc::ENFILE,
            libc::EAGAIN,
            libc::EBUSY,
        ] {
            assert!(failed("avcodec_open2", errno).is_resource_exhaustion());
        }
        assert!(!failed("avcodec_open2", libc::EINVAL).is_resource_exhaustion());
        assert!(!failed("avio_open", libc::ENOENT).is_resource_exhaustion());
        assert!(!FfmpegError::new("avcodec_open2", "no code").is_resource_exhaustion());
    }

    #[test]
    fn output_failures_are_the_operations_on_the_output() {
        for operation in ["avio_open", "avformat_write_header", "av_write_trailer"] {
            assert!(failed(operation, libc::ENOENT).is_output_failure());
        }
        assert!(!failed("avcodec_open2", libc::ENOENT).is_output_failure());
    }

    #[test]
    fn display_includes_every_context_field_that_is_set() {
        let mut error = FfmpegError::new("avcodec_send_packet", "Invalid data")
            .with_path("clip.mp4")
            .with_backend(GpuBackend::Cuda)
            .with_codec(VideoCodec::H264)
            .with_stream_index(1);
        error.code = Some(-22);
        assert_eq!(
            error.to_string(),
            "avcodec_send_packet failed: Invalid data (-22) [clip.mp4] backend=Cuda codec=H264 stream=1"
        );
        assert_eq!(
            FfmpegError::new("avcodec_send_packet", "Invalid data").to_string(),
            "avcodec_send_packet failed: Invalid data"
        );
    }
}
