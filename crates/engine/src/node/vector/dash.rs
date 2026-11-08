use serde_json::Value;

use crate::expr::Expression;
use crate::node::{Deferred, DelegateEvalContext, DelegateEvaluable, Delegated, PropertyRead};

/// Most lengths a dash pattern may list.
pub const MAX_DASH_LENGTHS: usize = 16;
/// Lengths the shader holds: an odd count is listed twice to make the pattern.
pub(super) const GPU_DASH_LENGTHS: usize = 2 * MAX_DASH_LENGTHS;
/// Longest a dash or gap may be, so that a pattern of [`GPU_DASH_LENGTHS`] still sums to a
/// finite f32.
pub const MAX_DASH_LENGTH: f64 = 1e37;

/// Lengths that a stroke alternately draws and skips, starting with a dash.
#[derive(Debug, Clone, Default, PartialEq, lumen_macros::StructuralHash)]
pub struct DashPattern(pub Vec<f64>);

/// [`DashPattern`] whose lengths take literals or expressions. The whole pattern cannot be an
/// expression.
#[derive(Debug, Clone, Default, lumen_macros::StructuralHash)]
pub struct DashPatternDelegate(pub Vec<Deferred<f64>>);

impl Delegated for DashPattern {
    type Delegate = DashPatternDelegate;

    fn into_delegate(self) -> Self::Delegate {
        DashPatternDelegate(self.0.into_iter().map(Deferred::Value).collect())
    }
}

impl DelegateEvaluable for DashPatternDelegate {
    type Evaluated = DashPattern;

    const ACCEPTS_EXPRESSIONS: bool = false;

    fn eval(&self, ctx: &DelegateEvalContext<'_>) -> crate::Result<Self::Evaluated> {
        self.0.eval(ctx).map(DashPattern)
    }

    fn for_each_expression(&self, f: &mut dyn FnMut(&Expression)) {
        self.0.for_each_expression(f);
    }

    /// Parses an array of non-negative lengths or `=expression`s.
    fn from_json_value(value: &Value, path: &str) -> Result<Self, String> {
        crate::json::reject_expression(value, path)?;
        let lengths = Vec::<Deferred<f64>>::from_json_value(value, path)?;
        check_count(lengths.len(), path)?;
        for (index, length) in lengths.iter().enumerate() {
            if let Deferred::Value(length) = length {
                check_length(*length, &format!("{path}[{index}]"))?;
            }
        }
        Ok(Self(lengths))
    }

    fn to_json_value(&self) -> Option<Value> {
        self.0.to_json_value()
    }

    fn read(&self) -> Option<PropertyRead<'_>> {
        Some(PropertyRead::Value(None))
    }
}

fn check_count(count: usize, path: &str) -> Result<(), String> {
    if count > MAX_DASH_LENGTHS {
        return Err(format!(
            "`{path}` has {count} lengths, more than {MAX_DASH_LENGTHS}"
        ));
    }
    Ok(())
}

fn check_length(length: f64, path: &str) -> Result<(), String> {
    if !length.is_finite() {
        Err(format!("`{path}` must be finite, got {length}"))
    } else if length < 0.0 {
        Err(format!("`{path}` must not be negative, got {length}"))
    } else if length > MAX_DASH_LENGTH {
        Err(format!(
            "`{path}` must be at most {MAX_DASH_LENGTH:e}, got {length:e}"
        ))
    } else {
        Ok(())
    }
}

/// A dash pattern as the path shader takes it. A count of 0 strokes solidly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct GpuDash {
    pub(super) count: u32,
    /// Sum of `lengths`.
    period: f64,
    /// The offset within one period, in `[0, period)`.
    offset: f64,
    pub(super) lengths: [[f32; 4]; GPU_DASH_LENGTHS / 4],
}

impl GpuDash {
    const SOLID: Self = Self {
        count: 0,
        period: 0.0,
        offset: 0.0,
        lengths: [[0.0; 4]; GPU_DASH_LENGTHS / 4],
    };

    /// Checks `pattern` and `offset`, naming the params `stroke_dash` and `stroke_dash_offset`
    /// in errors. A pattern that is empty or all zeros in f32 strokes solidly, as in SVG.
    pub(super) fn new(pattern: &DashPattern, offset: f64) -> Result<Self, String> {
        let lengths = &pattern.0;
        check_count(lengths.len(), "stroke_dash")?;
        for (index, &length) in lengths.iter().enumerate() {
            check_length(length, &format!("stroke_dash[{index}]"))?;
        }
        if !offset.is_finite() {
            return Err(format!("`stroke_dash_offset` must be finite, got {offset}"));
        }
        let count = lengths.len() * if lengths.len() % 2 == 1 { 2 } else { 1 };
        let mut gpu = Self {
            count: count as u32,
            ..Self::SOLID
        };
        for (slot, &length) in lengths.iter().cycle().take(count).enumerate() {
            let length = length as f32;
            gpu.lengths[slot / 4][slot % 4] = length;
            gpu.period += f64::from(length);
        }
        if gpu.period == 0.0 {
            return Ok(Self::SOLID);
        }
        gpu.offset = offset.rem_euclid(gpu.period);
        Ok(gpu)
    }

    pub(super) fn period(&self) -> f32 {
        self.period as f32
    }

    /// Distance into the pattern, within one period, at `distance` along a contour.
    pub(super) fn phase(&self, distance: f64) -> f32 {
        if self.count == 0 {
            return 0.0;
        }
        ((self.offset + distance) % self.period) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpu(lengths: &[f64], offset: f64) -> Result<GpuDash, String> {
        GpuDash::new(&DashPattern(lengths.to_vec()), offset)
    }

    #[test]
    fn empty_and_all_zero_patterns_stroke_solidly() {
        assert_eq!(gpu(&[], 5.0).unwrap(), GpuDash::SOLID);
        assert_eq!(gpu(&[0.0, 0.0, 0.0], 5.0).unwrap(), GpuDash::SOLID);
    }

    #[test]
    fn odd_patterns_repeat_and_the_offset_wraps_into_one_period() {
        let dash = gpu(&[3.0, 1.0, 2.0], -1.0).unwrap();
        assert_eq!(dash.count, 6);
        assert_eq!(dash.period(), 12.0);
        assert_eq!(dash.phase(0.0), 11.0);
        assert_eq!(dash.phase(3.0), 2.0);
        assert_eq!(dash.lengths[0], [3.0, 1.0, 2.0, 3.0]);
        assert_eq!(dash.lengths[1], [1.0, 2.0, 0.0, 0.0]);
        assert_eq!(gpu(&[12.0, 6.0], 40.0).unwrap().phase(0.0), 4.0);
    }

    #[test]
    fn lengths_stay_finite_in_f32_and_underflowing_patterns_stroke_solidly() {
        assert_eq!(gpu(&[1e-50, 1e-50], 0.0).unwrap(), GpuDash::SOLID);
        assert!(
            gpu(&[MAX_DASH_LENGTH; MAX_DASH_LENGTHS], 0.0)
                .unwrap()
                .period()
                .is_finite()
        );
        assert_eq!(
            gpu(&[1e40, 1.0], 0.0).unwrap_err(),
            "`stroke_dash[0]` must be at most 1e37, got 1e40"
        );
    }

    #[test]
    fn negative_non_finite_and_too_many_lengths_are_errors() {
        assert_eq!(
            gpu(&[4.0, -1.0], 0.0).unwrap_err(),
            "`stroke_dash[1]` must not be negative, got -1"
        );
        assert_eq!(
            gpu(&[f64::NAN], 0.0).unwrap_err(),
            "`stroke_dash[0]` must be finite, got NaN"
        );
        assert_eq!(
            gpu(&[4.0], f64::INFINITY).unwrap_err(),
            "`stroke_dash_offset` must be finite, got inf"
        );
        assert!(gpu(&[1.0; MAX_DASH_LENGTHS], 0.0).is_ok());
        assert!(
            gpu(&[1.0; MAX_DASH_LENGTHS + 1], 0.0)
                .unwrap_err()
                .contains("more than 16")
        );
    }

    #[test]
    fn json_rejects_negative_literals_whole_expressions_and_long_lists() {
        let parse = |value: Value| DashPatternDelegate::from_json_value(&value, "stroke_dash");
        let parsed = parse(serde_json::json!([12, "=frame", 0])).unwrap();
        assert_eq!(parsed.0.len(), 3);
        assert!(parsed.0[1].is_expr());
        assert_eq!(
            parse(serde_json::json!([4, -2])).unwrap_err(),
            "`stroke_dash[1]` must not be negative, got -2"
        );
        assert!(parse(serde_json::json!([1e38])).is_err());
        assert!(parse(serde_json::json!("=frame")).is_err());
        assert!(parse(serde_json::json!(4)).is_err());
        assert!(parse(serde_json::json!(vec![1; MAX_DASH_LENGTHS + 1])).is_err());
    }
}
