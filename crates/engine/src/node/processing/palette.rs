use serde_json::Value;

use crate::expr::Expression;
use crate::node::{DelegateEvalContext, DelegateEvaluable, Delegated, ParamTypeDef, PropertyRead};

/// Fewest colours a palette may list.
pub const MIN_PALETTE_COLORS: usize = 2;
/// Most colours a palette may list.
pub const MAX_PALETTE_COLORS: usize = 16;

/// A list of solid sRGB colours.
#[derive(Debug, Clone, PartialEq, Eq, lumen_macros::StructuralHash)]
pub struct Palette(pub Vec<[u8; 4]>);

impl Default for Palette {
    fn default() -> Self {
        Self(vec![[0, 0, 0, 255], [255, 255, 255, 255]])
    }
}

pub(crate) static PALETTE: ParamTypeDef = ParamTypeDef {
    name: "palette",
    #[cfg(feature = "metadata")]
    schema: || serde_json::json!({ "$ref": "#/$defs/palette" }),
};

impl Delegated for Palette {
    type Delegate = Palette;

    fn into_delegate(self) -> Self::Delegate {
        self
    }
}

impl DelegateEvaluable for Palette {
    type Evaluated = Palette;

    const ACCEPTS_EXPRESSIONS: bool = false;

    fn eval(&self, _: &DelegateEvalContext<'_>) -> crate::Result<Self::Evaluated> {
        Ok(self.clone())
    }

    fn for_each_expression(&self, _: &mut dyn FnMut(&Expression)) {}

    /// Parses an array of 2 to 16 literal colours.
    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        crate::json::reject_expression(value, path)?;
        let items = value
            .as_array()
            .ok_or_else(|| format!("`{path}` expected an array of colors, got {value}"))?;
        if !(MIN_PALETTE_COLORS..=MAX_PALETTE_COLORS).contains(&items.len()) {
            return Err(format!(
                "`{path}` has {} colors, expected {MIN_PALETTE_COLORS} to {MAX_PALETTE_COLORS}",
                items.len()
            ));
        }
        items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let path = format!("{path}[{index}]");
                crate::json::reject_expression(item, &path)?;
                crate::json::try_parse_color(item).map_err(|reason| format!("`{path}`: {reason}"))
            })
            .collect::<Result<_, _>>()
            .map(Self)
    }

    fn to_json_value(&self) -> Option<Value> {
        Some(Value::Array(
            self.0
                .iter()
                .map(|color| Value::from(color.to_vec()))
                .collect(),
        ))
    }

    fn read(&self) -> Option<PropertyRead<'_>> {
        Some(PropertyRead::Value(None))
    }
}
