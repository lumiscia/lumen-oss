//! The composition a node is evaluated in, and how traversal moves between frames and
//! compositions.
//!
//! Compiling, binding, plan keys and media prediction all read a node's inputs through
//! [`route`], so time remaps, switches and clips route frames the same way everywhere.

use std::{collections::BTreeMap, sync::Arc};

use crate::{
    composition::{Composition, CompositionTemplate, RenderSettings, TimelineSettings},
    error::GraphValidationError,
    expr::{ExpressionContext, ExpressionValue},
    graph::Graph,
    media::MediaStore,
    node::{
        NodeId, NodeKind, NodeParamEvalContext, NodeParams, PortRef,
        processing::time_remap::{TimeRemap, TimeRemapSettings, remap_frame},
    },
};

/// The props a template instance was given, by name.
pub type PropValues = BTreeMap<String, ExpressionValue>;

/// The root composition or a template instance: the graph a node id is looked up in, and the
/// timeline, canvas and props its expressions see.
#[derive(Debug, Clone)]
pub struct Scope<'a> {
    pub root: &'a Composition,
    pub graph: &'a Graph,
    pub timeline: &'a TimelineSettings,
    pub render_settings: &'a RenderSettings,
    /// `None` at the root.
    pub props: Option<Arc<PropValues>>,
}

impl<'a> Scope<'a> {
    pub fn root(composition: &'a Composition) -> Self {
        Self {
            root: composition,
            graph: &composition.graph,
            timeline: &composition.timeline,
            render_settings: &composition.render_settings,
            props: None,
        }
    }

    pub fn template(
        root: &'a Composition,
        template: &'a CompositionTemplate,
        props: PropValues,
    ) -> Self {
        Self {
            root,
            graph: &template.graph,
            timeline: &template.timeline,
            render_settings: &template.render_settings,
            props: Some(Arc::new(props)),
        }
    }

    pub fn node(&self, id: NodeId) -> Option<&'a NodeKind> {
        self.graph.nodes.get(&id)
    }

    /// The scope's only `media_output` node.
    pub fn media_output(&self) -> crate::Result<NodeId> {
        let mut outputs = self.graph.nodes.iter().filter_map(|(node_id, node)| {
            matches!(node, NodeKind::MediaOutput(_)).then_some(*node_id)
        });
        let Some(output) = outputs.next() else {
            return Err(GraphValidationError::MissingMediaOutput.into());
        };
        if outputs.next().is_some() {
            return Err(GraphValidationError::MultipleMediaOutputs { count: 2 }.into());
        }
        Ok(output)
    }

    /// The context expressions of this scope are evaluated in at `frame`.
    pub fn expr_context<'s>(
        &'s self,
        frame: f64,
        path: String,
        media: Option<&'s dyn MediaStore>,
    ) -> ExpressionContext<'s> {
        ExpressionContext {
            frame,
            fps: self.timeline.fps,
            width: self.render_settings.width,
            height: self.render_settings.height,
            duration_frames: self.timeline.duration_frames,
            path: Some(path),
            graph: Some(self.graph),
            media,
            glyph: None,
            props: self.props.as_deref(),
        }
    }

    /// The graph the nodes reached through `path` from the root belong to, or `None` when the
    /// path no longer names time remaps and clips.
    pub fn graph_at(root: &'a Composition, path: &[NodeId]) -> Option<&'a Graph> {
        path.iter()
            .try_fold(&root.graph, |graph, id| match graph.nodes.get(id)? {
                NodeKind::TimeRemap(_) => Some(graph),
                NodeKind::Clip(clip) => match clip.template_name() {
                    Some(name) => root.compositions.get(name).map(|template| &template.graph),
                    None => Some(graph),
                },
                _ => None,
            })
    }
}

/// An input a node reads, with the frame and scope it is read in.
#[derive(Debug, Clone)]
pub struct Routed<'a> {
    pub port: PortRef,
    pub frame: f64,
    pub scope: Scope<'a>,
    /// The time remap or clip that instances the input, which then compiles once per path of
    /// them from the root.
    pub instance: Option<NodeId>,
}

impl<'a> Routed<'a> {
    fn same(scope: &Scope<'a>, port: PortRef, frame: f64) -> Self {
        Self {
            port,
            frame,
            scope: scope.clone(),
            instance: None,
        }
    }
}

/// The inputs `node` of `scope` reads at `frame`: a time remap's source at the remapped frame,
/// a switch's selected layer, a clip's content in its own time while it shows, and every other
/// node's inputs as they are.
pub fn route<'a>(
    scope: &Scope<'a>,
    node: &NodeKind,
    frame: f64,
    media: Option<&dyn MediaStore>,
) -> crate::Result<Vec<Routed<'a>>> {
    Ok(match node {
        NodeKind::TimeRemap(node) => vec![route_time_remap(scope, node, frame, media)?],
        NodeKind::Switch(node) => {
            let ctx = scope.expr_context(frame, format!("{}.params", node.id), media);
            crate::node::compositing::switch::selected_layer_for_frame(node, &ctx)?
                .and_then(|index| node.layers.get(index))
                .map(|port| vec![Routed::same(scope, port.clone(), frame)])
                .unwrap_or_default()
        }
        NodeKind::Clip(node) => node.route(scope, frame, media)?.into_iter().collect(),
        _ => node
            .input_ports()
            .into_iter()
            .map(|port| Routed::same(scope, port, frame))
            .collect(),
    })
}

/// `frame`, or the whole frame it is a rounding error away from. Converting a whole frame to
/// another frame rate and back, as a template at another rate does, can land just below it,
/// where flooring would drop to the frame before.
pub(crate) fn snap_to_whole_frame(frame: f64) -> f64 {
    let whole = frame.round();
    if (frame - whole).abs() <= 1e-9 {
        whole
    } else {
        frame
    }
}

/// A time remap's source at the frame it maps `frame` to.
pub(crate) fn route_time_remap<'a>(
    scope: &Scope<'a>,
    node: &TimeRemap,
    frame: f64,
    media: Option<&dyn MediaStore>,
) -> crate::Result<Routed<'a>> {
    let params = node.params.eval(&NodeParamEvalContext {
        node_id: node.id,
        expr: &scope.expr_context(frame, format!("{}.params", node.id), media),
    })?;
    let frame = remap_frame(TimeRemapSettings {
        frame: params.frame,
        loop_enabled: params.loop_enabled,
        loop_start: params.loop_start,
        loop_end: params.loop_end,
    });
    Ok(Routed {
        instance: Some(node.id),
        ..Routed::same(scope, node.source.clone(), frame)
    })
}
