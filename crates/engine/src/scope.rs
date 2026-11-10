//! The composition a node is evaluated in, and how traversal moves between frames and
//! compositions.
//!
//! Recording and media prediction read a node's inputs through [`route`] and what it calls, so
//! time remaps, switches and clips route frames the same way everywhere.

use std::{collections::BTreeMap, sync::Arc};

use crate::{
    composition::{Composition, CompositionTemplate, Scene},
    expr::{ExpressionContext, ExpressionValue},
    media::MediaStore,
    node::{
        DelegateEvalContext, DelegateEvaluable, Node, NodeId, NodeKind,
        processing::{
            motion_blur::MotionBlur,
            time_remap::{TimeRemap, remap_frame},
        },
        required_input,
    },
};

/// A point in a composition's time, in its frames, as `num / den`.
///
/// A clip converts time between frame rates by multiplying numerators and denominators, so
/// whole frames stay exact through any depth of nested clips and are divided once, where the
/// time is used.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameTime {
    num: f64,
    den: f64,
}

impl FrameTime {
    /// `f64` represents integers up to this exactly.
    const EXACT_LIMIT: f64 = 9_007_199_254_740_992.0;

    /// `num / den`. Parts too large to stay exact are divided out.
    pub fn new(num: f64, den: f64) -> Self {
        if num.abs() <= Self::EXACT_LIMIT && den <= Self::EXACT_LIMIT {
            Self { num, den }
        } else {
            Self::from(num / den)
        }
    }

    pub fn num(self) -> f64 {
        self.num
    }

    pub fn den(self) -> f64 {
        self.den
    }

    /// The time in frames.
    pub fn value(self) -> f64 {
        self.num / self.den
    }

    /// This time moved by `frames`.
    pub fn offset(self, frames: f64) -> Self {
        Self::new(self.num + frames * self.den, self.den)
    }
}

impl From<f64> for FrameTime {
    fn from(frame: f64) -> Self {
        Self {
            num: frame,
            den: 1.0,
        }
    }
}

/// The props a template instance was given, by name.
pub type PropValues = BTreeMap<String, ExpressionValue>;

/// The root composition or a template instance: the graph a node id is looked up in, and the
/// timeline, canvas and props its expressions see.
#[derive(Debug, Clone)]
pub struct Scope<'a> {
    pub root: &'a Composition,
    pub scene: &'a Scene,
    /// The graph's `media_output` node.
    pub output: NodeId,
    /// `None` at the root.
    pub props: Option<Arc<PropValues>>,
}

impl<'a> Scope<'a> {
    pub fn root(composition: &'a Composition) -> Self {
        Self {
            root: composition,
            scene: composition.scene(),
            output: composition.output(),
            props: None,
        }
    }

    pub fn template(
        root: &'a Composition,
        template: &'a CompositionTemplate,
        output: NodeId,
        props: PropValues,
    ) -> Self {
        Self {
            root,
            scene: &template.scene,
            output,
            props: Some(Arc::new(props)),
        }
    }

    pub fn node(&self, id: NodeId) -> Option<&'a NodeKind> {
        self.scene.graph.nodes.get(&id)
    }

    /// The context expressions of this scope are evaluated in at `frame`.
    pub fn expr_context<'s>(
        &'s self,
        frame: FrameTime,
        media: Option<&'s dyn MediaStore>,
    ) -> ExpressionContext<'s> {
        ExpressionContext {
            frame: frame.value(),
            fps: self.scene.timeline.fps,
            width: self.scene.render_settings.width,
            height: self.scene.render_settings.height,
            graph: Some(&self.scene.graph),
            media,
            glyph: None,
            props: self.props.as_deref(),
        }
    }
}

/// An input a node reads, with the frame and scope it is read in.
#[derive(Debug, Clone)]
pub struct Routed<'a> {
    pub node: NodeId,
    pub frame: FrameTime,
    pub scope: Scope<'a>,
    /// The time remap or clip that instances the input, which then records once per path of
    /// them from the root.
    pub instance: Option<NodeId>,
}

impl<'a> Routed<'a> {
    fn same(scope: &Scope<'a>, node: NodeId, frame: FrameTime) -> Self {
        Self {
            node,
            frame,
            scope: scope.clone(),
            instance: None,
        }
    }
}

/// The inputs `node` of `scope` reads at `frame`: a time remap's source at the remapped frame,
/// a switch's selected layer, a clip's content in its own time while it shows, a `motion_blur`'s
/// source at each frame it samples, and every other node's inputs as they are. A `motion_blur`
/// inside one that samples renders its source once, which `sampling` says.
pub fn route<'a>(
    scope: &Scope<'a>,
    node: &NodeKind,
    frame: FrameTime,
    media: Option<&dyn MediaStore>,
    sampling: bool,
) -> crate::Result<Vec<Routed<'a>>> {
    Ok(match node {
        NodeKind::TimeRemap(node) => vec![route_time_remap(scope, node, frame, media)?],
        NodeKind::MotionBlur(node) if !sampling => route_motion_blur(scope, node, frame, media)?,
        NodeKind::Switch(node) => {
            let ctx = scope.expr_context(frame, media);
            crate::node::compositing::switch::selected_layer_for_frame(node, &ctx)?
                .and_then(|index| node.layers.get(index))
                .map(|&layer| vec![Routed::same(scope, layer, frame)])
                .unwrap_or_default()
        }
        NodeKind::Clip(node) => node.route(scope, frame, media)?.into_iter().collect(),
        _ => node
            .input_ports()
            .into_iter()
            .flatten()
            .map(|input| Routed::same(scope, input, frame))
            .collect(),
    })
}

/// A `motion_blur`'s source at each frame it samples, or at `frame` when it renders it once.
fn route_motion_blur<'a>(
    scope: &Scope<'a>,
    node: &MotionBlur,
    frame: FrameTime,
    media: Option<&dyn MediaStore>,
) -> crate::Result<Vec<Routed<'a>>> {
    let source = required_input(node.id, "source", node.source)?;
    Ok(
        match node.sample_offsets(&scope.expr_context(frame, media))? {
            None => vec![Routed::same(scope, source, frame)],
            Some(offsets) => offsets
                .into_iter()
                .map(|offset| Routed {
                    instance: Some(node.id),
                    ..Routed::same(scope, source, frame.offset(offset))
                })
                .collect(),
        },
    )
}

/// A time remap's source at the frame it maps `frame` to.
pub(crate) fn route_time_remap<'a>(
    scope: &Scope<'a>,
    node: &TimeRemap,
    frame: FrameTime,
    media: Option<&dyn MediaStore>,
) -> crate::Result<Routed<'a>> {
    let params = node.params.eval(&DelegateEvalContext {
        node_id: node.id,
        expr: &scope.expr_context(frame, media),
    })?;
    let frame = remap_frame(node.id, &params)?;
    Ok(Routed {
        instance: Some(node.id),
        ..Routed::same(
            scope,
            required_input(node.id, "source", node.source)?,
            frame.into(),
        )
    })
}
