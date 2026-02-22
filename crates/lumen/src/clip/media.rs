use std::ops::Range;

use crate::{
    LoopMode,
    clip::{Clip, style::BaseStyle},
};

#[derive(Debug, Clone)]
pub struct Image {
    id: Option<String>,
    source: String,
    style: BaseStyle,
}

impl Clip for Image {
    fn id(&self) -> Option<String> {
        self.id
    }

    fn draw(&self, frame: u32) {
        todo!()
    }

    fn start(&self) -> u32 {
        todo!()
    }

    fn end(&self) -> u32 {
        todo!()
    }
}

#[derive(Debug, Clone)]
pub struct Video {
    id: Option<String>,
    source: String,
    style: BaseStyle,
    trim: Option<Range<f32>>,
    speed: f32,
    r#loop: LoopMode,
}

impl Clip for Video {
    fn id(&self) -> Option<String> {
        self.id
    }

    fn draw(&self, frame: u32) {
        todo!()
    }

    fn start(&self) -> u32 {
        todo!()
    }

    fn end(&self) -> u32 {
        todo!()
    }
}
