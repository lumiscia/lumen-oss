use std::{fs, sync::mpsc};

use anyhow::Context;
use lumen_engine::{composition::Composition, gpu::GpuCompositionRenderer, media::MediaStore};
use lumen_ffmpeg::{VideoCodec, VideoEncoderChoice};

use super::{
    ProjectBundle, RenderError, RenderOptions, RenderProgress,
    encoder::{
        ENCODER_FRAME_QUEUE_CAPACITY, EncoderFrame, LumenFfmpegEncoder, has_audio,
        write_composited_audio,
    },
    media::{LocalMediaStore, media_root},
};

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
mod linux_cuda;
#[cfg(not(all(target_os = "linux", feature = "cuda", feature = "vulkan")))]
mod unsupported_cuda;

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
use linux_cuda::render_project_mp4_cuda;
#[cfg(not(all(target_os = "linux", feature = "cuda", feature = "vulkan")))]
use unsupported_cuda::render_project_mp4_cuda;

pub(super) const MEDIA_PREFETCH_LOOKAHEAD_FRAMES: u32 = 30;

pub fn render_project_mp4(
    bundle: &ProjectBundle,
    options: &RenderOptions,
    on_progress: &mut dyn FnMut(RenderProgress),
) -> Result<Vec<u8>, RenderError> {
    let composition = &bundle.composition;
    let width = composition.render_settings().width;
    let height = composition.render_settings().height;
    let fps = composition.timeline().fps;
    let total_frames = composition.timeline().duration_frames;

    let media_root = media_root(options.media_root.as_deref()).map_err(|err| RenderError {
        code: "media_root_error",
        message: err.to_string(),
        retryable: false,
    })?;
    let mut media_store = LocalMediaStore::new(media_root);
    let mut renderer = create_gpu_renderer(&mut media_store).map_err(|err| RenderError {
        code: "render_failed",
        message: format!("failed to create GPU renderer: {err:#}"),
        retryable: true,
    })?;
    if let Some(adapter) = renderer.gpu_renderer().adapter_info() {
        tracing::info!(
            adapter = %adapter.name,
            backend = ?adapter.backend,
            device_type = ?adapter.device_type,
            vendor = adapter.vendor,
            device = adapter.device,
            driver = %adapter.driver,
            driver_info = %adapter.driver_info,
            "rendering with GPU adapter"
        );
    }
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

    let choice = VideoEncoderChoice::resolve(options.video_encoder.as_deref());
    let encoder = video_encoder_selection(&choice.name).map_err(|err| RenderError {
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
            renderer,
            composition,
            &media_store,
            width,
            height,
            fps,
            total_frames,
            &encoder.name,
            encoder.codec,
            options.verbose_debug,
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
    let (encoder, encoder_name) = choice
        .open(
            |name| {
                LumenFfmpegEncoder::create(
                    &output_path,
                    width,
                    height,
                    fps,
                    name,
                    encoder.codec,
                    include_audio,
                )
            },
            |warning| tracing::warn!("{warning}"),
        )
        .map_err(|err| RenderError {
            code: "encode_failed",
            message: err.to_string(),
            retryable: false,
        })?;
    tracing::info!(
        encoder = encoder_name,
        path = "cpu_upload",
        "opened video encoder"
    );
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
        // Hold this frame's output so a later plan change cannot free it before readback.
        let output = renderer
            .gpu_renderer()
            .texture_arc(raster.texture)
            .ok_or_else(|| RenderError {
                code: "render_failed",
                message: format!(
                    "render output texture {:?} is unavailable for frame {frame}",
                    raster.texture
                ),
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
        let pixels = renderer
            .gpu_renderer()
            .read_texture_rgba8(&output, size)
            .map_err(|err| RenderError {
                code: "render_failed",
                message: format!("failed reading rendered pixels for frame {frame}: {err}"),
                retryable: true,
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

pub(super) fn prefetch_media_frames(
    composition: &Composition,
    media_store: &LocalMediaStore,
    frame: u32,
) {
    let Ok(requirements) =
        lumen_engine::media::collect_frame_requirements(composition, media_store, frame)
    else {
        return;
    };
    for video in requirements.videos {
        let Ok(resolver) = media_store.get_video_resolver(&video.stream_id) else {
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

/// Creates a renderer that hardware decode can share a device with, when decode is asked for and
/// the hardware has it.
fn create_gpu_renderer(
    media_store: &mut LocalMediaStore,
) -> anyhow::Result<GpuCompositionRenderer> {
    let handle = tokio::runtime::Handle::try_current()
        .context("GPU renderer creation requires a Tokio runtime")?;
    #[cfg(all(target_os = "linux", feature = "vulkan"))]
    if media_store.video_options.prefer_hardware_decode {
        match handle.block_on(GpuCompositionRenderer::new_vulkan_decode()) {
            Ok((renderer, device)) => {
                media_store.video_options.vulkan_device = Some(device);
                return Ok(renderer);
            }
            Err(error) => tracing::info!(%error, "renderer has no Vulkan decode device"),
        }
    }
    #[cfg(not(all(target_os = "linux", feature = "vulkan")))]
    let _ = media_store;
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

fn video_encoder_selection(name: &str) -> anyhow::Result<VideoEncoderSelection> {
    let codec = match name {
        "h264" | "libx264" | "h264_videotoolbox" | "h264_vulkan" | "h264_nvenc" => VideoCodec::H264,
        "hevc" | "h265" | "libx265" | "hevc_videotoolbox" | "hevc_vulkan" | "hevc_nvenc" => {
            VideoCodec::Hevc
        }
        "av1" | "libaom-av1" | "av1_nvenc" | "av1_vulkan" => VideoCodec::Av1,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn encoder_that_cannot_open_is_not_retryable() {
        let bundle = super::super::convert_project_payload(&serde_json::json!({
            "timeline": { "fps": 24, "duration_frames": 1 },
            "render_settings": { "width": 16, "height": 16 },
            "nodes": [
                { "id": 1, "type": "background", "params": { "paint": [255, 0, 0, 255], "width": 16, "height": 16 } },
                { "id": 2, "type": "media_output" }
            ],
            "connections": [{ "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }]
        }))
        .unwrap();
        let options = RenderOptions {
            video_encoder: Some("h264_videotoolbox".to_string()),
            ..RenderOptions::default()
        };

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _runtime = runtime.enter();
        let error = render_project_mp4(&bundle, &options, &mut |_| {}).unwrap_err();

        assert_eq!(error.code, "encode_failed", "{error}");
        assert!(!error.retryable, "{error}");
    }

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
    fn vulkan_av1_encoder_is_selectable() {
        let av1 = video_encoder_selection("av1_vulkan").unwrap();
        assert_eq!(av1.codec, VideoCodec::Av1);
        assert!(!av1.cuda_fast_path);
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
