use crate::clip::{Clip, Layer};

pub struct Group {
    pub layers: Vec<Layer>,
}

impl Clip for Group {
    fn draw(
        &self,
        frame: usize,
        context: &mut crate::render::RenderContext,
    ) -> Result<(), super::ClipError> {
        for layer in self.layers.iter() {
            layer.draw(frame, context)?;
        }

        Ok(())
    }
}
