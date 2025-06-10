use skia_safe::{
    AlphaType, Color, ColorSpace, ColorType, FontMgr, ImageInfo, Surface, image::CachingHint,
    surfaces,
};
use thiserror::Error;

use crate::{Timestamp, source::SourceProvider};

#[derive(Error, Debug)]
pub enum RendererError {
    #[error("Skia error: {0}")]
    SkiaError(String),
    #[error("The provided buffer's length ({0}) was not the required length ({1})")]
    MismatchedBufferLength(usize, usize),
}

pub struct Renderer {
    pub width: usize,
    pub height: usize,
    pub duration: Timestamp,
    pub rate: u16,

    surface: Surface,
    buf_size: usize,
    font_mgr: FontMgr,
    dst_info: ImageInfo,
}

impl Renderer {
    pub fn new(
        width: usize,
        height: usize,
        duration: Timestamp,
        rate: u16,
    ) -> Result<Self, RendererError> {
        let surface = match surfaces::raster_n32_premul((width as i32, height as i32)) {
            Some(surface) => surface,
            None => {
                return Err(RendererError::SkiaError(
                    "Failed to create surface".to_string(),
                ));
            }
        };

        Ok(Self {
            width,
            height,
            duration,
            rate,

            surface: surface,
            buf_size: width * height * 4,
            font_mgr: FontMgr::new(),
            dst_info: ImageInfo::new(
                (width as i32, height as i32),
                ColorType::RGBA8888,
                AlphaType::Premul,
                None::<ColorSpace>,
            ),
        })
    }

    pub fn draw_frame(
        &mut self,
        frame: usize,
        source_provider: &mut impl SourceProvider,
        buf: &mut [u8],
    ) -> Result<(), RendererError> {
        if buf.len() != self.buf_size {
            return Err(RendererError::MismatchedBufferLength(
                buf.len(),
                self.buf_size,
            ));
        }

        let canvas = self.surface.canvas();
        canvas.clear(Color::BLACK);
        self.paint(frame, source_provider)?;

        let image = self.surface.image_snapshot();

        let success = image.read_pixels(
            &self.dst_info,
            buf,
            self.width * 4,
            (0, 0),
            CachingHint::Disallow,
        );

        if !success {
            return Err(RendererError::SkiaError(format!(
                "Failed to read pixels for frame {}",
                frame,
            )));
        }

        Ok(())
    }

    fn paint(
        &mut self,
        frame: usize,
        source_provider: &mut impl SourceProvider,
    ) -> Result<(), RendererError> {
        /*
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
        */
        Ok(())
    }
}
