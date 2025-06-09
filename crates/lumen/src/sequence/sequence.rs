use serde::{Deserialize, Serialize};

use super::element::SequenceElement;

#[derive(Serialize, Deserialize)]
pub struct Sequence {
    pub properties: Properties,
    pub media: Vec<Media>,
    pub layers: Vec<Vec<SequenceElement>>,
    pub audio: Vec<AudioElement>,
}

#[derive(Serialize, Deserialize)]
pub struct AudioElement {
    pub media_id: usize,
    pub start: usize,
    pub duration: usize,
    pub audio_start: usize,
    pub audio_end: usize,
    pub volume: Option<f32>,
}

#[derive(Serialize, Deserialize)]
pub struct Properties {
    pub fps: usize,
    pub length: usize,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Media {
    pub media_type: MediaType,
    pub id: usize,
    pub source: String,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(tag = "type")]
pub enum MediaType {
    Video,
    Audio,
    Image,
}
