use std::path::Path;

use lumen_engine::{
    audio::{AUDIO_CHANNELS, AUDIO_SAMPLE_RATE},
    composition::{BitDepth, Composition},
    gpu::{CudaNvencTarget, CudaNvencTargetPool, GpuCompositionRenderer, RenderTarget},
};
use lumen_ffmpeg::{
    AudioEncoderConfig, EncodeMode, EncoderQuality, GpuBackend, MuxedEncoder, Rational,
    VideoEncoderConfig,
};

use super::{
    ErrorCode, RenderTiming, ServerRenderError, create_gpu_renderer, encode_failed,
    encoder_start_failed, gpu_renderer_failed, has_audio, poll, timed,
};
use crate::render::media::LocalMediaStore;

pub(in crate::render) struct CudaTarget {
    target: CudaNvencTarget,
    _pool: CudaNvencTargetPool,
    encoder: MuxedEncoder,
}

impl CudaTarget {
    pub(super) fn open(
        composition: &Composition,
        media_store: &mut LocalMediaStore,
        output: &Path,
        encoder: &str,
        quality: &EncoderQuality,
        depth: BitDepth,
    ) -> Result<(GpuCompositionRenderer, Self), ServerRenderError> {
        let renderer = create_gpu_renderer(media_store).map_err(gpu_renderer_failed)?;
        renderer
            .check_output_size(composition)
            .map_err(|err| ServerRenderError::engine("the output can't be rendered", err))?;
        let settings = composition.render_settings();
        let (width, height) = (settings.width, settings.height);
        let pool = CudaNvencTargetPool::new(
            renderer.gpu_renderer(),
            lumen_gpu::Size::new(width, height),
            depth,
            true,
        )
        .map_err(|err| {
            ServerRenderError::new(
                ErrorCode::RenderFailed,
                format!("failed to create CUDA/NVENC target pool: {err}"),
                false,
            )
        })?;
        let target = pool.acquire(renderer.gpu_renderer()).map_err(|err| {
            ServerRenderError::new(
                ErrorCode::RenderFailed,
                format!("failed to create CUDA/NVENC target: {err}"),
                false,
            )
        })?;
        tracing::info!(
            cuda_ordinal = pool.cuda_ordinal(),
            vulkan_device = %pool.vulkan_device().name,
            "created CUDA primary context for Vulkan device"
        );

        let frame_rate =
            Rational::from_fps(f64::from(composition.timeline().fps)).map_err(|err| {
                ServerRenderError::new(ErrorCode::InvalidRenderProfile, err.to_string(), false)
            })?;
        let mut config = VideoEncoderConfig::cpu_rgba(width, height, frame_rate, encoder);
        config.mode = EncodeMode::GpuTexture(GpuBackend::Cuda);
        config.cuda_ordinal = Some(pool.cuda_ordinal());
        config.quality = quality.clone();
        config.bit_depth = match depth {
            BitDepth::Eight => lumen_ffmpeg::BitDepth::Eight,
            BitDepth::Ten => lumen_ffmpeg::BitDepth::Ten,
        };
        let audio = has_audio(composition)
            .then(|| AudioEncoderConfig::aac(AUDIO_SAMPLE_RATE, AUDIO_CHANNELS as u16));
        let encoder =
            MuxedEncoder::create_with_audio(output.to_string_lossy().to_string(), config, audio)
                .map_err(|err| encoder_start_failed(err.into()))?;
        Ok((
            renderer,
            Self {
                target,
                _pool: pool,
                encoder,
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
    ) -> Result<(), ServerRenderError> {
        timed(&mut timing.render, || {
            renderer.render(
                composition,
                frame,
                media_store,
                RenderTarget::External(self.target.external_texture()),
            )
        })
        .map_err(|err| ServerRenderError::engine(format!("render failed at frame {frame}"), err))?;
        let converted = timed(&mut timing.render, || {
            self.target.convert(renderer.gpu_renderer())
        });
        poll(renderer, frame, timing, Some(converted))?;
        timed(&mut timing.encode_send, || {
            let frame_ref = self
                .target
                .video_frame(Some(i64::from(pts)))
                .map_err(|err| {
                    encode_failed(format!("CUDA frame failed at frame {frame}: {err}"))
                })?;
            self.encoder
                .write_gpu_frame(&self.target.video_input(&frame_ref))
                .map_err(|err| encode_failed(format!("encode failed at frame {frame}: {err}")))
        })
    }

    pub(super) fn into_encoder(self) -> MuxedEncoder {
        self.encoder
    }
}
