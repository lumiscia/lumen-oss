pub mod alpha_premultiply;
pub mod blur;
pub mod channel_shuffle;
pub mod color_grade;
pub mod crop;
pub mod curves;
pub mod exposure;
pub mod filter_geometry;
pub(crate) mod gpu_shader;
pub mod hue_saturation;
pub mod levels;
pub mod matte_cleanup;
pub mod memo;
pub(crate) mod raster_map;
pub mod resize;
pub mod shadow;
pub mod skia_shader;
pub mod time_remap;
pub mod transform;

#[cfg(test)]
pub(crate) mod test_support {
    use crate::{
        composition::{Composition, RenderSettings, TimelineSettings},
        graph::Graph,
        media::{ImageResolver, MediaStore, VideoFrameResolver},
        raster::{AlphaMode, RasterFrame, RectI},
        render::{LumenRenderer, RenderContext, surface::DefaultSurfacePool},
    };

    #[derive(Debug)]
    pub(crate) struct NullMediaStore;

    impl MediaStore for NullMediaStore {
        fn get_image_resolver(&self, _source: &str) -> Option<Box<dyn ImageResolver>> {
            None
        }

        fn get_video_resolver(&self, _stream_id: &str) -> Option<Box<dyn VideoFrameResolver>> {
            None
        }
    }

    pub(crate) fn frame_from_rgba(
        pixels: &[u8],
        width: u32,
        height: u32,
        alpha_mode: AlphaMode,
    ) -> RasterFrame {
        RasterFrame::from_rgba_bytes(
            pixels,
            width,
            height,
            width as usize * 4,
            alpha_mode,
            RectI::from_size(width, height),
            RectI::from_size(width, height),
        )
        .expect("test frame")
    }

    pub(crate) fn frame_from_pixel(pixel: [u8; 4], alpha_mode: AlphaMode) -> RasterFrame {
        frame_from_rgba(&pixel, 1, 1, alpha_mode)
    }

    pub(crate) fn read_pixels(frame: &RasterFrame) -> Vec<u8> {
        let (width, height) = frame.storage_dimensions();
        let mut pixels = vec![0; width as usize * height as usize * 4];
        frame
            .read_pixels_into(&mut pixels, width as usize * 4)
            .expect("read pixels");
        pixels
    }

    pub(crate) fn read_first_pixel(frame: &RasterFrame) -> [u8; 4] {
        read_pixels(frame)[..4].try_into().expect("first pixel")
    }

    pub(crate) fn assert_pixel_near(actual: [u8; 4], expected: [u8; 4], tolerance: u8) {
        for (index, (actual, expected)) in actual.into_iter().zip(expected).enumerate() {
            let delta = actual.abs_diff(expected);
            assert!(
                delta <= tolerance,
                "channel {index}: actual {actual}, expected {expected}, tolerance {tolerance}"
            );
        }
    }

    pub(crate) fn with_test_context<T>(
        width: u32,
        height: u32,
        f: impl FnOnce(&mut RenderContext<'_, DefaultSurfacePool, NullMediaStore>) -> T,
    ) -> T {
        let composition = Composition::new(
            Graph::new(),
            TimelineSettings {
                fps: 30.0,
                duration_frames: 1,
            },
            RenderSettings {
                width,
                height,
                background_color: [0, 0, 0, 0],
            },
        );
        let pool = DefaultSurfacePool::new();
        let media = NullMediaStore;
        let renderer = LumenRenderer::new(&composition, &pool, &media).unwrap();
        let mut ctx = RenderContext::new(&renderer, 0);
        f(&mut ctx)
    }
}
