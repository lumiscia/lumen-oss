use serde_json::Value;

use crate::{
    error::LumenError,
    expr::{Expression, ExpressionValue},
    json::{StringParam, classify_string, integer_mismatch, integral_i64, try_parse_color},
};

use super::{
    clip_props::ClipProps,
    delegate::{DelegateEvalContext, DelegateEvaluable, Delegated, PropertyRead},
    ids::NodeId,
    schema::EnumDef,
    shader_uniforms::ShaderUniforms,
    vector,
};

#[derive(Debug, Clone, lumen_macros::StructuralHash)]
pub enum Deferred<T> {
    Value(T),
    Expr(Expression),
}

impl<T> Deferred<T> {
    pub fn value(value: T) -> Self {
        Self::Value(value)
    }

    pub fn is_expr(&self) -> bool {
        matches!(self, Self::Expr(_))
    }

    /// This value at `ctx`. An expression's failure is located at `field` of node `node_id`.
    pub fn eval(
        &self,
        node_id: NodeId,
        field: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<T>
    where
        T: ParamValue,
    {
        match self {
            Self::Value(value) => Ok(value.clone()),
            Self::Expr(expr) => expr
                .evaluate(ctx)
                .and_then(T::from_expression)
                .map_err(|error| error.located(node_id, field)),
        }
    }
}

impl<T> From<T> for Deferred<T> {
    fn from(value: T) -> Self {
        Self::Value(value)
    }
}

/// A literal a param can hold, converted directly to and from JSON and expression values.
pub trait ParamValue: Clone {
    /// Whether `from_expression` can produce this type.
    const ACCEPTS_EXPRESSIONS: bool;

    /// Calls `f` with each expression this literal holds of its own.
    fn for_each_expression(&self, _f: &mut dyn FnMut(&Expression)) {}

    /// Parses a literal, already told apart from an `=expression`. Errors name `path`.
    fn from_json_value(value: &Value, path: &str) -> Result<Self, String>;

    /// The JSON that parses to this literal.
    fn to_json_value(&self) -> Value;

    /// This type from what an expression evaluated to.
    fn from_expression(value: ExpressionValue) -> crate::Result<Self>;

    /// This literal as `node(id, 'param')` reads it, or `None` for a kind expressions have no
    /// value for, such as a paint.
    fn to_expression(&self) -> Option<ExpressionValue>;
}

impl<T> DelegateEvaluable for Deferred<T>
where
    T: ParamValue,
{
    type Evaluated = T;

    const ACCEPTS_EXPRESSIONS: bool = T::ACCEPTS_EXPRESSIONS;

    fn eval(&self, ctx: &DelegateEvalContext<'_>) -> crate::Result<Self::Evaluated> {
        Deferred::eval(self, ctx.node_id, "", ctx.expr)
    }

    fn for_each_expression(&self, f: &mut dyn FnMut(&Expression)) {
        match self {
            Self::Value(value) => value.for_each_expression(f),
            Self::Expr(expr) => f(expr),
        }
    }

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        let escaped;
        let literal = match value.as_str().and_then(classify_string) {
            Some(StringParam::Expression(source)) => {
                let expr = Expression::parse(source)
                    .map_err(|error| format!("`{path}`: `={source}`: {error}"))?;
                return if T::ACCEPTS_EXPRESSIONS {
                    Ok(Self::Expr(expr))
                } else {
                    Err(crate::json::expression_unsupported(path))
                };
            }
            Some(StringParam::EscapedLiteral(text)) => {
                escaped = Value::from(text);
                &escaped
            }
            None => value,
        };
        T::from_json_value(literal, path).map(Self::Value)
    }

    fn to_json_value(&self) -> Option<Value> {
        Some(match self {
            Self::Expr(expr) => Value::from(format!("={}", expr.source)),
            Self::Value(value) => match value.to_json_value() {
                Value::String(text) if text.starts_with('=') => Value::from(format!("={text}")),
                json => json,
            },
        })
    }

    fn read(&self) -> Option<PropertyRead<'_>> {
        Some(match self {
            Self::Expr(expr) => PropertyRead::Expr(expr),
            Self::Value(value) => PropertyRead::Value(value.to_expression()),
        })
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
    ShaderUniforms,
    ClipProps
);

/// The failure of an expression whose value `value` has the wrong type for its param.
fn wrong_type(expected: &str, value: &ExpressionValue) -> LumenError {
    LumenError::evaluate(format!(
        "expected {expected}, got {} `{}`",
        value.type_name(),
        value.as_string()
    ))
}

fn number(value: ExpressionValue) -> crate::Result<f64> {
    value.as_f64().ok_or_else(|| wrong_type("a number", &value))
}

/// An expression's number as an integer param takes it: truncated toward zero, or an error when
/// it is NaN or infinite.
pub fn truncated_int(value: ExpressionValue) -> crate::Result<i64> {
    let value = number(value)?;
    if value.is_finite() {
        Ok(value as i64)
    } else {
        Err(LumenError::evaluate(format!(
            "{value} can't be used as an integer"
        )))
    }
}

/// The value of the option of `def` that an expression names, as JSON names it, or else its number
/// as [`truncated_int`] takes it.
pub fn enum_value(value: ExpressionValue, def: &EnumDef) -> crate::Result<i64> {
    if let ExpressionValue::String(name) = &value
        && let Some(option) = def
            .options
            .iter()
            .find(|option| option.name == name || option.aliases.contains(&name.as_str()))
    {
        return Ok(option.value);
    }
    if value.as_f64().is_none() {
        let names: Vec<_> = def.options.iter().map(|option| option.name).collect();
        return Err(wrong_type(
            &format!("one of `{}` or a number", names.join("`, `")),
            &value,
        ));
    }
    truncated_int(value)
}

fn vec2(value: ExpressionValue) -> crate::Result<(f64, f64)> {
    value.as_vec2().ok_or_else(|| wrong_type("a vec2", &value))
}

fn vec3(value: ExpressionValue) -> crate::Result<[f64; 3]> {
    value.as_vec3().ok_or_else(|| wrong_type("a vec3", &value))
}

fn color(value: ExpressionValue) -> crate::Result<[u8; 4]> {
    value
        .as_color_u8()
        .ok_or_else(|| wrong_type("a color", &value))
}

impl ParamValue for f64 {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        value
            .as_f64()
            .ok_or_else(|| format!("`{path}` expected float"))
    }

    fn to_json_value(&self) -> Value {
        Value::from(*self)
    }

    fn from_expression(value: ExpressionValue) -> crate::Result<Self> {
        number(value)
    }

    fn to_expression(&self) -> Option<ExpressionValue> {
        Some(ExpressionValue::Number(*self))
    }
}

impl ParamValue for f32 {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        f64::from_json_value(value, path).map(|value| value as f32)
    }

    fn to_json_value(&self) -> Value {
        Value::from(f64::from(*self))
    }

    fn from_expression(value: ExpressionValue) -> crate::Result<Self> {
        number(value).map(|value| value as f32)
    }

    fn to_expression(&self) -> Option<ExpressionValue> {
        Some(ExpressionValue::Number(f64::from(*self)))
    }
}

impl ParamValue for i64 {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        integral_i64(value).ok_or_else(|| format!("`{path}` {}", integer_mismatch(value)))
    }

    fn to_json_value(&self) -> Value {
        Value::from(*self)
    }

    fn from_expression(value: ExpressionValue) -> crate::Result<Self> {
        truncated_int(value)
    }

    fn to_expression(&self) -> Option<ExpressionValue> {
        Some(ExpressionValue::Number(*self as f64))
    }
}

impl ParamValue for u32 {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        let value = i64::from_json_value(value, path)?;
        u32::try_from(value).map_err(|_| {
            format!(
                "`{path}` must be an integer from 0 to {}, got {value}",
                u32::MAX
            )
        })
    }

    fn to_json_value(&self) -> Value {
        Value::from(*self)
    }

    fn from_expression(value: ExpressionValue) -> crate::Result<Self> {
        let value = truncated_int(value)?;
        u32::try_from(value).map_err(|_| {
            LumenError::evaluate(format!("{value} is out of range for an unsigned integer"))
        })
    }

    fn to_expression(&self) -> Option<ExpressionValue> {
        Some(ExpressionValue::Number(f64::from(*self)))
    }
}

impl ParamValue for u8 {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        let value = i64::from_json_value(value, path)?;
        u8::try_from(value)
            .map_err(|_| format!("`{path}` must be an integer from 0 to 255, got {value}"))
    }

    fn to_json_value(&self) -> Value {
        Value::from(*self)
    }

    fn from_expression(value: ExpressionValue) -> crate::Result<Self> {
        let value = truncated_int(value)?;
        u8::try_from(value)
            .map_err(|_| LumenError::evaluate(format!("{value} is out of range for 0 to 255")))
    }

    fn to_expression(&self) -> Option<ExpressionValue> {
        Some(ExpressionValue::Number(f64::from(*self)))
    }
}

impl ParamValue for bool {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        value
            .as_bool()
            .or_else(|| value.as_i64().map(|value| value != 0))
            .ok_or_else(|| format!("`{path}` expected bool"))
    }

    fn to_json_value(&self) -> Value {
        Value::from(*self)
    }

    fn from_expression(value: ExpressionValue) -> crate::Result<Self> {
        Ok(value.as_bool())
    }

    fn to_expression(&self) -> Option<ExpressionValue> {
        Some(ExpressionValue::Boolean(*self))
    }
}

impl ParamValue for String {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        value
            .as_str()
            .map(ToString::to_string)
            .ok_or_else(|| format!("`{path}` expected string"))
    }

    fn to_json_value(&self) -> Value {
        Value::from(self.as_str())
    }

    fn from_expression(value: ExpressionValue) -> crate::Result<Self> {
        Ok(value.as_string())
    }

    fn to_expression(&self) -> Option<ExpressionValue> {
        Some(ExpressionValue::String(self.clone()))
    }
}

impl ParamValue for [u8; 4] {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        try_parse_color(value).map_err(|error| format!("`{path}` expected color: {error}"))
    }

    fn to_json_value(&self) -> Value {
        Value::from(self.to_vec())
    }

    fn from_expression(value: ExpressionValue) -> crate::Result<Self> {
        color(value)
    }

    fn to_expression(&self) -> Option<ExpressionValue> {
        Some(ExpressionValue::color_from_u8(*self))
    }
}

impl ParamValue for vector::paint::Paint {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        vector::paint::Paint::from_json_value(value).map_err(|error| format!("`{path}`: {error}"))
    }

    fn to_json_value(&self) -> Value {
        vector::paint::Paint::to_json_value(self)
    }

    fn from_expression(value: ExpressionValue) -> crate::Result<Self> {
        color(value).map(vector::paint::Paint::solid)
    }

    fn to_expression(&self) -> Option<ExpressionValue> {
        None
    }
}

impl ParamValue for ShaderUniforms {
    const ACCEPTS_EXPRESSIONS: bool = false;

    fn for_each_expression(&self, f: &mut dyn FnMut(&Expression)) {
        for value in self.0.values() {
            match value {
                super::ShaderUniformValue::Expr(expr) => f(expr),
                super::ShaderUniformValue::Vector(components) => {
                    components.for_each_expression(f);
                }
                super::ShaderUniformValue::Number(_) | super::ShaderUniformValue::Color { .. } => {}
            }
        }
    }

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        ShaderUniforms::from_json_value(value, path)
    }

    fn to_json_value(&self) -> Value {
        ShaderUniforms::to_json_value(self)
    }

    fn from_expression(_value: ExpressionValue) -> crate::Result<Self> {
        Err(LumenError::evaluate(
            "ShaderUniforms can't be an expression",
        ))
    }

    fn to_expression(&self) -> Option<ExpressionValue> {
        None
    }
}

impl ParamValue for ClipProps {
    const ACCEPTS_EXPRESSIONS: bool = false;

    fn for_each_expression(&self, f: &mut dyn FnMut(&Expression)) {
        for value in self.0.values() {
            if let super::ClipPropValue::Expr(expr) = value {
                f(expr);
            }
        }
    }

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        ClipProps::from_json_value(value, path)
    }

    fn to_json_value(&self) -> Value {
        ClipProps::to_json_value(self)
    }

    fn from_expression(_value: ExpressionValue) -> crate::Result<Self> {
        Err(LumenError::evaluate("ClipProps can't be an expression"))
    }

    fn to_expression(&self) -> Option<ExpressionValue> {
        None
    }
}

impl ParamValue for (f64, f64) {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        let items = value
            .as_array()
            .ok_or_else(|| format!("`{path}` expected [x, y]"))?;
        if items.len() != 2 {
            return Err(format!(
                "`{path}` expected [x, y], got array of length {}",
                items.len()
            ));
        }
        let component = |index: usize| {
            items[index]
                .as_f64()
                .ok_or_else(|| format!("`{path}[{index}]` expected number"))
        };
        Ok((component(0)?, component(1)?))
    }

    fn to_json_value(&self) -> Value {
        Value::from(vec![self.0, self.1])
    }

    fn from_expression(value: ExpressionValue) -> crate::Result<Self> {
        vec2(value)
    }

    fn to_expression(&self) -> Option<ExpressionValue> {
        Some(ExpressionValue::Vec2(*self))
    }
}

impl ParamValue for [f32; 2] {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        let (x, y) = <(f64, f64)>::from_json_value(value, path)?;
        Ok([x as f32, y as f32])
    }

    fn to_json_value(&self) -> Value {
        Value::from(vec![f64::from(self[0]), f64::from(self[1])])
    }

    fn from_expression(value: ExpressionValue) -> crate::Result<Self> {
        vec2(value).map(|(x, y)| [x as f32, y as f32])
    }

    fn to_expression(&self) -> Option<ExpressionValue> {
        Some(ExpressionValue::Vec2((
            f64::from(self[0]),
            f64::from(self[1]),
        )))
    }
}

impl ParamValue for [f64; 3] {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        let items = value
            .as_array()
            .ok_or_else(|| format!("`{path}` expected [x, y, z]"))?;
        if items.len() != 3 {
            return Err(format!(
                "`{path}` expected [x, y, z], got array of length {}",
                items.len()
            ));
        }
        let component = |index: usize| {
            items[index]
                .as_f64()
                .ok_or_else(|| format!("`{path}[{index}]` expected number"))
        };
        Ok([component(0)?, component(1)?, component(2)?])
    }

    fn to_json_value(&self) -> Value {
        Value::from(self.to_vec())
    }

    fn from_expression(value: ExpressionValue) -> crate::Result<Self> {
        vec3(value)
    }

    fn to_expression(&self) -> Option<ExpressionValue> {
        Some(ExpressionValue::Vec3(*self))
    }
}

impl ParamValue for [f32; 3] {
    const ACCEPTS_EXPRESSIONS: bool = true;

    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        <[f64; 3]>::from_json_value(value, path).map(|vector| vector.map(|x| x as f32))
    }

    fn to_json_value(&self) -> Value {
        self.map(f64::from).to_json_value()
    }

    fn from_expression(value: ExpressionValue) -> crate::Result<Self> {
        vec3(value).map(|vector| vector.map(|x| x as f32))
    }

    fn to_expression(&self) -> Option<ExpressionValue> {
        Some(ExpressionValue::Vec3(self.map(f64::from)))
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        expr::{Expression, ExpressionContext},
        node::{
            Deferred, DelegateEvaluable, NodeId, ParamValue,
            vector::paint::{GradientSpread, Paint},
        },
    };

    fn context() -> ExpressionContext<'static> {
        ExpressionContext {
            frame: 12.0,
            fps: 24.0,
            width: 1920,
            height: 1080,
            graph: None,
            media: None,
            glyph: None,
            props: None,
        }
    }

    #[test]
    fn integer_targets_truncate_finite_results_and_reject_the_rest() {
        fn eval<T: ParamValue>(source: &str) -> crate::Result<T> {
            Deferred::<T>::Expr(Expression::parse(source).unwrap()).eval(
                NodeId::new(4),
                "count",
                &context(),
            )
        }
        assert_eq!(eval::<i64>("-2.9").unwrap(), -2);
        assert_eq!(eval::<u32>("2.9").unwrap(), 2);
        assert_eq!(eval::<u8>("2.9").unwrap(), 2);
        assert!(eval::<u8>("256").is_err());

        for source in ["pow(-1, 0.5)", "pow(10, 400)"] {
            for error in [
                eval::<i64>(source).err(),
                eval::<u32>(source).err(),
                eval::<u8>(source).err(),
                eval::<GradientSpread>(source).err(),
            ] {
                let error = error.expect(source).to_string();
                assert!(
                    error.contains("can't be used as an integer"),
                    "{source}: {error}"
                );
            }
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
    fn vec3_params_read_json_and_expressions() {
        let parse = |json: serde_json::Value| <[f64; 3]>::from_json_value(&json, "position");
        assert_eq!(
            parse(serde_json::json!([1, -2.5, 3])).unwrap(),
            [1.0, -2.5, 3.0]
        );
        assert_eq!(
            parse(serde_json::json!([1, 2])).unwrap_err(),
            "`position` expected [x, y, z], got array of length 2"
        );
        assert_eq!(
            parse(serde_json::json!([1, "2", 3])).unwrap_err(),
            "`position[1]` expected number"
        );
        assert_eq!(
            parse(serde_json::json!(1)).unwrap_err(),
            "`position` expected [x, y, z]"
        );

        let expression = Expression::parse("vec3(frame / 2, time + 0.25, -1)").unwrap();
        assert_eq!(
            Deferred::<[f32; 3]>::Expr(expression)
                .eval(NodeId::new(4), "position", &context())
                .unwrap(),
            [6.0, 0.75, -1.0]
        );
        let error = Deferred::<[f64; 3]>::Expr(Expression::parse("vec2(1, 2)").unwrap())
            .eval(NodeId::new(4), "position", &context())
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "node 4 `position`: expected a vec3, got vec2 `[1, 2]`"
        );
    }

    #[test]
    fn vec2_targets_reject_scalar_results_naming_the_param() {
        for error in [
            Deferred::<(f64, f64)>::Expr(Expression::parse("42").unwrap())
                .eval(NodeId::new(4), "position", &context())
                .unwrap_err(),
            Deferred::<[f32; 2]>::Expr(Expression::parse("42").unwrap())
                .eval(NodeId::new(4), "position", &context())
                .unwrap_err(),
        ] {
            assert_eq!(
                error.to_string(),
                "node 4 `position`: expected a vec2, got number `42`"
            );
        }
    }

    #[test]
    fn vec2_expression_errors_name_the_param() {
        let error = Deferred::<(f64, f64)>::Expr(Expression::parse("vec2(1, vec2(2, 3))").unwrap())
            .eval(NodeId::new(4), "position", &context())
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "node 4 `position`: vec2 expects numeric arguments"
        );
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
            error
                .to_string()
                .contains("expected a color, got number `12`"),
            "{error}"
        );
    }

    #[test]
    fn paint_delegate_evaluates_colour_expressions() {
        use crate::node::{DelegateEvalContext, PropertyRead, vector::paint::PaintDelegate};

        let delegate =
            PaintDelegate::from_json_value(&"=tween(#ff0000, #0000ff, 0, 24)".into(), "fill_paint")
                .unwrap();
        assert!(matches!(delegate.read(), Some(PropertyRead::Expr(_))));
        let ctx = DelegateEvalContext {
            node_id: NodeId::new(3),
            expr: &context(),
        };
        assert_eq!(
            delegate.eval(&ctx).unwrap(),
            Paint::solid([128, 0, 128, 255])
        );
    }

    #[test]
    fn color_and_paint_take_literals_and_expressions() {
        let parse_paint = |json: &str| Deferred::<Paint>::from_json_value(&json.into(), "paint");
        assert!(matches!(
            Deferred::<[u8; 4]>::from_json_value(&"=rgb(255, 0, 0)".into(), "color"),
            Ok(Deferred::Expr(_))
        ));
        assert!(matches!(parse_paint("=#ff0000"), Ok(Deferred::Expr(_))));
        assert!(matches!(parse_paint("#ff0000"), Ok(Deferred::Value(_))));
    }

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
            error.contains("node 7: `width` must be an integer from 0 to 4294967295, got -1"),
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

    #[test]
    fn a_leading_equals_is_an_expression_and_two_escape_a_literal() {
        let parse = |json: &str| Deferred::<String>::from_json_value(&json.into(), "text");
        for (input, expected) in [("==wow", "=wow"), ("===", "=="), ("==", "=")] {
            assert!(
                matches!(parse(input), Ok(Deferred::Value(ref text)) if text == expected),
                "{input}"
            );
            assert_eq!(
                parse(input).unwrap().to_json_value(),
                Some(input.into()),
                "{input}"
            );
        }
        assert!(matches!(parse("=frame"), Ok(Deferred::Expr(_))));
    }

    #[test]
    fn literal_kinds_reject_an_expression_by_name() {
        let error = Deferred::<crate::node::ClipProps>::from_json_value(&"=frame".into(), "props")
            .unwrap_err();
        assert_eq!(error, "`props` does not support expressions");
    }
}
