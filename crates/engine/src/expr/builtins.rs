use crate::{
    error::LumenError,
    expr::{
        ExpressionContext,
        ast::{BuiltinFn, Easing, ExpressionValue, Key},
        easing::{SPRING_DAMPING, SPRING_STIFFNESS, spring},
        noise::{noise, random},
        path::{MorphError, morph, sample_path},
        spec::{BuiltinArg, BuiltinSpec, NamedEasing, ValueKind, easing_builtins},
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
            "`{name}` expects `from` and `to` of the same type: numbers, vec2 values, colours or path data, got {} and {}",
            args[0].type_name(),
            args[1].type_name()
        ))
    };
    let named = |details: String| error(format!("`{}`: {details}", spec.name));
    let second_is_key = matches!(args.get(1), Some(ExpressionValue::Key(_)));
    for (index, arg) in args.iter().enumerate() {
        if let Some(details) = kind_error(spec, index, arg.kind(), second_is_key) {
            return Err(named(details));
        }
    }
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
        BuiltinFn::Sign => unary(|value| {
            if value == 0.0 || value.is_nan() {
                value
            } else {
                value.signum()
            }
        }),
        BuiltinFn::Atan2 => Ok(ExpressionValue::Number(number(0)?.atan2(number(1)?))),
        BuiltinFn::Hypot => Ok(ExpressionValue::Number(number(0)?.hypot(number(1)?))),
        BuiltinFn::Remap => {
            let (value, from_min, from_max) = (number(0)?, number(1)?, number(2)?);
            let (to_min, to_max) = (number(3)?, number(4)?);
            if from_min == from_max {
                return Err(error("remap requires from_min != from_max".to_string()));
            }
            let t = (value - from_min) / (from_max - from_min);
            Ok(ExpressionValue::Number(to_min + (to_max - to_min) * t))
        }
        BuiltinFn::Clamp => {
            let (value, min, max) = (number(0)?, number(1)?, number(2)?);
            if min.is_nan() || max.is_nan() || min > max {
                return Err(error("clamp requires min <= max".to_string()));
            }
            Ok(ExpressionValue::Number(value.clamp(min, max)))
        }
        BuiltinFn::Lerp => interpolate(&args[0], &args[1], number(2)?)
            .map_err(|error| named(error.describe(FROM_TO)))?
            .ok_or_else(|| mismatch(spec.name)),
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
            let ease = args.get(4).map(easing).transpose().map_err(named)?;
            let start = number(2)?;
            let t = ease
                .unwrap_or(Easing::LINEAR)
                .apply(progress(ctx, start, start + number(3)?));
            interpolate(&args[0], &args[1], t)
                .map_err(|error| named(error.describe(FROM_TO)))?
                .ok_or_else(|| mismatch(spec.name))
        }
        easing_builtins!() => {
            let easing = NamedEasing::for_builtin(builtin).expect("easing builtin");
            Ok(ExpressionValue::Number(easing.apply(number(0)?)))
        }
        BuiltinFn::Ease => {
            let easing = easing(&args[0]).map_err(named)?;
            Ok(ExpressionValue::Number(easing.apply(number(1)?)))
        }
        BuiltinFn::CubicBezier => {
            let handle = |index: usize| {
                let (x, y) = args[index].as_vec2().ok_or_else(|| {
                    named(format!(
                        "`{}` must be a vec2, got {}",
                        spec.args[index].name,
                        args[index].type_name()
                    ))
                })?;
                match handle_x_error(x) {
                    Some(details) => Err(named(details.to_string())),
                    None => Ok([x, y]),
                }
            };
            let ([x1, y1], [x2, y2]) = (handle(0)?, handle(1)?);
            Ok(ExpressionValue::Easing(Easing::CubicBezier([
                x1, y1, x2, y2,
            ])))
        }
        BuiltinFn::Key => {
            let at = number(0)?;
            if !at.is_finite() {
                return Err(named(format!("`at` must be finite, got {at}")));
            }
            // Booleans become numbers so a track has one kind before, between and after its keys.
            let value = match args[1].as_f64() {
                Some(number) => ExpressionValue::Number(number),
                None => args[1].clone(),
            };
            if !is_key_value(value.kind()) {
                return Err(named(key_value_error(value.kind())));
            }
            let ease = args.get(2).map(easing).transpose().map_err(named)?;
            Ok(ExpressionValue::Key(Box::new(Key { at, value, ease })))
        }
        BuiltinFn::Keyframes => keyframes(number(0)?, &args[1..]).map_err(named),
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
        BuiltinFn::Noise => Ok(ExpressionValue::Number(noise(
            checked(0, number(0)?)?,
            checked(1, number(1)?)?,
        ))),
        BuiltinFn::Random => {
            let (seed, x) = if args.len() == 1 {
                (0.0, checked(0, number(0)?)?)
            } else {
                (checked(0, number(0)?)?, checked(1, number(1)?)?)
            };
            Ok(ExpressionValue::Number(random(seed, x)))
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
        BuiltinFn::PathPoint | BuiltinFn::PathAngle => {
            let ExpressionValue::String(data) = &args[0] else {
                return Err(named(format!(
                    "`path_data` must be a string, got {}",
                    args[0].type_name()
                )));
            };
            let sample = sample_path(data, checked(1, number(1)?)?)
                .map_err(|details| named(format!("`path_data` {details}")))?;
            Ok(if builtin == BuiltinFn::PathPoint {
                ExpressionValue::Vec2(sample.point)
            } else {
                ExpressionValue::Number(sample.angle)
            })
        }
    }
}

/// The argument `index` of a call to `spec` binds to. The second argument of `keyframes` is its
/// first key when `second_is_key`, and its default easing otherwise.
pub(crate) fn param(spec: &BuiltinSpec, index: usize, second_is_key: bool) -> Option<&BuiltinArg> {
    let index = if spec.builtin == BuiltinFn::Keyframes && second_is_key && index == 1 {
        2
    } else {
        index
    };
    let repeats = spec.max_args.is_none();
    spec.args
        .get(index)
        .or_else(|| spec.args.last().filter(|_| repeats))
}

/// Why a `given` value can't be argument `index` of a call to `spec`, if it can't. Only easings
/// and keys are checked: they go to the params that take them and nowhere else.
pub(crate) fn kind_error(
    spec: &BuiltinSpec,
    index: usize,
    given: ValueKind,
    second_is_key: bool,
) -> Option<String> {
    let param = param(spec, index, second_is_key)?;
    let name = param.name;
    match (param.kind, given) {
        (ValueKind::Easing, ValueKind::Easing | ValueKind::String)
        | (ValueKind::Key, ValueKind::Key) => None,
        (ValueKind::Easing, _) => Some(format!(
            "`{name}` must be an easing such as 'out_expo' or cubic_bezier(out, in), got {}",
            given.name()
        )),
        (ValueKind::Key, _) => Some(format!(
            "`{name}` must be a key made with key(at, value), got {}",
            given.name()
        )),
        (_, ValueKind::Easing) => Some(format!("`{name}` does not take an easing")),
        (_, ValueKind::Key) => Some(format!("`{name}` does not take a key")),
        _ => None,
    }
}

pub(crate) fn unknown_easing(name: &str) -> String {
    let names = NamedEasing::ALL.map(NamedEasing::name).join(", ");
    format!("unknown easing `{name}`; expected one of {names}, or cubic_bezier(out, in)")
}

/// An easing argument, which is an easing or an easing's name.
fn easing(value: &ExpressionValue) -> Result<Easing, String> {
    match value {
        ExpressionValue::Easing(easing) => Ok(*easing),
        ExpressionValue::String(name) => NamedEasing::from_name(name)
            .map(Easing::Named)
            .ok_or_else(|| unknown_easing(name)),
        other => Err(format!("expected an easing, got {}", other.type_name())),
    }
}

/// Why `x` can't be the x component of a `cubic_bezier` handle, if it can't.
pub(crate) fn handle_x_error(x: f64) -> Option<&'static str> {
    (!(0.0..=1.0).contains(&x)).then_some("the x components of `out` and `in` must be in 0..=1")
}

pub(crate) fn is_key_value(kind: ValueKind) -> bool {
    matches!(
        kind,
        ValueKind::Number | ValueKind::Vec2 | ValueKind::Color | ValueKind::String
    )
}

pub(crate) fn key_value_error(kind: ValueKind) -> String {
    format!(
        "`value` must be a number, vec2, colour or path data, got {}",
        kind.name()
    )
}

/// Why a key at `at` can't follow one at `previous`, if it can't.
/// `unit` is the suffix the times were written with, such as `s` for time literals.
pub(crate) fn key_order_error(previous: f64, at: f64, unit: &str) -> Option<String> {
    (previous > at).then(|| {
        format!(
            "keys must be in order of `at`, but a key at {at}{unit} follows one at {previous}{unit}"
        )
    })
}

/// Why an expression whose result is `kind` can't be a param value, if it can't.
pub(crate) fn unusable_result(kind: ValueKind) -> Option<String> {
    match kind {
        ValueKind::Easing => Some(
            "the expression is an easing, which no param takes; pass it to `ease`, `tween`, \
             `key` or `keyframes`"
                .to_string(),
        ),
        ValueKind::Key => Some(
            "the expression is a key, which no param takes; pass it to `keyframes`".to_string(),
        ),
        _ => None,
    }
}

pub(crate) fn mixed_keys_error(first: ValueKind, other: ValueKind) -> String {
    format!(
        "every key must hold the same kind of value, got {} and {}",
        first.name(),
        other.name()
    )
}

/// The value at `t` of a track of keys, which may be preceded by the default easing.
fn keyframes(t: f64, args: &[ExpressionValue]) -> Result<ExpressionValue, String> {
    let (default, keys) = match args.split_first() {
        Some((ease, keys)) if !matches!(ease, ExpressionValue::Key(_)) => (easing(ease)?, keys),
        _ => (Easing::LINEAR, args),
    };
    let keys = keys
        .iter()
        .map(|key| match key {
            ExpressionValue::Key(key) => Ok(&**key),
            other => Err(format!("expected a key, got {}", other.type_name())),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let Some(first) = keys.first() else {
        return Err("expects at least one key".to_string());
    };
    for (index, key) in keys.iter().enumerate().skip(1) {
        let previous = keys[index - 1];
        if key.value.kind() != first.value.kind() {
            return Err(mixed_keys_error(first.value.kind(), key.value.kind()));
        }
        if let Some(details) = key_order_error(previous.at, key.at, "") {
            return Err(details);
        }
    }
    if t.is_nan() || t < first.at {
        return Ok(first.value.clone());
    }
    // The last key at or before `t`, so of keys at the same `at`, the later one is reached.
    let index = keys.partition_point(|key| key.at <= t) - 1;
    let key = keys[index];
    let Some(next) = keys.get(index + 1) else {
        return Ok(key.value.clone());
    };
    let ease = key.ease.unwrap_or(default);
    // A hold doesn't interpolate, so it can switch between paths of any structure.
    if ease == Easing::Named(NamedEasing::Hold) {
        return Ok(key.value.clone());
    }
    let eased = ease.apply((t - key.at) / (next.at - key.at));
    interpolate(&key.value, &next.value, eased)
        .map_err(|error| error.describe([&key_name(key.at), &key_name(next.at)]))?
        .ok_or_else(|| mixed_keys_error(key.value.kind(), next.value.kind()))
}

/// How errors name the key at `at`.
pub(crate) fn key_name(at: impl std::fmt::Display) -> String {
    format!("the key at {at}")
}

/// How errors name the sides of `lerp` and `tween`.
pub(crate) const FROM_TO: [&str; 2] = ["`from`", "`to`"];

/// Why `value` is out of range as argument `index` of `builtin`, if it is.
pub(crate) fn range_error(builtin: BuiltinFn, index: usize, value: f64) -> Option<&'static str> {
    match (builtin, index) {
        (BuiltinFn::Spring, 1) if value <= 0.0 || !value.is_finite() => {
            Some("stiffness must be greater than 0")
        }
        (BuiltinFn::Spring, 2) if value < 0.0 || !value.is_finite() => {
            Some("damping must not be negative")
        }
        (BuiltinFn::Noise | BuiltinFn::Random, _) if !value.is_finite() => {
            Some("arguments must be finite numbers")
        }
        (BuiltinFn::PathPoint | BuiltinFn::PathAngle, 1) if value.is_nan() => {
            Some("`t` must not be NaN")
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

/// `from` and `to` mixed by `t`, or `None` if they are values of kinds that don't mix.
fn interpolate(
    from: &ExpressionValue,
    to: &ExpressionValue,
    t: f64,
) -> Result<Option<ExpressionValue>, MorphError> {
    let mix = |from: f64, to: f64| from + (to - from) * t;
    Ok(match (from, to) {
        (ExpressionValue::String(from), ExpressionValue::String(to)) => {
            Some(ExpressionValue::String(morph(from, to, t)?))
        }
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
        _ => from
            .as_f64()
            .zip(to.as_f64())
            .map(|(from, to)| ExpressionValue::Number(mix(from, to))),
    })
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
