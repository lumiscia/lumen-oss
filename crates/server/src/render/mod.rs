mod encoder;
#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
mod frame_timing;
mod gpu;
mod media;

use std::path::PathBuf;

use lumen_engine::composition::Composition;

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
    pub verbose_debug: bool,
    pub video_encoder: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProjectInfo {
    pub width: u32,
    pub height: u32,
    pub duration_frames: u32,
}

pub struct ProjectBundle {
    pub project: ProjectInfo,
    pub composition: Composition,
}

pub fn convert_project_payload(payload: &serde_json::Value) -> Result<ProjectBundle, RenderError> {
    let project = serde_json::to_string(payload).map_err(|err| RenderError {
        code: "invalid_project_payload",
        message: err.to_string(),
        retryable: false,
    })?;

    let composition = lumen_engine::json::parse(&project).map_err(|err| RenderError {
        code: "invalid_project_payload",
        message: err.to_string(),
        retryable: false,
    })?;
    Ok(ProjectBundle {
        project: ProjectInfo {
            width: composition.render_settings.width,
            height: composition.render_settings.height,
            duration_frames: composition.timeline.duration_frames,
        },
        composition,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::convert_project_payload;

    #[test]
    fn convert_current_composition_preserves_duration_frames() {
        let payload = json!({
            "timeline": {
                "fps": 24,
                "duration_frames": 120
            },
            "render_settings": {
                "width": 64,
                "height": 64,
                "background_color": [0, 0, 0, 255]
            },
            "nodes": [
                {
                    "id": 1,
                    "type": "solid_color",
                    "properties": {
                        "color": [255, 255, 255, 255],
                        "width": 64,
                        "height": 64
                    }
                },
                {
                    "id": 2,
                    "type": "media_output",
                    "properties": {}
                }
            ],
            "connections": [
                {
                    "from_node": 1,
                    "from_port": "output",
                    "to_node": 2,
                    "to_port": "source"
                }
            ]
        });

        let bundle = convert_project_payload(&payload).expect("project bundle");
        assert_eq!(bundle.project.duration_frames, 120);
    }
}
