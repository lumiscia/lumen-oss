use std::borrow::Cow;

use anyhow::anyhow;
use serde_json::{Map, Value};

use crate::expr::{Expression, ExpressionValue};

use super::{ids::NodeId, schema::PropertyDef};

/// What `node(id, 'param')` finds in a param.
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyRead<'a> {
    Expr(&'a Expression),
    /// A literal's value, or `None` for a kind expressions have no value for, such as a paint.
    Value(Option<ExpressionValue>),
}

pub struct DelegateEvalContext<'a> {
    pub node_id: NodeId,
    pub expr: &'a crate::expr::ExpressionContext<'a>,
}

impl DelegateEvalContext<'_> {
    /// Evaluates `field` of the value being evaluated, naming it in the path of the failure.
    pub fn eval_field<D: DelegateEvaluable>(
        &self,
        field: &str,
        value: &D,
    ) -> crate::Result<D::Evaluated> {
        value.eval(self).map_err(|error| error.in_field(field))
    }
}

/// The form a param takes in a node: evaluated each frame, parsed from JSON and read back as JSON
/// or by `node(id, 'param')`.
pub trait DelegateEvaluable: Clone {
    type Evaluated;

    /// Whether an `=expression` may stand for the whole value.
    const ACCEPTS_EXPRESSIONS: bool;

    /// Whether JSON `null` is a value of its own, as for `Override` fields.
    const NULLABLE: bool = false;

    fn eval(&self, ctx: &DelegateEvalContext<'_>) -> crate::Result<Self::Evaluated>;

    /// Whether any part of this value is an expression.
    fn has_expressions(&self) -> bool;

    /// Parses the value as written in JSON. Errors name `path`.
    fn from_json_value(value: &Value, path: &str) -> Result<Self, String>;

    /// The JSON that parses to this value, or `None` for an unset value.
    fn to_json_value(&self) -> Option<Value>;

    /// What `node(id, 'param')` reads, or `None` for an unset value.
    fn read(&self) -> Option<PropertyRead<'_>>;
}

pub trait Delegated: Clone + Default + Sized {
    type Delegate: Clone;

    fn into_delegate(self) -> Self::Delegate;
}

pub trait NodeParams: DelegateEvaluable + Default {
    /// Whether any field is a `Vec`, which the node schema cannot describe.
    const HAS_VEC_FIELDS: bool = false;

    fn property_defs() -> Vec<PropertyDef>;
    fn is_property(id: &str) -> bool;
    /// The JSON of each param that is set. Applied to the defaults, this is what the schema
    /// documents.
    fn json_properties(&self) -> Vec<(&'static str, Value)>;
    fn get_property(&self, id: &str) -> Option<PropertyRead<'_>>;
    /// Parses the fields present in `fields`, with errors naming them under `path`.
    fn from_json_fields(fields: &Map<String, Value>, path: &str) -> Result<Self, String>;

    fn from_json(node_id: NodeId, params: Option<&Map<String, Value>>) -> anyhow::Result<Self> {
        params.map_or_else(
            || Ok(Self::default()),
            |params| {
                Self::from_json_fields(params, "")
                    .map_err(|error| anyhow!("node {node_id}: {error}"))
            },
        )
    }
}

/// `path.field`, or just `field` for a node's own params.
pub fn field_path<'a>(path: &str, field: &'a str) -> Cow<'a, str> {
    if path.is_empty() {
        Cow::Borrowed(field)
    } else {
        Cow::Owned(format!("{path}.{field}"))
    }
}

/// `value` as an object with every `required` field and no field outside `known`.
pub fn nested_fields<'a>(
    value: &'a Value,
    path: &str,
    known: &[&str],
    required: &[&str],
) -> Result<&'a Map<String, Value>, String> {
    crate::json::reject_expression(value, path)?;
    let fields = value
        .as_object()
        .ok_or_else(|| format!("`{path}` expected an object, got {value}"))?;
    if let Some(field) = required.iter().find(|field| !fields.contains_key(**field)) {
        return Err(format!("`{path}`: missing field `{field}`"));
    }
    if let Some(key) = fields.keys().find(|key| !known.contains(&key.as_str())) {
        let known = known
            .iter()
            .map(|field| format!("`{field}`"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "`{path}`: unknown field `{key}`, expected one of {known}"
        ));
    }
    Ok(fields)
}

impl<D: DelegateEvaluable> DelegateEvaluable for Vec<D> {
    type Evaluated = Vec<D::Evaluated>;

    const ACCEPTS_EXPRESSIONS: bool = false;

    fn eval(&self, ctx: &DelegateEvalContext<'_>) -> crate::Result<Self::Evaluated> {
        self.iter()
            .enumerate()
            .map(|(index, value)| value.eval(ctx).map_err(|error| error.in_index(index)))
            .collect()
    }

    fn has_expressions(&self) -> bool {
        self.iter().any(DelegateEvaluable::has_expressions)
    }

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        value
            .as_array()
            .ok_or_else(|| format!("`{path}` expected an array, got {value}"))?
            .iter()
            .enumerate()
            .map(|(index, item)| D::from_json_value(item, &format!("{path}[{index}]")))
            .collect()
    }

    fn to_json_value(&self) -> Option<Value> {
        Some(Value::Array(
            self.iter()
                .filter_map(DelegateEvaluable::to_json_value)
                .collect(),
        ))
    }

    fn read(&self) -> Option<PropertyRead<'_>> {
        None
    }
}

impl<T> Delegated for Vec<T>
where
    T: Delegated,
{
    type Delegate = Vec<T::Delegate>;

    fn into_delegate(self) -> Self::Delegate {
        self.into_iter().map(Delegated::into_delegate).collect()
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

    const ACCEPTS_EXPRESSIONS: bool = D::ACCEPTS_EXPRESSIONS;
    const NULLABLE: bool = true;

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

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        if value.is_null() {
            return Ok(Self::Clear);
        }
        D::from_json_value(value, path).map(Self::Set)
    }

    fn to_json_value(&self) -> Option<Value> {
        match self {
            Self::Inherit => None,
            Self::Clear => Some(Value::Null),
            Self::Set(value) => value.to_json_value(),
        }
    }

    fn read(&self) -> Option<PropertyRead<'_>> {
        match self {
            Self::Set(value) => value.read(),
            Self::Inherit | Self::Clear => None,
        }
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

    const ACCEPTS_EXPRESSIONS: bool = D::ACCEPTS_EXPRESSIONS;

    fn eval(&self, ctx: &DelegateEvalContext<'_>) -> crate::Result<Self::Evaluated> {
        self.as_ref().map(|value| value.eval(ctx)).transpose()
    }

    fn has_expressions(&self) -> bool {
        self.as_ref()
            .is_some_and(DelegateEvaluable::has_expressions)
    }

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        D::from_json_value(value, path).map(Some)
    }

    fn to_json_value(&self) -> Option<Value> {
        self.as_ref().and_then(DelegateEvaluable::to_json_value)
    }

    fn read(&self) -> Option<PropertyRead<'_>> {
        self.as_ref().and_then(DelegateEvaluable::read)
    }
}

#[cfg(test)]
mod tests {
    #[derive(Debug, Clone, Default, PartialEq, lumen_macros::Delegate)]
    pub struct Span {
        #[meta(required)]
        pub text: String,
        #[meta(min = 1, step = 1)]
        pub size: Option<f64>,
    }

    #[test]
    fn a_required_field_is_rejected_when_missing() {
        use crate::node::DelegateEvaluable;

        let error =
            SpanDelegate::from_json_value(&serde_json::json!({ "size": 12 }), "span").unwrap_err();
        assert_eq!(error, "`span`: missing field `text`");
    }
}
