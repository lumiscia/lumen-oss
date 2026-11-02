use crate::error::{ExpressionError, LumenError, PropertyError};

use super::{
    ids::NodeId,
    property::{PropertyExpression, PropertyValue},
    schema::PropertyDef,
};

pub struct DelegateEvalContext<'a> {
    pub node_id: NodeId,
    pub property_path: &'a str,
    pub expr: &'a crate::expr::ExpressionContext<'a>,
}

impl<'a> DelegateEvalContext<'a> {
    pub fn child(&'a self, property_path: &'a str) -> Self {
        Self {
            node_id: self.node_id,
            property_path,
            expr: self.expr,
        }
    }
    /// Evaluates `field` of the value at `property_path`, naming it in the path of the error a
    /// failure carries, such as `content[1].font_size`. The path is built only on failure.
    pub fn eval_field<D: DelegateEvaluable>(
        &self,
        field: &str,
        value: &D,
    ) -> crate::Result<D::Evaluated> {
        value.eval(self).map_err(|error| match error {
            LumenError::Property(PropertyError::InvalidType {
                node_id,
                property_path,
                expected,
                actual,
            }) => LumenError::Property(PropertyError::InvalidType {
                node_id,
                property_path: path_in_field(&property_path, self.property_path, field),
                expected,
                actual,
            }),
            LumenError::Expression(ExpressionError::Evaluate { path, details }) => {
                LumenError::Expression(ExpressionError::Evaluate {
                    path: self.expression_path_in_field(path, field),
                    details,
                })
            }
            LumenError::Expression(ExpressionError::UndefinedVariable { path, name }) => {
                LumenError::Expression(ExpressionError::UndefinedVariable {
                    path: self.expression_path_in_field(path, field),
                    name,
                })
            }
            error => error,
        })
    }

    /// An expression error's path starts with the path of the expression context, which names
    /// the node, and continues with the path within the param.
    fn expression_path_in_field(&self, path: Option<String>, field: &str) -> Option<String> {
        let base = self.expr.path.as_deref().unwrap_or_default();
        let within = path
            .as_deref()
            .unwrap_or_default()
            .strip_prefix(base)?
            .trim_start_matches('.');
        let within = path_in_field(within, self.property_path, field);
        Some(if base.is_empty() {
            within
        } else {
            format!("{base}.{within}")
        })
    }
}

/// `path`, a path within the value at `parent` or `parent` itself, with `field` after `parent`.
/// A `path` outside `parent` is returned as it is.
fn path_in_field(path: &str, parent: &str, field: &str) -> String {
    let rest = if parent.is_empty() {
        path
    } else if path.is_empty() || path == parent {
        ""
    } else if let Some(rest) = path
        .strip_prefix(parent)
        .and_then(|rest| rest.strip_prefix('.'))
    {
        rest
    } else {
        return path.to_string();
    };
    [parent, field, rest]
        .into_iter()
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join(".")
}

pub trait DelegateEvaluable: Clone {
    type Evaluated;

    fn eval(&self, ctx: &DelegateEvalContext<'_>) -> crate::Result<Self::Evaluated>;

    /// Whether any part of this value is an expression.
    fn has_expressions(&self) -> bool;
}

pub trait DelegateValue: DelegateEvaluable {
    /// Whether `from_property_expression` and evaluation accept `PropertyExpression::Expr`.
    const ACCEPTS_EXPRESSIONS: bool;

    fn to_property_value(&self) -> PropertyValue;
    fn to_property_expression(&self) -> PropertyExpression {
        PropertyExpression::Value(self.to_property_value())
    }
    fn from_property_expression(
        value: PropertyExpression,
        node_id: NodeId,
        property_path: &str,
    ) -> crate::Result<Self>
    where
        Self: Sized;
}

pub trait Delegated: Clone + Default + Sized {
    type Delegate: Clone;

    fn into_delegate(self) -> Self::Delegate;
}

pub struct NodeParamEvalContext<'a> {
    pub node_id: NodeId,
    pub expr: &'a crate::expr::ExpressionContext<'a>,
}

pub trait NodeParams: Clone + Default {
    type Evaluated;

    /// Whether any field is a `Vec`, which the node schema cannot describe.
    const HAS_VEC_FIELDS: bool = false;

    fn property_defs() -> Vec<PropertyDef>;
    fn is_property(id: &str) -> bool;
    fn default_properties(&self) -> Vec<(&'static str, PropertyValue)>;
    fn get_property(&self, id: &str) -> Option<PropertyExpression>;
    fn eval(&self, ctx: &NodeParamEvalContext<'_>) -> crate::Result<Self::Evaluated>;

    #[cfg(feature = "json")]
    fn from_json(
        node_id: NodeId,
        params: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> anyhow::Result<Self>
    where
        Self: Sized;
}

#[derive(Debug, Clone, Default, lumen_macros::StructuralHash)]
pub struct DelegateVec<T: Delegated> {
    pub items: Vec<T::Delegate>,
}

#[cfg(feature = "json")]
impl<'de, T> serde::Deserialize<'de> for DelegateVec<T>
where
    T: Delegated,
    T::Delegate: serde::Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        <Vec<T::Delegate> as serde::Deserialize>::deserialize(deserializer)
            .map(|items| Self { items })
    }
}

impl<T> DelegateEvaluable for DelegateVec<T>
where
    T: Delegated,
    T::Delegate: DelegateEvaluable<Evaluated = T>,
{
    type Evaluated = Vec<T>;

    fn eval(&self, ctx: &DelegateEvalContext<'_>) -> crate::Result<Self::Evaluated> {
        self.items
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let path = format!("{}[{index}]", ctx.property_path);
                value.eval(&ctx.child(&path))
            })
            .collect()
    }

    fn has_expressions(&self) -> bool {
        self.items.iter().any(DelegateEvaluable::has_expressions)
    }
}

impl<T> Delegated for Vec<T>
where
    T: Delegated,
{
    type Delegate = DelegateVec<T>;

    fn into_delegate(self) -> Self::Delegate {
        DelegateVec {
            items: self.into_iter().map(Delegated::into_delegate).collect(),
        }
    }
}

/// A field that inherits its parent's value when absent, is switched off by JSON `null`, or is
/// set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, lumen_macros::StructuralHash)]
pub enum Override<T> {
    #[default]
    Inherit,
    Clear,
    Set(T),
}

impl<T> Override<T> {
    /// The value in effect given the parent's `inherited` one.
    pub fn resolve(self, inherited: Option<T>) -> Option<T> {
        match self {
            Self::Inherit => inherited,
            Self::Clear => None,
            Self::Set(value) => Some(value),
        }
    }
}

impl<T: Delegated> Delegated for Override<T> {
    type Delegate = Override<T::Delegate>;

    fn into_delegate(self) -> Self::Delegate {
        match self {
            Self::Inherit => Override::Inherit,
            Self::Clear => Override::Clear,
            Self::Set(value) => Override::Set(value.into_delegate()),
        }
    }
}

impl<D: DelegateEvaluable> DelegateEvaluable for Override<D> {
    type Evaluated = Override<D::Evaluated>;

    fn eval(&self, ctx: &DelegateEvalContext<'_>) -> crate::Result<Self::Evaluated> {
        Ok(match self {
            Self::Inherit => Override::Inherit,
            Self::Clear => Override::Clear,
            Self::Set(value) => Override::Set(value.eval(ctx)?),
        })
    }

    fn has_expressions(&self) -> bool {
        matches!(self, Self::Set(value) if value.has_expressions())
    }
}

impl<T: Delegated> Delegated for Option<T> {
    type Delegate = Option<T::Delegate>;

    fn into_delegate(self) -> Self::Delegate {
        self.map(Delegated::into_delegate)
    }
}

impl<D: DelegateEvaluable> DelegateEvaluable for Option<D> {
    type Evaluated = Option<D::Evaluated>;

    fn eval(&self, ctx: &DelegateEvalContext<'_>) -> crate::Result<Self::Evaluated> {
        self.as_ref().map(|value| value.eval(ctx)).transpose()
    }

    fn has_expressions(&self) -> bool {
        self.as_ref()
            .is_some_and(DelegateEvaluable::has_expressions)
    }
}

/// A delegate field that can be unset: `Option` when absent, `Override` also when `null`.
pub trait OptionalDelegate: Sized {
    type Inner: DelegateValue;

    /// Whether JSON `null` sets [`Override::Clear`].
    const NULLABLE: bool;

    fn from_override(value: Override<Self::Inner>) -> Self;
    fn as_override(&self) -> Override<&Self::Inner>;
}

impl<D: DelegateValue> OptionalDelegate for Option<D> {
    type Inner = D;

    const NULLABLE: bool = false;

    fn from_override(value: Override<D>) -> Self {
        value.resolve(None)
    }

    fn as_override(&self) -> Override<&D> {
        self.as_ref().map_or(Override::Inherit, Override::Set)
    }
}

impl<D: DelegateValue> OptionalDelegate for Override<D> {
    type Inner = D;

    const NULLABLE: bool = true;

    fn from_override(value: Override<D>) -> Self {
        value
    }

    fn as_override(&self) -> Override<&D> {
        match self {
            Self::Inherit => Override::Inherit,
            Self::Clear => Override::Clear,
            Self::Set(value) => Override::Set(value),
        }
    }
}
