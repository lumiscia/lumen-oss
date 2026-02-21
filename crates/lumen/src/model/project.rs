use serde::{Deserialize, Serialize};

use crate::time::Rational;

use super::{Layer, Source};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Project {
    #[serde(default = "default_version")]
    pub version: String,
    pub canvas: Canvas,
    pub timeline: Timeline,
    #[serde(default)]
    pub sources: Vec<Source>,
    #[serde(default)]
    pub layers: Vec<Layer>,
    #[serde(default)]
    pub audio: AudioMix,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    #[serde(default = "default_background")]
    pub background: [u8; 4],
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Timeline {
    pub fps: Rational,
    pub duration_frames: u64,
}

impl Timeline {
    pub fn fps_f32(&self) -> f32 {
        self.fps.as_f32()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct AudioMix {
    #[serde(default)]
    pub tracks: Vec<AudioTrack>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct AudioTrack {
    pub source: String,
    #[serde(default)]
    pub start_frame: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_frames: Option<u64>,
}

fn default_version() -> String {
    "1".to_string()
}

fn default_background() -> [u8; 4] {
    [0, 0, 0, 255]
}
