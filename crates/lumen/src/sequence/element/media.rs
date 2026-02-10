use serde::{Deserialize, Serialize};

use crate::{Timestamp, sequence::element::ElementProperties};

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct MediaElement {
    pub source: u32,
    pub media: Media,
    pub properties: ElementProperties,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
pub enum Media {
    Video { video_start: Timestamp },
    Image,
}
