mod base;
mod shape;
mod text;

use std::marker::PhantomData;

pub use base::BaseStyle;
pub use text::TextStyle;

#[derive(Debug, Clone)]
pub struct Sequence<T>(Vec<Keyframe<T>>);

#[derive(Debug, Clone)]
pub struct Keyframe<T> {
    frame: u32,
    value: StyleValue<T>,
}

#[derive(Debug, Clone)]
pub struct StyleExpression<T> {
    pub expr: String,
    pub value_type: PhantomData<T>,
}

#[derive(Debug, Clone)]
pub enum StyleValue<T> {
    Literal(T),
    Expression(StyleExpression<T>),
}

#[derive(Debug, Clone)]
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
        Self::Literal(Default::default())
    }
}

type OptionalStyleValue<T> = Option<StyleValue<T>>;
