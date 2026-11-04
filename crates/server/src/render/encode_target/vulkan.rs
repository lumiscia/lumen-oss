use std::path::Path;

use anyhow::{Context, anyhow};
use lumen_engine::{
    audio::{AUDIO_CHANNELS, AUDIO_SAMPLE_RATE},
    composition::Composition,
    gpu::{GpuCompositionRenderer, VulkanVideoTarget},
};
use lumen_ffmpeg::{
    AudioEncoderConfig, EncodeMode, EncoderQuality, GpuBackend, GpuVideoInput, MuxedEncoder,
    Rational, VideoEncoderConfig,
};

use super::{
    RenderError, RenderTiming, checked_size, encode_failed, has_audio, missing_output, poll,
    precompile_ahead, render_failed, render_raster, timed,
};
use crate::render::{encoder::warn_if_container_cropped, media::LocalMediaStore};

pub(in crate::render) struct VulkanTarget {
    target: VulkanVideoTarget,
    encoder: MuxedEncoder,
}

impl VulkanTarget {
    /// Opens a renderer and encoder on one Vulkan device, so frames never leave the GPU. Decoders
    /// share the device too where the hardware has them.
    pub(super) fn open(
        composition: &Composition,
        media_store: &mut LocalMediaStore,
        output: &Path,
        encoder: &str,
        quality: &EncoderQuality,
    ) -> anyhow::Result<(GpuCompositionRenderer, Self)> {
        let settings = composition.render_settings();
        let (width, height) = (settings.width, settings.height);
        if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            return Err(anyhow!(
                "NV12 frames need even dimensions, got {width}x{height}"
            ));
        }
        let (renderer, device) = pollster::block_on(GpuCompositionRenderer::new_vulkan_video())?;
        let target =
            VulkanVideoTarget::nv12(renderer.gpu_renderer(), lumen_gpu::Size::new(width, height));
        let mut config = VideoEncoderConfig::cpu_rgba(
            width,
            height,
            Rational::from_fps(f64::from(composition.timeline().fps))?,
            encoder,
        );
        config.mode = EncodeMode::GpuTexture(GpuBackend::Vulkan);
        config.vulkan_device = Some(device.clone());
        config.quality = quality.clone();
        let audio = has_audio(composition)
            .then(|| AudioEncoderConfig::aac(AUDIO_SAMPLE_RATE, AUDIO_CHANNELS as u16));
        let encoder =
            MuxedEncoder::create_with_audio(output.to_string_lossy().to_string(), config, audio)
                .context("lumen-ffmpeg Vulkan encoder failed to start")?;
        warn_if_container_cropped(&encoder);
        media_store.video_options.vulkan_device = device.can_decode().then_some(device);
        Ok((renderer, Self { target, encoder }))
    }

    pub(super) fn write_frame(
        &mut self,
        renderer: &mut GpuCompositionRenderer,
        composition: &Composition,
        media_store: &LocalMediaStore,
        frame: u32,
        timing: &mut RenderTiming,
    ) -> Result<(), RenderError> {
        let raster = render_raster(renderer, composition, media_store, frame, timing)?;
        checked_size(composition, &raster, frame)?;
        let texture = renderer
            .gpu_renderer()
            .texture(raster.texture)
            .ok_or_else(|| missing_output(&raster, frame))?;
        let video_frame = timed(&mut timing.render, || {
            self.target
                .convert(renderer.gpu_renderer(), texture, Some(i64::from(frame)))
        })
        .map_err(|err| render_failed(format!("NV12 conversion failed at frame {frame}: {err}")))?;
        precompile_ahead(renderer, composition, media_store, frame);
        poll(renderer, frame, timing)?;
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
