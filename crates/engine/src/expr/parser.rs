use std::{collections::HashSet, ops::Range};

use crate::{
    error::ExpressionError,
    expr::{
        ast::{
            BinaryOp, ExprNode, Expression, ExpressionId, ExpressionReference, ExpressionValue,
            GlobalVar, PropertyPath, UnaryOp,
        },
        builtins::range_error,
        easing::Easing,
        functions,
        spec::{BuiltinSpec, ValueKind, builtin_for_name},
    },
    node::NodeId,
};

#[derive(Debug, Clone, PartialEq)]
enum TokenKind {
    Identifier(String),
    Number(f64),
    /// A number with an `s` or `ms` suffix, in seconds.
    Seconds(f64),
    Color([u8; 4]),
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
            '#' => TokenKind::Color(self.read_color()?),
            '\'' | '"' => TokenKind::String(self.read_string(ch)?),
            c if c.is_ascii_digit() => self.read_number_or_duration()?,
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

    fn read_number_or_duration(&mut self) -> Result<TokenKind, ExpressionError> {
        let start = self.position;
        let value = self.read_number()?;
        if !matches!(self.peek_char(), Some(ch) if is_ident_start(ch)) {
            return Ok(TokenKind::Number(value));
        }
        match self.read_identifier().as_str() {
            "s" => Ok(TokenKind::Seconds(value)),
            "ms" => Ok(TokenKind::Seconds(value / 1000.0)),
            unit => Err(ExpressionError::Parse {
                path: None,
                details: format!(
                    "unknown unit `{unit}` after the number at byte {start}; use `s` or `ms`"
                ),
            }),
        }
    }

    fn read_color(&mut self) -> Result<[u8; 4], ExpressionError> {
        let start = self.position;
        self.bump_char();
        while matches!(self.peek_char(), Some(ch) if ch.is_ascii_alphanumeric()) {
            self.bump_char();
        }
        let literal = &self.source[start..self.position];
        let hex = &literal[1..];
        if !(matches!(hex.len(), 6 | 8) && hex.bytes().all(|byte| byte.is_ascii_hexdigit())) {
            return Err(ExpressionError::Parse {
                path: None,
                details: format!("invalid colour `{literal}`; use #rrggbb or #rrggbbaa"),
            });
        }
        let mut color = [255; 4];
        for (index, channel) in color.iter_mut().take(hex.len() / 2).enumerate() {
            *channel = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).expect("hex digits");
        }
        Ok(color)
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

/// The distinct nodes and properties `ast` reads, in the order they first appear.
fn references(ast: &ExprNode) -> Vec<ExpressionReference> {
    let mut seen = HashSet::new();
    let mut references = Vec::new();
    functions::visit(ast, &mut |node| {
        let reference = match node {
            ExprNode::SymbolicPath(segments) => ExpressionReference::SymbolicPath {
                segments: segments.clone(),
            },
            ExprNode::Node(node_id) => ExpressionReference::Node { node_id: *node_id },
            ExprNode::PropertyValue(node_id, property_path) => ExpressionReference::PropertyValue {
                node_id: *node_id,
                property_path: property_path.clone(),
            },
            ExprNode::VirtualProperty(id) => ExpressionReference::VirtualProperty { id: *id },
            _ => return,
        };
        if seen.insert(reference.clone()) {
            references.push(reference);
        }
    });
    references
}

fn tokenize(source: &str) -> Result<Vec<TokenKind>, ExpressionError> {
    let mut lexer = Lexer::new(source);
    let mut tokens = Vec::new();
    loop {
        match lexer.next_token()?.kind {
            TokenKind::Eof => return Ok(tokens),
            kind => tokens.push(kind),
        }
    }
}

/// Splits a function signature such as `rise(start, distance)` into its name and parameters.
pub(crate) fn parse_signature(signature: &str) -> Result<(String, Vec<String>), ExpressionError> {
    let invalid = || ExpressionError::Parse {
        path: None,
        details: format!("invalid function signature `{signature}`; expected `name(a, b)`"),
    };
    let tokens = tokenize(signature).map_err(|_| invalid())?;
    let [
        TokenKind::Identifier(name),
        TokenKind::LParen,
        rest @ ..,
        TokenKind::RParen,
    ] = &tokens[..]
    else {
        return Err(invalid());
    };
    let mut params = Vec::new();
    for (index, token) in rest.iter().enumerate() {
        match token {
            TokenKind::Identifier(param) if index % 2 == 0 => params.push(param.clone()),
            TokenKind::Comma if index % 2 == 1 && index + 1 < rest.len() => {}
            _ => return Err(invalid()),
        }
    }
    Ok((name.clone(), params))
}

/// Names called anywhere in `source`, such as `rise` in `rise(4) + 1`.
pub(crate) fn called_names(source: &str) -> Result<Vec<String>, ExpressionError> {
    let tokens = tokenize(source)?;
    Ok(tokens
        .windows(2)
        .filter_map(|pair| match pair {
            [TokenKind::Identifier(name), TokenKind::LParen] => Some(name.clone()),
            _ => None,
        })
        .collect())
}

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<Token>,
    position: usize,
    budget: functions::Budget,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            tokens: Vec::new(),
            position: 0,
            budget: functions::Budget::default(),
        }
    }

    fn parse(&mut self) -> Result<Expression, ExpressionError> {
        self.tokenize()?;
        let ast = self.parse_or()?;
        self.expect(TokenExpectation::Eof)?;
        Ok(Expression {
            id: ExpressionId(0),
            references: references(&ast),
            ast,
            source: self.source.to_string(),
        })
    }

    fn binary(
        &mut self,
        lhs: ExprNode,
        op: BinaryOp,
        rhs: ExprNode,
    ) -> Result<ExprNode, ExpressionError> {
        self.budget
            .push(ExprNode::Binary(Box::new(lhs), op, Box::new(rhs)))
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
            node = self.binary(node, BinaryOp::Or, rhs)?;
        }
        Ok(node)
    }

    fn parse_and(&mut self) -> Result<ExprNode, ExpressionError> {
        let mut node = self.parse_equality()?;
        while self.match_token(|kind| matches!(kind, TokenKind::AndAnd)) {
            let rhs = self.parse_equality()?;
            node = self.binary(node, BinaryOp::And, rhs)?;
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
            node = self.binary(node, op, rhs)?;
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
            node = self.binary(node, op, rhs)?;
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
            node = self.binary(node, op, rhs)?;
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
            node = self.binary(node, op, rhs)?;
        }
        Ok(node)
    }

    fn parse_unary(&mut self) -> Result<ExprNode, ExpressionError> {
        if self.match_token(|kind| matches!(kind, TokenKind::Bang)) {
            let value = self.parse_unary()?;
            return self
                .budget
                .push(ExprNode::Unary(UnaryOp::Not, Box::new(value)));
        }
        if self.match_token(|kind| matches!(kind, TokenKind::Minus)) {
            let value = self.parse_unary()?;
            return self
                .budget
                .push(ExprNode::Unary(UnaryOp::Neg, Box::new(value)));
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
                self.budget
                    .push(ExprNode::Literal(ExpressionValue::Number(value)))
            }
            TokenKind::Seconds(value) => {
                self.position += 1;
                let seconds = self
                    .budget
                    .push(ExprNode::Literal(ExpressionValue::Number(value)))?;
                let fps = self.budget.push(ExprNode::Global(GlobalVar::Fps))?;
                self.binary(seconds, BinaryOp::Mul, fps)
            }
            TokenKind::String(value) => {
                self.position += 1;
                self.budget
                    .push(ExprNode::Literal(ExpressionValue::String(value)))
            }
            TokenKind::Color(value) => {
                self.position += 1;
                self.budget
                    .push(ExprNode::Literal(ExpressionValue::color_from_u8(value)))
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
            return self.budget.push(ExprNode::SymbolicPath(dotted_segments));
        }

        if identifier == "true" {
            return self
                .budget
                .push(ExprNode::Literal(ExpressionValue::Boolean(true)));
        }
        if identifier == "false" {
            return self
                .budget
                .push(ExprNode::Literal(ExpressionValue::Boolean(false)));
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
                let mut args = args.into_iter();
                let (condition, when_true, when_false) = (
                    Box::new(args.next().expect("3 args")),
                    Box::new(args.next().expect("3 args")),
                    Box::new(args.next().expect("3 args")),
                );
                return self
                    .budget
                    .push(ExprNode::Conditional(condition, when_true, when_false));
            }

            if identifier == "node" {
                return self.parse_node_reference(args);
            }

            if let Some(spec) = builtin_for_name(&identifier) {
                validate_builtin_call(spec, &args)?;
                return self.budget.push(ExprNode::Builtin(spec.builtin, args));
            }
            return functions::expand(&identifier, &args, &mut self.budget)?.ok_or(
                ExpressionError::Parse {
                    path: None,
                    details: format!("unknown function `{identifier}`"),
                },
            );
        }

        self.budget
            .push(ExprNode::Global(match identifier.as_str() {
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
            return self.budget.push(ExprNode::Node(node_id));
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

        self.budget
            .push(ExprNode::PropertyValue(node_id, property_path))
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

/// Checks what can be known before evaluation: arity, easing names and literal spring ranges.
fn validate_builtin_call(spec: &BuiltinSpec, args: &[ExprNode]) -> Result<(), ExpressionError> {
    let error = |details: String| ExpressionError::Parse {
        path: None,
        details: format!("`{}`: {details}", spec.name),
    };
    if !spec.accepts_arity(args.len()) {
        return Err(error(format!(
            "expects {} arguments, got {}",
            spec.arity_description(),
            args.len()
        )));
    }
    for (arg, param) in args.iter().zip(spec.args) {
        if param.kind != ValueKind::Easing {
            continue;
        }
        let ExprNode::Literal(ExpressionValue::String(name)) = arg else {
            return Err(error(format!(
                "`{}` must be a quoted easing name such as 'out_expo'",
                param.name
            )));
        };
        if Easing::from_name(name).is_none() {
            let names = Easing::ALL.map(Easing::name).join(", ");
            return Err(error(format!(
                "unknown easing `{name}`; expected one of {names}"
            )));
        }
    }
    for (index, arg) in args.iter().enumerate() {
        if let ExprNode::Literal(ExpressionValue::Number(value)) = arg
            && let Some(details) = range_error(spec.builtin, index, *value)
        {
            return Err(error(details.to_string()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::expr::{BuiltinFn, ExprNode, Expression, ExpressionValue};

    #[test]
    fn recognizes_vec2_builtin_and_preserves_components() {
        let expression = Expression::parse("vec2(10, frame + 2)").unwrap();

        let ExprNode::Builtin(BuiltinFn::Vec2, args) = expression.ast else {
            panic!("expected vec2 builtin");
        };
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], ExprNode::Literal(ExpressionValue::Number(10.0)));
        assert!(matches!(args[1], ExprNode::Binary(..)));
    }
}
