use std::path::Path;

use lumen_engine::{
    audio::{AUDIO_CHANNELS, AUDIO_SAMPLE_RATE},
    composition::Composition,
    gpu::{CudaNvencTarget, CudaNvencTargetPool, GpuCompositionRenderer},
};
use lumen_ffmpeg::{
    AudioEncoderConfig, EncodeMode, EncoderQuality, GpuBackend, MuxedEncoder, Rational,
    VideoEncoderConfig,
};

use super::{
    RenderError, RenderTiming, create_gpu_renderer, encode_failed, has_audio, poll_failed,
    render_failed, timed,
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
    ) -> Result<(GpuCompositionRenderer, Self), RenderError> {
        let renderer = create_gpu_renderer(media_store).map_err(|err| RenderError {
            code: "render_failed",
            message: format!("failed to create GPU renderer: {err:#}"),
            retryable: true,
        })?;
        let settings = composition.render_settings();
        let (width, height) = (settings.width, settings.height);
        let pool = CudaNvencTargetPool::rgba8(
            renderer.gpu_renderer(),
            lumen_gpu::Size::new(width, height),
        )
        .map_err(|err| RenderError {
            code: "render_failed",
            message: format!("failed to create CUDA/NVENC target pool: {err}"),
            retryable: false,
        })?;
        let target = pool
            .acquire(renderer.gpu_renderer())
            .map_err(|err| RenderError {
                code: "render_failed",
                message: format!("failed to create CUDA/NVENC target: {err}"),
                retryable: false,
            })?;
        tracing::info!(
            cuda_ordinal = pool.cuda_ordinal(),
            vulkan_device = %pool.vulkan_device().name,
            "created CUDA primary context for Vulkan device"
        );

        let frame_rate =
            Rational::from_fps(f64::from(composition.timeline().fps)).map_err(|err| {
                RenderError {
                    code: "invalid_fps",
                    message: err.to_string(),
                    retryable: false,
                }
            })?;
        let mut config = VideoEncoderConfig::cpu_rgba(width, height, frame_rate, encoder);
        config.mode = EncodeMode::GpuTexture(GpuBackend::Cuda);
        config.cuda_ordinal = Some(pool.cuda_ordinal());
        config.quality = quality.clone();
        let audio = has_audio(composition)
            .then(|| AudioEncoderConfig::aac(AUDIO_SAMPLE_RATE, AUDIO_CHANNELS as u16));
        let encoder =
            MuxedEncoder::create_with_audio(output.to_string_lossy().to_string(), config, audio)
                .map_err(|err| RenderError {
                    code: "encode_failed",
                    message: err.to_string(),
                    retryable: false,
                })?;
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
        timing: &mut RenderTiming,
    ) -> Result<(), RenderError> {
        let submitted = timed(&mut timing.render, || {
            renderer.render_frame_into_external(
                composition,
                frame,
                media_store,
                self.target.external_texture(),
            )
        })
        .map_err(|err| render_failed(format!("render failed at frame {frame}: {err}")))?;
        timed(&mut timing.flush, || {
            submitted.wait(&renderer.gpu_renderer().device)
        })
        .map_err(|err| poll_failed(frame, err))?;
        timed(&mut timing.encode_send, || {
            self.target.copy_rendered_frame_to_cuda().map_err(|err| {
                encode_failed(format!(
                    "failed to copy Vulkan frame into CUDA at frame {frame}: {err}"
                ))
            })?;
            let frame_ref = self.target.video_frame(Some(i64::from(frame)));
            self.encoder
                .write_gpu_frame(&self.target.video_input(&frame_ref))
                .map_err(|err| encode_failed(format!("encode failed at frame {frame}: {err}")))
        })
    }

    pub(super) fn into_encoder(self) -> MuxedEncoder {
        self.encoder
    }
}
