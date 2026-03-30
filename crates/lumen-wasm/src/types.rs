use lumen::{
    composition::{Composition, RenderSettings, TimelineSettings},
    graph::{Connection, Graph},
    media::{FrameRequirements, VideoFrameRequirement},
    node::{
        NodeId, NodeKind, NodeProperty, PortRef,
        media_output::MediaOutput,
        source::solid_color::SolidColor,
    },
};
use serde::{Deserialize, Serialize};

// ── Frame requirements output ─────────────────────────────────────────────────

#[derive(Serialize)]
pub struct FrameRequirementsPayload {
    pub images: Vec<String>,
    pub videos: Vec<FrameRequirementsVideoPayload>,
}

#[derive(Serialize)]
pub struct FrameRequirementsVideoPayload {
    #[serde(rename = "streamId")]
    pub stream_id: String,
    pub frames: Vec<u32>,
}

impl From<FrameRequirements> for FrameRequirementsPayload {
    fn from(value: FrameRequirements) -> Self {
        Self {
            images: value.images,
            videos: value.videos.into_iter().map(FrameRequirementsVideoPayload::from).collect(),
        }
    }
}

impl From<VideoFrameRequirement> for FrameRequirementsVideoPayload {
    fn from(value: VideoFrameRequirement) -> Self {
        Self { stream_id: value.source_id, frames: value.frames }
    }
}

// ── Preview project input ─────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct PreviewProjectInput {
    pub canvas: PreviewCanvasInput,
    pub timeline: PreviewTimelineInput,
    #[serde(default)]
    pub sources: Vec<serde_json::Value>,
    #[serde(default)]
    pub layers: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
pub struct PreviewCanvasInput {
    pub width: u32,
    pub height: u32,
    #[serde(default = "default_background")]
    pub background: [u8; 4],
}

#[derive(Deserialize)]
pub struct PreviewTimelineInput {
    pub fps: PreviewFpsInput,
    pub total_frames: Option<u32>,
    pub duration_frames: Option<u32>,
}

#[derive(Deserialize)]
pub struct PreviewFpsInput {
    pub num: u32,
    pub den: u32,
}

const fn default_background() -> [u8; 4] {
    [0, 0, 0, 255]
}

// ── Preview project → Composition ────────────────────────────────────────────

pub fn preview_project_to_composition(
    project: PreviewProjectInput,
    scale: f32,
) -> Result<Composition, String> {
    if !(scale.is_finite() && scale > 0.0) {
        return Err("scale must be finite and > 0".to_string());
    }
    if project.timeline.fps.num == 0 || project.timeline.fps.den == 0 {
        return Err("timeline fps num/den must be > 0".to_string());
    }
    let total_frames = match (project.timeline.total_frames, project.timeline.duration_frames) {
        (Some(value), _) if value > 0 => value,
        (None, Some(value)) if value > 0 => value,
        _ => return Err("timeline.total_frames must be > 0".to_string()),
    };
    if !project.sources.is_empty() || !project.layers.is_empty() {
        return Err(
            "preview project conversion is not implemented for sources/layers yet".to_string(),
        );
    }

    let width = scaled_dimension(project.canvas.width, scale)?;
    let height = scaled_dimension(project.canvas.height, scale)?;
    let fps = (project.timeline.fps.num as f32) / (project.timeline.fps.den as f32);

    let solid_id = NodeId::new(1);
    let output_id = NodeId::new(2);
    let mut graph = Graph::new();
    graph.nodes.insert(
        solid_id,
        NodeKind::SolidColor(SolidColor {
            id: solid_id,
            color: NodeProperty::Color(project.canvas.background),
            width: NodeProperty::Int(i64::from(width)),
            height: NodeProperty::Int(i64::from(height)),
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(solid_id, "output".to_string()),
        }),
    );
    graph
        .connect(Connection {
            from_node: solid_id,
            from_port: "output".to_string(),
            to_node: output_id,
            to_port: "source".to_string(),
        })
        .map_err(|e| e.to_string())?;

    Ok(Composition::new(
        graph,
        TimelineSettings { fps, duration_frames: total_frames },
        RenderSettings { width, height, background_color: project.canvas.background },
    ))
}

fn scaled_dimension(value: u32, scale: f32) -> Result<u32, String> {
    let scaled = (value as f32) * scale;
    if !scaled.is_finite() || scaled <= 0.0 {
        return Err("scaled dimension must be > 0".to_string());
    }
    Ok(scaled.round().max(1.0) as u32)
}
