use crate::{
    error::{ExpressionError, LumenError},
    expr::{
        ExpressionContext,
        ast::{BuiltinFn, ExprNode, ExpressionValue},
        eval::evaluate_expr,
    },
    node::{
        NodeId, NodeProperty, PropertyEval,
        source::{
            text::TextFontStyle,
            text_layout::{TextLayoutStyle, measure_text, resolved_max_width},
        },
    },
};

pub fn evaluate_builtin(
    builtin: BuiltinFn,
    args: &[ExpressionValue],
    ctx: &ExpressionContext<'_>,
) -> crate::Result<ExpressionValue> {
    let error = |details: String| {
        LumenError::Expression(ExpressionError::Evaluate {
            path: ctx.path.clone(),
            details,
        })
    };
    let expect_len = |expected: usize| {
        if args.len() == expected {
            Ok(())
        } else {
            Err(error(format!(
                "builtin expects {expected} arguments, got {}",
                args.len()
            )))
        }
    };
    let expect_min_len = |minimum: usize| {
        if args.len() >= minimum {
            Ok(())
        } else {
            Err(error(format!(
                "builtin expects at least {minimum} arguments, got {}",
                args.len()
            )))
        }
    };

    match builtin {
        BuiltinFn::Min => {
            expect_min_len(2)?;
            let mut value =
                to_number(&args[0], error("min expects numeric arguments".to_string()))?;
            for arg in &args[1..] {
                value = value.min(to_number(
                    arg,
                    error("min expects numeric arguments".to_string()),
                )?);
            }
            Ok(ExpressionValue::Number(value))
        }
        BuiltinFn::Max => {
            expect_min_len(2)?;
            let mut value =
                to_number(&args[0], error("max expects numeric arguments".to_string()))?;
            for arg in &args[1..] {
                value = value.max(to_number(
                    arg,
                    error("max expects numeric arguments".to_string()),
                )?);
            }
            Ok(ExpressionValue::Number(value))
        }
        BuiltinFn::Abs => unary_numeric(args, &expect_len, |value| value.abs()),
        BuiltinFn::Floor => unary_numeric(args, &expect_len, |value| value.floor()),
        BuiltinFn::Ceil => unary_numeric(args, &expect_len, |value| value.ceil()),
        BuiltinFn::Round => unary_numeric(args, &expect_len, |value| value.round()),
        BuiltinFn::Sin => unary_numeric(args, &expect_len, |value| value.sin()),
        BuiltinFn::Cos => unary_numeric(args, &expect_len, |value| value.cos()),
        BuiltinFn::Fract => unary_numeric(args, &expect_len, |value| value.fract()),
        BuiltinFn::Clamp => {
            expect_len(3)?;
            let value = to_number(
                &args[0],
                error("clamp expects numeric arguments".to_string()),
            )?;
            let min = to_number(
                &args[1],
                error("clamp expects numeric arguments".to_string()),
            )?;
            let max = to_number(
                &args[2],
                error("clamp expects numeric arguments".to_string()),
            )?;
            if min > max {
                return Err(error("clamp requires min <= max".to_string()));
            }
            Ok(ExpressionValue::Number(value.clamp(min, max)))
        }
        BuiltinFn::Lerp => interpolate_linear(args, &expect_len, &error, "lerp"),
        BuiltinFn::Pow => {
            expect_len(2)?;
            let lhs = to_number(&args[0], error("pow expects numeric arguments".to_string()))?;
            let rhs = to_number(&args[1], error("pow expects numeric arguments".to_string()))?;
            Ok(ExpressionValue::Number(lhs.powf(rhs)))
        }
        BuiltinFn::Mod => {
            expect_len(2)?;
            let lhs = to_number(&args[0], error("mod expects numeric arguments".to_string()))?;
            let rhs = to_number(&args[1], error("mod expects numeric arguments".to_string()))?;
            if rhs.abs() <= f64::EPSILON {
                return Err(error("mod divisor must be non-zero".to_string()));
            }
            Ok(ExpressionValue::Number(lhs % rhs))
        }
        BuiltinFn::Smoothstep => {
            expect_len(3)?;
            let edge0 = to_number(
                &args[0],
                error("smoothstep expects numeric arguments".to_string()),
            )?;
            let edge1 = to_number(
                &args[1],
                error("smoothstep expects numeric arguments".to_string()),
            )?;
            let x = to_number(
                &args[2],
                error("smoothstep expects numeric arguments".to_string()),
            )?;
            if (edge1 - edge0).abs() <= f64::EPSILON {
                return Ok(ExpressionValue::Number(0.0));
            }
            let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
            Ok(ExpressionValue::Number(t * t * (3.0 - 2.0 * t)))
        }
        BuiltinFn::Linear => interpolate_linear(args, &expect_len, &error, "linear"),
        BuiltinFn::Step => {
            expect_len(3)?;
            let start = to_number(
                &args[0],
                error("step expects numeric arguments".to_string()),
            )?;
            let end = to_number(
                &args[1],
                error("step expects numeric arguments".to_string()),
            )?;
            let t = to_number(
                &args[2],
                error("step expects numeric arguments".to_string()),
            )?;
            Ok(ExpressionValue::Number(if t >= 1.0 { end } else { start }))
        }
        BuiltinFn::TextHeight => Err(error(
            "text_height node references should be evaluated before calling this helper"
                .to_string(),
        )),
        BuiltinFn::TextWidth => Err(error(
            "text_width node references should be evaluated before calling this helper".to_string(),
        )),
        BuiltinFn::Uppercase => {
            expect_len(1)?;
            let input = to_string(
                &args[0],
                error("uppercase expects a string argument".to_string()),
            )?;
            Ok(ExpressionValue::String(input.to_uppercase()))
        }
        BuiltinFn::Lowercase => {
            expect_len(1)?;
            let input = to_string(
                &args[0],
                error("lowercase expects a string argument".to_string()),
            )?;
            Ok(ExpressionValue::String(input.to_lowercase()))
        }
    }
}

fn interpolate_linear(
    args: &[ExpressionValue],
    expect_len: &dyn Fn(usize) -> crate::Result<()>,
    error: &dyn Fn(String) -> LumenError,
    builtin_name: &'static str,
) -> crate::Result<ExpressionValue> {
    expect_len(3)?;
    let start = to_number(
        &args[0],
        error(format!("{builtin_name} expects numeric arguments")),
    )?;
    let end = to_number(
        &args[1],
        error(format!("{builtin_name} expects numeric arguments")),
    )?;
    let t = to_number(
        &args[2],
        error(format!("{builtin_name} expects numeric arguments")),
    )?;
    Ok(ExpressionValue::Number(start + (end - start) * t))
}

fn unary_numeric(
    args: &[ExpressionValue],
    expect_len: &dyn Fn(usize) -> crate::Result<()>,
    f: impl Fn(f64) -> f64,
) -> crate::Result<ExpressionValue> {
    expect_len(1)?;
    let value = to_number(
        &args[0],
        LumenError::Expression(ExpressionError::Evaluate {
            path: None,
            details: "builtin expects numeric argument".to_string(),
        }),
    )?;
    Ok(ExpressionValue::Number(f(value)))
}

fn to_number(value: &ExpressionValue, error: LumenError) -> crate::Result<f64> {
    match value {
        ExpressionValue::Number(number) => Ok(*number),
        ExpressionValue::Boolean(boolean) => Ok(if *boolean { 1.0 } else { 0.0 }),
        ExpressionValue::String(text) => text.parse::<f64>().map_err(|_| error),
    }
}

fn to_string(value: &ExpressionValue, _error: LumenError) -> crate::Result<String> {
    match value {
        ExpressionValue::String(text) => Ok(text.clone()),
        ExpressionValue::Number(number) => Ok(number.to_string()),
        ExpressionValue::Boolean(boolean) => Ok(boolean.to_string()),
    }
}

pub fn evaluate_text_measure_builtin(
    builtin: BuiltinFn,
    args: &[ExprNode],
    ctx: &ExpressionContext<'_>,
) -> crate::Result<ExpressionValue> {
    let builtin_name = match builtin {
        BuiltinFn::TextHeight => "text_height",
        BuiltinFn::TextWidth => "text_width",
        _ => unreachable!(),
    };
    if args.is_empty() || args.len() > 3 {
        return Err(text_measure_error(
            ctx,
            format!("{builtin_name} expects 1 to 3 arguments"),
        ));
    }

    let mut input = match args.first() {
        Some(ExprNode::Node(node_id)) => resolve_text_measure_node(*node_id, ctx, builtin_name)?,
        Some(arg) => TextMeasureInput {
            text: to_string(
                &evaluate_expr(arg, ctx)?,
                text_measure_error(
                    ctx,
                    format!("{builtin_name} expects first arg as string or node reference"),
                ),
            )?,
            style: TextLayoutStyle::default(),
            wrap_width: None,
        },
        None => unreachable!(),
    };

    if let Some(font_size_arg) = args.get(1) {
        input.style.font_size = to_number(
            &evaluate_expr(font_size_arg, ctx)?,
            text_measure_error(
                ctx,
                format!("{builtin_name} optional second arg must be numeric font size"),
            ),
        )? as f32;
    }
    if let Some(max_width_arg) = args.get(2) {
        let max_width = to_number(
            &evaluate_expr(max_width_arg, ctx)?,
            text_measure_error(
                ctx,
                format!("{builtin_name} optional third arg must be numeric max width"),
            ),
        )? as f32;
        input.wrap_width = resolved_max_width(max_width);
    }

    let (width, height) = measure_text(&input.text, &input.style, input.wrap_width, ctx.width);
    Ok(ExpressionValue::Number(match builtin {
        BuiltinFn::TextHeight => f64::from(height),
        BuiltinFn::TextWidth => f64::from(width),
        _ => unreachable!(),
    }))
}

#[derive(Debug, Clone)]
struct TextMeasureInput {
    text: String,
    style: TextLayoutStyle,
    wrap_width: Option<f32>,
}

fn resolve_text_measure_node(
    node_id: NodeId,
    ctx: &ExpressionContext<'_>,
    builtin_name: &str,
) -> crate::Result<TextMeasureInput> {
    let graph = ctx.graph.ok_or_else(|| {
        text_measure_error(
            ctx,
            format!(
                "{builtin_name} requires a graph to resolve node `{}`",
                node_id.0
            ),
        )
    })?;
    let node = graph.nodes.get(&node_id).ok_or_else(|| {
        text_measure_error(
            ctx,
            format!("{builtin_name} could not find node `{}`", node_id.0),
        )
    })?;

    let text = resolve_required_node_string(node_id, node, "content", ctx, builtin_name)?;
    let font_family =
        resolve_optional_node_string(node_id, node, "font_family", ctx, builtin_name)?
            .unwrap_or_else(|| TextLayoutStyle::default().font_family);
    let font_size = resolve_optional_node_number(node_id, node, "font_size", ctx, builtin_name)?
        .unwrap_or(TextLayoutStyle::default().font_size as f64) as f32;
    let font_weight = resolve_optional_node_number(node_id, node, "font_weight", ctx, builtin_name)?
        .unwrap_or(TextLayoutStyle::default().font_weight as f64) as i32;
    let font_style = resolve_optional_node_number(node_id, node, "font_style", ctx, builtin_name)?
        .map(|value| TextFontStyle::from_int(value as i64))
        .unwrap_or(TextLayoutStyle::default().font_style);
    let wrap_width = resolve_optional_node_number(node_id, node, "max_width", ctx, builtin_name)?
        .map(|value| resolved_max_width(value as f32))
        .unwrap_or(None);

    Ok(TextMeasureInput {
        text,
        style: TextLayoutStyle::new(font_family, font_size, font_weight, font_style),
        wrap_width,
    })
}

fn resolve_required_node_string(
    node_id: NodeId,
    node: &dyn PropertyEval,
    property_path: &str,
    ctx: &ExpressionContext<'_>,
    builtin_name: &str,
) -> crate::Result<String> {
    match node.get_property(property_path)? {
        Some(property) => property.resolve_string(node_id, property_path, ctx).map_err(|_| {
            text_measure_error(
                ctx,
                format!(
                    "{builtin_name} expected node `{}` property `{property_path}` to resolve as string",
                    node_id.0
                ),
            )
        }),
        None => Err(text_measure_error(
            ctx,
            format!(
                "{builtin_name} expected node `{}` to expose `{property_path}`",
                node_id.0
            ),
        )),
    }
}

fn resolve_optional_node_string(
    node_id: NodeId,
    node: &dyn PropertyEval,
    property_path: &str,
    ctx: &ExpressionContext<'_>,
    builtin_name: &str,
) -> crate::Result<Option<String>> {
    match node.get_property(property_path)? {
        Some(property) => property
            .resolve_string(node_id, property_path, ctx)
            .map(Some)
            .map_err(|_| {
                text_measure_error(
                    ctx,
                    format!(
                        "{builtin_name} expected node `{}` property `{property_path}` to resolve as string",
                        node_id.0
                    ),
                )
            }),
        None => Ok(None),
    }
}

fn resolve_optional_node_number(
    node_id: NodeId,
    node: &dyn PropertyEval,
    property_path: &str,
    ctx: &ExpressionContext<'_>,
    builtin_name: &str,
) -> crate::Result<Option<f64>> {
    match node.get_property(property_path)? {
        Some(NodeProperty::Expr(expr)) => expr
            .evaluate(ctx)?
            .as_f64()
            .map(Some)
            .ok_or_else(|| {
                text_measure_error(
                    ctx,
                    format!(
                        "{builtin_name} expected node `{}` property `{property_path}` to resolve as number",
                        node_id.0
                    ),
                )
            }),
        Some(property) => property
            .resolve_float(node_id, property_path, ctx)
            .map(Some)
            .map_err(|_| {
                text_measure_error(
                    ctx,
                    format!(
                        "{builtin_name} expected node `{}` property `{property_path}` to resolve as number",
                        node_id.0
                    ),
                )
            }),
        None => Ok(None),
    }
}

fn text_measure_error(ctx: &ExpressionContext<'_>, details: String) -> LumenError {
    LumenError::Expression(ExpressionError::Evaluate {
        path: ctx.path.clone(),
        details,
    })
}
