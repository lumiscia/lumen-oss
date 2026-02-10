use thiserror::Error;

mod group;
mod layer;
mod text;

pub use group::Group;
pub use layer::Layer;
// pub use text::TextClip;

use crate::render::RenderContext;

pub type Timeline = Group;

#[derive(Error, Debug)]
pub enum ClipError {
    #[error("Clip tried to draw while out of its range")]
    OutOfRange,
    #[error("{0}")]
    Message(String),
    #[error("Unknown clip error")]
    Unknown,
}

pub trait Clip {
    fn draw(&self, frame: usize, context: &mut RenderContext) -> Result<(), ClipError>;
}
