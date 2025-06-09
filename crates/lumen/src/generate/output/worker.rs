use ac_ffmpeg::{
    codec::video::{VideoFrame, VideoFrameMut, VideoFrameScaler, frame},
    time::{TimeBase, Timestamp},
};
use anyhow::anyhow;
use skia_safe::{
    AlphaType, Color, ColorSpace, ColorType, Font, FontMgr, ImageInfo, Point, Surface,
    image::CachingHint, surfaces,
};

pub struct RenderWorker {
    pub surface: Surface,
    pub width: usize,
    pub height: usize,
    pub frame_count: usize,
    pub source_frame: Option<VideoFrameMut>, // Now it's an Option
    pub font_mgr: FontMgr,
    pub scaler: VideoFrameScaler,
}

impl RenderWorker {
    pub fn new(
        width: usize,
        height: usize,
        frame_count: usize,
        time_base: TimeBase,
    ) -> anyhow::Result<Self> {
        let surface = match surfaces::raster_n32_premul((width as i32, height as i32)) {
            Some(surface) => surface,
            None => return Err(anyhow!("Failed to create surface")),
        };

        Ok(Self {
            surface: surface,
            width,
            height,
            frame_count,
            font_mgr: FontMgr::new(),
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

    pub fn draw_frame(&mut self, frame_index: usize) -> anyhow::Result<VideoFrame> {
        let dst_info = ImageInfo::new(
            (self.width as i32, self.height as i32),
            ColorType::RGBA8888,
            AlphaType::Premul,
            None::<ColorSpace>,
        );

        let canvas = self.surface.canvas();
        canvas.clear(Color::BLACK);
        self.paint(frame_index);

        let image = self.surface.image_snapshot();

        let success = image.read_pixels(
            &dst_info,
            self.source_frame.as_mut().unwrap().planes_mut()[0].data_mut(),
            self.width * 4, // row size
            (0, 0),
            CachingHint::Disallow,
        );

        if !success {
            return Err(anyhow!("Failed to read pixels for frame {}", frame_index));
        }
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

            scaled_frame.with_pts(Timestamp::new(frame_index as i64, time_base))
        };

        Ok(frame)
    }

    fn paint(&mut self, frame_index: usize) {
        let canvas = self.surface.canvas();
        let mut white_paint = skia_safe::Paint::default();
        white_paint.set_color(skia_safe::Color::WHITE);
        white_paint.set_anti_alias(true);
        let x = (self.width as f32 / self.frame_count as f32) * frame_index as f32;
        canvas.draw_circle((x, 600 as f32 / 2.0), 50.0, &white_paint);

        let mut font = Font::default();
        font.set_size(48.0); // Set font size

        let typeface = self
            .font_mgr
            .match_family_style(
                "Arial", // Try to find a serif font
                Default::default(),
            )
            .unwrap();
        font.set_typeface(typeface);

        // 4. The text to draw
        let text = "Hello, Skia-Safe!";
        let x = 50.0;
        let y = 100.0; // Y-coordinate is the baseline

        let (scalar, rect) = font.measure_str(text, Some(&white_paint));

        // 5. Draw the text
        let mut rect_paint = skia_safe::Paint::default();
        rect_paint.set_color(skia_safe::Color::RED);
        rect_paint.set_anti_alias(true);
        canvas.draw_round_rect(rect, 5.0, 5.0, &rect_paint);
        canvas.draw_str(text, Point::new(x, y), &font, &white_paint);
    }
}
