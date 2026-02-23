use std::collections::HashMap;

use skia_safe::{Canvas, Color, FontMgr, Image, Surface, surfaces, textlayout::FontCollection};
use thiserror::Error;

use crate::time::Rational;

use crate::expr::ExpressionScope;
use crate::media::MediaStore;

#[derive(Clone)]
struct CachedImage {
    width: u32,
    height: u32,
    image: Image,
}

#[derive(Clone)]
struct CachedVideoFrame {
    frame: u32,
    width: u32,
    height: u32,
    image: Image,
}

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
    image_cache: HashMap<String, CachedImage>,
    video_frame_cache: HashMap<String, CachedVideoFrame>,
    font_collection: FontCollection,
    expression_scope: ExpressionScope,
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

        let mut font_collection = FontCollection::new();
        font_collection.set_default_font_manager(FontMgr::default(), None);

        Ok(Self {
            width,
            height,
            frame_rate,
            clear_color: Color::from_argb(0, 0, 0, 0),
            surface,
            overlay_surface,
            media_store: None,
            image_cache: HashMap::new(),
            video_frame_cache: HashMap::new(),
            font_collection,
            expression_scope: ExpressionScope::default(),
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

    pub(crate) fn font_collection(&self) -> FontCollection {
        self.font_collection.clone()
    }

    pub(crate) fn set_expression_scope(&mut self, scope: ExpressionScope) {
        self.expression_scope = scope;
    }

    pub(crate) fn expression_scope(&self) -> &ExpressionScope {
        &self.expression_scope
    }
    pub fn clear(&mut self) {
        self.surface.canvas().clear(self.clear_color);
        self.overlay_surface
            .canvas()
            .clear(Color::from_argb(0, 0, 0, 0));
    }

    #[cfg(test)]
    pub(crate) fn cached_image(&self, source: &str, width: u32, height: u32) -> Option<Image> {
        let cached = self.image_cache.get(source)?;
        (cached.width == width && cached.height == height).then(|| cached.image.clone())
    }

    pub(crate) fn cached_image_by_source(&self, source: &str) -> Option<(u32, u32, Image)> {
        let cached = self.image_cache.get(source)?;
        Some((cached.width, cached.height, cached.image.clone()))
    }

    pub(crate) fn cache_image(
        &mut self,
        source: impl Into<String>,
        width: u32,
        height: u32,
        image: Image,
    ) {
        self.image_cache.insert(
            source.into(),
            CachedImage {
                width,
                height,
                image,
            },
        );
    }

    #[cfg(test)]
    pub(crate) fn cached_video_frame(
        &self,
        source: &str,
        frame: u32,
        width: u32,
        height: u32,
    ) -> Option<Image> {
        let cached = self.video_frame_cache.get(source)?;
        (cached.frame == frame && cached.width == width && cached.height == height)
            .then(|| cached.image.clone())
    }

    pub(crate) fn cached_video_frame_by_source(
        &self,
        source: &str,
        frame: u32,
    ) -> Option<(u32, u32, Image)> {
        let cached = self.video_frame_cache.get(source)?;
        (cached.frame == frame).then(|| (cached.width, cached.height, cached.image.clone()))
    }

    pub(crate) fn cache_video_frame(
        &mut self,
        source: impl Into<String>,
        frame: u32,
        width: u32,
        height: u32,
        image: Image,
    ) {
        self.video_frame_cache.insert(
            source.into(),
            CachedVideoFrame {
                frame,
                width,
                height,
                image,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use skia_safe::{Data, ImageInfo, images};

    use super::RendererContext;
    use crate::time::Rational;

    fn tiny_image(rgba: [u8; 4]) -> skia_safe::Image {
        let info = ImageInfo::new(
            (1, 1),
            skia_safe::ColorType::RGBA8888,
            skia_safe::AlphaType::Unpremul,
            None,
        );
        let data = Data::new_copy(&rgba);
        images::raster_from_data(&info, data, 4).expect("raster image")
    }

    #[test]
    fn caches_static_images_by_source_and_dimensions() {
        let mut ctx = RendererContext::new(16, 16, Rational::new(30, 1)).expect("context");
        let image = tiny_image([1, 2, 3, 255]);

        assert!(ctx.cached_image("img", 1, 1).is_none());
        ctx.cache_image("img", 1, 1, image.clone());

        assert!(ctx.cached_image("img", 1, 1).is_some());
        assert!(ctx.cached_image("img", 2, 1).is_none());
        assert!(ctx.cached_image("other", 1, 1).is_none());
    }

    #[test]
    fn caches_last_video_frame_per_source() {
        let mut ctx = RendererContext::new(16, 16, Rational::new(30, 1)).expect("context");
        let frame0 = tiny_image([10, 20, 30, 255]);
        let frame1 = tiny_image([40, 50, 60, 255]);

        ctx.cache_video_frame("video", 0, 1, 1, frame0);
        assert!(ctx.cached_video_frame("video", 0, 1, 1).is_some());
        assert!(ctx.cached_video_frame("video", 1, 1, 1).is_none());

        ctx.cache_video_frame("video", 1, 1, 1, frame1);
        assert!(ctx.cached_video_frame("video", 0, 1, 1).is_none());
        assert!(ctx.cached_video_frame("video", 1, 1, 1).is_some());
    }
}
