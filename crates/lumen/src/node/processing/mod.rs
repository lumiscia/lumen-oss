pub mod alpha_premultiply;
pub mod blur;
pub mod channel_shuffle;
pub mod color_grade;
pub(crate) mod color_table;
pub mod crop;
pub mod curves;
pub mod exposure;
pub mod filter_geometry;
pub(crate) mod gpu_shader;
pub mod hue_saturation;
pub mod levels;
pub mod matte_cleanup;
pub mod memo;
pub mod resize;
pub mod shadow;
pub mod skia_shader;
pub mod time_remap;
pub mod transform;

#[cfg(test)]
pub(crate) mod test_support {
    use crate::{
        composition::{Composition, RenderSettings, TimelineSettings},
        gpu_image::{AlphaMode, GpuImageFrame, RectI},
        graph::Graph,
        media::{ImageResolver, MediaStore, VideoFrameResolver},
        render::{
            LumenRenderer, RenderContext,
            surface::{SurfacePool, SurfacePoolStats},
        },
    };

    #[derive(Debug)]
    pub(crate) struct TestSurfacePool;

    impl SurfacePool for TestSurfacePool {
        fn with_surface<T>(
            &self,
            width: u32,
            height: u32,
            f: impl FnOnce(&mut skia_safe::Surface) -> crate::Result<T>,
        ) -> crate::Result<T> {
            let mut surface =
                skia_safe::surfaces::raster_n32_premul((width.max(1) as i32, height.max(1) as i32))
                    .ok_or(crate::error::RenderError::SurfaceAllocation { width, height })?;
            f(&mut surface)
        }

        fn stats(&self) -> SurfacePoolStats {
            SurfacePoolStats::default()
        }

        fn flush(&self) {}
    }

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
    ) -> GpuImageFrame {
        GpuImageFrame::from_cpu_decoded_rgba(
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

    pub(crate) fn frame_from_pixel(pixel: [u8; 4], alpha_mode: AlphaMode) -> GpuImageFrame {
        frame_from_rgba(&pixel, 1, 1, alpha_mode)
    }

    pub(crate) fn read_pixels(frame: &GpuImageFrame) -> Vec<u8> {
        let (width, height) = frame.storage_dimensions();
        let mut pixels = vec![0; width as usize * height as usize * 4];
        frame
            .read_pixels_into(&mut pixels, width as usize * 4)
            .expect("read pixels");
        pixels
    }

    pub(crate) fn read_first_pixel(frame: &GpuImageFrame) -> [u8; 4] {
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
        f: impl FnOnce(&mut RenderContext<'_, TestSurfacePool, NullMediaStore>) -> T,
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
        let pool = TestSurfacePool;
        let media = NullMediaStore;
        let renderer = LumenRenderer::new(&composition, &pool, &media).unwrap();
        let mut ctx = RenderContext::new(&renderer, 0);
        f(&mut ctx)
    }
}
