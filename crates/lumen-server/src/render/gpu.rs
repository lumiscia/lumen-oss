use std::{env, fs, sync::mpsc};

use anyhow::Context;
use lumen::{composition::Composition, gpu::GpuCompositionRenderer, media::MediaStore};
use lumen_ffmpeg::VideoCodec;

use super::{
    ProjectBundle, RenderError, RenderOptions, RenderProgress,
    encoder::{
        ENCODER_FRAME_QUEUE_CAPACITY, EncoderFrame, LumenFfmpegEncoder, has_audio,
        write_composited_audio,
    },
    media::{LocalMediaStore, media_root},
};

const MEDIA_PREFETCH_LOOKAHEAD_FRAMES: u32 = 30;

pub fn render_project_mp4(
    bundle: &ProjectBundle,
    options: &RenderOptions,
    on_progress: &mut dyn FnMut(RenderProgress),
) -> Result<Vec<u8>, RenderError> {
    let composition = &bundle.composition;
    let width = composition.render_settings.width;
    let height = composition.render_settings.height;
    let fps = composition.timeline.fps;
    let total_frames = composition.timeline.duration_frames;

    if fps <= 0.0 {
        return Err(RenderError {
            code: "invalid_project_payload",
            message: format!("invalid timeline fps: {fps}"),
            retryable: false,
        });
    }

    if total_frames == 0 {
        return Err(RenderError {
            code: "invalid_project_payload",
            message: "composition duration_frames must be greater than zero".to_string(),
            retryable: false,
        });
    }

    let media_root = media_root(options.media_root.as_deref()).map_err(|err| RenderError {
        code: "media_root_error",
        message: err.to_string(),
        retryable: false,
    })?;
    let media_store = LocalMediaStore::new(media_root);
    let mut renderer = create_gpu_renderer().map_err(|err| RenderError {
        code: "render_failed",
        message: format!("failed to create GPU renderer: {err}"),
        retryable: true,
    })?;
    renderer
        .compile_with_media(
            composition,
            &media_store,
            lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
        )
        .map_err(|err| RenderError {
            code: "render_failed",
            message: format!("failed to compile composition: {err}"),
            retryable: false,
        })?;

    let encoder =
        choose_video_encoder(options.video_encoder.as_deref()).map_err(|err| RenderError {
            code: "invalid_render_profile",
            message: err.to_string(),
            retryable: false,
        })?;
    tracing::info!(
        encoder = encoder.name,
        codec = ?encoder.codec,
        cuda_fast_path = cfg!(all(target_os = "linux", feature = "cuda", feature = "vulkan"))
            && encoder.cuda_fast_path,
            "selected video encoder"
    );
    if encoder.cuda_fast_path {
        return render_project_mp4_cuda(
            composition,
            &media_store,
            width,
            height,
            fps,
            total_frames,
            encoder.codec,
            on_progress,
        );
    }

    let tmp = tempfile::tempdir().map_err(|err| RenderError {
        code: "encode_failed",
        message: format!("failed to create temp dir: {err}"),
        retryable: true,
    })?;
    let output_path = tmp.path().join("output.mp4");
    let include_audio = has_audio(composition);
    let encoder = LumenFfmpegEncoder::create(
        &output_path,
        width,
        height,
        fps,
        &encoder.name,
        encoder.codec,
        include_audio,
    )
    .map_err(|err| RenderError {
        code: "encode_failed",
        message: err.to_string(),
        retryable: true,
    })?;
    let (pixel_recycle_tx, _pixel_recycle_rx) =
        mpsc::sync_channel::<Vec<u8>>(ENCODER_FRAME_QUEUE_CAPACITY + 2);

    for frame in 0..total_frames {
        prefetch_media_frames(composition, &media_store, frame);
        let (raster, _submission) = renderer
            .render_frame_submitted(composition, frame, &media_store)
            .map_err(|err| RenderError {
                code: "render_failed",
                message: format!("render failed at frame {frame}: {err}"),
                retryable: true,
            })?;
        let _ = renderer.precompile_frame_window(
            composition,
            frame.saturating_add(1),
            MEDIA_PREFETCH_LOOKAHEAD_FRAMES,
            &media_store,
        );
        renderer
            .gpu_renderer()
            .device
            .poll(lumen_gpu::wgpu::PollType::Poll)
            .map_err(|err| RenderError {
                code: "render_failed",
                message: format!("GPU poll failed at frame {frame}: {err}"),
                retryable: true,
            })?;
        let size = raster.domain.storage_size;
        if size.width != width || size.height != height {
            return Err(RenderError {
                code: "render_failed",
                message: format!(
                    "frame {frame} dimensions {}x{} do not match composition {}x{}",
                    size.width, size.height, width, height
                ),
                retryable: true,
            });
        }
        let pixels =
            read_texture_rgba8(renderer.gpu_renderer(), raster.texture, size).map_err(|err| {
                RenderError {
                    code: "render_failed",
                    message: format!("failed reading rendered pixels for frame {frame}: {err}"),
                    retryable: true,
                }
            })?;
        encoder
            .send(EncoderFrame {
                frame,
                pixels,
                recycle_tx: pixel_recycle_tx.clone(),
            })
            .map_err(|err| RenderError {
                code: "encode_failed",
                message: err.to_string(),
                retryable: true,
            })?;

        let completed = frame.saturating_add(1);
        let ratio = (completed as f32 / total_frames as f32).clamp(0.0, 1.0);
        on_progress(RenderProgress {
            stage: "rendering",
            frame: completed,
            total_frames,
            ratio,
        });
    }

    if include_audio {
        write_composited_audio(composition, &media_store, &encoder).map_err(|err| RenderError {
            code: "audio_render_failed",
            message: err.to_string(),
            retryable: true,
        })?;
    }

    encoder.finish().map_err(|err| RenderError {
        code: "encode_failed",
        message: err.to_string(),
        retryable: true,
    })?;

    fs::read(&output_path).map_err(|err| RenderError {
        code: "encode_failed",
        message: format!("failed to read encoded output: {err}"),
        retryable: true,
    })
}

fn prefetch_media_frames(composition: &Composition, media_store: &LocalMediaStore, frame: u32) {
    let Ok(requirements) =
        lumen::media::collect_frame_requirements(composition, media_store, frame)
    else {
        return;
    };
    for video in requirements.videos {
        let Some(resolver) = media_store.get_video_resolver(&video.stream_id) else {
            continue;
        };
        let mut frames = video.frames;
        for required_frame in frames.clone() {
            for offset in 1..=MEDIA_PREFETCH_LOOKAHEAD_FRAMES {
                frames.push(required_frame.saturating_add(offset));
            }
        }
        frames.sort_unstable();
        frames.dedup();
        for frame in &frames {
            let _ = resolver.enqueue_frame(*frame);
        }
        resolver.retain_frames(&frames);
    }
}

fn read_texture_rgba8(
    renderer: &lumen_gpu::Renderer,
    id: lumen_gpu::TextureId,
    size: lumen_gpu::Size,
) -> anyhow::Result<Vec<u8>> {
    let bytes_per_pixel = 4;
    let unpadded_bytes_per_row = size.width.saturating_mul(bytes_per_pixel);
    let padded_bytes_per_row = align_to(
        unpadded_bytes_per_row,
        lumen_gpu::wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
    );
    let output_size = u64::from(padded_bytes_per_row).saturating_mul(u64::from(size.height));
    let output = renderer
        .device
        .create_buffer(&lumen_gpu::wgpu::BufferDescriptor {
            label: Some("lumen-server readback"),
            size: output_size.max(1),
            usage: lumen_gpu::wgpu::BufferUsages::COPY_DST
                | lumen_gpu::wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
    let mut encoder =
        renderer
            .device
            .create_command_encoder(&lumen_gpu::wgpu::CommandEncoderDescriptor {
                label: Some("lumen-server readback encoder"),
            });
    let texture = renderer
        .texture(id)
        .ok_or_else(|| anyhow::anyhow!("render output texture {id:?} is unavailable"))?;
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        lumen_gpu::wgpu::TexelCopyBufferInfo {
            buffer: &output,
            layout: lumen_gpu::wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(size.height),
            },
        },
        lumen_gpu::wgpu::Extent3d {
            width: size.width,
            height: size.height,
            depth_or_array_layers: 1,
        },
    );
    renderer.queue.submit([encoder.finish()]);

    let slice = output.slice(..);
    let (tx, rx) = mpsc::channel();
    slice.map_async(lumen_gpu::wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    renderer
        .device
        .poll(lumen_gpu::wgpu::PollType::wait_indefinitely())
        .map_err(|error| anyhow::anyhow!("GPU readback poll failed: {error}"))?;
    rx.recv()
        .map_err(|_| anyhow::anyhow!("GPU readback channel closed"))?
        .map_err(|error| anyhow::anyhow!("GPU readback map failed: {error}"))?;

    let mapped = slice.get_mapped_range();
    let mut pixels = vec![
        0;
        (size.width as usize)
            .saturating_mul(size.height as usize)
            .saturating_mul(bytes_per_pixel as usize)
    ];
    for row in 0..size.height as usize {
        let src_start = row.saturating_mul(padded_bytes_per_row as usize);
        let src_end = src_start.saturating_add(unpadded_bytes_per_row as usize);
        let dst_start = row.saturating_mul(unpadded_bytes_per_row as usize);
        let dst_end = dst_start.saturating_add(unpadded_bytes_per_row as usize);
        pixels[dst_start..dst_end].copy_from_slice(&mapped[src_start..src_end]);
    }
    drop(mapped);
    output.unmap();
    Ok(pixels)
}

fn align_to(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}

fn create_gpu_renderer() -> anyhow::Result<GpuCompositionRenderer> {
    let handle = tokio::runtime::Handle::try_current()
        .context("GPU renderer creation requires a Tokio runtime")?;
    handle
        .block_on(GpuCompositionRenderer::new())
        .context("failed to initialize wgpu renderer")
}

#[derive(Debug)]
struct VideoEncoderSelection {
    name: String,
    codec: VideoCodec,
    cuda_fast_path: bool,
}

fn choose_video_encoder(override_encoder: Option<&str>) -> anyhow::Result<VideoEncoderSelection> {
    if let Some(encoder) = override_encoder {
        let encoder = encoder.trim();
        if !encoder.is_empty() {
            return video_encoder_selection(encoder);
        }
    }

    if let Ok(encoder) = env::var("LUMEN_VIDEO_ENCODER") {
        let encoder = encoder.trim();
        if !encoder.is_empty() {
            return video_encoder_selection(encoder);
        }
    }

    let name = if cfg!(target_os = "macos") {
        "h264_videotoolbox"
    } else {
        "libx264"
    };
    video_encoder_selection(name)
}

fn video_encoder_selection(name: &str) -> anyhow::Result<VideoEncoderSelection> {
    let codec = match name {
        "h264" | "libx264" | "h264_videotoolbox" | "h264_vulkan" | "h264_nvenc" => VideoCodec::H264,
        "hevc" | "h265" | "libx265" | "hevc_videotoolbox" | "hevc_vulkan" | "hevc_nvenc" => {
            VideoCodec::Hevc
        }
        "av1" | "libaom-av1" | "av1_nvenc" => VideoCodec::Av1,
        _ => anyhow::bail!(
            "unsupported video encoder `{name}`; expected a known H.264, HEVC, or AV1 encoder"
        ),
    };
    Ok(VideoEncoderSelection {
        name: name.to_string(),
        codec,
        cuda_fast_path: matches!(name, "h264_nvenc" | "hevc_nvenc" | "av1_nvenc"),
    })
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
#[allow(clippy::too_many_arguments)]
fn render_project_mp4_cuda(
    composition: &Composition,
    media_store: &LocalMediaStore,
    width: u32,
    height: u32,
    fps: f32,
    total_frames: u32,
    codec: VideoCodec,
    on_progress: &mut dyn FnMut(RenderProgress),
) -> Result<Vec<u8>, RenderError> {
    use lumen_ffmpeg::{
        AudioEncoderConfig, CudaDriver, EncodeMode, GpuBackend, GpuVideoInput, MuxedEncoder,
        VideoEncoderConfig, import_owned_vulkan_opaque_fd_image,
    };

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
    tracing::info!(
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
    let driver = CudaDriver::load().map_err(|err| RenderError {
        code: "encode_failed",
        message: format!("failed to load CUDA driver: {err}"),
        retryable: false,
    })?;
    match driver.driver_version() {
        Ok(version) => tracing::info!(cuda_driver_version = version, "loaded CUDA driver"),
        Err(err) => tracing::warn!(error = %err, "failed to query CUDA driver version"),
    }
    let cuda_devices = driver.devices().map_err(|err| RenderError {
        code: "encode_failed",
        message: format!("failed to enumerate CUDA devices: {err}"),
        retryable: false,
    })?;
    for device in &cuda_devices {
        tracing::info!(
            cuda_ordinal = device.ordinal,
            cuda_name = %device.name,
            cuda_uuid = %format_uuid(&device.uuid),
            cuda_pci_bus_id = %device.pci_bus_id,
            matches_vulkan_device = device.uuid == vulkan_device.device_uuid,
            "found CUDA device"
        );
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
    for frame in 0..total_frames {
        prefetch_media_frames(composition, media_store, frame);
        let (raster, _) = renderer
            .render_frame_submitted(composition, frame, media_store)
            .map_err(|err| RenderError {
                code: "render_failed",
                message: format!("render failed at frame {frame}: {err}"),
                retryable: true,
            })?;
        let _ = renderer.precompile_frame_window(
            composition,
            frame.saturating_add(1),
            MEDIA_PREFETCH_LOOKAHEAD_FRAMES,
            media_store,
        );
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
        let frame_ref = cuda_frame.as_video_frame(Some(i64::from(frame)));
        encoder
            .write_gpu_frame(&GpuVideoInput::Cuda(&frame_ref))
            .map_err(|err| RenderError {
                code: "encode_failed",
                message: err.to_string(),
                retryable: true,
            })?;

        let completed = frame.saturating_add(1);
        let ratio = (completed as f32 / total_frames as f32).clamp(0.0, 1.0);
        on_progress(RenderProgress {
            stage: "rendering",
            frame: completed,
            total_frames,
            ratio,
        });
    }

    if include_audio {
        super::encoder::write_composited_audio_with(composition, media_store, |frame| {
            encoder
                .write_audio_frame(&frame)
                .map_err(|err| anyhow::anyhow!(err.to_string()))
        })
        .map_err(|err| RenderError {
            code: "audio_render_failed",
            message: err.to_string(),
            retryable: true,
        })?;
    }

    encoder.finish().map_err(|err| RenderError {
        code: "encode_failed",
        message: err.to_string(),
        retryable: true,
    })?;
    fs::read(&output_path).map_err(|err| RenderError {
        code: "encode_failed",
        message: format!("failed to read encoded output: {err}"),
        retryable: true,
    })
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
fn format_uuid(uuid: &[u8; 16]) -> String {
    uuid.iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(not(all(target_os = "linux", feature = "cuda", feature = "vulkan")))]
#[allow(clippy::too_many_arguments)]
fn render_project_mp4_cuda(
    _composition: &Composition,
    _media_store: &LocalMediaStore,
    _width: u32,
    _height: u32,
    _fps: f32,
    _total_frames: u32,
    _codec: VideoCodec,
    _on_progress: &mut dyn FnMut(RenderProgress),
) -> Result<Vec<u8>, RenderError> {
    Err(RenderError {
        code: "invalid_render_profile",
        message: "CUDA/Vulkan render path requires linux build with cuda and vulkan features"
            .to_string(),
        retryable: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvenc_encoders_select_cuda_capable_codecs() {
        let h264 = video_encoder_selection("h264_nvenc").unwrap();
        assert_eq!(h264.name, "h264_nvenc");
        assert_eq!(h264.codec, VideoCodec::H264);

        let hevc = video_encoder_selection("hevc_nvenc").unwrap();
        assert_eq!(hevc.name, "hevc_nvenc");
        assert_eq!(hevc.codec, VideoCodec::Hevc);

        let av1 = video_encoder_selection("av1_nvenc").unwrap();
        assert_eq!(av1.name, "av1_nvenc");
        assert_eq!(av1.codec, VideoCodec::Av1);
    }

    #[test]
    fn unknown_encoder_is_rejected_before_rendering() {
        let error = video_encoder_selection("definitely_not_an_encoder").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("expected a known H.264, HEVC, or AV1 encoder")
        );
    }
}
