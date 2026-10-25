//! Composition-level `functions`, expanded into each call site while parsing.

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
};

use crate::{
    error::ExpressionError,
    expr::{
        ExprNode, ExpressionReference, GlobalVar,
        parser::{called_names, parse_expression, parse_signature},
        spec::{GLOBALS, builtin_for_name},
    },
};

/// Largest AST, in nodes, that expanding a function may produce.
const MAX_EXPANDED_NODES: usize = 10_000;
/// Deepest AST nesting that expanding a function may produce.
const MAX_EXPANDED_DEPTH: usize = 64;

struct Function {
    params: Vec<String>,
    body: ExprNode,
    shape: Shape,
    references: Vec<ExpressionReference>,
}

/// The size of a tree, with each use of a parameter left as a hole to be filled by an argument.
struct Shape {
    nodes: usize,
    depth: usize,
    param_uses: Vec<usize>,
    param_depth: Vec<usize>,
}

impl Shape {
    fn of(node: &ExprNode, params: &[String]) -> Self {
        let mut shape = Self {
            nodes: 0,
            depth: 0,
            param_uses: vec![0; params.len()],
            param_depth: vec![0; params.len()],
        };
        shape.measure(node, params, 1);
        shape
    }

    fn measure(&mut self, node: &ExprNode, params: &[String], depth: usize) {
        if let ExprNode::Global(GlobalVar::Custom(name)) = node
            && let Some(index) = params.iter().position(|param| param == name)
        {
            self.param_uses[index] += 1;
            self.param_depth[index] = self.param_depth[index].max(depth);
            return;
        }
        self.nodes = self.nodes.saturating_add(1);
        self.depth = self.depth.max(depth);
        match node {
            ExprNode::Binary(lhs, _, rhs) => {
                self.measure(lhs, params, depth + 1);
                self.measure(rhs, params, depth + 1);
            }
            ExprNode::Unary(_, value) => self.measure(value, params, depth + 1),
            ExprNode::Builtin(_, args) => {
                for arg in args {
                    self.measure(arg, params, depth + 1);
                }
            }
            ExprNode::Conditional(condition, when_true, when_false) => {
                self.measure(condition, params, depth + 1);
                self.measure(when_true, params, depth + 1);
                self.measure(when_false, params, depth + 1);
            }
            ExprNode::Literal(_)
            | ExprNode::Global(_)
            | ExprNode::SymbolicPath(_)
            | ExprNode::Node(_)
            | ExprNode::PropertyValue(..)
            | ExprNode::VirtualProperty(_) => {}
        }
    }

    /// The `(nodes, depth)` of the tree once each parameter is replaced by a tree of the given
    /// `(nodes, depth)`, computed without building it.
    fn expanded(&self, args: &[(usize, usize)]) -> (usize, usize) {
        let mut nodes = self.nodes;
        let mut depth = self.depth;
        for (index, &(arg_nodes, arg_depth)) in args.iter().enumerate() {
            let uses = self.param_uses[index];
            if uses > 0 {
                nodes = nodes.saturating_add(uses.saturating_mul(arg_nodes));
                depth = depth.max(self.param_depth[index] - 1 + arg_depth);
            }
        }
        (nodes, depth)
    }
}

fn limit_error(subject: &str, (nodes, depth): (usize, usize)) -> Option<ExpressionError> {
    if nodes > MAX_EXPANDED_NODES {
        Some(parse_error(format!(
            "{subject} expands to more than {MAX_EXPANDED_NODES} nodes"
        )))
    } else if depth > MAX_EXPANDED_DEPTH {
        Some(parse_error(format!(
            "{subject} expands to more than {MAX_EXPANDED_DEPTH} levels of nesting"
        )))
    } else {
        None
    }
}

pub(crate) struct Expansion {
    pub node: ExprNode,
    pub references: Vec<ExpressionReference>,
}

thread_local! {
    static FUNCTIONS: RefCell<HashMap<String, Function>> = RefCell::new(HashMap::new());
}

/// Makes composition functions callable from expressions parsed while the scope is alive.
pub struct FunctionScope {
    previous: HashMap<String, Function>,
}

impl FunctionScope {
    /// Parses `definitions`, which map signatures such as `"rise(start)"` to bodies.
    pub fn enter(definitions: &BTreeMap<String, String>) -> Result<Self, ExpressionError> {
        let scope = Self {
            previous: FUNCTIONS.take(),
        };
        let mut signatures = BTreeMap::new();
        for (signature, body) in definitions {
            let (name, params) = parse_signature(signature)?;
            let body = body.strip_prefix('=').unwrap_or(body);
            if signatures.insert(name.clone(), (params, body)).is_some() {
                return Err(parse_error(format!("function `{name}` is defined twice")));
            }
        }
        for (name, (params, body)) in &signatures {
            check_names(name, params, &signatures)
                .map_err(|details| function_error(name, body, details))?;
        }

        for name in dependency_order(&signatures)? {
            let (params, body) = &signatures[&name];
            let expression = parse_expression(body)
                .map_err(|error| function_error(&name, body, error_details(error)))?;
            let mut unknown = None;
            visit(&expression.ast, &mut |node| {
                if let ExprNode::Global(GlobalVar::Custom(variable)) = node
                    && !params.contains(variable)
                {
                    unknown.get_or_insert_with(|| variable.clone());
                }
            });
            if let Some(variable) = unknown {
                let details = format!("unknown variable `{variable}`");
                return Err(function_error(&name, body, details));
            }
            let shape = Shape::of(&expression.ast, params);
            let ones = vec![(1, 1); params.len()];
            if let Some(error) = limit_error(&format!("`functions.{name}`"), shape.expanded(&ones))
            {
                return Err(error);
            }
            let function = Function {
                params: params.clone(),
                body: expression.ast,
                shape,
                references: expression.references,
            };
            FUNCTIONS.with_borrow_mut(|functions| functions.insert(name, function));
        }
        Ok(scope)
    }
}

impl Drop for FunctionScope {
    fn drop(&mut self) {
        FUNCTIONS.set(std::mem::take(&mut self.previous));
    }
}

/// Expands a call to a composition function, or returns `None` if `name` isn't one.
pub(crate) fn expand(name: &str, args: &[ExprNode]) -> Result<Option<Expansion>, ExpressionError> {
    FUNCTIONS.with_borrow(|functions| {
        let Some(function) = functions.get(name) else {
            return Ok(None);
        };
        if args.len() != function.params.len() {
            return Err(parse_error(format!(
                "`{name}` expects {} arguments, got {}",
                function.params.len(),
                args.len()
            )));
        }
        let arg_sizes: Vec<_> = args
            .iter()
            .map(|arg| {
                let shape = Shape::of(arg, &[]);
                (shape.nodes, shape.depth)
            })
            .collect();
        if let Some(error) = limit_error(
            &format!("call to `{name}`"),
            function.shape.expanded(&arg_sizes),
        ) {
            return Err(error);
        }
        Ok(Some(Expansion {
            node: substitute(&function.body, &function.params, args),
            references: function.references.clone(),
        }))
    })
}

fn check_names(
    name: &str,
    params: &[String],
    signatures: &BTreeMap<String, (Vec<String>, &str)>,
) -> Result<(), String> {
    let reserved = |identifier: &str| {
        matches!(identifier, "if" | "node" | "true" | "false")
            || builtin_for_name(identifier).is_some()
            || GLOBALS.iter().any(|global| global.name == identifier)
    };
    if reserved(name) {
        return Err(format!("`{name}` is a reserved name"));
    }
    for (index, param) in params.iter().enumerate() {
        if reserved(param) || signatures.contains_key(param) {
            return Err(format!(
                "parameter `{param}` collides with a reserved or function name"
            ));
        }
        if params[..index].contains(param) {
            return Err(format!("parameter `{param}` is declared twice"));
        }
    }
    Ok(())
}

/// Orders functions so each comes after those it calls, rejecting cycles.
fn dependency_order(
    signatures: &BTreeMap<String, (Vec<String>, &str)>,
) -> Result<Vec<String>, ExpressionError> {
    fn visit_function(
        name: &str,
        signatures: &BTreeMap<String, (Vec<String>, &str)>,
        stack: &mut Vec<String>,
        order: &mut Vec<String>,
    ) -> Result<(), ExpressionError> {
        if order.iter().any(|done| done == name) {
            return Ok(());
        }
        if let Some(start) = stack.iter().position(|open| open == name) {
            let mut cycle = stack[start..].to_vec();
            cycle.push(name.to_string());
            return Err(parse_error(format!(
                "functions call each other in a cycle: {}",
                cycle.join(" -> ")
            )));
        }
        let body = signatures[name].1;
        stack.push(name.to_string());
        let callees =
            called_names(body).map_err(|error| function_error(name, body, error_details(error)))?;
        for callee in callees {
            if signatures.contains_key(&callee) {
                visit_function(&callee, signatures, stack, order)?;
            }
        }
        stack.pop();
        order.push(name.to_string());
        Ok(())
    }

    let mut order = Vec::new();
    for name in signatures.keys() {
        visit_function(name, signatures, &mut Vec::new(), &mut order)?;
    }
    Ok(order)
}

fn substitute(node: &ExprNode, params: &[String], args: &[ExprNode]) -> ExprNode {
    let recurse = |node: &ExprNode| Box::new(substitute(node, params, args));
    match node {
        ExprNode::Global(GlobalVar::Custom(name)) => params
            .iter()
            .position(|param| param == name)
            .map_or_else(|| node.clone(), |index| args[index].clone()),
        ExprNode::Binary(lhs, op, rhs) => ExprNode::Binary(recurse(lhs), *op, recurse(rhs)),
        ExprNode::Unary(op, value) => ExprNode::Unary(*op, recurse(value)),
        ExprNode::Builtin(builtin, builtin_args) => ExprNode::Builtin(
            *builtin,
            builtin_args
                .iter()
                .map(|arg| substitute(arg, params, args))
                .collect(),
        ),
        ExprNode::Conditional(condition, when_true, when_false) => {
            ExprNode::Conditional(recurse(condition), recurse(when_true), recurse(when_false))
        }
        ExprNode::Literal(_)
        | ExprNode::Global(_)
        | ExprNode::SymbolicPath(_)
        | ExprNode::Node(_)
        | ExprNode::PropertyValue(..)
        | ExprNode::VirtualProperty(_) => node.clone(),
    }
}

fn visit(node: &ExprNode, f: &mut impl FnMut(&ExprNode)) {
    f(node);
    match node {
        ExprNode::Binary(lhs, _, rhs) => {
            visit(lhs, f);
            visit(rhs, f);
        }
        ExprNode::Unary(_, value) => visit(value, f),
        ExprNode::Builtin(_, args) => args.iter().for_each(|arg| visit(arg, f)),
        ExprNode::Conditional(condition, when_true, when_false) => {
            visit(condition, f);
            visit(when_true, f);
            visit(when_false, f);
        }
        ExprNode::Literal(_)
        | ExprNode::Global(_)
        | ExprNode::SymbolicPath(_)
        | ExprNode::Node(_)
        | ExprNode::PropertyValue(..)
        | ExprNode::VirtualProperty(_) => {}
    }
}

fn parse_error(details: String) -> ExpressionError {
    ExpressionError::Parse {
        path: None,
        details,
    }
}

fn function_error(name: &str, body: &str, details: String) -> ExpressionError {
    parse_error(format!("functions.{name} (`{body}`): {details}"))
}

fn error_details(error: ExpressionError) -> String {
    match error {
        ExpressionError::Parse { details, .. } | ExpressionError::Evaluate { details, .. } => {
            details
        }
        ExpressionError::UndefinedVariable { name, .. } => format!("undefined variable `{name}`"),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::FunctionScope;
    use crate::expr::{Expression, ExpressionContext, ExpressionValue};

    fn definitions(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(signature, body)| (signature.to_string(), body.to_string()))
            .collect()
    }

    fn scope_error(entries: &[(&str, &str)]) -> String {
        match FunctionScope::enter(&definitions(entries)) {
            Ok(_) => panic!("{entries:?} should be rejected"),
            Err(error) => error.to_string(),
        }
    }

    fn at_frame(expression: &Expression, frame: u32) -> ExpressionValue {
        let ctx = ExpressionContext {
            frame,
            fps: 30.0,
            ..Default::default()
        };
        expression.evaluate(&ctx).unwrap()
    }

    #[test]
    fn expands_issue_function_and_nested_calls() {
        let _scope = FunctionScope::enter(&definitions(&[
            (
                "rise(start)",
                "tween(vec2(0, 60), vec2(0, 0), start, 16, 'out_expo')",
            ),
            (
                "settle( start , scale )",
                "=rise(start) * scale + vec2(1, 1)",
            ),
        ]))
        .unwrap();

        let source = "vec2(100, 200) + rise(10)";
        let expression = Expression::parse(source).unwrap();
        assert_eq!(expression.source, source);
        assert_eq!(
            at_frame(&expression, 0),
            ExpressionValue::Vec2((100.0, 260.0))
        );
        assert_eq!(
            at_frame(&expression, 26),
            ExpressionValue::Vec2((100.0, 200.0))
        );

        let nested = Expression::parse("settle(4 + 1, 2)").unwrap();
        assert_eq!(at_frame(&nested, 0), ExpressionValue::Vec2((1.0, 121.0)));
        assert_eq!(at_frame(&nested, 30), ExpressionValue::Vec2((1.0, 1.0)));
    }

    #[test]
    fn functions_only_exist_inside_their_scope() {
        {
            let _scope = FunctionScope::enter(&definitions(&[("double(x)", "x * 2")])).unwrap();
            assert!(Expression::parse("double(2)").is_ok());
        }
        let error = Expression::parse("double(2)").unwrap_err().to_string();
        assert!(error.contains("unknown function `double`"), "{error}");
    }

    #[test]
    fn rejects_invalid_functions() {
        for (entries, expected) in [
            (
                &[("a()", "b() + 1"), ("b()", "c()"), ("c()", "a()")][..],
                "cycle: a -> b -> c -> a",
            ),
            (&[("loop(x)", "loop(x - 1)")], "cycle: loop -> loop"),
            (&[("tween(x)", "x")], "`tween` is a reserved name"),
            (&[("f(frame)", "frame")], "parameter `frame` collides"),
            (&[("f(g)", "g"), ("g()", "1")], "parameter `g` collides"),
            (&[("f(x, x)", "x")], "parameter `x` is declared twice"),
            (
                &[("f(x)", "x + y")],
                "functions.f (`x + y`): unknown variable `y`",
            ),
            (
                &[("f(e)", "tween(0, 1, 0, 10, e)")],
                "must be a quoted easing name",
            ),
            (&[("f(x", "x")], "invalid function signature `f(x`"),
            (&[("f(x,)", "x")], "invalid function signature"),
        ] {
            let error = scope_error(entries);
            assert!(error.contains(expected), "{entries:?}: {error}");
        }
    }

    #[test]
    fn rejects_functions_that_expand_exponentially() {
        let mut entries = vec![("f0(x)".to_string(), "x + x".to_string())];
        for n in 1..=5 {
            entries.push((format!("f{n}(x)"), format!("f{}(f{}(x))", n - 1, n - 1)));
        }
        let definitions: BTreeMap<_, _> = entries.into_iter().collect();
        let error = FunctionScope::enter(&definitions)
            .err()
            .unwrap()
            .to_string();
        assert!(
            error.contains("functions.f4") && error.contains("expands to more than 10000 nodes"),
            "{error}"
        );
    }

    #[test]
    fn rejects_call_sites_that_expand_too_far() {
        let _scope =
            FunctionScope::enter(&definitions(&[("f(x)", "x + x + x + x + x + x + x + x")]))
                .unwrap();
        let mut source = "1".to_string();
        for _ in 0..5 {
            source = format!("f({source})");
        }
        let error = Expression::parse(&source).unwrap_err().to_string();
        assert!(
            error.contains("call to `f` expands to more than 10000 nodes"),
            "{error}"
        );
    }

    #[test]
    fn rejects_call_sites_that_nest_too_deep() {
        let _scope = FunctionScope::enter(&definitions(&[("next(x)", "x + 1")])).unwrap();
        let nested = |depth: usize| (0..depth).fold("0".to_string(), |s, _| format!("next({s})"));
        assert!(Expression::parse(&nested(60)).is_ok());
        let error = Expression::parse(&nested(70)).unwrap_err().to_string();
        assert!(
            error.contains("call to `next` expands to more than 64 levels"),
            "{error}"
        );
    }

    #[test]
    fn calls_must_match_arity() {
        let _scope = FunctionScope::enter(&definitions(&[("f(a, b)", "a + b")])).unwrap();
        let error = Expression::parse("f(1)").unwrap_err().to_string();
        assert!(error.contains("`f` expects 2 arguments, got 1"), "{error}");
    }
}
