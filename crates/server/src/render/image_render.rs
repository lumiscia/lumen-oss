use std::{ops::Range, time::Instant};

use image::{
    ExtendedColorType, ImageEncoder,
    codecs::{jpeg::JpegEncoder, png::PngEncoder},
};
use lumen_engine::{
    composition::Composition,
    gpu::{GpuCompositionRenderer, OutputAlpha},
};
use lumen_ffmpeg::EncoderFamily;
use lumen_gpu::{Size, wgpu::TextureFormat};

use super::{
    CancelToken, ErrorCode, OutputFormat, RenderOutput, RenderProgress, RenderReport, RenderTiming,
    ServerRenderError,
    device_budget::{DeviceReservation, reserve_devices},
    encode_target::{checked_size, gpu_renderer_failed, readback_failed, render_raster, timed},
    gpu::{create_gpu_renderer, prefetch_media_frames},
    media::LocalMediaStore,
};

/// One finished frame of an image render.
#[derive(Debug)]
pub struct EncodedImage {
    /// The composition's frame number.
    pub frame: u32,
    /// The PNG or JPEG file.
    pub bytes: Vec<u8>,
}

/// A render of a still or an image sequence with its composition compiled, ready to hand frames to
/// a caller one at a time. A frame is read back, encoded and handed over before the next one is
/// rendered, so memory holds one frame whatever the length of the range.
pub struct ImageRender<'a> {
    composition: &'a Composition,
    media_store: LocalMediaStore,
    renderer: GpuCompositionRenderer,
    format: OutputFormat,
    jpeg_quality: u8,
    frames: Range<u32>,
    cancel: CancelToken,
    // Last, so everything holding a device closes before its room is given back.
    _devices: DeviceReservation<'static>,
}

impl<'a> ImageRender<'a> {
    /// Checks `output`, which must name an image format and a range that fits the timeline, and
    /// then opens the renderer. Cancelling `cancel` ends the wait for GPU device room, and stops
    /// [`run`](Self::run) with [`ErrorCode::Cancelled`].
    pub fn open(
        composition: &'a Composition,
        mut media_store: LocalMediaStore,
        output: &RenderOutput,
        cancel: CancelToken,
    ) -> Result<Self, ServerRenderError> {
        if output.format.image_extension().is_none() {
            return Err(ServerRenderError::new(
                ErrorCode::InvalidOutput,
                "an image render needs a png, jpeg, png_sequence or jpeg_sequence format",
                false,
            ));
        }
        let frames = output.resolve(composition.timeline().duration_frames)?;
        let devices = reserve_devices(
            EncoderFamily::Other,
            media_store.video_options.prefer_hardware_decode,
            &cancel,
        )?;
        let mut renderer = create_gpu_renderer(&mut media_store).map_err(gpu_renderer_failed)?;
        // A JPEG is the frame over black, composited in linear light; a PNG keeps straight alpha.
        renderer.set_output_alpha(if output.format.is_jpeg() {
            OutputAlpha::Opaque
        } else {
            OutputAlpha::Straight
        });
        renderer
            .compile_with_output_format(composition, TextureFormat::Rgba8Unorm)
            .map_err(|err| ServerRenderError::engine("failed to compile composition", err))?;
        Ok(Self {
            composition,
            media_store,
            renderer,
            format: output.format,
            jpeg_quality: output.jpeg_quality(),
            frames,
            cancel,
            _devices: devices,
        })
    }

    /// Sets the most glyph, stroke and decoration instances one text node may draw, as
    /// [`GpuCompositionRenderer::set_max_text_glyphs`] does.
    pub fn set_max_text_glyphs(&mut self, glyphs: usize) {
        self.renderer.set_max_text_glyphs(glyphs);
    }

    /// The frames this render draws.
    pub fn frames(&self) -> Range<u32> {
        self.frames.clone()
    }

    /// Renders each frame of the range in order and passes it to `on_image`. An error from it ends
    /// the render: a [`ServerRenderError`] as it is, and any other as [`ErrorCode::OutputIo`].
    /// Cancelling stops it before the next frame.
    pub fn run(
        mut self,
        on_image: &mut dyn FnMut(EncodedImage) -> anyhow::Result<()>,
        on_progress: &mut dyn FnMut(RenderProgress),
    ) -> Result<RenderReport, ServerRenderError> {
        let started = Instant::now();
        let total_frames = self.frames.end - self.frames.start;
        let mut timing = RenderTiming::default();
        for (index, frame) in self.frames.clone().enumerate() {
            self.cancel.check()?;
            prefetch_media_frames(self.composition, &self.media_store, frame);
            let (raster, _submission) = render_raster(
                &mut self.renderer,
                self.composition,
                &self.media_store,
                frame,
                &mut timing,
            )?;
            let size = checked_size(self.composition, &raster, frame)?;
            let pixels = timed(&mut timing.readback, || {
                self.renderer
                    .gpu_renderer()
                    .read_texture_rgba8(&raster.texture, size)
            })
            .map_err(readback_failed)?;
            let bytes = timed(&mut timing.encode_send, || {
                encode_image(pixels, size, self.format, self.jpeg_quality)
            })
            .map_err(|err| {
                ServerRenderError::new(
                    ErrorCode::EncodeFailed,
                    format!("failed to encode frame {frame}"),
                    false,
                )
                .caused_by(err)
            })?;
            on_image(EncodedImage { frame, bytes }).map_err(|error| {
                error
                    .downcast::<ServerRenderError>()
                    .unwrap_or_else(|error| {
                        ServerRenderError::new(
                            ErrorCode::OutputIo,
                            format!("failed to write frame {frame}"),
                            true,
                        )
                        .caused_by(error)
                    })
            })?;
            let completed = index as u32 + 1;
            on_progress(RenderProgress {
                stage: "rendering",
                frame: completed,
                total_frames,
                ratio: (completed as f32 / total_frames as f32).clamp(0.0, 1.0),
            });
        }
        timing.total = started.elapsed();
        Ok(RenderReport {
            frames: total_frames,
            diagnostics: self.renderer.take_diagnostics(),
            held_bytes: self.renderer.held_bytes(),
            submitted_passes: self.renderer.submitted_pass_count(),
            skipped_passes: self.renderer.skipped_pass_count(),
            timing,
        })
    }
}

/// RGBA pixels as a PNG, or their colour as a JPEG.
fn encode_image(
    pixels: Vec<u8>,
    size: Size,
    format: OutputFormat,
    jpeg_quality: u8,
) -> anyhow::Result<Vec<u8>> {
    let mut encoded = Vec::new();
    if format.is_jpeg() {
        let rgb: Vec<u8> = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
            .collect();
        JpegEncoder::new_with_quality(&mut encoded, jpeg_quality).write_image(
            &rgb,
            size.width,
            size.height,
            ExtendedColorType::Rgb8,
        )?;
    } else {
        PngEncoder::new(&mut encoded).write_image(
            &pixels,
            size.width,
            size.height,
            ExtendedColorType::Rgba8,
        )?;
    }
    Ok(encoded)
}
