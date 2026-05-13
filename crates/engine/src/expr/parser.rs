use std::ops::Range;

use crate::{
    error::ExpressionError,
    expr::ast::{
        BinaryOp, BuiltinFn, ExprNode, Expression, ExpressionId, ExpressionReference,
        ExpressionValue, GlobalVar, PropertyPath, UnaryOp,
    },
    node::NodeId,
};

#[derive(Debug, Clone, PartialEq)]
enum TokenKind {
    Identifier(String),
    Number(f64),
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

        let kind = match ch {
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
                        path: None,
                        details: "unexpected `=`; use `==` for equality".to_string(),
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
                        path: None,
                        details: "unexpected `&`; use `&&`".to_string(),
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
                        path: None,
                        details: "unexpected `|`; use `||`".to_string(),
                    });
                }
            }
            '\'' | '"' => TokenKind::String(self.read_string(ch)?),
            c if c.is_ascii_digit() => TokenKind::Number(self.read_number()?),
            c if is_ident_start(c) => TokenKind::Identifier(self.read_identifier()),
            other => {
                return Err(ExpressionError::Parse {
                    path: None,
                    details: format!("unexpected character `{other}` at byte {start}"),
                });
            }
        };

        Ok(Token {
            kind,
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
        let mut output = String::new();
        loop {
            let Some(ch) = self.peek_char() else {
                return Err(ExpressionError::Parse {
                    path: None,
                    details: format!("unterminated string literal starting at byte {start}"),
                });
            };
            self.bump_char();
            match ch {
                c if c == quote => return Ok(output),
                '\\' => {
                    let Some(escaped) = self.peek_char() else {
                        return Err(ExpressionError::Parse {
                            path: None,
                            details: "unterminated escape sequence".to_string(),
                        });
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
                    output.push(decoded);
                }
                other => output.push(other),
            }
        }
    }

    fn read_number(&mut self) -> Result<f64, ExpressionError> {
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
            .parse::<f64>()
            .map_err(|error| ExpressionError::Parse {
                path: None,
                details: format!("invalid number at byte {start}: {error}"),
            })
    }

    fn read_identifier(&mut self) -> String {
        let start = self.position;
        self.bump_char();
        while matches!(self.peek_char(), Some(ch) if is_ident_continue(ch)) {
            self.bump_char();
        }
        self.source[start..self.position].to_string()
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

pub fn parse_expression(source: &str) -> Result<Expression, ExpressionError> {
    let mut parser = Parser::new(source);
    parser.parse()
}

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<Token>,
    position: usize,
    references: Vec<ExpressionReference>,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            tokens: Vec::new(),
            position: 0,
            references: Vec::new(),
        }
    }

    fn parse(&mut self) -> Result<Expression, ExpressionError> {
        self.tokenize()?;
        let ast = self.parse_or()?;
        self.expect(TokenExpectation::Eof)?;
        Ok(Expression {
            id: ExpressionId(0),
            ast,
            references: self.references.clone(),
            source: self.source.to_string(),
        })
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
        while self.match_token(|kind| matches!(kind, TokenKind::OrOr)) {
            let rhs = self.parse_and()?;
            node = ExprNode::Binary(Box::new(node), BinaryOp::Or, Box::new(rhs));
        }
        Ok(node)
    }

    fn parse_and(&mut self) -> Result<ExprNode, ExpressionError> {
        let mut node = self.parse_equality()?;
        while self.match_token(|kind| matches!(kind, TokenKind::AndAnd)) {
            let rhs = self.parse_equality()?;
            node = ExprNode::Binary(Box::new(node), BinaryOp::And, Box::new(rhs));
        }
        Ok(node)
    }

    fn parse_equality(&mut self) -> Result<ExprNode, ExpressionError> {
        let mut node = self.parse_comparison()?;
        loop {
            let op = if self.match_token(|kind| matches!(kind, TokenKind::EqEq)) {
                Some(BinaryOp::Eq)
            } else if self.match_token(|kind| matches!(kind, TokenKind::Neq)) {
                Some(BinaryOp::Neq)
            } else {
                None
            };
            let Some(op) = op else {
                break;
            };
            let rhs = self.parse_comparison()?;
            node = ExprNode::Binary(Box::new(node), op, Box::new(rhs));
        }
        Ok(node)
    }

    fn parse_comparison(&mut self) -> Result<ExprNode, ExpressionError> {
        let mut node = self.parse_term()?;
        loop {
            let op = if self.match_token(|kind| matches!(kind, TokenKind::Gte)) {
                Some(BinaryOp::Gte)
            } else if self.match_token(|kind| matches!(kind, TokenKind::Lte)) {
                Some(BinaryOp::Lte)
            } else if self.match_token(|kind| matches!(kind, TokenKind::Gt)) {
                Some(BinaryOp::Gt)
            } else if self.match_token(|kind| matches!(kind, TokenKind::Lt)) {
                Some(BinaryOp::Lt)
            } else {
                None
            };
            let Some(op) = op else {
                break;
            };
            let rhs = self.parse_term()?;
            node = ExprNode::Binary(Box::new(node), op, Box::new(rhs));
        }
        Ok(node)
    }

    fn parse_term(&mut self) -> Result<ExprNode, ExpressionError> {
        let mut node = self.parse_factor()?;
        loop {
            let op = if self.match_token(|kind| matches!(kind, TokenKind::Plus)) {
                Some(BinaryOp::Add)
            } else if self.match_token(|kind| matches!(kind, TokenKind::Minus)) {
                Some(BinaryOp::Sub)
            } else {
                None
            };
            let Some(op) = op else {
                break;
            };
            let rhs = self.parse_factor()?;
            node = ExprNode::Binary(Box::new(node), op, Box::new(rhs));
        }
        Ok(node)
    }

    fn parse_factor(&mut self) -> Result<ExprNode, ExpressionError> {
        let mut node = self.parse_unary()?;
        loop {
            let op = if self.match_token(|kind| matches!(kind, TokenKind::Star)) {
                Some(BinaryOp::Mul)
            } else if self.match_token(|kind| matches!(kind, TokenKind::Slash)) {
                Some(BinaryOp::Div)
            } else if self.match_token(|kind| matches!(kind, TokenKind::Percent)) {
                Some(BinaryOp::Mod)
            } else {
                None
            };
            let Some(op) = op else {
                break;
            };
            let rhs = self.parse_unary()?;
            node = ExprNode::Binary(Box::new(node), op, Box::new(rhs));
        }
        Ok(node)
    }

    fn parse_unary(&mut self) -> Result<ExprNode, ExpressionError> {
        if self.match_token(|kind| matches!(kind, TokenKind::Bang)) {
            return Ok(ExprNode::Unary(UnaryOp::Not, Box::new(self.parse_unary()?)));
        }
        if self.match_token(|kind| matches!(kind, TokenKind::Minus)) {
            return Ok(ExprNode::Unary(UnaryOp::Neg, Box::new(self.parse_unary()?)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<ExprNode, ExpressionError> {
        let token = self.peek().cloned().ok_or(ExpressionError::Parse {
            path: None,
            details: "unexpected end of expression".to_string(),
        })?;
        match token.kind {
            TokenKind::Number(value) => {
                self.position += 1;
                Ok(ExprNode::Literal(ExpressionValue::Number(value)))
            }
            TokenKind::String(value) => {
                self.position += 1;
                Ok(ExprNode::Literal(ExpressionValue::String(value)))
            }
            TokenKind::Identifier(identifier) => {
                self.position += 1;
                self.parse_identifier(identifier)
            }
            TokenKind::LParen => {
                self.position += 1;
                let expr = self.parse_or()?;
                self.expect(TokenExpectation::RParen)?;
                Ok(expr)
            }
            _ => Err(ExpressionError::Parse {
                path: None,
                details: format!("expected expression at byte {}", token.span.start),
            }),
        }
    }

    fn parse_identifier(&mut self, identifier: String) -> Result<ExprNode, ExpressionError> {
        let mut dotted_segments = vec![identifier.clone()];
        while self.match_token(|kind| matches!(kind, TokenKind::Dot)) {
            let token = self.peek().cloned().ok_or(ExpressionError::Parse {
                path: None,
                details: "expected identifier after `.`".to_string(),
            })?;
            match token.kind {
                TokenKind::Identifier(next_segment) => {
                    self.position += 1;
                    dotted_segments.push(next_segment);
                }
                _ => {
                    return Err(ExpressionError::Parse {
                        path: None,
                        details: "expected identifier after `.`".to_string(),
                    });
                }
            }
        }
        if dotted_segments.len() > 1 {
            self.references.push(ExpressionReference::SymbolicPath {
                segments: dotted_segments.clone(),
            });
            return Ok(ExprNode::SymbolicPath(dotted_segments));
        }

        if identifier == "true" {
            return Ok(ExprNode::Literal(ExpressionValue::Boolean(true)));
        }
        if identifier == "false" {
            return Ok(ExprNode::Literal(ExpressionValue::Boolean(false)));
        }

        if self.match_token(|kind| matches!(kind, TokenKind::LParen)) {
            let mut args = Vec::new();
            if !self.match_token(|kind| matches!(kind, TokenKind::RParen)) {
                loop {
                    args.push(self.parse_or()?);
                    if self.match_token(|kind| matches!(kind, TokenKind::Comma)) {
                        continue;
                    }
                    self.expect(TokenExpectation::RParen)?;
                    break;
                }
            }

            if identifier == "if" {
                if args.len() != 3 {
                    return Err(ExpressionError::Parse {
                        path: None,
                        details: "if(cond, then, else) requires exactly 3 arguments".to_string(),
                    });
                }
                return Ok(ExprNode::Conditional(
                    Box::new(args[0].clone()),
                    Box::new(args[1].clone()),
                    Box::new(args[2].clone()),
                ));
            }

            if identifier == "node" {
                return self.parse_node_reference(args);
            }

            let builtin = builtin_for_name(&identifier).ok_or(ExpressionError::Parse {
                path: None,
                details: format!("unknown function `{identifier}`"),
            })?;
            return Ok(ExprNode::Builtin(builtin, args));
        }

        Ok(ExprNode::Global(match identifier.as_str() {
            "frame" => GlobalVar::Frame,
            "time" => GlobalVar::Time,
            "fps" => GlobalVar::Fps,
            "width" => GlobalVar::Width,
            "height" => GlobalVar::Height,
            _ => GlobalVar::Custom(identifier),
        }))
    }

    fn parse_node_reference(&mut self, args: Vec<ExprNode>) -> Result<ExprNode, ExpressionError> {
        if args.is_empty() || args.len() > 2 {
            return Err(ExpressionError::Parse {
                path: None,
                details: "node(id) or node(id, property_path) requires 1 or 2 arguments"
                    .to_string(),
            });
        }

        let node_id = match &args[0] {
            ExprNode::Literal(ExpressionValue::Number(value)) => {
                if *value < 0.0 || value.fract() != 0.0 {
                    return Err(ExpressionError::Parse {
                        path: None,
                        details: "node(id, ..) expects an unsigned integer id".to_string(),
                    });
                }
                NodeId(*value as u64)
            }
            _ => {
                return Err(ExpressionError::Parse {
                    path: None,
                    details: "node(id, ..) first argument must be a numeric node id".to_string(),
                });
            }
        };

        if args.len() == 1 {
            self.references.push(ExpressionReference::Node { node_id });
            return Ok(ExprNode::Node(node_id));
        }

        let property_path = match &args[1] {
            ExprNode::Literal(ExpressionValue::String(value)) => PropertyPath::new(value.clone()),
            _ => {
                return Err(ExpressionError::Parse {
                    path: None,
                    details: "node(.., property_path) second argument must be a string".to_string(),
                });
            }
        };

        self.references.push(ExpressionReference::NodeProperty {
            node_id,
            property_path: property_path.clone(),
        });

        Ok(ExprNode::NodeProperty(node_id, property_path))
    }

    fn match_token(&mut self, predicate: impl Fn(&TokenKind) -> bool) -> bool {
        match self.peek() {
            Some(token) if predicate(&token.kind) => {
                self.position += 1;
                true
            }
            _ => false,
        }
    }

    fn expect(&mut self, expected: TokenExpectation) -> Result<(), ExpressionError> {
        let token = self.peek().ok_or(ExpressionError::Parse {
            path: None,
            details: format!("expected {}", expected.describe()),
        })?;

        let matches = match expected {
            TokenExpectation::RParen => matches!(token.kind, TokenKind::RParen),
            TokenExpectation::Eof => matches!(token.kind, TokenKind::Eof),
        };
        if matches {
            self.position += 1;
            Ok(())
        } else {
            Err(ExpressionError::Parse {
                path: None,
                details: format!("expected {}", expected.describe()),
            })
        }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }
}

enum TokenExpectation {
    RParen,
    Eof,
}

impl TokenExpectation {
    fn describe(&self) -> &'static str {
        match self {
            Self::RParen => "`)`",
            Self::Eof => "end of input",
        }
    }
}

fn builtin_for_name(name: &str) -> Option<BuiltinFn> {
    match name {
        "min" => Some(BuiltinFn::Min),
        "max" => Some(BuiltinFn::Max),
        "abs" => Some(BuiltinFn::Abs),
        "floor" => Some(BuiltinFn::Floor),
        "ceil" => Some(BuiltinFn::Ceil),
        "round" => Some(BuiltinFn::Round),
        "sin" => Some(BuiltinFn::Sin),
        "cos" => Some(BuiltinFn::Cos),
        "clamp" => Some(BuiltinFn::Clamp),
        "lerp" => Some(BuiltinFn::Lerp),
        "pow" => Some(BuiltinFn::Pow),
        "mod" => Some(BuiltinFn::Mod),
        "fract" => Some(BuiltinFn::Fract),
        "smoothstep" => Some(BuiltinFn::Smoothstep),
        "linear" => Some(BuiltinFn::Linear),
        "step" => Some(BuiltinFn::Step),
        "text_height" => Some(BuiltinFn::TextHeight),
        "text_width" => Some(BuiltinFn::TextWidth),
        "uppercase" => Some(BuiltinFn::Uppercase),
        "lowercase" => Some(BuiltinFn::Lowercase),
        _ => None,
    }
}
