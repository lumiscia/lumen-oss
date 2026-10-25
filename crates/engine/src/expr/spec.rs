//! The builtin function table. The parser, evaluator and generated schema all read from it.

/// The type of a builtin argument or result, as published in the schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Number,
    Vec2,
    Color,
    String,
    Easing,
    Any,
}

impl ValueKind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Number => "number",
            Self::Vec2 => "vec2",
            Self::Color => "color",
            Self::String => "string",
            Self::Easing => "easing",
            Self::Any => "any",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinArg {
    pub name: &'static str,
    pub kind: ValueKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinSpec {
    pub name: &'static str,
    pub builtin: BuiltinFn,
    pub min_args: usize,
    /// `None` means the last argument repeats.
    pub max_args: Option<usize>,
    pub args: &'static [BuiltinArg],
    pub returns: ValueKind,
    pub doc: &'static str,
}

impl BuiltinSpec {
    pub fn accepts_arity(&self, count: usize) -> bool {
        count >= self.min_args && self.max_args.is_none_or(|max| count <= max)
    }

    pub fn arity_description(&self) -> String {
        match self.max_args {
            None => format!("at least {}", self.min_args),
            Some(max) if max == self.min_args => max.to_string(),
            Some(max) => format!("{} to {max}", self.min_args),
        }
    }
}

/// Declares `BuiltinFn` and `BUILTINS` together so neither can drift from the other.
macro_rules! builtins {
    ($(
        $variant:ident $name:literal ($min:literal, $max:expr)
        [$($arg:ident: $kind:ident),*] -> $returns:ident, $doc:literal;
    )*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum BuiltinFn {
            $($variant),*
        }

        pub const BUILTINS: &[BuiltinSpec] = &[$(
            BuiltinSpec {
                name: $name,
                builtin: BuiltinFn::$variant,
                min_args: $min,
                max_args: $max,
                args: &[$(BuiltinArg { name: stringify!($arg), kind: ValueKind::$kind }),*],
                returns: ValueKind::$returns,
                doc: $doc,
            }
        ),*];
    };
}

builtins! {
    Min "min" (2, None) [values: Number] -> Number, "Smallest of two or more numbers.";
    Max "max" (2, None) [values: Number] -> Number, "Largest of two or more numbers.";
    Abs "abs" (1, Some(1)) [value: Number] -> Number, "Absolute value.";
    Floor "floor" (1, Some(1)) [value: Number] -> Number, "Rounds down to an integer.";
    Ceil "ceil" (1, Some(1)) [value: Number] -> Number, "Rounds up to an integer.";
    Round "round" (1, Some(1)) [value: Number] -> Number, "Rounds to the nearest integer.";
    Sin "sin" (1, Some(1)) [radians: Number] -> Number, "Sine of an angle in radians.";
    Cos "cos" (1, Some(1)) [radians: Number] -> Number, "Cosine of an angle in radians.";
    Clamp "clamp" (3, Some(3)) [value: Number, min: Number, max: Number] -> Number,
        "Limits a value to `min..=max`.";
    Lerp "lerp" (3, Some(3)) [from: Any, to: Any, t: Number] -> Any,
        "Interpolates from `from` to `to` by `t`.";
    Pow "pow" (2, Some(2)) [base: Number, exponent: Number] -> Number, "Raises `base` to `exponent`.";
    Mod "mod" (2, Some(2)) [value: Number, divisor: Number] -> Number,
        "Remainder of `value / divisor`, keeping the sign of `value`.";
    Fract "fract" (1, Some(1)) [value: Number] -> Number, "Fractional part of a number.";
    Smoothstep "smoothstep" (3, Some(3)) [edge0: Number, edge1: Number, x: Number] -> Number,
        "Hermite smoothing of `x` between two edges.";
    Linear "linear" (3, Some(3)) [from: Any, to: Any, t: Number] -> Any,
        "Same as `lerp`. Not to be confused with the `'linear'` easing name.";
    Step "step" (3, Some(3)) [from: Number, to: Number, t: Number] -> Number,
        "`to` once `t >= 1`, otherwise `from`.";
    TextHeight "text_height" (1, Some(4))
        [text: Any, font_size: Number, max_width: Number, font_family: String] -> Number,
        "Laid-out height of a string, or of `node(id)` for a text node.";
    TextWidth "text_width" (1, Some(4))
        [text: Any, font_size: Number, max_width: Number, font_family: String] -> Number,
        "Laid-out width of a string, or of `node(id)` for a text node.";
    Uppercase "uppercase" (1, Some(1)) [text: String] -> String, "Uppercases a string.";
    Lowercase "lowercase" (1, Some(1)) [text: String] -> String, "Lowercases a string.";
    Vec2 "vec2" (2, Some(2)) [x: Number, y: Number] -> Vec2, "Builds a 2D vector.";
    Progress "progress" (2, Some(2)) [start: Number, end: Number] -> Number,
        "0 before frame `start`, 1 after frame `end`, linear in between.";
    Tween "tween" (4, Some(5))
        [from: Any, to: Any, start: Number, duration: Number, ease: Easing] -> Any,
        "Eases from `from` to `to` over `duration` frames from frame `start`. `ease` defaults to `'linear'`.";
    InQuad "in_quad" (1, Some(1)) [t: Number] -> Number, "Quadratic ease-in.";
    OutQuad "out_quad" (1, Some(1)) [t: Number] -> Number, "Quadratic ease-out.";
    InOutQuad "in_out_quad" (1, Some(1)) [t: Number] -> Number, "Quadratic ease-in-out.";
    InCubic "in_cubic" (1, Some(1)) [t: Number] -> Number, "Cubic ease-in.";
    OutCubic "out_cubic" (1, Some(1)) [t: Number] -> Number, "Cubic ease-out.";
    InOutCubic "in_out_cubic" (1, Some(1)) [t: Number] -> Number, "Cubic ease-in-out.";
    InExpo "in_expo" (1, Some(1)) [t: Number] -> Number, "Exponential ease-in.";
    OutExpo "out_expo" (1, Some(1)) [t: Number] -> Number, "Exponential ease-out.";
    InOutExpo "in_out_expo" (1, Some(1)) [t: Number] -> Number, "Exponential ease-in-out.";
    InBack "in_back" (1, Some(1)) [t: Number] -> Number, "Ease-in that pulls back first.";
    OutBack "out_back" (1, Some(1)) [t: Number] -> Number, "Ease-out that overshoots.";
    InOutBack "in_out_back" (1, Some(1)) [t: Number] -> Number,
        "Ease-in-out that pulls back and overshoots.";
    CubicBezier "cubic_bezier" (5, Some(5))
        [x1: Number, y1: Number, x2: Number, y2: Number, t: Number] -> Number,
        "CSS `cubic-bezier(x1, y1, x2, y2)` at progress `t`.";
    Spring "spring" (1, Some(3)) [t: Number, stiffness: Number, damping: Number] -> Number,
        "Spring from 0 to 1, `t` seconds after release, e.g. `spring((frame - 12) / fps)`. Stiffness defaults to 170 and damping to 26.";
}

pub fn builtin_for_name(name: &str) -> Option<&'static BuiltinSpec> {
    BUILTINS.iter().find(|spec| spec.name == name)
}

impl BuiltinFn {
    pub fn spec(self) -> &'static BuiltinSpec {
        BUILTINS
            .iter()
            .find(|spec| spec.builtin == self)
            .expect("every builtin is declared in BUILTINS")
    }

    pub fn name(self) -> &'static str {
        self.spec().name
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalSpec {
    pub name: &'static str,
    pub doc: &'static str,
}

pub const GLOBALS: &[GlobalSpec] = &[
    GlobalSpec {
        name: "frame",
        doc: "Current frame number.",
    },
    GlobalSpec {
        name: "time",
        doc: "Current time in seconds.",
    },
    GlobalSpec {
        name: "fps",
        doc: "Frames per second.",
    },
    GlobalSpec {
        name: "width",
        doc: "Render width in pixels.",
    },
    GlobalSpec {
        name: "height",
        doc: "Render height in pixels.",
    },
];

#[cfg(test)]
mod tests {
    use super::{BUILTINS, GLOBALS, builtin_for_name};
    use crate::expr::{ExprNode, Expression, GlobalVar};

    #[test]
    fn every_builtin_appears_exactly_once() {
        for spec in BUILTINS {
            let same_name = BUILTINS.iter().filter(|other| other.name == spec.name);
            assert_eq!(same_name.count(), 1, "{}", spec.name);
            assert_eq!(builtin_for_name(spec.name), Some(spec));
            assert_eq!(spec.builtin.spec(), spec);
            match spec.max_args {
                Some(max) => assert!(max == spec.args.len() && spec.min_args <= max),
                None => assert!(!spec.args.is_empty()),
            }
        }
    }

    #[test]
    fn globals_parse_to_builtin_variables() {
        for global in GLOBALS {
            let ast = Expression::parse(global.name).unwrap().ast;
            assert!(
                matches!(ast, ExprNode::Global(ref var) if !matches!(var, GlobalVar::Custom(_))),
                "{}",
                global.name
            );
        }
    }
}
