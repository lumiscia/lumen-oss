use std::sync::Arc;

use ac_ffmpeg::{
    codec::video::{VideoFrame, VideoFrameMut, VideoFrameScaler, frame},
    time::{TimeBase, Timestamp},
};
use anyhow::anyhow;
use lumen::{
    media::MediaProvider,
    plan::RenderPlan,
    render::Renderer,
    time::FrameIndex,
};

use crate::video::ServerFontManager;

pub struct FFmpegRenderer {
    inner: Renderer,
    source_frame: Option<VideoFrameMut>,
    scaler: VideoFrameScaler,
}

impl FFmpegRenderer {
    pub fn new(
        plan: Arc<RenderPlan>,
        media_provider: impl MediaProvider + 'static,
        time_base: TimeBase,
    ) -> anyhow::Result<Self> {
        let width = plan.canvas.width as usize;
        let height = plan.canvas.height as usize;

        Ok(Self {
            inner: Renderer::new(plan, ServerFontManager::new(), media_provider)?,
            source_frame: Some(
                VideoFrameMut::black(frame::get_pixel_format("rgba"), width, height)
                    .with_time_base(time_base),
            ),
            scaler: VideoFrameScaler::builder()
                .source_width(width)
                .source_height(height)
                .source_pixel_format(frame::get_pixel_format("rgba"))
                .target_width(width)
                .target_height(height)
                .target_pixel_format(frame::get_pixel_format("yuv420p"))
                .build()?,
        })
    }

    pub fn draw_frame(&mut self, frame: usize) -> anyhow::Result<VideoFrame> {
        self.inner.draw_frame(FrameIndex(frame as u64))?;

        let source_frame = self
            .source_frame
            .as_mut()
            .ok_or_else(|| anyhow!("source_frame was unexpectedly missing"))?;
        self.inner.read_rgba(source_frame.planes_mut()[0].data_mut())?;

        let frame = {
            let temp_mutable_frame = self.source_frame.take().ok_or_else(|| {
                anyhow!("source_frame was unexpectedly missing during freeze operation")
            })?;

            let frozen = temp_mutable_frame.freeze();

            let scaled_frame = self.scaler.scale(&frozen)?;

            self.source_frame = match frozen.try_into_mut() {
                Ok(f) => Some(f),
                Err(_) => {
                    return Err(anyhow!("failed to convert frame back into mutable frame"));
                }
            };

            let time_base = scaled_frame.time_base();

            scaled_frame.with_pts(Timestamp::new(frame as i64, time_base))
        };

        Ok(frame)
    }
}
