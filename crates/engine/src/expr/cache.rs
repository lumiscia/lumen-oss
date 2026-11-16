//! Values of outermost evaluations kept across frames while they hold.

use std::{cell::RefCell, fmt, sync::Arc};

use foldhash::HashMap;

use crate::{
    composition::SceneId,
    expr::{
        ExpressionContext,
        ast::{Expression, ExpressionValue},
        eval::EvalPlan,
        hold::contains,
    },
    scope::PropValues,
};

/// A scope instance (its graph and the path that reaches it) and an expression's plan, by address.
type HeldKey = (SceneId, u32, usize);

/// What outermost evaluations found, by scope instance and expression, with the frames on which
/// each value holds. It is only borrowed for a lookup or a store, never across an evaluation.
#[derive(Debug, Default)]
pub(crate) struct HeldCache {
    entries: RefCell<HashMap<HeldKey, Kept>>,
    tick: u64,
}

#[derive(Debug)]
struct Kept {
    /// The plan whose address keys this, kept so that no other plan takes the address.
    _plan: Arc<EvalPlan>,
    /// One per frame range and props the expression was asked at, most recent last.
    held: Vec<Held>,
}

#[derive(Debug)]
struct Held {
    props: Option<Arc<PropValues>>,
    frames: (f64, f64),
    value: ExpressionValue,
    /// Whether the value measured text, in which case it is kept until the next frame starts.
    measured: bool,
    last_used: u64,
}

impl HeldCache {
    /// The handle contexts of one scope instance carry, whose props are `props`.
    pub(crate) fn scope<'a>(
        &'a self,
        scene: SceneId,
        path: u32,
        props: Option<&'a Arc<PropValues>>,
    ) -> HeldValues<'a> {
        HeldValues {
            cache: self,
            scope: (scene, path),
            props,
        }
    }

    /// Starts a frame, dropping what the last two frames didn't use, as animated props are new on
    /// every frame and a value they held for goes with them, and every value that measured text,
    /// as the fonts it measured with may have changed.
    pub(crate) fn begin_frame(&mut self) {
        self.tick += 1;
        let tick = self.tick;
        self.entries.get_mut().retain(|_, kept| {
            kept.held
                .retain(|held| !held.measured && held.last_used + 2 >= tick);
            !kept.held.is_empty()
        });
    }

    pub(crate) fn clear(&mut self) {
        self.entries.get_mut().clear();
    }
}

/// Where the outermost evaluations in the contexts of one scope instance keep their values while
/// they hold. The graph, timeline and canvas of every such context are the scope's.
#[derive(Clone, Copy)]
pub struct HeldValues<'a> {
    cache: &'a HeldCache,
    scope: (SceneId, u32),
    /// The scope's props, which a context's `props` borrows.
    props: Option<&'a Arc<PropValues>>,
}

impl fmt::Debug for HeldValues<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HeldValues")
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

impl HeldValues<'_> {
    /// `expression` in `ctx`: the value it was found to hold on `ctx.frame` with the same props, or
    /// else evaluated and kept with the frames it holds on. An expression that reads no node and
    /// measures no text, and last held for a single frame, is evaluated plainly and kept for its
    /// own frame, unless that gives its last value again. Errors are not kept.
    pub(super) fn evaluate(
        self,
        expression: &Expression,
        ctx: &ExpressionContext<'_>,
    ) -> crate::Result<ExpressionValue> {
        let key = (
            self.scope.0,
            self.scope.1,
            Arc::as_ptr(&expression.plan) as usize,
        );
        let tick = self.cache.tick;
        let shared = self
            .props
            .filter(|props| ctx.props.is_some_and(|read| std::ptr::eq(&***props, read)));
        let mut changed_last = None;
        if let Some(kept) = self.cache.entries.borrow_mut().get_mut(&key) {
            let same = |held: &Held| same_props(held.props.as_deref(), ctx.props);
            if let Some(held) = kept
                .held
                .iter_mut()
                .rev()
                .find(|held| contains(held.frames, ctx.frame) && same(held))
            {
                held.last_used = tick;
                // The props found equal are taken as they are now, so the next read finds them by address.
                if let Some(props) = shared
                    && !held
                        .props
                        .as_ref()
                        .is_some_and(|held| Arc::ptr_eq(held, props))
                {
                    held.props = Some(Arc::clone(props));
                }
                return Ok(held.value.clone());
            }
            changed_last = kept
                .held
                .iter()
                .rev()
                .find(|held| held.frames.0 == held.frames.1 && same(held))
                .map(|held| held.value.clone());
        }
        let (value, frames, measured) = match changed_last {
            Some(last) if expression.plan.steps[expression.plan.roots[0]].repeatable => {
                let value = expression.evaluate_now(ctx)?;
                if same_value(&value, &last) {
                    let found = expression.evaluate_held(ctx)?;
                    (found.value, found.frames, found.measured)
                } else {
                    (value, (ctx.frame, ctx.frame), false)
                }
            }
            _ => {
                let found = expression.evaluate_held(ctx)?;
                (found.value, found.frames, found.measured)
            }
        };
        let props = match shared {
            Some(props) => Some(Arc::clone(props)),
            None => ctx.props.map(|props| Arc::new(props.clone())),
        };
        self.cache
            .entries
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| Kept {
                _plan: Arc::clone(&expression.plan),
                held: Vec::new(),
            })
            .held
            .push(Held {
                props,
                frames,
                value: value.clone(),
                measured,
                last_used: tick,
            });
        Ok(value)
    }
}

/// Whether expressions read the same props from `a` and `b`, by [`same_value`].
fn same_props(a: Option<&PropValues>, b: Option<&PropValues>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            std::ptr::eq(a, b)
                || (a.len() == b.len()
                    && a.iter()
                        .zip(b)
                        .all(|((a_name, a), (b_name, b))| a_name == b_name && same_value(a, b)))
        }
        _ => false,
    }
}

/// Whether `a` and `b` are the same value, which tells `0` from `-0` and holds for two NaNs of the
/// same bits, as the same inputs give the same output.
pub(crate) fn same_value(a: &ExpressionValue, b: &ExpressionValue) -> bool {
    fn same_bits(a: &[f64], b: &[f64]) -> bool {
        a.iter().zip(b).all(|(a, b)| a.to_bits() == b.to_bits())
    }
    match (a, b) {
        (ExpressionValue::Number(a), ExpressionValue::Number(b)) => a.to_bits() == b.to_bits(),
        (ExpressionValue::Vec2((ax, ay)), ExpressionValue::Vec2((bx, by))) => {
            same_bits(&[*ax, *ay], &[*bx, *by])
        }
        (ExpressionValue::Vec3(a), ExpressionValue::Vec3(b)) => same_bits(a, b),
        (ExpressionValue::Color(a), ExpressionValue::Color(b)) => same_bits(a, b),
        (a, b) => a == b,
    }
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
