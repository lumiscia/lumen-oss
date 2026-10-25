use std::cell::RefCell;

use crate::{
    error::{ExpressionError, LumenError},
    expr::{
        ExpressionContext,
        ast::{BuiltinFn, ExprNode, ExpressionValue},
        easing::{Easing, SPRING_DAMPING, SPRING_STIFFNESS, cubic_bezier, spring},
        eval::evaluate_expr,
    },
    node::{
        DelegateEvalContext, DelegateEvaluable, NodeId, NodeKind,
        source::text::{
            TextAlignmentHorizontal, TextFontStyle, apply_line_spacing, layout_request,
        },
    },
};

pub fn evaluate_builtin(
    builtin: BuiltinFn,
    args: &[ExpressionValue],
    ctx: &ExpressionContext<'_>,
) -> crate::Result<ExpressionValue> {
    let spec = builtin.spec();
    let error = |details: String| eval_error(ctx, details);
    if !spec.accepts_arity(args.len()) {
        return Err(error(format!(
            "builtin expects {} arguments, got {}",
            spec.arity_description(),
            args.len()
        )));
    }
    let number = |index: usize| {
        args[index]
            .as_f64()
            .ok_or_else(|| error(format!("{} expects numeric arguments", spec.name)))
    };
    let mismatch = |name: &str| {
        error(format!(
            "`{name}` expects `from` and `to` of the same type: numbers or vec2 values, got {} and {}",
            args[0].type_name(),
            args[1].type_name()
        ))
    };
    let unary = |f: fn(f64) -> f64| Ok(ExpressionValue::Number(f(number(0)?)));

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
            if min > max {
                return Err(error("clamp requires min <= max".to_string()));
            }
            Ok(ExpressionValue::Number(value.clamp(min, max)))
        }
        BuiltinFn::Lerp | BuiltinFn::Linear => {
            interpolate(&args[0], &args[1], number(2)?).ok_or_else(|| mismatch(spec.name))
        }
        BuiltinFn::Pow => Ok(ExpressionValue::Number(number(0)?.powf(number(1)?))),
        BuiltinFn::Mod => {
            let (lhs, rhs) = (number(0)?, number(1)?);
            if rhs.abs() <= f64::EPSILON {
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
        BuiltinFn::TextHeight | BuiltinFn::TextWidth => Err(error(format!(
            "{} node references should be evaluated before calling this helper",
            spec.name
        ))),
        BuiltinFn::Uppercase => Ok(ExpressionValue::String(args[0].as_string().to_uppercase())),
        BuiltinFn::Lowercase => Ok(ExpressionValue::String(args[0].as_string().to_lowercase())),
        BuiltinFn::Vec2 => Ok(ExpressionValue::Vec2((number(0)?, number(1)?))),
        BuiltinFn::Progress => Ok(ExpressionValue::Number(progress(
            ctx,
            number(0)?,
            number(1)?,
        ))),
        BuiltinFn::Tween => {
            let ease = match args.get(4) {
                None => Easing::Linear,
                Some(ease) => Easing::from_name(&ease.as_string())
                    .ok_or_else(|| error(format!("unknown easing `{}`", ease.as_string())))?,
            };
            let start = number(2)?;
            let t = ease.apply(progress(ctx, start, start + number(3)?));
            interpolate(&args[0], &args[1], t).ok_or_else(|| mismatch(spec.name))
        }
        BuiltinFn::InQuad
        | BuiltinFn::OutQuad
        | BuiltinFn::InOutQuad
        | BuiltinFn::InCubic
        | BuiltinFn::OutCubic
        | BuiltinFn::InOutCubic
        | BuiltinFn::InExpo
        | BuiltinFn::OutExpo
        | BuiltinFn::InOutExpo
        | BuiltinFn::InBack
        | BuiltinFn::OutBack
        | BuiltinFn::InOutBack => {
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
        BuiltinFn::Spring => {
            let optional = |index: usize, default: f64| {
                if index < args.len() {
                    number(index)
                } else {
                    Ok(default)
                }
            };
            let stiffness = optional(1, SPRING_STIFFNESS)?;
            let damping = optional(2, SPRING_DAMPING)?;
            if !stiffness.is_finite() || stiffness <= 0.0 {
                return Err(error("spring stiffness must be greater than 0".to_string()));
            }
            if !damping.is_finite() || damping < 0.0 {
                return Err(error("spring damping must not be negative".to_string()));
            }
            Ok(ExpressionValue::Number(spring(
                number(0)?,
                stiffness,
                damping,
            )))
        }
    }
}

/// Clamped progress through frames `start..end`; a step at `start` when `end <= start`.
fn progress(ctx: &ExpressionContext<'_>, start: f64, end: f64) -> f64 {
    let frame = f64::from(ctx.frame);
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
        _ => Some(ExpressionValue::Number(mix(from.as_f64()?, to.as_f64()?))),
    }
}

fn eval_error(ctx: &ExpressionContext<'_>, details: String) -> LumenError {
    LumenError::Expression(ExpressionError::Evaluate {
        path: ctx.path.clone(),
        details,
    })
}

pub fn evaluate_text_measure_builtin(
    builtin: BuiltinFn,
    args: &[ExprNode],
    ctx: &ExpressionContext<'_>,
) -> crate::Result<ExpressionValue> {
    let builtin_name = builtin.name();
    if !builtin.spec().accepts_arity(args.len()) {
        return Err(eval_error(
            ctx,
            format!("{builtin_name} expects 1 to 4 arguments"),
        ));
    }

    if let Some(ExprNode::Node(node_id)) = args.first() {
        if args.len() != 1 {
            return Err(eval_error(
                ctx,
                format!("{builtin_name}(node(id)) does not accept override arguments"),
            ));
        }
        let measurement = measure_text_node(*node_id, ctx)?;
        return Ok(ExpressionValue::Number(match builtin {
            BuiltinFn::TextHeight => f64::from(measurement.height),
            BuiltinFn::TextWidth => f64::from(measurement.width),
            _ => unreachable!(),
        }));
    }

    let evaluate = |index: usize| {
        args.get(index)
            .map(|arg| evaluate_expr(arg, ctx))
            .transpose()
    };
    let number = |index: usize, details: &str| {
        evaluate(index)?
            .map(|value| {
                value
                    .as_f64()
                    .ok_or_else(|| eval_error(ctx, format!("{builtin_name} {details}")))
            })
            .transpose()
    };
    let text = evaluate(0)?
        .map(|value| value.as_string())
        .unwrap_or_default();
    let font_size = number(1, "optional second arg must be numeric font size")?.unwrap_or(16.0);
    let max_width = number(2, "optional third arg must be numeric max width")?.unwrap_or(0.0);
    let font_family = evaluate(3)?
        .map(|value| value.as_string())
        .unwrap_or_else(|| "sans-serif".to_string());
    let measurement = measure_text_value(
        ctx,
        layout_request(
            text,
            font_family,
            font_size as f32,
            400,
            TextFontStyle::Normal,
            max_width as f32,
            TextAlignmentHorizontal::Left,
        ),
    )?;
    Ok(ExpressionValue::Number(match builtin {
        BuiltinFn::TextHeight => f64::from(measurement.height),
        BuiltinFn::TextWidth => f64::from(measurement.width),
        _ => unreachable!(),
    }))
}

fn measure_text_node(
    node_id: NodeId,
    ctx: &ExpressionContext<'_>,
) -> crate::Result<lumen_text::TextMeasurement> {
    let _measurement_guard = TextMeasurementGuard::enter(node_id, ctx)?;
    let graph = ctx.graph.ok_or_else(|| {
        eval_error(
            ctx,
            format!("no graph available to resolve text node `{}`", node_id.0),
        )
    })?;
    let node = graph
        .nodes
        .get(&node_id)
        .ok_or_else(|| eval_error(ctx, format!("text node `{}` not found", node_id.0)))?;
    let NodeKind::Text(text) = node else {
        return Err(eval_error(
            ctx,
            format!("node `{}` is not a text node", node_id.0),
        ));
    };

    let content = text.params.content.eval(node_id, "content", ctx)?;
    let font_family = text.params.font_family.eval(node_id, "font_family", ctx)?;
    let font_size = text.params.font_size.eval(node_id, "font_size", ctx)? as f32;
    let font_weight = text.params.font_weight.eval(node_id, "font_weight", ctx)?;
    let font_style = text.params.font_style.eval(&DelegateEvalContext {
        node_id,
        property_path: "font_style",
        expr: ctx,
    })?;
    let max_width = text.params.max_width.eval(node_id, "max_width", ctx)? as f32;
    let letter_spacing = text
        .params
        .letter_spacing
        .eval(node_id, "letter_spacing", ctx)?;
    let line_height = text.params.line_height.eval(node_id, "line_height", ctx)?;
    let alignment_horizontal = text
        .params
        .alignment_horizontal
        .eval(&DelegateEvalContext {
            node_id,
            property_path: "alignment_horizontal",
            expr: ctx,
        })?;
    let mut request = layout_request(
        content,
        font_family,
        font_size,
        font_weight,
        font_style,
        max_width,
        alignment_horizontal,
    );
    apply_line_spacing(&mut request, letter_spacing, line_height);
    measure_text_value(ctx, request)
}

thread_local! {
    static TEXT_MEASUREMENT_STACK: RefCell<Vec<NodeId>> = const { RefCell::new(Vec::new()) };
}

struct TextMeasurementGuard(NodeId);

impl TextMeasurementGuard {
    fn enter(node_id: NodeId, ctx: &ExpressionContext<'_>) -> crate::Result<Self> {
        TEXT_MEASUREMENT_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            if stack.contains(&node_id) {
                return Err(eval_error(
                    ctx,
                    format!("recursive text measurement for node `{}`", node_id.0),
                ));
            }
            stack.push(node_id);
            Ok(Self(node_id))
        })
    }
}

impl Drop for TextMeasurementGuard {
    fn drop(&mut self) {
        TEXT_MEASUREMENT_STACK.with(|stack| {
            let removed = stack.borrow_mut().pop();
            debug_assert_eq!(removed, Some(self.0));
        });
    }
}

fn measure_text_value(
    ctx: &ExpressionContext<'_>,
    request: lumen_text::TextLayoutRequest,
) -> crate::Result<lumen_text::TextMeasurement> {
    let mut text_system = crate::node::source::text::text_system()?;
    crate::node::source::text::load_font_family(&mut text_system, ctx.media, &request.font_family)?;
    Ok(text_system.measure(&request))
}
