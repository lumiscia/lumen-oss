use std::ops::Range;

use crate::{
    error::ExpressionError,
    expr::{
        ast::{
            BinaryOp, ExprNode, Expression, ExpressionValue, GlobalVar, UnaryOp, parse_hex_color,
        },
        builtins::range_error,
        functions,
        spec::{BuiltinSpec, Easing, ValueKind, builtin_for_name},
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
            '(' => self.one(TokenKind::LParen),
            ')' => self.one(TokenKind::RParen),
            ',' => self.one(TokenKind::Comma),
            '.' if self.digit_follows_dot() => self.read_number_or_duration()?,
            '.' => self.one(TokenKind::Dot),
            '+' => self.one(TokenKind::Plus),
            '-' => self.one(TokenKind::Minus),
            '*' => self.one(TokenKind::Star),
            '/' => self.one(TokenKind::Slash),
            '%' => self.one(TokenKind::Percent),
            '!' => self.two('=', TokenKind::Neq, TokenKind::Bang),
            '>' => self.two('=', TokenKind::Gte, TokenKind::Gt),
            '<' => self.two('=', TokenKind::Lte, TokenKind::Lt),
            '=' => self.doubled(
                '=',
                TokenKind::EqEq,
                "unexpected `=`; use `==` for equality",
            )?,
            '&' => self.doubled('&', TokenKind::AndAnd, "unexpected `&`; use `&&`")?,
            '|' => self.doubled('|', TokenKind::OrOr, "unexpected `|`; use `||`")?,
            '#' => TokenKind::Color(self.read_color()?),
            '\'' | '"' => TokenKind::String(self.read_string(ch)?),
            c if c.is_ascii_digit() => self.read_number_or_duration()?,
            c if is_ident_start(c) => TokenKind::Identifier(self.read_identifier()),
            other => {
                return Err(ExpressionError::Parse {
                    details: format!("unexpected character `{other}` at byte {start}"),
                });
            }
        };

        Ok(Token {
            kind,
            span: start..self.position,
        })
    }

    fn one(&mut self, kind: TokenKind) -> TokenKind {
        self.bump_char();
        kind
    }

    /// `pair` when the current character is followed by `second`, otherwise `single`.
    fn two(&mut self, second: char, pair: TokenKind, single: TokenKind) -> TokenKind {
        self.bump_char();
        if self.peek_char() == Some(second) {
            self.one(pair)
        } else {
            single
        }
    }

    /// `pair` when the current character is followed by `second`, otherwise a failure.
    fn doubled(
        &mut self,
        second: char,
        pair: TokenKind,
        alone: &str,
    ) -> Result<TokenKind, ExpressionError> {
        self.bump_char();
        if self.peek_char() == Some(second) {
            Ok(self.one(pair))
        } else {
            Err(ExpressionError::Parse {
                details: alone.to_string(),
            })
        }
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek_char(), Some(ch) if ch.is_whitespace()) {
            self.bump_char();
        }
    }

    fn skip_digits(&mut self) {
        while matches!(self.peek_char(), Some(ch) if ch.is_ascii_digit()) {
            self.bump_char();
        }
    }

    fn digit_follows_dot(&self) -> bool {
        self.source[self.position + 1..].starts_with(|ch: char| ch.is_ascii_digit())
    }

    fn read_string(&mut self, quote: char) -> Result<String, ExpressionError> {
        let start = self.position;
        self.bump_char();
        let mut output = String::new();
        loop {
            let Some(ch) = self.peek_char() else {
                return Err(ExpressionError::Parse {
                    details: format!("unterminated string literal starting at byte {start}"),
                });
            };
            let escape = self.position;
            self.bump_char();
            match ch {
                c if c == quote => return Ok(output),
                '\\' => {
                    let Some(escaped) = self.peek_char() else {
                        return Err(ExpressionError::Parse {
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
                        other => {
                            return Err(ExpressionError::Parse {
                                details: format!("unknown escape `\\{other}` at byte {escape}"),
                            });
                        }
                    };
                    output.push(decoded);
                }
                other => output.push(other),
            }
        }
    }

    fn read_number(&mut self) -> Result<f64, ExpressionError> {
        let start = self.position;
        self.skip_digits();
        if self.peek_char() == Some('.') {
            self.bump_char();
            self.skip_digits();
        }
        // Without digits, the `e` starts a unit such as `2em`, which is reported as unknown.
        if let Some(exponent) = self.source[self.position..].strip_prefix(['e', 'E']) {
            let digits = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
            if digits.starts_with(|ch: char| ch.is_ascii_digit()) {
                self.position = self.source.len() - digits.len();
                self.skip_digits();
            }
        }

        let literal = &self.source[start..self.position];
        let value = literal
            .parse::<f64>()
            .map_err(|error| ExpressionError::Parse {
                details: format!("invalid number at byte {start}: {error}"),
            })?;
        if value.is_finite() {
            Ok(value)
        } else {
            Err(ExpressionError::Parse {
                details: format!("number `{literal}` at byte {start} is too large"),
            })
        }
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
        parse_hex_color(&literal[1..]).ok_or_else(|| ExpressionError::Parse {
            details: format!("invalid colour `{literal}`; use #rrggbb or #rrggbbaa"),
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
    let ast = Parser::new(source, &[])?.parse()?;
    Ok(Expression::new(ast, source.to_string()))
}

/// Parses the body of a composition function, where `params` are in scope.
pub(crate) fn parse_function_body(
    source: &str,
    params: &[String],
) -> Result<ExprNode, ExpressionError> {
    Parser::new(source, params)?.parse()
}

/// The tokens of `source`, ending with `TokenKind::Eof`.
fn tokenize(source: &str) -> Result<Vec<Token>, ExpressionError> {
    let mut lexer = Lexer::new(source);
    let mut tokens = Vec::new();
    loop {
        let token = lexer.next_token()?;
        let is_eof = token.kind == TokenKind::Eof;
        tokens.push(token);
        if is_eof {
            return Ok(tokens);
        }
    }
}

fn token_kinds(source: &str) -> Result<Vec<TokenKind>, ExpressionError> {
    let tokens = tokenize(source)?;
    Ok(tokens.into_iter().map(|token| token.kind).collect())
}

/// Splits a function signature such as `rise(start, distance)` into its name and parameters.
pub(crate) fn parse_signature(signature: &str) -> Result<(String, Vec<String>), ExpressionError> {
    let invalid = || ExpressionError::Parse {
        details: format!("invalid function signature `{signature}`; expected `name(a, b)`"),
    };
    let tokens = token_kinds(signature).map_err(|_| invalid())?;
    let [
        TokenKind::Identifier(name),
        TokenKind::LParen,
        rest @ ..,
        TokenKind::RParen,
        TokenKind::Eof,
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
    Ok(token_kinds(source)?
        .windows(2)
        .filter_map(|pair| match pair {
            [TokenKind::Identifier(name), TokenKind::LParen] => Some(name.clone()),
            _ => None,
        })
        .collect())
}

/// The largest node id an expression's number can hold exactly.
const MAX_NODE_ID_LITERAL: f64 = 9_007_199_254_740_992.0;

/// The binary operator a token stands for and how tightly it binds, where higher binds tighter.
fn binary_op(kind: &TokenKind) -> Option<(BinaryOp, u8)> {
    Some(match kind {
        TokenKind::OrOr => (BinaryOp::Or, 1),
        TokenKind::AndAnd => (BinaryOp::And, 2),
        TokenKind::EqEq => (BinaryOp::Eq, 3),
        TokenKind::Neq => (BinaryOp::Neq, 3),
        TokenKind::Gte => (BinaryOp::Gte, 4),
        TokenKind::Lte => (BinaryOp::Lte, 4),
        TokenKind::Gt => (BinaryOp::Gt, 4),
        TokenKind::Lt => (BinaryOp::Lt, 4),
        TokenKind::Plus => (BinaryOp::Add, 5),
        TokenKind::Minus => (BinaryOp::Sub, 5),
        TokenKind::Star => (BinaryOp::Mul, 6),
        TokenKind::Slash => (BinaryOp::Div, 6),
        TokenKind::Percent => (BinaryOp::Mod, 6),
        _ => return None,
    })
}

struct Parser<'a> {
    /// Always ends with `TokenKind::Eof`.
    tokens: Vec<Token>,
    position: usize,
    budget: functions::Budget,
    /// Parenthesised groups, call arguments and prefix operators the parser is inside.
    depth: usize,
    /// The parameters of the composition function being parsed.
    params: &'a [String],
}

impl<'a> Parser<'a> {
    fn new(source: &str, params: &'a [String]) -> Result<Self, ExpressionError> {
        Ok(Self {
            tokens: tokenize(source)?,
            position: 0,
            budget: functions::Budget::default(),
            depth: 0,
            params,
        })
    }

    /// Runs `parse` one nesting level deeper, so that input cannot recurse the parser without limit.
    fn nested(
        &mut self,
        parse: impl FnOnce(&mut Self) -> Result<ExprNode, ExpressionError>,
    ) -> Result<ExprNode, ExpressionError> {
        if self.depth >= functions::MAX_EXPANDED_DEPTH {
            return Err(ExpressionError::Parse {
                details: format!(
                    "expression nests deeper than {} levels",
                    functions::MAX_EXPANDED_DEPTH
                ),
            });
        }
        self.depth += 1;
        let node = parse(self);
        self.depth -= 1;
        node
    }

    fn parse(&mut self) -> Result<ExprNode, ExpressionError> {
        let ast = self.parse_binary(0)?;
        self.expect(TokenKind::Eof, "end of input")?;
        Ok(ast)
    }

    /// Parses operators binding at least as tightly as `min`, grouping equal ones to the left.
    fn parse_binary(&mut self, min: u8) -> Result<ExprNode, ExpressionError> {
        let mut node = self.parse_unary()?;
        while let Some((op, binding)) = binary_op(&self.peek().kind)
            && binding >= min
        {
            self.position += 1;
            let rhs = self.parse_binary(binding + 1)?;
            node = self.binary(node, op, rhs)?;
        }
        Ok(node)
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

    fn parse_unary(&mut self) -> Result<ExprNode, ExpressionError> {
        let op = if self.eat(TokenKind::Bang) {
            UnaryOp::Not
        } else if self.eat(TokenKind::Minus) {
            UnaryOp::Neg
        } else {
            return self.parse_primary();
        };
        let value = self.nested(Self::parse_unary)?;
        self.budget.push(ExprNode::Unary(op, Box::new(value)))
    }

    fn parse_primary(&mut self) -> Result<ExprNode, ExpressionError> {
        let Token { kind, span } = self.peek().clone();
        self.position += 1;
        match kind {
            TokenKind::Number(value) => self
                .budget
                .push(ExprNode::Literal(ExpressionValue::Number(value))),
            TokenKind::Seconds(value) => {
                let seconds = self
                    .budget
                    .push(ExprNode::Literal(ExpressionValue::Number(value)))?;
                let fps = self.budget.push(ExprNode::Global(GlobalVar::Fps))?;
                self.binary(seconds, BinaryOp::Mul, fps)
            }
            TokenKind::String(value) => self
                .budget
                .push(ExprNode::Literal(ExpressionValue::String(value))),
            TokenKind::Color(value) => self
                .budget
                .push(ExprNode::Literal(ExpressionValue::color_from_u8(value))),
            TokenKind::Identifier(identifier) => self.parse_identifier(identifier),
            TokenKind::LParen => {
                let expr = self.nested(|parser| parser.parse_binary(0))?;
                self.expect(TokenKind::RParen, "`)`")?;
                Ok(expr)
            }
            _ => Err(ExpressionError::Parse {
                details: format!("expected expression at byte {}", span.start),
            }),
        }
    }

    fn parse_identifier(&mut self, identifier: String) -> Result<ExprNode, ExpressionError> {
        if self.eat(TokenKind::Dot) {
            return Err(ExpressionError::Parse {
                details: format!(
                    "`{identifier}.` is not supported; read another node's property with node(id, 'property')"
                ),
            });
        }

        if identifier == "true" || identifier == "false" {
            return self.budget.push(ExprNode::Literal(ExpressionValue::Boolean(
                identifier == "true",
            )));
        }

        if self.eat(TokenKind::LParen) {
            let args = self.parse_call_args()?;
            return self.parse_call(&identifier, args);
        }

        if let Some(index) = self.params.iter().position(|param| *param == identifier) {
            return self.budget.push(ExprNode::Param(index));
        }
        let global = GlobalVar::from_name(&identifier).ok_or_else(|| ExpressionError::Parse {
            details: format!("unknown variable `{identifier}`"),
        })?;
        self.budget.push(ExprNode::Global(global))
    }

    fn parse_call_args(&mut self) -> Result<Vec<ExprNode>, ExpressionError> {
        let mut args = Vec::new();
        if self.eat(TokenKind::RParen) {
            return Ok(args);
        }
        loop {
            args.push(self.nested(|parser| parser.parse_binary(0))?);
            if !self.eat(TokenKind::Comma) {
                self.expect(TokenKind::RParen, "`)`")?;
                return Ok(args);
            }
        }
    }

    fn parse_call(
        &mut self,
        identifier: &str,
        args: Vec<ExprNode>,
    ) -> Result<ExprNode, ExpressionError> {
        if identifier == "if" {
            let [condition, when_true, when_false] =
                <[ExprNode; 3]>::try_from(args).map_err(|_| ExpressionError::Parse {
                    details: "if(cond, then, else) requires exactly 3 arguments".to_string(),
                })?;
            return self.budget.push(ExprNode::Conditional(
                Box::new(condition),
                Box::new(when_true),
                Box::new(when_false),
            ));
        }

        if identifier == "node" {
            return self.parse_node_reference(args);
        }

        if let Some(spec) = builtin_for_name(identifier) {
            validate_builtin_call(spec, &args)?;
            return self.budget.push(ExprNode::Builtin(spec.builtin, args));
        }
        functions::expand(identifier, &args, &mut self.budget)?.ok_or(ExpressionError::Parse {
            details: format!("unknown function `{identifier}`"),
        })
    }

    fn parse_node_reference(&mut self, args: Vec<ExprNode>) -> Result<ExprNode, ExpressionError> {
        if args.is_empty() || args.len() > 2 {
            return Err(ExpressionError::Parse {
                details: "node(id) or node(id, property_path) requires 1 or 2 arguments"
                    .to_string(),
            });
        }

        let node_id = match &args[0] {
            ExprNode::Literal(ExpressionValue::Number(value)) => {
                if *value < 0.0 || value.fract() != 0.0 {
                    return Err(ExpressionError::Parse {
                        details: "node(id, ..) expects an unsigned integer id".to_string(),
                    });
                }
                if *value > MAX_NODE_ID_LITERAL {
                    return Err(ExpressionError::Parse {
                        details: format!(
                            "node(id, ..) id {value} is above 2^53, where numbers skip integers"
                        ),
                    });
                }
                NodeId(*value as u64)
            }
            _ => {
                return Err(ExpressionError::Parse {
                    details: "node(id, ..) first argument must be a numeric node id".to_string(),
                });
            }
        };

        if args.len() == 1 {
            return self.budget.push(ExprNode::Node(node_id));
        }

        let property_path = match &args[1] {
            ExprNode::Literal(ExpressionValue::String(value)) => value.clone(),
            _ => {
                return Err(ExpressionError::Parse {
                    details: "node(.., property_path) second argument must be a string".to_string(),
                });
            }
        };

        self.budget
            .push(ExprNode::PropertyValue(node_id, property_path))
    }

    fn eat(&mut self, kind: TokenKind) -> bool {
        let matches = self.peek().kind == kind;
        if matches {
            self.position += 1;
        }
        matches
    }

    fn expect(&mut self, kind: TokenKind, description: &str) -> Result<(), ExpressionError> {
        if self.eat(kind) {
            return Ok(());
        }
        Err(ExpressionError::Parse {
            details: format!("expected {description} at byte {}", self.peek().span.start),
        })
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.position]
    }
}

/// Checks what can be known before evaluation: arity, easing names and literal spring ranges.
fn validate_builtin_call(spec: &BuiltinSpec, args: &[ExprNode]) -> Result<(), ExpressionError> {
    let error = |details: String| ExpressionError::Parse {
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
    use crate::{
        expr::{ExprNode, Expression, ExpressionValue},
        node::NodeId,
    };

    #[test]
    fn dotted_property_reads_point_to_node_references() {
        let error = Expression::parse("title.opacity * 2")
            .unwrap_err()
            .to_string();

        assert!(error.contains("node(id, 'property')"), "{error}");
    }

    fn parse_on_worker_stack(source: String) -> Result<Expression, crate::error::ExpressionError> {
        std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || Expression::parse(&source))
            .unwrap()
            .join()
            .unwrap()
    }

    #[test]
    fn nesting_is_bounded_on_every_recursive_edge() {
        let deep = 100_000;
        for source in [
            format!("{}1{}", "(".repeat(deep), ")".repeat(deep)),
            format!("{}1", "-".repeat(deep)),
            format!("{}1", "!".repeat(deep)),
            format!("{}1{}", "abs(".repeat(deep), ")".repeat(deep)),
            format!("{}1{}", "if(true, 1, ".repeat(deep), ")".repeat(deep)),
            format!("{}1{}", "-(abs(".repeat(deep), "))".repeat(deep)),
            format!(
                "{}1{}",
                "1 || 1 && 1 == 1 < 1 + 1 * (".repeat(deep),
                ")".repeat(deep)
            ),
        ] {
            let error = parse_on_worker_stack(source).expect_err("too deep");
            assert!(
                error.to_string().contains("nests deeper than 64 levels"),
                "{error}"
            );
        }
    }

    #[test]
    fn nesting_at_the_limit_parses_on_a_worker_stack() {
        let source = format!("{}1{}", "-(abs(".repeat(21), "))".repeat(21));
        parse_on_worker_stack(source).expect("63 levels");
        let source = format!("{}1{}", "(".repeat(64), ")".repeat(64));
        parse_on_worker_stack(source).expect("64 levels");
    }

    #[test]
    fn wide_calls_are_not_nested() {
        let args = vec!["1"; 3_000].join(", ");
        Expression::parse(&format!("max({args})")).expect("wide call");
    }

    fn number(source: &str) -> f64 {
        match Expression::parse(source).map(|expression| expression.ast().clone()) {
            Ok(ExprNode::Literal(ExpressionValue::Number(value))) => value,
            other => panic!("{source}: {other:?}"),
        }
    }

    fn parse_error(source: &str) -> String {
        Expression::parse(source).unwrap_err().to_string()
    }

    #[test]
    fn numbers_accept_exponents_and_leading_dots() {
        for (source, expected) in [
            ("1e3", 1000.0),
            ("1E3", 1000.0),
            ("1.5e-3", 0.0015),
            ("2e+2", 200.0),
            (".5", 0.5),
            (".5e1", 5.0),
            ("5.", 5.0),
            ("5.e1", 50.0),
            ("1e21", 1e21),
            ("1e-7", 1e-7),
        ] {
            assert_eq!(number(source), expected, "{source}");
        }
        for (source, expected) in [
            ("2em", "unknown unit `em`"),
            ("2e", "unknown unit `e`"),
            ("2e+", "unknown unit `e`"),
            ("1e999", "`1e999` at byte 0 is too large"),
            (".", "expected expression at byte 0"),
            ("1.5.5", "expected end of input at byte 3"),
        ] {
            let error = parse_error(source);
            assert!(error.contains(expected), "{source}: {error}");
        }
        assert!(Expression::parse("1e3s").is_ok());
    }

    #[test]
    fn operators_bind_and_group_as_the_table_says() {
        let bound = |source: &str| match Expression::parse(source) {
            Ok(expression) => expression.ast().clone(),
            Err(error) => panic!("{source}: {error}"),
        };
        for (source, grouped) in [
            ("1 + 2 * 3", "1 + (2 * 3)"),
            ("1 * 2 + 3", "(1 * 2) + 3"),
            ("1 - 2 - 3", "(1 - 2) - 3"),
            ("8 / 4 % 3", "(8 / 4) % 3"),
            ("1 + 2 < 3 + 4", "(1 + 2) < (3 + 4)"),
            ("1 < 2 == 3 > 4", "(1 < 2) == (3 > 4)"),
            ("1 == 2 && 3 != 4", "(1 == 2) && (3 != 4)"),
            ("1 && 2 || 3 && 4", "(1 && 2) || (3 && 4)"),
            ("1 || 2 || 3", "(1 || 2) || 3"),
            ("-1 + !2", "(-1) + (!2)"),
        ] {
            assert_eq!(bound(source), bound(grouped), "{source}");
        }
    }

    #[test]
    fn unknown_names_and_escapes_fail_at_parse() {
        for (source, expected) in [
            ("frmae + 1", "unknown variable `frmae`"),
            ("if(false, frmae, 1)", "unknown variable `frmae`"),
            ("'a\\qb'", "unknown escape `\\q` at byte 2"),
            ("node(9007199254740994)", "above 2^53"),
        ] {
            let error = parse_error(source);
            assert!(error.contains(expected), "{source}: {error}");
        }
        assert_eq!(
            Expression::parse("node(9007199254740992)").unwrap().ast(),
            &ExprNode::Node(NodeId(1 << 53))
        );
    }
}
