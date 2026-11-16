//! An expression's value at a frame, with the frames around it on which it keeps those bits.
//!
//! [`Expression::evaluate_held`] evaluates each step with the functions [`Expression::evaluate`]
//! uses, in the same order and as lazily, so it gives the same value or error. Alongside, each step
//! gets a closed range of frames around `ctx.frame`, ordered by [`f64::total_cmp`], on which, the
//! rest of the context being the same, the step is
//!
//! - constant: it evaluates to the same bits on every frame of the range, or
//! - monotone: a number, finite at `ctx.frame` and never NaN, that never decreases, or never
//!   increases, as the frame does, with a linear form used only to guess where it crosses a value.
//!
//! A step that is neither gets `[frame, frame]`, on which it is constant. Why the ranges hold:
//!
//! - Literals, folded constants, `fps`, `width`, `height` and `prop(...)` don't read the frame. A
//!   pure builtin or operator of constant operands is constant where they all are; a conditional,
//!   `&&` or `||` whose deciding operand is constant takes the same branch and is what it is.
//! - `frame` is monotone, and `time` is `frame / fps`. Rounding is monotone, so adding a finite
//!   constant, subtracting from one, negating, multiplying or dividing by a finite non-zero
//!   constant, adding two monotone numbers that move the same way, and `min`, `max` or `clamp` with
//!   finite constants keep a number monotone, and none of them makes a NaN from operands finite at
//!   `ctx.frame`.
//! - A predicate on a monotone number whose truth set is an interval of numbers holds on an
//!   interval of frames: a comparison with a constant, the number strictly past the bound a
//!   `clamp`, `min` or `max` saturates at, `progress` strictly before its start or after its end,
//!   the piece of a `keyframes` track `t` is in, or `floor`, `ceil` or `round` giving one whole
//!   number. Where it holds, so does the value it decides; a zero keeps its sign only while their
//!   argument stays strictly off zero. A side is found by guessing the far end from the linear
//!   form, clamped to where the operands hold, and checking the predicate there by evaluating the
//!   step again, then at a few nearer frames, or else stopping at `ctx.frame`. A side only the
//!   bound the number moves away from could end needs no check.
//! - A side found by a strict predicate moves out one frame in `total_cmp` order when the step
//!   evaluates to the same bits there, as no frame lies between.
//! - A node read takes the range of the held evaluation of the expression it reads. Text
//!   measurement depends on the fonts loaded, and gets `[frame, frame]`. Only a step that reads no
//!   node and measures no text is evaluated again, so the evaluations started, which
//!   `EvaluationGuard` counts, are those `evaluate` starts.
//!
//! A hold walk uses more stack per level than plain evaluation, so it is charged
//! `HELD_LEVEL_COST` times the levels of a plain evaluation, and one that does not fit the levels
//! left is not started: the expression is evaluated plainly and held for its own frame only. The
//! outermost evaluation whose hold walk fails after running out of levels is evaluated plainly, and
//! gives that result, as a plain evaluation nested in a walk has fewer levels left than it would
//! alone. So the errors and limits are those of plain evaluation, and the levels open never exceed
//! `2 * (held levels) + (plain levels) <= MAX_EVALUATION_LEVELS`, the stack a plain evaluation at
//! that limit can use.
//!
//! An expression that reads no node is first evaluated without the checks, which can only widen
//! ranges, and evaluated again with them only when that leaves more than its own frame.

use crate::{
    error::LumenError,
    expr::{
        ExpressionContext,
        ast::{BinaryOp, BuiltinFn, Easing, Expression, ExpressionValue, GlobalVar, UnaryOp},
        builtins::{evaluate_builtin, last_key_at, measure_node, progress_ratio, track},
        eval::{
            EvalPlan, EvaluationGuard, Literal, Op, Scratch, binary, global_value, outermost,
            ran_out_of_levels, read_property_with, unary, usable,
        },
        spec::{NamedEasing, easing_builtins},
    },
};

/// An expression's value at a frame, and the frames on which evaluating it gives the same bits,
/// both ends included and ordered by [`f64::total_cmp`].
#[derive(Debug, Clone)]
pub(crate) struct Held {
    pub(crate) value: ExpressionValue,
    pub(crate) frames: (f64, f64),
    /// Whether evaluating it measured text, or read an expression that did, which a font loaded
    /// since changes.
    pub(crate) measured: bool,
}

pub(crate) const EVERY_FRAME: (f64, f64) = (f64::NEG_INFINITY, f64::INFINITY);

impl Expression {
    /// [`Expression::evaluate`], with the frames around `ctx.frame` on which the value holds, the
    /// rest of `ctx` being the same. `ctx` has no glyph or instance scope.
    pub(crate) fn evaluate_held(&self, ctx: &ExpressionContext<'_>) -> crate::Result<Held> {
        let outermost = outermost();
        if outermost {
            ran_out_of_levels();
        }
        let walked = match EvaluationGuard::enter_held(self.depth) {
            Ok(_evaluation) => self.hold_walk(ctx),
            Err(_) => return self.evaluate_at_frame(ctx),
        };
        match walked {
            Err(_) if outermost && ran_out_of_levels() => self.evaluate_at_frame(ctx),
            walked => walked,
        }
    }

    /// The plain value, held for `ctx.frame` only.
    fn evaluate_at_frame(&self, ctx: &ExpressionContext<'_>) -> crate::Result<Held> {
        Ok(Held {
            value: self.evaluate_now(ctx)?,
            frames: (ctx.frame, ctx.frame),
            // Without a walk it can't see whether a step measured, unless none can.
            measured: !self.plan.steps[self.plan.roots[0]].repeatable,
        })
    }

    fn hold_walk(&self, ctx: &ExpressionContext<'_>) -> crate::Result<Held> {
        self.plan.load();
        let root = self.plan.roots[0];
        let held = |checks: bool| {
            let mut holding = Holding {
                plan: &self.plan,
                ctx,
                checks,
                measured: false,
                scratch: HeldScratch::new(&self.plan),
            };
            let step = holding.step(root)?;
            crate::Result::Ok((step, holding.measured))
        };
        let (mut step, mut measured) = held(!self.plan.steps[root].repeatable)?;
        let frames = |step: &HeldStep| match step.hold.shape {
            Shape::Constant => step.hold.frames,
            Shape::Monotone(_) => (ctx.frame, ctx.frame),
        };
        if frames(&step) != (ctx.frame, ctx.frame) && self.plan.steps[root].repeatable {
            (step, measured) = held(true)?;
        }
        Ok(Held {
            frames: frames(&step),
            value: usable(step.value)?,
            measured,
        })
    }
}

/// How a monotone number moves with the frame: up when `rising`, and about
/// `slope * frame + offset`.
#[derive(Debug, Clone, Copy)]
struct Line {
    rising: bool,
    slope: f64,
    offset: f64,
}

impl Line {
    const FRAME: Self = Self {
        rising: true,
        slope: 1.0,
        offset: 0.0,
    };

    /// The frame at which the linear form reaches `value`.
    fn crossing(self, value: f64) -> f64 {
        (value - self.offset) / self.slope
    }

    fn negated(self) -> Self {
        Self {
            rising: !self.rising,
            slope: -self.slope,
            offset: -self.offset,
        }
    }

    fn plus(self, constant: f64) -> Self {
        Self {
            offset: self.offset + constant,
            ..self
        }
    }

    fn times(self, factor: f64) -> Self {
        Self {
            rising: self.rising == (factor > 0.0),
            slope: self.slope * factor,
            offset: self.offset * factor,
        }
    }

    fn over(self, divisor: f64) -> Self {
        Self {
            rising: self.rising == (divisor > 0.0),
            slope: self.slope / divisor,
            offset: self.offset / divisor,
        }
    }

    fn sum(self, other: Self) -> Self {
        Self {
            rising: self.rising,
            slope: self.slope + other.slope,
            offset: self.offset + other.offset,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Shape {
    Constant,
    Monotone(Line),
}

/// What a step is on the frames it holds on.
#[derive(Debug, Clone, Copy)]
struct Hold {
    frames: (f64, f64),
    shape: Shape,
}

impl Hold {
    const ALWAYS: Self = Self::constant(EVERY_FRAME);

    const fn constant(frames: (f64, f64)) -> Self {
        Self {
            frames,
            shape: Shape::Constant,
        }
    }

    fn at(frame: f64) -> Self {
        Self::constant((frame, frame))
    }

    fn monotone(frames: (f64, f64), line: Line) -> Self {
        Self {
            frames,
            shape: Shape::Monotone(line),
        }
    }

    fn is_constant(&self) -> bool {
        matches!(self.shape, Shape::Constant)
    }
}

#[derive(Debug, Clone)]
struct HeldStep {
    value: ExpressionValue,
    hold: Hold,
}

/// An evaluated operand: its step, its value as a number if it is one, and how it holds.
#[derive(Debug, Clone, Copy)]
struct Operand {
    step: usize,
    number: Option<f64>,
    hold: Hold,
}

#[derive(Debug)]
struct HeldScratch {
    memo: Vec<Option<HeldStep>>,
    args: Vec<ExpressionValue>,
    holds: Vec<Hold>,
}

impl HeldScratch {
    fn new(plan: &EvalPlan) -> Self {
        Self {
            memo: vec![None; plan.memo_slots],
            args: Vec::new(),
            holds: Vec::new(),
        }
    }
}

/// A builtin call being held: its step, the steps of its arguments, their values and holds.
struct Call<'a> {
    step: usize,
    builtin: BuiltinFn,
    steps: &'a [usize],
    args: &'a [ExpressionValue],
    holds: &'a [Hold],
}

/// The numbers above `low` and below `high`, each bound included when its flag is set.
#[derive(Debug, Clone, Copy)]
struct Band {
    low: Option<(f64, bool)>,
    high: Option<(f64, bool)>,
}

impl Band {
    fn below(bound: f64, included: bool) -> Self {
        Self {
            low: None,
            high: Some((bound, included)),
        }
    }

    fn above(bound: f64, included: bool) -> Self {
        Self {
            low: Some((bound, included)),
            high: None,
        }
    }

    fn open(low: f64, high: f64) -> Self {
        Self {
            low: Some((low, false)),
            high: Some((high, false)),
        }
    }

    fn contains(self, x: f64) -> bool {
        self.low
            .is_none_or(|(bound, included)| x > bound || (included && x == bound))
            && self
                .high
                .is_none_or(|(bound, included)| x < bound || (included && x == bound))
    }

    /// The bounds a number moving like `line` meets going back and going forward in frames.
    fn ends(self, line: Line) -> [Option<f64>; 2] {
        let [low, high] = [self.low, self.high].map(|bound| bound.map(|(bound, _)| bound));
        if line.rising {
            [low, high]
        } else {
            [high, low]
        }
    }
}

/// How a builtin's value follows its arguments. Every builtin is listed, so a new one has to be
/// classed.
enum Rule {
    /// A function of its arguments alone.
    Pure,
    /// Pure, and holds while a monotone argument stays in a band.
    Shaped,
    /// Counts from the frame being evaluated.
    ReadsFrame,
    /// Depends on the fonts loaded.
    Measured,
}

fn rule(builtin: BuiltinFn) -> Rule {
    match builtin {
        BuiltinFn::TextWidth | BuiltinFn::TextHeight => Rule::Measured,
        BuiltinFn::Progress | BuiltinFn::Tween => Rule::ReadsFrame,
        BuiltinFn::Min
        | BuiltinFn::Max
        | BuiltinFn::Clamp
        | BuiltinFn::Floor
        | BuiltinFn::Ceil
        | BuiltinFn::Round
        | BuiltinFn::Keyframes => Rule::Shaped,
        // `prop` reads the props, which are part of the context.
        BuiltinFn::Prop
        | BuiltinFn::Abs
        | BuiltinFn::Sin
        | BuiltinFn::Cos
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
        | BuiltinFn::Ease
        | BuiltinFn::CubicBezier
        | BuiltinFn::Key
        | BuiltinFn::Noise
        | BuiltinFn::Random
        | BuiltinFn::Spring
        | BuiltinFn::PathPoint
        | BuiltinFn::PathAngle
        | easing_builtins!() => Rule::Pure,
    }
}

/// One held evaluation of a plan. Without `checks`, a range a check would find is the range of the
/// operands it is found in. `measured` is set once a step measures text, here or in a node it
/// reads.
struct Holding<'p, 'c> {
    plan: &'p EvalPlan,
    ctx: &'c ExpressionContext<'c>,
    checks: bool,
    measured: bool,
    scratch: HeldScratch,
}

impl Holding<'_, '_> {
    fn step(&mut self, index: usize) -> crate::Result<HeldStep> {
        let plan = self.plan;
        let ctx = self.ctx;
        let step = &plan.steps[index];
        if let Some(slot) = step.memo
            && let Some(held) = &self.scratch.memo[slot]
        {
            return Ok(held.clone());
        }
        let frame = ctx.frame;
        let (value, hold) = match &step.op {
            Op::Literal(Literal(value)) => (value.clone(), Hold::ALWAYS),
            Op::Constant(index) => (plan.constants[*index].clone(), Hold::ALWAYS),
            Op::Global(global) => (global_value(*global, ctx)?, global_hold(*global, ctx)),
            Op::Unary(op, operand) => {
                let operand = self.step(*operand)?;
                let hold = match (op, operand.hold.shape) {
                    (_, Shape::Constant) => operand.hold,
                    (UnaryOp::Neg, Shape::Monotone(line)) => {
                        Hold::monotone(operand.hold.frames, line.negated())
                    }
                    (UnaryOp::Not, Shape::Monotone(_)) => Hold::at(frame),
                };
                (unary(*op, operand.value)?, hold)
            }
            Op::Binary(lhs, op, rhs) => {
                let held = self.step(*lhs)?;
                let left = Operand {
                    step: *lhs,
                    number: held.value.as_f64(),
                    hold: held.hold,
                };
                let mut right = None;
                let value = binary(held.value, *op, || {
                    let held = self.step(*rhs)?;
                    right = Some(Operand {
                        step: *rhs,
                        number: held.value.as_f64(),
                        hold: held.hold,
                    });
                    Ok(held.value)
                })?;
                let hold = self.binary_hold(*op, left, right, &value);
                (value, hold)
            }
            Op::Builtin(builtin, args) => {
                self.measured |= matches!(rule(*builtin), Rule::Measured);
                // As `evaluate_step` does, with each argument's hold beside its value.
                let start = self.scratch.args.len();
                let value = args
                    .iter()
                    .try_for_each(|arg| {
                        let held = self.step(*arg)?;
                        self.scratch.args.push(held.value);
                        self.scratch.holds.push(held.hold);
                        Ok(())
                    })
                    .and_then(|()| evaluate_builtin(*builtin, &self.scratch.args[start..], ctx));
                let hold = value.as_ref().ok().map(|value| {
                    let call = Call {
                        step: index,
                        builtin: *builtin,
                        steps: args,
                        args: &self.scratch.args[start..],
                        holds: &self.scratch.holds[start..],
                    };
                    self.builtin_hold(&call, value)
                });
                self.scratch.args.truncate(start);
                self.scratch.holds.truncate(start);
                (value?, hold.unwrap_or(Hold::at(frame)))
            }
            Op::Conditional(condition, when_true, when_false) => {
                let condition = self.step(*condition)?;
                let branch = if condition.value.as_bool() {
                    when_true
                } else {
                    when_false
                };
                let branch = self.step(*branch)?;
                let hold = match condition.hold.shape {
                    Shape::Constant => Hold {
                        frames: intersect(condition.hold.frames, branch.hold.frames),
                        shape: branch.hold.shape,
                    },
                    Shape::Monotone(_) => Hold::at(frame),
                };
                (branch.value, hold)
            }
            Op::ReadProperty(node_id, property) => {
                let held = read_property_with(
                    *node_id,
                    property,
                    ctx,
                    Expression::evaluate_held,
                    |value| Held {
                        value,
                        frames: EVERY_FRAME,
                        measured: false,
                    },
                )?;
                self.measured |= held.measured;
                (held.value, Hold::constant(held.frames))
            }
            Op::NodeRef(node_id) => {
                return Err(LumenError::evaluate(format!(
                    "node reference `{}` can only be used in builtins that accept node references",
                    node_id.0
                )));
            }
            Op::MeasureNode(builtin, node_id) => {
                self.measured = true;
                (measure_node(*builtin, *node_id, ctx)?, Hold::at(frame))
            }
        };
        let finite = matches!(value, ExpressionValue::Number(number) if number.is_finite());
        let hold = match hold.shape {
            Shape::Monotone(_) if !finite || !step.repeatable => Hold::at(frame),
            _ => hold,
        };
        let held = HeldStep { value, hold };
        if let Some(slot) = step.memo {
            self.scratch.memo[slot] = Some(held.clone());
        }
        Ok(held)
    }

    fn binary_hold(
        &self,
        op: BinaryOp,
        left: Operand,
        right: Option<Operand>,
        value: &ExpressionValue,
    ) -> Hold {
        let point = Hold::at(self.ctx.frame);
        // Without a right side, `&&` or `||` was decided by the left.
        let Some(right) = right else {
            return if left.hold.is_constant() {
                left.hold
            } else {
                point
            };
        };
        let within = intersect(left.hold.frames, right.hold.frames);
        if left.hold.is_constant() && right.hold.is_constant() {
            return Hold::constant(within);
        }
        let (Some(l), Some(r)) = (left.number, right.number) else {
            return point;
        };
        let constant = |number: f64| number.is_finite();
        let factor = |number: f64| number.is_finite() && number != 0.0;
        let line = match (op, left.hold.shape, right.hold.shape) {
            (BinaryOp::Add, Shape::Monotone(a), Shape::Constant) if constant(r) => a.plus(r),
            (BinaryOp::Add, Shape::Constant, Shape::Monotone(b)) if constant(l) => b.plus(l),
            (BinaryOp::Add, Shape::Monotone(a), Shape::Monotone(b)) if a.rising == b.rising => {
                a.sum(b)
            }
            (BinaryOp::Sub, Shape::Monotone(a), Shape::Constant) if constant(r) => a.plus(-r),
            (BinaryOp::Sub, Shape::Constant, Shape::Monotone(b)) if constant(l) => {
                b.negated().plus(l)
            }
            (BinaryOp::Sub, Shape::Monotone(a), Shape::Monotone(b)) if a.rising != b.rising => {
                a.sum(b.negated())
            }
            (BinaryOp::Mul, Shape::Monotone(a), Shape::Constant) if factor(r) => a.times(r),
            (BinaryOp::Mul, Shape::Constant, Shape::Monotone(b)) if factor(l) => b.times(l),
            (BinaryOp::Div, Shape::Monotone(a), Shape::Constant) if factor(r) => a.over(r),
            (BinaryOp::Gt | BinaryOp::Lt | BinaryOp::Gte | BinaryOp::Lte, ..) => {
                return self.comparison_hold(op, left, right, within, value);
            }
            _ => return point,
        };
        Hold::monotone(within, line)
    }

    /// A comparison of a monotone number with a constant, which holds while the number stays on
    /// its side.
    fn comparison_hold(
        &self,
        op: BinaryOp,
        left: Operand,
        right: Operand,
        within: (f64, f64),
        value: &ExpressionValue,
    ) -> Hold {
        let point = Hold::at(self.ctx.frame);
        // As `x op c`, with `x` the monotone side.
        let (x, line, c, op) = match (left.hold.shape, right.hold.shape) {
            (Shape::Monotone(line), Shape::Constant) => (left.step, line, right.number, op),
            (Shape::Constant, Shape::Monotone(line)) => {
                let mirrored = match op {
                    BinaryOp::Lt => BinaryOp::Gt,
                    BinaryOp::Gt => BinaryOp::Lt,
                    BinaryOp::Lte => BinaryOp::Gte,
                    _ => BinaryOp::Lte,
                };
                (right.step, line, left.number, mirrored)
            }
            _ => return point,
        };
        let (Some(c), ExpressionValue::Boolean(result)) = (c, value) else {
            return point;
        };
        if !c.is_finite() {
            return point;
        }
        // The numbers `x` takes where the comparison gives `result`.
        let band = match (op, result) {
            (BinaryOp::Lt, true) | (BinaryOp::Gte, false) => Band::below(c, false),
            (BinaryOp::Lte, true) | (BinaryOp::Gt, false) => Band::below(c, true),
            (BinaryOp::Gt, true) | (BinaryOp::Lte, false) => Band::above(c, false),
            _ => Band::above(c, true),
        };
        Hold::constant(self.band_extent(within, x, line, band))
    }

    fn builtin_hold(&self, call: &Call<'_>, value: &ExpressionValue) -> Hold {
        let within = call
            .holds
            .iter()
            .fold(EVERY_FRAME, |frames, hold| intersect(frames, hold.frames));
        let constant = call.holds.iter().all(Hold::is_constant);
        match rule(call.builtin) {
            Rule::Pure | Rule::Shaped if constant => Hold::constant(within),
            Rule::ReadsFrame if constant => self.progress_hold(call, within, value),
            Rule::Shaped => self.shaped_hold(call, within, value),
            Rule::Pure | Rule::ReadsFrame | Rule::Measured => Hold::at(self.ctx.frame),
        }
    }

    /// `progress` or `tween` of constant arguments, which hold before the start and after the end.
    fn progress_hold(&self, call: &Call<'_>, within: (f64, f64), value: &ExpressionValue) -> Hold {
        let frame = self.ctx.frame;
        let point = Hold::at(frame);
        let number = |index: usize| call.args[index].as_f64();
        let (start, end) = match call.builtin {
            BuiltinFn::Progress => (number(0), number(1)),
            _ => {
                let start = number(2);
                (
                    start,
                    start
                        .zip(number(3))
                        .map(|(start, duration)| start + duration),
                )
            }
        };
        let (Some(start), Some(end)) = (start, end) else {
            return point;
        };
        if !start.is_finite() || !end.is_finite() {
            return point;
        }
        if end <= start {
            let band = if frame >= start {
                Band::above(start, true)
            } else {
                Band::below(start, false)
            };
            let line = Line::FRAME;
            return Hold::constant(
                self.extent(within, band.ends(line), line, |at| band.contains(at)),
            );
        }
        let span = end - start;
        if !span.is_finite() {
            return point;
        }
        let ratio = progress_ratio(frame, start, end);
        let band = if ratio < 0.0 {
            Band::below(0.0, false)
        } else if ratio > 1.0 {
            Band::above(1.0, false)
        } else {
            return point;
        };
        let line = Line {
            rising: true,
            slope: 1.0 / span,
            offset: -start / span,
        };
        let frames = self.extent(within, band.ends(line), line, |at| {
            band.contains(progress_ratio(at, start, end))
        });
        Hold::constant(self.extended(call.step, value, frames))
    }

    /// A builtin with a monotone argument, which holds while that argument stays in a band, or is
    /// monotone with it.
    fn shaped_hold(&self, call: &Call<'_>, within: (f64, f64), value: &ExpressionValue) -> Hold {
        let point = Hold::at(self.ctx.frame);
        let monotone = |index: usize| match call.holds[index].shape {
            Shape::Monotone(line) => Some((call.steps[index], line)),
            Shape::Constant => None,
        };
        let constant = |index: usize| {
            call.holds[index]
                .is_constant()
                .then(|| call.args[index].as_f64())
                .flatten()
                .filter(|number| number.is_finite())
        };
        match call.builtin {
            BuiltinFn::Keyframes => {
                let (Some((t_step, line)), Some(t)) = (monotone(0), call.args[0].as_f64()) else {
                    return point;
                };
                if !call.holds[1..].iter().all(Hold::is_constant) {
                    return point;
                }
                let Ok((default, keys)) = track(&call.args[1..]) else {
                    return point;
                };
                let band = match last_key_at(t, &keys) {
                    None => Band::below(keys[0].at, false),
                    Some(index) if index + 1 == keys.len() => Band::above(keys[index].at, true),
                    Some(index)
                        if keys[index].ease.unwrap_or(default)
                            == Easing::Named(NamedEasing::Hold) =>
                    {
                        Band {
                            low: Some((keys[index].at, true)),
                            high: Some((keys[index + 1].at, false)),
                        }
                    }
                    Some(_) => return point,
                };
                Hold::constant(self.band_extent(within, t_step, line, band))
            }
            BuiltinFn::Floor | BuiltinFn::Ceil | BuiltinFn::Round => {
                let (Some((x_step, line)), Some(x), ExpressionValue::Number(k)) =
                    (monotone(0), call.args[0].as_f64(), value)
                else {
                    return point;
                };
                let k = *k;
                if !k.is_finite() {
                    return point;
                }
                if k != 0.0 {
                    // Only `k` itself is equal to it. Where the argument would cross into the
                    // next whole number, for guessing:
                    let (low, high) = match call.builtin {
                        BuiltinFn::Floor => (k, k + 1.0),
                        BuiltinFn::Ceil => (k - 1.0, k),
                        _ => (k - 0.5, k + 0.5),
                    };
                    let ends = if line.rising {
                        [Some(low), Some(high)]
                    } else {
                        [Some(high), Some(low)]
                    };
                    return Hold::constant(self.extent(within, ends, line, |at| {
                        self.value_at(call.step, at)
                            .is_some_and(|other| same_bits(&other, value))
                    }));
                }
                // The arguments that give a zero of this sign without being zero themselves.
                let band = match (call.builtin, k.is_sign_negative()) {
                    (BuiltinFn::Floor, false) => Band::open(0.0, 1.0),
                    (BuiltinFn::Ceil, true) => Band::open(-1.0, 0.0),
                    (BuiltinFn::Round, false) => Band::open(0.0, 0.5),
                    (BuiltinFn::Round, true) => Band::open(-0.5, 0.0),
                    _ => return point,
                };
                if !band.contains(x) {
                    return point;
                }
                let frames = self.band_extent(within, x_step, line, band);
                Hold::constant(self.extended(call.step, value, frames))
            }
            BuiltinFn::Min | BuiltinFn::Max => {
                if call.args.len() != 2 {
                    return point;
                }
                let ((x_step, line), x, c) = match (monotone(0), monotone(1)) {
                    (Some(x), None) => (x, call.args[0].as_f64(), constant(1)),
                    (None, Some(x)) => (x, call.args[1].as_f64(), constant(0)),
                    _ => return point,
                };
                let (Some(x), Some(c)) = (x, c) else {
                    return point;
                };
                let beyond = if call.builtin == BuiltinFn::Min {
                    Band::above(c, false)
                } else {
                    Band::below(c, false)
                };
                if !beyond.contains(x) {
                    return Hold::monotone(within, line);
                }
                let frames = self.band_extent(within, x_step, line, beyond);
                Hold::constant(self.extended(call.step, value, frames))
            }
            BuiltinFn::Clamp => {
                let (Some((x_step, line)), Some(x), Some(low), Some(high)) =
                    (monotone(0), call.args[0].as_f64(), constant(1), constant(2))
                else {
                    return point;
                };
                let saturated = if x < low {
                    Band::below(low, false)
                } else if x > high {
                    Band::above(high, false)
                } else {
                    return Hold::monotone(within, line);
                };
                let frames = self.band_extent(within, x_step, line, saturated);
                Hold::constant(self.extended(call.step, value, frames))
            }
            _ => point,
        }
    }

    /// The frames of `within` around the frame on which step `x`, a number moving like `line`,
    /// stays in `band`.
    fn band_extent(&self, within: (f64, f64), x: usize, line: Line, band: Band) -> (f64, f64) {
        self.extent(within, band.ends(line), line, |at| {
            self.number_at(x, at)
                .is_some_and(|number| band.contains(number))
        })
    }

    /// The frames of `within` around the frame on which `holds` is true. It must be true at the
    /// frame, on frames forming an interval, and on every frame of `within` past the frame on a
    /// side whose entry in `ends`, the bound a number moving like `line` meets on that side, is
    /// `None`.
    fn extent(
        &self,
        within: (f64, f64),
        ends: [Option<f64>; 2],
        line: Line,
        mut holds: impl FnMut(f64) -> bool,
    ) -> (f64, f64) {
        if !self.checks {
            return within;
        }
        let frame = self.ctx.frame;
        let mut side = |end: f64, bound: Option<f64>, up: bool| match bound {
            None => end,
            Some(bound) => reach(frame, end, line.crossing(bound), up, &mut holds),
        };
        let low = side(within.0, ends[0], false);
        (low, side(within.1, ends[1], true))
    }

    /// `frames` moved out by one frame at each finite end at which step `index` evaluates to the
    /// bits of `value`.
    fn extended(&self, index: usize, value: &ExpressionValue, frames: (f64, f64)) -> (f64, f64) {
        if !self.checks || !self.plan.steps[index].repeatable {
            return frames;
        }
        let outward = |end: f64, up: bool| {
            if end.is_infinite() {
                return end;
            }
            let next = adjacent(end, up);
            let same = self
                .value_at(index, next)
                .is_some_and(|other| same_bits(&other, value));
            if same { next } else { end }
        };
        (outward(frames.0, false), outward(frames.1, true))
    }

    /// Step `index`, which reads no node and measures no text, evaluated at `frame`.
    fn value_at(&self, index: usize, frame: f64) -> Option<ExpressionValue> {
        debug_assert!(self.plan.steps[index].repeatable);
        let ctx = ExpressionContext { frame, ..*self.ctx };
        self.plan
            .evaluate_step(index, &ctx, &mut Scratch::new(self.plan))
            .ok()
    }

    fn number_at(&self, index: usize, frame: f64) -> Option<f64> {
        match self.value_at(index, frame)? {
            ExpressionValue::Number(number) => Some(number),
            _ => None,
        }
    }
}

fn global_hold(global: GlobalVar, ctx: &ExpressionContext<'_>) -> Hold {
    match global {
        GlobalVar::Fps | GlobalVar::Width | GlobalVar::Height => Hold::ALWAYS,
        GlobalVar::Frame => Hold::monotone(EVERY_FRAME, Line::FRAME),
        GlobalVar::Time if ctx.fps <= 0.0 => Hold::ALWAYS,
        GlobalVar::Time if ctx.fps.is_finite() => Hold::monotone(
            EVERY_FRAME,
            Line {
                rising: true,
                slope: 1.0 / f64::from(ctx.fps),
                offset: 0.0,
            },
        ),
        GlobalVar::Time
        | GlobalVar::Glyph
        | GlobalVar::Word
        | GlobalVar::Line
        | GlobalVar::Span
        | GlobalVar::GlyphCount
        | GlobalVar::WordCount
        | GlobalVar::LineCount
        | GlobalVar::SpanCount
        | GlobalVar::Instance
        | GlobalVar::InstanceCount => Hold::at(ctx.frame),
    }
}

/// The farthest of a few frames from `frame` toward `end`, the first `guess`, at which `holds` is
/// true, or `frame`.
fn reach(frame: f64, end: f64, guess: f64, up: bool, holds: &mut impl FnMut(f64) -> bool) -> f64 {
    let past = |a: f64, b: f64| {
        let order = a.total_cmp(&b);
        if up { order.is_gt() } else { order.is_lt() }
    };
    if guess.is_nan() {
        return frame;
    }
    let guess = if past(guess, end) { end } else { guess };
    if !past(guess, frame) {
        return frame;
    }
    // Just short of the guess, which an exact crossing needs, the last whole frame short of it,
    // which a rounded one may need, and half way.
    let whole = if up {
        guess.ceil() - 1.0
    } else {
        guess.floor() + 1.0
    };
    let candidates = [
        guess,
        adjacent(guess, !up),
        whole,
        frame / 2.0 + guess / 2.0,
    ];
    let mut tried: [Option<u64>; 4] = [None; 4];
    for (index, candidate) in candidates.into_iter().enumerate() {
        let bits = Some(candidate.to_bits());
        if candidate.is_nan()
            || !past(candidate, frame)
            || past(candidate, guess)
            || tried.contains(&bits)
        {
            continue;
        }
        tried[index] = bits;
        if holds(candidate) {
            return candidate;
        }
    }
    frame
}

/// The frame next to `frame` in [`f64::total_cmp`] order, after it when `up`. `frame` is finite.
fn adjacent(frame: f64, up: bool) -> f64 {
    // `total_cmp` orders the bits so mapped as integers.
    let ordered = |bits: i64| bits ^ ((((bits >> 63) as u64) >> 1) as i64);
    let key = ordered(frame.to_bits() as i64);
    f64::from_bits(ordered(if up { key + 1 } else { key - 1 }) as u64)
}

/// The frames both `a` and `b` hold.
pub(crate) fn intersect(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (
        if a.0.total_cmp(&b.0).is_ge() {
            a.0
        } else {
            b.0
        },
        if a.1.total_cmp(&b.1).is_le() {
            a.1
        } else {
            b.1
        },
    )
}

/// Whether `frames` holds `frame`.
pub(crate) fn contains(frames: (f64, f64), frame: f64) -> bool {
    frames.0.total_cmp(&frame).is_le() && frame.total_cmp(&frames.1).is_le()
}

/// Whether `a` and `b` are the same bits, for the values a step that saturates can take.
fn same_bits(a: &ExpressionValue, b: &ExpressionValue) -> bool {
    let bits = |a: &[f64], b: &[f64]| a.iter().zip(b).all(|(a, b)| a.to_bits() == b.to_bits());
    match (a, b) {
        (ExpressionValue::Number(a), ExpressionValue::Number(b)) => a.to_bits() == b.to_bits(),
        (ExpressionValue::Boolean(a), ExpressionValue::Boolean(b)) => a == b,
        (ExpressionValue::String(a), ExpressionValue::String(b)) => a == b,
        (ExpressionValue::Vec2((ax, ay)), ExpressionValue::Vec2((bx, by))) => {
            bits(&[*ax, *ay], &[*bx, *by])
        }
        (ExpressionValue::Vec3(a), ExpressionValue::Vec3(b)) => bits(a, b),
        (ExpressionValue::Color(a), ExpressionValue::Color(b)) => bits(a, b),
        _ => false,
    }
}

#[cfg(test)]
#[path = "hold_tests.rs"]
mod tests;
