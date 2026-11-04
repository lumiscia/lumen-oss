use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

use anyhow::Context;
use lumen_engine::{composition::Composition, gpu::GpuCompositionRenderer, media::MediaStore};
use lumen_ffmpeg::{EncoderQuality, VideoEncoderChoice};

use super::{
    RenderError, RenderOptions, RenderProgress,
    device_budget::DeviceReservation,
    encode_target::{EncodeTarget, OpenedTarget},
    fonts::font_dir_from_env,
    media::{LocalMediaStore, media_root},
};

pub(super) const MEDIA_PREFETCH_LOOKAHEAD_FRAMES: u32 = 30;

/// Renders `composition` to MP4 bytes.
pub fn render_project_mp4(
    composition: &Composition,
    options: &RenderOptions,
    on_progress: &mut dyn FnMut(RenderProgress),
) -> Result<Vec<u8>, RenderError> {
    let media_root = media_root(options.media_root.as_deref()).map_err(|err| RenderError {
        code: "media_root_error",
        message: err.to_string(),
        retryable: false,
    })?;
    let font_dir = font_dir_from_env().map_err(|err| RenderError {
        code: "font_dir_error",
        message: err.to_string(),
        retryable: false,
    })?;
    let media_store = LocalMediaStore::new(media_root, font_dir.as_deref());
    let tmp = tempfile::tempdir().map_err(|err| RenderError {
        code: "encode_failed",
        message: format!("failed to create temp dir: {err}"),
        retryable: true,
    })?;
    let output_path = tmp.path().join("output.mp4");

    let render = Mp4Render::open(
        composition,
        media_store,
        options.video_encoder.as_deref(),
        &options.quality,
        &output_path,
    )?;
    render.run(on_progress)?;

    fs::read(&output_path).map_err(|err| RenderError {
        code: "encode_failed",
        message: format!("failed to read encoded output: {err}"),
        retryable: true,
    })
}

/// What a finished render did and where its time went.
#[derive(Debug, Clone, Default)]
pub struct RenderReport {
    pub frames: u32,
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
    /// Opens `encoder`, or `LUMEN_VIDEO_ENCODER`, or the platform default, on the path that keeps
    /// frames on the GPU when it has one.
    pub fn open(
        composition: &'a Composition,
        mut media_store: LocalMediaStore,
        encoder: Option<&str>,
        quality: &EncoderQuality,
        output: &Path,
    ) -> Result<Self, RenderError> {
        let (choice, info) = VideoEncoderChoice::resolve(encoder)
            .look_up(|warning| tracing::warn!("{warning}"))
            .map_err(invalid_render_profile)?;
        tracing::info!(encoder = choice.name, codec = ?info.codec, "selected video encoder");

        let OpenedTarget {
            renderer,
            target,
            encoder,
            path,
            devices,
        } = EncodeTarget::open(
            &choice,
            info.family,
            composition,
            &mut media_store,
            output,
            quality,
        )?;
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
            .map_err(|err| RenderError {
                code: "render_failed",
                message: format!("failed to compile composition: {err}"),
                retryable: false,
            })?;
        Ok(render)
    }

    /// The encoder that opened, which is the fallback if the automatic default wouldn't.
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
    ) -> Result<RenderReport, RenderError> {
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
            held_bytes: self.renderer.held_bytes(),
            submitted_passes: self.renderer.submitted_pass_count(),
            skipped_passes: self.renderer.skipped_pass_count(),
            timing,
        })
    }
}

fn invalid_render_profile(err: lumen_ffmpeg::FfmpegError) -> RenderError {
    RenderError {
        code: "invalid_render_profile",
        message: err.to_string(),
        retryable: false,
    }
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
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn encoder_that_cannot_open_is_not_retryable() {
        let composition = super::super::parse_composition(&serde_json::json!({
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
            // Registered in every FFmpeg build, and refuses the frames lumen gives a video encoder.
            video_encoder: Some("bmp".to_string()),
            ..RenderOptions::default()
        };

        let error = render_project_mp4(&composition, &options, &mut |_| {}).unwrap_err();

        assert_eq!(error.code, "encode_failed", "{error}");
        assert!(!error.retryable, "{error}");
    }

    #[test]
    fn an_unknown_encoder_is_an_invalid_render_profile() {
        let composition = super::super::parse_composition(&serde_json::json!({
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
            video_encoder: Some("definitely_not_an_encoder".to_string()),
            ..RenderOptions::default()
        };

        let error = render_project_mp4(&composition, &options, &mut |_| {}).unwrap_err();

        assert_eq!(error.code, "invalid_render_profile", "{error}");
        assert!(
            error.message.contains("definitely_not_an_encoder"),
            "{error}"
        );
    }
}
