mod base;
mod shape;
mod text;

use std::marker::PhantomData;

pub use base::{
    BaseStyle, ResolvedBaseStyle, ResolvedShadowStyle, ShadowStyle, TransformStyle,
    resolve_base_style,
};
pub use shape::{EllipseStyle, PolygonStyle, RectStyle};
pub use text::TextStyle;

#[derive(Debug, Clone, PartialEq)]
pub struct Sequence<T>(Vec<Keyframe<T>>);

#[derive(Debug, Clone, PartialEq)]
pub struct Keyframe<T> {
    frame: u32,
    value: StyleValue<T>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StyleExpression<T> {
    pub expr: String,
    pub value_type: PhantomData<T>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StyleValue<T> {
    Literal(T),
    Expression(StyleExpression<T>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum StyleProperty<T> {
    Value(StyleValue<T>),
    Sequence(Sequence<T>),
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

pub fn resolve_style_value<T: Clone>(property: &StyleProperty<T>) -> Option<T> {
    match property {
        StyleProperty::Value(StyleValue::Literal(value)) => Some(value.clone()),
        StyleProperty::Value(StyleValue::Expression(_)) => None,
        StyleProperty::Sequence(Sequence(keyframes)) => {
            keyframes
                .iter()
                .rev()
                .find_map(|keyframe| match &keyframe.value {
                    StyleValue::Literal(value) => Some(value.clone()),
                    StyleValue::Expression(_) => None,
                })
        }
    }
}

pub fn resolve_style_value_or<T: Clone>(property: &StyleProperty<T>, fallback: T) -> T {
    resolve_style_value(property).unwrap_or(fallback)
}
