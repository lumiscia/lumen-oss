use std::collections::BTreeMap;

use crate::clip::Clip;

pub struct Layer {
    inner: BTreeMap<usize, (Box<dyn Clip + Sync + Send>, usize)>,
}

impl Layer {
    pub fn new() -> Self {
        Self {
            inner: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, clip: Box<dyn Clip + Sync + Send>, start: usize, duration: usize) {
        self.inner.insert(start, (clip, duration));
    }

    pub fn get_clip_at(
        &self,
        frame: usize,
    ) -> Option<(&Box<dyn Clip + Sync + Send>, &usize, &usize)> {
        self.inner.iter().find_map(|(start, (clip, duration))| {
            if frame >= *start && frame < start + duration {
                Some((clip, start, duration))
            } else {
                None
            }
        })
    }
}

impl Clip for Layer {
    fn draw(
        &self,
        frame: usize,
        context: &mut crate::render::RenderContext,
    ) -> Result<(), super::ClipError> {
        if let Some((clip, start, _)) = self.get_clip_at(frame) {
            clip.draw(frame - *start, context)
        } else {
            Ok(())
        }
    }
}
