use std::{
    cell::Cell,
    hash::{Hash, Hasher},
};

use crate::{
    error::LumenError,
    expr::{
        ExpressionContext, GlyphScope,
        ast::{BinaryOp, BuiltinFn, ExprNode, Expression, ExpressionValue, GlobalVar, UnaryOp},
        builtins::{evaluate_builtin, measure_node},
    },
    node::{NodeId, PropertyEval, PropertyRead},
};

impl Expression {
    pub fn evaluate(&self, ctx: &ExpressionContext<'_>) -> crate::Result<ExpressionValue> {
        let _evaluation = EvaluationGuard::enter(self.depth)?;
        self.plan.evaluate(ctx)
    }
}

/// Most levels of evaluation that may be open at once, counting an expression's depth for each
/// evaluation that is open. Reading another node's property or measuring its text evaluates its
/// expression inside the reader's, so this bounds the stack that nodes reading through each other
/// use. It is sized for the largest frames, on a 1 MiB stack.
const MAX_EVALUATION_LEVELS: u32 = 96;
/// Levels charged to every evaluation on top of its expression's depth, for the read or
/// measurement that starts it.
const EVALUATION_OVERHEAD: u32 = 2;
/// Most evaluations that one outermost evaluation may start, including all it reads through.
const MAX_EVALUATIONS: u32 = 10_000;

thread_local! {
    /// The levels of the open evaluations and the evaluations started since the outermost one began.
    static EVALUATIONS: Cell<(u32, u32)> = const { Cell::new((0, 0)) };
}

struct EvaluationGuard {
    levels: u32,
}

impl EvaluationGuard {
    fn enter(depth: usize) -> crate::Result<Self> {
        let levels = u32::try_from(depth)
            .unwrap_or(u32::MAX)
            .saturating_add(EVALUATION_OVERHEAD);
        let (open, started) = EVALUATIONS.get();
        let exhausted = if open.saturating_add(levels) > MAX_EVALUATION_LEVELS {
            Some(format!(
                "node reads nest deeper than {MAX_EVALUATION_LEVELS} levels of evaluation; do they read each other?"
            ))
        } else if started >= MAX_EVALUATIONS {
            Some(format!(
                "node reads started more than {MAX_EVALUATIONS} evaluations; do they fan out through each other?"
            ))
        } else {
            None
        };
        if let Some(details) = exhausted {
            return Err(LumenError::evaluate(details));
        }
        EVALUATIONS.set((open + levels, started + 1));
        Ok(Self { levels })
    }
}

impl Drop for EvaluationGuard {
    fn drop(&mut self) {
        let (open, started) = EVALUATIONS.get();
        let open = open - self.levels;
        EVALUATIONS.set((open, if open == 0 { 0 } else { started }));
    }
}

/// An expression's AST with each distinct subtree as one step, so a subtree it repeats, as
/// expanded easing curves repeat their progress, is evaluated once per evaluation. Steps are
/// evaluated in the AST's order and only where it evaluates them, so `&&`, `||` and conditional
/// branches stay lazy.
#[derive(Debug)]
pub(crate) struct EvalPlan {
    steps: Vec<Step>,
    root: usize,
    memo_slots: usize,
}

#[derive(Debug)]
struct Step {
    op: Op,
    /// Where an evaluation keeps the value of a step that more than one step reads.
    memo: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Op {
    Literal(Literal),
    Global(GlobalVar),
    Unary(UnaryOp, usize),
    Binary(usize, BinaryOp, usize),
    Builtin(BuiltinFn, Box<[usize]>),
    Conditional(usize, usize, usize),
    /// `node(id, 'property')`.
    ReadProperty(NodeId, String),
    /// `node(id)` outside a measurement, which fails when evaluated.
    NodeRef(NodeId),
    /// `text_width(node(id))` or `text_height(node(id))`.
    MeasureNode(BuiltinFn, NodeId),
}

/// A literal compared by its bits, so that `0` and `-0` stay distinct.
#[derive(Debug, Clone)]
struct Literal(ExpressionValue);

impl Literal {
    fn key(&self) -> (u8, [u64; 4], &str) {
        match &self.0 {
            ExpressionValue::Number(number) => (0, [number.to_bits(), 0, 0, 0], ""),
            ExpressionValue::Boolean(boolean) => (1, [u64::from(*boolean), 0, 0, 0], ""),
            ExpressionValue::String(text) => (2, [0; 4], text),
            ExpressionValue::Vec2((x, y)) => (3, [x.to_bits(), y.to_bits(), 0, 0], ""),
            ExpressionValue::Color(color) => (4, color.map(f64::to_bits), ""),
        }
    }
}

impl PartialEq for Literal {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl Eq for Literal {}

impl Hash for Literal {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key().hash(state);
    }
}

impl EvalPlan {
    pub(crate) fn new(ast: &ExprNode) -> Self {
        let mut builder = PlanBuilder::default();
        let root = builder.add(ast);
        let mut reads = vec![0_u32; builder.ops.len()];
        for op in &builder.ops {
            match op {
                Op::Unary(_, value) => reads[*value] += 1,
                Op::Binary(lhs, _, rhs) => {
                    reads[*lhs] += 1;
                    reads[*rhs] += 1;
                }
                Op::Builtin(_, args) => args.iter().for_each(|arg| reads[*arg] += 1),
                Op::Conditional(condition, when_true, when_false) => {
                    for step in [condition, when_true, when_false] {
                        reads[*step] += 1;
                    }
                }
                Op::Literal(_)
                | Op::Global(_)
                | Op::ReadProperty(..)
                | Op::NodeRef(_)
                | Op::MeasureNode(..) => {}
            }
        }
        let mut memo_slots = 0;
        let steps = builder
            .ops
            .into_iter()
            .zip(reads)
            .map(|(op, reads)| {
                let memo =
                    (reads > 1 && !matches!(op, Op::Literal(_) | Op::Global(_))).then(|| {
                        memo_slots += 1;
                        memo_slots - 1
                    });
                Step { op, memo }
            })
            .collect();
        Self {
            steps,
            root,
            memo_slots,
        }
    }

    fn evaluate(&self, ctx: &ExpressionContext<'_>) -> crate::Result<ExpressionValue> {
        let mut memo = vec![None; self.memo_slots];
        self.evaluate_step(self.root, ctx, &mut memo)
    }

    fn evaluate_step(
        &self,
        index: usize,
        ctx: &ExpressionContext<'_>,
        memo: &mut [Option<ExpressionValue>],
    ) -> crate::Result<ExpressionValue> {
        let step = &self.steps[index];
        if let Some(slot) = step.memo
            && let Some(value) = &memo[slot]
        {
            return Ok(value.clone());
        }
        let value = match &step.op {
            Op::Literal(Literal(value)) => value.clone(),
            Op::Global(global) => global_value(*global, ctx)?,
            Op::Unary(op, value) => unary(*op, self.evaluate_step(*value, ctx, memo)?)?,
            Op::Binary(lhs, op, rhs) => {
                let lhs = self.evaluate_step(*lhs, ctx, memo)?;
                binary(lhs, *op, || self.evaluate_step(*rhs, ctx, memo))?
            }
            Op::Builtin(builtin, args) => {
                let args = args
                    .iter()
                    .map(|arg| self.evaluate_step(*arg, ctx, memo))
                    .collect::<crate::Result<Vec<_>>>()?;
                evaluate_builtin(*builtin, &args, ctx)?
            }
            Op::Conditional(condition, when_true, when_false) => {
                let branch = if self.evaluate_step(*condition, ctx, memo)?.as_bool() {
                    when_true
                } else {
                    when_false
                };
                self.evaluate_step(*branch, ctx, memo)?
            }
            Op::ReadProperty(node_id, property) => read_property(*node_id, property, ctx)?,
            Op::NodeRef(node_id) => {
                return Err(LumenError::evaluate(format!(
                    "node reference `{}` can only be used in builtins that accept node references",
                    node_id.0
                )));
            }
            Op::MeasureNode(builtin, node_id) => measure_node(*builtin, *node_id, ctx)?,
        };
        if let Some(slot) = step.memo {
            memo[slot] = Some(value.clone());
        }
        Ok(value)
    }
}

#[derive(Default)]
struct PlanBuilder {
    ops: Vec<Op>,
    interned: foldhash::HashMap<Op, usize>,
}

impl PlanBuilder {
    /// Adds the steps of `node`, returning the step that evaluates it.
    fn add(&mut self, node: &ExprNode) -> usize {
        let op = match node {
            ExprNode::Literal(value) => Op::Literal(Literal(value.clone())),
            ExprNode::Global(global) => Op::Global(*global),
            ExprNode::Unary(op, value) => Op::Unary(*op, self.add(value)),
            ExprNode::Binary(lhs, op, rhs) => {
                let lhs = self.add(lhs);
                Op::Binary(lhs, *op, self.add(rhs))
            }
            ExprNode::Builtin(builtin, args) => match (builtin, args.as_slice()) {
                (BuiltinFn::TextHeight | BuiltinFn::TextWidth, [ExprNode::Node(node_id)]) => {
                    Op::MeasureNode(*builtin, *node_id)
                }
                _ => Op::Builtin(*builtin, args.iter().map(|arg| self.add(arg)).collect()),
            },
            ExprNode::Conditional(condition, when_true, when_false) => {
                let condition = self.add(condition);
                let when_true = self.add(when_true);
                Op::Conditional(condition, when_true, self.add(when_false))
            }
            ExprNode::Node(node_id) => Op::NodeRef(*node_id),
            ExprNode::PropertyValue(node_id, property) => {
                Op::ReadProperty(*node_id, property.clone())
            }
            ExprNode::Param(_) => unreachable!("function bodies are not evaluated"),
        };
        // Each read stays its own step, so `EvaluationGuard` counts every evaluation it starts.
        if matches!(op, Op::ReadProperty(..) | Op::MeasureNode(..)) {
            self.ops.push(op);
            return self.ops.len() - 1;
        }
        if let Some(&step) = self.interned.get(&op) {
            return step;
        }
        self.ops.push(op.clone());
        self.interned.insert(op, self.ops.len() - 1);
        self.ops.len() - 1
    }
}

/// The value of `property` on `node_id`, evaluating its expression if it has one.
fn read_property(
    node_id: NodeId,
    property: &str,
    ctx: &ExpressionContext<'_>,
) -> crate::Result<ExpressionValue> {
    let graph = ctx.graph.ok_or_else(|| {
        LumenError::evaluate(format!(
            "no graph available to resolve node property reference `{property}`"
        ))
    })?;
    let node = graph.nodes.get(&node_id).ok_or_else(|| {
        LumenError::evaluate(format!(
            "node `{}` not found for property reference `{property}`",
            node_id.0
        ))
    })?;
    let prop = node.get_property(property).ok_or_else(|| {
        LumenError::evaluate(format!(
            "property `{property}` not found on node `{}`",
            node_id.0
        ))
    })?;
    match prop {
        PropertyRead::Expr(inner_expr) => inner_expr
            .evaluate(&ctx.without_glyph())
            .map_err(|error| error.located(node_id, property)),
        PropertyRead::Value(value) => value.ok_or_else(|| {
            LumenError::evaluate(format!(
                "property `{property}` on node {} can't be read in an expression",
                node_id.0
            ))
        }),
    }
}

fn unary(op: UnaryOp, value: ExpressionValue) -> crate::Result<ExpressionValue> {
    match op {
        UnaryOp::Neg => match value {
            ExpressionValue::Vec2((x, y)) => Ok(ExpressionValue::Vec2((-x, -y))),
            other => Ok(ExpressionValue::Number(-to_number(&other)?)),
        },
        UnaryOp::Not => Ok(ExpressionValue::Boolean(!value.as_bool())),
    }
}

/// `lhs op rhs`, evaluating `rhs` unless `&&` or `||` is decided by `lhs`.
fn binary(
    lhs: ExpressionValue,
    op: BinaryOp,
    rhs: impl FnOnce() -> crate::Result<ExpressionValue>,
) -> crate::Result<ExpressionValue> {
    match op {
        BinaryOp::And => {
            if !lhs.as_bool() {
                return Ok(ExpressionValue::Boolean(false));
            }
            Ok(ExpressionValue::Boolean(rhs()?.as_bool()))
        }
        BinaryOp::Or => {
            if lhs.as_bool() {
                return Ok(ExpressionValue::Boolean(true));
            }
            Ok(ExpressionValue::Boolean(rhs()?.as_bool()))
        }
        BinaryOp::Eq => Ok(ExpressionValue::Boolean(lhs == rhs()?)),
        BinaryOp::Neq => Ok(ExpressionValue::Boolean(lhs != rhs()?)),
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => {
            arithmetic(op, &lhs, &rhs()?)
        }
        BinaryOp::Gt | BinaryOp::Lt | BinaryOp::Gte | BinaryOp::Lte => {
            let rhs = rhs()?;
            let (lhs, rhs) = (to_number(&lhs)?, to_number(&rhs)?);
            Ok(ExpressionValue::Boolean(match op {
                BinaryOp::Gt => lhs > rhs,
                BinaryOp::Lt => lhs < rhs,
                BinaryOp::Gte => lhs >= rhs,
                _ => lhs <= rhs,
            }))
        }
    }
}

fn global_value(global: GlobalVar, ctx: &ExpressionContext<'_>) -> crate::Result<ExpressionValue> {
    let in_glyph = |read: fn(&GlyphScope) -> u32| {
        ctx.glyph.as_ref().map(read).map(f64::from).ok_or_else(|| {
            LumenError::evaluate(format!(
                "`{}` is only available in a text node's glyph_offset, glyph_scale, \
                 glyph_rotation and glyph_opacity params",
                global.name()
            ))
        })
    };
    Ok(ExpressionValue::Number(match global {
        GlobalVar::Frame => ctx.frame,
        GlobalVar::Time => ctx.time_seconds(),
        GlobalVar::Fps => f64::from(ctx.fps),
        GlobalVar::Width => f64::from(ctx.width),
        GlobalVar::Height => f64::from(ctx.height),
        GlobalVar::Glyph => in_glyph(|scope| scope.item.glyph)?,
        GlobalVar::Word => in_glyph(|scope| scope.item.word)?,
        GlobalVar::Line => in_glyph(|scope| scope.item.line)?,
        GlobalVar::Span => in_glyph(|scope| scope.item.span)?,
        GlobalVar::GlyphCount => in_glyph(|scope| scope.counts.glyph)?,
        GlobalVar::WordCount => in_glyph(|scope| scope.counts.word)?,
        GlobalVar::LineCount => in_glyph(|scope| scope.counts.line)?,
        GlobalVar::SpanCount => in_glyph(|scope| scope.counts.span)?,
    }))
}

/// Numbers combine as usual. Two vec2 values combine componentwise, and a vec2 can be
/// multiplied or divided by a number on either side. Colours have no arithmetic.
fn arithmetic(
    op: BinaryOp,
    lhs: &ExpressionValue,
    rhs: &ExpressionValue,
) -> crate::Result<ExpressionValue> {
    let apply = |a: f64, b: f64| match op {
        BinaryOp::Add => Ok(a + b),
        BinaryOp::Sub => Ok(a - b),
        BinaryOp::Mul => Ok(a * b),
        BinaryOp::Div if b == 0.0 => Err(LumenError::evaluate("division by zero")),
        BinaryOp::Div => Ok(a / b),
        BinaryOp::Mod if b == 0.0 => Err(LumenError::evaluate("modulo by zero")),
        _ => Ok(a % b),
    };
    let scales = matches!(op, BinaryOp::Mul | BinaryOp::Div);
    let (a, b) = match (lhs, rhs) {
        (ExpressionValue::Vec2(a), ExpressionValue::Vec2(b)) if op != BinaryOp::Mod => (*a, *b),
        (ExpressionValue::Vec2(a), other) if scales => {
            let scalar = to_number(other)?;
            (*a, (scalar, scalar))
        }
        (other, ExpressionValue::Vec2(b)) if scales => {
            let scalar = to_number(other)?;
            ((scalar, scalar), *b)
        }
        (ExpressionValue::Vec2(_) | ExpressionValue::Color(_), _)
        | (_, ExpressionValue::Vec2(_) | ExpressionValue::Color(_)) => {
            return Err(LumenError::evaluate(format!(
                "cannot apply `{}` to {} and {}",
                operator_symbol(op),
                lhs.type_name(),
                rhs.type_name()
            )));
        }
        _ => {
            let value = apply(to_number(lhs)?, to_number(rhs)?)?;
            return Ok(ExpressionValue::Number(value));
        }
    };
    Ok(ExpressionValue::Vec2((apply(a.0, b.0)?, apply(a.1, b.1)?)))
}

fn operator_symbol(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "%",
        BinaryOp::Gt => ">",
        BinaryOp::Lt => "<",
        BinaryOp::Gte => ">=",
        BinaryOp::Lte => "<=",
        BinaryOp::Eq => "==",
        BinaryOp::Neq => "!=",
        BinaryOp::And => "&&",
        BinaryOp::Or => "||",
    }
}

fn to_number(value: &ExpressionValue) -> crate::Result<f64> {
    value.as_f64().ok_or_else(|| {
        LumenError::evaluate(format!(
            "cannot convert {} `{}` into a number",
            value.type_name(),
            value.as_string()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        error::ExpressionError,
        graph::Graph,
        node::{DelegateEvaluable, NodeId, NodeKind, ParamValue, source::text::Text},
    };

    fn test_context() -> ExpressionContext<'static> {
        ExpressionContext {
            frame: 48.0,
            fps: 24.0,
            width: 1920,
            height: 1080,
            graph: None,
            media: None,
            glyph: None,
            props: None,
        }
    }

    #[test]
    fn evaluates_globals_from_expression_context() {
        for (source, expected) in [
            ("time", 2.0),
            ("fps", 24.0),
            ("width", 1920.0),
            ("height", 1080.0),
        ] {
            assert_eq!(
                evaluate(source, 48).unwrap(),
                ExpressionValue::Number(expected),
                "{source}"
            );
        }
    }

    #[test]
    fn glyph_globals_read_the_glyph_scope_and_fail_outside_it() {
        let source = "glyph + word * 10 + line * 100 + glyph_count * 1000 + word_count * 10000 \
                      + line_count * 100000 + span * 1000000 + span_count * 10000000";
        let expression = Expression::parse(source).unwrap();
        let mut ctx = test_context();
        let error = expression.evaluate(&ctx).unwrap_err().to_string();
        assert!(error.contains("`glyph` is only available"), "{error}");
        let item = |glyph, word, line, span| lumen_text::TextItem {
            glyph,
            word,
            line,
            span,
        };
        ctx.glyph = Some(crate::expr::GlyphScope {
            item: item(1, 2, 3, 7),
            counts: item(4, 5, 6, 8),
        });
        assert_eq!(
            expression.evaluate(&ctx).unwrap(),
            ExpressionValue::Number(87_654_321.0)
        );
    }

    #[test]
    fn evaluates_fractional_frame_and_time() {
        let ctx = ExpressionContext {
            frame: 1.5,
            ..test_context()
        };

        let expression = Expression::parse("vec2(frame * 2, time * 24)").unwrap();
        assert_eq!(
            expression.evaluate(&ctx).unwrap(),
            ExpressionValue::Vec2((3.0, 1.5))
        );
    }

    #[test]
    fn subframe_offsets_frame_and_time() {
        let ctx = test_context().at_subframe(-0.25);

        let value = |source: &str| Expression::parse(source).unwrap().evaluate(&ctx).unwrap();
        assert_eq!(value("frame"), ExpressionValue::Number(47.75));
        assert_eq!(value("time"), ExpressionValue::Number(47.75 / 24.0));
    }

    #[test]
    fn converts_supported_node_properties() {
        assert_eq!(7_i64.to_expression(), Some(ExpressionValue::Number(7.0)));
        assert_eq!(true.to_expression(), Some(ExpressionValue::Boolean(true)));
        assert_eq!(
            (10.0, 20.0).to_expression(),
            Some(ExpressionValue::Vec2((10.0, 20.0)))
        );
        assert_eq!(
            [255_u8, 0, 0, 255].to_expression(),
            Some(ExpressionValue::Color([1.0, 0.0, 0.0, 1.0]))
        );
    }

    #[test]
    fn rejects_non_scalar_node_properties() {
        let graph = chained_text_nodes(1, "", "");
        let error = evaluate_in(&graph, "node(1, 'color')")
            .unwrap_err()
            .to_string();

        assert!(
            error.contains("property `color` on node 1 can't be read in an expression"),
            "{error}"
        );
    }

    #[test]
    fn evaluates_builtins_and_components() {
        for (source, expected) in [
            ("lerp(10, 20, 0.25)", ExpressionValue::Number(12.5)),
            ("step(10, 20, 0.5)", ExpressionValue::Number(10.0)),
            ("step(10, 20, 1)", ExpressionValue::Number(20.0)),
            (
                "vec2(frame * 2, time + 3)",
                ExpressionValue::Vec2((96.0, 5.0)),
            ),
        ] {
            assert_eq!(evaluate(source, 48).unwrap(), expected, "{source}");
        }
    }

    fn number_at(source: &str, frame: u32) -> f64 {
        let ctx = ExpressionContext {
            frame: f64::from(frame),
            fps: 30.0,
            ..test_context()
        };
        match Expression::parse(source).unwrap().evaluate(&ctx).unwrap() {
            ExpressionValue::Number(value) => value,
            other => panic!("expected a number, got {other:?}"),
        }
    }

    #[test]
    fn evaluates_issue_easing_examples() {
        let lerp = "lerp(1200, 540, out_expo(progress(0, 18)))";
        let tween = "tween(1200, 540, 0, 18, 'out_expo')";
        assert_eq!(number_at(lerp, 0), 1200.0);
        assert_eq!(number_at(lerp, 18), 540.0);
        assert_eq!(number_at(lerp, 40), 540.0);
        assert_eq!(number_at(lerp, 9), 1200.0 - 660.0 * (1.0 - 2f64.powi(-5)));
        for frame in [0, 3, 9, 17, 18, 30] {
            assert_eq!(number_at(lerp, frame), number_at(tween, frame));
        }
        assert_eq!(number_at("tween(0, 10, 4, 10)", 9), 5.0);
    }

    #[test]
    fn unit_suffixes_convert_seconds_to_frames() {
        let source = "tween(0, 1, 1.5s, 300ms, 'out_expo')";
        assert_eq!(number_at(source, 45), 0.0);
        assert_eq!(number_at(source, 54), 1.0);
        assert_eq!(number_at(source, 48), 1.0 - 2f64.powf(-10.0 / 3.0));
        assert_eq!(number_at("2s + 500ms", 0), 75.0);

        let ExpressionError::Parse { details, .. } = Expression::parse("1.5px").unwrap_err() else {
            panic!("expected parse error");
        };
        assert!(details.contains("unknown unit `px`"), "{details}");
    }

    #[test]
    fn progress_steps_when_the_range_is_empty() {
        assert_eq!(number_at("progress(10, 10)", 9), 0.0);
        assert_eq!(number_at("progress(10, 10)", 10), 1.0);
        assert_eq!(number_at("progress(10, 5)", 12), 1.0);
        assert_eq!(number_at("progress(10, 20)", 15), 0.5);
    }

    #[test]
    fn spring_and_cubic_bezier_evaluate() {
        assert_eq!(number_at("spring((frame - 12) / fps)", 6), 0.0);
        assert!((number_at("spring((frame - 12) / fps, 170, 26)", 60) - 1.0).abs() < 1e-3);
        assert!((number_at("cubic_bezier(0, 0, 1, 1, 0.25)", 0) - 0.25).abs() < 1e-6);
        let error = Expression::parse("spring(1, 0 - 1)")
            .unwrap()
            .evaluate(&test_context())
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("stiffness must be greater than 0")
        );
    }

    #[test]
    fn rejects_bad_builtin_arguments_at_parse_time() {
        for (source, expected) in [
            ("vec2()", "`vec2`: expects 2 arguments"),
            ("vec2(1)", "`vec2`: expects 2 arguments"),
            ("vec2(1, 2, 3)", "`vec2`: expects 2 arguments"),
            ("tween(0, 1, 0, 10, 'bounce')", "unknown easing `bounce`"),
            ("tween(0, 1, 0, 10, frame)", "must be a quoted easing name"),
            ("tween(0, 1, 0)", "expects 4 to 5 arguments, got 3"),
            ("spring(1, 0)", "stiffness must be greater than 0"),
            ("spring(1, 170, -0.5 * 2)", ""),
        ] {
            match Expression::parse(source) {
                Err(ExpressionError::Parse { details, .. }) if !expected.is_empty() => {
                    assert!(details.contains(expected), "{source}: {details}")
                }
                Ok(_) if expected.is_empty() => {}
                other => panic!("{source}: {other:?}"),
            }
        }
    }

    #[test]
    fn vec2_coerces_boolean_components() {
        assert_eq!(
            evaluate("vec2(true, false)", 0).unwrap(),
            ExpressionValue::Vec2((1.0, 0.0))
        );
    }

    #[test]
    fn vec2_rejects_invalid_component_types() {
        for source in [
            "vec2(\"left\", 2)",
            "vec2(\"12.5\", 2)",
            "vec2(1, vec2(2, 3))",
        ] {
            let error = Expression::parse(source)
                .unwrap()
                .evaluate(&test_context())
                .expect_err("nonnumeric component must fail");

            assert_eq!(error.to_string(), "vec2 expects numeric arguments");
        }
    }

    #[test]
    fn numeric_strings_are_not_numbers() {
        for (source, expected) in [
            ("'1' + 2", "cannot convert string `1` into a number"),
            ("'1' > 0", "cannot convert string `1` into a number"),
            ("-'1'", "cannot convert string `1` into a number"),
            ("abs('1')", "abs expects numeric arguments"),
            (
                "text_width('a', '32')",
                "text_width expects numeric arguments",
            ),
            (
                "lerp('0', '1', 0.5)",
                "expects `from` and `to` of the same type",
            ),
        ] {
            let error = evaluate(source, 0).unwrap_err().to_string();
            assert!(error.contains(expected), "{source}: {error}");
        }
        assert_eq!(
            evaluate("uppercase(1) == '1'", 0).unwrap(),
            ExpressionValue::Boolean(true)
        );
    }

    #[test]
    fn only_exact_zero_is_zero() {
        let tiny = 10_f64.powf(-20.0);
        for (source, expected) in [
            ("1 / pow(10, -20)", ExpressionValue::Number(1.0 / tiny)),
            ("1 % pow(10, -20)", ExpressionValue::Number(1.0 % tiny)),
            ("mod(1, pow(10, -20))", ExpressionValue::Number(1.0 % tiny)),
            ("if(pow(10, -20), 1, 2)", ExpressionValue::Number(1.0)),
            ("!pow(10, -20)", ExpressionValue::Boolean(false)),
            ("if(pow(-1, 0.5), 1, 2)", ExpressionValue::Number(2.0)),
            ("if(-0, 1, 2)", ExpressionValue::Number(2.0)),
            (
                "1 / sin(3.141592653589793) > 1",
                ExpressionValue::Boolean(true),
            ),
        ] {
            assert_eq!(evaluate(source, 0).unwrap(), expected, "{source}");
        }
        for (source, expected) in [
            ("1 / 0", "division by zero"),
            ("1 / -0", "division by zero"),
            ("1 % 0", "modulo by zero"),
            ("mod(1, 0)", "mod divisor must be non-zero"),
        ] {
            let error = evaluate(source, 0).unwrap_err().to_string();
            assert!(error.contains(expected), "{source}: {error}");
        }
    }

    fn evaluate(source: &str, frame: u32) -> crate::Result<ExpressionValue> {
        let ctx = ExpressionContext {
            frame: f64::from(frame),
            ..test_context()
        };
        Expression::parse(source).unwrap().evaluate(&ctx)
    }

    #[test]
    fn repeated_subtrees_stay_lazy_and_keep_their_errors() {
        let number = ExpressionValue::Number;
        let boolean = ExpressionValue::Boolean;
        for (source, frame, expected) in [
            (
                "min(1, max(0, (time - 0.5) / 0.5)) * min(1, max(0, (time - 0.5) / 0.5))",
                18,
                Ok(number(0.25)),
            ),
            (
                "if(frame > 48, 10 / (frame - 48), (frame - 48) * 2)",
                0,
                Ok(number(-96.0)),
            ),
            (
                "if(frame > 48, 10 / (frame - 48), (frame - 48) * 2)",
                48,
                Ok(number(0.0)),
            ),
            (
                "if(frame > 48, 10 / (frame - 48), (frame - 48) * 2)",
                58,
                Ok(number(1.0)),
            ),
            (
                "frame == 48 || 10 / (frame - 48) > 1",
                48,
                Ok(boolean(true)),
            ),
            (
                "frame == 48 || 10 / (frame - 48) > 1",
                58,
                Ok(boolean(false)),
            ),
            ("10 / (frame - 48) + 10 / (frame - 48)", 58, Ok(number(2.0))),
            (
                "10 / (frame - 48) + 10 / (frame - 48)",
                48,
                Err("division by zero"),
            ),
        ] {
            let result = evaluate(source, frame).map_err(|error| error.to_string());
            assert_eq!(
                result,
                expected.map_err(str::to_string),
                "{source} at {frame}"
            );
        }
    }

    #[test]
    fn subtrees_that_differ_only_in_the_sign_of_zero_stay_apart() {
        let ExpressionValue::Vec2((negative, positive)) =
            evaluate("vec2(-(time * 0), time * 0)", 48).unwrap()
        else {
            panic!("expected vec2");
        };
        assert_eq!(negative.to_bits(), (-0.0_f64).to_bits());
        assert_eq!(positive.to_bits(), 0.0_f64.to_bits());
    }

    #[test]
    fn vec2_arithmetic_is_componentwise() {
        for (source, expected) in [
            ("vec2(1, 2) + vec2(10, 20)", (11.0, 22.0)),
            ("vec2(1, 2) - vec2(10, 20)", (-9.0, -18.0)),
            ("-vec2(1, -2)", (-1.0, 2.0)),
            ("vec2(1, 2) * 3", (3.0, 6.0)),
            ("3 * vec2(1, 2)", (3.0, 6.0)),
            ("vec2(4, 8) / 2", (2.0, 4.0)),
            ("8 / vec2(4, 2)", (2.0, 4.0)),
            ("vec2(2, 3) * vec2(4, 5)", (8.0, 15.0)),
            ("vec2(8, 15) / vec2(4, 5)", (2.0, 3.0)),
            ("lerp(vec2(0, 10), vec2(10, 30), 0.25)", (2.5, 15.0)),
        ] {
            assert_eq!(
                evaluate(source, 0).unwrap(),
                ExpressionValue::Vec2(expected),
                "{source}"
            );
        }
    }

    #[test]
    fn vec2_rejects_mismatched_arithmetic() {
        for (source, expected) in [
            ("vec2(1, 2) + 3", "cannot apply `+` to vec2 and number"),
            ("3 - vec2(1, 2)", "cannot apply `-` to number and vec2"),
            ("vec2(1, 2) % 2", "cannot apply `%` to vec2 and number"),
            (
                "vec2(1, 2) % vec2(1, 2)",
                "cannot apply `%` to vec2 and vec2",
            ),
            ("vec2(1, 2) / 0", "division by zero"),
            (
                "vec2(1, 2) > 1",
                "cannot convert vec2 `[1, 2]` into a number",
            ),
            (
                "lerp(vec2(1, 2), 3, 0.5)",
                "`lerp` expects `from` and `to` of the same type",
            ),
        ] {
            let details = evaluate(source, 0).unwrap_err().to_string();
            assert!(details.starts_with(expected), "{source}: {details}");
        }
    }

    #[test]
    fn evaluates_issue_vec2_tween_sum() {
        let source = "vec2(960, 1200) \
            + tween(vec2(0, 0), vec2(0, -660), 0, 18, 'out_expo') \
            + tween(vec2(0, 0), vec2(-1360, 0), 90, 14, 'in_back')";
        for (frame, expected) in [
            (0, (960.0, 1200.0)),
            (18, (960.0, 540.0)),
            (90, (960.0, 540.0)),
            (104, (-400.0, 540.0)),
        ] {
            assert_eq!(
                evaluate(source, frame).unwrap(),
                ExpressionValue::Vec2(expected),
                "frame {frame}"
            );
        }
        let ExpressionValue::Vec2((x, _)) = evaluate(source, 93).unwrap() else {
            panic!("expected vec2");
        };
        assert!(x > 960.0, "in_back pulls back before leaving: {x}");
    }

    #[test]
    fn colour_literals_constructors_and_tweens() {
        for (source, expected) in [
            ("#ff8000", [1.0, 128.0 / 255.0, 0.0, 1.0]),
            ("#FF800080", [1.0, 128.0 / 255.0, 0.0, 128.0 / 255.0]),
            ("rgb(255, 0, 51)", [1.0, 0.0, 0.2, 1.0]),
            ("rgba(0, 0, 0, 0)", [0.0; 4]),
            ("lerp(#000000, #ffffff, 0.5)", [0.5, 0.5, 0.5, 1.0]),
            ("tween(#ff0000, #0000ff, 40, 16)", [0.5, 0.0, 0.5, 1.0]),
            ("if(frame > 10, #00ff00, #ff0000)", [0.0, 1.0, 0.0, 1.0]),
        ] {
            assert_eq!(
                evaluate(source, 48).unwrap(),
                ExpressionValue::Color(expected),
                "{source}"
            );
        }
    }

    #[test]
    fn colour_errors() {
        for (source, expected) in [
            ("#ff00", "invalid colour `#ff00`"),
            ("#gg0000", "invalid colour `#gg0000`"),
            (
                "rgb(256, 0, 0)",
                "colour components must be between 0 and 255",
            ),
            ("rgb(1, 2)", "expects 3 arguments"),
        ] {
            let ExpressionError::Parse { details, .. } = Expression::parse(source).unwrap_err()
            else {
                panic!("{source}: expected parse error");
            };
            assert!(details.contains(expected), "{source}: {details}");
        }
        for (source, expected) in [
            ("#ff0000 + #00ff00", "cannot apply `+` to color and color"),
            ("#ff0000 * 2", "cannot apply `*` to color and number"),
            ("-#ff0000", "cannot convert color `#ff0000ff` into a number"),
            (
                "rgb(frame * 10, 0, 0)",
                "colour components must be between 0 and 255",
            ),
            (
                "lerp(#ff0000, vec2(0, 0), 0.5)",
                "expects `from` and `to` of the same type",
            ),
        ] {
            let error = evaluate(source, 48).unwrap_err().to_string();
            assert!(error.contains(expected), "{source}: {error}");
        }
    }

    #[test]
    fn text_measure_builtins_follow_the_nodes_text_properties() {
        let text_id = NodeId::new(8);
        let mut graph = Graph::new();
        graph.nodes.insert(
            text_id,
            NodeKind::Text(Text {
                id: text_id,
                params: crate::node::source::text::TextParamsDelegate {
                    layout: crate::node::source::text::TextLayoutParamsDelegate {
                        content: crate::node::source::text_content::TextContentDelegate::Text(
                            crate::node::Deferred::Expr(
                                Expression::parse("\"Morning update\"").unwrap(),
                            ),
                        ),
                        font_family: crate::node::Deferred::Expr(
                            Expression::parse("\"Roboto\"").unwrap(),
                        ),
                        font_size: crate::node::Deferred::Expr(
                            Expression::parse("24 + frame").unwrap(),
                        ),
                        max_width: crate::node::Deferred::Expr(Expression::parse("120").unwrap()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            }),
        );
        let measure = |function: &str, frame: f64, with_graph: bool| {
            let ctx = ExpressionContext {
                frame,
                graph: with_graph.then_some(&graph),
                ..test_context()
            };
            let source = if with_graph {
                format!("{function}(node(8))")
            } else {
                let size = 24.0 + frame;
                format!("{function}(\"Morning update\", {size}, 120, \"Roboto\")")
            };
            match Expression::parse(&source).unwrap().evaluate(&ctx) {
                Ok(ExpressionValue::Number(value)) => value,
                other => panic!("{source}: {other:?}"),
            }
        };
        for function in ["text_width", "text_height"] {
            let (first, later) = (measure(function, 0.0, true), measure(function, 8.0, true));
            assert_ne!(first, later, "{function} ignores the animated font size");
            assert_eq!(first, measure(function, 0.0, false), "{function} at 0");
            assert_eq!(later, measure(function, 8.0, false), "{function} at 8");
        }

        for source in [
            "text_width(node(8), 32, 300, \"Roboto\")",
            "text_height(node(8), 32, 300, \"Roboto\")",
        ] {
            let error = Expression::parse(source).unwrap_err().to_string();
            assert!(error.contains("does not accept override"), "{error}");
        }
    }

    #[test]
    fn text_measure_builtins_apply_font_features() {
        use crate::node::source::text_content::FontFeatures;

        let text_id = NodeId::new(8);
        let width = |features: &[(&[u8; 4], u32)]| {
            let features = FontFeatures(
                features
                    .iter()
                    .map(|(tag, value)| (**tag, *value))
                    .collect(),
            );
            let mut graph = Graph::new();
            graph.nodes.insert(
                text_id,
                NodeKind::Text(Text {
                    id: text_id,
                    params: crate::node::source::text::TextParamsDelegate {
                        layout: crate::node::source::text::TextLayoutParamsDelegate {
                            content: crate::node::source::text_content::TextContentDelegate::Text(
                                crate::node::Deferred::value("1111".to_string()),
                            ),
                            font_size: crate::node::Deferred::value(32.0),
                            font_features: crate::node::Delegated::into_delegate(features),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                }),
            );
            let ctx = ExpressionContext {
                graph: Some(&graph),
                ..test_context()
            };
            match Expression::parse("text_width(node(8))")
                .unwrap()
                .evaluate(&ctx)
            {
                Ok(ExpressionValue::Number(width)) => width,
                other => panic!("expected a width, got {other:?}"),
            }
        };
        assert!(width(&[(b"pnum", 1)]) < width(&[]));
    }

    #[test]
    fn a_measured_string_takes_the_defaults_of_a_text_node() {
        let mut graph = Graph::new();
        graph.nodes.insert(
            NodeId::new(8),
            NodeKind::Text(Text {
                id: NodeId::new(8),
                params: crate::node::source::text::TextParamsDelegate {
                    layout: crate::node::source::text::TextLayoutParamsDelegate {
                        content: crate::node::source::text_content::TextContentDelegate::Text(
                            crate::node::Deferred::value("Morning update".to_string()),
                        ),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            }),
        );
        let ctx = ExpressionContext {
            graph: Some(&graph),
            ..test_context()
        };
        let measure = |source: &str| Expression::parse(source).unwrap().evaluate(&ctx).unwrap();

        assert_eq!(
            measure("text_width('Morning update')"),
            measure("text_width(node(8))")
        );
        assert_eq!(
            measure("text_height('Morning update')"),
            measure("text_height(node(8))")
        );
        assert_eq!(
            crate::node::source::text::TextLayoutParams::default().font_family,
            lumen_text::DEFAULT_FONT_FAMILY
        );
    }

    #[test]
    fn span_colors_may_read_the_measurement_of_their_text() {
        let text_id = NodeId::new(8);
        let content = crate::node::source::text_content::TextContentDelegate::from_json_value(
            &serde_json::json!([{
                "text": "Morning",
                "color": "=if(text_width(node(8)) > 10, #ff0000, #0000ff)",
                "underline": { "paint": "=if(text_height(node(8)) > 10, #ff0000, #0000ff)" }
            }]),
            "content",
        )
        .unwrap();
        let mut graph = Graph::new();
        graph.nodes.insert(
            text_id,
            NodeKind::Text(Text {
                id: text_id,
                params: crate::node::source::text::TextParamsDelegate {
                    layout: crate::node::source::text::TextLayoutParamsDelegate {
                        content,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            }),
        );
        let ctx = ExpressionContext {
            graph: Some(&graph),
            ..test_context()
        };

        let width = Expression::parse("text_width(node(8))")
            .unwrap()
            .evaluate(&ctx);

        assert!(matches!(width, Ok(ExpressionValue::Number(value)) if value > 10.0));
    }

    #[test]
    fn span_baseline_shifts_may_read_the_measurement_of_their_text() {
        let text_id = NodeId::new(8);
        let content = crate::node::source::text_content::TextContentDelegate::from_json_value(
            &serde_json::json!([{
                "text": "Morning",
                "baseline_shift": "=text_height(node(8)) / 2"
            }]),
            "content",
        )
        .unwrap();
        let mut graph = Graph::new();
        graph.nodes.insert(
            text_id,
            NodeKind::Text(Text {
                id: text_id,
                params: crate::node::source::text::TextParamsDelegate {
                    layout: crate::node::source::text::TextLayoutParamsDelegate {
                        content,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            }),
        );
        let ctx = ExpressionContext {
            graph: Some(&graph),
            ..test_context()
        };

        let height = Expression::parse("text_height(node(8))")
            .unwrap()
            .evaluate(&ctx);

        assert!(matches!(height, Ok(ExpressionValue::Number(value)) if value > 0.0));
    }

    #[test]
    fn recursive_text_measurement_returns_error_instead_of_panicking() {
        let text_id = NodeId::new(8);
        let mut graph = Graph::new();
        graph.nodes.insert(
            text_id,
            NodeKind::Text(Text {
                id: text_id,
                params: crate::node::source::text::TextParamsDelegate {
                    layout: crate::node::source::text::TextLayoutParamsDelegate {
                        content: crate::node::source::text_content::TextContentDelegate::Text(
                            crate::node::Deferred::Expr(
                                Expression::parse("text_width(node(8))").unwrap(),
                            ),
                        ),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            }),
        );
        let ctx = ExpressionContext {
            graph: Some(&graph),
            ..test_context()
        };

        let error = Expression::parse("text_width(node(8))")
            .unwrap()
            .evaluate(&ctx)
            .expect_err("recursive text measurement must fail");

        assert!(error.to_string().contains("do they read each other?"));
    }

    /// Text nodes `1..=count` with the given `font_size` and `content` expressions, where
    /// `{next}` stands for the id of the node after it. The last node has plain values.
    fn chained_text_nodes(count: u64, font_size: &str, content: &str) -> Graph {
        let mut graph = Graph::new();
        for id in 1..=count {
            let mut params = crate::node::source::text::TextParamsDelegate::default();
            if id < count {
                let next = (id + 1).to_string();
                params.layout.font_size = crate::node::Deferred::Expr(
                    Expression::parse(&font_size.replace("{next}", &next)).unwrap(),
                );
                params.layout.content =
                    crate::node::source::text_content::TextContentDelegate::Text(
                        crate::node::Deferred::Expr(
                            Expression::parse(&content.replace("{next}", &next)).unwrap(),
                        ),
                    );
            }
            graph.nodes.insert(
                NodeId::new(id),
                NodeKind::Text(Text {
                    id: NodeId::new(id),
                    params,
                }),
            );
        }
        graph
    }

    fn evaluate_in(graph: &Graph, source: &str) -> crate::Result<ExpressionValue> {
        let ctx = ExpressionContext {
            graph: Some(graph),
            ..test_context()
        };
        Expression::parse(source).unwrap().evaluate(&ctx)
    }

    #[test]
    fn a_node_reading_itself_fails_instead_of_overflowing_the_stack() {
        let graph = chained_text_nodes(2, "node(1, 'font_size')", "'a'");
        let error = evaluate_in(&graph, "node(1, 'font_size')").expect_err("self read");

        assert!(error.to_string().contains("levels of evaluation"));
    }

    #[test]
    fn nodes_reading_each_other_fail_and_leave_later_reads_working() {
        let mut graph = chained_text_nodes(3, "1", "'a'");
        for (id, other) in [(1, 2), (2, 1)] {
            let NodeKind::Text(text) = graph.nodes.get_mut(&NodeId::new(id)).unwrap() else {
                unreachable!()
            };
            text.params.layout.font_size = crate::node::Deferred::Expr(
                Expression::parse(&format!("node({other}, 'font_size')")).unwrap(),
            );
        }

        let error = evaluate_in(&graph, "node(1, 'font_size')").expect_err("loop");
        assert!(error.to_string().contains("levels of evaluation"));

        let after = evaluate_in(&graph, "node(3, 'font_size')").unwrap();
        assert!(matches!(after, ExpressionValue::Number(_)));
    }

    #[test]
    fn a_shallow_fan_out_that_multiplies_work_is_cut_off() {
        let read = "node({next}, 'font_size')";
        let graph = chained_text_nodes(8, &[read; 8].join(" + "), "'a'");
        let error = evaluate_in(&graph, "node(1, 'font_size')").expect_err("fan-out");

        assert!(error.to_string().contains("more than 10000 evaluations"));
    }

    #[test]
    fn fan_out_through_properties_and_measurements_shares_one_budget() {
        let font_size = ["node({next}, 'font_size')"; 5].join(" + ");
        let content = ["text_width(node({next}))"; 5].join(" + ");
        let graph = chained_text_nodes(8, &font_size, &content);
        let error = evaluate_in(&graph, "text_width(node(1)) + node(1, 'font_size')")
            .expect_err("mixed fan-out");

        assert!(error.to_string().contains("more than 10000 evaluations"));
        let after = evaluate_in(&graph, "node(8, 'font_size')").unwrap();
        assert!(matches!(after, ExpressionValue::Number(_)));
    }

    #[test]
    fn wide_shallow_reads_stay_within_budget() {
        let read = "node({next}, 'font_size')";
        let graph = chained_text_nodes(8, &format!("{read} + {read}"), "'a'");

        let value = evaluate_in(&graph, "node(1, 'font_size')").unwrap();

        assert!(matches!(value, ExpressionValue::Number(_)));
    }

    fn on_worker_stack<T: Send + 'static>(run: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(run)
            .unwrap()
            .join()
            .unwrap()
    }

    #[test]
    fn a_self_read_in_a_deep_expression_fails_on_a_worker_stack() {
        let read = format!(
            "{}node(1, 'font_size'){}",
            "abs(".repeat(63),
            ")".repeat(63)
        );
        let graph = chained_text_nodes(2, &read, "'a'");

        let error =
            on_worker_stack(move || evaluate_in(&graph, "node(1, 'font_size')").unwrap_err());

        assert!(error.to_string().contains("levels of evaluation"));
    }

    #[test]
    fn a_chain_of_deep_expressions_fails_on_a_worker_stack() {
        let read = format!(
            "{}node({{next}}, 'font_size'){}",
            "abs(".repeat(63),
            ")".repeat(63)
        );
        let graph = chained_text_nodes(8, &read, "'a'");

        let error =
            on_worker_stack(move || evaluate_in(&graph, "node(1, 'font_size')").unwrap_err());

        assert!(error.to_string().contains("levels of evaluation"));
    }

    #[test]
    fn the_whole_budget_fits_a_worker_stack_in_the_largest_frames() {
        // Right-nested binaries are the deepest frames.
        let nest = |levels: usize, leaf: &str| {
            format!("{}{leaf}{}", "1 + (".repeat(levels), ")".repeat(levels))
        };
        let cost = |depth: u32| depth + EVALUATION_OVERHEAD;
        let nodes = 2;
        let property_levels = (MAX_EVALUATION_LEVELS - cost(1)) / nodes - cost(1);
        let measure_levels = (MAX_EVALUATION_LEVELS - cost(2)) / nodes - cost(3);
        let read = nest(property_levels as usize, "node({next}, 'font_size')");
        let measure = format!(
            "text_width({})",
            nest(measure_levels as usize, "text_width(node({next}))")
        );
        let graph = std::sync::Arc::new(chained_text_nodes(nodes as u64 + 1, &read, &measure));

        for source in ["node(1, 'font_size')", "text_width(node(1))"] {
            let graph = graph.clone();
            let value = on_worker_stack(move || evaluate_in(&graph, source));
            assert!(value.is_ok(), "{source}: {value:?}");
        }
    }

    #[test]
    fn clamp_with_a_nan_bound_is_an_error_instead_of_a_panic() {
        for source in ["clamp(1, pow(-1, 0.5), 2)", "clamp(1, 0, pow(-1, 0.5))"] {
            let error = Expression::parse(source)
                .unwrap()
                .evaluate(&test_context())
                .expect_err("NaN bound");
            assert!(error.to_string().contains("clamp requires min <= max"));
        }
    }
}
