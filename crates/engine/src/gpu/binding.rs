use std::collections::HashSet;

use crate::{
    composition::Composition,
    error::RenderError,
    expr::ExpressionContext,
    media::MediaStore,
    node::{NodeId, PortRef},
    scope::{Routed, Scope},
};

use super::{BoundFrame, CompiledComposition, TimeRemapPath};

#[derive(Debug)]
pub struct FrameBindContext<'a> {
    /// The composition the node being bound belongs to.
    scope: Scope<'a>,
    frame: f64,
    media: Option<&'a dyn MediaStore>,
}

impl<'a> FrameBindContext<'a> {
    pub fn new(composition: &'a Composition, frame: u32) -> Self {
        Self {
            scope: Scope::root(composition),
            frame: f64::from(frame),
            media: None,
        }
    }

    pub fn with_media<M: MediaStore>(
        composition: &'a Composition,
        frame: u32,
        media: &'a M,
    ) -> Self {
        Self::with_media_at(composition, f64::from(frame), media)
    }

    /// Binds at a fractional frame time, e.g. `12.5` is halfway between frames 12 and 13.
    pub fn with_media_at<M: MediaStore>(
        composition: &'a Composition,
        frame: f64,
        media: &'a M,
    ) -> Self {
        Self {
            scope: Scope::root(composition),
            frame,
            media: Some(media),
        }
    }

    pub fn bind(&self, compiled: &CompiledComposition) -> crate::Result<BoundFrame> {
        tracing::trace!(
            target: "lumen_bind",
            frame = self.frame,
            nodes = compiled.compiled_nodes.len(),
            "bind compiled frame"
        );
        let mut bound = BoundFrame::new();
        if compiled.text_atlas_texture.is_some() {
            bound.text_atlas_frame = compiled
                .text_atlas
                .lock()
                .map_err(|_| RenderError::Gpu {
                    details: "text atlas lock was poisoned".to_string(),
                })?
                .begin_frame();
            bound.text_atlas = Some(std::sync::Arc::clone(&compiled.text_atlas));
        }
        let output = PortRef::new(self.scope.media_output()?, "output".to_string());
        let mut visited = HashSet::new();
        let mut time_remaps = TimeRemapPath::new();
        self.bind_port(
            compiled,
            &output,
            &mut time_remaps,
            &mut visited,
            &mut bound,
        )?;
        if bound.text_generation_count > 0 && bound.text_generation_ms > 3.0 {
            tracing::debug!(target: "lumen_text", glyphs = bound.text_generation_count, generation_ms = bound.text_generation_ms, "cold text glyph generation");
        }
        Ok(bound)
    }

    /// Binds `port`, which this context's scope and frame are for, after everything it reads.
    fn bind_port(
        &self,
        compiled: &CompiledComposition,
        port: &PortRef,
        time_remaps: &mut TimeRemapPath,
        visited: &mut HashSet<(PortRef, TimeRemapPath)>,
        bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        if port.is_empty() || !visited.insert((port.clone(), time_remaps.clone())) {
            return Ok(());
        }

        let node = self.scope.node(port.id).ok_or(RenderError::MissingNode {
            frame: self.frame as u32,
            node_id: port.id,
        })?;

        for Routed {
            port: input,
            frame,
            scope,
            instance,
        } in crate::scope::route(&self.scope, node, self.frame, self.media)?
        {
            let input_ctx = Self {
                scope,
                frame,
                media: self.media,
            };
            time_remaps.extend(instance);
            input_ctx.bind_port(compiled, &input, time_remaps, visited, bound)?;
            if instance.is_some() {
                time_remaps.pop();
            }
        }

        if let Some(compiled_node) = compiled
            .compiled_nodes
            .get(&port.id)
            .and_then(|instances| instances.get(time_remaps.as_slice()))
        {
            tracing::trace!(
                target: "lumen_bind",
                binding_frame = self.frame,
                node_id = port.id.0,
                "bind compiled node"
            );
            compiled_node.bind(self, bound)?;
        }

        Ok(())
    }

    /// Integer frame containing the current frame time, for diagnostics.
    pub(crate) fn frame(&self) -> u32 {
        self.frame as u32
    }

    pub(crate) fn frame_time(&self) -> f64 {
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
        self.scope
            .expr_context(self.frame, format!("{node_id}.{property_path}"), self.media)
    }
}
