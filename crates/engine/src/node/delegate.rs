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
}

pub trait DelegateEvaluable: Clone {
    type Evaluated;

    fn eval(&self, ctx: &DelegateEvalContext<'_>) -> crate::Result<Self::Evaluated>;
}

pub trait DelegateValue: DelegateEvaluable {
    fn to_property_value(&self) -> PropertyValue;
    fn to_property_expression(&self) -> PropertyExpression {
        PropertyExpression::Value(self.to_property_value())
    }
    fn from_property_expression(value: PropertyExpression) -> crate::Result<Self>
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

    fn property_defs() -> Vec<PropertyDef>;
    fn is_property(id: &str) -> bool;
    fn default_properties(&self) -> Vec<(&'static str, PropertyValue)>;
    fn get_property(&self, id: &str) -> Option<PropertyExpression>;
    fn eval(&self, ctx: &NodeParamEvalContext<'_>) -> crate::Result<Self::Evaluated>;

    #[cfg(feature = "json")]
    fn from_json(
        params: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> anyhow::Result<Self>
    where
        Self: Sized,
        Self: serde::de::DeserializeOwned,
    {
        let value = params
            .cloned()
            .map(serde_json::Value::Object)
            .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
        serde_path_to_error::deserialize(value).map_err(|error| anyhow::anyhow!(error.to_string()))
    }
}

#[derive(Debug, Clone, Default)]
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
