//! `instance_2d`: many placed copies of one drawing.

use serde_json::{Map, Value};

use super::Transform2d;
use crate::error::{ExpressionError, LumenError};
use crate::expr::{ExpressionContext, ExpressionValue, InstanceEvaluator, InstanceScope};
use crate::json::{StringParam, classify_string, try_parse_color};
use crate::node::vector::drawable::DrawableTransform;
use crate::node::{
    Deferred, Delegated, InstanceCount, MAX_INSTANCES, NodeId, NodeParamType, NodeParams,
    OutputRef, ParamTypeDef, ParamValue, PropertyKind,
};

/// How many `instance_2d` nodes may hold one another.
pub(super) const MAX_NESTING: u32 = 8;

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct Instance2dParams {
    /// Copies to place, from 0 to 100000. 0 draws nothing. Not set together with `copies`.
    #[meta(min = 0, enforced_min, max = 100000, step = 1)]
    pub count: InstanceCount,
    #[meta(
        flatten,
        doc(
            position = "Origin in pixels of each copy, in the drawing's own pixels. Evaluated for every copy, and can read `instance` and `instance_count`.",
            scale = "Scale multiplier per axis of each copy, about its anchor. Evaluated for every copy, and can read `instance` and `instance_count`.",
            rotation = "Clockwise rotation in degrees of each copy, about its anchor. Evaluated for every copy, and can read `instance` and `instance_count`.",
            anchor = "Pivot for each copy's scale and rotation, in pixels relative to its `position`. Evaluated for every copy, and can read `instance` and `instance_count`."
        )
    )]
    pub transform: Transform2d,
    /// Opacity multiplier of each copy. Evaluated for every copy, and can read `instance` and
    /// `instance_count`. Opacities of nested `instance_2d` nodes multiply.
    #[meta(min = 0, max = 1, step = 0.01)]
    pub opacity: f64,
    /// Colour that multiplies the paint of every fill and stroke in each copy, gradients
    /// included. White leaves a copy as it is. Evaluated for every copy, and can read `instance`
    /// and `instance_count`. Its alpha is ignored, and shadows keep their own colour. Tints of
    /// nested `instance_2d` nodes multiply.
    #[meta()]
    pub tint: [u8; 4],
    /// Copies to place, each given literally as `{ position, scale, rotation, anchor, opacity,
    /// tint }` with any field left out taking the default of the param of that name. The list
    /// can't hold expressions and holds at most 100000 copies. An empty list places none of its
    /// own. When it has copies, `count`, `position`, `scale`, `rotation`, `anchor`, `opacity` and
    /// `tint` must not be set, and `check` fails if they are.
    #[meta()]
    pub copies: Instance2dCopies,
}

impl Default for Instance2dParams {
    fn default() -> Self {
        Self {
            count: InstanceCount(1),
            transform: Transform2d::default(),
            opacity: 1.0,
            tint: [255; 4],
            copies: Instance2dCopies::default(),
        }
    }
}

/// Draws its input drawing `count` times, each copy placed by `position`, `scale`, `rotation` and
/// `anchor` evaluated for that copy, where `instance` is the copy's index from 0 and
/// `instance_count` is `count`. Copies are drawn in index order, the first lowest, each placed
/// over the drawing's own pixels as a `merge_2d` with those params would place it, strokes and
/// shadows included. A grid of 10 columns 40 pixels apart:
///
/// ```json
/// { "id": 1, "type": "shape_2d", "params": { "geometry_kind": "ellipse", "width": 24, "height": 24 } },
/// { "id": 2, "type": "fill_2d", "params": { "paint": "#ee5b24" }, "in": { "geometry": 1 } },
/// { "id": 3, "type": "instance_2d", "params": { "count": 50, "position": "=vec2((instance % 10) * 40, floor(instance / 10) * 40)" }, "in": { "drawing": 2 } },
/// { "id": 4, "type": "render_2d", "in": { "drawing": 3 } }
/// ```
///
/// A ring follows from `sin` and `cos` of `instance / instance_count`: `position =
/// vec2(960 + 300 * cos(6.283 * instance / instance_count), 540 + 300 * sin(6.283 * instance /
/// instance_count))` with `rotation = 360 * instance / instance_count` turns each copy to face
/// along the ring. `opacity` fades copies and `tint` colours them: `tint = rgb(255 * instance /
/// instance_count, 128, 255)` shifts the colour along the copies.
///
/// Copies placed by hand are listed in `copies` instead, as literals: `copies = [{ "position":
/// [0, 0] }, { "position": [80, 0], "rotation": 45, "tint": "#ff8040" }]` places two. `count`,
/// `position`, `scale`, `rotation`, `anchor`, `opacity` and `tint` must not be set beside a
/// `copies` list that has copies, or `check` fails.
///
/// The input drawing is walked once, and its outlines are flattened and measured once for every
/// copy that turns and scales them alike, so copies that only move cost a drawable each. A copy's
/// scale scales the softness and spread of the shadows inside it. An `instance_2d` may hold
/// another, which multiplies their counts, up to 8 deep. A drawing holds at most 16384
/// drawables in all, counting each copy's, and a count that would exceed this fails the render.
/// The node is static when its params don't read the frame and its drawing is static.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "instance_2d",
    name = "Instance 2D",
    category = "vector",
    output = "drawing"
)]
pub struct Instance2d {
    pub id: NodeId,
    #[params]
    pub params: Instance2dParamsDelegate,

    #[input(drawing)]
    pub drawing: Option<OutputRef>,
}

/// Params a non-empty `copies` list replaces.
const PER_COPY: [&str; 7] = [
    "count", "position", "scale", "rotation", "anchor", "opacity", "tint",
];

impl Instance2d {
    /// What is wrong with a node whose JSON `params` set `copies` beside params that it replaces,
    /// if anything is.
    pub(crate) fn copies_conflict(params: &Map<String, Value>) -> Option<String> {
        let has_copies = params
            .get("copies")
            .and_then(Value::as_array)
            .is_some_and(|copies| !copies.is_empty());
        if !has_copies {
            return None;
        }
        let set: Vec<String> = PER_COPY
            .into_iter()
            .filter(|name| params.contains_key(*name))
            .map(|name| format!("`{name}`"))
            .collect();
        (!set.is_empty()).then(|| {
            format!(
                "`copies` places the copies itself, so it can't be set together with {}",
                set.join(", ")
            )
        })
    }

    /// The literal list it places its copies from, if it has any copies in it.
    fn literal_copies(&self) -> Option<&Instance2dCopies> {
        match &self.params.copies {
            Deferred::Value(copies) if !copies.0.is_empty() => Some(copies),
            _ => None,
        }
    }

    /// How many copies it places, at `expr`.
    pub(super) fn count(&self, expr: &ExpressionContext<'_>) -> crate::Result<u32> {
        match self.literal_copies() {
            Some(copies) => Ok(copies.0.len() as u32),
            None => Ok(self.params.count.eval(self.id, "count", expr)?.0),
        }
    }

    /// The placement and tint of each of its `count` copies, at `expr`.
    pub(super) fn copies(
        &self,
        expr: &ExpressionContext<'_>,
        count: u32,
    ) -> crate::Result<Vec<(DrawableTransform, [u8; 4])>> {
        if let Some(copies) = self.literal_copies() {
            return Ok(copies.0.iter().map(Instance2dCopy::placed).collect());
        }
        let params = &self.params;
        let transform = &params.transform;
        let expressions = InstanceEvaluator::new(
            [
                expression(&transform.position),
                expression(&transform.scale),
                expression(&transform.rotation),
                expression(&transform.anchor),
                expression(&params.opacity),
                expression(&params.tint),
            ]
            .into_iter()
            .flatten(),
        );
        let mut copy = PerCopy {
            node_id: self.id,
            expressions,
            next: 0,
            index: 0,
        };
        let mut scoped = *expr;
        (0..count)
            .map(|index| {
                scoped.instance = Some(InstanceScope { index, count });
                copy.next = 0;
                copy.index = index;
                let placed = DrawableTransform {
                    position: copy.value(&scoped, "position", &transform.position)?,
                    scale: copy.value(&scoped, "scale", &transform.scale)?,
                    rotation: copy.value(&scoped, "rotation", &transform.rotation)?,
                    anchor: copy.value(&scoped, "anchor", &transform.anchor)?,
                    opacity: copy.value(&scoped, "opacity", &params.opacity)?,
                };
                Ok((placed, copy.value(&scoped, "tint", &params.tint)?))
            })
            .collect()
    }
}

fn expression<T>(param: &Deferred<T>) -> Option<&crate::expr::Expression> {
    match param {
        Deferred::Expr(expression) => Some(expression),
        Deferred::Value(_) => None,
    }
}

/// Evaluates the params of one copy, its expressions in the order they were given to
/// `expressions`.
struct PerCopy {
    node_id: NodeId,
    expressions: InstanceEvaluator,
    /// The next expression to evaluate.
    next: usize,
    index: u32,
}

impl PerCopy {
    fn value<T: ParamValue>(
        &mut self,
        scoped: &ExpressionContext<'_>,
        path: &str,
        param: &Deferred<T>,
    ) -> crate::Result<T> {
        let Deferred::Expr(_) = param else {
            return param.eval(self.node_id, path, scoped);
        };
        self.next += 1;
        self.expressions
            .evaluate(self.next - 1, scoped)
            .and_then(T::from_expression)
            .map_err(|error| at_instance(error.located(self.node_id, path), self.index))
    }
}

/// `error`, if it locates an expression failure, saying which instance it happened in.
fn at_instance(error: LumenError, index: u32) -> LumenError {
    match error {
        LumenError::Expression(ExpressionError::Located { at, failure }) => {
            LumenError::Expression(ExpressionError::Located {
                at,
                failure: Box::new(ExpressionError::Evaluate {
                    details: format!("instance {index}: {failure}"),
                }),
            })
        }
        error => error,
    }
}

/// One copy of an `instance_2d`'s `copies` list.
#[derive(Debug, Clone, PartialEq, lumen_macros::StructuralHash)]
pub struct Instance2dCopy {
    pub position: (f64, f64),
    pub scale: (f64, f64),
    pub rotation: f64,
    pub anchor: (f64, f64),
    pub opacity: f64,
    /// The tint, whose alpha is ignored.
    pub tint: [u8; 4],
}

impl Default for Instance2dCopy {
    fn default() -> Self {
        Self {
            position: (0.0, 0.0),
            scale: (1.0, 1.0),
            rotation: 0.0,
            anchor: (0.0, 0.0),
            opacity: 1.0,
            tint: [255; 4],
        }
    }
}

impl Instance2dCopy {
    fn placed(&self) -> (DrawableTransform, [u8; 4]) {
        let placed = DrawableTransform {
            position: self.position,
            scale: self.scale,
            rotation: self.rotation,
            anchor: self.anchor,
            opacity: self.opacity,
        };
        (placed, self.tint)
    }

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        let fields = value
            .as_object()
            .ok_or_else(|| format!("`{path}` must be an object, got {value}"))?;
        let mut copy = Self::default();
        for (name, value) in fields {
            let at = format!("{path}.{name}");
            let literal = |value: &Value| -> Result<(), String> {
                match value.as_str().and_then(classify_string) {
                    Some(StringParam::Expression(_)) => Err(format!(
                        "`{at}`: the copies are literal, so an expression can't go here"
                    )),
                    _ => Ok(()),
                }
            };
            match name.as_str() {
                "position" | "scale" | "anchor" => {
                    if let Some(items) = value.as_array() {
                        items.iter().try_for_each(literal)?;
                    }
                    let vector = <(f64, f64)>::from_json_value(value, &at)?;
                    match name.as_str() {
                        "position" => copy.position = vector,
                        "scale" => copy.scale = vector,
                        _ => copy.anchor = vector,
                    }
                }
                "rotation" | "opacity" => {
                    literal(value)?;
                    let number = value
                        .as_f64()
                        .ok_or_else(|| format!("`{at}` must be a number, got {value}"))?;
                    if name == "rotation" {
                        copy.rotation = number;
                    } else {
                        copy.opacity = number;
                    }
                }
                "tint" => {
                    literal(value)?;
                    if value.is_object() {
                        return Err(format!("`{at}` must be a solid colour, not a gradient"));
                    }
                    copy.tint =
                        try_parse_color(value).map_err(|error| format!("`{at}`: {error}"))?;
                }
                _ => {
                    return Err(format!(
                        "`{at}` isn't a copy field; a copy takes `position`, `scale`, `rotation`, `anchor`, `opacity` and `tint`"
                    ));
                }
            }
        }
        Ok(copy)
    }

    fn to_json_value(&self) -> Value {
        let mut fields = Map::new();
        fields.insert("position".into(), self.position.to_json_value());
        fields.insert("scale".into(), self.scale.to_json_value());
        fields.insert("rotation".into(), Value::from(self.rotation));
        fields.insert("anchor".into(), self.anchor.to_json_value());
        fields.insert("opacity".into(), Value::from(self.opacity));
        fields.insert("tint".into(), Value::from(self.tint.to_vec()));
        Value::Object(fields)
    }
}

/// The copies an `instance_2d` places, from a literal list. An empty list places none of its own.
#[derive(Debug, Clone, Default, PartialEq, lumen_macros::StructuralHash)]
pub struct Instance2dCopies(pub Vec<Instance2dCopy>);

pub(crate) static INSTANCE_2D_COPIES: ParamTypeDef = ParamTypeDef {
    name: "instance_2d_copies",
    #[cfg(feature = "metadata")]
    schema: || serde_json::json!({ "$ref": "#/$defs/instance2dCopies" }),
};

impl ParamValue for Instance2dCopies {
    const ACCEPTS_EXPRESSIONS: bool = false;

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        let items = value
            .as_array()
            .ok_or_else(|| format!("`{path}` must be an array of copies, got {value}"))?;
        if items.len() > MAX_INSTANCES as usize {
            return Err(format!(
                "`{path}` has {} copies, more than {MAX_INSTANCES}",
                items.len()
            ));
        }
        items
            .iter()
            .enumerate()
            .map(|(index, item)| Instance2dCopy::from_json_value(item, &format!("{path}[{index}]")))
            .collect::<Result<_, _>>()
            .map(Self)
    }

    fn to_json_value(&self) -> Value {
        self.0.iter().map(Instance2dCopy::to_json_value).collect()
    }

    fn from_expression(_value: ExpressionValue) -> crate::Result<Self> {
        Err(LumenError::evaluate("`copies` can't be an expression"))
    }

    fn to_expression(&self) -> Option<ExpressionValue> {
        None
    }
}

impl Delegated for Instance2dCopies {
    type Delegate = Deferred<Instance2dCopies>;

    fn into_delegate(self) -> Self::Delegate {
        Deferred::value(self)
    }
}

impl NodeParamType for Instance2dCopies {
    fn property_kind() -> PropertyKind {
        PropertyKind::Custom(&INSTANCE_2D_COPIES)
    }
}
