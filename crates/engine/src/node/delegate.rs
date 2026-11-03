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

    /// Calls `f` with every expression in this value, however deeply it is held.
    fn for_each_expression(&self, f: &mut dyn FnMut(&Expression));

    /// Whether any part of this value is an expression.
    fn has_expressions(&self) -> bool {
        let mut any = false;
        self.for_each_expression(&mut |_| any = true);
        any
    }

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
    /// The id of each param, in declaration order.
    fn field_ids() -> Vec<&'static str>;
    /// The ids of the params a nested object must set.
    fn required_ids() -> Vec<&'static str>;
    /// Sets the params present in `fields`, with errors naming them under `path`. Others keep
    /// their value, so a group flattened into params keeps the defaults those params give it.
    fn set_json_fields(&mut self, fields: &Map<String, Value>, path: &str) -> Result<(), String>;

    /// Parses the fields present in `fields` over the defaults, with errors naming them under
    /// `path`.
    fn from_json_fields(fields: &Map<String, Value>, path: &str) -> Result<Self, String> {
        let mut params = Self::default();
        params.set_json_fields(fields, path)?;
        Ok(params)
    }

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

/// Panics when two of `defs`, the params of `params` and the groups flattened into it, share an
/// id, since JSON could only ever set one of them.
pub fn assert_unique_params(defs: &[PropertyDef], params: &str) {
    for (index, def) in defs.iter().enumerate() {
        assert!(
            !defs[..index].iter().any(|other| other.id == def.id),
            "{params} declares the param `{}` twice",
            def.id
        );
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

    fn for_each_expression(&self, f: &mut dyn FnMut(&Expression)) {
        for value in self {
            value.for_each_expression(f);
        }
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

    fn for_each_expression(&self, f: &mut dyn FnMut(&Expression)) {
        if let Self::Set(value) = self {
            value.for_each_expression(f);
        }
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

    fn for_each_expression(&self, f: &mut dyn FnMut(&Expression)) {
        if let Some(value) = self {
            value.for_each_expression(f);
        }
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

    #[derive(Debug, Clone, PartialEq, lumen_macros::Delegate)]
    pub struct Group {
        /// The group's docs.
        #[meta(required)]
        pub size: f64,
        #[meta()]
        pub label: Option<String>,
    }

    impl Default for Group {
        fn default() -> Self {
            Self {
                size: 1.0,
                label: None,
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, lumen_macros::Delegate)]
    pub struct Flat {
        #[meta()]
        pub before: f64,
        #[meta(flatten, doc(size = "The parent's docs."))]
        pub group: Group,
    }

    impl Default for Flat {
        fn default() -> Self {
            Self {
                before: 0.0,
                group: Group {
                    size: 2.0,
                    label: None,
                },
            }
        }
    }

    #[test]
    fn flattened_params_are_the_parents_own() {
        use crate::expr::{Expression, ExpressionContext, ExpressionValue};
        use crate::node::{
            Deferred, DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams, PropertyRead,
        };
        use serde_json::json;

        let ids = FlatDelegate::property_defs()
            .iter()
            .map(|def| def.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, ["before", "size", "label"]);
        assert_eq!(FlatDelegate::field_ids(), ids);
        assert_eq!(FlatDelegate::required_ids(), ["size"]);
        assert!(FlatDelegate::is_property("label"));
        assert!(!FlatDelegate::is_property("group"));

        // Fields the JSON leaves out keep the parent's defaults, not the group's.
        let parsed =
            FlatDelegate::from_json_fields(json!({ "label": "=frame" }).as_object().unwrap(), "")
                .unwrap();
        assert!(matches!(parsed.group.size, Deferred::Value(2.0)));
        assert!(matches!(
            parsed.get_property("size"),
            Some(PropertyRead::Value(Some(ExpressionValue::Number(2.0))))
        ));
        assert!(matches!(
            parsed.get_property("label"),
            Some(PropertyRead::Expr(_))
        ));
        assert_eq!(parsed.get_property("group"), None);
        assert_eq!(
            parsed.to_json_value().unwrap(),
            json!({ "before": 0.0, "size": 2.0, "label": "=frame" })
        );

        // A nested object is checked against the group's fields too.
        let parse = |json| FlatDelegate::from_json_value(&json, "flat").unwrap_err();
        assert_eq!(parse(json!({})), "`flat`: missing field `size`");
        assert_eq!(
            parse(json!({ "size": 1, "group": {} })),
            "`flat`: unknown field `group`, expected one of `before`, `size`, `label`"
        );
        assert_eq!(
            parse(json!({ "size": "big" })),
            "`flat.size` expected float"
        );

        // A group adds no segment to where a failure is located.
        let failing = FlatDelegate {
            group: GroupDelegate {
                size: Deferred::Expr(Expression::parse("vec2(1, vec2(2, 3))").unwrap()),
                label: None,
            },
            ..FlatDelegate::default()
        };
        let error = DelegateEvalContext {
            node_id: NodeId::new(3),
            expr: &ExpressionContext::default(),
        }
        .eval_field("flat", &failing)
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "node 3 `flat.size`: vec2 expects numeric arguments"
        );
    }

    #[derive(Debug, Clone, Default, lumen_macros::Delegate)]
    pub struct Colliding {
        #[meta()]
        pub size: f64,
        #[meta(flatten)]
        pub group: Group,
    }

    #[test]
    #[should_panic(expected = "declares the param `size` twice")]
    fn a_param_declared_by_a_group_and_its_parent_is_rejected() {
        <CollidingDelegate as crate::node::NodeParams>::property_defs();
    }
}
