use lumen_engine::{
    audio::AudioTimeline,
    media::{FrameRequirements, VideoFrameRequirement},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FrameRequirementsPayload {
    pub images: Vec<String>,
    pub videos: Vec<FrameRequirementsVideoPayload>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FrameRequirementsVideoPayload {
    #[serde(rename = "streamId")]
    pub stream_id: String,
    pub frames: Vec<u32>,
}

/// The parsed audio timeline, with the loop length the preview plays it over.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioTimelinePayload<'a> {
    #[serde(flatten)]
    pub timeline: &'a AudioTimeline,
    pub duration_frames: u32,
    pub fps: f64,
}

impl From<FrameRequirements> for FrameRequirementsPayload {
    fn from(value: FrameRequirements) -> Self {
        Self {
            images: value.images,
            videos: value
                .videos
                .into_iter()
                .map(FrameRequirementsVideoPayload::from)
                .collect(),
        }
    }
}

impl From<VideoFrameRequirement> for FrameRequirementsVideoPayload {
    fn from(value: VideoFrameRequirement) -> Self {
        Self {
            stream_id: value.stream_id,
            frames: value.frames,
        }
    }
}
