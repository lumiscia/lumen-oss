use serde::{Deserialize, Serialize};

use crate::sequence::element::ElementProperties;

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct ShapeElement {
    pub shape: Shape,
    pub properties: ElementProperties,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Copy)]
pub enum Shape {
    Rectangle { radius: f32 },
    Video { video_start: u64, video_end: u64 },
    Image,
}
