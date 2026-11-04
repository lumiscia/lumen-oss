mod encoder;
mod fonts;
mod gpu;
mod media;

use std::path::PathBuf;

use lumen_engine::composition::Composition;
use lumen_ffmpeg::EncoderQuality;

pub use fonts::{FONT_DIR_ENV, FontCatalog, ResolvedFont, existing_font_dir, font_dir_from_env};
pub use gpu::render_project_mp4;

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
    pub video_encoder: Option<String>,
    pub quality: EncoderQuality,
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
