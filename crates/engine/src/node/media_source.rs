//! Params that name media: declared with `#[media(...)]` on a node's param struct, and read from
//! a node by [`Node::for_each_media_source`](super::Node::for_each_media_source).

use crate::{
    expr::{Expression, ExpressionValue},
    media::AssetKind,
};

use super::{Deferred, DelegateEvaluable, NodeEnum, PropertyRead};

/// What `#[media(...)]` declares about a param: which kinds of media it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaParam {
    pub param: &'static str,
    /// Every kind the param accepts, in the order the declaration lists them.
    pub kinds: &'static [AssetKind],
    /// The enum param, of the same node, whose options are named after `kinds` and pick the one
    /// the source reads. While it is an expression, any of `kinds` may be read.
    pub selector: Option<&'static str>,
}

/// A media param of a node, with what the node holds in it.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaSource<'a> {
    pub param: &'static str,
    /// The kinds the source reads: the declared ones, or the one its selector picks.
    pub kinds: Vec<AssetKind>,
    pub value: MediaValue<'a>,
}

/// What a media param holds.
#[derive(Debug, Clone, PartialEq)]
pub enum MediaValue<'a> {
    /// A source as written: a path, a URL or an alias, or `asset:<id>`. Empty when unset.
    Literal(&'a str),
    /// An expression, whose source is known only when it is evaluated.
    Expression(&'a Expression),
}

/// A param type that can name media.
pub trait MediaSourceField {
    fn media_value(&self) -> MediaValue<'_>;
}

impl MediaSourceField for Deferred<String> {
    fn media_value(&self) -> MediaValue<'_> {
        match self {
            Self::Value(source) => MediaValue::Literal(source),
            Self::Expr(expression) => MediaValue::Expression(expression),
        }
    }
}

impl MediaSourceField for String {
    fn media_value(&self) -> MediaValue<'_> {
        MediaValue::Literal(self)
    }
}

impl<'a> MediaSource<'a> {
    /// The source `field` holds for `param`, read as the `selected` kind or else as any of the
    /// declared ones.
    pub fn new<F: MediaSourceField>(
        param: &MediaParam,
        field: &'a F,
        selected: Option<AssetKind>,
    ) -> Self {
        Self {
            param: param.param,
            kinds: selected.map_or_else(|| param.kinds.to_vec(), |kind| vec![kind]),
            value: field.media_value(),
        }
    }
}

/// The one of `kinds` that the literal enum param `selector` names, or `None` while it is an
/// expression or names none of them.
pub fn selected_media_kind<S>(selector: &S, kinds: &[AssetKind]) -> Option<AssetKind>
where
    S: DelegateEvaluable,
    S::Evaluated: NodeEnum,
{
    let Some(PropertyRead::Value(Some(ExpressionValue::Number(value)))) = selector.read() else {
        return None;
    };
    let option = <S::Evaluated as NodeEnum>::enum_def()
        .options
        .iter()
        .find(|option| option.value as f64 == value)?;
    kinds
        .iter()
        .copied()
        .find(|kind| kind.name() == option.name)
}
