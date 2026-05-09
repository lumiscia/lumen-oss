use std::{fs, time::Instant};

use lumen::composition::Composition;
use lumen_ffmpeg::{
    AudioEncoderConfig, CudaDriver, EncodeMode, GpuBackend, GpuVideoInput, MuxedEncoder,
    VideoCodec, VideoEncoderConfig, import_owned_vulkan_opaque_fd_image,
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
        message: format!("failed to create GPU renderer: {err}"),
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
    let exportable = renderer
        .gpu_renderer()
        .create_exportable_vulkan_texture(
            Some("lumen-server vk-cuda encode texture"),
            size,
            lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            lumen_gpu::wgpu::TextureUsages::COPY_DST
                | lumen_gpu::wgpu::TextureUsages::COPY_SRC
                | lumen_gpu::wgpu::TextureUsages::TEXTURE_BINDING,
        )
        .map_err(|err| RenderError {
            code: "render_failed",
            message: format!("failed to create exportable Vulkan texture: {err}"),
            retryable: false,
        })?;
    let vulkan_device = exportable.device_info().clone();
    if verbose_debug {
        tracing::debug!(
            width,
            height,
            allocation_size = exportable.allocation_size(),
            row_pitch = exportable.row_pitch(),
            memory_fd = exportable.memory_fd_raw(),
            memory_type_index = exportable.memory_type_index(),
            vulkan_device_name = %vulkan_device.name,
            vulkan_vendor_id = vulkan_device.vendor_id,
            vulkan_device_id = vulkan_device.device_id,
            vulkan_device_type = %vulkan_device.device_type,
            vulkan_device_uuid = %format_uuid(&vulkan_device.device_uuid),
            vulkan_driver_uuid = %format_uuid(&vulkan_device.driver_uuid),
            "created exportable Vulkan texture for CUDA import"
        );
    }
    let driver = CudaDriver::load().map_err(|err| RenderError {
        code: "encode_failed",
        message: format!("failed to load CUDA driver: {err}"),
        retryable: false,
    })?;
    match driver.driver_version() {
        Ok(version) if verbose_debug => {
            tracing::debug!(cuda_driver_version = version, "loaded CUDA driver");
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(error = %err, "failed to query CUDA driver version"),
    }
    let cuda_devices = driver.devices().map_err(|err| RenderError {
        code: "encode_failed",
        message: format!("failed to enumerate CUDA devices: {err}"),
        retryable: false,
    })?;
    for device in &cuda_devices {
        if verbose_debug {
            tracing::debug!(
                cuda_ordinal = device.ordinal,
                cuda_name = %device.name,
                cuda_uuid = %format_uuid(&device.uuid),
                cuda_pci_bus_id = %device.pci_bus_id,
                matches_vulkan_device = device.uuid == vulkan_device.device_uuid,
                "found CUDA device"
            );
        }
    }
    let cuda_ordinal = cuda_devices
        .iter()
        .find(|device| device.uuid == vulkan_device.device_uuid)
        .map(|device| device.ordinal)
        .ok_or_else(|| RenderError {
            code: "encode_failed",
            message: format!(
                "no CUDA device UUID matched Vulkan device {} ({})",
                vulkan_device.name,
                format_uuid(&vulkan_device.device_uuid)
            ),
            retryable: false,
        })?;
    let context = driver
        .create_primary_context_for_ordinal(cuda_ordinal)
        .map_err(|err| RenderError {
            code: "encode_failed",
            message: format!(
                "failed to create CUDA primary context for device {cuda_ordinal}: {err}"
            ),
            retryable: false,
        })?;
    tracing::info!(
        cuda_ordinal = context.ordinal(),
        vulkan_device_uuid = %format_uuid(&vulkan_device.device_uuid),
        "created CUDA primary context for Vulkan device"
    );
    let imported = import_owned_vulkan_opaque_fd_image(
        &driver,
        exportable
            .memory_fd()
            .try_clone()
            .map_err(|err| RenderError {
                code: "encode_failed",
                message: format!("failed to duplicate Vulkan memory fd: {err}"),
                retryable: false,
            })?,
        exportable.allocation_size(),
        width,
        height,
    )
    .map_err(|err| RenderError {
        code: "encode_failed",
        message: format!(
            "failed to import Vulkan image into CUDA on device {cuda_ordinal}; vulkan_device={} vulkan_uuid={} allocation_size={} row_pitch={} fd={}: {err}",
            vulkan_device.name,
            format_uuid(&vulkan_device.device_uuid),
            exportable.allocation_size(),
            exportable.row_pitch(),
            exportable.memory_fd_raw()
        ),
        retryable: false,
    })?;
    let cuda_frame = driver
        .allocate_rgba_frame(width, height)
        .map_err(|err| RenderError {
            code: "encode_failed",
            message: format!("failed to allocate CUDA RGBA frame: {err}"),
            retryable: false,
        })?;

    let include_audio = has_audio(composition);
    let mut config =
        VideoEncoderConfig::cpu_rgba(width, height, fps.round().max(1.0) as u32, codec);
    config.mode = EncodeMode::GpuTexture(GpuBackend::Cuda);
    config.bit_rate = 14_000_000;
    let audio = include_audio.then(|| {
        AudioEncoderConfig::aac(
            lumen::audio::AUDIO_SAMPLE_RATE,
            lumen::audio::AUDIO_CHANNELS as u16,
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
        let (raster, _) = renderer
            .render_frame_submitted(composition, frame, media_store)
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
        renderer
            .gpu_renderer()
            .copy_texture_to_external(raster.texture, exportable.texture())
            .map_err(|err| RenderError {
                code: "render_failed",
                message: format!("failed to copy frame {frame} into exportable texture: {err}"),
                retryable: true,
            })?;
        renderer
            .gpu_renderer()
            .device
            .poll(lumen_gpu::wgpu::PollType::wait_indefinitely())
            .map_err(|err| RenderError {
                code: "render_failed",
                message: format!("GPU poll failed at frame {frame}: {err}"),
                retryable: true,
            })?;
        timing.gpu_wait = started.elapsed();

        let started = Instant::now();
        context.set_current().map_err(|err| RenderError {
            code: "encode_failed",
            message: format!("failed to restore CUDA context: {err}"),
            retryable: true,
        })?;
        driver
            .copy_image_to_rgba_frame(&imported, &cuda_frame)
            .map_err(|err| RenderError {
                code: "encode_failed",
                message: format!("failed to copy Vulkan frame into CUDA frame: {err}"),
                retryable: true,
            })?;
        timing.cuda_copy = started.elapsed();

        let started = Instant::now();
        let frame_ref = cuda_frame.as_video_frame(Some(i64::from(frame)));
        encoder
            .write_gpu_frame(&GpuVideoInput::Cuda(&frame_ref))
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
