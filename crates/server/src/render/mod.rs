mod device_budget;
mod encode_target;
mod encoder;
mod fonts;
mod gpu;
mod media;

use std::{path::PathBuf, sync::Arc};

use lumen_engine::{
    composition::{Composition, OutputCodec},
    media::AssetResolver,
};
use lumen_ffmpeg::{EncodeCodec, QualityConfig, QualityRequest, VideoEncoderChoice};

pub use fonts::{FONT_DIR_ENV, FontCatalog, ResolvedFont, existing_font_dir, font_dir_from_env};
pub use gpu::{Mp4Render, RenderReport, RenderTiming, create_gpu_renderer, render_project_mp4};
pub use media::LocalMediaStore;

#[derive(Debug, Clone)]
pub struct RenderError {
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RenderError {}

#[derive(Debug, Clone)]
pub struct RenderProgress {
    pub stage: &'static str,
    pub frame: u32,
    pub total_frames: u32,
    pub ratio: f32,
}

#[derive(Debug, Clone, Default)]
pub struct RenderOptions {
    pub media_root: Option<PathBuf>,
    /// The encoders to try. `None` resolves the composition's settings against the built-in
    /// presets, with `LUMEN_VIDEO_ENCODER` pinning one.
    pub encoder: Option<VideoEncoderChoice>,
    /// Where `asset:` sources resolve. Without one, they don't.
    pub assets: Option<Arc<dyn AssetResolver>>,
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

/// What [`lumen_engine::json::check`] finds in `payload`, with fonts resolved as a render on this
/// machine resolves them: from `LUMEN_FONT_DIR`, the system, or the media `staged` says the render
/// will stage under that name. `asset:` references resolve in `assets`.
pub fn check_composition(
    payload: &serde_json::Value,
    staged: &dyn Fn(&str) -> bool,
    assets: &dyn AssetResolver,
) -> Vec<lumen_engine::diagnostic::Diagnostic> {
    let font_dir = font_dir_from_env().ok().flatten();
    let fonts = FontCatalog::without_media(font_dir.as_deref());
    let known_font = |family: &str| staged(family) || fonts.resolve(family, || None).is_some();
    lumen_engine::json::check_value(payload, Some(&known_font), Some(assets))
}

pub fn parse_composition(payload: &serde_json::Value) -> Result<Composition, RenderError> {
    lumen_engine::json::parse_value(payload).map_err(|err| RenderError {
        code: "invalid_project_payload",
        message: format!("{err:#}"),
        retryable: false,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::parse_composition;

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
        assert!(
            error.message.contains("composition `card`"),
            "{}",
            error.message
        );
        assert!(error.message.contains("not_real"), "{}", error.message);
    }
}
