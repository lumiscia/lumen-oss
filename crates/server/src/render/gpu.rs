use std::{
    fs,
    ops::Range,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context;
use lumen_engine::{composition::Composition, gpu::GpuCompositionRenderer, media::MediaStore};
use lumen_ffmpeg::{QualityConfig, QualityRequest, VideoEncoderChoice};

use super::{
    CancelToken, ErrorCode, OutputFormat, RenderOptions, RenderOutput, RenderProgress,
    SequenceLimits, ServerRenderError,
    archive::{ArchiveError, StoredZip},
    device_budget::DeviceReservation,
    encode_target::{EncodeTarget, OpenedTarget},
    image_render::ImageRender,
    media::{LocalMediaStore, media_root},
    output::archive_frame_name,
    resolve_encoder,
};

pub(super) const MEDIA_PREFETCH_LOOKAHEAD_FRAMES: u32 = 30;

/// What [`render_project`] produced: the bytes of a file in `format`, which for a sequence is the
/// ZIP of its frames. [`OutputFormat::content_type`] and [`OutputFormat::extension`] describe it.
#[derive(Debug, Clone)]
pub struct RenderedOutput {
    pub format: OutputFormat,
    pub bytes: Vec<u8>,
}

/// Renders `composition` as `options.output` says: an MP4, a PNG or JPEG of one frame, or a ZIP of
/// a PNG or JPEG per frame, named `frame_0000.png` and so on with the composition's frame numbers.
/// An output that can't be rendered, or a sequence past `options.sequence_limits`, ends the render
/// before any GPU work. Frames are encoded and written to the ZIP one at a time, so a sequence
/// takes memory for one frame and, at the end, for the archive that holds them.
pub fn render_project(
    composition: &Composition,
    options: &RenderOptions,
    on_progress: &mut dyn FnMut(RenderProgress),
) -> Result<RenderedOutput, ServerRenderError> {
    let format = options.output.format;
    options.output.validate(
        composition.timeline().duration_frames,
        &options.sequence_limits,
    )?;
    if options.cancel.is_cancelled() {
        return Err(ServerRenderError::cancelled());
    }
    let media_root = media_root(options.media_root.as_deref())?;
    if let Some(dir) = options.config.font_dir()
        && !dir.is_dir()
    {
        return Err(ServerRenderError::new(
            ErrorCode::InvalidConfig,
            format!("font directory `{}` is not a directory", dir.display()),
            false,
        ));
    }
    let mut media_store = LocalMediaStore::new(media_root, &options.config);
    if let Some(assets) = &options.assets {
        media_store = media_store.with_assets(Arc::clone(assets));
    }
    let tmp = match &options.config.temp_dir {
        Some(dir) => tempfile::tempdir_in(dir),
        None => tempfile::tempdir(),
    }
    .map_err(|err| output_io("failed to create temp dir", err))?;
    let output_path = tmp.path().join(format!("output.{}", format.extension()));

    if format.is_video() {
        let choice = match &options.encoder {
            Some(choice) => choice.clone(),
            None => resolve_encoder(
                &QualityConfig::built_in(),
                composition,
                QualityRequest::default(),
            )
            .map_err(|message| ServerRenderError::new(ErrorCode::InvalidQuality, message, false))?,
        };
        let mut render = Mp4Render::open(
            composition,
            media_store,
            choice,
            &output_path,
            &options.output,
            options.cancel.clone(),
        )?;
        if let Some(glyphs) = options.max_text_glyphs {
            render.set_max_text_glyphs(glyphs);
        }
        render.run(on_progress)?;
    } else {
        let mut render = ImageRender::open(
            composition,
            media_store,
            &options.output,
            options.cancel.clone(),
        )?;
        if let Some(glyphs) = options.max_text_glyphs {
            render.set_max_text_glyphs(glyphs);
        }
        if format.is_still() {
            let mut still = Vec::new();
            render.run(
                &mut |image| {
                    still = image.bytes;
                    Ok(())
                },
                on_progress,
            )?;
            return Ok(RenderedOutput {
                format,
                bytes: still,
            });
        }
        write_archive(
            render,
            &output_path,
            format,
            &options.sequence_limits,
            on_progress,
        )?;
    }

    options.cancel.check()?;
    let bytes =
        fs::read(&output_path).map_err(|err| output_io("failed to read encoded output", err))?;
    Ok(RenderedOutput { format, bytes })
}

fn output_io(context: &str, error: std::io::Error) -> ServerRenderError {
    ServerRenderError::new(ErrorCode::OutputIo, context, true).caused_by(error)
}

/// Runs `render` and stores each of its frames in the ZIP at `path`.
fn write_archive(
    render: ImageRender<'_>,
    path: &Path,
    format: OutputFormat,
    limits: &SequenceLimits,
    on_progress: &mut dyn FnMut(RenderProgress),
) -> Result<(), ServerRenderError> {
    let extension = format.image_extension().expect("a sequence is images");
    let last_frame = render.frames().end - 1;
    let file =
        fs::File::create(path).map_err(|err| output_io("failed to create the archive", err))?;
    let mut zip = StoredZip::new(std::io::BufWriter::new(file));
    let mut total_bytes = 0_u64;
    let archive_failed = |error: ArchiveError| match error {
        ArchiveError::Full => ServerRenderError::new(
            ErrorCode::OutputTooLarge,
            "the sequence is too large for one archive",
            false,
        ),
        ArchiveError::Io(err) => output_io("failed to write the archive", err),
    };
    render.run(
        &mut |image| {
            total_bytes += image.bytes.len() as u64;
            if total_bytes > limits.max_bytes {
                return Err(ServerRenderError::new(
                    ErrorCode::OutputTooLarge,
                    format!(
                        "the sequence is past {} bytes of images at frame {}",
                        limits.max_bytes, image.frame
                    ),
                    false,
                )
                .into());
            }
            let name = archive_frame_name(image.frame, last_frame, extension);
            Ok(zip.add(&name, &image.bytes).map_err(archive_failed)?)
        },
        on_progress,
    )?;
    zip.finish()
        .map_err(archive_failed)?
        .into_inner()
        .map_err(|err| output_io("failed to write the archive", err.into_error()))?;
    Ok(())
}

/// What a finished render did and where its time went.
#[derive(Debug, Clone, Default)]
pub struct RenderReport {
    pub frames: u32,
    /// Warnings rendering raised, such as fonts that fell back.
    pub diagnostics: Vec<lumen_engine::diagnostic::Diagnostic>,
    pub held_bytes: u64,
    pub submitted_passes: u64,
    pub skipped_passes: u64,
    pub timing: RenderTiming,
}

/// Time spent per step, summed over the render.
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderTiming {
    pub bind: Duration,
    pub upload: Duration,
    pub render: Duration,
    pub flush: Duration,
    pub readback: Duration,
    pub encode_send: Duration,
    pub audio_mix: Duration,
    pub encode_finish: Duration,
    pub total: Duration,
}

/// A render with its encoder open and its composition compiled, ready to write frames to `output`.
pub struct Mp4Render<'a> {
    composition: &'a Composition,
    frames: Range<u32>,
    target: EncodeTarget,
    encoder: String,
    path: &'static str,
    media_store: LocalMediaStore,
    renderer: GpuCompositionRenderer,
    cancel: CancelToken,
    // Last, so everything holding a device closes before its room is given back.
    _devices: DeviceReservation<'static>,
}

impl<'a> Mp4Render<'a> {
    /// Checks that `render` is an MP4 of a range that fits the timeline, then opens the first of
    /// `choice`'s encoders that starts, on the path that keeps frames on the GPU when it has one,
    /// writing to `output`. Cancelling `cancel` ends the wait for GPU device room, and stops
    /// [`run`](Self::run) with [`ErrorCode::Cancelled`].
    pub fn open(
        composition: &'a Composition,
        mut media_store: LocalMediaStore,
        choice: VideoEncoderChoice,
        output: &Path,
        render: &RenderOutput,
        cancel: CancelToken,
    ) -> Result<Self, ServerRenderError> {
        if !render.format.is_video() {
            return Err(ServerRenderError::new(
                ErrorCode::InvalidOutput,
                "a video render needs the mp4 format",
                false,
            ));
        }
        let frames = render.resolve(composition.timeline().duration_frames)?;
        let (choice, info) = choice
            .look_up(|warning| tracing::warn!("{warning}"))
            .map_err(invalid_render_profile)?;
        tracing::info!(encoder = choice.first().encoder, codec = ?info.codec, "selected video encoder");

        let OpenedTarget {
            renderer,
            target,
            encoder,
            path,
            devices,
        } = EncodeTarget::open(&choice, composition, &mut media_store, output, &cancel)?;
        tracing::info!(encoder, path, "opened video encoder");
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
        let mut render = Self {
            composition,
            frames,
            target,
            encoder,
            path,
            media_store,
            renderer,
            cancel,
            _devices: devices,
        };
        render
            .renderer
            .compile_with_output_format(composition, render.target.output_format())
            .map_err(|err| ServerRenderError::engine("failed to compile composition", err))?;
        Ok(render)
    }

    /// Sets the most glyph, stroke and decoration instances one text node may draw, as
    /// [`GpuCompositionRenderer::set_max_text_glyphs`] does.
    pub fn set_max_text_glyphs(&mut self, glyphs: usize) {
        self.renderer.set_max_text_glyphs(glyphs);
    }

    /// The encoder that opened, which is a later candidate when an earlier one wouldn't.
    pub fn encoder(&self) -> &str {
        &self.encoder
    }

    /// How frames reach the encoder: `cpu_upload`, `zero_copy` or `cuda`.
    pub fn path(&self) -> &'static str {
        self.path
    }

    /// Renders and encodes the frames of the range, then the audio of the same span, and finishes
    /// the output, which starts at time 0. Cancelling stops it at the next frame boundary, audio
    /// chunk or drained frame, and before the encoder finishes; once the output is complete it is
    /// too late.
    pub fn run(
        mut self,
        on_progress: &mut dyn FnMut(RenderProgress),
    ) -> Result<RenderReport, ServerRenderError> {
        let started = Instant::now();
        let total_frames = self.frames.end - self.frames.start;
        let mut timing = RenderTiming::default();
        for frame in self.frames.clone() {
            self.cancel.check()?;
            prefetch_media_frames(self.composition, &self.media_store, frame);
            let pts = frame - self.frames.start;
            self.target.write_frame(
                &mut self.renderer,
                self.composition,
                &self.media_store,
                frame,
                pts,
                &mut timing,
            )?;
            let completed = pts + 1;
            let ratio = (completed as f32 / total_frames as f32).clamp(0.0, 1.0);
            on_progress(RenderProgress {
                stage: "rendering",
                frame: completed,
                total_frames,
                ratio,
            });
        }
        self.target.finish(
            &self.renderer,
            self.composition,
            &self.media_store,
            &self.frames,
            &self.cancel,
            &mut timing,
        )?;
        timing.total = started.elapsed();
        Ok(RenderReport {
            frames: total_frames,
            diagnostics: self.renderer.take_diagnostics(),
            held_bytes: self.renderer.held_bytes(),
            submitted_passes: self.renderer.submitted_pass_count(),
            skipped_passes: self.renderer.skipped_pass_count(),
            timing,
        })
    }
}

fn invalid_render_profile(err: lumen_ffmpeg::FfmpegError) -> ServerRenderError {
    ServerRenderError::new(ErrorCode::InvalidRenderProfile, err.to_string(), false)
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
pub fn create_gpu_renderer(
    media_store: &mut LocalMediaStore,
) -> anyhow::Result<GpuCompositionRenderer> {
    #[cfg(all(target_os = "linux", feature = "vulkan"))]
    if media_store.video_options.prefer_hardware_decode {
        match pollster::block_on(GpuCompositionRenderer::new_vulkan_decode_with_settings(
            &media_store.gpu,
        )) {
            Ok((renderer, device)) => {
                media_store.video_options.vulkan_device = Some(device);
                return Ok(renderer);
            }
            Err(error) => tracing::info!(%error, "renderer has no Vulkan decode device"),
        }
    }
    #[cfg(not(all(target_os = "linux", feature = "vulkan")))]
    let _ = media_store;
    pollster::block_on(GpuCompositionRenderer::new_with_settings(&media_store.gpu))
        .context("failed to initialize wgpu renderer")
}

#[cfg(test)]
mod tests {
    use lumen_ffmpeg::EncoderQuality;

    use super::*;
    use crate::render::RenderConfig;

    fn solid() -> Composition {
        super::super::parse_composition(&serde_json::json!({
            "timeline": { "fps": 24, "duration_frames": 1 },
            "render_settings": { "width": 16, "height": 16 },
            "nodes": [
                { "id": 1, "type": "background", "params": { "paint": [255, 0, 0, 255], "width": 16, "height": 16 } },
                { "id": 2, "type": "media_output" }
            ],
            "connections": [{ "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }]
        }))
        .unwrap()
    }

    fn image(source: &str) -> Composition {
        super::super::parse_composition(&serde_json::json!({
            "timeline": { "fps": 24, "duration_frames": 1 },
            "render_settings": { "width": 16, "height": 16 },
            "nodes": [
                { "id": 1, "type": "media_in", "params": { "kind": "image", "source": source } },
                { "id": 2, "type": "media_output" }
            ],
            "connections": [{ "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }]
        }))
        .unwrap()
    }

    /// One transparent pixel.
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0xef, 0xa0, 0xf0, 0x1f, 0x00, 0x06, 0x00, 0x02, 0x5f, 0x34, 0xcd, 0x40, 0xc0, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    fn libx264() -> Option<VideoEncoderChoice> {
        Some(VideoEncoderChoice::single(
            "libx264",
            EncoderQuality::default(),
        ))
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn encoder_that_cannot_open_is_not_retryable() {
        let _gpu = crate::render::gpu_test_guard();
        let options = RenderOptions {
            // libx264 only checks `tune` when it opens.
            encoder: Some(VideoEncoderChoice::single(
                "libx264",
                EncoderQuality {
                    options: vec![("tune".to_string(), "not-a-tune".to_string())],
                    ..EncoderQuality::default()
                },
            )),
            ..RenderOptions::default()
        };

        let error = render_project(&solid(), &options, &mut |_| {}).unwrap_err();

        assert_eq!(error.code(), ErrorCode::EncodeFailed, "{error}");
        assert!(!error.retryable(), "{error}");
    }

    fn frames(count: u32) -> Composition {
        super::super::parse_composition(&serde_json::json!({
            "timeline": { "fps": 24, "duration_frames": count },
            "render_settings": { "width": 16, "height": 16 },
            "nodes": [
                { "id": 1, "type": "background", "params": { "paint": [255, 0, 0, 255], "width": 16, "height": 16 } },
                { "id": 2, "type": "media_output" }
            ],
            "connections": [{ "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }]
        }))
        .unwrap()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cancelling_stops_at_the_next_frame_and_leaves_no_output() {
        let _gpu = crate::render::gpu_test_guard();
        let temp = tempfile::tempdir().unwrap();
        let cancel = CancelToken::new();
        let options = RenderOptions {
            config: RenderConfig {
                temp_dir: Some(temp.path().to_path_buf()),
                ..RenderConfig::default()
            },
            encoder: libx264(),
            cancel: cancel.clone(),
            ..RenderOptions::default()
        };

        let mut rendered = 0;
        let error = render_project(&frames(50), &options, &mut |progress| {
            rendered = progress.frame;
            if progress.frame == 3 {
                cancel.cancel();
            }
        })
        .unwrap_err();

        assert_eq!(error.code(), ErrorCode::Cancelled, "{error}");
        assert!(!error.retryable());
        assert_eq!(rendered, 3);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);

        let error = render_project(&frames(50), &options, &mut |_| {
            panic!("a cancelled token renders nothing")
        })
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::Cancelled, "{error}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cancelling_after_the_last_frame_still_stops_the_render() {
        let _gpu = crate::render::gpu_test_guard();
        let temp = tempfile::tempdir().unwrap();
        let cancel = CancelToken::new();
        let options = RenderOptions {
            config: RenderConfig {
                temp_dir: Some(temp.path().to_path_buf()),
                ..RenderConfig::default()
            },
            encoder: libx264(),
            cancel: cancel.clone(),
            ..RenderOptions::default()
        };

        let error = render_project(&frames(4), &options, &mut |progress| {
            if progress.frame == progress.total_frames {
                cancel.cancel();
            }
        })
        .unwrap_err();

        assert_eq!(error.code(), ErrorCode::Cancelled, "{error}");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[test]
    fn an_unknown_encoder_is_an_invalid_render_profile() {
        let _gpu = crate::render::gpu_test_guard();
        let options = RenderOptions {
            encoder: Some(VideoEncoderChoice::single(
                "definitely_not_an_encoder",
                EncoderQuality::default(),
            )),
            ..RenderOptions::default()
        };

        let error = render_project(&solid(), &options, &mut |_| {}).unwrap_err();

        assert_eq!(error.code(), ErrorCode::InvalidRenderProfile, "{error}");
        assert!(
            error.to_string().contains("definitely_not_an_encoder"),
            "{error}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn media_resolves_in_the_root_the_options_name_with_the_engine_code_when_missing() {
        let _gpu = crate::render::gpu_test_guard();
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("hero.png"), TINY_PNG).unwrap();
        let options = RenderOptions {
            media_root: Some(root.path().to_path_buf()),
            encoder: libx264(),
            ..RenderOptions::default()
        };

        let output = render_project(&image("hero.png"), &options, &mut |_| {}).unwrap();
        assert!(!output.bytes.is_empty());

        let error = render_project(&image("missing.png"), &options, &mut |_| {}).unwrap_err();
        assert_eq!(
            error.code(),
            ErrorCode::Engine("media.not_found"),
            "{error}"
        );
        assert!(!error.retryable(), "{error}");
        assert!(error.engine_error().is_some());
    }

    #[test]
    fn a_font_directory_that_does_not_exist_is_invalid_config() {
        let options = RenderOptions {
            config: RenderConfig {
                font_dir: Some("no-such-fonts".into()),
                ..RenderConfig::default()
            },
            encoder: libx264(),
            ..RenderOptions::default()
        };

        let error = render_project(&solid(), &options, &mut |_| {}).unwrap_err();

        assert_eq!(error.code(), ErrorCode::InvalidConfig, "{error}");
        assert!(error.to_string().contains("no-such-fonts"), "{error}");
    }
}
