use std::{fs, time::Instant};

use lumen_engine::composition::Composition;
use lumen_engine::gpu::CudaNvencTargetPool;
use lumen_ffmpeg::{
    AudioEncoderConfig, EncodeMode, GpuBackend, MuxedEncoder, VideoCodec, VideoEncoderConfig,
};

use crate::render::{
    RenderError, RenderProgress,
    encoder::has_audio,
    frame_timing::{FRAME_TIMING_LOG_INTERVAL, FrameTiming, FrameTimingTotals},
    media::LocalMediaStore,
};

use super::{MEDIA_PREFETCH_LOOKAHEAD_FRAMES, create_gpu_renderer, prefetch_media_frames};

#[allow(clippy::too_many_arguments)]
pub(super) fn render_project_mp4_cuda(
    composition: &Composition,
    media_store: &LocalMediaStore,
    width: u32,
    height: u32,
    fps: f32,
    total_frames: u32,
    encoder_name: &str,
    codec: VideoCodec,
    verbose_debug: bool,
    on_progress: &mut dyn FnMut(RenderProgress),
) -> Result<Vec<u8>, RenderError> {
    let tmp = tempfile::tempdir().map_err(|err| RenderError {
        code: "encode_failed",
        message: format!("failed to create temp dir: {err}"),
        retryable: true,
    })?;
    let output_path = tmp.path().join("output.mp4");
    let mut renderer = create_gpu_renderer().map_err(|err| RenderError {
        code: "render_failed",
        message: format!("failed to create GPU renderer: {err:#}"),
        retryable: true,
    })?;
    renderer
        .compile_with_media(
            composition,
            media_store,
            lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
        )
        .map_err(|err| RenderError {
            code: "render_failed",
            message: format!("failed to compile composition: {err}"),
            retryable: false,
        })?;

    let size = lumen_gpu::Size::new(width, height);
    let target_pool =
        CudaNvencTargetPool::rgba8(renderer.gpu_renderer(), size).map_err(|err| RenderError {
            code: "render_failed",
            message: format!("failed to create CUDA/NVENC target pool: {err}"),
            retryable: false,
        })?;
    let target = target_pool
        .acquire(renderer.gpu_renderer())
        .map_err(|err| RenderError {
            code: "render_failed",
            message: format!("failed to create CUDA/NVENC target: {err}"),
            retryable: false,
        })?;
    let vulkan_device = target_pool.vulkan_device();
    if verbose_debug {
        tracing::debug!(
            width,
            height,
            allocation_size = target.allocation_size(),
            row_pitch = target.row_pitch(),
            memory_fd = target.memory_fd_raw(),
            memory_type_index = target.memory_type_index(),
            cuda_ordinal = target_pool.cuda_ordinal(),
            vulkan_device_name = %vulkan_device.name,
            vulkan_vendor_id = vulkan_device.vendor_id,
            vulkan_device_id = vulkan_device.device_id,
            vulkan_device_type = %vulkan_device.device_type,
            vulkan_device_uuid = %format_uuid(&vulkan_device.device_uuid),
            vulkan_driver_uuid = %format_uuid(&vulkan_device.driver_uuid),
            "created CUDA/NVENC render target"
        );
    }
    match target_pool.driver().driver_version() {
        Ok(version) if verbose_debug => {
            tracing::debug!(cuda_driver_version = version, "loaded CUDA driver");
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(error = %err, "failed to query CUDA driver version"),
    }
    tracing::info!(
        cuda_ordinal = target_pool.cuda_ordinal(),
        vulkan_device_uuid = %format_uuid(&vulkan_device.device_uuid),
        "created CUDA primary context for Vulkan device"
    );

    let include_audio = has_audio(composition);
    let mut config =
        VideoEncoderConfig::cpu_rgba(width, height, fps.round().max(1.0) as u32, codec);
    config.encoder_name = Some(encoder_name.to_string());
    config.mode = EncodeMode::GpuTexture(GpuBackend::Cuda);
    config.bit_rate = 14_000_000;
    let audio = include_audio.then(|| {
        AudioEncoderConfig::aac(
            lumen_engine::audio::AUDIO_SAMPLE_RATE,
            lumen_engine::audio::AUDIO_CHANNELS as u16,
        )
    });
    let mut encoder =
        MuxedEncoder::create_with_audio(output_path.to_string_lossy().to_string(), config, audio)
            .map_err(|err| RenderError {
            code: "encode_failed",
            message: err.to_string(),
            retryable: true,
        })?;

    tracing::info!(
        width,
        height,
        total_frames,
        "using Vulkan-to-CUDA NVENC render path"
    );
    let mut timing_totals = FrameTimingTotals::default();
    for frame in 0..total_frames {
        let mut timing = FrameTiming::default();
        let started = Instant::now();
        prefetch_media_frames(composition, media_store, frame);
        timing.prefetch = started.elapsed();

        let started = Instant::now();
        let submitted = renderer
            .render_frame_into_external(composition, frame, media_store, target.external_texture())
            .map_err(|err| RenderError {
                code: "render_failed",
                message: format!("render failed at frame {frame}: {err}"),
                retryable: true,
            })?;
        timing.render_submit = started.elapsed();

        let started = Instant::now();
        let _ = renderer.precompile_frame_window(
            composition,
            frame.saturating_add(1),
            MEDIA_PREFETCH_LOOKAHEAD_FRAMES,
            media_store,
        );
        timing.precompile = started.elapsed();

        let started = Instant::now();
        submitted
            .wait(&renderer.gpu_renderer().device)
            .map_err(|err| RenderError {
                code: "render_failed",
                message: format!("GPU poll failed at frame {frame}: {err}"),
                retryable: true,
            })?;
        timing.gpu_wait = started.elapsed();

        let started = Instant::now();
        target
            .copy_rendered_frame_to_cuda()
            .map_err(|err| RenderError {
                code: "encode_failed",
                message: format!("failed to copy Vulkan frame into CUDA frame: {err}"),
                retryable: true,
            })?;
        timing.cuda_copy = started.elapsed();

        let started = Instant::now();
        let frame_ref = target.video_frame(Some(i64::from(frame)));
        encoder
            .write_gpu_frame(&target.video_input(&frame_ref))
            .map_err(|err| RenderError {
                code: "encode_failed",
                message: err.to_string(),
                retryable: true,
            })?;
        timing.encode_write = started.elapsed();

        let completed = frame.saturating_add(1);
        let ratio = (completed as f32 / total_frames as f32).clamp(0.0, 1.0);
        let started = Instant::now();
        on_progress(RenderProgress {
            stage: "rendering",
            frame: completed,
            total_frames,
            ratio,
        });
        timing.progress = started.elapsed();
        timing_totals.add(timing);
        if verbose_debug
            && (completed % FRAME_TIMING_LOG_INTERVAL == 0 || completed == total_frames)
        {
            timing_totals.log(completed, total_frames, "cuda_nvenc");
        }
    }

    let finish_started = Instant::now();
    if include_audio {
        let audio_started = Instant::now();
        crate::render::encoder::write_composited_audio_with(composition, media_store, |frame| {
            encoder
                .write_audio_frame(&frame)
                .map_err(|err| anyhow::anyhow!(err.to_string()))
        })
        .map_err(|err| RenderError {
            code: "audio_render_failed",
            message: err.to_string(),
            retryable: true,
        })?;
        if verbose_debug {
            tracing::debug!(
                audio_ms = audio_started.elapsed().as_millis(),
                "finished composited audio write"
            );
        }
    }

    let encoder_finish_started = Instant::now();
    encoder.finish().map_err(|err| RenderError {
        code: "encode_failed",
        message: err.to_string(),
        retryable: true,
    })?;
    let encoder_finish_ms = encoder_finish_started.elapsed().as_millis();

    let read_started = Instant::now();
    fs::read(&output_path)
        .map_err(|err| RenderError {
            code: "encode_failed",
            message: format!("failed to read encoded output: {err}"),
            retryable: true,
        })
        .inspect(|bytes| {
            if verbose_debug {
                tracing::debug!(
                    encoder_finish_ms,
                    read_output_ms = read_started.elapsed().as_millis(),
                    finalization_ms = finish_started.elapsed().as_millis(),
                    output_bytes = bytes.len(),
                    "finished CUDA/NVENC render finalization"
                );
            }
        })
}

fn format_uuid(uuid: &[u8; 16]) -> String {
    uuid.iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("")
}
