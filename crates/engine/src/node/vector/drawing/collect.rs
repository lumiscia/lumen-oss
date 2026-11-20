//! Walks the nodes a `render_2d` reads into a record of what it draws: each outline once, as its
//! source and the offsets applied to it, and each drawable as an outline, a style and where the
//! merges around it place it.

use foldhash::HashMap;

use super::super::dash::GpuDash;
use super::super::drawable::{DrawableTransform, GpuDrawableTransform};
use super::super::paint::{GpuPaint, Paint};
use super::super::path::{MAX_PATH_COORDINATE, normalized_trim_range};
use super::super::shape::ShapeGeometryKind;
use super::build::{identity, then};
use super::{CastShadow, Fill2d, Offset2d, Render2d, Shadow2d, Stroke2d, Text2dParams};
use crate::error::{At, PropertyError, RenderError};
use crate::expr::ExpressionContext;
use crate::node::{DelegateEvalContext, DelegateEvaluable, JsonNode, NodeId, NodeKind};

/// Most nodes a drawing may reach, counting a node each time it is reached.
const MAX_DRAWING_NODES: usize = 4096;
/// Most drawables a drawing may hold, counting each shadow cast.
pub(super) const MAX_DRAWABLES: usize = 16384;
/// Width of a shadow's fade per sigma of the Gaussian it stands in for, at which the fade's
/// slope across a straight edge matches the Gaussian's: `1.5 * sqrt(2 * pi)`.
const SHADOW_FADE_PER_SIGMA: f64 = 3.7599;

/// What a drawing's outlines are drawn from.
#[derive(Clone)]
pub(crate) enum Source {
    Path {
        node: NodeId,
        data: String,
    },
    Shape {
        kind: ShapeGeometryKind,
        size: [f32; 2],
        border_radius: f32,
    },
    /// The glyphs of a `text_2d`, which its consumer shapes from `params`.
    Text {
        node: NodeId,
        params: Box<Text2dParams>,
    },
}

impl Source {
    /// The node whose path or text outline this is; `None` for a shape.
    pub(super) fn path_node(&self) -> Option<NodeId> {
        match self {
            Self::Path { node, .. } | Self::Text { node, .. } => Some(*node),
            Self::Shape { .. } => None,
        }
    }
}

/// An outline: its source, laid out from `position` and placed by `transform`, grown by
/// `offset` untransformed pixels.
pub(super) struct Geometry {
    pub(super) source: Source,
    pub(super) position: [f32; 2],
    pub(super) transform: GpuDrawableTransform,
    pub(super) offset: f32,
}

/// What a chain of geometry nodes outputs: the outline of its source, placed by `transform`, and
/// the offset that its `offset_2d` nodes grow it by, in untransformed pixels.
pub(crate) struct Outline {
    pub(crate) source: Source,
    /// `position` is where the source is laid out from.
    pub(crate) transform: DrawableTransform,
    pub(crate) offset: f32,
    /// Whether the chain holds an `offset_2d`, whatever its offset.
    #[cfg_attr(not(feature = "3d"), expect(dead_code))]
    pub(crate) offsets: bool,
}

/// The outline the geometry node `node_id` outputs, following `offset_2d` nodes to their source.
/// `node_of` finds each node the walk reaches, along with a value that lives while it is
/// evaluated; a node of another kind fails as `N`.
pub(crate) fn resolve_outline<'e, N: JsonNode, G>(
    expr: &ExpressionContext<'_>,
    node_id: NodeId,
    mut node_of: impl FnMut(NodeId) -> crate::Result<(&'e NodeKind, G)>,
) -> crate::Result<Outline> {
    let mut offset = 0.0_f64;
    let mut offsets = false;
    let mut at = node_id;
    loop {
        let (node, _evaluating) = node_of(at)?;
        let ctx = DelegateEvalContext::new(at, expr);
        let (source, transform) = match node {
            NodeKind::Offset2d(node) => {
                let params = node.params.eval(&ctx)?;
                offset += f64::from(number::<Offset2d>(
                    at,
                    "offset",
                    params.offset,
                    -MAX_PATH_COORDINATE,
                )?);
                offsets = true;
                at = crate::node::required_input(at, "geometry", node.geometry)?;
                continue;
            }
            NodeKind::Path2d(path) => {
                let params = path.params.eval(&ctx)?;
                (
                    Source::Path {
                        node: at,
                        data: params.data,
                    },
                    params.transform.drawable(),
                )
            }
            NodeKind::Shape2d(shape) => {
                let params = shape.params.eval(&ctx)?;
                // `max` also turns NaN to 0, which draws nothing.
                let size = [params.width, params.height].map(|side| side.max(0.0) as f32);
                (
                    Source::Shape {
                        kind: params.geometry_kind,
                        size,
                        border_radius: params.border_radius.max(0.0) as f32,
                    },
                    params.transform.drawable(),
                )
            }
            NodeKind::Text2d(text) => {
                let params = text.params.eval(&ctx)?;
                let transform = params.transform.drawable();
                (
                    Source::Text {
                        node: at,
                        params: Box::new(params),
                    },
                    transform,
                )
            }
            // Validation keeps other kinds out of geometry inputs.
            _ => {
                return Err(RenderError::evaluating::<N>(
                    at,
                    "a geometry input takes path_2d, shape_2d, text_2d and offset_2d nodes",
                )
                .into());
            }
        };
        return Ok(Outline {
            source,
            transform,
            offset: offset.clamp(-MAX_PATH_COORDINATE, MAX_PATH_COORDINATE) as f32,
            offsets,
        });
    }
}

#[derive(Clone, Copy)]
pub(super) struct FillStyle {
    pub(super) feather: f32,
    pub(super) edge_antialias: bool,
}

#[derive(Clone, Copy)]
pub(super) struct StrokeStyle {
    pub(super) width: f32,
    pub(super) trim: (f32, f32),
    pub(super) dash: GpuDash,
}

#[derive(Clone, Copy)]
pub(super) enum Style {
    Fill(FillStyle),
    Stroke(StrokeStyle),
}

/// A shadow cast of a drawable, in canvas pixels.
#[derive(Clone, Copy)]
pub(super) struct Cast {
    pub(super) color: [u8; 4],
    pub(super) opacity: f32,
    pub(super) offset: [f32; 2],
    /// Sigma of the Gaussian the fade stands in for.
    pub(super) sigma: f64,
    pub(super) spread: f64,
}

impl Cast {
    /// Canvas width the shadow's edge fades over.
    pub(super) fn softness(&self) -> f64 {
        SHADOW_FADE_PER_SIGMA * self.sigma
    }

    /// The shadow `outer` casts of this one.
    fn under(self, outer: Cast) -> Cast {
        Cast {
            color: outer.color,
            opacity: self.opacity * outer.opacity,
            offset: [
                self.offset[0] + outer.offset[0],
                self.offset[1] + outer.offset[1],
            ],
            sigma: self.sigma.hypot(outer.sigma),
            spread: self.spread + outer.spread,
        }
    }
}

/// One drawable: an outline in a style, placed by the merges around its style node, or a shadow
/// cast of one.
#[derive(Clone, Copy)]
pub(super) struct Use {
    pub(super) geometry: usize,
    pub(super) parent: GpuDrawableTransform,
    pub(super) style: Style,
    pub(super) paint: GpuPaint,
    pub(super) opacity: f32,
    pub(super) shadow: Option<Cast>,
}

/// A drawing's outlines and its drawables, lowest first.
#[derive(Default)]
pub(super) struct Record {
    pub(super) geometries: Vec<Geometry>,
    pub(super) uses: Vec<Use>,
}

enum Step {
    Draw(NodeId, GpuDrawableTransform),
    /// Casts `node`'s shadows of the drawables from `start` on, under them.
    Cast {
        node: NodeId,
        parent: GpuDrawableTransform,
        start: usize,
    },
}

struct Collector<'a, 'e> {
    cx: &'a crate::gpu::frame_graph::FrameCx<'e>,
    expr: ExpressionContext<'a>,
    render_id: NodeId,
    record: Record,
    geometries: HashMap<NodeId, usize>,
    styles: HashMap<NodeId, (usize, Style, GpuPaint, f32)>,
    reached: usize,
}

/// Evaluates the drawing `root` outputs, walking its nodes depth first in input order so that
/// earlier inputs are drawn lower. Each node is evaluated once, however often it is reached. The
/// drawing is placed under the render scale, so its lengths are in canvas pixels.
pub(super) fn collect(
    cx: &crate::gpu::frame_graph::FrameCx<'_>,
    render_id: NodeId,
    root: NodeId,
) -> crate::Result<Record> {
    let mut collector = Collector {
        cx,
        expr: cx.expr(),
        render_id,
        record: Record::default(),
        geometries: HashMap::default(),
        styles: HashMap::default(),
        reached: 0,
    };
    let placed = match cx.pixel_scale() {
        1.0 => identity(),
        scale => DrawableTransform {
            position: (0.0, 0.0),
            scale: (scale, scale),
            rotation: 0.0,
            anchor: (0.0, 0.0),
            opacity: 1.0,
        }
        .to_gpu(),
    };
    let mut pending = vec![Step::Draw(root, placed)];
    while let Some(step) = pending.pop() {
        match step {
            Step::Draw(node_id, parent) => collector.draw(node_id, parent, &mut pending)?,
            Step::Cast {
                node,
                parent,
                start,
            } => collector.cast(node, parent, start)?,
        }
    }
    Ok(collector.record)
}

fn number<N: JsonNode>(node_id: NodeId, name: &str, value: f64, min: f64) -> crate::Result<f32> {
    if value.is_nan() {
        return Err(
            RenderError::evaluating::<N>(node_id, format!("`{name}` is not a number")).into(),
        );
    }
    Ok(value.clamp(min, MAX_PATH_COORDINATE) as f32)
}

/// The node `node_id`, counted among the `reached` nodes of a drawing.
fn reach_node<'e>(
    cx: &crate::gpu::frame_graph::FrameCx<'e>,
    reached: &mut usize,
    render_id: NodeId,
    node_id: NodeId,
) -> crate::Result<&'e NodeKind> {
    *reached += 1;
    if *reached > MAX_DRAWING_NODES {
        return Err(RenderError::evaluating::<Render2d>(
            render_id,
            format!("the drawing reaches more than {MAX_DRAWING_NODES} nodes"),
        )
        .into());
    }
    cx.node(node_id).ok_or_else(|| {
        RenderError::MissingNode {
            at: At::node(node_id),
        }
        .into()
    })
}

impl<'e> Collector<'_, 'e> {
    fn reach(&mut self, node_id: NodeId) -> crate::Result<&'e NodeKind> {
        reach_node(self.cx, &mut self.reached, self.render_id, node_id)
    }

    fn too_large(&self, details: String) -> crate::error::LumenError {
        RenderError::evaluating::<Render2d>(self.render_id, details).into()
    }

    fn draw(
        &mut self,
        node_id: NodeId,
        parent: GpuDrawableTransform,
        pending: &mut Vec<Step>,
    ) -> crate::Result<()> {
        let node = self.reach(node_id)?;
        let _timed = self.cx.node_times().time(node);
        match node {
            NodeKind::Merge2d(merge) => {
                let params = merge
                    .params
                    .eval(&DelegateEvalContext::new(node_id, &self.expr))?;
                let placed = params.transform.drawable().to_gpu_from((0.0, 0.0));
                let world = then(placed, parent);
                pending.extend(
                    merge
                        .drawings
                        .iter()
                        .rev()
                        .map(|&input| Step::Draw(input, world)),
                );
            }
            NodeKind::Shadow2d(shadow) => {
                let input = crate::node::required_input(node_id, "drawing", shadow.drawing)?;
                pending.push(Step::Cast {
                    node: node_id,
                    parent,
                    start: self.record.uses.len(),
                });
                pending.push(Step::Draw(input, parent));
            }
            NodeKind::Fill2d(_) | NodeKind::Stroke2d(_) => {
                let (geometry, style, paint, opacity) = self.style(node_id)?;
                self.push(Use {
                    geometry,
                    parent,
                    style,
                    paint,
                    opacity,
                    shadow: None,
                })?;
            }
            // Validation keeps other kinds out of drawing inputs.
            _ => {
                return Err(RenderError::evaluating::<Render2d>(
                    node_id,
                    "a drawing input takes fill_2d, stroke_2d, shadow_2d and merge_2d nodes",
                )
                .into());
            }
        }
        Ok(())
    }

    fn push(&mut self, drawable: Use) -> crate::Result<()> {
        if self.record.uses.len() >= MAX_DRAWABLES {
            return Err(self.too_large(format!(
                "the drawing holds more than {MAX_DRAWABLES} drawables"
            )));
        }
        self.record.uses.push(drawable);
        Ok(())
    }

    /// The outline, style, paint and opacity of fill or stroke node `node_id`.
    fn style(&mut self, node_id: NodeId) -> crate::Result<(usize, Style, GpuPaint, f32)> {
        if let Some(style) = self.styles.get(&node_id) {
            return Ok(*style);
        }
        let ctx = DelegateEvalContext::new(node_id, &self.expr);
        let (input, style, paint, opacity) = match self.cx.node(node_id) {
            Some(NodeKind::Fill2d(fill)) => {
                let params = fill.params.eval(&ctx)?;
                let style = Style::Fill(FillStyle {
                    feather: number::<Fill2d>(node_id, "feather", params.feather, 0.0)?,
                    edge_antialias: params.edge_antialias,
                });
                (fill.geometry, style, params.paint, params.opacity)
            }
            Some(NodeKind::Stroke2d(stroke)) => {
                let params = stroke.params.eval(&ctx)?;
                let dash =
                    GpuDash::named(&params.dash, params.dash_offset, ["dash", "dash_offset"])
                        .map_err(|details| RenderError::evaluating::<Stroke2d>(node_id, details))?;
                let style = Style::Stroke(StrokeStyle {
                    width: number::<Stroke2d>(node_id, "width", params.width, 0.0)?,
                    trim: normalized_trim_range(params.trim_start, params.trim_end),
                    dash,
                });
                (stroke.geometry, style, params.paint, params.opacity)
            }
            _ => unreachable!("only fill and stroke nodes have a style"),
        };
        let input = crate::node::required_input(node_id, "geometry", input)?;
        let geometry = self.geometry(input)?;
        let styled = (
            geometry,
            style,
            paint.to_gpu(),
            opacity.clamp(0.0, 1.0) as f32,
        );
        self.styles.insert(node_id, styled);
        Ok(styled)
    }

    /// The index of the outline `node_id` outputs, following offsets to their source.
    fn geometry(&mut self, node_id: NodeId) -> crate::Result<usize> {
        if let Some(&index) = self.geometries.get(&node_id) {
            return Ok(index);
        }
        let (cx, reached, render_id) = (self.cx, &mut self.reached, self.render_id);
        let outline = resolve_outline::<Render2d, _>(&self.expr, node_id, |at| {
            let node = reach_node(cx, reached, render_id, at)?;
            Ok((node, cx.node_times().time(node)))
        })?;
        let (x, y) = outline.transform.position;
        let index = self.record.geometries.len();
        self.record.geometries.push(Geometry {
            source: outline.source,
            position: [x as f32, y as f32],
            transform: outline.transform.to_gpu(),
            offset: outline.offset,
        });
        self.geometries.insert(node_id, index);
        Ok(index)
    }

    /// Casts shadow node `node_id`'s shadows of the drawables from `start` on, beneath them.
    fn cast(
        &mut self,
        node_id: NodeId,
        parent: GpuDrawableTransform,
        start: usize,
    ) -> crate::Result<()> {
        let Some(shadow @ NodeKind::Shadow2d(node)) = self.cx.node(node_id) else {
            unreachable!("only shadow nodes cast shadows")
        };
        let _timed = self.cx.node_times().time(shadow);
        let params = node
            .params
            .eval(&DelegateEvalContext::new(node_id, &self.expr))?;
        let mut casts = Vec::with_capacity(params.shadows.0.len());
        for (index, shadow) in params.shadows.0.iter().enumerate() {
            if let Some(cast) = canvas_cast(node_id, index, shadow, &parent, self.cx.pixel_scale())?
            {
                casts.push(cast);
            }
        }
        let drawn = self.record.uses.len() - start;
        if self.record.uses.len() + casts.len() * drawn > MAX_DRAWABLES {
            return Err(self.too_large(format!(
                "the drawing holds more than {MAX_DRAWABLES} drawables"
            )));
        }
        let cast_uses: Vec<Use> = casts
            .iter()
            .flat_map(|&cast| {
                self.record.uses[start..].iter().map(move |drawable| Use {
                    shadow: Some(drawable.shadow.map_or(cast, |inner| inner.under(cast))),
                    ..*drawable
                })
            })
            .collect();
        self.record.uses.splice(start..start, cast_uses);
        Ok(())
    }
}

/// `shadow`, whose lengths are in the pixels `parent` maps to the device pixels of
/// `pixel_scale`, in device pixels. `None` when it is invisible. The radius softens as it would
/// in canvas pixels, scaled to device pixels.
fn canvas_cast(
    node_id: NodeId,
    index: usize,
    shadow: &CastShadow,
    parent: &GpuDrawableTransform,
    pixel_scale: f64,
) -> crate::Result<Option<Cast>> {
    let Paint::SolidColor(r, g, b, a) = shadow.color else {
        return Err(PropertyError::InvalidType {
            at: At::param(node_id, format!("shadows[{index}].color")),
            expected: "a solid color",
            actual: "a gradient",
        }
        .into());
    };
    let opacity = shadow.opacity.clamp(0.0, 1.0) as f32 * f32::from(a) / 255.0;
    if opacity <= 0.0 {
        return Ok(None);
    }
    let [column_x, column_y] = parent.linear.map(|column| column.map(f64::from));
    // Lengths scale by the parent's area scale; offsets map through it.
    let length = (column_x[0] * column_y[1] - column_x[1] * column_y[0])
        .abs()
        .sqrt();
    let radius = f64::from(number::<Shadow2d>(
        node_id,
        "shadows.radius",
        shadow.radius,
        0.0,
    )?) * (length / pixel_scale);
    let spread = f64::from(number::<Shadow2d>(
        node_id,
        "shadows.spread",
        shadow.spread,
        -MAX_PATH_COORDINATE,
    )?) * length;
    let [x, y] = [shadow.offset.0, shadow.offset.1]
        .map(|value| value.clamp(-MAX_PATH_COORDINATE, MAX_PATH_COORDINATE));
    let offset = [
        column_x[0] * x + column_y[0] * y,
        column_x[1] * x + column_y[1] * y,
    ];
    Ok(Some(Cast {
        color: [r, g, b, 255],
        opacity,
        offset: offset.map(|value| value as f32),
        sigma: (radius * (radius + 1.0) / 3.0).sqrt() * pixel_scale,
        spread,
    }))
}
