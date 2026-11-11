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

use lumen_engine::{
    composition::{Composition, OutputCodec},
    media::AssetResolver,
};
use lumen_ffmpeg::{EncodeCodec, QualityConfig, QualityRequest, VideoEncoderChoice};

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
    /// The encoders to try, for an MP4 output only. `None` resolves the composition's settings
    /// against the built-in presets.
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

/// The encoders a render of `composition` tries: `request`'s preset and codec where it names them,
/// then the composition's `render_settings`, then `config`'s defaults.
pub fn resolve_encoder(
    config: &QualityConfig,
    composition: &Composition,
    mut request: QualityRequest,
) -> Result<VideoEncoderChoice, String> {
    let output = composition.output_settings();
    request.preset = request.preset.or_else(|| output.quality.clone());
    request.codec = request.codec.or(output.codec.map(|codec| match codec {
        OutputCodec::H264 => EncodeCodec::H264,
        OutputCodec::Hevc => EncodeCodec::Hevc,
        OutputCodec::Av1 => EncodeCodec::Av1,
    }));
    config.resolve(&request)
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
