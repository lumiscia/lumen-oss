use std::collections::HashSet;

use crate::{
    composition::Composition,
    error::RenderError,
    expr::ExpressionContext,
    media::MediaStore,
    node::{NodeId, NodeKind, NodeParamEvalContext, NodeParams, PortRef},
};

use super::{BoundFrame, CompiledComposition, FramePortRef};

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
            nodes = compiled.compiled_nodes.len(),
            "bind compiled frame"
        );
        let mut bound = BoundFrame::new();
        let output = media_output_port(self.composition)?;
        let mut visited = HashSet::new();
        self.bind_port(compiled, &output, self.frame, &mut visited, &mut bound)?;
        Ok(bound)
    }

    fn bind_port(
        &self,
        compiled: &CompiledComposition,
        port: &PortRef,
        frame: u32,
        visited: &mut HashSet<FramePortRef>,
        bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        if port.is_empty() || !visited.insert(FramePortRef::new(port.clone(), frame)) {
            return Ok(());
        }

        let node = self
            .composition
            .graph
            .nodes
            .get(&port.id)
            .ok_or(RenderError::MissingNode {
                frame,
                node_id: port.id,
            })?;

        let frame_ctx = self.with_frame(frame);
        for input in frame_ctx.frame_inputs(node)? {
            self.bind_port(compiled, &input.port, input.frame, visited, bound)?;
        }

        if let Some(compiled_node) = compiled.compiled_nodes.get(&port.id) {
            tracing::trace!(
                target: "lumen_bind",
                frame = self.frame,
                binding_frame = frame,
                node_id = port.id.0,
                "bind compiled node"
            );
            compiled_node.bind(&frame_ctx, bound)?;
        }

        Ok(())
    }

    fn with_frame(&self, frame: u32) -> Self {
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
                let selected = (params.selected_layer >= 0).then_some(params.selected_layer as usize);
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
