use std::{
    cell::Cell,
    hash::{Hash, Hasher},
};

use crate::{
    error::LumenError,
    expr::{
        ExpressionContext, GlyphScope, InstanceScope,
        ast::{BinaryOp, BuiltinFn, ExprNode, Expression, ExpressionValue, GlobalVar, UnaryOp},
        builtins::{evaluate_builtin, measure_node, unusable_result},
    },
    node::{NodeId, PropertyEval, PropertyRead},
};

impl Expression {
    /// The value of the expression in `ctx`. An evaluation that no other evaluation is inside,
    /// in a context with held values and no glyph or instance scope, reuses a value they hold.
    pub fn evaluate(&self, ctx: &ExpressionContext<'_>) -> crate::Result<ExpressionValue> {
        match ctx.held {
            Some(held) if ctx.glyph.is_none() && ctx.instance.is_none() && outermost() => {
                held.evaluate(self, ctx)
            }
            _ => self.evaluate_now(ctx),
        }
    }

    pub(super) fn evaluate_now(
        &self,
        ctx: &ExpressionContext<'_>,
    ) -> crate::Result<ExpressionValue> {
        let _evaluation = EvaluationGuard::enter(self.depth)?;
        self.plan.load();
        let mut scratch = Scratch::new(&self.plan);
        usable(
            self.plan
                .evaluate_step(self.plan.roots[0], ctx, &mut scratch)?,
        )
    }
}

/// `value`, unless it is a kind no param takes.
pub(super) fn usable(value: ExpressionValue) -> crate::Result<ExpressionValue> {
    match unusable_result(value.kind()) {
        Some(details) => Err(LumenError::evaluate(details)),
        None => Ok(value),
    }
}

/// Expressions evaluated for each copy of an `instance_3d` as one plan, so a step they share is
/// evaluated once per copy, and a step that reads neither `instance` nor `instance_count` is
/// evaluated once for all copies, by the first copy that reaches it. Every evaluation must be in
/// the same context but for its instance scope. A step that is not pure, or reads one that is
/// not, is evaluated for each expression as it would be alone.
#[cfg_attr(
    not(feature = "3d"),
    allow(dead_code, reason = "only instance_3d evaluates copies")
)]
pub(crate) struct InstanceEvaluator {
    plan: EvalPlan,
    depths: Vec<usize>,
    scratch: Scratch,
    /// The copy whose values the memo holds.
    copy: Option<InstanceScope>,
}

#[cfg_attr(
    not(feature = "3d"),
    allow(dead_code, reason = "only instance_3d evaluates copies")
)]
impl InstanceEvaluator {
    pub(crate) fn new<'e>(expressions: impl IntoIterator<Item = &'e Expression>) -> Self {
        let expressions: Vec<&Expression> = expressions.into_iter().collect();
        let asts: Vec<&ExprNode> = expressions
            .iter()
            .map(|expression| &expression.ast)
            .collect();
        let plan = EvalPlan::build(&asts, true);
        Self {
            scratch: Scratch::new(&plan),
            plan,
            depths: expressions
                .iter()
                .map(|expression| expression.depth)
                .collect(),
            copy: None,
        }
    }

    /// The value of the `index`th expression for the copy `ctx` is scoped to.
    pub(crate) fn evaluate(
        &mut self,
        index: usize,
        ctx: &ExpressionContext<'_>,
    ) -> crate::Result<ExpressionValue> {
        if self.copy != ctx.instance {
            self.scratch.memo[self.plan.kept_slots..].fill(None);
            self.copy = ctx.instance;
        } else {
            self.scratch.memo[self.plan.shared_slots..].fill(None);
        }
        let _evaluation = EvaluationGuard::enter(self.depths[index])?;
        usable(
            self.plan
                .evaluate_step(self.plan.roots[index], ctx, &mut self.scratch)?,
        )
    }
}

/// Most levels of evaluation that may be open at once, counting an expression's depth for each
/// evaluation that is open. Reading another node's property or measuring its text evaluates its
/// expression inside the reader's, so this bounds the stack that nodes reading through each other
/// use. It is sized for the largest frames, on a 1 MiB stack.
pub(super) const MAX_EVALUATION_LEVELS: u32 = 96;
/// A level of a hold walk uses up to this many times the stack of a level of plain evaluation, so
/// it is charged this many levels.
pub(super) const HELD_LEVEL_COST: u32 = 2;
/// Levels charged to every evaluation on top of its expression's depth, for the read or
/// measurement that starts it.
pub(super) const EVALUATION_OVERHEAD: u32 = 2;
/// Most evaluations that one outermost evaluation may start, including all it reads through.
const MAX_EVALUATIONS: u32 = 10_000;

thread_local! {
    /// The levels of the open evaluations and the evaluations started since the outermost one began.
    static EVALUATIONS: Cell<(u32, u32)> = const { Cell::new((0, 0)) };
    /// Whether an evaluation was refused for its levels since [`ran_out_of_levels`] last ran.
    static OUT_OF_LEVELS: Cell<bool> = const { Cell::new(false) };
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static STARTED: Cell<u64> = const { Cell::new(0) };
}

/// How many evaluations of an expression this thread has started, counting those started inside
/// another, as the evaluation limits count them.
#[cfg(any(test, feature = "test-hooks"))]
pub fn evaluation_count() -> u64 {
    STARTED.get()
}

/// Whether no evaluation is open on this thread.
pub(super) fn outermost() -> bool {
    EVALUATIONS.get().0 == 0
}

/// Whether an evaluation was refused for its levels since this last ran.
pub(super) fn ran_out_of_levels() -> bool {
    OUT_OF_LEVELS.replace(false)
}

pub(super) struct EvaluationGuard {
    levels: u32,
}

impl EvaluationGuard {
    fn charge(depth: usize) -> u32 {
        u32::try_from(depth)
            .unwrap_or(u32::MAX)
            .saturating_add(EVALUATION_OVERHEAD)
    }

    pub(super) fn enter(depth: usize) -> crate::Result<Self> {
        Self::open(Self::charge(depth))
    }

    /// Enters a hold walk of an expression of `depth`, which is charged [`HELD_LEVEL_COST`] times
    /// the levels of a plain evaluation.
    pub(super) fn enter_held(depth: usize) -> crate::Result<Self> {
        Self::open(Self::charge(depth).saturating_mul(HELD_LEVEL_COST))
    }

    fn open(levels: u32) -> crate::Result<Self> {
        let (open, started) = EVALUATIONS.get();
        let exhausted = if open.saturating_add(levels) > MAX_EVALUATION_LEVELS {
            OUT_OF_LEVELS.set(true);
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
        #[cfg(any(test, feature = "test-hooks"))]
        STARTED.set(STARTED.get() + 1);
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
/// expanded easing curves repeat their progress, is evaluated once per evaluation. A subtree that
/// reads no global, node or frame is evaluated once when the plan is built, unless that fails.
/// Steps are evaluated in the AST's order and only where it evaluates them, so `&&`, `||` and
/// conditional branches stay lazy.
#[derive(Debug)]
pub(crate) struct EvalPlan {
    pub(super) steps: Vec<Step>,
    /// The step of each expression the plan evaluates.
    pub(super) roots: Vec<usize>,
    /// The values of the steps that were evaluated when the plan was built.
    pub(super) constants: Vec<ExpressionValue>,
    pub(super) memo_slots: usize,
    /// Memo slots below this one hold steps whose value is the same for every copy of an
    /// `instance_3d`.
    kept_slots: usize,
    /// Memo slots from `kept_slots` below this one hold pure steps shared by the expressions for
    /// one copy. The slots from here hold steps that are not pure, which an expression reuses
    /// only within its own evaluation.
    shared_slots: usize,
}

#[derive(Debug)]
pub(super) struct Step {
    pub(super) op: Op,
    /// Where an evaluation keeps the value of the step, when more than one step reads it or it
    /// is kept for every copy.
    pub(super) memo: Option<usize>,
    /// Whether evaluating it again neither reads a node, so it starts no evaluation of its own,
    /// nor measures text.
    pub(super) repeatable: bool,
}

/// What an evaluation of a plan works in: the memo of its steps, and the arguments of the builtin
/// calls it has open.
#[derive(Debug)]
pub(super) struct Scratch {
    memo: Vec<Option<ExpressionValue>>,
    args: Vec<ExpressionValue>,
}

impl Scratch {
    pub(super) fn new(plan: &EvalPlan) -> Self {
        Self {
            memo: vec![None; plan.memo_slots],
            args: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum Op {
    Literal(Literal),
    /// An index into the plan's constants.
    Constant(usize),
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
pub(super) struct Literal(pub(super) ExpressionValue);

impl Literal {
    fn key(&self) -> (u8, [u64; 4], &str) {
        match &self.0 {
            ExpressionValue::Number(number) => (0, [number.to_bits(), 0, 0, 0], ""),
            ExpressionValue::Boolean(boolean) => (1, [u64::from(*boolean), 0, 0, 0], ""),
            ExpressionValue::String(text) => (2, [0; 4], text),
            ExpressionValue::Vec2((x, y)) => (3, [x.to_bits(), y.to_bits(), 0, 0], ""),
            ExpressionValue::Color(color) => (4, color.map(f64::to_bits), ""),
            ExpressionValue::Vec3([x, y, z]) => (5, [x.to_bits(), y.to_bits(), z.to_bits(), 0], ""),
            ExpressionValue::Easing(_) | ExpressionValue::Key(_) => {
                unreachable!("the parser makes no easing or key literals")
            }
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

impl Op {
    /// Whether the value can differ for the same operands and frame-constant context: it reads
    /// another node, measures text with a font session that its measurements load fonts into, or
    /// reads the props of the template. Every builtin is listed, so a new one has to be classed.
    fn reads_state(&self) -> bool {
        match self {
            Op::ReadProperty(..) | Op::NodeRef(_) | Op::MeasureNode(..) => true,
            Op::Literal(_)
            | Op::Constant(_)
            | Op::Global(_)
            | Op::Unary(..)
            | Op::Binary(..)
            | Op::Conditional(..) => false,
            Op::Builtin(builtin, _) => match builtin {
                BuiltinFn::TextWidth | BuiltinFn::TextHeight | BuiltinFn::Prop => true,
                BuiltinFn::Min
                | BuiltinFn::Max
                | BuiltinFn::Abs
                | BuiltinFn::Floor
                | BuiltinFn::Ceil
                | BuiltinFn::Round
                | BuiltinFn::Sin
                | BuiltinFn::Cos
                | BuiltinFn::Clamp
                | BuiltinFn::Lerp
                | BuiltinFn::Pow
                | BuiltinFn::Mod
                | BuiltinFn::Fract
                | BuiltinFn::Sign
                | BuiltinFn::Atan2
                | BuiltinFn::Hypot
                | BuiltinFn::Remap
                | BuiltinFn::Smoothstep
                | BuiltinFn::Step
                | BuiltinFn::Uppercase
                | BuiltinFn::Lowercase
                | BuiltinFn::Format
                | BuiltinFn::Concat
                | BuiltinFn::Vec2
                | BuiltinFn::Vec3
                | BuiltinFn::Rgb
                | BuiltinFn::Rgba
                | BuiltinFn::Progress
                | BuiltinFn::Tween
                | BuiltinFn::Ease
                | BuiltinFn::CubicBezier
                | BuiltinFn::Key
                | BuiltinFn::Keyframes
                | BuiltinFn::Noise
                | BuiltinFn::Random
                | BuiltinFn::Spring
                | BuiltinFn::PathPoint
                | BuiltinFn::PathAngle
                | BuiltinFn::InQuad
                | BuiltinFn::OutQuad
                | BuiltinFn::InOutQuad
                | BuiltinFn::InCubic
                | BuiltinFn::OutCubic
                | BuiltinFn::InOutCubic
                | BuiltinFn::InExpo
                | BuiltinFn::OutExpo
                | BuiltinFn::InOutExpo
                | BuiltinFn::InBack
                | BuiltinFn::OutBack
                | BuiltinFn::InOutBack
                | BuiltinFn::InSine
                | BuiltinFn::OutSine
                | BuiltinFn::InOutSine
                | BuiltinFn::InQuart
                | BuiltinFn::OutQuart
                | BuiltinFn::InOutQuart
                | BuiltinFn::InQuint
                | BuiltinFn::OutQuint
                | BuiltinFn::InOutQuint
                | BuiltinFn::InCirc
                | BuiltinFn::OutCirc
                | BuiltinFn::InOutCirc
                | BuiltinFn::InElastic
                | BuiltinFn::OutElastic
                | BuiltinFn::InOutElastic
                | BuiltinFn::InBounce
                | BuiltinFn::OutBounce
                | BuiltinFn::InOutBounce => false,
            },
        }
    }

    /// Whether it reads a node or measures text.
    fn reads_nodes_or_text(&self) -> bool {
        matches!(
            self,
            Op::ReadProperty(..)
                | Op::NodeRef(_)
                | Op::MeasureNode(..)
                | Op::Builtin(BuiltinFn::TextWidth | BuiltinFn::TextHeight, _)
        )
    }

    /// The steps this one reads.
    fn operands(&self) -> impl Iterator<Item = usize> + '_ {
        let (fixed, args): ([Option<usize>; 3], &[usize]) = match self {
            Op::Unary(_, value) => ([Some(*value), None, None], &[]),
            Op::Binary(lhs, _, rhs) => ([Some(*lhs), Some(*rhs), None], &[]),
            Op::Builtin(_, args) => ([None; 3], args),
            Op::Conditional(condition, when_true, when_false) => {
                ([Some(*condition), Some(*when_true), Some(*when_false)], &[])
            }
            Op::Literal(_)
            | Op::Constant(_)
            | Op::Global(_)
            | Op::ReadProperty(..)
            | Op::NodeRef(_)
            | Op::MeasureNode(..) => ([None; 3], &[]),
        };
        fixed.into_iter().flatten().chain(args.iter().copied())
    }
}

/// Which memo slots a step's value is kept in, in their order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slots {
    Kept,
    Shared,
    Local,
}

/// Whether `op` is cheaper to evaluate than to remember.
fn is_leaf(op: &Op) -> bool {
    matches!(op, Op::Literal(_) | Op::Constant(_) | Op::Global(_))
}

impl EvalPlan {
    pub(crate) fn new(ast: &ExprNode) -> Self {
        Self::build(&[ast], false)
    }

    /// The plan of `asts` together. With `per_copy`, a pure step that reads neither `instance` nor
    /// `instance_count` keeps its value for every copy, in a slot below `kept_slots`, when more
    /// than one step reads it, a step reading the instance scope reads it, or it is one of `asts`.
    fn build(asts: &[&ExprNode], per_copy: bool) -> Self {
        let mut builder = PlanBuilder::default();
        let roots: Vec<usize> = asts.iter().map(|ast| builder.add(ast)).collect();
        let (ops, constants) = (builder.ops, builder.constants);
        let mut reads = vec![0_u32; ops.len()];
        // Whether a step reads the instance scope, whether it reads something that is not pure,
        // and whether a step that reads the instance scope reads it.
        let mut copied = vec![false; ops.len()];
        let mut impure = vec![false; ops.len()];
        let mut repeatable = vec![true; ops.len()];
        let mut read_by_copied = vec![false; ops.len()];
        for root in &roots {
            reads[*root] += 1;
            read_by_copied[*root] = true;
        }
        // Operands come before the steps reading them.
        for (index, op) in ops.iter().enumerate() {
            copied[index] = matches!(
                op,
                Op::Global(GlobalVar::Instance | GlobalVar::InstanceCount)
            ) || op.operands().any(|operand| copied[operand]);
            impure[index] = op.reads_state() || op.operands().any(|operand| impure[operand]);
            repeatable[index] =
                !op.reads_nodes_or_text() && op.operands().all(|operand| repeatable[operand]);
            for operand in op.operands() {
                reads[operand] += 1;
                read_by_copied[operand] |= copied[index];
            }
        }
        let slots: Vec<Option<Slots>> = (0..ops.len())
            .map(|index| {
                if is_leaf(&ops[index]) {
                    None
                } else if impure[index] {
                    (reads[index] > 1).then_some(Slots::Local)
                } else if per_copy && !copied[index] && (reads[index] > 1 || read_by_copied[index])
                {
                    Some(Slots::Kept)
                } else {
                    (reads[index] > 1).then_some(Slots::Shared)
                }
            })
            .collect();
        let count = |class| slots.iter().filter(|slot| **slot == Some(class)).count();
        let kept_slots = count(Slots::Kept);
        let shared_slots = kept_slots + count(Slots::Shared);
        let mut next = [0, kept_slots, shared_slots];
        let steps = ops
            .into_iter()
            .zip(&slots)
            .zip(repeatable)
            .map(|((op, slot), repeatable)| Step {
                op,
                memo: slot.map(|class| {
                    next[class as usize] += 1;
                    next[class as usize] - 1
                }),
                repeatable,
            })
            .collect();
        Self {
            steps,
            roots,
            constants,
            memo_slots: next[Slots::Local as usize],
            kept_slots,
            shared_slots,
        }
    }

    /// Reads every cache line of the plan once. A plan is usually out of cache when an evaluation
    /// of it starts, as most are evaluated once a frame, and an evaluation reads its steps one
    /// after another, each waiting for the last to load. These reads don't wait for each other.
    pub(super) fn load(&self) {
        let mut read = 0_usize;
        for step in &self.steps {
            read = read.wrapping_add(usize::from(step.repeatable));
            if let Op::Builtin(_, args) = &step.op {
                read = read.wrapping_add(args.first().copied().unwrap_or(0));
            }
        }
        for constant in &self.constants {
            read = read.wrapping_add(usize::from(constant.as_f64().is_some()));
        }
        std::hint::black_box(read);
    }

    pub(super) fn evaluate_step(
        &self,
        index: usize,
        ctx: &ExpressionContext<'_>,
        scratch: &mut Scratch,
    ) -> crate::Result<ExpressionValue> {
        let step = &self.steps[index];
        if let Some(slot) = step.memo
            && let Some(value) = &scratch.memo[slot]
        {
            return Ok(value.clone());
        }
        let value = match &step.op {
            Op::Literal(Literal(value)) => value.clone(),
            Op::Constant(index) => self.constants[*index].clone(),
            Op::Global(global) => global_value(*global, ctx)?,
            Op::Unary(op, value) => unary(*op, self.evaluate_step(*value, ctx, scratch)?)?,
            Op::Binary(lhs, op, rhs) => {
                let lhs = self.evaluate_step(*lhs, ctx, scratch)?;
                binary(lhs, *op, || self.evaluate_step(*rhs, ctx, scratch))?
            }
            Op::Builtin(builtin, args) => {
                // The arguments go on the stack of open calls' arguments, and come off it however
                // the call ends.
                let start = scratch.args.len();
                let value = args
                    .iter()
                    .try_for_each(|arg| {
                        let value = self.evaluate_step(*arg, ctx, scratch)?;
                        scratch.args.push(value);
                        Ok(())
                    })
                    .and_then(|()| evaluate_builtin(*builtin, &scratch.args[start..], ctx));
                scratch.args.truncate(start);
                value?
            }
            Op::Conditional(condition, when_true, when_false) => {
                let branch = if self.evaluate_step(*condition, ctx, scratch)?.as_bool() {
                    when_true
                } else {
                    when_false
                };
                self.evaluate_step(*branch, ctx, scratch)?
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
            scratch.memo[slot] = Some(value.clone());
        }
        Ok(value)
    }
}

#[derive(Default)]
struct PlanBuilder {
    ops: Vec<Op>,
    constants: Vec<ExpressionValue>,
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
        let folded = self.fold(&op).map(|value| {
            self.constants.push(value);
            Op::Constant(self.constants.len() - 1)
        });
        self.ops.push(folded.unwrap_or_else(|| op.clone()));
        self.interned.insert(op, self.ops.len() - 1);
        self.ops.len() - 1
    }

    /// The value of `op` when it reads nothing but values known now and evaluating it succeeds.
    /// A failure is left for the evaluation, which may never reach it.
    fn fold(&self, op: &Op) -> Option<ExpressionValue> {
        let value = |step: &usize| match &self.ops[*step] {
            Op::Literal(Literal(value)) => Some(value),
            Op::Constant(index) => Some(&self.constants[*index]),
            _ => None,
        };
        match op {
            Op::Unary(unary_op, operand) => unary(*unary_op, value(operand)?.clone()).ok(),
            Op::Binary(lhs, binary_op, rhs) => {
                let (lhs, rhs) = (value(lhs)?.clone(), value(rhs)?.clone());
                binary(lhs, *binary_op, || Ok(rhs)).ok()
            }
            Op::Conditional(condition, when_true, when_false) => {
                let (condition, when_true, when_false) =
                    (value(condition)?, value(when_true)?, value(when_false)?);
                Some(
                    if condition.as_bool() {
                        when_true
                    } else {
                        when_false
                    }
                    .clone(),
                )
            }
            Op::Builtin(builtin, args) if !op.reads_state() && !builtin.reads_frame() => {
                let args: Vec<ExpressionValue> = args
                    .iter()
                    .map(|arg| value(arg).cloned())
                    .collect::<Option<_>>()?;
                evaluate_builtin(*builtin, &args, &ExpressionContext::default()).ok()
            }
            _ => None,
        }
    }
}

/// The value of `property` on `node_id`, evaluating its expression if it has one.
fn read_property(
    node_id: NodeId,
    property: &str,
    ctx: &ExpressionContext<'_>,
) -> crate::Result<ExpressionValue> {
    read_property_with(node_id, property, ctx, Expression::evaluate, |value| value)
}

/// What `node(node_id, 'property')` reads: `evaluate` of its expression, located at the property,
/// or `literal` of its value.
pub(super) fn read_property_with<T>(
    node_id: NodeId,
    property: &str,
    ctx: &ExpressionContext<'_>,
    evaluate: impl FnOnce(&Expression, &ExpressionContext<'_>) -> crate::Result<T>,
    literal: impl FnOnce(ExpressionValue) -> T,
) -> crate::Result<T> {
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
        PropertyRead::Expr(inner_expr) => evaluate(inner_expr, &ctx.without_scopes())
            .map_err(|error| error.located(node_id, property)),
        PropertyRead::Value(value) => value.map(literal).ok_or_else(|| {
            LumenError::evaluate(format!(
                "property `{property}` on node {} can't be read in an expression",
                node_id.0
            ))
        }),
    }
}

pub(super) fn unary(op: UnaryOp, value: ExpressionValue) -> crate::Result<ExpressionValue> {
    match op {
        UnaryOp::Neg => match value {
            ExpressionValue::Vec2((x, y)) => Ok(ExpressionValue::Vec2((-x, -y))),
            ExpressionValue::Vec3(vector) => Ok(ExpressionValue::Vec3(vector.map(|x| -x))),
            other => Ok(ExpressionValue::Number(-to_number(&other)?)),
        },
        UnaryOp::Not => Ok(ExpressionValue::Boolean(!value.as_bool())),
    }
}

/// `lhs op rhs`, evaluating `rhs` unless `&&` or `||` is decided by `lhs`.
pub(super) fn binary(
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

pub(super) fn global_value(
    global: GlobalVar,
    ctx: &ExpressionContext<'_>,
) -> crate::Result<ExpressionValue> {
    let in_glyph = |read: fn(&GlyphScope) -> u32| {
        ctx.glyph.as_ref().map(read).map(f64::from).ok_or_else(|| {
            LumenError::evaluate(format!(
                "`{}` is only available in a text node's glyph_offset, glyph_scale, \
                 glyph_rotation and glyph_opacity params",
                global.name()
            ))
        })
    };
    let in_instance = |read: fn(&InstanceScope) -> u32| {
        ctx.instance
            .as_ref()
            .map(read)
            .map(f64::from)
            .ok_or_else(|| {
                LumenError::evaluate(format!(
                    "`{}` is only available in an instance_3d's position, rotation, scale \
                     and tint params",
                    global.name()
                ))
            })
    };
    Ok(ExpressionValue::Number(match global {
        GlobalVar::Instance => in_instance(|scope| scope.index)?,
        GlobalVar::InstanceCount => in_instance(|scope| scope.count)?,
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

/// Numbers combine as usual. Two vec2 or two vec3 values combine componentwise, and a vector
/// can be multiplied or divided by a number on either side. Colours have no arithmetic.
fn arithmetic(
    op: BinaryOp,
    lhs: &ExpressionValue,
    rhs: &ExpressionValue,
) -> crate::Result<ExpressionValue> {
    let vec2 = |value: &ExpressionValue| value.as_vec2().map(|(x, y)| [x, y]);
    if let Some([x, y]) = componentwise(op, lhs, rhs, vec2)? {
        return Ok(ExpressionValue::Vec2((x, y)));
    }
    if let Some(vector) = componentwise(op, lhs, rhs, ExpressionValue::as_vec3)? {
        return Ok(ExpressionValue::Vec3(vector));
    }
    match (lhs, rhs) {
        (ExpressionValue::Vec2(_) | ExpressionValue::Vec3(_) | ExpressionValue::Color(_), _)
        | (_, ExpressionValue::Vec2(_) | ExpressionValue::Vec3(_) | ExpressionValue::Color(_)) => {
            Err(LumenError::evaluate(format!(
                "cannot apply `{}` to {} and {}",
                operator_symbol(op),
                lhs.type_name(),
                rhs.type_name()
            )))
        }
        _ => Ok(ExpressionValue::Number(apply(
            op,
            to_number(lhs)?,
            to_number(rhs)?,
        )?)),
    }
}

/// `lhs op rhs` for vectors that `vector` reads, or `None` when neither side is one or the
/// operator doesn't apply to them. A number on either side of `*` or `/` scales the vector.
fn componentwise<const N: usize>(
    op: BinaryOp,
    lhs: &ExpressionValue,
    rhs: &ExpressionValue,
    vector: impl Fn(&ExpressionValue) -> Option<[f64; N]>,
) -> crate::Result<Option<[f64; N]>> {
    let scales = matches!(op, BinaryOp::Mul | BinaryOp::Div);
    let (a, b) = match (vector(lhs), vector(rhs)) {
        (Some(a), Some(b)) if op != BinaryOp::Mod => (a, b),
        (Some(a), None) if scales => (a, [to_number(rhs)?; N]),
        (None, Some(b)) if scales => ([to_number(lhs)?; N], b),
        _ => return Ok(None),
    };
    let mut result = [0.0; N];
    for (index, component) in result.iter_mut().enumerate() {
        *component = apply(op, a[index], b[index])?;
    }
    Ok(Some(result))
}

/// `a op b` for an arithmetic operator.
fn apply(op: BinaryOp, a: f64, b: f64) -> crate::Result<f64> {
    match op {
        BinaryOp::Add => Ok(a + b),
        BinaryOp::Sub => Ok(a - b),
        BinaryOp::Mul => Ok(a * b),
        BinaryOp::Div if b == 0.0 => Err(LumenError::evaluate("division by zero")),
        BinaryOp::Div => Ok(a / b),
        BinaryOp::Mod if b == 0.0 => Err(LumenError::evaluate("modulo by zero")),
        _ => Ok(a % b),
    }
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
#[path = "eval_tests.rs"]
mod tests;
