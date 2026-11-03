use crate::{
    error::LumenError,
    expr::{
        ExpressionContext,
        ast::{BuiltinFn, ExpressionValue},
        easing::{SPRING_DAMPING, SPRING_STIFFNESS, cubic_bezier, spring},
        spec::{Easing, easing_builtins},
    },
    node::{
        DelegateEvalContext, DelegateEvaluable, NodeId, NodeKind,
        source::{text::TextLayoutParams, text_content::TextContent},
    },
};

pub fn evaluate_builtin(
    builtin: BuiltinFn,
    args: &[ExpressionValue],
    ctx: &ExpressionContext<'_>,
) -> crate::Result<ExpressionValue> {
    let spec = builtin.spec();
    let error = |details: String| LumenError::evaluate(details);
    let number = |index: usize| {
        args[index]
            .as_f64()
            .ok_or_else(|| error(format!("{} expects numeric arguments", spec.name)))
    };
    let checked = |index: usize, value: f64| match range_error(builtin, index, value) {
        Some(details) => Err(error(format!("`{}`: {details}", spec.name))),
        None => Ok(value),
    };
    let mismatch = |name: &str| {
        error(format!(
            "`{name}` expects `from` and `to` of the same type: numbers, vec2 values or colours, got {} and {}",
            args[0].type_name(),
            args[1].type_name()
        ))
    };
    let unary = |f: fn(f64) -> f64| Ok(ExpressionValue::Number(f(number(0)?)));
    let optional = |index: usize, default: f64| {
        if index < args.len() {
            number(index)
        } else {
            Ok(default)
        }
    };

    match builtin {
        BuiltinFn::Min | BuiltinFn::Max => {
            let mut value = number(0)?;
            for index in 1..args.len() {
                value = if builtin == BuiltinFn::Min {
                    value.min(number(index)?)
                } else {
                    value.max(number(index)?)
                };
            }
            Ok(ExpressionValue::Number(value))
        }
        BuiltinFn::Abs => unary(f64::abs),
        BuiltinFn::Floor => unary(f64::floor),
        BuiltinFn::Ceil => unary(f64::ceil),
        BuiltinFn::Round => unary(f64::round),
        BuiltinFn::Sin => unary(f64::sin),
        BuiltinFn::Cos => unary(f64::cos),
        BuiltinFn::Fract => unary(f64::fract),
        BuiltinFn::Clamp => {
            let (value, min, max) = (number(0)?, number(1)?, number(2)?);
            if min.is_nan() || max.is_nan() || min > max {
                return Err(error("clamp requires min <= max".to_string()));
            }
            Ok(ExpressionValue::Number(value.clamp(min, max)))
        }
        BuiltinFn::Lerp => {
            interpolate(&args[0], &args[1], number(2)?).ok_or_else(|| mismatch(spec.name))
        }
        BuiltinFn::Pow => Ok(ExpressionValue::Number(number(0)?.powf(number(1)?))),
        BuiltinFn::Mod => {
            let (lhs, rhs) = (number(0)?, number(1)?);
            if rhs == 0.0 {
                return Err(error("mod divisor must be non-zero".to_string()));
            }
            Ok(ExpressionValue::Number(lhs % rhs))
        }
        BuiltinFn::Smoothstep => {
            let (edge0, edge1, x) = (number(0)?, number(1)?, number(2)?);
            if (edge1 - edge0).abs() <= f64::EPSILON {
                return Ok(ExpressionValue::Number(0.0));
            }
            let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
            Ok(ExpressionValue::Number(t * t * (3.0 - 2.0 * t)))
        }
        BuiltinFn::Step => {
            let (start, end, t) = (number(0)?, number(1)?, number(2)?);
            Ok(ExpressionValue::Number(if t >= 1.0 { end } else { start }))
        }
        BuiltinFn::TextHeight | BuiltinFn::TextWidth => measure_text(
            builtin,
            args[0].as_string(),
            optional(1, 16.0)?,
            optional(2, 0.0)?,
            args.get(3).map(ExpressionValue::as_string),
            ctx,
        ),
        BuiltinFn::Uppercase => Ok(ExpressionValue::String(args[0].as_string().to_uppercase())),
        BuiltinFn::Lowercase => Ok(ExpressionValue::String(args[0].as_string().to_lowercase())),
        BuiltinFn::Vec2 => Ok(ExpressionValue::Vec2((number(0)?, number(1)?))),
        BuiltinFn::Rgb | BuiltinFn::Rgba => {
            let mut color = [1.0; 4];
            for (index, component) in color.iter_mut().take(args.len()).enumerate() {
                *component = checked(index, number(index)?)? / 255.0;
            }
            Ok(ExpressionValue::Color(color))
        }
        BuiltinFn::Progress => Ok(ExpressionValue::Number(progress(
            ctx,
            number(0)?,
            number(1)?,
        ))),
        BuiltinFn::Tween => {
            let ease = match args.get(4) {
                None => Some(Easing::Linear),
                Some(ExpressionValue::String(name)) => Easing::from_name(name),
                Some(_) => None,
            }
            .ok_or_else(|| error(format!("unknown easing `{}`", args[4].as_string())))?;
            let start = number(2)?;
            let t = ease.apply(progress(ctx, start, start + number(3)?));
            interpolate(&args[0], &args[1], t).ok_or_else(|| mismatch(spec.name))
        }
        easing_builtins!() => {
            let easing = Easing::for_builtin(builtin).expect("easing builtin");
            Ok(ExpressionValue::Number(easing.apply(number(0)?)))
        }
        BuiltinFn::CubicBezier => Ok(ExpressionValue::Number(cubic_bezier(
            number(0)?,
            number(1)?,
            number(2)?,
            number(3)?,
            number(4)?,
        ))),
        BuiltinFn::Prop => {
            let name = args[0].as_string();
            let props = ctx.props.ok_or_else(|| {
                error(format!(
                    "`prop('{name}')` is only available inside a composition template"
                ))
            })?;
            props.get(&name).cloned().ok_or_else(|| {
                let declared = props
                    .keys()
                    .map(|name| format!("`{name}`"))
                    .collect::<Vec<_>>();
                error(format!(
                    "unknown prop `{name}`; the template declares {}",
                    if declared.is_empty() {
                        "no props".to_string()
                    } else {
                        declared.join(", ")
                    }
                ))
            })
        }
        BuiltinFn::Spring => {
            let stiffness = checked(1, optional(1, SPRING_STIFFNESS)?)?;
            let damping = checked(2, optional(2, SPRING_DAMPING)?)?;
            Ok(ExpressionValue::Number(spring(
                number(0)?,
                stiffness,
                damping,
            )))
        }
    }
}

/// Why `value` is out of range as argument `index` of `builtin`, if it is.
pub(crate) fn range_error(builtin: BuiltinFn, index: usize, value: f64) -> Option<&'static str> {
    match (builtin, index) {
        (BuiltinFn::Spring, 1) if value <= 0.0 || !value.is_finite() => {
            Some("stiffness must be greater than 0")
        }
        (BuiltinFn::Spring, 2) if value < 0.0 || !value.is_finite() => {
            Some("damping must not be negative")
        }
        (BuiltinFn::Rgb | BuiltinFn::Rgba, _) if !(0.0..=255.0).contains(&value) => {
            Some("colour components must be between 0 and 255")
        }
        _ => None,
    }
}

/// Clamped progress through frames `start..end`; a step at `start` when `end <= start`.
fn progress(ctx: &ExpressionContext<'_>, start: f64, end: f64) -> f64 {
    let frame = ctx.frame;
    if end <= start {
        return if frame >= start { 1.0 } else { 0.0 };
    }
    ((frame - start) / (end - start)).clamp(0.0, 1.0)
}

fn interpolate(from: &ExpressionValue, to: &ExpressionValue, t: f64) -> Option<ExpressionValue> {
    let mix = |from: f64, to: f64| from + (to - from) * t;
    match (from, to) {
        (ExpressionValue::Vec2(from), ExpressionValue::Vec2(to)) => Some(ExpressionValue::Vec2((
            mix(from.0, to.0),
            mix(from.1, to.1),
        ))),
        // Premultiplied, like CSS, so a fade to transparent doesn't darken.
        (ExpressionValue::Color(from), ExpressionValue::Color(to)) => {
            let premultiply = |[r, g, b, a]: [f64; 4]| [r * a, g * a, b * a, a];
            let (from, to) = (premultiply(*from), premultiply(*to));
            let [r, g, b, a] = std::array::from_fn(|index| mix(from[index], to[index]));
            Some(ExpressionValue::Color(if a.abs() > f64::EPSILON {
                [r / a, g / a, b / a, a]
            } else {
                [0.0; 4]
            }))
        }
        _ => Some(ExpressionValue::Number(mix(from.as_f64()?, to.as_f64()?))),
    }
}

/// `builtin`'s dimension of `measurement`.
fn measured(builtin: BuiltinFn, measurement: lumen_text::TextMeasurement) -> ExpressionValue {
    ExpressionValue::Number(f64::from(if builtin == BuiltinFn::TextHeight {
        measurement.height
    } else {
        measurement.width
    }))
}

fn measure_text(
    builtin: BuiltinFn,
    text: String,
    font_size: f64,
    max_width: f64,
    font_family: Option<String>,
    ctx: &ExpressionContext<'_>,
) -> crate::Result<ExpressionValue> {
    let layout = TextLayoutParams {
        content: TextContent::plain(text),
        font_family: font_family.unwrap_or_else(|| lumen_text::DEFAULT_FONT_FAMILY.to_string()),
        font_size,
        max_width,
        ..TextLayoutParams::default()
    };
    let measurement = measure_text_value(ctx, layout.request(&[]))?;
    Ok(measured(builtin, measurement))
}

/// `text_width(node(id))` or `text_height(node(id))`.
pub fn measure_node(
    builtin: BuiltinFn,
    node_id: NodeId,
    ctx: &ExpressionContext<'_>,
) -> crate::Result<ExpressionValue> {
    Ok(measured(builtin, measure_text_node(node_id, ctx)?))
}

fn measure_text_node(
    node_id: NodeId,
    ctx: &ExpressionContext<'_>,
) -> crate::Result<lumen_text::TextMeasurement> {
    let ctx = &ctx.without_glyph();
    let graph = ctx.graph.ok_or_else(|| {
        LumenError::evaluate(format!(
            "no graph available to resolve text node `{}`",
            node_id.0
        ))
    })?;
    let node = graph
        .nodes
        .get(&node_id)
        .ok_or_else(|| LumenError::evaluate(format!("text node `{}` not found", node_id.0)))?;
    let NodeKind::Text(text) = node else {
        return Err(LumenError::evaluate(format!(
            "node `{}` is not a text node",
            node_id.0
        )));
    };

    let layout = text
        .params
        .layout
        .eval(&DelegateEvalContext { node_id, expr: ctx })?;
    measure_text_value(ctx, layout.request(&[]))
}

fn measure_text_value(
    ctx: &ExpressionContext<'_>,
    request: lumen_text::TextLayoutRequest,
) -> crate::Result<lumen_text::TextMeasurement> {
    let mut text_system = crate::node::source::text::text_system(ctx.media)?;
    for span in &request.spans {
        crate::node::source::text::load_font_family(
            &mut text_system,
            ctx.media,
            &span.font_family,
        )?;
    }
    Ok(text_system.measure(&request))
}
