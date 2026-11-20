use serde_json::Value;

use crate::expr::Expression;
use crate::node::{DelegateEvalContext, DelegateEvaluable, Delegated, ParamTypeDef, PropertyRead};

/// Most names `render_3d`'s `mattes` may list, one for each channel of its `matte` output.
pub const MAX_MATTES: usize = 4;

/// The matte names a `render_3d` gives the channels of its `matte` output, in channel order.
#[derive(Debug, Clone, Default, PartialEq, Eq, lumen_macros::StructuralHash)]
pub struct MatteNames(pub Vec<String>);

pub(crate) static MATTE_NAMES: ParamTypeDef = ParamTypeDef {
    name: "matte_names",
    #[cfg(feature = "metadata")]
    schema: || serde_json::json!({ "$ref": "#/$defs/matteNames" }),
};

impl MatteNames {
    /// What is wrong with the names, if anything: more than [`MAX_MATTES`] of them, or one
    /// listed twice.
    pub(crate) fn problem(&self) -> Option<String> {
        let names = &self.0;
        if names.len() > MAX_MATTES {
            return Some(format!(
                "`mattes` lists {} names, more than the {MAX_MATTES} channels of the matte output",
                names.len()
            ));
        }
        names.iter().enumerate().find_map(|(index, name)| {
            names[..index]
                .contains(name)
                .then(|| format!("`mattes` lists `{name}` twice"))
        })
    }

    /// The channel objects named `name` draw into, if `name` is listed.
    pub(crate) fn channel(&self, name: &str) -> Option<u8> {
        if name.is_empty() {
            return None;
        }
        let index = self.0.iter().position(|listed| listed == name)?;
        u8::try_from(index).ok()
    }
}

impl Delegated for MatteNames {
    type Delegate = MatteNames;

    fn into_delegate(self) -> Self::Delegate {
        self
    }
}

impl DelegateEvaluable for MatteNames {
    type Evaluated = MatteNames;

    const ACCEPTS_EXPRESSIONS: bool = false;

    fn eval(&self, _: &DelegateEvalContext<'_>) -> crate::Result<Self::Evaluated> {
        Ok(self.clone())
    }

    fn for_each_expression(&self, _: &mut dyn FnMut(&Expression)) {}

    /// Parses an array of literal strings.
    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        crate::json::reject_expression(value, path)?;
        let items = value
            .as_array()
            .ok_or_else(|| format!("`{path}` expected an array of names, got {value}"))?;
        items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let path = format!("{path}[{index}]");
                crate::json::reject_expression(item, &path)?;
                item.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("`{path}` expected a name, got {item}"))
            })
            .collect::<Result<_, _>>()
            .map(Self)
    }

    fn to_json_value(&self) -> Option<Value> {
        Some(Value::Array(
            self.0.iter().cloned().map(Value::String).collect(),
        ))
    }

    fn read(&self) -> Option<PropertyRead<'_>> {
        Some(PropertyRead::Value(None))
    }
}
