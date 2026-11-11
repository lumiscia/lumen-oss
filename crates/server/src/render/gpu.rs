use std::{
    fs,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context;
use lumen_engine::{composition::Composition, gpu::GpuCompositionRenderer, media::MediaStore};
use lumen_ffmpeg::{QualityConfig, QualityRequest, VideoEncoderChoice};

use super::{
    ErrorCode, RenderOptions, RenderProgress, ServerRenderError,
    device_budget::DeviceReservation,
    encode_target::{EncodeTarget, OpenedTarget},
    media::{LocalMediaStore, media_root},
    resolve_encoder,
};

pub(super) const MEDIA_PREFETCH_LOOKAHEAD_FRAMES: u32 = 30;

/// Renders `composition` to MP4 bytes.
pub fn render_project_mp4(
    composition: &Composition,
    options: &RenderOptions,
    on_progress: &mut dyn FnMut(RenderProgress),
) -> Result<Vec<u8>, ServerRenderError> {
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
    let output_io = |context: &str, error: std::io::Error| {
        ServerRenderError::new(ErrorCode::OutputIo, context, true).caused_by(error)
    };
    let tmp = tempfile::tempdir().map_err(|err| output_io("failed to create temp dir", err))?;
    let output_path = tmp.path().join("output.mp4");

    let choice = match &options.encoder {
        Some(choice) => choice.clone(),
        None => resolve_encoder(
            &QualityConfig::built_in(),
            composition,
            QualityRequest::default(),
        )
        .map_err(|message| ServerRenderError::new(ErrorCode::InvalidQuality, message, false))?,
    };
    let mut render = Mp4Render::open(composition, media_store, choice, &output_path)?;
    if let Some(glyphs) = options.max_text_glyphs {
        render.set_max_text_glyphs(glyphs);
    }
    render.run(on_progress)?;

    fs::read(&output_path).map_err(|err| output_io("failed to read encoded output", err))
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
    target: EncodeTarget,
    encoder: String,
    path: &'static str,
    media_store: LocalMediaStore,
    renderer: GpuCompositionRenderer,
    // Last, so everything holding a device closes before its room is given back.
    _devices: DeviceReservation<'static>,
}

impl<'a> Mp4Render<'a> {
    /// Opens the first of `choice`'s encoders that starts, on the path that keeps frames on the
    /// GPU when it has one.
    pub fn open(
        composition: &'a Composition,
        mut media_store: LocalMediaStore,
        choice: VideoEncoderChoice,
        output: &Path,
    ) -> Result<Self, ServerRenderError> {
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
        } = EncodeTarget::open(&choice, composition, &mut media_store, output)?;
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
            target,
            encoder,
            path,
            media_store,
            renderer,
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

    /// Renders and encodes every frame, then the audio, and finishes the output.
    pub fn run(
        mut self,
        on_progress: &mut dyn FnMut(RenderProgress),
    ) -> Result<RenderReport, ServerRenderError> {
        let started = Instant::now();
        let total_frames = self.composition.timeline().duration_frames;
        let mut timing = RenderTiming::default();
        for frame in 0..total_frames {
            prefetch_media_frames(self.composition, &self.media_store, frame);
            self.target.write_frame(
                &mut self.renderer,
                self.composition,
                &self.media_store,
                frame,
                &mut timing,
            )?;
            let completed = frame.saturating_add(1);
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

fn prefetch_media_frames(composition: &Composition, media_store: &LocalMediaStore, frame: u32) {
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
        match pollster::block_on(GpuCompositionRenderer::new_vulkan_decode()) {
            Ok((renderer, device)) => {
                media_store.video_options.vulkan_device = Some(device);
                return Ok(renderer);
            }
            Err(error) => tracing::info!(%error, "renderer has no Vulkan decode device"),
        }
    }
    #[cfg(not(all(target_os = "linux", feature = "vulkan")))]
    let _ = media_store;
    pollster::block_on(GpuCompositionRenderer::new()).context("failed to initialize wgpu renderer")
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

        let error = render_project_mp4(&solid(), &options, &mut |_| {}).unwrap_err();

        assert_eq!(error.code(), ErrorCode::EncodeFailed, "{error}");
        assert!(!error.retryable(), "{error}");
    }

    #[test]
    fn an_unknown_encoder_is_an_invalid_render_profile() {
        let options = RenderOptions {
            encoder: Some(VideoEncoderChoice::single(
                "definitely_not_an_encoder",
                EncoderQuality::default(),
            )),
            ..RenderOptions::default()
        };

        let error = render_project_mp4(&solid(), &options, &mut |_| {}).unwrap_err();

        assert_eq!(error.code(), ErrorCode::InvalidRenderProfile, "{error}");
        assert!(
            error.to_string().contains("definitely_not_an_encoder"),
            "{error}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn media_resolves_in_the_root_the_options_name_with_the_engine_code_when_missing() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("hero.png"), TINY_PNG).unwrap();
        let options = RenderOptions {
            media_root: Some(root.path().to_path_buf()),
            encoder: libx264(),
            ..RenderOptions::default()
        };

        let bytes = render_project_mp4(&image("hero.png"), &options, &mut |_| {}).unwrap();
        assert!(!bytes.is_empty());

        let error = render_project_mp4(&image("missing.png"), &options, &mut |_| {}).unwrap_err();
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

        let error = render_project_mp4(&solid(), &options, &mut |_| {}).unwrap_err();

        assert_eq!(error.code(), ErrorCode::InvalidConfig, "{error}");
        assert!(error.to_string().contains("no-such-fonts"), "{error}");
    }
}
