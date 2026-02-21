use serde::{Deserialize, Serialize};
use thiserror::Error;

// -- Scalar -------------------------------------------------------------------

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

// -- Expression AST -----------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum ParsedExpr {
    Literal(f32),
    Ref(ExprRef),
    UnaryOp {
        op: UnaryOp,
        expr: Box<ParsedExpr>,
    },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExprProp {
    Width,
    Height,
    X,
    Y,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Plus,
    Minus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
}

// -- Errors -------------------------------------------------------------------

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

// -- Display ------------------------------------------------------------------

impl std::fmt::Display for ExprProp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExprProp::Width => write!(f, "width"),
            ExprProp::Height => write!(f, "height"),
            ExprProp::X => write!(f, "x"),
            ExprProp::Y => write!(f, "y"),
        }
    }
}

impl std::fmt::Display for ParsedExpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParsedExpr::Literal(v) => write!(f, "{v}"),
            ParsedExpr::Ref(r) => write!(f, "{}.{}", r.target, r.property),
            ParsedExpr::UnaryOp { op, expr } => {
                let op_char = match op {
                    UnaryOp::Plus => "+",
                    UnaryOp::Minus => "-",
                };
                write!(f, "{op_char}({expr})")
            }
            ParsedExpr::BinOp { op, lhs, rhs } => {
                let op_char = match op {
                    BinOp::Add => "+",
                    BinOp::Sub => "-",
                    BinOp::Mul => "*",
                    BinOp::Div => "/",
                };
                write!(f, "({lhs} {op_char} {rhs})")
            }
        }
    }
}

// -- Eval context -------------------------------------------------------------

pub trait ExprEvalCtx {
    fn resolve(&self, target: &str, property: ExprProp) -> Option<f32>;
}

// -- Lexer --------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f32),
    Ident(String),
    Dot,
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
    Eof,
}

struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            pos: 0,
        }
    }

    fn next_token(&mut self) -> Result<Token, ExprParseError> {
        self.skip_whitespace();

        if self.pos >= self.bytes.len() {
            return Ok(Token::Eof);
        }

        let ch = self.bytes[self.pos] as char;
        let token = match ch {
            '+' => {
                self.pos += 1;
                Token::Plus
            }
            '-' => {
                self.pos += 1;
                Token::Minus
            }
            '*' => {
                self.pos += 1;
                Token::Star
            }
            '/' => {
                self.pos += 1;
                Token::Slash
            }
            '(' => {
                self.pos += 1;
                Token::LParen
            }
            ')' => {
                self.pos += 1;
                Token::RParen
            }
            '.' => {
                if self.peek_is_ascii_digit(1) {
                    self.parse_number_token()?
                } else {
                    self.pos += 1;
                    Token::Dot
                }
            }
            _ if ch.is_ascii_digit() => self.parse_number_token()?,
            _ if is_ident_start(ch) => self.parse_ident_token(),
            _ => {
                return Err(ExprParseError::Malformed(format!(
                    "unexpected character `{}`",
                    ch
                )));
            }
        };

        Ok(token)
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.bytes.len() && (self.bytes[self.pos] as char).is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek_is_ascii_digit(&self, offset: usize) -> bool {
        self.pos + offset < self.bytes.len()
            && (self.bytes[self.pos + offset] as char).is_ascii_digit()
    }

    fn parse_ident_token(&mut self) -> Token {
        let start = self.pos;
        self.pos += 1;
        while self.pos < self.bytes.len() {
            let ch = self.bytes[self.pos] as char;
            if !is_ident_continue(ch) {
                break;
            }
            self.pos += 1;
        }
        Token::Ident(self.src[start..self.pos].to_string())
    }

    fn parse_number_token(&mut self) -> Result<Token, ExprParseError> {
        let start = self.pos;
        let mut saw_digit = false;

        while self.pos < self.bytes.len() {
            let ch = self.bytes[self.pos] as char;
            if !ch.is_ascii_digit() {
                break;
            }
            saw_digit = true;
            self.pos += 1;
        }

        if self.pos < self.bytes.len() && (self.bytes[self.pos] as char) == '.' {
            self.pos += 1;
            while self.pos < self.bytes.len() {
                let ch = self.bytes[self.pos] as char;
                if !ch.is_ascii_digit() {
                    break;
                }
                saw_digit = true;
                self.pos += 1;
            }
        }

        if self.pos < self.bytes.len() {
            let ch = self.bytes[self.pos] as char;
            if ch == 'e' || ch == 'E' {
                self.pos += 1;
                if self.pos < self.bytes.len() {
                    let sign = self.bytes[self.pos] as char;
                    if sign == '+' || sign == '-' {
                        self.pos += 1;
                    }
                }

                let exp_start = self.pos;
                while self.pos < self.bytes.len() {
                    let ch = self.bytes[self.pos] as char;
                    if !ch.is_ascii_digit() {
                        break;
                    }
                    self.pos += 1;
                }
                if self.pos == exp_start {
                    let bad = self.src[start..self.pos].to_string();
                    return Err(ExprParseError::InvalidNumber(bad));
                }
            }
        }

        if !saw_digit {
            let bad = self.src[start..self.pos].to_string();
            return Err(ExprParseError::InvalidNumber(bad));
        }

        let token = &self.src[start..self.pos];
        let value = token
            .parse::<f32>()
            .map_err(|_| ExprParseError::InvalidNumber(token.to_string()))?;
        Ok(Token::Number(value))
    }
}

fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_ident_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

// -- Parser -------------------------------------------------------------------

struct Parser<'a> {
    lexer: Lexer<'a>,
    current: Token,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Result<Self, ExprParseError> {
        let mut lexer = Lexer::new(src);
        let current = lexer.next_token()?;
        Ok(Self { lexer, current })
    }

    fn bump(&mut self) -> Result<(), ExprParseError> {
        self.current = self.lexer.next_token()?;
        Ok(())
    }

    fn parse(mut self) -> Result<ParsedExpr, ExprParseError> {
        let expr = self.parse_add_sub()?;
        if self.current != Token::Eof {
            return Err(ExprParseError::Malformed(format!(
                "unexpected token {:?}",
                self.current
            )));
        }
        Ok(expr)
    }

    fn parse_add_sub(&mut self) -> Result<ParsedExpr, ExprParseError> {
        let mut expr = self.parse_mul_div()?;
        loop {
            let op = match self.current {
                Token::Plus => BinOp::Add,
                Token::Minus => BinOp::Sub,
                _ => break,
            };
            self.bump()?;
            let rhs = self.parse_mul_div()?;
            expr = ParsedExpr::BinOp {
                op,
                lhs: Box::new(expr),
                rhs: Box::new(rhs),
            };
        }
        Ok(expr)
    }

    fn parse_mul_div(&mut self) -> Result<ParsedExpr, ExprParseError> {
        let mut expr = self.parse_unary()?;
        loop {
            let op = match self.current {
                Token::Star => BinOp::Mul,
                Token::Slash => BinOp::Div,
                _ => break,
            };
            self.bump()?;
            let rhs = self.parse_unary()?;
            expr = ParsedExpr::BinOp {
                op,
                lhs: Box::new(expr),
                rhs: Box::new(rhs),
            };
        }
        Ok(expr)
    }

    fn parse_unary(&mut self) -> Result<ParsedExpr, ExprParseError> {
        match self.current {
            Token::Plus => {
                self.bump()?;
                let expr = self.parse_unary()?;
                Ok(ParsedExpr::UnaryOp {
                    op: UnaryOp::Plus,
                    expr: Box::new(expr),
                })
            }
            Token::Minus => {
                self.bump()?;
                let expr = self.parse_unary()?;
                Ok(ParsedExpr::UnaryOp {
                    op: UnaryOp::Minus,
                    expr: Box::new(expr),
                })
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<ParsedExpr, ExprParseError> {
        match &self.current {
            Token::Number(v) => {
                let value = *v;
                self.bump()?;
                Ok(ParsedExpr::Literal(value))
            }
            Token::Ident(target) => {
                let target = target.clone();
                self.bump()?;
                if self.current != Token::Dot {
                    return Err(ExprParseError::Malformed(format!(
                        "identifier `{target}` must be followed by `.property`"
                    )));
                }
                self.bump()?;
                let Token::Ident(property_ident) = &self.current else {
                    return Err(ExprParseError::Malformed(format!(
                        "reference `{target}.` is missing property"
                    )));
                };
                let property = parse_prop(property_ident)?;
                self.bump()?;
                Ok(ParsedExpr::Ref(ExprRef { target, property }))
            }
            Token::LParen => {
                self.bump()?;
                let expr = self.parse_add_sub()?;
                if self.current != Token::RParen {
                    return Err(ExprParseError::Malformed(
                        "expected closing `)`".to_string(),
                    ));
                }
                self.bump()?;
                Ok(expr)
            }
            Token::Eof => Err(ExprParseError::Malformed(
                "unexpected end of expression".to_string(),
            )),
            other => Err(ExprParseError::Malformed(format!(
                "unexpected token {:?}",
                other
            ))),
        }
    }
}

fn parse_prop(s: &str) -> Result<ExprProp, ExprParseError> {
    match s {
        "width" => Ok(ExprProp::Width),
        "height" => Ok(ExprProp::Height),
        "x" => Ok(ExprProp::X),
        "y" => Ok(ExprProp::Y),
        other => Err(ExprParseError::UnknownProperty(other.to_string())),
    }
}

pub fn parse_expr(s: &str) -> Result<ParsedExpr, ExprParseError> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(ExprParseError::Empty);
    }
    Parser::new(trimmed)?.parse()
}

// -- Evaluator ----------------------------------------------------------------

pub fn eval_expr(expr: &ParsedExpr, ctx: &dyn ExprEvalCtx) -> Result<f32, ExprEvalError> {
    match expr {
        ParsedExpr::Literal(v) => Ok(*v),
        ParsedExpr::Ref(r) => {
            ctx.resolve(&r.target, r.property)
                .ok_or_else(|| ExprEvalError::UnresolvedRef {
                    target: r.target.clone(),
                    property: r.property,
                })
        }
        ParsedExpr::UnaryOp { op, expr } => {
            let value = eval_expr(expr, ctx)?;
            match op {
                UnaryOp::Plus => Ok(value),
                UnaryOp::Minus => Ok(-value),
            }
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

// -- Tests --------------------------------------------------------------------

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
        assert_eq!(parse_expr("1e3"), Ok(ParsedExpr::Literal(1000.0)));
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
    fn parse_complex_expr_with_precedence() {
        let parsed = parse_expr("node_a.width + node_b.height * 0.5").expect("parse");
        match parsed {
            ParsedExpr::BinOp {
                op: BinOp::Add,
                lhs,
                rhs,
            } => {
                assert!(matches!(*lhs, ParsedExpr::Ref(_)));
                assert!(matches!(*rhs, ParsedExpr::BinOp { op: BinOp::Mul, .. }));
            }
            _ => panic!("expected add expression"),
        }
    }

    #[test]
    fn parse_parenthesized_expr() {
        let parsed = parse_expr("(node_a.width + node_b.height) * 0.5").expect("parse");
        match parsed {
            ParsedExpr::BinOp {
                op: BinOp::Mul,
                lhs,
                rhs,
            } => {
                assert!(matches!(*lhs, ParsedExpr::BinOp { op: BinOp::Add, .. }));
                assert!(matches!(*rhs, ParsedExpr::Literal(_)));
            }
            _ => panic!("expected multiply expression"),
        }
    }

    #[test]
    fn parse_unary_minus() {
        let parsed = parse_expr("-node_a.width").expect("parse");
        assert!(matches!(
            parsed,
            ParsedExpr::UnaryOp {
                op: UnaryOp::Minus,
                ..
            }
        ));
    }

    #[test]
    fn parse_unknown_property() {
        assert_eq!(
            parse_expr("a.foo"),
            Err(ExprParseError::UnknownProperty("foo".to_string()))
        );
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

    #[test]
    fn parse_rejects_unbalanced_parentheses() {
        assert!(matches!(
            parse_expr("(a.width + 1"),
            Err(ExprParseError::Malformed(_))
        ));
    }

    struct TestCtx;
    impl ExprEvalCtx for TestCtx {
        fn resolve(&self, target: &str, property: ExprProp) -> Option<f32> {
            match (target, property) {
                ("node_a", ExprProp::Width) => Some(200.0),
                ("node_b", ExprProp::Height) => Some(100.0),
                ("clip", ExprProp::X) => Some(40.0),
                _ => None,
            }
        }
    }

    #[test]
    fn eval_literal() {
        let expr = parse_expr("42").expect("parse");
        assert_eq!(eval_expr(&expr, &TestCtx), Ok(42.0));
    }

    #[test]
    fn eval_ref() {
        let expr = parse_expr("node_a.width").expect("parse");
        assert_eq!(eval_expr(&expr, &TestCtx), Ok(200.0));
    }

    #[test]
    fn eval_complex_expr() {
        let expr = parse_expr("((node_a.width + node_b.height) * 0.5) - clip.x").expect("parse");
        assert_eq!(eval_expr(&expr, &TestCtx), Ok(110.0));
    }

    #[test]
    fn eval_division_by_zero() {
        let expr = ParsedExpr::BinOp {
            op: BinOp::Div,
            lhs: Box::new(ParsedExpr::Literal(1.0)),
            rhs: Box::new(ParsedExpr::Literal(0.0)),
        };
        assert_eq!(
            eval_expr(&expr, &TestCtx),
            Err(ExprEvalError::DivisionByZero)
        );
    }

    #[test]
    fn eval_unresolved_ref() {
        let expr = parse_expr("missing.width").expect("parse");
        assert_eq!(
            eval_expr(&expr, &TestCtx),
            Err(ExprEvalError::UnresolvedRef {
                target: "missing".to_string(),
                property: ExprProp::Width,
            })
        );
    }
}
