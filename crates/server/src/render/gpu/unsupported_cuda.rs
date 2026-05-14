use lumen_engine::composition::Composition;
use lumen_ffmpeg::VideoCodec;

use crate::render::{RenderError, RenderProgress, media::LocalMediaStore};

#[allow(clippy::too_many_arguments)]
pub(super) fn render_project_mp4_cuda(
    _composition: &Composition,
    _media_store: &LocalMediaStore,
    _width: u32,
    _height: u32,
    _fps: f32,
    _total_frames: u32,
    _codec: VideoCodec,
    _verbose_debug: bool,
    _on_progress: &mut dyn FnMut(RenderProgress),
) -> Result<Vec<u8>, RenderError> {
    Err(RenderError {
        code: "invalid_render_profile",
        message: "CUDA/Vulkan render path requires linux build with cuda and vulkan features"
            .to_string(),
        retryable: false,
    })
}
