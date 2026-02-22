use skia_safe::{Canvas, Color, Surface, surfaces};
use thiserror::Error;

use crate::time::Rational;

use crate::media::MediaStore;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameContext {
    pub frame: u64,
    pub time_seconds: f64,
    pub width: u32,
    pub height: u32,
    pub device_scale: f32,
}

pub struct RendererContext {
    pub width: u32,
    pub height: u32,
    pub frame_rate: Rational,
    pub clear_color: Color,
    pub surface: Surface,
    pub overlay_surface: Surface,
    pub media_store: Option<Box<dyn MediaStore>>,
}

#[derive(Debug, Error)]
pub enum RendererContextError {
    #[error("failed to create renderer surface")]
    SurfaceCreation,
}

impl RendererContext {
    pub fn new(
        width: u32,
        height: u32,
        frame_rate: Rational,
    ) -> Result<Self, RendererContextError> {
        let surface = surfaces::raster_n32_premul((width as i32, height as i32))
            .ok_or(RendererContextError::SurfaceCreation)?;
        let overlay_surface = surfaces::raster_n32_premul((width as i32, height as i32))
            .ok_or(RendererContextError::SurfaceCreation)?;

        Ok(Self {
            width,
            height,
            frame_rate,
            clear_color: Color::from_argb(0, 0, 0, 0),
            surface,
            overlay_surface,
            media_store: None,
        })
    }

    pub fn canvas(&mut self) -> &Canvas {
        self.surface.canvas()
    }

    pub fn overlay_canvas(&mut self) -> &Canvas {
        self.overlay_surface.canvas()
    }

    pub fn set_media_store(&mut self, media_store: Box<dyn MediaStore>) {
        self.media_store = Some(media_store);
    }

    pub fn media_store_mut(&mut self) -> Option<&mut (dyn MediaStore + 'static)> {
        self.media_store.as_deref_mut()
    }

    pub fn clear(&mut self) {
        self.surface.canvas().clear(self.clear_color);
        self.overlay_surface
            .canvas()
            .clear(Color::from_argb(0, 0, 0, 0));
    }
}
