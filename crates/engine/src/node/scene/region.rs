use serde_json::Value;

use crate::expr::Expression;
use crate::node::{Deferred, DelegateEvalContext, DelegateEvaluable, Delegated, PropertyRead};

/// The part of a raster a node shows, `[x, y, width, height]` in pixels, or all of it when unset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, lumen_macros::StructuralHash)]
pub struct Region(pub Option<[i64; 4]>);

/// [`Region`] whose four numbers take literals or expressions. The whole region cannot be an
/// expression.
#[derive(Debug, Clone, Default, lumen_macros::StructuralHash)]
pub struct RegionDelegate(Option<[Deferred<i64>; 4]>);

impl Delegated for Region {
    type Delegate = RegionDelegate;

    fn into_delegate(self) -> Self::Delegate {
        RegionDelegate(self.0.map(|numbers| numbers.map(Deferred::Value)))
    }
}

impl DelegateEvaluable for RegionDelegate {
    type Evaluated = Region;

    const ACCEPTS_EXPRESSIONS: bool = false;

    fn eval(&self, ctx: &DelegateEvalContext<'_>) -> crate::Result<Self::Evaluated> {
        let Some(numbers) = &self.0 else {
            return Ok(Region(None));
        };
        let mut region = [0; 4];
        for (index, (slot, number)) in region.iter_mut().zip(numbers).enumerate() {
            *slot = DelegateEvaluable::eval(number, ctx).map_err(|error| error.in_index(index))?;
        }
        Ok(Region(Some(region)))
    }

    fn for_each_expression(&self, f: &mut dyn FnMut(&Expression)) {
        for number in self.0.iter().flatten() {
            number.for_each_expression(f);
        }
    }

    /// Parses an array of four integers or `=expression`s.
    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        crate::json::reject_expression(value, path).map_err(|_| {
            format!(
                "`{path}` cannot be an expression as a whole; put expressions inside its \
                 four-element array, [x, y, width, height]"
            )
        })?;
        let items = value
            .as_array()
            .ok_or_else(|| format!("`{path}` expected an array, got {value}"))?;
        let numbers = items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                Deferred::<i64>::from_json_value(item, &format!("{path}[{index}]"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let count = numbers.len();
        let numbers: [Deferred<i64>; 4] = numbers
            .try_into()
            .map_err(|_| format!("`{path}` expected [x, y, width, height], got {count} numbers"))?;
        Ok(Self(Some(numbers)))
    }

    fn to_json_value(&self) -> Option<Value> {
        let numbers = self.0.as_ref()?;
        Some(Value::Array(
            numbers
                .iter()
                .filter_map(DelegateEvaluable::to_json_value)
                .collect(),
        ))
    }

    fn read(&self) -> Option<PropertyRead<'_>> {
        Some(PropertyRead::Value(None))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn four_numbers_or_expressions_make_a_region() {
        let region =
            RegionDelegate::from_json_value(&json!([1, 2, "=frame", 4]), "region").expect("parses");
        assert!(region.has_expressions());
        assert_eq!(region.to_json_value(), Some(json!([1, 2, "=frame", 4])));
    }

    #[test]
    fn a_region_is_four_numbers_and_never_an_expression_as_a_whole() {
        for bad in [
            json!([1, 2, 3]),
            json!([1, 2, 3, 4, 5]),
            json!("=vec4(0)"),
            json!(1.5),
        ] {
            assert!(
                RegionDelegate::from_json_value(&bad, "region").is_err(),
                "{bad}"
            );
        }
        assert!(RegionDelegate::from_json_value(&json!([0, 0, 1.5, 1]), "region").is_err());
        let whole = RegionDelegate::from_json_value(&json!("=vec4(0)"), "region").unwrap_err();
        assert!(whole.contains("inside its four-element array"), "{whole}");
    }

    #[test]
    fn an_unset_region_has_no_json() {
        assert_eq!(RegionDelegate::default().to_json_value(), None);
    }
}
