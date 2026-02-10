use skia_safe::{Font, Paint, utils::text_utils::Align};

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

        let canvas = context.surface.canvas();

        let font = Font::new(context.font_manager.arial().unwrap(), 120.0);

        let paint = Paint::new(self.color.as_color4f(), None);

        canvas.draw_str_align(
            &self.text,
            (
                self.properties.transform.x.unwrap_or(0),
                self.properties.transform.y.unwrap_or(0),
            ),
            &font,
            &paint,
            Align::Center,
        );

        Ok(())
    }
}
