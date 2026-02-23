use crate::{
    error::{ExpressionError, LumenError},
    expr::ast::{BuiltinFn, ExpressionValue},
    render::RenderContext,
};

pub fn evaluate_builtin(
    builtin: BuiltinFn,
    args: &[ExpressionValue],
    ctx: &RenderContext,
    node_id: Option<crate::node::NodeId>,
    property_path: Option<String>,
) -> Result<ExpressionValue, LumenError> {
    let error = |details: String| {
        LumenError::Expression(ExpressionError::Evaluate {
            node_id,
            property_path: property_path.clone(),
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
        BuiltinFn::Lerp => {
            expect_len(3)?;
            let start = to_number(
                &args[0],
                error("lerp expects numeric arguments".to_string()),
            )?;
            let end = to_number(
                &args[1],
                error("lerp expects numeric arguments".to_string()),
            )?;
            let t = to_number(
                &args[2],
                error("lerp expects numeric arguments".to_string()),
            )?;
            Ok(ExpressionValue::Number(start + (end - start) * t))
        }
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
        BuiltinFn::TextHeight => {
            if args.is_empty() || args.len() > 2 {
                return Err(error("text_height expects 1 or 2 arguments".to_string()));
            }
            let text = to_string(
                &args[0],
                error("text_height expects first arg as string".to_string()),
            )?;
            let font_size = if args.len() == 2 {
                to_number(
                    &args[1],
                    error("text_height optional second arg must be numeric font size".to_string()),
                )?
            } else {
                16.0
            };
            let line_count = text.lines().count().max(1) as f64;
            Ok(ExpressionValue::Number(
                line_count * font_size.max(1.0) * 1.2,
            ))
        }
        BuiltinFn::TextWidth => {
            if args.is_empty() || args.len() > 2 {
                return Err(error("text_width expects 1 or 2 arguments".to_string()));
            }
            let text = to_string(
                &args[0],
                error("text_width expects first arg as string".to_string()),
            )?;
            let font_size = if args.len() == 2 {
                to_number(
                    &args[1],
                    error("text_width optional second arg must be numeric font size".to_string()),
                )?
            } else {
                16.0
            };
            let avg_glyph_width = font_size.max(1.0) * 0.6;
            let char_count = text.chars().count().max(1) as f64;
            let width = (char_count * avg_glyph_width).clamp(0.0, f64::from(ctx.width));
            Ok(ExpressionValue::Number(width))
        }
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

fn unary_numeric(
    args: &[ExpressionValue],
    expect_len: &dyn Fn(usize) -> Result<(), LumenError>,
    f: impl Fn(f64) -> f64,
) -> Result<ExpressionValue, LumenError> {
    expect_len(1)?;
    let value = to_number(
        &args[0],
        LumenError::Expression(ExpressionError::Evaluate {
            node_id: None,
            property_path: None,
            details: "builtin expects numeric argument".to_string(),
        }),
    )?;
    Ok(ExpressionValue::Number(f(value)))
}

fn to_number(value: &ExpressionValue, error: LumenError) -> Result<f64, LumenError> {
    match value {
        ExpressionValue::Number(number) => Ok(*number),
        ExpressionValue::Boolean(boolean) => Ok(if *boolean { 1.0 } else { 0.0 }),
        ExpressionValue::String(text) => text.parse::<f64>().map_err(|_| error),
    }
}

fn to_string(value: &ExpressionValue, _error: LumenError) -> Result<String, LumenError> {
    match value {
        ExpressionValue::String(text) => Ok(text.clone()),
        ExpressionValue::Number(number) => Ok(number.to_string()),
        ExpressionValue::Boolean(boolean) => Ok(boolean.to_string()),
    }
}
