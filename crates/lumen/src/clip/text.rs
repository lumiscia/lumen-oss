use crate::{
    clip::{Clip, ClipError},
    render::RenderContext,
    sequence::element::TextElement,
};

impl Clip for TextElement {
    fn draw(&self, frame: usize, context: &mut RenderContext) -> Result<(), super::ClipError> {
        if frame < self.properties.start || frame > self.properties.start + self.properties.duration
        {
            return Err(ClipError::OutOfRange);
        }

        let mut canvas = context.surface.canvas();

        todo!()
    }
}
