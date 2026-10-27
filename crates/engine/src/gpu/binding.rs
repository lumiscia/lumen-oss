use std::collections::HashMap;

use crate::{
    composition::Composition,
    error::RenderError,
    expr::ExpressionContext,
    media::MediaStore,
    node::{NodeId, NodeKind, PortRef},
    scope::{FrameTime, Routed, Scope},
};

use super::{BoundFrame, CompiledComposition, TimeRemapPath, cache::BindCache};

#[derive(Debug)]
pub struct FrameBindContext<'a> {
    /// The composition the node being bound belongs to.
    scope: Scope<'a>,
    frame: FrameTime,
    media: Option<&'a dyn MediaStore>,
}

impl<'a> FrameBindContext<'a> {
    pub fn new(composition: &'a Composition, frame: u32) -> Self {
        Self {
            scope: Scope::root(composition),
            frame: f64::from(frame).into(),
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
            frame: frame.into(),
            media: Some(media),
        }
    }

    pub fn bind(&self, compiled: &CompiledComposition) -> crate::Result<BoundFrame> {
        self.bind_with_cache(compiled, None)
    }

    /// Binds every node, except those whose output `cache` keeps from earlier frames.
    pub(crate) fn bind_with_cache(
        &self,
        compiled: &CompiledComposition,
        mut cache: Option<BindCache<'_>>,
    ) -> crate::Result<BoundFrame> {
        tracing::trace!(
            target: "lumen_bind",
            frame = self.frame.value(),
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
        let mut visited = HashMap::new();
        let mut time_remaps = TimeRemapPath::new();
        self.bind_port(
            compiled,
            &output,
            &mut time_remaps,
            &mut visited,
            &mut bound,
            &mut cache,
        )?;
        bound.cache = cache.map(|cache| {
            let mut kept = cache.bound;
            if bound.text_dropped_glyphs {
                kept.keep_nothing();
            }
            kept
        });
        if bound.text_generation_count > 0 && bound.text_generation_ms > 3.0 {
            tracing::debug!(target: "lumen_text", glyphs = bound.text_generation_count, generation_ms = bound.text_generation_ms, "cold text glyph generation");
        }
        Ok(bound)
    }

    /// Binds `port`, which this context's scope and frame are for, after everything it reads,
    /// returning whether its output renders this frame rather than staying as kept from an
    /// earlier one.
    fn bind_port(
        &self,
        compiled: &CompiledComposition,
        port: &PortRef,
        time_remaps: &mut TimeRemapPath,
        visited: &mut HashMap<(PortRef, TimeRemapPath), bool>,
        bound: &mut BoundFrame,
        cache: &mut Option<BindCache<'_>>,
    ) -> crate::Result<bool> {
        if port.is_empty() {
            return Ok(false);
        }
        let visit = (port.clone(), time_remaps.clone());
        if let Some(renders) = visited.get(&visit) {
            return Ok(*renders);
        }
        visited.insert(visit.clone(), false);

        let node = self.scope.node(port.id).ok_or(RenderError::MissingNode {
            frame: self.frame(),
            node_id: port.id,
        })?;

        let instance = cache
            .as_ref()
            .and_then(|cache| cache.layout.instance(port.id, time_remaps));
        let mut kept = false;
        let mut holds = false;
        if let Some(cache) = cache.as_mut()
            && let Some(instance) = instance
        {
            kept = cache
                .state
                .keeps_static(cache.layout, instance, cache.epoch);
            // Text under kept output still places its glyphs, as the atlas is shared.
            if kept && !cache.layout.reaches_text(instance) {
                return Ok(false);
            }
            if let NodeKind::Memo(memo) = node
                && cache.layout.is_keyed_memo(instance)
            {
                // Keys compare by bits, so a key that is NaN on consecutive frames still holds.
                let key = memo
                    .params
                    .key
                    .eval(memo.id, "key", &self.expr_context(memo.id, "key"))?
                    .to_bits();
                // Without skipped passes the source is bound and dispatched as usual.
                holds = cache.hold_memo(instance, key) && cache.layout.enabled();
            }
        }
        let inputs = if holds {
            Vec::new()
        } else {
            crate::scope::route(&self.scope, node, self.frame, self.media)?
        };
        let mut inputs_render = false;
        for Routed {
            port: input,
            frame,
            scope,
            instance: remap,
        } in inputs
        {
            let input_ctx = Self {
                scope,
                frame,
                media: self.media,
            };
            time_remaps.extend(remap);
            inputs_render |=
                input_ctx.bind_port(compiled, &input, time_remaps, visited, bound, cache)?;
            if remap.is_some() {
                time_remaps.pop();
            }
        }

        let compiled_node = compiled
            .compiled_nodes
            .get(&port.id)
            .and_then(|instances| instances.get(time_remaps.as_slice()));
        let renders = if kept && !inputs_render {
            match compiled_node {
                Some(compiled_node) => compiled_node.bind_kept(self, bound)?,
                None => false,
            }
        } else {
            if let Some(compiled_node) = compiled_node {
                tracing::trace!(
                    target: "lumen_bind",
                    binding_frame = self.frame.value(),
                    node_id = port.id.0,
                    "bind compiled node"
                );
                compiled_node.bind(self, bound)?;
            }
            true
        };
        if renders
            && let Some(cache) = cache.as_mut()
            && let Some(instance) = instance
        {
            cache.mark_bound(instance);
        }
        visited.insert(visit, renders);
        Ok(renders)
    }

    /// Integer frame containing the current frame time, for diagnostics.
    pub(crate) fn frame(&self) -> u32 {
        self.frame.value() as u32
    }

    pub(crate) fn frame_time(&self) -> FrameTime {
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
