use std::{collections::HashMap, rc::Rc};

use crate::{
    expr::ExpressionContext,
    media::MediaStore,
    node::{Node, NodeEval, NodeResult, PortRef},
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

    pub fn eval(&mut self, port: PortRef) -> crate::Result<Rc<NodeResult>> {
        if let Some(cached) = self.output_cache.get(&port) {
            return Ok(cached.clone());
        }
        match self.renderer.composition.graph.nodes.get(&port.id) {
            Some(node) => {
                let node_id = node.id();
                let result = node
                    .evaluate(self, &port.port)
                    .map(|result| Rc::new(result))?;

                // cache if there's another thing that needs it
                if self
                    .renderer
                    .composition
                    .graph
                    .connections
                    .iter()
                    .filter(|connection| connection.from_node == node_id)
                    .count()
                    > 1
                {
                    self.output_cache.insert(port, result.clone());
                }

                Ok(result)
            }
            None => Err(todo!("create error type")),
        }
    }

    /// Evaluates once without caching.
    pub fn eval_once(&mut self, port: PortRef) -> crate::Result<NodeResult> {
        match self.renderer.composition.graph.nodes.get(&port.id) {
            Some(node) => node.evaluate(self, &port.port),
            None => Err(todo!("create error type")),
        }
    }

    pub fn expr_context(&self, path: String) -> ExpressionContext {
        ExpressionContext {
            frame: self.frame,
            fps: self.renderer.composition.timeline.fps,
            width: self.renderer.composition.render_settings.width,
            height: self.renderer.composition.render_settings.height,
            duration_frames: self.renderer.composition.timeline.duration_frames,
            path: Some(path),
        }
    }
}
