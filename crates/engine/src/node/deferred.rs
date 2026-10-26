use crate::expr::Expression;

use super::{
    delegate::{DelegateEvalContext, DelegateEvaluable, DelegateValue, Delegated},
    ids::NodeId,
    property::{PropertyExpression, PropertyValue},
    shader_uniforms::ShaderUniforms,
    vector,
};

#[derive(Debug, Clone)]
pub enum Deferred<T> {
    Value(T),
    Expr(Expression),
}

impl<T> Deferred<T> {
    pub fn value(value: T) -> Self {
        Self::Value(value)
    }

    pub fn eval(
        &self,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<T>
    where
        T: DeferredValue,
    {
        T::eval_deferred(self, node_id, property_path, ctx)
    }

    pub fn to_property_value(&self) -> PropertyValue
    where
        T: DeferredValue,
    {
        match self {
            Self::Value(value) => T::to_property_value(value),
            Self::Expr(_) => {
                panic!(
                    "Deferred::to_property_value called on expression; use to_property_expression"
                )
            }
        }
    }

    pub fn to_property_expression(&self) -> PropertyExpression
    where
        T: DeferredValue,
    {
        match self {
            Self::Value(value) => PropertyExpression::Value(T::to_property_value(value)),
            Self::Expr(expr) => PropertyExpression::Expr(expr.clone()),
        }
    }

    pub fn from_property_expression(
        value: PropertyExpression,
        node_id: NodeId,
        property_path: &str,
    ) -> crate::Result<Self>
    where
        T: DeferredValue,
    {
        match value {
            PropertyExpression::Value(value) => T::from_property_value(value)
                .map(Self::Value)
                .ok_or_else(|| {
                    PropertyValue::invalid_type(
                        node_id,
                        property_path,
                        T::property_kind_name(),
                        "incompatible value",
                    )
                }),
            PropertyExpression::Expr(expr) if T::ACCEPTS_EXPRESSIONS => Ok(Self::Expr(expr)),
            PropertyExpression::Expr(_) => Err(PropertyValue::invalid_type(
                node_id,
                property_path,
                T::property_kind_name(),
                "expression",
            )),
        }
    }
}

impl<T> From<T> for Deferred<T> {
    fn from(value: T) -> Self {
        Self::Value(value)
    }
}

impl Deferred<f64> {
    pub fn resolve_float(
        &self,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<f64> {
        self.eval(node_id, property_path, ctx)
    }
}

impl Deferred<i64> {
    pub fn resolve_int(
        &self,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<i64> {
        self.eval(node_id, property_path, ctx)
    }

    pub fn resolve_float(
        &self,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<f64> {
        self.eval(node_id, property_path, ctx)
            .map(|value| value as f64)
    }
}

impl Deferred<bool> {
    pub fn resolve_bool(
        &self,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<bool> {
        self.eval(node_id, property_path, ctx)
    }
}

impl Deferred<String> {
    pub fn resolve_string(
        &self,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<String> {
        self.eval(node_id, property_path, ctx)
    }
}

impl Deferred<[u8; 4]> {
    pub fn resolve_color(
        &self,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<[u8; 4]> {
        self.eval(node_id, property_path, ctx)
    }
}

impl Deferred<vector::paint::Paint> {
    pub fn resolve_paint(
        &self,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<vector::paint::Paint> {
        self.eval(node_id, property_path, ctx)
    }
}

impl Deferred<(f64, f64)> {
    pub fn resolve_vec2(
        &self,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<(f64, f64)> {
        self.eval(node_id, property_path, ctx)
    }
}

#[cfg(feature = "json")]
impl<'de, T> serde::Deserialize<'de> for Deferred<T>
where
    T: DeferredJsonValue + DeferredValue,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use crate::json::{StringParam, classify_string};

        let mut value = serde_json::Value::deserialize(deserializer)?;
        match value.as_str().and_then(classify_string) {
            Some(StringParam::Expression(_)) if !T::ACCEPTS_EXPRESSIONS => {
                return Err(serde::de::Error::custom(format!(
                    "{} does not support expressions",
                    T::property_kind_name()
                )));
            }
            Some(StringParam::Expression(source)) => {
                return Expression::parse(source)
                    .map(Self::Expr)
                    .map_err(serde::de::Error::custom);
            }
            Some(StringParam::EscapedLiteral(text)) => value = serde_json::Value::from(text),
            None => {}
        }
        T::from_json_value(&value)
            .map(Self::Value)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(feature = "json")]
pub trait DeferredJsonValue: Sized {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, String>;
}

pub trait DeferredValue: Clone {
    /// Whether `eval_deferred` can resolve a `Deferred::Expr` to this type.
    const ACCEPTS_EXPRESSIONS: bool;

    fn eval_deferred(
        deferred: &Deferred<Self>,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<Self>
    where
        Self: Sized;

    fn to_property_value(value: &Self) -> PropertyValue;
    fn from_property_value(value: PropertyValue) -> Option<Self>
    where
        Self: Sized;
    fn property_kind_name() -> &'static str;
}

impl<T> DelegateEvaluable for Deferred<T>
where
    T: DeferredValue,
{
    type Evaluated = T;

    fn eval(&self, ctx: &DelegateEvalContext<'_>) -> crate::Result<Self::Evaluated> {
        Deferred::eval(self, ctx.node_id, ctx.property_path, ctx.expr)
    }
}

impl<T> DelegateValue for Deferred<T>
where
    T: DeferredValue,
{
    const ACCEPTS_EXPRESSIONS: bool = T::ACCEPTS_EXPRESSIONS;

    fn to_property_value(&self) -> PropertyValue {
        Deferred::to_property_value(self)
    }

    fn to_property_expression(&self) -> PropertyExpression {
        Deferred::to_property_expression(self)
    }

    fn from_property_expression(
        value: PropertyExpression,
        node_id: NodeId,
        property_path: &str,
    ) -> crate::Result<Self> {
        Deferred::from_property_expression(value, node_id, property_path)
    }
}

macro_rules! impl_delegated_deferred {
    ($($ty:ty),* $(,)?) => {
        $(
            impl Delegated for $ty {
                type Delegate = Deferred<$ty>;

                fn into_delegate(self) -> Self::Delegate {
                    Deferred::value(self)
                }
            }
        )*
    };
}

impl_delegated_deferred!(
    f64,
    f32,
    i64,
    u32,
    u8,
    bool,
    String,
    [u8; 4],
    (f64, f64),
    [f32; 2],
    ShaderUniforms
);

macro_rules! impl_deferred_float_expr {
    ($ty:ty, $conv:expr) => {
        impl DeferredValue for $ty {
            const ACCEPTS_EXPRESSIONS: bool = true;

            fn eval_deferred(
                deferred: &Deferred<Self>,
                node_id: NodeId,
                property_path: &str,
                ctx: &crate::expr::ExpressionContext<'_>,
            ) -> crate::Result<Self> {
                match deferred {
                    Deferred::Value(value) => Ok(*value),
                    Deferred::Expr(expr) => {
                        expr.evaluate(ctx)?.as_f64().map($conv).ok_or_else(|| {
                            PropertyValue::invalid_type(
                                node_id,
                                property_path,
                                "Float",
                                "expression",
                            )
                        })
                    }
                }
            }

            fn to_property_value(value: &Self) -> PropertyValue {
                PropertyValue::Float(f64::from(*value))
            }

            fn from_property_value(value: PropertyValue) -> Option<Self> {
                value.coerce_float().map($conv)
            }

            fn property_kind_name() -> &'static str {
                "Float"
            }
        }
    };
}

impl_deferred_float_expr!(f64, |value: f64| value);

impl DeferredValue for f32 {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn eval_deferred(
        deferred: &Deferred<Self>,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<Self> {
        match deferred {
            Deferred::Value(value) => Ok(*value),
            Deferred::Expr(expr) => expr
                .evaluate(ctx)?
                .as_f64()
                .map(|value| value as f32)
                .ok_or_else(|| {
                    PropertyValue::invalid_type(node_id, property_path, "Float", "expression")
                }),
        }
    }

    fn to_property_value(value: &Self) -> PropertyValue {
        PropertyValue::Float(f64::from(*value))
    }

    fn from_property_value(value: PropertyValue) -> Option<Self> {
        value.coerce_float().map(|value| value as f32)
    }

    fn property_kind_name() -> &'static str {
        "Float"
    }
}

impl DeferredValue for i64 {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn eval_deferred(
        deferred: &Deferred<Self>,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<Self> {
        match deferred {
            Deferred::Value(value) => Ok(*value),
            Deferred::Expr(expr) => {
                let value = expr.evaluate(ctx)?.as_f64().ok_or_else(|| {
                    PropertyValue::invalid_type(node_id, property_path, "Int", "expression")
                })?;
                Ok(value as i64)
            }
        }
    }

    fn to_property_value(value: &Self) -> PropertyValue {
        PropertyValue::Int(*value)
    }

    fn from_property_value(value: PropertyValue) -> Option<Self> {
        value.coerce_int()
    }

    fn property_kind_name() -> &'static str {
        "Int"
    }
}

impl DeferredValue for u32 {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn eval_deferred(
        deferred: &Deferred<Self>,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<Self> {
        let value = match deferred {
            Deferred::Value(value) => i64::from(*value),
            Deferred::Expr(expr) => expr.evaluate(ctx)?.as_f64().ok_or_else(|| {
                PropertyValue::invalid_type(node_id, property_path, "UInt", "expression")
            })? as i64,
        };
        u32::try_from(value).map_err(|_| {
            PropertyValue::invalid_type(node_id, property_path, "UInt", "out-of-range value")
        })
    }

    fn to_property_value(value: &Self) -> PropertyValue {
        PropertyValue::Int(i64::from(*value))
    }

    fn from_property_value(value: PropertyValue) -> Option<Self> {
        value
            .coerce_int()
            .and_then(|value| u32::try_from(value).ok())
    }

    fn property_kind_name() -> &'static str {
        "UInt"
    }
}

impl DeferredValue for u8 {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn eval_deferred(
        deferred: &Deferred<Self>,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<Self> {
        let value = match deferred {
            Deferred::Value(value) => i64::from(*value),
            Deferred::Expr(expr) => expr.evaluate(ctx)?.as_f64().ok_or_else(|| {
                PropertyValue::invalid_type(node_id, property_path, "Int", "expression")
            })? as i64,
        };
        u8::try_from(value).map_err(|_| {
            PropertyValue::invalid_type(node_id, property_path, "U8", "out-of-range value")
        })
    }

    fn to_property_value(value: &Self) -> PropertyValue {
        PropertyValue::Int(i64::from(*value))
    }

    fn from_property_value(value: PropertyValue) -> Option<Self> {
        value
            .coerce_int()
            .and_then(|value| u8::try_from(value).ok())
    }

    fn property_kind_name() -> &'static str {
        "U8"
    }
}

impl DeferredValue for bool {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn eval_deferred(
        deferred: &Deferred<Self>,
        _node_id: NodeId,
        _property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<Self> {
        match deferred {
            Deferred::Value(value) => Ok(*value),
            Deferred::Expr(expr) => Ok(expr.evaluate(ctx)?.as_bool()),
        }
    }

    fn to_property_value(value: &Self) -> PropertyValue {
        PropertyValue::Bool(*value)
    }

    fn from_property_value(value: PropertyValue) -> Option<Self> {
        value.coerce_bool()
    }

    fn property_kind_name() -> &'static str {
        "Bool"
    }
}

impl DeferredValue for String {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn eval_deferred(
        deferred: &Deferred<Self>,
        _node_id: NodeId,
        _property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<Self> {
        match deferred {
            Deferred::Value(value) => Ok(value.clone()),
            Deferred::Expr(expr) => Ok(expr.evaluate(ctx)?.as_string()),
        }
    }

    fn to_property_value(value: &Self) -> PropertyValue {
        PropertyValue::String(value.clone())
    }

    fn from_property_value(value: PropertyValue) -> Option<Self> {
        value.coerce_string()
    }

    fn property_kind_name() -> &'static str {
        "String"
    }
}

fn eval_color(
    expr: &Expression,
    node_id: NodeId,
    property_path: &str,
    expected: &'static str,
    ctx: &crate::expr::ExpressionContext<'_>,
) -> crate::Result<[u8; 4]> {
    expr.evaluate(ctx)?
        .as_color_u8()
        .ok_or_else(|| PropertyValue::invalid_type(node_id, property_path, expected, "expression"))
}

impl DeferredValue for [u8; 4] {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn eval_deferred(
        deferred: &Deferred<Self>,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<Self> {
        match deferred {
            Deferred::Value(value) => Ok(*value),
            Deferred::Expr(expr) => eval_color(expr, node_id, property_path, "Color", ctx),
        }
    }

    fn to_property_value(value: &Self) -> PropertyValue {
        PropertyValue::Color(*value)
    }

    fn from_property_value(value: PropertyValue) -> Option<Self> {
        match value {
            PropertyValue::Color(value) => Some(value),
            _ => None,
        }
    }

    fn property_kind_name() -> &'static str {
        "Color"
    }
}

impl DeferredValue for vector::paint::Paint {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn eval_deferred(
        deferred: &Deferred<Self>,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<Self> {
        match deferred {
            Deferred::Value(value) => Ok(value.clone()),
            Deferred::Expr(expr) => eval_color(expr, node_id, property_path, "Paint", ctx)
                .map(vector::paint::Paint::solid),
        }
    }

    fn to_property_value(value: &Self) -> PropertyValue {
        PropertyValue::Paint(value.clone())
    }

    fn from_property_value(value: PropertyValue) -> Option<Self> {
        match value {
            PropertyValue::Paint(value) => Some(value),
            PropertyValue::Color(value) => Some(vector::paint::Paint::solid(value)),
            _ => None,
        }
    }

    fn property_kind_name() -> &'static str {
        "Paint"
    }
}

impl DeferredValue for ShaderUniforms {
    const ACCEPTS_EXPRESSIONS: bool = false;

    fn eval_deferred(
        deferred: &Deferred<Self>,
        node_id: NodeId,
        property_path: &str,
        _ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<Self> {
        match deferred {
            Deferred::Value(value) => Ok(value.clone()),
            Deferred::Expr(_) => Err(PropertyValue::invalid_type(
                node_id,
                property_path,
                "ShaderUniforms",
                "expression",
            )),
        }
    }

    fn to_property_value(value: &Self) -> PropertyValue {
        PropertyValue::ShaderUniforms(value.clone())
    }

    fn from_property_value(value: PropertyValue) -> Option<Self> {
        match value {
            PropertyValue::ShaderUniforms(value) => Some(value),
            _ => None,
        }
    }

    fn property_kind_name() -> &'static str {
        "ShaderUniforms"
    }
}

impl DeferredValue for (f64, f64) {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn eval_deferred(
        deferred: &Deferred<Self>,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<Self> {
        match deferred {
            Deferred::Value(value) => Ok(*value),
            Deferred::Expr(expr) => expr.evaluate(ctx)?.as_vec2().ok_or_else(|| {
                PropertyValue::invalid_type(node_id, property_path, "Vec2", "expression")
            }),
        }
    }

    fn to_property_value(value: &Self) -> PropertyValue {
        PropertyValue::Vec2(*value)
    }

    fn from_property_value(value: PropertyValue) -> Option<Self> {
        match value {
            PropertyValue::Vec2(value) => Some(value),
            _ => None,
        }
    }

    fn property_kind_name() -> &'static str {
        "Vec2"
    }
}

impl DeferredValue for [f32; 2] {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn eval_deferred(
        deferred: &Deferred<Self>,
        node_id: NodeId,
        property_path: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<Self> {
        match deferred {
            Deferred::Value(value) => Ok(*value),
            Deferred::Expr(expr) => expr
                .evaluate(ctx)?
                .as_vec2()
                .map(|(x, y)| [x as f32, y as f32])
                .ok_or_else(|| {
                    PropertyValue::invalid_type(node_id, property_path, "Vec2", "expression")
                }),
        }
    }

    fn to_property_value(value: &Self) -> PropertyValue {
        PropertyValue::Vec2((f64::from(value[0]), f64::from(value[1])))
    }

    fn from_property_value(value: PropertyValue) -> Option<Self> {
        match value {
            PropertyValue::Vec2((x, y)) => Some([x as f32, y as f32]),
            _ => None,
        }
    }

    fn property_kind_name() -> &'static str {
        "Vec2"
    }
}

#[cfg(feature = "json")]
impl DeferredJsonValue for f64 {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, String> {
        value
            .as_f64()
            .or_else(|| value.as_i64().map(|value| value as f64))
            .ok_or_else(|| "expected float".to_string())
    }
}

#[cfg(feature = "json")]
impl DeferredJsonValue for f32 {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, String> {
        f64::from_json_value(value).map(|value| value as f32)
    }
}

#[cfg(feature = "json")]
impl DeferredJsonValue for i64 {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, String> {
        crate::json::integral_i64(value)
            .ok_or_else(|| "expected an integer within the i64 range".to_string())
    }
}

#[cfg(feature = "json")]
impl DeferredJsonValue for u8 {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, String> {
        let value = i64::from_json_value(value)?;
        u8::try_from(value).map_err(|_| "expected u8".to_string())
    }
}

#[cfg(feature = "json")]
impl DeferredJsonValue for u32 {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, String> {
        let value = i64::from_json_value(value)?;
        u32::try_from(value).map_err(|_| "expected u32".to_string())
    }
}

#[cfg(feature = "json")]
impl DeferredJsonValue for bool {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, String> {
        value
            .as_bool()
            .or_else(|| value.as_i64().map(|value| value != 0))
            .ok_or_else(|| "expected bool".to_string())
    }
}

#[cfg(feature = "json")]
impl DeferredJsonValue for String {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, String> {
        value
            .as_str()
            .map(ToString::to_string)
            .ok_or_else(|| "expected string".to_string())
    }
}

#[cfg(feature = "json")]
impl DeferredJsonValue for [u8; 4] {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, String> {
        crate::json::try_parse_color(value).map_err(|error| format!("expected color: {error}"))
    }
}

#[cfg(feature = "json")]
impl DeferredJsonValue for vector::paint::Paint {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, String> {
        vector::paint::Paint::from_json_value(value)
    }
}

#[cfg(feature = "json")]
impl DeferredJsonValue for ShaderUniforms {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, String> {
        ShaderUniforms::from_json_value(value, "uniforms")
    }
}

#[cfg(feature = "json")]
impl DeferredJsonValue for (f64, f64) {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, String> {
        let values = value
            .as_array()
            .ok_or_else(|| "expected vec2".to_string())?;
        if values.len() != 2 {
            return Err(format!(
                "expected vec2, got array of length {}",
                values.len()
            ));
        }
        let x = values[0]
            .as_f64()
            .ok_or_else(|| "expected vec2 x number".to_string())?;
        let y = values[1]
            .as_f64()
            .ok_or_else(|| "expected vec2 y number".to_string())?;
        Ok((x, y))
    }
}

#[cfg(feature = "json")]
impl DeferredJsonValue for [f32; 2] {
    fn from_json_value(value: &serde_json::Value) -> Result<Self, String> {
        let (x, y) = <(f64, f64)>::from_json_value(value)?;
        Ok([x as f32, y as f32])
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        error::{LumenError, PropertyError},
        expr::{Expression, ExpressionContext},
        node::{Deferred, NodeId, vector::paint::Paint},
    };

    fn context() -> ExpressionContext<'static> {
        ExpressionContext {
            frame: 12.0,
            fps: 24.0,
            width: 1920,
            height: 1080,
            duration_frames: 240,
            path: Some("4.position".to_string()),
            graph: None,
            media: None,
        }
    }

    #[test]
    fn evaluates_vec2_expression_for_f64_and_f32_targets() {
        let expression = Expression::parse("vec2(frame / 2, time + 0.25)").unwrap();

        assert_eq!(
            Deferred::<(f64, f64)>::Expr(expression.clone())
                .eval(NodeId::new(4), "position", &context())
                .unwrap(),
            (6.0, 0.75)
        );
        assert_eq!(
            Deferred::<[f32; 2]>::Expr(expression)
                .eval(NodeId::new(4), "position", &context())
                .unwrap(),
            [6.0, 0.75]
        );
    }

    #[test]
    fn vec2_targets_reject_scalar_results_with_property_context() {
        for (error, expected_path) in [
            (
                Deferred::<(f64, f64)>::Expr(Expression::parse("42").unwrap())
                    .eval(NodeId::new(4), "position", &context())
                    .unwrap_err(),
                "position",
            ),
            (
                Deferred::<[f32; 2]>::Expr(Expression::parse("42").unwrap())
                    .eval(NodeId::new(4), "anchor", &context())
                    .unwrap_err(),
                "anchor",
            ),
        ] {
            let LumenError::Property(PropertyError::InvalidType {
                node_id,
                property_path,
                expected,
                actual,
            }) = error
            else {
                panic!("expected property type error");
            };
            assert_eq!(node_id, NodeId::new(4));
            assert_eq!(property_path, expected_path);
            assert_eq!(expected, "Vec2");
            assert_eq!(actual, "expression");
        }
    }

    #[test]
    fn vec2_expression_errors_preserve_expression_path() {
        let error = Deferred::<(f64, f64)>::Expr(Expression::parse("vec2(1, vec2(2, 3))").unwrap())
            .eval(NodeId::new(4), "position", &context())
            .unwrap_err();

        let LumenError::Expression(crate::error::ExpressionError::Evaluate { path, details }) =
            error
        else {
            panic!("expected expression evaluation error");
        };
        assert_eq!(path.as_deref(), Some("4.position"));
        assert_eq!(details, "vec2 expects numeric arguments");
    }

    #[test]
    fn color_and_paint_evaluate_colour_expressions() {
        let expression = Expression::parse("tween(#ff000000, #0000ff, 0, 24)").unwrap();
        assert_eq!(
            Deferred::<[u8; 4]>::Expr(expression.clone())
                .eval(NodeId::new(3), "color", &context())
                .unwrap(),
            [0, 0, 255, 128]
        );
        assert_eq!(
            Deferred::<Paint>::Expr(expression)
                .eval(NodeId::new(3), "fill_paint", &context())
                .unwrap(),
            Paint::solid([0, 0, 255, 128])
        );

        let error = Deferred::<Paint>::Expr(Expression::parse("frame").unwrap())
            .eval(NodeId::new(3), "fill_paint", &context())
            .unwrap_err();
        assert!(
            error.to_string().contains("expected Paint, got expression"),
            "{error}"
        );
    }

    #[test]
    fn paint_delegate_evaluates_colour_expressions() {
        use crate::node::{
            DelegateEvalContext, DelegateEvaluable, DelegateValue, PropertyExpression,
            vector::paint::PaintDelegate,
        };

        let expression = Expression::parse("tween(#ff0000, #0000ff, 0, 24)").unwrap();
        let delegate = PaintDelegate::from_property_expression(
            PropertyExpression::Expr(expression),
            NodeId::new(3),
            "fill_paint",
        )
        .unwrap();
        assert!(matches!(
            delegate.to_property_expression(),
            PropertyExpression::Expr(_)
        ));
        let ctx = DelegateEvalContext {
            node_id: NodeId::new(3),
            property_path: "fill_paint",
            expr: &context(),
        };
        assert_eq!(
            delegate.eval(&ctx).unwrap(),
            Paint::solid([128, 0, 128, 255])
        );
    }

    #[cfg(feature = "json")]
    #[test]
    fn color_and_paint_accept_expressions_when_deserialized() {
        let color = serde_json::from_value::<Deferred<[u8; 4]>>("=rgb(255, 0, 0)".into()).unwrap();
        assert!(matches!(color, Deferred::Expr(_)));
        let paint = serde_json::from_value::<Deferred<Paint>>("=#ff0000".into()).unwrap();
        assert!(matches!(paint, Deferred::Expr(_)));
        let literal = serde_json::from_value::<Deferred<Paint>>("#ff0000".into()).unwrap();
        assert!(matches!(literal, Deferred::Value(_)));
    }

    #[cfg(feature = "json")]
    #[test]
    fn parse_errors_name_the_node_and_property() {
        let json = r#"{
            "timeline": { "fps": 30, "duration_frames": 1 },
            "render_settings": { "width": 100, "height": 100 },
            "nodes": [{ "id": 7, "type": "background", "params": { "width": -1 } }],
            "connections": []
        }"#;

        let error = crate::json::parse(json).unwrap_err().to_string();
        assert!(
            error.contains("property `width` on node 7 has invalid type: expected UInt"),
            "{error}"
        );

        let json = json
            .replace("background", "text")
            .replace(r#""width": -1"#, r#""position": [0, "bad"]"#);
        let error = format!("{:#}", crate::json::parse(&json).unwrap_err());
        assert!(
            error.contains("node 7") && error.contains("position[1]"),
            "{error}"
        );
    }

    #[cfg(feature = "json")]
    #[test]
    fn deserializes_leading_equals_as_expression_or_escaped_literal() {
        for (input, expected) in [("==wow", "=wow"), ("===", "==")] {
            let deferred: Deferred<String> = serde_json::from_value(input.into()).unwrap();
            assert!(matches!(deferred, Deferred::Value(ref s) if s == expected));
        }
        let deferred: Deferred<String> = serde_json::from_value("=frame".into()).unwrap();
        assert!(matches!(deferred, Deferred::Expr(_)));
    }
}
