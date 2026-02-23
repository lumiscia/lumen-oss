mod base;
mod shape;
mod text;

use std::marker::PhantomData;

use crate::expr::{Expression, ExpressionId, ExpressionScope, ExpressionValue};

pub use base::{
    BaseStyle, Mask, MaskShape, MaskSource, PathCommand, ResolvedBaseStyle, ResolvedMask,
    ResolvedMaskShape, ResolvedMaskSource, ResolvedShadowStyle, ShadowStyle, TransformStyle,
};
pub use shape::{EllipseStyle, Fill, PolygonStyle, RectStyle, Stroke};
pub use text::{
    ResolvedTextPlaceholder, TextAlign, TextDecoration, TextOverflow, TextStyle, VerticalAlign,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct StyleContext<'a> {
    pub frame: u32,
    pub scope: Option<&'a ExpressionScope>,
}

impl<'a> StyleContext<'a> {
    pub fn new(frame: u32) -> Self {
        Self { frame, scope: None }
    }

    pub fn with_scope(frame: u32, scope: &'a ExpressionScope) -> Self {
        Self {
            frame,
            scope: Some(scope),
        }
    }
}

pub trait Interpolate: Clone {
    fn lerp(&self, other: &Self, t: f32) -> Self;
}

impl Interpolate for f32 {
    fn lerp(&self, other: &Self, t: f32) -> Self {
        self + (other - self) * t
    }
}

impl Interpolate for u8 {
    fn lerp(&self, other: &Self, t: f32) -> Self {
        (*self as f32 + (*other as f32 - *self as f32) * t)
            .round()
            .clamp(0.0, u8::MAX as f32) as u8
    }
}

impl Interpolate for u32 {
    fn lerp(&self, other: &Self, t: f32) -> Self {
        (*self as f32 + (*other as f32 - *self as f32) * t)
            .round()
            .clamp(0.0, u32::MAX as f32) as u32
    }
}

impl Interpolate for bool {
    fn lerp(&self, other: &Self, t: f32) -> Self {
        if t < 0.5 { *self } else { *other }
    }
}

pub trait FromExpressionValue: Sized {
    fn from_expression_value(value: ExpressionValue) -> Option<Self>;
}

impl FromExpressionValue for f32 {
    fn from_expression_value(value: ExpressionValue) -> Option<Self> {
        match value {
            ExpressionValue::Number(value) => Some(value),
            ExpressionValue::Boolean(_) | ExpressionValue::String(_) => None,
        }
    }
}

impl FromExpressionValue for u8 {
    fn from_expression_value(value: ExpressionValue) -> Option<Self> {
        match value {
            ExpressionValue::Number(value) => {
                if !value.is_finite() {
                    return None;
                }
                Some(value.round().clamp(0.0, u8::MAX as f32) as u8)
            }
            ExpressionValue::Boolean(_) | ExpressionValue::String(_) => None,
        }
    }
}

impl FromExpressionValue for u32 {
    fn from_expression_value(value: ExpressionValue) -> Option<Self> {
        match value {
            ExpressionValue::Number(value) => {
                if !value.is_finite() {
                    return None;
                }
                Some(value.round().clamp(0.0, u32::MAX as f32) as u32)
            }
            ExpressionValue::Boolean(_) | ExpressionValue::String(_) => None,
        }
    }
}

impl FromExpressionValue for bool {
    fn from_expression_value(value: ExpressionValue) -> Option<Self> {
        match value {
            ExpressionValue::Boolean(value) => Some(value),
            ExpressionValue::Number(value) => Some(value != 0.0),
            ExpressionValue::String(_) => None,
        }
    }
}

impl FromExpressionValue for String {
    fn from_expression_value(value: ExpressionValue) -> Option<Self> {
        match value {
            ExpressionValue::String(value) => Some(value),
            ExpressionValue::Number(_) | ExpressionValue::Boolean(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Easing {
    #[default]
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

impl Easing {
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::EaseIn => t * t,
            Self::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
            Self::EaseInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - ((-2.0 * t + 2.0).powi(2) / 2.0)
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sequence<T>(pub Vec<Keyframe<T>>);

impl<T> Sequence<T> {
    pub fn new(mut keyframes: Vec<Keyframe<T>>) -> Self {
        keyframes.sort_by_key(|keyframe| keyframe.frame);
        Self(keyframes)
    }

    pub fn keyframes(&self) -> &[Keyframe<T>] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Keyframe<T> {
    pub frame: u32,
    pub value: StyleValue<T>,
    pub easing: Easing,
}

impl<T> Keyframe<T> {
    pub fn new(frame: u32, value: StyleValue<T>) -> Self {
        Self {
            frame,
            value,
            easing: Easing::Linear,
        }
    }

    pub fn with_easing(mut self, easing: Easing) -> Self {
        self.easing = easing;
        self
    }
}

#[derive(Debug, Clone)]
pub struct StyleExpression<T> {
    pub expr: String,
    parsed: Option<Expression>,
    pub value_type: PhantomData<T>,
}

impl<T> PartialEq for StyleExpression<T> {
    fn eq(&self, other: &Self) -> bool {
        self.expr == other.expr
    }
}

impl<T> StyleExpression<T> {
    pub fn new(expr: impl Into<String>) -> Self {
        let expr = expr.into();
        let parsed = Expression::parse(ExpressionId("style".to_owned()), expr.as_str()).ok();
        Self {
            expr,
            parsed,
            value_type: PhantomData,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StyleValue<T> {
    Literal(T),
    Expression(StyleExpression<T>),
}

impl<T> StyleValue<T>
where
    T: Clone + FromExpressionValue,
{
    fn resolve(&self, ctx: &StyleContext<'_>) -> Option<T> {
        match self {
            Self::Literal(value) => Some(value.clone()),
            Self::Expression(expr) => {
                let parsed = expr.parsed.as_ref()?;
                let default_scope = ExpressionScope::default();
                let scope = ctx.scope.unwrap_or(&default_scope);
                let value = parsed.evaluate(scope).ok()?;
                T::from_expression_value(value)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StyleProperty<T> {
    Value(StyleValue<T>),
    Sequence(Sequence<T>),
}

impl<T> StyleProperty<T>
where
    T: Clone + Interpolate + FromExpressionValue,
{
    pub fn resolve(&self, ctx: &StyleContext<'_>) -> Option<T> {
        match self {
            Self::Value(value) => value.resolve(ctx),
            Self::Sequence(sequence) => sequence.resolve(ctx),
        }
    }

    pub fn resolve_or(&self, ctx: &StyleContext<'_>, fallback: T) -> T {
        self.resolve(ctx).unwrap_or(fallback)
    }
}

impl<T> Sequence<T>
where
    T: Clone + Interpolate + FromExpressionValue,
{
    fn resolve(&self, ctx: &StyleContext<'_>) -> Option<T> {
        let keyframes = &self.0;
        if keyframes.is_empty() {
            return None;
        }

        if let Some(exact) = keyframes
            .iter()
            .rev()
            .find(|keyframe| keyframe.frame == ctx.frame)
        {
            return exact.value.resolve(ctx);
        }

        let first = keyframes.first()?;
        if ctx.frame < first.frame {
            return first.value.resolve(ctx);
        }

        let last = keyframes.last()?;
        if ctx.frame > last.frame {
            return last.value.resolve(ctx);
        }

        for pair in keyframes.windows(2) {
            let [start, end] = pair else {
                continue;
            };
            if !(start.frame < ctx.frame && ctx.frame < end.frame) {
                continue;
            }

            let start_value = start.value.resolve(ctx);
            let end_value = end.value.resolve(ctx);
            return match (start_value, end_value) {
                (Some(start_value), Some(end_value)) => {
                    let total = (end.frame - start.frame) as f32;
                    if total <= 0.0 {
                        Some(end_value)
                    } else {
                        let t = (ctx.frame - start.frame) as f32 / total;
                        Some(start_value.lerp(&end_value, start.easing.apply(t)))
                    }
                }
                (Some(start_value), None) => Some(start_value),
                (None, Some(end_value)) => Some(end_value),
                (None, None) => None,
            };
        }

        last.value.resolve(ctx)
    }
}

impl<T> Default for StyleValue<T>
where
    T: Default,
{
    fn default() -> Self {
        Self::Literal(Default::default())
    }
}

impl<T> Default for StyleProperty<T>
where
    T: Default,
{
    fn default() -> Self {
        Self::Value(Default::default())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Easing, Keyframe, Sequence, StyleContext, StyleExpression, StyleProperty, StyleValue,
    };
    use crate::expr::{ExpressionProperty, ExpressionScope, ExpressionValue};

    #[test]
    fn resolves_literal_style_value() {
        let property = StyleProperty::Value(StyleValue::Literal(12.0f32));
        let ctx = StyleContext::new(0);

        assert_eq!(property.resolve(&ctx), Some(12.0));
    }

    #[test]
    fn resolves_sequence_with_interpolation_and_edge_frames() {
        let property = StyleProperty::Sequence(Sequence::new(vec![
            Keyframe::new(10, StyleValue::Literal(0.0f32)),
            Keyframe::new(20, StyleValue::Literal(100.0f32)),
        ]));

        assert_eq!(property.resolve(&StyleContext::new(0)), Some(0.0));
        assert_eq!(property.resolve(&StyleContext::new(10)), Some(0.0));
        assert_eq!(property.resolve(&StyleContext::new(15)), Some(50.0));
        assert_eq!(property.resolve(&StyleContext::new(20)), Some(100.0));
        assert_eq!(property.resolve(&StyleContext::new(30)), Some(100.0));
    }

    #[test]
    fn applies_easing_to_interpolation() {
        let property = StyleProperty::Sequence(Sequence::new(vec![
            Keyframe::new(0, StyleValue::Literal(0.0f32)).with_easing(Easing::EaseIn),
            Keyframe::new(10, StyleValue::Literal(10.0f32)),
        ]));

        assert_eq!(property.resolve(&StyleContext::new(5)), Some(2.5));
    }

    #[test]
    fn interpolates_bool_as_step() {
        let property = StyleProperty::Sequence(Sequence::new(vec![
            Keyframe::new(0, StyleValue::Literal(false)),
            Keyframe::new(10, StyleValue::Literal(true)),
        ]));

        assert_eq!(property.resolve(&StyleContext::new(4)), Some(false));
        assert_eq!(property.resolve(&StyleContext::new(6)), Some(true));
    }

    #[test]
    fn resolves_style_expression_with_scope() {
        let mut scope = ExpressionScope::default();
        scope.clip_properties.insert(
            ("hero".to_owned(), ExpressionProperty::Width),
            ExpressionValue::Number(320.0),
        );
        let property = StyleProperty::Value(StyleValue::Expression(StyleExpression::new(
            "clip('hero').width * 0.5",
        )));
        let ctx = StyleContext::with_scope(12, &scope);

        assert_eq!(property.resolve(&ctx), Some(160.0));
    }

    #[test]
    fn resolves_constant_expression_without_scope() {
        let property = StyleProperty::Value(StyleValue::Expression(StyleExpression::<f32>::new(
            "lerp(10, 30, 0.25)",
        )));
        let ctx = StyleContext::new(0);

        assert_eq!(property.resolve(&ctx), Some(15.0));
    }
    #[test]
    fn falls_back_when_expression_scope_is_missing() {
        let property = StyleProperty::Value(StyleValue::Expression(StyleExpression::<f32>::new(
            "clip('hero').width",
        )));
        let ctx = StyleContext::new(0);

        assert_eq!(property.resolve(&ctx), None);
        assert_eq!(property.resolve_or(&ctx, 42.0), 42.0);
    }
}
