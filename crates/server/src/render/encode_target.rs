use std::{
    path::Path,
    time::{Duration, Instant},
};

use lumen_engine::{
    composition::Composition,
    gpu::{GpuCompositionRenderer, RasterHandle},
};
use lumen_ffmpeg::{EncoderFamily, EncoderQuality, VideoEncoderChoice};
use lumen_gpu::wgpu::{PollType, SubmissionIndex};

use super::{
    RenderError,
    device_budget::{DeviceReservation, reserve_devices},
    encoder::{EncoderSink, has_audio, write_composited_audio},
    gpu::{MEDIA_PREFETCH_LOOKAHEAD_FRAMES, RenderTiming, create_gpu_renderer},
    media::LocalMediaStore,
};

#[cfg(all(target_os = "linux", feature = "vulkan"))]
use super::device_budget::reserve_shared_video_device;

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
mod cuda;
#[cfg(all(target_os = "macos", feature = "metal"))]
mod metal;
#[cfg(all(target_os = "linux", feature = "vulkan"))]
mod vulkan;

/// How one render's frames get from the renderer into its encoder. Each target writes one finished
/// frame at a time and owns what has to happen between submitting it and encoding it.
#[allow(clippy::large_enum_variant)]
pub(super) enum EncodeTarget {
    /// Reads frames back and encodes them on a writer thread.
    Cpu(EncoderSink),
    /// Converts frames to NV12 on the render device, which the encoder shares.
    #[cfg(all(target_os = "linux", feature = "vulkan"))]
    Vulkan(vulkan::VulkanTarget),
    /// Renders into memory CUDA imports, and NVENC converts the RGBA frames itself.
    #[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
    Cuda(cuda::CudaTarget),
    /// Renders into pixel buffers VideoToolbox reads.
    #[cfg(all(target_os = "macos", feature = "metal"))]
    Metal(metal::MetalTarget),
}

pub(super) struct OpenedTarget {
    pub renderer: GpuCompositionRenderer,
    pub target: EncodeTarget,
    pub encoder: String,
    pub path: &'static str,
    pub devices: DeviceReservation<'static>,
}

impl EncodeTarget {
    /// Opens the encoder on the target its family has, and CPU upload for the rest. A hardware
    /// target that won't open moves to CPU upload with the same encoder, where the automatic
    /// default can still fall back.
    pub(super) fn open(
        choice: &VideoEncoderChoice,
        family: EncoderFamily,
        composition: &Composition,
        media_store: &mut LocalMediaStore,
        output: &Path,
        quality: &EncoderQuality,
    ) -> Result<OpenedTarget, RenderError> {
        let hardware_decode = media_store.video_options.prefer_hardware_decode;
        let unavailable: Option<anyhow::Error> = match family {
            #[cfg(all(target_os = "linux", feature = "vulkan"))]
            EncoderFamily::Vulkan => {
                let devices = reserve_shared_video_device();
                match vulkan::VulkanTarget::open(
                    composition,
                    media_store,
                    output,
                    &choice.name,
                    quality,
                ) {
                    Ok((renderer, target)) => {
                        return Ok(OpenedTarget {
                            renderer,
                            target: Self::Vulkan(target),
                            encoder: choice.name.clone(),
                            path: "zero_copy",
                            devices,
                        });
                    }
                    Err(error) => Some(error),
                }
            }
            #[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
            EncoderFamily::Nvenc => {
                let devices = reserve_devices(family, hardware_decode);
                match cuda::CudaTarget::open(
                    composition,
                    media_store,
                    output,
                    &choice.name,
                    quality,
                ) {
                    Ok((renderer, target)) => {
                        return Ok(OpenedTarget {
                            renderer,
                            target: Self::Cuda(target),
                            encoder: choice.name.clone(),
                            path: "cuda",
                            devices,
                        });
                    }
                    Err(error) if error.code == "invalid_fps" => return Err(error),
                    Err(error) => {
                        // The failed renderer's device goes before its room in the budget does.
                        media_store.video_options.vulkan_device = None;
                        Some(anyhow::anyhow!(error.message))
                    }
                }
            }
            #[cfg(all(target_os = "macos", feature = "metal"))]
            EncoderFamily::VideoToolbox => {
                let devices = reserve_devices(family, hardware_decode);
                match metal::MetalTarget::open(composition, output, &choice.name, quality) {
                    Ok((renderer, target)) => {
                        return Ok(OpenedTarget {
                            renderer,
                            target: Self::Metal(target),
                            encoder: choice.name.clone(),
                            path: "zero_copy",
                            devices,
                        });
                    }
                    Err(error) => Some(error),
                }
            }
            _ => None,
        };

        let devices = reserve_devices(family, hardware_decode);
        let renderer = create_gpu_renderer(media_store).map_err(|err| RenderError {
            code: "render_failed",
            message: format!("failed to create GPU renderer: {err:#}"),
            retryable: true,
        })?;
        let settings = composition.render_settings();
        let (sink, encoder) = choice
            .open(
                |name| {
                    EncoderSink::create(
                        output,
                        settings.width,
                        settings.height,
                        composition.timeline().fps,
                        name,
                        quality,
                        has_audio(composition),
                    )
                },
                |warning| tracing::warn!("{warning}"),
            )
            .map_err(|err| RenderError {
                code: "encode_failed",
                message: err.to_string(),
                retryable: false,
            })?;
        if let Some(error) = unavailable.filter(|_| encoder == choice.name) {
            tracing::warn!(
                "{encoder} can't keep frames on the GPU, uploading them instead: {error:#}"
            );
        }
        Ok(OpenedTarget {
            renderer,
            target: Self::Cpu(sink),
            encoder,
            path: "cpu_upload",
            devices,
        })
    }

    pub(super) fn output_format(&self) -> lumen_gpu::wgpu::TextureFormat {
        match self {
            #[cfg(all(target_os = "macos", feature = "metal"))]
            Self::Metal(_) => lumen_gpu::wgpu::TextureFormat::Bgra8Unorm,
            _ => lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
        }
    }

    /// Renders frame `frame` and hands it to the encoder.
    pub(super) fn write_frame(
        &mut self,
        renderer: &mut GpuCompositionRenderer,
        composition: &Composition,
        media_store: &LocalMediaStore,
        frame: u32,
        timing: &mut RenderTiming,
    ) -> Result<(), RenderError> {
        match self {
            Self::Cpu(sink) => {
                let (raster, _submission) =
                    render_raster(renderer, composition, media_store, frame, timing)?;
                // Hold this frame's output so a later plan change cannot free it before readback.
                let output = renderer
                    .gpu_renderer()
                    .texture_arc(raster.texture)
                    .ok_or_else(|| missing_output(&raster, frame))?;
                precompile_ahead(renderer, composition, media_store, frame);
                poll(renderer, frame, timing, None)?;
                let size = checked_size(composition, &raster, frame)?;
                let pixels = timed(&mut timing.readback, || {
                    renderer.gpu_renderer().read_texture_rgba8(&output, size)
                })
                .map_err(|err| {
                    render_failed(format!(
                        "failed reading rendered pixels for frame {frame}: {err}"
                    ))
                })?;
                timed(&mut timing.encode_send, || sink.send_frame(frame, pixels))
                    .map_err(|err| encode_failed(format!("{err:#}")))
            }
            #[cfg(all(target_os = "linux", feature = "vulkan"))]
            Self::Vulkan(target) => {
                target.write_frame(renderer, composition, media_store, frame, timing)
            }
            #[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
            Self::Cuda(target) => {
                target.write_frame(renderer, composition, media_store, frame, timing)
            }
            #[cfg(all(target_os = "macos", feature = "metal"))]
            Self::Metal(target) => {
                target.write_frame(renderer, composition, media_store, frame, timing)
            }
        }
    }

    /// Writes the audio after the last frame and finishes the output.
    pub(super) fn finish(
        self,
        renderer: &GpuCompositionRenderer,
        composition: &Composition,
        media_store: &LocalMediaStore,
        timing: &mut RenderTiming,
    ) -> Result<(), RenderError> {
        #[cfg(not(all(target_os = "macos", feature = "metal")))]
        let _ = renderer;
        match self {
            Self::Cpu(mut sink) => {
                timed(&mut timing.audio_mix, || {
                    write_composited_audio(composition, media_store, |frame| sink.send_audio(frame))
                })
                .map_err(audio_render_failed)?;
                timed(&mut timing.encode_finish, || sink.finish())
                    .map_err(|err| encode_failed(format!("{err:#}")))
            }
            #[cfg(all(target_os = "linux", feature = "vulkan"))]
            Self::Vulkan(target) => {
                finish_muxed(target.into_encoder(), composition, media_store, timing)
            }
            #[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
            Self::Cuda(target) => {
                finish_muxed(target.into_encoder(), composition, media_store, timing)
            }
            #[cfg(all(target_os = "macos", feature = "metal"))]
            Self::Metal(target) => {
                let encoder = target.drain(renderer, timing)?;
                finish_muxed(encoder, composition, media_store, timing)
            }
        }
    }
}

/// Writes the audio to an encoder on this thread and finishes it.
#[cfg(any(
    all(target_os = "linux", feature = "vulkan"),
    all(target_os = "macos", feature = "metal"),
))]
fn finish_muxed(
    mut encoder: lumen_ffmpeg::MuxedEncoder,
    composition: &Composition,
    media_store: &LocalMediaStore,
    timing: &mut RenderTiming,
) -> Result<(), RenderError> {
    timed(&mut timing.audio_mix, || {
        write_composited_audio(composition, media_store, |frame| {
            encoder
                .write_audio_frame(&frame)
                .map_err(|err| anyhow::anyhow!("lumen-ffmpeg audio encode failed: {err}"))
        })
    })
    .map_err(audio_render_failed)?;
    timed(&mut timing.encode_finish, || encoder.finish())
        .map_err(|err| encode_failed(err.to_string()))
}

fn timed<T>(total: &mut Duration, work: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let result = work();
    *total += started.elapsed();
    result
}

fn render_failed(message: String) -> RenderError {
    RenderError {
        code: "render_failed",
        message,
        retryable: true,
    }
}

fn encode_failed(message: String) -> RenderError {
    RenderError {
        code: "encode_failed",
        message,
        retryable: true,
    }
}

fn audio_render_failed(err: anyhow::Error) -> RenderError {
    RenderError {
        code: "audio_render_failed",
        message: format!("{err:#}"),
        retryable: true,
    }
}

fn missing_output(raster: &RasterHandle, frame: u32) -> RenderError {
    render_failed(format!(
        "render output texture {:?} is unavailable for frame {frame}",
        raster.texture
    ))
}

/// Binds, uploads and submits one frame to the renderer's own output texture.
fn render_raster(
    renderer: &mut GpuCompositionRenderer,
    composition: &Composition,
    media_store: &LocalMediaStore,
    frame: u32,
    timing: &mut RenderTiming,
) -> Result<(RasterHandle, SubmissionIndex), RenderError> {
    let failed = |err: lumen_engine::error::LumenError| {
        render_failed(format!("render failed at frame {frame}: {err}"))
    };
    let bound = timed(&mut timing.bind, || {
        renderer.bind_frame(composition, frame, media_store)
    })
    .map_err(failed)?;
    timed(&mut timing.upload, || renderer.upload_bound_frame(&bound)).map_err(failed)?;
    timed(&mut timing.render, || renderer.submit_render()).map_err(failed)
}

fn checked_size(
    composition: &Composition,
    raster: &RasterHandle,
    frame: u32,
) -> Result<lumen_gpu::Size, RenderError> {
    let size = raster.domain.storage_size;
    let settings = composition.render_settings();
    if (size.width, size.height) != (settings.width, settings.height) {
        return Err(render_failed(format!(
            "frame {frame} dimensions {}x{} do not match composition {}x{}",
            size.width, size.height, settings.width, settings.height
        )));
    }
    Ok(size)
}

fn precompile_ahead(
    renderer: &mut GpuCompositionRenderer,
    composition: &Composition,
    media_store: &LocalMediaStore,
    frame: u32,
) {
    let _ = renderer.precompile_frame_window(
        composition,
        frame.saturating_add(1),
        MEDIA_PREFETCH_LOOKAHEAD_FRAMES,
        media_store,
    );
}

#[cfg(any(
    all(target_os = "linux", feature = "cuda", feature = "vulkan"),
    all(target_os = "macos", feature = "metal"),
))]
fn poll_failed(frame: u32, err: impl std::fmt::Display) -> RenderError {
    render_failed(format!("GPU poll failed at frame {frame}: {err}"))
}

/// Collects finished GPU work, first waiting for the submission `until` if there is one.
fn poll(
    renderer: &GpuCompositionRenderer,
    frame: u32,
    timing: &mut RenderTiming,
    until: Option<SubmissionIndex>,
) -> Result<(), RenderError> {
    let poll_type = match until {
        Some(submission) => PollType::Wait {
            submission_index: Some(submission),
            timeout: None,
        },
        None => PollType::Poll,
    };
    timed(&mut timing.flush, || {
        renderer.gpu_renderer().device.poll(poll_type)
    })
    .map(|_| ())
    .map_err(|err| render_failed(format!("GPU poll failed at frame {frame}: {err}")))
}
