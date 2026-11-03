//! Composition-level `functions`, expanded into each call site while parsing.

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
};

use crate::{
    error::ExpressionError,
    expr::{
        ExprNode,
        parser::{called_names, parse_function_body, parse_signature},
        spec::reserved_names,
    },
};

/// Largest AST, in nodes, that expanding a function may produce.
const MAX_EXPANDED_NODES: usize = 10_000;
/// Deepest AST nesting that expanding a function may produce.
pub(crate) const MAX_EXPANDED_DEPTH: usize = 64;

struct Function {
    arity: usize,
    body: ExprNode,
}

/// The nodes a single top-level expression parse may build, counting call expansions.
#[derive(Default)]
pub(crate) struct Budget {
    nodes: usize,
}

impl Budget {
    /// Accounts for `nodes` new nodes in a tree that is now `depth` levels deep.
    fn spend(&mut self, nodes: usize, depth: usize) -> Result<(), ExpressionError> {
        self.nodes = self.nodes.saturating_add(nodes);
        if self.nodes > MAX_EXPANDED_NODES {
            Err(parse_error(format!(
                "expression expands to more than {MAX_EXPANDED_NODES} nodes"
            )))
        } else if depth > MAX_EXPANDED_DEPTH {
            Err(parse_error(format!(
                "expression expands deeper than {MAX_EXPANDED_DEPTH} levels"
            )))
        } else {
            Ok(())
        }
    }

    /// Accounts for a node that was just built from already-counted children.
    pub(crate) fn push(&mut self, node: ExprNode) -> Result<ExprNode, ExpressionError> {
        self.spend(1, node.depth())?;
        Ok(node)
    }
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
            let body = parse_function_body(body, params)
                .map_err(|error| function_error(&name, body, error_details(error)))?;
            let function = Function {
                arity: params.len(),
                body,
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
pub(crate) fn expand(
    name: &str,
    args: &[ExprNode],
    budget: &mut Budget,
) -> Result<Option<ExprNode>, ExpressionError> {
    FUNCTIONS.with_borrow(|functions| {
        let Some(function) = functions.get(name) else {
            return Ok(None);
        };
        if args.len() != function.arity {
            return Err(parse_error(format!(
                "`{name}` expects {} arguments, got {}",
                function.arity,
                args.len()
            )));
        }
        // The expansion holds the only copies of the arguments, which were spent on when they
        // were parsed. A call that discards them still doesn't give any budget back.
        let spent = budget.nodes;
        let arg_nodes: usize = args.iter().map(ExprNode::count).sum();
        budget.nodes = spent.saturating_sub(arg_nodes);
        let expanded = substitute(&function.body, args, 1, budget)?;
        budget.nodes = budget.nodes.max(spent);
        Ok(Some(expanded))
    })
}

fn check_names(
    name: &str,
    params: &[String],
    signatures: &BTreeMap<String, (Vec<String>, &str)>,
) -> Result<(), String> {
    let reserved = |identifier: &str| reserved_names().any(|name| name == identifier);
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

/// Orders functions so each comes after those it calls, rejecting cycles and call chains
/// deeper than `MAX_EXPANDED_DEPTH`.
fn dependency_order(
    signatures: &BTreeMap<String, (Vec<String>, &str)>,
) -> Result<Vec<String>, ExpressionError> {
    /// Returns the length of the longest call chain starting at `name`.
    fn visit_function(
        name: &str,
        signatures: &BTreeMap<String, (Vec<String>, &str)>,
        stack: &mut Vec<String>,
        order: &mut Vec<String>,
        heights: &mut HashMap<String, usize>,
    ) -> Result<usize, ExpressionError> {
        if let Some(start) = stack.iter().position(|open| open == name) {
            let mut cycle = stack[start..].to_vec();
            cycle.push(name.to_string());
            return Err(parse_error(format!(
                "functions call each other in a cycle: {}",
                cycle.join(" -> ")
            )));
        }
        let finished = heights.get(name).copied();
        if stack.len() + finished.unwrap_or(1) > MAX_EXPANDED_DEPTH {
            return Err(parse_error(format!(
                "functions call each other more than {MAX_EXPANDED_DEPTH} levels deep: {} -> {name}",
                stack.join(" -> ")
            )));
        }
        if let Some(height) = finished {
            return Ok(height);
        }
        let body = signatures[name].1;
        stack.push(name.to_string());
        let callees =
            called_names(body).map_err(|error| function_error(name, body, error_details(error)))?;
        let mut height = 1;
        for callee in callees {
            if signatures.contains_key(&callee) {
                height =
                    height.max(1 + visit_function(&callee, signatures, stack, order, heights)?);
            }
        }
        stack.pop();
        heights.insert(name.to_string(), height);
        order.push(name.to_string());
        Ok(height)
    }

    let mut order = Vec::new();
    let mut heights = HashMap::new();
    for name in signatures.keys() {
        visit_function(name, signatures, &mut Vec::new(), &mut order, &mut heights)?;
    }
    Ok(order)
}

/// A copy of `node` with each parameter replaced by its argument, placed at `level` of the tree
/// being built. An argument's own parameters belong to the function it was written in.
fn substitute(
    node: &ExprNode,
    args: &[ExprNode],
    level: usize,
    budget: &mut Budget,
) -> Result<ExprNode, ExpressionError> {
    if let ExprNode::Param(index) = node
        && let Some(arg) = args.get(*index)
    {
        return substitute(arg, &[], level, budget);
    }
    budget.spend(1, level)?;
    node.map_children(|child| substitute(child, args, level + 1, budget))
}

pub(crate) fn visit(node: &ExprNode, f: &mut impl FnMut(&ExprNode)) {
    f(node);
    for child in node.children() {
        visit(child, f);
    }
}

fn parse_error(details: String) -> ExpressionError {
    ExpressionError::Parse { details }
}

fn function_error(name: &str, body: &str, details: String) -> ExpressionError {
    parse_error(format!("functions.{name} (`{body}`): {details}"))
}

fn error_details(error: ExpressionError) -> String {
    match error {
        ExpressionError::Parse { details, .. } | ExpressionError::Evaluate { details, .. } => {
            details
        }
        ExpressionError::Located { failure, .. } => error_details(*failure),
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
            frame: f64::from(frame),
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
            error.contains("expression expands to more than 10000 nodes"),
            "{error}"
        );
    }

    #[test]
    fn rejects_call_sites_that_nest_too_deep() {
        let _scope = FunctionScope::enter(&definitions(&[("next(x)", "x + 1 + 1 + 1")])).unwrap();
        let nested = |depth: usize| (0..depth).fold("0".to_string(), |s, _| format!("next({s})"));
        assert!(Expression::parse(&nested(20)).is_ok());
        let error = Expression::parse(&nested(30)).unwrap_err().to_string();
        assert!(
            error.contains("expression expands deeper than 64 levels"),
            "{error}"
        );
    }

    fn big_calls() -> FunctionScope {
        FunctionScope::enter(&definitions(&[
            ("f(x)", "x + x + x + x + x + x + x + x"),
            ("h(x)", "f(f(f(f(x))))"),
        ]))
        .unwrap()
    }

    #[test]
    fn sibling_calls_share_one_node_budget() {
        let _scope = big_calls();
        assert!(Expression::parse("h(1)").is_ok());
        let siblings = vec!["h(1)"; 64].join(", ");
        let error = Expression::parse(&format!("min({siblings})"))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("expression expands to more than 10000 nodes"),
            "{error}"
        );
    }

    #[test]
    fn wrapped_calls_share_one_depth_budget() {
        let _scope = big_calls();
        let wrapped = |count: usize| format!("{}h(1){}", "abs(".repeat(count), ")".repeat(count));
        assert!(Expression::parse(&wrapped(30)).is_ok());
        let error = Expression::parse(&wrapped(40)).unwrap_err().to_string();
        assert!(
            error.contains("expression expands deeper than 64 levels"),
            "{error}"
        );
    }

    #[test]
    fn calls_must_match_arity() {
        let _scope = FunctionScope::enter(&definitions(&[("f(a, b)", "a + b")])).unwrap();
        let error = Expression::parse("f(1)").unwrap_err().to_string();
        assert!(error.contains("`f` expects 2 arguments, got 1"), "{error}");
    }

    #[test]
    fn call_chain_limit_does_not_depend_on_name_order() {
        let scope_error_for = |functions: Vec<(String, String)>| {
            let entries: Vec<(&str, &str)> = functions
                .iter()
                .map(|(signature, body)| (signature.as_str(), body.as_str()))
                .collect();
            scope_error(&entries)
        };
        let callee_sorts_after_caller: Vec<_> = (0..200)
            .map(|index| (format!("f{index:03}(x)"), format!("f{:03}(x)", index + 1)))
            .chain([("f200(x)".to_string(), "x".to_string())])
            .collect();
        let callee_sorts_before_caller: Vec<_> = (1..=200)
            .map(|index| (format!("f{index:03}(x)"), format!("f{:03}(x)", index - 1)))
            .chain([("f000(x)".to_string(), "x".to_string())])
            .collect();
        for functions in [callee_sorts_after_caller, callee_sorts_before_caller] {
            assert!(scope_error_for(functions).contains("more than 64 levels deep"));
        }

        let chain: Vec<(String, String)> = (1..64)
            .map(|index| (format!("f{index:03}(x)"), format!("f{:03}(x)", index - 1)))
            .chain([("f000(x)".to_string(), "x".to_string())])
            .collect();
        let entries: Vec<(&str, &str)> = chain
            .iter()
            .map(|(signature, body)| (signature.as_str(), body.as_str()))
            .collect();
        assert!(FunctionScope::enter(&definitions(&entries)).is_ok());
    }
}
