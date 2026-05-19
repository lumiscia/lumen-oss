use crate::{composition::Composition, expr::ExpressionContext, media::MediaStore, node::NodeId};

use super::{BoundFrame, CompiledComposition};

#[derive(Debug)]
pub struct FrameBindContext<'a> {
    composition: &'a Composition,
    frame: u32,
    media: Option<&'a dyn MediaStore>,
}

impl<'a> FrameBindContext<'a> {
    pub fn new(composition: &'a Composition, frame: u32) -> Self {
        Self {
            composition,
            frame,
            media: None,
        }
    }

    pub fn with_media<M: MediaStore>(
        composition: &'a Composition,
        frame: u32,
        media: &'a M,
    ) -> Self {
        Self {
            composition,
            frame,
            media: Some(media),
        }
    }

    pub fn bind(&self, compiled: &CompiledComposition) -> crate::Result<BoundFrame> {
        tracing::trace!(
            target: "lumen_bind",
            frame = self.frame,
            bindings = compiled.frame_bindings.len(),
            "bind compiled frame"
        );
        let mut bound = BoundFrame::new();
        for (index, binding) in compiled.frame_bindings.iter().enumerate() {
            let binding_frame = binding.frame_override.unwrap_or(self.frame);
            let binding_context = Self {
                composition: self.composition,
                frame: binding_frame,
                media: self.media,
            };
            let node_id = binding.node_id();
            tracing::trace!(
                target: "lumen_bind",
                frame = self.frame,
                binding_frame,
                node_id = node_id.0,
                binding_index = index,
                "bind frame resource"
            );
            binding.binding.bind(&binding_context, &mut bound)?;
        }
        Ok(bound)
    }

    pub(crate) fn frame(&self) -> u32 {
        self.frame
    }

    pub(crate) fn media(&self) -> Option<&dyn MediaStore> {
        self.media
    }

    pub(crate) fn expr_context(
        &self,
        node_id: NodeId,
        property_path: &str,
    ) -> ExpressionContext<'_> {
        ExpressionContext {
            frame: self.frame,
            fps: self.composition.timeline.fps,
            width: self.composition.render_settings.width,
            height: self.composition.render_settings.height,
            duration_frames: self.composition.timeline.duration_frames,
            path: Some(format!("{node_id}.{property_path}")),
            graph: Some(&self.composition.graph),
        }
    }
}
