use crate::{
    composition::Composition,
    error::{ExpressionError, LumenError},
    expr::{
        ast::{BinaryOp, ExprNode, Expression, ExpressionValue, GlobalVar, UnaryOp},
        builtins::evaluate_builtin,
    },
    node::{NodeId, PropertyValue},
    render::RenderContext,
};

impl Expression {
    pub fn evaluate(&self, ctx: &RenderContext) -> Result<ExpressionValue, LumenError> {
        self.evaluate_with_context(ctx, None, None, None)
    }

    pub fn evaluate_with_context(
        &self,
        ctx: &RenderContext,
        composition: Option<&Composition>,
        node_id: Option<NodeId>,
        property_path: Option<String>,
    ) -> Result<ExpressionValue, LumenError> {
        evaluate_expr(
            &self.ast,
            ctx,
            composition,
            node_id,
            property_path.as_deref(),
        )
    }
}

fn evaluate_expr(
    expr: &ExprNode,
    ctx: &RenderContext,
    composition: Option<&Composition>,
    node_id: Option<NodeId>,
    property_path: Option<&str>,
) -> Result<ExpressionValue, LumenError> {
    match expr {
        ExprNode::Literal(value) => Ok(value.clone()),
        ExprNode::Unary(op, value) => {
            let evaluated = evaluate_expr(value, ctx, composition, node_id, property_path)?;
            match op {
                UnaryOp::Neg => Ok(ExpressionValue::Number(-to_number(
                    &evaluated,
                    node_id,
                    property_path,
                    "unary negation expects numeric operand",
                )?)),
                UnaryOp::Not => Ok(ExpressionValue::Boolean(!to_boolean(&evaluated))),
            }
        }
        ExprNode::Binary(left, op, right) => {
            let lhs = evaluate_expr(left, ctx, composition, node_id, property_path)?;
            match op {
                BinaryOp::And => {
                    if !to_boolean(&lhs) {
                        return Ok(ExpressionValue::Boolean(false));
                    }
                    let rhs = evaluate_expr(right, ctx, composition, node_id, property_path)?;
                    Ok(ExpressionValue::Boolean(to_boolean(&rhs)))
                }
                BinaryOp::Or => {
                    if to_boolean(&lhs) {
                        return Ok(ExpressionValue::Boolean(true));
                    }
                    let rhs = evaluate_expr(right, ctx, composition, node_id, property_path)?;
                    Ok(ExpressionValue::Boolean(to_boolean(&rhs)))
                }
                BinaryOp::Eq => {
                    let rhs = evaluate_expr(right, ctx, composition, node_id, property_path)?;
                    Ok(ExpressionValue::Boolean(lhs == rhs))
                }
                BinaryOp::Neq => {
                    let rhs = evaluate_expr(right, ctx, composition, node_id, property_path)?;
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
                    let rhs = evaluate_expr(right, ctx, composition, node_id, property_path)?;
                    let lhs_num = to_number(
                        &lhs,
                        node_id,
                        property_path,
                        "binary operation expects numeric operands",
                    )?;
                    let rhs_num = to_number(
                        &rhs,
                        node_id,
                        property_path,
                        "binary operation expects numeric operands",
                    )?;
                    match op {
                        BinaryOp::Add => Ok(ExpressionValue::Number(lhs_num + rhs_num)),
                        BinaryOp::Sub => Ok(ExpressionValue::Number(lhs_num - rhs_num)),
                        BinaryOp::Mul => Ok(ExpressionValue::Number(lhs_num * rhs_num)),
                        BinaryOp::Div => {
                            if rhs_num.abs() <= f64::EPSILON {
                                return Err(expression_eval_error(
                                    node_id,
                                    property_path,
                                    "division by zero",
                                ));
                            }
                            Ok(ExpressionValue::Number(lhs_num / rhs_num))
                        }
                        BinaryOp::Mod => {
                            if rhs_num.abs() <= f64::EPSILON {
                                return Err(expression_eval_error(
                                    node_id,
                                    property_path,
                                    "modulo by zero",
                                ));
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
            let mut evaluated_args = Vec::with_capacity(args.len());
            for arg in args {
                evaluated_args.push(evaluate_expr(
                    arg,
                    ctx,
                    composition,
                    node_id,
                    property_path,
                )?);
            }
            evaluate_builtin(
                *builtin,
                &evaluated_args,
                ctx,
                node_id,
                property_path.map(ToString::to_string),
            )
        }
        ExprNode::Global(global) => match global {
            GlobalVar::Frame => Ok(ExpressionValue::Number(f64::from(ctx.frame))),
            GlobalVar::Time => {
                if ctx.fps <= 0.0 {
                    Ok(ExpressionValue::Number(0.0))
                } else {
                    Ok(ExpressionValue::Number(
                        f64::from(ctx.frame) / f64::from(ctx.fps),
                    ))
                }
            }
            GlobalVar::Fps => Ok(ExpressionValue::Number(f64::from(ctx.fps))),
            GlobalVar::Width => Ok(ExpressionValue::Number(f64::from(ctx.width))),
            GlobalVar::Height => Ok(ExpressionValue::Number(f64::from(ctx.height))),
            GlobalVar::Custom(name) => {
                Err(LumenError::Expression(ExpressionError::UndefinedVariable {
                    node_id,
                    property_path: property_path.map(ToString::to_string),
                    name: name.clone(),
                }))
            }
        },
        ExprNode::NodeProperty(target_node_id, target_path) => {
            let Some(composition) = composition else {
                return Err(expression_eval_error(
                    node_id,
                    property_path,
                    "node property reference requires composition context",
                ));
            };
            let value = composition.sample_property_without_expressions(
                *target_node_id,
                &target_path.0,
                ctx.frame,
            )?;
            property_value_to_expression_value(&value)
        }
        ExprNode::Conditional(condition, when_true, when_false) => {
            let condition = evaluate_expr(condition, ctx, composition, node_id, property_path)?;
            if to_boolean(&condition) {
                evaluate_expr(when_true, ctx, composition, node_id, property_path)
            } else {
                evaluate_expr(when_false, ctx, composition, node_id, property_path)
            }
        }
    }
}

pub fn property_value_to_expression_value(
    value: &PropertyValue,
) -> Result<ExpressionValue, LumenError> {
    match value {
        PropertyValue::Float(number) => Ok(ExpressionValue::Number(*number)),
        PropertyValue::Int(number) => Ok(ExpressionValue::Number(*number as f64)),
        PropertyValue::Bool(boolean) => Ok(ExpressionValue::Boolean(*boolean)),
        PropertyValue::String(text) => Ok(ExpressionValue::String(text.clone())),
        PropertyValue::Color(color) => Ok(ExpressionValue::String(format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            color[0], color[1], color[2], color[3]
        ))),
        PropertyValue::Vector2(x, y) => Ok(ExpressionValue::String(format!("{x},{y}"))),
        PropertyValue::Map(_) => Err(expression_eval_error(
            None,
            None,
            "cannot coerce map property value into expression value",
        )),
    }
}

pub fn expression_value_to_property_value(value: &ExpressionValue) -> PropertyValue {
    match value {
        ExpressionValue::Number(number) => PropertyValue::Float(*number),
        ExpressionValue::Boolean(boolean) => PropertyValue::Bool(*boolean),
        ExpressionValue::String(text) => PropertyValue::String(text.clone()),
    }
}

fn to_number(
    value: &ExpressionValue,
    node_id: Option<NodeId>,
    property_path: Option<&str>,
    error_message: &str,
) -> Result<f64, LumenError> {
    match value {
        ExpressionValue::Number(number) => Ok(*number),
        ExpressionValue::Boolean(boolean) => Ok(if *boolean { 1.0 } else { 0.0 }),
        ExpressionValue::String(text) => text
            .parse::<f64>()
            .map_err(|_| expression_eval_error(node_id, property_path, error_message)),
    }
}

fn to_boolean(value: &ExpressionValue) -> bool {
    match value {
        ExpressionValue::Boolean(boolean) => *boolean,
        ExpressionValue::Number(number) => number.abs() > f64::EPSILON,
        ExpressionValue::String(text) => !text.is_empty(),
    }
}

fn expression_eval_error(
    node_id: Option<NodeId>,
    property_path: Option<&str>,
    details: &str,
) -> LumenError {
    LumenError::Expression(ExpressionError::Evaluate {
        node_id,
        property_path: property_path.map(ToString::to_string),
        details: details.to_string(),
    })
}
