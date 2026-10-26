use crate::{
    error::{ExpressionError, LumenError},
    expr::{
        ExpressionContext,
        ast::{BinaryOp, ExprNode, Expression, ExpressionValue, GlobalVar, UnaryOp},
        builtins::{evaluate_builtin, evaluate_text_measure_builtin},
    },
    node::{PropertyExpression, PropertyValue},
};

impl Expression {
    pub fn evaluate(&self, ctx: &ExpressionContext<'_>) -> crate::Result<ExpressionValue> {
        evaluate_expr(&self.ast, ctx)
    }
}

pub fn property_value_to_expression_value(value: &PropertyValue) -> crate::Result<ExpressionValue> {
    match value {
        PropertyValue::Float(number) => Ok(ExpressionValue::Number(*number)),
        PropertyValue::Int(number) => Ok(ExpressionValue::Number(*number as f64)),
        PropertyValue::Bool(boolean) => Ok(ExpressionValue::Boolean(*boolean)),
        PropertyValue::String(text) => Ok(ExpressionValue::String(text.clone())),
        PropertyValue::Vec2(value) => Ok(ExpressionValue::Vec2(*value)),
        PropertyValue::Color(value) => Ok(ExpressionValue::color_from_u8(*value)),
        unsupported => Err(LumenError::Expression(ExpressionError::Evaluate {
            path: None,
            details: format!(
                "cannot convert node property `{}` into an expression value",
                node_property_type_name(unsupported)
            ),
        })),
    }
}

pub(crate) fn evaluate_expr(
    expr: &ExprNode,
    ctx: &ExpressionContext<'_>,
) -> crate::Result<ExpressionValue> {
    match expr {
        ExprNode::Literal(value) => Ok(value.clone()),
        ExprNode::Unary(op, value) => {
            let evaluated = evaluate_expr(value, ctx)?;
            match op {
                UnaryOp::Neg => match evaluated {
                    ExpressionValue::Vec2((x, y)) => Ok(ExpressionValue::Vec2((-x, -y))),
                    other => Ok(ExpressionValue::Number(-to_number(&other, ctx)?)),
                },
                UnaryOp::Not => Ok(ExpressionValue::Boolean(!evaluated.as_bool())),
            }
        }
        ExprNode::Binary(left, op, right) => {
            let lhs = evaluate_expr(left, ctx)?;
            match op {
                BinaryOp::And => {
                    if !lhs.as_bool() {
                        return Ok(ExpressionValue::Boolean(false));
                    }
                    let rhs = evaluate_expr(right, ctx)?;
                    Ok(ExpressionValue::Boolean(rhs.as_bool()))
                }
                BinaryOp::Or => {
                    if lhs.as_bool() {
                        return Ok(ExpressionValue::Boolean(true));
                    }
                    let rhs = evaluate_expr(right, ctx)?;
                    Ok(ExpressionValue::Boolean(rhs.as_bool()))
                }
                BinaryOp::Eq => {
                    let rhs = evaluate_expr(right, ctx)?;
                    Ok(ExpressionValue::Boolean(lhs == rhs))
                }
                BinaryOp::Neq => {
                    let rhs = evaluate_expr(right, ctx)?;
                    Ok(ExpressionValue::Boolean(lhs != rhs))
                }
                BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => {
                    let rhs = evaluate_expr(right, ctx)?;
                    arithmetic(*op, &lhs, &rhs, ctx)
                }
                BinaryOp::Gt | BinaryOp::Lt | BinaryOp::Gte | BinaryOp::Lte => {
                    let rhs = evaluate_expr(right, ctx)?;
                    let (lhs, rhs) = (to_number(&lhs, ctx)?, to_number(&rhs, ctx)?);
                    Ok(ExpressionValue::Boolean(match op {
                        BinaryOp::Gt => lhs > rhs,
                        BinaryOp::Lt => lhs < rhs,
                        BinaryOp::Gte => lhs >= rhs,
                        _ => lhs <= rhs,
                    }))
                }
            }
        }
        ExprNode::Builtin(builtin, args) => {
            if matches!(
                builtin,
                crate::expr::ast::BuiltinFn::TextHeight | crate::expr::ast::BuiltinFn::TextWidth
            ) {
                return evaluate_text_measure_builtin(*builtin, args, ctx);
            }
            let mut evaluated_args = Vec::with_capacity(args.len());
            for arg in args {
                evaluated_args.push(evaluate_expr(arg, ctx)?);
            }
            evaluate_builtin(*builtin, &evaluated_args, ctx)
        }
        ExprNode::Global(global) => match global {
            GlobalVar::Frame => Ok(ExpressionValue::Number(ctx.frame)),
            GlobalVar::Time => Ok(ExpressionValue::Number(ctx.time_seconds())),
            GlobalVar::Fps => Ok(ExpressionValue::Number(f64::from(ctx.fps))),
            GlobalVar::Width => Ok(ExpressionValue::Number(f64::from(ctx.width))),
            GlobalVar::Height => Ok(ExpressionValue::Number(f64::from(ctx.height))),
            GlobalVar::Custom(name) => {
                Err(LumenError::Expression(ExpressionError::UndefinedVariable {
                    path: ctx.path.clone(),
                    name: name.clone(),
                }))
            }
        },
        ExprNode::SymbolicPath(segments) => {
            Err(LumenError::Expression(ExpressionError::Evaluate {
                path: ctx.path.clone(),
                details: format!(
                    "unresolved symbolic property reference `{}`",
                    segments.join(".")
                ),
            }))
        }
        ExprNode::Node(node_id) => Err(LumenError::Expression(ExpressionError::Evaluate {
            path: ctx.path.clone(),
            details: format!(
                "node reference `{}` can only be used in builtins that accept node references",
                node_id.0
            ),
        })),
        ExprNode::PropertyValue(node_id, target_path) => {
            let graph = ctx.graph.ok_or_else(|| {
                LumenError::Expression(ExpressionError::Evaluate {
                    path: ctx.path.clone(),
                    details: format!(
                        "no graph available to resolve node property reference `{}`",
                        target_path.0
                    ),
                })
            })?;
            let node = graph.nodes.get(node_id).ok_or_else(|| {
                LumenError::Expression(ExpressionError::Evaluate {
                    path: ctx.path.clone(),
                    details: format!(
                        "node `{}` not found for property reference `{}`",
                        node_id.0, target_path.0
                    ),
                })
            })?;
            let prop = node
                .as_property_eval()
                .get_property(&target_path.0)?
                .ok_or_else(|| {
                    LumenError::Expression(ExpressionError::Evaluate {
                        path: ctx.path.clone(),
                        details: format!(
                            "property `{}` not found on node `{}`",
                            target_path.0, node_id.0
                        ),
                    })
                })?;
            match &prop {
                PropertyExpression::Expr(inner_expr) => inner_expr.evaluate(ctx),
                PropertyExpression::Value(value) => property_value_to_expression_value(value),
            }
        }
        ExprNode::VirtualProperty(id) => Err(LumenError::Expression(ExpressionError::Evaluate {
            path: ctx.path.clone(),
            details: format!("unresolved virtual property reference `{}`", id.0),
        })),
        ExprNode::Conditional(condition, when_true, when_false) => {
            let condition = evaluate_expr(condition, ctx)?;
            if condition.as_bool() {
                evaluate_expr(when_true, ctx)
            } else {
                evaluate_expr(when_false, ctx)
            }
        }
    }
}

/// Numbers combine as usual. Two vec2 values combine componentwise, and a vec2 can be
/// multiplied or divided by a number on either side. Colours have no arithmetic.
fn arithmetic(
    op: BinaryOp,
    lhs: &ExpressionValue,
    rhs: &ExpressionValue,
    ctx: &ExpressionContext<'_>,
) -> crate::Result<ExpressionValue> {
    let error = |details: String| {
        LumenError::Expression(ExpressionError::Evaluate {
            path: ctx.path.clone(),
            details,
        })
    };
    let apply = |a: f64, b: f64| match op {
        BinaryOp::Add => Ok(a + b),
        BinaryOp::Sub => Ok(a - b),
        BinaryOp::Mul => Ok(a * b),
        BinaryOp::Div if b.abs() <= f64::EPSILON => Err(error("division by zero".to_string())),
        BinaryOp::Div => Ok(a / b),
        BinaryOp::Mod if b.abs() <= f64::EPSILON => Err(error("modulo by zero".to_string())),
        _ => Ok(a % b),
    };
    let scales = matches!(op, BinaryOp::Mul | BinaryOp::Div);
    let (a, b) = match (lhs, rhs) {
        (ExpressionValue::Vec2(a), ExpressionValue::Vec2(b)) if op != BinaryOp::Mod => (*a, *b),
        (ExpressionValue::Vec2(a), other) if scales => {
            let scalar = to_number(other, ctx)?;
            (*a, (scalar, scalar))
        }
        (other, ExpressionValue::Vec2(b)) if scales => {
            let scalar = to_number(other, ctx)?;
            ((scalar, scalar), *b)
        }
        (ExpressionValue::Vec2(_) | ExpressionValue::Color(_), _)
        | (_, ExpressionValue::Vec2(_) | ExpressionValue::Color(_)) => {
            return Err(error(format!(
                "cannot apply `{}` to {} and {}",
                operator_symbol(op),
                lhs.type_name(),
                rhs.type_name()
            )));
        }
        _ => {
            let value = apply(to_number(lhs, ctx)?, to_number(rhs, ctx)?)?;
            return Ok(ExpressionValue::Number(value));
        }
    };
    Ok(ExpressionValue::Vec2((apply(a.0, b.0)?, apply(a.1, b.1)?)))
}

fn operator_symbol(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "%",
        BinaryOp::Gt => ">",
        BinaryOp::Lt => "<",
        BinaryOp::Gte => ">=",
        BinaryOp::Lte => "<=",
        BinaryOp::Eq => "==",
        BinaryOp::Neq => "!=",
        BinaryOp::And => "&&",
        BinaryOp::Or => "||",
    }
}

fn to_number(value: &ExpressionValue, ctx: &ExpressionContext<'_>) -> crate::Result<f64> {
    match value {
        ExpressionValue::Number(number) => Ok(*number),
        ExpressionValue::Boolean(boolean) => Ok(if *boolean { 1.0 } else { 0.0 }),
        ExpressionValue::String(text) => text.parse::<f64>().map_err(|_| {
            LumenError::Expression(ExpressionError::Parse {
                path: ctx.path.clone(),
                details: format!("cannot convert `{text}` into f64"),
            })
        }),
        ExpressionValue::Vec2(_) | ExpressionValue::Color(_) => {
            Err(LumenError::Expression(ExpressionError::Evaluate {
                path: ctx.path.clone(),
                details: format!("cannot convert {} into f64", value.type_name()),
            }))
        }
    }
}

fn node_property_type_name(value: &PropertyValue) -> &'static str {
    match value {
        PropertyValue::Float(_) => "float",
        PropertyValue::Int(_) => "int",
        PropertyValue::Bool(_) => "bool",
        PropertyValue::String(_) => "string",
        PropertyValue::Color(_) => "color",
        PropertyValue::Paint(_) => "paint",
        PropertyValue::Vec2(_) => "vec2",
        PropertyValue::ShaderUniforms(_) => "shader uniforms",
        PropertyValue::FloatVec(_) => "float[]",
        PropertyValue::IntVec(_) => "int[]",
        PropertyValue::StringVec(_) => "string[]",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        expr::ast::{BuiltinFn, ExprNode, ExpressionId},
        graph::Graph,
        node::{NodeId, NodeKind, PropertyValue, source::text::Text},
    };

    fn test_context() -> ExpressionContext<'static> {
        ExpressionContext {
            frame: 48.0,
            fps: 24.0,
            width: 1920,
            height: 1080,
            duration_frames: 240,
            path: Some("node.opacity".to_string()),
            graph: None,
            media: None,
        }
    }

    #[test]
    fn evaluates_globals_from_expression_context() {
        let ctx = test_context();

        assert_eq!(
            evaluate_expr(&ExprNode::Global(GlobalVar::Time), &ctx).unwrap(),
            ExpressionValue::Number(2.0)
        );
        assert_eq!(
            evaluate_expr(&ExprNode::Global(GlobalVar::Fps), &ctx).unwrap(),
            ExpressionValue::Number(24.0)
        );
        assert_eq!(
            evaluate_expr(&ExprNode::Global(GlobalVar::Width), &ctx).unwrap(),
            ExpressionValue::Number(1920.0)
        );
        assert_eq!(
            evaluate_expr(&ExprNode::Global(GlobalVar::Height), &ctx).unwrap(),
            ExpressionValue::Number(1080.0)
        );
    }

    #[test]
    fn evaluates_fractional_frame_and_time() {
        let ctx = ExpressionContext {
            frame: 1.5,
            ..test_context()
        };

        let expression = Expression::parse("vec2(frame * 2, time * 24)").unwrap();
        assert_eq!(
            expression.evaluate(&ctx).unwrap(),
            ExpressionValue::Vec2((3.0, 1.5))
        );
    }

    #[test]
    fn integer_frames_evaluate_to_whole_numbers() {
        let ctx = test_context();

        let expression = Expression::parse("vec2(frame * 2, time)").unwrap();
        assert_eq!(
            expression.evaluate(&ctx).unwrap(),
            ExpressionValue::Vec2((96.0, 2.0))
        );
    }

    #[test]
    fn subframe_offsets_frame_and_time() {
        let ctx = test_context().at_subframe(-0.25);

        assert_eq!(
            evaluate_expr(&ExprNode::Global(GlobalVar::Frame), &ctx).unwrap(),
            ExpressionValue::Number(47.75)
        );
        assert_eq!(
            evaluate_expr(&ExprNode::Global(GlobalVar::Time), &ctx).unwrap(),
            ExpressionValue::Number(47.75 / 24.0)
        );
    }

    #[test]
    fn converts_supported_node_properties() {
        assert_eq!(
            property_value_to_expression_value(&PropertyValue::Int(7)).unwrap(),
            ExpressionValue::Number(7.0)
        );
        assert_eq!(
            property_value_to_expression_value(&PropertyValue::Bool(true)).unwrap(),
            ExpressionValue::Boolean(true)
        );
        assert_eq!(
            property_value_to_expression_value(&PropertyValue::Vec2((10.0, 20.0))).unwrap(),
            ExpressionValue::Vec2((10.0, 20.0))
        );
        assert_eq!(
            property_value_to_expression_value(&PropertyValue::Color([255, 0, 0, 255])).unwrap(),
            ExpressionValue::Color([1.0, 0.0, 0.0, 1.0])
        );
    }

    #[test]
    fn rejects_non_scalar_node_properties() {
        let error = property_value_to_expression_value(&PropertyValue::FloatVec(vec![1.0]))
            .unwrap_err()
            .to_string();

        assert!(error.contains("cannot convert node property `float[]`"));
    }

    #[test]
    fn evaluates_linear_and_step_builtins() {
        let ctx = test_context();
        let expression = Expression {
            id: ExpressionId(1),
            ast: ExprNode::Builtin(
                BuiltinFn::Linear,
                vec![
                    ExprNode::Literal(ExpressionValue::Number(10.0)),
                    ExprNode::Literal(ExpressionValue::Number(20.0)),
                    ExprNode::Literal(ExpressionValue::Number(0.25)),
                ],
            ),
            references: Vec::new(),
            source: "linear(10, 20, 0.25)".to_string(),
        };
        let stepped = Expression {
            id: ExpressionId(2),
            ast: ExprNode::Builtin(
                BuiltinFn::Step,
                vec![
                    ExprNode::Literal(ExpressionValue::Number(10.0)),
                    ExprNode::Literal(ExpressionValue::Number(20.0)),
                    ExprNode::Literal(ExpressionValue::Number(0.5)),
                ],
            ),
            references: Vec::new(),
            source: "step(10, 20, 0.5)".to_string(),
        };

        assert_eq!(
            expression.evaluate(&ctx).unwrap(),
            ExpressionValue::Number(12.5)
        );
        assert_eq!(
            stepped.evaluate(&ctx).unwrap(),
            ExpressionValue::Number(10.0)
        );
    }

    #[test]
    fn evaluates_vec2_components_independently() {
        let expression = Expression::parse("vec2(frame * 2, time + 3)").unwrap();

        assert_eq!(
            expression.evaluate(&test_context()).unwrap(),
            ExpressionValue::Vec2((96.0, 5.0))
        );
    }

    #[test]
    fn vec2_requires_exactly_two_components() {
        for source in ["vec2()", "vec2(1)", "vec2(1, 2, 3)"] {
            let ExpressionError::Parse { details, .. } = Expression::parse(source).unwrap_err()
            else {
                panic!("expected parse error");
            };
            assert!(details.contains("`vec2`: expects 2 arguments"), "{details}");
        }
    }

    fn number_at(source: &str, frame: u32) -> f64 {
        let ctx = ExpressionContext {
            frame: f64::from(frame),
            fps: 30.0,
            ..test_context()
        };
        match Expression::parse(source).unwrap().evaluate(&ctx).unwrap() {
            ExpressionValue::Number(value) => value,
            other => panic!("expected a number, got {other:?}"),
        }
    }

    #[test]
    fn evaluates_issue_easing_examples() {
        let lerp = "lerp(1200, 540, out_expo(progress(0, 18)))";
        let tween = "tween(1200, 540, 0, 18, 'out_expo')";
        assert_eq!(number_at(lerp, 0), 1200.0);
        assert_eq!(number_at(lerp, 18), 540.0);
        assert_eq!(number_at(lerp, 40), 540.0);
        assert_eq!(number_at(lerp, 9), 1200.0 - 660.0 * (1.0 - 2f64.powi(-5)));
        for frame in [0, 3, 9, 17, 18, 30] {
            assert_eq!(number_at(lerp, frame), number_at(tween, frame));
        }
        assert_eq!(number_at("tween(0, 10, 4, 10)", 9), 5.0);
    }

    #[test]
    fn unit_suffixes_convert_seconds_to_frames() {
        let source = "tween(0, 1, 1.5s, 300ms, 'out_expo')";
        assert_eq!(number_at(source, 45), 0.0);
        assert_eq!(number_at(source, 54), 1.0);
        assert_eq!(number_at(source, 48), 1.0 - 2f64.powf(-10.0 / 3.0));
        assert_eq!(number_at("2s + 500ms", 0), 75.0);

        let ExpressionError::Parse { details, .. } = Expression::parse("1.5px").unwrap_err() else {
            panic!("expected parse error");
        };
        assert!(details.contains("unknown unit `px`"), "{details}");
    }

    #[test]
    fn progress_steps_when_the_range_is_empty() {
        assert_eq!(number_at("progress(10, 10)", 9), 0.0);
        assert_eq!(number_at("progress(10, 10)", 10), 1.0);
        assert_eq!(number_at("progress(10, 5)", 12), 1.0);
        assert_eq!(number_at("progress(10, 20)", 15), 0.5);
    }

    #[test]
    fn spring_and_cubic_bezier_evaluate() {
        assert_eq!(number_at("spring((frame - 12) / fps)", 6), 0.0);
        assert!((number_at("spring((frame - 12) / fps, 170, 26)", 60) - 1.0).abs() < 1e-3);
        assert!((number_at("cubic_bezier(0, 0, 1, 1, 0.25)", 0) - 0.25).abs() < 1e-6);
        let error = Expression::parse("spring(1, 0 - 1)")
            .unwrap()
            .evaluate(&test_context())
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("stiffness must be greater than 0")
        );
    }

    #[test]
    fn rejects_bad_easing_arguments_at_parse_time() {
        for (source, expected) in [
            ("tween(0, 1, 0, 10, 'bounce')", "unknown easing `bounce`"),
            ("tween(0, 1, 0, 10, frame)", "must be a quoted easing name"),
            ("tween(0, 1, 0)", "expects 4 to 5 arguments, got 3"),
            ("spring(1, 0)", "stiffness must be greater than 0"),
            ("spring(1, 170, -0.5 * 2)", ""),
        ] {
            match Expression::parse(source) {
                Err(ExpressionError::Parse { details, .. }) if !expected.is_empty() => {
                    assert!(details.contains(expected), "{source}: {details}")
                }
                Ok(_) if expected.is_empty() => {}
                other => panic!("{source}: {other:?}"),
            }
        }
    }

    #[test]
    fn vec2_coerces_boolean_and_numeric_string_components() {
        for (source, expected) in [
            ("vec2(true, false)", (1.0, 0.0)),
            ("vec2(\"12.5\", \"-3\")", (12.5, -3.0)),
            ("vec2(true, \"2.25\")", (1.0, 2.25)),
        ] {
            assert_eq!(
                Expression::parse(source)
                    .unwrap()
                    .evaluate(&test_context())
                    .unwrap(),
                ExpressionValue::Vec2(expected)
            );
        }
    }

    #[test]
    fn vec2_rejects_invalid_component_types() {
        for source in ["vec2(\"left\", 2)", "vec2(1, vec2(2, 3))"] {
            let error = Expression::parse(source)
                .unwrap()
                .evaluate(&test_context())
                .expect_err("nonnumeric component must fail");

            let LumenError::Expression(ExpressionError::Evaluate { path, details }) = error else {
                panic!("expected expression evaluation error");
            };
            assert_eq!(path.as_deref(), Some("node.opacity"));
            assert_eq!(details, "vec2 expects numeric arguments");
        }
    }

    fn evaluate(source: &str, frame: u32) -> crate::Result<ExpressionValue> {
        let ctx = ExpressionContext {
            frame: f64::from(frame),
            ..test_context()
        };
        Expression::parse(source).unwrap().evaluate(&ctx)
    }

    #[test]
    fn vec2_arithmetic_is_componentwise() {
        for (source, expected) in [
            ("vec2(1, 2) + vec2(10, 20)", (11.0, 22.0)),
            ("vec2(1, 2) - vec2(10, 20)", (-9.0, -18.0)),
            ("-vec2(1, -2)", (-1.0, 2.0)),
            ("vec2(1, 2) * 3", (3.0, 6.0)),
            ("3 * vec2(1, 2)", (3.0, 6.0)),
            ("vec2(4, 8) / 2", (2.0, 4.0)),
            ("8 / vec2(4, 2)", (2.0, 4.0)),
            ("vec2(2, 3) * vec2(4, 5)", (8.0, 15.0)),
            ("vec2(8, 15) / vec2(4, 5)", (2.0, 3.0)),
            ("lerp(vec2(0, 10), vec2(10, 30), 0.25)", (2.5, 15.0)),
        ] {
            assert_eq!(
                evaluate(source, 0).unwrap(),
                ExpressionValue::Vec2(expected),
                "{source}"
            );
        }
    }

    #[test]
    fn vec2_rejects_mismatched_arithmetic() {
        for (source, expected) in [
            ("vec2(1, 2) + 3", "cannot apply `+` to vec2 and number"),
            ("3 - vec2(1, 2)", "cannot apply `-` to number and vec2"),
            ("vec2(1, 2) % 2", "cannot apply `%` to vec2 and number"),
            (
                "vec2(1, 2) % vec2(1, 2)",
                "cannot apply `%` to vec2 and vec2",
            ),
            ("vec2(1, 2) / 0", "division by zero"),
            ("vec2(1, 2) > 1", "cannot convert vec2 into f64"),
            (
                "lerp(vec2(1, 2), 3, 0.5)",
                "`lerp` expects `from` and `to` of the same type",
            ),
        ] {
            let LumenError::Expression(ExpressionError::Evaluate { path, details }) =
                evaluate(source, 0).unwrap_err()
            else {
                panic!("expected expression evaluation error");
            };
            assert_eq!(path.as_deref(), Some("node.opacity"));
            assert!(details.starts_with(expected), "{source}: {details}");
        }
    }

    #[test]
    fn evaluates_issue_vec2_tween_sum() {
        let source = "vec2(960, 1200) \
            + tween(vec2(0, 0), vec2(0, -660), 0, 18, 'out_expo') \
            + tween(vec2(0, 0), vec2(-1360, 0), 90, 14, 'in_back')";
        for (frame, expected) in [
            (0, (960.0, 1200.0)),
            (18, (960.0, 540.0)),
            (90, (960.0, 540.0)),
            (104, (-400.0, 540.0)),
        ] {
            assert_eq!(
                evaluate(source, frame).unwrap(),
                ExpressionValue::Vec2(expected),
                "frame {frame}"
            );
        }
        let ExpressionValue::Vec2((x, _)) = evaluate(source, 93).unwrap() else {
            panic!("expected vec2");
        };
        assert!(x > 960.0, "in_back pulls back before leaving: {x}");
    }

    #[test]
    fn colour_literals_constructors_and_tweens() {
        for (source, expected) in [
            ("#ff8000", [1.0, 128.0 / 255.0, 0.0, 1.0]),
            ("#FF800080", [1.0, 128.0 / 255.0, 0.0, 128.0 / 255.0]),
            ("rgb(255, 0, 51)", [1.0, 0.0, 0.2, 1.0]),
            ("rgba(0, 0, 0, 0)", [0.0; 4]),
            ("lerp(#000000, #ffffff, 0.5)", [0.5, 0.5, 0.5, 1.0]),
            ("tween(#ff0000, #0000ff, 40, 16)", [0.5, 0.0, 0.5, 1.0]),
            ("if(frame > 10, #00ff00, #ff0000)", [0.0, 1.0, 0.0, 1.0]),
        ] {
            assert_eq!(
                evaluate(source, 48).unwrap(),
                ExpressionValue::Color(expected),
                "{source}"
            );
        }
    }

    #[test]
    fn colour_errors() {
        for (source, expected) in [
            ("#ff00", "invalid colour `#ff00`"),
            ("#gg0000", "invalid colour `#gg0000`"),
            (
                "rgb(256, 0, 0)",
                "colour components must be between 0 and 255",
            ),
            ("rgb(1, 2)", "expects 3 arguments"),
        ] {
            let ExpressionError::Parse { details, .. } = Expression::parse(source).unwrap_err()
            else {
                panic!("{source}: expected parse error");
            };
            assert!(details.contains(expected), "{source}: {details}");
        }
        for (source, expected) in [
            ("#ff0000 + #00ff00", "cannot apply `+` to color and color"),
            ("#ff0000 * 2", "cannot apply `*` to color and number"),
            ("-#ff0000", "cannot convert color into f64"),
            (
                "rgb(frame * 10, 0, 0)",
                "colour components must be between 0 and 255",
            ),
            (
                "lerp(#ff0000, vec2(0, 0), 0.5)",
                "expects `from` and `to` of the same type",
            ),
        ] {
            let error = evaluate(source, 48).unwrap_err().to_string();
            assert!(error.contains(expected), "{source}: {error}");
        }
    }

    #[test]
    fn text_measure_builtins_use_explicit_text_inputs() {
        let ctx = test_context();

        let implicit = Expression::parse("text_width(\"Morning, update posted.\")").unwrap();
        let explicit =
            Expression::parse("text_width(node(8, \"content\"), 32, 300, \"Roboto\")").unwrap();
        let implicit_height =
            Expression::parse("text_height(\"Morning, update posted.\")").unwrap();
        let explicit_height =
            Expression::parse("text_height(node(8, \"content\"), 32, 300, \"Roboto\")").unwrap();

        assert!(implicit.evaluate(&ctx).is_ok());
        assert!(explicit.evaluate(&ctx).is_err());
        assert!(implicit_height.evaluate(&ctx).is_ok());
        assert!(explicit_height.evaluate(&ctx).is_err());
    }

    #[test]
    fn text_measure_builtins_resolve_text_nodes_from_graph() {
        let text_id = NodeId::new(8);
        let mut graph = Graph::new();
        graph.nodes.insert(
            text_id,
            NodeKind::Text(Text {
                id: text_id,
                params: crate::node::source::text::TextParamsDelegate {
                    content: crate::node::Deferred::value("Morning, update posted.".to_string()),
                    font_family: crate::node::Deferred::value("Roboto".to_string()),
                    font_size: crate::node::Deferred::value(32.0),
                    max_width: crate::node::Deferred::value(300.0),
                    ..Default::default()
                },
            }),
        );
        let ctx = ExpressionContext {
            graph: Some(&graph),
            ..test_context()
        };

        let width = Expression::parse("text_width(node(8))")
            .unwrap()
            .evaluate(&ctx)
            .unwrap();
        let height = Expression::parse("text_height(node(8))")
            .unwrap()
            .evaluate(&ctx)
            .unwrap();

        assert!(matches!(width, ExpressionValue::Number(value) if value > 0.0));
        assert!(matches!(height, ExpressionValue::Number(value) if value > 32.0));
    }

    #[test]
    fn text_measure_builtins_evaluate_expression_backed_text_properties() {
        let text_id = NodeId::new(8);
        let mut graph = Graph::new();
        graph.nodes.insert(
            text_id,
            NodeKind::Text(Text {
                id: text_id,
                params: crate::node::source::text::TextParamsDelegate {
                    content: crate::node::Deferred::Expr(
                        Expression::parse("\"Frame two\"").unwrap(),
                    ),
                    font_family: crate::node::Deferred::Expr(
                        Expression::parse("\"Roboto\"").unwrap(),
                    ),
                    font_size: crate::node::Deferred::Expr(
                        Expression::parse("24 + frame").unwrap(),
                    ),
                    max_width: crate::node::Deferred::Expr(Expression::parse("300").unwrap()),
                    ..Default::default()
                },
            }),
        );
        let ctx = ExpressionContext {
            frame: 2.0,
            graph: Some(&graph),
            ..test_context()
        };

        let result = Expression::parse("text_width(node(8))")
            .unwrap()
            .evaluate(&ctx);

        assert!(matches!(result, Ok(ExpressionValue::Number(value)) if value > 0.0));
    }

    #[test]
    fn recursive_text_measurement_returns_error_instead_of_panicking() {
        let text_id = NodeId::new(8);
        let mut graph = Graph::new();
        graph.nodes.insert(
            text_id,
            NodeKind::Text(Text {
                id: text_id,
                params: crate::node::source::text::TextParamsDelegate {
                    content: crate::node::Deferred::Expr(
                        Expression::parse("text_width(node(8))").unwrap(),
                    ),
                    ..Default::default()
                },
            }),
        );
        let ctx = ExpressionContext {
            graph: Some(&graph),
            ..test_context()
        };

        let error = Expression::parse("text_width(node(8))")
            .unwrap()
            .evaluate(&ctx)
            .expect_err("recursive text measurement must fail");

        assert!(error.to_string().contains("recursive text measurement"));
    }
}
