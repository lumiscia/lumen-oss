use std::sync::Arc;

use skia_safe::{Color, Surface, surfaces};
use thiserror::Error;

use crate::{
    Timestamp,
    clip::{Clip, ClipError, Timeline},
    font::FontManager,
};

#[derive(Error, Debug)]
pub enum RendererError {
    #[error("Clip error: {0}")]
    ClipError(#[from] ClipError),
    #[error("Skia error: {0}")]
    SkiaError(String),
    #[error("The provided buffer's length ({0}) was not the required length ({1})")]
    MismatchedBufferLength(usize, usize),
}

pub struct RenderContext {
    pub width: usize,
    pub height: usize,
    pub duration: Timestamp,
    pub rate: u16,

    pub surface: Surface,
    pub font_manager: Box<dyn FontManager>,
}

pub struct Renderer {
    timeline: Arc<Timeline>,

    pub context: RenderContext,
}

impl Renderer {
    pub fn new(
        width: usize,
        height: usize,
        duration: Timestamp,
        rate: u16,
        timeline: Arc<Timeline>,
        font_manager: impl FontManager + 'static,
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
            timeline,

            context: RenderContext {
                width,
                height,
                duration,
                rate,
                surface: surface,
                font_manager: Box::new(font_manager),
            },
        })
    }

    pub fn draw_frame(&mut self, frame: usize) -> Result<(), RendererError> {
        let canvas = self.context.surface.canvas();
        canvas.clear(Color::BLACK);
        self.paint(frame)?;

        Ok(())
    }

    fn paint(&mut self, frame: usize) -> Result<(), RendererError> {
        self.timeline.draw(frame, &mut self.context)?;
        Ok(())
    }
}
