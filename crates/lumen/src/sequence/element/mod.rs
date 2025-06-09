mod media;
mod shape;
mod text;

use serde::{Deserialize, Serialize};

pub use media::*;
pub use shape::*;
pub use text::*;

use crate::sequence::{Transition, effect::Effect};

#[derive(Serialize, Deserialize, PartialEq, Clone)]
pub enum SequenceElement {
    Shape(ShapeElement),
    Media(MediaElement),
    Text(TextElement),
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct ElementProperties {
    /// Start in microseconds
    pub start: u64,
    /// Duration in microseconds
    pub duration: u64,
    pub transform: Transform,
    pub transition_in: Option<Transition>,
    pub transition_out: Option<Transition>,

    #[serde(default)]
    pub effects: Vec<Effect>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
pub struct Transform {
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub width: Option<i32>,
    pub height: Option<i32>,
}
