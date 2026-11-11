use std::{collections::VecDeque, path::Path};

use anyhow::Context;
use lumen_engine::{
    audio::{AUDIO_CHANNELS, AUDIO_SAMPLE_RATE},
    composition::Composition,
    gpu::{GpuCompositionRenderer, MetalVideoToolboxTarget, MetalVideoToolboxTargetPool},
};
use lumen_ffmpeg::{
    AudioEncoderConfig, EncodeMode, EncoderQuality, GpuBackend, MuxedEncoder, Rational,
    VideoEncoderConfig,
};

use super::{RenderTiming, ServerRenderError, encode_failed, has_audio, poll_failed, timed};
use crate::render::media::LocalMediaStore;

const FRAMES_IN_FLIGHT: usize = 3;

struct PendingFrame {
    frame: u32,
    target: MetalVideoToolboxTarget,
    submitted: lumen_gpu::SubmittedExternalTexture,
}

pub(in crate::render) struct MetalTarget {
    pool: MetalVideoToolboxTargetPool,
    pending: VecDeque<PendingFrame>,
    encoder: MuxedEncoder,
}

impl MetalTarget {
    pub(super) fn open(
        composition: &Composition,
        gpu: &lumen_gpu::GpuSettings,
        output: &Path,
        encoder: &str,
        quality: &EncoderQuality,
    ) -> anyhow::Result<(GpuCompositionRenderer, Self)> {
        let renderer = pollster::block_on(GpuCompositionRenderer::new_with_settings(gpu))
            .context("failed to initialize wgpu renderer")?;
        renderer.check_output_size(composition)?;
        let settings = composition.render_settings();
        let (width, height) = (settings.width, settings.height);
        let pool = MetalVideoToolboxTargetPool::bgra8(
            renderer.gpu_renderer(),
            lumen_gpu::Size::new(width, height),
        )?;
        let mut config = VideoEncoderConfig::cpu_rgba(
            width,
            height,
            Rational::from_fps(f64::from(composition.timeline().fps))?,
            encoder,
        );
        config.mode = EncodeMode::GpuTexture(GpuBackend::Metal);
        config.quality = quality.clone();
        let audio = has_audio(composition)
            .then(|| AudioEncoderConfig::aac(AUDIO_SAMPLE_RATE, AUDIO_CHANNELS as u16));
        let encoder =
            MuxedEncoder::create_with_audio(output.to_string_lossy().to_string(), config, audio)
                .context("lumen-ffmpeg VideoToolbox encoder failed to start")?;
        Ok((
            renderer,
            Self {
                pool,
                pending: VecDeque::new(),
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
    ) -> Result<(), ServerRenderError> {
        let target = self
            .pool
            .acquire(renderer.gpu_renderer(), frame)
            .map_err(|err| {
                ServerRenderError::engine(format!("failed to acquire frame {frame}"), err)
            })?;
        let submitted = timed(&mut timing.render, || {
            renderer.render_frame_into_external(
                composition,
                frame,
                media_store,
                target.external_texture(),
            )
        })
        .map_err(|err| ServerRenderError::engine(format!("render failed at frame {frame}"), err))?;
        self.pending.push_back(PendingFrame {
            frame,
            target,
            submitted,
        });
        if self.pending.len() >= FRAMES_IN_FLIGHT {
            self.encode_oldest(renderer, timing)?;
        }
        Ok(())
    }

    /// Encodes the frames still in flight and returns the encoder.
    pub(super) fn drain(
        mut self,
        renderer: &GpuCompositionRenderer,
        timing: &mut RenderTiming,
    ) -> Result<MuxedEncoder, ServerRenderError> {
        while !self.pending.is_empty() {
            self.encode_oldest(renderer, timing)?;
        }
        Ok(self.encoder)
    }

    fn encode_oldest(
        &mut self,
        renderer: &GpuCompositionRenderer,
        timing: &mut RenderTiming,
    ) -> Result<(), ServerRenderError> {
        let Some(PendingFrame {
            frame,
            target,
            submitted,
        }) = self.pending.pop_front()
        else {
            return Ok(());
        };
        timed(&mut timing.flush, || {
            submitted.wait(&renderer.gpu_renderer().device)
        })
        .map_err(|err| poll_failed(frame, err))?;
        timed(&mut timing.encode_send, || {
            self.encoder.write_gpu_frame(&target.video_input())
        })
        .map_err(|err| encode_failed(format!("encode failed at frame {frame}: {err}")))
    }
}
