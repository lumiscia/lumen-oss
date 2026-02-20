use serde::{Deserialize, Serialize};
use thiserror::Error;

// ── Scalar ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Scalar {
	Literal(f32),
	Expr(String),
}

impl Default for Scalar {
	fn default() -> Self {
		Self::Literal(0.0)
	}
}

impl From<f32> for Scalar {
	fn from(v: f32) -> Self {
		Self::Literal(v)
	}
}

// ── Expression AST ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum ParsedExpr {
	Literal(f32),
	Ref(ExprRef),
	BinOp {
		op: BinOp,
		lhs: Box<ParsedExpr>,
		rhs: Box<ParsedExpr>,
	},
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExprRef {
	pub target: String,
	pub property: ExprProp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExprProp {
	Width,
	Height,
	X,
	Y,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
	Add,
	Sub,
	Mul,
	Div,
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[derive(Debug, Error, PartialEq)]
pub enum ExprParseError {
	#[error("expression is empty")]
	Empty,
	#[error("unknown property: {0}")]
	UnknownProperty(String),
	#[error("invalid number: {0}")]
	InvalidNumber(String),
	#[error("malformed expression: {0}")]
	Malformed(String),
}

#[derive(Debug, Error, PartialEq)]
pub enum ExprEvalError {
	#[error("unresolved reference: {target}.{property:?}")]
	UnresolvedRef { target: String, property: ExprProp },
	#[error("division by zero")]
	DivisionByZero,
}

// ── Eval context ──────────────────────────────────────────────────────────────

pub trait ExprEvalCtx {
	fn resolve(&self, target: &str, property: ExprProp) -> Option<f32>;
}

// ── Parser ────────────────────────────────────────────────────────────────────

fn parse_prop(s: &str) -> Result<ExprProp, ExprParseError> {
	match s {
		"width" => Ok(ExprProp::Width),
		"height" => Ok(ExprProp::Height),
		"x" => Ok(ExprProp::X),
		"y" => Ok(ExprProp::Y),
		other => Err(ExprParseError::UnknownProperty(other.to_string())),
	}
}

fn parse_term(s: &str) -> Result<ParsedExpr, ExprParseError> {
	let s = s.trim();
	if s.is_empty() {
		return Err(ExprParseError::Malformed("empty term".to_string()));
	}

	// Try ref: ident.prop
	if let Some(dot) = s.find('.') {
		let ident = &s[..dot];
		let prop_str = &s[dot + 1..];

		// Validate ident: [a-zA-Z_][a-zA-Z0-9_]*
		let valid_ident = {
			let mut chars = ident.chars();
			match chars.next() {
				Some(c) if c.is_ascii_alphabetic() || c == '_' => {
					chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
				}
				_ => false,
			}
		};

		if valid_ident && !prop_str.is_empty() && !prop_str.contains('.') {
			let property = parse_prop(prop_str)?;
			return Ok(ParsedExpr::Ref(ExprRef {
				target: ident.to_string(),
				property,
			}));
		}
	}

	// Try number
	s.parse::<f32>()
		.map(ParsedExpr::Literal)
		.map_err(|_| ExprParseError::InvalidNumber(s.to_string()))
}

/// Split `s` on the first binary operator (+, -, *, /) that is not inside a
/// reference token. We scan character-by-character and split at the first
/// top-level op we find.
fn split_on_op(s: &str) -> Option<(&str, BinOp, &str)> {
	let bytes = s.as_bytes();
	let mut i = 0usize;
	while i < bytes.len() {
		let ch = bytes[i] as char;
		match ch {
			'+' => return Some((&s[..i], BinOp::Add, &s[i + 1..])),
			'-' => {
				// A leading '-' or a '-' immediately after another op is part
				// of a number; only treat it as a binary op if there's a
				// non-whitespace character before it.
				let before = s[..i].trim();
				if !before.is_empty() {
					return Some((before, BinOp::Sub, &s[i + 1..]));
				}
			}
			'*' => return Some((&s[..i], BinOp::Mul, &s[i + 1..])),
			'/' => return Some((&s[..i], BinOp::Div, &s[i + 1..])),
			_ => {}
		}
		i += 1;
	}
	None
}

pub fn parse_expr(s: &str) -> Result<ParsedExpr, ExprParseError> {
	let s = s.trim();
	if s.is_empty() {
		return Err(ExprParseError::Empty);
	}

	// Try splitting on a binary operator.
	if let Some((lhs_str, op, rhs_str)) = split_on_op(s) {
		let lhs_str = lhs_str.trim();
		let rhs_str = rhs_str.trim();

		// If there's another operator in rhs, that's more than one op → Malformed.
		if split_on_op(rhs_str).is_some() {
			return Err(ExprParseError::Malformed(format!(
				"more than one operator in expression: {s}"
			)));
		}

		let lhs = parse_term(lhs_str)?;
		let rhs = parse_term(rhs_str)?;
		return Ok(ParsedExpr::BinOp {
			op,
			lhs: Box::new(lhs),
			rhs: Box::new(rhs),
		});
	}

	// No operator — must be a single term.
	parse_term(s)
}

// ── Evaluator ─────────────────────────────────────────────────────────────────

pub fn eval_expr(expr: &ParsedExpr, ctx: &dyn ExprEvalCtx) -> Result<f32, ExprEvalError> {
	match expr {
		ParsedExpr::Literal(v) => Ok(*v),
		ParsedExpr::Ref(r) => {
			ctx.resolve(&r.target, r.property).ok_or_else(|| ExprEvalError::UnresolvedRef {
				target: r.target.clone(),
				property: r.property,
			})
		}
		ParsedExpr::BinOp { op, lhs, rhs } => {
			let l = eval_expr(lhs, ctx)?;
			let r = eval_expr(rhs, ctx)?;
			match op {
				BinOp::Add => Ok(l + r),
				BinOp::Sub => Ok(l - r),
				BinOp::Mul => Ok(l * r),
				BinOp::Div => {
					if r == 0.0 {
						Err(ExprEvalError::DivisionByZero)
					} else {
						Ok(l / r)
					}
				}
			}
		}
	}
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parse_empty() {
		assert_eq!(parse_expr(""), Err(ExprParseError::Empty));
		assert_eq!(parse_expr("   "), Err(ExprParseError::Empty));
	}

	#[test]
	fn parse_literal() {
		assert_eq!(parse_expr("100"), Ok(ParsedExpr::Literal(100.0)));
		assert_eq!(parse_expr("100.5"), Ok(ParsedExpr::Literal(100.5)));
	}

	#[test]
	fn parse_ref() {
		assert_eq!(
			parse_expr("canvas.width"),
			Ok(ParsedExpr::Ref(ExprRef {
				target: "canvas".to_string(),
				property: ExprProp::Width,
			}))
		);
	}

	#[test]
	fn parse_binop_ref_literal() {
		assert_eq!(
			parse_expr("node_a.width * 0.5"),
			Ok(ParsedExpr::BinOp {
				op: BinOp::Mul,
				lhs: Box::new(ParsedExpr::Ref(ExprRef {
					target: "node_a".to_string(),
					property: ExprProp::Width,
				})),
				rhs: Box::new(ParsedExpr::Literal(0.5)),
			})
		);
	}

	#[test]
	fn parse_binop_two_refs() {
		assert_eq!(
			parse_expr("a.width + b.height"),
			Ok(ParsedExpr::BinOp {
				op: BinOp::Add,
				lhs: Box::new(ParsedExpr::Ref(ExprRef {
					target: "a".to_string(),
					property: ExprProp::Width,
				})),
				rhs: Box::new(ParsedExpr::Ref(ExprRef {
					target: "b".to_string(),
					property: ExprProp::Height,
				})),
			})
		);
	}

	#[test]
	fn parse_unknown_property() {
		assert_eq!(
			parse_expr("a.foo"),
			Err(ExprParseError::UnknownProperty("foo".to_string()))
		);
	}

	#[test]
	fn parse_too_many_ops() {
		assert!(matches!(
			parse_expr("a.width + b.width + c.width"),
			Err(ExprParseError::Malformed(_))
		));
	}

	#[test]
	fn parse_ident_with_underscores_and_digits() {
		assert_eq!(
			parse_expr("node_1.height"),
			Ok(ParsedExpr::Ref(ExprRef {
				target: "node_1".to_string(),
				property: ExprProp::Height,
			}))
		);
	}

	struct TestCtx;
	impl ExprEvalCtx for TestCtx {
		fn resolve(&self, target: &str, property: ExprProp) -> Option<f32> {
			match (target, property) {
				("node_a", ExprProp::Width) => Some(200.0),
				("node_b", ExprProp::Height) => Some(100.0),
				_ => None,
			}
		}
	}

	#[test]
	fn eval_literal() {
		let expr = parse_expr("42").unwrap();
		assert_eq!(eval_expr(&expr, &TestCtx), Ok(42.0));
	}

	#[test]
	fn eval_ref() {
		let expr = parse_expr("node_a.width").unwrap();
		assert_eq!(eval_expr(&expr, &TestCtx), Ok(200.0));
	}

	#[test]
	fn eval_binop() {
		let expr = parse_expr("node_a.width * 0.5").unwrap();
		assert_eq!(eval_expr(&expr, &TestCtx), Ok(100.0));
	}

	#[test]
	fn eval_division_by_zero() {
		let expr = ParsedExpr::BinOp {
			op: BinOp::Div,
			lhs: Box::new(ParsedExpr::Literal(1.0)),
			rhs: Box::new(ParsedExpr::Literal(0.0)),
		};
		assert_eq!(eval_expr(&expr, &TestCtx), Err(ExprEvalError::DivisionByZero));
	}

	#[test]
	fn eval_unresolved_ref() {
		let expr = parse_expr("missing.width").unwrap();
		assert_eq!(
			eval_expr(&expr, &TestCtx),
			Err(ExprEvalError::UnresolvedRef {
				target: "missing".to_string(),
				property: ExprProp::Width,
			})
		);
	}
}
