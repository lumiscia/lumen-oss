use crate::{
    error::{ExpressionError, LumenError},
    expr::{
        ast::{BinaryOp, ExprNode, Expression, ExpressionValue, GlobalVar, UnaryOp},
        builtins::{evaluate_builtin, evaluate_text_measure_builtin},
        ExpressionContext,
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
                UnaryOp::Neg => Ok(ExpressionValue::Number(-to_number(&evaluated, ctx)?)),
                UnaryOp::Not => Ok(ExpressionValue::Boolean(!to_boolean(&evaluated))),
            }
        }
        ExprNode::Binary(left, op, right) => {
            let lhs = evaluate_expr(left, ctx)?;
            match op {
                BinaryOp::And => {
                    if !to_boolean(&lhs) {
                        return Ok(ExpressionValue::Boolean(false));
                    }
                    let rhs = evaluate_expr(right, ctx)?;
                    Ok(ExpressionValue::Boolean(to_boolean(&rhs)))
                }
                BinaryOp::Or => {
                    if to_boolean(&lhs) {
                        return Ok(ExpressionValue::Boolean(true));
                    }
                    let rhs = evaluate_expr(right, ctx)?;
                    Ok(ExpressionValue::Boolean(to_boolean(&rhs)))
                }
                BinaryOp::Eq => {
                    let rhs = evaluate_expr(right, ctx)?;
                    Ok(ExpressionValue::Boolean(lhs == rhs))
                }
                BinaryOp::Neq => {
                    let rhs = evaluate_expr(right, ctx)?;
                    Ok(ExpressionValue::Boolean(lhs != rhs))
                }
                BinaryOp::Add
                | BinaryOp::Sub
                | BinaryOp::Mul
                | BinaryOp::Div
                | BinaryOp::Mod
                | BinaryOp::Gt
                | BinaryOp::Lt
                | BinaryOp::Gte
                | BinaryOp::Lte => {
                    let rhs = evaluate_expr(right, ctx)?;
                    let lhs_num = to_number(&lhs, ctx)?;
                    let rhs_num = to_number(&rhs, ctx)?;
                    match op {
                        BinaryOp::Add => Ok(ExpressionValue::Number(lhs_num + rhs_num)),
                        BinaryOp::Sub => Ok(ExpressionValue::Number(lhs_num - rhs_num)),
                        BinaryOp::Mul => Ok(ExpressionValue::Number(lhs_num * rhs_num)),
                        BinaryOp::Div => {
                            if rhs_num.abs() <= f64::EPSILON {
                                return Err(LumenError::Expression(ExpressionError::Evaluate {
                                    path: ctx.path.clone(),
                                    details: "division by zero".to_string(),
                                }));
                            }
                            Ok(ExpressionValue::Number(lhs_num / rhs_num))
                        }
                        BinaryOp::Mod => {
                            if rhs_num.abs() <= f64::EPSILON {
                                return Err(LumenError::Expression(ExpressionError::Evaluate {
                                    path: ctx.path.clone(),
                                    details: "modulo by zero".to_string(),
                                }));
                            }
                            Ok(ExpressionValue::Number(lhs_num % rhs_num))
                        }
                        BinaryOp::Gt => Ok(ExpressionValue::Boolean(lhs_num > rhs_num)),
                        BinaryOp::Lt => Ok(ExpressionValue::Boolean(lhs_num < rhs_num)),
                        BinaryOp::Gte => Ok(ExpressionValue::Boolean(lhs_num >= rhs_num)),
                        BinaryOp::Lte => Ok(ExpressionValue::Boolean(lhs_num <= rhs_num)),
                        _ => unreachable!(),
                    }
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
            GlobalVar::Frame => Ok(ExpressionValue::Number(f64::from(ctx.frame))),
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
            if to_boolean(&condition) {
                evaluate_expr(when_true, ctx)
            } else {
                evaluate_expr(when_false, ctx)
            }
        }
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
    }
}

fn to_boolean(value: &ExpressionValue) -> bool {
    match value {
        ExpressionValue::Boolean(boolean) => *boolean,
        ExpressionValue::Number(number) => number.abs() > f64::EPSILON,
        ExpressionValue::String(text) => !text.is_empty(),
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
        node::{source::text::Text, NodeId, NodeKind, PropertyValue},
    };

    fn test_context() -> ExpressionContext<'static> {
        ExpressionContext {
            frame: 48,
            fps: 24.0,
            width: 1920,
            height: 1080,
            duration_frames: 240,
            path: Some("node.opacity".to_string()),
            graph: None,
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
    fn converts_supported_node_properties() {
        assert_eq!(
            property_value_to_expression_value(&PropertyValue::Int(7)).unwrap(),
            ExpressionValue::Number(7.0)
        );
        assert_eq!(
            property_value_to_expression_value(&PropertyValue::Bool(true)).unwrap(),
            ExpressionValue::Boolean(true)
        );
    }

    #[test]
    fn rejects_non_scalar_node_properties() {
        let error = property_value_to_expression_value(&PropertyValue::Color([0, 0, 0, 255]))
            .unwrap_err()
            .to_string();

        assert!(error.contains("cannot convert node property `color`"));
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
                ..Text::default()
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
}
