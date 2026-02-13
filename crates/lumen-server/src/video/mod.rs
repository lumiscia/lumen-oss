mod common;

#[cfg(feature = "decode-libav")]
mod libav;
#[cfg(not(feature = "decode-libav"))]
mod subprocess;

#[cfg(feature = "decode-libav")]
pub use libav::{FfmpegRenderBackend, RenderBackendOptions};
#[cfg(not(feature = "decode-libav"))]
pub use subprocess::{FfmpegRenderBackend, RenderBackendOptions};
