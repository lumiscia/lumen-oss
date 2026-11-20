use std::path::Path;

use crate::{
    audio::{AUDIO_CHANNELS, AUDIO_SAMPLE_RATE},
    composition::Composition,
    gpu::{GpuCompositionRenderer, RenderedFrame, VulkanVideoTarget},
};
use anyhow::{Context, anyhow};
use lumen_ffmpeg::{
    AudioEncoderConfig, EncodeMode, Encoder, EncoderQuality, GpuBackend, GpuVideoInput,
    MuxedEncoder, Rational, VideoEncoderConfig,
};
use lumen_gpu::wgpu::SubmissionIndex;

use super::{
    Error, RenderTiming, checked_size, encode_failed, has_audio, poll, render_failed,
    render_raster, timed,
};
use crate::render::{encoder::warn_if_container_cropped, media::LocalMediaStore};

pub(in crate::render) struct VulkanTarget {
    target: VulkanVideoTarget,
    encoder: MuxedEncoder,
    previous_render: Option<SubmissionIndex>,
}

impl VulkanTarget {
    /// Opens a renderer and encoder on one Vulkan device, so frames never leave the GPU. Decoders
    /// share the device too where the hardware has them.
    pub(super) fn open(
        composition: &Composition,
        media_store: &mut LocalMediaStore,
        output: &Path,
        encoder: &Encoder,
        quality: &EncoderQuality,
        depth: lumen_ffmpeg::BitDepth,
    ) -> anyhow::Result<(GpuCompositionRenderer, Self)> {
        let (mut renderer, device) = pollster::block_on(
            GpuCompositionRenderer::new_vulkan_video_with_settings(&media_store.gpu),
        )?;
        renderer.set_render_scale(media_store.render_scale)?;
        let lumen_gpu::Size { width, height } = renderer.output_size(composition);
        if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            return Err(anyhow!(
                "YUV 4:2:0 frames for Vulkan encode need even dimensions, got {width}x{height}"
            ));
        }
        renderer.check_output_size(composition)?;
        let target = VulkanVideoTarget::new(
            renderer.gpu_renderer(),
            lumen_gpu::Size::new(width, height),
            depth.into(),
            true,
        );
        let mut config = VideoEncoderConfig::cpu_rgba(
            width,
            height,
            Rational::from_fps(f64::from(composition.timeline().fps))?,
            encoder,
        );
        config.mode = EncodeMode::GpuTexture(GpuBackend::Vulkan);
        config.vulkan_device = Some(device.clone());
        config.quality = quality.clone();
        config.bit_depth = depth;
        let audio = has_audio(composition)
            .then(|| AudioEncoderConfig::aac(AUDIO_SAMPLE_RATE, AUDIO_CHANNELS as u16));
        let encoder =
            MuxedEncoder::create_with_audio(output.to_string_lossy().to_string(), config, audio)
                .context("lumen-ffmpeg Vulkan encoder failed to start")?;
        warn_if_container_cropped(&encoder);
        media_store.video_options.vulkan_device = device.can_decode().then_some(device);
        Ok((
            renderer,
            Self {
                target,
                encoder,
                previous_render: None,
            },
        ))
    }

    pub(super) fn write_frame(
        &mut self,
        renderer: &mut GpuCompositionRenderer,
        composition: &Composition,
        media_store: &LocalMediaStore,
        frame: u32,
        pts: u32,
        timing: &mut RenderTiming,
    ) -> Result<(), Error> {
        let RenderedFrame { raster, submission } =
            render_raster(renderer, composition, media_store, frame, timing)?;
        checked_size(renderer.output_size(composition), &raster, frame)?;
        let video_frame = timed(&mut timing.render, || {
            self.target.convert(
                renderer.gpu_renderer(),
                &raster.texture,
                Some(i64::from(pts)),
            )
        })
        .map_err(|err| render_failed(format!("YUV conversion failed at frame {frame}: {err}")))?;
        // Holds the GPU to the frame before this one, so a frame waits for about one frame's work.
        // With the queue free to run ahead, that wait builds up in the encoder and lands on every
        // third frame, where B-frames make it take three at once.
        poll(
            renderer,
            frame,
            timing,
            self.previous_render.replace(submission),
        )?;
        timed(&mut timing.encode_send, || {
            self.encoder
                .write_gpu_frame(&GpuVideoInput::Vulkan(&video_frame))
        })
        .map_err(|err| encode_failed(format!("encode failed at frame {frame}: {err}")))
    }

    pub(super) fn into_encoder(self) -> MuxedEncoder {
        self.encoder
    }
}
