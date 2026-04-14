use std::{collections::HashMap, rc::Rc};

use crate::{
    error::{LumenError, RenderError},
    expr::ExpressionContext,
    media::MediaStore,
    node::{Node, NodeEval, NodeId, NodeResult, PortRef},
    render::{LumenRenderer, surface::SurfacePool},
};

pub struct RenderContext<'a, S: SurfacePool, M: MediaStore> {
    pub renderer: &'a LumenRenderer<'a, S, M>,
    pub frame: u32,

    output_cache: HashMap<PortRef, Rc<NodeResult>>,
}

impl<'a, S: SurfacePool, M: MediaStore> RenderContext<'a, S, M> {
    pub fn new(renderer: &'a LumenRenderer<'a, S, M>, frame: u32) -> Self {
        Self {
            renderer,
            frame,
            output_cache: Default::default(),
        }
    }

    pub fn eval(&mut self, port: &PortRef) -> crate::Result<Rc<NodeResult>> {
        if let Some(cached) = self.output_cache.get(&port) {
            return Ok(Rc::clone(cached));
        }

        let node = self
            .renderer
            .composition
            .graph
            .nodes
            .get(&port.id)
            .ok_or_else(|| self.missing_node_error(port.id))?;
        let node_id = node.id();
        let result = node.evaluate(self, &port.port)?;

        if self
            .renderer
            .composition
            .graph
            .outgoing_connection_count(node_id)
            > 1
        {
            let result = Rc::new(result);
            self.output_cache.insert(port.clone(), Rc::clone(&result));
            return Ok(result);
        }

        Ok(Rc::new(result))
    }

    /// Evaluates once without caching.
    pub fn eval_once(&mut self, port: &PortRef) -> crate::Result<NodeResult> {
        let node = self
            .renderer
            .composition
            .graph
            .nodes
            .get(&port.id)
            .ok_or_else(|| self.missing_node_error(port.id))?;
        node.evaluate(self, &port.port)
    }

    pub fn missing_node_error(&self, node_id: NodeId) -> LumenError {
        RenderError::MissingNode {
            frame: self.frame,
            node_id,
        }
        .into()
    }

    pub fn missing_node_output_error(&self, node_id: NodeId) -> LumenError {
        RenderError::MissingNodeOutput {
            frame: self.frame,
            node_id,
        }
        .into()
    }

    pub fn invalid_node_output_type(
        &self,
        node_id: NodeId,
        expected: &'static str,
        actual: &'static str,
    ) -> LumenError {
        RenderError::InvalidNodeOutputType {
            frame: self.frame,
            node_id,
            expected,
            actual,
        }
        .into()
    }

    pub fn expr_context(&self, path: String) -> ExpressionContext<'_> {
        ExpressionContext {
            frame: self.frame,
            fps: self.renderer.composition.timeline.fps,
            width: self.renderer.composition.render_settings.width,
            height: self.renderer.composition.render_settings.height,
            duration_frames: self.renderer.composition.timeline.duration_frames,
            path: Some(path),
            graph: Some(&self.renderer.composition.graph),
        }
    }
}
