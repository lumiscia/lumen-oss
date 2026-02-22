pub mod group;
pub mod layout;
pub mod media;
pub mod shape;
pub mod style;
pub mod text;

use crate::render::context::FrameContext;

#[derive(Debug, Clone)]
pub struct ClipMeta {
    pub id: Option<String>,
    pub start_frame: u32,
    pub end_frame: u32,
}
pub trait Clip {
    fn meta(&self) -> &ClipMeta;

    fn id(&self) -> Option<&str> {
        self.meta().id.as_deref()
    }

    fn start(&self) -> u32 {
        self.meta().start_frame
    }

    fn end(&self) -> u32 {
        self.meta().end_frame
    }

    fn draw(&self, frame: u32, frame_ctx: &FrameContext);
}

#[derive(Debug)]
pub enum ClipType {
    Group(group::GroupClip),
    Layout(layout::LayoutClip),
    Image(media::ImageClip),
    Video(media::VideoClip),
    Shape(shape::ShapeClip),
    Text(text::TextClip),
}
