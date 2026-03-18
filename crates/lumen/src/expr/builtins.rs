use std::cell::RefCell;

use skia_safe::{
    Color, FontMgr, FontStyle,
    font_style::Weight,
    textlayout::{
        FontCollection, ParagraphBuilder, ParagraphStyle, TextStyle as ParagraphTextStyle,
    },
};

use crate::{
    error::{ExpressionError, LumenError},
    expr::{
        ExpressionContext,
        ast::{BuiltinFn, ExpressionValue},
    },
};

thread_local! {
    static EXPR_TEXT_FONT_MGR: RefCell<Option<FontMgr>> = const { RefCell::new(None) };
}

fn with_expr_font_mgr<R>(f: impl FnOnce(&FontMgr) -> R) -> R {
    EXPR_TEXT_FONT_MGR.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let mgr = borrow.get_or_insert_with(FontMgr::default);
        f(mgr)
    })
}

fn measure_text_with_skia(
    text: &str,
    font_size: f64,
    wrap_width: Option<f64>,
    fallback_width: u32,
) -> (f64, f64) {
    let mut paragraph_style = ParagraphStyle::new();
    let mut text_style = ParagraphTextStyle::new();
    text_style.set_font_size(font_size.max(1.0) as f32);
    text_style.set_color(Color::WHITE);
    text_style.set_font_style(FontStyle::new(
        Weight::from(500),
        skia_safe::font_style::Width::NORMAL,
        skia_safe::font_style::Slant::Upright,
    ));
    text_style.set_font_families(&["Helvetica"]);
    paragraph_style.set_text_style(&text_style);

    let layout_width = wrap_width
        .unwrap_or(16_384.0)
        .max(1.0)
        .min(f64::from(u32::MAX)) as f32;

    with_expr_font_mgr(|font_mgr| {
        let mut font_collection = FontCollection::new();
        font_collection.set_default_font_manager(font_mgr.clone(), None);
        let mut builder = ParagraphBuilder::new(&paragraph_style, font_collection);
        builder.push_style(&text_style);
        builder.add_text(text);
        let mut paragraph = builder.build();
        paragraph.layout(layout_width);

        let width = if wrap_width.is_some() {
            paragraph.longest_line()
        } else {
            paragraph.max_intrinsic_width()
        }
        .max(1.0)
        .min(fallback_width.max(1) as f32);
        let height = paragraph.height().max(1.0);
        (f64::from(width), f64::from(height))
    })
}

pub fn evaluate_builtin(
    builtin: BuiltinFn,
    args: &[ExpressionValue],
    ctx: &ExpressionContext,
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
        BuiltinFn::TextHeight => {
            if args.is_empty() || args.len() > 3 {
                return Err(error("text_height expects 1 to 3 arguments".to_string()));
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
            } else if args.len() == 3 {
                to_number(
                    &args[1],
                    error("text_height optional second arg must be numeric font size".to_string()),
                )?
            } else {
                16.0
            };
            let wrap_width = if args.len() == 3 {
                Some(to_number(
                    &args[2],
                    error("text_height optional third arg must be numeric max width".to_string()),
                )?)
            } else {
                None
            };
            let (_, height) = measure_text_with_skia(&text, font_size, wrap_width, ctx.width);
            Ok(ExpressionValue::Number(height))
        }
        BuiltinFn::TextWidth => {
            if args.is_empty() || args.len() > 3 {
                return Err(error("text_width expects 1 to 3 arguments".to_string()));
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
            } else if args.len() == 3 {
                to_number(
                    &args[1],
                    error("text_width optional second arg must be numeric font size".to_string()),
                )?
            } else {
                16.0
            };
            let wrap_width = if args.len() == 3 {
                Some(to_number(
                    &args[2],
                    error("text_width optional third arg must be numeric max width".to_string()),
                )?)
            } else {
                None
            };
            let (width, _) = measure_text_with_skia(&text, font_size, wrap_width, ctx.width);
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
