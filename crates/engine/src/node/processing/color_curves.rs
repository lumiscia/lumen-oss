use serde_json::{Map, Value};

use crate::error::LumenError;
use crate::expr::Expression;
use crate::node::{
    Deferred, DelegateEvalContext, DelegateEvaluable, Delegated, ParamTypeDef, PropertyRead,
    field_path, nested_fields,
};

/// Most points one curve may hold.
pub const MAX_CURVE_POINTS: usize = 16;

/// Tone curves over sRGB-encoded colour: one for all channels and one per channel. A curve is
/// its `[x, y]` points, each in `0..=1` with `x` increasing; none is the identity.
#[derive(Debug, Clone, Default, PartialEq, lumen_macros::StructuralHash)]
pub struct ColorCurves {
    pub master: Vec<[f64; 2]>,
    pub red: Vec<[f64; 2]>,
    pub green: Vec<[f64; 2]>,
    pub blue: Vec<[f64; 2]>,
}

pub(crate) static COLOR_CURVES: ParamTypeDef = ParamTypeDef {
    name: "color_curves",
    #[cfg(feature = "metadata")]
    schema: || serde_json::json!({ "$ref": "#/$defs/colorCurves" }),
};

/// [`ColorCurves`] whose numbers take literals or expressions. The curves themselves, and the
/// whole value, cannot be expressions.
#[derive(Debug, Clone, Default, lumen_macros::StructuralHash)]
pub struct ColorCurvesDelegate {
    master: Vec<[Deferred<f64>; 2]>,
    red: Vec<[Deferred<f64>; 2]>,
    green: Vec<[Deferred<f64>; 2]>,
    blue: Vec<[Deferred<f64>; 2]>,
}

const CURVE_NAMES: [&str; 4] = ["master", "red", "green", "blue"];

impl Delegated for ColorCurves {
    type Delegate = ColorCurvesDelegate;

    fn into_delegate(self) -> Self::Delegate {
        let literal = |points: Vec<[f64; 2]>| {
            points
                .into_iter()
                .map(|point| point.map(Deferred::Value))
                .collect()
        };
        ColorCurvesDelegate {
            master: literal(self.master),
            red: literal(self.red),
            green: literal(self.green),
            blue: literal(self.blue),
        }
    }
}

impl ColorCurvesDelegate {
    fn curves(&self) -> [&Vec<[Deferred<f64>; 2]>; 4] {
        [&self.master, &self.red, &self.green, &self.blue]
    }
}

impl DelegateEvaluable for ColorCurvesDelegate {
    type Evaluated = ColorCurves;

    const ACCEPTS_EXPRESSIONS: bool = false;

    fn eval(&self, ctx: &DelegateEvalContext<'_>) -> crate::Result<Self::Evaluated> {
        let mut evaluated = [const { Vec::new() }; 4];
        for ((name, curve), slot) in CURVE_NAMES
            .into_iter()
            .zip(self.curves())
            .zip(&mut evaluated)
        {
            *slot = curve
                .iter()
                .enumerate()
                .map(|(index, [x, y])| {
                    Ok([
                        DelegateEvaluable::eval(x, ctx)?,
                        DelegateEvaluable::eval(y, ctx)?,
                    ])
                    .map_err(|error: LumenError| error.in_index(index).in_field(name))
                })
                .collect::<crate::Result<Vec<_>>>()?;
            check_curve(name, slot)
                .map_err(|details| LumenError::evaluate(details).located(ctx.node_id, name))?;
        }
        let [master, red, green, blue] = evaluated;
        Ok(ColorCurves {
            master,
            red,
            green,
            blue,
        })
    }

    fn for_each_expression(&self, f: &mut dyn FnMut(&Expression)) {
        for curve in self.curves() {
            for point in curve {
                for number in point {
                    number.for_each_expression(f);
                }
            }
        }
    }

    /// Parses an object of curves, each an array of `[x, y]` points of numbers or `=expression`s.
    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        let fields = nested_fields(value, path, &CURVE_NAMES, &[])?;
        let mut curves = Self::default();
        for (name, slot) in [
            ("master", &mut curves.master),
            ("red", &mut curves.red),
            ("green", &mut curves.green),
            ("blue", &mut curves.blue),
        ] {
            if let Some(value) = fields.get(name) {
                *slot = parse_curve(value, &field_path(path, name))?;
            }
        }
        Ok(curves)
    }

    fn to_json_value(&self) -> Option<Value> {
        let mut fields = Map::new();
        for (name, curve) in CURVE_NAMES.into_iter().zip(self.curves()) {
            if !curve.is_empty() {
                let points = curve
                    .iter()
                    .map(|point| {
                        Value::Array(
                            point
                                .iter()
                                .filter_map(DelegateEvaluable::to_json_value)
                                .collect(),
                        )
                    })
                    .collect();
                fields.insert(name.into(), Value::Array(points));
            }
        }
        Some(Value::Object(fields))
    }

    fn read(&self) -> Option<PropertyRead<'_>> {
        Some(PropertyRead::Value(None))
    }
}

fn parse_curve(value: &Value, path: &str) -> Result<Vec<[Deferred<f64>; 2]>, String> {
    crate::json::reject_expression(value, path)?;
    let items = value
        .as_array()
        .ok_or_else(|| format!("`{path}` expected an array of [x, y] points, got {value}"))?;
    if items.len() > MAX_CURVE_POINTS {
        return Err(format!(
            "`{path}` has {} points, more than {MAX_CURVE_POINTS}",
            items.len()
        ));
    }
    let points = items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let at = format!("{path}[{index}]");
            crate::json::reject_expression(item, &at)?;
            let [x, y] = item
                .as_array()
                .map(Vec::as_slice)
                .and_then(|numbers| <&[Value; 2]>::try_from(numbers).ok())
                .ok_or_else(|| format!("`{at}` expected [x, y], got {item}"))?;
            Ok([
                Deferred::<f64>::from_json_value(x, &format!("{at}[0]"))?,
                Deferred::<f64>::from_json_value(y, &format!("{at}[1]"))?,
            ])
        })
        .collect::<Result<Vec<_>, String>>()?;
    let literal = points
        .iter()
        .map(|[x, y]| match (x, y) {
            (Deferred::Value(x), Deferred::Value(y)) => Some([*x, *y]),
            _ => None,
        })
        .collect::<Option<Vec<_>>>();
    if let Some(points) = literal {
        check_curve(path, &points).map_err(|details| format!("`{path}` {details}"))?;
    }
    Ok(points)
}

/// Why `points` is not a curve, or `Ok`. The error continues a sentence that starts with the
/// curve's name.
fn check_curve(name: &str, points: &[[f64; 2]]) -> Result<(), String> {
    if points.len() > MAX_CURVE_POINTS {
        return Err(format!(
            "has {} points, more than {MAX_CURVE_POINTS}",
            points.len()
        ));
    }
    for (index, [x, y]) in points.iter().enumerate() {
        for (axis, value) in [("x", x), ("y", y)] {
            if !(0.0..=1.0).contains(value) {
                return Err(format!(
                    "point {index} has {axis} {value} outside 0..1 in `{name}`"
                ));
            }
        }
    }
    if let Some(index) = points.windows(2).position(|pair| pair[1][0] <= pair[0][0]) {
        return Err(format!(
            "points must have strictly increasing x, but point {} has x {} after {} in `{name}`",
            index + 1,
            points[index + 1][0],
            points[index][0]
        ));
    }
    Ok(())
}

/// One point of a curve as the shader reads it: its position and the slope of the curve there.
type GpuPoint = [f32; 4];

impl ColorCurves {
    /// The curves as the shader takes them: the point counts, then the points of each curve in
    /// `CURVE_NAMES` order, padded to [`MAX_CURVE_POINTS`].
    pub(super) fn gpu(&self) -> ([u32; 4], [GpuPoint; 4 * MAX_CURVE_POINTS]) {
        let mut counts = [0; 4];
        let mut gpu = [[0.0; 4]; 4 * MAX_CURVE_POINTS];
        for (curve, points) in [&self.master, &self.red, &self.green, &self.blue]
            .into_iter()
            .enumerate()
        {
            counts[curve] = points.len() as u32;
            let slopes = tangents(points);
            for (slot, (point, slope)) in points.iter().zip(slopes).enumerate() {
                gpu[curve * MAX_CURVE_POINTS + slot] =
                    [point[0] as f32, point[1] as f32, slope as f32, 0.0];
            }
        }
        (counts, gpu)
    }
}

/// The slope at each of `points` that makes the cubic Hermite spline through them monotone
/// (Fritsch–Carlson): flat where the data turns, and limited where a segment would overshoot.
fn tangents(points: &[[f64; 2]]) -> Vec<f64> {
    let count = points.len();
    if count < 2 {
        return vec![0.0; count];
    }
    let secants: Vec<f64> = points
        .windows(2)
        .map(|pair| (pair[1][1] - pair[0][1]) / (pair[1][0] - pair[0][0]))
        .collect();
    let mut slopes = Vec::with_capacity(count);
    slopes.push(secants[0]);
    slopes.extend(secants.windows(2).map(|pair| {
        if pair[0] * pair[1] <= 0.0 {
            0.0
        } else {
            (pair[0] + pair[1]) / 2.0
        }
    }));
    slopes.push(secants[count - 2]);
    for (index, &secant) in secants.iter().enumerate() {
        if secant == 0.0 {
            slopes[index] = 0.0;
            slopes[index + 1] = 0.0;
            continue;
        }
        let (a, b) = (slopes[index] / secant, slopes[index + 1] / secant);
        let size = a * a + b * b;
        if size > 9.0 {
            let scale = 3.0 / size.sqrt();
            slopes[index] = scale * a * secant;
            slopes[index + 1] = scale * b * secant;
        }
    }
    slopes
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parse(value: Value) -> Result<ColorCurvesDelegate, String> {
        ColorCurvesDelegate::from_json_value(&value, "curves")
    }

    #[test]
    fn curves_take_expressions_and_round_trip() {
        let value =
            json!({ "master": [[0.0, 0.0], ["=0.5", 0.75], [1.0, 1.0]], "blue": [[0.0, 0.1]] });
        let curves = parse(value.clone()).unwrap();
        assert!(curves.has_expressions());
        assert_eq!(curves.to_json_value(), Some(value));
    }

    #[test]
    fn more_than_sixteen_points_or_a_stalled_x_is_an_error() {
        let points: Vec<_> = (0..17)
            .map(|index| json!([f64::from(index) / 17.0, 0]))
            .collect();
        let error = parse(json!({ "red": points })).unwrap_err();
        assert!(error.contains("17 points, more than 16"), "{error}");
        for (curve, expected) in [
            (json!([[0.5, 0], [0.5, 1]]), "strictly increasing x"),
            (json!([[0.6, 0], [0.4, 1]]), "strictly increasing x"),
            (json!([[0, 0], [1.5, 1]]), "outside 0..1"),
            (json!([[0, -0.1]]), "outside 0..1"),
        ] {
            let error = parse(json!({ "master": curve })).unwrap_err();
            assert!(error.contains(expected), "{curve}: {error}");
        }
    }

    #[test]
    fn tangents_never_overshoot_a_steep_segment() {
        let slopes = tangents(&[[0.0, 0.0], [0.5, 0.01], [1.0, 1.0]]);
        assert!(slopes[1] <= 3.0 * 0.02, "{slopes:?}");
        assert_eq!(tangents(&[[0.5, 0.5]]), [0.0]);
    }
}
