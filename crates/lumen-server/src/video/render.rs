use ac_ffmpeg::{
    codec::video::{VideoFrame, VideoFrameMut, VideoFrameScaler, frame},
    time::{TimeBase, Timestamp},
};
use anyhow::anyhow;
use lumen::{render::Renderer, source::SourceProvider};

pub struct FFmpegRenderer {
    inner: Renderer,
    pub source_frame: Option<VideoFrameMut>,
    pub scaler: VideoFrameScaler,
}

impl FFmpegRenderer {
    pub fn new(
        width: usize,
        height: usize,
        duration: lumen::Timestamp,
        rate: u16,
        time_base: TimeBase,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            inner: Renderer::new(width, height, duration, rate)?,
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

    pub fn draw_frame(
        &mut self,
        frame: usize,
        source_provider: &mut impl SourceProvider,
    ) -> anyhow::Result<VideoFrame> {
        self.inner.draw_frame(
            frame,
            source_provider,
            self.source_frame.as_mut().unwrap().planes_mut()[0].data_mut(),
        )?;

        let frame = {
            let temp_mutable_frame = self.source_frame.take().ok_or_else(|| {
                anyhow!("source_frame was unexpectedly missing during freeze operation")
            })?;

            let frozen = temp_mutable_frame.freeze();

            let scaled_frame = self.scaler.scale(&frozen)?;

            self.source_frame = match frozen.try_into_mut() {
                Ok(f) => Some(f),
                Err(_) => {
                    panic!("Failed to convert frame back into mut");
                }
            };

            let time_base = scaled_frame.time_base();

            scaled_frame.with_pts(Timestamp::new(frame as i64, time_base))
        };

        Ok(frame)
    }
}
