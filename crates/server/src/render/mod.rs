mod archive;
mod device_budget;
mod encode_target;
mod encoder;
mod error;
mod fonts;
mod gpu;
mod image_render;
mod media;
mod output;
mod readback;

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use lumen_engine::{composition::Composition, media::AssetResolver};
use lumen_ffmpeg::{
    AudioEncoderConfig, QualityConfig, QualityRequest, VideoEncoderChoice, check_audio_encoder,
};

pub use error::{ErrorCode, ServerRenderError};
pub use fonts::{FONT_DIR_ENV, FontCatalog, ResolvedFont, existing_font_dir};
pub use gpu::{
    Mp4Render, RenderReport, RenderTiming, RenderedOutput, create_gpu_renderer, render_project,
};
pub use image_render::{EncodedImage, ImageRender};
pub use lumen_gpu::GpuSettings;
pub use media::{HARDWARE_DECODE_ENV, LocalMediaStore, is_enabled_flag};
pub use output::{
    DEFAULT_JPEG_QUALITY, FrameRange, MAX_ARCHIVE_FRAMES, OutputFormat, RenderOutput,
    SequenceLimits, frame_number_width,
};

/// What renders take from their host. The library reads no environment variables: a binary reads
/// [`FONT_DIR_ENV`], [`HARDWARE_DECODE_ENV`] and [`GpuSettings::from_env`], or its flags, and builds
/// this, with [`std::env::temp_dir`] as `temp_dir`. The default has no font directory, decodes
/// video on the CPU, picks the GPU with [`GpuSettings::default`] and leaves the temporary
/// directory to the system.
#[derive(Debug, Clone, Default)]
pub struct RenderConfig {
    /// Fonts looked up before the media root's `assets/fonts` and the system fonts. A render fails
    /// with [`ErrorCode::InvalidConfig`] if it isn't a directory.
    pub font_dir: Option<PathBuf>,
    /// Decode video on the GPU where the hardware supports it.
    pub hardware_decode: bool,
    /// How the GPU instance and adapter are chosen. The default reads no environment variables.
    pub gpu: GpuSettings,
    /// Where a render keeps its output while encoding, in a directory of its own that it removes.
    /// `None` is the system temporary directory, which the standard library takes from `TMPDIR`.
    pub temp_dir: Option<PathBuf>,
}

impl RenderConfig {
    pub(crate) fn font_dir(&self) -> Option<&Path> {
        self.font_dir.as_deref()
    }
}

#[derive(Debug, Clone)]
pub struct RenderProgress {
    pub stage: &'static str,
    pub frame: u32,
    pub total_frames: u32,
    pub ratio: f32,
}

/// Stops the renders that hold a clone of it. A render checks it before each frame and, once it is
/// cancelled, ends with [`ErrorCode::Cancelled`] after releasing its encoder, output and media.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// The cancelled error once the token is cancelled.
    pub(crate) fn check(&self) -> Result<(), ServerRenderError> {
        if self.is_cancelled() {
            return Err(ServerRenderError::cancelled());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct RenderOptions {
    pub config: RenderConfig,
    /// The directory media sources must stay inside. Without one, the current directory.
    pub media_root: Option<PathBuf>,
    /// The encoders to try, for a video output only. `None` resolves the built-in presets with
    /// [`resolve_encoder`].
    pub encoder: Option<VideoEncoderChoice>,
    /// Where `asset:` sources resolve. Without one, they don't.
    pub assets: Option<Arc<dyn AssetResolver>>,
    /// The most glyph, stroke and decoration instances one text node may draw. `None` keeps
    /// [`lumen_engine::gpu::DEFAULT_MAX_TEXT_GLYPHS`].
    pub max_text_glyphs: Option<usize>,
    /// What to render: the format and the frames. The default is an MP4 of the whole timeline.
    pub output: RenderOutput,
    /// What an image sequence may add up to, which [`render_project`] enforces.
    pub sequence_limits: SequenceLimits,
    /// Cancels the render when [`CancelToken::cancel`] is called on a clone of it.
    pub cancel: CancelToken,
}

/// The encoders a render of `composition` to `format` tries: `request`'s preset, codec, bit depth
/// and alpha where it names them, then the codec a MOV or WebM holds, then `config`'s defaults. A
/// codec or alpha `format` can't hold, or audio in a composition with audio when this FFmpeg has no
/// encoder for `format`'s audio (Opus in WebM, AAC otherwise), is [`ErrorCode::InvalidOutput`], and
/// anything else no encoder here can write is [`ErrorCode::InvalidQuality`].
pub fn resolve_encoder(
    config: &QualityConfig,
    composition: &Composition,
    format: OutputFormat,
    request: QualityRequest,
) -> Result<VideoEncoderChoice, ServerRenderError> {
    resolve_encoder_with(config, composition, format, request, &check_audio_encoder)
}

/// [`resolve_encoder`], with `audio_encoder` saying whether this FFmpeg can encode an audio
/// config.
fn resolve_encoder_with(
    config: &QualityConfig,
    composition: &Composition,
    format: OutputFormat,
    mut request: QualityRequest,
    audio_encoder: &dyn Fn(&AudioEncoderConfig) -> Result<(), String>,
) -> Result<VideoEncoderChoice, ServerRenderError> {
    if format != OutputFormat::Mp4 {
        request.codec = request.codec.or(format.video_codecs().first().copied());
    }
    format
        .check_video(request.codec, request.alpha.unwrap_or(false))
        .map_err(|message| ServerRenderError::new(ErrorCode::InvalidOutput, message, false))?;
    if encoder::has_audio(composition) {
        audio_encoder(&encoder::audio_config(format == OutputFormat::Webm))
            .map_err(|message| ServerRenderError::new(ErrorCode::InvalidOutput, message, false))?;
    }
    config
        .resolve(&request)
        .map_err(|message| ServerRenderError::new(ErrorCode::InvalidQuality, message, false))
}

/// What [`lumen_engine::json::check`] finds in `payload`, with fonts resolved as a render with
/// `config` resolves them: from its font directory, the system, or the media `staged` says the
/// render will stage under that name. `asset:` references resolve in `assets`.
pub fn check_composition(
    payload: &serde_json::Value,
    staged: &dyn Fn(&str) -> bool,
    assets: &dyn AssetResolver,
    config: &RenderConfig,
) -> Vec<lumen_engine::diagnostic::Diagnostic> {
    let fonts = FontCatalog::without_media(config.font_dir());
    let known_font = |family: &str| staged(family) || fonts.resolve(family, || None).is_some();
    lumen_engine::json::check_value(payload, Some(&known_font), Some(assets))
}

/// The composition `payload` describes. A payload that isn't one fails with
/// [`ErrorCode::InvalidProjectPayload`], caused by the [`lumen_engine::json::ParseError`] that
/// holds its diagnostics.
pub fn parse_composition(payload: &serde_json::Value) -> Result<Composition, ServerRenderError> {
    lumen_engine::json::parse_value(payload).map_err(|err| {
        ServerRenderError::new(
            ErrorCode::InvalidProjectPayload,
            "the payload is not a composition",
            false,
        )
        .caused_by(err)
    })
}

/// Held by a test while it opens GPU devices. Several of the library's tests doing so at once
/// crash some Vulkan drivers inside `vkSetDebugUtilsObjectNameEXT`.
#[cfg(test)]
pub(crate) fn gpu_test_guard() -> std::sync::MutexGuard<'static, ()> {
    static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());
    GPU.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod output_tests;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ErrorCode, parse_composition};

    #[test]
    fn invalid_payload_message_includes_the_nested_cause() {
        let payload = json!({
            "timeline": { "fps": 24, "duration_frames": 120 },
            "render_settings": { "width": 64, "height": 64 },
            "compositions": {
                "card": {
                    "nodes": [{ "id": 1, "type": "merge", "params": { "blend_mode": "not_real" } }],
                    "connections": []
                }
            },
            "nodes": [],
            "connections": []
        });

        let Err(error) = parse_composition(&payload) else {
            panic!("the payload should be rejected");
        };
        assert_eq!(error.code(), ErrorCode::InvalidProjectPayload);
        let message = format!("{:#}", anyhow::Error::new(error));
        assert!(message.contains("composition `card`"), "{message}");
        assert!(message.contains("not_real"), "{message}");
    }
}
