use serde::{Deserialize, Serialize};

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
    pub fps: [u32; 2],
    pub duration_frames: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct AudioMix {
    #[serde(default)]
    pub tracks: Vec<AudioTrack>,
}

impl Default for AudioMix {
    fn default() -> Self {
        Self { tracks: Vec::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct AudioTrack {
    pub source: String,
    #[serde(default)]
    pub start_frame: u64,
    #[serde(default)]
    pub duration_frames: Option<u64>,
}

fn default_version() -> String {
    "1".to_string()
}

fn default_background() -> [u8; 4] {
    [0, 0, 0, 255]
}
