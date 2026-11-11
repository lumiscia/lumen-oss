use std::{
    path::Path,
    time::{Duration, Instant},
};

use lumen_engine::{
    composition::Composition,
    gpu::{GpuCompositionRenderer, RasterHandle},
};
use lumen_ffmpeg::{EncoderCandidate, EncoderFamily, FfmpegError, VideoEncoderChoice};
use lumen_gpu::wgpu::{PollType, SubmissionIndex};

use super::{
    ErrorCode, ServerRenderError,
    device_budget::{DeviceReservation, reserve_devices},
    encoder::{EncoderSink, has_audio, write_composited_audio},
    gpu::{RenderTiming, create_gpu_renderer},
    media::LocalMediaStore,
    readback::ReadbackRing,
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
    /// Reads frames back through a ring of buffers and encodes them on a writer thread.
    Cpu {
        sink: EncoderSink,
        ring: ReadbackRing,
    },
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
    /// Opens the choice's encoders in order until one starts, warning for each that won't.
    pub(super) fn open(
        choice: &VideoEncoderChoice,
        composition: &Composition,
        media_store: &mut LocalMediaStore,
        output: &Path,
    ) -> Result<OpenedTarget, ServerRenderError> {
        choice
            .open(
                |candidate| {
                    Self::open_encoder(
                        candidate,
                        EncoderFamily::of(&candidate.encoder),
                        composition,
                        media_store,
                        output,
                    )
                    .map_err(WithCauses)
                },
                |warning| tracing::warn!("{warning}"),
            )
            .map(|(opened, _)| opened)
            .map_err(|WithCauses(error)| error)
    }

    /// Opens one encoder on the target its family has, and CPU upload for the rest. A hardware
    /// target that won't open moves to CPU upload with the same encoder.
    fn open_encoder(
        candidate: &EncoderCandidate,
        family: EncoderFamily,
        composition: &Composition,
        media_store: &mut LocalMediaStore,
        output: &Path,
    ) -> Result<OpenedTarget, ServerRenderError> {
        let (name, quality) = (candidate.encoder.as_str(), &candidate.quality);
        let hardware_decode = media_store.video_options.prefer_hardware_decode;
        let unavailable: Option<anyhow::Error> = match family {
            #[cfg(all(target_os = "linux", feature = "vulkan"))]
            EncoderFamily::Vulkan => {
                let devices = reserve_shared_video_device();
                match vulkan::VulkanTarget::open(composition, media_store, output, name, quality) {
                    Ok((renderer, target)) => {
                        return Ok(OpenedTarget {
                            renderer,
                            target: Self::Vulkan(target),
                            encoder: name.to_string(),
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
                match cuda::CudaTarget::open(composition, media_store, output, name, quality) {
                    Ok((renderer, target)) => {
                        return Ok(OpenedTarget {
                            renderer,
                            target: Self::Cuda(target),
                            encoder: name.to_string(),
                            path: "cuda",
                            devices,
                        });
                    }
                    Err(error) if error.code() == ErrorCode::InvalidRenderProfile => {
                        return Err(error);
                    }
                    Err(error) => {
                        // The failed renderer's device goes before its room in the budget does.
                        media_store.video_options.vulkan_device = None;
                        Some(anyhow::Error::new(error))
                    }
                }
            }
            #[cfg(all(target_os = "macos", feature = "metal"))]
            EncoderFamily::VideoToolbox => {
                let devices = reserve_devices(family, hardware_decode);
                match metal::MetalTarget::open(composition, &media_store.gpu, output, name, quality)
                {
                    Ok((renderer, target)) => {
                        return Ok(OpenedTarget {
                            renderer,
                            target: Self::Metal(target),
                            encoder: name.to_string(),
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
        let renderer = create_gpu_renderer(media_store).map_err(gpu_renderer_failed)?;
        let settings = composition.render_settings();
        let sink = EncoderSink::create(
            output,
            settings.width,
            settings.height,
            composition.timeline().fps,
            name,
            quality,
            has_audio(composition),
        )
        .map_err(|err| {
            // The next encoder tried makes its own renderer, and decoders must share its device.
            #[cfg(all(target_os = "linux", feature = "vulkan"))]
            {
                media_store.video_options.vulkan_device = None;
            }
            encoder_start_failed(err)
        })?;
        let ring = ReadbackRing::new(
            &renderer.gpu_renderer().device,
            lumen_gpu::Size::new(settings.width, settings.height),
        );
        if let Some(error) = unavailable {
            tracing::warn!(
                "{name} can't keep frames on the GPU, uploading them instead: {error:#}"
            );
        }
        Ok(OpenedTarget {
            renderer,
            target: Self::Cpu { sink, ring },
            encoder: name.to_string(),
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
    ) -> Result<(), ServerRenderError> {
        match self {
            Self::Cpu { sink, ring } => {
                let (raster, _submission) =
                    render_raster(renderer, composition, media_store, frame, timing)?;
                poll(renderer, frame, timing, None)?;
                checked_size(composition, &raster, frame)?;
                let read = timed(&mut timing.readback, || {
                    ring.push(renderer.gpu_renderer(), &raster.texture, frame)
                })
                .map_err(readback_failed)?;
                send_read(sink, read, timing)
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
    ) -> Result<(), ServerRenderError> {
        #[cfg(not(all(target_os = "macos", feature = "metal")))]
        let _ = renderer;
        match self {
            Self::Cpu { mut sink, mut ring } => {
                while let Some(read) =
                    timed(&mut timing.readback, || ring.pop(renderer.gpu_renderer()))
                        .map_err(readback_failed)?
                {
                    send_read(&mut sink, Some(read), timing)?;
                }
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
) -> Result<(), ServerRenderError> {
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

/// A frame the readback ring returned, handed to the encoder.
fn send_read(
    sink: &mut EncoderSink,
    read: Option<(u32, Vec<u8>)>,
    timing: &mut RenderTiming,
) -> Result<(), ServerRenderError> {
    let Some((frame, pixels)) = read else {
        return Ok(());
    };
    timed(&mut timing.encode_send, || sink.send_frame(frame, pixels))
        .map_err(|err| encode_failed(format!("{err:#}")))
}

fn readback_failed(err: anyhow::Error) -> ServerRenderError {
    render_failed("failed reading rendered pixels back".to_string()).caused_by(err)
}

fn render_failed(message: String) -> ServerRenderError {
    ServerRenderError::new(ErrorCode::RenderFailed, message, true)
}

fn encode_failed(message: String) -> ServerRenderError {
    ServerRenderError::new(ErrorCode::EncodeFailed, message, true)
}

/// An error that displays with its causes, so the warning for an encoder that won't open says why.
struct WithCauses(ServerRenderError);

impl std::fmt::Display for WithCauses {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)?;
        let mut cause = std::error::Error::source(&self.0);
        while let Some(error) = cause {
            write!(f, ": {error}")?;
            cause = error.source();
        }
        Ok(())
    }
}

/// An encoder that wouldn't start. Failing on the output is `output_io`, and running short of
/// memory, descriptors or a hardware encoder session is retryable. Anything else is a bad option or
/// an encoder this host can't run, which another attempt won't fix.
fn encoder_start_failed(error: anyhow::Error) -> ServerRenderError {
    let message = "lumen-ffmpeg encoder failed to start";
    match error.downcast::<FfmpegError>() {
        Ok(error) => {
            let code = if error.is_output_failure() {
                ErrorCode::OutputIo
            } else {
                ErrorCode::EncodeFailed
            };
            ServerRenderError::new(code, message, error.is_resource_exhaustion()).caused_by(error)
        }
        Err(error) => {
            ServerRenderError::new(ErrorCode::EncodeFailed, message, false).caused_by(error)
        }
    }
}

fn gpu_renderer_failed(err: anyhow::Error) -> ServerRenderError {
    render_failed("failed to create GPU renderer".to_string()).caused_by(err)
}

/// A failure mixing the audio has the engine's code, and one encoding it is `encode_failed`.
fn audio_render_failed(err: anyhow::Error) -> ServerRenderError {
    encode_failed("failed to write the audio".to_string()).caused_by(err)
}

/// Binds, uploads and submits one frame to the renderer's own output texture.
fn render_raster(
    renderer: &mut GpuCompositionRenderer,
    composition: &Composition,
    media_store: &LocalMediaStore,
    frame: u32,
    timing: &mut RenderTiming,
) -> Result<(RasterHandle, SubmissionIndex), ServerRenderError> {
    let failed = |err: lumen_engine::error::LumenError| {
        ServerRenderError::engine(format!("render failed at frame {frame}"), err)
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
) -> Result<lumen_gpu::Size, ServerRenderError> {
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

#[cfg(any(
    all(target_os = "linux", feature = "cuda", feature = "vulkan"),
    all(target_os = "macos", feature = "metal"),
))]
#[cfg(all(target_os = "macos", feature = "metal"))]
fn poll_failed(frame: u32, err: impl std::fmt::Display) -> ServerRenderError {
    render_failed(format!("GPU poll failed at frame {frame}: {err}"))
}

/// Collects finished GPU work, first waiting for the submission `until` if there is one.
fn poll(
    renderer: &GpuCompositionRenderer,
    frame: u32,
    timing: &mut RenderTiming,
    until: Option<SubmissionIndex>,
) -> Result<(), ServerRenderError> {
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

#[cfg(test)]
mod start_failure_tests {
    use lumen_ffmpeg::EncoderQuality;

    use super::*;

    fn failed(operation: &'static str, errno: i32) -> anyhow::Error {
        let mut error = FfmpegError::new(operation, "failed");
        error.code = Some(-errno);
        error.into()
    }

    #[test]
    fn failing_on_the_output_is_output_io() {
        let error = encoder_start_failed(failed("avio_open", libc::ENOENT));
        assert_eq!(error.code(), ErrorCode::OutputIo);
        assert!(!error.retryable());
    }

    #[test]
    fn running_short_of_a_resource_is_retryable() {
        for (operation, errno, code) in [
            ("avcodec_open2", libc::ENOMEM, ErrorCode::EncodeFailed),
            (
                "av_hwdevice_ctx_create",
                libc::EBUSY,
                ErrorCode::EncodeFailed,
            ),
            ("avio_open", libc::EMFILE, ErrorCode::OutputIo),
        ] {
            let error = encoder_start_failed(failed(operation, errno));
            assert_eq!(error.code(), code, "{operation}");
            assert!(error.retryable(), "{operation}");
        }
    }

    #[test]
    fn invalid_options_and_unknown_causes_are_permanent() {
        let invalid = encoder_start_failed(failed("avcodec_open2", libc::EINVAL));
        assert_eq!(invalid.code(), ErrorCode::EncodeFailed);
        assert!(!invalid.retryable());
        let other = encoder_start_failed(anyhow::anyhow!("startup thread stopped"));
        assert_eq!(other.code(), ErrorCode::EncodeFailed);
        assert!(!other.retryable());
    }

    #[test]
    fn the_ffmpeg_error_stays_the_source() {
        let error = encoder_start_failed(failed("avio_open", libc::ENOENT));
        let source = std::error::Error::source(&error).expect("source");
        assert!(source.downcast_ref::<FfmpegError>().is_some());
    }

    #[test]
    fn an_output_in_a_missing_directory_is_output_io() {
        let missing = std::env::temp_dir()
            .join("lumen-no-such-dir")
            .join("out.mp4");
        let Err(error) = EncoderSink::create(
            &missing,
            16,
            16,
            30.0,
            "libx264",
            &EncoderQuality::default(),
            false,
        ) else {
            panic!("the output can't be created");
        };
        assert_eq!(encoder_start_failed(error).code(), ErrorCode::OutputIo);
    }
}

#[cfg(all(test, target_os = "linux", feature = "cuda", feature = "vulkan"))]
mod tests {
    use lumen_ffmpeg::{
        DecodeMode, DecodedVideoFrame, EncoderQuality, InputContext, VideoDecoder,
        VideoDecoderConfig, YuvVideoFrame,
    };

    use super::*;

    /// Decodes the first video stream of `path` to the YUV planes it stores, which sidesteps any
    /// colour conversion.
    fn decode_yuv(path: &std::path::Path) -> Vec<YuvVideoFrame> {
        let mut input = InputContext::open(path.to_string_lossy().to_string()).expect("open video");
        let stream_index = input
            .best_video_stream()
            .expect("video stream")
            .stream_index;
        let mut decoder = VideoDecoder::open(
            &input,
            VideoDecoderConfig {
                stream_index,
                mode: DecodeMode::Cpu,
            },
        )
        .expect("open decoder");
        let mut frames = Vec::new();
        let mut take = |decoder: &mut VideoDecoder| {
            while let Some(frame) = decoder.receive_video_frame().expect("receive frame") {
                match frame {
                    DecodedVideoFrame::Yuv(frame) => frames.push(frame),
                    DecodedVideoFrame::Rgba { .. } => panic!("the encoder writes a YUV stream"),
                }
            }
        };
        while let Some(packet) = input.read_packet().expect("read packet") {
            decoder.send_packet(&packet).expect("send packet");
            take(&mut decoder);
        }
        decoder.send_eof().expect("send eof");
        take(&mut decoder);
        frames
    }

    /// libx264 can't take CUDA frames, so its target always fails to open, after the renderer and
    /// on NVIDIA the CUDA context are up.
    #[test]
    fn a_cuda_target_that_will_not_open_leaves_a_working_cpu_upload() {
        let composition = super::super::parse_composition(&serde_json::json!({
            "timeline": { "fps": 24, "duration_frames": 3 },
            "render_settings": { "width": 16, "height": 16 },
            "nodes": [
                { "id": 1, "type": "background", "params": { "paint": [255, 0, 0, 255], "width": 16, "height": 16 } },
                { "id": 2, "type": "media_output" }
            ],
            "connections": [{ "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }]
        }))
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("output.mp4");
        let candidate = EncoderCandidate {
            encoder: "libx264".to_string(),
            quality: EncoderQuality::default(),
        };
        let mut media_store = LocalMediaStore::new(
            directory.path().to_path_buf(),
            &crate::render::RenderConfig::default(),
        );

        let OpenedTarget {
            mut renderer,
            mut target,
            encoder,
            path,
            devices: _devices,
        } = EncodeTarget::open_encoder(
            &candidate,
            EncoderFamily::Nvenc,
            &composition,
            &mut media_store,
            &output,
        )
        .unwrap();

        assert_eq!((encoder.as_str(), path), ("libx264", "cpu_upload"));
        let mut timing = RenderTiming::default();
        for frame in 0..3 {
            target
                .write_frame(
                    &mut renderer,
                    &composition,
                    &media_store,
                    frame,
                    &mut timing,
                )
                .unwrap();
        }
        target
            .finish(&renderer, &composition, &media_store, &mut timing)
            .unwrap();
        let frames = decode_yuv(&output);
        assert_eq!(frames.len(), 3);
        // The encoder writes red (255, 0, 0) as BT.709 limited-range Y'CbCr.
        const RED: [u16; 3] = [63, 102, 240];
        for (frame, decoded) in frames.iter().enumerate() {
            let is_red = (0..decoded.height)
                .flat_map(|y| (0..decoded.width).map(move |x| (x, y)))
                .all(|(x, y)| {
                    let sample = decoded.sample(x, y);
                    sample
                        .iter()
                        .zip(RED)
                        .all(|(got, want)| got.abs_diff(want) <= 8)
                });
            assert!(is_red, "frame {frame} isn't the composition's red");
        }
    }
}
