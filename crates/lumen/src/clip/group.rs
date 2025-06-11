use rangemap::RangeMap;

use crate::clip::Clip;

pub struct Group {
    layers: Vec<RangeMap<usize, Box<dyn Clip>>>,
}

impl Clip for Group {
    fn draw(
        &self,
        frame: usize,
        context: &mut crate::render::RenderContext,
    ) -> Result<(), super::ClipError> {
        for layer in self.layers.iter() {
            if let Some(clip) = layer.get(&frame) {
                clip.draw(frame, context)?;
            }
        }

        Ok(())
    }
}
