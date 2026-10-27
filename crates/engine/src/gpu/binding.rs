use std::collections::HashMap;

use crate::{
    composition::Composition,
    error::RenderError,
    expr::ExpressionContext,
    media::MediaStore,
    node::{NodeId, NodeKind, NodeParamEvalContext, NodeParams, PortRef},
};

use super::{BoundFrame, CompiledComposition, FramePortRef, TimeRemapPath, cache::BindCache};

#[derive(Debug)]
pub struct FrameBindContext<'a> {
    composition: &'a Composition,
    frame: f64,
    media: Option<&'a dyn MediaStore>,
}

impl<'a> FrameBindContext<'a> {
    pub fn new(composition: &'a Composition, frame: u32) -> Self {
        Self {
            composition,
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
            composition,
            frame,
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
        let output = media_output_port(self.composition)?;
        let mut visited = HashMap::new();
        let mut time_remaps = TimeRemapPath::new();
        self.bind_port(
            compiled,
            &output,
            self.frame,
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

    /// Binds the node at `port` after its inputs, returning whether its output renders this
    /// frame rather than staying as kept from an earlier one.
    #[allow(
        clippy::too_many_arguments,
        reason = "the traversal state is threaded through each recursive call"
    )]
    fn bind_port(
        &self,
        compiled: &CompiledComposition,
        port: &PortRef,
        frame: f64,
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

        let node = self
            .composition
            .graph
            .nodes
            .get(&port.id)
            .ok_or(RenderError::MissingNode {
                frame: frame as u32,
                node_id: port.id,
            })?;

        let frame_ctx = self.with_frame(frame);
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
                    .eval(memo.id, "key", &frame_ctx.expr_context(memo.id, "key"))?
                    .to_bits();
                // Without skipped passes the source is bound and dispatched as usual.
                holds = cache.hold_memo(instance, key) && cache.layout.enabled();
            }
        }
        let is_time_remap = matches!(node, NodeKind::TimeRemap(_));
        if is_time_remap {
            time_remaps.push(port.id);
        }
        let inputs = if holds {
            Vec::new()
        } else {
            frame_ctx.frame_inputs(node)?
        };
        let mut inputs_render = false;
        for input in inputs {
            inputs_render |= self.bind_port(
                compiled,
                &input.port,
                input.frame,
                time_remaps,
                visited,
                bound,
                cache,
            )?;
        }
        if is_time_remap {
            time_remaps.pop();
        }

        let compiled_node = compiled
            .compiled_nodes
            .get(&port.id)
            .and_then(|instances| instances.get(time_remaps.as_slice()));
        let renders = if kept && !inputs_render {
            match compiled_node {
                Some(compiled_node) => compiled_node.bind_kept(&frame_ctx, bound)?,
                None => false,
            }
        } else {
            if let Some(compiled_node) = compiled_node {
                tracing::trace!(
                    target: "lumen_bind",
                    frame = self.frame,
                    binding_frame = frame,
                    node_id = port.id.0,
                    "bind compiled node"
                );
                compiled_node.bind(&frame_ctx, bound)?;
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

    fn with_frame(&self, frame: f64) -> Self {
        Self {
            composition: self.composition,
            frame,
            media: self.media,
        }
    }

    fn frame_inputs(&self, node: &NodeKind) -> crate::Result<Vec<FramePortRef>> {
        match node {
            NodeKind::MediaOutput(node) => {
                Ok(vec![FramePortRef::new(node.source.clone(), self.frame)])
            }
            NodeKind::TimeRemap(node) => {
                let params = node.params.eval(&NodeParamEvalContext {
                    node_id: node.id,
                    expr: &self.expr_context(node.id, "params"),
                })?;
                let settings = crate::node::processing::time_remap::TimeRemapSettings {
                    frame: params.frame,
                    loop_enabled: params.loop_enabled,
                    loop_start: params.loop_start,
                    loop_end: params.loop_end,
                };
                Ok(vec![FramePortRef::new(
                    node.source.clone(),
                    crate::node::processing::time_remap::remap_frame(settings),
                )])
            }
            NodeKind::Switch(node) => {
                let params = node.params.eval(&NodeParamEvalContext {
                    node_id: node.id,
                    expr: &self.expr_context(node.id, "params"),
                })?;
                let selected =
                    (params.selected_layer >= 0).then_some(params.selected_layer as usize);
                Ok(selected
                    .and_then(|index| node.layers.get(index))
                    .map(|port| vec![FramePortRef::new(port.clone(), self.frame)])
                    .unwrap_or_default())
            }
            _ => Ok(node
                .input_ports()
                .into_iter()
                .map(|port| FramePortRef::new(port, self.frame))
                .collect()),
        }
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
        ExpressionContext {
            frame: self.frame,
            fps: self.composition.timeline.fps,
            width: self.composition.render_settings.width,
            height: self.composition.render_settings.height,
            duration_frames: self.composition.timeline.duration_frames,
            path: Some(format!("{node_id}.{property_path}")),
            graph: Some(&self.composition.graph),
            media: self.media,
            glyph: None,
        }
    }
}

fn media_output_port(composition: &Composition) -> crate::Result<PortRef> {
    let mut outputs = composition
        .graph
        .nodes
        .iter()
        .filter_map(|(node_id, node)| matches!(node, NodeKind::MediaOutput(_)).then_some(*node_id));
    let Some(output) = outputs.next() else {
        return Err(crate::error::GraphValidationError::MissingMediaOutput.into());
    };
    if outputs.next().is_some() {
        return Err(crate::error::GraphValidationError::MultipleMediaOutputs { count: 2 }.into());
    }
    Ok(PortRef::new(output, "output".to_string()))
}
