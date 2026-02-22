use std::collections::HashMap;
use std::ops::Range;

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExpressionId(pub String);

#[derive(Debug, Clone)]
pub struct Expression {
    pub id: ExpressionId,
    pub source: String,
    pub references: Vec<ExpressionReference>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ExpressionProperty {
    X,
    Y,
    Width,
    Height,
    Opacity,
}

impl ExpressionProperty {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::X => "x",
            Self::Y => "y",
            Self::Width => "width",
            Self::Height => "height",
            Self::Opacity => "opacity",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        match name {
            "x" => Some(Self::X),
            "y" => Some(Self::Y),
            "width" => Some(Self::Width),
            "height" => Some(Self::Height),
            "opacity" => Some(Self::Opacity),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ExpressionReferenceTarget {
    ClipProperty {
        clip_id: String,
        property: ExpressionProperty,
    },
    LayoutNodeProperty {
        node_id: String,
        property: ExpressionProperty,
    },
}

#[derive(Debug, Clone)]
pub struct ExpressionReference {
    pub target: ExpressionReferenceTarget,
    pub span: Range<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExpressionValue {
    Number(f32),
    Boolean(bool),
    String(String),
}

impl ExpressionValue {
    fn kind_name(&self) -> &'static str {
        match self {
            Self::Number(_) => "number",
            Self::Boolean(_) => "boolean",
            Self::String(_) => "string",
        }
    }

    fn as_number(&self, context: &'static str) -> Result<f32, ExpressionError> {
        match self {
            Self::Number(value) => Ok(*value),
            other => Err(ExpressionError::TypeMismatch {
                context,
                expected: "number",
                found: other.kind_name(),
            }),
        }
    }

    fn as_bool(&self, context: &'static str) -> Result<bool, ExpressionError> {
        match self {
            Self::Boolean(value) => Ok(*value),
            other => Err(ExpressionError::TypeMismatch {
                context,
                expected: "boolean",
                found: other.kind_name(),
            }),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ExpressionScope {
    pub clip_properties: HashMap<(String, ExpressionProperty), ExpressionValue>,
    pub layout_properties: HashMap<(String, ExpressionProperty), ExpressionValue>,
}

#[derive(Debug, Error)]
pub enum ExpressionError {
    #[error("parse error at byte {position}: {message}")]
    Parse { position: usize, message: String },
    #[error("unexpected end of input")]
    UnexpectedEof,
    #[error("unknown function `{name}`")]
    UnknownFunction { name: String },
    #[error("invalid argument count for `{name}`: expected {expected}, got {got}")]
    InvalidArgumentCount {
        name: String,
        expected: &'static str,
        got: usize,
    },
    #[error("type mismatch in {context}: expected {expected}, found {found}")]
    TypeMismatch {
        context: &'static str,
        expected: &'static str,
        found: &'static str,
    },
    #[error("unresolved expression reference")]
    UnresolvedReference { target: ExpressionReferenceTarget },
    #[error("division by zero")]
    DivisionByZero,
    #[error("invalid arguments for `{name}`: {message}")]
    InvalidArguments { name: String, message: String },
}

#[derive(Debug, Clone, PartialEq)]
enum ExprNode {
    Number(f32),
    Boolean(bool),
    String(String),
    ClipRef {
        clip_id: String,
        property: ExpressionProperty,
    },
    LayoutRef {
        node_id: String,
        property: ExpressionProperty,
    },
    Unary {
        op: UnaryOp,
        expr: Box<ExprNode>,
    },
    Binary {
        op: BinaryOp,
        left: Box<ExprNode>,
        right: Box<ExprNode>,
    },
    FuncCall {
        name: String,
        args: Vec<ExprNode>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Gt,
    Lt,
    Gte,
    Lte,
    Eq,
    Neq,
    And,
    Or,
}

impl Expression {
    pub fn parse(id: ExpressionId, source: impl Into<String>) -> Result<Self, ExpressionError> {
        let source = source.into();
        let mut parser = Parser::new(&source);
        let (_, references) = parser.parse_full()?;

        Ok(Self {
            id,
            source,
            references,
        })
    }

    pub fn evaluate(&self, scope: &ExpressionScope) -> Result<ExpressionValue, ExpressionError> {
        let mut parser = Parser::new(&self.source);
        let (ast, _) = parser.parse_full()?;
        ast.evaluate(scope)
    }
}

impl ExprNode {
    fn evaluate(&self, scope: &ExpressionScope) -> Result<ExpressionValue, ExpressionError> {
        match self {
            Self::Number(value) => Ok(ExpressionValue::Number(*value)),
            Self::Boolean(value) => Ok(ExpressionValue::Boolean(*value)),
            Self::String(value) => Ok(ExpressionValue::String(value.clone())),
            Self::ClipRef { clip_id, property } => scope
                .clip_properties
                .get(&(clip_id.clone(), *property))
                .cloned()
                .ok_or_else(|| ExpressionError::UnresolvedReference {
                    target: ExpressionReferenceTarget::ClipProperty {
                        clip_id: clip_id.clone(),
                        property: *property,
                    },
                }),
            Self::LayoutRef { node_id, property } => scope
                .layout_properties
                .get(&(node_id.clone(), *property))
                .cloned()
                .ok_or_else(|| ExpressionError::UnresolvedReference {
                    target: ExpressionReferenceTarget::LayoutNodeProperty {
                        node_id: node_id.clone(),
                        property: *property,
                    },
                }),
            Self::Unary { op, expr } => {
                let value = expr.evaluate(scope)?;
                match op {
                    UnaryOp::Neg => Ok(ExpressionValue::Number(-value.as_number("negation")?)),
                    UnaryOp::Not => Ok(ExpressionValue::Boolean(!value.as_bool("logical not")?)),
                }
            }
            Self::Binary { op, left, right } => match op {
                BinaryOp::And => {
                    let left_value = left.evaluate(scope)?;
                    let left_bool = left_value.as_bool("logical and")?;
                    if !left_bool {
                        return Ok(ExpressionValue::Boolean(false));
                    }
                    let right_bool = right.evaluate(scope)?.as_bool("logical and")?;
                    Ok(ExpressionValue::Boolean(right_bool))
                }
                BinaryOp::Or => {
                    let left_value = left.evaluate(scope)?;
                    let left_bool = left_value.as_bool("logical or")?;
                    if left_bool {
                        return Ok(ExpressionValue::Boolean(true));
                    }
                    let right_bool = right.evaluate(scope)?.as_bool("logical or")?;
                    Ok(ExpressionValue::Boolean(right_bool))
                }
                BinaryOp::Eq | BinaryOp::Neq => {
                    let left_value = left.evaluate(scope)?;
                    let right_value = right.evaluate(scope)?;
                    let is_equal = left_value == right_value;
                    Ok(ExpressionValue::Boolean(if matches!(op, BinaryOp::Eq) {
                        is_equal
                    } else {
                        !is_equal
                    }))
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
                    let left_number = left.evaluate(scope)?.as_number("binary operation")?;
                    let right_number = right.evaluate(scope)?.as_number("binary operation")?;
                    match op {
                        BinaryOp::Add => Ok(ExpressionValue::Number(left_number + right_number)),
                        BinaryOp::Sub => Ok(ExpressionValue::Number(left_number - right_number)),
                        BinaryOp::Mul => Ok(ExpressionValue::Number(left_number * right_number)),
                        BinaryOp::Div => {
                            if right_number == 0.0 {
                                return Err(ExpressionError::DivisionByZero);
                            }
                            Ok(ExpressionValue::Number(left_number / right_number))
                        }
                        BinaryOp::Mod => {
                            if right_number == 0.0 {
                                return Err(ExpressionError::DivisionByZero);
                            }
                            Ok(ExpressionValue::Number(left_number % right_number))
                        }
                        BinaryOp::Gt => Ok(ExpressionValue::Boolean(left_number > right_number)),
                        BinaryOp::Lt => Ok(ExpressionValue::Boolean(left_number < right_number)),
                        BinaryOp::Gte => Ok(ExpressionValue::Boolean(left_number >= right_number)),
                        BinaryOp::Lte => Ok(ExpressionValue::Boolean(left_number <= right_number)),
                        BinaryOp::Eq | BinaryOp::Neq | BinaryOp::And | BinaryOp::Or => {
                            Err(ExpressionError::Parse {
                                position: 0,
                                message: "invalid binary operator dispatch".to_owned(),
                            })
                        }
                    }
                }
            },
            Self::FuncCall { name, args } => evaluate_builtin(name, args, scope),
        }
    }
}

fn evaluate_builtin(
    name: &str,
    args: &[ExprNode],
    scope: &ExpressionScope,
) -> Result<ExpressionValue, ExpressionError> {
    let eval_number =
        |expr: &ExprNode, context: &'static str| expr.evaluate(scope)?.as_number(context);
    match name {
        "if" => {
            if args.len() != 3 {
                return Err(ExpressionError::InvalidArgumentCount {
                    name: name.to_owned(),
                    expected: "3",
                    got: args.len(),
                });
            }
            let condition = args[0].evaluate(scope)?;
            let condition = match condition {
                ExpressionValue::Boolean(value) => value,
                ExpressionValue::Number(value) => value != 0.0,
                ExpressionValue::String(_) => {
                    return Err(ExpressionError::TypeMismatch {
                        context: "if condition",
                        expected: "boolean or number",
                        found: "string",
                    });
                }
            };
            if condition {
                args[1].evaluate(scope)
            } else {
                args[2].evaluate(scope)
            }
        }
        "min" => {
            if args.len() < 2 {
                return Err(ExpressionError::InvalidArgumentCount {
                    name: name.to_owned(),
                    expected: "at least 2",
                    got: args.len(),
                });
            }
            let mut iter = args.iter();
            let Some(first) = iter.next() else {
                return Err(ExpressionError::InvalidArgumentCount {
                    name: name.to_owned(),
                    expected: "at least 2",
                    got: 0,
                });
            };
            let mut value = eval_number(first, "min")?;
            for arg in iter {
                value = value.min(eval_number(arg, "min")?);
            }
            Ok(ExpressionValue::Number(value))
        }
        "max" => {
            if args.len() < 2 {
                return Err(ExpressionError::InvalidArgumentCount {
                    name: name.to_owned(),
                    expected: "at least 2",
                    got: args.len(),
                });
            }
            let mut iter = args.iter();
            let Some(first) = iter.next() else {
                return Err(ExpressionError::InvalidArgumentCount {
                    name: name.to_owned(),
                    expected: "at least 2",
                    got: 0,
                });
            };
            let mut value = eval_number(first, "max")?;
            for arg in iter {
                value = value.max(eval_number(arg, "max")?);
            }
            Ok(ExpressionValue::Number(value))
        }
        "abs" => unary_math(name, args, scope, |v| v.abs()),
        "floor" => unary_math(name, args, scope, |v| v.floor()),
        "ceil" => unary_math(name, args, scope, |v| v.ceil()),
        "round" => unary_math(name, args, scope, |v| v.round()),
        "sin" => unary_math(name, args, scope, |v| v.sin()),
        "cos" => unary_math(name, args, scope, |v| v.cos()),
        "clamp" => {
            if args.len() != 3 {
                return Err(ExpressionError::InvalidArgumentCount {
                    name: name.to_owned(),
                    expected: "3",
                    got: args.len(),
                });
            }
            let value = eval_number(&args[0], "clamp")?;
            let min = eval_number(&args[1], "clamp")?;
            let max = eval_number(&args[2], "clamp")?;
            if min > max {
                return Err(ExpressionError::InvalidArguments {
                    name: name.to_owned(),
                    message: "min must be <= max".to_owned(),
                });
            }
            Ok(ExpressionValue::Number(value.clamp(min, max)))
        }
        "lerp" => {
            if args.len() != 3 {
                return Err(ExpressionError::InvalidArgumentCount {
                    name: name.to_owned(),
                    expected: "3",
                    got: args.len(),
                });
            }
            let start = eval_number(&args[0], "lerp")?;
            let end = eval_number(&args[1], "lerp")?;
            let t = eval_number(&args[2], "lerp")?;
            Ok(ExpressionValue::Number(start + (end - start) * t))
        }
        _ => Err(ExpressionError::UnknownFunction {
            name: name.to_owned(),
        }),
    }
}

fn unary_math(
    name: &str,
    args: &[ExprNode],
    scope: &ExpressionScope,
    f: impl Fn(f32) -> f32,
) -> Result<ExpressionValue, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::InvalidArgumentCount {
            name: name.to_owned(),
            expected: "1",
            got: args.len(),
        });
    }

    let value = args[0].evaluate(scope)?.as_number("math function")?;
    Ok(ExpressionValue::Number(f(value)))
}

pub fn parse_expression(
    id: ExpressionId,
    source: impl Into<String>,
) -> Result<Expression, ExpressionError> {
    Expression::parse(id, source)
}

pub fn evaluate_expression(
    expression: &Expression,
    scope: &ExpressionScope,
) -> Result<ExpressionValue, ExpressionError> {
    expression.evaluate(scope)
}

#[derive(Debug, Clone, PartialEq)]
enum TokenKind {
    Identifier(String),
    Number(f32),
    String(String),
    LParen,
    RParen,
    Comma,
    Dot,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Bang,
    Gt,
    Lt,
    Gte,
    Lte,
    EqEq,
    Neq,
    AndAnd,
    OrOr,
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
struct Token {
    kind: TokenKind,
    span: Range<usize>,
}

struct Lexer<'a> {
    source: &'a str,
    position: usize,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            position: 0,
        }
    }

    fn next_token(&mut self) -> Result<Token, ExpressionError> {
        self.skip_whitespace();
        let start = self.position;

        let Some(ch) = self.peek_char() else {
            return Ok(Token {
                kind: TokenKind::Eof,
                span: start..start,
            });
        };

        let token = match ch {
            '(' => {
                self.bump_char();
                TokenKind::LParen
            }
            ')' => {
                self.bump_char();
                TokenKind::RParen
            }
            ',' => {
                self.bump_char();
                TokenKind::Comma
            }
            '.' => {
                self.bump_char();
                TokenKind::Dot
            }
            '+' => {
                self.bump_char();
                TokenKind::Plus
            }
            '-' => {
                self.bump_char();
                TokenKind::Minus
            }
            '*' => {
                self.bump_char();
                TokenKind::Star
            }
            '/' => {
                self.bump_char();
                TokenKind::Slash
            }
            '%' => {
                self.bump_char();
                TokenKind::Percent
            }
            '!' => {
                self.bump_char();
                if self.peek_char() == Some('=') {
                    self.bump_char();
                    TokenKind::Neq
                } else {
                    TokenKind::Bang
                }
            }
            '=' => {
                self.bump_char();
                if self.peek_char() == Some('=') {
                    self.bump_char();
                    TokenKind::EqEq
                } else {
                    return Err(ExpressionError::Parse {
                        position: start,
                        message: "unexpected `=`; use `==` for equality".to_owned(),
                    });
                }
            }
            '>' => {
                self.bump_char();
                if self.peek_char() == Some('=') {
                    self.bump_char();
                    TokenKind::Gte
                } else {
                    TokenKind::Gt
                }
            }
            '<' => {
                self.bump_char();
                if self.peek_char() == Some('=') {
                    self.bump_char();
                    TokenKind::Lte
                } else {
                    TokenKind::Lt
                }
            }
            '&' => {
                self.bump_char();
                if self.peek_char() == Some('&') {
                    self.bump_char();
                    TokenKind::AndAnd
                } else {
                    return Err(ExpressionError::Parse {
                        position: start,
                        message: "unexpected `&`; use `&&`".to_owned(),
                    });
                }
            }
            '|' => {
                self.bump_char();
                if self.peek_char() == Some('|') {
                    self.bump_char();
                    TokenKind::OrOr
                } else {
                    return Err(ExpressionError::Parse {
                        position: start,
                        message: "unexpected `|`; use `||`".to_owned(),
                    });
                }
            }
            '\'' | '"' => TokenKind::String(self.read_string(ch)?),
            c if c.is_ascii_digit() => TokenKind::Number(self.read_number()?),
            c if is_ident_start(c) => TokenKind::Identifier(self.read_identifier()),
            _ => {
                return Err(ExpressionError::Parse {
                    position: start,
                    message: format!("unexpected character `{ch}`"),
                });
            }
        };

        Ok(Token {
            kind: token,
            span: start..self.position,
        })
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek_char(), Some(ch) if ch.is_whitespace()) {
            self.bump_char();
        }
    }

    fn read_string(&mut self, quote: char) -> Result<String, ExpressionError> {
        let start = self.position;
        self.bump_char();
        let mut out = String::new();

        loop {
            let Some(ch) = self.peek_char() else {
                return Err(ExpressionError::Parse {
                    position: start,
                    message: "unterminated string literal".to_owned(),
                });
            };

            self.bump_char();
            match ch {
                c if c == quote => return Ok(out),
                '\\' => {
                    let Some(escaped) = self.peek_char() else {
                        return Err(ExpressionError::UnexpectedEof);
                    };
                    self.bump_char();
                    let decoded = match escaped {
                        '\\' => '\\',
                        '\'' => '\'',
                        '"' => '"',
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        other => other,
                    };
                    out.push(decoded);
                }
                other => out.push(other),
            }
        }
    }

    fn read_number(&mut self) -> Result<f32, ExpressionError> {
        let start = self.position;
        while matches!(self.peek_char(), Some(ch) if ch.is_ascii_digit()) {
            self.bump_char();
        }
        if self.peek_char() == Some('.') {
            self.bump_char();
            while matches!(self.peek_char(), Some(ch) if ch.is_ascii_digit()) {
                self.bump_char();
            }
        }

        self.source[start..self.position]
            .parse::<f32>()
            .map_err(|err| ExpressionError::Parse {
                position: start,
                message: format!("invalid number: {err}"),
            })
    }

    fn read_identifier(&mut self) -> String {
        let start = self.position;
        self.bump_char();
        while matches!(self.peek_char(), Some(ch) if is_ident_continue(ch)) {
            self.bump_char();
        }
        self.source[start..self.position].to_owned()
    }

    fn peek_char(&self) -> Option<char> {
        self.source[self.position..].chars().next()
    }

    fn bump_char(&mut self) -> Option<char> {
        let ch = self.peek_char()?;
        self.position += ch.len_utf8();
        Some(ch)
    }
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    is_ident_start(ch) || ch.is_ascii_digit()
}

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<Token>,
    index: usize,
    references: Vec<ExpressionReference>,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            tokens: Vec::new(),
            index: 0,
            references: Vec::new(),
        }
    }

    fn parse_full(&mut self) -> Result<(ExprNode, Vec<ExpressionReference>), ExpressionError> {
        if self.tokens.is_empty() {
            self.tokenize()?;
        }
        let expr = self.parse_or()?;
        self.expect_kind("end of input", |kind| matches!(kind, TokenKind::Eof))?;
        Ok((expr, self.references.clone()))
    }

    fn tokenize(&mut self) -> Result<(), ExpressionError> {
        let mut lexer = Lexer::new(self.source);
        loop {
            let token = lexer.next_token()?;
            let is_eof = matches!(token.kind, TokenKind::Eof);
            self.tokens.push(token);
            if is_eof {
                break;
            }
        }
        Ok(())
    }

    fn parse_or(&mut self) -> Result<ExprNode, ExpressionError> {
        let mut node = self.parse_and()?;
        while self.match_kind(|kind| matches!(kind, TokenKind::OrOr)) {
            let right = self.parse_and()?;
            node = ExprNode::Binary {
                op: BinaryOp::Or,
                left: Box::new(node),
                right: Box::new(right),
            };
        }
        Ok(node)
    }

    fn parse_and(&mut self) -> Result<ExprNode, ExpressionError> {
        let mut node = self.parse_equality()?;
        while self.match_kind(|kind| matches!(kind, TokenKind::AndAnd)) {
            let right = self.parse_equality()?;
            node = ExprNode::Binary {
                op: BinaryOp::And,
                left: Box::new(node),
                right: Box::new(right),
            };
        }
        Ok(node)
    }

    fn parse_equality(&mut self) -> Result<ExprNode, ExpressionError> {
        let mut node = self.parse_comparison()?;
        loop {
            let op = if self.match_kind(|kind| matches!(kind, TokenKind::EqEq)) {
                Some(BinaryOp::Eq)
            } else if self.match_kind(|kind| matches!(kind, TokenKind::Neq)) {
                Some(BinaryOp::Neq)
            } else {
                None
            };
            let Some(op) = op else { break };
            let right = self.parse_comparison()?;
            node = ExprNode::Binary {
                op,
                left: Box::new(node),
                right: Box::new(right),
            };
        }
        Ok(node)
    }

    fn parse_comparison(&mut self) -> Result<ExprNode, ExpressionError> {
        let mut node = self.parse_term()?;
        loop {
            let op = if self.match_kind(|kind| matches!(kind, TokenKind::Gte)) {
                Some(BinaryOp::Gte)
            } else if self.match_kind(|kind| matches!(kind, TokenKind::Lte)) {
                Some(BinaryOp::Lte)
            } else if self.match_kind(|kind| matches!(kind, TokenKind::Gt)) {
                Some(BinaryOp::Gt)
            } else if self.match_kind(|kind| matches!(kind, TokenKind::Lt)) {
                Some(BinaryOp::Lt)
            } else {
                None
            };
            let Some(op) = op else { break };
            let right = self.parse_term()?;
            node = ExprNode::Binary {
                op,
                left: Box::new(node),
                right: Box::new(right),
            };
        }
        Ok(node)
    }

    fn parse_term(&mut self) -> Result<ExprNode, ExpressionError> {
        let mut node = self.parse_factor()?;
        loop {
            let op = if self.match_kind(|kind| matches!(kind, TokenKind::Plus)) {
                Some(BinaryOp::Add)
            } else if self.match_kind(|kind| matches!(kind, TokenKind::Minus)) {
                Some(BinaryOp::Sub)
            } else {
                None
            };
            let Some(op) = op else { break };
            let right = self.parse_factor()?;
            node = ExprNode::Binary {
                op,
                left: Box::new(node),
                right: Box::new(right),
            };
        }
        Ok(node)
    }

    fn parse_factor(&mut self) -> Result<ExprNode, ExpressionError> {
        let mut node = self.parse_unary()?;
        loop {
            let op = if self.match_kind(|kind| matches!(kind, TokenKind::Star)) {
                Some(BinaryOp::Mul)
            } else if self.match_kind(|kind| matches!(kind, TokenKind::Slash)) {
                Some(BinaryOp::Div)
            } else if self.match_kind(|kind| matches!(kind, TokenKind::Percent)) {
                Some(BinaryOp::Mod)
            } else {
                None
            };
            let Some(op) = op else { break };
            let right = self.parse_unary()?;
            node = ExprNode::Binary {
                op,
                left: Box::new(node),
                right: Box::new(right),
            };
        }
        Ok(node)
    }

    fn parse_unary(&mut self) -> Result<ExprNode, ExpressionError> {
        if self.match_kind(|kind| matches!(kind, TokenKind::Bang)) {
            return Ok(ExprNode::Unary {
                op: UnaryOp::Not,
                expr: Box::new(self.parse_unary()?),
            });
        }
        if self.match_kind(|kind| matches!(kind, TokenKind::Minus)) {
            return Ok(ExprNode::Unary {
                op: UnaryOp::Neg,
                expr: Box::new(self.parse_unary()?),
            });
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<ExprNode, ExpressionError> {
        let token = self.peek().cloned().ok_or(ExpressionError::UnexpectedEof)?;
        match token.kind {
            TokenKind::Number(value) => {
                self.index += 1;
                Ok(ExprNode::Number(value))
            }
            TokenKind::String(value) => {
                self.index += 1;
                Ok(ExprNode::String(value))
            }
            TokenKind::Identifier(ref ident) if ident == "true" || ident == "false" => {
                self.index += 1;
                Ok(ExprNode::Boolean(ident == "true"))
            }
            TokenKind::Identifier(_) => self.parse_identifier_expression(),
            TokenKind::LParen => {
                self.index += 1;
                let expr = self.parse_or()?;
                self.expect_kind("`)`", |kind| matches!(kind, TokenKind::RParen))?;
                Ok(expr)
            }
            _ => Err(ExpressionError::Parse {
                position: token.span.start,
                message: "expected expression".to_owned(),
            }),
        }
    }

    fn parse_identifier_expression(&mut self) -> Result<ExprNode, ExpressionError> {
        let name_token = self.next_token()?.clone();
        let name = match &name_token.kind {
            TokenKind::Identifier(name) => name.clone(),
            _ => {
                return Err(ExpressionError::Parse {
                    position: name_token.span.start,
                    message: "expected identifier".to_owned(),
                });
            }
        };

        if !self.match_kind(|kind| matches!(kind, TokenKind::LParen)) {
            return Err(ExpressionError::Parse {
				position: name_token.span.start,
				message: "unexpected bare identifier; only booleans, function calls, and references are supported"
					.to_owned(),
			});
        }

        let mut args = Vec::new();
        if !self.match_kind(|kind| matches!(kind, TokenKind::RParen)) {
            loop {
                args.push(self.parse_or()?);
                if self.match_kind(|kind| matches!(kind, TokenKind::Comma)) {
                    continue;
                }
                self.expect_kind("`)`", |kind| matches!(kind, TokenKind::RParen))?;
                break;
            }
        }

        let call_end = self.previous_span_end();
        if (name == "clip" || name == "layout")
            && self.match_kind(|kind| matches!(kind, TokenKind::Dot))
        {
            let property_token = self.next_token()?.clone();
            let property_name = match &property_token.kind {
                TokenKind::Identifier(value) => value.as_str(),
                _ => {
                    return Err(ExpressionError::Parse {
                        position: property_token.span.start,
                        message: "expected property name after `.`".to_owned(),
                    });
                }
            };

            let property =
                ExpressionProperty::parse(property_name).ok_or_else(|| ExpressionError::Parse {
                    position: property_token.span.start,
                    message: format!("unknown property `{property_name}`"),
                })?;

            if args.len() != 1 {
                return Err(ExpressionError::InvalidArgumentCount {
                    name,
                    expected: "1",
                    got: args.len(),
                });
            }
            let id = match &args[0] {
                ExprNode::String(value) => value.clone(),
                other => {
                    return Err(ExpressionError::TypeMismatch {
                        context: "reference id",
                        expected: "string",
                        found: other.kind_name(),
                    });
                }
            };

            let span = name_token.span.start..property_token.span.end;
            let target = if name == "clip" {
                ExpressionReferenceTarget::ClipProperty {
                    clip_id: id.clone(),
                    property,
                }
            } else {
                ExpressionReferenceTarget::LayoutNodeProperty {
                    node_id: id.clone(),
                    property,
                }
            };
            self.references.push(ExpressionReference {
                target: target.clone(),
                span,
            });

            return Ok(match target {
                ExpressionReferenceTarget::ClipProperty { clip_id, property } => {
                    ExprNode::ClipRef { clip_id, property }
                }
                ExpressionReferenceTarget::LayoutNodeProperty { node_id, property } => {
                    ExprNode::LayoutRef { node_id, property }
                }
            });
        }

        if name == "clip" || name == "layout" {
            return Err(ExpressionError::Parse {
                position: call_end,
                message: "expected `.property` after reference call".to_owned(),
            });
        }

        Ok(ExprNode::FuncCall { name, args })
    }

    fn match_kind(&mut self, predicate: impl Fn(&TokenKind) -> bool) -> bool {
        match self.peek() {
            Some(token) if predicate(&token.kind) => {
                self.index += 1;
                true
            }
            _ => false,
        }
    }

    fn expect_kind(
        &mut self,
        expected: &str,
        predicate: impl Fn(&TokenKind) -> bool,
    ) -> Result<(), ExpressionError> {
        let Some(token) = self.peek().cloned() else {
            return Err(ExpressionError::UnexpectedEof);
        };

        if predicate(&token.kind) {
            self.index += 1;
            Ok(())
        } else {
            Err(ExpressionError::Parse {
                position: token.span.start,
                message: format!("expected {expected}"),
            })
        }
    }

    fn next_token(&mut self) -> Result<&Token, ExpressionError> {
        let token = self
            .tokens
            .get(self.index)
            .ok_or(ExpressionError::UnexpectedEof)?;
        self.index += 1;
        Ok(token)
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.index)
    }

    fn previous_span_end(&self) -> usize {
        self.index
            .checked_sub(1)
            .and_then(|idx| self.tokens.get(idx))
            .map_or(0, |token| token.span.end)
    }
}

impl ExprNode {
    fn kind_name(&self) -> &'static str {
        match self {
            Self::Number(_) => "number",
            Self::Boolean(_) => "boolean",
            Self::String(_) => "string",
            Self::ClipRef { .. } => "reference",
            Self::LayoutRef { .. } => "reference",
            Self::Unary { .. } => "expression",
            Self::Binary { .. } => "expression",
            Self::FuncCall { .. } => "expression",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Expression, ExpressionError, ExpressionId, ExpressionProperty, ExpressionReferenceTarget,
        ExpressionScope, ExpressionValue, evaluate_expression, parse_expression,
    };

    fn parse(source: &str) -> Expression {
        Expression::parse(ExpressionId("expr".to_owned()), source).expect("expression should parse")
    }

    #[test]
    fn parse_collects_clip_and_layout_references_with_spans() {
        let source = "clip('bg').width + layout(\"nav\").height";
        let expression = parse(source);

        assert_eq!(expression.references.len(), 2);
        assert_eq!(
            expression.references[0].target,
            ExpressionReferenceTarget::ClipProperty {
                clip_id: "bg".to_owned(),
                property: ExpressionProperty::Width,
            }
        );
        assert_eq!(
            &source[expression.references[0].span.clone()],
            "clip('bg').width"
        );
        assert_eq!(
            expression.references[1].target,
            ExpressionReferenceTarget::LayoutNodeProperty {
                node_id: "nav".to_owned(),
                property: ExpressionProperty::Height,
            }
        );
        assert_eq!(
            &source[expression.references[1].span.clone()],
            "layout(\"nav\").height"
        );
    }

    #[test]
    fn evaluate_respects_operator_precedence() {
        let expression = parse("1 + 2 * 3");
        let value = expression
            .evaluate(&ExpressionScope::default())
            .expect("expression should evaluate");

        assert_eq!(value, ExpressionValue::Number(7.0));
    }

    #[test]
    fn evaluate_reads_clip_and_layout_scope_values() {
        let expression = parse("clip('bg').width * 0.5 + layout('nav').x");
        let mut scope = ExpressionScope::default();
        scope.clip_properties.insert(
            ("bg".to_owned(), ExpressionProperty::Width),
            ExpressionValue::Number(400.0),
        );
        scope.layout_properties.insert(
            ("nav".to_owned(), ExpressionProperty::X),
            ExpressionValue::Number(24.0),
        );

        let value = expression
            .evaluate(&scope)
            .expect("expression should evaluate");

        assert_eq!(value, ExpressionValue::Number(224.0));
    }

    #[test]
    fn evaluate_supports_builtin_functions() {
        let expression = parse("max(1, min(10, 20), clamp(15, 0, 12)) + lerp(0, 8, 0.5)");
        let value = expression
            .evaluate(&ExpressionScope::default())
            .expect("expression should evaluate");

        assert_eq!(value, ExpressionValue::Number(16.0));
    }

    #[test]
    fn evaluate_supports_conditional_and_logical_ops() {
        let expression = parse("if(clip('t').opacity > 0 && true, 100, 0)");
        let mut scope = ExpressionScope::default();
        scope.clip_properties.insert(
            ("t".to_owned(), ExpressionProperty::Opacity),
            ExpressionValue::Number(0.2),
        );

        let value = evaluate_expression(&expression, &scope).expect("expression should evaluate");
        assert_eq!(value, ExpressionValue::Number(100.0));
    }

    #[test]
    fn evaluate_errors_on_unresolved_reference() {
        let expression = parse("clip('missing').width");
        let err = expression
            .evaluate(&ExpressionScope::default())
            .expect_err("missing reference should error");

        assert!(matches!(err, ExpressionError::UnresolvedReference { .. }));
    }

    #[test]
    fn parse_errors_on_unknown_property() {
        let err = parse_expression(ExpressionId("expr".to_owned()), "clip('a').depth")
            .expect_err("unknown property should fail");

        assert!(matches!(err, ExpressionError::Parse { .. }));
    }

    #[test]
    fn evaluate_errors_on_type_mismatch() {
        let expression = parse("min('a', 2)");
        let err = expression
            .evaluate(&ExpressionScope::default())
            .expect_err("type mismatch should fail");

        assert!(matches!(err, ExpressionError::TypeMismatch { .. }));
    }
}
