use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedExpr {
    source: String,
    ast: Expr,
    refs: Vec<ExprRef>,
}

impl ParsedExpr {
    pub fn source(&self) -> &str {
        self.source.as_str()
    }

    pub fn references(&self) -> &[ExprRef] {
        self.refs.as_slice()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExprRef {
    pub target: String,
    pub property: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Pos,
    Neg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Debug, Clone, PartialEq)]
enum Expr {
    Number(f32),
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Call {
        name: String,
        args: Vec<Expr>,
    },
    Ref(ExprRef),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ExprParseError {
    #[error("unexpected end of expression")]
    UnexpectedEnd,
    #[error("unexpected token `{token}` at byte {offset}")]
    UnexpectedToken { token: String, offset: usize },
    #[error("invalid number literal `{literal}` at byte {offset}")]
    InvalidNumber { literal: String, offset: usize },
    #[error("expected {expected} at byte {offset}")]
    Expected {
        expected: &'static str,
        offset: usize,
    },
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum ExprEvalError {
    #[error("unknown reference `{target}.{property}`")]
    UnknownReference { target: String, property: String },
    #[error("division by zero")]
    DivisionByZero,
    #[error("unknown function `{name}`")]
    UnknownFunction { name: String },
    #[error("invalid argument count for `{name}`: expected {expected}, got {actual}")]
    InvalidArgumentCount {
        name: String,
        expected: &'static str,
        actual: usize,
    },
    #[error("non-finite expression result")]
    NonFinite,
}

pub trait ExprEvalContext {
    fn resolve(&self, target: &str, property: &str) -> Option<f32>;
}

pub fn parse_expr(source: &str) -> Result<ParsedExpr, ExprParseError> {
    let mut parser = Parser::new(source);
    let ast = parser.parse_expr()?;
    parser.expect_end()?;

    let mut refs = Vec::new();
    collect_refs(&ast, &mut refs);

    Ok(ParsedExpr {
        source: source.to_string(),
        ast,
        refs,
    })
}

pub fn eval_expr(expr: &ParsedExpr, ctx: &dyn ExprEvalContext) -> Result<f32, ExprEvalError> {
    let value = eval_node(&expr.ast, ctx)?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(ExprEvalError::NonFinite)
    }
}

fn eval_node(expr: &Expr, ctx: &dyn ExprEvalContext) -> Result<f32, ExprEvalError> {
    match expr {
        Expr::Number(value) => Ok(*value),
        Expr::Unary { op, expr } => {
            let value = eval_node(expr, ctx)?;
            match op {
                UnaryOp::Pos => Ok(value),
                UnaryOp::Neg => Ok(-value),
            }
        }
        Expr::Binary { op, left, right } => {
            let left = eval_node(left, ctx)?;
            let right = eval_node(right, ctx)?;
            let value = match op {
                BinOp::Add => left + right,
                BinOp::Sub => left - right,
                BinOp::Mul => left * right,
                BinOp::Div => {
                    if right.abs() <= f32::EPSILON {
                        return Err(ExprEvalError::DivisionByZero);
                    }
                    left / right
                }
            };
            if value.is_finite() {
                Ok(value)
            } else {
                Err(ExprEvalError::NonFinite)
            }
        }
        Expr::Call { name, args } => {
            let mut values = Vec::with_capacity(args.len());
            for arg in args {
                values.push(eval_node(arg, ctx)?);
            }
            eval_call(name.as_str(), values.as_slice())
        }
        Expr::Ref(reference) => ctx
            .resolve(reference.target.as_str(), reference.property.as_str())
            .ok_or_else(|| ExprEvalError::UnknownReference {
                target: reference.target.clone(),
                property: reference.property.clone(),
            }),
    }
}

fn eval_call(name: &str, args: &[f32]) -> Result<f32, ExprEvalError> {
    let value = match name {
        "clamp" => {
            assert_arg_count(name, args, 3)?;
            args[0].clamp(args[1], args[2])
        }
        "min" => {
            assert_arg_count(name, args, 2)?;
            args[0].min(args[1])
        }
        "max" => {
            assert_arg_count(name, args, 2)?;
            args[0].max(args[1])
        }
        "abs" => {
            assert_arg_count(name, args, 1)?;
            args[0].abs()
        }
        "floor" => {
            assert_arg_count(name, args, 1)?;
            args[0].floor()
        }
        "ceil" => {
            assert_arg_count(name, args, 1)?;
            args[0].ceil()
        }
        "round" => {
            assert_arg_count(name, args, 1)?;
            args[0].round()
        }
        "sqrt" => {
            assert_arg_count(name, args, 1)?;
            args[0].sqrt()
        }
        "sin" => {
            assert_arg_count(name, args, 1)?;
            args[0].sin()
        }
        "cos" => {
            assert_arg_count(name, args, 1)?;
            args[0].cos()
        }
        "mix" => {
            assert_arg_count(name, args, 3)?;
            args[0] + (args[1] - args[0]) * args[2]
        }
        "step" => {
            assert_arg_count(name, args, 2)?;
            if args[1] < args[0] { 0.0 } else { 1.0 }
        }
        "smoothstep" => {
            assert_arg_count(name, args, 3)?;
            let e0 = args[0];
            let e1 = args[1];
            let x = args[2];
            if (e1 - e0).abs() <= f32::EPSILON {
                if x < e0 { 0.0 } else { 1.0 }
            } else {
                let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
                t * t * (3.0 - 2.0 * t)
            }
        }
        _ => {
            return Err(ExprEvalError::UnknownFunction {
                name: name.to_string(),
            });
        }
    };

    if value.is_finite() {
        Ok(value)
    } else {
        Err(ExprEvalError::NonFinite)
    }
}

fn assert_arg_count(name: &str, args: &[f32], expected: usize) -> Result<(), ExprEvalError> {
    if args.len() == expected {
        Ok(())
    } else {
        Err(ExprEvalError::InvalidArgumentCount {
            name: name.to_string(),
            expected: match expected {
                1 => "1",
                2 => "2",
                3 => "3",
                _ => "fixed",
            },
            actual: args.len(),
        })
    }
}

fn collect_refs(expr: &Expr, out: &mut Vec<ExprRef>) {
    match expr {
        Expr::Number(_) => {}
        Expr::Unary { expr, .. } => collect_refs(expr, out),
        Expr::Binary { left, right, .. } => {
            collect_refs(left, out);
            collect_refs(right, out);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_refs(arg, out);
            }
        }
        Expr::Ref(reference) => out.push(reference.clone()),
    }
}

#[derive(Debug, Clone, PartialEq)]
enum TokenKind {
    Number(f32),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Dot,
    Comma,
    LParen,
    RParen,
    End,
}

#[derive(Debug, Clone, PartialEq)]
struct Token {
    kind: TokenKind,
    offset: usize,
    text: String,
}

struct Lexer<'a> {
    source: &'a str,
    bytes: &'a [u8],
    index: usize,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            index: 0,
        }
    }

    fn next_token(&mut self) -> Result<Token, ExprParseError> {
        self.skip_ws();
        let start = self.index;
        let Some(byte) = self.peek() else {
            return Ok(Token {
                kind: TokenKind::End,
                offset: self.index,
                text: String::new(),
            });
        };

        let token = match byte {
            b'+' => self.single(TokenKind::Plus),
            b'-' => self.single(TokenKind::Minus),
            b'*' => self.single(TokenKind::Star),
            b'/' => self.single(TokenKind::Slash),
            b'.' => self.single(TokenKind::Dot),
            b',' => self.single(TokenKind::Comma),
            b'(' => self.single(TokenKind::LParen),
            b')' => self.single(TokenKind::RParen),
            b'0'..=b'9' => self.number()?,
            _ if is_ident_start(byte) => self.ident(),
            _ => {
                return Err(ExprParseError::UnexpectedToken {
                    token: self.source[start..start + 1].to_string(),
                    offset: start,
                });
            }
        };

        Ok(token)
    }

    fn single(&mut self, kind: TokenKind) -> Token {
        let offset = self.index;
        let text = self.source[offset..offset + 1].to_string();
        self.index += 1;
        Token { kind, offset, text }
    }

    fn number(&mut self) -> Result<Token, ExprParseError> {
        let start = self.index;
        let mut saw_dot = false;

        while let Some(byte) = self.peek() {
            if byte == b'.' {
                if saw_dot {
                    break;
                }
                saw_dot = true;
                self.index += 1;
                continue;
            }
            if byte.is_ascii_digit() {
                self.index += 1;
            } else {
                break;
            }
        }

        let literal = &self.source[start..self.index];
        let value = literal
            .parse::<f32>()
            .map_err(|_| ExprParseError::InvalidNumber {
                literal: literal.to_string(),
                offset: start,
            })?;
        Ok(Token {
            kind: TokenKind::Number(value),
            offset: start,
            text: literal.to_string(),
        })
    }

    fn ident(&mut self) -> Token {
        let start = self.index;
        self.index += 1;
        while let Some(byte) = self.peek() {
            if is_ident_continue(byte) {
                self.index += 1;
            } else {
                break;
            }
        }
        let ident = &self.source[start..self.index];
        Token {
            kind: TokenKind::Ident(ident.to_string()),
            offset: start,
            text: ident.to_string(),
        }
    }

    fn skip_ws(&mut self) {
        while let Some(byte) = self.peek() {
            if byte.is_ascii_whitespace() {
                self.index += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.index).copied()
    }
}

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_ident_continue(byte: u8) -> bool {
    is_ident_start(byte) || byte.is_ascii_digit()
}

struct Parser<'a> {
    lexer: Lexer<'a>,
    current: Token,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        let mut lexer = Lexer::new(source);
        let current = lexer.next_token().unwrap_or(Token {
            kind: TokenKind::End,
            offset: source.len(),
            text: String::new(),
        });
        Self { lexer, current }
    }

    fn parse_expr(&mut self) -> Result<Expr, ExprParseError> {
        let mut expr = self.parse_term()?;
        loop {
            let op = match self.current.kind {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                _ => break,
            };
            self.bump()?;
            let right = self.parse_term()?;
            expr = Expr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_term(&mut self) -> Result<Expr, ExprParseError> {
        let mut expr = self.parse_factor()?;
        loop {
            let op = match self.current.kind {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                _ => break,
            };
            self.bump()?;
            let right = self.parse_factor()?;
            expr = Expr::Binary {
                op,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_factor(&mut self) -> Result<Expr, ExprParseError> {
        match self.current.kind {
            TokenKind::Plus => {
                self.bump()?;
                let expr = self.parse_factor()?;
                Ok(Expr::Unary {
                    op: UnaryOp::Pos,
                    expr: Box::new(expr),
                })
            }
            TokenKind::Minus => {
                self.bump()?;
                let expr = self.parse_factor()?;
                Ok(Expr::Unary {
                    op: UnaryOp::Neg,
                    expr: Box::new(expr),
                })
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, ExprParseError> {
        match &self.current.kind {
            TokenKind::Number(value) => {
                let value = *value;
                self.bump()?;
                Ok(Expr::Number(value))
            }
            TokenKind::LParen => {
                self.bump()?;
                let expr = self.parse_expr()?;
                self.expect(TokenKind::RParen, "`)`")?;
                Ok(expr)
            }
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.bump()?;
                match &self.current.kind {
                    TokenKind::LParen => self.parse_call(name),
                    TokenKind::Dot => self.parse_ref(name),
                    _ => Err(ExprParseError::Expected {
                        expected: "`.` for reference or `(` for function call",
                        offset: self.current.offset,
                    }),
                }
            }
            TokenKind::End => Err(ExprParseError::UnexpectedEnd),
            _ => Err(ExprParseError::UnexpectedToken {
                token: self.current.text.clone(),
                offset: self.current.offset,
            }),
        }
    }

    fn parse_call(&mut self, name: String) -> Result<Expr, ExprParseError> {
        self.expect(TokenKind::LParen, "`(`")?;

        let mut args = Vec::new();
        if !matches!(self.current.kind, TokenKind::RParen) {
            loop {
                args.push(self.parse_expr()?);
                if matches!(self.current.kind, TokenKind::Comma) {
                    self.bump()?;
                    continue;
                }
                break;
            }
        }

        self.expect(TokenKind::RParen, "`)`")?;
        Ok(Expr::Call { name, args })
    }

    fn parse_ref(&mut self, target: String) -> Result<Expr, ExprParseError> {
        self.expect(TokenKind::Dot, "`.`")?;
        match &self.current.kind {
            TokenKind::Ident(property) => {
                let reference = ExprRef {
                    target,
                    property: property.clone(),
                };
                self.bump()?;
                Ok(Expr::Ref(reference))
            }
            _ => Err(ExprParseError::Expected {
                expected: "property identifier",
                offset: self.current.offset,
            }),
        }
    }

    fn expect(&mut self, kind: TokenKind, expected: &'static str) -> Result<(), ExprParseError> {
        if std::mem::discriminant(&self.current.kind) == std::mem::discriminant(&kind) {
            self.bump()?;
            Ok(())
        } else {
            Err(ExprParseError::Expected {
                expected,
                offset: self.current.offset,
            })
        }
    }

    fn expect_end(&self) -> Result<(), ExprParseError> {
        if matches!(self.current.kind, TokenKind::End) {
            Ok(())
        } else {
            Err(ExprParseError::UnexpectedToken {
                token: self.current.text.clone(),
                offset: self.current.offset,
            })
        }
    }

    fn bump(&mut self) -> Result<(), ExprParseError> {
        self.current = self.lexer.next_token()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{ExprEvalContext, eval_expr, parse_expr};

    struct MapCtx {
        map: HashMap<(String, String), f32>,
    }

    impl ExprEvalContext for MapCtx {
        fn resolve(&self, target: &str, property: &str) -> Option<f32> {
            self.map
                .get(&(target.to_string(), property.to_string()))
                .copied()
        }
    }

    #[test]
    fn parses_references() {
        let parsed = parse_expr("canvas.width * 0.5 + clip_a.x").expect("parse");
        let refs = parsed.references();
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].target, "canvas");
        assert_eq!(refs[0].property, "width");
        assert_eq!(refs[1].target, "clip_a");
        assert_eq!(refs[1].property, "x");
    }

    #[test]
    fn evaluates_math_and_functions() {
        let parsed = parse_expr("mix(0, 10, 0.5) + clamp(2, 0, 1)").expect("parse");
        let ctx = MapCtx {
            map: HashMap::new(),
        };
        let value = eval_expr(&parsed, &ctx).expect("eval");
        assert!((value - 6.0).abs() < 0.0001);
    }

    #[test]
    fn errors_on_unknown_reference() {
        let parsed = parse_expr("clip_missing.width").expect("parse");
        let ctx = MapCtx {
            map: HashMap::new(),
        };
        let error = eval_expr(&parsed, &ctx).expect_err("expected error");
        assert!(error.to_string().contains("clip_missing.width"));
    }
}
