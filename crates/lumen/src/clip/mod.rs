use crate::clip::media::Image;

pub mod layout;
pub mod media;
pub mod shape;
pub mod style;
pub mod text;

trait Clip {
    fn id(&self) -> Option<String>;

    fn start(&self) -> u32;

    fn end(&self) -> u32;

    fn draw(&self, frame: u32);
}

enum ClipType {
    Image(Image),
}

impl Clip for ClipType {
    fn id(&self) -> Option<String> {
        match self {
            ClipType::Image(image) => image.id(),
        }
    }

    fn draw(&self, frame: u32) {
        match self {
            ClipType::Image(image) => image.draw(frame),
        }
    }
}
